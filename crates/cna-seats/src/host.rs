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
}

impl RouteCancellation {
    pub(crate) fn new() -> Self {
        Self { requested: AtomicBool::new(false), changed: Notify::new() }
    }
    pub fn request(&self) {
        self.requested.store(true, Ordering::Release);
        self.changed.notify_one();
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
}

impl PreparedRoute {
    pub fn stream(&self) -> &str { &self.stream }
    pub fn owner(&self) -> &HostIdentity { &self.owner }
    pub fn boundary(&self) -> u64 { self.boundary }
    pub fn allowance(&self) -> Allowance { self.allowance }
    pub fn cancellation(&self) -> Arc<RouteCancellation> { self.cancel.clone() }

    /// Called by the single writer within its coherent begin transition BEFORE replying. This
    /// changes issue routing, never an already-issued append's author or payload.
    pub fn install(&self, token: String, delivery: Arc<dyn HostDeliveryRoute>) -> Result<(), HostRefusal> {
        self.allowance.check().map_err(operation_refusal)?;
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
