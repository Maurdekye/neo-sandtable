//! Real-pool route preparation and trusted edge adjudication. The Logistics caller
//! owns payment, physical history, movement, breakdown and the complete transaction.
use super::{
    map::{self, StepCost},
    stacking, zoc,
};
use crate::{CnaContent, State, state::TruckPool, steps::illegal};
use cna_core::{
    engine::{EngineError, Rejection},
    ids::HexId,
};
use cna_protocol::Side;
use cna_tables::land::{administration::OrganizationLevel, weather::WeatherKind};

/// Internal preparation has no actor deserializer and makes no hidden-control query.
#[derive(Debug, Clone)]
pub struct PreparedPoolRoute {
    pub pool: String,
    pub origin: HexId,
    pub path: Vec<HexId>,
    pub costs: Vec<StepCost>,
}

#[derive(Debug, Clone)]
pub enum PoolEdge {
    Pass(StepCost),
    /// The caller retains the legal prefix, discards its suffix and finishes this stop.
    Blocked,
}

fn invariant(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: detail.into(),
    }
}

fn pool<'a>(s: &'a State, id: &str) -> Result<&'a TruckPool, EngineError> {
    let mut matches = s.logistics.truck_pools.iter().filter(|p| p.id == id);
    let p = matches
        .next()
        .ok_or_else(|| invariant("missing real convoy pool"))?;
    if id.is_empty() || matches.next().is_some() {
        return Err(invariant("duplicate or empty convoy identity"));
    }
    truck_halves_from_counts(&p.trucks, None)?;
    Ok(p)
}

fn truck_halves_from_counts(
    trucks: &cna_content::units::Trucks,
    content: Option<&CnaContent>,
) -> Result<i32, EngineError> {
    let total = [trucks.light, trucks.medium, trucks.heavy]
        .into_iter()
        .try_fold(0_i32, |sum, n| {
            if n < 0 {
                return Err(invariant("negative convoy truck count"));
            }
            sum.checked_add(n)
                .ok_or_else(|| invariant("convoy truck count overflow"))
        })?;
    match content {
        Some(c) => c
            .tables
            .land
            .stacking_values
            .blocks_halves(OrganizationLevel::TruckPointsInConvoy, total)
            .ok_or_else(|| invariant("convoy stacking chart is invalid")),
        None => Ok(total),
    }
}

fn cost(
    c: &CnaContent,
    s: &State,
    p: &TruckPool,
    from: &HexId,
    to: &HexId,
    strict: bool,
) -> Result<StepCost, Rejection> {
    let weather = crate::logistics::weather::at_hex(c, s, to).map_err(Rejection::Engine)?;
    let rain = weather == WeatherKind::Rainstorm;
    let mut cost = map::truck_step_cost(c, p.side, &p.trucks, from, to, strict, rain)?;
    if cost.on_network
        && stacking::road_over_limit(
            stacking::road_occupancy_halves(c, s, to, p.side, &[], Some(&p.id))
                .map_err(Rejection::Engine)?,
            truck_halves_from_counts(&p.trucks, Some(c)).map_err(Rejection::Engine)?,
        )
        .map_err(Rejection::Engine)?
    {
        cost =
            map::truck_step_cost_with_network(c, p.side, &p.trucks, from, to, strict, rain, false)?;
    }
    if weather == WeatherKind::Sandstorm {
        cost.cp_quarters = cost
            .cp_quarters
            .checked_mul(2)
            .ok_or_else(|| Rejection::Engine(invariant("convoy weather CP overflow")))?;
    }
    Ok(cost)
}

/// Validate only actor-known inventory, public presence and route prices.
/// No stock, funding, CP history, posture, pending decision or RNG is written.
/// Cases: land:8.13, land:8.37, land:9.29, land:9.33, land:10.24, land:29.57
pub fn prepare_pool_route(
    c: &CnaContent,
    s: &State,
    side: Side,
    id: &str,
    path: &[HexId],
    strict: bool,
) -> Result<PreparedPoolRoute, Rejection> {
    let own = s
        .logistics
        .truck_pools
        .iter()
        .filter(|p| p.id == id && p.side == side)
        .count();
    if own == 0 {
        return Err(illegal("unknown own convoy pool"));
    }
    let p = pool(s, id).map_err(Rejection::Engine)?;
    if truck_halves_from_counts(&p.trucks, None).map_err(Rejection::Engine)? == 0 {
        return Err(illegal("convoy has no trucks"));
    }
    let origin = p
        .location
        .as_ref()
        .and_then(|l| l.hex())
        .ok_or_else(|| illegal("convoy location is unresolved or off-map"))?
        .clone();
    if path.len() > 4096 {
        return Err(illegal("convoy path exceeds the order limit"));
    }
    let mut from = &origin;
    let mut costs = Vec::with_capacity(path.len());
    for to in path {
        if s.stack_presence(to, side.opponent()) {
            return Err(illegal("not a legal convoy destination"));
        }
        costs.push(cost(c, s, p, from, to, strict)?);
        from = to;
    }
    Ok(PreparedPoolRoute {
        pool: id.into(),
        origin,
        path: path.to_vec(),
        costs,
    })
}

/// Truth is consulted only by finish on its unpublished transaction draft.
/// Friendly combat coverage negates control; friendly noncombat presence does not.
/// Cases: land:10.24, land:10.26, land:10.29, land:9.33
pub fn adjudicate_pool_edge(
    c: &CnaContent,
    s: &State,
    id: &str,
    from: &HexId,
    to: &HexId,
    strict: bool,
) -> Result<PoolEdge, EngineError> {
    let p = pool(s, id)?;
    if p.location.as_ref().and_then(|l| l.hex()) != Some(from) {
        return Err(invariant("convoy edge does not start at its actual pool"));
    }
    if !c
        .map
        .get(from)
        .zip(c.map.get(to))
        .is_some_and(|(a, b)| a.axial.distance(b.axial) == 1)
    {
        return Err(invariant(
            "trusted convoy path is not an adjacent mapped edge",
        ));
    }
    if s.stack_presence(to, p.side.opponent()) {
        return Ok(PoolEdge::Blocked);
    }
    // Map/source failure remains typed; a newly prohibited edge ends the legal prefix.
    let cost = match cost(c, s, p, from, to, strict) {
        Ok(cost) => cost,
        Err(Rejection::Engine(error)) => return Err(error),
        Err(_) => return Ok(PoolEdge::Blocked),
    };
    let covered_to = zoc::friendly_combat(c, s, p.side, to);
    if !covered_to && zoc::controlled(c, s, p.side.opponent(), to, strict)? {
        return Ok(PoolEdge::Blocked);
    }
    // An uncovered destination ZOC is forbidden even when departure is uncontrolled.
    // This also prevents a ZOC-to-ZOC move, while10.26 coverage removes that control.
    Ok(PoolEdge::Pass(cost))
}

/// Once at trusted phase departure; not a Respond eligibility oracle or Engaged charge.
/// Cases: land:8.15, land:8.62, land:8.65, land:8.68, land:10.23, land:10.26
/// Interpretations: interp:land-0039
pub fn pool_departure_cp(
    c: &CnaContent,
    s: &State,
    id: &str,
    strict: bool,
) -> Result<i32, EngineError> {
    let p = pool(s, id)?;
    let hex = p
        .location
        .as_ref()
        .and_then(|l| l.hex())
        .ok_or_else(|| invariant("trusted convoy departure lacks a map location"))?;
    Ok(
        if !zoc::friendly_combat(c, s, p.side, hex)
            && zoc::controlled(c, s, p.side.opponent(), hex, strict)?
        {
            8
        } else {
            0
        },
    )
}

/// Call only after a successful edge, together with the physical location update.
/// Zero-edge orders never call this function. Removal/split writers have separate hooks.
/// Cases: land:9.34
pub fn record_pool_posture(s: &mut State, id: &str, on_network: bool) -> Result<(), EngineError> {
    pool(s, id)?;
    if on_network {
        s.land.movement.pool_off_road.remove(id);
    } else {
        s.land.movement.pool_off_road.insert(id.into());
    }
    Ok(())
}
