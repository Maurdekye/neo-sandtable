//! Frozen raw evidence from the ORIGINAL concrete owner, without a second Vec,
//! copied errors, normalized reply, hidden observer task or mutex across use.
//! This is not a namespace/member token or permission to record global CLEAN.
//! Actual App ingress coverage, Host frozen members, canonical file/alias and
//! archive authority, expected pins and final deadline/durability checks remain
//! required in the one joint implementation. No live callsite exists yet.

use super::*;
use crate::owned_writer::WriterRowEvidence;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FreezeFailure {
    Ingress(IngressRefusal),
    AdmissionOpen,
    NoOwnerDeadline,
    PendingCreator,
    UnknownCreator,
    CreatorJoinFailed,
    ProductFailed,
    ContradictoryNoEffects,
    Writer(WriterEvidenceRefusal),
    NoStopRoot,
    StopPending,
    StopUnknown,
    StopQueueClosed,
    ReplyLost,
    StopRejected,
    UnexpectedAck,
    DuplicateCampaign { earlier: usize },
    DuplicateRawPath { earlier: usize },
    FinalizationRefused,
    WriterDispatchRefused,
    WriterJoinRefused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FreezeRefusal {
    pub(crate) index: Option<usize>,
    pub(crate) reason: FreezeFailure,
}

/// Each item retains access to the original whole owner. NoEffects is backed
/// by its never-reused attempt slot/actual nonpublication or exact returned
/// CreatorExit, not a disappeared handle, missing product or final map count.
pub(crate) enum RawAttempt<'a, const N: usize, const R: usize> {
    NoEffects {
        key: CreatorKey,
        source: &'a CreationOwner<N, R>,
        creator: Option<&'a Result<CreatorExit, JoinError>>,
    },
    Writer {
        key: CreatorKey,
        source: &'a CreationOwner<N, R>,
        creator: &'a Result<CreatorExit, JoinError>,
        product: &'a Product,
        identity: WriterPreparedIdentity<'a, R>,
        rows: WriterRowEvidence<'a, R>,
        stop: &'a OwnedStop,
    },
}

/// The borrow prevents original owner mutation/dispatch through this view.
/// Closed admission prevents observer clones adding a late attempt. A successful
/// view has checked every reserved index, including queued/cancelled/no-seat
/// attempts. Any failure refuses the view, never an apparently empty success.
pub(crate) struct FrozenRawSet<'a, const N: usize, const R: usize> {
    source: &'a CreationOwner<N, R>,
    used: usize,
}

/// Closed membership for truthful UNKNOWN reporting too. Unlike FrozenRawSet,
/// this permits incomplete/failed outcomes and exposes every original root.
/// It is never a positive settlement/CLEAN claim. Consumers visit every item,
/// so the first failure cannot hide a later writer or queued original request.
pub(crate) struct ClosedRawView<'a, const N: usize, const R: usize> {
    source: &'a CreationOwner<N, R>,
    used: usize,
}

pub(crate) struct RawInspection<'a, const N: usize, const R: usize> {
    pub(crate) key: CreatorKey,
    pub(crate) source: &'a CreationOwner<N, R>,
    pub(crate) creator: JoinObservation<'a, Result<CreatorExit, JoinError>>,
    pub(crate) product: Option<&'a Product>,
    pub(crate) writer: &'a WriterJoin<Result<(), Error>, R>,
    pub(crate) stop: Option<&'a OwnedStop>,
    pub(crate) validation: Result<RawAttempt<'a, N, R>, FreezeRefusal>,
    pub(crate) dispatch_refusal: Option<&'a crate::owned_writer::WriterRefusal>,
    pub(crate) join_refusal: Option<&'a crate::owned_retained::PublicationRefusal>,
}

impl<'a, const N: usize, const R: usize> ClosedRawView<'a, N, R> {
    pub(crate) fn outcomes(&self) -> impl Iterator<Item = RawInspection<'a, N, R>> + '_ {
        let source = self.source;
        (0..self.used).map(move |index| RawInspection {
            key: CreatorKey { incarnation: source.shared.incarnation, index },
            source,
            creator: source.tasks[index].observe(),
            product: source.shared.replies[index].product.get(),
            writer: &source.writers[index],
            stop: source.stops[index].as_ref(),
            validation: source.raw_attempt(index),
            dispatch_refusal: source.finalization.dispatch_refusals[index].observe(),
            join_refusal: source.finalization.join_refusals[index].observe(),
        })
    }
}

impl<'a, const N: usize, const R: usize> FrozenRawSet<'a, N, R> {
    pub(crate) fn incarnation(&self) -> uuid::Uuid { self.source.shared.incarnation }

    pub(crate) fn attempts(&self) -> impl Iterator<Item = Result<RawAttempt<'a, N, R>, FreezeRefusal>> + '_ {
        let source = self.source;
        (0..self.used).map(move |index| source.raw_attempt(index))
    }
}

impl<const N: usize, const R: usize> CreationOwner<N, R> {
    fn no_writer_effects(&self, index: usize) -> bool {
        // All actual publication paths claim before standard-thread creation.
        // A claimed-but-failed constructor must therefore remain a refusal.
        self.writers[index].can_publish()
            && self.writers[index].record().prepared().is_none()
            && self.writers[index].record().initial().is_none()
            && self.writers[index].record().terminal().is_none()
            && self.shared.replies[index].product.get().is_none()
            && self.stops[index].is_none()
            && matches!(self.writers[index].observe(), JoinObservation::Pending)
    }

    pub(super) fn raw_attempt(&self, index: usize) -> Result<RawAttempt<'_, N, R>, FreezeRefusal> {
        let refused = |reason| FreezeRefusal { index: Some(index), reason };
        if self.finalization.dispatch_refusals[index].observe().is_some() {
            return Err(refused(FreezeFailure::WriterDispatchRefused));
        }
        if self.finalization.join_refusals[index].observe().is_some() {
            return Err(refused(FreezeFailure::WriterJoinRefused));
        }
        let key = CreatorKey { incarnation: self.shared.incarnation, index };
        if !self.started[index] {
            let input_present = {
                let admission = self.shared.admission.lock()
                    .map_err(|_| refused(FreezeFailure::Ingress(IngressRefusal::Poisoned)))?;
                admission.inputs[index].is_some()
            };
            if !input_present || !self.terminal_published[index]
                || self.shared.replies[index].status.load(Ordering::Acquire) != NOT_STARTED
                || self.shared.replies[index].gate.state.load(Ordering::Acquire) != CANCELLED
                || !self.tasks[index].is_unpublished() || !self.no_writer_effects(index)
            {
                return Err(refused(FreezeFailure::ContradictoryNoEffects));
            }
            return Ok(RawAttempt::NoEffects { key, source: self, creator: None });
        }
        let creator = match self.tasks[index].observe() {
            JoinObservation::Pending => return Err(refused(FreezeFailure::PendingCreator)),
            JoinObservation::Unknown | JoinObservation::Returned { was_unknown: true, .. } => {
                return Err(refused(FreezeFailure::UnknownCreator));
            }
            JoinObservation::Returned { original, was_unknown: false } => original,
        };
        match creator {
            Err(_) => return Err(refused(FreezeFailure::CreatorJoinFailed)),
            Ok(CreatorExit::CancelledBeforeEffects) => {
                if self.shared.replies[index].gate.state.load(Ordering::Acquire) != CANCELLED
                    || !self.no_writer_effects(index)
                {
                    return Err(refused(FreezeFailure::ContradictoryNoEffects));
                }
                return Ok(RawAttempt::NoEffects { key, source: self, creator: Some(creator) });
            }
            Ok(CreatorExit::Produced) => {}
        }
        let product = match self.shared.replies[index].product.get() {
            Some(original @ Ok(_)) => original,
            _ => return Err(refused(FreezeFailure::ProductFailed)),
        };
        let rows = self.writers[index].terminal_rows().map_err(|cause| refused(FreezeFailure::Writer(cause)))?;
        let identity = self.writers[index].prepared_identity().map_err(|cause| refused(FreezeFailure::Writer(cause)))?;
        let stop = self.stops[index].as_ref().ok_or_else(|| refused(FreezeFailure::NoStopRoot))?;
        match stop.observe() {
            StopObservation::Pending { .. } => return Err(refused(FreezeFailure::StopPending)),
            StopObservation::Unknown { .. } => return Err(refused(FreezeFailure::StopUnknown)),
            StopObservation::QueueClosed { .. } => return Err(refused(FreezeFailure::StopQueueClosed)),
            StopObservation::Reply { prior_timeout: Some(_), .. } => return Err(refused(FreezeFailure::StopUnknown)),
            StopObservation::Reply { original: Err(_), .. } => return Err(refused(FreezeFailure::ReplyLost)),
            StopObservation::Reply { original: Ok(Err(_)), .. } => return Err(refused(FreezeFailure::StopRejected)),
            StopObservation::Reply { original: Ok(Ok(serde_json::Value::Null)), prior_timeout: None } => {}
            StopObservation::Reply { .. } => return Err(refused(FreezeFailure::UnexpectedAck)),
        }
        Ok(RawAttempt::Writer { key, source: self, creator, product, identity, rows, stop })
    }

    /// Closed COMPLETE membership of this concrete owner only, including failed
    /// and unfinished outcomes for UNKNOWN reporting. No other App/Host scope
    /// or arbitrary legacy Factory route is claimed covered by this view.
    pub(crate) fn closed_raw_view(&self) -> Result<ClosedRawView<'_, N, R>, FreezeRefusal> {
        let used = {
            let admission = self.shared.admission.lock().map_err(|_| FreezeRefusal {
                index: None, reason: FreezeFailure::Ingress(IngressRefusal::Poisoned),
            })?;
            if !admission.closed { return Err(FreezeRefusal { index: None, reason: FreezeFailure::AdmissionOpen }); }
            admission.used
        };
        if self.shutdown_deadline.is_none() {
            return Err(FreezeRefusal { index: None, reason: FreezeFailure::NoOwnerDeadline });
        }
        Ok(ClosedRawView { source: self, used })
    }

    /// Positive raw rows require each reserved entry. Equal IDs/raw paths are
    /// refused; unequal paths are NOT proof of different files until separate
    /// canonical namespace/alias validation. This cannot authorize global CLEAN.
    pub(crate) fn freeze_raw(&self) -> Result<FrozenRawSet<'_, N, R>, FreezeRefusal> {
        if self.finalization.ingress_refusal.observe().is_some() {
            return Err(FreezeRefusal { index: None, reason: FreezeFailure::FinalizationRefused });
        }
        let used = self.closed_raw_view()?.used;
        for index in 0..used {
            let current = self.raw_attempt(index)?;
            if let RawAttempt::Writer { identity, .. } = current {
                for earlier in 0..index {
                    if let RawAttempt::Writer { identity: prior, .. } = self.raw_attempt(earlier)? {
                        if prior.meta.id == identity.meta.id {
                            return Err(FreezeRefusal { index: Some(index), reason: FreezeFailure::DuplicateCampaign { earlier } });
                        }
                        if prior.connection_path == identity.connection_path {
                            return Err(FreezeRefusal { index: Some(index), reason: FreezeFailure::DuplicateRawPath { earlier } });
                        }
                    }
                }
            }
        }
        Ok(FrozenRawSet { source: self, used })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn closed_queued_attempts_stay_in_frozen_original_owner_without_dispatch() {
        let mut owner = CreationOwner::<2, 512>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(30), PathBuf::new(), PathBuf::new(),
        );
        let observer = owner.observer();
        let first = observer.submit(CreationInput::Recover(PathBuf::from("first.sqlite"))).unwrap();
        let second = observer.submit(CreationInput::Recover(PathBuf::from("second.sqlite"))).unwrap();
        assert!(matches!(owner.freeze_raw(), Err(FreezeRefusal { reason: FreezeFailure::AdmissionOpen, .. })));
        drop(observer);
        owner.close(Instant::now()+Duration::from_secs(120)).unwrap();
        let frozen = owner.freeze_raw().unwrap();
        assert_eq!(frozen.incarnation(), uuid::Uuid::from_u128(30));
        let mut entries = frozen.attempts();
        for expected in [first, second] {
            match entries.next().unwrap().unwrap() {
                RawAttempt::NoEffects { key, source, creator: None } => {
                    assert_eq!(key, expected);
                    assert!(std::ptr::eq(source, &owner));
                    assert!(owner.tasks[key.index].is_unpublished());
                }
                _ => panic!("queued original attempt was replaced or omitted"),
            }
        }
        assert!(entries.next().is_none());
    }

    #[tokio::test]
    async fn real_failed_factory_is_not_silently_omitted_from_frozen_set() {
        let directory = tempfile::tempdir().unwrap();
        let mut owner = CreationOwner::<2, 512>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(31),
            directory.path().to_owned(), directory.path().to_owned(),
        );
        let key = owner.observer().submit(CreationInput::Recover(directory.path().join("absent.sqlite"))).unwrap();
        std::future::poll_fn(|cx| owner.poll_creators(cx)).await.unwrap();
        let queued = owner.observer().submit(CreationInput::Recover(directory.path().join("queued.sqlite"))).unwrap();
        owner.join_creators_until(Instant::now()+Duration::from_secs(120)).await.unwrap();
        assert!(matches!(owner.freeze_raw(), Err(FreezeRefusal { index: Some(0), reason: FreezeFailure::ProductFailed })));
        assert!(matches!(owner.observer().observe(key).unwrap(),
            CreationObservation::Returned(Err(WriterStartFailure::Setup(Error::Storage(_))))));
        assert!(matches!(owner.tasks[0].observe(), JoinObservation::Returned { original: Ok(CreatorExit::Produced), was_unknown: false }));
        let closed = owner.closed_raw_view().unwrap();
        let mut all = closed.outcomes();
        let first = all.next().unwrap();
        assert_eq!(first.key, key);
        assert!(std::ptr::eq(first.source, &owner));
        assert!(std::ptr::eq(first.product.unwrap(), owner.shared.replies[0].product.get().unwrap()));
        assert!(matches!(first.validation, Err(FreezeRefusal { reason: FreezeFailure::ProductFailed, .. })));
        let second = all.next().unwrap();
        assert_eq!(second.key, queued);
        assert!(matches!(second.validation, Ok(RawAttempt::NoEffects { .. })));
        assert!(all.next().is_none());
    }

    #[tokio::test]
    async fn real_no_seat_sandbox_writer_requires_actual_ack_and_original_join() {
        let directory = tempfile::tempdir().unwrap();
        let data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let mut owner = CreationOwner::<1, 16384>::new_prepaid(
            Handle::current(), uuid::Uuid::from_u128(32), directory.path().to_owned(), data,
        );
        let key = owner.observer().submit(CreationInput::Fresh(CreateRequest {
            kind: crate::http::CampaignKind::Sandbox, rules_profile: cna_sandbox::PROFILE_ID.into(),
            seed: [7; 32], title: "Original no-seat writer".into(), paused: true, controller: "human".into(),
        })).unwrap();
        std::future::poll_fn(|cx| owner.poll_creators(cx)).await.unwrap();
        assert_eq!(owner.prepared_binding(key).unwrap().pins.rules_profile, cna_sandbox::PROFILE_ID);
        owner.writers[0].release().unwrap();
        let deadline = Instant::now()+Duration::from_secs(120);
        owner.join_creators_until(deadline).await.unwrap();
        assert!(matches!(owner.freeze_raw(), Err(FreezeRefusal { reason: FreezeFailure::Writer(WriterEvidenceRefusal::Pending), .. })));
        assert!(matches!(owner.settle_writer_stop(key).await.unwrap(),
            StopObservation::Reply { original: Ok(Ok(serde_json::Value::Null)), prior_timeout: None }));
        assert!(matches!(owner.freeze_raw(), Err(FreezeRefusal { reason: FreezeFailure::Writer(WriterEvidenceRefusal::Pending), .. })));
        owner.writers[0].dispatch_join(&owner.runtime).unwrap();
        assert!(matches!(owner.writers[0].join_until(deadline).await,
            Ok(JoinObservation::Returned { original: Ok(Ok(Ok(()))), was_unknown: false })));
        let frozen = owner.freeze_raw().unwrap();
        let mut facts = frozen.attempts();
        match facts.next().unwrap().unwrap() {
            RawAttempt::Writer { key: actual, source, creator, product, rows, identity, stop } => {
                assert_eq!(actual, key);
                assert!(std::ptr::eq(source, &owner));
                assert!(matches!(creator, Ok(CreatorExit::Produced)));
                assert!(std::ptr::eq(product, owner.shared.replies[0].product.get().unwrap()));
                assert!(std::ptr::eq(rows.prepared, identity.row));
                assert!(std::ptr::eq(rows.terminal, owner.writers[0].record().terminal().unwrap().as_ref().unwrap()));
                assert!(matches!(stop.observe(), StopObservation::Reply { original: Ok(Ok(serde_json::Value::Null)), prior_timeout: None }));
            }
            _ => panic!("actual no-seat writer was omitted from frozen records"),
        }
        assert!(facts.next().is_none());
    }
}
