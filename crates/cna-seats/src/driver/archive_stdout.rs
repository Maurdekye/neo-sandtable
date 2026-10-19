//! Borrowed canonical stdout storage only. No child joins, custody or release
//! are inferred from this view; ChildProc establishes actual joins separately.

use super::{ChildFrame, FrameAssembly, RetainedEvents, StdoutChunk, StreamEvent};
use crate::failed_evidence::{
    ArchiveEncoder, ArchiveError, EvidenceCategory, EvidenceCoverageBuilder, EvidenceRecord,
};
use crate::operation::{CaptureOrigin, Operation};
use serde::Serialize;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};

/// Unique ownership permits callbacks/I/O without holding a synchronous lock.
/// Shared readers, poisoned storage and checked-out chunks must refuse, never
/// masquerade as empty evidence after cancellation.
pub(super) fn unique_storage<T>(storage: &mut Arc<Mutex<T>>) -> Result<&mut T, ArchiveError> {
    Arc::get_mut(storage)
        .ok_or(ArchiveError::Identity)?
        .get_mut()
        .map_err(|_| ArchiveError::Identity)
}

pub(super) struct BorrowedStdout<'a> {
    pub operation: &'a Operation,
    pub physical_child: &'a str,
    pub queued: &'a VecDeque<ChildFrame>,
    pub pending: Option<&'a ChildFrame>,
    pub events: Option<&'a RetainedEvents>,
    pub assembly: &'a FrameAssembly,
    pub reader_pending: Option<&'a ChildFrame>,
    pub chunk: &'a StdoutChunk,
}

pub(super) trait RecordSink {
    fn bytes(&mut self, record: EvidenceRecord<'_>, bytes: &[u8]) -> Result<(), ArchiveError>;
    fn json<T: Serialize + ?Sized>(
        &mut self,
        record: EvidenceRecord<'_>,
        value: &T,
    ) -> Result<(), ArchiveError>;
}

impl RecordSink for EvidenceCoverageBuilder<'_> {
    fn bytes(&mut self, record: EvidenceRecord<'_>, _: &[u8]) -> Result<(), ArchiveError> {
        self.include(&record)
    }
    fn json<T: Serialize + ?Sized>(
        &mut self,
        record: EvidenceRecord<'_>,
        _: &T,
    ) -> Result<(), ArchiveError> {
        self.include(&record)
    }
}

impl<W: Write> RecordSink for ArchiveEncoder<'_, W> {
    fn bytes(&mut self, record: EvidenceRecord<'_>, bytes: &[u8]) -> Result<(), ArchiveError> {
        self.bytes(&record, bytes)
    }
    fn json<T: Serialize + ?Sized>(
        &mut self,
        record: EvidenceRecord<'_>,
        value: &T,
    ) -> Result<(), ArchiveError> {
        self.json(&record, value)
    }
}

// References every retained event field without cloning Value/String bodies or
// constructing a second event queue. These are private diagnostic records.
#[derive(Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum BorrowedEvent<'a> {
    ModelStart(&'a str, &'a crate::operation::ConfirmationFrame),
    Init(
        &'a str,
        &'a Option<String>,
        &'a Option<String>,
        &'a [String],
        &'a Option<bool>,
    ),
    Entry(&'a cna_protocol::TranscriptEntry),
    Context(u64),
    Compacted(&'a str, &'a Option<u64>),
    Quota(&'a super::QuotaReading),
    TurnDone(&'a super::TurnOutcome),
}

impl<'a> From<&'a StreamEvent> for BorrowedEvent<'a> {
    fn from(event: &'a StreamEvent) -> Self {
        match event {
            StreamEvent::ModelStart { session_id, frame } => Self::ModelStart(session_id, frame),
            StreamEvent::Init {
                session_id,
                model,
                cli_version,
                tools,
                mcp_connected,
            } => Self::Init(session_id, model, cli_version, tools, mcp_connected),
            StreamEvent::Entry(value) => Self::Entry(value),
            StreamEvent::Context(value) => Self::Context(*value),
            StreamEvent::Compacted {
                trigger,
                pre_tokens,
            } => Self::Compacted(trigger, pre_tokens),
            StreamEvent::Quota(value) => Self::Quota(value),
            StreamEvent::TurnDone(value) => Self::TurnDone(value),
        }
    }
}

impl BorrowedStdout<'_> {
    fn validate(&self) -> Result<(), ArchiveError> {
        if self.chunk.count > self.chunk.bytes.len() || self.chunk.cursor > self.chunk.count {
            return Err(ArchiveError::Identity);
        }
        for frame in self
            .queued
            .iter()
            .chain(self.pending)
            .chain(self.events.map(|events| &events.raw))
            .chain(self.reader_pending)
        {
            if !frame.operation.same_instance(self.operation) {
                return Err(ArchiveError::Identity);
            }
        }
        // Preflight EVERY retained suffix before emitting even its raw prefix.
        // Nested protocol Values have no finite type-only recursion bound.
        if let Some(events) = self.events {
            for event in &events.events {
                if let StreamEvent::Entry(entry) = event {
                    crate::failed_evidence::check_entry_shape(entry)?;
                }
            }
        }
        match self.chunk.operation.as_ref() {
            Some(operation) if !operation.same_instance(self.operation) => {
                Err(ArchiveError::Identity)
            }
            None if self.chunk.count != 0
                || self.chunk.cursor != 0
                || self.chunk.read_uncertain
                || self.chunk.seen_accounted =>
            {
                Err(ArchiveError::Identity)
            }
            _ => Ok(()),
        }
    }

    fn record<'a>(
        &'a self,
        operation: &'a Operation,
        part: &'static str,
        index: usize,
        origin: Option<&'a CaptureOrigin>,
    ) -> Result<EvidenceRecord<'a>, ArchiveError> {
        Ok(EvidenceRecord {
            operation,
            category: EvidenceCategory::Stdout,
            part,
            index: u64::try_from(index).map_err(|_| ArchiveError::Fit)?,
            physical_child: Some(self.physical_child),
            origin,
        })
    }

    /// Both passes share this exact order over immutable borrowed storage.
    /// Multiple retained copies remain distinct records; consumed read prefixes
    /// are not silently omitted or used to manufacture a new owner/origin.
    pub(super) fn visit(&self, sink: &mut impl RecordSink) -> Result<(), ArchiveError> {
        self.validate()?;
        for (index, frame) in self.queued.iter().enumerate() {
            sink.bytes(
                self.record(
                    &frame.operation,
                    "queued_frame",
                    index,
                    frame.origin.as_ref(),
                )?,
                frame.text.as_bytes(),
            )?;
        }
        for (part, frame) in [
            ("pending_frame", self.pending),
            ("reader_pending", self.reader_pending),
        ] {
            if let Some(frame) = frame {
                sink.bytes(
                    self.record(&frame.operation, part, 0, frame.origin.as_ref())?,
                    frame.text.as_bytes(),
                )?;
            }
        }
        if let Some(events) = self.events {
            let raw = &events.raw;
            sink.bytes(
                self.record(&raw.operation, "parser_raw", 0, raw.origin.as_ref())?,
                raw.text.as_bytes(),
            )?;
            for (index, event) in events.events.iter().enumerate() {
                sink.json(
                    self.record(&raw.operation, "parser_suffix", index, raw.origin.as_ref())?,
                    &BorrowedEvent::from(event),
                )?;
            }
        }
        if !self.assembly.0.is_empty() {
            // Assembly has no per-byte capture witness; preserve unknown origin.
            sink.bytes(
                self.record(self.operation, "assembly", 0, None)?,
                &self.assembly.0,
            )?;
        }
        if let Some(operation) = self.chunk.operation.as_ref() {
            sink.json(
                self.record(operation, "read_state", 0, None)?,
                &(
                    self.chunk.count,
                    self.chunk.cursor,
                    self.chunk.read_uncertain,
                    self.chunk.seen_accounted,
                ),
            )?;
            if self.chunk.count != 0 {
                sink.bytes(
                    self.record(operation, "read_bytes", 0, None)?,
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

/// Stack control only: exercise the actual borrowed event wrapper and JSONL
/// encoder without constructing an output body or claiming physical joins.
#[cfg(test)]
pub(super) fn measure_stack_fixture(
    operation: &Operation,
    event: &StreamEvent,
) -> Result<(crate::failed_evidence::ArchiveMeasurement, usize), ArchiveError> {
    struct Output {
        meter: crate::failed_evidence::ArchiveMeter,
        bytes: usize,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let next = self
                .bytes
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("stack fixture output length overflow"))?;
            self.meter.write_all(bytes)?;
            self.bytes = next;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let StreamEvent::Entry(entry) = event else {
        return Err(ArchiveError::Identity);
    };
    crate::failed_evidence::check_entry_shape(entry)?;
    let record = EvidenceRecord {
        operation,
        category: EvidenceCategory::Stdout,
        part: "parser_suffix",
        index: 0,
        physical_child: None,
        origin: None,
    };
    let mut coverage = EvidenceCoverageBuilder::new(operation)?;
    coverage.include(&record)?;
    let mut output = Output {
        meter: crate::failed_evidence::ArchiveMeter::new(),
        bytes: 0,
    };
    let mut encoder = ArchiveEncoder::new(operation, coverage.finish()?, &mut output)?;
    encoder.json(&record, &BorrowedEvent::from(event))?;
    let encoded = encoder.finish()?;
    let bytes = output.bytes;
    if encoded != output.meter.finish()? {
        return Err(ArchiveError::Mismatch);
    }
    Ok((encoded, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failed_evidence::CampaignArchive;
    use crate::operation::Allowance;
    use std::time::Duration;
    use tokio::time::Instant;

    const CHILD: &str = "33333333-3333-4333-8333-333333333333";
    fn operation() -> Operation {
        let root = std::env::temp_dir().join("synthetic-stdout-fragments-no-io");
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
    fn frame(operation: &Operation, text: &str) -> ChildFrame {
        ChildFrame {
            text: text.into(),
            origin: None,
            operation: operation.clone(),
        }
    }

    #[test]
    fn overdepth_retained_suffix_refuses_before_any_borrowed_prefix_callback() {
        struct CountingSink(usize);
        impl RecordSink for CountingSink {
            fn bytes(&mut self, _: EvidenceRecord<'_>, _: &[u8]) -> Result<(), ArchiveError> {
                self.0 += 1;
                Ok(())
            }
            fn json<T: Serialize + ?Sized>(
                &mut self,
                _: EvidenceRecord<'_>,
                _: &T,
            ) -> Result<(), ArchiveError> {
                self.0 += 1;
                Ok(())
            }
        }
        let original = operation();
        let mut args = serde_json::Value::Null;
        for _ in 0..=crate::failed_evidence::MAX_ARCHIVE_JSON_CONTAINERS {
            args = serde_json::Value::Array(vec![args]); // Fixture construction only.
        }
        let queued = VecDeque::from([frame(&original, "canonical queued prefix")]);
        let events = RetainedEvents {
            raw: frame(&original, "canonical parser raw"),
            events: VecDeque::from([
                StreamEvent::Context(19),
                StreamEvent::Entry(cna_protocol::TranscriptEntry::ToolCall {
                    call_id: "unaccepted-original".into(),
                    tool: "observe".into(),
                    args,
                }),
            ]),
        };
        let queued_pointer = queued[0].text.as_ptr();
        let raw_pointer = events.raw.text.as_ptr();
        let charged = original.captured_bytes();
        original.overflow();
        let assembly = FrameAssembly(Vec::new());
        let chunk = StdoutChunk::empty();
        let view = BorrowedStdout {
            operation: &original,
            physical_child: CHILD,
            queued: &queued,
            pending: None,
            events: Some(&events),
            assembly: &assembly,
            reader_pending: None,
            chunk: &chunk,
        };
        let mut sink = CountingSink(0);
        assert!(matches!(view.visit(&mut sink), Err(ArchiveError::Fit)));
        assert_eq!(sink.0, 0); // EVERY suffix checked before even the queued raw prefix.
        assert_eq!(queued.len(), 1);
        assert_eq!(events.events.len(), 2);
        assert_eq!(queued[0].text.as_ptr(), queued_pointer);
        assert_eq!(events.raw.text.as_ptr(), raw_pointer);
        assert_eq!(original.captured_bytes(), charged);
    }

    #[test]
    fn every_stdout_fragment_and_unaccepted_event_suffix_is_borrowed_without_consumption() {
        let original = operation();
        let queued = VecDeque::from([frame(&original, "queued")]);
        let pending = frame(&original, "pending");
        let reader_pending = frame(&original, "reader pending");
        let events = RetainedEvents {
            raw: frame(&original, "original parser raw"),
            events: VecDeque::from([
                StreamEvent::Context(19),
                StreamEvent::Compacted {
                    trigger: "captured λ".into(),
                    pre_tokens: Some(17),
                },
            ]),
        };
        let assembly = FrameAssembly(vec![0xff, 0, b'\n']);
        let mut chunk = StdoutChunk::empty();
        chunk.operation = Some(original.clone());
        chunk.bytes[..4].copy_from_slice(&[0xfe, 0, b'a', b'b']);
        chunk.count = 4;
        chunk.cursor = 2; // preserve full retained buffer and its accepted-prefix position
        chunk.read_uncertain = true;
        let body_pointer = events.raw.text.as_ptr();
        let assembly_pointer = assembly.0.as_ptr();
        let charged = original.captured_bytes();
        original.overflow();
        let view = BorrowedStdout {
            operation: &original,
            physical_child: CHILD,
            queued: &queued,
            pending: Some(&pending),
            events: Some(&events),
            assembly: &assembly,
            reader_pending: Some(&reader_pending),
            chunk: &chunk,
        };
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        view.cover(&mut coverage).unwrap();
        let coverage = coverage.finish().unwrap();
        let mut output = Vec::new(); // serialized reference ONLY, no production body buffer
        let mut encoder = ArchiveEncoder::new(&original, coverage, &mut output).unwrap();
        view.encode(&mut encoder).unwrap();
        let measurement = encoder.finish().unwrap();
        let records: Vec<serde_json::Value> = output
            .split(|byte| *byte == b'\n')
            .filter(|row| !row.is_empty())
            .map(|row| serde_json::from_slice(row).unwrap())
            .collect();
        assert_eq!(records.len(), 10); // header + nine distinct canonical components
        assert_eq!(records[4]["evidence"]["part"], "parser_raw");
        assert_eq!(records[5]["payload"]["kind"], "context");
        assert_eq!(records[6]["payload"]["value"][0], "captured λ");
        assert_eq!(records[7]["payload"], "ff000a");
        assert_eq!(
            records[8]["payload"],
            serde_json::json!([4, 2, true, false])
        );
        assert_eq!(records[9]["payload"], "fe006162");
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
        assert_eq!(queued.len(), 1);
        assert_eq!(events.events.len(), 2);
        assert_eq!(events.raw.text.as_ptr(), body_pointer);
        assert_eq!(assembly.0.as_ptr(), assembly_pointer);
        assert_eq!(
            (chunk.count, chunk.cursor, chunk.read_uncertain),
            (4, 2, true)
        );
        assert_eq!(original.captured_bytes(), charged);
    }

    #[test]
    fn shared_or_checked_out_storage_and_stale_read_association_refuse() {
        let original = operation();
        let mut slot = Arc::new(Mutex::new(Some(StdoutChunk::empty())));
        let shared = slot.clone();
        assert!(unique_storage(&mut slot).is_err());
        drop(shared);
        let read = super::super::StdoutRead::begin(slot.clone(), original.clone()).unwrap();
        assert!(unique_storage(&mut slot).is_err()); // in-service reader still owns original
        drop(read); // restoration alone is NOT child/readers joins or settlement
        let chunk = unique_storage(&mut slot).unwrap().as_mut().unwrap();
        assert!(chunk.operation.as_ref().unwrap().same_instance(&original));
        assert!(chunk.read_uncertain);
        let queued = VecDeque::new();
        let assembly = FrameAssembly(Vec::new());
        let mut view = BorrowedStdout {
            operation: &original,
            physical_child: CHILD,
            queued: &queued,
            pending: None,
            events: None,
            assembly: &assembly,
            reader_pending: None,
            chunk,
        };
        let replacement = operation(); // identical text ID is not original instance
        view.operation = &replacement;
        assert!(matches!(view.validate(), Err(ArchiveError::Identity)));
        view.operation = &original;
        assert!(view.validate().is_ok());
        let mut malformed = StdoutChunk::empty();
        malformed.read_uncertain = true;
        assert!(matches!(
            BorrowedStdout {
                operation: &original,
                physical_child: CHILD,
                queued: &queued,
                pending: None,
                events: None,
                assembly: &assembly,
                reader_pending: None,
                chunk: &malformed,
            }
            .validate(),
            Err(ArchiveError::Identity)
        ));
        malformed.read_uncertain = false;
        malformed.count = malformed.bytes.len() + 1;
        assert!(matches!(
            BorrowedStdout {
                operation: &original,
                physical_child: CHILD,
                queued: &queued,
                pending: None,
                events: None,
                assembly: &assembly,
                reader_pending: None,
                chunk: &malformed,
            }
            .validate(),
            Err(ArchiveError::Identity)
        ));
    }

    #[test]
    fn interrupted_encoding_retains_full_assembly_and_parser_suffix() {
        use std::cell::Cell;
        struct RefusedWriter<'a> {
            armed: &'a Cell<bool>,
            written: &'a Cell<usize>,
        }
        impl Write for RefusedWriter<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.armed.get() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "synthetic refusal",
                    ));
                }
                self.written.set(self.written.get() + bytes.len());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let original = operation();
        let queued = VecDeque::new();
        let assembly = FrameAssembly(vec![0xff; 129]);
        let events = RetainedEvents {
            raw: frame(&original, "raw retained"),
            events: VecDeque::from([StreamEvent::Context(3)]),
        };
        let chunk = StdoutChunk::empty();
        let view = BorrowedStdout {
            operation: &original,
            physical_child: CHILD,
            queued: &queued,
            pending: None,
            events: Some(&events),
            assembly: &assembly,
            reader_pending: None,
            chunk: &chunk,
        };
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        view.cover(&mut coverage).unwrap();
        let coverage = coverage.finish().unwrap();
        let armed = Cell::new(false);
        let written = Cell::new(0);
        let mut writer = RefusedWriter {
            armed: &armed,
            written: &written,
        };
        let mut encoder = ArchiveEncoder::new(&original, coverage, &mut writer).unwrap();
        let header_bytes = written.get();
        assert!(header_bytes > 0);
        armed.set(true); // synthetic interruption AFTER header, during original visitor
        assert!(view.encode(&mut encoder).is_err());
        armed.set(false); // writer recovery cannot reopen the encoder's sticky refusal
        assert!(view.encode(&mut encoder).is_err());
        assert!(encoder.finish().is_err());
        assert_eq!(written.get(), header_bytes);
        assert_eq!(events.raw.text, "raw retained");
        assert_eq!(events.events.len(), 1);
        assert_eq!(assembly.0, vec![0xff; 129]);
        assert!(original.check().is_ok()); // no capture renewal/refund or release
    }
}
