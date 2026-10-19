//! Fixed original-attempt publication progress. The safe-std adapter is owned
//! source, but no whole-driver custody/settlement entry point is installed yet.
//! A fake adapter result is never evidence of real joins or durable storage.

use super::{ArchiveError, ArchiveMeasurement, ArchivePlan, EvidenceCoverage};
use crate::operation::Operation;

#[path = "archive_filesystem.rs"]
mod filesystem;

pub(crate) use filesystem::{
    JoinedArchiveSource, check_retained_publication_workspace, publish_joined_archive,
    verify_existing_joined_archive,
};

#[derive(Clone, Copy)]
pub(super) struct PublicationProgress {
    expected: Option<(EvidenceCoverage, ArchiveMeasurement)>,
    write_attempted: bool,
    temp_synced: bool,
    removal_attempted: bool,
    temp_removed: bool,
}

impl PublicationProgress {
    pub(super) const fn new() -> Self {
        Self {
            expected: None,
            write_attempted: false,
            temp_synced: false,
            removal_attempted: false,
            temp_removed: false,
        }
    }
}

/// Implemented by the trusted whole-driver adapter, with
/// actual original ALL-child/reader joins and containment before effects. This
/// private interface accepts neither a consumer ACK nor a replacement operation.
/// All path/library allocations and calls must use the original plan reserve.
/// write_temp MUST use create_new and streaming borrowed serialization once;
/// final_exists MUST distinguish absence from errors; verify methods MUST check
/// original header, complete length and hash, not merely file existence.
trait PublicationIo {
    fn preflight(&mut self) -> Result<(), ArchiveError>;
    fn final_exists(&mut self) -> Result<bool, ArchiveError>;
    fn write_temp(&mut self) -> Result<(), ArchiveError>;
    fn verify_temp(&mut self) -> Result<(), ArchiveError>;
    fn sync_temp(&mut self) -> Result<(), ArchiveError>;
    fn hard_link(&mut self) -> Result<(), ArchiveError>;
    fn remove_just_linked_temp(&mut self) -> Result<(), ArchiveError>;
    fn sync_final(&mut self) -> Result<(), ArchiveError>;
    fn verify_final(&mut self) -> Result<(), ArchiveError>;
}

/// Verification observation only. It releases no canonical evidence, prompt,
/// actor credit, journal liability or permit. Final-name crash durability is
/// deliberately limited by the lead's safe-std hard-link ruling.
#[derive(Debug, PartialEq, Eq)]
struct FileVerified {
    best_effort_sync_failed: bool,
    // A temp may exist even if the write/link callback returned an error before
    // its acknowledgment. Such leftovers are unaccepted and never auto-deleted.
    unaccepted_temp_possible: bool,
}

/// A private observation of this original attempt's current verified FINAL.
/// No path/raw content is exposed, no consumer can construct the observation,
/// and no canonical evidence/permit/prompt/actor/journal credit is released.
/// The safe-std ruling accepts limited final-name power-loss durability.
/// Retain the SAME instance handle so a returned observation is independent of
/// the caller's borrow. This copies no body, plan or workspace, renews no allowance,
/// performs no new allocation; its fixed storage is included before publication.
pub(crate) struct ArchiveFileObservation {
    original: Operation,
    coverage: EvidenceCoverage,
    measurement: ArchiveMeasurement,
    file: FileVerified,
}

impl ArchiveFileObservation {
    pub(crate) fn matches(
        &self,
        original: &Operation,
        coverage: EvidenceCoverage,
        measurement: ArchiveMeasurement,
    ) -> bool {
        self.original.same_instance(original)
            && self.coverage == coverage
            && self.measurement == measurement
    }

    pub(crate) fn best_effort_sync_failed(&self) -> bool {
        self.file.best_effort_sync_failed
    }

    pub(crate) fn unaccepted_temp_possible(&self) -> bool {
        self.file.unaccepted_temp_possible
    }
}

struct PublicationBorrow<'a> {
    plan: &'a ArchivePlan,
    progress: PublicationProgress,
}

impl ArchivePlan {
    fn borrow_existing_publication(
        &self,
        operation: &Operation,
        coverage: EvidenceCoverage,
        expected: ArchiveMeasurement,
    ) -> Result<PublicationBorrow<'_>, ArchiveError> {
        // Recovery can only inspect this already-written ORIGINAL attempt. It
        // never binds a fresh descriptor or renews permission to emit/link TEMP.
        self.header(operation, coverage)?;
        let mut slot = self
            .publication
            .lock()
            .map_err(|_| ArchiveError::Filesystem)?;
        let progress = slot.as_ref().ok_or(ArchiveError::Identity)?;
        if progress.expected != Some((coverage, expected)) || !progress.write_attempted {
            return Err(ArchiveError::Mismatch);
        }
        let progress = slot.take().ok_or(ArchiveError::Identity)?;
        drop(slot);
        Ok(PublicationBorrow {
            plan: self,
            progress,
        })
    }

    fn borrow_publication(
        &self,
        operation: &Operation,
        coverage: EvidenceCoverage,
        expected: ArchiveMeasurement,
    ) -> Result<PublicationBorrow<'_>, ArchiveError> {
        // Original INSTANCE and the fixed attempt header are checked before any
        // progress checkout or callback. Overflow/expiry does not erase cleanup.
        self.header(operation, coverage)?;
        let mut slot = self
            .publication
            .lock()
            .map_err(|_| ArchiveError::Filesystem)?;
        let progress = slot.as_mut().ok_or(ArchiveError::Identity)?;
        match progress.expected {
            Some(bound) if bound != (coverage, expected) => return Err(ArchiveError::Mismatch),
            Some(_) => {}
            None => progress.expected = Some((coverage, expected)),
        }
        let progress = slot.take().ok_or(ArchiveError::Identity)?;
        drop(slot);
        Ok(PublicationBorrow {
            plan: self,
            progress,
        })
    }
}

impl PublicationBorrow<'_> {
    fn verify_existing_final(
        &self,
        io: &mut impl PublicationIo,
    ) -> Result<FileVerified, ArchiveError> {
        // Also guard direct use of an ordinary progress borrow. A pre-write
        // borrow cannot manufacture a prior publication attempt.
        if self.progress.expected.is_none() || !self.progress.write_attempted {
            return Err(ArchiveError::Mismatch);
        }
        io.preflight()?;
        if !io.final_exists()? {
            return Err(ArchiveError::Mismatch);
        }
        // No TEMP callback or progress mutation, even when FINAL is absent.
        // Caller retains the original publication Err separately from this
        // current observation; file verification cannot rewrite that outcome.
        self.verify_final(io)
    }

    fn drive(&mut self, io: &mut impl PublicationIo) -> Result<FileVerified, ArchiveError> {
        io.preflight()?;
        if io.final_exists()? {
            // SAME-attempt lost acknowledgment: never write, copy, link or delete
            // a temp. A prior observation is not durable acceptance on this call.
            return self.verify_final(io);
        }
        if !self.progress.write_attempted {
            // Mark BEFORE invoking anything: partial write, cancellation or
            // unknown return cannot turn into permission to emit the body again.
            self.progress.write_attempted = true;
            io.write_temp()?;
        }
        // A complete temp left by a lost write ACK can still be verified/synced.
        // Partial/mismatched/missing temp refuses, with the original bytes owned.
        io.verify_temp()?;
        if !self.progress.temp_synced {
            io.sync_temp()?;
            self.progress.temp_synced = true;
        }
        io.preflight()?;
        // MUST be nonclobbering. AlreadyExists/error is not acceptance; the next
        // call observes and fully verifies the SAME final without rewriting it.
        io.hard_link()?;
        if !self.progress.removal_attempted {
            self.progress.removal_attempted = true;
            self.progress.temp_removed = io.remove_just_linked_temp().is_ok();
        }
        self.verify_final(io)
    }

    fn verify_final(&self, io: &mut impl PublicationIo) -> Result<FileVerified, ArchiveError> {
        io.preflight()?;
        // Metadata flush failure is the accepted residual risk, not permission
        // to skip opening/identity-length-hash verification of the final file.
        let best_effort_sync_failed = match io.sync_final() {
            Ok(()) => false,
            Err(ArchiveError::Filesystem) => true,
            // Best effort never excuses failed admission, identity or fit.
            Err(error) => return Err(error),
        };
        io.verify_final()?;
        Ok(FileVerified {
            best_effort_sync_failed,
            unaccepted_temp_possible: self.progress.write_attempted && !self.progress.temp_removed,
        })
    }
}

impl Drop for PublicationBorrow<'_> {
    fn drop(&mut self) {
        // No I/O/callback/await under this lock. Cancellation returns fixed SAME
        // progress; poison must not manufacture a fresh write allowance either.
        let mut slot = self
            .plan
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = Some(self.progress);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failed_evidence::tests::{coverage, measurement, verification_operation};

    // Private synthetic observations exercise ownership only. They are not
    // filesystem acceptance, actual task joins or custody/release witnesses.
    fn synthetic_observation(original: &Operation) -> ArchiveFileObservation {
        ArchiveFileObservation {
            original: original.clone(),
            coverage: coverage(),
            measurement: measurement(b"synthetic-original-body"),
            file: FileVerified {
                best_effort_sync_failed: true,
                unaccepted_temp_possible: true,
            },
        }
    }

    #[test]
    fn returned_observation_moves_without_borrow_or_identity_retagging() {
        fn requires_send<T: Send + 'static>() {}
        requires_send::<ArchiveFileObservation>(); // No worker/thread is launched.
        let original = verification_operation();
        let replacement = verification_operation(); // Same visible ID, distinct instance.
        let charged = original.captured_bytes();
        let observation = {
            let temporary_handle = original.clone();
            synthetic_observation(&temporary_handle)
        }; // The argument's handle and borrow have ended before the result is moved.
        let mut retained = Some(observation);
        assert!(retained.as_ref().unwrap().matches(
            &original,
            coverage(),
            measurement(b"synthetic-original-body")
        ));
        let observation = retained.take().unwrap(); // Synchronous MOVE, no body copy.
        assert!(retained.is_none());
        assert!(!observation.matches(
            &replacement,
            coverage(),
            measurement(b"synthetic-original-body")
        ));
        assert!(!observation.matches(&original, coverage(), measurement(b"changed-body")));
        assert!(!observation.matches(
            &original,
            EvidenceCoverage::new(0x3f, 7, [0x5a; 32]).unwrap(),
            measurement(b"synthetic-original-body")
        ));
        assert!(observation.best_effort_sync_failed());
        assert!(observation.unaccepted_temp_possible());
        assert_eq!(
            observation.original.allowance().deadline(),
            original.allowance().deadline()
        );
        assert_eq!(original.captured_bytes(), charged);
        assert!(original.borrow_archive_chunk(&original).is_ok());
    }

    #[test]
    fn observation_move_and_drop_preserve_original_overflow_credit_and_attempt() {
        let original = verification_operation();
        let observation = synthetic_observation(&original);
        let charged = original.captured_bytes();
        let header = original
            .archive_plan()
            .unwrap()
            .header(&original, coverage())
            .unwrap();
        original.overflow();
        assert_eq!(
            observation.original.check(),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert!(observation.matches(
            &original,
            coverage(),
            measurement(b"synthetic-original-body")
        ));
        drop(observation); // Drops one handle only, not an operation settlement or refund.
        assert_eq!(
            original.check(),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert_eq!(original.captured_bytes(), charged);
        assert!(original.borrow_archive_chunk(&original).is_ok());
        let current = original
            .archive_plan()
            .unwrap()
            .header(&original, coverage())
            .unwrap();
        assert_eq!(&header.bytes[..header.len], &current.bytes[..current.len]);
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Step {
        Preflight,
        Exists,
        Write,
        VerifyTemp,
        SyncTemp,
        Link,
        Remove,
        SyncFinal,
        VerifyFinal,
    }

    struct FakeIo {
        log: [Option<Step>; 40],
        count: usize,
        fail: Option<Step>,
        fail_fit: bool,
        final_exists: bool,
        temp_complete: bool,
        // Effect can happen before a failed/lost callback acknowledgment.
        lose_link_ack: bool,
        lose_write_ack: bool,
    }

    impl FakeIo {
        fn new() -> Self {
            Self {
                log: [None; 40],
                count: 0,
                fail: None,
                fail_fit: false,
                final_exists: false,
                temp_complete: false,
                lose_link_ack: false,
                lose_write_ack: false,
            }
        }
        fn step(&mut self, step: Step) -> Result<(), ArchiveError> {
            self.log[self.count] = Some(step);
            self.count += 1;
            if self.fail == Some(step) {
                Err(if self.fail_fit {
                    ArchiveError::Fit
                } else {
                    ArchiveError::Filesystem
                })
            } else {
                Ok(())
            }
        }
        fn occurrences(&self, step: Step) -> usize {
            self.log[..self.count]
                .iter()
                .filter(|item| **item == Some(step))
                .count()
        }
    }

    impl PublicationIo for FakeIo {
        fn preflight(&mut self) -> Result<(), ArchiveError> {
            self.step(Step::Preflight)
        }
        fn final_exists(&mut self) -> Result<bool, ArchiveError> {
            self.step(Step::Exists)?;
            Ok(self.final_exists)
        }
        fn write_temp(&mut self) -> Result<(), ArchiveError> {
            self.step(Step::Write)?;
            self.temp_complete = true;
            if self.lose_write_ack {
                Err(ArchiveError::Filesystem)
            } else {
                Ok(())
            }
        }
        fn verify_temp(&mut self) -> Result<(), ArchiveError> {
            self.step(Step::VerifyTemp)?;
            if self.temp_complete {
                Ok(())
            } else {
                Err(ArchiveError::Mismatch)
            }
        }
        fn sync_temp(&mut self) -> Result<(), ArchiveError> {
            self.step(Step::SyncTemp)
        }
        fn hard_link(&mut self) -> Result<(), ArchiveError> {
            self.step(Step::Link)?;
            assert!(!self.final_exists, "nonclobber link fixture");
            self.final_exists = true;
            if self.lose_link_ack {
                Err(ArchiveError::Filesystem)
            } else {
                Ok(())
            }
        }
        fn remove_just_linked_temp(&mut self) -> Result<(), ArchiveError> {
            self.step(Step::Remove)?;
            self.temp_complete = false;
            Ok(())
        }
        fn sync_final(&mut self) -> Result<(), ArchiveError> {
            self.step(Step::SyncFinal)
        }
        fn verify_final(&mut self) -> Result<(), ArchiveError> {
            self.step(Step::VerifyFinal)?;
            if self.final_exists {
                Ok(())
            } else {
                Err(ArchiveError::Mismatch)
            }
        }
    }

    #[test]
    fn existing_final_verification_retains_lost_link_error_and_never_resumes_temp() {
        let original = verification_operation();
        let plan = original.archive_plan().unwrap();
        let expected = measurement(b"fixture");
        let mut io = FakeIo::new();
        io.lose_link_ack = true;
        let original_error = {
            let mut attempt = plan
                .borrow_publication(&original, coverage(), expected)
                .unwrap();
            attempt.drive(&mut io).unwrap_err()
        };
        original.overflow();
        let charged = original.captured_bytes();
        let before = io.count;
        {
            let attempt = plan
                .borrow_existing_publication(&original, coverage(), expected)
                .unwrap();
            assert!(
                attempt
                    .verify_existing_final(&mut io)
                    .unwrap()
                    .unaccepted_temp_possible
            );
        }
        assert!(matches!(original_error, ArchiveError::Filesystem));
        assert_eq!(
            &io.log[before..io.count],
            &[
                Some(Step::Preflight),
                Some(Step::Exists),
                Some(Step::Preflight),
                Some(Step::SyncFinal),
                Some(Step::VerifyFinal),
            ]
        );
        assert_eq!(io.occurrences(Step::Write), 1);
        assert_eq!(io.occurrences(Step::VerifyTemp), 1);
        assert_eq!(io.occurrences(Step::SyncTemp), 1);
        assert_eq!(io.occurrences(Step::Link), 1);
        assert_eq!(io.occurrences(Step::Remove), 0);
        assert!(io.temp_complete); // Unaccepted leftover is not removed by recovery.
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(
            original.check(),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        let progress = plan.publication.lock().unwrap();
        let progress = progress.as_ref().unwrap();
        assert_eq!(progress.expected, Some((coverage(), expected)));
        assert!(progress.write_attempted && progress.temp_synced);
        assert!(!progress.removal_attempted && !progress.temp_removed);
    }

    #[test]
    fn existing_final_absence_never_promotes_complete_unaccepted_temp() {
        let original = verification_operation();
        let plan = original.archive_plan().unwrap();
        let expected = measurement(b"fixture");
        let mut io = FakeIo::new();
        io.lose_write_ack = true;
        let original_error = {
            let mut attempt = plan
                .borrow_publication(&original, coverage(), expected)
                .unwrap();
            attempt.drive(&mut io).unwrap_err()
        };
        let before = io.count;
        for _ in 0..2 {
            let attempt = plan
                .borrow_existing_publication(&original, coverage(), expected)
                .unwrap();
            assert!(matches!(
                attempt.verify_existing_final(&mut io),
                Err(ArchiveError::Mismatch)
            ));
        }
        assert!(matches!(original_error, ArchiveError::Filesystem));
        assert_eq!(
            &io.log[before..io.count],
            &[
                Some(Step::Preflight),
                Some(Step::Exists),
                Some(Step::Preflight),
                Some(Step::Exists),
            ]
        );
        assert!(io.temp_complete && !io.final_exists);
        assert_eq!(io.occurrences(Step::Write), 1);
        assert_eq!(io.occurrences(Step::VerifyTemp), 0);
        assert_eq!(io.occurrences(Step::SyncTemp), 0);
        assert_eq!(io.occurrences(Step::Link), 0);
        assert_eq!(io.occurrences(Step::Remove), 0);
        assert_eq!(io.occurrences(Step::VerifyFinal), 0);
        let progress = plan.publication.lock().unwrap();
        let progress = progress.as_ref().unwrap();
        assert_eq!(progress.expected, Some((coverage(), expected)));
        assert!(progress.write_attempted);
        assert!(!progress.temp_synced && !progress.removal_attempted);
    }

    #[test]
    fn existing_final_recovery_refuses_unbound_unwritten_foreign_and_changed_attempts() {
        let original = verification_operation();
        let foreign = verification_operation();
        let plan = original.archive_plan().unwrap();
        let expected = measurement(b"fixture");
        let charged = original.captured_bytes();
        assert!(matches!(
            plan.borrow_existing_publication(&original, coverage(), expected),
            Err(ArchiveError::Mismatch)
        ));
        assert!(
            plan.publication
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .expected
                .is_none()
        );
        let mut io = FakeIo::new();
        io.final_exists = true; // Existence alone is not prior original-attempt provenance.
        {
            let ordinary = plan
                .borrow_publication(&original, coverage(), expected)
                .unwrap();
            assert!(matches!(
                ordinary.verify_existing_final(&mut io),
                Err(ArchiveError::Mismatch)
            ));
            assert!(matches!(
                plan.borrow_existing_publication(&original, coverage(), expected),
                Err(ArchiveError::Identity)
            ));
        }
        assert_eq!(io.count, 0);
        assert!(matches!(
            plan.borrow_existing_publication(&original, coverage(), expected),
            Err(ArchiveError::Mismatch)
        ));
        // Synthetic failed write latches only the original write obligation.
        io.final_exists = false;
        io.fail = Some(Step::Write);
        {
            let mut ordinary = plan
                .borrow_publication(&original, coverage(), expected)
                .unwrap();
            assert!(ordinary.drive(&mut io).is_err());
        }
        let before = io.count;
        assert!(matches!(
            plan.borrow_existing_publication(&foreign, coverage(), expected),
            Err(ArchiveError::Identity)
        ));
        assert!(matches!(
            plan.borrow_existing_publication(
                &original,
                EvidenceCoverage::new(1, 1, [0; 32]).unwrap(),
                expected
            ),
            Err(ArchiveError::Mismatch)
        ));
        assert!(matches!(
            plan.borrow_existing_publication(&original, coverage(), measurement(b"changed")),
            Err(ArchiveError::Mismatch)
        ));
        assert!(matches!(
            plan.borrow_existing_publication(&original, coverage(), measurement(b"fixturE")),
            Err(ArchiveError::Mismatch)
        ));
        assert_eq!(io.count, before);
        let attempt = plan
            .borrow_existing_publication(&original, coverage(), expected)
            .unwrap();
        assert!(matches!(
            plan.borrow_existing_publication(&original, coverage(), expected),
            Err(ArchiveError::Identity)
        ));
        drop(attempt); // Returns SAME fixed binding, not an unbound replacement.
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(
            plan.publication.lock().unwrap().as_ref().unwrap().expected,
            Some((coverage(), expected))
        );
    }

    #[test]
    fn existing_final_best_effort_flush_never_excuses_fit_or_verification_failure() {
        let original = verification_operation();
        let plan = original.archive_plan().unwrap();
        let expected = measurement(b"fixture");
        let mut io = FakeIo::new();
        io.lose_link_ack = true;
        {
            let mut attempt = plan
                .borrow_publication(&original, coverage(), expected)
                .unwrap();
            assert!(attempt.drive(&mut io).is_err());
        }
        let attempt = plan
            .borrow_existing_publication(&original, coverage(), expected)
            .unwrap();
        io.fail = Some(Step::SyncFinal);
        assert!(
            attempt
                .verify_existing_final(&mut io)
                .unwrap()
                .best_effort_sync_failed
        );
        let verified = io.occurrences(Step::VerifyFinal);
        io.fail_fit = true;
        assert!(matches!(
            attempt.verify_existing_final(&mut io),
            Err(ArchiveError::Fit)
        ));
        assert_eq!(io.occurrences(Step::VerifyFinal), verified);
        io.fail = Some(Step::VerifyFinal);
        io.fail_fit = false;
        assert!(matches!(
            attempt.verify_existing_final(&mut io),
            Err(ArchiveError::Filesystem)
        ));
        assert_eq!(io.occurrences(Step::Write), 1);
        assert_eq!(io.occurrences(Step::Link), 1);
        assert_eq!(io.occurrences(Step::Remove), 0);
    }

    #[test]
    fn fixed_publication_order_syncs_temp_before_nonclobber_and_verifies_after_flush() {
        let operation = verification_operation();
        let plan = operation.archive_plan().unwrap();
        let mut attempt = plan
            .borrow_publication(&operation, coverage(), measurement(b"fixture"))
            .unwrap();
        let mut io = FakeIo::new();
        assert_eq!(
            attempt.drive(&mut io).unwrap(),
            FileVerified {
                best_effort_sync_failed: false,
                unaccepted_temp_possible: false,
            }
        );
        assert_eq!(
            &io.log[..io.count],
            &[
                Some(Step::Preflight),
                Some(Step::Exists),
                Some(Step::Write),
                Some(Step::VerifyTemp),
                Some(Step::SyncTemp),
                Some(Step::Preflight),
                Some(Step::Link),
                Some(Step::Remove),
                Some(Step::Preflight),
                Some(Step::SyncFinal),
                Some(Step::VerifyFinal),
            ]
        );
    }

    #[test]
    fn dropped_borrow_and_failed_partial_write_never_renew_emission() {
        let operation = verification_operation();
        let plan = operation.archive_plan().unwrap();
        let expected = measurement(b"fixture");
        let mut io = FakeIo::new();
        io.fail = Some(Step::Write);
        {
            let mut attempt = plan
                .borrow_publication(&operation, coverage(), expected)
                .unwrap();
            assert!(
                plan.borrow_publication(&operation, coverage(), expected)
                    .is_err()
            );
            assert!(attempt.drive(&mut io).is_err());
        }
        io.fail = None;
        let mut attempt = plan
            .borrow_publication(&operation, coverage(), expected)
            .unwrap();
        assert!(matches!(
            attempt.drive(&mut io),
            Err(ArchiveError::Mismatch)
        ));
        assert_eq!(io.occurrences(Step::Write), 1);
        assert_eq!(io.occurrences(Step::SyncTemp), 0);
        assert_eq!(io.occurrences(Step::Link), 0);
    }

    #[test]
    fn lost_link_ack_checks_same_final_without_copy_or_temp_deletion() {
        let operation = verification_operation();
        let plan = operation.archive_plan().unwrap();
        let expected = measurement(b"fixture");
        let mut io = FakeIo::new();
        io.lose_link_ack = true;
        {
            let mut attempt = plan
                .borrow_publication(&operation, coverage(), expected)
                .unwrap();
            assert!(attempt.drive(&mut io).is_err());
        }
        let mut attempt = plan
            .borrow_publication(&operation, coverage(), expected)
            .unwrap();
        assert!(attempt.drive(&mut io).unwrap().unaccepted_temp_possible);
        assert!(attempt.drive(&mut io).unwrap().unaccepted_temp_possible);
        assert_eq!(io.occurrences(Step::Write), 1);
        assert_eq!(io.occurrences(Step::Link), 1);
        assert_eq!(io.occurrences(Step::Remove), 0);
        assert_eq!(io.occurrences(Step::VerifyFinal), 2);
    }

    #[test]
    fn lost_complete_write_ack_resumes_verification_and_sync_without_rewrite() {
        let operation = verification_operation();
        let plan = operation.archive_plan().unwrap();
        let expected = measurement(b"fixture");
        let mut io = FakeIo::new();
        io.lose_write_ack = true;
        {
            let mut attempt = plan
                .borrow_publication(&operation, coverage(), expected)
                .unwrap();
            assert!(attempt.drive(&mut io).is_err());
        }
        let mut attempt = plan
            .borrow_publication(&operation, coverage(), expected)
            .unwrap();
        attempt.drive(&mut io).unwrap();
        assert_eq!(io.occurrences(Step::Write), 1);
        assert_eq!(io.occurrences(Step::VerifyTemp), 1);
        assert_eq!(io.occurrences(Step::SyncTemp), 1);
        assert_eq!(io.occurrences(Step::Link), 1);
    }

    #[test]
    fn failed_temp_removal_is_unaccepted_leftover_and_never_auto_retried() {
        let operation = verification_operation();
        let plan = operation.archive_plan().unwrap();
        let mut attempt = plan
            .borrow_publication(&operation, coverage(), measurement(b"fixture"))
            .unwrap();
        let mut io = FakeIo::new();
        io.fail = Some(Step::Remove);
        assert!(attempt.drive(&mut io).unwrap().unaccepted_temp_possible);
        io.fail = Some(Step::SyncFinal);
        let verified = attempt.drive(&mut io).unwrap();
        assert!(verified.best_effort_sync_failed);
        assert!(verified.unaccepted_temp_possible);
        assert_eq!(io.occurrences(Step::Remove), 1);
        io.fail = Some(Step::VerifyFinal);
        assert!(attempt.drive(&mut io).is_err());
    }

    #[test]
    fn original_instance_and_fixed_measurement_bind_before_any_callback() {
        let operation = verification_operation();
        let replacement = verification_operation();
        let plan = operation.archive_plan().unwrap();
        let expected = measurement(b"fixture");
        assert!(matches!(
            plan.borrow_publication(&replacement, coverage(), expected),
            Err(ArchiveError::Identity)
        ));
        drop(
            plan.borrow_publication(&operation, coverage(), expected)
                .unwrap(),
        );
        assert!(matches!(
            plan.borrow_publication(&operation, coverage(), measurement(b"replacement")),
            Err(ArchiveError::Mismatch)
        ));
        let different = EvidenceCoverage::new(1, 1, [0; 32]).unwrap();
        assert!(matches!(
            plan.borrow_publication(&operation, different, expected),
            Err(ArchiveError::Mismatch)
        ));
        operation
            .charge(crate::operation::OPERATION_BYTES)
            .unwrap_err();
        let charged = operation.captured_bytes();
        let mut io = FakeIo::new();
        io.final_exists = true;
        let mut attempt = plan
            .borrow_publication(&operation, coverage(), expected)
            .unwrap();
        attempt.drive(&mut io).unwrap();
        assert_eq!(io.occurrences(Step::Write), 0);
        assert_eq!(operation.captured_bytes(), charged);
        let verified_count = io.occurrences(Step::VerifyFinal);
        io.fail = Some(Step::SyncFinal);
        io.fail_fit = true;
        assert!(matches!(attempt.drive(&mut io), Err(ArchiveError::Fit)));
        assert_eq!(io.occurrences(Step::VerifyFinal), verified_count);
        let fixed = std::mem::size_of::<PublicationProgress>()
            + std::mem::size_of::<PublicationBorrow<'_>>()
            + std::mem::size_of::<FileVerified>()
            + std::mem::size_of::<ArchiveFileObservation>();
        assert!(fixed < super::super::FIXED_ARCHIVE_WORKSPACE);
    }
}
