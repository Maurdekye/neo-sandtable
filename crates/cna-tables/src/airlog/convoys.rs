//! Naval convoy tables (section 56) and the abstract truck loss chart (section 58).

use std::collections::BTreeMap;

use cna_core::dice::Die;
use cna_core::quantity::Tons;
use serde::{Deserialize, Serialize};

use crate::calendar::Month;
use crate::{Bound, RawTable, TableError};

// ---------------------------------------------------------------------------------------------
// 56.4 Axis Naval Convoy Level and 56.5 Axis Naval Convoy Capacity
// ---------------------------------------------------------------------------------------------

/// An Axis convoy level, A-G (`airlog:56.4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub enum ConvoyLevel {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
}

#[derive(Debug, Clone, Deserialize)]
struct LevelRowRaw {
    year: i32,
    levels: BTreeMap<Month, ConvoyLevel>,
    dash_months: Vec<Month>,
}

#[derive(Debug, Clone, Deserialize)]
struct LevelRaw {
    row: Vec<LevelRowRaw>,
}

/// The Axis Naval Convoy Level chart (`airlog:56.4`).
#[derive(Debug, Clone)]
pub struct ConvoyLevelTable {
    /// Level per `(year, month)`; `None` where the chart prints a dash.
    cells: BTreeMap<(i32, Month), Option<ConvoyLevel>>,
}

impl Bound for ConvoyLevelTable {
    const ID: &'static str = "airlog.56.4.axis_naval_convoy_level";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: LevelRaw = raw.deserialize()?;
        let mut cells = BTreeMap::new();
        for (i, row) in r.row.iter().enumerate() {
            for m in Month::ALL {
                let level = row.levels.get(&m).copied();
                let dash = row.dash_months.contains(&m);
                if level.is_some() == dash {
                    return Err(raw.err(
                        format!("row[{i}] ({} {m:?})", row.year),
                        "every month needs exactly one of a level or a dash",
                    ));
                }
                cells.insert((row.year, m), level);
            }
        }
        Ok(Self { cells })
    }
}

impl ConvoyLevelTable {
    /// The convoy level for a month, or `None` where the chart prints a dash or does not
    /// cover the year. `airlog:56.4`.
    pub fn level(&self, year: i32, month: Month) -> Option<ConvoyLevel> {
        self.cells.get(&(year, month)).copied().flatten()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct CapacityRowRaw {
    convoy_level: ConvoyLevel,
    fixed_tons: i32,
    variable_tons_per_pip: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct RoundingRaw {
    round_up_to_tons: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct CapacityRaw {
    row: Vec<CapacityRowRaw>,
    rounding: RoundingRaw,
}

/// The Axis Naval Convoy Capacity chart (`airlog:56.5`).
#[derive(Debug, Clone)]
pub struct ConvoyCapacityTable {
    rows: Vec<CapacityRowRaw>,
    round_up_to: i32,
}

impl Bound for ConvoyCapacityTable {
    const ID: &'static str = "airlog.56.5.axis_naval_convoy_capacity";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: CapacityRaw = raw.deserialize()?;
        if r.rounding.round_up_to_tons <= 0 {
            return Err(raw.err("rounding.round_up_to_tons", "must be positive"));
        }
        for l in [
            ConvoyLevel::A,
            ConvoyLevel::B,
            ConvoyLevel::C,
            ConvoyLevel::D,
            ConvoyLevel::E,
            ConvoyLevel::F,
            ConvoyLevel::G,
        ] {
            if r.row.iter().filter(|x| x.convoy_level == l).count() != 1 {
                return Err(raw.err("row", format!("level {l:?} must appear exactly once")));
            }
        }
        Ok(Self {
            rows: r.row,
            round_up_to: r.rounding.round_up_to_tons,
        })
    }
}

impl ConvoyCapacityTable {
    /// Capacity of an Axis convoy: fixed tonnage plus the variable tonnage for each pip of one
    /// die, rounded up to a multiple of the chart rounding (1,000 tons). `airlog:56.5`.
    pub fn capacity(&self, level: ConvoyLevel, die: Die) -> Tons {
        let row = self
            .rows
            .iter()
            .find(|x| x.convoy_level == level)
            .expect("validated: every level present");
        let raw = row.fixed_tons + row.variable_tons_per_pip * i32::from(die.value());
        Tons::new((raw + self.round_up_to - 1).div_euclid(self.round_up_to) * self.round_up_to)
    }
}

// ---------------------------------------------------------------------------------------------
// 56.18 Axis Naval Convoy Air Distance
// ---------------------------------------------------------------------------------------------

/// Where aircraft fly from when they attack an Axis convoy lane (`airlog:56.18`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AirOrigin {
    Sicily,
    Italy,
    Crete,
    Malta,
    /// Rally Point.
    Benghazi,
    /// Rally Point.
    Derna,
    /// Rally Point.
    Tobruk,
}

#[derive(Debug, Clone, Deserialize)]
struct LaneRowRaw {
    lane: u8,
    route: String,
    #[serde(default)]
    sicily: Option<i32>,
    #[serde(default)]
    italy: Option<i32>,
    #[serde(default)]
    crete: Option<i32>,
    #[serde(default)]
    malta: Option<i32>,
    #[serde(default)]
    benghazi: Option<i32>,
    #[serde(default)]
    derna: Option<i32>,
    #[serde(default)]
    tobruk: Option<i32>,
    #[serde(default)]
    dash_columns: Vec<AirOrigin>,
}

#[derive(Debug, Clone, Deserialize)]
struct LaneRaw {
    row: Vec<LaneRowRaw>,
}

/// The Axis Naval Convoy Air Distance chart (`airlog:56.18`).
#[derive(Debug, Clone)]
pub struct ConvoyAirDistance {
    rows: Vec<LaneRowRaw>,
}

impl Bound for ConvoyAirDistance {
    const ID: &'static str = "airlog.56.18.axis_naval_convoy_air_distance";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: LaneRaw = raw.deserialize()?;
        for lane in 1..=6u8 {
            if r.row.iter().filter(|x| x.lane == lane).count() != 1 {
                return Err(raw.err("row", format!("lane {lane} must appear exactly once")));
            }
        }
        for (i, row) in r.row.iter().enumerate() {
            for origin in [
                AirOrigin::Sicily,
                AirOrigin::Italy,
                AirOrigin::Crete,
                AirOrigin::Malta,
                AirOrigin::Benghazi,
                AirOrigin::Derna,
                AirOrigin::Tobruk,
            ] {
                let present = row.distance_raw(origin).is_some();
                let dash = row.dash_columns.contains(&origin);
                if present == dash {
                    return Err(raw.err(
                        format!("row[{i}] (lane {}, {origin:?})", row.lane),
                        "needs exactly one of a distance or a dash",
                    ));
                }
            }
        }
        Ok(Self { rows: r.row })
    }
}

impl LaneRowRaw {
    fn distance_raw(&self, origin: AirOrigin) -> Option<i32> {
        match origin {
            AirOrigin::Sicily => self.sicily,
            AirOrigin::Italy => self.italy,
            AirOrigin::Crete => self.crete,
            AirOrigin::Malta => self.malta,
            AirOrigin::Benghazi => self.benghazi,
            AirOrigin::Derna => self.derna,
            AirOrigin::Tobruk => self.tobruk,
        }
    }
}

impl ConvoyAirDistance {
    /// Distance in hexes from `origin` to shipping lane `lane` (1-6); `None` where the chart
    /// prints a dash (lane 1 from Crete) or the lane does not exist. `airlog:56.18`.
    pub fn distance_hexes(&self, lane: u8, origin: AirOrigin) -> Option<i32> {
        self.rows
            .iter()
            .find(|x| x.lane == lane)?
            .distance_raw(origin)
    }

    /// The route name of a lane, as the data labels it.
    pub fn route(&self, lane: u8) -> Option<&str> {
        self.rows
            .iter()
            .find(|x| x.lane == lane)
            .map(|x| x.route.as_str())
    }
}

// ---------------------------------------------------------------------------------------------
// 56.26 Road Distance
// ---------------------------------------------------------------------------------------------

/// The coastal-road places of the Road Distance Table (`airlog:56.26`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoadPlace {
    Nofilia,
    MarbleArch,
    ElAgheila,
    Benghazi,
    Derna,
    Tobruk,
    Bardia,
    MersaMatruh,
    Alexandria,
    Cairo,
}

#[derive(Debug, Clone, Deserialize)]
struct RoadRowRaw {
    place: RoadPlace,
    to: BTreeMap<RoadPlace, i32>,
}

#[derive(Debug, Clone, Deserialize)]
struct RoadRaw {
    places: Vec<RoadPlace>,
    row: Vec<RoadRowRaw>,
}

/// The Road Distance Table (`airlog:56.26`); the chart prints no unit.
#[derive(Debug, Clone)]
pub struct RoadDistance {
    pairs: BTreeMap<(RoadPlace, RoadPlace), i32>,
}

impl Bound for RoadDistance {
    const ID: &'static str = "airlog.56.26.road_distance";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: RoadRaw = raw.deserialize()?;
        if r.row.len() + 1 != r.places.len() {
            return Err(raw.err("row", "needs one row per place after the first"));
        }
        let mut pairs = BTreeMap::new();
        for (i, row) in r.row.iter().enumerate() {
            if row.place != r.places[i + 1] {
                return Err(raw.err(
                    format!("row[{i}].place"),
                    "rows must follow the places order",
                ));
            }
            let earlier = &r.places[..=i];
            if row.to.len() != earlier.len() || earlier.iter().any(|p| !row.to.contains_key(p)) {
                return Err(raw.err(
                    format!("row[{i}].to"),
                    "must give the distance to every earlier place, and only those",
                ));
            }
            for (&other, &d) in &row.to {
                if d <= 0 {
                    return Err(raw.err(format!("row[{i}].to"), "distances must be positive"));
                }
                pairs.insert((row.place, other), d);
                pairs.insert((other, row.place), d);
            }
        }
        Ok(Self { pairs })
    }
}

impl RoadDistance {
    /// Road distance between two places (symmetric; zero from a place to itself).
    /// `airlog:56.26`.
    pub fn distance(&self, a: RoadPlace, b: RoadPlace) -> i32 {
        if a == b { 0 } else { self.pairs[&(a, b)] }
    }
}

// ---------------------------------------------------------------------------------------------
// 58.5 Abstract Truck Loss
// ---------------------------------------------------------------------------------------------

/// The monthly percentages of motorization points destroyed (`airlog:58.5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct TruckLoss {
    /// Percent of Commonwealth motorization points in North Africa destroyed.
    pub cw_percent: i32,
    /// Percent of Axis motorization points in North Africa destroyed.
    pub axis_percent: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct LossRowRaw {
    year: i32,
    months: BTreeMap<Month, TruckLoss>,
    dash_months: Vec<Month>,
}

#[derive(Debug, Clone, Deserialize)]
struct LossRaw {
    row: Vec<LossRowRaw>,
}

/// The Abstract Truck Loss Chart (`airlog:58.5`); used only with the abstract Air rules.
#[derive(Debug, Clone)]
pub struct AbstractTruckLoss {
    cells: BTreeMap<(i32, Month), Option<TruckLoss>>,
}

impl Bound for AbstractTruckLoss {
    const ID: &'static str = "airlog.58.5.abstract_truck_loss";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: LossRaw = raw.deserialize()?;
        let mut cells = BTreeMap::new();
        for (i, row) in r.row.iter().enumerate() {
            for m in Month::ALL {
                let loss = row.months.get(&m).copied();
                let dash = row.dash_months.contains(&m);
                if loss.is_some() == dash {
                    return Err(raw.err(
                        format!("row[{i}] ({} {m:?})", row.year),
                        "every month needs exactly one of a loss or a dash",
                    ));
                }
                if let Some(l) = loss
                    && (!(0..=100).contains(&l.cw_percent) || !(0..=100).contains(&l.axis_percent))
                {
                    return Err(raw.err(
                        format!("row[{i}] ({} {m:?})", row.year),
                        "percentages must be 0-100",
                    ));
                }
                cells.insert((row.year, m), loss);
            }
        }
        Ok(Self { cells })
    }
}

impl AbstractTruckLoss {
    /// The loss percentages for a month, or `None` where the chart prints a dash or does not
    /// cover the year. `airlog:58.5`.
    pub fn loss(&self, year: i32, month: Month) -> Option<TruckLoss> {
        self.cells.get(&(year, month)).copied().flatten()
    }
}
