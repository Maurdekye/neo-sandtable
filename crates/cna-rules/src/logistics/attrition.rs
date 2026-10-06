//! Infantry losses from food and water shortages; weapons are never removed here.
use super::rations::{self, WaterStage};
use super::stores::{engine, option};
use super::{SupplyError, toe_strength};
use crate::content::CnaContent;
use crate::state::{Location, Pending, State};
use crate::steps::{illegal, open};
use cna_content::units::Toe;
use cna_core::decision::{ActionSchema, ActionSpace, Secrecy, Trigger};
use cna_core::engine::{Cx, EngineError, Rejection};
use cna_core::event::EngineEvent;
use cna_core::ids::{SeatId, UnitId};
use cna_core::visibility::Audience;
use cna_protocol::{GameEvent, Role, Side};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const KIND: &str = "cna.logistics.attrition";
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoodLoss {
    pub owner: Side,
    pub location: Location,
    pub units: Vec<UnitId>,
    pub remaining: i32,
}

/// Only unambiguous infantry strength is reduced; an explicit weapon composition is immune.
/// Cases: airlog:51.22, airlog:52.53
fn strength(content: &CnaContent, state: &State, id: &UnitId) -> Result<i32, SupplyError> {
    let unit = state.land.units.get(id).ok_or(SupplyError::Invalid)?;
    if !rations::in_play(&unit.location)
        || unit.toe.is_none()
        || matches!(unit.toe, Some(Toe::Weapons(_)))
        || !rations::infantry(content, id)?
    {
        return Ok(0);
    }
    Ok(toe_strength(content, unit)?.get())
}
/// Record the lower strength against its printed maximum; trucks/cargo are separate.
/// Cases: airlog:51.22, airlog:52.53
fn lose(content: &CnaContent, state: &mut State, id: &UnitId, n: i32) -> Result<(), SupplyError> {
    let current = strength(content, state, id)?;
    if n < 0 || n > current {
        return Err(SupplyError::Invalid);
    }
    let max = rations::class(content, id)?
        .max_toe
        .ok_or(SupplyError::Unsupported { case: "land:4.46" })?;
    let left = current - n;
    let toe = if left <= max {
        Toe::Under { under: max - left }
    } else {
        Toe::Over { over: left - max }
    };
    state
        .land
        .units
        .get_mut(id)
        .ok_or(SupplyError::Invalid)?
        .toe = Some(toe);
    Ok(())
}

/// Food casualties are assessed once, in OpStage1; dehydration casualties once in
/// each consecutive dry OpStage after the first. The owner allocates food losses.
/// Cases: airlog:51.22, airlog:52.53, land:3.6
/// Interpretations: interp:airlog-0007
pub fn enter(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    let stage = WaterStage::current(state);
    if state.logistics.attrition_started_stage == Some(stage) {
        return Ok(());
    }
    let mut groups = BTreeMap::<String, (Side, Location, u16, Vec<UnitId>)>::new();
    for unit in state
        .land
        .units
        .values()
        .filter(|u| rations::in_play(&u.location))
    {
        if strength(content, state, &unit.id).map_err(engine)? == 0 {
            continue;
        }
        let Some(r) = state.logistics.rations.get(&unit.id) else {
            continue;
        };
        if stage.op_stage == 1
            && r.finalized_gt == Some(stage.game_turn)
            && r.last_short_gt == Some(stage.game_turn)
            && r.consecutive_short_gt >= 2
            && r.consecutive_short_gt % 2 == 0
        {
            let key = format!(
                "{:?}:{}:{}",
                unit.side,
                serde_json::to_string(&unit.location).unwrap(),
                r.consecutive_short_gt
            );
            groups
                .entry(key)
                .or_insert_with(|| {
                    (
                        unit.side,
                        unit.location.clone(),
                        r.consecutive_short_gt,
                        Vec::new(),
                    )
                })
                .3
                .push(unit.id.clone());
        }
    }
    let mut losses = Vec::new();
    for (owner, location, consecutive, ids) in groups.into_values() {
        let mut total = 0i64;
        for id in &ids {
            total += i64::from(strength(content, state, id).map_err(engine)?);
        }
        let percent = i64::from(consecutive).min(100);
        let n = i32::try_from((total * percent + 50) / 100)
            .map_err(|_| engine(SupplyError::Invalid))?;
        if n > 0 {
            losses.push(FoodLoss {
                owner,
                location,
                units: ids,
                remaining: n,
            });
        }
    }
    // Apply the per-unit dehydration loss; a unit cannot lose more than its remaining strength.
    let ids: Vec<_> = state.land.units.keys().cloned().collect();
    for id in ids {
        let current = strength(content, state, &id).map_err(engine)?;
        let Some(r) = state.logistics.rations.get(&id) else {
            continue;
        };
        if r.attrition_stage == Some(stage) {
            continue;
        }
        let dry = r.water_finalized_stage == Some(stage)
            && r.last_short_water_stage == Some(stage)
            && r.consecutive_short_water_stages > 1;
        if dry && current > 0 {
            let side = state.land.units[&id].side;
            lose(content, state, &id, 1).map_err(engine)?;
            cx.emit(EngineEvent::new(
                Audience::Side(side),
                GameEvent::Note {
                    text: format!(
                        "{id} lost one infantry TOE point after consecutive water shortages."
                    ),
                },
            ));
        }
        state
            .logistics
            .rations
            .get_mut(&id)
            .unwrap()
            .attrition_stage = Some(stage);
    }
    // Water casualties can exhaust a small food-loss cohort; only surviving strength is available.
    for loss in &mut losses {
        let remaining = loss
            .units
            .iter()
            .try_fold(0i64, |sum, id| {
                strength(content, state, id).map(|n| sum + i64::from(n))
            })
            .map_err(engine)?;
        loss.remaining = loss
            .remaining
            .min(i32::try_from(remaining).unwrap_or(i32::MAX));
    }
    state.logistics.food_losses = losses;
    state.logistics.attrition_started_stage = Some(stage);
    for side in [Side::Axis, Side::Commonwealth] {
        open_menu(content, state, side, cx)?;
    }
    Ok(())
}
/// Mandatory losses cannot be passed or allocated to immune weapons.
/// Cases: airlog:51.22, land:3.6
fn open_menu(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let Some(loss) = state
        .logistics
        .food_losses
        .iter()
        .find(|l| l.owner == side && l.remaining > 0)
    else {
        return Ok(());
    };
    let mut options = Vec::new();
    for id in &loss.units {
        if strength(content, state, id).map_err(engine)? > 0 {
            options.push(option(
                id.to_string(),
                format!("Remove one infantry TOE from {id}"),
            ));
        }
    }
    if options.is_empty() {
        return Err(engine(SupplyError::Invalid));
    }
    open(
        state,
        cx,
        SeatId::new(side, Role::Logistics),
        KIND,
        format!(
            "Allocate {} remaining infantry TOE losses from the food shortage at {:?}.",
            loss.remaining, loss.location
        ),
        &["airlog:51.22", "land:3.6"],
        Trigger::Scheduled,
        Secrecy::Secret,
        ActionSpace::new(ActionSchema::Choice { options }),
    );
    Ok(())
}
/// Cases: airlog:51.22, land:3.6
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    let id = UnitId::new(
        action
            .as_str()
            .ok_or_else(|| illegal("select an infantry unit for the loss"))?,
    );
    let index = state
        .logistics
        .food_losses
        .iter()
        .position(|l| l.owner == pending.seat.side && l.remaining > 0)
        .ok_or_else(|| illegal("no food losses remain"))?;
    if !state.logistics.food_losses[index].units.contains(&id)
        || strength(content, state, &id).map_err(|e| Rejection::Engine(engine(e)))? == 0
    {
        return Err(illegal("unit cannot take this infantry loss"));
    }
    lose(content, state, &id, 1).map_err(|_| illegal("invalid infantry loss"))?;
    state.logistics.food_losses[index].remaining -= 1;
    open_menu(content, state, pending.seat.side, cx).map_err(Rejection::Engine)?;
    Ok(format!(
        "{id} lost one infantry TOE point to the food shortage."
    ))
}
#[cfg(test)]
mod tests;
