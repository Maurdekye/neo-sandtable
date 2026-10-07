//! Transcript capture (`docs/protocol.md` §3 `transcript`).
//!
//! Every CLI's stream is converted into [`cna_protocol::TranscriptEntry`] values. Drivers hand
//! them to a [`TranscriptSink`], which delivers them *in order* to a [`TranscriptStore`] — the
//! campaign server's persistence in production, [`LocalTranscript`] in tests and demos. The store
//! assigns the per-seat `tseq`, so numbering survives a CLI restart, a session resume or a reseed
//! (the sink belongs to the run, not to a CLI process).
//!
//! Delivery is reliable: entries wait in an unbounded queue and are retried until the store takes
//! them, so a slow or briefly failing store never loses an entry and never blocks a driver.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cna_core::ids::SeatId;
use cna_protocol::ServerMessage;
pub use cna_protocol::TranscriptEntry;
use tokio::sync::{broadcast, mpsc, oneshot};

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
}

enum Item {
    Entry(UnconfirmedEntry),
    Flush(oneshot::Sender<()>),
}

struct DeliveryTask(tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>);

/// The cloneable handle drivers write to.
#[derive(Clone)]
pub struct TranscriptSink {
    tx: mpsc::UnboundedSender<Item>,
    pending: Arc<Mutex<std::collections::VecDeque<UnconfirmedEntry>>>,
    delivery: Arc<DeliveryTask>,
}

impl TranscriptSink {
    /// Start the delivery task for `store`. Must be called inside a Tokio runtime.
    pub fn new(store: Arc<dyn TranscriptStore>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Item>();
        let pending = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        let remaining = pending.clone();
        let task = tokio::spawn(async move {
            while let Some(item) = rx.recv().await {
                match item {
                    Item::Entry(capture) => {
                        let mut delay = Duration::from_millis(50);
                        while let Err(e) = store
                            .append(capture.seat, capture.at.clone(), capture.entry.clone())
                            .await
                        {
                            eprintln!(
                                "transcript store refused an entry for {}: {e}; retrying",
                                capture.seat
                            );
                            tokio::time::sleep(delay).await;
                            delay = (delay * 2).min(Duration::from_secs(5));
                        }
                        remaining.lock().expect("pending captures").pop_front();
                    }
                    Item::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
        Self {
            tx,
            pending,
            delivery: Arc::new(DeliveryTask(tokio::sync::Mutex::new(Some(task)))),
        }
    }

    /// Queue one entry for a seat (never waits for storage; capture time is taken now).
    pub fn emit(&self, seat: SeatId, entry: TranscriptEntry) {
        let capture = UnconfirmedEntry {
            seat,
            at: now_rfc3339(),
            entry,
        };
        // Hold this lock through enqueue so concurrent producers and the recovery mirror
        // have the same order. A failed enqueue remains visible in the mirror.
        let mut pending = self.pending.lock().expect("pending captures");
        pending.push_back(capture.clone());
        let _ = self.tx.send(Item::Entry(capture));
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
        let (done, wait) = oneshot::channel();
        self.tx
            .send(Item::Flush(done))
            .map_err(|_| "transcript delivery worker is closed".to_string())?;
        wait.await
            .map_err(|_| "transcript delivery marker was not confirmed".to_string())
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().expect("pending captures").len()
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
            .iter()
            .cloned()
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
