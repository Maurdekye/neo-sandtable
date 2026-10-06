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
        "{}/api/campaigns/{id}/stream",
        server.url.replace("http:", "ws:")
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
    let client = reqwest::Client::new();
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
