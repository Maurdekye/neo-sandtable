//! Server-backed seat launcher with opt-in durable campaign sessions.
pub mod budget;
mod campaign;
pub mod config;
pub mod journal;
use cna_core::{ids::SeatId, visibility::Perspective};
use cna_protocol::{ControllerInfo, ControllerKind, ServerMessage};
use cna_seats::{
    driver::SeatDriver,
    mcp::{McpServer, SeatEndpoint, ToolRouter},
    run::{DefaultPrompts, PromptBuilder},
    transcript::TranscriptSink,
};
use cna_server::{
    CampaignStatus,
    actor::CampaignHandle,
    campaigns,
    http::{App, CreateRequest},
};
use config::{Controller, GameKind, LaunchConfig};
use futures_util::{StreamExt, stream::FuturesUnordered};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::task::JoinHandle;

pub const CALL_CAP: u64 = 40;
pub const WALL_LIMIT: Duration = Duration::from_secs(150);
pub const SCRIPTED_WALL_LIMIT: Duration = Duration::from_secs(600);
pub const TURN_LIMIT: Duration = Duration::from_secs(60);

// md15: fixed seat-level reason, never raw/formatted panic payload.
const TRANSCRIPT_WORKER_PANIC: &str = "transcript delivery worker panicked; see server log";

/// A new pending revision gets its existing ceiling once; resume/reseed/nudge of the SAME
/// revision inherits that deadline, including notebook/storage/semaphore waits already spent.
fn decision_deadline(
    retained: &mut Option<(String, u32, tokio::time::Instant)>,
    id: &str,
    revision: u32,
    parent: tokio::time::Instant,
    ceiling: Duration,
) -> tokio::time::Instant {
    if let Some((old_id, old_revision, deadline)) = retained.as_mut()
        && old_id.as_str() == id
        && *old_revision == revision
    {
        *deadline = (*deadline).min(parent);
        return *deadline;
    }
    let deadline = parent.min(tokio::time::Instant::now() + ceiling);
    *retained = Some((id.into(), revision, deadline));
    deadline
}

pub struct Demo {
    pub handle: CampaignHandle,
    pub config: LaunchConfig,
    pub epochs: BTreeMap<SeatId, u64>,
    pub seat: SeatId,
    pub epoch: u64,
    pub router: Arc<ToolRouter>,
    pub sink: TranscriptSink,
    pub prompts: DefaultPrompts,
    pub mcp: McpServer,
    pub base_url: String,
    pub outbox: PathBuf,
    pub journal: Option<Arc<journal::SessionJournal>>,
    /// Foreground trusted owners, one per existing seat task. Uncertainty never drops a
    /// reservation just because its borrowed execution future or whole-run timer ended.
    forced_answers:
        BTreeMap<SeatId, tokio::sync::Mutex<Option<cna_seats::automatic::ForcedAnswerSupervisor>>>,
    /// Original operation is installed here before borrowing the driver turn future.
    /// A storage error or canceled borrowed caller does not discard its original evidence.
    journal_turns: BTreeMap<SeatId, tokio::sync::Mutex<Option<PendingJournalTurn>>>,
    /// Actual per-seat canonical startup slots; a borrowed owner restores here.
    /// Public startup dispatch remains held until full source assembly/fit.
    journal_starts: BTreeMap<SeatId, campaign::JournalStartRoot>,
    cleanup_uncertain: tokio::sync::Mutex<BTreeSet<SeatId>>,
    unsettled_permits: tokio::sync::Mutex<BTreeMap<SeatId, tokio::sync::OwnedSemaphorePermit>>,
    driver_allowances: std::sync::Mutex<BTreeMap<SeatId, cna_seats::operation::Allowance>>,
    /// Trusted campaign-derived paths only; never part of a seat's configuration or tools.
    driver_archive: cna_seats::failed_evidence::CampaignArchive,
    app: App,
    http: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

/// Actual external control types, ONE instance each, without constructing a
/// launcher or invoking a method/future. Nested frames overlap: this inventory
/// is never summed as independent allocations, a live refusal count, or F.
/// String outputs are inline headers only; their capacity cover is separate.
#[allow(dead_code)]
struct LauncherControlFrame {
    concrete: std::alloc::Layout,
    timeout: std::alloc::Layout,
    boxed_pointer: std::alloc::Layout,
    erased_pointer: std::alloc::Layout,
    output: std::alloc::Layout,
    timeout_output: std::alloc::Layout,
}

#[allow(dead_code)]
struct LauncherControlFrames {
    drain: LauncherControlFrame,
    stopped_worker: LauncherControlFrame,
    epoch_pause: LauncherControlFrame,
    finish: LauncherControlFrame,
    shutdown: LauncherControlFrame,
}

#[allow(dead_code)]
impl LauncherControlFrames {
    fn derive() -> Self {
        Self {
            drain: launcher_borrowed_frame(Demo::drain),
            stopped_worker: launcher_borrowed_frame(Demo::observe_stopped_worker),
            epoch_pause: launcher_borrowed_frame(Demo::pause_worker_panic_owners),
            finish: launcher_result_frame(Demo::finish_run),
            shutdown: launcher_borrowed_frame(Demo::shutdown),
        }
    }
}

fn launcher_borrowed_frame<'a, F>(_: impl Fn(&'a Demo) -> F) -> LauncherControlFrame
where
    F: std::future::Future + Send + 'a,
{
    launcher_future_frame::<'a, F>()
}

fn launcher_result_frame<'a, F>(
    _: impl Fn(&'a Demo, Result<(), String>) -> F,
) -> LauncherControlFrame
where
    F: std::future::Future + Send + 'a,
{
    launcher_future_frame::<'a, F>()
}

fn launcher_future_frame<'a, F>() -> LauncherControlFrame
where
    F: std::future::Future + Send + 'a,
{
    // No argument values, future/timeout construction, boxing, erasure or poll.
    LauncherControlFrame {
        concrete: std::alloc::Layout::new::<F>().pad_to_align(),
        timeout: std::alloc::Layout::new::<tokio::time::Timeout<F>>().pad_to_align(),
        boxed_pointer: std::alloc::Layout::new::<std::pin::Pin<Box<F>>>().pad_to_align(),
        erased_pointer: std::alloc::Layout::new::<
            std::pin::Pin<Box<dyn std::future::Future<Output = F::Output> + Send + 'a>>,
        >()
        .pad_to_align(),
        output: std::alloc::Layout::new::<F::Output>().pad_to_align(),
        timeout_output: std::alloc::Layout::new::<Result<F::Output, tokio::time::error::Elapsed>>()
            .pad_to_align(),
    }
}

struct PendingJournalTurn {
    operation: cna_seats::operation::Operation,
    /// Sticky before asking the driver for its erased future. An empty result after
    /// cancellation/unwind is still an owned attempt, never permission to dispatch again.
    /// This local marker does not prove native start, returned outcome or any join.
    supervisor_started: bool,
    /// None means the original borrowed supervisor has not returned or been canceled.
    outcome: Option<Result<cna_seats::driver::TurnOutcome, cna_seats::driver::DriverError>>,
    /// Set only after original measurements and telemetry return. A retained
    /// result alone cannot commit journal/usage after a callback unwind.
    supervisor_observed: bool,
    turn_started_at: Option<tokio::time::Instant>,
    /// Actual CLI evidence moved separately; original supervisor outcome stays authoritative.
    captured_outcome: Option<cna_seats::driver::TurnOutcome>,
    /// True while the capture callback has not returned. An unwind leaves it
    /// unresolved; do not call it again or infer absence/commit from this flag.
    capture_in_service: bool,
    /// First returned handoff refusal, retained BEFORE diagnostic formatting.
    /// It neither replaces the original supervisor error nor permits a retry.
    capture_refusal: Option<cna_seats::driver::DriverError>,
    journal_revision: u64,
    elapsed_ms: u64,
    supervisor_turn_elapsed_ms: u64,
    requested_turn_ms: u64,
    telemetry: cna_seats::driver::SessionTelemetry,
}

impl Demo {
    pub async fn new(directory: &Path, data: &Path, dist: &Path) -> Self {
        Self::with_config(directory, data, dist, LaunchConfig::default())
            .await
            .expect("create demo")
    }

    pub async fn with_config(
        directory: &Path,
        data: &Path,
        dist: &Path,
        config: LaunchConfig,
    ) -> Result<Self, String> {
        config.validate()?;
        let handle = campaigns::create(
            directory,
            data,
            CreateRequest {
                kind: match config.kind {
                    GameKind::Sandbox => cna_server::http::CampaignKind::Sandbox,
                    GameKind::Cna => cna_server::http::CampaignKind::Cna,
                },
                rules_profile: config.profile().into(),
                seed: [7; 32],
                title: format!("AI integration ({})", config.profile()),
                paused: true,
                controller: "human".into(),
            },
        )
        .map_err(|e| e.to_string())?;
        let mut epochs = BTreeMap::new();
        for (seat, controller) in &config.seats {
            let (info, settings) = match controller {
                Controller::Claude(model) => (
                    ControllerInfo {
                        kind: ControllerKind::LlmCli,
                        label: format!("Claude Code / {model}"),
                    },
                    json!({"model":model,"max_turns":config.max_turns,"max_tool_calls":config.tool_calls,
                        "max_wall_s":config.session.as_ref().map_or(WALL_LIMIT.as_secs(),|s|s.wall_seconds),"turn_timeout_s":config.session.as_ref().map_or(TURN_LIMIT.as_secs(),|s|s.turn_seconds),"session_limits":config.session}),
                ),
                Controller::Scripted(mode) => (
                    ControllerInfo {
                        kind: ControllerKind::Scripted,
                        label: format!("scripted:{mode}"),
                    },
                    json!({"mode":mode}),
                ),
                Controller::Human => (
                    ControllerInfo {
                        kind: ControllerKind::Human,
                        label: "Human".into(),
                    },
                    json!({"mode":"human"}),
                ),
            };
            let binding = handle
                .handover(*seat, Some(info), settings)
                .await
                .map_err(|e| e.to_string())?;
            if matches!(controller, Controller::Claude(_)) {
                epochs.insert(*seat, binding.controller_epoch);
            }
        }
        if let Some(run) = &config.run {
            handle
                .set_run_boundary(Some(run.boundary))
                .await
                .map_err(|e| e.to_string())?;
        }
        let id = handle.header(Perspective::Operator).meta.id;
        let journal = if config.session.is_some() {
            Some(Arc::new(journal::SessionJournal::create(
                directory,
                &id,
                config.clone(),
                &epochs,
            )?))
        } else {
            None
        };
        Self::serve(directory, data, dist, config, handle, epochs, journal).await
    }

    pub async fn resume(path: &Path, data: &Path, dist: &Path) -> Result<Self, String> {
        let directory = path.parent().ok_or("campaign path needs a directory")?;
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("invalid campaign database name")?;
        // Acquire lease before starting the recovered campaign writer.
        let journal = Arc::new(journal::SessionJournal::recover(directory, id)?);
        let saved = journal.snapshot()?;
        let handle = campaigns::recover(path, data).map_err(|e| e.to_string())?;
        let checked = async {
            if handle.header(Perspective::Operator).meta.id != id {return Err("campaign identity differs from journal".to_string());}
            // Recovery's internal activation barrier already blocks work. Preserve the
            // committed Running witness; only existing explicit play activation may open it.
            for (seat, record) in &saved.seats {
                let binding=handle.seat(*seat).binding;
                if binding.controller_epoch!=record.epoch || binding.paused || binding.failure.is_some() || binding.controller.as_ref().map(|c|c.kind)!=Some(ControllerKind::LlmCli) || binding.config.get("model").and_then(|s|s.as_str())!=Some(record.model.as_str()) {
                    return Err(format!("{seat}: journal binding was replaced or failed; explicit operator action required"));
                }
            }
            if let Some(run)=&saved.config.run {handle.set_run_boundary(Some(run.boundary)).await.map_err(|e|e.to_string())?;}
            Ok(())
        }.await;
        if let Err(error) = checked {
            let cleanup = handle.shutdown().await.map_err(|e| e.to_string());
            return combine_results(Err(error), cleanup).map(|_| unreachable!());
        }
        let epochs = saved.seats.iter().map(|(s, r)| (*s, r.epoch)).collect();
        Self::serve(
            directory,
            data,
            dist,
            saved.config,
            handle,
            epochs,
            Some(journal),
        )
        .await
    }

    async fn serve(
        directory: &Path,
        data: &Path,
        dist: &Path,
        config: LaunchConfig,
        handle: CampaignHandle,
        epochs: BTreeMap<SeatId, u64>,
        journal: Option<Arc<journal::SessionJournal>>,
    ) -> Result<Self, String> {
        let campaign_id = handle.header(Perspective::Operator).meta.id;
        let archive_configuration = (|| {
            let root = directory
                .parent()
                .ok_or("campaign storage needs a launcher root")?;
            let isolation = if cfg!(windows) {
                PathBuf::from(r"C:\CNA-Seat-Isolation")
            } else {
                root.join("isolation")
            };
            cna_seats::failed_evidence::CampaignArchive::new(
                &directory.join(format!("{campaign_id}.sqlite")),
                &root.join("sandbox"),
                &root.join("run"),
                &[isolation],
            )
            .map_err(|e| e.to_string())
        })();
        let driver_archive = match archive_configuration {
            Ok(archive) => archive,
            Err(error) => {
                // This construction happens before MCP/HTTP tasks. The already-owned
                // campaign writer still requires its actual shutdown result on refusal.
                let cleanup = handle.shutdown().await.map_err(|e| e.to_string());
                return combine_results(Err(error), cleanup).map(|_| unreachable!());
            }
        };
        let seat = epochs
            .keys()
            .next()
            .copied()
            .unwrap_or_else(|| "axis.commander".parse().unwrap());
        let epoch = handle.seat(seat).binding.controller_epoch;
        let shared = Arc::new(handle.clone());
        // Empty trusted roots precede MCP/HTTP task creation and any future
        // startup preparation. No Operation/body is installed or dispatched here.
        let journal_starts = epochs
            .iter()
            .map(|(seat, epoch)| (*seat, campaign::JournalStartRoot::empty(*seat, *epoch)))
            .collect();
        let seats: Vec<_> = epochs.keys().copied().collect();
        let mut router = ToolRouter::new(shared.clone(), shared.clone(), &seats);
        if let Some(journal) = &journal {
            router = router.with_budget(journal.clone());
        } else {
            router = router.with_call_cap(Some(config.tool_calls));
        }
        let router = Arc::new(router);
        let prompts = DefaultPrompts { game_description: match config.kind {
            GameKind::Sandbox => "This is sandbox-v1, a synthetic integration game. Call observe for its complete rules summary.",
            GameKind::Cna => "This is The Campaign for North Africa, Graziani's Offensive, under the development rules profile. Only implemented procedures are offered. Read the current observation, decision context and legal action schema through your tools. Future windows may ask for different orders; never assume initiative is the only kind. Unimplemented procedures are skipped by this profile; this is not a complete rules simulation. observe.enemy_counters groups enemy map counters by printed type and size per hex; inspect a disclosed counter or hex for its printed face. These are counters, not strength or a complete enemy inventory: attached units, contents and off-map units are not disclosed. logistics.rations gives problem unit lists to logistics and counts to other seats; counts are not unit IDs. Inspect an own unit for its current record.",
        }.into() };
        let endpoints = epochs
            .iter()
            .map(|(seat, epoch)| SeatEndpoint {
                seat: *seat,
                epoch: *epoch,
                instructions: prompts.system_prompt(*seat),
            })
            .collect();
        let mcp = McpServer::start(router.clone(), endpoints)
            .await
            .map_err(|e| e.to_string())?;
        let sink = TranscriptSink::new(shared);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let data: PathBuf = data.into();
        let app = App::new(
            directory.into(),
            port,
            Arc::new(move |r, dir| campaigns::create(dir, &data, r)),
        );
        app.register(handle.clone());
        let routes = app.router(dist);
        let http = tokio::spawn(async move {
            axum::serve(listener, routes).await.unwrap();
        });
        let forced_answers = epochs
            .keys()
            .map(|seat| (*seat, tokio::sync::Mutex::new(None)))
            .collect();
        let journal_turns = epochs
            .keys()
            .map(|seat| (*seat, tokio::sync::Mutex::new(None)))
            .collect();
        Ok(Self {
            handle,
            config,
            epochs,
            seat,
            epoch,
            router,
            sink,
            prompts,
            mcp,
            base_url: format!("http://127.0.0.1:{port}"),
            outbox: directory.join(format!("{campaign_id}.unconfirmed.jsonl")),
            journal,
            forced_answers,
            journal_turns,
            journal_starts,
            cleanup_uncertain: tokio::sync::Mutex::new(BTreeSet::new()),
            unsettled_permits: tokio::sync::Mutex::new(BTreeMap::new()),
            driver_allowances: std::sync::Mutex::new(BTreeMap::new()),
            driver_archive,
            app,
            http: tokio::sync::Mutex::new(Some(http)),
        })
    }

    /// Required SAME-handle host adapters. Parent stamps the original allowance before this
    /// call; neither preparation nor a retry creates a new whole-answer timer.
    pub(crate) async fn answer_hosted(
        &self,
        seat: SeatId,
        epoch: u64,
        request: &cna_core::decision::DecisionRequest,
        allowance: cna_seats::operation::Allowance,
    ) -> Result<bool, String> {
        if request.seat != seat {
            return Err("forced answer seat mismatch".into());
        }
        if !cna_seats::automatic::forced_pass(&request.space) {
            return Ok(false);
        }
        let retained = self
            .forced_answers
            .get(&seat)
            .ok_or("missing required forced owner")?;
        let mut foreground = retained.lock().await;
        if foreground.is_some() {
            return Err("previous forced operation remains Unsettled".into());
        }
        let operation_id = uuid::Uuid::new_v4().to_string();
        let operation = self
            .driver_archive
            .operation(seat, epoch, operation_id.clone(), allowance)
            .map_err(|e| e.to_string())?;
        let identity = cna_seats::host::HostIdentity {
            seat,
            epoch,
            decision_id: request.id.to_string(),
            revision: request.revision,
            operation_id,
        };
        let original = cna_seats::game::SubmitRequest {
            decision_id: request.id.to_string(),
            epoch,
            revision: Some(request.revision),
            idempotency_key: format!(
                "automatic|{seat}|{}|e{epoch}|r{}|null",
                request.id, request.revision
            ),
            action: serde_json::Value::Null,
            public_explanation: None,
        };
        // The synchronous reserve happens BEFORE any worker route and BEFORE the first await.
        *foreground = Some(
            cna_seats::automatic::ForcedAnswerSupervisor::new(
                self.handle.host_forced_submission(),
                identity,
                original,
                operation,
            )
            .map_err(|e| e.to_string())?,
        );
        let owner = foreground.as_mut().expect("retained forced owner");
        let result = owner.execute(&self.sink).await.map_err(|e| e.to_string());
        if result.is_ok() {
            // Both prefix markers and scoped final pending are confirmed; this local forced
            // operation owns no native child and no in-flight append beyond marker TWO.
            owner.request_settlement_after_supervision();
            let ack = owner.settlement_ack().await.map_err(|e| e.to_string())?;
            if ack.uncertain() {
                return Err("forced actor settlement remains uncertain".into());
            }
            *foreground = None;
        }
        // Error/cancellation retains the owner, actual accepted receipt and actor association.
        result
    }

    fn driver_context(
        &self,
        seat: SeatId,
        epoch: u64,
        request: &cna_core::decision::DecisionRequest,
        allowance: cna_seats::operation::Allowance,
    ) -> Result<cna_seats::host::HostDriverContext, String> {
        self.driver_context_with_id(
            seat,
            epoch,
            request,
            allowance,
            uuid::Uuid::new_v4().to_string(),
        )
    }

    fn driver_context_with_id(
        &self,
        seat: SeatId,
        epoch: u64,
        request: &cna_core::decision::DecisionRequest,
        allowance: cna_seats::operation::Allowance,
        operation_id: String,
    ) -> Result<cna_seats::host::HostDriverContext, String> {
        uuid::Uuid::parse_str(&operation_id).map_err(|_| "invalid driver operation identity")?;
        let operation = self
            .driver_archive
            .operation(seat, epoch, operation_id.clone(), allowance)
            .map_err(|e| e.to_string())?;
        self.driver_allowances
            .lock()
            .expect("driver supervision allowances")
            .insert(seat, allowance);
        Ok(cna_seats::host::HostDriverContext {
            admission: self.handle.host_admission(),
            identity: cna_seats::host::HostIdentity {
                seat,
                epoch,
                decision_id: request.id.to_string(),
                revision: request.revision,
                operation_id,
            },
            operation,
        })
    }

    /// Private startup SOURCE bridge: construct metadata for the ALREADY-owned
    /// original. Never mint a second Operation or renew/register an allowance.
    /// The caller retains this whole result before inspection; full creation-time
    /// growing-string/host/library fit and actual driver binding are still held.
    #[allow(dead_code)]
    fn driver_context_from_original(
        &self,
        seat: SeatId,
        epoch: u64,
        request: &cna_core::decision::DecisionRequest,
        original: &cna_seats::operation::Operation,
    ) -> Result<cna_seats::host::HostDriverContext, String> {
        if self.epochs.get(&seat) != Some(&epoch) || uuid::Uuid::parse_str(original.id()).is_err() {
            return Err("original startup context identity refused".into());
        }
        original
            .check()
            .map_err(|_| "original startup context allowance refused")?;
        Ok(cna_seats::host::HostDriverContext {
            admission: self.handle.host_admission(),
            identity: cna_seats::host::HostIdentity {
                seat,
                epoch,
                decision_id: request.id.to_string(),
                revision: request.revision,
                operation_id: original.id().into(),
            },
            operation: original.clone(),
        })
    }

    /// Call only after producers and SAME native children/readers have actually settled.
    /// Joining the transcript worker does not claim cancellation of accepted storage work.
    async fn settle_forced_after_supervision(&self) -> Result<(), String> {
        let mut result = Ok(());
        for retained in self.forced_answers.values() {
            let mut foreground = retained.lock().await;
            if let Some(owner) = foreground.as_mut() {
                owner.request_settlement_after_supervision();
                match owner.settlement_ack().await {
                    Ok(ack) if !ack.uncertain() => {
                        *foreground = None;
                    }
                    Ok(_) => {
                        result = combine_results(
                            result,
                            Err("forced actor settlement uncertain; owner retained".into()),
                        )
                    }
                    Err(error) => {
                        result = combine_results(
                            result,
                            Err(format!(
                                "forced actor ACK unavailable; owner retained: {error}"
                            )),
                        )
                    }
                }
            }
        }
        result
    }

    async fn forced_cleanup_deadline(&self) -> Option<tokio::time::Instant> {
        let mut earliest = None;
        for retained in self.forced_answers.values() {
            if let Some(owner) = retained.lock().await.as_ref() {
                let deadline = owner.allowance().deadline();
                earliest =
                    Some(earliest.map_or(deadline, |old: tokio::time::Instant| old.min(deadline)));
            }
        }
        earliest
    }

    async fn forced_owned(&self, seat: SeatId) -> bool {
        match self.forced_answers.get(&seat) {
            Some(retained) => retained.lock().await.is_some(),
            None => false,
        }
    }

    fn journal_owned(&self, seat: SeatId) -> bool {
        self.journal_starts
            .get(&seat)
            .is_some_and(|root| root.position().is_owned())
            || self
                .journal_turns
                .get(&seat)
                .is_some_and(|retained| match retained.try_lock() {
                    Ok(owner) => owner.is_some(),
                    // An in-service borrowed turn is still owned, never an empty/default permission.
                    Err(_) => true,
                })
    }

    fn any_journal_owned(&self) -> bool {
        self.journal_turns
            .keys()
            .chain(self.journal_starts.keys())
            .any(|seat| self.journal_owned(*seat))
    }
    /// Persist an explicit operator rebalance/boundary change while paused, without resuming.
    pub async fn reconfigure_run(&mut self, run: budget::RunControl) -> Result<(), String> {
        self.handle.pause(true).await.map_err(|e| e.to_string())?;
        self.journal
            .as_ref()
            .ok_or("missing journal")?
            .reconfigure_run(run.clone())?;
        self.handle
            .set_run_boundary(Some(run.boundary))
            .await
            .map_err(|e| e.to_string())?;
        self.config.run = Some(run);
        Ok(())
    }
    pub fn campaign_id(&self) -> String {
        self.handle.header(Perspective::Operator).meta.id
    }

    /// Trusted spectator URL. Never pass it to a seat driver or its environment.
    pub fn board_url(&self) -> String {
        format!(
            "{}/?campaign={}#cap={}",
            self.base_url,
            self.campaign_id(),
            self.app.operator_token()
        )
    }

    /// Own-seat console links for current human bindings. Keep these in trusted output;
    /// each page receives only its seat capability, held in its URL fragment and memory.
    pub fn human_console_urls(&self) -> Vec<(SeatId, String)> {
        let campaign_id = self.campaign_id();
        self.config
            .seats
            .keys()
            .filter_map(|seat| {
                if self
                    .handle
                    .seat(*seat)
                    .binding
                    .controller
                    .as_ref()
                    .is_none_or(|controller| controller.kind != ControllerKind::Human)
                {
                    return None;
                }
                self.app.seat_token(&campaign_id, *seat).map(|token| {
                    (
                        *seat,
                        format!(
                            "{}/console.html?campaign={campaign_id}&seat={seat}#cap={token}",
                            self.base_url
                        ),
                    )
                })
            })
            .collect()
    }

    /// Compatibility helper for the original single-seat probe.
    pub async fn play(&self, driver: &mut dyn SeatDriver, turns: usize) -> Result<(), String> {
        if self.epochs.len() != 1 {
            return Err("use play_sessions for zero or multiple Claude bindings".into());
        }
        let start = self.handle.pause(false).await.map_err(|e| e.to_string());
        let result = if start.is_ok() {
            let (_cancel, stop) = tokio::sync::watch::channel(false);
            self.run_session(self.seat, self.epoch, driver, turns, stop)
                .await
        } else {
            combine_results(start, driver.stop().await.map_err(|e| e.to_string()))
        };
        self.finish_run(result).await
    }

    /// All configured CLI seats run concurrently (at most two); a completed or failed bounded session stops siblings.
    /// Human/scripted-only runs launch no CLI and need no paid opt-in.
    pub async fn play_sessions(
        &self,
        drivers: &mut [(SeatId, Box<dyn SeatDriver>)],
        turns: usize,
    ) -> Result<(), String> {
        // The bounded launcher is not a bypass around a retained durable root.
        // Refuse before any driver callback, activation or cleanup attempt.
        if self.any_journal_owned() {
            return Err("original startup/turn owner remains Unsettled; dispatch refused".into());
        }
        let provided: BTreeSet<_> = drivers.iter().map(|(seat, _)| *seat).collect();
        let expected: BTreeSet<_> = self.epochs.keys().copied().collect();
        if provided != expected
            || provided.len() != drivers.len()
            || drivers
                .iter()
                .any(|(_, driver)| driver.kind() != cna_seats::driver::CliKind::Claude)
        {
            return Err(
                "exactly one confined Claude driver is required for each Claude binding".into(),
            );
        }
        let mut start = self.handle.pause(false).await.map_err(|e| e.to_string());
        if start.is_err() {
            for (_, driver) in drivers {
                start = combine_results(start, driver.stop().await.map_err(|e| e.to_string()));
            }
            return self.finish_run(start).await;
        }
        let result = if drivers.is_empty() {
            let mut state = self.handle.watch_status();
            tokio::time::timeout(SCRIPTED_WALL_LIMIT, async {
                loop {
                    match state.borrow_and_update().clone() {
                        CampaignStatus::Running => {}
                        CampaignStatus::Finished { .. } => return Ok(()),
                        other => {
                            return Err(format!("campaign stopped before completion: {other:?}"));
                        }
                    }
                    state.changed().await.map_err(|e| e.to_string())?;
                }
            })
            .await
            .unwrap_or_else(|_| Err("scripted campaign wall-clock budget exhausted".into()))
        } else {
            let (cancel, stop) = tokio::sync::watch::channel(false);
            let mut tasks = FuturesUnordered::new();
            for (seat, driver) in drivers {
                let seat = *seat;
                let epoch = self.epochs[&seat];
                let stop = stop.clone();
                tasks.push(async move {
                    (
                        seat,
                        self.run_session(seat, epoch, driver.as_mut(), turns, stop)
                            .await,
                    )
                });
            }
            let mut result = Ok(());
            while let Some((seat, outcome)) = tasks.next().await {
                let _ = cancel.send(true);
                result = combine_results(result, outcome.map_err(|e| format!("{seat}: {e}")));
            }
            result
        };
        self.finish_run(result).await
    }

    async fn run_session(
        &self,
        seat: SeatId,
        epoch: u64,
        driver: &mut dyn SeatDriver,
        turns: usize,
        mut stop: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), String> {
        // Stamp the existing whole-run allowance once, before pending/prepare/native work.
        let run_deadline = tokio::time::Instant::now() + WALL_LIMIT;
        let mut state = self.handle.watch_seat(seat);
        let invalidated = async {
            loop {
                {
                    let current = state.borrow_and_update();
                    if current.binding.controller_epoch != epoch || current.binding.paused {
                        break;
                    }
                }
                if state.changed().await.is_err() {
                    break;
                }
            }
        };
        let cancelled = async {
            while !*stop.borrow_and_update() {
                if stop.changed().await.is_err() {
                    break;
                }
            }
        };
        let work = async {
            let mut turn = 0;
            let mut started = false;
            let mut decision_allowance = None;
            loop {
                let mut windows = self.handle.watch_seat(seat);
                let pending = loop {
                    let p = windows.borrow_and_update().pending.clone();
                    if !p.is_empty() {
                        break p;
                    }
                    match self.handle.status() {
                        CampaignStatus::Finished { .. } => return Ok(()),
                        CampaignStatus::Stopped { .. } | CampaignStatus::Paused => {
                            return Err("campaign is no longer running".into());
                        }
                        CampaignStatus::Running => {}
                    }
                    windows.changed().await.map_err(|e| e.to_string())?;
                };
                let decision_deadline = decision_deadline(
                    &mut decision_allowance,
                    &pending[0].id.to_string(),
                    pending[0].revision,
                    run_deadline,
                    TURN_LIMIT,
                );
                let mut automatic = false;
                for request in &pending {
                    let allowance =
                        cna_seats::operation::Allowance::required(Some(decision_deadline))
                            .map_err(|e| e.to_string())?
                            .capped(Duration::from_secs(5));
                    if self
                        .answer_hosted(seat, epoch, request, allowance)
                        .await
                        .map_err(|e| e.to_string())?
                    {
                        automatic = true;
                    }
                }
                if automatic {
                    continue;
                }
                if turn >= turns.min(self.config.max_turns) {
                    return Ok(());
                }
                if !started {
                    let allowance =
                        cna_seats::operation::Allowance::required(Some(decision_deadline))
                            .map_err(|e| e.to_string())?
                            .capped(TURN_LIMIT);
                    driver
                        .set_host_context(self.driver_context(
                            seat,
                            epoch,
                            &pending[0],
                            allowance,
                        )?)
                        .map_err(|e| e.to_string())?;
                    driver.start(None).await.map_err(|e| e.to_string())?;
                    started = true;
                }
                let prompt = if turn == 0 {
                    let notebook = self
                        .handle
                        .notebook(seat)
                        .await
                        .map_err(|e| e.to_string())?;
                    self.prompts.first_turn(seat, &notebook, &pending)
                } else {
                    self.prompts.window_turn(seat, &pending)
                };
                let prompt = format!(
                    "{prompt}\n\nThis is a bounded probe. Answer only the decision IDs listed in this request, then end your turn even if a later observation reveals new windows. Do not chase later windows in this turn."
                );
                let allowance = cna_seats::operation::Allowance::required(Some(decision_deadline))
                    .map_err(|e| e.to_string())?
                    .capped(TURN_LIMIT);
                driver
                    .set_host_context(self.driver_context(seat, epoch, &pending[0], allowance)?)
                    .map_err(|e| e.to_string())?;
                let outcome = driver
                    .run_turn(&prompt, TURN_LIMIT)
                    .await
                    .map_err(|e| e.to_string())?;
                if !outcome.ok {
                    return Err(outcome.error.unwrap_or_else(|| "CLI refused turn".into()));
                }
                let now = self.handle.seat(seat);
                if pending.iter().any(|old| {
                    now.pending
                        .iter()
                        .any(|new| new.id == old.id && new.revision == old.revision)
                }) {
                    return Err("CLI ended without answering its pending decisions".into());
                }
                self.sink.system(
                    seat,
                    format!(
                        "bounded demo turn {} complete; usage {:?}",
                        turn + 1,
                        outcome.usage
                    ),
                );
                turn += 1;
                if turn >= turns.min(self.config.max_turns) {
                    return Ok(());
                }
            }
        };
        let mut result = tokio::select! {
            _ = invalidated => Err("controller binding changed; old session stopped".into()),
            _ = cancelled => { self.sink.system(seat, "bounded peer session ended; this session stopped"); Ok(()) },
            result = tokio::time::timeout_at(run_deadline, work) => result.unwrap_or_else(|_| Err("demo wall-clock budget exhausted".into())),
        };
        let stopped = self.stop_child(seat, driver).await;
        let cleanup_confirmed = stopped.is_ok();
        if !cleanup_confirmed {
            self.cleanup_uncertain.lock().await.insert(seat);
        }
        result = combine_results(result, stopped);
        if let Err(reason) = &result
            && cleanup_confirmed
            && !self.journal_owned(seat)
            && !self.forced_owned(seat).await
            && !matches!(
                self.handle.status(),
                CampaignStatus::Finished { .. } | CampaignStatus::Stopped { .. }
            )
        {
            match self.handle.mark_failure_if_epoch(seat, epoch, reason).await {
                Ok(()) | Err(cna_server::Error::StaleEpoch) => {}
                Err(error) => {
                    result = combine_results(
                        result,
                        Err(format!("recording seat failure failed: {error}")),
                    )
                }
            }
        }
        result
    }

    async fn pause_worker_panic_owners(&self) -> Result<(), String> {
        if matches!(
            self.handle.status(),
            CampaignStatus::Finished { .. } | CampaignStatus::Stopped { .. }
        ) {
            return Ok(());
        }
        let mut result = Ok(());
        // EXISTING fixed seat set and original epochs, not current/replacement
        // epochs. Existing context registrations and retained forced owners
        // identify users of this shared sink, rather than every configured seat.
        // Preserve each owner/receipt; this pause is NOT settlement or cleanup.
        for (&seat, &epoch) in &self.epochs {
            let driver_used = self
                .driver_allowances
                .lock()
                .map_err(|_| "transcript worker user registry unavailable; Unsettled".to_string())?
                .contains_key(&seat);
            // The synchronous registry guard ends before either awaited call.
            if !driver_used && !self.forced_owned(seat).await {
                continue;
            }
            match self
                .handle
                .mark_failure_if_epoch(seat, epoch, TRANSCRIPT_WORKER_PANIC)
                .await
            {
                Ok(()) | Err(cna_server::Error::StaleEpoch) => {}
                Err(error) => {
                    result = combine_results(
                        result,
                        Err(format!(
                            "recording transcript worker seat pause failed; Unsettled: {error}"
                        )),
                    );
                }
            }
        }
        result
    }

    /// Observe the class saved by stop_delivery without renewing a wait budget.
    /// Owned missing/busy/unjoined state remains Unsettled; legacy no-task
    /// semantics remain unchanged. This observation never claims actor custody.
    async fn observe_stopped_worker(&self) -> Result<(), String> {
        match self
            .sink
            .join_delivery_until(tokio::time::Instant::now())
            .await
        {
            Err(error) if error == TRANSCRIPT_WORKER_PANIC => combine_results(
                Err(
                    "transcript delivery worker panicked; seat cleanup Unsettled; see server log"
                        .into(),
                ),
                self.pause_worker_panic_owners().await,
            ),
            result => result,
        }
    }

    async fn finish_run(&self, mut result: Result<(), String>) -> Result<(), String> {
        if self.any_journal_owned() {
            return combine_results(result, Err("original journal outcome/evidence remains foreground-owned; no healthy finish or automatic pause".into()));
        }
        if !self.cleanup_uncertain.lock().await.is_empty() {
            return combine_results(result, Err("native cleanup remains foreground-owned Unsettled; no actor settlement or automatic pause".into()));
        }
        if let Some(deadline) = self.forced_cleanup_deadline().await {
            // Native producers have actually joined; now observe the worker's actual join.
            // The actor still owns any accepted storage receipt until its truthful ACK.
            if let Err(error) = self.sink.join_delivery_until(deadline).await {
                if error == TRANSCRIPT_WORKER_PANIC {
                    let paused = self.pause_worker_panic_owners().await;
                    // Do not clear foreground owners or claim an actor ACK.
                    return combine_results(combine_results(result,
                        Err("transcript delivery worker panicked; seat cleanup Unsettled; see server log".into())), paused);
                }
                return combine_results(result, Err(error));
            }
            result = combine_results(result, self.settle_forced_after_supervision().await);
            if self.forced_cleanup_deadline().await.is_some() {
                return result;
            }
        }
        let admission = self.handle.host_admission();
        let _changes = admission.subscribe();
        let snapshot = match admission.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return combine_results(
                    result,
                    Err(format!("host disposition unavailable: {error}")),
                );
            }
        };
        if snapshot.activated
            && matches!(self.handle.status(), CampaignStatus::Running)
            && let Err(error) = self.handle.pause(true).await
            && !matches!(
                self.handle.status(),
                CampaignStatus::Finished { .. } | CampaignStatus::Stopped { .. }
            )
        {
            result = combine_results(result, Err(format!("campaign pause failed: {error}")));
        }
        combine_results(result, self.drain().await)
    }
    pub fn transcript(&self) -> Vec<ServerMessage> {
        self.transcript_for(self.seat)
            .expect("persisted transcript")
    }

    pub fn transcript_for(&self, seat: SeatId) -> Result<Vec<ServerMessage>, String> {
        self.handle
            .replay()
            .transcripts(Perspective::Operator, seat, 0)
            .map_err(|e| e.to_string())
    }
    /// Bound a probe's persistence drain. A dead writer cannot hold HTTP/MCP open forever.
    async fn drain(&self) -> Result<(), String> {
        // flush() deliberately discards its marker error. Require the existing
        // confirmed API's actual success, including when no rows remain.
        let confirmed = matches!(
            tokio::time::timeout(Duration::from_secs(5), self.sink.flush_confirmed()).await,
            Ok(Ok(()))
        );
        if confirmed && self.sink.pending_count() == 0 {
            return Ok(());
        }
        let pending = self.sink.stop_delivery().await;
        // Rows remain in the sink; an outbox is not a worker/actor custody ACK.
        self.observe_stopped_worker().await?;
        let mut text = String::new();
        for entry in &pending {
            text.push_str(&serde_json::to_string(entry).map_err(|e| e.to_string())?);
            text.push('\n');
        }
        tokio::fs::write(&self.outbox, text).await.map_err(|e| {
            format!(
                "transcript persistence failed; recovery outbox {} could not be written: {e}",
                self.outbox.display()
            )
        })?;
        Err(format!(
            "transcript persistence failed; {} unconfirmed captures saved to {}. Inspect before replaying.",
            pending.len(),
            self.outbox.display()
        ))
    }

    /// Borrow the launcher so a failed/canceled shutdown cannot consume its uncertain owners.
    /// The caller must keep this launcher and its original driver boxes while Unsettled.
    pub async fn shutdown(&self) -> Result<(), String> {
        if self
            .journal_starts
            .values()
            .any(|root| root.position().is_owned())
        {
            return Err("shutdown refused: original startup owner remains Unsettled".into());
        }
        if self.any_journal_owned()
            || !self.cleanup_uncertain.lock().await.is_empty()
            || !self.unsettled_permits.lock().await.is_empty()
            || self.forced_cleanup_deadline().await.is_some()
        {
            return Err("shutdown refused: foreground supervision/actor ACK remains Unsettled; launcher and drivers must be retained".into());
        }
        let result = self.drain().await;
        self.sink.stop_delivery().await;
        // Preserve launcher services on panic or an unknown actual join.
        // Repeated shutdown reads the first fixed class, never re-polls it.
        if let Err(error) = self.observe_stopped_worker().await {
            return combine_results(result, Err(error));
        }
        self.mcp.shutdown();
        let mut http = self.http.lock().await;
        if let Some(task) = http.as_mut() {
            task.abort();
            let _ = task.await;
            http.take();
        }
        drop(http);
        self.app.shutdown().await;
        result
    }
}

/// Serve a saved campaign for thirty seconds without starting any model session.
pub async fn replay_board(path: &Path, data: &Path, dist: &Path) {
    let handle = campaigns::recover(path, data).expect("recover saved campaign");
    // Recovery's internal activation barrier blocks controllers without a committed Game write.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let data: PathBuf = data.into();
    let app = App::new(
        path.parent().unwrap().into(),
        port,
        Arc::new(move |r, dir| campaigns::create(dir, &data, r)),
    );
    let id = handle.header(Perspective::Operator).meta.id;
    app.register(handle);
    let routes = app.router(dist);
    let task = tokio::spawn(async move {
        axum::serve(listener, routes).await.unwrap();
    });
    println!(
        "Board: http://127.0.0.1:{port}/?campaign={id}#cap={}",
        app.operator_token()
    );
    println!("Saved campaign viewer; no CLI starts. Closing in 30 seconds.");
    tokio::time::sleep(Duration::from_secs(30)).await;
    task.abort();
    let _ = task.await;
    app.shutdown().await;
}

/// Preserve a primary failure alongside a cleanup/recovery failure.
pub fn combine_results(
    primary: Result<(), String>,
    cleanup: Result<(), String>,
) -> Result<(), String> {
    match (primary, cleanup) {
        (Err(primary), Err(cleanup)) => Err(format!("{primary}; additionally: {cleanup}")),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

#[cfg(test)]
mod admission_deadline_tests {
    use super::*;

    #[test]
    fn same_revision_recovery_cannot_renew_or_borrow_allowance() {
        let now = tokio::time::Instant::now();
        let mut retained = None;
        let first = decision_deadline(
            &mut retained,
            "d",
            7,
            now + Duration::from_secs(100),
            Duration::from_secs(20),
        );
        assert_eq!(
            decision_deadline(
                &mut retained,
                "d",
                7,
                now + Duration::from_secs(1000),
                Duration::from_secs(500)
            ),
            first
        );
        let exhausted_parent = now - Duration::from_millis(1);
        assert_eq!(
            decision_deadline(
                &mut retained,
                "d",
                7,
                exhausted_parent,
                Duration::from_secs(500)
            ),
            exhausted_parent
        );
        // A shorter parent sticks to this revision even when a later caller offers more time.
        assert_eq!(
            decision_deadline(
                &mut retained,
                "d",
                7,
                now + Duration::from_secs(1000),
                Duration::from_secs(500)
            ),
            exhausted_parent
        );
    }
}
