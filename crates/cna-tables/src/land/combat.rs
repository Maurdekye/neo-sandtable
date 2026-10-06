//! Combat strength recipes, organization size shifts and captured-casualty percentages.

use std::collections::{BTreeMap, BTreeSet};

use cna_core::dice::Die;
use serde::{Deserialize, Serialize};

use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StrengthActivity {
    Barrage,
    AntiArmor,
    CloseAssault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CombatModifier {
    ForwardBackItalianCombination,
    TerrainBenefitsTarget,
    Terrain,
    Minefields,
    PhasingMustEnterHex,
    ProbeRestriction,
    CombinedArms,
    Morale,
    OrganizationalSize,
    TwoToOneRawSuperiority,
    Engineers,
}

#[derive(Deserialize)]
struct Modifier {
    id: CombatModifier,
}
#[derive(Deserialize)]
struct Activity {
    id: StrengthActivity,
    raw_points: String,
    actual_points: String,
    modifiers: Vec<Modifier>,
}
#[derive(Deserialize)]
struct SummaryBody {
    activity: Vec<Activity>,
}

#[derive(Debug, Clone)]
pub struct StrengthRecipe {
    pub actual_divisor: i32,
    pub modifiers: Vec<CombatModifier>,
}

#[derive(Debug, Clone)]
pub struct CombatCalculations {
    recipes: BTreeMap<StrengthActivity, StrengthRecipe>,
}

impl Bound for CombatCalculations {
    const ID: &'static str = "land.11.4.combat_calculations_summary";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: SummaryBody = raw.deserialize()?;
        let mut recipes = BTreeMap::new();
        for (i, activity) in body.activity.into_iter().enumerate() {
            let expected = match activity.id {
                StrengthActivity::Barrage => {
                    "sum of barrage ratings times assigned TOE strength points"
                }
                StrengthActivity::AntiArmor => {
                    "sum of anti-armor ratings times assigned TOE strength points"
                }
                StrengthActivity::CloseAssault => {
                    "sum of the offensive or defensive close assault ratings (as applies) times assigned TOE strength points"
                }
            };
            if activity.raw_points != expected {
                return Err(raw.err(
                    format!("activity[{i}].raw_points"),
                    "unknown raw-strength recipe",
                ));
            }
            let divisor = activity
                .actual_points
                .strip_prefix("raw points divided by ")
                .and_then(|s| s.parse::<i32>().ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| {
                    raw.err(
                        format!("activity[{i}].actual_points"),
                        "positive raw-point divisor required",
                    )
                })?;
            let modifiers: Vec<_> = activity.modifiers.into_iter().map(|m| m.id).collect();
            let unique: BTreeSet<_> = modifiers.iter().copied().collect();
            let expected_modifiers: &[CombatModifier] = match activity.id {
                StrengthActivity::Barrage => &[
                    CombatModifier::ForwardBackItalianCombination,
                    CombatModifier::TerrainBenefitsTarget,
                ],
                StrengthActivity::AntiArmor => &[
                    CombatModifier::Terrain,
                    CombatModifier::Minefields,
                    CombatModifier::PhasingMustEnterHex,
                ],
                StrengthActivity::CloseAssault => &[
                    CombatModifier::ProbeRestriction,
                    CombatModifier::CombinedArms,
                    CombatModifier::Terrain,
                    CombatModifier::Morale,
                    CombatModifier::OrganizationalSize,
                    CombatModifier::TwoToOneRawSuperiority,
                    CombatModifier::Minefields,
                    CombatModifier::Engineers,
                ],
            };
            if unique.len() != modifiers.len()
                || unique != expected_modifiers.iter().copied().collect()
            {
                return Err(raw.err(
                    format!("activity[{i}].modifiers"),
                    "each modifier for this activity required once",
                ));
            }
            if recipes
                .insert(
                    activity.id,
                    StrengthRecipe {
                        actual_divisor: divisor,
                        modifiers,
                    },
                )
                .is_some()
            {
                return Err(raw.err(format!("activity[{i}].id"), "duplicate activity"));
            }
        }
        if recipes.len() != 3 {
            return Err(raw.err("activity.id", "all three combat activities required"));
        }
        Ok(Self { recipes })
    }
}

impl CombatCalculations {
    /// The cited summary is chart 11.4; its rules source is 11.3.
    /// Cases: land:11.3
    pub fn recipe(&self, activity: StrengthActivity) -> &StrengthRecipe {
        &self.recipes[&activity]
    }

    /// Sum each assigned rating times its TOE points before dividing; negative/overflow input is rejected.
    /// Cases: land:11.3, land:11.31, land:11.34, land:11.35
    pub fn raw_points(&self, assigned: impl IntoIterator<Item = (i32, i32)>) -> Option<i32> {
        assigned.into_iter().try_fold(0i32, |sum, (rating, toe)| {
            if rating < 0 || toe < 0 {
                return None;
            }
            sum.checked_add(rating.checked_mul(toe)?)
        })
    }

    /// Round the pooled raw total to the nearest Actual Point, with ties upward.
    /// Cases: land:11.3, land:11.32, land:11.34
    pub fn actual_points(&self, activity: StrengthActivity, raw_points: i32) -> Option<i32> {
        if raw_points < 0 {
            return None;
        }
        let divisor = i64::from(self.recipe(activity).actual_divisor);
        i32::try_from((i64::from(raw_points) + divisor / 2) / divisor).ok()
    }
    /// When both sides have fewer than ten raw assault points, compare their raw strengths.
    /// Otherwise each side uses the rounded actual strength. Negative input is rejected.
    /// Cases: land:11.33, land:15.51
    pub fn assault_points(&self, attacker_raw: i32, defender_raw: i32) -> Option<(i32, i32)> {
        if attacker_raw < 0 || defender_raw < 0 {
            return None;
        }
        if attacker_raw < 10 && defender_raw < 10 {
            return Some((attacker_raw, defender_raw));
        }
        Some((
            self.actual_points(StrengthActivity::CloseAssault, attacker_raw)?,
            self.actual_points(StrengthActivity::CloseAssault, defender_raw)?,
        ))
    }
}

#[derive(Deserialize)]
struct SizeRow {
    larger_sp: Vec<i32>,
    smaller_sp: i32,
    column_shift: i32,
}
#[derive(Deserialize)]
struct SizeBody {
    row: Vec<SizeRow>,
}
#[derive(Debug, Clone)]
pub struct OrganizationSize {
    shifts: BTreeMap<(i32, i32), i32>,
}

impl Bound for OrganizationSize {
    const ID: &'static str = "land.15.53.organization_size_modifications";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: SizeBody = raw.deserialize()?;
        let mut shifts = BTreeMap::new();
        for (i, row) in body.row.into_iter().enumerate() {
            if row.larger_sp.is_empty()
                || ![0, 1, 2, 3, 5].contains(&row.smaller_sp)
                || row.column_shift < 0
            {
                return Err(raw.err(
                    format!("row[{i}]"),
                    "valid size equivalents and nonnegative shift required",
                ));
            }
            for large in row.larger_sp {
                if ![0, 1, 2, 3, 5].contains(&large)
                    || large <= row.smaller_sp
                    || shifts
                        .insert((large, row.smaller_sp), row.column_shift)
                        .is_some()
                {
                    return Err(raw.err(
                        format!("row[{i}].larger_sp"),
                        "unique larger/smaller size pairs required",
                    ));
                }
            }
        }
        if shifts.len() != 10 {
            return Err(raw.err("row", "all ten unequal size pairs required"));
        }
        Ok(Self { shifts })
    }
}

impl OrganizationSize {
    /// Positive shifts favor the attacker; negative shifts favor the defender. Supply the largest
    /// participating size-equivalent of each side after shell tests, not their total stacking points.
    /// Cases: land:15.53, land:15.52
    pub fn assault_shift(&self, attacker_largest_sp: i32, defender_largest_sp: i32) -> Option<i32> {
        if ![0, 1, 2, 3, 5].contains(&attacker_largest_sp)
            || ![0, 1, 2, 3, 5].contains(&defender_largest_sp)
        {
            return None;
        }
        if attacker_largest_sp == defender_largest_sp {
            return Some(0);
        }
        if attacker_largest_sp > defender_largest_sp {
            self.shifts
                .get(&(attacker_largest_sp, defender_largest_sp))
                .copied()
        } else {
            self.shifts
                .get(&(defender_largest_sp, attacker_largest_sp))
                .map(|n| -n)
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct PrisonerRow {
    die: u8,
    percent_prisoners: i32,
}
#[derive(Debug, Clone, Deserialize)]
pub struct PrisonersCaptured {
    row: Vec<PrisonerRow>,
}
impl Bound for PrisonersCaptured {
    const ID: &'static str = "land.15.89.prisoners_captured";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let table: Self = raw.deserialize()?;
        let mut faces = BTreeSet::new();
        for (i, row) in table.row.iter().enumerate() {
            if !(1..=6).contains(&row.die) || !faces.insert(row.die) {
                return Err(raw.err(format!("row[{i}].die"), "unique faces in 1-6 required"));
            }
            if !(0..=100).contains(&row.percent_prisoners) {
                return Err(raw.err(
                    format!("row[{i}].percent_prisoners"),
                    "percentage in 0-100 required",
                ));
            }
        }
        if faces.len() != 6 {
            return Err(raw.err("row.die", "all six die faces required"));
        }
        Ok(table)
    }
}
impl PrisonersCaptured {
    /// Applies to casualties, not to the whole force; the procedure rounds prisoner losses upward.
    /// Cases: land:15.89, land:15.85
    pub fn percent(&self, die: Die) -> i32 {
        self.row
            .iter()
            .find(|r| r.die == die.value())
            .expect("validated face")
            .percent_prisoners
    }
}
