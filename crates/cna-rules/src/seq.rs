//! The sequence of play as an explicit state machine (`airlog:48.0` for the full Land + Air +
//! Logistics game; `airlog:33.0` and `land:5.2` are its subsets).
//!
//! Every step is named by its timing anchor from `data/rules/README.md`, so the rule-case registry
//! says which cases govern it. The cursor walks four blocks per game-turn:
//!
//! ```text
//! setup (once) → [ PRE (game-turn stages I–IV)
//!                  → 3 × [ OPSTAGE (phases A–F) → PLAYER_HALF for Player A → PLAYER_HALF for B ]
//!                  → POST (stages VIII–IX) ]* → end_of_game
//! ```
//!
//! Phases G–M (the player half) run once for Player A, then again for Player B
//! (`airlog:33.0` IV; `land:5.2` III). The phasing player may repeat the four Movement-and-Combat segments
//! (`land:8.2`); see [`Cursor::repeat_movement_and_combat`]. The scenario ends at the close of
//! its last OpStage (`scen:60.22`), so the end-of-turn stages of the final game-turn are skipped.

use cna_protocol::Side;
use serde::{Deserialize, Serialize};

/// A game system a step belongs to; steps of systems not in play are skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum System {
    Land,
    Air,
    Logistics,
}

/// One step of the sequence of play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepDef {
    /// The timing anchor (`data/rules/README.md`).
    pub anchor: &'static str,
    /// The system that must be in play for the step to run.
    pub system: System,
}

const fn step(anchor: &'static str, system: System) -> StepDef {
    StepDef { anchor, system }
}

use System::{Air, Land, Logistics};

/// Scenario set-up, before Game-Turn 1.
pub const SETUP: &[StepDef] = &[step("setup", Land)];

/// Stages I–IV of each game-turn (`airlog:48.0` I–IV).
pub const PRE: &[StepDef] = &[
    step("initiative", Land),
    step("strategic_air.designation", Air),
    step("strategic_air.malta_availability", Air),
    step("strategic_air.mission_assignment", Air),
    step("strategic_air.malta_raid", Air),
    step("naval_convoy.schedule", Land),
    step("naval_convoy.recon", Air),
    step("naval_convoy.lane_assignment", Air),
    step("naval_convoy.bombing", Air),
    step("logistics.stores_expenditure", Logistics),
];

/// Phases A–F of each Operations Stage (`airlog:48.0` V.A–F). The Organization segments may be
/// done in any order; the engine runs them in printed order.
pub const OPSTAGE: &[StepDef] = &[
    step("opstage.initiative_declaration", Land),
    step("opstage.weather", Land),
    step("opstage.organization.water_distribution", Logistics),
    step("opstage.organization.reorganization", Land),
    step("opstage.organization.attrition", Logistics),
    step("opstage.organization.construction", Land),
    step("opstage.organization.training", Land),
    step("opstage.organization.supply_distribution", Land),
    step("opstage.organization.tactical_shipping", Land),
    step("opstage.convoy_arrival", Land),
    step("opstage.cw_fleet.assignment", Land),
    step("opstage.cw_fleet.repair", Land),
    step("opstage.land_support_air.assignment", Air),
    step("opstage.land_support_air.deployment", Air),
    step("opstage.land_support_air.air_combat", Air),
    step("opstage.land_support_air.flak", Air),
    step("opstage.land_support_air.completion", Air),
    step("opstage.land_support_air.return", Air),
    step("opstage.land_support_air.maintenance", Air),
];

/// Phases G–M, run for Player A and then for Player B (`airlog:48.0` V.G–M).
pub const PLAYER_HALF: &[StepDef] = &[
    step("opstage.reserve_designation", Land),
    step("opstage.movement_and_combat.movement", Land),
    step("opstage.movement_and_combat.breakdown", Land),
    step("opstage.movement_and_combat.combat.position", Land),
    step("opstage.movement_and_combat.combat.barrage", Land),
    step(
        "opstage.movement_and_combat.combat.retreat_before_assault",
        Land,
    ),
    step("opstage.movement_and_combat.combat.force_assignment", Land),
    step("opstage.movement_and_combat.combat.anti_armor", Land),
    step("opstage.movement_and_combat.combat.close_assault", Land),
    step("opstage.movement_and_combat.reserve_release", Land),
    step("opstage.truck_convoy_movement", Land),
    step("opstage.cw_rail_movement", Land),
    step("opstage.repair.towing", Land),
    step("opstage.repair.maintenance", Land),
    step("opstage.patrol", Land),
];

/// Stages VIII–IX of each game-turn (`airlog:48.0` VIII–IX).
pub const POST: &[StepDef] = &[
    step("strategic_air_recovery.return_to_base", Air),
    step("strategic_air_recovery.maintenance", Air),
    step("end_of_turn", Land),
];

/// Victory determination after the last OpStage.
pub const END: &[StepDef] = &[step("end_of_game", Land)];

/// Which block of the sequence the cursor is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Block {
    Setup,
    Pre,
    OpStage,
    PlayerHalf,
    Post,
    End,
    Finished,
}

impl Block {
    pub fn steps(self) -> &'static [StepDef] {
        match self {
            Block::Setup => SETUP,
            Block::Pre => PRE,
            Block::OpStage => OPSTAGE,
            Block::PlayerHalf => PLAYER_HALF,
            Block::Post => POST,
            Block::End => END,
            Block::Finished => &[],
        }
    }
}

/// Player A moves first in an Operations Stage; Player B second (`land:7.16`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Half {
    A,
    B,
}

/// The position in the sequence of play.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub game_turn: u16,
    pub op_stage: Option<u8>,
    pub half: Option<Half>,
    /// 1-based repetition of the Movement-and-Combat segments within the current half.
    pub cycle: u16,
    pub block: Block,
    pub index: usize,
    /// Whether the current step's entry procedure has run.
    pub entered: bool,
}

/// The scenario's clock bounds and systems, as the cursor needs them.
#[derive(Debug, Clone)]
pub struct Bounds {
    pub start_gt: u16,
    pub end_gt: u16,
    pub end_opstage: u8,
    pub systems: Vec<System>,
}

impl Cursor {
    /// Before set-up.
    pub fn start(bounds: &Bounds) -> Self {
        Cursor {
            game_turn: bounds.start_gt,
            op_stage: None,
            half: None,
            cycle: 1,
            block: Block::Setup,
            index: 0,
            entered: false,
        }
    }

    /// The current step, or `None` when the game is finished.
    pub fn step(&self) -> Option<StepDef> {
        self.block.steps().get(self.index).copied()
    }

    pub fn anchor(&self) -> &'static str {
        self.step().map_or("end_of_game", |s| s.anchor)
    }

    pub fn is_finished(&self) -> bool {
        self.block == Block::Finished
    }

    /// The phasing side in a player half, given who is Player A.
    pub fn phasing(&self, player_a: Option<Side>) -> Option<Side> {
        let a = player_a?;
        match self.half? {
            Half::A => Some(a),
            Half::B => Some(a.opponent()),
        }
    }

    /// Move to the next step whose system is in play. Returns `true` if a new game-turn began.
    pub fn advance(&mut self, bounds: &Bounds) -> bool {
        let mut new_turn = false;
        loop {
            new_turn |= self.next_raw(bounds);
            match self.step() {
                None => return new_turn,
                Some(s) if bounds.systems.contains(&s.system) => return new_turn,
                Some(_) => {}
            }
        }
    }

    /// Go back to the Movement Segment for another Movement-and-Combat cycle (`land:8.2`).
    pub fn repeat_movement_and_combat(&mut self) {
        let movement = PLAYER_HALF
            .iter()
            .position(|s| s.anchor == "opstage.movement_and_combat.movement")
            .unwrap_or(0);
        self.block = Block::PlayerHalf;
        self.index = movement;
        self.cycle += 1;
        self.entered = false;
    }

    fn next_raw(&mut self, bounds: &Bounds) -> bool {
        self.entered = false;
        self.index += 1;
        if self.index < self.block.steps().len() {
            return false;
        }
        self.index = 0;
        match self.block {
            Block::Setup => {
                self.block = Block::Pre;
                true
            }
            Block::Pre => {
                self.block = Block::OpStage;
                self.op_stage = Some(1);
                false
            }
            Block::OpStage => {
                self.block = Block::PlayerHalf;
                self.half = Some(Half::A);
                self.cycle = 1;
                false
            }
            Block::PlayerHalf if self.half == Some(Half::A) => {
                self.half = Some(Half::B);
                self.cycle = 1;
                false
            }
            Block::PlayerHalf => {
                self.half = None;
                self.cycle = 1;
                let op = self.op_stage.unwrap_or(1);
                if self.game_turn == bounds.end_gt && op >= bounds.end_opstage {
                    self.op_stage = None;
                    self.block = Block::End;
                } else if op < 3 {
                    self.op_stage = Some(op + 1);
                    self.block = Block::OpStage;
                } else {
                    self.op_stage = None;
                    self.block = Block::Post;
                }
                false
            }
            Block::Post => {
                self.game_turn += 1;
                self.block = Block::Pre;
                true
            }
            Block::End | Block::Finished => {
                self.block = Block::Finished;
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Bounds {
        Bounds {
            start_gt: 1,
            end_gt: 2,
            end_opstage: 3,
            systems: vec![Land, Air, Logistics],
        }
    }

    #[test]
    fn walks_two_full_game_turns_and_ends_after_the_last_opstage() {
        let b = bounds();
        let mut c = Cursor::start(&b);
        let mut anchors = vec![c.anchor()];
        let mut guard = 0;
        while !c.is_finished() {
            c.advance(&b);
            if let Some(step) = c.step() {
                anchors.push(step.anchor);
            }
            guard += 1;
            assert!(guard < 1000);
        }
        let count = |a: &str| anchors.iter().filter(|x| **x == a).count();
        assert_eq!(count("initiative"), 2);
        assert_eq!(count("opstage.initiative_declaration"), 6);
        // Player A and Player B each get a movement segment in every OpStage.
        assert_eq!(count("opstage.movement_and_combat.movement"), 12);
        // The final game-turn has no Strategic Air Recovery stage.
        assert_eq!(count("strategic_air_recovery.return_to_base"), 1);
        assert_eq!(count("end_of_game"), 1);
    }

    #[test]
    fn steps_of_systems_not_in_play_are_skipped() {
        let mut b = bounds();
        b.systems = vec![Land];
        let mut c = Cursor::start(&b);
        while !c.is_finished() {
            c.advance(&b);
            if let Some(s) = c.step() {
                assert_eq!(s.system, Land, "{} ran without its system", s.anchor);
            }
        }
    }

    #[test]
    fn phasing_follows_player_a() {
        let b = bounds();
        let mut c = Cursor::start(&b);
        while c.block != Block::PlayerHalf {
            c.advance(&b);
        }
        assert_eq!(c.phasing(Some(Side::Axis)), Some(Side::Axis));
        while c.half == Some(Half::A) {
            c.advance(&b);
        }
        assert_eq!(c.phasing(Some(Side::Axis)), Some(Side::Commonwealth));
    }

    #[test]
    fn every_anchor_is_in_the_registry_vocabulary() {
        let readme = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rules/README.md"),
        )
        .unwrap();
        for block in [
            Block::Setup,
            Block::Pre,
            Block::OpStage,
            Block::PlayerHalf,
            Block::Post,
            Block::End,
        ] {
            for s in block.steps() {
                let last = s.anchor.rsplit('.').next().unwrap();
                assert!(
                    readme.contains(s.anchor) || readme.contains(&format!(".{last}")),
                    "{} is not in data/rules/README.md",
                    s.anchor
                );
            }
        }
    }
}
