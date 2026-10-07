mod support;
use async_trait::async_trait;
use cna_play::Demo;
use cna_protocol::{ClientMessage, ServerMessage, TranscriptEntry};
use cna_seats::run::PromptBuilder;
use cna_seats::{
    driver::{
        CliKind, DriverError, EntryEmitter, SeatDriver, SessionInfo, TurnOutcome, Usage,
        tool_result_entry,
    },
    transcript::TranscriptSink,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio_tungstenite::tungstenite::Message;

struct FakeCli {
    url: String,
    sink: TranscriptSink,
    seat: cna_core::ids::SeatId,
    fail: bool,
    stopped: bool,
    drain_pending: bool,
}
impl FakeCli {
    async fn confirm_captures(&self) {
        tokio::time::timeout(Duration::from_secs(30), self.sink.flush_confirmed())
            .await
            .expect("fixture transcript confirmation hang guard elapsed")
            .expect("fixture transcript worker stopped before confirming its batch");
    }
    async fn call(&self, tool: &str, args: Value, emitter: &mut EntryEmitter, id: &str) -> Value {
        emitter.emit(TranscriptEntry::ToolCall {
            call_id: id.into(),
            tool: tool.into(),
            args: args.clone(),
        });
        let v: Value = reqwest::Client::new().post(&self.url)
            .json(&json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":args}}))
            .send().await.unwrap().json().await.unwrap();
        let r = &v["result"];
        let text = r["content"][0]["text"].as_str().unwrap();
        assert_ne!(r["isError"], true, "{v}");
        emitter.emit(tool_result_entry(id.into(), true, text));
        serde_json::from_str(text).unwrap()
    }
}
#[async_trait]
impl SeatDriver for FakeCli {
    fn kind(&self) -> CliKind {
        CliKind::Claude
    }
    async fn start(&mut self, _: Option<&str>) -> Result<SessionInfo, DriverError> {
        Ok(SessionInfo {
            session_id: "fake".into(),
            model: Some("fake".into()),
            cli_version: None,
            resumed: false,
        })
    }
    async fn run_turn(&mut self, _: &str, _: Duration) -> Result<TurnOutcome, DriverError> {
        if self.fail {
            return Err(DriverError::Timeout);
        }
        let mut e = EntryEmitter::new(self.seat, self.sink.clone());
        for step in 0..4096 {
            let observed = self
                .call("observe", json!({}), &mut e, &format!("observe-{step}"))
                .await;
            let Some(id) = observed["pending_decisions"][0]["decision_id"].as_str() else {
                break;
            };
            if self.drain_pending {
                // The actual observation remains complete even above the capture detail cap.
                assert!(
                    observed["pending_decisions"]
                        .as_array()
                        .is_some_and(|pending| {
                            pending.iter().any(|p| {
                                p["kind"]
                                    .as_str()
                                    .is_some_and(|kind| kind.starts_with("cna."))
                            })
                        })
                );
            }
            let described = self
                .call(
                    "describe_actions",
                    json!({"decision_id":id}),
                    &mut e,
                    &format!("actions-{step}"),
                )
                .await;
            let request: cna_core::decision::DecisionRequest =
                serde_json::from_value(described["request"].clone()).unwrap();
            let action = if request.space.pass.is_some() {
                Value::Null
            } else {
                first_fixture_action(&request.space.schema)
            };
            request
                .space
                .check(&action)
                .expect("fixture action matches advertised schema");
            self.call(
                "submit",
                json!({"decision_id":id,"revision":request.revision,"action":action}),
                &mut e,
                &format!("submit-{step}"),
            )
            .await;
            if !self.drain_pending {
                break;
            }
            // The inert setup fixture can outpace the serialized transcript writer.
            // Confirm each decision batch before generating the next one.
            self.confirm_captures().await;
            assert!(
                step < 4095,
                "inert fixture exceeded its decision safety bound"
            );
        }
        e.emit(TranscriptEntry::AssistantText {
            text: "Offered decisions submitted.".into(),
        });
        // The fake turn ends when its emitted captures are confirmed, independently
        // of the production final drain. Writer-loss tests inject failure afterward.
        self.confirm_captures().await;
        Ok(TurnOutcome {
            ok: true,
            error: None,
            text: None,
            usage: Usage::default(),
            quota: vec![],
        })
    }
    fn session_id(&self) -> Option<String> {
        Some("fake".into())
    }
    fn is_alive(&mut self) -> bool {
        !self.stopped
    }
    async fn stop(&mut self) {
        self.stopped = true;
    }
}
// An inert baseline for integration fixtures, never a real AI failure fallback.
fn first_fixture_action(schema: &cna_core::decision::ActionSchema) -> Value {
    use cna_core::decision::ActionSchema;
    match schema {
        ActionSchema::Choice { options } => json!(options.first().expect("empty choice").id),
        ActionSchema::Integer { min, .. } => json!(min),
        ActionSchema::Bool => json!(false),
        ActionSchema::Unit { among } => json!(among.first().expect("empty unit domain").as_str()),
        ActionSchema::Hex { among: Some(hexes) } => {
            json!(hexes.first().expect("empty hex domain").as_str())
        }
        ActionSchema::Path { .. } => json!([]),
        ActionSchema::Record { fields } => Value::Object(
            fields
                .iter()
                .filter(|f| !f.optional)
                .map(|f| (f.name.clone(), first_fixture_action(&f.schema)))
                .collect(),
        ),
        ActionSchema::List { item, min, .. } => {
            Value::Array((0..*min).map(|_| first_fixture_action(item)).collect())
        }
        ActionSchema::Hex { among: None } | ActionSchema::Text { .. } => {
            panic!("fixture needs an enumerated domain or an offered pass")
        }
    }
}
async fn setup() -> (tempfile::TempDir, Demo) {
    let root = tempfile::tempdir().unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let demo = Demo::new(root.path(), &repo.join("data"), &repo.join("web/dist")).await;
    (root, demo)
}

type FixtureSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn collect_fixture_transcripts(
    mut socket: FixtureSocket,
    wanted_seat: String,
    initial_rows: Vec<ServerMessage>,
    mut completed: tokio::sync::oneshot::Receiver<(Vec<u64>, tokio::time::Instant)>,
    require_resync: bool,
) -> Result<(Vec<ServerMessage>, usize, usize), String> {
    let result = async {
        let mut rows = initial_rows.into_iter().map(|frame| {
            let ServerMessage::Transcript { tseq, .. } = &frame else { unreachable!() };
            (*tseq, frame)
        }).collect::<std::collections::BTreeMap<_, _>>();
        let mut expected = None;
        let mut deadline = None;
        let mut resyncs = 0;
        let mut duplicates = 0;
        loop {
            let found = rows.keys().copied().collect::<Vec<_>>();
            if expected.as_ref() == Some(&found) && (!require_resync || (resyncs > 0 && duplicates > 0)) {
                return Ok((rows.into_values().collect(), resyncs, duplicates));
            }
            tokio::select! {
                target = &mut completed, if expected.is_none() => {
                    let (target, limit) = target.map_err(|e| e.to_string())?;
                    expected = Some(target);
                    deadline = Some(limit);
                }
                _ = async {
                    match deadline {
                        Some(limit) => tokio::time::sleep_until(limit).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    return Err(format!("30s post-play receive guard elapsed: expected {expected:?}, received {found:?}, resyncs {resyncs}, identical replays {duplicates}"));
                }
                message = socket.next() => {
                    let message = message.ok_or("socket closed before transcript confirmation")?
                        .map_err(|e| e.to_string())?;
                    match message {
                        Message::Text(text) => {
                            let frame: ServerMessage = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                            match &frame {
                                ServerMessage::Resync => {
                                    resyncs += 1;
                                    // Lag requests a new subscription. Its transcript history may
                                    // repeat frames already received, keyed by the seat and tseq.
                                    socket.send(Message::Text(serde_json::to_string(&ClientMessage::Subscribe {
                                        perspective: "operator".into(), from_seq: None,
                                    }).map_err(|e| e.to_string())?.into())).await.map_err(|e| e.to_string())?;
                                }
                                ServerMessage::Transcript { seat, tseq, .. } if seat == &wanted_seat => {
                                    if let Some(previous) = rows.insert(*tseq, frame.clone()) {
                                        if previous != frame {
                                            return Err(format!("replay changed transcript frame {tseq}"));
                                        }
                                        duplicates += 1;
                                    }
                                }
                                _ => {}
                            }
                        }
                        Message::Close(_) => return Err("socket closed before transcript confirmation".into()),
                        _ => {}
                    }
                }
            }
        }
    }.await;
    let closed = socket.close(None).await.map_err(|e| e.to_string());
    match result {
        Ok(rows) => closed.map(|()| rows),
        Err(error) => Err(error),
    }
}

#[tokio::test]
async fn mcp_answer_reaches_persistent_websocket_transcript() {
    websocket_transcript_fixture(false).await;
}

#[tokio::test]
async fn mcp_transcript_resync_replays_identical_frames_without_double_counting() {
    websocket_transcript_fixture(true).await;
}

async fn websocket_transcript_fixture(force_resync: bool) {
    let (_root, demo) = setup().await;
    if force_resync {
        demo.sink.system(demo.seat, "inert fixture replay canary");
        tokio::time::timeout(Duration::from_secs(30), demo.sink.flush_confirmed())
            .await
            .unwrap()
            .unwrap();
    }
    let url = format!(
        "{}/api/campaigns/{}/stream?cap={}",
        demo.base_url.replace("http:", "ws:"),
        demo.campaign_id(),
        demo.board_url().split_once("#cap=").unwrap().1
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    socket
        .send(Message::Text(
            serde_json::to_string(&ClientMessage::Subscribe {
                perspective: "operator".into(),
                from_seq: None,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    // The snapshot confirms registration; drain concurrently while CLI captures
    // arrive, including while the server is finishing its initial history replay.
    loop {
        let message = socket.next().await.unwrap().unwrap();
        if let Message::Text(t) = message
            && matches!(
                serde_json::from_str::<ServerMessage>(&t).unwrap(),
                ServerMessage::Snapshot { .. }
            )
        {
            break;
        }
    }
    let mut initial_rows = Vec::new();
    if force_resync {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            if let Message::Text(text) = message {
                let frame: ServerMessage = serde_json::from_str(&text).unwrap();
                if matches!(&frame, ServerMessage::Transcript { seat, tseq: 1, .. } if seat == &demo.seat.to_string())
                {
                    initial_rows.push(frame);
                    break;
                }
            }
        }
        // Deterministic real-server recovery witness: the previous fixture ignored
        // this Resync and exhausted its unchanged 30s guard. No delay/load is needed.
        socket
            .send(Message::Text(
                serde_json::to_string(&ClientMessage::Subscribe {
                    perspective: "operator".into(),
                    from_seq: Some(u64::MAX),
                })
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
    }
    let mut driver = FakeCli {
        url: demo.mcp.url(demo.seat).unwrap(),
        sink: demo.sink.clone(),
        seat: demo.seat,
        fail: false,
        stopped: false,
        drain_pending: false,
    };
    let (finished, completed) = tokio::sync::oneshot::channel();
    let play = async {
        let result = demo.play(&mut driver, 1).await;
        let rows = demo.transcript();
        let expected = rows
            .iter()
            .map(|m| match m {
                ServerMessage::Transcript { tseq, .. } => *tseq,
                _ => unreachable!(),
            })
            .collect::<Vec<_>>();
        // Exactly the previous post-play guard, including time before the reader
        // observes this signal; concurrent draining adds no extra receive time.
        let _ = finished.send((
            expected,
            tokio::time::Instant::now() + Duration::from_secs(30),
        ));
        (result, rows)
    };
    let receive = collect_fixture_transcripts(
        socket,
        demo.seat.to_string(),
        initial_rows,
        completed,
        force_resync,
    );
    let ((played, rows), received) = tokio::join!(play, receive);
    let cleanup = demo.shutdown().await;
    played.unwrap();
    let (actual, resyncs, duplicates) = received.unwrap();
    cleanup.unwrap();
    assert!(driver.stopped);
    assert!(rows.iter().any(|m| matches!(
        m,
        ServerMessage::Transcript {
            entry: TranscriptEntry::DecisionSubmitted { .. },
            ..
        }
    )));
    let expected = rows
        .iter()
        .map(|m| match m {
            ServerMessage::Transcript { tseq, .. } => *tseq,
            _ => unreachable!(),
        })
        .collect::<Vec<_>>();
    assert_eq!(expected, (1..=expected.len() as u64).collect::<Vec<_>>());
    // Compare complete durable payloads, timestamps and alignment, not just ids.
    assert_eq!(actual, rows);
    if force_resync {
        assert!(resyncs > 0);
        assert!(duplicates > 0);
    }
}

#[tokio::test]
async fn human_console_links_authenticate_only_their_current_seat() {
    use cna_play::config::{GameKind, LaunchConfig};
    let root = tempfile::tempdir().unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = LaunchConfig::resolve(
        GameKind::Sandbox,
        &[
            "axis.commander=human".into(),
            "commonwealth.commander=human".into(),
            "*=scripted:aggressive".into(),
        ],
    )
    .unwrap();
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        config,
    )
    .await
    .unwrap();
    assert!(
        demo.epochs.is_empty(),
        "human bindings start no CLI sessions"
    );
    let links = demo.human_console_urls();
    assert_eq!(links.len(), 2);
    let client = reqwest::Client::new();
    let operator_cap = demo.board_url().split_once("#cap=").unwrap().1.to_owned();
    for (seat, link) in &links {
        let url = reqwest::Url::parse(link).unwrap();
        assert_eq!(
            url.origin(),
            reqwest::Url::parse(&demo.base_url).unwrap().origin()
        );
        assert_eq!(url.path(), "/console.html");
        let query: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("campaign"), Some(&demo.campaign_id()));
        assert_eq!(query.get("seat"), Some(&seat.to_string()));
        assert!(!query.contains_key("cap"));
        let cap = url.fragment().unwrap().strip_prefix("cap=").unwrap();
        assert!(
            cap != operator_cap,
            "console must not contain operator authority"
        );
        let session: Value = client
            .get(format!("{}/api/session", demo.base_url))
            .bearer_auth(cap)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(session["operator"], false);
        assert_eq!(session["campaign_id"], demo.campaign_id());
        assert_eq!(session["perspective"], format!("seat:{seat}"));
        let snapshot = format!("{}/api/campaigns/{}", demo.base_url, demo.campaign_id());
        assert_eq!(
            client
                .get(&snapshot)
                .query(&[("perspective", format!("seat:{seat}"))])
                .bearer_auth(cap)
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::OK
        );
        for denied in [
            "operator".to_owned(),
            format!(
                "seat:{}",
                links.iter().find(|(other, _)| other != seat).unwrap().0
            ),
        ] {
            assert_eq!(
                client
                    .get(&snapshot)
                    .query(&[("perspective", denied)])
                    .bearer_auth(cap)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                reqwest::StatusCode::FORBIDDEN
            );
        }
    }
    let changed = links[0].0;
    demo.handle
        .handover(
            changed,
            Some(cna_protocol::ControllerInfo {
                kind: cna_protocol::ControllerKind::Scripted,
                label: "scripted".into(),
            }),
            json!({"mode":"aggressive"}),
        )
        .await
        .unwrap();
    assert_eq!(
        demo.human_console_urls()
            .iter()
            .map(|(seat, _)| *seat)
            .collect::<Vec<_>>(),
        vec![links[1].0]
    );
    demo.shutdown().await.unwrap();
}

#[tokio::test]
async fn timeout_pauses_without_substituting_an_order() {
    let (_root, demo) = setup().await;
    let mut driver = FakeCli {
        url: demo.mcp.url(demo.seat).unwrap(),
        sink: demo.sink.clone(),
        seat: demo.seat,
        fail: true,
        stopped: false,
        drain_pending: false,
    };
    assert!(demo.play(&mut driver, 1).await.is_err());
    assert!(driver.stopped);
    assert!(demo.handle.seat(demo.seat).binding.failure.is_some());
    assert!(!demo.handle.seat(demo.seat).pending.is_empty());
    assert!(!demo.transcript().iter().any(|m| matches!(
        m,
        ServerMessage::Transcript {
            entry: TranscriptEntry::DecisionSubmitted { .. },
            ..
        }
    )));
    demo.shutdown().await.unwrap();
}

#[tokio::test]
async fn live_haiku_server_probe_is_opt_in() {
    if std::env::var("CNA_LIVE_CLI_TESTS").as_deref() != Ok("1") {
        return;
    }
    use cna_seats::{
        driver::claude::{ClaudeConfig, ClaudeDriver},
        run::PromptBuilder,
    };
    let profile = std::env::var_os("CNA_CLAUDE_CONFIG_DIR").expect("claude-5 profile required");
    let email = std::env::var("CNA_CLAUDE_EMAIL").expect("expected claude-5 email required");
    let (root, demo) = setup().await;
    let mut driver = ClaudeDriver::new(
        ClaudeConfig {
            seat: demo.seat,
            exe: None,
            model: "haiku".into(),
            config_dir: Some(profile.into()),
            expected_email: Some(email),
            sandbox: std::env::temp_dir().join(format!("cna-live-seat-{}", std::process::id())),
            run_dir: root.path().join("cli"),
            mcp_url: demo.mcp.url(demo.seat).unwrap(),
            system_prompt: demo.prompts.system_prompt(demo.seat),
            effort: None,
            context_window: None,
        },
        demo.sink.clone(),
    );
    let result = demo.play(&mut driver, 1).await;
    let rows = demo.transcript();
    demo.shutdown().await.unwrap();
    result.unwrap();
    assert!(rows.iter().any(|m| matches!(
        m,
        ServerMessage::Transcript {
            entry: TranscriptEntry::DecisionSubmitted { .. },
            ..
        }
    )));
}

struct FinishCli {
    inner: FakeCli,
    handle: cna_server::actor::CampaignHandle,
}
#[async_trait]
impl SeatDriver for FinishCli {
    fn kind(&self) -> CliKind {
        self.inner.kind()
    }
    async fn start(&mut self, resume: Option<&str>) -> Result<SessionInfo, DriverError> {
        self.inner.start(resume).await
    }
    async fn run_turn(
        &mut self,
        prompt: &str,
        limit: Duration,
    ) -> Result<TurnOutcome, DriverError> {
        let mut state = self.handle.watch_seat(self.inner.seat);
        let mut status = self.handle.watch_status();
        let mut last = None;
        loop {
            if matches!(
                *status.borrow_and_update(),
                cna_server::CampaignStatus::Finished { .. }
            ) {
                return Ok(last.unwrap());
            }
            let pending = !state.borrow_and_update().pending.is_empty();
            if pending {
                last = Some(self.inner.run_turn(prompt, limit).await?);
            } else {
                tokio::select! {
                    _ = state.changed() => {},
                    _ = status.changed() => {},
                }
            }
        }
    }
    fn session_id(&self) -> Option<String> {
        self.inner.session_id()
    }
    fn is_alive(&mut self) -> bool {
        self.inner.is_alive()
    }
    async fn stop(&mut self) {
        self.inner.stop().await;
    }
}
#[tokio::test]
async fn finishing_inside_one_cli_turn_is_success() {
    let (_root, demo) = setup().await;
    let mut driver = FinishCli {
        inner: FakeCli {
            url: demo.mcp.url(demo.seat).unwrap(),
            sink: demo.sink.clone(),
            seat: demo.seat,
            fail: false,
            stopped: false,
            drain_pending: false,
        },
        handle: demo.handle.clone(),
    };
    let result = demo.play(&mut driver, 2).await;
    assert!(matches!(
        demo.handle.status(),
        cna_server::CampaignStatus::Finished { .. }
    ));
    assert!(driver.inner.stopped);
    demo.shutdown().await.unwrap();
    result.unwrap();
}

#[test]
fn measured_haiku_fixture_has_three_decisions_and_paired_tools() {
    use std::collections::BTreeSet;
    let fixture = include_str!("fixtures/claude_sandbox_transcript.jsonl");
    assert!(!fixture.contains("@gmail.com"));
    assert!(!fixture.contains("/mcp/"));
    let mut calls = BTreeSet::new();
    let mut last_game_seq = 0;
    let mut decisions = 0;
    let mut assistant = 0;
    for (index, line) in fixture.lines().enumerate() {
        let ServerMessage::Transcript {
            seat,
            tseq,
            game_seq,
            entry,
            ..
        } = serde_json::from_str(line).unwrap()
        else {
            panic!("not a transcript");
        };
        assert_eq!(seat, "axis.commander");
        assert_eq!(tseq, index as u64 + 1);
        assert!(game_seq >= last_game_seq);
        last_game_seq = game_seq;
        match entry {
            TranscriptEntry::ToolCall { call_id, .. } => {
                assert!(calls.insert(call_id));
            }
            TranscriptEntry::ToolResult { call_id, ok, .. } => {
                assert!(ok);
                assert!(calls.remove(&call_id));
            }
            TranscriptEntry::DecisionSubmitted { .. } => decisions += 1,
            TranscriptEntry::AssistantText { .. } => assistant += 1,
            _ => {}
        }
    }
    assert!(calls.is_empty());
    assert_eq!(decisions, 3);
    assert_eq!(assistant, 1);
    assert!(last_game_seq > 0);
}

struct HangingCli {
    inner: FakeCli,
    started: std::sync::Arc<tokio::sync::Notify>,
}
#[async_trait]
impl SeatDriver for HangingCli {
    fn kind(&self) -> CliKind {
        self.inner.kind()
    }
    async fn start(&mut self, resume: Option<&str>) -> Result<SessionInfo, DriverError> {
        self.inner.start(resume).await
    }
    async fn run_turn(&mut self, _: &str, _: Duration) -> Result<TurnOutcome, DriverError> {
        self.started.notify_one();
        std::future::pending().await
    }
    fn session_id(&self) -> Option<String> {
        self.inner.session_id()
    }
    fn is_alive(&mut self) -> bool {
        self.inner.is_alive()
    }
    async fn stop(&mut self) {
        self.inner.stop().await;
    }
}
#[tokio::test]
async fn handover_stops_old_cli_without_pausing_replacement_binding() {
    let (_root, demo) = setup().await;
    let started = std::sync::Arc::new(tokio::sync::Notify::new());
    let mut driver = HangingCli {
        inner: FakeCli {
            url: demo.mcp.url(demo.seat).unwrap(),
            sink: demo.sink.clone(),
            seat: demo.seat,
            fail: false,
            stopped: false,
            drain_pending: false,
        },
        started: started.clone(),
    };
    let handle = demo.handle.clone();
    let seat = demo.seat;
    let (result, replacement) = tokio::join!(demo.play(&mut driver, 1), async {
        started.notified().await;
        handle
            .handover(
                seat,
                Some(cna_protocol::ControllerInfo {
                    kind: cna_protocol::ControllerKind::Human,
                    label: "replacement".into(),
                }),
                json!({}),
            )
            .await
            .unwrap()
    });
    assert!(result.is_err());
    assert!(driver.inner.stopped);
    let binding = demo.handle.seat(demo.seat).binding;
    assert_eq!(binding.controller_epoch, replacement.controller_epoch);
    assert!(binding.failure.is_none());
    assert!(!binding.paused);
    demo.shutdown().await.unwrap();
}

#[tokio::test]
async fn stopped_writer_cleanup_is_bounded_and_saves_unconfirmed_captures() {
    let (_root, demo) = setup().await;
    let outbox = demo.outbox.clone();
    demo.handle.shutdown().await.unwrap();
    demo.sink.system(demo.seat, "capture after writer loss");
    let error = tokio::time::timeout(Duration::from_secs(30), demo.shutdown())
        .await
        .expect("shutdown must be bounded")
        .expect_err("persistence must report failure");
    assert!(error.contains("unconfirmed captures"));
    let text = tokio::fs::read_to_string(outbox).await.unwrap();
    let rows: Vec<cna_seats::transcript::UnconfirmedEntry> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 1);
    assert!(
        matches!(&rows[0].entry, TranscriptEntry::System { text } if text == "capture after writer loss")
    );
}

struct AuthProbeCleanup(std::path::PathBuf);
impl Drop for AuthProbeCleanup {
    fn drop(&mut self) {
        if let Ok(text) = std::fs::read_to_string(&self.0)
            && let Ok(pid) = text.parse::<u32>()
        {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("taskkill.exe")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .creation_flags(0x0800_0000)
                    .output();
            }
            #[cfg(not(windows))]
            {
                let _ = std::process::Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .output();
            }
        }
    }
}
async fn process_exists(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let out = tokio::process::Command::new("tasklist.exe")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .creation_flags(0x0800_0000)
            .output()
            .await
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
    }
    #[cfg(not(windows))]
    {
        tokio::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .await
            .unwrap()
            .status
            .success()
    }
}
#[tokio::test]
async fn handover_during_authentication_kills_the_probe_child() {
    use cna_seats::{
        driver::claude::{ClaudeConfig, ClaudeDriver},
        run::PromptBuilder,
    };
    let (root, demo) = setup().await;
    // A native inert test executable, never the installed provider CLI.
    let source = root.path().join("fake_auth.rs");
    let exe = root
        .path()
        .join(format!("fake-auth{}", std::env::consts::EXE_SUFFIX));
    tokio::fs::write(
        &source,
        r#"
        fn main() {
            let pid = std::env::current_exe().unwrap().parent().unwrap().join("auth.pid");
            std::fs::write(pid, std::process::id().to_string()).unwrap();
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
    "#,
    )
    .await
    .unwrap();
    let build = tokio::process::Command::new("rustc")
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output()
        .await
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let pid_file = root.path().join("auth.pid");
    let _cleanup = AuthProbeCleanup(pid_file.clone());
    let mut driver = ClaudeDriver::new(
        ClaudeConfig {
            seat: demo.seat,
            exe: Some(exe),
            model: "haiku".into(),
            config_dir: None,
            expected_email: Some("fake@unit.invalid".into()),
            sandbox: root.path().join("sandbox"),
            run_dir: root.path().join("cli"),
            mcp_url: demo.mcp.url(demo.seat).unwrap(),
            system_prompt: demo.prompts.system_prompt(demo.seat),
            effort: None,
            context_window: None,
        },
        demo.sink.clone(),
    );
    let handle = demo.handle.clone();
    let seat = demo.seat;
    let (result, ()) = tokio::join!(demo.play(&mut driver, 1), async {
        let pid = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(text) = tokio::fs::read_to_string(&pid_file).await
                    && let Ok(pid) = text.parse::<u32>()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            process_exists(pid).await,
            "the probe must actually have started"
        );
        handle
            .handover(
                seat,
                Some(cna_protocol::ControllerInfo {
                    kind: cna_protocol::ControllerKind::Human,
                    label: "replacement".into(),
                }),
                json!({}),
            )
            .await
            .unwrap();
        // Less than the child's 30-second natural lifetime: this must prove cancellation.
        tokio::time::timeout(Duration::from_secs(10), async {
            while process_exists(pid).await {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("cancelled auth child must terminate");
    });
    assert!(result.is_err());
    assert!(demo.handle.seat(seat).binding.failure.is_none());
    demo.shutdown().await.unwrap();
}

#[tokio::test]
async fn recovery_write_error_survives_an_existing_cli_failure() {
    let (root, mut demo) = setup().await;
    // An existing directory is unwritable as a file even under elevated test permissions.
    demo.outbox = root.path().into();
    demo.handle.shutdown().await.unwrap();
    demo.sink.system(demo.seat, "unconfirmed capture");
    let cleanup = tokio::time::timeout(Duration::from_secs(30), demo.shutdown())
        .await
        .expect("bounded cleanup");
    let error = cna_play::combine_results(Err("CLI timed out".into()), cleanup).unwrap_err();
    assert!(error.contains("CLI timed out"));
    assert!(error.contains("could not be written"));
    assert!(error.contains(&root.path().display().to_string()));
    assert_eq!(
        cna_play::combine_results(Ok(()), Err("cleanup".into())),
        Err("cleanup".into())
    );
    assert_eq!(
        cna_play::combine_results(Err("primary".into()), Ok(())),
        Err("primary".into())
    );
    assert!(cna_play::combine_results(Ok(()), Ok(())).is_ok());
}

#[tokio::test]
async fn campaigns_in_one_directory_keep_separate_recovery_outboxes() {
    let (root, first) = setup().await;
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let second = Demo::new(root.path(), &repo.join("data"), &repo.join("web/dist")).await;
    assert_ne!(first.campaign_id(), second.campaign_id());
    let first_path = first.outbox.clone();
    let second_path = second.outbox.clone();
    assert_ne!(first_path, second_path);
    first.handle.shutdown().await.unwrap();
    second.handle.shutdown().await.unwrap();
    first.sink.system(first.seat, "first campaign capture");
    second.sink.system(second.seat, "second campaign capture");
    let (one, two) = tokio::join!(first.shutdown(), second.shutdown());
    assert!(one.is_err() && two.is_err());
    let one = tokio::fs::read_to_string(first_path).await.unwrap();
    let two = tokio::fs::read_to_string(second_path).await.unwrap();
    assert!(one.contains("first campaign capture"));
    assert!(!one.contains("second campaign capture"));
    assert!(two.contains("second campaign capture"));
    assert!(!two.contains("first campaign capture"));
}

#[tokio::test]
async fn trusted_viewer_capability_is_separate_from_the_driver_mcp_endpoint() {
    let (_root, demo) = setup().await;
    let board_url = demo.board_url();
    let (public_url, operator) = board_url.split_once("#cap=").unwrap();
    assert_eq!(
        public_url,
        format!("{}/?campaign={}", demo.base_url, demo.campaign_id())
    );
    let seat_url = demo.mcp.url(demo.seat).unwrap();
    assert!(!seat_url.contains(operator));
    assert!(!demo.prompts.system_prompt(demo.seat).contains(operator));
    let client = reqwest::Client::new();
    let session_url = format!("{}/api/session", demo.base_url);
    assert_eq!(client.get(&session_url).send().await.unwrap().status(), 401);
    // The MCP credential cannot be used as an HTTP operator capability.
    let mcp_token = seat_url.rsplit('/').next().unwrap();
    assert_eq!(
        client
            .get(&session_url)
            .bearer_auth(mcp_token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let session: Value = client
        .get(&session_url)
        .bearer_auth(operator)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["perspective"], "operator");
    assert_eq!(
        client
            .get(format!(
                "{}/api/campaigns/{}",
                demo.base_url,
                demo.campaign_id()
            ))
            .bearer_auth(operator)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    demo.shutdown().await.unwrap();
}

struct WriterLossOnStopCli {
    inner: FakeCli,
    handle: cna_server::actor::CampaignHandle,
}
#[async_trait]
impl SeatDriver for WriterLossOnStopCli {
    fn kind(&self) -> CliKind {
        self.inner.kind()
    }
    async fn start(&mut self, resume: Option<&str>) -> Result<SessionInfo, DriverError> {
        self.inner.start(resume).await
    }
    async fn run_turn(
        &mut self,
        prompt: &str,
        limit: Duration,
    ) -> Result<TurnOutcome, DriverError> {
        if self.inner.fail {
            return Err(DriverError::Cli("original CLI failure marker".into()));
        }
        self.inner.run_turn(prompt, limit).await
    }
    fn session_id(&self) -> Option<String> {
        self.inner.session_id()
    }
    fn is_alive(&mut self) -> bool {
        self.inner.is_alive()
    }
    async fn stop(&mut self) {
        self.inner.stop().await;
        // Result was already selected. Deterministic writer loss before control persistence.
        self.handle.shutdown().await.unwrap();
        self.inner
            .sink
            .system(self.inner.seat, "unconfirmed after writer loss");
    }
}

#[tokio::test]
async fn writer_loss_during_control_preserves_cli_error_and_drains_captures() {
    for fail in [true, false] {
        let (_root, demo) = setup().await;
        let outbox = demo.outbox.clone();
        let mut driver = WriterLossOnStopCli {
            inner: FakeCli {
                url: demo.mcp.url(demo.seat).unwrap(),
                sink: demo.sink.clone(),
                seat: demo.seat,
                fail,
                stopped: false,
                drain_pending: false,
            },
            handle: demo.handle.clone(),
        };
        let error = tokio::time::timeout(Duration::from_secs(30), demo.play(&mut driver, 1))
            .await
            .expect("control loss must not prevent bounded drain")
            .unwrap_err();
        assert!(driver.inner.stopped);
        if fail {
            assert!(error.contains("original CLI failure marker"));
            assert!(error.contains("recording seat failure failed"));
        }
        assert!(error.contains("campaign pause failed"));
        assert!(error.contains("unconfirmed captures saved"));
        assert!(error.contains(&outbox.display().to_string()));
        let saved = tokio::fs::read_to_string(outbox).await.unwrap();
        assert!(saved.contains("unconfirmed after writer loss"));
        let cleanup = demo.shutdown().await;
        let combined = cna_play::combine_results(Err(error), cleanup).unwrap_err();
        if fail {
            assert!(combined.contains("original CLI failure marker"));
        }
        assert!(combined.contains("unconfirmed captures saved"));
    }
}

async fn configured(config: cna_play::config::LaunchConfig) -> (tempfile::TempDir, Demo) {
    let root = tempfile::tempdir().unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        config,
    )
    .await
    .unwrap();
    (root, demo)
}
#[tokio::test]
async fn cna_binding_uses_real_observation_and_current_action_schema() {
    cna_binding_fixture(true).await;
}
#[tokio::test]
#[ignore = "slow: drain complete commander setup through the real scoped MCP bridge"]
async fn cna_commander_setup_uses_real_observation_and_current_action_schema() {
    cna_binding_fixture(false).await;
}
async fn cna_binding_fixture(movement: bool) {
    use cna_play::config::{GameKind, LaunchConfig};
    let config = LaunchConfig::resolve(
        GameKind::Cna,
        &[
            if movement {
                "axis.front_line=claude:haiku"
            } else {
                "axis.commander=claude:haiku"
            }
            .into(),
            if movement {
                "*=scripted:pass_when_possible"
            } else {
                "*=scripted:legal_random"
            }
            .into(),
            "axis.logistics=scripted:legal_random".into(),
        ],
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let demo = if movement {
        support::movement_demo(root.path(), &repo, config).await
    } else {
        Demo::with_config(
            root.path(),
            &repo.join("data"),
            &repo.join("web/dist"),
            config,
        )
        .await
        .unwrap()
    };
    let meta = demo
        .handle
        .projection(cna_core::visibility::Perspective::Operator)
        .meta;
    assert_eq!(meta.scenario_id, "graziani");
    assert_eq!(meta.rules_profile, "cna-2021-dev");
    assert!(!demo.prompts.system_prompt(demo.seat).contains("sandbox-v1"));
    demo.handle
        .write_notebook(
            demo.seat,
            cna_seats::memory::WriteMode::Replace,
            "Persisted commander plan",
        )
        .await
        .unwrap();
    // Large setup rosters exceed a small paid probe's 40-call allocation. This
    // offline fixture has no real CLI and uses a separate own-seat/epoch MCP
    // endpoint with no quota cap; production router budgets stay unchanged.
    let shared = std::sync::Arc::new(demo.handle.clone());
    let fixture_router = std::sync::Arc::new(cna_seats::mcp::ToolRouter::new(
        shared.clone(),
        shared,
        &[demo.seat],
    ));
    let fixture_mcp = cna_seats::mcp::McpServer::start(
        fixture_router,
        vec![cna_seats::mcp::SeatEndpoint {
            seat: demo.seat,
            epoch: demo.epoch,
            instructions: demo.prompts.system_prompt(demo.seat),
        }],
    )
    .await
    .unwrap();
    let prompt = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let mut driver = TrackedCli {
        inner: FakeCli {
            url: fixture_mcp.url(demo.seat).unwrap(),
            sink: demo.sink.clone(),
            seat: demo.seat,
            fail: false,
            stopped: false,
            drain_pending: !movement,
        },
        hang: false,
        stopped: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        prompt: Some(prompt.clone()),
    };
    demo.play(&mut driver, 1).await.unwrap();
    assert!(prompt.lock().unwrap().contains("Persisted commander plan"));
    let rows = demo.transcript();
    assert!(rows.iter().any(|m| matches!(
        m,
        ServerMessage::Transcript {
            entry: TranscriptEntry::DecisionSubmitted { .. },
            ..
        }
    )));
    assert!(rows.iter().any(|m| {
        match m {
            ServerMessage::Transcript {
                entry:
                    TranscriptEntry::ToolResult {
                        detail: Some(value),
                        ..
                    },
                ..
            } => value["request"]["kind"]
                .as_str()
                .is_some_and(|kind| kind.starts_with("cna.")),
            _ => false,
        }
    }));
    assert!(matches!(
        demo.handle.status(),
        cna_server::CampaignStatus::Paused | cna_server::CampaignStatus::Finished { .. }
    ));
    fixture_mcp.shutdown();
    demo.shutdown().await.unwrap();
}
#[tokio::test]
#[ignore = "slow: whole campaign"]
async fn cna_scripted_only_runs_and_recovers_without_any_driver_endpoint() {
    use cna_play::config::{GameKind, LaunchConfig};
    let config = LaunchConfig::resolve(GameKind::Cna, &["*=scripted:legal_random".into()]).unwrap();
    let (root, demo) = configured(config).await;
    assert!(demo.epochs.is_empty());
    assert!(demo.mcp.url(demo.seat).is_none());
    // Completed green CI run37584278878 measured184.100s (4868commands).
    // Test-only370s hang guard is about2x that standalone campaign duration.
    // Production scripted and paid limits remain unchanged.
    let mut seats: Vec<_> = cna_core::ids::SeatId::all()
        .map(|seat| (seat, demo.handle.watch_seat(seat)))
        .collect();
    demo.handle.pause(false).await.unwrap();
    let started = tokio::time::Instant::now();
    let mut status = demo.handle.watch_status();
    let result = tokio::time::timeout(Duration::from_secs(370), async {
        loop {
            match status.borrow_and_update().clone() {
                cna_server::CampaignStatus::Running => {}
                cna_server::CampaignStatus::Finished { .. } => return Ok::<_, String>(()),
                other => return Err(format!("scripted fixture stopped: {other:?}")),
            }
            for (seat, state) in &seats {
                let state = state.borrow();
                if state.binding.paused {
                    return Err(format!(
                        "scripted fixture seat {seat} paused: {:?}",
                        state.binding.failure
                    ));
                }
            }
            // A failed seat need not change Running status. Watch the binding
            // publications themselves; stream events can precede those publications.
            let mut changes: futures_util::stream::FuturesUnordered<_> =
                seats.iter_mut().map(|(_, state)| state.changed()).collect();
            tokio::select! {
                changed = status.changed() => changed.map_err(|e| e.to_string())?,
                Some(changed) = changes.next() => changed.map_err(|e| e.to_string())?,
            }
        }
    })
    .await
    .map_err(|_| {
        format!(
            "scripted fixture hang guard elapsed; metrics {:?}",
            demo.handle.runtime_metrics()
        )
    })
    .and_then(|r| r);
    if let Err(error) = result {
        let cleanup = demo.shutdown().await;
        cna_play::combine_results(Err(error), cleanup).unwrap();
        unreachable!();
    }
    // Write directly so a successful libtest run retains this calibration sample in CI logs.
    use std::io::Write as _;
    writeln!(
        std::io::stdout(),
        "CNA_PLAY_SCRIPTED_COMPLETE seconds={:.3} commands={}",
        started.elapsed().as_secs_f64(),
        demo.handle.runtime_metrics().committed_commands
    )
    .unwrap();
    assert!(matches!(
        demo.handle.status(),
        cna_server::CampaignStatus::Finished { .. }
    ));
    let id = demo.campaign_id();
    let expected = demo
        .handle
        .projection(cna_core::visibility::Perspective::Operator);
    demo.shutdown().await.unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let recovered = cna_server::campaigns::recover(
        &root.path().join(format!("{id}.sqlite")),
        &repo.join("data"),
    )
    .unwrap();
    assert_eq!(
        recovered.projection(cna_core::visibility::Perspective::Operator),
        expected
    );
    recovered.shutdown().await.unwrap();
}

struct TrackedCli {
    inner: FakeCli,
    hang: bool,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    prompt: Option<std::sync::Arc<std::sync::Mutex<String>>>,
}
#[async_trait]
impl SeatDriver for TrackedCli {
    fn kind(&self) -> CliKind {
        self.inner.kind()
    }
    async fn start(&mut self, r: Option<&str>) -> Result<SessionInfo, DriverError> {
        self.inner.start(r).await
    }
    async fn run_turn(&mut self, p: &str, l: Duration) -> Result<TurnOutcome, DriverError> {
        if let Some(prompt) = &self.prompt {
            *prompt.lock().unwrap() = p.into();
        }
        if self.hang {
            std::future::pending().await
        } else {
            self.inner.run_turn(p, l).await
        }
    }
    fn session_id(&self) -> Option<String> {
        self.inner.session_id()
    }
    fn is_alive(&mut self) -> bool {
        self.inner.is_alive()
    }
    async fn stop(&mut self) {
        self.inner.stop().await;
        self.stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}
#[tokio::test]
async fn bounded_peer_completion_or_failure_stops_all_sessions_without_fallbacks() {
    use cna_play::config::{GameKind, LaunchConfig};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    for fail in [false, true] {
        let config = LaunchConfig::resolve(
            GameKind::Sandbox,
            &[
                "axis.commander=claude:haiku".into(),
                "commonwealth.commander=claude:haiku".into(),
            ],
        )
        .unwrap();
        let (_root, demo) = configured(config).await;
        let flags = [
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        ];
        let seats: Vec<_> = demo.epochs.keys().copied().collect();
        assert_ne!(demo.mcp.url(seats[0]), demo.mcp.url(seats[1]));
        let mut drivers: Vec<_> = seats
            .iter()
            .enumerate()
            .map(|(index, seat)| {
                (
                    *seat,
                    Box::new(TrackedCli {
                        inner: FakeCli {
                            url: demo.mcp.url(*seat).unwrap(),
                            sink: demo.sink.clone(),
                            seat: *seat,
                            fail: index == 0 && fail,
                            stopped: false,
                            drain_pending: false,
                        },
                        hang: index == 1,
                        stopped: flags[index].clone(),
                        prompt: None,
                    }) as Box<dyn SeatDriver>,
                )
            })
            .collect();
        let result =
            tokio::time::timeout(Duration::from_secs(30), demo.play_sessions(&mut drivers, 1))
                .await
                .expect("bounded peer must not idle until wall timeout");
        assert_eq!(result.is_err(), fail);
        assert!(flags.iter().all(|flag| flag.load(Ordering::SeqCst)));
        assert!(demo.handle.seat(seats[1]).binding.failure.is_none());
        assert!(matches!(
            demo.handle.status(),
            cna_server::CampaignStatus::Paused
        ));
        assert!(
            demo.transcript_for(seats[1])
                .unwrap()
                .iter()
                .all(|m| !matches!(
                    m,
                    ServerMessage::Transcript {
                        entry: TranscriptEntry::DecisionSubmitted { .. },
                        ..
                    }
                ))
        );
        demo.shutdown().await.unwrap();
    }
}

struct PairedCli {
    inner: FakeCli,
    answered: std::sync::Arc<tokio::sync::Barrier>,
}
#[async_trait]
impl SeatDriver for PairedCli {
    fn kind(&self) -> CliKind {
        self.inner.kind()
    }
    async fn start(&mut self, r: Option<&str>) -> Result<SessionInfo, DriverError> {
        self.inner.start(r).await
    }
    async fn run_turn(&mut self, p: &str, l: Duration) -> Result<TurnOutcome, DriverError> {
        let result = self.inner.run_turn(p, l).await;
        // Both seats must actually answer through their own endpoints before either
        // completes its bounded allocation and triggers sibling cancellation.
        self.answered.wait().await;
        result
    }
    fn session_id(&self) -> Option<String> {
        self.inner.session_id()
    }
    fn is_alive(&mut self) -> bool {
        self.inner.is_alive()
    }
    async fn stop(&mut self) {
        self.inner.stop().await;
    }
}
#[tokio::test]
async fn two_active_seats_submit_through_separate_scoped_endpoints() {
    use cna_play::config::{GameKind, LaunchConfig};
    let config = LaunchConfig::resolve(
        GameKind::Sandbox,
        &[
            "axis.commander=claude:haiku".into(),
            "commonwealth.commander=claude:haiku".into(),
        ],
    )
    .unwrap();
    let (_root, demo) = configured(config).await;
    let answered = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let seats: Vec<_> = demo.epochs.keys().copied().collect();
    let mut drivers: Vec<_> = seats
        .iter()
        .map(|seat| {
            (
                *seat,
                Box::new(PairedCli {
                    inner: FakeCli {
                        url: demo.mcp.url(*seat).unwrap(),
                        sink: demo.sink.clone(),
                        seat: *seat,
                        fail: false,
                        stopped: false,
                        drain_pending: false,
                    },
                    answered: answered.clone(),
                }) as Box<dyn SeatDriver>,
            )
        })
        .collect();
    tokio::time::timeout(Duration::from_secs(30), demo.play_sessions(&mut drivers, 1))
        .await
        .unwrap()
        .unwrap();
    for seat in seats {
        let rows = demo.transcript_for(seat).unwrap();
        assert!(rows.iter().all(|m| matches!(m, ServerMessage::Transcript { seat: owner, .. } if owner == &seat.to_string())));
        assert_eq!(
            rows.iter()
                .filter(|m| matches!(
                    m,
                    ServerMessage::Transcript {
                        entry: TranscriptEntry::DecisionSubmitted { .. },
                        ..
                    }
                ))
                .count(),
            1
        );
        assert!(rows.iter().any(|m| matches!(
            m,
            ServerMessage::Transcript {
                entry: TranscriptEntry::ToolResult { ok: true, .. },
                ..
            }
        )));
    }
    demo.shutdown().await.unwrap();
}

#[test]
fn measured_cna_haiku_fixture_keeps_real_decision_and_notebook_tools_paired() {
    use std::collections::BTreeSet;
    let fixture = include_str!("fixtures/claude_cna_transcript.jsonl");
    assert!(!fixture.contains("@gmail.com"));
    assert!(!fixture.contains("/mcp/"));
    assert!(!fixture.contains("#cap="));
    let mut pending = BTreeSet::new();
    let mut calls = 0;
    let mut decisions = 0;
    let mut last_seq = 0;
    let mut notebook_written = false;
    let mut cna_window = false;
    let mut count = 0;
    for (index, line) in fixture.lines().enumerate() {
        let ServerMessage::Transcript {
            seat,
            tseq,
            game_seq,
            entry,
            ..
        } = serde_json::from_str(line).unwrap()
        else {
            panic!("not a transcript")
        };
        count += 1;
        assert_eq!(seat, "axis.commander");
        assert_eq!(tseq, index as u64 + 1);
        assert!(game_seq >= last_seq);
        last_seq = game_seq;
        match entry {
            TranscriptEntry::ToolCall { call_id, tool, .. } => {
                assert!(pending.insert(call_id));
                calls += 1;
                notebook_written |= tool == "notebook_write";
            }
            TranscriptEntry::ToolResult {
                call_id,
                ok,
                detail,
                ..
            } => {
                assert!(ok);
                assert!(pending.remove(&call_id));
                if let Some(detail) = detail {
                    cna_window |= detail["request"]["kind"] == "cna.initiative_declaration";
                }
            }
            TranscriptEntry::DecisionSubmitted { decision_id, .. } => {
                assert_eq!(decision_id, "d1");
                decisions += 1;
            }
            _ => {}
        }
    }
    assert!(pending.is_empty());
    assert_eq!((count, calls, decisions), (18, 5, 1));
    assert!(notebook_written && cna_window);
    assert!(last_seq > 0);
}

#[tokio::test]
async fn bounded_scripted_cna_progress_recovers_without_any_cli_endpoint() {
    use cna_play::config::{GameKind, LaunchConfig};
    let config =
        LaunchConfig::resolve(GameKind::Cna, &["*=scripted:pass_when_possible".into()]).unwrap();
    let (root, demo) = configured(config).await;
    assert!(demo.epochs.is_empty());
    assert!(demo.mcp.url(demo.seat).is_none());
    let accepted = || {
        demo.transcript()
            .iter()
            .filter(|row| {
                matches!(
                    row,
                    ServerMessage::Transcript {
                        entry: TranscriptEntry::DecisionSubmitted { .. },
                        ..
                    }
                )
            })
            .count()
    };
    demo.handle.pause(false).await.unwrap();
    let progress = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            if accepted() >= 2
                || matches!(
                    demo.handle.status(),
                    cna_server::CampaignStatus::Finished { .. }
                )
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    let pause = if matches!(demo.handle.status(), cna_server::CampaignStatus::Running) {
        demo.handle.pause(true).await
    } else {
        Ok(())
    };
    let accepted_count = accepted();
    let id = demo.campaign_id();
    let expected = demo
        .handle
        .projection(cna_core::visibility::Perspective::Operator);
    let cleanup = demo.shutdown().await;
    progress.expect("bounded scripted progress");
    pause.unwrap();
    cleanup.unwrap();
    assert!(accepted_count >= 2, "no substantive scripted decisions");
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let recovered = cna_server::campaigns::recover(
        &root.path().join(format!("{id}.sqlite")),
        &repo.join("data"),
    )
    .unwrap();
    assert_eq!(
        recovered.projection(cna_core::visibility::Perspective::Operator),
        expected
    );
    recovered.shutdown().await.unwrap();
}

#[test]
fn measured_durable_haiku_fixture_preserves_resume_and_corrected_movement() {
    use std::collections::BTreeSet;
    let fixture = include_str!("fixtures/claude_durable_transcript.jsonl");
    for secret in ["@gmail.com", "/mcp/", "#cap=", "Bearer "] {
        assert!(!fixture.contains(secret));
    }
    let mut pending = BTreeSet::new();
    let (mut calls, mut decisions, mut failures, mut notes) = (0, 0, 0, 0);
    let (mut started, mut resumed) = (false, false);
    let mut last_seq = 0;
    for (index, line) in fixture.lines().enumerate() {
        let ServerMessage::Transcript {
            seat,
            tseq,
            game_seq,
            entry,
            ..
        } = serde_json::from_str(line).unwrap()
        else {
            panic!("not a transcript")
        };
        assert_eq!(seat, "axis.front_line");
        assert_eq!(tseq, index as u64 + 1);
        assert!(game_seq >= last_seq);
        last_seq = game_seq;
        match entry {
            TranscriptEntry::ToolCall { call_id, tool, .. } => {
                assert!(pending.insert(call_id));
                calls += 1;
                notes += usize::from(tool == "notebook_write");
            }
            TranscriptEntry::ToolResult {
                call_id,
                ok,
                summary,
                ..
            } => {
                assert!(pending.remove(&call_id));
                if !ok {
                    failures += 1;
                    assert!(summary.contains("only the requested decision revision"));
                }
            }
            TranscriptEntry::DecisionSubmitted {
                decision_id,
                summary,
            } => {
                decisions += 1;
                assert_eq!(decision_id, format!("axis.front_line-{decisions}"));
                assert!(summary.contains("C3318"));
                assert!(summary.contains("it.gruppo_maletti."));
            }
            TranscriptEntry::System { text } => {
                started |= text.starts_with("started session <session>");
                resumed |= text.starts_with("resumed session <session>");
            }
            _ => {}
        }
    }
    assert!(pending.is_empty());
    assert_eq!(
        (fixture.lines().count(), calls, decisions, failures, notes),
        (54, 15, 2, 1, 2)
    );
    assert!(started && resumed);
    let accounting: Value =
        serde_json::from_str(include_str!("fixtures/claude_durable_accounting.json")).unwrap();
    assert_eq!(accounting["callsCharged"], calls);
    assert_eq!(accounting["incompleteTurns"], 1);
    assert_eq!(accounting["stages"]["GT1:OpStage1"]["completed"], decisions);
    assert_eq!(accounting["contextTokens"], 23357);
    assert!(
        accounting["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool.as_str().unwrap().starts_with("mcp__cna__"))
    );
    let cost = accounting["reportedCostUsd"].as_f64().unwrap();
    assert!((cost - (0.0550391 + 0.0459118)).abs() < 0.0000001);
}
