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
use cna_protocol::{GameEvent, ServerMessage, TranscriptEntry, UsageSnapshot};
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
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;

// The producer owns these fresh, private handoff paths even if no browser starts.
struct PrivateHandoff {
    ready: Option<PathBuf>,
    release: Option<PathBuf>,
}
impl PrivateHandoff {
    fn new(ready: Option<PathBuf>, release: Option<PathBuf>) -> Result<Self, String> {
        if ready.is_some() != release.is_some() || (ready.is_some() && ready == release) {
            return Err("private handoff needs two distinct fresh paths".into());
        }
        for path in ready.iter().chain(release.iter()) {
            if !path.is_absolute() {
                return Err("private handoff paths must be absolute".into());
            }
            match std::fs::symlink_metadata(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err("private handoff paths must be fresh".into()),
            }
        }
        Ok(Self { ready, release })
    }
    fn write(&self, value: &Value) -> Result<(), String> {
        if let Some(path) = &self.ready {
            std::fs::write(path, serde_json::to_vec(value).map_err(|e| e.to_string())?)
                .map_err(|e| format!("private ready write failed: {e}"))?;
        }
        Ok(())
    }
    fn cleanup(&self) -> Result<(), String> {
        let mut errors = Vec::new();
        for (name, path) in [("ready", &self.ready), ("release", &self.release)] {
            if let Some(path) = path {
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => errors.push(format!("private {name} cleanup failed: {e}")),
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}
impl Drop for PrivateHandoff {
    fn drop(&mut self) {
        // Normal completion checks cleanup errors after server shutdown. Also
        // remove artifacts during unwinding, without logging their contents.
        let _ = self.cleanup();
    }
}

#[test]
fn private_handoff_removes_success_no_ack_and_unwind_artifacts() {
    for outcome in ["success", "no_ack", "unwind"] {
        let root = tempfile::tempdir().unwrap();
        let ready = root.path().join("ready.json");
        let release = root.path().join("release");
        let handoff = PrivateHandoff::new(Some(ready.clone()), Some(release.clone())).unwrap();
        handoff
            .write(&json!({"board_url":"private synthetic capability"}))
            .unwrap();
        if outcome == "success" {
            std::fs::write(&release, []).unwrap();
        }
        if outcome == "unwind" {
            assert!(
                std::panic::catch_unwind(move || {
                    let _handoff = handoff;
                    panic!("synthetic helper failure");
                })
                .is_err()
            );
        } else {
            handoff.cleanup().unwrap();
            drop(handoff);
        }
        assert!(!ready.exists() && !release.exists(), "{outcome}");
    }
}

#[test]
fn private_handoff_reports_failures_and_never_owns_existing_files() {
    let root = tempfile::tempdir().unwrap();
    let ready = root.path().join("ready.json");
    let release = root.path().join("release");
    std::fs::write(&ready, b"existing").unwrap();
    assert!(PrivateHandoff::new(Some(ready.clone()), Some(release.clone())).is_err());
    assert_eq!(std::fs::read(&ready).unwrap(), b"existing");
    std::fs::remove_file(&ready).unwrap();
    let handoff = PrivateHandoff::new(Some(ready.clone()), Some(release.clone())).unwrap();
    handoff.write(&json!({"state":"incomplete"})).unwrap();
    // A filesystem error must remain a failure while cleanup still removes READY.
    std::fs::create_dir(&release).unwrap();
    assert!(
        handoff
            .cleanup()
            .unwrap_err()
            .contains("release cleanup failed")
    );
    assert!(!ready.exists());
    std::fs::remove_dir(&release).unwrap();
    handoff.cleanup().unwrap();
    let failed =
        PrivateHandoff::new(Some(root.path().join("absent/ready")), Some(release)).unwrap();
    assert!(failed.write(&json!({"state":"ready"})).is_err());
    failed.cleanup().unwrap();
}

#[derive(Default)]
struct Trace {
    active: usize,
    peak: usize,
    starts: BTreeMap<SeatId, Vec<(String, bool, f64)>>,
    decisions: BTreeMap<SeatId, u64>,
    phases: BTreeMap<SeatId, BTreeMap<String, u64>>,
    prompt_bytes: BTreeMap<SeatId, Vec<usize>>,
    observe_bytes: BTreeMap<SeatId, Vec<usize>>,
    windows: BTreeMap<SeatId, Vec<Value>>,
    tool_text_bytes: BTreeMap<SeatId, usize>,
}
struct Fake {
    client: reqwest::Client,
    seat: SeatId,
    url: String,
    sink: TranscriptSink,
    id: Option<String>,
    alive: bool,
    cap: f64,
    cost: f64,
    model: String,
    trace: Arc<Mutex<Trace>>,
    refuse_cleanup: Arc<AtomicBool>,
    batch_setup: bool,
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
        let response:Value=self.client.post(&self.url).json(&json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":args}})).send().await.map_err(|e|DriverError::Protocol(format!("{:#?}", e.without_url())))?.json().await.map_err(|e|DriverError::Protocol(format!("{:#?}", e.without_url())))?;
        let result = &response["result"];
        let text = result["content"][0]["text"]
            .as_str()
            .ok_or_else(|| DriverError::Protocol(response.to_string()))?;
        *self
            .trace
            .lock()
            .unwrap()
            .tool_text_bytes
            .entry(self.seat)
            .or_default() += text.len();
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
        let before_bytes = self
            .trace
            .lock()
            .unwrap()
            .tool_text_bytes
            .get(&self.seat)
            .copied()
            .unwrap_or(0);
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
        self.trace.lock().unwrap().windows.entry(self.seat).or_default().push(json!({
            "phase":phase,"anchor":request.clock.anchor,"kind":request.kind,"user_prompt_bytes":prompt.len(),
            "observation_bytes":observed.to_string().len(),"action_description_bytes":described.to_string().len(),
            "provider_tokens":null
        }));
        let action = if request.space.pass.is_some() {
            Value::Null
        } else {
            if self.batch_setup && request.kind == "cna.setup.first_line_trucks" {
                largest_setup_allocation(&request.space.schema)?
            } else {
                first(&request.space.schema)?
            }
        };
        self.call(
            "validate",
            json!({"decision_id":request.id,"action":action}),
        )
        .await?;
        self.call("submit",json!({"decision_id":request.id,"revision":request.revision,"action":action,"public_explanation":"Inert fixture answer; no AI/provider call."})).await?;
        {
            let mut t = self.trace.lock().unwrap();
            let bytes = t.tool_text_bytes[&self.seat] - before_bytes;
            t.windows.get_mut(&self.seat).unwrap().last_mut().unwrap()["tool_result_text_bytes"] =
                json!(bytes);
        }
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
    async fn stop(&mut self) -> Result<(), cna_seats::driver::DriverError> {
        if self.alive && self.refuse_cleanup.load(Ordering::SeqCst) {
            return Err(DriverError::Died("inert parking termination denied".into()));
        }
        if self.alive {
            self.trace.lock().unwrap().active -= 1;
            self.alive = false;
        }

        Ok(())
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
// Choose quantities only from this own-seat advertised allocation record. This
// batches a finite setup pool, without reading engine state or another seat.
fn largest_setup_allocation(schema: &ActionSchema) -> Result<Value, DriverError> {
    match schema {
        ActionSchema::Record { fields } => Ok(Value::Object(
            fields
                .iter()
                .filter(|f| !f.optional)
                .map(|f| {
                    Ok((
                        f.name.clone(),
                        match &f.schema {
                            ActionSchema::Integer { max, .. } => json!(max),
                            other => first(other)?,
                        },
                    ))
                })
                .collect::<Result<_, DriverError>>()?,
        )),
        _ => Err(DriverError::Protocol(
            "setup allocation must advertise a record".into(),
        )),
    }
}
fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn drivers(demo: &Demo, trace: Arc<Mutex<Trace>>) -> Vec<(SeatId, Box<dyn SeatDriver>)> {
    drivers_with_policy(demo, trace, false)
}
fn drivers_with_policy(
    demo: &Demo,
    trace: Arc<Mutex<Trace>>,
    batch_setup: bool,
) -> Vec<(SeatId, Box<dyn SeatDriver>)> {
    demo.config
        .claude_seats()
        .map(|(seat, model)| {
            (
                seat,
                Box::new(Fake {
                    client: reqwest::Client::new(),
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
                    refuse_cleanup: Arc::new(AtomicBool::new(false)),
                    batch_setup,
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

fn expected_usage(record: &cna_play::journal::SeatJournal) -> Option<UsageSnapshot> {
    let usage = record.usage.as_ref().filter(|u| u.revision > 0)?;
    Some(UsageSnapshot {
        controller_epoch: record.epoch,
        revision: usage.revision,
        provider: Some("claude-code".into()),
        model: Some(
            record
                .session
                .as_ref()
                .and_then(|s| s.model.clone())
                .unwrap_or_else(|| record.model.clone()),
        ),
        attempts: record.turns,
        completed: record.stages.values().map(|s| s.completed).sum(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        reported_cost_usd: usage.reported_cost_usd,
        incomplete_turns: record.incomplete_turns,
    })
}
fn retain_latest_usage(
    latest: &mut Option<(u64, UsageSnapshot)>,
    tseq: u64,
    snapshot: UsageSnapshot,
) {
    if latest.as_ref().is_none_or(|(_, old)| {
        (snapshot.controller_epoch, snapshot.revision) > (old.controller_epoch, old.revision)
    }) {
        *latest = Some((tseq, snapshot));
    }
}

#[test]
fn usage_replay_selects_epoch_revision_without_summing_repeated_frames() {
    let base = UsageSnapshot {
        controller_epoch: 2,
        revision: 3,
        provider: None,
        model: None,
        attempts: 5,
        completed: 4,
        input_tokens: Some(12),
        output_tokens: None,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        reasoning_tokens: None,
        reported_cost_usd: Some(0.1),
        incomplete_turns: 1,
    };
    let mut latest = None;
    retain_latest_usage(&mut latest, 10, base.clone());
    retain_latest_usage(&mut latest, 11, base.clone());
    assert_eq!(latest, Some((10, base.clone())));
    let mut old = base.clone();
    old.controller_epoch = 1;
    old.revision = 100;
    retain_latest_usage(&mut latest, 12, old);
    assert_eq!(latest, Some((10, base.clone())));
    let mut next = base;
    next.revision += 1;
    next.input_tokens = Some(13);
    retain_latest_usage(&mut latest, 13, next.clone());
    assert_eq!(latest, Some((13, next)));
}

#[tokio::test]
#[ignore = "slow: complete ten-seat inert preset game-turn; no provider calls"]
async fn full_game_turn_inert_preset_reports_each_seat_and_phase() {
    use cna_core::visibility::Perspective;
    let handoff = PrivateHandoff::new(
        std::env::var_os("CNA_PRESET_BOARD_READY").map(PathBuf::from),
        std::env::var_os("CNA_PRESET_BOARD_RELEASE").map(PathBuf::from),
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let Command::Play(config) = parse_args(
        &[
            "--preset",
            "graziani-haiku",
            "--stop-after-turn",
            "1",
            "--budget-usd",
            "30",
            "--seat-budget-usd",
            "axis.commander=2",
            "--seat-budget-usd",
            "axis.front_line=1",
            "--seat-budget-usd",
            "axis.rear_area=1",
            "--seat-budget-usd",
            "axis.logistics=9",
            "--seat-budget-usd",
            "axis.air=2",
            "--seat-budget-usd",
            "commonwealth.commander=2",
            "--seat-budget-usd",
            "commonwealth.front_line=1",
            "--seat-budget-usd",
            "commonwealth.rear_area=1",
            "--seat-budget-usd",
            "commonwealth.logistics=9",
            "--seat-budget-usd",
            "commonwealth.air=2",
        ]
        .map(str::to_string),
    )
    .unwrap() else {
        panic!()
    };
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        config,
    )
    .await
    .unwrap();
    let trace = Arc::new(Mutex::new(Trace::default()));
    let mut seats = drivers_with_policy(&demo, trace.clone(), true);
    let (stop, rx) = watch::channel(false);
    // Optional trusted browser handoff. The operator URL never enters a driver,
    // transcript, public report or stdout; the helper consumes this private file in memory.
    let initial_handoff = handoff.write(&json!({
            "state":"ready","board_url":demo.board_url(),"campaign_id":demo.campaign_id(),
            "usage_basis":"synthetic inert usage: 10 input/5 output/0.01 USD per completed fake window; provider/native calls zero"
        }));
    for seat in SeatId::all() {
        demo.sink.system(seat, "OFFLINE PRESET: all driver usage is synthetic; no provider or native model process is running.");
    }
    let started = std::time::Instant::now();
    // Persisted per-decision/lifetime bounds remain in force. Full-turn completion
    // additionally requires the actual fence, never merely an Ok supervisor result.
    // CI 37668460452 (db029cfa, 2026-10-07): this sole ignored preset test
    // completed in 242.46 s. Guard 485 s is about 2x that measured duration;
    // this calibration does not diagnose the separate browser setup stall.
    // Send graceful stop and join supervisors, rather than canceling cleanup.
    let result = if let Err(error) = initial_handoff {
        Err(error)
    } else {
        let play = demo.play_durable(&mut seats, rx);
        tokio::pin!(play);
        tokio::select! {
            result = &mut play => result,
            _ = tokio::time::sleep(Duration::from_secs(485)) => {
                let _ = stop.send(true);
                cna_play::combine_results(Err("offline full-turn measurement hang guard reached".into()), play.await)
            }
        }
    };
    let gameplay_seconds = started.elapsed().as_secs_f64();
    let accounting = demo.journal.as_ref().unwrap().snapshot().unwrap();
    let projection = demo.handle.projection(Perspective::Operator);
    let clock = projection.view.clock.clone();
    let replay = demo.handle.replay();
    let mut canonical: BTreeMap<SeatId, BTreeMap<String, u64>> = BTreeMap::new();
    let mut cursor = 0;
    loop {
        let page = replay
            .events(Perspective::Operator, cursor, projection.seq)
            .unwrap();
        if page.is_empty() {
            break;
        }
        for message in page {
            if let ServerMessage::Event {
                seq, clock, event, ..
            } = message
            {
                cursor = seq;
                if let GameEvent::DecisionResolved { seat, .. } = event {
                    let phase = format!(
                        "GT{}:{}",
                        clock.game_turn,
                        clock
                            .op_stage
                            .map(|n| format!("OpStage{n}"))
                            .unwrap_or(clock.stage)
                    );
                    *canonical
                        .entry(seat.parse().unwrap())
                        .or_default()
                        .entry(phase)
                        .or_default() += 1;
                }
            }
        }
    }
    let mut budget_stops = vec![];
    let mut usage_diagnostics = BTreeMap::new();
    for seat in SeatId::all() {
        let mut cursor = 0;
        let mut pages = 0;
        let mut rows = 0;
        let mut latest = None;
        loop {
            let page = replay
                .transcripts(Perspective::Operator, seat, cursor)
                .unwrap();
            if page.is_empty() {
                break;
            }
            pages += 1;
            rows += page.len();
            for message in page {
                if let ServerMessage::Transcript { tseq, entry, .. } = message {
                    cursor = tseq;
                    match entry {
                        TranscriptEntry::System { text }
                            if text.starts_with("automatic budget stop:") =>
                        {
                            budget_stops.push(json!({"seat":seat,"reason":text}));
                        }
                        TranscriptEntry::UsageSnapshot(snapshot) => {
                            retain_latest_usage(&mut latest, tseq, *snapshot)
                        }
                        _ => {}
                    }
                }
            }
        }
        let record = &accounting.seats[&seat];
        let expected = expected_usage(record);
        let matches_journal = latest.as_ref().map(|(_, u)| u) == expected.as_ref();
        let pending = record.usage.as_ref().map_or(0, |u| u.outbox.len());
        let acknowledged = record.usage.as_ref().map_or(0, |u| u.acknowledged_revision);
        usage_diagnostics.insert(seat, json!({
            "visited":record.turns>0,"pages":pages,"rows":rows,"last_tseq":cursor,
            "latest":latest.as_ref().map(|(tseq, snapshot)|json!({"tseq":tseq,"snapshot":snapshot})),
            "expected":expected,"matches_journal":matches_journal,
            "acknowledged_revision":acknowledged,"pending_outbox":pending,
            "unfinished_turn":record.inflight.is_some(),"incomplete_turns":record.incomplete_turns,
        }));
    }
    let healthy = demo.handle.status() == cna_server::CampaignStatus::Paused
        && SeatId::all().all(|seat| {
            let b = demo.handle.seat(seat).binding;
            !b.paused && b.failure.is_none()
        });
    let stages_seen = (1..=3).all(|n| {
        canonical
            .values()
            .any(|p| p.contains_key(&format!("GT1:OpStage{n}")))
    });
    let candidate = result.is_ok()
        && healthy
        && budget_stops.is_empty()
        && projection.view.pending.is_empty()
        && clock.game_turn == 1
        && stages_seen;
    // An Advance can cross automatic Post anchors and reach GT2 in one preview.
    // The fence discards that whole preview, so the saved clock can remain in OP3.
    // With no pending decisions or running fake drivers, explicitly resume ONLY
    // the existing actor. The same fence must park it again without any event/view change.
    let fence_repreview = if candidate {
        let mut status = demo.handle.watch_status();
        let resumed = demo.handle.pause(false).await;
        let parked = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if *status.borrow_and_update() == cna_server::CampaignStatus::Paused {
                    break;
                }
                status.changed().await.map_err(|e| e.to_string())?;
            }
            Ok::<_, String>(())
        })
        .await;
        resumed.is_ok()
            && matches!(parked, Ok(Ok(())))
            && demo.handle.projection(Perspective::Operator) == projection
            && demo.handle.run_boundary().await.unwrap()
                == Some(cna_server::RunBoundary {
                    game_turn: 1,
                    op_stage: None,
                })
    } else {
        false
    };
    let complete = candidate && fence_repreview;
    let mut browser_acknowledged = None;
    let final_handoff = handoff.write(&json!({
            "state":if complete { "OFFLINE_PRESET_GT1_BOUNDARY" } else { "incomplete" },
            "board_url":demo.board_url(),"campaign_id":demo.campaign_id(),"complete":complete,
            "final_clock":clock,"fence_repreview":fence_repreview,
            "usage_diagnostics":usage_diagnostics,
            "usage_basis":"synthetic inert usage: 10 input/5 output/0.01 USD per completed fake window; provider/native calls zero"
        }));
    if handoff.ready.is_some() && final_handoff.is_ok() {
        // Browser acknowledgement is independent of game/model deadlines. This
        // optional 120 s cleanup guard applies only to the local evidence helper.
        let release = handoff.release.as_ref().unwrap();
        let acknowledged = tokio::time::timeout(Duration::from_secs(120), async {
            while !release.exists() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        browser_acknowledged = Some(acknowledged.is_ok());
        if acknowledged.is_err() {
            eprintln!("offline browser acknowledgement absent; shutting down helper");
        }
    }
    let system_bytes: BTreeMap<_, _> = SeatId::all()
        .map(|seat| {
            (
                seat,
                cna_seats::run::PromptBuilder::system_prompt(&demo.prompts, seat).len(),
            )
        })
        .collect();
    let cleanup = demo.shutdown().await;
    let combined = cna_play::combine_results(
        cna_play::combine_results(result, cleanup),
        cna_play::combine_results(final_handoff, handoff.cleanup()),
    );
    let t = trace.lock().unwrap();
    let rows: Vec<_> = SeatId::all().map(|seat| {
        let record = &accounting.seats[&seat];
        let accepted = t.decisions.get(&seat).copied().unwrap_or(0);
        let automatic = record.stages.values().map(|s|s.automatic_completed).sum::<u64>();
        let resolved = canonical.get(&seat).map(|p|p.values().sum::<u64>()).unwrap_or(0);
        json!({"seat":seat,"model_admissions":record.turns,
            "system_prompt_bytes":system_bytes[&seat],
            "successful_fake_submit_receipts":accepted,"canonical_answers":resolved,
            "canonical_answers_by_phase":canonical.get(&seat),"model_decisions_by_phase":t.phases.get(&seat),
            "automatic_answers":automatic,"counts_reconcile":resolved==accepted+automatic,
            "windows":t.windows.get(&seat),"accounting":"synthetic fake usage, not measured provider tokens or USD"})
    }).collect();
    let report = json!({"scope":"game-turn one including setup; pass whenever declared; maximum advertised first-line truck allocation; otherwise minimal enumerated mandatory choice",
        "complete":complete,"fence_repreview":fence_repreview,"final_clock":clock,"healthy":healthy,"budget_stops":budget_stops,"error":combined.as_ref().err(),
        "gameplay_seconds":gameplay_seconds,"total_helper_seconds":started.elapsed().as_secs_f64(),"browser_acknowledged":browser_acknowledged,"paid_calls":0,"native_processes":0,"fake_peak":t.peak,"rows":rows,
        "usage_diagnostics":usage_diagnostics,
        "measurement":"UTF-8 bytes only; provider tokens, real USD and an active-movement game-turn cost are unmeasured"});
    if let Ok(path) = std::env::var("CNA_PRESET_FULL_OUTPUT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    println!("OFFLINE_PRESET_GAME_TURN {report}");
    assert_eq!(t.active, 0);
    assert!(t.peak <= 2);
    combined.unwrap();
    assert!(
        complete,
        "supervisor stopped before the full game-turn boundary; inspect report"
    );
    assert!(rows.iter().all(|r| r["counts_reconcile"] == true));
    assert_eq!(usage_diagnostics.len(), 10);
    for (seat, usage) in &usage_diagnostics {
        assert!(
            usage["visited"] == true && usage["latest"].is_object(),
            "missing persisted usage for {seat}"
        );
        assert_eq!(
            usage["matches_journal"], true,
            "persisted usage differs from committed journal for {seat}"
        );
        assert_eq!(usage["pending_outbox"], 0, "unconfirmed usage for {seat}");
        assert_eq!(
            usage["acknowledged_revision"], usage["expected"]["revision"],
            "unacknowledged usage revision for {seat}"
        );
    }
    assert_ne!(
        browser_acknowledged,
        Some(false),
        "browser helper did not acknowledge evidence capture"
    );
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

#[tokio::test]
async fn accounted_turn_with_failed_parking_retains_full_envelope() {
    use cna_play::{
        budget::{Admission, RunControl, SpendBudget},
        config::{GameKind, LaunchConfig, SessionLimits},
    };
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let seat: SeatId = "axis.commander".parse().unwrap();
    let mut config = LaunchConfig::resolve(
        GameKind::Sandbox,
        &[
            "axis.commander=claude:haiku".into(),
            "*=scripted:aggressive".into(),
        ],
    )
    .unwrap();
    config.session = Some(SessionLimits::default());
    config.max_turns = 20;
    config.run = Some(RunControl {
        boundary: cna_server::RunBoundary {
            game_turn: 2,
            op_stage: None,
        },
        budget: SpendBudget::usd(4., &[seat], BTreeMap::from([(seat, 1.)])).unwrap(),
    });
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        config,
    )
    .await
    .unwrap();
    let trace = Arc::new(Mutex::new(Trace::default()));
    let refuse = Arc::new(AtomicBool::new(true));
    let driver = Fake {
        client: reqwest::Client::new(),
        seat,
        url: demo.mcp.url(seat).unwrap(),
        sink: demo.sink.clone(),
        id: None,
        alive: false,
        cap: 0.,
        cost: 0.,
        model: "haiku".into(),
        trace: trace.clone(),
        refuse_cleanup: refuse.clone(),
        batch_setup: false,
    };
    let mut drivers = vec![(seat, Box::new(driver) as Box<dyn SeatDriver>)];
    let (_tx, rx) = watch::channel(false);
    let result = demo.play_durable(&mut drivers, rx).await;
    let record = demo.journal.as_ref().unwrap().snapshot().unwrap().seats[&seat].clone();
    let paused = matches!(demo.handle.status(), cna_server::CampaignStatus::Paused);
    let admission = demo
        .journal
        .as_ref()
        .unwrap()
        .admission(seat, drivers[0].1.estimate_bound().as_ref())
        .unwrap();
    let active_before_cleanup = trace.lock().unwrap().active;
    refuse.store(false, Ordering::SeqCst);
    drivers[0].1.stop().await.unwrap();
    demo.shutdown().await.unwrap();
    assert!(
        result
            .unwrap_err()
            .contains("inert parking termination denied")
    );
    assert_eq!(active_before_cleanup, 1);
    assert_eq!(trace.lock().unwrap().active, 0);
    assert_eq!(record.turns, 1);
    assert_eq!(record.reported_cost_usd, 0.01);
    assert_eq!(record.admitted_estimate_usd, Some(1.02));
    assert!(record.uncertain_budget_spend && paused);
    assert!(matches!(admission, Admission::Stop(_)));
}
