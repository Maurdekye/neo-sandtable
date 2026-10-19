//! Safe-std path adapter for the SAME original attempt. There is deliberately
//! no public constructor or whole-driver custody/healthy-settlement entry point.
//! Original-driver enumeration is separate from publication/custody wiring.

use std::{
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::Path,
};

use super::{ArchiveError, ArchiveMeasurement, ArchivePlan, EvidenceCoverage, PublicationIo};
use crate::{failed_evidence::ArchiveEncoder, operation::Operation};

/// The trusted original-driver implementation must check ALL original
/// session/helper children and ALL reader joins, original INSTANCE/physical
/// association and complete canonical coverage. The source is borrowed, never
/// cloned or moved into another owner here. A test implementation is not a join
/// witness. Implementations must preflight the entire set before writing a header.
pub(crate) trait JoinedArchiveSource {
    fn check_original_supervision(&mut self, original: &Operation) -> Result<(), ArchiveError>;

    fn stream<W: Write>(
        &mut self,
        original: &Operation,
        coverage: EvidenceCoverage,
        output: &mut W,
    ) -> Result<ArchiveMeasurement, ArchiveError>;
}

pub(crate) fn check_retained_publication_workspace<S: JoinedArchiveSource>(
    retained_bytes: usize,
) -> Result<(), ArchiveError> {
    StdArchiveIo::<S>::check_fixed_workspace_with(retained_bytes)
}

/// Assemble the actual source with the SAME original publication attempt.
/// Measurement streams into fixed hash state, never a body buffer. ALL source
/// gates run before any path effect/progress checkout; an already-bound attempt
/// then refuses a changed coverage/body rather than rewriting evidence.
///
/// This synchronous private operation must not be called from operator stop or
/// the async supervisor until its blocking-I/O responsiveness is established.
/// Preparing the source grants no runtime/archive-I/O permission. Its result is
/// a file observation, not a custody clear or healthy aggregate settlement.
pub(crate) fn publish_joined_archive<S: JoinedArchiveSource>(
    original: &Operation,
    coverage: EvidenceCoverage,
    source: &mut S,
) -> Result<super::ArchiveFileObservation, ArchiveError> {
    let plan = original.archive_plan().ok_or(ArchiveError::Identity)?;
    plan.header(original, coverage)?;
    StdArchiveIo::<S>::check_fixed_workspace()?;
    source.check_original_supervision(original)?;
    let mut meter = crate::failed_evidence::ArchiveMeter::new();
    let streamed = source.stream(original, coverage, &mut meter)?;
    let expected = meter.finish()?;
    if streamed != expected {
        return Err(ArchiveError::Mismatch);
    }
    let mut io = StdArchiveIo::new(original, coverage, expected, source)?;
    let mut progress = plan.borrow_publication(original, coverage, expected)?;
    let file = progress.drive(&mut io)?;
    Ok(super::ArchiveFileObservation {
        original: original.clone(),
        coverage,
        measurement: expected,
        file,
    })
}

/// Reobserve only an existing FINAL from the already-bound, written ORIGINAL
/// attempt. Borrowed whole-source measurement must still match that binding.
/// Missing FINAL refuses even if a complete TEMP remains; no emission, link,
/// copy or TEMP deletion is reachable here. The caller's original publication
/// error and this file observation have separate lifetimes and meanings.
///
/// SOURCE ONLY/unwired: this synchronous helper has the same blocking-I/O/full
/// fit admission obligations as publication. Never call on async/operator stop.
pub(crate) fn verify_existing_joined_archive<S: JoinedArchiveSource>(
    original: &Operation,
    coverage: EvidenceCoverage,
    source: &mut S,
) -> Result<super::ArchiveFileObservation, ArchiveError> {
    let plan = original.archive_plan().ok_or(ArchiveError::Identity)?;
    plan.header(original, coverage)?;
    StdArchiveIo::<S>::check_fixed_workspace()?;
    source.check_original_supervision(original)?;
    let mut meter = crate::failed_evidence::ArchiveMeter::new();
    let streamed = source.stream(original, coverage, &mut meter)?;
    let expected = meter.finish()?;
    if streamed != expected {
        return Err(ArchiveError::Mismatch);
    }
    // Verify prior binding BEFORE any std path effect, with no rebinding.
    let progress = plan.borrow_existing_publication(original, coverage, expected)?;
    let mut io = StdArchiveIo::new(original, coverage, expected, source)?;
    let file = progress.verify_existing_final(&mut io)?;
    Ok(super::ArchiveFileObservation {
        original: original.clone(),
        coverage,
        measurement: expected,
        file,
    })
}

struct StdArchiveIo<'a, S> {
    plan: &'a ArchivePlan,
    original: &'a Operation,
    source: &'a mut S,
    coverage: EvidenceCoverage,
    expected: ArchiveMeasurement,
    // A failed callback leaves this handle and the original temp untouched until
    // the adapter borrow ends. Closing it transfers no evidence or liability.
    temp: Option<File>,
}

impl<'a, S: JoinedArchiveSource> StdArchiveIo<'a, S> {
    fn check_fixed_workspace() -> Result<(), ArchiveError> {
        Self::check_fixed_workspace_with(0)
    }

    fn check_fixed_workspace_with(retained_bytes: usize) -> Result<(), ArchiveError> {
        // Conservatively sum our fixed states for measurement and file passes.
        // Variable captures and std/serializer/task allocation are NOT justified
        // by this inline-size check; complete fit remains separately required.
        let own_fixed = std::mem::size_of::<Self>()
            + std::mem::size_of::<ArchiveEncoder<'_, File>>()
            + std::mem::size_of::<ArchiveEncoder<'_, crate::failed_evidence::ArchiveMeter>>()
            + std::mem::size_of::<crate::failed_evidence::EvidenceCoverageBuilder<'_>>()
            + std::mem::size_of::<super::PublicationBorrow<'_>>()
            + std::mem::size_of::<crate::failed_evidence::ArchiveMeter>()
            + std::mem::size_of::<super::ArchiveFileObservation>()
            + super::shape_workspace_bytes()
            + 512;
        if own_fixed
            .checked_add(retained_bytes)
            .is_none_or(|bytes| bytes > crate::failed_evidence::FIXED_ARCHIVE_WORKSPACE)
        {
            return Err(ArchiveError::Fit);
        }
        Ok(())
    }

    fn new(
        original: &'a Operation,
        coverage: EvidenceCoverage,
        expected: ArchiveMeasurement,
        source: &'a mut S,
    ) -> Result<Self, ArchiveError> {
        let plan = original.archive_plan().ok_or(ArchiveError::Identity)?;
        plan.header(original, coverage)?;
        Self::check_fixed_workspace()?;
        Ok(Self {
            plan,
            original,
            source,
            coverage,
            expected,
            temp: None,
        })
    }

    fn check_source(&mut self) -> Result<(), ArchiveError> {
        self.plan.header(self.original, self.coverage)?;
        self.source.check_original_supervision(self.original)
    }

    fn verify_path(&mut self, path: &Path, directory: bool) -> Result<(), ArchiveError> {
        self.check_source()?;
        self.plan.verify_ancestors(path, directory)?;
        Ok(())
    }

    fn regular_file(&self, file: &File, path: &Path) -> Result<(), ArchiveError> {
        // No path conversion is necessary for handle metadata, but conservatively
        // consume one original call/error allowance as well. No renewed reserve.
        self.plan.reserve_std_call(path, false)?;
        let metadata = file.metadata().map_err(|_| ArchiveError::Filesystem)?;
        if !metadata.is_file() || crate::failed_evidence::forbidden_link(&metadata) {
            return Err(ArchiveError::Path);
        }
        if metadata.len() != self.expected.length {
            return Err(ArchiveError::Mismatch);
        }
        Ok(())
    }

    fn open_temp(&mut self) -> Result<(), ArchiveError> {
        let plan = self.plan;
        let path = plan.temp.as_path();
        self.verify_path(path, false)?;
        if self.temp.is_none() {
            self.plan.reserve_std_call(path, false)?;
            // Reopen only for verification/sync after a lost write ACK. There is
            // no append/truncate/create permission on this cleanup path.
            self.temp = Some(
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .map_err(|_| ArchiveError::Filesystem)?,
            );
        }
        Ok(())
    }

    fn open_final(&mut self, sync_access: bool) -> Result<File, ArchiveError> {
        let plan = self.plan;
        let path = plan.final_path.as_path();
        self.verify_path(path, false)?;
        self.plan.reserve_std_call(path, false)?;
        OpenOptions::new()
            .read(true)
            // Windows FlushFileBuffers requires write access. This opens an
            // existing name only, with no create/truncate or body-writing path.
            .write(sync_access)
            .open(path)
            .map_err(|_| ArchiveError::Filesystem)
    }
}

impl<S: JoinedArchiveSource> PublicationIo for StdArchiveIo<'_, S> {
    fn preflight(&mut self) -> Result<(), ArchiveError> {
        self.check_source()?;
        self.plan.verify_containment()
    }

    fn final_exists(&mut self) -> Result<bool, ArchiveError> {
        let plan = self.plan;
        let path = plan.final_path.as_path();
        self.verify_path(path, false)?;
        self.plan.reserve_std_call(path, false)?;
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                if !metadata.is_file() || crate::failed_evidence::forbidden_link(&metadata) {
                    return Err(ArchiveError::Path);
                }
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(ArchiveError::Filesystem),
        }
    }

    fn write_temp(&mut self) -> Result<(), ArchiveError> {
        self.check_source()?;
        self.plan.verify_containment()?;
        self.plan.reserve_std_call(&self.plan.folder, false)?;
        match std::fs::create_dir(&self.plan.folder) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(ArchiveError::Filesystem),
        }
        // Existing/new folder must be a real resolved directory, not an alias,
        // file or junction. These observations are not a permanent race fence.
        self.plan.verify_ancestors(&self.plan.folder, true)?;
        self.plan.verify_ancestors(&self.plan.temp, false)?;
        self.plan.reserve_std_call(&self.plan.temp, false)?;
        self.temp = Some(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&self.plan.temp)
                .map_err(|_| ArchiveError::Filesystem)?,
        );
        // Borrowed source serialization must return the exact previously measured
        // attempt. A mismatch/error keeps the temp and canonical evidence; the
        // publication state has already made the write attempt sticky.
        let actual = self.source.stream(
            self.original,
            self.coverage,
            self.temp.as_mut().ok_or(ArchiveError::Filesystem)?,
        )?;
        if actual != self.expected {
            return Err(ArchiveError::Mismatch);
        }
        Ok(())
    }

    fn verify_temp(&mut self) -> Result<(), ArchiveError> {
        self.open_temp()?;
        let file = self.temp.as_ref().ok_or(ArchiveError::Filesystem)?;
        self.regular_file(file, &self.plan.temp)?;
        self.plan.reserve_std_call(&self.plan.temp, false)?;
        let file = self.temp.as_mut().ok_or(ArchiveError::Filesystem)?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| ArchiveError::Filesystem)?;
        self.plan
            .verify_stream(self.original, self.coverage, self.expected, file)
    }

    fn sync_temp(&mut self) -> Result<(), ArchiveError> {
        self.open_temp()?;
        self.plan.reserve_std_call(&self.plan.temp, false)?;
        self.temp
            .as_ref()
            .ok_or(ArchiveError::Filesystem)?
            .sync_all()
            .map_err(|_| ArchiveError::Filesystem)
    }

    fn hard_link(&mut self) -> Result<(), ArchiveError> {
        self.check_source()?;
        // Reopen the CURRENT temp name, rather than treating the earlier write
        // handle as identity of a path that an operator could have replaced.
        drop(self.temp.take());
        self.verify_temp()?;
        self.sync_temp()?;
        self.plan.verify_ancestors(&self.plan.temp, false)?;
        self.plan.verify_ancestors(&self.plan.final_path, false)?;
        // Both source and destination conversions are admitted BEFORE this one
        // nonclobbering syscall. Unsupported filesystems/AlreadyExists refuse.
        self.plan.reserve_std_call(&self.plan.temp, false)?;
        self.plan.reserve_std_call(&self.plan.final_path, false)?;
        std::fs::hard_link(&self.plan.temp, &self.plan.final_path)
            .map_err(|_| ArchiveError::Filesystem)
    }

    fn remove_just_linked_temp(&mut self) -> Result<(), ArchiveError> {
        self.check_source()?;
        // Only a freshly compared SAME-attempt temp is eligible for this one
        // immediate cleanup. A replaced/mismatched name remains unaccepted.
        drop(self.temp.take());
        self.verify_temp()?;
        self.plan.verify_ancestors(&self.plan.temp, false)?;
        self.plan.reserve_std_call(&self.plan.temp, false)?;
        // Drop only the archive's auxiliary handle, never canonical raw evidence.
        // The progress machine permits this call only once after a new link ACK.
        drop(self.temp.take());
        std::fs::remove_file(&self.plan.temp).map_err(|_| ArchiveError::Filesystem)
    }

    fn sync_final(&mut self) -> Result<(), ArchiveError> {
        let mut file = self.open_final(true)?;
        self.regular_file(&file, &self.plan.final_path)?;
        self.plan
            .verify_stream(self.original, self.coverage, self.expected, &mut file)?;
        self.plan.reserve_std_call(&self.plan.final_path, false)?;
        file.sync_all().map_err(|_| ArchiveError::Filesystem)
    }

    fn verify_final(&mut self) -> Result<(), ArchiveError> {
        // Open the current FINAL name afresh after the best-effort sync. A prior
        // handle observation is not proof that the current name still matches.
        let mut file = self.open_final(false)?;
        self.regular_file(&file, &self.plan.final_path)?;
        self.plan
            .verify_stream(self.original, self.coverage, self.expected, &mut file)?;
        self.plan.verify_ancestors(&self.plan.final_path, false)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        failed_evidence::tests::{
            context, coverage, filesystem_fixture, fixture_bytes, measurement,
            verification_operation,
        },
        operation::{Allowance, OPERATION_BYTES},
    };

    // Synthetic fixture source ONLY. True here is never a production child/readers
    // witness or complete evidence set, and fixture Vecs are not fit evidence.
    struct Source<'a> {
        original: &'a Operation,
        bytes: &'a [u8],
        joined: bool,
        partial: bool,
        writes: usize,
    }

    impl JoinedArchiveSource for Source<'_> {
        fn check_original_supervision(&mut self, original: &Operation) -> Result<(), ArchiveError> {
            if !self.joined || !self.original.same_instance(original) {
                return Err(ArchiveError::Identity);
            }
            Ok(())
        }

        fn stream<W: Write>(
            &mut self,
            original: &Operation,
            _coverage: EvidenceCoverage,
            output: &mut W,
        ) -> Result<ArchiveMeasurement, ArchiveError> {
            self.check_original_supervision(original)?;
            self.writes += 1;
            let bytes = if self.partial {
                &self.bytes[..self.bytes.len() / 2]
            } else {
                self.bytes
            };
            output
                .write_all(bytes)
                .map_err(|_| ArchiveError::Filesystem)?;
            if self.partial {
                Err(ArchiveError::Filesystem)
            } else {
                Ok(measurement(bytes))
            }
        }
    }

    fn source<'a>(original: &'a Operation, bytes: &'a [u8]) -> Source<'a> {
        Source {
            original,
            bytes,
            joined: true,
            partial: false,
            writes: 0,
        }
    }

    fn filesystem_operation(root: &Path) -> Operation {
        context(root)
            .operation(
                "axis.commander".parse().unwrap(),
                7,
                "22222222-2222-4222-8222-222222222222".into(),
                Allowance::required(Some(
                    tokio::time::Instant::now() + std::time::Duration::from_secs(10),
                ))
                .unwrap(),
            )
            .unwrap()
    }

    #[test]
    fn assembled_publication_measurement_refusal_has_no_path_or_progress_effect() {
        struct WrongMeasurement<'a>(Source<'a>);
        impl JoinedArchiveSource for WrongMeasurement<'_> {
            fn check_original_supervision(
                &mut self,
                original: &Operation,
            ) -> Result<(), ArchiveError> {
                self.0.check_original_supervision(original)
            }
            fn stream<W: Write>(
                &mut self,
                original: &Operation,
                covered: EvidenceCoverage,
                output: &mut W,
            ) -> Result<ArchiveMeasurement, ArchiveError> {
                self.0.stream(original, covered, output)?;
                Ok(measurement(b"wrong measurement"))
            }
        }
        let original = verification_operation();
        let plan = original.archive_plan().unwrap();
        let bytes = fixture_bytes(&original);
        let pointer = bytes.as_ptr();
        let mut source = WrongMeasurement(source(&original, &bytes));
        let calls = plan.io.lock().unwrap().remaining;
        source.0.joined = false;
        assert!(matches!(
            publish_joined_archive(&original, coverage(), &mut source),
            Err(ArchiveError::Identity)
        ));
        assert_eq!(source.0.writes, 0);
        assert_eq!(plan.io.lock().unwrap().remaining, calls);
        source.0.joined = true;
        assert!(matches!(
            publish_joined_archive(&original, coverage(), &mut source),
            Err(ArchiveError::Mismatch)
        ));
        assert_eq!(source.0.writes, 1); // Fixed measurement sink only; no file body.
        assert_eq!(source.0.bytes.as_ptr(), pointer);
        assert_eq!(plan.io.lock().unwrap().remaining, calls);
        assert!(
            plan.publication
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .expected
                .is_none()
        );
    }

    // SOURCE ONLY/UNRUN: no fixture/filesystem effects occur by defining this test.
    #[test]
    fn existing_final_source_checks_never_bind_unstarted_progress_or_touch_paths() {
        let original = verification_operation();
        let foreign = verification_operation();
        let plan = original.archive_plan().unwrap();
        let bytes = fixture_bytes(&original);
        let pointer = bytes.as_ptr();
        let mut source = source(&original, &bytes);
        let calls = plan.io.lock().unwrap().remaining;
        source.joined = false;
        assert!(matches!(
            verify_existing_joined_archive(&original, coverage(), &mut source),
            Err(ArchiveError::Identity)
        ));
        assert_eq!(source.writes, 0);
        source.joined = true; // Fixture-only predicate, never actual join proof.
        source.original = &foreign;
        assert!(matches!(
            verify_existing_joined_archive(&original, coverage(), &mut source),
            Err(ArchiveError::Identity)
        ));
        assert_eq!(source.writes, 0);
        source.original = &original;
        assert!(matches!(
            verify_existing_joined_archive(&original, coverage(), &mut source),
            Err(ArchiveError::Mismatch)
        ));
        assert_eq!(source.writes, 1); // Borrowed fixed meter only, no file callback.
        assert_eq!(source.bytes.as_ptr(), pointer);
        assert_eq!(plan.io.lock().unwrap().remaining, calls);
        let progress = plan.publication.lock().unwrap();
        let progress = progress.as_ref().unwrap();
        assert!(progress.expected.is_none());
        assert!(!progress.write_attempted);
    }

    // SOURCE ONLY/UNRUN: this existing fixture will require separate admission.
    #[test]
    fn assembled_publication_observation_is_original_bound_and_never_rewrites_lost_ack() {
        let root = filesystem_fixture();
        let original = filesystem_operation(&root);
        let replacement = filesystem_operation(&root); // SAME textual ID, different instance.
        let plan = original.archive_plan().unwrap();
        let bytes = fixture_bytes(&original);
        let expected = measurement(&bytes);
        let mut source = source(&original, &bytes);
        original.charge(OPERATION_BYTES).unwrap_err();
        let charged = original.captured_bytes();
        let observed = publish_joined_archive(&original, coverage(), &mut source).unwrap();
        assert!(observed.matches(&original, coverage(), expected));
        assert!(!observed.matches(&replacement, coverage(), expected));
        assert!(!observed.matches(&original, coverage(), measurement(b"other body")));
        assert!(!observed.unaccepted_temp_possible());
        let _flush_warning = observed.best_effort_sync_failed(); // No stronger crash promise.
        assert_eq!(source.writes, 2); // Fixed measurement + ONE actual temp body write.
        drop(observed); // Lost observation is never a fresh emission allowance.
        let observed = publish_joined_archive(&original, coverage(), &mut source).unwrap();
        assert!(observed.matches(&original, coverage(), expected));
        assert_eq!(source.writes, 3); // Measurement only; FINAL is independently reverified.
        drop(observed);
        let observed = verify_existing_joined_archive(&original, coverage(), &mut source).unwrap();
        assert!(observed.matches(&original, coverage(), expected));
        assert_eq!(source.writes, 4); // Strict existing-FINAL path, measurement only.
        assert!(!observed.unaccepted_temp_possible());
        drop(observed);
        let mut changed = bytes.clone(); // Synthetic test reference only.
        *changed.last_mut().unwrap() ^= 1;
        source.bytes = &changed;
        let calls = plan.io.lock().unwrap().remaining;
        assert!(matches!(
            publish_joined_archive(&original, coverage(), &mut source),
            Err(ArchiveError::Mismatch)
        ));
        assert_eq!(source.writes, 5); // Changed measurement rejected BEFORE any file effect.
        assert!(matches!(
            verify_existing_joined_archive(&original, coverage(), &mut source),
            Err(ArchiveError::Mismatch)
        ));
        assert_eq!(source.writes, 6); // Recovery also refuses changed SAME-attempt hash.
        assert_eq!(plan.io.lock().unwrap().remaining, calls);
        assert_eq!(std::fs::read(&plan.final_path).unwrap(), bytes);
        assert!(!plan.temp.exists());
        assert_eq!(original.captured_bytes(), charged);
        assert!(original.borrow_archive_chunk(&original).is_ok());
        // Explicit isolated synthetic-fixture cleanup only, no production leftovers.
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unjoined_and_stale_original_source_refuse_before_any_filesystem_call() {
        let original = verification_operation();
        let replacement = verification_operation();
        let plan = original.archive_plan().unwrap();
        let bytes = fixture_bytes(&original);
        let mut source = source(&original, &bytes);
        source.joined = false;
        let calls = plan.io.lock().unwrap().remaining;
        let mut io =
            StdArchiveIo::new(&original, coverage(), measurement(&bytes), &mut source).unwrap();
        assert!(matches!(io.preflight(), Err(ArchiveError::Identity)));
        assert!(matches!(io.write_temp(), Err(ArchiveError::Identity)));
        assert_eq!(plan.io.lock().unwrap().remaining, calls);
        assert_eq!(io.source.writes, 0);
        io.source.joined = true;
        io.source.original = &replacement;
        assert!(matches!(io.preflight(), Err(ArchiveError::Identity)));
        assert_eq!(plan.io.lock().unwrap().remaining, calls);
    }

    #[test]
    fn exhausted_original_call_reserve_refuses_before_paths_or_body_effects() {
        let original = verification_operation();
        let plan = original.archive_plan().unwrap();
        let bytes = fixture_bytes(&original);
        let mut source = source(&original, &bytes);
        plan.io.lock().unwrap().remaining = 0;
        let charged = original.captured_bytes();
        let mut io =
            StdArchiveIo::new(&original, coverage(), measurement(&bytes), &mut source).unwrap();
        assert!(matches!(io.write_temp(), Err(ArchiveError::Fit)));
        assert!(matches!(io.hard_link(), Err(ArchiveError::Fit)));
        assert_eq!(io.source.writes, 0);
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(plan.io.lock().unwrap().remaining, 0);
    }

    // The following filesystem controls are SOURCE ONLY/UNRUN until a separately
    // admitted local validation slot. No fixture/root is created by preparing them.
    #[test]
    fn same_attempt_std_publication_and_lost_result_reverify_without_rewrite_or_refund() {
        let root = filesystem_fixture();
        let original = filesystem_operation(&root);
        let plan = original.archive_plan().unwrap();
        let bytes = fixture_bytes(&original);
        let pointer = bytes.as_ptr();
        let expected = measurement(&bytes);
        let mut source = source(&original, &bytes);
        original.charge(OPERATION_BYTES).unwrap_err();
        let charged = original.captured_bytes();
        let mut io = StdArchiveIo::new(&original, coverage(), expected, &mut source).unwrap();
        {
            let mut progress = plan
                .borrow_publication(&original, coverage(), expected)
                .unwrap();
            let observation = progress.drive(&mut io).unwrap();
            assert!(!observation.unaccepted_temp_possible);
        }
        assert_eq!(io.source.bytes.as_ptr(), pointer);
        assert_eq!(io.source.writes, 1);
        assert_eq!(std::fs::read(&plan.final_path).unwrap(), bytes);
        assert!(!plan.temp.exists());
        // Simulate loss of the first observation. The current final is synced and
        // fully compared again; the original source is never emitted a second time.
        {
            let mut progress = plan
                .borrow_publication(&original, coverage(), expected)
                .unwrap();
            progress.drive(&mut io).unwrap();
        }
        assert_eq!(io.source.writes, 1);
        assert_eq!(original.captured_bytes(), charged);
        assert!(plan.io.lock().unwrap().remaining > 0);
        assert!(plan.io.lock().unwrap().resolved_remaining > 0);
        assert_eq!(io.source.bytes.as_ptr(), pointer);
        drop(io);
        // Explicit synthetic-fixture cleanup ONLY, never production leftover cleanup.
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unrelated_existing_final_and_same_length_tampering_are_never_accepted_or_clobbered() {
        let root = filesystem_fixture();
        let original = filesystem_operation(&root);
        let plan = original.archive_plan().unwrap();
        let bytes = fixture_bytes(&original);
        let expected = measurement(&bytes);
        std::fs::create_dir(&plan.folder).unwrap();
        std::fs::write(&plan.final_path, b"other attempt").unwrap();
        let mut source = source(&original, &bytes);
        let mut io = StdArchiveIo::new(&original, coverage(), expected, &mut source).unwrap();
        let mut progress = plan
            .borrow_publication(&original, coverage(), expected)
            .unwrap();
        assert!(matches!(
            progress.drive(&mut io),
            Err(ArchiveError::Mismatch)
        ));
        assert_eq!(std::fs::read(&plan.final_path).unwrap(), b"other attempt");
        assert_eq!(io.source.writes, 0);
        assert!(!plan.temp.exists());
        // Test-only external replacement keeps valid SAME header and length, but
        // corrupts a body byte. Header/length-only verification would miss this.
        std::fs::write(&plan.final_path, &bytes).unwrap();
        let mut corrupt = OpenOptions::new()
            .write(true)
            .open(&plan.final_path)
            .unwrap();
        corrupt.seek(SeekFrom::End(-1)).unwrap();
        corrupt.write_all(b"!").unwrap();
        corrupt.sync_all().unwrap();
        drop(corrupt);
        assert!(matches!(
            progress.drive(&mut io),
            Err(ArchiveError::Mismatch)
        ));
        assert_eq!(io.source.writes, 0);
        assert!(!plan.temp.exists());
        drop(progress);
        drop(io);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn partial_original_temp_remains_unaccepted_and_cannot_trigger_reemission() {
        let root = filesystem_fixture();
        let original = filesystem_operation(&root);
        let plan = original.archive_plan().unwrap();
        let bytes = fixture_bytes(&original);
        let expected = measurement(&bytes);
        let mut source = source(&original, &bytes);
        source.partial = true;
        let mut io = StdArchiveIo::new(&original, coverage(), expected, &mut source).unwrap();
        {
            let mut progress = plan
                .borrow_publication(&original, coverage(), expected)
                .unwrap();
            assert!(matches!(
                progress.drive(&mut io),
                Err(ArchiveError::Filesystem)
            ));
        }
        io.source.partial = false;
        {
            let mut progress = plan
                .borrow_publication(&original, coverage(), expected)
                .unwrap();
            assert!(matches!(
                progress.drive(&mut io),
                Err(ArchiveError::Mismatch)
            ));
        }
        assert_eq!(io.source.writes, 1);
        assert!(!plan.final_path.exists());
        assert_eq!(std::fs::read(&plan.temp).unwrap(), bytes[..bytes.len() / 2]);
        assert_eq!(io.source.bytes.as_ptr(), bytes.as_ptr());
        drop(io);
        // Production leaves this temp reported UNACCEPTED with canonical evidence.
        // Only this isolated synthetic fixture is explicitly disposed after assertions.
        std::fs::remove_dir_all(root).unwrap();
    }
}
