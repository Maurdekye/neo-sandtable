//! Oob982 controlled ALL-EMPTY fixture; real public DEV opening, window-only FULL response.
//! No RP inventory, content edits, manufactured requests or private Rules helpers.
use super::*;
use crate::{
    CampaignStatus,
    scripted::{self, Mode, NoCandidates},
};
use cna_core::{
    decision::{ActionSchema, DecisionResponse},
    engine::{Command, Ruleset, evaluate},
    event::EngineEvent,
    visibility::{Audience, Perspective},
};
use cna_protocol::{CampaignMeta, ControllerInfo, ControllerKind, ServerMessage, TranscriptEntry};
use rusqlite::{Connection, types::Value as SqlValue};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn bytes(value: &(impl Serialize + ?Sized)) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

struct Opened {
    content: CnaContent,
    before: Game<Cna>,
    game: Game<Cna>,
    events: Vec<EngineEvent>,
}

impl Opened {
    fn retain(&self, directory: &Path) {
        fs::write(
            directory.join("controlled-before.json"),
            bytes(&self.before),
        )
        .unwrap();
        fs::write(directory.join("actual-dev-opened.json"), bytes(&self.game)).unwrap();
        fs::write(
            directory.join("actual-opening-events.json"),
            bytes(&self.events),
        )
        .unwrap();
    }
}

fn real_entered(kind: &str) -> Opened {
    let (content, _) = inputs(&cna_content::repo_data_dir(), cna_rules::PROFILE_DEV).unwrap();
    let rules = Cna::dev();
    let mut before: Game<Cna> = Game {
        state: State::new(&content).unwrap(),
        rng: CampaignRng::from_seed([92; 32]).state(),
    };
    let anchor = match kind {
        "cna.replacements.training" => "opstage.organization.training",
        "cna.replacements.allocation" => "opstage.organization.reorganization",
        _ => panic!("fixture authority covers exactly the two named kinds"),
    };
    // Exact owner982 arrangement only; cycle and every non-cursor State byte stay fresh.
    let fresh = bytes(&before.state);
    let original_cursor = before.state.cursor.clone();
    before.state.cursor.game_turn = 3;
    before.state.cursor.op_stage = Some(1);
    before.state.cursor.block = cna_rules::seq::Block::OpStage;
    before.state.cursor.half = None;
    before.state.cursor.index = cna_rules::seq::OPSTAGE
        .iter()
        .position(|step| step.anchor == anchor)
        .unwrap();
    before.state.cursor.entered = false;
    let mut inverse = before.state.clone();
    inverse.cursor = original_cursor;
    assert_eq!(bytes(&inverse), fresh);
    assert!(rules.pending(&content, &before.state).is_empty());
    let frozen = bytes(&before);
    assert!(matches!(
        evaluate(&Cna::full(), &content, &before, &Command::Advance),
        Err(cna_core::engine::Rejection::Engine(
            cna_core::engine::EngineError::Unsupported { .. }
        ))
    ));
    assert_eq!(bytes(&before), frozen);
    let transition = evaluate(&rules, &content, &before, &Command::Advance).unwrap();
    assert_eq!(bytes(&before), frozen);
    assert_eq!(
        transition.progress,
        Some(cna_core::engine::Progress::AwaitingDecisions)
    );
    let requests = rules.pending(&content, &transition.game.state);
    let seats: Vec<_> = cna_protocol::Side::ALL
        .into_iter()
        .flat_map(|side| {
            [
                cna_protocol::Role::FrontLine,
                cna_protocol::Role::RearArea,
                cna_protocol::Role::Logistics,
            ]
            .into_iter()
            .map(move |role| SeatId::new(side, role))
        })
        .collect();
    assert_eq!(requests.iter().map(|r| r.seat).collect::<Vec<_>>(), seats);
    assert!(requests.iter().all(|r| r.kind == kind
        && r.space.pass.is_some()
        && matches!(&r.space.schema, ActionSchema::Choice { options } if options.is_empty())));
    assert_eq!(transition.game.rng, before.rng);
    Opened {
        content,
        before,
        game: transition.game,
        events: transition.events,
    }
}

fn campaign(path: &Path, profile: &str, game: Game<Cna>) -> (Campaign<Cna>, Pins) {
    let (content, pins) = inputs(&cna_content::repo_data_dir(), profile).unwrap();
    let campaign = Campaign::create(
        path,
        ruleset(profile).unwrap(),
        content,
        game,
        CampaignMeta {
            id: "replacement-actual-control".into(),
            scenario_id: "graziani".into(),
            rules_profile: profile.into(),
            title: "Entered replacement window".into(),
            seats: SeatId::all()
                .map(|seat| cna_protocol::SeatInfo {
                    id: seat.to_string(),
                    side: seat.side,
                    role: seat.role,
                    controller: None,
                    status: cna_protocol::SeatStatus::Idle,
                })
                .collect(),
        },
        pins.clone(),
    )
    .unwrap();
    (campaign, pins)
}

fn target(campaign: &Campaign<Cna>, kind: &str) -> cna_core::decision::DecisionRequest {
    campaign.pending().into_iter().find(|request| {
        request.kind == kind && request.space.pass.is_some()
            && matches!(&request.space.schema, ActionSchema::Choice { options } if options.is_empty())
    }).expect("genuine fixed empty window from actual handler")
}

fn bind(campaign: &mut Campaign<Cna>, seat: SeatId) -> u64 {
    campaign
        .handover(
            seat,
            Some(ControllerInfo {
                kind: ControllerKind::Scripted,
                label: "scripted:legal_random".into(),
            }),
            json!({"mode":"legal_random"}),
        )
        .unwrap()
        .controller_epoch
}

fn response(request: &cna_core::decision::DecisionRequest, epoch: u64) -> DecisionResponse {
    scripted::answer(request, epoch, Mode::LegalRandom, 0, &NoCandidates).unwrap()
}

fn canonical_pass(request: &cna_core::decision::DecisionRequest, epoch: u64) -> DecisionResponse {
    DecisionResponse {
        decision_id: request.id.clone(),
        seat: request.seat,
        controller_epoch: epoch,
        decision_revision: request.revision,
        idempotency_key: format!("scripted:{}:{epoch}:{}:0", request.id, request.revision),
        action: Value::Null,
        public_explanation: None,
    }
}

fn rows(path: &Path) -> BTreeMap<String, Vec<Vec<SqlValue>>> {
    checked_rows(path).unwrap()
}

fn checked_rows(path: &Path) -> rusqlite::Result<BTreeMap<String, Vec<Vec<SqlValue>>>> {
    let db = Connection::open(path)?;
    let names: Vec<String> = db.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
    )?.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
    names
        .into_iter()
        .map(|name| {
            let mut query = db.prepare(&format!("SELECT * FROM \"{name}\""))?;
            let count = query.column_count();
            let mut rows = query
                .query_map([], |row| {
                    (0..count)
                        .map(|index| row.get(index))
                        .collect::<Result<Vec<SqlValue>, _>>()
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows.sort_by_key(|row| format!("{row:?}"));
            Ok((name, rows))
        })
        .collect()
}

/// Cases: land:17.33, land:20.41, land:20.43, land:20.49
#[test]
fn actual_empty_fixed_schedule_waits_for_every_recipient_before_dev_finish() {
    for kind in ["cna.replacements.training", "cna.replacements.allocation"] {
        let fixture = real_entered(kind);
        let original_anchor = fixture.game.state.cursor.anchor();
        let mut game = fixture.game.clone();
        let requests = Cna::dev().pending(&fixture.content, &game.state);
        for (index, request) in requests.iter().enumerate() {
            let waiting =
                evaluate(&Cna::dev(), &fixture.content, &game, &Command::Advance).unwrap();
            assert_eq!(
                waiting.progress,
                Some(cna_core::engine::Progress::AwaitingDecisions)
            );
            assert_eq!(bytes(&waiting.game), bytes(&game));
            assert!(waiting.events.is_empty());
            let transition = evaluate(
                &Cna::dev(),
                &fixture.content,
                &game,
                &Command::Respond(canonical_pass(request, 1)),
            )
            .unwrap();
            game = transition.game;
            let remaining = Cna::dev().pending(&fixture.content, &game.state);
            assert_eq!(bytes(&remaining), bytes(&requests[index + 1..]));
            assert_eq!(game.state.cursor.anchor(), original_anchor);
            assert_eq!(game.rng, fixture.game.rng);
        }
        let finished = evaluate(&Cna::dev(), &fixture.content, &game, &Command::Advance).unwrap();
        assert_ne!(finished.game.state.cursor.anchor(), original_anchor);
    }
}

/// Cases: land:3.6, land:17.33, land:20.41, land:20.43, land:20.49
#[tokio::test]
async fn replacement_real_empty_windows_use_original_writer_and_match_direct_canonical_pass() {
    for kind in ["cna.replacements.training", "cna.replacements.allocation"] {
        let fixture = real_entered(kind);
        let (content, entered) = (&fixture.content, &fixture.game);
        for selected in Cna::dev()
            .pending(content, &entered.state)
            .iter()
            .map(|request| request.seat)
        {
            for profile in [cna_rules::PROFILE_DEV, cna_rules::PROFILE_FULL] {
                let directory = tempfile::tempdir().unwrap();
                fixture.retain(directory.path());
                let path = directory.path().join("actor.sqlite");
                let (mut current, pins) = campaign(&path, profile, entered.clone());
                let request = current
                    .pending()
                    .into_iter()
                    .find(|request| request.seat == selected && request.kind == kind)
                    .unwrap();
                let epoch = bind(&mut current, request.seat);
                let answer = response(&request, epoch);
                assert_eq!(bytes(&answer), bytes(&canonical_pass(&request, epoch)));
                let expected = evaluate(
                    &ruleset(profile).unwrap(),
                    content,
                    entered,
                    &Command::Respond(canonical_pass(&request, epoch)),
                )
                .unwrap();
                current.set_paused(true).unwrap();
                let before_bindings: Vec<_> = SeatId::all()
                    .map(|seat| current.binding(seat).clone())
                    .collect();
                let handle = CampaignHandle::spawn_with_policy(
                    current,
                    &path,
                    movement_policy_with_profile(profile == cna_rules::PROFILE_FULL),
                )
                .unwrap();
                let mut live: Vec<_> = Perspective::all()
                    .map(|p| (p, handle.header(p).seq, handle.subscribe(p)))
                    .collect();
                // With all other seats unbound, the first real response leaves other
                // fixed windows pending. No next Advance can run before the pause.
                let mut watch = handle.watch_seat(request.seat);
                let run = handle.pause(false).await;
                let consumed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    watch
                        .wait_for(|state| !state.pending.iter().any(|r| r.id == request.id))
                        .await
                        .map(|_| ())
                })
                .await;
                let pause = handle.pause(true).await;
                let joined = handle.shutdown().await;
                // Original writer is actually joined before any assertion or DB failure.
                joined.unwrap();
                run.unwrap();
                consumed.unwrap().unwrap();
                pause.unwrap();
                let snapshots: Vec<_> = Perspective::all()
                    .map(|p| (p, handle.projection(p)))
                    .collect();
                let bindings: Vec<_> = SeatId::all()
                    .map(|seat| handle.seat(seat).binding)
                    .collect();
                let reader = handle.replay();
                assert_eq!(bindings, before_bindings);
                let recovery_content =
                    CnaContent::load(&cna_content::repo_data_dir(), content.scenario_key())
                        .unwrap();
                let recovered =
                    Campaign::recover(&path, ruleset(profile).unwrap(), recovery_content, &pins)
                        .unwrap();
                assert_eq!(bytes(recovered.game.as_ref()), bytes(&expected.game));
                assert_eq!(recovered.status(), &CampaignStatus::Paused);
                let db = Connection::open(&path).unwrap();
                let commands: Vec<String> = db
                    .prepare("SELECT command FROM commands ORDER BY revision")
                    .unwrap()
                    .query_map([], |row| row.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                assert_eq!(commands.len(), 1);
                assert_eq!(
                    serde_json::from_str::<Command>(&commands[0]).unwrap(),
                    Command::Respond(answer)
                );
                let persisted_events: Vec<EngineEvent> = db
                    .prepare("SELECT payload FROM events ORDER BY event_id")
                    .unwrap()
                    .query_map([], |row| row.get::<_, String>(0))
                    .unwrap()
                    .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
                    .collect();
                assert_eq!(bytes(&persisted_events), bytes(&expected.events));
                let public = |events: &[EngineEvent]| {
                    bytes(
                        &events
                            .iter()
                            .filter(|event| event.audience == Audience::Public)
                            .collect::<Vec<_>>(),
                    )
                };
                assert_eq!(public(&persisted_events), public(&expected.events));
                for (p, projection) in snapshots {
                    assert_eq!(bytes(&projection.view), bytes(&recovered.view(p).unwrap()));
                    let (_, from_seq, receiver) = live
                        .iter_mut()
                        .find(|(perspective, _, _)| *perspective == p)
                        .unwrap();
                    let mut messages = vec![];
                    while let Ok(message) = receiver.try_recv() {
                        messages.push(message);
                    }
                    let live_events: Vec<_> = messages
                        .iter()
                        .filter(|message| matches!(message, ServerMessage::Event { .. }))
                        .collect();
                    let replayed = reader.events(p, *from_seq, projection.seq).unwrap();
                    assert_eq!(bytes(&live_events), bytes(&replayed));
                    let transcripts = reader.transcripts(p, request.seat, 0).unwrap();
                    let live_transcripts: Vec<_> = messages
                        .iter()
                        .filter(|message| matches!(message, ServerMessage::Transcript { .. }))
                        .collect();
                    assert_eq!(bytes(&live_transcripts), bytes(&transcripts));
                    if p.can_see(&Audience::Seat(request.seat)) {
                        assert_eq!(transcripts.len(), 1);
                        assert!(matches!(&transcripts[0], ServerMessage::Transcript {
                        entry: TranscriptEntry::DecisionSubmitted { decision_id, summary }, ..
                    } if decision_id.as_str() == request.id.as_str() && summary.as_str() == "legal_random accepted action null"));
                    } else {
                        assert!(transcripts.is_empty());
                    }
                }
                let pending = recovered.pending();
                assert!(!pending.iter().any(|pending| pending.id == request.id));
                assert_eq!(
                    pending.len() + 1,
                    Cna::dev().pending(content, &entered.state).len()
                );
            }
        }
    }
}

/// Cases: land:17.33, land:20.41, land:20.43, land:20.49
#[test]
fn replacement_real_pass_persistence_duplicate_conflict_and_errors_keep_complete_state() {
    for kind in ["cna.replacements.training", "cna.replacements.allocation"] {
        let fixture = real_entered(kind);
        let (content, entered) = (&fixture.content, &fixture.game);
        for profile in [cna_rules::PROFILE_DEV, cna_rules::PROFILE_FULL] {
            let directory = tempfile::tempdir().unwrap();
            fixture.retain(directory.path());
            let path = directory.path().join("submit.sqlite");
            let (mut current, pins) = campaign(&path, profile, entered.clone());
            let request = target(&current, kind);
            let epoch = bind(&mut current, request.seat);
            let answer = response(&request, epoch);
            let before = rows(&path);
            let mut stale_epoch = answer.clone();
            stale_epoch.controller_epoch += 1;
            assert!(matches!(
                current.submit(stale_epoch),
                Err(Error::StaleEpoch)
            ));
            let mut stale_revision = answer.clone();
            stale_revision.decision_revision += 1;
            assert!(
                matches!(current.submit(stale_revision), Err(Error::Rejected(cna_core::engine::Rejection::StaleRevision { expected, got })) if expected == request.revision && got == request.revision + 1)
            );
            let mut wrong = answer.clone();
            wrong.seat = SeatId::all().find(|seat| *seat != request.seat).unwrap();
            wrong.controller_epoch = current.binding(wrong.seat).controller_epoch;
            assert!(
                matches!(current.submit(wrong), Err(Error::Rejected(cna_core::engine::Rejection::UnknownDecision { decision_id })) if decision_id == request.id)
            );
            let mut fabricated = answer.clone();
            fabricated.action = json!({"selected":"not-an-entitled-counter"});
            fabricated.idempotency_key.push_str(":invalid-selection");
            let expected_rejection = match evaluate(
                &ruleset(profile).unwrap(),
                content,
                entered,
                &Command::Respond(fabricated.clone()),
            ) {
                Err(rejection @ cna_core::engine::Rejection::Illegal { .. }) => rejection,
                _ => panic!("empty actual domain must reject a non-pass selection"),
            };
            match current.submit(fabricated) {
                Err(Error::Rejected(rejection)) => {
                    assert_eq!(bytes(&rejection), bytes(&expected_rejection))
                }
                other => panic!("exact empty-domain rejection required: {other:?}"),
            }
            assert_eq!(rows(&path), before);
            assert_eq!(bytes(current.game.as_ref()), bytes(&entered));
            let expected = evaluate(
                &ruleset(profile).unwrap(),
                content,
                entered,
                &Command::Respond(answer.clone()),
            )
            .unwrap();
            assert!(!current.submit(answer.clone()).unwrap().duplicate);
            let accepted = rows(&path);
            assert!(current.submit(answer.clone()).unwrap().duplicate);
            assert_eq!(rows(&path), accepted);
            let mut conflict = answer;
            conflict.action = Value::Bool(false);
            assert!(matches!(
                current.submit(conflict),
                Err(Error::IdempotencyConflict)
            ));
            assert_eq!(rows(&path), accepted);
            assert_eq!(bytes(current.game.as_ref()), bytes(&expected.game));
            drop(current);
            let recovery_content =
                CnaContent::load(&cna_content::repo_data_dir(), content.scenario_key()).unwrap();
            let recovered =
                Campaign::recover(&path, ruleset(profile).unwrap(), recovery_content, &pins)
                    .unwrap();
            assert_eq!(rows(&path), accepted);
            assert_eq!(bytes(recovered.game.as_ref()), bytes(&expected.game));
        }
    }
}

/// Cases: land:17.33, land:20.41, land:20.43, land:20.49
#[test]
fn replacement_real_pass_transaction_failure_commits_no_command_transcript_or_memory() {
    for kind in ["cna.replacements.training", "cna.replacements.allocation"] {
        let fixture = real_entered(kind);
        let (content, entered) = (&fixture.content, &fixture.game);
        let directory = tempfile::tempdir().unwrap();
        fixture.retain(directory.path());
        let path = directory.path().join("failure.sqlite");
        let (mut current, pins) = campaign(&path, cna_rules::PROFILE_FULL, entered.clone());
        let request = target(&current, kind);
        let epoch = bind(&mut current, request.seat);
        current
            .pause_seat_with_reason(request.seat, "test pause")
            .unwrap();
        let before_pause = rows(&path);
        assert!(
            matches!(current.submit(response(&request, epoch)), Err(Error::Invalid(message)) if message == "seat is paused; handover to retry")
        );
        assert_eq!(rows(&path), before_pause);
        let epoch = bind(&mut current, request.seat);
        Connection::open(&path).unwrap().execute_batch(
            "CREATE TRIGGER deny_replacement_command BEFORE INSERT ON commands BEGIN SELECT RAISE(ABORT,'replacement transaction failure'); END;"
        ).unwrap();
        let before = rows(&path);
        let bindings: Vec<_> = SeatId::all()
            .map(|seat| current.binding(seat).clone())
            .collect();
        assert!(matches!(
            current.submit(response(&request, epoch)),
            Err(Error::Storage(_))
        ));
        assert_eq!(rows(&path), before);
        assert_eq!(bytes(current.game.as_ref()), bytes(&entered));
        assert_eq!(
            SeatId::all()
                .map(|seat| current.binding(seat).clone())
                .collect::<Vec<_>>(),
            bindings
        );
        drop(current);
        let recovery_content =
            CnaContent::load(&cna_content::repo_data_dir(), content.scenario_key()).unwrap();
        let recovered = Campaign::recover(&path, Cna::full(), recovery_content, &pins).unwrap();
        assert_eq!(rows(&path), before);
        assert_eq!(bytes(recovered.game.as_ref()), bytes(&entered));
    }
}

#[path = "replacement_surface_controls.rs"]
mod surfaces;
