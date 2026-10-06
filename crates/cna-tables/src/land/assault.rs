//! One dice pair supplies both sequential losses and summed special results.
use super::grid::{Cell, Grid, span_cell};
use crate::{Bound, RawTable, TableError};
use cna_core::dice::TwoDiceReading;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CombatSide {
    Attacker,
    Defender,
}
#[derive(Debug, Clone, Deserialize)]
struct Column {
    id: String,
    differential_min: Option<i32>,
    differential_max: Option<i32>,
    #[serde(default)]
    overrun: bool,
}
impl Column {
    fn contains(&self, n: i32) -> bool {
        self.differential_min.is_none_or(|v| n >= v) && self.differential_max.is_none_or(|v| n <= v)
    }
}
#[derive(Deserialize)]
struct LossRow {
    side: CombatSide,
    loss_percent: i32,
    cells: Vec<Cell>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Special {
    Captured,
    Engaged,
    #[serde(rename = "retreat_1_hex")]
    Retreat1,
    #[serde(rename = "retreat_2_hexes")]
    Retreat2,
    #[serde(rename = "retreat_3_hexes")]
    Retreat3,
}
#[derive(Deserialize)]
struct SumCell {
    column: String,
    sums: Vec<u8>,
}
#[derive(Deserialize)]
struct SumRow {
    side: CombatSide,
    kind: Special,
    cells: Vec<SumCell>,
}
#[derive(Deserialize)]
struct Gap {
    side: CombatSide,
    column: String,
    readings: Vec<i32>,
    interp: String,
}
#[derive(Deserialize)]
struct Body {
    column: Vec<Column>,
    loss_row: Vec<LossRow>,
    sum_result: Vec<SumRow>,
    gap: Vec<Gap>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssaultResult {
    pub loss_percent: i32,
    pub captured: bool,
    pub engaged: bool,
    pub retreat_hexes: u8,
    pub overrun: bool,
}
#[derive(Debug, Clone)]
pub struct CloseAssaultTable {
    column: Vec<Column>,
    losses: BTreeMap<CombatSide, Grid<i32>>,
    special: BTreeMap<(CombatSide, String, u8), Vec<Special>>,
}
impl Bound for CloseAssaultTable {
    const ID: &'static str = "land.15.79.close_assault_results";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let b: Body = raw.deserialize()?;
        if b.column.len() != 18 {
            return Err(raw.err("column", "eighteen differential columns required"));
        }
        let mut next = i64::from(i32::MIN);
        for (i, c) in b.column.iter().enumerate() {
            let lo = i64::from(c.differential_min.unwrap_or(i32::MIN));
            let hi = i64::from(c.differential_max.unwrap_or(i32::MAX));
            if lo != next
                || hi < lo
                || (c.differential_min.is_none() && i != 0)
                || (c.differential_max.is_none() && i + 1 != b.column.len())
                || c.overrun != (lo >= 11)
            {
                return Err(raw.err(
                    format!("column[{i}]"),
                    "ordered contiguous differential bands and overrun marking required",
                ));
            }
            next = hi + 1;
        }
        if next != i64::from(i32::MAX) + 1 {
            return Err(raw.err("column.differential", "last band must remain open"));
        }
        let ids: Vec<_> = b.column.iter().map(|c| c.id.clone()).collect();
        if b.gap.len() != 1
            || b.gap[0].side != CombatSide::Defender
            || b.gap[0].column != "p2"
            || b.gap[0].readings != [34, 35, 36]
            || b.gap[0].interp != "land-0011"
        {
            return Err(raw.err(
                "gap",
                "only the printed defender +2 gap under land-0011 is supported",
            ));
        }
        if b.column
            .iter()
            .find(|c| c.id == "p2")
            .is_none_or(|c| c.differential_min != Some(2) || c.differential_max != Some(2))
        {
            return Err(raw.err("column.p2", "gap column must be exactly differential +2"));
        }
        let mut seen = BTreeSet::new();
        for (i, row) in b.loss_row.iter().enumerate() {
            if !(0..=100).contains(&row.loss_percent) || !seen.insert((row.side, row.loss_percent))
            {
                return Err(raw.err(
                    format!("loss_row[{i}].loss_percent"),
                    "unique percentages in 0-100 required for each side",
                ));
            }
        }
        let mut losses = BTreeMap::new();
        for side in [CombatSide::Attacker, CombatSide::Defender] {
            let mut rows: Vec<_> = b
                .loss_row
                .iter()
                .filter(|r| r.side == side)
                .map(|r| (r.loss_percent, r.cells.clone()))
                .collect();
            if side == CombatSide::Defender {
                rows.push((10, vec![span_cell("p2", 34, 36)]));
            }
            losses.insert(side, Grid::new(raw, "loss_row", &ids, rows)?);
        }
        let mut special: BTreeMap<_, Vec<_>> = BTreeMap::new();
        let mut kinds = BTreeSet::new();
        let mut retreats = BTreeSet::new();
        for (i, row) in b.sum_result.into_iter().enumerate() {
            let valid = match row.kind {
                Special::Captured => true,
                Special::Engaged => row.side == CombatSide::Attacker,
                _ => row.side == CombatSide::Defender,
            };
            if !valid || !kinds.insert((row.side, row.kind)) {
                return Err(raw.err(
                    format!("sum_result[{i}]"),
                    "unique special-result kind on its proper side required",
                ));
            }
            let mut columns = BTreeSet::new();
            for cell in row.cells {
                if !ids.contains(&cell.column)
                    || !columns.insert(cell.column.clone())
                    || cell.sums.is_empty()
                {
                    return Err(raw.err(
                        format!("sum_result[{i}].cells.column"),
                        "known unique columns with nonempty sums required",
                    ));
                }
                let mut sums = BTreeSet::new();
                for sum in cell.sums {
                    if !(2..=12).contains(&sum) || !sums.insert(sum) {
                        return Err(raw.err(
                            format!("sum_result[{i}].cells.sums"),
                            "unique two-dice sums in 2-12 required",
                        ));
                    }
                    if matches!(
                        row.kind,
                        Special::Retreat1 | Special::Retreat2 | Special::Retreat3
                    ) && !retreats.insert((cell.column.clone(), sum))
                    {
                        return Err(raw.err(
                            format!("sum_result[{i}].cells.sums"),
                            "retreat distances cannot overlap",
                        ));
                    }
                    special
                        .entry((row.side, cell.column.clone(), sum))
                        .or_default()
                        .push(row.kind);
                }
            }
        }
        if kinds.len() != 6 {
            return Err(raw.err("sum_result","capture for both sides, attacker engagement and three defender retreat distances required"));
        }
        Ok(Self {
            column: b.column,
            losses,
            special,
        })
    }
}
impl CloseAssaultTable {
    /// Resolve chart flags only: the combat procedure cancels engagement if defenders retreat,
    /// converts percentages into casualties and applies the overrun rounding rule.
    /// Cases: land:15.79, land:15.73, land:15.77
    /// Interpretations: interp:land-0011, interp:land-0022
    pub fn resolve(
        &self,
        side: CombatSide,
        differential: i32,
        reading: TwoDiceReading,
    ) -> AssaultResult {
        self.resolve_shifted(side, differential, 0, reading)
    }
    /// Apply signed COLUMN shifts to the chart band, not arithmetic changes to the differential.
    /// Positive shifts favor the attacker; endpoints stop at the first/last column.
    /// Cases: land:15.52, land:15.53, land:15.79, land:15.77
    /// Interpretations: interp:land-0011, interp:land-0022
    pub fn resolve_shifted(
        &self,
        side: CombatSide,
        differential: i32,
        column_shift: i32,
        reading: TwoDiceReading,
    ) -> AssaultResult {
        let base = self
            .column
            .iter()
            .position(|c| c.contains(differential))
            .expect("validated differential");
        let index =
            (base as i64 + i64::from(column_shift)).clamp(0, self.column.len() as i64 - 1) as usize;
        let column = &self.column[index];
        let mut result = AssaultResult {
            loss_percent: *self.losses[&side].get(&column.id, i32::from(reading.value())),
            overrun: column.overrun,
            ..AssaultResult::default()
        };
        if let Some(flags) = self.special.get(&(side, column.id.clone(), reading.sum())) {
            for flag in flags {
                match flag {
                    Special::Captured => result.captured = true,
                    Special::Engaged => result.engaged = true,
                    Special::Retreat1 => result.retreat_hexes = 1,
                    Special::Retreat2 => result.retreat_hexes = 2,
                    Special::Retreat3 => result.retreat_hexes = 3,
                }
            }
        }
        result
    }
}
