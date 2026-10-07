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

    /// Whether `action` has a legal shape for this space: `null` exactly when passing is
    /// allowed, otherwise a value matching the schema ([`ActionSchema::check`]).
    pub fn check(&self, action: &Value) -> Result<(), String> {
        match (action, &self.pass) {
            (Value::Null, Some(_)) => Ok(()),
            (Value::Null, None) => Err("action: passing is not allowed for this decision".into()),
            _ => self.schema.check(action),
        }
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
    /// Text of `min_length..=max_length` characters, for ids an answer itself creates (e.g. the
    /// cohorts a division splits off); the ruleset validates what it names.
    Text { min_length: u32, max_length: u32 },
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
    /// Whether `value` has the shape this schema advertises: its JSON type, enum membership,
    /// integer range, text and list lengths, path length, and record fields (no unknown field,
    /// every required one present, `null` allowed for an optional one). Hex syntax, adjacency and
    /// meaning stay the ruleset's to judge. The error names the first offending path and speaks
    /// only of the schema, which the answering seat already has, so it can never disclose
    /// anything else (docs/engine.md §3 rule 7).
    pub fn check(&self, value: &Value) -> Result<(), String> {
        self.check_at(value, "action")
    }

    fn check_at(&self, value: &Value, path: &str) -> Result<(), String> {
        let fail = |what: String| Err(format!("{path}: {what}"));
        match self {
            ActionSchema::Choice { options } => match value.as_str() {
                Some(s) if options.iter().any(|o| o.id == s) => Ok(()),
                _ => fail("expected one of the listed option ids".into()),
            },
            ActionSchema::Integer { min, max } => match value.as_i64() {
                Some(n) if (*min..=*max).contains(&n) => Ok(()),
                _ => fail(format!("expected an integer in {min}..={max}")),
            },
            ActionSchema::Bool => match value {
                Value::Bool(_) => Ok(()),
                _ => fail("expected true or false".into()),
            },
            ActionSchema::Unit { among } => match value.as_str() {
                Some(s) if among.iter().any(|u| u.as_str() == s) => Ok(()),
                _ => fail("expected one of the listed unit ids".into()),
            },
            ActionSchema::Hex { among } => match (value.as_str(), among) {
                (Some(s), Some(hexes)) if hexes.iter().any(|h| h.as_str() == s) => Ok(()),
                (Some(_), None) => Ok(()),
                (_, Some(_)) => fail("expected one of the listed hex ids".into()),
                (None, None) => fail("expected a hex id".into()),
            },
            ActionSchema::Path { max_steps, .. } => match value.as_array() {
                Some(steps)
                    if steps.len() <= *max_steps as usize && steps.iter().all(Value::is_string) =>
                {
                    Ok(())
                }
                _ => fail(format!("expected a list of at most {max_steps} hex ids")),
            },
            ActionSchema::Text {
                min_length,
                max_length,
            } => match value.as_str() {
                Some(s)
                    if (*min_length as usize..=*max_length as usize)
                        .contains(&s.chars().count()) =>
                {
                    Ok(())
                }
                _ => fail(format!(
                    "expected text of {min_length}..={max_length} characters"
                )),
            },
            ActionSchema::Record { fields } => {
                let Some(map) = value.as_object() else {
                    return fail("expected an object".into());
                };
                if let Some(extra) = map.keys().find(|k| fields.iter().all(|f| &f.name != *k)) {
                    return fail(format!("unknown field `{extra}`"));
                }
                for f in fields {
                    match map.get(&f.name) {
                        None | Some(Value::Null) if f.optional => {}
                        None => return fail(format!("missing field `{}`", f.name)),
                        Some(v) => f.schema.check_at(v, &format!("{path}.{}", f.name))?,
                    }
                }
                Ok(())
            }
            ActionSchema::List { item, min, max } => {
                let Some(items) = value.as_array() else {
                    return fail("expected a list".into());
                };
                if !(*min as usize..=*max as usize).contains(&items.len()) {
                    return fail(format!("expected {min}..={max} items"));
                }
                items
                    .iter()
                    .enumerate()
                    .try_for_each(|(i, v)| item.check_at(v, &format!("{path}[{i}]")))
            }
        }
    }

    /// The `x-` annotations repeat what the description says in a form programs can use:
    /// `x-options` (each choice's id, label and detail), `x-kind` (`unit`, `hex` or `path`, for
    /// fields picked on the map) and `x-from` (where a path starts). JSON Schema validators
    /// ignore them.
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
                "x-options": options
                    .iter()
                    .map(|o| match &o.detail {
                        Some(d) => json!({ "id": o.id, "label": o.label, "detail": d }),
                        None => json!({ "id": o.id, "label": o.label }),
                    })
                    .collect::<Vec<_>>(),
            }),
            ActionSchema::Integer { min, max } => {
                json!({ "type": "integer", "minimum": min, "maximum": max })
            }
            ActionSchema::Bool => json!({ "type": "boolean" }),
            ActionSchema::Unit { among } => json!({
                "type": "string",
                "enum": among.iter().map(|u| u.as_str().to_owned()).collect::<Vec<_>>(),
                "description": "unit id",
                "x-kind": "unit",
            }),
            ActionSchema::Hex { among } => match among {
                Some(hexes) => json!({
                    "type": "string",
                    "enum": hexes.iter().map(|h| h.as_str().to_owned()).collect::<Vec<_>>(),
                    "description": "hex id",
                    "x-kind": "hex",
                }),
                None => json!({
                    "type": "string",
                    "pattern": "^[A-E][0-9]{4}$",
                    "description": "printed hex id, e.g. C4218",
                    "x-kind": "hex",
                }),
            },
            ActionSchema::Path { from, max_steps } => json!({
                "type": "array",
                "items": { "type": "string", "pattern": "^[A-E][0-9]{4}$" },
                "maxItems": max_steps,
                "description": format!(
                    "hexes entered in order, each adjacent to the previous, starting next to {from}"
                ),
                "x-kind": "path",
                "x-from": from.as_str(),
            }),
            ActionSchema::Text {
                min_length,
                max_length,
            } => json!({ "type": "string", "minLength": min_length, "maxLength": max_length }),
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
                    if f.optional {
                        // `check` accepts an optional field given as null (meaning absent), so
                        // the exported schema must allow it too.
                        let description = s.as_object_mut().and_then(|o| o.remove("description"));
                        s = json!({ "anyOf": [s, { "type": "null" }] });
                        if let (Some(d), Some(o)) = (description, s.as_object_mut()) {
                            o.insert("description".into(), d);
                        }
                    } else {
                        required.push(Value::String(f.name.clone()));
                    }
                    props.insert(f.name.clone(), s);
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
    fn answers_must_fit_the_advertised_space() {
        use super::*;
        let field = |name: &str, schema: ActionSchema, optional: bool| FieldSchema {
            name: name.into(),
            doc: String::new(),
            schema,
            optional,
        };
        let unit = ActionSchema::Unit {
            among: vec!["it.a".into()],
        };
        let text = ActionSchema::Text {
            min_length: 1,
            max_length: 4,
        };
        let space = ActionSpace::new(ActionSchema::List {
            item: Box::new(ActionSchema::Record {
                fields: vec![
                    field("unit", unit, false),
                    field("n", ActionSchema::Integer { min: 0, max: 3 }, false),
                    field("label", text, true),
                ],
            }),
            min: 0,
            max: 1,
        });
        assert!(space.check(&json!([{"unit": "it.a", "n": 3}])).is_ok());
        assert!(
            space
                .check(&json!([{"unit": "it.a", "n": 0, "label": null}]))
                .is_ok()
        );
        assert!(
            space
                .check(&json!([{"unit": "it.a", "n": 0, "label": "c1"}]))
                .is_ok()
        );
        let two = json!([{"unit": "it.a", "n": 0}, {"unit": "it.a", "n": 0}]);
        for (bad, says) in [
            (json!(null), "passing is not allowed"),
            (json!({}), "action: expected a list"),
            (two, "0..=1 items"),
            (
                json!([{"unit": "it.b", "n": 0}]),
                "action[0].unit: expected one of the listed unit",
            ),
            (
                json!([{"unit": "it.a", "n": 4}]),
                "action[0].n: expected an integer in 0..=3",
            ),
            (json!([{"unit": "it.a"}]), "missing field `n`"),
            (
                json!([{"unit": "it.a", "n": 0, "x": 1}]),
                "unknown field `x`",
            ),
            (
                json!([{"unit": "it.a", "n": 0, "label": "toolong"}]),
                "1..=4 characters",
            ),
        ] {
            let err = space.check(&bad).unwrap_err();
            assert!(err.contains(says), "{bad} -> {err}");
        }
        assert!(space.clone().with_pass("skip").check(&json!(null)).is_ok());
        // The exported schema agrees: optional fields admit null, required ones do not.
        let exported = space.to_json_schema();
        let props = &exported["items"]["properties"];
        assert_eq!(props["label"]["anyOf"][1], json!({ "type": "null" }));
        assert_eq!(props["label"]["anyOf"][0]["type"], json!("string"));
        assert!(props["label"]["description"].is_string());
        assert!(props["unit"].get("anyOf").is_none());
        assert_eq!(exported["items"]["required"], json!(["unit", "n"]));
        let path = ActionSchema::Path {
            from: "C4218".into(),
            max_steps: 2,
        };
        assert!(path.check(&json!(["C4219", "C4220"])).is_ok());
        assert!(path.check(&json!(["C4219", "C4220", "C4221"])).is_err());
        assert!(
            ActionSchema::Hex { among: None }
                .check(&json!("C4219"))
                .is_ok()
        );
        assert!(ActionSchema::Bool.check(&json!(1)).is_err());
    }

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
        // Map-picked fields say what they pick, and a path says where it starts.
        assert_eq!(obj["properties"]["unit"]["x-kind"], "unit");
        assert_eq!(obj["properties"]["path"]["x-kind"], "path");
        assert_eq!(obj["properties"]["path"]["x-from"], "C4218");
        assert_eq!(
            ActionSchema::Hex { among: None }.to_json_schema()["x-kind"],
            "hex"
        );
    }

    #[test]
    fn choices_export_their_labels_as_structured_options() {
        let schema = ActionSchema::Choice {
            options: vec![
                ChoiceOption {
                    id: "a".into(),
                    label: "Alpha".into(),
                    detail: Some("first".into()),
                },
                ChoiceOption {
                    id: "b".into(),
                    label: "Beta".into(),
                    detail: None,
                },
            ],
        }
        .to_json_schema();
        assert_eq!(
            schema["x-options"],
            json!([
                { "id": "a", "label": "Alpha", "detail": "first" },
                { "id": "b", "label": "Beta" },
            ])
        );
        assert_eq!(schema["description"], "a: Alpha (first); b: Beta");
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
