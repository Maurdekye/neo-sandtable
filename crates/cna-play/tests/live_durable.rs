//! Opt-in proof: two small model turns in one logical session, with an intervening launcher restart.
use async_trait::async_trait;
use cna_play::{
    Demo,
    config::{GameKind, LaunchConfig, SessionLimits},
};
use cna_seats::{
    driver::{
        CliKind, DriverError, SeatDriver, SessionInfo, SessionTelemetry, TurnOutcome,
        claude::{ClaudeConfig, ClaudeDriver},
    },
    run::PromptBuilder,
};
use std::{path::PathBuf, time::Duration};
use tokio::sync::watch;
struct OneTurn {
    inner: ClaudeDriver,
    stop: watch::Sender<bool>,
    turns: usize,
}
#[async_trait]
impl SeatDriver for OneTurn {
    fn kind(&self) -> CliKind {
        CliKind::Claude
    }
    fn telemetry(&self) -> SessionTelemetry {
        self.inner.telemetry()
    }
    async fn start(&mut self, id: Option<&str>) -> Result<SessionInfo, DriverError> {
        self.inner.start(id).await
    }
    async fn run_turn(
        &mut self,
        prompt: &str,
        limit: Duration,
    ) -> Result<TurnOutcome, DriverError> {
        self.turns += 1;
        if self.turns > 1 {
            let _ = self.stop.send(true);
            return std::future::pending().await;
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
    }
}
fn driver(
    demo: &Demo,
    root: &std::path::Path,
    profile: &std::path::Path,
    email: &str,
    stop: watch::Sender<bool>,
) -> OneTurn {
    OneTurn {
        inner: ClaudeDriver::new(
            ClaudeConfig {
                seat: demo.seat,
                exe: None,
                model: "haiku".into(),
                config_dir: Some(profile.into()),
                expected_email: Some(email.into()),
                sandbox: root.join("sandbox").join(demo.seat.to_string()),
                run_dir: root.join("run").join(demo.seat.to_string()),
                mcp_url: demo.mcp.url(demo.seat).unwrap(),
                system_prompt: demo.prompts.system_prompt(demo.seat),
                effort: None,
                context_window: Some(100_000),
            },
            demo.sink.clone(),
        ),
        stop,
        turns: 0,
    }
}
#[tokio::test]
async fn live_haiku_durable_resume_is_opt_in() {
    if std::env::var("CNA_LIVE_CLI_TESTS").as_deref() != Ok("1") {
        return;
    }
    let profile =
        PathBuf::from(std::env::var_os("CNA_CLAUDE_CONFIG_DIR").expect("claude-5 profile"));
    let email = std::env::var("CNA_CLAUDE_EMAIL").expect("claude-5 expected email");
    let root = tempfile::Builder::new()
        .prefix("cna-durable-proof-")
        .tempdir()
        .unwrap()
        .keep();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut config = LaunchConfig::resolve(
        GameKind::Cna,
        &[
            "axis.front_line=claude:haiku".into(),
            "*=scripted:pass_when_possible".into(),
        ],
    )
    .unwrap();
    config.session = Some(SessionLimits {
        wall_seconds: 150,
        turn_seconds: 45,
        context_tokens: 100_000,
        recoveries: 1,
    });
    config.max_turns = 3;
    config.tool_calls = 16;
    let demo = Demo::with_config(
        &root.join("campaigns"),
        &repo.join("data"),
        &repo.join("web/dist"),
        config,
    )
    .await
    .unwrap();
    let path = root
        .join("campaigns")
        .join(format!("{}.sqlite", demo.campaign_id()));
    // Free scripted setup/logistics advancement; no CLI has been constructed or started.
    // Start the paid lifetime budget at the first actual movement request.
    let mut windows = demo.handle.watch_seat(demo.seat);
    let prepared = async {
        demo.handle.pause(false).await.map_err(|e| e.to_string())?;
        tokio::time::timeout(Duration::from_secs(600), async {
            loop {
                if !windows.borrow_and_update().pending.is_empty() {
                    break;
                }
                windows.changed().await.map_err(|e| e.to_string())?;
            }
            Ok::<(), String>(())
        })
        .await
        .map_err(|_| "unpaid movement preparation timed out".to_string())??;
        demo.handle.pause(true).await.map_err(|e| e.to_string())
    }
    .await;
    if let Err(error) = prepared {
        let cleanup = demo.shutdown().await;
        cna_play::combine_results(Err(error), cleanup).unwrap();
        unreachable!();
    }
    assert!(
        demo.handle
            .seat(demo.seat)
            .pending
            .iter()
            .any(|p| p.kind.contains("movement"))
    );
    let (stop, rx) = watch::channel(false);
    let fake = driver(&demo, &root, &profile, &email, stop);
    let first_result = demo
        .play_durable(&mut [(demo.seat, Box::new(fake))], rx)
        .await;
    let first = demo.journal.as_ref().unwrap().snapshot().unwrap().seats[&demo.seat].clone();
    let first_cleanup = demo.shutdown().await;
    cna_play::combine_results(first_result, first_cleanup).expect("first short turn");
    assert_eq!(first.turns, 2);
    assert_eq!(first.stages.values().map(|s| s.completed).sum::<u64>(), 1);
    let id = first.session.unwrap().session_id;
    let demo = Demo::resume(&path, &repo.join("data"), &repo.join("web/dist"))
        .await
        .unwrap();
    let (_stop, rx) = watch::channel(false);
    let (fake_stop, _) = watch::channel(false);
    let fake = driver(&demo, &root, &profile, &email, fake_stop);
    let second_result = demo
        .play_durable(&mut [(demo.seat, Box::new(fake))], rx)
        .await;
    let state = demo.journal.as_ref().unwrap().snapshot().unwrap();
    let record = state.seats[&demo.seat].clone();
    let transcript = demo.transcript();
    let text = transcript
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n")
        + "\n";
    std::fs::write(root.join("transcript.jsonl"), text).unwrap();
    std::fs::write(
        root.join("accounting.json"),
        serde_json::to_string_pretty(&state).unwrap(),
    )
    .unwrap();
    println!("D8 proof directory: {}", root.display());
    println!("Board: {}", demo.board_url());
    println!("D8 accounting: {:?}", record.stages);
    let cleanup = demo.shutdown().await;
    assert!(
        second_result
            .as_ref()
            .is_err_and(|e| e.contains("turn budget exhausted")),
        "{second_result:?}"
    );
    cleanup.unwrap();
    assert_eq!(record.session.unwrap().session_id, id);
    assert_eq!(record.turns, 3);
    assert_eq!(record.stages.values().map(|s| s.completed).sum::<u64>(), 2);
    assert!(record.calls <= 16);
    assert!(
        record
            .telemetry
            .tools
            .iter()
            .all(|t| t.starts_with("mcp__cna__"))
    );
    assert!(!record.telemetry.tools.is_empty());
}
