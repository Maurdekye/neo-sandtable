//! Continual movement preserves stage costs and each preceding segment's movement restriction.
use crate::{
    CnaContent, State,
    state::Pending,
    steps::{illegal, open},
};
use cna_core::{
    decision::{ActionSchema, ActionSpace, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    ids::{SeatId, UnitId},
};
use cna_protocol::Role;
use serde_json::Value;

pub const KIND: &str = "cna.movement.repeat";

/// Record the end of movement before breakdown or combat can change positions.
/// A unit that has finished away from enemy presence cannot regain movement by a later enemy retreat.
/// Cases: land:8.21, land:8.22, land:8.23
/// Interpretations: interp:land-0023
pub fn finish_movement(content: &CnaContent, state: &mut State) {
    if state.land.movement.ended
        || state
            .decisions
            .pending
            .iter()
            .any(|p| p.kind == super::movement::KIND)
    {
        return;
    }
    let Some(side) = state.cursor.phasing(state.turn.player_a) else {
        return;
    };
    let enemy: Vec<_> = state
        .units_of(side.opponent())
        .filter_map(|u| {
            u.location
                .hex()
                .and_then(|h| content.map.get(h))
                .map(|h| h.axial)
        })
        .collect();
    let blocked: Vec<_> = state
        .units_of(side)
        .filter(|u| {
            !matches!(
                u.reserve.status,
                super::reserve::Status::First | super::reserve::Status::Second
            ) && u
                .location
                .hex()
                .and_then(|h| content.map.get(h))
                .is_some_and(|h| !enemy.iter().any(|enemy| h.axial.distance(*enemy) <= 2))
        })
        .map(|u| u.id.clone())
        .collect();
    state.land.movement.cycle_blocked.extend(blocked);
    state.land.movement.ended = true;
}

/// The first segment is unrestricted by proximity; later ones retain the preceding prohibition.
/// Cases: land:8.23
/// Interpretations: interp:land-0023
pub fn movement_allowed(state: &State, id: &UnitId) -> bool {
    state.cursor.cycle == 1
        || state
            .land
            .units
            .get(id)
            .is_some_and(|u| super::reserve::bypass_proximity(u, state.cursor.cycle))
        || !state.land.movement.cycle_blocked.contains(id)
}

/// The phasing side chooses whether to repeat after all movement and combat are complete.
/// Combat may be repeated without movement, so lack of a movable unit does not remove the choice.
/// Cases: land:8.21, land:8.22, land:8.25
pub fn enter(_content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    let Some(side) = state.cursor.phasing(state.turn.player_a) else {
        return Ok(());
    };
    open(
        state,
        cx,
        SeatId::new(side, Role::FrontLine),
        KIND,
        "Repeat movement and combat, or end this half's movement and combat phase.".into(),
        &["land:8.21", "land:8.22", "land:8.25"],
        Trigger::Triggered,
        Secrecy::Open,
        ActionSpace::new(ActionSchema::Bool).with_pass("End the movement and combat phase."),
    );
    Ok(())
}

/// Repeating resets the segment marker and fuel origin, while CP and cohesion remain in the stage.
/// Cases: land:8.21, land:8.22
pub fn answer(state: &mut State, _pending: &Pending, action: &Value) -> Result<String, Rejection> {
    if action.is_null() || action == &Value::Bool(false) {
        return Ok("Movement and combat phase complete.".into());
    }
    if action != &Value::Bool(true) {
        return Err(illegal("choose true to repeat, false or pass to finish"));
    }
    if state.cursor.anchor() != "opstage.movement_and_combat.reserve_release" {
        return Err(illegal(
            "repeat is only available after movement and combat",
        ));
    }
    if state.cursor.cycle == u16::MAX {
        return Err(illegal("movement cycle counter exhausted"));
    }
    state.cursor.repeat_movement_and_combat();
    Ok("Begin another movement and combat cycle.".into())
}
