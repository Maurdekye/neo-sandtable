//! Typed outcomes of the raider chart. Guard comparisons and follow-up choices belong to rules.
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
