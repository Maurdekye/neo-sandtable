//! Inert session proof: real server and own-scoped MCP, no provider/CLI calls.
#![allow(clippy::float_arithmetic)]
use async_trait::async_trait;
use cna_core::{
    decision::{ActionSchema, DecisionRequest},
    ids::SeatId,
};
use cna_play::{
    Demo,
    config::{Command, parse_args},
};
use cna_protocol::{ServerMessage, TranscriptEntry};
use cna_seats::{
    driver::{
        CliKind, DriverError, EntryEmitter, EstimateBound, SeatDriver, SessionInfo, TurnOutcome,
        Usage, tool_result_entry,
    },
    transcript::TranscriptSink,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

#[derive(Default)]
struct Trace {
    active: usize,
    peak: usize,
    starts: BTreeMap<SeatId, Vec<(String, bool, f64)>>,
    decisions: BTreeMap<SeatId, u64>,
    phases: BTreeMap<SeatId, BTreeMap<String, u64>>,
    prompt_bytes: BTreeMap<SeatId, Vec<usize>>,
    observe_bytes: BTreeMap<SeatId, Vec<usize>>,
}
struct Fake {
    seat: SeatId,
    url: String,
    sink: TranscriptSink,
    id: Option<String>,
    alive: bool,
    cap: f64,
    cost: f64,
    model: String,
    trace: Arc<Mutex<Trace>>,
}
impl Fake {
    async fn call(&self, tool: &str, args: Value) -> Result<Value, DriverError> {
        let id = uuid::Uuid::new_v4().to_string();
        let mut emitter = EntryEmitter::new(self.seat, self.sink.clone());
        emitter.emit(TranscriptEntry::ToolCall {
            call_id: id.clone(),
            tool: tool.into(),
            args: args.clone(),
        });
        let response:Value=reqwest::Client::new().post(&self.url).json(&json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":args}})).send().await.map_err(|e|DriverError::Protocol(e.to_string()))?.json().await.map_err(|e|DriverError::Protocol(e.to_string()))?;
        let result = &response["result"];
        let text = result["content"][0]["text"]
            .as_str()
            .ok_or_else(|| DriverError::Protocol(response.to_string()))?;
        emitter.emit(tool_result_entry(id, result["isError"] != true, text));
        if result["isError"] == true {
            return Err(DriverError::Cli(text.into()));
        }
        serde_json::from_str(text).map_err(|e| DriverError::Protocol(e.to_string()))
    }
}
#[async_trait]
impl SeatDriver for Fake {
    fn kind(&self) -> CliKind {
        CliKind::Claude
    }
    fn estimate_bound(&self) -> Option<EstimateBound> {
        Some(EstimateBound {
            pinned_model: self.model.clone(),
            context_tokens: 100,
            output_tokens: 100,
            max_input_or_cache_write_usd_per_token: 0.0001,
            output_usd_per_token: 0.0001,
            evidence: "injected inert fixture, NOT a native/provider proof".into(),
        })
    }
    fn set_reported_cost_limit(&mut self, cap: f64) -> Result<(), DriverError> {
        assert!(!self.alive);
        assert!(cap > 0. && cap.is_finite());
        self.cap = cap;
        Ok(())
    }
    async fn start(&mut self, resume: Option<&str>) -> Result<SessionInfo, DriverError> {
        assert!(!self.alive);
        self.alive = true;
        let id = resume
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if let Some(old) = &self.id {
            assert_eq!(&id, old, "parking must resume the same native session");
        }
        self.id = Some(id.clone());
        let mut t = self.trace.lock().unwrap();
        t.active += 1;
        t.peak = t.peak.max(t.active);
        t.starts
            .entry(self.seat)
            .or_default()
            .push((id.clone(), resume.is_some(), self.cap));
        Ok(SessionInfo {
            session_id: id,
            model: Some(self.model.clone()),
            cli_version: Some("inert; no native process".into()),
            resumed: resume.is_some(),
        })
    }
    async fn run_turn(&mut self, prompt: &str, _: Duration) -> Result<TurnOutcome, DriverError> {
        assert!(self.alive);
        self.trace
            .lock()
            .unwrap()
            .prompt_bytes
            .entry(self.seat)
            .or_default()
            .push(prompt.len());
        let observed = self.call("observe", json!({})).await?;
        self.trace
            .lock()
            .unwrap()
            .observe_bytes
            .entry(self.seat)
            .or_default()
            .push(observed.to_string().len());
        let id = observed["pending_decisions"][0]["decision_id"].clone();
        let described = self
            .call("describe_actions", json!({"decision_id":id}))
            .await?;
        let request: DecisionRequest = serde_json::from_value(described["request"].clone())
            .map_err(|e| DriverError::Protocol(e.to_string()))?;
        assert_eq!(request.seat, self.seat);
        let phase = format!(
            "GT{}:{}",
            request.clock.game_turn,
            request
                .clock
                .op_stage
                .map(|n| format!("OpStage{n}"))
                .unwrap_or_else(|| request.clock.anchor.stage().into())
        );
        let action = if request.space.pass.is_some() {
            Value::Null
        } else {
            first(&request.space.schema)?
        };
        self.call(
            "validate",
            json!({"decision_id":request.id,"action":action}),
        )
        .await?;
        self.call("submit",json!({"decision_id":request.id,"revision":request.revision,"action":action,"public_explanation":"Inert fixture answer; no AI/provider call."})).await?;
        *self
            .trace
            .lock()
            .unwrap()
            .decisions
            .entry(self.seat)
            .or_default() += 1;
        *self
            .trace
            .lock()
            .unwrap()
            .phases
            .entry(self.seat)
            .or_default()
            .entry(phase)
            .or_default() += 1;
        self.sink.emit(
            self.seat,
            TranscriptEntry::AssistantText {
                text: "Offline fake turn complete.".into(),
            },
        );
        tokio::time::timeout(Duration::from_secs(30), self.sink.flush_confirmed())
            .await
            .map_err(|_| DriverError::Timeout)?
            .map_err(DriverError::Protocol)?;
        self.cost += 0.01;
        Ok(TurnOutcome {
            ok: true,
            error: None,
            text: None,
            quota: vec![],
            usage: Usage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                cached_input_tokens: Some(0),
                cache_creation_tokens: Some(0),
                cost_usd: Some(self.cost),
                cost_basis_known: Some(true),
                ..Usage::default()
            },
        })
    }
    fn session_id(&self) -> Option<String> {
        self.id.clone()
    }
    fn is_alive(&mut self) -> bool {
        self.alive
    }
    async fn stop(&mut self) {
        if self.alive {
            self.trace.lock().unwrap().active -= 1;
            self.alive = false;
        }
    }
}
fn first(schema: &ActionSchema) -> Result<Value, DriverError> {
    Ok(match schema {
        ActionSchema::Choice { options } => json!(
            options
                .iter()
                .find(|o| ["cancel", "done", "finish"].contains(&o.id.as_str()))
                .unwrap_or(&options[0])
                .id
        ),
        ActionSchema::Integer { min, max } => json!((*min).max(1).min(*max)),
        ActionSchema::Bool => json!(false),
        ActionSchema::Unit { among } => json!(
            among
                .first()
                .ok_or_else(|| DriverError::Protocol("empty mandatory unit domain".into()))?
        ),
        ActionSchema::Hex { among: Some(among) } => json!(
            among
                .first()
                .ok_or_else(|| DriverError::Protocol("empty mandatory hex domain".into()))?
        ),
        ActionSchema::Record { fields } => Value::Object(
            fields
                .iter()
                .filter(|f| !f.optional)
                .map(|f| Ok((f.name.clone(), first(&f.schema)?)))
                .collect::<Result<_, DriverError>>()?,
        ),
        ActionSchema::List { item, min, .. } => {
            Value::Array((0..*min).map(|_| first(item)).collect::<Result<_, _>>()?)
        }
        _ => {
            return Err(DriverError::Protocol(
                "inert fixture has no enumerated mandatory answer".into(),
            ));
        }
    })
}
fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn drivers(demo: &Demo, trace: Arc<Mutex<Trace>>) -> Vec<(SeatId, Box<dyn SeatDriver>)> {
    demo.config
        .claude_seats()
        .map(|(seat, model)| {
            (
                seat,
                Box::new(Fake {
                    seat,
                    url: demo.mcp.url(seat).unwrap(),
                    sink: demo.sink.clone(),
                    id: None,
                    alive: false,
                    cap: 0.,
                    cost: demo.journal.as_ref().unwrap().snapshot().unwrap().seats[&seat]
                        .last_session_cost,
                    model: model.into(),
                    trace: trace.clone(),
                }) as Box<dyn SeatDriver>,
            )
        })
        .collect()
}
fn preset() -> cna_play::config::LaunchConfig {
    let args = [
        "--preset",
        "graziani-haiku",
        "--stop-after-turn",
        "1",
        "--stop-after-opstage",
        "1",
        "--budget-usd",
        "10",
    ]
    .map(str::to_string);
    let Command::Play(config) = parse_args(&args).unwrap() else {
        panic!()
    };
    config
}
async fn healthy(demo: &Demo) {
    assert_eq!(demo.handle.status(), cna_server::CampaignStatus::Paused);
    for seat in SeatId::all() {
        let b = demo.handle.seat(seat).binding;
        assert!(!b.paused && b.failure.is_none());
    }
}
#[tokio::test]
async fn two_slots_park_and_resume_with_usage_and_healthy_boundary_stop() {
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let mut config = preset();
    config.kind = cna_play::config::GameKind::Sandbox;
    config.run.as_mut().unwrap().boundary.game_turn = 2;
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        config,
    )
    .await
    .unwrap();
    let trace = Arc::new(Mutex::new(Trace::default()));
    let mut seats = drivers(&demo, trace.clone());
    let (_stop, rx) = watch::channel(false);
    let result = tokio::time::timeout(Duration::from_secs(60), demo.play_durable(&mut seats, rx))
        .await
        .unwrap();
    if let Err(e) = &result {
        panic!("{e}; decisions {:?}", trace.lock().unwrap().decisions);
    }
    healthy(&demo).await;
    let saved = demo.journal.as_ref().unwrap().snapshot().unwrap();
    // Sandbox has no forced-pass decision domains; the real-CNA proof below covers them.
    for (seat, record) in &saved.seats {
        assert_eq!(
            record.turns,
            trace
                .lock()
                .unwrap()
                .decisions
                .get(seat)
                .copied()
                .unwrap_or(0)
        );
    }
    {
        let t = trace.lock().unwrap();
        assert_eq!(t.active, 0);
        assert_eq!(t.peak, 2);
        assert!(t.starts.values().any(|v| v.len() > 1));
        for starts in t.starts.values() {
            for (i, (id, resumed, _)) in starts.iter().enumerate() {
                assert_eq!(id, &starts[0].0);
                assert_eq!(*resumed, i > 0);
            }
        }
    }
    let entries = demo.transcript_for(demo.seat).unwrap();
    assert!(entries.iter().any(|e| matches!(
        e,
        ServerMessage::Transcript {
            entry: TranscriptEntry::UsageSnapshot(_),
            ..
        }
    )));
    let path = root.path().join(format!("{}.sqlite", demo.campaign_id()));
    demo.shutdown().await.unwrap();
    drop(seats);
    let mut demo = Demo::resume(&path, &repo.join("data"), &repo.join("web/dist"))
        .await
        .unwrap();
    let restored = demo.journal.as_ref().unwrap().snapshot().unwrap();
    for (seat, record) in &saved.seats {
        assert_eq!(
            restored.seats[seat].reported_cost_usd,
            record.reported_cost_usd
        );
        assert_eq!(restored.seats[seat].session, record.session);
    }
    let mut run = restored.config.run.unwrap();
    run.boundary.game_turn = 3;
    demo.reconfigure_run(run).await.unwrap();
    assert_eq!(demo.handle.status(), cna_server::CampaignStatus::Paused);
    let mut seats = drivers(&demo, trace.clone());
    let (_stop, rx) = watch::channel(false);
    demo.play_durable(&mut seats, rx).await.unwrap();
    healthy(&demo).await;
    let resumed = demo.journal.as_ref().unwrap().snapshot().unwrap();
    assert!(
        resumed
            .seats
            .iter()
            .any(|(seat, s)| s.turns > saved.seats[seat].turns)
    );
    for (seat, s) in &resumed.seats {
        assert!(s.reported_cost_usd >= saved.seats[seat].reported_cost_usd);
        if let Some(old) = &saved.seats[seat].session {
            assert_eq!(s.session.as_ref().unwrap().session_id, old.session_id);
        }
    }
    assert_eq!(trace.lock().unwrap().active, 0);
    demo.shutdown().await.unwrap();
}
#[tokio::test]
async fn real_graziani_ten_seats_stop_cleanly_after_bounded_fake_admissions() {
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let mut config = preset();
    config.max_turns = 2;
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        config,
    )
    .await
    .unwrap();
    let trace = Arc::new(Mutex::new(Trace::default()));
    let mut seats = drivers(&demo, trace.clone());
    let (_stop, rx) = watch::channel(false);
    let result = tokio::time::timeout(Duration::from_secs(60), demo.play_durable(&mut seats, rx))
        .await
        .unwrap();
    if let Err(e) = &result {
        let failures: Vec<_> = SeatId::all()
            .filter_map(|seat| demo.handle.seat(seat).binding.failure.map(|f| (seat, f)))
            .collect();
        eprintln!("preset result {e}; failures {failures:?}");
    }
    if result.is_ok() {
        healthy(&demo).await;
    }
    let accounting = demo.journal.as_ref().unwrap().snapshot().unwrap();
    let cleanup = demo.shutdown().await;
    cna_play::combine_results(result, cleanup).unwrap();
    let t = trace.lock().unwrap();
    let rows: Vec<_> = SeatId::all().map(|seat| {
        let record=&accounting.seats[&seat];
        json!({"seat":seat,"model_admissions":record.turns,"accepted_fake_decisions":t.decisions.get(&seat).copied().unwrap_or(0),"model_decisions_by_phase":t.phases.get(&seat),"automatic_answers":record.stages.values().map(|s|s.automatic_completed).sum::<u64>(),"user_prompt_bytes":t.prompt_bytes.get(&seat),"observe_bytes":t.observe_bytes.get(&seat)})
    }).collect();
    println!(
        "OFFLINE_PRESET_SAMPLE {}",
        json!({"scope":"bounded two-admissions-per-seat setup slice; not a complete OpStage/game-turn","paid_calls":0,"native_processes":0,"fake_peak":t.peak,"rows":rows,"usage":"synthetic fixture values; byte sizes are measured, token estimates are not provider measurements"})
    );
    assert_eq!(t.active, 0);
    assert!(t.peak <= 2);
    assert!(!t.decisions.is_empty());
}

#[tokio::test]
async fn real_driver_without_verified_bound_pauses_before_any_native_start() {
    use cna_seats::driver::claude::{ClaudeConfig, ClaudeDriver};
    use cna_seats::run::PromptBuilder;
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        preset(),
    )
    .await
    .unwrap();
    let mut seats: Vec<_> = demo
        .config
        .claude_seats()
        .map(|(seat, model)| {
            let driver = ClaudeDriver::new(
                ClaudeConfig {
                    seat,
                    exe: Some(root.path().join("must-never-be-started.exe")),
                    model: model.into(),
                    config_dir: None,
                    expected_email: None,
                    sandbox: root.path().join("sandbox").join(seat.to_string()),
                    run_dir: root.path().join("run").join(seat.to_string()),
                    mcp_url: demo.mcp.url(seat).unwrap(),
                    system_prompt: demo.prompts.system_prompt(seat),
                    effort: None,
                    context_window: None,
                },
                demo.sink.clone(),
            );
            (seat, Box::new(driver) as Box<dyn SeatDriver>)
        })
        .collect();
    let (_stop, rx) = watch::channel(false);
    demo.play_durable(&mut seats, rx).await.unwrap();
    healthy(&demo).await;
    assert!(
        demo.journal
            .as_ref()
            .unwrap()
            .snapshot()
            .unwrap()
            .seats
            .values()
            .all(|s| s.turns == 0 && s.session.is_none() && s.incomplete_turns == 0)
    );
    assert!(!root.path().join("sandbox").exists());
    demo.shutdown().await.unwrap();
}
