//! Actual std archive-thread completion, separate from returned evidence/custody.
//! No thread is launched here. ORIGINAL plan creation reserves the lead's
//! explicit stack/overhead policy margins; the producer must use a named Builder
//! with the exact stack constant. Those margins are accepted residuals, not proof.

use std::{
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{JoinHandle, Result as ThreadResult, ThreadId},
};

use crate::{failed_evidence::ArchiveError, operation::Operation};

// ONE is the smallest positive concurrency ceiling: archive diagnostics need no
// parallelism. At-cap START refuses synchronously, retaining canonical evidence;
// there is no waiter, queued request, alternate worker or operation allowance.
pub(super) const MAX_ARCHIVE_THREADS: usize = 1;
const _: () = assert!(MAX_ARCHIVE_THREADS == 1); // AtomicBool represents exactly ONE slot.

pub(super) struct ThreadCapacity {
    occupied: AtomicBool,
}

pub(super) static ARCHIVE_THREADS: ThreadCapacity = ThreadCapacity::new();

/// Kept in the ORIGINAL root BEFORE launch. Drop intentionally DOES NOT free
/// capacity: dropping a handle/root cannot establish that its thread finished.
/// A refused/uncertain launch stays owned until its explicit disposition. True
/// process exit never waits on this token; all in-process bookkeeping dies then.
#[must_use = "retain the ORIGINAL slot; dropping it is not a completion disposition"]
pub(super) struct ThreadSlot {
    capacity: &'static ThreadCapacity,
    original: Operation,
    worker: Option<ThreadId>,
    released: bool,
}

impl ThreadSlot {
    /// The producer has already checked this SAME unstarted slot and installed
    /// its launch obligation. Move the actual Builder outcome into one complete
    /// owner without another fallible boundary that could detach its handle.
    /// This conversion performs no launch, join, registration or capacity release.
    pub(super) fn retain_spawn_outcome(
        mut self,
        outcome: std::io::Result<JoinHandle<()>>,
    ) -> Result<StdArchiveWorker, RefusedStdStart> {
        match outcome {
            Ok(handle) => {
                let id = handle.thread().id();
                self.worker = Some(id);
                Ok(StdArchiveWorker {
                    slot: self,
                    id,
                    handle: Some(handle),
                    joined: None,
                })
            }
            Err(error) => Err(RefusedStdStart { slot: self, error }),
        }
    }

    pub(super) fn check_unstarted(&self, expected: &Operation) -> Result<(), ArchiveError> {
        if !self.original.same_instance(expected) || self.worker.is_some() || self.released {
            return Err(ArchiveError::Identity);
        }
        Ok(())
    }
}

/// A single ORIGINAL handle plus its separately retained actual join outcome.
/// Consuming std::JoinHandle at join does not discard the original opaque panic.
/// ThreadId comes ONLY from that handle; no caller-supplied ID is accepted.
#[must_use = "retain the actual ORIGINAL handle/outcome; process exit never waits for it"]
pub(super) struct StdArchiveWorker {
    slot: ThreadSlot,
    id: ThreadId,
    handle: Option<JoinHandle<()>>,
    joined: Option<ThreadResult<()>>,
}

/// Refused START, not a worker/join outcome. Keep the SAME original slot and
/// opaque spawn error together; ordinary drop releases NO capacity. The future
/// Builder producer must move its actual Err here, never manufacture a receipt.
/// Full error payload/library fit is still a creation-time launch prerequisite.
pub(super) struct RefusedStdStart {
    slot: ThreadSlot,
    error: std::io::Error,
}

impl RefusedStdStart {
    pub(super) fn retain(
        slot: ThreadSlot,
        expected: &Operation,
        error: std::io::Error,
    ) -> Result<Self, (ArchiveError, ThreadSlot, std::io::Error)> {
        if let Err(refused) = slot.check_unstarted(expected) {
            return Err((refused, slot, error));
        }
        Ok(Self { slot, error }) // Opaque MOVE, never clone/format/release.
    }

    pub(super) fn check_original(&self, expected: &Operation) -> Result<(), ArchiveError> {
        self.slot.check_unstarted(expected)
    }

    pub(super) fn error(&self) -> &std::io::Error {
        &self.error
    }
}

/// Embedded in the canonical root's SAME original pre-admitted metadata.
/// Only the actual std worker may wait here; the operator never waits for it.
pub(super) struct ThreadRegistration {
    original: Operation,
    state: Mutex<RegistrationState>,
    changed: Condvar,
}

struct RegistrationState {
    registered: Option<ThreadId>,
    observed: Option<ThreadId>,
    closed: bool,
}

pub(super) const fn fixed_metadata_bytes() -> usize {
    // Conservative overlap: launch slot and completed owner/result are counted
    // together. This covers inline headers only, NOT std's hidden allocations,
    // thread stack reserve, closure, TLS, allocator or OS/library work.
    std::mem::size_of::<ThreadCapacity>()
        + std::mem::size_of::<ThreadSlot>()
        + std::mem::size_of::<StdArchiveWorker>()
        + std::mem::size_of::<ThreadResult<()>>()
        + std::mem::size_of::<ThreadRegistration>()
        + std::mem::size_of::<RefusedStdStart>()
}

impl ThreadRegistration {
    pub(super) fn new(original: &Operation) -> Self {
        Self {
            original: original.clone(),
            state: Mutex::new(RegistrationState {
                registered: None,
                observed: None,
                closed: false,
            }),
            changed: Condvar::new(),
        }
    }

    /// The canonical root MUST store this worker BEFORE calling registration.
    /// The ID is read from the retained actual owner, never supplied as text or
    /// inferred from a returned Box, current task, operation ID or notification.
    pub(super) fn register(
        &self,
        expected: &Operation,
        worker: &StdArchiveWorker,
    ) -> Result<(), ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        worker.check_original(expected)?;
        let mut state = self.state.lock().map_err(|_| ArchiveError::Identity)?;
        if state.closed
            || state.registered.is_some()
            || state.observed.is_some_and(|id| id != worker.id())
        {
            return Err(ArchiveError::Identity);
        }
        state.registered = Some(worker.id());
        drop(state);
        self.changed.notify_all();
        Ok(())
    }

    /// Synchronous bridge for the dedicated std worker ONLY, not an async or
    /// operator wait. Condvar releases the metadata lock while waiting, checks
    /// its predicate after every wake, and always uses the ORIGINAL deadline.
    /// Both observe-before-retain and retain-before-observe are supported.
    pub(super) fn wait_in_worker(&self, expected: &Operation) -> Result<(), ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        let actual = std::thread::current().id();
        let mut state = self.state.lock().map_err(|_| ArchiveError::Identity)?;
        loop {
            if state.closed
                || state.observed.is_some_and(|id| id != actual)
                || state.registered.is_some_and(|id| id != actual)
            {
                return Err(ArchiveError::Identity);
            }
            // Test the original allowance even when registration was already
            // present; wake/retention cannot extend effect admission.
            if let Err(error) = self.original.allowance().check() {
                state.closed = true;
                drop(state);
                self.changed.notify_all();
                return Err(error.into());
            }
            state.observed = Some(actual);
            if state.registered == Some(actual) {
                return Ok(());
            }
            let remaining = self
                .original
                .allowance()
                .deadline()
                .saturating_duration_since(tokio::time::Instant::now());
            // Registration mutates/checks the SAME mutex predicate. No lost
            // wake between inspection and atomic unlock/wait; no timer reset.
            let (next, _) = self
                .changed
                .wait_timeout(state, remaining)
                .map_err(|_| ArchiveError::Identity)?;
            state = next;
        }
    }

    pub(super) fn validate_current(&self, expected: &Operation) -> Result<(), ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        self.original.allowance().check()?;
        let actual = std::thread::current().id();
        let state = self.state.lock().map_err(|_| ArchiveError::Identity)?;
        if state.closed || state.registered != Some(actual) || state.observed != Some(actual) {
            return Err(ArchiveError::Identity);
        }
        Ok(())
    }

    /// Historical exact association fact only, including after admission closes.
    /// This does not claim callback success, actual join, file custody or health.
    pub(super) fn associated(&self, expected: &Operation) -> Result<bool, ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        let state = self.state.lock().map_err(|_| ArchiveError::Identity)?;
        Ok(state.registered.is_some() && state.registered == state.observed)
    }

    /// Operator interruption never waits for registration or joins a thread.
    /// This closes admission only, without taking a Box, handle or outcome.
    pub(super) fn close(&self, expected: &Operation) -> Result<(), ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        let mut state = self.state.lock().map_err(|_| ArchiveError::Identity)?;
        state.closed = true;
        drop(state);
        self.changed.notify_all();
        Ok(())
    }
}

impl ThreadCapacity {
    const fn new() -> Self {
        Self {
            occupied: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    pub(super) const fn isolated_test_counter() -> Self {
        Self::new() // Inert controls only; no second production pool/lane.
    }

    /// ORIGINAL plan creation includes ARCHIVE_WORKER_STACK_BYTES plus
    /// ARCHIVE_WORKER_OVERHEAD_BYTES under the unchanged cumulative cap. The
    /// concrete producer must use that exact named Builder policy, not another
    /// stack or a late reserve. No claim here proves std/OS internals or joins.
    /// Sticky capture overflow is not a new allowance and does not erase that
    /// pre-admitted archive reserve. Check the SAME absolute deadline instead.
    pub(super) fn try_claim(
        &'static self,
        original: &Operation,
        expected: &Operation,
    ) -> Result<ThreadSlot, ArchiveError> {
        if !original.same_instance(expected) || original.archive_plan().is_none() {
            return Err(ArchiveError::Identity);
        }
        original.allowance().check()?;
        self.occupied
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| ArchiveError::Fit)?;
        Ok(ThreadSlot {
            capacity: self,
            original: original.clone(), // Existing Arc headers only, no body copy.
            worker: None,
            released: false,
        })
    }
}

impl StdArchiveWorker {
    pub(super) fn check_original(&self, expected: &Operation) -> Result<(), ArchiveError> {
        if !self.slot.original.same_instance(expected) || self.slot.worker != Some(self.id) {
            return Err(ArchiveError::Identity);
        }
        Ok(())
    }
    /// Synchronous handle retention. Refusal returns BOTH original owners without
    /// join, detach, abort, formatting or replacement. The producer still must
    /// retain them in its installed root, including any launch failure outcome.
    /// This is handle identity ONLY. The actual checkout must independently
    /// observe the SAME current std ThreadId before any archive callback effect.
    pub(super) fn retain(
        mut slot: ThreadSlot,
        expected: &Operation,
        handle: JoinHandle<()>,
    ) -> Result<Self, (ArchiveError, ThreadSlot, JoinHandle<()>)> {
        if !slot.original.same_instance(expected) || slot.worker.is_some() || slot.released {
            return Err((ArchiveError::Identity, slot, handle));
        }
        let id = handle.thread().id();
        slot.worker = Some(id);
        Ok(Self {
            slot,
            id,
            handle: Some(handle),
            joined: None,
        })
    }

    pub(super) fn id(&self) -> ThreadId {
        self.id
    }

    /// Nonblocking observation on the async/operator path. Never join a running
    /// thread, never wait for a registration/channel/future here. Once the actual
    /// stored handle reports finished, join it and MOVE the actual outcome before
    /// releasing only this private thread-capacity token. Even join Err means
    /// only that the original thread finished; evidence/custody/healthy stop,
    /// command credit, prompt/actor/journal ACKs and Operation liability remain.
    pub(super) fn poll_join(&mut self, expected: &Operation) -> Result<bool, ArchiveError> {
        if !self.slot.original.same_instance(expected) || self.slot.worker != Some(self.id) {
            return Err(ArchiveError::Identity);
        }
        if self.joined.is_some() {
            return Ok(true); // SAME retained result; never join twice.
        }
        let handle = self.handle.as_ref().ok_or(ArchiveError::Identity)?;
        if handle.thread().id() != self.id {
            return Err(ArchiveError::Identity);
        }
        if !handle.is_finished() {
            return Ok(false); // Retain handle/slot unchanged; no async wait needed.
        }
        let handle = self.handle.take().ok_or(ArchiveError::Identity)?;
        self.joined = Some(handle.join()); // Actual original opaque Result MOVE.
        self.slot.released = true;
        self.slot.capacity.occupied.store(false, Ordering::Release);
        Ok(true)
    }

    pub(super) fn joined(
        &self,
        expected: &Operation,
    ) -> Result<Option<&ThreadResult<()>>, ArchiveError> {
        if !self.slot.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        Ok(self.joined.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operation() -> Operation {
        crate::failed_evidence::tests::verification_operation()
    }

    #[test]
    fn closed_registration_and_taskless_validation_never_infer_worker_pairing() {
        let original = operation();
        let foreign = operation();
        let before = original.captured_bytes();
        let registration = ThreadRegistration::new(&original);
        assert!(registration.validate_current(&original).is_err());
        assert!(registration.close(&foreign).is_err());
        assert!(!registration.state.lock().unwrap().closed);
        registration.close(&original).unwrap();
        // Already closed: synchronous refusal, no sleep/thread/handle invented.
        assert!(registration.wait_in_worker(&original).is_err());
        let state = registration.state.lock().unwrap();
        assert!(state.closed && state.registered.is_none() && state.observed.is_none());
        assert_eq!(original.captured_bytes(), before);
    }

    #[test]
    fn cap_is_one_no_queue_and_dropping_unjoined_slot_never_frees_it() {
        static CAPACITY: ThreadCapacity = ThreadCapacity::new();
        assert_eq!(MAX_ARCHIVE_THREADS, 1);
        let original = operation();
        let other = operation();
        let before = original.captured_bytes();
        original.overflow(); // Uses ORIGINAL creation reserve, never charges anew.
        let slot = CAPACITY.try_claim(&original, &original).unwrap();
        assert!(slot.original.same_instance(&original));
        assert!(slot.worker.is_none() && !slot.released);
        assert!(matches!(
            CAPACITY.try_claim(&other, &other),
            Err(ArchiveError::Fit)
        ));
        drop(slot); // No actual launch/join: MUST NOT manufacture free capacity.
        assert!(matches!(
            CAPACITY.try_claim(&original, &original),
            Err(ArchiveError::Fit)
        ));
        assert_eq!(original.captured_bytes(), before);
        assert!(original.borrow_archive_chunk(&original).is_ok());
        // This isolated inert counter is not the production ARCHIVE_THREADS.
        // No thread, child, file, actual join, custody or healthy ACK is created.
    }

    #[test]
    fn foreign_instance_refuses_before_claim_without_meter_or_owner_change() {
        static CAPACITY: ThreadCapacity = ThreadCapacity::new();
        let original = operation();
        let foreign = operation();
        let before = original.captured_bytes();
        assert!(matches!(
            CAPACITY.try_claim(&original, &foreign),
            Err(ArchiveError::Identity)
        ));
        assert!(!CAPACITY.occupied.load(Ordering::Acquire));
        let slot = CAPACITY.try_claim(&original, &original).unwrap();
        assert!(slot.original.same_instance(&original));
        assert!(!slot.original.same_instance(&foreign));
        assert_eq!(original.captured_bytes(), before);
        // Inline storage check is NOT proof of hidden thread/stack/library fit.
        assert!(fixed_metadata_bytes() < crate::failed_evidence::FIXED_ARCHIVE_WORKSPACE);
    }
}
