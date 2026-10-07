//! Nonphasing retreat plans close before any unit, fuel, water or dice is changed.
use super::super::{
    formation,
    movement::{self, NonPhasingMove, Order, Reachable},
    zoc,
};
use crate::{
    CnaContent, State, ownership,
    state::Pending,
    steps::{illegal, open},
    view,
};
use cna_core::{
    decision::{ActionSchema, ActionSpace, DecisionRequest, FieldSchema, Secrecy, Trigger},
    dice::CampaignRng,
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{HexId, SeatId, UnitId},
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const KIND: &str = "cna.combat.retreat_before_assault";
pub const ANCHOR: &str = "opstage.movement_and_combat.combat.retreat_before_assault";
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RetreatState {
    pub strict: bool,
    pub entered: bool,
    pub closed: bool,
    pub resolved: bool,
    /// Durable index over the canonical closed plan; interruptions never rerun accepted retreats.
    pub next_order: usize,
    pub units: BTreeMap<UnitId, Start>,
    pub plans: BTreeMap<SeatId, Vec<Order>>,
    pub retreated: BTreeSet<UnitId>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Start {
    pub hex: HexId,
    pub adjacent: bool,
}

fn allowed(c: &CnaContent, s: &State, id: &UnitId, seat: SeatId) -> bool {
    s.land.combat.retreat.units.contains_key(id)
        && s.land.units.get(id).is_some_and(|u| u.side == seat.side)
        && ownership::seat_for_unit(c, s, id) == seat.role
        && formation::members(c, s, id).iter().all(|member| {
            !s.land.combat.pinned.contains(member)
                && !s.land.combat.retreat.retreated.contains(member)
                && s.land.units[member].cohesion_quarters > -104
        })
}
fn ids(c: &CnaContent, s: &State, seat: SeatId) -> Vec<UnitId> {
    s.land
        .combat
        .retreat
        .units
        .keys()
        .filter(|id| allowed(c, s, id, seat))
        .cloned()
        .collect()
}
/// Snapshot source eligibility and disclosed start control before opening fixed role windows.
/// Cases: land:13.0, land:13.1, land:13.21, land:13.22, land:13.23, land:13.24, land:10.6
/// Interpretations: interp:land-0030
/// Unsupported: land:13.25 - optional dump demolition remains a gap.
pub fn enter(
    c: &CnaContent,
    s: &mut State,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    if strict {
        return Err(EngineError::Unsupported {
            case: "land:13.25".into(),
            detail:
                "RBA dump demolition is not implemented; development supports unit retreat plans."
                    .into(),
        });
    }
    let Some(phasing) = s.cursor.phasing(s.turn.player_a) else {
        return Ok(());
    };
    let side = phasing.opponent();
    // Only the disclosed counter's own printed type decides adjacency. A formation's
    // hidden combat members do not turn its noncombat counter into a combat face.
    // Cases: land:3.62, land:13.23, land:13.24
    let enemy_hexes: BTreeSet<_> = s
        .units_of(phasing)
        .filter(|u| view::is_map_counter(c, s, u) && view::printed_combat_face(c, &u.id))
        .filter_map(|u| u.location.hex().cloned())
        .collect();
    let units = s
        .units_of(side)
        .filter(|u| {
            u.location.hex().is_some()
                && u.cohesion_quarters > -104
                && !s.land.combat.pinned.contains(&u.id)
        })
        .map(|u| {
            let hex = u.location.hex().unwrap().clone();
            let adjacent = c
                .map
                .neighbors(&hex)
                .iter()
                .any(|h| enemy_hexes.contains(&h.id));
            (u.id.clone(), Start { hex, adjacent })
        })
        .collect::<BTreeMap<_, _>>();
    s.land.combat.retreat = RetreatState {
        strict,
        entered: true,
        units,
        ..Default::default()
    };
    s.land.reaction.controls.clear();
    for hex in s
        .land
        .combat
        .retreat
        .units
        .values()
        .map(|start| start.hex.clone())
        .collect::<BTreeSet<_>>()
    {
        s.land
            .reaction
            .controls
            .insert(hex.clone(), zoc::controlled(c, s, phasing, &hex, strict)?);
    }
    for role in [Role::FrontLine, Role::RearArea, Role::Logistics] {
        let seat = SeatId::new(side, role);
        let among = ids(c, s, seat);
        let count = u32::try_from(among.len()).map_err(|_| EngineError::Invariant {
            detail: "too many retreat units".into(),
        })?;
        let space = ActionSpace::new(ActionSchema::List {
            min: 0,
            max: count,
            item: Box::new(ActionSchema::Record {
                fields: vec![
                    FieldSchema {
                        name: "unit".into(),
                        doc: "One eligible own unit, including its represented components.".into(),
                        schema: ActionSchema::Unit { among },
                        optional: false,
                    },
                    FieldSchema {
                        name: "path".into(),
                        doc: "Ordered adjacent hexes entered after the current hex.".into(),
                        schema: ActionSchema::List {
                            min: 1,
                            max: 4096,
                            item: Box::new(ActionSchema::Hex { among: None }),
                        },
                        optional: false,
                    },
                    FieldSchema {
                        name: "with_stack".into(),
                        doc: "Move independent counters commanded by this seat together.".into(),
                        schema: ActionSchema::Bool,
                        optional: true,
                    },
                ],
            }),
        })
        .with_pass("Keep this seat's units in place.");
        open(s,cx,seat,KIND,"Plan voluntary retreats in execution order; inspect own units for paths. Pass explicitly keeps them in place.".into(),
            &["land:13.1","land:13.21","land:13.22","land:13.23","land:13.24","land:13.26"],Trigger::Scheduled,Secrecy::Open,space);
    }
    cx.emit(EngineEvent::new(Audience::Side(side),GameEvent::Note{text:"Development RBA adjudicates unit movement and breakdown; optional dump demolition remains unapplied (land:13.25).".into()}));
    Ok(())
}
fn members(c: &CnaContent, s: &State, seat: SeatId, order: &Order) -> BTreeSet<UnitId> {
    let roots = if order.with_stack {
        let Some(hex) = s.land.units.get(&order.unit).and_then(|u| u.location.hex()) else {
            return BTreeSet::new();
        };
        formation::roots(c, s, hex, seat.side)
            .into_iter()
            .filter(|id| ownership::seat_for_unit(c, s, id) == seat.role)
            .collect()
    } else {
        vec![order.unit.clone()]
    };
    roots
        .iter()
        .flat_map(|id| formation::members(c, s, id))
        .collect()
}
/// Apply each member's own breakoff and movement cost, never the ordered root's cost.
/// Cases: land:13.22, land:13.23, land:13.24
fn capped(
    snapshot: &State,
    before: &State,
    after: &State,
    member_ids: &BTreeSet<UnitId>,
    path_len: usize,
) -> bool {
    member_ids.iter().all(|id| {
        snapshot
            .land
            .combat
            .retreat
            .units
            .get(id)
            .is_some_and(|start| {
                if start.adjacent || path_len == 1 {
                    return true;
                }
                before
                    .land
                    .units
                    .get(id)
                    .zip(after.land.units.get(id))
                    .is_some_and(|(a, b)| {
                        let delta = i64::from(b.cp_spent_quarters) - i64::from(a.cp_spent_quarters);
                        (0..=16).contains(&delta)
                    })
            })
    })
}
fn validate_plans(c: &CnaContent, s: &State) -> Result<(), Rejection> {
    let mut preview = s.clone();
    let mut moved = BTreeSet::new();
    for (seat, orders) in &s.land.combat.retreat.plans {
        if orders.len() > ids(c, s, *seat).len() {
            return Err(illegal("too many retreat orders"));
        }
        for order in orders {
            if !allowed(c, &preview, &order.unit, *seat) || !order.close_assault.is_empty() {
                return Err(illegal(
                    "choose an eligible own retreat unit without assault announcements",
                ));
            }
            let selected = members(c, &preview, *seat, order);
            if selected.is_empty()
                || selected
                    .iter()
                    .any(|id| moved.contains(id) || !s.land.combat.retreat.units.contains_key(id))
            {
                return Err(illegal(
                    "retreat each eligible represented unit at most once",
                ));
            }
            let (next, cost) = movement::preview_nonphasing(
                c,
                &preview,
                order,
                *seat,
                s.land.combat.retreat.strict,
                NonPhasingMove::Retreat,
            )?;
            if !capped(s, &preview, &next, &selected, cost.path.len()) {
                return Err(illegal(
                    "nonadjacent retreat is limited to four CP or one hex",
                ));
            }
            moved.extend(selected);
            preview = next;
        }
    }
    Ok(())
}
/// Record owner-known ordered plans only; a bad final order cannot spend stocks or dice.
/// Cases: land:13.1, land:13.21, land:13.22, land:13.23, land:13.24, land:13.26
pub fn answer(
    c: &CnaContent,
    s: &mut State,
    p: &Pending,
    a: &Value,
    _cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    if !s.land.combat.retreat.entered
        || s.land.combat.retreat.closed
        || s.land.combat.retreat.plans.contains_key(&p.seat)
    {
        return Err(illegal("retreat window is not awaiting this plan"));
    }
    let phasing = s
        .cursor
        .phasing(s.turn.player_a)
        .ok_or_else(|| illegal("not a player half"))?;
    if p.seat.side != phasing.opponent() {
        return Err(illegal("retreat belongs to the nonphasing side"));
    }
    let orders = if a.is_null() {
        vec![]
    } else {
        serde_json::from_value(a.clone())
            .map_err(|_| illegal("expected an ordered retreat list"))?
    };
    s.land.combat.retreat.plans.insert(p.seat, orders);
    validate_plans(c, s)?;
    s.land.combat.retreat.closed = !s.decisions.pending.iter().any(|p| p.kind == KIND);
    Ok("Retreat plan recorded.".into())
}
/// Resolve closed plans under Advance. Previous movement and RBA truth-only stops remain deferred.
/// Cases: land:13.21, land:13.22, land:13.26, land:13.28
pub fn finish(
    c: &CnaContent,
    s: &mut State,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    if strict {
        return Err(EngineError::Unsupported {
            case: "land:13.25".into(),
            detail: "RBA dump demolition is not implemented.".into(),
        });
    }
    crate::land::reaction::finish_adjudication(s)?;
    // Entry opens no window until a public phasing player exists.
    if s.cursor.phasing(s.turn.player_a).is_none() {
        return Ok(());
    }
    if s.land.combat.retreat.resolved {
        return Ok(());
    }
    if !s.land.combat.retreat.closed {
        return Err(EngineError::Invariant {
            detail: "retreat plans are not closed".into(),
        });
    }
    crate::land::breakdown::window::finish(c, s, strict, cx)?;
    if !s.decisions.pending.is_empty() {
        return Ok(());
    }
    let orders: Vec<_> = s
        .land
        .combat
        .retreat
        .plans
        .iter()
        .flat_map(|(seat, orders)| orders.iter().map(move |order| (*seat, order.clone())))
        .collect();
    while let Some((seat, order)) = orders.get(s.land.combat.retreat.next_order).cloned() {
        let selected = members(c, s, seat, &order);
        match movement::execute_nonphasing(
            c,
            s,
            &order,
            seat,
            s.land.combat.retreat.strict,
            NonPhasingMove::Retreat,
            cx,
        ) {
            Ok(cost) => {
                if !cost.path.is_empty() {
                    s.land.combat.retreat.retreated.extend(selected);
                }
            }
            Err(Rejection::Engine(e)) => return Err(e),
            Err(Rejection::Illegal { message }) => {
                if s.land.combat.retreat.strict {
                    return Err(EngineError::Unsupported {
                        case: "land:13.21".into(),
                        detail: message,
                    });
                }
                let event = EngineEvent::new(Audience::Side(seat.side),GameEvent::Note{text:format!("Accepted retreat cannot execute after earlier adjudication: {message}; no replacement move was invented (land:13.21).") }).about(order.unit.clone());
                cx.emit(
                    if let Some(hex) = s.land.units[&order.unit].location.hex() {
                        event.at(hex.clone())
                    } else {
                        event
                    },
                );
            }
            Err(_) => {
                return Err(EngineError::Invariant {
                    detail: "retreat ownership changed during adjudication".into(),
                });
            }
        }
        crate::land::reaction::finish_adjudication(s)?;
        s.land.combat.retreat.next_order += 1;
        if crate::land::breakdown::window::park(s, None) {
            crate::land::breakdown::window::finish(c, s, strict, cx)?;
            if !s.decisions.pending.is_empty() {
                return Ok(());
            }
        }
    }
    s.land.combat.retreat.resolved = true;
    Ok(())
}
/// Own-only legal path catalog, including contact costs and the snapshot's printed-combat-counter adjacency cap.
/// Cases: land:13.21, land:13.22, land:13.23, land:13.24, land:13.26
pub fn reachable(c: &CnaContent, s: &State, id: &UnitId, strict: bool) -> Vec<Reachable> {
    let Some(unit) = s.land.units.get(id) else {
        return vec![];
    };
    let seat = SeatId::new(unit.side, ownership::seat_for_unit(c, s, id));
    if !allowed(c, s, id, seat) {
        return vec![];
    }
    let selected: BTreeSet<UnitId> = formation::members(c, s, id).into_iter().collect();
    movement::nonphasing_reachable(c, s, id, strict, NonPhasingMove::Retreat)
        .into_iter()
        .filter(|cost| {
            // Adjacent members and a one-hex retreat have no four-CP cap. Avoid a second
            // path evaluation in those common cases; all other members need their own delta.
            if selected.iter().all(|member| {
                s.land
                    .combat
                    .retreat
                    .units
                    .get(member)
                    .is_some_and(|start| start.adjacent || cost.path.len() == 1)
            }) {
                return true;
            }
            if s.land
                .combat
                .retreat
                .units
                .get(id)
                .is_some_and(|start| !start.adjacent)
                && cost.cp_quarters > 16
                && cost.path.len() != 1
            {
                return false;
            }
            let order = Order::new(id.clone(), cost.path.clone());
            movement::preview_nonphasing(c, s, &order, seat, strict, NonPhasingMove::Retreat)
                .is_ok_and(|(next, actual)| capped(s, s, &next, &selected, actual.path.len()))
        })
        .collect()
}
fn index(rng: &mut CampaignRng, count: usize) -> usize {
    let mut space = 6usize;
    let mut digits = 1;
    while space < count {
        space *= 6;
        digits += 1;
    }
    loop {
        let mut value = 0;
        for _ in 0..digits {
            value = value * 6 + usize::from(rng.d6().value() - 1)
        }
        if value < space - space % count {
            return value % count;
        }
    }
}
/// Choose one complete own retreat, avoiding list interactions without reading enemy composition.
/// Cases: land:13.1, land:13.21, land:13.23, land:13.24
pub fn random_orders(
    c: &CnaContent,
    s: &State,
    r: &DecisionRequest,
    rng: &mut CampaignRng,
) -> Value {
    if r.kind != KIND {
        return Value::Null;
    }
    let mut among = ids(c, s, r.seat);
    while !among.is_empty() {
        let id = among.remove(index(rng, among.len()));
        let mut paths = reachable(c, s, &id, s.land.combat.retreat.strict);
        while !paths.is_empty() {
            let cost = paths.remove(index(rng, paths.len()));
            let order = Order::new(id.clone(), cost.path);
            let mut candidate = s.clone();
            candidate
                .land
                .combat
                .retreat
                .plans
                .insert(r.seat, vec![order.clone()]);
            if validate_plans(c, &candidate).is_ok() {
                return json!([order]);
            }
        }
    }
    json!([])
}

#[cfg(test)]
#[path = "retreat_tests.rs"]
mod tests;
