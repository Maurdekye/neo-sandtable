//! The controller contract: what a seat is asked to decide, and how it answers.
//!
//! Every controller kind (scripted baseline, LLM CLI over MCP, System 1 model, human) receives
//! the same [`DecisionRequest`] and answers with the same [`DecisionResponse`]. The legal action
//! space is described as data ([`ActionSchema`]) so that each adapter can present it its own way
//! (JSON Schema for LLM tools, bounded questions for System 1, forms for humans) while the ruleset
//! alone decides legality.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::clock::Clock;
use crate::ids::{DecisionId, HexId, SeatId, UnitId};

/// How a decision arises (design doc §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// A legal choice at a specified phase of the sequence of play.
    Scheduled,
    /// A choice caused by an event (movement, combat, discovery); it suspends the operation that
    /// caused it until resolved.
    Triggered,
    /// A persistent policy the seat may revise.
    Standing,
}

/// Who learns the answer, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Secrecy {
    /// The opponent may see the answer as soon as it is accepted.
    Open,
    /// Kept from the opponent unless a rule reveals it.
    Secret,
    /// Part of a simultaneous secret window: held privately until every required answer is in.
    SecretSimultaneous,
}

/// A pending decision, as presented to the seat that owns it. It is already filtered for that
/// seat: nothing in it may depend on information the seat may not have.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    pub id: DecisionId,
    pub seat: SeatId,
    /// A stable kind name, e.g. `cna.barrage.plot` or `sandbox.move_orders`.
    pub kind: String,
    /// Bumped when the window's prerequisites change; answers to an older revision are rejected.
    pub revision: u32,
    pub clock: Clock,
    /// One or two sentences saying what is being decided.
    pub summary: String,
    /// Rule cases governing the decision (e.g. `land:12.2`).
    pub rules: Vec<String>,
    pub trigger: Trigger,
    pub secrecy: Secrecy,
    pub space: ActionSpace,
}

/// A seat's answer to a pending decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionResponse {
    pub decision_id: DecisionId,
    pub seat: SeatId,
    /// The seat's controller epoch; answers from a replaced controller are rejected.
    pub controller_epoch: u64,
    /// Must equal the request's `revision`.
    pub decision_revision: u32,
    /// Unique per intended action; a retried submission with the same key is not applied twice.
    pub idempotency_key: String,
    /// The chosen action, shaped by the request's [`ActionSpace`]; the ruleset parses and
    /// validates it. `Value::Null` means "pass" where the space allows passing.
    pub action: Value,
    /// Optional player-visible commentary. Never executable.
    pub public_explanation: Option<String>,
}

/// The legal action space of one decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionSpace {
    pub schema: ActionSchema,
    /// If passing is legal, what passing means (e.g. "do not barrage this step").
    pub pass: Option<String>,
    /// What the decision is about, as structured ids for viewers and seats (e.g.
    /// `{"unit": "it.x", "group": "g3"}` for a set-up placement), so nobody has to parse the
    /// summary. Never needed to answer; reaches exactly who sees the decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
}

impl ActionSpace {
    pub fn new(schema: ActionSchema) -> Self {
        Self {
            schema,
            pass: None,
            context: None,
        }
    }

    pub fn with_pass(mut self, meaning: impl Into<String>) -> Self {
        self.pass = Some(meaning.into());
        self
    }

    /// Attach structured context (see [`ActionSpace::context`]).
    pub fn with_context(mut self, context: Value) -> Self {
        self.context = Some(context);
        self
    }

    /// A JSON Schema (draft 2020-12 subset) for the `action` field, for LLM tools. Context, if
    /// any, rides along as the `x-context` annotation.
    pub fn to_json_schema(&self) -> Value {
        let schema = self.schema.to_json_schema();
        let mut out = match &self.pass {
            None => schema,
            Some(meaning) => json!({
                "anyOf": [schema, { "type": "null", "description": format!("Pass: {meaning}") }]
            }),
        };
        if let (Some(context), Some(map)) = (&self.context, out.as_object_mut()) {
            map.insert("x-context".to_owned(), context.clone());
        }
        out
    }
}

/// A parameterized description of legal answers. Domains that are small are enumerated; large
/// ones (paths, arbitrary hexes) are described and validated by the ruleset, so a candidate menu
/// never silently becomes the whole action space (design doc §4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionSchema {
    /// Exactly one of the listed options, answered by its `id`.
    Choice { options: Vec<ChoiceOption> },
    /// An integer in `min..=max`.
    Integer { min: i64, max: i64 },
    /// `true` or `false`.
    Bool,
    /// One unit id from `among`.
    Unit { among: Vec<UnitId> },
    /// One hex id; `among` lists the legal hexes when enumerable, otherwise any id the map defines
    /// (validated by the ruleset).
    Hex { among: Option<Vec<HexId>> },
    /// A sequence of adjacent hexes beginning next to `from`, at most `max_steps` long.
    Path { from: HexId, max_steps: u32 },
    /// An object with named fields.
    Record { fields: Vec<FieldSchema> },
    /// A list of items, `min..=max` long.
    List {
        item: Box<ActionSchema>,
        min: u32,
        max: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSchema {
    pub name: String,
    pub doc: String,
    pub schema: ActionSchema,
    pub optional: bool,
}

impl ActionSchema {
    pub fn to_json_schema(&self) -> Value {
        match self {
            ActionSchema::Choice { options } => json!({
                "type": "string",
                "enum": options.iter().map(|o| o.id.clone()).collect::<Vec<_>>(),
                "description": options
                    .iter()
                    .map(|o| match &o.detail {
                        Some(d) => format!("{}: {} ({})", o.id, o.label, d),
                        None => format!("{}: {}", o.id, o.label),
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            }),
            ActionSchema::Integer { min, max } => {
                json!({ "type": "integer", "minimum": min, "maximum": max })
            }
            ActionSchema::Bool => json!({ "type": "boolean" }),
            ActionSchema::Unit { among } => json!({
                "type": "string",
                "enum": among.iter().map(|u| u.as_str().to_owned()).collect::<Vec<_>>(),
                "description": "unit id",
            }),
            ActionSchema::Hex { among } => match among {
                Some(hexes) => json!({
                    "type": "string",
                    "enum": hexes.iter().map(|h| h.as_str().to_owned()).collect::<Vec<_>>(),
                    "description": "hex id",
                }),
                None => json!({
                    "type": "string",
                    "pattern": "^[A-E][0-9]{4}$",
                    "description": "printed hex id, e.g. C4218",
                }),
            },
            ActionSchema::Path { from, max_steps } => json!({
                "type": "array",
                "items": { "type": "string", "pattern": "^[A-E][0-9]{4}$" },
                "maxItems": max_steps,
                "description": format!(
                    "hexes entered in order, each adjacent to the previous, starting next to {from}"
                ),
            }),
            ActionSchema::Record { fields } => {
                let mut props = Map::new();
                let mut required = Vec::new();
                for f in fields {
                    let mut s = f.schema.to_json_schema();
                    if let Value::Object(o) = &mut s {
                        let doc = match o.get("description").and_then(Value::as_str) {
                            Some(existing) => format!("{} — {}", f.doc, existing),
                            None => f.doc.clone(),
                        };
                        o.insert("description".into(), Value::String(doc));
                    }
                    props.insert(f.name.clone(), s);
                    if !f.optional {
                        required.push(Value::String(f.name.clone()));
                    }
                }
                json!({
                    "type": "object",
                    "properties": props,
                    "required": required,
                    "additionalProperties": false,
                })
            }
            ActionSchema::List { item, min, max } => json!({
                "type": "array",
                "items": item.to_json_schema(),
                "minItems": min,
                "maxItems": max,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn context_rides_along_as_an_annotation() {
        let space = ActionSpace::new(ActionSchema::Bool)
            .with_pass("skip")
            .with_context(json!({"unit": "it.a"}));
        let schema = space.to_json_schema();
        assert_eq!(schema["x-context"], json!({"unit": "it.a"}));
        assert!(schema["anyOf"].is_array());
        let plain = ActionSpace::new(ActionSchema::Bool).to_json_schema();
        assert!(plain.get("x-context").is_none());
        // Old serialized spaces without the field still load.
        let old: ActionSpace =
            serde_json::from_value(json!({"schema": {"type": "bool"}, "pass": null})).unwrap();
        assert_eq!(old.context, None);
    }

    use super::*;

    #[test]
    fn json_schema_for_a_record_with_a_pass() {
        let space = ActionSpace::new(ActionSchema::Record {
            fields: vec![
                FieldSchema {
                    name: "unit".into(),
                    doc: "the moving unit".into(),
                    schema: ActionSchema::Unit {
                        among: vec!["it.a".into(), "it.b".into()],
                    },
                    optional: false,
                },
                FieldSchema {
                    name: "path".into(),
                    doc: "where it goes".into(),
                    schema: ActionSchema::Path {
                        from: "C4218".into(),
                        max_steps: 10,
                    },
                    optional: false,
                },
            ],
        })
        .with_pass("no further movement");

        let schema = space.to_json_schema();
        let obj = &schema["anyOf"][0];
        assert_eq!(obj["type"], "object");
        assert_eq!(obj["required"], json!(["unit", "path"]));
        assert_eq!(obj["properties"]["unit"]["enum"], json!(["it.a", "it.b"]));
        assert_eq!(schema["anyOf"][1]["type"], "null");
    }

    #[test]
    fn schemas_round_trip_through_json() {
        let schema = ActionSchema::List {
            item: Box::new(ActionSchema::Choice {
                options: vec![ChoiceOption {
                    id: "a".into(),
                    label: "Alpha".into(),
                    detail: None,
                }],
            }),
            min: 0,
            max: 3,
        };
        let text = serde_json::to_string(&schema).unwrap();
        assert_eq!(serde_json::from_str::<ActionSchema>(&text).unwrap(), schema);
    }
}
