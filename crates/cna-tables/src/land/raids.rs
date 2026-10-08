//! Typed raid chart cells. Eligibility, dice rolls and follow-up choices belong to rules.
use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::ranges::{IntRange, check_int_tiling};
use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RaidTarget {
    WaterPipeline,
    Airfield,
    AirplanesOnTheGround,
    Rommel,
    SupplyDumpWithCombatUnitsInHex,
    SupplyDumpWithoutCombatUnitsOrRaiderSurvived,
    TrucksInConvoy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RaidEffect {
    TargetDestroyed,
    NoEffect,
    ReduceOneLevelOfEffectiveness,
    TenPercentOfPlanesDestroyedRaiderChooses,
    PlanesUnaffectedRaiderEliminated,
    RaiderSurvivesAndAttacksAsBelow,
    RaiderEliminated,
    TenPercentOfSuppliesDestroyed,
    SuppliesUnaffectedRaiderEliminatedIfCombatUnitsPresent,
    OneTruckPointEliminated,
    TrucksUnaffectedRerollIfInfantryReplacementPointsCarried,
}

/// These are chart predicates, rather than an adjudication of the case-text discrepancy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum GuardCondition {
    #[serde(rename = "sum_at_least_total_raw_defensive_close_assault_points_of_guards")]
    SumAtLeastGuardsDefense,
    #[serde(rename = "sum_below_that_total")]
    SumBelowGuardsDefense,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultRow {
    die: Option<IntRange>,
    condition: Option<GuardCondition>,
    effect: RaidEffect,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetRow {
    id: RaidTarget,
    #[serde(default)]
    two_dice_sum: bool,
    results: Vec<ResultRow>,
    notes: String,
}
#[derive(Deserialize)]
struct Body {
    target: Vec<TargetRow>,
}
#[derive(Debug, Clone)]
struct TargetResults {
    results: Vec<ResultRow>,
    notes: String,
}
#[derive(Debug, Clone)]
pub struct DesertRaiderRaids {
    targets: BTreeMap<RaidTarget, TargetResults>,
}
impl Bound for DesertRaiderRaids {
    const ID: &'static str = "land.27.91.desert_raider_raids";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: Body = raw.deserialize()?;
        let mut targets = BTreeMap::new();
        for (i, row) in body.target.into_iter().enumerate() {
            let field = format!("target[{i}]");
            match row.id {
                RaidTarget::Rommel if !row.two_dice_sum && row.results.is_empty() => {}
                RaidTarget::SupplyDumpWithCombatUnitsInHex if row.two_dice_sum => {
                    let conditions: BTreeSet<_> =
                        row.results.iter().filter_map(|r| r.condition).collect();
                    if conditions
                        != BTreeSet::from([
                            GuardCondition::SumAtLeastGuardsDefense,
                            GuardCondition::SumBelowGuardsDefense,
                        ])
                        || row.results.len() != conditions.len()
                        || row.results.iter().any(|r| r.die.is_some())
                    {
                        return Err(raw.err(
                            format!("{field}.results"),
                            "both distinct guard conditions without die ranges required",
                        ));
                    }
                }
                RaidTarget::Rommel | RaidTarget::SupplyDumpWithCombatUnitsInHex => {
                    return Err(raw.err(
                        format!("{field}.results"),
                        "target requires its separate table or guard-sum shape",
                    ));
                }
                _ => {
                    if row.two_dice_sum
                        || row
                            .results
                            .iter()
                            .any(|r| r.die.is_none() || r.condition.is_some())
                    {
                        return Err(raw.err(
                            format!("{field}.results"),
                            "one-die target requires only die ranges",
                        ));
                    }
                    check_int_tiling(row.results.iter().filter_map(|r| r.die), 1, 6)
                        .map_err(|e| raw.err(format!("{field}.results.die"), e))?;
                }
            }
            if row.notes.trim().is_empty() {
                return Err(raw.err(
                    format!("{field}.notes"),
                    "target restrictions must be retained",
                ));
            }
            if targets
                .insert(
                    row.id,
                    TargetResults {
                        results: row.results,
                        notes: row.notes,
                    },
                )
                .is_some()
            {
                return Err(raw.err(field, "duplicate raid target"));
            }
        }
        let expected = BTreeSet::from([
            RaidTarget::WaterPipeline,
            RaidTarget::Airfield,
            RaidTarget::AirplanesOnTheGround,
            RaidTarget::Rommel,
            RaidTarget::SupplyDumpWithCombatUnitsInHex,
            RaidTarget::SupplyDumpWithoutCombatUnitsOrRaiderSurvived,
            RaidTarget::TrucksInConvoy,
        ]);
        if targets.keys().copied().collect::<BTreeSet<_>>() != expected {
            return Err(raw.err("target", "all raid target records required"));
        }
        Ok(Self { targets })
    }
}
impl DesertRaiderRaids {
    /// Return one chart cell. Rommel and the guards' sum comparison have no one-die cell.
    /// Cases: land:27.91
    pub fn result(&self, target: RaidTarget, die: u8) -> Option<RaidEffect> {
        self.targets
            .get(&target)?
            .results
            .iter()
            .find(|r| r.die.is_some_and(|band| band.contains(i32::from(die))))
            .map(|r| r.effect)
    }
    /// Callers choose the applicable predicate according to the pinned rules interpretation.
    /// Cases: land:27.91
    pub fn guard_result(&self, condition: GuardCondition) -> RaidEffect {
        self.targets[&RaidTarget::SupplyDumpWithCombatUnitsInHex]
            .results
            .iter()
            .find(|r| r.condition == Some(condition))
            .expect("validated guard condition")
            .effect
    }
    /// Printed target restrictions, for review and presentation; procedures do not parse these.
    /// Cases: land:27.91
    pub fn notes(&self, target: RaidTarget) -> &str {
        &self.targets[&target].notes
    }
}

/// The chart's elimination result still needs the procedure's combat/HQ-presence exception.
/// Cases: land:27.63, land:27.92
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum RommelRaidResult {
    /// Axis initiative is three for the next two Game-Turns' initiative determinations.
    #[serde(rename = "ai_temp_3")]
    TemporaryAxisInitiativeThree,
    #[serde(rename = "lrdg_eliminated")]
    RaiderEliminated,
    #[serde(rename = "no_effect")]
    NoEffect,
    /// The native chart key removes the Rommel counter; the raw code is historical.
    #[serde(rename = "ai_perm_3")]
    RemoveRommel,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RommelRow {
    dice_min: i32,
    dice_max: i32,
    result: RommelRaidResult,
}

#[derive(Debug, Clone)]
pub struct RaidOnRommel {
    rows: Vec<RommelRow>,
}

impl Bound for RaidOnRommel {
    const ID: &'static str = "land.27.92.raid_on_rommel";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        #[derive(Deserialize)]
        struct Body {
            row: Vec<RommelRow>,
        }
        let body: Body = raw.deserialize()?;
        for (i, row) in body.row.iter().enumerate() {
            if !(2..=12).contains(&row.dice_min) || !(row.dice_min..=12).contains(&row.dice_max) {
                return Err(raw.err(
                    format!("row[{i}].dice_min/dice_max"),
                    "ordered sum band within 2..=12 required",
                ));
            }
        }
        check_int_tiling(
            body.row
                .iter()
                .map(|r| IntRange::new(r.dice_min, r.dice_max)),
            2,
            12,
        )
        .map_err(|e| raw.err("row.dice_min/dice_max", e))?;
        Ok(Self { rows: body.row })
    }
}

impl RaidOnRommel {
    /// Read the two-dice sum, not a tens-and-units reading. Unprinted totals have no cell.
    /// Cases: land:27.63, land:27.92
    pub fn result(&self, total: i32) -> Option<RommelRaidResult> {
        self.rows
            .iter()
            .find(|r| (r.dice_min..=r.dice_max).contains(&total))
            .map(|r| r.result)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SasRow {
    die: i32,
    percent_planes_destroyed: i32,
}

#[derive(Debug, Clone)]
pub struct SasBrigadeRaid {
    percentages: [i32; 6],
}

impl Bound for SasBrigadeRaid {
    const ID: &'static str = "land.27.93.sas_brigade_raid";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        #[derive(Deserialize)]
        struct Body {
            row: Vec<SasRow>,
        }
        let body: Body = raw.deserialize()?;
        let mut cells = [None; 6];
        for (i, row) in body.row.into_iter().enumerate() {
            if !(1..=6).contains(&row.die) {
                return Err(raw.err(format!("row[{i}].die"), "die within 1..=6 required"));
            }
            if !(0..=100).contains(&row.percent_planes_destroyed) {
                return Err(raw.err(
                    format!("row[{i}].percent_planes_destroyed"),
                    "percentage within 0..=100 required",
                ));
            }
            if cells[(row.die - 1) as usize]
                .replace(row.percent_planes_destroyed)
                .is_some()
            {
                return Err(raw.err(format!("row[{i}].die"), "duplicate die"));
            }
        }
        if cells.iter().any(Option::is_none) {
            return Err(raw.err("row.die", "each die from 1 through 6 required"));
        }
        Ok(Self {
            percentages: cells.map(|cell| cell.expect("validated die")),
        })
    }
}

impl SasBrigadeRaid {
    /// Integer percentage for each aircraft type and make; the procedure applies losses.
    /// Cases: land:27.86, land:27.93
    pub fn percentage_destroyed(&self, die: i32) -> Option<i32> {
        (1..=6)
            .contains(&die)
            .then(|| self.percentages[(die - 1) as usize])
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChariotRow {
    die_min: i32,
    die_max: i32,
    dice_to_roll: i32,
}

#[derive(Debug, Clone)]
pub struct ChariotRaid {
    rows: Vec<ChariotRow>,
}

impl Bound for ChariotRaid {
    const ID: &'static str = "land.30.46.chariot_raid";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        #[derive(Deserialize)]
        struct Body {
            row: Vec<ChariotRow>,
        }
        let body: Body = raw.deserialize()?;
        for (i, row) in body.row.iter().enumerate() {
            if !(1..=6).contains(&row.die_min) || !(row.die_min..=6).contains(&row.die_max) {
                return Err(raw.err(
                    format!("row[{i}].die_min/die_max"),
                    "ordered die band within 1..=6 required",
                ));
            }
            if !(0..=3).contains(&row.dice_to_roll) {
                return Err(raw.err(
                    format!("row[{i}].dice_to_roll"),
                    "secondary dice count within 0..=3 required",
                ));
            }
        }
        check_int_tiling(
            body.row.iter().map(|r| IntRange::new(r.die_min, r.die_max)),
            1,
            6,
        )
        .map_err(|e| raw.err("row.die_min/die_max", e))?;
        Ok(Self { rows: body.row })
    }
}

impl ChariotRaid {
    /// Number of secondary damage dice. Zero produces no damage; rolling and allocation are external.
    /// Cases: land:30.44, land:30.46
    pub fn damage_dice(&self, die: i32) -> Option<i32> {
        self.rows
            .iter()
            .find(|r| (r.die_min..=r.die_max).contains(&die))
            .map(|r| r.dice_to_roll)
    }
}
