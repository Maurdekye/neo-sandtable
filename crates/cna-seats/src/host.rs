//! Required trusted-host admission. Nothing here is a seat tool or spectator payload.
//!
//! The Server adapter orders permission against its single writer. A snapshot is advisory;
//! every effect needs a fresh permit. Native cleanup remains owned when permission closes.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cna_core::decision::DecisionRequest;
use cna_core::ids::SeatId;
use tokio::sync::{Notify, watch};

use crate::game::{SubmitReceipt, SubmitRequest};
use crate::operation::{Allowance, Operation};
use crate::transcript::UnconfirmedEntry;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostIdentity {
    pub seat: SeatId,
    pub epoch: u64,
    pub decision_id: String,
    pub revision: u32,
    pub operation_id: String,
}

/// Immutable neutral route text has an exact length allocation. Charge the
/// SAME original meter BEFORE copying, with the existing PREPARE ceiling as
/// an additional gate. No growable reserve/excess capacity or refund is used.
/// Call outside route/settlement exclusion: overflow may synchronously wake.
pub(crate) fn copy_route_text(operation: &Operation, text: &str) -> Result<String, HostRefusal> {
    operation.check().map_err(operation_refusal)?;
    if text.len() > crate::operation::PREPARE_BYTES {
        return Err(operation_refusal(operation.overflow()));
    }
    operation.charge(text.len()).map_err(operation_refusal)?;
    operation.check().map_err(operation_refusal)?;
    // Box<str> has exactly len bytes; consuming it as String does not copy or
    // allocate again. Allocator residual follows the existing policy. These
    // owned strings are never appended to, so no later capacity growth occurs.
    Ok(String::from(Box::<str>::from(text)))
}

pub(crate) fn copy_route_identity(
    operation: &Operation,
    identity: &HostIdentity,
) -> Result<HostIdentity, HostRefusal> {
    operation.check().map_err(operation_refusal)?;
    if identity.operation_id != operation.id() {
        return Err(HostRefusal::IdentityChanged);
    }
    // Inputs remain in their canonical owner if either admitted copy refuses.
    // Partial temporary copies may drop; their cumulative charge is not returned.
    let decision_id = copy_route_text(operation, &identity.decision_id)?;
    let operation_id = copy_route_text(operation, &identity.operation_id)?;
    Ok(HostIdentity {
        seat: identity.seat,
        epoch: identity.epoch,
        decision_id,
        revision: identity.revision,
        operation_id,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostStatus {
    Running,
    Paused,
    Finished,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSnapshot {
    pub generation: u64,
    pub activated: bool,
    pub status: HostStatus,
    pub epochs: BTreeMap<SeatId, u64>,
    pub pending: BTreeMap<SeatId, Vec<(String, u32)>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostStep {
    Reserve,
    PreflightSpawn,
    SessionSpawn,
    Resume,
    Reseed,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HostRefusal {
    #[error("recovered, awaiting activation")]
    WaitingForActivation,
    #[error("host stopped before work")]
    StoppedBeforeWork,
    #[error("host identity changed")]
    IdentityChanged,
    #[error("host admission unavailable")]
    Unknown,
    #[error("no_allowance")]
    NoAllowance,
    #[error("operation allowance exhausted")]
    Expired,
    #[error("capture_overflow")]
    CaptureOverflow,
    #[error("host ownership error: {0}")]
    Ownership(String),
}

/// The implementation holds its writer exclusion ONLY around this bounded synchronous effect.
/// It must revalidate the identity before executing, and must never await the actor or storage.
pub trait HostStepLease: Send {
    fn perform_sync(
        &mut self,
        effect: &mut dyn FnMut() -> Result<(), HostRefusal>,
    ) -> Result<(), HostRefusal>;
}

/// Dispatch owns the host's Prompt state across async writes. `settle` is called only after
/// actual dispatch/supervision disposition, never because the caller's timer or handle dropped.
#[async_trait]
pub trait HostPromptLease: Send + Sync {
    fn check(&self) -> Result<(), HostRefusal>;
    async fn settle(&self) -> Result<(), HostRefusal>;
}

pub trait HostAdmission: Send + Sync {
    /// Subscribe BEFORE snapshot; readers still reacquire before every effect across awaits.
    fn subscribe(&self) -> watch::Receiver<HostSnapshot>;
    fn snapshot(&self) -> Result<HostSnapshot, HostRefusal>;
    fn acquire(
        &self,
        identity: &HostIdentity,
        step: HostStep,
    ) -> Result<Box<dyn HostStepLease>, HostRefusal>;
    fn acquire_prompt(
        &self,
        identity: &HostIdentity,
        allowance: Allowance,
    ) -> Result<Box<dyn HostPromptLease>, HostRefusal>;
}

/// Neutral operator-only storage facts (lead maf02). No private Server type or
/// user-supplied byte count crosses this boundary. The producer must use its
/// ACTUAL fixed entry/slot/permit/copy aggregate, verified in the whole joint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostOperationLayout {
    size: usize,
    alignment: usize,
}

impl HostOperationLayout {
    pub const fn size(self) -> usize {
        self.size
    }
    pub const fn alignment(self) -> usize {
        self.alignment
    }
}

/// Seats-owned fixed child inventory for ONE neutral reservation. These are
/// ACTUAL private allocation bodies, rather than Arc pointer headers. Prepared
/// route and BOTH marker headers conservatively overlap the reservation's
/// children until final settlement. No values of this inventory are constructed.
///
/// This is deliberately NOT complete Forced storage: String capacities/copies
/// MUST consume original G before allocation; complete original seed wiring,
/// begin/submit/delivery frames and library/accepted residual policy remain
/// separate obligations.
/// Server's own Reserved/Active aggregate must still contain all its own types.
#[allow(dead_code)]
struct NeutralForcedFixedStorage {
    reserved: ReservedInner,
    cancellation: RouteCancellation,
    state: Mutex<RouteState>,
    changed: Notify,
    prepared: PreparedRoute,
    intent: ConfirmedIntent,
    postcommit: ConfirmedPostcommit,
    worker: crate::transcript::WorkerRoute,
}

/// The descriptor VALUE contains only size/alignment. This separate trusted
/// envelope keeps the exact service owner alive for the creation-time identity
/// check. It is neither an effect permit nor a full original-fit receipt.
/// Server's ONE synchronous CampaignHandle accessor is the production source;
/// callers must not construct a receipt with an unrelated storage type.
pub struct BoundHostOperationLayout {
    owner: Arc<dyn HostAdmission>,
    layout: HostOperationLayout,
}

impl BoundHostOperationLayout {
    /// Necessary cross-crate representation constructor for the owner accessor.
    /// Derive only compile-time ACTUAL storage facts; NO numeric-size constructor,
    /// custom-layout input, effect method, query, callback, downcast or UUID guess.
    /// `Storage` is Server's concrete worst-case bounded storage aggregate.
    /// The same descriptor also includes Seats' actual private neutral child
    /// bodies. Public owner/route pointer sizes alone cannot account for them.
    /// Complete payload/copy/frame/library accounting is still required before
    /// this descriptor may participate in positive original admission.
    pub fn for_storage<Storage>(owner: Arc<dyn HostAdmission>) -> Self {
        let actual = std::alloc::Layout::new::<(Storage, NeutralForcedFixedStorage)>();
        Self {
            owner,
            layout: HostOperationLayout {
                // Neutral children cannot turn a missing owner aggregate into
                // a positive descriptor. Preserve the existing unknown gate.
                size: if std::mem::size_of::<Storage>() == 0 {
                    0
                } else {
                    actual.size()
                },
                alignment: actual.align(),
            },
        }
    }

    pub const fn layout(&self) -> HostOperationLayout {
        self.layout
    }

    pub fn matches_owner(&self, expected: &Arc<dyn HostAdmission>) -> bool {
        Arc::ptr_eq(&self.owner, expected)
    }

    /// Borrow only; never consumes/repairs a missing, foreign or malformed input.
    /// Check identity BEFORE even observing the owner's charge. This scalar is
    /// a creation obligation, never authority to allocate/bind or reserve late.
    pub(crate) fn creation_bytes_for(
        &self,
        expected: &Arc<dyn HostAdmission>,
    ) -> Result<usize, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        if !self.matches_owner(expected) {
            return Err(ArchiveError::Identity);
        }
        if self.layout.size == 0 {
            return Err(ArchiveError::Identity);
        }
        let actual = std::alloc::Layout::from_size_align(self.layout.size, self.layout.alignment)
            .map_err(|_| ArchiveError::Fit)?;
        let bytes = actual.pad_to_align().size();
        if bytes > crate::operation::OPERATION_BYTES {
            return Err(ArchiveError::Fit);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod owner_layout_source_controls {
    use super::*;
    use crate::failed_evidence::ArchiveError;
    struct Owner;
    impl HostAdmission for Owner {
        fn subscribe(&self) -> watch::Receiver<HostSnapshot> {
            panic!("layout must not subscribe");
        }
        fn snapshot(&self) -> Result<HostSnapshot, HostRefusal> {
            panic!("layout must not snapshot");
        }
        fn acquire(
            &self,
            _: &HostIdentity,
            _: HostStep,
        ) -> Result<Box<dyn HostStepLease>, HostRefusal> {
            panic!("layout must not acquire");
        }
        fn acquire_prompt(
            &self,
            _: &HostIdentity,
            _: Allowance,
        ) -> Result<Box<dyn HostPromptLease>, HostRefusal> {
            panic!("layout must not acquire prompt");
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Synthetic private types, NOT actual Server
    // storage equality or a positive original creation/admission witness.
    #[test]
    fn owner_layout_uses_actual_type_and_checks_identity_before_malformed_facts() {
        #[repr(align(64))]
        struct Storage {
            _entry: [u8; 47],
            _permit: [u8; 32],
        }
        let owner: Arc<dyn HostAdmission> = Arc::new(Owner);
        let foreign: Arc<dyn HostAdmission> = Arc::new(Owner);
        let mut descriptor = BoundHostOperationLayout::for_storage::<Storage>(owner.clone());
        let actual = std::alloc::Layout::new::<(Storage, NeutralForcedFixedStorage)>();
        assert_eq!(
            std::mem::size_of::<HostOperationLayout>(),
            2 * std::mem::size_of::<usize>()
        );
        assert_eq!(descriptor.layout().size(), actual.size());
        assert_eq!(descriptor.layout().alignment(), actual.align());
        assert_eq!(
            descriptor.creation_bytes_for(&owner).unwrap(),
            actual.size()
        );
        assert!(matches!(
            descriptor.creation_bytes_for(&foreign),
            Err(ArchiveError::Identity)
        ));
        let count = Arc::strong_count(&owner);
        for facts in [
            HostOperationLayout {
                size: 1,
                alignment: 0,
            },
            HostOperationLayout {
                size: 1,
                alignment: 3,
            },
            HostOperationLayout {
                size: usize::MAX,
                alignment: 16,
            },
        ] {
            descriptor.layout = facts; // Private malformed SOURCE fixture only.
            assert!(matches!(
                descriptor.creation_bytes_for(&foreign),
                Err(ArchiveError::Identity)
            ));
            assert!(matches!(
                descriptor.creation_bytes_for(&owner),
                Err(ArchiveError::Fit)
            ));
            assert_eq!(descriptor.layout(), facts); // No repair/retry mutation.
        }
        assert_eq!(Arc::strong_count(&owner), count);
        let oversized = BoundHostOperationLayout::for_storage::<
            [u8; crate::operation::OPERATION_BYTES + 1],
        >(owner.clone());
        assert!(matches!(
            oversized.creation_bytes_for(&owner),
            Err(ArchiveError::Fit)
        ));
        let unknown = BoundHostOperationLayout::for_storage::<()>(owner.clone());
        assert!(matches!(
            unknown.creation_bytes_for(&owner),
            Err(ArchiveError::Identity)
        ));
    }
}

/// Required on every native entrypoint, including preflight helpers. No absent-adapter fallback.
#[derive(Clone)]
pub struct HostDriverContext {
    pub admission: Arc<dyn HostAdmission>,
    pub identity: HostIdentity,
    pub operation: Operation,
}

impl HostDriverContext {
    pub fn acquire(&self, step: HostStep) -> Result<Box<dyn HostStepLease>, HostRefusal> {
        self.operation.check().map_err(operation_refusal)?;
        self.admission.acquire(&self.identity, step)
    }

    pub fn acquire_prompt(&self) -> Result<Box<dyn HostPromptLease>, HostRefusal> {
        self.operation.check().map_err(operation_refusal)?;
        self.admission
            .acquire_prompt(&self.identity, self.operation.allowance())
    }
}

// OWN neutral refusal bodies (lead m86): these THREE reachable producers have
// no IDs/numbers/input text. Exact immutable boxed strings avoid Display-buffer
// spare growth. Actual-capacity controls are still UNCOMPILED/UNRUN; this is
// not original-F coverage until N is derived from the whole live MOVE trace.
const SETTLED_OWNERSHIP_TEXT: &str = "operation already settled";
const CREDIT_OWNERSHIP_TEXT: &str = "operation credit still owned";
const SETTLEMENT_OWNERSHIP_TEXT: &str = "settlement transition in progress";
pub(crate) const NEUTRAL_OWNERSHIP_BODY_CAPACITY: usize = SETTLEMENT_OWNERSHIP_TEXT.len();
const _: () = assert!(SETTLED_OWNERSHIP_TEXT.len() <= NEUTRAL_OWNERSHIP_BODY_CAPACITY);
const _: () = assert!(CREDIT_OWNERSHIP_TEXT.len() <= NEUTRAL_OWNERSHIP_BODY_CAPACITY);

fn neutral_ownership_body(text: &'static str) -> String {
    // These allocations MUST be prepaid in ORIGINAL F under m86. This body
    // constructor only supplies the capacity fact; NO complete N/F cover is
    // implemented yet. No G/counter/reserve/refund occurs here. The positive
    // original entry stays CLOSED pending the full live-count/MOVE trace.
    String::from(Box::<str>::from(text))
}

pub fn operation_refusal(error: crate::operation::OperationError) -> HostRefusal {
    use crate::operation::OperationError;
    match error {
        OperationError::NoAllowance => HostRefusal::NoAllowance,
        OperationError::Expired => HostRefusal::Expired,
        OperationError::CaptureOverflow => HostRefusal::CaptureOverflow,
        OperationError::Settled => {
            HostRefusal::Ownership(neutral_ownership_body(SETTLED_OWNERSHIP_TEXT))
        }
        OperationError::CreditOwned => {
            HostRefusal::Ownership(neutral_ownership_body(CREDIT_OWNERSHIP_TEXT))
        }
    }
}

/// Original-author append validation remains the adapter's responsibility. Routing cannot grant
/// the forced owner permission to author someone else's payload or change its epoch.
#[async_trait]
pub trait HostDeliveryRoute: Send + Sync {
    async fn append(
        &self,
        stream: &str,
        ordinal: u64,
        row: &UnconfirmedEntry,
    ) -> Result<u64, String>;
}

pub(crate) enum RouteState {
    Reserved,
    Prepared,
    Installed {
        token: String,
        delivery: Arc<dyn HostDeliveryRoute>,
    },
    Released,
    Uncertain,
}

/// One preallocated drop/abort notification, independent of either full settlement lane.
pub struct RouteCancellation {
    requested: AtomicBool,
    changed: Notify,
    settlement_requested: AtomicBool,
    settlement_changed: Notify,
    settlement: Mutex<RouteSettlementState>,
    acknowledged: Notify,
}

struct ReservedInner {
    identity: HostIdentity,
    operation: Operation,
    cancel: Arc<RouteCancellation>,
    state: Arc<Mutex<RouteState>>,
    changed: Arc<Notify>,
    stream: Mutex<Option<String>>,
}

/// Kept ONLY by the trusted actor's existing campaign operation slot. Creating this value
/// does not reserve a campaign or admit work; Server must claim its sole slot synchronously
/// before returning a reservation. It must retain this owner on every admitted refusal.
pub struct ForcedReservationOwner {
    inner: Arc<ReservedInner>,
}

/// Read/worker view on the SAME reservation; it cannot register a writer or create an ACK.
#[derive(Clone)]
pub struct ReservedForcedRoute {
    inner: Arc<ReservedInner>,
}

impl ForcedReservationOwner {
    pub fn new(identity: HostIdentity, operation: Operation) -> Result<Self, HostRefusal> {
        operation.check().map_err(operation_refusal)?;
        if identity.operation_id != operation.id() {
            return Err(HostRefusal::IdentityChanged);
        }
        let metadata = (
            identity.seat.to_string(),
            identity.epoch,
            &identity.decision_id,
            identity.revision,
            &identity.operation_id,
        );
        let bytes = crate::operation::encoded_len(&metadata, crate::operation::PREPARE_BYTES)
            .map_err(|_| operation_refusal(operation.overflow()))?;
        let fixed = std::mem::size_of::<ReservedInner>()
            + std::mem::size_of::<RouteCancellation>()
            + crate::operation::PREPARE_BYTES
            + crate::automatic::retained_begin_metadata_bytes();
        let total = bytes
            .checked_mul(8)
            .and_then(|n| n.checked_add(fixed))
            .ok_or_else(|| operation_refusal(operation.overflow()))?;
        operation.charge(total).map_err(operation_refusal)?;
        if std::mem::size_of::<std::thread::Thread>() * 4 > crate::operation::PREPARE_BYTES {
            return Err(operation_refusal(operation.overflow()));
        }
        Ok(Self {
            inner: Arc::new(ReservedInner {
                identity,
                operation,
                cancel: Arc::new(RouteCancellation::new()),
                state: Arc::new(Mutex::new(RouteState::Reserved)),
                changed: Arc::new(Notify::new()),
                stream: Mutex::new(None),
            }),
        })
    }
    pub fn route(&self) -> ReservedForcedRoute {
        ReservedForcedRoute {
            inner: self.inner.clone(),
        }
    }
    pub fn identity(&self) -> &HostIdentity {
        &self.inner.identity
    }
    pub fn operation(&self) -> Operation {
        self.inner.operation.clone()
    }
    pub fn cancellation(&self) -> Arc<RouteCancellation> {
        self.inner.cancel.clone()
    }
    pub fn register_writer_wake(
        &self,
        writer: std::thread::Thread,
        metadata_bytes: usize,
    ) -> Result<(), HostRefusal> {
        register_writer_association(
            &self.inner.cancel,
            &self.inner.operation,
            writer,
            metadata_bytes,
        )
    }
    /// Actual actor final transition only, including Reserved with no accepted worker route.
    /// Neither consumer view nor cancellation handle exposes this witness constructor.
    pub fn acknowledge_settlement(
        &self,
        handle: &Arc<RouteCancellation>,
        uncertain: bool,
    ) -> Result<RouteSettlementAck, HostRefusal> {
        acknowledge_writer_association(
            &self.inner.cancel,
            &self.inner.state,
            &self.inner.changed,
            handle,
            uncertain,
        )
    }
}

impl Drop for ForcedReservationOwner {
    fn drop(&mut self) {
        self.inner.cancel.request();
    }
}

impl ReservedForcedRoute {
    pub fn identity(&self) -> &HostIdentity {
        &self.inner.identity
    }
    pub fn operation(&self) -> Operation {
        self.inner.operation.clone()
    }
    pub fn cancellation(&self) -> Arc<RouteCancellation> {
        self.inner.cancel.clone()
    }
    /// Claim exactly one sink BEFORE accepting its prepare envelope. This is not campaign
    /// admission: the actor's reservation already owns that sole operation and original request.
    pub(crate) fn claim_stream(&self, stream: &str) -> Result<(), HostRefusal> {
        self.inner.operation.check().map_err(operation_refusal)?;
        if self.inner.cancel.requested() || self.inner.cancel.settlement_requested() {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        let bytes = crate::operation::encoded_len(&stream, crate::operation::PREPARE_BYTES)
            .map_err(|_| operation_refusal(self.inner.operation.overflow()))?;
        self.inner
            .operation
            .charge(
                bytes
                    .checked_mul(3)
                    .ok_or_else(|| operation_refusal(self.inner.operation.overflow()))?,
            )
            .map_err(operation_refusal)?;
        {
            let claimed = self.inner.stream.lock().expect("reserved route stream");
            let state = self.inner.state.lock().expect("reserved route state");
            if claimed.is_some() || !matches!(*state, RouteState::Reserved) {
                return Err(HostRefusal::IdentityChanged);
            }
        }
        let stream = copy_route_text(&self.inner.operation, stream)?;
        let mut claimed = self.inner.stream.lock().expect("reserved route stream");
        let state = self.inner.state.lock().expect("reserved route state");
        self.inner.operation.check().map_err(operation_refusal)?;
        if self.inner.cancel.requested() || self.inner.cancel.settlement_requested() {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        if claimed.is_some() || !matches!(*state, RouteState::Reserved) {
            return Err(HostRefusal::IdentityChanged);
        }
        *claimed = Some(stream);
        Ok(())
    }
    /// Worker-only boundary after its actual normal in-flight append ACK, on the claimed sink.
    pub(crate) fn prepare(
        &self,
        stream: &str,
        boundary: u64,
    ) -> Result<PreparedRoute, HostRefusal> {
        self.inner.operation.check().map_err(operation_refusal)?;
        if self.inner.cancel.requested() || self.inner.cancel.settlement_requested() {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        {
            let claimed = self.inner.stream.lock().expect("reserved route stream");
            let state = self.inner.state.lock().expect("reserved route state");
            if claimed.as_deref() != Some(stream) || !matches!(*state, RouteState::Reserved) {
                return Err(HostRefusal::IdentityChanged);
            }
        }
        // All fallible body copies finish BEFORE the state transition and
        // outside route locks. A failed budget wakes no observer under them.
        let owned_stream = copy_route_text(&self.inner.operation, stream)?;
        let owner = copy_route_identity(&self.inner.operation, &self.inner.identity)?;
        let claimed = self.inner.stream.lock().expect("reserved route stream");
        let mut state = self.inner.state.lock().expect("reserved route state");
        self.inner.operation.check().map_err(operation_refusal)?;
        if self.inner.cancel.requested() || self.inner.cancel.settlement_requested() {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        if claimed.as_deref() != Some(stream) || !matches!(*state, RouteState::Reserved) {
            return Err(HostRefusal::IdentityChanged);
        }
        *state = RouteState::Prepared;
        Ok(PreparedRoute {
            stream: owned_stream,
            owner,
            boundary,
            allowance: self.inner.operation.allowance(),
            state: self.inner.state.clone(),
            changed: self.inner.changed.clone(),
            cancel: self.inner.cancel.clone(),
            operation: self.inner.operation.clone(),
        })
    }
}

struct RouteSettlementState {
    writer_wake: Option<std::thread::Thread>,
    acknowledging: bool,
    ack: Option<RouteSettlementAck>,
}

/// Actor disposition only. This does not witness child, reader or storage joins.
/// The constructor is private; the actor can obtain it only through its SAME route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteSettlementAck {
    uncertain: bool,
}

impl RouteSettlementAck {
    pub fn uncertain(self) -> bool {
        self.uncertain
    }
}

impl RouteCancellation {
    pub(crate) fn new() -> Self {
        Self {
            requested: AtomicBool::new(false),
            changed: Notify::new(),
            settlement_requested: AtomicBool::new(false),
            settlement_changed: Notify::new(),
            settlement: Mutex::new(RouteSettlementState {
                writer_wake: None,
                acknowledging: false,
                ack: None,
            }),
            acknowledged: Notify::new(),
        }
    }
    fn wake_writer(&self) {
        let wake = self
            .settlement
            .lock()
            .expect("route settlement")
            .writer_wake
            .clone();
        // A concrete Thread handle cannot hide arbitrary closure captures. Its unpark runs
        // outside exclusion; it requests writer attention, never admission or settlement.
        if let Some(wake) = wake {
            wake.unpark();
        }
    }
    pub fn request(&self) {
        if self.requested.swap(true, Ordering::AcqRel) {
            return;
        }
        self.changed.notify_one();
        self.wake_writer();
    }
    pub fn requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
    pub async fn notified(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.requested() {
                return;
            }
            notified.await;
        }
    }

    /// Trusted caller only, AFTER actual original supervision disposition. A deadline or
    /// dropped begin future calls `request`, never this method. No credit is released here.
    pub fn request_settlement_after_supervision(&self) {
        if self.settlement_requested.swap(true, Ordering::AcqRel) {
            return;
        }
        self.settlement_changed.notify_waiters();
        self.wake_writer();
    }
    pub fn settlement_requested(&self) -> bool {
        self.settlement_requested.load(Ordering::Acquire)
    }
    pub async fn settlement_notified(&self) {
        loop {
            let notified = self.settlement_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.settlement_requested() {
                return;
            }
            notified.await;
        }
    }
    pub fn acknowledged_settlement(&self) -> Option<RouteSettlementAck> {
        self.settlement.lock().expect("route settlement").ack
    }
    /// Retained ACK is independent of delivery of the begin reply or token. The caller owns
    /// this wait across cancellation; an elapsed allowance is never substituted for an ACK.
    pub async fn settlement_ack(&self) -> RouteSettlementAck {
        loop {
            let notified = self.acknowledged.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(ack) = self.acknowledged_settlement() {
                return ack;
            }
            notified.await;
        }
    }
}

/// Worker-created after SAME normal in-flight append ACK and next-issue freeze. No public
/// constructor: Server receives this capability from the trusted sink's prepare handshake.
pub struct PreparedRoute {
    pub(crate) stream: String,
    pub(crate) owner: HostIdentity,
    pub(crate) boundary: u64,
    pub(crate) allowance: Allowance,
    pub(crate) state: Arc<Mutex<RouteState>>,
    pub(crate) changed: Arc<Notify>,
    pub(crate) cancel: Arc<RouteCancellation>,
    pub(crate) operation: Operation,
}

fn register_writer_association(
    cancel: &Arc<RouteCancellation>,
    operation: &Operation,
    writer: std::thread::Thread,
    metadata_bytes: usize,
) -> Result<(), HostRefusal> {
    let mut state = cancel.settlement.lock().expect("route settlement");
    if state.writer_wake.is_some() || state.ack.is_some() {
        return Err(HostRefusal::IdentityChanged);
    }
    state.writer_wake = Some(writer);
    drop(state);
    if cancel.requested() || cancel.settlement_requested() {
        cancel.wake_writer();
    }
    if metadata_bytes > crate::operation::PREPARE_BYTES {
        operation.overflow();
        cancel.request();
        return Err(HostRefusal::CaptureOverflow);
    }
    match operation.check() {
        Ok(()) => Ok(()),
        Err(error) => {
            cancel.request();
            Err(operation_refusal(error))
        }
    }
}

fn acknowledge_writer_association(
    cancel: &Arc<RouteCancellation>,
    route_state: &Arc<Mutex<RouteState>>,
    changed: &Arc<Notify>,
    handle: &Arc<RouteCancellation>,
    uncertain: bool,
) -> Result<RouteSettlementAck, HostRefusal> {
    if !Arc::ptr_eq(cancel, handle) || !handle.settlement_requested() {
        return Err(HostRefusal::IdentityChanged);
    }
    let ack = RouteSettlementAck { uncertain };
    let mut state = handle.settlement.lock().expect("route settlement");
    if state.writer_wake.is_none() {
        return Err(HostRefusal::IdentityChanged);
    }
    if let Some(previous) = state.ack {
        return if previous == ack {
            Ok(previous)
        } else {
            Err(HostRefusal::IdentityChanged)
        };
    }
    if state.acknowledging {
        return Err(HostRefusal::Ownership(neutral_ownership_body(
            SETTLEMENT_OWNERSHIP_TEXT,
        )));
    }
    state.acknowledging = true;
    drop(state);
    *route_state.lock().expect("reserved route disposition") = if uncertain {
        RouteState::Uncertain
    } else {
        RouteState::Released
    };
    changed.notify_one();
    let mut state = handle.settlement.lock().expect("route settlement");
    state.ack = Some(ack);
    drop(state);
    handle.acknowledged.notify_waiters();
    Ok(ack)
}

impl PreparedRoute {
    pub fn stream(&self) -> &str {
        &self.stream
    }
    pub fn owner(&self) -> &HostIdentity {
        &self.owner
    }
    pub fn boundary(&self) -> u64 {
        self.boundary
    }
    pub fn allowance(&self) -> Allowance {
        self.allowance
    }
    pub fn cancellation(&self) -> Arc<RouteCancellation> {
        self.cancel.clone()
    }
    /// The SAME cumulative meter and operation credit, never a new identity-only meter.
    pub fn operation(&self) -> Operation {
        self.operation.clone()
    }

    /// Called by the single writer within its coherent begin transition BEFORE replying. This
    /// changes issue routing, never an already-issued append's author or payload.
    pub fn install(
        &self,
        token: String,
        delivery: Arc<dyn HostDeliveryRoute>,
    ) -> Result<(), HostRefusal> {
        self.operation.check().map_err(operation_refusal)?;
        if self.cancel.requested() || self.cancel.settlement_requested() || token.is_empty() {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        let mut state = self.state.lock().expect("prepared route");
        if !matches!(*state, RouteState::Prepared) {
            return Err(HostRefusal::IdentityChanged);
        }
        *state = RouteState::Installed { token, delivery };
        drop(state);
        self.changed.notify_one();
        Ok(())
    }
}

impl Drop for PreparedRoute {
    fn drop(&mut self) {
        self.cancel.request();
    }
}

pub struct ConfirmedIntent {
    pub(crate) marker: MarkerWitness,
}
pub struct ConfirmedPostcommit {
    pub(crate) marker: MarkerWitness,
}

pub(crate) struct MarkerWitness {
    pub stream: String,
    pub token: String,
    pub owner: HostIdentity,
    pub ordinal: u64,
}

macro_rules! marker_accessors {
    ($name:ident) => {
        impl $name {
            pub fn stream(&self) -> &str {
                &self.marker.stream
            }
            pub fn token(&self) -> &str {
                &self.marker.token
            }
            pub fn owner(&self) -> &HostIdentity {
                &self.marker.owner
            }
            pub fn ordinal(&self) -> u64 {
                self.marker.ordinal
            }
        }
    };
}
marker_accessors!(ConfirmedIntent);
marker_accessors!(ConfirmedPostcommit);

pub trait HostForcedSubmission: Send + Sync {
    /// Atomically claim the existing sole campaign operation BEFORE any worker prepare.
    /// Retain the original request once, charging all bodies/copies in COMMAND and Operation.
    /// Busy/closed/oversize refuses without accepting a worker route. No async/storage work.
    fn reserve(
        &self,
        identity: HostIdentity,
        request: SubmitRequest,
        operation: Operation,
    ) -> Result<Box<dyn HostForcedReservation>, HostRefusal>;
}

#[async_trait]
pub trait HostForcedReservation: Send + Sync {
    fn identity(&self) -> &HostIdentity;
    fn operation(&self) -> Operation;
    fn cancellation(&self) -> Arc<RouteCancellation>;
    fn route(&self) -> ReservedForcedRoute;
    /// Uses only the original request and deadline retained by reserve; never reacquires.
    async fn begin(&self, route: PreparedRoute) -> Result<Box<dyn HostForcedLease>, HostRefusal>;
    async fn abort(&self) -> Result<(), HostRefusal>;
    /// Actual post-supervision actor settlement, including a reservation without a route.
    async fn settle(&self) -> Result<(), HostRefusal>;
}

#[async_trait]
pub trait HostForcedLease: Send + Sync {
    fn token(&self) -> &str;
    fn cancellation(&self) -> Arc<RouteCancellation>;
    async fn commit(&self, intent: ConfirmedIntent) -> Result<SubmitReceipt, HostRefusal>;
    /// `receipt_identity` is the ORIGINAL request.idempotency_key, never decision_id alone.
    async fn finish_delivery(
        &self,
        receipt_identity: &str,
        marker: ConfirmedPostcommit,
    ) -> Result<Vec<DecisionRequest>, HostRefusal>;
    async fn abort(&self) -> Result<(), HostRefusal>;
    async fn settle(&self) -> Result<(), HostRefusal>;
}

#[cfg(test)]
pub(crate) fn test_reserved_owner(operation_id: &str) -> ForcedReservationOwner {
    test_reserved_owner_with_allowance(operation_id, std::time::Duration::from_secs(5))
}

#[cfg(test)]
pub(crate) fn test_reserved_owner_with_allowance(
    operation_id: &str,
    duration: std::time::Duration,
) -> ForcedReservationOwner {
    let allowance = Allowance::required(Some(tokio::time::Instant::now() + duration)).unwrap();
    let operation = Operation::new(operation_id.into(), 1, allowance).unwrap();
    ForcedReservationOwner::new(
        HostIdentity {
            seat: crate::toy::AXIS,
            epoch: 7,
            decision_id: "test-decision".into(),
            revision: 1,
            operation_id: operation_id.into(),
        },
        operation,
    )
    .unwrap()
}

#[cfg(test)]
mod settlement_tests {
    use super::*;

    fn prepared(owner: &ForcedReservationOwner) -> PreparedRoute {
        let view = owner.route();
        view.claim_stream("test-stream").unwrap();
        view.prepare("test-stream", 0).unwrap()
    }

    struct Delivery;
    #[async_trait]
    impl HostDeliveryRoute for Delivery {
        async fn append(&self, _: &str, ordinal: u64, _: &UnconfirmedEntry) -> Result<u64, String> {
            Ok(ordinal)
        }
    }

    #[test]
    fn reserved_without_route_has_actor_only_idempotent_terminal_ack() {
        let owner = test_reserved_owner("reserved-no-route");
        let view = owner.route();
        let handle = view.cancellation();
        owner
            .register_writer_wake(
                std::thread::current(),
                std::mem::size_of::<std::thread::Thread>(),
            )
            .unwrap();
        assert!(matches!(
            *owner.inner.state.lock().unwrap(),
            RouteState::Reserved
        ));
        handle.request();
        assert_eq!(
            owner.acknowledge_settlement(&handle, false),
            Err(HostRefusal::IdentityChanged)
        );
        handle.request_settlement_after_supervision();
        let ack = owner.acknowledge_settlement(&handle, false).unwrap();
        assert_eq!(owner.acknowledge_settlement(&handle, false).unwrap(), ack);
        assert!(matches!(
            *owner.inner.state.lock().unwrap(),
            RouteState::Released
        ));
        assert_eq!(
            view.claim_stream("late-stream"),
            Err(HostRefusal::StoppedBeforeWork)
        );
        assert!(
            owner.operation().check().is_ok(),
            "actor ACK is not a native or Operation join"
        );
    }

    #[test]
    fn reserved_view_binds_only_one_sink_on_same_cancellation() {
        let owner = test_reserved_owner("one-reserved-route");
        let view = owner.route();
        view.claim_stream("first").unwrap();
        assert_eq!(
            view.claim_stream("second"),
            Err(HostRefusal::IdentityChanged)
        );
        let route = view.prepare("first", 9).unwrap();
        assert!(Arc::ptr_eq(&route.cancellation(), &view.cancellation()));
        assert_eq!(route.boundary(), 9);
        assert!(view.prepare("first", 9).is_err());
    }

    #[tokio::test]
    async fn lost_begin_reply_retains_tokenless_actual_settlement_ack() {
        let owner = test_reserved_owner("same-op");
        let route = prepared(&owner);
        let handle = route.cancellation(); // retained BEFORE install/possible begin reply
        owner
            .register_writer_wake(
                std::thread::current(),
                std::mem::size_of::<std::thread::Thread>(),
            )
            .unwrap();
        route
            .install("private-token".into(), Arc::new(Delivery))
            .unwrap();
        handle.request(); // begin reply lost/timeout: abort ONLY, no ACK or route release
        assert!(handle.acknowledged_settlement().is_none());
        assert!(!handle.settlement_requested());
        assert!(matches!(
            *route.state.lock().unwrap(),
            RouteState::Installed { .. }
        ));
        assert_eq!(
            owner.acknowledge_settlement(&handle, false),
            Err(HostRefusal::IdentityChanged)
        );
        // Test's original work has now actually settled; this explicit phase is required.
        handle.request_settlement_after_supervision();
        let ack = owner.acknowledge_settlement(&handle, false).unwrap();
        assert_eq!(handle.settlement_ack().await, ack);
        assert_eq!(owner.acknowledge_settlement(&handle, false).unwrap(), ack);
        assert_eq!(
            owner.acknowledge_settlement(&handle, true),
            Err(HostRefusal::IdentityChanged)
        );
        assert!(matches!(*route.state.lock().unwrap(), RouteState::Released));
        assert!(
            route.operation().check().is_ok(),
            "actor ACK is not an Operation/child join"
        );
    }

    #[test]
    fn same_text_identity_is_not_the_same_capability_and_registration_keeps_fixed_thread_only() {
        let owner = test_reserved_owner("same-text");
        let route = prepared(&owner);
        let other = test_reserved_owner("same-text");
        let handle = route.cancellation();
        handle.request(); // request BEFORE registration must not lose the writer wake
        owner
            .register_writer_wake(
                std::thread::current(),
                std::mem::size_of::<std::thread::Thread>(),
            )
            .unwrap();
        assert_eq!(
            handle
                .settlement
                .lock()
                .unwrap()
                .writer_wake
                .as_ref()
                .unwrap()
                .id(),
            std::thread::current().id()
        );
        assert!(std::mem::size_of::<std::thread::Thread>() * 4 <= crate::operation::PREPARE_BYTES);
        assert_eq!(
            owner.register_writer_wake(std::thread::current(), 8),
            Err(HostRefusal::IdentityChanged)
        );
        handle.request_settlement_after_supervision();
        let stale = other.cancellation();
        stale.request_settlement_after_supervision();
        assert_eq!(
            owner.acknowledge_settlement(&stale, false),
            Err(HostRefusal::IdentityChanged)
        );
        assert!(handle.acknowledged_settlement().is_none());
        owner.acknowledge_settlement(&handle, true).unwrap();
        assert!(handle.acknowledged_settlement().unwrap().uncertain());
    }

    #[tokio::test]
    async fn overflow_before_enqueue_refuses_effect_but_keeps_terminal_wake_and_ack() {
        let owner = test_reserved_owner("pre-enqueue-overflow");
        let route = prepared(&owner);
        let handle = route.cancellation();
        let operation = route.operation();
        let remaining = crate::operation::OPERATION_BYTES - operation.captured_bytes();
        operation.charge(remaining).unwrap();
        assert_eq!(
            operation.charge(1),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        let before = operation.captured_bytes();
        assert_eq!(
            owner.register_writer_wake(
                std::thread::current(),
                std::mem::size_of::<std::thread::Thread>()
            ),
            Err(HostRefusal::CaptureOverflow)
        );
        assert_eq!(
            operation.captured_bytes(),
            before,
            "cleanup metadata was precharged"
        );
        assert!(handle.requested());
        assert!(handle.settlement.lock().unwrap().writer_wake.is_some());
        assert_eq!(
            route.install("never-delivered".into(), Arc::new(Delivery)),
            Err(HostRefusal::CaptureOverflow)
        );
        assert!(matches!(*route.state.lock().unwrap(), RouteState::Prepared));
        assert!(handle.acknowledged_settlement().is_none());
        handle.request_settlement_after_supervision();
        let ack = owner.acknowledge_settlement(&handle, false).unwrap();
        assert_eq!(handle.settlement_ack().await, ack);
        assert_eq!(owner.acknowledge_settlement(&handle, false).unwrap(), ack);
        assert_eq!(
            operation.check(),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
    }

    #[tokio::test]
    async fn oversized_metadata_refuses_before_install_and_retains_only_terminal_thread_wake() {
        let owner = test_reserved_owner("oversized-metadata");
        let route = prepared(&owner);
        let handle = route.cancellation();
        let bytes = route.operation().captured_bytes();
        assert_eq!(
            owner.register_writer_wake(std::thread::current(), crate::operation::PREPARE_BYTES + 1),
            Err(HostRefusal::CaptureOverflow)
        );
        assert_eq!(
            route.operation().captured_bytes(),
            bytes,
            "no oversized body was retained"
        );
        assert!(handle.requested());
        assert!(handle.settlement.lock().unwrap().writer_wake.is_some());
        assert_eq!(
            route.install("refused".into(), Arc::new(Delivery)),
            Err(HostRefusal::CaptureOverflow)
        );
        assert!(matches!(*route.state.lock().unwrap(), RouteState::Prepared));
        assert!(handle.acknowledged_settlement().is_none());
        handle.request_settlement_after_supervision();
        let ack = owner.acknowledge_settlement(&handle, false).unwrap();
        assert_eq!(handle.settlement_ack().await, ack);
    }

    #[tokio::test]
    async fn expired_before_registration_keeps_terminal_ack_but_never_installs() {
        let owner = test_reserved_owner_with_allowance(
            "expired-registration",
            std::time::Duration::from_millis(100),
        );
        let route = prepared(&owner);
        let handle = route.cancellation();
        tokio::time::sleep_until(route.allowance().deadline()).await;
        assert_eq!(
            owner.register_writer_wake(
                std::thread::current(),
                std::mem::size_of::<std::thread::Thread>()
            ),
            Err(HostRefusal::Expired)
        );
        assert!(handle.requested());
        assert!(handle.settlement.lock().unwrap().writer_wake.is_some());
        assert_eq!(
            route.install("never-started".into(), Arc::new(Delivery)),
            Err(HostRefusal::Expired)
        );
        assert!(matches!(*route.state.lock().unwrap(), RouteState::Prepared));
        handle.request_settlement_after_supervision();
        let ack = owner.acknowledge_settlement(&handle, false).unwrap();
        assert_eq!(handle.settlement_ack().await, ack);
    }
}
