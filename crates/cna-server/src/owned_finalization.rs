//! Private staged original-owner final writer phase. No live caller exists.
//! The actual caller MUST retain the real Host result and finish its required
//! seat/child joins BEFORE entering here. No boolean flag replaces that proof.
//! Host report/deadline API, process main wiring and full F/G remain unfinished.
//! All original raw roots survive borrowing-future cancellation or timeout.

use super::*;
use crate::owned_retained::{Original, PublicationRefusal};
use crate::owned_writer::WriterRefusal;

/// Inline original storage, constructed with the owner BEFORE factory effects.
/// These typed dispatch/publication refusals have no hidden error payload. Raw
/// creator/stop/standard/Tokio failures stay in their existing canonical roots.
/// The larger owner/future/array backing still needs complete original F/G/N.
pub(super) struct FinalizationRoots<const N: usize> {
    pub(super) ingress_refusal: Original<IngressRefusal>,
    pub(super) dispatch_refusals: [Original<WriterRefusal>; N],
    pub(super) join_refusals: [Original<PublicationRefusal>; N],
}

impl<const N: usize> FinalizationRoots<N> {
    pub(super) fn new() -> Self {
        Self { ingress_refusal: Original::empty(),
            dispatch_refusals: std::array::from_fn(|_| Original::empty()),
            join_refusals: std::array::from_fn(|_| Original::empty()) }
    }
}

impl<const N: usize, const R: usize> CreationOwner<N, R> {
    pub(crate) fn finalization_refusal(&self) -> Option<&IngressRefusal> {
        self.finalization.ingress_refusal.observe()
    }

    /// After the actual Host phase ONLY. The supplied cutoff must be its SAME
    /// original deadline, not now+another window. This function cannot certify
    /// that caller contract until the exact Host/main integration is present.
    /// It never returns a positive settlement bit; later borrowed raw evidence
    /// still checks every creator/product/writer/ACK plus sticky uncertainty.
    pub(crate) async fn finalize_writers_after_host_until(&mut self, requested: Instant) {
        let deadline = match self.close(requested) {
            Ok(original) => original,
            Err(original) => {
                let _ = self.finalization.ingress_refusal.publish(original);
                return; // All raw roots remain here; no closure/absence claim.
            }
        };
        if let Err(original) = self.join_creators_until(deadline).await {
            let _ = self.finalization.ingress_refusal.publish(original);
            // A creator failure must not hide another published standard handle.
        }
        let used = match self.shared.admission.lock() {
            Ok(admission) => admission.used,
            Err(_) => {
                let _ = self.finalization.ingress_refusal.publish(IngressRefusal::Poisoned);
                return; // Membership cannot be asserted from a poisoned census.
            }
        }; // No mutex guard spans stop/dispatch/join/await/wake.
        for index in 0..used {
            // Skip ONLY concrete proof of no writer effects. Missing facade,
            // failed product, unknown creator or no stop root is not that proof.
            if matches!(self.raw_attempt(index), Ok(evidence::RawAttempt::NoEffects { .. })) { continue; }
            if let Some(stop) = self.stops[index].as_mut() {
                let _ = stop.settle_until(deadline).await;
            }
            if !self.writers[index].join_dispatched() {
                if let Err(original) = self.writers[index].dispatch_join(&self.runtime) {
                    let _ = self.finalization.dispatch_refusals[index].publish(original);
                }
            }
            if self.writers[index].join_dispatched() {
                if let Err(original) = self.writers[index].join_until(deadline).await {
                    let _ = self.finalization.join_refusals[index].publish(original);
                }
            }
            // Every later entry is still visited after a refusal/UNKNOWN. Even
            // late actual completion cannot erase a previously held refusal.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn failed_creator_and_later_queued_attempt_remain_visible_after_finalization() {
        let directory = tempfile::tempdir().unwrap();
        let mut owner = CreationOwner::<2, 512>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(70),
            directory.path().to_owned(), directory.path().to_owned(),
        );
        let observer = owner.observer();
        let failed = observer.submit(CreationInput::Recover(directory.path().join("missing.sqlite"))).unwrap();
        std::future::poll_fn(|cx| owner.poll_creators(cx)).await.unwrap();
        let queued = observer.submit(CreationInput::Recover(directory.path().join("never.sqlite"))).unwrap();
        let deadline = Instant::now() + Duration::from_secs(120);
        owner.finalize_writers_after_host_until(deadline).await;
        let refusal_address = owner.finalization.dispatch_refusals[0].observe().unwrap() as *const _;
        assert_eq!(owner.finalization.dispatch_refusals[0].observe(), Some(&WriterRefusal::MissingStandard));
        assert!(owner.finalization.dispatch_refusals[1].observe().is_none());
        assert!(!owner.writers[1].join_dispatched());
        owner.finalize_writers_after_host_until(deadline + Duration::from_secs(120)).await;
        assert_eq!(owner.finalization.dispatch_refusals[0].observe().unwrap() as *const _, refusal_address);
        let closed = owner.closed_raw_view().unwrap();
        let mut all = closed.outcomes();
        let first = all.next().unwrap();
        assert_eq!(first.key, failed);
        assert!(matches!(first.product, Some(Err(WriterStartFailure::Setup(Error::Storage(_))))));
        assert!(first.dispatch_refusal.is_some());
        assert!(first.validation.is_err());
        let second = all.next().unwrap();
        assert_eq!(second.key, queued);
        assert!(matches!(second.validation, Ok(evidence::RawAttempt::NoEffects { .. })));
        assert!(second.dispatch_refusal.is_none());
        assert!(all.next().is_none());
        assert!(owner.freeze_raw().is_err());
    }

    #[tokio::test]
    async fn real_no_seat_writer_finalization_keeps_ack_and_actual_standard_join() {
        let directory = tempfile::tempdir().unwrap();
        let mut owner = CreationOwner::<1, 16384>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(71), directory.path().to_owned(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data"),
        );
        let key = owner.observer().submit(CreationInput::Fresh(CreateRequest {
            kind: crate::http::CampaignKind::Sandbox, rules_profile: cna_sandbox::PROFILE_ID.into(),
            seed: [11; 32], title: "Final writer phase".into(), paused: true, controller: "human".into(),
        })).unwrap();
        std::future::poll_fn(|cx| owner.poll_creators(cx)).await.unwrap();
        owner.writers[0].release().unwrap();
        // No CLI/seat/Host exists in this fixture: it tests the actual writer
        // phase only, not the production Host-before-writer ordering contract.
        let deadline = Instant::now() + Duration::from_secs(120);
        owner.finalize_writers_after_host_until(deadline).await;
        let address = match owner.writers[0].observe() {
            JoinObservation::Returned { original: raw @ Ok(Ok(Ok(()))), was_unknown: false } => raw as *const _,
            _ => panic!("actual standard/blocking writer join was not retained"),
        };
        assert!(matches!(owner.stop_observation(key).unwrap(),
            StopObservation::Reply { original: Ok(Ok(serde_json::Value::Null)), prior_timeout: None }));
        assert_eq!(owner.freeze_raw().unwrap().attempts().count(), 1);
        owner.finalize_writers_after_host_until(deadline + Duration::from_secs(120)).await;
        match owner.writers[0].observe() {
            JoinObservation::Returned { original, was_unknown: false } => assert_eq!(original as *const _, address),
            _ => panic!("repeated finalization replaced or lost the actual join"),
        }
        assert!(owner.finalization.dispatch_refusals[0].observe().is_none());
        assert!(owner.finalization.join_refusals[0].observe().is_none());
    }

    #[tokio::test]
    async fn missing_facade_does_not_hide_published_standard_handle_or_original_error() {
        let mut owner = CreationOwner::<1, 512>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(72), PathBuf::new(), PathBuf::new(),
        );
        let key = owner.observer().submit(CreationInput::Recover(PathBuf::from("never-open.sqlite"))).unwrap();
        let message = String::from("original writer error without facade");
        let allocation = message.as_ptr();
        // Deliberately contradictory lifecycle fixture: an actual standard
        // handle exists although creator/product/facade was not published. The
        // original cleanup root must join it, not infer absence from the facade.
        let vacant = owner.writers[0].vacant_standard().unwrap();
        let actual = std::thread::spawn(move || Err::<(), _>(Error::Invalid(message)));
        vacant.publish(actual);
        owner.finalize_writers_after_host_until(Instant::now() + Duration::from_secs(120)).await;
        match owner.writers[0].observe() {
            JoinObservation::Returned { original: Ok(Ok(Err(Error::Invalid(raw)))), was_unknown: false } => {
                assert_eq!(raw.as_ptr(), allocation);
                assert_eq!(raw, "original writer error without facade");
            }
            _ => panic!("published standard handle or actual raw error disappeared"),
        }
        assert!(owner.stop_observation(key).is_err());
        assert!(owner.freeze_raw().is_err());
        let closed = owner.closed_raw_view().unwrap();
        let only = closed.outcomes().next().unwrap();
        assert!(only.product.is_none());
        assert!(only.validation.is_err());
        assert!(std::ptr::eq(only.writer, &owner.writers[0]));
    }
}
