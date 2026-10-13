//! Fixed two-Air finite refit/dues batches. No load producer or flight rights.
use std::collections::{BTreeMap, BTreeSet};

use cna_core::{
    decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::SeatId,
    quantity::{FuelTenths, StoresPoints, WaterPoints},
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role, Side};
use cna_tables::airlog::air::{Nationality, SquadronKind};
use serde_json::{Value, json};

use super::{
    invalid,
    refit::{self, MaintenancePeriod, RefitMethod, RefitPlan},
};
use crate::air::{
    designation::{self, Family},
    facilities::{FacilityCapacity, FacilityId, FacilityTheatre},
    inventory,
    sgsu::{SgsuId, SgsuOpStage, SgsuPosition},
    state::PlaneId,
};
use crate::logistics::{
    SupplyDemand, SupplyError, SupplySource,
    air_supply::{self, AirSupplyDebit, AirSupplyError, AirSupplyUse},
};
use crate::{
    CnaContent, State,
    state::{Location, Pending},
    steps::{illegal, open},
};

pub const TACTICAL: &str = "opstage.land_support_air.maintenance";
pub const STRATEGIC: &str = "strategic_air_recovery.maintenance";
pub const KIND: &str = "cna.air.finite_refit";
const MISSING: &str = "fuel/gun/ordnance preparation, exempt facility consumers, cross-servicer Stores allocation, mixed-method cohorts and sandstorm refit loss selection";

fn unsupported(case: &str, detail: &str) -> EngineError {
    EngineError::Unsupported {
        case: case.into(),
        detail: detail.into(),
    }
}
fn owned(force: &str, side: Side) -> bool {
    match side {
        Side::Axis => force == "axis",
        Side::Commonwealth => force == "commonwealth",
    }
}
fn period(state: &State) -> Result<MaintenancePeriod, EngineError> {
    let current = match state.cursor.anchor() {
        TACTICAL => MaintenancePeriod::Tactical {
            game_turn: state.cursor.game_turn,
            op_stage: state
                .cursor
                .op_stage
                .ok_or_else(|| invalid("maintenance has no stage"))?,
        },
        STRATEGIC
            if state.cursor.block == crate::seq::Block::Post && state.cursor.op_stage.is_none() =>
        {
            MaintenancePeriod::Strategic {
                game_turn: state.cursor.game_turn,
            }
        }
        _ => {
            return Err(invalid(
                "maintenance cursor does not identify a fixed anchor",
            ));
        }
    };
    current.check()?;
    Ok(current)
}
fn dues_stage(current: MaintenancePeriod) -> Result<SgsuOpStage, EngineError> {
    // POST is reached only after stage3 in the actual cursor. Identity alone
    // grants no paid credit: operation_paid below must match stage3 exactly.
    let completed = match current {
        MaintenancePeriod::Strategic { game_turn } => Some(SgsuOpStage {
            game_turn,
            op_stage: 3,
        }),
        _ => None,
    };
    current.dues_stage(completed)
}
fn first_bar(content: &CnaContent, current: MaintenancePeriod) -> bool {
    matches!(current, MaintenancePeriod::Tactical { game_turn, op_stage }
        if game_turn == content.scenario.meta.start.gt
            && op_stage == content.scenario.meta.start.opstage)
}
fn choice(ids: impl IntoIterator<Item = String>) -> ActionSchema {
    ActionSchema::Choice {
        options: ids
            .into_iter()
            .map(|id| ChoiceOption {
                label: id.clone(),
                id,
                detail: None,
            })
            .collect(),
    }
}
fn field(name: &str, schema: ActionSchema) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        doc: name.into(),
        schema,
        optional: false,
    }
}
fn list(item: ActionSchema, max: u32) -> ActionSchema {
    ActionSchema::List {
        item: Box::new(item),
        min: 0,
        max,
    }
}
fn space(state: &State, owner: Side) -> ActionSpace {
    // Exact own roster and stock explain the ID choices without exposing an
    // opponent's planes, plans, receipts, project flags or stock quantities.
    let roster: BTreeMap<_, _> = state
        .air
        .runtime
        .aircraft
        .iter()
        .filter(|(_, p)| owned(&p.force, owner))
        .map(|(id, p)| {
            (
                id.0.clone(),
                json!({
                    "aircraft": p.aircraft, "assigned_squadron": p.squadron,
                    "physical_facility": p.facility, "refitted": p.refitted,
                }),
            )
        })
        .collect();
    let consumers: BTreeMap<_, _> = state
        .air
        .runtime
        .sgsus
        .iter()
        .filter(|(_, s)| s.side().ok() == Some(owner))
        .map(|(id, s)| {
            (
                id.0.clone(),
                json!({
                    "nationality": s.nationality, "position": s.position,
                    "stores_paid_game_turn": s.stores_paid_game_turn,
                    "fuel_water_paid": s.fuel_water_paid,
                }),
            )
        })
        .collect();
    let holdings: BTreeMap<_, _> = state
        .logistics
        .air_dumps
        .iter()
        .filter(|(_, d)| d.side == owner)
        .map(|(id, d)| {
            (
                id.clone(),
                json!({
                    "facility": d.facility, "supplies": d.supplies,
                }),
            )
        })
        .collect();
    let planes: Vec<_> = state
        .air
        .runtime
        .aircraft
        .iter()
        .filter(|(_, p)| owned(&p.force, owner))
        .map(|(id, _)| id.0.clone())
        .collect();
    let sgsus: Vec<_> = state
        .air
        .runtime
        .sgsus
        .iter()
        .filter(|(_, s)| s.side().ok() == Some(owner))
        .map(|(id, _)| id.0.clone())
        .collect();
    let squadrons: Vec<_> = state
        .air
        .squadrons
        .values()
        .filter(|s| s.side == owner)
        .map(|s| s.id.clone())
        .collect();
    let dumps: Vec<_> = state
        .logistics
        .air_dumps
        .iter()
        .filter(|(_, d)| d.side == owner)
        .map(|(id, _)| id.clone())
        .collect();
    let plane_max = u32::try_from(planes.len()).unwrap_or(u32::MAX);
    let sgsu_max = u32::try_from(sgsus.len()).unwrap_or(u32::MAX);
    let dump_max = u32::try_from(dumps.len()).unwrap_or(u32::MAX);
    let amount = ActionSchema::Record {
        fields: vec![
            field(
                "fuel",
                ActionSchema::Integer {
                    min: 0,
                    max: i64::from(i32::MAX),
                },
            ),
            field("ammo", ActionSchema::Integer { min: 0, max: 0 }),
            field(
                "stores",
                ActionSchema::Integer {
                    min: 0,
                    max: i64::from(i32::MAX),
                },
            ),
            field(
                "water",
                ActionSchema::Integer {
                    min: 0,
                    max: i64::from(i32::MAX),
                },
            ),
        ],
    };
    let draw = ActionSchema::Record {
        fields: vec![
            field(
                "source",
                ActionSchema::Record {
                    fields: vec![
                        field("source", choice(["air_dump".into()])),
                        field("id", choice(dumps)),
                    ],
                },
            ),
            field("amount", amount),
        ],
    };
    ActionSpace::new(ActionSchema::Record {
        fields: vec![
            field("dues", list(choice(sgsus.clone()), sgsu_max)),
            field(
                "cohorts",
                list(
                    ActionSchema::Record {
                        fields: vec![
                            field("servicer", choice(sgsus.clone())),
                            field("assigned_squadron", choice(squadrons)),
                            field("priority", list(choice(planes), plane_max)),
                            field("method", choice(["squadron".into(), "per_plane".into()])),
                        ],
                    },
                    plane_max,
                ),
            ),
            field(
                "draws",
                list(
                    ActionSchema::Record {
                        fields: vec![
                            field("servicer", choice(sgsus)),
                            field("draws", list(draw, dump_max)),
                        ],
                    },
                    sgsu_max,
                ),
            ),
        ],
    })
    .with_pass("Perform no finite refit or SGSU dues payment in this maintenance period.")
    .with_context(
        json!({"planes": roster, "sgsus": consumers, "air_dumps": holdings,
            "fuel_draw_unit": "tenths of a Fuel Point",
            "scope": "Refit and SGSU dues only; no aircraft fuel or ammunition loading."}),
    )
}

/// Fixed owning roles, never hidden-eligibility scheduling. Full uniformly
/// stops before inspecting either roster while legal procedure shapes are absent.
/// Cases: airlog:35.14, airlog:38.32, scen:59.36
pub fn enter(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    if strict {
        return Err(unsupported("airlog:38.1", MISSING));
    }
    let current = period(state)?;
    let mut draft = state.clone();
    inventory::initialize(content, &mut draft)?;
    let previous = &draft.air.runtime.maintenance;
    if previous.period == Some(current) {
        if previous.finished {
            return Ok(());
        }
        return Err(invalid("finite refit window entered twice"));
    }
    if previous.period.is_some() && !previous.finished {
        return Err(invalid("finite refit rollover before finish"));
    }
    inventory::update(content, &mut draft.air, |runtime| {
        // Prior period receipts are retained as history, not renewed credit.
        runtime.maintenance.period = Some(current);
        runtime.maintenance.closed.clear();
        runtime.maintenance.waiting.clear();
        runtime.maintenance.finished = false;
        Ok(())
    })?;
    *state = draft;
    for side in Side::ALL {
        cx.emit(EngineEvent::new(Audience::Side(side), GameEvent::Note {
            text: format!("Development maintenance supports finite SGSU refit/dues only; missing: {MISSING}. No load or flight permission follows."),
        }));
        let domain = space(state, side);
        open(
            state,
            cx,
            SeatId::new(side, Role::Air),
            KIND,
            "Choose finite refit cohorts in success-priority order and exact SGSU dues/draws."
                .into(),
            &["airlog:35.14", "airlog:38.33", "airlog:38.36"],
            Trigger::Scheduled,
            Secrecy::SecretSimultaneous,
            domain,
        );
    }
    Ok(())
}

fn supply_error(error: AirSupplyError) -> Rejection {
    match error {
        AirSupplyError::Canonical(e) => Rejection::Engine(e),
        AirSupplyError::Supply(SupplyError::Unsupported { case }) => {
            Rejection::Engine(unsupported(case, "finite refit supply source unavailable"))
        }
        AirSupplyError::Supply(SupplyError::Invalid | SupplyError::Insufficient) => {
            illegal("invalid or insufficient own finite refit stock allocation")
        }
        AirSupplyError::Supply(SupplyError::UnknownFuelRate) => Rejection::Engine(invalid(
            "unexpected fuel-rate lookup for finite Air maintenance",
        )),
    }
}

/// An ordinary refusal is valid during Respond. After plans were accepted it
/// denotes an inconsistent adjudication draft; trusted engine errors keep their
/// exact source case and diagnostic at both boundaries.
fn adjudication_error(error: Rejection) -> EngineError {
    match error {
        Rejection::Engine(error) => error,
        Rejection::Illegal { message } => invalid(format!(
            "accepted finite refit plan became invalid: {message}"
        )),
        other => invalid(format!(
            "unexpected finite refit adjudication refusal: {other}"
        )),
    }
}
fn nationality(nation: &str) -> Result<Nationality, EngineError> {
    match nation {
        "it" => Ok(Nationality::Italian),
        "ge" => Ok(Nationality::German),
        "cw" => Ok(Nationality::Commonwealth),
        _ => Err(unsupported(
            "airlog:38.35",
            "aircraft nationality has no reviewed refit binding",
        )),
    }
}
fn capacity(content: &CnaContent, state: &State, nation: &str) -> Result<u32, EngineError> {
    let date = crate::view::wire_clock(content, state).date;
    if date.len() != 10 {
        return Err(unsupported(
            "airlog:35.23",
            "maintenance capacity has no resolved campaign date",
        ));
    }
    let kind = match nation {
        "it" => SquadronKind::ItalianSquadriglia,
        "ge" => SquadronKind::GermanStaffel,
        "cw" if date.as_str() >= "1941-07-01" => SquadronKind::CommonwealthSquadron194243,
        "cw" => SquadronKind::CommonwealthSquadron194041,
        "us" if date.as_str() < "1942-08-01" => {
            return Err(unsupported(
                "airlog:35.18",
                "USAF SGSU unavailable before source date",
            ));
        }
        _ => {
            return Err(unsupported(
                "airlog:35.23",
                "SGSU nationality capacity not bound",
            ));
        }
    };
    let row = content.tables.airlog.squadron_capacity.capacity(kind);
    u32::try_from(
        row.ready
            .checked_add(row.reserve)
            .ok_or_else(|| invalid("SGSU capacity overflow"))?,
    )
    .map_err(|_| invalid("negative bound SGSU capacity"))
}

struct Validated {
    plans: RefitPlan,
    demands: BTreeMap<SgsuId, SupplyDemand>,
    prior: BTreeMap<SupplySource, FuelTenths>,
    groups: Vec<(refit::RefitCohort, Nationality)>,
    paid: SgsuOpStage,
}

/// Owner-only validation and stock preview, on disposable state. No RNG.
/// Choice failures are Illegal; canonical/content failures are Engine and are
/// never inferred from an Invariant diagnostic or relabelled as a bad answer.
fn validate(
    content: &CnaContent,
    state: &State,
    owner: Side,
    current: MaintenancePeriod,
    plan: &RefitPlan,
) -> Result<Validated, Rejection> {
    if first_bar(content, current) && !plan.cohorts.is_empty() {
        return Err(unsupported(
            "scen:59.36",
            "aircraft maintenance barred in first scenario OpStage",
        )
        .into());
    }
    let paid = dues_stage(current)?;
    let mut demands = BTreeMap::<SgsuId, SupplyDemand>::new();
    let mut limits = BTreeMap::new();
    let mut operation = BTreeSet::new();
    for id in &plan.dues {
        if !operation.insert(id.clone()) {
            return Err(illegal("duplicate SGSU dues choice"));
        }
        let s = state
            .air
            .runtime
            .sgsus
            .get(id)
            .filter(|s| s.side().ok() == Some(owner))
            .ok_or_else(|| illegal("unknown own SGSU"))?;
        if !matches!(&s.position, SgsuPosition::Facility(_)) {
            return Err(unsupported(
                "airlog:35.14",
                "SGSU ground-position stock access has no reviewed finite Air consumer",
            )
            .into());
        }
        let demand = demands.entry(id.clone()).or_default();
        if s.stores_paid_game_turn != Some(paid.game_turn) {
            demand.stores = StoresPoints::new(1);
        }
        if s.fuel_water_paid != Some(paid) {
            demand.fuel = FuelTenths::new(10);
            demand.water = WaterPoints::new(1);
        }
    }
    let previous = state
        .air
        .runtime
        .maintenance
        .receipts
        .get(&owner)
        .cloned()
        .unwrap_or_default();
    let mut groups: Vec<(refit::RefitCohort, Nationality)> = Vec::new();
    let mut methods = BTreeMap::new();
    let mut active: BTreeMap<FacilityId, BTreeSet<SgsuId>> = BTreeMap::new();
    for cohort in &plan.cohorts {
        if cohort.priority.is_empty() {
            continue;
        }
        let s = state
            .air
            .runtime
            .sgsus
            .get(&cohort.servicer)
            .filter(|s| s.side().ok() == Some(owner))
            .ok_or_else(|| illegal("unknown own refit SGSU"))?;
        let SgsuPosition::Facility(site_id) = &s.position else {
            return Err(
                unsupported("airlog:36.17", "finite refit needs exact facility SGSU").into(),
            );
        };
        let site = state
            .air
            .runtime
            .facilities
            .get(site_id)
            .ok_or_else(|| invalid("missing SGSU site"))?;
        let props = site.properties(content, site_id)?;
        if props.theatre != FacilityTheatre::Africa
            || !matches!(props.location, Location::Hex { .. })
        {
            return Err(unsupported(
                "airlog:36.5",
                "unlimited/exempt aircraft refit requires reviewed consumer extension",
            )
            .into());
        }
        if !site.operational() {
            return Err(illegal("own refit facility is unavailable"));
        }
        if !s.operation_paid(paid) && !operation.contains(&cohort.servicer) {
            return Err(illegal("refit SGSU dues not paid for actual period"));
        }
        let limit = capacity(content, state, &s.nationality)?;
        limits.insert(cohort.servicer.clone(), limit);
        active
            .entry(site_id.clone())
            .or_default()
            .insert(cohort.servicer.clone());
        let mut by_nation = BTreeMap::<Nationality, Vec<PlaneId>>::new();
        for id in &cohort.priority {
            let p = state
                .air
                .runtime
                .aircraft
                .get(id)
                .filter(|p| owned(&p.force, owner))
                .ok_or_else(|| illegal("unknown own refit plane"))?;
            if p.squadron.as_deref() != Some(cohort.assigned_squadron.as_str())
                || p.facility.as_deref() != Some(site_id.0.as_str())
            {
                return Err(illegal("plane assignment or actual refit site differs"));
            }
            let family = designation::squadron_family(state, &cohort.assigned_squadron)
                .ok_or_else(|| {
                    unsupported(
                        "airlog:38.32",
                        "maintenance class has no committed designation",
                    )
                })?;
            if matches!(
                (current, family),
                (
                    MaintenancePeriod::Tactical { .. },
                    Family::Convoy | Family::MaltaRaid
                ) | (MaintenancePeriod::Strategic { .. }, Family::LandSupport)
            ) {
                return Err(illegal("plane maintenance class differs from fixed period"));
            }
            let aircraft = content
                .units
                .aircraft
                .get(&p.aircraft)
                .ok_or_else(|| unsupported("airlog:38.35", "refit aircraft source missing"))?;
            if !props.kind.accepts(aircraft.role == "flying_boat") {
                return Err(illegal("refit aircraft incompatible with facility"));
            }
            by_nation
                .entry(nationality(&aircraft.nation)?)
                .or_default()
                .push(id.clone());
        }
        if methods
            .insert(cohort.assigned_squadron.clone(), cohort.method)
            .is_some_and(|previous| previous != cohort.method)
        {
            return Err(unsupported(
                "airlog:38.39",
                "mixed-method assigned-squadron refit needs a reviewed procedure shape",
            )
            .into());
        }
        for (nationality, priority) in by_nation {
            // Wire list partitioning cannot manufacture extra squadron dice
            // or extra percentage ceilings. Preserve the declared ID order.
            if let Some((group, _)) = groups.iter_mut().find(|(group, nation)| {
                group.servicer == cohort.servicer
                    && group.assigned_squadron == cohort.assigned_squadron
                    && *nation == nationality
            }) {
                group.priority.extend(priority);
            } else {
                let mut group = cohort.clone();
                group.priority = priority;
                groups.push((group, nationality));
            }
        }
    }
    // Retained actual attempts consume capacity even if no new plane succeeds.
    for receipt in previous.iter().filter(|r| r.period == current) {
        let id = &receipt.cohort.servicer;
        if !limits.contains_key(id) {
            let s = state
                .air
                .runtime
                .sgsus
                .get(id)
                .filter(|s| s.side().ok() == Some(owner))
                .ok_or_else(|| invalid("period receipt SGSU missing"))?;
            limits.insert(id.clone(), capacity(content, state, &s.nationality)?);
            if let SgsuPosition::Facility(site_id) = &s.position {
                active
                    .entry(site_id.clone())
                    .or_default()
                    .insert(id.clone());
            }
        }
    }
    for (id, consumers) in active {
        let site = state
            .air
            .runtime
            .facilities
            .get(&id)
            .ok_or_else(|| invalid("active refit site missing"))?;
        if let FacilityCapacity::Levels(levels) = site.current_capacity {
            let levels = usize::try_from(levels).map_err(|_| invalid("negative site levels"))?;
            if consumers.len() > levels {
                return Err(illegal(
                    "active servicing SGSUs exceed damaged facility capacity",
                ));
            }
        }
    }
    // Validate committed history as trusted data before checking answer-owned
    // duplicates/capacity. Never classify account_refit's Invariant by message.
    let historical = refit::account_refit(current, &previous, &[], &limits)?;
    let mut attempts = historical.attempted;
    let mut seen: BTreeSet<_> = attempts.values().flatten().cloned().collect();
    for cohort in &plan.cohorts {
        for id in &cohort.priority {
            if !seen.insert(id.clone()) {
                return Err(illegal(
                    "PlaneId already attempted in this maintenance period",
                ));
            }
            attempts
                .entry(cohort.servicer.clone())
                .or_default()
                .insert(id.clone());
        }
    }
    for (id, ids) in attempts {
        let limit = limits
            .get(&id)
            .ok_or_else(|| invalid("missing trusted actual-SGSU refit capacity"))?;
        if u64::try_from(ids.len()).map_err(|_| invalid("attempt count overflow"))?
            > u64::from(*limit)
        {
            return Err(illegal("refit attempts exceed SGSU ready+reserve"));
        }
    }
    let accounting = refit::account_refit(current, &previous, &plan.cohorts, &limits)?;
    for squadron in accounting.newly_charged_squadrons {
        let id = &plan
            .cohorts
            .iter()
            .find(|c| c.assigned_squadron == squadron && !c.priority.is_empty())
            .ok_or_else(|| invalid("charged squadron absent from plan"))?
            .servicer;
        let demand = demands.entry(id.clone()).or_default();
        demand.stores = StoresPoints::new(
            demand
                .stores
                .get()
                .checked_add(1)
                .ok_or_else(|| invalid("refit Stores overflow"))?,
        );
    }
    let mut prior = BTreeMap::<SupplySource, FuelTenths>::new();
    for receipt in state
        .air
        .runtime
        .maintenance
        .debits
        .get(&owner)
        .into_iter()
        .flatten()
    {
        for (source, amount) in &receipt.fuel_rounding {
            // Each receipt retains its own newly debited fuel, not a copied total.
            let current = prior.entry(source.clone()).or_default();
            *current = FuelTenths::new(
                current
                    .get()
                    .checked_add(amount.get())
                    .ok_or_else(|| invalid("trusted finite refit rounding overflow"))?,
            );
        }
    }
    let mut draw_ids = BTreeSet::new();
    let mut supply_draft = state.clone();
    for selected in &plan.draws {
        if !draw_ids.insert(selected.servicer.clone()) || !demands.contains_key(&selected.servicer)
        {
            return Err(illegal("duplicate or uncosted refit consumer draws"));
        }
    }
    for (id, demand) in &demands {
        let draws = plan
            .draws
            .iter()
            .find(|d| &d.servicer == id)
            .map(|d| d.draws.as_slice())
            .unwrap_or(&[]);
        let use_ = if operation.contains(id) {
            AirSupplyUse::SgsuOperation
        } else {
            AirSupplyUse::AircraftServicing
        };
        air_supply::spend(
            content,
            &mut supply_draft,
            owner,
            id,
            use_,
            AirSupplyDebit {
                demand: *demand,
                draws,
                prior: &prior,
            },
        )
        .map_err(supply_error)?;
        for draw in draws {
            let total = prior.entry(draw.source.clone()).or_default();
            *total = FuelTenths::new(
                total
                    .get()
                    .checked_add(draw.amount.fuel.get())
                    .ok_or_else(|| invalid("finite refit rounding overflow"))?,
            );
        }
    }
    groups.sort_by(|(a, an), (b, bn)| {
        (&a.servicer, &a.assigned_squadron, an).cmp(&(&b.servicer, &b.assigned_squadron, bn))
    });
    Ok(Validated {
        plans: plan.clone(),
        demands,
        prior: BTreeMap::new(),
        groups,
        paid,
    })
}

/// Acceptance reads only the owner's identities/stock and public content/cursor.
/// No stock, dues, aircraft or RNG change; the accepted answer is buffered only.
/// Cases: airlog:35.14, airlog:38.33, land:3.6
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
) -> Result<String, Rejection> {
    let current = period(state).map_err(Rejection::from)?;
    let owner = pending.seat.side;
    let window = &state.air.runtime.maintenance;
    if pending.kind != KIND
        || pending.seat.role != Role::Air
        || window.period != Some(current)
        || window.finished
        || window.closed.contains(&owner)
    {
        return Err(illegal("finite refit window is not open"));
    }
    space(state, owner).check(action).map_err(illegal)?;
    let plan = if action.is_null() {
        RefitPlan::default()
    } else {
        serde_json::from_value(action.clone()).map_err(|_| illegal("invalid own refit plan"))?
    };
    validate(content, state, owner, current, &plan)?;
    state.air.runtime.maintenance.waiting.insert(owner, plan);
    state.air.runtime.maintenance.closed.insert(owner);
    Ok("Recorded the private finite refit and SGSU dues plan.".into())
}

/// Whole-State plus disposable RNG commit. Validate both plans/allocations
/// before rolling; any later inventory error leaves state/RNG/events unchanged.
/// Resolved reentry returns before stock, RNG or receipt changes.
/// Cases: airlog:35.14, airlog:38.33, airlog:38.34, airlog:38.36
/// Interpretations: interp:airlog-0022, interp:airlog-0023, interp:airlog-0025
pub fn finish(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    let current = period(state)?;
    let window = &state.air.runtime.maintenance;
    if window.period != Some(current) {
        return Err(invalid("no current finite refit batch"));
    }
    if window.finished || Side::ALL.iter().any(|s| !window.closed.contains(s)) {
        return Ok(());
    }
    let mut draft = state.clone();
    let mut rng = (*cx.rng).clone();
    let mut validated = Vec::new();
    for side in Side::ALL {
        let plan = window
            .waiting
            .get(&side)
            .ok_or_else(|| invalid("closed refit batch missing plan"))?;
        validated.push((
            side,
            validate(content, &draft, side, current, plan).map_err(adjudication_error)?,
        ));
    }
    for (side, v) in &mut validated {
        for receipt in draft
            .air
            .runtime
            .maintenance
            .debits
            .get(side)
            .into_iter()
            .flatten()
        {
            for (source, amount) in &receipt.fuel_rounding {
                let total = v.prior.entry(source.clone()).or_default();
                *total = FuelTenths::new(
                    total
                        .get()
                        .checked_add(amount.get())
                        .ok_or_else(|| invalid("finite refit rounding overflow"))?,
                );
            }
        }
        for (id, demand) in &v.demands {
            let draws = v
                .plans
                .draws
                .iter()
                .find(|d| &d.servicer == id)
                .map(|d| d.draws.as_slice())
                .unwrap_or(&[]);
            let use_ = if v.plans.dues.contains(id) {
                AirSupplyUse::SgsuOperation
            } else {
                AirSupplyUse::AircraftServicing
            };
            air_supply::spend(
                content,
                &mut draft,
                *side,
                id,
                use_,
                AirSupplyDebit {
                    demand: *demand,
                    draws,
                    prior: &v.prior,
                },
            )
            .map_err(supply_error)
            .map_err(adjudication_error)?;
            for draw in draws {
                let total = v.prior.entry(draw.source.clone()).or_default();
                *total = FuelTenths::new(
                    total
                        .get()
                        .checked_add(draw.amount.fuel.get())
                        .ok_or_else(|| invalid("finite refit rounding overflow"))?,
                );
            }
        }
    }
    let mut results = BTreeMap::new();
    for (side, v) in &validated {
        let mut receipts = Vec::new();
        for (cohort, nation) in &v.groups {
            let assigned = cohort.servicer.0 == cohort.assigned_squadron;
            let successful = match cohort.method {
                RefitMethod::Squadron => {
                    let count = super::squadron_refitted(
                        &content.tables.airlog.aircraft_refit,
                        *nation,
                        assigned,
                        u32::try_from(cohort.priority.len())
                            .map_err(|_| invalid("cohort count overflow"))?,
                        rng.d6(),
                    )?;
                    refit::successful_ids(&cohort.priority, count)?
                }
                RefitMethod::PerPlane => cohort
                    .priority
                    .iter()
                    .filter_map(|id| {
                        let passed = content.tables.airlog.aircraft_refit.plane_refitted(
                            *nation,
                            i32::from(rng.two_dice().sum()),
                            assigned,
                        );
                        passed.then(|| id.clone())
                    })
                    .collect(),
            };
            receipts.push(refit::RefitReceipt {
                period: current,
                cohort: cohort.clone(),
                successful,
            });
        }
        results.insert(*side, receipts);
    }
    inventory::update(content, &mut draft.air, |runtime| {
        for (side, v) in &validated {
            for id in &v.plans.dues {
                let s = runtime
                    .sgsus
                    .get_mut(id)
                    .ok_or_else(|| invalid("dues SGSU disappeared"))?;
                s.stores_paid_game_turn = Some(v.paid.game_turn);
                s.fuel_water_paid = Some(v.paid);
            }
            for receipt in &results[side] {
                for id in &receipt.successful {
                    runtime
                        .aircraft
                        .get_mut(id)
                        .ok_or_else(|| invalid("refit plane disappeared"))?
                        .refitted = true;
                }
            }
            runtime
                .maintenance
                .receipts
                .entry(*side)
                .or_default()
                .extend(results[side].clone());
            for (id, demand) in &v.demands {
                if *demand == SupplyDemand::default() {
                    continue;
                }
                let draws = v
                    .plans
                    .draws
                    .iter()
                    .find(|d| &d.servicer == id)
                    .map(|d| d.draws.clone())
                    .unwrap_or_default();
                let fuel_rounding = draws
                    .iter()
                    .map(|d| (d.source.clone(), d.amount.fuel))
                    .collect();
                runtime
                    .maintenance
                    .debits
                    .entry(*side)
                    .or_default()
                    .push(refit::DebitReceipt {
                        servicer: id.clone(),
                        draws,
                        fuel_rounding,
                    });
            }
        }
        runtime.maintenance.waiting.clear();
        runtime.maintenance.finished = true;
        Ok(())
    })?;
    *state = draft;
    *cx.rng = rng;
    Ok(())
}

#[cfg(test)]
mod tests;
