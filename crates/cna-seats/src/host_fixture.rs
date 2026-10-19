//! Explicit INERT unit-test host. No native/provider effects, recovery policy or Server proof.
//! The small test backends complete reads synchronously; an actually pending read is CLOSED.
use crate::game::{GameBackend, SubmitReceipt, SubmitRequest};
use crate::host::*;
use crate::operation::{Allowance, COMMAND_BYTES, CommandCredit, Operation, encoded_len};
use crate::transcript::{LocalTranscript, TranscriptStore, UnconfirmedEntry};
use async_trait::async_trait;
use cna_core::{decision::DecisionRequest, ids::SeatId};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

pub(crate) fn accept_inert_context(
    context: HostDriverContext,
) -> Result<(), crate::driver::DriverError> {
    let mut lease = context
        .acquire(HostStep::SessionSpawn)
        .map_err(crate::driver::DriverError::Host)?;
    lease
        .perform_sync(&mut || context.operation.check().map_err(operation_refusal))
        .map_err(crate::driver::DriverError::Host)
}

struct ReplyFootprint(usize);
impl serde::Serialize for ReplyFootprint {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut sequence = serializer.serialize_seq(Some(self.0))?;
        for _ in 0..self.0 {
            sequence.serialize_element(&0_u8)?;
        }
        sequence.end()
    }
}

fn ready<T>(future: impl Future<Output = T>) -> Result<T, HostRefusal> {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(value) => Ok(value),
        std::task::Poll::Pending => Err(HostRefusal::Unknown),
    }
}
pub(crate) struct FixtureHost {
    game: Arc<dyn GameBackend>,
    store: Arc<LocalTranscript>,
    epochs: BTreeMap<SeatId, u64>,
    snapshot: tokio::sync::watch::Sender<HostSnapshot>,
    slot: Arc<Mutex<Option<Arc<Reserved>>>>,
    actor: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl FixtureHost {
    pub(crate) fn explicit(
        game: Arc<dyn GameBackend>,
        store: Arc<LocalTranscript>,
        epochs: BTreeMap<SeatId, u64>,
    ) -> Arc<Self> {
        let initial = HostSnapshot {
            generation: 0,
            activated: true,
            status: HostStatus::Running,
            epochs: epochs.clone(),
            pending: BTreeMap::new(),
        };
        let (snapshot, _) = tokio::sync::watch::channel(initial);
        Arc::new(Self {
            game,
            store,
            epochs,
            snapshot,
            slot: Arc::new(Mutex::new(None)),
            actor: tokio::sync::Mutex::new(None),
        })
    }
    fn validate(&self, identity: &HostIdentity) -> Result<(), HostRefusal> {
        let snapshot = self.snapshot.borrow();
        if !snapshot.activated {
            return Err(HostRefusal::WaitingForActivation);
        }
        if snapshot.status != HostStatus::Running {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        if self.epochs.get(&identity.seat) != Some(&identity.epoch) {
            return Err(HostRefusal::IdentityChanged);
        }
        if !ready(self.game.pending(identity.seat))?
            .iter()
            .any(|p| p.id.as_str() == identity.decision_id && p.revision == identity.revision)
        {
            return Err(HostRefusal::IdentityChanged);
        }
        Ok(())
    }
    pub(crate) fn set_activation(&self, activated: bool) {
        self.snapshot
            .send_modify(|snapshot| snapshot.activated = activated);
    }
    pub(crate) async fn join_actor(&self) {
        let mut retained = self.actor.lock().await;
        if let Some(task) = retained.as_mut() {
            task.await.expect("inert fixture actor join");
        }
        // Remove only after the actual borrowed join completed. Cancellation keeps the owner.
        retained.take();
    }
}
struct Step;
impl HostStepLease for Step {
    fn perform_sync(
        &mut self,
        effect: &mut dyn FnMut() -> Result<(), HostRefusal>,
    ) -> Result<(), HostRefusal> {
        effect()
    }
}
struct Prompt(Allowance);
#[async_trait]
impl HostPromptLease for Prompt {
    fn check(&self) -> Result<(), HostRefusal> {
        self.0.check().map_err(operation_refusal)
    }
    async fn settle(&self) -> Result<(), HostRefusal> {
        Ok(())
    }
}
impl HostAdmission for FixtureHost {
    fn subscribe(&self) -> tokio::sync::watch::Receiver<HostSnapshot> {
        self.snapshot.subscribe()
    }
    fn snapshot(&self) -> Result<HostSnapshot, HostRefusal> {
        let mut value = self.snapshot.borrow().clone();
        value.pending = self
            .epochs
            .keys()
            .map(|s| {
                ready(self.game.pending(*s)).map(|p| {
                    (
                        *s,
                        p.into_iter()
                            .map(|p| (p.id.to_string(), p.revision))
                            .collect(),
                    )
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(value)
    }
    fn acquire(
        &self,
        identity: &HostIdentity,
        _: HostStep,
    ) -> Result<Box<dyn HostStepLease>, HostRefusal> {
        self.validate(identity)?;
        Ok(Box::new(Step))
    }
    fn acquire_prompt(
        &self,
        identity: &HostIdentity,
        allowance: Allowance,
    ) -> Result<Box<dyn HostPromptLease>, HostRefusal> {
        self.validate(identity)?;
        allowance.check().map_err(operation_refusal)?;
        Ok(Box::new(Prompt(allowance)))
    }
}
struct Reserved {
    owner: ForcedReservationOwner,
    request: SubmitRequest,
    route: Mutex<Option<PreparedRoute>>,
    credit: Mutex<CommandCredit<SubmitReceipt>>,
    game: Arc<dyn GameBackend>,
    store: Arc<LocalTranscript>,
    first_marker: Mutex<Option<u64>>,
}
struct Reservation(Arc<Reserved>);
struct Lease(Arc<Reserved>);
struct Delivery {
    reserved: std::sync::Weak<Reserved>,
}
#[async_trait]
impl HostDeliveryRoute for Delivery {
    async fn append(
        &self,
        stream: &str,
        ordinal: u64,
        row: &UnconfirmedEntry,
    ) -> Result<u64, String> {
        let reserved = self
            .reserved
            .upgrade()
            .ok_or("inert fixture reservation closed")?;
        {
            let route = reserved.route.lock().unwrap();
            let route = route.as_ref().ok_or("fixture route missing")?;
            if stream != route.stream() || ordinal <= route.boundary() {
                return Err("fixture stream/ordinal mismatch".into());
            }
        }
        // Keep the ORIGINAL row author. This fixture tests two markers, not Server authority.
        reserved
            .store
            .append(row.seat, row.at.clone(), row.entry.clone())
            .await?;
        Ok(ordinal)
    }
}
impl HostForcedSubmission for FixtureHost {
    fn reserve(
        &self,
        identity: HostIdentity,
        request: SubmitRequest,
        operation: Operation,
    ) -> Result<Box<dyn HostForcedReservation>, HostRefusal> {
        self.validate(&identity)?;
        let mut actor = self
            .actor
            .try_lock()
            .map_err(|_| HostRefusal::StoppedBeforeWork)?;
        if actor.is_some() {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        let mut slot = self.slot.lock().unwrap();
        if slot.is_some() {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        let metadata = (
            &request,
            identity.seat.to_string(),
            identity.epoch,
            &identity.decision_id,
            identity.revision,
            &identity.operation_id,
            &request.idempotency_key,
        );
        let bytes = encoded_len(&metadata, COMMAND_BYTES)
            .map_err(|_| operation_refusal(operation.overflow()))?;
        operation.charge(bytes).map_err(operation_refusal)?;
        let mut credit = CommandCredit::new(operation.clone()).map_err(operation_refusal)?;
        credit
            .admit_settlement(&metadata)
            .map_err(operation_refusal)?;
        let owner = ForcedReservationOwner::new(identity, operation)?;
        let reserved = Arc::new(Reserved {
            owner,
            request,
            route: Mutex::new(None),
            credit: Mutex::new(credit),
            game: self.game.clone(),
            store: self.store.clone(),
            first_marker: Mutex::new(None),
        });
        *slot = Some(reserved.clone());
        // The test actor listens to the SAME preallocated settlement signal. This is not a
        // production alternative lane or timer, and the test harness owns/joins its task.
        let retained = reserved.clone();
        let actual_slot = self.slot.clone();
        let handle = retained.owner.cancellation();
        let task = tokio::spawn(async move {
            handle.settlement_notified().await;
            {
                let mut slot = actual_slot.lock().unwrap();
                assert!(slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, &retained)));
                slot.take(); // actual fixture final transition BEFORE retained actor ACK
            }
            retained
                .owner
                .acknowledge_settlement(&handle, false)
                .unwrap();
            retained.owner.operation().settle();
        });
        *actor = Some(task);
        // Registration cannot grant admission. A postacceptance refusal still returns the
        // abort-only reservation so the original caller can reach its retained terminal ACK.
        let _registration = reserved.owner.register_writer_wake(
            std::thread::current(),
            std::mem::size_of::<std::thread::Thread>(),
        );
        Ok(Box::new(Reservation(reserved)))
    }
}
#[async_trait]
impl HostForcedReservation for Reservation {
    fn identity(&self) -> &HostIdentity {
        self.0.owner.identity()
    }
    fn operation(&self) -> Operation {
        self.0.owner.operation()
    }
    fn cancellation(&self) -> Arc<RouteCancellation> {
        self.0.owner.cancellation()
    }
    fn route(&self) -> ReservedForcedRoute {
        self.0.owner.route()
    }
    async fn begin(&self, route: PreparedRoute) -> Result<Box<dyn HostForcedLease>, HostRefusal> {
        if !Arc::ptr_eq(&self.cancellation(), &route.cancellation())
            || route.owner() != self.identity()
        {
            return Err(HostRefusal::IdentityChanged);
        }
        let mut retained = self.0.route.lock().unwrap();
        if retained.is_some() {
            return Err(HostRefusal::IdentityChanged);
        }
        *retained = Some(route);
        retained.as_ref().unwrap().install(
            "fixture-token".into(),
            Arc::new(Delivery {
                reserved: Arc::downgrade(&self.0),
            }),
        )?;
        Ok(Box::new(Lease(self.0.clone())))
    }
    async fn abort(&self) -> Result<(), HostRefusal> {
        self.cancellation().request();
        Ok(())
    }
    async fn settle(&self) -> Result<(), HostRefusal> {
        self.cancellation().settlement_ack().await;
        Ok(())
    }
}
#[async_trait]
impl HostForcedLease for Lease {
    fn token(&self) -> &str {
        "fixture-token"
    }
    fn cancellation(&self) -> Arc<RouteCancellation> {
        self.0.owner.cancellation()
    }
    async fn commit(&self, intent: ConfirmedIntent) -> Result<SubmitReceipt, HostRefusal> {
        {
            let route = self.0.route.lock().unwrap();
            let route = route.as_ref().ok_or(HostRefusal::Unknown)?;
            if intent.owner() != self.0.owner.identity()
                || intent.stream() != route.stream()
                || intent.token() != self.token()
                || intent.ordinal() < route.boundary()
            {
                return Err(HostRefusal::IdentityChanged);
            }
            *self.0.first_marker.lock().unwrap() = Some(intent.ordinal());
        }
        self.0
            .owner
            .operation()
            .check()
            .map_err(operation_refusal)?;
        let request_bytes = encoded_len(&self.0.request, COMMAND_BYTES)
            .map_err(|_| operation_refusal(self.0.owner.operation().overflow()))?;
        self.0
            .owner
            .operation()
            .charge(request_bytes)
            .map_err(operation_refusal)?;
        let receipt = self
            .0
            .game
            .submit(self.0.owner.identity().seat, self.0.request.clone())
            .await
            .map_err(|e| HostRefusal::Ownership(e.to_string()))?;
        let mut credit = self.0.credit.lock().unwrap();
        credit.retain_receipt(receipt).map_err(operation_refusal)?;
        let receipt = credit
            .receipt()
            .expect("canonical accepted fixture receipt");
        let reply_bytes = encoded_len(receipt, COMMAND_BYTES)
            .map_err(|_| operation_refusal(credit.operation().overflow()))?;
        // Fixture bookkeeping remains conservative: charge the actual full reply before clone.
        // Nonallocating sequence encoding is >= full encoded reply length and occupies live
        // COMMAND and cumulative Operation. This is not an RSS/Server fit proof.
        credit
            .admit_settlement(&ReplyFootprint(reply_bytes))
            .map_err(operation_refusal)?;
        Ok(credit.receipt().unwrap().clone())
    }
    async fn finish_delivery(
        &self,
        key: &str,
        marker: ConfirmedPostcommit,
    ) -> Result<Vec<DecisionRequest>, HostRefusal> {
        {
            let route = self.0.route.lock().unwrap();
            let route = route.as_ref().ok_or(HostRefusal::Unknown)?;
            let first = self
                .0
                .first_marker
                .lock()
                .unwrap()
                .ok_or(HostRefusal::Unknown)?;
            if key != self.0.request.idempotency_key
                || marker.owner() != self.0.owner.identity()
                || marker.stream() != route.stream()
                || marker.token() != self.token()
                || marker.ordinal() < first
            {
                return Err(HostRefusal::IdentityChanged);
            }
        }
        Ok(self.0.game.pending(self.0.owner.identity().seat).await)
    }
    async fn abort(&self) -> Result<(), HostRefusal> {
        self.cancellation().request();
        Ok(())
    }
    async fn settle(&self) -> Result<(), HostRefusal> {
        self.cancellation().settlement_ack().await;
        Ok(())
    }
}
