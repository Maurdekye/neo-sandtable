//! Initiative, off-map travel and basic stacking (sections 7-9).

use std::collections::BTreeSet;

use serde::Deserialize;

use crate::ranges::{IntRange, check_int_tiling};
use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InitiativeSituation {
    GameTurns,
    RommelCounterOnGameMaps,
    GermanLandCombatUnitsWithoutRommelCounterOnGameMaps,
    NoGermanCombatUnitsOrRommelCounterOnGameMaps,
}

#[derive(Debug, Clone, Deserialize)]
struct InitiativeRow {
    player: String,
    situation: InitiativeSituation,
    game_turn_range: Option<IntRange>,
    rating: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InitiativeRatings {
    row: Vec<InitiativeRow>,
}

impl Bound for InitiativeRatings {
    const ID: &'static str = "land.7.2.initiative_ratings";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let table: Self = raw.deserialize()?;
        let mut situations = BTreeSet::new();
        let mut turns = Vec::new();
        for (i, row) in table.row.iter().enumerate() {
            if row.rating < 0 {
                return Err(raw.err(format!("row[{i}].rating"), "must be nonnegative"));
            }
            match (row.player.as_str(), row.situation, row.game_turn_range) {
                ("commonwealth", InitiativeSituation::GameTurns, Some(range)) => turns.push(range),
                ("axis", situation, None) if situation != InitiativeSituation::GameTurns => {
                    if !situations.insert(situation) {
                        return Err(
                            raw.err(format!("row[{i}].situation"), "duplicate Axis situation")
                        );
                    }
                }
                _ => {
                    return Err(raw.err(
                        format!("row[{i}]"),
                        "player, situation and turn range disagree",
                    ));
                }
            }
        }
        check_int_tiling(turns, 1, 111).map_err(|e| raw.err("row.game_turn_range", e))?;
        if situations.len() != 3 {
            return Err(raw.err("row.situation", "all three Axis situations are required"));
        }
        Ok(table)
    }
}

impl InitiativeRatings {
    /// Cases: land:7.2, land:7.14
    pub fn commonwealth_rating(&self, game_turn: i32) -> Option<i32> {
        self.row
            .iter()
            .find(|r| r.game_turn_range.is_some_and(|s| s.contains(game_turn)))
            .map(|r| r.rating)
    }

    /// Supply map presence after excluding the Tripoli/Tunisia holding boxes.
    /// Cases: land:7.2, land:7.14
    pub fn axis_rating(
        &self,
        rommel_on_game_maps: bool,
        german_combat_units_on_game_maps: bool,
    ) -> i32 {
        let situation = if rommel_on_game_maps {
            InitiativeSituation::RommelCounterOnGameMaps
        } else if german_combat_units_on_game_maps {
            InitiativeSituation::GermanLandCombatUnitsWithoutRommelCounterOnGameMaps
        } else {
            InitiativeSituation::NoGermanCombatUnitsOrRommelCounterOnGameMaps
        };
        self.row
            .iter()
            .find(|r| r.situation == situation)
            .expect("validated situation")
            .rating
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OffMapPlace {
    Tunis,
    Gabes,
    Tripoli,
    Tripolitania,
    Nofilia,
}

#[derive(Debug, Clone, Deserialize)]
struct StagesByCpa {
    cpa_25_or_more: i32,
    cpa_15_to_20: i32,
    cpa_under_15: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct MovementRow {
    from: OffMapPlace,
    to: OffMapPlace,
    stages_by_cpa: StagesByCpa,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OffMapMovement {
    row: Vec<MovementRow>,
}

impl Bound for OffMapMovement {
    const ID: &'static str = "land.8.89.off_map_movement_distance";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let table: Self = raw.deserialize()?;
        let mut pairs = BTreeSet::new();
        for (i, r) in table.row.iter().enumerate() {
            let pair = (r.from.min(r.to), r.from.max(r.to));
            if r.from == r.to || !pairs.insert(pair) {
                return Err(raw.err(
                    format!("row[{i}].from"),
                    "distinct places and unique unordered pairs required",
                ));
            }
            let c = &r.stages_by_cpa;
            if [c.cpa_25_or_more, c.cpa_15_to_20, c.cpa_under_15]
                .iter()
                .any(|n| *n <= 0)
            {
                return Err(raw.err(
                    format!("row[{i}].stages_by_cpa"),
                    "travel stages must be positive",
                ));
            }
        }
        if pairs.len() != 10 {
            return Err(raw.err(
                "row",
                "all ten unordered pairs of the five places are required",
            ));
        }
        Ok(table)
    }
}

impl OffMapMovement {
    /// Returns None for a CPA absent from the chart (21-24), or a nonpositive CPA.
    /// Cases: land:8.89, land:8.83
    pub fn stages(&self, from: OffMapPlace, to: OffMapPlace, basic_cpa: i32) -> Option<i32> {
        if basic_cpa <= 0 || (21..=24).contains(&basic_cpa) {
            return None;
        }
        if from == to {
            return Some(0);
        }
        let r = &self
            .row
            .iter()
            .find(|r| (r.from == from && r.to == to) || (r.from == to && r.to == from))?
            .stages_by_cpa;
        Some(if basic_cpa >= 25 {
            r.cpa_25_or_more
        } else if basic_cpa >= 15 {
            r.cpa_15_to_20
        } else {
            r.cpa_under_15
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationLevel {
    Division,
    SuperBrigade,
    StandardBrigadeOrBattleGroup,
    Battalion,
    Company,
    TruckPointsInConvoy,
    ReplacementPoints,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellOf {
    Division,
    AnyBrigade,
    BattleGroup,
    Battalion,
    Hq,
    AnyAttachedUnits,
}

#[derive(Debug, Clone, Deserialize)]
struct StackingRow {
    level: OrganizationLevel,
    #[serde(default)]
    shell_of: Vec<ShellOf>,
    stacking_points: Option<i32>,
    per_block_of_points: Option<i32>,
    stacking_points_per_block_x2: Option<i32>,
    #[serde(default)]
    fraction_counts_as_full_block: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StackingValues {
    row: Vec<StackingRow>,
}

impl Bound for StackingValues {
    const ID: &'static str = "land.9.4.stacking_point_values";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let table: Self = raw.deserialize()?;
        let mut levels = BTreeSet::new();
        let mut shells = BTreeSet::new();
        for (i, row) in table.row.iter().enumerate() {
            if !levels.insert(row.level) {
                return Err(raw.err(format!("row[{i}].level"), "duplicate level"));
            }
            let is_block = matches!(
                row.level,
                OrganizationLevel::TruckPointsInConvoy | OrganizationLevel::ReplacementPoints
            );
            match (
                row.stacking_points,
                row.per_block_of_points,
                row.stacking_points_per_block_x2,
            ) {
                (Some(n), None, None)
                    if (0..=i32::MAX / 2).contains(&n)
                        && !is_block
                        && !row.fraction_counts_as_full_block => {}
                (None, Some(n), Some(v))
                    if n > 0
                        && v >= 0
                        && is_block
                        && row.fraction_counts_as_full_block
                        && row.shell_of.is_empty() => {}
                _ => return Err(raw.err(
                    format!("row[{i}]"),
                    "one nonnegative stacking value or positive block with ceiling flag required",
                )),
            }
            for shell in &row.shell_of {
                if !shells.insert(*shell) {
                    return Err(raw.err(format!("row[{i}].shell_of"), "duplicate shell mapping"));
                }
            }
        }
        if levels.len() != 7 || shells.len() != 6 {
            return Err(raw.err("row", "seven levels and six shell mappings are required"));
        }
        Ok(table)
    }
}

impl StackingValues {
    /// Base value of a full unit, in half stacking points. Block-priced items return None.
    /// Cases: land:9.4, land:9.26
    pub fn full_unit_halves(&self, level: OrganizationLevel) -> Option<i32> {
        self.row
            .iter()
            .find(|r| r.level == level)?
            .stacking_points
            .map(|n| n * 2)
    }

    /// Cases: land:9.4, land:9.28
    pub fn shell_halves(&self, shell_of: ShellOf) -> i32 {
        self.row
            .iter()
            .find(|r| r.shell_of.contains(&shell_of))
            .expect("validated shell")
            .stacking_points
            .expect("unit value")
            * 2
    }

    /// Convoy values apply when checking road/track cost eligibility; replacement values apply generally.
    /// Cases: land:9.4, land:9.29, land:9.33
    pub fn blocks_halves(&self, item: OrganizationLevel, points: i32) -> Option<i32> {
        if points < 0 {
            return None;
        }
        let r = self.row.iter().find(|r| r.level == item)?;
        let size = r.per_block_of_points?;
        let blocks = points / size + i32::from(points % size != 0);
        blocks.checked_mul(r.stacking_points_per_block_x2?)
    }
}
