//! Scripted actions use controller-local randomness, never campaign adjudication dice.
use cna_core::{
    decision::{ActionSchema, DecisionRequest, DecisionResponse},
    engine::Ruleset,
    ids::SeatId,
};
use rand_chacha::ChaCha8Rng;
use rand_core::{RngCore, SeedableRng};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{Campaign, CampaignStatus, Error};

/// The ruleset/content adapter supplies candidates for unenumerated hexes and paths.
/// Candidate generation may only use the owning seat's authorized knowledge.
pub trait Candidates {
    /// Whether this adapter supplies unenumerated hex/path domains. Existing hooks
    /// retain their behavior; a missing hook declares the limitation before sampling.
    fn has_unenumerated_domains(&self) -> bool {
        true
    }
    fn candidate(
        &self,
        request: &DecisionRequest,
        schema: &ActionSchema,
        attempt: u32,
    ) -> Option<Value>;
}
pub struct NoCandidates;
impl Candidates for NoCandidates {
    fn has_unenumerated_domains(&self) -> bool {
        false
    }
    fn candidate(&self, _: &DecisionRequest, _: &ActionSchema, _: u32) -> Option<Value> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    LegalRandom,
    PassWhenPossible,
}

fn sample(rng: &mut ChaCha8Rng, count: u128) -> Result<u64, Error> {
    if count == 0 || count > (1u128 << 64) {
        return Err(Error::Invalid("empty or invalid action domain".into()));
    }
    if count == (1u128 << 64) {
        return Ok(rng.next_u64());
    }
    let n = count as u64;
    let threshold = n.wrapping_neg() % n;
    loop {
        let value = rng.next_u64();
        if value >= threshold {
            return Ok(value % n);
        }
    }
}

struct Generator<'a> {
    request: &'a DecisionRequest,
    rng: ChaCha8Rng,
    nodes_left: u32,
    candidates: &'a dyn Candidates,
    attempt: u32,
}
impl Generator<'_> {
    fn action(&mut self, schema: &ActionSchema, depth: u32) -> Result<Value, Error> {
        if depth > 32 || self.nodes_left == 0 {
            return Err(Error::Invalid("action generation budget exhausted".into()));
        }
        self.nodes_left -= 1;
        Ok(match schema {
            ActionSchema::Choice { options } => options
                [sample(&mut self.rng, options.len() as u128)? as usize]
                .id
                .clone()
                .into(),
            ActionSchema::Integer { min, max } => {
                if min > max {
                    return Err(Error::Invalid("inverted integer domain".into()));
                }
                let offset = sample(
                    &mut self.rng,
                    (i128::from(*max) - i128::from(*min) + 1) as u128,
                )?;
                Value::from((i128::from(*min) + i128::from(offset)) as i64)
            }
            ActionSchema::Bool => Value::Bool(sample(&mut self.rng, 2)? == 1),
            ActionSchema::Unit { among } => among
                [sample(&mut self.rng, among.len() as u128)? as usize]
                .as_str()
                .into(),
            ActionSchema::Hex { among: Some(among) } => among
                [sample(&mut self.rng, among.len() as u128)? as usize]
                .as_str()
                .into(),
            ActionSchema::Hex { among: None }
            | ActionSchema::Path { .. }
            | ActionSchema::Text { .. } => self
                .candidates
                .candidate(self.request, schema, self.attempt)
                .ok_or_else(|| {
                    Error::Invalid("ruleset candidate hook required for hex/path actions".into())
                })?,
            ActionSchema::Record { fields } => {
                let mut object = Map::new();
                for field in fields {
                    if !field.optional || sample(&mut self.rng, 2)? == 1 {
                        object.insert(field.name.clone(), self.action(&field.schema, depth + 1)?);
                    }
                }
                Value::Object(object)
            }
            ActionSchema::List { item, min, max } => {
                if min > max {
                    return Err(Error::Invalid("inverted list domain".into()));
                }
                let length = u64::from(*min) + sample(&mut self.rng, u128::from(max - min) + 1)?;
                if length > u64::from(self.nodes_left) {
                    return Err(Error::Invalid("action generation budget exhausted".into()));
                }
                let mut list = Vec::new();
                for _ in 0..length {
                    list.push(self.action(item, depth + 1)?);
                }
                Value::Array(list)
            }
        })
    }
}

fn requires_candidates(schema: &ActionSchema) -> bool {
    match schema {
        ActionSchema::Hex { among: None }
        | ActionSchema::Path { .. }
        | ActionSchema::Text { .. } => true,
        ActionSchema::Record { fields } => fields
            .iter()
            .any(|field| requires_candidates(&field.schema)),
        ActionSchema::List { item, max, .. } => *max > 0 && requires_candidates(item),
        _ => false,
    }
}

fn unavailable_domain(request: &DecisionRequest, candidates: &dyn Candidates) -> bool {
    !candidates.has_unenumerated_domains() && requires_candidates(&request.space.schema)
}

pub fn answer(
    request: &DecisionRequest,
    epoch: u64,
    mode: Mode,
    attempt: u32,
    candidates: &dyn Candidates,
) -> Result<DecisionResponse, Error> {
    let seed: [u8; 32] = Sha256::digest(serde_json::to_vec(&(request, epoch, attempt))?).into();
    let mut generator = Generator {
        request,
        rng: ChaCha8Rng::from_seed(seed),
        nodes_left: 4096,
        candidates,
        attempt,
    };
    let unavailable = unavailable_domain(request, candidates);
    if unavailable && request.space.pass.is_none() {
        return Err(Error::Invalid(format!(
            "scripted action domain unavailable for {}: ruleset candidate hook required and no declared pass",
            request.kind
        )));
    }
    // A declared pass is this baseline's policy for an unavailable domain. It is
    // selected before generation, never substituted after an execution failure.
    let pass = request.space.pass.is_some()
        && (unavailable || mode == Mode::PassWhenPossible || sample(&mut generator.rng, 5)? == 0);
    let action = if pass {
        Value::Null
    } else {
        generator.action(&request.space.schema, 0)?
    };
    Ok(DecisionResponse {
        decision_id: request.id.clone(),
        seat: request.seat,
        controller_epoch: epoch,
        decision_revision: request.revision,
        idempotency_key: format!(
            "scripted:{}:{epoch}:{}:{attempt}",
            request.id, request.revision
        ),
        action,
        public_explanation: None,
    })
}

#[derive(Debug)]
pub enum Step {
    Advanced,
    Responded { seat: SeatId },
    SeatPaused { seat: SeatId, error: String },
    Idle,
}

impl<R: Ruleset> Campaign<R> {
    /// One safe boundary of the runner. Caller yields between steps for control requests.
    pub fn step(&mut self, candidates: &dyn Candidates) -> Result<Step, Error> {
        if self.status() != &CampaignStatus::Running {
            return Ok(Step::Idle);
        }
        let pending = self.pending();
        if pending.is_empty() {
            self.advance()?;
            return Ok(Step::Advanced);
        }
        for request in pending {
            let binding = self.binding(request.seat);
            if binding.paused
                || !binding
                    .controller
                    .as_ref()
                    .is_some_and(|c| c.kind == cna_protocol::ControllerKind::Scripted)
            {
                continue;
            }
            let mode = if binding.config["mode"] == "pass_when_possible" {
                Mode::PassWhenPossible
            } else {
                Mode::LegalRandom
            };
            let epoch = binding.controller_epoch;
            let mut last_error = "scripted retry budget exhausted".to_owned();
            // Do not retry a domain we cannot generate, including a rejected pass.
            let attempts = if unavailable_domain(&request, candidates) {
                1
            } else {
                64
            };
            for attempt in 0..attempts {
                let response = match answer(&request, epoch, mode, attempt, candidates) {
                    Ok(r) => r,
                    Err(e) => {
                        last_error = e.to_string();
                        break;
                    }
                };
                match self.submit(response) {
                    Ok(receipt) if !receipt.duplicate => {
                        return Ok(Step::Responded { seat: request.seat });
                    }
                    Ok(_) => {
                        last_error =
                            "ruleset retained an already-answered request without a new revision"
                                .into();
                        break;
                    }
                    Err(Error::Rejected(cna_core::engine::Rejection::Illegal { message })) => {
                        last_error = message
                    }
                    Err(e) => return Err(e),
                }
            }
            self.pause_seat_with_reason(request.seat, &last_error)?;
            return Ok(Step::SeatPaused {
                seat: request.seat,
                error: last_error,
            });
        }
        Ok(Step::Idle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cna_core::{
        clock::{Anchor, Clock},
        decision::{ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger},
        ids::{Role, Side},
    };
    use serde_json::json;
    fn request(schema: ActionSchema) -> DecisionRequest {
        DecisionRequest {
            id: "d-test".into(),
            seat: SeatId::new(Side::Axis, Role::Commander),
            kind: "test".into(),
            revision: 1,
            clock: Clock::start(1, Anchor::new("test")),
            summary: "test".into(),
            rules: vec![],
            trigger: Trigger::Scheduled,
            secrecy: Secrecy::Open,
            space: ActionSpace::new(schema),
        }
    }
    #[test]
    fn structured_domains_produce_valid_deterministic_answers() {
        let req = request(ActionSchema::Record {
            fields: vec![
                FieldSchema {
                    name: "unit".into(),
                    doc: String::new(),
                    optional: false,
                    schema: ActionSchema::Unit {
                        among: vec!["u1".into(), "u2".into()],
                    },
                },
                FieldSchema {
                    name: "hex".into(),
                    doc: String::new(),
                    optional: false,
                    schema: ActionSchema::Hex {
                        among: Some(vec!["C4218".into()]),
                    },
                },
                FieldSchema {
                    name: "choice".into(),
                    doc: String::new(),
                    optional: false,
                    schema: ActionSchema::Choice {
                        options: vec![ChoiceOption {
                            id: "yes".into(),
                            label: "Yes".into(),
                            detail: None,
                        }],
                    },
                },
                FieldSchema {
                    name: "flags".into(),
                    doc: String::new(),
                    optional: false,
                    schema: ActionSchema::List {
                        item: Box::new(ActionSchema::Bool),
                        min: 2,
                        max: 4,
                    },
                },
            ],
        });
        for attempt in 0..20 {
            let a = answer(&req, 3, Mode::LegalRandom, attempt, &NoCandidates).unwrap();
            assert_eq!(
                a,
                answer(&req, 3, Mode::LegalRandom, attempt, &NoCandidates).unwrap()
            );
            assert!([json!("u1"), json!("u2")].contains(&a.action["unit"]));
            assert_eq!(a.action["hex"], "C4218");
            assert_eq!(a.action["choice"], "yes");
            let flags = a.action["flags"].as_array().unwrap();
            assert!((2..=4).contains(&flags.len()));
            assert!(flags.iter().all(Value::is_boolean));
        }
        let req = request(ActionSchema::Integer {
            min: i64::MIN,
            max: i64::MAX,
        });
        assert!(
            answer(&req, 0, Mode::LegalRandom, 0, &NoCandidates)
                .unwrap()
                .action
                .is_i64()
        );
    }
    struct Hook;
    impl Candidates for Hook {
        fn candidate(&self, _: &DecisionRequest, schema: &ActionSchema, _: u32) -> Option<Value> {
            match schema {
                ActionSchema::Path { .. } => Some(json!(["C4219"])),
                ActionSchema::Hex { .. } => Some(json!("C4218")),
                _ => None,
            }
        }
    }
    #[test]
    fn missing_domains_and_generation_exhaustion_fail_without_a_substitute_order() {
        for schema in [
            ActionSchema::Hex { among: None },
            ActionSchema::Path {
                from: "C4218".into(),
                max_steps: 2,
            },
        ] {
            let req = request(schema);
            assert!(answer(&req, 0, Mode::LegalRandom, 0, &NoCandidates).is_err());
            assert!(answer(&req, 0, Mode::LegalRandom, 0, &Hook).is_ok());
        }
        let req = request(ActionSchema::List {
            item: Box::new(ActionSchema::Bool),
            min: 5000,
            max: 5000,
        });
        assert!(answer(&req, 0, Mode::LegalRandom, 0, &NoCandidates).is_err());
        let mut req = request(ActionSchema::Choice { options: vec![] });
        assert!(answer(&req, 0, Mode::LegalRandom, 0, &NoCandidates).is_err());
        req.space.pass = Some("explicit pass".into());
        assert!(
            answer(&req, 0, Mode::PassWhenPossible, 0, &NoCandidates)
                .unwrap()
                .action
                .is_null()
        );
    }
    #[test]
    fn nested_unenumerated_domains_choose_declared_pass_before_generation() {
        let mut req = request(ActionSchema::List {
            item: Box::new(ActionSchema::Record {
                fields: vec![FieldSchema {
                    name: "path".into(),
                    doc: String::new(),
                    optional: false,
                    schema: ActionSchema::List {
                        item: Box::new(ActionSchema::Hex { among: None }),
                        min: 1,
                        max: 4096,
                    },
                }],
            }),
            min: 0,
            max: 4096,
        });
        req.kind = "test.movement".into();
        let error = answer(&req, 0, Mode::LegalRandom, 0, &NoCandidates).unwrap_err();
        assert!(error.to_string().contains("test.movement"));
        assert!(error.to_string().contains("no declared pass"));
        req.space.pass = Some("done".into());
        for attempt in 0..20 {
            assert!(
                answer(&req, 0, Mode::LegalRandom, attempt, &NoCandidates)
                    .unwrap()
                    .action
                    .is_null()
            );
        }
        // An existing domain provider is still used even when pass is available.
        req.space.schema = ActionSchema::Path {
            from: "C4218".into(),
            max_steps: 2,
        };
        assert!((0..20).any(|attempt| {
            answer(&req, 0, Mode::LegalRandom, attempt, &Hook)
                .unwrap()
                .action
                == json!(["C4219"])
        }));
        // A zero-length list never needs its item's domain.
        req.space.pass = None;
        req.space.schema = ActionSchema::List {
            item: Box::new(ActionSchema::Hex { among: None }),
            min: 0,
            max: 0,
        };
        assert_eq!(
            answer(&req, 0, Mode::LegalRandom, 0, &NoCandidates)
                .unwrap()
                .action,
            json!([])
        );
    }
}
