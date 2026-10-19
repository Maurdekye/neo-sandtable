//! Actual join/output slots for the process owner. Sealed registration may
//! install one gated task. No detach, output take, refund or lifecycle inference.

use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;
use tokio::task::{JoinError, JoinHandle};
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TaskObservation {
    Empty,
    Returned,
    Unknown,
}

/// Holds the REAL handle, not a wrapper which transfers it into an await
/// temporary. T is the real returned output; every transitive child stays owned.
pub(super) struct RetainedTask<T> {
    handle: Option<JoinHandle<T>>,
    returned: Option<Result<T, JoinError>>,
    deadline: Option<Instant>,
    elapsed: Option<tokio::time::error::Elapsed>,
    unknown: bool,
}

/// Fixed facts and a borrow of the original whole output. Never exposes the
/// JoinHandle, a waiter future, an output take or a healthy-settlement bit.
pub(super) struct TaskReadback<'a, T> {
    pub(super) handle_retained: bool,
    pub(super) returned: Option<&'a Result<T, JoinError>>,
    pub(super) deadline: Option<Instant>,
    pub(super) elapsed: Option<&'a tokio::time::error::Elapsed>,
    pub(super) unknown: bool,
}

impl<T> RetainedTask<T> {
    pub(super) fn empty() -> Self {
        Self { handle: None, returned: None, deadline: None, elapsed: None, unknown: false }
    }

    /// Registration uses this on a fresh Host entry. Validation is BEFORE spawn;
    /// the returned actual handle moves directly into its canonical field, with
    /// no fallible hook/await between spawn returning and handle publication.
    pub(super) fn spawn_closed<F>(
        &mut self, runtime: &tokio::runtime::Handle, future: F,
    ) -> Result<(), F>
    where F: std::future::Future<Output = T> + Send + 'static, T: Send + 'static {
        if self.handle.is_some() || self.returned.is_some() || self.unknown || self.deadline.is_some() || self.elapsed.is_some() {
            return Err(future);
        }
        self.handle = Some(runtime.spawn(future));
        Ok(())
    }

    /// Caller retains the whole pending handle on refusal. This is not effect
    /// admission: the driver publication gate must remain closed until install.
    pub(super) fn install(&mut self, pending: &mut Option<JoinHandle<T>>) -> Result<(), ()> {
        if self.handle.is_some() || self.returned.is_some() || self.unknown
            || self.deadline.is_some() || self.elapsed.is_some() || pending.is_none()
        {
            return Err(());
        }
        self.handle = pending.take();
        Ok(())
    }

    /// Cancellation only releases the borrow. The handle and original pinned
    /// deadline remain here, even if the task finishes while no observer exists.
    pub(super) async fn observe(
        &mut self,
        requested: Instant,
        original: Instant,
    ) -> TaskObservation {
        if self.returned.is_some() {
            return TaskObservation::Returned;
        }
        if self.unknown || self.elapsed.is_some() {
            return TaskObservation::Unknown;
        }
        if self.handle.is_none() {
            return TaskObservation::Empty;
        }
        let candidate = requested.min(original);
        let deadline = self.deadline.map_or(candidate, |first| first.min(candidate));
        self.deadline = Some(deadline);
        let result = {
            let Some(handle) = self.handle.as_mut() else {
                self.unknown = true;
                return TaskObservation::Unknown;
            };
            tokio::time::timeout_at(deadline, handle).await
        };
        match result {
            Ok(output) => {
                // FIRST whole original Result MOVE, before any observation or
                // dropping the fully consumed handle. Never clone/format it.
                self.returned = Some(output);
                self.handle = None;
                TaskObservation::Returned
            }
            Err(error) => {
                // FIRST whole timeout error MOVE before publishing UNKNOWN.
                // The early sticky check prevents replacement or another poll.
                self.elapsed = Some(error);
                // Permanent UNKNOWN: preserve the actual handle and its output
                // in Tokio's task storage. A later observer cannot reset/retry.
                self.unknown = true;
                TaskObservation::Unknown
            }
        }
    }

    /// Borrow only. Returned includes original JoinError; it is NOT a healthy
    /// child/worker/journal settlement or an authorization to discard any root.
    pub(super) fn returned(&self) -> Option<&Result<T, JoinError>> {
        self.returned.as_ref()
    }

    pub(super) fn readback(&self) -> TaskReadback<'_, T> {
        TaskReadback { handle_retained: self.handle.is_some(), returned: self.returned.as_ref(),
            deadline: self.deadline, elapsed: self.elapsed.as_ref(), unknown: self.unknown }
    }
}

/// Private one-way publication gate for the future driver registration seam.
/// Only our driver task may await it; no foreign waiter/public API exists.
pub(crate) struct PublicationGate {
    published: AtomicBool,
    ready: Notify,
}

impl PublicationGate {
    pub(crate) fn closed() -> Self {
        Self { published: AtomicBool::new(false), ready: Notify::new() }
    }

    /// Future registration calls this ONLY AFTER the actual handle and all
    /// canonical roots are installed. It performs no child/worker admission.
    pub(crate) fn publish(&self) {
        self.published.store(true, Ordering::Release);
        self.ready.notify_waiters();
    }

    pub(crate) async fn wait(&self) {
        loop {
            let ready = self.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            if self.published.load(Ordering::Acquire) {
                return;
            }
            ready.await;
        }
    }
}

#[cfg(test)]
mod controls {
    use super::*;
    use std::future::{Future, poll_fn};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::task::Poll;
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_join_then_real_timeout_keeps_first_elapsed_and_never_retries_unknown() {
        tokio::spawn(async {
            let (release, released) = tokio::sync::oneshot::channel::<()>();
            let mut pending = Some(tokio::spawn(async move { released.await.unwrap(); 73u8 }));
            let mut slot = RetainedTask::empty(); slot.install(&mut pending).unwrap();
            let first = Instant::now() + Duration::from_secs(30);
            let original = first + Duration::from_secs(30);
            let mut observing = Box::pin(slot.observe(first, original));
            poll_fn(|cx| match observing.as_mut().poll(cx) {
                Poll::Pending => Poll::Ready(()),
                Poll::Ready(_) => panic!("producer is still behind its fixture release"),
            }).await;
            drop(observing);
            assert_eq!(slot.deadline, Some(first));
            assert!(slot.readback().elapsed.is_none() && slot.returned().is_none());

            let earlier = Instant::now() - Duration::from_secs(1);
            assert_eq!(slot.observe(earlier, original).await, TaskObservation::Unknown);
            assert!(slot.readback().elapsed.is_some());
            assert_eq!(slot.deadline, Some(earlier));
            assert!(slot.readback().unknown && slot.readback().handle_retained && slot.returned().is_none());

            release.send(()).unwrap();
            // ONLY this fixture borrows the actual retained handle to join its
            // producer before ending. Its whole output stays in this local root.
            // Production UNKNOWN never re-polls or publishes a late output.
            let late = Some(slot.handle.as_mut().unwrap().await);
            assert_eq!(*late.as_ref().unwrap().as_ref().unwrap(), 73);
            assert_eq!(slot.observe(original, original).await, TaskObservation::Unknown);
            assert_eq!(slot.deadline, Some(earlier));
            // Pinned Tokio Elapsed is zero-sized: pointer equality would not
            // establish error identity. Presence plus no re-poll of the consumed
            // fixture handle checks retention and the sticky UNKNOWN behavior.
            assert!(slot.readback().elapsed.is_some());
            assert!(slot.returned().is_none() && slot.readback().unknown && slot.readback().handle_retained);
            // Task-slot fixture only: no complete Host fit, child containment,
            // writer settlement, OS failure injection or permission for CLEAN.
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn task_readback_only_borrows_the_real_pending_handle_and_retained_whole_output() {
        tokio::spawn(async {
            let (release, released) = tokio::sync::oneshot::channel();
            let bytes = vec![31u8, 37, 41]; let pointer = bytes.as_ptr() as usize;
            let mut pending = Some(tokio::spawn(async move { released.await.unwrap(); bytes }));
            let mut slot = RetainedTask::empty(); slot.install(&mut pending).unwrap();
            {
                let view = slot.readback();
                assert!(view.handle_retained && view.returned.is_none() && view.deadline.is_none() && !view.unknown);
            }
            assert!(slot.handle.is_some() && slot.returned.is_none() && pending.is_none());
            release.send(()).unwrap(); let deadline = Instant::now() + Duration::from_secs(30);
            assert_eq!(slot.observe(deadline, deadline).await, TaskObservation::Returned);
            let raw = std::ptr::from_ref(slot.returned().unwrap()) as usize;
            {
                let view = slot.readback();
                assert!(!view.handle_retained && !view.unknown && view.deadline == Some(deadline));
                assert_eq!(std::ptr::from_ref(view.returned.unwrap()) as usize, raw);
                assert_eq!(view.returned.unwrap().as_ref().unwrap().as_ptr() as usize, pointer);
                assert_eq!(view.returned.unwrap().as_ref().unwrap().as_slice(), &[31, 37, 41]);
            }
            assert_eq!(std::ptr::from_ref(slot.returned().unwrap()) as usize, raw);
            assert_eq!(slot.returned().unwrap().as_ref().unwrap().as_ptr() as usize, pointer);
        }).await.unwrap();
    }

    // SOURCE-only controls. All custom observation runs inside an actual task
    // on the SAME multi-thread runtime as the producer. No manual/noop Waker or
    // foreign executor is introduced into the owned worker integration graph.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_observer_keeps_the_real_handle_deadline_and_whole_output() {
        tokio::spawn(async {
            let (release, released) = tokio::sync::oneshot::channel::<()>();
            let bytes = vec![7u8, 11, 13];
            let pointer = bytes.as_ptr() as usize;
            let producer = tokio::spawn(async move {
                released.await.unwrap();
                bytes
            });
            let mut pending = Some(producer);
            let mut slot = RetainedTask::empty();
            assert!(slot.install(&mut pending).is_ok());
            assert!(pending.is_none());
            let first = Instant::now() + Duration::from_secs(30);
            let original = first + Duration::from_secs(30);
            let mut observer = Box::pin(slot.observe(first, original));
            poll_fn(|cx| match observer.as_mut().poll(cx) {
                Poll::Pending => Poll::Ready(()),
                Poll::Ready(_) => panic!("producer cannot finish before release"),
            }).await;
            drop(observer);
            assert!(slot.handle.is_some());
            assert!(slot.returned().is_none());
            assert_eq!(slot.deadline, Some(first));
            release.send(()).unwrap();
            assert_eq!(
                slot.observe(original + Duration::from_secs(30), original).await,
                TaskObservation::Returned,
            );
            assert_eq!(slot.deadline, Some(first));
            assert!(slot.handle.is_none());
            let raw = slot.returned().unwrap().as_ref().unwrap();
            assert_eq!(raw.as_ptr() as usize, pointer);
            assert_eq!(raw.as_slice(), &[7, 11, 13]);
            assert_eq!(slot.observe(first, original).await, TaskObservation::Returned);
            assert_eq!(slot.returned().unwrap().as_ref().unwrap().as_ptr() as usize, pointer);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejected_second_install_preserves_both_actual_handle_owners() {
        tokio::spawn(async {
            let mut first = Some(tokio::spawn(async { 17u8 }));
            let mut second = Some(tokio::spawn(async { 19u8 }));
            let mut slot = RetainedTask::empty();
            assert!(slot.install(&mut first).is_ok());
            assert!(slot.install(&mut second).is_err());
            assert!(second.is_some());
            let deadline = Instant::now() + Duration::from_secs(30);
            assert_eq!(slot.observe(deadline, deadline).await, TaskObservation::Returned);
            assert_eq!(*slot.returned().unwrap().as_ref().unwrap(), 17);
            // Explicitly join the refused original before this control ends.
            assert_eq!(second.as_mut().unwrap().await.unwrap(), 19);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn returned_panic_stays_in_the_original_output_slot() {
        tokio::spawn(async {
            // Explicit synthetic task defect, not an ordinary stdio-worker path
            // or permission to add a production hook/catch/retry mechanism.
            let producer: JoinHandle<()> = tokio::spawn(async {
                std::panic::panic_any("synthetic owned-task failure");
            });
            let mut pending = Some(producer);
            let mut slot = RetainedTask::empty();
            assert!(slot.install(&mut pending).is_ok());
            let deadline = Instant::now() + Duration::from_secs(30);
            assert_eq!(slot.observe(deadline, deadline).await, TaskObservation::Returned);
            let error = slot.returned().unwrap().as_ref().unwrap_err();
            assert!(error.is_panic());
            let pointer = error as *const JoinError as usize;
            assert!(slot.handle.is_none());
            assert_eq!(slot.observe(deadline, deadline).await, TaskObservation::Returned);
            assert_eq!(slot.returned().unwrap().as_ref().unwrap_err() as *const JoinError as usize, pointer);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn private_publication_gate_blocks_body_until_owner_publication() {
        tokio::spawn(async {
            let gate = Arc::new(PublicationGate::closed());
            let body_calls = Arc::new(AtomicUsize::new(0));
            let task_gate = gate.clone();
            let task_calls = body_calls.clone();
            let (entered, entry) = tokio::sync::oneshot::channel();
            let producer = tokio::spawn(async move {
                entered.send(()).unwrap();
                task_gate.wait().await;
                task_calls.fetch_add(1, Ordering::SeqCst);
                [23u8, 29]
            });
            entry.await.unwrap();
            let mut pending = Some(producer);
            let mut slot = RetainedTask::empty();
            assert_eq!(body_calls.load(Ordering::SeqCst), 0);
            assert!(slot.install(&mut pending).is_ok());
            assert!(slot.handle.is_some());
            gate.publish();
            let deadline = Instant::now() + Duration::from_secs(30);
            assert_eq!(slot.observe(deadline, deadline).await, TaskObservation::Returned);
            assert_eq!(body_calls.load(Ordering::SeqCst), 1);
            assert_eq!(slot.returned().unwrap().as_ref().unwrap(), &[23, 29]);
        }).await.unwrap();
    }
}
