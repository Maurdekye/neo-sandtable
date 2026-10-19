//! Single-writer forced intent/commit/delivery ownership.
//!
//! This is a trusted host seam, never GameBackend or a seat/public message.
//! Accepted commands survive expiry, overflow, lost replies and abort. No timer
//! here proves child/reader/SQLite settlement. The owner settles after supervision.
use std::{alloc::Layout, future::Future, pin::Pin, sync::Arc};

use cna_core::{
    decision::{DecisionRequest, DecisionResponse},
    engine::{Rejection, Ruleset},
};
use cna_seats::{
    game::{SubmitReceipt, SubmitRequest},
    host::{
        ConfirmedIntent, ConfirmedPostcommit, HostIdentity, HostRefusal, PreparedRoute,
        RouteCancellation, operation_refusal,
    },
    operation::{CommandCredit, Operation},
    transcript::UnconfirmedEntry,
};
use serde_json::json;

use crate::{
    Campaign, Error,
    host_credit::{
        BeginBody, CommandMetadata, DeliveryBody, FixedStorage, MarkerBody, StoredHostIdentity,
        admit_accepted_receipt, admit_receipt_children, charge_command, charge_delivery,
        copy_command_text, copy_delivery_row, copy_delivery_text, copy_pending,
        copy_transcript_entry, reserve_receipt_reply,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    AwaitingIntent,
    CommittedAwaitingDelivery,
    DeliveryConfirmed,
    Aborted,
}
/// The UUID formatting type supplies actual fixed token storage. Cleanup
/// copies this value without a String allocation or a new growth admission.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Token {
    uuid: uuid::Uuid,
    text: [u8; uuid::fmt::Hyphenated::LENGTH],
}
impl Token {
    fn from_uuid(uuid: uuid::Uuid) -> Self {
        let mut text = [0; uuid::fmt::Hyphenated::LENGTH];
        uuid.hyphenated().encode_lower(&mut text);
        Self { uuid, text }
    }
    fn parse(text: &str) -> Result<Self, HostRefusal> {
        let token =
            Self::from_uuid(uuid::Uuid::parse_str(text).map_err(|_| HostRefusal::IdentityChanged)?);
        if token.as_str() != text {
            return Err(HostRefusal::IdentityChanged);
        }
        Ok(token)
    }
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.text).expect("UUID ASCII token")
    }
}
fn copy_token(operation: &Operation, token: Token) -> Result<String, HostRefusal> {
    copy_command_text(operation, token.as_str())
}

/// Exactly the existing ordinary submit receipt shape, with bounded source
/// identity admitted before reservation. No new model-facing receipt fields.
pub(crate) fn accepted_receipt(decision_id: &str, duplicate: bool) -> SubmitReceipt {
    // Exact destination capacity, prepaid before production submit. Avoid
    // format!'s estimated/growing capacity while preserving the old bytes.
    let mut summary = String::with_capacity("accepted decision ".len() + decision_id.len());
    summary.push_str("accepted decision ");
    summary.push_str(decision_id);
    SubmitReceipt {
        decision_id: decision_id.to_owned(),
        duplicate,
        summary,
        result: json!({"accepted":true}),
    }
}

/// The SAME production metadata footprint used by byte-fit controls. This is
/// conservative encoded owner accounting, never an allocator/RSS assertion.
pub(crate) fn retained_command_storage_bytes() -> usize {
    std::mem::size_of::<ForcedState>()
        + std::mem::size_of::<Active>()
        + std::mem::size_of::<ForcedLease>()
        + std::mem::size_of::<CommandCredit<SubmitReceipt>>()
}

pub(crate) fn retained_reservation_storage_bytes() -> usize {
    std::mem::size_of::<Reserved>()
        + std::mem::size_of::<Reservation>()
        + std::mem::size_of::<Active>()
        + std::mem::size_of::<ForcedLease>()
}

/// Owned by the existing synchronous campaign writer. The route is retained here
/// through final settlement; dropping a caller's lease does not discard it.
pub(crate) struct ForcedState {
    identity: StoredHostIdentity,
    token: Token,
    receipt_identity: Option<String>,
    request: Option<SubmitRequest>,
    route: PreparedRoute,
    operation: Operation,
    credit: Option<CommandCredit<SubmitReceipt>>,
    phase: Phase,
    acknowledged_ordinal: u64,
    intent_ordinal: Option<u64>,
    postcommit_ordinal: Option<u64>,
    append_uncertain: bool,
    refusal: Option<HostRefusal>,
    fatal: Option<crate::writer_fatal::FatalClass>,
}

impl ForcedState {
    fn record_fatal(&mut self, error: Error, writer: &mut crate::writer_fatal::WriterFatals) {
        self.fatal
            .get_or_insert_with(|| crate::writer_fatal::FatalClass::of(&error));
        writer.record(error); // direct MOVE before publication; no Error child here
    }
    // Total ownership transfer comes BEFORE admission/encoding/receipt checks.
    // Even a refused BEGIN retains the SAME capability until actual supervision
    // settlement; no Result error can drop an already-prepared worker route.
    fn new(
        identity: StoredHostIdentity,
        request: SubmitRequest,
        route: PreparedRoute,
        token: Token,
        credit: CommandCredit<SubmitReceipt>,
    ) -> Self {
        let operation = route.operation();
        let acknowledged_ordinal = route.boundary();
        Self {
            identity,
            token,
            receipt_identity: None,
            request: Some(request),
            route,
            operation,
            credit: Some(credit),
            phase: Phase::AwaitingIntent,
            acknowledged_ordinal,
            intent_ordinal: None,
            postcommit_ordinal: None,
            append_uncertain: false,
            refusal: None,
            fatal: None,
        }
    }

    fn admit(&mut self) -> Result<(), HostRefusal> {
        self.operation.check().map_err(operation_refusal)?;
        let request = self.request.as_ref().ok_or(HostRefusal::IdentityChanged)?;
        if !self.identity.matches(self.route.owner(), &self.operation)
            || request.decision_id != self.identity.decision_id()
            || request.epoch != self.route.owner().epoch
            || request.revision != Some(self.identity.revision())
            || request.idempotency_key.is_empty()
            || self.route.allowance().deadline() != self.operation.allowance().deadline()
            || self.route.cancellation().requested()
        {
            return Err(HostRefusal::IdentityChanged);
        }
        let identity_body = self.identity.body(&self.operation);
        let body = BeginBody {
            identity: &identity_body,
            stream: self.route.stream(),
            boundary: self.route.boundary(),
            token: self.token.as_str(),
        };
        // SAME credit was claimed at campaign reserve, not reacquired after
        // worker prepare. Retained reservation metadata and this handoff are
        // accounted together before accepting the next envelope.
        self.credit
            .as_mut()
            .ok_or(HostRefusal::Unknown)?
            .admit_settlement(&body)
            .map_err(operation_refusal)?;
        // Owned metadata copies are conservatively charged BEFORE copying them.
        self.receipt_identity = Some(copy_command_text(
            &self.operation,
            &request.idempotency_key,
        )?);
        Ok(())
    }

    pub(crate) fn token(&self) -> &str {
        self.token.as_str()
    }
    pub(crate) fn cancellation(&self) -> Arc<RouteCancellation> {
        self.route.cancellation()
    }
    pub(crate) fn install(
        &self,
        delivery: Arc<dyn cna_seats::host::HostDeliveryRoute>,
    ) -> Result<(), HostRefusal> {
        self.route
            .install(copy_token(&self.operation, self.token)?, delivery)
    }
    pub(crate) fn operation(&self) -> &Operation {
        &self.operation
    }

    /// Called by the writer on notification or before any new effect. This
    /// closes the operation but does not release its route/receipt or controls.
    pub(crate) fn observe_abort(&mut self) {
        let failure = self
            .operation
            .check()
            .err()
            .map(operation_refusal)
            .or_else(|| {
                self.route
                    .cancellation()
                    .requested()
                    .then_some(HostRefusal::StoppedBeforeWork)
            });
        if let Some(failure) = failure {
            self.abort_with(failure);
        }
    }
    fn abort_with(&mut self, failure: HostRefusal) {
        if self.refusal.is_none() {
            self.refusal = Some(failure);
        }
        self.phase = Phase::Aborted;
        self.route.cancellation().request();
    }
    pub(crate) fn abort(&mut self) {
        self.abort_with(HostRefusal::StoppedBeforeWork);
    }

    fn marker_matches(
        &self,
        stream: &str,
        token: &str,
        owner: &HostIdentity,
        ordinal: u64,
    ) -> bool {
        stream == self.route.stream()
            && token == self.token.as_str()
            && self.identity.matches(owner, &self.operation)
            && ordinal == self.acknowledged_ordinal
    }

    /// Worker calls are FIFO in ONE shared stream, not ordered by per-seat tseq.
    /// Every original author remains original; the forced owner does not gain
    /// another seat's author authority. No unsuccessful/unknown append is retried.
    pub(crate) fn append<R: Ruleset>(
        &mut self,
        campaign: &mut Campaign<R>,
        writer: &mut crate::writer_fatal::WriterFatals,
        stream: &str,
        ordinal: u64,
        row: &UnconfirmedEntry,
    ) -> Result<u64, HostRefusal> {
        self.observe_abort();
        if !matches!(
            self.phase,
            Phase::AwaitingIntent | Phase::CommittedAwaitingDelivery
        ) || stream != self.route.stream()
            || self.acknowledged_ordinal.checked_add(1) != Some(ordinal)
        {
            return Err(self.refusal.clone().unwrap_or(HostRefusal::IdentityChanged));
        }
        let body = DeliveryBody {
            stream,
            token: self.token.as_str(),
            ordinal,
            row,
        };
        charge_delivery(&self.operation, &body)?;
        // Existing encoded guard stays additional. This distinct entry-only
        // copy prepays all resident children before cloning; timestamp/origin
        // belong only to the separately admitted queued row copy.
        charge_delivery(&self.operation, row)?;
        let entry = copy_transcript_entry(&self.operation, &row.entry)?;
        let result = campaign.transcript(row.seat, &row.at, entry);
        match result {
            Ok(tseq) => {
                self.acknowledged_ordinal = ordinal;
                Ok(tseq)
            }
            Err(error) => {
                self.append_uncertain = true;
                if matches!(
                    error,
                    Error::Storage(_) | Error::Json(_) | Error::Recovery(_)
                ) {
                    self.record_fatal(error, writer);
                }
                self.abort_with(HostRefusal::Unknown);
                Err(HostRefusal::Unknown)
            }
        }
    }

    /// Exactly the marker-confirmed original request is consumed ONCE. Later
    /// sibling capture/pending counts are irrelevant to this intent boundary.
    pub(crate) fn commit<R: Ruleset>(
        &mut self,
        campaign: &mut Campaign<R>,
        writer: &mut crate::writer_fatal::WriterFatals,
        intent: ConfirmedIntent,
    ) -> Result<SubmitReceipt, HostRefusal> {
        self.observe_abort();
        if self.phase != Phase::AwaitingIntent
            || !self.marker_matches(
                intent.stream(),
                intent.token(),
                intent.owner(),
                intent.ordinal(),
            )
        {
            return Err(self.refusal.clone().unwrap_or(HostRefusal::IdentityChanged));
        }
        let marker = MarkerBody {
            stream: intent.stream(),
            token: intent.token(),
            owner: intent.owner().into(),
            ordinal: intent.ordinal(),
            receipt_identity: None,
        };
        charge_command(&self.operation, &marker)?;
        admit_accepted_receipt(&self.operation, self.identity.decision_id())?;
        let request = self.request.take().ok_or(HostRefusal::IdentityChanged)?;
        self.intent_ordinal = Some(intent.ordinal());
        let result = campaign.submit(DecisionResponse {
            decision_id: cna_core::ids::DecisionId::new(request.decision_id),
            seat: self.identity.seat(),
            controller_epoch: request.epoch,
            decision_revision: self.identity.revision(),
            idempotency_key: request.idempotency_key,
            action: request.action,
            public_explanation: request.public_explanation,
        });
        match result {
            Ok(receipt) => {
                // Move accepted R into canonical ownership BEFORE checking its
                // retention charge. A failed cap cannot undo the SQLite commit.
                let receipt = accepted_receipt(self.identity.decision_id(), receipt.duplicate);
                self.phase = Phase::CommittedAwaitingDelivery;
                if let Err(error) = self
                    .credit
                    .as_mut()
                    .ok_or(HostRefusal::Unknown)?
                    .retain_receipt(receipt)
                {
                    let refusal = operation_refusal(error);
                    self.abort_with(refusal.clone());
                    return Err(refusal);
                }
                // Reserve and BEGIN charges remain in the SAME credit through
                // final settlement. Add these retained identity/token/route
                // bodies conservatively beside accepted R before any reply;
                // consuming the request never grants a reset or refund.
                // Refusal retains canonical accepted R and never undoes commit.
                let metadata = CommandMetadata {
                    identity: self.identity.body(&self.operation),
                    stream: self.route.stream(),
                    token: self.token.as_str(),
                    receipt_identity: self
                        .receipt_identity
                        .as_deref()
                        .ok_or(HostRefusal::Unknown)?,
                    caller_receipt_identity: self
                        .receipt_identity
                        .as_deref()
                        .ok_or(HostRefusal::Unknown)?,
                    fixed_storage: FixedStorage(retained_command_storage_bytes()),
                };
                if let Err(error) = self
                    .credit
                    .as_mut()
                    .ok_or(HostRefusal::Unknown)?
                    .admit_settlement(&metadata)
                {
                    let refusal = operation_refusal(error);
                    self.abort_with(refusal.clone());
                    return Err(refusal);
                }
                // Synchronous storage may have crossed the SAME absolute
                // allowance. Keep accepted R, return truthful timeout/overflow.
                self.observe_abort();
                if let Some(refusal) = &self.refusal {
                    return Err(refusal.clone());
                }
                // Canonical R and its actual reply copy can coexist. Reserve
                // that complete body under BOTH live COMMAND and cumulative
                // operation caps BEFORE cloning, with no ACK/dequeue refund.
                if let Err(error) =
                    reserve_receipt_reply(self.credit.as_mut().ok_or(HostRefusal::Unknown)?)
                {
                    self.abort_with(error.clone());
                    return Err(error);
                }
                let accepted = self
                    .credit
                    .as_ref()
                    .and_then(|credit| credit.receipt())
                    .ok_or(HostRefusal::Unknown)?;
                admit_receipt_children(&self.operation, accepted)?;
                Ok(accepted.clone())
            }
            Err(error) => {
                // Preserve ordinary submission validation, including existing
                // illegal-answer seat pause; no forced author/planner privilege.
                if matches!(error, Error::Rejected(Rejection::Illegal { .. })) {
                    if let Err(error) = campaign.pause_seat(self.identity.seat()) {
                        self.record_fatal(error, writer);
                        self.abort_with(HostRefusal::Unknown);
                        return Err(HostRefusal::Unknown);
                    }
                }
                if matches!(
                    error,
                    Error::Storage(_) | Error::Json(_) | Error::Recovery(_)
                ) {
                    self.record_fatal(error, writer);
                }
                self.abort_with(HostRefusal::StoppedBeforeWork);
                Err(HostRefusal::StoppedBeforeWork)
            }
        }
    }

    /// Postcommit ACK is meaningful even if its reply is lost. Markers preserve
    /// the full sibling prefix; no normal reroute or capture-after-marker drain.
    pub(crate) fn finish_delivery<R: Ruleset>(
        &mut self,
        campaign: &Campaign<R>,
        published: &crate::actor::Published,
        receipt_identity: &str,
        marker: ConfirmedPostcommit,
    ) -> Result<Vec<DecisionRequest>, HostRefusal> {
        // Do not turn expiry into a false "command never committed" outcome.
        if self
            .credit
            .as_ref()
            .and_then(|credit| credit.receipt())
            .is_none()
            || self.postcommit_ordinal.is_some()
            || Some(receipt_identity) != self.receipt_identity.as_deref()
            || self.append_uncertain
            || !self.marker_matches(
                marker.stream(),
                marker.token(),
                marker.owner(),
                marker.ordinal(),
            )
            || self
                .intent_ordinal
                .is_none_or(|first| marker.ordinal() < first)
        {
            return Err(HostRefusal::IdentityChanged);
        }
        self.postcommit_ordinal = Some(marker.ordinal());
        self.phase = Phase::DeliveryConfirmed;
        let envelope = MarkerBody {
            stream: marker.stream(),
            token: marker.token(),
            owner: marker.owner().into(),
            ordinal: marker.ordinal(),
            receipt_identity: Some(receipt_identity),
        };
        // R + second marker + returned pending + metadata stay inside the ONE
        // COMMAND operation credit. No ACK/dequeue refunds live/cumulative bytes.
        self.credit
            .as_mut()
            .ok_or(HostRefusal::Unknown)?
            .admit_settlement(&envelope)
            .map_err(operation_refusal)?;
        // Borrow the already published campaign-owned root only when it is
        // the current commit. Admit encoded and recursive resident copies BEFORE
        // cloning only this owner's requests. Ordinary Full publication stays.
        let pending = published.host_pending(campaign, self.identity.seat())?;
        self.credit
            .as_mut()
            .ok_or(HostRefusal::Unknown)?
            .admit_settlement(&pending)
            .map_err(operation_refusal)?;
        self.observe_abort();
        if let Some(refusal) = &self.refusal {
            return Err(refusal.clone());
        }
        copy_pending(&self.operation, pending)
    }

    /// Called ONLY after the caller's actual supervision disposition. This
    /// ordered actor ACK releases route/control ownership; it does not invent
    /// child/reader/native/storage completion or refund possible-start liability.
    fn settlement_uncertain(&self) -> bool {
        self.append_uncertain || self.postcommit_ordinal.is_none()
    }
    pub(crate) fn settle(
        &mut self,
        gate: &HostGate,
        owner: &ForcedReservationOwner,
    ) -> Result<(), HostRefusal> {
        if !matches!(self.phase, Phase::DeliveryConfirmed | Phase::Aborted) {
            // Refusal leaves this SAME route, receipt and operation owned.
            return Err(HostRefusal::IdentityChanged);
        }
        // A missing second marker is an unsettled delivery prefix, even if no
        // failed SQLite append was observed. Never resume it through normal
        // routing based on timeout/drop or a guessed ACK.
        let handle = self.route.cancellation();
        if !handle.settlement_requested() {
            return Err(HostRefusal::IdentityChanged);
        }
        // The private witness is bound to THIS retained capability and available
        // even when the original begin future/reply/token was lost. Expiry/drop
        // requests only abort and cannot reach this transition.
        if !Arc::ptr_eq(&handle, &owner.cancellation()) {
            return Err(HostRefusal::IdentityChanged);
        }
        gate.release_forced()?;
        self.operation.settle();
        if let Err(error) = owner.acknowledge_settlement(&handle, self.settlement_uncertain()) {
            // Retain the SAME canonical body and permit on an incomplete ACK.
            // Closed admission cannot accidentally revive an uncertain route.
            gate.close();
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn can_settle(&self) -> bool {
        matches!(self.phase, Phase::DeliveryConfirmed | Phase::Aborted)
    }
}

// Exactly one operation owns COMMAND from admission through final settlement.
// Requests serialize within that ownership, while DELIVERY has its own lane.
use crate::host_gate::{HostGate, WriterWake, wake};
use crate::host_reply::{FixedReply, ReplyOwner, ReplyReceiver, ReplySender};
use cna_seats::host::{
    ForcedReservationOwner, HostDeliveryRoute, HostForcedLease, HostForcedReservation,
    HostForcedSubmission, ReservedForcedRoute,
};
use std::sync::Mutex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

enum Command {
    Begin {
        cancel: Arc<RouteCancellation>,
        route: PreparedRoute,
        reply: ReplySender<Result<String, HostRefusal>>,
    },
    Commit {
        token: Token,
        intent: ConfirmedIntent,
        reply: ReplySender<Result<SubmitReceipt, HostRefusal>>,
    },
    Finish {
        token: Token,
        receipt_identity: String,
        marker: ConfirmedPostcommit,
        reply: ReplySender<Result<Vec<DecisionRequest>, HostRefusal>>,
    },
    Abort {
        token: Token,
        reply: ReplySender<Result<(), HostRefusal>>,
    },
    Settle {
        token: Token,
        reply: ReplySender<Result<(), HostRefusal>>,
    },
}
struct Delivery {
    token: Token,
    stream: String,
    ordinal: u64,
    row: UnconfirmedEntry,
    reply: ReplySender<Result<u64, HostRefusal>>,
}
struct Reserved {
    identity: StoredHostIdentity,
    reply_owner: ReplyOwner,
    owner: ForcedReservationOwner,
    request: SubmitRequest,
    credit: CommandCredit<SubmitReceipt>,
    permit: OwnedSemaphorePermit,
}
struct Active {
    state: ForcedState,
    owner: ForcedReservationOwner,
    _permit: OwnedSemaphorePermit,
    reply_owner: ReplyOwner,
}

struct ForcedReplies {
    begin: Arc<FixedReply<Result<String, HostRefusal>>>,
    commit: Arc<FixedReply<Result<SubmitReceipt, HostRefusal>>>,
    finish: Arc<FixedReply<Result<Vec<DecisionRequest>, HostRefusal>>>,
    abort: Arc<FixedReply<Result<(), HostRefusal>>>,
    settle: Arc<FixedReply<Result<(), HostRefusal>>>,
    delivery: Arc<FixedReply<Result<u64, HostRefusal>>>,
}
impl ForcedReplies {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            begin: FixedReply::new(),
            commit: FixedReply::new(),
            finish: FixedReply::new(),
            abort: FixedReply::new(),
            settle: FixedReply::new(),
            delivery: FixedReply::new(),
        })
    }
    fn available(&self) -> Result<bool, HostRefusal> {
        Ok(self.begin.available().map_err(crate::host_reply::refusal)?
            && self
                .commit
                .available()
                .map_err(crate::host_reply::refusal)?
            && self
                .finish
                .available()
                .map_err(crate::host_reply::refusal)?
            && self.abort.available().map_err(crate::host_reply::refusal)?
            && self
                .settle
                .available()
                .map_err(crate::host_reply::refusal)?
            && self
                .delivery
                .available()
                .map_err(crate::host_reply::refusal)?)
    }
    fn settle(&self, owner: ReplyOwner) -> Result<(), HostRefusal> {
        self.begin
            .settle(owner)
            .map_err(crate::host_reply::refusal)?;
        self.commit
            .settle(owner)
            .map_err(crate::host_reply::refusal)?;
        self.finish
            .settle(owner)
            .map_err(crate::host_reply::refusal)?;
        self.abort
            .settle(owner)
            .map_err(crate::host_reply::refusal)?;
        self.settle
            .settle(owner)
            .map_err(crate::host_reply::refusal)?;
        self.delivery
            .settle(owner)
            .map_err(crate::host_reply::refusal)?;
        Ok(())
    }
    fn accepts(&self, owner: ReplyOwner) -> Result<(), HostRefusal> {
        self.begin
            .accepts(owner)
            .map_err(crate::host_reply::refusal)?;
        self.commit
            .accepts(owner)
            .map_err(crate::host_reply::refusal)?;
        self.finish
            .accepts(owner)
            .map_err(crate::host_reply::refusal)?;
        self.abort
            .accepts(owner)
            .map_err(crate::host_reply::refusal)?;
        self.settle
            .accepts(owner)
            .map_err(crate::host_reply::refusal)?;
        self.delivery
            .accepts(owner)
            .map_err(crate::host_reply::refusal)?;
        Ok(())
    }
}
/// Full precreated reply child bodies, not only their Arc pointers. Payload
/// growth still needs the SAME original G before constructing any copy.
struct ForcedReplyStorage {
    _handles: ForcedReplies,
    _begin: FixedReply<Result<String, HostRefusal>>,
    _commit: FixedReply<Result<SubmitReceipt, HostRefusal>>,
    _finish: FixedReply<Result<Vec<DecisionRequest>, HostRefusal>>,
    _abort: FixedReply<Result<(), HostRefusal>>,
    _settle: FixedReply<Result<(), HostRefusal>>,
    _delivery: FixedReply<Result<u64, HostRefusal>>,
}

/// The existing campaign operation has one canonical owner even before BEGIN
/// reaches the actor. Reserved bodies stay here; Active bodies move to the
/// writer, leaving the SAME association occupied. No pool or second credit.
enum Slot {
    Vacant,
    Reserved(Box<Reserved>),
    Active(Arc<RouteCancellation>),
}
type SharedSlot = Arc<Mutex<Slot>>;

pub(crate) struct ForcedBridge {
    commands: mpsc::Sender<Command>,
    delivery: mpsc::Sender<Delivery>,
    admission: Arc<Semaphore>,
    slot: SharedSlot,
    gate: Arc<HostGate>,
    wake: WriterWake,
    replies: Arc<ForcedReplies>,
}
pub(crate) struct ForcedInbox {
    commands: mpsc::Receiver<Command>,
    delivery: mpsc::Receiver<Delivery>,
    bridge: Arc<ForcedBridge>,
    active: Option<Active>,
}

pub(crate) fn channel(gate: Arc<HostGate>, wake: WriterWake) -> (Arc<ForcedBridge>, ForcedInbox) {
    let (commands, command_receiver) = mpsc::channel(1);
    let (delivery, delivery_receiver) = mpsc::channel(1);
    let bridge = Arc::new(ForcedBridge {
        commands,
        delivery,
        admission: Arc::new(Semaphore::new(1)),
        slot: Arc::new(Mutex::new(Slot::Vacant)),
        gate,
        wake,
        replies: ForcedReplies::new(),
    });
    (
        bridge.clone(),
        ForcedInbox {
            commands: command_receiver,
            delivery: delivery_receiver,
            bridge,
            active: None,
        },
    )
}

#[cfg(test)]
impl ForcedBridge {
    pub(crate) fn command_pending(&self) -> bool {
        self.commands.capacity() == 0
    }
    pub(crate) fn reserved(&self) -> bool {
        matches!(*self.slot.lock().unwrap(), Slot::Reserved(_))
    }
}

impl ForcedInbox {
    pub(crate) fn observe_abort(&mut self) {
        if let Some(active) = &mut self.active {
            active.state.observe_abort();
        }
    }
    /// Writer teardown closes new admission and asks abort without claiming
    /// actual caller supervision. It retains reserved/prepared/pending/active
    /// capabilities until their explicit original settlement request arrives.
    pub(crate) fn close(&mut self) {
        self.bridge.gate.close();
        if let Some(active) = &mut self.active {
            active.state.abort();
        }
        if let Ok(slot) = self.bridge.slot.lock() {
            match &*slot {
                Slot::Reserved(reserved) => reserved.owner.cancellation().request(),
                Slot::Active(cancel) => cancel.request(),
                Slot::Vacant => {}
            }
        }
    }
    /// Reserved tokenless settlement wake, checked before either request lane.
    /// The caller latches it ONLY after original supervision. A missing reply
    /// cannot strand the associated route, canonical R or control deferral.
    pub(crate) fn settlement(&mut self) -> bool {
        if let Some(active) = &mut self.active {
            if !active.state.cancellation().settlement_requested() {
                return false;
            }
            if !active.state.can_settle() {
                active.state.abort();
            }
            if self.bridge.replies.accepts(active.reply_owner).is_err() {
                self.bridge.gate.close();
                return false;
            }
            let Ok(mut slot) = self.bridge.slot.lock() else {
                self.bridge.gate.close();
                return false;
            };
            if !matches!(&*slot,Slot::Active(cancel) if Arc::ptr_eq(cancel,&active.owner.cancellation()))
            {
                self.bridge.gate.close();
                return false;
            }
            if active
                .state
                .settle(&self.bridge.gate, &active.owner)
                .is_ok()
            {
                if self.bridge.replies.settle(active.reply_owner).is_err() {
                    self.bridge.gate.close();
                    return false;
                }
                // Final writer transition precedes visible ACK. The original
                // credit/body/permit is forgotten only after that SAME ACK.
                self.active.take();
                *slot = Slot::Vacant;
                return true;
            }
            return false;
        }
        let Ok(mut slot) = self.bridge.slot.lock() else {
            self.bridge.gate.close();
            return false;
        };
        let Slot::Reserved(reserved) = &mut *slot else {
            return false;
        };
        let handle = reserved.owner.cancellation();
        if !handle.settlement_requested() {
            return false;
        }
        handle.request();
        if self.bridge.replies.accepts(reserved.reply_owner).is_err() {
            self.bridge.gate.close();
            return false;
        }
        if self.bridge.gate.release_forced().is_err() {
            return false;
        }
        reserved.owner.operation().settle();
        // No route proves only that no routed prefix exists; an already claimed
        // worker stream may have an incomplete prefix. Conservatively uncertain.
        if reserved
            .owner
            .acknowledge_settlement(&handle, true)
            .is_err()
        {
            self.bridge.gate.close();
            return false;
        }
        if self.bridge.replies.settle(reserved.reply_owner).is_err() {
            self.bridge.gate.close();
            return false;
        }
        *slot = Slot::Vacant;
        true
    }
    fn state(&mut self, token: &Token) -> Result<&mut ForcedState, HostRefusal> {
        self.active
            .as_mut()
            .filter(|active| active.state.token == *token)
            .map(|active| &mut active.state)
            .ok_or(HostRefusal::IdentityChanged)
    }
    /// One COMMAND then one DELIVERY per writer iteration. No storage await or
    /// same-actor synchronous lock is held by the caller of these handlers.
    pub(crate) fn command<R: Ruleset>(
        &mut self,
        campaign: &mut Campaign<R>,
        published: &crate::actor::Published,
        writer: &mut crate::writer_fatal::WriterFatals,
    ) -> bool {
        let Ok(command) = self.commands.try_recv() else {
            return false;
        };
        match command {
            Command::Begin {
                cancel,
                route,
                reply,
            } => {
                let result = (|| {
                    if self.active.is_some() {
                        return Err(HostRefusal::StoppedBeforeWork);
                    }
                    let mut slot = self.bridge.slot.lock().map_err(|_| HostRefusal::Unknown)?;
                    let Slot::Reserved(reserved) = &*slot else {
                        return Err(HostRefusal::IdentityChanged);
                    };
                    if !Arc::ptr_eq(&reserved.owner.cancellation(), &cancel)
                        || !Arc::ptr_eq(&route.cancellation(), &cancel)
                    {
                        // A foreign prepared route cannot replace the canonical
                        // owner or turn its supervision into another operation.
                        return Err(HostRefusal::IdentityChanged);
                    }
                    let Slot::Reserved(reserved) =
                        std::mem::replace(&mut *slot, Slot::Active(cancel.clone()))
                    else {
                        return Err(HostRefusal::Unknown);
                    };
                    drop(slot); // NEVER hold the shared admission lock over SQL.
                    let Reserved {
                        identity,
                        reply_owner,
                        owner,
                        request,
                        credit,
                        permit,
                    } = *reserved;
                    // Move the SAME fixed UUID/key storage, original request
                    // and credit once. No identity String clone or replacement
                    // owner is created by the Reserved-to-Active handoff.
                    let token = Token::from_uuid(uuid::Uuid::new_v4());
                    let state = ForcedState::new(identity, request, route, token, credit);
                    self.active = Some(Active {
                        state,
                        owner,
                        _permit: permit,
                        reply_owner,
                    });
                    let active = self.active.as_mut().ok_or(HostRefusal::Unknown)?;
                    // Campaign ownership was claimed before worker prepare.
                    // No second claim or allowance is granted by dequeueing.
                    let state = &mut active.state;
                    if let Err(error) = state.admit() {
                        state.abort_with(error.clone());
                        return Err(error);
                    }
                    let operation = state.operation().clone();
                    charge_command(&operation, &state.token())?;
                    operation
                        .charge(std::mem::size_of::<DeliveryAdapter>())
                        .map_err(operation_refusal)?;
                    let delivery = Arc::new(DeliveryAdapter {
                        bridge: self.bridge.clone(),
                        token: state.token,
                        operation,
                        replies: Arc::clone(&self.bridge.replies),
                        reply_owner,
                    });
                    if let Err(error) = state.install(delivery) {
                        // Keep the SAME route and credit through ordered abort/
                        // supervision settlement even on a failed begin reply.
                        state.abort_with(error.clone());
                        return Err(error);
                    }
                    let token = copy_token(state.operation(), state.token)?;
                    Ok(token)
                })();
                let lost = reply.send(result).is_err();
                if lost {
                    if let Some(active) = &mut self.active {
                        active.state.abort();
                    }
                }
            }
            Command::Commit {
                token,
                intent,
                reply,
            } => {
                let result = self
                    .state(&token)
                    .and_then(|state| state.commit(campaign, writer, intent));
                // Sending may fail; canonical R remains actor-owned regardless.
                let _ = reply.send(result);
            }
            Command::Finish {
                token,
                receipt_identity,
                marker,
                reply,
            } => {
                let result = self.state(&token).and_then(|state| {
                    state.finish_delivery(campaign, published, &receipt_identity, marker)
                });
                let _ = reply.send(result);
            }
            Command::Abort { token, reply } => {
                let result = self.state(&token).map(|state| state.abort());
                let _ = reply.send(result);
            }
            Command::Settle { token, reply } => {
                // Trusted leased path uses the SAME sticky post-supervision
                // request; no independent timeout/token interpretation.
                if let Ok(state) = self.state(&token) {
                    state.cancellation().request_settlement_after_supervision();
                }
                let result = if self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.state.token == token)
                    && self.settlement()
                {
                    Ok(())
                } else {
                    Err(HostRefusal::IdentityChanged)
                };
                let _ = reply.send(result);
            }
        }
        true
    }
    pub(crate) fn delivery<R: Ruleset>(
        &mut self,
        campaign: &mut Campaign<R>,
        writer: &mut crate::writer_fatal::WriterFatals,
    ) -> bool {
        let Ok(delivery) = self.delivery.try_recv() else {
            return false;
        };
        let result = self.state(&delivery.token).and_then(|state| {
            state.append(
                campaign,
                writer,
                &delivery.stream,
                delivery.ordinal,
                &delivery.row,
            )
        });
        if delivery.reply.send(result).is_err() {
            if let Some(active) = &mut self.active {
                active.state.append_uncertain = true;
                active.state.abort_with(HostRefusal::Unknown);
            }
        }
        true
    }
}

impl HostForcedSubmission for ForcedBridge {
    fn reserve(
        &self,
        identity: HostIdentity,
        request: SubmitRequest,
        operation: Operation,
    ) -> Result<Box<dyn HostForcedReservation>, HostRefusal> {
        operation.check().map_err(operation_refusal)?;
        if identity.operation_id != operation.id()
            || identity.decision_id != request.decision_id
            || identity.epoch != request.epoch
            || request.revision != Some(identity.revision)
            || request.idempotency_key.is_empty()
        {
            return Err(HostRefusal::IdentityChanged);
        }
        let permit = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| HostRefusal::StoppedBeforeWork)?;
        let mut slot = self.slot.lock().map_err(|_| HostRefusal::Unknown)?;
        if !matches!(*slot, Slot::Vacant) {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        if !self.replies.available()? {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        let writer = self.wake.get().ok_or(HostRefusal::Unknown)?.clone();
        let mut credit = CommandCredit::new(operation.clone()).map_err(operation_refusal)?;
        let body = crate::host_credit::ReservationBody {
            identity: (&identity).into(),
            request: (&request).into(),
            caller_receipt_identity: &request.idempotency_key,
            fixed_storage: FixedStorage(retained_reservation_storage_bytes()),
        };
        credit.admit_settlement(&body).map_err(operation_refusal)?;
        // This actual fixed representation and its WHOLE boxed child belong in
        // the pending original-creation descriptor; later encoded credit is
        // not its resident-storage substitute. Positive creation stays closed
        // until the complete owner F/G composition is assembled and validated.
        let stored_identity = StoredHostIdentity::capture(&identity, &operation)?;
        // The identity copies at prepared/active/marker/lease handoff are
        // precharged conservatively by the neutral owner before any acceptance.
        let owner = ForcedReservationOwner::new(identity, operation.clone())?;
        let view = owner.route();
        let generation = self.gate.claim_forced(owner.identity())?;
        let reply_owner = stored_identity.reply_owner(generation);
        *slot = Slot::Reserved(Box::new(Reserved {
            identity: stored_identity,
            reply_owner,
            owner,
            request,
            credit,
            permit,
        }));
        let Slot::Reserved(reserved) = &*slot else {
            return Err(HostRefusal::Unknown);
        };
        // Canonical association is present BEFORE registration outcome or the
        // caller's first await. Registration retains its fixed cleanup Thread
        // even on an asynchronously raced cap/deadline refusal. In that case
        // return only the aborting retained reservation, never admission to
        // prepare/install/effects and never an orphan owner without a handle.
        if reserved
            .owner
            .register_writer_wake(writer, std::mem::size_of::<Reserved>())
            .is_err()
        {
            reserved.owner.cancellation().request();
        }
        drop(slot);
        wake(&self.wake);
        Ok(Box::new(Reservation {
            view,
            commands: self.commands.clone(),
            wake: self.wake.clone(),
            begin_order: tokio::sync::Mutex::new(false),
            replies: Arc::clone(&self.replies),
            reply_owner,
        }))
    }
}

struct Reservation {
    view: ReservedForcedRoute,
    commands: mpsc::Sender<Command>,
    wake: WriterWake,
    begin_order: tokio::sync::Mutex<bool>,
    replies: Arc<ForcedReplies>,
    reply_owner: ReplyOwner,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.view.cancellation().request();
        wake(&self.wake);
    }
}
impl HostForcedReservation for Reservation {
    fn identity(&self) -> &HostIdentity {
        self.view.identity()
    }
    fn operation(&self) -> Operation {
        self.view.operation()
    }
    fn cancellation(&self) -> Arc<RouteCancellation> {
        self.view.cancellation()
    }
    fn route(&self) -> ReservedForcedRoute {
        self.view.clone()
    }
    fn begin<'life0, 'async_trait>(
        &'life0 self,
        route: PreparedRoute,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Box<dyn HostForcedLease>, HostRefusal>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(self.begin_future(route))
    }
    fn abort<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), HostRefusal>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(self.abort_future())
    }
    fn settle<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), HostRefusal>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(self.settle_future())
    }
}
impl Reservation {
    async fn begin_future(
        &self,
        route: PreparedRoute,
    ) -> Result<Box<dyn HostForcedLease>, HostRefusal> {
        let mut begun = self.begin_order.lock().await;
        if *begun {
            return Err(HostRefusal::IdentityChanged);
        }
        *begun = true; // ONE attempt, including refusal/canceled enqueue.
        let operation = self.operation();
        let cancel = self.cancellation();
        if !Arc::ptr_eq(&cancel, &route.cancellation()) || self.identity() != route.owner() {
            cancel.request();
            return Err(HostRefusal::IdentityChanged);
        }
        operation.check().map_err(operation_refusal)?;
        if cancel.requested() || cancel.settlement_requested() {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        let (reply, mut wait) = self
            .replies
            .begin
            .claim(self.reply_owner)
            .map_err(crate::host_reply::refusal)?;
        let allowance = operation.allowance();
        match tokio::time::timeout_at(
            allowance.deadline(),
            self.commands.send(Command::Begin {
                cancel: cancel.clone(),
                route,
                reply,
            }),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                cancel.request();
                return Err(HostRefusal::Unknown);
            }
            Err(_) => {
                cancel.request();
                return Err(HostRefusal::Expired);
            }
        }
        wake(&self.wake);
        let token = match tokio::time::timeout_at(allowance.deadline(), wait.receive()).await {
            Ok(result) => result.map_err(|_| HostRefusal::Unknown)??,
            Err(_) => {
                cancel.request();
                wake(&self.wake);
                return Err(HostRefusal::Expired);
            }
        };
        // The lease refers to the SAME channels, meter and preallocated cancel.
        Ok(Box::new(ForcedLease {
            token: Token::parse(&token)?,
            commands: self.commands.clone(),
            wake: self.wake.clone(),
            operation,
            cancel,
            command_order: tokio::sync::Mutex::new(()),
            replies: Arc::clone(&self.replies),
            reply_owner: self.reply_owner,
        }))
    }
    async fn abort_future(&self) -> Result<(), HostRefusal> {
        self.cancellation().request();
        wake(&self.wake);
        Ok(()) // abort request only; never represented as final settlement ACK.
    }
    async fn settle_future(&self) -> Result<(), HostRefusal> {
        let cancel = self.cancellation();
        cancel.request();
        cancel.request_settlement_after_supervision();
        wake(&self.wake);
        let _ = cancel.settlement_ack().await;
        Ok(())
    }
}

struct ForcedLease {
    token: Token,
    commands: mpsc::Sender<Command>,
    wake: WriterWake,
    operation: Operation,
    cancel: Arc<RouteCancellation>,
    command_order: tokio::sync::Mutex<()>,
    replies: Arc<ForcedReplies>,
    reply_owner: ReplyOwner,
}
impl Drop for ForcedLease {
    fn drop(&mut self) {
        self.cancel.request();
        wake(&self.wake);
    }
}
impl ForcedLease {
    async fn send(&self, command: Command) -> Result<(), HostRefusal> {
        match tokio::time::timeout_at(
            self.operation.allowance().deadline(),
            self.commands.send(command),
        )
        .await
        {
            Ok(Ok(())) => {
                wake(&self.wake);
                Ok(())
            }
            Ok(Err(_)) => {
                self.cancel.request();
                Err(HostRefusal::Unknown)
            }
            Err(_) => {
                self.cancel.request();
                Err(HostRefusal::Expired)
            }
        }
    }
    async fn result<T>(
        &self,
        mut wait: ReplyReceiver<Result<T, HostRefusal>>,
    ) -> Result<T, HostRefusal> {
        match tokio::time::timeout_at(self.operation.allowance().deadline(), wait.receive()).await {
            Ok(result) => result.map_err(|_| HostRefusal::Unknown)?,
            Err(_) => {
                self.cancel.request();
                wake(&self.wake);
                Err(HostRefusal::Expired)
            }
        }
    }
}
impl HostForcedLease for ForcedLease {
    fn token(&self) -> &str {
        self.token.as_str()
    }
    fn cancellation(&self) -> Arc<RouteCancellation> {
        self.cancel.clone()
    }
    fn commit<'life0, 'async_trait>(
        &'life0 self,
        intent: ConfirmedIntent,
    ) -> Pin<Box<dyn Future<Output = Result<SubmitReceipt, HostRefusal>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(self.commit_future(intent))
    }
    fn finish_delivery<'life0, 'life1, 'async_trait>(
        &'life0 self,
        receipt_identity: &'life1 str,
        marker: ConfirmedPostcommit,
    ) -> Pin<
        Box<dyn Future<Output = Result<Vec<DecisionRequest>, HostRefusal>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(self.finish_future(receipt_identity, marker))
    }
    fn abort<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), HostRefusal>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(self.abort_future())
    }
    fn settle<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), HostRefusal>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(self.settle_future())
    }
}
impl ForcedLease {
    async fn commit_future(&self, intent: ConfirmedIntent) -> Result<SubmitReceipt, HostRefusal> {
        let _ordered = self.command_order.lock().await;
        self.operation.check().map_err(operation_refusal)?;
        charge_command(&self.operation, &self.token.as_str())?;
        let (reply, wait) = self
            .replies
            .commit
            .claim(self.reply_owner)
            .map_err(crate::host_reply::refusal)?;
        self.send(Command::Commit {
            token: self.token,
            intent,
            reply,
        })
        .await?;
        self.result(wait).await
    }
    async fn finish_future(
        &self,
        receipt_identity: &str,
        marker: ConfirmedPostcommit,
    ) -> Result<Vec<DecisionRequest>, HostRefusal> {
        let _ordered = self.command_order.lock().await;
        charge_command(&self.operation, &(self.token.as_str(), receipt_identity))?;
        let (reply, wait) = self
            .replies
            .finish
            .claim(self.reply_owner)
            .map_err(crate::host_reply::refusal)?;
        self.send(Command::Finish {
            token: self.token,
            receipt_identity: copy_command_text(&self.operation, receipt_identity)?,
            marker,
            reply,
        })
        .await?;
        self.result(wait).await
    }
    async fn abort_future(&self) -> Result<(), HostRefusal> {
        // Preallocated cancellation is independent of a busy command lane.
        self.cancel.request();
        wake(&self.wake);
        let _ordered = self.command_order.lock().await;
        let (reply, mut wait) = self
            .replies
            .abort
            .claim(self.reply_owner)
            .map_err(crate::host_reply::refusal)?;
        self.commands
            .send(Command::Abort {
                token: self.token,
                reply,
            })
            .await
            .map_err(|_| HostRefusal::Unknown)?;
        wake(&self.wake);
        wait.receive().await.map_err(crate::host_reply::refusal)?
    }
    async fn settle_future(&self) -> Result<(), HostRefusal> {
        // Trusted caller invokes this AFTER actual child/reader disposition.
        // Expiry is not a reason to skip the original actor settlement ACK.
        let _ordered = self.command_order.lock().await;
        let (reply, mut wait) = self
            .replies
            .settle
            .claim(self.reply_owner)
            .map_err(crate::host_reply::refusal)?;
        self.commands
            .send(Command::Settle {
                token: self.token,
                reply,
            })
            .await
            .map_err(|_| HostRefusal::Unknown)?;
        wake(&self.wake);
        wait.receive().await.map_err(crate::host_reply::refusal)?
    }
}
struct DeliveryAdapter {
    bridge: Arc<ForcedBridge>,
    token: Token,
    operation: Operation,
    replies: Arc<ForcedReplies>,
    reply_owner: ReplyOwner,
}
const DELIVERY_REFUSED_BODY: &str = "host delivery refused";
const DELIVERY_UNAVAILABLE_BODY: &str = "host delivery unavailable";
const DELIVERY_ACK_UNAVAILABLE_BODY: &str = "host delivery acknowledgement unavailable";
const DELIVERY_REFUSED_BODY_CAPACITY: usize = DELIVERY_REFUSED_BODY.len();
const DELIVERY_UNAVAILABLE_BODY_CAPACITY: usize = DELIVERY_UNAVAILABLE_BODY.len();
const DELIVERY_ACK_UNAVAILABLE_BODY_CAPACITY: usize = DELIVERY_ACK_UNAVAILABLE_BODY.len();
// charge_delivery only produces Operation::check/charge/overflow refusals.
// Include every current OperationError rendering; no identity/number is printed.
// The neutral producer's own String remains a separate owner's F obligation.
const DELIVERY_ADMISSION_BODY_CAPACITY: usize =
    "host ownership error: ".len() + "operation credit still owned".len();

fn delivery_refusal_body(text: &'static str, capacity: usize) -> String {
    let mut body = String::with_capacity(capacity);
    body.push_str(text);
    body
}
fn delivery_admission_body(error: HostRefusal) -> String {
    use std::fmt::Write;
    // Preserve the previous Display bytes without formatter growth estimates.
    // Original F/live-count assembly is still required; there is no meter here.
    // m734060959054: choose counting. Ownership's inner body and this outer
    // String coexist through write!, so this conversion's N_site is TWO.
    // Include String-returning refusals; this is not whole strict-call-site N.
    let mut body = String::with_capacity(DELIVERY_ADMISSION_BODY_CAPACITY);
    write!(&mut body, "{error}").expect("writing to String cannot fail");
    body
}
impl HostDeliveryRoute for DeliveryAdapter {
    fn append<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        stream: &'life1 str,
        ordinal: u64,
        row: &'life2 UnconfirmedEntry,
    ) -> Pin<Box<dyn Future<Output = Result<u64, String>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(self.append_future(stream, ordinal, row))
    }
}
impl DeliveryAdapter {
    async fn append_future(
        &self,
        stream: &str,
        ordinal: u64,
        row: &UnconfirmedEntry,
    ) -> Result<u64, String> {
        let body = DeliveryBody {
            stream,
            token: self.token.as_str(),
            ordinal,
            row,
        };
        charge_delivery(&self.operation, &body).map_err(delivery_admission_body)?;
        let row = copy_delivery_row(&self.operation, row).map_err(|_| {
            delivery_refusal_body(DELIVERY_REFUSED_BODY, DELIVERY_REFUSED_BODY_CAPACITY)
        })?;
        let (reply, mut wait) = self.replies.delivery.claim(self.reply_owner).map_err(|_| {
            delivery_refusal_body(
                DELIVERY_UNAVAILABLE_BODY,
                DELIVERY_UNAVAILABLE_BODY_CAPACITY,
            )
        })?;
        self.bridge
            .delivery
            .send(Delivery {
                token: self.token,
                stream: copy_delivery_text(&self.operation, stream).map_err(|_| {
                    delivery_refusal_body(DELIVERY_REFUSED_BODY, DELIVERY_REFUSED_BODY_CAPACITY)
                })?,
                ordinal,
                row,
                reply,
            })
            .await
            .map_err(|_| {
                delivery_refusal_body(
                    DELIVERY_UNAVAILABLE_BODY,
                    DELIVERY_UNAVAILABLE_BODY_CAPACITY,
                )
            })?;
        wake(&self.bridge.wake);
        wait.receive()
            .await
            .map_err(|_| {
                delivery_refusal_body(
                    DELIVERY_ACK_UNAVAILABLE_BODY,
                    DELIVERY_ACK_UNAVAILABLE_BODY_CAPACITY,
                )
            })?
            .map_err(|_| {
                delivery_refusal_body(DELIVERY_REFUSED_BODY, DELIVERY_REFUSED_BODY_CAPACITY)
            })
    }
}

#[cfg(test)]
mod refusal_body_tests {
    use super::*;

    #[test]
    fn every_delivery_literal_preserves_text_and_bounds_actual_capacity() {
        for (text, bound) in [
            (DELIVERY_REFUSED_BODY, DELIVERY_REFUSED_BODY_CAPACITY),
            (
                DELIVERY_UNAVAILABLE_BODY,
                DELIVERY_UNAVAILABLE_BODY_CAPACITY,
            ),
            (
                DELIVERY_ACK_UNAVAILABLE_BODY,
                DELIVERY_ACK_UNAVAILABLE_BODY_CAPACITY,
            ),
        ] {
            let body = delivery_refusal_body(text, bound);
            assert_eq!(body, text);
            assert!(body.capacity() <= bound);
        }
    }

    #[test]
    fn every_current_operation_refusal_preserves_display_and_bounds_actual_capacity() {
        use cna_seats::operation::OperationError;
        for error in [
            OperationError::NoAllowance,
            OperationError::Expired,
            OperationError::CaptureOverflow,
            OperationError::Settled,
            OperationError::CreditOwned,
        ] {
            let refusal = operation_refusal(error);
            // Inspect the ACTUAL neutral producer's child, including spare
            // capacity, before moving it into the production formatter.
            let input_capacity = match &refusal {
                HostRefusal::Ownership(body) => body.capacity(),
                _ => 0,
            };
            assert!(input_capacity <= DELIVERY_ADMISSION_BODY_CAPACITY);
            let expected = refusal.to_string();
            let body = delivery_admission_body(refusal);
            assert_eq!(body, expected);
            assert!(body.capacity() <= DELIVERY_ADMISSION_BODY_CAPACITY);
            // Source keeps the owned input alive through the formatting write.
            // These are TWO producer bodies at the chosen conversion site,
            // not two caller sites or two fixed cells. No F is seeded here.
            assert!(input_capacity + body.capacity() <= 2 * DELIVERY_ADMISSION_BODY_CAPACITY);
        }
    }
}

/// Server-owned fixed representations across reserve, active, queued payload,
/// trait-frame and returned-result overlap. These are actual private types;
/// the one exact boxed key child MOVES with its canonical entry. Channels have
/// capacity one; the frames include the separate in-flight send payloads.
///
/// This is NOT yet a complete descriptor: neutral child facts, dynamic String/
/// Value/Vec capacities, library channel children, and original F/G wiring must
/// be composed by their actual owners. No positive creation follows this fact.
struct ForcedFixedStorage<
    Begin,
    ReservedAbort,
    ReservedSettle,
    Commit,
    Finish,
    Abort,
    Settle,
    Append,
> {
    _slot: Slot,
    _reserved: Reserved,
    _active: Active,
    _identity_child: crate::host_credit::RetainedDecisionStorage,
    _reservation: Reservation,
    _lease: ForcedLease,
    _delivery_adapter: DeliveryAdapter,
    _queued_command: Command,
    _queued_delivery: Delivery,
    _begin_frame: Begin,
    _reserved_abort_frame: ReservedAbort,
    _reserved_settle_frame: ReservedSettle,
    _commit_frame: Commit,
    _finish_frame: Finish,
    _abort_frame: Abort,
    _settle_frame: Settle,
    _append_frame: Append,
    // Output layouts do not live inside a completed Future's frame. Enumerate
    // each actual producer separately; these are not a custody/N proof.
    _reserve_result: Result<Box<dyn HostForcedReservation>, HostRefusal>,
    _lease_result: Result<Box<dyn HostForcedLease>, HostRefusal>,
    _begin_reply: Result<String, HostRefusal>,
    _commit_reply: Result<SubmitReceipt, HostRefusal>,
    _finish_reply: Result<Vec<DecisionRequest>, HostRefusal>,
    _abort_reply: Result<(), HostRefusal>,
    _settle_reply: Result<(), HostRefusal>,
    _delivery_reply: Result<u64, HostRefusal>,
    _append_result: Result<u64, String>,
    _reply_storage: ForcedReplyStorage,
}

/// Pure type facts. Function items identify the SAME unboxed concrete futures
/// actually erased by each production trait method; none is called/allocated.
pub(crate) fn forced_fixed_layout() -> Layout {
    fn layout<B, RA, RS, C, F, A, S, D>(
        _: impl FnOnce(&'static Reservation, PreparedRoute) -> B,
        _: impl FnOnce(&'static Reservation) -> RA,
        _: impl FnOnce(&'static Reservation) -> RS,
        _: impl FnOnce(&'static ForcedLease, ConfirmedIntent) -> C,
        _: impl FnOnce(&'static ForcedLease, &'static str, ConfirmedPostcommit) -> F,
        _: impl FnOnce(&'static ForcedLease) -> A,
        _: impl FnOnce(&'static ForcedLease) -> S,
        _: impl FnOnce(&'static DeliveryAdapter, &'static str, u64, &'static UnconfirmedEntry) -> D,
    ) -> Layout {
        Layout::new::<ForcedFixedStorage<B, RA, RS, C, F, A, S, D>>()
    }
    layout(
        Reservation::begin_future,
        Reservation::abort_future,
        Reservation::settle_future,
        ForcedLease::commit_future,
        ForcedLease::finish_future,
        ForcedLease::abort_future,
        ForcedLease::settle_future,
        DeliveryAdapter::append_future,
    )
}

/// The actual Server-owned prompt AND Forced fixed type graph. Owner-neutral
/// facts and creation/growth wiring are still required; this is not the final
/// trusted CampaignHandle descriptor and does not permit Operation creation.
struct HostFixedStorage<P, B, RA, RS, C, F, A, S, D> {
    _prompt: crate::host_gate::PromptFixedStorage<P>,
    _forced: ForcedFixedStorage<B, RA, RS, C, F, A, S, D>,
}
#[cfg(test)]
mod fatal_storage_tests {
    #[test]
    fn actual_active_state_fatal_field_is_only_a_copy_class() {
        // This binds the ACTUAL field type, not a stand-in storage marker.
        let field: fn(&super::ForcedState) -> Option<crate::writer_fatal::FatalClass> =
            |state| state.fatal;
        let _ = field;
        assert!(!std::mem::needs_drop::<
            Option<crate::writer_fatal::FatalClass>,
        >());
    }
}
pub(crate) fn host_owned_fixed_layout() -> Layout {
    fn layout<P, B, RA, RS, C, F, A, S, D>(
        _: impl FnOnce(
            &'static mpsc::Sender<crate::host_gate::PromptSettlement>,
            &'static WriterWake,
            &'static tokio::sync::Mutex<crate::host_reply::ReplyReceiver<Result<(), HostRefusal>>>,
            &'static Arc<Mutex<crate::host_gate::State>>,
            crate::host_gate::PromptOwner,
        ) -> P,
        _: impl FnOnce(&'static Reservation, PreparedRoute) -> B,
        _: impl FnOnce(&'static Reservation) -> RA,
        _: impl FnOnce(&'static Reservation) -> RS,
        _: impl FnOnce(&'static ForcedLease, ConfirmedIntent) -> C,
        _: impl FnOnce(&'static ForcedLease, &'static str, ConfirmedPostcommit) -> F,
        _: impl FnOnce(&'static ForcedLease) -> A,
        _: impl FnOnce(&'static ForcedLease) -> S,
        _: impl FnOnce(&'static DeliveryAdapter, &'static str, u64, &'static UnconfirmedEntry) -> D,
    ) -> Layout {
        Layout::new::<HostFixedStorage<P, B, RA, RS, C, F, A, S, D>>()
    }
    layout(
        crate::host_gate::prompt_settlement_future,
        Reservation::begin_future,
        Reservation::abort_future,
        Reservation::settle_future,
        ForcedLease::commit_future,
        ForcedLease::finish_future,
        ForcedLease::abort_future,
        ForcedLease::settle_future,
        DeliveryAdapter::append_future,
    )
}
