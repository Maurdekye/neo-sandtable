//! Handling supplies at the four Tripoli/Tunisia boxes records a private
//! OpStage movement ban. A new stage expires the ban without deleting history.
//! Transfers record goods totals, not an invented allocation to physical trucks.
//! Until division apportionment is implemented, full refuses a stamped carrier's
//! division; dev keeps its stamp and reports the unrestricted separated trucks.
use super::{SupplyError, rations::WaterStage};
use crate::state::{Location, State};
use cna_content::scenario::Supplies;
use cna_core::{
    engine::{Cx, EngineError},
    event::EngineEvent,
    ids::UnitId,
    visibility::Audience,
};
use cna_protocol::{GameEvent, Side};
use serde::{Deserialize, Serialize};

/// Owner-private history of actual cargo handling; tank refills are excluded.
/// Cases: land:8.88, land:3.6
/// Interpretations: interp:airlog-0019
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxHandling {
    pub stage: WaterStage,
    pub loaded: Supplies,
    pub unloaded: Supplies,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Carrier {
    Unit(UnitId),
    Pool(String),
}
const REASON: &str =
    "This carrier loaded or unloaded supplies at a Tripoli/Tunisia box this OpStage (8.88).";

/// Only these four boxes are supply dumps under the off-map rule.
/// Cases: land:8.88
pub fn is_supply_box(location: &Location) -> bool {
    matches!(location, Location::OffMap { id } if
        ["box_tripoli", "box_tripolitania", "box_gabes", "box_tunis"].contains(&id.as_str()))
}
fn info<'a>(
    state: &'a State,
    carrier: &Carrier,
) -> Option<(Side, Option<&'a Location>, Option<&'a BoxHandling>)> {
    match carrier {
        Carrier::Unit(id) => state
            .land
            .units
            .get(id)
            .map(|u| (u.side, Some(&u.location), u.box_handling.as_ref())),
        Carrier::Pool(id) => state
            .logistics
            .truck_pools
            .iter()
            .find(|p| &p.id == id)
            .map(|p| (p.side, p.location.as_ref(), p.box_handling.as_ref())),
    }
}
/// Query only the owner's carrier history; it never examines enemy information.
/// The ban also applies after leaving a box or entering/continuing Transit.
/// Cases: land:8.88
/// Interpretations: interp:airlog-0019
pub fn blocks_movement(state: &State, carrier: &Carrier) -> Option<&'static str> {
    let (_, _, handling) = info(state, carrier)?;
    handling
        .filter(|h| state.cursor.op_stage.is_some() && h.stage == WaterStage::current(state))
        .map(|_| REASON)
}
fn add(a: Supplies, b: Supplies) -> Result<Supplies, SupplyError> {
    let sum = |a: i32, b: i32| {
        a.checked_add(b)
            .filter(|n| *n >= 0)
            .ok_or(SupplyError::Invalid)
    };
    Ok(Supplies {
        ammo: sum(a.ammo, b.ammo)?,
        fuel: sum(a.fuel, b.fuel)?,
        stores: sum(a.stores, b.stores)?,
        water: sum(a.water, b.water)?,
    })
}
/// Stamp an already validated cargo load/unload. Call inside the transfer's draft;
/// failure must roll back the stock transfer too. This changes no stock itself.
/// On-map endpoints and Transit are outside the box rule.
/// Cases: land:8.88, airlog:53.24
/// Interpretations: interp:airlog-0019
pub fn record_goods(
    state: &mut State,
    carrier: &Carrier,
    goods: Supplies,
    loading: bool,
) -> Result<(), SupplyError> {
    if [goods.ammo, goods.fuel, goods.stores, goods.water]
        .into_iter()
        .any(|n| n < 0)
    {
        return Err(SupplyError::Invalid);
    }
    if goods == Supplies::default() {
        return Ok(());
    }
    let (_, location, old) = info(state, carrier).ok_or(SupplyError::Invalid)?;
    if !location.is_some_and(is_supply_box) {
        return Ok(());
    }
    if state.cursor.op_stage.is_none() {
        return Err(SupplyError::Invalid);
    }
    let stage = WaterStage::current(state);
    let mut h = old
        .filter(|h| h.stage == stage)
        .cloned()
        .unwrap_or(BoxHandling {
            stage,
            loaded: Supplies::default(),
            unloaded: Supplies::default(),
        });
    if loading {
        h.loaded = add(h.loaded, goods)?;
    } else {
        h.unloaded = add(h.unloaded, goods)?;
    }
    match carrier {
        Carrier::Unit(id) => {
            state
                .land
                .units
                .get_mut(id)
                .ok_or(SupplyError::Invalid)?
                .box_handling = Some(h)
        }
        Carrier::Pool(id) => {
            state
                .logistics
                .truck_pools
                .iter_mut()
                .find(|p| &p.id == id)
                .ok_or(SupplyError::Invalid)?
                .box_handling = Some(h)
        }
    }
    Ok(())
}
/// Every operation separating trucks from a carrier must call this before mutation.
/// The original keeps its history; never copy its stamp to newly separated parts.
/// Existing destination carriers retain their own independently recorded history.
/// Cases: land:8.88, land:3.6
/// Interpretations: interp:airlog-0019
pub fn prepare_division(
    state: &State,
    carrier: &Carrier,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let (side, _, _) = info(state, carrier).ok_or_else(|| EngineError::Invariant {
        detail: "unknown cargo carrier".into(),
    })?;
    if blocks_movement(state, carrier).is_none() {
        return Ok(());
    }
    if strict {
        return Err(EngineError::Unsupported {
            case: "land:8.88".into(),
            detail: "8.88 apportionment not implemented".into(),
        });
    }
    cx.emit(EngineEvent::new(Audience::Side(side),GameEvent::Note {
        text:"Box-handling apportionment is unavailable; the original carrier keeps its movement ban and the separated trucks are unrestricted (8.88).".into(),
    }));
    Ok(())
}
#[cfg(test)]
mod tests;
