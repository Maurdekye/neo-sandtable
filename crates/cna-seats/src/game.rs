//! The seam between the seat drivers and the rules engine.
//!
//! The real engine's decision model (owned by the lead) is not ready, so the MCP server is built
//! against [`GameBackend`] and exercised with [`crate::toy::NumberDuel`]. Wiring the real engine
//! means implementing this trait over `cna-core`'s pending decisions and action spaces; nothing in
//! the drivers or the MCP server knows what game is behind it.
//!
//! Every method takes the calling seat and must return only what that seat may know
//! (`docs/architecture.md` §4). The server never filters again, so a leak here is a leak.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Seat id, `<side>.<role>` (for example `axis.commander`).
pub type SeatId = String;

/// A seat's static description.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatInfo {
    pub id: SeatId,
    /// `axis` or `commonwealth` (any string for the toy game); team messages stay inside a side.
    pub side: String,
    pub role: String,
}

/// One open decision window owned by a seat (`protocol.md` `PendingDecision`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingDecision {
    pub id: String,
    pub seat: SeatId,
    pub kind: String,
    pub summary: String,
    pub opened_seq: u64,
}

/// A request to commit a decision response.
#[derive(Clone, Debug)]
pub struct SubmitRequest {
    pub decision_id: String,
    /// The controller epoch of the endpoint the call arrived on. A handover bumps the epoch, so a
    /// response from a superseded session is rejected by the engine.
    pub epoch: u64,
    pub response: Value,
}

/// What a successful `submit` reports back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubmitReceipt {
    pub decision_id: String,
    /// `true` when this exact response had already been accepted (idempotent replay).
    pub duplicate: bool,
    /// Human-readable one-liner, also used as the transcript's `decision_submitted` summary.
    pub summary: String,
    /// Model-facing result (what the seat is told after submitting).
    pub result: Value,
}

/// Why a tool call failed. The message goes back to the model, so it must not leak hidden state.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ToolError {
    #[error("unknown decision `{0}` for this seat")]
    UnknownDecision(String),
    #[error("unknown target `{0}`")]
    UnknownTarget(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("stale: {0}")]
    Stale(String),
    #[error("this seat's controller epoch is out of date; the session was superseded")]
    EpochMismatch,
    #[error("{0}")]
    Other(String),
}

/// The game as the seat tools see it. Implemented by the toy game now and by the engine adapter
/// later. All methods are synchronous and cheap; the server holds the backend behind a mutex.
pub trait GameBackend: Send + 'static {
    /// All seats, in a stable order.
    fn seats(&self) -> Vec<SeatInfo>;
    /// Latest game event sequence number (used to align transcripts with the replay).
    fn game_seq(&self) -> u64;
    /// The seat's open decisions.
    fn pending(&self, seat: &str) -> Vec<PendingDecision>;
    /// The seat's filtered situation report. Includes its pending decisions.
    fn observe(&self, seat: &str) -> Value;
    /// Authorized detail of one thing (hex, unit, history entry, …).
    fn inspect(&self, seat: &str, target: &str) -> Result<Value, ToolError>;
    /// The legal action space of one pending decision, with parameter domains.
    fn describe_actions(&self, seat: &str, decision_id: &str) -> Result<Value, ToolError>;
    /// Check a draft response without committing. Must not consume randomness or reveal hidden
    /// information beyond what `describe_actions` already shows the seat.
    fn validate(&self, seat: &str, decision_id: &str, response: &Value)
    -> Result<Value, ToolError>;
    /// Commit a response. Idempotent: replaying an accepted response returns `duplicate: true`;
    /// a different response to an already-resolved decision is an error.
    fn submit(&mut self, seat: &str, request: SubmitRequest) -> Result<SubmitReceipt, ToolError>;
    /// The controller epoch the backend currently accepts for a seat.
    fn epoch(&self, seat: &str) -> u64;
    /// `Some(outcome)` once the game is over.
    fn outcome(&self) -> Option<Value>;
}
