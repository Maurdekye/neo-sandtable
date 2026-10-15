//! Disjoint synthetic sampler controls only: not an Oob window/actor fixture.
//! Entire module and its observation calls are cfg(test); no production API.
use super::*;
use cna_core::{
    clock::{Anchor, Clock},
    decision::{ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger},
    ids::{Role, Side},
};
use std::cell::RefCell;

#[path = "replacement_sampler_frozen_b5.rs"]
mod frozen_b5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Begin,
    Sample(u128),
    Generate,
    Preview,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Observation {
    phase: Phase,
    word_pos: u128,
    stream: u64,
    next_words: [u64; 8],
}

thread_local! {
    static OBSERVATIONS: RefCell<Option<Vec<Observation>>> = const { RefCell::new(None) };
}

// The parent may call only this test-only observation. Clone reads never advance rng.
pub(super) fn observe(phase: Phase, rng: &ChaCha8Rng) {
    OBSERVATIONS.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(rows) = slot.as_mut() {
            let mut copy = rng.clone();
            rows.push(Observation {
                phase,
                word_pos: rng.get_word_pos(),
                stream: rng.get_stream(),
                next_words: std::array::from_fn(|_| copy.next_u64()),
            });
        }
    });
}

fn captured<T>(call: impl FnOnce() -> T) -> (T, Vec<Observation>) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            OBSERVATIONS.with(|cell| *cell.borrow_mut() = None);
        }
    }
    OBSERVATIONS.with(|cell| {
        assert!(
            cell.borrow().is_none(),
            "nested observation would corrupt evidence"
        );
        *cell.borrow_mut() = Some(Vec::new());
    });
    let reset = Reset;
    let result = call();
    let trace = OBSERVATIONS.with(|cell| cell.borrow_mut().take().unwrap());
    drop(reset);
    (result, trace)
}

fn choice(n: usize) -> ActionSchema {
    ActionSchema::Choice {
        options: (0..n)
            .map(|i| ChoiceOption {
                id: format!("option-{i}"),
                label: format!("Option {i}"),
                detail: None,
            })
            .collect(),
    }
}

fn request(kind: &str, schema: ActionSchema, pass: Option<&str>) -> DecisionRequest {
    DecisionRequest {
        id: format!("sampler-control:{kind}").as_str().into(),
        seat: SeatId::new(Side::Axis, Role::FrontLine),
        kind: kind.into(),
        revision: 3,
        clock: Clock::start(1, Anchor::new("sampler-control")),
        summary: "Synthetic sampler shape, not an Oob declaration".into(),
        rules: vec![],
        trigger: Trigger::Scheduled,
        secrecy: Secrecy::SecretSimultaneous,
        space: ActionSpace {
            schema,
            pass: pass.map(str::to_owned),
            context: None,
        },
    }
}

// Preserve the complete Result branch and payload. Invalid(String) is the only
// error these actual sampler inputs should produce; other variants fail the test.
fn same_result(actual: &Result<DecisionResponse, Error>, old: &Result<DecisionResponse, Error>) {
    match (actual, old) {
        (Ok(actual), Ok(old)) => {
            assert_eq!(actual, old);
            assert_eq!(
                serde_json::to_vec(actual).unwrap(),
                serde_json::to_vec(old).unwrap()
            );
        }
        (Err(Error::Invalid(actual)), Err(Error::Invalid(old))) => assert_eq!(actual, old),
        _ => panic!("complete Result mismatch: actual={actual:?}; frozen_b5={old:?}"),
    }
}

fn same_as_b5(req: &DecisionRequest, epoch: u64, mode: Mode, attempt: u32) -> Vec<Observation> {
    let before = serde_json::to_vec(req).unwrap();
    let (actual, actual_trace) = captured(|| answer(req, epoch, mode, attempt, &NoCandidates));
    let (old, old_trace) = captured(|| frozen_b5::answer(req, epoch, mode, attempt, &NoCandidates));
    same_result(&actual, &old);
    // Compare actual word positions AND eight next words at every observed site,
    // including sample/preview/generator sites on error paths. No String-only oracle.
    assert_eq!(
        actual_trace, old_trace,
        "entire actual RNG/path trace for {req:?}"
    );
    assert_eq!(serde_json::to_vec(req).unwrap(), before);
    actual_trace
}

/// Cases: land:17.33, land:20.41, land:20.43, land:20.49
#[test]
fn replacements_selected_branch_is_canonical_deterministic_and_observed_zero_draw() {
    for kind in ["cna.replacements.training", "cna.replacements.allocation"] {
        for meaning in ["Declared pass", ""] {
            let req = request(kind, choice(0), Some(meaning));
            let mut old_pass = false;
            let mut old_error = false;
            for epoch in [0, 7] {
                for attempt in 0..64 {
                    for mode in [Mode::LegalRandom, Mode::PassWhenPossible] {
                        let (actual, trace) =
                            captured(|| answer(&req, epoch, mode, attempt, &NoCandidates));
                        let actual = actual.unwrap();
                        let (again, again_trace) =
                            captured(|| answer(&req, epoch, mode, attempt, &NoCandidates));
                        assert_eq!(again.unwrap(), actual);
                        assert_eq!(again_trace, trace);
                        assert_eq!(
                            trace.len(),
                            2,
                            "only actual Begin and End; no sample, preview or generation"
                        );
                        assert_eq!(trace[0].phase, Phase::Begin);
                        assert_eq!(trace[1].phase, Phase::End);
                        assert_eq!(trace[0].word_pos, 0);
                        assert_eq!(trace[1].word_pos, trace[0].word_pos);
                        assert_eq!(trace[1].stream, trace[0].stream);
                        assert_eq!(trace[1].next_words, trace[0].next_words);
                        let expected = DecisionResponse {
                            decision_id: req.id.clone(),
                            seat: req.seat,
                            controller_epoch: epoch,
                            decision_revision: req.revision,
                            idempotency_key: format!(
                                "scripted:{}:{epoch}:{}:{attempt}",
                                req.id, req.revision
                            ),
                            action: Value::Null,
                            public_explanation: None,
                        };
                        assert_eq!(actual, expected);
                        assert_eq!(
                            serde_json::to_vec(&actual).unwrap(),
                            serde_json::to_vec(&expected).unwrap()
                        );
                        if mode == Mode::LegalRandom {
                            let (old, _) = captured(|| {
                                frozen_b5::answer(&req, epoch, mode, attempt, &NoCandidates)
                            });
                            match old {
                                Ok(old) => {
                                    old_pass = true;
                                    assert_eq!(old, expected);
                                }
                                Err(Error::Invalid(message)) => {
                                    old_error = true;
                                    assert_eq!(message, "empty or invalid action domain");
                                }
                                other => panic!("unexpected b5 branch {other:?}"),
                            }
                        }
                    }
                }
            }
            assert!(
                old_pass && old_error,
                "cover both genuine b5 sample5 outcomes"
            );
        }
    }
}

/// Cases: land:17.33, land:20.41, land:20.43, land:20.49
#[test]
fn actual_owner982_empty_requests_select_pass_without_a_local_rng_draw() {
    use cna_core::engine::Ruleset;
    let content = cna_rules::CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    for anchor in [
        "opstage.organization.reorganization",
        "opstage.organization.training",
    ] {
        // The same precisely consented controlled cursor, not an RP entitlement.
        let mut game = cna_core::engine::Game::<cna_rules::Cna> {
            state: cna_rules::State::new(&content).unwrap(),
            rng: cna_core::dice::CampaignRng::from_seed([92; 32]).state(),
        };
        game.state.cursor.game_turn = 3;
        game.state.cursor.op_stage = Some(1);
        game.state.cursor.half = None;
        game.state.cursor.block = cna_rules::seq::Block::OpStage;
        game.state.cursor.index = cna_rules::seq::OPSTAGE
            .iter()
            .position(|step| step.anchor == anchor)
            .unwrap();
        game.state.cursor.entered = false;
        let opened = cna_core::engine::evaluate(
            &cna_rules::Cna::dev(),
            &content,
            &game,
            &cna_core::engine::Command::Advance,
        )
        .unwrap();
        let pending = cna_rules::Cna::dev().pending(&content, &opened.game.state);
        assert_eq!(pending.len(), 6);
        for request in pending {
            assert!(
                matches!(&request.space.schema, ActionSchema::Choice { options } if options.is_empty())
            );
            assert!(request.space.pass.is_some());
            let (result, trace) =
                captured(|| answer(&request, 1, Mode::LegalRandom, 0, &NoCandidates));
            let answer = result.unwrap();
            assert_eq!(
                trace.iter().map(|row| row.phase).collect::<Vec<_>>(),
                [Phase::Begin, Phase::End]
            );
            assert_eq!(trace[0].word_pos, 0);
            assert_eq!(trace[1].word_pos, trace[0].word_pos);
            assert_eq!(trace[1].stream, trace[0].stream);
            assert_eq!(trace[1].next_words, trace[0].next_words);
            assert_eq!(
                answer,
                DecisionResponse {
                    decision_id: request.id.clone(),
                    seat: request.seat,
                    controller_epoch: 1,
                    decision_revision: request.revision,
                    idempotency_key: format!("scripted:{}:1:{}:0", request.id, request.revision),
                    action: Value::Null,
                    public_explanation: None,
                }
            );
        }
    }
}

/// Cases: land:17.33, land:20.41, land:20.43, land:20.49
#[test]
fn replacements_nonempty_and_no_pass_match_whole_frozen_b5_result_and_rng() {
    for kind in ["cna.replacements.training", "cna.replacements.allocation"] {
        for options in [0, 1, 4] {
            for pass in [None, Some("Declared pass"), Some("")] {
                if options == 0 && pass.is_some() {
                    continue;
                }
                let req = request(kind, choice(options), pass);
                for epoch in [0, 7] {
                    for attempt in 0..32 {
                        for mode in [Mode::LegalRandom, Mode::PassWhenPossible] {
                            same_as_b5(&req, epoch, mode, attempt);
                        }
                    }
                }
            }
        }
    }
}

/// Cases: land:17.33, land:20.41, land:20.43, land:20.49
#[test]
fn replacements_other_kinds_keep_both_empty_choice_outcomes_and_actual_rng() {
    for kind in [
        "test.empty-choice",
        "cna.replacements.schedule",
        "cna.replacements.distribution",
        "cna.replacements.arrival",
        "CNA.replacements.training",
        "cna.replacements.training.extra",
        "cna.replacements.allocation.extra",
    ] {
        let mut saw_pass = false;
        let mut saw_error = false;
        for pass in [None, Some("Declared pass")] {
            let req = request(kind, choice(0), pass);
            for epoch in [0, 7] {
                for attempt in 0..64 {
                    for mode in [Mode::LegalRandom, Mode::PassWhenPossible] {
                        same_as_b5(&req, epoch, mode, attempt);
                        if pass.is_some() && mode == Mode::LegalRandom {
                            match answer(&req, epoch, mode, attempt, &NoCandidates) {
                                Ok(response) => {
                                    saw_pass = true;
                                    assert!(response.action.is_null());
                                }
                                Err(Error::Invalid(message)) => {
                                    saw_error = true;
                                    assert_eq!(message, "empty or invalid action domain");
                                }
                                other => panic!("unexpected actual excluded outcome {other:?}"),
                            }
                        }
                    }
                }
            }
        }
        assert!(saw_pass && saw_error);
    }
}

fn excluded_shapes() -> Vec<ActionSchema> {
    let nested = ActionSchema::Record {
        fields: vec![FieldSchema {
            name: "nested".into(),
            doc: "Synthetic nested empty choice".into(),
            schema: choice(0),
            optional: false,
        }],
    };
    vec![
        nested,
        ActionSchema::Unit { among: vec![] },
        ActionSchema::Hex {
            among: Some(vec![]),
        },
        ActionSchema::Record { fields: vec![] },
        ActionSchema::List {
            item: Box::new(choice(0)),
            min: 0,
            max: 0,
        },
        ActionSchema::List {
            item: Box::new(choice(0)),
            min: 1,
            max: 1,
        },
        ActionSchema::Integer { min: 1, max: 0 },
        ActionSchema::List {
            item: Box::new(ActionSchema::Bool),
            min: 2,
            max: 1,
        },
        ActionSchema::Integer {
            min: i64::MIN,
            max: i64::MAX,
        },
        ActionSchema::Integer {
            min: -1,
            max: i64::MAX,
        },
        // Existing resource-budget policy remains part of the frozen result.
        ActionSchema::List {
            item: Box::new(ActionSchema::Bool),
            min: 4096,
            max: 4096,
        },
    ]
}

/// Cases: land:17.33, land:20.41, land:20.43, land:20.49
#[test]
fn replacements_non_choice_shapes_keep_frozen_full_results_and_rng() {
    for kind in ["cna.replacements.training", "cna.replacements.allocation"] {
        for schema in excluded_shapes() {
            for pass in [None, Some("Declared pass")] {
                let req = request(kind, schema.clone(), pass);
                for epoch in [0, 7] {
                    for attempt in 0..8 {
                        for mode in [Mode::LegalRandom, Mode::PassWhenPossible] {
                            same_as_b5(&req, epoch, mode, attempt);
                        }
                    }
                }
            }
        }
    }
}

/// Cases: land:17.33, land:20.41, land:20.43, land:20.49
#[test]
fn replacements_excluded_rejection_sampling_preserves_actual_cursor_and_next_words() {
    // Cardinality 2^63+1 rejects roughly half of u64 inputs; no rare 1-in-2^64 fixture.
    let req = request(
        "cna.replacements.training",
        ActionSchema::Integer {
            min: -1,
            max: i64::MAX,
        },
        None,
    );
    let mut saw_rejection = false;
    for attempt in 0..128 {
        let trace = same_as_b5(&req, 0, Mode::LegalRandom, attempt);
        let end = trace.iter().find(|row| row.phase == Phase::End).unwrap();
        assert!(end.word_pos >= 2);
        saw_rejection |= end.word_pos > 2;
    }
    assert!(
        saw_rejection,
        "must exercise a real extra rejection-sampling word"
    );
}

/// Cases: land:17.33, land:20.41, land:20.43, land:20.49
#[test]
fn replacements_absent_or_malformed_declared_choice_cannot_reach_answer_observation() {
    let value = serde_json::to_value(request(
        "cna.replacements.training",
        choice(0),
        Some("pass"),
    ))
    .unwrap();
    let mut missing_schema = value.clone();
    missing_schema["space"]
        .as_object_mut()
        .unwrap()
        .remove("schema");
    let mut missing_options = value.clone();
    missing_options["space"]["schema"]
        .as_object_mut()
        .unwrap()
        .remove("options");
    let mut malformed_options = value;
    malformed_options["space"]["schema"]["options"] = Value::Null;
    for invalid in [missing_schema, missing_options, malformed_options] {
        let (decoded, trace) = captured(|| serde_json::from_value::<DecisionRequest>(invalid));
        assert!(decoded.is_err());
        assert!(
            trace.is_empty(),
            "typed decoder refusal occurs before any sampler path"
        );
    }
}
