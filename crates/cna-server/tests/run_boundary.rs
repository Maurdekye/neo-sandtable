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
    CampaignMeta, ControllerInfo, ControllerKind, GameEvent, PendingDecision, ViewState,
};
use cna_server::{
    Campaign, CampaignStatus, Error, Pins, RunBoundary,
    actor::CampaignHandle,
    scripted::{NoCandidates, Step},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    tick: u16,
    pending: bool,
    rolls: Vec<u8>,
}
struct Windows;
fn seat() -> SeatId {
    "axis.commander".parse().unwrap()
}
impl Ruleset for Windows {
    type Content = ();
    type State = State;
    fn profile_id(&self) -> &str {
        "boundary-test"
    }
    fn clock(&self, _: &(), s: &State) -> cna_protocol::Clock {
        cna_protocol::Clock {
            game_turn: 1 + s.tick / 5,
            op_stage: match s.tick % 5 {
                n @ 1..=3 => Some(n as u8),
                _ => None,
            },
            date: "1940-09-15".into(),
            stage: "test".into(),
            phase: "orders".into(),
            segment: None,
            step: None,
            phasing: None,
        }
    }
    fn advance(&self, _: &(), s: &mut State, cx: &mut Cx<'_>) -> Result<Progress, EngineError> {
        s.tick += 1;
        let die = cx.rng.d6().value();
        s.rolls.push(die);
        cx.emit(EngineEvent::public(GameEvent::DiceRolled {
            purpose: "boundary preview".into(),
            dice: vec![die],
            reading: None,
            rule: None,
        }));
        s.pending = s.tick < 7;
        if !s.pending {
            return Ok(Progress::Finished {
                summary: "done".into(),
            });
        }
        let d = self.pending(&(), s).remove(0);
        cx.emit(EngineEvent::new(
            Audience::Seat(seat()),
            GameEvent::DecisionOpened {
                decision: PendingDecision {
                    id: d.id.to_string(),
                    seat: seat().to_string(),
                    kind: d.kind,
                    summary: d.summary,
                    opened_seq: 0,
                    rules: vec![],
                    space: None,
                },
            },
        ));
        Ok(Progress::AwaitingDecisions)
    }
    fn respond(
        &self,
        _: &(),
        s: &mut State,
        r: &DecisionResponse,
        cx: &mut Cx<'_>,
    ) -> Result<(), Rejection> {
        if r.action != json!(true) {
            return Err(Rejection::Illegal {
                message: "need true".into(),
            });
        }
        s.pending = false;
        cx.emit(EngineEvent::new(
            Audience::Seat(seat()),
            GameEvent::DecisionResolved {
                decision_id: r.decision_id.to_string(),
                seat: seat().to_string(),
                summary: "recorded".into(),
                explanation: None,
            },
        ));
        Ok(())
    }
    fn pending(&self, _: &(), s: &State) -> Vec<DecisionRequest> {
        if !s.pending {
            return vec![];
        }
        vec![DecisionRequest {
            id: format!("own-{}", s.tick).as_str().into(),
            seat: seat(),
            kind: "test.plan".into(),
            revision: 1,
            clock: Clock::start(1 + s.tick / 5, Anchor::new("orders")),
            summary: "plan".into(),
            rules: vec![],
            trigger: Trigger::Scheduled,
            secrecy: Secrecy::SecretSimultaneous,
            space: ActionSpace::new(ActionSchema::Bool),
        }]
    }
    fn observe(&self, _: &(), s: &State, _: Perspective) -> Value {
        json!({"tick":s.tick})
    }
    fn view(&self, _: &(), s: &State, p: Perspective) -> ViewState {
        ViewState {
            clock: self.clock(&(), s),
            stacks: vec![],
            units: BTreeMap::new(),
            markers: vec![],
            pending: self
                .pending(&(), s)
                .into_iter()
                .filter(|d| p.can_see(&Audience::Seat(d.seat)))
                .map(|d| PendingDecision {
                    id: d.id.to_string(),
                    seat: d.seat.to_string(),
                    kind: d.kind,
                    summary: d.summary,
                    opened_seq: 0,
                    rules: vec![],
                    space: None,
                })
                .collect(),
        }
    }
}
fn pins() -> Pins {
    Pins {
        rules_profile: "boundary-test".into(),
        content_hash: "test".into(),
        engine_version: "test".into(),
    }
}
fn create(path: &std::path::Path) -> Campaign<Windows> {
    Campaign::create(
        path,
        Windows,
        (),
        Game {
            state: State::default(),
            rng: CampaignRng::from_seed([7; 32]).state(),
        },
        CampaignMeta {
            id: "boundary".into(),
            scenario_id: "test".into(),
            rules_profile: "boundary-test".into(),
            title: "test".into(),
            seats: vec![],
        },
        pins(),
    )
    .unwrap()
}
fn respond(c: &mut Campaign<Windows>) {
    let d = c.pending().remove(0);
    c.submit(DecisionResponse {
        decision_id: d.id.clone(),
        seat: d.seat,
        controller_epoch: 0,
        decision_revision: d.revision,
        idempotency_key: d.id.to_string(),
        action: json!(true),
        public_explanation: None,
    })
    .unwrap();
}
fn drive_to(c: &mut Campaign<Windows>, tick: u16) {
    while c.observe(seat())["tick"].as_u64().unwrap() < u64::from(tick) {
        if !c.pending().is_empty() {
            respond(c);
        }
        c.advance().unwrap();
    }
}
#[test]
fn stage_and_turn_boundaries_discard_dice_events_and_recover_then_resume_identically() {
    for (boundary, last_tick) in [
        (
            RunBoundary {
                game_turn: 1,
                op_stage: Some(1),
            },
            1,
        ),
        (
            RunBoundary {
                game_turn: 1,
                op_stage: None,
            },
            4,
        ),
        (
            RunBoundary {
                game_turn: 1,
                op_stage: Some(3),
            },
            4,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bounded.sqlite");
        let mut bounded = create(&path);
        let mut unbounded = create(&dir.path().join("unbounded.sqlite"));
        bounded.set_run_boundary(Some(boundary)).unwrap();
        drive_to(&mut bounded, last_tick);
        drive_to(&mut unbounded, last_tick);
        respond(&mut bounded);
        respond(&mut unbounded);
        let hash = bounded.state_hash().unwrap();
        let views: Vec<_> = Perspective::all()
            .map(|p| bounded.view(p).unwrap())
            .collect();
        let streams: Vec<_> = Perspective::all()
            .map(|p| bounded.events_after(p, 0, 100).unwrap())
            .collect();
        let bindings: Vec<_> = SeatId::all().map(|s| bounded.binding(s).clone()).collect();
        assert!(matches!(bounded.advance(), Err(Error::RunBoundaryReached)));
        assert_eq!(bounded.status(), &CampaignStatus::Paused);
        assert_eq!(bounded.state_hash().unwrap(), hash);
        let db = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            u64::from(last_tick) * 2
        );
        drop(db);
        assert_eq!(
            SeatId::all()
                .map(|s| bounded.binding(s).clone())
                .collect::<Vec<_>>(),
            bindings
        );
        drop(bounded);
        let mut bounded = Campaign::recover(&path, Windows, (), &pins()).unwrap();
        assert_eq!(bounded.run_boundary(), Some(boundary));
        assert_eq!(bounded.status(), &CampaignStatus::Paused);
        assert_eq!(bounded.state_hash().unwrap(), hash);
        assert_eq!(
            Perspective::all()
                .map(|p| bounded.view(p).unwrap())
                .collect::<Vec<_>>(),
            views
        );
        assert_eq!(
            Perspective::all()
                .map(|p| bounded.events_after(p, 0, 100).unwrap())
                .collect::<Vec<_>>(),
            streams
        );
        // Raising/clearing is not an implicit resume.
        bounded.set_run_boundary(None).unwrap();
        assert_eq!(bounded.status(), &CampaignStatus::Paused);
        assert!(matches!(bounded.advance(), Err(Error::NotRunning)));
        bounded.set_paused(false).unwrap();
        bounded.advance().unwrap();
        unbounded.advance().unwrap();
        assert_eq!(
            bounded.state_hash().unwrap(),
            unbounded.state_hash().unwrap()
        );
        for p in Perspective::all() {
            assert_eq!(
                bounded.events_after(p, 0, 100).unwrap(),
                unbounded.events_after(p, 0, 100).unwrap()
            );
            assert_eq!(bounded.view(p).unwrap(), unbounded.view(p).unwrap());
        }
    }
}
#[test]
fn paused_answers_are_plan_only_and_ignore_the_future_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = create(&dir.path().join("a.sqlite"));
    let mut b = create(&dir.path().join("b.sqlite"));
    a.advance().unwrap();
    b.advance().unwrap();
    a.set_run_boundary(Some(RunBoundary {
        game_turn: 1,
        op_stage: Some(1),
    }))
    .unwrap();
    a.set_paused(true).unwrap();
    b.set_paused(true).unwrap();
    let d = a.pending().remove(0);
    let answer = DecisionResponse {
        decision_id: d.id.clone(),
        seat: seat(),
        controller_epoch: 0,
        decision_revision: 1,
        idempotency_key: d.id.to_string(),
        action: json!(true),
        public_explanation: None,
    };
    a.validate(&answer).unwrap();
    b.validate(&answer).unwrap();
    a.submit(answer.clone()).unwrap();
    b.submit(answer.clone()).unwrap();
    assert!(a.submit(answer).unwrap().duplicate);
    assert_eq!(a.status(), &CampaignStatus::Paused);
    assert_eq!(a.state_hash().unwrap(), b.state_hash().unwrap());
    for p in Perspective::all() {
        assert_eq!(
            a.events_after(p, 0, 100).unwrap(),
            b.events_after(p, 0, 100).unwrap()
        );
    }
    assert!(matches!(a.advance(), Err(Error::NotRunning)));
}
#[tokio::test]
async fn automatic_writer_stops_healthily_and_control_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("actor.sqlite");
    let mut campaign = create(&path);
    drive_to(&mut campaign, 1);
    // The pending answer is submitted by the actual scripted controller, then its
    // automatically queued Advance is fenced by the campaign control.
    campaign
        .handover(
            seat(),
            Some(ControllerInfo {
                kind: ControllerKind::Scripted,
                label: "legal".into(),
            }),
            json!({"mode":"legal_random"}),
        )
        .unwrap();
    let mut expected = create(&dir.path().join("expected.sqlite"));
    drive_to(&mut expected, 1);
    respond(&mut expected);
    campaign
        .set_run_boundary(Some(RunBoundary {
            game_turn: 1,
            op_stage: Some(1),
        }))
        .unwrap();
    let hash = expected.state_hash().unwrap();
    // The real writer previews the next automatic advance and parks without a seat failure.
    let handle = CampaignHandle::spawn(campaign, &path, None).unwrap();
    let mut status = handle.watch_status();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if *status.borrow_and_update() == CampaignStatus::Paused {
                break;
            }
            status.changed().await.unwrap();
        }
    })
    .await;
    let boundary = handle.run_boundary().await.unwrap();
    let healthy = SeatId::all()
        .all(|s| !handle.seat(s).binding.paused && handle.seat(s).binding.failure.is_none());
    handle.shutdown().await.unwrap();
    result.unwrap();
    assert!(healthy);
    assert_eq!(
        boundary,
        Some(RunBoundary {
            game_turn: 1,
            op_stage: Some(1)
        })
    );
    let mut restored = Campaign::recover(&path, Windows, (), &pins()).unwrap();
    assert_eq!(restored.state_hash().unwrap(), hash);
    restored
        .set_run_boundary(Some(RunBoundary {
            game_turn: 2,
            op_stage: None,
        }))
        .unwrap();
    assert_eq!(restored.status(), &CampaignStatus::Paused);
    restored.set_paused(false).unwrap();
    assert!(matches!(
        restored.step(&NoCandidates).unwrap(),
        Step::Advanced
    ));
    assert_eq!(restored.observe(seat())["tick"], 2);
}

#[test]
fn legacy_database_defaults_to_unbounded_and_bad_controls_are_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite");
    drop(create(&path));
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("DROP TABLE run_control", []).unwrap();
    drop(db);
    let mut c = Campaign::recover(&path, Windows, (), &pins()).unwrap();
    assert_eq!(c.run_boundary(), None);
    let before = c.state_hash().unwrap();
    for boundary in [
        RunBoundary {
            game_turn: 0,
            op_stage: None,
        },
        RunBoundary {
            game_turn: 1,
            op_stage: Some(0),
        },
        RunBoundary {
            game_turn: 1,
            op_stage: Some(4),
        },
    ] {
        assert!(matches!(
            c.set_run_boundary(Some(boundary)),
            Err(Error::Invalid(_))
        ));
        assert_eq!(c.run_boundary(), None);
        assert_eq!(c.state_hash().unwrap(), before);
        assert_eq!(c.status(), &CampaignStatus::Running);
    }
    c.advance().unwrap();
}
