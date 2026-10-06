//! Pure, deterministic rules engine core for neo-sandtable, a digital implementation of
//! *The Campaign for North Africa* (SPI, 1979).
//!
//! This crate performs no I/O. Everything that affects a game result is a function of the
//! world state, the pinned content, the rules profile, the command being evaluated, and the
//! campaign RNG. See `docs/architecture.md` for the overall design.
//!
//! - [`engine`]: the [`Ruleset`](engine::Ruleset) trait and [`evaluate`](engine::evaluate).
//! - [`decision`]: the controller contract every seat type answers through.
//! - [`event`], [`visibility`]: audience-tagged events and perspective filtering.
//! - [`clock`], [`hex`], [`ids`], [`quantity`], [`dice`]: shared building blocks.

pub mod clock;
pub mod decision;
pub mod dice;
pub mod engine;
pub mod event;
pub mod hex;
pub mod ids;
pub mod quantity;
pub mod visibility;
