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
}
impl FakeCli {
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
        self.call("observe", json!({}), &mut e, "observe").await;
        let pending: Value = self
            .call(
                "describe_actions",
                json!({
                    "decision_id": self.pending_id().await
                }),
                &mut e,
                "actions",
            )
            .await;
        // Choose first from the authorized initiative options.
        let id = pending["request"]["id"].as_str().unwrap();
        self.call(
            "submit",
            json!({"decision_id":id,"action":"first"}),
            &mut e,
            "submit",
        )
        .await;
        e.emit(TranscriptEntry::AssistantText {
            text: "Initiative submitted.".into(),
        });
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
impl FakeCli {
    async fn pending_id(&self) -> String {
        let mut e = EntryEmitter::new(self.seat, self.sink.clone());
        let observation = self.call("observe", json!({}), &mut e, "pending").await;
        observation["pending_decisions"][0]["decision_id"]
            .as_str()
            .unwrap()
            .into()
    }
}
async fn setup() -> (tempfile::TempDir, Demo) {
    let root = tempfile::tempdir().unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let demo = Demo::new(root.path(), &repo.join("data"), &repo.join("web/dist")).await;
    (root, demo)
}

#[tokio::test]
async fn mcp_answer_reaches_persistent_websocket_transcript() {
    let (_root, demo) = setup().await;
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
    // Establish subscription before generating CLI transcript entries.
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
    let mut driver = FakeCli {
        url: demo.mcp.url(demo.seat).unwrap(),
        sink: demo.sink.clone(),
        seat: demo.seat,
        fail: false,
        stopped: false,
    };
    demo.play(&mut driver, 1).await.unwrap();
    assert!(driver.stopped);
    let rows = demo.transcript();
    assert!(rows.iter().any(|m| matches!(
        m,
        ServerMessage::Transcript {
            entry: TranscriptEntry::DecisionSubmitted { .. },
            ..
        }
    )));
    let expected: Vec<_> = rows
        .iter()
        .map(|m| match m {
            ServerMessage::Transcript { tseq, .. } => *tseq,
            _ => unreachable!(),
        })
        .collect();
    let actual = tokio::time::timeout(Duration::from_secs(5), async {
        let mut found = vec![];
        while found.len() < expected.len() {
            if let Message::Text(t) = socket.next().await.unwrap().unwrap()
                && let ServerMessage::Transcript { seat, tseq, .. } =
                    serde_json::from_str(&t).unwrap()
                && seat == demo.seat.to_string()
            {
                found.push(tseq);
            }
        }
        found
    })
    .await
    .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(expected, (1..=expected.len() as u64).collect::<Vec<_>>());
    socket.close(None).await.unwrap();
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
    let error = tokio::time::timeout(Duration::from_secs(8), demo.shutdown())
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
        },
        demo.sink.clone(),
    );
    let handle = demo.handle.clone();
    let seat = demo.seat;
    let (result, ()) = tokio::join!(demo.play(&mut driver, 1), async {
        let pid = tokio::time::timeout(Duration::from_secs(5), async {
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
        tokio::time::timeout(Duration::from_secs(3), async {
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
    let cleanup = tokio::time::timeout(Duration::from_secs(8), demo.shutdown())
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
            },
            handle: demo.handle.clone(),
        };
        let error = tokio::time::timeout(Duration::from_secs(8), demo.play(&mut driver, 1))
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
