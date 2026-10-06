//! Transcript records (`docs/protocol.md` §3 `transcript`).
//!
//! Every CLI's stream is converted into [`TranscriptEntry`] values; the [`TranscriptSink`] stamps
//! each with a per-seat `tseq`, a wall-clock time and the latest game sequence, and broadcasts it
//! for the server to persist and stream. The sink belongs to the run, not to a CLI process, so
//! `tseq` keeps increasing across a session resume or a reseed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

/// One transcript entry. Field names and kinds are the provisional protocol's; unknown kinds must
/// be ignored by readers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptEntry {
    AssistantText {
        text: String,
    },
    /// Only when the CLI exposes reasoning text.
    Reasoning {
        text: String,
    },
    ToolCall {
        call_id: String,
        tool: String,
        args: Value,
    },
    ToolResult {
        call_id: String,
        ok: bool,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<Value>,
    },
    DecisionSubmitted {
        decision_id: String,
        summary: String,
    },
    System {
        text: String,
    },
}

/// The `transcript` message the live stream carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptRecord {
    #[serde(rename = "type", default = "transcript_type")]
    pub message_type: String,
    pub seat: String,
    /// Per-seat sequence, strictly increasing from 1.
    pub tseq: u64,
    /// RFC 3339 capture time (display only).
    pub at: String,
    pub game_seq: u64,
    pub entry: TranscriptEntry,
}

fn transcript_type() -> String {
    "transcript".into()
}

/// Source of the latest game event sequence.
pub type GameSeqFn = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Stamps and fans out transcript entries. Cloning shares the counters.
#[derive(Clone)]
pub struct TranscriptSink {
    inner: Arc<SinkInner>,
}

struct SinkInner {
    next: Mutex<BTreeMap<String, u64>>,
    log: Mutex<Vec<TranscriptRecord>>,
    tx: broadcast::Sender<TranscriptRecord>,
    game_seq: GameSeqFn,
}

impl TranscriptSink {
    pub fn new(game_seq: GameSeqFn) -> Self {
        let (tx, _) = broadcast::channel(4096);
        Self {
            inner: Arc::new(SinkInner {
                next: Mutex::new(BTreeMap::new()),
                log: Mutex::new(Vec::new()),
                tx,
                game_seq,
            }),
        }
    }

    /// A sink whose `game_seq` is always 0 (tests and tools).
    pub fn detached() -> Self {
        Self::new(Arc::new(|| 0))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TranscriptRecord> {
        self.inner.tx.subscribe()
    }

    /// Record one entry for a seat and broadcast it. Returns the stamped record.
    pub fn push(&self, seat: &str, entry: TranscriptEntry) -> TranscriptRecord {
        let record = {
            // One lock covers sequence assignment and log append so the log is tseq-ordered.
            let mut next = self.inner.next.lock().expect("sink lock");
            let tseq = next.entry(seat.to_string()).or_insert(0);
            *tseq += 1;
            let record = TranscriptRecord {
                message_type: transcript_type(),
                seat: seat.to_string(),
                tseq: *tseq,
                at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                game_seq: (self.inner.game_seq)(),
                entry,
            };
            self.inner
                .log
                .lock()
                .expect("sink log")
                .push(record.clone());
            record
        };
        // No subscribers is fine; the log keeps everything.
        let _ = self.inner.tx.send(record.clone());
        record
    }

    pub fn system(&self, seat: &str, text: impl Into<String>) {
        self.push(seat, TranscriptEntry::System { text: text.into() });
    }

    /// Everything captured so far, across seats, in capture order.
    pub fn log(&self) -> Vec<TranscriptRecord> {
        self.inner.log.lock().expect("sink log").clone()
    }

    /// Captured records for one seat.
    pub fn seat_log(&self, seat: &str) -> Vec<TranscriptRecord> {
        self.log().into_iter().filter(|r| r.seat == seat).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tseq_is_per_seat_and_strictly_increasing() {
        let sink = TranscriptSink::detached();
        let mut rx = sink.subscribe();
        sink.system("a", "one");
        sink.system("b", "one");
        sink.system("a", "two");
        let a: Vec<u64> = sink.seat_log("a").iter().map(|r| r.tseq).collect();
        let b: Vec<u64> = sink.seat_log("b").iter().map(|r| r.tseq).collect();
        assert_eq!(a, [1, 2]);
        assert_eq!(b, [1]);
        assert_eq!(rx.try_recv().unwrap().seat, "a");
    }

    #[test]
    fn wire_shape_matches_the_protocol() {
        let sink = TranscriptSink::new(Arc::new(|| 1235));
        let r = sink.push(
            "axis.commander",
            TranscriptEntry::ToolCall {
                call_id: "c12".into(),
                tool: "describe_actions".into(),
                args: serde_json::json!({ "decision_id": "d1" }),
            },
        );
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["type"], "transcript");
        assert_eq!(v["seat"], "axis.commander");
        assert_eq!(v["tseq"], 1);
        assert_eq!(v["game_seq"], 1235);
        assert_eq!(v["entry"]["kind"], "tool_call");
        assert_eq!(v["entry"]["call_id"], "c12");
        let back: TranscriptRecord = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
    }
}
