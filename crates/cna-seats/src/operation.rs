//! Trusted operation ownership. These records never enter the spectator protocol.
//!
//! Encoded caps bound accepted bytes, not the allocator's RSS. A timeout closes admission;
//! only actual supervision settlement releases ownership. ACKs and drops release nothing.

use std::io::{self, Write};
use std::sync::{Arc, Mutex, Weak};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::time::Instant;

pub const FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const OPERATION_BYTES: usize = 16 * 1024 * 1024;
// Lead m9aff: private Host-only, one active call, explicit C-only release.
// Pinned POLICY reservation, not a std/private/process heap bound.
#[cfg(windows)]
pub(crate) const HOST_CANONICAL_BYTES: usize = 524_288;
pub const STDOUT_CHUNK_BYTES: usize = 4096;
pub(crate) const ARCHIVE_CHUNK_BYTES: usize = 4096;
pub const STDERR_BYTES: usize = 64 * 1024;
pub const STDERR_LINES: usize = 40;
pub const COMMAND_BYTES: usize = 64 * 1024;
pub const DELIVERY_BYTES: usize = 2 * 1024 * 1024;
pub const PREPARE_BYTES: usize = 64 * 1024;
pub const MIRROR_BYTES: usize = 4 * 1024 * 1024;
pub const MIRROR_ENTRIES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OperationError {
    #[error("no_allowance")]
    NoAllowance,
    #[error("operation allowance exhausted")]
    Expired,
    #[error("capture_overflow")]
    CaptureOverflow,
    #[error("operation already settled")]
    Settled,
    #[error("operation credit still owned")]
    CreditOwned,
}

/// A host-derived absolute deadline. Cloning never renews its allowance.
#[derive(Clone, Copy, Debug)]
pub struct Allowance(Instant);

impl Allowance {
    pub fn required(deadline: Option<Instant>) -> Result<Self, OperationError> {
        let value = Self(deadline.ok_or(OperationError::NoAllowance)?);
        value.check()?;
        Ok(value)
    }

    pub fn deadline(self) -> Instant {
        self.0
    }

    pub fn check(self) -> Result<(), OperationError> {
        if Instant::now() >= self.0 {
            Err(OperationError::Expired)
        } else {
            Ok(())
        }
    }

    /// Existing local ceilings may shorten, never extend, the parent allowance.
    pub fn capped(self, ceiling: std::time::Duration) -> Self {
        Self(self.0.min(Instant::now() + ceiling))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureOrigin {
    pub operation_id: String,
    /// Legacy admitted generation (controller epoch), not physical process identity.
    pub child_generation: u64,
    pub ordinal: u64,
    /// Trusted reader association. Old captures remain unknown; never infer this from
    /// a session id emitted by the CLI or the current active operation pointer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physical_child_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationFrame {
    MessageStart,
    Assistant,
    ResultUsage,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "classification", rename_all = "snake_case")]
pub enum StartEvidence {
    #[default]
    NotStarted,
    Possible,
    Confirmed {
        frame: ConfirmationFrame,
        ordinal: u64,
    },
}

struct State {
    claude_correlation: Option<crate::driver::claude_correlation::Correlation>,
    captured_bytes: usize,
    #[cfg(windows)]
    canonical_reserved: bool,
    original_growth: Option<OriginalGrowthBudget>,
    original_refusal: Option<OriginalRefusalPhase>,
    archive_chunk: Option<Vec<u8>>,
    ordinal: u64,
    overflow: bool,
    settled: bool,
    start: StartEvidence,
    command_owned: bool,
}

/// m5beab8311911: fixed metadata INSIDE the SAME original State, not a token,
/// counter, new service or permission to adopt the owned protocol. Only a
/// complete original constructor seeds this; legacy constructors keep None.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OriginalRefusalPhase {
    Accepting,
    Refused,
    Published,
}

#[allow(dead_code)] // Private SOURCE mechanics; no Operation/EventStorage adoption yet.
impl State {
    fn admitted_total(&self, bytes: usize) -> Option<usize> {
        self.captured_bytes
            .checked_add(bytes)
            .filter(|total| {
                total.checked_add(self.active_canonical_bytes()).is_some_and(|liability| {
                    liability <= OPERATION_BYTES && self.original_growth.as_ref()
                        .is_none_or(|budget| budget.admits(liability))
                })
            })
    }

    fn active_canonical_bytes(&self) -> usize {
        #[cfg(windows)]
        if self.canonical_reserved { return HOST_CANONICAL_BYTES; }
        0
    }

    #[cfg(all(test, windows))]
    fn reserve_host_canonical(&mut self) -> Result<(), OperationError> {
        if self.overflow { return Err(OperationError::CaptureOverflow); }
        if self.settled { return Err(OperationError::Settled); }
        if self.canonical_reserved { return Err(OperationError::CreditOwned); }
        // Active C is currently zero: admit this ONE C against the same
        // cumulative prefix/budget without adding it to that ordinary counter.
        if self.admitted_total(HOST_CANONICAL_BYTES).is_none() {
            if self.original_growth.is_some() || self.original_refusal.is_some() {
                self.refuse_original_unpublished()?;
            } else {
                self.overflow = true;
            }
            return Err(OperationError::CaptureOverflow);
        }
        self.canonical_reserved = true;
        Ok(())
    }

    fn original_phase(&self) -> Result<OriginalRefusalPhase, OperationError> {
        let budget = self
            .original_growth
            .as_ref()
            .ok_or(OperationError::CreditOwned)?;
        let phase = self.original_refusal.ok_or(OperationError::CreditOwned)?;
        if self.captured_bytes > OPERATION_BYTES || !budget.admits(self.captured_bytes) {
            return Err(OperationError::CreditOwned);
        }
        match (phase, self.overflow) {
            (OriginalRefusalPhase::Accepting, false)
            | (OriginalRefusalPhase::Refused | OriginalRefusalPhase::Published, true) => Ok(phase),
            _ => Err(OperationError::CreditOwned), // No repair or inferred publication.
        }
    }

    /// No allowance check or notification belongs to this State-only primitive.
    /// A future strict caller must perform its original deadline check first.
    fn charge_original_unpublished(&mut self, bytes: usize) -> Result<(), OperationError> {
        if self.original_phase()? != OriginalRefusalPhase::Accepting {
            return Err(OperationError::CaptureOverflow);
        }
        if self.settled {
            return Err(OperationError::Settled);
        }
        match self.admitted_total(bytes) {
            Some(total) => {
                self.captured_bytes = total; // SAME counter; no mirror or later seed.
                Ok(())
            }
            None => {
                self.refuse_original_unpublished()?;
                Err(OperationError::CaptureOverflow)
            }
        }
    }

    fn refuse_original_unpublished(&mut self) -> Result<(), OperationError> {
        let phase = self.original_phase()?;
        self.overflow = true; // Existing admission checks refuse IMMEDIATELY.
        if phase == OriginalRefusalPhase::Accepting {
            self.original_refusal = Some(OriginalRefusalPhase::Refused);
        }
        Ok(()) // Never touches Notify, buffers, credits or another original.
    }

    /// The future owner MUST establish actual raw cause/retained Result FIRST
    /// MOVE and release every capture/stats/failure lock BEFORE calling this.
    /// This private primitive cannot itself prove those external obligations.
    /// true is an in-call observation to notify AFTER the State lock drops,
    /// never an exported token or a second permission to repeat publication.
    fn publish_original_refusal(&mut self) -> Result<bool, OperationError> {
        match self.original_phase()? {
            OriginalRefusalPhase::Accepting => Err(OperationError::CreditOwned),
            OriginalRefusalPhase::Refused => {
                self.original_refusal = Some(OriginalRefusalPhase::Published);
                Ok(true)
            }
            OriginalRefusalPhase::Published => Ok(false),
        }
    }

    fn original_supervision_published(&self) -> Result<bool, OperationError> {
        Ok(self.original_phase()? == OriginalRefusalPhase::Published)
    }
}

// UNCOMPILED/UNRUN SOURCE controls. This synthetic State is not a sealed Fit
// producer and cannot create an Operation. No task/Notify/IO is exercised.
#[cfg(test)]
mod original_refusal_state_source_controls {
    use super::*;

    fn state() -> State {
        // Actual types for a local arithmetic fixture, NOT complete original F.
        let layout = std::alloc::Layout::new::<(State, Operation, Notify)>();
        let budget = OriginalGrowthBudget::from_complete_layout(layout).unwrap();
        State {
            claude_correlation: None,
            captured_bytes: budget.fixed_bytes,
            #[cfg(windows)] canonical_reserved: false,
            original_growth: Some(budget),
            original_refusal: Some(OriginalRefusalPhase::Accepting),
            archive_chunk: None,
            ordinal: 0,
            overflow: false,
            settled: false,
            start: StartEvidence::NotStarted,
            command_owned: false,
        }
    }

    #[test]
    fn exact_same_growth_counter_then_one_byte_refuses_without_publication_or_refund() {
        let mut state = state();
        let fixed = state.captured_bytes;
        let growth = state.original_growth.as_ref().unwrap().growth_limit;
        assert_eq!(growth, OPERATION_BYTES - fixed);
        assert_eq!(state.charge_original_unpublished(growth), Ok(()));
        assert_eq!(state.captured_bytes, OPERATION_BYTES);
        assert_eq!(state.original_supervision_published(), Ok(false));
        assert_eq!(
            state.charge_original_unpublished(1),
            Err(OperationError::CaptureOverflow)
        );
        assert!(state.overflow); // Public admission check sees the SAME sticky flag.
        assert_eq!(state.original_phase(), Ok(OriginalRefusalPhase::Refused));
        assert_eq!(state.original_supervision_published(), Ok(false));
        assert_eq!(
            state.charge_original_unpublished(0),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(state.captured_bytes, OPERATION_BYTES);
        assert_eq!(state.original_growth.as_ref().unwrap().fixed_bytes, fixed);
        assert_eq!(state.original_growth.as_ref().unwrap().growth_limit, growth);
        assert_eq!(state.ordinal, 0);
        assert!(!state.command_owned);
    }

    #[cfg(windows)]
    #[test]
    fn canonical_class_is_inside_the_same_original_growth_ceiling() {
        let mut original = state();
        let prefix = original.captured_bytes;
        assert_eq!(original.reserve_host_canonical(), Ok(()));
        assert_eq!(original.captured_bytes, prefix);
        assert_eq!(original.reserve_host_canonical(), Err(OperationError::CreditOwned));
        assert_eq!(original.charge_original_unpublished(OPERATION_BYTES - HOST_CANONICAL_BYTES - prefix), Ok(()));
        assert_eq!(original.captured_bytes + original.active_canonical_bytes(), OPERATION_BYTES);
        assert_eq!(original.charge_original_unpublished(1), Err(OperationError::CaptureOverflow));
        assert!(original.canonical_reserved);
        assert_eq!(original.original_phase(), Ok(OriginalRefusalPhase::Refused));
        assert_eq!(original.captured_bytes, OPERATION_BYTES - HOST_CANONICAL_BYTES);
        let mut insufficient = state();
        let bytes = OPERATION_BYTES - HOST_CANONICAL_BYTES + 1 - insufficient.captured_bytes;
        insufficient.charge_original_unpublished(bytes).unwrap();
        let prefix = insufficient.captured_bytes;
        assert_eq!(insufficient.reserve_host_canonical(), Err(OperationError::CaptureOverflow));
        assert!(!insufficient.canonical_reserved);
        assert_eq!(insufficient.captured_bytes, prefix);
        assert_eq!(insufficient.original_phase(), Ok(OriginalRefusalPhase::Refused));
    }

    #[test]
    fn publication_is_separate_monotone_and_cannot_reopen_admission() {
        let mut state = state();
        let charged = state.captured_bytes;
        assert_eq!(
            state.publish_original_refusal(),
            Err(OperationError::CreditOwned)
        );
        assert_eq!(state.refuse_original_unpublished(), Ok(()));
        assert_eq!(state.original_supervision_published(), Ok(false));
        // A Copy fixture represents first MOVE only. Actual raw cause, locks,
        // Notify and real waiters remain mandatory WHOLE-joint controls.
        let retained: Option<Result<(), OperationError>> =
            Some(Err(OperationError::CaptureOverflow));
        assert!(matches!(
            retained,
            Some(Err(OperationError::CaptureOverflow))
        ));
        state.settled = true; // Publication must not require fresh admission/credit.
        assert_eq!(state.publish_original_refusal(), Ok(true));
        assert_eq!(state.original_supervision_published(), Ok(true));
        assert_eq!(state.publish_original_refusal(), Ok(false));
        assert_eq!(state.refuse_original_unpublished(), Ok(()));
        assert_eq!(state.original_phase(), Ok(OriginalRefusalPhase::Published));
        assert_eq!(
            state.charge_original_unpublished(0),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(state.captured_bytes, charged);
        assert_eq!(state.original_supervision_published(), Ok(true));
    }

    #[test]
    fn legacy_or_inconsistent_metadata_cannot_enter_or_repair_owned_protocol() {
        for case in 0..4 {
            let mut state = state();
            match case {
                0 => {
                    state.original_growth = None;
                    state.original_refusal = None;
                }
                1 => state.original_refusal = None,
                2 => state.original_growth = None,
                3 => state.overflow = true, // Public legacy route is NOT owned adoption.
                _ => unreachable!(),
            }
            let charged = state.captured_bytes;
            let phase = state.original_refusal;
            let overflow = state.overflow;
            assert_eq!(
                state.charge_original_unpublished(1),
                Err(OperationError::CreditOwned)
            );
            assert_eq!(
                state.refuse_original_unpublished(),
                Err(OperationError::CreditOwned)
            );
            assert_eq!(
                state.publish_original_refusal(),
                Err(OperationError::CreditOwned)
            );
            assert_eq!(
                state.original_supervision_published(),
                Err(OperationError::CreditOwned)
            );
            assert_eq!(state.original_refusal, phase);
            assert_eq!(state.overflow, overflow);
            assert_eq!(state.captured_bytes, charged);
        }
    }

    #[test]
    fn arithmetic_overflow_and_settlement_preserve_the_admitted_prefix_counter() {
        let mut state = state();
        state.charge_original_unpublished(17).unwrap();
        let prefix = state.captured_bytes;
        assert_eq!(
            state.charge_original_unpublished(usize::MAX),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(state.captured_bytes, prefix);
        assert_eq!(state.original_supervision_published(), Ok(false));
        let mut settled = self::state();
        let initial = settled.captured_bytes;
        settled.settled = true;
        assert_eq!(
            settled.charge_original_unpublished(1),
            Err(OperationError::Settled)
        );
        assert_eq!(settled.captured_bytes, initial);
        assert!(!settled.overflow);
        assert_eq!(
            settled.original_phase(),
            Ok(OriginalRefusalPhase::Accepting)
        );
    }
}

/// Immutable ORIGINAL seed, not another meter. captured_bytes remains the ONE
/// cumulative counter; growth consumed/peak is captured_bytes - fixed_bytes.
/// Drops, ACKs, cleanup and deadline changes cannot replenish this ceiling.
pub(crate) struct OriginalGrowthBudget {
    pub(crate) fixed_bytes: usize,
    pub(crate) growth_limit: usize,
}

impl OriginalGrowthBudget {
    pub(crate) fn from_complete_layout(layout: std::alloc::Layout) -> Result<Self, OperationError> {
        let fixed_bytes = layout.pad_to_align().size();
        let growth_limit = OPERATION_BYTES
            .checked_sub(fixed_bytes)
            .ok_or(OperationError::CaptureOverflow)?;
        Ok(Self {
            fixed_bytes,
            growth_limit,
        })
    }

    pub(crate) fn admits(&self, total: usize) -> bool {
        total
            .checked_sub(self.fixed_bytes)
            .is_some_and(|used| used <= self.growth_limit)
    }
}

/// Read-only cumulative values for the later separately authorized STRICT
/// synthetic startup/turn report. No measurement or provider call runs here.
/// These are ordinary cumulative liabilities only. The narrow transient C
/// class is checked separately; this is not a whole-process peak receipt.
#[allow(dead_code)]
pub(crate) struct OriginalGrowthUsage {
    pub(crate) fixed_bytes: usize,
    pub(crate) growth_limit: usize,
    pub(crate) consumed_and_peak: usize,
}

/// Weak identity retains the allocation's identity without making a cycle from the
/// creation-reserved reader handshake back into its canonical Operation.
#[derive(Clone)]
pub(crate) struct ReaderIdentity {
    state: Weak<Mutex<State>>,
    generation: u64,
}

impl ReaderIdentity {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn same(&self, other: &Self) -> bool {
        self.generation == other.generation && Weak::ptr_eq(&self.state, &other.state)
    }
}

/// The same handle owns reader/capture liability and the canonical receipt through final
/// settlement. There is no automatic release on clone/drop, dispatch ACK or deadline expiry.
#[derive(Clone)]
pub struct Operation {
    id: Arc<str>,
    child_generation: u64,
    allowance: Allowance,
    state: Arc<Mutex<State>>,
    overflowed: Arc<Notify>,
    archive: Option<Arc<crate::failed_evidence::ArchivePlan>>,
    reader_handoff: Arc<crate::driver::reader_handoff::Gate>,
}

/// Linear private Host call reservation on the SAME original allocation. No
/// Clone/Drop refund or public byte/refund API; losing this owner leaves C held.
/// A completed call must first retain its whole Result before explicit release.
#[cfg(all(test, windows))]
#[allow(dead_code)] // Staged canonical Host call source, not production admission.
pub(crate) struct HostCanonicalCharge {
    original: Operation,
    active: bool,
}

#[cfg(all(test, windows))]
mod canonical_reservation_source_controls {
    use super::*;
    use std::time::Duration;

    fn original() -> Operation {
        Operation::new(String::from("host-canonical-source"), 1,
            Allowance::required(Some(Instant::now() + Duration::from_secs(30))).unwrap()).unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canonical_release_keeps_normal_prefix_overflow_and_original_result() {
        tokio::runtime::Handle::current().spawn(async {
            let operation = original();
            let alias = operation.clone();
            let prefix = operation.captured_bytes();
            let mut credit = operation.reserve_host_canonical().unwrap();
            assert!(credit.original.same_instance(&operation));
            assert_eq!(alias.captured_bytes(), prefix);
            assert!(matches!(alias.reserve_host_canonical(), Err(OperationError::CreditOwned)));
            operation.charge(OPERATION_BYTES - HOST_CANONICAL_BYTES - prefix).unwrap();
            assert_eq!(operation.charge(1), Err(OperationError::CaptureOverflow));
            let charged = operation.captured_bytes();
            // Explicit returned-library-error fixture; no filesystem or actual
            // canonicalize call. Whole Result is stored BEFORE C-only release.
            let returned: Option<Result<std::path::PathBuf, std::io::Error>> =
                Some(Err(std::io::Error::from_raw_os_error(5)));
            let error = returned.as_ref().unwrap().as_ref().unwrap_err();
            let pointer = std::ptr::from_ref(error) as usize;
            assert_eq!(credit.release_after_retained_result(), Ok(()));
            assert_eq!(operation.captured_bytes(), charged);
            assert!(!operation.state.lock().unwrap().canonical_reserved);
            assert_eq!(operation.check(), Err(OperationError::CaptureOverflow));
            assert_eq!(credit.release_after_retained_result(), Err(OperationError::CreditOwned));
            assert!(matches!(alias.reserve_host_canonical(), Err(OperationError::CaptureOverflow)));
            assert_eq!(std::ptr::from_ref(returned.as_ref().unwrap().as_ref().unwrap_err()) as usize, pointer);
            assert_eq!(returned.as_ref().unwrap().as_ref().unwrap_err().raw_os_error(), Some(5));
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canonical_drop_never_refunds_and_expired_explicit_release_only_reuses_c() {
        tokio::runtime::Handle::current().spawn(async {
            let lost = original();
            let prefix = lost.captured_bytes();
            let credit = lost.reserve_host_canonical().unwrap();
            drop(credit); // Interrupted ownership keeps this original C held.
            assert!(lost.state.lock().unwrap().canonical_reserved);
            assert!(matches!(lost.reserve_host_canonical(), Err(OperationError::CreditOwned)));
            assert_eq!(lost.captured_bytes(), prefix);

            let operation = original();
            operation.charge(131_068).unwrap(); // Ordinary persistent-path fixture.
            let charged = operation.captured_bytes();
            let mut first = operation.reserve_host_canonical().unwrap();
            // Only the token's cloned allowance expires in this source fixture;
            // SAME meter is retained. Cleanup must not require new admission.
            first.original.allowance = Allowance(Instant::now().checked_sub(Duration::from_secs(1)).unwrap());
            let returned: Option<Result<(), std::io::Error>> = Some(Ok(()));
            assert!(returned.is_some());
            assert_eq!(first.release_after_retained_result(), Ok(()));
            assert_eq!(operation.captured_bytes(), charged);
            let mut second = operation.reserve_host_canonical().unwrap();
            assert!(second.original.same_instance(&operation));
            assert_eq!(first.release_after_retained_result(), Err(OperationError::CreditOwned));
            assert!(operation.state.lock().unwrap().canonical_reserved); // Old token cannot release new C.
            assert_eq!(second.release_after_retained_result(), Ok(()));
            assert_eq!(operation.captured_bytes(), charged);
        }).await.unwrap();
    }
}

#[cfg(all(test, windows))]
#[allow(dead_code)]
impl HostCanonicalCharge {
    pub(crate) fn release_after_retained_result(&mut self) -> Result<(), OperationError> {
        if !self.active { return Err(OperationError::CreditOwned); }
        let mut state = self.original.state.lock().expect("operation state");
        if !state.canonical_reserved { return Err(OperationError::CreditOwned); }
        // Cleanup remains possible after ordinary expiry/overflow/settlement.
        // No captured byte, overflow latch, phase or other ownership is reset.
        state.canonical_reserved = false;
        self.active = false;
        Ok(())
    }
}

/// Temporarily owns the SAME canonical scalar metadata. Not an evidence copy,
/// consume, custody ACK or effect lease. Drop restores on refusal/unwind; no
/// synchronous lock survives into a serializer or writer callback.
pub(crate) struct CorrelationArchiveLease<'a> {
    original: &'a Operation,
    value: Option<crate::driver::claude_correlation::Correlation>,
}

impl CorrelationArchiveLease<'_> {
    pub(crate) fn value(&self) -> &crate::driver::claude_correlation::Correlation {
        self.value.as_ref().expect("canonical correlation checkout")
    }
}

impl Drop for CorrelationArchiveLease<'_> {
    fn drop(&mut self) {
        let mut state = self.original.state.lock().expect("operation state");
        // No normal parser/preparation path can fill a checked-out slot.
        assert!(state.claude_correlation.is_none());
        state.claude_correlation = self.value.take();
    }
}

/// A borrowed cleanup workspace, not effect admission or evidence acceptance. The caller
/// must separately establish every original child/reader join before storage. Returning
/// this buffer releases no liability, command credit, permit or prompt ownership.
pub(crate) struct ArchiveChunkLease {
    operation: Operation,
    buffer: Option<Vec<u8>>,
}

impl ArchiveChunkLease {
    pub(crate) fn bytes_mut(&mut self) -> &mut [u8] {
        self.buffer
            .as_mut()
            .expect("owned archive chunk")
            .as_mut_slice()
    }
}

impl Drop for ArchiveChunkLease {
    fn drop(&mut self) {
        let mut state = self.operation.state.lock().expect("operation state");
        // Exclusive checkout guarantees the same allocation returns to its original owner.
        debug_assert!(state.archive_chunk.is_none());
        state.archive_chunk = self.buffer.take();
    }
}

fn prepare_archive_chunk(cap: usize) -> Result<(Vec<u8>, usize), OperationError> {
    // Count the retained slot and in-service lease before allocating. There is one payload
    // allocation, moved between them, and no replacement allocation after capture overflow.
    let fixed = std::mem::size_of::<Option<Vec<u8>>>()
        .checked_add(std::mem::size_of::<ArchiveChunkLease>())
        .ok_or(OperationError::CaptureOverflow)?;
    if ARCHIVE_CHUNK_BYTES
        .checked_add(fixed)
        .is_none_or(|n| n > cap)
    {
        return Err(OperationError::CaptureOverflow);
    }
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(ARCHIVE_CHUNK_BYTES)
        .map_err(|_| OperationError::CaptureOverflow)?;
    // Allocator capacity is measured before accepting the operation; an excessive actual
    // allocation refuses creation. This is retained-byte accounting, not an RSS guarantee.
    let bytes = buffer
        .capacity()
        .checked_add(fixed)
        .filter(|n| *n <= cap)
        .ok_or(OperationError::CaptureOverflow)?;
    buffer.resize(ARCHIVE_CHUNK_BYTES, 0);
    Ok((buffer, bytes))
}

impl Operation {
    pub fn new(
        id: String,
        child_generation: u64,
        allowance: Allowance,
    ) -> Result<Self, OperationError> {
        Self::create(id, child_generation, allowance, None)
    }

    pub(crate) fn new_with_archive(
        id: String,
        child_generation: u64,
        allowance: Allowance,
        archive: crate::failed_evidence::ArchivePlan,
    ) -> Result<Self, OperationError> {
        Self::create(id, child_generation, allowance, Some(archive))
    }

    fn create(
        id: String,
        child_generation: u64,
        allowance: Allowance,
        archive: Option<crate::failed_evidence::ArchivePlan>,
    ) -> Result<Self, OperationError> {
        allowance.check()?;
        let archive_bytes = archive
            .as_ref()
            .map_or(0, |plan| plan.reserved_bytes)
            .checked_add(std::mem::size_of::<
                Option<Arc<crate::failed_evidence::ArchivePlan>>,
            >())
            .and_then(|n| n.checked_add(crate::driver::reader_handoff::reserved_bytes()))
            .and_then(|n| n.checked_add(crate::driver::claude_correlation::reserved_bytes()))
            .ok_or(OperationError::CaptureOverflow)?;
        let cap = OPERATION_BYTES
            .checked_sub(archive_bytes)
            .ok_or(OperationError::CaptureOverflow)?;
        let (archive_chunk, captured_bytes) = prepare_archive_chunk(cap)?;
        let captured_bytes = captured_bytes
            .checked_add(archive_bytes)
            .ok_or(OperationError::CaptureOverflow)?;
        allowance.check()?;
        let state = Arc::new(Mutex::new(State {
            claude_correlation: Some(crate::driver::claude_correlation::Correlation::new()),
            captured_bytes,
            #[cfg(windows)] canonical_reserved: false,
            original_growth: None, // Legacy constructor/counter behavior is unchanged.
            original_refusal: None,
            archive_chunk: Some(archive_chunk),
            ordinal: 0,
            overflow: false,
            settled: false,
            start: StartEvidence::NotStarted,
            command_owned: false,
        }));
        let reader_handoff = Arc::new(crate::driver::reader_handoff::Gate::new(
            ReaderIdentity {
                state: Arc::downgrade(&state),
                generation: child_generation,
            },
            allowance.deadline(),
        ));
        Ok(Self {
            id: id.into(),
            child_generation,
            allowance,
            state,
            overflowed: Arc::new(Notify::new()),
            archive: archive.map(Arc::new),
            reader_handoff,
        })
    }

    /// Internal consumer of a COMPLETE trusted creation proof, never a byte
    /// input. The Fit currently has NO producer, so this supplies no positive
    /// creation/factory activation. All refusal checks precede its sole MOVE;
    /// a refused input stays in the original caller's pending slot untouched.
    #[allow(dead_code)]
    pub(crate) fn new_with_original_fit(
        pending: &mut Option<crate::failed_evidence::OriginalCreationFit>,
    ) -> Result<Self, OperationError> {
        let fit = pending.as_ref().ok_or(OperationError::CreditOwned)?;
        let parts = fit.parts();
        parts.allowance.check()?;
        let budget = OriginalGrowthBudget::from_complete_layout(parts.fixed)?;
        // Recheck necessary actual local storage; this is NOT a replacement
        // for the sealed proof's whole owner/caller/outer/library derivation.
        let actual_chunk = std::alloc::Layout::array::<u8>(parts.archive_chunk.capacity())
            .map_err(|_| OperationError::CaptureOverflow)?;
        let minimum = parts
            .archive
            .reserved_bytes
            .checked_add(crate::driver::reader_handoff::reserved_bytes())
            .and_then(|n| n.checked_add(crate::driver::claude_correlation::reserved_bytes()))
            .and_then(|n| {
                n.checked_add(
                    std::alloc::Layout::new::<(
                        Operation,
                        Mutex<State>,
                        Notify,
                        ArchiveChunkLease,
                        crate::failed_evidence::OriginalCreationFit,
                    )>()
                    .size(),
                )
            })
            .and_then(|n| n.checked_add(actual_chunk.size()))
            // Pending String and its immutable Arc<str> allocation overlap.
            .and_then(|n| n.checked_add(parts.id.capacity()))
            .and_then(|n| n.checked_add(parts.id.len()))
            .ok_or(OperationError::CaptureOverflow)?;
        if parts.archive_chunk.len() != ARCHIVE_CHUNK_BYTES
            || parts.archive.epoch != parts.generation
            || parts.id.len() != 36
            || uuid::Uuid::parse_str(&parts.id).is_err()
            || budget.fixed_bytes < minimum
        {
            return Err(OperationError::CaptureOverflow);
        }
        parts.allowance.check()?;
        // F and G are pinned above BEFORE State/Operation allocation. These
        // pending bodies were already retained/measured by the trusted fit;
        // do not allocate another archive chunk or generate another identity.
        let parts = pending
            .take()
            .expect("checked complete original fit")
            .into_parts();
        let fixed_bytes = budget.fixed_bytes;
        let state = Arc::new(Mutex::new(State {
            claude_correlation: Some(parts.correlation), // Original seed MOVE, no new UUID.
            captured_bytes: fixed_bytes, // Seed F ONCE; later charge consumes only G.
            #[cfg(windows)] canonical_reserved: false,
            original_growth: Some(budget),
            original_refusal: Some(OriginalRefusalPhase::Accepting),
            archive_chunk: Some(parts.archive_chunk),
            ordinal: 0,
            overflow: false,
            settled: false,
            start: StartEvidence::NotStarted,
            command_owned: false,
        }));
        let reader_handoff = Arc::new(crate::driver::reader_handoff::Gate::new(
            ReaderIdentity {
                state: Arc::downgrade(&state),
                generation: parts.generation,
            },
            parts.allowance.deadline(),
        ));
        Ok(Self {
            id: parts.id.into(),
            child_generation: parts.generation,
            allowance: parts.allowance,
            state,
            overflowed: Arc::new(Notify::new()),
            archive: Some(Arc::new(parts.archive)),
            reader_handoff,
        })
    }

    pub fn allowance(&self) -> Allowance {
        self.allowance
    }

    pub(crate) fn prepare_claude_input(&self) -> Result<[u8; 36], OperationError> {
        self.check()?;
        self.state
            .lock()
            .expect("operation state")
            .claude_correlation
            .as_mut()
            .ok_or(OperationError::CreditOwned)?
            .prepare()
            .map_err(|_| OperationError::CreditOwned)
    }

    pub(crate) fn validate_claude_frame(
        &self,
        value: &serde_json::Value,
    ) -> Result<crate::driver::claude_correlation::Frame, crate::driver::claude_correlation::Refusal>
    {
        self.state
            .lock()
            .expect("operation state")
            .claude_correlation
            .as_mut()
            .ok_or(crate::driver::claude_correlation::Refusal::Unsupported)?
            .validate(value)
    }

    pub(crate) fn refuse_claude_frame(&self) {
        if let Some(value) = self
            .state
            .lock()
            .expect("operation state")
            .claude_correlation
            .as_mut()
        {
            value.fail(crate::driver::claude_correlation::Refusal::Malformed);
        }
    }

    pub(crate) fn check_claude_terminal(
        &self,
    ) -> Result<(), crate::driver::claude_correlation::Refusal> {
        self.state
            .lock()
            .expect("operation state")
            .claude_correlation
            .as_ref()
            .ok_or(crate::driver::claude_correlation::Refusal::Unsupported)?
            .check_terminal()
    }

    /// Only canonical driver enumeration after actual original joins uses this.
    /// No live allowance check: overflow/expiry must retain inspectable metadata.
    /// Checked-out/nested access refuses; normal methods cannot mutate the lease.
    pub(crate) fn borrow_claude_correlation(
        &self,
        expected: &Operation,
    ) -> Result<CorrelationArchiveLease<'_>, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        if !self.same_instance(expected) || self.archive_plan().is_none() {
            return Err(ArchiveError::Identity);
        }
        let value = self
            .state
            .lock()
            .expect("operation state")
            .claude_correlation
            .take()
            .ok_or(ArchiveError::Identity)?;
        Ok(CorrelationArchiveLease {
            original: self,
            value: Some(value),
        })
    }

    pub(crate) fn archive_plan(&self) -> Option<&crate::failed_evidence::ArchivePlan> {
        self.archive.as_deref()
    }

    /// Uses creation-time storage even after expiry/overflow; it never opens effects or
    /// creates a new allowance. Text IDs cannot substitute for original instance identity.
    pub(crate) fn borrow_archive_chunk(
        &self,
        expected: &Self,
    ) -> Result<ArchiveChunkLease, OperationError> {
        if !self.same_instance(expected) {
            return Err(OperationError::CreditOwned);
        }
        let mut state = self.state.lock().expect("operation state");
        if state.settled {
            return Err(OperationError::Settled);
        }
        let buffer = state
            .archive_chunk
            .take()
            .ok_or(OperationError::CreditOwned)?;
        Ok(ArchiveChunkLease {
            operation: self.clone(),
            buffer: Some(buffer),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn check(&self) -> Result<(), OperationError> {
        let state = self.state.lock().expect("operation state");
        if state.overflow {
            return Err(OperationError::CaptureOverflow);
        }
        if state.settled {
            return Err(OperationError::Settled);
        }
        drop(state);
        self.allowance.check()
    }

    /// Private operator-configured Host admission ONLY. The Host must globally
    /// serialize calls across roots, store this value in its prepaid call root
    /// before invoking std, and retain the entire Result before C-only release.
    #[cfg(all(test, windows))]
    #[allow(dead_code)]
    pub(crate) fn reserve_host_canonical(&self) -> Result<HostCanonicalCharge, OperationError> {
        self.check()?;
        let mut state = self.state.lock().expect("operation state");
        let result = state.reserve_host_canonical();
        drop(state); // No original lock survives notification or the std call.
        if matches!(result, Err(OperationError::CaptureOverflow)) { self.overflowed.notify_one(); }
        result?;
        Ok(HostCanonicalCharge { original: self.clone(), active: true })
    }

    /// Admission precedes acceptance or copying. A failed charge is sticky; the successful
    /// prefix remains owned and no ACK/dequeue/eviction refunds this cumulative meter.
    pub fn charge(&self, bytes: usize) -> Result<(), OperationError> {
        let mut state = self.state.lock().expect("operation state");
        if state.overflow {
            return Err(OperationError::CaptureOverflow);
        }
        if state.settled {
            return Err(OperationError::Settled);
        }
        match state.admitted_total(bytes) {
            Some(total) => {
                state.captured_bytes = total;
                Ok(())
            }
            None => {
                state.overflow = true;
                drop(state);
                self.overflowed.notify_one();
                Err(OperationError::CaptureOverflow)
            }
        }
    }

    pub fn overflow(&self) -> OperationError {
        self.state.lock().expect("operation state").overflow = true;
        self.overflowed.notify_one();
        OperationError::CaptureOverflow
    }

    /// Register before checking the latch so a concurrent overflow cannot become a lost wake.
    pub(crate) async fn wait_overflow(&self) {
        loop {
            let notified = self.overflowed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.state.lock().expect("operation state").overflow {
                return;
            }
            notified.await;
        }
    }

    pub fn next_origin(&self) -> Result<CaptureOrigin, OperationError> {
        self.next_origin_for_child(None)
    }

    pub(crate) fn next_child_origin(
        &self,
        child_id: &str,
    ) -> Result<CaptureOrigin, OperationError> {
        self.next_origin_for_child(Some(child_id))
    }

    pub(crate) fn same_instance(&self, other: &Self) -> bool {
        // Literal identity of the SAME retained allocation, never textual UUID
        // equality or equality of temporary Operation handle addresses.
        std::ptr::eq(self.state.as_ref(), other.state.as_ref())
    }

    pub(crate) fn reader_identity(&self) -> ReaderIdentity {
        ReaderIdentity {
            state: Arc::downgrade(&self.state),
            generation: self.child_generation,
        }
    }

    pub(crate) fn reader_handoff(&self) -> Arc<crate::driver::reader_handoff::Gate> {
        self.reader_handoff.clone()
    }

    pub(crate) fn reader_handoff_ref(&self) -> &crate::driver::reader_handoff::Gate {
        &self.reader_handoff
    }

    fn next_origin_for_child(
        &self,
        child_id: Option<&str>,
    ) -> Result<CaptureOrigin, OperationError> {
        // Admit actual identifier copies and retained origin metadata before allocation or
        // advancing the shared ordinal. Later row encoding is an additional charged copy.
        let bytes = self
            .id
            .len()
            .checked_add(child_id.map_or(0, str::len))
            .and_then(|n| n.checked_add(std::mem::size_of::<CaptureOrigin>()))
            .ok_or_else(|| self.overflow())?;
        self.charge(bytes)?;
        let mut state = self.state.lock().expect("operation state");
        if state.overflow {
            return Err(OperationError::CaptureOverflow);
        }
        if state.settled {
            return Err(OperationError::Settled);
        }
        let Some(ordinal) = state.ordinal.checked_add(1) else {
            drop(state);
            return Err(self.overflow());
        };
        state.ordinal = ordinal;
        Ok(CaptureOrigin {
            operation_id: self.id.to_string(),
            child_generation: self.child_generation,
            ordinal,
            physical_child_id: child_id.map(str::to_owned),
        })
    }

    /// Call after a completed write, or conservatively when an in-progress transfer is canceled.
    pub fn possible_start(&self) {
        let mut state = self.state.lock().expect("operation state");
        if !state.settled && state.start == StartEvidence::NotStarted {
            state.start = StartEvidence::Possible;
        }
    }

    /// The reader must establish the operation/session boundary before issuing an origin.
    /// Stale/unattributed rows are never repaired by the current active pointer.
    pub fn confirm(&self, origin: &CaptureOrigin, frame: ConfirmationFrame) -> bool {
        let mut state = self.state.lock().expect("operation state");
        if state.settled
            || origin.operation_id != self.id.as_ref()
            || origin.child_generation != self.child_generation
            || origin.ordinal == 0
            || origin.ordinal > state.ordinal
            || state.start != StartEvidence::Possible
        {
            return false;
        }
        state.start = StartEvidence::Confirmed {
            frame,
            ordinal: origin.ordinal,
        };
        true
    }

    /// Native confirmation requires the original reader's physical child in addition
    /// to the operation/epoch/ordinal checks. A synthetic/legacy origin cannot pass.
    pub(crate) fn confirm_child(
        &self,
        origin: &CaptureOrigin,
        child_id: &str,
        frame: ConfirmationFrame,
    ) -> bool {
        origin.physical_child_id.as_deref() == Some(child_id) && self.confirm(origin, frame)
    }

    pub fn start_evidence(&self) -> StartEvidence {
        self.state.lock().expect("operation state").start.clone()
    }

    pub fn captured_bytes(&self) -> usize {
        self.state.lock().expect("operation state").captured_bytes
    }

    #[allow(dead_code)]
    pub(crate) fn original_growth_usage(
        &self,
    ) -> Result<Option<OriginalGrowthUsage>, OperationError> {
        let state = self.state.lock().map_err(|_| OperationError::CreditOwned)?;
        let Some(budget) = &state.original_growth else {
            return Ok(None); // Legacy state is not a complete original-fit receipt.
        };
        let consumed_and_peak = state
            .captured_bytes
            .checked_sub(budget.fixed_bytes)
            .filter(|used| *used <= budget.growth_limit)
            .ok_or(OperationError::CaptureOverflow)?;
        Ok(Some(OriginalGrowthUsage {
            fixed_bytes: budget.fixed_bytes,
            growth_limit: budget.growth_limit,
            consumed_and_peak,
        }))
    }

    fn claim_command_credit(&self) -> Result<(), OperationError> {
        self.check()?;
        let mut state = self.state.lock().expect("operation state");
        if state.command_owned {
            return Err(OperationError::CreditOwned);
        }
        if state.settled {
            return Err(OperationError::Settled);
        }
        if state.overflow {
            return Err(OperationError::CaptureOverflow);
        }
        state.command_owned = true;
        Ok(())
    }

    /// Only the owning supervisor calls this after actual child/reader/storage settlement.
    /// Timeout itself is not settlement, and settlement never changes start/spend evidence.
    pub fn settle(&self) {
        self.state.lock().expect("operation state").settled = true;
    }
}

/// Counts encoded JSON without first allocating an oversized string or cloning its body.
pub fn encoded_len(value: &impl Serialize, cap: usize) -> Result<usize, OperationError> {
    struct Counter {
        bytes: usize,
        cap: usize,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|n| *n <= self.cap)
                .ok_or_else(|| io::Error::other("capture_overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, cap };
    serde_json::to_writer(&mut counter, value).map_err(|_| OperationError::CaptureOverflow)?;
    Ok(counter.bytes)
}

/// One COMMAND *operation*, with its receipt and settlement envelope under one live subcap.
/// Replacing/dropping an envelope does not refund the cumulative operation charge.
pub struct CommandCredit<R> {
    operation: Operation,
    receipt: Option<R>,
    live_bytes: usize,
}

impl<R: Serialize> CommandCredit<R> {
    pub fn new(operation: Operation) -> Result<Self, OperationError> {
        operation.claim_command_credit()?;
        Ok(Self {
            operation,
            receipt: None,
            live_bytes: 0,
        })
    }

    pub fn retain_receipt(&mut self, receipt: R) -> Result<(), OperationError> {
        if self.receipt.is_some() {
            return Err(OperationError::CreditOwned);
        }
        // An actual accepted receipt is never erased even when its retention charge refuses.
        // The caller already owns R; move it here without cloning, keep it on every error.
        self.receipt = Some(receipt);
        let remaining = COMMAND_BYTES
            .checked_sub(self.live_bytes)
            .ok_or_else(|| self.operation.overflow())?;
        let bytes = encoded_len(self.receipt.as_ref().expect("retained receipt"), remaining)
            .map_err(|_| self.operation.overflow())?;
        self.operation.charge(bytes)?;
        self.live_bytes += bytes;
        Ok(())
    }

    /// Check R+C+metadata together BEFORE the caller copies/accepts C. R survives refusal.
    pub fn admit_settlement(&mut self, command: &impl Serialize) -> Result<usize, OperationError> {
        self.operation.check()?;
        let remaining = COMMAND_BYTES
            .checked_sub(self.live_bytes)
            .ok_or_else(|| self.operation.overflow())?;
        let bytes = encoded_len(command, remaining).map_err(|_| self.operation.overflow())?;
        self.operation.charge(bytes)?;
        self.live_bytes += bytes;
        Ok(bytes)
    }

    pub fn receipt(&self) -> Option<&R> {
        self.receipt.as_ref()
    }

    pub fn operation(&self) -> &Operation {
        &self.operation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn operation() -> Operation {
        Operation::new(
            "op".into(),
            1,
            Allowance::required(Some(Instant::now() + std::time::Duration::from_secs(1))).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn archive_storage_is_reserved_at_creation_and_small_cap_refuses_before_start() {
        let op = operation();
        let fixed =
            std::mem::size_of::<Option<Vec<u8>>>() + std::mem::size_of::<ArchiveChunkLease>();
        assert!(op.captured_bytes() >= ARCHIVE_CHUNK_BYTES + fixed);
        assert_eq!(op.start_evidence(), StartEvidence::NotStarted);
        assert!(matches!(
            prepare_archive_chunk(ARCHIVE_CHUNK_BYTES + fixed - 1),
            Err(OperationError::CaptureOverflow)
        ));
        let chunk = op.borrow_archive_chunk(&op).unwrap();
        assert!(chunk.operation.same_instance(&op));
        assert_eq!(chunk.buffer.as_ref().unwrap().len(), ARCHIVE_CHUNK_BYTES);
        assert_eq!(
            op.captured_bytes(),
            chunk.buffer.as_ref().unwrap().capacity()
                + fixed
                + std::mem::size_of::<Option<Arc<crate::failed_evidence::ArchivePlan>>>()
                + crate::driver::reader_handoff::reserved_bytes()
                + crate::driver::claude_correlation::reserved_bytes()
        );
    }

    #[test]
    fn overflow_and_expiry_keep_original_cleanup_chunk_without_readmission_or_refund() {
        let mut op = operation();
        op.possible_start();
        op.charge(OPERATION_BYTES - op.captured_bytes()).unwrap();
        assert_eq!(op.charge(1), Err(OperationError::CaptureOverflow));
        op.allowance = Allowance(Instant::now());
        assert_eq!(op.allowance.check(), Err(OperationError::Expired));
        let mut chunk = op.borrow_archive_chunk(&op).unwrap();
        let pointer = chunk.bytes_mut().as_ptr();
        chunk.bytes_mut()[0] = 91;
        assert!(matches!(
            op.borrow_archive_chunk(&op),
            Err(OperationError::CreditOwned)
        ));
        drop(chunk); // Cancellation returns the SAME buffer, not evidence/credit settlement.
        let mut again = op.borrow_archive_chunk(&op).unwrap();
        assert_eq!(again.bytes_mut().as_ptr(), pointer);
        assert_eq!(again.bytes_mut()[0], 91);
        assert_eq!(op.captured_bytes(), OPERATION_BYTES);
        assert_eq!(op.check(), Err(OperationError::CaptureOverflow));
        assert_eq!(op.start_evidence(), StartEvidence::Possible);
        assert!(!op.state.lock().unwrap().settled);
    }

    #[test]
    fn archive_checkout_rejects_same_text_different_instance_and_settled_owner() {
        let original = operation();
        let replacement = operation();
        assert_eq!(original.id(), replacement.id());
        let before = original.captured_bytes();
        assert!(matches!(
            original.borrow_archive_chunk(&replacement),
            Err(OperationError::CreditOwned)
        ));
        assert!(original.state.lock().unwrap().archive_chunk.is_some());
        assert_eq!(original.captured_bytes(), before);
        original.settle();
        assert!(matches!(
            original.borrow_archive_chunk(&original),
            Err(OperationError::Settled)
        ));
    }

    #[test]
    fn origin_copy_refusal_keeps_shared_ordinal_and_possible_liability() {
        let operation = operation();
        let child = "original-physical-child";
        let required = operation.id().len() + child.len() + std::mem::size_of::<CaptureOrigin>();
        operation
            .charge(OPERATION_BYTES - operation.captured_bytes() - required + 1)
            .unwrap();
        operation.possible_start();
        let charged = operation.captured_bytes();
        assert_eq!(
            operation.next_child_origin(child),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(operation.state.lock().unwrap().ordinal, 0);
        assert_eq!(operation.captured_bytes(), charged);
        assert_eq!(operation.start_evidence(), StartEvidence::Possible);
    }

    #[test]
    fn exact_fit_origin_charges_identifiers_once_before_shared_ordinal_advance() {
        let operation = operation();
        let child = "original-physical-child";
        let required = operation.id().len() + child.len() + std::mem::size_of::<CaptureOrigin>();
        operation
            .charge(OPERATION_BYTES - operation.captured_bytes() - required)
            .unwrap();
        let origin = operation.next_child_origin(child).unwrap();
        assert_eq!(origin.operation_id, operation.id());
        assert_eq!(origin.physical_child_id.as_deref(), Some(child));
        assert_eq!(origin.ordinal, 1);
        assert_eq!(operation.captured_bytes(), OPERATION_BYTES);
        assert_eq!(
            operation.next_child_origin(child),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(operation.state.lock().unwrap().ordinal, 1);
        assert_eq!(operation.captured_bytes(), OPERATION_BYTES);
        assert_eq!(operation.start_evidence(), StartEvidence::NotStarted);
    }

    #[test]
    fn missing_or_exhausted_allowance_refuses() {
        assert!(matches!(
            Allowance::required(None),
            Err(OperationError::NoAllowance)
        ));
        assert!(matches!(
            Allowance::required(Some(Instant::now())),
            Err(OperationError::Expired)
        ));
    }

    #[test]
    fn command_receipt_and_settlement_exact_fit_or_receipt_kept() {
        let command = json!({"marker":2,"metadata":"known"});
        let command_bytes = encoded_len(&command, COMMAND_BYTES).unwrap();
        let receipt = "r".repeat(COMMAND_BYTES - command_bytes - 2);
        let mut credit = CommandCredit::new(operation()).unwrap();
        credit.retain_receipt(receipt.clone()).unwrap();
        assert_eq!(credit.admit_settlement(&command).unwrap(), command_bytes);
        assert_eq!(credit.live_bytes, COMMAND_BYTES);
        assert_eq!(
            credit.admit_settlement(&json!(null)),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(credit.receipt(), Some(&receipt));
        assert_eq!(
            credit.operation.check(),
            Err(OperationError::CaptureOverflow)
        );
    }

    #[test]
    fn receipt_keeps_preexisting_request_metadata_in_live_cap() {
        let metadata = "m".repeat(COMMAND_BYTES - 6);
        let mut credit = CommandCredit::new(operation()).unwrap();
        assert_eq!(
            credit.admit_settlement(&metadata).unwrap(),
            COMMAND_BYTES - 4
        );
        credit.retain_receipt("ok").unwrap();
        assert_eq!(credit.live_bytes, COMMAND_BYTES);
        assert_eq!(
            credit.admit_settlement(&()),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(credit.receipt(), Some(&"ok"));
    }

    #[test]
    fn receipt_survives_combined_metadata_overflow() {
        let mut credit = CommandCredit::new(operation()).unwrap();
        credit
            .admit_settlement(&"m".repeat(COMMAND_BYTES - 4))
            .unwrap();
        assert_eq!(
            credit.retain_receipt("ok"),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(credit.receipt(), Some(&"ok"));
        assert_eq!(credit.live_bytes, COMMAND_BYTES - 2);
    }

    #[test]
    fn copies_exhaust_total_without_refunding_receipt() {
        let op = operation();
        let mut credit = CommandCredit::new(op.clone()).unwrap();
        credit.retain_receipt(json!({"accepted":true})).unwrap();
        let used = op.captured_bytes();
        op.charge(OPERATION_BYTES - used).unwrap();
        assert_eq!(
            credit.admit_settlement(&json!({"finish":true})),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(credit.receipt(), Some(&json!({"accepted":true})));
        assert_eq!(op.captured_bytes(), OPERATION_BYTES);
    }

    #[test]
    fn actual_oversize_receipt_is_retained_on_subcap_refusal() {
        let receipt = "r".repeat(COMMAND_BYTES);
        let mut credit = CommandCredit::new(operation()).unwrap();
        assert_eq!(
            credit.retain_receipt(receipt.clone()),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(credit.receipt(), Some(&receipt));
    }

    #[test]
    fn real_accepted_receipt_domain_over_command_cap_is_kept_whole() {
        let receipt = crate::game::SubmitReceipt {
            decision_id: "actual-decision".into(),
            duplicate: false,
            summary: "accepted".into(),
            result: json!({"body":"x".repeat(COMMAND_BYTES)}),
        };
        // This real receipt is below the raw-frame cap yet cannot fit the COMMAND subcap.
        assert!(encoded_len(&receipt, FRAME_BYTES).unwrap() > COMMAND_BYTES);
        let mut credit = CommandCredit::new(operation()).unwrap();
        assert_eq!(
            credit.retain_receipt(receipt),
            Err(OperationError::CaptureOverflow)
        );
        let kept = credit.receipt().unwrap();
        assert_eq!(kept.decision_id, "actual-decision");
        assert_eq!(kept.result["body"].as_str().unwrap().len(), COMMAND_BYTES);
        assert_eq!(
            credit.admit_settlement(&json!({"finish_delivery":true})),
            Err(OperationError::CaptureOverflow)
        );
        assert!(credit.receipt().is_some());
    }

    #[test]
    fn one_command_credit_counts_operation_not_ack_or_handle_drop() {
        let operation = operation();
        let credit = CommandCredit::<serde_json::Value>::new(operation.clone()).unwrap();
        assert!(matches!(
            CommandCredit::<serde_json::Value>::new(operation.clone()),
            Err(OperationError::CreditOwned)
        ));
        drop(credit);
        assert!(matches!(
            CommandCredit::<serde_json::Value>::new(operation.clone()),
            Err(OperationError::CreditOwned)
        ));
        operation.settle();
        assert!(matches!(
            CommandCredit::<serde_json::Value>::new(operation),
            Err(OperationError::Settled)
        ));
    }

    #[test]
    fn escaped_json_size_is_encoded_and_confirmations_are_monotonic() {
        assert_eq!(encoded_len(&"\n", 3), Err(OperationError::CaptureOverflow));
        assert_eq!(encoded_len(&"\n", 4).unwrap(), 4);
        let op = operation();
        let origin = op.next_origin().unwrap();
        assert!(!op.confirm(&origin, ConfirmationFrame::Assistant));
        op.possible_start();
        let mut stale = origin.clone();
        stale.child_generation = 0;
        assert!(!op.confirm(&stale, ConfirmationFrame::Assistant));
        assert!(op.confirm(&origin, ConfirmationFrame::Assistant));
        assert!(!op.confirm(&origin, ConfirmationFrame::ResultUsage));
        op.possible_start();
        op.settle();
        assert_eq!(
            op.start_evidence(),
            StartEvidence::Confirmed {
                frame: ConfirmationFrame::Assistant,
                ordinal: 1
            }
        );
    }

    #[test]
    fn native_confirmation_requires_same_physical_child_and_legacy_stays_unknown() {
        let op = operation();
        op.possible_start();
        let first_child = uuid::Uuid::new_v4().to_string();
        let replacement_child = uuid::Uuid::new_v4().to_string();
        let origin = op.next_child_origin(&first_child).unwrap();
        assert!(!op.confirm_child(&origin, &replacement_child, ConfirmationFrame::Assistant));
        let legacy: CaptureOrigin = serde_json::from_value(json!({
            "operation_id":op.id(),"child_generation":origin.child_generation,"ordinal":origin.ordinal
        })).unwrap();
        assert!(legacy.physical_child_id.is_none());
        assert!(!op.confirm_child(&legacy, &first_child, ConfirmationFrame::Assistant));
        assert_eq!(op.start_evidence(), StartEvidence::Possible);
        assert!(op.confirm_child(&origin, &first_child, ConfirmationFrame::MessageStart));
        assert!(!op.confirm_child(&origin, &first_child, ConfirmationFrame::ResultUsage));
        assert_eq!(
            op.start_evidence(),
            StartEvidence::Confirmed {
                frame: ConfirmationFrame::MessageStart,
                ordinal: origin.ordinal
            }
        );
        assert!(legacy.physical_child_id.is_none());
    }
}
