//! Morale modifiers, surrender, integer endpoints and fractional cohesion rows.
use crate::ranges::{RollCell, all_readings};
use crate::{Bound, RawTable, TableError};
use cna_core::dice::TwoDiceReading;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoraleModifier {
    Change(i32),
    Surrender,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdjustedMorale {
    Rating(i32),
    Surrender,
}
#[derive(Deserialize)]
struct Cell {
    column: String,
    dice: RollCell,
}
#[derive(Deserialize)]
struct Row {
    cohesion_level_min: Option<i32>,
    cohesion_level_max: Option<i32>,
    cells: Vec<Cell>,
}
#[derive(Deserialize)]
struct Gap {
    cohesion_level: i32,
    readings: Vec<i32>,
    interp: String,
}
#[derive(Deserialize)]
struct Body {
    row: Vec<Row>,
    gap: Vec<Gap>,
}
#[derive(Debug, Clone)]
pub struct MoraleTable {
    values: std::collections::BTreeMap<(i32, i32), MoraleModifier>,
}
fn modifier(name: &str) -> Option<MoraleModifier> {
    Some(match name {
        "no_change" => MoraleModifier::Change(0),
        "surrender" => MoraleModifier::Surrender,
        "plus_1" => MoraleModifier::Change(1),
        "plus_2" => MoraleModifier::Change(2),
        "plus_3" => MoraleModifier::Change(3),
        "plus_4" => MoraleModifier::Change(4),
        "minus_1" => MoraleModifier::Change(-1),
        "minus_2" => MoraleModifier::Change(-2),
        "minus_3" => MoraleModifier::Change(-3),
        "minus_4" => MoraleModifier::Change(-4),
        _ => return None,
    })
}
impl Bound for MoraleTable {
    const ID: &'static str = "land.17.4.morale_modifier";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let b: Body = raw.deserialize()?;
        if b.gap.len() != 1
            || b.gap[0].cohesion_level != -4
            || b.gap[0].readings != [56]
            || b.gap[0].interp != "land-0012"
        {
            return Err(raw.err(
                "gap",
                "only the printed -4/56 gap under land-0012 is supported",
            ));
        }
        if b.row.len() != 26 {
            return Err(raw.err("row", "one row per level from +8 to -17 required"));
        }
        let mut values = std::collections::BTreeMap::new();
        for (i, r) in b.row.into_iter().enumerate() {
            let level = 8 - i32::try_from(i).expect("26 rows");
            let expected_min = (level != -17).then_some(level);
            let expected_max = (level != 8).then_some(level);
            if r.cohesion_level_min != expected_min || r.cohesion_level_max != expected_max {
                return Err(raw.err(
                    format!("row[{i}].cohesion_level"),
                    "rows must descend by one; only endpoints open",
                ));
            }
            let mut columns = BTreeSet::new();
            for cell in r.cells {
                let result = modifier(&cell.column).ok_or_else(|| {
                    raw.err(format!("row[{i}].cells.column"), "unknown morale result")
                })?;
                if !columns.insert(cell.column) {
                    return Err(
                        raw.err(format!("row[{i}].cells.column"), "duplicate morale result")
                    );
                }
                let span = cell.dice.0.ok_or_else(|| {
                    raw.err(format!("row[{i}].cells.dice"), "omit printed dashes")
                })?;
                for reading in all_readings().filter(|n| span.contains(*n)) {
                    if values.insert((level, reading), result).is_some() {
                        return Err(raw.err(
                            format!("row[{i}].cells.dice"),
                            format!("reading {reading} overlaps"),
                        ));
                    }
                }
            }
        }
        if values
            .insert((-4, 56), MoraleModifier::Change(-2))
            .is_some()
        {
            return Err(raw.err("gap", "printed gap must remain unfilled in source data"));
        }
        for level in -17..=8 {
            for reading in all_readings() {
                if !values.contains_key(&(level, reading)) {
                    return Err(raw.err(
                        "row.cells.dice",
                        format!("cohesion {level} misses reading {reading}"),
                    ));
                }
            }
        }
        Ok(Self { values })
    }
}
impl MoraleTable {
    /// Cases: land:17.4, land:17.22, land:17.24
    /// Interpretations: interp:land-0012, interp:land-0022
    pub fn modifier(&self, cohesion_level: i32, reading: TwoDiceReading) -> MoraleModifier {
        self.values[&(cohesion_level.clamp(-17, 8), i32::from(reading.value()))]
    }
    /// Select the row below a fractional signed cohesion value; accounting retains its quarters.
    /// Cases: land:17.4, land:17.22, land:17.24
    /// Interpretations: interp:land-0020, interp:land-0021
    pub fn modifier_quarters(
        &self,
        cohesion_quarters: i32,
        reading: TwoDiceReading,
    ) -> MoraleModifier {
        self.modifier(cohesion_quarters.div_euclid(4), reading)
    }
    /// Standard adjusted rating, before any Rommel bonus or surrender exception.
    /// Cases: land:17.22, land:17.23, land:17.25
    pub fn adjusted(
        &self,
        basic_morale: i32,
        cohesion_level: i32,
        reading: TwoDiceReading,
    ) -> AdjustedMorale {
        match self.modifier(cohesion_level, reading) {
            MoraleModifier::Surrender => AdjustedMorale::Surrender,
            MoraleModifier::Change(n) => {
                AdjustedMorale::Rating((i64::from(basic_morale) + i64::from(n)).clamp(-3, 3) as i32)
            }
        }
    }
    /// Standard adjusted morale from exact cohesion quarters, before the Rommel bonus.
    /// Cases: land:17.22, land:17.23, land:17.25
    /// Interpretations: interp:land-0020, interp:land-0021
    pub fn adjusted_quarters(
        &self,
        basic_morale: i32,
        cohesion_quarters: i32,
        reading: TwoDiceReading,
    ) -> AdjustedMorale {
        self.adjusted(basic_morale, cohesion_quarters.div_euclid(4), reading)
    }
}
