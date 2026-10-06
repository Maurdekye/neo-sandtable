//! Pure, deterministic rules engine core for neo-sandtable, a digital implementation of
//! *The Campaign for North Africa* (SPI, 1979).
//!
//! This crate performs no I/O. Everything that affects a game result is a function of the
//! world state, the pinned content, the rules profile, the command being evaluated, and the
//! campaign RNG. See `docs/architecture.md` for the overall design.

pub mod dice;
