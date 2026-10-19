//! Trusted launcher journal, separate from the campaign event log and seat working directories.
//! SQLite commits precede model work. In-flight reservations survive a hard crash.
use crate::budget::{Admission, RunControl, SpendBudget};
use crate::config::LaunchConfig;
use cna_core::ids::SeatId;
use cna_protocol::UsageSnapshot;
use cna_seats::{
    driver::{EstimateBound, SessionInfo, SessionTelemetry, TimeoutDiagnostic, TurnOutcome},
    mcp::ToolBudget,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct StageUsage {
    #[serde(default)]
    pub start_accounting: Option<StartTotals>,
    #[serde(default)]
    pub automatic_attempts: u64,
    #[serde(default)]
    pub automatic_completed: u64,
    pub attempts: u64,
    pub completed: u64,
    pub reported_cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
}

/// PRIVATE journal2 categories. Public attempts and all existing usage outbox rows retain
/// their pre-dispatch compatibility meaning; legacy integers are never called measured starts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartTotals {
    pub legacy_dispatch_attempts: u64,
    pub possible: u64,
    pub confirmed: u64,
    pub provisional: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartPhase {
    Authorized,
    Possible,
    Confirmed,
    SettledZero,
    SettledPossible,
    SettledConfirmed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StartOperation {
    pub id: String,
    pub revision: u64,
    pub epoch: u64,
    pub stage: String,
    pub decision: Option<(String, u32)>,
    pub phase: StartPhase,
    pub confirmation: Option<(cna_seats::operation::ConfirmationFrame, u64)>,
    #[serde(default)]
    pub interruption_reported: bool,
    /// Set WITH this operation's usage/outbox commit, independent of start classification.
    #[serde(default)]
    pub accounting_committed: bool,
}

pub struct StartAuthorization<'a> {
    pub epoch: u64,
    pub operation_id: &'a str,
}

fn migrate_starts(state: &mut JournalState) -> Result<(), String> {
    match state.version {
        1 => {
            if state.seats.values().any(|s| {
                s.start_accounting.is_some()
                    || !s.start_operations.is_empty()
                    || s.stages
                        .values()
                        .any(|stage| stage.start_accounting.is_some())
            }) {
                return Err("mixed legacy/start-accounting journal".into());
            }
            for seat in state.seats.values_mut() {
                // Carry every old pre-dispatch integer EXACTLY. Inflight is already included;
                // never count it twice, reinterpret it as a measured start, or refund it.
                seat.start_accounting = Some(StartTotals {
                    legacy_dispatch_attempts: seat.turns,
                    ..StartTotals::default()
                });
                for stage in seat.stages.values_mut() {
                    stage.start_accounting = Some(StartTotals {
                        legacy_dispatch_attempts: stage.attempts,
                        ..StartTotals::default()
                    });
                }
            }
            state.version = 2;
            Ok(())
        }
        2 | 3 => validate_start_accounting(state),
        _ => Err("unsupported or incomplete journal2 start accounting".into()),
    }
}

// Lead mfdd66541b5e0: ONE additive operator-only receipt representation. It is
// deliberately separate from model StartOperation/decision/counters/outbox.
// TEXT integers retain the full existing u64 domain without SQLite i64 casts.
const STARTUP_RECEIPT_TABLE: &str = "CREATE TABLE cli_startup_receipt (
    campaign_id TEXT NOT NULL,
    seat TEXT NOT NULL,
    epoch TEXT NOT NULL,
    operation_id TEXT NOT NULL UNIQUE,
    revision INTEGER NOT NULL CHECK(revision >= 0),
    requested_ms TEXT NOT NULL,
    reserved_ms TEXT NOT NULL,
    completed INTEGER NOT NULL CHECK(completed IN (0,1)),
    elapsed_ms TEXT,
    returned_ok INTEGER CHECK(returned_ok IN (0,1)),
    PRIMARY KEY(campaign_id,seat,epoch,operation_id),
    CHECK((completed=0 AND elapsed_ms IS NULL AND returned_ok IS NULL)
       OR (completed=1 AND elapsed_ms IS NOT NULL AND returned_ok IS NOT NULL))
)";

/// Call ONLY in the transaction that also writes the journal version/state.
/// A pre-existing table on journal2 is inconsistent, never repaired/adopted.
/// Journal3 missing its table refuses, rather than reconstructing receipts.
fn migrate_startup_receipts(db: &Connection, state: &mut JournalState) -> Result<(), String> {
    migrate_starts(state)?;
    if state.version == 2 {
        db.execute_batch(STARTUP_RECEIPT_TABLE)
            .map_err(|error| format!("private startup receipt migration failed: {error}"))?;
        state.version = 3;
    } else {
        db.prepare("SELECT campaign_id,seat,epoch,operation_id,revision,requested_ms,reserved_ms,completed,elapsed_ms,returned_ok FROM cli_startup_receipt WHERE 0")
            .map_err(|error| format!("private startup receipt schema unavailable: {error}"))?;
    }
    Ok(())
}

/// Trusted private lookup input; never serialized, included in views or supplied
/// by a seat. The future root supplies its original creation UUID, not a new one.
#[allow(dead_code)] // SOURCE assembly, not public startup/root adoption.
pub(crate) struct StartupReceiptKey<'a> {
    pub campaign_id: &'a str,
    pub seat: SeatId,
    pub epoch: u64,
    pub original: &'a cna_seats::operation::Operation,
}

/// Receipt fields alone prove neither returned body/native joins nor healthy
/// settlement. Caller must compare every expected step field and revision before
/// lost-ACK reconciliation; lookup never adopts, rewrites, refunds or dispatches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct StartupReceipt {
    pub revision: u64,
    pub requested_ms: u64,
    pub reserved_ms: u64,
    pub completed: bool,
    pub elapsed_ms: Option<u64>,
    pub returned_ok: Option<bool>,
}

#[allow(dead_code)] // Private lookup assembly remains unadopted.
fn startup_integer(text: &str) -> Result<u64, String> {
    let value: u64 = text
        .parse()
        .map_err(|_| "invalid startup receipt integer")?;
    if value.to_string() != text {
        return Err("noncanonical startup receipt integer".into());
    }
    Ok(value)
}

fn add_start_counter(value: &mut u64) -> Result<(), String> {
    *value = value.checked_add(1).ok_or("start accounting overflow")?;
    Ok(())
}

fn reserve_in_state(
    state: &mut JournalState,
    seat: SeatId,
    kind: &str,
    stage: &str,
    requested_ms: u64,
    decision: Option<(String, u32)>,
) -> Result<u64, String> {
    let limits = state.config.session.as_ref().ok_or("missing limits")?;
    let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
    if s.inflight.is_some() {
        return Err("overlapping model operations".into());
    }
    if kind == "turn" {
        if s.turns >= state.config.max_turns as u64 {
            return Err("durable turn budget exhausted".into());
        }
        if s.calls >= state.config.tool_calls {
            return Err("durable tool budget exhausted".into());
        }
    }
    let remaining = limits
        .wall_seconds
        .saturating_mul(1000)
        .saturating_sub(s.wall_millis);
    let millis = requested_ms.min(remaining);
    if millis == 0 {
        return Err("durable wall-clock budget exhausted".into());
    }
    let usage = s.stages.entry(stage.into()).or_insert_with(|| StageUsage {
        start_accounting: Some(StartTotals::default()),
        ..StageUsage::default()
    });
    if kind == "automatic" {
        usage.automatic_attempts = usage
            .automatic_attempts
            .checked_add(1)
            .ok_or("automatic attempt overflow")?;
    }
    if kind == "turn" {
        // Compatibility counts remain pre-dispatch reservations, never measured starts.
        s.turns = s.turns.checked_add(1).ok_or("turn reservation overflow")?;
        usage.attempts = usage
            .attempts
            .checked_add(1)
            .ok_or("stage attempt overflow")?;
    }
    s.wall_millis = s
        .wall_millis
        .checked_add(millis)
        .ok_or("wall reservation overflow")?;
    s.inflight = Some(Reservation {
        kind: kind.into(),
        stage: stage.into(),
        millis,
        decision,
        operation_id: None,
    });
    Ok(millis)
}

/// Called only inside a journal transaction after reserve_in_state. Authorization is a
/// provisional liability, not an observed model start or completed usage.
fn authorize_start(s: &mut SeatJournal, operation_id: &str) -> Result<(), String> {
    uuid::Uuid::parse_str(operation_id).map_err(|_| "invalid start operation identity")?;
    if s.start_operations.contains_key(operation_id) {
        return Err("start operation identity reused".into());
    }
    let r = s.inflight.as_mut().ok_or("no reserved operation")?;
    if r.kind != "turn" || r.decision.is_none() || r.operation_id.is_some() {
        return Err("start authorization requires an unbound decision reservation".into());
    }
    let stage = s.stages.get_mut(&r.stage).ok_or("missing reserved stage")?;
    add_start_counter(
        &mut s
            .start_accounting
            .as_mut()
            .ok_or("missing lifetime start accounting")?
            .provisional,
    )?;
    add_start_counter(
        &mut stage
            .start_accounting
            .as_mut()
            .ok_or("missing stage start accounting")?
            .provisional,
    )?;
    r.operation_id = Some(operation_id.into());
    s.start_operations.insert(
        operation_id.into(),
        StartOperation {
            id: operation_id.into(),
            revision: 0,
            epoch: s.epoch,
            stage: r.stage.clone(),
            decision: r.decision.clone(),
            phase: StartPhase::Authorized,
            confirmation: None,
            interruption_reported: false,
            accounting_committed: false,
        },
    );
    Ok(())
}

fn start_category(phase: &StartPhase) -> Option<usize> {
    match phase {
        StartPhase::Authorized => Some(0),
        StartPhase::Possible | StartPhase::SettledPossible => Some(1),
        StartPhase::Confirmed | StartPhase::SettledConfirmed => Some(2),
        StartPhase::SettledZero => None,
    }
}

fn validate_start_accounting(state: &JournalState) -> Result<(), String> {
    for s in state.seats.values() {
        let mut lifetime = [0u64; 3];
        let mut stages: BTreeMap<&str, [u64; 3]> = BTreeMap::new();
        for (key, record) in &s.start_operations {
            if key != &record.id
                || record.epoch != s.epoch
                || record.decision.is_none()
                || !s.stages.contains_key(&record.stage)
                || matches!(
                    record.phase,
                    StartPhase::Confirmed | StartPhase::SettledConfirmed
                ) != record.confirmation.is_some()
                || record.confirmation.is_some_and(|(_, ordinal)| ordinal == 0)
                || (record.accounting_committed
                    && !matches!(
                        record.phase,
                        StartPhase::SettledZero
                            | StartPhase::SettledPossible
                            | StartPhase::SettledConfirmed
                    ))
            {
                return Err("invalid private start operation association".into());
            }
            uuid::Uuid::parse_str(key).map_err(|_| "invalid start operation identity")?;
            if let Some(category) = start_category(&record.phase) {
                lifetime[category] = lifetime[category]
                    .checked_add(1)
                    .ok_or("lifetime start delta overflow")?;
                let stage = stages.entry(&record.stage).or_default();
                stage[category] = stage[category]
                    .checked_add(1)
                    .ok_or("stage start delta overflow")?;
            }
        }
        let totals = s
            .start_accounting
            .as_ref()
            .ok_or("missing lifetime start accounting")?;
        if [totals.provisional, totals.possible, totals.confirmed] != lifetime {
            return Err("lifetime private start delta mismatch".into());
        }
        for (name, usage) in &s.stages {
            let totals = usage
                .start_accounting
                .as_ref()
                .ok_or("missing stage start accounting")?;
            if [totals.provisional, totals.possible, totals.confirmed]
                != stages.get(name.as_str()).copied().unwrap_or_default()
            {
                return Err("stage private start delta mismatch".into());
            }
        }
    }
    Ok(())
}

fn move_start_counter(
    totals: &mut StartTotals,
    from: &StartPhase,
    to: &StartPhase,
) -> Result<(), String> {
    let old = start_category(from);
    let new = start_category(to);
    if old == new {
        return Ok(());
    }
    let source = match old {
        Some(0) => Some(&mut totals.provisional),
        Some(1) => Some(&mut totals.possible),
        Some(2) => Some(&mut totals.confirmed),
        _ => None,
    };
    if let Some(source) = source {
        *source = source
            .checked_sub(1)
            .ok_or("missing operation start delta")?;
    }
    match new {
        Some(0) => add_start_counter(&mut totals.provisional),
        Some(1) => add_start_counter(&mut totals.possible),
        Some(2) => add_start_counter(&mut totals.confirmed),
        _ => Ok(()),
    }
}

fn reconcile_start(
    s: &mut SeatJournal,
    operation_id: &str,
    expected_revision: u64,
    evidence: Option<cna_seats::operation::StartEvidence>,
    supervised: bool,
) -> Result<StartOperation, String> {
    use cna_seats::operation::StartEvidence;
    let old = s
        .start_operations
        .get(operation_id)
        .ok_or("unknown start operation")?
        .clone();
    if old.epoch != s.epoch {
        return Err("start operation epoch mismatch".into());
    }
    // A known current operation may later upgrade a settled POSSIBLE with retained evidence.
    // Repeated identical evidence is idempotent, but a stale conflicting CAS is refused.
    let settled = matches!(
        old.phase,
        StartPhase::SettledZero | StartPhase::SettledPossible | StartPhase::SettledConfirmed
    );
    let mut next = old.clone();
    match evidence {
        Some(StartEvidence::Confirmed { frame, ordinal }) => {
            if ordinal == 0 || old.phase == StartPhase::SettledZero {
                return Err("invalid confirmed start witness".into());
            }
            if next.confirmation.is_none() {
                next.confirmation = Some((frame, ordinal));
            }
            next.phase = if settled || supervised {
                StartPhase::SettledConfirmed
            } else {
                StartPhase::Confirmed
            };
        }
        Some(StartEvidence::NotStarted) if supervised && old.phase == StartPhase::Authorized => {
            // Only this operation's provisional delta can be canceled, with verified
            // no-write evidence AND actual supervision. Legacy/public totals are untouched.
            next.phase = StartPhase::SettledZero;
        }
        Some(StartEvidence::NotStarted) if !supervised => {}
        _ if matches!(
            old.phase,
            StartPhase::Authorized | StartPhase::Possible | StartPhase::SettledPossible
        ) =>
        {
            next.phase = if settled || supervised {
                StartPhase::SettledPossible
            } else {
                StartPhase::Possible
            };
        }
        _ => {
            // No later unknown/zero report downgrades previously confirmed evidence.
            if supervised && old.phase == StartPhase::Confirmed {
                next.phase = StartPhase::SettledConfirmed;
            }
        }
    }
    if next.phase == old.phase && next.confirmation == old.confirmation {
        return Ok(old);
    }
    if old.revision != expected_revision {
        return Err("start operation revision conflict".into());
    }
    // Live reconciliation must still bind the original decision reservation. Settled
    // evidence upgrades retain their original identity without borrowing a newer inflight.
    if !settled {
        let r = s
            .inflight
            .as_ref()
            .ok_or("start operation reservation missing")?;
        if r.operation_id.as_deref() != Some(operation_id)
            || r.decision != old.decision
            || r.stage != old.stage
        {
            return Err("start operation reservation mismatch".into());
        }
    }
    next.revision = old
        .revision
        .checked_add(1)
        .ok_or("start operation revision overflow")?;
    move_start_counter(
        s.start_accounting
            .as_mut()
            .ok_or("missing lifetime start accounting")?,
        &old.phase,
        &next.phase,
    )?;
    move_start_counter(
        s.stages
            .get_mut(&old.stage)
            .ok_or("missing start stage")?
            .start_accounting
            .as_mut()
            .ok_or("missing stage start accounting")?,
        &old.phase,
        &next.phase,
    )?;
    s.start_operations.insert(operation_id.into(), next.clone());
    Ok(next)
}
/// Known provider totals, with a transactionally committed transcript outbox.
/// Missing optional channels stay null until measured; partial totals are marked
/// by incomplete_turns when input, output or cost reports are unavailable.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct UsageJournal {
    pub revision: u64,
    pub acknowledged_revision: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub reported_cost_usd: Option<f64>,
    pub outbox: Vec<UsageSnapshot>,
}
fn add_channel(total: &mut Option<u64>, measured: Option<u64>) -> Result<(), String> {
    if let Some(value) = measured {
        let value = total
            .unwrap_or(0)
            .checked_add(value)
            .ok_or("usage token total overflow")?;
        if value > 9_007_199_254_740_991 {
            return Err("usage token total exceeds wire integer precision".into());
        }
        *total = Some(value);
    }
    Ok(())
}
fn migrate_usage(s: &mut SeatJournal) -> bool {
    if s.usage.is_some() {
        return false;
    }
    // Old journals discarded auxiliary channels and used zero for absent input
    // reports. Do not reconstruct measured zeros or complete totals from them.
    let interrupted = u64::from(s.inflight.as_ref().is_some_and(|r| r.kind == "turn"));
    s.incomplete_turns = s.incomplete_turns.max(s.turns.saturating_sub(interrupted));
    s.usage = Some(UsageJournal {
        reported_cost_usd: (s.reported_cost_usd > 0.).then_some(s.reported_cost_usd),
        ..UsageJournal::default()
    });
    true
}
fn uncertain_budget(s: &SeatJournal, budget: Option<&SpendBudget>) -> bool {
    match budget {
        Some(SpendBudget::ReportedUsd { .. }) => s.incomplete_turns > 0 || s.uncertain_budget_spend,
        Some(SpendBudget::Tokens { .. }) => s.incomplete_token_turns > 0,
        None => s.incomplete_turns > 0,
    }
}
fn queue_usage(s: &mut SeatJournal) -> Result<(), String> {
    let completed = s
        .stages
        .values()
        .try_fold(0u64, |sum, stage| sum.checked_add(stage.completed))
        .ok_or("usage completion total overflow")?;
    let model = s
        .session
        .as_ref()
        .and_then(|i| i.model.clone())
        .unwrap_or_else(|| s.model.clone());
    let u = s.usage.as_mut().ok_or("missing usage journal")?;
    u.revision = u.revision.checked_add(1).ok_or("usage revision overflow")?;
    u.outbox.push(UsageSnapshot {
        controller_epoch: s.epoch,
        revision: u.revision,
        provider: Some("claude-code".into()),
        model: Some(model),
        attempts: s.turns,
        completed,
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        cache_read_tokens: u.cache_read_tokens,
        cache_creation_tokens: u.cache_creation_tokens,
        reasoning_tokens: u.reasoning_tokens,
        reported_cost_usd: u.reported_cost_usd,
        incomplete_turns: s.incomplete_turns,
    });
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reservation {
    pub kind: String,
    pub stage: String,
    pub millis: u64,
    pub decision: Option<(String, u32)>,
    /// Private operation association; absent on old compatibility reservations.
    #[serde(default)]
    pub operation_id: Option<String>,
}
/// Operator-only launcher record, never included in the usage outbox or seat transcript.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeoutEvent {
    pub schema_version: u32,
    pub campaign_id: String,
    pub seat: SeatId,
    pub controller_epoch: u64,
    pub attempt: u64,
    pub reservation: Reservation,
    pub requested_turn_ms: u64,
    pub effective_reserved_ms: u64,
    pub supervisor_turn_elapsed_ms: u64,
    pub diagnostic: TimeoutDiagnostic,
}
/// Trusted supervisor measurements accompanying a returned driver timeout.
pub struct TimeoutCompletion {
    pub diagnostic: TimeoutDiagnostic,
    pub requested_turn_ms: u64,
    pub supervisor_turn_elapsed_ms: u64,
}
/// A failed completion retains diagnostic and storage failure locally; nothing was committed.
#[derive(Debug)]
pub struct CompletionError {
    pub timeout: Option<TimeoutDiagnostic>,
    pub storage: String,
}
impl std::fmt::Display for CompletionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "journal completion failed: {}", self.storage)
    }
}
impl std::error::Error for CompletionError {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeatJournal {
    #[serde(default)]
    pub start_accounting: Option<StartTotals>,
    #[serde(default)]
    pub start_operations: BTreeMap<String, StartOperation>,
    pub epoch: u64,
    pub model: String,
    pub session: Option<SessionInfo>,
    pub calls: u64,
    pub turns: u64,
    pub wall_millis: u64,
    pub recoveries: u32,
    pub incomplete_turns: u64,
    #[serde(default)]
    pub incomplete_token_turns: u64,
    #[serde(default)]
    pub uncertain_budget_spend: bool,
    #[serde(default)]
    pub admitted_estimate_usd: Option<f64>,
    #[serde(default)]
    pub admitted_tokens: Option<u64>,
    pub reported_cost_usd: f64,
    pub last_session_cost: f64,
    pub compactions: u64,
    pub telemetry: SessionTelemetry,
    pub stages: BTreeMap<String, StageUsage>,
    pub inflight: Option<Reservation>,
    #[serde(default)]
    pub usage: Option<UsageJournal>,
    #[serde(default)]
    pub timeout_events: Vec<TimeoutEvent>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalState {
    pub version: u32,
    pub campaign_id: String,
    pub config: LaunchConfig,
    pub seats: BTreeMap<SeatId, SeatJournal>,
    #[serde(default)]
    pub largest_turn_cost_usd: f64,
}
pub struct SessionJournal {
    path: PathBuf,
    db: Mutex<Connection>,
    // Held transaction in a separate DB is an OS-released exclusive launcher lease.
    _lease: Mutex<Connection>,
}

/// Private transaction candidate temporarily owns the SAME returned SessionInfo.
/// The caller's exclusive borrow lasts until synchronous restoration on Drop.
/// No session-body clone, public schema, worker or completion inference is added.
/// Fields stay private: serialization/validation only borrow the candidate;
/// nobody may remove its checked seat or replace its leased session body.
struct SessionWriteLease<'a> {
    original: &'a mut SessionInfo,
    state: JournalState,
    seat: SeatId,
    installed: bool,
}

impl<'a> SessionWriteLease<'a> {
    fn prepare(
        original: &'a mut SessionInfo,
        state: JournalState,
        seat: SeatId,
    ) -> Result<Self, String> {
        uuid::Uuid::parse_str(&original.session_id).map_err(|_| "invalid CLI session id")?;
        if !state.seats.contains_key(&seat) {
            return Err("unknown journal seat".into());
        }
        // Install the restore owner BEFORE moving a returned body. Only plain
        // moves and fixed scalar assignments follow the checked seat lookup.
        let mut lease = Self {
            original,
            state,
            seat,
            installed: false,
        };
        let candidate = lease
            .state
            .seats
            .get_mut(&seat)
            .expect("checked session seat");
        if candidate.session.as_ref().map(|info| &info.session_id)
            != Some(&lease.original.session_id)
        {
            candidate.last_session_cost = 0.;
        }
        candidate.telemetry = SessionTelemetry::default();
        let info = std::mem::replace(
            lease.original,
            SessionInfo {
                session_id: String::new(),
                model: None,
                cli_version: None,
                resumed: false,
            },
        );
        candidate.session = Some(info);
        lease.installed = true;
        Ok(lease)
    }

    /// Inline overlap only, for the future ORIGINAL creation-time plan. Does
    /// not cover input-growing journal state/JSON/SQL/allocator storage and is
    /// NOT a complete-fit witness or permission to charge after startup.
    #[allow(dead_code)] // SOURCE assembly; caller/root adoption remains closed.
    fn inline_bytes() -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<JournalState>()
            + 3 * std::mem::size_of::<SessionInfo>()
            + std::mem::size_of::<Result<Self, String>>()
            + 2 * std::mem::size_of::<Result<(), String>>()
    }
}

impl Drop for SessionWriteLease<'_> {
    fn drop(&mut self) {
        if self.installed {
            // Private invariant: prepare checked this seat and installed Some;
            // only immutable validation/serialization has used it since then.
            // Check BEFORE the plain swap; no callback/format/await afterwards.
            let info = self
                .state
                .seats
                .get_mut(&self.seat)
                .and_then(|seat| seat.session.as_mut())
                .expect("retained original session body");
            std::mem::swap(self.original, info);
        }
    }
}

impl SessionJournal {
    pub fn path(directory: &Path, id: &str) -> Result<PathBuf, String> {
        uuid::Uuid::parse_str(id).map_err(|_| "invalid campaign id")?;
        Ok(directory
            .join("session-journals")
            .join(format!("{id}.sqlite")))
    }
    fn open(path: PathBuf, create: bool) -> Result<Self, String> {
        if !create && !path.is_file() {
            return Err("no durable session journal for campaign".into());
        }
        std::fs::create_dir_all(path.parent().ok_or("journal needs parent")?)
            .map_err(|e| e.to_string())?;
        let lease =
            Connection::open(path.with_extension("lease.sqlite")).map_err(|e| e.to_string())?;
        lease
            .busy_timeout(std::time::Duration::ZERO)
            .map_err(|e| e.to_string())?;
        lease
            .execute_batch("CREATE TABLE IF NOT EXISTS lease (id INTEGER); BEGIN EXCLUSIVE;")
            .map_err(|e| format!("campaign launcher already leased or lease unavailable: {e}"))?;
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | if create {
                OpenFlags::SQLITE_OPEN_CREATE
            } else {
                OpenFlags::empty()
            };
        let db = Connection::open_with_flags(&path, flags).map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS journal (id INTEGER PRIMARY KEY CHECK(id=1), state TEXT NOT NULL);").map_err(|e|e.to_string())?;
        Ok(Self {
            path,
            db: Mutex::new(db),
            _lease: Mutex::new(lease),
        })
    }
    pub fn config_for_campaign(path: &Path) -> Result<LaunchConfig, String> {
        let directory = path.parent().ok_or("campaign needs parent")?;
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("invalid campaign file")?;
        let db = Connection::open_with_flags(
            Self::path(directory, id)?,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| e.to_string())?;
        let state = Self::read(&db)?;
        if ![1, 2, 3].contains(&state.version)
            || state.campaign_id != id
            || state.config.session.is_none()
        {
            return Err("invalid durable campaign journal".into());
        }
        state.config.validate()?;
        Ok(state.config)
    }
    pub fn create(
        directory: &Path,
        id: &str,
        config: LaunchConfig,
        epochs: &BTreeMap<SeatId, u64>,
    ) -> Result<Self, String> {
        config.validate()?;
        if config.session.is_none() {
            return Err("durable limits required".into());
        }
        let journal = Self::open(Self::path(directory, id)?, true)?;
        let seats = config
            .claude_seats()
            .map(|(seat, model)| {
                let epoch = *epochs.get(&seat).expect("configured seat epoch");
                (
                    seat,
                    SeatJournal {
                        start_accounting: Some(StartTotals::default()),
                        start_operations: BTreeMap::new(),
                        epoch,
                        model: model.into(),
                        session: None,
                        calls: 0,
                        turns: 0,
                        wall_millis: 0,
                        recoveries: 0,
                        incomplete_turns: 0,
                        incomplete_token_turns: 0,
                        uncertain_budget_spend: false,
                        admitted_estimate_usd: None,
                        admitted_tokens: None,
                        reported_cost_usd: 0.,
                        last_session_cost: 0.,
                        compactions: 0,
                        telemetry: SessionTelemetry::default(),
                        stages: BTreeMap::new(),
                        inflight: None,
                        usage: Some(UsageJournal::default()),
                        timeout_events: Vec::new(),
                    },
                )
            })
            .collect();
        let mut state = JournalState {
            version: 2,
            campaign_id: id.into(),
            config,
            seats,
            largest_turn_cost_usd: 0.,
        };
        let mut db = journal.db.lock().map_err(|error| error.to_string())?;
        let tx = db.transaction().map_err(|error| error.to_string())?;
        migrate_startup_receipts(&tx, &mut state)?;
        let encoded = serde_json::to_string(&state).map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO journal VALUES(1,?1)", params![encoded])
            .map_err(|e| format!("journal already exists or cannot be created: {e}"))?;
        tx.commit().map_err(|error| error.to_string())?;
        drop(db);
        Ok(journal)
    }
    pub fn recover(directory: &Path, id: &str) -> Result<Self, String> {
        let journal = Self::open(Self::path(directory, id)?, false)?;
        let state = journal.snapshot()?;
        if ![1, 2, 3].contains(&state.version) || state.campaign_id != id {
            return Err("journal identity/version mismatch".into());
        }
        state.config.validate()?;
        if state.config.session.is_none() {
            return Err("missing durable limits".into());
        }
        if state.seats.len() != state.config.claude_seats().count() {
            return Err("journal seat roster mismatch".into());
        }
        for (seat, model) in state.config.claude_seats() {
            let s = state.seats.get(&seat).ok_or("journal seat missing")?;
            if s.model != model
                || !s.last_session_cost.is_finite()
                || s.last_session_cost < 0.
                || !s.reported_cost_usd.is_finite()
                || s.reported_cost_usd < 0.
            {
                return Err("invalid journal session metadata".into());
            }
            if let Some(info) = &s.session {
                uuid::Uuid::parse_str(&info.session_id)
                    .map_err(|_| "invalid stored CLI session id")?;
            }
        }
        // Keep the full interrupted reservation charged. Result-less turns have unknown spend.
        journal.update(|state| {
            for s in state.seats.values_mut() {
                if s.inflight.as_ref().is_some_and(|r| r.kind == "start") {
                    // Legacy/unbound startup is still ambiguous and fully charged.
                    // New private receipt reconciliation will be a distinct route;
                    // no session equality, model-counter row or replacement here.
                    continue;
                }
                let already_reported = s
                    .inflight
                    .as_ref()
                    .and_then(|r| r.operation_id.as_ref())
                    .and_then(|id| s.start_operations.get(id))
                    .is_some_and(|record| record.interruption_reported);
                let admitted = s.admitted_estimate_usd.is_some() || s.admitted_tokens.is_some();
                if admitted {
                    // A crash cannot prove whether a launched process started model work.
                    // Retain its reservation and block admission until accounting is reconciled.
                    s.uncertain_budget_spend = true;
                    if !already_reported {
                        s.incomplete_token_turns = s
                            .incomplete_token_turns
                            .checked_add(1)
                            .ok_or("incomplete token count overflow")?;
                    }
                }
                let migrated = migrate_usage(s);
                let owned_start = s.inflight.as_ref().and_then(|r| r.operation_id.clone());
                let mut newly_interrupted = true;
                if let Some(operation_id) = &owned_start {
                    let revision = s
                        .start_operations
                        .get(operation_id)
                        .ok_or("missing interrupted start operation")?
                        .revision;
                    // A crash gives no no-write or supervision witness. Retain the original
                    // association/headroom; it must not authorize a replacement operation.
                    reconcile_start(s, operation_id, revision, None, false)?;
                    let record = s
                        .start_operations
                        .get_mut(operation_id)
                        .ok_or("missing interrupted start operation")?;
                    newly_interrupted = !record.interruption_reported;
                    if newly_interrupted {
                        record.interruption_reported = true;
                        record.revision = record
                            .revision
                            .checked_add(1)
                            .ok_or("start operation revision overflow")?;
                    }
                    s.uncertain_budget_spend = true;
                }
                let interrupted = if owned_start.is_some() {
                    newly_interrupted && s.inflight.as_ref().is_some_and(|r| r.kind == "turn")
                } else {
                    s.inflight.take().is_some_and(|r| r.kind == "turn")
                };
                if interrupted {
                    s.incomplete_turns += 1;
                    s.incomplete_token_turns += 1;
                }
                if interrupted || (migrated && s.turns > 0) {
                    queue_usage(s)?;
                }
            }
            Ok(())
        })?;
        Ok(journal)
    }
    /// Settle a cancelled operation without claiming result-less model spend is known.
    /// Its full pre-action reservation remains charged, as on hard-crash recovery.
    /// Cleanup must come from a verified termination result, never an absent turn marker.
    pub fn interrupt(&self, seat: SeatId, cleanup_confirmed: bool) -> Result<(), String> {
        self.update(|state| {
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            if s.inflight.as_ref().is_some_and(|r| r.kind == "start") {
                return Err(
                    "CLI startup remains Unsettled; original private receipt required".into(),
                );
            }
            migrate_usage(s);
            let mut report_interruption = true;
            if let Some(operation_id) = s.inflight.as_ref().and_then(|r| r.operation_id.clone()) {
                let revision = s
                    .start_operations
                    .get(&operation_id)
                    .ok_or("missing interrupted start operation")?
                    .revision;
                report_interruption = !s.start_operations[&operation_id].interruption_reported;
                // Cleanup alone cannot manufacture no-write evidence. Unknown dispatch stays
                // possible even when this child/readers have actually been joined.
                reconcile_start(s, &operation_id, revision, None, cleanup_confirmed)?;
                if !cleanup_confirmed {
                    s.uncertain_budget_spend = true;
                    return Ok(());
                }
                if report_interruption {
                    let record = s
                        .start_operations
                        .get_mut(&operation_id)
                        .ok_or("missing interrupted start operation")?;
                    record.interruption_reported = true;
                    record.revision = record
                        .revision
                        .checked_add(1)
                        .ok_or("start operation revision overflow")?;
                }
            }
            if s.inflight.take().is_some_and(|r| r.kind == "turn") {
                s.uncertain_budget_spend = true;
                if report_interruption {
                    s.incomplete_turns = s
                        .incomplete_turns
                        .checked_add(1)
                        .ok_or("incomplete turn count overflow")?;
                    s.incomplete_token_turns = s
                        .incomplete_token_turns
                        .checked_add(1)
                        .ok_or("incomplete token count overflow")?;
                    queue_usage(s)?;
                }
            } else if !cleanup_confirmed {
                if !s.uncertain_budget_spend {
                    s.uncertain_budget_spend = true;
                    s.incomplete_turns += 1;
                    s.incomplete_token_turns += 1;
                    queue_usage(s)?;
                }
            } else if !uncertain_budget(s, state.config.run.as_ref().map(|r| &r.budget)) {
                // stop() has confirmed cleanup before model input was sent.
                s.admitted_estimate_usd = None;
                s.admitted_tokens = None;
            }
            Ok(())
        })
    }
    pub fn usage_outbox(&self, seat: SeatId) -> Result<Vec<UsageSnapshot>, String> {
        let state = self.snapshot()?;
        Ok(state
            .seats
            .get(&seat)
            .ok_or("unknown seat")?
            .usage
            .as_ref()
            .map(|u| u.outbox.clone())
            .unwrap_or_default())
    }
    /// Only acknowledge the exact committed prefix whose delivery marker was confirmed.
    pub fn acknowledge_usage(&self, seat: SeatId, epoch: u64, revision: u64) -> Result<(), String> {
        self.update(|state| {
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            if s.epoch != epoch {
                return Err("usage acknowledgement epoch mismatch".into());
            }
            let u = s.usage.as_mut().ok_or("missing usage journal")?;
            if revision <= u.acknowledged_revision {
                return Ok(());
            }
            if !u.outbox.iter().any(|entry| entry.revision == revision) {
                return Err("usage acknowledgement was not committed".into());
            }
            u.outbox.retain(|entry| entry.revision > revision);
            u.acknowledged_revision = revision;
            Ok(())
        })
    }
    /// Deliver a committed prefix. A crash after server commit but before this
    /// acknowledgement replays the same epoch/revision, never a new cost increment.
    pub async fn deliver_usage(
        &self,
        seat: SeatId,
        sink: &cna_seats::transcript::TranscriptSink,
    ) -> Result<(), String> {
        let committed = self.usage_outbox(seat)?;
        let Some(last) = committed.last() else {
            return Ok(());
        };
        for snapshot in &committed {
            sink.emit(
                seat,
                cna_protocol::TranscriptEntry::UsageSnapshot(Box::new(snapshot.clone())),
            );
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), sink.flush_confirmed())
            .await
            .map_err(|_| {
                "usage transcript confirmation timed out; committed outbox retained".to_string()
            })??;
        self.acknowledge_usage(seat, last.controller_epoch, last.revision)
    }
    pub fn file(&self) -> &Path {
        &self.path
    }
    pub fn snapshot(&self) -> Result<JournalState, String> {
        let db = self.db.lock().map_err(|e| e.to_string())?;
        Self::read(&db)
    }
    #[allow(dead_code)] // Private source bridge; public startup/root adoption remains held.
    pub(crate) fn startup_receipt(
        &self,
        key: StartupReceiptKey<'_>,
    ) -> Result<Option<StartupReceipt>, String> {
        let db = self.db.lock().map_err(|error| error.to_string())?;
        let state = Self::read(&db)?;
        Self::read_startup_receipt(&db, &state, &key)
    }

    fn read_startup_receipt(
        db: &Connection,
        state: &JournalState,
        key: &StartupReceiptKey<'_>,
    ) -> Result<Option<StartupReceipt>, String> {
        uuid::Uuid::parse_str(key.original.id()).map_err(|_| "invalid startup original UUID")?;
        if state.version != 3 || state.campaign_id != key.campaign_id {
            return Err("startup receipt campaign/version mismatch".into());
        }
        let seat = state
            .seats
            .get(&key.seat)
            .ok_or("unknown startup receipt seat")?;
        if seat.epoch != key.epoch {
            return Err("startup receipt epoch mismatch".into());
        }
        // Search by globally unique UUID so foreign-key collisions refuse rather
        // than appearing absent. Length checks bound TEXT copies before extraction.
        let row = db.query_row(
            "SELECT campaign_id=?1 AND seat=?2 AND epoch=?3, revision,
             CASE WHEN length(requested_ms)<=20 THEN requested_ms ELSE NULL END,
             CASE WHEN length(reserved_ms)<=20 THEN reserved_ms ELSE NULL END, completed,
             CASE WHEN elapsed_ms IS NULL OR length(elapsed_ms)<=20 THEN elapsed_ms ELSE 'invalid' END,
             returned_ok FROM cli_startup_receipt WHERE operation_id=?4",
            params![key.campaign_id, key.seat.to_string(), key.epoch.to_string(), key.original.id()],
            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, u64>(1)?, row.get::<_, String>(2)?,
                row.get::<_, String>(3)?, row.get::<_, bool>(4)?, row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<bool>>(6)?)),
        ).optional().map_err(|error| format!("startup receipt lookup failed: {error}"))?;
        let Some((matches, revision, requested, reserved, completed, elapsed, returned_ok)) = row
        else {
            return Ok(None); // Absence is NOT native absence or another-start permission.
        };
        if !matches || revision != u64::from(completed) {
            return Err("startup receipt original key/revision mismatch".into());
        }
        let requested_ms = startup_integer(&requested)?;
        let reserved_ms = startup_integer(&reserved)?;
        let elapsed_ms = elapsed.as_deref().map(startup_integer).transpose()?;
        if reserved_ms == 0
            || requested_ms < reserved_ms
            || completed != elapsed_ms.is_some()
            || completed != returned_ok.is_some()
        {
            return Err("startup receipt step fields mismatch".into());
        }
        Ok(Some(StartupReceipt {
            revision,
            requested_ms,
            reserved_ms,
            completed,
            elapsed_ms,
            returned_ok,
        }))
    }

    /// Private accounting only. Caller retains the ORIGINAL owner and typed
    /// result, and must creation-admit all storage/transient costs before use.
    /// No dispatch, metadata commit, native-absence or healthy inference here.
    #[allow(dead_code)] // SOURCE assembly; not wired into public startup.
    pub(crate) fn bind_startup_receipt(
        &self,
        key: StartupReceiptKey<'_>,
        expected: StartupReceipt,
    ) -> Result<(), String> {
        Self::check_bound_startup_fields(expected)?;
        self.startup_transaction(|db, state| {
            if Self::read_startup_receipt(db, state, &key)?.is_some() {
                return Err(
                    "startup UUID already recorded; reconcile exact original commit".into(),
                );
            }
            Self::require_no_other_bound_startup(db, &key, false)?;
            // A pre-existing legacy unbound reservation is never adopted.
            let reserved = reserve_in_state(
                state,
                key.seat,
                "start",
                "start",
                expected.requested_ms,
                None,
            )?;
            if reserved != expected.reserved_ms {
                return Err("startup creation reservation plan mismatch".into());
            }
            db.execute(
                "INSERT INTO cli_startup_receipt
                 (campaign_id,seat,epoch,operation_id,revision,requested_ms,reserved_ms,completed)
                 VALUES(?1,?2,?3,?4,0,?5,?6,0)",
                params![
                    key.campaign_id,
                    key.seat.to_string(),
                    key.epoch.to_string(),
                    key.original.id(),
                    expected.requested_ms.to_string(),
                    expected.reserved_ms.to_string()
                ],
            )
            .map_err(|error| format!("startup receipt binding refused: {error}"))?;
            Ok(())
        })
    }

    /// Compare every immutable and step field inside the same read transaction.
    /// This ONLY confirms the expected commit after a lost ACK. It never starts,
    /// removes a reservation, refunds, adopts a session, or releases custody.
    #[allow(dead_code)]
    pub(crate) fn reconcile_startup_receipt(
        &self,
        key: StartupReceiptKey<'_>,
        expected: StartupReceipt,
    ) -> Result<(), String> {
        let mut db = self.db.lock().map_err(|error| error.to_string())?;
        let tx = db.transaction().map_err(|error| error.to_string())?;
        let state = Self::read(&tx)?;
        if Self::read_startup_receipt(&tx, &state, &key)? != Some(expected) {
            return Err("startup lost-ACK original receipt fields mismatch".into());
        }
        if !expected.completed {
            Self::check_startup_reservation(&state, &key, expected)?;
            Self::require_no_other_bound_startup(&tx, &key, true)?;
        }
        tx.commit().map_err(|error| error.to_string())
    }

    /// CAS revision0 to revision1 WITH the matching reservation's accounting
    /// step. Scalar returned_ok is never a native join or healthy settlement.
    /// Re-entry refuses even if identical; exact lost-ACK lookup is separate.
    #[allow(dead_code)]
    pub(crate) fn complete_startup_receipt(
        &self,
        key: StartupReceiptKey<'_>,
        expected: StartupReceipt,
        elapsed_ms: u64,
        returned_ok: bool,
    ) -> Result<StartupReceipt, String> {
        Self::check_bound_startup_fields(expected)?;
        self.startup_transaction(|db, state| {
            if Self::read_startup_receipt(db, state, &key)? != Some(expected) {
                return Err("startup completion original receipt CAS conflict".into());
            }
            Self::require_no_other_bound_startup(db, &key, true)?;
            Self::check_startup_reservation(state, &key, expected)?;
            let completed = StartupReceipt {
                revision: 1,
                completed: true,
                elapsed_ms: Some(elapsed_ms),
                returned_ok: Some(returned_ok),
                ..expected
            };
            let changed = db.execute(
                "UPDATE cli_startup_receipt SET revision=1,completed=1,elapsed_ms=?5,returned_ok=?6
                 WHERE campaign_id=?1 AND seat=?2 AND epoch=?3 AND operation_id=?4
                   AND revision=0 AND completed=0 AND requested_ms=?7 AND reserved_ms=?8
                   AND elapsed_ms IS NULL AND returned_ok IS NULL",
                params![key.campaign_id, key.seat.to_string(), key.epoch.to_string(),
                    key.original.id(), elapsed_ms.to_string(), returned_ok,
                    expected.requested_ms.to_string(), expected.reserved_ms.to_string()],
            ).map_err(|error| format!("startup completion CAS refused: {error}"))?;
            if changed != 1 {
                return Err("startup completion receipt revision conflict".into());
            }
            let seat = state
                .seats
                .get_mut(&key.seat)
                .ok_or("unknown startup seat")?;
            seat.wall_millis = seat
                .wall_millis
                .checked_sub(expected.reserved_ms)
                .and_then(|wall| wall.checked_add(elapsed_ms.max(1)))
                .ok_or("startup wall accounting overflow/inconsistent reservation")?;
            seat.inflight = None;
            // No decision/StartOperation/model count/usage/telemetry/session change.
            Ok(completed)
        })
    }

    fn check_bound_startup_fields(receipt: StartupReceipt) -> Result<(), String> {
        if receipt.revision != 0
            || receipt.completed
            || receipt.elapsed_ms.is_some()
            || receipt.returned_ok.is_some()
            || receipt.reserved_ms == 0
            || receipt.requested_ms < receipt.reserved_ms
        {
            return Err("startup expected original binding fields invalid".into());
        }
        Ok(())
    }

    fn check_startup_reservation(
        state: &JournalState,
        key: &StartupReceiptKey<'_>,
        receipt: StartupReceipt,
    ) -> Result<(), String> {
        Self::check_bound_startup_fields(receipt)?;
        let seat = state.seats.get(&key.seat).ok_or("unknown startup seat")?;
        let reservation = seat
            .inflight
            .as_ref()
            .ok_or("missing original startup reservation")?;
        if seat.epoch != key.epoch
            || reservation.kind != "start"
            || reservation.stage != "start"
            || reservation.millis != receipt.reserved_ms
            || reservation.decision.is_some()
            || reservation.operation_id.is_some()
        {
            return Err("original startup reservation fields mismatch".into());
        }
        Ok(())
    }

    fn require_no_other_bound_startup(
        db: &Connection,
        key: &StartupReceiptKey<'_>,
        require_original: bool,
    ) -> Result<(), String> {
        // Together with atomic bind/complete and generic startup removal guards,
        // one live receipt links the inflight tuple without a model operation_id.
        let valid: bool = db
            .query_row(
                "SELECT count(*)=?4 AND coalesce(sum(operation_id=?3),0)=?4
             FROM cli_startup_receipt WHERE campaign_id=?1 AND seat=?2 AND completed=0",
                params![
                    key.campaign_id,
                    key.seat.to_string(),
                    key.original.id(),
                    u32::from(require_original)
                ],
                |row| row.get(0),
            )
            .map_err(|error| format!("startup reservation receipt association failed: {error}"))?;
        if !valid {
            return Err("startup reservation has conflicting original receipt".into());
        }
        Ok(())
    }

    fn startup_transaction<T>(
        &self,
        action: impl FnOnce(&Connection, &mut JournalState) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut db = self.db.lock().map_err(|error| error.to_string())?;
        let tx = db.transaction().map_err(|error| error.to_string())?;
        let mut state = Self::read(&tx)?;
        migrate_startup_receipts(&tx, &mut state)?;
        let result = action(&tx, &mut state)?;
        validate_start_accounting(&state)?;
        let text = serde_json::to_string(&state).map_err(|error| error.to_string())?;
        if tx
            .execute("UPDATE journal SET state=?1 WHERE id=1", params![text])
            .map_err(|error| error.to_string())?
            != 1
        {
            return Err("startup journal step row mismatch".into());
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(result)
    }
    fn read(db: &Connection) -> Result<JournalState, String> {
        let text: String = db
            .query_row("SELECT state FROM journal WHERE id=1", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let state: JournalState =
            serde_json::from_str(&text).map_err(|e| format!("invalid session journal: {e}"))?;
        if ![1, 2, 3].contains(&state.version) {
            return Err("unsupported session journal version".into());
        }
        Ok(state)
    }
    fn update<T>(
        &self,
        action: impl FnOnce(&mut JournalState) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut db = self.db.lock().map_err(|e| e.to_string())?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let mut state = Self::read(&tx)?;
        migrate_startup_receipts(&tx, &mut state)?;
        let result = action(&mut state)?;
        validate_start_accounting(&state)?;
        let text = serde_json::to_string(&state).map_err(|e| e.to_string())?;
        tx.execute("UPDATE journal SET state=?1 WHERE id=1", params![text])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(result)
    }
    pub fn session(&self, seat: SeatId, info: SessionInfo) -> Result<(), String> {
        uuid::Uuid::parse_str(&info.session_id).map_err(|_| "invalid CLI session id")?;
        self.update(|state| {
            let s = state.seats.get_mut(&seat).ok_or("unknown journal seat")?;
            if s.session.as_ref().map(|i| &i.session_id) != Some(&info.session_id) {
                s.last_session_cost = 0.;
            }
            s.session = Some(info);
            s.telemetry = SessionTelemetry::default();
            Ok(())
        })
    }

    /// Expose only the inline planning count, not the private restore lease type.
    /// Input-growing state/JSON/SQL and allocator/library costs remain separate.
    pub(crate) fn retained_session_inline_bytes() -> usize {
        SessionWriteLease::inline_bytes()
    }

    /// SOURCE-only private startup assembly. Borrow a retained result's Ok body
    /// without surrendering it to a fallible storage callback. On every return
    /// or unwind the exact body goes back to that caller, including after COMMIT.
    /// A storage receipt is separate from original startup/child/reader/custody
    /// disposition. Existing public session/startup paths are NOT redirected.
    #[allow(dead_code)] // Awaiting full original caller/root/fit installation.
    pub(crate) fn session_retaining_body(
        &self,
        seat: SeatId,
        info: &mut SessionInfo,
    ) -> Result<(), String> {
        uuid::Uuid::parse_str(&info.session_id).map_err(|_| "invalid CLI session id")?;
        let mut db = self.db.lock().map_err(|error| error.to_string())?;
        let tx = db.transaction().map_err(|error| error.to_string())?;
        let mut state = Self::read(&tx)?;
        migrate_startup_receipts(&tx, &mut state)?;
        let lease = SessionWriteLease::prepare(info, state, seat)?;
        validate_start_accounting(&lease.state)?;
        let text = serde_json::to_string(&lease.state).map_err(|error| error.to_string())?;
        tx.execute("UPDATE journal SET state=?1 WHERE id=1", params![text])
            .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        // Restore also on successful storage. This is not ownership discharge,
        // accepted original supervision, journal accounting or dispatch permission.
        drop(lease);
        Ok(())
    }
    pub fn recover_process(&self, seat: SeatId, reseed: bool) -> Result<(), String> {
        self.update(|state| {
            let max = state
                .config
                .session
                .as_ref()
                .ok_or("missing limits")?
                .recoveries;
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            if s.inflight.as_ref().is_some_and(|r| r.kind == "start") {
                return Err("CLI startup remains Unsettled; cannot reseed or replace".into());
            }
            if s.recoveries >= max {
                return Err("session recovery budget exhausted".into());
            }
            s.recoveries += 1;
            if reseed {
                s.session = None;
                s.last_session_cost = 0.;
            }
            Ok(())
        })
    }
    /// Explicit operator rebalance. Global cap/unit and accumulated spend are immutable here.
    pub fn reconfigure_run(&self, run: RunControl) -> Result<(), String> {
        self.update(|state| {
            if state.seats.values().any(|s| {
                s.inflight.is_some()
                    || s.admitted_estimate_usd.is_some()
                    || s.admitted_tokens.is_some()
            }) {
                return Err("cannot rebalance while a model turn is admitted or unresolved".into());
            }
            let old = state
                .config
                .run
                .as_ref()
                .ok_or("campaign has no run controls")?;
            let same_cap = match (&old.budget, &run.budget) {
                (
                    SpendBudget::ReportedUsd { global: a, .. },
                    SpendBudget::ReportedUsd { global: b, .. },
                ) => a == b,
                (SpendBudget::Tokens { global: a, .. }, SpendBudget::Tokens { global: b, .. }) => {
                    a == b
                }
                _ => false,
            };
            if !same_cap {
                return Err("rebalance cannot reset or replace the lifetime global cap".into());
            }
            run.validate(&state.seats.keys().copied().collect::<Vec<_>>())?;
            state.config.run = Some(run);
            Ok(())
        })
    }
    /// Admission uses committed lifetime spend before any model observation/spawn.
    /// Forced answers do not call this method or consume a process slot.
    #[allow(clippy::float_arithmetic)]
    pub fn admission(
        &self,
        seat: SeatId,
        bound: Option<&EstimateBound>,
    ) -> Result<Admission, String> {
        Self::check_admission(&self.snapshot()?, seat, bound)
    }
    /// Atomically reserve the proved estimate; concurrent seats cannot reuse headroom.
    // Decimal provider estimates are operator accounting, outside game adjudication.
    #[allow(clippy::float_arithmetic)]
    pub fn admit(&self, seat: SeatId, bound: Option<&EstimateBound>) -> Result<Admission, String> {
        self.update(|state| {
            let result = Self::check_admission(state, seat, bound)?;
            if matches!(result, Admission::Ready { .. }) {
                let bound = bound.ok_or("missing admitted bound")?;
                let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
                match result {
                    Admission::Ready {
                        remaining_usd: Some(remaining_usd),
                        ..
                    } => {
                        s.admitted_estimate_usd =
                            Some(remaining_usd + bound.ceiling_usd().map_err(|e| e.to_string())?)
                    }
                    Admission::Ready {
                        remaining_tokens: Some(remaining_tokens),
                        ..
                    } => {
                        s.admitted_tokens = Some(
                            bound
                                .context_tokens
                                .checked_add(bound.output_tokens)
                                .and_then(|response| response.checked_add(remaining_tokens))
                                .ok_or("token envelope overflow")?,
                        )
                    }
                    _ => {}
                }
            }
            Ok(result)
        })
    }
    pub fn release_admission(&self, seat: SeatId, cleanup_confirmed: bool) -> Result<(), String> {
        if !cleanup_confirmed {
            return self.interrupt(seat, false);
        }
        self.update(|state| {
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            if s.inflight.as_ref().is_some_and(|r| r.kind == "start") {
                return Err("CLI startup remains Unsettled; cannot release admission".into());
            }
            if uncertain_budget(s, state.config.run.as_ref().map(|r| &r.budget)) {
                return Ok(()); // Never free uncertain headroom after interrupted model work.
            }
            if s.inflight.as_ref().is_some_and(|r| r.kind == "turn") {
                return Err("cannot release an unresolved model admission".into());
            }
            s.admitted_estimate_usd = None;
            s.admitted_tokens = None;
            Ok(())
        })
    }
    #[allow(clippy::float_arithmetic)]
    fn check_admission(
        state: &JournalState,
        seat: SeatId,
        bound: Option<&EstimateBound>,
    ) -> Result<Admission, String> {
        let run = state.config.run.as_ref().ok_or("missing run controls")?;
        let own = state.seats.get(&seat).ok_or("unknown budget seat")?;
        let Some(bound) = bound else {
            return Ok(Admission::Stop(
                "unproved model/context/price/output ceiling; no real admission".into(),
            ));
        };
        if bound.pinned_model != own.model {
            return Ok(Admission::Stop(
                "estimate bound differs from pinned model".into(),
            ));
        }
        let ceiling = bound.ceiling_usd().map_err(|e| e.to_string())?;
        if state
            .seats
            .values()
            .any(|s| uncertain_budget(s, Some(&run.budget)))
        {
            return Ok(Admission::Stop(
                "unknown telemetry; committed accounting retained".into(),
            ));
        }
        if own.admitted_estimate_usd.is_some() || own.admitted_tokens.is_some() {
            return Err("overlapping model admissions".into());
        }
        if own.turns >= state.config.max_turns as u64 || own.calls >= state.config.tool_calls {
            return Ok(Admission::Stop(
                "lifetime turn/tool allocation exhausted".into(),
            ));
        }
        match &run.budget {
            SpendBudget::ReportedUsd {
                global,
                allocations,
            } => {
                let spent: f64 = state.seats.values().map(|s| s.reported_cost_usd).sum();
                let reserved: f64 = state
                    .seats
                    .values()
                    .filter_map(|s| s.admitted_estimate_usd)
                    .sum();
                let remaining = global - spent - reserved;
                let own_remaining =
                    allocations.get(&seat).ok_or("missing seat allotment")? - own.reported_cost_usd;
                if !remaining.is_finite()
                    || !own_remaining.is_finite()
                    || !state.largest_turn_cost_usd.is_finite()
                    || state.largest_turn_cost_usd < 0.
                {
                    return Err("invalid budget telemetry".into());
                }
                if own_remaining <= 0. {
                    return Ok(Admission::Stop(format!(
                        "{seat} reported-cost allotment exhausted"
                    )));
                }
                // A turn can make multiple provider responses. Reserve the actual native
                // remaining-allotment cap plus one response crossing it, then retain a
                // separate global response margin. Observed costs never bound admission.
                let margin = ceiling.max(0.25);
                let envelope = own_remaining + ceiling;
                let required_global = envelope + margin;
                if !required_global.is_finite() {
                    return Err("estimate envelope overflow".into());
                }
                if remaining < required_global || own_remaining < ceiling + margin {
                    return Ok(Admission::Stop(
                        "global reported-cost admission margin reached".into(),
                    ));
                }
                Ok(Admission::Ready {
                    remaining_usd: Some(own_remaining),
                    remaining_tokens: None,
                })
            }
            SpendBudget::Tokens {
                global,
                allocations,
            } => {
                let mut spent = 0u64;
                let mut own_spent = 0;
                for (s, record) in &state.seats {
                    let tokens = if record.turns == 0 {
                        0
                    } else {
                        let u = record.usage.as_ref().ok_or("missing usage")?;
                        // Claude reports uncached input separately from cache channels.
                        // Reasoning is part of output, not an extra universal charge.
                        let (Some(input), Some(output), Some(read), Some(create)) = (
                            u.input_tokens,
                            u.output_tokens,
                            u.cache_read_tokens,
                            u.cache_creation_tokens,
                        ) else {
                            return Ok(Admission::Stop(
                                "unknown token channels; no guessed accounting".into(),
                            ));
                        };
                        [input, output, read, create]
                            .into_iter()
                            .try_fold(0u64, |n, v| n.checked_add(v))
                            .ok_or("token total overflow")?
                    };
                    spent = spent
                        .checked_add(tokens)
                        .ok_or("global token total overflow")?;
                    if *s == seat {
                        own_spent = tokens;
                    }
                }
                let remaining = allocations
                    .get(&seat)
                    .ok_or("missing seat allotment")?
                    .saturating_sub(own_spent);
                if spent >= *global || remaining == 0 {
                    return Ok(Admission::Stop("token allotment exhausted".into()));
                }
                let reserved = state
                    .seats
                    .values()
                    .filter_map(|s| s.admitted_tokens)
                    .try_fold(0u64, |n, v| n.checked_add(v))
                    .ok_or("token reservation overflow")?;
                let ceiling = bound
                    .context_tokens
                    .checked_add(bound.output_tokens)
                    .ok_or("token ceiling overflow")?;
                let required = ceiling.checked_mul(2).ok_or("token margin overflow")?;
                let required_global = remaining
                    .checked_add(required)
                    .ok_or("token envelope overflow")?;
                if remaining < required
                    || global.saturating_sub(spent).saturating_sub(reserved) < required_global
                {
                    return Ok(Admission::Stop(
                        "proved token ceiling plus margin unavailable".into(),
                    ));
                }
                Ok(Admission::Ready {
                    remaining_usd: None,
                    remaining_tokens: Some(remaining.min(global - spent)),
                })
            }
        }
    }
    /// Reserve a bounded operation before it starts; the reservation remains charged on hard kill.
    pub fn reserve(
        &self,
        seat: SeatId,
        kind: &str,
        stage: &str,
        requested_ms: u64,
    ) -> Result<u64, String> {
        self.update(|state| reserve_in_state(state, seat, kind, stage, requested_ms, None))
    }
    pub fn reserve_decision(
        &self,
        seat: SeatId,
        stage: &str,
        millis: u64,
        id: &str,
        revision: u32,
    ) -> Result<u64, String> {
        self.update(|state| {
            reserve_in_state(
                state,
                seat,
                "turn",
                stage,
                millis,
                Some((id.into(), revision)),
            )
        })
    }

    /// Reserve compatibility headroom, original decision identity and private provisional
    /// start together. This transaction must commit BEFORE the corresponding dispatch.
    pub fn reserve_decision_authorized(
        &self,
        seat: SeatId,
        stage: &str,
        millis: u64,
        id: &str,
        revision: u32,
        authorization: StartAuthorization<'_>,
    ) -> Result<u64, String> {
        self.update(|state| {
            if state.seats.get(&seat).ok_or("unknown seat")?.epoch != authorization.epoch {
                return Err("start authorization epoch mismatch".into());
            }
            let reserved = reserve_in_state(
                state,
                seat,
                "turn",
                stage,
                millis,
                Some((id.into(), revision)),
            )?;
            authorize_start(
                state.seats.get_mut(&seat).ok_or("unknown seat")?,
                authorization.operation_id,
            )?;
            Ok(reserved)
        })
    }

    /// Private current-operation evidence, with a per-operation CAS revision. Unknown
    /// evidence retains possible liability. `supervised` is supplied only after the
    /// original child/readers/storage disposition, never merely because a timer fired.
    pub fn reconcile_start(
        &self,
        seat: SeatId,
        operation_id: &str,
        expected_revision: u64,
        evidence: Option<cna_seats::operation::StartEvidence>,
        supervised: bool,
    ) -> Result<StartOperation, String> {
        self.update(|state| {
            reconcile_start(
                state.seats.get_mut(&seat).ok_or("unknown seat")?,
                operation_id,
                expected_revision,
                evidence,
                supervised,
            )
        })
    }

    /// An actual committed private marker for the original operation, never a guess from
    /// aggregate usage, an absent reservation, a timer or a later operation's completion.
    pub fn start_accounting_committed(
        &self,
        seat: SeatId,
        operation_id: &str,
    ) -> Result<bool, String> {
        self.start_accounting_state(seat, operation_id)
            .map(|(_, committed)| committed)
    }

    pub fn start_accounting_state(
        &self,
        seat: SeatId,
        operation_id: &str,
    ) -> Result<(u64, bool), String> {
        let state = self.snapshot()?;
        validate_start_accounting(&state)?;
        let s = state.seats.get(&seat).ok_or("unknown seat")?;
        let record = s
            .start_operations
            .get(operation_id)
            .ok_or("unknown start operation")?;
        if record.epoch != s.epoch {
            return Err("start operation epoch mismatch".into());
        }
        Ok((record.revision, record.accounting_committed))
    }

    /// Trusted operator/team report only. This is never a UsageSnapshot producer and
    /// never publishes the private categories or rewrites prior outbox prefixes.
    pub fn private_start_report(&self) -> Result<String, String> {
        let mut state = self.snapshot()?;
        migrate_starts(&mut state)?;
        let mut report = String::new();
        for (seat, s) in &state.seats {
            let totals = s
                .start_accounting
                .as_ref()
                .ok_or("missing lifetime start accounting")?;
            report.push_str(&format!("{seat}: legacy_dispatch_attempts={} (upper bound on starts, ambiguous), possible={}, confirmed={}, provisional={}\n",
                totals.legacy_dispatch_attempts, totals.possible, totals.confirmed, totals.provisional));
            for (stage, usage) in &s.stages {
                let totals = usage
                    .start_accounting
                    .as_ref()
                    .ok_or("missing stage start accounting")?;
                report.push_str(&format!("  {stage}: legacy_dispatch_attempts={} (upper bound on starts, ambiguous), possible={}, confirmed={}, provisional={}\n",
                    totals.legacy_dispatch_attempts, totals.possible, totals.confirmed, totals.provisional));
            }
        }
        Ok(report)
    }
    /// Finish a local automatic operation without attributing it to a model turn.
    pub fn complete_automatic(
        &self,
        seat: SeatId,
        elapsed_ms: u64,
        accepted: bool,
    ) -> Result<(), String> {
        self.update(|state| {
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            let r = s.inflight.take().ok_or("no reserved operation")?;
            if r.kind != "automatic" {
                return Err("not an automatic reservation".into());
            }
            s.wall_millis = s
                .wall_millis
                .saturating_sub(r.millis)
                .saturating_add(elapsed_ms.max(1));
            s.stages.entry(r.stage).or_default().automatic_completed += u64::from(accepted);
            Ok(())
        })
    }
    /// Existing callers retain completion/accounting semantics without a diagnostic.
    pub fn complete(
        &self,
        seat: SeatId,
        elapsed_ms: u64,
        outcome: Option<&TurnOutcome>,
        telemetry: &SessionTelemetry,
    ) -> Result<(), String> {
        self.complete_with_diagnostic(seat, elapsed_ms, outcome, telemetry, None)
            .map_err(|e| e.storage)
    }
    // CLI estimates use decimal USD; these values never enter game adjudication.
    #[allow(clippy::float_arithmetic)]
    pub fn complete_with_diagnostic(
        &self,
        seat: SeatId,
        elapsed_ms: u64,
        outcome: Option<&TurnOutcome>,
        telemetry: &SessionTelemetry,
        timeout: Option<TimeoutCompletion>,
    ) -> Result<(), CompletionError> {
        self.complete_bound_operation(seat, None, elapsed_ms, outcome, telemetry, timeout)
    }

    /// Retained callers may finish only their original operation. The association is
    /// checked in the SAME transaction as usage, outbox and the committed marker.
    #[allow(clippy::float_arithmetic, clippy::too_many_arguments)]
    pub(crate) fn complete_bound_operation(
        &self,
        seat: SeatId,
        expected_operation: Option<&str>,
        elapsed_ms: u64,
        outcome: Option<&TurnOutcome>,
        telemetry: &SessionTelemetry,
        timeout: Option<TimeoutCompletion>,
    ) -> Result<(), CompletionError> {
        self.complete_accounted_operation(
            seat,
            expected_operation,
            elapsed_ms,
            outcome,
            outcome,
            telemetry,
            timeout,
        )
    }

    /// Private recovery records already-captured measurements without converting a
    /// failed supervisor result into a completed turn. The original operation is
    /// mandatory and checked with the supervision phase in the accounting transaction.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn complete_captured_operation(
        &self,
        seat: SeatId,
        expected_operation: &str,
        elapsed_ms: u64,
        captured: &TurnOutcome,
        telemetry: &SessionTelemetry,
        timeout: Option<TimeoutCompletion>,
    ) -> Result<(), CompletionError> {
        self.complete_accounted_operation(
            seat,
            Some(expected_operation),
            elapsed_ms,
            None,
            Some(captured),
            telemetry,
            timeout,
        )
    }

    #[allow(clippy::float_arithmetic, clippy::too_many_arguments)]
    fn complete_accounted_operation(
        &self,
        seat: SeatId,
        expected_operation: Option<&str>,
        elapsed_ms: u64,
        supervisor_outcome: Option<&TurnOutcome>,
        outcome: Option<&TurnOutcome>,
        telemetry: &SessionTelemetry,
        timeout: Option<TimeoutCompletion>,
    ) -> Result<(), CompletionError> {
        let diagnostic = timeout.as_ref().map(|t| t.diagnostic.clone());
        self.update(|state| {
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            if s.inflight.as_ref().is_some_and(|r| r.kind == "start") {
                return Err(
                    "CLI startup completion requires its original private receipt CAS".into(),
                );
            }
            if let Some(expected) = expected_operation
                && s.inflight.as_ref().and_then(|r| r.operation_id.as_deref()) != Some(expected)
            {
                return Err("original completion reservation mismatch".into());
            }
            if let Some(operation_id) = s.inflight.as_ref().and_then(|r| r.operation_id.as_ref()) {
                let phase = &s
                    .start_operations
                    .get(operation_id)
                    .ok_or("missing reserved start operation")?
                    .phase;
                if !matches!(
                    phase,
                    StartPhase::SettledZero
                        | StartPhase::SettledPossible
                        | StartPhase::SettledConfirmed
                ) {
                    return Err("original start operation supervision is unresolved".into());
                }
            }
            if let Some(timeout) = timeout {
                let reservation = s.inflight.as_ref().ok_or("no reserved operation")?;
                if reservation.kind != "turn" || supervisor_outcome.is_some() {
                    return Err("timeout completion requires a failed reserved turn".into());
                }
                s.timeout_events.push(TimeoutEvent {
                    schema_version: 1,
                    campaign_id: state.campaign_id.clone(),
                    seat,
                    controller_epoch: s.epoch,
                    attempt: s.turns,
                    reservation: reservation.clone(),
                    requested_turn_ms: timeout.requested_turn_ms,
                    effective_reserved_ms: reservation.millis,
                    supervisor_turn_elapsed_ms: timeout.supervisor_turn_elapsed_ms,
                    diagnostic: timeout.diagnostic,
                });
            }
            let r = s.inflight.take().ok_or("no reserved operation")?;
            // Round up elapsed time; never charge less than one millisecond for an operation.
            s.wall_millis = s
                .wall_millis
                .saturating_sub(r.millis)
                .saturating_add(elapsed_ms.max(1));
            s.compactions += telemetry
                .compactions
                .saturating_sub(s.telemetry.compactions);
            s.telemetry = telemetry.clone();
            if let Some(info) = &mut s.session {
                if telemetry.model.is_some() {
                    info.model = telemetry.model.clone();
                }
                if telemetry.cli_version.is_some() {
                    info.cli_version = telemetry.cli_version.clone();
                }
            }
            if r.kind == "turn" {
                migrate_usage(s);
                let mut incomplete = false;
                let stage = s.stages.entry(r.stage).or_default();
                stage.completed = stage
                    .completed
                    .checked_add(u64::from(supervisor_outcome.is_some_and(|o| o.ok)))
                    .ok_or("stage completed overflow")?;
                if let Some(o) = outcome {
                    stage.input_tokens = stage
                        .input_tokens
                        .checked_add(o.usage.input_tokens.unwrap_or(0))
                        .ok_or("stage input overflow")?;
                    stage.output_tokens = stage
                        .output_tokens
                        .checked_add(o.usage.output_tokens.unwrap_or(0))
                        .ok_or("stage output overflow")?;
                    add_channel(&mut stage.cache_read_tokens, o.usage.cached_input_tokens)?;
                    add_channel(
                        &mut stage.cache_creation_tokens,
                        o.usage.cache_creation_tokens,
                    )?;
                    add_channel(&mut stage.reasoning_tokens, o.usage.reasoning_tokens)?;
                    let usage = s.usage.as_mut().ok_or("missing usage journal")?;
                    add_channel(&mut usage.input_tokens, o.usage.input_tokens)?;
                    add_channel(&mut usage.output_tokens, o.usage.output_tokens)?;
                    add_channel(&mut usage.cache_read_tokens, o.usage.cached_input_tokens)?;
                    add_channel(
                        &mut usage.cache_creation_tokens,
                        o.usage.cache_creation_tokens,
                    )?;
                    add_channel(&mut usage.reasoning_tokens, o.usage.reasoning_tokens)?;
                    incomplete |= o.usage.input_tokens.is_none() || o.usage.output_tokens.is_none();
                    if let Some(raw) = o.usage.cost_usd {
                        if !raw.is_finite() || raw < 0. {
                            return Err("invalid reported cost".into());
                        }
                        // Preserve the existing per-session delta rules across resume/reseed.
                        let delta = if raw >= s.last_session_cost {
                            raw - s.last_session_cost
                        } else {
                            incomplete = true;
                            raw
                        };
                        state.largest_turn_cost_usd = state.largest_turn_cost_usd.max(delta);
                        if !(!o.ok && raw == 0. && s.last_session_cost > 0.) {
                            s.last_session_cost = raw;
                        }
                        if state.config.run.is_some() && o.usage.cost_basis_known != Some(true) {
                            s.uncertain_budget_spend = true;
                        }
                        s.reported_cost_usd += delta;
                        stage.reported_cost_usd += delta;
                        if !s.reported_cost_usd.is_finite() {
                            return Err("reported cost overflow".into());
                        }
                        usage.reported_cost_usd = Some(s.reported_cost_usd);
                    } else {
                        incomplete = true;
                    }
                } else {
                    incomplete = true;
                }
                if state.config.run.is_some()
                    && outcome.is_some_and(|o| !o.ok && o.usage.cost_usd == Some(0.))
                {
                    s.uncertain_budget_spend = true;
                }
                let token_incomplete = outcome.is_none_or(|o| {
                    o.usage.input_tokens.is_none()
                        || o.usage.output_tokens.is_none()
                        || o.usage.cached_input_tokens.is_none()
                        || o.usage.cache_creation_tokens.is_none()
                });
                s.incomplete_token_turns += u64::from(token_incomplete);
                s.incomplete_turns += u64::from(incomplete);
                let accounted = match state.config.run.as_ref().map(|r| &r.budget) {
                    Some(SpendBudget::ReportedUsd { .. }) => {
                        !incomplete && !s.uncertain_budget_spend
                    }
                    Some(SpendBudget::Tokens { .. }) => !token_incomplete,
                    None => true,
                };
                // Controlled runs retain the envelope until a confirmed process stop;
                // a final usage result alone does not release an unconfirmed live child.
                if accounted && state.config.run.is_none() {
                    s.admitted_estimate_usd = None;
                    s.admitted_tokens = None;
                }
                queue_usage(s)?;
            }
            if let Some(operation_id) = r.operation_id {
                let record = s
                    .start_operations
                    .get_mut(&operation_id)
                    .ok_or("missing completed start operation")?;
                if record.accounting_committed {
                    return Err("start operation accounting already committed".into());
                }
                record.accounting_committed = true;
                record.revision = record
                    .revision
                    .checked_add(1)
                    .ok_or("start operation revision overflow")?;
            }
            Ok(())
        })
        .map_err(|storage| CompletionError {
            timeout: diagnostic,
            storage,
        })
    }
}
impl ToolBudget for SessionJournal {
    fn authorize(
        &self,
        seat: SeatId,
        epoch: u64,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<(), String> {
        let state = self.snapshot()?;
        let s = state.seats.get(&seat).ok_or("unknown seat")?;
        if s.epoch != epoch {
            return Err("journal epoch differs from endpoint".into());
        }
        if tool == "submit" {
            let decision = s
                .inflight
                .as_ref()
                .and_then(|r| r.decision.as_ref())
                .ok_or("no authorized decision")?;
            if args["decision_id"].as_str() != Some(decision.0.as_str())
                || args["revision"].as_u64() != Some(u64::from(decision.1))
            {
                return Err(
                    "only the requested decision revision is authorized in this turn".into(),
                );
            }
        }
        Ok(())
    }

    fn charge(&self, seat: SeatId) -> Result<(), String> {
        self.update(|state| {
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            if s.inflight.as_ref().map(|r| r.kind.as_str()) != Some("turn") {
                return Err("no authorized model turn".into());
            }
            if s.calls >= state.config.tool_calls {
                return Err("durable tool budget exhausted".into());
            }
            s.calls += 1;
            Ok(())
        })
    }
}
#[cfg(test)]
#[allow(clippy::float_arithmetic)]
mod tests {
    use super::*;
    use crate::config::{GameKind, SessionLimits};
    use cna_seats::driver::{SessionTelemetry, Usage};
    fn config() -> LaunchConfig {
        let mut c = LaunchConfig::resolve(
            GameKind::Cna,
            &[
                "axis.front_line=claude:haiku".into(),
                "*=scripted:legal_random".into(),
            ],
        )
        .unwrap();
        c.session = Some(SessionLimits::default());
        c.max_turns = 20;
        c.tool_calls = 6;
        c
    }
    fn seat() -> SeatId {
        "axis.front_line".parse().unwrap()
    }

    fn startup_original() -> cna_seats::operation::Operation {
        cna_seats::operation::Operation::new(
            uuid::Uuid::new_v4().to_string(),
            7,
            cna_seats::operation::Allowance::required(Some(
                tokio::time::Instant::now() + std::time::Duration::from_secs(30),
            ))
            .unwrap(),
        )
        .unwrap()
    }

    fn startup_key<'a>(
        id: &'a str,
        original: &'a cna_seats::operation::Operation,
    ) -> StartupReceiptKey<'a> {
        StartupReceiptKey {
            campaign_id: id,
            seat: seat(),
            epoch: 7,
            original,
        }
    }

    fn startup_bound() -> StartupReceipt {
        StartupReceipt {
            revision: 0,
            requested_ms: 1000,
            reserved_ms: 1000,
            completed: false,
            elapsed_ms: None,
            returned_ok: None,
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. These invoke the actual proposed journal
    // transactions, never a CLI. Connection close/reopen models lost return ACK,
    // NOT a real process crash, native join or permission to dispatch again.
    #[test]
    fn private_startup_binding_rolls_back_receipt_and_reservation_together() {
        for case in 0..7 {
            let (_root, journal, id) = start_fixture();
            let original = startup_original();
            let mut expected = startup_bound();
            if case == 0 || case == 1 {
                journal.db.lock().unwrap().execute_batch(if case == 0 {
                    "CREATE TEMP TRIGGER deny_start_binding BEFORE INSERT ON cli_startup_receipt BEGIN SELECT RAISE(ABORT,'source binding refusal'); END"
                } else {
                    "CREATE TEMP TRIGGER deny_start_journal BEFORE UPDATE ON journal BEGIN SELECT RAISE(ABORT,'source journal refusal'); END"
                }).unwrap();
            }
            if case == 2 {
                expected.reserved_ms = 999;
            }
            if case == 3 {
                journal.reserve(seat(), "start", "start", 1000).unwrap();
            }
            let mut key = startup_key(&id, &original);
            if case == 4 {
                key.campaign_id = "foreign-campaign";
            }
            if case == 5 {
                key.epoch = 6;
            }
            if case == 6 {
                key.seat = "allied.front_line".parse().unwrap();
            }
            let before = serde_json::to_value(journal.snapshot().unwrap()).unwrap();
            assert!(journal.bind_startup_receipt(key, expected).is_err());
            assert_eq!(
                serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
                before
            );
            let count: u64 = journal
                .db
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM cli_startup_receipt", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0);
            assert_eq!(
                original.start_evidence(),
                cna_seats::operation::StartEvidence::NotStarted
            );
        }
    }

    #[test]
    fn private_startup_commits_reconcile_exact_lost_ack_without_reentry_or_model_changes() {
        let (root, journal, id) = start_fixture();
        journal
            .reserve_decision(seat(), "existing-stage", 1000, "existing-decision", 3)
            .unwrap();
        journal
            .complete(
                seat(),
                1,
                Some(&outcome(0.01)),
                &SessionTelemetry::default(),
            )
            .unwrap();
        let prefix = journal.snapshot().unwrap();
        assert_eq!(prefix.seats[&seat()].turns, 1);
        assert!(
            !prefix.seats[&seat()]
                .usage
                .as_ref()
                .unwrap()
                .outbox
                .is_empty()
        );
        let original = startup_original();
        let bound = startup_bound();
        journal
            .bind_startup_receipt(startup_key(&id, &original), bound)
            .unwrap();
        let bound_state = serde_json::to_value(journal.snapshot().unwrap()).unwrap();
        drop(journal); // Commit happened; the caller's return ACK is discarded.
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        assert_eq!(
            serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
            bound_state
        );
        journal
            .reconcile_startup_receipt(startup_key(&id, &original), bound)
            .unwrap();
        assert!(
            journal
                .bind_startup_receipt(startup_key(&id, &original), bound)
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
            bound_state
        );
        for (index, mut wrong) in [bound; 6].into_iter().enumerate() {
            match index {
                0 => wrong.requested_ms += 1,
                1 => wrong.reserved_ms -= 1,
                2 => wrong.revision = 1,
                3 => wrong.completed = true,
                4 => wrong.elapsed_ms = Some(0),
                _ => wrong.returned_ok = Some(false),
            }
            assert!(
                journal
                    .reconcile_startup_receipt(startup_key(&id, &original), wrong)
                    .is_err()
            );
            assert!(
                journal
                    .complete_startup_receipt(startup_key(&id, &original), wrong, 42, true)
                    .is_err()
            );
        }
        for case in 0..3 {
            let mut key = startup_key(&id, &original);
            match case {
                0 => key.campaign_id = "foreign-campaign",
                1 => key.seat = "allied.front_line".parse().unwrap(),
                _ => key.epoch = 6,
            }
            assert!(
                journal
                    .complete_startup_receipt(key, bound, 42, true)
                    .is_err()
            );
        }
        let foreign = startup_original();
        assert!(
            journal
                .complete_startup_receipt(startup_key(&id, &foreign), bound, 42, true)
                .is_err()
        );
        let completed = journal
            .complete_startup_receipt(startup_key(&id, &original), bound, 42, true)
            .unwrap();
        assert_eq!(
            completed,
            StartupReceipt {
                revision: 1,
                completed: true,
                elapsed_ms: Some(42),
                returned_ok: Some(true),
                ..bound
            }
        );
        let completed_state = serde_json::to_value(journal.snapshot().unwrap()).unwrap();
        drop(journal); // Again discard return ACK; retain SAME original key/expected step.
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        assert_eq!(
            serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
            completed_state
        );
        journal
            .reconcile_startup_receipt(startup_key(&id, &original), completed)
            .unwrap();
        assert!(
            journal
                .reconcile_startup_receipt(startup_key(&id, &original), bound)
                .is_err()
        );
        assert!(
            journal
                .complete_startup_receipt(startup_key(&id, &original), bound, 42, true)
                .is_err()
        );
        let mut wrong = completed;
        wrong.elapsed_ms = Some(43);
        assert!(
            journal
                .reconcile_startup_receipt(startup_key(&id, &original), wrong)
                .is_err()
        );
        wrong = completed;
        wrong.returned_ok = Some(false);
        assert!(
            journal
                .reconcile_startup_receipt(startup_key(&id, &original), wrong)
                .is_err()
        );
        let mut after = journal.snapshot().unwrap();
        assert!(after.seats[&seat()].inflight.is_none());
        // Remove ONLY the documented startup accounting changes, then compare
        // the ENTIRE previous model/decision/outbox/metadata snapshot.
        let s = after.seats.get_mut(&seat()).unwrap();
        assert_eq!(s.wall_millis, prefix.seats[&seat()].wall_millis + 42);
        s.wall_millis = prefix.seats[&seat()].wall_millis;
        let stage = s.stages.remove("start").unwrap();
        assert_eq!(
            serde_json::to_value(stage).unwrap(),
            serde_json::to_value(StageUsage {
                start_accounting: Some(StartTotals::default()),
                ..StageUsage::default()
            })
            .unwrap()
        );
        assert_eq!(
            serde_json::to_value(after).unwrap(),
            serde_json::to_value(prefix).unwrap()
        );
        // An old completion must not clear a subsequent unbound legacy startup,
        // even with exactly the same tuple. Reconciliation is read-only.
        journal.reserve(seat(), "start", "start", 1000).unwrap();
        let legacy = serde_json::to_value(journal.snapshot().unwrap()).unwrap();
        assert!(
            journal
                .complete_startup_receipt(startup_key(&id, &original), bound, 42, true)
                .is_err()
        );
        journal
            .reconcile_startup_receipt(startup_key(&id, &original), completed)
            .unwrap();
        assert_eq!(
            serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
            legacy
        );
        assert_eq!(
            original.start_evidence(),
            cna_seats::operation::StartEvidence::NotStarted
        );
    }

    #[test]
    fn private_startup_completion_cas_rollback_retains_both_original_rows() {
        for case in 0..3 {
            let (_root, journal, id) = start_fixture();
            let original = startup_original();
            let bound = startup_bound();
            journal
                .bind_startup_receipt(startup_key(&id, &original), bound)
                .unwrap();
            journal.db.lock().unwrap().execute_batch(match case {
                0 => "CREATE TEMP TRIGGER deny_receipt_complete BEFORE UPDATE ON cli_startup_receipt BEGIN SELECT RAISE(ABORT,'source receipt refusal'); END",
                1 => "CREATE TEMP TRIGGER deny_journal_complete BEFORE UPDATE ON journal BEGIN SELECT RAISE(ABORT,'source journal refusal'); END",
                _ => "UPDATE cli_startup_receipt SET revision=2", // SYNTHETIC stale revision.
            }).unwrap();
            let before = serde_json::to_value(journal.snapshot().unwrap()).unwrap();
            let row_before: (u64, bool, Option<String>, Option<bool>) = journal
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT revision,completed,elapsed_ms,returned_ok FROM cli_startup_receipt",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
            assert!(
                journal
                    .complete_startup_receipt(startup_key(&id, &original), bound, 42, false)
                    .is_err()
            );
            assert_eq!(
                serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
                before
            );
            let row_after: (u64, bool, Option<String>, Option<bool>) = journal
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT revision,completed,elapsed_ms,returned_ok FROM cli_startup_receipt",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
            assert_eq!(row_after, row_before);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Planned SQLite migration controls; no
    // native start, crash, supervision/custody or lost-ACK acceptance witness.
    #[test]
    fn private_startup_migration_is_atomic_and_preserves_existing_rows() {
        for case in 0..6 {
            let (_root, journal, _id) = start_fixture();
            let mut old = journal.snapshot().unwrap();
            old.version = if case == 1 {
                1
            } else if case == 3 {
                4
            } else if case == 5 {
                3
            } else {
                2
            };
            if case == 1 {
                for seat in old.seats.values_mut() {
                    seat.start_accounting = None;
                    for stage in seat.stages.values_mut() {
                        stage.start_accounting = None;
                    }
                }
            }
            let encoded = serde_json::to_string(&old).unwrap();
            {
                let db = journal.db.lock().unwrap();
                if case != 4 {
                    db.execute_batch("DROP TABLE cli_startup_receipt").unwrap();
                }
                db.execute("UPDATE journal SET state=?1", params![encoded])
                    .unwrap();
                if case == 2 {
                    db.execute_batch("CREATE TEMP TRIGGER deny_migration BEFORE UPDATE ON journal BEGIN SELECT RAISE(ABORT,'source migration refusal'); END").unwrap();
                }
            }
            let migrated = journal.update(|_| Ok(()));
            let db = journal.db.lock().unwrap();
            let raw: String = db
                .query_row("SELECT state FROM journal WHERE id=1", [], |row| row.get(0))
                .unwrap();
            let tables: u64 = db.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='cli_startup_receipt'", [], |row| row.get(0)).unwrap();
            if case >= 2 {
                assert!(migrated.is_err());
                assert_eq!(raw, encoded);
                assert_eq!(tables, u64::from(case == 4)); // No schema repair/adoption.
                if case == 3 {
                    assert!(SessionJournal::read(&db).is_err());
                }
            } else {
                migrated.unwrap();
                let mut expected = old;
                // Existing journal1 categories migrate by their existing exact
                // legacy rules. The new receipt migration adds no inferred row.
                migrate_starts(&mut expected).unwrap();
                expected.version = 3;
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&raw).unwrap(),
                    serde_json::to_value(&expected).unwrap()
                );
                assert_eq!(tables, 1);
                let receipts: u64 = db
                    .query_row("SELECT count(*) FROM cli_startup_receipt", [], |row| {
                        row.get(0)
                    })
                    .unwrap();
                assert_eq!(receipts, 0);
                drop(db);
                let before = serde_json::to_value(journal.snapshot().unwrap()).unwrap();
                journal.update(|_| Ok(())).unwrap();
                assert_eq!(
                    serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
                    before
                );
            }
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Rows are SYNTHETIC test SQL, not actual
    // startup reservation/completion commits or lost-ACK/recovery proof.
    #[tokio::test]
    async fn private_startup_lookup_rejects_foreign_stale_and_malformed_rows() {
        for case in 0..9 {
            let (_root, journal, campaign_id) = start_fixture();
            journal
                .reserve_decision(seat(), "existing-stage", 1000, "existing-decision", 3)
                .unwrap();
            journal
                .complete(
                    seat(),
                    1,
                    Some(&outcome(0.01)),
                    &SessionTelemetry::default(),
                )
                .unwrap();
            // Nonempty model-counter/decision/outbox prefix, including a real
            // journal-only provisional row. No dispatch or join is inferred.
            let model_id = uuid::Uuid::new_v4().to_string();
            journal
                .reserve_decision_authorized(
                    seat(),
                    "existing-stage",
                    1000,
                    "pending-model-decision",
                    4,
                    StartAuthorization {
                        epoch: 7,
                        operation_id: &model_id,
                    },
                )
                .unwrap();
            let original = cna_seats::operation::Operation::new(
                uuid::Uuid::new_v4().to_string(),
                7,
                cna_seats::operation::Allowance::required(Some(
                    tokio::time::Instant::now() + std::time::Duration::from_secs(30),
                ))
                .unwrap(),
            )
            .unwrap();
            let model_prefix = journal.snapshot().unwrap();
            assert_eq!(
                model_prefix.seats[&seat()]
                    .start_accounting
                    .as_ref()
                    .unwrap()
                    .provisional,
                1
            );
            assert_eq!(model_prefix.seats[&seat()].start_operations.len(), 1);
            assert_eq!(
                model_prefix.seats[&seat()]
                    .usage
                    .as_ref()
                    .unwrap()
                    .outbox
                    .len(),
                1
            );
            let before = serde_json::to_value(journal.snapshot().unwrap()).unwrap();
            if case != 0 {
                let row_campaign = if case == 3 { "foreign" } else { &campaign_id };
                let row_seat = if case == 4 {
                    "allies.front_line".to_string()
                } else {
                    seat().to_string()
                };
                let epoch = if case == 5 { "6" } else { "7" };
                let completed = i64::from(case == 2);
                let revision = if case == 6 { 2 } else { completed };
                let requested = if case == 7 { "01000" } else { "1000" };
                let db = journal.db.lock().unwrap();
                db.execute(
                    "INSERT INTO cli_startup_receipt VALUES(?1,?2,?3,?4,?5,?6,'500',?7,?8,?9)",
                    params![
                        row_campaign,
                        row_seat,
                        epoch,
                        original.id(),
                        revision,
                        requested,
                        completed,
                        (case == 2).then_some("12"),
                        (case == 2).then_some(1)
                    ],
                )
                .unwrap();
                if case == 8 {
                    assert!(db.execute("INSERT INTO cli_startup_receipt VALUES('foreign','allies.front_line','8',?1,0,'1000','500',0,NULL,NULL)",params![original.id()]).is_err());
                    let count: u64 = db
                        .query_row("SELECT count(*) FROM cli_startup_receipt", [], |row| {
                            row.get(0)
                        })
                        .unwrap();
                    assert_eq!(count, 1);
                }
            }
            let receipt = journal.startup_receipt(StartupReceiptKey {
                campaign_id: &campaign_id,
                seat: seat(),
                epoch: 7,
                original: &original,
            });
            match case {
                0 => assert_eq!(receipt.unwrap(), None),
                1 | 8 => assert_eq!(
                    receipt.unwrap(),
                    Some(StartupReceipt {
                        revision: 0,
                        requested_ms: 1000,
                        reserved_ms: 500,
                        completed: false,
                        elapsed_ms: None,
                        returned_ok: None
                    })
                ),
                2 => assert_eq!(
                    receipt.unwrap(),
                    Some(StartupReceipt {
                        revision: 1,
                        requested_ms: 1000,
                        reserved_ms: 500,
                        completed: true,
                        elapsed_ms: Some(12),
                        returned_ok: Some(true)
                    })
                ),
                _ => assert!(receipt.is_err()),
            }
            assert!(
                journal
                    .startup_receipt(StartupReceiptKey {
                        campaign_id: "foreign",
                        seat: seat(),
                        epoch: 7,
                        original: &original
                    })
                    .is_err()
            );
            assert!(
                journal
                    .startup_receipt(StartupReceiptKey {
                        campaign_id: &campaign_id,
                        seat: seat(),
                        epoch: 8,
                        original: &original
                    })
                    .is_err()
            );
            // Entire public journal snapshot is unchanged, including decisions,
            // StartOperation/lifetime/stage model counters and usage outbox.
            assert_eq!(
                serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
                before
            );
        }
    }

    #[test] // SOURCE ONLY/UNCOMPILED/UNRUN; no native or receipt-proven startup.
    fn legacy_unbound_cli_startup_survives_recovery_without_refund_or_reseed() {
        let (root, journal, id) = start_fixture();
        let initial = journal.snapshot().unwrap();
        journal.reserve(seat(), "start", "start", 1000).unwrap();
        let mut legacy = journal.snapshot().unwrap();
        assert_eq!(
            legacy.seats[&seat()].start_accounting,
            initial.seats[&seat()].start_accounting
        );
        assert_eq!(legacy.seats[&seat()].turns, initial.seats[&seat()].turns);
        assert!(legacy.seats[&seat()].start_operations.is_empty());
        assert!(
            legacy.seats[&seat()]
                .usage
                .as_ref()
                .unwrap()
                .outbox
                .is_empty()
        );
        legacy.version = 2;
        {
            let db = journal.db.lock().unwrap();
            db.execute_batch("DROP TABLE cli_startup_receipt").unwrap();
            db.execute(
                "UPDATE journal SET state=?1",
                params![serde_json::to_string(&legacy).unwrap()],
            )
            .unwrap();
        }
        let mut expected = legacy;
        expected.version = 3;
        let before = serde_json::to_value(&expected).unwrap();
        assert!(
            journal.snapshot().unwrap().seats[&seat()]
                .inflight
                .as_ref()
                .unwrap()
                .operation_id
                .is_none()
        );
        drop(journal);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        assert_eq!(
            serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
            before
        );
        for cleanup in [false, true] {
            assert!(journal.interrupt(seat(), cleanup).is_err());
            assert!(journal.release_admission(seat(), cleanup).is_err());
        }
        assert!(journal.recover_process(seat(), true).is_err());
        assert!(
            journal
                .complete(seat(), 1, None, &SessionTelemetry::default())
                .is_err()
        );
        assert!(journal.reserve(seat(), "start", "start", 1000).is_err());
        assert_eq!(
            serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
            before
        );
        let db = journal.db.lock().unwrap();
        let count: u64 = db
            .query_row("SELECT count(*) FROM cli_startup_receipt", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0); // No invented receipt or model-start association.
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Future local SQLite fixture, no CLI/child/
    // archive or native-start/custody witness. Public startup remains unadopted.
    #[test]
    fn session_storage_refusal_and_commit_restore_exact_retained_body() {
        for case in 0..5 {
            let root = tempfile::tempdir().unwrap();
            let journal = SessionJournal::create(
                root.path(),
                &uuid::Uuid::new_v4().to_string(),
                config(),
                &BTreeMap::from([(seat(), 2)]),
            )
            .unwrap();
            let old = SessionInfo {
                session_id: uuid::Uuid::new_v4().to_string(),
                model: Some("old model".into()),
                cli_version: Some("old version".into()),
                resumed: false,
            };
            journal.session(seat(), old).unwrap();
            journal
                .update(|state| {
                    let record = state.seats.get_mut(&seat()).unwrap();
                    record.last_session_cost = 0.5;
                    record.telemetry.model = Some("old telemetry".into());
                    Ok(())
                })
                .unwrap();
            let before = journal.snapshot().unwrap();
            let mut info = SessionInfo {
                session_id: if case == 0 {
                    "invalid session id".into()
                } else if case == 4 {
                    // Fixture-only copy of the SAME stored session identity.
                    before.seats[&seat()]
                        .session
                        .as_ref()
                        .unwrap()
                        .session_id
                        .clone()
                } else {
                    uuid::Uuid::new_v4().to_string()
                },
                model: Some("returned model body".into()),
                cli_version: Some("returned version body".into()),
                resumed: true,
            };
            let pointers = (
                info.session_id.as_ptr(),
                info.model.as_ref().unwrap().as_ptr(),
                info.cli_version.as_ref().unwrap().as_ptr(),
            );
            if case == 2 {
                // Deliberate fixture-only write refusal after candidate serialization.
                journal.db.lock().unwrap().execute_batch(
                    "CREATE TEMP TRIGGER refuse_session_source BEFORE UPDATE ON journal BEGIN SELECT RAISE(ABORT, 'synthetic session write refusal'); END;",
                ).unwrap();
            }
            let target = if case == 1 {
                "axis.commander".parse().unwrap()
            } else {
                seat()
            };
            let receipt = journal.session_retaining_body(target, &mut info);
            assert_eq!(receipt.is_ok(), case >= 3);
            assert_eq!(info.session_id.as_ptr(), pointers.0);
            assert_eq!(info.model.as_ref().unwrap().as_ptr(), pointers.1);
            assert_eq!(info.cli_version.as_ref().unwrap().as_ptr(), pointers.2);
            assert!(info.resumed);
            let mut expected = before;
            if case >= 3 {
                // Fixture comparison only; production performed no SessionInfo clone.
                let record = expected.seats.get_mut(&seat()).unwrap();
                record.session = Some(info.clone());
                if case == 3 {
                    record.last_session_cost = 0.;
                }
                record.telemetry = SessionTelemetry::default();
            }
            assert_eq!(
                serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
                serde_json::to_value(expected).unwrap(),
            );
            assert!(journal.usage_outbox(seat()).unwrap().is_empty());
            // Commit means ONLY this stored session candidate. Retaining the
            // returned body neither settles startup nor confirms any native join.
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Exercise actual restore lease unwind without
    // a live DB mutex/transaction or thread; no poisoned-lock recovery inference.
    #[test]
    #[cfg(panic = "unwind")]
    fn session_candidate_unwind_restores_body_before_caller_can_reuse_it() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        struct Opaque(std::cell::Cell<usize>);
        let root = tempfile::tempdir().unwrap();
        let journal = SessionJournal::create(
            root.path(),
            &uuid::Uuid::new_v4().to_string(),
            config(),
            &BTreeMap::from([(seat(), 2)]),
        )
        .unwrap();
        let before = journal.snapshot().unwrap();
        let state = journal.snapshot().unwrap();
        let mut info = SessionInfo {
            session_id: uuid::Uuid::new_v4().to_string(),
            model: Some("original model".into()),
            cli_version: Some("original version".into()),
            resumed: true,
        };
        let pointers = (
            info.session_id.as_ptr(),
            info.model.as_ref().unwrap().as_ptr(),
            info.cli_version.as_ref().unwrap().as_ptr(),
        );
        let payload = Box::new(Opaque(std::cell::Cell::new(73)));
        let payload_pointer = std::ptr::from_ref(payload.as_ref());
        let result = catch_unwind(AssertUnwindSafe(|| {
            let lease = SessionWriteLease::prepare(&mut info, state, seat()).unwrap();
            assert!(lease.installed);
            let held = lease.state.seats[&seat()].session.as_ref().unwrap();
            assert_eq!(held.session_id.as_ptr(), pointers.0);
            assert_eq!(held.model.as_ref().unwrap().as_ptr(), pointers.1);
            assert_eq!(held.cli_version.as_ref().unwrap().as_ptr(), pointers.2);
            std::panic::resume_unwind(payload);
        }));
        let opaque = match result {
            Err(payload) => payload.downcast::<Opaque>().ok().unwrap(),
            Ok(()) => panic!("session source control did not unwind"),
        };
        assert_eq!(std::ptr::from_ref(opaque.as_ref()), payload_pointer);
        assert_eq!(opaque.0.get(), 73);
        assert_eq!(info.session_id.as_ptr(), pointers.0);
        assert_eq!(info.model.as_ref().unwrap().as_ptr(), pointers.1);
        assert_eq!(info.cli_version.as_ref().unwrap().as_ptr(), pointers.2);
        assert!(info.resumed);
        assert_eq!(
            serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
            serde_json::to_value(before).unwrap(),
        );
        assert!(journal.usage_outbox(seat()).unwrap().is_empty());
        assert!(SessionWriteLease::inline_bytes() >= std::mem::size_of::<SessionWriteLease<'_>>());
        // Inline count is not whole-fit/actual original creation admission.
    }

    fn outcome(cost: f64) -> TurnOutcome {
        TurnOutcome {
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
        }
    }
    fn bound() -> EstimateBound {
        EstimateBound {
            pinned_model: "haiku".into(),
            context_tokens: 10,
            output_tokens: 10,
            max_input_or_cache_write_usd_per_token: 0.001,
            output_usd_per_token: 0.001,
            evidence: "inert unit fixture; not native proof".into(),
        }
    }
    #[test]
    fn no_model_cleanup_uses_the_selected_budget_unit_not_optional_other_telemetry() {
        for usd in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            let mut c = config();
            c.run = Some(RunControl {
                boundary: cna_server::RunBoundary {
                    game_turn: 1,
                    op_stage: None,
                },
                budget: if usd {
                    SpendBudget::usd(2., &[seat()], BTreeMap::from([(seat(), 1.)])).unwrap()
                } else {
                    SpendBudget::tokens(1000, &[seat()], BTreeMap::from([(seat(), 500)])).unwrap()
                },
            });
            let j = SessionJournal::create(root.path(), &id, c, &BTreeMap::from([(seat(), 2)]))
                .unwrap();
            j.admit(seat(), Some(&bound())).unwrap();
            j.reserve_decision(seat(), "GT1:OpStage1", 1000, "d", 0)
                .unwrap();
            let mut report = outcome(0.01);
            report.usage.cost_basis_known = Some(true);
            if !usd {
                report.usage.cost_usd = None;
                report.usage.cached_input_tokens = Some(0);
                report.usage.cache_creation_tokens = Some(0);
            }
            j.complete(seat(), 1, Some(&report), &SessionTelemetry::default())
                .unwrap();
            let record = j.snapshot().unwrap().seats[&seat()].clone();
            assert_eq!(record.incomplete_token_turns, u64::from(usd));
            assert_eq!(record.incomplete_turns, u64::from(!usd));
            assert!(j.admit(seat(), Some(&bound())).is_err());
            j.release_admission(seat(), true).unwrap();
            assert!(matches!(
                j.admit(seat(), Some(&bound())).unwrap(),
                Admission::Ready { .. }
            ));
            j.release_admission(seat(), true).unwrap();
            assert!(matches!(
                j.admit(seat(), Some(&bound())).unwrap(),
                Admission::Ready { .. }
            ));
            j.interrupt(seat(), true).unwrap();
            let record = j.snapshot().unwrap().seats[&seat()].clone();
            assert!(record.admitted_estimate_usd.is_none() && record.admitted_tokens.is_none());
        }
    }
    #[test]
    fn concurrent_seats_cannot_reuse_the_native_cap_envelope() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let other: SeatId = "axis.commander".parse().unwrap();
        let mut c = config();
        c.seats
            .insert(other, crate::config::Controller::Claude("haiku".into()));
        c.run = Some(RunControl {
            boundary: cna_server::RunBoundary {
                game_turn: 1,
                op_stage: None,
            },
            budget: SpendBudget::usd(
                1.,
                &[seat(), other],
                BTreeMap::from([(seat(), 0.45), (other, 0.45)]),
            )
            .unwrap(),
        });
        let j = std::sync::Arc::new(
            SessionJournal::create(
                root.path(),
                &id,
                c,
                &BTreeMap::from([(seat(), 2), (other, 2)]),
            )
            .unwrap(),
        );
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let outcomes = std::thread::scope(|scope| {
            let tasks: Vec<_> = [seat(), other]
                .into_iter()
                .map(|seat| {
                    let j = j.clone();
                    let barrier = barrier.clone();
                    scope.spawn(move || {
                        barrier.wait();
                        j.admit(seat, Some(&bound())).unwrap()
                    })
                })
                .collect();
            tasks
                .into_iter()
                .map(|t| t.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            outcomes
                .iter()
                .filter(|r| matches!(r, Admission::Ready { .. }))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|r| matches!(r, Admission::Stop(_)))
                .count(),
            1
        );
        let reserved: f64 = j
            .snapshot()
            .unwrap()
            .seats
            .values()
            .filter_map(|s| s.admitted_estimate_usd)
            .sum();
        assert!((reserved - 0.47).abs() < 1e-12);
    }
    #[test]
    fn concurrent_admission_and_crash_keep_uncertain_headroom_reserved() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let mut c = config();
        c.run = Some(RunControl {
            boundary: cna_server::RunBoundary {
                game_turn: 1,
                op_stage: Some(1),
            },
            budget: SpendBudget::usd(2., &[seat()], BTreeMap::from([(seat(), 1.)])).unwrap(),
        });
        let j = std::sync::Arc::new(
            SessionJournal::create(root.path(), &id, c, &BTreeMap::from([(seat(), 2)])).unwrap(),
        );
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let outcomes = std::thread::scope(|scope| {
            let tasks: Vec<_> = (0..2)
                .map(|_| {
                    let j = j.clone();
                    let barrier = barrier.clone();
                    scope.spawn(move || {
                        barrier.wait();
                        j.admit(seat(), Some(&bound()))
                    })
                })
                .collect();
            tasks
                .into_iter()
                .map(|t| t.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            outcomes
                .iter()
                .filter(|r| matches!(r, Ok(Admission::Ready { .. })))
                .count(),
            1
        );
        assert_eq!(outcomes.iter().filter(|r| r.is_err()).count(), 1);
        assert_eq!(
            j.snapshot().unwrap().seats[&seat()].admitted_estimate_usd,
            Some(1.02)
        );
        let run = j.snapshot().unwrap().config.run.unwrap();
        assert!(j.reconfigure_run(run).is_err());
        drop(j); // crash after admission, before any durable turn-start marker
        let j = SessionJournal::recover(root.path(), &id).unwrap();
        let s = j.snapshot().unwrap();
        assert!(s.seats[&seat()].uncertain_budget_spend);
        assert_eq!(s.seats[&seat()].admitted_estimate_usd, Some(1.02));
        assert!(matches!(
            j.admission(seat(), Some(&bound())).unwrap(),
            Admission::Stop(_)
        ));
        j.release_admission(seat(), true).unwrap();
        assert_eq!(
            j.snapshot().unwrap().seats[&seat()].admitted_estimate_usd,
            Some(1.02)
        );
    }
    #[test]
    fn verified_no_model_cleanup_and_accounted_completion_release_reservations() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let mut c = config();
        c.run = Some(RunControl {
            boundary: cna_server::RunBoundary {
                game_turn: 1,
                op_stage: None,
            },
            budget: SpendBudget::usd(2., &[seat()], BTreeMap::from([(seat(), 1.)])).unwrap(),
        });
        let j =
            SessionJournal::create(root.path(), &id, c, &BTreeMap::from([(seat(), 2)])).unwrap();
        assert!(matches!(
            j.admit(seat(), Some(&bound())).unwrap(),
            Admission::Ready { .. }
        ));
        j.release_admission(seat(), true).unwrap(); // caller verified stop before sending model input
        assert!(
            j.snapshot().unwrap().seats[&seat()]
                .admitted_estimate_usd
                .is_none()
        );
        j.admit(seat(), Some(&bound())).unwrap();
        j.reserve_decision(seat(), "GT1:OpStage1", 1000, "d", 0)
            .unwrap();
        assert!(j.release_admission(seat(), true).is_err());
        let mut report = outcome(0.01);
        report.usage.cost_basis_known = Some(true);
        j.complete(seat(), 1, Some(&report), &SessionTelemetry::default())
            .unwrap();
        assert!(
            j.snapshot().unwrap().seats[&seat()]
                .admitted_estimate_usd
                .is_some()
        );
        j.release_admission(seat(), true).unwrap();
        assert!(
            j.snapshot().unwrap().seats[&seat()]
                .admitted_estimate_usd
                .is_none()
        );
        assert_eq!(j.snapshot().unwrap().seats[&seat()].reported_cost_usd, 0.01);
        assert!(matches!(
            j.admission(seat(), None).unwrap(),
            Admission::Stop(_)
        ));
    }
    #[test]
    fn remaining_admission_rebalance_and_uncertainty_survive_resume() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let mut c = config();
        c.run = Some(RunControl {
            boundary: cna_server::RunBoundary {
                game_turn: 1,
                op_stage: Some(1),
            },
            budget: SpendBudget::usd(2., &[seat()], BTreeMap::from([(seat(), 1.)])).unwrap(),
        });
        let j =
            SessionJournal::create(root.path(), &id, c, &BTreeMap::from([(seat(), 2)])).unwrap();
        assert_eq!(
            j.admission(seat(), Some(&bound())).unwrap(),
            Admission::Ready {
                remaining_usd: Some(1.),
                remaining_tokens: None
            }
        );
        let mut report = outcome(0.1);
        report.usage.cost_basis_known = Some(true);
        j.reserve_decision(seat(), "GT1:OpStage1", 1000, "d", 0)
            .unwrap();
        j.complete(seat(), 1, Some(&report), &SessionTelemetry::default())
            .unwrap();
        drop(j);
        let j = SessionJournal::recover(root.path(), &id).unwrap();
        assert_eq!(
            j.admission(seat(), Some(&bound())).unwrap(),
            Admission::Ready {
                remaining_usd: Some(0.9),
                remaining_tokens: None
            }
        );
        let mut run = j.snapshot().unwrap().config.run.unwrap();
        run.boundary.game_turn = 2;
        if let SpendBudget::ReportedUsd { allocations, .. } = &mut run.budget {
            allocations.insert(seat(), 0.6);
        }
        j.reconfigure_run(run.clone()).unwrap();
        let Admission::Ready {
            remaining_usd: Some(value),
            ..
        } = j.admission(seat(), Some(&bound())).unwrap()
        else {
            panic!()
        };
        assert!((value - 0.5).abs() < 1e-12);
        if let SpendBudget::ReportedUsd { global, .. } = &mut run.budget {
            *global = 3.;
        }
        assert!(j.reconfigure_run(run).is_err());
        report.usage.cost_basis_known = Some(false);
        report.usage.cost_usd = Some(0.15);
        j.reserve_decision(seat(), "GT1:OpStage1", 1000, "d", 1)
            .unwrap();
        j.complete(seat(), 1, Some(&report), &SessionTelemetry::default())
            .unwrap();
        assert!(matches!(
            j.admission(seat(), Some(&bound())).unwrap(),
            Admission::Stop(_)
        ));
        drop(j);
        let j = SessionJournal::recover(root.path(), &id).unwrap();
        assert!(matches!(
            j.admission(seat(), Some(&bound())).unwrap(),
            Admission::Stop(_)
        ));
    }
    #[test]
    fn global_margin_and_resultless_turns_stop_before_new_observations() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let mut c = config();
        c.run = Some(RunControl {
            boundary: cna_server::RunBoundary {
                game_turn: 1,
                op_stage: None,
            },
            budget: SpendBudget::usd(0.2, &[seat()], BTreeMap::new()).unwrap(),
        });
        let j =
            SessionJournal::create(root.path(), &id, c, &BTreeMap::from([(seat(), 2)])).unwrap();
        assert!(matches!(
            j.admission(seat(), Some(&bound())).unwrap(),
            Admission::Stop(_)
        ));
        j.reserve_decision(seat(), "GT1:OpStage1", 1000, "d", 0)
            .unwrap();
        drop(j);
        let j = SessionJournal::recover(root.path(), &id).unwrap();
        assert_eq!(j.snapshot().unwrap().seats[&seat()].incomplete_turns, 1);
        assert!(matches!(
            j.admission(seat(), Some(&bound())).unwrap(),
            Admission::Stop(_)
        ));
    }
    #[test]
    fn usage_outbox_survives_restart_and_acknowledges_only_confirmed_revisions() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        let sid = uuid::Uuid::new_v4().to_string();
        let info = SessionInfo {
            session_id: sid,
            model: Some("haiku".into()),
            cli_version: None,
            resumed: false,
        };
        journal.session(seat(), info.clone()).unwrap();
        let mut report = outcome(0.01);
        report.usage.cached_input_tokens = Some(100);
        report.usage.cache_creation_tokens = Some(30);
        report.usage.reasoning_tokens = Some(2);
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        journal
            .complete(seat(), 10, Some(&report), &SessionTelemetry::default())
            .unwrap();
        let first = journal.usage_outbox(seat()).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(
            (
                first[0].controller_epoch,
                first[0].revision,
                first[0].attempts,
                first[0].completed
            ),
            (7, 1, 1, 1)
        );
        assert_eq!(
            (
                first[0].input_tokens,
                first[0].cache_read_tokens,
                first[0].cache_creation_tokens,
                first[0].reasoning_tokens
            ),
            (Some(10), Some(100), Some(30), Some(2))
        );
        drop(journal); // crash after journal commit or transcript delivery, before acknowledgement
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        assert_eq!(journal.usage_outbox(seat()).unwrap(), first);
        journal
            .session(
                seat(),
                SessionInfo {
                    resumed: true,
                    ..info
                },
            )
            .unwrap();
        report.usage.cost_usd = Some(0.025);
        journal
            .reserve_decision(seat(), "GT1:OpStage2", 1000, "d2", 1)
            .unwrap();
        journal
            .complete(seat(), 10, Some(&report), &SessionTelemetry::default())
            .unwrap();
        let second = journal.usage_outbox(seat()).unwrap();
        assert_eq!(second.len(), 2);
        assert_eq!(
            (
                second[1].revision,
                second[1].input_tokens,
                second[1].cache_read_tokens,
                second[1].cache_creation_tokens,
                second[1].reasoning_tokens
            ),
            (2, Some(20), Some(200), Some(60), Some(4))
        );
        assert_eq!(second[1].reported_cost_usd, Some(0.025));
        assert!(journal.acknowledge_usage(seat(), 8, 2).is_err());
        assert!(journal.acknowledge_usage(seat(), 7, 3).is_err());
        assert_eq!(journal.usage_outbox(seat()).unwrap(), second);
        journal.acknowledge_usage(seat(), 7, 1).unwrap();
        assert_eq!(journal.usage_outbox(seat()).unwrap(), second[1..]);
        journal.acknowledge_usage(seat(), 7, 2).unwrap();
        journal.acknowledge_usage(seat(), 7, 2).unwrap();
        assert!(journal.usage_outbox(seat()).unwrap().is_empty());
        journal.recover_process(seat(), true).unwrap();
        report.usage.cost_usd = Some(0.005);
        journal
            .reserve_decision(seat(), "GT2:OpStage1", 1000, "d3", 1)
            .unwrap();
        journal
            .complete(seat(), 10, Some(&report), &SessionTelemetry::default())
            .unwrap();
        let third = journal.usage_outbox(seat()).unwrap();
        assert_eq!(third[0].revision, 3);
        assert!((third[0].reported_cost_usd.unwrap() - 0.03).abs() < 1e-10);
        assert_eq!(third[0].input_tokens, Some(30));
    }
    #[test]
    fn unusable_provider_counts_roll_back_and_become_an_unknown_turn() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        let mut report = outcome(0.01);
        report.usage.input_tokens = Some(u64::MAX);
        assert!(
            journal
                .complete(seat(), 10, Some(&report), &SessionTelemetry::default())
                .is_err()
        );
        assert!(journal.usage_outbox(seat()).unwrap().is_empty());
        assert!(
            journal.snapshot().unwrap().seats[&seat()]
                .inflight
                .is_some()
        );
        journal.interrupt(seat(), true).unwrap();
        let snapshot = journal.usage_outbox(seat()).unwrap()[0].clone();
        assert_eq!(
            (
                snapshot.attempts,
                snapshot.completed,
                snapshot.incomplete_turns
            ),
            (1, 0, 1)
        );
        assert_eq!(
            (snapshot.input_tokens, snapshot.reported_cost_usd),
            (None, None)
        );
    }
    #[test]
    fn interrupted_usage_is_durable_null_and_never_counted_twice() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        drop(journal);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        let snapshot = journal.usage_outbox(seat()).unwrap()[0].clone();
        assert_eq!(
            (
                snapshot.attempts,
                snapshot.completed,
                snapshot.incomplete_turns
            ),
            (1, 0, 1)
        );
        assert_eq!(
            (
                snapshot.input_tokens,
                snapshot.output_tokens,
                snapshot.reported_cost_usd
            ),
            (None, None, None)
        );
        journal.interrupt(seat(), true).unwrap();
        assert_eq!(
            journal.usage_outbox(seat()).unwrap(),
            vec![snapshot.clone()]
        );
        journal
            .reserve_decision(seat(), "GT1:OpStage2", 1000, "d2", 1)
            .unwrap();
        journal.interrupt(seat(), true).unwrap();
        drop(journal);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        let entries = journal.usage_outbox(seat()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            (
                entries[1].revision,
                entries[1].attempts,
                entries[1].incomplete_turns
            ),
            (2, 2, 2)
        );
        assert_eq!(journal.snapshot().unwrap().seats[&seat()].wall_millis, 2000);
    }
    #[tokio::test]
    async fn usage_delivery_confirms_before_ack_and_replays_after_a_dead_sink() {
        use cna_seats::transcript::{LocalTranscript, TranscriptSink};
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        journal
            .complete(
                seat(),
                10,
                Some(&outcome(0.01)),
                &SessionTelemetry::default(),
            )
            .unwrap();
        let store = LocalTranscript::detached();
        let sink = TranscriptSink::new(store.clone());
        journal.deliver_usage(seat(), &sink).await.unwrap();
        assert!(journal.usage_outbox(seat()).unwrap().is_empty());
        assert_eq!(store.seat_log(seat()).len(), 1);
        assert!(sink.stop_delivery().await.is_empty());
        journal
            .reserve_decision(seat(), "GT1:OpStage2", 1000, "d2", 1)
            .unwrap();
        journal
            .complete(
                seat(),
                10,
                Some(&outcome(0.025)),
                &SessionTelemetry::default(),
            )
            .unwrap();
        let committed = journal.usage_outbox(seat()).unwrap();
        assert!(journal.deliver_usage(seat(), &sink).await.is_err());
        assert_eq!(journal.usage_outbox(seat()).unwrap(), committed);
        drop(journal);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        let recovered = TranscriptSink::new(store.clone());
        journal.deliver_usage(seat(), &recovered).await.unwrap();
        assert!(journal.usage_outbox(seat()).unwrap().is_empty());
        let rows = store.seat_log(seat());
        assert_eq!(rows.len(), 2);
        let cna_protocol::TranscriptEntry::UsageSnapshot(snapshot) = &rows[1].entry else {
            panic!("missing typed usage")
        };
        assert_eq!(snapshot.as_ref(), &committed[0]);
        assert!(recovered.stop_delivery().await.is_empty());
    }
    #[test]
    fn fully_measured_refusal_is_not_completion_and_reset_cost_is_retained() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        let mut refused = outcome(0.10);
        refused.ok = false;
        refused.error = Some("measured refusal".into());
        journal
            .complete(seat(), 10, Some(&refused), &SessionTelemetry::default())
            .unwrap();
        let first = journal.usage_outbox(seat()).unwrap()[0].clone();
        assert_eq!(
            (first.attempts, first.completed, first.incomplete_turns),
            (1, 0, 0)
        );
        assert_eq!(
            (first.input_tokens, first.output_tokens),
            (Some(10), Some(5))
        );
        assert_eq!(first.reported_cost_usd, Some(0.10));
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        journal
            .complete(
                seat(),
                10,
                Some(&outcome(0.05)),
                &SessionTelemetry::default(),
            )
            .unwrap();
        let second = journal.usage_outbox(seat()).unwrap()[1].clone();
        assert_eq!(
            (
                second.revision,
                second.attempts,
                second.completed,
                second.incomplete_turns
            ),
            (2, 2, 1, 1)
        );
        assert_eq!(
            (second.input_tokens, second.output_tokens),
            (Some(20), Some(10))
        );
        assert!((second.reported_cost_usd.unwrap() - 0.15).abs() < 1e-10);
        drop(journal);
        let recovered = SessionJournal::recover(root.path(), &id).unwrap();
        assert_eq!(recovered.usage_outbox(seat()).unwrap(), vec![first, second]);
    }

    #[tokio::test]
    async fn cancelled_delivery_after_transcript_commit_replays_identical_usage() {
        use cna_seats::transcript::{
            LocalTranscript, TranscriptEntry, TranscriptSink, TranscriptStore,
        };
        use std::sync::Arc;
        use tokio::sync::Notify;
        struct CommitBeforeConfirmation {
            stored: Arc<LocalTranscript>,
            committed: Notify,
            confirmation: Notify,
        }
        #[async_trait::async_trait]
        impl TranscriptStore for CommitBeforeConfirmation {
            async fn append(
                &self,
                seat: SeatId,
                at: String,
                entry: TranscriptEntry,
            ) -> Result<u64, String> {
                let seq = self.stored.append(seat, at, entry).await?;
                self.committed.notify_one();
                self.confirmation.notified().await;
                Ok(seq)
            }
        }
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal = Arc::new(
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap(),
        );
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        journal
            .complete(
                seat(),
                10,
                Some(&outcome(0.01)),
                &SessionTelemetry::default(),
            )
            .unwrap();
        let committed = journal.usage_outbox(seat()).unwrap();
        let rows = LocalTranscript::detached();
        let store = Arc::new(CommitBeforeConfirmation {
            stored: rows.clone(),
            committed: Notify::new(),
            confirmation: Notify::new(),
        });
        let sink = TranscriptSink::new(store.clone());
        let task = {
            let journal = journal.clone();
            let sink = sink.clone();
            tokio::spawn(async move { journal.deliver_usage(seat(), &sink).await })
        };
        // Wait for the actual commit, not a delay. The timeout is only a hang guard.
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            store.committed.notified(),
        )
        .await
        .unwrap();
        assert_eq!(rows.seat_log(seat()).len(), 1);
        assert_eq!(journal.usage_outbox(seat()).unwrap(), committed);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(sink.stop_delivery().await.len(), 1);
        drop(journal);
        let recovered = SessionJournal::recover(root.path(), &id).unwrap();
        let replay = TranscriptSink::new(rows.clone());
        recovered.deliver_usage(seat(), &replay).await.unwrap();
        assert!(recovered.usage_outbox(seat()).unwrap().is_empty());
        let rows = rows.seat_log(seat());
        assert_eq!(rows.len(), 2);
        for row in rows {
            let TranscriptEntry::UsageSnapshot(snapshot) = row.entry else {
                panic!("missing usage");
            };
            assert_eq!(snapshot.as_ref(), &committed[0]);
        }
        assert_eq!(
            recovered.snapshot().unwrap().seats[&seat()].reported_cost_usd,
            0.01
        );
        assert!(replay.stop_delivery().await.is_empty());
    }
    #[test]
    fn old_journals_do_not_invent_auxiliary_or_missing_token_totals() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        journal
            .complete(
                seat(),
                10,
                Some(&outcome(0.01)),
                &SessionTelemetry::default(),
            )
            .unwrap();
        let path = journal.file().to_owned();
        drop(journal);
        let db = Connection::open(&path).unwrap();
        let mut old: serde_json::Value = serde_json::from_str(
            &db.query_row::<String, _, _>("SELECT state FROM journal WHERE id=1", [], |r| r.get(0))
                .unwrap(),
        )
        .unwrap();
        old["seats"][seat().to_string()]
            .as_object_mut()
            .unwrap()
            .remove("usage");
        db.execute("UPDATE journal SET state=?1 WHERE id=1", [old.to_string()])
            .unwrap();
        drop(db);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        let snapshot = journal.usage_outbox(seat()).unwrap()[0].clone();
        assert_eq!(
            (
                snapshot.attempts,
                snapshot.completed,
                snapshot.incomplete_turns
            ),
            (1, 1, 1)
        );
        assert_eq!(
            (
                snapshot.input_tokens,
                snapshot.cache_read_tokens,
                snapshot.cache_creation_tokens,
                snapshot.reasoning_tokens
            ),
            (None, None, None, None)
        );
        assert_eq!(snapshot.reported_cost_usd, Some(0.01));
    }
    #[test]
    fn automatic_answers_preserve_model_caps_cost_and_crash_reservations() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let mut limits = config();
        limits.tool_calls = 1;
        limits.max_turns = 1;
        let journal =
            SessionJournal::create(root.path(), &id, limits, &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        ToolBudget::charge(&journal, seat()).unwrap();
        journal
            .complete(
                seat(),
                10,
                Some(&outcome(0.01)),
                &SessionTelemetry::default(),
            )
            .unwrap();
        journal
            .reserve(seat(), "automatic", "GT1:OpStage1", 5000)
            .unwrap();
        assert!(ToolBudget::charge(&journal, seat()).is_err());
        journal.complete_automatic(seat(), 12, true).unwrap();
        let state = journal.snapshot().unwrap();
        let record = &state.seats[&seat()];
        assert_eq!((record.turns, record.calls, record.wall_millis), (1, 1, 22));
        assert!((record.reported_cost_usd - 0.01).abs() < 1e-9);
        assert_eq!(record.stages["GT1:OpStage1"].automatic_completed, 1);
        journal
            .reserve(seat(), "automatic", "GT1:OpStage1", 5000)
            .unwrap();
        drop(journal);
        let recovered = SessionJournal::recover(root.path(), &id).unwrap();
        let state = recovered.snapshot().unwrap();
        let record = &state.seats[&seat()];
        assert_eq!(
            (
                record.turns,
                record.calls,
                record.wall_millis,
                record.incomplete_turns
            ),
            (1, 1, 5022, 0)
        );
        assert_eq!(record.stages["GT1:OpStage1"].automatic_attempts, 2);
        assert_eq!(record.stages["GT1:OpStage1"].automatic_completed, 1);
        let old:StageUsage=serde_json::from_str(r#"{"attempts":1,"completed":1,"reported_cost_usd":0,"input_tokens":0,"output_tokens":0}"#).unwrap();
        assert_eq!(old.automatic_completed, 0);
    }
    #[test]
    fn hard_crash_keeps_charge_lease_and_lifetime_call_cap() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        assert!(SessionJournal::recover(root.path(), &id).is_err());
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 60_000, "d1", 2)
            .unwrap();
        for _ in 0..3 {
            journal.charge(seat()).unwrap();
        }
        assert!(
            journal
                .authorize(
                    seat(),
                    7,
                    "submit",
                    &serde_json::json!({"decision_id":"d1","revision":3})
                )
                .is_err()
        );
        assert!(
            journal
                .authorize(seat(), 8, "observe", &serde_json::json!({}))
                .is_err()
        );
        assert!(
            journal
                .authorize(
                    seat(),
                    7,
                    "submit",
                    &serde_json::json!({"decision_id":"d1","revision":2})
                )
                .is_ok()
        );
        drop(journal);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        let s = &journal.snapshot().unwrap().seats[&seat()];
        assert_eq!(
            (s.turns, s.calls, s.wall_millis, s.incomplete_turns),
            (1, 3, 60_000, 1)
        );
        assert!(journal.charge(seat()).is_err());
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 60_000, "d1", 3)
            .unwrap();
        for _ in 0..3 {
            journal.charge(seat()).unwrap();
        }
        assert!(journal.charge(seat()).is_err());
    }
    #[test]
    fn cumulative_cost_deltas_and_compactions_survive_same_session_resume() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        let sid = uuid::Uuid::new_v4().to_string();
        let info = SessionInfo {
            session_id: sid.clone(),
            model: Some("haiku".into()),
            cli_version: None,
            resumed: false,
        };
        journal.session(seat(), info.clone()).unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 60_000, "d1", 1)
            .unwrap();
        journal
            .complete(
                seat(),
                20,
                Some(&outcome(0.01)),
                &SessionTelemetry {
                    compactions: 1,
                    context_tokens: Some(1000),
                    ..SessionTelemetry::default()
                },
            )
            .unwrap();
        drop(journal);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        journal
            .session(
                seat(),
                SessionInfo {
                    resumed: true,
                    ..info
                },
            )
            .unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage2", 60_000, "d2", 1)
            .unwrap();
        journal
            .complete(
                seat(),
                30,
                Some(&outcome(0.025)),
                &SessionTelemetry {
                    compactions: 1,
                    ..SessionTelemetry::default()
                },
            )
            .unwrap();
        let s = &journal.snapshot().unwrap().seats[&seat()];
        assert!((s.reported_cost_usd - 0.025).abs() < 1e-10);
        assert!((s.stages["GT1:OpStage2"].reported_cost_usd - 0.015).abs() < 1e-10);
        assert_eq!((s.compactions, s.wall_millis, s.turns), (2, 50, 2));
        assert_eq!(s.session.as_ref().unwrap().session_id, sid);
    }
    #[test]
    fn unavailable_session_reseed_keeps_budget_and_caps_recovery() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal
            .reserve_decision(seat(), "setup", 60_000, "d1", 1)
            .unwrap();
        journal.charge(seat()).unwrap();
        journal
            .complete(seat(), 10, None, &SessionTelemetry::default())
            .unwrap();
        journal.recover_process(seat(), true).unwrap();
        journal.recover_process(seat(), false).unwrap();
        assert!(journal.recover_process(seat(), true).is_err());
        let s = &journal.snapshot().unwrap().seats[&seat()];
        assert_eq!((s.turns, s.calls, s.recoveries), (1, 1, 2));
        drop(journal);
        std::fs::write(SessionJournal::path(root.path(), &id).unwrap(), b"corrupt").unwrap();
        assert!(SessionJournal::recover(root.path(), &id).is_err());
    }
    #[test]
    fn measured_overrun_is_charged_and_survives_recovery() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal.reserve(seat(), "idle", "idle", 1000).unwrap();
        journal
            .complete(seat(), 1200, None, &SessionTelemetry::default())
            .unwrap();
        drop(journal);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        assert_eq!(journal.snapshot().unwrap().seats[&seat()].wall_millis, 1200);
        journal.reserve(seat(), "idle", "idle", 1000).unwrap();
        journal
            .complete(seat(), u64::MAX, None, &SessionTelemetry::default())
            .unwrap();
        assert!(journal.reserve(seat(), "idle", "idle", 1000).is_err());
    }

    #[test]
    fn actual_prechange_journal_deserializes_and_recovers_without_invented_timeout() {
        // Generated by eb65bef's unchanged SessionJournal serializer before this change.
        let text = include_str!("../tests/fixtures/journal-before-timeout-eb65bef.json");
        let state: JournalState = serde_json::from_str(text).unwrap();
        assert!(state.seats[&seat()].timeout_events.is_empty());
        assert_eq!(
            state.seats[&seat()].inflight.as_ref().unwrap().decision,
            Some(("fixture-decision".into(), 2))
        );
        let root = tempfile::tempdir().unwrap();
        let journal = SessionJournal::create(
            root.path(),
            &state.campaign_id,
            config(),
            &BTreeMap::from([(seat(), 7)]),
        )
        .unwrap();
        journal
            .db
            .lock()
            .unwrap()
            .execute("UPDATE journal SET state=?1 WHERE id=1", [text])
            .unwrap();
        drop(journal);
        let recovered = SessionJournal::recover(root.path(), &state.campaign_id)
            .unwrap()
            .snapshot()
            .unwrap();
        assert!(recovered.seats[&seat()].timeout_events.is_empty());
        assert!(recovered.seats[&seat()].inflight.is_none());
        assert_eq!(recovered.seats[&seat()].wall_millis, 1000);
        assert_eq!(recovered.seats[&seat()].incomplete_turns, 1);
    }
    #[test]
    fn timeout_record_commits_with_identical_completion_accounting_but_not_usage_payload() {
        let roots = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
        let id = uuid::Uuid::new_v4().to_string();
        let journals: Vec<_> = roots
            .iter()
            .map(|root| {
                let mut c = config();
                c.session.as_mut().unwrap().wall_seconds = 1;
                c.session.as_mut().unwrap().turn_seconds = 1;
                let j = SessionJournal::create(root.path(), &id, c, &BTreeMap::from([(seat(), 7)]))
                    .unwrap();
                assert_eq!(
                    j.reserve_decision(seat(), "private-stage", 60000, "private-decision", 9)
                        .unwrap(),
                    1000
                );
                j
            })
            .collect();
        journals[0]
            .complete(seat(), 1234, None, &SessionTelemetry::default())
            .unwrap();
        let diagnostic = TimeoutDiagnostic::legacy();
        journals[1]
            .complete_with_diagnostic(
                seat(),
                1234,
                None,
                &SessionTelemetry::default(),
                Some(TimeoutCompletion {
                    diagnostic: diagnostic.clone(),
                    requested_turn_ms: 60000,
                    supervisor_turn_elapsed_ms: 1233,
                }),
            )
            .unwrap();
        let old = journals[0].snapshot().unwrap();
        let new = journals[1].snapshot().unwrap();
        let record = &new.seats[&seat()];
        assert!(record.inflight.is_none());
        assert_eq!(record.timeout_events.len(), 1);
        let event = &record.timeout_events[0];
        assert_eq!(
            (
                event.campaign_id.as_str(),
                event.seat,
                event.controller_epoch,
                event.attempt
            ),
            (id.as_str(), seat(), 7, 1)
        );
        assert_eq!(
            event.reservation.decision,
            Some(("private-decision".into(), 9))
        );
        assert_eq!(event.reservation.stage, "private-stage");
        assert_eq!(
            (
                event.requested_turn_ms,
                event.effective_reserved_ms,
                event.supervisor_turn_elapsed_ms
            ),
            (60000, 1000, 1233)
        );
        assert_eq!(event.diagnostic, diagnostic);
        assert_eq!(record.wall_millis, 1234);
        let usage = serde_json::to_string(&journals[1].usage_outbox(seat()).unwrap()).unwrap();
        for private in [
            "private-decision",
            "private-stage",
            "timeout_events",
            "legacy_unclassified",
            "supervisor_turn_elapsed_ms",
        ] {
            assert!(!usage.contains(private));
        }
        let mut expected = serde_json::to_value(old).unwrap();
        let mut actual = serde_json::to_value(new).unwrap();
        for state in [&mut expected, &mut actual] {
            state["seats"][seat().to_string()]
                .as_object_mut()
                .unwrap()
                .remove("timeout_events");
        }
        assert_eq!(actual, expected);
        drop(journals);
        let recovered = SessionJournal::recover(roots[1].path(), &id)
            .unwrap()
            .snapshot()
            .unwrap();
        assert_eq!(recovered.seats[&seat()].timeout_events.len(), 1);
        assert_eq!(
            recovered.seats[&seat()].timeout_events[0].diagnostic,
            diagnostic
        );
    }
    #[test]
    fn failed_timeout_commit_retains_both_causes_and_rolls_back_event_accounting_outbox() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let j = SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
            .unwrap();
        j.reserve_decision(seat(), "private-stage", 1000, "private-decision", 9)
            .unwrap();
        let before = serde_json::to_value(j.snapshot().unwrap()).unwrap();
        j.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_completion BEFORE UPDATE ON journal BEGIN SELECT RAISE(ABORT, 'injected completion storage failure'); END;").unwrap();
        let diagnostic = TimeoutDiagnostic::transcript_confirmation(
            tokio::time::Instant::now(),
            std::time::Duration::from_secs(30),
        );
        let error = j
            .complete_with_diagnostic(
                seat(),
                11,
                None,
                &SessionTelemetry::default(),
                Some(TimeoutCompletion {
                    diagnostic: diagnostic.clone(),
                    requested_turn_ms: 1000,
                    supervisor_turn_elapsed_ms: 10,
                }),
            )
            .unwrap_err();
        assert_eq!(error.timeout, Some(diagnostic));
        assert!(
            error
                .storage
                .contains("injected completion storage failure")
        );
        assert_eq!(serde_json::to_value(j.snapshot().unwrap()).unwrap(), before);
        assert!(j.usage_outbox(seat()).unwrap().is_empty());
    }

    fn start_fixture() -> (tempfile::TempDir, SessionJournal, String) {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let j = SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
            .unwrap();
        (root, j, id)
    }

    fn captured_recovery_owner(
        j: &SessionJournal,
        error: cna_seats::driver::DriverError,
    ) -> crate::PendingJournalTurn {
        use cna_seats::operation::{Allowance, Operation};
        let operation_id = uuid::Uuid::new_v4().to_string();
        j.reserve_decision_authorized(
            seat(),
            "recovery-stage",
            1000,
            "original-decision",
            3,
            StartAuthorization {
                epoch: 7,
                operation_id: &operation_id,
            },
        )
        .unwrap();
        let operation = Operation::new(
            operation_id,
            7,
            Allowance::required(Some(
                tokio::time::Instant::now() + std::time::Duration::from_secs(1),
            ))
            .unwrap(),
        )
        .unwrap();
        operation.possible_start();
        let mut captured = outcome(0.02);
        captured.usage.cached_input_tokens = Some(0);
        captured.usage.cache_creation_tokens = Some(0);
        captured.usage.reasoning_tokens = Some(2);
        captured.usage.cost_basis_known = Some(true);
        crate::PendingJournalTurn {
            operation,
            supervisor_started: true, // Synthetic returned attempt, not native start evidence.
            outcome: Some(Err(error)),
            supervisor_observed: true, // Synthetic completed observation fixture.
            turn_started_at: None,
            captured_outcome: Some(captured),
            capture_in_service: false,
            capture_refusal: None,
            journal_revision: 0,
            elapsed_ms: 42,
            supervisor_turn_elapsed_ms: 41,
            requested_turn_ms: 1000,
            telemetry: SessionTelemetry::default(),
        }
    }

    #[test]
    fn public_timeout_with_outcome_still_rejects_without_changing_any_prefix() {
        let (_root, j, _id) = start_fixture();
        j.reserve_decision(seat(), "stage", 1000, "d", 1).unwrap();
        let before = serde_json::to_value(j.snapshot().unwrap()).unwrap();
        let error = j
            .complete_with_diagnostic(
                seat(),
                42,
                Some(&outcome(0.02)),
                &SessionTelemetry::default(),
                Some(TimeoutCompletion {
                    diagnostic: TimeoutDiagnostic::legacy(),
                    requested_turn_ms: 1000,
                    supervisor_turn_elapsed_ms: 41,
                }),
            )
            .unwrap_err();
        assert!(
            error
                .storage
                .contains("timeout completion requires a failed reserved turn")
        );
        assert_eq!(error.timeout, Some(TimeoutDiagnostic::legacy()));
        assert_eq!(serde_json::to_value(j.snapshot().unwrap()).unwrap(), before);
        assert!(j.usage_outbox(seat()).unwrap().is_empty());
    }

    #[test]
    fn captured_cli_success_accounts_usage_without_completing_failed_supervisor() {
        for error in [
            cna_seats::driver::DriverError::Timeout,
            cna_seats::driver::DriverError::Host(cna_seats::host::HostRefusal::Ownership(
                "original settlement failed".into(),
            )),
        ] {
            let (_root, j, _id) = start_fixture();
            let mut owner = captured_recovery_owner(&j, error);
            let original_error = owner
                .outcome
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap_err()
                .to_string();
            let original_deadline = owner.operation.allowance().deadline();
            assert!(
                owner
                    .commit(&j, seat(), false)
                    .unwrap_err()
                    .contains("supervision remains unresolved")
            );
            assert!(j.usage_outbox(seat()).unwrap().is_empty());
            assert!(owner.captured_outcome.as_ref().unwrap().ok);
            // Explicit synthetic original supervision disposition; never a timer/CLI proof.
            owner.commit(&j, seat(), true).unwrap();
            let state = j.snapshot().unwrap();
            let s = &state.seats[&seat()];
            assert_eq!(s.stages["recovery-stage"].completed, 0);
            assert_eq!(s.stages["recovery-stage"].input_tokens, 10);
            assert_eq!(s.stages["recovery-stage"].output_tokens, 5);
            assert_eq!(s.reported_cost_usd, 0.02);
            assert_eq!(s.incomplete_turns, 0);
            assert_eq!(s.incomplete_token_turns, 0);
            assert_eq!(s.start_accounting.as_ref().unwrap().possible, 1);
            assert_eq!(s.start_accounting.as_ref().unwrap().confirmed, 0);
            assert!(s.start_operations[owner.operation.id()].accounting_committed);
            assert_eq!(
                s.timeout_events.len(),
                usize::from(
                    owner
                        .outcome
                        .as_ref()
                        .unwrap()
                        .as_ref()
                        .unwrap_err()
                        .is_timeout()
                )
            );
            assert_eq!(
                owner
                    .outcome
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap_err()
                    .to_string(),
                original_error
            );
            assert_eq!(owner.operation.allowance().deadline(), original_deadline);
            assert!(owner.captured_outcome.as_ref().unwrap().ok);
            let prefix = serde_json::to_value(&state).unwrap();
            let outbox = serde_json::to_value(j.usage_outbox(seat()).unwrap()).unwrap();
            owner.commit(&j, seat(), true).unwrap();
            assert_eq!(serde_json::to_value(j.snapshot().unwrap()).unwrap(), prefix);
            assert_eq!(
                serde_json::to_value(j.usage_outbox(seat()).unwrap()).unwrap(),
                outbox
            );
        }
    }

    #[test]
    fn captured_recovery_storage_refusal_retains_timeout_body_and_original_atomic_prefix() {
        let (_root, j, _id) = start_fixture();
        let mut owner = captured_recovery_owner(&j, cna_seats::driver::DriverError::Timeout);
        owner.journal_revision = j
            .reconcile_start(
                seat(),
                owner.operation.id(),
                0,
                Some(owner.operation.start_evidence()),
                true,
            )
            .unwrap()
            .revision;
        let before = serde_json::to_value(j.snapshot().unwrap()).unwrap();
        j.db.lock().unwrap().execute_batch(r#"CREATE TRIGGER deny_captured BEFORE UPDATE ON journal WHEN instr(NEW.state, '"accounting_committed":true') > 0 BEGIN SELECT RAISE(ABORT, 'injected captured storage failure'); END;"#).unwrap();
        let error = owner.commit(&j, seat(), true).unwrap_err();
        assert!(error.contains("time limit"));
        assert!(error.contains("injected captured storage failure"));
        assert_eq!(serde_json::to_value(j.snapshot().unwrap()).unwrap(), before);
        assert!(j.usage_outbox(seat()).unwrap().is_empty());
        assert!(
            owner
                .outcome
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap_err()
                .is_timeout()
        );
        assert_eq!(
            owner.captured_outcome.as_ref().unwrap().usage.cost_usd,
            Some(0.02)
        );
        // Fixture storage repair is explicit and is not a production retry policy.
        j.db.lock()
            .unwrap()
            .execute_batch("DROP TRIGGER deny_captured;")
            .unwrap();
        owner.commit(&j, seat(), true).unwrap();
        let committed = serde_json::to_value(j.snapshot().unwrap()).unwrap();
        owner.commit(&j, seat(), true).unwrap();
        assert_eq!(
            serde_json::to_value(j.snapshot().unwrap()).unwrap(),
            committed
        );
    }

    #[test]
    fn captured_recovery_requires_original_reservation_and_preserves_unknown_measurements() {
        let (_root, j, _id) = start_fixture();
        let mut owner = captured_recovery_owner(&j, cna_seats::driver::DriverError::Timeout);
        owner.journal_revision = j
            .reconcile_start(
                seat(),
                owner.operation.id(),
                0,
                Some(owner.operation.start_evidence()),
                true,
            )
            .unwrap()
            .revision;
        let before = serde_json::to_value(j.snapshot().unwrap()).unwrap();
        assert!(
            j.complete_captured_operation(
                seat(),
                "stale",
                42,
                owner.captured_outcome.as_ref().unwrap(),
                &owner.telemetry,
                None
            )
            .unwrap_err()
            .storage
            .contains("reservation mismatch")
        );
        assert_eq!(serde_json::to_value(j.snapshot().unwrap()).unwrap(), before);
        owner.captured_outcome.as_mut().unwrap().usage.cost_usd = Some(f64::NAN);
        assert!(
            owner
                .commit(&j, seat(), true)
                .unwrap_err()
                .contains("invalid reported cost")
        );
        assert_eq!(serde_json::to_value(j.snapshot().unwrap()).unwrap(), before);
        assert!(j.usage_outbox(seat()).unwrap().is_empty());
        owner.captured_outcome.as_mut().unwrap().usage = Usage::default();
        owner.commit(&j, seat(), true).unwrap();
        let s = &j.snapshot().unwrap().seats[&seat()];
        assert_eq!(s.stages["recovery-stage"].completed, 0);
        assert_eq!(s.incomplete_turns, 1);
        assert_eq!(s.incomplete_token_turns, 1);
        assert!(owner.outcome.as_ref().unwrap().is_err());
    }

    #[test]
    fn legacy_start_migration_is_exact_idempotent_and_preserves_usage_prefix() {
        let (_root, j, _id) = start_fixture();
        j.reserve_decision(seat(), "original-stage", 1000, "d", 3)
            .unwrap();
        j.complete(
            seat(),
            1,
            Some(&outcome(0.01)),
            &SessionTelemetry::default(),
        )
        .unwrap();
        let mut state = j.snapshot().unwrap();
        state.version = 1;
        let s = state.seats.get_mut(&seat()).unwrap();
        s.turns = 17;
        s.start_accounting = None;
        s.stages.get_mut("original-stage").unwrap().attempts = 13;
        s.stages.get_mut("original-stage").unwrap().start_accounting = None;
        let old_usage = serde_json::to_value(&s.usage).unwrap();
        migrate_starts(&mut state).unwrap();
        let migrated = serde_json::to_value(&state).unwrap();
        migrate_starts(&mut state).unwrap();
        assert_eq!(serde_json::to_value(&state).unwrap(), migrated);
        let s = &state.seats[&seat()];
        assert_eq!(
            s.start_accounting
                .as_ref()
                .unwrap()
                .legacy_dispatch_attempts,
            17
        );
        assert_eq!(
            s.stages["original-stage"]
                .start_accounting
                .as_ref()
                .unwrap()
                .legacy_dispatch_attempts,
            13
        );
        assert_eq!((s.turns, s.stages["original-stage"].attempts), (17, 13));
        assert_eq!(serde_json::to_value(&s.usage).unwrap(), old_usage);
        assert_eq!(
            (
                s.start_accounting.as_ref().unwrap().possible,
                s.start_accounting.as_ref().unwrap().confirmed
            ),
            (0, 0)
        );
        state.version = 1;
        assert!(migrate_starts(&mut state).is_err());
    }

    #[test]
    fn authorization_commits_original_decision_and_rolls_back_invalid_identity_atomically() {
        let (_root, j, _id) = start_fixture();
        let before = serde_json::to_value(j.snapshot().unwrap()).unwrap();
        assert!(
            j.reserve_decision_authorized(
                seat(),
                "stage",
                1000,
                "d",
                4,
                StartAuthorization {
                    epoch: 8,
                    operation_id: &uuid::Uuid::new_v4().to_string()
                }
            )
            .is_err()
        );
        assert!(
            j.reserve_decision_authorized(
                seat(),
                "stage",
                1000,
                "d",
                4,
                StartAuthorization {
                    epoch: 7,
                    operation_id: "invalid"
                }
            )
            .is_err()
        );
        assert_eq!(serde_json::to_value(j.snapshot().unwrap()).unwrap(), before);
        let operation_id = uuid::Uuid::new_v4().to_string();
        j.reserve_decision_authorized(
            seat(),
            "stage",
            1000,
            "d",
            4,
            StartAuthorization {
                epoch: 7,
                operation_id: &operation_id,
            },
        )
        .unwrap();
        let state = j.snapshot().unwrap();
        let s = &state.seats[&seat()];
        let reservation = s.inflight.as_ref().unwrap();
        assert_eq!(reservation.decision, Some(("d".into(), 4)));
        assert_eq!(
            reservation.operation_id.as_deref(),
            Some(operation_id.as_str())
        );
        assert_eq!(
            s.start_operations[&operation_id].phase,
            StartPhase::Authorized
        );
        assert_eq!(s.start_accounting.as_ref().unwrap().provisional, 1);
        assert_eq!(
            s.stages["stage"]
                .start_accounting
                .as_ref()
                .unwrap()
                .provisional,
            1
        );
        assert_eq!((s.turns, s.stages["stage"].attempts), (1, 1));
        assert!(j.usage_outbox(seat()).unwrap().is_empty());
    }

    #[test]
    fn start_cas_duplicates_and_late_confirmation_conserve_each_private_delta() {
        use cna_seats::operation::{ConfirmationFrame, StartEvidence};
        let (_root, j, _id) = start_fixture();
        let operation_id = uuid::Uuid::new_v4().to_string();
        j.reserve_decision_authorized(
            seat(),
            "stage",
            1000,
            "d",
            4,
            StartAuthorization {
                epoch: 7,
                operation_id: &operation_id,
            },
        )
        .unwrap();
        let possible = j
            .reconcile_start(
                seat(),
                &operation_id,
                0,
                Some(StartEvidence::Possible),
                false,
            )
            .unwrap();
        assert_eq!(possible.revision, 1);
        let before = serde_json::to_value(j.snapshot().unwrap()).unwrap();
        assert!(
            j.reconcile_start(
                seat(),
                &operation_id,
                0,
                Some(StartEvidence::Confirmed {
                    frame: ConfirmationFrame::Assistant,
                    ordinal: 1
                }),
                false
            )
            .is_err()
        );
        assert_eq!(serde_json::to_value(j.snapshot().unwrap()).unwrap(), before);
        assert_eq!(
            j.reconcile_start(
                seat(),
                &operation_id,
                0,
                Some(StartEvidence::Possible),
                false
            )
            .unwrap()
            .revision,
            1
        );
        let settled = j
            .reconcile_start(seat(), &operation_id, 1, None, true)
            .unwrap();
        assert_eq!(settled.phase, StartPhase::SettledPossible);
        j.complete(seat(), 1, None, &SessionTelemetry::default())
            .unwrap();
        let old_prefix = serde_json::to_value(j.usage_outbox(seat()).unwrap()).unwrap();
        let committed_revision = j.start_accounting_state(seat(), &operation_id).unwrap().0;
        let confirmed = j
            .reconcile_start(
                seat(),
                &operation_id,
                committed_revision,
                Some(StartEvidence::Confirmed {
                    frame: ConfirmationFrame::ResultUsage,
                    ordinal: 9,
                }),
                false,
            )
            .unwrap();
        assert_eq!(confirmed.phase, StartPhase::SettledConfirmed);
        assert_eq!(
            confirmed.confirmation,
            Some((ConfirmationFrame::ResultUsage, 9))
        );
        j.reconcile_start(
            seat(),
            &operation_id,
            confirmed.revision,
            Some(StartEvidence::NotStarted),
            true,
        )
        .unwrap();
        let state = j.snapshot().unwrap();
        for totals in [
            state.seats[&seat()].start_accounting.as_ref().unwrap(),
            state.seats[&seat()].stages["stage"]
                .start_accounting
                .as_ref()
                .unwrap(),
        ] {
            assert_eq!(
                (totals.provisional, totals.possible, totals.confirmed),
                (0, 0, 1)
            );
        }
        assert_eq!(
            serde_json::to_value(j.usage_outbox(seat()).unwrap()).unwrap(),
            old_prefix
        );
        assert_eq!(state.seats[&seat()].turns, 1);
    }

    #[test]
    fn verified_zero_cancels_only_its_own_provisional_delta_never_legacy_or_public_attempts() {
        use cna_seats::operation::StartEvidence;
        let (_root, j, _id) = start_fixture();
        let operation_id = uuid::Uuid::new_v4().to_string();
        j.reserve_decision_authorized(
            seat(),
            "stage",
            1000,
            "d",
            1,
            StartAuthorization {
                epoch: 7,
                operation_id: &operation_id,
            },
        )
        .unwrap();
        let observed = j
            .reconcile_start(
                seat(),
                &operation_id,
                0,
                Some(StartEvidence::NotStarted),
                false,
            )
            .unwrap();
        assert_eq!(observed.phase, StartPhase::Authorized);
        assert!(
            j.complete(seat(), 1, None, &SessionTelemetry::default())
                .is_err()
        );
        let settled = j
            .reconcile_start(
                seat(),
                &operation_id,
                0,
                Some(StartEvidence::NotStarted),
                true,
            )
            .unwrap();
        assert_eq!(settled.phase, StartPhase::SettledZero);
        j.complete(seat(), 1, None, &SessionTelemetry::default())
            .unwrap();
        let state = j.snapshot().unwrap();
        let s = &state.seats[&seat()];
        assert_eq!((s.turns, s.stages["stage"].attempts), (1, 1));
        assert_eq!(
            s.start_accounting.as_ref().unwrap(),
            &StartTotals::default()
        );
        assert_eq!(j.usage_outbox(seat()).unwrap()[0].attempts, 1);
        assert!(
            j.private_start_report()
                .unwrap()
                .contains("upper bound on starts, ambiguous")
        );
    }

    #[test]
    fn crash_keeps_original_authorized_operation_and_blocks_replacement_without_join_inference() {
        let (root, j, id) = start_fixture();
        let operation_id = uuid::Uuid::new_v4().to_string();
        j.reserve_decision_authorized(
            seat(),
            "stage",
            1000,
            "d",
            1,
            StartAuthorization {
                epoch: 7,
                operation_id: &operation_id,
            },
        )
        .unwrap();
        drop(j);
        let recovered = SessionJournal::recover(root.path(), &id).unwrap();
        let s = recovered.snapshot().unwrap().seats[&seat()].clone();
        assert_eq!(
            s.start_operations[&operation_id].phase,
            StartPhase::Possible
        );
        assert_eq!(s.start_accounting.as_ref().unwrap().possible, 1);
        assert_eq!(
            s.inflight.as_ref().unwrap().operation_id.as_deref(),
            Some(operation_id.as_str())
        );
        assert_eq!(s.wall_millis, 1000);
        assert!(s.uncertain_budget_spend);
        assert!(
            recovered
                .reserve_decision_authorized(
                    seat(),
                    "stage",
                    1000,
                    "other",
                    2,
                    StartAuthorization {
                        epoch: 7,
                        operation_id: &uuid::Uuid::new_v4().to_string()
                    }
                )
                .is_err()
        );
        recovered.interrupt(seat(), false).unwrap();
        assert!(
            recovered.snapshot().unwrap().seats[&seat()]
                .inflight
                .is_some()
        );
        drop(recovered);
        let repeated = SessionJournal::recover(root.path(), &id).unwrap();
        let again = repeated.snapshot().unwrap().seats[&seat()].clone();
        assert_eq!(again.start_accounting, s.start_accounting);
        assert_eq!(again.incomplete_turns, s.incomplete_turns);
        assert_eq!(again.incomplete_token_turns, s.incomplete_token_turns);
        repeated.interrupt(seat(), true).unwrap();
        let settled = repeated.snapshot().unwrap();
        assert!(settled.seats[&seat()].inflight.is_none());
        assert_eq!(
            settled.seats[&seat()].start_operations[&operation_id].phase,
            StartPhase::SettledPossible
        );
        assert_eq!(
            settled.seats[&seat()]
                .start_accounting
                .as_ref()
                .unwrap()
                .possible,
            1
        );
        assert_eq!(settled.seats[&seat()].incomplete_turns, s.incomplete_turns);
        assert_eq!(
            settled.seats[&seat()].incomplete_token_turns,
            s.incomplete_token_turns
        );
        assert_eq!(
            serde_json::to_value(&settled.seats[&seat()].usage).unwrap(),
            serde_json::to_value(&s.usage).unwrap()
        );
    }

    #[test]
    fn terminal_memory_release_preserves_durable_unsettled_operation_and_reserved_budget() {
        let (root, journal, id) = start_fixture();
        let operation_id = uuid::Uuid::new_v4().to_string();
        journal
            .reserve_decision_authorized(
                seat(),
                "stage",
                1000,
                "original",
                1,
                StartAuthorization {
                    epoch: 7,
                    operation_id: &operation_id,
                },
            )
            .unwrap();
        // Failed evidence remains driver-owned after physical joins. The caller's
        // existing false disposition must not be replaced with interrupt(..., true).
        journal.interrupt(seat(), false).unwrap();
        let before = journal.snapshot().unwrap().seats[&seat()].clone();
        assert!(before.inflight.is_some());
        assert_eq!(
            before.start_operations[&operation_id].phase,
            StartPhase::Possible
        );
        assert!(!before.start_operations[&operation_id].accounting_committed);
        let liability = before.start_accounting.clone();
        // Process memory lifetime ends without a new settlement/write/shape.
        // The executable's separate inert subprocess control covers actual exit.
        drop(journal);
        let recovered = SessionJournal::recover(root.path(), &id).unwrap();
        let after = recovered.snapshot().unwrap().seats[&seat()].clone();
        assert_eq!(
            after.inflight.as_ref().unwrap().operation_id.as_deref(),
            Some(operation_id.as_str())
        );
        assert_eq!(
            after.start_operations[&operation_id].phase,
            StartPhase::Possible
        );
        assert!(!after.start_operations[&operation_id].accounting_committed);
        assert_eq!(after.start_accounting, liability);
        assert_eq!(after.wall_millis, before.wall_millis);
        assert_eq!(after.turns, before.turns);
        assert_eq!(after.stages["stage"].completed, 0);
        assert!(after.uncertain_budget_spend);
        // Recovery may add its existing interrupted-usage row. It must preserve
        // unknown liability and zero completed turns rather than infer settlement.
        let outbox = recovered.usage_outbox(seat()).unwrap();
        assert!(!outbox.is_empty());
        assert!(
            outbox
                .iter()
                .all(|snapshot| snapshot.completed == 0 && snapshot.attempts == 1)
        );
        let replacement_id = uuid::Uuid::new_v4().to_string();
        assert!(
            recovered
                .reserve_decision_authorized(
                    seat(),
                    "stage",
                    1000,
                    "replacement",
                    2,
                    StartAuthorization {
                        epoch: 7,
                        operation_id: &replacement_id
                    }
                )
                .is_err()
        );
        assert!(
            recovered.snapshot().unwrap().seats[&seat()]
                .inflight
                .is_some()
        );
        drop(recovered);
        let repeated = SessionJournal::recover(root.path(), &id).unwrap();
        assert_eq!(
            serde_json::to_value(repeated.usage_outbox(seat()).unwrap()).unwrap(),
            serde_json::to_value(outbox).unwrap()
        );
        assert_eq!(
            repeated.snapshot().unwrap().seats[&seat()].wall_millis,
            before.wall_millis
        );
    }

    #[test]
    fn start_update_storage_error_keeps_original_operation_counters_and_outbox() {
        use cna_seats::operation::StartEvidence;
        let (_root, j, _id) = start_fixture();
        let operation_id = uuid::Uuid::new_v4().to_string();
        j.reserve_decision_authorized(
            seat(),
            "stage",
            1000,
            "d",
            1,
            StartAuthorization {
                epoch: 7,
                operation_id: &operation_id,
            },
        )
        .unwrap();
        let before = serde_json::to_value(j.snapshot().unwrap()).unwrap();
        j.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_start BEFORE UPDATE ON journal BEGIN SELECT RAISE(ABORT, 'injected start storage failure'); END;").unwrap();
        assert!(
            j.reconcile_start(
                seat(),
                &operation_id,
                0,
                Some(StartEvidence::Possible),
                false
            )
            .unwrap_err()
            .contains("injected start storage failure")
        );
        assert_eq!(serde_json::to_value(j.snapshot().unwrap()).unwrap(), before);
    }

    #[test]
    fn canonical_returned_outcome_survives_completion_storage_failure_without_reauthorization() {
        use cna_seats::operation::{Allowance, ConfirmationFrame, Operation};
        let (_root, j, _id) = start_fixture();
        let operation_id = uuid::Uuid::new_v4().to_string();
        j.reserve_decision_authorized(
            seat(),
            "stage",
            1000,
            "d",
            1,
            StartAuthorization {
                epoch: 7,
                operation_id: &operation_id,
            },
        )
        .unwrap();
        let operation = Operation::new(
            operation_id.clone(),
            7,
            Allowance::required(Some(
                tokio::time::Instant::now() + std::time::Duration::from_secs(1),
            ))
            .unwrap(),
        )
        .unwrap();
        operation.possible_start();
        let origin = operation.next_origin().unwrap();
        assert!(operation.confirm(&origin, ConfirmationFrame::ResultUsage));
        let mut owner = Some(crate::PendingJournalTurn {
            operation,
            supervisor_started: true, // Synthetic returned attempt, not native start evidence.
            outcome: Some(Ok(outcome(0.02))),
            supervisor_observed: true, // Synthetic completed observation fixture.
            turn_started_at: None,
            captured_outcome: None,
            capture_in_service: false,
            capture_refusal: None,
            journal_revision: 0,
            elapsed_ms: 1,
            supervisor_turn_elapsed_ms: 1,
            requested_turn_ms: 1000,
            telemetry: SessionTelemetry::default(),
        });
        let retained = owner.as_mut().unwrap();
        retained.journal_revision = j
            .reconcile_start(
                seat(),
                retained.operation.id(),
                0,
                Some(retained.operation.start_evidence()),
                true,
            )
            .unwrap()
            .revision;
        j.db.lock().unwrap().execute_batch(r#"CREATE TRIGGER deny_outcome BEFORE UPDATE ON journal WHEN instr(NEW.state, '"accounting_committed":true') > 0 BEGIN SELECT RAISE(ABORT, 'injected outcome storage failure'); END;"#).unwrap();
        let error = retained.commit(&j, seat(), true).unwrap_err();
        assert!(error.contains("injected outcome storage failure"));
        assert_eq!(retained.operation.id(), operation_id);
        assert_eq!(
            retained
                .outcome
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .usage
                .cost_usd,
            Some(0.02)
        );
        assert!(j.usage_outbox(seat()).unwrap().is_empty());
        assert_eq!(
            j.snapshot().unwrap().seats[&seat()]
                .start_accounting
                .as_ref()
                .unwrap()
                .confirmed,
            1
        );
        // This is an explicit fixture storage repair, not an automatic production retry.
        j.db.lock()
            .unwrap()
            .execute_batch("DROP TRIGGER deny_outcome;")
            .unwrap();
        retained.commit(&j, seat(), true).unwrap();
        assert!(j.start_accounting_committed(seat(), &operation_id).unwrap());
        let committed_prefix = serde_json::to_value(j.usage_outbox(seat()).unwrap()).unwrap();
        // Keep the same owner to model loss of the caller's commit ACK, then observe the
        // actual original operation marker without adding its usage a second time.
        retained.commit(&j, seat(), true).unwrap();
        assert_eq!(
            serde_json::to_value(j.usage_outbox(seat()).unwrap()).unwrap(),
            committed_prefix
        );
        let canonical = owner.take().unwrap().outcome.unwrap().unwrap();
        assert_eq!(canonical.usage.cost_usd, Some(0.02));
        let state = j.snapshot().unwrap();
        assert_eq!(state.seats[&seat()].turns, 1);
        assert_eq!(
            state.seats[&seat()]
                .start_accounting
                .as_ref()
                .unwrap()
                .confirmed,
            1
        );
        assert_eq!(
            j.usage_outbox(seat()).unwrap()[0].reported_cost_usd,
            Some(0.02)
        );
    }

    #[test]
    fn malformed_private_counter_or_stage_refuses_without_reinterpreting_legacy_values() {
        let (_root, j, _id) = start_fixture();
        let operation_id = uuid::Uuid::new_v4().to_string();
        j.reserve_decision_authorized(
            seat(),
            "stage",
            1000,
            "d",
            1,
            StartAuthorization {
                epoch: 7,
                operation_id: &operation_id,
            },
        )
        .unwrap();
        let original = j.snapshot().unwrap();
        let mut missing = original.clone();
        missing
            .seats
            .get_mut(&seat())
            .unwrap()
            .stages
            .get_mut("stage")
            .unwrap()
            .start_accounting = None;
        assert!(migrate_starts(&mut missing).is_err());
        let mut wrong = original.clone();
        wrong
            .seats
            .get_mut(&seat())
            .unwrap()
            .start_accounting
            .as_mut()
            .unwrap()
            .possible = 1;
        assert!(migrate_starts(&mut wrong).is_err());
        let mut stale = original.clone();
        stale
            .seats
            .get_mut(&seat())
            .unwrap()
            .start_operations
            .get_mut(&operation_id)
            .unwrap()
            .epoch = 6;
        assert!(migrate_starts(&mut stale).is_err());
        assert_eq!(
            serde_json::to_value(j.snapshot().unwrap()).unwrap(),
            serde_json::to_value(original).unwrap()
        );
    }

    #[test]
    fn retained_timeout_requires_actual_cleanup_and_preserves_original_measurements_and_prefix() {
        use cna_seats::operation::{Allowance, Operation};
        let (_root, j, _id) = start_fixture();
        let operation_id = uuid::Uuid::new_v4().to_string();
        j.reserve_decision_authorized(
            seat(),
            "stage",
            1000,
            "d",
            1,
            StartAuthorization {
                epoch: 7,
                operation_id: &operation_id,
            },
        )
        .unwrap();
        let allowance = Allowance::required(Some(
            tokio::time::Instant::now() + std::time::Duration::from_secs(1),
        ))
        .unwrap();
        let original_deadline = allowance.deadline();
        let operation = Operation::new(operation_id.clone(), 7, allowance).unwrap();
        operation.possible_start();
        let mut owner = crate::PendingJournalTurn {
            operation,
            supervisor_started: true, // Synthetic returned attempt, not native start evidence.
            outcome: Some(Err(cna_seats::driver::DriverError::Timeout)),
            supervisor_observed: true, // Synthetic completed observation fixture.
            turn_started_at: None,
            captured_outcome: None,
            capture_in_service: false,
            capture_refusal: None,
            journal_revision: 0,
            elapsed_ms: 42,
            supervisor_turn_elapsed_ms: 41,
            requested_turn_ms: 1000,
            telemetry: SessionTelemetry::default(),
        };
        assert!(
            owner
                .commit(&j, seat(), false)
                .unwrap_err()
                .contains("supervision remains unresolved")
        );
        assert!(!j.start_accounting_committed(seat(), &operation_id).unwrap());
        assert!(j.snapshot().unwrap().seats[&seat()].inflight.is_some());
        assert!(j.usage_outbox(seat()).unwrap().is_empty());
        assert_eq!(owner.operation.allowance().deadline(), original_deadline);
        // Synthetic actual-supervision input exercises the handoff, never native proof.
        owner.commit(&j, seat(), true).unwrap();
        let committed = j.snapshot().unwrap();
        let s = &committed.seats[&seat()];
        assert_eq!(s.wall_millis, 42);
        assert_eq!(s.timeout_events.len(), 1);
        assert_eq!(s.timeout_events[0].requested_turn_ms, 1000);
        assert_eq!(s.timeout_events[0].supervisor_turn_elapsed_ms, 41);
        assert_eq!(
            s.timeout_events[0].diagnostic,
            cna_seats::driver::TimeoutDiagnostic::legacy()
        );
        assert_eq!(
            s.start_operations[&operation_id].phase,
            StartPhase::SettledPossible
        );
        assert!(s.start_operations[&operation_id].accounting_committed);
        assert!(owner.outcome.as_ref().unwrap().is_err());
        assert_eq!(owner.operation.allowance().deadline(), original_deadline);
        owner.commit(&j, seat(), true).unwrap();
        assert_eq!(
            serde_json::to_value(j.snapshot().unwrap()).unwrap(),
            serde_json::to_value(committed).unwrap()
        );
    }

    #[test]
    fn lost_old_completion_ack_never_completes_or_releases_a_newer_operation() {
        use cna_seats::operation::{Allowance, Operation};
        let (_root, j, _id) = start_fixture();
        let old_id = uuid::Uuid::new_v4().to_string();
        j.reserve_decision_authorized(
            seat(),
            "stage",
            1000,
            "d",
            1,
            StartAuthorization {
                epoch: 7,
                operation_id: &old_id,
            },
        )
        .unwrap();
        let operation = Operation::new(
            old_id.clone(),
            7,
            Allowance::required(Some(
                tokio::time::Instant::now() + std::time::Duration::from_secs(1),
            ))
            .unwrap(),
        )
        .unwrap();
        operation.possible_start();
        let mut old = crate::PendingJournalTurn {
            operation,
            supervisor_started: true, // Synthetic returned attempt, not native start evidence.
            outcome: Some(Ok(outcome(0.02))),
            supervisor_observed: true, // Synthetic completed observation fixture.
            turn_started_at: None,
            captured_outcome: None,
            capture_in_service: false,
            capture_refusal: None,
            journal_revision: 0,
            elapsed_ms: 1,
            supervisor_turn_elapsed_ms: 1,
            requested_turn_ms: 1000,
            telemetry: SessionTelemetry::default(),
        };
        old.commit(&j, seat(), true).unwrap();
        let new_id = uuid::Uuid::new_v4().to_string();
        // Journal-only fixture: production's foreground-owner gate forbids this overlap.
        j.reserve_decision_authorized(
            seat(),
            "stage",
            1000,
            "new",
            2,
            StartAuthorization {
                epoch: 7,
                operation_id: &new_id,
            },
        )
        .unwrap();
        let before = j.snapshot().unwrap();
        old.commit(&j, seat(), true).unwrap();
        assert_eq!(
            serde_json::to_value(j.snapshot().unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        assert!(!j.start_accounting_committed(seat(), &new_id).unwrap());
        assert!(j.start_accounting_committed(seat(), &old_id).unwrap());
        // Even a settled replacement cannot be completed by an old caller between
        // its snapshot read and transaction. No usage or marker is changed.
        j.reconcile_start(
            seat(),
            &new_id,
            0,
            Some(cna_seats::operation::StartEvidence::Possible),
            true,
        )
        .unwrap();
        let replacement = j.snapshot().unwrap();
        assert!(
            j.complete_bound_operation(
                seat(),
                Some(&old_id),
                1,
                None,
                &SessionTelemetry::default(),
                None
            )
            .unwrap_err()
            .storage
            .contains("reservation mismatch")
        );
        assert_eq!(
            serde_json::to_value(j.snapshot().unwrap()).unwrap(),
            serde_json::to_value(replacement).unwrap()
        );
    }
}
