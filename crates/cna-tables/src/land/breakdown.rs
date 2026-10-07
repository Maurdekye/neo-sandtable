//! Accumulated breakdown points choose a column; BAR and weather shift columns.
use super::grid::{Cell, Grid};
use crate::{Bound, RawTable, TableError};
use cna_core::dice::TwoDiceReading;
use serde::Deserialize;
#[derive(Deserialize)]
struct Column {
    id: String,
    breakdown_points_min: i32,
    breakdown_points_max: Option<i32>,
}
#[derive(Deserialize)]
struct Row {
    percent_breakdown: i32,
    cells: Vec<Cell>,
}
#[derive(Deserialize)]
struct Body {
    column: Vec<Column>,
    row: Vec<Row>,
}
#[derive(Debug, Clone)]
pub struct BreakdownTable {
    columns: Vec<String>,
    grid: Grid<i32>,
}
impl Bound for BreakdownTable {
    const ID: &'static str = "land.21.38.breakdown";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let b: Body = raw.deserialize()?;
        let bands = [
            (0, Some(3)),
            (4, Some(10)),
            (11, Some(20)),
            (21, Some(30)),
            (31, Some(40)),
            (41, Some(50)),
            (51, Some(60)),
            (61, Some(70)),
            (71, None),
        ];
        if b.column.len() != bands.len() {
            return Err(raw.err("column", "nine breakdown bands required"));
        }
        for (i, (c, (lo, hi))) in b.column.iter().zip(bands).enumerate() {
            if c.breakdown_points_min != lo || c.breakdown_points_max != hi {
                return Err(raw.err(
                    format!("column[{i}].breakdown_points"),
                    "bands must tile 0 through the open 71+ endpoint",
                ));
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for (i, r) in b.row.iter().enumerate() {
            if ![0, 10, 25, 33, 50, 75].contains(&r.percent_breakdown)
                || !seen.insert(r.percent_breakdown)
            {
                return Err(raw.err(
                    format!("row[{i}].percent_breakdown"),
                    "unique printed percentage required",
                ));
            }
        }
        if seen.len() != 6 {
            return Err(raw.err(
                "row.percent_breakdown",
                "all six printed percentages required",
            ));
        }
        let columns = b.column.into_iter().map(|c| c.id).collect::<Vec<_>>();
        let grid = Grid::new(
            raw,
            "row",
            &columns,
            b.row.into_iter().map(|r| (r.percent_breakdown, r.cells)),
        )?;
        Ok(Self { columns, grid })
    }
}
impl BreakdownTable {
    /// Fractions round up once; lower shifts can remove the check and upper shifts clamp at 71+.
    /// Cases: land:21.31, land:21.32, land:21.33, land:21.34, land:21.38
    pub fn percent_quarters(
        &self,
        points_quarters: i32,
        column_shift: i32,
        roll: TwoDiceReading,
    ) -> Option<i32> {
        if points_quarters < 0 {
            return None;
        }
        let n = (i64::from(points_quarters) + 3) / 4;
        let base = match n {
            0..=3 => 0,
            4..=10 => 1,
            11..=20 => 2,
            21..=30 => 3,
            31..=40 => 4,
            41..=50 => 5,
            51..=60 => 6,
            61..=70 => 7,
            _ => 8,
        };
        let index = (i64::from(base) + i64::from(column_shift)).min(8);
        if index < 1 {
            return Some(0);
        }
        Some(
            *self
                .grid
                .get(&self.columns[index as usize], i32::from(roll.value())),
        )
    }
    /// The 33 cell is a one-third result; a lone point ignores only a ten-percent loss.
    /// Cases: land:21.34, land:21.35
    pub fn broken_points(&self, points: i32, percent: i32) -> Option<i32> {
        if points < 0 || ![0, 10, 25, 33, 50, 75].contains(&percent) {
            return None;
        }
        if points == 1 && percent == 10 {
            return Some(0);
        }
        let (num, den) = if percent == 33 {
            (1, 3)
        } else {
            (percent, 100)
        };
        i32::try_from((i64::from(points) * i64::from(num) + i64::from(den) - 1) / i64::from(den))
            .ok()
    }
}
