//! Session drivers: one long-lived CLI session per seat.
//!
//! A [`SeatDriver`] owns one seat's CLI process (or its resumable session) and turns the CLI's
//! output into transcript entries. The shared plumbing lives here: the child-process wrapper, the
//! scrubbed environment every seat runs with, and the event vocabulary the per-CLI stream parsers
//! produce. Parsers are pure (`line -> events`) so they are tested against recorded fixtures.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cna_core::ids::SeatId;
use cna_protocol::TranscriptEntry;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::time::Instant;

pub mod antigravity;
mod archive_stderr;
mod archive_stdout;
mod archive_thread;
mod archive_work;
mod charged_json;
mod child_pending;
pub mod claude;
mod claude_caller;
pub(crate) mod claude_correlation;
pub mod codex;
mod event_storage;
mod frame_queue;
mod owned_driver_task;
mod reader_failure;
pub(crate) mod reader_handoff;

/// Which CLI drives a seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliKind {
    Claude,
    Codex,
    Antigravity,
}

impl CliKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CliKind::Claude => "claude-code",
            CliKind::Codex => "codex",
            CliKind::Antigravity => "antigravity",
        }
    }
}

/// Token and cost accounting for one turn, where the CLI reports it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    /// CLI estimate; Claude streaming reports cumulative session cost.
    pub cost_usd: Option<f64>,
    /// Whether every reported model price basis is list or managed (not unknown/missing).
    #[serde(default)]
    pub cost_basis_known: Option<bool>,
}

/// A quota reading the CLI volunteered (Claude Code emits these on every turn).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuotaReading {
    pub window: String,
    /// 0.0..=1.0
    pub utilization: f64,
    pub resets_at_epoch_s: Option<i64>,
}

/// How a turn ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnOutcome {
    pub ok: bool,
    /// The CLI's own error text when `ok` is false.
    pub error: Option<String>,
    /// The final assistant text of the turn.
    pub text: Option<String>,
    pub usage: Usage,
    pub quota: Vec<QuotaReading>,
}

/// What a stream parser extracts from one line of CLI output.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    /// Private accounting evidence; it is never emitted as a spectator entry.
    ModelStart {
        session_id: String,
        frame: crate::operation::ConfirmationFrame,
    },
    /// The session is up. `tools` is the CLI's own list of tools the model can call; the driver
    /// checks it against the isolation policy.
    Init {
        session_id: String,
        model: Option<String>,
        cli_version: Option<String>,
        tools: Vec<String>,
        mcp_connected: Option<bool>,
    },
    Entry(TranscriptEntry),
    Context(u64),
    Compacted {
        trigger: String,
        pre_tokens: Option<u64>,
    },
    Quota(QuotaReading),
    TurnDone(TurnOutcome),
}

/// Pure line-by-line parser for one CLI's stream.
pub trait StreamParser: Send {
    fn feed(&mut self, line: &str) -> Vec<StreamEvent>;

    /// Runtime decoding must admit derived storage against the original read owner.
    /// A fixture-only parser does not acquire runtime permission from its pure feed method.
    fn feed_owned(
        &mut self,
        _line: &str,
        _operation: &crate::operation::Operation,
    ) -> Result<Vec<StreamEvent>, DriverError> {
        Err(DriverError::Protocol(
            "parser lacks owned decode admission".into(),
        ))
    }
}

/// Private launcher provenance. Displaying a driver error never exposes these fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutKind {
    TurnReservation,
    TranscriptConfirmation,
    LegacyUnclassified,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    Unknown,
    Observe,
    DescribeActions,
    Decode,
    Choose,
    Validate,
    Submit,
    FinalCapture,
    TranscriptConfirmation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitOrigin {
    ConfiguredTurnReservation,
    FixedPresetFlush,
    UnclassifiedLegacy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseMeasurement {
    Available,
    Unavailable,
    RejectedStale,
}
/// Monotonic client operation boundary, never an engine/provider timing claim.
/// Drivers reset turn_started_at at actual turn entry and phase_started_at at each boundary.
#[derive(Clone, Copy, Debug)]
pub struct TurnProgress {
    pub turn_started_at: Instant,
    pub phase_started_at: Instant,
    pub phase: TurnPhase,
}
pub fn measured_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeoutDiagnostic {
    pub kind: TimeoutKind,
    pub observed_limit_ms: Option<u64>,
    pub limit_origin: LimitOrigin,
    pub observed_elapsed_ms: Option<u64>,
    pub phase: TurnPhase,
    pub phase_elapsed_ms: Option<u64>,
    pub phase_measurement: PhaseMeasurement,
}
impl TimeoutDiagnostic {
    pub fn legacy() -> Self {
        Self {
            kind: TimeoutKind::LegacyUnclassified,
            observed_limit_ms: None,
            limit_origin: LimitOrigin::UnclassifiedLegacy,
            observed_elapsed_ms: None,
            phase: TurnPhase::Unknown,
            phase_elapsed_ms: None,
            phase_measurement: PhaseMeasurement::Unavailable,
        }
    }
    /// Called only when the outer timer wins, after the cancelled future has been dropped.
    pub fn turn_reservation(
        start: Instant,
        reserved_ms: u64,
        progress: Option<TurnProgress>,
    ) -> Self {
        let now = Instant::now();
        let mut result = Self::legacy();
        result.kind = TimeoutKind::TurnReservation;
        result.observed_limit_ms = Some(reserved_ms);
        result.limit_origin = LimitOrigin::ConfiguredTurnReservation;
        result.observed_elapsed_ms = Some(measured_millis(now.duration_since(start)));
        if let Some(p) = progress {
            if p.turn_started_at >= start
                && p.turn_started_at <= p.phase_started_at
                && p.phase_started_at <= now
            {
                result.phase = p.phase;
                result.phase_elapsed_ms =
                    Some(measured_millis(now.duration_since(p.phase_started_at)));
                result.phase_measurement = PhaseMeasurement::Available;
            } else {
                result.phase_measurement = PhaseMeasurement::RejectedStale;
            }
        }
        result
    }
    /// Inert preset's own flush timer; not a classification of a native CLI timeout.
    pub fn transcript_confirmation(wait_start: Instant, limit: Duration) -> Self {
        let elapsed = measured_millis(wait_start.elapsed());
        Self {
            kind: TimeoutKind::TranscriptConfirmation,
            observed_limit_ms: Some(measured_millis(limit)),
            limit_origin: LimitOrigin::FixedPresetFlush,
            observed_elapsed_ms: Some(elapsed),
            phase: TurnPhase::TranscriptConfirmation,
            phase_elapsed_ms: Some(elapsed),
            phase_measurement: PhaseMeasurement::Available,
        }
    }
}

/// What can go wrong driving a CLI.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("could not start {cli}: {source}")]
    Spawn {
        cli: &'static str,
        source: std::io::Error,
    },
    #[error("the CLI process ended unexpectedly: {0}")]
    Died(String),
    #[error("the turn exceeded its time limit")]
    Timeout,
    #[error("the turn exceeded its time limit")]
    TimeoutAt(TimeoutDiagnostic),
    #[error("the CLI refused the turn: {0}")]
    Cli(String),
    #[error("isolation check failed: {0}")]
    Isolation(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("{0}")]
    Host(crate::host::HostRefusal),
}

impl DriverError {
    pub fn is_timeout(&self) -> bool {
        matches!(
            self,
            Self::Timeout
                | Self::TimeoutAt(_)
                | Self::Host(
                    crate::host::HostRefusal::Expired | crate::host::HostRefusal::CaptureOverflow
                )
        )
    }
    pub fn timeout_diagnostic(&self) -> Option<TimeoutDiagnostic> {
        match self {
            Self::Timeout => Some(TimeoutDiagnostic::legacy()),
            Self::TimeoutAt(diagnostic) => Some(diagnostic.clone()),
            _ => None,
        }
    }
}

/// Information about a started session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub model: Option<String>,
    pub cli_version: Option<String>,
    /// `true` when an existing session was resumed rather than started fresh.
    pub resumed: bool,
}

/// Driver-reported context occupancy and compaction events, never inferred from
/// aggregate turn token usage. Counts restart at zero for each process.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionTelemetry {
    pub model: Option<String>,
    pub cli_version: Option<String>,
    pub tools: Vec<String>,
    pub context_tokens: Option<u64>,
    pub compactions: u64,
}
/// Verified upper bound on one request's client-side estimate, never a billing claim.
#[derive(Clone, Debug)]
pub struct EstimateBound {
    pub pinned_model: String,
    pub context_tokens: u64,
    pub output_tokens: u64,
    pub max_input_or_cache_write_usd_per_token: f64,
    pub output_usd_per_token: f64,
    pub evidence: String,
}
impl EstimateBound {
    #[allow(clippy::float_arithmetic)]
    pub fn ceiling_usd(&self) -> Result<f64, DriverError> {
        if self.pinned_model.is_empty()
            || self.evidence.is_empty()
            || self.context_tokens == 0
            || self.output_tokens == 0
            || !self.max_input_or_cache_write_usd_per_token.is_finite()
            || self.max_input_or_cache_write_usd_per_token <= 0.
            || !self.output_usd_per_token.is_finite()
            || self.output_usd_per_token <= 0.
        {
            return Err(DriverError::Isolation("unproved estimate bound".into()));
        }
        let ceiling = self.context_tokens as f64 * self.max_input_or_cache_write_usd_per_token
            + self.output_tokens as f64 * self.output_usd_per_token;
        if !ceiling.is_finite() || ceiling <= 0. {
            return Err(DriverError::Isolation("invalid estimate ceiling".into()));
        }
        Ok(ceiling)
    }
}
/// Operator-only observation from a retained-result entry. Typed callback bodies
/// stay in their original root/destination, never in this future's output.
/// `true` means only a completed body MOVE; neither value proves success, joins,
/// custody or settlement. Construction requires full ORIGINAL admission.
pub type RetainedDriverFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, DriverError>> + Send + 'a>>;

/// One seat's CLI session.
#[async_trait]
pub trait SeatDriver: Send + Sync {
    fn kind(&self) -> CliKind;
    /// Operator-only retained startup; this is SYNCHRONOUS before erasure.
    /// Implementations must check trusted SAME original ownership, a typed EMPTY
    /// creation-admitted destination, and FULL concrete/outer/erased/growing fit
    /// before boxing any future. The factory must already own the SAME concrete
    /// Box/root before context binding. Callback return lands FIRST in that root,
    /// then MOVEs to this destination before the fixed observation completes.
    /// Never format/clone DriverError bodies, renew allowance/deadline, redispatch
    /// after lost ACK, or infer settlement. Attempts/results remain sticky after
    /// cancel/unpolled drop/unwind/refused retry. Unsupported default is CLOSED:
    /// fixed refusal, no allocation, destination mutation or legacy start call.
    fn start_retained<'a>(
        &'a mut self,
        _original: &'a crate::operation::Operation,
        _resume_session: Option<&'a str>,
        _destination_original: &'a crate::operation::Operation,
        _destination: &'a mut Option<Result<SessionInfo, DriverError>>,
    ) -> Result<RetainedDriverFuture<'a>, DriverError> {
        Err(DriverError::Host(crate::host::HostRefusal::Unknown))
    }

    /// Operator-only retained turn; SYNCHRONOUS original/ownership/typed EMPTY
    /// destination and FULL creation-admitted concrete/outer/erased/growing fit
    /// checks precede every future box. SAME factory Box/root association must
    /// exist before binding. Retain the first callback Result in that root, then
    /// MOVE it here before fixed observation; never format/clone DriverError or
    /// replace an earlier timeout/error with captured evidence. Keep attempts and
    /// bodies through cancel/unpolled drop/unwind/retry, with no lost-ACK dispatch,
    /// new Operation, renewal or healthy inference. Unsupported default refuses
    /// CLOSED without allocation, mutation or fallback to legacy run_turn.
    fn turn_retained<'a>(
        &'a mut self,
        _original: &'a crate::operation::Operation,
        _prompt: &'a str,
        _limit: Duration,
        _destination_original: &'a crate::operation::Operation,
        _destination: &'a mut Option<Result<TurnOutcome, DriverError>>,
    ) -> Result<RetainedDriverFuture<'a>, DriverError> {
        Err(DriverError::Host(crate::host::HostRefusal::Unknown))
    }

    /// Operator-only cleanup disposition, distinct from EVERY original child/
    /// reader join, archive verification/custody, usage completion, prompt ACK,
    /// journal CAS and healthy settlement. SYNCHRONOUS SAME-original ownership,
    /// typed EMPTY creation-admitted destination and FULL concrete/outer/erased/
    /// growing fit checks precede boxing. SAME factory Box/root must already be
    /// installed before binding; store callback Result FIRST there, then MOVE
    /// here before fixed observation. Never clone/format DriverError, renew,
    /// retry lost ACK or overwrite prior start/turn causes. Retain attempt/body
    /// on cancel/unpolled drop/unwind/refusal. Unsupported default is CLOSED
    /// with fixed refusal/no allocation/mutation/legacy stop fallback.
    fn cleanup_retained<'a>(
        &'a mut self,
        _original: &'a crate::operation::Operation,
        _destination_original: &'a crate::operation::Operation,
        _destination: &'a mut Option<Result<(), DriverError>>,
    ) -> Result<RetainedDriverFuture<'a>, DriverError> {
        Err(DriverError::Host(crate::host::HostRefusal::Unknown))
    }

    /// Operator-only synchronous captured-body MOVE to a typed EMPTY ORIGINAL
    /// creation-admitted slot. SAME trusted original/ownership/factory Box/root
    /// and FULL storage/frame/growing fit checks precede transfer; callback body
    /// must already be canonical. No clone/format/future erasure or by-value
    /// legacy take_captured_outcome fallback. Absence is not successful startup,
    /// cleanup, joins/custody/usage completion/prompt ACK/journal CAS/settlement.
    /// Preserve original start/turn/cleanup errors, sticky attempts and bodies on
    /// refusal/unwind/retry; no new Operation/allowance/deadline/lost-ACK dispatch.
    /// Unsupported default refuses CLOSED, touching neither owner nor slot.
    fn move_captured_retained(
        &mut self,
        _original: &crate::operation::Operation,
        _destination_original: &crate::operation::Operation,
        _destination: &mut Option<TurnOutcome>,
    ) -> Result<bool, DriverError> {
        Err(DriverError::Host(crate::host::HostRefusal::Unknown))
    }

    /// Move a separately captured terminal result to its ORIGINAL foreground owner.
    /// Absence is not a start/cleanup witness. This releases no child, reader, prompt or credit,
    /// and must never replace the caller's original error/timeout with a successful result.
    fn take_captured_outcome(
        &mut self,
        _original: &crate::operation::Operation,
    ) -> Result<Option<TurnOutcome>, DriverError> {
        Ok(None)
    }
    /// An explicit trusted adapter is required. This default is CLOSED, never a fresh-harness
    /// fallback for a server-backed seat. Inert drivers opt in with their own required adapter.
    fn set_host_context(
        &mut self,
        _context: crate::host::HostDriverContext,
    ) -> Result<(), DriverError> {
        Err(DriverError::Host(crate::host::HostRefusal::Unknown))
    }
    fn start_evidence(&self) -> Option<crate::operation::StartEvidence> {
        None
    }
    /// Optional current-turn client boundary; native drivers stay opaque by default.
    fn turn_progress(&self) -> Option<TurnProgress> {
        None
    }
    fn telemetry(&self) -> SessionTelemetry {
        SessionTelemetry::default()
    }
    /// None means real admission must fail closed; fake drivers may inject explicit bounds.
    fn estimate_bound(&self) -> Option<EstimateBound> {
        None
    }
    /// Apply only this seat's remaining provider-reported cost allocation before spawn/resume.
    /// Drivers without a verified provider enforcement mechanism fail closed.
    fn set_reported_cost_limit(&mut self, _remaining_usd: f64) -> Result<(), DriverError> {
        Err(DriverError::Isolation(
            "driver cannot enforce reported-cost limit".into(),
        ))
    }
    /// Token-budget fallback requires native enforcement, not a guessed USD conversion.
    fn set_token_limit(&mut self, _remaining: u64) -> Result<(), DriverError> {
        Err(DriverError::Isolation(
            "driver cannot enforce token limit".into(),
        ))
    }
    /// Start (or resume, if `resume_session` is given) the CLI session.
    async fn start(&mut self, resume_session: Option<&str>) -> Result<SessionInfo, DriverError>;
    /// Send one user turn and stream the CLI's work into the transcript until the turn ends.
    async fn run_turn(&mut self, prompt: &str, limit: Duration)
    -> Result<TurnOutcome, DriverError>;
    /// The CLI's session id, once known. Persist it to resume after a crash.
    fn session_id(&self) -> Option<String>;
    /// Is the CLI process still alive?
    fn is_alive(&mut self) -> bool;
    /// Return Ok only after process termination is confirmed; errors retain uncertain cleanup.
    async fn stop(&mut self) -> Result<(), crate::driver::DriverError>;
}

// ---------------------------------------------------------------------------------------------
// Child process plumbing

/// A child process with line-oriented stdout, writable stdin and a stderr tail for diagnostics.
pub struct ChildProc {
    physical_child_id: Arc<str>,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: frame_queue::Receiver,
    pending_frame: Option<ChildFrame>,
    pending_events: Option<RetainedEvents>,
    stdout_assembly: Arc<Mutex<FrameAssembly>>,
    stdout_pending: Arc<Mutex<Option<ChildFrame>>>,
    stdout_chunk: Arc<Mutex<Option<StdoutChunk>>>,
    turn_quota: Vec<QuotaReading>,
    helper_output: String,
    stderr_tail: Arc<Mutex<StderrCapture>>,
    stderr_chunk: Arc<Mutex<Option<StdoutChunk>>>,
    readers: Vec<tokio::task::JoinHandle<std::io::Result<()>>>,
    // Original stdout/stderr indices never change while either handle remains.
    // An awaited non-tail handle stays here, marked against a second poll.
    reader_joined: [bool; 2],
    reader_error: Arc<Mutex<reader_failure::ReaderFailures>>,
    capture: Arc<Mutex<crate::operation::Operation>>,
    stderr_stats: Arc<Mutex<StderrStats>>,
    last_origin: Option<crate::operation::CaptureOrigin>,
    cleanup_deadline: Option<Instant>,
    // Private physical fact only. Evidence, prompt ACK and operation liability
    // remain independently owned; this never authorizes replacement or refund.
    physical_joined: bool,
    // Only the canonical long-lived session participates; helpers keep their
    // original independent join/read/error path and cannot consume this gate.
    handoff: Option<Arc<reader_handoff::Gate>>,
    handoff_request: Option<reader_handoff::Binding>,
}

#[derive(Serialize)]
struct ChildFrame {
    text: String,
    origin: Option<crate::operation::CaptureOrigin>,
    #[serde(skip)]
    operation: crate::operation::Operation,
}

/// Prepare the transient returned body and retained origin copy under the SAME raw
/// frame's operation. Refusal leaves both canonical frame and prior origin untouched.
fn copy_stdout_frame_view(
    frame: &ChildFrame,
    last_origin: &mut Option<crate::operation::CaptureOrigin>,
) -> Result<String, crate::operation::OperationError> {
    use crate::operation::{CaptureOrigin, Operation, OperationError};
    fn copy_admitted(text: &str, operation: &Operation) -> Result<String, OperationError> {
        operation.check()?;
        let mut value = String::new();
        value
            .try_reserve_exact(text.len())
            .map_err(|_| operation.overflow())?;
        // Reject excess reported capacity before accepting copied bytes. This does
        // not establish allocator internals or a bound on transient native storage.
        if value.capacity() > text.len() {
            return Err(operation.overflow());
        }
        operation.check()?;
        value.push_str(text); // whole admitted string fits without backing growth
        Ok(value)
    }
    let operation = &frame.operation;
    operation.check()?;
    let mut bytes = frame.text.len();
    if let Some(origin) = &frame.origin {
        bytes = bytes
            .checked_add(origin.operation_id.len())
            .and_then(|n| n.checked_add(origin.physical_child_id.as_ref().map_or(0, String::len)))
            .ok_or_else(|| operation.overflow())?;
    }
    operation.charge(
        bytes
            .checked_add(std::mem::size_of::<(String, Option<CaptureOrigin>)>())
            .ok_or_else(|| operation.overflow())?,
    )?;
    let text = copy_admitted(&frame.text, operation)?;
    let origin = frame
        .origin
        .as_ref()
        .map(|origin| {
            Ok::<_, OperationError>(CaptureOrigin {
                operation_id: copy_admitted(&origin.operation_id, operation)?,
                child_generation: origin.child_generation,
                ordinal: origin.ordinal,
                physical_child_id: origin
                    .physical_child_id
                    .as_deref()
                    .map(|id| copy_admitted(id, operation))
                    .transpose()?,
            })
        })
        .transpose()?;
    operation.check()?;
    *last_origin = origin; // no fallible step after this original-metadata commit
    Ok(text)
}

fn queued_stdout_record<'a>(
    frame: &'a ChildFrame,
    physical_child: &'a str,
    index: usize,
) -> Result<crate::failed_evidence::EvidenceRecord<'a>, crate::failed_evidence::ArchiveError> {
    Ok(crate::failed_evidence::EvidenceRecord {
        operation: &frame.operation,
        category: crate::failed_evidence::EvidenceCategory::Stdout,
        part: "queued_frame",
        index: u64::try_from(index).map_err(|_| crate::failed_evidence::ArchiveError::Fit)?,
        physical_child: Some(physical_child),
        origin: frame.origin.as_ref(),
    })
}

/// The child retains the whole accepted frame and its unaccepted derived suffix. Parsing is
/// once-only; successful synchronous callbacks pop exactly one event. An error/canceled wait
/// cannot replay a previously accepted prefix or discard the in-service raw frame.
struct RetainedEvents {
    raw: ChildFrame,
    events: VecDeque<StreamEvent>,
}

fn decode_pending_frame(
    pending: &mut Option<ChildFrame>,
    parser: &mut dyn StreamParser,
) -> Result<RetainedEvents, DriverError> {
    let raw = pending
        .as_ref()
        .ok_or_else(|| DriverError::Protocol("no retained parser frame".into()))?;
    let events = parser.feed_owned(&raw.text, &raw.operation)?;
    Ok(RetainedEvents {
        raw: pending.take().expect("original decoded frame"),
        events: events.into(),
    })
}

fn admit_quota_destination(
    batch: &RetainedEvents,
    quota: &mut Vec<QuotaReading>,
) -> Result<(), DriverError> {
    if matches!(batch.events.front(), Some(StreamEvent::Quota(_))) {
        // Recheck even when the destination has spare capacity. A prior accepted
        // event can close this SAME operation without ending the current batch.
        batch
            .raw
            .operation
            .check()
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        event_storage::EventStorage(Some(&batch.raw.operation))
            .reserve_next(quota)
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
    }
    Ok(())
}

/// A completed positive write is possible-start evidence. Cancellation of a pending write
/// also retains possible liability because transfer may have occurred without its return.
/// A verified zero return disarms this guard; an error has no reliable transfer count and
/// retains possible liability. It never downgrades earlier evidence or proves provider spend.
struct PendingDispatch {
    operation: crate::operation::Operation,
    armed: bool,
}
impl Drop for PendingDispatch {
    fn drop(&mut self) {
        if self.armed {
            self.operation.possible_start();
        }
    }
}

impl RetainedEvents {
    fn check_operation(&self, expected: &crate::operation::Operation) -> Result<(), DriverError> {
        if self.raw.operation.same_instance(expected) {
            return Ok(());
        }
        Err(DriverError::Host(crate::host::HostRefusal::Ownership(
            "retained frame belongs to another original operation".into(),
        )))
    }

    fn accept_front(
        &mut self,
        accept: &mut impl FnMut(
            &StreamEvent,
            Option<&crate::operation::CaptureOrigin>,
        ) -> Result<(), DriverError>,
    ) -> Result<Option<StreamEvent>, DriverError> {
        let Some(event) = self.events.front() else {
            return Ok(None);
        };
        self.raw
            .operation
            .check()
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        accept(event, self.raw.origin.as_ref())?;
        // Callback success commits this one acceptance, even if it closed admission.
        // Do not check after success and replay it; the next front is checked afresh.
        Ok(self.events.pop_front())
    }
}

fn event_bytes(event: &StreamEvent) -> Result<usize, crate::operation::OperationError> {
    use crate::operation::{OPERATION_BYTES, encoded_len};
    match event {
        StreamEvent::ModelStart { session_id, frame } => {
            encoded_len(&(session_id, frame), OPERATION_BYTES)
        }
        StreamEvent::Init {
            session_id,
            model,
            cli_version,
            tools,
            mcp_connected,
        } => encoded_len(
            &(session_id, model, cli_version, tools, mcp_connected),
            OPERATION_BYTES,
        ),
        StreamEvent::Entry(entry) => encoded_len(entry, OPERATION_BYTES),
        StreamEvent::Context(value) => encoded_len(value, OPERATION_BYTES),
        StreamEvent::Compacted {
            trigger,
            pre_tokens,
        } => encoded_len(&(trigger, pre_tokens), OPERATION_BYTES),
        StreamEvent::Quota(value) => encoded_len(value, OPERATION_BYTES),
        StreamEvent::TurnDone(value) => encoded_len(value, OPERATION_BYTES),
    }
}

fn enqueue_stdout_frame(
    tx: &frame_queue::Sender,
    retained: &Mutex<Option<ChildFrame>>,
    frame: ChildFrame,
) -> std::io::Result<()> {
    let mut pending = retained.lock().expect("in-service stdout frame");
    if pending.is_some() {
        return Err(std::io::Error::other("stdout frame still owned"));
    }
    *pending = Some(frame); // retain BEFORE sizing/charge/enqueue can refuse
    let frame = pending.as_ref().expect("retained stdout frame");
    let operation = &frame.operation;
    let bytes = crate::operation::encoded_len(frame, crate::operation::OPERATION_BYTES)
        .map_err(|_| reader_failure::operation_error(operation.overflow()))?;
    operation
        .charge(
            bytes
                .checked_add(std::mem::size_of::<ChildFrame>())
                .ok_or_else(|| reader_failure::operation_error(operation.overflow()))?,
        )
        .map_err(reader_failure::operation_error)?;
    // Queue closure, capacity admission and growth all precede the only move.
    // A refusal keeps this SAME pending allocation, with no dequeue/requeue copy.
    tx.send(&mut pending)
        .map_err(reader_failure::operation_error)
}

/// Diagnostic truncation is explicit and applies only to the non-answer stderr stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StderrStats {
    pub seen_bytes: u64,
    pub retained_bytes: usize,
    pub discarded_bytes: u64,
    pub truncated: bool,
}

struct StderrLine {
    bytes: Vec<u8>,
    operation: crate::operation::Operation,
}

/// mdcc865c89393 owned-only precreation backing. This is NOT the legacy
/// StderrCapture ring and is not yet connected to physical reader mutation.
/// Payload Vec capacities remain separately admitted through ORIGINAL G.
#[allow(dead_code)]
struct PendingStderrLines {
    rows: VecDeque<StderrLine>,
    capacity_pin: usize,
    used: bool,
    original: Option<crate::operation::Operation>,
    first_refusal: Option<crate::operation::OperationError>,
}

#[allow(dead_code)]
impl PendingStderrLines {
    fn new() -> Result<Self, crate::failed_evidence::ArchiveError> {
        let backing = Vec::with_capacity(crate::operation::STDERR_LINES);
        let capacity_pin = backing.capacity();
        let rows = VecDeque::from(backing);
        if capacity_pin < crate::operation::STDERR_LINES || rows.capacity() != capacity_pin {
            return Err(crate::failed_evidence::ArchiveError::Identity);
        }
        Ok(Self {
            rows,
            capacity_pin,
            used: false,
            original: None,
            first_refusal: None,
        })
    }

    fn creation_layout(&self) -> Result<std::alloc::Layout, crate::failed_evidence::ArchiveError> {
        if self.used
            || self.original.is_some()
            || self.first_refusal.is_some()
            || !self.rows.is_empty()
            || self.rows.capacity() != self.capacity_pin
            || self.capacity_pin < crate::operation::STDERR_LINES
        {
            return Err(crate::failed_evidence::ArchiveError::Identity);
        }
        std::alloc::Layout::array::<StderrLine>(self.capacity_pin)
            .map_err(|_| crate::failed_evidence::ArchiveError::Fit)
    }

    fn bind_original(
        &mut self,
        original: &crate::operation::Operation,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.creation_layout()?;
        self.used = true;
        self.original = Some(original.clone()); // SAME handles retained BEFORE postcheck.
        original.check()?;
        Ok(())
    }

    fn refuse(
        &mut self,
        error: crate::operation::OperationError,
    ) -> crate::operation::OperationError {
        *self.first_refusal.get_or_insert(error) // Fixed Copy class, no callbacks.
    }

    /// All mutation is under unique owner exclusion, BEFORE the only push.
    /// Refusal leaves the ENTIRE already-admitted line in its caller's slot.
    fn push_from_slot(
        &mut self,
        candidate: &mut Option<StderrLine>,
    ) -> Result<(), crate::operation::OperationError> {
        self.check_line_insertion(candidate.as_ref().map(|line| &line.operation))?;
        self.rows
            .push_back(candidate.take().expect("checked original stderr line"));
        Ok(()) // No reserve, replacement, pop, clear, refund or notification.
    }

    /// Future reader assembly can move its SAME admitted partial Vec directly
    /// into the pinned ring. Every refusal precedes BOTH takes, retaining the
    /// raw partial body/capacity and its original association in their slots.
    /// No separate StderrLine candidate or new backing is allocated here.
    fn finish_partial_from_slots(
        &mut self,
        partial: &mut Vec<u8>,
        partial_original: &mut Option<crate::operation::Operation>,
    ) -> Result<(), crate::operation::OperationError> {
        if let Some(error) = self.first_refusal {
            return Err(error);
        }
        if partial.is_empty() {
            return Ok(()); // As in legacy finish_partial, no insertion or owner take.
        }
        self.check_line_insertion(partial_original.as_ref())?;
        self.rows.push_back(StderrLine {
            bytes: std::mem::take(partial), // MOVE the admitted Vec, never ring replacement.
            operation: partial_original.take().expect("checked original partial"),
        });
        Ok(()) // Byte/counter/cursor bookkeeping stays with the future reader caller.
    }

    /// Shared insertion gate, under the SAME unique owner exclusion as push.
    /// No mutation between this successful guard and its corresponding push
    /// may grow/change the ring or change the body owner's original identity.
    fn check_line_insertion(
        &mut self,
        candidate_original: Option<&crate::operation::Operation>,
    ) -> Result<(), crate::operation::OperationError> {
        use crate::operation::OperationError;
        if let Some(error) = self.first_refusal {
            return Err(error);
        }
        if self.rows.capacity() != self.capacity_pin
            || self.rows.len().checked_add(1).is_none_or(|length| {
                length > self.capacity_pin || length > crate::operation::STDERR_LINES
            })
        {
            return Err(self.refuse(OperationError::CaptureOverflow));
        }
        let Some(original) = self.original.as_ref() else {
            return Err(self.refuse(OperationError::CreditOwned));
        };
        if !candidate_original.is_some_and(|candidate| candidate.same_instance(original)) {
            return Err(self.refuse(OperationError::CreditOwned));
        }
        if let Err(error) = original.check() {
            return Err(self.refuse(error));
        }
        Ok(())
    }

    /// Future reader integration MUST retain raw cause, release capture mutex,
    /// THEN notify this SAME original. A local latch alone is NOT global stop.
    fn overflow_owner(&self) -> Option<crate::operation::Operation> {
        if matches!(
            self.first_refusal,
            Some(crate::operation::OperationError::CaptureOverflow)
        ) {
            self.original.clone()
        } else {
            None
        }
    }
}

// UNCOMPILED/UNRUN source controls. Legacy Operation fixtures below do NOT
// certify complete original F; they isolate pin/identity/custody mechanics.
#[cfg(test)]
mod stderr_lines_source_controls {
    use super::*;
    use crate::operation::{Allowance, Operation, OperationError, STDERR_LINES};

    fn original() -> Operation {
        Operation::new(
            "stderr-ring-source".into(),
            7,
            Allowance::required(Some(Instant::now() + Duration::from_secs(1))).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn precreation_layout_counts_all_actual_spare_without_an_operation() {
        let ordinary = PendingStderrLines::new().unwrap();
        assert!(ordinary.original.is_none());
        assert_eq!(ordinary.rows.capacity(), ordinary.capacity_pin);
        assert_eq!(
            ordinary.creation_layout().unwrap(),
            std::alloc::Layout::array::<StderrLine>(ordinary.rows.capacity()).unwrap()
        );
        // Synthetic allocator-excess fixture: a NEW object with the pin set
        // once, not an existing owner's pin rewritten to repair/refund it.
        let backing = Vec::with_capacity(STDERR_LINES + 7);
        let pin = backing.capacity();
        let spare = PendingStderrLines {
            rows: VecDeque::from(backing),
            capacity_pin: pin,
            used: false,
            original: None,
            first_refusal: None,
        };
        assert!(pin > STDERR_LINES);
        assert_eq!(
            spare.creation_layout().unwrap().size(),
            pin * std::mem::size_of::<StderrLine>()
        );
    }

    #[test]
    fn forty_lines_then_refusal_keeps_entire_input_capacity_and_original() {
        let mut pending = PendingStderrLines::new().unwrap(); // BEFORE fixture Op.
        let pin = pending.capacity_pin;
        let operation = original();
        pending.bind_original(&operation).unwrap();
        let before = operation.captured_bytes();
        for byte in 0..STDERR_LINES {
            let mut line = Some(StderrLine {
                bytes: vec![byte as u8],
                operation: operation.clone(),
            });
            pending.push_from_slot(&mut line).unwrap();
            assert!(line.is_none());
            assert_eq!(pending.rows.capacity(), pin);
        }
        let mut extra = Some(StderrLine {
            bytes: vec![255],
            operation: operation.clone(),
        });
        let body = extra.as_ref().unwrap().bytes.as_ptr();
        assert!(matches!(
            pending.push_from_slot(&mut extra),
            Err(OperationError::CaptureOverflow)
        ));
        assert!(std::ptr::eq(extra.as_ref().unwrap().bytes.as_ptr(), body));
        assert!(extra.as_ref().unwrap().operation.same_instance(&operation));
        assert_eq!(pending.rows.len(), STDERR_LINES);
        assert_eq!(pending.rows.capacity(), pin);
        assert_eq!(operation.captured_bytes(), before); // Move guard never charges/refunds.
        assert!(pending.creation_layout().is_err()); // Used cannot become pristine.
        assert!(pending.bind_original(&operation).is_err());
        assert_eq!(
            pending
                .rows
                .iter()
                .map(|row| row.bytes[0])
                .collect::<Vec<_>>(),
            (0..STDERR_LINES)
                .map(|value| value as u8)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn pin_failure_is_sticky_and_same_original_notification_can_follow_unlock() {
        let pending = Mutex::new(PendingStderrLines::new().unwrap());
        let operation = original();
        let mut line = Some(StderrLine {
            bytes: vec![7],
            operation: operation.clone(),
        });
        let signal = {
            let mut guard = pending.lock().unwrap();
            guard.bind_original(&operation).unwrap();
            guard.capacity_pin += 1; // TEST-ONLY corruption, never a production setter.
            assert!(matches!(
                guard.push_from_slot(&mut line),
                Err(OperationError::CaptureOverflow)
            ));
            assert!(line.is_some());
            assert!(guard.rows.is_empty());
            assert!(operation.check().is_ok()); // No notification under this mutex.
            guard.capacity_pin -= 1; // Repairing test metadata must NOT renew a failed owner.
            assert!(matches!(
                guard.push_from_slot(&mut line),
                Err(OperationError::CaptureOverflow)
            ));
            guard.overflow_owner().unwrap()
        };
        assert!(signal.same_instance(&operation));
        signal.overflow(); // ONLY after capture exclusion is released.
        assert!(matches!(
            operation.check(),
            Err(OperationError::CaptureOverflow)
        ));
        assert!(line.is_some());
    }

    #[test]
    fn same_text_foreign_operation_cannot_replace_owner_or_consume_line() {
        let mut pending = PendingStderrLines::new().unwrap();
        let operation = original();
        let foreign = original();
        assert_eq!(operation.id(), foreign.id());
        assert!(!operation.same_instance(&foreign));
        pending.bind_original(&operation).unwrap();
        assert!(pending.bind_original(&foreign).is_err());
        let mut line = Some(StderrLine {
            bytes: vec![9],
            operation: foreign.clone(),
        });
        assert!(matches!(
            pending.push_from_slot(&mut line),
            Err(OperationError::CreditOwned)
        ));
        assert!(line.is_some());
        assert!(pending.rows.is_empty());
        assert!(pending.original.as_ref().unwrap().same_instance(&operation));
        assert!(foreign.check().is_ok());
    }

    #[test]
    fn partial_publication_moves_same_backing_and_owner_without_charge_or_refund() {
        let mut pending = PendingStderrLines::new().unwrap();
        let operation = original();
        pending.bind_original(&operation).unwrap();
        let pin = pending.capacity_pin;
        // Synthetic admitted-body stand-in; this does NOT prove production G.
        let mut partial = Vec::with_capacity(19);
        partial.extend_from_slice(&[0xff, b'x', b'\n']);
        let pointer = partial.as_ptr();
        let capacity = partial.capacity();
        let mut owner = Some(operation.clone());
        let before = operation.captured_bytes();
        pending
            .finish_partial_from_slots(&mut partial, &mut owner)
            .unwrap();
        assert!(partial.is_empty());
        assert_eq!(partial.capacity(), 0);
        assert!(owner.is_none());
        assert_eq!(pending.rows.len(), 1);
        assert_eq!(pending.rows[0].bytes, [0xff, b'x', b'\n']);
        assert_eq!(pending.rows[0].bytes.as_ptr(), pointer);
        assert_eq!(pending.rows[0].bytes.capacity(), capacity);
        assert!(pending.rows[0].operation.same_instance(&operation));
        assert_eq!(pending.rows.capacity(), pin);
        assert_eq!(operation.captured_bytes(), before);
        // Empty publication does not create a row or consume an association.
        owner = Some(operation.clone());
        pending
            .finish_partial_from_slots(&mut partial, &mut owner)
            .unwrap();
        assert!(owner.as_ref().unwrap().same_instance(&operation));
        assert_eq!(pending.rows.len(), 1);
        assert_eq!(operation.captured_bytes(), before);
    }

    #[test]
    fn refused_partial_publication_keeps_both_slots_and_first_cause() {
        // Independent fixtures avoid repairing/reusing any failed owner.
        for case in 0..5 {
            let mut pending = PendingStderrLines::new().unwrap();
            let operation = original();
            let foreign = original();
            pending.bind_original(&operation).unwrap();
            let mut partial = Vec::with_capacity(17);
            partial.extend_from_slice(&[0xfe, b'\n']);
            let pointer = partial.as_ptr();
            let capacity = partial.capacity();
            let mut owner = match case {
                0 => None,
                1 => Some(foreign.clone()),
                _ => Some(operation.clone()),
            };
            match case {
                2 => pending.capacity_pin += 1, // TEST-ONLY corruption.
                3 => {
                    for _ in 0..STDERR_LINES {
                        let mut line = Some(StderrLine {
                            bytes: vec![b'x'],
                            operation: operation.clone(),
                        });
                        pending.push_from_slot(&mut line).unwrap();
                    }
                }
                4 => {
                    operation.overflow();
                }
                _ => {}
            }
            let rows = pending.rows.len();
            let pin = pending.capacity_pin;
            let actual_capacity = pending.rows.capacity();
            let charged = operation.captured_bytes();
            let error = pending
                .finish_partial_from_slots(&mut partial, &mut owner)
                .unwrap_err();
            assert_eq!(
                error,
                if case < 2 {
                    OperationError::CreditOwned
                } else {
                    OperationError::CaptureOverflow
                }
            );
            assert_eq!(pending.first_refusal, Some(error));
            assert_eq!(
                pending.finish_partial_from_slots(&mut partial, &mut owner),
                Err(error)
            );
            assert_eq!(partial, [0xfe, b'\n']);
            assert_eq!(partial.as_ptr(), pointer);
            assert_eq!(partial.capacity(), capacity);
            match case {
                0 => assert!(owner.is_none()),
                1 => assert!(owner.as_ref().unwrap().same_instance(&foreign)),
                _ => assert!(owner.as_ref().unwrap().same_instance(&operation)),
            }
            assert_eq!(pending.rows.len(), rows);
            assert_eq!(pending.rows.capacity(), actual_capacity);
            assert_eq!(pending.capacity_pin, pin);
            assert_eq!(operation.captured_bytes(), charged);
            assert!(foreign.check().is_ok());
        }
    }
}

// Source controls for the private capture constructor; no physical reader is
// spawned and no complete original F/G/custody or native result is claimed.
#[cfg(test)]
mod owned_stderr_capture_source_controls {
    use super::*;
    use crate::operation::{Allowance, Operation, STDERR_LINES};

    fn operation() -> Operation {
        Operation::new(
            "owned-stderr-source".into(),
            7,
            Allowance::required(Some(Instant::now() + Duration::from_secs(1))).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn capture_transfer_preserves_whole_ring_pin_and_refuses_legacy_reset() {
        let pending = PendingStderrLines::new().unwrap(); // BEFORE fixture Op.
        let original = operation();
        let foreign = operation();
        let mut candidate = Some(pending);
        assert!(StderrCapture::from_pending_lines(&mut candidate, &original).is_err());
        candidate
            .as_mut()
            .unwrap()
            .bind_original(&original)
            .unwrap();
        let pointer = candidate.as_ref().unwrap().rows.as_slices().0.as_ptr();
        let pin = candidate.as_ref().unwrap().capacity_pin;
        assert!(StderrCapture::from_pending_lines(&mut candidate, &foreign).is_err());
        assert!(
            candidate
                .as_ref()
                .unwrap()
                .original
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        let charged = original.captured_bytes();
        let capture = StderrCapture::from_pending_lines(&mut candidate, &original).unwrap();
        assert!(candidate.is_none());
        assert!(StderrCapture::from_pending_lines(&mut candidate, &original).is_err());
        assert_eq!(capture.lines.as_slices().0.as_ptr(), pointer);
        assert_eq!(capture.lines.capacity(), pin);
        assert_eq!(capture.owned_pin.as_ref().unwrap().capacity, pin);
        assert!(
            capture
                .owned_pin
                .as_ref()
                .unwrap()
                .original
                .same_instance(&original)
        );
        assert!(capture.check_legacy_archive_release().is_err()); // Even empty: never reset.
        assert_eq!(capture.partial.capacity(), 0);
        assert_eq!(original.captured_bytes(), charged);
        let legacy = StderrCapture::new();
        assert!(legacy.owned_pin.is_none());
        assert!(legacy.check_legacy_archive_release().is_ok());
    }

    #[test]
    fn owned_capture_preserves_line_limit_and_explicit_discard_counts() {
        let mut pending = Some(PendingStderrLines::new().unwrap());
        let original = operation();
        pending.as_mut().unwrap().bind_original(&original).unwrap();
        let mut capture = StderrCapture::from_pending_lines(&mut pending, &original).unwrap();
        let pin = capture.lines.capacity();
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let mut read = StdoutRead::begin(slot.clone(), original.clone()).unwrap();
        let mut bytes = [0xff, b'\n'].repeat(STDERR_LINES);
        bytes.extend_from_slice(b"DROP\n");
        read.buffer()[..bytes.len()].copy_from_slice(&bytes);
        read.returned(bytes.len());
        let mut stats = StderrStats::default();
        capture.accept_read(&mut read, &mut stats).unwrap();
        capture.finish_partial().unwrap();
        assert_eq!(capture.lines.len(), STDERR_LINES);
        assert!(
            capture
                .lines
                .iter()
                .all(|line| line.bytes == [0xff, b'\n'] && line.operation.same_instance(&original))
        );
        assert_eq!(capture.lines.capacity(), pin);
        assert_eq!(stats.seen_bytes as usize, bytes.len());
        assert_eq!(stats.retained_bytes, STDERR_LINES * 2);
        assert_eq!(stats.discarded_bytes, 5);
        assert!(stats.truncated && capture.dropping);
        assert!(capture.partial.is_empty());
        assert!(capture.check_legacy_archive_release().is_err());
        assert!(capture.owned_pin.as_ref().unwrap().first_refusal.is_none());
        drop(read);
        assert!(!stdout_chunk_owned(&slot));
    }

    #[test]
    fn owned_pin_refusal_keeps_returned_read_uncounted_and_never_copies_on_retry() {
        let mut pending = Some(PendingStderrLines::new().unwrap());
        let original = operation();
        pending.as_mut().unwrap().bind_original(&original).unwrap();
        let mut capture = StderrCapture::from_pending_lines(&mut pending, &original).unwrap();
        capture.owned_pin.as_mut().unwrap().capacity += 1; // TEST-ONLY corruption.
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let mut read = StdoutRead::begin(slot.clone(), original.clone()).unwrap();
        read.buffer()[..3].copy_from_slice(b"x\ny");
        read.returned(3);
        let charged = original.captured_bytes();
        let mut stats = StderrStats::default();
        assert!(capture.accept_read(&mut read, &mut stats).is_err());
        assert!(capture.accept_read(&mut read, &mut stats).is_err());
        assert_eq!(stats.seen_bytes, 0);
        assert_eq!(stats.retained_bytes, 0);
        assert_eq!(stats.discarded_bytes, 0);
        assert!(!stats.truncated);
        assert!(capture.partial.is_empty() && capture.lines.is_empty());
        assert_eq!(original.captured_bytes(), charged);
        assert!(original.check().is_ok()); // Local cause; no global wake under capture exclusion.
        drop(read);
        let held = slot.lock().unwrap();
        let chunk = held.as_ref().unwrap();
        assert_eq!(chunk.cursor, 0);
        assert_eq!(chunk.count, 3);
        assert_eq!(&chunk.bytes[..3], b"x\ny");
        assert!(!chunk.seen_accounted);
        assert!(chunk.operation.as_ref().unwrap().same_instance(&original));
    }
}

/// Private immutable ownership pin carried with the SAME VecDeque allocation.
/// Some in StderrCapture denotes an owned ring, never a legacy resettable ring.
struct OwnedStderrPin {
    capacity: usize,
    original: crate::operation::Operation,
    first_refusal: Option<crate::operation::OperationError>,
}

impl OwnedStderrPin {
    fn check_read(
        &mut self,
        rows: &VecDeque<StderrLine>,
        operation: &crate::operation::Operation,
    ) -> Result<(), crate::operation::OperationError> {
        use crate::operation::OperationError;
        if let Some(error) = self.first_refusal {
            return Err(error);
        }
        let result = if rows.capacity() != self.capacity
            || self.capacity < crate::operation::STDERR_LINES
            || rows.len() > self.capacity
            || rows.len() > crate::operation::STDERR_LINES
        {
            Err(OperationError::CaptureOverflow)
        } else if !operation.same_instance(&self.original) {
            Err(OperationError::CreditOwned)
        } else {
            operation.check()
        };
        if let Err(error) = result {
            self.first_refusal = Some(error);
        }
        result // No callbacks; caller must retain error and unlock before wake.
    }

    fn check_insertion(
        &mut self,
        rows: &VecDeque<StderrLine>,
        candidate: Option<&crate::operation::Operation>,
    ) -> Result<(), crate::operation::OperationError> {
        use crate::operation::OperationError;
        if let Some(error) = self.first_refusal {
            return Err(error);
        }
        let result = if rows.capacity() != self.capacity
            || rows
                .len()
                .checked_add(1)
                .is_none_or(|len| len > self.capacity || len > crate::operation::STDERR_LINES)
        {
            Err(OperationError::CaptureOverflow)
        } else if !candidate.is_some_and(|owner| owner.same_instance(&self.original)) {
            Err(OperationError::CreditOwned)
        } else {
            self.original.check()
        };
        if let Err(error) = result {
            self.first_refusal = Some(error); // Fixed cause BEFORE returning; no wake here.
        }
        result
    }

    fn validate_archive(
        &self,
        rows: &VecDeque<StderrLine>,
        expected: &crate::operation::Operation,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        if !self.original.same_instance(expected)
            || self.capacity < crate::operation::STDERR_LINES
            || rows.capacity() != self.capacity
            || rows.len() > self.capacity
            || rows.len() > crate::operation::STDERR_LINES
            || rows
                .iter()
                .any(|row| !row.operation.same_instance(expected))
        {
            return Err(crate::failed_evidence::ArchiveError::Identity);
        }
        Ok(()) // Recovery is allowed after original overflow/deadline.
    }
}

/// Raw diagnostics stay on the SAME child on charge failure, invalid UTF-8 or reader
/// cancellation. Lossy display conversion is separate from this bounded raw ownership.
struct StderrCapture {
    lines: VecDeque<StderrLine>,
    owned_pin: Option<OwnedStderrPin>,
    partial: Vec<u8>,
    partial_operation: Option<crate::operation::Operation>,
    dropping: bool,
    // Cumulative physical-child counters are never silently rebound to another
    // operation. Fixed field storage participates in spawn's StderrCapture size.
    stats_operation: Option<crate::operation::Operation>,
}
impl StderrCapture {
    fn new() -> Self {
        Self {
            lines: VecDeque::with_capacity(crate::operation::STDERR_LINES),
            owned_pin: None,
            partial: Vec::new(),
            partial_operation: None,
            dropping: false,
            stats_operation: None,
        }
    }

    /// SOURCE-only strict construction. Preflight BEFORE taking the entire
    /// pending owner; no second ring allocation, clone, pin reset or new meter.
    /// Physical spawn remains legacy until the whole joint is approved.
    #[allow(dead_code)]
    fn from_pending_lines(
        candidate: &mut Option<PendingStderrLines>,
        expected: &crate::operation::Operation,
    ) -> Result<Self, crate::failed_evidence::ArchiveError> {
        let pending = candidate
            .as_ref()
            .ok_or(crate::failed_evidence::ArchiveError::Identity)?;
        if !pending.used
            || pending.first_refusal.is_some()
            || !pending.rows.is_empty()
            || pending.capacity_pin < crate::operation::STDERR_LINES
            || pending.rows.capacity() != pending.capacity_pin
            || !pending
                .original
                .as_ref()
                .is_some_and(|owner| owner.same_instance(expected))
        {
            return Err(crate::failed_evidence::ArchiveError::Identity);
        }
        expected.check()?;
        let PendingStderrLines {
            rows,
            capacity_pin,
            original,
            ..
        } = candidate.take().expect("preflighted whole stderr owner");
        Ok(Self {
            lines: rows, // MOVE allocation; never mem::take/replace the ring.
            owned_pin: Some(OwnedStderrPin {
                capacity: capacity_pin,
                original: original.expect("preflighted bound stderr owner"),
                first_refusal: None,
            }),
            partial: Vec::new(),
            partial_operation: None,
            dropping: false,
            stats_operation: None,
        })
    }

    fn check_legacy_archive_release(&self) -> Result<(), crate::failed_evidence::ArchiveError> {
        if self.owned_pin.is_some() {
            return Err(crate::failed_evidence::ArchiveError::Identity);
        }
        Ok(()) // Owned disposition is unfinished: never clear/reset even empty.
    }

    fn finish_partial(&mut self) -> std::io::Result<()> {
        if let Some(error) = self.owned_pin.as_ref().and_then(|pin| pin.first_refusal) {
            return Err(reader_failure::operation_error(error));
        }
        if !self.partial.is_empty() {
            if let Some(pin) = &mut self.owned_pin {
                pin.check_insertion(&self.lines, self.partial_operation.as_ref())
                    .map_err(reader_failure::operation_error)?;
            }
            self.lines.push_back(StderrLine {
                bytes: std::mem::take(&mut self.partial),
                operation: self.partial_operation.take().expect("original stderr line"),
            });
            if self.lines.len() >= crate::operation::STDERR_LINES {
                self.dropping = true;
            }
        }
        Ok(())
    }

    fn accept_read(
        &mut self,
        read: &mut StdoutRead,
        stats: &mut StderrStats,
    ) -> std::io::Result<()> {
        let chunk = read.chunk.as_mut().expect("owned stderr read");
        let operation = chunk.operation.as_ref().expect("original stderr operation");
        if let Some(pin) = &mut self.owned_pin {
            pin.check_read(&self.lines, operation)
                .map_err(reader_failure::operation_error)?;
        }
        if self
            .stats_operation
            .as_ref()
            .is_some_and(|owner| !owner.same_instance(operation))
        {
            return Err(std::io::Error::other(
                "original stderr counters remain owned",
            ));
        }
        if chunk.count > 0 && self.stats_operation.is_none() {
            if stats.seen_bytes != 0
                || stats.retained_bytes != 0
                || stats.discarded_bytes != 0
                || stats.truncated
            {
                return Err(std::io::Error::other(
                    "stderr counters lack original ownership",
                ));
            }
            self.stats_operation = Some(operation.clone());
        }
        if !chunk.seen_accounted {
            stats.seen_bytes = stats
                .seen_bytes
                .checked_add(chunk.count as u64)
                .ok_or_else(|| std::io::Error::other("stderr count overflow"))?;
            chunk.seen_accounted = true;
        }
        while chunk.cursor < chunk.count {
            let byte = chunk.bytes[chunk.cursor];
            if stats.retained_bytes < crate::operation::STDERR_BYTES && !self.dropping {
                if self
                    .partial_operation
                    .as_ref()
                    .is_some_and(|old| !old.same_instance(operation))
                {
                    return Err(std::io::Error::other(
                        "original stderr partial line remains owned",
                    ));
                }
                operation
                    .charge(1)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                // The byte guard is additional: prepay the FULL replacement
                // Vec backing before push can grow it. Old capacity charges
                // never refund; refusal leaves this read cursor/prefix intact.
                event_storage::EventStorage(Some(operation))
                    .reserve_next(&mut self.partial)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                if self.partial_operation.is_none() {
                    self.partial_operation = Some(operation.clone());
                }
                self.partial.push(byte);
                stats.retained_bytes += 1;
            } else {
                // Only this diagnostic stream permits explicit truncation. A charge error
                // above leaves bytes in the original read, never counts them as discarded.
                stats.discarded_bytes = stats
                    .discarded_bytes
                    .checked_add(1)
                    .ok_or_else(|| std::io::Error::other("stderr count overflow"))?;
                stats.truncated = true;
            }
            chunk.cursor += 1; // accepted raw prefix or explicitly counted stderr truncation
            if byte == b'\n' {
                self.finish_partial()?; // moves raw storage, no UTF-8 conversion/copy/refund
            }
        }
        Ok(())
    }

    fn display(&self) -> std::io::Result<String> {
        let mut text = String::new();
        for (bytes, operation) in self
            .lines
            .iter()
            .map(|line| (line.bytes.as_slice(), &line.operation))
            .chain(
                self.partial_operation
                    .as_ref()
                    .map(|operation| (self.partial.as_slice(), operation)),
            )
        {
            // Keep the existing conservative display guard additional to actual
            // capacity admission below. No lossy temporary or raw body copy.
            let copy_bytes = bytes
                .len()
                .checked_mul(6)
                .and_then(|n| n.checked_add(2 + 2 * std::mem::size_of::<String>()))
                .ok_or_else(|| std::io::Error::other(operation.overflow().to_string()))?;
            operation
                .charge(copy_bytes)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            // Utf8Chunks supplies the same maximal invalid subsequences as
            // from_utf8_lossy. Count without allocating; render from the SAME
            // immutable raw bytes with one replacement character per chunk.
            let line_bytes = bytes
                .utf8_chunks()
                .try_fold(0usize, |length, chunk| {
                    length.checked_add(chunk.valid().len()).and_then(|length| {
                        length.checked_add(if chunk.invalid().is_empty() {
                            0
                        } else {
                            '\u{fffd}'.len_utf8()
                        })
                    })
                })
                .ok_or_else(|| std::io::Error::other(operation.overflow().to_string()))?;
            let separator = usize::from(!text.is_empty());
            let additional = separator
                .checked_add(line_bytes)
                .ok_or_else(|| std::io::Error::other(operation.overflow().to_string()))?;
            if text
                .len()
                .checked_add(additional)
                .is_none_or(|n| n > crate::operation::STDERR_BYTES)
            {
                return Err(std::io::Error::other(operation.overflow().to_string()));
            }
            event_storage::EventStorage(Some(operation))
                .reserve_text_extension(&mut text, additional)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            if separator != 0 {
                text.push('\n');
            }
            for chunk in bytes.utf8_chunks() {
                operation
                    .check()
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
                text.push_str(chunk.valid());
                if !chunk.invalid().is_empty() {
                    text.push('\u{fffd}');
                }
            }
        }
        Ok(text)
    }

    fn has_retained_evidence(&self) -> bool {
        !self.lines.is_empty()
            || !self.partial.is_empty()
            || self.dropping
            || self.stats_operation.is_some()
    }
}

/// The raw cause has already left reader capture/stat guards on entry. First
/// MOVE it into the existing canonical slot or this retained Result; only
/// AFTER all locks drop may an owned local overflow notify its SAME original.
/// No new error body/cell, formatting, repair, spawn or credit is introduced.
fn retain_stderr_reader_failure(
    capture: &Mutex<StderrCapture>,
    failures: &Mutex<reader_failure::ReaderFailures>,
    error: std::io::Error,
) -> std::io::Error {
    let owner = match capture.lock() {
        Ok(capture) => capture.owned_pin.as_ref().map(|pin| {
            (
                pin.original.clone(), // SAME Arc handles, not cloned text/error bodies.
                matches!(
                    pin.first_refusal,
                    Some(crate::operation::OperationError::CaptureOverflow)
                ),
            )
        }),
        Err(_) => return error, // Poison is custody-only, never repaired or assumed empty.
    }; // Capture lock is gone BEFORE error retention or notification.
    let retained = match failures.lock() {
        Ok(mut failures) => match &owner {
            Some((original, _)) => failures.retain_io_for_original(
                reader_failure::ReaderStream::Stderr,
                original,
                error,
            ),
            None => failures.retain_io(reader_failure::ReaderStream::Stderr, error),
        },
        Err(_) => Err(error), // Actual refused cause stays owned by the Result.
    }; // Failure lock is gone; WHOLE error is now canonical or retained here.
    if let Some((original, true)) = owner {
        original.overflow();
    }
    match retained {
        Ok(kind) => std::io::Error::from(kind), // Canonical raw body stays in its fixed slot.
        Err(error) => error, // Existing task output keeps the original refused body.
    }
}

// DESIGN controls until the strict joint. No process/task/runtime is started;
// manual Notify polling below is not actual physical reader supervision proof.
#[cfg(test)]
mod stderr_failure_order_source_controls {
    use super::*;
    use crate::operation::{Allowance, Operation, OperationError};
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};

    fn operation() -> Operation {
        Operation::new(
            "stderr-order-source".into(),
            7,
            Allowance::required(Some(Instant::now() + Duration::from_secs(1))).unwrap(),
        )
        .unwrap()
    }

    fn local_overflow(
        original: &Operation,
        mut pending: PendingStderrLines,
    ) -> Arc<Mutex<StderrCapture>> {
        pending.bind_original(original).unwrap();
        let mut pending = Some(pending);
        let mut capture = StderrCapture::from_pending_lines(&mut pending, original).unwrap();
        let pin = capture.owned_pin.as_mut().unwrap();
        pin.capacity += 1; // TEST-ONLY mismatch drives the ACTUAL local guard.
        assert_eq!(
            pin.check_read(&capture.lines, original),
            Err(OperationError::CaptureOverflow)
        );
        assert!(original.check().is_ok());
        Arc::new(Mutex::new(capture))
    }

    #[derive(Debug)]
    struct RawCause(Arc<AtomicUsize>);
    impl std::fmt::Display for RawCause {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("synthetic raw cause")
        }
    }
    impl std::error::Error for RawCause {}
    impl Drop for RawCause {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct Probe {
        capture: Arc<Mutex<StderrCapture>>,
        failures: Arc<Mutex<reader_failure::ReaderFailures>>,
        drops: Arc<AtomicUsize>,
        observed: AtomicUsize,
    }
    impl Wake for Probe {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            let mut bits = 0;
            if self.capture.try_lock().is_ok() {
                bits |= 1;
            }
            if let Ok(failures) = self.failures.try_lock() {
                bits |= 2;
                if failures.has_io(reader_failure::ReaderStream::Stderr) {
                    bits |= 4;
                }
            }
            if self.drops.load(Ordering::SeqCst) == 0 {
                bits |= 8;
            }
            self.observed.store(bits, Ordering::SeqCst);
        }
    }

    #[test]
    fn owned_overflow_wakes_only_after_raw_cause_is_retained_and_locks_are_free() {
        let pending = PendingStderrLines::new().unwrap();
        let original = operation();
        let capture = local_overflow(&original, pending);
        let failures = reader_failure::ReaderFailures::admit_shared(&original).unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let probe = Arc::new(Probe {
            capture: capture.clone(),
            failures: failures.clone(),
            drops: drops.clone(),
            observed: AtomicUsize::new(0),
        });
        let waker = Waker::from(probe.clone());
        let mut context = Context::from_waker(&waker);
        let mut waiter = std::pin::pin!(original.wait_overflow());
        assert_eq!(waiter.as_mut().poll(&mut context), Poll::Pending);
        let raw = std::io::Error::other(RawCause(drops.clone())); // Synthetic payload, no F claim.
        let charged = original.captured_bytes();
        let returned = retain_stderr_reader_failure(&capture, &failures, raw);
        assert_eq!(probe.observed.load(Ordering::SeqCst), 15); // Both locks free, raw canonical/alive AT wake.
        assert_eq!(waiter.as_mut().poll(&mut context), Poll::Ready(()));
        assert!(returned.get_ref().is_none()); // Fixed kind return, raw body stays in existing cell.
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert_eq!(original.captured_bytes(), charged);
        assert!(matches!(
            original.check(),
            Err(OperationError::CaptureOverflow)
        ));
    }

    #[test]
    fn occupied_or_foreign_failure_slot_keeps_entire_returned_raw_body_before_wake() {
        for foreign_slot in [false, true] {
            let pending = PendingStderrLines::new().unwrap();
            let original = operation();
            let foreign = operation();
            let capture = local_overflow(&original, pending);
            let failures = reader_failure::ReaderFailures::admit_shared(if foreign_slot {
                &foreign
            } else {
                &original
            })
            .unwrap();
            if !foreign_slot {
                failures
                    .lock()
                    .unwrap()
                    .retain_io(
                        reader_failure::ReaderStream::Stderr,
                        std::io::ErrorKind::BrokenPipe.into(),
                    )
                    .unwrap();
            }
            let drops = Arc::new(AtomicUsize::new(0));
            let probe = Arc::new(Probe {
                capture: capture.clone(),
                failures: failures.clone(),
                drops: drops.clone(),
                observed: AtomicUsize::new(0),
            });
            let waker = Waker::from(probe.clone());
            let mut context = Context::from_waker(&waker);
            let mut waiter = std::pin::pin!(original.wait_overflow());
            assert_eq!(waiter.as_mut().poll(&mut context), Poll::Pending);
            let raw = std::io::Error::other(RawCause(drops.clone()));
            let pointer = std::ptr::from_ref(raw.get_ref().unwrap());
            let charged = original.captured_bytes();
            let foreign_charged = foreign.captured_bytes();
            let returned = retain_stderr_reader_failure(&capture, &failures, raw);
            assert!(std::ptr::eq(
                std::ptr::from_ref(returned.get_ref().unwrap()),
                pointer
            ));
            assert_eq!(
                probe.observed.load(Ordering::SeqCst),
                if foreign_slot { 11 } else { 15 }
            );
            assert_eq!(waiter.as_mut().poll(&mut context), Poll::Ready(()));
            assert_eq!(drops.load(Ordering::SeqCst), 0); // Whole refused body is still in returned task result.
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(foreign.captured_bytes(), foreign_charged);
            assert!(foreign.check().is_ok());
            let failures = failures.lock().unwrap();
            assert_eq!(
                failures.first_kind(),
                if foreign_slot {
                    None
                } else {
                    Some(std::io::ErrorKind::BrokenPipe)
                }
            );
            drop(failures);
            drop(returned);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn legacy_failure_retention_never_synthesizes_owned_overflow() {
        let original = operation();
        let capture = Mutex::new(StderrCapture::new());
        let failures = reader_failure::ReaderFailures::admit_shared(&original).unwrap();
        let returned = retain_stderr_reader_failure(
            &capture,
            &failures,
            std::io::ErrorKind::BrokenPipe.into(),
        );
        assert_eq!(returned.kind(), std::io::ErrorKind::BrokenPipe);
        assert!(
            failures
                .lock()
                .unwrap()
                .has_io(reader_failure::ReaderStream::Stderr)
        );
        assert!(original.check().is_ok());
    }
}

/// Incremental framing bounds assembly BEFORE extending the buffer. Delimiters count toward
/// the raw limit; an oversized entry is rejected whole, never split into accepted answers.
struct FrameAssembly(Vec<u8>);

/// The existing 4 KiB read buffer stays owned by the SAME child, including a read
/// that returned bytes before a charge/assembly/enqueue failure. Cursor advances
/// only after that prefix moves into the retained assembly/frame owner.
struct StdoutChunk {
    bytes: [u8; crate::operation::STDOUT_CHUNK_BYTES],
    count: usize,
    cursor: usize,
    read_uncertain: bool,
    seen_accounted: bool,
    operation: Option<crate::operation::Operation>,
}
impl StdoutChunk {
    fn empty() -> Self {
        Self {
            bytes: [0; crate::operation::STDOUT_CHUNK_BYTES],
            count: 0,
            cursor: 0,
            read_uncertain: false,
            seen_accounted: false,
            operation: None,
        }
    }
    fn has_pending(&self) -> bool {
        self.read_uncertain || self.cursor < self.count
    }
}

fn stdout_chunk_owned(owner: &Mutex<Option<StdoutChunk>>) -> bool {
    // None means the original read guard has checked out the buffer, not an empty queue.
    owner
        .lock()
        .expect("original stdout chunk")
        .as_ref()
        .is_none_or(StdoutChunk::has_pending)
}

/// Take the bounded buffer out of its lock before pipe await. Cancellation restores
/// the original read association/uncertainty; no task drop is an ACK or join.
struct StdoutRead {
    owner: Arc<Mutex<Option<StdoutChunk>>>,
    chunk: Option<StdoutChunk>,
}
impl StdoutRead {
    fn begin_captured(
        owner: Arc<Mutex<Option<StdoutChunk>>>,
        capture: &Mutex<crate::operation::Operation>,
    ) -> std::io::Result<(crate::operation::Operation, Self)> {
        // Match rebind's capture -> chunk lock order. Association and checkout are atomic
        // against rebinding; neither synchronous lock survives the subsequent pipe await.
        let current = capture.lock().expect("stdout operation");
        let operation = current.clone();
        let read = Self::begin(owner, operation.clone())?;
        Ok((operation, read))
    }

    fn begin_reader(
        owner: Arc<Mutex<Option<StdoutChunk>>>,
        capture: &Mutex<crate::operation::Operation>,
        failures: &Mutex<reader_failure::ReaderFailures>,
        stream: reader_failure::ReaderStream,
    ) -> std::io::Result<(crate::operation::Operation, Self)> {
        let current = capture.lock().expect("original reader operation");
        let operation = current.clone();
        failures
            .lock()
            .expect("reader association")
            .begin(stream, &operation)?;
        let read = Self::begin(owner, operation.clone())?;
        Ok((operation, read))
    }

    fn begin(
        owner: Arc<Mutex<Option<StdoutChunk>>>,
        operation: crate::operation::Operation,
    ) -> std::io::Result<Self> {
        let chunk = {
            let mut slot = owner.lock().expect("original stdout chunk");
            if slot.as_ref().is_none_or(StdoutChunk::has_pending) {
                return Err(std::io::Error::other("original stdout chunk remains owned"));
            }
            let mut chunk = slot.take().expect("available stdout buffer");
            chunk.count = 0;
            chunk.cursor = 0;
            chunk.read_uncertain = true;
            chunk.seen_accounted = false;
            chunk.operation = Some(operation);
            chunk
        };
        Ok(Self {
            owner,
            chunk: Some(chunk),
        })
    }
    fn buffer(&mut self) -> &mut [u8] {
        &mut self.chunk.as_mut().expect("owned stdout read").bytes
    }
    fn returned(&mut self, count: usize) {
        let chunk = self.chunk.as_mut().expect("owned stdout read");
        chunk.count = count;
        chunk.read_uncertain = false;
    }

    /// ONLY the losing cancel-safe AsyncReadExt::read branch may call this.
    /// Nothing was delivered into this chunk; SAME pipe may still be Busy or
    /// buffer bytes internally. Generic cancellation/Drop remains uncertain.
    fn cooperative_pause(&mut self) -> std::io::Result<()> {
        let chunk = self.chunk.as_mut().expect("owned stdout read");
        if chunk.count != 0 || chunk.cursor != 0 || !chunk.read_uncertain {
            return Err(std::io::ErrorKind::Other.into());
        }
        chunk.read_uncertain = false;
        Ok(())
    }
}

enum PipeRead {
    Returned(usize),
    Paused(reader_handoff::Binding),
}

/// Concrete reader helper; caller retains the SAME pipe variable before/after
/// await. Actual returned bytes are recorded BEFORE gate/error/parser/copy work.
/// A read-winning race retains all bytes with the OLD fixed chunk, not FIFO or
/// normal transcript. This future is sized before first poll against creation
/// space; no box/task/channel/body or replacement read object is allocated here.
async fn read_or_pause<R: tokio::io::AsyncRead + Unpin>(
    pipe: &mut R,
    held: &mut StdoutRead,
    original: &crate::operation::Operation,
    gate: &reader_handoff::Gate,
    stream: reader_handoff::Pipe,
) -> std::io::Result<PipeRead> {
    tokio::select! {
        biased;
        result = pipe.read(held.buffer()) => {
            let count = match result {
                Ok(count) => count,
                Err(error) => { let _ = gate.read_error(original); return Err(error); }
            };
            held.returned(count);
            // Normal EOF retains existing partial-assembly behavior. During a
            // boundary it is failure, never the requested reader ACK.
            if count != 0 || gate.boundary_active() {
                gate.delivered(original, stream, count)
                    .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))?;
            }
            Ok(PipeRead::Returned(count))
        },
        request = gate.wait_request(original) => {
            let binding = request.map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))?;
            // select dropped ONLY this .read future before this branch executes.
            // The pipe itself is still the caller's SAME retained object.
            held.cooperative_pause()?;
            Ok(PipeRead::Paused(binding))
        },
    }
}

/// No operator wait here: this is inside the existing retained reader task.
/// Failure/deadline leaves it owning its pipe until actual original child exit;
/// later EOF drain cannot erase the sticky pause failure or mean healthy rebind.
async fn park_original_pipe(
    gate: &reader_handoff::Gate,
    binding: &reader_handoff::Binding,
    stream: reader_handoff::Pipe,
) -> bool {
    if gate.acknowledge(binding, stream).is_ok()
        && let Ok(resume) = gate.wait_resume(binding).await
    {
        return resume;
    }
    gate.wait_original_exit().await;
    false
}
impl Drop for StdoutRead {
    fn drop(&mut self) {
        if let Some(chunk) = self.chunk.take() {
            let mut slot = self.owner.lock().expect("original stdout chunk");
            debug_assert!(slot.is_none());
            *slot = Some(chunk);
        }
    }
}

impl FrameAssembly {
    fn push(
        &mut self,
        bytes: &[u8],
        operation: &crate::operation::Operation,
    ) -> Result<(), crate::operation::OperationError> {
        operation.check()?;
        let needed = self
            .0
            .len()
            .checked_add(bytes.len())
            .filter(|n| *n <= crate::operation::FRAME_BYTES)
            .ok_or(crate::operation::OperationError::CaptureOverflow)?;
        if needed > self.0.capacity() {
            // Admit requested replacement backing, old/new overlap and the Vec header
            // under the ORIGINAL read's cumulative credit before any reserve effect.
            // Like the stdout FIFO, allow bounded allocator rounding, then inspect the
            // returned capacity before accepting bytes. This postcheck is not a proof
            // of allocator internals or transient RSS; complete allocation fit is open.
            let target = needed
                .checked_next_power_of_two()
                .ok_or_else(|| operation.overflow())?;
            let admitted_capacity = target.checked_mul(2).ok_or_else(|| operation.overflow())?;
            let admission = admitted_capacity
                .checked_add(self.0.capacity())
                .and_then(|n| n.checked_add(std::mem::size_of::<Vec<u8>>()))
                .ok_or_else(|| operation.overflow())?;
            operation.charge(admission)?;
            self.0
                .try_reserve_exact(target - self.0.len())
                .map_err(|_| operation.overflow())?;
            if self.0.capacity() > admitted_capacity {
                // The original assembly still owns this allocation and its old prefix;
                // the caller's retained read chunk still owns every refused new byte.
                return Err(operation.overflow());
            }
        }
        // Capacity now covers the entire entry: this append cannot grow the backing.
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    fn take_line(&mut self, operation: &crate::operation::Operation) -> std::io::Result<String> {
        operation.check().map_err(std::io::Error::other)?;
        let bytes = std::mem::take(&mut self.0);
        match String::from_utf8(bytes) {
            Ok(line) => {
                // from_utf8 moves the original allocation. Admit the additional trimmed
                // String before copying; refusal restores the whole accepted raw assembly.
                let length = line.trim_end_matches(['\r', '\n']).len();
                let admission = length
                    .checked_add(std::mem::size_of::<String>())
                    .ok_or_else(|| operation.overflow())
                    .and_then(|bytes| operation.charge(bytes));
                if let Err(error) = admission {
                    self.0 = line.into_bytes();
                    return Err(std::io::Error::other(error));
                }
                let mut trimmed = String::new();
                if trimmed.try_reserve_exact(length).is_err() || trimmed.capacity() > length {
                    // No new body was accepted. Keep the SAME full raw allocation on
                    // allocator refusal or capacity beyond the already charged request.
                    // Capacity inspection is not a transient allocator/RSS fit proof.
                    self.0 = line.into_bytes();
                    return Err(std::io::Error::other(operation.overflow()));
                }
                if let Err(error) = operation.check() {
                    self.0 = line.into_bytes();
                    return Err(std::io::Error::other(error));
                }
                // The entire admitted UTF-8 body fits; push_str cannot grow the buffer.
                trimmed.push_str(line.trim_end_matches(['\r', '\n']));
                Ok(trimmed)
            }
            Err(error) => {
                self.0 = error.into_bytes(); // accepted raw evidence remains in SAME owner
                Err(std::io::Error::other("invalid stdout UTF-8"))
            }
        }
    }
}

fn rebind_capture(
    capture: &Mutex<crate::operation::Operation>,
    operation: crate::operation::Operation,
    queued_frames: impl FnOnce() -> usize,
) -> Result<(), DriverError> {
    let mut current = capture.lock().expect("child operation");
    if current.same_instance(&operation) {
        return Ok(());
    }
    if queued_frames() != 0 {
        return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
            "original child frames remain owned; operation rebinding refused".into(),
        )));
    }
    operation
        .check()
        .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
    *current = operation;
    Ok(())
}

fn pin_cleanup_deadline(
    retained: &mut Option<Instant>,
    requested: Instant,
    original: Instant,
) -> Instant {
    let deadline = requested.min(original);
    let deadline = retained.map_or(deadline, |old| old.min(deadline));
    *retained = Some(deadline);
    deadline
}

impl ChildProc {
    pub fn spawn(
        cmd: &mut Command,
        operation: crate::operation::Operation,
    ) -> std::io::Result<Self> {
        Self::spawn_inner(cmd, operation, false)
    }

    fn spawn_session(
        cmd: &mut Command,
        operation: crate::operation::Operation,
    ) -> std::io::Result<Self> {
        Self::spawn_inner(cmd, operation, true)
    }

    fn spawn_inner(
        cmd: &mut Command,
        operation: crate::operation::Operation,
        session_handoff: bool,
    ) -> std::io::Result<Self> {
        operation
            .check()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        // UUID text and its transient-to-retained conversion are charged BEFORE spawn.
        // This is a trusted process identity, independent of a reused controller epoch.
        operation
            .charge(
                72 + std::mem::size_of::<String>()
                    + (std::mem::size_of::<Self>()
                        - if session_handoff {
                            reader_handoff::child_header_bytes()
                        } else {
                            0
                        })
                    + 2 * std::mem::size_of::<tokio::task::JoinHandle<std::io::Result<()>>>()
                    + 4 * std::mem::size_of::<StdoutChunk>()
                    + 2 * std::mem::size_of::<StdoutRead>()
                    + std::mem::size_of::<StderrCapture>()
                    + (crate::operation::STDERR_LINES + 1) * std::mem::size_of::<StderrLine>(),
            )
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        // Admit the one FIFO's Arc/synchronization/closure/receive-future metadata
        // BEFORE child spawn; sticky overflow cannot create an uncharged channel.
        let (tx, lines) =
            frame_queue::channel(&operation).map_err(|e| std::io::Error::other(e.to_string()))?;
        let reader_error = reader_failure::ReaderFailures::admit_shared(&operation)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let stdout_chunk = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let stderr_chunk = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let physical_child_id: Arc<str> = uuid::Uuid::new_v4().to_string().into();
        let handoff = if session_handoff {
            let gate = operation.reader_handoff();
            gate.attach(&operation, &physical_child_id)
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))?;
            Some(gate)
        } else {
            None
        };
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        operation
            .check()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let capture = Arc::new(Mutex::new(operation));
        let stdout_capture = capture.clone();
        let stdout_child_id = physical_child_id.clone();
        let chunk_owner = stdout_chunk.clone();
        let stdout_error = reader_error.clone();
        let stdout_assembly = Arc::new(Mutex::new(FrameAssembly(Vec::new())));
        let assembly_owner = stdout_assembly.clone();
        let stdout_pending = Arc::new(Mutex::new(None));
        let frame_owner = stdout_pending.clone();
        let stdout_handoff = handoff.clone();
        let stdout_reader = tokio::spawn(async move {
            let mut stdout = stdout;
            let mut frame_operation: Option<crate::operation::Operation> = None;
            let mut supervision_drain = false;
            loop {
                let result = async {
                    let (before_read, mut held) = StdoutRead::begin_reader(
                        chunk_owner.clone(),
                        &stdout_capture,
                        &stdout_error,
                        reader_failure::ReaderStream::Stdout,
                    )?;
                    let count = if let Some(gate) = &stdout_handoff
                        && !supervision_drain
                    {
                        let future = read_or_pause(
                            &mut stdout,
                            &mut held,
                            &before_read,
                            gate,
                            reader_handoff::Pipe::Stdout,
                        );
                        if std::mem::size_of_val(&future) > reader_handoff::WAIT_BYTES {
                            return Err(std::io::ErrorKind::Other.into());
                        }
                        match future.await? {
                            PipeRead::Returned(count) => count,
                            PipeRead::Paused(binding) => {
                                drop(held); // SAME zero-delivery chunk restored BEFORE ACK
                                let future = park_original_pipe(
                                    gate,
                                    &binding,
                                    reader_handoff::Pipe::Stdout,
                                );
                                if std::mem::size_of_val(&future) > reader_handoff::WAIT_BYTES {
                                    return Err(std::io::ErrorKind::Other.into());
                                }
                                supervision_drain = !future.await;
                                return Ok(true);
                            }
                        }
                    } else {
                        let count = stdout.read(held.buffer()).await?;
                        held.returned(count);
                        count
                    };
                    let current_read = before_read
                        .same_instance(&stdout_capture.lock().expect("stdout operation"));
                    // Returned bytes remain the original read's liability even if the capture
                    // association changed unexpectedly. UNKNOWN origin cannot transfer costs.
                    let operation = before_read;
                    // No filesystem/pipe await under this synchronous exclusion. The SAME
                    // ChildProc retains partial/invalid below-cap bytes even if this task ends.
                    let mut assembly = assembly_owner.lock().expect("retained stdout assembly");
                    if count == 0 {
                        if !assembly.0.is_empty() {
                            let line = assembly.take_line(&operation)?;
                            // An unavailable origin never permits dropping the already-read
                            // body. Preserve UNKNOWN provenance and let the SAME meter refuse
                            // at the retained enqueue boundary if overflow/settlement closed it.
                            let origin = frame_operation
                                .take()
                                .map(|op| op.next_child_origin(&stdout_child_id))
                                .transpose()
                                .unwrap_or(None);
                            let frame = ChildFrame {
                                text: line,
                                origin,
                                operation: operation.clone(),
                            };
                            enqueue_stdout_frame(&tx, &frame_owner, frame)?;
                        }
                        return Ok(false);
                    }
                    operation
                        .charge(count)
                        .map_err(|e| std::io::Error::other(e.to_string()))?;
                    while held.chunk.as_ref().expect("owned read").cursor < count {
                        let chunk = held.chunk.as_ref().expect("owned read");
                        let start = chunk.cursor;
                        let end = chunk.bytes[start..count]
                            .iter()
                            .position(|b| *b == b'\n')
                            .map_or(count, |n| start + n + 1);
                        let part = &chunk.bytes[start..end];
                        let newline = part.last() == Some(&b'\n');
                        if assembly.0.is_empty() {
                            frame_operation = current_read.then(|| operation.clone());
                        } else if !current_read
                            || frame_operation
                                .as_ref()
                                .is_some_and(|op| !op.same_instance(&operation))
                        {
                            frame_operation = None;
                        }
                        if let Err(error) = assembly.push(part, &operation) {
                            if error == crate::operation::OperationError::CaptureOverflow {
                                operation.overflow();
                            }
                            return Err(std::io::Error::other(error));
                        }
                        held.chunk.as_mut().expect("owned read").cursor = end;
                        if newline {
                            let line = assembly.take_line(&operation)?;
                            if !line.is_empty() {
                                let origin = frame_operation
                                    .take()
                                    .map(|op| op.next_child_origin(&stdout_child_id))
                                    .transpose()
                                    .unwrap_or(None);
                                let frame = ChildFrame {
                                    text: line,
                                    origin,
                                    operation: operation.clone(),
                                };
                                enqueue_stdout_frame(&tx, &frame_owner, frame)?;
                            }
                        }
                    }
                    if let Some(gate) = &stdout_handoff
                        && !supervision_drain
                    {
                        gate.processed(&operation, reader_handoff::Pipe::Stdout)
                            .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))?;
                    }
                    Ok(true)
                }
                .await;
                match result {
                    Ok(true) => {}
                    Ok(false) => return Ok(()),
                    Err(error) => {
                        let kind = stdout_error
                            .lock()
                            .expect("stdout reader error")
                            .retain_io(reader_failure::ReaderStream::Stdout, error)?;
                        return Err(std::io::Error::from(kind));
                    }
                }
            }
        });
        let stderr_tail = Arc::new(Mutex::new(StderrCapture::new()));
        let tail = stderr_tail.clone();
        let stderr_stats = Arc::new(Mutex::new(StderrStats::default()));
        let stats = stderr_stats.clone();
        let stderr_capture = capture.clone();
        let stderr_chunk_owner = stderr_chunk.clone();
        let stderr_error = reader_error.clone();
        let stderr_handoff = handoff.clone();
        let stderr_reader = tokio::spawn(async move {
            let mut supervision_drain = false;
            let result: std::io::Result<()> = async {
                loop {
                    // Reuse the bounded pipe guard, but keep a distinct SAME-child stderr
                    // buffer. Its original capture association is checked out before await.
                    let (before_read, mut read) = StdoutRead::begin_reader(
                        stderr_chunk_owner.clone(),
                        &stderr_capture,
                        &stderr_error,
                        reader_failure::ReaderStream::Stderr,
                    )?;
                    let count = if let Some(gate) = &stderr_handoff
                        && !supervision_drain
                    {
                        let future = read_or_pause(
                            &mut stderr,
                            &mut read,
                            &before_read,
                            gate,
                            reader_handoff::Pipe::Stderr,
                        );
                        if std::mem::size_of_val(&future) > reader_handoff::WAIT_BYTES {
                            return Err(std::io::ErrorKind::Other.into());
                        }
                        match future.await? {
                            PipeRead::Returned(count) => count,
                            PipeRead::Paused(binding) => {
                                drop(read);
                                let future = park_original_pipe(
                                    gate,
                                    &binding,
                                    reader_handoff::Pipe::Stderr,
                                );
                                if std::mem::size_of_val(&future) > reader_handoff::WAIT_BYTES {
                                    return Err(std::io::ErrorKind::Other.into());
                                }
                                supervision_drain = !future.await;
                                continue;
                            }
                        }
                    } else {
                        let count = stderr.read(read.buffer()).await?;
                        read.returned(count);
                        count
                    };
                    let mut retained = tail.lock().expect("stderr tail");
                    let mut stats = stats.lock().expect("stderr stats");
                    retained.accept_read(&mut read, &mut stats)?;
                    if count == 0 {
                        retained.finish_partial()?;
                        return Ok(());
                    }
                    if let Some(gate) = &stderr_handoff
                        && !supervision_drain
                    {
                        gate.processed(&before_read, reader_handoff::Pipe::Stderr)
                            .map_err(|_| std::io::Error::from(std::io::ErrorKind::Other))?;
                    }
                }
            }
            .await;
            match result {
                Ok(()) => Ok(()),
                Err(error) => Err(retain_stderr_reader_failure(&tail, &stderr_error, error)),
            }
        });
        Ok(Self {
            physical_child_id,
            child,
            stdin,
            lines,
            pending_frame: None,
            pending_events: None,
            stdout_assembly,
            stdout_pending,
            stdout_chunk,
            turn_quota: Vec::new(),
            helper_output: String::new(),
            stderr_tail,
            stderr_chunk,
            readers: vec![stdout_reader, stderr_reader],
            reader_joined: [false; 2],
            reader_error,
            capture,
            stderr_stats,
            last_origin: None,
            cleanup_deadline: None,
            physical_joined: false,
            handoff,
            handoff_request: None,
        })
    }

    /// Accepted old frames keep their SAME owner. A new operation cannot replace that
    /// owner, including when a caller reuses its textual id. Same-owner cleanup is allowed.
    pub fn bind_operation(
        &mut self,
        operation: crate::operation::Operation,
    ) -> Result<(), DriverError> {
        self.check_reader_rebind(&operation)?;
        if (!self.helper_output.is_empty()
            || self
                .reader_error
                .lock()
                .expect("original reader failure")
                .has_failure())
            && !self.capture_operation().same_instance(&operation)
        {
            return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                "original helper output/reader failure remains owned; rebinding refused".into(),
            )));
        }
        if self.cleanup_deadline.is_some() && !self.capture_operation().same_instance(&operation) {
            return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                "original child cleanup bound remains owned; operation rebinding refused".into(),
            )));
        }
        rebind_capture(&self.capture, operation, || self.queued_frames())
    }

    fn check_reader_rebind(
        &self,
        operation: &crate::operation::Operation,
    ) -> Result<(), DriverError> {
        if self.handoff.is_some() && !self.capture_operation().same_instance(operation) {
            // Source integration intentionally has no new-prompt/sequence adoption
            // yet. Even a zero queue/BOTH ACK is not authority to transfer an idle
            // pipe's internally buffered bytes to a different Operation.
            return Err(DriverError::Host(crate::host::HostRefusal::Unknown));
        }
        Ok(())
    }

    /// Private actual-terminal caller seam. Install the original request BEFORE
    /// await; interruption keeps its child/gate/old Operation on this SAME owner.
    /// Called after the independently correlated actual terminal was consumed
    /// and its outcome retained. BOTH ACKs still grant no new-prompt/root release.
    async fn pause_after_terminal(
        &mut self,
        original: &crate::operation::Operation,
    ) -> Result<(), DriverError> {
        if !self.capture_operation().same_instance(original) || self.handoff_request.is_some() {
            return Err(DriverError::Host(crate::host::HostRefusal::Unknown));
        }
        original
            .check_claude_terminal()
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))?;
        let gate = self
            .handoff
            .as_ref()
            .ok_or(DriverError::Host(crate::host::HostRefusal::Unknown))?;
        gate.terminal_processed(original)
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))?;
        let binding = gate
            .request(original, &self.physical_child_id)
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))?;
        self.handoff_request = Some(binding);
        let wait = gate
            .wait_both(
                self.handoff_request
                    .as_ref()
                    .expect("original pause request"),
            )
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))?;
        wait.await
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))
    }

    fn check_terminal_pause(
        &self,
        original: &crate::operation::Operation,
    ) -> Result<(), DriverError> {
        if !self.capture_operation().same_instance(original) {
            return Err(DriverError::Host(crate::host::HostRefusal::Unknown));
        }
        original
            .check_claude_terminal()
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))?;
        let gate = self
            .handoff
            .as_ref()
            .filter(|gate| std::ptr::eq(gate.as_ref(), original.reader_handoff_ref()))
            .ok_or(DriverError::Host(crate::host::HostRefusal::Unknown))?;
        let request = self
            .handoff_request
            .as_ref()
            .ok_or(DriverError::Host(crate::host::HostRefusal::Unknown))?;
        gate.check_paused(request, &self.physical_child_id)
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))
    }

    /// Pin an existing substage's earlier ceiling, including before a preflight await can
    /// be canceled. Later cleanup cannot reset it or borrow a replacement context's time.
    pub(crate) fn restrict_cleanup_deadline(&mut self, requested: Instant) -> Instant {
        let original = self.capture_operation().allowance().deadline();
        pin_cleanup_deadline(&mut self.cleanup_deadline, requested, original)
    }

    pub(crate) fn physical_child_id(&self) -> &str {
        &self.physical_child_id
    }

    pub fn stderr_stats(&self) -> StderrStats {
        self.stderr_stats.lock().expect("stderr stats").clone()
    }

    pub fn reader_error(&mut self) -> Result<Option<String>, DriverError> {
        // A diagnostic display callback must not run under a synchronous lock.
        // Still-shared reader storage refuses this optional display copy; it
        // never hides/retags the canonical failure or proves a task join.
        archive_stdout::unique_storage(&mut self.reader_error)
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))?
            .display_first()
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))
    }

    pub fn last_origin(&self) -> Option<&crate::operation::CaptureOrigin> {
        self.last_origin.as_ref()
    }

    pub fn capture_operation(&self) -> crate::operation::Operation {
        self.capture.lock().expect("child operation").clone()
    }

    /// Joined readers may still own accepted FIFO rows. The supervisor must preserve/drain
    /// them before dropping this child owner; EOF/terminal response is not queue settlement.
    pub fn queued_frames(&self) -> usize {
        self.lines.len()
            + usize::from(self.handoff_request.is_some())
            + usize::from(self.pending_frame.is_some())
            + usize::from(self.pending_events.is_some())
            + usize::from(
                !self
                    .stdout_assembly
                    .lock()
                    .expect("retained stdout assembly")
                    .0
                    .is_empty(),
            )
            + usize::from(
                self.stdout_pending
                    .lock()
                    .expect("in-service stdout frame")
                    .is_some(),
            )
            + usize::from(stdout_chunk_owned(&self.stdout_chunk))
            + usize::from(stdout_chunk_owned(&self.stderr_chunk))
            + usize::from(
                self.reader_error
                    .lock()
                    .expect("retained reader failures")
                    .has_failure(),
            )
            + usize::from(
                self.stderr_tail
                    .lock()
                    .expect("stderr tail")
                    .has_retained_evidence(),
            )
    }

    pub(crate) fn acknowledge_line(&mut self) {
        self.pending_frame = None;
    }

    pub(crate) fn append_helper_output(&mut self, line: &str) -> Result<(), DriverError> {
        let original = self.capture_operation();
        append_helper_frame(
            &mut self.helper_output,
            &mut self.pending_frame,
            &original,
            line,
        )
    }

    pub(crate) fn has_retained_helper_output(&self) -> bool {
        !self.helper_output.is_empty()
    }

    fn parse_retained(&mut self, parser: &mut dyn StreamParser) -> Result<(), DriverError> {
        if self.pending_events.is_some() {
            return Ok(());
        }
        // Decode refusal leaves this exact frame, read association and liability in place.
        // Do not take raw ownership until the fallible owned decoder has returned events.
        self.pending_events = Some(decode_pending_frame(&mut self.pending_frame, parser)?);
        let batch = self.pending_events.as_ref().expect("retained parsed frame");
        // This charges encoded derived bodies plus their actual enum storage. It is not an
        // RSS bound. The owned decoder/builder admit requested JSON/event payload storage
        // first; map/allocator overhead and complete fit remain unproved. An overflow
        // retains the raw frame/derived suffix in the original child instead of dropping it.
        let mut bytes = batch
            .events
            .capacity()
            .checked_mul(std::mem::size_of::<StreamEvent>())
            .ok_or_else(|| {
                DriverError::Host(crate::host::operation_refusal(
                    batch.raw.operation.overflow(),
                ))
            })?;
        for event in &batch.events {
            bytes = bytes
                .checked_add(event_bytes(event).map_err(|_| {
                    DriverError::Host(crate::host::operation_refusal(
                        batch.raw.operation.overflow(),
                    ))
                })?)
                .ok_or_else(|| {
                    DriverError::Host(crate::host::operation_refusal(
                        batch.raw.operation.overflow(),
                    ))
                })?;
        }
        batch
            .raw
            .operation
            .charge(bytes)
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))
    }

    fn accept_event(
        &mut self,
        accept: &mut impl FnMut(
            &StreamEvent,
            Option<&crate::operation::CaptureOrigin>,
        ) -> Result<(), DriverError>,
    ) -> Result<Option<StreamEvent>, DriverError> {
        let batch = self
            .pending_events
            .as_mut()
            .ok_or_else(|| DriverError::Protocol("no retained event batch".into()))?;
        let event = batch.accept_front(accept)?;
        if batch.events.is_empty() {
            self.pending_events = None;
        }
        Ok(event)
    }

    async fn join_readers(&mut self, deadline: Instant) -> std::io::Result<()> {
        if let Some(gate) = &self.handoff {
            gate.child_exit_observed(); // entered only AFTER actual original child wait
        }
        // SAME child is already joined. Readers drain to EOF into the admitted retained FIFO.
        // No abort of a healthy reader discards a below-cap partial frame. On timeout the handles
        // and FIFO remain owned by this ChildProc, including pipes held open by descendants.
        if self.readers.len() > self.reader_joined.len() {
            return Err(std::io::ErrorKind::Other.into());
        }
        while !self.readers.is_empty() {
            for index in 0..self.readers.len() {
                let stream = reader_failure::ReaderStream::from_index(index)
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::Other))?;
                if !self.reader_joined[index]
                    && self
                        .reader_error
                        .lock()
                        .expect("original joined reader")
                        .has_join(stream)
                {
                    // An occupied actual-join slot cannot authorize another poll.
                    return Err(std::io::ErrorKind::Other.into());
                }
            }
            let finished = (0..self.readers.len())
                .rev()
                .find(|&index| !self.reader_joined[index] && self.readers[index].is_finished());
            let (index, result) = if let Some(index) = finished {
                // Actual finished dispositions are observed even after expiry.
                (index, (&mut self.readers[index]).await)
            } else {
                let joined = self.reader_joined;
                let next = async {
                    match self.readers.as_mut_slice() {
                        [stdout, stderr] => {
                            tokio::select! {
                                biased;
                                result = stderr, if !joined[1] => (1, result),
                                result = stdout, if !joined[0] => (0, result),
                            }
                        }
                        [stdout] => (0, stdout.await),
                        _ => unreachable!("bounded original reader handles"),
                    }
                };
                tokio::time::timeout_at(deadline, next).await.map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "reader join unsettled")
                })?
            };
            let stream = reader_failure::ReaderStream::from_index(index)
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::Other))?;
            {
                let mut retained = self.reader_error.lock().expect("retained reader error");
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        // Production tasks return a fixed summary after moving the original
                        // error into this slot. A direct task error is retained here instead.
                        if !retained.has_io(stream) {
                            retained.retain_io(stream, error)?;
                        }
                    }
                    Err(error) => {
                        // The slot was checked before polling and reader tasks never write
                        // join outcomes, so no competing join can occupy it during await.
                        if let Err(error) = retained.retain_join(stream, error) {
                            return Err(std::io::Error::new(std::io::ErrorKind::Other, error));
                        }
                    }
                }
            }
            // This synchronous interval has no cancellation point: retain the
            // actual disposition before marking, and never repoll a consumed future.
            self.reader_joined[index] = true;
            while !self.readers.is_empty() && self.reader_joined[self.readers.len() - 1] {
                let tail = self.readers.len() - 1;
                self.readers.pop(); // Only AFTER actual await and canonical retention.
                self.reader_joined[tail] = false;
            }
        }
        // Every original reader handle was actually awaited after the SAME child
        // wait. Preserve this fact even if a reader returned an error/evidence remains.
        self.physical_joined = true;
        match self
            .reader_error
            .lock()
            .expect("first reader failure")
            .first_kind()
        {
            Some(kind) => Err(std::io::Error::from(kind)),
            None => Ok(()),
        }
    }

    /// Close input and give an idle CLI time to persist resumable session totals.
    /// Active/canceled turns use kill instead; this grace is bounded.
    pub async fn finish(&mut self, grace: Duration) -> std::io::Result<()> {
        let operation = self.capture.lock().expect("child operation").clone();
        let deadline = operation.allowance().capped(grace).deadline();
        self.finish_until(deadline).await
    }

    pub async fn finish_until(&mut self, deadline: Instant) -> std::io::Result<()> {
        let deadline = self.restrict_cleanup_deadline(deadline);
        self.close_stdin();
        match tokio::time::timeout_at(deadline, self.child.wait()).await {
            Ok(Ok(_)) => self.join_readers(deadline).await,
            Ok(Err(error)) => Err(error),
            Err(_) => self.kill_until(deadline).await,
        }
    }

    pub(crate) async fn send_line(
        &mut self,
        line: &str,
        deadline: Instant,
        lease: &dyn crate::host::HostPromptLease,
    ) -> Result<(), DriverError> {
        if self.handoff_request.is_some() {
            // Current source has no accepted sequence-aware resume adoption.
            // BOTH pause ACKs alone never authorize a new or repeated prompt.
            return Err(DriverError::Host(crate::host::HostRefusal::Unknown));
        }
        let operation = self.capture.lock().expect("child operation").clone();
        let deadline = deadline.min(operation.allowance().deadline());
        operation
            .check()
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        lease.check().map_err(DriverError::Host)?;
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| DriverError::Died("stdin closed".into()))?;
        let dispatch = async {
            // Same absolute deadline and fresh host checks for every write, newline and flush.
            // No generic unguarded stdin entrypoint remains.
            for bytes in [line.as_bytes(), b"\n".as_slice()] {
                let mut remaining = bytes;
                while !remaining.is_empty() {
                    operation
                        .check()
                        .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
                    lease.check().map_err(DriverError::Host)?;
                    let mut transfer = PendingDispatch {
                        operation: operation.clone(),
                        armed: true,
                    };
                    let result = stdin.write(remaining).await;
                    let count = result.map_err(|e| DriverError::Died(e.to_string()))?;
                    transfer.armed = false; // actual Ok(count), including a verified zero
                    if count == 0 {
                        return Err(DriverError::Died("prompt write zero".into()));
                    }
                    operation.possible_start();
                    remaining = &remaining[count..];
                }
            }
            operation
                .check()
                .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
            lease.check().map_err(DriverError::Host)?;
            stdin
                .flush()
                .await
                .map_err(|e| DriverError::Died(e.to_string()))
        };
        tokio::select! {
            result = tokio::time::timeout_at(deadline, dispatch) =>
                result.map_err(|_| DriverError::Host(crate::host::HostRefusal::Expired))?,
            () = operation.wait_overflow() => Err(DriverError::Host(crate::host::HostRefusal::CaptureOverflow)),
        }
    }

    /// Close stdin (EOF), which ends a CLI that reads its prompts from it.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Next stdout line; `Ok(None)` at EOF; `Err` when `deadline` passes first.
    pub async fn next_line(&mut self, deadline: Instant) -> Result<Option<String>, DriverError> {
        if self.handoff_request.is_some() {
            // Between-turn raw evidence stays on the OLD canonical owners and
            // cannot be copied into a normal parser/seat/spectator transcript.
            return Err(DriverError::Host(crate::host::HostRefusal::Unknown));
        }
        if self.pending_events.is_some() {
            return Err(DriverError::Protocol("parsed frame still owned".into()));
        }
        if self.pending_frame.is_none() {
            self.pending_frame = tokio::time::timeout_at(deadline, self.lines.recv())
                .await
                .map_err(|_| DriverError::Timeout)?;
        }
        let Some(frame) = self.pending_frame.as_ref() else {
            return Ok(None);
        };
        copy_stdout_frame_view(frame, &mut self.last_origin)
            .map(Some)
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))
    }

    pub fn is_running(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(Some(_)))
    }

    fn physical_supervision_complete(&self) -> bool {
        self.physical_joined && self.readers.is_empty()
    }

    /// Borrow only this canonical FIFO after actual original child/reader joins.
    /// Other partial buffers, startup/helper records and pairings remain separately
    /// owned. This borrow is no complete covered-set or storage/settlement ACK.
    fn borrowed_stdout_evidence(
        &mut self,
        original: &crate::operation::Operation,
    ) -> Result<(&str, &VecDeque<ChildFrame>), crate::failed_evidence::ArchiveError> {
        if !self.physical_supervision_complete()
            || !self
                .capture
                .lock()
                .expect("archive original capture")
                .same_instance(original)
        {
            return Err(crate::failed_evidence::ArchiveError::Identity);
        }
        let rows = self.lines.joined_frames()?;
        if rows
            .iter()
            .any(|row| !row.operation.same_instance(original))
        {
            return Err(crate::failed_evidence::ArchiveError::Identity);
        }
        Ok((&self.physical_child_id, rows))
    }

    fn cover_queued_stdout(
        &mut self,
        original: &crate::operation::Operation,
        coverage: &mut crate::failed_evidence::EvidenceCoverageBuilder<'_>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        let (physical_child, rows) = self.borrowed_stdout_evidence(original)?;
        for (index, row) in rows.iter().enumerate() {
            coverage.include(&queued_stdout_record(row, physical_child, index)?)?;
        }
        Ok(())
    }

    fn visit_reader_failures_archive(
        &mut self,
        original: &crate::operation::Operation,
        sink: &mut impl archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        use archive_stdout::unique_storage;
        if !self.physical_supervision_complete()
            || !unique_storage(&mut self.capture)?.same_instance(original)
        {
            return Err(ArchiveError::Identity);
        }
        // All original tasks are joined before unique access; no Mutex guard,
        // move, body clone or lock survives into the serializer callback.
        let failures = unique_storage(&mut self.reader_error)?;
        failures.visit_archive(original, &self.physical_child_id, sink)
    }

    fn cover_reader_failures_archive(
        &mut self,
        original: &crate::operation::Operation,
        coverage: &mut crate::failed_evidence::EvidenceCoverageBuilder<'_>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.visit_reader_failures_archive(original, coverage)
    }

    fn encode_reader_failures_archive<W: std::io::Write>(
        &mut self,
        original: &crate::operation::Operation,
        encoder: &mut crate::failed_evidence::ArchiveEncoder<'_, W>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.visit_reader_failures_archive(original, encoder)
    }

    fn encode_queued_stdout<W: std::io::Write>(
        &mut self,
        original: &crate::operation::Operation,
        encoder: &mut crate::failed_evidence::ArchiveEncoder<'_, W>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        let (physical_child, rows) = self.borrowed_stdout_evidence(original)?;
        for (index, row) in rows.iter().enumerate() {
            encoder.bytes(
                &queued_stdout_record(row, physical_child, index)?,
                row.text.as_bytes(),
            )?;
        }
        Ok(())
    }

    /// Actual joins and unique storage precede a borrowed view. No lock or
    /// evidence move survives into coverage/encoder callbacks. This covers only
    /// stdout: helper/startup/stderr/pairing custody remains separately required.
    fn borrowed_stdout_storage<'a>(
        &'a mut self,
        original: &'a crate::operation::Operation,
    ) -> Result<archive_stdout::BorrowedStdout<'a>, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        use archive_stdout::unique_storage;
        if !self.physical_supervision_complete()
            || !unique_storage(&mut self.capture)?.same_instance(original)
        {
            return Err(ArchiveError::Identity);
        }
        let queued = self.lines.joined_frames()?;
        let assembly = unique_storage(&mut self.stdout_assembly)?;
        let reader_pending = unique_storage(&mut self.stdout_pending)?.as_ref();
        let chunk = unique_storage(&mut self.stdout_chunk)?
            .as_ref()
            .ok_or(ArchiveError::Identity)?;
        Ok(archive_stdout::BorrowedStdout {
            operation: original,
            physical_child: &self.physical_child_id,
            queued,
            pending: self.pending_frame.as_ref(),
            events: self.pending_events.as_ref(),
            assembly,
            reader_pending,
            chunk,
        })
    }

    fn cover_stdout_storage(
        &mut self,
        original: &crate::operation::Operation,
        coverage: &mut crate::failed_evidence::EvidenceCoverageBuilder<'_>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.borrowed_stdout_storage(original)?.cover(coverage)
    }

    fn encode_stdout_storage<W: std::io::Write>(
        &mut self,
        original: &crate::operation::Operation,
        encoder: &mut crate::failed_evidence::ArchiveEncoder<'_, W>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.borrowed_stdout_storage(original)?.encode(encoder)
    }

    fn borrowed_stderr_storage<'a>(
        &'a mut self,
        original: &'a crate::operation::Operation,
    ) -> Result<archive_stderr::BorrowedStderr<'a>, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        use archive_stdout::unique_storage;
        if !self.physical_supervision_complete()
            || !unique_storage(&mut self.capture)?.same_instance(original)
        {
            return Err(ArchiveError::Identity);
        }
        Ok(archive_stderr::BorrowedStderr {
            operation: original,
            physical_child: &self.physical_child_id,
            retained: unique_storage(&mut self.stderr_tail)?,
            stats: unique_storage(&mut self.stderr_stats)?,
            chunk: unique_storage(&mut self.stderr_chunk)?
                .as_ref()
                .ok_or(ArchiveError::Identity)?,
        })
    }

    fn cover_stderr_storage(
        &mut self,
        original: &crate::operation::Operation,
        coverage: &mut crate::failed_evidence::EvidenceCoverageBuilder<'_>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.borrowed_stderr_storage(original)?.cover(coverage)
    }

    fn encode_stderr_storage<W: std::io::Write>(
        &mut self,
        original: &crate::operation::Operation,
        encoder: &mut crate::failed_evidence::ArchiveEncoder<'_, W>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.borrowed_stderr_storage(original)?.encode(encoder)
    }

    fn visit_helper_output(
        &mut self,
        original: &crate::operation::Operation,
        sink: &mut impl archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::{ArchiveError, EvidenceCategory, EvidenceRecord};
        if !self.physical_supervision_complete()
            || !archive_stdout::unique_storage(&mut self.capture)?.same_instance(original)
        {
            return Err(ArchiveError::Identity);
        }
        if !self.helper_output.is_empty() {
            // Accepted helper output is an existing canonical derived string,
            // not a reconstructed kernel frame or an archive of every diagnostic.
            sink.bytes(
                EvidenceRecord {
                    operation: original,
                    category: EvidenceCategory::Helper,
                    part: "accepted_helper_output",
                    index: 0,
                    physical_child: Some(&self.physical_child_id),
                    origin: None,
                },
                self.helper_output.as_bytes(),
            )?;
        }
        Ok(())
    }

    fn cover_helper_output(
        &mut self,
        original: &crate::operation::Operation,
        coverage: &mut crate::failed_evidence::EvidenceCoverageBuilder<'_>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.visit_helper_output(original, coverage)
    }

    /// One deterministic borrowed traversal of this actual original child.
    /// Reader panic payloads and any shared/checked-out storage refuse. A caller
    /// must complete this traversal as a coverage preflight BEFORE output.
    fn visit_retained_archive(
        &mut self,
        original: &crate::operation::Operation,
        sink: &mut impl archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        if let Some(gate) = self.handoff.as_ref() {
            // The original driver enumerates gate/request metadata separately.
            // Recheck association before child bodies without duplicating that
            // metadata or treating coverage as accepted custody/healthy resume.
            gate.check_archive_child(
                original,
                &self.physical_child_id,
                self.handoff_request.as_ref(),
            )?;
        } else if self.handoff_request.is_some() {
            return Err(crate::failed_evidence::ArchiveError::Identity);
        }
        use crate::failed_evidence::{EvidenceCategory, EvidenceRecord};
        self.borrowed_stdout_storage(original)?.visit(sink)?;
        self.borrowed_stderr_storage(original)?.visit(sink)?;
        self.visit_reader_failures_archive(original, sink)?;
        self.visit_helper_output(original, sink)?;
        // These are retained derived copies, not recreated raw bytes. Known
        // last-origin association is validated; absent origin stays absent.
        sink.json(
            EvidenceRecord {
                operation: original,
                category: EvidenceCategory::Diagnostic,
                part: "child_retained_metadata",
                index: 0,
                physical_child: Some(&self.physical_child_id),
                origin: self.last_origin.as_ref(),
            },
            &self.turn_quota,
        )
    }

    /// Serialization of an io::Error's display does not establish complete
    /// custody of an arbitrary custom payload. Keep ALL canonical buffers when
    /// any reader error remains, including its original first-cause owner.
    /// The complete joined/unique/body preflight is performed by the driver.
    fn check_archive_release(&mut self) -> Result<(), crate::failed_evidence::ArchiveError> {
        archive_stdout::unique_storage(&mut self.stderr_tail)?.check_legacy_archive_release()?;
        if self.handoff_request.is_some()
            || self
                .handoff
                .as_ref()
                .is_some_and(|gate| gate.boundary_active())
            || archive_stdout::unique_storage(&mut self.reader_error)?.has_failure()
            || !self.turn_quota.is_empty()
            || self.pending_events.as_ref().is_some_and(|batch| {
                batch.events.iter().any(|event| {
                    matches!(
                        event,
                        StreamEvent::ModelStart { .. }
                            | StreamEvent::Quota(_)
                            | StreamEvent::TurnDone(_)
                    )
                })
            })
        {
            // Active handoff lifecycle and captured accounting still need their
            // original disposition. Covered bytes or a diagnostic file cannot
            // clear the gate/request or substitute journal/usage settlement.
            return Err(crate::failed_evidence::ArchiveError::Identity);
        }
        Ok(())
    }

    /// Called only after the SAME driver's complete source matches a private
    /// verified FINAL observation and every release preflight has succeeded.
    /// There is no await, I/O, serializer callback or fallible admission in this
    /// interval. Original child, capture, capacities and all charges stay owned.
    /// Prompt/actor/journal settlement is not changed by this evidence disposal.
    fn clear_archived_storage(&mut self) {
        fn unique<T>(storage: &mut Arc<Mutex<T>>) -> &mut T {
            archive_stdout::unique_storage(storage).expect("preflighted archive storage")
        }
        fn clear_chunk(chunk: &mut Option<StdoutChunk>) {
            let chunk = chunk.as_mut().expect("preflighted returned archive chunk");
            chunk.bytes.fill(0);
            chunk.count = 0;
            chunk.cursor = 0;
            chunk.read_uncertain = false;
            chunk.seen_accounted = false;
            chunk.operation = None;
        }
        // Fail before the FIRST mutation if an internal caller bypasses the
        // complete release preflight. Owned ring disposition is still closed.
        unique(&mut self.stderr_tail)
            .check_legacy_archive_release()
            .expect("owned stderr ring cannot enter legacy archive reset");
        self.lines.clear_archived_frames();
        self.pending_frame = None;
        self.pending_events = None;
        unique(&mut self.stdout_assembly).0.clear();
        *unique(&mut self.stdout_pending) = None;
        clear_chunk(unique(&mut self.stdout_chunk));
        clear_chunk(unique(&mut self.stderr_chunk));
        let retained = unique(&mut self.stderr_tail);
        retained.lines.clear();
        retained.partial.clear();
        retained.partial_operation = None;
        retained.stats_operation = None;
        retained.dropping = false;
        *unique(&mut self.stderr_stats) = StderrStats::default();
        self.helper_output.clear();
        self.turn_quota.clear();
        self.last_origin = None;
        // ReaderFailures, physical identities/joins, original capture operation,
        // cleanup deadline, child handles and stdin are deliberately still here.
    }

    fn encode_helper_output<W: std::io::Write>(
        &mut self,
        original: &crate::operation::Operation,
        encoder: &mut crate::failed_evidence::ArchiveEncoder<'_, W>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.visit_helper_output(original, encoder)
    }

    pub fn exit_success(&mut self) -> Result<bool, DriverError> {
        self.child
            .try_wait()
            .map(|status| status.is_some_and(|s| s.success()))
            .map_err(|e| DriverError::Died(e.to_string()))
    }

    pub fn stderr_tail(&self) -> String {
        let g = self.stderr_tail.lock().expect("stderr tail");
        g.display().unwrap_or_else(|error| error.to_string())
    }

    pub async fn kill(&mut self) -> std::io::Result<()> {
        let operation = self.capture.lock().expect("child operation").clone();
        self.kill_until(operation.allowance().deadline()).await
    }

    pub async fn kill_until(&mut self, deadline: Instant) -> std::io::Result<()> {
        let deadline = self.restrict_cleanup_deadline(deadline);
        self.stdin = None;
        if self.child.try_wait()?.is_none() {
            self.child.start_kill()?;
            tokio::time::timeout_at(deadline, self.child.wait())
                .await
                .map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "child retained unsettled")
                })??;
        }
        self.join_readers(deadline).await
    }
}

/// Only OS/runtime variables may be inherited by a seat process. Credential names are
/// not predictable, so a prefix deny-list cannot prove absence of ambient authority.
/// Provider/profile/compaction settings are supplied explicitly by the trusted driver.
const SEAT_RUNTIME_ENV: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "WINDIR",
    "SYSTEMDRIVE",
    "COMSPEC",
    "TEMP",
    "TMP",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
];

fn is_runtime_env_name(name: &std::ffi::OsStr) -> bool {
    // Every permitted name is ASCII. An invalid Unicode name cannot become one by
    // lossy replacement; borrowed comparisons avoid allocating an uppercase copy.
    name.to_str().is_some_and(|name| {
        SEAT_RUNTIME_ENV
            .iter()
            .any(|allowed| name.eq_ignore_ascii_case(allowed))
    })
}

fn env_name_matches(name: &std::ffi::OsStr, replacement: &str) -> bool {
    match name.to_str() {
        Some(name) => name.eq_ignore_ascii_case(replacement),
        None if replacement.is_ascii() => false,
        // Preserve the public fixture helper's legacy lossy comparison for arbitrary
        // non-ASCII override keys. Runtime trusted-driver keys are fixed ASCII.
        None => name.to_string_lossy().eq_ignore_ascii_case(replacement),
    }
}

/// The runtime allowlist plus explicit trusted-driver settings, with no ambient credentials.
pub fn seat_env(
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    set: &[(&str, OsString)],
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = parent
        .into_iter()
        .filter(|(k, _)| is_runtime_env_name(k))
        .collect();
    for (k, v) in set {
        env.retain(|(ek, _)| !env_name_matches(ek, k));
        env.push((OsString::from(k), v.clone()));
    }
    env
}

/// Apply [`seat_env`] to a command (clearing everything else).
pub fn apply_seat_env(cmd: &mut Command, set: &[(&str, OsString)]) {
    cmd.env_clear();
    for (k, v) in seat_env(std::env::vars_os(), set) {
        cmd.env(k, v);
    }
}

/// Retained environment staging for the trusted runtime. Parent iterator values already
/// exist when yielded: their library/OS snapshot allocation is a separate fit obligation.
/// Accepted parent pairs MOVE; explicit overrides and command copies are precharged.
fn owned_seat_env(
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    set: &[(&str, OsString)],
    operation: &crate::operation::Operation,
) -> Result<Vec<(OsString, OsString)>, crate::operation::OperationError> {
    operation.check()?;
    let storage = event_storage::EventStorage(Some(operation));
    storage.charge(Some(std::mem::size_of::<Vec<(OsString, OsString)>>()))?;
    let mut env = Vec::new();
    for (key, value) in parent {
        if !is_runtime_env_name(&key) {
            continue;
        }
        storage.reserve_next(&mut env)?;
        storage.charge(
            key.as_encoded_bytes()
                .len()
                .checked_add(value.as_encoded_bytes().len())
                .and_then(|n| n.checked_add(std::mem::size_of::<(OsString, OsString)>())),
        )?;
        env.push((key, value));
    }
    for (key, value) in set {
        env.retain(|(old, _)| !env_name_matches(old, key));
        storage.reserve_next(&mut env)?;
        let key = storage.copy_os(std::ffi::OsStr::new(key))?;
        let value = storage.copy_os(value)?;
        env.push((key, value));
    }
    operation.check()?;
    Ok(env)
}

fn apply_seat_env_owned(
    cmd: &mut Command,
    set: &[(&str, OsString)],
    operation: &crate::operation::Operation,
) -> Result<(), crate::operation::OperationError> {
    operation.check()?; // Before the ambient library snapshot is requested.
    let env = owned_seat_env(std::env::vars_os(), set, operation)?;
    apply_owned_env_values(cmd, &env, operation)
}

fn apply_owned_env_values(
    cmd: &mut Command,
    env: &[(OsString, OsString)],
    operation: &crate::operation::Operation,
) -> Result<(), crate::operation::OperationError> {
    operation.check()?;
    let storage = event_storage::EventStorage(Some(operation));
    // Admit ALL explicit command-input copies before clearing/mutating this local command.
    // Command map nodes, OS conversion/spawn buffers and allocator excess stay unproved.
    for (key, value) in env {
        storage.charge(
            key.as_encoded_bytes()
                .len()
                .checked_add(value.as_encoded_bytes().len())
                .and_then(|n| n.checked_add(std::mem::size_of::<(OsString, OsString)>())),
        )?;
    }
    operation.check()?;
    cmd.env_clear();
    for (key, value) in env {
        cmd.env(key, value);
    }
    Ok(())
}

fn append_helper_frame(
    output: &mut String,
    pending: &mut Option<ChildFrame>,
    original: &crate::operation::Operation,
    line: &str,
) -> Result<(), DriverError> {
    let frame = pending
        .as_ref()
        .ok_or_else(|| DriverError::Protocol("no retained helper frame".into()))?;
    if !frame.operation.same_instance(original) {
        return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
            "helper frame belongs to another original operation instance".into(),
        )));
    }
    if frame.text != line {
        return Err(DriverError::Protocol(
            "helper output does not match its retained frame".into(),
        ));
    }
    let storage = event_storage::EventStorage(Some(&frame.operation));
    let bytes = line.len().checked_add(1);
    storage
        .charge(bytes)
        .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
    storage
        .reserve_text_extension(output, bytes.expect("admitted helper suffix"))
        .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
    output.push_str(line);
    output.push('\n');
    // Only accepted copying into the retained canonical prefix disposes of this raw
    // frame. Allocation/identity refusal leaves BOTH prefix and SAME raw in service.
    *pending = None;
    Ok(())
}

/// Find an executable on `PATH` (honouring `PATHEXT` on Windows), preferring real executables
/// over `.cmd` shims, which mangle JSON arguments.
pub fn find_exe(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        vec![".exe".into(), ".com".into()]
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Create (and return) an empty per-seat sandbox working directory under `root`. The directory is
/// the CLI's cwd; it holds nothing, and nothing above it is ever read by the seat's tools (it has
/// none beyond the game's).
pub fn make_sandbox(root: &Path, seat_label: &str) -> std::io::Result<PathBuf> {
    let dir = root.join(seat_label.replace(['.', '/', '\\', ':'], "_"));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Drive one turn: feed CLI lines to the parser, send entries to the sink in order, and return
/// when the parser reports the turn is done. `on_init` sees the session's init event.
pub(crate) async fn pump_turn(
    proc: &mut ChildProc,
    parser: &mut dyn StreamParser,
    deadline: Instant,
    mut on_event: impl FnMut(
        &StreamEvent,
        Option<&crate::operation::CaptureOrigin>,
    ) -> Result<(), DriverError>,
) -> Result<TurnOutcome, DriverError> {
    let operation = proc.capture_operation();
    loop {
        operation
            .check()
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        if proc.pending_events.is_none() {
            let line = match tokio::select! {
                result = proc.next_line(deadline) => result,
                () = operation.wait_overflow() => return Err(DriverError::Host(crate::host::HostRefusal::CaptureOverflow)),
            } {
                Err(error) => return Err(error),
                Ok(None) => {
                    return Err(DriverError::Died(
                        proc.reader_error()?.unwrap_or_else(|| proc.stderr_tail()),
                    ));
                }
                Ok(Some(line)) => line,
            };
            drop(line); // charged transient copy; canonical raw frame stays in SAME child.
            if !proc
                .pending_frame
                .as_ref()
                .expect("owned raw frame")
                .operation
                .same_instance(&operation)
            {
                return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                    "raw frame belongs to another original operation; parser untouched".into(),
                )));
            }
            proc.parse_retained(parser)?;
        }
        while proc.pending_events.is_some() {
            proc.pending_events
                .as_ref()
                .expect("owned event batch")
                .check_operation(&operation)?;
            // Reserve the retained destination before callback acceptance/pop. Refusal
            // leaves this quota event in its original batch; callback errors keep the
            // admitted destination capacity, without refund or repeated allocation.
            admit_quota_destination(
                proc.pending_events.as_ref().expect("owned event batch"),
                &mut proc.turn_quota,
            )?;
            let Some(event) = proc.accept_event(&mut on_event)? else {
                break;
            };
            match event {
                StreamEvent::Quota(q) => proc.turn_quota.push(q),
                StreamEvent::TurnDone(mut outcome) => {
                    outcome.quota = std::mem::take(&mut proc.turn_quota);
                    return Ok(outcome);
                }
                _ => {}
            }
        }
    }
}

/// Truncate to at most `max` characters on a char boundary, appending `…` when cut.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max).collect();
        s.push('…');
        s
    }
}

/// A tool result as a transcript entry: a short summary plus the parsed detail (bounded).
pub fn tool_result_entry(call_id: String, ok: bool, text: &str) -> TranscriptEntry {
    const MAX_DETAIL_BYTES: usize = 64 * 1024;
    let detail = if text.len() > MAX_DETAIL_BYTES {
        None
    } else {
        Some(
            serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.to_string())),
        )
    };
    let mut flat = String::new();
    for word in text.split_whitespace() {
        if !flat.is_empty() {
            flat.push(' ');
        }
        for character in word.chars() {
            flat.push(character);
            if flat.chars().count() > 200 {
                break;
            }
        }
        if flat.chars().count() > 200 {
            break;
        }
    }
    TranscriptEntry::ToolResult {
        call_id,
        ok,
        summary: clip(&flat, 200),
        detail,
    }
}

struct CachedCall {
    tool: String,
    args: Value,
    operation: Option<crate::operation::Operation>,
    origin: Option<crate::operation::CaptureOrigin>,
}

// m7ea2c2e1ec6e: owner-derived lead-set structural/spare backing bound,
// NOT a proof of private BTreeMap layout. Child storage remains separate.
const PAIRING_MAP_ENTRY_BACKING_BYTES: usize = 16
    * (std::mem::size_of::<String>()
        + std::mem::size_of::<CachedCall>()
        + std::mem::size_of::<usize>());

impl CachedCall {
    /// Child copies only: the caller prepays pairing backing separately.
    /// Existing encoded/header guards stay intentionally additional.
    fn copy_owned_children(
        call_id: &str,
        tool: &str,
        args: &Value,
        operation: &crate::operation::Operation,
        origin: Option<&crate::operation::CaptureOrigin>,
    ) -> Result<(String, Self), crate::operation::OperationError> {
        let storage = event_storage::EventStorage(Some(operation));
        let call_id = storage.copy(call_id)?;
        let tool = storage.copy(tool)?;
        let args = storage.copy_value(args)?;
        // Origin's inline metadata is admitted by the existing caller. Its
        // independently owned strings use actual charged capacity before copy.
        let origin = origin
            .map(|origin| -> Result<_, crate::operation::OperationError> {
                Ok(crate::operation::CaptureOrigin {
                    operation_id: storage.copy(&origin.operation_id)?,
                    child_generation: origin.child_generation,
                    ordinal: origin.ordinal,
                    physical_child_id: storage.optional(origin.physical_child_id.as_deref())?,
                })
            })
            .transpose()?;
        operation.check()?;
        Ok((
            call_id,
            Self {
                tool,
                args,
                operation: Some(operation.clone()), // Original handle, no new meter.
                origin,
            },
        ))
    }
}

/// Sends parsed entries to the transcript and derives `decision_submitted` entries: when a
/// `submit` tool call returns successfully and was not a duplicate replay, the decision counts as
/// submitted. Works for every CLI because it only looks at paired call/result entries.
pub struct EntryEmitter {
    seat: SeatId,
    sink: crate::transcript::TranscriptSink,
    calls: BTreeMap<String, CachedCall>,
}

impl EntryEmitter {
    fn visit_pairing_archive(
        &self,
        original: &crate::operation::Operation,
        physical_child: Option<&str>,
        sink: &mut impl archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::{ArchiveError, EvidenceCategory, EvidenceRecord};
        if original
            .archive_plan()
            .is_none_or(|plan| plan.seat != self.seat)
        {
            return Err(ArchiveError::Identity);
        }
        // Validate the entire cache before emitting a component prefix. Legacy
        // unassociated calls cannot acquire an owner at archive time.
        for call in self.calls.values() {
            if call
                .operation
                .as_ref()
                .is_none_or(|op| !op.same_instance(original))
                || call
                    .origin
                    .as_ref()
                    .and_then(|o| o.physical_child_id.as_deref())
                    .is_some_and(|child| Some(child) != physical_child)
            {
                return Err(ArchiveError::Identity);
            }
            // Check every original borrowed recursive value BEFORE a component
            // prefix. Fit refusal retains this SAME pairing and its body.
            crate::failed_evidence::check_value_shape(&call.args)?;
        }
        // One canonical map, deterministically borrowed in key order; neither
        // coverage nor encoding removes a pairing or copies its JSON body.
        for (index, (call_id, call)) in self.calls.iter().enumerate() {
            let record = EvidenceRecord {
                operation: call.operation.as_ref().ok_or(ArchiveError::Identity)?,
                category: EvidenceCategory::Pairing,
                part: "unpaired_call",
                index: u64::try_from(index).map_err(|_| ArchiveError::Fit)?,
                physical_child: call
                    .origin
                    .as_ref()
                    .and_then(|o| o.physical_child_id.as_deref()),
                origin: call.origin.as_ref(),
            };
            sink.json(record, &(call_id.as_str(), call.tool.as_str(), &call.args))?;
        }
        Ok(())
    }

    pub(crate) fn has_pending_calls(&self) -> bool {
        !self.calls.is_empty()
    }
    pub fn new(seat: SeatId, sink: crate::transcript::TranscriptSink) -> Self {
        Self {
            seat,
            sink,
            calls: BTreeMap::new(),
        }
    }

    pub fn emit(&mut self, entry: TranscriptEntry) {
        let _ = self.try_emit(entry);
    }

    pub fn set_origin(
        &mut self,
        origin: Option<&crate::operation::CaptureOrigin>,
    ) -> Result<(), DriverError> {
        self.sink = self
            .sink
            .for_origin(origin)
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        Ok(())
    }

    pub fn try_emit(&mut self, entry: TranscriptEntry) -> Result<(), DriverError> {
        if let TranscriptEntry::ToolCall {
            call_id,
            tool,
            args,
        } = &entry
        {
            // Adjacent compile-time type tie: changing the map's key/value
            // types requires updating this owner bound, never a silent fallback.
            let calls: &BTreeMap<String, CachedCall> = &self.calls;
            if calls.contains_key(call_id) {
                // Reusing an identity cannot overwrite still-owned evidence.
                // This fixed refusal needs no error-text allocation after overflow.
                return Err(DriverError::Host(crate::host::HostRefusal::IdentityChanged));
            }
            // ONE new-key backing charge BEFORE ANY derived child copy or
            // insertion. Duplicate refusal precedes this; removal refunds none.
            self.sink
                .charge_allocation(PAIRING_MAP_ENTRY_BACKING_BYTES)
                .map_err(|error| DriverError::Host(crate::host::operation_refusal(error)))?;
            self.sink
                .charge_copy(&(call_id, tool, args))
                .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
            self.sink
                .admit_pairing_metadata(std::mem::size_of::<CachedCall>())
                .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        }
        if let TranscriptEntry::ToolResult { call_id, .. } = &entry {
            if let Some(call) = self.calls.get(call_id)
                && let Some(original) = call.operation.as_ref()
                && self
                    .sink
                    .pairing_association()
                    .0
                    .is_none_or(|op| !op.same_instance(original))
            {
                return Err(DriverError::Host(crate::host::HostRefusal::IdentityChanged));
            }
            // Keep the key needed AFTER batch acceptance without an uncharged clone.
            self.sink
                .charge_allocation(call_id.len().saturating_add(std::mem::size_of::<String>()))
                .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        }
        let follow_up = match &entry {
            TranscriptEntry::ToolCall { .. } => None,
            TranscriptEntry::ToolResult {
                call_id,
                ok: true,
                detail,
                ..
            } => match self.calls.get(call_id) {
                Some(CachedCall { tool, args, .. }) if tool == "submit" => {
                    let duplicate = detail
                        .as_ref()
                        .and_then(|d| d.get("duplicate"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if duplicate {
                        None
                    } else {
                        let decision_id = args["decision_id"].as_str().unwrap_or_default();
                        self.sink
                            .charge_copy(&args["action"])
                            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
                        // Count the JSON temporary without constructing it. Each clipped
                        // payload has at most 120 UTF8 characters plus the ellipsis, bounded
                        // further by the encoded byte count; prefix and metadata add below.
                        let action_bytes = crate::operation::encoded_len(
                            &args["action"],
                            crate::operation::OPERATION_BYTES,
                        )
                        .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
                        let display_bound = action_bytes.min(120 * 4) + '…'.len_utf8();
                        let copies = decision_id
                            .len()
                            .saturating_add(display_bound * 2)
                            .saturating_add("submitted ".len())
                            .saturating_add(4 * std::mem::size_of::<String>());
                        self.sink
                            .charge_allocation(copies)
                            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
                        Some(TranscriptEntry::DecisionSubmitted {
                            decision_id: decision_id.to_string(),
                            summary: format!(
                                "submitted {}",
                                clip(&args["action"].to_string(), 120)
                            ),
                        })
                    }
                }
                _ => None,
            },
            _ => None,
        };
        let cache_call = match &entry {
            TranscriptEntry::ToolCall {
                call_id,
                tool,
                args,
            } => {
                let (operation, origin) = self.sink.pairing_association();
                Some(match operation {
                    Some(operation) => {
                        CachedCall::copy_owned_children(call_id, tool, args, operation, origin)
                            .map_err(|error| {
                                DriverError::Host(crate::host::operation_refusal(error))
                            })?
                    }
                    None => (
                        call_id.clone(),
                        CachedCall {
                            tool: tool.clone(),
                            args: args.clone(),
                            operation: None,
                            origin: origin.cloned(),
                        },
                    ),
                })
            }
            _ => None,
        };
        let remove_call = match &entry {
            TranscriptEntry::ToolResult { call_id, .. } => {
                Some(match self.sink.pairing_association().0 {
                    Some(operation) => event_storage::EventStorage(Some(operation))
                        .copy(call_id)
                        .map_err(|error| {
                        DriverError::Host(crate::host::operation_refusal(error))
                    })?,
                    None => call_id.clone(),
                })
            }
            _ => None,
        };
        let mut rows = vec![entry];
        if let Some(f) = follow_up {
            rows.push(f);
        }
        self.sink
            .try_emit_batch(self.seat, rows)
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        if let Some((call_id, call)) = cache_call {
            self.calls.insert(call_id, call);
        }
        if let Some(call_id) = remove_call {
            self.calls.remove(&call_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SOURCE ONLY/UNCOMPILED/UNRUN. These synchronous defaults NEVER return
    // a future. Legacy methods panic at CONSTRUCTION, before boxing/polling.
    // No native/allocator/positive-fit/custody/healthy evidence is claimed.
    #[test]
    fn unsupported_retained_entries_are_closed_without_legacy_construction_or_slot_loss() {
        struct Unsupported {
            legacy_calls: usize,
        }
        // Explicit async_trait-compatible signatures let this control detect
        // legacy future CONSTRUCTION, which an async body counter cannot do.
        impl SeatDriver for Unsupported {
            fn kind(&self) -> CliKind {
                CliKind::Claude
            }
            fn take_captured_outcome(
                &mut self,
                _: &crate::operation::Operation,
            ) -> Result<Option<TurnOutcome>, DriverError> {
                self.legacy_calls += 1;
                panic!("legacy captured-body fallback called")
            }
            fn start<'life0, 'life1, 'async_trait>(
                &'life0 mut self,
                _: Option<&'life1 str>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<SessionInfo, DriverError>>
                        + Send
                        + 'async_trait,
                >,
            >
            where
                'life0: 'async_trait,
                'life1: 'async_trait,
                Self: 'async_trait,
            {
                self.legacy_calls += 1;
                panic!("legacy start future constructed")
            }
            fn run_turn<'life0, 'life1, 'async_trait>(
                &'life0 mut self,
                _: &'life1 str,
                _: Duration,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<TurnOutcome, DriverError>>
                        + Send
                        + 'async_trait,
                >,
            >
            where
                'life0: 'async_trait,
                'life1: 'async_trait,
                Self: 'async_trait,
            {
                self.legacy_calls += 1;
                panic!("legacy turn future constructed")
            }
            fn session_id(&self) -> Option<String> {
                None
            }
            fn is_alive(&mut self) -> bool {
                self.legacy_calls += 1;
                panic!("legacy process query called")
            }
            fn stop<'life0, 'async_trait>(
                &'life0 mut self,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<(), DriverError>> + Send + 'async_trait,
                >,
            >
            where
                'life0: 'async_trait,
                Self: 'async_trait,
            {
                self.legacy_calls += 1;
                panic!("legacy cleanup future constructed")
            }
        }
        for overflow in [false, true] {
            for foreign in [false, true] {
                for occupied in [false, true] {
                    let original = pairing_archive_operation();
                    let replacement = pairing_archive_operation();
                    assert_eq!(original.id(), replacement.id());
                    assert!(!original.same_instance(&replacement));
                    if overflow {
                        original.overflow();
                    }
                    let charged = original.captured_bytes();
                    let deadline = original.allowance().deadline();
                    let destination_original = if foreign { &replacement } else { &original };
                    let mut start: Option<Result<SessionInfo, DriverError>> = occupied
                        .then(|| Err(DriverError::Protocol("first startup refusal".into())));
                    let mut turn: Option<Result<TurnOutcome, DriverError>> =
                        occupied.then(|| Err(DriverError::Protocol("first turn refusal".into())));
                    let mut cleanup: Option<Result<(), DriverError>> = occupied
                        .then(|| Err(DriverError::Protocol("first cleanup refusal".into())));
                    let mut captured = occupied.then(|| TurnOutcome {
                        ok: false,
                        error: Some("first captured refusal".into()),
                        text: Some("first captured body".into()),
                        usage: Usage::default(),
                        quota: Vec::new(),
                    });
                    fn refusal_pointer<T>(
                        slot: &Option<Result<T, DriverError>>,
                    ) -> Option<*const u8> {
                        match slot {
                            Some(Err(DriverError::Protocol(body))) => Some(body.as_ptr()),
                            None => None,
                            _ => panic!("original refusal changed"),
                        }
                    }
                    let pointers = (
                        refusal_pointer(&start),
                        refusal_pointer(&turn),
                        refusal_pointer(&cleanup),
                        captured
                            .as_ref()
                            .map(|body| body.text.as_ref().unwrap().as_ptr()),
                    );
                    let mut unsupported = Unsupported { legacy_calls: 0 };
                    let driver: &mut dyn SeatDriver = &mut unsupported;
                    for _ in 0..2 {
                        let result = driver.start_retained(
                            &original,
                            Some("no resume effect"),
                            destination_original,
                            &mut start,
                        );
                        assert!(matches!(
                            result,
                            Err(DriverError::Host(crate::host::HostRefusal::Unknown))
                        ));
                        drop(result);
                        let result = driver.turn_retained(
                            &original,
                            "no prompt effect",
                            Duration::ZERO,
                            destination_original,
                            &mut turn,
                        );
                        assert!(matches!(
                            result,
                            Err(DriverError::Host(crate::host::HostRefusal::Unknown))
                        ));
                        drop(result);
                        let result =
                            driver.cleanup_retained(&original, destination_original, &mut cleanup);
                        assert!(matches!(
                            result,
                            Err(DriverError::Host(crate::host::HostRefusal::Unknown))
                        ));
                        drop(result);
                        assert!(matches!(
                            driver.move_captured_retained(
                                &original,
                                destination_original,
                                &mut captured
                            ),
                            Err(DriverError::Host(crate::host::HostRefusal::Unknown))
                        ));
                    }
                    assert_eq!(unsupported.legacy_calls, 0);
                    for (before, after) in [
                        (pointers.0, refusal_pointer(&start)),
                        (pointers.1, refusal_pointer(&turn)),
                        (pointers.2, refusal_pointer(&cleanup)),
                        (
                            pointers.3,
                            captured
                                .as_ref()
                                .map(|body| body.text.as_ref().unwrap().as_ptr()),
                        ),
                    ] {
                        match (before, after) {
                            (Some(before), Some(after)) => assert!(std::ptr::eq(before, after)),
                            (None, None) => {}
                            _ => panic!("unsupported entry consumed original body"),
                        }
                    }
                    assert_eq!(original.captured_bytes(), charged);
                    assert_eq!(original.allowance().deadline(), deadline);
                }
            }
        }
    }

    fn pairing_archive_operation() -> crate::operation::Operation {
        let root = std::env::temp_dir().join("synthetic-pairing-archive-no-io");
        crate::failed_evidence::CampaignArchive::new(
            &root.join("campaigns/11111111-1111-4111-8111-111111111111.sqlite"),
            &root.join("sandbox"),
            &root.join("run"),
            &[root.join("isolation")],
        )
        .unwrap()
        .operation(
            "axis.commander".parse().unwrap(),
            7,
            "22222222-2222-4222-8222-222222222222".into(),
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(60)))
                .unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn pairing_archive_borrows_accepted_calls_and_keeps_original_origin_after_overflow() {
        use crate::failed_evidence::{ArchiveEncoder, EvidenceCoverageBuilder};
        let original = pairing_archive_operation();
        let child = "33333333-3333-4333-8333-333333333333";
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached())
            .for_operation(original.clone());
        let mut emitter = EntryEmitter::new("axis.commander".parse().unwrap(), sink.clone());
        let first = original.next_child_origin(child).unwrap();
        emitter.set_origin(Some(&first)).unwrap();
        emitter
            .try_emit(submit_call("z-known", "decision-z", serde_json::json!("λ")))
            .unwrap();
        emitter.set_origin(None).unwrap();
        emitter
            .try_emit(submit_call("a-unknown", "decision-a", Value::Null))
            .unwrap();
        // A later known frame must not retag either prior cache entry.
        let later = original.next_child_origin(child).unwrap();
        emitter.set_origin(Some(&later)).unwrap();
        sink.flush_confirmed().await.unwrap();
        let pointer = emitter.calls["z-known"].args["decision_id"]
            .as_str()
            .unwrap()
            .as_ptr();
        let charged = original.captured_bytes();
        original.overflow();
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        emitter
            .visit_pairing_archive(&original, Some(child), &mut coverage)
            .unwrap();
        let coverage = coverage.finish().unwrap();
        let mut output = Vec::new(); // Synthetic reference output, no archive I/O.
        let mut encoder = ArchiveEncoder::new(&original, coverage, &mut output).unwrap();
        emitter
            .visit_pairing_archive(&original, Some(child), &mut encoder)
            .unwrap();
        let measurement = encoder.finish().unwrap();
        let records: Vec<Value> = output
            .split(|b| *b == b'\n')
            .filter(|row| !row.is_empty())
            .map(|row| serde_json::from_slice(row).unwrap())
            .collect();
        assert_eq!(records.len(), 3);
        assert_eq!(records[1]["payload"][0], "a-unknown");
        assert!(records[1]["evidence"]["origin"].is_null());
        assert!(records[1]["evidence"]["physical_child"].is_null());
        assert_eq!(records[2]["payload"][0], "z-known");
        assert_eq!(records[2]["payload"][2]["decision_id"], "decision-z");
        assert_eq!(records[2]["evidence"]["origin"]["ordinal"], first.ordinal);
        assert_ne!(first.ordinal, later.ordinal);
        original
            .archive_plan()
            .unwrap()
            .verify_stream(
                &original,
                coverage,
                measurement,
                &mut std::io::Cursor::new(&output),
            )
            .unwrap();
        assert_eq!(emitter.calls.len(), 2);
        assert_eq!(
            emitter.calls["z-known"].args["decision_id"]
                .as_str()
                .unwrap()
                .as_ptr(),
            pointer
        );
        assert!(
            emitter.calls["z-known"]
                .operation
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        assert_eq!(original.captured_bytes(), charged);
        // Fixture worker is actually joined; this never proves a CLI/reader join.
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn pairing_duplicate_and_cross_instance_result_keep_original_cache_before_delivery() {
        let original = pairing_archive_operation();
        let replacement = pairing_archive_operation(); // Same text, different instance.
        let store = crate::transcript::LocalTranscript::detached();
        let sink = TranscriptSink::new(store.clone()).for_operation(original.clone());
        let mut emitter = EntryEmitter::new("axis.commander".parse().unwrap(), sink.clone());
        emitter
            .try_emit(submit_call("call", "original", Value::Null))
            .unwrap();
        sink.flush_confirmed().await.unwrap();
        let pointer = emitter.calls["call"].args["decision_id"]
            .as_str()
            .unwrap()
            .as_ptr();
        let charged = original.captured_bytes();
        assert!(matches!(
            emitter.try_emit(submit_call("call", "replacement", Value::Null)),
            Err(DriverError::Host(crate::host::HostRefusal::IdentityChanged))
        ));
        assert_eq!(original.captured_bytes(), charged);
        emitter.sink = sink.for_operation(replacement.clone());
        let replacement_charged = replacement.captured_bytes();
        assert!(matches!(
            emitter.try_emit(submit_result("call", false)),
            Err(DriverError::Host(crate::host::HostRefusal::IdentityChanged))
        ));
        assert_eq!(replacement.captured_bytes(), replacement_charged);
        assert_eq!(
            emitter.calls["call"].args["decision_id"]
                .as_str()
                .unwrap()
                .as_ptr(),
            pointer
        );
        assert!(
            emitter.calls["call"]
                .operation
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        let mut coverage =
            crate::failed_evidence::EvidenceCoverageBuilder::new(&replacement).unwrap();
        assert!(
            emitter
                .visit_pairing_archive(&replacement, None, &mut coverage)
                .is_err()
        );
        assert!(coverage.finish().is_err()); // No components silently retagged.
        sink.flush_confirmed().await.unwrap();
        assert_eq!(store.log().len(), 1);
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn pairing_archive_rejects_unknown_owner_and_mismatched_child_without_consumption() {
        let original = pairing_archive_operation();
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let mut legacy = EntryEmitter::new("axis.commander".parse().unwrap(), sink.clone());
        legacy
            .try_emit(submit_call("legacy", "unassociated", Value::Null))
            .unwrap();
        let mut coverage = crate::failed_evidence::EvidenceCoverageBuilder::new(&original).unwrap();
        assert!(
            legacy
                .visit_pairing_archive(&original, None, &mut coverage)
                .is_err()
        );
        assert!(legacy.has_pending_calls());
        let mut emitter = EntryEmitter::new(
            "axis.commander".parse().unwrap(),
            sink.for_operation(original.clone()),
        );
        let origin = original
            .next_child_origin("33333333-3333-4333-8333-333333333333")
            .unwrap();
        emitter.set_origin(Some(&origin)).unwrap();
        emitter
            .try_emit(submit_call("call", "retained", Value::Null))
            .unwrap();
        let mut coverage = crate::failed_evidence::EvidenceCoverageBuilder::new(&original).unwrap();
        assert!(
            emitter
                .visit_pairing_archive(
                    &original,
                    Some("44444444-4444-4444-8444-444444444444"),
                    &mut coverage
                )
                .is_err()
        );
        assert!(coverage.finish().is_err());
        assert_eq!(emitter.calls["call"].origin.as_ref(), Some(&origin));
        assert!(emitter.has_pending_calls());
        sink.flush_confirmed().await.unwrap();
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn pairing_metadata_refusal_occurs_before_cache_or_transcript_acceptance() {
        let original = pairing_archive_operation();
        let store = crate::transcript::LocalTranscript::detached();
        let sink = TranscriptSink::new(store.clone()).for_operation(original.clone());
        let mut emitter = EntryEmitter::new("axis.commander".parse().unwrap(), sink.clone());
        let origin = original
            .next_child_origin("33333333-3333-4333-8333-333333333333")
            .unwrap();
        emitter.set_origin(Some(&origin)).unwrap();
        let entry = submit_call("not-admitted", "decision", Value::Null);
        let TranscriptEntry::ToolCall {
            call_id,
            tool,
            args,
        } = &entry
        else {
            unreachable!()
        };
        let body_bytes = crate::operation::encoded_len(
            &(call_id, tool, args),
            crate::operation::OPERATION_BYTES,
        )
        .unwrap();
        let remaining = body_bytes + std::mem::size_of::<CachedCall>() - 1;
        original
            .charge(crate::operation::OPERATION_BYTES - original.captured_bytes() - remaining)
            .unwrap();
        let before = original.captured_bytes();
        assert!(matches!(
            emitter.try_emit(entry),
            Err(DriverError::Host(crate::host::HostRefusal::CaptureOverflow))
        ));
        assert_eq!(original.captured_bytes(), before + body_bytes);
        assert!(!emitter.has_pending_calls());
        sink.flush_confirmed().await.unwrap();
        assert!(store.log().is_empty());
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[test]
    fn queued_stdout_archive_record_two_pass_borrows_original_fifo_and_unknown_origin() {
        use crate::failed_evidence::{ArchiveEncoder, CampaignArchive, EvidenceCoverageBuilder};
        let root = std::env::temp_dir().join("synthetic-stdout-archive-no-io");
        let operation = CampaignArchive::new(
            &root.join("campaigns/11111111-1111-4111-8111-111111111111.sqlite"),
            &root.join("sandbox"),
            &root.join("run"),
            &[root.join("isolation")],
        )
        .unwrap()
        .operation(
            "axis.commander".parse().unwrap(),
            7,
            "22222222-2222-4222-8222-222222222222".into(),
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(60)))
                .unwrap(),
        )
        .unwrap();
        let child = "33333333-3333-4333-8333-333333333333";
        let known = operation.next_child_origin(child).unwrap();
        let (tx, mut rx) = frame_queue::channel(&operation).unwrap();
        let retained = Mutex::new(None);
        let first = ChildFrame {
            text: "first canonical".into(),
            origin: None,
            operation: operation.clone(),
        };
        let pointer = first.text.as_ptr();
        enqueue_stdout_frame(&tx, &retained, first).unwrap();
        enqueue_stdout_frame(
            &tx,
            &retained,
            ChildFrame {
                text: "second canonical".into(),
                origin: Some(known),
                operation: operation.clone(),
            },
        )
        .unwrap();
        drop(tx);
        let charged = operation.captured_bytes();
        operation.overflow();
        // Storage-level borrowed fixture only: sender EOF is NOT actual child or
        // reader join proof. Production ChildProc requires those facts separately.
        let rows = rx.joined_frames().unwrap();
        let mut coverage = EvidenceCoverageBuilder::new(&operation).unwrap();
        for (index, row) in rows.iter().enumerate() {
            coverage
                .include(&queued_stdout_record(row, child, index).unwrap())
                .unwrap();
        }
        let coverage = coverage.finish().unwrap();
        let mut output = Vec::new(); // synthetic serialized reference ONLY
        let mut encoder = ArchiveEncoder::new(&operation, coverage, &mut output).unwrap();
        for (index, row) in rows.iter().enumerate() {
            encoder
                .bytes(
                    &queued_stdout_record(row, child, index).unwrap(),
                    row.text.as_bytes(),
                )
                .unwrap();
        }
        let measurement = encoder.finish().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].text.as_ptr(), pointer);
        assert!(rows[0].origin.is_none());
        assert_eq!(rows[1].origin.as_ref().unwrap().ordinal, 1);
        assert!(
            rows.iter()
                .all(|row| row.operation.same_instance(&operation))
        );
        assert_eq!(operation.captured_bytes(), charged);
        operation
            .archive_plan()
            .unwrap()
            .verify_stream(
                &operation,
                coverage,
                measurement,
                &mut std::io::Cursor::new(&output),
            )
            .unwrap();
        assert_eq!(rx.len(), 2); // successful byte comparison is not consumption/custody
    }

    fn helper_frame(text: &str, operation: &crate::operation::Operation) -> Option<ChildFrame> {
        // Recorded synthetic raw, not a kernel/read/provider attribution witness.
        Some(ChildFrame {
            text: text.into(),
            operation: operation.clone(),
            origin: None,
        })
    }

    #[test]
    fn helper_append_admits_full_replacements_and_preserves_fifo() {
        let original = emitter_operation();
        let mut output = event_storage::EventStorage(Some(&original))
            .copy("accepted prefix\n")
            .unwrap();
        let large = "λ".repeat(output.capacity() + 1);
        let mut expected = output.clone();
        for line in ["first λ", "quoted\" line", large.as_str(), ""] {
            let before = original.captured_bytes();
            let suffix = line.len() + 1;
            let required = output.len() + suffix;
            let replacement = if required > output.capacity() {
                required
            } else {
                0
            };
            let mut pending = helper_frame(line, &original);
            append_helper_frame(&mut output, &mut pending, &original, line).unwrap();
            expected.push_str(line);
            expected.push('\n');
            assert_eq!(output, expected);
            assert!(pending.is_none());
            assert_eq!(original.captured_bytes(), before + suffix + replacement);
        }
        assert_eq!(
            original.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[test]
    fn helper_replacement_refusal_keeps_prefix_and_same_raw_frame_without_ack() {
        let original = emitter_operation();
        let mut output = event_storage::EventStorage(Some(&original))
            .copy("retained accepted prefix\n")
            .unwrap();
        let accepted_pointer = output.as_ptr();
        let accepted_capacity = output.capacity();
        let line = "λ".repeat(accepted_capacity + 1);
        let suffix = line.len() + 1;
        let requested = output.len() + suffix;
        assert!(requested > accepted_capacity);
        let mut pending = helper_frame(&line, &original);
        let raw_pointer = pending.as_ref().unwrap().text.as_ptr();
        original.possible_start();
        original
            .charge(
                crate::operation::OPERATION_BYTES - original.captured_bytes() - suffix - requested
                    + 1,
            )
            .unwrap();
        let before = original.captured_bytes();
        assert!(matches!(
            append_helper_frame(&mut output, &mut pending, &original, &line),
            Err(DriverError::Host(crate::host::HostRefusal::CaptureOverflow))
        ));
        assert_eq!(output, "retained accepted prefix\n");
        assert_eq!(output.as_ptr(), accepted_pointer);
        assert_eq!(output.capacity(), accepted_capacity);
        let retained = pending.as_ref().unwrap();
        assert_eq!(retained.text.as_ptr(), raw_pointer);
        assert_eq!(retained.text, line);
        assert!(retained.operation.same_instance(&original));
        assert!(retained.origin.is_none());
        assert_eq!(original.captured_bytes(), before + suffix); // No prefix/refused charge refund.
        assert_eq!(
            original.start_evidence(),
            crate::operation::StartEvidence::Possible
        );
    }

    #[test]
    fn helper_append_rejects_reused_operation_id_and_replacement_body_before_charge() {
        let original = emitter_operation();
        let other = crate::operation::Operation::new(original.id().into(), 7, original.allowance())
            .unwrap();
        let other_baseline = other.captured_bytes();
        let mut output = "original prefix\n".to_owned();
        let pointer = output.as_ptr();
        let mut pending = helper_frame("original raw", &original);
        let raw_pointer = pending.as_ref().unwrap().text.as_ptr();
        let before = original.captured_bytes();
        assert!(matches!(
            append_helper_frame(&mut output, &mut pending, &other, "original raw"),
            Err(DriverError::Host(crate::host::HostRefusal::Ownership(_)))
        ));
        assert!(matches!(
            append_helper_frame(&mut output, &mut pending, &original, "replacement raw"),
            Err(DriverError::Protocol(_))
        ));
        assert_eq!(output, "original prefix\n");
        assert_eq!(output.as_ptr(), pointer);
        assert_eq!(pending.as_ref().unwrap().text.as_ptr(), raw_pointer);
        assert!(pending.as_ref().unwrap().operation.same_instance(&original));
        assert!(pending.as_ref().unwrap().origin.is_none());
        assert_eq!(original.captured_bytes(), before);
        assert_eq!(other.captured_bytes(), other_baseline);
        assert_eq!(
            original.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    fn quota_batch(operation: crate::operation::Operation) -> RetainedEvents {
        RetainedEvents {
            raw: ChildFrame {
                text: "original quota frame".into(),
                origin: None,
                operation,
            },
            events: VecDeque::from([StreamEvent::Quota(QuotaReading {
                window: "session".into(),
                utilization: 0.5,
                resets_at_epoch_s: None,
            })]),
        }
    }

    #[test]
    fn quota_destination_refusal_precedes_callback_and_keeps_raw_and_event() {
        let operation = emitter_operation();
        operation.possible_start();
        let mut batch = quota_batch(operation.clone());
        let pointer = batch.raw.text.as_ptr();
        let mut quota = Vec::new();
        operation
            .charge(crate::operation::OPERATION_BYTES - operation.captured_bytes() - 1)
            .unwrap();
        let mut calls = 0;
        let result = admit_quota_destination(&batch, &mut quota).and_then(|()| {
            batch.accept_front(&mut |_, _| {
                calls += 1;
                Ok(())
            })
        });
        assert!(matches!(result, Err(DriverError::Host(_))));
        assert_eq!(calls, 0);
        assert!(quota.is_empty());
        assert!(matches!(batch.events.front(), Some(StreamEvent::Quota(_))));
        assert_eq!(batch.raw.text.as_ptr(), pointer);
        assert!(batch.raw.operation.same_instance(&operation));
        assert!(batch.raw.origin.is_none());
        assert_eq!(
            operation.captured_bytes(),
            crate::operation::OPERATION_BYTES - 1
        );
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::Possible
        );
    }

    #[test]
    fn quota_callback_retry_reuses_admitted_destination_then_moves_without_copy() {
        let operation = emitter_operation();
        let mut batch = quota_batch(operation.clone());
        let mut quota = Vec::new();
        admit_quota_destination(&batch, &mut quota).unwrap();
        let charged = operation.captured_bytes();
        let destination = quota.as_ptr();
        assert!(
            batch
                .accept_front(&mut |_, _| Err(DriverError::Protocol(
                    "fixture storage refusal".into()
                )))
                .is_err()
        );
        assert_eq!(batch.events.len(), 1);
        admit_quota_destination(&batch, &mut quota).unwrap();
        assert_eq!(operation.captured_bytes(), charged);
        let mut calls = 0;
        let event = batch
            .accept_front(&mut |_, origin| {
                assert!(origin.is_none());
                calls += 1;
                Ok(())
            })
            .unwrap()
            .unwrap();
        let StreamEvent::Quota(reading) = event else {
            panic!("original quota event")
        };
        let body = reading.window.as_ptr();
        quota.push(reading);
        assert_eq!(calls, 1);
        assert!(batch.events.is_empty());
        assert_eq!(quota.as_ptr(), destination);
        assert_eq!(quota[0].window.as_ptr(), body);
        assert_eq!(operation.captured_bytes(), charged);
        assert!(batch.raw.operation.same_instance(&operation));
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[test]
    fn quota_spare_destination_cannot_bypass_closed_original_admission() {
        let operation = emitter_operation();
        let mut batch = quota_batch(operation.clone());
        let raw = batch.raw.text.as_ptr();
        let mut quota = Vec::new();
        admit_quota_destination(&batch, &mut quota).unwrap();
        let destination = quota.as_ptr();
        let capacity = quota.capacity();
        let charged = operation.captured_bytes();
        let Some(StreamEvent::Quota(reading)) = batch.events.front() else {
            panic!("original quota front")
        };
        let body = reading.window.as_ptr();
        operation.overflow();
        let mut calls = 0;
        for _ in 0..2 {
            assert!(matches!(
                admit_quota_destination(&batch, &mut quota),
                Err(DriverError::Host(crate::host::HostRefusal::CaptureOverflow))
            ));
            assert!(matches!(
                batch.accept_front(&mut |_, _| {
                    calls += 1;
                    Ok(())
                }),
                Err(DriverError::Host(crate::host::HostRefusal::CaptureOverflow))
            ));
            assert_eq!(calls, 0);
            assert!(quota.is_empty());
            assert_eq!(quota.as_ptr(), destination);
            assert_eq!(quota.capacity(), capacity);
            let Some(StreamEvent::Quota(reading)) = batch.events.front() else {
                panic!("refused quota still owned")
            };
            assert_eq!(reading.window.as_ptr(), body);
            assert_eq!(batch.events.len(), 1);
            assert_eq!(batch.raw.text.as_ptr(), raw);
            assert!(batch.raw.origin.is_none());
            assert!(batch.raw.operation.same_instance(&operation));
            assert_eq!(operation.captured_bytes(), charged);
        }
    }

    #[test]
    fn owned_parser_refusal_keeps_raw_frame_and_original_unknown_association() {
        let original = emitter_operation();
        original.possible_start();
        let text = r#"{"type":"system","subtype":"init","session_id":"unaccepted"}"#.to_owned();
        let pointer = text.as_ptr();
        let mut pending = Some(ChildFrame {
            text,
            origin: None,
            operation: original.clone(),
        });
        original
            .charge(crate::operation::OPERATION_BYTES - original.captured_bytes() - 1)
            .unwrap();
        let mut parser = claude::ClaudeParser::default();
        assert!(matches!(
            decode_pending_frame(&mut pending, &mut parser),
            Err(DriverError::Host(_))
        ));
        let retained = pending.as_ref().unwrap();
        assert_eq!(retained.text.as_ptr(), pointer);
        assert!(retained.operation.same_instance(&original));
        assert!(retained.origin.is_none());
        assert_eq!(
            original.captured_bytes(),
            crate::operation::OPERATION_BYTES - 1
        );
        assert_eq!(
            original.start_evidence(),
            crate::operation::StartEvidence::Possible
        );
        // Refusal occurs before Claude's init state changes. Pure recorded-frame behavior
        // is independently preserved; it does not grant runtime decoding admission.
        assert!(matches!(
            parser.feed(&retained.text).as_slice(),
            [StreamEvent::Init { .. }]
        ));
    }

    #[test]
    fn fixture_parser_cannot_fall_back_to_unowned_runtime_feed() {
        struct FixtureOnly;
        impl StreamParser for FixtureOnly {
            fn feed(&mut self, _: &str) -> Vec<StreamEvent> {
                panic!("runtime must not call fixture-only feed")
            }
        }
        let op = emitter_operation();
        let op_baseline = op.captured_bytes();
        let mut pending = Some(ChildFrame {
            text: "{}".into(),
            origin: None,
            operation: op.clone(),
        });
        assert!(matches!(
            decode_pending_frame(&mut pending, &mut FixtureOnly),
            Err(DriverError::Protocol(_))
        ));
        assert!(pending.as_ref().unwrap().operation.same_instance(&op));
        assert_eq!(op.captured_bytes(), op_baseline);
        assert_eq!(
            op.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    fn emitter_operation() -> crate::operation::Operation {
        crate::operation::Operation::new(
            "emitter-original".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap()
    }

    fn submit_call(call_id: &str, decision_id: &str, action: Value) -> TranscriptEntry {
        TranscriptEntry::ToolCall {
            call_id: call_id.into(),
            tool: "submit".into(),
            args: serde_json::json!({"decision_id": decision_id, "action": action}),
        }
    }

    fn submit_result(call_id: &str, duplicate: bool) -> TranscriptEntry {
        TranscriptEntry::ToolResult {
            call_id: call_id.into(),
            ok: true,
            summary: "accepted".into(),
            detail: Some(serde_json::json!({"duplicate": duplicate})),
        }
    }

    #[tokio::test]
    async fn emitter_call_copy_refusal_never_inserts_pairing_or_capture() {
        let operation = emitter_operation();
        operation.charge(crate::operation::OPERATION_BYTES).unwrap();
        let store = crate::transcript::LocalTranscript::detached();
        let sink = TranscriptSink::new(store.clone()).for_operation(operation.clone());
        let mut emitter = EntryEmitter::new("axis.commander".parse().unwrap(), sink.clone());
        assert!(
            emitter
                .try_emit(submit_call("call", "decision", Value::Null))
                .is_err()
        );
        assert!(!emitter.has_pending_calls());
        sink.flush_confirmed().await.unwrap();
        assert!(store.log().is_empty());
        assert!(sink.stop_delivery().await.is_empty()); // Actual fixture worker join.
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[tokio::test]
    async fn emitter_derived_copy_refusal_keeps_original_pair_and_accepted_call() {
        let operation = emitter_operation();
        let store = crate::transcript::LocalTranscript::detached();
        let sink = TranscriptSink::new(store.clone()).for_operation(operation.clone());
        let mut emitter = EntryEmitter::new("axis.commander".parse().unwrap(), sink.clone());
        emitter
            .try_emit(submit_call("call", "original-decision", Value::Null))
            .unwrap();
        sink.flush_confirmed().await.unwrap();
        let headroom = "call".len() + std::mem::size_of::<String>() + "null".len() + 1;
        operation
            .charge(crate::operation::OPERATION_BYTES - operation.captured_bytes() - headroom)
            .unwrap();
        assert!(emitter.try_emit(submit_result("call", false)).is_err());
        assert_eq!(
            emitter.calls["call"].args["decision_id"],
            "original-decision"
        );
        assert!(emitter.has_pending_calls());
        sink.flush_confirmed().await.unwrap();
        let rows = store.log();
        assert_eq!(rows.len(), 1);
        assert!(
            matches!(&rows[0].entry, TranscriptEntry::ToolCall { call_id, .. } if call_id == "call")
        );
        assert_eq!(
            operation.check(),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn emitter_multibyte_submission_is_fifo_and_duplicate_has_no_followup() {
        let operation = emitter_operation();
        let store = crate::transcript::LocalTranscript::detached();
        let sink = TranscriptSink::new(store.clone()).for_operation(operation.clone());
        let mut emitter = EntryEmitter::new("axis.commander".parse().unwrap(), sink.clone());
        let action = serde_json::json!({"text": "😀\n\"".repeat(130)});
        emitter
            .try_emit(submit_call("first", "original", action.clone()))
            .unwrap();
        let before_result = operation.captured_bytes();
        emitter.try_emit(submit_result("first", false)).unwrap();
        assert!(operation.captured_bytes() > before_result);
        assert!(!emitter.has_pending_calls());
        emitter
            .try_emit(submit_call("duplicate", "original", action.clone()))
            .unwrap();
        emitter.try_emit(submit_result("duplicate", true)).unwrap();
        assert!(!emitter.has_pending_calls());
        sink.flush_confirmed().await.unwrap();
        let rows = store.log();
        assert_eq!(rows.len(), 5);
        assert!(
            matches!(&rows[0].entry, TranscriptEntry::ToolCall { call_id, .. } if call_id == "first")
        );
        assert!(
            matches!(&rows[1].entry, TranscriptEntry::ToolResult { call_id, .. } if call_id == "first")
        );
        assert_eq!(
            rows[2].entry,
            TranscriptEntry::DecisionSubmitted {
                decision_id: "original".into(),
                summary: format!("submitted {}", clip(&action.to_string(), 120)),
            }
        );
        assert!(
            matches!(&rows[3].entry, TranscriptEntry::ToolCall { call_id, .. } if call_id == "duplicate")
        );
        assert!(
            matches!(&rows[4].entry, TranscriptEntry::ToolResult { call_id, .. } if call_id == "duplicate")
        );
        assert!(sink.stop_delivery().await.is_empty());
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[test]
    fn operation_cap_refusal_keeps_in_service_stdout_body() {
        let operation = crate::operation::Operation::new(
            "capture-cap".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let (tx, rx) = frame_queue::channel(&operation).unwrap();
        operation
            .charge(crate::operation::OPERATION_BYTES - operation.captured_bytes())
            .unwrap();
        let retained = Mutex::new(None);
        assert!(
            enqueue_stdout_frame(
                &tx,
                &retained,
                ChildFrame {
                    text: "accepted raw before envelope refusal".into(),
                    origin: None,
                    operation: operation.clone()
                }
            )
            .is_err()
        );
        assert!(rx.is_empty());
        assert_eq!(
            retained.lock().unwrap().as_ref().unwrap().text,
            "accepted raw before envelope refusal"
        );
        assert_eq!(
            operation.check(),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
    }

    #[test]
    fn refused_stdout_enqueue_retains_whole_same_frame() {
        let operation = crate::operation::Operation::new(
            "enqueue-refusal".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let (tx, rx) = frame_queue::channel(&operation).unwrap();
        drop(rx);
        let retained = Mutex::new(None);
        assert!(
            enqueue_stdout_frame(
                &tx,
                &retained,
                ChildFrame {
                    text: "whole accepted raw".into(),
                    origin: None,
                    operation
                }
            )
            .is_err()
        );
        let pending = retained.lock().unwrap();
        let frame = pending.as_ref().unwrap();
        assert_eq!(frame.text, "whole accepted raw");
        assert_eq!(frame.operation.id(), "enqueue-refusal");
        assert!(frame.origin.is_none());
    }

    #[test]
    fn frame_view_metadata_refusal_preserves_raw_frame_and_previous_origin() {
        let operation = emitter_operation();
        let frame = ChildFrame {
            text: "whole stdout body".into(),
            origin: Some(operation.next_child_origin("same-physical-child").unwrap()),
            operation: operation.clone(),
        };
        let raw = frame.text.as_ptr();
        let origin_id = frame.origin.as_ref().unwrap().operation_id.as_ptr();
        let mut last_origin = Some(operation.next_child_origin("prior-physical-child").unwrap());
        let prior = last_origin.clone();
        let prior_id = last_origin.as_ref().unwrap().operation_id.as_ptr();
        // Text alone fits the remaining credit, but the complete copied origin
        // and destination headers do not. There is no partial metadata commit.
        let body_only = frame.text.len() + std::mem::size_of::<String>();
        operation
            .charge(crate::operation::OPERATION_BYTES - operation.captured_bytes() - body_only)
            .unwrap();
        let charged = operation.captured_bytes();
        for _ in 0..2 {
            assert_eq!(
                copy_stdout_frame_view(&frame, &mut last_origin),
                Err(crate::operation::OperationError::CaptureOverflow)
            );
            assert_eq!(frame.text, "whole stdout body");
            assert_eq!(frame.text.as_ptr(), raw);
            assert_eq!(
                frame.origin.as_ref().unwrap().operation_id.as_ptr(),
                origin_id
            );
            assert_eq!(last_origin, prior);
            assert_eq!(
                last_origin.as_ref().unwrap().operation_id.as_ptr(),
                prior_id
            );
            assert!(frame.operation.same_instance(&operation));
            assert_eq!(operation.captured_bytes(), charged);
        }
    }

    #[test]
    fn exact_fit_frame_view_copies_known_origin_and_keeps_unknown_unattributed() {
        for known in [false, true] {
            let operation = emitter_operation();
            let frame = ChildFrame {
                text: "stdout \u{03bb}".into(),
                origin: known.then(|| operation.next_child_origin("original-child").unwrap()),
                operation: operation.clone(),
            };
            let raw = frame.text.as_ptr();
            let mut last_origin = Some(operation.next_child_origin("prior-child").unwrap());
            let metadata = frame.origin.as_ref().map_or(0, |origin| {
                origin.operation_id.len() + origin.physical_child_id.as_ref().unwrap().len()
            });
            let required = frame.text.len()
                + metadata
                + std::mem::size_of::<(String, Option<crate::operation::CaptureOrigin>)>();
            operation
                .charge(crate::operation::OPERATION_BYTES - operation.captured_bytes() - required)
                .unwrap();
            let text = copy_stdout_frame_view(&frame, &mut last_origin).unwrap();
            assert_eq!(text, frame.text);
            assert_ne!(text.as_ptr(), raw);
            assert_eq!(frame.text.as_ptr(), raw);
            assert_eq!(last_origin, frame.origin);
            assert!(frame.operation.same_instance(&operation));
            assert_eq!(
                operation.captured_bytes(),
                crate::operation::OPERATION_BYTES
            );
            assert_eq!(operation.check(), Ok(()));
            assert_eq!(
                operation.start_evidence(),
                crate::operation::StartEvidence::NotStarted
            );
        }
    }

    #[test]
    fn trimmed_stdout_copy_refusal_restores_whole_original_raw_assembly() {
        let operation = crate::operation::Operation::new(
            "trim-refusal".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let mut assembly = FrameAssembly(b"accepted raw\r\n".to_vec());
        let original_ptr = assembly.0.as_ptr();
        let required = b"accepted raw".len() + std::mem::size_of::<String>();
        operation
            .charge(crate::operation::OPERATION_BYTES - operation.captured_bytes() - required + 1)
            .unwrap();
        let charged = operation.captured_bytes();
        assert!(assembly.take_line(&operation).is_err());
        assert_eq!(assembly.0, b"accepted raw\r\n");
        assert_eq!(assembly.0.as_ptr(), original_ptr); // restores SAME allocation, not a copy
        assert_eq!(operation.captured_bytes(), charged);
        assert!(operation.check().is_err());
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[test]
    fn closed_trim_admission_keeps_whole_original_assembly_before_conversion() {
        for settled in [false, true] {
            let operation = emitter_operation();
            let mut assembly = FrameAssembly(b"whole raw\r\n".to_vec());
            let pointer = assembly.0.as_ptr();
            let capacity = assembly.0.capacity();
            let charged = operation.captured_bytes();
            if settled {
                operation.settle(); // fixture latch, not a physical settlement witness
            } else {
                operation.overflow();
            }
            let expected = if settled {
                crate::operation::OperationError::Settled
            } else {
                crate::operation::OperationError::CaptureOverflow
            };
            for _ in 0..2 {
                let error = assembly.take_line(&operation).unwrap_err();
                assert_eq!(error.kind(), std::io::ErrorKind::Other);
                assert_eq!(
                    error
                        .get_ref()
                        .and_then(|e| e.downcast_ref::<crate::operation::OperationError>()),
                    Some(&expected)
                );
                assert_eq!(assembly.0, b"whole raw\r\n");
                assert_eq!(assembly.0.as_ptr(), pointer);
                assert_eq!(assembly.0.capacity(), capacity);
                assert_eq!(operation.captured_bytes(), charged);
                assert_eq!(operation.check(), Err(expected));
            }
        }
    }

    #[test]
    fn trimmed_utf8_copy_uses_exact_admitted_bytes_and_preserves_interior_delimiters() {
        let operation = emitter_operation();
        let raw = "kept\n\u{03bb}\u{1f642}\r\n";
        let expected = "kept\n\u{03bb}\u{1f642}";
        let mut assembly = FrameAssembly(raw.as_bytes().to_vec());
        let charged = operation.captured_bytes();
        let line = assembly.take_line(&operation).unwrap();
        assert_eq!(line, expected);
        assert_eq!(line.capacity(), expected.len());
        assert_eq!(
            operation.captured_bytes() - charged,
            expected.len() + std::mem::size_of::<String>()
        );
        assert!(assembly.0.is_empty());
        assert_eq!(operation.check(), Ok(()));
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[test]
    fn exact_fit_trimmed_copy_survives_subsequent_frame_enqueue_refusal() {
        let operation = crate::operation::Operation::new(
            "trim-exact-fit".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let (tx, mut rx) = frame_queue::channel(&operation).unwrap();
        let required = b"kept".len() + std::mem::size_of::<String>();
        operation
            .charge(crate::operation::OPERATION_BYTES - operation.captured_bytes() - required)
            .unwrap();
        let mut assembly = FrameAssembly(b"kept\r\n".to_vec());
        let text = assembly.take_line(&operation).unwrap();
        assert_eq!(text, "kept");
        assert!(assembly.0.is_empty());
        assert_eq!(
            operation.captured_bytes(),
            crate::operation::OPERATION_BYTES
        );
        let retained = Mutex::new(None);
        assert!(
            enqueue_stdout_frame(
                &tx,
                &retained,
                ChildFrame {
                    text,
                    origin: None,
                    operation: operation.clone(),
                }
            )
            .is_err()
        );
        assert!(rx.try_recv().is_err());
        let held = retained.lock().unwrap();
        let frame = held.as_ref().unwrap();
        assert_eq!(frame.text, "kept");
        assert!(frame.operation.same_instance(&operation));
        assert!(frame.origin.is_none());
        assert_eq!(
            operation.captured_bytes(),
            crate::operation::OPERATION_BYTES
        );
    }

    #[test]
    fn invalid_utf8_and_oversize_keep_the_accepted_assembly_prefix() {
        let operation = crate::operation::Operation::new(
            "invalid-assembly".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let mut invalid = FrameAssembly(vec![0xff, b'\n']);
        assert!(invalid.take_line(&operation).is_err());
        assert_eq!(invalid.0, [0xff, b'\n']);
        let mut oversized = FrameAssembly(vec![b'a'; crate::operation::FRAME_BYTES]);
        assert!(oversized.push(b"x", &operation).is_err());
        assert_eq!(oversized.0.len(), crate::operation::FRAME_BYTES);
        assert!(oversized.0.iter().all(|b| *b == b'a'));
    }

    #[test]
    fn rebind_refuses_retained_frames_and_same_text_is_not_same_owner() {
        let make = || {
            crate::operation::Operation::new(
                "reused-text".into(),
                7,
                crate::operation::Allowance::required(Some(
                    Instant::now() + Duration::from_secs(1),
                ))
                .unwrap(),
            )
            .unwrap()
        };
        let original = make();
        let replacement = make();
        assert!(!original.same_instance(&replacement));
        let capture = Mutex::new(original.clone());
        assert!(rebind_capture(&capture, replacement.clone(), || 1).is_err());
        assert!(capture.lock().unwrap().same_instance(&original));
        original.overflow();
        rebind_capture(&capture, original.clone(), || {
            panic!("same-owner cleanup must not reacquire")
        })
        .unwrap();
        assert!(original.check().is_err()); // association/cleanup is never effect admission
        let closed = make();
        closed.settle();
        assert!(rebind_capture(&capture, closed, || 0).is_err());
        assert!(capture.lock().unwrap().same_instance(&original));
        rebind_capture(&capture, replacement.clone(), || 0).unwrap();
        assert!(capture.lock().unwrap().same_instance(&replacement));
    }

    #[tokio::test]
    async fn stdout_read_charge_failure_retains_whole_returned_chunk_and_original_owner() {
        let operation = crate::operation::Operation::new(
            "read-charge".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let mut read = StdoutRead::begin(slot.clone(), operation.clone()).unwrap();
        let mut source = std::io::Cursor::new(b"original returned bytes\n".as_slice());
        let count = source.read(read.buffer()).await.unwrap();
        read.returned(count);
        operation.overflow();
        assert!(operation.charge(count).is_err());
        drop(read);
        let held = slot.lock().unwrap();
        let chunk = held.as_ref().unwrap();
        assert_eq!(&chunk.bytes[..chunk.count], b"original returned bytes\n");
        assert_eq!(chunk.cursor, 0);
        assert!(!chunk.read_uncertain);
        assert!(chunk.has_pending());
        assert!(chunk.operation.as_ref().unwrap().same_instance(&operation));
        drop(held);
        assert!(StdoutRead::begin(slot.clone(), operation).is_err());
        assert_eq!(slot.lock().unwrap().as_ref().unwrap().cursor, 0);
    }

    #[test]
    fn stdout_assembly_overflow_retains_original_chunk_without_partial_acceptance() {
        let operation = crate::operation::Operation::new(
            "read-assembly".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let mut read = StdoutRead::begin(slot.clone(), operation.clone()).unwrap();
        read.buffer()[..6].copy_from_slice(b"whole\n");
        read.returned(6);
        let mut assembly = FrameAssembly(vec![b'x'; crate::operation::FRAME_BYTES]);
        assert!(
            assembly
                .push(&read.chunk.as_ref().unwrap().bytes[..6], &operation)
                .is_err()
        );
        drop(read);
        let held = slot.lock().unwrap();
        let chunk = held.as_ref().unwrap();
        assert_eq!(&chunk.bytes[..chunk.count], b"whole\n");
        assert_eq!(chunk.cursor, 0);
        assert!(chunk.has_pending());
        assert_eq!(assembly.0.len(), crate::operation::FRAME_BYTES);
    }

    #[test]
    fn stdout_assembly_admits_growth_once_and_never_grows_on_sticky_refusal() {
        let operation = crate::operation::Operation::new(
            "assembly-backing".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let baseline = operation.captured_bytes();
        let mut assembly = FrameAssembly(Vec::new());
        assembly.push(b"abc", &operation).unwrap();
        assert_eq!(
            operation.captured_bytes() - baseline,
            2 * 4 + std::mem::size_of::<Vec<u8>>()
        );
        assert!((4..=8).contains(&assembly.0.capacity()));
        let pointer = assembly.0.as_ptr();
        let capacity = assembly.0.capacity();
        let charged = operation.captured_bytes();
        assembly.push(b"d", &operation).unwrap();
        assert_eq!(assembly.0, b"abcd");
        assert_eq!(assembly.0.as_ptr(), pointer);
        assert_eq!(assembly.0.capacity(), capacity);
        assert_eq!(operation.captured_bytes(), charged);

        let needed = capacity + 1;
        let suffix = vec![b'x'; needed - assembly.0.len()];
        let target = needed.next_power_of_two();
        assembly.push(&suffix, &operation).unwrap();
        assert_eq!(
            operation.captured_bytes() - charged,
            2 * target + capacity + std::mem::size_of::<Vec<u8>>()
        );
        assert_eq!(&assembly.0[..4], b"abcd");
        assert_eq!(&assembly.0[4..], suffix);
        let pointer = assembly.0.as_ptr();
        let capacity = assembly.0.capacity();
        let charged = operation.captured_bytes();
        operation.overflow();
        assert_eq!(
            assembly.push(b"z", &operation),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert_eq!(assembly.0.len(), needed);
        assert_eq!(&assembly.0[..4], b"abcd");
        assert_eq!(&assembly.0[4..], suffix);
        assert_eq!(assembly.0.as_ptr(), pointer);
        assert_eq!(assembly.0.capacity(), capacity);
        assert_eq!(operation.captured_bytes(), charged);
    }

    #[test]
    fn stdout_growth_refusal_keeps_original_prefix_returned_read_and_rebind_liability() {
        let make = || {
            crate::operation::Operation::new(
                "same-visible-assembly-id".into(),
                7,
                crate::operation::Allowance::required(Some(
                    Instant::now() + Duration::from_secs(1),
                ))
                .unwrap(),
            )
            .unwrap()
        };
        let original = make();
        let replacement = make();
        assert!(!original.same_instance(&replacement));
        let replacement_baseline = replacement.captured_bytes();
        let capture = Mutex::new(original.clone());
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let mut read = StdoutRead::begin(slot.clone(), original.clone()).unwrap();
        read.buffer()[..6].copy_from_slice(b"whole\n");
        read.returned(6);
        let mut assembly = FrameAssembly(b"old".to_vec());
        let pointer = assembly.0.as_ptr();
        let capacity = assembly.0.capacity();
        original
            .charge(crate::operation::OPERATION_BYTES - original.captured_bytes())
            .unwrap();
        let charged = original.captured_bytes();
        assert_eq!(
            assembly.push(&read.chunk.as_ref().unwrap().bytes[..6], &original),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert_eq!(assembly.0, b"old");
        assert_eq!(assembly.0.as_ptr(), pointer);
        assert_eq!(assembly.0.capacity(), capacity);
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(replacement.captured_bytes(), replacement_baseline);
        drop(read); // SAME returned raw chunk, not an ACK or a discard.
        {
            let held = slot.lock().unwrap();
            let chunk = held.as_ref().unwrap();
            assert_eq!(&chunk.bytes[..chunk.count], b"whole\n");
            assert_eq!(chunk.cursor, 0);
            assert!(!chunk.read_uncertain);
            assert!(chunk.has_pending());
            assert!(chunk.operation.as_ref().unwrap().same_instance(&original));
        }
        assert!(
            rebind_capture(&capture, replacement.clone(), || {
                assembly.0.len() + usize::from(stdout_chunk_owned(&slot))
            })
            .is_err()
        );
        assert!(capture.lock().unwrap().same_instance(&original));
        assert_eq!(replacement.captured_bytes(), replacement_baseline);
        assert_eq!(original.captured_bytes(), charged);
    }

    #[test]
    fn cleanup_deadline_pins_original_and_never_renews_across_retry_or_new_context() {
        let now = Instant::now();
        let original = now + Duration::from_secs(10);
        let mut retained = None;
        assert_eq!(
            pin_cleanup_deadline(&mut retained, now + Duration::from_secs(20), original),
            original
        );
        // A replacement context's later allowance cannot lend time to this same child.
        assert_eq!(
            pin_cleanup_deadline(
                &mut retained,
                now + Duration::from_secs(30),
                now + Duration::from_secs(40)
            ),
            original
        );
        let earlier = now + Duration::from_secs(2);
        assert_eq!(
            pin_cleanup_deadline(&mut retained, earlier, original),
            earlier
        );
        assert_eq!(
            pin_cleanup_deadline(&mut retained, original, original),
            earlier
        );
        let expired = now - Duration::from_secs(1);
        assert_eq!(
            pin_cleanup_deadline(&mut retained, expired, original),
            expired
        );
        assert_eq!(
            pin_cleanup_deadline(&mut retained, original, original),
            expired
        );
    }

    #[test]
    fn expired_cleanup_bound_preserves_pending_read_and_possible_start_liability() {
        let operation = crate::operation::Operation::new(
            "cleanup-owned".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let operation_baseline = operation.captured_bytes();
        operation.charge(17).unwrap();
        operation.possible_start();
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let read = StdoutRead::begin(slot.clone(), operation.clone()).unwrap();
        let mut bound = None;
        let expired = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            pin_cleanup_deadline(&mut bound, expired, operation.allowance().deadline()),
            expired
        );
        assert!(stdout_chunk_owned(&slot));
        assert_eq!(operation.captured_bytes(), operation_baseline + 17);
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::Possible
        );
        drop(read); // guard restoration, never child/reader supervision settlement
        assert!(stdout_chunk_owned(&slot));
        let held = slot.lock().unwrap();
        assert!(held.as_ref().unwrap().read_uncertain);
        assert!(
            held.as_ref()
                .unwrap()
                .operation
                .as_ref()
                .unwrap()
                .same_instance(&operation)
        );
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::Possible
        );
        assert_eq!(operation.captured_bytes(), operation_baseline + 17);
    }

    #[test]
    fn stderr_charge_failure_keeps_accepted_prefix_and_original_returned_suffix() {
        let operation = crate::operation::Operation::new(
            "stderr-prefix".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        operation
            .charge(crate::operation::OPERATION_BYTES - operation.captured_bytes() - 3)
            .unwrap();
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let mut read = StdoutRead::begin(slot.clone(), operation.clone()).unwrap();
        read.buffer()[..6].copy_from_slice(b"a\nxyz\n");
        read.returned(6);
        let mut retained = StderrCapture::new();
        let mut stats = StderrStats::default();
        assert!(retained.accept_read(&mut read, &mut stats).is_err());
        assert_eq!(stats.seen_bytes, 6);
        assert_eq!(stats.retained_bytes, 3);
        assert_eq!(stats.discarded_bytes, 0);
        assert!(!stats.truncated);
        assert_eq!(retained.lines.front().unwrap().bytes, b"a\n");
        assert!(
            retained
                .lines
                .front()
                .unwrap()
                .operation
                .same_instance(&operation)
        );
        assert_eq!(retained.partial, b"x");
        assert!(
            retained
                .partial_operation
                .as_ref()
                .unwrap()
                .same_instance(&operation)
        );
        // A repeated unsuccessful observation neither recounts the read nor advances it.
        assert!(retained.accept_read(&mut read, &mut stats).is_err());
        assert_eq!(stats.seen_bytes, 6);
        drop(read);
        let held = slot.lock().unwrap();
        let chunk = held.as_ref().unwrap();
        assert_eq!(chunk.cursor, 3);
        assert_eq!(&chunk.bytes[chunk.cursor..chunk.count], b"yz\n");
        assert!(chunk.operation.as_ref().unwrap().same_instance(&operation));
        assert!(!chunk.read_uncertain);
        drop(held);
        assert!(stdout_chunk_owned(&slot));
        assert_eq!(
            operation.captured_bytes(),
            crate::operation::OPERATION_BYTES
        );
    }

    #[test]
    fn stderr_truncation_counts_only_non_answer_suffix_at_original_line_limit() {
        let operation = crate::operation::Operation::new(
            "stderr-lines".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let operation_baseline = operation.captured_bytes();
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let mut read = StdoutRead::begin(slot.clone(), operation.clone()).unwrap();
        let mut bytes = b"x\n".repeat(crate::operation::STDERR_LINES);
        bytes.extend_from_slice(b"DROP\n");
        read.buffer()[..bytes.len()].copy_from_slice(&bytes);
        read.returned(bytes.len());
        let mut retained = StderrCapture::new();
        let mut stats = StderrStats::default();
        retained.accept_read(&mut read, &mut stats).unwrap();
        assert_eq!(retained.lines.len(), crate::operation::STDERR_LINES);
        assert!(retained.partial.is_empty());
        assert_eq!(stats.seen_bytes as usize, bytes.len());
        assert_eq!(stats.retained_bytes, crate::operation::STDERR_LINES * 2);
        assert_eq!(stats.discarded_bytes, 5);
        assert!(stats.truncated);
        assert_eq!(
            operation.captured_bytes(),
            operation_baseline + stats.retained_bytes
        );
        drop(read);
        assert!(!stdout_chunk_owned(&slot));
    }

    #[test]
    fn stderr_byte_limit_retains_raw_prefix_and_counts_whole_returned_suffix() {
        let operation = crate::operation::Operation::new(
            "stderr-bytes".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let operation_baseline = operation.captured_bytes();
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let mut retained = StderrCapture::new();
        let mut stats = StderrStats::default();
        for _ in 0..crate::operation::STDERR_BYTES / crate::operation::STDOUT_CHUNK_BYTES {
            let mut read = StdoutRead::begin(slot.clone(), operation.clone()).unwrap();
            read.buffer().fill(b'x');
            read.returned(crate::operation::STDOUT_CHUNK_BYTES);
            retained.accept_read(&mut read, &mut stats).unwrap();
        }
        let mut read = StdoutRead::begin(slot.clone(), operation.clone()).unwrap();
        read.buffer()[..8].copy_from_slice(b"gone\nend");
        read.returned(8);
        retained.accept_read(&mut read, &mut stats).unwrap();
        assert_eq!(
            stats.seen_bytes as usize,
            crate::operation::STDERR_BYTES + 8
        );
        assert_eq!(stats.retained_bytes, crate::operation::STDERR_BYTES);
        assert_eq!(stats.discarded_bytes, 8);
        assert!(stats.truncated);
        assert_eq!(
            retained.lines.front().unwrap().bytes.len(),
            crate::operation::STDERR_BYTES
        );
        assert!(retained.partial.is_empty());
        assert_eq!(
            operation.captured_bytes(),
            operation_baseline + crate::operation::STDERR_BYTES
        );
        drop(read);
        assert!(!stdout_chunk_owned(&slot));
    }

    #[test]
    fn stderr_display_oversize_refuses_without_discarding_raw_or_fabricating_truncation() {
        let operation = crate::operation::Operation::new(
            "stderr-display-fit".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let mut retained = StderrCapture::new();
        let mut stats = StderrStats::default();
        for _ in 0..crate::operation::STDERR_BYTES / crate::operation::STDOUT_CHUNK_BYTES {
            let mut read = StdoutRead::begin(slot.clone(), operation.clone()).unwrap();
            read.buffer().fill(0xff);
            read.returned(crate::operation::STDOUT_CHUNK_BYTES);
            retained.accept_read(&mut read, &mut stats).unwrap();
        }
        retained.finish_partial().unwrap();
        assert_eq!(stats.seen_bytes as usize, crate::operation::STDERR_BYTES);
        assert_eq!(stats.discarded_bytes, 0);
        assert!(!stats.truncated);
        let before_display = operation.captured_bytes();
        assert!(retained.display().is_err());
        assert!(operation.captured_bytes() > before_display); // no refund of admitted copies
        assert!(operation.check().is_err());
        assert_eq!(
            retained.lines.front().unwrap().bytes.len(),
            crate::operation::STDERR_BYTES
        );
        assert!(
            retained
                .lines
                .front()
                .unwrap()
                .bytes
                .iter()
                .all(|byte| *byte == 0xff)
        );
        assert_eq!(stats.discarded_bytes, 0);
        assert!(!stats.truncated);
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[test]
    fn stderr_eof_moves_invalid_partial_without_rebind_or_raw_loss() {
        let make = || {
            crate::operation::Operation::new(
                "stderr-same-id".into(),
                7,
                crate::operation::Allowance::required(Some(
                    Instant::now() + Duration::from_secs(1),
                ))
                .unwrap(),
            )
            .unwrap()
        };
        let original = make();
        let original_baseline = original.captured_bytes();
        let replacement = make();
        let replacement_baseline = replacement.captured_bytes();
        let capture = Mutex::new(original.clone());
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let mut retained = StderrCapture::new();
        let mut stats = StderrStats::default();
        let (_, mut read) = StdoutRead::begin_captured(slot.clone(), &capture).unwrap();
        read.buffer()[..2].copy_from_slice(&[0xff, b'x']);
        read.returned(2);
        retained.accept_read(&mut read, &mut stats).unwrap();
        drop(read);
        assert!(
            rebind_capture(&capture, replacement.clone(), || {
                usize::from(stdout_chunk_owned(&slot)) + usize::from(!retained.partial.is_empty())
            })
            .is_err()
        );
        let (_, mut eof) = StdoutRead::begin_captured(slot.clone(), &capture).unwrap();
        eof.returned(0); // actual synthetic EOF observation, not timer/drop inference
        retained.accept_read(&mut eof, &mut stats).unwrap();
        retained.finish_partial().unwrap();
        drop(eof);
        assert_eq!(stats.seen_bytes, 2);
        assert_eq!(stats.retained_bytes, 2);
        assert_eq!(stats.discarded_bytes, 0);
        assert!(!stats.truncated);
        assert_eq!(retained.lines.front().unwrap().bytes, [0xff, b'x']);
        assert!(
            retained
                .lines
                .front()
                .unwrap()
                .operation
                .same_instance(&original)
        );
        assert_eq!(retained.display().unwrap(), "\u{fffd}x");
        assert!(original.captured_bytes() > original_baseline + 2); // original owner's diagnostic copies
        assert_eq!(replacement.captured_bytes(), replacement_baseline);
        assert_eq!(retained.lines.front().unwrap().bytes, [0xff, b'x']);
    }

    #[test]
    fn pending_read_refuses_rebind_and_returned_frame_keeps_original_charge_owner() {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let make = || {
            crate::operation::Operation::new(
                "reused-read-id".into(),
                7,
                crate::operation::Allowance::required(Some(
                    Instant::now() + Duration::from_secs(1),
                ))
                .unwrap(),
            )
            .unwrap()
        };
        let original = make();
        let original_baseline = original.captured_bytes();
        let replacement = make();
        let replacement_baseline = replacement.captured_bytes();
        let capture = Arc::new(Mutex::new(original.clone()));
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let owned_slot = slot.clone();
        let owned_capture = capture.clone();
        let (returned, read_return) = tokio::sync::oneshot::channel::<[u8; 6]>();
        let (tx, mut rx) = frame_queue::channel(&original).unwrap();
        let retained = Mutex::new(None);
        let mut borrowed = Box::pin(async {
            let (operation, mut read) =
                StdoutRead::begin_captured(owned_slot, &owned_capture).unwrap();
            // A bounded synthetic pipe return exercises the real checkout/rebind/charge
            // and enqueue helpers. It is not a same-child kernel or delayed-frame witness.
            let bytes = read_return.await.unwrap();
            read.buffer()[..bytes.len()].copy_from_slice(&bytes);
            read.returned(bytes.len());
            operation.charge(bytes.len()).unwrap();
            let mut assembly = FrameAssembly(Vec::new());
            assembly
                .push(
                    &read.chunk.as_ref().unwrap().bytes[..bytes.len()],
                    &operation,
                )
                .unwrap();
            read.chunk.as_mut().unwrap().cursor = bytes.len();
            enqueue_stdout_frame(
                &tx,
                &retained,
                ChildFrame {
                    text: assembly.take_line(&operation).unwrap(),
                    origin: None, // unknown provenance does not change the liability owner
                    operation,
                },
            )
            .unwrap();
        });
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            borrowed.as_mut().poll(&mut context),
            Poll::Pending
        ));
        assert!(slot.lock().unwrap().is_none());
        assert!(stdout_chunk_owned(&slot));
        assert!(
            rebind_capture(&capture, replacement.clone(), || {
                usize::from(stdout_chunk_owned(&slot))
            })
            .is_err()
        );
        assert!(capture.lock().unwrap().same_instance(&original));
        assert_eq!(replacement.captured_bytes(), replacement_baseline);
        returned.send(*b"owned\n").unwrap();
        assert!(matches!(
            borrowed.as_mut().poll(&mut context),
            Poll::Ready(())
        ));
        drop(borrowed);
        let frame = rx.try_recv().unwrap();
        assert_eq!(frame.text, "owned");
        assert!(frame.origin.is_none());
        assert!(frame.operation.same_instance(&original));
        assert!(original.captured_bytes() > original_baseline + 6); // raw bytes plus retained frame/envelope
        assert_eq!(replacement.captured_bytes(), replacement_baseline);
        let held = slot.lock().unwrap();
        let chunk = held.as_ref().unwrap();
        assert_eq!((chunk.cursor, chunk.count), (6, 6));
        assert!(chunk.operation.as_ref().unwrap().same_instance(&original));
        assert!(!chunk.read_uncertain);
        drop(held);
        // The accepted queued frame still blocks the real ChildProc admission predicate.
        assert!(rebind_capture(&capture, replacement, || 1).is_err());
    }

    #[tokio::test]
    async fn cooperative_read_losing_pause_restores_original_chunk_before_ack() {
        struct PendingPipe;
        impl tokio::io::AsyncRead for PendingPipe {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                _: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Pending
            }
        }
        let original = crate::operation::Operation::new(
            "old".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(10)))
                .unwrap(),
        )
        .unwrap();
        let physical: Arc<str> = Arc::from("real-child-owner-fixture");
        let gate = original.reader_handoff();
        gate.attach(&original, &physical).unwrap();
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let used = original.captured_bytes();
        let mut pipe = PendingPipe;
        let mut held = StdoutRead::begin(slot.clone(), original.clone()).unwrap();
        gate.terminal_processed(&original).unwrap();
        let requested = gate.request(&original, &physical).unwrap();
        let future = read_or_pause(
            &mut pipe,
            &mut held,
            &original,
            &gate,
            reader_handoff::Pipe::Stdout,
        );
        assert!(std::mem::size_of_val(&future) <= reader_handoff::WAIT_BYTES);
        let PipeRead::Paused(binding) = future.await.unwrap() else {
            panic!("pending read won");
        };
        assert!(slot.lock().unwrap().is_none()); // no ACK or rebind while still borrowed
        drop(held);
        {
            let kept = slot.lock().unwrap();
            let chunk = kept.as_ref().unwrap();
            assert!(!chunk.read_uncertain && !chunk.has_pending());
            assert_eq!((chunk.count, chunk.cursor), (0, 0));
            assert!(chunk.operation.as_ref().unwrap().same_instance(&original));
            // This stack-owned inline array moves with its guard. Body bytes and
            // original association survive; pointer equality is not an invariant.
            assert_eq!(chunk.bytes, [0; crate::operation::STDOUT_CHUNK_BYTES]);
        }
        gate.acknowledge(&binding, reader_handoff::Pipe::Stdout)
            .unwrap();
        assert!(gate.requested().is_some()); // single ACK not BOTH or join
        assert_eq!(original.captured_bytes(), used);
        let _retained_request = requested; // unchanged installed token, no release
    }

    #[tokio::test]
    async fn cooperative_read_winning_after_request_keeps_full_old_raw_before_delivery() {
        struct BytesPipe;
        impl tokio::io::AsyncRead for BytesPipe {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                buffer: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                buffer.put_slice(b"late");
                std::task::Poll::Ready(Ok(()))
            }
        }
        for stream in [reader_handoff::Pipe::Stdout, reader_handoff::Pipe::Stderr] {
            let original = crate::operation::Operation::new(
                "old".into(),
                7,
                crate::operation::Allowance::required(Some(
                    Instant::now() + Duration::from_secs(10),
                ))
                .unwrap(),
            )
            .unwrap();
            let gate = original.reader_handoff();
            let physical: Arc<str> = Arc::from("child");
            gate.attach(&original, &physical).unwrap();
            gate.terminal_processed(&original).unwrap();
            let binding = gate.request(&original, &physical).unwrap();
            let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
            let used = original.captured_bytes();
            let mut pipe = BytesPipe;
            let mut held = StdoutRead::begin(slot.clone(), original.clone()).unwrap();
            assert!(
                read_or_pause(&mut pipe, &mut held, &original, &gate, stream)
                    .await
                    .is_err()
            );
            drop(held);
            let kept = slot.lock().unwrap();
            let chunk = kept.as_ref().unwrap();
            assert_eq!(&chunk.bytes[..chunk.count], b"late");
            assert_eq!(chunk.cursor, 0); // no assembly/enqueue/parser/transcript consumption
            assert!(chunk.has_pending() && !chunk.read_uncertain);
            assert!(chunk.operation.as_ref().unwrap().same_instance(&original));
            assert!(gate.acknowledge(&binding, stream).is_err());
            assert_eq!(original.captured_bytes(), used); // no copy/renewal/refund
        }
    }

    #[test]
    fn canceled_read_borrow_keeps_uncertain_original_owner_without_join_inference() {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let original = crate::operation::Operation::new(
            "read-cancel".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let owned_slot = slot.clone();
        let operation = original.clone();
        let mut borrowed = Box::pin(async move {
            let mut read = StdoutRead::begin(owned_slot, operation).unwrap();
            // Synthetic pending pipe await exercises the actual guard lifetime, not native I/O.
            let count = std::future::pending::<usize>().await;
            read.returned(count);
        });
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            borrowed.as_mut().poll(&mut context),
            Poll::Pending
        ));
        assert!(slot.lock().unwrap().is_none()); // in-service storage stays in the borrowed future
        assert!(stdout_chunk_owned(&slot));
        drop(borrowed);
        let held = slot.lock().unwrap();
        let chunk = held.as_ref().unwrap();
        assert_eq!(chunk.count, 0); // no actual read return, NOT verified zero transfer
        assert!(chunk.read_uncertain && chunk.has_pending());
        assert!(chunk.operation.as_ref().unwrap().same_instance(&original));
        assert_eq!(
            original.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[test]
    fn retained_batch_rejects_reused_operation_id_without_consuming_original_suffix() {
        let make = || {
            crate::operation::Operation::new(
                "same-id".into(),
                7,
                crate::operation::Allowance::required(Some(
                    Instant::now() + Duration::from_secs(1),
                ))
                .unwrap(),
            )
            .unwrap()
        };
        let original = make();
        let replacement = make();
        let batch = RetainedEvents {
            raw: ChildFrame {
                text: "original raw".into(),
                origin: None,
                operation: original.clone(),
            },
            events: VecDeque::from([StreamEvent::Context(17)]),
        };
        assert!(batch.check_operation(&replacement).is_err());
        assert_eq!(batch.raw.text, "original raw");
        assert!(batch.raw.origin.is_none());
        assert_eq!(batch.events.front(), Some(&StreamEvent::Context(17)));
        batch.check_operation(&original).unwrap();
    }

    #[test]
    fn dispatch_transfer_unknown_is_possible_and_verified_zero_stays_unstarted() {
        let make = |id: &str| {
            crate::operation::Operation::new(
                id.into(),
                7,
                crate::operation::Allowance::required(Some(
                    Instant::now() + Duration::from_secs(1),
                ))
                .unwrap(),
            )
            .unwrap()
        };
        let zero = make("verified-zero");
        let mut transfer = PendingDispatch {
            operation: zero.clone(),
            armed: true,
        };
        transfer.armed = false;
        drop(transfer);
        assert_eq!(
            zero.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
        let unknown = make("unknown-write");
        drop(PendingDispatch {
            operation: unknown.clone(),
            armed: true,
        });
        assert_eq!(
            unknown.start_evidence(),
            crate::operation::StartEvidence::Possible
        );
        let origin = unknown.next_origin().unwrap();
        assert!(unknown.confirm(&origin, crate::operation::ConfirmationFrame::Assistant));
        drop(PendingDispatch {
            operation: unknown.clone(),
            armed: true,
        });
        assert!(matches!(
            unknown.start_evidence(),
            crate::operation::StartEvidence::Confirmed { .. }
        ));
    }

    #[test]
    fn failed_capture_keeps_same_raw_frame_and_unaccepted_suffix_without_replay() {
        let operation = crate::operation::Operation::new(
            "retained-frame".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let origin = operation.next_origin().unwrap();
        let mut retained = RetainedEvents {
            raw: ChildFrame {
                text: "original full raw".into(),
                origin: Some(origin.clone()),
                operation: operation.clone(),
            },
            events: [StreamEvent::Context(10), StreamEvent::Context(20)].into(),
        };
        let mut accepted = Vec::new();
        retained
            .accept_front(&mut |event, observed| {
                assert_eq!(observed, Some(&origin));
                if let StreamEvent::Context(value) = event {
                    accepted.push(*value);
                }
                Ok(())
            })
            .unwrap();
        let refused = retained
            .accept_front(&mut |_, _| Err(DriverError::Protocol("injected closed capture".into())));
        assert!(refused.is_err());
        assert_eq!(accepted, [10]);
        assert_eq!(retained.raw.text, "original full raw");
        assert_eq!(retained.raw.operation.id(), "retained-frame");
        assert_eq!(retained.events.front(), Some(&StreamEvent::Context(20)));
        retained
            .accept_front(&mut |event, observed| {
                assert_eq!(observed, Some(&origin));
                if let StreamEvent::Context(value) = event {
                    accepted.push(*value);
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(accepted, [10, 20]);
        assert!(retained.events.is_empty());
    }

    #[test]
    fn accepted_prefix_closing_admission_commits_once_and_keeps_uncalled_suffix() {
        // Local closure latches only: these controls do not establish actual
        // physical settlement, child/readers joins or provider dispatch fencing.
        for settled in [false, true] {
            let operation = emitter_operation();
            operation.possible_start();
            let origin = operation.next_origin().unwrap();
            let mut retained = RetainedEvents {
                raw: ChildFrame {
                    text: "same complete frame".into(),
                    origin: Some(origin.clone()),
                    operation: operation.clone(),
                },
                events: [StreamEvent::Context(10), StreamEvent::Context(20)].into(),
            };
            let raw = retained.raw.text.as_ptr();
            let capacity = retained.events.capacity();
            let charged = operation.captured_bytes();
            let mut calls = 0;
            let accepted = retained
                .accept_front(&mut |event, observed| {
                    calls += 1;
                    assert_eq!(event, &StreamEvent::Context(10));
                    assert_eq!(observed, Some(&origin));
                    if settled {
                        operation.settle();
                    } else {
                        operation.overflow();
                    }
                    Ok(())
                })
                .unwrap();
            assert_eq!(accepted, Some(StreamEvent::Context(10)));
            let expected = if settled {
                crate::operation::OperationError::Settled
            } else {
                crate::operation::OperationError::CaptureOverflow
            };
            for _ in 0..2 {
                assert!(matches!(
                    retained.accept_front(&mut |_, _| {
                        calls += 1;
                        Ok(())
                    }),
                    Err(DriverError::Host(_))
                ));
                assert_eq!(calls, 1); // accepted prefix was never replayed
                assert_eq!(retained.events.len(), 1);
                assert_eq!(retained.events.front(), Some(&StreamEvent::Context(20)));
                assert_eq!(retained.events.capacity(), capacity);
                assert_eq!(retained.raw.text.as_ptr(), raw);
                assert_eq!(retained.raw.text, "same complete frame");
                assert_eq!(retained.raw.origin.as_ref(), Some(&origin));
                assert!(retained.raw.operation.same_instance(&operation));
                assert_eq!(operation.check(), Err(expected));
                assert_eq!(operation.captured_bytes(), charged);
                assert_eq!(
                    operation.start_evidence(),
                    crate::operation::StartEvidence::Possible
                );
            }
        }
    }

    #[test]
    fn unattributed_retained_frame_never_receives_a_current_operation_origin() {
        let operation = crate::operation::Operation::new(
            "legacy-unknown".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let mut retained = RetainedEvents {
            raw: ChildFrame {
                text: "legacy frame".into(),
                origin: None,
                operation,
            },
            events: [StreamEvent::Context(1)].into(),
        };
        retained
            .accept_front(&mut |_, observed| {
                assert!(observed.is_none());
                Ok(())
            })
            .unwrap();
        assert!(retained.raw.origin.is_none());
    }

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    #[test]
    fn owned_environment_preserves_allowlist_override_order_and_moves_parent_payload() {
        let parent = vec![
            (os("pAtH"), os("original path λ")),
            (os("SystemRoot"), os("recorded root")),
            (os("PATH"), os("second original path")),
            (os("APPDATA"), os("original profile β")),
            (os("UNRELATED_SECRET_NAME"), os("synthetic denied canary")),
            (os("OPENAI_API_KEY"), os("synthetic denied provider")),
            (os("CLAUDE_CONFIG_DIR"), os("synthetic ambient profile")),
        ];
        let profile_pointer = parent[3].1.as_encoded_bytes().as_ptr();
        let set = [
            ("path", os("trusted replacement path")),
            ("CLAUDE_CONFIG_DIR", os("trusted profile λ")),
            ("PATH", os("final replacement path")),
        ];
        let expected = seat_env(parent.clone(), &set);
        let operation = emitter_operation();
        let operation_baseline = operation.captured_bytes();
        let owned = owned_seat_env(parent, &set, &operation).unwrap();
        assert_eq!(owned, expected);
        assert_eq!(
            owned.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
            [
                os("SystemRoot"),
                os("APPDATA"),
                os("CLAUDE_CONFIG_DIR"),
                os("PATH")
            ]
        );
        assert_eq!(owned[1].1.as_encoded_bytes().as_ptr(), profile_pointer);
        assert_eq!(owned[3].1, os("final replacement path"));
        let payload = owned
            .iter()
            .map(|(k, v)| k.as_encoded_bytes().len() + v.as_encoded_bytes().len())
            .sum::<usize>();
        assert!(operation.captured_bytes() - operation_baseline >= payload);
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[test]
    fn environment_override_boundary_keeps_borrowed_settings_and_original_liability() {
        let set = [("CLAUDE_CONFIG_DIR", os("trusted profile λ"))];
        let pointer = set[0].1.as_encoded_bytes().as_ptr();
        let probe = emitter_operation();
        let probe_baseline = probe.captured_bytes();
        let expected = owned_seat_env(Vec::new(), &set, &probe).unwrap();
        let required = probe.captured_bytes() - probe_baseline;
        let fits = emitter_operation();
        fits.charge(crate::operation::OPERATION_BYTES - fits.captured_bytes() - required)
            .unwrap();
        assert_eq!(owned_seat_env(Vec::new(), &set, &fits).unwrap(), expected);
        assert_eq!(fits.captured_bytes(), crate::operation::OPERATION_BYTES);
        let refused = emitter_operation();
        refused.possible_start();
        refused
            .charge(crate::operation::OPERATION_BYTES - refused.captured_bytes() - required + 1)
            .unwrap();
        let before = refused.captured_bytes();
        assert_eq!(
            owned_seat_env(Vec::new(), &set, &refused),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert!(refused.captured_bytes() >= before);
        let charged = refused.captured_bytes();
        assert_eq!(
            owned_seat_env(Vec::new(), &set, &refused),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert_eq!(refused.captured_bytes(), charged);
        assert_eq!(set[0].1.as_encoded_bytes().as_ptr(), pointer);
        assert_eq!(
            refused.start_evidence(),
            crate::operation::StartEvidence::Possible
        );
    }

    #[test]
    fn refused_environment_copy_leaves_command_unchanged_until_all_copies_admitted() {
        let env = vec![(os("PATH"), os("recorded replacement λ"))];
        let bytes = env[0].0.as_encoded_bytes().len()
            + env[0].1.as_encoded_bytes().len()
            + std::mem::size_of::<(OsString, OsString)>();
        let refused = emitter_operation();
        refused.possible_start();
        refused
            .charge(crate::operation::OPERATION_BYTES - refused.captured_bytes() - bytes + 1)
            .unwrap();
        let before = refused.captured_bytes();
        let mut command = Command::new("never-spawned-recorded-command");
        command.env("SYNTHETIC_EXISTING_COMMAND", "keep on refusal");
        assert_eq!(
            apply_owned_env_values(&mut command, &env, &refused),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert_eq!(refused.captured_bytes(), before);
        assert_eq!(
            command.as_std().get_envs().collect::<Vec<_>>(),
            [(
                std::ffi::OsStr::new("SYNTHETIC_EXISTING_COMMAND"),
                Some(std::ffi::OsStr::new("keep on refusal"))
            )]
        );
        let fits = emitter_operation();
        fits.charge(crate::operation::OPERATION_BYTES - fits.captured_bytes() - bytes)
            .unwrap();
        apply_owned_env_values(&mut command, &env, &fits).unwrap();
        assert_eq!(fits.captured_bytes(), crate::operation::OPERATION_BYTES);
        assert_eq!(
            command.as_std().get_envs().collect::<Vec<_>>(),
            [(env[0].0.as_os_str(), Some(env[0].1.as_os_str()))]
        );
        assert_eq!(
            refused.start_evidence(),
            crate::operation::StartEvidence::Possible
        );
        // No process, reader or native/provider request is launched by this control.
    }

    #[cfg(windows)]
    #[test]
    fn environment_policy_preserves_invalid_os_values_and_legacy_lossy_override() {
        use std::os::windows::ffi::OsStringExt;
        let invalid = OsString::from_wide(&[b'x' as u16, 0xd800]);
        assert!(!is_runtime_env_name(&invalid));
        let replacement = invalid.to_string_lossy().into_owned();
        assert!(env_name_matches(&invalid, &replacement));
        let parent = vec![(os("PATH"), invalid.clone())];
        let operation = emitter_operation();
        let owned = owned_seat_env(parent.clone(), &[], &operation).unwrap();
        assert_eq!(owned, seat_env(parent, &[]));
        assert_eq!(owned[0].1, invalid); // No lossy conversion of the value/profile payload.
    }

    #[test]
    fn env_scrubbing_removes_identity_and_credentials() {
        let parent = vec![
            (os("PATH"), os("/bin")),
            (os("SystemRoot"), os("C:/Windows")),
            (os("CNA_OPERATOR_TOKEN"), os("operator-secret")),
            (os("UNRELATED_SECRET_NAME"), os("operator-secret")),
            (os("CAPABILITY"), os("operator-secret")),
            (os("AWS_SESSION_TOKEN"), os("unrelated-provider")),
            (os("HTTPS_PROXY"), os("credential-bearing-proxy")),
            (
                os("CNA_CAPABILITY_FILE"),
                os("/server/operator-capabilities.json"),
            ),
            (os("CNA_SOURCES"), os("/game-private-sources")),
            (os("ORGTREE_AGENT_TOKEN"), os("secret")),
            (os("CLAUDE_CONFIG_DIR"), os("/other")),
            (os("ClaudeCode"), os("1")),
            (os("ANTHROPIC_API_KEY"), os("k")),
            (os("OPENAI_API_KEY"), os("k")),
            (os("CODEX_HOME"), os("/x")),
            (os("GEMINI_API_KEY"), os("k")),
            (os("AI_AGENT"), os("claude-code")),
        ];
        let env = seat_env(parent, &[("CLAUDE_CONFIG_DIR", os("/seat"))]);
        let names: Vec<String> = env
            .iter()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"PATH".to_string()));
        assert!(names.contains(&"SystemRoot".to_string()));
        assert_eq!(names.len(), 3, "{names:?}");
        let dir = env.iter().find(|(k, _)| k == "CLAUDE_CONFIG_DIR").unwrap();
        assert_eq!(dir.1, os("/seat"));
    }

    #[tokio::test]
    async fn runtime_allowlist_is_applied_to_spawn_and_resume() {
        if let Ok(phase) = std::env::var("SEAT_ENV_PROBE") {
            assert!(["spawn", "resume"].contains(&phase.as_str()));
            for name in [
                "UNRELATED_SECRET_NAME",
                "CNA_OPERATOR_TOKEN",
                "AWS_SESSION_TOKEN",
                "HTTPS_PROXY",
            ] {
                assert!(
                    std::env::var_os(name).is_none(),
                    "ambient credential name survived"
                );
            }
            assert_eq!(
                std::env::var("CLAUDE_CONFIG_DIR").unwrap(),
                "fixture-own-profile"
            );
            assert_eq!(
                std::env::var("CLAUDE_CODE_DISABLE_REFUSAL_FALLBACK").unwrap(),
                "1"
            );
            assert!(std::env::vars_os().all(|(name, _)| {
                let upper = name.to_string_lossy().to_ascii_uppercase();
                SEAT_RUNTIME_ENV.contains(&upper.as_str())
                    || [
                        "SEAT_ENV_PROBE",
                        "CLAUDE_CONFIG_DIR",
                        "CLAUDE_CODE_DISABLE_REFUSAL_FALLBACK",
                    ]
                    .contains(&upper.as_str())
            }));
            return;
        }
        for phase in ["spawn", "resume"] {
            let mut cmd = tokio::process::Command::new(std::env::current_exe().unwrap());
            cmd.args([
                "--exact",
                "driver::tests::runtime_allowlist_is_applied_to_spawn_and_resume",
                "--nocapture",
            ]);
            cmd.env("UNRELATED_SECRET_NAME", "synthetic-operator-canary");
            apply_seat_env(
                &mut cmd,
                &[
                    ("SEAT_ENV_PROBE", os(phase)),
                    ("CLAUDE_CONFIG_DIR", os("fixture-own-profile")),
                    ("CLAUDE_CODE_DISABLE_REFUSAL_FALLBACK", os("1")),
                ],
            );
            #[cfg(windows)]
            cmd.creation_flags(0x08000000);
            let output = tokio::time::timeout(Duration::from_secs(10), cmd.output())
                .await
                .unwrap()
                .unwrap();
            assert!(
                output.status.success(),
                "inert child environment assertion failed: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }
    #[test]
    fn termination_probe_child() {
        if std::env::var_os("SEAT_TERMINATION_PROBE").is_some() {
            println!("termination probe ready");
            std::thread::sleep(Duration::from_secs(20));
        }
    }
    #[tokio::test]
    async fn native_child_forced_finish_confirms_exit_before_success() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args([
            "--exact",
            "driver::tests::termination_probe_child",
            "--nocapture",
        ]);
        apply_seat_env(
            &mut command,
            &[("SEAT_TERMINATION_PROBE", OsString::from("1"))],
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let operation = crate::operation::Operation::new(
            "inert-termination".into(),
            1,
            crate::operation::Allowance::required(Some(deadline)).unwrap(),
        )
        .unwrap();
        let mut child = ChildProc::spawn(&mut command, operation).unwrap();
        let mut ready = false;
        while let Ok(Some(line)) = child.next_line(deadline).await {
            child.acknowledge_line();
            if line.contains("termination probe ready") {
                ready = true;
                break;
            }
        }
        // The child ignores stdin, so idle grace must use the checked kill/wait path.
        child.finish(Duration::from_millis(10)).await.unwrap();
        assert!(!child.is_running());
        child.kill().await.unwrap(); // An already reaped process also confirms cleanly.
        assert!(ready);
    }
    #[test]
    fn tool_results_get_summary_and_detail() {
        let e = tool_result_entry("c1".into(), true, "{\n  \"a\": 1\n}");
        match e {
            TranscriptEntry::ToolResult {
                summary, detail, ..
            } => {
                assert_eq!(summary, "{ \"a\": 1 }");
                assert_eq!(detail.unwrap()["a"], 1);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn timeout_classification_and_display_keep_private_fields_private() {
        for kind in [
            TimeoutKind::TurnReservation,
            TimeoutKind::TranscriptConfirmation,
            TimeoutKind::LegacyUnclassified,
        ] {
            let mut d = TimeoutDiagnostic::legacy();
            d.kind = kind;
            d.phase = TurnPhase::Submit;
            d.phase_elapsed_ms = Some(12345);
            let error = DriverError::TimeoutAt(d.clone());
            assert!(error.is_timeout());
            assert_eq!(error.timeout_diagnostic(), Some(d));
            assert_eq!(error.to_string(), DriverError::Timeout.to_string());
            assert!(!error.to_string().contains("12345"));
            assert!(!error.to_string().contains("submit"));
        }
        assert!(DriverError::Timeout.is_timeout());
        assert_eq!(
            DriverError::Timeout.timeout_diagnostic(),
            Some(TimeoutDiagnostic::legacy())
        );
        for error in [
            DriverError::Protocol("sink failed".into()),
            DriverError::Cli("refused".into()),
            DriverError::Died("exited".into()),
        ] {
            assert!(!error.is_timeout());
            assert!(error.timeout_diagnostic().is_none());
        }
    }
    #[test]
    fn timeout_phase_requires_this_attempt_and_monotonic_boundary() {
        let start = Instant::now();
        let valid = TurnProgress {
            turn_started_at: start,
            phase_started_at: start,
            phase: TurnPhase::Submit,
        };
        let d = TimeoutDiagnostic::turn_reservation(start, 19, Some(valid));
        assert_eq!(
            (d.phase, d.phase_measurement),
            (TurnPhase::Submit, PhaseMeasurement::Available)
        );
        assert!(d.phase_elapsed_ms.is_some());
        for invalid in [
            TurnProgress {
                turn_started_at: start - Duration::from_secs(1),
                ..valid
            },
            TurnProgress {
                phase_started_at: start - Duration::from_secs(1),
                ..valid
            },
            TurnProgress {
                phase_started_at: start + Duration::from_secs(60),
                ..valid
            },
            TurnProgress {
                turn_started_at: start + Duration::from_secs(60),
                ..valid
            },
        ] {
            let d = TimeoutDiagnostic::turn_reservation(start, 19, Some(invalid));
            assert_eq!(
                (d.phase, d.phase_measurement, d.phase_elapsed_ms),
                (TurnPhase::Unknown, PhaseMeasurement::RejectedStale, None)
            );
        }
        let d = TimeoutDiagnostic::turn_reservation(start - Duration::from_secs(2), 19, None);
        assert_eq!(
            (d.phase, d.phase_measurement),
            (TurnPhase::Unknown, PhaseMeasurement::Unavailable)
        );
        assert!(d.observed_elapsed_ms.unwrap() >= 2000);
        assert_eq!(d.observed_limit_ms, Some(19)); // Measured elapsed is not the configured limit.
    }

    #[test]
    fn frame_assembly_counts_delimiter_and_rejects_whole_oversize_entry() {
        let operation = crate::operation::Operation::new(
            "assembly-fit".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(1)))
                .unwrap(),
        )
        .unwrap();
        let mut frame = FrameAssembly(Vec::new());
        frame
            .push(&vec![b'a'; crate::operation::FRAME_BYTES - 1], &operation)
            .unwrap();
        frame.push(b"\n", &operation).unwrap();
        assert_eq!(frame.0.len(), crate::operation::FRAME_BYTES);
        assert!(frame.push(b"x", &operation).is_err());
        assert_eq!(
            frame.0.len(),
            crate::operation::FRAME_BYTES,
            "refused byte never accepted"
        );
        assert_eq!(
            frame.take_line(&operation).unwrap().len(),
            crate::operation::FRAME_BYTES - 1
        );
    }
}
