//! Required trusted-host admission. Nothing here is a seat tool or spectator payload.
//!
//! The Server adapter orders permission against its single writer. A snapshot is advisory;
//! every effect needs a fresh permit. Native cleanup remains owned when permission closes.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostStatus { Running, Paused, Finished, Unknown }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSnapshot {
    pub generation: u64,
    pub activated: bool,
    pub status: HostStatus,
    pub epochs: BTreeMap<SeatId, u64>,
    pub pending: BTreeMap<SeatId, Vec<(String, u32)>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostStep { Reserve, PreflightSpawn, SessionSpawn, Resume, Reseed }

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
    fn perform_sync(&mut self, effect: &mut dyn FnMut() -> Result<(), HostRefusal>) -> Result<(), HostRefusal>;
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
    fn acquire(&self, identity: &HostIdentity, step: HostStep) -> Result<Box<dyn HostStepLease>, HostRefusal>;
    fn acquire_prompt(&self, identity: &HostIdentity, allowance: Allowance) -> Result<Box<dyn HostPromptLease>, HostRefusal>;
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
        self.admission.acquire_prompt(&self.identity, self.operation.allowance())
    }
}

pub fn operation_refusal(error: crate::operation::OperationError) -> HostRefusal {
    use crate::operation::OperationError;
    match error {
        OperationError::NoAllowance => HostRefusal::NoAllowance,
        OperationError::Expired => HostRefusal::Expired,
        OperationError::CaptureOverflow => HostRefusal::CaptureOverflow,
        other => HostRefusal::Ownership(other.to_string()),
    }
}

/// Original-author append validation remains the adapter's responsibility. Routing cannot grant
/// the forced owner permission to author someone else's payload or change its epoch.
#[async_trait]
pub trait HostDeliveryRoute: Send + Sync {
    async fn append(&self, stream: &str, ordinal: u64, row: &UnconfirmedEntry) -> Result<u64, String>;
}

pub(crate) enum RouteState {
    Prepared,
    Installed { token: String, delivery: Arc<dyn HostDeliveryRoute> },
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

struct RouteSettlementState {
    writer_wake: Option<std::thread::Thread>,
    acknowledging: bool,
    ack: Option<RouteSettlementAck>,
}

/// Actor disposition only. This does not witness child, reader or storage joins.
/// The constructor is private; the actor can obtain it only through its SAME route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteSettlementAck { uncertain: bool }

impl RouteSettlementAck {
    pub fn uncertain(self) -> bool { self.uncertain }
}

impl RouteCancellation {
    pub(crate) fn new() -> Self {
        Self {
            requested: AtomicBool::new(false), changed: Notify::new(),
            settlement_requested: AtomicBool::new(false), settlement_changed: Notify::new(),
            settlement: Mutex::new(RouteSettlementState { writer_wake: None, acknowledging: false, ack: None }),
            acknowledged: Notify::new(),
        }
    }
    fn wake_writer(&self) {
        let wake = self.settlement.lock().expect("route settlement").writer_wake.clone();
        // A concrete Thread handle cannot hide arbitrary closure captures. Its unpark runs
        // outside exclusion; it requests writer attention, never admission or settlement.
        if let Some(wake) = wake { wake.unpark(); }
    }
    pub fn request(&self) {
        if self.requested.swap(true, Ordering::AcqRel) { return; }
        self.changed.notify_one();
        self.wake_writer();
    }
    pub fn requested(&self) -> bool { self.requested.load(Ordering::Acquire) }
    pub async fn notified(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.requested() { return; }
            notified.await;
        }
    }

    /// Trusted caller only, AFTER actual original supervision disposition. A deadline or
    /// dropped begin future calls `request`, never this method. No credit is released here.
    pub fn request_settlement_after_supervision(&self) {
        if self.settlement_requested.swap(true, Ordering::AcqRel) { return; }
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
            if self.settlement_requested() { return; }
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
            if let Some(ack) = self.acknowledged_settlement() { return ack; }
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

impl PreparedRoute {
    pub fn stream(&self) -> &str { &self.stream }
    pub fn owner(&self) -> &HostIdentity { &self.owner }
    pub fn boundary(&self) -> u64 { self.boundary }
    pub fn allowance(&self) -> Allowance { self.allowance }
    pub fn cancellation(&self) -> Arc<RouteCancellation> { self.cancel.clone() }
    /// The SAME cumulative meter and operation credit, never a new identity-only meter.
    pub fn operation(&self) -> Operation { self.operation.clone() }

    /// Adapter registers its existing parked writer BEFORE enqueueing begin. Its closure
    /// retains only a concrete Thread handle. At most one retained handle and three temporary
    /// clones exist: registration's prior-sticky wake, first abort, first settlement. Their
    /// actual handle bytes fit the existing PREPARE_BYTES reservation charged at prepare.
    /// Variable metadata is not passed/stored here; its declared size cannot grant admission
    /// or excuse its own producer's original-domain byte accounting.
    /// This is the SAME capability association, not a registry keyed by model-supplied text.
    pub fn register_writer_wake(&self, writer: std::thread::Thread, metadata_bytes: usize)
        -> Result<(), HostRefusal>
    {
        let mut state = self.cancel.settlement.lock().expect("route settlement");
        if state.writer_wake.is_some() || state.ack.is_some() {
            return Err(HostRefusal::IdentityChanged);
        }
        state.writer_wake = Some(writer);
        drop(state);
        if self.cancel.requested() || self.cancel.settlement_requested() { self.cancel.wake_writer(); }
        // Even an oversized declared variable body refuses WITHOUT retaining that body,
        // while this fixed precharged cleanup association can still receive terminal ACK.
        if metadata_bytes > crate::operation::PREPARE_BYTES {
            self.operation.overflow();
            self.cancel.request();
            return Err(HostRefusal::CaptureOverflow);
        }
        // Register cleanup BEFORE reporting a cap/deadline refusal. The actor/pending owner
        // retains this SAME route on error; no effect may be enqueued/admitted by this result.
        match self.operation.check() {
            Ok(()) => Ok(()),
            Err(error) => { self.cancel.request(); Err(operation_refusal(error)) }
        }
    }

    /// The actor calls this only AFTER its actual final transition. It keeps canonical
    /// receipt/credit/prefix ownership until that transition; request ACKs free nothing.
    /// Lost replies and duplicate settlement use this retained witness, without another begin.
    pub fn acknowledge_settlement(&self, handle: &Arc<RouteCancellation>, uncertain: bool)
        -> Result<RouteSettlementAck, HostRefusal>
    {
        if !Arc::ptr_eq(&self.cancel, handle) || !handle.settlement_requested() {
            return Err(HostRefusal::IdentityChanged);
        }
        let ack = RouteSettlementAck { uncertain };
        let mut state = handle.settlement.lock().expect("route settlement");
        if state.writer_wake.is_none() { return Err(HostRefusal::IdentityChanged); }
        if let Some(previous) = state.ack {
            return if previous == ack { Ok(previous) } else { Err(HostRefusal::IdentityChanged) };
        }
        if state.acknowledging {
            return Err(HostRefusal::Ownership("settlement transition in progress".into()));
        }
        state.acknowledging = true;
        drop(state);
        // Route disposition precedes observable ACK. This never settles Operation itself.
        self.release(uncertain);
        let mut state = handle.settlement.lock().expect("route settlement");
        state.ack = Some(ack);
        drop(state);
        handle.acknowledged.notify_waiters();
        Ok(ack)
    }

    /// Called by the single writer within its coherent begin transition BEFORE replying. This
    /// changes issue routing, never an already-issued append's author or payload.
    pub fn install(&self, token: String, delivery: Arc<dyn HostDeliveryRoute>) -> Result<(), HostRefusal> {
        self.operation.check().map_err(operation_refusal)?;
        if self.cancel.requested() || token.is_empty() { return Err(HostRefusal::StoppedBeforeWork); }
        let mut state = self.state.lock().expect("prepared route");
        if !matches!(*state, RouteState::Prepared) { return Err(HostRefusal::IdentityChanged); }
        *state = RouteState::Installed { token, delivery };
        drop(state);
        self.changed.notify_one();
        Ok(())
    }

    /// Authoritative actor settlement only; release of a route is distinct from worker/storage
    /// join. Unknown append ACKs retain their prefix and never fall through to blind normal retry.
    pub fn release(&self, uncertain: bool) {
        *self.state.lock().expect("prepared route") = if uncertain { RouteState::Uncertain } else { RouteState::Released };
        self.changed.notify_one();
    }
}

impl Drop for PreparedRoute {
    fn drop(&mut self) { self.cancel.request(); }
}

pub struct ConfirmedIntent { pub(crate) marker: MarkerWitness }
pub struct ConfirmedPostcommit { pub(crate) marker: MarkerWitness }

pub(crate) struct MarkerWitness {
    pub stream: String,
    pub token: String,
    pub owner: HostIdentity,
    pub ordinal: u64,
}

macro_rules! marker_accessors {
    ($name:ident) => {
        impl $name {
            pub fn stream(&self) -> &str { &self.marker.stream }
            pub fn token(&self) -> &str { &self.marker.token }
            pub fn owner(&self) -> &HostIdentity { &self.marker.owner }
            pub fn ordinal(&self) -> u64 { self.marker.ordinal }
        }
    };
}
marker_accessors!(ConfirmedIntent);
marker_accessors!(ConfirmedPostcommit);

#[async_trait]
pub trait HostForcedSubmission: Send + Sync {
    async fn begin(&self, identity: HostIdentity, request: SubmitRequest, route: PreparedRoute,
        allowance: Allowance) -> Result<Box<dyn HostForcedLease>, HostRefusal>;
}

#[async_trait]
pub trait HostForcedLease: Send + Sync {
    fn token(&self) -> &str;
    fn cancellation(&self) -> Arc<RouteCancellation>;
    async fn commit(&self, intent: ConfirmedIntent) -> Result<SubmitReceipt, HostRefusal>;
    /// `receipt_identity` is the ORIGINAL request.idempotency_key, never decision_id alone.
    async fn finish_delivery(&self, receipt_identity: &str, marker: ConfirmedPostcommit)
        -> Result<Vec<DecisionRequest>, HostRefusal>;
    async fn abort(&self) -> Result<(), HostRefusal>;
    async fn settle(&self) -> Result<(), HostRefusal>;
}

#[cfg(test)]
pub(crate) fn test_prepared_route(operation_id: &str) -> PreparedRoute {
    test_prepared_route_with_allowance(operation_id, std::time::Duration::from_secs(5))
}

#[cfg(test)]
pub(crate) fn test_prepared_route_with_allowance(operation_id: &str, duration: std::time::Duration) -> PreparedRoute {
    let allowance = Allowance::required(Some(tokio::time::Instant::now()
        + duration)).unwrap();
    let operation = Operation::new(operation_id.into(), 1, allowance).unwrap();
    operation.charge(std::mem::size_of::<RouteCancellation>() + crate::operation::PREPARE_BYTES).unwrap();
    PreparedRoute {
        stream: "test-stream".into(), owner: HostIdentity { seat: crate::toy::AXIS,
            epoch: 7, decision_id: "test-decision".into(), revision: 1,
            operation_id: operation_id.into() }, boundary: 0, allowance,
        state: Arc::new(Mutex::new(RouteState::Prepared)), changed: Arc::new(Notify::new()),
        cancel: Arc::new(RouteCancellation::new()),
        operation,
    }
}

#[cfg(test)]
mod settlement_tests {
    use super::*;

    struct Delivery;
    #[async_trait]
    impl HostDeliveryRoute for Delivery {
        async fn append(&self, _: &str, ordinal: u64, _: &UnconfirmedEntry) -> Result<u64, String> {
            Ok(ordinal)
        }
    }

    #[tokio::test]
    async fn lost_begin_reply_retains_tokenless_actual_settlement_ack() {
        let route = test_prepared_route("same-op");
        let handle = route.cancellation(); // retained BEFORE install/possible begin reply
        route.register_writer_wake(std::thread::current(), std::mem::size_of::<std::thread::Thread>()).unwrap();
        route.install("private-token".into(), Arc::new(Delivery)).unwrap();
        handle.request(); // begin reply lost/timeout: abort ONLY, no ACK or route release
        assert!(handle.acknowledged_settlement().is_none());
        assert!(!handle.settlement_requested());
        assert!(matches!(*route.state.lock().unwrap(), RouteState::Installed { .. }));
        assert_eq!(route.acknowledge_settlement(&handle, false), Err(HostRefusal::IdentityChanged));
        // Test's original work has now actually settled; this explicit phase is required.
        handle.request_settlement_after_supervision();
        let ack = route.acknowledge_settlement(&handle, false).unwrap();
        assert_eq!(handle.settlement_ack().await, ack);
        assert_eq!(route.acknowledge_settlement(&handle, false).unwrap(), ack);
        assert_eq!(route.acknowledge_settlement(&handle, true), Err(HostRefusal::IdentityChanged));
        assert!(matches!(*route.state.lock().unwrap(), RouteState::Released));
        assert!(route.operation().check().is_ok(), "actor ACK is not an Operation/child join");
    }

    #[test]
    fn same_text_identity_is_not_the_same_capability_and_registration_keeps_fixed_thread_only() {
        let route = test_prepared_route("same-text");
        let other = test_prepared_route("same-text");
        let handle = route.cancellation();
        handle.request(); // request BEFORE registration must not lose the writer wake
        route.register_writer_wake(std::thread::current(), std::mem::size_of::<std::thread::Thread>()).unwrap();
        assert_eq!(handle.settlement.lock().unwrap().writer_wake.as_ref().unwrap().id(), std::thread::current().id());
        assert!(std::mem::size_of::<std::thread::Thread>() * 4 <= crate::operation::PREPARE_BYTES);
        assert_eq!(route.register_writer_wake(std::thread::current(), 8), Err(HostRefusal::IdentityChanged));
        handle.request_settlement_after_supervision();
        let stale = other.cancellation();
        stale.request_settlement_after_supervision();
        assert_eq!(route.acknowledge_settlement(&stale, false), Err(HostRefusal::IdentityChanged));
        assert!(handle.acknowledged_settlement().is_none());
        route.acknowledge_settlement(&handle, true).unwrap();
        assert!(handle.acknowledged_settlement().unwrap().uncertain());
    }

    #[tokio::test]
    async fn overflow_before_enqueue_refuses_effect_but_keeps_terminal_wake_and_ack() {
        let route = test_prepared_route("pre-enqueue-overflow");
        let handle = route.cancellation();
        let operation = route.operation();
        let remaining = crate::operation::OPERATION_BYTES - operation.captured_bytes();
        operation.charge(remaining).unwrap();
        assert_eq!(operation.charge(1), Err(crate::operation::OperationError::CaptureOverflow));
        let before = operation.captured_bytes();
        assert_eq!(route.register_writer_wake(std::thread::current(), std::mem::size_of::<std::thread::Thread>()), Err(HostRefusal::CaptureOverflow));
        assert_eq!(operation.captured_bytes(), before, "cleanup metadata was precharged");
        assert!(handle.requested());
        assert!(handle.settlement.lock().unwrap().writer_wake.is_some());
        assert_eq!(route.install("never-delivered".into(), Arc::new(Delivery)),
            Err(HostRefusal::CaptureOverflow));
        assert!(matches!(*route.state.lock().unwrap(), RouteState::Prepared));
        assert!(handle.acknowledged_settlement().is_none());
        handle.request_settlement_after_supervision();
        let ack = route.acknowledge_settlement(&handle, false).unwrap();
        assert_eq!(handle.settlement_ack().await, ack);
        assert_eq!(route.acknowledge_settlement(&handle, false).unwrap(), ack);
        assert_eq!(operation.check(), Err(crate::operation::OperationError::CaptureOverflow));
    }

    #[tokio::test]
    async fn oversized_metadata_refuses_before_install_and_retains_only_terminal_thread_wake() {
        let route = test_prepared_route("oversized-metadata");
        let handle = route.cancellation();
        let bytes = route.operation().captured_bytes();
        assert_eq!(route.register_writer_wake(std::thread::current(), crate::operation::PREPARE_BYTES + 1),
            Err(HostRefusal::CaptureOverflow));
        assert_eq!(route.operation().captured_bytes(), bytes, "no oversized body was retained");
        assert!(handle.requested());
        assert!(handle.settlement.lock().unwrap().writer_wake.is_some());
        assert_eq!(route.install("refused".into(), Arc::new(Delivery)), Err(HostRefusal::CaptureOverflow));
        assert!(matches!(*route.state.lock().unwrap(), RouteState::Prepared));
        assert!(handle.acknowledged_settlement().is_none());
        handle.request_settlement_after_supervision();
        let ack = route.acknowledge_settlement(&handle, false).unwrap();
        assert_eq!(handle.settlement_ack().await, ack);
    }

    #[tokio::test]
    async fn expired_before_registration_keeps_terminal_ack_but_never_installs() {
        let route = test_prepared_route_with_allowance("expired-registration", std::time::Duration::from_millis(100));
        let handle = route.cancellation();
        tokio::time::sleep_until(route.allowance().deadline()).await;
        assert_eq!(route.register_writer_wake(std::thread::current(), std::mem::size_of::<std::thread::Thread>()),
            Err(HostRefusal::Expired));
        assert!(handle.requested());
        assert!(handle.settlement.lock().unwrap().writer_wake.is_some());
        assert_eq!(route.install("never-started".into(), Arc::new(Delivery)), Err(HostRefusal::Expired));
        assert!(matches!(*route.state.lock().unwrap(), RouteState::Prepared));
        handle.request_settlement_after_supervision();
        let ack = route.acknowledge_settlement(&handle, false).unwrap();
        assert_eq!(handle.settlement_ack().await, ack);
    }
}
