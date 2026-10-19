//! Transcript capture (`docs/protocol.md` §3 `transcript`).
//!
//! Every CLI's stream is converted into [`cna_protocol::TranscriptEntry`] values. Drivers hand
//! them to a [`TranscriptSink`], which delivers them *in order* to a [`TranscriptStore`] — the
//! campaign server's persistence in production, [`LocalTranscript`] in tests and demos. The store
//! assigns the per-seat `tseq`, so numbering survives a CLI restart, a session resume or a reseed
//! (the sink belongs to the run, not to a CLI process).
//!
//! Accepted entries keep their FIFO and recovery mirror. Admission is synchronous and fail
//! closed under the encoded capture caps; no oversized answer is split, spilled or dropped.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::host::{
    ConfirmedIntent, ConfirmedPostcommit, HostIdentity, HostRefusal, MarkerWitness, PreparedRoute,
    ReservedForcedRoute, RouteCancellation, RouteState,
};
use crate::operation::{Allowance, Operation, OperationError};
use async_trait::async_trait;
use cna_core::ids::SeatId;
use cna_protocol::ServerMessage;
pub use cna_protocol::TranscriptEntry;
use tokio::sync::{Notify, broadcast, mpsc, oneshot};

/// RFC 3339 UTC time with millisecond precision, as the protocol's `at` field.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Where transcript entries end up.
#[async_trait]
pub trait TranscriptStore: Send + Sync + 'static {
    /// Persist one entry and return its per-seat `tseq`. Called sequentially, in capture order.
    async fn append(&self, seat: SeatId, at: String, entry: TranscriptEntry)
    -> Result<u64, String>;
}

/// A capture not yet confirmed by its store. Recovery outboxes use this local shape,
/// not the viewer protocol: numbering and alignment belong to the campaign writer.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct UnconfirmedEntry {
    pub seat: SeatId,
    pub at: String,
    pub entry: TranscriptEntry,
    /// Private capture provenance. Old outboxes remain unknown, never retagged on recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<crate::operation::CaptureOrigin>,
}

struct Capture {
    row: UnconfirmedEntry,
    bytes: usize,
    operation: Option<crate::operation::Operation>,
    ordinal: u64,
}

#[derive(Default)]
struct PendingCaptures {
    rows: std::collections::VecDeque<Arc<Capture>>,
    // mdcc865c89393: owned construction sets this ONCE, including spare.
    // No ACK/drain/settlement/rebinding can rewrite it; legacy default is None.
    rows_capacity_pin: Option<usize>,
    bytes: usize,
    legacy_overflow: bool,
    ordinal: u64,
    route_owned: bool,
    route_operation: Option<String>,
    marker_phase: u8,
    first_queue_refusal: Option<QueueSendRefusal>,
    // Actual terminal worker cause, not a formatted observation or healthy ACK.
    // Header is part of this fixed body; referenced refusal capacities still
    // need their owner bounds and complete original-F cover before adoption.
    first_worker_refusal: Option<WorkerRefusal>,
    // Owned route attempt keeps the ACTUAL original graph before any copy can
    // fail/wake/unwind. Never reset by queue drain, cancellation or settlement.
    prepare_original: Option<ReservedForcedRoute>,
}

impl PendingCaptures {
    fn owned() -> Self {
        let backing = Vec::<Arc<Capture>>::with_capacity(crate::operation::MIRROR_ENTRIES);
        let capacity = backing.capacity();
        let rows = std::collections::VecDeque::from(backing);
        // A mismatch remains unadmitted storage. Original layout/creation and
        // runtime insertion refuse it; never reserve/repair/reset the pin.
        let rows_capacity_pin = (capacity >= crate::operation::MIRROR_ENTRIES
            && rows.capacity() == capacity)
            .then_some(capacity);
        Self {
            rows,
            rows_capacity_pin,
            ..Self::default()
        }
    }

    fn owned_rows_capacity(&self) -> Option<usize> {
        self.rows_capacity_pin.filter(|capacity| {
            *capacity >= crate::operation::MIRROR_ENTRIES && self.rows.capacity() == *capacity
        })
    }
}

#[allow(dead_code)] // Canonical SOURCE custody; archive enumeration unfinished.
enum WorkerRefusal {
    Installation(String),
    Append(String),
    MarkerCopy(HostRefusal),
    Settlement(OwnedSettlementFailure, String),
}

const OWNED_SETTLEMENT_UNCERTAIN: &str = "forced route settlement uncertain";
const OWNED_SETTLEMENT_TIMED_OUT: &str = "forced route settlement timed out";
#[allow(dead_code)] // Actual-capacity controls/original live-count cover UNRUN.
const OWNED_SETTLEMENT_REFUSAL_CAPACITY: usize =
    if OWNED_SETTLEMENT_UNCERTAIN.len() > OWNED_SETTLEMENT_TIMED_OUT.len() {
        OWNED_SETTLEMENT_UNCERTAIN.len()
    } else {
        OWNED_SETTLEMENT_TIMED_OUT.len()
    };

#[derive(Clone, Copy)]
enum OwnedSettlementFailure {
    Uncertain,
    TimedOut,
}

impl OwnedSettlementFailure {
    fn body(self) -> String {
        let text = match self {
            Self::Uncertain => OWNED_SETTLEMENT_UNCERTAIN,
            Self::TimedOut => OWNED_SETTLEMENT_TIMED_OUT,
        };
        String::from(Box::<str>::from(text)) // Exact existing text, no Display.
    }
}

/// Borrowed canonical worker cause only. The actual String (including its
/// capacity) or full HostRefusal stays in the original pending cell. A view is
/// not archive coverage, publication/custody, or a worker/actor join witness.
#[allow(dead_code)]
pub(crate) enum BorrowedWorkerRefusal<'a> {
    Installation(&'a String),
    Append(&'a String),
    MarkerCopy(&'a HostRefusal),
    SettlementUncertain(&'a String),
    SettlementTimedOut(&'a String),
}

#[allow(dead_code)]
pub(crate) struct BorrowedWorkerDiagnostics<'a> {
    pub(crate) join: OwnedDeliveryJoinObservation,
    pub(crate) refusal: Option<BorrowedWorkerRefusal<'a>>,
}

pub(crate) struct WorkerRoute {
    stream: String,
    owner: HostIdentity,
    allowance: Allowance,
    state: Arc<Mutex<RouteState>>,
    changed: Arc<Notify>,
    cancel: Arc<RouteCancellation>,
    operation: Operation,
}

enum MarkerReply {
    Intent(oneshot::Sender<Result<ConfirmedIntent, String>>),
    Postcommit(oneshot::Sender<Result<ConfirmedPostcommit, String>>),
    Owned(OwnedMarkerSender),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkerKind {
    Intent,
    Postcommit,
}

impl MarkerKind {
    fn observation(self) -> &'static str {
        match self {
            Self::Intent => "intent marker unconfirmed",
            Self::Postcommit => "postcommit marker unconfirmed",
        }
    }
}

enum MarkerValue {
    Intent(ConfirmedIntent),
    Postcommit(ConfirmedPostcommit),
}

impl MarkerValue {
    fn kind(&self) -> MarkerKind {
        match self {
            Self::Intent(_) => MarkerKind::Intent,
            Self::Postcommit(_) => MarkerKind::Postcommit,
        }
    }

    fn witness(&self) -> &MarkerWitness {
        match self {
            Self::Intent(value) => &value.marker,
            Self::Postcommit(value) => &value.marker,
        }
    }
}

/// Two distinct instances are precreated: exactly the EXISTING intent and
/// postcommit replies. No per-message channel allocation or third marker.
struct OwnedMarkerReply {
    kind: MarkerKind,
    state: Mutex<OwnedMarkerState>,
    changed: Notify,
}

struct OwnedMarkerState {
    owner: Option<(Operation, Arc<RouteCancellation>, u64)>,
    value: Option<MarkerValue>,
    first_unsent: Option<MarkerValue>,
    disposition: Option<Result<(), PrepareReplyFailure>>,
    observer_error: Option<String>,
    sender_closed: bool,
    receiver_closed: bool,
    taken: bool,
}

struct OwnedMarkerSender {
    shared: Arc<OwnedMarkerReply>,
    operation: Operation,
    cancel: Arc<RouteCancellation>,
    completed: bool,
}

struct OwnedMarkerReceiver {
    shared: Arc<OwnedMarkerReply>,
    operation: Operation,
    cancel: Arc<RouteCancellation>,
}

impl OwnedMarkerReply {
    fn new(kind: MarkerKind) -> Self {
        Self {
            kind,
            state: Mutex::new(OwnedMarkerState {
                owner: None,
                value: None,
                first_unsent: None,
                disposition: None,
                observer_error: Some(String::from(Box::<str>::from(kind.observation()))),
                sender_closed: false,
                receiver_closed: false,
                taken: false,
            }),
            changed: Notify::new(),
        }
    }

    fn pristine(&self) -> bool {
        self.state.try_lock().is_ok_and(|state| {
            state.owner.is_none()
                && state.value.is_none()
                && state.first_unsent.is_none()
                && state.disposition.is_none()
                && state.observer_error.as_ref().is_some_and(|text| {
                    text == self.kind.observation() && text.len() == text.capacity()
                })
                && !state.sender_closed
                && !state.receiver_closed
                && !state.taken
        })
    }

    fn observer_layout(&self) -> Result<std::alloc::Layout, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        let state = self.state.try_lock().map_err(|_| ArchiveError::Identity)?;
        let text = state
            .observer_error
            .as_ref()
            .ok_or(ArchiveError::Identity)?;
        if text != self.kind.observation() || text.capacity() != text.len() {
            return Err(ArchiveError::Identity);
        }
        Ok(std::alloc::Layout::for_value(text.as_bytes()).pad_to_align())
    }

    fn claim(
        shared: &Arc<Self>,
        operation: &Operation,
        cancel: &Arc<RouteCancellation>,
        ordinal: u64,
    ) -> Result<(OwnedMarkerSender, OwnedMarkerReceiver), PrepareReplyFailure> {
        let mut state = match shared.state.try_lock() {
            Ok(state) => state,
            Err(std::sync::TryLockError::WouldBlock) => return Err(PrepareReplyFailure::Busy),
            Err(std::sync::TryLockError::Poisoned(_)) => return Err(PrepareReplyFailure::Poisoned),
        };
        if state.owner.is_some()
            || state.value.is_some()
            || state.first_unsent.is_some()
            || state.disposition.is_some()
            || state.observer_error.is_none()
            || state.sender_closed
            || state.receiver_closed
            || state.taken
        {
            return Err(PrepareReplyFailure::Occupied);
        }
        state.owner = Some((operation.clone(), cancel.clone(), ordinal));
        Ok((
            OwnedMarkerSender {
                shared: shared.clone(),
                operation: operation.clone(),
                cancel: cancel.clone(),
                completed: false,
            },
            OwnedMarkerReceiver {
                shared: shared.clone(),
                operation: operation.clone(),
                cancel: cancel.clone(),
            },
        ))
    }
}

impl OwnedMarkerState {
    fn matches_owner(&self, operation: &Operation, cancel: &Arc<RouteCancellation>) -> bool {
        self.owner
            .as_ref()
            .is_some_and(|(original, cancellation, _)| {
                original.same_instance(operation) && Arc::ptr_eq(cancellation, cancel)
            })
    }
}

impl OwnedMarkerSender {
    fn matches_prefix(&self, operation: &Operation, ordinal: u64) -> bool {
        self.operation.same_instance(operation)
            && self.shared.state.try_lock().is_ok_and(|state| {
                state.matches_owner(operation, &self.cancel)
                    && state
                        .owner
                        .as_ref()
                        .is_some_and(|(_, _, original)| *original == ordinal)
                    && state.value.is_none()
                    && state.first_unsent.is_none()
                    && state.disposition.is_none()
                    && !state.sender_closed
                    && !state.receiver_closed
                    && !state.taken
            })
    }

    fn send(
        mut self,
        value: MarkerValue,
        worker_operation: &Operation,
        worker_cancel: &Arc<RouteCancellation>,
    ) -> Result<(), Option<MarkerValue>> {
        let (mut state, poisoned) = match self.shared.state.lock() {
            Ok(state) => (state, false),
            Err(cause) => (cause.into_inner(), true), // Custody only; NEVER clear poison.
        };
        if state.value.is_some() || state.disposition.is_some() || state.taken {
            self.completed = true;
            if state.first_unsent.is_none() {
                state.first_unsent = Some(value);
                return Err(None); // First reply remains canonical and unchanged.
            }
            return Err(Some(value)); // Invalid extra remains returned, not overwritten.
        }
        let same = state.matches_owner(&self.operation, &self.cancel)
            && self.operation.same_instance(worker_operation)
            && Arc::ptr_eq(&self.cancel, worker_cancel)
            && self.shared.kind == value.kind()
            && state
                .owner
                .as_ref()
                .is_some_and(|(_, _, ordinal)| *ordinal == value.witness().ordinal);
        state.value = Some(value); // ENTIRE actual marker first MOVE before any wake.
        let disposition = if poisoned {
            Err(PrepareReplyFailure::Poisoned)
        } else if !same {
            Err(PrepareReplyFailure::Foreign)
        } else if state.receiver_closed {
            Err(PrepareReplyFailure::Lost)
        } else {
            Ok(())
        };
        state.disposition = Some(disposition);
        state.sender_closed = true;
        self.completed = true;
        drop(state);
        self.shared.changed.notify_waiters();
        disposition.map_err(|_| None)
    }
}

impl Drop for OwnedMarkerSender {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        {
            let mut state = match self.shared.state.lock() {
                Ok(state) => state,
                Err(cause) => cause.into_inner(), // Closure only, no repair/consumption.
            };
            state.sender_closed = true;
        }
        self.shared.changed.notify_waiters();
    }
}

impl OwnedMarkerReceiver {
    async fn receive(&mut self) -> Result<MarkerValue, PrepareReplyFailure> {
        loop {
            let notified = self.shared.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self
                    .shared
                    .state
                    .lock()
                    .map_err(|_| PrepareReplyFailure::Poisoned)?;
                if !state.matches_owner(&self.operation, &self.cancel) {
                    return Err(PrepareReplyFailure::Foreign);
                }
                if state.taken {
                    return Err(PrepareReplyFailure::Taken);
                }
                if let Some(disposition) = state.disposition {
                    disposition?;
                    let value = state.value.take().ok_or(PrepareReplyFailure::Lost)?;
                    state.taken = true;
                    return Ok(value);
                }
                if state.sender_closed || state.receiver_closed {
                    return Err(PrepareReplyFailure::Lost);
                }
            }
            notified.await;
        }
    }

    fn close_observation(&mut self) -> Option<String> {
        let mut state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(cause) => cause.into_inner(), // Closure/custody only; poison remains.
        };
        state.receiver_closed = true;
        state.observer_error.take() // One precreated body, no format/allocation/refund.
    }
}

impl Drop for OwnedMarkerReceiver {
    fn drop(&mut self) {
        let mut state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(cause) => cause.into_inner(),
        };
        state.receiver_closed = true;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrepareReplyFailure {
    Busy,
    Poisoned,
    Occupied,
    Foreign,
    Lost,
    Taken,
}

/// Entire prepare reply payload/synchronization body is precreated with the
/// owned inputs. No channel, node or new reply allocation is created per use.
struct OwnedPrepareReply {
    state: Mutex<OwnedPrepareReplyState>,
    changed: Notify,
}

struct OwnedPrepareReplyState {
    owner: Option<(Operation, Arc<RouteCancellation>)>,
    value: Option<Result<PreparedRoute, HostRefusal>>,
    // One fixed diagnostic destination for a refused occupied-slot completion;
    // not a second observable reply, attempt, queue, growth pool or allowance.
    first_unsent: Option<Result<PreparedRoute, HostRefusal>>,
    // Fixed observation storage, not formatting/substitution of the retained
    // original HostRefusal. Entire exact-length child participates in F.
    observer_error: Option<String>,
    disposition: Option<Result<(), PrepareReplyFailure>>,
    sender_closed: bool,
    receiver_closed: bool,
    taken: bool,
}

struct OwnedPrepareReplySender {
    shared: Arc<OwnedPrepareReply>,
    operation: Operation,
    cancel: Arc<RouteCancellation>,
    completed: bool,
}

struct OwnedPrepareReplyReceiver {
    shared: Arc<OwnedPrepareReply>,
    operation: Operation,
    cancel: Arc<RouteCancellation>,
}

enum PrepareSendFailure {
    Retained,
    LegacyUnstored(Result<PreparedRoute, String>),
    OwnedUnstored(Result<PreparedRoute, HostRefusal>),
}

enum PrepareReply {
    Legacy(oneshot::Sender<Result<PreparedRoute, String>>),
    Owned(OwnedPrepareReplySender),
}

impl PrepareReply {
    fn send_prepared(self, value: PreparedRoute) -> Result<(), PrepareSendFailure> {
        match self {
            Self::Legacy(done) => done
                .send(Ok(value))
                .map_err(PrepareSendFailure::LegacyUnstored),
            Self::Owned(done) => done.send(Ok(value)),
        }
    }

    fn refuse(self, error: HostRefusal, cancel: &Arc<RouteCancellation>) {
        match self {
            Self::Legacy(done) => {
                cancel.request(); // Preserve legacy cancellation/format/send ordering.
                let _ = done.send(Err(error.to_string()));
            }
            Self::Owned(done) => {
                let _ = done.send(Err(error)); // Raw original cause MOVE before wake/cancel.
                cancel.request();
            }
        }
    }
}

const PREPARE_REPLY_OBSERVATION: &str = "transcript prepare unconfirmed";

impl OwnedPrepareReply {
    fn new() -> Self {
        Self {
            state: Mutex::new(OwnedPrepareReplyState {
                owner: None,
                value: None,
                first_unsent: None,
                observer_error: Some(String::from(Box::<str>::from(PREPARE_REPLY_OBSERVATION))),
                disposition: None,
                sender_closed: false,
                receiver_closed: false,
                taken: false,
            }),
            changed: Notify::new(),
        }
    }

    fn pristine(&self) -> bool {
        self.state.try_lock().is_ok_and(|state| {
            state.owner.is_none()
                && state.value.is_none()
                && state.first_unsent.is_none()
                && state.disposition.is_none()
                && state.observer_error.as_ref().is_some_and(|text| {
                    text == PREPARE_REPLY_OBSERVATION && text.capacity() == text.len()
                })
                && !state.sender_closed
                && !state.receiver_closed
                && !state.taken
        })
    }

    fn observer_layout(&self) -> Result<std::alloc::Layout, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        let state = self.state.try_lock().map_err(|_| ArchiveError::Identity)?;
        let text = state
            .observer_error
            .as_ref()
            .ok_or(ArchiveError::Identity)?;
        if text != PREPARE_REPLY_OBSERVATION || text.capacity() != text.len() {
            return Err(ArchiveError::Identity);
        }
        Ok(std::alloc::Layout::for_value(text.as_bytes()).pad_to_align())
    }

    fn claim(
        shared: &Arc<Self>,
        operation: &Operation,
        cancel: &Arc<RouteCancellation>,
    ) -> Result<(OwnedPrepareReplySender, OwnedPrepareReplyReceiver), PrepareReplyFailure> {
        let mut state = match shared.state.try_lock() {
            Ok(state) => state,
            Err(std::sync::TryLockError::WouldBlock) => return Err(PrepareReplyFailure::Busy),
            Err(std::sync::TryLockError::Poisoned(_)) => return Err(PrepareReplyFailure::Poisoned),
        };
        if state.owner.is_some()
            || state.value.is_some()
            || state.first_unsent.is_some()
            || state.disposition.is_some()
            || state.sender_closed
            || state.receiver_closed
            || state.taken
            || state.observer_error.is_none()
        {
            return Err(PrepareReplyFailure::Occupied);
        }
        state.owner = Some((operation.clone(), cancel.clone())); // Sticky original, no reset.
        Ok((
            OwnedPrepareReplySender {
                shared: shared.clone(),
                operation: operation.clone(),
                cancel: cancel.clone(),
                completed: false,
            },
            OwnedPrepareReplyReceiver {
                shared: shared.clone(),
                operation: operation.clone(),
                cancel: cancel.clone(),
            },
        ))
    }
}

impl OwnedPrepareReplyState {
    fn matches_owner(&self, operation: &Operation, cancel: &Arc<RouteCancellation>) -> bool {
        self.owner.as_ref().is_some_and(|(original, cancellation)| {
            original.same_instance(operation) && Arc::ptr_eq(cancellation, cancel)
        })
    }
}

impl OwnedPrepareReplySender {
    fn send(mut self, value: Result<PreparedRoute, HostRefusal>) -> Result<(), PrepareSendFailure> {
        let (mut state, poisoned) = match self.shared.state.lock() {
            Ok(state) => (state, false),
            // Custody write ONLY: intrinsic poison remains, never clear/repair
            // or admit work. Retain first actual body even under unknown state.
            Err(cause) => (cause.into_inner(), true),
        };
        if state.value.is_some() || state.disposition.is_some() || state.taken {
            self.completed = true;
            if state.first_unsent.is_none() {
                state.first_unsent = Some(value); // First reply remains untouched.
                return Err(PrepareSendFailure::Retained);
            }
            return Err(PrepareSendFailure::OwnedUnstored(value)); // Never overwrite first body.
        }
        let same = state.matches_owner(&self.operation, &self.cancel)
            && match &value {
                Ok(prepared) => {
                    prepared.operation.same_instance(&self.operation)
                        && Arc::ptr_eq(&prepared.cancel, &self.cancel)
                }
                Err(_) => true,
            };
        state.value = Some(value); // FIRST actual Result MOVE before any callback.
        let disposition = if poisoned {
            Err(PrepareReplyFailure::Poisoned)
        } else if !same {
            Err(PrepareReplyFailure::Foreign)
        } else if state.receiver_closed {
            Err(PrepareReplyFailure::Lost)
        } else {
            Ok(())
        };
        state.disposition = Some(disposition);
        state.sender_closed = true;
        self.completed = true; // A panic during wake must not wake again from Drop.
        drop(state);
        self.shared.changed.notify_waiters();
        disposition.map_err(|_| PrepareSendFailure::Retained)
    }
}

impl Drop for OwnedPrepareReplySender {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        {
            let mut state = match self.shared.state.lock() {
                Ok(state) => state,
                Err(cause) => cause.into_inner(), // Closure only; poison stays intrinsic.
            };
            state.sender_closed = true; // No invented result or second sender.
        }
        self.shared.changed.notify_waiters();
    }
}

impl OwnedPrepareReplyReceiver {
    async fn receive(&mut self) -> Result<Result<PreparedRoute, String>, PrepareReplyFailure> {
        loop {
            let notified = self.shared.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self
                    .shared
                    .state
                    .lock()
                    .map_err(|_| PrepareReplyFailure::Poisoned)?;
                if !state.matches_owner(&self.operation, &self.cancel) {
                    return Err(PrepareReplyFailure::Foreign);
                }
                if state.taken {
                    return Err(PrepareReplyFailure::Taken);
                }
                if let Some(disposition) = state.disposition {
                    disposition?; // Refused bodies stay canonical in ORIGINAL slot.
                    let value = match state.value.as_ref().ok_or(PrepareReplyFailure::Lost)? {
                        Ok(_) => {
                            let Some(Ok(prepared)) = state.value.take() else {
                                unreachable!()
                            };
                            Ok(prepared)
                        }
                        Err(_) => {
                            // Actual HostRefusal stays canonical, untouched even if
                            // G/deadline is exhausted. Only fixed observation MOVEs.
                            Err(state
                                .observer_error
                                .take()
                                .ok_or(PrepareReplyFailure::Lost)?)
                        }
                    };
                    state.taken = true;
                    return Ok(value); // Single MOVE, no clone/format/refund/renewal.
                }
                if state.sender_closed || state.receiver_closed {
                    return Err(PrepareReplyFailure::Lost);
                }
            } // No metadata guard crosses await or observer callback.
            notified.await;
        }
    }
}

impl Drop for OwnedPrepareReplyReceiver {
    fn drop(&mut self) {
        let mut state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(cause) => cause.into_inner(), // Closure only; never consume a poisoned body.
        };
        state.receiver_closed = true;
        // No result drop, replacement reply, credit return or callback here.
    }
}

enum Item {
    Entry(Arc<Capture>),
    Flush(oneshot::Sender<()>),
    Prepare {
        reserved: ReservedForcedRoute,
        done: PrepareReply,
    },
    Marker {
        ordinal: u64,
        reply: MarkerReply,
    },
}

// Private retained queue uses the EXISTING mirror-entry bound, including its
// control messages. Saturation is sticky failure, never wait/retry/new credit.
const OWNED_DELIVERY_ITEMS: usize = crate::operation::MIRROR_ENTRIES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueueFailure {
    Full,
    Closed,
    Busy,
    Poisoned,
    Invariant,
}

struct QueueSendRefusal {
    kind: QueueFailure,
    // Original unsent message stays canonical, including its body/ACK sender.
    _item: Item,
}

/// Each retained queue's entire slot/metadata body is one fixed allocation.
/// No per-send queue node, capacity permit, growable buffer or new pool exists.
/// Referenced Item bodies and reply-channel allocations remain separate costs.
struct OwnedQueue<const CAPACITY: usize = OWNED_DELIVERY_ITEMS> {
    state: Mutex<OwnedQueueState<CAPACITY>>,
    available: Notify,
    senders: AtomicUsize,
    receiver_closed: AtomicBool,
}

struct OwnedQueueState<const CAPACITY: usize> {
    slots: [Option<Item>; CAPACITY],
    head: usize,
    length: usize,
    used: bool,
}

impl<const CAPACITY: usize> OwnedQueueState<CAPACITY> {
    fn take_next(&mut self) -> Option<Item> {
        if self.length == 0 {
            return None;
        }
        let item = self.slots[self.head]
            .take()
            .expect("original owned queue head");
        self.head = (self.head + 1) % CAPACITY;
        self.length -= 1;
        Some(item)
    }
}

struct OwnedSender<const CAPACITY: usize = OWNED_DELIVERY_ITEMS> {
    shared: Arc<OwnedQueue<CAPACITY>>,
}

struct OwnedReceiver<const CAPACITY: usize = OWNED_DELIVERY_ITEMS> {
    shared: Arc<OwnedQueue<CAPACITY>>,
}

fn owned_transcript_queue<const CAPACITY: usize>()
-> (OwnedSender<CAPACITY>, OwnedReceiver<CAPACITY>) {
    assert!(
        CAPACITY > 0,
        "owned transcript queue has fixed nonzero slots"
    );
    let shared = Arc::new(OwnedQueue {
        state: Mutex::new(OwnedQueueState {
            slots: [const { None }; CAPACITY],
            head: 0,
            length: 0,
            used: false,
        }),
        available: Notify::new(),
        senders: AtomicUsize::new(1),
        receiver_closed: AtomicBool::new(false),
    });
    (
        OwnedSender {
            shared: shared.clone(),
        },
        OwnedReceiver { shared },
    )
}

impl<const CAPACITY: usize> Clone for OwnedSender<CAPACITY> {
    fn clone(&self) -> Self {
        self.shared
            .senders
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .expect("owned transcript sender count");
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl<const CAPACITY: usize> Drop for OwnedSender<CAPACITY> {
    fn drop(&mut self) {
        if self.shared.senders.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.shared.available.notify_waiters(); // No metadata lock spans wake.
        }
    }
}

impl<const CAPACITY: usize> OwnedSender<CAPACITY> {
    const fn max_capacity(&self) -> usize {
        CAPACITY
    }

    fn capacity(&self) -> usize {
        self.shared
            .state
            .try_lock()
            .ok()
            .and_then(|state| CAPACITY.checked_sub(state.length))
            .unwrap_or(0)
    }

    fn is_closed(&self) -> bool {
        // Closed OR unknown admission, not a physical join/closure witness.
        self.shared.receiver_closed.load(Ordering::Acquire) || self.shared.state.try_lock().is_err()
    }

    fn same_channel(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    fn pristine(&self) -> bool {
        !self.shared.receiver_closed.load(Ordering::Acquire)
            && self
                .shared
                .state
                .try_lock()
                .is_ok_and(|state| !state.used && state.head == 0 && state.length == 0)
    }

    fn try_send(&self, item: Item) -> Result<(), QueueSendRefusal> {
        let refuse = |kind, item| QueueSendRefusal { kind, _item: item };
        if self.shared.receiver_closed.load(Ordering::Acquire) {
            return Err(refuse(QueueFailure::Closed, item));
        }
        let mut state = match self.shared.state.try_lock() {
            Ok(state) => state,
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(refuse(QueueFailure::Busy, item));
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(refuse(QueueFailure::Poisoned, item));
            }
        };
        if self.shared.receiver_closed.load(Ordering::Acquire) {
            return Err(refuse(QueueFailure::Closed, item));
        }
        if state.length > CAPACITY || state.head >= CAPACITY {
            return Err(refuse(QueueFailure::Invariant, item));
        }
        if state.length == CAPACITY {
            return Err(refuse(QueueFailure::Full, item));
        }
        let Some(position) = state.head.checked_add(state.length).map(|n| n % CAPACITY) else {
            return Err(refuse(QueueFailure::Invariant, item));
        };
        if state.slots[position].is_some() {
            return Err(refuse(QueueFailure::Invariant, item));
        }
        state.slots[position] = Some(item); // Canonical MOVE before notification.
        state.length += 1;
        state.used = true; // Dequeue never makes prior use a fresh creation input.
        drop(state);
        // Caller wakes only AFTER its capture/marker exclusion is released.
        Ok(())
    }
}

impl<const CAPACITY: usize> OwnedReceiver<CAPACITY> {
    const fn max_capacity(&self) -> usize {
        CAPACITY
    }

    fn is_empty(&self) -> bool {
        self.shared
            .state
            .try_lock()
            .is_ok_and(|state| state.length == 0)
    }

    fn close(&mut self) {
        self.shared.receiver_closed.store(true, Ordering::Release);
        self.shared.available.notify_waiters();
        // Keep ALL queued items canonical; close is not discard/refund/ACK.
    }

    #[cfg(test)]
    fn try_recv(&mut self) -> Result<Item, mpsc::error::TryRecvError> {
        let mut state = self
            .shared
            .state
            .try_lock()
            .map_err(|_| mpsc::error::TryRecvError::Empty)?;
        if let Some(item) = state.take_next() {
            return Ok(item);
        }
        if self.shared.receiver_closed.load(Ordering::Acquire)
            || self.shared.senders.load(Ordering::Acquire) == 0
        {
            Err(mpsc::error::TryRecvError::Disconnected)
        } else {
            Err(mpsc::error::TryRecvError::Empty)
        }
    }

    async fn recv(&mut self) -> Option<Item> {
        loop {
            let notified = self.shared.available.notified();
            tokio::pin!(notified);
            notified.as_mut().enable(); // Register BEFORE condition inspection.
            {
                let mut state = match self.shared.state.lock() {
                    Ok(state) => state,
                    Err(_) => return None, // Poison retains every original slot.
                };
                if let Some(item) = state.take_next() {
                    return Some(item);
                }
                if self.shared.receiver_closed.load(Ordering::Acquire)
                    || self.shared.senders.load(Ordering::Acquire) == 0
                {
                    return None;
                }
            } // No mutex/Item borrow crosses await or observer wake.
            notified.await;
        }
    }
}

impl<const CAPACITY: usize> Drop for OwnedReceiver<CAPACITY> {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Clone)]
enum TranscriptSender {
    Legacy(mpsc::UnboundedSender<Item>),
    Owned(OwnedSender),
}

impl TranscriptSender {
    fn owned(&self) -> bool {
        matches!(self, Self::Owned(_))
    }
    fn maximum(&self) -> Option<usize> {
        match self {
            Self::Legacy(_) => None,
            Self::Owned(tx) => Some(tx.max_capacity()),
        }
    }
    fn is_closed(&self) -> bool {
        match self {
            Self::Legacy(tx) => tx.is_closed(),
            Self::Owned(tx) => tx.is_closed(),
        }
    }
    fn same_channel(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Legacy(a), Self::Legacy(b)) => a.same_channel(b),
            (Self::Owned(a), Self::Owned(b)) => a.same_channel(b),
            _ => false,
        }
    }
    fn send(&self, item: Item) -> Result<(), QueueSendRefusal> {
        let result = self.send_deferred(item);
        if result.is_ok() {
            self.wake_owned();
        }
        result
    }

    fn wake_owned(&self) {
        if let Self::Owned(tx) = self {
            tx.shared.available.notify_one();
        }
    }

    fn send_deferred(&self, item: Item) -> Result<(), QueueSendRefusal> {
        match self {
            Self::Legacy(tx) => tx.send(item).map_err(|error| QueueSendRefusal {
                kind: QueueFailure::Closed,
                _item: error.0,
            }),
            Self::Owned(tx) => tx.try_send(item),
        }
    }
}

enum TranscriptReceiver {
    Legacy(mpsc::UnboundedReceiver<Item>),
    Owned(OwnedReceiver),
}

impl TranscriptReceiver {
    fn maximum(&self) -> Option<usize> {
        match self {
            Self::Legacy(_) => None,
            Self::Owned(rx) => Some(rx.max_capacity()),
        }
    }
    async fn recv(&mut self) -> Option<Item> {
        match self {
            Self::Legacy(rx) => rx.recv().await,
            Self::Owned(rx) => rx.recv().await,
        }
    }
}

fn retain_queue_refusal(pending: &mut PendingCaptures, refusal: QueueSendRefusal) {
    if pending.first_queue_refusal.is_none() {
        pending.first_queue_refusal = Some(refusal); // FIRST MOVE before overflow/wake.
    }
    pending.legacy_overflow = true;
}

/// Terminal owned-worker path only. First actual body MOVE precedes its outer
/// cancellation callback. Poison permits custody/closure, never repair or ACK.
/// An occupied slot returns the extra body intact to remain caller-owned across
/// cancellation; it never replaces the first cause or creates another queue.
fn retain_worker_refusal(
    pending: &Mutex<PendingCaptures>,
    refusal: WorkerRefusal,
) -> Option<WorkerRefusal> {
    let mut pending = match pending.lock() {
        Ok(pending) => pending,
        Err(poisoned) => poisoned.into_inner(),
    };
    pending.legacy_overflow = true; // Sticky stop, not permission to retry.
    if pending.first_worker_refusal.is_some() {
        return Some(refusal);
    }
    pending.first_worker_refusal = Some(refusal);
    None // Mutex guard drops before any caller wake/cancel/unwind boundary.
}

/// Private original prepare lane uses the EXISTING capacity of one. Its entire
/// slot, exclusion and wake metadata are concrete bodies, not Tokio headers.
#[derive(Clone)]
enum PrepareSender {
    Legacy(mpsc::Sender<Item>),
    Owned(OwnedSender<1>),
}

impl PrepareSender {
    fn max_capacity(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.max_capacity(),
            Self::Owned(tx) => tx.max_capacity(),
        }
    }

    fn capacity(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.capacity(),
            Self::Owned(tx) => tx.capacity(),
        }
    }

    fn is_closed(&self) -> bool {
        match self {
            Self::Legacy(tx) => tx.is_closed(),
            Self::Owned(tx) => tx.is_closed(),
        }
    }

    fn same_channel(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Legacy(a), Self::Legacy(b)) => a.same_channel(b),
            (Self::Owned(a), Self::Owned(b)) => a.same_channel(b),
            _ => false,
        }
    }

    fn try_send_deferred(&self, item: Item) -> Result<(), QueueSendRefusal> {
        match self {
            Self::Legacy(tx) => tx.try_send(item).map_err(|error| match error {
                mpsc::error::TrySendError::Full(item) => QueueSendRefusal {
                    kind: QueueFailure::Full,
                    _item: item,
                },
                mpsc::error::TrySendError::Closed(item) => QueueSendRefusal {
                    kind: QueueFailure::Closed,
                    _item: item,
                },
            }),
            Self::Owned(tx) => tx.try_send(item),
        }
    }

    fn wake_owned(&self) {
        if let Self::Owned(tx) = self {
            tx.shared.available.notify_one();
        }
    }
}

enum PrepareReceiver {
    Legacy(mpsc::Receiver<Item>),
    Owned(OwnedReceiver<1>),
}

impl PrepareReceiver {
    fn max_capacity(&self) -> usize {
        match self {
            Self::Legacy(rx) => rx.max_capacity(),
            Self::Owned(rx) => rx.max_capacity(),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Legacy(rx) => rx.is_empty(),
            Self::Owned(rx) => rx.is_empty(),
        }
    }

    async fn recv(&mut self) -> Option<Item> {
        match self {
            Self::Legacy(rx) => rx.recv().await,
            Self::Owned(rx) => rx.recv().await,
        }
    }
}

struct DeliveryTask(tokio::sync::Mutex<DeliveryOwner>);

struct DeliveryOwner {
    task: Option<tokio::task::JoinHandle<()>>,
    // SAME owner mutex serializes task observation and fixed class publication.
    // No payload lock/callback/second slot after the terminal result arrives.
    join: OwnedDeliveryJoin,
}

impl DeliveryOwner {
    fn new(task: Option<tokio::task::JoinHandle<()>>) -> Self {
        Self {
            task,
            join: OwnedDeliveryJoin::default(),
        }
    }
}

// Keep existing private legacy handle operations; class is never removed by
// task.take(). Public signatures remain unchanged.
impl std::ops::Deref for DeliveryOwner {
    type Target = Option<tokio::task::JoinHandle<()>>;
    fn deref(&self) -> &Self::Target {
        &self.task
    }
}
impl std::ops::DerefMut for DeliveryOwner {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.task
    }
}

#[derive(Default)]
struct OwnedDeliveryJoin {
    original: Option<Operation>,
    // md15b08b71f9e: TYPE cannot own JoinError/Any. Raw panic is dropped at
    // the join AFTER the fixed class is recorded; default panic hook preserved.
    first_observation: Option<OwnedDeliveryJoinObservation>,
}

/// Physical completion class ONLY. Completed is not healthy delivery/actor ACK.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnedDeliveryJoinObservation {
    Completed,
    Cancelled,
    Panicked,
}

impl OwnedDeliveryJoin {
    fn pristine(&self) -> bool {
        self.original.is_none() && self.first_observation.is_none()
    }

    fn observation(&self) -> Option<OwnedDeliveryJoinObservation> {
        self.first_observation
    }
}

const WORKER_PANIC_REASON: &str = "transcript delivery worker panicked; see server log";

fn join_observation(result: &Result<(), tokio::task::JoinError>) -> OwnedDeliveryJoinObservation {
    match result {
        Ok(()) => OwnedDeliveryJoinObservation::Completed,
        Err(error) if error.is_cancelled() => OwnedDeliveryJoinObservation::Cancelled,
        Err(_) => OwnedDeliveryJoinObservation::Panicked,
    }
}

/// The cloneable handle drivers write to.
#[derive(Clone)]
pub struct TranscriptSink {
    tx: TranscriptSender,
    pending: Arc<Mutex<PendingCaptures>>,
    delivery: Arc<DeliveryTask>,
    operation: Option<crate::operation::Operation>,
    stream: Arc<str>,
    prepare: PrepareSender,
    prepare_reply: Option<Arc<OwnedPrepareReply>>,
    intent_reply: Option<Arc<OwnedMarkerReply>>,
    postcommit_reply: Option<Arc<OwnedMarkerReply>>,
    source_origin: Option<Option<crate::operation::CaptureOrigin>>,
}

/// Immediate producer references only. Queue and prepare/marker reply bodies
/// are counted separately by the pending-input inventory. Flush replies,
/// task allocations, store graphs and allocator metadata are NOT covered by
/// these actual Layouts. This private
/// snapshot cannot authorize original admission.
#[allow(dead_code)]
pub(crate) struct TranscriptCreationLayout {
    pub(crate) pending: std::alloc::Layout,
    pub(crate) delivery: std::alloc::Layout,
    pub(crate) stream: std::alloc::Layout,
    pub(crate) queue: std::alloc::Layout,
    pub(crate) referenced_bytes: usize,
}

/// Own the ACTUAL store, receivers and producer BEFORE a delivery task exists.
/// No launch entry is provided until whole original admission is assembled.
#[allow(dead_code)]
pub(crate) struct PendingTranscriptInputs {
    producer: Option<TranscriptSink>,
    // HANDLE-only association clone, not captured/store/result body copying.
    // Keeps exact channel/Arc identity after producer MOVE to its driver.
    association: TranscriptSink,
    worker: Option<DeliveryInputs>,
    measurement_started: bool,
    first_layout:
        Option<Result<PendingTranscriptCreationLayout, crate::failed_evidence::ArchiveError>>,
}

/// Concrete pending/worker/store/queue/reply and individual control-frame
/// inventory, still PARTIAL: referenced children, flush reply/task metadata,
/// allocator overhead and live-instance counts remain unfinished. Success
/// cannot admit an Operation or spawn.
#[allow(dead_code)]
pub(crate) struct PendingTranscriptCreationLayout {
    pub(crate) pending: std::alloc::Layout,
    pub(crate) store_body: std::alloc::Layout,
    pub(crate) worker_frame: std::alloc::Layout,
    worker_payload: TaskPayloadLayout,
    pub(crate) producer_references: TranscriptCreationLayout,
    pub(crate) queue_elements: std::alloc::Layout,
    pub(crate) owned_queue: std::alloc::Layout,
    pub(crate) owned_prepare_queue: std::alloc::Layout,
    pub(crate) owned_prepare_reply: std::alloc::Layout,
    pub(crate) owned_prepare_observer: std::alloc::Layout,
    pub(crate) owned_marker_replies: std::alloc::Layout,
    pub(crate) intent_observer: std::alloc::Layout,
    pub(crate) postcommit_observer: std::alloc::Layout,
    pub(crate) control_frames: TranscriptControlFrameLayouts,
    pub(crate) known_bytes: usize,
}

/// One INSTANCE of each actual future/wrapper/output TYPE. These facts supply
/// no live-count bound, N, concurrency admission or total original-F proof.
/// Nested timeout/public-wrapper frames overlap; never sum this inventory as
/// independent caller allocations or infer reachable copies from its length.
#[allow(dead_code)]
pub(crate) struct ControlFutureLayout {
    pub(crate) concrete: std::alloc::Layout,
    pub(crate) timeout: std::alloc::Layout,
    pub(crate) boxed_pointer: std::alloc::Layout,
    pub(crate) erased_pointer: std::alloc::Layout,
    pub(crate) output: std::alloc::Layout,
    pub(crate) timeout_output: std::alloc::Layout,
}

#[allow(dead_code)]
pub(crate) struct TranscriptControlFrameLayouts {
    pub(crate) prepare: ControlFutureLayout,
    pub(crate) owned_prepare: ControlFutureLayout,
    pub(crate) intent: ControlFutureLayout,
    pub(crate) postcommit: ControlFutureLayout,
    pub(crate) owned_marker: ControlFutureLayout,
    pub(crate) flush: ControlFutureLayout,
    pub(crate) prepare_receive: ControlFutureLayout,
    pub(crate) marker_receive: ControlFutureLayout,
    pub(crate) owned_delivery_join: ControlFutureLayout,
    pub(crate) owned_settlement: ControlFutureLayout,
}

impl TranscriptControlFrameLayouts {
    fn derive() -> Self {
        Self {
            prepare: borrowed_argument_frame(TranscriptSink::prepare_route),
            owned_prepare: borrowed_argument_frame(TranscriptSink::prepare_owned_route),
            intent: borrowed_frame(TranscriptSink::confirm_intent),
            postcommit: borrowed_frame(TranscriptSink::confirm_postcommit),
            owned_marker: borrowed_argument_frame(TranscriptSink::confirm_owned_marker),
            flush: borrowed_frame(TranscriptSink::flush_confirmed),
            prepare_receive: borrowed_mut_frame(OwnedPrepareReplyReceiver::receive),
            marker_receive: borrowed_mut_frame(OwnedMarkerReceiver::receive),
            owned_delivery_join: borrowed_argument_frame(TranscriptSink::join_owned_delivery),
            owned_settlement: borrowed_argument_frame(WorkerRoute::settled_owned),
        }
    }
}

#[allow(dead_code)]
impl PendingTranscriptInputs {
    /// Trusted actual stream input, no generated replacement identity or seed.
    /// Channels are pending input allocations; no receiver is polled and no
    /// store method/task/native effect occurs. Legacy TranscriptSink::new keeps
    /// its existing eager behavior and does not call this constructor.
    pub(crate) fn new(store: Arc<dyn TranscriptStore>, stream: Arc<str>) -> Self {
        let (tx, rx) = owned_transcript_queue::<OWNED_DELIVERY_ITEMS>();
        let (prepare, prepare_rx) = owned_transcript_queue::<1>();
        let pending = Arc::new(Mutex::new(PendingCaptures::owned()));
        let worker = DeliveryInputs {
            store,
            rx: TranscriptReceiver::Owned(rx),
            prepare_rx: PrepareReceiver::Owned(prepare_rx),
            remaining: pending.clone(),
            worker_stream: stream.clone(),
        };
        let producer = TranscriptSink {
            tx: TranscriptSender::Owned(tx),
            pending,
            delivery: Arc::new(DeliveryTask(tokio::sync::Mutex::new(DeliveryOwner::new(
                None,
            )))),
            operation: None,
            stream,
            prepare: PrepareSender::Owned(prepare),
            prepare_reply: Some(Arc::new(OwnedPrepareReply::new())),
            intent_reply: Some(Arc::new(OwnedMarkerReply::new(MarkerKind::Intent))),
            postcommit_reply: Some(Arc::new(OwnedMarkerReply::new(MarkerKind::Postcommit))),
            source_origin: None,
        };
        Self {
            association: producer.clone(),
            producer: Some(producer),
            worker: Some(worker),
            measurement_started: false,
            first_layout: None,
        }
    }

    pub(crate) fn measure_creation(&mut self) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.measure_with_cap(crate::operation::OPERATION_BYTES)
    }

    fn measure_with_cap(&mut self, cap: usize) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        if self.measurement_started {
            return Err(ArchiveError::Identity);
        }
        self.measurement_started = true; // Unwound is unfinished, never retry.
        let result = self.derive_layout(cap);
        self.first_layout = Some(result); // First actual MOVE before fixed observation.
        if matches!(self.first_layout, Some(Ok(_))) {
            Ok(())
        } else {
            Err(ArchiveError::Identity)
        }
    }

    fn derive_layout(
        &self,
        cap: usize,
    ) -> Result<PendingTranscriptCreationLayout, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        let worker = self.worker.as_ref().ok_or(ArchiveError::Identity)?;
        let producer = self.producer.as_ref().ok_or(ArchiveError::Identity)?;
        if !self.same_producer(producer)
            || producer.stream.len() != 36
            || uuid::Uuid::parse_str(&producer.stream).is_err()
        {
            return Err(ArchiveError::Identity);
        }
        let producer_references = producer.original_referenced_layout()?;
        if producer.tx.maximum() != Some(OWNED_DELIVERY_ITEMS)
            || worker.rx.maximum() != Some(OWNED_DELIVERY_ITEMS)
            || producer.prepare.max_capacity() != 1
            || worker.prepare_rx.max_capacity() != 1
            || !matches!(&producer.tx, TranscriptSender::Owned(tx) if tx.capacity() == OWNED_DELIVERY_ITEMS && tx.pristine())
            || !matches!(&worker.rx, TranscriptReceiver::Owned(rx) if rx.is_empty())
            || producer.prepare.capacity() != 1
            || !worker.prepare_rx.is_empty()
            || !matches!((&producer.tx, &worker.rx),
                (TranscriptSender::Owned(tx), TranscriptReceiver::Owned(rx))
                    if Arc::ptr_eq(&tx.shared, &rx.shared))
            || !matches!((&producer.prepare, &worker.prepare_rx),
                (PrepareSender::Owned(tx), PrepareReceiver::Owned(rx))
                    if tx.pristine() && Arc::ptr_eq(&tx.shared, &rx.shared))
            || !producer
                .prepare_reply
                .as_ref()
                .is_some_and(|reply| reply.pristine())
            || !producer
                .intent_reply
                .as_ref()
                .is_some_and(|reply| reply.kind == MarkerKind::Intent && reply.pristine())
            || !producer
                .postcommit_reply
                .as_ref()
                .is_some_and(|reply| reply.kind == MarkerKind::Postcommit && reply.pristine())
        {
            return Err(ArchiveError::Identity);
        }
        // Diagnostic logical message layout, including capacity-one prepare.
        // Actual retained MAIN and prepare queue bodies are fixed below;
        // per-message reply/task metadata remains an explicit unfinished cost.
        let queue_elements = std::alloc::Layout::array::<Item>(
            OWNED_DELIVERY_ITEMS
                .checked_add(1)
                .ok_or(ArchiveError::Fit)?,
        )
        .map_err(|_| ArchiveError::Fit)?
        .pad_to_align();
        let owned_queue = std::alloc::Layout::new::<OwnedQueue>().pad_to_align();
        let owned_prepare_queue = std::alloc::Layout::new::<OwnedQueue<1>>().pad_to_align();
        let owned_prepare_reply = std::alloc::Layout::new::<OwnedPrepareReply>().pad_to_align();
        let owned_prepare_observer = producer
            .prepare_reply
            .as_ref()
            .ok_or(ArchiveError::Identity)?
            .observer_layout()?;
        let owned_marker_replies =
            std::alloc::Layout::new::<(OwnedMarkerReply, OwnedMarkerReply)>().pad_to_align();
        let intent_observer = producer
            .intent_reply
            .as_ref()
            .ok_or(ArchiveError::Identity)?
            .observer_layout()?;
        let postcommit_observer = producer
            .postcommit_reply
            .as_ref()
            .ok_or(ArchiveError::Identity)?
            .observer_layout()?;
        let pending = std::alloc::Layout::for_value(self).pad_to_align();
        let store_body = std::alloc::Layout::for_value(worker.store.as_ref()).pad_to_align();
        // The function item only supplies its concrete Future TYPE. Neither
        // the constructor nor the worker body is called, polled or boxed.
        let worker_payload = delivery_payload_layout(delivery_worker);
        let worker_frame = worker_payload.future;
        // Type carriers ONLY. No sink/reservation/receiver argument is made,
        // method/future constructor called, timeout created, boxed or polled.
        let control_frames = TranscriptControlFrameLayouts::derive();
        // Preserve the older diagnostic overlap margin and its source controls.
        // known_bytes is NOT the m18 task charge or a complete original F;
        // the actual task payload uses worker_payload.bytes(), ONE F + Output.
        let known_bytes = pending
            .size()
            .checked_add(store_body.size())
            .and_then(|bytes| bytes.checked_add(owned_queue.size()))
            .and_then(|bytes| bytes.checked_add(owned_prepare_queue.size()))
            .and_then(|bytes| bytes.checked_add(owned_prepare_reply.size()))
            .and_then(|bytes| bytes.checked_add(owned_prepare_observer.size()))
            .and_then(|bytes| bytes.checked_add(owned_marker_replies.size()))
            .and_then(|bytes| bytes.checked_add(intent_observer.size()))
            .and_then(|bytes| bytes.checked_add(postcommit_observer.size()))
            .and_then(|bytes| bytes.checked_add(producer_references.referenced_bytes))
            .and_then(|bytes| bytes.checked_add(worker_frame.size().checked_mul(2)?))
            .and_then(|bytes| bytes.checked_add(worker_payload.output.size()))
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<(
                    DeliveryInputs,
                    PendingTranscriptCreationLayout,
                    Result<PendingTranscriptCreationLayout, ArchiveError>,
                    TranscriptCreationLayout,
                    Result<TranscriptCreationLayout, ArchiveError>,
                    tokio::task::JoinHandle<()>,
                    Result<(), ArchiveError>,
                    &mut Self,
                    usize,
                )>())
            })
            .ok_or(ArchiveError::Fit)?;
        if known_bytes > cap {
            return Err(ArchiveError::Fit);
        }
        Ok(PendingTranscriptCreationLayout {
            pending,
            store_body,
            worker_frame,
            worker_payload,
            producer_references,
            queue_elements,
            owned_queue,
            owned_prepare_queue,
            owned_prepare_reply,
            owned_prepare_observer,
            owned_marker_replies,
            intent_observer,
            postcommit_observer,
            control_frames,
            known_bytes,
        })
    }

    pub(crate) fn creation_layout(
        &self,
    ) -> Option<&Result<PendingTranscriptCreationLayout, crate::failed_evidence::ArchiveError>>
    {
        self.first_layout.as_ref()
    }

    fn same_producer(&self, producer: &TranscriptSink) -> bool {
        let Some(worker) = self.worker.as_ref() else {
            return false;
        };
        producer.tx.same_channel(&self.association.tx)
            // Stale successful measurement cannot transfer an already-used
            // original queue into a fresh factory/binding after it was drained.
            && matches!(&producer.tx, TranscriptSender::Owned(tx) if tx.pristine())
            && matches!(&producer.prepare, PrepareSender::Owned(tx) if tx.pristine())
            && !producer.tx.is_closed()
            && !producer.prepare.is_closed()
            && producer.prepare.same_channel(&self.association.prepare)
            && matches!((&producer.prepare_reply, &self.association.prepare_reply),
                (Some(a), Some(b)) if Arc::ptr_eq(a, b) && a.pristine())
            && matches!((&producer.intent_reply, &self.association.intent_reply),
                (Some(a), Some(b)) if Arc::ptr_eq(a, b) && a.kind == MarkerKind::Intent && a.pristine())
            && matches!((&producer.postcommit_reply, &self.association.postcommit_reply),
                (Some(a), Some(b)) if Arc::ptr_eq(a, b) && a.kind == MarkerKind::Postcommit && a.pristine())
            && Arc::ptr_eq(&producer.pending, &self.association.pending)
            && Arc::ptr_eq(&producer.delivery, &self.association.delivery)
            && Arc::ptr_eq(&producer.stream, &self.association.stream)
            && Arc::ptr_eq(&worker.remaining, &producer.pending)
            && Arc::ptr_eq(&worker.worker_stream, &producer.stream)
            && producer.operation.is_none()
            && producer.source_origin.is_none()
            && producer
                .delivery
                .0
                .try_lock()
                .is_ok_and(|owner| owner.task.is_none() && owner.join.pristine())
    }

    /// Source input transfer only, never admission, host binding or task start.
    /// Validate exact association/first partial Result and empty destination
    /// before moving the producer. Worker/store/result remain in this root.
    pub(crate) fn move_producer_into(
        &mut self,
        destination: &mut Option<TranscriptSink>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        if destination.is_some()
            || !matches!(self.first_layout, Some(Ok(_)))
            || !self
                .producer
                .as_ref()
                .is_some_and(|producer| self.same_producer(producer))
        {
            return Err(ArchiveError::Identity);
        }
        *destination = self.producer.take(); // SAME producer MOVE after all checks.
        Ok(())
    }

    pub(crate) fn matches_transferred_producer(&self, producer: &TranscriptSink) -> bool {
        self.producer.is_none()
            && matches!(self.first_layout, Some(Ok(_)))
            && self.same_producer(producer)
    }

    #[cfg(test)]
    pub(crate) fn measure_with_cap_for_source_control(
        &mut self,
        cap: usize,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.measure_with_cap(cap)
    }

    #[cfg(test)]
    pub(crate) fn unstarted_for_source_control(&self) -> bool {
        self.worker.is_some()
            && self
                .association
                .delivery
                .0
                .try_lock()
                .is_ok_and(|task| task.is_none())
            && self.association.operation.is_none()
            && !self.association.tx.is_closed()
            && !self.association.prepare.is_closed()
    }
}

struct DeliveryInputs {
    store: Arc<dyn TranscriptStore>,
    rx: TranscriptReceiver,
    prepare_rx: PrepareReceiver,
    remaining: Arc<Mutex<PendingCaptures>>,
    worker_stream: Arc<str>,
}

/// SAME worker body for legacy and the pending original path. No caller can
/// infer a join/ACK from constructing this concrete unpolled future.
async fn delivery_worker(inputs: DeliveryInputs) {
    let DeliveryInputs {
        store,
        mut rx,
        mut prepare_rx,
        remaining,
        worker_stream,
    } = inputs;
    let owned = matches!(&rx, TranscriptReceiver::Owned(_));
    let mut active: Option<Arc<WorkerRoute>> = None;
    let mut acknowledged_ordinal = 0u64;
    loop {
        // Separate capacity-one prepare handshake takes priority after the ACTUAL
        // current append ACK. Older buffered entries then use the installed route.
        let item = tokio::select! {
            biased;
            Some(item) = prepare_rx.recv(), if active.is_none() => item,
            item = rx.recv() => match item { Some(item) => item, None => break },
        };
        match item {
            Item::Entry(capture) => {
                if acknowledged_ordinal.checked_add(1) != Some(capture.ordinal) {
                    if let Some(route) = &active {
                        route.cancel.request();
                    }
                    if let Some(operation) = &capture.operation {
                        operation.overflow();
                    }
                    return;
                }
                let mut delay = Duration::from_millis(50);
                loop {
                    // The owned-store API requires a full clone on each issue/retry.
                    // Charge BEFORE copying; retry/ACK never refunds cumulative bytes.
                    if let Some(operation) = &capture.operation {
                        if operation.charge(capture.bytes).is_err() {
                            return;
                        }
                    }
                    let result = if let Some(route) = &active {
                        if route.operation.check().is_err() {
                            route.cancel.request();
                            return;
                        }
                        // Older/legacy captures keep unknown provenance, but their new
                        // actor-side full-row references still charge THIS route's meter.
                        if route.operation.charge(capture.bytes).is_err() {
                            route.cancel.request();
                            return;
                        }
                        let (_, delivery) = match route.installed().await {
                            Ok(value) => value,
                            Err(error) => {
                                let _unmoved = owned.then(|| {
                                    retain_worker_refusal(
                                        &remaining,
                                        WorkerRefusal::Installation(error),
                                    )
                                });
                                route.cancel.request();
                                return;
                            }
                        };
                        // Borrow the canonical full row; original author remains unchanged.
                        delivery
                            .append(&route.stream, capture.ordinal, &capture.row)
                            .await
                    } else {
                        store
                            .append(
                                capture.row.seat,
                                capture.row.at.clone(),
                                capture.row.entry.clone(),
                            )
                            .await
                    };
                    let Err(e) = result else {
                        break;
                    };
                    if let Some(route) = &active {
                        // Lost/error ACK may hide an accepted append. Never reroute/retry it.
                        if owned {
                            let _unmoved =
                                retain_worker_refusal(&remaining, WorkerRefusal::Append(e));
                            route.cancel.request();
                            return;
                        }
                        route.cancel.request();
                        return;
                    }
                    eprintln!(
                        "transcript store refused an entry for {}: {e}; retrying",
                        capture.row.seat
                    );
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(5));
                }
                // The receipt's u64 is per-seat tseq, NOT this shared stream ordinal.
                // Only the actual successful append ACK advances this witness.
                acknowledged_ordinal = capture.ordinal;
                let mut pending = remaining.lock().expect("pending captures");
                if let Some(row) = pending.rows.pop_front() {
                    pending.bytes -= row.bytes;
                }
            }
            Item::Flush(done) => {
                let _ = done.send(());
            }
            Item::Prepare { reserved, done } => {
                let prepared = match reserved.prepare(&worker_stream, acknowledged_ordinal) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        done.refuse(error, &reserved.cancellation());
                        // No route was installed. Actor still owns reserved settlement;
                        // unknown supervision cannot be replaced by a worker ACK.
                        return;
                    }
                };
                let route = match WorkerRoute::copy_prepared(&prepared) {
                    Ok(route) => Arc::new(route),
                    Err(error) => {
                        done.refuse(error, &prepared.cancel);
                        return;
                    }
                };
                active = Some(route.clone());
                if let Err(refusal) = done.send_prepared(prepared) {
                    // Retained means the original fixed slot owns the body.
                    // Unstored is the existing lost legacy reply or an invalid
                    // second owned completion; never replace its first reply.
                    match refusal {
                        PrepareSendFailure::Retained => {}
                        PrepareSendFailure::LegacyUnstored(value) => drop(value),
                        PrepareSendFailure::OwnedUnstored(value) => {
                            // No legal original sender can reach a third completion:
                            // claim is one-shot and this sender is not cloneable.
                            // Keep an invalid extra return owned across cancellation.
                            let _unmoved = value;
                            route.cancel.request();
                            return;
                        }
                    }
                    route.cancel.request();
                    return;
                }
                // This queue boundary follows the actual normal in-flight ACK. Freeze
                // the next issue until begin installs the actor route before its ACK.
                if let Err(error) = route.installed().await {
                    // Legacy is_err() discarded this body before reading state.
                    // The owned branch keeps it until its actual disposition.
                    let error = owned.then_some(error);
                    let released = matches!(
                        *route.state.lock().expect("worker route"),
                        RouteState::Released
                    );
                    if !released {
                        let _unmoved = error.and_then(|error| {
                            retain_worker_refusal(&remaining, WorkerRefusal::Installation(error))
                        });
                        route.cancel.request();
                        return;
                    }
                    let mut pending = remaining.lock().expect("pending captures");
                    pending.route_owned = false;
                    pending.route_operation = None;
                    active = None;
                }
            }
            Item::Marker { ordinal, reply } => {
                let Some(route) = &active else {
                    return;
                };
                if ordinal != acknowledged_ordinal {
                    route.cancel.request();
                    return;
                }
                let (token, _) = match route.installed().await {
                    Ok(value) => value,
                    Err(error) => {
                        let _unmoved = owned.then(|| {
                            retain_worker_refusal(&remaining, WorkerRefusal::Installation(error))
                        });
                        route.cancel.request();
                        return;
                    }
                };
                let marker = match route.copy_marker(token, ordinal) {
                    Ok(marker) => marker,
                    Err(error) => {
                        let _unmoved = owned.then(|| {
                            retain_worker_refusal(&remaining, WorkerRefusal::MarkerCopy(error))
                        });
                        route.cancel.request();
                        return; // Closed reply, never an invented marker/ACK.
                    }
                };
                match reply {
                    MarkerReply::Intent(done) => {
                        if done.send(Ok(ConfirmedIntent { marker })).is_err() {
                            route.cancel.request();
                        }
                    }
                    MarkerReply::Postcommit(done) => {
                        if done.send(Ok(ConfirmedPostcommit { marker })).is_err() {
                            route.cancel.request();
                        }
                        // Freeze after marker TWO: scoped final pending read belongs to
                        // actor settlement. Later siblings cannot delay that command.
                        if route.settled().await.is_err() {
                            return;
                        }
                        let mut pending = remaining.lock().expect("pending captures");
                        pending.route_owned = false;
                        pending.route_operation = None;
                        active = None;
                    }
                    MarkerReply::Owned(done) => {
                        let postcommit = done.shared.kind == MarkerKind::Postcommit;
                        let value = if postcommit {
                            MarkerValue::Postcommit(ConfirmedPostcommit { marker })
                        } else {
                            MarkerValue::Intent(ConfirmedIntent { marker })
                        };
                        if let Err(unstored) = done.send(value, &route.operation, &route.cancel) {
                            // None: original slot retains first/unsent body. Some:
                            // invalid extra stays owned across original cancellation.
                            let _unmoved = unstored;
                            route.cancel.request();
                        }
                        if postcommit {
                            if route.settled_owned(&remaining).await.is_err() {
                                return;
                            }
                            let mut pending = remaining.lock().expect("pending captures");
                            pending.route_owned = false;
                            pending.route_operation = None;
                            active = None;
                        }
                    }
                }
            }
        }
    }
}

/// m18b741c8091f: ONLY the concrete future and its output are task payload.
/// Tokio Header/scheduler/id/Trailer/Stage tag-padding/allocator bookkeeping is
/// outside m67; no private cell mirror or guessed factor is used here. Heap
/// children behind captured handles still need their separate ORIGINAL F/G.
#[allow(dead_code)] // Actual-value use awaits the ONE whole creation assembly.
struct TaskPayloadLayout {
    future: std::alloc::Layout,
    output: std::alloc::Layout,
}

#[allow(dead_code)]
impl TaskPayloadLayout {
    fn for_type<F: std::future::Future>() -> Self {
        Self {
            future: std::alloc::Layout::new::<F>(),
            output: std::alloc::Layout::new::<F::Output>(),
        }
    }

    fn for_value<F: std::future::Future>(future: &F) -> Self {
        Self {
            future: std::alloc::Layout::for_value(future),
            output: std::alloc::Layout::new::<F::Output>(),
        }
    }

    fn bytes(&self) -> Result<usize, crate::failed_evidence::ArchiveError> {
        self.future
            .size()
            .checked_add(self.output.size())
            .ok_or(crate::failed_evidence::ArchiveError::Fit)
    }

    /// Validate the ACTUAL concrete value before a future spawn. This borrows
    /// without polling, boxing, moving or charging. It proves layout equality
    /// only: the complete original-F producer and same-owner provenance gate
    /// must still exist. There is currently NO admitted spawn caller.
    fn check_value<F: std::future::Future>(
        &self,
        future: &F,
    ) -> Result<usize, crate::failed_evidence::ArchiveError> {
        let actual = Self::for_value(future);
        if self.future != actual.future || self.output != actual.output {
            return Err(crate::failed_evidence::ArchiveError::Identity);
        }
        // Same F layout ONCE even when Tokio separately boxes a large future.
        // JoinHandle's pointer is already in its actual op-owned struct.
        actual.bytes()
    }
}

/// Infer the concrete worker Future and Output TYPES WITHOUT constructing them.
/// This remains a prospective inventory; check_value on THAT actual future and
/// complete original admission must precede any eventual spawn.
fn delivery_payload_layout<F>(_: impl FnOnce(DeliveryInputs) -> F) -> TaskPayloadLayout
where
    F: std::future::Future<Output = ()> + Send,
{
    TaskPayloadLayout::for_type::<F>()
}

#[cfg(test)]
mod task_payload_controls {
    use super::TaskPayloadLayout;
    use crate::failed_evidence::ArchiveError;
    use std::alloc::Layout;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};

    struct NeverPolled {
        captured: [u8; 32769],
        polls: Arc<AtomicUsize>,
    }

    impl Future for NeverPolled {
        type Output = [u8; 17];

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            panic!("task payload inspection must not poll");
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. No runtime, task/spawn, Box or Operation.
    // The big concrete fixture exceeds BOTH pinned Tokio boxing thresholds;
    // this checks accounting only, not an observed Tokio allocation branch.
    #[test]
    fn actual_large_future_and_output_count_once_without_polling_or_boxing() {
        let polls = Arc::new(AtomicUsize::new(0));
        let future = NeverPolled {
            captured: [7; 32769],
            polls: polls.clone(),
        };
        assert!(future.captured.len() > 16384);
        let prospective = TaskPayloadLayout::for_type::<NeverPolled>();
        let actual = TaskPayloadLayout::for_value(&future);
        assert_eq!(actual.future, Layout::for_value(&future));
        assert_eq!(actual.output, Layout::new::<[u8; 17]>());
        let expected = Layout::for_value(&future).size() + Layout::new::<[u8; 17]>().size();
        assert_eq!(prospective.check_value(&future).unwrap(), expected);
        assert_eq!(actual.bytes().unwrap(), expected);
        assert!(expected < actual.future.size() * 2 + actual.output.size());
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        assert!(future.captured.iter().all(|byte| *byte == 7));

        let empty_output = std::future::ready(());
        let unit = TaskPayloadLayout::for_value(&empty_output);
        assert_eq!(unit.output, Layout::new::<()>());
        assert_eq!(
            unit.bytes().unwrap(),
            Layout::for_value(&empty_output).size()
        );
    }

    #[test]
    fn mismatched_future_or_output_layout_refuses_without_polling() {
        let polls = Arc::new(AtomicUsize::new(0));
        let future = NeverPolled {
            captured: [7; 32769],
            polls: polls.clone(),
        };
        for change_output in [false, true] {
            let mut prospective = TaskPayloadLayout::for_type::<NeverPolled>();
            if change_output {
                prospective.output = Layout::new::<u8>();
            } else {
                prospective.future = Layout::new::<u8>();
            }
            assert!(matches!(
                prospective.check_value(&future),
                Err(ArchiveError::Identity)
            ));
            assert_eq!(polls.load(Ordering::SeqCst), 0);
            assert!(future.captured.iter().all(|byte| *byte == 7));
        }
    }
}

fn control_future_layout<'a, F>() -> ControlFutureLayout
where
    F: std::future::Future + Send + 'a,
{
    ControlFutureLayout {
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

fn borrowed_frame<'a, T: 'a, F>(_: impl FnOnce(&'a T) -> F) -> ControlFutureLayout
where
    F: std::future::Future + Send + 'a,
{
    control_future_layout::<F>()
}

fn borrowed_argument_frame<'a, T: 'a, A, F>(_: impl FnOnce(&'a T, A) -> F) -> ControlFutureLayout
where
    F: std::future::Future + Send + 'a,
{
    control_future_layout::<F>()
}

fn borrowed_mut_frame<'a, T: 'a, F>(_: impl FnOnce(&'a mut T) -> F) -> ControlFutureLayout
where
    F: std::future::Future + Send + 'a,
{
    control_future_layout::<F>()
}

impl WorkerRoute {
    /// Owned postcommit wait preserves its original failure BEFORE timeout
    /// cancellation can wake/unwind. Original F must cover the named literal
    /// capacity and actual simultaneous cells/sites; no new G reserve/refund.
    /// Legacy settled() keeps its exact original return/callback behavior.
    async fn settled_owned(&self, remaining: &Mutex<PendingCaptures>) -> Result<(), ()> {
        {
            let pending = remaining.lock().map_err(|_| ())?;
            let original = pending.prepare_original.as_ref().ok_or(())?;
            if !original.operation().same_instance(&self.operation)
                || !Arc::ptr_eq(&original.cancellation(), &self.cancel)
                || self.allowance.deadline() != self.operation.allowance().deadline()
                || pending.first_worker_refusal.is_some()
            {
                return Err(()); // No foreign wait/cancel or erased first cause.
            }
        }
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let uncertain = {
                let state = self.state.lock().expect("worker route");
                match &*state {
                    RouteState::Released => return Ok(()), // ACTUAL actor state.
                    RouteState::Uncertain => true,
                    _ => false,
                }
            }; // No route/pending guard around body creation or cancellation.
            if uncertain {
                let failure = OwnedSettlementFailure::Uncertain;
                let _unmoved = retain_worker_refusal(
                    remaining,
                    WorkerRefusal::Settlement(failure, failure.body()),
                );
                return Err(()); // Fixed observation; raw cause stays original.
            }
            if tokio::time::timeout_at(self.allowance.deadline(), changed)
                .await
                .is_err()
            {
                let failure = OwnedSettlementFailure::TimedOut;
                let _unmoved = retain_worker_refusal(
                    remaining,
                    WorkerRefusal::Settlement(failure, failure.body()),
                );
                self.cancel.request(); // AFTER raw first MOVE and guard release.
                return Err(()); // Timeout never manufactures settled/healthy ACK.
            }
        }
    }

    fn copy_prepared(prepared: &PreparedRoute) -> Result<Self, crate::host::HostRefusal> {
        let stream = crate::host::copy_route_text(&prepared.operation, &prepared.stream)?;
        let owner = crate::host::copy_route_identity(&prepared.operation, &prepared.owner)?;
        Ok(Self {
            stream,
            owner,
            allowance: prepared.allowance,
            state: prepared.state.clone(),
            changed: prepared.changed.clone(),
            cancel: prepared.cancel.clone(),
            operation: prepared.operation.clone(),
        })
    }

    fn copy_marker(
        &self,
        token: String,
        ordinal: u64,
    ) -> Result<MarkerWitness, crate::host::HostRefusal> {
        // installed() already admitted/copied this token. MOVE it, never
        // clone it again; other immutable bodies consume the SAME original.
        let stream = crate::host::copy_route_text(&self.operation, &self.stream)?;
        let owner = crate::host::copy_route_identity(&self.operation, &self.owner)?;
        Ok(MarkerWitness {
            stream,
            token,
            owner,
            ordinal,
        })
    }

    // Never retain the state mutex across a wait or storage call. No timeout implies a join.
    async fn installed(&self) -> Result<(String, Arc<dyn crate::host::HostDeliveryRoute>), String> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let installed = {
                let state = self.state.lock().expect("worker route");
                match &*state {
                    RouteState::Installed { token, delivery } => {
                        // Measure while borrowed, but retain no body copy and
                        // run no overflow notification under route exclusion.
                        let bytes = crate::operation::encoded_len(
                            &(token, &self.stream, &self.owner.operation_id),
                            crate::operation::COMMAND_BYTES,
                        );
                        Some((token.len(), bytes, delivery.clone()))
                    }
                    RouteState::Released => return Err("route settled before installation".into()),
                    RouteState::Uncertain => return Err("route settlement uncertain".into()),
                    RouteState::Reserved => return Err("worker route not prepared".into()),
                    RouteState::Prepared => None,
                }
            };
            if let Some((length, encoded, delivery)) = installed {
                let encoded = encoded.map_err(|_| self.operation.overflow().to_string())?;
                if length > crate::operation::PREPARE_BYTES {
                    return Err(self.operation.overflow().to_string());
                }
                let bytes = encoded
                    .checked_add(length)
                    .ok_or_else(|| self.operation.overflow().to_string())?;
                self.operation.check().map_err(|e| e.to_string())?;
                self.operation.charge(bytes).map_err(|e| e.to_string())?;
                let state = self.state.lock().expect("worker route");
                self.operation.check().map_err(|e| e.to_string())?;
                match &*state {
                    RouteState::Installed {
                        token,
                        delivery: current,
                    } if token.len() == length && Arc::ptr_eq(current, &delivery) => {
                        // Installed token is immutable until terminal release.
                        // Revalidated SAME route/delivery; actual exact-length
                        // copy was charged OUTSIDE this lock, before allocation.
                        return Ok((String::from(Box::<str>::from(token.as_str())), delivery));
                    }
                    RouteState::Released => return Err("route settled before installation".into()),
                    RouteState::Uncertain => return Err("route settlement uncertain".into()),
                    _ => return Err("worker route changed before token copy".into()),
                }
            }
            tokio::time::timeout_at(self.allowance.deadline(), changed)
                .await
                .map_err(|_| "forced route installation timed out".to_string())?;
        }
    }

    async fn settled(&self) -> Result<(), String> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.state.lock().expect("worker route");
                match &*state {
                    RouteState::Released => return Ok(()),
                    RouteState::Uncertain => return Err("forced route settlement uncertain".into()),
                    _ => {}
                }
            }
            // Expiry requests abort; it cannot manufacture actor/storage settlement.
            if tokio::time::timeout_at(self.allowance.deadline(), changed)
                .await
                .is_err()
            {
                self.cancel.request();
                return Err("forced route settlement timed out".into());
            }
        }
    }
}

impl TranscriptSink {
    /// No callback/I/O under exclusion, no body MOVE or second storage owner.
    /// Unique Arcs and get_mut provide an exclusive borrow of the original
    /// cell; shared/busy/poisoned storage refuses instead of appearing empty.
    /// A missing task alone does NOT establish any actual supervision result:
    /// an unstarted sink also has no task. Whole archive custody must separately
    /// prove actual joins and census every other retained queue/result/body.
    #[allow(dead_code)]
    pub(crate) fn borrowed_worker_refusal(
        &mut self,
        expected: &Operation,
    ) -> Result<BorrowedWorkerDiagnostics<'_>, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        if !self.tx.owned()
            || !self
                .operation
                .as_ref()
                .is_some_and(|original| original.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        let delivery = Arc::get_mut(&mut self.delivery).ok_or(ArchiveError::Identity)?;
        if delivery.0.get_mut().is_some() {
            // Even is_finished/abort requested is NOT an observed join.
            return Err(ArchiveError::Identity);
        }
        let joined = &delivery.0.get_mut().join;
        if joined.first_observation.is_none()
            || !joined
                .original
                .as_ref()
                .is_some_and(|owner| owner.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        let pending = Arc::get_mut(&mut self.pending)
            .ok_or(ArchiveError::Identity)?
            .get_mut()
            .map_err(|_| ArchiveError::Identity)?;
        if pending
            .prepare_original
            .as_ref()
            .is_some_and(|reserved| !reserved.operation().same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        if pending.first_worker_refusal.is_some()
            && (!pending.legacy_overflow || pending.prepare_original.is_none())
        {
            return Err(ArchiveError::Identity);
        }
        let refusal = pending
            .first_worker_refusal
            .as_ref()
            .map(|refusal| match refusal {
                WorkerRefusal::Installation(body) => BorrowedWorkerRefusal::Installation(body),
                WorkerRefusal::Append(body) => BorrowedWorkerRefusal::Append(body),
                WorkerRefusal::MarkerCopy(body) => BorrowedWorkerRefusal::MarkerCopy(body),
                WorkerRefusal::Settlement(OwnedSettlementFailure::Uncertain, body) => {
                    BorrowedWorkerRefusal::SettlementUncertain(body)
                }
                WorkerRefusal::Settlement(OwnedSettlementFailure::TimedOut, body) => {
                    BorrowedWorkerRefusal::SettlementTimedOut(body)
                }
            });
        Ok(BorrowedWorkerDiagnostics {
            join: joined
                .first_observation
                .expect("checked first worker class"),
            refusal,
        })
    }

    /// No host/store callback, wait, task or channel send. Refuse busy, poisoned,
    /// previously-used or in-service queue metadata; never clear it to fit.
    /// Owned mirror capacity is pinned; other referenced graphs remain partial.
    #[allow(dead_code)]
    pub(crate) fn original_referenced_layout(
        &self,
    ) -> Result<TranscriptCreationLayout, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        let pending = self
            .pending
            .try_lock()
            .map_err(|_| ArchiveError::Identity)?;
        if self.operation.is_some()
            || self.source_origin.is_some()
            || !pending.rows.is_empty()
            || pending.bytes != 0
            || pending.legacy_overflow
            || pending.ordinal != 0
            || pending.route_owned
            || pending.route_operation.is_some()
            || pending.marker_phase != 0
            || pending.first_queue_refusal.is_some()
            || pending.first_worker_refusal.is_some()
            || pending.prepare_original.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        if matches!(&self.tx, TranscriptSender::Owned(_))
            && !self
                .delivery
                .0
                .try_lock()
                .is_ok_and(|owner| owner.join.pristine())
        {
            return Err(ArchiveError::Identity);
        }
        let pending_layout = std::alloc::Layout::for_value(self.pending.as_ref()).pad_to_align();
        let delivery = std::alloc::Layout::for_value(self.delivery.as_ref()).pad_to_align();
        let stream = std::alloc::Layout::for_value(self.stream.as_ref()).pad_to_align();
        let rows_capacity = if self.tx.owned() {
            pending
                .owned_rows_capacity()
                .ok_or(ArchiveError::Identity)?
        } else {
            pending.rows.capacity() // Legacy/closed fixture semantics unchanged.
        };
        let queue = std::alloc::Layout::array::<Arc<Capture>>(rows_capacity)
            .map_err(|_| ArchiveError::Fit)?
            .pad_to_align();
        let referenced_bytes = [pending_layout, delivery, stream, queue]
            .into_iter()
            .try_fold(0usize, |bytes, layout| bytes.checked_add(layout.size()))
            .ok_or(ArchiveError::Fit)?;
        if referenced_bytes > crate::operation::OPERATION_BYTES {
            return Err(ArchiveError::Fit);
        }
        Ok(TranscriptCreationLayout {
            pending: pending_layout,
            delivery,
            stream,
            queue,
            referenced_bytes,
        })
    }

    /// Explicit closed SOURCE fixture. No store, delivery task, runtime or
    /// Operation is constructed. Dropped receivers make accidental sends fail.
    /// This cannot witness live channel/store/library fit or original joins.
    #[cfg(test)]
    pub(crate) fn closed_for_source_control() -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<Item>();
        let (prepare, prepare_rx) = mpsc::channel::<Item>(1);
        drop((rx, prepare_rx));
        Self {
            tx: TranscriptSender::Legacy(tx),
            pending: Arc::new(Mutex::new(PendingCaptures::default())),
            delivery: Arc::new(DeliveryTask(tokio::sync::Mutex::new(DeliveryOwner::new(
                None,
            )))),
            operation: None,
            stream: Arc::from("00000000-0000-0000-0000-000000000001"),
            prepare: PrepareSender::Legacy(prepare),
            prepare_reply: None,
            intent_reply: None,
            postcommit_reply: None,
            source_origin: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_route_owned(&self) -> bool {
        self.pending.lock().expect("pending captures").route_owned
    }
    /// Start the delivery task for `store`. Must be called inside a Tokio runtime.
    pub fn new(store: Arc<dyn TranscriptStore>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<Item>();
        let (prepare, prepare_rx) = mpsc::channel::<Item>(1);
        let pending = Arc::new(Mutex::new(PendingCaptures::default()));
        let remaining = pending.clone();
        let stream: Arc<str> = uuid::Uuid::new_v4().to_string().into();
        let worker_stream = stream.clone();
        let task = tokio::spawn(delivery_worker(DeliveryInputs {
            store,
            rx: TranscriptReceiver::Legacy(rx),
            prepare_rx: PrepareReceiver::Legacy(prepare_rx),
            remaining,
            worker_stream,
        }));
        Self {
            tx: TranscriptSender::Legacy(tx),
            pending,
            delivery: Arc::new(DeliveryTask(tokio::sync::Mutex::new(DeliveryOwner::new(
                Some(task),
            )))),
            operation: None,
            stream: stream.clone(),
            prepare: PrepareSender::Legacy(prepare),
            prepare_reply: None,
            intent_reply: None,
            postcommit_reply: None,
            source_origin: None,
        }
    }

    /// Queue one entry for a seat (never waits for storage; capture time is taken now).
    pub fn emit(&self, seat: SeatId, entry: TranscriptEntry) {
        let _ = self.try_emit_batch(seat, vec![entry]);
    }

    /// Explicit owned producer association; old mirror rows retain their original provenance.
    pub fn for_operation(&self, operation: crate::operation::Operation) -> Self {
        // Association replacement has no reason to copy the previous variable origin.
        Self {
            tx: self.tx.clone(),
            pending: self.pending.clone(),
            delivery: self.delivery.clone(),
            operation: Some(operation),
            stream: self.stream.clone(),
            prepare: self.prepare.clone(),
            prepare_reply: self.prepare_reply.clone(),
            intent_reply: self.intent_reply.clone(),
            postcommit_reply: self.postcommit_reply.clone(),
            source_origin: None,
        }
    }

    /// Preserve the reader's exact operation/frame association, including explicit UNKNOWN.
    /// Deriving multiple rows from one frame does not fabricate fresh source evidence ordinals.
    pub(crate) fn for_origin(
        &self,
        origin: Option<&crate::operation::CaptureOrigin>,
    ) -> Result<Self, OperationError> {
        self.admit_origin_copy(origin)?;
        self.charge_allocation(std::mem::size_of::<Self>())?;
        // Do not clone the previous source_origin just to replace it: that would make
        // an extra unadmitted variable copy of an earlier frame's association.
        Ok(Self {
            tx: self.tx.clone(),
            pending: self.pending.clone(),
            delivery: self.delivery.clone(),
            operation: self.operation.clone(),
            stream: self.stream.clone(),
            prepare: self.prepare.clone(),
            prepare_reply: self.prepare_reply.clone(),
            intent_reply: self.intent_reply.clone(),
            postcommit_reply: self.postcommit_reply.clone(),
            source_origin: Some(origin.cloned()),
        })
    }

    /// Borrow the producer association when retaining a pairing, never infer it
    /// later from whichever frame most recently updated this sink.
    pub(crate) fn pairing_association(
        &self,
    ) -> (Option<&Operation>, Option<&crate::operation::CaptureOrigin>) {
        (
            self.operation.as_ref(),
            self.source_origin.as_ref().and_then(Option::as_ref),
        )
    }

    pub(crate) fn admit_pairing_metadata(&self, fixed_bytes: usize) -> Result<(), OperationError> {
        self.charge_allocation(fixed_bytes)?;
        self.admit_origin_copy(self.pairing_association().1)
    }

    fn admit_origin_copy(
        &self,
        origin: Option<&crate::operation::CaptureOrigin>,
    ) -> Result<(), OperationError> {
        let bytes = match origin {
            Some(origin) => origin
                .operation_id
                .len()
                .checked_add(origin.physical_child_id.as_ref().map_or(0, String::len))
                .and_then(|n| n.checked_add(std::mem::size_of::<crate::operation::CaptureOrigin>()))
                .ok_or_else(|| self.signal_overflow())?,
            None => 0,
        };
        self.charge_allocation(bytes)
    }

    fn admit_batch_storage(&self, entries: usize) -> Result<(), OperationError> {
        use crate::operation::MIRROR_ENTRIES;
        // The batch cannot fit even an empty shared mirror if its entry count is oversized.
        // Refuse before Vec::with_capacity or per-row provenance/serialization work.
        if entries > MIRROR_ENTRIES {
            return Err(self.signal_overflow());
        }
        let bytes = entries
            .checked_mul(std::mem::size_of::<Capture>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Vec<Capture>>()))
            .ok_or_else(|| self.signal_overflow())?;
        self.charge_allocation(bytes)
    }

    fn capture_timestamp(
        &self,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<String, OperationError> {
        struct Length(usize);
        impl std::fmt::Write for Length {
            fn write_str(&mut self, text: &str) -> std::fmt::Result {
                self.0 = self.0.checked_add(text.len()).ok_or(std::fmt::Error)?;
                Ok(())
            }
        }
        // Naive UTC has no allocated offset name; write_to avoids Display's temporary
        // String. One captured instant feeds both passes, preserving Millis + Z wire bytes.
        let utc = timestamp.naive_utc();
        let formatted = utc.format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let mut length = Length(0);
        formatted
            .write_to(&mut length)
            .map_err(|_| self.signal_overflow())?;
        let bytes = length
            .0
            .checked_add(std::mem::size_of::<String>())
            .ok_or_else(|| self.signal_overflow())?;
        self.charge_allocation(bytes)?;
        let mut text = String::with_capacity(length.0);
        formatted
            .write_to(&mut text)
            .map_err(|_| self.signal_overflow())?;
        Ok(text)
    }

    pub fn charge_copy(
        &self,
        value: &impl serde::Serialize,
    ) -> Result<(), crate::operation::OperationError> {
        let bytes = crate::operation::encoded_len(value, crate::operation::OPERATION_BYTES)
            .map_err(|error| {
                if let Some(operation) = &self.operation {
                    operation.overflow();
                }
                error
            })?;
        self.charge_allocation(bytes)
    }

    /// Admit driver-owned derived copies on the SAME meter before allocating them.
    /// Byte estimates include their explicit metadata; they are not an RSS proof.
    pub(crate) fn charge_allocation(&self, bytes: usize) -> Result<(), OperationError> {
        if let Some(operation) = &self.operation {
            operation.charge(bytes)?;
        }
        Ok(())
    }

    pub fn try_emit_batch(
        &self,
        seat: SeatId,
        entries: Vec<TranscriptEntry>,
    ) -> Result<(), crate::operation::OperationError> {
        use crate::operation::{DELIVERY_BYTES, MIRROR_BYTES, MIRROR_ENTRIES, OperationError};
        if self.tx.owned() {
            let mut pending = self.pending.lock().expect("pending captures");
            if pending.first_queue_refusal.is_some() {
                return Err(OperationError::CaptureOverflow); // Original refusal stays sticky.
            }
            if pending.owned_rows_capacity().is_none() {
                pending.legacy_overflow = true;
                drop(pending);
                return Err(self.signal_overflow()); // No derived copy or ring repair.
            }
        }
        self.admit_batch_storage(entries.len())?;
        let mut rows = Vec::with_capacity(entries.len());
        let mut bytes = 0usize;
        for entry in entries {
            let origin = match &self.source_origin {
                Some(origin) => {
                    // Each accepted/attempted row owns this actual copy. An earlier
                    // for_origin admission does not pay for an unlimited number of clones.
                    self.admit_origin_copy(origin.as_ref())?;
                    origin.clone()
                }
                None => self
                    .operation
                    .as_ref()
                    .map(|op| op.next_origin())
                    .transpose()?,
            };
            let row = UnconfirmedEntry {
                seat,
                at: self.capture_timestamp(chrono::Utc::now())?,
                entry,
                origin,
            };
            let size = match crate::operation::encoded_len(&row, DELIVERY_BYTES) {
                Ok(size) => size,
                Err(error) => {
                    if let Some(op) = &self.operation {
                        op.overflow();
                    } else {
                        self.pending
                            .lock()
                            .expect("pending captures")
                            .legacy_overflow = true;
                    }
                    return Err(error);
                }
            };
            bytes = bytes
                .checked_add(size)
                .ok_or_else(|| self.signal_overflow())?;
            rows.push(Capture {
                row,
                bytes: size,
                operation: self.operation.clone(),
                ordinal: 0,
            });
        }
        if self.tx.owned() {
            if let Some(op) = &self.operation {
                // Overflow can synchronously wake observers. Consume before exclusion;
                // a subsequent mirror/race refusal never refunds this admission.
                op.charge(bytes.checked_mul(3).ok_or_else(|| op.overflow())?)?;
            }
        }
        // Hold this lock through enqueue so concurrent producers and the recovery mirror
        // have the same order. A failed enqueue remains visible in the mirror.
        let mut pending = self.pending.lock().expect("pending captures");
        if self.tx.owned() {
            if pending.first_queue_refusal.is_some() || pending.legacy_overflow {
                return Err(OperationError::CaptureOverflow); // Recheck concurrent producer race.
            }
            if let Some(op) = &self.operation {
                op.check()?; // No observer notification; admission may have raced expiry/refusal.
            }
        }
        let total = pending.bytes.checked_add(bytes);
        // Recheck under the SAME mutation lock BEFORE Arc::new/push/send.
        // The logical 1024/4MiB guards remain additional even with spare.
        let owned_ring_refused = self.tx.owned()
            && pending.owned_rows_capacity().is_none_or(|capacity| {
                pending
                    .rows
                    .len()
                    .checked_add(rows.len())
                    .is_none_or(|length| length > capacity)
            });
        if owned_ring_refused
            || total.is_none_or(|n| n > MIRROR_BYTES)
            || pending
                .rows
                .len()
                .checked_add(rows.len())
                .is_none_or(|n| n > MIRROR_ENTRIES)
        {
            if self.tx.owned() {
                pending.legacy_overflow = true; // Sticky local cause before callback/unwind.
                drop(pending);
                return Err(self.signal_overflow());
            }
            if let Some(op) = &self.operation {
                op.overflow();
            } else {
                pending.legacy_overflow = true;
            }
            return Err(OperationError::CaptureOverflow);
        }
        if !self.tx.owned() {
            if let Some(op) = &self.operation {
                // Canonical row + TWO newly created full logical-body references. Physical sharing
                // does not hide their conservative cumulative charge, and encoded caps are not RSS.
                op.charge(bytes.checked_mul(3).ok_or_else(|| op.overflow())?)?;
            }
        }
        let last = match pending.ordinal.checked_add(rows.len() as u64) {
            Some(last) => last,
            None if self.tx.owned() => {
                pending.legacy_overflow = true;
                drop(pending);
                return Err(self.signal_overflow());
            }
            None => {
                if let Some(op) = &self.operation {
                    op.overflow();
                } else {
                    pending.legacy_overflow = true;
                }
                return Err(OperationError::CaptureOverflow);
            }
        };
        pending.bytes = total.expect("checked capture sum");
        let mut queue_failed = false;
        for mut capture in rows {
            pending.ordinal += 1;
            capture.ordinal = pending.ordinal;
            let capture = Arc::new(capture);
            pending.rows.push_back(capture.clone());
            if !queue_failed {
                if let Err(refusal) = self.tx.send_deferred(Item::Entry(capture)) {
                    if self.tx.owned() {
                        retain_queue_refusal(&mut pending, refusal);
                        queue_failed = true;
                    } // Legacy failed-send behavior remains unchanged.
                }
            }
        }
        debug_assert_eq!(pending.ordinal, last);
        if queue_failed {
            // ALL batch rows and original unsent message are retained BEFORE
            // overflow may wake/unwind. Never enqueue a later row past the gap.
            drop(pending);
            return Err(self.signal_overflow());
        }
        drop(pending);
        self.tx.wake_owned(); // Entire batch is canonical before observer wake.
        Ok(())
    }

    fn signal_overflow(&self) -> OperationError {
        if let Some(op) = &self.operation {
            op.overflow();
        } else {
            self.pending
                .lock()
                .expect("pending captures")
                .legacy_overflow = true;
        }
        OperationError::CaptureOverflow
    }

    /// Requires the actor's already-accepted campaign reservation. Reuses its SAME operation,
    /// cancellation and settlement association; cannot create an unreserved route or credit.
    pub async fn prepare_route(
        &self,
        reserved: ReservedForcedRoute,
    ) -> Result<PreparedRoute, String> {
        if self.tx.owned() {
            return self.prepare_owned_route(reserved).await;
        }
        let operation = reserved.operation();
        operation.check().map_err(|e| e.to_string())?;
        let owner = reserved.identity();
        let metadata = (
            &owner.operation_id,
            owner.seat.to_string(),
            owner.epoch,
            &owner.decision_id,
            owner.revision,
            self.stream.as_ref(),
        );
        let bytes = crate::operation::encoded_len(&metadata, crate::operation::PREPARE_BYTES)
            .map_err(|_| operation.overflow().to_string())?;
        operation
            .charge(
                bytes
                    .checked_mul(8)
                    .ok_or_else(|| operation.overflow().to_string())?,
            )
            .map_err(|e| e.to_string())?;
        let cancel = reserved.cancellation();
        let (done, wait) = oneshot::channel();
        {
            let mut pending = self.pending.lock().expect("pending captures");
            if pending.route_owned || pending.legacy_overflow {
                return Err("transcript route unavailable".into());
            }
            reserved
                .claim_stream(&self.stream)
                .map_err(|error| error.to_string())?;
            pending.route_owned = true;
            pending.route_operation = Some(operation.id().to_string());
            pending.marker_phase = 0;
            if self
                .prepare
                .try_send_deferred(Item::Prepare {
                    reserved,
                    done: PrepareReply::Legacy(done),
                })
                .is_err()
            {
                cancel.request();
                return Err("transcript worker closed during prepare".into());
            }
        }
        match tokio::time::timeout_at(operation.allowance().deadline(), wait).await {
            Ok(Ok(value)) => value,
            _ => {
                cancel.request();
                Err("transcript prepare unconfirmed".into())
            }
        }
    }

    async fn prepare_owned_route(
        &self,
        reserved: ReservedForcedRoute,
    ) -> Result<PreparedRoute, String> {
        let operation = reserved.operation();
        if !self
            .operation
            .as_ref()
            .is_some_and(|original| original.same_instance(&operation))
            || !matches!(&self.prepare, PrepareSender::Owned(_))
        {
            return Err("transcript prepare original mismatch".into());
        }
        operation.check().map_err(|error| error.to_string())?;
        let cancel = reserved.cancellation();
        let reply = self
            .prepare_reply
            .as_ref()
            .ok_or("no original prepare reply slot")?;
        {
            let mut pending = self.pending.lock().expect("pending captures");
            if pending.prepare_original.is_some()
                || pending.route_owned
                || pending.legacy_overflow
                || pending.first_queue_refusal.is_some()
            {
                return Err("transcript route unavailable".into());
            }
            // Allocation-free original Arc association MOVE before charge or
            // observer wake. Failure/unwind cannot authorize a second attempt.
            pending.prepare_original = Some(reserved.clone());
        }
        let (done, mut wait) = OwnedPrepareReply::claim(reply, &operation, &cancel)
            .map_err(|_| "transcript prepare reply unavailable".to_string())?;
        let owner = reserved.identity();
        let metadata = (
            &owner.operation_id,
            owner.seat.to_string(),
            owner.epoch,
            &owner.decision_id,
            owner.revision,
            self.stream.as_ref(),
        );
        let bytes = crate::operation::encoded_len(&metadata, crate::operation::PREPARE_BYTES)
            .map_err(|_| operation.overflow().to_string())?;
        operation
            .charge(
                bytes
                    .checked_mul(8)
                    .ok_or_else(|| operation.overflow().to_string())?,
            )
            .map_err(|error| error.to_string())?;
        let operation_id = crate::host::copy_route_text(&operation, operation.id())
            .map_err(|error| error.to_string())?;
        // Actual stream copy/admission may overflow-notify. No PendingCaptures
        // exclusion spans it; SAME first reservation remains retained above.
        reserved
            .claim_stream(&self.stream)
            .map_err(|error| error.to_string())?;
        let refusal = {
            let mut pending = self.pending.lock().expect("pending captures");
            if pending.route_owned
                || pending.legacy_overflow
                || pending.first_queue_refusal.is_some()
                || !pending.prepare_original.as_ref().is_some_and(|original| {
                    original.operation().same_instance(&operation)
                        && Arc::ptr_eq(&original.cancellation(), &cancel)
                })
            {
                pending.legacy_overflow = true;
                drop(pending);
                cancel.request(); // Original latch precedes unlocked callback.
                return Err("transcript prepare state changed during admission".into());
            }
            operation.check().map_err(|error| error.to_string())?;
            pending.route_owned = true;
            pending.route_operation = Some(operation_id); // Admitted body MOVE.
            pending.marker_phase = 0;
            match self.prepare.try_send_deferred(Item::Prepare {
                reserved,
                done: PrepareReply::Owned(done),
            }) {
                Ok(()) => false,
                Err(refusal) => {
                    retain_queue_refusal(&mut pending, refusal); // ENTIRE first Item MOVE.
                    true
                }
            }
        }; // No capture exclusion spans cancel or receiver wake.
        if refusal {
            cancel.request();
            return Err("transcript worker unavailable during prepare".into());
        }
        self.prepare.wake_owned();
        match tokio::time::timeout_at(operation.allowance().deadline(), wait.receive()).await {
            Ok(Ok(value)) => value,
            _ => {
                cancel.request();
                Err("transcript prepare unconfirmed".into())
            }
        }
    }

    fn enqueue_marker(&self, reply: MarkerReply) -> Result<(), String> {
        let mut pending = self.pending.lock().expect("pending captures");
        if !pending.route_owned || pending.legacy_overflow {
            return Err("no owned transcript route".into());
        }
        let operation = self
            .operation
            .as_ref()
            .ok_or("no owned transcript operation")?;
        if pending.route_operation.as_deref() != Some(operation.id()) {
            return Err("transcript marker operation mismatch".into());
        }
        if let MarkerReply::Owned(done) = &reply {
            if !done.matches_prefix(operation, pending.ordinal) {
                return Err("transcript marker original prefix mismatch".into());
            }
        }
        let next = match &reply {
            MarkerReply::Intent(_) if pending.marker_phase == 0 => 1,
            MarkerReply::Postcommit(_) if pending.marker_phase == 1 => 2,
            MarkerReply::Owned(done)
                if done.shared.kind == MarkerKind::Intent && pending.marker_phase == 0 =>
            {
                1
            }
            MarkerReply::Owned(done)
                if done.shared.kind == MarkerKind::Postcommit && pending.marker_phase == 1 =>
            {
                2
            }
            _ => return Err("transcript marker already issued or out of order".into()),
        };
        operation.check().map_err(|e| e.to_string())?;
        let ordinal = pending.ordinal;
        let phase = pending.marker_phase;
        let measured = crate::operation::encoded_len(
            &(self.stream.as_ref(), pending.ordinal),
            crate::operation::COMMAND_BYTES,
        );
        pending = if self.tx.owned() {
            // Charge/overflow may wake or unwind. No capture exclusion spans it.
            drop(pending);
            let bytes = measured.map_err(|_| operation.overflow().to_string())?;
            operation.charge(bytes).map_err(|e| e.to_string())?;
            let mut pending = self.pending.lock().expect("pending captures");
            if !pending.route_owned
                || pending.legacy_overflow
                || pending.first_queue_refusal.is_some()
                || pending.route_operation.as_deref() != Some(operation.id())
                || pending.ordinal != ordinal
                || pending.marker_phase != phase
            {
                // Never issue a marker for a different prefix/phase after admission.
                // Consumed credits remain consumed; no retry or replacement reply.
                pending.legacy_overflow = true;
                drop(pending);
                operation.overflow(); // Sticky refusal precedes unlocked notification.
                return Err("transcript marker state changed during admission".into());
            }
            operation.check().map_err(|e| e.to_string())?;
            pending
        } else {
            // Legacy admission and transport keep their existing exclusion behavior.
            let bytes = measured.map_err(|_| operation.overflow().to_string())?;
            operation.charge(bytes).map_err(|e| e.to_string())?;
            pending
        };
        if let Err(refusal) = self.tx.send_deferred(Item::Marker {
            ordinal: pending.ordinal,
            reply,
        }) {
            let message = match refusal.kind {
                QueueFailure::Full => "transcript queue full before marker",
                QueueFailure::Closed => "transcript worker closed before marker",
                QueueFailure::Busy => "transcript queue metadata busy before marker",
                QueueFailure::Poisoned => "transcript queue metadata poisoned before marker",
                QueueFailure::Invariant => "transcript queue metadata invalid before marker",
            };
            if self.tx.owned() {
                retain_queue_refusal(&mut pending, refusal);
                drop(pending);
                operation.overflow(); // Actual message/cause already retained.
            }
            return Err(message.into());
        }
        pending.marker_phase = next;
        drop(pending);
        self.tx.wake_owned(); // Phase/item retained before restoration callback.
        Ok(())
    }

    pub async fn confirm_intent(&self) -> Result<ConfirmedIntent, String> {
        if self.tx.owned() {
            return match self.confirm_owned_marker(MarkerKind::Intent).await? {
                MarkerValue::Intent(value) => Ok(value),
                MarkerValue::Postcommit(_) => unreachable!("original intent reply kind"),
            };
        }
        let (done, wait) = oneshot::channel();
        self.enqueue_marker(MarkerReply::Intent(done))?;
        let operation = self
            .operation
            .as_ref()
            .ok_or("no owned transcript operation")?;
        tokio::time::timeout_at(operation.allowance().deadline(), wait)
            .await
            .map_err(|_| "intent marker timed out".to_string())?
            .map_err(|_| "intent marker unconfirmed".to_string())?
    }

    pub async fn confirm_postcommit(&self) -> Result<ConfirmedPostcommit, String> {
        if self.tx.owned() {
            return match self.confirm_owned_marker(MarkerKind::Postcommit).await? {
                MarkerValue::Postcommit(value) => Ok(value),
                MarkerValue::Intent(_) => unreachable!("original postcommit reply kind"),
            };
        }
        let (done, wait) = oneshot::channel();
        self.enqueue_marker(MarkerReply::Postcommit(done))?;
        let operation = self
            .operation
            .as_ref()
            .ok_or("no owned transcript operation")?;
        tokio::time::timeout_at(operation.allowance().deadline(), wait)
            .await
            .map_err(|_| "postcommit marker timed out".to_string())?
            .map_err(|_| "postcommit marker unconfirmed".to_string())?
    }

    async fn confirm_owned_marker(&self, kind: MarkerKind) -> Result<MarkerValue, String> {
        let operation = self
            .operation
            .as_ref()
            .ok_or("no owned transcript operation")?;
        operation.check().map_err(|error| error.to_string())?;
        let shared = match kind {
            MarkerKind::Intent => self.intent_reply.as_ref(),
            MarkerKind::Postcommit => self.postcommit_reply.as_ref(),
        }
        .ok_or("no original marker reply slot")?;
        if shared.kind != kind {
            return Err("transcript marker reply kind mismatch".into());
        }
        let (cancel, ordinal) = {
            let pending = self.pending.lock().expect("pending captures");
            if !pending.route_owned
                || pending.legacy_overflow
                || pending.first_queue_refusal.is_some()
            {
                return Err("no owned transcript route".into());
            }
            let original = pending
                .prepare_original
                .as_ref()
                .ok_or("no original transcript reservation")?;
            if !original.operation().same_instance(operation) {
                return Err("transcript marker original mismatch".into());
            }
            (original.cancellation(), pending.ordinal)
        };
        // Claim/observer Drop and enqueue-budget callbacks are outside capture
        // exclusion. Sticky slot association prevents a new channel/attempt.
        let (done, mut wait) = OwnedMarkerReply::claim(shared, operation, &cancel, ordinal)
            .map_err(|_| "transcript marker reply unavailable".to_string())?;
        self.enqueue_marker(MarkerReply::Owned(done))?;
        match tokio::time::timeout_at(operation.allowance().deadline(), wait.receive()).await {
            Ok(Ok(value)) => Ok(value),
            _ => {
                let observation = wait
                    .close_observation()
                    .expect("original marker observation");
                // Body remains in same slot if already sent or arriving later.
                // Closure/observation is canonical before unlocked cancel wake.
                cancel.request();
                Err(observation)
            }
        }
    }

    pub fn system(&self, seat: SeatId, text: impl Into<String>) {
        self.emit(seat, TranscriptEntry::System { text: text.into() });
    }

    /// Wait until everything queued so far has been accepted by the store.
    /// A permanently unavailable store needs the supervisor's bounded shutdown path.
    pub async fn flush(&self) {
        let _ = self.flush_confirmed().await;
    }

    /// Confirm captures queued before this marker. Later captures from other
    /// producers may remain pending. A dead delivery worker returns an error.
    pub async fn flush_confirmed(&self) -> Result<(), String> {
        if self
            .pending
            .lock()
            .expect("pending captures")
            .legacy_overflow
        {
            return Err("capture_overflow".into());
        }
        if let Some(operation) = &self.operation {
            operation.check().map_err(|e| e.to_string())?;
        }
        let (done, wait) = oneshot::channel();
        if let Err(refusal) = self.tx.send(Item::Flush(done)) {
            let message = match refusal.kind {
                QueueFailure::Full => "transcript delivery queue is full",
                QueueFailure::Closed => "transcript delivery worker is closed",
                QueueFailure::Busy => "transcript delivery queue metadata is busy",
                QueueFailure::Poisoned => "transcript delivery queue metadata is poisoned",
                QueueFailure::Invariant => "transcript delivery queue metadata is invalid",
            };
            if self.tx.owned() {
                {
                    let mut pending = self.pending.lock().expect("pending captures");
                    retain_queue_refusal(&mut pending, refusal);
                }
                self.signal_overflow();
            }
            return Err(message.to_string());
        }
        wait.await
            .map_err(|_| "transcript delivery marker was not confirmed".to_string())
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().expect("pending captures").rows.len()
    }

    /// md15 seat-level policy: first fixed class BEFORE raw error/handle Drop.
    /// Full JoinError is never stored/formatted/downcast or charged to F/G.
    /// Default panic hook is untouched; caller uses the EXISTING epoch pause.
    #[allow(dead_code)]
    pub(crate) async fn join_owned_delivery(
        &self,
        expected: &Operation,
    ) -> Result<OwnedDeliveryJoinObservation, crate::failed_evidence::ArchiveError> {
        self.join_owned_delivery_at(expected, expected.allowance().deadline())
            .await
    }

    async fn join_owned_delivery_at(
        &self,
        expected: &Operation,
        caller_deadline: tokio::time::Instant,
    ) -> Result<OwnedDeliveryJoinObservation, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        if !self.tx.owned()
            || !self
                .operation
                .as_ref()
                .is_some_and(|original| original.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        let deadline = caller_deadline.min(expected.allowance().deadline());
        let mut delivery = match self.delivery.0.try_lock() {
            Ok(owner) => owner,
            Err(_) => tokio::time::timeout_at(deadline, self.delivery.0.lock())
                .await
                .map_err(|_| ArchiveError::Identity)?,
        };
        if delivery
            .join
            .original
            .as_ref()
            .is_some_and(|original| !original.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        if let Some(class) = delivery.join.observation() {
            // Handles a raw-payload Drop unwind after class publication too.
            // Actual join was already observed; no second poll/abort/overwrite.
            delivery.task.take();
            return Ok(class);
        }
        if delivery.task.is_none() {
            return Err(ArchiveError::Identity);
        }
        if delivery.join.original.is_none() {
            delivery.join.original = Some(expected.clone());
        }
        let task = delivery.task.as_mut().expect("checked original worker");
        task.abort();
        let result = if task.is_finished() {
            task.await
        } else {
            tokio::time::timeout_at(deadline, task)
                .await
                .map_err(|_| ArchiveError::Identity)?
        };
        let class = join_observation(&result);
        delivery.join.first_observation = Some(class); // FIRST, serialized only writer.
        drop(result); // Raw panic DROP AT JOIN; default hook supplied message/location.
        delivery.task.take(); // AFTER fixed class and raw error disposition.
        Ok(class) // Physical class only, never actor/storage/healthy ACK.
    }

    /// Abort and observe the SAME worker's actual join without moving its retained handle
    /// into a canceled timer future. Accepted rows stay in the canonical mirror; this does
    /// not clone them or claim that an actor's already accepted storage write was canceled.
    pub async fn join_delivery_until(&self, deadline: tokio::time::Instant) -> Result<(), String> {
        if self.tx.owned() {
            let original = self.operation.as_ref().ok_or_else(|| {
                "transcript worker original owner unavailable; Unsettled".to_string()
            })?;
            return match self.join_owned_delivery_at(original, deadline).await {
                Ok(OwnedDeliveryJoinObservation::Panicked) => Err(WORKER_PANIC_REASON.into()),
                Ok(_) => Ok(()),
                Err(_) => Err("transcript worker remains owned Unsettled".into()),
            };
        }
        let mut delivery = match self.delivery.0.try_lock() {
            Ok(owner) => owner,
            Err(_) => tokio::time::timeout_at(deadline, self.delivery.0.lock())
                .await
                .map_err(|_| "transcript worker owner unavailable; Unsettled".to_string())?,
        };
        if let Some(class) = delivery.join.observation() {
            delivery.task.take();
            return if class == OwnedDeliveryJoinObservation::Panicked {
                Err(WORKER_PANIC_REASON.into())
            } else {
                Ok(())
            };
        }
        if let Some(task) = delivery.task.as_mut() {
            task.abort();
            let joined = if task.is_finished() {
                task.await
            } else {
                tokio::time::timeout_at(deadline, task)
                    .await
                    .map_err(|_| "transcript worker remains owned Unsettled".to_string())?
            };
            let class = join_observation(&joined);
            delivery.join.first_observation = Some(class);
            drop(joined); // No old formatted panic cleanup String, no retained payload.
            delivery.task.take();
            if class == OwnedDeliveryJoinObservation::Panicked {
                return Err(WORKER_PANIC_REASON.into());
            }
        }
        Ok(())
    }

    /// Stop and join delivery, retaining captures for a recovery outbox.
    /// Stop all producers first. This is irreversible for every clone of this sink.
    /// The last in-flight append may have committed before cancellation without confirming;
    /// inspect the campaign before replaying an outbox. Never replay it blindly.
    pub async fn stop_delivery(&self) -> Vec<UnconfirmedEntry> {
        if self.tx.owned() {
            if let Some(original) = self.operation.as_ref() {
                // Original deadline and fixed class only. Failure/unknown keeps
                // the original handle; outbox rows are NOT a supervision ACK.
                let _ = self.join_owned_delivery(original).await;
            }
        } else {
            let mut delivery = self.delivery.0.lock().await;
            if delivery.join.observation().is_some() {
                delivery.task.take();
            } else if let Some(task) = delivery.task.as_mut() {
                task.abort();
                let result = task.await;
                delivery.join.first_observation = Some(join_observation(&result));
                drop(result);
                delivery.task.take();
            }
        }
        self.pending
            .lock()
            .expect("pending captures")
            .rows
            .iter()
            .map(|capture| capture.row.clone())
            .collect()
    }
}
/// One stored transcript entry.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredEntry {
    pub seat: SeatId,
    pub tseq: u64,
    pub at: String,
    pub game_seq: u64,
    pub entry: TranscriptEntry,
}

impl StoredEntry {
    /// The live-stream message for this entry.
    pub fn to_message(&self) -> ServerMessage {
        ServerMessage::Transcript {
            seat: self.seat.to_string(),
            tseq: self.tseq,
            at: self.at.clone(),
            game_seq: self.game_seq,
            entry: self.entry.clone(),
        }
    }
}

/// Source of the latest game event sequence, for [`LocalTranscript`].
pub type GameSeqFn = Arc<dyn Fn() -> u64 + Send + Sync>;

/// In-memory store that numbers entries per seat and broadcasts them. Used by tests and the demo
/// runner; the campaign server has its own persistent store.
pub struct LocalTranscript {
    next: Mutex<BTreeMap<SeatId, u64>>,
    log: Mutex<Vec<StoredEntry>>,
    tx: broadcast::Sender<StoredEntry>,
    game_seq: GameSeqFn,
}

impl LocalTranscript {
    pub fn new(game_seq: GameSeqFn) -> Arc<Self> {
        let (tx, _) = broadcast::channel(4096);
        Arc::new(Self {
            next: Mutex::new(BTreeMap::new()),
            log: Mutex::new(Vec::new()),
            tx,
            game_seq,
        })
    }

    pub fn detached() -> Arc<Self> {
        Self::new(Arc::new(|| 0))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StoredEntry> {
        self.tx.subscribe()
    }

    /// Everything stored so far, in capture order.
    pub fn log(&self) -> Vec<StoredEntry> {
        self.log.lock().expect("log").clone()
    }

    pub fn seat_log(&self, seat: SeatId) -> Vec<StoredEntry> {
        self.log().into_iter().filter(|r| r.seat == seat).collect()
    }
}

#[async_trait]
impl TranscriptStore for LocalTranscript {
    async fn append(
        &self,
        seat: SeatId,
        at: String,
        entry: TranscriptEntry,
    ) -> Result<u64, String> {
        let stored = {
            let mut next = self.next.lock().expect("next");
            let tseq = next.entry(seat).or_insert(0);
            *tseq += 1;
            let stored = StoredEntry {
                seat,
                tseq: *tseq,
                at,
                game_seq: (self.game_seq)(),
                entry,
            };
            self.log.lock().expect("log").push(stored.clone());
            stored
        };
        let tseq = stored.tseq;
        let _ = self.tx.send(stored);
        Ok(tseq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cna_core::ids::{Role, Side};

    const A: SeatId = SeatId::new(Side::Axis, Role::Commander);
    const B: SeatId = SeatId::new(Side::Commonwealth, Role::Commander);

    #[tokio::test]
    async fn tseq_is_per_seat_and_strictly_increasing() {
        let store = LocalTranscript::detached();
        let mut rx = store.subscribe();
        let sink = TranscriptSink::new(store.clone());
        sink.system(A, "one");
        sink.system(B, "one");
        sink.system(A, "two");
        sink.flush().await;
        let a: Vec<u64> = store.seat_log(A).iter().map(|r| r.tseq).collect();
        let b: Vec<u64> = store.seat_log(B).iter().map(|r| r.tseq).collect();
        assert_eq!(a, [1, 2]);
        assert_eq!(b, [1]);
        assert_eq!(rx.try_recv().unwrap().seat, A);
    }

    /// Fails the first `n` appends, then behaves.
    struct Flaky {
        fail: Mutex<u32>,
        inner: Arc<LocalTranscript>,
    }

    #[async_trait]
    impl TranscriptStore for Flaky {
        async fn append(
            &self,
            seat: SeatId,
            at: String,
            entry: TranscriptEntry,
        ) -> Result<u64, String> {
            {
                let mut n = self.fail.lock().unwrap();
                if *n > 0 {
                    *n -= 1;
                    return Err("db busy".into());
                }
            }
            self.inner.append(seat, at, entry).await
        }
    }

    #[tokio::test]
    async fn a_failing_store_delays_but_never_loses_or_reorders() {
        let inner = LocalTranscript::detached();
        let sink = TranscriptSink::new(Arc::new(Flaky {
            fail: Mutex::new(2),
            inner: inner.clone(),
        }));
        for i in 0..5 {
            sink.system(A, format!("m{i}"));
        }
        sink.flush().await;
        let texts: Vec<String> = inner
            .seat_log(A)
            .into_iter()
            .map(|r| match r.entry {
                TranscriptEntry::System { text } => text,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(texts, ["m0", "m1", "m2", "m3", "m4"]);
    }

    #[tokio::test]
    async fn wire_shape_matches_the_protocol() {
        let store = LocalTranscript::new(Arc::new(|| 1235));
        let sink = TranscriptSink::new(store.clone());
        sink.emit(
            A,
            TranscriptEntry::ToolCall {
                call_id: "c12".into(),
                tool: "describe_actions".into(),
                args: serde_json::json!({ "decision_id": "d1" }),
            },
        );
        sink.flush().await;
        let v = serde_json::to_value(store.log()[0].to_message()).unwrap();
        assert_eq!(v["type"], "transcript");
        assert_eq!(v["seat"], "axis.commander");
        assert_eq!(v["tseq"], 1);
        assert_eq!(v["game_seq"], 1235);
        assert_eq!(v["entry"]["kind"], "tool_call");
        assert_eq!(v["entry"]["call_id"], "c12");
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use tokio::sync::Notify;

    struct Blocked {
        entered: Arc<Notify>,
    }
    #[async_trait]
    impl TranscriptStore for Blocked {
        async fn append(&self, _: SeatId, _: String, _: TranscriptEntry) -> Result<u64, String> {
            self.entered.notify_one();
            std::future::pending().await
        }
    }
    #[tokio::test]
    async fn expired_borrowed_join_retains_worker_and_canonical_rows_without_renewal() {
        let entered = Arc::new(Notify::new());
        let sink = TranscriptSink::new(Arc::new(Blocked {
            entered: entered.clone(),
        }));
        let seat = "axis.commander".parse().unwrap();
        sink.system(seat, "retained accepted capture");
        entered.notified().await;
        let deadline = tokio::time::Instant::now();
        // Another supervisor temporarily borrows ownership. Expiry cannot take/drop it.
        let borrowed = sink.delivery.0.lock().await;
        assert!(sink.join_delivery_until(deadline).await.is_err());
        assert!(borrowed.is_some());
        assert_eq!(sink.pending_count(), 1);
        borrowed.as_ref().unwrap().abort();
        while !borrowed.as_ref().unwrap().is_finished() {
            tokio::task::yield_now().await;
        }
        drop(borrowed);
        // The original expired deadline now observes an already-completed SAME join.
        sink.join_delivery_until(deadline).await.unwrap();
        assert!(sink.delivery.0.lock().await.is_none());
        let pending = sink.pending.lock().unwrap();
        assert_eq!(pending.rows.len(), 1);
        assert!(matches!(&pending.rows[0].row.entry,
            TranscriptEntry::System { text } if text == "retained accepted capture"));
    }
    #[tokio::test]
    async fn stopping_delivery_retains_ordered_captures_and_joins_in_flight_append() {
        let entered = Arc::new(Notify::new());
        let sink = TranscriptSink::new(Arc::new(Blocked {
            entered: entered.clone(),
        }));
        let seat = "axis.commander".parse().unwrap();
        for i in 0..3 {
            sink.system(seat, format!("m{i}"));
        }
        entered.notified().await;
        let (first, second) = tokio::join!(sink.stop_delivery(), sink.stop_delivery());
        assert_eq!(first, second);
        let texts: Vec<_> = first
            .iter()
            .map(|c| match &c.entry {
                TranscriptEntry::System { text } => text.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(texts, ["m0", "m1", "m2"]);
        sink.system(seat, "captured after close");
        assert_eq!(
            sink.pending_count(),
            4,
            "closed delivery must not silently drop captures"
        );
    }
    #[tokio::test]
    async fn dropping_last_producer_still_drains_a_healthy_store() {
        let store = LocalTranscript::detached();
        let mut live = store.subscribe();
        let sink = TranscriptSink::new(store.clone());
        sink.system("axis.commander".parse().unwrap(), "last capture");
        drop(sink);
        let row = tokio::time::timeout(Duration::from_secs(1), live.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(row.entry, TranscriptEntry::System { text } if text == "last capture"));
    }
}

#[cfg(test)]
mod host_route_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FirstAppend {
        entered: Notify,
        release: Notify,
        calls: AtomicUsize,
        normal: Mutex<Vec<SeatId>>,
    }
    #[async_trait]
    impl TranscriptStore for FirstAppend {
        async fn append(&self, seat: SeatId, _: String, _: TranscriptEntry) -> Result<u64, String> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.entered.notify_one();
                self.release.notified().await;
            }
            self.normal.lock().unwrap().push(seat);
            Ok(1)
        }
    }

    struct Routed(Mutex<Vec<(u64, SeatId)>>);
    #[async_trait]
    impl crate::host::HostDeliveryRoute for Routed {
        async fn append(
            &self,
            _: &str,
            ordinal: u64,
            row: &UnconfirmedEntry,
        ) -> Result<u64, String> {
            self.0.lock().unwrap().push((ordinal, row.seat));
            Ok(ordinal)
        }
    }

    fn operation() -> Operation {
        Operation::new(
            "forced-operation".into(),
            1,
            Allowance::required(Some(tokio::time::Instant::now() + Duration::from_secs(5)))
                .unwrap(),
        )
        .unwrap()
    }

    fn held_append() -> Arc<FirstAppend> {
        Arc::new(FirstAppend {
            entered: Notify::new(),
            release: Notify::new(),
            calls: AtomicUsize::new(0),
            normal: Mutex::new(vec![]),
        })
    }

    #[tokio::test]
    async fn admitted_timestamp_matches_existing_millis_z_bytes_for_calendar_boundaries() {
        use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
        let sink = TranscriptSink::new(LocalTranscript::detached());
        let leap_second = NaiveDate::from_ymd_opt(2016, 12, 31)
            .unwrap()
            .and_hms_nano_opt(23, 59, 59, 1_987_654_321)
            .unwrap();
        let mut dates = vec![NaiveDateTime::MIN, NaiveDateTime::MAX, leap_second];
        for year in [-10000, -1, 0, 1, 9999, 10000] {
            dates.push(
                NaiveDate::from_ymd_opt(year, 1, 2)
                    .unwrap()
                    .and_hms_nano_opt(3, 4, 5, 123_456_789)
                    .unwrap(),
            );
        }
        for date in dates {
            let timestamp = DateTime::<Utc>::from_naive_utc_and_offset(date, Utc);
            let expected = timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let op = operation();
            let owned = sink.for_operation(op.clone());
            let bytes = op.captured_bytes();
            let text = owned.capture_timestamp(timestamp).unwrap();
            assert_eq!(text, expected);
            assert_eq!(
                op.captured_bytes(),
                bytes + text.len() + std::mem::size_of::<String>()
            );
            assert_eq!(owned.pending_count(), 0);
            assert!(matches!(
                op.start_evidence(),
                crate::operation::StartEvidence::NotStarted
            ));
        }
        assert!(sink.stop_delivery().await.is_empty()); // Actual fixture join, no capture issue.
    }

    #[tokio::test]
    async fn timestamp_exact_charge_and_one_byte_refusal_do_not_refund_or_start() {
        let timestamp = chrono::NaiveDate::from_ymd_opt(2026, 10, 10)
            .unwrap()
            .and_hms_milli_opt(1, 2, 3, 456)
            .unwrap()
            .and_utc();
        let expected = timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let needed = expected.len() + std::mem::size_of::<String>();
        let sink = TranscriptSink::new(LocalTranscript::detached());
        let op = operation();
        op.charge(crate::operation::OPERATION_BYTES - op.captured_bytes() - needed)
            .unwrap();
        let owned = sink.for_operation(op.clone());
        assert_eq!(owned.capture_timestamp(timestamp).unwrap(), expected);
        assert_eq!(op.captured_bytes(), crate::operation::OPERATION_BYTES);
        assert_eq!(op.check(), Ok(()));
        let refused = operation();
        refused.possible_start();
        refused
            .charge(crate::operation::OPERATION_BYTES - refused.captured_bytes() - needed + 1)
            .unwrap();
        let bytes = refused.captured_bytes();
        let other = sink.for_operation(refused.clone());
        assert_eq!(
            other.capture_timestamp(timestamp),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(refused.captured_bytes(), bytes);
        assert_eq!(refused.check(), Err(OperationError::CaptureOverflow));
        assert!(matches!(
            refused.start_evidence(),
            crate::operation::StartEvidence::Possible
        ));
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn timestamp_charge_refusal_preserves_accepted_unknown_prefix_and_original_meter() {
        let store = held_append();
        let op = operation();
        op.possible_start();
        let sink = TranscriptSink::new(store.clone()).for_operation(op.clone());
        let owned = sink.for_origin(None).unwrap();
        owned
            .try_emit_batch(
                A,
                vec![TranscriptEntry::System {
                    text: "original unknown prefix".into(),
                }],
            )
            .unwrap();
        store.entered.notified().await;
        let batch_bytes = std::mem::size_of::<Capture>() + std::mem::size_of::<Vec<Capture>>();
        op.charge(crate::operation::OPERATION_BYTES - op.captured_bytes() - batch_bytes)
            .unwrap();
        let bytes = op.captured_bytes();
        assert_eq!(
            owned.try_emit_batch(
                B,
                vec![TranscriptEntry::System {
                    text: "refused timestamp row".into()
                }]
            ),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(op.captured_bytes(), bytes + batch_bytes);
        assert_eq!(owned.pending_count(), 1);
        assert_eq!(owned.source_origin, Some(None));
        assert!(matches!(
            op.start_evidence(),
            crate::operation::StartEvidence::Possible
        ));
        let rows = sink.stop_delivery().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].seat, A);
        assert_eq!(rows[0].origin, None);
        assert_eq!(
            rows[0].entry,
            TranscriptEntry::System {
                text: "original unknown prefix".into()
            }
        );
    }

    #[tokio::test]
    async fn impossible_batch_count_refuses_before_storage_or_origin_and_keeps_prefix() {
        let store = held_append();
        let sink = TranscriptSink::new(store.clone());
        sink.system(A, "original accepted prefix");
        store.entered.notified().await;
        let op = operation();
        op.possible_start();
        let owned = sink.for_operation(op.clone());
        let bytes = op.captured_bytes();
        assert_eq!(
            owned.try_emit_batch(
                A,
                (0..=crate::operation::MIRROR_ENTRIES)
                    .map(|_| TranscriptEntry::System {
                        text: "refused batch".into()
                    })
                    .collect()
            ),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(op.captured_bytes(), bytes); // No derived storage/origin was admitted.
        assert_eq!(sink.pending_count(), 1);
        assert!(matches!(
            op.start_evidence(),
            crate::operation::StartEvidence::Possible
        ));
        let rows = sink.stop_delivery().await; // Actual original fixture worker join.
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].entry,
            TranscriptEntry::System {
                text: "original accepted prefix".into()
            }
        );
        assert!(rows[0].origin.is_none()); // Existing legacy prefix is not retagged.
    }

    #[tokio::test]
    async fn row_origin_copy_refusal_keeps_original_association_and_accepted_prefix() {
        let store = held_append();
        let op = operation();
        op.possible_start();
        let origin = op.next_child_origin("original-kernel-child").unwrap();
        let sink = TranscriptSink::new(store.clone()).for_operation(op.clone());
        let owned = sink.for_origin(Some(&origin)).unwrap();
        owned
            .try_emit_batch(
                A,
                vec![TranscriptEntry::System {
                    text: "accepted".into(),
                }],
            )
            .unwrap();
        store.entered.notified().await;
        let pointer = owned
            .source_origin
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .operation_id
            .as_ptr();
        let batch_bytes = std::mem::size_of::<Capture>() + std::mem::size_of::<Vec<Capture>>();
        let origin_bytes = origin.operation_id.len()
            + origin.physical_child_id.as_ref().unwrap().len()
            + std::mem::size_of::<crate::operation::CaptureOrigin>();
        op.charge(
            crate::operation::OPERATION_BYTES - op.captured_bytes() - batch_bytes - origin_bytes
                + 1,
        )
        .unwrap();
        let bytes = op.captured_bytes();
        assert_eq!(
            owned.try_emit_batch(
                B,
                vec![TranscriptEntry::System {
                    text: "refused next row".into()
                }]
            ),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(op.captured_bytes(), bytes + batch_bytes);
        let retained = owned.source_origin.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(retained, &origin);
        assert_eq!(retained.operation_id.as_ptr(), pointer);
        assert_eq!(sink.pending_count(), 1);
        assert!(matches!(
            op.start_evidence(),
            crate::operation::StartEvidence::Possible
        ));
        let rows = sink.stop_delivery().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].seat, A);
        assert_eq!(rows[0].origin.as_ref(), Some(&origin));
        assert_eq!(
            rows[0].entry,
            TranscriptEntry::System {
                text: "accepted".into()
            }
        );
    }

    #[tokio::test]
    async fn each_row_origin_and_batch_storage_charged_without_new_ordinals_or_unknown_retag() {
        let store = held_append();
        let op = operation();
        let origin = op.next_child_origin("original-kernel-child").unwrap();
        let sink = TranscriptSink::new(store.clone()).for_operation(op.clone());
        let owned = sink.for_origin(Some(&origin)).unwrap();
        let bytes = op.captured_bytes();
        owned
            .try_emit_batch(
                A,
                vec![
                    TranscriptEntry::System {
                        text: "one λ".into(),
                    },
                    TranscriptEntry::System {
                        text: "two escaped\"".into(),
                    },
                ],
            )
            .unwrap();
        store.entered.notified().await;
        let (row_bytes, timestamp_bytes, issued_bytes) = {
            let pending = sink.pending.lock().unwrap();
            (
                pending
                    .rows
                    .iter()
                    .map(|capture| capture.bytes)
                    .sum::<usize>(),
                pending
                    .rows
                    .iter()
                    .map(|capture| capture.row.at.len() + std::mem::size_of::<String>())
                    .sum::<usize>(),
                pending.rows.front().unwrap().bytes,
            )
        };
        assert_eq!(store.calls.load(Ordering::SeqCst), 1); // Actual first issue, no ACK/retry.
        let origin_bytes = origin.operation_id.len()
            + origin.physical_child_id.as_ref().unwrap().len()
            + std::mem::size_of::<crate::operation::CaptureOrigin>();
        assert_eq!(
            op.captured_bytes(),
            bytes
                + 2 * std::mem::size_of::<Capture>()
                + std::mem::size_of::<Vec<Capture>>()
                + 2 * origin_bytes
                + timestamp_bytes
                + 3 * row_bytes
                + issued_bytes // Retained store-issue copy is charged before its held await.
        );
        let unknown = owned.for_origin(None).unwrap();
        unknown
            .try_emit_batch(
                B,
                vec![TranscriptEntry::System {
                    text: "unknown original".into(),
                }],
            )
            .unwrap();
        let rows = sink.stop_delivery().await;
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].origin.as_ref(), Some(&origin));
        assert_eq!(rows[1].origin.as_ref(), Some(&origin));
        assert_eq!(rows[2].origin, None);
        assert_eq!(rows[2].seat, B);
        assert_eq!(
            rows[0].entry,
            TranscriptEntry::System {
                text: "one λ".into()
            }
        );
        assert_eq!(
            rows[1].entry,
            TranscriptEntry::System {
                text: "two escaped\"".into()
            }
        );
        assert!(matches!(
            op.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        ));
    }

    #[tokio::test]
    async fn prepare_joins_current_ack_routes_older_authors_and_second_marker_freezes_issue() {
        let a: SeatId = "axis.commander".parse().unwrap();
        let b: SeatId = "commonwealth.commander".parse().unwrap();
        let store = Arc::new(FirstAppend {
            entered: Notify::new(),
            release: Notify::new(),
            calls: AtomicUsize::new(0),
            normal: Mutex::new(vec![]),
        });
        let sink = TranscriptSink::new(store.clone());
        sink.system(a, "already issued");
        store.entered.notified().await;
        sink.system(b, "older buffered sibling");
        let op = operation();
        let forced = sink.for_operation(op.clone());
        let owner = HostIdentity {
            seat: a,
            epoch: 1,
            decision_id: "decision".into(),
            revision: 2,
            operation_id: op.id().into(),
        };
        let reserved = crate::host::ForcedReservationOwner::new(owner, op.clone()).unwrap();
        reserved
            .register_writer_wake(
                std::thread::current(),
                std::mem::size_of::<std::thread::Thread>(),
            )
            .unwrap();
        let preparing = {
            let sink = forced.clone();
            let view = reserved.route();
            tokio::spawn(async move { sink.prepare_route(view).await })
        };
        // The preparation future must enqueue while the normal append is still owned.
        tokio::time::timeout(Duration::from_secs(1), async {
            while !sink.pending.lock().unwrap().route_owned {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            !preparing.is_finished(),
            "prepare cannot fake the normal append ACK"
        );
        store.release.notify_one();
        let route = preparing.await.unwrap().unwrap();
        assert_eq!(
            route.boundary(),
            1,
            "buffered captures are not normal append ACKs"
        );
        let routed = Arc::new(Routed(Mutex::new(vec![])));
        route.install("token".into(), routed.clone()).unwrap();
        forced.system(a, "confirmed intent");
        let first = forced.confirm_intent().await.unwrap();
        assert_eq!(first.ordinal(), 3);
        assert_eq!(first.token(), "token");
        assert_eq!(*store.normal.lock().unwrap(), [a]);
        assert_eq!(*routed.0.lock().unwrap(), [(2, b), (3, a)]);
        sink.system(b, "between-marker sibling");
        forced.system(a, "accepted receipt");
        let second = forced.confirm_postcommit().await.unwrap();
        assert!(second.ordinal() >= first.ordinal());
        assert_eq!(*routed.0.lock().unwrap(), [(2, b), (3, a), (4, b), (5, a)]);
        sink.system(b, "later sibling");
        tokio::task::yield_now().await;
        assert_eq!(
            sink.pending_count(),
            1,
            "marker two freezes later issue for scoped final pending"
        );
        let cancellation = reserved.cancellation();
        cancellation.request_settlement_after_supervision();
        reserved
            .acknowledge_settlement(&cancellation, false)
            .unwrap(); // Actual test actor transition.
        sink.flush_confirmed().await.unwrap();
        assert_eq!(*store.normal.lock().unwrap(), [a, b]);
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn oversized_whole_group_refuses_and_keeps_accepted_prefix() {
        let seat = "axis.commander".parse().unwrap();
        let store = Arc::new(FirstAppend {
            entered: Notify::new(),
            release: Notify::new(),
            calls: AtomicUsize::new(0),
            normal: Mutex::new(vec![]),
        });
        let sink = TranscriptSink::new(store.clone());
        sink.system(seat, "accepted prefix");
        store.entered.notified().await;
        let op = operation();
        let owned = sink.for_operation(op.clone());
        assert_eq!(
            owned.try_emit_batch(
                seat,
                vec![
                    TranscriptEntry::System {
                        text: "must not partially accept".into()
                    },
                    TranscriptEntry::System {
                        text: "x".repeat(crate::operation::DELIVERY_BYTES)
                    },
                ]
            ),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(op.check(), Err(OperationError::CaptureOverflow));
        assert_eq!(sink.pending_count(), 1);
        let retained = sink.stop_delivery().await;
        assert_eq!(retained.len(), 1);
        assert!(
            matches!(&retained[0].entry, TranscriptEntry::System { text } if text == "accepted prefix")
        );
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Actual bounded queue, no delivery worker,
    // Operation, native/store callback, task, timer or physical join is started.
    #[test]
    fn pending_owned_queue_retains_full_or_closed_refusal_and_entire_batch() {
        let seat: SeatId = "axis.commander".parse().unwrap();
        for closed in [false, true] {
            let mut inputs = PendingTranscriptInputs::new(
                LocalTranscript::detached(),
                Arc::from("00000000-0000-0000-0000-000000000001"),
            );
            inputs.measure_creation().unwrap();
            let layout = inputs.creation_layout().unwrap().as_ref().unwrap();
            let expected = std::alloc::Layout::array::<Item>(OWNED_DELIVERY_ITEMS + 1)
                .unwrap()
                .pad_to_align();
            assert_eq!(layout.queue_elements, expected);
            assert!(
                layout.known_bytes
                    >= expected.size() + layout.pending.size() + layout.worker_frame.size() * 2
            );
            assert_eq!(
                inputs.producer.as_ref().unwrap().tx.maximum(),
                Some(crate::operation::MIRROR_ENTRIES)
            );
            let mut receivers = Vec::new();
            if closed {
                let TranscriptReceiver::Owned(rx) = &mut inputs.worker.as_mut().unwrap().rx else {
                    panic!("not bounded");
                };
                rx.close();
            } else {
                for _ in 0..OWNED_DELIVERY_ITEMS {
                    let (done, wait) = oneshot::channel();
                    assert!(
                        inputs
                            .producer
                            .as_ref()
                            .unwrap()
                            .tx
                            .send(Item::Flush(done))
                            .is_ok()
                    );
                    receivers.push(wait); // Unpolled ACK receivers, no waiter task.
                }
            }
            let first = String::from("original first unsent capture");
            let first_pointer = first.as_ptr();
            let second = String::from("original later capture retained despite earlier gap");
            let second_pointer = second.as_ptr();
            let producer = inputs.producer.as_ref().unwrap();
            assert_eq!(
                producer.try_emit_batch(
                    seat,
                    vec![
                        TranscriptEntry::System { text: first },
                        TranscriptEntry::System { text: second },
                    ]
                ),
                Err(OperationError::CaptureOverflow)
            );
            let first_refusal_pointer;
            {
                let pending = producer.pending.lock().unwrap();
                assert!(pending.legacy_overflow);
                assert_eq!(pending.rows.len(), 2);
                assert_eq!(pending.ordinal, 2);
                assert!(pending.bytes > 0);
                let refusal = pending.first_queue_refusal.as_ref().unwrap();
                first_refusal_pointer = std::ptr::from_ref(refusal);
                assert_eq!(
                    refusal.kind,
                    if closed {
                        QueueFailure::Closed
                    } else {
                        QueueFailure::Full
                    }
                );
                let Item::Entry(original) = &refusal._item else {
                    panic!("unsent body not retained");
                };
                assert!(Arc::ptr_eq(original, pending.rows.front().unwrap()));
                for (capture, expected) in pending.rows.iter().zip([first_pointer, second_pointer])
                {
                    let TranscriptEntry::System { text } = &capture.row.entry else {
                        panic!("changed body");
                    };
                    assert!(std::ptr::eq(text.as_ptr(), expected));
                    assert!(capture.operation.is_none());
                }
            }
            if !closed {
                let TranscriptReceiver::Owned(rx) = &mut inputs.worker.as_mut().unwrap().rx else {
                    panic!("not bounded");
                };
                for _ in 0..OWNED_DELIVERY_ITEMS {
                    assert!(matches!(rx.try_recv(), Ok(Item::Flush(_))));
                }
                assert!(rx.is_empty()); // Freed capacity cannot renew a failed owner.
            }
            let producer = inputs.producer.as_ref().unwrap();
            assert_eq!(
                producer.try_emit_batch(
                    seat,
                    vec![TranscriptEntry::System {
                        text: String::from("must not retry")
                    }]
                ),
                Err(OperationError::CaptureOverflow)
            );
            let pending = producer.pending.lock().unwrap();
            assert_eq!(pending.rows.len(), 2);
            assert!(std::ptr::eq(
                pending.first_queue_refusal.as_ref().unwrap(),
                first_refusal_pointer
            ));
            assert!(producer.operation.is_none());
            assert!(producer.delivery.0.try_lock().unwrap().is_none());
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Used or legacy-unbounded inputs cannot
    // masquerade as a fresh bounded pending queue at original creation.
    #[test]
    fn pending_queue_creation_refuses_used_closed_and_unbounded_input_without_repair() {
        use crate::failed_evidence::ArchiveError;
        for mode in 0..3 {
            let mut inputs = PendingTranscriptInputs::new(
                LocalTranscript::detached(),
                Arc::from("00000000-0000-0000-0000-000000000001"),
            );
            if mode == 0 {
                let (done, _wait) = oneshot::channel();
                assert!(
                    inputs
                        .producer
                        .as_ref()
                        .unwrap()
                        .tx
                        .send(Item::Flush(done))
                        .is_ok()
                );
            } else if mode == 1 {
                let TranscriptReceiver::Owned(rx) = &mut inputs.worker.as_mut().unwrap().rx else {
                    panic!("not bounded");
                };
                rx.close();
            } else {
                let (tx, rx) = mpsc::unbounded_channel();
                inputs.producer.as_mut().unwrap().tx = TranscriptSender::Legacy(tx);
                inputs.worker.as_mut().unwrap().rx = TranscriptReceiver::Legacy(rx);
            }
            assert!(inputs.measure_creation().is_err());
            assert!(matches!(
                inputs.creation_layout(),
                Some(Err(ArchiveError::Identity))
            ));
            let first = std::ptr::from_ref(inputs.creation_layout().unwrap());
            assert!(inputs.measure_creation().is_err());
            assert!(std::ptr::eq(inputs.creation_layout().unwrap(), first));
            assert!(
                inputs
                    .producer
                    .as_ref()
                    .unwrap()
                    .delivery
                    .0
                    .try_lock()
                    .unwrap()
                    .is_none()
            );
            assert!(inputs.producer.as_ref().unwrap().operation.is_none());
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Pending inputs own both receivers and the
    // actual store before a task/Operation exists. This is not full graph fit.
    #[test]
    fn pending_original_transcript_keeps_inputs_and_first_partial_layout_without_task() {
        use crate::failed_evidence::ArchiveError;
        struct NoAppend {
            body: Vec<u8>,
            calls: std::sync::atomic::AtomicUsize,
        }
        // Explicit expanded signature makes accidental append CONSTRUCTION
        // panic, before even a returned future could be polled or boxed here.
        impl TranscriptStore for NoAppend {
            fn append<'life0, 'async_trait>(
                &'life0 self,
                _: SeatId,
                _: String,
                _: TranscriptEntry,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<u64, String>> + Send + 'async_trait>,
            >
            where
                'life0: 'async_trait,
                Self: 'async_trait,
            {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                panic!("creation sizing must not construct append future")
            }
        }
        for mode in 0..4 {
            let mut body = Vec::with_capacity(4096);
            body.push(19u8);
            let store = Arc::new(NoAppend {
                body,
                calls: std::sync::atomic::AtomicUsize::new(0),
            });
            let body_pointer = store.body.as_ptr();
            let body_capacity = store.body.capacity();
            let stream: Arc<str> = Arc::from("00000000-0000-0000-0000-000000000001");
            let mut pending = PendingTranscriptInputs::new(store.clone(), stream.clone());
            assert!(
                !pending.producer.as_ref().unwrap().tx.is_closed()
                    && !pending.producer.as_ref().unwrap().prepare.is_closed()
            );
            assert!(
                pending
                    .producer
                    .as_ref()
                    .unwrap()
                    .delivery
                    .0
                    .try_lock()
                    .unwrap()
                    .is_none()
            );
            if mode == 3 {
                let foreign: Arc<str> = Arc::from("00000000-0000-0000-0000-000000000001");
                assert_eq!(&*foreign, &*stream);
                assert!(!Arc::ptr_eq(&foreign, &stream));
                pending.worker.as_mut().unwrap().worker_stream = foreign.clone();
                assert!(pending.measure_creation().is_err());
                assert!(matches!(
                    pending.creation_layout(),
                    Some(Err(ArchiveError::Identity))
                ));
                assert!(Arc::ptr_eq(
                    &pending.worker.as_ref().unwrap().worker_stream,
                    &foreign
                ));
                pending.worker.as_mut().unwrap().worker_stream = stream.clone();
            } else if mode == 2 {
                let worker = pending.worker.take(); // Synthetic omitted-input refusal.
                assert!(pending.measure_creation().is_err());
                assert!(matches!(
                    pending.creation_layout(),
                    Some(Err(ArchiveError::Identity))
                ));
                pending.worker = worker; // Restoring input never renews the attempt.
            } else if mode == 1 {
                assert!(pending.measure_with_cap(0).is_err());
                assert!(matches!(
                    pending.creation_layout(),
                    Some(Err(ArchiveError::Fit))
                ));
            } else {
                let inline = std::alloc::Layout::for_value(&pending).pad_to_align();
                pending.measure_creation().unwrap();
                let layout = pending.creation_layout().unwrap().as_ref().unwrap();
                assert_eq!(layout.pending, inline);
                assert_eq!(
                    layout.store_body,
                    std::alloc::Layout::for_value(store.as_ref()).pad_to_align()
                );
                assert!(layout.worker_frame.size() > 0);
                assert!(
                    layout.known_bytes
                        >= inline.size()
                            + layout.worker_frame.size() * 2
                            + layout.store_body.size()
                            + layout.producer_references.referenced_bytes
                );
            }
            let first = std::ptr::from_ref(pending.creation_layout().unwrap());
            assert!(pending.measure_creation().is_err());
            assert!(std::ptr::eq(pending.creation_layout().unwrap(), first));
            let worker = pending.worker.as_ref().unwrap();
            let actual_store: Arc<dyn TranscriptStore> = store.clone();
            assert!(Arc::ptr_eq(&worker.store, &actual_store));
            assert!(Arc::ptr_eq(
                &worker.remaining,
                &pending.producer.as_ref().unwrap().pending
            ));
            assert!(Arc::ptr_eq(&worker.worker_stream, &stream));
            assert!(Arc::ptr_eq(
                &pending.producer.as_ref().unwrap().stream,
                &stream
            ));
            assert!(
                pending
                    .producer
                    .as_ref()
                    .unwrap()
                    .delivery
                    .0
                    .try_lock()
                    .unwrap()
                    .is_none()
            );
            assert!(pending.producer.as_ref().unwrap().operation.is_none());
            assert!(
                pending
                    .producer
                    .as_ref()
                    .unwrap()
                    .pending
                    .lock()
                    .unwrap()
                    .rows
                    .is_empty()
            );
            assert_eq!(store.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
            assert_eq!(store.body.capacity(), body_capacity);
            assert_eq!(store.body[0], 19);
            assert!(std::ptr::eq(store.body.as_ptr(), body_pointer));
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN: immediate object/queue snapshots only.
    // This closed fixture starts no delivery task and has no store/Operation.
    #[test]
    fn original_reference_layout_counts_queue_capacity_and_refuses_busy_or_used_sink() {
        use crate::failed_evidence::ArchiveError;
        let sink = TranscriptSink::closed_for_source_control();
        assert!(sink.tx.is_closed() && sink.prepare.is_closed());
        assert!(sink.delivery.0.try_lock().unwrap().is_none());
        let stream_pointer = sink.stream.as_ptr();
        let queue_capacity = {
            let mut pending = sink.pending.lock().unwrap();
            pending.rows.reserve_exact(37);
            assert!(matches!(
                sink.original_referenced_layout(),
                Err(ArchiveError::Identity)
            ));
            assert_eq!(pending.ordinal, 0);
            pending.rows.capacity()
        };
        let saved = sink.original_referenced_layout().unwrap();
        assert_eq!(
            saved.pending,
            std::alloc::Layout::for_value(sink.pending.as_ref()).pad_to_align()
        );
        assert_eq!(
            saved.delivery,
            std::alloc::Layout::for_value(sink.delivery.as_ref()).pad_to_align()
        );
        assert_eq!(saved.stream.size(), sink.stream.len());
        assert_eq!(
            saved.queue,
            std::alloc::Layout::array::<Arc<Capture>>(queue_capacity)
                .unwrap()
                .pad_to_align()
        );
        assert_eq!(
            saved.referenced_bytes,
            saved.pending.size() + saved.delivery.size() + saved.stream.size() + saved.queue.size()
        );
        assert!(std::ptr::eq(sink.stream.as_ptr(), stream_pointer));
        {
            let mut pending = sink.pending.lock().unwrap();
            pending.ordinal = 1; // Synthetic prior-use marker, no emitted body.
        }
        assert!(matches!(
            sink.original_referenced_layout(),
            Err(ArchiveError::Identity)
        ));
        let pending = sink.pending.lock().unwrap();
        assert_eq!(pending.ordinal, 1);
        assert_eq!(pending.rows.capacity(), queue_capacity);
        assert!(pending.rows.is_empty());
        assert!(std::ptr::eq(sink.stream.as_ptr(), stream_pointer));
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn original_reference_layout_does_not_clear_poison_or_consume_unwind_payload() {
        use crate::failed_evidence::ArchiveError;
        struct Opaque(std::cell::Cell<usize>);
        let sink = TranscriptSink::closed_for_source_control();
        let payload = Box::new(Opaque(std::cell::Cell::new(53)));
        let pointer = std::ptr::from_ref(payload.as_ref());
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = sink.pending.lock().unwrap();
            std::panic::resume_unwind(payload);
        }))
        .unwrap_err();
        let payload = caught.downcast::<Opaque>().ok().unwrap();
        assert!(std::ptr::eq(payload.as_ref(), pointer));
        assert_eq!(payload.0.get(), 53);
        for _ in 0..2 {
            assert!(matches!(
                sink.original_referenced_layout(),
                Err(ArchiveError::Identity)
            ));
            assert!(sink.pending.is_poisoned());
        }
        assert!(sink.delivery.0.try_lock().unwrap().is_none());
    }

    #[test]
    fn legacy_recovery_row_stays_unattributed_and_shape_preserved() {
        let old = serde_json::json!({"seat":"axis.commander", "at":"old",
            "entry":{"kind":"system","text":"legacy"}});
        let row: UnconfirmedEntry = serde_json::from_value(old.clone()).unwrap();
        assert!(row.origin.is_none());
        assert_eq!(serde_json::to_value(row).unwrap(), old);
    }
}
