//! Held owned writer custody. Legacy CampaignHandle::shutdown stays separate.
//!
//! All roots, N-byte row bodies, Arc backing, writer stack, blocking task and
//! transitive raw errors/results must be in the actual original F/G inventory
//! before construction/effects. No fit or positive-creation producer exists in
//! this module. It introduces no runtime discovery, helper task or Drop join.

use crate::owned_retained::{JoinObservation, RetainedJoin};
use rusqlite::{Connection, types::{Type, ValueRef}};
use std::sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, AtomicU8, Ordering}};
use tokio::{runtime::Handle, task::{JoinError, JoinHandle}, time::Instant};

#[derive(Debug)]
pub(crate) enum RowFailure {
    Storage(rusqlite::Error),
    Json(serde_json::Error),
    Title(crate::owned_row_envelope::TitleRefusal),
    MissingConnectionPath,
    ColumnType { column: usize, actual: Type },
    NegativeRevision(i64),
    Capacity { required: usize, available: usize },
    SizeOverflow,
}

impl From<rusqlite::Error> for RowFailure {
    fn from(original: rusqlite::Error) -> Self { Self::Storage(original) }
}

/// The original stored bytes, not a viewer projection or a reserialized Pins.
/// Metadata/pins must still be decoded/bound by the private evidence builder;
/// these raw rows alone do not prove canonical filesystem identity or CLEAN.
#[derive(Debug)]
pub(crate) struct StoredWriterRow<const N: usize> {
    bytes: [u8; N],
    ends: [usize; 5],
    pub(crate) revision: u64,
}

impl<const N: usize> StoredWriterRow<N> {
    pub(crate) fn field(&self, index: usize) -> Option<&[u8]> {
        let end = *self.ends.get(index)?;
        let start = if index == 0 { 0 } else { self.ends[index - 1] };
        self.bytes.get(start..end)
    }

    /// Read ONLY from the actual writer-owned connection at a quiescent point.
    /// Output copies never grow or allocate: the whole array is original F.
    /// SQLite statement/connection/error backing remains a separate fit duty,
    /// not an exemption inferred from using a fixed output buffer.
    pub(crate) fn capture(db: &Connection) -> Result<Self, RowFailure> {
        let mut statement = db.prepare(
            "SELECT meta,pins,revision,state_hash,status FROM campaign WHERE id=1",
        )?;
        let mut rows = statement.query([])?;
        let row = rows.next()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let revision: i64 = row.get(2)?;
        if revision < 0 { return Err(RowFailure::NegativeRevision(revision)); }
        let mut texts: [&[u8]; 5] = [&[]; 5];
        for (target, column) in [0_usize, 1, 3, 4].into_iter().enumerate() {
            texts[target] = match row.get_ref(column)? {
                ValueRef::Text(bytes) => bytes,
                other => return Err(RowFailure::ColumnType { column, actual: other.data_type() }),
            };
        }
        // This is the exact connection's path, not a basename-derived UUID or
        // proof against filesystem aliases. The final builder must validate the
        // trusted storage/archive identity separately before using any evidence.
        texts[4] = db.path().ok_or(RowFailure::MissingConnectionPath)?.as_bytes();
        crate::owned_row_envelope::fields(texts)?;
        Self::copy_fields(texts, revision as u64)
    }

    fn copy_fields(fields: [&[u8]; 5], revision: u64) -> Result<Self, RowFailure> {
        let required = fields.iter().try_fold(0_usize, |total, field| {
            total.checked_add(field.len()).ok_or(RowFailure::SizeOverflow)
        })?;
        if required > N { return Err(RowFailure::Capacity { required, available: N }); }
        let mut result = Self { bytes: [0; N], ends: [0; 5], revision };
        let mut at = 0;
        for (index, field) in fields.into_iter().enumerate() {
            let end = at + field.len(); // checked complete sum above
            result.bytes[at..end].copy_from_slice(field);
            result.ends[index] = end;
            at = end;
        }
        Ok(result)
    }
}

const BLOCKED: u8 = 0;
const GO: u8 = 1;
const CANCELLED: u8 = 2;

#[derive(Debug)]
pub(crate) enum PreparedPathFailure { Utf8(std::str::Utf8Error), Empty }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartBindingFailure {
    PreparedUnavailable,
    InitialUnavailable,
    ChangedField { index: usize },
    ChangedRevision { prepared: u64, initial: u64 },
}

/// A single producer writes each original exactly once. Canonical snapshot
/// errors stay here while the standard writer's own raw result stays in Join.
pub(crate) struct WriterRecord<const N: usize> {
    start: AtomicU8,
    guarded_connection: OnceLock<()>,
    attempt_path: OnceLock<std::path::PathBuf>,
    connection_custody: OnceLock<Arc<cna_seats::seat_host::CampaignFileCustody>>,
    connection_provenance: OnceLock<Result<(), ConnectionProvenanceRefusal>>,
    pair_ack: OnceLock<Result<(), std::io::ErrorKind>>,
    initial_path: OnceLock<Result<(), RowFailure>>,
    prepared: OnceLock<Result<StoredWriterRow<N>, RowFailure>>,
    prepared_meta: OnceLock<Result<cna_protocol::CampaignMeta, serde_json::Error>>,
    prepared_pins: OnceLock<Result<crate::Pins, serde_json::Error>>,
    prepared_path: OnceLock<Result<std::path::PathBuf, PreparedPathFailure>>,
    initial: OnceLock<Result<StoredWriterRow<N>, RowFailure>>,
    start_binding: OnceLock<Result<(), StartBindingFailure>>,
    terminal: OnceLock<Result<StoredWriterRow<N>, RowFailure>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionProvenanceRefusal { MissingReserved, MissingActive, DifferentRoot }

impl<const N: usize> WriterRecord<N> {
    // Intent only; never file identity/member/fit evidence. Set before dispatch.
    pub(crate) fn require_guarded_connection(&self) { self.guarded_connection.get_or_init(|| ()); }
    pub(crate) fn guarded_connection_required(&self) -> bool { self.guarded_connection.get().is_some() }

    /// Original recovery input moves here; guarded fresh preparation
    /// stores its ONCE generated path here. It is installed before the custody gate notification.
    pub(crate) fn retain_attempt_path(&self, original: std::path::PathBuf)
        -> Result<(), std::path::PathBuf>
    { self.attempt_path.set(original) }

    pub(crate) fn attempt_path(&self) -> Option<&std::path::Path> {
        self.attempt_path.get().map(|original| original.as_path())
    }

    pub(crate) fn install_connection_custody(&self,
        original: Arc<cna_seats::seat_host::CampaignFileCustody>,
    ) -> Result<(), Arc<cna_seats::seat_host::CampaignFileCustody>> {
        self.connection_custody.set(original)
    }

    pub(crate) fn connection_custody(&self)
        -> Option<&Arc<cna_seats::seat_host::CampaignFileCustody>>
    { self.connection_custody.get() }

    /// Only the actual guarded Campaign constructor/capture may supply
    /// its pre-open Arc here; no public setter/path tuple or retrospective proof.
    /// A missing/legacy Arc stays a sticky refusal; membership/GO remain separately closed.
    pub(crate) fn capture_connection_provenance(&self,
        active: Option<&Arc<cna_seats::seat_host::CampaignFileCustody>>,
    ) {
        self.connection_provenance.get_or_init(|| {
            let reserved = self.connection_custody.get().ok_or(ConnectionProvenanceRefusal::MissingReserved)?;
            let active = active.ok_or(ConnectionProvenanceRefusal::MissingActive)?;
            if !Arc::ptr_eq(reserved, active) { return Err(ConnectionProvenanceRefusal::DifferentRoot); }
            Ok(())
        });
    }

    pub(crate) fn connection_provenance(&self) -> Option<&Result<(), ConnectionProvenanceRefusal>> {
        self.connection_provenance.get()
    }

    pub(crate) fn pair_ack(&self) -> &OnceLock<Result<(), std::io::ErrorKind>> { &self.pair_ack }

    /// Before this is called the complete actual Arc/row/task graph must have
    /// been prepaid. No caller-byte estimate or cleanup admission is performed.
    pub(crate) fn new_prepaid() -> Arc<Self> {
        Arc::new(Self {
            start: AtomicU8::new(BLOCKED),
            guarded_connection: OnceLock::new(),
            attempt_path: OnceLock::new(),
            connection_custody: OnceLock::new(),
            connection_provenance: OnceLock::new(),
            pair_ack: OnceLock::new(),
            initial_path: OnceLock::new(),
            prepared: OnceLock::new(),
            prepared_meta: OnceLock::new(),
            prepared_pins: OnceLock::new(),
            prepared_path: OnceLock::new(),
            initial: OnceLock::new(),
            start_binding: OnceLock::new(),
            terminal: OnceLock::new(),
        })
    }

    /// Called by the actual standard writer before its command/automatic loop.
    /// Park/unpark is level-retained; no fresh notification or waiter allocation.
    pub(crate) fn await_owner_publication(&self) -> bool {
        loop {
            match self.start.load(Ordering::Acquire) {
                BLOCKED => std::thread::park(),
                GO => return true,
                _ => return false,
            }
        }
    }

    /// Concrete factory owns this SAME connection before moving it into the
    /// original standard worker. These are original retained results, not
    /// projected metadata, basename-derived IDs or reconstructed serde errors.
    /// All THREE full row bodies plus decoded children/serde errors/path backing
    /// are original F/G obligations; neither this read nor the journal J waives
    /// accounting for the campaign connection/statement or these Rust values.
    /// Initial filename admission is retained before the worker is published.
    /// This is NOT the after-gate initial row snapshot, which stays in place.
    pub(crate) fn capture_initial_path(&self, db: &Connection) -> bool {
        self.initial_path.get_or_init(|| crate::owned_row_envelope::path(db.path())).is_ok()
    }

    pub(crate) fn initial_path(&self) -> Option<&Result<(), RowFailure>> { self.initial_path.get() }

    pub(crate) fn capture_prepared(&self, db: &Connection) {
        if let Ok(row) = self.prepared.get_or_init(|| StoredWriterRow::capture(db)) {
            self.prepared_meta.get_or_init(|| serde_json::from_slice(
                row.field(0).expect("fixed stored meta field"),
            ));
            self.prepared_pins.get_or_init(|| serde_json::from_slice(
                row.field(1).expect("fixed stored pins field"),
            ));
            self.prepared_path.get_or_init(|| {
                let bytes = row.field(4).expect("fixed connection path field");
                if bytes.is_empty() { return Err(PreparedPathFailure::Empty); }
                let text = std::str::from_utf8(bytes).map_err(PreparedPathFailure::Utf8)?;
                Ok(std::path::PathBuf::from(text))
            });
        }
    }

    pub(crate) fn prepared(&self) -> Option<&Result<StoredWriterRow<N>, RowFailure>> { self.prepared.get() }
    pub(crate) fn prepared_meta(&self) -> Option<&Result<cna_protocol::CampaignMeta, serde_json::Error>> { self.prepared_meta.get() }
    pub(crate) fn prepared_pins(&self) -> Option<&Result<crate::Pins, serde_json::Error>> { self.prepared_pins.get() }
    pub(crate) fn prepared_path(&self) -> Option<&Result<std::path::PathBuf, PreparedPathFailure>> { self.prepared_path.get() }
    pub(crate) fn start_binding(&self) -> Option<&Result<(), StartBindingFailure>> { self.start_binding.get() }

    pub(crate) fn capture_initial(&self, db: &Connection) -> bool {
        self.initial.get_or_init(|| StoredWriterRow::capture(db));
        self.start_binding.get_or_init(|| {
            let prepared = match self.prepared() {
                Some(Ok(original)) => original,
                _ => return Err(StartBindingFailure::PreparedUnavailable),
            };
            let initial = match self.initial() {
                Some(Ok(original)) => original,
                _ => return Err(StartBindingFailure::InitialUnavailable),
            };
            // No actor command can precede this first check after the gate.
            // Even state/status drift is refused; both complete originals stay.
            for index in 0..5 {
                if prepared.field(index) != initial.field(index) {
                    return Err(StartBindingFailure::ChangedField { index });
                }
            }
            if prepared.revision != initial.revision {
                return Err(StartBindingFailure::ChangedRevision {
                    prepared: prepared.revision, initial: initial.revision,
                });
            }
            Ok(())
        }).is_ok()
    }

    pub(crate) fn capture_terminal(&self, db: &Connection) {
        self.terminal.get_or_init(|| StoredWriterRow::capture(db));
    }

    pub(crate) fn initial(&self) -> Option<&Result<StoredWriterRow<N>, RowFailure>> {
        self.initial.get()
    }

    pub(crate) fn terminal(&self) -> Option<&Result<StoredWriterRow<N>, RowFailure>> {
        self.terminal.get()
    }
}

type BlockingWriterResult<T> = Result<std::thread::Result<T>, JoinError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriterRefusal { Occupied, MissingStandard, AlreadyDispatched, NotPublished }

/// Original outer owner; observers cannot clone this or take its std handle.
pub(crate) struct WriterJoin<T, const N: usize> {
    publication: Arc<WriterPublication<T, N>>,
    dispatched: bool,
    joining: RetainedJoin<JoinHandle<std::thread::Result<T>>, BlockingWriterResult<T>>,
}

impl<T: Send + 'static, const N: usize> WriterJoin<T, N> {
    pub(crate) fn new_prepaid() -> Self {
        Self {
            publication: Arc::new(WriterPublication {
                record: WriterRecord::new_prepaid(),
                standard: Mutex::new(None),
                claimed: AtomicBool::new(false),
            }),
            dispatched: false,
            joining: RetainedJoin::before_shutdown(),
        }
    }

    pub(crate) fn can_publish(&self) -> bool {
        !self.publication.claimed.load(Ordering::Acquire)
    }

    pub(crate) fn vacant_standard(&mut self) -> Option<VacantStandard<'_, T, N>> {
        self.publication.claim()
    }

    pub(crate) fn record(&self) -> &Arc<WriterRecord<N>> { &self.publication.record }

    /// Only a trusted creator gets this endpoint. It can publish the one actual
    /// standard handle, never take/observe/join it. The outer root already holds
    /// the SAME storage before the creator or its fallible work can begin.
    pub(crate) fn creator_publication(&self) -> Arc<WriterPublication<T, N>> {
        Arc::clone(&self.publication)
    }

    pub(crate) fn publish_standard(
        &mut self,
        original: std::thread::JoinHandle<T>,
    ) -> Result<(), (WriterRefusal, std::thread::JoinHandle<T>)> {
        let vacant = match self.publication.claim() {
            Some(vacant) => vacant,
            None => return Err((WriterRefusal::Occupied, original)),
        };
        vacant.publish(original);
        Ok(())
    }

    /// The actual original handle must already be installed in this outer root.
    /// The release is synchronous and occurs only once; duplicate release is not
    /// an effects retry. This is writer publication, not native child Route A GO.
    pub(crate) fn release(&self) -> Result<(), WriterRefusal> {
        self.publication.release()
    }

    pub(crate) fn cancel_before_release(&self) -> Result<(), WriterRefusal> {
        self.publication.record.start.compare_exchange(BLOCKED, CANCELLED, Ordering::Release, Ordering::Relaxed)
            .map_err(|_| WriterRefusal::AlreadyDispatched)?;
        let thread = self.publication.standard.lock().expect("writer publication")
            .as_ref().map(|handle| handle.thread().clone());
        if let Some(thread) = thread { thread.unpark(); }
        // No handle yet is a real in-flight creator, not a settled/absent writer.
        // Its eventual publication must observe CANCELLED and unpark to exit.
        Ok(())
    }

    /// Dispatch the EXISTING blocking std join through the explicit host runtime.
    /// Complete backing is an original-owner obligation BEFORE any dispatch.
    /// No new hidden supervisor or async join wrapper task is introduced.
    pub(crate) fn dispatch_join(&mut self, runtime: &Handle) -> Result<(), WriterRefusal> {
        if self.dispatched { return Err(WriterRefusal::AlreadyDispatched); }
        if self.publication.record.start.load(Ordering::Acquire) == BLOCKED {
            // A refused/abandoned initialization may never have released GO.
            // Do not move away the only unparking Thread while it is blocked.
            self.cancel_before_release()?;
        }
        let vacant = self.joining.vacant_blocking().ok_or(WriterRefusal::AlreadyDispatched)?;
        let original = self.publication.standard.lock().expect("writer publication")
            .take().ok_or(WriterRefusal::MissingStandard)?;
        // Guard dropped; no mutex spans dispatch, join polling or any await.
        vacant.dispatch(runtime, move || original.join());
        self.dispatched = true;
        Ok(())
    }

    pub(crate) async fn join_until(
        &mut self,
        deadline: Instant,
    ) -> Result<JoinObservation<'_, BlockingWriterResult<T>>, crate::owned_retained::PublicationRefusal> {
        self.joining.join_until(deadline).await
    }

    pub(crate) fn observe(&self) -> JoinObservation<'_, BlockingWriterResult<T>> {
        self.joining.observe()
    }

    pub(crate) fn join_dispatched(&self) -> bool { self.dispatched }

    /// Negative setup guard, NOT a filesystem observation or positive member.
    /// Unique owner holds both original joins; parked writer owns the connection.
    pub(crate) fn require_pairing_window(&self) -> Result<(), WriterRefusal> {
        if self.dispatched || self.publication.record.start.load(Ordering::Acquire) != BLOCKED {
            return Err(WriterRefusal::AlreadyDispatched);
        }
        let original = self.publication.standard.lock().map_err(|_| WriterRefusal::NotPublished)?;
        match original.as_ref() {
            Some(handle) if !handle.is_finished() => Ok(()),
            _ => Err(WriterRefusal::NotPublished),
        }
    }
}

pub(crate) struct WriterPublication<T, const N: usize> {
    record: Arc<WriterRecord<N>>,
    standard: Mutex<Option<std::thread::JoinHandle<T>>>,
    claimed: AtomicBool,
}

impl<T, const N: usize> WriterPublication<T, N> {
    pub(crate) fn claim(&self) -> Option<VacantStandard<'_, T, N>> {
        self.claimed.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).ok()?;
        Some(VacantStandard { publication: self })
    }

    /// Trusted producer release only AFTER original standard custody is stored.
    /// Closed initialization cannot be reopened by late publication/release.
    pub(crate) fn release(&self) -> Result<(), WriterRefusal> {
        let thread = self.standard.lock().expect("writer publication")
            .as_ref().map(|handle| handle.thread().clone()).ok_or(WriterRefusal::NotPublished)?;
        self.record.start.compare_exchange(BLOCKED, GO, Ordering::Release, Ordering::Relaxed)
            .map_err(|_| WriterRefusal::AlreadyDispatched)?;
        thread.unpark(); // Outside the publication mutex.
        Ok(())
    }
}

pub(crate) struct VacantStandard<'a, T, const N: usize> {
    publication: &'a WriterPublication<T, N>,
}

impl<T, const N: usize> VacantStandard<'_, T, N> {
    pub(crate) fn record(&self) -> &Arc<WriterRecord<N>> { &self.publication.record }

    pub(crate) fn publish(self, original: std::thread::JoinHandle<T>) {
        // The atomic one-producer claim precedes effects. No observer or creator
        // can take this handle; only the outer cleanup owner can move it to join.
        // No callback, await, destructor or wake occurs while this guard is held.
        let thread = original.thread().clone();
        *self.publication.standard.lock().expect("first writer publication") = Some(original);
        if self.publication.record.start.load(Ordering::Acquire) == CANCELLED {
            thread.unpark(); // Stored handle precedes wake; mutex already dropped.
        }
    }
}

pub(crate) enum WriterStartFailure {
    Setup(crate::Error),
    Spawn(std::io::Error),
    Envelope(RowFailure),
}

impl From<RowFailure> for WriterStartFailure {
    fn from(original: RowFailure) -> Self { Self::Envelope(original) }
}
impl From<crate::owned_row_envelope::TitleRefusal> for WriterStartFailure {
    fn from(original: crate::owned_row_envelope::TitleRefusal) -> Self {
        Self::Envelope(RowFailure::Title(original))
    }
}

impl From<crate::Error> for WriterStartFailure {
    fn from(original: crate::Error) -> Self { Self::Setup(original) }
}

pub(crate) enum OwnedSpawnFailure {
    Refused(WriterRefusal),
    Start(WriterStartFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriterEvidenceRefusal {
    Pending,
    Unknown,
    BlockingJoinFailed,
    StandardPanicked,
    WriterFailed,
    PreparedUnavailable,
    PreparedMetaUnavailable,
    PreparedPinsUnavailable,
    PreparedPathUnavailable,
    PreparedProfileMismatch,
    StartBindingUnavailable,
    InitialUnavailable,
    TerminalUnavailable,
    ImmutableRowChanged,
    RevisionWentBackwards,
    InvalidStateHash,
}

/// Borrowed per-writer facts, NOT complete process/journal evidence. The final
/// private builder still needs actual namespace/archive/storage authority,
/// immutable typed member pins and frozen ALL-attempt/ALL-writer correspondence.
/// It must not turn this one positive row into a count/boolean CLEAN assertion.
pub(crate) struct WriterRowEvidence<'a, const N: usize> {
    pub(crate) prepared: &'a StoredWriterRow<N>,
    pub(crate) initial: &'a StoredWriterRow<N>,
    pub(crate) terminal: &'a StoredWriterRow<N>,
    pub(crate) original_join: &'a BlockingWriterResult<Result<(), crate::Error>>,
}

/// Actual decoded stored values; connection_path is NOT a canonical file/alias
/// identity. Namespace/archive/member authority and expected immutable pins must
/// be validated by the complete caller before registration/writer GO.
pub(crate) struct WriterPreparedIdentity<'a, const N: usize> {
    pub(crate) row: &'a StoredWriterRow<N>,
    pub(crate) meta: &'a cna_protocol::CampaignMeta,
    pub(crate) pins: &'a crate::Pins,
    pub(crate) connection_path: &'a std::path::Path,
}

impl<const N: usize> WriterJoin<Result<(), crate::Error>, N> {
    /// Available before writer GO; each failed decoder's original remains in
    /// WriterRecord. Tags reference those roots, never replace the actual cause.
    pub(crate) fn prepared_identity(&self) -> Result<WriterPreparedIdentity<'_, N>, WriterEvidenceRefusal> {
        use WriterEvidenceRefusal as Refused;
        let row = match self.record().prepared() {
            Some(Ok(original)) => original,
            _ => return Err(Refused::PreparedUnavailable),
        };
        let meta = match self.record().prepared_meta() {
            Some(Ok(original)) => original,
            _ => return Err(Refused::PreparedMetaUnavailable),
        };
        let pins = match self.record().prepared_pins() {
            Some(Ok(original)) => original,
            _ => return Err(Refused::PreparedPinsUnavailable),
        };
        let connection_path = match self.record().prepared_path() {
            Some(Ok(original)) => original.as_path(),
            _ => return Err(Refused::PreparedPathUnavailable),
        };
        if meta.rules_profile != pins.rules_profile { return Err(Refused::PreparedProfileMismatch); }
        Ok(WriterPreparedIdentity { row, meta, pins, connection_path })
    }

    /// The original result is already retained; this neither moves/clones it nor
    /// formats a raw failure. Refusal tags name originals still owned by self.
    pub(crate) fn terminal_rows(&self) -> Result<WriterRowEvidence<'_, N>, WriterEvidenceRefusal> {
        use WriterEvidenceRefusal as Refused;
        let original_join = match self.observe() {
            JoinObservation::Pending => return Err(Refused::Pending),
            JoinObservation::Unknown
            | JoinObservation::Returned { was_unknown: true, .. } => return Err(Refused::Unknown),
            JoinObservation::Returned { original, was_unknown: false } => original,
        };
        match original_join {
            Err(_) => return Err(Refused::BlockingJoinFailed),
            Ok(Err(_)) => return Err(Refused::StandardPanicked),
            Ok(Ok(Err(_))) => return Err(Refused::WriterFailed),
            Ok(Ok(Ok(()))) => {}
        }
        let prepared = self.prepared_identity()?.row;
        if !matches!(self.record().start_binding(), Some(Ok(()))) {
            return Err(Refused::StartBindingUnavailable);
        }
        let initial = match self.record().initial() {
            Some(Ok(original)) => original,
            _ => return Err(Refused::InitialUnavailable),
        };
        let terminal = match self.record().terminal() {
            Some(Ok(original)) => original,
            _ => return Err(Refused::TerminalUnavailable),
        };
        // Compare the actual unchanged stored meta/pins/path bytes. No JSON
        // reserialization, guessed UUID, lossy path normalization or projection.
        // These comparisons do NOT establish filesystem alias equivalence.
        for index in [0, 1, 4] {
            if initial.field(index) != terminal.field(index) {
                return Err(Refused::ImmutableRowChanged);
            }
        }
        if terminal.revision < initial.revision { return Err(Refused::RevisionWentBackwards); }
        let state_hash = terminal.field(2).ok_or(Refused::InvalidStateHash)?;
        if state_hash.len() != 64 || !state_hash.iter().all(u8::is_ascii_hexdigit) {
            return Err(Refused::InvalidStateHash);
        }
        Ok(WriterRowEvidence { prepared, initial, terminal, original_join })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future::Future, time::Duration};

    #[test]
    fn fixed_row_copy_keeps_exact_fields_and_refuses_without_truncation() {
        let fields: [&[u8]; 5] = [b"meta", b"pins", b"hash", b"status", b"path"];
        let row = StoredWriterRow::<24>::copy_fields(fields, 17).unwrap();
        for (index, original) in fields.into_iter().enumerate() {
            assert_eq!(row.field(index).unwrap(), original);
        }
        assert_eq!(row.revision, 17);
        assert!(matches!(StoredWriterRow::<21>::copy_fields(fields, 17),
            Err(RowFailure::Capacity { required: 22, available: 21 })));
        assert!(row.field(5).is_none());
    }

    #[tokio::test]
    async fn actual_standard_join_survives_cancelled_borrow_with_original_error() {
        // Actual std+Tokio handles, not a fit producer or native child fixture.
        let mut owner = WriterJoin::<Result<(), Box<[u8; 4]>>, 0>::new_prepaid();
        let record = Arc::clone(owner.record());
        let (release, wait) = std::sync::mpsc::sync_channel(0);
        let raw = Box::new([0_u8, 255, 13, 0]);
        let address = raw.as_ptr() as usize;
        let standard = std::thread::Builder::new().stack_size(1024 * 1024).spawn(move || {
            assert!(record.await_owner_publication());
            wait.recv().unwrap();
            Err(raw)
        }).unwrap();
        let standard_id = standard.thread().id();
        assert!(owner.publish_standard(standard).is_ok());
        assert_eq!(owner.publication.standard.lock().unwrap().as_ref().unwrap().thread().id(), standard_id);
        owner.release().unwrap();
        owner.dispatch_join(&Handle::current()).unwrap();
        let blocking_id = owner.joining.handle_for_poll().unwrap().id();
        let deadline = Instant::now() + Duration::from_secs(120);
        {
            let mut observer = std::pin::pin!(owner.join_until(deadline));
            std::future::poll_fn(|cx| {
                assert!(observer.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            }).await;
        }
        assert_eq!(owner.joining.handle_for_poll().unwrap().id(), blocking_id);
        release.send(()).unwrap();
        match owner.join_until(deadline).await.unwrap() {
            JoinObservation::Returned { original: Ok(Ok(Err(raw))), was_unknown: false } => {
                assert_eq!(raw.as_ptr() as usize, address);
                assert_eq!(**raw, [0, 255, 13, 0]);
            }
            _ => panic!("actual original std writer error was lost"),
        }
        assert!(matches!(owner.dispatch_join(&Handle::current()), Err(WriterRefusal::AlreadyDispatched)));
        assert!(matches!(owner.observe(), JoinObservation::Returned { .. }));
    }

    #[tokio::test]
    async fn refused_initialization_cancels_parked_writer_before_moving_join_handle() {
        let mut owner = WriterJoin::<bool, 0>::new_prepaid();
        let record = Arc::clone(owner.record());
        let standard = std::thread::Builder::new().stack_size(1024 * 1024)
            .spawn(move || record.await_owner_publication()).unwrap();
        assert!(owner.publish_standard(standard).is_ok());
        owner.dispatch_join(&Handle::current()).unwrap();
        assert!(matches!(owner.join_until(Instant::now() + Duration::from_secs(120)).await,
            Ok(JoinObservation::Returned { original: Ok(Ok(false)), was_unknown: false })));
        assert!(owner.record().initial().is_none());
        assert!(owner.record().terminal().is_none());
    }

    #[tokio::test]
    async fn late_standard_publication_after_close_can_never_release_writer_effects() {
        let mut owner = WriterJoin::<bool, 0>::new_prepaid();
        let publication = owner.creator_publication();
        let claimed = publication.claim().unwrap();
        owner.cancel_before_release().unwrap(); // Creator still has no handle.
        let record = Arc::clone(claimed.record());
        let standard = std::thread::Builder::new().stack_size(1024 * 1024)
            .spawn(move || record.await_owner_publication()).unwrap();
        claimed.publish(standard); // Publishes AND wakes the blocked cancelled writer.
        assert_eq!(publication.release(), Err(WriterRefusal::AlreadyDispatched));
        drop(publication); // Creator cancellation/drop does not own std custody.
        owner.dispatch_join(&Handle::current()).unwrap();
        assert!(matches!(owner.join_until(Instant::now() + Duration::from_secs(120)).await,
            Ok(JoinObservation::Returned { original: Ok(Ok(false)), was_unknown: false })));
    }
}
