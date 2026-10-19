//! Original raw diagnostics and explicit truncation accounting, by borrow only.
//! Nothing here establishes all-child joins, archive custody or a healthy stop.

use super::archive_stdout::RecordSink;
use super::{StderrCapture, StderrStats, StdoutChunk};
use crate::failed_evidence::{
    ArchiveEncoder, ArchiveError, EvidenceCategory, EvidenceCoverageBuilder, EvidenceRecord,
};
use crate::operation::Operation;
use std::io::Write;

impl super::PendingStderrLines {
    /// Borrow-only precursor for the staged owner, before physical-reader
    /// adoption. The caller must supply its actual physical child identity.
    /// This covers queued rows ONLY, not the caller's unsent input or stats.
    /// It establishes neither complete census nor permission to release rows.
    #[allow(dead_code)]
    pub(super) fn visit_staged_archive(
        &self,
        expected: &Operation,
        physical_child: &str,
        sink: &mut impl RecordSink,
    ) -> Result<(), ArchiveError> {
        // Validate the WHOLE batch before the first sink call. Recovery must
        // remain possible after an original deadline/overflow, so do not call
        // Operation::check or interpret first_refusal as an archive veto.
        if !self.used
            || !self
                .original
                .as_ref()
                .is_some_and(|owner| owner.same_instance(expected))
            || self.capacity_pin < crate::operation::STDERR_LINES
            || self.rows.capacity() != self.capacity_pin
            || self.rows.len() > self.capacity_pin
            || self.rows.len() > crate::operation::STDERR_LINES
            || self
                .rows
                .iter()
                .any(|line| !line.operation.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        for (index, line) in self.rows.iter().enumerate() {
            sink.bytes(
                EvidenceRecord {
                    operation: &line.operation,
                    category: EvidenceCategory::Stderr,
                    part: "stderr_line",
                    index: u64::try_from(index).map_err(|_| ArchiveError::Fit)?,
                    physical_child: Some(physical_child),
                    origin: None,
                },
                &line.bytes,
            )?;
        }
        Ok(()) // No pop, clone, reset, refund or release, including sink failure.
    }
}

pub(super) struct BorrowedStderr<'a> {
    pub operation: &'a Operation,
    pub physical_child: &'a str,
    pub retained: &'a StderrCapture,
    pub stats: &'a StderrStats,
    pub chunk: &'a StdoutChunk,
}

impl BorrowedStderr<'_> {
    fn validate(&self) -> Result<(), ArchiveError> {
        let identity = || Err(ArchiveError::Identity);
        if let Some(pin) = &self.retained.owned_pin {
            pin.validate_archive(&self.retained.lines, self.operation)?;
        }
        if self.chunk.count > self.chunk.bytes.len() || self.chunk.cursor > self.chunk.count {
            return identity();
        }
        match &self.chunk.operation {
            Some(operation) if !operation.same_instance(self.operation) => return identity(),
            None if self.chunk.count != 0
                || self.chunk.cursor != 0
                || self.chunk.read_uncertain
                || self.chunk.seen_accounted =>
            {
                return identity();
            }
            _ => {}
        }
        if self
            .retained
            .lines
            .iter()
            .any(|line| !line.operation.same_instance(self.operation))
            || self
                .retained
                .partial_operation
                .as_ref()
                .is_some_and(|owner| !owner.same_instance(self.operation))
            || (!self.retained.partial.is_empty() && self.retained.partial_operation.is_none())
        {
            return identity();
        }
        let stats_present = self.stats.seen_bytes != 0
            || self.stats.retained_bytes != 0
            || self.stats.discarded_bytes != 0
            || self.stats.truncated
            || self.retained.dropping;
        match &self.retained.stats_operation {
            Some(operation) if !operation.same_instance(self.operation) => return identity(),
            None if stats_present => return identity(),
            _ => {}
        }
        let retained_bytes = self
            .retained
            .lines
            .iter()
            .try_fold(self.retained.partial.len(), |total, line| {
                total.checked_add(line.bytes.len())
            })
            .ok_or(ArchiveError::Fit)?;
        let accounted = u64::try_from(retained_bytes)
            .map_err(|_| ArchiveError::Fit)?
            .checked_add(self.stats.discarded_bytes)
            .ok_or(ArchiveError::Fit)?;
        if retained_bytes != self.stats.retained_bytes
            || accounted > self.stats.seen_bytes
            || (self.stats.discarded_bytes != 0 && !self.stats.truncated)
        {
            return identity();
        }
        Ok(())
    }

    fn record<'a>(
        &'a self,
        owner: &'a Operation,
        part: &'static str,
        index: usize,
    ) -> Result<EvidenceRecord<'a>, ArchiveError> {
        Ok(EvidenceRecord {
            operation: owner,
            category: EvidenceCategory::Stderr,
            part,
            index: u64::try_from(index).map_err(|_| ArchiveError::Fit)?,
            physical_child: Some(self.physical_child),
            origin: None,
        })
    }

    pub(super) fn visit(&self, sink: &mut impl RecordSink) -> Result<(), ArchiveError> {
        self.validate()?;
        for (index, line) in self.retained.lines.iter().enumerate() {
            sink.bytes(
                self.record(&line.operation, "stderr_line", index)?,
                &line.bytes,
            )?;
        }
        if let Some(owner) = &self.retained.partial_operation {
            sink.bytes(
                self.record(owner, "stderr_partial", 0)?,
                &self.retained.partial,
            )?;
        }
        if let Some(owner) = &self.retained.stats_operation {
            // Count-only discarded diagnostic bytes are explicit, never recreated
            // as raw or interpreted as accepted stdout/game answers.
            sink.json(
                self.record(owner, "stderr_stats", 0)?,
                &(
                    self.stats.seen_bytes,
                    self.stats.retained_bytes,
                    self.stats.discarded_bytes,
                    self.stats.truncated,
                    self.retained.dropping,
                ),
            )?;
        }
        if let Some(owner) = &self.chunk.operation {
            sink.json(
                self.record(owner, "stderr_read_state", 0)?,
                &(
                    self.chunk.count,
                    self.chunk.cursor,
                    self.chunk.read_uncertain,
                    self.chunk.seen_accounted,
                ),
            )?;
            if self.chunk.count != 0 {
                sink.bytes(
                    self.record(owner, "stderr_read_bytes", 0)?,
                    &self.chunk.bytes[..self.chunk.count],
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn cover(
        &self,
        coverage: &mut EvidenceCoverageBuilder<'_>,
    ) -> Result<(), ArchiveError> {
        self.visit(coverage)
    }
    pub(super) fn encode<W: Write>(
        &self,
        encoder: &mut ArchiveEncoder<'_, W>,
    ) -> Result<(), ArchiveError> {
        self.visit(encoder)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{StderrLine, StdoutRead, rebind_capture};
    use super::*;
    use crate::failed_evidence::CampaignArchive;
    use crate::operation::Allowance;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::time::Instant;

    const CHILD: &str = "33333333-3333-4333-8333-333333333333";
    fn operation() -> Operation {
        let root = std::env::temp_dir().join("synthetic-stderr-archive-no-io");
        CampaignArchive::new(
            &root.join("campaigns/11111111-1111-4111-8111-111111111111.sqlite"),
            &root.join("sandbox"),
            &root.join("run"),
            &[root.join("isolation")],
        )
        .unwrap()
        .operation(
            "axis.commander".parse().unwrap(),
            7,
            "22222222-2222-4222-8222-222222222222".into(),
            Allowance::required(Some(Instant::now() + Duration::from_secs(60))).unwrap(),
        )
        .unwrap()
    }

    // DESIGN controls only until the separately admitted strict joint. These
    // legacy Operation fixtures do not certify complete original F/body G.
    #[test]
    fn staged_rows_archive_raw_fifo_after_overflow_without_consuming_or_refunding() {
        let mut staged = super::super::PendingStderrLines::new().unwrap();
        let original = operation();
        staged.bind_original(&original).unwrap();
        for bytes in [vec![0xff, b'\n'], vec![0xfe, b'x']] {
            let mut input = Some(StderrLine {
                bytes,
                operation: original.clone(),
            });
            staged.push_from_slot(&mut input).unwrap();
            assert!(input.is_none());
        }
        let pointers = [staged.rows[0].bytes.as_ptr(), staged.rows[1].bytes.as_ptr()];
        let pin = staged.capacity_pin;
        let charged = original.captured_bytes();
        original.overflow();
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        staged
            .visit_staged_archive(&original, CHILD, &mut coverage)
            .unwrap();
        let coverage = coverage.finish().unwrap();
        let mut output = Vec::new(); // Synthetic reference bytes, not file I/O.
        let mut encoder = ArchiveEncoder::new(&original, coverage, &mut output).unwrap();
        staged
            .visit_staged_archive(&original, CHILD, &mut encoder)
            .unwrap();
        let measurement = encoder.finish().unwrap();
        original
            .archive_plan()
            .unwrap()
            .verify_stream(
                &original,
                coverage,
                measurement,
                &mut std::io::Cursor::new(&output),
            )
            .unwrap();
        let records: Vec<serde_json::Value> = output
            .split(|byte| *byte == b'\n')
            .filter(|row| !row.is_empty())
            .map(|row| serde_json::from_slice(row).unwrap())
            .collect();
        assert_eq!(records.len(), 3);
        assert_eq!(records[1]["payload"], "ff0a");
        assert_eq!(records[2]["payload"], "fe78");
        assert_eq!(staged.rows.len(), 2);
        assert_eq!(staged.rows[0].bytes.as_ptr(), pointers[0]);
        assert_eq!(staged.rows[1].bytes.as_ptr(), pointers[1]);
        assert_eq!(staged.rows.capacity(), pin);
        assert_eq!(staged.capacity_pin, pin);
        assert_eq!(original.captured_bytes(), charged);
    }

    struct StagedSink {
        calls: usize,
        fail: bool,
    }
    impl RecordSink for StagedSink {
        fn bytes(&mut self, _: EvidenceRecord<'_>, _: &[u8]) -> Result<(), ArchiveError> {
            self.calls += 1;
            if self.fail {
                Err(ArchiveError::Filesystem)
            } else {
                Ok(())
            }
        }
        fn json<T: serde::Serialize + ?Sized>(
            &mut self,
            _: EvidenceRecord<'_>,
            _: &T,
        ) -> Result<(), ArchiveError> {
            panic!("staged raw rows must not synthesize metadata");
        }
    }

    #[test]
    fn adopted_ring_pin_is_checked_before_archive_sink_without_clearing_raw_rows() {
        let mut pending = Some(super::super::PendingStderrLines::new().unwrap());
        let original = operation();
        pending.as_mut().unwrap().bind_original(&original).unwrap();
        let mut retained = StderrCapture::from_pending_lines(&mut pending, &original).unwrap();
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let mut read = StdoutRead::begin(slot.clone(), original.clone()).unwrap();
        read.buffer()[..2].copy_from_slice(&[0xff, b'\n']);
        read.returned(2);
        let mut stats = StderrStats::default();
        retained.accept_read(&mut read, &mut stats).unwrap();
        drop(read);
        let pin = retained.owned_pin.as_ref().unwrap().capacity;
        let pointer = retained.lines[0].bytes.as_ptr();
        let charged = original.captured_bytes();
        let chunk = slot.lock().unwrap();
        let chunk = chunk.as_ref().unwrap();
        assert!(
            BorrowedStderr {
                operation: &original,
                physical_child: CHILD,
                retained: &retained,
                stats: &stats,
                chunk
            }
            .validate()
            .is_ok()
        );
        retained.owned_pin.as_mut().unwrap().capacity += 1; // TEST-ONLY mismatch, no repair.
        let mut sink = StagedSink {
            calls: 0,
            fail: true,
        };
        assert!(matches!(
            BorrowedStderr {
                operation: &original,
                physical_child: CHILD,
                retained: &retained,
                stats: &stats,
                chunk
            }
            .visit(&mut sink),
            Err(ArchiveError::Identity)
        ));
        assert_eq!(sink.calls, 0);
        assert_eq!(retained.lines[0].bytes, [0xff, b'\n']);
        assert_eq!(retained.lines[0].bytes.as_ptr(), pointer);
        assert_eq!(retained.lines.capacity(), pin);
        assert_eq!(stats.retained_bytes, 2);
        assert_eq!(original.captured_bytes(), charged);
        assert!(retained.check_legacy_archive_release().is_err());
    }

    #[test]
    fn staged_validation_precedes_every_sink_and_sink_error_retains_whole_batch() {
        let mut staged = super::super::PendingStderrLines::new().unwrap();
        let original = operation();
        let foreign = operation(); // Same text, distinct original State.
        assert_eq!(original.id(), foreign.id());
        assert!(!original.same_instance(&foreign));
        let mut sink = StagedSink {
            calls: 0,
            fail: false,
        };
        assert!(
            staged
                .visit_staged_archive(&original, CHILD, &mut sink)
                .is_err()
        );
        staged.bind_original(&original).unwrap();
        for byte in [1, 2] {
            let mut input = Some(StderrLine {
                bytes: vec![byte],
                operation: original.clone(),
            });
            staged.push_from_slot(&mut input).unwrap();
        }
        assert!(
            staged
                .visit_staged_archive(&foreign, CHILD, &mut sink)
                .is_err()
        );
        // TEST-ONLY corruption in the LAST row: no valid prefix may escape.
        staged.rows[1].operation = foreign;
        assert!(
            staged
                .visit_staged_archive(&original, CHILD, &mut sink)
                .is_err()
        );
        assert_eq!(sink.calls, 0);
        staged.rows[1].operation = original.clone(); // Test setup, no production repair.
        let pin = staged.capacity_pin;
        staged.capacity_pin += 1; // TEST-ONLY pin mismatch.
        assert!(
            staged
                .visit_staged_archive(&original, CHILD, &mut sink)
                .is_err()
        );
        assert_eq!(sink.calls, 0);
        staged.capacity_pin = pin;
        let pointer = staged.rows[0].bytes.as_ptr();
        let charged = original.captured_bytes();
        sink.fail = true;
        assert!(matches!(
            staged.visit_staged_archive(&original, CHILD, &mut sink),
            Err(ArchiveError::Filesystem)
        ));
        assert_eq!(sink.calls, 1);
        assert_eq!(staged.rows.len(), 2);
        assert_eq!(staged.rows[0].bytes.as_ptr(), pointer);
        assert_eq!(staged.rows[1].bytes, [2]);
        assert_eq!(staged.rows.capacity(), pin);
        assert_eq!(original.captured_bytes(), charged);
        assert!(staged.first_refusal.is_none()); // Sink cause returns unchanged, no ring reset.
    }

    #[test]
    fn raw_partial_read_and_explicit_truncation_counters_remain_borrowed() {
        let original = operation();
        // Synthetic canonical storage; no filesystem/native/actual-join claim.
        let mut retained = StderrCapture::new();
        retained.lines.push_back(StderrLine {
            bytes: vec![0xff, b'\n'],
            operation: original.clone(),
        });
        retained.partial = vec![0xfe, b'a'];
        retained.partial_operation = Some(original.clone());
        retained.stats_operation = Some(original.clone());
        let stats = StderrStats {
            seen_bytes: 10,
            retained_bytes: 4,
            discarded_bytes: 3,
            truncated: true,
        };
        let mut chunk = StdoutChunk::empty();
        chunk.operation = Some(original.clone());
        chunk.bytes[..3].copy_from_slice(&[0xfd, b'x', b'y']);
        chunk.count = 3;
        chunk.cursor = 0; // seen but not accepted or discarded suffix stays raw
        chunk.seen_accounted = true;
        let line_pointer = retained.lines[0].bytes.as_ptr();
        let partial_pointer = retained.partial.as_ptr();
        let charged = original.captured_bytes();
        original.overflow();
        let view = BorrowedStderr {
            operation: &original,
            physical_child: CHILD,
            retained: &retained,
            stats: &stats,
            chunk: &chunk,
        };
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        view.cover(&mut coverage).unwrap();
        let coverage = coverage.finish().unwrap();
        let mut output = Vec::new(); // synthetic serialized reference ONLY
        let mut encoder = ArchiveEncoder::new(&original, coverage, &mut output).unwrap();
        view.encode(&mut encoder).unwrap();
        let measurement = encoder.finish().unwrap();
        let records: Vec<serde_json::Value> = output
            .split(|byte| *byte == b'\n')
            .filter(|row| !row.is_empty())
            .map(|row| serde_json::from_slice(row).unwrap())
            .collect();
        assert_eq!(records.len(), 6);
        assert_eq!(records[1]["payload"], "ff0a");
        assert_eq!(records[2]["payload"], "fe61");
        assert_eq!(
            records[3]["payload"],
            serde_json::json!([10, 4, 3, true, false])
        );
        assert_eq!(records[5]["payload"], "fd7879");
        assert!(
            records[1..]
                .iter()
                .all(|row| row["evidence"]["origin"].is_null())
        );
        original
            .archive_plan()
            .unwrap()
            .verify_stream(
                &original,
                coverage,
                measurement,
                &mut std::io::Cursor::new(&output),
            )
            .unwrap();
        assert_eq!(retained.lines[0].bytes.as_ptr(), line_pointer);
        assert_eq!(retained.partial.as_ptr(), partial_pointer);
        assert_eq!(chunk.cursor, 0);
        assert_eq!(stats.discarded_bytes, 3);
        assert_eq!(original.captured_bytes(), charged);
    }

    #[test]
    fn completed_stderr_line_and_counters_block_rebind_and_cross_operation_accounting() {
        let original = operation();
        let replacement = operation(); // same textual ID, distinct original instance
        let capture = Mutex::new(original.clone());
        let slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let mut retained = StderrCapture::new();
        let mut stats = StderrStats::default();
        let mut read = StdoutRead::begin(slot.clone(), original.clone()).unwrap();
        read.buffer()[..2].copy_from_slice(&[0xff, b'\n']);
        read.returned(2);
        retained.accept_read(&mut read, &mut stats).unwrap();
        drop(read);
        assert!(retained.partial.is_empty());
        assert!(!super::super::stdout_chunk_owned(&slot));
        assert!(retained.has_retained_evidence());
        assert!(
            rebind_capture(&capture, replacement.clone(), || usize::from(
                retained.has_retained_evidence()
            ))
            .is_err()
        );
        assert!(capture.lock().unwrap().same_instance(&original));
        assert!(
            retained
                .stats_operation
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        let before = stats.clone();
        let replacement_charge = replacement.captured_bytes();
        let mut wrong = StdoutRead::begin(slot.clone(), replacement.clone()).unwrap();
        wrong.buffer()[..3].copy_from_slice(b"new");
        wrong.returned(3);
        assert!(retained.accept_read(&mut wrong, &mut stats).is_err());
        assert_eq!(stats, before);
        assert_eq!(wrong.chunk.as_ref().unwrap().cursor, 0);
        assert!(!wrong.chunk.as_ref().unwrap().seen_accounted);
        assert_eq!(replacement.captured_bytes(), replacement_charge);
        assert_eq!(retained.lines[0].bytes, [0xff, b'\n']);
        drop(wrong); // restores rejected bytes, no joins/ACK/healthy release
        assert!(super::super::stdout_chunk_owned(&slot));
    }

    #[test]
    fn missing_counter_owner_and_false_truncation_refuse_before_emission() {
        let original = operation();
        let mut retained = StderrCapture::new();
        let mut stats = StderrStats {
            seen_bytes: 3,
            discarded_bytes: 3,
            ..StderrStats::default()
        };
        let chunk = StdoutChunk::empty();
        {
            let view = BorrowedStderr {
                operation: &original,
                physical_child: CHILD,
                retained: &retained,
                stats: &stats,
                chunk: &chunk,
            };
            assert!(matches!(view.validate(), Err(ArchiveError::Identity)));
        }
        retained.stats_operation = Some(original.clone());
        {
            let view = BorrowedStderr {
                operation: &original,
                physical_child: CHILD,
                retained: &retained,
                stats: &stats,
                chunk: &chunk,
            };
            assert!(matches!(view.validate(), Err(ArchiveError::Identity)));
        }
        stats.truncated = true;
        let replacement = operation();
        let view = BorrowedStderr {
            operation: &original,
            physical_child: CHILD,
            retained: &retained,
            stats: &stats,
            chunk: &chunk,
        };
        assert!(view.validate().is_ok());
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        view.cover(&mut coverage).unwrap();
        let coverage = coverage.finish().unwrap();
        let mut output = Vec::new();
        let mut encoder = ArchiveEncoder::new(&original, coverage, &mut output).unwrap();
        let stale = BorrowedStderr {
            operation: &replacement,
            ..view
        };
        assert!(stale.encode(&mut encoder).is_err());
        drop(encoder);
        assert_eq!(output.iter().filter(|byte| **byte == b'\n').count(), 1); // header only
        assert!(
            retained
                .stats_operation
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        assert_eq!(stats.discarded_bytes, 3);
    }
}
