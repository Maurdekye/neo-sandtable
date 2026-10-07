//! Reaction interrupts an entered hex; subsequent movement remains the phasing player's plan.
use super::{formation, movement, zoc};
use crate::{
    CnaContent, State, ownership,
    state::Pending,
    steps::{illegal, open},
};
use cna_core::{
    decision::{ActionSchema, ActionSpace, FieldSchema, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{HexId, SeatId, UnitId},
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
pub const KIND: &str = "cna.movement.reaction";
pub const CONTINUE: &str = "cna.movement.continue";
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReactionState {
    pub window: Option<Window>,
    pub continuation: Option<Continuation>,
    pub held: Vec<Pending>,
    pub controls: BTreeMap<HexId, bool>,
    /// A failure found using hidden facts is raised by adjudication, never answer validation.
    pub adjudication_stop: Option<EngineError>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub trigger_hex: HexId,
    pub eligible: Vec<UnitId>,
    pub reacted: BTreeSet<UnitId>,
    pub mover_stopped: bool,
    pub moving: Vec<UnitId>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Continuation {
    pub seat: SeatId,
    pub unit: UnitId,
    pub members: Vec<UnitId>,
    pub with_stack: bool,
    pub planned_path: Vec<HexId>,
    pub orders_after: Vec<movement::Order>,
    pub mover_stopped: bool,
}
/// Test pre-entry contact and current individual/represented CPA, before the triggering mover arrives.
/// Attached components may qualify independently; a parent must qualify with all its current components.
/// Cases: land:8.51, land:8.52, land:8.53, land:8.54, land:8.56
/// Interpretations: interp:land-0003
pub(super) fn candidates(
    c: &CnaContent,
    s: &State,
    moving: &[UnitId],
    to: &HexId,
    announced: &[HexId],
    strict: bool,
) -> Result<Vec<UnitId>, Rejection> {
    let side = s.land.units[&moving[0]].side;
    if !moving
        .iter()
        .any(|id| formation::combat_unit(c, id) && formation::strength(c, s, id) > 0)
    {
        return Ok(vec![]);
    }
    let mover_cpa = moving
        .iter()
        .filter_map(|id| formation::individual_allowance(c, s, id))
        .map(|a| a.cpa)
        .min()
        .unwrap_or(0);
    let targets: BTreeSet<_> = c.map.neighbors(to).iter().map(|h| h.id.clone()).collect();
    let mut out = vec![];
    for u in s
        .units_of(side.opponent())
        .filter(|u| u.location.hex().is_some_and(|h| targets.contains(h)))
    {
        let members = formation::members(c, s, &u.id);
        let Some(a) = formation::allowance(c, s, &u.id) else {
            continue;
        };
        if !a.motorized
            || members.iter().any(|id| {
                s.land.units[id].engaged
                    || s.land.combat.pinned.contains(id)
                    || s.land.units[id].cohesion_quarters <= -104
                    || super::reserve::validate_path(&s.land.units[id], 1).is_err()
            })
        {
            continue;
        }
        if formation::class(c, &u.id).is_some_and(|cl| cl.unit_type == "sgsu") {
            continue;
        }
        if c.units.units[&u.id]
            .kind
            .as_deref()
            .is_some_and(|k| matches!(k, "convoy" | "second_line_truck" | "third_line_truck"))
            && !zoc::friendly_combat(c, s, u.side, u.location.hex().unwrap())
        {
            continue;
        }
        // Size protection applies to existing pinning as well as a fresh announcement.
        let mut pinning = s.clone();
        for neighbor in c.map.neighbors(u.location.hex().unwrap()) {
            for root in formation::roots(c, s, &neighbor.id, side) {
                if !formation::can_pin(c, s, &root, &u.id) {
                    for id in formation::members(c, s, &root) {
                        pinning.land.units.get_mut(&id).unwrap().location =
                            crate::state::Location::Eliminated;
                    }
                }
            }
        }
        if zoc::controlled(c, &pinning, side, u.location.hex().unwrap(), strict)
            .map_err(Rejection::Engine)?
        {
            continue;
        }
        // This eligibility fact is fixed when the interrupt opens, rather than queried from
        // hidden pinner strength when a defender subsequently answers.
        if let Some(parent) = ownership::parent_for_unit(c, s, &u.id) {
            let mut detached = s.clone();
            detached.land.units.get_mut(&u.id).unwrap().detached = true;
            detached.land.units.get_mut(&u.id).unwrap().attached_to = None;
            if moving.iter().any(|id| {
                !formation::can_pin(c, s, id, parent)
                    && formation::can_pin(c, &detached, id, parent)
            }) {
                continue;
            }
        }
        if announced.contains(u.location.hex().unwrap())
            && mover_cpa >= a.cpa.saturating_add(6)
            && moving.iter().any(|p| formation::can_pin(c, s, p, &u.id))
        {
            continue;
        }
        out.push(u.id.clone());
    }
    Ok(out)
}
fn eligible(s: &State, seat: SeatId, c: &CnaContent) -> Vec<UnitId> {
    s.land.reaction.window.as_ref().map_or_else(Vec::new, |w| {
        w.eligible
            .iter()
            .filter(|id| {
                !formation::members(c, s, id)
                    .iter()
                    .any(|m| w.reacted.contains(m))
                    && s.land.units.get(*id).is_some_and(|u| u.side == seat.side)
                    && ownership::seat_for_unit(c, s, id) == seat.role
            })
            .cloned()
            .collect()
    })
}
fn space(ids: Vec<UnitId>, pass: bool) -> ActionSpace {
    let max = ids.len().min(1) as u32;
    let mut a = ActionSpace::new(ActionSchema::List {
        min: if pass { 0 } else { 1 },
        max,
        item: Box::new(ActionSchema::Record {
            fields: vec![
                FieldSchema {
                    name: "unit".into(),
                    doc: "One unit completing its reaction or interrupted move.".into(),
                    schema: ActionSchema::Unit { among: ids },
                    optional: false,
                },
                FieldSchema {
                    name: "path".into(),
                    doc: "Entered adjacent hexes from the current hex.".into(),
                    schema: ActionSchema::List {
                        item: Box::new(ActionSchema::Hex { among: None }),
                        min: 1,
                        max: 4096,
                    },
                    optional: false,
                },
            ],
        }),
    });
    if pass {
        a = a.with_pass("Stop here, or decline reaction.")
    };
    a
}
fn open_role(c: &CnaContent, s: &mut State, seat: SeatId, cx: &mut Cx<'_>) {
    let ids = eligible(s, seat, c);
    if ids.is_empty() {
        return;
    }
    let hex = s.land.reaction.window.as_ref().unwrap().trigger_hex.clone();
    open(
        s,
        cx,
        seat,
        KIND,
        format!(
            "Enemy movement at {hex} permits reaction. Choose one complete path, or finish reacting to this entry."
        ),
        &[
            "land:8.51",
            "land:8.52",
            "land:8.53",
            "land:8.55",
            "land:8.56",
        ],
        Trigger::Triggered,
        Secrecy::Open,
        space(ids, true).with_context(serde_json::json!({"trigger_hex":hex})),
    );
}
/// Park all ordinary movement decisions while the nonphasing side resolves this entry.
/// Only the defending seats receive their own eligible units and costs.
/// Cases: land:8.51, land:8.52
pub(super) fn open_interrupt(c: &CnaContent, s: &mut State, cx: &mut Cx<'_>) {
    s.land
        .reaction
        .held
        .extend(std::mem::take(&mut s.decisions.pending));
    let side = s.land.units[&s.land.reaction.continuation.as_ref().unwrap().unit]
        .side
        .opponent();
    for role in [Role::FrontLine, Role::RearArea, Role::Logistics] {
        open_role(c, s, SeatId::new(side, role), cx)
    }
}
/// One trigger may move several defenders; each selected unit completes its reaction once.
/// Invalid paths are rejected atomically and a new entry can trigger the same unit again.
/// Cases: land:8.51, land:8.52, land:8.53, land:8.55, land:8.56
pub fn answer(
    c: &CnaContent,
    s: &mut State,
    p: &Pending,
    action: &Value,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    if s.land.reaction.window.is_none() {
        return Err(illegal("reaction window is no longer open"));
    }
    let legal = eligible(s, p.seat, c);
    let orders: Vec<movement::Order> = if action.is_null() {
        vec![]
    } else {
        serde_json::from_value(action.clone())
            .map_err(|_| illegal("choose one reaction path or pass"))?
    };
    if orders.len() > 1 {
        return Err(illegal("choose one complete reaction at a time"));
    }
    if let Some(order) = orders.first() {
        if !legal.contains(&order.unit) || order.with_stack || !order.close_assault.is_empty() {
            return Err(illegal("unit is not available for this reaction"));
        }
        let members = formation::members(c, s, &order.unit);
        movement::execute_nonphasing(
            c,
            s,
            order,
            p.seat,
            strict,
            movement::NonPhasingMove::Reaction,
            cx,
        )?;
        s.land
            .reaction
            .window
            .as_mut()
            .unwrap()
            .reacted
            .extend(members);
        if s.land.reaction.adjudication_stop.is_some() {
            return Ok("Reaction choice complete.".into());
        }
        open_role(c, s, p.seat, cx);
    } else {
        s.land
            .reaction
            .window
            .as_mut()
            .unwrap()
            .reacted
            .extend(legal);
    }
    if s.land.reaction.adjudication_stop.is_some() {
        return Ok("Reaction choice complete.".into());
    }
    if !s.decisions.pending.iter().any(|p| p.kind == KIND) {
        s.land.reaction.window = None;
        open_continuation(c, s, strict, cx)?;
    }
    Ok("Reaction choice complete.".into())
}
/// The original remaining path is a revisable plan. Stop is offered only at a legal endpoint.
/// Cases: land:8.13, land:8.51, land:9.31, land:9.32
/// Interpretations: interp:land-0026
pub(super) fn open_continuation(
    c: &CnaContent,
    s: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), Rejection> {
    let k = s.land.reaction.continuation.as_ref().unwrap().clone();
    let hex = s.land.units[&k.unit]
        .location
        .hex()
        .ok_or_else(|| illegal("interrupted mover is no longer on the map"))?;
    let stop = super::stacking::validate_end(c, s, hex, k.seat.side, strict).is_ok();
    if k.mover_stopped && stop {
        return movement::finish_continuation(c, s, strict, cx);
    }
    let reaches = movement::reachable(c, s, &k.unit, strict);
    if !stop && reaches.is_empty() {
        if strict {
            movement::defer_stop(
                s,
                EngineError::Unsupported {
                    case: "land:9.31".into(),
                    detail: "reaction leaves an overstack with no legal continuation".into(),
                },
            );
            return Ok(());
        }
        cx.emit(EngineEvent::new(Audience::Side(k.seat.side),GameEvent::Note{text:"Interrupted movement stops with unresolved overstacking (land:9.31; interp:land-0026).".into()}));
        return movement::finish_continuation(c, s, strict, cx);
    }
    open(
        s,
        cx,
        k.seat,
        CONTINUE,
        "Continue this interrupted unit's move along a new path, or stop where legal.".into(),
        &["land:8.13", "land:8.51", "land:9.31", "land:9.32"],
        Trigger::Triggered,
        Secrecy::Open,
        space(vec![k.unit.clone()], stop).with_context(
            serde_json::json!({"unit":k.unit,"planned_remaining_path":k.planned_path}),
        ),
    );
    Ok(())
}
/// A continuation cannot move any other unit or alter the represented moving stack.
/// Cases: land:8.13, land:8.51, land:9.31
/// Interpretations: interp:land-0026
pub fn answer_continuation(
    c: &CnaContent,
    s: &mut State,
    p: &Pending,
    action: &Value,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    let k = s
        .land
        .reaction
        .continuation
        .as_ref()
        .ok_or_else(|| illegal("no interrupted movement"))?
        .clone();
    if p.seat != k.seat {
        return Err(illegal("interrupted move belongs to another seat"));
    }
    let mut orders: Vec<movement::Order> = if action.is_null() {
        vec![]
    } else {
        serde_json::from_value(action.clone())
            .map_err(|_| illegal("choose the interrupted unit's remaining path"))?
    };
    if orders.is_empty() {
        super::stacking::validate_end(
            c,
            s,
            s.land.units[&k.unit].location.hex().unwrap(),
            p.seat.side,
            strict,
        )?;
        movement::finish_continuation(c, s, strict, cx)?;
    } else {
        if orders.len() != 1
            || orders[0].unit != k.unit
            || orders[0].with_stack && !k.with_stack
            || k.mover_stopped
        {
            return Err(illegal("unit cannot continue this move"));
        }
        orders[0].with_stack = k.with_stack;
        movement::continue_order(c, s, &orders[0], strict, cx)?;
    }
    Ok("Interrupted movement choice complete.".into())
}

/// Raise a truth-only failure after the answer has been accepted and its windows parked.
/// Cases: land:10.21, land:10.6, land:9.31
pub(crate) fn finish_adjudication(state: &State) -> Result<(), EngineError> {
    state
        .land
        .reaction
        .adjudication_stop
        .clone()
        .map_or(Ok(()), Err)
}
