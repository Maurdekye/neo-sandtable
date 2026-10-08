//! Abstract-logistics supply quantities. Procedures select columns and arrival dates.
use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum TonnageAvailability {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
}
impl TonnageAvailability {
    pub const ALL: [Self; 7] = [
        Self::A,
        Self::B,
        Self::C,
        Self::D,
        Self::E,
        Self::F,
        Self::G,
    ];
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum CommonwealthSupplyPeriod {
    #[serde(rename = "period_I")]
    ThroughApril1941,
    #[serde(rename = "period_II")]
    May1941ThroughMay1942,
    #[serde(rename = "period_III")]
    June1942Onward,
}
impl CommonwealthSupplyPeriod {
    pub const ALL: [Self; 3] = [
        Self::ThroughApril1941,
        Self::May1941ThroughMay1942,
        Self::June1942Onward,
    ];
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, bound(deserialize = "C: Deserialize<'de> + Ord"))]
struct Row<C> {
    die: i32,
    supply_units: BTreeMap<C, i32>,
}
#[derive(Deserialize)]
#[serde(bound(deserialize = "C: Deserialize<'de> + Ord"))]
struct Body<C> {
    row: Vec<Row<C>>,
}
fn bind<C: Copy + Ord>(
    raw: &RawTable,
    rows: Vec<Row<C>>,
    columns: &[C],
) -> Result<BTreeMap<(i32, C), i32>, TableError> {
    let expected: BTreeSet<_> = columns.iter().copied().collect();
    let mut values = BTreeMap::new();
    let mut dice = BTreeSet::new();
    for (i, row) in rows.into_iter().enumerate() {
        if !(1..=6).contains(&row.die) || !dice.insert(row.die) {
            return Err(raw.err(format!("row[{i}].die"), "unique die in 1..6 required"));
        }
        if row.supply_units.keys().copied().collect::<BTreeSet<_>>() != expected {
            return Err(raw.err(
                format!("row[{i}].supply_units"),
                "every printed column is required",
            ));
        }
        for (column, n) in row.supply_units {
            if n < 0 {
                return Err(raw.err(format!("row[{i}].supply_units"), "negative supply units"));
            }
            values.insert((row.die, column), n);
        }
    }
    if dice != BTreeSet::from([1, 2, 3, 4, 5, 6]) {
        return Err(raw.err("row.die", "all six die faces are required"));
    }
    Ok(values)
}
#[derive(Debug, Clone)]
pub struct AxisSupplyAvailability {
    values: BTreeMap<(i32, TonnageAvailability), i32>,
}
impl Bound for AxisSupplyAvailability {
    const ID: &'static str = "land.32.46.axis_supply_availability";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: Body<TonnageAvailability> = raw.deserialize()?;
        Ok(Self {
            values: bind(raw, body.row, &TonnageAvailability::ALL)?,
        })
    }
}
impl AxisSupplyAvailability {
    /// Real Supply Units; the column is selected for the planning month (32.44).
    /// The procedure schedules arrival two Game-Turns later and rolls the die.
    /// Cases: land:32.44, land:32.46
    pub fn supply_units(&self, die: i32, column: TonnageAvailability) -> Option<i32> {
        self.values.get(&(die, column)).copied()
    }
}
#[derive(Debug, Clone)]
pub struct CommonwealthSupplyAvailability {
    values: BTreeMap<(i32, CommonwealthSupplyPeriod), i32>,
}
impl Bound for CommonwealthSupplyAvailability {
    const ID: &'static str = "land.32.47.cw_supply_availability";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: Body<CommonwealthSupplyPeriod> = raw.deserialize()?;
        Ok(Self {
            values: bind(raw, body.row, &CommonwealthSupplyPeriod::ALL)?,
        })
    }
}
impl CommonwealthSupplyAvailability {
    /// Real Supply Units; choose the period of arrival, not planning (32.45).
    /// Date conversion, the four-Game-Turn delay and die roll belong to the procedure.
    /// Cases: land:32.45, land:32.47
    pub fn supply_units(&self, die: i32, period: CommonwealthSupplyPeriod) -> Option<i32> {
        self.values.get(&(die, period)).copied()
    }
}
