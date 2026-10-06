//! Scripted baseline controllers for the sandbox.
//!
//! These answer decisions from the request alone plus what the deciding seat may observe, so
//! they never use hidden information. `aggressive` closes with the enemy and attacks; it gives
//! integration tests and demo campaigns real combat to show.

use std::collections::BTreeSet;

use cna_core::decision::{ActionSchema, DecisionRequest};
use cna_core::ids::{HexId, Side};
use serde_json::{Map, Value, json};

use crate::{SandboxContent, State};

/// An objective-directed baseline: seize the initiative, supply the strongest units, move every
/// unit as close as possible to the nearest enemy stack, hold when threatened, put all air on the
/// first contact hex, assault everything in reach, repair the first damaged unit.
///
/// It reads only enemy stack *positions*, which every seat may see; never stack contents.
pub fn aggressive(content: &SandboxContent, state: &State, req: &DecisionRequest) -> Value {
    let side = req.seat.side;
    let enemy_stacks: BTreeSet<HexId> = state
        .units
        .values()
        .filter(|u| u.side == side.opponent())
        .map(|u| u.hex.clone())
        .collect();
    let distance_to_enemy = |hex: &HexId| -> u32 {
        enemy_stacks
            .iter()
            .map(|e| content.distance(hex, e))
            .min()
            .unwrap_or(u32::MAX)
    };
    match req.kind.as_str() {
        "sandbox.initiative" => json!("first"),
        "sandbox.supply" => match &req.space.schema {
            ActionSchema::List { item, max, .. } => match item.as_ref() {
                ActionSchema::Unit { among } => {
                    let mut units: Vec<_> =
                        among.iter().filter_map(|id| state.units.get(id)).collect();
                    units.sort_by_key(|u| (std::cmp::Reverse(u.strength), u.id.clone()));
                    json!(
                        units
                            .iter()
                            .take(*max as usize)
                            .map(|u| u.id.as_str())
                            .collect::<Vec<_>>()
                    )
                }
                _ => json!([]),
            },
            _ => json!([]),
        },
        "sandbox.movement" | "sandbox.assault" => {
            let ActionSchema::Record { fields } = &req.space.schema else {
                return Value::Null;
            };
            let mut out = Map::new();
            for f in fields {
                if let ActionSchema::Hex { among: Some(hexes) } = &f.schema
                    && let Some(best) = hexes
                        .iter()
                        .min_by_key(|h| (distance_to_enemy(h), (*h).clone()))
                {
                    out.insert(f.name.clone(), json!(best.as_str()));
                }
            }
            Value::Object(out)
        }
        "sandbox.reaction" => json!("hold"),
        "sandbox.air" => match &req.space.schema {
            ActionSchema::Record { fields } => match fields.first() {
                Some(f) => json!({ f.name.clone(): 3 }),
                None => Value::Null,
            },
            _ => Value::Null,
        },
        "sandbox.repair" => match &req.space.schema {
            ActionSchema::Choice { options } => {
                options.first().map(|o| json!(o.id)).unwrap_or(Value::Null)
            }
            _ => Value::Null,
        },
        _ => Value::Null,
    }
}

/// Whether `side` has any units left (convenience for demo runners).
pub fn has_units(state: &State, side: Side) -> bool {
    state.units.values().any(|u| u.side == side)
}
