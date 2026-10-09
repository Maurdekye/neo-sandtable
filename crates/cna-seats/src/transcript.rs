//! Transcript capture (`docs/protocol.md` §3 `transcript`).
//!
//! Every CLI's stream is converted into [`cna_protocol::TranscriptEntry`] values. Drivers hand
//! them to a [`TranscriptSink`], which delivers them *in order* to a [`TranscriptStore`] — the
//! campaign server's persistence in production, [`LocalTranscript`] in tests and demos. The store
//! assigns the per-seat `tseq`, so numbering survives a CLI restart, a session resume or a reseed
//! (the sink belongs to the run, not to a CLI process).
//!
//! Accepted entries keep their FIFO and recovery mirror. Admission is synchronous and fail
//! closed under the encoded capture caps; no oversized answer is split, spilled or dropped.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cna_core::ids::SeatId;
use cna_protocol::ServerMessage;
pub use cna_protocol::TranscriptEntry;
use tokio::sync::{broadcast, mpsc, oneshot, Notify};
use crate::host::{ConfirmedIntent, ConfirmedPostcommit, HostIdentity, MarkerWitness, PreparedRoute, RouteCancellation, RouteState};
use crate::operation::{Allowance, Operation, OperationError};

/// RFC 3339 UTC time with millisecond precision, as the protocol's `at` field.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Where transcript entries end up.
#[async_trait]
pub trait TranscriptStore: Send + Sync + 'static {
    /// Persist one entry and return its per-seat `tseq`. Called sequentially, in capture order.
    async fn append(&self, seat: SeatId, at: String, entry: TranscriptEntry)
    -> Result<u64, String>;
}

/// A capture not yet confirmed by its store. Recovery outboxes use this local shape,
/// not the viewer protocol: numbering and alignment belong to the campaign writer.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct UnconfirmedEntry {
    pub seat: SeatId,
    pub at: String,
    pub entry: TranscriptEntry,
    /// Private capture provenance. Old outboxes remain unknown, never retagged on recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<crate::operation::CaptureOrigin>,
}

struct Capture {
    row: UnconfirmedEntry,
    bytes: usize,
    operation: Option<crate::operation::Operation>,
    ordinal: u64,
}

#[derive(Default)]
struct PendingCaptures {
    rows: std::collections::VecDeque<Arc<Capture>>,
    bytes: usize,
    legacy_overflow: bool,
    ordinal: u64,
    route_owned: bool,
    route_operation: Option<String>,
    marker_phase: u8,
}

struct WorkerRoute {
    stream: String,
    owner: HostIdentity,
    allowance: Allowance,
    state: Arc<Mutex<RouteState>>,
    changed: Arc<Notify>,
    cancel: Arc<RouteCancellation>,
    operation: Operation,
}

enum MarkerReply {
    Intent(oneshot::Sender<Result<ConfirmedIntent, String>>),
    Postcommit(oneshot::Sender<Result<ConfirmedPostcommit, String>>),
}

enum Item {
    Entry(Arc<Capture>),
    Flush(oneshot::Sender<()>),
    Prepare { route: Arc<WorkerRoute>, done: oneshot::Sender<PreparedRoute> },
    Marker { ordinal: u64, reply: MarkerReply },
}

struct DeliveryTask(tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>);

/// The cloneable handle drivers write to.
#[derive(Clone)]
pub struct TranscriptSink {
    tx: mpsc::UnboundedSender<Item>,
    pending: Arc<Mutex<PendingCaptures>>,
    delivery: Arc<DeliveryTask>,
    operation: Option<crate::operation::Operation>,
    stream: Arc<str>,
    prepare: mpsc::Sender<Item>,
    source_origin: Option<Option<crate::operation::CaptureOrigin>>,
}

impl WorkerRoute {
    // Never retain the state mutex across a wait or storage call. No timeout implies a join.
    async fn installed(&self) -> Result<(String, Arc<dyn crate::host::HostDeliveryRoute>), String> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.state.lock().expect("worker route");
                match &*state {
                    RouteState::Installed { token, delivery } => {
                        let bytes = crate::operation::encoded_len(&(token, &self.stream,
                            &self.owner.operation_id), crate::operation::COMMAND_BYTES)
                            .map_err(|_| self.operation.overflow().to_string())?;
                        self.operation.charge(bytes).map_err(|e| e.to_string())?;
                        return Ok((token.clone(), delivery.clone()));
                    }
                    RouteState::Released => return Err("route settled before installation".into()),
                    RouteState::Uncertain => return Err("route settlement uncertain".into()),
                    RouteState::Prepared => {}
                }
            }
            tokio::time::timeout_at(self.allowance.deadline(), changed).await
                .map_err(|_| "forced route installation timed out".to_string())?;
        }
    }

    async fn settled(&self) -> Result<(), String> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.state.lock().expect("worker route");
                match &*state {
                    RouteState::Released => return Ok(()),
                    RouteState::Uncertain => return Err("forced route settlement uncertain".into()),
                    _ => {}
                }
            }
            // Expiry requests abort; it cannot manufacture actor/storage settlement.
            if tokio::time::timeout_at(self.allowance.deadline(), changed).await.is_err() {
                self.cancel.request();
                return Err("forced route settlement timed out".into());
            }
        }
    }
}

impl TranscriptSink {
    /// Start the delivery task for `store`. Must be called inside a Tokio runtime.
    pub fn new(store: Arc<dyn TranscriptStore>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Item>();
        let (prepare, mut prepare_rx) = mpsc::channel::<Item>(1);
        let pending = Arc::new(Mutex::new(PendingCaptures::default()));
        let remaining = pending.clone();
        let stream: Arc<str> = uuid::Uuid::new_v4().to_string().into();
        let task = tokio::spawn(async move {
            let mut active: Option<Arc<WorkerRoute>> = None;
            let mut acknowledged_ordinal = 0u64;
            loop {
                // Separate capacity-one prepare handshake takes priority after the ACTUAL
                // current append ACK. Older buffered entries then use the installed route.
                let item = tokio::select! {
                    biased;
                    Some(item) = prepare_rx.recv(), if active.is_none() => item,
                    item = rx.recv() => match item { Some(item) => item, None => break },
                };
                match item {
                    Item::Entry(capture) => {
                        if acknowledged_ordinal.checked_add(1) != Some(capture.ordinal) {
                            if let Some(route) = &active { route.cancel.request(); }
                            if let Some(operation) = &capture.operation { operation.overflow(); }
                            return;
                        }
                        let mut delay = Duration::from_millis(50);
                        loop {
                            // The owned-store API requires a full clone on each issue/retry.
                            // Charge BEFORE copying; retry/ACK never refunds cumulative bytes.
                            if let Some(operation) = &capture.operation {
                                if operation.charge(capture.bytes).is_err() { return; }
                            }
                            let result = if let Some(route) = &active {
                                if route.operation.check().is_err() { route.cancel.request(); return; }
                                // Older/legacy captures keep unknown provenance, but their new
                                // actor-side full-row references still charge THIS route's meter.
                                if route.operation.charge(capture.bytes).is_err() { route.cancel.request(); return; }
                                let (_, delivery) = match route.installed().await {
                                    Ok(value) => value,
                                    Err(_) => { route.cancel.request(); return; }
                                };
                                // Borrow the canonical full row; original author remains unchanged.
                                delivery.append(&route.stream, capture.ordinal, &capture.row).await
                            } else {
                                store.append(capture.row.seat, capture.row.at.clone(), capture.row.entry.clone()).await
                            };
                            let Err(e) = result else { break; };
                            if let Some(route) = &active {
                                // Lost/error ACK may hide an accepted append. Never reroute/retry it.
                                route.cancel.request();
                                return;
                            }
                            eprintln!(
                                "transcript store refused an entry for {}: {e}; retrying",
                                capture.row.seat
                            );
                            tokio::time::sleep(delay).await;
                            delay = (delay * 2).min(Duration::from_secs(5));
                        }
                        // The receipt's u64 is per-seat tseq, NOT this shared stream ordinal.
                        // Only the actual successful append ACK advances this witness.
                        acknowledged_ordinal = capture.ordinal;
                        let mut pending = remaining.lock().expect("pending captures");
                        if let Some(row) = pending.rows.pop_front() { pending.bytes -= row.bytes; }
                    }
                    Item::Flush(done) => {
                        let _ = done.send(());
                    }
                    Item::Prepare { route, done } => {
                        let prepared = PreparedRoute {
                            stream: route.stream.clone(), owner: route.owner.clone(),
                            boundary: acknowledged_ordinal,
                            allowance: route.allowance, state: route.state.clone(),
                            changed: route.changed.clone(), cancel: route.cancel.clone(),
                            operation: route.operation.clone(),
                        };
                        active = Some(route.clone());
                        if done.send(prepared).is_err() { route.cancel.request(); return; }
                        // This queue boundary follows the actual normal in-flight ACK. Freeze
                        // the next issue until begin installs the actor route before its ACK.
                        if route.installed().await.is_err() {
                            let released = matches!(*route.state.lock().expect("worker route"), RouteState::Released);
                            if !released { route.cancel.request(); return; }
                            let mut pending = remaining.lock().expect("pending captures");
                            pending.route_owned = false;
                            pending.route_operation = None;
                            active = None;
                        }
                    }
                    Item::Marker { ordinal, reply } => {
                        let Some(route) = &active else { return; };
                        if ordinal != acknowledged_ordinal { route.cancel.request(); return; }
                        let (token, _) = match route.installed().await {
                            Ok(value) => value,
                            Err(_) => { route.cancel.request(); return; }
                        };
                        let marker = MarkerWitness { stream: route.stream.clone(), token,
                            owner: route.owner.clone(), ordinal };
                        match reply {
                            MarkerReply::Intent(done) => {
                                if done.send(Ok(ConfirmedIntent { marker })).is_err() { route.cancel.request(); }
                            }
                            MarkerReply::Postcommit(done) => {
                                if done.send(Ok(ConfirmedPostcommit { marker })).is_err() { route.cancel.request(); }
                                // Freeze after marker TWO: scoped final pending read belongs to
                                // actor settlement. Later siblings cannot delay that command.
                                if route.settled().await.is_err() { return; }
                                let mut pending = remaining.lock().expect("pending captures");
                                pending.route_owned = false;
                                pending.route_operation = None;
                                active = None;
                            }
                        }
                    }
                }
            }
        });
        Self {
            tx,
            pending,
            delivery: Arc::new(DeliveryTask(tokio::sync::Mutex::new(Some(task)))),
            operation: None,
            stream,
            prepare,
            source_origin: None,
        }
    }

    /// Queue one entry for a seat (never waits for storage; capture time is taken now).
    pub fn emit(&self, seat: SeatId, entry: TranscriptEntry) {
        let _ = self.try_emit_batch(seat, vec![entry]);
    }

    /// Explicit owned producer association; old mirror rows retain their original provenance.
    pub fn for_operation(&self, operation: crate::operation::Operation) -> Self {
        let mut sink = self.clone();
        sink.operation = Some(operation);
        sink.source_origin = None;
        sink
    }

    /// Preserve the reader's exact operation/frame association, including explicit UNKNOWN.
    /// Deriving multiple rows from one frame does not fabricate fresh source evidence ordinals.
    pub(crate) fn for_origin(&self, origin: Option<&crate::operation::CaptureOrigin>) -> Result<Self, OperationError> {
        self.charge_copy(&origin)?;
        let mut sink = self.clone();
        sink.source_origin = Some(origin.cloned());
        Ok(sink)
    }

    pub fn charge_copy(&self, value: &impl serde::Serialize) -> Result<(), crate::operation::OperationError> {
        let bytes = crate::operation::encoded_len(value, crate::operation::OPERATION_BYTES)
            .map_err(|error| {
                if let Some(operation) = &self.operation { operation.overflow(); }
                error
            })?;
        if let Some(operation) = &self.operation { operation.charge(bytes)?; }
        Ok(())
    }

    pub fn try_emit_batch(&self, seat: SeatId, entries: Vec<TranscriptEntry>) -> Result<(), crate::operation::OperationError> {
        use crate::operation::{DELIVERY_BYTES, MIRROR_BYTES, MIRROR_ENTRIES, OperationError};
        let mut rows = Vec::with_capacity(entries.len());
        let mut bytes = 0usize;
        for entry in entries {
            let origin = match &self.source_origin {
                Some(origin) => origin.clone(),
                None => self.operation.as_ref().map(|op| op.next_origin()).transpose()?,
            };
            let row = UnconfirmedEntry { seat, at: now_rfc3339(), entry, origin };
            let size = match crate::operation::encoded_len(&row, DELIVERY_BYTES) {
                Ok(size) => size,
                Err(error) => {
                    if let Some(op) = &self.operation { op.overflow(); }
                    else { self.pending.lock().expect("pending captures").legacy_overflow = true; }
                    return Err(error);
                }
            };
            bytes = bytes.checked_add(size).ok_or_else(|| self.signal_overflow())?;
            rows.push(Capture { row, bytes: size, operation: self.operation.clone(), ordinal: 0 });
        }
        // Hold this lock through enqueue so concurrent producers and the recovery mirror
        // have the same order. A failed enqueue remains visible in the mirror.
        let mut pending = self.pending.lock().expect("pending captures");
        let total = pending.bytes.checked_add(bytes);
        if total.is_none_or(|n| n > MIRROR_BYTES)
            || pending.rows.len().checked_add(rows.len()).is_none_or(|n| n > MIRROR_ENTRIES)
        {
            if let Some(op) = &self.operation { op.overflow(); }
            else { pending.legacy_overflow = true; }
            return Err(OperationError::CaptureOverflow);
        }
        if let Some(op) = &self.operation {
            // Canonical row + TWO newly created full logical-body references. Physical sharing
            // does not hide their conservative cumulative charge, and encoded caps are not RSS.
            op.charge(bytes.checked_mul(3).ok_or_else(|| op.overflow())?)?;
        }
        let last = pending.ordinal.checked_add(rows.len() as u64).ok_or_else(|| {
            if let Some(op) = &self.operation { op.overflow(); } else { pending.legacy_overflow = true; }
            OperationError::CaptureOverflow
        })?;
        pending.bytes = total.expect("checked capture sum");
        for mut capture in rows {
            pending.ordinal += 1;
            capture.ordinal = pending.ordinal;
            let capture = Arc::new(capture);
            pending.rows.push_back(capture.clone());
            let _ = self.tx.send(Item::Entry(capture));
        }
        debug_assert_eq!(pending.ordinal, last);
        Ok(())
    }

    fn signal_overflow(&self) -> OperationError {
        if let Some(op) = &self.operation { op.overflow(); }
        else { self.pending.lock().expect("pending captures").legacy_overflow = true; }
        OperationError::CaptureOverflow
    }

    /// Register cancellation before enqueueing begin. The returned capability exists only after
    /// the worker has joined its current normal append ACK and frozen the next issue.
    pub async fn prepare_route(&self, owner: HostIdentity, operation: Operation) -> Result<PreparedRoute, String> {
        operation.check().map_err(|e| e.to_string())?;
        if owner.operation_id != operation.id() { return Err("operation identity mismatch".into()); }
        let metadata = (&owner.operation_id, owner.seat.to_string(), owner.epoch,
            &owner.decision_id, owner.revision, self.stream.as_ref());
        let bytes = crate::operation::encoded_len(&metadata, crate::operation::PREPARE_BYTES)
            .map_err(|_| operation.overflow().to_string())?;
        operation.charge(bytes.checked_mul(8).ok_or_else(|| operation.overflow().to_string())?)
            .map_err(|e| e.to_string())?;
        // Reserve this handle's bounded cleanup writer-wake metadata while admission is
        // open. A later overflow may refuse begin but must not strand terminal actor ACK.
        let wake_handles = std::mem::size_of::<std::thread::Thread>().checked_mul(4)
            .filter(|n| *n <= crate::operation::PREPARE_BYTES)
            .ok_or_else(|| operation.overflow().to_string())?;
        debug_assert!(wake_handles <= crate::operation::PREPARE_BYTES);
        operation.charge(std::mem::size_of::<RouteCancellation>() + crate::operation::PREPARE_BYTES)
            .map_err(|e| e.to_string())?;
        operation.charge(crate::automatic::retained_begin_metadata_bytes())
            .map_err(|e| e.to_string())?;
        let route = Arc::new(WorkerRoute { stream: self.stream.to_string(), owner,
            allowance: operation.allowance(), state: Arc::new(Mutex::new(RouteState::Prepared)),
            changed: Arc::new(Notify::new()), cancel: Arc::new(RouteCancellation::new()),
            operation: operation.clone() });
        let (done, wait) = oneshot::channel();
        {
            let mut pending = self.pending.lock().expect("pending captures");
            if pending.route_owned || pending.legacy_overflow { return Err("transcript route unavailable".into()); }
            pending.route_owned = true;
            pending.route_operation = Some(operation.id().to_string());
            pending.marker_phase = 0;
            if self.prepare.try_send(Item::Prepare { route: route.clone(), done }).is_err() {
                route.cancel.request();
                return Err("transcript worker closed during prepare".into());
            }
        }
        match tokio::time::timeout_at(operation.allowance().deadline(), wait).await {
            Ok(Ok(value)) => Ok(value),
            _ => { route.cancel.request(); Err("transcript prepare unconfirmed".into()) }
        }
    }

    fn enqueue_marker(&self, reply: MarkerReply) -> Result<(), String> {
        let mut pending = self.pending.lock().expect("pending captures");
        if !pending.route_owned || pending.legacy_overflow { return Err("no owned transcript route".into()); }
        let operation = self.operation.as_ref().ok_or("no owned transcript operation")?;
        if pending.route_operation.as_deref() != Some(operation.id()) {
            return Err("transcript marker operation mismatch".into());
        }
        let next = match &reply {
            MarkerReply::Intent(_) if pending.marker_phase == 0 => 1,
            MarkerReply::Postcommit(_) if pending.marker_phase == 1 => 2,
            _ => return Err("transcript marker already issued or out of order".into()),
        };
        operation.check().map_err(|e| e.to_string())?;
        let bytes = crate::operation::encoded_len(&(self.stream.as_ref(), pending.ordinal), crate::operation::COMMAND_BYTES)
            .map_err(|_| operation.overflow().to_string())?;
        operation.charge(bytes).map_err(|e| e.to_string())?;
        self.tx.send(Item::Marker { ordinal: pending.ordinal, reply })
            .map_err(|_| "transcript worker closed before marker".into())?;
        pending.marker_phase = next;
        Ok(())
    }

    pub async fn confirm_intent(&self) -> Result<ConfirmedIntent, String> {
        let (done, wait) = oneshot::channel();
        self.enqueue_marker(MarkerReply::Intent(done))?;
        let operation = self.operation.as_ref().ok_or("no owned transcript operation")?;
        tokio::time::timeout_at(operation.allowance().deadline(), wait).await
            .map_err(|_| "intent marker timed out".to_string())?
            .map_err(|_| "intent marker unconfirmed".to_string())?
    }

    pub async fn confirm_postcommit(&self) -> Result<ConfirmedPostcommit, String> {
        let (done, wait) = oneshot::channel();
        self.enqueue_marker(MarkerReply::Postcommit(done))?;
        let operation = self.operation.as_ref().ok_or("no owned transcript operation")?;
        tokio::time::timeout_at(operation.allowance().deadline(), wait).await
            .map_err(|_| "postcommit marker timed out".to_string())?
            .map_err(|_| "postcommit marker unconfirmed".to_string())?
    }

    pub fn system(&self, seat: SeatId, text: impl Into<String>) {
        self.emit(seat, TranscriptEntry::System { text: text.into() });
    }

    /// Wait until everything queued so far has been accepted by the store.
    /// A permanently unavailable store needs the supervisor's bounded shutdown path.
    pub async fn flush(&self) {
        let _ = self.flush_confirmed().await;
    }

    /// Confirm captures queued before this marker. Later captures from other
    /// producers may remain pending. A dead delivery worker returns an error.
    pub async fn flush_confirmed(&self) -> Result<(), String> {
        if self.pending.lock().expect("pending captures").legacy_overflow {
            return Err("capture_overflow".into());
        }
        if let Some(operation) = &self.operation { operation.check().map_err(|e| e.to_string())?; }
        let (done, wait) = oneshot::channel();
        self.tx
            .send(Item::Flush(done))
            .map_err(|_| "transcript delivery worker is closed".to_string())?;
        wait.await
            .map_err(|_| "transcript delivery marker was not confirmed".to_string())
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().expect("pending captures").rows.len()
    }

    /// Stop and join delivery, retaining captures for a recovery outbox.
    /// Stop all producers first. This is irreversible for every clone of this sink.
    /// The last in-flight append may have committed before cancellation without confirming;
    /// inspect the campaign before replaying an outbox. Never replay it blindly.
    pub async fn stop_delivery(&self) -> Vec<UnconfirmedEntry> {
        let mut delivery = self.delivery.0.lock().await;
        if let Some(task) = delivery.take() {
            task.abort();
            let _ = task.await;
        }
        self.pending
            .lock()
            .expect("pending captures")
            .rows
            .iter()
            .map(|capture| capture.row.clone())
            .collect()
    }
}
/// One stored transcript entry.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredEntry {
    pub seat: SeatId,
    pub tseq: u64,
    pub at: String,
    pub game_seq: u64,
    pub entry: TranscriptEntry,
}

impl StoredEntry {
    /// The live-stream message for this entry.
    pub fn to_message(&self) -> ServerMessage {
        ServerMessage::Transcript {
            seat: self.seat.to_string(),
            tseq: self.tseq,
            at: self.at.clone(),
            game_seq: self.game_seq,
            entry: self.entry.clone(),
        }
    }
}

/// Source of the latest game event sequence, for [`LocalTranscript`].
pub type GameSeqFn = Arc<dyn Fn() -> u64 + Send + Sync>;

/// In-memory store that numbers entries per seat and broadcasts them. Used by tests and the demo
/// runner; the campaign server has its own persistent store.
pub struct LocalTranscript {
    next: Mutex<BTreeMap<SeatId, u64>>,
    log: Mutex<Vec<StoredEntry>>,
    tx: broadcast::Sender<StoredEntry>,
    game_seq: GameSeqFn,
}

impl LocalTranscript {
    pub fn new(game_seq: GameSeqFn) -> Arc<Self> {
        let (tx, _) = broadcast::channel(4096);
        Arc::new(Self {
            next: Mutex::new(BTreeMap::new()),
            log: Mutex::new(Vec::new()),
            tx,
            game_seq,
        })
    }

    pub fn detached() -> Arc<Self> {
        Self::new(Arc::new(|| 0))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StoredEntry> {
        self.tx.subscribe()
    }

    /// Everything stored so far, in capture order.
    pub fn log(&self) -> Vec<StoredEntry> {
        self.log.lock().expect("log").clone()
    }

    pub fn seat_log(&self, seat: SeatId) -> Vec<StoredEntry> {
        self.log().into_iter().filter(|r| r.seat == seat).collect()
    }
}

#[async_trait]
impl TranscriptStore for LocalTranscript {
    async fn append(
        &self,
        seat: SeatId,
        at: String,
        entry: TranscriptEntry,
    ) -> Result<u64, String> {
        let stored = {
            let mut next = self.next.lock().expect("next");
            let tseq = next.entry(seat).or_insert(0);
            *tseq += 1;
            let stored = StoredEntry {
                seat,
                tseq: *tseq,
                at,
                game_seq: (self.game_seq)(),
                entry,
            };
            self.log.lock().expect("log").push(stored.clone());
            stored
        };
        let tseq = stored.tseq;
        let _ = self.tx.send(stored);
        Ok(tseq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cna_core::ids::{Role, Side};

    const A: SeatId = SeatId::new(Side::Axis, Role::Commander);
    const B: SeatId = SeatId::new(Side::Commonwealth, Role::Commander);

    #[tokio::test]
    async fn tseq_is_per_seat_and_strictly_increasing() {
        let store = LocalTranscript::detached();
        let mut rx = store.subscribe();
        let sink = TranscriptSink::new(store.clone());
        sink.system(A, "one");
        sink.system(B, "one");
        sink.system(A, "two");
        sink.flush().await;
        let a: Vec<u64> = store.seat_log(A).iter().map(|r| r.tseq).collect();
        let b: Vec<u64> = store.seat_log(B).iter().map(|r| r.tseq).collect();
        assert_eq!(a, [1, 2]);
        assert_eq!(b, [1]);
        assert_eq!(rx.try_recv().unwrap().seat, A);
    }

    /// Fails the first `n` appends, then behaves.
    struct Flaky {
        fail: Mutex<u32>,
        inner: Arc<LocalTranscript>,
    }

    #[async_trait]
    impl TranscriptStore for Flaky {
        async fn append(
            &self,
            seat: SeatId,
            at: String,
            entry: TranscriptEntry,
        ) -> Result<u64, String> {
            {
                let mut n = self.fail.lock().unwrap();
                if *n > 0 {
                    *n -= 1;
                    return Err("db busy".into());
                }
            }
            self.inner.append(seat, at, entry).await
        }
    }

    #[tokio::test]
    async fn a_failing_store_delays_but_never_loses_or_reorders() {
        let inner = LocalTranscript::detached();
        let sink = TranscriptSink::new(Arc::new(Flaky {
            fail: Mutex::new(2),
            inner: inner.clone(),
        }));
        for i in 0..5 {
            sink.system(A, format!("m{i}"));
        }
        sink.flush().await;
        let texts: Vec<String> = inner
            .seat_log(A)
            .into_iter()
            .map(|r| match r.entry {
                TranscriptEntry::System { text } => text,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(texts, ["m0", "m1", "m2", "m3", "m4"]);
    }

    #[tokio::test]
    async fn wire_shape_matches_the_protocol() {
        let store = LocalTranscript::new(Arc::new(|| 1235));
        let sink = TranscriptSink::new(store.clone());
        sink.emit(
            A,
            TranscriptEntry::ToolCall {
                call_id: "c12".into(),
                tool: "describe_actions".into(),
                args: serde_json::json!({ "decision_id": "d1" }),
            },
        );
        sink.flush().await;
        let v = serde_json::to_value(store.log()[0].to_message()).unwrap();
        assert_eq!(v["type"], "transcript");
        assert_eq!(v["seat"], "axis.commander");
        assert_eq!(v["tseq"], 1);
        assert_eq!(v["game_seq"], 1235);
        assert_eq!(v["entry"]["kind"], "tool_call");
        assert_eq!(v["entry"]["call_id"], "c12");
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use tokio::sync::Notify;

    struct Blocked {
        entered: Arc<Notify>,
    }
    #[async_trait]
    impl TranscriptStore for Blocked {
        async fn append(&self, _: SeatId, _: String, _: TranscriptEntry) -> Result<u64, String> {
            self.entered.notify_one();
            std::future::pending().await
        }
    }
    #[tokio::test]
    async fn stopping_delivery_retains_ordered_captures_and_joins_in_flight_append() {
        let entered = Arc::new(Notify::new());
        let sink = TranscriptSink::new(Arc::new(Blocked {
            entered: entered.clone(),
        }));
        let seat = "axis.commander".parse().unwrap();
        for i in 0..3 {
            sink.system(seat, format!("m{i}"));
        }
        entered.notified().await;
        let (first, second) = tokio::join!(sink.stop_delivery(), sink.stop_delivery());
        assert_eq!(first, second);
        let texts: Vec<_> = first
            .iter()
            .map(|c| match &c.entry {
                TranscriptEntry::System { text } => text.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(texts, ["m0", "m1", "m2"]);
        sink.system(seat, "captured after close");
        assert_eq!(
            sink.pending_count(),
            4,
            "closed delivery must not silently drop captures"
        );
    }
    #[tokio::test]
    async fn dropping_last_producer_still_drains_a_healthy_store() {
        let store = LocalTranscript::detached();
        let mut live = store.subscribe();
        let sink = TranscriptSink::new(store.clone());
        sink.system("axis.commander".parse().unwrap(), "last capture");
        drop(sink);
        let row = tokio::time::timeout(Duration::from_secs(1), live.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(row.entry, TranscriptEntry::System { text } if text == "last capture"));
    }
}

#[cfg(test)]
mod host_route_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FirstAppend {
        entered: Notify,
        release: Notify,
        calls: AtomicUsize,
        normal: Mutex<Vec<SeatId>>,
    }
    #[async_trait]
    impl TranscriptStore for FirstAppend {
        async fn append(&self, seat: SeatId, _: String, _: TranscriptEntry) -> Result<u64, String> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.entered.notify_one();
                self.release.notified().await;
            }
            self.normal.lock().unwrap().push(seat);
            Ok(1)
        }
    }

    struct Routed(Mutex<Vec<(u64, SeatId)>>);
    #[async_trait]
    impl crate::host::HostDeliveryRoute for Routed {
        async fn append(&self, _: &str, ordinal: u64, row: &UnconfirmedEntry) -> Result<u64, String> {
            self.0.lock().unwrap().push((ordinal, row.seat));
            Ok(ordinal)
        }
    }

    fn operation() -> Operation {
        Operation::new("forced-operation".into(), 1, Allowance::required(Some(
            tokio::time::Instant::now() + Duration::from_secs(5))).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn prepare_joins_current_ack_routes_older_authors_and_second_marker_freezes_issue() {
        let a: SeatId = "axis.commander".parse().unwrap();
        let b: SeatId = "commonwealth.commander".parse().unwrap();
        let store = Arc::new(FirstAppend { entered: Notify::new(), release: Notify::new(),
            calls: AtomicUsize::new(0), normal: Mutex::new(vec![]) });
        let sink = TranscriptSink::new(store.clone());
        sink.system(a, "already issued");
        store.entered.notified().await;
        sink.system(b, "older buffered sibling");
        let op = operation();
        let forced = sink.for_operation(op.clone());
        let owner = HostIdentity { seat: a, epoch: 1, decision_id: "decision".into(),
            revision: 2, operation_id: op.id().into() };
        let preparing = {
            let sink = forced.clone(); let op = op.clone();
            tokio::spawn(async move { sink.prepare_route(owner, op).await })
        };
        // The preparation future must enqueue while the normal append is still owned.
        tokio::time::timeout(Duration::from_secs(1), async {
            while !sink.pending.lock().unwrap().route_owned { tokio::task::yield_now().await; }
        }).await.unwrap();
        assert!(!preparing.is_finished(), "prepare cannot fake the normal append ACK");
        store.release.notify_one();
        let route = preparing.await.unwrap().unwrap();
        assert_eq!(route.boundary(), 1, "buffered captures are not normal append ACKs");
        let routed = Arc::new(Routed(Mutex::new(vec![])));
        route.install("token".into(), routed.clone()).unwrap();
        forced.system(a, "confirmed intent");
        let first = forced.confirm_intent().await.unwrap();
        assert_eq!(first.ordinal(), 3);
        assert_eq!(first.token(), "token");
        assert_eq!(*store.normal.lock().unwrap(), [a]);
        assert_eq!(*routed.0.lock().unwrap(), [(2, b), (3, a)]);
        sink.system(b, "between-marker sibling");
        forced.system(a, "accepted receipt");
        let second = forced.confirm_postcommit().await.unwrap();
        assert!(second.ordinal() >= first.ordinal());
        assert_eq!(*routed.0.lock().unwrap(), [(2, b), (3, a), (4, b), (5, a)]);
        sink.system(b, "later sibling");
        tokio::task::yield_now().await;
        assert_eq!(sink.pending_count(), 1, "marker two freezes later issue for scoped final pending");
        route.release(false); // Test adapter's actual final settlement ACK.
        sink.flush_confirmed().await.unwrap();
        assert_eq!(*store.normal.lock().unwrap(), [a, b]);
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn oversized_whole_group_refuses_and_keeps_accepted_prefix() {
        let seat = "axis.commander".parse().unwrap();
        let store = Arc::new(FirstAppend { entered: Notify::new(), release: Notify::new(),
            calls: AtomicUsize::new(0), normal: Mutex::new(vec![]) });
        let sink = TranscriptSink::new(store.clone());
        sink.system(seat, "accepted prefix");
        store.entered.notified().await;
        let op = operation();
        let owned = sink.for_operation(op.clone());
        assert_eq!(owned.try_emit_batch(seat, vec![
            TranscriptEntry::System { text: "must not partially accept".into() },
            TranscriptEntry::System { text: "x".repeat(crate::operation::DELIVERY_BYTES) },
        ]), Err(OperationError::CaptureOverflow));
        assert_eq!(op.check(), Err(OperationError::CaptureOverflow));
        assert_eq!(sink.pending_count(), 1);
        let retained = sink.stop_delivery().await;
        assert_eq!(retained.len(), 1);
        assert!(matches!(&retained[0].entry, TranscriptEntry::System { text } if text == "accepted prefix"));
    }

    #[test]
    fn legacy_recovery_row_stays_unattributed_and_shape_preserved() {
        let old = serde_json::json!({"seat":"axis.commander", "at":"old",
            "entry":{"kind":"system","text":"legacy"}});
        let row: UnconfirmedEntry = serde_json::from_value(old.clone()).unwrap();
        assert!(row.origin.is_none());
        assert_eq!(serde_json::to_value(row).unwrap(), old);
    }
}
