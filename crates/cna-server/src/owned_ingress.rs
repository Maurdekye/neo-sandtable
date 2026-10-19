//! Held concrete creation ingress. No live App/main route calls this yet.
//!
//! Request observers enqueue/borrow; only the original outer owner dispatches
//! and polls factory tasks. Actual writer publication storage predates dispatch.
//! No hidden supervisor, arbitrary legacy Factory, detached task, Drop join or
//! runtime discovery. Host OPEN/member validation, HTTP observer adaptation,
//! registration/GO and full F/G/N remain separate unfinished integration duties.
//! The complete N-slot body, Arc/Notify/task backing and every input/result/error
//! child must be prepaid before construction. N/R are supplied by that producer;
//! they are not a new numeric campaign limit or an allocation/fit exemption.

use crate::{
    Error,
    actor::{CampaignHandle, OwnedStop, StopObservation},
    http::CreateRequest,
    owned_retained::{JoinObservation, RetainedJoin},
    owned_writer::{WriterJoin, WriterStartFailure, WriterPreparedIdentity, WriterEvidenceRefusal},
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, atomic::{AtomicU8, Ordering}},
    task::{Context, Poll},
};
use tokio::{runtime::Handle, sync::Notify, task::{JoinError, JoinHandle}, time::Instant};

const BLOCKED: u8 = 0;
const GO: u8 = 1;
const CANCELLED: u8 = 2;
const OBSERVING: u8 = 0;
const RETURNED: u8 = 1;
const JOIN_FAILED: u8 = 2;
const NOT_STARTED: u8 = 3;
const UNKNOWN: u8 = 4;

/// Owned inputs remain in the attempt even when the HTTP observer disappears.
/// Recovery is a trusted launcher input, never an HTTP-supplied arbitrary path.
#[derive(Debug)]
pub(crate) enum CreationInput {
    Fresh(CreateRequest),
    Recover(PathBuf),
    /// Supporting owned recovery holds its sole original path in WriterRecord.
    /// Only the private guarded driver consumes this; legacy dispatch refuses it.
    GuardedRecovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CreatorKey { incarnation: uuid::Uuid, index: usize }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IngressRefusal {
    Closed, Full, ForeignAttempt, MissingAttempt, Poisoned, ShutdownNotRequested, NoStopRoot,
    Title(crate::owned_row_envelope::TitleRefusal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparedAccessRefusal {
    Ingress(IngressRefusal),
    PendingCreator,
    UnknownCreator,
    CreatorJoinFailed,
    UnsuccessfulProduct,
    Stored(WriterEvidenceRefusal),
}

/// Synchronous gate on the existing blocking factory worker. A worker records
/// its Thread before checking the atomic; a prior GO/CANCEL needs no unpark.
/// Otherwise a later unpark has a retained permit. No additional thread exists.
struct FactoryGate {
    state: AtomicU8,
    worker: OnceLock<std::thread::Thread>,
}

impl FactoryGate {
    fn new() -> Self { Self { state: AtomicU8::new(BLOCKED), worker: OnceLock::new() } }

    fn wait(&self) -> bool {
        self.worker.get_or_init(std::thread::current);
        loop {
            match self.state.load(Ordering::Acquire) {
                GO => return true,
                CANCELLED => return false,
                _ => std::thread::park(),
            }
        }
    }

    /// Called while the admission mutex establishes the ordering with CLOSE.
    /// No wake or user callback is invoked under that mutex.
    fn decide(&self, open: bool) {
        let next = if open { GO } else { CANCELLED };
        let _ = self.state.compare_exchange(BLOCKED, next, Ordering::Release, Ordering::Relaxed);
    }

    fn wake(&self) {
        if let Some(worker) = self.worker.get() { worker.unpark(); }
    }
}

type Product = Result<CampaignHandle, WriterStartFailure>;
pub(crate) enum CreatorExit { Produced, CancelledBeforeEffects }
type FactoryJoin = RetainedJoin<JoinHandle<CreatorExit>, Result<CreatorExit, JoinError>>;

struct ReplyRoot {
    gate: FactoryGate,
    custody_gate: FactoryGate,
    custody_requested: OnceLock<()>,
    #[cfg(test)] test_preparation_hold: OnceLock<FactoryGate>,
    #[cfg(test)] test_preparation_reached: OnceLock<()>,
    #[cfg(test)] test_custody_notified: OnceLock<()>,
    product: OnceLock<Product>,
    status: AtomicU8,
    changed: Notify,
}

impl ReplyRoot {
    fn new() -> Self {
        Self { gate: FactoryGate::new(), custody_gate: FactoryGate::new(),
            custody_requested: OnceLock::new(), product: OnceLock::new(),
            #[cfg(test)] test_preparation_hold: OnceLock::new(),
            #[cfg(test)] test_preparation_reached: OnceLock::new(),
            #[cfg(test)] test_custody_notified: OnceLock::new(),
            status: AtomicU8::new(OBSERVING), changed: Notify::new() }
    }
}

struct Admission<const N: usize> {
    inputs: [Option<CreationInput>; N],
    used: usize,
    closed: bool,
}

/// The same original callback boundary for later Host/deadline wiring. This
/// stores no newly invented shutdown duration or proof that anything joined.
struct ScopeClose {
    started_at: Instant,
    result: Result<(), IngressRefusal>,
}

struct Shared<const N: usize> {
    incarnation: uuid::Uuid,
    admission: Mutex<Admission<N>>,
    replies: [ReplyRoot; N],
    owner_changed: Notify,
    close_request: OnceLock<ScopeClose>,
}

/// Cloneable observer/ingress only. It cannot take a task, writer, original raw
/// join failure or product. The original owner retains the same Shared Arc.
pub(crate) struct CreationObserver<const N: usize> { shared: Arc<Shared<N>> }

impl<const N: usize> Clone for CreationObserver<N> {
    fn clone(&self) -> Self { Self { shared: Arc::clone(&self.shared) } }
}

pub(crate) enum CreationObservation<'a> {
    Pending,
    /// Concrete output is retained, but writer GO/registration/Host member
    /// binding are NOT established by this observation and remain closed.
    Returned(&'a Product),
    /// Raw JoinError remains in the original owner; this neutral tag does not
    /// format or replace it. No product/absence can prove zero created writers.
    JoinFailed,
    CancelledBeforeEffects,
    Unknown,
}

impl<const N: usize> CreationObserver<N> {
    pub(crate) fn submit(
        &self,
        original: CreationInput,
    ) -> Result<CreatorKey, (IngressRefusal, CreationInput)> {
        if let CreationInput::Fresh(request) = &original {
            if let Err(cause) = crate::owned_row_envelope::title(&request.title) {
                return Err((IngressRefusal::Title(cause), original));
            }
        }
        let key = {
            let mut admission = match self.shared.admission.lock() {
                Ok(admission) => admission,
                Err(_) => return Err((IngressRefusal::Poisoned, original)),
            };
            if admission.closed { return Err((IngressRefusal::Closed, original)); }
            if admission.used == N { return Err((IngressRefusal::Full, original)); }
            let index = admission.used;
            admission.inputs[index] = Some(original);
            admission.used += 1;
            CreatorKey { incarnation: self.shared.incarnation, index }
        };
        self.shared.owner_changed.notify_one(); // After original input is held.
        Ok(key)
    }

    fn reply(&self, key: CreatorKey) -> Result<&ReplyRoot, IngressRefusal> {
        if key.incarnation != self.shared.incarnation { return Err(IngressRefusal::ForeignAttempt); }
        let used = self.shared.admission.lock().map_err(|_| IngressRefusal::Poisoned)?.used;
        if key.index >= used { return Err(IngressRefusal::MissingAttempt); }
        self.shared.replies.get(key.index).ok_or(IngressRefusal::MissingAttempt)
    }

    pub(crate) fn observe(&self, key: CreatorKey) -> Result<CreationObservation<'_>, IngressRefusal> {
        let reply = self.reply(key)?;
        Ok(match reply.status.load(Ordering::Acquire) {
            RETURNED => reply.product.get().map_or(
                CreationObservation::Unknown, CreationObservation::Returned,
            ),
            JOIN_FAILED => CreationObservation::JoinFailed,
            NOT_STARTED => CreationObservation::CancelledBeforeEffects,
            UNKNOWN => CreationObservation::Unknown,
            _ => CreationObservation::Pending,
        })
    }

    pub(crate) async fn wait(&self, key: CreatorKey) -> Result<CreationObservation<'_>, IngressRefusal> {
        let reply = self.reply(key)?;
        loop {
            // Register before observing, so completion between the check and
            // await cannot be lost. Cancelling this waiter removes no owner root.
            let changed = reply.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            match self.observe(key)? {
                CreationObservation::Pending => changed.await,
                other => return Ok(other),
            }
        }
    }

    /// Scope-close request, NOT a join and never a callback awaiting outer main.
    pub(crate) fn request_close(&self) -> Result<(), IngressRefusal> {
        let original = self.shared.close_request.get_or_init(|| {
            let started_at = Instant::now();
            let result = (|| {
                let mut admission = self.shared.admission.lock().map_err(|_| IngressRefusal::Poisoned)?;
                admission.closed = true;
                Ok(())
            })();
            ScopeClose { started_at, result }
        });
        // Retain the first actual result BEFORE waking main/another observer.
        self.shared.owner_changed.notify_one();
        original.result
    }

    pub(crate) fn close_result(&self) -> Option<&Result<(), IngressRefusal>> {
        self.shared.close_request.get().map(|original| &original.result)
    }

    pub(crate) fn close_started_at(&self) -> Option<&Instant> {
        self.shared.close_request.get().map(|original| &original.started_at)
    }

    /// HTTP admission check, NOT a substitute for the later actor/Host command
    /// linearization and in-flight command accounting in the joint integration.
    pub(crate) fn check_admission(&self) -> Result<(), IngressRefusal> {
        let admission = self.shared.admission.lock().map_err(|_| IngressRefusal::Poisoned)?;
        if admission.closed { Err(IngressRefusal::Closed) } else { Ok(()) }
    }

    /// Main selects this borrowed notification alongside serving and polls the
    /// original owner. No spawned helper drives the owner behind the main frame.
    pub(crate) async fn owner_changed(&self) { self.shared.owner_changed.notified().await; }
}

/// Not Clone. Neither this array nor its joins move into an observer/App clone.
/// Parent storage exists before any task/factory/campaign/writer effects.
pub(crate) struct CreationOwner<const N: usize, const R: usize> {
    shared: Arc<Shared<N>>,
    runtime: Handle,
    directory: Arc<PathBuf>,
    data: Arc<PathBuf>,
    tasks: [FactoryJoin; N],
    writers: [WriterJoin<Result<(), Error>, R>; N],
    stops: [Option<OwnedStop>; N],
    started: [bool; N],
    guarded_started: [bool; N],
    terminal_published: [bool; N],
    shutdown_deadline: Option<Instant>,
    finalization: finalization::FinalizationRoots<N>,
}

impl<const N: usize, const R: usize> CreationOwner<N, R> {
    /// Caller must already own the durable OPEN lease and actual full admission
    /// allowance before this constructor; no live callsite or fit exists yet.
    pub(crate) fn new_prepaid(
        runtime: Handle,
        incarnation: uuid::Uuid,
        directory: PathBuf,
        data: PathBuf,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                incarnation,
                admission: Mutex::new(Admission {
                    inputs: std::array::from_fn(|_| None), used: 0, closed: false,
                }),
                replies: std::array::from_fn(|_| ReplyRoot::new()),
                owner_changed: Notify::new(),
                close_request: OnceLock::new(),
            }),
            runtime, directory: Arc::new(directory), data: Arc::new(data),
            tasks: std::array::from_fn(|_| RetainedJoin::before_shutdown()),
            writers: std::array::from_fn(|_| WriterJoin::new_prepaid()),
            stops: std::array::from_fn(|_| None),
            started: [false; N], guarded_started: [false; N], terminal_published: [false; N], shutdown_deadline: None,
            finalization: finalization::FinalizationRoots::new(),
        }
    }

    pub(crate) fn observer(&self) -> CreationObserver<N> {
        CreationObserver { shared: Arc::clone(&self.shared) }
    }

    pub(crate) fn storage_directory(&self) -> &std::path::Path { self.directory.as_path() }

    pub(crate) fn close_result(&self) -> Option<&Result<(), IngressRefusal>> {
        self.shared.close_request.get().map(|original| &original.result)
    }

    pub(crate) fn close_started_at(&self) -> Option<&Instant> {
        self.shared.close_request.get().map(|original| &original.started_at)
    }

    fn owned_index(&self, key: CreatorKey) -> Result<usize, IngressRefusal> {
        if key.incarnation != self.shared.incarnation { return Err(IngressRefusal::ForeignAttempt); }
        let used = self.shared.admission.lock().map_err(|_| IngressRefusal::Poisoned)?.used;
        if key.index >= used { return Err(IngressRefusal::MissingAttempt); }
        Ok(key.index)
    }

    /// Reserve original recovery request and sole path in one admission turn.
    /// This queues ONLY; no target file, creator or writer effect occurs here.
    pub(crate) fn reserve_guarded_recovery(&mut self, original: PathBuf)
        -> Result<CreatorKey, (IngressRefusal, PathBuf)>
    {
        let key = {
            let mut admission = match self.shared.admission.lock() {
                Ok(admission) => admission,
                Err(_) => return Err((IngressRefusal::Poisoned, original)),
            };
            if admission.closed { return Err((IngressRefusal::Closed, original)); }
            if admission.used == N { return Err((IngressRefusal::Full, original)); }
            let index = admission.used;
            if let Err(original) = self.writers[index].record().retain_attempt_path(original) {
                return Err((IngressRefusal::MissingAttempt, original));
            }
            admission.inputs[index] = Some(CreationInput::GuardedRecovery);
            admission.used += 1;
            CreatorKey { incarnation: self.shared.incarnation, index }
        };
        self.shared.owner_changed.notify_one(); // Both original roots already held.
        Ok(key)
    }

    pub(crate) fn custody_record(&self, key: CreatorKey)
        -> Result<&Arc<crate::owned_writer::WriterRecord<R>>, IngressRefusal>
    { Ok(self.writers[self.owned_index(key)?].record()) }

    pub(crate) fn require_pairing_window(&self, key: CreatorKey) -> Result<(), IngressRefusal> {
        let index = self.owned_index(key)?;
        if self.shared.close_request.get().is_some() { return Err(IngressRefusal::Closed); }
        self.writers[index].require_pairing_window().map_err(|_| IngressRefusal::NoStopRoot)
    }

    /// Original owner borrows the actual prepared binding before member/GO
    /// decisions. A row alone cannot stand in for a retained creator completion.
    /// UnsuccessfulProduct is a refusal, NEVER proof that no writer was created.
    pub(crate) fn prepared_binding(&self, key: CreatorKey) -> Result<WriterPreparedIdentity<'_, R>, PreparedAccessRefusal> {
        let index = self.owned_index(key).map_err(PreparedAccessRefusal::Ingress)?;
        match self.tasks[index].observe() {
            JoinObservation::Pending => return Err(PreparedAccessRefusal::PendingCreator),
            JoinObservation::Unknown | JoinObservation::Returned { was_unknown: true, .. } => return Err(PreparedAccessRefusal::UnknownCreator),
            JoinObservation::Returned { original: Err(_), was_unknown: false } => return Err(PreparedAccessRefusal::CreatorJoinFailed),
            JoinObservation::Returned { original: Ok(CreatorExit::CancelledBeforeEffects), was_unknown: false } => return Err(PreparedAccessRefusal::UnsuccessfulProduct),
            JoinObservation::Returned { original: Ok(CreatorExit::Produced), was_unknown: false } => {}
        }
        if !matches!(self.shared.replies[index].product.get(), Some(Ok(_))) {
            return Err(PreparedAccessRefusal::UnsuccessfulProduct);
        }
        self.writers[index].prepared_identity().map_err(PreparedAccessRefusal::Stored)
    }

    /// Raw stop reply/queue cause still lives in the unique parent, not in the
    /// observer's future or a copied HTTP result. NoStopRoot is NOT no-writer
    /// evidence: failed/pending/unwound creators can still own a standard handle.
    pub(crate) fn stop_observation(&self, key: CreatorKey) -> Result<StopObservation<'_>, IngressRefusal> {
        self.stops[self.owned_index(key)?].as_ref()
            .map(OwnedStop::observe).ok_or(IngressRefusal::NoStopRoot)
    }

    /// Owner final-writer phase ONLY, after actual Host joins. App's borrowed
    /// shutdown callback must not call this while a seat still needs its ACK.
    /// The exact Host report/lease wiring remains pending, not a boolean proof.
    /// This joins no standard thread; actual WriterJoin + terminal rows must
    /// still settle separately even after an original successful shutdown ACK.
    pub(crate) async fn settle_writer_stop(
        &mut self,
        key: CreatorKey,
    ) -> Result<StopObservation<'_>, IngressRefusal> {
        let index = self.owned_index(key)?;
        let deadline = self.shutdown_deadline.ok_or(IngressRefusal::ShutdownNotRequested)?;
        let root = self.stops[index].as_mut().ok_or(IngressRefusal::NoStopRoot)?;
        Ok(root.settle_until(deadline).await)
    }

    /// Only outer main calls this. No await/foreign wake under admission guard.
    /// A concrete factory runs only after its actual task handle was installed.
    pub(crate) fn dispatch_pending(&mut self) -> Result<(), IngressRefusal> {
        let used = self.shared.admission.lock().map_err(|_| IngressRefusal::Poisoned)?.used;
        for index in 0..used {
            if self.started[index] || self.terminal_published[index] { continue; }
            let input = {
                let mut admission = self.shared.admission.lock().map_err(|_| IngressRefusal::Poisoned)?;
                if admission.closed { None } else { admission.inputs[index].take() }
            };
            let Some(input) = input else {
                // The unconsumed original request stays in its census slot.
                self.publish_status(index, NOT_STARTED);
                continue;
            };
            let shared = Arc::clone(&self.shared);
            let directory = Arc::clone(&self.directory);
            let data = Arc::clone(&self.data);
            let writer = self.writers[index].creator_publication();
            let vacant = self.tasks[index].vacant_blocking().expect("unique original creator slot");
            vacant.dispatch(&self.runtime, move || {
                let reply = &shared.replies[index];
                if !reply.gate.wait() { return CreatorExit::CancelledBeforeEffects; }
                // There is exactly one trusted producer. get_or_init keeps its
                // COMPLETE raw result in the parent's original slot before this
                // worker returns; observers still see Pending until actual join.
                reply.product.get_or_init(|| {
                    let claimed = writer.claim().expect("one reserved writer per creator");
                    match input {
                        CreationInput::Fresh(request) => crate::campaigns::create_owned(
                            directory.as_path(), data.as_path(), request, claimed,
                        ),
                        CreationInput::Recover(path) => crate::campaigns::recover_owned(
                            &path, data.as_path(), claimed,
                        ),
                        // Never route an admitted guarded request into legacy
                        // readonly/writer opens. Real guarded producer is owed.
                        CreationInput::GuardedRecovery => Err(Error::NotRunning.into()),
                    }
                });
                CreatorExit::Produced
            });
            self.started[index] = true;
            let open = {
                let admission = self.shared.admission.lock().map_err(|_| IngressRefusal::Poisoned)?;
                let open = !admission.closed;
                self.shared.replies[index].gate.decide(open);
                open
            };
            if !open { let _ = self.writers[index].cancel_before_release(); }
            self.shared.replies[index].gate.wake(); // Actual handle already held.
        }
        Ok(())
    }

    fn publish_status(&mut self, index: usize, status: u8) {
        // Construct the SAME actor queue/reply root before any later writer GO
        // or HTTP observation. Creation failure/uncertainty is not no-writer
        // evidence; the separate writer root remains held in every outcome.
        if status == RETURNED && self.stops[index].is_none() {
            if let Some(Ok(handle)) = self.shared.replies[index].product.get() {
                self.stops[index] = Some(OwnedStop::new_prepaid(handle));
            }
        }
        self.terminal_published[index] = true;
        self.shared.replies[index].status.store(status, Ordering::Release);
        self.shared.replies[index].changed.notify_waiters();
    }

    /// Main's original future polls every actual task. No arbitrary factory, raw
    /// output formatting or extra task drives completion. The underlying root
    /// holds JoinError/Ok BEFORE notification, including unwind/cancel failures.
    pub(crate) fn poll_creators(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), IngressRefusal>> {
        if let Err(refusal) = self.dispatch_pending() { return Poll::Ready(Err(refusal)); }
        self.poll_started_creators(cx, false)
    }

    fn poll_started_creators(&mut self, cx: &mut Context<'_>, guarded_only: bool) -> Poll<Result<(), IngressRefusal>> {
        let mut pending = false;
        for index in 0..N {
            if guarded_only && !self.guarded_started[index] { continue; }
            if !self.started[index] || self.terminal_published[index] { continue; }
            match self.tasks[index].poll_completion(cx) {
                Poll::Pending => pending = true,
                Poll::Ready(Err(_)) => return Poll::Ready(Err(IngressRefusal::MissingAttempt)),
                Poll::Ready(Ok(())) => {
                    let status = match self.tasks[index].observe() {
                        JoinObservation::Returned { original: Ok(CreatorExit::Produced), was_unknown: false } => RETURNED,
                        JoinObservation::Returned { original: Ok(CreatorExit::CancelledBeforeEffects), was_unknown: false } => NOT_STARTED,
                        JoinObservation::Returned { original: Err(_), was_unknown: false } => JOIN_FAILED,
                        _ => UNKNOWN,
                    };
                    self.publish_status(index, status);
                }
            }
        }
        if pending { Poll::Pending } else { Poll::Ready(Ok(())) }
    }

    /// Actual tasks remain here on timeout/cancel. Repeated close never restarts
    /// the original deadline. This phase does NOT stop released writers while
    /// Host seats can still need their ACK lane; final writer stop/join is later.
    pub(crate) fn close(&mut self, requested: Instant) -> Result<Instant, IngressRefusal> {
        self.observer().request_close()?;
        let deadline = self.shutdown_deadline.map_or(requested, |old| old.min(requested));
        self.shutdown_deadline = Some(deadline);
        for index in 0..N {
            self.tasks[index].request_deadline(deadline);
            self.shared.replies[index].gate.decide(false);
            self.shared.replies[index].custody_gate.decide(false);
            let _ = self.writers[index].cancel_before_release();
            self.shared.replies[index].gate.wake();
            self.shared.replies[index].custody_gate.wake();
            #[cfg(test)] {
                if let Some(gate) = self.shared.replies[index].test_preparation_hold.get()
                { gate.decide(false); gate.wake(); }
            }
        }
        self.dispatch_pending()?;
        Ok(deadline)
    }

    pub(crate) async fn join_creators_until(&mut self, requested: Instant) -> Result<(), IngressRefusal> {
        let deadline = self.close(requested)?;
        for index in 0..N {
            if !self.started[index] { continue; }
            let status = match self.tasks[index].join_until(deadline).await {
                Ok(JoinObservation::Returned { original: Ok(CreatorExit::Produced), was_unknown: false }) => RETURNED,
                Ok(JoinObservation::Returned { original: Ok(CreatorExit::CancelledBeforeEffects), was_unknown: false }) => NOT_STARTED,
                Ok(JoinObservation::Returned { original: Err(_), was_unknown: false }) => JOIN_FAILED,
                Ok(_) => UNKNOWN,
                Err(_) => return Err(IngressRefusal::MissingAttempt),
            };
            self.publish_status(index, status);
        }
        Ok(())
    }
}

#[path = "owned_preparation.rs"]
mod preparation;

#[path = "owned_ingress_evidence.rs"]
mod evidence;

#[path = "owned_finalization.rs"]
mod finalization;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn first_observer_close_clock_survives_later_original_owner_cleanup() {
        let mut owner = CreationOwner::<1, 512>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(60), PathBuf::new(), PathBuf::new(),
        );
        let observer = owner.observer();
        assert!(owner.close_started_at().is_none());
        let before = Instant::now();
        observer.request_close().unwrap();
        let after = Instant::now();
        let first = *observer.close_started_at().unwrap();
        assert!(before <= first && first <= after);
        let first_address = observer.close_started_at().unwrap() as *const _;
        let result_address = observer.close_result().unwrap() as *const _;
        drop(observer); // Original owner keeps the same record, not a snapshot.
        let supplied_deadline = first + Duration::from_secs(120);
        assert_eq!(owner.close(supplied_deadline).unwrap(), supplied_deadline);
        assert_eq!(owner.close(supplied_deadline + Duration::from_secs(120)).unwrap(), supplied_deadline);
        assert_eq!(owner.close_started_at().unwrap() as *const _, first_address);
        assert_eq!(owner.close_result().unwrap() as *const _, result_address);
        assert_eq!(*owner.close_started_at().unwrap(), first);
        // This control proves retained clock identity, NOT that supplied_deadline
        // is the actual Host deadline. That source integration is still closed.
        assert_eq!(owner.freeze_raw().unwrap().attempts().count(), 0);
    }

    #[tokio::test]
    async fn dropped_observer_and_queued_close_keep_original_request_without_effects() {
        let mut owner = CreationOwner::<1, 512>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(10),
            PathBuf::from("unused-campaigns"), PathBuf::from("unused-data"),
        );
        let observer = owner.observer();
        let original = PathBuf::from("never-open-this.sqlite");
        let address = original.as_os_str().as_encoded_bytes().as_ptr();
        let key = observer.submit(CreationInput::Recover(original)).unwrap();
        drop(observer);
        let deadline = Instant::now() + Duration::from_secs(120);
        assert_eq!(owner.close(deadline).unwrap(), deadline);
        assert_eq!(owner.close(deadline + Duration::from_secs(120)).unwrap(), deadline);
        let observer = owner.observer();
        assert!(matches!(observer.wait(key).await.unwrap(), CreationObservation::CancelledBeforeEffects));
        assert!(owner.tasks[0].handle_for_poll().is_err());
        assert!(owner.writers[0].can_publish());
        let admission = owner.shared.admission.lock().unwrap();
        match admission.inputs[0].as_ref().unwrap() {
            CreationInput::Recover(path) => assert_eq!(path.as_os_str().as_encoded_bytes().as_ptr(), address),
            _ => panic!("original recovery input was replaced"),
        }
        drop(admission);
        assert!(matches!(observer.submit(CreationInput::Recover(PathBuf::new())),
            Err((IngressRefusal::Closed, _))));
    }

    #[tokio::test]
    async fn concrete_recovery_failure_is_held_before_observer_completion() {
        // Real concrete recovery dispatch, with a definitely absent readonly DB.
        // No Engine fixture or injected substitute Factory is involved.
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("absent.sqlite");
        let mut owner = CreationOwner::<1, 512>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(11),
            directory.path().to_owned(), directory.path().to_owned(),
        );
        let observer = owner.observer();
        let key = observer.submit(CreationInput::Recover(missing.clone())).unwrap();
        owner.dispatch_pending().unwrap();
        // Worker return alone cannot wake/publish the observer as completed.
        assert!(matches!(observer.observe(key).unwrap(), CreationObservation::Pending));
        drop(observer);
        std::future::poll_fn(|cx| owner.poll_creators(cx)).await.unwrap();
        assert!(matches!(owner.tasks[0].observe(),
            JoinObservation::Returned { original: Ok(CreatorExit::Produced), was_unknown: false }));
        let raw_address = match owner.shared.replies[0].product.get().unwrap() {
            Err(WriterStartFailure::Setup(Error::Storage(original))) => original as *const _,
            _ => panic!("actual readonly recovery error was not retained"),
        };
        let observer = owner.observer();
        match observer.wait(key).await.unwrap() {
            CreationObservation::Returned(Err(WriterStartFailure::Setup(Error::Storage(original)))) => {
                assert_eq!(original as *const _, raw_address);
            }
            _ => panic!("observer did not borrow the canonical original failure"),
        }
        assert!(!missing.exists());
        owner.join_creators_until(Instant::now() + Duration::from_secs(120)).await.unwrap();
        assert!(matches!(owner.tasks[0].observe(),
            JoinObservation::Returned { original: Ok(CreatorExit::Produced), was_unknown: false }));
    }

    #[tokio::test]
    async fn actual_creator_unwind_retains_raw_join_error_before_neutral_reply() {
        let directory = tempfile::tempdir().unwrap();
        let mut owner = CreationOwner::<1, 512>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(12),
            directory.path().to_owned(), directory.path().to_owned(),
        );
        let observer = owner.observer();
        let key = observer.submit(CreationInput::Recover(directory.path().join("absent.sqlite"))).unwrap();
        // Deliberately violate the single-producer claim inside this fixture:
        // the actual creator panics at its own pre-effect claim, not in a new
        // wrapper task or replaced Engine/factory. Actual JoinError is the cause.
        let publication = owner.writers[0].creator_publication();
        let first_claim = publication.claim().unwrap();
        owner.dispatch_pending().unwrap();
        std::future::poll_fn(|cx| owner.poll_creators(cx)).await.unwrap();
        let address = match owner.tasks[0].observe() {
            JoinObservation::Returned { original: Err(raw), was_unknown: false } => {
                assert!(raw.is_panic());
                raw as *const _
            }
            _ => panic!("original actual task unwind was lost"),
        };
        assert!(matches!(observer.wait(key).await.unwrap(), CreationObservation::JoinFailed));
        drop(observer);
        owner.join_creators_until(Instant::now() + Duration::from_secs(120)).await.unwrap();
        match owner.tasks[0].observe() {
            JoinObservation::Returned { original: Err(raw), was_unknown: false } => assert_eq!(raw as *const _, address),
            _ => panic!("repeated cleanup replaced the actual JoinError"),
        }
        drop(first_claim);
    }
}
