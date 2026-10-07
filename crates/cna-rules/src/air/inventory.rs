//! Deterministic setup import and a single transactional aggregate update path.
//!
//! This slice models inventory only. It does not authorize missions, maintenance,
//! pilot reassignment or reinforcement arrivals. Those steps still stop under
//! the full profile until their procedures exist.

use std::collections::BTreeMap;

use cna_core::engine::EngineError;
use cna_protocol::Side;

use super::state::{AirRuntime, AircraftState, PilotId, PilotState, PlaneId};
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
            ids.push(runtime.insert_aircraft(AircraftState {
                aircraft: aircraft.into(),
                force: force.into(),
                squadron: None,
                facility: None,
                refitted: false,
                fuelled: false,
                armed: false,
            })?);
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
/// Cases: airlog:34.0, airlog:35.24, scen:59.32
/// Interpretations: interp:air-0005
pub fn initialize(content: &CnaContent, state: &mut State) -> Result<(), EngineError> {
    if !state.setup.closed || !state.setup.tasks.is_empty() {
        return Err(invalid("setup must close before inventory import"));
    }
    if state.air.runtime.initialized {
        return check(content, &state.air);
    }
    if state.air.runtime != AirRuntime::default() {
        return Err(invalid("uninitialized runtime contains records"));
    }
    let mut runtime = AirRuntime::default();
    for (force, pool) in &state.air.forces {
        side(force)?;
        if pool.sgsu_available < 0 {
            return Err(invalid("negative SGSU reserve"));
        }
        import_planes(&mut runtime, force, None, None, &pool.planes)?;
        import_pilots(&mut runtime, force, None, None, &pool.pilots)?;
    }
    for (id, squadron) in &state.air.squadrons {
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
    let mut draft = state.air.clone();
    draft.runtime = runtime;
    check(content, &draft)?;
    state.air = draft;
    Ok(())
}

fn validate_id(
    id: &str,
    side: Side,
    kind: &str,
    serials: &BTreeMap<Side, u64>,
) -> Result<(), EngineError> {
    let prefix = format!("{}.{kind}-", side_name(side));
    let serial = id
        .strip_prefix(&prefix)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0 && *value <= serials.get(&side).copied().unwrap_or(0));
    if serial.is_none() || id != format!("{prefix}{}", serial.unwrap_or(0)) {
        return Err(invalid("invalid persistent identity"));
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
            || squadron.facility.is_empty()
            || side(&squadron.force)? != squadron.side
            || !air.forces.contains_key(&squadron.force)
        {
            return Err(invalid("invalid squadron identity or owner"));
        }
    }
    let mut draft = air.clone();
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
        check_assignment(air, &plane.force, plane.squadron.as_deref())?;
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
        check_assignment(air, &pilot.force, pilot.squadron.as_deref())?;
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
    if !air.runtime.initialized {
        return Err(invalid("inventory has not been imported"));
    }
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
    check(content, air)?;
    let mut draft = air.clone();
    edit(&mut draft.runtime)?;
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
    if !draft.runtime.initialized {
        return Err(invalid("an initialized inventory cannot be reset"));
    }
    draft = mirrors(content, &draft)?;
    *air = draft;
    Ok(())
}

#[cfg(test)]
mod tests;
