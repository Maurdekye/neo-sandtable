//! AI seat drivers for neo-sandtable.
//!
//! Each LLM seat is one long-lived CLI session (Claude Code, Codex or Antigravity) that reaches the
//! game only through its own MCP tools. This crate provides the MCP tool server, one session
//! driver per CLI behind [`driver::SeatDriver`], conversion of every CLI's stream into protocol
//! transcript entries, and run-level budgets and recovery. The game itself sits behind
//! [`game::GameBackend`]; the toy game in [`toy`] stands in until the engine's decision model
//! lands. See `README.md` for how each CLI is driven.

pub mod automatic;
pub mod demo;
pub mod driver;
pub mod failed_evidence;
pub mod game;
pub mod host;
#[cfg(test)]
mod host_fixture;
pub mod mcp;
pub mod memory;
pub mod operation;
pub mod run;
pub mod seat_host;
pub mod toy;
mod toy_host;
pub mod transcript;
