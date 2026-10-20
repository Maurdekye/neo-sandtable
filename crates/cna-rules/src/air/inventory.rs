//! Deterministic setup import and a single transactional aggregate update path.
//!
//! This slice models inventory only. It does not authorize missions, maintenance,
//! pilot reassignment or reinforcement arrivals. Those steps still stop under
//! the full profile until their procedures exist.

use std::collections::{BTreeMap, BTreeSet};

use cna_core::engine::EngineError;
use cna_protocol::Side;

use super::{
    facilities::{self, FacilityCapacity, FacilityId, FacilityOrigin, FacilityState},
    preparation::{self, CurrentPreparation, InitialChoice},
    sgsu::{SgsuId, SgsuPosition, SgsuState},
    state::{AirRuntime, AircraftState, PilotId, PilotState, PlaneId},
};
use crate::{
    CnaContent, State,
    state::{AirState, PlaneCount},
};

fn invalid(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("air inventory: {detail}"),
    }
}

fn side(force: &str) -> Result<Side, EngineError> {
    match force {
        "axis" => Ok(Side::Axis),
        "commonwealth" | "malta" => Ok(Side::Commonwealth),
        _ => Err(invalid("unknown force")),
    }
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Axis => "axis",
        Side::Commonwealth => "commonwealth",
    }
}

fn allocate(
    serials: &mut BTreeMap<Side, u64>,
    side: Side,
    kind: &str,
) -> Result<String, EngineError> {
    let serial = serials.entry(side).or_default();
    *serial = serial
        .checked_add(1)
        .ok_or_else(|| invalid("IDs exhausted"))?;
    Ok(format!("{}.{kind}-{serial}", side_name(side)))
}

impl AirRuntime {
    /// Allocate a never-reused, side-local identity in a transactional draft.
    /// No rule entitlement is implied; the calling procedure validates arrivals.
    /// Cases: airlog:34.0
    pub fn insert_aircraft(&mut self, aircraft: AircraftState) -> Result<PlaneId, EngineError> {
        let id = PlaneId(allocate(
            &mut self.plane_serial,
            side(&aircraft.force)?,
            "aircraft",
        )?);
        if self.aircraft.contains_key(&id) {
            return Err(invalid("aircraft identity collision"));
        }
        self.aircraft.insert(id.clone(), aircraft);
        Ok(id)
    }

    /// Allocate only a rated pilot; zero-rated pilots remain implicit.
    /// Cases: airlog:34.0, airlog:35.24
    /// Source case: airlog:40.16
    pub fn insert_pilot(&mut self, pilot: PilotState) -> Result<PilotId, EngineError> {
        if !matches!(pilot.rating, 1 | 2 | 3 | 4 | 6) {
            return Err(invalid("invalid rated pilot"));
        }
        let id = PilotId(allocate(
            &mut self.pilot_serial,
            side(&pilot.force)?,
            "pilot",
        )?);
        if self.pilots.contains_key(&id) {
            return Err(invalid("pilot identity collision"));
        }
        self.pilots.insert(id.clone(), pilot);
        Ok(id)
    }
}

/// Receive a calendar-authorized batch into the unassigned force inventory.
/// The caller owns schedule/quota/type eligibility and invokes this only from
/// its atomic finish draft. These records add total only: no facility, SGSU,
/// refit, fuel or ammunition entitlement. Setup-era aggregates use their
/// existing caller-owned branch until canonical inventory has been imported.
/// Cases: airlog:34.8, airlog:34.84
pub fn receive_unassigned(
    content: &CnaContent,
    air: &mut AirState,
    owner: Side,
    aircraft: &str,
    count: i32,
) -> Result<Vec<PlaneId>, EngineError> {
    if count <= 0 || !air.runtime.initialized() {
        return Err(invalid(
            "arrival needs a positive count and imported inventory",
        ));
    }
    if !content.units.aircraft.contains_key(aircraft) {
        return Err(EngineError::Unsupported {
            case: "airlog:34.84".into(),
            detail: "Calendar-authorized aircraft content is missing".into(),
        });
    }
    let force = crate::state::side_key(owner);
    let pool = air
        .forces
        .get(force)
        .ok_or_else(|| invalid("arrival force pool is absent"))?;
    pool.planes
        .get(aircraft)
        .map_or(0, |p| p.total)
        .checked_add(count)
        .ok_or_else(|| invalid("arrival aircraft total overflow"))?;
    let mut ids = Vec::new();
    update(content, air, |runtime| {
        for _ in 0..count {
            let id = runtime.insert_aircraft(AircraftState {
                aircraft: aircraft.into(),
                force: force.into(),
                squadron: None,
                facility: None,
                refitted: false,
                fuelled: false,
                armed: false,
            })?;
            runtime
                .preparation
                .current
                .insert(id.clone(), CurrentPreparation::Unprepared);
            ids.push(id);
        }
        Ok(())
    })?;
    Ok(ids)
}

fn check_count(count: PlaneCount) -> Result<(), EngineError> {
    if count.total < 0
        || count.ready < 0
        || count.ready > count.total
        || ![0, count.total].contains(&count.fuelled)
        || ![0, count.total].contains(&count.armed)
    {
        return Err(invalid(
            "initial aggregate cannot identify individual readiness flags",
        ));
    }
    Ok(())
}

fn import_planes(
    runtime: &mut AirRuntime,
    force: &str,
    squadron: Option<&str>,
    facility: Option<&str>,
    planes: &BTreeMap<String, PlaneCount>,
) -> Result<(), EngineError> {
    for (aircraft, count) in planes {
        check_count(*count)?;
        for i in 0..count.total {
            runtime.insert_aircraft(AircraftState {
                aircraft: aircraft.clone(),
                force: force.into(),
                squadron: squadron.map(str::to_owned),
                facility: facility.map(str::to_owned),
                refitted: i < count.ready,
                fuelled: count.fuelled > 0,
                armed: count.armed > 0,
            })?;
        }
    }
    Ok(())
}

fn import_pilots(
    runtime: &mut AirRuntime,
    force: &str,
    squadron: Option<&str>,
    trained_aircraft: Option<&str>,
    pilots: &BTreeMap<u8, i32>,
) -> Result<(), EngineError> {
    for (rating, count) in pilots {
        if *count < 0 || !matches!(rating, 1 | 2 | 3 | 4 | 6) {
            return Err(invalid("invalid initial pilot roster"));
        }
        for _ in 0..*count {
            runtime.insert_pilot(PilotState {
                force: force.into(),
                squadron: squadron.map(str::to_owned),
                rating: *rating,
                trained_aircraft: trained_aircraft.map(str::to_owned),
            })?;
        }
    }
    Ok(())
}

/// Import once, after all setup choices and adjudication have closed. Source
/// counts are preserved exactly, including unplaced force reserves. Initial
/// fuel/arming is uniform per source cohort; partial legacy counts cannot reveal
/// overlap and are rejected rather than assigned an invented correlation.
/// This generic import creates only legacy-unknown preparation, never initial
/// credit. Actual fresh setup authentication requires a separate reviewed path.
/// Cases: airlog:34.0, airlog:35.24, scen:59.32
/// Interpretations: interp:air-0005
pub fn initialize(content: &CnaContent, state: &mut State) -> Result<(), EngineError> {
    if !state.setup.closed || !state.setup.tasks.is_empty() {
        return Err(invalid("setup must close before inventory import"));
    }
    if state.air.runtime.initialized {
        check_preparation(content, &state.air.runtime)?;
        if state.air.runtime.bases_initialized {
            check(content, &state.air)?;
            if state.air.runtime.preparation.initialized {
                return Ok(());
            }
            let mut draft = state.air.clone();
            migrate_preparation(content, &mut draft.runtime)?;
            check(content, &draft)?;
            state.air = draft;
            return Ok(());
        }
        // Legacy inventory is already canonical. Import only bases in a draft;
        // do not allocate or recreate a single aircraft, pilot or serial.
        let projected = mirrors(content, &state.air)?;
        if projected.forces != state.air.forces || projected.squadrons != state.air.squadrons {
            return Err(invalid(
                "legacy inventory mirrors disagree before base migration",
            ));
        }
        let mut draft = state.air.clone();
        import_bases(content, &mut draft)?;
        migrate_preparation(content, &mut draft.runtime)?;
        check(content, &draft)?;
        state.air = draft;
        return Ok(());
    }
    state.air = initial_inventory(content, &state.air, InitialAdmission::Legacy)?;
    Ok(())
}

// The fresh variant is reachable only from the consumed setup-close authority.
// Ordinary initialization and checkpoint migration always use Legacy.
enum InitialAdmission {
    Legacy,
    FreshSetup,
}

fn initial_inventory(
    content: &CnaContent,
    air: &AirState,
    admission: InitialAdmission,
) -> Result<AirState, EngineError> {
    if air.runtime != AirRuntime::default() {
        return Err(invalid("uninitialized runtime contains records"));
    }
    let mut runtime = AirRuntime::default();
    for (force, pool) in &air.forces {
        side(force)?;
        if pool.sgsu_available < 0 {
            return Err(invalid("negative SGSU reserve"));
        }
        import_planes(&mut runtime, force, None, None, &pool.planes)?;
        import_pilots(&mut runtime, force, None, None, &pool.pilots)?;
    }
    for (id, squadron) in &air.squadrons {
        if id != &squadron.id
            || side(&squadron.force)? != squadron.side
            || squadron.facility.is_empty()
        {
            return Err(invalid("invalid squadron identity or owner"));
        }
        import_planes(
            &mut runtime,
            &squadron.force,
            Some(id),
            Some(&squadron.facility),
            &squadron.planes,
        )?;
        let types: Vec<_> = squadron
            .planes
            .iter()
            .filter(|(_, count)| count.total > 0)
            .map(|(aircraft, _)| aircraft.as_str())
            .collect();
        let trained = if types.len() == 1 {
            Some(types[0])
        } else {
            None
        };
        import_pilots(
            &mut runtime,
            &squadron.force,
            Some(id),
            trained,
            &squadron.pilots,
        )?;
    }
    runtime.initialized = true;
    let mut draft = air.clone();
    draft.runtime = runtime;
    import_bases(content, &mut draft)?;
    match admission {
        InitialAdmission::Legacy => migrate_preparation(content, &mut draft.runtime)?,
        InitialAdmission::FreshSetup => {
            // These are newly allocated original identities. No UnknownLegacy
            // record is converted, and no selected load is inferred.
            for id in draft.runtime.aircraft.keys() {
                draft
                    .runtime
                    .preparation
                    .initial_choices
                    .insert(id.clone(), InitialChoice::Unselected);
                draft
                    .runtime
                    .preparation
                    .current
                    .insert(id.clone(), CurrentPreparation::InitialUnspecified);
            }
            draft.runtime.preparation.initialized = true;
        }
    }
    check(content, &draft)?;
    Ok(draft)
}

/// Admit original aircraft directly at the authenticated successful setup close.
/// The authority is private to setup's final finish branch and consumed once;
/// repeated source/provenance checks also reject stale or altered authority.
/// No mode, load, supply payment or operational eligibility is granted here.
/// Cases: scen:59.2, scen:59.32, scen:59.36, airlog:34.0, airlog:35.24
/// Interpretations: interp:air-0018
pub(crate) fn import_initial_setup(
    content: &CnaContent,
    state: &mut State,
    authority: crate::setup::FreshAirSetupClose,
) -> Result<(), EngineError> {
    if !state.setup.closed || !state.setup.tasks.is_empty() {
        return Err(invalid("fresh inventory requires completed setup closure"));
    }
    crate::setup::validate_initial_air_close(content, state, authority.original_aircraft())?;
    let draft = initial_inventory(content, &state.air, InitialAdmission::FreshSetup)?;
    // Both assignments are infallible. All Air and setup validation above took
    // place before either is published; the enclosing whole-State draft also
    // discards this import if the subsequent convoy close fails.
    state.air = draft;
    state.setup.initial_air_import = crate::setup::InitialAirImport::Imported;
    Ok(())
}

/// Source-backed initial bases, including checkpoint migration. Unresolved
/// catalog entries remain unknown; Malta aggregate capacity is not a site.
/// Cases: airlog:35.11, airlog:36.0, airlog:44.14
fn import_bases(content: &CnaContent, air: &mut AirState) -> Result<(), EngineError> {
    if air.runtime.bases_initialized {
        return Err(invalid("base import cannot be repeated"));
    }
    if !air.runtime.facilities.is_empty()
        || !air.runtime.sgsus.is_empty()
        || air.runtime.site_incarnation_serial != 0
    {
        return Err(invalid("uninitialized base maps contain records"));
    }
    for source in facilities::catalog(content)?.facilities {
        if source.force == "malta" {
            continue;
        }
        let id = FacilityId(source.id);
        let site = FacilityState {
            incarnation: None,
            origin: FacilityOrigin::Scenario,
            owner: source.side,
            current_capacity: source
                .limit
                .map_or(FacilityCapacity::Unlimited, FacilityCapacity::Levels),
            upgraded_kind: None,
            project_unavailable: false,
        };
        site.properties(content, &id)?;
        if air.runtime.facilities.insert(id, site).is_some() {
            return Err(invalid("duplicate source facility identity"));
        }
    }
    for (id, squadron) in &air.squadrons {
        if squadron.force == "malta" {
            continue;
        }
        let facility = FacilityId(squadron.facility.clone());
        if !air.runtime.facilities.contains_key(&facility) {
            return Err(EngineError::Unsupported {
                case: "airlog:35.11".into(),
                detail: "Initial SGSU facility has no verified canonical source".into(),
            });
        }
        air.runtime.sgsus.insert(
            SgsuId(id.clone()),
            SgsuState {
                force: squadron.force.clone(),
                nationality: squadron.nationality.clone(),
                position: SgsuPosition::Facility(facility),
                stores_paid_game_turn: None,
                fuel_water_paid: None,
            },
        );
    }
    // This happens only in a disposable draft. Publication follows full check.
    air.runtime.bases_initialized = true;
    Ok(())
}

/// Validate canonical bases and project SGSU aggregates from their one truth.
/// Force SGSU reserves remain the pre-existing unplaced count until arrivals
/// have individual source identities. Malta rosters remain non-SGSU groups.
/// Cases: airlog:35.11, airlog:35.14, airlog:36.0, airlog:44.14
fn base_mirrors(content: &CnaContent, air: &AirState) -> Result<AirState, EngineError> {
    let mut draft = air.clone();
    let mut tokens = BTreeSet::new();
    for (id, site) in &air.runtime.facilities {
        site.properties(content, id)?;
        if let Some(token) = site.incarnation
            && (token.serial() == 0
                || token.serial() > air.runtime.site_incarnation_serial
                || !tokens.insert(token))
        {
            return Err(invalid("invalid or duplicate facility incarnation"));
        }
    }
    for (id, unit) in &air.runtime.sgsus {
        if id.0.is_empty()
            || unit.nationality.is_empty()
            || !air.forces.contains_key(&unit.force)
            || unit.stores_paid_game_turn == Some(0)
            || unit
                .fuel_water_paid
                .is_some_and(|p| p.game_turn == 0 || !(1..=3).contains(&p.op_stage))
        {
            return Err(invalid("invalid canonical SGSU record"));
        }
        let squadron = draft
            .squadrons
            .get_mut(&id.0)
            .ok_or_else(|| invalid("SGSU has no corresponding squadron identity"))?;
        if squadron.force == "malta" {
            return Err(invalid("Malta roster grouping cannot become an SGSU"));
        }
        unit.location(content, &air.runtime.facilities)?;
        squadron.force = unit.force.clone();
        squadron.side = unit.side()?;
        squadron.nationality = unit.nationality.clone();
        squadron.facility = match &unit.position {
            SgsuPosition::Facility(id) => id.0.clone(),
            SgsuPosition::Ground(_) => String::new(),
        };
    }
    // Removed actual SGSUs remove their mirror roster too. Aircraft/pilots
    // must already have been reassigned or resolved by the owning procedure;
    // the subsequent assignment check rejects dangling identities atomically.
    draft.squadrons.retain(|id, squadron| {
        squadron.force == "malta" || air.runtime.sgsus.contains_key(&SgsuId(id.clone()))
    });
    for plane in air.runtime.aircraft.values() {
        let Some(id) = &plane.facility else {
            continue;
        };
        // A legacy Malta grouping has no flight/maintenance entitlement. This
        // exception preserves inventory only while individual sites are unknown.
        if plane.force == "malta"
            && facilities::catalog(content)?
                .facilities
                .iter()
                .any(|site| site.force == "malta" && site.id == *id)
        {
            continue;
        }
        let facility = FacilityId(id.clone());
        let site = air
            .runtime
            .facilities
            .get(&facility)
            .ok_or_else(|| invalid("aircraft physical facility is not canonical"))?;
        let properties = site.properties(content, &facility)?;
        if site.removed(&properties) {
            return Err(invalid("aircraft still references a removed facility"));
        }
    }
    Ok(draft)
}

fn validate_id(
    id: &str,
    side: Side,
    kind: &str,
    serials: &BTreeMap<Side, u64>,
) -> Result<u64, EngineError> {
    let prefix = format!("{}.{kind}-", side_name(side));
    let serial = id
        .strip_prefix(&prefix)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0 && *value <= serials.get(&side).copied().unwrap_or(0));
    let serial = serial.ok_or_else(|| invalid("invalid persistent identity"))?;
    if id != format!("{prefix}{serial}") {
        return Err(invalid("invalid persistent identity"));
    }
    Ok(serial)
}

/// Historical ledger keys use the same canonical syntax and serial bounds as
/// live identities; an eliminated ID does not require a surviving force record.
fn plane_identity(id: &PlaneId, serials: &BTreeMap<Side, u64>) -> Result<(Side, u64), EngineError> {
    let owner = if id.0.starts_with("axis.") {
        Side::Axis
    } else if id.0.starts_with("commonwealth.") {
        Side::Commonwealth
    } else {
        return Err(invalid("invalid aircraft identity side"));
    };
    Ok((owner, validate_id(&id.0, owner, "aircraft", serials)?))
}

/// Validate explicit records independently of the one-time marker admission.
/// This establishes consistency only, never payment or fresh setup authority.
/// Cases: airlog:34.0, airlog:34.14, scen:59.32
/// Interpretations: interp:air-0018
fn check_preparation_records(
    content: &CnaContent,
    runtime: &AirRuntime,
) -> Result<(), EngineError> {
    let prep = &runtime.preparation;
    for id in prep.initial_choices.keys() {
        plane_identity(id, &runtime.plane_serial)?;
    }
    if prep
        .current
        .keys()
        .any(|id| !runtime.aircraft.contains_key(id))
    {
        return Err(invalid("preparation references an absent aircraft"));
    }
    for (id, plane) in &runtime.aircraft {
        let current = prep
            .current
            .get(id)
            .ok_or_else(|| invalid("live aircraft preparation is missing"))?;
        let valid = matches!(
            (prep.initial_choices.get(id), current),
            (
                Some(InitialChoice::Unselected),
                CurrentPreparation::InitialUnspecified
            ) | (
                Some(InitialChoice::Committed | InitialChoice::Forfeited),
                CurrentPreparation::Unprepared
                    | CurrentPreparation::KnownAmmunition { .. }
                    | CurrentPreparation::KnownLoad { .. }
            ) | (
                None,
                CurrentPreparation::UnknownLegacy
                    | CurrentPreparation::Unprepared
                    | CurrentPreparation::KnownFuel { .. }
                    | CurrentPreparation::KnownAmmunition { .. }
                    | CurrentPreparation::KnownLoad { .. }
            )
        );
        if !valid {
            return Err(invalid("initial credit and current preparation disagree"));
        }
        if let CurrentPreparation::KnownLoad { load } = current {
            preparation::validate_load(content, plane, load)?;
        }
        if let CurrentPreparation::KnownAmmunition { knowledge } = current {
            knowledge.check(content, plane)?;
        }
        if let Some((mode, _)) = preparation::known_fuel(current) {
            let source = content
                .units
                .aircraft
                .get(&plane.aircraft)
                .ok_or_else(|| invalid("known fuel has no aircraft source"))?;
            super::maintenance::fuel_points(source, mode)?;
        }
        if let Some(available) = preparation::fuel_availability(current)
            && plane.fuelled != available
        {
            return Err(invalid("typed fuel and informational fuel mirror disagree"));
        }
        if preparation::armed_projection(current, plane.armed) != plane.armed {
            return Err(invalid("informational arming projection disagrees"));
        }
    }
    Ok(())
}

/// False/empty is global legacy knowledge; pure validation never migrates.
/// Cases: airlog:34.0, scen:59.32
/// Interpretations: interp:air-0018
fn check_preparation(content: &CnaContent, runtime: &AirRuntime) -> Result<(), EngineError> {
    if !runtime.preparation.initialized {
        if !runtime.preparation.initial_choices.is_empty()
            || !runtime.preparation.current.is_empty()
        {
            return Err(invalid("legacy preparation marker has populated maps"));
        }
        return Ok(());
    }
    check_preparation_records(content, runtime)
}

/// Private one-time no-credit migration in the enclosing disposable Air draft.
/// No source setup, load, refit, assignment or allocation is inferred.
/// Cases: airlog:34.0, scen:59.32
/// Interpretations: interp:air-0018
fn migrate_preparation(content: &CnaContent, runtime: &mut AirRuntime) -> Result<(), EngineError> {
    check_preparation(content, runtime)?;
    if runtime.preparation.initialized {
        return Ok(());
    }
    for id in runtime.aircraft.keys() {
        runtime
            .preparation
            .current
            .insert(id.clone(), CurrentPreparation::UnknownLegacy);
    }
    check_preparation_records(content, runtime)?;
    runtime.preparation.initialized = true;
    Ok(())
}

/// Compare the post-bootstrap/pre-callback truth with the edited draft. This
/// admits no generic ledger writer or setup registration. Exact private initial
/// and paid servicing admissions authorize only their bound before-to-final edit.
/// Restored known loads and edits of existing known records remain structurally
/// valid; creating paid fuel knowledge requires the exact finish admission.
/// Cases: airlog:34.0, scen:59.32
/// Interpretations: interp:air-0018
fn check_preparation_transition(
    old: &AirRuntime,
    new: &AirRuntime,
    paid: Option<&super::maintenance::window::fueling::PaidServicingAdmission>,
    initial: Option<&super::missions::initial::InitialLoadAdmission>,
) -> Result<(), EngineError> {
    if !new.preparation.initialized {
        return Err(invalid("preparation initialized marker cannot reset"));
    }
    if old.preparation.initial_choices != new.preparation.initial_choices
        && initial.is_none_or(|admission| !admission.permits_history(old, new))
    {
        return Err(invalid(
            "ordinary update cannot edit initial credit history",
        ));
    }
    if (old.maintenance.arming_receipts != new.maintenance.arming_receipts
        || old.maintenance.gun_charges != new.maintenance.gun_charges)
        && paid.is_none_or(|p| !p.permits_history(old, new))
    {
        return Err(invalid(
            "ordinary update cannot establish paid arming history",
        ));
    }
    for (id, plane) in &new.aircraft {
        let current = new.preparation.current.get(id);
        if let Some(previous) = old.aircraft.get(id) {
            if previous.aircraft != plane.aircraft || previous.force != plane.force {
                return Err(invalid("persistent aircraft type and force cannot change"));
            }
            let before = old.preparation.current.get(id);
            if paid.is_some_and(|p| p.permits(id, old, new)) {
                continue;
            }
            if initial.is_some_and(|admission| admission.permits_plane(id, old, new)) {
                continue;
            }
            if (matches!(current, Some(CurrentPreparation::KnownAmmunition { .. }))
                || matches!(before, Some(CurrentPreparation::KnownAmmunition { .. })))
                && before != current
            {
                return Err(invalid(
                    "ordinary update cannot change partial ammunition knowledge",
                ));
            }
            if let (
                Some(CurrentPreparation::KnownLoad { load: prior }),
                Some(CurrentPreparation::KnownLoad { load: next }),
            ) = (before, current)
                && ((!prior.gun_ammunition && next.gun_ammunition)
                    || (next.ordnance.is_some() && prior.ordnance != next.ordnance))
            {
                return Err(invalid("ordinary update cannot establish paid ammunition"));
            }
            let prior_fuel = before.and_then(preparation::known_fuel);
            let next_fuel = current.and_then(preparation::known_fuel);
            if (!previous.fuelled && plane.fuelled)
                || next_fuel.is_some_and(|(mode, available)| {
                    available
                        && prior_fuel.is_none_or(|(old_mode, old_available)| {
                            !old_available || old_mode != mode
                        })
                })
                || prior_fuel.is_some_and(|(mode, available)| {
                    available && next_fuel.is_some_and(|(next, _)| mode != next)
                })
                || (matches!(current, Some(CurrentPreparation::KnownFuel { .. }))
                    && !matches!(before, Some(CurrentPreparation::KnownFuel { .. })))
            {
                return Err(invalid(
                    "ordinary update cannot establish or change paid fuel",
                ));
            }
            if matches!(before, Some(CurrentPreparation::KnownFuel { .. }))
                && !matches!(current, Some(CurrentPreparation::KnownFuel { .. }))
            {
                return Err(invalid(
                    "ordinary update cannot replace partial fuel knowledge",
                ));
            }
            match (before, current) {
                (
                    Some(CurrentPreparation::UnknownLegacy | CurrentPreparation::Unprepared),
                    Some(CurrentPreparation::KnownLoad { .. }),
                ) => return Err(invalid("ordinary update cannot establish a known load")),
                (
                    Some(CurrentPreparation::KnownLoad { .. }),
                    None | Some(CurrentPreparation::UnknownLegacy | CurrentPreparation::Unprepared),
                )
                | (Some(CurrentPreparation::Unprepared), Some(CurrentPreparation::UnknownLegacy))
                | (Some(CurrentPreparation::UnknownLegacy), Some(CurrentPreparation::Unprepared)) =>
                {
                    return Err(invalid(
                        "ordinary update cannot downgrade preparation knowledge",
                    ));
                }
                _ => {}
            }
            if matches!(current, Some(CurrentPreparation::UnknownLegacy))
                && !matches!(before, Some(CurrentPreparation::UnknownLegacy))
            {
                return Err(invalid("unknown preparation requires private migration"));
            }
            if matches!(
                before,
                Some(CurrentPreparation::UnknownLegacy | CurrentPreparation::InitialUnspecified)
            ) && before == current
                && previous.armed != plane.armed
            {
                return Err(invalid(
                    "unknown or initial coarse arming must be preserved",
                ));
            }
        } else {
            let (owner, serial) = plane_identity(id, &new.plane_serial)?;
            if serial <= old.plane_serial.get(&owner).copied().unwrap_or(0) {
                return Err(invalid("historical aircraft identity cannot be reinserted"));
            }
            if matches!(current, Some(CurrentPreparation::UnknownLegacy)) {
                return Err(invalid("new aircraft cannot fabricate legacy preparation"));
            }
            if plane.fuelled
                || matches!(
                    current,
                    Some(
                        CurrentPreparation::KnownLoad { .. }
                            | CurrentPreparation::KnownFuel { .. }
                            | CurrentPreparation::KnownAmmunition { .. }
                    )
                )
            {
                return Err(invalid("new aircraft cannot establish a known load"));
            }
        }
    }
    Ok(())
}

/// The sole informational projection write; it never supplies mission rights.
fn project_preparation(old: &AirRuntime, new: &mut AirRuntime) -> Result<(), EngineError> {
    for (id, plane) in &mut new.aircraft {
        let current = new
            .preparation
            .current
            .get(id)
            .ok_or_else(|| invalid("live aircraft preparation is missing"))?;
        let old_bit = old.aircraft.get(id).map_or(plane.armed, |p| p.armed);
        plane.armed = preparation::armed_projection(current, old_bit);
        if let Some(available) = preparation::fuel_availability(current) {
            plane.fuelled = available;
        }
    }
    Ok(())
}

fn check_assignment(
    air: &AirState,
    force: &str,
    squadron: Option<&str>,
) -> Result<(), EngineError> {
    if !air.forces.contains_key(force) {
        return Err(invalid("missing force pool"));
    }
    if let Some(id) = squadron
        && air
            .squadrons
            .get(id)
            .is_none_or(|s| s.id != id || s.force != force || side(force).ok() != Some(s.side))
    {
        return Err(invalid("squadron assignment is not owned by this force"));
    }
    Ok(())
}

fn increment(value: &mut i32, amount: i32) -> Result<(), EngineError> {
    *value = value
        .checked_add(amount)
        .ok_or_else(|| invalid("aggregate overflow"))?;
    Ok(())
}

fn mirrors(content: &CnaContent, air: &AirState) -> Result<AirState, EngineError> {
    for (force, pool) in &air.forces {
        side(force)?;
        if pool.sgsu_available < 0 {
            return Err(invalid("negative SGSU reserve"));
        }
    }
    for (id, squadron) in &air.squadrons {
        if id != &squadron.id
            || (!air.runtime.bases_initialized && squadron.facility.is_empty())
            || side(&squadron.force)? != squadron.side
            || !air.forces.contains_key(&squadron.force)
        {
            return Err(invalid("invalid squadron identity or owner"));
        }
    }
    let mut draft = if air.runtime.bases_initialized {
        base_mirrors(content, air)?
    } else {
        if !air.runtime.facilities.is_empty() || !air.runtime.sgsus.is_empty() {
            return Err(invalid("uninitialized base maps contain records"));
        }
        air.clone()
    };
    for pool in draft.forces.values_mut() {
        for count in pool.planes.values_mut() {
            *count = PlaneCount::default();
        }
        for count in pool.pilots.values_mut() {
            *count = 0;
        }
    }
    for squadron in draft.squadrons.values_mut() {
        for count in squadron.planes.values_mut() {
            *count = PlaneCount::default();
        }
        for count in squadron.pilots.values_mut() {
            *count = 0;
        }
    }
    for (id, plane) in &air.runtime.aircraft {
        validate_id(
            &id.0,
            side(&plane.force)?,
            "aircraft",
            &air.runtime.plane_serial,
        )?;
        check_assignment(&draft, &plane.force, plane.squadron.as_deref())?;
        if !content.units.aircraft.contains_key(&plane.aircraft)
            || plane.facility.as_ref().is_some_and(String::is_empty)
            || (plane.squadron.is_some() && plane.facility.is_none())
        {
            return Err(invalid("unknown aircraft or missing physical facility"));
        }
        let counts = if let Some(squadron) = &plane.squadron {
            &mut draft
                .squadrons
                .get_mut(squadron)
                .expect("assignment checked")
                .planes
        } else {
            &mut draft
                .forces
                .get_mut(&plane.force)
                .expect("force checked")
                .planes
        };
        let count = counts.entry(plane.aircraft.clone()).or_default();
        increment(&mut count.total, 1)?;
        increment(&mut count.ready, i32::from(plane.refitted))?;
        increment(&mut count.fuelled, i32::from(plane.fuelled))?;
        increment(&mut count.armed, i32::from(plane.armed))?;
    }
    for (id, pilot) in &air.runtime.pilots {
        validate_id(
            &id.0,
            side(&pilot.force)?,
            "pilot",
            &air.runtime.pilot_serial,
        )?;
        check_assignment(&draft, &pilot.force, pilot.squadron.as_deref())?;
        if !matches!(pilot.rating, 1 | 2 | 3 | 4 | 6)
            || pilot
                .trained_aircraft
                .as_ref()
                .is_some_and(|id| !content.units.aircraft.contains_key(id))
        {
            return Err(invalid("invalid pilot rating or training type"));
        }
        let pilots = if let Some(squadron) = &pilot.squadron {
            &mut draft
                .squadrons
                .get_mut(squadron)
                .expect("assignment checked")
                .pilots
        } else {
            &mut draft
                .forces
                .get_mut(&pilot.force)
                .expect("force checked")
                .pilots
        };
        increment(pilots.entry(pilot.rating).or_default(), 1)?;
    }
    Ok(draft)
}

/// Verify that every individual record belongs to a known owned assignment and
/// that each aggregate is exactly its inventory projection. No flight eligibility
/// or supply entitlement is inferred from these consistency checks.
/// Cases: airlog:34.0, airlog:35.24
pub fn check(content: &CnaContent, air: &AirState) -> Result<(), EngineError> {
    super::maintenance::refit::check(&air.runtime.maintenance)?;
    if !air.runtime.initialized || !air.runtime.bases_initialized {
        return Err(invalid("inventory and bases must be imported"));
    }
    check_preparation(content, &air.runtime)?;
    let draft = mirrors(content, air)?;
    if air.forces != draft.forces || air.squadrons != draft.squadrons {
        return Err(invalid(
            "aggregate mirrors disagree with individual inventory",
        ));
    }
    Ok(())
}

/// The sole operational inventory update path. The caller validates its rules
/// entitlement, changes individual records in a draft, and this helper rebuilds
/// all mirrors before publishing anything. A failed edit or invalid draft leaves
/// every original record, aggregate and serial untouched.
/// Cases: airlog:34.0, airlog:35.24
pub fn update<F>(content: &CnaContent, air: &mut AirState, edit: F) -> Result<(), EngineError>
where
    F: FnOnce(&mut AirRuntime) -> Result<(), EngineError>,
{
    update_inner(content, air, None, None, None, edit)
}

/// Compatibility wrapper for exact post-debit servicing. No caller edit
/// closure is accepted, so unrelated edits cannot ride this permission.
/// Cases: airlog:38.21, airlog:38.24
pub(in crate::air) fn update_fueling(
    content: &CnaContent,
    air: &mut AirState,
    paid: super::maintenance::window::fueling::PaidServicingAdmission,
) -> Result<(), EngineError> {
    update_inner(content, air, None, Some(&paid), None, |runtime| {
        paid.apply(runtime)
    })
}

/// Exact combined fuel/ammunition admission from the successful shared debit.
/// Cases: airlog:38.21, airlog:38.43, airlog:38.44
pub(in crate::air) fn update_servicing(
    content: &CnaContent,
    air: &mut AirState,
    paid: super::maintenance::window::fueling::PaidServicingAdmission,
) -> Result<(), EngineError> {
    update_fueling(content, air, paid)
}

/// Exact one-time source-backed initial declaration choice. No caller callback
/// or setup-registration authority is accepted by this seam.
/// Cases: scen:59.32, scen:59.36, airlog:39.12
/// Interpretations: interp:air-0018
pub(in crate::air) fn update_initial_loads(
    content: &CnaContent,
    air: &mut AirState,
    initial: super::missions::initial::InitialLoadAdmission,
) -> Result<(), EngineError> {
    update_inner(content, air, None, None, Some(&initial), |runtime| {
        initial.apply(runtime)
    })
}

/// Internal permission for exactly one canonical physical-site allocation.
/// It is not serialized, answer-provided or a project authentication receipt.
pub(super) enum SiteAllocation {
    Establish(FacilityId),
    Create(FacilityId),
}

/// Same aggregate mutation path, restricted to the trusted new-project seam.
pub(super) fn update_site<F>(
    content: &CnaContent,
    air: &mut AirState,
    allocation: SiteAllocation,
    edit: F,
) -> Result<(), EngineError>
where
    F: FnOnce(&mut AirRuntime) -> Result<(), EngineError>,
{
    update_inner(content, air, Some(allocation), None, None, edit)
}

fn check_site_transition(
    content: &CnaContent,
    old: &AirRuntime,
    new: &AirRuntime,
    allocation: Option<&SiteAllocation>,
) -> Result<(), EngineError> {
    let target = match allocation {
        Some(SiteAllocation::Establish(id) | SiteAllocation::Create(id)) => Some(id),
        None => None,
    };
    let expected = if target.is_some() {
        old.site_incarnation_serial
            .checked_add(1)
            .ok_or_else(|| invalid("facility incarnation serial overflow"))?
    } else {
        old.site_incarnation_serial
    };
    if new.site_incarnation_serial != expected {
        return Err(invalid(
            "facility serial changed outside canonical allocation",
        ));
    }
    for (id, site) in &new.facilities {
        if target == Some(id) {
            if site.incarnation.map(|token| token.serial()) != Some(expected) {
                return Err(invalid("new facility token does not match allocation"));
            }
            match allocation.expect("target has allocation") {
                SiteAllocation::Establish(_) => {
                    let before = old
                        .facilities
                        .get(id)
                        .ok_or_else(|| invalid("legacy establishment cannot create a facility"))?;
                    if before.incarnation.is_some() || before.origin != site.origin {
                        return Err(invalid("legacy establishment changed physical identity"));
                    }
                }
                SiteAllocation::Create(_) => {
                    if !matches!(site.origin, FacilityOrigin::Constructed { .. }) {
                        return Err(invalid("new physical instance must be constructed"));
                    }
                    if let Some(before) = old.facilities.get(id) {
                        let properties = before.properties(content, id)?;
                        if !matches!(before.origin, FacilityOrigin::Constructed { .. })
                            || !before.removed(&properties)
                            || before.project_unavailable
                        {
                            return Err(invalid("replacement cannot revive a surviving site"));
                        }
                    }
                }
            }
        } else {
            let before = old
                .facilities
                .get(id)
                .ok_or_else(|| invalid("generic inventory callback cannot create a facility"))?;
            if before.incarnation != site.incarnation || before.origin != site.origin {
                return Err(invalid(
                    "facility incarnation cannot be cleared, reused or changed",
                ));
            }
            let old_properties = before.properties(content, id)?;
            let new_properties = site.properties(content, id)?;
            if before.removed(&old_properties) && !site.removed(&new_properties) {
                return Err(invalid("removed facility needs a fresh canonical instance"));
            }
        }
    }
    if target.is_some_and(|id| !new.facilities.contains_key(id)) {
        return Err(invalid("allocated facility is absent"));
    }
    Ok(())
}

fn update_inner<F>(
    content: &CnaContent,
    air: &mut AirState,
    allocation: Option<SiteAllocation>,
    paid: Option<&super::maintenance::window::fueling::PaidServicingAdmission>,
    initial: Option<&super::missions::initial::InitialLoadAdmission>,
    edit: F,
) -> Result<(), EngineError>
where
    F: FnOnce(&mut AirRuntime) -> Result<(), EngineError>,
{
    check(content, air)?;
    let mut draft = air.clone();
    migrate_preparation(content, &mut draft.runtime)?;
    let before_edit = draft.runtime.clone();
    edit(&mut draft.runtime)?;
    check_site_transition(content, &air.runtime, &draft.runtime, allocation.as_ref())?;
    super::maintenance::refit::check_transition(
        &air.runtime.maintenance,
        &draft.runtime.maintenance,
    )?;
    if [
        (&air.runtime.plane_serial, &draft.runtime.plane_serial),
        (&air.runtime.pilot_serial, &draft.runtime.pilot_serial),
    ]
    .iter()
    .any(|(old, new)| {
        old.iter()
            .any(|(side, serial)| new.get(side).copied().unwrap_or(0) < *serial)
    }) {
        return Err(invalid("persistent serials cannot move backward"));
    }
    if !draft.runtime.initialized || !draft.runtime.bases_initialized {
        return Err(invalid("initialized inventory and bases cannot be reset"));
    }
    check_preparation_transition(&before_edit, &draft.runtime, paid, initial)?;
    project_preparation(&before_edit, &mut draft.runtime)?;
    check_preparation(content, &draft.runtime)?;
    draft = mirrors(content, &draft)?;
    check(content, &draft)?;
    *air = draft;
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod preparation_tests;

#[cfg(test)]
mod initial_setup_tests;
