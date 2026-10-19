//! Private held-source primitives for the process owner's writer cleanup.
//!
//! The outer owner keeps this value; observers borrow it. There is no handle
//! extraction, output replacement, callback, mutex, notification or destructor
//! join here. Every body and transitive input belongs in the complete original
//! F/G inventory before construction. These inline primitives allocate nothing
//! themselves and are NOT a complete fit producer, registry or activation path.

use tokio::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PublicationRefusal {
    AlreadyPublished,
    MissingHandle,
    AlreadyReturned,
}

/// A failed second publication returns the SAME rejected original to its caller.
/// The first publication never moves out through an observation or a refusal.
pub(crate) struct Original<T> {
    value: Option<T>,
}

impl<T> Original<T> {
    pub(crate) const fn empty() -> Self {
        Self { value: None }
    }

    pub(crate) fn publish(&mut self, original: T) -> Result<(), T> {
        if self.value.is_some() {
            return Err(original);
        }
        self.value = Some(original);
        Ok(())
    }

    pub(crate) fn observe(&self) -> Option<&T> {
        self.value.as_ref()
    }

    fn borrow_mut(&mut self) -> Option<&mut T> {
        self.value.as_mut()
    }
}

pub(crate) enum JoinObservation<'a, O> {
    Pending,
    Unknown,
    Returned {
        original: &'a O,
        /// Actual late output does not refund or erase earlier uncertainty.
        was_unknown: bool,
    },
}

/// The SAME handle remains in the outer owner while an await borrows it.
/// Actual completion is published into `output` BEFORE any report observes it.
/// Unknown retains both roots and the original cumulative charge; there is no
/// settlement/refund/disposal operation on this primitive.
pub(crate) struct RetainedJoin<H, O> {
    handle: Original<H>,
    output: Original<O>,
    deadline: Option<Instant>,
    was_unknown: bool,
}

impl<H, O> RetainedJoin<H, O> {
    pub(crate) fn before_publication(original_deadline: Instant) -> Self {
        Self {
            handle: Original::empty(),
            output: Original::empty(),
            deadline: Some(original_deadline),
            was_unknown: false,
        }
    }

    /// Admission can precede shutdown by an arbitrary campaign lifetime. The
    /// first shutdown request fixes the deadline; observation never clears it.
    pub(crate) fn before_shutdown() -> Self {
        Self {
            handle: Original::empty(),
            output: Original::empty(),
            deadline: None,
            was_unknown: false,
        }
    }

    pub(crate) fn publish_handle(
        &mut self,
        original: H,
    ) -> Result<(), (PublicationRefusal, H)> {
        self.handle
            .publish(original)
            .map_err(|original| (PublicationRefusal::AlreadyPublished, original))
    }

    /// Exclusive outer-owner access only. No handle is taken into a timeout
    /// temporary; cancelling the borrowing future leaves the stored handle.
    pub(crate) fn handle_for_poll(&mut self) -> Result<&mut H, PublicationRefusal> {
        if self.output.observe().is_some() {
            return Err(PublicationRefusal::AlreadyReturned);
        }
        self.handle
            .borrow_mut()
            .ok_or(PublicationRefusal::MissingHandle)
    }

    /// Caller supplies the actual await result, whole and without normalization.
    /// This moves it into the original terminal slot before observation/formatting.
    pub(crate) fn publish_returned(
        &mut self,
        original: O,
    ) -> Result<(), (PublicationRefusal, O)> {
        if self.handle.observe().is_none() {
            return Err((PublicationRefusal::MissingHandle, original));
        }
        self.output
            .publish(original)
            .map_err(|original| (PublicationRefusal::AlreadyReturned, original))
    }

    pub(crate) fn request_deadline(&mut self, requested: Instant) -> Instant {
        let deadline = self.deadline.map_or(requested, |original| original.min(requested));
        self.deadline = Some(deadline);
        deadline
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub(crate) fn mark_unknown(&mut self) {
        self.was_unknown = true;
    }

    pub(crate) fn observe(&self) -> JoinObservation<'_, O> {
        match self.output.observe() {
            Some(original) => JoinObservation::Returned {
                original,
                was_unknown: self.was_unknown,
            },
            None if self.was_unknown => JoinObservation::Unknown,
            None => JoinObservation::Pending,
        }
    }

    /// Read actual storage, not Pending-as-proof-of-absence. Used together with
    /// the closed concrete producer census; this boolean alone is no join proof.
    pub(crate) fn is_unpublished(&self) -> bool {
        self.handle.observe().is_none() && self.output.observe().is_none() && !self.was_unknown
    }
}

impl<T> RetainedJoin<tokio::task::JoinHandle<T>, Result<T, tokio::task::JoinError>> {
    /// Original outer-main polling, without starting a shutdown deadline during
    /// normal service. The handle stays in this root and the COMPLETE actual
    /// return moves here before the caller may publish a completion notification.
    /// A cancelled poll/serve future cannot take either root away.
    pub(crate) fn poll_completion(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), PublicationRefusal>> {
        use std::{future::Future, pin::Pin, task::Poll};
        if self.output.observe().is_some() { return Poll::Ready(Ok(())); }
        let handle = match self.handle_for_poll() {
            Ok(handle) => handle,
            Err(refusal) => return Poll::Ready(Err(refusal)),
        };
        match Pin::new(handle).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(original) => {
                self.output.value = Some(original);
                Poll::Ready(Ok(()))
            }
        }
    }

    /// Obtain exclusive vacant storage BEFORE taking a standard writer handle
    /// into a blocking closure. A refused slot has consumed no work or capture.
    pub(crate) fn vacant_blocking(&mut self) -> Option<VacantBlocking<'_, T>> {
        if self.handle.observe().is_some() || self.output.observe().is_some() { return None; }
        Some(VacantBlocking { root: self })
    }

    /// Only the process cleanup owner drives this await. Dropping this future
    /// cancels the observation, not the stored task. Both ordinary task output
    /// and a raw Tokio JoinError move into the canonical slot before return.
    /// Task/future allocation and all transitive output storage must already be
    /// covered by the original owner budget; this method does not admit a task.
    pub(crate) async fn join_until(
        &mut self,
        requested: Instant,
    ) -> Result<JoinObservation<'_, Result<T, tokio::task::JoinError>>, PublicationRefusal> {
        let deadline = self.request_deadline(requested);
        if self.output.observe().is_none() {
            let completion = tokio::time::timeout_at(deadline, self.handle_for_poll()?).await;
            match completion {
                Ok(original) => {
                    // Exclusive borrow + the empty-slot check above exclude a
                    // competing publisher. Assignment cannot invoke user code.
                    self.output.value = Some(original);
                }
                Err(_) => self.mark_unknown(),
            }
        }
        Ok(self.observe())
    }
}

pub(crate) struct VacantBlocking<'a, T> {
    root: &'a mut RetainedJoin<tokio::task::JoinHandle<T>, Result<T, tokio::task::JoinError>>,
}

impl<T: Send + 'static> VacantBlocking<'_, T> {
    /// All task/capture/result backing is prepaid by the original fit BEFORE
    /// this call. No runtime discovery or fresh cleanup admission. The private
    /// guard proves this root vacant and exclusively borrowed across dispatch;
    /// no duplicate-publication error path can abandon the closure's captures.
    pub(crate) fn dispatch<F>(self, runtime: &tokio::runtime::Handle, work: F)
    where F: FnOnce() -> T + Send + 'static,
    {
        let original = runtime.spawn_blocking(work);
        self.root.handle.value = Some(original);
    }
}

// SOURCE ONLY, UNCOMPILED/UNRUN. These direct inline-value fixtures do not
// create an Operation, runtime task, worker, child or complete fit producer.
// Actual retained Tokio/std join and App cancellation controls follow integration.
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn rejected_publication_preserves_both_original_allocations() {
        let first = Box::new([1_u8, 2, 3]);
        let first_address = first.as_ptr();
        let rejected = Box::new([4_u8, 5, 6]);
        let rejected_address = rejected.as_ptr();
        let mut root = Original::empty();
        assert!(root.publish(first).is_ok());
        let rejected = root.publish(rejected).unwrap_err();
        assert_eq!(rejected.as_ptr(), rejected_address);
        assert_eq!(root.observe().unwrap().as_ptr(), first_address);
        assert_eq!(**root.observe().unwrap(), [1, 2, 3]);
        assert_eq!(*rejected, [4, 5, 6]);
    }

    #[test]
    fn borrowing_and_unknown_do_not_remove_the_original_handle() {
        let original_deadline = Instant::now() + Duration::from_secs(120);
        let mut root = RetainedJoin::<Box<[u8; 3]>, Result<(), (u16, u8)>>::before_publication(
            original_deadline,
        );
        let handle = Box::new([7, 8, 9]);
        let address = handle.as_ptr();
        assert!(root.publish_handle(handle).is_ok());
        {
            let borrowed = root.handle_for_poll().unwrap();
            assert_eq!(borrowed.as_ptr(), address);
        }
        root.mark_unknown();
        assert!(matches!(root.observe(), JoinObservation::Unknown));
        assert_eq!(root.handle_for_poll().unwrap().as_ptr(), address);
        assert_eq!(root.deadline(), Some(original_deadline));
    }

    #[test]
    fn raw_first_return_remains_unchanged_after_late_duplicate() {
        let mut root = RetainedJoin::<(), Result<(), Box<[u8; 4]>>>::before_publication(Instant::now());
        assert!(root.publish_handle(()).is_ok());
        let raw = Box::new([0, 255, 9, 0]);
        let raw_address = raw.as_ptr();
        root.mark_unknown();
        assert!(root.publish_returned(Err(raw)).is_ok());
        let late = Box::new([4, 3, 2, 1]);
        let late_address = late.as_ptr();
        let (refusal, returned_late) = root.publish_returned(Err(late)).unwrap_err();
        assert_eq!(refusal, PublicationRefusal::AlreadyReturned);
        assert_eq!(returned_late.unwrap_err().as_ptr(), late_address);
        match root.observe() {
            JoinObservation::Returned { original: Err(raw), was_unknown } => {
                assert!(was_unknown);
                assert_eq!(raw.as_ptr(), raw_address);
                assert_eq!(**raw, [0, 255, 9, 0]);
            }
            _ => panic!("original raw return is unavailable"),
        }
        assert!(matches!(root.handle_for_poll(), Err(PublicationRefusal::AlreadyReturned)));
    }

    #[test]
    fn repeat_observation_cannot_restart_original_deadline() {
        let original = Instant::now() + Duration::from_secs(120);
        let mut root = RetainedJoin::<(), ()>::before_publication(original);
        assert_eq!(root.request_deadline(original + Duration::from_secs(120)), original);
        let earlier = original - Duration::from_secs(60);
        assert_eq!(root.request_deadline(earlier), earlier);
        root.mark_unknown();
        assert_eq!(root.request_deadline(original), earlier);
        assert_eq!(root.deadline(), Some(earlier));
        assert!(matches!(root.observe(), JoinObservation::Unknown));
    }

    // Integration controls ADD to the inline primitive fixtures above.
    // SOURCE AUTHORED ONLY: no runtime, task or worker has been invoked here.
    #[tokio::test]
    async fn cancelled_join_observer_keeps_actual_task_and_raw_output() {
        use std::future::Future;
        let deadline = Instant::now() + Duration::from_secs(120);
        let (release, wait) = tokio::sync::oneshot::channel();
        let output = Box::new([0_u8, 255, 7, 0]);
        let output_address = output.as_ptr() as usize;
        let handle = tokio::spawn(async move {
            wait.await.unwrap();
            Err::<(), _>(output)
        });
        let task_id = handle.id();
        let mut root = RetainedJoin::before_publication(deadline);
        assert!(root.publish_handle(handle).is_ok());
        {
            let mut observer = std::pin::pin!(root.join_until(deadline));
            std::future::poll_fn(|cx| {
                assert!(observer.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            }).await;
        }
        assert_eq!(root.handle_for_poll().unwrap().id(), task_id);
        release.send(()).unwrap();
        match root.join_until(deadline).await.unwrap() {
            JoinObservation::Returned { original: Ok(Err(raw)), was_unknown } => {
                assert!(!was_unknown);
                assert_eq!(raw.as_ptr() as usize, output_address);
                assert_eq!(**raw, [0, 255, 7, 0]);
            }
            _ => panic!("actual task result was not retained"),
        }
        assert!(matches!(root.handle_for_poll(), Err(PublicationRefusal::AlreadyReturned)));
    }

    #[tokio::test]
    async fn actual_task_panic_remains_raw_join_error_across_observations() {
        let deadline = Instant::now() + Duration::from_secs(120);
        let handle = tokio::spawn(async { panic!("retained task panic"); });
        let task_id = handle.id();
        let mut root = RetainedJoin::before_publication(deadline);
        assert!(root.publish_handle(handle).is_ok());
        match root.join_until(deadline).await.unwrap() {
            JoinObservation::Returned { original: Err(raw), was_unknown } => {
                assert!(!was_unknown);
                assert!(raw.is_panic());
                assert_eq!(raw.id(), task_id);
            }
            _ => panic!("actual panic cause was not retained"),
        }
        match root.observe() {
            JoinObservation::Returned { original: Err(raw), .. } => {
                assert!(raw.is_panic());
                assert_eq!(raw.id(), task_id);
            }
            _ => panic!("observation consumed the panic cause"),
        }
    }
}
