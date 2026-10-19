//! Durable per-seat supervisor. The CLI owns compaction; the game owns notebooks.
use crate::{Demo, budget::Admission, combine_results};
use cna_core::ids::SeatId;
use cna_seats::{
    driver::{CliKind, DriverError, SeatDriver, TimeoutDiagnostic, measured_millis},
    run::PromptBuilder,
};
use cna_server::CampaignStatus;
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, watch},
    time::Instant,
};

fn elapsed(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .saturating_add(1)
}
fn unavailable(text: &str) -> bool {
    text.to_ascii_lowercase()
        .contains("no conversation found with session id")
}

fn retryable_death(error: &DriverError) -> bool {
    let DriverError::Died(text) = error else {
        return false;
    };
    let text = text.to_ascii_lowercase();
    ![
        "quota",
        "rate limit",
        "usage limit",
        "not logged",
        "authentication",
        "unauthorized",
        "invalid api key",
        "insufficient credit",
    ]
    .iter()
    .any(|word| text.contains(word))
}
/// Observe which timer branch actually returned. An inner diagnostic stays intact.
#[cfg(test)]
async fn bounded_turn(
    driver: &mut dyn SeatDriver,
    prompt: &str,
    reserved_ms: u64,
    start: Instant,
) -> Result<cna_seats::driver::TurnOutcome, DriverError> {
    bounded_turn_until(
        driver,
        prompt,
        reserved_ms,
        start,
        start + Duration::from_millis(reserved_ms),
    )
    .await
}

async fn bounded_turn_until(
    driver: &mut dyn SeatDriver,
    prompt: &str,
    reserved_ms: u64,
    start: Instant,
    deadline: Instant,
) -> Result<cna_seats::driver::TurnOutcome, DriverError> {
    match tokio::time::timeout_at(
        deadline.min(start + Duration::from_millis(reserved_ms)),
        driver.run_turn(prompt, Duration::from_millis(reserved_ms)),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => Err(DriverError::TimeoutAt(TimeoutDiagnostic::turn_reservation(
            start,
            reserved_ms,
            driver.turn_progress(),
        ))),
    }
}
fn completion_failure(
    original: Option<&DriverError>,
    storage: &crate::journal::CompletionError,
) -> String {
    match original {
        Some(error) => format!("{error}; {storage}"),
        None => storage.to_string(),
    }
}

#[derive(Clone, Copy)]
enum JournalInterruption {
    BindingChanged,
    CampaignStopped,
    WallTimeout,
    BorrowCanceled,
}
impl JournalInterruption {
    fn message(self) -> &'static str {
        match self {
            Self::BindingChanged => "controller binding changed or paused; old session stopped",
            Self::CampaignStopped => "campaign session stopped; journal retained for resume",
            Self::WallTimeout => "durable wall-clock budget exhausted",
            Self::BorrowCanceled => {
                "original supervisor borrow canceled before returned turn outcome"
            }
        }
    }
    fn error(self) -> DriverError {
        match self {
            // The whole-run timer supplies no turn/phase measurement; preserve that
            // uncertainty instead of fabricating a configured-turn diagnostic.
            Self::WallTimeout => DriverError::Timeout,
            other => DriverError::Host(cna_seats::host::HostRefusal::Ownership(
                other.message().into(),
            )),
        }
    }
    fn storage_bytes() -> usize {
        [
            Self::BindingChanged,
            Self::CampaignStopped,
            Self::WallTimeout,
            Self::BorrowCanceled,
        ]
        .into_iter()
        .map(|cause| cause.message().len())
        .max()
        .unwrap_or(0)
        .saturating_add(std::mem::size_of::<String>())
    }
}

// Fixed diagnostic/argument/return overlaps for retaining the timeout before
// callback enrichment. Admit BEFORE host binding/driver effects, never after a
// timer win. This is not complete async-frame/library/allocator fit evidence.
const JOURNAL_TIMEOUT_WORKSPACE_BYTES: usize = 3 * std::mem::size_of::<TimeoutDiagnostic>()
    + std::mem::size_of::<Option<cna_seats::driver::TurnProgress>>()
    + 4 * std::mem::size_of::<Instant>()
    + std::mem::size_of::<u64>()
    + 2 * std::mem::size_of::<Result<(), String>>();
// The callback return overlaps the canonical capture/refusal slot. Header
// admission is not a bound on driver error bodies or diagnostic allocations.
const JOURNAL_CAPTURE_WORKSPACE_BYTES: usize = std::mem::size_of::<
    Result<Option<cna_seats::driver::TurnOutcome>, DriverError>,
>() + std::mem::size_of::<Result<(), String>>()
    + 4 * std::mem::size_of::<usize>();

/// Private SOURCE assembly only. The public startup path is not redirected until
/// canonical root installation, original journal receipts/CAS and full creation
/// fit are assembled. A returned start result is not native settlement evidence.
#[allow(dead_code)]
struct PendingJournalStart {
    operation: cna_seats::operation::Operation,
    started_at: Instant,
    preparation_started: bool,
    preparation_result: Option<Result<(), cna_seats::operation::OperationError>>,
    host_context_started: bool,
    host_context_result: Option<Result<cna_seats::host::HostDriverContext, String>>,
    host_context_validated: bool,
    host_binding_started: bool,
    host_binding_result: Option<Result<(), DriverError>>,
    cleanup_started: bool,
    cleanup_result: Option<Result<(), DriverError>>,
    supervisor_started: bool,
    outcome: Option<Result<cna_seats::driver::SessionInfo, DriverError>>,
    supervisor_observed: bool,
    elapsed_ms: u64,
    telemetry: cna_seats::driver::SessionTelemetry,
    /// Sticky BEFORE the storage callback. An empty receipt after unwind is
    /// unresolved ownership, never authorization to issue another write.
    storage_started: bool,
    storage_result: Option<Result<(), String>>,
    receipt: Option<PendingStartupReceipt>,
}

/// Canonical per-seat startup storage. No task/queue or ownership-release route.
/// Shared inspection is fixed metadata; a checked-out slot is NEVER empty.
pub(super) struct JournalStartRoot {
    seat: SeatId,
    epoch: u64,
    slot: std::sync::Mutex<JournalStartSlot>,
}

struct JournalStartSlot {
    owner: Option<PendingJournalStart>,
    in_service: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JournalStartPosition {
    Empty,
    Installed,
    InService,
    Unknown,
}

impl JournalStartPosition {
    /// Fixed metadata only. Uncertainty and an active restore lease retain
    /// ownership; neither a receipt ACK nor a returned driver result releases it.
    pub(super) fn is_owned(self) -> bool {
        self != Self::Empty
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JournalStartRefusal {
    Identity,
    Busy,
    AlreadyOwned,
}

/// MOVE the same owner out under a short metadata lock, then release the guard
/// before journal/driver callbacks, file I/O or await. Drop restores it on return,
/// cancellation and unwind, without clearing results or settling any liability.
struct JournalStartLease<'a> {
    root: &'a JournalStartRoot,
    owner: Option<PendingJournalStart>,
}

#[allow(dead_code)] // SOURCE installation bridge, public startup not redirected.
impl JournalStartRoot {
    pub(super) fn empty(seat: SeatId, epoch: u64) -> Self {
        Self {
            seat,
            epoch,
            slot: std::sync::Mutex::new(JournalStartSlot {
                owner: None,
                in_service: false,
            }),
        }
    }

    /// Refusal preserves the caller's candidate. Only a pristine receipt-backed
    /// creation owner can move in, BEFORE preparation or any attempted effect.
    fn install(
        &self,
        candidate: &mut Option<PendingJournalStart>,
    ) -> Result<(), JournalStartRefusal> {
        let owner = candidate.as_ref().ok_or(JournalStartRefusal::Identity)?;
        let receipt = owner
            .receipt
            .as_ref()
            .ok_or(JournalStartRefusal::Identity)?;
        if receipt.seat != self.seat
            || receipt.epoch != self.epoch
            || owner.preparation_started
            || owner.preparation_result.is_some()
            || owner.host_context_started
            || owner.host_context_result.is_some()
            || owner.host_context_validated
            || owner.host_binding_started
            || owner.host_binding_result.is_some()
            || owner.cleanup_started
            || owner.cleanup_result.is_some()
            || owner.supervisor_started
            || owner.outcome.is_some()
            || owner.supervisor_observed
            || owner.storage_started
            || owner.storage_result.is_some()
            || receipt.binding_started
            || receipt.binding_result.is_some()
            || receipt.completion_started
            || receipt.completion_expected.is_some()
            || receipt.completion_result.is_some()
            || receipt.binding_lookup_started
            || receipt.binding_lookup_result.is_some()
            || receipt.completion_lookup_started
            || receipt.completion_lookup_result.is_some()
        {
            return Err(JournalStartRefusal::Identity);
        }
        let mut slot = self
            .slot
            .try_lock()
            .map_err(|_| JournalStartRefusal::Busy)?;
        if slot.in_service || slot.owner.is_some() {
            return Err(JournalStartRefusal::AlreadyOwned);
        }
        slot.owner = candidate.take(); // Plain MOVE; no fallible step/callback follows.
        Ok(())
    }

    fn checkout(
        &self,
        original: &cna_seats::operation::Operation,
    ) -> Result<JournalStartLease<'_>, JournalStartRefusal> {
        let mut slot = self
            .slot
            .try_lock()
            .map_err(|_| JournalStartRefusal::Busy)?;
        if slot.in_service {
            return Err(JournalStartRefusal::AlreadyOwned);
        }
        if slot
            .owner
            .as_ref()
            .is_none_or(|owner| !owner.operation.same_instance(original))
        {
            return Err(JournalStartRefusal::Identity);
        }
        // Construct restore guard BEFORE MOVE; do not expose a replacement-ready
        // interval. Only this guard can restore while in_service is set.
        let mut lease = JournalStartLease {
            root: self,
            owner: None,
        };
        slot.in_service = true;
        lease.owner = slot.owner.take();
        drop(slot);
        Ok(lease)
    }

    pub(super) fn position(&self) -> JournalStartPosition {
        let Ok(slot) = self.slot.try_lock() else {
            return JournalStartPosition::Unknown;
        };
        if slot.in_service {
            JournalStartPosition::InService
        } else if slot.owner.is_some() {
            JournalStartPosition::Installed
        } else {
            JournalStartPosition::Empty
        }
    }
}

impl JournalStartLease<'_> {
    fn owner_mut(&mut self) -> &mut PendingJournalStart {
        self.owner
            .as_mut()
            .expect("private startup restore lease owner")
    }
}

impl Drop for JournalStartLease<'_> {
    fn drop(&mut self) {
        // This lock protects only inline ownership MOVEs: no callback, I/O,
        // join, await, formatting or worker wait. Recover poison solely to
        // preserve canonical evidence; inspection/admission still refuses it.
        let mut slot = match self.root.slot.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        slot.owner = self.owner.take();
        slot.in_service = false;
    }
}

/// Trusted creation inputs and independent first-result destinations. The future
/// actual root moves this context into its owner BEFORE borrowing preparation.
/// No field is recovered from CLI output or a session-id equality guess.
#[allow(dead_code)]
struct PendingStartupReceipt {
    journal: std::sync::Arc<crate::journal::SessionJournal>,
    campaign_id: String,
    seat: SeatId,
    epoch: u64,
    bound: crate::journal::StartupReceipt,
    binding_started: bool,
    binding_result: Option<Result<(), String>>,
    completion_started: bool,
    completion_expected: Option<crate::journal::StartupReceipt>,
    completion_result: Option<Result<crate::journal::StartupReceipt, String>>,
    binding_lookup_started: bool,
    binding_lookup_result: Option<Result<(), String>>,
    completion_lookup_started: bool,
    completion_lookup_result: Option<Result<(), String>>,
}

#[allow(dead_code)]
impl PendingStartupReceipt {
    fn unattempted(
        journal: std::sync::Arc<crate::journal::SessionJournal>,
        campaign_id: String,
        seat: SeatId,
        epoch: u64,
        bound: crate::journal::StartupReceipt,
    ) -> Self {
        Self {
            journal,
            campaign_id,
            seat,
            epoch,
            bound,
            binding_started: false,
            binding_result: None,
            completion_started: false,
            completion_expected: None,
            completion_result: None,
            binding_lookup_started: false,
            binding_lookup_result: None,
            completion_lookup_started: false,
            completion_lookup_result: None,
        }
    }

    fn key<'a>(
        &'a self,
        original: &'a cna_seats::operation::Operation,
    ) -> crate::journal::StartupReceiptKey<'a> {
        crate::journal::StartupReceiptKey {
            campaign_id: &self.campaign_id,
            seat: self.seat,
            epoch: self.epoch,
            original,
        }
    }

    fn original_bind_returned_ok(&self) -> bool {
        self.binding_started
            && matches!(self.binding_result, Some(Ok(())))
            && !self.binding_lookup_started
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupReceiptRefusal {
    Unprepared,
    Unbound,
    Unobserved,
    AlreadyOwned,
    JournalFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionStorageRefusal {
    Identity,
    Unprepared,
    Unobserved,
    NotSuccessful,
    AlreadyOwned,
    StorageFailed,
    JournalBinding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupContextRefusal {
    Unprepared,
    Unbound,
    AlreadyOwned,
    ContextFailed,
    BindingFailed,
    Identity,
}

#[allow(dead_code)] // SOURCE preparation, not public startup/root adoption.
impl PendingJournalStart {
    /// Install this value in the canonical root before borrowing preparation.
    /// Construction allocates no body and invokes no driver or storage callback.
    fn unprepared(operation: cna_seats::operation::Operation, started_at: Instant) -> Self {
        Self {
            operation,
            started_at,
            preparation_started: false,
            preparation_result: None,
            host_context_started: false,
            host_context_result: None,
            host_context_validated: false,
            host_binding_started: false,
            host_binding_result: None,
            cleanup_started: false,
            cleanup_result: None,
            supervisor_started: false,
            outcome: None,
            supervisor_observed: false,
            elapsed_ms: 0,
            telemetry: Default::default(),
            storage_started: false,
            storage_result: None,
            receipt: None,
        }
    }

    fn unprepared_with_receipt(
        operation: cna_seats::operation::Operation,
        started_at: Instant,
        receipt: PendingStartupReceipt,
    ) -> Self {
        let mut owner = Self::unprepared(operation, started_at);
        owner.receipt = Some(receipt); // Plain MOVE; no callback/allocation.
        owner
    }

    /// Borrow an already-installed owner. A failed/in-service preparation never
    /// drops its original association or opens another charge/dispatch attempt.
    fn prepare(&mut self) -> Result<(), cna_seats::operation::OperationError> {
        use cna_seats::operation::OperationError;
        if self.preparation_started || self.preparation_result.is_some() {
            return Err(OperationError::CreditOwned);
        }
        self.preparation_started = true;
        // Fixed inline/header overlap BEFORE driver effects. Input-growing
        // SessionInfo/telemetry bodies, async frames, journal/SQL/library and
        // allocator costs still need the complete ORIGINAL creation-time plan.
        // This private partial charge is NOT a complete-fit witness or late reserve.
        let result = self.operation.charge(
            std::mem::size_of::<Self>()
                + 2 * std::mem::size_of::<Result<cna_seats::driver::SessionInfo, DriverError>>()
                + std::mem::size_of::<Result<(), DriverError>>()
                + std::mem::size_of::<Result<(), DriverError>>()
                + std::mem::size_of::<Result<bool, StartupCleanupRefusal>>()
                + std::mem::size_of::<cna_seats::driver::SessionTelemetry>()
                + 2 * std::mem::size_of::<Instant>()
                + 3 * std::mem::size_of::<usize>()
                + std::mem::size_of::<Result<(), String>>()
                + std::mem::size_of::<Result<(), SessionStorageRefusal>>()
                + 2 * std::mem::size_of::<Result<(), OperationError>>()
                + 4 * std::mem::size_of::<Result<(), String>>()
                + 2 * std::mem::size_of::<Result<crate::journal::StartupReceipt, String>>()
                + std::mem::size_of::<Result<(), StartupReceiptRefusal>>()
                + std::mem::size_of::<JournalStartRoot>()
                + std::mem::size_of::<JournalStartLease<'_>>()
                + std::mem::size_of::<Result<cna_seats::host::HostDriverContext, String>>()
                + std::mem::size_of::<Result<(), StartupContextRefusal>>()
                + std::mem::size_of::<cna_seats::host::HostDriverContext>()
                + std::mem::size_of::<Result<(), DriverError>>()
                + crate::journal::SessionJournal::retained_session_inline_bytes(),
        );
        self.preparation_result = Some(result);
        result
    }

    fn preparation_complete(&self) -> bool {
        self.preparation_started && matches!(self.preparation_result, Some(Ok(())))
    }

    /// Retain the first creation result BEFORE inspecting its identity. This is
    /// context metadata construction only, not driver binding or host admission.
    /// The canonical root/lease must already own this value before the callback.
    fn capture_host_context(
        &mut self,
        decision_id: &str,
        revision: u32,
        build: impl FnOnce(
            &cna_seats::operation::Operation,
        ) -> Result<cna_seats::host::HostDriverContext, String>,
    ) -> Result<(), StartupContextRefusal> {
        if !self.preparation_complete() {
            return Err(StartupContextRefusal::Unprepared);
        }
        let receipt = self
            .receipt
            .as_ref()
            .ok_or(StartupContextRefusal::Unbound)?;
        if !receipt.original_bind_returned_ok() {
            return Err(StartupContextRefusal::Unbound);
        }
        if self.host_context_started
            || self.host_context_result.is_some()
            || self.host_context_validated
            || self.host_binding_started
            || self.host_binding_result.is_some()
            || self.supervisor_started
            || self.outcome.is_some()
            || self.cleanup_started
            || self.cleanup_result.is_some()
        {
            return Err(StartupContextRefusal::AlreadyOwned);
        }
        self.host_context_started = true; // BEFORE callback; unwind stays unresolved.
        let result = build(&self.operation);
        self.host_context_result = Some(result); // Plain MOVE before identity/projection.
        let Some(Ok(context)) = self.host_context_result.as_ref() else {
            return Err(StartupContextRefusal::ContextFailed);
        };
        if !context.operation.same_instance(&self.operation)
            || context.identity.operation_id != self.operation.id()
            || context.identity.seat != receipt.seat
            || context.identity.epoch != receipt.epoch
            || context.identity.decision_id != decision_id
            || context.identity.revision != revision
        {
            // Keep even a foreign returned context in its first result slot.
            return Err(StartupContextRefusal::Identity);
        }
        self.host_context_validated = true;
        Ok(())
    }

    /// Private SOURCE assembly only: the complete creation-time fit MUST include
    /// both owned HostIdentity string copies and callback/driver/library frames
    /// before any production use. Never move away the canonical first context.
    /// Binding acknowledgement remains separate from native startup permission.
    fn bind_host_context(
        &mut self,
        original: &cna_seats::operation::Operation,
        bind: impl FnOnce(cna_seats::host::HostDriverContext) -> Result<(), DriverError>,
    ) -> Result<(), StartupContextRefusal> {
        if !self.operation.same_instance(original) {
            return Err(StartupContextRefusal::Identity);
        }
        if !self.preparation_complete() {
            return Err(StartupContextRefusal::Unprepared);
        }
        if !self.host_context_started || !self.host_context_validated {
            return Err(StartupContextRefusal::Unbound);
        }
        if self.host_binding_started
            || self.host_binding_result.is_some()
            || self.supervisor_started
            || self.outcome.is_some()
            || self.cleanup_started
            || self.cleanup_result.is_some()
        {
            return Err(StartupContextRefusal::AlreadyOwned);
        }
        let receipt = self
            .receipt
            .as_ref()
            .ok_or(StartupContextRefusal::Unbound)?;
        if !receipt.original_bind_returned_ok() {
            return Err(StartupContextRefusal::Unbound);
        }
        let Some(Ok(context)) = self.host_context_result.as_ref() else {
            return Err(StartupContextRefusal::Unbound);
        };
        self.host_binding_started = true; // BEFORE clone and erased callback.
        // Canonical context stays owned even when the consuming trait refuses or
        // unwinds. Growing duplicate bodies are a required FULL-plan input, not
        // a late charge or permission derived from the partial header plan.
        let transfer = context.clone();
        let result = bind(transfer);
        self.host_binding_result = Some(result); // MOVE before projection/observation.
        match self.host_binding_result.as_ref() {
            Some(Ok(())) => Ok(()),
            _ => Err(StartupContextRefusal::BindingFailed),
        }
    }

    fn bind_receipt(&mut self) -> Result<(), StartupReceiptRefusal> {
        if !self.preparation_complete() {
            return Err(StartupReceiptRefusal::Unprepared);
        }
        let receipt = self
            .receipt
            .as_mut()
            .ok_or(StartupReceiptRefusal::Unbound)?;
        if receipt.binding_started
            || receipt.binding_result.is_some()
            || receipt.binding_lookup_started
            || self.supervisor_started
            || self.outcome.is_some()
        {
            return Err(StartupReceiptRefusal::AlreadyOwned);
        }
        receipt.binding_started = true; // BEFORE journal callback, sticky on unwind.
        let result = receipt
            .journal
            .bind_startup_receipt(receipt.key(&self.operation), receipt.bound);
        receipt.binding_result = Some(result); // MOVE BEFORE projection/observation.
        match receipt.binding_result.as_ref() {
            Some(Ok(())) => Ok(()),
            _ => Err(StartupReceiptRefusal::JournalFailed),
        }
    }

    /// Accounting result is separate from retained driver/metadata results and
    /// all original native/archive/healthy gates. This helper releases no owner.
    fn complete_receipt(&mut self) -> Result<(), StartupReceiptRefusal> {
        if !self.preparation_complete() {
            return Err(StartupReceiptRefusal::Unprepared);
        }
        if !self.supervisor_started || !self.supervisor_observed {
            return Err(StartupReceiptRefusal::Unobserved);
        }
        let returned_ok = self
            .outcome
            .as_ref()
            .ok_or(StartupReceiptRefusal::Unobserved)?
            .is_ok();
        let receipt = self
            .receipt
            .as_mut()
            .ok_or(StartupReceiptRefusal::Unbound)?;
        if !receipt.original_bind_returned_ok() {
            return Err(StartupReceiptRefusal::Unbound);
        }
        if receipt.completion_started
            || receipt.completion_expected.is_some()
            || receipt.completion_result.is_some()
            || receipt.completion_lookup_started
        {
            return Err(StartupReceiptRefusal::AlreadyOwned);
        }
        receipt.completion_expected = Some(crate::journal::StartupReceipt {
            revision: 1,
            completed: true,
            elapsed_ms: Some(self.elapsed_ms),
            returned_ok: Some(returned_ok),
            ..receipt.bound
        });
        receipt.completion_started = true; // Original expected step fixed BEFORE callback.
        let result = receipt.journal.complete_startup_receipt(
            receipt.key(&self.operation),
            receipt.bound,
            self.elapsed_ms,
            returned_ok,
        );
        receipt.completion_result = Some(result);
        match receipt.completion_result.as_ref() {
            Some(Ok(actual)) if Some(*actual) == receipt.completion_expected => Ok(()),
            _ => Err(StartupReceiptRefusal::JournalFailed),
        }
    }

    /// Each step has its own one-shot read-only reconciliation destination. A
    /// matching lookup never replaces a returned error or enables driver start.
    /// No body/result is invented from stored scalars; no write/dispatch replay.
    fn reconcile_receipt(&mut self, completion: bool) -> Result<(), StartupReceiptRefusal> {
        if !self.preparation_complete() {
            return Err(StartupReceiptRefusal::Unprepared);
        }
        let receipt = self
            .receipt
            .as_mut()
            .ok_or(StartupReceiptRefusal::Unbound)?;
        let expected = if completion {
            if !receipt.completion_started {
                return Err(StartupReceiptRefusal::Unobserved);
            }
            if receipt.completion_lookup_started || receipt.completion_lookup_result.is_some() {
                return Err(StartupReceiptRefusal::AlreadyOwned);
            }
            let expected = receipt
                .completion_expected
                .ok_or(StartupReceiptRefusal::Unobserved)?;
            receipt.completion_lookup_started = true;
            expected
        } else {
            if !receipt.binding_started {
                return Err(StartupReceiptRefusal::Unobserved);
            }
            if receipt.binding_lookup_started || receipt.binding_lookup_result.is_some() {
                return Err(StartupReceiptRefusal::AlreadyOwned);
            }
            receipt.binding_lookup_started = true;
            receipt.bound
        };
        let result = receipt
            .journal
            .reconcile_startup_receipt(receipt.key(&self.operation), expected);
        let destination = if completion {
            &mut receipt.completion_lookup_result
        } else {
            &mut receipt.binding_lookup_result
        };
        *destination = Some(result);
        match destination.as_ref() {
            Some(Ok(())) => Ok(()),
            _ => Err(StartupReceiptRefusal::JournalFailed),
        }
    }

    /// Write only the session metadata candidate, preserving the original typed
    /// result. Receipt-backed owners require their actual original bind return
    /// and exact journal/seat. Receipt-less legacy SOURCE fixtures remain separate;
    /// public startup adoption stays closed until actual root/lifecycle/full fit.
    /// This storage result is never a native/accounting/custody/healthy receipt.
    fn write_session_candidate(
        &mut self,
        original: &cna_seats::operation::Operation,
        journal: &crate::journal::SessionJournal,
        seat: SeatId,
    ) -> Result<(), SessionStorageRefusal> {
        if !self.operation.same_instance(original) {
            return Err(SessionStorageRefusal::Identity);
        }
        if !self.preparation_complete() {
            return Err(SessionStorageRefusal::Unprepared);
        }
        if let Some(receipt) = &self.receipt
            && (!receipt.original_bind_returned_ok()
                || receipt.seat != seat
                || !std::ptr::eq(receipt.journal.as_ref(), journal))
        {
            return Err(SessionStorageRefusal::JournalBinding);
        }
        if !self.supervisor_started || !self.supervisor_observed {
            return Err(SessionStorageRefusal::Unobserved);
        }
        if self.storage_started || self.storage_result.is_some() {
            return Err(SessionStorageRefusal::AlreadyOwned);
        }
        let Some(Ok(info)) = self.outcome.as_mut() else {
            return Err(SessionStorageRefusal::NotSuccessful);
        };
        self.storage_started = true;
        // The helper's restore lease returns the SAME lent SessionInfo body on
        // Err, unwind and success. No clone, formatting or observation callback.
        let result = journal.session_retaining_body(seat, info);
        self.storage_result = Some(result);
        // Retain an actual returned storage error BEFORE projecting a fixed
        // refusal. A lost ACK/unwind leaves started=true/result=None and refuses
        // replay; future original receipt/CAS reconciliation is separate.
        match self.storage_result.as_ref() {
            Some(Ok(())) => Ok(()),
            _ => Err(SessionStorageRefusal::StorageFailed),
        }
    }
}

/// Borrow an ALREADY-installed original owner. Cancellation/unwind after first
/// poll leaves its attempt sticky even when no returned body was installed.
/// No observation/storage/recovery callback can request a second attempt here.
#[allow(dead_code)] // Existing startup remains unchanged pending full assembly.
async fn supervise_journal_start(
    owner: &mut PendingJournalStart,
    driver: &mut dyn SeatDriver,
    resume: Option<&str>,
) -> Result<(), DriverError> {
    if !owner.preparation_complete()
        // A constructed context is NOT driver binding. The private source bridge
        // stays closed here until actual binding/full-fit assembly is complete.
        || owner.host_context_started
        || owner.host_context_result.is_some()
        || owner.supervisor_started
        || owner.outcome.is_some()
        || owner.cleanup_started
        || owner.cleanup_result.is_some()
        || owner
            .receipt
            .as_ref()
            .is_some_and(|receipt| !receipt.original_bind_returned_ok())
    {
        // Fixed projection only; do not format or replace the original result.
        return Err(DriverError::Host(cna_seats::host::HostRefusal::Unknown));
    }
    owner.supervisor_started = true;
    // Keep the ORIGINAL absolute allowance, including a timeout result retained
    // BEFORE telemetry. No renewal or fabricated startup phase measurement.
    let outcome =
        match tokio::time::timeout_at(owner.operation.allowance().deadline(), driver.start(resume))
            .await
        {
            Ok(outcome) => outcome,
            Err(_) => Err(DriverError::Timeout),
        };
    // Plain MOVE precedes measurements, formatting, driver callbacks and storage.
    // The exclusive original owner borrow keeps this result destination empty.
    owner.outcome = Some(outcome);
    owner.elapsed_ms = elapsed(owner.started_at);
    owner.telemetry = driver.telemetry();
    owner.supervisor_observed = true;
    // This denotes completed observation ONLY, not a successful SessionInfo,
    // child/readers join, accounting, custody, healthy start or permission release.
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupCleanupRefusal {
    Identity,
    Unprepared,
    Unattempted,
    AlreadyOwned,
}

/// Private original cleanup observation, not a native join/custody/healthy gate.
/// Caller must keep the SAME concrete driver/Box and creation-admit the complete
/// concrete/erased/caller future and growing return storage before adoption.
#[allow(dead_code)]
async fn observe_journal_start_cleanup(
    owner: &mut PendingJournalStart,
    original: &cna_seats::operation::Operation,
    driver: &mut dyn SeatDriver,
) -> Result<bool, StartupCleanupRefusal> {
    if !owner.operation.same_instance(original) {
        return Err(StartupCleanupRefusal::Identity);
    }
    if !owner.preparation_complete() {
        return Err(StartupCleanupRefusal::Unprepared);
    }
    if !owner.host_binding_started && !owner.supervisor_started {
        return Err(StartupCleanupRefusal::Unattempted);
    }
    if owner.cleanup_started || owner.cleanup_result.is_some() {
        return Err(StartupCleanupRefusal::AlreadyOwned);
    }
    owner.cleanup_started = true; // BEFORE stop constructs its erased future.
    let result = match tokio::time::timeout_at(
        owner.operation.allowance().deadline(),
        driver.stop(),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(DriverError::Timeout),
    };
    owner.cleanup_result = Some(result); // MOVE before projection or any callback.
    // No telemetry, elapsed overwrite, formatting, storage/receipt or release.
    // This scalar records ONLY the typed return disposition, never all-join.
    Ok(owner.cleanup_result.as_ref().is_some_and(Result::is_ok))
}

/// Borrows an ALREADY-installed foreground owner. Canceling this future drops only
/// its borrow: the original Operation and pending result remain in the launcher's slot.
async fn supervise_journal_turn(
    owner: &mut crate::PendingJournalTurn,
    driver: &mut dyn SeatDriver,
    prompt: &str,
    reserved_ms: u64,
    start: Instant,
    deadline: Instant,
) -> Result<(), String> {
    // Refuse BEFORE polling a second turn. The exclusive owner borrow keeps
    // this destination empty until the original returned value is installed.
    if owner.supervisor_started || owner.outcome.is_some() {
        return Err("original supervisor attempt is already owned".into());
    }
    // Stamp before driver.run_turn constructs its erased future, not just before
    // polling it. A dropped borrow or callback unwind cannot reopen this attempt.
    // The unpolled outer future has not reached this boundary and invents no start.
    owner.supervisor_started = true;
    // Keep the same original timer and reservation. The retained journal path
    // cannot use bounded_turn_until's returned temporary for a timer win: its
    // progress callback may unwind before returning that timeout to this owner.
    match tokio::time::timeout_at(
        deadline.min(start + Duration::from_millis(reserved_ms)),
        driver.run_turn(prompt, Duration::from_millis(reserved_ms)),
    )
    .await
    {
        Ok(outcome) => owner.finish_supervisor(outcome, driver),
        Err(_) => owner.finish_timeout(start, reserved_ms, driver),
    }
}

impl crate::PendingJournalTurn {
    fn prepare(
        operation: cna_seats::operation::Operation,
        start: Instant,
        requested_turn_ms: u64,
    ) -> Result<Self, String> {
        // The fixed owner, timeout overlaps and bounded cancellation cause are admitted before
        // borrowing the driver. No payload body is allocated by this constructor.
        operation
            .charge(
                std::mem::size_of::<Self>()
                    .saturating_add(JournalInterruption::storage_bytes())
                    .saturating_add(JOURNAL_TIMEOUT_WORKSPACE_BYTES)
                    .saturating_add(JOURNAL_CAPTURE_WORKSPACE_BYTES),
            )
            .map_err(|error| error.to_string())?;
        Ok(Self {
            operation,
            supervisor_started: false,
            outcome: None,
            supervisor_observed: false,
            turn_started_at: Some(start),
            captured_outcome: None,
            capture_in_service: false,
            capture_refusal: None,
            journal_revision: 0,
            elapsed_ms: elapsed(start),
            supervisor_turn_elapsed_ms: measured_millis(start.elapsed()),
            requested_turn_ms,
            telemetry: Default::default(),
        })
    }

    fn finish_supervisor(
        &mut self,
        outcome: Result<cna_seats::driver::TurnOutcome, DriverError>,
        driver: &dyn SeatDriver,
    ) -> Result<(), String> {
        self.install_supervisor(outcome)?;
        self.observe_supervisor(driver)
    }

    fn install_supervisor(
        &mut self,
        outcome: Result<cna_seats::driver::TurnOutcome, DriverError>,
    ) -> Result<(), String> {
        if self.outcome.is_some() {
            return Err("original supervisor outcome is already retained".into());
        }
        // MOVE into the already-admitted original slot BEFORE measurements or
        // any driver callback. Telemetry can unwind; it must not drop a returned
        // error or captured body that was still in a caller temporary.
        self.outcome = Some(outcome);
        Ok(())
    }

    fn finish_timeout(
        &mut self,
        start: Instant,
        reserved_ms: u64,
        driver: &dyn SeatDriver,
    ) -> Result<(), String> {
        // Retain the timer's original cause BEFORE any driver callback. An
        // unwind leaves this timeout with unavailable phase, and observation
        // incomplete; it cannot turn an absent result into a later success.
        // TimeoutDiagnostic contains fixed numeric/enumeration fields only.
        self.install_supervisor(Err(DriverError::TimeoutAt(
            TimeoutDiagnostic::turn_reservation(start, reserved_ms, None),
        )))?;
        let progress = driver.turn_progress();
        let diagnostic = TimeoutDiagnostic::turn_reservation(start, reserved_ms, progress);
        match self.outcome.as_mut() {
            Some(Err(DriverError::TimeoutAt(retained))) => *retained = diagnostic,
            _ => return Err("original timeout result changed before observation".into()),
        }
        // Only the existing timeout's diagnostic fields change. Its error
        // class, separately captured body and original operation remain owned.
        self.observe_supervisor(driver)
    }

    fn observe_supervisor(&mut self, driver: &dyn SeatDriver) -> Result<(), String> {
        if self.outcome.is_none() {
            return Err("original supervisor is still in service".into());
        }
        if let Some(start) = self.turn_started_at {
            self.elapsed_ms = elapsed(start);
            self.supervisor_turn_elapsed_ms = measured_millis(start.elapsed());
        }
        self.telemetry = driver.telemetry();
        self.supervisor_observed = true;
        Ok(())
    }

    fn interrupt_supervisor(
        &mut self,
        cause: JournalInterruption,
        driver: &dyn SeatDriver,
    ) -> Result<(), String> {
        if self.outcome.is_none() {
            self.finish_supervisor(Err(cause.error()), driver)?;
        }
        // An already-returned error/timeout is the first cause and is never replaced.
        Ok(())
    }

    fn capture_from_driver(&mut self, driver: &mut dyn SeatDriver) -> Result<(), String> {
        if self.capture_in_service {
            return Err("original captured outcome callback remains unresolved".into());
        }
        if self.capture_refusal.is_some() {
            return Err("original captured outcome handoff refusal remains retained".into());
        }
        let outcome = self
            .outcome
            .as_ref()
            .ok_or("original supervisor is still in service")?;
        if outcome.is_ok() || self.captured_outcome.is_some() {
            return Ok(());
        }
        // Mark BEFORE the callback. A returned value is immediately installed;
        // a callback unwind keeps this obligation unknown, never a clean None.
        self.capture_in_service = true;
        match driver.take_captured_outcome(&self.operation) {
            Ok(captured) => {
                self.captured_outcome = captured;
                self.capture_in_service = false;
                Ok(())
            }
            Err(error) => {
                // Canonical typed refusal survives formatting/allocation unwind.
                // Keep it separate from the first supervisor result and capture.
                self.capture_refusal = Some(error);
                self.capture_in_service = false;
                Err(format!(
                    "{}; captured outcome handoff failed: {error}",
                    outcome
                        .as_ref()
                        .err()
                        .expect("original failed supervisor outcome"),
                    error = self
                        .capture_refusal
                        .as_ref()
                        .expect("retained capture refusal"),
                ))
            }
        }
    }

    /// Original evidence and measurements are retained across a failed handoff. This
    /// method never dispatches, renews an allowance or proves native cleanup by itself.
    pub(crate) fn commit(
        &mut self,
        journal: &crate::journal::SessionJournal,
        seat: SeatId,
        original_supervision_joined: bool,
    ) -> Result<(), String> {
        let outcome = self
            .outcome
            .as_ref()
            .ok_or("original supervisor is still in service; journal owner retained")?;
        if !self.supervisor_observed {
            return Err(
                "original supervisor observation remains unresolved; journal outcome retained"
                    .into(),
            );
        }
        if self.capture_in_service || self.capture_refusal.is_some() {
            return Err(
                "original captured outcome handoff remains unresolved; journal owner retained"
                    .into(),
            );
        }
        let (revision, committed) = journal.start_accounting_state(seat, self.operation.id())?;
        if committed {
            // A lost storage ACK may have left this foreground owner after the real commit.
            // Observe THAT operation's committed marker; do not increment usage again.
            let observed = journal.reconcile_start(
                seat,
                self.operation.id(),
                revision,
                Some(self.operation.start_evidence()),
                false,
            )?;
            self.journal_revision = observed.revision;
            return Ok(());
        }
        let observed = journal
            .reconcile_start(
                seat,
                self.operation.id(),
                self.journal_revision,
                Some(self.operation.start_evidence()),
                original_supervision_joined || outcome.is_ok(),
            )
            .map_err(|error| match outcome.as_ref().err() {
                Some(original) => format!("{original}; start accounting failed: {error}"),
                None => format!("start accounting failed: {error}"),
            })?;
        self.journal_revision = observed.revision;
        if outcome.is_err() && !original_supervision_joined {
            return Err(format!(
                "{}; original child/readers supervision remains unresolved; journal outcome retained",
                outcome.as_ref().err().expect("failed outcome")
            ));
        }
        let timeout = outcome
            .as_ref()
            .err()
            .and_then(DriverError::timeout_diagnostic)
            .map(|diagnostic| crate::journal::TimeoutCompletion {
                diagnostic,
                requested_turn_ms: self.requested_turn_ms,
                supervisor_turn_elapsed_ms: self.supervisor_turn_elapsed_ms,
            });
        let completion = if let Some(captured) = self.captured_outcome.as_ref() {
            journal.complete_captured_operation(
                seat,
                self.operation.id(),
                self.elapsed_ms,
                captured,
                &self.telemetry,
                timeout,
            )
        } else {
            journal.complete_bound_operation(
                seat,
                Some(self.operation.id()),
                self.elapsed_ms,
                outcome.as_ref().ok(),
                &self.telemetry,
                timeout,
            )
        };
        completion.map_err(|storage| completion_failure(outcome.as_ref().err(), &storage))
    }
}

impl Demo {
    /// Private actual Demo root bridge. No driver/native effect or preparation
    /// here; caller retains its candidate on refusal. Whole-fit/caller/factory
    /// integration must precede any future public use of this source bridge.
    #[allow(dead_code)]
    fn install_journal_start(
        &self,
        seat: SeatId,
        candidate: &mut Option<PendingJournalStart>,
    ) -> Result<(), JournalStartRefusal> {
        let receipt = candidate
            .as_ref()
            .and_then(|owner| owner.receipt.as_ref())
            .ok_or(JournalStartRefusal::Identity)?;
        if receipt.seat != seat
            || self
                .journal
                .as_ref()
                .is_none_or(|journal| !std::sync::Arc::ptr_eq(journal, &receipt.journal))
        {
            return Err(JournalStartRefusal::Identity);
        }
        self.journal_starts
            .get(&seat)
            .ok_or(JournalStartRefusal::Identity)?
            .install(candidate)
    }

    #[allow(dead_code)]
    fn checkout_journal_start(
        &self,
        seat: SeatId,
        original: &cna_seats::operation::Operation,
    ) -> Result<JournalStartLease<'_>, JournalStartRefusal> {
        self.journal_starts
            .get(&seat)
            .ok_or(JournalStartRefusal::Identity)?
            .checkout(original)
    }

    /// Actual Demo metadata builder routed through the SAME retained root.
    /// No new Operation, allowance registration, driver/native entry or receipt
    /// reconciliation is performed. Public startup remains unredirected.
    #[allow(dead_code)]
    fn capture_journal_start_context(
        &self,
        seat: SeatId,
        request: &cna_core::decision::DecisionRequest,
        original: &cna_seats::operation::Operation,
    ) -> Result<(), StartupContextRefusal> {
        let mut lease = self
            .checkout_journal_start(seat, original)
            .map_err(|_| StartupContextRefusal::Identity)?;
        let owner = lease.owner_mut();
        let epoch = owner
            .receipt
            .as_ref()
            .ok_or(StartupContextRefusal::Unbound)?
            .epoch;
        owner.capture_host_context(request.id.as_str(), request.revision, |operation| {
            self.driver_context_from_original(seat, epoch, request, operation)
        })
    }

    /// Borrow the original root while retaining the binding callback's first
    /// result. No allowance registration/start/telemetry/receipt completion here.
    /// Full creation-time fit and public factory adoption remain held.
    #[allow(dead_code)]
    fn bind_journal_start_context(
        &self,
        seat: SeatId,
        original: &cna_seats::operation::Operation,
        driver: &mut dyn SeatDriver,
    ) -> Result<(), StartupContextRefusal> {
        let mut lease = self
            .checkout_journal_start(seat, original)
            .map_err(|_| StartupContextRefusal::Identity)?;
        lease
            .owner_mut()
            .bind_host_context(original, |context| driver.set_host_context(context))
    }

    /// Retain cleanup in the SAME startup root, with no lock across await and
    /// no inference of driver-Box identity or every original child/reader join.
    /// Actual generic stop/receipt/public factory paths remain unredirected.
    #[allow(dead_code)]
    async fn observe_journal_start_cleanup(
        &self,
        seat: SeatId,
        original: &cna_seats::operation::Operation,
        driver: &mut dyn SeatDriver,
    ) -> Result<bool, StartupCleanupRefusal> {
        let mut lease = self
            .checkout_journal_start(seat, original)
            .map_err(|_| StartupCleanupRefusal::Identity)?;
        observe_journal_start_cleanup(lease.owner_mut(), original, driver).await
    }

    /// Stop is graceful and resumable; exhausted budgets or CLI failures pause the bound seat.
    pub async fn play_durable(
        &self,
        drivers: &mut [(SeatId, Box<dyn SeatDriver>)],
        stop: watch::Receiver<bool>,
    ) -> Result<(), String> {
        // BEFORE kind/telemetry callbacks, usage publication, activation or a
        // new task. The original startup/turn root must never be bypassed by
        // re-entering the launcher with an otherwise identical driver list.
        if self.any_journal_owned() {
            return Err("original startup/turn owner remains Unsettled; dispatch refused".into());
        }
        if self.journal.is_none() {
            return Err("durable session journal required".into());
        }
        let expected: BTreeSet<_> = self.epochs.keys().copied().collect();
        let provided: BTreeSet<_> = drivers.iter().map(|(s, _)| *s).collect();
        if expected != provided
            || provided.len() != drivers.len()
            || drivers.iter().any(|(_, d)| d.kind() != CliKind::Claude)
        {
            return Err("exactly one confined Claude driver per binding required".into());
        }
        if drivers.is_empty() {
            return self.play_sessions(drivers, self.config.max_turns).await;
        }
        let mut result = Ok(());
        for seat in &expected {
            result = combine_results(result, self.publish_usage(*seat).await);
        }
        if result.is_ok() {
            result = self.handle.pause(false).await.map_err(|e| e.to_string());
        }
        if result.is_ok() {
            let (cancel, peer) = watch::channel(false);
            let mut tasks = FuturesUnordered::new();
            let slots = Arc::new(Semaphore::new(2));
            for (seat, driver) in drivers.iter_mut() {
                let seat = *seat;
                let stop = stop.clone();
                let peer = peer.clone();
                let slots = slots.clone();
                tasks.push(async move {
                    (
                        seat,
                        self.durable_seat(seat, driver.as_mut(), stop, peer, slots)
                            .await,
                    )
                });
            }
            while let Some((seat, outcome)) = tasks.next().await {
                if self.config.run.is_none() || outcome.is_err() {
                    let _ = cancel.send(true);
                }
                result = combine_results(result, outcome.map_err(|e| format!("{seat}: {e}")));
            }
        } else {
            for (_, driver) in drivers.iter_mut() {
                result = combine_results(result, driver.stop().await.map_err(|e| e.to_string()));
            }
        }
        self.finish_run(result).await
    }
    async fn stop_child(&self, seat: SeatId, driver: &mut dyn SeatDriver) -> Result<(), String> {
        if let Some(slot) = self.journal_turns.get(&seat) {
            let mut retained = slot
                .try_lock()
                .map_err(|_| "journal turn is still in service")?;
            if let Some(owner) = retained.as_mut() {
                owner.interrupt_supervisor(JournalInterruption::BorrowCanceled, driver)?;
                owner.capture_from_driver(driver)?;
            }
        }
        self.stop_child_supervision(seat, driver).await
    }

    /// The caller has already moved any captured result into its retained owner.
    /// This path must not reacquire that owner's lock while its borrow is in service.
    async fn stop_child_supervision(
        &self,
        seat: SeatId,
        driver: &mut dyn SeatDriver,
    ) -> Result<(), String> {
        let allowance = self
            .driver_allowances
            .lock()
            .expect("driver supervision allowances")
            .get(&seat)
            .copied();
        let Some(allowance) = allowance else {
            self.cleanup_uncertain.lock().await.insert(seat);
            return Err(
                "no_allowance: native supervision retained without dispatch permission".into(),
            );
        };
        let stopped = tokio::time::timeout_at(allowance.deadline(), driver.stop())
            .await
            .map_err(|_| {
                "CLI stop exceeded original allowance; SAME child/readers retained Unsettled"
                    .to_string()
            })
            .and_then(|r| r.map_err(|e| e.to_string()));
        if stopped.is_err() {
            self.cleanup_uncertain.lock().await.insert(seat);
            // A normal pause can be deferred behind this still-owned prompt/forced route.
            // Record private uncertainty without requesting a conflicting persisted control.
            let marked = if self.journal_owned(seat) {
                Ok(())
            } else {
                self.journal
                    .as_ref()
                    .map_or(Ok(()), |journal| journal.interrupt(seat, false))
            };
            return combine_results(stopped, marked);
        }
        self.cleanup_uncertain.lock().await.remove(&seat);
        if !self.journal_owned(seat) {
            self.unsettled_permits.lock().await.remove(&seat);
        }
        Ok(())
    }
    async fn publish_usage(&self, seat: SeatId) -> Result<(), String> {
        // Board decoder/revision replacement landed in 87ffa001 before producer enable.
        self.journal
            .as_ref()
            .ok_or("missing journal")?
            .deliver_usage(seat, &self.sink)
            .await
    }

    fn interrupt_journal_turn(
        &self,
        seat: SeatId,
        cause: JournalInterruption,
        driver: &dyn SeatDriver,
    ) -> Result<(), String> {
        let Some(slot) = self.journal_turns.get(&seat) else {
            return Ok(());
        };
        let mut foreground = slot
            .try_lock()
            .map_err(|_| "journal turn is still in service")?;
        if let Some(owner) = foreground.as_mut() {
            owner.interrupt_supervisor(cause, driver)?;
        }
        Ok(())
    }

    /// Invoked only after stop_child's actual original-supervision result. Borrowed
    /// ownership stays in the launcher on SQL error or cancellation; no retry loop.
    async fn settle_journal_after_supervision(&self, seat: SeatId) -> Result<(), String> {
        let Some(slot) = self.journal_turns.get(&seat) else {
            return Ok(());
        };
        let mut foreground = slot
            .try_lock()
            .map_err(|_| "journal turn is still in service")?;
        if let Some(retained) = foreground.as_mut() {
            retained.commit(self.journal.as_ref().ok_or("missing journal")?, seat, true)?;
            // Accounting is durably committed; the original caller error remains in its
            // result, and known usage/outbox now owns the completed accounting prefix.
            foreground.take();
        }
        Ok(())
    }
    async fn durable_seat(
        &self,
        seat: SeatId,
        driver: &mut dyn SeatDriver,
        mut stop: watch::Receiver<bool>,
        mut peer: watch::Receiver<bool>,
        slots: Arc<Semaphore>,
    ) -> Result<(), String> {
        let epoch = self.epochs[&seat];
        let mut binding = self.handle.watch_seat(seat);
        let invalidated = async {
            loop {
                {
                    let state = binding.borrow_and_update();
                    if state.binding.controller_epoch != epoch || state.binding.paused {
                        break;
                    }
                }
                if binding.changed().await.is_err() {
                    break;
                }
            }
        };
        let cancelled = async {
            loop {
                if *stop.borrow_and_update() || *peer.borrow_and_update() {
                    break;
                }
                tokio::select! {r = stop.changed()=>{if r.is_err(){break;}}, r = peer.changed()=>{if r.is_err(){break;}}}
            }
        };
        let saved = self.journal.as_ref().ok_or("missing journal")?.snapshot()?;
        let used = saved.seats[&seat].wall_millis;
        let remaining = self
            .config
            .session
            .as_ref()
            .ok_or("missing limits")?
            .wall_seconds
            .saturating_mul(1000)
            .saturating_sub(used);
        let mut permit: Option<OwnedSemaphorePermit> = None;
        let run_deadline = Instant::now() + Duration::from_millis(remaining);
        let (mut result, interruption) = tokio::select! {
            _=invalidated=>(Err("controller binding changed or paused; old session stopped".into()),
                Some(JournalInterruption::BindingChanged)),
            _=cancelled=>{self.sink.system(seat,"campaign session stopped; journal retained for resume");
                (Ok(()), Some(JournalInterruption::CampaignStopped))},
            r=tokio::time::timeout_at(run_deadline,self.durable_work(seat,driver,slots,&mut permit,run_deadline))=>match r {
                Ok(result)=>(result,None),
                Err(_)=>(Err("durable wall-clock budget exhausted".into()),Some(JournalInterruption::WallTimeout)),
            },
        };
        if let Some(cause) = interruption {
            result = combine_results(result, self.interrupt_journal_turn(seat, cause, driver));
        }
        let stopped = self.stop_child(seat, driver).await;
        let cleanup_confirmed = stopped.is_ok();
        result = combine_results(result, stopped);
        if cleanup_confirmed {
            result = combine_results(result, self.settle_journal_after_supervision(seat).await);
        }
        if cleanup_confirmed
            && !self.journal_owned(seat)
            && !self.forced_owned(seat).await
            && self.config.run.is_some()
            && result.as_ref().is_err_and(|e| {
                matches!(
                    e.as_str(),
                    "durable turn budget exhausted"
                        | "durable tool budget exhausted"
                        | "durable wall-clock budget exhausted"
                )
            })
        {
            let reason = result.as_ref().unwrap_err().clone();
            self.sink.system(
                seat,
                format!("automatic budget stop: {reason}; lifetime journal retained"),
            );
            result = self.handle.pause(true).await.map_err(|e| e.to_string());
        }
        let accounting = if self.journal_owned(seat) {
            Err("original returned journal outcome is retained; no interruption may replace its evidence".into())
        } else {
            self.journal
                .as_ref()
                .ok_or("missing journal")?
                .interrupt(seat, cleanup_confirmed)
        };
        result = combine_results(result, accounting);
        if cleanup_confirmed && !self.journal_owned(seat) {
            drop(permit);
        } else if let Some(permit) = permit.take() {
            // Move the ORIGINAL session permit into foreground ownership; never replace or
            // reacquire it while its child/readers remain uncertain.
            self.unsettled_permits.lock().await.insert(seat, permit);
        }
        if cleanup_confirmed && !self.journal_owned(seat) && !self.forced_owned(seat).await {
            result = combine_results(result, self.publish_usage(seat).await);
        }
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
                Err(e) => {
                    result =
                        combine_results(result, Err(format!("recording seat failure failed: {e}")))
                }
            }
        }
        result
    }
    async fn durable_work(
        &self,
        seat: SeatId,
        driver: &mut dyn SeatDriver,
        slots: Arc<Semaphore>,
        permit: &mut Option<OwnedSemaphorePermit>,
        run_deadline: Instant,
    ) -> Result<(), String> {
        let journal = self.journal.as_ref().ok_or("missing journal")?;
        if self.journal_owned(seat) {
            return Err("original returned journal outcome remains owned; dispatch refused".into());
        }
        let limits = self.config.session.as_ref().ok_or("missing limits")?;
        let mut windows = self.handle.watch_seat(seat);
        let mut started = false;
        let mut first = true;
        let mut reseed = false;
        let mut decision_allowance = None;
        loop {
            let pending = loop {
                match self.handle.status() {
                    CampaignStatus::Finished { .. } => return Ok(()),
                    CampaignStatus::Running => {}
                    CampaignStatus::Paused if self.config.run.is_some() => return Ok(()),
                    other => return Err(format!("campaign stopped: {other:?}")),
                }
                let p = windows.borrow_and_update().pending.first().cloned();
                if let Some(p) = p {
                    break p;
                }
                // Idle intervals are reserved too, so an abrupt restart cannot reset elapsed time.
                let ms = journal.reserve(seat, "idle", "idle", 1000)?;
                let start = Instant::now();
                tokio::select! {_=tokio::time::sleep(Duration::from_millis(ms))=>{}, _=windows.changed()=>{}}
                journal.complete(seat, elapsed(start), None, &driver.telemetry())?;
            };
            let decision_deadline = crate::decision_deadline(
                &mut decision_allowance,
                &pending.id.to_string(),
                pending.revision,
                run_deadline,
                Duration::from_secs(limits.turn_seconds),
            );
            let stage = format!(
                "GT{}:{}",
                pending.clock.game_turn,
                pending
                    .clock
                    .op_stage
                    .map(|n| format!("OpStage{n}"))
                    .unwrap_or_else(|| pending.clock.anchor.stage().into())
            );
            if cna_seats::automatic::forced_pass(&pending.space) {
                let start = Instant::now();
                let ms = journal.reserve(seat, "automatic", &stage, 5000)?;
                let allowance = cna_seats::operation::Allowance::required(Some(
                    decision_deadline.min(start + Duration::from_millis(ms)),
                ))
                .map_err(|e| e.to_string())?;
                let result = self
                    .answer_hosted(seat, self.epochs[&seat], &pending, allowance)
                    .await;
                journal.complete_automatic(seat, elapsed(start), matches!(result, Ok(true)))?;
                result?;
                continue;
            }
            if self.config.run.is_some() {
                *permit = Some(
                    slots
                        .clone()
                        .acquire_owned()
                        .await
                        .map_err(|e| e.to_string())?,
                );
                if !matches!(self.handle.status(), CampaignStatus::Running) {
                    return Ok(());
                }
                let current = self.handle.seat(seat);
                if current.binding.controller_epoch != self.epochs[&seat] {
                    return Err("binding changed while waiting for model slot".into());
                }
                if !current
                    .pending
                    .iter()
                    .any(|p| p.id == pending.id && p.revision == pending.revision)
                {
                    permit.take();
                    continue;
                }
                let bound = driver.estimate_bound();
                let admission = journal.admit(seat, bound.as_ref())?;
                let applied = match admission {
                    Admission::Stop(reason) => {
                        self.sink.system(
                            seat,
                            format!("automatic budget stop: {reason}; no new model turn"),
                        );
                        self.handle.pause(true).await.map_err(|e| e.to_string())?;
                        return Ok(());
                    }
                    Admission::Ready {
                        remaining_usd: Some(usd),
                        ..
                    } => driver.set_reported_cost_limit(usd),
                    Admission::Ready {
                        remaining_tokens: Some(tokens),
                        ..
                    } => driver.set_token_limit(tokens),
                    _ => return Err("missing enforced budget".into()),
                };
                if let Err(error) = applied {
                    self.sink.system(
                        seat,
                        format!("budget enforcement unavailable: {error}; no model turn"),
                    );
                    self.handle.pause(true).await.map_err(|e| e.to_string())?;
                    return Ok(());
                }
            }
            if !started {
                let saved = journal.snapshot()?.seats[&seat].session.clone();
                let resume = saved.as_ref().map(|s| s.session_id.as_str());
                let start = Instant::now();
                let ms = journal.reserve(seat, "start", "start", 30_000)?;
                let allowance = cna_seats::operation::Allowance::required(Some(
                    decision_deadline.min(start + Duration::from_millis(ms)),
                ))
                .map_err(|e| e.to_string())?;
                driver
                    .set_host_context(self.driver_context(
                        seat,
                        self.epochs[&seat],
                        &pending,
                        allowance,
                    )?)
                    .map_err(|e| e.to_string())?;
                let info = tokio::time::timeout_at(allowance.deadline(), driver.start(resume))
                    .await
                    .map_err(|_| "session start timed out".to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()));
                journal.complete(seat, elapsed(start), None, &driver.telemetry())?;
                match info {
                    Ok(info) => {
                        journal.session(seat, info)?;
                        started = true;
                        first = true;
                    }
                    Err(error) if resume.is_some() && unavailable(&error) => {
                        journal.recover_process(seat, true)?;
                        self.stop_child(seat, driver).await?;
                        reseed = true;
                        journal.release_admission(seat, true)?;
                        permit.take();
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            if let Some(context) = driver.telemetry().context_tokens
                && context >= limits.context_tokens.saturating_mul(95) / 100
            {
                return Err(
                    "context budget exhausted after native compaction opportunity; session paused"
                        .into(),
                );
            }
            let prompt = if first {
                let notes = self
                    .handle
                    .notebook(seat)
                    .await
                    .map_err(|e| format!("notebook recovery failed: {e}"))?;
                let preface = if reseed {
                    "The previous CLI session was unavailable. Recover your standing plan from this durable notebook."
                } else {
                    "Continue your campaign seat, preserving your standing plan."
                };
                format!(
                    "{preface}\n{}",
                    self.prompts
                        .first_turn(seat, &notes, std::slice::from_ref(&pending))
                )
            } else {
                self.prompts
                    .window_turn(seat, std::slice::from_ref(&pending))
            };
            let prompt = format!(
                "{prompt}\n\nAnswer exactly decision {} revision {} once, then end this model turn. The same group may reopen with a newer revision: leave that new request for the next turn. Keep durable plans in notebook_write. Use only your scoped MCP tools.",
                pending.id, pending.revision
            );

            let start = Instant::now();
            let operation_id = uuid::Uuid::new_v4().to_string();
            let ms = journal.reserve_decision_authorized(
                seat,
                &stage,
                limits.turn_seconds * 1000,
                pending.id.as_str(),
                pending.revision,
                crate::journal::StartAuthorization {
                    epoch: self.epochs[&seat],
                    operation_id: &operation_id,
                },
            )?;
            let allowance = cna_seats::operation::Allowance::required(Some(
                decision_deadline.min(start + Duration::from_millis(ms)),
            ))
            .map_err(|e| e.to_string())?;
            let context = self.driver_context_with_id(
                seat,
                self.epochs[&seat],
                &pending,
                allowance,
                operation_id,
            )?;
            // Keep THIS operation's evidence association; a later driver context or cumulative
            // restored usage cannot confirm this reservation by inference.
            let operation = context.operation.clone();
            let mut foreground = self
                .journal_turns
                .get(&seat)
                .ok_or("missing foreground journal owner")?
                .try_lock()
                .map_err(|_| "original journal turn is still in service")?;
            if foreground.is_some() {
                return Err("original journal outcome/evidence is still owned".into());
            }
            *foreground = Some(crate::PendingJournalTurn::prepare(
                operation,
                start,
                limits.turn_seconds * 1000,
            )?);
            let retained = foreground
                .as_mut()
                .ok_or("missing original journal operation")?;
            match driver.set_host_context(context) {
                Ok(()) => {
                    supervise_journal_turn(
                        retained,
                        driver,
                        &prompt,
                        ms,
                        start,
                        allowance.deadline(),
                    )
                    .await?
                }
                Err(error) => retained.finish_supervisor(Err(error), driver)?,
            }
            let original_supervision_joined =
                if retained.outcome.as_ref().is_some_and(Result::is_err) {
                    retained.capture_from_driver(driver)?;
                    // Same retained child/readers and original allowance. Failed cleanup leaves
                    // the canonical outcome here; expiry is never converted to a join witness.
                    self.stop_child_supervision(seat, driver)
                        .await
                        .map_err(|cleanup| {
                            format!(
                                "{}; {cleanup}",
                                retained
                                    .outcome
                                    .as_ref()
                                    .expect("returned outcome")
                                    .as_ref()
                                    .err()
                                    .expect("failed outcome")
                            )
                        })?;
                    true
                } else {
                    false
                };
            retained.commit(journal, seat, original_supervision_joined)?;
            // Actual storage commit is the handoff. Move this canonical outcome back to
            // the caller; no clone, inferred completion, or refund on a failed write.
            let outcome = foreground
                .take()
                .ok_or("missing committed journal outcome")?
                .outcome
                .ok_or("missing committed supervisor outcome")?;
            drop(foreground);
            self.publish_usage(seat).await?;
            if let Some(id) = driver.session_id()
                && journal.snapshot()?.seats[&seat]
                    .session
                    .as_ref()
                    .map(|s| s.session_id.as_str())
                    != Some(id.as_str())
            {
                return Err("CLI session identity changed unexpectedly".into());
            }
            let error = match outcome {
                Ok(ref o) if o.ok => None,
                Ok(ref o) => Some(o.error.clone().unwrap_or_else(|| "CLI refused turn".into())),
                Err(ref e) => Some(e.to_string()),
            };
            if let Some(error) = error {
                if self.config.run.is_some()
                    && error.to_ascii_lowercase().contains("max_budget_usd")
                {
                    self.sink.system(
                        seat,
                        "provider reported-cost limit reached; campaign paused",
                    );
                    self.handle.pause(true).await.map_err(|e| e.to_string())?;
                    return Ok(());
                }
                let saved = journal.snapshot()?.seats[&seat].session.clone();
                let reseed_now =
                    first && saved.as_ref().is_some_and(|s| s.resumed) && unavailable(&error);
                if reseed_now || outcome.as_ref().is_err_and(retryable_death) {
                    journal.recover_process(seat, reseed_now)?;
                    self.stop_child(seat, driver).await?;
                    started = false;
                    journal.release_admission(seat, true)?;
                    permit.take();
                    reseed = reseed_now;
                    self.sink.system(
                        seat,
                        if reseed_now {
                            "CLI resume unavailable; reseeding from durable notebook"
                        } else {
                            "CLI process died; resuming the same session"
                        },
                    );
                    continue;
                }
                return Err(error);
            }
            if self.config.run.is_some() {
                self.stop_child(seat, driver).await?;
                journal.release_admission(seat, true)?;
                permit.take();
                started = false;
            }
            if self
                .handle
                .seat(seat)
                .pending
                .iter()
                .any(|p| p.id == pending.id && p.revision == pending.revision)
            {
                return Err("CLI ended without answering the requested decision revision".into());
            }
            if self.config.run.is_some()
                && let Admission::Stop(reason) =
                    journal.admission(seat, driver.estimate_bound().as_ref())?
            {
                self.sink.system(
                    seat,
                    format!("automatic budget stop: {reason}; journal retained"),
                );
                self.handle.pause(true).await.map_err(|e| e.to_string())?;
                return Ok(());
            }
            first = false;
            reseed = false;
            let record = journal.snapshot()?.seats[&seat].clone();
            self.sink.system(seat,format!("durable turn {} complete ({stage}); calls {}, reported cumulative cost ${:.6}, compactions {}, incomplete telemetry turns {}",record.turns,record.calls,record.reported_cost_usd,record.compactions,record.incomplete_turns));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Inert startup fixture only. No child, journal, archive or model work.
    struct StartDriver {
        outcome: Option<Result<cna_seats::driver::SessionInfo, DriverError>>,
        hang: bool,
        calls: usize,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
        start_panic: std::sync::Mutex<Option<Box<dyn std::any::Any + Send>>>,
        telemetry_panic: std::sync::Mutex<Option<Box<dyn std::any::Any + Send>>>,
        telemetry_calls: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl SeatDriver for StartDriver {
        fn kind(&self) -> CliKind {
            CliKind::Claude
        }
        fn telemetry(&self) -> cna_seats::driver::SessionTelemetry {
            self.telemetry_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let payload = self.telemetry_panic.lock().unwrap().take();
            if let Some(payload) = payload {
                std::panic::resume_unwind(payload);
            }
            Default::default()
        }
        async fn start(
            &mut self,
            resume: Option<&str>,
        ) -> Result<cna_seats::driver::SessionInfo, DriverError> {
            self.calls += 1;
            assert_eq!(resume, Some("original resume argument"));
            let _guard = CancelGuard(self.cancelled.clone());
            if let Some(payload) = self.start_panic.get_mut().unwrap().take() {
                std::panic::resume_unwind(payload);
            }
            if self.hang {
                std::future::pending::<()>().await;
            }
            self.outcome.take().expect("one original fixture result")
        }
        async fn run_turn(
            &mut self,
            _: &str,
            _: Duration,
        ) -> Result<cna_seats::driver::TurnOutcome, DriverError> {
            unreachable!()
        }
        fn session_id(&self) -> Option<String> {
            None
        }
        fn is_alive(&mut self) -> bool {
            false
        }
        async fn stop(&mut self) -> Result<(), DriverError> {
            unreachable!()
        }
    }
    fn start_driver(
        outcome: Option<Result<cna_seats::driver::SessionInfo, DriverError>>,
        hang: bool,
    ) -> StartDriver {
        StartDriver {
            outcome,
            hang,
            calls: 0,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            start_panic: std::sync::Mutex::new(None),
            telemetry_panic: std::sync::Mutex::new(None),
            telemetry_calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. SAME inert Box across startup/cleanup, not
    // a native child/reader join, archive custody or actual Demo/factory witness.
    #[tokio::test]
    #[cfg(panic = "unwind")]
    async fn startup_cleanup_retains_return_cancel_unwind_and_unpolled_owner() {
        use crate::config::{GameKind, LaunchConfig, SessionLimits};
        use futures_util::FutureExt;
        use std::panic::AssertUnwindSafe;
        struct Opaque(std::cell::Cell<u32>);
        struct CleanupDriver {
            info: Option<cna_seats::driver::SessionInfo>,
            returned: Option<Result<(), DriverError>>,
            mode: u8,
            starts: usize,
            stops: usize,
            canceled: Arc<std::sync::atomic::AtomicBool>,
            payload: std::sync::Mutex<Option<Box<dyn std::any::Any + Send>>>,
        }
        #[async_trait::async_trait]
        impl SeatDriver for CleanupDriver {
            fn kind(&self) -> CliKind {
                CliKind::Claude
            }
            async fn start(
                &mut self,
                resume: Option<&str>,
            ) -> Result<cna_seats::driver::SessionInfo, DriverError> {
                assert_eq!(resume, Some("original resume argument"));
                self.starts += 1;
                Ok(self.info.take().unwrap())
            }
            async fn run_turn(
                &mut self,
                _: &str,
                _: Duration,
            ) -> Result<cna_seats::driver::TurnOutcome, DriverError> {
                unreachable!("no model turn in cleanup fixture")
            }
            fn session_id(&self) -> Option<String> {
                None
            }
            fn is_alive(&mut self) -> bool {
                false
            }
            async fn stop(&mut self) -> Result<(), DriverError> {
                self.stops += 1;
                let _guard = CancelGuard(self.canceled.clone());
                if let Some(payload) = self.payload.get_mut().unwrap().take() {
                    std::panic::resume_unwind(payload);
                }
                if self.mode == 2 {
                    std::future::pending::<()>().await;
                }
                self.returned.take().unwrap()
            }
        }
        for mode in 0..5 {
            let seat: SeatId = "axis.front_line".parse().unwrap();
            let directory = tempfile::tempdir().unwrap();
            let campaign_id = uuid::Uuid::new_v4().to_string();
            let mut config = LaunchConfig::resolve(
                GameKind::Cna,
                &[
                    "axis.front_line=claude:haiku".into(),
                    "*=scripted:legal_random".into(),
                ],
            )
            .unwrap();
            config.session = Some(SessionLimits::default());
            let journal = Arc::new(
                crate::journal::SessionJournal::create(
                    directory.path(),
                    &campaign_id,
                    config,
                    &std::collections::BTreeMap::from([(seat, 7)]),
                )
                .unwrap(),
            );
            let root = JournalStartRoot::empty(seat, 7);
            let start = Instant::now();
            let original = cna_seats::operation::Operation::new(
                uuid::Uuid::new_v4().to_string(),
                7,
                cna_seats::operation::Allowance::required(Some(start + Duration::from_secs(30)))
                    .unwrap(),
            )
            .unwrap();
            let mut candidate = Some(PendingJournalStart::unprepared_with_receipt(
                original.clone(),
                start,
                PendingStartupReceipt::unattempted(
                    journal,
                    campaign_id,
                    seat,
                    7,
                    crate::journal::StartupReceipt {
                        revision: 0,
                        requested_ms: 1000,
                        reserved_ms: 1000,
                        completed: false,
                        elapsed_ms: None,
                        returned_ok: None,
                    },
                ),
            ));
            root.install(&mut candidate).unwrap();
            let info = cna_seats::driver::SessionInfo {
                session_id: uuid::Uuid::new_v4().to_string(),
                model: None,
                cli_version: None,
                resumed: true,
            };
            let startup_pointer = info.session_id.as_ptr();
            let error = String::from("first original cleanup error");
            let error_pointer = error.as_ptr();
            let payload = Box::new(Opaque(std::cell::Cell::new(41)));
            let payload_pointer = std::ptr::from_ref(payload.as_ref());
            let mut driver = Box::new(CleanupDriver {
                info: Some(info),
                returned: Some(if mode == 1 {
                    Err(DriverError::Protocol(error))
                } else {
                    drop(error);
                    Ok(())
                }),
                mode,
                starts: 0,
                stops: 0,
                canceled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                payload: std::sync::Mutex::new(if mode == 3 {
                    Some(payload)
                } else {
                    drop(payload);
                    None
                }),
            });
            let driver_pointer = std::ptr::from_ref(driver.as_ref());
            let elapsed_before;
            {
                let mut lease = root.checkout(&original).unwrap();
                let owner = lease.owner_mut();
                assert_eq!(
                    observe_journal_start_cleanup(owner, &original, driver.as_mut()).await,
                    Err(StartupCleanupRefusal::Unprepared)
                );
                owner.prepare().unwrap();
                assert_eq!(
                    observe_journal_start_cleanup(owner, &original, driver.as_mut()).await,
                    Err(StartupCleanupRefusal::Unattempted)
                );
                owner.bind_receipt().unwrap();
                supervise_journal_start(owner, driver.as_mut(), Some("original resume argument"))
                    .await
                    .unwrap();
                elapsed_before = owner.elapsed_ms;
            }
            {
                let borrowed = async {
                    let mut lease = root.checkout(&original).unwrap();
                    let impostor = cna_seats::operation::Operation::new(
                        original.id().into(),
                        7,
                        original.allowance(),
                    )
                    .unwrap();
                    assert_eq!(
                        observe_journal_start_cleanup(
                            lease.owner_mut(),
                            &impostor,
                            driver.as_mut()
                        )
                        .await,
                        Err(StartupCleanupRefusal::Identity)
                    );
                    observe_journal_start_cleanup(lease.owner_mut(), &original, driver.as_mut())
                        .await
                };
                match mode {
                    2 => {
                        let mut borrowed = std::pin::pin!(borrowed);
                        std::future::poll_fn(|context| {
                            assert!(
                                std::future::Future::poll(borrowed.as_mut(), context).is_pending()
                            );
                            std::task::Poll::Ready(())
                        })
                        .await;
                        assert_eq!(root.position(), JournalStartPosition::InService);
                    }
                    3 => {
                        let panic = AssertUnwindSafe(borrowed).catch_unwind().await.unwrap_err();
                        let payload = panic.downcast::<Opaque>().ok().unwrap();
                        assert_eq!(std::ptr::from_ref(payload.as_ref()), payload_pointer);
                        assert_eq!(payload.0.get(), 41);
                    }
                    4 => drop(borrowed),
                    _ => assert_eq!(borrowed.await, Ok(mode == 0)),
                }
            }
            assert_eq!(std::ptr::from_ref(driver.as_ref()), driver_pointer);
            assert_eq!(root.position(), JournalStartPosition::Installed);
            assert_eq!(driver.starts, 1);
            assert_eq!(driver.stops, usize::from(mode != 4));
            let mut lease = root.checkout(&original).unwrap();
            let owner = lease.owner_mut();
            assert!(owner.operation.same_instance(&original));
            assert_eq!(
                owner.operation.allowance().deadline(),
                original.allowance().deadline()
            );
            assert!(owner.supervisor_observed);
            assert_eq!(owner.elapsed_ms, elapsed_before);
            assert_eq!(
                owner
                    .outcome
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .session_id
                    .as_ptr(),
                startup_pointer
            );
            assert_eq!(owner.cleanup_started, mode != 4);
            match owner.cleanup_result.as_ref() {
                Some(Ok(())) => assert_eq!(mode, 0),
                Some(Err(DriverError::Protocol(error))) => {
                    assert_eq!(mode, 1);
                    assert_eq!(error.as_ptr(), error_pointer);
                }
                None => assert!(matches!(mode, 2..=4)),
                _ => panic!("unexpected retained cleanup return"),
            }
            if mode != 4 {
                for _ in 0..2 {
                    assert_eq!(
                        observe_journal_start_cleanup(owner, &original, driver.as_mut()).await,
                        Err(StartupCleanupRefusal::AlreadyOwned)
                    );
                }
                assert_eq!(driver.stops, 1);
            }
            match owner.cleanup_result.as_ref() {
                Some(Ok(())) => assert_eq!(mode, 0),
                Some(Err(DriverError::Protocol(error))) => {
                    assert_eq!(mode, 1);
                    assert_eq!(error.as_ptr(), error_pointer);
                }
                None => assert!(matches!(mode, 2..=4)),
                _ => panic!("cleanup return changed after refused repeats"),
            }
            if mode == 2 {
                assert!(driver.canceled.load(std::sync::atomic::Ordering::SeqCst));
            }
            drop(lease);
            assert!(root.position().is_owned()); // Cleanup return is NOT a release or join witness.
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Metadata builder callbacks and local journal,
    // not an executed Demo/host-admission/driver-binding/native fence witness.
    #[tokio::test]
    #[cfg(panic = "unwind")]
    async fn startup_context_keeps_first_body_error_or_unwind_and_cannot_start() {
        use crate::config::{GameKind, LaunchConfig, SessionLimits};
        use cna_seats::host::*;
        use std::cell::Cell;
        use std::panic::{AssertUnwindSafe, catch_unwind};
        struct NoHostCalls;
        impl HostAdmission for NoHostCalls {
            fn subscribe(&self) -> watch::Receiver<HostSnapshot> {
                panic!("no host callback permitted in metadata fixture")
            }
            fn snapshot(&self) -> Result<HostSnapshot, HostRefusal> {
                panic!("no host callback permitted in metadata fixture")
            }
            fn acquire(
                &self,
                _: &HostIdentity,
                _: HostStep,
            ) -> Result<Box<dyn HostStepLease>, HostRefusal> {
                panic!("no host callback permitted in metadata fixture")
            }
            fn acquire_prompt(
                &self,
                _: &HostIdentity,
                _: cna_seats::operation::Allowance,
            ) -> Result<Box<dyn HostPromptLease>, HostRefusal> {
                panic!("no host callback permitted in metadata fixture")
            }
        }
        struct Opaque(Cell<u32>);
        for case in 0..12 {
            let seat: SeatId = "axis.front_line".parse().unwrap();
            let directory = tempfile::tempdir().unwrap();
            let campaign_id = uuid::Uuid::new_v4().to_string();
            let mut config = LaunchConfig::resolve(
                GameKind::Cna,
                &[
                    "axis.front_line=claude:haiku".into(),
                    "*=scripted:legal_random".into(),
                ],
            )
            .unwrap();
            config.session = Some(SessionLimits::default());
            let journal = Arc::new(
                crate::journal::SessionJournal::create(
                    directory.path(),
                    &campaign_id,
                    config,
                    &std::collections::BTreeMap::from([(seat, 7)]),
                )
                .unwrap(),
            );
            let start = Instant::now();
            let root = JournalStartRoot::empty(seat, 7);
            let original = cna_seats::operation::Operation::new(
                uuid::Uuid::new_v4().to_string(),
                7,
                cna_seats::operation::Allowance::required(Some(start + Duration::from_secs(30)))
                    .unwrap(),
            )
            .unwrap();
            let mut candidate = Some(PendingJournalStart::unprepared_with_receipt(
                original.clone(),
                start,
                PendingStartupReceipt::unattempted(
                    journal,
                    campaign_id,
                    seat,
                    7,
                    crate::journal::StartupReceipt {
                        revision: 0,
                        requested_ms: 1000,
                        reserved_ms: 1000,
                        completed: false,
                        elapsed_ms: None,
                        returned_ok: None,
                    },
                ),
            ));
            root.install(&mut candidate).unwrap();
            {
                let mut lease = root.checkout(&original).unwrap();
                let owner = lease.owner_mut();
                assert_eq!(
                    owner.capture_host_context("original decision", 3, |_| {
                        panic!("unprepared callback entered")
                    }),
                    Err(StartupContextRefusal::Unprepared)
                );
                owner.prepare().unwrap();
                assert_eq!(
                    owner.capture_host_context("original decision", 3, |_| {
                        panic!("unbound callback entered")
                    }),
                    Err(StartupContextRefusal::Unbound)
                );
                assert!(!owner.host_context_started);
                owner.bind_receipt().unwrap();
                assert_eq!(
                    owner.bind_host_context(&original, |_| panic!("contextless binding entered")),
                    Err(StartupContextRefusal::Unbound)
                );
                assert!(!owner.host_binding_started);
            }
            let calls = Cell::new(0);
            let body_pointer = Cell::new(std::ptr::null::<u8>());
            let payload = Box::new(Opaque(Cell::new(19)));
            let payload_pointer = std::ptr::from_ref(payload.as_ref());
            let returned = catch_unwind(AssertUnwindSafe(|| {
                let mut lease = root.checkout(&original).unwrap();
                lease
                    .owner_mut()
                    .capture_host_context("original decision", 3, |operation| {
                        calls.set(calls.get() + 1);
                        assert_eq!(root.position(), JournalStartPosition::InService);
                        assert!(matches!(
                            root.checkout(operation),
                            Err(JournalStartRefusal::AlreadyOwned)
                        ));
                        if case == 8 {
                            std::panic::resume_unwind(payload);
                        }
                        if case == 1 {
                            let error = String::from("first context failure");
                            body_pointer.set(error.as_ptr());
                            return Err(error);
                        }
                        let mut context = HostDriverContext {
                            admission: Arc::new(NoHostCalls),
                            identity: HostIdentity {
                                seat,
                                epoch: 7,
                                decision_id: "original decision".into(),
                                revision: 3,
                                operation_id: operation.id().into(),
                            },
                            operation: operation.clone(),
                        };
                        match case {
                            2 => {
                                context.operation = cna_seats::operation::Operation::new(
                                    operation.id().into(),
                                    7,
                                    operation.allowance(),
                                )
                                .unwrap()
                            }
                            3 => context.identity.seat = "axis.commander".parse().unwrap(),
                            4 => context.identity.epoch = 8,
                            5 => context.identity.decision_id = "foreign decision".into(),
                            6 => context.identity.revision = 4,
                            7 => context.identity.operation_id = uuid::Uuid::new_v4().to_string(),
                            _ => {}
                        }
                        body_pointer.set(context.identity.decision_id.as_ptr());
                        Ok(context)
                    })
            }));
            match case {
                0 | 9..=11 => assert_eq!(returned.unwrap(), Ok(())),
                1 => assert_eq!(returned.unwrap(), Err(StartupContextRefusal::ContextFailed)),
                8 => {
                    let payload = returned.unwrap_err().downcast::<Opaque>().ok().unwrap();
                    assert_eq!(std::ptr::from_ref(payload.as_ref()), payload_pointer);
                    assert_eq!(payload.0.get(), 19);
                }
                _ => assert_eq!(returned.unwrap(), Err(StartupContextRefusal::Identity)),
            }
            assert_eq!(root.position(), JournalStartPosition::Installed);
            assert_eq!(calls.get(), 1);
            let mut lease = root.checkout(&original).unwrap();
            let owner = lease.owner_mut();
            assert!(owner.operation.same_instance(&original));
            assert_eq!(
                owner.operation.allowance().deadline(),
                original.allowance().deadline()
            );
            assert!(owner.host_context_started);
            assert_eq!(owner.host_context_validated, case == 0 || case >= 9);
            match owner.host_context_result.as_ref() {
                Some(Ok(context)) => {
                    assert_eq!(context.identity.decision_id.as_ptr(), body_pointer.get())
                }
                Some(Err(error)) => assert_eq!(error.as_ptr(), body_pointer.get()),
                None => assert_eq!(case, 8),
            }
            for _ in 0..2 {
                assert_eq!(
                    owner.capture_host_context("original decision", 3, |_| {
                        panic!("repeated context callback entered")
                    }),
                    Err(StartupContextRefusal::AlreadyOwned)
                );
            }
            match owner.host_context_result.as_ref() {
                Some(Ok(context)) => {
                    assert_eq!(context.identity.decision_id.as_ptr(), body_pointer.get())
                }
                Some(Err(error)) => assert_eq!(error.as_ptr(), body_pointer.get()),
                None => assert_eq!(case, 8),
            }
            drop(lease);
            if case >= 9 {
                let binding_calls = Cell::new(0);
                let binding_error_pointer = Cell::new(std::ptr::null::<u8>());
                let payload = Box::new(Opaque(Cell::new(29)));
                let payload_pointer = std::ptr::from_ref(payload.as_ref());
                let binding = catch_unwind(AssertUnwindSafe(|| {
                    let mut lease = root.checkout(&original).unwrap();
                    let owner = lease.owner_mut();
                    let impostor = cna_seats::operation::Operation::new(
                        original.id().into(),
                        7,
                        original.allowance(),
                    )
                    .unwrap();
                    assert_eq!(
                        owner.bind_host_context(&impostor, |_| panic!("foreign binding entered")),
                        Err(StartupContextRefusal::Identity)
                    );
                    assert!(!owner.host_binding_started);
                    owner.bind_host_context(&original, |context| {
                        binding_calls.set(binding_calls.get() + 1);
                        assert_eq!(root.position(), JournalStartPosition::InService);
                        assert!(context.operation.same_instance(&original));
                        assert_eq!(
                            context.operation.allowance().deadline(),
                            original.allowance().deadline()
                        );
                        assert_eq!(context.identity.decision_id, "original decision");
                        assert_ne!(context.identity.decision_id.as_ptr(), body_pointer.get());
                        assert_eq!(context.identity.operation_id, original.id());
                        if case == 11 {
                            std::panic::resume_unwind(payload);
                        }
                        if case == 10 {
                            let error = String::from("first binding failure");
                            binding_error_pointer.set(error.as_ptr());
                            return Err(DriverError::Protocol(error));
                        }
                        Ok(())
                    })
                }));
                match case {
                    9 => assert_eq!(binding.unwrap(), Ok(())),
                    10 => assert_eq!(binding.unwrap(), Err(StartupContextRefusal::BindingFailed)),
                    11 => {
                        let payload = binding.unwrap_err().downcast::<Opaque>().ok().unwrap();
                        assert_eq!(std::ptr::from_ref(payload.as_ref()), payload_pointer);
                        assert_eq!(payload.0.get(), 29);
                    }
                    _ => unreachable!(),
                }
                assert_eq!(root.position(), JournalStartPosition::Installed);
                let mut lease = root.checkout(&original).unwrap();
                let owner = lease.owner_mut();
                assert!(owner.host_binding_started);
                for _ in 0..2 {
                    assert_eq!(
                        owner.bind_host_context(&original, |_| panic!("repeat binding entered")),
                        Err(StartupContextRefusal::AlreadyOwned)
                    );
                }
                match owner.host_binding_result.as_ref() {
                    Some(Ok(())) => assert_eq!(case, 9),
                    Some(Err(DriverError::Protocol(error))) => {
                        assert_eq!(case, 10);
                        assert_eq!(error.as_ptr(), binding_error_pointer.get());
                    }
                    None => assert_eq!(case, 11),
                    _ => panic!("unexpected original binding result"),
                }
                assert_eq!(binding_calls.get(), 1);
            }
            let mut lease = root.checkout(&original).unwrap();
            let owner = lease.owner_mut();
            match owner.host_context_result.as_ref() {
                Some(Ok(context)) => {
                    assert_eq!(context.identity.decision_id.as_ptr(), body_pointer.get())
                }
                Some(Err(error)) => assert_eq!(error.as_ptr(), body_pointer.get()),
                None => assert_eq!(case, 8),
            }
            if (1..=8).contains(&case) {
                assert_eq!(
                    owner.bind_host_context(&original, |_| panic!(
                        "invalid context binding entered"
                    )),
                    Err(StartupContextRefusal::Unbound)
                );
                assert!(!owner.host_binding_started);
                assert!(owner.host_binding_result.is_none());
            }
            let mut driver = start_driver(None, false);
            assert!(
                supervise_journal_start(owner, &mut driver, Some("original resume argument"))
                    .await
                    .is_err()
            );
            assert_eq!(driver.calls, 0); // Neither metadata nor binding ACK opens the held native gate.
            assert!(!owner.supervisor_started);
            assert!(owner.outcome.is_none());
            assert_eq!(calls.get(), 1);
            drop(lease);
            assert!(root.position().is_owned());
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Root/lease controls with local journal and
    // mock driver. No actual Demo/MCP/HTTP/native process is started here.
    #[tokio::test]
    #[cfg(panic = "unwind")]
    async fn startup_root_restores_same_owner_on_unpolled_cancel_unwind_and_poison() {
        use crate::config::{GameKind, LaunchConfig, SessionLimits};
        use futures_util::FutureExt;
        use std::panic::AssertUnwindSafe;
        struct Opaque(std::cell::Cell<u32>);
        for case in 0..4 {
            let seat: SeatId = "axis.front_line".parse().unwrap();
            let directory = tempfile::tempdir().unwrap();
            let campaign_id = uuid::Uuid::new_v4().to_string();
            let mut config = LaunchConfig::resolve(
                GameKind::Cna,
                &[
                    "axis.front_line=claude:haiku".into(),
                    "*=scripted:legal_random".into(),
                ],
            )
            .unwrap();
            config.session = Some(SessionLimits::default());
            let journal = std::sync::Arc::new(
                crate::journal::SessionJournal::create(
                    directory.path(),
                    &campaign_id,
                    config,
                    &std::collections::BTreeMap::from([(seat, 7)]),
                )
                .unwrap(),
            );
            let root = JournalStartRoot::empty(seat, 7);
            let start = Instant::now();
            let original = cna_seats::operation::Operation::new(
                uuid::Uuid::new_v4().to_string(),
                7,
                cna_seats::operation::Allowance::required(Some(start + Duration::from_secs(30)))
                    .unwrap(),
            )
            .unwrap();
            let bound = crate::journal::StartupReceipt {
                revision: 0,
                requested_ms: 1000,
                reserved_ms: 1000,
                completed: false,
                elapsed_ms: None,
                returned_ok: None,
            };
            let make_candidate = || {
                Some(PendingJournalStart::unprepared_with_receipt(
                    original.clone(),
                    start,
                    PendingStartupReceipt::unattempted(
                        journal.clone(),
                        campaign_id.clone(),
                        seat,
                        7,
                        bound,
                    ),
                ))
            };
            let mut candidate = make_candidate();
            assert_eq!(root.position(), JournalStartPosition::Empty);
            assert!(!root.position().is_owned());
            let campaign_pointer = candidate
                .as_ref()
                .unwrap()
                .receipt
                .as_ref()
                .unwrap()
                .campaign_id
                .as_ptr();
            let wrong_epoch = JournalStartRoot::empty(seat, 8);
            assert_eq!(
                wrong_epoch.install(&mut candidate),
                Err(JournalStartRefusal::Identity)
            );
            assert!(
                candidate
                    .as_ref()
                    .unwrap()
                    .operation
                    .same_instance(&original)
            );
            assert_eq!(
                candidate
                    .as_ref()
                    .unwrap()
                    .receipt
                    .as_ref()
                    .unwrap()
                    .campaign_id
                    .as_ptr(),
                campaign_pointer
            );
            assert_eq!(wrong_epoch.position(), JournalStartPosition::Empty);
            root.install(&mut candidate).unwrap();
            assert!(candidate.is_none());
            assert_eq!(root.position(), JournalStartPosition::Installed);
            assert!(root.position().is_owned());
            let mut duplicate = make_candidate();
            assert_eq!(
                root.install(&mut duplicate),
                Err(JournalStartRefusal::AlreadyOwned)
            );
            assert!(
                duplicate
                    .as_ref()
                    .unwrap()
                    .operation
                    .same_instance(&original)
            );
            let impostor =
                cna_seats::operation::Operation::new(original.id().into(), 7, original.allowance())
                    .unwrap();
            assert!(matches!(
                root.checkout(&impostor),
                Err(JournalStartRefusal::Identity)
            ));
            let info = cna_seats::driver::SessionInfo {
                session_id: uuid::Uuid::new_v4().to_string(),
                model: Some("root retained model".into()),
                cli_version: None,
                resumed: true,
            };
            let body_pointer = info.session_id.as_ptr();
            let mut driver = start_driver(Some(Ok(info)), case == 1);
            let payload = Box::new(Opaque(std::cell::Cell::new(91)));
            let payload_pointer = std::ptr::from_ref(payload.as_ref());
            if case == 2 {
                *driver.telemetry_panic.get_mut().unwrap() = Some(payload);
            } else {
                drop(payload);
            }
            if case == 3 {
                let mut lease = root.checkout(&original).unwrap();
                assert!(lease.owner_mut().operation.same_instance(&original));
                // Deliberately poison ONLY the private metadata lock. This is
                // a synthetic internal fault, not a driver/native panic witness.
                let poisoned = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    let _guard = root.slot.lock().unwrap();
                    panic!("source metadata poison control");
                }));
                assert!(poisoned.is_err());
                drop(lease); // Recover poison ONLY for ownership restoration.
                assert_eq!(root.position(), JournalStartPosition::Unknown);
                assert!(root.position().is_owned());
                assert!(matches!(
                    root.checkout(&original),
                    Err(JournalStartRefusal::Busy)
                ));
                assert_eq!(root.install(&mut duplicate), Err(JournalStartRefusal::Busy));
                let slot = match root.slot.lock() {
                    Err(poisoned) => poisoned.into_inner(),
                    Ok(_) => panic!("metadata poison was not retained"),
                };
                assert!(!slot.in_service);
                let restored = slot.owner.as_ref().unwrap();
                assert!(restored.operation.same_instance(&original));
                assert!(!restored.preparation_started);
                assert!(!restored.receipt.as_ref().unwrap().binding_started);
                continue;
            }
            {
                let borrowed = async {
                    let mut lease = root.checkout(&original).unwrap();
                    assert_eq!(root.position(), JournalStartPosition::InService);
                    assert!(root.position().is_owned());
                    assert!(matches!(
                        root.checkout(&original),
                        Err(JournalStartRefusal::AlreadyOwned)
                    ));
                    assert_eq!(
                        root.install(&mut duplicate),
                        Err(JournalStartRefusal::AlreadyOwned)
                    );
                    let owner = lease.owner_mut();
                    owner.prepare().unwrap();
                    owner.bind_receipt().unwrap();
                    supervise_journal_start(owner, &mut driver, Some("original resume argument"))
                        .await
                };
                if case == 0 {
                    drop(borrowed);
                }
                // Unpolled: no checkout/preparation/dispatch.
                else if case == 1 {
                    let mut borrowed = std::pin::pin!(borrowed);
                    std::future::poll_fn(|context| {
                        assert!(std::future::Future::poll(borrowed.as_mut(), context).is_pending());
                        std::task::Poll::Ready(())
                    })
                    .await;
                    assert_eq!(root.position(), JournalStartPosition::InService);
                }
                // Drop polled future: cancel mock start and restore SAME owner.
                else {
                    let panic = AssertUnwindSafe(borrowed).catch_unwind().await.unwrap_err();
                    let payload = panic.downcast::<Opaque>().ok().unwrap();
                    assert_eq!(std::ptr::from_ref(payload.as_ref()), payload_pointer);
                    assert_eq!(payload.0.get(), 91);
                }
            }
            assert_eq!(root.position(), JournalStartPosition::Installed);
            assert!(root.position().is_owned());
            let mut lease = root.checkout(&original).unwrap();
            let restored = lease.owner_mut();
            assert!(restored.operation.same_instance(&original));
            assert_eq!(
                restored.operation.allowance().deadline(),
                original.allowance().deadline()
            );
            assert_eq!(driver.calls, usize::from(case != 0));
            if case == 0 {
                assert!(!restored.preparation_started);
                assert!(!restored.receipt.as_ref().unwrap().binding_started);
                assert!(!restored.supervisor_started);
            } else {
                assert!(restored.preparation_complete());
                assert!(
                    restored
                        .receipt
                        .as_ref()
                        .unwrap()
                        .original_bind_returned_ok()
                );
                assert!(restored.supervisor_started);
                assert!(!restored.supervisor_observed);
                if case == 1 {
                    assert!(restored.outcome.is_none());
                } else {
                    assert_eq!(
                        restored
                            .outcome
                            .as_ref()
                            .unwrap()
                            .as_ref()
                            .unwrap()
                            .session_id
                            .as_ptr(),
                        body_pointer
                    );
                }
                assert!(
                    supervise_journal_start(
                        restored,
                        &mut driver,
                        Some("original resume argument")
                    )
                    .await
                    .is_err()
                );
                assert_eq!(driver.calls, 1);
            }
            drop(lease);
            assert_eq!(root.position(), JournalStartPosition::Installed);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Actual proposed private owner/journal
    // callbacks, with a mock driver only. Cases3-5 explicitly synthesize a lost
    // return slot after commit, not a process crash/unwind or native witness.
    #[tokio::test]
    async fn startup_receipt_owner_keeps_first_results_and_never_dispatches_after_reconciliation() {
        use crate::config::{GameKind, LaunchConfig, SessionLimits};
        for case in 0..6 {
            let seat: SeatId = "axis.front_line".parse().unwrap();
            let root = tempfile::tempdir().unwrap();
            let campaign_id = uuid::Uuid::new_v4().to_string();
            let mut config = LaunchConfig::resolve(
                GameKind::Cna,
                &[
                    "axis.front_line=claude:haiku".into(),
                    "*=scripted:legal_random".into(),
                ],
            )
            .unwrap();
            config.session = Some(SessionLimits::default());
            let journal = std::sync::Arc::new(
                crate::journal::SessionJournal::create(
                    root.path(),
                    &campaign_id,
                    config.clone(),
                    &std::collections::BTreeMap::from([(seat, 7)]),
                )
                .unwrap(),
            );
            let start = Instant::now();
            let original = cna_seats::operation::Operation::new(
                uuid::Uuid::new_v4().to_string(),
                7,
                cna_seats::operation::Allowance::required(Some(start + Duration::from_secs(30)))
                    .unwrap(),
            )
            .unwrap();
            let bound = crate::journal::StartupReceipt {
                revision: 0,
                requested_ms: 1000,
                reserved_ms: 1000,
                completed: false,
                elapsed_ms: None,
                returned_ok: None,
            };
            let context = PendingStartupReceipt::unattempted(
                journal.clone(),
                if case == 1 {
                    "foreign-campaign".into()
                } else {
                    campaign_id.clone()
                },
                seat,
                7,
                bound,
            );
            // Stack fixture only; actual Demo/factory root installation is still
            // pending. Context is MOVED into owner before borrowed preparation.
            let mut owner =
                PendingJournalStart::unprepared_with_receipt(original.clone(), start, context);
            assert_eq!(owner.bind_receipt(), Err(StartupReceiptRefusal::Unprepared));
            assert!(!owner.receipt.as_ref().unwrap().binding_started);
            owner.prepare().unwrap();
            if case != 0 {
                assert_eq!(
                    owner.bind_receipt(),
                    if case == 1 {
                        Err(StartupReceiptRefusal::JournalFailed)
                    } else {
                        Ok(())
                    }
                );
            }
            if case == 3 {
                // SYNTHETIC lost returned ACK, actual binding commit retained.
                owner.receipt.as_mut().unwrap().binding_result =
                    Some(Err("first binding return retained".into()));
            } else if case == 4 {
                owner.receipt.as_mut().unwrap().binding_result = None; // SYNTHETIC in-service slot.
            }
            let binding_pointer = owner
                .receipt
                .as_ref()
                .unwrap()
                .binding_result
                .as_ref()
                .and_then(|result| result.as_ref().err())
                .map(|error| error.as_ptr());
            if case == 3 || case == 4 {
                assert_eq!(owner.reconcile_receipt(false), Ok(()));
                assert_eq!(
                    owner.reconcile_receipt(false),
                    Err(StartupReceiptRefusal::AlreadyOwned)
                );
                assert_eq!(
                    owner.bind_receipt(),
                    Err(StartupReceiptRefusal::AlreadyOwned)
                );
                assert!(!owner.receipt.as_ref().unwrap().original_bind_returned_ok());
            }
            let info = cna_seats::driver::SessionInfo {
                session_id: uuid::Uuid::new_v4().to_string(),
                model: Some("retained receipt owner model".into()),
                cli_version: None,
                resumed: false,
            };
            let body_pointer = info.session_id.as_ptr();
            let mut driver = start_driver(Some(Ok(info)), false);
            if case == 2 || case == 5 {
                supervise_journal_start(&mut owner, &mut driver, Some("original resume argument"))
                    .await
                    .unwrap();
                assert_eq!(driver.calls, 1);
                assert_eq!(
                    owner
                        .outcome
                        .as_ref()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .session_id
                        .as_ptr(),
                    body_pointer
                );
                let other = crate::journal::SessionJournal::create(
                    root.path(),
                    &uuid::Uuid::new_v4().to_string(),
                    config,
                    &std::collections::BTreeMap::from([(seat, 7)]),
                )
                .unwrap();
                assert_eq!(
                    owner.write_session_candidate(&original, &other, seat),
                    Err(SessionStorageRefusal::JournalBinding)
                );
                assert!(!owner.storage_started);
                // Keep metadata commit separate from original accounting commit.
                owner
                    .write_session_candidate(&original, &journal, seat)
                    .unwrap();
                owner.complete_receipt().unwrap();
                let first_completion = *owner
                    .receipt
                    .as_ref()
                    .unwrap()
                    .completion_result
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap();
                assert_eq!(
                    first_completion,
                    owner.receipt.as_ref().unwrap().completion_expected.unwrap()
                );
                if case == 5 {
                    owner.receipt.as_mut().unwrap().completion_result =
                        Some(Err("first completion return retained".into()));
                    let first_pointer = owner
                        .receipt
                        .as_ref()
                        .unwrap()
                        .completion_result
                        .as_ref()
                        .unwrap()
                        .as_ref()
                        .unwrap_err()
                        .as_ptr();
                    assert_eq!(owner.reconcile_receipt(true), Ok(()));
                    assert_eq!(
                        owner.reconcile_receipt(true),
                        Err(StartupReceiptRefusal::AlreadyOwned)
                    );
                    assert_eq!(
                        owner
                            .receipt
                            .as_ref()
                            .unwrap()
                            .completion_result
                            .as_ref()
                            .unwrap()
                            .as_ref()
                            .unwrap_err()
                            .as_ptr(),
                        first_pointer
                    );
                }
                for _ in 0..2 {
                    assert_eq!(
                        owner.complete_receipt(),
                        Err(StartupReceiptRefusal::AlreadyOwned)
                    );
                    assert!(
                        supervise_journal_start(
                            &mut owner,
                            &mut driver,
                            Some("original resume argument")
                        )
                        .await
                        .is_err()
                    );
                    assert_eq!(driver.calls, 1);
                    assert_eq!(
                        owner
                            .outcome
                            .as_ref()
                            .unwrap()
                            .as_ref()
                            .unwrap()
                            .session_id
                            .as_ptr(),
                        body_pointer
                    );
                }
            } else {
                for _ in 0..2 {
                    assert!(
                        supervise_journal_start(
                            &mut owner,
                            &mut driver,
                            Some("original resume argument")
                        )
                        .await
                        .is_err()
                    );
                    assert_eq!(driver.calls, 0);
                    assert!(!owner.supervisor_started);
                    assert!(owner.outcome.is_none());
                    assert_eq!(
                        owner.complete_receipt(),
                        Err(StartupReceiptRefusal::Unobserved)
                    );
                }
            }
            assert_eq!(
                owner
                    .receipt
                    .as_ref()
                    .unwrap()
                    .binding_result
                    .as_ref()
                    .and_then(|result| result.as_ref().err())
                    .map(|error| error.as_ptr()),
                binding_pointer
            );
            let snapshot = journal.snapshot().unwrap();
            assert_eq!(snapshot.seats[&seat].turns, 0);
            assert_eq!(
                snapshot.seats[&seat].start_accounting,
                Some(Default::default())
            );
            assert!(snapshot.seats[&seat].start_operations.is_empty());
            assert!(
                snapshot.seats[&seat]
                    .usage
                    .as_ref()
                    .unwrap()
                    .outbox
                    .is_empty()
            );
            assert!(owner.operation.same_instance(&original));
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. An actual fit refusal retains its typed
    // cause and original allocation. Case 3 is synthetic in-service state,
    // not evidence of a real preparation unwind or native absence.
    #[tokio::test]
    async fn startup_preparation_retains_first_result_and_refuses_reentry() {
        use cna_seats::operation::{OPERATION_BYTES, Operation, OperationError};
        for case in 0..4 {
            let start = Instant::now();
            let original = Operation::new(
                format!("source-start-preparation-{case}"),
                7,
                cna_seats::operation::Allowance::required(Some(start + Duration::from_secs(30)))
                    .unwrap(),
            )
            .unwrap();
            let deadline = original.allowance().deadline();
            // Source fixture's stack owner stands in for the future canonical
            // root. Actual Demo/factory/root installation remains unadopted.
            let mut owner = PendingJournalStart::unprepared(original.clone(), start);
            assert!(!owner.preparation_complete());
            match case {
                0 => {}
                1 => {
                    original
                        .charge(OPERATION_BYTES - original.captured_bytes())
                        .unwrap();
                    assert_eq!(owner.prepare(), Err(OperationError::CaptureOverflow));
                    assert_eq!(
                        owner.preparation_result,
                        Some(Err(OperationError::CaptureOverflow))
                    );
                }
                2 => {
                    assert_eq!(owner.prepare(), Ok(()));
                    assert!(owner.preparation_complete());
                }
                3 => owner.preparation_started = true, // Synthetic no-result state only.
                _ => unreachable!(),
            }
            let first = owner.preparation_result;
            let bytes = original.captured_bytes();
            if case != 0 {
                for _ in 0..2 {
                    assert_eq!(owner.prepare(), Err(OperationError::CreditOwned));
                    assert_eq!(owner.preparation_result, first);
                    assert_eq!(original.captured_bytes(), bytes);
                }
            }
            if case != 2 {
                let mut driver = start_driver(None, true);
                for _ in 0..2 {
                    assert!(matches!(
                        supervise_journal_start(&mut owner, &mut driver, None).await,
                        Err(DriverError::Host(cna_seats::host::HostRefusal::Unknown))
                    ));
                    assert_eq!(driver.calls, 0);
                    assert_eq!(
                        driver
                            .telemetry_calls
                            .load(std::sync::atomic::Ordering::SeqCst),
                        0
                    );
                    assert!(!owner.supervisor_started);
                    assert!(owner.outcome.is_none());
                    assert!(!owner.supervisor_observed);
                    assert!(!owner.storage_started);
                    assert!(owner.storage_result.is_none());
                    assert_eq!(owner.preparation_result, first);
                }
            }
            assert!(owner.operation.same_instance(&original));
            assert_eq!(owner.operation.allowance().deadline(), deadline);
            assert_eq!(original.captured_bytes(), bytes);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Exercise the actual private supervisor with
    // returned Ok/error and timer win, including a non-Sync opaque callback unwind.
    #[tokio::test]
    #[cfg(panic = "unwind")]
    async fn journal_start_retains_typed_body_before_telemetry() {
        use futures_util::FutureExt;
        use std::panic::AssertUnwindSafe;
        struct Opaque(std::cell::Cell<usize>);
        for case in 0..5 {
            let start = Instant::now();
            let original = cna_seats::operation::Operation::new(
                format!("source-start-result-{case}"),
                7,
                cna_seats::operation::Allowance::required(Some(
                    start + Duration::from_millis(if case == 2 { 30 } else { 30_000 }),
                ))
                .unwrap(),
            )
            .unwrap();
            let deadline = original.allowance().deadline();
            let mut owner = PendingJournalStart::unprepared(original.clone(), start);
            owner.prepare().unwrap();
            let info = cna_seats::driver::SessionInfo {
                session_id: "retained original session body".into(),
                model: Some("retained model body".into()),
                cli_version: Some("retained version body".into()),
                resumed: true,
            };
            let pointers = (
                info.session_id.as_ptr(),
                info.model.as_ref().unwrap().as_ptr(),
                info.cli_version.as_ref().unwrap().as_ptr(),
            );
            let error = String::from("retained original startup refusal body");
            let error_pointer = error.as_ptr();
            let outcome = match case {
                0 | 3 => Some(Ok(info)),
                1 | 4 => Some(Err(DriverError::Died(error))),
                2 => None,
                _ => unreachable!(),
            };
            let mut driver = start_driver(outcome, case == 2);
            let payload = Box::new(Opaque(std::cell::Cell::new(73)));
            let payload_pointer = std::ptr::from_ref(payload.as_ref());
            if case <= 2 {
                *driver.telemetry_panic.get_mut().unwrap() = Some(payload);
            } else {
                drop(payload);
            }
            let bytes = original.captured_bytes();
            let result = AssertUnwindSafe(supervise_journal_start(
                &mut owner,
                &mut driver,
                Some("original resume argument"),
            ))
            .catch_unwind()
            .await;
            match result {
                Err(payload) if case <= 2 => {
                    let payload = payload.downcast::<Opaque>().ok().unwrap();
                    assert_eq!(std::ptr::from_ref(payload.as_ref()), payload_pointer);
                    assert_eq!(payload.0.get(), 73);
                }
                Ok(Ok(())) if case >= 3 => {}
                _ => panic!("startup observation control did not match"),
            }
            match (case, owner.outcome.as_ref().unwrap()) {
                (0 | 3, Ok(info)) => {
                    assert_eq!(
                        (
                            info.session_id.as_ptr(),
                            info.model.as_ref().unwrap().as_ptr(),
                            info.cli_version.as_ref().unwrap().as_ptr(),
                        ),
                        pointers
                    );
                    assert!(info.resumed);
                }
                (1 | 4, Err(DriverError::Died(error))) => {
                    assert_eq!(error.as_ptr(), error_pointer);
                    assert_eq!(error, "retained original startup refusal body");
                }
                (2, Err(DriverError::Timeout)) => {}
                _ => panic!("original startup result was replaced"),
            }
            assert!(owner.supervisor_started);
            assert_eq!(owner.supervisor_observed, case >= 3);
            assert!(owner.elapsed_ms > 0);
            assert_eq!(driver.calls, 1);
            assert!(driver.cancelled.load(std::sync::atomic::Ordering::SeqCst));
            for _ in 0..2 {
                assert!(
                    supervise_journal_start(&mut owner, &mut driver, None)
                        .await
                        .is_err()
                );
            }
            assert_eq!(driver.calls, 1);
            assert_eq!(
                driver
                    .telemetry_calls
                    .load(std::sync::atomic::Ordering::SeqCst),
                1
            );
            assert!(owner.operation.same_instance(&original));
            assert_eq!(owner.operation.allowance().deadline(), deadline);
            assert_eq!(original.captured_bytes(), bytes);
            // Inert fixture has no native evidence. This is NOT native absence,
            // original joins, SessionInfo freshness, journal CAS or healthy start.
            assert!(matches!(
                original.start_evidence(),
                cna_seats::operation::StartEvidence::NotStarted
            ));
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Empty result after cancellation/unwind cannot
    // reopen the original; wholly unpolled outer borrow has not dispatched.
    #[tokio::test]
    #[cfg(panic = "unwind")]
    async fn journal_start_empty_result_does_not_permit_another_dispatch() {
        use std::{
            future::Future,
            panic::{AssertUnwindSafe, catch_unwind},
            task::{Context, Poll, Waker},
        };
        struct Opaque(std::cell::Cell<usize>);
        for case in 0..3 {
            let start = Instant::now();
            let original = cna_seats::operation::Operation::new(
                format!("source-start-cancel-{case}"),
                7,
                cna_seats::operation::Allowance::required(Some(start + Duration::from_secs(30)))
                    .unwrap(),
            )
            .unwrap();
            let deadline = original.allowance().deadline();
            let mut owner = PendingJournalStart::unprepared(original.clone(), start);
            owner.prepare().unwrap();
            let mut driver = start_driver(None, true);
            let bytes = original.captured_bytes();
            if case == 2 {
                drop(supervise_journal_start(
                    &mut owner,
                    &mut driver,
                    Some("original resume argument"),
                ));
                assert!(!owner.supervisor_started);
                assert_eq!(driver.calls, 0);
                assert!(!driver.cancelled.load(std::sync::atomic::Ordering::SeqCst));
            }
            let payload = Box::new(Opaque(std::cell::Cell::new(73)));
            let pointer = std::ptr::from_ref(payload.as_ref());
            if case == 1 {
                *driver.start_panic.get_mut().unwrap() = Some(payload);
            } else {
                drop(payload);
            }
            let mut context = Context::from_waker(Waker::noop());
            let result = catch_unwind(AssertUnwindSafe(|| {
                let mut borrowed = std::pin::pin!(supervise_journal_start(
                    &mut owner,
                    &mut driver,
                    Some("original resume argument")
                ));
                assert!(matches!(
                    borrowed.as_mut().poll(&mut context),
                    Poll::Pending
                ));
            }));
            match (case, result) {
                (1, Err(payload)) => {
                    let payload = payload.downcast::<Opaque>().ok().unwrap();
                    assert_eq!(std::ptr::from_ref(payload.as_ref()), pointer);
                    assert_eq!(payload.0.get(), 73);
                }
                (0 | 2, Ok(())) => {}
                _ => panic!("startup cancellation control did not match"),
            }
            assert!(owner.supervisor_started && owner.outcome.is_none());
            assert!(!owner.supervisor_observed);
            assert_eq!(driver.calls, 1);
            assert!(driver.cancelled.load(std::sync::atomic::Ordering::SeqCst));
            for _ in 0..2 {
                let mut repeat =
                    std::pin::pin!(supervise_journal_start(&mut owner, &mut driver, None));
                assert!(matches!(
                    repeat.as_mut().poll(&mut context),
                    Poll::Ready(Err(_))
                ));
            }
            assert_eq!(driver.calls, 1);
            assert_eq!(
                driver
                    .telemetry_calls
                    .load(std::sync::atomic::Ordering::SeqCst),
                0
            );
            assert!(owner.operation.same_instance(&original));
            assert_eq!(owner.operation.allowance().deadline(), deadline);
            assert_eq!(original.captured_bytes(), bytes);
            assert!(owner.outcome.is_none() && !owner.supervisor_observed);
            // Synthetic fixture state only, never a native-absence/join witness.
            assert!(matches!(
                original.start_evidence(),
                cna_seats::operation::StartEvidence::NotStarted
            ));
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Planned local SQLite metadata fixture only.
    // No actual startup receipt/CAS, native child, archive or healthy-start proof.
    #[tokio::test]
    #[cfg(panic = "unwind")]
    async fn retained_startup_storage_is_one_attempt_and_keeps_both_results() {
        use crate::config::{GameKind, LaunchConfig, SessionLimits};
        use futures_util::FutureExt;
        use std::panic::AssertUnwindSafe;
        struct Opaque(std::cell::Cell<usize>);
        for case in 0..6 {
            let seat: SeatId = "axis.front_line".parse().unwrap();
            let root = tempfile::tempdir().unwrap();
            let mut config = LaunchConfig::resolve(
                GameKind::Cna,
                &[
                    "axis.front_line=claude:haiku".into(),
                    "*=scripted:legal_random".into(),
                ],
            )
            .unwrap();
            config.session = Some(SessionLimits::default());
            let journal = crate::journal::SessionJournal::create(
                root.path(),
                &uuid::Uuid::new_v4().to_string(),
                config,
                &std::collections::BTreeMap::from([(seat, 7)]),
            )
            .unwrap();
            let before = serde_json::to_value(journal.snapshot().unwrap()).unwrap();
            let start = Instant::now();
            let id = uuid::Uuid::new_v4().to_string();
            let allowance =
                cna_seats::operation::Allowance::required(Some(start + Duration::from_secs(30)))
                    .unwrap();
            let original = cna_seats::operation::Operation::new(id.clone(), 7, allowance).unwrap();
            // SAME text/generation/deadline is not SAME original allocation.
            let foreign = cna_seats::operation::Operation::new(id, 7, allowance).unwrap();
            let mut owner = PendingJournalStart::unprepared(original.clone(), start);
            owner.prepare().unwrap();
            let info = cna_seats::driver::SessionInfo {
                session_id: uuid::Uuid::new_v4().to_string(),
                model: Some("retained startup model".into()),
                cli_version: Some("retained startup version".into()),
                resumed: true,
            };
            let pointers = (
                info.session_id.as_ptr(),
                info.model.as_ref().unwrap().as_ptr(),
                info.cli_version.as_ref().unwrap().as_ptr(),
            );
            let error = String::from("original driver failure stays separate from storage");
            let error_pointer = error.as_ptr();
            let mut driver = start_driver(
                Some(if case == 3 {
                    Err(DriverError::Died(error))
                } else {
                    Ok(info)
                }),
                false,
            );
            let payload = Box::new(Opaque(std::cell::Cell::new(73)));
            let payload_pointer = std::ptr::from_ref(payload.as_ref());
            if case == 2 {
                *driver.telemetry_panic.get_mut().unwrap() = Some(payload);
            } else {
                drop(payload);
            }
            let returned = AssertUnwindSafe(supervise_journal_start(
                &mut owner,
                &mut driver,
                Some("original resume argument"),
            ))
            .catch_unwind()
            .await;
            match returned {
                Err(payload) if case == 2 => {
                    let payload = payload.downcast::<Opaque>().ok().unwrap();
                    assert_eq!(std::ptr::from_ref(payload.as_ref()), payload_pointer);
                    assert_eq!(payload.0.get(), 73);
                }
                Ok(Ok(())) if case != 2 => {}
                _ => panic!("original startup fixture did not return as planned"),
            }
            if case == 5 {
                // Explicit synthetic in-service marker ONLY. It exercises the
                // refusal gate, never claims a real storage unwind/lost ACK.
                owner.storage_started = true;
            }
            let expected = match case {
                0 => Ok(()),
                1 => Err(SessionStorageRefusal::StorageFailed),
                2 => Err(SessionStorageRefusal::Unobserved),
                3 => Err(SessionStorageRefusal::NotSuccessful),
                4 => Err(SessionStorageRefusal::Identity),
                5 => Err(SessionStorageRefusal::AlreadyOwned),
                _ => unreachable!(),
            };
            let target = if case == 1 {
                "axis.commander".parse().unwrap()
            } else {
                seat
            };
            let bound = if case == 4 { &foreign } else { &original };
            let charged = original.captured_bytes();
            assert_eq!(
                owner.write_session_candidate(bound, &journal, target),
                expected
            );
            let storage_error_pointer = owner
                .storage_result
                .as_ref()
                .and_then(|result| result.as_ref().err())
                .map(|error| error.as_ptr());
            let repeated = match case {
                0 | 1 | 5 => SessionStorageRefusal::AlreadyOwned,
                2 => SessionStorageRefusal::Unobserved,
                3 => SessionStorageRefusal::NotSuccessful,
                4 => SessionStorageRefusal::Identity,
                _ => unreachable!(),
            };
            for _ in 0..2 {
                assert_eq!(
                    owner.write_session_candidate(bound, &journal, target),
                    Err(repeated)
                );
            }
            assert_eq!(owner.storage_started, matches!(case, 0 | 1 | 5));
            match (case, owner.storage_result.as_ref()) {
                (0, Some(Ok(()))) => {}
                (1, Some(Err(error))) => {
                    assert_eq!(Some(error.as_ptr()), storage_error_pointer);
                    assert!(error.contains("unknown journal seat"));
                }
                (2..=5, None) => {}
                _ => panic!("storage result was replaced or inferred"),
            }
            match owner.outcome.as_ref().unwrap() {
                Ok(info) if case != 3 => {
                    assert_eq!(
                        (
                            info.session_id.as_ptr(),
                            info.model.as_ref().unwrap().as_ptr(),
                            info.cli_version.as_ref().unwrap().as_ptr()
                        ),
                        pointers
                    );
                    assert!(info.resumed);
                }
                Err(DriverError::Died(error)) if case == 3 => {
                    assert_eq!(error.as_ptr(), error_pointer)
                }
                _ => panic!("returned original result body was discarded"),
            }
            let mut expected_state = before;
            if case == 0 {
                expected_state["seats"][seat.to_string()]["session"] =
                    serde_json::to_value(owner.outcome.as_ref().unwrap().as_ref().unwrap())
                        .unwrap();
            }
            assert_eq!(
                serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
                expected_state
            );
            assert!(journal.usage_outbox(seat).unwrap().is_empty());
            assert_eq!(driver.calls, 1);
            assert_eq!(
                driver
                    .telemetry_calls
                    .load(std::sync::atomic::Ordering::SeqCst),
                1
            );
            assert!(owner.operation.same_instance(&original));
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), allowance.deadline());
            // No reservation/accounting record was created for this fixture.
            // Metadata COMMIT is not actual original startup receipt/settlement.
            assert!(journal.start_accounting_state(seat, original.id()).is_err());
        }
    }

    #[test]
    fn explicit_refusals_are_not_retried_as_process_crashes() {
        for text in [
            "usage limit reached",
            "quota exhausted",
            "authentication required",
            "rate limit exceeded",
        ] {
            assert!(!retryable_death(&DriverError::Died(text.into())));
        }
        assert!(retryable_death(&DriverError::Died(
            "process exited unexpectedly".into()
        )));
        assert!(!retryable_death(&DriverError::Timeout));
    }

    struct TimerDriver {
        error: Option<DriverError>,
        hang: bool,
        progress: Option<cna_seats::driver::TurnProgress>,
        entered: bool,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
        captured: Option<cna_seats::driver::TurnOutcome>,
        expected_capture_id: Option<String>,
        cleanup_ack: bool,
        capture_error: Option<DriverError>,
        capture_panic: std::sync::Mutex<Option<Box<dyn std::any::Any + Send>>>,
        capture_calls: std::sync::atomic::AtomicUsize,
        progress_panic: std::sync::Mutex<Option<Box<dyn std::any::Any + Send>>>,
        progress_calls: std::sync::atomic::AtomicUsize,
        telemetry_panic: std::sync::Mutex<Option<Box<dyn std::any::Any + Send>>>,
        turn_panic: std::sync::Mutex<Option<Box<dyn std::any::Any + Send>>>,
    }
    struct CancelGuard(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for CancelGuard {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    #[async_trait::async_trait]
    impl SeatDriver for TimerDriver {
        fn kind(&self) -> CliKind {
            CliKind::Claude
        }
        fn turn_progress(&self) -> Option<cna_seats::driver::TurnProgress> {
            self.progress_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let payload = self.progress_panic.lock().unwrap().take();
            if let Some(payload) = payload {
                std::panic::resume_unwind(payload);
            }
            self.progress
        }
        fn telemetry(&self) -> cna_seats::driver::SessionTelemetry {
            let payload = self.telemetry_panic.lock().unwrap().take();
            if let Some(payload) = payload {
                std::panic::resume_unwind(payload);
            }
            Default::default()
        }
        fn take_captured_outcome(
            &mut self,
            original: &cna_seats::operation::Operation,
        ) -> Result<Option<cna_seats::driver::TurnOutcome>, DriverError> {
            self.capture_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.captured.is_some() {
                assert_eq!(self.expected_capture_id.as_deref(), Some(original.id()));
            }
            // Synthetic accounting fixture only. Claude's production method independently
            // checks both original instance associations; this is not that native witness.
            if let Some(payload) = self.capture_panic.get_mut().unwrap().take() {
                std::panic::resume_unwind(payload);
            }
            if let Some(error) = self.capture_error.take() {
                return Err(error);
            }
            Ok(self.captured.take())
        }
        async fn start(
            &mut self,
            _: Option<&str>,
        ) -> Result<cna_seats::driver::SessionInfo, DriverError> {
            unreachable!()
        }
        async fn run_turn(
            &mut self,
            _: &str,
            limit: Duration,
        ) -> Result<cna_seats::driver::TurnOutcome, DriverError> {
            assert!(!limit.is_zero());
            self.entered = true;
            let _guard = CancelGuard(self.cancelled.clone());
            if let Some(payload) = self.turn_panic.get_mut().unwrap().take() {
                std::panic::resume_unwind(payload);
            }
            if self.hang {
                std::future::pending::<()>().await;
            }
            Err(self.error.take().unwrap())
        }
        fn session_id(&self) -> Option<String> {
            None
        }
        fn is_alive(&mut self) -> bool {
            false
        }
        async fn stop(&mut self) -> Result<(), DriverError> {
            if self.cleanup_ack {
                Ok(())
            } else {
                Err(DriverError::Host(cna_seats::host::HostRefusal::Ownership(
                    "synthetic original cleanup remains unresolved".into(),
                )))
            }
        }
    }
    fn timer_driver(error: Option<DriverError>, hang: bool) -> TimerDriver {
        TimerDriver {
            error,
            hang,
            progress: None,
            entered: false,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            captured: None,
            expected_capture_id: None,
            cleanup_ack: true,
            capture_error: None,
            capture_panic: std::sync::Mutex::new(None),
            capture_calls: std::sync::atomic::AtomicUsize::new(0),
            progress_panic: std::sync::Mutex::new(None),
            progress_calls: std::sync::atomic::AtomicUsize::new(0),
            telemetry_panic: std::sync::Mutex::new(None),
            turn_panic: std::sync::Mutex::new(None),
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. No native child, journal or archive I/O.
    // Cancellation and driver unwind leave no outcome, but cannot dispatch twice.
    #[tokio::test]
    #[cfg(panic = "unwind")]
    async fn journal_empty_result_after_dispatch_cannot_reopen_original_attempt() {
        use std::{
            future::Future,
            panic::{AssertUnwindSafe, catch_unwind},
            task::{Context, Poll, Waker},
        };
        struct Opaque(std::cell::Cell<usize>);
        for case in 0..3 {
            let start = Instant::now();
            let original = cna_seats::operation::Operation::new(
                format!("source-dispatch-{case}"),
                7,
                cna_seats::operation::Allowance::required(Some(start + Duration::from_secs(30)))
                    .unwrap(),
            )
            .unwrap();
            let deadline = original.allowance().deadline();
            let mut owner =
                crate::PendingJournalTurn::prepare(original.clone(), start, 1000).unwrap();
            let text = String::from("separate original captured body");
            let pointer = text.as_ptr();
            owner.captured_outcome = Some(cna_seats::driver::TurnOutcome {
                ok: true,
                error: None,
                text: Some(text),
                usage: Default::default(),
                quota: Vec::new(),
            });
            let mut driver = timer_driver(None, true);
            let bytes = original.captured_bytes();
            if case == 2 {
                let unpolled = supervise_journal_turn(
                    &mut owner,
                    &mut driver,
                    "unpolled",
                    1000,
                    start,
                    deadline,
                );
                drop(unpolled);
                assert!(!owner.supervisor_started && !driver.entered);
                assert!(!driver.cancelled.load(std::sync::atomic::Ordering::SeqCst));
            }
            let payload = Box::new(Opaque(std::cell::Cell::new(73)));
            let payload_pointer = std::ptr::from_ref(payload.as_ref());
            if case == 1 {
                *driver.turn_panic.get_mut().unwrap() = Some(payload);
            } else {
                drop(payload);
            }
            let mut context = Context::from_waker(Waker::noop());
            let result = catch_unwind(AssertUnwindSafe(|| {
                let mut borrowed = std::pin::pin!(supervise_journal_turn(
                    &mut owner,
                    &mut driver,
                    "first attempt",
                    1000,
                    start,
                    deadline,
                ));
                assert!(matches!(
                    borrowed.as_mut().poll(&mut context),
                    Poll::Pending
                ));
            }));
            match (case, result) {
                (1, Err(payload)) => {
                    let opaque = payload.downcast::<Opaque>().ok().unwrap();
                    assert_eq!(std::ptr::from_ref(opaque.as_ref()), payload_pointer);
                    assert_eq!(opaque.0.get(), 73);
                }
                (0 | 2, Ok(())) => {}
                _ => panic!("original dispatch control did not match"),
            }
            assert!(owner.supervisor_started && driver.entered);
            assert!(driver.cancelled.load(std::sync::atomic::Ordering::SeqCst));
            assert!(owner.outcome.is_none() && !owner.supervisor_observed);
            // Reset ONLY the inert fixture sentinel to detect any second entry.
            // Neither the production owner nor any native start evidence is cleared.
            driver.entered = false;
            for _ in 0..2 {
                let mut duplicate = std::pin::pin!(supervise_journal_turn(
                    &mut owner,
                    &mut driver,
                    "must refuse",
                    1000,
                    start,
                    deadline,
                ));
                assert!(matches!(
                    duplicate.as_mut().poll(&mut context),
                    Poll::Ready(Err(_))
                ));
            }
            assert!(!driver.entered);
            assert!(owner.supervisor_started && owner.outcome.is_none());
            assert!(!owner.supervisor_observed);
            assert_eq!(
                owner
                    .captured_outcome
                    .as_ref()
                    .unwrap()
                    .text
                    .as_ref()
                    .unwrap()
                    .as_ptr(),
                pointer
            );
            assert!(original.same_instance(&owner.operation));
            assert_eq!(original.captured_bytes(), bytes);
            assert_eq!(original.allowance().deadline(), deadline);
            assert_eq!(
                original.start_evidence(),
                cna_seats::operation::StartEvidence::NotStarted
            );
            // Local dispatch ownership is NOT native start/absence, cleanup/join,
            // journal completion, provider membership or healthy settlement proof.
        }
    }

    // SOURCE ONLY/UNRUN. Inert callback unwind, no child, archive or journal I/O.
    #[tokio::test]
    #[cfg(panic = "unwind")]
    async fn journal_installs_returned_result_before_telemetry_unwind() {
        use std::{
            future::Future,
            panic::{AssertUnwindSafe, catch_unwind},
            task::{Context, Poll, Waker},
        };
        struct Opaque(std::cell::Cell<usize>);
        for case in 0..3 {
            let start = Instant::now();
            let original = cna_seats::operation::Operation::new(
                format!("source-telemetry-{case}"),
                7,
                cna_seats::operation::Allowance::required(Some(start + Duration::from_secs(30)))
                    .unwrap(),
            )
            .unwrap();
            let mut owner =
                crate::PendingJournalTurn::prepare(original.clone(), start, 1000).unwrap();
            let captured_text = String::from("separate original captured evidence");
            let captured_pointer = captured_text.as_ptr();
            owner.captured_outcome = Some(cna_seats::driver::TurnOutcome {
                ok: true,
                error: None,
                text: Some(captured_text),
                usage: Default::default(),
                quota: Vec::new(),
            });
            let returned_text = String::from("original returned body");
            let returned_pointer = returned_text.as_ptr();
            let mut returned = Some(cna_seats::driver::TurnOutcome {
                ok: true,
                error: None,
                text: Some(returned_text),
                usage: Default::default(),
                quota: Vec::new(),
            });
            let error = String::from("original returned error");
            let error_pointer = error.as_ptr();
            let mut driver = timer_driver(Some(DriverError::Died(error)), false);
            let payload = Box::new(Opaque(std::cell::Cell::new(73)));
            let payload_pointer = std::ptr::from_ref(payload.as_ref());
            *driver.telemetry_panic.get_mut().unwrap() = Some(payload);
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            let mut context = Context::from_waker(Waker::noop());
            let unwind = catch_unwind(AssertUnwindSafe(|| match case {
                0 => {
                    let mut future = std::pin::pin!(supervise_journal_turn(
                        &mut owner,
                        &mut driver,
                        "inert",
                        1000,
                        start,
                        deadline,
                    ));
                    let _ = future.as_mut().poll(&mut context);
                }
                1 => {
                    let _ = owner.finish_supervisor(Ok(returned.take().unwrap()), &driver);
                }
                _ => {
                    let _ = owner.interrupt_supervisor(JournalInterruption::WallTimeout, &driver);
                }
            }));
            let opaque = match unwind {
                Err(payload) => payload.downcast::<Opaque>().ok().unwrap(),
                Ok(()) => panic!("telemetry callback never unwound"),
            };
            assert_eq!(std::ptr::from_ref(opaque.as_ref()), payload_pointer);
            assert_eq!(opaque.0.get(), 73);
            match owner.outcome.as_ref().unwrap() {
                Err(DriverError::Died(text)) if case == 0 => {
                    assert_eq!(text.as_ptr(), error_pointer);
                    assert!(driver.entered);
                }
                Ok(outcome) if case == 1 => {
                    assert_eq!(outcome.text.as_ref().unwrap().as_ptr(), returned_pointer);
                }
                Err(DriverError::Timeout) if case == 2 => {}
                _ => panic!("first original supervisor result was lost or replaced"),
            }
            assert_eq!(
                owner
                    .captured_outcome
                    .as_ref()
                    .unwrap()
                    .text
                    .as_ref()
                    .unwrap()
                    .as_ptr(),
                captured_pointer,
            );
            owner
                .interrupt_supervisor(JournalInterruption::BorrowCanceled, &driver)
                .unwrap();
            assert!(!owner.supervisor_observed);
            {
                let mut duplicate = std::pin::pin!(supervise_journal_turn(
                    &mut owner,
                    &mut driver,
                    "never dispatched",
                    1000,
                    start,
                    deadline,
                ));
                assert!(matches!(
                    duplicate.as_mut().poll(&mut context),
                    Poll::Ready(Err(_))
                ));
            }
            assert!(original.same_instance(&owner.operation));
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
            // No commit, physical cleanup, usage acceptance or healthy inference.
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Actual outer timer against an inert pending
    // future; neither its Drop guard nor this result witnesses native joins.
    #[tokio::test]
    #[cfg(panic = "unwind")]
    async fn journal_retains_timer_cause_before_progress_or_telemetry_unwind() {
        use cna_seats::driver::{PhaseMeasurement, TimeoutKind, TurnPhase, TurnProgress};
        use futures_util::FutureExt;
        use std::{panic::AssertUnwindSafe, sync::atomic::Ordering};
        struct Opaque(std::cell::Cell<usize>);
        for case in 0..3 {
            let start = Instant::now();
            let original = cna_seats::operation::Operation::new(
                format!("source-timeout-callback-{case}"),
                7,
                cna_seats::operation::Allowance::required(Some(start + Duration::from_secs(30)))
                    .unwrap(),
            )
            .unwrap();
            let deadline = original.allowance().deadline();
            let mut owner = crate::PendingJournalTurn::prepare(original.clone(), start, 1).unwrap();
            let captured = String::from("separate timer captured evidence");
            let captured_pointer = captured.as_ptr();
            owner.captured_outcome = Some(cna_seats::driver::TurnOutcome {
                ok: true,
                error: None,
                text: Some(captured),
                usage: Default::default(),
                quota: Vec::new(),
            });
            let mut driver = timer_driver(None, true);
            driver.progress = Some(TurnProgress {
                turn_started_at: start,
                phase_started_at: start,
                phase: TurnPhase::Submit,
            });
            let payload = Box::new(Opaque(std::cell::Cell::new(73)));
            let payload_pointer = std::ptr::from_ref(payload.as_ref());
            match case {
                0 => *driver.progress_panic.get_mut().unwrap() = Some(payload),
                1 => *driver.telemetry_panic.get_mut().unwrap() = Some(payload),
                _ => drop(payload),
            }
            let charged = original.captured_bytes();
            let returned = AssertUnwindSafe(supervise_journal_turn(
                &mut owner,
                &mut driver,
                "inert",
                1,
                start,
                deadline,
            ))
            .catch_unwind()
            .await;
            match (case, returned) {
                (0 | 1, Err(payload)) => {
                    let opaque = payload.downcast::<Opaque>().ok().unwrap();
                    assert_eq!(std::ptr::from_ref(opaque.as_ref()), payload_pointer);
                    assert_eq!(opaque.0.get(), 73);
                }
                (2, Ok(Ok(()))) => {}
                _ => panic!("timer callback outcome did not match its source control"),
            }
            let diagnostic = match owner.outcome.as_ref().unwrap() {
                Err(DriverError::TimeoutAt(diagnostic)) => diagnostic.clone(),
                _ => panic!("timer cause was lost or replaced"),
            };
            assert_eq!(diagnostic.kind, TimeoutKind::TurnReservation);
            assert_eq!(diagnostic.observed_limit_ms, Some(1));
            assert!(diagnostic.observed_elapsed_ms.is_some());
            assert_eq!(
                diagnostic.phase_measurement,
                if case == 0 {
                    PhaseMeasurement::Unavailable
                } else {
                    PhaseMeasurement::Available
                }
            );
            assert_eq!(
                diagnostic.phase,
                if case == 0 {
                    TurnPhase::Unknown
                } else {
                    TurnPhase::Submit
                }
            );
            assert_eq!(owner.supervisor_observed, case == 2);
            assert!(driver.entered && driver.cancelled.load(Ordering::SeqCst));
            assert_eq!(driver.progress_calls.load(Ordering::SeqCst), 1);
            // Preserve the timer and observation status, without callback retry
            // or replacement by this later cancellation classification.
            owner
                .interrupt_supervisor(JournalInterruption::BorrowCanceled, &driver)
                .unwrap();
            assert_eq!(owner.supervisor_observed, case == 2);
            assert!(
                matches!(owner.outcome.as_ref(), Some(Err(DriverError::TimeoutAt(saved))) if saved == &diagnostic)
            );
            assert!(
                supervise_journal_turn(
                    &mut owner,
                    &mut driver,
                    "never dispatched",
                    1,
                    start,
                    deadline,
                )
                .await
                .is_err()
            );
            assert_eq!(driver.progress_calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                owner
                    .captured_outcome
                    .as_ref()
                    .unwrap()
                    .text
                    .as_ref()
                    .unwrap()
                    .as_ptr(),
                captured_pointer
            );
            assert!(original.same_instance(&owner.operation));
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
            // No journal/accounting commit, actor call, child, archive I/O or
            // healthy inference. The private facade/full-fit assembly is separate.
        }
    }

    // SOURCE ONLY/UNRUN. Future local SQLite fixture, not native/join evidence.
    #[tokio::test]
    #[cfg(panic = "unwind")]
    async fn retained_result_without_observation_refuses_journal_and_usage_commit() {
        let (_root, journal, mut owner) = pending_journal_fixture();
        let seat = "axis.front_line".parse().unwrap();
        let before = serde_json::to_value(journal.snapshot().unwrap()).unwrap();
        let original = owner.operation.clone();
        let charged = original.captured_bytes();
        let deadline = original.allowance().deadline();
        let mut driver = timer_driver(None, false);
        *driver.telemetry_panic.get_mut().unwrap() = Some(Box::new(std::cell::Cell::new(73)));
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = owner.finish_supervisor(Err(DriverError::Timeout), &driver);
        }));
        assert!(unwind.is_err());
        assert!(matches!(owner.outcome, Some(Err(DriverError::Timeout))));
        assert!(!owner.supervisor_observed);
        // Even this synthetic independent join input cannot cure the missing
        // telemetry/measurement observation or authorize accounting inference.
        assert!(
            owner
                .commit(&journal, seat, true)
                .unwrap_err()
                .contains("observation remains unresolved")
        );
        assert_eq!(
            serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
            before
        );
        assert!(
            !journal
                .start_accounting_committed(seat, original.id())
                .unwrap()
        );
        assert!(journal.usage_outbox(seat).unwrap().is_empty());
        owner
            .interrupt_supervisor(JournalInterruption::BorrowCanceled, &driver)
            .unwrap();
        assert!(!owner.supervisor_observed);
        assert!(original.same_instance(&owner.operation));
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(original.allowance().deadline(), deadline);
        // Original result/root remains owned; no commit, refund or healthy state.
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Inert capture callbacks and future local
    // SQLite fixture. No native driver, archive custody or physical join proof.
    #[tokio::test]
    #[cfg(panic = "unwind")]
    async fn capture_refusal_or_callback_unwind_retains_journal_owner_and_first_cause() {
        use std::{
            panic::{AssertUnwindSafe, catch_unwind},
            sync::atomic::{AtomicUsize, Ordering},
        };
        struct Opaque(std::cell::Cell<usize>);
        struct FormatUnwind {
            payload: std::sync::Mutex<Option<Box<dyn std::any::Any + Send>>>,
            dropped: Arc<AtomicUsize>,
        }
        impl std::fmt::Debug for FormatUnwind {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("synthetic formatting unwind")
            }
        }
        impl std::fmt::Display for FormatUnwind {
            fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let payload = self.payload.lock().unwrap().take().unwrap();
                std::panic::resume_unwind(payload)
            }
        }
        impl std::error::Error for FormatUnwind {}
        impl Drop for FormatUnwind {
            fn drop(&mut self) {
                self.dropped.fetch_add(1, Ordering::SeqCst);
            }
        }
        for case in 0..3 {
            let (_root, journal, mut owner) = pending_journal_fixture();
            let seat = "axis.front_line".parse().unwrap();
            let original = owner.operation.clone();
            let mut driver = timer_driver(None, false);
            owner
                .finish_supervisor(Err(DriverError::Timeout), &driver)
                .unwrap();
            assert!(owner.supervisor_observed);
            let before = serde_json::to_value(journal.snapshot().unwrap()).unwrap();
            let captured = String::from("capture still belongs to inert driver");
            let captured_pointer = captured.as_ptr();
            driver.captured = Some(cna_seats::driver::TurnOutcome {
                ok: true,
                error: None,
                text: Some(captured),
                usage: Default::default(),
                quota: Vec::new(),
            });
            driver.expected_capture_id = Some(original.id().into());
            let error = String::from("first typed capture refusal");
            let error_pointer = error.as_ptr();
            let dropped = Arc::new(AtomicUsize::new(0));
            let mut payload_pointer = None;
            let mut source_pointer = None;
            match case {
                0 => driver.capture_error = Some(DriverError::Cli(error)),
                1 | 2 => {
                    let payload = Box::new(Opaque(std::cell::Cell::new(73)));
                    payload_pointer = Some(std::ptr::from_ref(payload.as_ref()));
                    if case == 1 {
                        *driver.capture_panic.get_mut().unwrap() = Some(payload);
                    } else {
                        let source = std::io::Error::other(FormatUnwind {
                            payload: std::sync::Mutex::new(Some(payload)),
                            dropped: dropped.clone(),
                        });
                        source_pointer = Some(std::ptr::from_ref(
                            source
                                .get_ref()
                                .unwrap()
                                .downcast_ref::<FormatUnwind>()
                                .unwrap(),
                        ));
                        driver.capture_error = Some(DriverError::Spawn {
                            cli: "source control",
                            source,
                        });
                    }
                }
                _ => unreachable!(),
            }
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            let attempt = catch_unwind(AssertUnwindSafe(|| owner.capture_from_driver(&mut driver)));
            match (case, attempt) {
                (0, Ok(Err(message))) => {
                    assert!(message.contains("captured outcome handoff failed"))
                }
                (1 | 2, Err(payload)) => {
                    let payload = payload.downcast::<Opaque>().ok().unwrap();
                    assert_eq!(Some(std::ptr::from_ref(payload.as_ref())), payload_pointer);
                    assert_eq!(payload.0.get(), 73);
                }
                _ => panic!("capture boundary did not preserve its planned refusal"),
            }
            match (case, owner.capture_refusal.as_ref()) {
                (0, Some(DriverError::Cli(error))) => assert_eq!(error.as_ptr(), error_pointer),
                (1, None) => assert!(owner.capture_in_service),
                (2, Some(DriverError::Spawn { source, .. })) => {
                    assert_eq!(
                        Some(std::ptr::from_ref(
                            source
                                .get_ref()
                                .unwrap()
                                .downcast_ref::<FormatUnwind>()
                                .unwrap()
                        )),
                        source_pointer,
                    );
                    assert_eq!(dropped.load(Ordering::SeqCst), 0);
                }
                _ => panic!("typed capture refusal was lost or replaced"),
            }
            assert_eq!(owner.capture_in_service, case == 1);
            assert!(owner.captured_outcome.is_none());
            assert!(matches!(
                owner.outcome.as_ref(),
                Some(Err(DriverError::Timeout))
            ));
            assert_eq!(
                driver
                    .captured
                    .as_ref()
                    .unwrap()
                    .text
                    .as_ref()
                    .unwrap()
                    .as_ptr(),
                captured_pointer,
            );
            assert!(owner.capture_from_driver(&mut driver).is_err());
            assert_eq!(driver.capture_calls.load(Ordering::SeqCst), 1);
            owner
                .interrupt_supervisor(JournalInterruption::BorrowCanceled, &driver)
                .unwrap();
            assert!(owner.supervisor_observed);
            assert!(matches!(
                owner.outcome.as_ref(),
                Some(Err(DriverError::Timeout))
            ));
            // This deliberately synthetic joined=true cannot override the
            // retained refusal/unreturned callback or mutate accounting storage.
            assert!(
                owner
                    .commit(&journal, seat, true)
                    .unwrap_err()
                    .contains("handoff remains unresolved")
            );
            assert_eq!(
                serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
                before
            );
            assert!(
                !journal
                    .start_accounting_committed(seat, original.id())
                    .unwrap()
            );
            assert!(journal.usage_outbox(seat).unwrap().is_empty());
            assert!(!driver.entered);
            assert!(original.same_instance(&owner.operation));
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
            if case == 2 {
                drop(owner); // Fixture disposal only, not production discard authority.
                assert_eq!(dropped.load(Ordering::SeqCst), 1);
            }
        }
    }

    fn pending_journal_fixture() -> (
        tempfile::TempDir,
        crate::journal::SessionJournal,
        crate::PendingJournalTurn,
    ) {
        use crate::config::{GameKind, LaunchConfig, SessionLimits};
        use cna_seats::operation::{Allowance, Operation};
        let seat: SeatId = "axis.front_line".parse().unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut config = LaunchConfig::resolve(
            GameKind::Cna,
            &[
                "axis.front_line=claude:haiku".into(),
                "*=scripted:legal_random".into(),
            ],
        )
        .unwrap();
        config.session = Some(SessionLimits::default());
        let journal = crate::journal::SessionJournal::create(
            root.path(),
            &uuid::Uuid::new_v4().to_string(),
            config,
            &std::collections::BTreeMap::from([(seat, 7)]),
        )
        .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let reserved = journal
            .reserve_decision_authorized(
                seat,
                "cancel-stage",
                1000,
                "original",
                1,
                crate::journal::StartAuthorization {
                    epoch: 7,
                    operation_id: &id,
                },
            )
            .unwrap();
        let start = Instant::now();
        let operation = Operation::new(
            id,
            7,
            Allowance::required(Some(start + Duration::from_millis(reserved))).unwrap(),
        )
        .unwrap();
        let owner = crate::PendingJournalTurn::prepare(operation, start, 1000).unwrap();
        (root, journal, owner)
    }

    #[tokio::test]
    async fn canceled_borrow_keeps_preinstalled_owner_captured_body_and_first_cause_until_actual_cleanup()
     {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let (_root, journal, owner) = pending_journal_fixture();
        let seat = "axis.front_line".parse().unwrap();
        let slot = tokio::sync::Mutex::new(Some(owner));
        let mut foreground = slot.try_lock().unwrap();
        let original_id = foreground.as_ref().unwrap().operation.id().to_owned();
        let deadline = foreground
            .as_ref()
            .unwrap()
            .operation
            .allowance()
            .deadline();
        let start = foreground.as_ref().unwrap().turn_started_at.unwrap();
        let mut driver = timer_driver(None, true);
        driver.cleanup_ack = false;
        driver.expected_capture_id = Some(original_id.clone());
        driver.captured = Some(cna_seats::driver::TurnOutcome {
            ok: true,
            error: None,
            text: Some("synthetic canonical captured evidence".into()),
            usage: cna_seats::driver::Usage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                cached_input_tokens: Some(0),
                cache_creation_tokens: Some(0),
                cost_usd: Some(0.01),
                ..Default::default()
            },
            quota: Vec::new(),
        });
        let body_pointer = driver
            .captured
            .as_ref()
            .unwrap()
            .text
            .as_ref()
            .unwrap()
            .as_ptr();
        // Explicit synthetic possible-start evidence, not a native dispatch or provider proof.
        foreground.as_ref().unwrap().operation.possible_start();
        let mut borrowed = Box::pin(supervise_journal_turn(
            foreground.as_mut().unwrap(),
            &mut driver,
            "inert",
            1000,
            start,
            deadline,
        ));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            borrowed.as_mut().poll(&mut context),
            Poll::Pending
        ));
        assert!(slot.try_lock().is_err()); // In-service borrow remains owned, never empty.
        drop(borrowed);
        drop(foreground);
        let mut foreground = slot.try_lock().unwrap();
        assert!(driver.cancelled.load(std::sync::atomic::Ordering::SeqCst));
        let retained = foreground.as_mut().unwrap();
        assert!(retained.outcome.is_none());
        assert_eq!(retained.operation.id(), original_id);
        assert_eq!(retained.operation.allowance().deadline(), deadline);
        let before = serde_json::to_value(journal.snapshot().unwrap()).unwrap();
        assert!(
            retained
                .commit(&journal, seat, true)
                .unwrap_err()
                .contains("still in service")
        );
        assert_eq!(
            serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
            before
        );
        retained
            .interrupt_supervisor(JournalInterruption::CampaignStopped, &driver)
            .unwrap();
        let cause = retained
            .outcome
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap_err()
            .to_string();
        retained.capture_from_driver(&mut driver).unwrap();
        assert!(driver.captured.is_none());
        assert_eq!(
            retained
                .captured_outcome
                .as_ref()
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .as_ptr(),
            body_pointer
        );
        retained
            .interrupt_supervisor(JournalInterruption::WallTimeout, &driver)
            .unwrap();
        assert_eq!(
            retained
                .outcome
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap_err()
                .to_string(),
            cause
        );
        assert!(driver.stop().await.is_err());
        assert!(
            retained
                .commit(&journal, seat, false)
                .unwrap_err()
                .contains("supervision remains unresolved")
        );
        assert!(journal.usage_outbox(seat).unwrap().is_empty());
        assert!(retained.captured_outcome.as_ref().unwrap().ok);
        driver.cleanup_ack = true; // Deliberate synthetic cleanup disposition, never a timer ACK.
        driver.stop().await.unwrap();
        retained.commit(&journal, seat, true).unwrap();
        let state = journal.snapshot().unwrap();
        assert_eq!(state.seats[&seat].stages["cancel-stage"].completed, 0);
        assert_eq!(state.seats[&seat].stages["cancel-stage"].output_tokens, 5);
        assert!(state.seats[&seat].start_operations[&original_id].accounting_committed);
        assert_eq!(
            retained
                .outcome
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap_err()
                .to_string(),
            cause
        );
        retained.commit(&journal, seat, true).unwrap();
        assert_eq!(
            serde_json::to_value(journal.snapshot().unwrap()).unwrap(),
            serde_json::to_value(state).unwrap()
        );
        assert!(foreground.is_some()); // Only actual committed handoff may consume this owner.
    }

    #[tokio::test]
    async fn unpolled_turn_borrow_preserves_pending_owner_without_inventing_start_or_completion() {
        let (_root, journal, mut owner) = pending_journal_fixture();
        let seat = "axis.front_line".parse().unwrap();
        let mut driver = timer_driver(None, true);
        let start = owner.turn_started_at.unwrap();
        let deadline = owner.operation.allowance().deadline();
        let bytes = owner.operation.captured_bytes();
        let unpolled =
            supervise_journal_turn(&mut owner, &mut driver, "inert", 1000, start, deadline);
        drop(unpolled);
        assert!(!driver.entered);
        assert!(owner.outcome.is_none());
        assert_eq!(
            owner.operation.start_evidence(),
            cna_seats::operation::StartEvidence::NotStarted
        );
        owner
            .interrupt_supervisor(JournalInterruption::BorrowCanceled, &driver)
            .unwrap();
        owner.capture_from_driver(&mut driver).unwrap();
        assert!(owner.captured_outcome.is_none());
        driver.stop().await.unwrap(); // Synthetic driver owns no child/readers.
        owner.commit(&journal, seat, true).unwrap();
        let state = journal.snapshot().unwrap();
        assert_eq!(state.seats[&seat].turns, 1); // Legacy public attempt was never refunded.
        assert_eq!(state.seats[&seat].stages["cancel-stage"].completed, 0);
        assert_eq!(
            state.seats[&seat].start_operations[owner.operation.id()].phase,
            crate::journal::StartPhase::SettledZero
        );
        assert_eq!(owner.operation.captured_bytes(), bytes);
        assert_eq!(owner.operation.allowance().deadline(), deadline);
    }

    #[tokio::test]
    async fn returned_inner_timeout_survives_later_outer_interruption_and_duplicate_finish() {
        let (_root, journal, mut owner) = pending_journal_fixture();
        let seat = "axis.front_line".parse().unwrap();
        let diagnostic =
            TimeoutDiagnostic::transcript_confirmation(Instant::now(), Duration::from_secs(30));
        let mut driver = timer_driver(Some(DriverError::TimeoutAt(diagnostic.clone())), false);
        let start = owner.turn_started_at.unwrap();
        let deadline = owner.operation.allowance().deadline();
        supervise_journal_turn(&mut owner, &mut driver, "inert", 1000, start, deadline)
            .await
            .unwrap();
        let original_elapsed = owner.supervisor_turn_elapsed_ms;
        owner
            .interrupt_supervisor(JournalInterruption::WallTimeout, &driver)
            .unwrap();
        assert_eq!(
            owner
                .outcome
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap_err()
                .timeout_diagnostic(),
            Some(diagnostic.clone())
        );
        assert!(
            owner
                .finish_supervisor(Err(DriverError::Timeout), &driver)
                .is_err()
        );
        assert_eq!(owner.supervisor_turn_elapsed_ms, original_elapsed);
        driver.stop().await.unwrap();
        owner.commit(&journal, seat, true).unwrap();
        let state = journal.snapshot().unwrap();
        assert_eq!(state.seats[&seat].timeout_events[0].diagnostic, diagnostic);
        assert_eq!(state.seats[&seat].stages["cancel-stage"].completed, 0);
    }

    #[tokio::test]
    async fn pending_whole_run_timeout_keeps_unknown_provenance_and_captured_usage_separate() {
        let (_root, journal, mut owner) = pending_journal_fixture();
        let seat = "axis.front_line".parse().unwrap();
        let deadline = owner.operation.allowance().deadline();
        let mut driver = timer_driver(None, true);
        driver.expected_capture_id = Some(owner.operation.id().into());
        driver.captured = Some(cna_seats::driver::TurnOutcome {
            ok: true,
            error: None,
            text: None,
            usage: cna_seats::driver::Usage {
                output_tokens: Some(17),
                ..Default::default()
            },
            quota: Vec::new(),
        });
        owner.operation.possible_start(); // Synthetic accounting evidence only.
        owner
            .interrupt_supervisor(JournalInterruption::WallTimeout, &driver)
            .unwrap();
        let diagnostic = owner
            .outcome
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap_err()
            .timeout_diagnostic()
            .unwrap();
        assert_eq!(diagnostic, TimeoutDiagnostic::legacy());
        assert!(diagnostic.observed_limit_ms.is_none()); // No guessed per-turn timer or phase.
        owner.capture_from_driver(&mut driver).unwrap();
        owner
            .interrupt_supervisor(JournalInterruption::BorrowCanceled, &driver)
            .unwrap();
        assert_eq!(
            owner
                .outcome
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap_err()
                .timeout_diagnostic(),
            Some(diagnostic.clone())
        );
        assert_eq!(owner.operation.allowance().deadline(), deadline);
        driver.stop().await.unwrap(); // Inert no-child cleanup disposition.
        owner.commit(&journal, seat, true).unwrap();
        let state = journal.snapshot().unwrap();
        let saved = &state.seats[&seat];
        assert_eq!(saved.timeout_events[0].diagnostic, diagnostic);
        assert_eq!(saved.stages["cancel-stage"].completed, 0);
        assert_eq!(saved.stages["cancel-stage"].output_tokens, 17);
        assert_eq!(saved.incomplete_token_turns, 1);
        assert_eq!(saved.incomplete_turns, 1);
        assert!(owner.captured_outcome.as_ref().unwrap().ok);
    }
    #[tokio::test]
    async fn actual_outer_timer_cancels_and_classifies_without_retry_or_stale_phase() {
        use cna_seats::driver::{PhaseMeasurement, TimeoutKind, TurnPhase, TurnProgress};
        let mut driver = timer_driver(None, true);
        let start = Instant::now();
        driver.progress = Some(TurnProgress {
            turn_started_at: start - Duration::from_secs(1),
            phase_started_at: start,
            phase: TurnPhase::Submit,
        });
        let result = bounded_turn(&mut driver, "inert", 5, start).await;
        let error = result.unwrap_err();
        assert!(driver.entered);
        assert!(driver.cancelled.load(std::sync::atomic::Ordering::SeqCst));
        let d = error.timeout_diagnostic().unwrap();
        assert_eq!(d.kind, TimeoutKind::TurnReservation);
        assert_eq!(d.observed_limit_ms, Some(5));
        assert!(d.observed_elapsed_ms.unwrap() >= 5);
        assert_eq!(
            (d.phase, d.phase_measurement),
            (TurnPhase::Unknown, PhaseMeasurement::RejectedStale)
        );
        assert!(!retryable_death(&error));
    }
    #[tokio::test]
    async fn inner_timeout_and_other_errors_are_not_reclassified_by_outer_timer() {
        let diagnostic =
            TimeoutDiagnostic::transcript_confirmation(Instant::now(), Duration::from_secs(30));
        let mut driver = timer_driver(Some(DriverError::TimeoutAt(diagnostic.clone())), false);
        let error = bounded_turn(&mut driver, "inert", 1000, Instant::now())
            .await
            .unwrap_err();
        assert_eq!(error.timeout_diagnostic(), Some(diagnostic));
        assert!(!retryable_death(&error));
        let mut driver = timer_driver(Some(DriverError::Timeout), false);
        let error = bounded_turn(&mut driver, "inert", 1000, Instant::now())
            .await
            .unwrap_err();
        assert!(matches!(error, DriverError::Timeout));
        assert_eq!(
            error.timeout_diagnostic(),
            Some(TimeoutDiagnostic::legacy())
        );
        for original in [
            DriverError::Cli("refused".into()),
            DriverError::Protocol("sink failed".into()),
        ] {
            let text = original.to_string();
            let mut driver = timer_driver(Some(original), false);
            let error = bounded_turn(&mut driver, "inert", 1000, Instant::now())
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), text);
            assert!(!error.is_timeout());
        }
    }
    #[test]
    fn completion_error_preserves_both_failures_without_exposing_diagnostic() {
        let error = DriverError::TimeoutAt(TimeoutDiagnostic::transcript_confirmation(
            Instant::now(),
            Duration::from_secs(30),
        ));
        let storage = crate::journal::CompletionError {
            timeout: error.timeout_diagnostic(),
            storage: "injected completion persistence failure".into(),
        };
        let displayed = completion_failure(Some(&error), &storage);
        assert!(displayed.contains(&DriverError::Timeout.to_string()));
        assert!(displayed.contains(&storage.storage));
        assert!(!displayed.contains("transcript_confirmation"));
        assert_eq!(storage.timeout, error.timeout_diagnostic());
    }
}
