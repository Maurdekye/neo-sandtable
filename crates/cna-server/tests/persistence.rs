use std::collections::BTreeMap;

use cna_core::{
    clock::{Anchor, Clock},
    decision::{ActionSchema, ActionSpace, DecisionRequest, DecisionResponse, Secrecy, Trigger},
    dice::CampaignRng,
    engine::{Cx, EngineError, Game, Progress, Rejection, Ruleset},
    event::EngineEvent,
    ids::SeatId,
    visibility::{Audience, Perspective},
};
use cna_protocol::{
    CampaignMeta, ControllerInfo, ControllerKind, GameEvent, PendingDecision, ServerMessage,
    TranscriptEntry, ViewState,
};
use cna_server::{
    Campaign, CampaignStatus, Error, Pins,
    scripted::{NoCandidates, Step},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    round: u16,
    active: bool,
    answers: BTreeMap<SeatId, i64>,
    rolls: Vec<u8>,
}
struct Tiny {
    unsupported: bool,
    empty_progress: bool,
    impossible: bool,
}
fn rules() -> Tiny {
    Tiny {
        unsupported: false,
        empty_progress: false,
        impossible: false,
    }
}
fn clock(state: &State) -> cna_protocol::Clock {
    cna_protocol::Clock {
        game_turn: state.round + 1,
        date: "1940-09-15".into(),
        stage: "test".into(),
        op_stage: None,
        phase: "orders".into(),
        segment: None,
        step: None,
        phasing: None,
    }
}
impl Ruleset for Tiny {
    type State = State;
    type Content = ();
    fn profile_id(&self) -> &str {
        "tiny-v1"
    }
    fn advance(&self, _: &(), state: &mut State, cx: &mut Cx<'_>) -> Result<Progress, EngineError> {
        if self.empty_progress {
            state.round = 999;
            cx.rng.d6();
            return Ok(Progress::AwaitingDecisions);
        }
        if self.unsupported {
            state.round = 999;
            cx.rng.d6();
            return Err(EngineError::Unsupported {
                case: "test".into(),
                detail: "deliberate".into(),
            });
        }
        if state.round == 5 {
            return Ok(Progress::Finished {
                summary: "synthetic complete".into(),
            });
        }
        state.active = true;
        state.answers.clear();
        cx.emit(EngineEvent::public(GameEvent::PhaseChanged {
            clock: clock(state),
        }));
        for request in self.pending(&(), state) {
            cx.emit(EngineEvent::new(
                Audience::Seat(request.seat),
                GameEvent::DecisionOpened {
                    decision: PendingDecision {
                        id: request.id.to_string(),
                        seat: request.seat.to_string(),
                        kind: request.kind,
                        summary: request.summary,
                        opened_seq: 999999,
                        rules: Vec::new(),
                        space: None,
                    },
                },
            ));
        }
        Ok(Progress::AwaitingDecisions)
    }
    fn respond(
        &self,
        _: &(),
        state: &mut State,
        response: &DecisionResponse,
        cx: &mut Cx<'_>,
    ) -> Result<(), Rejection> {
        let Some(request) = self
            .pending(&(), state)
            .into_iter()
            .find(|d| d.id == response.decision_id)
        else {
            return Err(Rejection::UnknownDecision {
                decision_id: response.decision_id.clone(),
            });
        };
        if request.seat != response.seat {
            return Err(Rejection::WrongSeat {
                decision_id: request.id,
                seat: response.seat,
            });
        }
        if self.impossible {
            return Err(Rejection::Illegal {
                message: "no candidate works".into(),
            });
        }
        let Some(value) = response.action.as_i64().filter(|n| (0..=3).contains(n)) else {
            return Err(Rejection::Illegal {
                message: "need integer 0..3".into(),
            });
        };
        state.answers.insert(response.seat, value);
        cx.emit(EngineEvent::new(
            Audience::Seat(response.seat),
            GameEvent::DecisionResolved {
                decision_id: response.decision_id.to_string(),
                seat: response.seat.to_string(),
                summary: format!("secret {value}"),
            },
        ));
        cx.emit(EngineEvent::new(
            Audience::Operator,
            GameEvent::Note {
                text: "operator-only bookkeeping".into(),
            },
        ));
        if state.answers.len() == 10 {
            let die = cx.rng.d6().value();
            state.rolls.push(die);
            cx.emit(EngineEvent::public(GameEvent::DiceRolled {
                purpose: "synthetic".into(),
                dice: vec![die],
                reading: None,
                rule: None,
            }));
            state.active = false;
            state.round += 1;
        }
        Ok(())
    }
    fn pending(&self, _: &(), state: &State) -> Vec<DecisionRequest> {
        if !state.active {
            return vec![];
        }
        SeatId::all()
            .filter(|s| !state.answers.contains_key(s))
            .map(|seat| DecisionRequest {
                id: format!("d-{}-{seat}", state.round).as_str().into(),
                seat,
                kind: "tiny.integer".into(),
                revision: 1,
                clock: Clock::start(state.round + 1, Anchor::new("test.orders")),
                summary: "Choose a number".into(),
                rules: vec![],
                trigger: Trigger::Scheduled,
                secrecy: Secrecy::SecretSimultaneous,
                space: ActionSpace::new(ActionSchema::Integer { min: 0, max: 3 }),
            })
            .collect()
    }
    fn observe(&self, _: &(), state: &State, perspective: Perspective) -> Value {
        let answers: BTreeMap<_, _> = state
            .answers
            .iter()
            .filter(|(seat, _)| perspective.can_see(&Audience::Seat(**seat)))
            .map(|(seat, value)| (seat.to_string(), *value))
            .collect();
        json!({"answers": answers})
    }
    fn view(&self, _: &(), state: &State, perspective: Perspective) -> ViewState {
        ViewState {
            clock: clock(state),
            stacks: vec![],
            units: BTreeMap::new(),
            markers: vec![],
            pending: self
                .pending(&(), state)
                .into_iter()
                .filter(|d| perspective.can_see(&Audience::Seat(d.seat)))
                .map(|d| PendingDecision {
                    id: d.id.to_string(),
                    seat: d.seat.to_string(),
                    kind: d.kind,
                    summary: d.summary,
                    opened_seq: 999999,
                    rules: Vec::new(),
                    space: None,
                })
                .collect(),
        }
    }
}
fn pins() -> Pins {
    Pins {
        rules_profile: "tiny-v1".into(),
        content_hash: "empty-content-v1".into(),
        engine_version: "test-v1".into(),
    }
}
fn campaign(path: &std::path::Path, ruleset: Tiny) -> Campaign<Tiny> {
    Campaign::create(
        path,
        ruleset,
        (),
        Game {
            state: State::default(),
            rng: CampaignRng::from_seed([7; 32]).state(),
        },
        CampaignMeta {
            id: "tiny".into(),
            scenario_id: "tiny".into(),
            rules_profile: "tiny-v1".into(),
            title: "Synthetic".into(),
            seats: SeatId::all()
                .map(|s| cna_protocol::SeatInfo {
                    id: s.to_string(),
                    side: s.side,
                    role: s.role,
                    controller: None,
                    status: cna_protocol::SeatStatus::Idle,
                })
                .collect(),
        },
        pins(),
    )
    .unwrap()
}
fn response(campaign: &Campaign<Tiny>, seat: SeatId) -> DecisionResponse {
    let d = campaign
        .pending()
        .into_iter()
        .find(|d| d.seat == seat)
        .unwrap();
    DecisionResponse {
        decision_id: d.id,
        seat,
        controller_epoch: campaign.binding(seat).controller_epoch,
        decision_revision: d.revision,
        idempotency_key: format!("key-{seat}"),
        action: json!(2),
        public_explanation: None,
    }
}
fn scripted<R: Ruleset>(campaign: &mut Campaign<R>) {
    for seat in SeatId::all() {
        campaign
            .handover(
                seat,
                Some(ControllerInfo {
                    kind: ControllerKind::Scripted,
                    label: "legal-random".into(),
                }),
                json!({"mode":"legal_random"}),
            )
            .unwrap();
    }
}

#[test]
fn duplicate_validation_and_handover_do_not_touch_secret_state_or_rng() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("campaign.sqlite");
    let mut game = campaign(&path, rules());
    game.advance().unwrap();
    let seat = SeatId::all().next().unwrap();
    let order = response(&game, seat);
    let before = game.state_hash().unwrap();
    game.validate(&order).unwrap();
    assert_eq!(before, game.state_hash().unwrap());
    let mut illegal = order.clone();
    illegal.action = json!(99);
    assert!(game.validate(&illegal).is_err());
    assert_eq!(before, game.state_hash().unwrap());
    assert!(!game.submit(order.clone()).unwrap().duplicate);
    let accepted = game.state_hash().unwrap();
    assert!(game.submit(order.clone()).unwrap().duplicate);
    assert_eq!(accepted, game.state_hash().unwrap());
    let mut conflict = order.clone();
    conflict.action = json!(1);
    assert!(matches!(
        game.submit(conflict),
        Err(Error::IdempotencyConflict)
    ));
    game.handover(seat, None, json!({"new":true})).unwrap();
    assert_eq!(accepted, game.state_hash().unwrap());
    // Retrying a committed command is acknowledged even after its controller was replaced.
    assert!(game.submit(order.clone()).unwrap().duplicate);
    let other = SeatId::all().nth(1).unwrap();
    let mut stale = response(&game, other);
    game.handover(other, None, Value::Null).unwrap();
    stale.idempotency_key = "late".into();
    assert!(matches!(game.submit(stale), Err(Error::StaleEpoch)));
    assert_eq!(accepted, game.state_hash().unwrap());
    drop(game);
    let restored = Campaign::recover(&path, rules(), (), &pins()).unwrap();
    assert_eq!(accepted, restored.state_hash().unwrap());
    assert_eq!(restored.pending().len(), 9);
    assert_eq!(restored.binding(seat).controller_epoch, 1);
    assert_eq!(restored.observe(seat)["answers"][seat.to_string()], 2);
}

#[test]
fn atomic_failure_rolls_back_command_events_pending_rng_and_memory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("campaign.sqlite");
    let mut game = campaign(&path, rules());
    game.advance().unwrap();
    let seat = SeatId::all().next().unwrap();
    let before = game.state_hash().unwrap();
    let order = response(&game, seat);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_stream BEFORE INSERT ON perspective_events BEGIN SELECT RAISE(ABORT, 'injected stream write failure'); END;").unwrap();
    assert!(matches!(game.submit(order.clone()), Err(Error::Storage(_))));
    assert_eq!(before, game.state_hash().unwrap());
    assert_eq!(game.pending().len(), 10);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        11
    );
    drop(game);
    let mut restored = Campaign::recover(&path, rules(), (), &pins()).unwrap();
    assert_eq!(before, restored.state_hash().unwrap());
    db.execute_batch("DROP TRIGGER fail_stream").unwrap();
    restored.submit(order.clone()).unwrap();
    assert!(restored.submit(order).unwrap().duplicate);
}

#[test]
fn every_perspective_has_contiguous_authorized_events_and_transcripts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("campaign.sqlite");
    let mut game = campaign(&path, rules());
    game.advance().unwrap();
    let axis = SeatId::all().next().unwrap();
    let enemy = SeatId::all().find(|s| s.side != axis.side).unwrap();
    let before = game.current_seq(Perspective::Side(axis.side)).unwrap();
    game.submit(response(&game, enemy)).unwrap();
    assert_eq!(
        before,
        game.current_seq(Perspective::Side(axis.side)).unwrap()
    );
    game.submit(response(&game, axis)).unwrap();
    for seat in SeatId::all() {
        game.transcript(
            seat,
            "2026-10-06T11:00:00Z",
            TranscriptEntry::AssistantText {
                text: format!("private {seat}"),
            },
        )
        .unwrap();
    }
    for perspective in Perspective::all() {
        let events = game.events_after(perspective, 0, 512).unwrap();
        assert!(!events.is_empty());
        for (i, event) in events.iter().enumerate() {
            let ServerMessage::Event { seq, event, .. } = event else {
                panic!("not event")
            };
            assert_eq!(*seq, i as u64 + 1);
            match event {
                GameEvent::DecisionOpened { decision } => {
                    let seat: SeatId = decision.seat.parse().unwrap();
                    assert!(perspective.can_see(&Audience::Seat(seat)));
                    assert_eq!(decision.opened_seq, *seq);
                }
                GameEvent::DecisionResolved { seat, .. } => {
                    assert!(perspective.can_see(&Audience::Seat(seat.parse().unwrap())))
                }
                GameEvent::Note { text } if text == "operator-only bookkeeping" => {
                    assert_eq!(perspective, Perspective::Operator)
                }
                _ => {}
            }
        }
        let view = game.view(perspective).unwrap();
        for d in view.pending {
            assert!(d.opened_seq <= game.current_seq(perspective).unwrap());
            assert!(d.opened_seq > 0);
        }
        for seat in SeatId::all() {
            let rows = game.transcripts_after(perspective, seat, 0, 100).unwrap();
            assert_eq!(
                rows.len(),
                usize::from(perspective.can_see(&Audience::Seat(seat)))
            );
            if let Some(ServerMessage::Transcript { tseq, game_seq, .. }) = rows.first() {
                assert_eq!(*tseq, 1);
                assert_eq!(*game_seq, game.current_seq(perspective).unwrap());
            }
        }
    }
    assert!(
        game.events_after(Perspective::Seat(axis), 9999, 100)
            .is_err()
    );
    let last = game.current_seq(Perspective::Seat(axis)).unwrap();
    assert!(
        game.events_after(Perspective::Seat(axis), last, 100)
            .unwrap()
            .is_empty()
    );
    // Metadata also hides enemy-seat runtime activity and controller configuration.
    let meta = game.metadata(Perspective::Seat(axis));
    assert!(
        meta.seats
            .iter()
            .filter(|s| s.id != axis.to_string())
            .all(|s| s.controller.is_none() && s.status == cna_protocol::SeatStatus::Idle)
    );
}

#[test]
fn scripted_campaign_finishes_and_replays_after_periodic_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("campaign.sqlite");
    let mut game = campaign(&path, rules());
    scripted(&mut game);
    for _ in 0..100 {
        if matches!(game.status(), CampaignStatus::Finished { .. }) {
            break;
        }
        assert!(!matches!(
            game.step(&NoCandidates).unwrap(),
            Step::Idle | Step::SeatPaused { .. }
        ));
    }
    assert!(matches!(game.status(), CampaignStatus::Finished { .. }));
    let state_hash = game.state_hash().unwrap();
    let seqs: Vec<_> = Perspective::all()
        .map(|p| game.current_seq(p).unwrap())
        .collect();
    let db = rusqlite::Connection::open(&path).unwrap();
    let checkpoint: u64 = db
        .query_row("SELECT MAX(revision) FROM checkpoints", [], |r| r.get(0))
        .unwrap();
    assert!(checkpoint >= 32);
    let revision: u64 = db
        .query_row("SELECT revision FROM campaign", [], |r| r.get(0))
        .unwrap();
    assert!(revision > checkpoint);
    drop(game);
    let restored = Campaign::recover(&path, rules(), (), &pins()).unwrap();
    assert_eq!(state_hash, restored.state_hash().unwrap());
    assert!(matches!(restored.status(), CampaignStatus::Finished { .. }));
    assert_eq!(
        seqs,
        Perspective::all()
            .map(|p| restored.current_seq(p).unwrap())
            .collect::<Vec<_>>()
    );
    let mut wrong = pins();
    wrong.content_hash = "changed".into();
    assert!(matches!(
        Campaign::recover(&path, rules(), (), &wrong),
        Err(Error::Recovery(_))
    ));
    db.execute(
        "UPDATE commands SET transition_hash='corrupt' WHERE revision=?",
        [revision],
    )
    .unwrap();
    assert!(matches!(
        Campaign::recover(&path, rules(), (), &pins()),
        Err(Error::Recovery(_))
    ));
}

#[test]
fn engine_failure_stops_without_an_invented_order_and_bad_controller_pauses_seat() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unsupported.sqlite");
    let mut unsupported = campaign(
        &path,
        Tiny {
            unsupported: true,
            empty_progress: false,
            impossible: false,
        },
    );
    let before = unsupported.state_hash().unwrap();
    assert!(unsupported.advance().is_err());
    assert_eq!(before, unsupported.state_hash().unwrap());
    assert!(matches!(
        unsupported.status(),
        CampaignStatus::Stopped { .. }
    ));
    drop(unsupported);
    assert!(matches!(
        Campaign::recover(
            &path,
            Tiny {
                unsupported: true,
                empty_progress: false,
                impossible: false
            },
            (),
            &pins()
        )
        .unwrap()
        .status(),
        CampaignStatus::Stopped { .. }
    ));
    let mut game = campaign(
        &dir.path().join("bad-controller.sqlite"),
        Tiny {
            unsupported: false,
            empty_progress: false,
            impossible: true,
        },
    );
    scripted(&mut game);
    game.advance().unwrap();
    let before = game.state_hash().unwrap();
    let Step::SeatPaused { seat, .. } = game.step(&NoCandidates).unwrap() else {
        panic!("must pause")
    };
    assert!(game.binding(seat).paused);
    assert_eq!(before, game.state_hash().unwrap());
    assert_eq!(game.pending().len(), 10);
    game.set_paused(true).unwrap();
    assert!(matches!(game.step(&NoCandidates).unwrap(), Step::Idle));
}

#[test]
fn inconsistent_progress_stops_without_committing_a_transition() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.sqlite");
    let mut ruleset = rules();
    ruleset.empty_progress = true;
    let mut game = campaign(&path, ruleset);
    let before = game.state_hash().unwrap();
    assert!(game.advance().is_err());
    assert_eq!(before, game.state_hash().unwrap());
    assert!(matches!(game.status(), CampaignStatus::Stopped { .. }));
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        0
    );
}

struct Sticky;
impl Ruleset for Sticky {
    type State = State;
    type Content = ();
    fn profile_id(&self) -> &str {
        "tiny-v1"
    }
    fn advance(&self, c: &(), s: &mut State, cx: &mut Cx<'_>) -> Result<Progress, EngineError> {
        rules().advance(c, s, cx)
    }
    fn respond(
        &self,
        _: &(),
        _: &mut State,
        _: &DecisionResponse,
        _: &mut Cx<'_>,
    ) -> Result<(), Rejection> {
        Ok(())
    }
    fn pending(&self, c: &(), s: &State) -> Vec<DecisionRequest> {
        rules().pending(c, s)
    }
    fn observe(&self, c: &(), s: &State, p: Perspective) -> Value {
        rules().observe(c, s, p)
    }
    fn view(&self, c: &(), s: &State, p: Perspective) -> ViewState {
        rules().view(c, s, p)
    }
}
#[test]
fn unchanged_scripted_request_pauses_instead_of_spinning_on_duplicate_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sticky.sqlite");
    let mut game = Campaign::create(
        &path,
        Sticky,
        (),
        Game {
            state: State::default(),
            rng: CampaignRng::from_seed([7; 32]).state(),
        },
        CampaignMeta {
            id: "sticky".into(),
            scenario_id: "sticky".into(),
            rules_profile: "tiny-v1".into(),
            title: "Sticky".into(),
            seats: vec![],
        },
        pins(),
    )
    .unwrap();
    scripted(&mut game);
    game.advance().unwrap();
    assert!(matches!(
        game.step(&NoCandidates).unwrap(),
        Step::Responded { .. }
    ));
    assert!(matches!(
        game.step(&NoCandidates).unwrap(),
        Step::SeatPaused { .. }
    ));
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        2
    );
}
#[test]
fn unsupported_scripted_configuration_is_not_an_implicit_random_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let mut game = campaign(&dir.path().join("bad-binding.sqlite"), rules());
    let seat = SeatId::all().next().unwrap();
    let binding = game.binding(seat).clone();
    assert!(
        game.handover(
            seat,
            Some(ControllerInfo {
                kind: ControllerKind::Scripted,
                label: "unimplemented".into()
            }),
            json!({"mode":"conservative"})
        )
        .is_err()
    );
    assert_eq!(&binding, game.binding(seat));
}

#[test]
fn scripted_transcript_is_atomic_with_answer_and_idempotent_on_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("campaign.sqlite");
    let mut game = campaign(&path, rules());
    scripted(&mut game);
    game.advance().unwrap();
    let seat: SeatId = "axis.commander".parse().unwrap();
    let order = response(&game, seat);
    let before = game.state_hash().unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_scripted_transcript BEFORE INSERT ON transcripts
        BEGIN SELECT RAISE(ABORT,'injected transcript failure'); END;",
    )
    .unwrap();
    assert!(matches!(game.submit(order.clone()), Err(Error::Storage(_))));
    assert_eq!(game.state_hash().unwrap(), before);
    assert_eq!(game.pending().len(), 10);
    assert_eq!(
        db.query_row("SELECT count(*) FROM commands", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM transcripts", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    db.execute_batch("DROP TRIGGER fail_scripted_transcript")
        .unwrap();
    game.submit(order.clone()).unwrap();
    assert!(game.submit(order.clone()).unwrap().duplicate);
    let rows = game
        .transcripts_after(Perspective::Seat(seat), seat, 0, 512)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(matches!(&rows[0], ServerMessage::Transcript {
        tseq:1, entry:TranscriptEntry::DecisionSubmitted { decision_id, summary }, ..
    } if decision_id == order.decision_id.as_str() && summary == "legal_random accepted action 2"));
    let enemy: SeatId = "commonwealth.commander".parse().unwrap();
    assert!(
        game.transcripts_after(Perspective::Seat(enemy), seat, 0, 512)
            .unwrap()
            .is_empty()
    );
    let accepted_hash = game.state_hash().unwrap();
    // A repeated identical pause produces only one factual system entry, also atomically.
    game.pause_seat_with_reason(seat, "scripted budget exhausted")
        .unwrap();
    game.pause_seat_with_reason(seat, "scripted budget exhausted")
        .unwrap();
    let reported = game
        .transcripts_after(Perspective::Seat(seat), seat, 0, 512)
        .unwrap();
    assert_eq!(reported.len(), 2);
    assert!(matches!(&reported[1], ServerMessage::Transcript {
        entry:TranscriptEntry::System { text }, ..
    } if text == "Scripted controller paused: scripted budget exhausted"));
    drop(game);
    let restored = Campaign::recover(&path, rules(), (), &pins()).unwrap();
    assert_eq!(restored.state_hash().unwrap(), accepted_hash);
    assert_eq!(
        restored
            .transcripts_after(Perspective::Seat(seat), seat, 0, 512)
            .unwrap(),
        reported
    );
}

struct Unenumerated {
    pass: bool,
    reject_pass: bool,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl Ruleset for Unenumerated {
    type State = State;
    type Content = ();
    fn profile_id(&self) -> &str {
        "tiny-v1"
    }
    fn advance(&self, c: &(), s: &mut State, cx: &mut Cx<'_>) -> Result<Progress, EngineError> {
        rules().advance(c, s, cx)
    }
    fn respond(
        &self,
        c: &(),
        s: &mut State,
        r: &DecisionResponse,
        cx: &mut Cx<'_>,
    ) -> Result<(), Rejection> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.reject_pass || !r.action.is_null() {
            return Err(Rejection::Illegal {
                message: "declared pass rejected by test ruleset".into(),
            });
        }
        let mut response = r.clone();
        response.action = json!(0);
        rules().respond(c, s, &response, cx)
    }
    fn pending(&self, c: &(), s: &State) -> Vec<DecisionRequest> {
        rules()
            .pending(c, s)
            .into_iter()
            .map(|mut r| {
                r.kind = "test.unenumerated".into();
                r.space = ActionSpace::new(ActionSchema::List {
                    item: Box::new(ActionSchema::Hex { among: None }),
                    min: 1,
                    max: 4096,
                });
                if self.pass {
                    r.space.pass = Some("done".into());
                }
                r
            })
            .collect()
    }
    fn observe(&self, c: &(), s: &State, p: Perspective) -> Value {
        rules().observe(c, s, p)
    }
    fn view(&self, c: &(), s: &State, p: Perspective) -> ViewState {
        rules().view(c, s, p)
    }
}
#[test]
fn unavailable_scripted_domain_passes_or_durably_pauses_without_retrying() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for (pass, reject_pass, expected_calls) in
        [(false, false, 0), (true, false, 1), (true, true, 1)]
    {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("domain.sqlite");
        let calls = Arc::new(AtomicUsize::new(0));
        let mut game = Campaign::create(
            &path,
            Unenumerated {
                pass,
                reject_pass,
                calls: calls.clone(),
            },
            (),
            Game {
                state: State::default(),
                rng: CampaignRng::from_seed([7; 32]).state(),
            },
            CampaignMeta {
                id: "domain".into(),
                scenario_id: "domain".into(),
                rules_profile: "tiny-v1".into(),
                title: "Domain".into(),
                seats: vec![],
            },
            pins(),
        )
        .unwrap();
        let seat = SeatId::all().next().unwrap();
        game.handover(
            seat,
            Some(ControllerInfo {
                kind: ControllerKind::Scripted,
                label: "legal-random".into(),
            }),
            json!({"mode":"legal_random"}),
        )
        .unwrap();
        game.advance().unwrap();
        let before = game.state_hash().unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        let rng_before: String = db
            .query_row("SELECT rng FROM campaign", [], |r| r.get(0))
            .unwrap();
        let step = game.step(&NoCandidates).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
        let succeeds = pass && !reject_pass;
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            if succeeds { 2 } else { 1 }
        );
        assert_eq!(
            rng_before,
            db.query_row("SELECT rng FROM campaign", [], |r| r.get::<_, String>(0))
                .unwrap()
        );
        let rows = game
            .transcripts_after(Perspective::Seat(seat), seat, 0, 10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(
            game.transcripts_after(
                Perspective::Side(SeatId::all().find(|s| s.side != seat.side).unwrap().side),
                seat,
                0,
                10
            )
            .unwrap()
            .is_empty()
        );
        if succeeds {
            assert!(matches!(step, Step::Responded {seat: s} if s == seat));
            assert!(matches!(
                &rows[0],
                ServerMessage::Transcript {
                    entry: TranscriptEntry::DecisionSubmitted { .. },
                    ..
                }
            ));
        } else {
            let Step::SeatPaused { error, .. } = step else {
                panic!("must pause")
            };
            assert!(error.contains(if pass {
                "declared pass rejected"
            } else {
                "no declared pass"
            }));
            assert_eq!(before, game.state_hash().unwrap());
            assert!(game.binding(seat).paused);
            assert!(matches!(
                &rows[0],
                ServerMessage::Transcript {
                    entry: TranscriptEntry::System { .. },
                    ..
                }
            ));
            assert!(matches!(game.step(&NoCandidates).unwrap(), Step::Idle));
            assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
            drop(game);
            let restored = Campaign::recover(
                &path,
                Unenumerated {
                    pass,
                    reject_pass,
                    calls: calls.clone(),
                },
                (),
                &pins(),
            )
            .unwrap();
            assert!(restored.binding(seat).paused);
            assert_eq!(before, restored.state_hash().unwrap());
            assert_eq!(
                rows,
                restored
                    .transcripts_after(Perspective::Seat(seat), seat, 0, 10)
                    .unwrap()
            );
        }
    }
}
