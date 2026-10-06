//! Durable per-seat notes and team messages.
//!
//! In the real game these live in the campaign database (`docs/architecture.md` §6); a session's
//! own memory is never relied on for handover or recovery. [`SeatMemory`] is the seam (async, so
//! the campaign server can persist them); [`InMemorySeatMemory`] backs the toy game and tests.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use cna_core::ids::SeatId;
use serde::{Deserialize, Serialize};

/// Maximum notebook size in bytes; larger writes are refused so a seat cannot grow it forever.
pub const NOTEBOOK_LIMIT_BYTES: usize = 32 * 1024;

/// A team message. Free-form text for now (`architecture.md` leaves the schema open).
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

#[async_trait]
pub trait SeatMemory: Send + Sync + 'static {
    async fn notebook_read(&self, seat: SeatId) -> String;
    async fn notebook_write(
        &self,
        seat: SeatId,
        mode: WriteMode,
        text: &str,
    ) -> Result<usize, String>;
    /// Send to every other seat on the sender's side. Returns how many seats received it.
    async fn message_team(&self, from: SeatId, text: &str) -> Result<usize, String>;
    /// Messages addressed to `seat` with number greater than `after`.
    async fn read_messages(&self, seat: SeatId, after: u64) -> Vec<TeamMessage>;
}

#[derive(Default)]
struct Inner {
    notebooks: BTreeMap<SeatId, String>,
    inbox: BTreeMap<SeatId, Vec<TeamMessage>>,
    next_n: u64,
}

/// In-process notes and messages with team scoping: a message reaches only the sender's side.
pub struct InMemorySeatMemory {
    seats: Vec<SeatId>,
    inner: Mutex<Inner>,
}

impl InMemorySeatMemory {
    pub fn new(seats: Vec<SeatId>) -> Self {
        Self {
            seats,
            inner: Mutex::new(Inner::default()),
        }
    }
}

#[async_trait]
impl SeatMemory for InMemorySeatMemory {
    async fn notebook_read(&self, seat: SeatId) -> String {
        let g = self.inner.lock().expect("memory lock");
        g.notebooks.get(&seat).cloned().unwrap_or_default()
    }

    async fn notebook_write(
        &self,
        seat: SeatId,
        mode: WriteMode,
        text: &str,
    ) -> Result<usize, String> {
        let mut g = self.inner.lock().expect("memory lock");
        let current = g.notebooks.get(&seat).cloned().unwrap_or_default();
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
        g.notebooks.insert(seat, next);
        Ok(len)
    }

    async fn message_team(&self, from: SeatId, text: &str) -> Result<usize, String> {
        let mut g = self.inner.lock().expect("memory lock");
        g.next_n += 1;
        let n = g.next_n;
        let mut sent = 0;
        for s in self
            .seats
            .iter()
            .filter(|s| s.side == from.side && **s != from)
        {
            g.inbox.entry(*s).or_default().push(TeamMessage {
                n,
                from,
                text: text.to_string(),
            });
            sent += 1;
        }
        Ok(sent)
    }

    async fn read_messages(&self, seat: SeatId, after: u64) -> Vec<TeamMessage> {
        let g = self.inner.lock().expect("memory lock");
        g.inbox
            .get(&seat)
            .map(|v| v.iter().filter(|m| m.n > after).cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(s: &str) -> SeatId {
        s.parse().unwrap()
    }

    fn memory() -> InMemorySeatMemory {
        InMemorySeatMemory::new(
            ["axis.commander", "axis.air", "commonwealth.commander"]
                .map(seat)
                .to_vec(),
        )
    }

    #[tokio::test]
    async fn messages_stay_inside_the_side() {
        let m = memory();
        assert_eq!(
            m.message_team(seat("axis.commander"), "hold the line")
                .await
                .unwrap(),
            1
        );
        assert_eq!(m.read_messages(seat("axis.air"), 0).await.len(), 1);
        assert!(
            m.read_messages(seat("commonwealth.commander"), 0)
                .await
                .is_empty()
        );
        assert!(m.read_messages(seat("axis.commander"), 0).await.is_empty());
        assert!(m.read_messages(seat("axis.air"), 1).await.is_empty());
    }

    #[tokio::test]
    async fn notebook_modes_and_limit() {
        let m = memory();
        let air = seat("axis.air");
        m.notebook_write(air, WriteMode::Replace, "a")
            .await
            .unwrap();
        m.notebook_write(air, WriteMode::Append, "b").await.unwrap();
        assert_eq!(m.notebook_read(air).await, "ab");
        assert_eq!(m.notebook_read(seat("axis.commander")).await, "");
        let big = "x".repeat(NOTEBOOK_LIMIT_BYTES + 1);
        assert!(
            m.notebook_write(air, WriteMode::Replace, &big)
                .await
                .is_err()
        );
        assert_eq!(m.notebook_read(air).await, "ab");
    }
}
