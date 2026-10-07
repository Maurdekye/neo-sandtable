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
    Paused(PauseReason),
    Failed(PauseReason),
}

/// Everything one seat runner needs.
pub struct SeatRunner {
    pub seat: SeatId,
    pub controller_epoch: u64,
    pub driver: Box<dyn SeatDriver>,
    pub game: Arc<dyn GameBackend>,
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
        let mut end = self.run_inner().await;
        self.driver.stop().await;
        let drained = tokio::time::timeout(Duration::from_secs(5), self.sink.flush()).await;
        if drained.is_err() || self.sink.pending_count() != 0 {
            self.sink.stop_delivery().await;
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
            let remaining = self.limits.max_wall.saturating_sub(self.started.elapsed());
            let result = tokio::select! {
                result = tokio::time::timeout(remaining, crate::automatic::answer(
                    self.game.as_ref(), self.seat, self.controller_epoch, request, &self.sink,
                )) => result.map_err(|_| SeatEnd::Paused(PauseReason::WallClockExhausted))?,
                _ = async {
                    if self.stop.wait_for(|stop| *stop).await.is_err() {
                        std::future::pending::<()>().await;
                    }
                } => return Err(SeatEnd::Stopped),
            };
            answered |=
                result.map_err(|e| SeatEnd::Paused(PauseReason::CliError(e.to_string())))?;
        }
        Ok(answered)
    }

    async fn run_inner(&mut self) -> SeatEnd {
        let mut fresh = self.sessions.load(self.seat).is_none();
        let mut alive = false;
        let mut recoveries = 0u32;
        let mut permit: Option<OwnedSemaphorePermit> = None;
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

            // Take a session slot before touching the CLI.
            if permit.is_none() {
                permit = match self.permits.clone().acquire_owned().await {
                    Ok(p) => Some(p),
                    Err(_) => return SeatEnd::Stopped,
                };
            }

            if !alive {
                match self.bring_up(fresh).await {
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
                let prompt = match kind {
                    TurnKind::First => {
                        let notebook = self.memory.notebook_read(self.seat).await;
                        self.prompts.first_turn(self.seat, &notebook, &pending)
                    }
                    TurnKind::Window => self.prompts.window_turn(self.seat, &pending),
                    TurnKind::Nudge => self.prompts.nudge(self.seat, &pending),
                };
                match self
                    .driver
                    .run_turn(&prompt, self.limits.turn_timeout)
                    .await
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
                    Err(
                        e
                        @ (DriverError::Died(_) | DriverError::Timeout | DriverError::Protocol(_)),
                    ) => {
                        let timed_out = matches!(e, DriverError::Timeout);
                        self.sink.system(self.seat, format!("session problem: {e}"));
                        recoveries += 1;
                        if recoveries > self.limits.max_recoveries {
                            return SeatEnd::Paused(PauseReason::RecoveriesExhausted);
                        }
                        if timed_out && recoveries > 1 {
                            return SeatEnd::Paused(PauseReason::TurnTimeout);
                        }
                        // Kill whatever is left and bring the session back (resume, else reseed).
                        self.driver.stop().await;
                        match self.bring_up(false).await {
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
                self.driver.stop().await;
                alive = false;
                permit = None;
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
            match self.driver.start(Some(id)).await {
                Ok(_) => return Ok(false),
                Err(DriverError::Isolation(e)) => {
                    return Err(SeatEnd::Failed(PauseReason::IsolationFailure(e)));
                }
                Err(e) => {
                    self.sink.system(
                        self.seat,
                        format!("could not resume session {id} ({e}); starting a new one from the notebook"),
                    );
                    self.driver.stop().await;
                    self.sessions.clear(self.seat);
                }
            }
        }
        match self.driver.start(None).await {
            Ok(info) => {
                self.sessions.save(self.seat, &info.session_id);
                Ok(true)
            }
            Err(DriverError::Isolation(e)) => {
                Err(SeatEnd::Failed(PauseReason::IsolationFailure(e)))
            }
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
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A scripted "CLI": it answers by calling the real tool router like a model would.
    struct FakeDriver {
        seat: SeatId,
        router: Arc<ToolRouter>,
        /// Which turn numbers (1-based) behave how.
        script: Script,
        turns: u32,
        starts: Arc<AtomicU32>,
        resumes: Arc<AtomicU32>,
        session: String,
        alive: bool,
    }

    #[derive(Clone, Copy)]
    enum Script {
        /// Observe and submit every pending decision.
        Plays,
        /// Never answers anything (ends turns immediately).
        Silent,
        /// Plays, but the process "dies" during turn 2 *after* submitting.
        DiesAfterSubmitOnTurn2,
    }

    #[async_trait]
    impl SeatDriver for FakeDriver {
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
            let play = !matches!(self.script, Script::Silent);
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
        async fn stop(&mut self) {
            self.alive = false;
        }
    }

    struct Harness {
        game: Arc<dyn GameBackend>,
        router: Arc<ToolRouter>,
        sink: TranscriptSink,
        sessions: Arc<InMemorySessions>,
        permits: Arc<Semaphore>,
        starts: Arc<AtomicU32>,
        resumes: Arc<AtomicU32>,
    }

    fn harness() -> Harness {
        let seats = vec![AXIS, COMMONWEALTH];
        let game: Arc<dyn GameBackend> = Arc::new(NumberDuel::game(11));
        let memory = Arc::new(InMemorySeatMemory::new(seats.clone()));
        let router = Arc::new(ToolRouter::new(game.clone(), memory, &seats));
        Harness {
            game,
            router,
            sink: TranscriptSink::new(LocalTranscript::detached()),
            sessions: Arc::new(InMemorySessions::default()),
            permits: Arc::new(Semaphore::new(2)),
            starts: Arc::new(AtomicU32::new(0)),
            resumes: Arc::new(AtomicU32::new(0)),
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
                session: String::new(),
                alive: false,
            }),
            game: h.game.clone(),
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
}
