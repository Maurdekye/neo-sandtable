//! Private staged original-main service polling. There is no binary caller.
//! Main must pin its actual serving future in its ORIGINAL frame, then keep
//! this binding and that frame through Host joins and writer finalization.
//! Cancelling drive() borrows away neither the pinned future nor cleanup owner.
//! This does not prove Axum internal task custody, service drain or final fit.

use super::OwnedAppOwner;
use crate::{owned_ingress::IngressRefusal, owned_retained::Original};
use std::{future::Future, pin::Pin, task::Poll};

/// No clone, take, replacement-server, destructor join or hidden task. The
/// caller's pin and this original output slot have one binding for this life.
/// Its full body, real serving future/captures/wakers and every output/error
/// child still require actual original F/G/N before serving; no allowance here.
pub(crate) struct OriginalService<'a, S: Future, const N: usize, const R: usize> {
    owner: OwnedAppOwner<N, R>,
    serving: Pin<&'a mut S>,
    returned: Original<S::Output>,
    ingress_failure: Original<IngressRefusal>,
    serving_timeout: Original<tokio::time::error::Elapsed>,
    serving_deadline: Option<tokio::time::Instant>,
}

/// Borrow all three original facts. A scope-close request is not a joined
/// server/Host/writer, and missing output is not positive settlement evidence.
pub(crate) struct ServiceObservation<'a, O> {
    pub(crate) returned: Option<&'a O>,
    pub(crate) ingress_failure: Option<&'a IngressRefusal>,
    pub(crate) close: Option<&'a Result<(), IngressRefusal>>,
    pub(crate) close_started_at: Option<&'a tokio::time::Instant>,
}

impl<'a, S: Future, const N: usize, const R: usize> OriginalService<'a, S, N, R> {
    pub(crate) fn bind_prepaid(owner: OwnedAppOwner<N, R>, serving: Pin<&'a mut S>) -> Self {
        Self { owner, serving, returned: Original::empty(), ingress_failure: Original::empty(),
            serving_timeout: Original::empty(), serving_deadline: None }
    }

    pub(crate) fn owner_mut(&mut self) -> &mut OwnedAppOwner<N, R> { &mut self.owner }

    /// Supporting exported wrapper polls serving ONLY. No call to the current
    /// unguarded creator dispatch is exposed through this new public boundary.
    /// Guarded preparation/factory driving remains a separate source gap.
    pub(crate) async fn drive_serving_only(&mut self) {
        if self.returned.observe().is_some() || self.owner.creation.close_result().is_some() { return; }
        loop {
            let observer = self.owner.creation.observer();
            tokio::select! {
                original = self.serving.as_mut() => {
                    let rejected = self.returned.publish(original);
                    assert!(rejected.is_ok(), "original serving output already held");
                    let _ = self.owner.request_shutdown();
                    return;
                }
                _ = observer.owner_changed() => {
                    if self.owner.creation.close_result().is_some() { return; }
                }
            }
        }
    }

    pub(crate) fn serving_timeout(&self) -> Option<&tokio::time::error::Elapsed> {
        self.serving_timeout.observe()
    }

    /// Borrow the same pinned future after close, retaining actual output or
    /// FIRST timeout before observation. Cutoff never moves later on repeat.
    /// Not Axum task custody, Host joins, all-writer report or CLEAN evidence.
    pub(crate) async fn settle_serving_until(&mut self, requested: tokio::time::Instant) {
        let _ = self.owner.request_shutdown();
        let deadline = self.serving_deadline.map_or(requested, |old| old.min(requested));
        self.serving_deadline = Some(deadline);
        if self.returned.observe().is_some() { return; }
        match tokio::time::timeout_at(deadline, self.serving.as_mut()).await {
            Ok(original) => {
                let rejected = self.returned.publish(original);
                assert!(rejected.is_ok(), "unique original serving output");
            }
            Err(original) => {
                if self.serving_timeout.observe().is_none() {
                    let rejected = self.serving_timeout.publish(original);
                    assert!(rejected.is_ok(), "first actual serving timeout");
                }
            }
        }
    }

    pub(crate) fn observe(&self) -> ServiceObservation<'_, S::Output> {
        // Borrow the actual owner storage, not a new observer temporary whose
        // lifetime could escape here or a projected trait-level success flag.
        ServiceObservation {
            returned: self.returned.observe(),
            ingress_failure: self.ingress_failure.observe(),
            close: self.owner.creation.close_result(),
            close_started_at: self.owner.creation.close_started_at(),
        }
    }

    /// Normal serving only. Return on scope close, original serving completion
    /// or a held creator-drive failure, leaving ALL roots in this main binding.
    /// Main must next perform the approved bounded Host phase, THEN final writer
    /// cleanup. This method neither consumes that phase nor supplies its proof.
    pub(crate) async fn drive(&mut self) {
        if self.returned.observe().is_some() || self.ingress_failure.observe().is_some()
            || self.owner.creation.close_result().is_some()
        { return; }
        loop {
            let observer = self.owner.creation.observer();
            // This notification future survives each Pending select across
            // awaits. notify_one retains a permit if ingress arrived before
            // registration. Normal idle creator readiness must NOT hot-loop.
            tokio::select! {
                biased;
                refusal = std::future::poll_fn(|cx| {
                    match self.owner.creation.poll_creators(cx) {
                        Poll::Ready(Err(original)) => Poll::Ready(original),
                        Poll::Ready(Ok(())) | Poll::Pending => Poll::Pending,
                    }
                }) => {
                    let _ = self.ingress_failure.publish(refusal);
                    let _ = self.owner.request_shutdown();
                    return;
                }
                original = self.serving.as_mut() => {
                    // Unique original owner and entry guard prevent re-polling
                    // completed serving. Store its WHOLE actual output first.
                    let rejected = self.returned.publish(original);
                    assert!(rejected.is_ok(), "original serving output already held");
                    let _ = self.owner.request_shutdown();
                    return;
                }
                _ = observer.owner_changed() => {
                    if self.owner.creation.close_result().is_some() { return; }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{http::{CampaignKind, CreateRequest}, owned_ingress::CreationOwner};
    use std::{io, path::PathBuf, time::Duration};
    use tokio::{runtime::Handle, time::Instant};

    #[tokio::test]
    async fn borrowed_callback_clock_remains_original_while_service_is_pending() {
        let creation = CreationOwner::<1, 512>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(61), PathBuf::new(), PathBuf::new(),
        );
        let owner = OwnedAppOwner::new_prepaid(creation, 0);
        let app = owner.observer();
        let serving = std::future::pending::<io::Result<()>>();
        tokio::pin!(serving);
        let mut original = OriginalService::bind_prepaid(owner, serving.as_mut());
        assert!(original.observe().close_started_at.is_none());
        app.shutdown().await;
        let first_address = original.observe().close_started_at.unwrap() as *const _;
        let first = *original.observe().close_started_at.unwrap();
        original.drive().await;
        app.shutdown().await;
        drop(app);
        let deadline = first + Duration::from_secs(120);
        original.owner_mut().original_creation().join_creators_until(deadline).await.unwrap();
        let facts = original.observe();
        assert_eq!(facts.close_started_at.unwrap() as *const _, first_address);
        assert_eq!(*facts.close_started_at.unwrap(), first);
        assert_eq!(facts.close, Some(&Ok(())));
        assert!(facts.returned.is_none()); // Clock+CLOSE is still not a service join.
        assert!(facts.ingress_failure.is_none());
    }

    #[tokio::test]
    async fn original_service_drives_real_factory_then_holds_actual_service_error() {
        let directory = tempfile::tempdir().unwrap();
        let creation = CreationOwner::<1, 512>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(50),
            directory.path().to_owned(), directory.path().to_owned(),
        );
        let owner = OwnedAppOwner::new_prepaid(creation, 0);
        let app = owner.observer();
        let super::super::FactoryRoute::Owned(endpoint) = &app.factory else { panic!("owned route lost"); };
        let endpoint = std::sync::Arc::clone(endpoint);
        // Synthetic serving FUTURE ONLY, with the real concrete creation body.
        // No HTTP listener, Axum internal task or actual Host proof is claimed.
        let serving = async {
            let refusal = endpoint.create(CreateRequest {
                kind: CampaignKind::Sandbox, rules_profile: "unsupported-service-test".into(),
                seed: [10; 32], title: "Original serving request".into(), paused: true, controller: "human".into(),
            }).await;
            assert!(matches!(refusal, Err(super::super::OwnedCreateRefusal::FactoryFailed)));
            Err::<(), _>(io::Error::other("original serving failure"))
        };
        tokio::pin!(serving); // The same original future survives borrowing drive.
        let mut original = OriginalService::bind_prepaid(owner, serving.as_mut());
        original.drive().await;
        let address = {
            let facts = original.observe();
            assert_eq!(facts.close, Some(&Ok(())));
            assert!(facts.ingress_failure.is_none());
            match facts.returned.unwrap() {
                Err(raw) => { assert_eq!(raw.to_string(), "original serving failure"); raw as *const _ }
                _ => panic!("actual serving error not retained"),
            }
        };
        original.drive().await; // A completed async future must not be re-polled.
        match original.observe().returned.unwrap() {
            Err(raw) => assert_eq!(raw as *const _, address),
            _ => panic!("repeated drive replaced actual serving failure"),
        }
        let deadline = Instant::now() + Duration::from_secs(120);
        original.owner_mut().original_creation().join_creators_until(deadline).await.unwrap();
        let closed = original.owner.creation.closed_raw_view().unwrap();
        assert_eq!(closed.outcomes().count(), 1);
        assert!(original.owner.creation.freeze_raw().is_err());
        assert!(std::fs::read_dir(directory.path()).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn cancelled_drive_then_callback_close_keeps_pending_future_and_original_owner() {
        let creation = CreationOwner::<1, 512>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(51), PathBuf::new(), PathBuf::new(),
        );
        let owner = OwnedAppOwner::new_prepaid(creation, 0);
        let app = owner.observer();
        let serving = std::future::pending::<io::Result<()>>();
        tokio::pin!(serving);
        let mut original = OriginalService::bind_prepaid(owner, serving.as_mut());
        {
            let drive = original.drive();
            tokio::pin!(drive);
            std::future::poll_fn(|cx| {
                assert!(drive.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            }).await;
        } // Drop only the borrowing drive future, not the pinned service/owner.
        assert!(original.observe().returned.is_none());
        app.shutdown().await;
        original.drive().await;
        let facts = original.observe();
        assert_eq!(facts.close, Some(&Ok(())));
        assert!(facts.returned.is_none()); // CLOSE cannot masquerade as a join.
        assert!(facts.ingress_failure.is_none());
        let deadline = Instant::now() + Duration::from_secs(120);
        original.owner_mut().original_creation().join_creators_until(deadline).await.unwrap();
        assert_eq!(original.owner.creation.freeze_raw().unwrap().attempts().count(), 0);
        assert!(original.observe().returned.is_none());
    }
}

#[cfg(test)]
#[path = "owned_service_controls.rs"]
mod serving_settlement_controls;
