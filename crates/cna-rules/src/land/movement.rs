//! Ordered complete moves. Planning reads disclosed control, execution asks truthful control on entry.
use super::{capability, formation, map, stacking, zoc};
use crate::{
    CnaContent, State,
    logistics::{self, SupplyError},
    ownership,
    state::{Location, Pending},
    steps::{illegal, open},
    view,
};
use cna_core::{
    decision::{ActionSchema, ActionSpace, FieldSchema, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{HexId, SeatId, UnitId},
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role, Side, Stack};
use cna_tables::land::weather::WeatherKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const KIND: &str = "cna.movement.orders";
const PATH_LIMIT: u32 = 4096;
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MovementState {
    /// Profile of this entered window, so scripted controllers use the same reachability policy.
    #[serde(default)]
    pub strict: bool,
    pub moved: BTreeSet<UnitId>,
    /// Truthful answers already disclosed to the phasing side in this segment.
    pub controls: BTreeMap<HexId, bool>,
    /// Units explicitly off the network; retained between segments until they use it again.
    pub off_road: BTreeSet<UnitId>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Order {
    unit: UnitId,
    path: Vec<HexId>,
    #[serde(default)]
    with_stack: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct Reachable {
    pub hex: HexId,
    pub path: Vec<HexId>,
    pub cp_quarters: i32,
    pub fuel_tenths: i32,
    pub control_unknown: bool,
    #[serde(skip)]
    end_valid: bool,
}
fn unsupported(case: &str, detail: &str) -> Rejection {
    Rejection::Engine(EngineError::Unsupported {
        case: case.into(),
        detail: detail.into(),
    })
}
fn supply_error(error: SupplyError, strict: bool) -> Rejection {
    match error {
        SupplyError::UnknownFuelRate if strict => {
            unsupported("airlog:49.12", "fuel rate is unknown (interp:units-0005)")
        }
        SupplyError::UnknownFuelRate => illegal("unit fuel rate is unknown (units-0005)"),
        SupplyError::Unsupported { case } => {
            unsupported(case, "movement fuel is not supported for this unit")
        }
        SupplyError::Insufficient => illegal("insufficient fuel for the ordered path"),
        SupplyError::Invalid => illegal("invalid movement supply allocation"),
    }
}
/// Units represented by a parent may be selected to detach; convoy trucks have their own phase.
/// Cases: land:6.26, land:8.11, land:8.17, land:8.18, land:19.44
fn eligible(content: &CnaContent, state: &State, id: &UnitId, seat: SeatId) -> bool {
    let Some(u) = state.land.units.get(id) else {
        return false;
    };
    u.side == seat.side
        && u.location.hex().is_some()
        && ownership::seat_for_unit(content, state, id) == seat.role
        && !state.land.movement.moved.contains(id)
        && !matches!(
            content.units.units[id].kind.as_deref(),
            Some("convoy" | "second_line_truck" | "third_line_truck")
        )
        && formation::allowance(content, state, id)
            .is_some_and(|a| capability::validate_move(u, a, 1).is_ok())
}
fn available(content: &CnaContent, state: &State, seat: SeatId) -> Vec<UnitId> {
    state
        .units_of(seat.side)
        .filter(|u| eligible(content, state, &u.id, seat))
        .map(|u| u.id.clone())
        .collect()
}
fn open_seat(content: &CnaContent, state: &mut State, seat: SeatId, cx: &mut Cx<'_>) {
    let ids = available(content, state, seat);
    if ids.is_empty() {
        return;
    }
    let count = u32::try_from(ids.len()).unwrap_or(u32::MAX);
    let space=ActionSpace::new(ActionSchema::List {min:0,max:count,item:Box::new(ActionSchema::Record {fields:vec![
        FieldSchema {name:"unit".into(),doc:"Unit to move once in this segment; its represented components move with it.".into(),schema:ActionSchema::Unit {among:ids},optional:false},
        FieldSchema {name:"path".into(),doc:"Complete ordered move: adjacent hexes entered, beginning next to the current hex.".into(),schema:ActionSchema::List {item:Box::new(ActionSchema::Hex {among:None}),min:1,max:PATH_LIMIT},optional:false},
        FieldSchema {name:"with_stack".into(),doc:"Move all independent counters here commanded by this seat as one stack.".into(),schema:ActionSchema::Bool,optional:true},
    ]})}).with_pass("Finish this seat's movement for the segment.");
    open(state,cx,seat,KIND,"Order complete moves in execution order. Inspect your units for paths and exact quarter-CP/fuel costs, or pass to finish.".into(),
        &["land:8.11","land:8.13","land:19.44"],Trigger::Scheduled,Secrecy::Open,space);
}
/// Disclose control only in starting hexes, not hypothetical future destinations.
/// Cases: land:8.11, land:10.6, land:19.44
pub fn enter(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let Some(side) = state.cursor.phasing(state.turn.player_a) else {
        return Ok(());
    };
    state.land.movement.strict = strict;
    state.land.movement.moved.clear();
    state.land.movement.controls.clear();
    let starts: BTreeSet<_> = state
        .units_of(side)
        .filter_map(|u| u.location.hex().cloned())
        .collect();
    for hex in starts {
        let control = zoc::controlled(content, state, side.opponent(), &hex, strict)?;
        state.land.movement.controls.insert(hex, control);
    }
    for role in [Role::FrontLine, Role::RearArea, Role::Logistics] {
        open_seat(content, state, SeatId::new(side, role), cx);
    }
    Ok(())
}
fn effective_control(
    content: &CnaContent,
    state: &State,
    side: Side,
    hex: &HexId,
    moving: &[UnitId],
    control: bool,
) -> bool {
    control
        && !state.units_of(side).any(|u| {
            u.location.hex() == Some(hex)
                && !moving.contains(&u.id)
                && formation::combat_unit(content, &u.id)
                && formation::strength(content, state, &u.id) > 0
                && u.cohesion_quarters > -104
        })
}
fn unit_stack(
    content: &CnaContent,
    state: &mut State,
    order: &Order,
    seat: SeatId,
) -> Result<Vec<UnitId>, Rejection> {
    if !eligible(content, state, &order.unit, seat) {
        return Err(illegal(
            "unit is not available to this seat in this segment",
        ));
    }
    let origin = state.land.units[&order.unit]
        .location
        .hex()
        .unwrap()
        .clone();
    let roots = if order.with_stack {
        if !formation::roots(content, state, &origin, seat.side).contains(&order.unit) {
            return Err(illegal("select a represented counter to move a stack"));
        }
        formation::roots(content, state, &origin, seat.side)
    } else {
        vec![order.unit.clone()]
    };
    let mut members = BTreeSet::new();
    for root in &roots {
        if !eligible(content, state, root, seat) {
            return Err(illegal("stack contains a unit unavailable to this seat"));
        }
        members.extend(formation::members(content, state, root));
    }
    // Selecting an attached component detaches it and its complete represented subtree.
    if !order.with_stack
        && let Some(parent) = ownership::parent_for_unit(content, state, &order.unit).cloned()
        && state
            .land
            .units
            .get(&parent)
            .is_some_and(|u| u.location.hex() == Some(&origin))
    {
        let a = formation::individual_allowance(content, state, &parent)
            .ok_or_else(|| illegal("parent movement rating is unresolved"))?;
        capability::validate_move(&state.land.units[&parent], a, 4)?;
        capability::charge(state.land.units.get_mut(&parent).unwrap(), a, 4, true)?;
        let a = formation::individual_allowance(content, state, &order.unit)
            .ok_or_else(|| illegal("movement rating is unresolved"))?;
        capability::validate_move(&state.land.units[&order.unit], a, 4)?;
        capability::charge(state.land.units.get_mut(&order.unit).unwrap(), a, 4, true)?;
        state.land.units.get_mut(&order.unit).unwrap().detached = true;
        state.land.units.get_mut(&order.unit).unwrap().attached_to = None;
        if content.units.units[&order.unit].parent.as_ref() != Some(&parent) {
            state.land.movement.moved.insert(parent);
        }
    }
    Ok(members.into_iter().collect())
}
/// Price every edge and verify known constraints before execution. Unknown control is deliberately
/// ignored by the planning pass; execution discloses it only on entry and truncates there.
/// Cases: land:8.13, land:8.14, land:8.15, land:8.24, land:8.65, land:9.31, land:9.33
/// Cases: land:29.44, land:29.51
/// Cases: land:10.22, land:10.23, land:10.24, land:10.25, land:10.26, land:10.29, land:19.43, land:19.44
/// Unsupported: land:8.51 - reaction requires the separate reaction procedure.
#[allow(clippy::too_many_arguments)]
fn run(
    content: &CnaContent,
    state: &mut State,
    order: &Order,
    seat: SeatId,
    strict: bool,
    truth: bool,
    check_end: bool,
    events: &mut Vec<EngineEvent>,
) -> Result<Reachable, Rejection> {
    if order.path.is_empty() || order.path.len() > PATH_LIMIT as usize {
        return Err(illegal("path length is invalid"));
    }
    let moving = unit_stack(content, state, order, seat)?;
    let mut from = state.land.units[&order.unit]
        .location
        .hex()
        .unwrap()
        .clone();
    let mut path = Vec::new();
    let mut total = 0i32;
    let mut fuel = 0i32;
    let mut uncertain = false;
    let group_allowance = moving
        .iter()
        .map(|id| formation::individual_allowance(content, state, id))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| illegal("movement rating is unresolved"))?
        .into_iter()
        .min_by_key(|a| a.cpa)
        .ok_or_else(|| illegal("movement rating is unresolved"))?;
    let start_control = state
        .land
        .movement
        .controls
        .get(&from)
        .copied()
        .unwrap_or(false);
    let start_contact = effective_control(content, state, seat.side, &from, &moving, start_control);
    if start_contact {
        for id in &moving {
            capability::validate_move(&state.land.units[id], group_allowance, 8)?;
            capability::charge(
                state.land.units.get_mut(id).unwrap(),
                group_allowance,
                8,
                true,
            )?;
        }
        total += 8;
    }
    let mut last_legal = (
        state.clone(),
        from.clone(),
        path.clone(),
        total,
        fuel,
        events.len(),
    );
    for requested in &order.path {
        let to = content
            .map
            .canonical(requested)
            .ok_or_else(|| illegal("not a legal destination"))?
            .clone();
        if state
            .units_of(seat.side.opponent())
            .any(|u| u.location.hex() == Some(&to))
        {
            return Err(illegal("not a legal destination"));
        }
        map::terrain(content, &to, strict)?;
        let local_weather =
            logistics::weather::at_hex(content, state, &to).map_err(Rejection::Engine)?;
        let rain = local_weather == WeatherKind::Rainstorm;
        let mut costs = Vec::new();
        for id in &moving {
            costs.push(map::step_cost(
                content, state, id, &from, &to, strict, rain,
            )?);
        }
        let moving_sp: i32 = formation::roots(content, state, &from, seat.side)
            .iter()
            .filter(|id| moving.contains(id))
            .map(|id| formation::stacking_halves(content, state, id))
            .sum();
        let congested = costs.iter().any(|c| c.on_network)
            && moving.iter().any(|id| {
                formation::individual_allowance(content, state, id).is_some_and(|a| a.motorized)
            })
            && stacking::road_halves(content, state, &to, seat.side, &moving) + moving_sp > 10;
        if congested {
            costs.clear();
            for id in &moving {
                costs.push(map::step_cost_with_network(
                    content, state, id, &from, &to, strict, rain, false,
                )?);
            }
        }
        let mut cp = costs.iter().map(|c| c.cp_quarters).max().unwrap();
        if local_weather == WeatherKind::Sandstorm {
            cp = cp
                .checked_mul(2)
                .ok_or_else(|| illegal("CP expenditure overflow"))?;
        }
        let known = state.land.movement.controls.get(&to).copied();
        let control = if truth {
            zoc::controlled(content, state, seat.side.opponent(), &to, strict)
                .map_err(Rejection::Engine)?
        } else {
            known.unwrap_or(false)
        };
        uncertain |= known.is_none()
            && zoc::possibly_controlled(content, state, seat.side.opponent(), &to)
            && effective_control(content, state, seat.side, &to, &moving, true);
        let controlled = effective_control(content, state, seat.side, &to, &moving, control);
        if truth {
            state.land.movement.controls.insert(to.clone(), control);
        }
        let cannot_enter = controlled
            && (start_contact && path.is_empty()
                || !moving.iter().any(|id| {
                    formation::combat_unit(content, id)
                        && formation::strength(content, state, id) > 0
                }));
        if cannot_enter {
            if truth && known.is_none() {
                events.push(EngineEvent::new(Audience::Side(seat.side),GameEvent::Note{text:"Move stops before a newly disclosed controlled destination (land:10.24/10.29).".into()}));
                break;
            }
            return Err(illegal("destination is prohibited by disclosed control"));
        }
        for id in &moving {
            capability::validate_move(&state.land.units[id], group_allowance, cp)?;
            let previous = state
                .logistics
                .fuel_segments
                .get(id)
                .filter(|l| {
                    l.segment.game_turn == state.cursor.game_turn
                        && l.segment.op_stage == state.cursor.op_stage
                        && l.segment.half == state.cursor.half
                        && l.segment.cycle == state.cursor.cycle
                })
                .map_or(0, |l| l.cp_quarters);
            let next = previous
                .checked_add(cp)
                .ok_or_else(|| illegal("CP expenditure overflow"))?;
            let p = logistics::plan_segment_fuel(content, state, id, next)
                .map_err(|e| supply_error(e, strict))?;
            fuel = fuel
                .checked_add(p.increment.get())
                .ok_or_else(|| illegal("fuel expenditure overflow"))?;
            logistics::spend_segment_fuel(content, state, id, next)
                .map_err(|e| supply_error(e, strict))?;
            capability::charge(
                state.land.units.get_mut(id).unwrap(),
                group_allowance,
                cp,
                true,
            )?;
        }
        for id in &moving {
            state.land.units.get_mut(id).unwrap().location = Location::Hex { hex: to.clone() };
            if costs.iter().all(|c| c.on_network) {
                state.land.movement.off_road.remove(id);
            } else {
                state.land.movement.off_road.insert(id.clone());
            }
        }
        total = total
            .checked_add(cp)
            .ok_or_else(|| illegal("CP expenditure overflow"))?;
        path.push(to.clone());
        if truth {
            state.land.movement.controls.insert(to.clone(), control);
            let end = stacking::validate_end(content, state, &to, seat.side, strict);
            if let Err(Rejection::Engine(e)) = &end
                && controlled
            {
                return Err(Rejection::Engine(e.clone()));
            }
            if controlled && end.is_err() {
                let disclosed = state.land.movement.controls.clone();
                *state = last_legal.0;
                state.land.movement.controls = disclosed;
                from = last_legal.1;
                path = last_legal.2;
                total = last_legal.3;
                fuel = last_legal.4;
                events.truncate(last_legal.5);
                events.push(EngineEvent::new(Audience::Side(seat.side),GameEvent::Note {text:"Move stops at the last legal hex before newly disclosed control and stacking prevent continuation.".into()}));
                break;
            }
            if costs.iter().any(|c| c.assumed_edges) {
                events.push(EngineEvent::new(Audience::Side(seat.side),GameEvent::Note{text:format!("{from} to {to}: incomplete edge layers; plain terrain assumed (land:8.37).")}));
            }
            let adjacent_presence =
                zoc::possibly_controlled(content, state, seat.side.opponent(), &to);
            if adjacent_presence {
                if strict
                    && formation::combat_unit(content, &order.unit)
                    && content.map.neighbors(&to).iter().any(|h| {
                        state.units_of(seat.side.opponent()).any(|u| {
                            u.location.hex() == Some(&h.id)
                                && formation::combat_unit(content, &u.id)
                                && u.cohesion_quarters > -104
                                && formation::individual_allowance(content, state, &u.id)
                                    .is_some_and(|a| a.motorized)
                        })
                    })
                {
                    return Err(unsupported(
                        "land:8.51",
                        "reaction procedure is not implemented",
                    ));
                }
                if !strict {
                    events.push(EngineEvent::new(Audience::Side(seat.side),GameEvent::Note {text:"Movement is adjacent to an enemy stack; possible reaction is not implemented (land:8.51).".into()}));
                }
            }
            emit_stacks(content, state, seat.side, &from, events);
            emit_stacks(content, state, seat.side, &to, events);
            if stacking::validate_end(content, state, &to, seat.side, strict).is_ok() {
                last_legal = (
                    state.clone(),
                    to.clone(),
                    path.clone(),
                    total,
                    fuel,
                    events.len(),
                );
            }
        }
        from = to;
        if controlled {
            if !truth && path.len() != order.path.len() {
                return Err(illegal(
                    "path continues beyond a disclosed enemy zone of control",
                ));
            }
            break;
        }
    }
    let end = stacking::validate_end(content, state, &from, seat.side, strict);
    let end_valid = end.is_ok();
    if check_end && !path.is_empty() {
        end?;
    }
    state.land.movement.moved.extend(moving.iter().cloned());
    if truth {
        for id in &moving {
            events.push(EngineEvent::new(
                Audience::Side(seat.side),
                GameEvent::UnitMoved {
                    unit_id: id.to_string(),
                    path: path.iter().map(ToString::to_string).collect(),
                    cp_spent: (total % 4 == 0).then_some(total / 4),
                },
            ));
            events.push(EngineEvent::new(
                Audience::Side(seat.side),
                GameEvent::UnitUpdated {
                    unit: view::unit_view(content, &state.land.units[id]),
                },
            ));
        }
    }
    Ok(Reachable {
        hex: from,
        path,
        cp_quarters: total,
        fuel_tenths: fuel,
        control_unknown: uncertain,
        end_valid,
    })
}
/// Enemy event copies contain only stack presence; the operator receives the owner's full copy.
/// Cases: land:3.61, land:3.62
fn emit_stacks(
    _content: &CnaContent,
    state: &State,
    side: Side,
    hex: &HexId,
    events: &mut Vec<EngineEvent>,
) {
    let ids: Vec<_> = state
        .units_of(side)
        .filter(|u| u.location.hex() == Some(hex))
        .map(|u| u.id.to_string())
        .collect();
    if ids.is_empty() {
        for audience in [Audience::Side(side), Audience::SideOnly(side.opponent())] {
            events.push(EngineEvent::new(
                audience,
                GameEvent::StackRemoved {
                    hex: hex.to_string(),
                    side,
                },
            ));
        }
    } else {
        events.push(EngineEvent::new(
            Audience::Side(side),
            GameEvent::StackUpdated {
                stack: Stack {
                    hex: hex.to_string(),
                    side,
                    visible_count: Some(ids.len() as u32),
                    unit_ids: ids,
                },
            },
        ));
        events.push(EngineEvent::new(
            Audience::SideOnly(side.opponent()),
            GameEvent::StackUpdated {
                stack: Stack {
                    hex: hex.to_string(),
                    side,
                    visible_count: None,
                    unit_ids: vec![],
                },
            },
        ));
    }
}
/// Validate the entire answer against own information, then execute on a separate draft.
/// A rejected answer produces neither partial movement nor events, including direct handler use.
/// Cases: land:8.11, land:8.13, land:19.44
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    if action.is_null() {
        return Ok("Movement complete for this seat.".into());
    }
    let orders: Vec<Order> = serde_json::from_value(action.clone())
        .map_err(|_| illegal("expected an ordered list of unit/path/with_stack moves"))?;
    if orders.is_empty() {
        return Ok("Movement complete for this seat.".into());
    }
    if orders.len() > available(content, state, pending.seat).len() {
        return Err(illegal("too many movement orders"));
    }
    let mut preview = state.clone();
    for order in &orders {
        run(
            content,
            &mut preview,
            order,
            pending.seat,
            strict,
            false,
            true,
            &mut Vec::new(),
        )?;
    }
    let mut draft = state.clone();
    let mut events = Vec::new();
    for order in &orders {
        run(
            content,
            &mut draft,
            order,
            pending.seat,
            strict,
            true,
            true,
            &mut events,
        )?;
    }
    let updated: BTreeSet<_> = events
        .iter()
        .filter_map(|e| {
            if let GameEvent::UnitUpdated { unit } = &e.event {
                Some(UnitId::new(&unit.id))
            } else {
                None
            }
        })
        .collect();
    for (id, unit) in &draft.land.units {
        if state.land.units.get(id) != Some(unit) && !updated.contains(id) {
            events.push(EngineEvent::new(
                Audience::Side(unit.side),
                GameEvent::UnitUpdated {
                    unit: view::unit_view(content, unit),
                },
            ));
        }
    }
    *state = draft;
    cx.events.extend(events);
    open_seat(content, state, pending.seat, cx);
    Ok(format!(
        "Executed {} complete movement orders.",
        orders.len()
    ))
}
/// Paths are priced using only the side's disclosed control answers. An unknown future control
/// is labelled; inspect never asks authoritative enemy strength at a hypothetical destination.
/// Cases: land:3.62, land:8.13, land:8.17, land:10.6
pub fn reachable(content: &CnaContent, state: &State, id: &UnitId, strict: bool) -> Vec<Reachable> {
    let Some(unit) = state.land.units.get(id) else {
        return vec![];
    };
    let seat = SeatId::new(unit.side, ownership::seat_for_unit(content, state, id));
    if state.cursor.anchor() != "opstage.movement_and_combat.movement"
        || state.cursor.phasing(state.turn.player_a) != Some(unit.side)
        || !eligible(content, state, id, seat)
    {
        return vec![];
    }
    let origin = unit.location.hex().unwrap().clone();
    let initial_cp = unit.cp_spent_quarters;
    let mut best: BTreeMap<HexId, Reachable> = BTreeMap::new();
    let mut queue = BTreeSet::from([(0i32, origin.clone())]);
    let mut frontier = BTreeMap::from([(origin.clone(), state.clone())]);
    while let Some((cost, hex)) = queue.pop_first() {
        if hex != origin && best.get(&hex).is_none_or(|r| r.cp_quarters != cost) {
            continue;
        }
        let Some(node) = frontier.remove(&hex) else {
            continue;
        };
        let prior = best.get(&hex).cloned();
        if prior.as_ref().is_some_and(|r| {
            r.control_unknown
                || (state.land.movement.controls.get(&hex) == Some(&true)
                    && effective_control(
                        content,
                        &node,
                        seat.side,
                        &hex,
                        &formation::members(content, &node, id),
                        true,
                    ))
        }) {
            continue;
        }
        for to in content.map.neighbors(&hex) {
            if to.id == origin {
                continue;
            }
            let mut draft = node.clone();
            if let Ok(mut r) = run(
                content,
                &mut draft,
                &Order {
                    unit: id.clone(),
                    path: vec![to.id.clone()],
                    with_stack: false,
                },
                seat,
                strict,
                false,
                false,
                &mut Vec::new(),
            ) {
                let mut path = prior.as_ref().map_or_else(Vec::new, |p| p.path.clone());
                path.extend(r.path);
                r.path = path;
                r.cp_quarters = draft.land.units[id].cp_spent_quarters - initial_cp;
                r.fuel_tenths += prior.as_ref().map_or(0, |p| p.fuel_tenths);
                r.control_unknown |= prior.as_ref().is_some_and(|p| p.control_unknown);
                if best
                    .get(&to.id)
                    .is_none_or(|old| r.cp_quarters < old.cp_quarters)
                {
                    for member in formation::members(content, &draft, id) {
                        draft.land.movement.moved.remove(&member);
                    }
                    queue.insert((r.cp_quarters, to.id.clone()));
                    frontier.insert(to.id.clone(), draft);
                    best.insert(to.id.clone(), r);
                }
            }
        }
    }
    best.into_values().filter(|r| r.end_valid).collect()
}

#[cfg(test)]
#[path = "movement_tests.rs"]
mod tests;
