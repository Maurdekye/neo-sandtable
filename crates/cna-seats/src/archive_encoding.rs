//! Borrowed diagnostic records, never a transcript conversion or custody ACK.
//! The driver must enumerate its complete canonical set after actual joins in
//! both passes. A matching caller-provided descriptor alone proves neither fact.

use std::io::{self, Write};

use serde::Serialize;

use super::{ArchiveError, ArchiveMeasurement, ArchiveMeter, EvidenceCoverage};
use crate::operation::{CaptureOrigin, Operation};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceCategory {
    Startup,
    Helper,
    Stdout,
    Stderr,
    Diagnostic,
    Pairing,
}

impl EvidenceCategory {
    fn bit(self) -> u16 {
        match self {
            Self::Startup => 1,
            Self::Helper => 2,
            Self::Stdout => 4,
            Self::Stderr => 8,
            Self::Diagnostic => 16,
            Self::Pairing => 32,
        }
    }
}

/// References an already-owned component. The physical association and origin
/// come from that original record, never the current CLI/session/active pointer.
/// Unknown origin remains null even when the driver knows its physical child.
pub(crate) struct EvidenceRecord<'a> {
    pub(crate) operation: &'a Operation,
    pub(crate) category: EvidenceCategory,
    pub(crate) part: &'static str,
    pub(crate) index: u64,
    pub(crate) physical_child: Option<&'a str>,
    pub(crate) origin: Option<&'a CaptureOrigin>,
}

#[derive(Serialize)]
struct RecordIdentity<'a> {
    sequence: u64,
    category: EvidenceCategory,
    part: &'static str,
    index: u64,
    operation_id: &'a str,
    epoch: u64,
    physical_child: Option<&'a str>,
    origin: Option<&'a CaptureOrigin>,
}

impl EvidenceRecord<'_> {
    fn identity<'a>(
        &'a self,
        expected: &Operation,
        sequence: u64,
    ) -> Result<RecordIdentity<'a>, ArchiveError> {
        let plan = expected.archive_plan().ok_or(ArchiveError::Identity)?;
        if !self.operation.same_instance(expected)
            || self.part.is_empty()
            || self.part.len() > 64
            || !self
                .part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))
        {
            return Err(ArchiveError::Identity);
        }
        if let Some(child) = self.physical_child
            && (child.len() != 36 || uuid::Uuid::parse_str(child).is_err())
        {
            return Err(ArchiveError::Identity);
        }
        if let Some(origin) = self.origin {
            if origin.operation_id != expected.id() || origin.child_generation != plan.epoch {
                return Err(ArchiveError::Identity);
            }
            // A saved known origin may not be attached to a different/unknown
            // physical component. Missing origin stays missing, never synthesized.
            if let Some(child) = origin.physical_child_id.as_deref()
                && self.physical_child != Some(child)
            {
                return Err(ArchiveError::Identity);
            }
        }
        Ok(RecordIdentity {
            sequence,
            category: self.category,
            part: self.part,
            index: self.index,
            operation_id: self.operation.id(),
            epoch: plan.epoch,
            physical_child: self.physical_child,
            origin: self.origin,
        })
    }
}

/// First borrowed pass hashes ordered component identities, not cloned bodies.
/// No Vec, map or shadow collection is retained. The canonical driver supplies
/// the same complete ordering again for emission; omitted/reordered suffixes fail.
pub(crate) struct EvidenceCoverageBuilder<'a> {
    operation: &'a Operation,
    meter: ArchiveMeter,
    categories: u16,
    count: u64,
    failed: bool,
}

impl<'a> EvidenceCoverageBuilder<'a> {
    pub(crate) fn new(operation: &'a Operation) -> Result<Self, ArchiveError> {
        operation.archive_plan().ok_or(ArchiveError::Identity)?;
        Ok(Self {
            operation,
            meter: ArchiveMeter::new(),
            categories: 0,
            count: 0,
            failed: false,
        })
    }

    pub(crate) fn include(&mut self, record: &EvidenceRecord<'_>) -> Result<(), ArchiveError> {
        if self.failed {
            return Err(ArchiveError::Identity);
        }
        let result = (|| {
            let next = self.count.checked_add(1).ok_or(ArchiveError::Fit)?;
            let identity = record.identity(self.operation, self.count)?;
            serde_json::to_writer(&mut self.meter, &identity).map_err(|_| ArchiveError::Fit)?;
            self.meter.write_all(b"\n").map_err(|_| ArchiveError::Fit)?;
            self.categories |= record.category.bit();
            self.count = next;
            Ok(())
        })();
        self.failed |= result.is_err();
        result
    }

    pub(crate) fn finish(self) -> Result<EvidenceCoverage, ArchiveError> {
        if self.failed {
            return Err(ArchiveError::Identity);
        }
        EvidenceCoverage::new(self.categories, self.count, self.meter.finish()?.sha256)
    }
}

struct MeasuredWriter<'a, W: Write> {
    output: &'a mut W,
    meter: ArchiveMeter,
    failed: bool,
}

impl<W: Write> Write for MeasuredWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.failed {
            return Err(io::ErrorKind::Other.into());
        }
        match self.output.write(bytes) {
            Ok(count) if count <= bytes.len() && (count != 0 || bytes.is_empty()) => {
                if self.meter.write_all(&bytes[..count]).is_err() {
                    self.failed = true;
                    return Err(io::ErrorKind::FileTooLarge.into());
                }
                Ok(count)
            }
            Ok(_) => {
                self.failed = true;
                Err(io::ErrorKind::WriteZero.into())
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.failed {
            return Err(io::ErrorKind::Other.into());
        }
        let result = self.output.flush();
        self.failed |= result.is_err();
        result
    }
}

/// Second borrowed pass streams JSONL with fixed own encoder/hash state. Input
/// bodies stay canonical on every return. Arbitrary custom Serialize/Write code
/// is not a fit proof; actual trusted record/library/future captures must fit the
/// original creation reserve before adoption. This helper performs no filesystem
/// calls, join checks, accepted write acknowledgment, clear, refund or settlement.
pub(crate) struct ArchiveEncoder<'a, W: Write> {
    operation: &'a Operation,
    expected: EvidenceCoverage,
    coverage: EvidenceCoverageBuilder<'a>,
    writer: MeasuredWriter<'a, W>,
}

impl<'a, W: Write> ArchiveEncoder<'a, W> {
    pub(crate) fn new(
        operation: &'a Operation,
        coverage: EvidenceCoverage,
        output: &'a mut W,
    ) -> Result<Self, ArchiveError> {
        let plan = operation.archive_plan().ok_or(ArchiveError::Identity)?;
        let header = plan.header(operation, coverage)?;
        let mut writer = MeasuredWriter {
            output,
            meter: ArchiveMeter::new(),
            failed: false,
        };
        writer
            .write_all(&header.bytes[..header.len])
            .map_err(|_| ArchiveError::Filesystem)?;
        Ok(Self {
            operation,
            expected: coverage,
            coverage: EvidenceCoverageBuilder::new(operation)?,
            writer,
        })
    }

    fn begin_record(
        &mut self,
        record: &EvidenceRecord<'_>,
        encoding: &str,
    ) -> Result<(), ArchiveError> {
        if self.writer.failed {
            return Err(ArchiveError::Filesystem);
        }
        let identity = record.identity(self.operation, self.coverage.count)?;
        self.coverage.include(record)?;
        self.writer
            .write_all(b"{\"evidence\":")
            .map_err(|_| ArchiveError::Filesystem)?;
        serde_json::to_writer(&mut self.writer, &identity).map_err(|_| ArchiveError::Fit)?;
        self.writer
            .write_all(b",\"encoding\":")
            .map_err(|_| ArchiveError::Filesystem)?;
        serde_json::to_writer(&mut self.writer, encoding).map_err(|_| ArchiveError::Fit)?;
        self.writer
            .write_all(b",\"payload\":")
            .map_err(|_| ArchiveError::Filesystem)?;
        Ok(())
    }

    pub(crate) fn bytes(
        &mut self,
        record: &EvidenceRecord<'_>,
        bytes: &[u8],
    ) -> Result<(), ArchiveError> {
        let result = (|| {
            self.begin_record(record, "hex")?;
            self.writer
                .write_all(b"\"")
                .map_err(|_| ArchiveError::Filesystem)?;
            // Invalid UTF-8 and every original byte survive, without a lossy
            // conversion, body-sized encoder allocation or replacement operation.
            let mut hex = [0u8; 128];
            const DIGITS: &[u8; 16] = b"0123456789abcdef";
            for chunk in bytes.chunks(hex.len() / 2) {
                for (index, byte) in chunk.iter().enumerate() {
                    hex[2 * index] = DIGITS[usize::from(byte >> 4)];
                    hex[2 * index + 1] = DIGITS[usize::from(byte & 15)];
                }
                self.writer
                    .write_all(&hex[..2 * chunk.len()])
                    .map_err(|_| ArchiveError::Filesystem)?;
            }
            self.writer
                .write_all(b"\"}\n")
                .map_err(|_| ArchiveError::Filesystem)?;
            Ok(())
        })();
        self.writer.failed |= result.is_err();
        result
    }

    pub(crate) fn json<T: Serialize + ?Sized>(
        &mut self,
        record: &EvidenceRecord<'_>,
        payload: &T,
    ) -> Result<(), ArchiveError> {
        let result = (|| {
            self.begin_record(record, "json")?;
            serde_json::to_writer(&mut self.writer, payload).map_err(|_| ArchiveError::Fit)?;
            self.writer
                .write_all(b"}\n")
                .map_err(|_| ArchiveError::Filesystem)?;
            Ok(())
        })();
        self.writer.failed |= result.is_err();
        result
    }

    pub(crate) fn finish(self) -> Result<ArchiveMeasurement, ArchiveError> {
        if self.writer.failed {
            return Err(ArchiveError::Filesystem);
        }
        if self.coverage.finish()? != self.expected {
            return Err(ArchiveError::Mismatch);
        }
        self.writer.meter.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{CampaignArchive, FIXED_ARCHIVE_WORKSPACE};
    use super::*;
    use crate::operation::{Allowance, OperationError};
    use std::time::Duration;
    use tokio::time::Instant;

    const CHILD: &str = "33333333-3333-4333-8333-333333333333";
    fn operation() -> Operation {
        let root = std::env::temp_dir().join("synthetic-borrowed-archive-no-io");
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
    fn record(operation: &Operation, index: u64) -> EvidenceRecord<'_> {
        EvidenceRecord {
            operation,
            category: EvidenceCategory::Stdout,
            part: "frame",
            index,
            physical_child: Some(CHILD),
            origin: None,
        }
    }
    fn coverage(operation: &Operation, records: &[EvidenceRecord<'_>]) -> EvidenceCoverage {
        let mut builder = EvidenceCoverageBuilder::new(operation).unwrap();
        for record in records {
            builder.include(record).unwrap();
        }
        builder.finish().unwrap()
    }

    #[test]
    fn borrowed_invalid_utf8_and_structured_records_preserve_pointer_unknown_and_meter() {
        let operation = operation();
        let records = [record(&operation, 0), record(&operation, 1)];
        let coverage = coverage(&operation, &records);
        let raw = [0, 0xff, b'\n', b'"', b'\\', 0x80];
        let pointer = raw.as_ptr();
        let payload = serde_json::json!({"unicode":"é\n", "nested":{"unknown":null}});
        let payload_ptr = payload["unicode"].as_str().unwrap().as_ptr();
        let charged = operation.captured_bytes();
        operation.overflow();
        let mut output = Vec::new(); // synthetic reference output ONLY
        let mut encoder = ArchiveEncoder::new(&operation, coverage, &mut output).unwrap();
        encoder.bytes(&records[0], &raw).unwrap();
        encoder.json(&records[1], &payload).unwrap();
        let measurement = encoder.finish().unwrap();
        let lines: Vec<_> = output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<serde_json::Value>(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1]["payload"], "00ff0a225c80");
        assert!(lines[1]["evidence"]["origin"].is_null());
        assert_eq!(lines[1]["evidence"]["physical_child"], CHILD);
        assert_eq!(lines[2]["payload"], payload);
        assert_eq!(raw.as_ptr(), pointer);
        assert_eq!(payload["unicode"].as_str().unwrap().as_ptr(), payload_ptr);
        assert_eq!(operation.captured_bytes(), charged);
        assert_eq!(operation.check(), Err(OperationError::CaptureOverflow));
        operation
            .archive_plan()
            .unwrap()
            .verify_stream(
                &operation,
                coverage,
                measurement,
                &mut std::io::Cursor::new(&output),
            )
            .unwrap();
    }

    #[test]
    fn omitted_reordered_and_replaced_instance_records_cannot_finish_as_complete() {
        let operation = operation();
        let records = [record(&operation, 0), record(&operation, 1)];
        let coverage = coverage(&operation, &records);
        let mut output = Vec::new();
        let mut encoder = ArchiveEncoder::new(&operation, coverage, &mut output).unwrap();
        encoder.bytes(&records[0], b"canonical first").unwrap();
        assert!(matches!(encoder.finish(), Err(ArchiveError::Mismatch)));
        let mut output = Vec::new();
        let mut encoder = ArchiveEncoder::new(&operation, coverage, &mut output).unwrap();
        encoder.bytes(&records[1], b"canonical second").unwrap();
        encoder.bytes(&records[0], b"canonical first").unwrap();
        assert!(matches!(encoder.finish(), Err(ArchiveError::Mismatch)));
        let replacement = self::operation();
        let mut output = Vec::new();
        let mut encoder = ArchiveEncoder::new(&operation, coverage, &mut output).unwrap();
        let prefix_len = encoder.writer.meter.length;
        assert!(matches!(
            encoder.bytes(&record(&replacement, 0), b"wrong original"),
            Err(ArchiveError::Identity)
        ));
        assert_eq!(encoder.writer.meter.length, prefix_len);
        assert!(encoder.bytes(&records[0], b"retry after refusal").is_err());
        assert!(encoder.finish().is_err());
        let mut builder = EvidenceCoverageBuilder::new(&operation).unwrap();
        assert!(builder.include(&record(&replacement, 0)).is_err());
        assert!(builder.include(&records[0]).is_err());
        assert!(builder.finish().is_err());
    }

    #[test]
    fn known_origin_mismatch_refuses_and_unknown_is_never_repaired() {
        let operation = operation();
        let origin = CaptureOrigin {
            operation_id: operation.id().into(),
            child_generation: 7,
            ordinal: 9,
            physical_child_id: Some(CHILD.into()),
        };
        let mut record = record(&operation, 0);
        record.origin = Some(&origin);
        assert!(record.identity(&operation, 0).is_ok());
        record.physical_child = None;
        assert!(record.identity(&operation, 0).is_err());
        record.origin = None;
        let unknown = record.identity(&operation, 0).unwrap();
        assert!(unknown.origin.is_none());
        assert!(unknown.physical_child.is_none());
        record.physical_child = Some("not-a-trusted-uuid");
        assert!(record.identity(&operation, 0).is_err());
        assert_eq!(origin.ordinal, 9); // no new ordinal or synthesized origin
        let stale_epoch = CaptureOrigin {
            child_generation: 8,
            ..origin.clone()
        };
        record.physical_child = Some(CHILD);
        record.origin = Some(&stale_epoch);
        assert!(record.identity(&operation, 0).is_err());
        let stale_operation = CaptureOrigin {
            operation_id: "44444444-4444-4444-8444-444444444444".into(),
            ..origin.clone()
        };
        record.origin = Some(&stale_operation);
        assert!(record.identity(&operation, 0).is_err());
    }

    #[test]
    fn partial_writer_failure_is_sticky_and_keeps_borrowed_body_and_original_workspace() {
        struct PrefixWriter {
            remaining: usize,
            written: usize,
        }
        impl Write for PrefixWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.remaining == 0 {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                let count = bytes.len().min(7).min(self.remaining);
                self.remaining -= count;
                self.written += count;
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let operation = operation();
        let records = [record(&operation, 0)];
        let coverage = coverage(&operation, &records);
        let header = operation
            .archive_plan()
            .unwrap()
            .header(&operation, coverage)
            .unwrap();
        let raw = vec![0xff; 4097]; // synthetic canonical input ONLY
        let pointer = raw.as_ptr();
        let charged = operation.captured_bytes();
        let mut output = PrefixWriter {
            remaining: header.len + 300,
            written: 0,
        };
        let mut encoder = ArchiveEncoder::new(&operation, coverage, &mut output).unwrap();
        assert!(encoder.bytes(&records[0], &raw).is_err());
        let prefix = encoder.writer.output.written;
        assert!(encoder.bytes(&records[0], &raw).is_err());
        assert_eq!(encoder.writer.output.written, prefix);
        assert!(encoder.finish().is_err());
        assert_eq!(raw.as_ptr(), pointer);
        assert!(raw.iter().all(|byte| *byte == 0xff));
        assert_eq!(operation.captured_bytes(), charged);
        // Own fixed state only, not Serialize/Write captures, library stack,
        // allocator RSS or a proof of the eventual complete driver enumeration.
        assert!(
            std::mem::size_of::<ArchiveEncoder<'_, PrefixWriter>>()
                + std::mem::size_of::<RecordIdentity<'_>>()
                + 128
                + 64 * std::mem::size_of::<u32>()
                < FIXED_ARCHIVE_WORKSPACE
        );
    }
}
