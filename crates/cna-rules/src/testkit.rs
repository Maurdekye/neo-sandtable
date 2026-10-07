//! Test helpers shared by every module's tests.
//!
//! **Indistinguishability** (`land:3.6`, docs/engine.md §3 rules 7 and 9): two states that differ
//! only in facts a side must not know must look identical to that side. `assert_indistinguishable`
//! compares everything the side's seats can read: the board view and `observe` of the side and of
//! each of its seats, `inspect` of every unit, truck pool, dump and occupied hex either state
//! knows, and the decisions pending for its seats. Build the pair by cloning a state and changing
//! only a hidden enemy fact (strength, supply, a dummy flag, a plot), then call it for the side
//! that must not learn it. `assert_action_indistinguishable` does the same for an action: the
//! same command evaluated on both states must be accepted or rejected alike (a server preflight
//! is exactly this evaluation, rule 7), deliver the same events to every perspective of the side,
//! and leave states that still look identical; `assert_actions_indistinguishable` compares two
//! different hidden actions (a pass against counter traffic the enemy must not notice).

use std::collections::BTreeSet;

use cna_core::engine::{Command, Game, Rejection, Ruleset, Transition, evaluate};
use cna_core::event::EngineEvent;
use cna_core::ids::SeatId;
use cna_core::visibility::Perspective;
use cna_protocol::GameEvent;
use cna_protocol::{Role, Side};
use serde_json::{Map, Value, json};

use crate::{Cna, CnaContent, State};

/// Everything `side` can see in `state`. `targets` are the ids to inspect.
pub(crate) fn visible_to(
    ruleset: &Cna,
    content: &CnaContent,
    state: &State,
    side: Side,
    targets: &BTreeSet<String>,
) -> Value {
    let mut out = Map::new();
    for (name, perspective) in perspectives(side) {
        let view = serde_json::to_value(ruleset.view(content, state, perspective))
            .expect("views serialize");
        let observe = ruleset.observe(content, state, perspective);
        let inspected: Map<String, Value> = targets
            .iter()
            .map(|t| {
                let result = match ruleset.inspect(content, state, perspective, t) {
                    Ok(v) => v,
                    Err(e) => json!({ "rejected": format!("{e:?}") }),
                };
                (t.clone(), result)
            })
            .collect();
        out.insert(
            name,
            json!({ "view": view, "observe": observe, "inspect": inspected }),
        );
    }
    let pending: Vec<Value> = ruleset
        .pending(content, state)
        .into_iter()
        .filter(|d| d.seat.side == side)
        .map(|d| serde_json::to_value(d).expect("decisions serialize"))
        .collect();
    out.insert("pending".to_owned(), Value::Array(pending));
    Value::Object(out)
}

/// The side's own perspective and each of its five seats, with stable names.
fn perspectives(side: Side) -> Vec<(String, Perspective)> {
    let mut out = vec![("side".to_owned(), Perspective::Side(side))];
    for role in Role::ALL {
        let seat = SeatId::new(side, role);
        out.push((seat.to_string(), Perspective::Seat(seat)));
    }
    out
}

/// What `side` learns from one evaluated command: acceptance or the rejection, every event each
/// of its perspectives receives (in order), and the progress report.
fn learned_from(result: &Result<Transition<Cna>, Rejection>, side: Side) -> Value {
    match result {
        Err(e) => json!({ "rejected": format!("{e:?}") }),
        Ok(t) => {
            let events: Map<String, Value> = perspectives(side)
                .into_iter()
                .map(|(name, p)| {
                    let seen: Vec<Value> = t
                        .events
                        .iter()
                        .filter(|e| p.can_see(&e.audience))
                        .map(|e| serde_json::to_value(&e.event).expect("events serialize"))
                        .collect();
                    (name, Value::Array(seen))
                })
                .collect();
            json!({
                "accepted": true,
                "events": events,
                "progress": t.progress.as_ref().map(|p| format!("{p:?}")),
            })
        }
    }
}

/// Assert that `command` evaluated on `a` and on `b` teaches `side` nothing: the same
/// acceptance or rejection, the same events per perspective, and resulting states that are
/// still indistinguishable. Use it for paired hidden facts at decision points (rule 7).
pub(crate) fn assert_action_indistinguishable(
    ruleset: &Cna,
    content: &CnaContent,
    a: &Game<Cna>,
    b: &Game<Cna>,
    command: &Command,
    side: Side,
) {
    assert_actions_indistinguishable(ruleset, content, (a, command), (b, command), side);
}

/// Assert that two different hidden actions teach `side` nothing: e.g. the enemy passing versus
/// moving counters between hexes it already occupies. Same comparison as
/// `assert_action_indistinguishable`, with each game evaluated under its own command.
pub(crate) fn assert_actions_indistinguishable(
    ruleset: &Cna,
    content: &CnaContent,
    (a, command_a): (&Game<Cna>, &Command),
    (b, command_b): (&Game<Cna>, &Command),
    side: Side,
) {
    let ra = evaluate(ruleset, content, a, command_a);
    let rb = evaluate(ruleset, content, b, command_b);
    if let Some((path, x, y)) = first_difference(
        &learned_from(&ra, side),
        &learned_from(&rb, side),
        String::new(),
    ) {
        panic!(
            "{side:?} can tell the actions apart at {path}:
  a: {x}
  b: {y}"
        );
    }
    if let (Ok(ta), Ok(tb)) = (&ra, &rb) {
        assert_indistinguishable(ruleset, content, &ta.game.state, &tb.game.state, side);
    }
}

/// Every id either state could be asked to inspect: units, truck pools, dumps and occupied hexes.
pub(crate) fn inspect_targets(states: &[&State]) -> BTreeSet<String> {
    let mut targets = BTreeSet::new();
    for s in states {
        for (id, unit) in &s.land.units {
            targets.insert(id.to_string());
            if let Some(hex) = unit.location.hex() {
                targets.insert(hex.to_string());
            }
        }
        for pool in &s.logistics.truck_pools {
            targets.insert(pool.id.clone());
        }
        for id in s.logistics.dumps.keys() {
            targets.insert(id.clone());
        }
    }
    targets
}

/// Assert that `a` and `b` look identical to `side`, naming the first difference.
pub(crate) fn assert_indistinguishable(
    ruleset: &Cna,
    content: &CnaContent,
    a: &State,
    b: &State,
    side: Side,
) {
    let targets = inspect_targets(&[a, b]);
    let va = visible_to(ruleset, content, a, side, &targets);
    let vb = visible_to(ruleset, content, b, side, &targets);
    if let Some((path, x, y)) = first_difference(&va, &vb, String::new()) {
        panic!("{side:?} can tell the states apart at {path}:\n  a: {x}\n  b: {y}");
    }
}

/// The first path where two JSON values differ, with both values there.
fn first_difference(a: &Value, b: &Value, path: String) -> Option<(String, Value, Value)> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            keys.into_iter().find_map(|k| {
                let (xa, yb) = (
                    x.get(k).unwrap_or(&Value::Null),
                    y.get(k).unwrap_or(&Value::Null),
                );
                first_difference(xa, yb, format!("{path}/{k}"))
            })
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .find_map(|(i, (xa, yb))| first_difference(xa, yb, format!("{path}[{i}]"))),
        _ if a == b => None,
        _ => Some((path, a.clone(), b.clone())),
    }
}

/// Viewers update from events, not snapshots. Assert that every change between `before` and
/// `after` in `perspective`'s board view is announced by an event that perspective receives:
/// a unit whose view changed or appeared needs a `UnitUpdated` (one that left needs
/// `UnitRemoved`), a stack that appeared or changed needs a matching `StackUpdated` (one that
/// vanished needs `StackRemoved`), markers need `MarkerPlaced`/`MarkerRemoved`, and new or
/// resolved pending decisions need `DecisionOpened`/`DecisionResolved`. Returns the first gap.
pub(crate) fn events_explain_view_changes(
    ruleset: &Cna,
    content: &CnaContent,
    before: &State,
    after: &State,
    events: &[EngineEvent],
    perspective: Perspective,
) -> Result<(), String> {
    let (v0, v1) = (
        ruleset.view(content, before, perspective),
        ruleset.view(content, after, perspective),
    );
    let seen: Vec<&GameEvent> = events
        .iter()
        .filter(|e| perspective.can_see(&e.audience))
        .map(|e| &e.event)
        .collect();
    for (id, unit) in &v1.units {
        if v0.units.get(id) != Some(unit)
            && !seen
                .iter()
                .any(|e| matches!(e, GameEvent::UnitUpdated { unit: u } if &u.id == id))
        {
            let before_unit = serde_json::to_value(v0.units.get(id)).unwrap_or(Value::Null);
            let after_unit = serde_json::to_value(unit).unwrap_or(Value::Null);
            let what = first_difference(&before_unit, &after_unit, String::new())
                .map(|(path, x, y)| format!("{path}: {x} -> {y}"))
                .unwrap_or_default();
            return Err(format!("unit {id} changed without a UnitUpdated ({what})"));
        }
    }
    for id in v0.units.keys() {
        if !v1.units.contains_key(id)
            && !seen
                .iter()
                .any(|e| matches!(e, GameEvent::UnitRemoved { unit_id, .. } if unit_id == id))
        {
            return Err(format!("unit {id} left the view without a UnitRemoved"));
        }
    }
    let stacks = |v: &cna_protocol::ViewState| {
        v.stacks
            .iter()
            .map(|s| ((s.hex.clone(), s.side), s.clone()))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let (s0, s1) = (stacks(&v0), stacks(&v1));
    for (key, stack) in &s1 {
        if s0.get(key) != Some(stack)
            && !seen
                .iter()
                .any(|e| matches!(e, GameEvent::StackUpdated { stack: st } if st == stack))
        {
            return Err(format!(
                "stack {key:?} changed without a matching StackUpdated"
            ));
        }
    }
    for key in s0.keys() {
        if !s1.contains_key(key)
            && !seen.iter().any(|e| matches!(e, GameEvent::StackRemoved { hex, side } if (hex, side) == (&key.0, &key.1)))
        {
            return Err(format!("stack {key:?} vanished without a StackRemoved"));
        }
    }
    for marker in &v1.markers {
        if !v0.markers.contains(marker)
            && !seen
                .iter()
                .any(|e| matches!(e, GameEvent::MarkerPlaced { marker: m } if m == marker))
        {
            let previous = v0.markers.iter().find(|m| m.id == marker.id);
            return Err(format!(
                "marker {} appeared or changed without a MarkerPlaced ({previous:?} -> {marker:?})",
                marker.id
            ));
        }
    }
    for marker in &v0.markers {
        if !v1.markers.iter().any(|m| m.id == marker.id)
            && !seen.iter().any(
                |e| matches!(e, GameEvent::MarkerRemoved { marker_id } if marker_id == &marker.id),
            )
        {
            return Err(format!(
                "marker {} vanished without a MarkerRemoved",
                marker.id
            ));
        }
    }
    for d in &v1.pending {
        if !v0.pending.iter().any(|p| p.id == d.id)
            && !seen
                .iter()
                .any(|e| matches!(e, GameEvent::DecisionOpened { decision } if decision.id == d.id))
        {
            return Err(format!("decision {} opened without a DecisionOpened", d.id));
        }
    }
    for d in &v0.pending {
        if !v1.pending.iter().any(|p| p.id == d.id)
            && !seen.iter().any(|e| matches!(e, GameEvent::DecisionResolved { decision_id, .. } if decision_id == &d.id))
        {
            return Err(format!("decision {} closed without a DecisionResolved", d.id));
        }
    }
    Ok(())
}
