//! Test helpers shared by every module's tests.
//!
//! **Indistinguishability** (`land:3.6`, docs/engine.md §3 rules 7 and 9): two states that differ
//! only in facts a side must not know must look identical to that side. `assert_indistinguishable`
//! compares everything the side's seats can read: the board view and `observe` of the side and of
//! each of its seats, `inspect` of every unit, truck pool, dump and occupied hex either state
//! knows, and the decisions pending for its seats. Build the pair by cloning a state and changing
//! only a hidden enemy fact (strength, supply, a dummy flag, a plot), then call it for the side
//! that must not learn it.

use std::collections::BTreeSet;

use cna_core::engine::Ruleset;
use cna_core::ids::SeatId;
use cna_core::visibility::Perspective;
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
    let mut perspectives = vec![("side".to_owned(), Perspective::Side(side))];
    for role in Role::ALL {
        let seat = SeatId::new(side, role);
        perspectives.push((seat.to_string(), Perspective::Seat(seat)));
    }
    for (name, perspective) in perspectives {
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
