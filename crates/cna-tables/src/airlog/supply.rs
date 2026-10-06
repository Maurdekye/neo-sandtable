//! Ammunition, water and supply-dump tables (sections 50-54).

use cna_core::dice::Die;
use cna_core::quantity::{AmmoPoints, WaterPoints};
use serde::Deserialize;

use crate::ranges::{IntRange, check_int_tiling};
use crate::{Bound, RawTable, TableError};

// ---------------------------------------------------------------------------------------------
// 50.2 Ammunition Consumption
// ---------------------------------------------------------------------------------------------

/// Whether the Logistics game is played in full or abstracted (`airlog:50.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmmoMode {
    Played,
    Abstracted,
}

/// The actions the Ammunition Consumption Chart prices, named as in the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmmoAction {
    Barrage,
    AntiArmor,
    CloseAssaultArmorGunMgInfHvywpnInf,
    CloseAssaultInfClass,
    AntiAirSingleTargetGroup,
    RearmSquadronTacair,
    RearmOnePlaneBombsTorpedoesMines,
    AirToAirCombatOrStrafe,
    BarragePhasingBattalionEq,
    BarrageNonPhasingBattalionEq,
    AssaultPhasingBattalionEq,
    AssaultNonPhasingBattalionEq,
    AntiAirGunClassSingleTargetGroup,
    RearmNonFighterSquadronTacairAndBombload,
    FlightByNonFighterPlane,
}

/// What an action costs: a fixed number of points, or a rating-based cost the chart prints as
/// a word (see `data/tables/airlog/GAPS.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AmmoCost {
    Points(AmmoPoints),
    /// The cost is the named rating (`tacair_bombload`, `bombload`); the chart gives no number.
    Rating(String),
}

#[derive(Debug, Clone, Deserialize)]
struct AmmoRowRaw {
    mode: AmmoMode,
    action: AmmoAction,
    #[serde(default)]
    ammo_points: Option<i32>,
    #[serde(default)]
    ammo_cost_basis: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct AmmoRaw {
    row: Vec<AmmoRowRaw>,
}

/// The Ammunition Consumption Chart (`airlog:50.2`).
#[derive(Debug, Clone)]
pub struct AmmunitionConsumption {
    rows: Vec<(AmmoMode, AmmoAction, AmmoCost)>,
}

impl Bound for AmmunitionConsumption {
    const ID: &'static str = "airlog.50.2.ammunition_consumption";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: AmmoRaw = raw.deserialize()?;
        let mut rows: Vec<(AmmoMode, AmmoAction, AmmoCost)> = Vec::new();
        for (i, row) in r.row.into_iter().enumerate() {
            let cost = match (row.ammo_points, row.ammo_cost_basis) {
                (Some(p), None) if p >= 0 => AmmoCost::Points(AmmoPoints::new(p)),
                (None, Some(b)) => AmmoCost::Rating(b),
                _ => {
                    return Err(raw.err(
                        format!("row[{i}]"),
                        "give exactly one of ammo_points (>= 0) or ammo_cost_basis",
                    ));
                }
            };
            if rows
                .iter()
                .any(|(m, a, _)| *m == row.mode && *a == row.action)
            {
                return Err(raw.err(
                    format!("row[{i}].action"),
                    "action priced twice in one mode",
                ));
            }
            rows.push((row.mode, row.action, cost));
        }
        Ok(Self { rows })
    }
}

impl AmmunitionConsumption {
    /// Ammunition cost of `action` in `mode`, or `None` if the chart does not price that pair.
    /// `airlog:50.2`.
    pub fn cost(&self, mode: AmmoMode, action: AmmoAction) -> Option<&AmmoCost> {
        self.rows
            .iter()
            .find(|(m, a, _)| *m == mode && *a == action)
            .map(|(_, _, c)| c)
    }

    /// Fixed point cost of `action`, or `None` when the chart prints a rating instead or does
    /// not price the pair. `airlog:50.2`.
    pub fn points(&self, mode: AmmoMode, action: AmmoAction) -> Option<AmmoPoints> {
        match self.cost(mode, action)? {
            AmmoCost::Points(p) => Some(*p),
            AmmoCost::Rating(_) => None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 52.7 Water Availability and 52.8 Poisoning and Sweetening
// ---------------------------------------------------------------------------------------------

/// A water source that is drawn on by die roll (`airlog:52.7`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WellSource {
    /// A town (case 52.13 calls it a village).
    Town,
    Bir,
}

/// What one draw at a well yields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaterDraw {
    pub water: WaterPoints,
    /// After this result the drawing player rolls once more, secretly; see
    /// [`WaterAvailability::depleted_on`].
    pub depletion_check: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct WaterRowRaw {
    source: WellSource,
    die: u8,
    water_points: i32,
    depletion_check: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct DepletionRaw {
    depleted_on: u8,
}

#[derive(Debug, Clone, Deserialize)]
struct WaterRaw {
    row: Vec<WaterRowRaw>,
    depletion: DepletionRaw,
}

/// The Water Availability Chart (`airlog:52.7`).
#[derive(Debug, Clone)]
pub struct WaterAvailability {
    town: [WaterDraw; 6],
    bir: [WaterDraw; 6],
    depleted_on: u8,
}

impl Bound for WaterAvailability {
    const ID: &'static str = "airlog.52.7.water_availability";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: WaterRaw = raw.deserialize()?;
        let empty = WaterDraw {
            water: WaterPoints::ZERO,
            depletion_check: false,
        };
        let mut town = [empty; 6];
        let mut bir = [empty; 6];
        let mut seen = [[false; 6]; 2];
        for (i, row) in r.row.iter().enumerate() {
            if !(1..=6).contains(&row.die) {
                return Err(raw.err(format!("row[{i}].die"), "must be 1-6"));
            }
            let slot = usize::from(row.die - 1);
            let (which, table) = match row.source {
                WellSource::Town => (0, &mut town),
                WellSource::Bir => (1, &mut bir),
            };
            if std::mem::replace(&mut seen[which][slot], true) {
                return Err(raw.err(format!("row[{i}].die"), "die face given twice"));
            }
            table[slot] = WaterDraw {
                water: WaterPoints::new(row.water_points),
                depletion_check: row.depletion_check,
            };
        }
        if seen.iter().flatten().any(|s| !s) {
            return Err(raw.err("row", "every die face 1-6 is needed for both sources"));
        }
        if !(1..=6).contains(&r.depletion.depleted_on) {
            return Err(raw.err("depletion.depleted_on", "must be 1-6"));
        }
        Ok(Self {
            town,
            bir,
            depleted_on: r.depletion.depleted_on,
        })
    }
}

impl WaterAvailability {
    /// Water drawn by one die roll at a well. `airlog:52.7`.
    pub fn draw(&self, source: WellSource, die: Die) -> WaterDraw {
        let slot = usize::from(die.value() - 1);
        match source {
            WellSource::Town => self.town[slot],
            WellSource::Bir => self.bir[slot],
        }
    }

    /// The die face on which a checked well is depleted. `airlog:52.7`.
    pub fn depleted_on(&self) -> Die {
        Die::new(self.depleted_on).expect("validated at load")
    }
}

/// The two things a player can attempt on a well (`airlog:52.8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WellAttempt {
    PoisonWell,
    SweetenWell,
}

/// The outcome of a poisoning or sweetening attempt (`airlog:52.8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WellEffect {
    WellPoisoned,
    NoEffect,
    WellSweetened,
    NoEffectStillPoisoned,
}

#[derive(Debug, Clone, Deserialize)]
struct PoisonRowRaw {
    attempt: WellAttempt,
    die_range: IntRange,
    result: WellEffect,
}

#[derive(Debug, Clone, Deserialize)]
struct PoisonRaw {
    row: Vec<PoisonRowRaw>,
}

/// The Poisoning and Sweetening chart (`airlog:52.8`).
#[derive(Debug, Clone)]
pub struct PoisoningAndSweetening {
    rows: Vec<(WellAttempt, IntRange, WellEffect)>,
}

impl Bound for PoisoningAndSweetening {
    const ID: &'static str = "airlog.52.8.poisoning_and_sweetening";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: PoisonRaw = raw.deserialize()?;
        for attempt in [WellAttempt::PoisonWell, WellAttempt::SweetenWell] {
            check_int_tiling(
                r.row
                    .iter()
                    .filter(|x| x.attempt == attempt)
                    .map(|x| x.die_range),
                1,
                6,
            )
            .map_err(|m| raw.err(format!("row ({attempt:?})"), m))?;
        }
        Ok(Self {
            rows: r
                .row
                .into_iter()
                .map(|x| (x.attempt, x.die_range, x.result))
                .collect(),
        })
    }
}

impl PoisoningAndSweetening {
    /// The result of one attempt. `airlog:52.8`.
    pub fn result(&self, attempt: WellAttempt, die: Die) -> WellEffect {
        let v = i32::from(die.value());
        self.rows
            .iter()
            .find(|(a, r, _)| *a == attempt && r.contains(v))
            .map(|(_, _, e)| *e)
            .expect("validated: the rows tile 1-6")
    }
}

// ---------------------------------------------------------------------------------------------
// 54.12 Supply Dump Capacity
// ---------------------------------------------------------------------------------------------

/// The four supply types (`airlog:49`-`52`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupplyType {
    Ammo,
    Fuel,
    Stores,
    Water,
}

/// Where a supply dump sits (`airlog:54.12`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DumpLocation {
    TunisTripoli,
    MajorCity,
    Village,
    OtherTerrain,
    /// A hex that is not a dump: what units and trucks leave lying there.
    NonDump,
}

/// How many points of one supply a location can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpCapacity {
    Unlimited,
    Max(i32),
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct CapacityRaw {
    #[serde(default)]
    unlimited: bool,
    #[serde(default)]
    max_points: Option<i32>,
}

impl CapacityRaw {
    fn resolve(self) -> Result<DumpCapacity, String> {
        match (self.unlimited, self.max_points) {
            (true, None) => Ok(DumpCapacity::Unlimited),
            (false, Some(n)) if n >= 0 => Ok(DumpCapacity::Max(n)),
            _ => Err("give exactly one of unlimited = true or max_points >= 0".to_string()),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct DumpRowRaw {
    location: DumpLocation,
    ammo: CapacityRaw,
    fuel: CapacityRaw,
    stores: CapacityRaw,
    water: CapacityRaw,
}

#[derive(Debug, Clone, Deserialize)]
struct DumpRaw {
    row: Vec<DumpRowRaw>,
}

/// The Supply Dump Capacity chart (`airlog:54.12`).
#[derive(Debug, Clone)]
pub struct SupplyDumpCapacity {
    rows: Vec<(DumpLocation, [DumpCapacity; 4])>,
}

impl Bound for SupplyDumpCapacity {
    const ID: &'static str = "airlog.54.12.supply_dump_capacity";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: DumpRaw = raw.deserialize()?;
        let mut rows = Vec::new();
        for (i, row) in r.row.iter().enumerate() {
            let cell = |name: &str, c: CapacityRaw| {
                c.resolve()
                    .map_err(|m| raw.err(format!("row[{i}].{name}"), m))
            };
            rows.push((
                row.location,
                [
                    cell("ammo", row.ammo)?,
                    cell("fuel", row.fuel)?,
                    cell("stores", row.stores)?,
                    cell("water", row.water)?,
                ],
            ));
        }
        for loc in [
            DumpLocation::TunisTripoli,
            DumpLocation::MajorCity,
            DumpLocation::Village,
            DumpLocation::OtherTerrain,
            DumpLocation::NonDump,
        ] {
            if rows.iter().filter(|(l, _)| *l == loc).count() != 1 {
                return Err(raw.err("row", format!("location {loc:?} must appear exactly once")));
            }
        }
        Ok(Self { rows })
    }
}

impl SupplyDumpCapacity {
    /// Most points of `supply` a dump at `location` may hold. `airlog:54.12`.
    pub fn capacity(&self, location: DumpLocation, supply: SupplyType) -> DumpCapacity {
        let row = self
            .rows
            .iter()
            .find(|(l, _)| *l == location)
            .expect("validated: every location present");
        row.1[match supply {
            SupplyType::Ammo => 0,
            SupplyType::Fuel => 1,
            SupplyType::Stores => 2,
            SupplyType::Water => 3,
        }]
    }
}

// ---------------------------------------------------------------------------------------------
// 54.17 Supply Dump Demolition
// ---------------------------------------------------------------------------------------------

/// The conditions that modify a demolition die roll (`airlog:54.17`), named as in the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DemolitionCondition {
    /// Applies once for each additional third of the unit basic CPA expended.
    PerAdditionalThirdOfBasicCpaExpended,
    AttemptingUnitIsFullNonShellDivision,
    AttemptingUnitsTotalOneStackingPointOrLess,
    AttemptInMajorCityHex,
    #[serde(rename = "not_major_city_and_dump_total_supplies_500_or_less")]
    NotMajorCityAndDumpTotalSupplies500OrLess,
    #[serde(rename = "not_major_city_and_dump_total_supplies_4000_or_more")]
    NotMajorCityAndDumpTotalSupplies4000OrMore,
    AttemptingUnitsJustCapturedTheDump,
    #[serde(
        rename = "dump_not_just_captured_and_nearest_enemy_unit_at_least_20_cp_via_medium_truck_away"
    )]
    DumpNotJustCapturedAndNearestEnemyUnitAtLeast20CpViaMediumTruckAway,
}

/// One die-roll modifier of the demolition chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct DemolitionModifier {
    /// The chart `and`-joined clause; conditions in one group are alternatives.
    pub group: u8,
    pub condition: DemolitionCondition,
    pub modifier: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct DemolitionRowRaw {
    modified_die: i32,
    #[serde(default)]
    or_more: bool,
    percent_destroyed: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct DemolitionRaw {
    row: Vec<DemolitionRowRaw>,
    modifier: Vec<DemolitionModifier>,
}

/// The Supply Dump Demolition chart (`airlog:54.17`).
#[derive(Debug, Clone)]
pub struct SupplyDumpDemolition {
    lowest: i32,
    highest: i32,
    /// Percent destroyed for `lowest..=highest`.
    percents: Vec<i32>,
    modifiers: Vec<DemolitionModifier>,
}

impl Bound for SupplyDumpDemolition {
    const ID: &'static str = "airlog.54.17.supply_dump_demolition";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: DemolitionRaw = raw.deserialize()?;
        let first = r.row.first().ok_or_else(|| raw.err("row", "no rows"))?;
        let lowest = first.modified_die;
        let mut percents: Vec<i32> = Vec::new();
        let mut highest = lowest;
        for (i, row) in r.row.iter().enumerate() {
            if row.modified_die != lowest + i as i32 {
                return Err(raw.err(
                    format!("row[{i}].modified_die"),
                    "columns must be consecutive integers",
                ));
            }
            if !(0..=100).contains(&row.percent_destroyed) {
                return Err(raw.err(format!("row[{i}].percent_destroyed"), "must be 0-100"));
            }
            if percents.last().is_some_and(|&p| row.percent_destroyed < p) {
                return Err(raw.err(
                    format!("row[{i}].percent_destroyed"),
                    "destruction must not fall as the roll rises",
                ));
            }
            if row.or_more && i + 1 != r.row.len() {
                return Err(raw.err(
                    format!("row[{i}].or_more"),
                    "only the last row is open-ended",
                ));
            }
            percents.push(row.percent_destroyed);
            highest = row.modified_die;
        }
        if !r.row.last().is_some_and(|x| x.or_more) {
            return Err(raw.err(
                "row",
                "the last row must be the open-ended `or more` column",
            ));
        }
        for (i, m) in r.modifier.iter().enumerate() {
            if !(1..=4).contains(&m.group) {
                return Err(raw.err(format!("modifier[{i}].group"), "must be 1-4"));
            }
        }
        Ok(Self {
            lowest,
            highest,
            percents,
            modifiers: r.modifier,
        })
    }
}

impl SupplyDumpDemolition {
    /// Percent of every supply type destroyed at a modified die roll.
    ///
    /// `interp:airlog-0002`: rolls below the lowest printed column (-2) destroy 0%, like the
    /// column itself; the last column applies to that roll and every higher one. `airlog:54.17`.
    pub fn percent_destroyed(&self, modified_die: i32) -> i32 {
        let clamped = modified_die.clamp(self.lowest, self.highest);
        self.percents[(clamped - self.lowest) as usize]
    }

    /// The printed die-roll modifiers, in chart order. `airlog:54.17`.
    pub fn modifiers(&self) -> &[DemolitionModifier] {
        &self.modifiers
    }

    /// The modifier a condition adds to the roll (once, or per third for the CPA clause).
    /// `airlog:54.17`.
    pub fn modifier_for(&self, condition: DemolitionCondition) -> i32 {
        self.modifiers
            .iter()
            .find(|m| m.condition == condition)
            .map_or(0, |m| m.modifier)
    }
}
