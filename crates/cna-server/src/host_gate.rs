//! Trusted host admission, ordered with the campaign writer. Never a seat API.
use std::{
    alloc::Layout,
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
};

use cna_core::{engine::Ruleset, ids::SeatId};
use cna_protocol::ControllerKind;
use cna_seats::{
    host::{
        HostAdmission, HostIdentity, HostPromptLease, HostRefusal, HostSnapshot, HostStatus,
        HostStep, HostStepLease, operation_refusal,
    },
    operation::Allowance,
};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use crate::{
    Campaign, CampaignStatus,
    host_reply::{FixedReply, ReplyOwner, ReplyReceiver, ReplySender},
};

pub(crate) type WriterWake = Arc<OnceLock<std::thread::Thread>>;
pub(crate) fn wake(writer: &WriterWake) {
    if let Some(writer) = writer.get() {
        writer.unpark();
    }
}
pub(crate) struct PromptSettlement {
    owner: PromptOwner,
    reply: ReplySender<Result<(), HostRefusal>>,
}
const SEAT_COUNT: usize = cna_protocol::Side::ALL.len() * cna_protocol::Role::ALL.len();
const PROMPT_OWNERSHIP_BODY: &str = "prompt still owned";
// m86f562554a81: the eventual original F must cover actual body capacity.
// This bound alone is not a live-body count or permission to create an Operation.
const PROMPT_OWNERSHIP_BODY_CAPACITY: usize = PROMPT_OWNERSHIP_BODY.len();

fn prompt_ownership_refusal() -> HostRefusal {
    let mut body = String::with_capacity(PROMPT_OWNERSHIP_BODY_CAPACITY);
    body.push_str(PROMPT_OWNERSHIP_BODY);
    HostRefusal::Ownership(body)
}

/// The original UUID value, never a copied or hashed String identity. Every
/// seat slot exists before any Operation and its contents cannot grow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PromptOwner {
    seat: SeatId,
    epoch: u64,
    revision: u32,
    operation: Uuid,
    generation: u64,
}
type PromptSlots = [Option<PromptOwner>; SEAT_COUNT];
impl PromptOwner {
    fn reply_owner(self) -> ReplyOwner {
        ReplyOwner {
            operation: self.operation,
            seat: self.seat,
            epoch: self.epoch,
            revision: self.revision,
            host_generation: self.generation,
        }
    }
}
fn seat_index(seat: SeatId) -> Result<usize, HostRefusal> {
    SeatId::all()
        .position(|candidate| candidate == seat)
        .ok_or(HostRefusal::IdentityChanged)
}
fn has_prompts(state: &State) -> bool {
    state.prompts.iter().any(Option::is_some)
}

// The existing PREPARE limit supplies the hard maximum; this is not a new cap.
// The complete boxed child, not just its pointer/header, is a creation-layout
// obligation for the pending owner descriptor and whole original-fit assembly.
type DecisionStorage = [u8; cna_seats::operation::PREPARE_BYTES];
struct PermitIdentity {
    owner: PromptOwner,
    decision_len: usize,
    decision: Box<DecisionStorage>,
}
impl PermitIdentity {
    fn owner(state: &State, identity: &HostIdentity) -> Result<PromptOwner, HostRefusal> {
        HostGate::check(state, identity, state.snapshot.generation)?;
        let operation =
            Uuid::parse_str(&identity.operation_id).map_err(|_| HostRefusal::IdentityChanged)?;
        if identity.decision_id.len() > cna_seats::operation::PREPARE_BYTES {
            return Err(HostRefusal::CaptureOverflow);
        }
        Ok(PromptOwner {
            seat: identity.seat,
            epoch: identity.epoch,
            revision: identity.revision,
            operation,
            generation: state.snapshot.generation,
        })
    }
    fn bind(state: &State, identity: &HostIdentity) -> Result<Self, HostRefusal> {
        let owner = Self::owner(state, identity)?;
        let mut decision = Box::new([0; cna_seats::operation::PREPARE_BYTES]);
        decision[..identity.decision_id.len()].copy_from_slice(identity.decision_id.as_bytes());
        Ok(Self {
            owner,
            decision_len: identity.decision_id.len(),
            decision,
        })
    }
    fn check(&self, state: &State) -> Result<(), HostRefusal> {
        if !state.known || state.writing {
            return Err(HostRefusal::Unknown);
        }
        if state.recovered {
            return Err(HostRefusal::WaitingForActivation);
        }
        if !state.snapshot.activated
            || state.deferred
            || state.forced
            || state.snapshot.status != HostStatus::Running
        {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        if state.snapshot.generation != self.owner.generation
            || !state.eligible.contains(&self.owner.seat)
            || state.snapshot.epochs.get(&self.owner.seat) != Some(&self.owner.epoch)
            || !state
                .snapshot
                .pending
                .get(&self.owner.seat)
                .is_some_and(|pending| {
                    pending.iter().any(|(id, revision)| {
                        id.as_bytes() == &self.decision[..self.decision_len]
                            && *revision == self.owner.revision
                    })
                })
        {
            return Err(HostRefusal::IdentityChanged);
        }
        Ok(())
    }
}
pub(crate) struct State {
    snapshot: HostSnapshot,
    eligible: BTreeSet<SeatId>,
    known: bool,
    writing: bool,
    deferred: bool,
    forced: bool,
    recovered: bool,
    prompts: PromptSlots,
    prompt_senders: [Option<ReplySender<Result<(), HostRefusal>>>; SEAT_COUNT],
}
pub(crate) struct HostGate {
    state: Arc<Mutex<State>>,
    changed: watch::Sender<HostSnapshot>,
    settlements: mpsc::Sender<PromptSettlement>,
    replies: [Arc<FixedReply<Result<(), HostRefusal>>>; SEAT_COUNT],
    wake: WriterWake,
}

fn snapshot<R: Ruleset>(
    campaign: &Campaign<R>,
    generation: u64,
) -> (HostSnapshot, BTreeSet<SeatId>) {
    let status = match campaign.status() {
        CampaignStatus::Running => HostStatus::Running,
        CampaignStatus::Paused => HostStatus::Paused,
        CampaignStatus::Finished { .. } => HostStatus::Finished,
        CampaignStatus::Stopped { .. } => HostStatus::Unknown,
    };
    let mut pending: BTreeMap<_, Vec<_>> = SeatId::all().map(|s| (s, Vec::new())).collect();
    for request in campaign.pending() {
        pending
            .entry(request.seat)
            .or_default()
            .push((request.id.to_string(), request.revision));
    }
    let eligible = SeatId::all()
        .filter(|&seat| {
            let binding = campaign.binding(seat);
            !binding.paused
                && binding.failure.is_none()
                && binding.controller.as_ref().is_some_and(|c| {
                    matches!(c.kind, ControllerKind::LlmCli | ControllerKind::System1)
                })
        })
        .collect();
    (
        HostSnapshot {
            generation,
            activated: status == HostStatus::Running && !campaign.recovered_awaiting_activation(),
            status,
            epochs: SeatId::all()
                .map(|s| (s, campaign.binding(s).controller_epoch))
                .collect(),
            pending,
        },
        eligible,
    )
}

impl HostGate {
    pub(crate) fn new<R: Ruleset>(
        campaign: &Campaign<R>,
        wake: WriterWake,
    ) -> (Arc<Self>, mpsc::Receiver<PromptSettlement>) {
        let (snapshot, eligible) = snapshot(campaign, 1);
        let (changed, _) = watch::channel(snapshot.clone());
        let (settlements, receiver) = mpsc::channel(1);
        (
            Arc::new(Self {
                state: Arc::new(Mutex::new(State {
                    snapshot,
                    eligible,
                    known: true,
                    writing: false,
                    deferred: false,
                    forced: false,
                    recovered: campaign.recovered_awaiting_activation(),
                    prompts: [None; SEAT_COUNT],
                    prompt_senders: std::array::from_fn(|_| None),
                })),
                changed,
                settlements,
                replies: std::array::from_fn(|_| FixedReply::new()),
                wake,
            }),
            receiver,
        )
    }
    fn check(state: &State, identity: &HostIdentity, generation: u64) -> Result<(), HostRefusal> {
        if !state.known || state.writing {
            return Err(HostRefusal::Unknown);
        }
        if state.recovered {
            return Err(HostRefusal::WaitingForActivation);
        }
        if !state.snapshot.activated
            || state.deferred
            || state.forced
            || state.snapshot.status != HostStatus::Running
        {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        if state.snapshot.generation != generation
            || identity.operation_id.is_empty()
            || !state.eligible.contains(&identity.seat)
            || state.snapshot.epochs.get(&identity.seat) != Some(&identity.epoch)
            || !state
                .snapshot
                .pending
                .get(&identity.seat)
                .is_some_and(|pending| {
                    pending.iter().any(|(id, revision)| {
                        id == &identity.decision_id && revision == &identity.revision
                    })
                })
        {
            return Err(HostRefusal::IdentityChanged);
        }
        Ok(())
    }
    /// The writer's control/exit boundary includes the campaign-owned forced
    /// reservation, even before a per-sink route or BEGIN reply exists. Closing
    /// admission does not clear either kind of ownership. Unknown exclusion
    /// retains the writer rather than treating poison as a settled operation.
    pub(crate) fn has_outstanding(&self) -> bool {
        self.state
            .lock()
            .map(|s| s.forced || has_prompts(&s))
            .unwrap_or(true)
    }
    // Called by the writer before a conflicting control can change anything.
    // Closing admission does NOT release leases or imply native cleanup.
    pub(crate) fn defer_controls(&self, deferred: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.deferred = deferred;
            let mut announced = state.snapshot.clone();
            announced.activated &= !deferred && !state.forced && state.known && !state.writing;
            self.changed.send_replace(announced);
        }
    }
    /// Enter writer mutation after the last prompt has actually acknowledged
    /// settlement. The shared short lock orders native perform_sync against this
    /// invalidation, then is released BEFORE any synchronous SQLite operation.
    pub(crate) fn begin_write(&self) -> Result<(), HostRefusal> {
        let mut state = self.state.lock().map_err(|_| HostRefusal::Unknown)?;
        if !state.known || state.writing {
            return Err(HostRefusal::Unknown);
        }
        if has_prompts(&state) || state.forced {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        state.snapshot.generation = state
            .snapshot
            .generation
            .checked_add(1)
            .ok_or(HostRefusal::Unknown)?;
        state.writing = true;
        let mut announced = state.snapshot.clone();
        announced.activated = false;
        self.changed.send_replace(announced);
        Ok(())
    }
    pub(crate) fn claim_forced(&self, identity: &HostIdentity) -> Result<u64, HostRefusal> {
        let mut state = self.state.lock().map_err(|_| HostRefusal::Unknown)?;
        Self::check(&state, identity, state.snapshot.generation)?;
        if has_prompts(&state) {
            return Err(HostRefusal::StoppedBeforeWork);
        }
        state.snapshot.generation = state
            .snapshot
            .generation
            .checked_add(1)
            .ok_or(HostRefusal::Unknown)?;
        state.forced = true;
        let mut announced = state.snapshot.clone();
        announced.activated = false;
        self.changed.send_replace(announced);
        Ok(state.snapshot.generation)
    }
    pub(crate) fn release_forced(&self) -> Result<(), HostRefusal> {
        let mut state = self.state.lock().map_err(|_| HostRefusal::Unknown)?;
        state.forced = false;
        let mut announced = state.snapshot.clone();
        announced.activated &= !state.deferred && !state.writing && state.known;
        self.changed.send_replace(announced);
        Ok(())
    }
    pub(crate) fn publish<R: Ruleset>(&self, campaign: &Campaign<R>) {
        if let Ok(mut state) = self.state.lock() {
            let (next, eligible) = snapshot(campaign, state.snapshot.generation);
            state.snapshot = next;
            state.eligible = eligible;
            state.recovered = campaign.recovered_awaiting_activation();
            state.writing = false;
            // Publication after a fatal writer stop must not reopen admission.
            // A closed gate has no automatic recovery/activation escape hatch.
            let mut announced = state.snapshot.clone();
            announced.activated &= state.known && !state.deferred && !state.forced;
            if !state.known {
                announced.status = HostStatus::Unknown;
            }
            self.changed.send_replace(announced);
        }
    }
    pub(crate) fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.known = false;
            let mut announced = state.snapshot.clone();
            announced.activated = false;
            announced.status = HostStatus::Unknown;
            self.changed.send_replace(announced);
        }
    }
    pub(crate) fn settle_prompt(&self, request: PromptSettlement) {
        let mut retained_reply = None;
        let result = (|| {
            let mut state = self.state.lock().map_err(|_| HostRefusal::Unknown)?;
            let index = seat_index(request.owner.seat)?;
            if state.prompts[index] != Some(request.owner) {
                return Err(HostRefusal::IdentityChanged);
            }
            state.prompts[index] = None;
            retained_reply = state.prompt_senders[index].take();
            Ok(())
        })();
        // ACK is after the actual ordered removal. It is not a child/reader join,
        // command-credit release or permission to refund possible-start liability.
        let success = result.is_ok();
        let _ = request.reply.send(result);
        if success {
            let _ = self.replies[seat_index(request.owner.seat).expect("validated seat")]
                .settle(request.owner.reply_owner());
        }
        // A canceled queued transient cannot close this original sender before
        // ordered completion. After completion it owns no second reply.
        drop(retained_reply);
    }
}

struct StepPermit {
    state: Arc<Mutex<State>>,
    identity: PermitIdentity,
}
impl HostStepLease for StepPermit {
    fn perform_sync(
        &mut self,
        effect: &mut dyn FnMut() -> Result<(), HostRefusal>,
    ) -> Result<(), HostRefusal> {
        let state = self.state.lock().map_err(|_| HostRefusal::Unknown)?;
        self.identity.check(&state)?;
        // Required HostDriverContext also checks its operation allowance inside
        // this bounded effect. No actor/storage await is permitted under the lock.
        effect()
    }
}
struct PromptPermit {
    state: Arc<Mutex<State>>,
    identity: PermitIdentity,
    allowance: Allowance,
    settlements: mpsc::Sender<PromptSettlement>,
    reply: tokio::sync::Mutex<ReplyReceiver<Result<(), HostRefusal>>>,
    wake: WriterWake,
}
impl HostPromptLease for PromptPermit {
    fn check(&self) -> Result<(), HostRefusal> {
        self.allowance.check().map_err(operation_refusal)?;
        let state = self.state.lock().map_err(|_| HostRefusal::Unknown)?;
        self.identity.check(&state)?;
        if state.prompts[seat_index(self.identity.owner.seat)?] != Some(self.identity.owner) {
            return Err(HostRefusal::IdentityChanged);
        }
        Ok(())
    }
    fn settle<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), HostRefusal>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        // This is the same trait signature produced by async_trait. The real
        // concrete future below can now contribute its actual Layout BEFORE
        // erasure rather than counting only the erased Box pointer.
        Box::pin(prompt_settlement_future(
            &self.settlements,
            &self.wake,
            &self.reply,
            &self.state,
            self.identity.owner,
        ))
    }
}
pub(crate) async fn prompt_settlement_future(
    settlements: &mpsc::Sender<PromptSettlement>,
    writer: &WriterWake,
    receiver: &tokio::sync::Mutex<ReplyReceiver<Result<(), HostRefusal>>>,
    state: &Arc<Mutex<State>>,
    owner: PromptOwner,
) -> Result<(), HostRefusal> {
    let mut wait = receiver.lock().await;
    let reply = {
        let state = state.lock().map_err(|_| HostRefusal::Unknown)?;
        let index = seat_index(owner.seat)?;
        if state.prompts[index] != Some(owner) {
            return Err(HostRefusal::IdentityChanged);
        }
        state.prompt_senders[index]
            .as_ref()
            .ok_or(HostRefusal::Unknown)?
            .reborrow()
    };
    settlements
        .send(PromptSettlement { owner, reply })
        .await
        .map_err(|_| HostRefusal::Unknown)?;
    wake(writer);
    wait.receive().await.map_err(crate::host_reply::refusal)?
}

/// Actual fixed prompt representations, including BOTH boxed decision children
/// and the same concrete settlement future used by the production trait entry.
/// This is one component of complete F, NOT a full forced/owner descriptor.
pub(crate) struct PromptFixedStorage<F> {
    _slot: Option<PromptOwner>,
    _step: StepPermit,
    _step_decision: DecisionStorage,
    _prompt: PromptPermit,
    _prompt_decision: DecisionStorage,
    _settlement: PromptSettlement,
    _settlement_future: F,
    // Synchronous admission outputs are distinct from the settlement output.
    // Strict-caller custody, not these type facts, must bound their lifetimes.
    _step_admission_result: Result<Box<dyn HostStepLease>, HostRefusal>,
    _prompt_admission_result: Result<Box<dyn HostPromptLease>, HostRefusal>,
    _returned_result: Result<(), HostRefusal>,
    _reply_cell: FixedReply<Result<(), HostRefusal>>,
    _root_reply: ReplySender<Result<(), HostRefusal>>,
}
pub(crate) fn prompt_fixed_layout() -> Layout {
    fn layout<F>(
        _: impl FnOnce(
            &'static mpsc::Sender<PromptSettlement>,
            &'static WriterWake,
            &'static tokio::sync::Mutex<ReplyReceiver<Result<(), HostRefusal>>>,
            &'static Arc<Mutex<State>>,
            PromptOwner,
        ) -> F,
    ) -> Layout {
        Layout::new::<PromptFixedStorage<F>>()
    }
    // Infer the actual production frame from its function item without calling
    // it. No future, owner/key, Operation, allowance or channel is created.
    layout(prompt_settlement_future)
}
// No Drop release: caller cancellation/drop cannot establish supervision settlement.
impl HostAdmission for HostGate {
    fn subscribe(&self) -> watch::Receiver<HostSnapshot> {
        self.changed.subscribe()
    }
    fn snapshot(&self) -> Result<HostSnapshot, HostRefusal> {
        let state = self.state.lock().map_err(|_| HostRefusal::Unknown)?;
        if !state.known || state.writing {
            return Err(HostRefusal::Unknown);
        }
        let mut snapshot = state.snapshot.clone();
        snapshot.activated &= !state.deferred && !state.forced;
        Ok(snapshot)
    }
    fn acquire(
        &self,
        identity: &HostIdentity,
        _step: HostStep,
    ) -> Result<Box<dyn HostStepLease>, HostRefusal> {
        let state = self.state.lock().map_err(|_| HostRefusal::Unknown)?;
        let identity = PermitIdentity::bind(&state, identity)?;
        Ok(Box::new(StepPermit {
            state: self.state.clone(),
            identity,
        }))
    }
    fn acquire_prompt(
        &self,
        identity: &HostIdentity,
        allowance: Allowance,
    ) -> Result<Box<dyn HostPromptLease>, HostRefusal> {
        allowance.check().map_err(operation_refusal)?;
        let mut state = self.state.lock().map_err(|_| HostRefusal::Unknown)?;
        let owner = PermitIdentity::owner(&state, identity)?;
        let index = seat_index(owner.seat)?;
        if state.prompts[index].is_some() {
            return Err(prompt_ownership_refusal());
        }
        let identity = PermitIdentity::bind(&state, identity)?;
        let (reply, wait) = self.replies[index]
            .claim(owner.reply_owner())
            .map_err(crate::host_reply::refusal)?;
        state.prompt_senders[index] = Some(reply);
        state.prompts[index] = Some(identity.owner);
        Ok(Box::new(PromptPermit {
            state: self.state.clone(),
            identity,
            allowance,
            settlements: self.settlements.clone(),
            reply: tokio::sync::Mutex::new(wait),
            wake: self.wake.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn actual_busy_prompt_bodies_have_bounded_capacity_but_outlive_the_slot_call() {
        let (gate, _receiver, mut identity) = fixture();
        identity.epoch = u64::MAX;
        identity.revision = u32::MAX;
        identity.operation_id = Uuid::from_u128(u128::MAX).to_string();
        identity.decision_id = "d".repeat(cna_seats::operation::PREPARE_BYTES);
        {
            let mut state = gate.state.lock().unwrap();
            state.snapshot.epochs.insert(identity.seat, identity.epoch);
            state.snapshot.pending.insert(
                identity.seat,
                vec![(identity.decision_id.clone(), identity.revision)],
            );
        }
        let _permit = gate.acquire_prompt(&identity, allowance()).unwrap();
        let owner = gate.state.lock().unwrap().prompts[seat_index(identity.seat).unwrap()];
        let mut retained = Vec::new();
        // A small witness, NOT a proposed maximum N: none of these returned
        // bodies is retained by the single original prompt slot.
        for _ in 0..3 {
            let error = match gate.acquire_prompt(&identity, allowance()) {
                Err(error) => error,
                Ok(_) => panic!("busy prompt unexpectedly acquired"),
            };
            let HostRefusal::Ownership(body) = error else {
                panic!("busy prompt changed refusal type");
            };
            assert_eq!(body, "prompt still owned");
            assert!(body.capacity() <= PROMPT_OWNERSHIP_BODY_CAPACITY);
            retained.push(body);
        }
        assert_eq!(retained.len(), 3);
        assert_eq!(
            gate.state.lock().unwrap().prompts[seat_index(identity.seat).unwrap()],
            owner,
        );
        assert!(retained.iter().all(|body| body == "prompt still owned"));
    }

    fn fixture() -> (
        Arc<HostGate>,
        mpsc::Receiver<PromptSettlement>,
        HostIdentity,
    ) {
        let seat = SeatId::all().next().unwrap();
        let identity = HostIdentity {
            seat,
            epoch: 3,
            decision_id: "synthetic".into(),
            revision: 4,
            operation_id: Uuid::from_u128(1).to_string(),
        };
        let snapshot = HostSnapshot {
            generation: 1,
            activated: true,
            status: HostStatus::Running,
            epochs: BTreeMap::from([(seat, 3)]),
            pending: BTreeMap::from([(seat, vec![("synthetic".into(), 4)])]),
        };
        let (changed, _) = watch::channel(snapshot.clone());
        let (settlements, receiver) = mpsc::channel(1);
        let gate = Arc::new(HostGate {
            state: Arc::new(Mutex::new(State {
                snapshot,
                eligible: BTreeSet::from([seat]),
                known: true,
                writing: false,
                deferred: false,
                forced: false,
                recovered: false,
                prompts: [None; SEAT_COUNT],
                prompt_senders: std::array::from_fn(|_| None),
            })),
            changed,
            settlements,
            replies: std::array::from_fn(|_| FixedReply::new()),
            wake: Arc::new(OnceLock::new()),
        });
        (gate, receiver, identity)
    }
    fn allowance() -> Allowance {
        // Synthetic test allowance only, no changed production decision guard.
        Allowance::required(Some(
            tokio::time::Instant::now() + std::time::Duration::from_secs(5),
        ))
        .unwrap()
    }

    #[test]
    fn recovered_closed_unknown_and_stale_permits_never_execute_effect() {
        let (gate, _receiver, identity) = fixture();
        let executions = AtomicUsize::new(0);
        let mut permit = gate.acquire(&identity, HostStep::SessionSpawn).unwrap();
        gate.begin_write().unwrap();
        assert!(matches!(
            permit.perform_sync(&mut || {
                executions.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
            Err(HostRefusal::Unknown)
        ));
        {
            let mut state = gate.state.lock().unwrap();
            state.writing = false;
            state.recovered = true;
            state.snapshot.activated = false;
        }
        assert!(matches!(
            gate.acquire(&identity, HostStep::Resume),
            Err(HostRefusal::WaitingForActivation)
        ));
        {
            let mut state = gate.state.lock().unwrap();
            state.recovered = false;
            state.snapshot.activated = true;
        }
        assert!(matches!(
            permit.perform_sync(&mut || {
                executions.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
            Err(HostRefusal::IdentityChanged)
        ));
        gate.close();
        assert!(matches!(gate.snapshot(), Err(HostRefusal::Unknown)));
        assert_eq!(executions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn effect_uses_writer_exclusion_and_revoked_epoch_never_spawns() {
        let (gate, _receiver, identity) = fixture();
        let mut permit = gate.acquire(&identity, HostStep::PreflightSpawn).unwrap();
        permit
            .perform_sync(&mut || {
                assert!(
                    gate.state.try_lock().is_err(),
                    "writer must be excluded during bounded synchronous effect"
                );
                Ok(())
            })
            .unwrap();
        gate.state
            .lock()
            .unwrap()
            .snapshot
            .epochs
            .insert(identity.seat, 4);
        assert!(matches!(
            permit.perform_sync(&mut || panic!("revoked effect ran")),
            Err(HostRefusal::IdentityChanged)
        ));
    }

    #[tokio::test]
    async fn prompt_control_conflict_waits_for_actual_ordered_ack_not_drop_or_expiry() {
        let (gate, mut receiver, identity) = fixture();
        let permit = gate.acquire_prompt(&identity, allowance()).unwrap();
        gate.defer_controls(true);
        assert!(matches!(
            gate.begin_write(),
            Err(HostRefusal::StoppedBeforeWork)
        ));
        assert!(matches!(
            permit.check(),
            Err(HostRefusal::StoppedBeforeWork)
        ));
        assert!(matches!(
            gate.acquire(&identity, HostStep::Reserve),
            Err(HostRefusal::StoppedBeforeWork)
        ));
        let settlement = tokio::spawn(async move { permit.settle().await });
        let request = receiver.recv().await.unwrap();
        assert!(gate.has_outstanding(), "enqueue is not settlement");
        gate.settle_prompt(request);
        settlement.await.unwrap().unwrap();
        assert!(!gate.has_outstanding());
        gate.begin_write().unwrap();
    }

    #[test]
    fn lost_prompt_handle_retains_ownership_and_prevents_writer_mutation() {
        let (gate, _receiver, identity) = fixture();
        let permit = gate.acquire_prompt(&identity, allowance()).unwrap();
        drop(permit);
        assert!(gate.has_outstanding());
        assert!(matches!(
            gate.begin_write(),
            Err(HostRefusal::StoppedBeforeWork)
        ));
    }

    #[test]
    fn forced_operation_blocks_effects_and_controls_until_explicit_writer_release() {
        let (gate, _receiver, identity) = fixture();
        let mut prior = gate.acquire(&identity, HostStep::SessionSpawn).unwrap();
        gate.claim_forced(&identity).unwrap();
        assert!(gate.has_outstanding());
        assert!(!gate.snapshot().unwrap().activated);
        assert!(matches!(
            prior.perform_sync(&mut || panic!("forced owner did not exclude old effect")),
            Err(HostRefusal::StoppedBeforeWork)
        ));
        assert!(matches!(
            gate.begin_write(),
            Err(HostRefusal::StoppedBeforeWork)
        ));
        assert!(matches!(
            gate.acquire(&identity, HostStep::Reserve),
            Err(HostRefusal::StoppedBeforeWork)
        ));
        gate.defer_controls(true);
        gate.release_forced().unwrap();
        assert!(!gate.has_outstanding());
        assert!(
            !gate.snapshot().unwrap().activated,
            "deferred control remains closed after forced release"
        );
        gate.defer_controls(false);
        // Prior permits stay stale even after explicit release; new acquisition
        // uses the current generation. No deadline/drop substitutes for release.
        assert!(matches!(
            prior.perform_sync(&mut || panic!("stale effect ran")),
            Err(HostRefusal::IdentityChanged)
        ));
        gate.acquire(&identity, HostStep::Reserve).unwrap();
    }

    #[test]
    fn closed_writer_retains_reserved_ownership_until_actual_release() {
        let (gate, _receiver, identity) = fixture();
        gate.claim_forced(&identity).unwrap();
        // Writer closure between campaign reserve and sink prepare does not
        // imply that caller supervision, route issue or settlement has happened.
        gate.close();
        assert!(gate.has_outstanding());
        assert!(matches!(
            gate.claim_forced(&identity),
            Err(HostRefusal::Unknown)
        ));
        assert!(matches!(gate.begin_write(), Err(HostRefusal::Unknown)));
        gate.release_forced().unwrap();
        assert!(!gate.has_outstanding());
        assert!(
            matches!(gate.snapshot(), Err(HostRefusal::Unknown)),
            "final ownership release cannot reopen a dead writer"
        );
    }

    #[test]
    fn prompt_slots_use_original_uuid_and_foreign_settlement_cannot_release() {
        let (gate, _receiver, identity) = fixture();
        let _permit = gate.acquire_prompt(&identity, allowance()).unwrap();
        let index = seat_index(identity.seat).unwrap();
        let owner = gate.state.lock().unwrap().prompts[index].unwrap();
        assert_eq!(
            owner.operation,
            Uuid::parse_str(&identity.operation_id).unwrap()
        );
        assert_eq!(owner.epoch, identity.epoch);
        assert_eq!(owner.revision, identity.revision);
        assert!(matches!(
            gate.acquire_prompt(&identity, allowance()),
            Err(HostRefusal::Ownership(_))
        ));
        let mut foreign_owners = [owner; 4];
        foreign_owners[0].operation = Uuid::from_u128(2);
        foreign_owners[1].epoch += 1;
        foreign_owners[2].revision += 1;
        foreign_owners[3].generation += 1;
        for foreign in foreign_owners {
            // Each different original UUID/epoch/revision/generation is denied
            // independently; no string alias or truncated key is stored.
            let cell = FixedReply::new();
            let (reply, mut result) = cell.claim(foreign.reply_owner()).unwrap();
            gate.settle_prompt(PromptSettlement {
                owner: foreign,
                reply,
            });
            assert!(matches!(
                result.try_recv().unwrap(),
                Err(HostRefusal::IdentityChanged)
            ));
            assert_eq!(gate.state.lock().unwrap().prompts[index], Some(owner));
        }
        let cell = FixedReply::new();
        let (reply, mut result) = cell.claim(owner.reply_owner()).unwrap();
        gate.settle_prompt(PromptSettlement { owner, reply });
        result.try_recv().unwrap().unwrap();
        assert!(!gate.has_outstanding());
        assert!(
            gate.state
                .lock()
                .unwrap()
                .prompts
                .iter()
                .all(Option::is_none)
        );
        let mut next = identity;
        next.operation_id = Uuid::from_u128(2).to_string();
        gate.acquire_prompt(&next, allowance()).unwrap();
        assert_eq!(
            gate.state.lock().unwrap().prompts[index].unwrap().operation,
            Uuid::from_u128(2)
        );
    }

    #[test]
    fn fixed_identity_rejects_oversize_or_unknown_uuid_before_slot_mutation() {
        let (gate, _receiver, mut identity) = fixture();
        let original = gate.state.lock().unwrap().prompts;
        identity.operation_id = "not-the-original-uuid".into();
        assert!(matches!(
            gate.acquire_prompt(&identity, allowance()),
            Err(HostRefusal::IdentityChanged)
        ));
        assert_eq!(gate.state.lock().unwrap().prompts, original);
        identity.operation_id = Uuid::from_u128(1).to_string();
        identity.decision_id = "x".repeat(cna_seats::operation::PREPARE_BYTES + 1);
        gate.state
            .lock()
            .unwrap()
            .snapshot
            .pending
            .get_mut(&identity.seat)
            .unwrap()[0]
            .0 = identity.decision_id.clone();
        assert!(matches!(
            gate.acquire_prompt(&identity, allowance()),
            Err(HostRefusal::CaptureOverflow)
        ));
        assert!(matches!(
            gate.acquire(&identity, HostStep::SessionSpawn),
            Err(HostRefusal::CaptureOverflow)
        ));
        assert_eq!(gate.state.lock().unwrap().prompts, original);
        assert!(!gate.has_outstanding());
    }

    #[test]
    fn prompt_layout_counts_whole_children_and_actual_unpolled_frame_without_effects() {
        let (gate, mut receiver, identity) = fixture();
        let original = gate.state.lock().unwrap().prompts;
        let owner = PermitIdentity::owner(&gate.state.lock().unwrap(), &identity).unwrap();
        let cell = FixedReply::new();
        let (_send, receive) = cell.claim(owner.reply_owner()).unwrap();
        let receive = tokio::sync::Mutex::new(receive);
        let future =
            prompt_settlement_future(&gate.settlements, &gate.wake, &receive, &gate.state, owner);
        let frame = Layout::for_value(&future);
        let layout = prompt_fixed_layout();
        let fields = [
            Layout::new::<Option<PromptOwner>>(),
            Layout::new::<StepPermit>(),
            Layout::new::<DecisionStorage>(),
            Layout::new::<PromptPermit>(),
            Layout::new::<DecisionStorage>(),
            Layout::new::<PromptSettlement>(),
            frame,
            Layout::new::<Result<(), HostRefusal>>(),
            Layout::new::<FixedReply<Result<(), HostRefusal>>>(),
            Layout::new::<ReplySender<Result<(), HostRefusal>>>(),
        ];
        let sum: usize = fields.iter().map(Layout::size).sum();
        assert!(layout.size() >= sum);
        assert!(layout.align() >= fields.iter().map(Layout::align).max().unwrap());
        assert_eq!(
            std::mem::size_of::<DecisionStorage>(),
            cna_seats::operation::PREPARE_BYTES
        );
        assert!(layout.size() >= 2 * std::mem::size_of::<DecisionStorage>());
        assert_eq!(gate.state.lock().unwrap().prompts, original);
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert!(!gate.has_outstanding());
    }
}
