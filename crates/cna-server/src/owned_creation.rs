//! Held private creation-attempt custody, separate from the viewer App map.
//!
//! N is the actual pre-admitted owner capacity, not a growing collection. No
//! slot is recycled: failed, late and cancelled HTTP observations remain in the
//! lifetime census. This is a building block, NOT evidence that legacy Factory
//! closures publish every writer. The trusted producer and writer roots must be
//! integrated before any positive creation or CLEAN decision.

use crate::owned_retained::{JoinObservation, RetainedJoin};
use tokio::{task::{JoinError, JoinHandle}, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CensusRefusal {
    Closed,
    Full,
    ForeignAttempt,
    MissingAttempt,
    Pending,
    Unknown,
    AlreadyPublished,
}

/// The incarnation is supplied by the actual scope owner, not an HTTP caller.
/// A key gives no access to an output or mutable join handle by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AttemptKey {
    incarnation: uuid::Uuid,
    index: usize,
}

type AttemptRoot<T> = RetainedJoin<JoinHandle<T>, Result<T, JoinError>>;

/// Unique process/App cleanup owner. Never Clone, Arc or hidden behind a
/// callback. Observer-side admission synchronisation is a separate integration
/// obligation; the exclusive methods cannot be called through App clones.
pub(crate) struct CreationCensus<T, const N: usize> {
    incarnation: uuid::Uuid,
    attempts: [Option<AttemptRoot<T>>; N],
    used: usize,
    closed: bool,
}

impl<T, const N: usize> CreationCensus<T, N> {
    /// The full inline array, task roots, task captures and output children must
    /// already be covered by the original F/G producer before construction.
    pub(crate) fn new(incarnation: uuid::Uuid) -> Self {
        Self {
            incarnation,
            attempts: std::array::from_fn(|_| None),
            used: 0,
            closed: false,
        }
    }

    /// Reserve BEFORE constructing/spawning a factory future or starting a
    /// recovery/writer effect. Publication is distinct so a suspended/gated task
    /// can be installed before its trusted producer is released.
    pub(crate) fn reserve(&mut self) -> Result<AttemptKey, CensusRefusal> {
        if self.closed { return Err(CensusRefusal::Closed); }
        if self.used == N { return Err(CensusRefusal::Full); }
        let index = self.used;
        self.attempts[index] = Some(RetainedJoin::before_shutdown());
        self.used += 1;
        Ok(AttemptKey { incarnation: self.incarnation, index })
    }

    fn root(&self, key: AttemptKey) -> Result<&AttemptRoot<T>, CensusRefusal> {
        if key.incarnation != self.incarnation { return Err(CensusRefusal::ForeignAttempt); }
        self.attempts.get(key.index).and_then(Option::as_ref)
            .ok_or(CensusRefusal::MissingAttempt)
    }

    fn root_mut(&mut self, key: AttemptKey) -> Result<&mut AttemptRoot<T>, CensusRefusal> {
        if key.incarnation != self.incarnation { return Err(CensusRefusal::ForeignAttempt); }
        self.attempts.get_mut(key.index).and_then(Option::as_mut)
            .ok_or(CensusRefusal::MissingAttempt)
    }

    /// Even after admission closes, an already-reserved in-flight attempt must
    /// publish its actual handle. Refusals return the same unconsumed handle to
    /// its owner; the caller must never drop it as a detach workaround.
    pub(crate) fn publish_handle(
        &mut self,
        key: AttemptKey,
        handle: JoinHandle<T>,
    ) -> Result<(), (CensusRefusal, JoinHandle<T>)> {
        let root = match self.root_mut(key) {
            Ok(root) => root,
            Err(refusal) => return Err((refusal, handle)),
        };
        root.publish_handle(handle)
            .map_err(|(_, handle)| (CensusRefusal::AlreadyPublished, handle))
    }

    pub(crate) fn close(&mut self) { self.closed = true; }

    pub(crate) fn observe(
        &self,
        key: AttemptKey,
    ) -> Result<JoinObservation<'_, Result<T, JoinError>>, CensusRefusal> {
        Ok(self.root(key)?.observe())
    }

    /// Main drives original task joins without moving their handles into a
    /// local timeout. A missing publication remains Pending and bars freezing.
    pub(crate) async fn join_attempt(
        &mut self,
        key: AttemptKey,
        original_shutdown_deadline: Instant,
    ) -> Result<JoinObservation<'_, Result<T, JoinError>>, CensusRefusal> {
        self.root_mut(key)?.join_until(original_shutdown_deadline).await
            .map_err(|_| CensusRefusal::Pending)
    }

    /// Freezes ATTEMPT membership only. Raw failed outputs are included and do
    /// not imply writer success, all-writer coverage, durable ACK or CLEAN.
    /// The borrowed view prevents reservation/mutation throughout evidence use.
    pub(crate) fn freeze(&self) -> Result<FrozenAttempts<'_, T, N>, CensusRefusal> {
        if !self.closed { return Err(CensusRefusal::Pending); }
        for slot in &self.attempts[..self.used] {
            let root = slot.as_ref().ok_or(CensusRefusal::MissingAttempt)?;
            match root.observe() {
                JoinObservation::Returned { was_unknown: false, .. } => {}
                JoinObservation::Returned { was_unknown: true, .. }
                | JoinObservation::Unknown => return Err(CensusRefusal::Unknown),
                JoinObservation::Pending => return Err(CensusRefusal::Pending),
            }
        }
        Ok(FrozenAttempts { census: self })
    }
}

pub(crate) struct FrozenAttempts<'a, T, const N: usize> {
    census: &'a CreationCensus<T, N>,
}

impl<T, const N: usize> FrozenAttempts<'_, T, N> {
    /// No projection/clone/normalisation of raw result payloads.
    pub(crate) fn originals(&self) -> impl Iterator<Item = &Result<T, JoinError>> {
        self.census.attempts[..self.census.used].iter().filter_map(|slot| {
            match slot.as_ref()?.observe() {
                JoinObservation::Returned { original, .. } => Some(original),
                _ => None,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future::Future, time::Duration};

    #[test]
    fn reservation_capacity_and_close_never_forget_pending_attempts() {
        let mut census = CreationCensus::<(), 1>::new(uuid::Uuid::from_u128(1));
        let key = census.reserve().unwrap();
        assert_eq!(census.reserve(), Err(CensusRefusal::Full));
        census.close();
        assert_eq!(census.reserve(), Err(CensusRefusal::Closed));
        assert!(matches!(census.observe(key), Ok(JoinObservation::Pending)));
        assert!(matches!(census.freeze(), Err(CensusRefusal::Pending)));
        let foreign = AttemptKey { incarnation: uuid::Uuid::from_u128(2), index: 0 };
        assert!(matches!(census.observe(foreign), Err(CensusRefusal::ForeignAttempt)));
    }

    #[tokio::test]
    async fn late_factory_result_survives_cancelled_http_observation_and_close() {
        let mut census = CreationCensus::<Result<Box<[u8; 3]>, ()>, 1>::new(
            uuid::Uuid::from_u128(3),
        );
        let key = census.reserve().unwrap();
        let (release, wait) = tokio::sync::oneshot::channel();
        let payload = Box::new([4_u8, 0, 255]);
        let address = payload.as_ptr() as usize;
        let handle = tokio::spawn(async move { wait.await.unwrap(); Ok(payload) });
        census.close();
        assert!(census.publish_handle(key, handle).is_ok());
        let deadline = Instant::now() + Duration::from_secs(120);
        {
            let mut observer = std::pin::pin!(census.join_attempt(key, deadline));
            std::future::poll_fn(|cx| {
                assert!(observer.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            }).await;
        }
        assert!(matches!(census.freeze(), Err(CensusRefusal::Pending)));
        release.send(()).unwrap();
        assert!(matches!(census.join_attempt(key, deadline).await,
            Ok(JoinObservation::Returned { original: Ok(Ok(_)), was_unknown: false })));
        let frozen = census.freeze().unwrap();
        let mut originals = frozen.originals();
        let original = originals.next().unwrap().as_ref().unwrap().as_ref().unwrap();
        assert_eq!(original.as_ptr() as usize, address);
        assert_eq!(**original, [4, 0, 255]);
        assert!(originals.next().is_none());
    }

    #[tokio::test]
    async fn bounded_unknown_retains_actual_task_and_never_freezes_as_clean() {
        let mut census = CreationCensus::<(), 1>::new(uuid::Uuid::from_u128(4));
        let key = census.reserve().unwrap();
        let (release, wait) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move { wait.await.unwrap(); });
        let task_id = handle.id();
        assert!(census.publish_handle(key, handle).is_ok());
        census.close();
        let expired = Instant::now() - Duration::from_secs(1);
        assert!(matches!(census.join_attempt(key, expired).await, Ok(JoinObservation::Unknown)));
        assert!(matches!(census.freeze(), Err(CensusRefusal::Unknown)));
        let root = census.root_mut(key).unwrap();
        assert_eq!(root.handle_for_poll().unwrap().id(), task_id);
        assert_eq!(root.deadline(), Some(expired));
        // Test teardown deliberately waits for actual completion. It neither
        // abandons a live fixture task nor models a production timeout retry.
        release.send(()).unwrap();
        let original = root.handle_for_poll().unwrap().await;
        assert!(root.publish_returned(original).is_ok());
        assert!(matches!(census.observe(key),
            Ok(JoinObservation::Returned { original: Ok(()), was_unknown: true })));
        assert!(matches!(census.freeze(), Err(CensusRefusal::Unknown)));
    }
}
