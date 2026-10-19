//! Held owned App adapter. No binary/live constructor calls this module yet.
//! The original outer frame keeps OwnedAppOwner; router/callback clones contain
//! only the endpoint's CreationObserver. They own no factory/writer JoinHandle.
//! No background driver, global registry, public task-registration or guessed
//! Seats API. Actual durable OPEN, full allowance and final Host wiring remain
//! prerequisites. Positive creation is explicitly CLOSED pending member binding.

use super::*;
use crate::owned_ingress::{CreationInput, CreationObservation, CreationObserver, CreationOwner, IngressRefusal};

#[derive(Clone)]
pub(super) enum FactoryRoute {
    Legacy(Factory),
    Owned(Arc<dyn OwnedEndpoint>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnedCreateRefusal {
    Admission(IngressRefusal),
    FactoryFailed,
    CreatorJoinFailed,
    CancelledBeforeEffects,
    Unknown,
    BindingPending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnedShutdownRequest { HostJoinPending }

/// Server-private HTTP adapter only, not a new public controller/protocol trait.
/// The boxed async-trait request future, Arc backing, captures, error response
/// and every input child remain in original F/G before effects. No fit supplied.
#[async_trait::async_trait]
pub(super) trait OwnedEndpoint: Send + Sync {
    async fn create(&self, request: CreateRequest) -> Result<CampaignHandle, OwnedCreateRefusal>;
    fn request_shutdown(&self) -> Result<OwnedShutdownRequest, IngressRefusal>;
    fn check_admission(&self) -> Result<(), IngressRefusal>;
}

struct Endpoint<const N: usize> { observer: CreationObserver<N> }

#[async_trait::async_trait]
impl<const N: usize> OwnedEndpoint for Endpoint<N> {
    async fn create(&self, request: CreateRequest) -> Result<CampaignHandle, OwnedCreateRefusal> {
        let key = self.observer.submit(CreationInput::Fresh(request))
            .map_err(|(cause, _unadmitted_original)| OwnedCreateRefusal::Admission(cause))?;
        match self.observer.wait(key).await.map_err(OwnedCreateRefusal::Admission)? {
            CreationObservation::Returned(Err(_)) => Err(OwnedCreateRefusal::FactoryFailed),
            CreationObservation::JoinFailed => Err(OwnedCreateRefusal::CreatorJoinFailed),
            CreationObservation::CancelledBeforeEffects => Err(OwnedCreateRefusal::CancelledBeforeEffects),
            CreationObservation::Unknown | CreationObservation::Pending => Err(OwnedCreateRefusal::Unknown),
            CreationObservation::Returned(Ok(_original)) => {
                // Original product/writer/stop roots remain in the parent. Never
                // publish an unbound facade or pass a new legacy registration as
                // proof. Exact namespace/member+pin validation and parent map/GO
                // publication have to replace this refusal in the joint packet.
                Err(OwnedCreateRefusal::BindingPending)
            }
        }
    }

    fn request_shutdown(&self) -> Result<OwnedShutdownRequest, IngressRefusal> {
        self.observer.request_close()?;
        Ok(OwnedShutdownRequest::HostJoinPending)
    }

    fn check_admission(&self) -> Result<(), IngressRefusal> { self.observer.check_admission() }
}

/// Unique owner: never Clone, never moved into the graceful callback/router.
/// Later main/Play wiring must retain this original through Host joins, then
/// drive its stored writer stops/joins/evidence and final global report.
#[allow(dead_code)]
pub(crate) struct OwnedAppOwner<const N: usize, const R: usize> {
    observer_app: App,
    creation: CreationOwner<N, R>,
}

#[allow(dead_code)]
impl<const N: usize, const R: usize> OwnedAppOwner<N, R> {
    /// Full actual original allowance and the durable OPEN owner boundary must
    /// already exist; no live callsite or admission producer does yet. This is
    /// private staging, not an alternative to the approved process SeatHost.
    pub(crate) fn new_prepaid(creation: CreationOwner<N, R>, port: u16) -> Self {
        let endpoint = Endpoint { observer: creation.observer() };
        let observer_app = App {
            campaigns: Arc::new(RwLock::new(BTreeMap::new())),
            directory: creation.storage_directory().to_owned(),
            factory: FactoryRoute::Owned(Arc::new(endpoint)),
            port,
            capabilities: Arc::new(Capabilities::new()),
        };
        Self { observer_app, creation }
    }

    pub(crate) fn observer(&self) -> App { self.observer_app.clone() }

    pub(crate) fn request_shutdown(&self) -> Result<OwnedShutdownRequest, IngressRefusal> {
        match &self.observer_app.factory {
            FactoryRoute::Owned(endpoint) => endpoint.request_shutdown(),
            FactoryRoute::Legacy(_) => unreachable!("private owned constructor has no legacy fallback"),
        }
    }

    pub(crate) fn original_creation(&mut self) -> &mut CreationOwner<N, R> { &mut self.creation }
}

#[path = "owned_service.rs"]
pub(crate) mod service;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owned_retained::JoinObservation;
    use crate::owned_writer::WriterStartFailure;
    use std::{future::Future, task::Poll};
    use tokio::{runtime::Handle, time::Instant};

    #[tokio::test]
    async fn cancelled_endpoint_and_borrowed_shutdown_leave_original_cleanup_owner() {
        let directory = tempfile::tempdir().unwrap();
        let unopened = directory.path().join("never-open-owned-app");
        let creation = CreationOwner::<1, 512>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(40),
            unopened.clone(), PathBuf::from("unused-data"),
        );
        let mut owner = OwnedAppOwner::new_prepaid(creation, 0);
        let app = owner.observer();
        let FactoryRoute::Owned(endpoint) = &app.factory else { panic!("owned route lost"); };
        let endpoint = Arc::clone(endpoint);
        let mut request = endpoint.create(CreateRequest {
            kind: CampaignKind::Sandbox, rules_profile: cna_sandbox::PROFILE_ID.into(),
            seed: [8; 32], title: "Cancelled HTTP observer".into(), paused: true, controller: "human".into(),
        });
        std::future::poll_fn(|cx| {
            assert!(request.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        }).await;
        drop(request); // Cancels only the observer, not the admitted original.
        app.shutdown().await; // No owner driver/helper is running to satisfy this.
        let close_address = owner.creation.observer().close_result().unwrap() as *const _;
        assert_eq!(endpoint.check_admission(), Err(IngressRefusal::Closed));
        drop(app);
        assert_eq!(owner.request_shutdown(), Ok(OwnedShutdownRequest::HostJoinPending));
        assert_eq!(owner.creation.observer().close_result().unwrap() as *const _, close_address);
        let deadline = Instant::now() + Duration::from_secs(120);
        owner.original_creation().join_creators_until(deadline).await.unwrap();
        let closed = owner.creation.closed_raw_view().unwrap();
        let mut all = closed.outcomes();
        let only = all.next().unwrap();
        assert!(std::ptr::eq(only.source, &owner.creation));
        assert!(only.product.is_none());
        assert!(only.writer.can_publish());
        assert!(only.validation.is_ok()); // Actual retained no-effect checks.
        assert!(all.next().is_none());
        assert_eq!(owner.creation.freeze_raw().unwrap().attempts().count(), 1);
        assert!(!unopened.exists());
    }

    #[tokio::test]
    async fn actual_factory_error_survives_http_refusal_and_repeated_callback_close() {
        let directory = tempfile::tempdir().unwrap();
        let creation = CreationOwner::<1, 512>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(41),
            directory.path().to_owned(), directory.path().to_owned(),
        );
        let mut owner = OwnedAppOwner::new_prepaid(creation, 0);
        let app = owner.observer();
        let FactoryRoute::Owned(endpoint) = &app.factory else { panic!("owned route lost"); };
        let endpoint = Arc::clone(endpoint);
        // The actual Sandbox producer rejects this before storage creation.
        // No injected Factory, duplicate Engine fixture or replacement error.
        let mut request = endpoint.create(CreateRequest {
            kind: CampaignKind::Sandbox, rules_profile: "unsupported-owned-test".into(),
            seed: [9; 32], title: "Actual factory refusal".into(), paused: true, controller: "human".into(),
        });
        std::future::poll_fn(|cx| {
            assert!(request.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        }).await;
        std::future::poll_fn(|cx| owner.original_creation().poll_creators(cx)).await.unwrap();
        app.shutdown().await;
        let deadline = Instant::now() + Duration::from_secs(120);
        owner.original_creation().join_creators_until(deadline).await.unwrap();
        let original_address = {
            let closed = owner.creation.closed_raw_view().unwrap();
            let first = closed.outcomes().next().unwrap();
            assert!(matches!(first.creator, JoinObservation::Returned { original: Ok(_), was_unknown: false }));
            match first.product.unwrap() {
                Err(WriterStartFailure::Setup(Error::Invalid(original))) => {
                    assert_eq!(original, "only sandbox-v1 is implemented");
                    original as *const _
                }
                _ => panic!("actual original factory error was replaced"),
            }
        };
        assert!(matches!(request.await, Err(OwnedCreateRefusal::FactoryFailed)));
        app.shutdown().await;
        drop(app);
        owner.original_creation().join_creators_until(deadline + Duration::from_secs(120)).await.unwrap();
        let closed = owner.creation.closed_raw_view().unwrap();
        let mut all = closed.outcomes();
        let first = all.next().unwrap();
        assert!(first.validation.is_err()); // Failed factory is not no-writer proof.
        match first.product.unwrap() {
            Err(WriterStartFailure::Setup(Error::Invalid(original))) => assert_eq!(original as *const _, original_address),
            _ => panic!("HTTP refusal or cleanup consumed the original error"),
        }
        assert!(all.next().is_none());
        assert!(owner.creation.freeze_raw().is_err());
        assert!(std::fs::read_dir(directory.path()).unwrap().next().is_none());
    }
}
