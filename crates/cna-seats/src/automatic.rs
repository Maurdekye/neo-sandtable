//! Forced answers inferred only from the seat-visible action space, never game knowledge.
use crate::{
    game::{GameBackend, SubmitRequest, ToolError},
    transcript::{TranscriptEntry, TranscriptSink},
};
use cna_core::{
    decision::{ActionSchema, ActionSpace, DecisionRequest},
    ids::SeatId,
};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};
use crate::host::{HostForcedLease, HostForcedSubmission, HostIdentity, HostRefusal,
    PreparedRoute, RouteCancellation, RouteSettlementAck};
use crate::operation::Allowance;

type RetainedBegin = Pin<Box<dyn Future<Output = Result<Box<dyn HostForcedLease>, HostRefusal>> + Send>>;

struct BeginAbortOnDrop { handle: Arc<RouteCancellation>, armed: bool }
impl Drop for BeginAbortOnDrop {
    fn drop(&mut self) { if self.armed { self.handle.request(); } }
}

/// Concrete retained owner/capture representations, not heap payloads or an RSS estimate.
/// Variable request/identity payloads retain their separate producer/COMMAND charges.
pub(crate) fn retained_begin_metadata_bytes() -> usize {
    std::mem::size_of::<ForcedAnswerSupervisor>()
        + std::mem::size_of::<BeginAbortOnDrop>()
        + std::mem::size_of::<(Arc<dyn HostForcedSubmission>, HostIdentity, SubmitRequest, PreparedRoute, Allowance)>()
}

/// Foreground caller ownership, allocated BEFORE polling/enqueueing begin. The containing
/// runner must retain this value on every uncertain return; it is never a detached task.
/// The preallocated handle survives loss of the begin reply/token. Dropping it asks abort
/// but cannot release a credit, canonical receipt, child or reader.
pub struct ForcedAnswerSupervisor {
    handle: Arc<RouteCancellation>,
    allowance: Allowance,
    begin: Option<RetainedBegin>,
    lease: Option<Box<dyn HostForcedLease>>,
    settled: Option<RouteSettlementAck>,
}

impl ForcedAnswerSupervisor {
    pub fn new(host: Arc<dyn HostForcedSubmission>, identity: HostIdentity,
        request: SubmitRequest, route: PreparedRoute) -> Result<Self, HostRefusal>
    {
        if route.owner() != &identity { return Err(HostRefusal::IdentityChanged); }
        // Fixed owner/future metadata was charged at successful prepare. A post-prepare
        // overflow/expiry must not refuse BEFORE owning the terminal registration future.
        let allowance = route.allowance();
        let handle = route.cancellation();
        // The actor adapter registers the SAME handle/writer wake before its first enqueue.
        // The caller retains both the future and handle before that future can be polled.
        let begin = Box::pin(async move { host.begin(identity, request, route, allowance).await });
        Ok(Self { handle, allowance, begin: Some(begin), lease: None, settled: None })
    }

    pub fn cancellation(&self) -> Arc<RouteCancellation> { self.handle.clone() }
    pub fn lease(&self) -> Option<&dyn HostForcedLease> { self.lease.as_deref() }
    pub fn settled(&self) -> Option<RouteSettlementAck> { self.settled }

    /// One original whole-answer deadline; timeout borrows rather than drops begin.
    /// A lost reply leaves ownership here and in the actor's registered association.
    pub async fn begin(&mut self) -> Result<(), HostRefusal> {
        if self.lease.is_some() { return Ok(()); }
        if self.settled.is_some() { return Err(HostRefusal::StoppedBeforeWork); }
        let mut abort = BeginAbortOnDrop { handle: self.handle.clone(), armed: true };
        let pending = self.begin.as_mut().ok_or(HostRefusal::Unknown)?;
        match tokio::time::timeout_at(self.allowance.deadline(), pending.as_mut()).await {
            Ok(result) => {
                self.begin = None;
                match result {
                    Ok(lease) => { self.lease = Some(lease); abort.armed = false; Ok(()) }
                    Err(error) => { self.handle.request(); Err(error) }
                }
            }
            Err(_) => { self.handle.request(); Err(HostRefusal::Expired) }
        }
    }

    /// Called only after the original supervision has actually disposed of its owned work.
    /// Even if begin never delivered a token, the pre-enqueue association receives this wake.
    /// Actual actor ACK is retained separately; no timeout or Drop substitutes for it.
    pub fn request_settlement_after_supervision(&self) {
        self.handle.request(); // late first poll may only establish terminal association
        self.handle.request_settlement_after_supervision();
    }

    /// Expiry can close the caller's wait but leaves this foreground owner intact. The
    /// containing runner must retain it and report Unsettled, never healthy finish/re-admit.
    pub async fn settlement_ack(&mut self) -> Result<RouteSettlementAck, HostRefusal> {
        if let Some(ack) = self.settled { return Ok(ack); }
        if !self.handle.settlement_requested() {
            return Err(HostRefusal::Ownership("supervision settlement not requested".into()));
        }
        // A caller may be canceled before begin was ever polled. Drive its first bounded
        // poll here so the adapter registers the SAME pending capability before observing
        // abort/expiry. No new task is spawned; actual actor ACK remains the only witness.
        if let Some(pending) = self.begin.as_mut() {
            let completion = std::future::poll_fn(|cx| {
                std::task::Poll::Ready(pending.as_mut().poll(cx))
            }).await;
            if let std::task::Poll::Ready(result) = completion {
                self.begin = None;
                if let Ok(lease) = result { self.lease = Some(lease); }
            }
        }
        let ack = match self.handle.acknowledged_settlement() {
            Some(ack) => ack,
            None => tokio::time::timeout_at(self.allowance.deadline(), self.handle.settlement_ack())
                .await.map_err(|_| HostRefusal::Expired)?,
        };
        self.settled = Some(ack);
        // Only the actual retained actor witness permits forgetting begin/lease futures.
        self.begin = None;
        self.lease = None;
        Ok(ack)
    }
}

impl Drop for ForcedAnswerSupervisor {
    fn drop(&mut self) { if self.settled.is_none() { self.handle.request(); } }
}

fn empty(schema: &ActionSchema) -> bool {
    match schema {
        ActionSchema::Choice { options } => options.is_empty(),
        ActionSchema::Unit { among } => among.is_empty(),
        ActionSchema::Hex { among: Some(hexes) } => hexes.is_empty(),
        ActionSchema::Integer { min, max } => min > max,
        ActionSchema::Record { fields } => fields.iter().any(|f| !f.optional && empty(&f.schema)),
        ActionSchema::List { item, min, max } => min > max || (*min > 0 && empty(item)),
        _ => false,
    }
}
/// An explicit pass with no selectable action, or only a zero-item list. An optional
/// empty field, an unenumerated domain, a valid scalar or a freely chosen empty list
/// cannot prove force. Null is never inferred from option labels or decision kinds.
pub fn forced_pass(space: &ActionSpace) -> bool {
    space.pass.is_some()
        && (empty(&space.schema)
            || matches!(&space.schema,
        ActionSchema::List { item, min: 0, max, .. } if *max == 0 || empty(item)))
}

/// Trusted local adapter: the same own-seat game boundary, original epoch and exact
/// revision as MCP submissions. Refusals propagate; this is never failure fallback.
pub async fn answer(
    game: &dyn GameBackend,
    seat: SeatId,
    epoch: u64,
    request: &DecisionRequest,
    sink: &TranscriptSink,
) -> Result<bool, ToolError> {
    if request.seat != seat {
        return Err(ToolError::UnknownDecision(request.id.to_string()));
    }
    if !forced_pass(&request.space) {
        return Ok(false);
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let current = game.pending(seat).await;
        if !current.iter().any(|p| {
            p.id == request.id && p.revision == request.revision && p.space == request.space
        }) {
            return Err(ToolError::Stale("forced answer window changed".into()));
        }
        sink.system(
            seat,
            format!(
                "Automatic forced answer attempt: pass for {} revision {}; no model invoked",
                request.id, request.revision
            ),
        );
        // Confirm the intent before committing the action, so even a process death
        // between accepted receipt and capture leaves a visible automatic intent.
        sink.flush_confirmed().await.map_err(ToolError::Other)?;
        let receipt = game
            .submit(
                seat,
                SubmitRequest {
                    decision_id: request.id.to_string(),
                    epoch,
                    revision: Some(request.revision),
                    idempotency_key: format!(
                        "automatic|{seat}|{}|e{epoch}|r{}|null",
                        request.id, request.revision
                    ),
                    action: Value::Null,
                    public_explanation: None,
                },
            )
            .await?;
        if !receipt.duplicate {
            sink.emit(
                seat,
                TranscriptEntry::DecisionSubmitted {
                    decision_id: receipt.decision_id,
                    summary: "automatic forced answer: pass (no model invoked)".into(),
                },
            );
        }
        sink.flush_confirmed().await.map_err(ToolError::Other)?;
        if game
            .pending(seat)
            .await
            .iter()
            .any(|p| p.id == request.id && p.revision == request.revision)
        {
            return Err(ToolError::Other(
                "automatic answer left its decision revision pending".into(),
            ));
        }
        Ok(true)
    })
    .await
    .map_err(|_| ToolError::Other("automatic answer or transcript delivery timed out".into()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        driver::{CliKind, DriverError, SeatDriver, SessionInfo, TurnOutcome},
        mcp::ToolRouter,
        memory::InMemorySeatMemory,
        run::{DefaultPrompts, InMemorySessions, RunLimits, SeatEnd, SeatRunner, SeatState},
        toy::{AXIS, NumberDuel},
        transcript::LocalTranscript,
    };
    use async_trait::async_trait;
    use cna_core::decision::FieldSchema;
    use cna_protocol::SeatStatus;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use tokio::sync::{Semaphore, watch};

    struct LostBegin {
        retained: Mutex<Option<PreparedRoute>>,
        installed: tokio::sync::Notify,
    }
    struct BeginDelivery;
    #[async_trait]
    impl crate::host::HostDeliveryRoute for BeginDelivery {
        async fn append(&self, _: &str, ordinal: u64, _: &crate::transcript::UnconfirmedEntry)
            -> Result<u64, String> { Ok(ordinal) }
    }
    #[async_trait]
    impl HostForcedSubmission for LostBegin {
        async fn begin(&self, _: HostIdentity, _: SubmitRequest, route: PreparedRoute, _: Allowance)
            -> Result<Box<dyn HostForcedLease>, HostRefusal>
        {
            *self.retained.lock().unwrap() = Some(route);
            {
                let retained = self.retained.lock().unwrap();
                let route = retained.as_ref().unwrap();
                route.register_writer_wake(std::thread::current(), std::mem::size_of::<std::thread::Thread>())?;
                route.install("undelivered-token".into(), Arc::new(BeginDelivery))?;
            }
            self.installed.notify_one();
            std::future::pending().await // actor installed, but token/reply is lost
        }
    }

    #[tokio::test]
    async fn canceled_begin_wait_keeps_foreground_future_until_actual_actor_ack() {
        let host = Arc::new(LostBegin { retained: Mutex::new(None), installed: Default::default() });
        let route = crate::host::test_prepared_route("lost-begin-op");
        let identity = route.owner().clone();
        let request = SubmitRequest { decision_id: identity.decision_id.clone(), epoch: identity.epoch,
            revision: Some(identity.revision), idempotency_key: "original-request-key".into(),
            action: Value::Null, public_explanation: None };
        let mut owner = ForcedAnswerSupervisor::new(host.clone(), identity, request, route).unwrap();
        tokio::select! {
            _ = owner.begin() => panic!("lost begin cannot return a lease"),
            _ = host.installed.notified() => {},
        }
        assert!(owner.begin.is_some(), "canceled borrowed wait must retain begin future");
        assert!(owner.lease().is_none());
        assert!(owner.cancellation().requested());
        assert!(!owner.cancellation().settlement_requested());
        assert_eq!(owner.settlement_ack().await,
            Err(HostRefusal::Ownership("supervision settlement not requested".into())));
        assert!(owner.begin.is_some());
        // Original supervisor's actual cleanup precedes settlement request, never the timer.
        owner.request_settlement_after_supervision();
        let route = host.retained.lock().unwrap().take().unwrap();
        route.acknowledge_settlement(&owner.cancellation(), false).unwrap();
        let ack = owner.settlement_ack().await.unwrap();
        assert_eq!(owner.settlement_ack().await.unwrap(), ack);
        assert!(owner.begin.is_none());
        assert_eq!(owner.settled(), Some(ack));
    }

    #[tokio::test]
    async fn post_prepare_overflow_retains_caller_and_actor_association_before_refusal() {
        let host = Arc::new(LostBegin { retained: Mutex::new(None), installed: Default::default() });
        let route = crate::host::test_prepared_route("overflow-before-caller");
        let identity = route.owner().clone();
        route.operation().overflow();
        let request = SubmitRequest { decision_id: identity.decision_id.clone(), epoch: identity.epoch,
            revision: Some(identity.revision), idempotency_key: "overflow-original-key".into(),
            action: Value::Null, public_explanation: None };
        let mut owner = ForcedAnswerSupervisor::new(host.clone(), identity, request, route).unwrap();
        assert_eq!(owner.begin().await, Err(HostRefusal::CaptureOverflow));
        assert!(host.retained.lock().unwrap().is_some());
        assert!(owner.cancellation().acknowledged_settlement().is_none());
        owner.request_settlement_after_supervision();
        let retained = host.retained.lock().unwrap().take().unwrap();
        retained.acknowledge_settlement(&owner.cancellation(), false).unwrap();
        assert!(owner.settlement_ack().await.is_ok());
    }

    #[tokio::test]
    async fn expired_unpolled_begin_reaches_terminal_registration_without_renewal() {
        let host = Arc::new(LostBegin { retained: Mutex::new(None), installed: Default::default() });
        let route = crate::host::test_prepared_route_with_allowance("expired-unpolled", std::time::Duration::from_millis(100));
        let deadline = route.allowance().deadline();
        let identity = route.owner().clone();
        let request = SubmitRequest { decision_id: identity.decision_id.clone(), epoch: identity.epoch,
            revision: Some(identity.revision), idempotency_key: "expired-original-key".into(),
            action: Value::Null, public_explanation: None };
        tokio::time::sleep_until(deadline).await;
        let mut owner = ForcedAnswerSupervisor::new(host.clone(), identity, request, route).unwrap();
        assert_eq!(owner.allowance.deadline(), deadline);
        owner.request_settlement_after_supervision();
        assert_eq!(owner.settlement_ack().await, Err(HostRefusal::Expired));
        assert!(host.retained.lock().unwrap().is_some(), "expired wait must drive terminal registration");
        assert!(owner.cancellation().acknowledged_settlement().is_none());
        assert!(owner.settled().is_none(), "timeout is not actor ACK");
        let retained = host.retained.lock().unwrap().take().unwrap();
        retained.acknowledge_settlement(&owner.cancellation(), false).unwrap();
        assert!(owner.settlement_ack().await.is_ok());
        assert_eq!(owner.allowance.deadline(), deadline, "terminal observation did not renew allowance");
    }

    #[tokio::test]
    async fn canceled_before_first_begin_poll_retains_registration_until_actual_ack() {
        let host = Arc::new(LostBegin { retained: Mutex::new(None), installed: Default::default() });
        let route = crate::host::test_prepared_route("never-polled-begin");
        let identity = route.owner().clone();
        let request = SubmitRequest { decision_id: identity.decision_id.clone(), epoch: identity.epoch,
            revision: Some(identity.revision), idempotency_key: "unpolled-original-key".into(),
            action: Value::Null, public_explanation: None };
        let mut owner = ForcedAnswerSupervisor::new(host.clone(), identity, request, route).unwrap();
        assert!(host.retained.lock().unwrap().is_none());
        owner.request_settlement_after_supervision();
        {
            let mut wait = Box::pin(owner.settlement_ack());
            std::future::poll_fn(|cx| {
                assert!(wait.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            }).await;
            // Cancel this borrowed wait; the foreground owner/actor association still exist.
        }
        assert!(host.retained.lock().unwrap().is_some());
        assert!(owner.settled().is_none());
        let retained = host.retained.lock().unwrap().take().unwrap();
        retained.acknowledge_settlement(&owner.cancellation(), false).unwrap();
        assert!(owner.settlement_ack().await.is_ok());
    }

    #[test]
    fn force_requires_explicit_pass_and_proven_empty_action_domain() {
        let empty_unit = ActionSchema::Unit { among: vec![] };
        let list = |item, min, max| ActionSchema::List {
            item: Box::new(item),
            min,
            max,
        };
        for schema in [
            ActionSchema::Choice { options: vec![] },
            empty_unit.clone(),
            ActionSchema::Hex {
                among: Some(vec![]),
            },
            list(ActionSchema::Bool, 0, 0),
            list(empty_unit.clone(), 0, 10),
            list(empty_unit.clone(), 1, 10),
            ActionSchema::Record {
                fields: vec![FieldSchema {
                    name: "unit".into(),
                    doc: "".into(),
                    schema: empty_unit.clone(),
                    optional: false,
                }],
            },
        ] {
            assert!(!forced_pass(&ActionSpace::new(schema.clone())));
            assert!(forced_pass(
                &ActionSpace::new(schema).with_pass("No actions.")
            ));
        }
        for schema in [
            ActionSchema::Bool,
            ActionSchema::Integer { min: 0, max: 0 },
            ActionSchema::Hex { among: None },
            ActionSchema::Path {
                from: "A1".into(),
                max_steps: 0,
            },
            list(ActionSchema::Bool, 0, 10),
            ActionSchema::Record {
                fields: vec![FieldSchema {
                    name: "optional".into(),
                    doc: "".into(),
                    schema: empty_unit,
                    optional: true,
                }],
            },
            ActionSchema::Record { fields: vec![] },
        ] {
            assert!(!forced_pass(
                &ActionSpace::new(schema).with_pass("Offered, not forced.")
            ));
        }
    }
    struct ForcedGame {
        request: Mutex<Option<DecisionRequest>>,
        submissions: Mutex<Vec<SubmitRequest>>,
        reject: bool,
    }
    impl ForcedGame {
        async fn new(reject: bool) -> Arc<Self> {
            let mut request = NumberDuel::game(11).pending(AXIS).await.remove(0);
            request.space = ActionSpace::new(ActionSchema::Choice { options: vec![] })
                .with_pass("Nothing to choose.");
            Arc::new(Self {
                request: Mutex::new(Some(request)),
                submissions: Mutex::new(vec![]),
                reject,
            })
        }
    }
    #[async_trait]
    impl GameBackend for ForcedGame {
        async fn seats(&self) -> Vec<SeatId> {
            vec![AXIS]
        }
        async fn game_seq(&self) -> u64 {
            0
        }
        async fn pending(&self, seat: SeatId) -> Vec<DecisionRequest> {
            if seat == AXIS {
                self.request.lock().unwrap().iter().cloned().collect()
            } else {
                vec![]
            }
        }
        async fn observe(&self, _: SeatId) -> Value {
            json!({})
        }
        async fn inspect(&self, _: SeatId, _: &str) -> Result<Value, ToolError> {
            panic!("no inspection is needed")
        }
        async fn describe_actions(&self, _: SeatId, _: &str) -> Result<Value, ToolError> {
            panic!("no model tools are needed")
        }
        async fn validate(&self, _: SeatId, _: &str, _: &Value) -> Result<Value, ToolError> {
            panic!("no model validation is needed")
        }
        async fn epoch(&self, _: SeatId) -> u64 {
            7
        }
        async fn outcome(&self) -> Option<Value> {
            self.request
                .lock()
                .unwrap()
                .is_none()
                .then(|| json!("finished"))
        }
        async fn submit(
            &self,
            seat: SeatId,
            request: SubmitRequest,
        ) -> Result<crate::game::SubmitReceipt, ToolError> {
            assert_eq!(seat, AXIS);
            self.submissions.lock().unwrap().push(request.clone());
            if request.epoch != 7 {
                return Err(ToolError::EpochMismatch);
            }
            let mut pending = self.request.lock().unwrap();
            let p = pending.as_ref().unwrap();
            assert_eq!(request.revision, Some(p.revision));
            assert_eq!(request.decision_id, p.id.as_str());
            if !forced_pass(&p.space) {
                assert_eq!(request.action, Value::Bool(false));
                let p = pending.as_mut().unwrap();
                p.revision += 1;
                p.space = ActionSpace::new(ActionSchema::Choice { options: vec![] })
                    .with_pass("No choices.");
                return Ok(crate::game::SubmitReceipt {
                    decision_id: request.decision_id,
                    duplicate: false,
                    summary: "model answer".into(),
                    result: json!({}),
                });
            }
            assert_eq!(request.action, Value::Null);
            if self.reject {
                return Err(ToolError::Illegal("inert rejection".into()));
            }
            *pending = None;
            Ok(crate::game::SubmitReceipt {
                decision_id: request.decision_id,
                duplicate: false,
                summary: "accepted".into(),
                result: json!({"accepted":true}),
            })
        }
    }
    struct NoCli;
    #[async_trait]
    impl SeatDriver for NoCli {
        fn kind(&self) -> CliKind {
            CliKind::Claude
        }
        async fn start(&mut self, _: Option<&str>) -> Result<SessionInfo, DriverError> {
            panic!("forced answer must not start a CLI")
        }
        async fn run_turn(
            &mut self,
            _: &str,
            _: std::time::Duration,
        ) -> Result<TurnOutcome, DriverError> {
            panic!("forced answer must not invoke a model")
        }
        fn session_id(&self) -> Option<String> {
            None
        }
        fn is_alive(&mut self) -> bool {
            false
        }
        async fn stop(&mut self) -> Result<(), crate::driver::DriverError> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn local_forced_answer_needs_no_cli_slot_or_model_call_budget() {
        let game = ForcedGame::new(false).await;
        let memory = Arc::new(InMemorySeatMemory::new(vec![AXIS]));
        let router = Arc::new(ToolRouter::new(game.clone(), memory.clone(), &[AXIS]));
        let store = LocalTranscript::detached();
        let sink = TranscriptSink::new(store.clone());
        let (state, _) = watch::channel(SeatState {
            status: SeatStatus::Idle,
            reason: None,
        });
        let (stop_sender, stop) = watch::channel(false);
        drop(stop_sender); // Closing a false stop channel is not a stop request.
        let mut runner = SeatRunner {
            seat: AXIS,
            controller_epoch: 7,
            driver: Box::new(NoCli),
            game: game.clone(),
            memory,
            router: router.clone(),
            sink,
            sessions: Arc::new(InMemorySessions::default()),
            prompts: Arc::new(DefaultPrompts {
                game_description: "test".into(),
            }),
            limits: RunLimits {
                max_tool_calls_per_seat: 0,
                ..RunLimits::default()
            },
            permits: Arc::new(Semaphore::new(0)),
            started: tokio::time::Instant::now(),
            state,
            stop,
        };
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(2), runner.run())
                .await
                .unwrap(),
            SeatEnd::Finished
        );
        assert_eq!(
            router
                .counters(AXIS)
                .unwrap()
                .calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(game.submissions.lock().unwrap().len(), 1);
        let rows = store.seat_log(AXIS);
        assert!(rows.iter().any(|r|matches!(&r.entry,TranscriptEntry::DecisionSubmitted {summary,..} if summary.contains("automatic forced answer"))));
    }
    #[tokio::test]
    async fn forced_answer_keeps_original_epoch_and_never_falls_back_on_rejection() {
        let game = ForcedGame::new(true).await;
        let sink = TranscriptSink::new(LocalTranscript::detached());
        let request = game.pending(AXIS).await.remove(0);
        assert_eq!(
            answer(game.as_ref(), AXIS, 6, &request, &sink)
                .await
                .unwrap_err(),
            ToolError::EpochMismatch
        );
        assert!(matches!(
            answer(game.as_ref(), AXIS, 7, &request, &sink).await,
            Err(ToolError::Illegal(_))
        ));
        assert_eq!(game.submissions.lock().unwrap().len(), 2);
        let mut stale = request.clone();
        stale.revision += 1;
        assert!(matches!(
            answer(game.as_ref(), AXIS, 7, &stale, &sink).await,
            Err(ToolError::Stale(_))
        ));
        assert_eq!(game.submissions.lock().unwrap().len(), 2);
    }
    #[tokio::test]
    async fn unconfirmed_intent_cannot_commit_an_automatic_order() {
        let game = ForcedGame::new(false).await;
        let sink = TranscriptSink::new(LocalTranscript::detached());
        sink.stop_delivery().await;
        let request = game.pending(AXIS).await.remove(0);
        assert!(matches!(
            answer(game.as_ref(), AXIS, 7, &request, &sink).await,
            Err(ToolError::Other(_))
        ));
        assert!(game.submissions.lock().unwrap().is_empty());
        assert_eq!(game.pending(AXIS).await.len(), 1);
        assert_eq!(sink.pending_count(), 1);
    }
    struct OneModel {
        game: Arc<ForcedGame>,
        turns: Arc<std::sync::atomic::AtomicU32>,
    }
    #[async_trait]
    impl SeatDriver for OneModel {
        fn kind(&self) -> CliKind {
            CliKind::Claude
        }
        async fn start(&mut self, _: Option<&str>) -> Result<SessionInfo, DriverError> {
            Ok(SessionInfo {
                session_id: "inert-one".into(),
                model: None,
                cli_version: None,
                resumed: false,
            })
        }
        async fn run_turn(
            &mut self,
            _: &str,
            _: std::time::Duration,
        ) -> Result<TurnOutcome, DriverError> {
            assert_eq!(
                self.turns.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                0,
                "forced successor must not receive a model nudge"
            );
            let p = self.game.pending(AXIS).await.remove(0);
            self.game
                .submit(
                    AXIS,
                    SubmitRequest {
                        decision_id: p.id.to_string(),
                        epoch: 7,
                        revision: Some(p.revision),
                        idempotency_key: "model-one".into(),
                        action: Value::Bool(false),
                        public_explanation: None,
                    },
                )
                .await
                .unwrap();
            Ok(TurnOutcome {
                ok: true,
                error: None,
                text: None,
                usage: Default::default(),
                quota: vec![],
            })
        }
        fn session_id(&self) -> Option<String> {
            Some("inert-one".into())
        }
        fn is_alive(&mut self) -> bool {
            true
        }
        async fn stop(&mut self) -> Result<(), crate::driver::DriverError> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn forced_successor_is_local_instead_of_a_model_nudge() {
        let game = ForcedGame::new(false).await;
        game.request.lock().unwrap().as_mut().unwrap().space = ActionSpace::new(ActionSchema::Bool);
        let memory = Arc::new(InMemorySeatMemory::new(vec![AXIS]));
        let router = Arc::new(ToolRouter::new(game.clone(), memory.clone(), &[AXIS]));
        let store = LocalTranscript::detached();
        let sink = TranscriptSink::new(store.clone());
        let (state, _) = watch::channel(SeatState {
            status: SeatStatus::Idle,
            reason: None,
        });
        let (_sender, stop) = watch::channel(false);
        let turns = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let mut runner = SeatRunner {
            seat: AXIS,
            controller_epoch: 7,
            driver: Box::new(OneModel {
                game: game.clone(),
                turns: turns.clone(),
            }),
            game: game.clone(),
            memory,
            router,
            sink,
            sessions: Arc::new(InMemorySessions::default()),
            prompts: Arc::new(DefaultPrompts {
                game_description: "inert".into(),
            }),
            limits: RunLimits {
                max_nudges: 0,
                ..RunLimits::default()
            },
            permits: Arc::new(Semaphore::new(1)),
            started: tokio::time::Instant::now(),
            state,
            stop,
        };
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(2), runner.run())
                .await
                .unwrap(),
            SeatEnd::Finished
        );
        assert_eq!(turns.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(game.submissions.lock().unwrap().len(), 2);
        assert!(store.seat_log(AXIS).iter().any(|r|matches!(&r.entry,TranscriptEntry::DecisionSubmitted{summary,..} if summary.contains("automatic forced"))));
    }
    #[tokio::test]
    async fn sibling_capture_after_marker_cannot_refuse_a_confirmed_intent() {
        struct SlowSibling {
            store: Arc<LocalTranscript>,
            intent_seen: tokio::sync::Notify,
            release_intent: tokio::sync::Notify,
            sibling_seen: tokio::sync::Notify,
            release_sibling: tokio::sync::Notify,
        }
        #[async_trait]
        impl crate::transcript::TranscriptStore for SlowSibling {
            async fn append(
                &self,
                seat: SeatId,
                at: String,
                entry: TranscriptEntry,
            ) -> Result<u64, String> {
                if let TranscriptEntry::System { text } = &entry {
                    if text.starts_with("Automatic forced answer attempt") {
                        self.intent_seen.notify_one();
                        self.release_intent.notified().await;
                    } else if text == "sibling" {
                        self.sibling_seen.notify_one();
                        self.release_sibling.notified().await;
                    }
                }
                crate::transcript::TranscriptStore::append(self.store.as_ref(), seat, at, entry)
                    .await
            }
        }
        let store = Arc::new(SlowSibling {
            store: LocalTranscript::detached(),
            intent_seen: Default::default(),
            release_intent: Default::default(),
            sibling_seen: Default::default(),
            release_sibling: Default::default(),
        });
        let sink = TranscriptSink::new(store.clone());
        let game = ForcedGame::new(false).await;
        let task_game = game.clone();
        let task_sink = sink.clone();
        let request = game.pending(AXIS).await.remove(0);
        let task =
            tokio::spawn(
                async move { answer(task_game.as_ref(), AXIS, 7, &request, &task_sink).await },
            );
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.intent_seen.notified(),
        )
        .await
        .unwrap();
        // The automatic task already enqueued its flush marker before yielding to
        // the writer. This sibling capture is deliberately after that marker.
        sink.system(crate::toy::COMMONWEALTH, "sibling");
        store.release_intent.notify_one();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.sibling_seen.notified(),
        )
        .await
        .unwrap();
        let committed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while game.submissions.lock().unwrap().is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await
        .is_ok();
        store.release_sibling.notify_one();
        let result = task.await.unwrap();
        sink.stop_delivery().await;
        assert!(
            committed,
            "a later sibling capture must not refuse the confirmed automatic intent"
        );
        assert!(result.unwrap());
    }
}
