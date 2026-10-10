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
    /// Own-holdings CPA options and their eligibility, frozen before the triggering entry.
    #[serde(default)]
    pub cpa_options: BTreeMap<UnitId, BTreeMap<i32, bool>>,
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
pub(super) fn candidate_options(
    c: &CnaContent,
    s: &State,
    moving: &[UnitId],
    to: &HexId,
    announced: &[HexId],
    strict: bool,
) -> Result<BTreeMap<UnitId, BTreeMap<i32, bool>>, Rejection> {
    let side = s.land.units[&moving[0]].side;
    if !moving
        .iter()
        .any(|id| formation::combat_unit(c, id) && formation::strength(c, s, id) > 0)
    {
        return Ok(BTreeMap::new());
    }
    let mover_cpa = moving
        .iter()
        .filter_map(|id| formation::individual_allowance(c, s, id))
        .map(|a| a.cpa)
        .min()
        .unwrap_or(0);
    let targets: BTreeSet<_> = c.map.neighbors(to).iter().map(|h| h.id.clone()).collect();
    let mut out = BTreeMap::new();
    for u in s
        .units_of(side.opponent())
        .filter(|u| u.location.hex().is_some_and(|h| targets.contains(h)))
    {
        let members = formation::members(c, s, &u.id);
        if members.iter().any(|id| {
            s.land.units[id].engaged
                || s.land.combat.pinned.contains(id)
                || s.land.units[id].cohesion_quarters <= -104
                || super::reserve::validate_path(&s.land.units[id], 1).is_err()
        }) {
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
        let pinned_by_announcement = announced.contains(u.location.hex().unwrap())
            && moving.iter().any(|p| formation::can_pin(c, s, p, &u.id));
        let ratings = super::trucks::reachable_divisions(c, s, &u.id, strict)
            .into_keys()
            .map(|cpa| {
                (
                    cpa,
                    cpa > 10 && !(pinned_by_announcement && mover_cpa >= cpa.saturating_add(6)),
                )
            })
            .collect::<BTreeMap<_, _>>();
        if !ratings.is_empty() {
            out.insert(u.id.clone(), ratings);
        }
    }
    Ok(out)
}
/// Compatibility query for the unit eligibility declared by the saved CPA options.
/// Cases: land:8.51, land:8.53, land:8.56
#[cfg(test)]
pub(super) fn candidates(
    c: &CnaContent,
    s: &State,
    moving: &[UnitId],
    to: &HexId,
    announced: &[HexId],
    strict: bool,
) -> Result<Vec<UnitId>, Rejection> {
    Ok(candidate_options(c, s, moving, to, announced, strict)?
        .into_iter()
        .filter(|(_, ratings)| ratings.values().any(|yes| *yes))
        .map(|(id, _)| id)
        .collect())
}
/// A path may carry an explicit division of the represented family's trucks and holdings.
/// Cases: land:8.56
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReactionOrder {
    pub unit: UnitId,
    pub path: Vec<HexId>,
    #[serde(default)]
    pub with_stack: bool,
    #[serde(default)]
    pub close_assault: Vec<HexId>,
    #[serde(default)]
    pub truck_division: Option<super::trucks::Division>,
}
impl ReactionOrder {
    fn movement(&self) -> movement::Order {
        movement::Order {
            unit: self.unit.clone(),
            path: self.path.clone(),
            with_stack: self.with_stack,
            close_assault: self.close_assault.clone(),
        }
    }
}
/// Read only the eligibility already declared to this side; never re-query hidden pins.
/// A previously eligible parent stays eligible when its slower component reacts away.
/// Cases: land:8.53, land:8.54, land:8.56
fn validate_saved_cpa(
    c: &CnaContent,
    s: &State,
    draft: &State,
    order: &ReactionOrder,
) -> Result<(), Rejection> {
    let a = formation::allowance(c, draft, &order.unit)
        .ok_or_else(|| illegal("reaction movement rating is unresolved"))?;
    if !a.motorized {
        return Err(illegal("reaction requires motorized transport"));
    }
    let w = s
        .land
        .reaction
        .window
        .as_ref()
        .ok_or_else(|| illegal("reaction window is no longer open"))?;
    if let Some(options) = w.cpa_options.get(&order.unit) {
        let declared = options.get(&a.cpa).copied().unwrap_or(false);
        let faster_parent = order.truck_division.is_none()
            && ownership::parent_for_unit(c, s, &order.unit).is_none()
            && options.iter().any(|(cpa, yes)| *yes && *cpa <= a.cpa);
        if !declared && !faster_parent {
            return Err(illegal(
                "this CPA was not eligible at the reaction interrupt",
            ));
        }
    } else if order.truck_division.is_some() {
        return Err(illegal(
            "this saved reaction has no declared truck division options",
        ));
    }
    Ok(())
}
/// Reachability variants use current own holdings and the saved eligibility bit for each CPA.
/// The original window never needs to query the moving opponent again.
/// Cases: land:8.53, land:8.55, land:8.56
pub fn plans(
    c: &CnaContent,
    s: &State,
    id: &UnitId,
    strict: bool,
) -> Vec<(Option<super::trucks::Division>, Vec<movement::Reachable>)> {
    let Some(w) = &s.land.reaction.window else {
        return vec![];
    };
    if !w.eligible.contains(id)
        || formation::members(c, s, id)
            .iter()
            .any(|m| w.reacted.contains(m))
    {
        return vec![];
    }
    super::trucks::reachable_divisions(c, s, id, strict)
        .into_values()
        .filter_map(|division| {
            let draft = match &division {
                Some(d) => super::trucks::preview_reaction_division(c, s, id, d, strict).ok()?,
                None => s.clone(),
            };
            let order = ReactionOrder {
                unit: id.clone(),
                path: vec![],
                with_stack: false,
                close_assault: vec![],
                truck_division: division.clone(),
            };
            validate_saved_cpa(c, s, &draft, &order).ok()?;
            let paths = movement::nonphasing_reachable(
                c,
                &draft,
                id,
                strict,
                movement::NonPhasingMove::Reaction,
            );
            (!paths.is_empty()).then_some((division, paths))
        })
        .collect()
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
fn reaction_space(c: &CnaContent, s: &State, ids: Vec<UnitId>) -> ActionSpace {
    let family = ids
        .iter()
        .flat_map(|id| super::trucks::family(c, s, id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut a = space(ids, true);
    if let ActionSchema::List { item, .. } = &mut a.schema
        && let ActionSchema::Record { fields } = item.as_mut()
    {
        fields.extend([
            FieldSchema { name: "with_stack".into(), doc: "A reaction selects its complete represented unit, without unrelated counters.".into(), schema: ActionSchema::Bool, optional: true },
            FieldSchema { name: "close_assault".into(), doc: "Reaction cannot announce an offensive assault.".into(), schema: ActionSchema::List { min: 0, max: 0, item: Box::new(ActionSchema::Hex { among: None }) }, optional: true },
            FieldSchema { name: "truck_division".into(), doc: "Optional explicit division of parent-owned physical trucks, cargo, tank fuel and reserve activity water, followed by final packing.".into(), schema: super::trucks::division_schema(family), optional: true },
        ]);
    }
    a
}
// Optional fields may be sent as null by every ActionSpace consumer. Removing only the
// known default fields keeps unknown fields visible to the conformance/parser checks.
fn default_nulls(value: &mut Value) {
    match value {
        Value::Array(a) => a.iter_mut().for_each(default_nulls),
        Value::Object(a) => {
            for key in [
                "with_stack",
                "close_assault",
                "truck_division",
                "light",
                "medium",
                "heavy",
                "ammo",
                "fuel",
                "stores",
                "water",
            ] {
                if a.get(key).is_some_and(Value::is_null) {
                    a.remove(key);
                }
            }
            a.values_mut().for_each(default_nulls);
        }
        _ => {}
    }
}
fn continuation_space(id: UnitId, pass: bool) -> ActionSpace {
    let mut a = space(vec![id], pass);
    if let ActionSchema::List { item, .. } = &mut a.schema
        && let ActionSchema::Record { fields } = item.as_mut()
    {
        fields.push(FieldSchema {
            name: "with_stack".into(),
            doc: "Retain the interrupted move's represented stack selection.".into(),
            schema: ActionSchema::Bool,
            optional: true,
        });
        fields.push(FieldSchema {
            name: "close_assault".into(),
            doc: "Public enemy stack hexes against which the revised path announces close assault."
                .into(),
            schema: ActionSchema::List {
                item: Box::new(ActionSchema::Hex { among: None }),
                min: 0,
                max: 6,
            },
            optional: true,
        });
    }
    a
}
fn open_role(c: &CnaContent, s: &mut State, seat: SeatId, cx: &mut Cx<'_>, force: bool) {
    let ids = eligible(s, seat, c);
    if ids.is_empty() && !force {
        return;
    }
    let forced_pass = ids.is_empty();
    let window = s.land.reaction.window.as_ref().unwrap();
    let hex = window.trigger_hex.clone();
    let options: BTreeMap<_, _> = window
        .cpa_options
        .iter()
        .filter(|(id, _)| {
            ownership::seat_for_unit(c, s, id) == seat.role && s.land.units[*id].side == seat.side
        })
        .map(|(id, options)| (id.clone(), options.clone()))
        .collect();
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
        reaction_space(c, s, ids).with_context(
            serde_json::json!({"trigger_hex":hex,"forced_pass":forced_pass,"cpa_options":options}),
        ),
    );
}
/// Public adjacent stack presence opens the same defender-role windows on every entry.
/// Empty eligibility is a private forced pass, never a scheduling signal to the mover.
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
        open_role(c, s, SeatId::new(side, role), cx, true)
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
    let orders: Vec<ReactionOrder> = if action.is_null() {
        vec![]
    } else {
        let mut action = action.clone();
        default_nulls(&mut action);
        serde_json::from_value(action).map_err(|_| illegal("choose one reaction path or pass"))?
    };
    if orders.len() > 1 {
        return Err(illegal("choose one complete reaction at a time"));
    }
    if let Some(order) = orders.first() {
        if !legal.contains(&order.unit) || order.with_stack || !order.close_assault.is_empty() {
            return Err(illegal("unit is not available for this reaction"));
        }
        let mut draft = match &order.truck_division {
            Some(division) => {
                super::trucks::preview_reaction_division(c, s, &order.unit, division, strict)?
            }
            None => s.clone(),
        };
        validate_saved_cpa(c, s, &draft, order)?;
        let move_order = order.movement();
        movement::validate_nonphasing(
            c,
            &draft,
            &move_order,
            p.seat,
            strict,
            movement::NonPhasingMove::Reaction,
        )?;
        if let Some(division) = &order.truck_division {
            super::trucks::commit_reaction_division(c, s, &order.unit, division, strict, cx)?;
            draft = s.clone();
        }
        let members = formation::members(c, &draft, &order.unit);
        movement::execute_nonphasing(
            c,
            s,
            &move_order,
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
        if super::breakdown::window::park(
            s,
            Some(super::breakdown::window::Resume::Reaction { seat: p.seat }),
        ) {
            return Ok("Reaction choice complete.".into());
        }
        open_role(c, s, p.seat, cx, false);
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
/// Continue the reaction window only after the selected reactor's breakdown allocation ends.
/// Cases: land:8.51, land:21.24
pub(super) fn resume_after_breakdown(
    c: &CnaContent,
    s: &mut State,
    seat: SeatId,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), Rejection> {
    open_role(c, s, seat, cx, false);
    if !s.decisions.pending.iter().any(|p| p.kind == KIND) {
        s.land.reaction.window = None;
        open_continuation(c, s, strict, cx)?;
    }
    Ok(())
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
        cx.emit(EngineEvent::new(Audience::Side(k.seat.side),GameEvent::Note{text:"Interrupted movement stops with unresolved overstacking (land:9.31; interp:land-0026).".into()}).at(hex.clone()).about(k.unit.clone()));
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
        continuation_space(k.unit.clone(), stop).with_context(
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
        let mut action = action.clone();
        default_nulls(&mut action);
        serde_json::from_value(action)
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

#[cfg(test)]
pub(crate) fn plans_legacy_for_step1(
    c: &CnaContent,
    s: &State,
    id: &UnitId,
    strict: bool,
) -> Vec<(Option<super::trucks::Division>, Vec<movement::Reachable>)> {
    let Some(w) = &s.land.reaction.window else {
        return vec![];
    };
    if !w.eligible.contains(id)
        || formation::members(c, s, id)
            .iter()
            .any(|m| w.reacted.contains(m))
    {
        return vec![];
    }
    super::trucks::reachable_divisions(c, s, id, strict)
        .into_values()
        .filter_map(|division| {
            let draft = match &division {
                Some(d) => super::trucks::preview_reaction_division(c, s, id, d, strict).ok()?,
                None => s.clone(),
            };
            let order = ReactionOrder {
                unit: id.clone(),
                path: vec![],
                with_stack: false,
                close_assault: vec![],
                truck_division: division.clone(),
            };
            validate_saved_cpa(c, s, &draft, &order).ok()?;
            let paths = movement::nonphasing_reachable_legacy_for_step1(
                c,
                &draft,
                id,
                strict,
                movement::NonPhasingMove::Reaction,
            );
            (!paths.is_empty()).then_some((division, paths))
        })
        .collect()
}
