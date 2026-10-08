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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonPhasingMove {
    #[default]
    Reaction,
    Retreat,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WindowMode {
    #[default]
    Segment,
    Reaction,
    Retreat,
}

const PATH_LIMIT: u32 = 4096;
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MovementState {
    #[serde(default)]
    mode: WindowMode,
    #[serde(default)]
    pub starting_controls: BTreeMap<HexId, bool>,
    /// Profile of this entered window, so scripted controllers use the same reachability policy.
    #[serde(default)]
    pub strict: bool,
    pub moved: BTreeSet<UnitId>,
    /// Units that finished a prior segment too far from enemy units (8.23).
    #[serde(default)]
    pub cycle_blocked: BTreeSet<UnitId>,
    #[serde(default)]
    pub ended: bool,
    /// Truthful answers already disclosed to the phasing side in this segment.
    pub controls: BTreeMap<HexId, bool>,
    /// Units explicitly off the network; retained between segments until they use it again.
    pub off_road: BTreeSet<UnitId>,
    /// Actual supply pools share road occupancy only after using the network; absence means OFF.
    /// Cases: land:9.29, land:9.33, land:9.34
    #[serde(default)]
    pub pool_on_road: BTreeSet<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Order {
    pub unit: UnitId,
    pub path: Vec<HexId>,
    #[serde(default)]
    pub with_stack: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub close_assault: Vec<HexId>,
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
fn activity_error(error: SupplyError, strict: bool) -> Rejection {
    match error {
        SupplyError::Unsupported { case } if !strict => {
            illegal(format!("unit water requirement is unknown ({case})"))
        }
        SupplyError::Unsupported { case } => {
            unsupported(case, "unit water requirement is not supported")
        }
        SupplyError::Insufficient => illegal("insufficient activity water for movement"),
        other => supply_error(other, strict),
    }
}
/// Positive movement CP establishes the fuel rate before a separate water-composition query.
/// Cases: airlog:49.12, airlog:51.23, airlog:52.51, airlog:52.52, airlog:52.6
/// Interpretations: interp:units-0005
fn moving_limits(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    strict: bool,
) -> Result<logistics::MovementRestrictions, Rejection> {
    if state
        .land
        .breakdown
        .unresolved_passengers
        .get(id)
        .is_some_and(|v| !v.is_empty())
    {
        return Err(illegal(
            "unit has unresolved embarked infantry from split truck carriage (land:21.45, interp:land-0028)",
        ));
    }
    logistics::movement_fuel_cost(content, state, id, 1).map_err(|e| supply_error(e, strict))?;
    logistics::movement_restrictions(content, state, id).map_err(|e| activity_error(e, strict))
}
/// Every represented member uses the formation CPA; a restricted member sets its own ceiling.
/// Cases: land:6.15, airlog:51.23, airlog:52.51, airlog:52.52, airlog:52.6
fn validate_supplied_cp(
    unit: &crate::state::LandUnit,
    allowance: capability::Allowance,
    quarters: i32,
    limits: logistics::MovementRestrictions,
) -> Result<(), Rejection> {
    capability::validate_move(unit, allowance, quarters)?;
    if !limits.may_move {
        return Err(illegal(
            "unit cannot move under its ration or water restrictions",
        ));
    }
    if !limits.may_exceed_cpa
        && i64::from(unit.cp_spent_quarters) + i64::from(quarters) > i64::from(allowance.cpa) * 4
    {
        return Err(illegal(
            "ration or water restriction limits movement to CPA",
        ));
    }
    Ok(())
}
/// Activity water is consumed once per OpStage, before the first CPA expenditure.
/// Cases: airlog:52.42, airlog:52.43
fn spend_activity(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    strict: bool,
) -> Result<(), Rejection> {
    logistics::spend_activity_water(content, state, id).map_err(|e| activity_error(e, strict))
}
/// Units represented by a parent may be selected to detach; convoy trucks have their own phase.
/// Cases: land:6.26, land:8.11, land:8.17, land:8.18, land:19.44
fn eligible_base(content: &CnaContent, state: &State, id: &UnitId, seat: SeatId) -> bool {
    let Some(u) = state.land.units.get(id) else {
        return false;
    };
    u.side == seat.side
        && logistics::box_handling::blocks_movement(
            state,
            &logistics::box_handling::Carrier::Unit(id.clone()),
        )
        .is_none()
        && state
            .land
            .breakdown
            .unresolved_passengers
            .get(id)
            .is_none_or(Vec::is_empty)
        && u.location.hex().is_some()
        && ownership::seat_for_unit(content, state, id) == seat.role
        && (state.land.movement.mode != WindowMode::Segment
            || continuing(state, id)
            || !state.land.movement.moved.contains(id))
        && (state.land.movement.mode != WindowMode::Segment
            || super::cycles::movement_allowed(state, id))
        && super::reserve::validate_path(u, 1).is_ok()
        && (state.land.movement.mode != WindowMode::Segment
            || !matches!(
                content.units.units[id].kind.as_deref(),
                Some("convoy" | "second_line_truck" | "third_line_truck")
            ))
}
fn eligible(content: &CnaContent, state: &State, id: &UnitId, seat: SeatId) -> bool {
    eligible_base(content, state, id, seat)
        && formation::allowance(content, state, id)
            .is_some_and(|a| validate_window_move(state, &state.land.units[id], a, 1).is_ok())
}
fn available(content: &CnaContent, state: &State, seat: SeatId) -> Vec<UnitId> {
    let formations = formation::FormationIndex::new(content, state);
    let mut limits: BTreeMap<UnitId, Option<logistics::MovementRestrictions>> = BTreeMap::new();
    state
        .units_of(seat.side)
        .filter(|u| {
            eligible_base(content, state, &u.id, seat)
                && formations
                    .allowance(content, state, &u.id)
                    .is_some_and(|a| {
                        formations.members(&u.id).iter().all(|id| {
                            if logistics::box_handling::blocks_movement(
                                state,
                                &logistics::box_handling::Carrier::Unit(id.clone()),
                            )
                            .is_some()
                            {
                                return false;
                            }
                            if capability::validate_move(&state.land.units[id], a, 1).is_err() {
                                return false;
                            }
                            let limit = limits.entry(id.clone()).or_insert_with(|| {
                                logistics::movement_restrictions(content, state, id).ok()
                            });
                            // Unknown composition remains an explicit response error, not an assumed rate.
                            limit.is_none_or(|limit| {
                                validate_supplied_cp(&state.land.units[id], a, 1, limit).is_ok()
                            })
                        })
                    })
        })
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
        FieldSchema {name:"close_assault".into(),doc:"Public enemy stack hexes against which this move announces close assault.".into(),schema:ActionSchema::List {item:Box::new(ActionSchema::Hex{among:None}),min:0,max:6},optional:true},
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
    if state.cursor.cycle == 1 {
        state.land.movement.cycle_blocked.clear();
    } else {
        let released: Vec<_> = state
            .land
            .units
            .values()
            .filter(|u| u.reserve.released_for_cycle == Some(state.cursor.cycle))
            .map(|u| u.id.clone())
            .collect();
        for id in released {
            state.land.movement.cycle_blocked.remove(&id);
        }
    }
    state.land.movement.ended = false;
    state.land.movement.strict = strict;
    state.land.movement.moved.clear();
    state.land.movement.controls.clear();
    state.land.movement.starting_controls.clear();
    state.land.reaction = super::reaction::ReactionState::default();
    let starts: BTreeSet<_> = state
        .units_of(side)
        .filter_map(|u| u.location.hex().cloned())
        .collect();
    for hex in starts {
        let control = zoc::controlled(content, state, side.opponent(), &hex, strict)?;
        state.land.movement.controls.insert(hex, control);
    }
    state.land.movement.starting_controls = state.land.movement.controls.clone();
    for role in [Role::FrontLine, Role::RearArea, Role::Logistics] {
        open_seat(content, state, SeatId::new(side, role), cx);
    }
    super::cycles::finish_movement(content, state);
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
    strict: bool,
) -> Result<Vec<UnitId>, Rejection> {
    if continuing(state, &order.unit) {
        let k = state.land.reaction.continuation.as_ref().unwrap();
        if k.seat != seat || k.with_stack != order.with_stack || k.mover_stopped {
            return Err(illegal("unit cannot continue this move"));
        }
        return Ok(k.members.clone());
    }
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
    if members.iter().any(|id| {
        state.land.movement.mode == WindowMode::Segment
            && !super::cycles::movement_allowed(state, id)
    }) {
        return Err(illegal(
            "a represented unit cannot repeat movement this phase (land:8.23)",
        ));
    }
    for id in &members {
        super::reserve::validate_path(&state.land.units[id], order.path.len())?;
    }
    // Validate all moving fuel rates before querying water, including unknown-rate HQs.
    for id in &members {
        logistics::movement_fuel_cost(content, state, id, 1)
            .map_err(|e| supply_error(e, strict))?;
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
        let limits = logistics::movement_restrictions(content, state, &parent)
            .map_err(|e| activity_error(e, strict))?;
        validate_window_cp(state, &state.land.units[&parent], a, 4, limits)?;
        spend_activity(content, state, &parent, strict)?;
        let own_half = state.land.movement.mode == WindowMode::Segment;
        capability::charge(state.land.units.get_mut(&parent).unwrap(), a, 4, own_half)?;
        let a = formation::individual_allowance(content, state, &order.unit)
            .ok_or_else(|| illegal("movement rating is unresolved"))?;
        let limits = moving_limits(content, state, &order.unit, strict)?;
        validate_window_cp(state, &state.land.units[&order.unit], a, 4, limits)?;
        spend_activity(content, state, &order.unit, strict)?;
        capability::charge(
            state.land.units.get_mut(&order.unit).unwrap(),
            a,
            4,
            own_half,
        )?;
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
/// Cases: land:8.51, land:8.52, land:8.53, land:8.55, land:8.56
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
    planning_group: Option<&PlanningGroup<'_>>,
) -> Result<Reachable, Rejection> {
    if order.path.is_empty() || order.path.len() > PATH_LIMIT as usize {
        return Err(illegal("path length is invalid"));
    }
    validate_announcements(content, state, order)?;
    let initial_cp = state
        .land
        .units
        .get(&order.unit)
        .map_or(0, |u| u.cp_spent_quarters);
    let moving = match planning_group {
        Some(group) => group.members.to_vec(),
        None => unit_stack(content, state, order, seat, strict)?,
    };
    // Explicit detachment has already happened. A disclosed counter's executed path
    // remains visible even if its destination rejoins a stationary parent.
    let public_counters: BTreeSet<_> = if truth {
        moving
            .iter()
            .filter(|id| view::is_map_counter(content, state, &state.land.units[*id]))
            .cloned()
            .collect()
    } else {
        BTreeSet::new()
    };
    for id in &moving {
        if let Some(reason) = logistics::box_handling::blocks_movement(
            state,
            &logistics::box_handling::Carrier::Unit(id.clone()),
        ) {
            return Err(illegal(reason));
        }
    }
    let mut from = state.land.units[&order.unit]
        .location
        .hex()
        .unwrap()
        .clone();
    let mut path = Vec::new();
    let mut total = 0i32;
    let mut fuel = 0i32;
    let mut uncertain = false;
    let group_allowance = if let Some(group) = planning_group {
        group.allowance
    } else {
        moving
            .iter()
            .map(|id| formation::individual_allowance(content, state, id))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| illegal("movement rating is unresolved"))?
            .into_iter()
            .min_by_key(|a| a.cpa)
            .ok_or_else(|| illegal("movement rating is unresolved"))?
    };
    let limits = if let Some(group) = planning_group {
        group.limits.clone()
    } else {
        moving
            .iter()
            .map(|id| moving_limits(content, state, id, strict))
            .collect::<Result<Vec<_>, _>>()?
    };
    let own_half = state.land.movement.mode == WindowMode::Segment;
    let start_control = state
        .land
        .movement
        .starting_controls
        .get(&from)
        .copied()
        .unwrap_or(false);
    let start_contact = effective_control(content, state, seat.side, &from, &moving, start_control);
    if !continuing(state, &order.unit)
        && state.land.movement.mode != WindowMode::Reaction
        && (start_contact || moving.iter().any(|id| state.land.units[id].engaged))
    {
        let mut highest = 0;
        for (id, limit) in moving.iter().zip(&limits) {
            let cp = if state.land.units[id].engaged {
                16
            } else if start_contact {
                8
            } else {
                0
            };
            if cp == 0 {
                continue;
            }
            highest = highest.max(cp);
            validate_window_cp(state, &state.land.units[id], group_allowance, cp, *limit)?;
            spend_activity(content, state, id, strict)?;
            capability::charge(
                state.land.units.get_mut(id).unwrap(),
                group_allowance,
                cp,
                own_half,
            )?;
            if truth {
                super::engagement::break_off(state, id);
            } else {
                // Own-known planning prices the mover's breakoff, never updates its opponents.
                state.land.units.get_mut(id).unwrap().engaged = false;
            }
        }
        total += highest;
    }
    // Only authoritative execution needs a rollback snapshot for newly disclosed control.
    let mut last_legal = truth.then(|| {
        (
            state.clone(),
            from.clone(),
            path.clone(),
            total,
            fuel,
            events.len(),
        )
    });
    for requested in &order.path {
        let to = content
            .map
            .canonical(requested)
            .ok_or_else(|| illegal("not a legal destination"))?
            .clone();
        let occupied = match planning_group {
            Some(group) => group.enemy_positions.contains(&to),
            None => state.stack_presence(&to, seat.side.opponent()),
        };
        if occupied {
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
        let congested = if costs.iter().any(|c| c.on_network)
            && moving.iter().any(|id| {
                formation::individual_allowance(content, state, id).is_some_and(|a| a.motorized)
            }) {
            if let Some(group) = planning_group {
                stacking::road_over_limit(
                    group.stacks.road_halves(&to).map_err(Rejection::Engine)?,
                    group.stacks.moving_halves().map_err(Rejection::Engine)?,
                )
                .map_err(Rejection::Engine)?
            } else {
                let moving_sp: i32 = formation::roots(content, state, &from, seat.side)
                    .iter()
                    .filter(|id| moving.contains(id))
                    .map(|id| formation::stacking_halves(content, state, id))
                    .try_fold(0_i32, |sum, n| sum.checked_add(n))
                    .ok_or_else(|| {
                        Rejection::Engine(EngineError::Invariant {
                            detail: "moving road occupancy overflow".into(),
                        })
                    })?;
                stacking::road_over_limit(
                    stacking::road_halves(content, state, &to, seat.side, &moving)
                        .map_err(Rejection::Engine)?,
                    moving_sp,
                )
                .map_err(Rejection::Engine)?
            }
        } else {
            false
        };
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
        let adjacent_presence = match planning_group {
            Some(group) => group.enemy_adjacent(content, &to),
            None => zoc::possibly_controlled(content, state, seat.side.opponent(), &to),
        };
        uncertain |= known.is_none()
            && adjacent_presence
            && effective_control(content, state, seat.side, &to, &moving, true);
        let controlled = effective_control(content, state, seat.side, &to, &moving, control);
        if truth {
            state.land.movement.controls.insert(to.clone(), control);
        }
        let cannot_enter = controlled
            && (state.land.movement.mode == WindowMode::Reaction
                || start_contact && path.is_empty()
                || limits.iter().any(|limit| !limit.may_enter_enemy_zoc)
                || moving.iter().any(|id| {
                    state.land.units[id].reserve.status == super::reserve::Status::First
                })
                || !moving.iter().any(|id| {
                    formation::combat_unit(content, id)
                        && formation::strength(content, state, id) > 0
                }));
        if cannot_enter {
            if truth && known.is_none() {
                events.push(EngineEvent::new(Audience::Side(seat.side),GameEvent::Note{text:"Move stops before a newly disclosed controlled destination (land:10.24/10.29).".into()}).at(from.clone()).about(order.unit.clone()));
                break;
            }
            return Err(illegal(
                if limits.iter().any(|limit| !limit.may_enter_enemy_zoc) {
                    "half-ration unit may not enter an enemy zone of control"
                } else {
                    "destination is prohibited by disclosed control"
                },
            ));
        }
        let public_reaction = truth
            && own_half
            && content
                .map
                .neighbors(&to)
                .iter()
                .any(|h| state.stack_presence(&h.id, seat.side.opponent()));
        let reactors = if public_reaction {
            super::reaction::candidate_options(
                content,
                state,
                &moving,
                &to,
                &order.close_assault,
                strict,
            )?
        } else {
            BTreeMap::new()
        };
        for (id, limit) in moving.iter().zip(&limits) {
            validate_window_cp(state, &state.land.units[id], group_allowance, cp, *limit)?;
            spend_activity(content, state, id, strict)?;
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
            let spent = logistics::spend_segment_fuel_report(content, state, id, next)
                .map_err(|e| supply_error(e, strict))?;
            fuel = fuel
                .checked_add(spent.increment.get())
                .ok_or_else(|| illegal("fuel expenditure overflow"))?;
            capability::charge(
                state.land.units.get_mut(id).unwrap(),
                group_allowance,
                cp,
                own_half,
            )?;
        }
        let before_from = state.stack_presence(&from, seat.side);
        let before_to = state.stack_presence(&to, seat.side);
        for (id, cost) in moving.iter().zip(&costs) {
            if truth {
                super::breakdown::begin_motion(content, state, id, &from, strict);
                if state.land.units[id].trucks.light > 0 {
                    super::breakdown::record_light_extra(state, id, cost.light_extra_quarters)
                        .map_err(Rejection::Engine)?;
                }
                super::breakdown::record_edge(
                    state,
                    id,
                    &from,
                    cost.breakdown_quarters,
                    cp,
                    local_weather,
                )
                .map_err(Rejection::Engine)?;
            }
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
                let last_legal = last_legal.take().unwrap();
                *state = last_legal.0;
                state.land.movement.controls = disclosed;
                from = last_legal.1;
                path = last_legal.2;
                total = last_legal.3;
                fuel = last_legal.4;
                events.truncate(last_legal.5);
                events.push(EngineEvent::new(Audience::Side(seat.side),GameEvent::Note {text:"Move stops at the last legal hex before newly disclosed control and stacking prevent continuation.".into()}).at(from.clone()).about(order.unit.clone()));
                break;
            }
            if costs.iter().any(|c| c.assumed_edges) {
                events.push(EngineEvent::new(Audience::Side(seat.side),GameEvent::Note{text:format!("{from} to {to}: incomplete edge layers; plain terrain assumed (land:8.37).")}).at(to.clone()).about(order.unit.clone()));
            }
            if own_half {
                for target in &order.close_assault {
                    if content.map.neighbors(&to).iter().any(|h| &h.id == target)
                        && state
                            .land
                            .assault_intentions
                            .entry(order.unit.clone())
                            .or_default()
                            .insert(target.clone())
                    {
                        events.push(EngineEvent::public(GameEvent::Note{text:format!("Stack at {to} announces close assault against {target} (land:8.53).") }).at(target.clone()));
                    }
                }
                if public_reaction {
                    state.land.reaction.controls.clear();
                    state.land.reaction.window = Some(super::reaction::Window {
                        trigger_hex: to.clone(),
                        eligible: reactors
                            .iter()
                            .filter(|(_, ratings)| ratings.values().any(|yes| *yes))
                            .map(|(id, _)| id.clone())
                            .collect(),
                        cpa_options: reactors,
                        reacted: BTreeSet::new(),
                        mover_stopped: controlled
                            || moving.iter().any(|id| {
                                state.land.units[id].reserve.status == super::reserve::Status::First
                            }),
                        moving: moving.clone(),
                    });
                }
            }
            logistics::ports::record_entry(
                content,
                state,
                seat.side,
                &Location::Hex { hex: to.clone() },
                strict,
                events,
            )
            .map_err(Rejection::Engine)?;
            emit_stacks(
                content,
                state,
                seat.side,
                &from,
                before_from != state.stack_presence(&from, seat.side),
                events,
            );
            emit_stacks(
                content,
                state,
                seat.side,
                &to,
                before_to != state.stack_presence(&to, seat.side),
                events,
            );
            if stacking::validate_end(content, state, &to, seat.side, strict).is_ok() {
                last_legal = Some((
                    state.clone(),
                    to.clone(),
                    path.clone(),
                    total,
                    fuel,
                    events.len(),
                ));
            }
        }
        from = to;
        if truth && own_half && state.land.reaction.window.is_some() {
            break;
        }
        if controlled {
            if !truth && path.len() != order.path.len() {
                return Err(illegal(
                    "path continues beyond a disclosed enemy zone of control",
                ));
            }
            break;
        }
    }
    let end = match planning_group {
        Some(group) => group.stacks.validate_end(&from, strict),
        None => stacking::validate_end(content, state, &from, seat.side, strict),
    };
    let end_valid = end.is_ok();
    if check_end
        && !path.is_empty()
        && (state.land.movement.mode != WindowMode::Segment || state.land.reaction.window.is_none())
    {
        end?;
    }
    if truth
        && !path.is_empty()
        && (state.land.movement.mode != WindowMode::Segment || state.land.reaction.window.is_none())
    {
        super::breakdown::stop(state, &moving, &from);
    }
    state.land.movement.moved.extend(moving.iter().cloned());
    if truth {
        for id in &moving {
            // Printed map counters are visible under RAW fog; attached contents are not.
            // A side-only copy avoids duplicating the owner's full event for the operator.
            if public_counters.contains(id)
                || view::is_map_counter(content, state, &state.land.units[id])
            {
                events.push(
                    EngineEvent::new(
                        Audience::SideOnly(seat.side.opponent()),
                        GameEvent::UnitMoved {
                            unit_id: id.to_string(),
                            path: path.iter().map(ToString::to_string).collect(),
                            cp_spent: None,
                        },
                    )
                    .at(from.clone())
                    .about(id.clone()),
                );
            }
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
        cp_quarters: state.land.units[&order.unit].cp_spent_quarters - initial_cp,
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
    enemy_presence_changed: bool,
    events: &mut Vec<EngineEvent>,
) {
    let mut ids: Vec<_> = state
        .units_of(side)
        .filter(|u| u.location.hex() == Some(hex))
        .map(|u| u.id.to_string())
        .collect();
    ids.extend(
        state
            .land
            .breakdown
            .markers
            .values()
            .filter(|m| m.side == side && &m.hex == hex)
            .map(|m| m.id.clone()),
    );
    ids.sort();
    if ids.is_empty() {
        for audience in std::iter::once(Audience::Side(side))
            .chain(enemy_presence_changed.then_some(Audience::SideOnly(side.opponent())))
        {
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
        if enemy_presence_changed {
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
        super::cycles::finish_movement(content, state);
        return Ok("Movement complete for this seat.".into());
    }
    let orders: Vec<Order> = serde_json::from_value(action.clone())
        .map_err(|_| illegal("expected an ordered list of unit/path/with_stack moves"))?;
    if orders.is_empty() {
        super::cycles::finish_movement(content, state);
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
            None,
        )?;
    }
    execute_orders(content, state, pending.seat, &orders, strict, cx, false)?;
    Ok(format!(
        "Accepted {} planned movement orders.",
        orders.len()
    ))
}
impl Order {
    pub fn new(unit: UnitId, path: Vec<HexId>) -> Self {
        Self {
            unit,
            path,
            with_stack: false,
            close_assault: vec![],
        }
    }
}
fn continuing(state: &State, id: &UnitId) -> bool {
    state.land.movement.mode == WindowMode::Segment
        && state.land.reaction.window.is_none()
        && state
            .land
            .reaction
            .continuation
            .as_ref()
            .is_some_and(|k| &k.unit == id && !k.mover_stopped)
}
fn validate_window_move(
    state: &State,
    u: &crate::state::LandUnit,
    a: capability::Allowance,
    q: i32,
) -> Result<(), Rejection> {
    if state.land.movement.mode == WindowMode::Segment {
        capability::validate_move(u, a, q)
    } else {
        capability::validate_nonphasing_move(u, a, q)
    }
}
fn validate_window_cp(
    state: &State,
    u: &crate::state::LandUnit,
    a: capability::Allowance,
    q: i32,
    l: logistics::MovementRestrictions,
) -> Result<(), Rejection> {
    validate_window_move(state, u, a, q)?;
    if !l.may_move {
        return Err(illegal(
            "unit cannot move under its ration or water restrictions",
        ));
    }
    if !l.may_exceed_cpa && i64::from(u.cp_spent_quarters) + i64::from(q) > i64::from(a.cpa) * 4 {
        return Err(illegal(
            "ration or water restriction limits movement to CPA",
        ));
    }
    Ok(())
}
/// An assault announcement is a public commitment against a publicly present stack.
/// Cases: land:8.53, land:8.54
fn validate_announcements(c: &CnaContent, s: &State, o: &Order) -> Result<(), Rejection> {
    if o.close_assault.is_empty() {
        return Ok(());
    }
    if s.land.movement.mode != WindowMode::Segment || !formation::combat_unit(c, &o.unit) {
        return Err(illegal("this unit cannot announce close assault"));
    }
    let Some(u) = s.land.units.get(&o.unit) else {
        return Err(illegal("unit is unavailable"));
    };
    let members = formation::members(c, s, &o.unit);
    for id in &members {
        if matches!(
            s.land.units[id].reserve.status,
            super::reserve::Status::First | super::reserve::Status::Second
        ) || !logistics::movement_restrictions(c, s, id)
            .map_err(|e| activity_error(e, s.land.movement.strict))?
            .may_offensive_close_assault
        {
            return Err(illegal("unit cannot announce offensive close assault"));
        }
    }
    let mut seen = BTreeSet::new();
    for h in &o.close_assault {
        if !seen.insert(h)
            || c.map.canonical(h) != Some(h)
            || !s
                .units_of(u.side.opponent())
                .any(|e| e.location.hex() == Some(h))
            || !o
                .path
                .iter()
                .any(|p| c.map.neighbors(p).iter().any(|n| &n.id == h))
        {
            return Err(illegal(
                "assault target must be a public enemy stack adjacent to the planned path",
            ));
        }
    }
    Ok(())
}
/// A truth-only failure cannot invalidate a seat's already valid plan.
/// Adjudication reports it once ordinary and interrupt decisions have been parked.
/// Cases: land:10.21, land:10.6
pub(super) fn defer_stop(state: &mut State, error: EngineError) {
    state.land.reaction.adjudication_stop = Some(error);
    state
        .land
        .reaction
        .held
        .extend(std::mem::take(&mut state.decisions.pending));
}
/// Execute accepted plans until a reaction suspends them. Later plans remain checkpointed.
/// Cases: land:8.13, land:8.51, land:8.52, land:10.6
pub(super) fn resume_orders(
    c: &CnaContent,
    s: &mut State,
    seat: SeatId,
    orders: &[Order],
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), Rejection> {
    execute_orders(c, s, seat, orders, strict, cx, true)
}
fn execute_orders(
    c: &CnaContent,
    s: &mut State,
    seat: SeatId,
    orders: &[Order],
    strict: bool,
    cx: &mut Cx<'_>,
    skip_invalid: bool,
) -> Result<(), Rejection> {
    for (index, o) in orders.iter().enumerate() {
        let mut trial = s.clone();
        let mut events = vec![];
        // Resumed lists can have become impossible after public reaction moves.
        let mut preview = trial.clone();
        if let Err(e) = run(
            c,
            &mut preview,
            o,
            seat,
            strict,
            false,
            true,
            &mut vec![],
            None,
        ) {
            if !skip_invalid && index == 0 {
                return Err(e);
            }
            cx.emit(
                EngineEvent::new(
                    Audience::Side(seat.side),
                    GameEvent::Note {
                        text:
                            "A remaining planned move is unavailable after reaction; order skipped."
                                .into(),
                    },
                )
                .about(o.unit.clone()),
            );
            s.land.movement.moved.insert(o.unit.clone());
            continue;
        }
        let r = match run(
            c,
            &mut trial,
            o,
            seat,
            strict,
            true,
            true,
            &mut events,
            None,
        ) {
            Ok(r) => r,
            Err(Rejection::Engine(e)) => {
                defer_stop(s, e);
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        // Detaching a component also changes its stationary parent.
        for (id, u) in &trial.land.units {
            if s.land
                .units
                .get(id)
                .is_some_and(|old| old.cp_spent_quarters != u.cp_spent_quarters)
                && !events.iter().any(
                    |e| matches!(&e.event,GameEvent::UnitUpdated{unit} if unit.id==id.to_string()),
                )
            {
                events.push(EngineEvent::new(
                    Audience::Side(u.side),
                    GameEvent::UnitUpdated {
                        unit: view::unit_view(c, u),
                    },
                ));
            }
        }
        *s = trial;
        cx.events.extend(events);
        if let Some(w) = &s.land.reaction.window {
            s.land.reaction.continuation = Some(super::reaction::Continuation {
                seat,
                unit: o.unit.clone(),
                members: w.moving.clone(),
                with_stack: o.with_stack,
                planned_path: o.path[r.path.len()..].to_vec(),
                orders_after: orders[index + 1..].to_vec(),
                mover_stopped: w.mover_stopped,
            });
            super::reaction::open_interrupt(c, s, cx);
            return Ok(());
        }
        if super::breakdown::window::park(
            s,
            Some(super::breakdown::window::Resume::Orders {
                seat,
                orders: orders[index + 1..].to_vec(),
            }),
        ) {
            return Ok(());
        }
    }
    if !s
        .decisions
        .pending
        .iter()
        .any(|p| p.kind == KIND && p.seat == seat)
    {
        open_seat(c, s, seat, cx)
    }
    super::cycles::finish_movement(c, s);
    Ok(())
}
/// Resume the same represented membership without repeating detachment or breaking-off CP.
/// Cases: land:8.13, land:8.51, land:9.31
/// Interpretations: interp:land-0026
pub(super) fn continue_order(
    c: &CnaContent,
    s: &mut State,
    o: &Order,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), Rejection> {
    let k = s.land.reaction.continuation.as_ref().unwrap().clone();
    let mut preview = s.clone();
    run(
        c,
        &mut preview,
        o,
        k.seat,
        strict,
        false,
        true,
        &mut vec![],
        None,
    )?;
    let mut draft = s.clone();
    let mut events = vec![];
    let r = match run(
        c,
        &mut draft,
        o,
        k.seat,
        strict,
        true,
        true,
        &mut events,
        None,
    ) {
        Ok(r) => r,
        Err(Rejection::Engine(e)) => {
            defer_stop(s, e);
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    *s = draft;
    cx.events.extend(events);
    if let Some(w) = &s.land.reaction.window {
        s.land.reaction.continuation = Some(super::reaction::Continuation {
            planned_path: o.path[r.path.len()..].to_vec(),
            mover_stopped: w.mover_stopped,
            ..k
        });
        super::reaction::open_interrupt(c, s, cx);
        Ok(())
    } else {
        finish_continuation(c, s, strict, cx)
    }
}
/// Restore the parked seat windows only after the moving unit's complete move ends.
/// Cases: land:8.13, land:8.51
pub(super) fn finish_continuation(
    c: &CnaContent,
    s: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), Rejection> {
    let k = s.land.reaction.continuation.take().unwrap();
    let hex = s.land.units[&k.unit].location.hex().unwrap().clone();
    super::breakdown::stop(s, &k.members, &hex);
    s.land.reaction.window = None;
    s.decisions
        .pending
        .extend(std::mem::take(&mut s.land.reaction.held));
    if super::breakdown::window::park(
        s,
        Some(super::breakdown::window::Resume::Orders {
            seat: k.seat,
            orders: k.orders_after.clone(),
        }),
    ) {
        return Ok(());
    }
    execute_orders(c, s, k.seat, &k.orders_after, strict, cx, true)
}
fn nonphasing_draft(s: &State, kind: NonPhasingMove) -> State {
    let mut draft = s.clone();
    draft.land.movement.mode = match kind {
        NonPhasingMove::Reaction => WindowMode::Reaction,
        NonPhasingMove::Retreat => WindowMode::Retreat,
    };
    draft.land.movement.controls = draft.land.reaction.controls.clone();
    draft.land.movement.starting_controls = draft.land.reaction.controls.clone();
    draft
}
/// Shared own-information path search for nonphasing reaction and retreat windows.
/// The caller retains source eligibility and retreat distance limits.
/// Cases: land:8.17, land:8.55, land:13.21, land:13.22, land:13.26
pub fn nonphasing_reachable(
    c: &CnaContent,
    s: &State,
    id: &UnitId,
    strict: bool,
    kind: NonPhasingMove,
) -> Vec<Reachable> {
    reachable_inner(c, &nonphasing_draft(s, kind), id, strict)
}
/// Pure own-information validation; neither the input state nor adjudication dice changes.
/// Cases: land:13.21, land:13.22, land:13.26
pub fn validate_nonphasing(
    c: &CnaContent,
    s: &State,
    o: &Order,
    seat: SeatId,
    strict: bool,
    kind: NonPhasingMove,
) -> Result<Reachable, Rejection> {
    preview_nonphasing(c, s, o, seat, strict, kind).map(|(_, cost)| cost)
}
/// Retain an own-known simulation for ordered plan validation, never an authoritative transition.
/// Cases: land:13.21, land:13.22, land:13.26
pub(crate) fn preview_nonphasing(
    c: &CnaContent,
    s: &State,
    o: &Order,
    seat: SeatId,
    strict: bool,
    kind: NonPhasingMove,
) -> Result<(State, Reachable), Rejection> {
    let mut draft = nonphasing_draft(s, kind);
    let cost = run(
        c,
        &mut draft,
        o,
        seat,
        strict,
        false,
        true,
        &mut vec![],
        None,
    )?;
    Ok((draft, cost))
}

/// Execute a prevalidated nonphasing complete path, preserving the ordinary movement window.
/// Hidden control stops the accepted move and a hidden technical gap stops adjudication.
/// Cases: land:8.55, land:8.56, land:13.21, land:13.22, land:13.26
pub fn execute_nonphasing(
    c: &CnaContent,
    s: &mut State,
    o: &Order,
    seat: SeatId,
    strict: bool,
    kind: NonPhasingMove,
    cx: &mut Cx<'_>,
) -> Result<Reachable, Rejection> {
    let mut draft = nonphasing_draft(s, kind);
    let mut preview = draft.clone();
    run(
        c,
        &mut preview,
        o,
        seat,
        strict,
        false,
        true,
        &mut vec![],
        None,
    )?;
    let mut events = vec![];
    let r = match run(
        c,
        &mut draft,
        o,
        seat,
        strict,
        true,
        true,
        &mut events,
        None,
    ) {
        Ok(r) => r,
        Err(Rejection::Engine(e)) => {
            defer_stop(s, e);
            return Ok(Reachable {
                hex: s.land.units[&o.unit].location.hex().unwrap().clone(),
                path: vec![],
                cp_quarters: 0,
                fuel_tenths: 0,
                control_unknown: true,
                end_valid: true,
            });
        }
        Err(e) => return Err(e),
    };
    let controls = draft.land.movement.controls.clone();
    let off_road = draft.land.movement.off_road.clone();
    draft.land.movement = s.land.movement.clone();
    draft.land.movement.controls.clear(); // The enemy moved publicly; stale disclosed absence is invalid.
    draft.land.movement.off_road = off_road;
    draft.land.reaction.controls = controls;
    *s = draft;
    cx.events.extend(events);
    Ok(r)
}

// Search nodes contain only the fields `run(..., truth=false)` can change. One working
// world is reused across edges; unrelated units, air state and decision schemas are not copied.
struct PlanningGroup<'a> {
    members: &'a [UnitId],
    allowance: capability::Allowance,
    stacks: stacking::PlanningStacks<'a>,
    enemy_positions: BTreeSet<HexId>,
    limits: Vec<logistics::MovementRestrictions>,
}
impl PlanningGroup<'_> {
    fn enemy_adjacent(&self, content: &CnaContent, hex: &HexId) -> bool {
        content
            .map
            .neighbors(hex)
            .iter()
            .any(|h| self.enemy_positions.contains(&h.id))
    }
}
struct PlanningNode {
    fuel_accounts: logistics::FuelAccountSnapshot,
    units: Vec<crate::state::LandUnit>,
    unit_supply: Vec<(UnitId, Option<crate::state::UnitSupply>)>,
    dumps: Vec<(String, crate::state::Dump)>,
    fuel_segments: Vec<(UnitId, Option<logistics::FuelSegmentLedger>)>,
    rations: Vec<(UnitId, Option<logistics::Rations>)>,
    movement: MovementState,
}
impl PlanningNode {
    fn capture(state: &State, changed: &[UnitId], stocks: &[UnitId], dumps: &[String]) -> Self {
        Self {
            fuel_accounts: logistics::snapshot_fuel_accounts(state, changed),
            units: changed
                .iter()
                .filter_map(|id| state.land.units.get(id).cloned())
                .collect(),
            unit_supply: stocks
                .iter()
                .map(|id| (id.clone(), state.logistics.unit_supply.get(id).cloned()))
                .collect(),
            dumps: dumps
                .iter()
                .map(|id| (id.clone(), state.logistics.dumps[id].clone()))
                .collect(),
            fuel_segments: changed
                .iter()
                .map(|id| (id.clone(), state.logistics.fuel_segments.get(id).cloned()))
                .collect(),
            rations: changed
                .iter()
                .map(|id| (id.clone(), state.logistics.rations.get(id).cloned()))
                .collect(),
            movement: state.land.movement.clone(),
        }
    }
    fn restore(&self, state: &mut State) {
        logistics::restore_fuel_accounts(state, &self.fuel_accounts);
        for unit in &self.units {
            state.land.units.insert(unit.id.clone(), unit.clone());
        }
        for (id, stock) in &self.unit_supply {
            match stock {
                Some(stock) => {
                    state
                        .logistics
                        .unit_supply
                        .insert(id.clone(), stock.clone());
                }
                None => {
                    state.logistics.unit_supply.remove(id);
                }
            }
        }
        for (id, dump) in &self.dumps {
            state.logistics.dumps.insert(id.clone(), dump.clone());
        }
        for (id, ledger) in &self.fuel_segments {
            match ledger {
                Some(ledger) => {
                    state
                        .logistics
                        .fuel_segments
                        .insert(id.clone(), ledger.clone());
                }
                None => {
                    state.logistics.fuel_segments.remove(id);
                }
            }
        }
        for (id, rations) in &self.rations {
            match rations {
                Some(rations) => {
                    state.logistics.rations.insert(id.clone(), rations.clone());
                }
                None => {
                    state.logistics.rations.remove(id);
                }
            }
        }
        state.land.movement.clone_from(&self.movement);
    }
}
/// Paths are priced using only the side's disclosed control answers. An unknown future control
/// is labelled; inspect never asks authoritative enemy strength at a hypothetical destination.
/// Cases: land:3.62, land:8.13, land:8.17, land:10.6
pub fn reachable(content: &CnaContent, state: &State, id: &UnitId, strict: bool) -> Vec<Reachable> {
    if state.land.movement.mode == WindowMode::Segment
        && state
            .land
            .reaction
            .window
            .as_ref()
            .is_some_and(|w| w.eligible.contains(id) && !w.reacted.contains(id))
    {
        return nonphasing_reachable(content, state, id, strict, NonPhasingMove::Reaction);
    }
    reachable_inner(content, state, id, strict)
}
fn reachable_inner(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    strict: bool,
) -> Vec<Reachable> {
    let Some(unit) = state.land.units.get(id) else {
        return vec![];
    };
    let seat = SeatId::new(unit.side, ownership::seat_for_unit(content, state, id));
    if (state.land.movement.mode == WindowMode::Segment
        && state.cursor.anchor() != "opstage.movement_and_combat.movement")
        || (state.land.movement.mode == WindowMode::Segment
            && state.cursor.phasing(state.turn.player_a) != Some(unit.side))
        || !eligible(content, state, id, seat)
    {
        return vec![];
    }
    let moving = state
        .land
        .reaction
        .continuation
        .as_ref()
        .filter(|_| continuing(state, id))
        .map_or_else(
            || formation::members(content, state, id),
            |k| k.members.clone(),
        );
    let Some(allowance) = moving
        .iter()
        .filter_map(|m| formation::individual_allowance(content, state, m))
        .min_by_key(|a| a.cpa)
    else {
        return vec![];
    };
    // Every entered edge costs at least one quarter CP. Do not search a graph when even
    // that lower bound is impossible for a represented member or its fuel holdings.
    for member in &moving {
        if moving_limits(content, state, member, strict)
            .and_then(|limit| {
                validate_window_cp(state, &state.land.units[member], allowance, 1, limit)
            })
            .is_err()
        {
            return vec![];
        }
        let previous = state
            .logistics
            .fuel_segments
            .get(member)
            .filter(|ledger| {
                ledger.segment.game_turn == state.cursor.game_turn
                    && ledger.segment.op_stage == state.cursor.op_stage
                    && ledger.segment.half == state.cursor.half
                    && ledger.segment.cycle == state.cursor.cycle
            })
            .map_or(0, |ledger| ledger.cp_quarters);
        if previous
            .checked_add(1)
            .is_none_or(|next| logistics::plan_segment_fuel(content, state, member, next).is_err())
        {
            return vec![];
        }
    }
    let origin = unit.location.hex().unwrap().clone();
    let initial_cp = unit.cp_spent_quarters;
    let mut best: BTreeMap<HexId, Reachable> = BTreeMap::new();
    let mut queue = BTreeSet::from([(0i32, origin.clone())]);
    let mut changed = moving;
    if let Some(parent) = ownership::parent_for_unit(content, state, id) {
        changed.push(parent.clone());
    }
    let mut draft = state.clone();
    let Ok(members) = unit_stack(
        content,
        &mut draft,
        &Order {
            unit: id.clone(),
            path: vec![],
            with_stack: state
                .land
                .reaction
                .continuation
                .as_ref()
                .filter(|_| continuing(state, id))
                .is_some_and(|k| k.with_stack),
            close_assault: vec![],
        },
        seat,
        strict,
    ) else {
        return vec![];
    };
    let Ok(limits) = members
        .iter()
        .map(|id| moving_limits(content, &draft, id, strict))
        .collect::<Result<Vec<_>, _>>()
    else {
        return vec![];
    };
    let base = draft.clone();
    let group = PlanningGroup {
        members: &members,
        allowance,
        stacks: stacking::PlanningStacks::new(content, &base, seat.side, &members, &origin),
        limits,
        enemy_positions: state
            .stacks()
            .into_keys()
            .filter(|(_, side)| *side == seat.side.opponent())
            .map(|(h, _)| h)
            .collect(),
    };
    // Fuel can debit the current members' tanks and friendly stocks at each captured
    // segment origin. Activity water changes members and a stationary detached parent.
    // These source identities are query-local; no other holding can change in this search.
    let mut origins: BTreeSet<_> = members
        .iter()
        .map(|m| {
            state
                .logistics
                .fuel_segments
                .get(m)
                .filter(|l| {
                    l.segment.game_turn == state.cursor.game_turn
                        && l.segment.op_stage == state.cursor.op_stage
                        && l.segment.half == state.cursor.half
                        && l.segment.cycle == state.cursor.cycle
                })
                .and_then(|l| l.origin.hex())
                .cloned()
                .unwrap_or_else(|| origin.clone())
        })
        .collect();
    // Incoming physical cohorts retain their original funding account and sources.
    // Restore both credit and the corresponding holdings when exploring another branch.
    let accounts: BTreeSet<_> = members
        .iter()
        .flat_map(|id| {
            state
                .logistics
                .fuel_segments
                .get(id)
                .into_iter()
                .flat_map(|l| {
                    std::iter::once(id.clone()).chain(l.cohorts.iter().map(|g| g.account.clone()))
                })
        })
        .collect();
    for account in accounts
        .iter()
        .filter_map(|id| state.logistics.fuel_accounts.get(id))
    {
        if let Some(hex) = account.origin.hex() {
            origins.insert(hex.clone());
        }
    }
    let mut stocks: BTreeSet<_> = changed.iter().cloned().collect();
    stocks.extend(accounts.iter().cloned());
    stocks.extend(
        state
            .units_of(seat.side)
            .filter(|u| u.location.hex().is_some_and(|h| origins.contains(h)))
            .map(|u| u.id.clone()),
    );
    let mut dumps:BTreeSet<_>=state.logistics.dumps.iter().filter(|(_,d)|d.side==seat.side&&matches!(&d.location,crate::state::DumpLocation::Hex{hex} if origins.contains(hex))).map(|(id,_)|id.clone()).collect();
    for id in &members {
        if let Some(l) = state.logistics.fuel_segments.get(id) {
            for draw in &l.draws {
                match &draw.source {
                    logistics::SupplySource::UnitStock(id) => {
                        stocks.insert(id.clone());
                    }
                    logistics::SupplySource::Dump(id) => {
                        dumps.insert(id.clone());
                    }
                    _ => {}
                }
            }
        }
    }
    for account in accounts
        .iter()
        .filter_map(|id| state.logistics.fuel_accounts.get(id))
    {
        for draw in &account.draws {
            match &draw.source {
                logistics::SupplySource::UnitStock(id) => {
                    stocks.insert(id.clone());
                }
                logistics::SupplySource::Dump(id) => {
                    dumps.insert(id.clone());
                }
                _ => {}
            }
        }
    }
    let stocks = stocks.into_iter().collect::<Vec<_>>();
    let dumps = dumps
        .into_iter()
        .filter(|id| state.logistics.dumps.contains_key(id))
        .collect::<Vec<_>>();
    let mut frontier = BTreeMap::from([(
        origin.clone(),
        PlanningNode::capture(&draft, &changed, &stocks, &dumps),
    )]);
    while let Some((cost, hex)) = queue.pop_first() {
        if hex != origin && best.get(&hex).is_none_or(|r| r.cp_quarters != cost) {
            continue;
        }
        let Some(node) = frontier.remove(&hex) else {
            continue;
        };
        node.restore(&mut draft);
        if group
            .members
            .iter()
            .zip(&group.limits)
            .any(|(member, limit)| {
                validate_window_cp(
                    &draft,
                    &draft.land.units[member],
                    group.allowance,
                    1,
                    *limit,
                )
                .is_err()
            })
        {
            continue;
        }
        if hex != origin
            && group
                .members
                .iter()
                .any(|id| draft.land.units[id].reserve.status == super::reserve::Status::First)
        {
            continue;
        }
        let prior = best.get(&hex).cloned();
        if prior.as_ref().is_some_and(|r| {
            r.control_unknown
                || (state.land.movement.controls.get(&hex) == Some(&true)
                    && effective_control(
                        content,
                        &draft,
                        seat.side,
                        &hex,
                        &formation::members(content, &draft, id),
                        true,
                    ))
        }) {
            continue;
        }
        for to in content.map.neighbors(&hex) {
            if to.id == origin
                || best
                    .get(&to.id)
                    .is_some_and(|r| r.cp_quarters <= cost.saturating_add(1))
                || map::terrain(content, &to.id, strict).is_err()
            {
                continue;
            }
            node.restore(&mut draft);
            if let Ok(mut r) = run(
                content,
                &mut draft,
                &Order {
                    unit: id.clone(),
                    path: vec![to.id.clone()],
                    with_stack: state
                        .land
                        .reaction
                        .continuation
                        .as_ref()
                        .filter(|_| continuing(state, id))
                        .is_some_and(|k| k.with_stack),
                    close_assault: vec![],
                },
                seat,
                strict,
                false,
                false,
                &mut Vec::new(),
                Some(&group),
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
                    frontier.insert(
                        to.id.clone(),
                        PlanningNode::capture(&draft, &changed, &stocks, &dumps),
                    );
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
