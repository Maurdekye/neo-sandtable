//! Durable per-seat notes and team messages.
//!
//! In the real game these live in the campaign database (`docs/architecture.md` §6); a session's
//! own memory is never relied on for handover or recovery. [`SeatMemory`] is the seam;
//! [`InMemorySeatMemory`] backs the toy game and tests.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::game::{SeatId, SeatInfo};

/// Maximum notebook size in bytes; larger writes are refused so a seat cannot grow it forever.
pub const NOTEBOOK_LIMIT_BYTES: usize = 32 * 1024;

/// A structured team message. Free-form for now (`architecture.md` leaves the schema open).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMessage {
    /// Monotonic per-run message number; `read_messages` pages on it.
    pub n: u64,
    pub from: SeatId,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    Replace,
    Append,
}

pub trait SeatMemory: Send + Sync + 'static {
    fn notebook_read(&self, seat: &str) -> String;
    fn notebook_write(&self, seat: &str, mode: WriteMode, text: &str) -> Result<usize, String>;
    /// Send to every other seat on the sender's side. Returns how many seats received it.
    fn message_team(&self, from: &str, text: &str) -> Result<usize, String>;
    /// Messages addressed to `seat` with number greater than `after`.
    fn read_messages(&self, seat: &str, after: u64) -> Vec<TeamMessage>;
}

#[derive(Default)]
struct Inner {
    notebooks: BTreeMap<SeatId, String>,
    inbox: BTreeMap<SeatId, Vec<TeamMessage>>,
    next_n: u64,
}

/// In-process notes and messages with team scoping: a message reaches only the sender's side.
pub struct InMemorySeatMemory {
    seats: Vec<SeatInfo>,
    inner: Mutex<Inner>,
}

impl InMemorySeatMemory {
    pub fn new(seats: Vec<SeatInfo>) -> Self {
        Self {
            seats,
            inner: Mutex::new(Inner::default()),
        }
    }
}

impl SeatMemory for InMemorySeatMemory {
    fn notebook_read(&self, seat: &str) -> String {
        let g = self.inner.lock().expect("memory lock");
        g.notebooks.get(seat).cloned().unwrap_or_default()
    }

    fn notebook_write(&self, seat: &str, mode: WriteMode, text: &str) -> Result<usize, String> {
        let mut g = self.inner.lock().expect("memory lock");
        let current = g.notebooks.get(seat).cloned().unwrap_or_default();
        let next = match mode {
            WriteMode::Replace => text.to_string(),
            WriteMode::Append => format!("{current}{text}"),
        };
        if next.len() > NOTEBOOK_LIMIT_BYTES {
            return Err(format!(
                "notebook would be {} bytes; the limit is {NOTEBOOK_LIMIT_BYTES}",
                next.len()
            ));
        }
        let len = next.len();
        g.notebooks.insert(seat.to_string(), next);
        Ok(len)
    }

    fn message_team(&self, from: &str, text: &str) -> Result<usize, String> {
        let Some(me) = self.seats.iter().find(|s| s.id == from) else {
            return Err("unknown seat".into());
        };
        let mut g = self.inner.lock().expect("memory lock");
        g.next_n += 1;
        let n = g.next_n;
        let mut sent = 0;
        for s in self
            .seats
            .iter()
            .filter(|s| s.side == me.side && s.id != from)
        {
            g.inbox.entry(s.id.clone()).or_default().push(TeamMessage {
                n,
                from: from.to_string(),
                text: text.to_string(),
            });
            sent += 1;
        }
        Ok(sent)
    }

    fn read_messages(&self, seat: &str, after: u64) -> Vec<TeamMessage> {
        let g = self.inner.lock().expect("memory lock");
        g.inbox
            .get(seat)
            .map(|v| v.iter().filter(|m| m.n > after).cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seats() -> Vec<SeatInfo> {
        ["axis.commander", "axis.air", "commonwealth.commander"]
            .iter()
            .map(|id| {
                let (side, role) = id.split_once('.').unwrap();
                SeatInfo {
                    id: id.to_string(),
                    side: side.into(),
                    role: role.into(),
                }
            })
            .collect()
    }

    #[test]
    fn messages_stay_inside_the_side() {
        let m = InMemorySeatMemory::new(seats());
        assert_eq!(
            m.message_team("axis.commander", "hold the line").unwrap(),
            1
        );
        assert_eq!(m.read_messages("axis.air", 0).len(), 1);
        assert!(m.read_messages("commonwealth.commander", 0).is_empty());
        assert!(m.read_messages("axis.commander", 0).is_empty());
        assert!(m.read_messages("axis.air", 1).is_empty());
    }

    #[test]
    fn notebook_modes_and_limit() {
        let m = InMemorySeatMemory::new(seats());
        m.notebook_write("axis.air", WriteMode::Replace, "a")
            .unwrap();
        m.notebook_write("axis.air", WriteMode::Append, "b")
            .unwrap();
        assert_eq!(m.notebook_read("axis.air"), "ab");
        assert_eq!(m.notebook_read("axis.commander"), "");
        let big = "x".repeat(NOTEBOOK_LIMIT_BYTES + 1);
        assert!(
            m.notebook_write("axis.air", WriteMode::Replace, &big)
                .is_err()
        );
        assert_eq!(m.notebook_read("axis.air"), "ab");
    }
}
