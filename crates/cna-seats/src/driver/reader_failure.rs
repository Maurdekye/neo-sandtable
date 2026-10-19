//! Canonical reader failures, moved before task completion; never copied to a
//! replacement operation or treated as a child/reader join or archive ACK.

use std::{fmt, io};

use serde::{Serialize, Serializer};
use tokio::task::JoinError;

use super::archive_stdout::RecordSink;
use crate::{
    failed_evidence::{ArchiveError, EvidenceCategory, EvidenceRecord},
    operation::Operation,
};

// The existing OperationError Display literals, not an arbitrary text limit.
// This covers ONLY this producer's String body. Original F/live-count coverage
// and std::io::Error's private boxed wrapper remain separate obligations.
const OPERATION_ERROR_TEXT: [&str; 5] = [
    "no_allowance",
    "operation allowance exhausted",
    "capture_overflow",
    "operation already settled",
    "operation credit still owned",
];

pub(super) const OPERATION_ERROR_BODY_CAPACITY: usize = {
    let mut largest = 0;
    let mut index = 0;
    while index < OPERATION_ERROR_TEXT.len() {
        if OPERATION_ERROR_TEXT[index].len() > largest {
            largest = OPERATION_ERROR_TEXT[index].len();
        }
        index += 1;
    }
    largest
};

fn operation_body(error: crate::operation::OperationError) -> String {
    use crate::operation::OperationError;
    let index = match error {
        OperationError::NoAllowance => 0,
        OperationError::Expired => 1,
        OperationError::CaptureOverflow => 2,
        OperationError::Settled => 3,
        OperationError::CreditOwned => 4,
    };
    // Boxed str -> String preserves that exact-length allocation without a
    // formatting buffer or spare-capacity growth. No meter check/renewal here:
    // this refusal body requires original fixed cover, still unproved in SOURCE.
    Box::<str>::from(OPERATION_ERROR_TEXT[index]).into_string()
}

pub(super) fn operation_error(error: crate::operation::OperationError) -> io::Error {
    io::Error::other(operation_body(error))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReaderStream {
    Stdout,
    Stderr,
}

impl ReaderStream {
    fn index(self) -> usize {
        match self {
            Self::Stdout => 0,
            Self::Stderr => 1,
        }
    }

    pub(super) fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Stdout),
            1 => Some(Self::Stderr),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cause {
    Io(ReaderStream),
    Join(ReaderStream),
}

struct ReaderAssociation(Option<Operation>);

impl ReaderAssociation {
    fn same_instance(&self, expected: &Operation) -> bool {
        self.0
            .as_ref()
            .is_some_and(|operation| operation.same_instance(expected))
    }
}

struct ReaderSlot {
    operation: ReaderAssociation,
    io: Option<io::Error>,
    join: Option<JoinError>,
}

/// Exactly the two existing reader slots, no additional collection or lane.
/// The original error values stay here; tasks return only a fixed kind summary.
pub(super) struct ReaderFailures {
    slots: [ReaderSlot; 2],
    first: Option<Cause>,
    owned_pending: bool,
}

impl ReaderFailures {
    pub(super) fn admit_shared(
        operation: &Operation,
    ) -> Result<std::sync::Arc<std::sync::Mutex<Self>>, crate::operation::OperationError> {
        operation.charge(
            std::mem::size_of::<std::sync::Mutex<Self>>()
                + 2 * std::mem::size_of::<std::sync::atomic::AtomicUsize>(),
        )?;
        Ok(std::sync::Arc::new(std::sync::Mutex::new(Self::new(
            operation,
        ))))
    }

    fn new(operation: &Operation) -> Self {
        Self {
            slots: std::array::from_fn(|_| ReaderSlot {
                operation: ReaderAssociation(Some(operation.clone())),
                io: None,
                join: None,
            }),
            first: None,
            owned_pending: false,
        }
    }

    /// The SAME two slots staged before an original Operation exists. No error,
    /// additional result destination, body copy or placeholder State is created.
    pub(super) fn pending() -> Self {
        Self {
            slots: std::array::from_fn(|_| ReaderSlot {
                operation: ReaderAssociation(None),
                io: None,
                join: None,
            }),
            first: None,
            owned_pending: true,
        }
    }

    pub(super) fn pristine_pending(&self) -> bool {
        self.owned_pending
            && self.first.is_none()
            && self
                .slots
                .iter()
                .all(|slot| slot.operation.0.is_none() && slot.io.is_none() && slot.join.is_none())
    }

    /// Caller holds its existing failure lock, and checks allowance OUTSIDE it.
    /// Only fixed SAME State handles move here. Repeat binding never reopens.
    pub(super) fn bind_pending(&mut self, original: &Operation) -> Result<(), ArchiveError> {
        if !self.pristine_pending() {
            return Err(ArchiveError::Identity);
        }
        for slot in &mut self.slots {
            slot.operation = ReaderAssociation(Some(original.clone()));
        }
        Ok(())
    }

    pub(super) fn has_failure(&self) -> bool {
        self.first.is_some()
    }

    /// Owned checkout preflight only: no allocation, reassociation, meter or
    /// notification. Legacy begin/rebind semantics remain separate. Any raw
    /// reader failure closes BOTH streams' future checkout, without consuming
    /// its original cause or manufacturing another io::Error.
    pub(super) fn check_owned_checkout(&self, expected: &Operation) -> Result<(), ArchiveError> {
        if !self.owned_pending
            || self.first.is_some()
            || self.slots.iter().any(|slot| {
                !slot.operation.same_instance(expected) || slot.io.is_some() || slot.join.is_some()
            })
        {
            return Err(ArchiveError::Identity);
        }
        Ok(())
    }

    /// Called under the capture exclusion BEFORE checkout/await, not after a
    /// pipe error. Closed reader storage cannot be rebound to another instance.
    pub(super) fn begin(&mut self, stream: ReaderStream, operation: &Operation) -> io::Result<()> {
        let slot = &mut self.slots[stream.index()];
        if slot.io.is_some()
            || slot.join.is_some()
            || (self.owned_pending && !slot.operation.same_instance(operation))
        {
            return Err(io::ErrorKind::Other.into());
        }
        slot.operation = ReaderAssociation(Some(operation.clone()));
        Ok(())
    }

    pub(super) fn retain_io(
        &mut self,
        stream: ReaderStream,
        error: io::Error,
    ) -> Result<io::ErrorKind, io::Error> {
        let slot = &mut self.slots[stream.index()];
        if slot.io.is_some() || slot.operation.0.is_none() {
            return Err(error);
        } // Caller still owns a refused value.
        let kind = error.kind();
        slot.io = Some(error); // MOVE, no to_string/clone at an overflow boundary.
        self.first.get_or_insert(Cause::Io(stream));
        Ok(kind)
    }

    /// Owned stderr assembly may move a raw cause only into the SAME original
    /// reader slot. Foreign/occupied storage returns the ENTIRE error unchanged.
    /// This neither renews an allowance nor allocates a new failure cell.
    pub(super) fn retain_io_for_original(
        &mut self,
        stream: ReaderStream,
        expected: &Operation,
        error: io::Error,
    ) -> Result<io::ErrorKind, io::Error> {
        if !self.slots[stream.index()].operation.same_instance(expected) {
            return Err(error);
        }
        self.retain_io(stream, error)
    }

    pub(super) fn retain_join(
        &mut self,
        stream: ReaderStream,
        error: JoinError,
    ) -> Result<(), JoinError> {
        let slot = &mut self.slots[stream.index()];
        if slot.join.is_some() || slot.operation.0.is_none() {
            return Err(error);
        }
        slot.join = Some(error);
        self.first.get_or_insert(Cause::Join(stream));
        Ok(())
    }

    pub(super) fn has_io(&self, stream: ReaderStream) -> bool {
        self.slots[stream.index()].io.is_some()
    }

    pub(super) fn has_join(&self, stream: ReaderStream) -> bool {
        self.slots[stream.index()].join.is_some()
    }

    pub(super) fn first_kind(&self) -> Option<io::ErrorKind> {
        match self.first? {
            Cause::Io(stream) => self.slots[stream.index()].io.as_ref().map(io::Error::kind),
            Cause::Join(_) => Some(io::ErrorKind::Other),
        }
    }

    /// Compatibility display is a separately admitted copy. A failed admission
    /// leaves the canonical error intact, rather than allocating after overflow.
    pub(super) fn display_first(&self) -> Result<Option<String>, crate::operation::OperationError> {
        let Some(first) = self.first else {
            return Ok(None);
        };
        let stream = match first {
            Cause::Io(s) | Cause::Join(s) => s,
        };
        let slot = &self.slots[stream.index()];
        let operation = slot
            .operation
            .0
            .as_ref()
            .ok_or(crate::operation::OperationError::CreditOwned)?;
        let storage = super::event_storage::EventStorage(Some(operation));
        match first {
            Cause::Io(_) => storage.formatted(format_args!(
                "{}",
                slot.io.as_ref().expect("first io cause")
            )),
            Cause::Join(_) => storage.formatted(format_args!(
                "{}",
                slot.join.as_ref().expect("first join cause")
            )),
        }
        .map(Some)
    }

    pub(super) fn visit_archive(
        &self,
        original: &Operation,
        child: &str,
        sink: &mut impl RecordSink,
    ) -> Result<(), ArchiveError> {
        if self.owned_pending
            && self
                .slots
                .iter()
                .any(|slot| !slot.operation.same_instance(original))
        {
            return Err(ArchiveError::Identity); // Unbound/foreign is not empty evidence.
        }
        // Boxed I/O errors and panic payloads can contain arbitrary formatter
        // callbacks and hidden storage. Refuse before any record, without calling
        // their Display/Debug implementation, and retain the original values.
        for slot in &self.slots {
            if (slot.io.is_some() || slot.join.is_some()) && !slot.operation.same_instance(original)
                || slot
                    .io
                    .as_ref()
                    .is_some_and(|error| error.get_ref().is_some())
                || slot.join.as_ref().is_some_and(JoinError::is_panic)
            {
                return Err(ArchiveError::Identity);
            }
        }
        // These are fixed diagnostic facts, not complete error custody. The
        // separate release preflight still refuses every retained reader error.
        for stream in [ReaderStream::Stdout, ReaderStream::Stderr] {
            let slot = &self.slots[stream.index()];
            if let Some(error) = slot.io.as_ref() {
                sink.json(
                    EvidenceRecord {
                        operation: slot.operation.0.as_ref().ok_or(ArchiveError::Identity)?,
                        category: EvidenceCategory::Diagnostic,
                        part: match stream {
                            ReaderStream::Stdout => "stdout_reader_io",
                            ReaderStream::Stderr => "stderr_reader_io",
                        },
                        index: stream.index() as u64,
                        physical_child: Some(child),
                        origin: None,
                    },
                    &(
                        stream,
                        self.first == Some(Cause::Io(stream)),
                        ErrorKindText(error.kind()),
                        error.raw_os_error(),
                        "standard_io_error",
                    ),
                )?;
            }
            if let Some(error) = slot.join.as_ref() {
                sink.json(
                    EvidenceRecord {
                        operation: slot.operation.0.as_ref().ok_or(ArchiveError::Identity)?,
                        category: EvidenceCategory::Diagnostic,
                        part: match stream {
                            ReaderStream::Stdout => "stdout_reader_join",
                            ReaderStream::Stderr => "stderr_reader_join",
                        },
                        index: stream.index() as u64,
                        physical_child: Some(child),
                        origin: None,
                    },
                    &(
                        stream,
                        self.first == Some(Cause::Join(stream)),
                        error.is_cancelled(),
                        "reader_task_cancelled",
                    ),
                )?;
            }
        }
        Ok(())
    }
}

struct ErrorKindText(io::ErrorKind);
impl fmt::Display for ErrorKindText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}", self.0)
    }
}
impl Serialize for ErrorKindText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failed_evidence::{ArchiveEncoder, CampaignArchive, EvidenceCoverageBuilder};
    use crate::operation::{Allowance, OPERATION_BYTES, OperationError};
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::time::Instant;

    const CHILD: &str = "33333333-3333-4333-8333-333333333333";
    fn operation() -> Operation {
        let root = std::env::temp_dir().join("synthetic-reader-failure-archive-no-io");
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

    #[test]
    fn reader_failure_archive_borrows_both_original_errors_and_preserves_first_cause() {
        let original = operation();
        let mut shared = ReaderFailures::admit_shared(&original).unwrap();
        let failures = super::super::archive_stdout::unique_storage(&mut shared).unwrap();
        failures
            .retain_io(ReaderStream::Stderr, io::ErrorKind::BrokenPipe.into())
            .unwrap();
        failures
            .retain_io(ReaderStream::Stdout, io::Error::from_raw_os_error(5))
            .unwrap();
        let retained_pointer = std::ptr::from_ref(failures.slots[1].io.as_ref().unwrap());
        let charged = original.captured_bytes();
        original.overflow();
        assert!(matches!(
            failures.display_first(),
            Err(OperationError::CaptureOverflow)
        ));
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        failures
            .visit_archive(&original, CHILD, &mut coverage)
            .unwrap();
        let coverage = coverage.finish().unwrap();
        let mut output = Vec::new(); // Synthetic reference serialization, no file I/O.
        let mut encoder = ArchiveEncoder::new(&original, coverage, &mut output).unwrap();
        failures
            .visit_archive(&original, CHILD, &mut encoder)
            .unwrap();
        let measurement = encoder.finish().unwrap();
        let rows: Vec<serde_json::Value> = output
            .split(|byte| *byte == b'\n')
            .filter(|row| !row.is_empty())
            .map(|row| serde_json::from_slice(row).unwrap())
            .collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1]["payload"][0], "stdout");
        assert_eq!(rows[1]["payload"][1], false);
        assert_eq!(rows[1]["payload"][3], 5);
        assert_eq!(rows[2]["payload"][0], "stderr");
        assert_eq!(rows[2]["payload"][1], true);
        assert_eq!(rows[2]["payload"][2], "BrokenPipe");
        assert!(rows[2]["payload"][3].is_null());
        assert_eq!(rows[2]["payload"][4], "standard_io_error");
        assert!(
            rows[1..]
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
                &mut io::Cursor::new(&output),
            )
            .unwrap();
        assert!(std::ptr::eq(
            retained_pointer,
            std::ptr::from_ref(failures.slots[1].io.as_ref().unwrap())
        ));
        assert_eq!(failures.first, Some(Cause::Io(ReaderStream::Stderr)));
        assert!(
            failures
                .slots
                .iter()
                .all(|slot| slot.operation.same_instance(&original))
        );
        assert_eq!(original.captured_bytes(), charged);
    }

    #[derive(Debug)]
    struct OpaqueError(Arc<std::sync::atomic::AtomicUsize>);
    impl fmt::Display for OpaqueError {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            panic!("opaque error formatter must not be invoked");
        }
    }
    impl std::error::Error for OpaqueError {}

    #[test] // UNCOMPILED/UNRUN; no native reader or positive original Fit.
    fn pending_slots_refuse_unbound_and_foreign_raw_causes_then_bind_the_same_pair_once() {
        let mut pending = ReaderFailures::pending(); // Actual slots BEFORE fixture Op.
        assert!(pending.pristine_pending());
        assert_eq!(pending.slots.len(), 2);
        let original = operation();
        let foreign = operation();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let raw = io::Error::other(OpaqueError(calls.clone()));
        let pointer = std::ptr::from_ref(raw.get_ref().unwrap());
        let raw = pending.retain_io(ReaderStream::Stdout, raw).unwrap_err();
        assert!(std::ptr::eq(
            pointer,
            std::ptr::from_ref(raw.get_ref().unwrap())
        ));
        assert!(pending.pristine_pending());
        assert!(pending.begin(ReaderStream::Stdout, &original).is_err());
        let mut sink = CountingSink::default();
        assert_eq!(
            pending.visit_archive(&original, CHILD, &mut sink),
            Err(ArchiveError::Identity)
        );
        assert_eq!(sink.0, 0);
        let charged = original.captured_bytes();
        pending.bind_pending(&original).unwrap();
        assert!(!pending.pristine_pending());
        assert!(
            pending
                .slots
                .iter()
                .all(|slot| slot.operation.same_instance(&original))
        );
        assert_eq!(pending.bind_pending(&foreign), Err(ArchiveError::Identity));
        assert!(pending.begin(ReaderStream::Stdout, &foreign).is_err());
        let raw = pending
            .retain_io_for_original(ReaderStream::Stdout, &foreign, raw)
            .unwrap_err();
        assert!(std::ptr::eq(
            pointer,
            std::ptr::from_ref(raw.get_ref().unwrap())
        ));
        pending.begin(ReaderStream::Stdout, &original).unwrap();
        pending
            .retain_io_for_original(ReaderStream::Stdout, &original, raw)
            .unwrap();
        assert!(std::ptr::eq(
            pointer,
            std::ptr::from_ref(pending.slots[0].io.as_ref().unwrap().get_ref().unwrap())
        ));
        assert_eq!(pending.first, Some(Cause::Io(ReaderStream::Stdout)));
        assert!(pending.slots[1].io.is_none() && pending.slots[1].join.is_none());
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        // The opaque error remains held, not rendered or called complete custody.
    }

    #[test] // Legacy empty reader rebind stays distinct from owned pinning.
    fn legacy_bound_slots_keep_their_previous_empty_rebind_behavior() {
        let original = operation();
        let next = operation();
        let mut legacy = ReaderFailures::new(&original);
        assert!(!legacy.owned_pending && !legacy.pristine_pending());
        legacy.begin(ReaderStream::Stdout, &next).unwrap();
        assert!(legacy.slots[0].operation.same_instance(&next));
        assert!(legacy.slots[1].operation.same_instance(&original));
        assert_eq!(legacy.bind_pending(&original), Err(ArchiveError::Identity));
    }

    #[derive(Default)]
    struct CountingSink(usize);
    impl RecordSink for CountingSink {
        fn bytes(&mut self, _: EvidenceRecord<'_>, _: &[u8]) -> Result<(), ArchiveError> {
            self.0 += 1;
            Ok(())
        }
        fn json<T: Serialize + ?Sized>(
            &mut self,
            _: EvidenceRecord<'_>,
            value: &T,
        ) -> Result<(), ArchiveError> {
            self.0 += 1;
            serde_json::to_writer(io::sink(), value).unwrap();
            Ok(())
        }
    }

    #[test]
    fn opaque_io_refuses_before_any_archive_callback_and_retains_original_payload() {
        for stream in [ReaderStream::Stdout, ReaderStream::Stderr] {
            for closed in 0..3 {
                let original = operation();
                let mut shared = ReaderFailures::admit_shared(&original).unwrap();
                let failures = super::super::archive_stdout::unique_storage(&mut shared).unwrap();
                let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let error = io::Error::other(OpaqueError(calls.clone()));
                let pointer = std::ptr::from_ref(error.get_ref().unwrap());
                failures.retain_io(stream, error).unwrap();
                let other = match stream {
                    ReaderStream::Stdout => ReaderStream::Stderr,
                    ReaderStream::Stderr => ReaderStream::Stdout,
                };
                failures
                    .retain_io(other, io::ErrorKind::BrokenPipe.into())
                    .unwrap();
                let charged = original.captured_bytes();
                match closed {
                    1 => {
                        original.overflow();
                    }
                    2 => original.settle(), // Synthetic closure, never a join/custody ACK.
                    _ => {}
                }
                let first_check = original.check();
                let mut sink = CountingSink::default();
                assert!(failures.visit_archive(&original, CHILD, &mut sink).is_err());
                assert_eq!(sink.0, 0); // A late opaque slot prevents an earlier prefix.
                if closed != 0 {
                    assert!(failures.display_first().is_err());
                }
                assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
                assert!(std::ptr::eq(
                    pointer,
                    std::ptr::from_ref(
                        failures.slots[stream.index()]
                            .io
                            .as_ref()
                            .unwrap()
                            .get_ref()
                            .unwrap()
                    )
                ));
                assert_eq!(failures.first, Some(Cause::Io(stream)));
                assert!(
                    failures
                        .slots
                        .iter()
                        .all(|slot| slot.operation.same_instance(&original))
                );
                assert!(failures.begin(stream, &operation()).is_err());
                assert_eq!(original.captured_bytes(), charged);
                assert_eq!(original.check(), first_check);
            }
        }
    }

    #[test]
    fn checked_out_read_binds_failure_before_await_and_refuses_replacement_or_duplicate() {
        let original = operation();
        let replacement = operation();
        let failures = ReaderFailures::admit_shared(&original).unwrap();
        let capture = Mutex::new(original.clone());
        let chunk = Arc::new(Mutex::new(Some(super::super::StdoutChunk::empty())));
        let (before_read, mut read) = super::super::StdoutRead::begin_reader(
            chunk.clone(),
            &capture,
            &failures,
            ReaderStream::Stdout,
        )
        .unwrap();
        assert!(before_read.same_instance(&original));
        assert!(
            super::super::rebind_capture(&capture, replacement.clone(), || usize::from(
                super::super::stdout_chunk_owned(&chunk)
            ))
            .is_err()
        );
        read.returned(0);
        drop(read); // Restoring a guard is not reader/child join evidence.
        let mut failures = failures.lock().unwrap();
        failures
            .retain_io(ReaderStream::Stdout, io::ErrorKind::BrokenPipe.into())
            .unwrap();
        assert!(failures.begin(ReaderStream::Stdout, &replacement).is_err());
        assert!(failures.slots[0].operation.same_instance(&original));
        let refused = io::Error::other("second refused body".to_owned());
        let pointer = std::ptr::from_ref(refused.get_ref().unwrap());
        let refused = failures
            .retain_io(ReaderStream::Stdout, refused)
            .unwrap_err();
        assert!(std::ptr::eq(
            pointer,
            std::ptr::from_ref(refused.get_ref().unwrap())
        ));
        assert_eq!(failures.first_kind(), Some(io::ErrorKind::BrokenPipe));
        let mut coverage = EvidenceCoverageBuilder::new(&replacement).unwrap();
        assert!(
            failures
                .visit_archive(&replacement, CHILD, &mut coverage)
                .is_err()
        );
        assert!(coverage.finish().is_err());
    }

    #[tokio::test]
    async fn join_cancellation_is_retained_but_opaque_panic_is_never_covered_as_text() {
        let original = operation();
        let mut failures = ReaderFailures::new(&original); // Pure fixed stack fixture.
        let task = tokio::spawn(std::future::pending::<()>());
        task.abort();
        let canceled = task.await.unwrap_err(); // Actual original task ACK, no timer inference.
        failures
            .retain_join(ReaderStream::Stderr, canceled)
            .unwrap();
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        failures
            .visit_archive(&original, CHILD, &mut coverage)
            .unwrap();
        let coverage = coverage.finish().unwrap();
        let mut output = Vec::new(); // Synthetic serialization only; no archive I/O.
        let mut encoder = ArchiveEncoder::new(&original, coverage, &mut output).unwrap();
        failures
            .visit_archive(&original, CHILD, &mut encoder)
            .unwrap();
        encoder.finish().unwrap();
        let row: serde_json::Value =
            serde_json::from_slice(output.split(|byte| *byte == b'\n').nth(1).unwrap()).unwrap();
        assert_eq!(
            row["payload"],
            serde_json::json!(["stderr", true, true, "reader_task_cancelled"])
        );
        assert!(failures.slots[1].join.as_ref().unwrap().is_cancelled());
        let panicking: tokio::task::JoinHandle<()> =
            tokio::spawn(async { panic!("original opaque fixture payload") });
        let panic = panicking.await.unwrap_err();
        assert!(panic.is_panic());
        failures.retain_join(ReaderStream::Stdout, panic).unwrap();
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        assert!(
            failures
                .visit_archive(&original, CHILD, &mut coverage)
                .is_err()
        );
        assert!(coverage.finish().is_err()); // No diagnostic prefix accepted.
        assert!(failures.slots[0].join.as_ref().unwrap().is_panic());
        assert!(failures.slots[1].join.as_ref().unwrap().is_cancelled());
        assert!(failures.begin(ReaderStream::Stdout, &operation()).is_err());
    }

    #[test]
    fn reader_failure_shared_metadata_overflow_refuses_before_allocation() {
        let original = operation();
        let needed = std::mem::size_of::<Mutex<ReaderFailures>>()
            + 2 * std::mem::size_of::<std::sync::atomic::AtomicUsize>();
        original
            .charge(OPERATION_BYTES - original.captured_bytes() - needed + 1)
            .unwrap();
        let charged = original.captured_bytes();
        assert!(matches!(
            ReaderFailures::admit_shared(&original),
            Err(OperationError::CaptureOverflow)
        ));
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(original.check(), Err(OperationError::CaptureOverflow));
    }
}
