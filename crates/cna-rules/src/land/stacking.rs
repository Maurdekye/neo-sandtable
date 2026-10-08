//! Terrain and road capacity count represented formations, including their shell equivalents.
use super::formation;
use crate::steps::illegal;
use crate::{CnaContent, State};
use cna_content::map::Survey;
use cna_core::{
    engine::{EngineError, Rejection},
    ids::{HexId, UnitId},
};
use cna_protocol::Side;
use cna_tables::land::terrain::{StackingLimit, TerrainFeature as F};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

/// Count one represented counter per root. Attached first-line trucks add no points.
/// Cases: land:9.11, land:9.12, land:9.13, land:9.21, land:9.29
pub fn halves(content: &CnaContent, state: &State, hex: &HexId, side: Side) -> i32 {
    formation::roots(content, state, hex, side)
        .iter()
        .map(|id| formation::stacking_halves(content, state, id))
        .sum()
}
/// Network transit is limited to five stacking points for vehicles. Off-road counters do not count.
/// Cases: land:8.34, land:9.29, land:9.33, land:9.34
pub fn road_halves(
    content: &CnaContent,
    state: &State,
    hex: &HexId,
    side: Side,
    excluded: &[UnitId],
) -> Result<i32, EngineError> {
    road_occupancy_halves(content, state, hex, side, excluded, None)
}

fn road_invariant() -> EngineError {
    EngineError::Invariant {
        detail: "invalid road occupancy or truck-point count".into(),
    }
}

fn checked_road_sum(mut values: impl Iterator<Item = i32>) -> Result<i32, EngineError> {
    values.try_fold(0_i32, |total, n| {
        if n < 0 {
            return Err(road_invariant());
        }
        total.checked_add(n).ok_or_else(road_invariant)
    })
}

/// Convoy chart blocks contribute to the same five-point transit limit as formations.
/// Attached first-line trucks stay included only through their represented formation.
/// Cases: land:9.29, land:9.33, land:9.34
/// Interpretations: interp:land-0040
pub fn road_occupancy_halves(
    content: &CnaContent,
    state: &State,
    hex: &HexId,
    side: Side,
    excluded_units: &[UnitId],
    excluded_pool: Option<&str>,
) -> Result<i32, EngineError> {
    let units = checked_road_sum(
        formation::roots(content, state, hex, side)
            .iter()
            .filter(|id| !excluded_units.contains(id) && !state.land.movement.off_road.contains(id))
            .map(|id| formation::stacking_halves(content, state, id)),
    )?;
    combine_road_occupancy(content, state, hex, side, excluded_pool, units)
}

fn combine_road_occupancy(
    content: &CnaContent,
    state: &State,
    hex: &HexId,
    side: Side,
    excluded_pool: Option<&str>,
    units: i32,
) -> Result<i32, EngineError> {
    let mut ids = BTreeSet::new();
    let mut total = units;
    for pool in state
        .logistics
        .truck_pools
        .iter()
        .filter(|p| p.side == side)
    {
        if pool.id.is_empty() || !ids.insert(&pool.id) {
            return Err(road_invariant());
        }
        if excluded_pool == Some(pool.id.as_str())
            || pool.location.as_ref().and_then(|l| l.hex()) != Some(hex)
            || state.land.movement.pool_off_road.contains(&pool.id)
        {
            continue;
        }
        let points = checked_road_sum(
            [pool.trucks.light, pool.trucks.medium, pool.trucks.heavy].into_iter(),
        )?;
        let halves = content
            .tables
            .land
            .stacking_values
            .blocks_halves(
                cna_tables::land::administration::OrganizationLevel::TruckPointsInConvoy,
                points,
            )
            .ok_or_else(road_invariant)?;
        total = total.checked_add(halves).ok_or_else(road_invariant)?;
    }
    Ok(total)
}

/// The mover joins the occupied road space only when it will use the network.
/// Cases: land:9.33
pub fn road_over_limit(occupied: i32, moving: i32) -> Result<bool, EngineError> {
    Ok(checked_road_sum([occupied, moving].into_iter())? > 10)
}
/// A unit may pass an overfull ordinary hex, but may never finish its move there.
/// Zero-point independent companies/batteries have a separate five-counter limit.
/// Cases: land:9.14, land:9.25, land:9.31, land:9.32
/// Cases: land:9.16
/// Unsupported: land:9.16 - garrison assignments and airfield exemptions need placement data.
pub fn validate_end(
    content: &CnaContent,
    state: &State,
    hex: &HexId,
    side: Side,
    strict: bool,
) -> Result<(), Rejection> {
    stacking_limit(content, hex)?;
    let roots = formation::roots(content, state, hex, side);
    let counters: Vec<_> = roots
        .iter()
        .map(|id| Counter::new(content, state, id))
        .collect();
    validate_counters(content, hex, strict, counters.iter())
}
#[derive(Clone)]
struct Counter {
    halves: i32,
    zero: bool,
    anti_air: bool,
    immobile_plus: bool,
    garrison: bool,
    on_network: bool,
}
impl Counter {
    fn new(content: &CnaContent, state: &State, id: &UnitId) -> Self {
        let class = formation::class(content, id);
        Self {
            halves: formation::stacking_halves(content, state, id),
            zero: content.units.units[id].stacking_points == Some(0)
                && class.is_some_and(|c| c.unit_type != "headquarters"),
            anti_air: class.is_some_and(|c| c.unit_type == "anti_air"),
            immobile_plus: class.is_some_and(|c| c.cpa == 0 && c.cpa_plus)
                && state.land.units[id].transport_trucks.total() == 0,
            garrison: content.units.units[id].sheet.contains("garrison"),
            on_network: !state.land.movement.off_road.contains(id),
        }
    }
}
fn validate_counters<'a>(
    content: &CnaContent,
    hex: &HexId,
    strict: bool,
    counters: impl Iterator<Item = &'a Counter>,
) -> Result<(), Rejection> {
    let (terrain, limit) = stacking_limit(content, hex)?;
    let mut zero = 0;
    let mut counted = 0;
    for counter in counters {
        zero += usize::from(counter.zero);
        if counter.immobile_plus || (counter.anti_air && terrain == F::MajorCity) {
            continue;
        }
        if strict && (counter.garrison || counter.anti_air) {
            return Err(Rejection::Engine(EngineError::Unsupported {
                case: "land:9.16".into(),
                detail: "garrison/airfield stacking exemption placement is not digitized".into(),
            }));
        }
        counted += counter.halves;
    }
    if counted > limit || (terrain != F::MajorCity && zero > 5) {
        return Err(illegal("destination exceeds the stacking limit"));
    }
    Ok(())
}
fn stacking_limit(content: &CnaContent, hex: &HexId) -> Result<(F, i32), Rejection> {
    let terrain: F = match content.map.terrain_survey(hex) {
        Survey::Present(name) => serde_json::from_value(serde_json::Value::String(name.into()))
            .map_err(|_| illegal("terrain class has no stacking limit"))?,
        _ => {
            return Err(Rejection::Engine(EngineError::Unsupported {
                case: "land:8.37".into(),
                detail: "terrain not yet digitized".into(),
            }));
        }
    };
    let limit = match content
        .tables
        .land
        .terrain_effects
        .feature(terrain)
        .stacking_limit
    {
        StackingLimit::Points(n) => n * 2,
        _ => return Err(illegal("terrain class has no stacking limit")),
    };
    Ok((terrain, limit))
}
/// A single-query memo of fixed friendly counters. Enemy strength is never inspected.
/// The base state is captured after the selected component's detachment; only its moving
/// subtree changes during this search. A later query rebuilds this structure from scratch.
/// Cases: land:9.12, land:9.21, land:9.25, land:9.31, land:9.33
pub(super) struct PlanningStacks<'a> {
    content: &'a CnaContent,
    base: &'a State,
    side: Side,
    excluded: &'a [UnitId],
    moving: Vec<Counter>,
    fixed: RefCell<BTreeMap<HexId, Vec<Counter>>>,
}
impl<'a> PlanningStacks<'a> {
    pub(super) fn new(
        content: &'a CnaContent,
        base: &'a State,
        side: Side,
        moving: &'a [UnitId],
        origin: &HexId,
    ) -> Self {
        let counters = formation::roots(content, base, origin, side)
            .into_iter()
            .filter(|id| moving.contains(id))
            .map(|id| Counter::new(content, base, &id))
            .collect();
        Self {
            content,
            base,
            side,
            excluded: moving,
            moving: counters,
            fixed: RefCell::new(BTreeMap::new()),
        }
    }
    fn ensure(&self, hex: &HexId) {
        if self.fixed.borrow().contains_key(hex) {
            return;
        }
        let counters = formation::roots(self.content, self.base, hex, self.side)
            .into_iter()
            .filter(|id| !self.excluded.contains(id))
            .map(|id| Counter::new(self.content, self.base, &id))
            .collect();
        self.fixed.borrow_mut().insert(hex.clone(), counters);
    }
    pub(super) fn moving_halves(&self) -> Result<i32, EngineError> {
        checked_road_sum(self.moving.iter().map(|c| c.halves))
    }
    pub(super) fn road_halves(&self, hex: &HexId) -> Result<i32, EngineError> {
        self.ensure(hex);
        let units = checked_road_sum(
            self.fixed.borrow()[hex]
                .iter()
                .filter(|c| c.on_network)
                .map(|c| c.halves),
        )?;
        combine_road_occupancy(self.content, self.base, hex, self.side, None, units)
    }
    pub(super) fn validate_end(&self, hex: &HexId, strict: bool) -> Result<(), Rejection> {
        self.ensure(hex);
        let fixed = self.fixed.borrow();
        validate_counters(
            self.content,
            hex,
            strict,
            fixed[hex].iter().chain(self.moving.iter()),
        )
    }
}
