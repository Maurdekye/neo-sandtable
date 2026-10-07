use cna_core::{
    ids::{SeatId, Side},
    visibility::Perspective,
};
use cna_protocol::{ClientMessage, ServerMessage, TranscriptEntry};
use cna_seats::{
    game::{GameBackend, SubmitRequest},
    memory::{SeatMemory, WriteMode},
    transcript::TranscriptStore,
};
use cna_server::{
    CampaignStatus,
    actor::{CampaignHandle, VIEWER_BUFFER},
    http::{App, CreateRequest},
    sandbox,
    scripted::{self, Mode, NoCandidates},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

fn data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data")
}
fn request(mode: &str, paused: bool) -> CreateRequest {
    CreateRequest {
        kind: cna_server::http::CampaignKind::Sandbox,
        rules_profile: "sandbox-v1".into(),
        seed: [7; 32],
        title: "Integration".into(),
        paused,
        controller: mode.into(),
    }
}
async fn finished(handle: &CampaignHandle) {
    let mut status = handle.watch_status();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if matches!(*status.borrow_and_update(), CampaignStatus::Finished { .. }) {
                break;
            }
            status.changed().await.expect("actor closed before finish");
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "did not finish: {:?}; seats {:?}",
            handle.status(),
            SeatId::all().map(|s| handle.seat(s)).collect::<Vec<_>>()
        )
    });
}
async fn pending(handle: &CampaignHandle) -> cna_core::decision::DecisionRequest {
    pending_for(handle, "axis.commander".parse().unwrap()).await
}
async fn pending_for(handle: &CampaignHandle, s: SeatId) -> cna_core::decision::DecisionRequest {
    let mut state = handle.watch_seat(s);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(d) = state.borrow_and_update().pending.first().cloned() {
                return d;
            }
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}
type Socket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
async fn read(socket: &mut Socket) -> ServerMessage {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("socket timeout")
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap();
        }
    }
}
async fn subscribe(socket: &mut Socket, p: &str, from: Option<u64>) {
    socket
        .send(Message::Text(
            serde_json::to_string(&ClientMessage::Subscribe {
                perspective: p.into(),
                from_seq: from,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
}
struct Server {
    app: App,
    task: tokio::task::JoinHandle<()>,
    url: String,
}
impl Server {
    async fn new(directory: &Path, handle: Option<CampaignHandle>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = App::new(
            directory.to_owned(),
            port,
            Arc::new(|r, dir| sandbox::create(dir, &data(), r)),
        );
        if let Some(handle) = handle {
            app.register(handle);
        }
        let router = app.router(&directory.join("missing-dist"));
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            app,
            task,
            url: format!("http://127.0.0.1:{port}"),
        }
    }
    fn client(&self) -> reqwest::Client {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", self.app.operator_token())
                .parse()
                .unwrap(),
        );
        reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap()
    }
    async fn stop(self) {
        self.app.shutdown().await;
        self.task.abort();
        let _ = self.task.await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_finishes_while_unread_viewer_lags_then_recovery_matches() {
    let dir = tempfile::tempdir().unwrap();
    let handle = sandbox::create(dir.path(), &data(), request("legal_random", true)).unwrap();
    let id = handle.projection(Perspective::Operator).meta.id;
    let mut slow = handle.subscribe(Perspective::Operator);
    let axis: SeatId = "axis.commander".parse().unwrap();
    let enemy: SeatId = "commonwealth.air".parse().unwrap();
    let mut hidden = handle.subscribe(Perspective::Seat(axis));
    let mut my_seat = handle.watch_seat(axis);
    my_seat.borrow_and_update();
    // More than a full bounded viewer buffer is captured before anybody reads it.
    for n in 0..(VIEWER_BUFFER + 5) {
        handle
            .append(
                enemy,
                "2026-10-06T11:00:00Z".into(),
                TranscriptEntry::System {
                    text: format!("{n}"),
                },
            )
            .await
            .unwrap();
    }
    assert!(matches!(
        slow.recv().await,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
    ));
    assert_eq!(handle.projection(Perspective::Seat(axis)).seq, 0);
    assert!(matches!(
        hidden.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(!my_seat.has_changed().unwrap());
    handle.pause(false).await.unwrap();
    finished(&handle).await;
    let final_view = handle.projection(Perspective::Operator);
    assert!(final_view.seq > 10);
    handle.shutdown().await.unwrap();
    let restored = sandbox::recover(&dir.path().join(format!("{id}.sqlite")), &data()).unwrap();
    assert_eq!(final_view, restored.projection(Perspective::Operator));
    assert_eq!(handle.status(), restored.status());
    restored.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seat_bridge_is_durable_filtered_idempotent_and_reissues_after_handover() {
    let dir = tempfile::tempdir().unwrap();
    let handle = sandbox::create(dir.path(), &data(), request("human", false)).unwrap();
    let d = pending(&handle).await;
    let seat = d.seat;
    let mut mine = handle.watch_seat(seat);
    mine.borrow_and_update();
    let binding = handle.handover(seat, None, Value::Null).await.unwrap();
    mine.changed().await.unwrap();
    assert_eq!(mine.borrow_and_update().pending[0].id, d.id);
    assert_eq!(
        mine.borrow().binding.controller_epoch,
        binding.controller_epoch
    );
    let candidate = scripted::answer(
        &d,
        binding.controller_epoch,
        Mode::PassWhenPossible,
        0,
        &NoCandidates,
    )
    .unwrap();
    handle
        .validate_action(seat, d.id.as_str(), candidate.action.clone())
        .await
        .unwrap();
    let req = SubmitRequest {
        decision_id: d.id.to_string(),
        epoch: binding.controller_epoch,
        revision: Some(d.revision),
        idempotency_key: candidate.idempotency_key,
        action: candidate.action,
        public_explanation: None,
    };
    let accepted = GameBackend::submit(&handle, seat, req.clone())
        .await
        .unwrap();
    assert!(!accepted.duplicate);
    let duplicate = GameBackend::submit(&handle, seat, req).await.unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(accepted.result, duplicate.result);
    assert_eq!(GameBackend::game_seq(&handle).await, 0);
    handle
        .notebook_write(seat, WriteMode::Replace, "remember")
        .await
        .unwrap();
    handle
        .notebook_write(seat, WriteMode::Append, " this")
        .await
        .unwrap();
    assert_eq!(handle.notebook_read(seat).await, "remember this");
    assert_eq!(
        SeatMemory::message_team(&handle, seat, "orders")
            .await
            .unwrap(),
        4
    );
    let same: SeatId = "axis.air".parse().unwrap();
    let enemy: SeatId = "commonwealth.air".parse().unwrap();
    assert_eq!(handle.read_messages(same, 0).await[0].n, 1);
    assert!(handle.read_messages(enemy, 0).await.is_empty());
    SeatMemory::message_team(&handle, enemy, "enemy private")
        .await
        .unwrap();
    SeatMemory::message_team(&handle, seat, "next")
        .await
        .unwrap();
    assert_eq!(handle.read_messages(same, 1).await[0].n, 2);
    handle.mark_failure(seat, "budget exhausted").await.unwrap();
    let id = handle.projection(Perspective::Operator).meta.id;
    handle.shutdown().await.unwrap();
    let restored = sandbox::recover(&dir.path().join(format!("{id}.sqlite")), &data()).unwrap();
    assert_eq!(restored.notebook_read(seat).await, "remember this");
    assert!(restored.seat(seat).binding.paused);
    assert_eq!(
        restored.seat(seat).binding.failure.as_deref(),
        Some("budget exhausted")
    );
    restored.handover(seat, None, Value::Null).await.unwrap();
    assert!(!restored.seat(seat).binding.paused);
    assert_eq!(restored.read_messages(same, 0).await.len(), 2);
    restored.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_snapshot_live_resume_switch_and_ahead_cursor_resync() {
    let dir = tempfile::tempdir().unwrap();
    let handle = sandbox::create(dir.path(), &data(), request("human", true)).unwrap();
    let id = handle.projection(Perspective::Operator).meta.id;
    let server = Server::new(dir.path(), Some(handle.clone())).await;
    let address = format!(
        "{}/api/campaigns/{id}/stream?cap={}",
        server.url.replace("http:", "ws:"),
        server.app.operator_token()
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(&address).await.unwrap();
    subscribe(&mut socket, "side:axis", None).await;
    assert!(
        matches!(read(&mut socket).await,ServerMessage::Hello{perspective,..} if perspective=="side:axis")
    );
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Snapshot { seq: 0, .. }
    ));
    handle.pause(false).await.unwrap();
    let d = pending(&handle).await;
    let mut last = 0;
    while last < handle.projection(Perspective::Side(Side::Axis)).seq {
        let ServerMessage::Event { seq, .. } = read(&mut socket).await else {
            panic!("not an event")
        };
        assert_eq!(seq, last + 1);
        last = seq;
    }
    // A subscription consumed in the live loop must be honored on the same socket.
    subscribe(&mut socket, "side:commonwealth", None).await;
    assert!(
        matches!(read(&mut socket).await,ServerMessage::Hello{perspective,..} if perspective=="side:commonwealth")
    );
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Snapshot { .. }
    ));
    let mut enemy_seq = handle.projection(Perspective::Side(Side::Commonwealth)).seq;
    let mut response = scripted::answer(
        &d,
        handle.seat(d.seat).binding.controller_epoch,
        Mode::PassWhenPossible,
        0,
        &NoCandidates,
    )
    .unwrap();
    response.action = serde_json::json!("first");
    handle.submit(response).await.unwrap();
    pending_for(&handle, "axis.logistics".parse().unwrap()).await;
    let target_enemy_seq = handle.projection(Perspective::Side(Side::Commonwealth)).seq;
    while enemy_seq < target_enemy_seq {
        let ServerMessage::Event { seq, .. } = read(&mut socket).await else {
            panic!("expected public initiative events")
        };
        assert_eq!(seq, enemy_seq + 1);
        enemy_seq = seq;
    }
    handle
        .append(
            d.seat,
            "2026-10-06T11:00:00Z".into(),
            TranscriptEntry::Reasoning {
                text: "secret".into(),
            },
        )
        .await
        .unwrap();
    let commonwealth: SeatId = "commonwealth.air".parse().unwrap();
    handle
        .append(
            commonwealth,
            "2026-10-06T11:00:00Z".into(),
            TranscriptEntry::AssistantText {
                text: "visible".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(read(&mut socket).await,ServerMessage::Transcript{seat,tseq:1,game_seq,..} if seat==commonwealth.to_string() && game_seq==enemy_seq)
    );
    subscribe(&mut socket, "side:axis", Some(999999)).await;
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Hello { .. }
    ));
    assert!(matches!(read(&mut socket).await, ServerMessage::Resync));
    subscribe(&mut socket, "side:axis", Some(last)).await;
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Hello { .. }
    ));
    let mut seq = last;
    let target = handle.projection(Perspective::Side(Side::Axis)).seq;
    while seq < target {
        let ServerMessage::Event { seq: next, .. } = read(&mut socket).await else {
            panic!("expected replay event")
        };
        assert_eq!(next, seq + 1);
        seq = next;
    }
    assert!(
        matches!(read(&mut socket).await,ServerMessage::Transcript{seat,tseq:1,..} if seat==d.seat.to_string())
    );
    // A damaged replay row requests a fresh snapshot instead of inventing the missing event.
    let db = rusqlite::Connection::open(dir.path().join(format!("{id}.sqlite"))).unwrap();
    // Deliberately bypass the opening-index foreign key for this corruption injection only.
    db.pragma_update(None, "foreign_keys", "OFF").unwrap();
    db.execute(
        "DELETE FROM perspective_events WHERE perspective='side:axis' AND seq=1",
        [],
    )
    .unwrap();
    subscribe(&mut socket, "side:axis", Some(0)).await;
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Hello { .. }
    ));
    assert!(matches!(read(&mut socket).await, ServerMessage::Resync));
    subscribe(&mut socket, "side:axis", None).await;
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Hello { .. }
    ));
    assert!(
        matches!(read(&mut socket).await, ServerMessage::Snapshot { seq, .. } if seq == target)
    );
    socket.close(None).await.unwrap();
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_creation_control_and_local_origin_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::new(dir.path(), None).await;
    let client = server.client();
    let response = client
        .post(format!("{}/api/campaigns", server.url))
        .json(&request("aggressive", true))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let meta: Value = response.json().await.unwrap();
    let id = meta["id"].as_str().unwrap();
    assert_eq!(
        client
            .get(format!("{}/api/campaigns", server.url))
            .send()
            .await
            .unwrap()
            .json::<Vec<Value>>()
            .await
            .unwrap()
            .len(),
        1
    );
    let response = client
        .post(format!("{}/api/campaigns/{id}/resume", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let disallowed = client
        .get(format!("{}/api/campaigns", server.url))
        .header("origin", "https://outside.example")
        .send()
        .await
        .unwrap();
    assert_eq!(disallowed.status(), 403);
    let rebound = client
        .get(format!("{}/api/campaigns", server.url))
        .header("host", "outside.example")
        .send()
        .await
        .unwrap();
    assert_eq!(rebound.status(), 403);
    let cors = client
        .get(format!("{}/api/campaigns", server.url))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();
    assert_eq!(
        cors.headers()["access-control-allow-origin"],
        "http://localhost:5173"
    );
    let denied = client
        .get(format!(
            "{}/api/campaigns/{id}/transcripts?perspective=side:axis&seat=commonwealth.air",
            server.url
        ))
        .send()
        .await
        .unwrap()
        .json::<Vec<Value>>()
        .await
        .unwrap();
    assert!(denied.is_empty());
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn committed_receipt_survives_publication_failure_and_writer_stop_is_durable() {
    let dir = tempfile::tempdir().unwrap();
    let handle = sandbox::create(dir.path(), &data(), request("human", false)).unwrap();
    let d = pending(&handle).await;
    let id = handle.projection(Perspective::Operator).meta.id;
    let path = dir.path().join(format!("{id}.sqlite"));
    let db = rusqlite::Connection::open(&path).unwrap();
    let before: u64 = db
        .query_row("SELECT revision FROM campaign", [], |r| r.get(0))
        .unwrap();
    // Fault injection: the command can commit, but its new operator stream row cannot be
    // decoded by the publisher. Preserve original rows so recovery can exercise exact retry.
    db.execute_batch("CREATE TABLE publication_backup AS SELECT * FROM perspective_events WHERE 0;
        CREATE TRIGGER publication_failure AFTER INSERT ON perspective_events
        WHEN NEW.perspective='operator' BEGIN
            INSERT INTO publication_backup VALUES (NEW.perspective,NEW.seq,NEW.event_id,NEW.message);
            UPDATE perspective_events SET message='broken-json' WHERE perspective=NEW.perspective AND seq=NEW.seq;
        END;").unwrap();
    let candidate = scripted::answer(
        &d,
        handle.seat(d.seat).binding.controller_epoch,
        Mode::PassWhenPossible,
        0,
        &NoCandidates,
    )
    .unwrap();
    let req = SubmitRequest {
        decision_id: d.id.to_string(),
        epoch: candidate.controller_epoch,
        revision: Some(d.revision),
        idempotency_key: candidate.idempotency_key,
        action: candidate.action,
        public_explanation: None,
    };
    let receipt = handle.submit_action(d.seat, req.clone()).await.unwrap();
    assert!(!receipt.duplicate);
    let mut lifecycle = handle.watch_status();
    tokio::time::timeout(
        Duration::from_secs(5),
        lifecycle.wait_for(|s| matches!(s, CampaignStatus::Stopped { .. })),
    )
    .await
    .unwrap()
    .unwrap();
    let stored_status: String = db
        .query_row("SELECT status FROM campaign", [], |r| r.get(0))
        .unwrap();
    assert!(matches!(
        serde_json::from_str::<CampaignStatus>(&stored_status).unwrap(),
        CampaignStatus::Stopped { .. }
    ));
    let committed: u64 = db
        .query_row("SELECT revision FROM campaign", [], |r| r.get(0))
        .unwrap();
    assert_eq!(committed, before + 1);
    assert!(handle.shutdown().await.is_err());
    db.execute_batch("DROP TRIGGER publication_failure;
        UPDATE perspective_events SET message=(SELECT message FROM publication_backup b
            WHERE b.perspective=perspective_events.perspective AND b.seq=perspective_events.seq)
        WHERE EXISTS (SELECT 1 FROM publication_backup b WHERE b.perspective=perspective_events.perspective AND b.seq=perspective_events.seq);
        DROP TABLE publication_backup;").unwrap();
    let restored = sandbox::recover(&path, &data()).unwrap();
    assert!(matches!(restored.status(), CampaignStatus::Stopped { .. }));
    let retry = restored.submit_action(d.seat, req).await.unwrap();
    assert!(retry.duplicate);
    assert_eq!(retry.decision_id, receipt.decision_id);
    let after_retry: u64 = db
        .query_row("SELECT revision FROM campaign", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after_retry, committed);
    restored.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn storage_operation_failure_reports_a_stop_without_claiming_success() {
    let dir = tempfile::tempdir().unwrap();
    let handle = sandbox::create(dir.path(), &data(), request("human", true)).unwrap();
    let id = handle.projection(Perspective::Operator).meta.id;
    let path = dir.path().join(format!("{id}.sqlite"));
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER notebook_failure BEFORE INSERT ON notebooks
        BEGIN SELECT RAISE(ABORT,'injected storage failure'); END;",
    )
    .unwrap();
    let seat = "axis.commander".parse().unwrap();
    assert!(
        handle
            .write_notebook(seat, WriteMode::Replace, "not committed")
            .await
            .is_err()
    );
    let mut lifecycle = handle.watch_status();
    tokio::time::timeout(
        Duration::from_secs(5),
        lifecycle.wait_for(|s| matches!(s, CampaignStatus::Stopped { .. })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM notebooks", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        0
    );
    assert!(handle.shutdown().await.is_err());
    let restored = sandbox::recover(&path, &data()).unwrap();
    assert!(matches!(restored.status(), CampaignStatus::Stopped { .. }));
    restored.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scripted_baselines_emit_truthful_live_and_replayed_private_transcripts() {
    for mode in ["legal_random", "pass_when_possible", "aggressive"] {
        let dir = tempfile::tempdir().unwrap();
        let handle = sandbox::create(dir.path(), &data(), request(mode, true)).unwrap();
        let id = handle.projection(Perspective::Operator).meta.id;
        let seat: SeatId = "axis.commander".parse().unwrap();
        let enemy: SeatId = "commonwealth.commander".parse().unwrap();
        let mut live = handle.subscribe(Perspective::Seat(seat));
        handle.pause(false).await.unwrap();
        let entry = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let message = live.recv().await.unwrap();
                if matches!(message, ServerMessage::Transcript { .. }) {
                    return message;
                }
            }
        })
        .await
        .unwrap();
        assert!(matches!(entry, ServerMessage::Transcript {
            seat:ref received_seat, entry:TranscriptEntry::DecisionSubmitted { ref summary, .. }, ..
        } if received_seat==&seat.to_string() && summary.starts_with(mode)));
        finished(&handle).await;
        let rows = handle
            .replay()
            .transcripts(Perspective::Seat(seat), seat, 0)
            .unwrap();
        assert!(!rows.is_empty());
        assert!(
            handle
                .replay()
                .transcripts(Perspective::Seat(enemy), seat, 0)
                .unwrap()
                .is_empty()
        );
        let server = Server::new(dir.path(), Some(handle.clone())).await;
        let address = format!(
            "{}/api/campaigns/{id}/stream?cap={}",
            server.url.replace("http://", "ws://"),
            server.app.operator_token()
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(address).await.unwrap();
        subscribe(&mut socket, &format!("seat:{seat}"), None).await;
        assert!(matches!(
            read(&mut socket).await,
            ServerMessage::Hello { .. }
        ));
        assert!(matches!(
            read(&mut socket).await,
            ServerMessage::Snapshot { .. }
        ));
        assert_eq!(read(&mut socket).await, rows[0]);
        socket.close(None).await.unwrap();
        server.stop().await;
        let restored = sandbox::recover(&dir.path().join(format!("{id}.sqlite")), &data()).unwrap();
        assert_eq!(
            restored
                .replay()
                .transcripts(Perspective::Seat(seat), seat, 0)
                .unwrap(),
            rows
        );
        restored.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_controller_failure_cannot_pause_or_report_for_replacement_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let handle = sandbox::create(dir.path(), &data(), request("human", false)).unwrap();
    let decision = pending(&handle).await;
    let seat = decision.seat;
    handle.pause(true).await.unwrap();
    let old_epoch = handle.seat(seat).binding.controller_epoch;
    let old_controller = handle.clone();
    let (release, callback) = tokio::sync::oneshot::channel();
    let failure = tokio::spawn(async move {
        callback.await.unwrap();
        old_controller
            .mark_failure_if_epoch(seat, old_epoch, "old controller timed out")
            .await
    });
    let replacement = handle
        .handover(
            seat,
            Some(cna_protocol::ControllerInfo {
                kind: cna_protocol::ControllerKind::Scripted,
                label: "scripted:legal_random".into(),
            }),
            serde_json::json!({"mode":"legal_random"}),
        )
        .await
        .unwrap();
    assert_eq!(replacement.controller_epoch, old_epoch + 1);
    let before = handle.seat(seat);
    assert_eq!(before.pending[0].id, decision.id);
    let mut state = handle.watch_seat(seat);
    state.borrow_and_update();
    let mut stream = handle.subscribe(Perspective::Operator);
    release.send(()).unwrap();
    assert!(matches!(
        failure.await.unwrap(),
        Err(cna_server::Error::StaleEpoch)
    ));
    assert_eq!(handle.seat(seat), before);
    assert!(!state.has_changed().unwrap());
    assert!(matches!(
        stream.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(
        handle
            .replay()
            .transcripts(Perspective::Operator, seat, 0)
            .unwrap()
            .is_empty()
    );
    handle
        .mark_failure_if_epoch(
            seat,
            replacement.controller_epoch,
            "current controller budget exhausted",
        )
        .await
        .unwrap();
    assert!(handle.seat(seat).binding.paused);
    assert_eq!(
        handle.seat(seat).binding.failure.as_deref(),
        Some("current controller budget exhausted")
    );
    let rows = handle
        .replay()
        .transcripts(Perspective::Operator, seat, 0)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        matches!(&rows[0], ServerMessage::Transcript { entry:TranscriptEntry::System {text}, .. }
        if text == "Scripted controller paused: current controller budget exhausted")
    );
    assert!(matches!(
        handle
            .mark_failure_if_epoch(seat, old_epoch, "old controller retries timeout")
            .await,
        Err(cna_server::Error::StaleEpoch)
    ));
    assert_eq!(
        handle
            .replay()
            .transcripts(Perspective::Operator, seat, 0)
            .unwrap(),
        rows
    );
    let id = handle.projection(Perspective::Operator).meta.id;
    let accepted_binding = handle.seat(seat).binding;
    handle.shutdown().await.unwrap();
    let restored = sandbox::recover(&dir.path().join(format!("{id}.sqlite")), &data()).unwrap();
    assert_eq!(restored.seat(seat).binding, accepted_binding);
    assert_eq!(
        restored
            .replay()
            .transcripts(Perspective::Operator, seat, 0)
            .unwrap(),
        rows
    );
    restored.shutdown().await.unwrap();
}

// Compare all persisted rows, including RNG, receipts, windows, transcripts and bindings.
fn persisted(path: &Path) -> Vec<String> {
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let transaction = db.unchecked_transaction().unwrap();
    let mut tables = transaction.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap();
    let names: Vec<String> = tables
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut result = Vec::new();
    for name in names {
        let mut rows = transaction
            .prepare(&format!("SELECT * FROM {name} ORDER BY rowid"))
            .unwrap();
        let columns = rows.column_count();
        result.extend(
            rows.query_map([], |r| {
                let values = (0..columns)
                    .map(|i| r.get::<_, rusqlite::types::Value>(i))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!("{name}:{values:?}"))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        );
    }
    result
}
fn all_views(handle: &CampaignHandle) -> Vec<String> {
    Perspective::all()
        .map(|p| format!("{:?}", handle.projection(p)))
        .collect()
}
fn unchanged(handle: &CampaignHandle, path: &Path, rows: &[String], views: &[String]) {
    assert_eq!(persisted(path), rows);
    assert_eq!(all_views(handle), views);
}
async fn denied(socket: &mut Socket) {
    let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match frame {
        Message::Close(Some(close)) => {
            assert_eq!(u16::from(close.code), 1008);
            assert_eq!(close.reason, "capability scope denied");
        }
        other => panic!("unauthorized subscription sent a frame: {other:?}"),
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capabilities_authorize_every_http_route_without_mutation_on_denial() {
    let dir = tempfile::tempdir().unwrap();
    let handle = sandbox::create(dir.path(), &data(), request("human", false)).unwrap();
    let decision = pending(&handle).await;
    handle.pause(true).await.unwrap();
    let id = handle.projection(Perspective::Operator).meta.id;
    let path = dir.path().join(format!("{id}.sqlite"));
    let enemy: SeatId = "commonwealth.commander".parse().unwrap();
    let own = decision.seat;
    handle
        .append(
            enemy,
            "2026-10-07T00:00:00Z".into(),
            TranscriptEntry::Reasoning {
                text: "enemy-private-canary".into(),
            },
        )
        .await
        .unwrap();
    let server = Server::new(dir.path(), Some(handle.clone())).await;
    let second = sandbox::create(dir.path(), &data(), request("human", true)).unwrap();
    let other_id = second.projection(Perspective::Operator).meta.id;
    server.app.register(second);
    let rows = persisted(&path);
    let views = all_views(&handle);
    let client = reqwest::Client::new();
    let operator = server.app.operator_token();
    let side = server.app.side_token(&id, Side::Axis).unwrap();
    let seat_cap = server.app.seat_token(&id, own).unwrap();
    let wrong_side = server.app.side_token(&id, Side::Commonwealth).unwrap();
    let draft = scripted::answer(
        &decision,
        handle.seat(own).binding.controller_epoch,
        Mode::PassWhenPossible,
        0,
        &NoCandidates,
    )
    .unwrap();
    let root = format!("/api/campaigns/{id}");
    let seat_path = format!("{root}/seats/{own}");
    let decision_path = format!("{seat_path}/decisions/{}", decision.id);
    let routes = vec![
        ("GET", "/api/session".into(), Value::Null),
        ("GET", "/api/campaigns".into(), Value::Null),
        (
            "POST",
            "/api/campaigns".into(),
            serde_json::to_value(request("human", true)).unwrap(),
        ),
        ("GET", root.clone(), Value::Null),
        ("GET", format!("{root}/seats"), Value::Null),
        ("GET", format!("{root}/capabilities"), Value::Null),
        ("POST", format!("{root}/pause"), Value::Null),
        ("POST", format!("{root}/resume"), Value::Null),
        ("POST", format!("{seat_path}/pause"), Value::Null),
        (
            "POST",
            format!("{seat_path}/controller"),
            serde_json::json!({"controller":null}),
        ),
        ("GET", format!("{seat_path}/observe"), Value::Null),
        ("GET", format!("{seat_path}/inspect/unknown"), Value::Null),
        ("GET", format!("{decision_path}/actions"), Value::Null),
        (
            "POST",
            format!("{decision_path}/validate"),
            serde_json::json!({"action":draft.action,"controller_epoch":draft.controller_epoch,"decision_revision":draft.decision_revision}),
        ),
        (
            "POST",
            format!("{decision_path}/submit"),
            serde_json::to_value(&draft).unwrap(),
        ),
        (
            "GET",
            format!("{root}/transcripts?perspective=operator&seat={enemy}"),
            Value::Null,
        ),
    ];
    for (method, route, body) in &routes {
        let response = client
            .request(
                method.parse::<reqwest::Method>().unwrap(),
                format!("{}{route}", server.url),
            )
            .json(body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401, "{method} {route}");
        unchanged(&handle, &path, &rows, &views);
    }
    for token in ["wrong-token".to_owned(), "0".repeat(64)] {
        assert_eq!(
            client
                .get(format!("{}{root}", server.url))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        unchanged(&handle, &path, &rows, &views);
    }
    assert_eq!(
        client
            .get(format!("{}{root}?cap={operator}", server.url))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    for token in [&side, &seat_cap, &wrong_side] {
        for (method, route, body) in routes.iter().filter(|(_, route, _)| {
            route.ends_with("/pause")
                || route.ends_with("/resume")
                || route.ends_with("/controller")
                || route.ends_with("/capabilities")
                || route == "/api/campaigns"
        }) {
            // Restricted discovery is permitted only with an authorized explicit perspective.
            let response = client
                .request(
                    method.parse::<reqwest::Method>().unwrap(),
                    format!("{}{route}", server.url),
                )
                .bearer_auth(token)
                .json(body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 403, "{method} {route}");
            unchanged(&handle, &path, &rows, &views);
        }
    }
    for (token, route) in [
        (&wrong_side, format!("{seat_path}/observe")),
        (&side, format!("{root}?perspective=operator")),
        (&side, format!("{root}?perspective=side:commonwealth")),
        (
            &side,
            format!("{root}/transcripts?perspective=side:axis&seat={enemy}"),
        ),
        (&seat_cap, format!("{root}?perspective=side:axis")),
        (&seat_cap, format!("{root}/seats/axis.air/observe")),
        (&seat_cap, format!("{root}/seats/{enemy}/observe")),
        (
            &seat_cap,
            format!("/api/campaigns/{other_id}?perspective=seat:{own}"),
        ),
    ] {
        assert_eq!(
            client
                .get(format!("{}{route}", server.url))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .status(),
            403,
            "{route}"
        );
        unchanged(&handle, &path, &rows, &views);
    }
    assert_eq!(
        client
            .post(format!("{}{decision_path}/submit", server.url))
            .bearer_auth(&side)
            .json(&draft)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    unchanged(&handle, &path, &rows, &views);
    for (token, p) in [
        (&side, "side:axis".to_owned()),
        (&seat_cap, format!("seat:{own}")),
        (&operator, "operator".to_owned()),
    ] {
        assert_eq!(
            client
                .get(format!("{}{root}?perspective={p}", server.url))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        assert_eq!(
            client
                .get(format!("{}/api/session", server.url))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()["perspective"],
            p
        );
    }
    for token in [&side, &seat_cap, &operator] {
        for route in [
            format!("{seat_path}/observe"),
            format!("{decision_path}/actions"),
        ] {
            assert_eq!(
                client
                    .get(format!("{}{route}", server.url))
                    .bearer_auth(token)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                200
            );
        }
        assert_eq!(
            client
                .post(format!("{}{decision_path}/validate", server.url))
                .bearer_auth(token)
                .json(&serde_json::json!({"action":draft.action,"controller_epoch":draft.controller_epoch,"decision_revision":draft.decision_revision}))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    let listed: Vec<Value> = client
        .get(format!(
            "{}/api/campaigns?perspective=side:axis",
            server.url
        ))
        .bearer_auth(&side)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], id);
    let private: Vec<Value> = client
        .get(format!(
            "{}{root}/transcripts?perspective=operator&seat={enemy}",
            server.url
        ))
        .bearer_auth(&operator)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(private[0]["entry"]["text"], "enemy-private-canary");
    let issued: Value = server
        .client()
        .get(format!("{}{root}/capabilities", server.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(issued["seats"][own.to_string()], seat_cap);
    assert_eq!(issued["sides"]["axis"], side);
    assert!(issued.get("operator").is_none());
    unchanged(&handle, &path, &rows, &views);
    let preflight = client
        .request(reqwest::Method::OPTIONS, format!("{}{root}", server.url))
        .header("origin", "http://localhost:5173")
        .header("access-control-request-method", "GET")
        .header("access-control-request-headers", "authorization")
        .send()
        .await
        .unwrap();
    assert!(preflight.status().is_success());
    assert_eq!(
        preflight.headers()["access-control-allow-origin"],
        "http://localhost:5173"
    );
    let authorized = client
        .get(format!("{}{root}", server.url))
        .header("origin", "http://localhost:5173")
        .bearer_auth(&operator)
        .send()
        .await
        .unwrap();
    assert_eq!(authorized.status(), 200);
    assert_eq!(authorized.headers()["cache-control"], "no-store");
    assert_eq!(authorized.headers()["referrer-policy"], "no-referrer");
    // Administrative authority remains available to the operator.
    assert_eq!(
        server
            .client()
            .post(format!("{}{root}/resume", server.url))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        server
            .client()
            .post(format!("{}{root}/pause", server.url))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    server.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_capabilities_guard_upgrade_initial_subscription_and_every_switch() {
    let dir = tempfile::tempdir().unwrap();
    let handle = sandbox::create(dir.path(), &data(), request("human", true)).unwrap();
    let id = handle.projection(Perspective::Operator).meta.id;
    let own: SeatId = "axis.commander".parse().unwrap();
    let enemy: SeatId = "commonwealth.commander".parse().unwrap();
    for (s, text) in [(own, "own-private-canary"), (enemy, "enemy-private-canary")] {
        handle
            .append(
                s,
                "2026-10-07T00:00:00Z".into(),
                TranscriptEntry::Reasoning { text: text.into() },
            )
            .await
            .unwrap();
    }
    let server = Server::new(dir.path(), Some(handle.clone())).await;
    let side = server.app.side_token(&id, Side::Axis).unwrap();
    let seat_cap = server.app.seat_token(&id, own).unwrap();
    let address = format!(
        "{}/api/campaigns/{id}/stream",
        server.url.replace("http:", "ws:")
    );
    for (url, status) in [
        (address.clone(), 401),
        (format!("{address}?cap={}", "0".repeat(64)), 401),
        (
            format!(
                "{}/api/campaigns/other/stream?cap={seat_cap}",
                server.url.replace("http:", "ws:")
            ),
            403,
        ),
    ] {
        match tokio_tungstenite::connect_async(url).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                assert_eq!(response.status(), status)
            }
            other => panic!("unexpected upgrade: {other:?}"),
        }
    }
    for (token, p) in [
        (&side, "operator"),
        (&side, "side:commonwealth"),
        (&seat_cap, "side:axis"),
        (&seat_cap, "seat:axis.air"),
    ] {
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("{address}?cap={token}"))
            .await
            .unwrap();
        subscribe(&mut socket, p, None).await;
        denied(&mut socket).await;
    }
    for token in [&side, &seat_cap] {
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("{address}?cap={token}"))
            .await
            .unwrap();
        subscribe(&mut socket, "seat:axis.commander", None).await;
        assert!(
            matches!(read(&mut socket).await,ServerMessage::Hello { perspective,.. } if perspective=="seat:axis.commander")
        );
        assert!(matches!(
            read(&mut socket).await,
            ServerMessage::Snapshot { .. }
        ));
        assert!(
            matches!(read(&mut socket).await,ServerMessage::Transcript {seat,entry:TranscriptEntry::Reasoning{text},..} if seat==own.to_string() && text=="own-private-canary")
        );
        subscribe(&mut socket, "operator", None).await;
        denied(&mut socket).await;
    }
    // Side tokens may switch among that side and its seats; never the enemy side.
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("{address}?cap={side}"))
        .await
        .unwrap();
    subscribe(&mut socket, "side:axis", None).await;
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Hello { .. }
    ));
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Snapshot { .. }
    ));
    assert!(
        matches!(read(&mut socket).await,ServerMessage::Transcript {seat,..} if seat==own.to_string())
    );
    subscribe(&mut socket, "seat:axis.commander", None).await;
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Hello { .. }
    ));
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Snapshot { .. }
    ));
    assert!(
        matches!(read(&mut socket).await,ServerMessage::Transcript {seat,..} if seat==own.to_string())
    );
    subscribe(&mut socket, "side:commonwealth", None).await;
    denied(&mut socket).await;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut header_request = address.clone().into_client_request().unwrap();
    header_request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", server.app.operator_token())
            .parse()
            .unwrap(),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(header_request)
        .await
        .unwrap();
    subscribe(&mut socket, "operator", None).await;
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Hello { .. }
    ));
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Snapshot { .. }
    ));
    let transcripts = [read(&mut socket).await, read(&mut socket).await];
    assert!(transcripts.iter().any(|entry|matches!(entry,ServerMessage::Transcript {entry:TranscriptEntry::Reasoning{text},..} if text=="enemy-private-canary")));
    socket.close(None).await.unwrap();
    let mut ambiguous = format!("{address}?cap={seat_cap}")
        .into_client_request()
        .unwrap();
    ambiguous.headers_mut().insert(
        "authorization",
        format!("Bearer {}", server.app.operator_token())
            .parse()
            .unwrap(),
    );
    assert!(
        matches!(tokio_tungstenite::connect_async(ambiguous).await,Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status()==401)
    );
    server.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_submit_retry_and_rejections_preserve_all_persistence_and_projections() {
    let dir = tempfile::tempdir().unwrap();
    let handle = sandbox::create(dir.path(), &data(), request("human", false)).unwrap();
    let d = pending(&handle).await;
    let id = handle.projection(Perspective::Operator).meta.id;
    let path = dir.path().join(format!("{id}.sqlite"));
    let response = scripted::answer(
        &d,
        handle.seat(d.seat).binding.controller_epoch,
        Mode::PassWhenPossible,
        0,
        &NoCandidates,
    )
    .unwrap();
    let server = Server::new(dir.path(), Some(handle.clone())).await;
    let client = server.client();
    let root = format!("{}/api/campaigns/{id}", server.url);
    let endpoint = format!("{root}/seats/{}/decisions/{}/submit", d.seat, d.id);
    let rows = persisted(&path);
    let views = all_views(&handle);
    let mut stale = response.clone();
    stale.controller_epoch -= 1;
    let mut unknown = response.clone();
    unknown.decision_id = "unknown".into();
    for (url, body, status) in [
        (
            format!(
                "{root}/seats/commonwealth.commander/decisions/{}/submit",
                d.id
            ),
            response.clone(),
            400,
        ),
        (
            format!("{root}/seats/{}/decisions/another/submit", d.seat),
            response.clone(),
            400,
        ),
        (
            format!(
                "{}/api/campaigns/missing/seats/{}/decisions/{}/submit",
                server.url, d.seat, d.id
            ),
            response.clone(),
            404,
        ),
        (endpoint.clone(), stale, 409),
        (
            format!("{root}/seats/{}/decisions/unknown/submit", d.seat),
            unknown,
            400,
        ),
    ] {
        assert_eq!(
            client.post(url).json(&body).send().await.unwrap().status(),
            status
        );
        unchanged(&handle, &path, &rows, &views);
    }
    assert_eq!(
        client
            .post(&endpoint)
            .header("content-type", "application/json")
            .body("{")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    unchanged(&handle, &path, &rows, &views);
    let wrong = server
        .app
        .seat_token(&id, "commonwealth.commander".parse().unwrap())
        .unwrap();
    assert_eq!(
        reqwest::Client::new()
            .post(&endpoint)
            .bearer_auth(wrong)
            .json(&response)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    unchanged(&handle, &path, &rows, &views);
    let cap = server.app.seat_token(&id, d.seat).unwrap();
    let accepted = reqwest::Client::new()
        .post(&endpoint)
        .bearer_auth(&cap)
        .json(&response)
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), 200);
    let accepted: Value = accepted.json().await.unwrap();
    assert_eq!(accepted["duplicate"], false);
    assert_eq!(accepted["decision_id"], d.id.as_str());
    // Freeze automatic progress before measuring the exact retry; retries survive pause.
    handle.pause(true).await.unwrap();
    let after = persisted(&path);
    let after_views = all_views(&handle);
    let retry = reqwest::Client::new()
        .post(&endpoint)
        .bearer_auth(&cap)
        .json(&response)
        .send()
        .await
        .unwrap();
    assert_eq!(retry.status(), 200);
    let mut retry: Value = retry.json().await.unwrap();
    assert_eq!(retry["duplicate"], true);
    retry["duplicate"] = Value::Bool(false);
    assert_eq!(retry, accepted);
    unchanged(&handle, &path, &after, &after_views);
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM commands WHERE seat=? AND idempotency_key=?",
            rusqlite::params![d.seat.to_string(), response.idempotency_key],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        1
    );
    drop(db);
    server.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_credential_export_refreshes_atomically_and_restart_rotates_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::new(dir.path(), None).await;
    let file = dir.path().join("trusted/operator-capabilities.json");
    server.app.write_credentials(&file).unwrap();
    let initial: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(initial["operator"], server.app.operator_token());
    assert_eq!(initial["campaigns"], serde_json::json!({}));
    let handle = sandbox::create(dir.path(), &data(), request("human", true)).unwrap();
    let id = handle.projection(Perspective::Operator).meta.id;
    server.app.register(handle);
    // Inspect the automatic refresh before any explicit export could mask its failure.
    let exported: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert!(
        exported["campaigns"].get(&id).is_some(),
        "registration did not refresh the credential document"
    );
    assert_eq!(exported["operator"], initial["operator"]);
    // Independently prove an explicit export replaces a stale existing document.
    std::fs::write(&file, serde_json::to_vec(&initial).unwrap()).unwrap();
    server.app.write_credentials(&file).unwrap();
    let replaced: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(replaced, exported);
    let caps = &exported["campaigns"][&id];
    let tokens: std::collections::BTreeSet<_> = caps["sides"]
        .as_object()
        .unwrap()
        .values()
        .chain(caps["seats"].as_object().unwrap().values())
        .map(|v| v.as_str().unwrap())
        .chain([initial["operator"].as_str().unwrap()])
        .collect();
    assert_eq!(tokens.len(), 13);
    assert!(
        tokens
            .iter()
            .all(|token| token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
    );
    let second = Server::new(dir.path(), None).await;
    assert_ne!(server.app.operator_token(), second.app.operator_token());
    assert_eq!(
        reqwest::Client::new()
            .get(format!("{}/api/session", second.url))
            .bearer_auth(server.app.operator_token())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        std::fs::read_dir(file.parent().unwrap()).unwrap().count(),
        1
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    server.stop().await;
    second.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_discovery_and_retained_websocket_resume_build_no_views() {
    let dir = tempfile::tempdir().unwrap();
    let handle = sandbox::create(dir.path(), &data(), request("human", true)).unwrap();
    let id = handle.header(Perspective::Operator).meta.id;
    let server = Server::new(dir.path(), Some(handle.clone())).await;
    let listed: Vec<Value> = server
        .client()
        .get(format!("{}/api/campaigns", server.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], id);
    assert_eq!(handle.runtime_metrics().projection_views, Duration::ZERO);
    let address = format!(
        "{}/api/campaigns/{id}/stream?cap={}",
        server.url.replace("http:", "ws:"),
        server.app.operator_token()
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(&address).await.unwrap();
    subscribe(&mut socket, "side:axis", Some(0)).await;
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Hello { .. }
    ));
    assert_eq!(
        handle.runtime_metrics().projection_views,
        Duration::ZERO,
        "retained-state resume must not compute an unused snapshot"
    );
    subscribe(&mut socket, "side:axis", None).await;
    assert!(matches!(
        read(&mut socket).await,
        ServerMessage::Hello { .. }
    ));
    let ServerMessage::Snapshot { seq, view } = read(&mut socket).await else {
        panic!("fresh snapshot")
    };
    assert_eq!(seq, 0);
    assert_eq!(view, handle.projection(Perspective::Side(Side::Axis)).view);
    assert!(handle.runtime_metrics().projection_views > Duration::ZERO);
    socket.close(None).await.unwrap();
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_boundary_http_is_operator_only_and_persists_without_resuming() {
    let dir = tempfile::tempdir().unwrap();
    let handle = sandbox::create(dir.path(), &data(), request("human", true)).unwrap();
    let id = handle.header(Perspective::Operator).meta.id;
    let path = dir.path().join(format!("{id}.sqlite"));
    let server = Server::new(dir.path(), Some(handle.clone())).await;
    let root = format!("{}/api/campaigns/{id}/run-boundary", server.url);
    let client = reqwest::Client::new();
    let body = serde_json::json!({"boundary":{"game_turn":1,"op_stage":1}});
    let original = persisted(&path);
    for method in [reqwest::Method::GET, reqwest::Method::POST] {
        assert_eq!(
            client
                .request(method.clone(), &root)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        for token in [
            server.app.side_token(&id, Side::Axis).unwrap(),
            server
                .app
                .seat_token(&id, "axis.commander".parse().unwrap())
                .unwrap(),
        ] {
            assert_eq!(
                client
                    .request(method.clone(), &root)
                    .bearer_auth(token)
                    .json(&body)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                403
            );
            assert_eq!(handle.run_boundary().await.unwrap(), None);
            assert_eq!(persisted(&path), original);
        }
    }
    let operator = server.client();
    assert_eq!(
        operator
            .post(&root)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        operator
            .get(&root)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        body
    );
    assert_eq!(handle.status(), CampaignStatus::Paused);
    for invalid in [
        serde_json::json!({}),
        serde_json::json!({"boundary":{"game_turn":0}}),
        serde_json::json!({"boundary":{"game_turn":1,"op_stage":4}}),
        serde_json::json!({"boundary":{"game_turn":1,"typo":2}}),
    ] {
        assert!(
            !operator
                .post(&root)
                .json(&invalid)
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
        assert_eq!(
            operator
                .get(&root)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap(),
            body
        );
        assert_eq!(handle.status(), CampaignStatus::Paused);
    }
    assert_eq!(
        operator
            .get(format!("{}/api/campaigns/unknown/run-boundary", server.url))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    // Only the independently persisted operator control row changes.
    assert_eq!(
        persisted(&path)
            .into_iter()
            .filter(|r| !r.starts_with("run_control:"))
            .collect::<Vec<_>>(),
        original
            .into_iter()
            .filter(|r| !r.starts_with("run_control:"))
            .collect::<Vec<_>>()
    );
    server.stop().await;
    let restored = sandbox::recover(&path, &data()).unwrap();
    let status = restored.status();
    let boundary = restored.run_boundary().await.unwrap();
    restored.shutdown().await.unwrap();
    assert_eq!(status, CampaignStatus::Paused);
    assert_eq!(
        boundary,
        Some(cna_server::RunBoundary {
            game_turn: 1,
            op_stage: Some(1)
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_console_observation_and_validation_use_current_binding_prerequisites() {
    let dir = tempfile::tempdir().unwrap();
    let handle = sandbox::create(dir.path(), &data(), request("human", false)).unwrap();
    let d = pending(&handle).await;
    handle.pause(true).await.unwrap();
    let id = handle.header(Perspective::Operator).meta.id;
    let path = dir.path().join(format!("{id}.sqlite"));
    let epoch = handle.seat(d.seat).binding.controller_epoch;
    let response = scripted::answer(&d, epoch, Mode::PassWhenPossible, 0, &NoCandidates).unwrap();
    let server = Server::new(dir.path(), Some(handle.clone())).await;
    let client = server.client();
    let root = format!("{}/api/campaigns/{id}/seats/{}", server.url, d.seat);
    let endpoint = format!("{root}/decisions/{}/validate", d.id);
    let observe: Value = client
        .get(format!("{root}/observe"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(observe["controller"]["kind"], "human");
    assert_eq!(observe["controller_epoch"], epoch);
    assert_eq!(observe["pending"][0]["revision"], d.revision);
    let rows = persisted(&path);
    let views = all_views(&handle);
    let draft = serde_json::json!({"action":response.action,"controller_epoch":epoch,"decision_revision":d.revision});
    assert_eq!(
        client
            .post(&endpoint)
            .json(&draft)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    unchanged(&handle, &path, &rows, &views);
    // The current epoch must not replace the caller's stale value, even for a legal action.
    for (body, error) in [
        (
            serde_json::json!({"action":response.action,"controller_epoch":epoch-1,"decision_revision":d.revision}),
            "this seat's controller epoch is out of date; the session was superseded".to_owned(),
        ),
        (
            serde_json::json!({"action":response.action,"controller_epoch":epoch,"decision_revision":d.revision+1}),
            format!(
                "stale decision revision {} (current is {})",
                d.revision + 1,
                d.revision
            ),
        ),
    ] {
        let rejected = client.post(&endpoint).json(&body).send().await.unwrap();
        assert_eq!(rejected.status(), 409);
        assert_eq!(
            rejected.json::<Value>().await.unwrap(),
            serde_json::json!({"error":error})
        );
        unchanged(&handle, &path, &rows, &views);
    }
    for body in [
        serde_json::json!({"action":response.action}),
        serde_json::json!({"action":response.action,"controller_epoch":epoch}),
    ] {
        assert_eq!(
            client
                .post(&endpoint)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            422
        );
        unchanged(&handle, &path, &rows, &views);
    }
    let mut invalid = draft.clone();
    invalid["action"] = serde_json::json!({"not_a_legal_answer":true});
    let expected = handle
        .validate_action_at(
            d.seat,
            d.id.as_str(),
            invalid["action"].clone(),
            epoch,
            d.revision,
        )
        .await
        .unwrap_err()
        .to_string();
    let rejected = client.post(&endpoint).json(&invalid).send().await.unwrap();
    assert_eq!(rejected.status(), 400);
    assert_eq!(
        rejected.json::<Value>().await.unwrap(),
        serde_json::json!({"error":expected})
    );
    unchanged(&handle, &path, &rows, &views);
    let replacement = handle
        .handover(
            d.seat,
            Some(cna_protocol::ControllerInfo {
                kind: cna_protocol::ControllerKind::Scripted,
                label: "scripted:legal_random".into(),
            }),
            serde_json::json!({"mode":"legal_random"}),
        )
        .await
        .unwrap();
    let rows = persisted(&path);
    let views = all_views(&handle);
    let observe: Value = client
        .get(format!("{root}/observe"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(observe["controller"]["kind"], "scripted");
    assert_eq!(observe["controller_epoch"], replacement.controller_epoch);
    assert_eq!(observe["pending"][0]["id"], d.id.as_str());
    assert_eq!(
        client
            .post(&endpoint)
            .json(&draft)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(
        client
            .post(format!("{root}/decisions/{}/submit", d.id))
            .json(&response)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    unchanged(&handle, &path, &rows, &views);
    let mut refreshed = draft;
    refreshed["controller_epoch"] = replacement.controller_epoch.into();
    assert_eq!(
        client
            .post(&endpoint)
            .json(&refreshed)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    unchanged(&handle, &path, &rows, &views);
    server.stop().await;
}
