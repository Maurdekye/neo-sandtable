//! Infantry losses from food and water shortages; weapons are never removed here.
use super::rations::{self, WaterStage};
use super::stores::{engine, field, option};
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
/// A fixed private batch from each side is adjudicated only at joint closure.
/// Cases: airlog:51.22, land:3.6
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AttritionWindow {
    pub submitted: BTreeMap<Side, Value>,
    pub completed_stage: Option<WaterStage>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LossOrder {
    unit: UnitId,
    points: i32,
}

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
    let toe = if left < max {
        Toe::Under { under: left }
    } else if left == max {
        Toe::Normal(cna_content::units::NormalToe::N)
    } else {
        Toe::Over { over: left }
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
    state.logistics.attrition_window = AttritionWindow::default();
    state.logistics.food_losses = losses;
    state.logistics.attrition_started_stage = Some(stage);
    for side in [Side::Axis, Side::Commonwealth] {
        open_menu(content, state, side, cx)?;
    }
    Ok(())
}
/// Each scheduled side receives exactly one batch, including an empty forced pass.
/// Cases: airlog:51.22, land:3.6
fn open_menu(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let own: Vec<_> = state
        .logistics
        .food_losses
        .iter()
        .filter(|l| l.owner == side && l.remaining > 0)
        .collect();
    let mut options = Vec::new();
    let mut maximum = 0;
    for loss in &own {
        for id in &loss.units {
            let n = strength(content, state, id).map_err(engine)?;
            if n > 0 {
                maximum = maximum.max(n);
                options.push(option(
                    id.to_string(),
                    format!(
                        "{id}: up to {n} infantry TOE; allocate {} across [{}]",
                        loss.remaining,
                        loss.units
                            .iter()
                            .map(|u| u.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
        }
    }
    let empty = own.is_empty();
    if !empty && options.is_empty() {
        return Err(engine(SupplyError::Invalid));
    }
    let count = options.len() as u32;
    let schema = if empty {
        ActionSchema::Choice { options: vec![] }
    } else {
        ActionSchema::List {
            min: 1,
            max: count,
            item: Box::new(ActionSchema::Record {
                fields: vec![
                    field(
                        "unit",
                        "Own infantry casualty",
                        ActionSchema::Choice { options },
                    ),
                    field(
                        "points",
                        "Infantry TOE lost",
                        ActionSchema::Integer {
                            min: 1,
                            max: i64::from(maximum),
                        },
                    ),
                ],
            }),
        }
    };
    let mut space = ActionSpace::new(schema);
    if empty {
        space = space.with_pass("No food casualties to allocate");
    }
    open(
        state,
        cx,
        SeatId::new(side, Role::Logistics),
        KIND,
        "Allocate all of this side's food-shortage casualties.".into(),
        &["airlog:51.22", "land:3.6"],
        Trigger::Scheduled,
        Secrecy::SecretSimultaneous,
        space,
    );
    Ok(())
}
/// A complete list is checked against only the submitting owner's groups and strength.
/// Cases: airlog:51.22, land:3.6
fn validate(
    content: &CnaContent,
    state: &State,
    side: Side,
    action: &Value,
) -> Result<Vec<LossOrder>, Rejection> {
    let groups: Vec<_> = state
        .logistics
        .food_losses
        .iter()
        .filter(|l| l.owner == side && l.remaining > 0)
        .collect();
    if groups.is_empty() {
        if action.is_null() {
            return Ok(vec![]);
        }
        return Err(illegal("no own food casualties to allocate"));
    }
    let orders: Vec<LossOrder> = serde_json::from_value(action.clone())
        .map_err(|_| illegal("provide a complete infantry-loss list"))?;
    let count: usize = groups.iter().map(|g| g.units.len()).sum();
    if orders.is_empty() || orders.len() > count {
        return Err(illegal("invalid own food-loss list length"));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut sums = vec![0i64; groups.len()];
    for order in &orders {
        if !seen.insert(&order.unit) {
            return Err(illegal("repeated infantry casualty"));
        }
        let group = groups
            .iter()
            .position(|g| g.units.contains(&order.unit))
            .ok_or_else(|| illegal("unit is not an own food-loss candidate"))?;
        if order.points <= 0
            || order.points
                > strength(content, state, &order.unit).map_err(|e| Rejection::Engine(engine(e)))?
        {
            return Err(illegal("invalid infantry casualty amount"));
        }
        sums[group] += i64::from(order.points);
    }
    if groups
        .iter()
        .zip(sums)
        .any(|(g, n)| i64::from(g.remaining) != n)
    {
        return Err(illegal("allocate every own food-loss group exactly"));
    }
    Ok(orders)
}
/// Answering buffers a validated owner-known plan without reducing strength.
/// Cases: airlog:51.22, land:3.6
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    _cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    if state
        .logistics
        .attrition_window
        .submitted
        .contains_key(&pending.seat.side)
        || state.logistics.attrition_window.completed_stage == Some(WaterStage::current(state))
    {
        return Err(illegal("food-loss batch already closed"));
    }
    validate(content, state, pending.seat.side, action)?;
    state
        .logistics
        .attrition_window
        .submitted
        .insert(pending.seat.side, action.clone());
    Ok("Food-loss allocation recorded for joint closure.".into())
}
/// Apply both complete casualty lists atomically after the fixed window closes.
/// Cases: airlog:51.22, land:3.6
pub fn finish(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    let stage = WaterStage::current(state);
    if state.logistics.attrition_started_stage != Some(stage)
        || state.logistics.attrition_window.completed_stage == Some(stage)
        || !state.decisions.pending.is_empty()
        || ![Side::Axis, Side::Commonwealth]
            .iter()
            .all(|s| state.logistics.attrition_window.submitted.contains_key(s))
    {
        return Ok(());
    }
    let mut draft = state.clone();
    for side in [Side::Axis, Side::Commonwealth] {
        let action = &state.logistics.attrition_window.submitted[&side];
        let orders =
            validate(content, state, side, action).map_err(|e| EngineError::Invariant {
                detail: format!("closed food-loss plan: {e:?}"),
            })?;
        for order in orders {
            lose(content, &mut draft, &order.unit, order.points).map_err(engine)?;
        }
        for group in draft
            .logistics
            .food_losses
            .iter_mut()
            .filter(|g| g.owner == side)
        {
            group.remaining = 0;
        }
    }
    draft.logistics.attrition_window.submitted.clear();
    draft.logistics.attrition_window.completed_stage = Some(stage);
    *state = draft;
    for side in [Side::Axis, Side::Commonwealth] {
        cx.emit(EngineEvent::new(
            Audience::Side(side),
            GameEvent::Note {
                text: "Food-shortage casualty allocation completed.".into(),
            },
        ));
    }
    Ok(())
}
/// Deterministic complete allocation over the owner's advertised casualty groups.
/// Cases: airlog:51.22, land:3.6
pub fn baseline(content: &CnaContent, state: &State, side: Side) -> Option<Value> {
    let mut orders = Vec::new();
    for group in state
        .logistics
        .food_losses
        .iter()
        .filter(|g| g.owner == side && g.remaining > 0)
    {
        let mut left = group.remaining;
        for id in &group.units {
            let points = left.min(strength(content, state, id).ok()?);
            if points > 0 {
                orders.push(serde_json::json!({"unit":id,"points":points}));
                left -= points;
            }
        }
        if left != 0 {
            return None;
        }
    }
    Some(if orders.is_empty() {
        Value::Null
    } else {
        Value::Array(orders)
    })
}
#[cfg(test)]
mod tests;
