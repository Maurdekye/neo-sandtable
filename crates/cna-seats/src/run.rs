//! The seat runner: wakes a seat's CLI session when it has pending decisions, keeps it within its
//! budgets, recovers it after a crash, and pauses (never guesses) when it cannot continue.
//!
//! Design rules (`docs/design-v0.1.md` §4.8, `architecture.md` §5):
//! * A timeout, an invalid or missing answer, a CLI outage or an exhausted cap **pauses** the seat.
//!   The runner never submits anything itself, so a failure can never become a pass, a retreat or
//!   a surrender.
//! * One CLI session per seat for the whole campaign. A dead process is resumed by session id; if
//!   the session cannot be resumed a new one is started and seeded from the seat's notebook
//!   (durable notes live in the game, not in the session).
//! * Duplicate submissions are harmless (the tools are idempotent) and a lost one simply leaves
//!   the decision pending, so the next window asks again.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cna_core::decision::DecisionRequest;
use cna_core::ids::SeatId;
use cna_protocol::SeatStatus;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::time::Instant;

use crate::driver::{DriverError, SeatDriver, TurnOutcome};
use crate::game::GameBackend;
use crate::mcp::ToolRouter;
use crate::memory::SeatMemory;
use crate::transcript::TranscriptSink;

/// Per-run limits (`decisions.md` D7/D9: at most 2 concurrent CLI sessions by default, plus caps
/// on calls and elapsed time).
#[derive(Clone, Debug)]
pub struct RunLimits {
    /// Live CLI sessions at once, across all seats of the run.
    pub max_concurrent_sessions: usize,
    /// Tool calls one seat may make over the run.
    pub max_tool_calls_per_seat: u64,
    /// Wall time for the whole run.
    pub max_wall: Duration,
    /// Wall time for one CLI turn (one request to the model, tools included).
    pub turn_timeout: Duration,
    /// Extra "you still have pending decisions" turns before the seat is paused.
    pub max_nudges: u32,
    /// Crash recoveries (resume or reseed) per seat over the run.
    pub max_recoveries: u32,
    /// Stop the CLI process (keeping the session resumable) and give back the session slot after
    /// every decision window. Needed when there are more seats than `max_concurrent_sessions`.
    pub park_between_windows: bool,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_concurrent_sessions: 2,
            max_tool_calls_per_seat: 2_000,
            max_wall: Duration::from_secs(6 * 60 * 60),
            turn_timeout: Duration::from_secs(20 * 60),
            max_nudges: 2,
            max_recoveries: 3,
            park_between_windows: false,
        }
    }
}

/// Why a seat is paused or failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PauseReason {
    /// The seat did not answer its decisions after the allowed nudges.
    Unanswered,
    TurnTimeout,
    ToolBudgetExhausted,
    WallClockExhausted,
    RecoveriesExhausted,
    /// The CLI refused work (quota, login, outage): the operator can retry or take over.
    CliError(String),
    /// The session broke the isolation policy; the seat is failed, not retried.
    IsolationFailure(String),
}

impl std::fmt::Display for PauseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PauseReason::Unanswered => write!(f, "the seat did not answer its pending decisions"),
            PauseReason::TurnTimeout => write!(f, "a turn exceeded its time limit"),
            PauseReason::ToolBudgetExhausted => {
                write!(f, "the seat's tool-call budget is exhausted")
            }
            PauseReason::WallClockExhausted => write!(f, "the run's wall-clock limit is reached"),
            PauseReason::RecoveriesExhausted => write!(f, "the session crashed too many times"),
            PauseReason::CliError(e) => write!(f, "the CLI reported: {e}"),
            PauseReason::IsolationFailure(e) => write!(f, "isolation check failed: {e}"),
        }
    }
}

/// A seat's published state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeatState {
    pub status: SeatStatus,
    pub reason: Option<PauseReason>,
}

/// Where session ids are kept so a restarted run can resume them. The server persists these.
pub trait SessionStore: Send + Sync + 'static {
    fn load(&self, seat: SeatId) -> Option<String>;
    fn save(&self, seat: SeatId, session_id: &str);
    fn clear(&self, seat: SeatId);
}

#[derive(Default)]
pub struct InMemorySessions(Mutex<BTreeMap<SeatId, String>>);

impl SessionStore for InMemorySessions {
    fn load(&self, seat: SeatId) -> Option<String> {
        self.0.lock().expect("sessions").get(&seat).cloned()
    }
    fn save(&self, seat: SeatId, session_id: &str) {
        self.0
            .lock()
            .expect("sessions")
            .insert(seat, session_id.to_string());
    }
    fn clear(&self, seat: SeatId) {
        self.0.lock().expect("sessions").remove(&seat);
    }
}

/// The words a seat is told. Games provide their own; [`DefaultPrompts`] is generic.
#[async_trait]
pub trait PromptBuilder: Send + Sync + 'static {
    /// The CLI's system prompt for the seat (its role and how to use the tools).
    fn system_prompt(&self, seat: SeatId) -> String;
    /// First turn of a fresh (or reseeded) session. `notebook` is the seat's durable notes.
    fn first_turn(&self, seat: SeatId, notebook: &str, pending: &[DecisionRequest]) -> String;
    /// A later turn when new decisions are pending.
    fn window_turn(&self, seat: SeatId, pending: &[DecisionRequest]) -> String;
    /// The seat ended its turn without answering everything.
    fn nudge(&self, seat: SeatId, pending: &[DecisionRequest]) -> String;
}

pub struct DefaultPrompts {
    pub game_description: String,
}

fn batch_hint(pending: &[DecisionRequest]) -> &'static str {
    if pending.iter().any(|p| {
        matches!(
            p.space.schema,
            cna_core::decision::ActionSchema::List { .. }
        )
    }) {
        "\nOne answer can carry a list. Plan this segment/window first, inspect key disclosed units or targets, and submit several compatible items together where legal. Respect list bounds and order; validate the complete list. Use an offered pass only when its described meaning matches your intended plan. Do not replace a rejected draft with a pass."
    } else {
        ""
    }
}
fn list(pending: &[DecisionRequest]) -> String {
    pending
        .iter()
        .map(|d| format!("- {} ({}): {}", d.id, d.kind, d.summary))
        .collect::<Vec<_>>()
        .join("\n")
}

#[async_trait]
impl PromptBuilder for DefaultPrompts {
    fn system_prompt(&self, seat: SeatId) -> String {
        format!(
            "You are the {seat} seat in a turn-based game played entirely through tools. {}\n\n\
             Your ONLY way to learn about or act in the game is the tools provided: observe (your \
             situation and pending decisions), inspect, describe_actions (the legal answers for a \
             decision), validate (check a draft answer), submit (commit an answer; final), \
             message_team / read_messages (your own side only), and notebook_read / notebook_write \
             (durable notes that survive restarts; keep standing plans and lessons there). You have \
             no other tools: no files, no shell, no web. Never invent decision ids or answers: take \
             them from the tools. When you have answered the decision IDs and revisions requested for this model turn, stop calling tools \
             and reply with a one-line summary. Leave newer revisions and later windows for the next turn. \
             One answer can carry a list: plan the current segment/window, inspect key disclosed targets, \
             and group compatible actions in one ordered list when the schema allows it. Respect bounds \
             and validate the complete list. Follow the described pass meaning; never use pass as fallback \
             after a rejected draft. The submit explanation is optional: brief commentary for your own \
             side and the operator only; the enemy cannot read it. It is never executable or an order. \
             Keep the rationale short to save output tokens.",
            self.game_description
        )
    }

    fn first_turn(&self, seat: SeatId, notebook: &str, pending: &[DecisionRequest]) -> String {
        let notes = if notebook.trim().is_empty() {
            "Your notebook is empty.".to_string()
        } else {
            format!(
                "Your notebook (written by you earlier):\n{notebook}\nThese notes are historical. Separate tool-disclosed facts from hypotheses; recheck the current tools after a restart instead of treating old notes as a current observation."
            )
        };
        format!(
            "The game begins for you as {seat}. {notes}\n\nPending decisions:\n{}\n\nCall observe, then \
             answer each pending decision with submit.{}",
            list(pending),
            batch_hint(pending)
        )
    }

    fn window_turn(&self, _seat: SeatId, pending: &[DecisionRequest]) -> String {
        format!(
            "New pending decisions:\n{}\n\nCall observe, then answer each with submit.{}",
            list(pending),
            batch_hint(pending)
        )
    }

    fn nudge(&self, _seat: SeatId, pending: &[DecisionRequest]) -> String {
        format!(
            "You still have unanswered decisions:\n{}\n\nAnswer each with submit (use describe_actions \
             for the legal answers). If a call failed, read the error and try again.",
            list(pending)
        )
    }
}

/// How a seat's run loop ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeatEnd {
    /// The game is over.
    Finished,
    /// Stopped on request.
    Stopped,
    /// A trusted host closed admission before new work. This is not a seat failure/pause.
    Closed(crate::host::HostRefusal),
    Paused(PauseReason),
    Failed(PauseReason),
}

/// Required in-process host adapters. A game/tool backend cannot supply permission implicitly.
#[derive(Clone)]
pub struct RunnerHost {
    pub admission: Arc<dyn crate::host::HostAdmission>,
    pub forced: Arc<dyn crate::host::HostForcedSubmission>,
}

/// Foreground ownership survives a borrowed run timeout or error. This has no permissions and
/// no destructor release: the actor/native owners need actual supervision and settlement ACK.
pub struct RunnerOwnership {
    /// Trusted launcher configuration stays with the ORIGINAL foreground owner.
    /// Its absence never supplies a native archive plan or substitutes a destination.
    archive: Option<crate::failed_evidence::CampaignArchive>,
    forced: Option<crate::automatic::ForcedAnswerSupervisor>,
    permit: Option<OwnedSemaphorePermit>,
    context: Option<crate::host::HostDriverContext>,
    native_entered: bool,
    uncertain: bool,
    decision: Option<(String, u32, Instant)>,
}
impl RunnerOwnership {
    pub fn empty() -> Self {
        Self {
            archive: None,
            forced: None,
            permit: None,
            context: None,
            native_entered: false,
            uncertain: false,
            decision: None,
        }
    }
    /// Install trusted campaign-derived archive inputs BEFORE borrowing the run.
    /// This moves the configuration once; it does not grant host admission, inspect
    /// paths, create files, settle prior work or introduce a second allowance.
    pub fn for_campaign(archive: crate::failed_evidence::CampaignArchive) -> Self {
        Self {
            archive: Some(archive),
            ..Self::empty()
        }
    }
    pub fn is_unsettled(&self) -> bool {
        self.uncertain || self.forced.is_some() || self.native_entered || self.permit.is_some()
    }
}

/// Everything one seat runner needs.
pub struct SeatRunner {
    pub seat: SeatId,
    pub controller_epoch: u64,
    pub driver: Box<dyn SeatDriver>,
    pub game: Arc<dyn GameBackend>,
    pub host: RunnerHost,
    pub ownership: RunnerOwnership,
    pub memory: Arc<dyn SeatMemory>,
    pub router: Arc<ToolRouter>,
    pub sink: TranscriptSink,
    pub sessions: Arc<dyn SessionStore>,
    pub prompts: Arc<dyn PromptBuilder>,
    pub limits: RunLimits,
    pub permits: Arc<Semaphore>,
    pub started: Instant,
    pub state: watch::Sender<SeatState>,
    /// Stop requests (`true` = stop).
    pub stop: watch::Receiver<bool>,
}

enum TurnKind {
    First,
    Window,
    Nudge,
}

impl SeatRunner {
    fn identity(
        &self,
        request: &DecisionRequest,
        operation_id: String,
    ) -> crate::host::HostIdentity {
        crate::host::HostIdentity {
            seat: self.seat,
            epoch: self.controller_epoch,
            decision_id: request.id.to_string(),
            revision: request.revision,
            operation_id,
        }
    }

    fn decision_deadline(&mut self, request: &DecisionRequest) -> Instant {
        let parent = self.started + self.limits.max_wall;
        if let Some((id, revision, deadline)) = self.ownership.decision.as_mut()
            && id.as_str() == request.id.as_str()
            && *revision == request.revision
        {
            *deadline = (*deadline).min(parent);
            return *deadline;
        }
        let deadline = parent.min(Instant::now() + self.limits.turn_timeout);
        self.ownership.decision = Some((request.id.to_string(), request.revision, deadline));
        deadline
    }

    fn closed(error: crate::host::HostRefusal) -> SeatEnd {
        use crate::host::HostRefusal;
        match error {
            HostRefusal::WaitingForActivation
            | HostRefusal::StoppedBeforeWork
            | HostRefusal::IdentityChanged
            | HostRefusal::Unknown
            | HostRefusal::NoAllowance => SeatEnd::Closed(error),
            HostRefusal::Expired => SeatEnd::Paused(PauseReason::TurnTimeout),
            other => SeatEnd::Failed(PauseReason::CliError(other.to_string())),
        }
    }

    fn install_context(&mut self, request: &DecisionRequest) -> Result<(), SeatEnd> {
        let deadline = self.decision_deadline(request);
        let allowance = crate::operation::Allowance::required(Some(deadline))
            .map_err(|e| Self::closed(crate::host::operation_refusal(e)))?;
        let id = uuid::Uuid::new_v4().to_string();
        let operation = match self.ownership.archive.as_ref() {
            Some(archive) => archive
                .operation(self.seat, self.controller_epoch, id.clone(), allowance)
                .map_err(|error| {
                    use crate::failed_evidence::ArchiveError;
                    let refusal = match error {
                        ArchiveError::Operation(error) => crate::host::operation_refusal(error),
                        ArchiveError::Identity => crate::host::HostRefusal::IdentityChanged,
                        ArchiveError::Fit => crate::host::HostRefusal::CaptureOverflow,
                        ArchiveError::Path | ArchiveError::Filesystem | ArchiveError::Mismatch => {
                            crate::host::HostRefusal::Ownership(
                                "original campaign archive planning refused".into(),
                            )
                        }
                    };
                    Self::closed(refusal)
                })?,
            // Inert drivers can use the existing toy runner. Native Claude's
            // required plan guard refuses this context before any native effect.
            // No fabricated campaign database, archive path or fallback is supplied.
            None => crate::operation::Operation::new(id.clone(), self.controller_epoch, allowance)
                .map_err(|error| Self::closed(crate::host::operation_refusal(error)))?,
        };
        let context = crate::host::HostDriverContext {
            admission: self.host.admission.clone(),
            identity: self.identity(request, id),
            operation,
        };
        self.driver
            .set_host_context(context.clone())
            .map_err(|error| match error {
                DriverError::Host(refusal) => Self::closed(refusal),
                other => SeatEnd::Failed(PauseReason::CliError(other.to_string())),
            })?;
        self.ownership.context = Some(context);
        Ok(())
    }

    async fn stop_owned(&mut self) -> Result<(), DriverError> {
        if !self.ownership.native_entered {
            return Ok(());
        }
        let context = self
            .ownership
            .context
            .as_ref()
            .ok_or_else(|| DriverError::Host(crate::host::HostRefusal::NoAllowance))?;
        let result =
            tokio::time::timeout_at(context.operation.allowance().deadline(), self.driver.stop())
                .await
                .map_err(|_| {
                    DriverError::Host(crate::host::HostRefusal::Ownership(
                        "native child/readers remain foreground-owned Unsettled".into(),
                    ))
                })?;
        if result.is_ok() {
            self.ownership.native_entered = false;
        }
        result
    }

    /// Retry observation of retained cleanup only, using the original allowances. This never
    /// re-enters dispatch or re-admits a credit because a timer/drop/request ACK occurred.
    pub async fn settle_owned(&mut self) -> Result<(), DriverError> {
        self.stop_owned().await?;
        if let Some(owner) = self.ownership.forced.as_mut() {
            self.sink
                .join_delivery_until(owner.allowance().deadline())
                .await
                .map_err(|e| DriverError::Host(crate::host::HostRefusal::Ownership(e)))?;
            owner.request_settlement_after_supervision();
            let ack = owner.settlement_ack().await.map_err(DriverError::Host)?;
            if ack.uncertain() {
                return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                    "actor settlement uncertainty retained".into(),
                )));
            }
            self.ownership.forced = None;
        }
        self.ownership.permit.take();
        self.ownership.uncertain = false;
        Ok(())
    }
    fn set(&self, status: SeatStatus, reason: Option<PauseReason>) {
        let _ = self.state.send(SeatState { status, reason });
    }

    async fn pending(&self) -> Vec<DecisionRequest> {
        self.game.pending(self.seat).await
    }

    fn budget_exhausted(&self) -> Option<PauseReason> {
        if self.started.elapsed() > self.limits.max_wall {
            return Some(PauseReason::WallClockExhausted);
        }
        let calls = self
            .router
            .counters(self.seat)
            .map_or(0, |c| c.calls.load(std::sync::atomic::Ordering::SeqCst));
        (calls >= self.limits.max_tool_calls_per_seat).then_some(PauseReason::ToolBudgetExhausted)
    }

    /// Run until the game ends, the seat pauses or fails, or a stop is requested.
    pub async fn run(&mut self) -> SeatEnd {
        if self.ownership.is_unsettled() {
            let reason = match self.state.borrow().reason.clone() {
                Some(PauseReason::CliError(reason)) if reason.contains("Unsettled") => {
                    PauseReason::CliError(reason)
                }
                Some(reason) => PauseReason::CliError(format!(
                    "{reason}; original supervision remains owned Unsettled; no new admission"
                )),
                None => PauseReason::CliError(
                    "original supervision remains owned Unsettled; no new admission".into(),
                ),
            };
            self.set(SeatStatus::Paused, Some(reason.clone()));
            return SeatEnd::Paused(reason);
        }
        let changes = self.host.admission.subscribe();
        match self.host.admission.snapshot() {
            Ok(snapshot)
                if snapshot.activated && snapshot.status == crate::host::HostStatus::Running => {}
            Ok(snapshot) if snapshot.status == crate::host::HostStatus::Finished => {
                return SeatEnd::Finished;
            }
            Ok(snapshot) if !snapshot.activated => {
                return SeatEnd::Closed(crate::host::HostRefusal::WaitingForActivation);
            }
            Ok(_) => return SeatEnd::Closed(crate::host::HostRefusal::StoppedBeforeWork),
            Err(error) => return Self::closed(error),
        }
        let mut end = match tokio::time::timeout_at(
            self.started + self.limits.max_wall,
            self.run_inner(changes),
        )
        .await
        {
            Ok(end) => end,
            Err(_) => SeatEnd::Paused(PauseReason::WallClockExhausted),
        };
        if let Err(error) = self.settle_owned().await {
            self.ownership.uncertain = true;
            let original = match &end {
                SeatEnd::Paused(reason) | SeatEnd::Failed(reason) => format!("{reason}; "),
                _ => String::new(),
            };
            end = SeatEnd::Paused(PauseReason::CliError(format!(
                "{original}CLI cleanup remains Unsettled: {error}"
            )));
            self.set(
                SeatStatus::Paused,
                match &end {
                    SeatEnd::Paused(r) => Some(r.clone()),
                    _ => None,
                },
            );
            return end;
        }
        let deadline =
            (self.started + self.limits.max_wall).min(Instant::now() + Duration::from_secs(5));
        let drained = tokio::time::timeout_at(deadline, self.sink.flush_confirmed()).await;
        if !matches!(drained, Ok(Ok(()))) || self.sink.pending_count() != 0 {
            if let Err(error) = self.sink.join_delivery_until(deadline).await {
                self.ownership.uncertain = true;
                let original = match &end {
                    SeatEnd::Paused(r) | SeatEnd::Failed(r) => format!("{r}; "),
                    _ => String::new(),
                };
                return SeatEnd::Failed(PauseReason::CliError(format!(
                    "{original}{error}; {} unconfirmed captures remain owned",
                    self.sink.pending_count()
                )));
            }
            let original = match &end {
                SeatEnd::Paused(r) | SeatEnd::Failed(r) => format!("{r}; "),
                _ => String::new(),
            };
            end = SeatEnd::Failed(PauseReason::CliError(format!(
                "{original}transcript persistence did not drain; {} unconfirmed captures retained in the sink",
                self.sink.pending_count()
            )));
        }
        match &end {
            SeatEnd::Paused(r) => self.set(SeatStatus::Paused, Some(r.clone())),
            SeatEnd::Failed(r) => self.set(SeatStatus::Failed, Some(r.clone())),
            _ => self.set(SeatStatus::Idle, None),
        }
        end
    }

    async fn answer_automatic(&mut self, pending: &[DecisionRequest]) -> Result<bool, SeatEnd> {
        let mut answered = false;
        for request in pending
            .iter()
            .filter(|p| crate::automatic::forced_pass(&p.space))
        {
            if self.ownership.forced.is_some() {
                return Err(SeatEnd::Failed(PauseReason::CliError(
                    "forced supervision already owned".into(),
                )));
            }
            let deadline = self
                .decision_deadline(request)
                .min(Instant::now() + Duration::from_secs(5));
            let allowance = crate::operation::Allowance::required(Some(deadline))
                .map_err(|e| Self::closed(crate::host::operation_refusal(e)))?;
            let id = uuid::Uuid::new_v4().to_string();
            let operation =
                crate::operation::Operation::new(id.clone(), self.controller_epoch, allowance)
                    .map_err(|e| Self::closed(crate::host::operation_refusal(e)))?;
            let original = crate::game::SubmitRequest {
                decision_id: request.id.to_string(),
                epoch: self.controller_epoch,
                revision: Some(request.revision),
                idempotency_key: format!(
                    "automatic|{}|{}|e{}|r{}|null",
                    self.seat, request.id, self.controller_epoch, request.revision
                ),
                action: serde_json::Value::Null,
                public_explanation: None,
            };
            self.ownership.forced = Some(
                crate::automatic::ForcedAnswerSupervisor::new(
                    self.host.forced.clone(),
                    self.identity(request, id),
                    original,
                    operation,
                )
                .map_err(Self::closed)?,
            );
            let owner = self
                .ownership
                .forced
                .as_mut()
                .expect("retained forced reservation");
            let result = tokio::select! {
                result = owner.execute(&self.sink) => result,
                _ = async {
                    if self.stop.wait_for(|stop| *stop).await.is_err() {
                        std::future::pending::<()>().await;
                    }
                } => return Err(SeatEnd::Stopped),
            };
            answered |= result.map_err(Self::closed)?;
            // A previous model turn can leave the SAME session/readers alive. Observe actual
            // original supervision before asking the forced actor to settle, even though this
            // forced answer itself dispatched no native bytes.
            self.stop_owned()
                .await
                .map_err(|error| SeatEnd::Failed(PauseReason::CliError(error.to_string())))?;
            let owner = self
                .ownership
                .forced
                .as_mut()
                .expect("retained forced reservation");
            owner.request_settlement_after_supervision();
            let ack = owner.settlement_ack().await.map_err(Self::closed)?;
            if ack.uncertain() {
                return Err(SeatEnd::Failed(PauseReason::CliError(
                    "forced actor remains uncertain".into(),
                )));
            }
            self.ownership.forced = None;
        }
        Ok(answered)
    }

    async fn run_inner(
        &mut self,
        mut changes: watch::Receiver<crate::host::HostSnapshot>,
    ) -> SeatEnd {
        let mut fresh = self.sessions.load(self.seat).is_none();
        let mut alive = false;
        let mut recoveries = 0u32;
        loop {
            // Wait for something to decide.
            let pending = loop {
                if *self.stop.borrow() {
                    return SeatEnd::Stopped;
                }
                if self.game.outcome().await.is_some() {
                    return SeatEnd::Finished;
                }
                if self.started.elapsed() > self.limits.max_wall {
                    return SeatEnd::Paused(PauseReason::WallClockExhausted);
                }
                let p = self.pending().await;
                if !p.is_empty() {
                    break p;
                }
                if let Some(reason) = self.budget_exhausted() {
                    return SeatEnd::Paused(reason);
                }
                self.set(SeatStatus::Idle, None);
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_millis(250)) => {}
                    _ = self.stop.changed() => {}
                    _ = changes.changed() => {
                        match self.host.admission.snapshot() {
                            Ok(snapshot) if snapshot.activated && snapshot.status == crate::host::HostStatus::Running => {},
                            Ok(_) => return SeatEnd::Closed(crate::host::HostRefusal::StoppedBeforeWork),
                            Err(error) => return Self::closed(error),
                        }
                    }
                }
            };
            match self.answer_automatic(&pending).await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(end) => return end,
            }
            if let Some(reason) = self.budget_exhausted() {
                return SeatEnd::Paused(reason);
            }
            self.set(SeatStatus::Deciding, None);

            // Stamp this revision's original deadline BEFORE waiting for a scarce session
            // permit or preparing prompts. Retries and later awaits cannot renew it.
            let admission_deadline = self.decision_deadline(&pending[0]);

            // Take a session slot before touching the CLI.
            if self.ownership.permit.is_none() {
                self.ownership.permit = match tokio::time::timeout_at(
                    admission_deadline,
                    self.permits.clone().acquire_owned(),
                )
                .await
                {
                    Ok(Ok(p)) => Some(p),
                    Ok(Err(_)) => return SeatEnd::Stopped,
                    Err(_) => return SeatEnd::Paused(PauseReason::TurnTimeout),
                };
            }

            if !alive {
                if let Err(end) = self.install_context(&pending[0]) {
                    return end;
                }
                let deadline = self
                    .ownership
                    .context
                    .as_ref()
                    .expect("required startup context")
                    .operation
                    .allowance()
                    .deadline();
                match tokio::time::timeout_at(deadline, self.bring_up(fresh))
                    .await
                    .unwrap_or_else(|_| Err(SeatEnd::Paused(PauseReason::TurnTimeout)))
                {
                    Ok(was_fresh) => {
                        fresh = was_fresh;
                        alive = true;
                    }
                    Err(end) => return end,
                }
            }

            // One decision window: first/window turn, then nudges until nothing is pending.
            let mut kind = if fresh {
                TurnKind::First
            } else {
                TurnKind::Window
            };
            let mut nudges = 0;
            let mut pending = pending;
            loop {
                // New revisions can open immediately after a model submission or
                // crash recovery. Apply the same no-model rule before every nudge.
                match self.answer_automatic(&pending).await {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(end) => return end,
                }
                if let Err(end) = self.install_context(&pending[0]) {
                    return end;
                }
                let deadline = self
                    .ownership
                    .context
                    .as_ref()
                    .expect("required prompt context")
                    .operation
                    .allowance()
                    .deadline();
                let prompt = match kind {
                    TurnKind::First => {
                        let notebook = match tokio::time::timeout_at(
                            deadline,
                            self.memory.notebook_read(self.seat),
                        )
                        .await
                        {
                            Ok(notebook) => notebook,
                            Err(_) => return SeatEnd::Paused(PauseReason::TurnTimeout),
                        };
                        self.prompts.first_turn(self.seat, &notebook, &pending)
                    }
                    TurnKind::Window => self.prompts.window_turn(self.seat, &pending),
                    TurnKind::Nudge => self.prompts.nudge(self.seat, &pending),
                };
                match tokio::time::timeout_at(
                    deadline,
                    self.driver.run_turn(&prompt, self.limits.turn_timeout),
                )
                .await
                .unwrap_or(Err(DriverError::Timeout))
                {
                    Ok(outcome) => {
                        if let Some(id) = self.driver.session_id() {
                            self.sessions.save(self.seat, &id);
                        }
                        fresh = false;
                        if let Some(end) = self.after_turn(&outcome) {
                            return end;
                        }
                    }
                    Err(DriverError::Isolation(e)) => {
                        return SeatEnd::Failed(PauseReason::IsolationFailure(e));
                    }
                    Err(DriverError::Cli(e)) => return SeatEnd::Paused(PauseReason::CliError(e)),
                    Err(DriverError::Host(error)) => return Self::closed(error),
                    Err(
                        e @ (DriverError::Died(_)
                        | DriverError::Timeout
                        | DriverError::TimeoutAt(_)
                        | DriverError::Protocol(_)),
                    ) => {
                        let timed_out = e.is_timeout();
                        self.sink.system(self.seat, format!("session problem: {e}"));
                        recoveries += 1;
                        if recoveries > self.limits.max_recoveries {
                            return SeatEnd::Paused(PauseReason::RecoveriesExhausted);
                        }
                        if timed_out && recoveries > 1 {
                            return SeatEnd::Paused(PauseReason::TurnTimeout);
                        }
                        // Kill whatever is left and bring the session back (resume, else reseed).
                        if let Err(error) = self.stop_owned().await {
                            return SeatEnd::Failed(PauseReason::CliError(error.to_string()));
                        }
                        if let Err(end) = self.install_context(&pending[0]) {
                            return end;
                        }
                        let deadline = self
                            .ownership
                            .context
                            .as_ref()
                            .expect("required recovery context")
                            .operation
                            .allowance()
                            .deadline();
                        match tokio::time::timeout_at(deadline, self.bring_up(false))
                            .await
                            .unwrap_or_else(|_| Err(SeatEnd::Paused(PauseReason::TurnTimeout)))
                        {
                            Ok(was_fresh) => fresh = was_fresh,
                            Err(end) => return end,
                        }
                        kind = if fresh {
                            TurnKind::First
                        } else {
                            TurnKind::Window
                        };
                        pending = self.pending().await;
                        if pending.is_empty() {
                            break;
                        }
                        continue;
                    }
                    Err(e @ DriverError::Spawn { .. }) => {
                        return SeatEnd::Paused(PauseReason::CliError(e.to_string()));
                    }
                }
                pending = self.pending().await;
                if pending.is_empty() {
                    break;
                }
                if pending
                    .iter()
                    .any(|p| crate::automatic::forced_pass(&p.space))
                {
                    break;
                }
                if let Some(r) = self.budget_exhausted() {
                    return SeatEnd::Paused(r);
                }
                if nudges >= self.limits.max_nudges {
                    return SeatEnd::Paused(PauseReason::Unanswered);
                }
                nudges += 1;
                kind = TurnKind::Nudge;
            }
            if self.limits.park_between_windows {
                if let Err(error) = self.stop_owned().await {
                    return SeatEnd::Failed(PauseReason::CliError(error.to_string()));
                }
                alive = false;
                self.ownership.permit = None;
            }
        }
    }

    /// A turn the CLI itself reports as failed (quota, auth, model error) pauses the seat.
    fn after_turn(&self, outcome: &TurnOutcome) -> Option<SeatEnd> {
        if outcome.ok {
            return None;
        }
        Some(SeatEnd::Paused(PauseReason::CliError(
            outcome
                .error
                .clone()
                .unwrap_or_else(|| "turn failed".into()),
        )))
    }

    /// Start the CLI: resume the stored session if there is one and `try_resume`, else begin a
    /// new session. Returns whether the session is fresh (needs the seeded first turn).
    async fn bring_up(&mut self, fresh_hint: bool) -> Result<bool, SeatEnd> {
        let stored = self.sessions.load(self.seat);
        if let (Some(id), false) = (&stored, fresh_hint) {
            self.ownership.native_entered = true;
            match self.driver.start(Some(id)).await {
                Ok(_) => return Ok(false),
                Err(DriverError::Isolation(e)) => {
                    return Err(SeatEnd::Failed(PauseReason::IsolationFailure(e)));
                }
                Err(DriverError::Host(error)) => return Err(Self::closed(error)),
                Err(e) => {
                    self.sink.system(
                        self.seat,
                        format!("could not resume session {id} ({e}); starting a new one from the notebook"),
                    );
                    self.stop_owned().await.map_err(|error| {
                        SeatEnd::Failed(PauseReason::CliError(error.to_string()))
                    })?;
                    self.sessions.clear(self.seat);
                }
            }
        }
        self.ownership.native_entered = true;
        match self.driver.start(None).await {
            Ok(info) => {
                self.sessions.save(self.seat, &info.session_id);
                Ok(true)
            }
            Err(DriverError::Isolation(e)) => {
                Err(SeatEnd::Failed(PauseReason::IsolationFailure(e)))
            }
            Err(DriverError::Host(error)) => Err(Self::closed(error)),
            Err(e) => Err(SeatEnd::Paused(PauseReason::CliError(e.to_string()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::{CliKind, SessionInfo, Usage};
    use crate::mcp::ToolRouter;
    use crate::memory::InMemorySeatMemory;
    use crate::toy::{AXIS, COMMONWEALTH, NumberDuel};
    use crate::transcript::LocalTranscript;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    /// A scripted "CLI": it answers by calling the real tool router like a model would.
    struct FakeDriver {
        seat: SeatId,
        router: Arc<ToolRouter>,
        /// Which turn numbers (1-based) behave how.
        script: Script,
        turns: u32,
        starts: Arc<AtomicU32>,
        resumes: Arc<AtomicU32>,
        cleanup_observed: Arc<AtomicBool>,
        session: String,
        alive: bool,
    }

    #[derive(Clone, Copy)]
    enum Script {
        /// Observe and submit every pending decision.
        Plays,
        /// Never answers anything (ends turns immediately).
        Silent,
        /// No submission, and stop cannot establish a child/readers join.
        CleanupUnknown,
        /// Owns the same inert session while a borrowed startup wait is canceled.
        StartWait,
        /// Plays, but the process "dies" during turn 2 *after* submitting.
        DiesAfterSubmitOnTurn2,
        TimesOut(crate::driver::TimeoutKind),
    }

    #[async_trait]
    impl SeatDriver for FakeDriver {
        fn set_host_context(
            &mut self,
            context: crate::host::HostDriverContext,
        ) -> Result<(), DriverError> {
            crate::host_fixture::accept_inert_context(context)
        }
        fn kind(&self) -> CliKind {
            CliKind::Claude
        }
        async fn start(&mut self, resume: Option<&str>) -> Result<SessionInfo, DriverError> {
            if resume.is_some() {
                self.resumes.fetch_add(1, Ordering::SeqCst);
            } else {
                self.starts.fetch_add(1, Ordering::SeqCst);
                self.session = format!("s{}", self.starts.load(Ordering::SeqCst));
            }
            self.alive = true;
            if matches!(self.script, Script::StartWait) {
                std::future::pending::<()>().await;
            }
            Ok(SessionInfo {
                session_id: self.session.clone(),
                model: None,
                cli_version: None,
                resumed: resume.is_some(),
            })
        }
        async fn run_turn(
            &mut self,
            _prompt: &str,
            _limit: Duration,
        ) -> Result<TurnOutcome, DriverError> {
            self.turns += 1;
            if let Script::TimesOut(kind) = self.script {
                return if kind == crate::driver::TimeoutKind::LegacyUnclassified {
                    Err(DriverError::Timeout)
                } else {
                    let mut diagnostic = crate::driver::TimeoutDiagnostic::legacy();
                    diagnostic.kind = kind;
                    Err(DriverError::TimeoutAt(diagnostic))
                };
            }
            let play = !matches!(self.script, Script::Silent | Script::CleanupUnknown);
            if play {
                let obs = self
                    .router
                    .call(self.seat, 1, "observe", &json!({}))
                    .await
                    .unwrap();
                for d in obs["pending_decisions"].as_array().unwrap() {
                    let card = obs["observation"]["your_hand"][0].to_string();
                    self.router
                        .call(
                            self.seat,
                            1,
                            "submit",
                            &json!({ "decision_id": d["decision_id"], "action": card }),
                        )
                        .await
                        .unwrap();
                }
            }
            if matches!(self.script, Script::DiesAfterSubmitOnTurn2) && self.turns == 2 {
                self.alive = false;
                return Err(DriverError::Died("killed".into()));
            }
            Ok(TurnOutcome {
                ok: true,
                error: None,
                text: Some("done".into()),
                usage: Usage::default(),
                quota: vec![],
            })
        }
        fn session_id(&self) -> Option<String> {
            Some(self.session.clone())
        }
        fn is_alive(&mut self) -> bool {
            self.alive
        }
        async fn stop(&mut self) -> Result<(), crate::driver::DriverError> {
            if matches!(self.script, Script::CleanupUnknown)
                && !self.cleanup_observed.load(Ordering::SeqCst)
            {
                return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                    "inert unresolved child/readers".into(),
                )));
            }
            self.alive = false;

            Ok(())
        }
    }

    struct Harness {
        host: RunnerHost,
        game: Arc<dyn GameBackend>,
        router: Arc<ToolRouter>,
        sink: TranscriptSink,
        stored: Arc<LocalTranscript>,
        sessions: Arc<InMemorySessions>,
        permits: Arc<Semaphore>,
        starts: Arc<AtomicU32>,
        resumes: Arc<AtomicU32>,
        cleanup_observed: Arc<AtomicBool>,
    }

    fn harness() -> Harness {
        let seats = vec![AXIS, COMMONWEALTH];
        let toy = Arc::new(NumberDuel::game(11));
        let host = crate::toy_host::ToyHost::fresh_number_duel(toy.clone());
        let game: Arc<dyn GameBackend> = toy;
        let memory = Arc::new(InMemorySeatMemory::new(seats.clone()));
        let router = Arc::new(ToolRouter::new(game.clone(), memory, &seats));
        let stored = LocalTranscript::detached();
        Harness {
            host: RunnerHost {
                admission: host.clone(),
                forced: host,
            },
            game,
            router,
            sink: TranscriptSink::new(stored.clone()),
            stored,
            sessions: Arc::new(InMemorySessions::default()),
            permits: Arc::new(Semaphore::new(2)),
            starts: Arc::new(AtomicU32::new(0)),
            resumes: Arc::new(AtomicU32::new(0)),
            cleanup_observed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn runner(h: &Harness, seat: SeatId, script: Script, limits: RunLimits) -> SeatRunner {
        let (state, _) = watch::channel(SeatState {
            status: SeatStatus::Idle,
            reason: None,
        });
        let (_, stop) = watch::channel(false);
        SeatRunner {
            seat,
            controller_epoch: 1,
            driver: Box::new(FakeDriver {
                seat,
                router: h.router.clone(),
                script,
                turns: 0,
                starts: h.starts.clone(),
                resumes: h.resumes.clone(),
                cleanup_observed: h.cleanup_observed.clone(),
                session: String::new(),
                alive: false,
            }),
            game: h.game.clone(),
            host: h.host.clone(),
            ownership: RunnerOwnership::empty(),
            memory: Arc::new(InMemorySeatMemory::new(vec![AXIS, COMMONWEALTH])),
            router: h.router.clone(),
            sink: h.sink.clone(),
            sessions: h.sessions.clone(),
            prompts: Arc::new(DefaultPrompts {
                game_description: "A toy.".into(),
            }),
            limits,
            permits: h.permits.clone(),
            started: Instant::now(),
            state,
            stop,
        }
    }

    fn archive_configuration(root: &std::path::Path) -> crate::failed_evidence::CampaignArchive {
        crate::failed_evidence::CampaignArchive::new(
            &root.join("campaigns/11111111-1111-4111-8111-111111111111.sqlite"),
            &root.join("sandbox"),
            &root.join("run"),
            &[root.join("isolation")],
        )
        .unwrap()
    }

    // SOURCE ONLY/UNRUN: synthetic path configuration and an inert driver; no
    // database, path probe, archive write, child, model or provider is invoked.
    #[tokio::test]
    async fn campaign_runner_installs_original_plan_before_borrowed_work() {
        let h = harness();
        let mut a = runner(&h, AXIS, Script::Plays, RunLimits::default());
        let root = std::env::temp_dir().join("synthetic-runner-archive-no-io");
        a.ownership = RunnerOwnership::for_campaign(archive_configuration(&root));
        let pending = a.pending().await;
        let deadline = a.decision_deadline(&pending[0]);
        a.install_context(&pending[0]).unwrap();
        let retained = a.ownership.context.as_ref().unwrap();
        let original = retained.operation.clone();
        let plan = original.archive_plan().unwrap();
        assert_eq!(plan.seat, AXIS);
        assert_eq!(plan.epoch, a.controller_epoch);
        assert_eq!(original.id(), retained.identity.operation_id);
        assert_eq!(original.allowance().deadline(), deadline);
        assert_eq!(retained.identity.decision_id, pending[0].id.to_string());
        assert_eq!(retained.identity.revision, pending[0].revision);
        let charged = original.captured_bytes();
        let chunk = original.borrow_archive_chunk(&original).unwrap();
        assert!(retained.operation.same_instance(&original));
        drop(chunk);
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(a.decision_deadline(&pending[0]), deadline);
        assert_eq!(h.starts.load(Ordering::SeqCst), 0);
        assert_eq!(h.permits.available_permits(), 2);
        assert!(!a.ownership.is_unsettled());
    }

    #[tokio::test]
    async fn campaign_archive_fit_refuses_before_context_or_native_start() {
        let h = harness();
        let mut a = runner(&h, AXIS, Script::Plays, RunLimits::default());
        // Configuration construction is inert; the ORIGINAL operation's fixed
        // conversion reserve exceeds 16 MiB and must refuse without a fallback.
        let root = std::env::temp_dir().join("x".repeat(4000));
        a.ownership = RunnerOwnership::for_campaign(archive_configuration(&root));
        let pending = a.pending().await;
        assert!(matches!(a.install_context(&pending[0]),
            Err(SeatEnd::Failed(PauseReason::CliError(ref reason)))
                if reason == &crate::host::HostRefusal::CaptureOverflow.to_string()));
        assert!(a.ownership.archive.is_some());
        assert!(a.ownership.context.is_none());
        assert_eq!(h.starts.load(Ordering::SeqCst), 0);
        assert_eq!(h.permits.available_permits(), 2);
        assert!(!a.ownership.is_unsettled());
    }

    #[tokio::test]
    async fn expired_campaign_decision_retains_original_context_and_credit() {
        let h = harness();
        let mut a = runner(&h, AXIS, Script::Plays, RunLimits::default());
        let root = std::env::temp_dir().join("synthetic-expired-runner-archive-no-io");
        a.ownership = RunnerOwnership::for_campaign(archive_configuration(&root));
        let pending = a.pending().await;
        a.install_context(&pending[0]).unwrap();
        let original = a.ownership.context.as_ref().unwrap().operation.clone();
        let deadline = original.allowance().deadline();
        let charged = original.captured_bytes();
        // No wall-clock sleep or synthetic ACK. The existing decision ceiling
        // shortens admission only; it does not alter/release the retained original.
        a.ownership.decision.as_mut().unwrap().2 = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            a.install_context(&pending[0]),
            Err(SeatEnd::Paused(PauseReason::TurnTimeout))
        );
        assert!(
            a.ownership
                .context
                .as_ref()
                .unwrap()
                .operation
                .same_instance(&original)
        );
        assert_eq!(original.allowance().deadline(), deadline);
        assert_eq!(original.captured_bytes(), charged);
        assert!(original.archive_plan().is_some());
        assert_eq!(h.starts.load(Ordering::SeqCst), 0);
        assert_eq!(h.permits.available_permits(), 2);
    }

    #[tokio::test]
    async fn activation_closed_does_not_start_or_fail_the_seat() {
        let h = harness();
        let closed = crate::host_fixture::FixtureHost::explicit(
            h.game.clone(),
            h.stored.clone(),
            [(AXIS, 1), (COMMONWEALTH, 1)].into(),
        );
        closed.set_activation(false);
        let mut a = runner(&h, AXIS, Script::Plays, RunLimits::default());
        a.host = RunnerHost {
            admission: closed.clone(),
            forced: closed,
        };
        assert_eq!(
            a.run().await,
            SeatEnd::Closed(crate::host::HostRefusal::WaitingForActivation)
        );
        assert_eq!(h.starts.load(Ordering::SeqCst), 0);
        assert_eq!(a.state.borrow().status, SeatStatus::Idle);
        assert_eq!(h.permits.available_permits(), 2);
        assert!(!a.ownership.is_unsettled());
        assert_eq!(
            h.game.observe(AXIS).await["history"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn unknown_cleanup_keeps_original_permit_and_refuses_reentry() {
        let h = harness();
        let mut a = runner(
            &h,
            AXIS,
            Script::CleanupUnknown,
            RunLimits {
                max_nudges: 0,
                ..RunLimits::default()
            },
        );
        assert!(
            matches!(a.run().await, SeatEnd::Paused(PauseReason::CliError(ref reason)) if reason.contains("Unsettled"))
        );
        assert!(a.ownership.is_unsettled());
        assert_eq!(a.state.borrow().status, SeatStatus::Paused);
        let original_reason = a.state.borrow().reason.clone();
        assert_eq!(h.permits.available_permits(), 1);
        assert!(
            matches!(a.run().await, SeatEnd::Paused(PauseReason::CliError(ref reason)) if reason.contains("Unsettled"))
        );
        assert_eq!(a.state.borrow().reason, original_reason);
        assert_eq!(h.starts.load(Ordering::SeqCst), 1);
        assert_eq!(h.permits.available_permits(), 1);
        // SAME inert driver now reports its actual cleanup observation. No owner replacement,
        // drop, timer or permit reacquisition substitutes for the next successful stop.
        h.cleanup_observed.store(true, Ordering::SeqCst);
        a.settle_owned().await.unwrap();
        assert_eq!(h.permits.available_permits(), 2);
        assert!(!a.ownership.is_unsettled());
    }

    #[tokio::test]
    async fn canceled_borrowed_run_retains_same_session_and_permit_until_stop() {
        let h = harness();
        let mut a = runner(&h, AXIS, Script::StartWait, RunLimits::default());
        assert!(
            tokio::time::timeout(Duration::from_millis(1), a.run())
                .await
                .is_err()
        );
        assert!(a.ownership.is_unsettled());
        assert!(a.ownership.native_entered);
        assert_eq!(h.starts.load(Ordering::SeqCst), 1);
        assert_eq!(h.permits.available_permits(), 1);
        assert!(
            matches!(a.run().await, SeatEnd::Paused(PauseReason::CliError(ref reason)) if reason.contains("Unsettled"))
        );
        assert_eq!(h.starts.load(Ordering::SeqCst), 1);
        a.settle_owned().await.unwrap();
        assert!(!a.ownership.is_unsettled());
        assert_eq!(h.permits.available_permits(), 2);
    }

    #[tokio::test]
    async fn same_revision_deadline_and_permit_wait_never_renew() {
        let h = harness();
        let mut a = runner(
            &h,
            AXIS,
            Script::Plays,
            RunLimits {
                turn_timeout: Duration::from_millis(20),
                ..RunLimits::default()
            },
        );
        let request = h.game.pending(AXIS).await.remove(0);
        let first = a.decision_deadline(&request);
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(a.decision_deadline(&request), first);
        a.permits = Arc::new(Semaphore::new(0));
        assert_eq!(a.run().await, SeatEnd::Paused(PauseReason::TurnTimeout));
        assert_eq!(h.starts.load(Ordering::SeqCst), 0);
        assert!(!a.ownership.native_entered);
        assert_eq!(a.decision_deadline(&request), first);
    }

    #[tokio::test]
    async fn two_seats_play_a_whole_game() {
        let h = harness();
        let mut a = runner(&h, AXIS, Script::Plays, RunLimits::default());
        let mut b = runner(&h, COMMONWEALTH, Script::Plays, RunLimits::default());
        let (ea, eb) = tokio::join!(a.run(), b.run());
        assert_eq!((ea, eb), (SeatEnd::Finished, SeatEnd::Finished));
        assert!(h.game.outcome().await.is_some());
        assert_eq!(h.starts.load(Ordering::SeqCst), 2, "one session per seat");
        assert_eq!(h.resumes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_silent_seat_is_paused_and_nothing_is_submitted_for_it() {
        let h = harness();
        let mut a = runner(
            &h,
            AXIS,
            Script::Silent,
            RunLimits {
                max_nudges: 1,
                ..RunLimits::default()
            },
        );
        let end = a.run().await;
        assert_eq!(end, SeatEnd::Paused(PauseReason::Unanswered));
        assert_eq!(
            h.game.pending(AXIS).await.len(),
            1,
            "the decision is still open"
        );
        assert!(h.game.outcome().await.is_none());
    }

    #[tokio::test]
    async fn a_tool_budget_pauses_the_seat() {
        let h = harness();
        let mut a = runner(
            &h,
            AXIS,
            Script::Plays,
            RunLimits {
                max_tool_calls_per_seat: 2,
                ..RunLimits::default()
            },
        );
        // Round 1 uses observe+submit = 2 calls; the budget stops it before round 2 is started.
        assert_eq!(
            a.run().await,
            SeatEnd::Paused(PauseReason::ToolBudgetExhausted)
        );
    }

    #[tokio::test]
    async fn a_killed_session_resumes_without_duplicates_or_lost_submissions() {
        let h = harness();
        let mut a = runner(
            &h,
            AXIS,
            Script::DiesAfterSubmitOnTurn2,
            RunLimits::default(),
        );
        let mut b = runner(&h, COMMONWEALTH, Script::Plays, RunLimits::default());
        let (ea, eb) = tokio::join!(a.run(), b.run());
        assert_eq!((ea, eb), (SeatEnd::Finished, SeatEnd::Finished));
        assert_eq!(
            h.resumes.load(Ordering::SeqCst),
            1,
            "the same session was resumed"
        );
        // Every round was played exactly once by each seat.
        let obs = h.game.observe(AXIS).await;
        assert_eq!(obs["history"].as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn parking_lets_two_seats_share_one_session_slot() {
        let h = harness();
        let h = Harness {
            permits: Arc::new(Semaphore::new(1)),
            ..h
        };
        let limits = RunLimits {
            park_between_windows: true,
            ..RunLimits::default()
        };
        let mut a = runner(&h, AXIS, Script::Plays, limits.clone());
        let mut b = runner(&h, COMMONWEALTH, Script::Plays, limits);
        let (ea, eb) = tokio::join!(a.run(), b.run());
        assert_eq!((ea, eb), (SeatEnd::Finished, SeatEnd::Finished));
        // Each window after the first resumed the parked session instead of starting a new one.
        assert_eq!(h.starts.load(Ordering::SeqCst), 2);
        assert!(h.resumes.load(Ordering::SeqCst) >= 4);
    }
    #[tokio::test]
    async fn dead_transcript_store_cannot_hold_runner_cleanup_forever() {
        struct Dead;
        #[async_trait]
        impl crate::transcript::TranscriptStore for Dead {
            async fn append(
                &self,
                _: SeatId,
                _: String,
                _: cna_protocol::TranscriptEntry,
            ) -> Result<u64, String> {
                std::future::pending().await
            }
        }
        let mut h = harness();
        h.sink = TranscriptSink::new(Arc::new(Dead));
        h.sink.system(AXIS, "unconfirmed");
        let mut run = runner(
            &h,
            AXIS,
            Script::Silent,
            RunLimits {
                max_nudges: 0,
                ..RunLimits::default()
            },
        );
        let end = tokio::time::timeout(Duration::from_secs(7), run.run())
            .await
            .expect("bounded flush");
        assert!(
            matches!(end,SeatEnd::Failed(PauseReason::CliError(ref s)) if s.contains("pending decisions") && s.contains("unconfirmed"))
        );
        assert_eq!(h.sink.pending_count(), 1);
    }

    #[tokio::test]
    async fn both_timeout_categories_and_legacy_keep_existing_recovery_pause_and_private_transcript()
     {
        use crate::driver::TimeoutKind;
        for kind in [
            TimeoutKind::TurnReservation,
            TimeoutKind::TranscriptConfirmation,
            TimeoutKind::LegacyUnclassified,
        ] {
            let h = harness();
            let mut r = runner(&h, AXIS, Script::TimesOut(kind), RunLimits::default());
            let result = r.run().await;
            let entries = h.stored.seat_log(AXIS);
            h.sink.stop_delivery().await;
            assert_eq!(result, SeatEnd::Paused(PauseReason::TurnTimeout));
            assert_eq!(h.starts.load(Ordering::SeqCst), 1);
            assert_eq!(h.resumes.load(Ordering::SeqCst), 1);
            assert!(h.game.outcome().await.is_none());
            let wire =
                serde_json::to_string(&entries.iter().map(|e| e.to_message()).collect::<Vec<_>>())
                    .unwrap();
            assert!(wire.contains("the turn exceeded its time limit"));
            for private in [
                "turn_reservation",
                "transcript_confirmation",
                "legacy_unclassified",
                "phase_elapsed_ms",
            ] {
                assert!(!wire.contains(private));
            }
            let h = harness();
            let limits = RunLimits {
                max_recoveries: 0,
                ..RunLimits::default()
            };
            let mut r = runner(&h, AXIS, Script::TimesOut(kind), limits);
            let result = r.run().await;
            h.sink.stop_delivery().await;
            assert_eq!(result, SeatEnd::Paused(PauseReason::RecoveriesExhausted));
            assert_eq!(h.resumes.load(Ordering::SeqCst), 0);
        }
    }
}
