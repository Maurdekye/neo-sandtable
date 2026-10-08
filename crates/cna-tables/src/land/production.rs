//! Printed production quantities. Scheduling, depletion, shipment and dice remain procedural.
use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::airlog::trucks::TruckType;
use crate::{Bound, RawTable, TableError};

/// Axis and Commonwealth marks have different meanings; these are the resolved chart periods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductionPeriod {
    GameTurn,
    HalfCalendarMonth,
    CalendarMonth,
    /// The PzIV E numeral is legible, but the adjoining period mark is damaged.
    Unreadable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductionMaximum {
    Points {
        points: i32,
        period: ProductionPeriod,
    },
    /// A printed dash is neither a numeric zero nor permission for unlimited production.
    PrintedDash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductionDates {
    pub first_game_turn: i32,
    /// None retains the lack of a printed final date.
    pub last_game_turn: Option<i32>,
}
impl ProductionDates {
    /// Test an explicitly supplied chart date; no arrival/planning conversion is performed.
    /// Cases: land:20.66, land:20.78
    pub fn contains(self, chart_game_turn: i32) -> bool {
        (1..=111).contains(&chart_game_turn)
            && chart_game_turn >= self.first_game_turn
            && self
                .last_game_turn
                .is_none_or(|last| chart_game_turn <= last)
    }
}

fn dates(
    raw: &RawTable,
    field: &str,
    first: i32,
    last: Option<i32>,
) -> Result<ProductionDates, TableError> {
    if !(1..=111).contains(&first) || last.is_some_and(|n| n < first || n > 111) {
        return Err(raw.err(field, "ordered campaign dates required"));
    }
    Ok(ProductionDates {
        first_game_turn: first,
        last_game_turn: last,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AxisProductionNation {
    German,
    Italian,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxisAvailability {
    /// Index into the entry's printed pools. Consecutive lines can share one pool.
    pub pool_index: usize,
    pub maximum: ProductionMaximum,
    pub dates: ProductionDates,
    /// French pieces use the Tunis call-up exception instead of ordinary delayed shipment.
    pub french_tunis_exception: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxisProductionEntry {
    pub pools: Vec<i32>,
    pub tonnage_per_point: i32,
    pub availability: Vec<AxisAvailability>,
    pub autobelinda_41_only: bool,
}
impl AxisProductionEntry {
    /// Find the printed availability line using a planning date, not an inferred arrival date.
    /// Cases: land:20.66
    /// Interpretation: interp:land-0014
    pub fn availability_at(&self, planning_game_turn: i32) -> Option<&AxisAvailability> {
        self.availability
            .iter()
            .find(|line| line.dates.contains(planning_game_turn))
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AxisMark {
    Star,
    Dagger,
    DoubleDagger,
    Unreadable,
}
fn axis_max(points: i32, marker: Option<AxisMark>) -> ProductionMaximum {
    let period = match marker {
        None | Some(AxisMark::DoubleDagger) => ProductionPeriod::GameTurn,
        Some(AxisMark::Star) => ProductionPeriod::HalfCalendarMonth,
        Some(AxisMark::Dagger) => ProductionPeriod::CalendarMonth,
        Some(AxisMark::Unreadable) => ProductionPeriod::Unreadable,
    };
    ProductionMaximum::Points { points, period }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AxisLine {
    max: i32,
    first_game_turn: i32,
    last_game_turn: Option<i32>,
    pool: Option<i32>,
    marker: Option<AxisMark>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AxisTruck {
    truck_type: TruckType,
    number: i32,
    tonnage: i32,
    availability: Vec<AxisLine>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GermanRow {
    item: String,
    number: i32,
    max_per_game_turn: i32,
    max_marker: Option<AxisMark>,
    first_game_turn: i32,
    last_game_turn: Option<i32>,
    tonnage: i32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItalianRow {
    item: String,
    item_marker: Option<String>,
    numbers: Vec<i32>,
    availability: Vec<AxisLine>,
    tonnage: i32,
}
#[derive(Deserialize)]
struct AxisBody {
    truck_row: Vec<AxisTruck>,
    german_row: Vec<GermanRow>,
    italian_row: Vec<ItalianRow>,
}

const GERMAN_ITEMS: &[&str] = &[
    "Infantry",
    "Armed Recce",
    "Light AA",
    "Heavy AA",
    "7.5cm IG18",
    "10.5cm K18",
    "15cm sFH18",
    "15cm sIG33",
    "15cm K18",
    "17cm K18",
    "21cm Mrs.18",
    "SP 10.5cm",
    "5cm Pak38",
    "2.8cm s.Pz.B.41 28/20 Pak",
    "7.62cm Pak(R)",
    "Marder III",
    "PzII",
    "PzIII E",
    "PzIII H",
    "PzIII J Special",
    "PzIV D",
    "PzIV E",
    "PzIV F2 Special",
];
const ITALIAN_ITEMS: &[&str] = &[
    "Infantry",
    "Armored Reconnaissance/Armored Car",
    "Light AA",
    "Heavy AA (75/46)",
    "Heavy AA (90/53)",
    "47/32 Mod. 37",
    "65/17 Gun",
    "75/27 Gun",
    "75/18 Howitzer",
    "100/17 Howitzer",
    "105/28 Gun",
    "Semovente 75/18 SP Gun",
    "149/13 Howitzer",
    "ParaArt",
    "149mm (Fr.)",
    "155mm Rimhailo (Fr.)",
    "CV L.3 or CV 33/35",
    "CA L6/40",
    "CA M 13/40",
    "CA M 14/41",
];
fn keys_complete(
    raw: &RawTable,
    field: &str,
    actual: &BTreeSet<String>,
    expected: &[&str],
) -> Result<(), TableError> {
    if actual != &expected.iter().map(|s| (*s).to_owned()).collect() {
        return Err(raw.err(field, "exact complete printed item set required"));
    }
    Ok(())
}
fn make_axis_entry(
    raw: &RawTable,
    field: &str,
    pools: Vec<i32>,
    tonnage: i32,
    lines: Vec<AxisLine>,
    auto: bool,
    french: bool,
) -> Result<AxisProductionEntry, TableError> {
    if pools.is_empty()
        || pools.iter().any(|n| *n <= 0)
        || tonnage <= 0
        || pools
            .iter()
            .try_fold(0_i32, |sum, n| sum.checked_add(*n))
            .is_none()
        || lines.is_empty()
    {
        return Err(raw.err(
            field,
            "positive pools, tonnage and complete availability required",
        ));
    }
    let mut availability = Vec::new();
    let mut pool_index = 0;
    for (i, line) in lines.into_iter().enumerate() {
        let path = format!("{field}.availability[{i}]");
        if let Some(pool) = line.pool {
            if pool != pools[pool_index] {
                if i == 0 {
                    return Err(raw.err(
                        &path,
                        "first availability line must reference the first pool",
                    ));
                }
                pool_index += 1;
                if pools.get(pool_index) != Some(&pool) {
                    return Err(
                        raw.err(&path, "availability must follow the printed pools in order")
                    );
                }
            }
        } else if pools.len() != 1 {
            return Err(raw.err(&path, "multi-pool lines require a pool figure"));
        }
        if line.max <= 0
            || line.max > pools[pool_index]
            || matches!(line.marker, Some(AxisMark::Unreadable))
            || (matches!(line.marker, Some(AxisMark::DoubleDagger)) != french)
        {
            return Err(raw.err(
                &path,
                "positive pool-bounded maximum and applicable mark required",
            ));
        }
        let line_dates = dates(raw, &path, line.first_game_turn, line.last_game_turn)?;
        if let Some(previous) = availability.last() {
            let previous: &AxisAvailability = previous;
            if previous.dates.last_game_turn.and_then(|n| n.checked_add(1))
                != Some(line_dates.first_game_turn)
            {
                return Err(raw.err(
                    &path,
                    "availability lines must be adjacent and nonoverlapping",
                ));
            }
        }
        availability.push(AxisAvailability {
            pool_index,
            maximum: axis_max(line.max, line.marker),
            dates: line_dates,
            french_tunis_exception: french,
        });
    }
    if pool_index + 1 != pools.len() {
        return Err(raw.err(field, "every printed pool requires an availability line"));
    }
    Ok(AxisProductionEntry {
        pools,
        tonnage_per_point: tonnage,
        availability,
        autobelinda_41_only: auto,
    })
}

#[derive(Debug, Clone)]
pub struct AxisReplacementPool {
    trucks: BTreeMap<TruckType, AxisProductionEntry>,
    equipment: BTreeMap<(AxisProductionNation, String), AxisProductionEntry>,
}
impl Bound for AxisReplacementPool {
    const ID: &'static str = "land.20.66.axis_replacement_pool";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: AxisBody = raw.deserialize()?;
        let mut trucks = BTreeMap::new();
        for (i, row) in body.truck_row.into_iter().enumerate() {
            if row
                .availability
                .iter()
                .any(|line| line.pool.is_some() || line.marker.is_some())
            {
                return Err(raw.err(
                    format!("truck_row[{i}]"),
                    "truck lines do not carry pool or period marks",
                ));
            }
            let entry = make_axis_entry(
                raw,
                &format!("truck_row[{i}]"),
                vec![row.number],
                row.tonnage,
                row.availability,
                false,
                false,
            )?;
            if trucks.insert(row.truck_type, entry).is_some() {
                return Err(raw.err("truck_row.truck_type", "duplicate truck type"));
            }
        }
        if trucks.len() != 3 {
            return Err(raw.err("truck_row", "all three truck types required"));
        }
        let mut equipment = BTreeMap::new();
        let mut german = BTreeSet::new();
        for (i, row) in body.german_row.into_iter().enumerate() {
            let field = format!("german_row[{i}]");
            if !german.insert(row.item.clone())
                || row.number <= 0
                || row.tonnage <= 0
                || row.max_per_game_turn <= 0
                || row.max_per_game_turn > row.number
                || matches!(row.max_marker, Some(AxisMark::DoubleDagger))
                || (matches!(row.max_marker, Some(AxisMark::Unreadable)) != (row.item == "PzIV E"))
            {
                return Err(raw.err(
                    &field,
                    "unique item, positive quantities and applicable period required",
                ));
            }
            let line = AxisAvailability {
                pool_index: 0,
                maximum: axis_max(row.max_per_game_turn, row.max_marker),
                dates: dates(raw, &field, row.first_game_turn, row.last_game_turn)?,
                french_tunis_exception: false,
            };
            equipment.insert(
                (AxisProductionNation::German, row.item),
                AxisProductionEntry {
                    pools: vec![row.number],
                    tonnage_per_point: row.tonnage,
                    availability: vec![line],
                    autobelinda_41_only: false,
                },
            );
        }
        keys_complete(raw, "german_row.item", &german, GERMAN_ITEMS)?;
        let mut italian = BTreeSet::new();
        for (i, row) in body.italian_row.into_iter().enumerate() {
            let field = format!("italian_row[{i}]");
            let auto = row.item == "Armored Reconnaissance/Armored Car";
            let french = matches!(row.item.as_str(), "149mm (Fr.)" | "155mm Rimhailo (Fr.)");
            if !italian.insert(row.item.clone())
                || row.item_marker.as_deref() != auto.then_some("star_all_autobelinda_41")
                || row.availability.iter().any(|line| line.pool.is_none())
            {
                return Err(raw.err(
                    &field,
                    "unique item and exact armored-car restriction required",
                ));
            }
            let entry = make_axis_entry(
                raw,
                &field,
                row.numbers,
                row.tonnage,
                row.availability,
                auto,
                french,
            )?;
            if french
                && (entry.availability.len() != 1
                    || entry.availability[0].dates.first_game_turn != 39
                    || entry.availability[0].maximum != axis_max(2, Some(AxisMark::DoubleDagger)))
            {
                return Err(raw.err(
                    &field,
                    "French Tunis call-up requires its printed turn and maximum",
                ));
            }
            equipment.insert((AxisProductionNation::Italian, row.item), entry);
        }
        keys_complete(raw, "italian_row.item", &italian, ITALIAN_ITEMS)?;
        Ok(Self { trucks, equipment })
    }
}
impl AxisReplacementPool {
    /// Truck pool and availability data in the chart's printed point units.
    /// Cases: land:20.66
    pub fn truck(&self, kind: TruckType) -> &AxisProductionEntry {
        &self.trucks[&kind]
    }
    /// Native chart designations, not runtime equipment IDs. Unsupported items remain absent.
    /// Cases: land:20.66
    pub fn equipment(
        &self,
        nation: AxisProductionNation,
        chart_item: &str,
    ) -> Option<&AxisProductionEntry> {
        self.equipment.get(&(nation, chart_item.to_owned()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruckProductionColumn {
    GameTurns1To30,
    GameTurns31To107,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum InfantryProductionColumn {
    #[serde(rename = "gt_3_to_30")]
    GameTurns3To30,
    #[serde(rename = "gt_31_to_46")]
    GameTurns31To46,
    #[serde(rename = "gt_47_to_102")]
    GameTurns47To102,
    #[serde(rename = "gt_103_to_107")]
    GameTurns103To107,
}
impl InfantryProductionColumn {
    pub const ALL: [Self; 4] = [
        Self::GameTurns3To30,
        Self::GameTurns31To46,
        Self::GameTurns47To102,
        Self::GameTurns103To107,
    ];
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfantryProductionResult {
    Points(i32),
    PrintedNone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonwealthProductionEntry {
    pub pool_points: i32,
    pub maximum: ProductionMaximum,
    pub dates: ProductionDates,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TruckQuantities {
    light: i32,
    medium: i32,
    heavy: i32,
}
impl TruckQuantities {
    fn get(&self, kind: TruckType) -> i32 {
        match kind {
            TruckType::Light => self.light,
            TruckType::Medium => self.medium,
            TruckType::Heavy => self.heavy,
        }
    }
    fn valid(&self) -> bool {
        self.light >= 0 && self.medium >= 0 && self.heavy >= 0
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommonwealthTruckRow {
    die: i32,
    game_turns_1_to_30: TruckQuantities,
    game_turns_31_to_107: TruckQuantities,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InfantryRow {
    dice_sum: i32,
    points_by_game_turns: BTreeMap<InfantryProductionColumn, i32>,
    none_in: Vec<InfantryProductionColumn>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum CommonwealthPeriod {
    Month,
    HalfMonth,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommonwealthRow {
    item: String,
    number: i32,
    max_per_game_turn: Option<i32>,
    max_limit_period: Option<CommonwealthPeriod>,
    #[serde(default)]
    max_per_game_turn_printed_dash: bool,
    first_game_turn: i32,
    last_game_turn: Option<i32>,
}
#[derive(Deserialize)]
struct CommonwealthBody {
    truck_row: Vec<CommonwealthTruckRow>,
    infantry_row: Vec<InfantryRow>,
    chart_row: Vec<CommonwealthRow>,
}
const CW_ITEMS: &[&str] = &[
    "Armored Recce/Armored Car",
    "Light AA",
    "Heavy AA",
    "25-pounders",
    "4.5-inch Guns",
    "5.5-inch Howitzers",
    "155mm Howitzers",
    "2-pounders",
    "6-pounders",
    "SP 6-pounders",
    "17-pounders",
    "Mark VI Light",
    "A9 Cruiser",
    "A10 Cruiser",
    "A13 Cruiser",
    "Crusader Mk I",
    "Crusader Mk II",
    "Crusader Mk III",
    "Matilda",
    "Valentine",
    "Stuart",
    "Grant",
    "Sherman",
    "Churchill",
];
#[derive(Debug, Clone)]
pub struct CommonwealthProduction {
    trucks: BTreeMap<i32, [BTreeMap<TruckType, i32>; 2]>,
    infantry: BTreeMap<(i32, InfantryProductionColumn), InfantryProductionResult>,
    equipment: BTreeMap<String, CommonwealthProductionEntry>,
}
impl Bound for CommonwealthProduction {
    const ID: &'static str = "land.20.78.commonwealth_production";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: CommonwealthBody = raw.deserialize()?;
        let mut trucks = BTreeMap::new();
        for (i, row) in body.truck_row.into_iter().enumerate() {
            if !(1..=6).contains(&row.die)
                || !row.game_turns_1_to_30.valid()
                || !row.game_turns_31_to_107.valid()
                || trucks.contains_key(&row.die)
            {
                return Err(raw.err(
                    format!("truck_row[{i}]"),
                    "unique die 1-6 and nonnegative quantities required",
                ));
            }
            let values = [row.game_turns_1_to_30, row.game_turns_31_to_107].map(|q| {
                [TruckType::Light, TruckType::Medium, TruckType::Heavy]
                    .into_iter()
                    .map(|kind| (kind, q.get(kind)))
                    .collect()
            });
            trucks.insert(row.die, values);
        }
        if trucks.len() != 6 {
            return Err(raw.err("truck_row", "all six dice rows required"));
        }
        let mut infantry = BTreeMap::new();
        let mut sums = BTreeSet::new();
        for (i, row) in body.infantry_row.into_iter().enumerate() {
            let field = format!("infantry_row[{i}]");
            let none: BTreeSet<_> = row.none_in.iter().copied().collect();
            if !(2..=12).contains(&row.dice_sum)
                || !sums.insert(row.dice_sum)
                || none.len() != row.none_in.len()
                || row
                    .points_by_game_turns
                    .keys()
                    .copied()
                    .collect::<BTreeSet<_>>()
                    != BTreeSet::from(InfantryProductionColumn::ALL)
            {
                return Err(raw.err(&field, "unique sum 2-12 and complete four columns required"));
            }
            for (column, points) in row.points_by_game_turns {
                if points < 0 || ((points == 0) != none.contains(&column)) {
                    return Err(raw.err(&field, "none marks must match exactly the zero cells"));
                }
                infantry.insert(
                    (row.dice_sum, column),
                    if points == 0 {
                        InfantryProductionResult::PrintedNone
                    } else {
                        InfantryProductionResult::Points(points)
                    },
                );
            }
        }
        if sums.len() != 11 {
            return Err(raw.err("infantry_row", "all eleven sums required"));
        }
        let mut equipment = BTreeMap::new();
        for (i, row) in body.chart_row.into_iter().enumerate() {
            let field = format!("chart_row[{i}]");
            let maximum = match (
                row.max_per_game_turn,
                row.max_per_game_turn_printed_dash,
                row.max_limit_period,
            ) {
                (None, true, None) if row.item == "Churchill" => ProductionMaximum::PrintedDash,
                (Some(points), false, period)
                    if points > 0 && points <= row.number && row.item != "Churchill" =>
                {
                    let period = match period {
                        None => ProductionPeriod::GameTurn,
                        Some(CommonwealthPeriod::Month) => ProductionPeriod::CalendarMonth,
                        Some(CommonwealthPeriod::HalfMonth) => ProductionPeriod::HalfCalendarMonth,
                    };
                    ProductionMaximum::Points { points, period }
                }
                _ => {
                    return Err(raw.err(
                        &field,
                        "exactly one positive maximum or Churchill's printed dash required",
                    ));
                }
            };
            if row.number <= 0 {
                return Err(raw.err(&field, "positive pool required"));
            }
            let entry = CommonwealthProductionEntry {
                pool_points: row.number,
                maximum,
                dates: dates(raw, &field, row.first_game_turn, row.last_game_turn)?,
            };
            if equipment.insert(row.item, entry).is_some() {
                return Err(raw.err(&field, "duplicate item"));
            }
        }
        keys_complete(
            raw,
            "chart_row.item",
            &equipment.keys().cloned().collect(),
            CW_ITEMS,
        )?;
        Ok(Self {
            trucks,
            infantry,
            equipment,
        })
    }
}
impl CommonwealthProduction {
    /// One type's independent die result in an explicitly chosen printed block.
    /// Scheduling (including the four-turn delay) and Alexandria's fraction cap are separate.
    /// Cases: land:20.78
    /// Interpretation: interp:airlog-0004
    pub fn trucks(&self, column: TruckProductionColumn, kind: TruckType, die: i32) -> Option<i32> {
        let index = match column {
            TruckProductionColumn::GameTurns1To30 => 0,
            TruckProductionColumn::GameTurns31To107 => 1,
        };
        self.trucks.get(&die).map(|blocks| blocks[index][&kind])
    }
    /// The caller selects a printed column; this does not decide arrival versus planning timing.
    /// Cases: land:20.78
    /// Interpretation: interp:airlog-0004
    pub fn infantry(
        &self,
        column: InfantryProductionColumn,
        dice_sum: i32,
    ) -> Option<InfantryProductionResult> {
        self.infantry.get(&(dice_sum, column)).copied()
    }
    /// Native chart designation and printed limits; no production entitlement is allocated.
    /// Cases: land:20.78
    pub fn equipment(&self, chart_item: &str) -> Option<&CommonwealthProductionEntry> {
        self.equipment.get(chart_item)
    }
}
