//! Where a campaign is in the sequence of play.
//!
//! The exact sequence (stages, phases, segments, steps) belongs to each ruleset; this module only
//! gives the shared, serializable position that events, decisions and the board refer to. Anchors
//! use the timing vocabulary of `data/rules/README.md` (e.g.
//! `opstage.movement_and_combat.combat.barrage`).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::Side;

/// A dotted timing anchor from the shared vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Anchor(String);

impl Anchor {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The top-level stage, e.g. `opstage` for `opstage.movement_and_combat.movement`.
    pub fn stage(&self) -> &str {
        self.0.split('.').next().unwrap_or("")
    }

    /// The anchor's dotted components.
    pub fn parts(&self) -> impl Iterator<Item = &str> {
        self.0.split('.')
    }
}

impl fmt::Display for Anchor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The campaign clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clock {
    /// 1-based Game-Turn; one Game-Turn is one week.
    pub game_turn: u16,
    /// 1..=3 while inside an Operations Stage.
    pub op_stage: Option<u8>,
    /// Position within the turn.
    pub anchor: Anchor,
    /// The phasing side, once Player A/B are resolved for the current Operations Stage.
    pub phasing: Option<Side>,
    /// Repetition counter for repeatable cycles (e.g. Movement-and-Combat segments, `land:8.2`).
    pub cycle: Option<u16>,
}

impl Clock {
    pub fn start(game_turn: u16, anchor: Anchor) -> Self {
        Self {
            game_turn,
            op_stage: None,
            anchor,
            phasing: None,
            cycle: None,
        }
    }
}

impl fmt::Display for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GT{}", self.game_turn)?;
        if let Some(op) = self.op_stage {
            write!(f, " OpS{op}")?;
        }
        write!(f, " {}", self.anchor)?;
        if let Some(side) = self.phasing {
            write!(f, " [{side}]")?;
        }
        if let Some(cycle) = self.cycle {
            write!(f, " #{cycle}")?;
        }
        Ok(())
    }
}
