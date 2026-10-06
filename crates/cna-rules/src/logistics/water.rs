//! OpStage water consumption and retained reserves for vehicle activity.
pub use super::rations::WaterStage;
use super::rations::{self};
use super::stores::{RationDraw, draw_schema, draws, engine, field, option};
use super::{
    SupplyDemand, SupplyDraw, SupplyError, available_sources_with_content,
    spend_for_unit_with_content,
};
use crate::content::CnaContent;
use crate::state::{Pending, State};
use crate::steps::{illegal, open};
use cna_core::decision::{ActionSchema, ActionSpace, Secrecy, Trigger};
use cna_core::engine::{Cx, EngineError, Rejection};
use cna_core::event::EngineEvent;
use cna_core::ids::{SeatId, UnitId};
use cna_core::quantity::WaterPoints;
use cna_core::visibility::Audience;
use cna_protocol::{GameEvent, Role, Side};
use serde::Deserialize;
use serde_json::Value;

pub const KIND: &str = "cna.logistics.water";
pub const ISSUE_PREFIX: &str = "cna.logistics.water.issue:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaterRequirements {
    pub infantry: i32,
    pub activity: i32,
    pub pasta: i32,
}

/// Body water is consumed now; activity water is reserved until CPA is used.
/// Hot weather doubles the ordinary requirement, not the weekly pasta point.
/// Cases: airlog:52.41, airlog:52.42, airlog:52.43, airlog:52.6, land:29.31
pub fn requirements(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<WaterRequirements, SupplyError> {
    let multiplier = rations::hot_multiplier(content, state, id)?;
    let history = state.logistics.rations.get(id).cloned().unwrap_or_default();
    let stage = WaterStage::current(state);
    let held = state
        .logistics
        .unit_supply
        .get(id)
        .map_or(0, |s| s.activity_water.get());
    if held < 0 {
        return Err(SupplyError::Invalid);
    }
    let total = rations::activity_points(content, state, id)?
        .checked_mul(multiplier)
        .ok_or(SupplyError::Invalid)?;
    let infantry = if rations::infantry(content, id)? {
        multiplier
    } else {
        0
    };
    let activity = if history.activity_used_stage == Some(stage) {
        0
    } else {
        (total - held).max(0)
    };
    let pasta = i32::from(
        rations::pasta(content, id)
            && history.issued_gt == Some(stage.game_turn)
            && history.stores_received > 0
            && history.pasta_gt != Some(stage.game_turn),
    );
    Ok(WaterRequirements {
        infantry,
        activity,
        pasta,
    })
}
fn candidates(
    content: &CnaContent,
    state: &State,
    side: Side,
    strict: bool,
) -> Result<Vec<UnitId>, EngineError> {
    let mut ids = Vec::new();
    let stage = WaterStage::current(state);
    for unit in state
        .land
        .units
        .values()
        .filter(|u| u.side == side && rations::in_play(&u.location))
    {
        if state
            .logistics
            .rations
            .get(&unit.id)
            .is_some_and(|r| r.water_stage == Some(stage))
        {
            continue;
        }
        if unit.toe.is_none()
            && content
                .units
                .units
                .get(&unit.id)
                .is_some_and(|oa| oa.class.is_none())
        {
            continue;
        }
        match requirements(content, state, &unit.id) {
            Ok(r) if r.infantry > 0 || r.activity > 0 || r.pasta > 0 => ids.push(unit.id.clone()),
            Ok(_) => {}
            Err(SupplyError::Unsupported {
                case: "airlog:52.42",
            }) if !strict => {}
            Err(e) => return Err(engine(e)),
        }
    }
    Ok(ids)
}
/// Limited Intelligence overrides the registry's default open distribution window.
/// Unknown HQ vehicle composition stays unsupported in full, unassessed in dev.
/// Cases: airlog:52.0, airlog:52.41, airlog:52.42, land:3.6
pub fn enter(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    // Validate both sides before any consumption or events.
    for side in [Side::Axis, Side::Commonwealth] {
        candidates(content, state, side, strict)?;
    }
    super::stores::feed_prisoners(content, state, cx, false)?;
    if !strict {
        for unit in state
            .land
            .units
            .values()
            .filter(|u| rations::in_play(&u.location))
        {
            if requirements(content, state, &unit.id)
                == Err(SupplyError::Unsupported {
                    case: "airlog:52.42",
                })
            {
                cx.emit(EngineEvent::new(Audience::Side(unit.side), GameEvent::Note {
                    text: format!("{}: vehicle water need is unknown (airlog:52.42; units-0005); left unassessed.", unit.id),
                }));
            }
        }
    }
    for side in [Side::Axis, Side::Commonwealth] {
        open_menu(content, state, side, cx, strict)?;
    }
    Ok(())
}
/// Cases: airlog:52.0, airlog:52.41, airlog:52.42, land:3.6
fn open_menu(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    let ids = candidates(content, state, side, strict)?;
    if ids.is_empty() {
        finalize(content, state, side, strict)?;
        return Ok(());
    }
    let mut options = vec![option(
        "done".into(),
        "Finish; record water shortages and retain unused activity reserves".into(),
    )];
    options.extend(
        ids.into_iter()
            .map(|id| option(id.to_string(), format!("Distribute water to {id}"))),
    );
    open(
        state,
        cx,
        SeatId::new(side, Role::Logistics),
        KIND,
        "Distribute this OpStage's infantry water and reserve water for vehicles that will use CP."
            .into(),
        &["airlog:52.41", "airlog:52.42", "airlog:52.6", "land:3.6"],
        Trigger::Scheduled,
        Secrecy::Secret,
        ActionSpace::new(ActionSchema::Choice { options }),
    );
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Issue {
    infantry: i32,
    activity: i32,
    pasta: bool,
    draws: Vec<RationDraw>,
}

/// Cases: airlog:52.41, airlog:52.42, airlog:52.6, land:3.6
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<String, Rejection> {
    let side = pending.seat.side;
    if pending.kind == KIND {
        let selected = action
            .as_str()
            .ok_or_else(|| illegal("select a unit or done"))?;
        if selected == "done" {
            finalize(content, state, side, strict).map_err(Rejection::Engine)?;
            return Ok("Water distribution finished; shortages recorded privately.".into());
        }
        let id = UnitId::new(selected);
        if !candidates(content, state, side, strict)
            .map_err(Rejection::Engine)?
            .contains(&id)
        {
            return Err(illegal("unit is not eligible for water"));
        }
        let r = requirements(content, state, &id).map_err(|e| Rejection::Engine(engine(e)))?;
        let total = r
            .infantry
            .checked_add(r.activity)
            .and_then(|n| n.checked_add(r.pasta))
            .ok_or_else(|| illegal("water amount overflow"))?;
        let sources = available_sources_with_content(content, state, &id)
            .map_err(|e| Rejection::Engine(engine(e)))?;
        let fields = vec![
            field(
                "infantry",
                "Water consumed by infantry now",
                ActionSchema::Integer {
                    min: 0,
                    max: r.infantry.into(),
                },
            ),
            field(
                "activity",
                "Water reserved for future CPA use",
                ActionSchema::Integer {
                    min: 0,
                    max: r.activity.into(),
                },
            ),
            field(
                "pasta",
                "Supply a missing weekly pasta point",
                ActionSchema::Bool,
            ),
            field(
                "draws",
                "Exact withdrawals; stores must be zero",
                draw_schema(&sources, 0, total),
            ),
        ];
        open(
            state,
            cx,
            pending.seat,
            &format!("{ISSUE_PREFIX}{id}"),
            format!(
                "{id}: {} infantry water; up to {} activity water; {} missing pasta water.",
                r.infantry, r.activity, r.pasta
            ),
            &["airlog:52.41", "airlog:52.42", "airlog:52.6", "land:3.6"],
            Trigger::Scheduled,
            Secrecy::Secret,
            ActionSpace::new(ActionSchema::Record { fields })
                .with_pass("Return to unit selection without issuing water"),
        );
        return Ok(format!("Selected {id} for water."));
    }
    let id = UnitId::new(
        pending
            .kind
            .strip_prefix(ISSUE_PREFIX)
            .ok_or_else(|| illegal("unknown water decision"))?,
    );
    if !candidates(content, state, side, strict)
        .map_err(Rejection::Engine)?
        .contains(&id)
    {
        return Err(illegal("unit is no longer eligible"));
    }
    if !action.is_null() {
        let issue: Issue =
            serde_json::from_value(action.clone()).map_err(|_| illegal("invalid water issue"))?;
        issue_unit(
            content,
            state,
            &id,
            issue.infantry,
            issue.activity,
            issue.pasta,
            &draws(issue.draws)?,
        )?;
    }
    open_menu(content, state, side, cx, strict).map_err(Rejection::Engine)?;
    Ok(format!("{id}: water allocation recorded privately."))
}
/// Exact consumption/reservation is atomic and cannot be repeated in the same stage.
/// Cases: airlog:52.41, airlog:52.42, airlog:52.43, airlog:52.6
pub fn issue_unit(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    infantry: i32,
    activity: i32,
    pasta: bool,
    allocation: &[SupplyDraw],
) -> Result<(), Rejection> {
    let stage = WaterStage::current(state);
    let unit = state
        .land
        .units
        .get(id)
        .ok_or_else(|| illegal("unknown unit"))?;
    if !rations::in_play(&unit.location)
        || state
            .logistics
            .rations
            .get(id)
            .is_some_and(|r| r.water_stage == Some(stage))
    {
        return Err(illegal("unit already assessed or not in play"));
    }
    let r = requirements(content, state, id).map_err(|e| Rejection::Engine(engine(e)))?;
    if infantry < 0
        || infantry > r.infantry
        || activity < 0
        || activity > r.activity
        || (pasta && r.pasta != 1)
    {
        return Err(illegal("water allocation exceeds the requirement"));
    }
    let total = infantry
        .checked_add(activity)
        .and_then(|n| n.checked_add(i32::from(pasta)))
        .ok_or_else(|| illegal("water amount overflow"))?;
    let old = state
        .logistics
        .unit_supply
        .get(id)
        .map_or(0, |s| s.activity_water.get());
    let new = old
        .checked_add(activity)
        .ok_or_else(|| illegal("water reserve overflow"))?;
    spend_for_unit_with_content(
        content,
        state,
        id,
        SupplyDemand {
            water: WaterPoints::new(total),
            ..SupplyDemand::default()
        },
        allocation,
    )
    .map_err(|_| illegal("water cannot be drawn from these friendly sources"))?;
    state
        .logistics
        .unit_supply
        .entry(id.clone())
        .or_default()
        .activity_water = WaterPoints::new(new);
    let history = state.logistics.rations.entry(id.clone()).or_default();
    history.water_stage = Some(stage);
    history.infantry_water_received = infantry;
    if pasta {
        rations::receive_pasta(state, id);
    }
    Ok(())
}
/// Record consecutive infantry shortages once per stage. Idle vehicles do not
/// spend activity water and do not gain infantry dehydration losses.
/// Cases: airlog:52.41, airlog:52.42, airlog:52.53, airlog:52.6
pub fn finalize(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    strict: bool,
) -> Result<(), EngineError> {
    let stage = WaterStage::current(state);
    let ids: Vec<_> = state
        .land
        .units
        .values()
        .filter(|u| u.side == side && rations::in_play(&u.location))
        .map(|u| u.id.clone())
        .collect();
    for id in ids {
        if state.land.units[&id].toe.is_none() && content.units.units[&id].class.is_none() {
            continue;
        }
        let need = match requirements(content, state, &id) {
            Ok(r) => r,
            Err(SupplyError::Unsupported {
                case: "airlog:52.42",
            }) if !strict => continue,
            Err(e) => return Err(engine(e)),
        };
        let r = state.logistics.rations.entry(id.clone()).or_default();
        if r.water_finalized_stage == Some(stage) {
            continue;
        }
        if r.water_stage != Some(stage) {
            r.water_stage = Some(stage);
            r.infantry_water_received = 0;
        }
        r.water_finalized_stage = Some(stage);
        if r.infantry_water_received < need.infantry {
            r.consecutive_short_water_stages = if r
                .last_short_water_stage
                .is_some_and(|previous| previous.ordinal() + 1 == stage.ordinal())
            {
                r.consecutive_short_water_stages.saturating_add(1)
            } else {
                1
            };
            r.last_short_water_stage = Some(stage);
        } else {
            r.consecutive_short_water_stages = 0;
            r.last_short_water_stage = None;
        }
        rations::apply_pasta(content, state, &id);
    }
    Ok(())
}
#[cfg(test)]
mod tests;
