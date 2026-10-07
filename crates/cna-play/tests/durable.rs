mod support;
use async_trait::async_trait;
use cna_core::{decision::ActionSchema, ids::SeatId};
use cna_play::{
    Demo,
    config::{GameKind, LaunchConfig, SessionLimits},
};
use cna_seats::{
    driver::{CliKind, DriverError, SeatDriver, SessionInfo, SessionTelemetry, TurnOutcome, Usage},
    memory::WriteMode,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

#[derive(Default)]
struct Trace {
    starts: Vec<Option<String>>,
    prompts: Vec<String>,
    cost: f64,
    kinds: Vec<String>,
}
struct Inert {
    handle: cna_server::actor::CampaignHandle,
    url: String,
    seat: SeatId,
    id: Option<String>,
    trace: Arc<Mutex<Trace>>,
    stop: Option<watch::Sender<bool>>,
    stop_after: usize,
    turns: usize,
    die_after_submit: bool,
    missing_resume: bool,
    resumed: bool,
    context: u64,
}
impl Inert {
    async fn call(&self, tool: &str, args: Value) -> Result<Value, DriverError> {
        let response:Value=reqwest::Client::new().post(&self.url).json(&json!({"jsonrpc":"2.0","id":uuid::Uuid::new_v4().to_string(),"method":"tools/call","params":{"name":tool,"arguments":args}})).send().await.map_err(|e|DriverError::Protocol(e.to_string()))?.json().await.map_err(|e|DriverError::Protocol(e.to_string()))?;
        let result = &response["result"];
        let text = result["content"][0]["text"]
            .as_str()
            .ok_or_else(|| DriverError::Protocol(response.to_string()))?;
        if result["isError"] == true {
            return Err(DriverError::Cli(text.into()));
        }
        serde_json::from_str(text).map_err(|e| DriverError::Protocol(e.to_string()))
    }
}
#[async_trait]
impl SeatDriver for Inert {
    fn kind(&self) -> CliKind {
        CliKind::Claude
    }
    fn telemetry(&self) -> SessionTelemetry {
        SessionTelemetry {
            model: Some("haiku".into()),
            cli_version: Some("inert".into()),
            tools: vec!["mcp__cna__observe".into()],
            context_tokens: Some(self.context),
            compactions: u64::from(self.turns >= 3),
        }
    }
    async fn start(&mut self, resume: Option<&str>) -> Result<SessionInfo, DriverError> {
        self.trace
            .lock()
            .unwrap()
            .starts
            .push(resume.map(str::to_string));
        self.resumed = resume.is_some();
        self.id = Some(
            resume
                .map(str::to_string)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        );
        Ok(SessionInfo {
            session_id: self.id.clone().unwrap(),
            model: Some("haiku".into()),
            cli_version: Some("inert".into()),
            resumed: self.resumed,
        })
    }
    #[allow(clippy::float_arithmetic)]
    async fn run_turn(&mut self, prompt: &str, _: Duration) -> Result<TurnOutcome, DriverError> {
        self.turns += 1;
        self.trace.lock().unwrap().prompts.push(prompt.into());
        if self.turns > self.stop_after
            && let Some(stop) = &self.stop
        {
            let _ = stop.send(true);
            return std::future::pending().await;
        }
        if self.missing_resume && self.resumed {
            self.missing_resume = false;
            return Err(DriverError::Died(
                "No conversation found with session ID: unavailable".into(),
            ));
        }
        let requested = self
            .handle
            .seat(self.seat)
            .pending
            .first()
            .cloned()
            .unwrap();
        self.trace
            .lock()
            .unwrap()
            .kinds
            .push(requested.kind.clone());
        self.call("observe", json!({})).await?;
        let described = self
            .call("describe_actions", json!({"decision_id":requested.id}))
            .await?;
        let request: cna_core::decision::DecisionRequest =
            serde_json::from_value(described["request"].clone()).unwrap();
        let action = if request.space.pass.is_some() {
            Value::Null
        } else {
            first(&request.space.schema)
        };
        request.space.check(&action).map_err(DriverError::Cli)?;
        self.call(
            "submit",
            json!({"decision_id":request.id,"revision":request.revision,"action":action}),
        )
        .await?;
        if self.die_after_submit && self.turns == 2 {
            self.die_after_submit = false;
            return Err(DriverError::Died("simulated process exit".into()));
        }
        let cost = {
            let mut trace = self.trace.lock().unwrap();
            trace.cost += 0.01;
            trace.cost
        };
        Ok(TurnOutcome {
            ok: true,
            error: None,
            text: None,
            usage: Usage {
                cost_usd: Some(cost),
                input_tokens: Some(10),
                output_tokens: Some(5),
                ..Usage::default()
            },
            quota: vec![],
        })
    }
    fn session_id(&self) -> Option<String> {
        self.id.clone()
    }
    fn is_alive(&mut self) -> bool {
        true
    }
    async fn stop(&mut self) -> Result<(), cna_seats::driver::DriverError> {
        Ok(())
    }
}
fn first(schema: &ActionSchema) -> Value {
    match schema {
        ActionSchema::Choice { options } => json!(options[0].id),
        ActionSchema::Integer { min, .. } => json!(min),
        ActionSchema::Bool => json!(false),
        ActionSchema::Unit { among } => json!(among[0]),
        ActionSchema::Hex { among: Some(h) } => json!(h[0]),
        ActionSchema::Path { .. } => json!([]),
        ActionSchema::Record { fields } => Value::Object(
            fields
                .iter()
                .filter(|f| !f.optional)
                .map(|f| (f.name.clone(), first(&f.schema)))
                .collect(),
        ),
        ActionSchema::List { item, min, .. } => {
            Value::Array((0..*min).map(|_| first(item)).collect())
        }
        _ => panic!("no enumerated fixture answer"),
    }
}
fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn config(seat: &str) -> LaunchConfig {
    let mut c = LaunchConfig::resolve(
        GameKind::Cna,
        &[
            format!("{seat}=claude:haiku"),
            "*=scripted:pass_when_possible".into(),
        ],
    )
    .unwrap();
    c.session = Some(SessionLimits {
        wall_seconds: 600,
        // Inert MCP work is count-bounded; this is a hang guard, not a speed assertion.
        turn_seconds: 30,
        context_tokens: 100_000,
        recoveries: 3,
    });
    c.max_turns = 8192;
    c.tool_calls = 30_000;
    c
}
fn driver(demo: &Demo, trace: Arc<Mutex<Trace>>) -> Inert {
    Inert {
        handle: demo.handle.clone(),
        url: demo.mcp.url(demo.seat).unwrap(),
        seat: demo.seat,
        id: None,
        trace,
        stop: None,
        stop_after: usize::MAX,
        turns: 0,
        die_after_submit: false,
        missing_resume: false,
        resumed: false,
        context: 1000,
    }
}
#[tokio::test]
async fn real_cna_windows_resume_same_session_and_recover_notebook() {
    cna_resume(false).await;
}
#[tokio::test]
#[ignore = "slow: whole CNA campaign across OpStages"]
async fn real_cna_whole_campaign_preserves_session_across_opstages() {
    cna_resume(true).await;
}
async fn cna_resume(full: bool) {
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        config("axis.commander"),
    )
    .await
    .unwrap();
    let epoch = demo.epoch;
    let path = root.path().join(format!("{}.sqlite", demo.campaign_id()));
    demo.handle
        .write_notebook(demo.seat, WriteMode::Replace, "Persist the standing plan")
        .await
        .unwrap();
    let trace = Arc::new(Mutex::new(Trace::default()));
    let (stop, rx) = watch::channel(false);
    let mut fake = driver(&demo, trace.clone());
    fake.stop = Some(stop);
    fake.stop_after = 3;
    demo.play_durable(&mut [(demo.seat, Box::new(fake))], rx)
        .await
        .unwrap();
    let before = demo.journal.as_ref().unwrap().snapshot().unwrap().seats[&demo.seat].clone();
    assert_eq!(before.turns, 4);
    // Graceful cancellation now commits the incomplete usage snapshot immediately,
    // rather than leaving it for the next launcher recovery to discover.
    assert!(before.inflight.is_none());
    assert_eq!(before.incomplete_turns, 1);
    let usage = before.usage.as_ref().unwrap();
    assert_eq!(usage.revision, 4);
    assert_eq!(
        (usage.input_tokens, usage.output_tokens),
        (Some(30), Some(15))
    );
    let snapshots: Vec<_> = demo
        .transcript()
        .into_iter()
        .filter_map(|message| match message {
            cna_protocol::ServerMessage::Transcript {
                entry: cna_protocol::TranscriptEntry::UsageSnapshot(snapshot),
                ..
            } => Some(snapshot),
            _ => None,
        })
        .collect();
    assert_eq!(snapshots.len(), 4);
    assert_eq!(
        snapshots.iter().map(|s| s.revision).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    let last = snapshots.last().unwrap();
    assert_eq!(
        (last.attempts, last.completed, last.incomplete_turns),
        (4, 3, 1)
    );
    assert_eq!(
        (
            last.input_tokens,
            last.output_tokens,
            last.cache_read_tokens,
            last.cache_creation_tokens,
            last.reasoning_tokens
        ),
        (Some(30), Some(15), None, None, None)
    );
    if !full && let Some(path) = std::env::var_os("CNA_INERT_USAGE_FIXTURE") {
        let entries: Vec<_> = demo
            .transcript()
            .into_iter()
            .filter(|message| {
                matches!(
                    message,
                    cna_protocol::ServerMessage::Transcript {
                        entry: cna_protocol::TranscriptEntry::UsageSnapshot(_),
                        ..
                    }
                )
            })
            .collect();
        let fixture = json!({"source":"inert driver through real Graziani/dev server; synthetic usage, not provider measurements","paid_calls":0,"entries":entries});
        std::fs::write(path, serde_json::to_string_pretty(&fixture).unwrap()).unwrap();
    }
    assert!(before.usage.as_ref().unwrap().outbox.is_empty());
    assert_eq!(before.usage.as_ref().unwrap().acknowledged_revision, 4);
    let replay = demo.handle.replay();
    let own = cna_core::visibility::Perspective::Side(demo.seat.side);
    let side_snapshots: Vec<_> = replay
        .transcripts(own, demo.seat, 0)
        .unwrap()
        .into_iter()
        .filter_map(|message| match message {
            cna_protocol::ServerMessage::Transcript {
                entry: cna_protocol::TranscriptEntry::UsageSnapshot(snapshot),
                ..
            } => Some(snapshot),
            _ => None,
        })
        .collect();
    // Game alignment sequences belong to each audience, so compare payloads.
    assert_eq!(side_snapshots, snapshots);
    let enemy = match demo.seat.side {
        cna_core::ids::Side::Axis => cna_core::ids::Side::Commonwealth,
        cna_core::ids::Side::Commonwealth => cna_core::ids::Side::Axis,
    };
    assert!(
        replay
            .transcripts(cna_core::visibility::Perspective::Side(enemy), demo.seat, 0)
            .unwrap()
            .is_empty()
    );
    demo.shutdown().await.unwrap();
    let demo = Demo::resume(&path, &repo.join("data"), &repo.join("web/dist"))
        .await
        .unwrap();
    assert_eq!(demo.epoch, epoch);
    let recovered = demo.journal.as_ref().unwrap().snapshot().unwrap().seats[&demo.seat].clone();
    assert_eq!(
        (recovered.turns, recovered.calls),
        (before.turns, before.calls)
    );
    assert_eq!(recovered.incomplete_turns, before.incomplete_turns);
    let mut fake = driver(&demo, trace.clone());
    let (stop, rx) = watch::channel(false);
    if !full {
        fake.stop = Some(stop);
        fake.stop_after = 3;
    }
    demo.play_durable(&mut [(demo.seat, Box::new(fake))], rx)
        .await
        .unwrap();
    let after = demo.journal.as_ref().unwrap().snapshot().unwrap().seats[&demo.seat].clone();
    if full {
        assert!(after.stages.len() >= 3, "{:?}", after.stages.keys());
    }
    assert!(after.turns >= 6, "{:?}", after.stages);
    assert!(after.compactions >= 2);
    assert_eq!(
        after.session.unwrap().session_id,
        before.session.unwrap().session_id
    );
    {
        let trace = trace.lock().unwrap();
        assert_eq!(trace.starts.len(), 2);
        assert!(trace.starts[1].is_some());
        assert!(
            trace
                .prompts
                .iter()
                .filter(|p| p.contains("Persist the standing plan"))
                .count()
                >= 2
        );
    }
    demo.shutdown().await.unwrap();
}
#[tokio::test]
async fn death_after_committed_order_resumes_without_repeating_old_revision() {
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        config("axis.commander"),
    )
    .await
    .unwrap();
    let trace = Arc::new(Mutex::new(Trace::default()));
    let mut fake = driver(&demo, trace.clone());
    fake.die_after_submit = true;
    let (stop, rx) = watch::channel(false);
    fake.stop = Some(stop);
    fake.stop_after = 3;
    demo.play_durable(&mut [(demo.seat, Box::new(fake))], rx)
        .await
        .unwrap();
    let s = &demo.journal.as_ref().unwrap().snapshot().unwrap().seats[&demo.seat];
    assert_eq!(s.recoveries, 1);
    // One interrupted CLI result plus the intentionally cancelled final turn.
    assert_eq!(s.incomplete_turns, 2);
    assert!(s.inflight.is_none());
    assert!(s.turns >= 3, "{:?}", s.stages);
    assert_eq!(trace.lock().unwrap().starts.len(), 2);
    demo.shutdown().await.unwrap();
}
#[tokio::test]
async fn replaced_binding_cannot_resume_from_the_old_journal() {
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        config("axis.commander"),
    )
    .await
    .unwrap();
    let path = root.path().join(format!("{}.sqlite", demo.campaign_id()));
    demo.handle
        .handover(demo.seat, None, json!({"mode":"human"}))
        .await
        .unwrap();
    demo.shutdown().await.unwrap();
    assert!(
        Demo::resume(&path, &repo.join("data"), &repo.join("web/dist"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn unavailable_resume_reseeds_from_notebook_without_resetting_attempts() {
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        config("axis.commander"),
    )
    .await
    .unwrap();
    demo.handle
        .write_notebook(
            demo.seat,
            WriteMode::Replace,
            "Recovery notebook is authoritative",
        )
        .await
        .unwrap();
    let old = uuid::Uuid::new_v4().to_string();
    demo.journal
        .as_ref()
        .unwrap()
        .session(
            demo.seat,
            SessionInfo {
                session_id: old.clone(),
                model: Some("haiku".into()),
                cli_version: None,
                resumed: false,
            },
        )
        .unwrap();
    let trace = Arc::new(Mutex::new(Trace::default()));
    let mut fake = driver(&demo, trace.clone());
    fake.missing_resume = true;
    let (stop, rx) = watch::channel(false);
    fake.stop = Some(stop);
    fake.stop_after = 3;
    demo.play_durable(&mut [(demo.seat, Box::new(fake))], rx)
        .await
        .unwrap();
    let s = &demo.journal.as_ref().unwrap().snapshot().unwrap().seats[&demo.seat];
    assert_eq!(s.recoveries, 1);
    // One interrupted CLI result plus the intentionally cancelled final turn.
    assert_eq!(s.incomplete_turns, 2);
    assert!(s.inflight.is_none());
    assert_ne!(s.session.as_ref().unwrap().session_id, old);
    {
        let trace = trace.lock().unwrap();
        assert_eq!(trace.starts[0], Some(old));
        assert_eq!(trace.starts[1], None);
        assert!(trace.prompts[1].contains("Recovery notebook is authoritative"));
        assert!(trace.prompts[1].contains("unavailable"));
    }
    demo.shutdown().await.unwrap();
}
#[tokio::test]
async fn durable_call_budget_refuses_submit_and_pauses_without_a_fallback() {
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let mut config = config("axis.commander");
    config.tool_calls = 2;
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        config,
    )
    .await
    .unwrap();
    let fake = driver(&demo, Arc::new(Mutex::new(Trace::default())));
    let (_stop, rx) = watch::channel(false);
    let error = demo
        .play_durable(&mut [(demo.seat, Box::new(fake))], rx)
        .await
        .unwrap_err();
    assert!(error.contains("tool budget"), "{error}");
    assert!(demo.handle.seat(demo.seat).binding.paused);
    let s = &demo.journal.as_ref().unwrap().snapshot().unwrap().seats[&demo.seat];
    assert_eq!(s.calls, 2);
    assert_eq!(s.turns, 1);
    assert!(!demo.transcript().iter().any(|r| matches!(
        r,
        cna_protocol::ServerMessage::Transcript {
            entry: cna_protocol::TranscriptEntry::DecisionSubmitted { .. },
            ..
        }
    )));
    demo.shutdown().await.unwrap();
}

#[tokio::test]
async fn context_headroom_failure_stops_durable_siblings_without_failed_replacement() {
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let mut config = config("axis.commander");
    config.seats.insert(
        "commonwealth.commander".parse().unwrap(),
        cna_play::config::Controller::Claude("haiku".into()),
    );
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        config,
    )
    .await
    .unwrap();
    let other: SeatId = "commonwealth.commander".parse().unwrap();
    let mut a = driver(&demo, Arc::new(Mutex::new(Trace::default())));
    a.context = 99_000;
    let mut b = driver(&demo, Arc::new(Mutex::new(Trace::default())));
    b.seat = other;
    b.url = demo.mcp.url(other).unwrap();
    let (_stop, rx) = watch::channel(false);
    let error = demo
        .play_durable(&mut [(demo.seat, Box::new(a)), (other, Box::new(b))], rx)
        .await
        .unwrap_err();
    assert!(error.contains("context budget"), "{error}");
    assert!(demo.handle.seat(demo.seat).binding.paused);
    assert!(!demo.handle.seat(other).binding.paused);
    assert_eq!(
        demo.journal.as_ref().unwrap().snapshot().unwrap().seats[&demo.seat].calls,
        0
    );
    demo.shutdown().await.unwrap();
}

// Current scripted supply may restrict unit motion; these are explicit offered passes,
// exercising real movement windows rather than inventing a legal unit move.
#[tokio::test]
async fn bounded_real_cna_windows_keep_one_session_and_stage_accounting() {
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let demo = support::movement_demo(root.path(), &repo, config("axis.front_line")).await;
    assert!(
        demo.handle
            .seat(demo.seat)
            .pending
            .iter()
            .any(|p| p.kind == "cna.movement.orders")
    );
    let trace = Arc::new(Mutex::new(Trace::default()));
    let (stop, rx) = watch::channel(false);
    let mut inert = driver(&demo, trace.clone());
    inert.stop = Some(stop.clone());
    inert.stop_after = 2;
    let journal = demo.journal.as_ref().unwrap().clone();
    let seat = demo.seat;
    let checkpoint = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if journal.snapshot().is_ok_and(|s| {
                s.seats[&seat]
                    .stages
                    .values()
                    .map(|s| s.completed)
                    .sum::<u64>()
                    >= 2
            }) {
                let _ = stop.send(true);
                break;
            }
        }
    });
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        demo.play_durable(&mut [(seat, Box::new(inert))], rx),
    )
    .await
    .map_err(|_| "bounded movement run hang guard elapsed".to_string())
    .and_then(|r| r);
    checkpoint.abort();
    let _ = checkpoint.await;
    let record = demo.journal.as_ref().unwrap().snapshot().unwrap().seats[&seat].clone();
    let cleanup = demo.shutdown().await;
    cna_play::combine_results(result, cleanup).unwrap();
    let trace = trace.lock().unwrap();
    assert_eq!(trace.starts.len(), 1);
    assert_eq!(trace.kinds.len(), 2);
    assert_eq!(trace.kinds[0], "cna.movement.orders");
    assert!(trace.kinds.iter().all(|kind| kind.starts_with("cna.")));
    assert_eq!(record.stages.values().map(|s| s.completed).sum::<u64>(), 2);
    assert!(record.stages.keys().all(|s| s.contains("OpStage")));
    assert!((2..=3).contains(&record.turns));
    assert!(record.session.is_some());
}
