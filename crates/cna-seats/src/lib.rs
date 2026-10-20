//! AI seat drivers for neo-sandtable.
//!
//! Each LLM seat is one long-lived CLI session (Claude Code, Codex or Antigravity) that reaches the
//! game only through its own MCP tools. This crate provides the MCP tool server, one session
//! driver per CLI behind [`driver::SeatDriver`], conversion of every CLI's stream into protocol
//! transcript entries, and run-level budgets and recovery. The game itself sits behind
//! [`game::GameBackend`]; the toy game in [`toy`] stands in until the engine's decision model
//! lands. See `README.md` for how each CLI is driven.

pub mod automatic;
pub mod calibration;
pub mod calibration_cost;
pub mod calibration_http;
pub mod calibration_isolation;
pub mod calibration_run;
pub mod demo;
pub mod driver;
pub mod game;
pub mod mcp;
pub mod memory;
pub mod request_budget;
pub mod run;
pub mod subscription_login;
pub mod toy;
pub mod transcript;
#[cfg(windows)]
pub mod watched_job;
// Row/schema restore exploration is shelved off the watched first-turn path.
// Preserved in wip/seats-restore-schema-parked-20261020; not a launch gate.

#[cfg(windows)]
pub mod watched_state;
