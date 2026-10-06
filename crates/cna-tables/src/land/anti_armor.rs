//! Anti-armor damage points, including phasing and zero-column readings.
use crate::ranges::{RollCell, all_readings, check_reading_tiling};
use crate::{Bound, RawTable, TableError};
use cna_core::dice::TwoDiceReading;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, Deserialize)]
struct Row {
    dice: RollCell,
    damage_points_by_column: BTreeMap<String, i32>,
}
#[derive(Deserialize)]
struct Column {
    id: String,
    actual_points_min: i32,
    actual_points_max: Option<i32>,
    only_if_raw_points_between: Option<Vec<i32>>,
}
#[derive(Deserialize)]
struct Body {
    column: Vec<Column>,
    row: Vec<Row>,
}
#[derive(Debug, Clone)]
pub struct AntiArmorTable {
    row: Vec<Row>,
}
impl Bound for AntiArmorTable {
    const ID: &'static str = "land.14.6.anti_armor_results";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let b: Body = raw.deserialize()?;
        if b.column.len() != 2
            || !b.column.iter().any(|c| {
                c.id == "0*"
                    && c.actual_points_min == 0
                    && c.actual_points_max == Some(0)
                    && c.only_if_raw_points_between.as_deref() == Some(&[1, 4])
            })
            || !b.column.iter().any(|c| {
                c.id == "16+"
                    && c.actual_points_min == 16
                    && c.actual_points_max.is_none()
                    && c.only_if_raw_points_between.is_none()
            })
        {
            return Err(raw.err(
                "column",
                "zero raw-point condition and open 16+ column required",
            ));
        }
        check_reading_tiling(b.row.iter().filter_map(|r| r.dice.0))
            .map_err(|e| raw.err("row.dice", e))?;
        if b.row.len() != 18 {
            return Err(raw.err("row", "eighteen two-reading rows required"));
        }
        let keys: BTreeSet<_> = (1..=15)
            .map(|n| n.to_string())
            .chain(["0*".into(), "16+".into()])
            .collect();
        let readings: Vec<_> = all_readings().collect();
        for (i, r) in b.row.iter().enumerate() {
            if r.dice
                .0
                .is_none_or(|s| s.lo != readings[i * 2] || s.hi != readings[i * 2 + 1])
            {
                return Err(raw.err(
                    format!("row[{i}].dice"),
                    "rows must ascend in consecutive pairs",
                ));
            }
            for (key, value) in &r.damage_points_by_column {
                if !keys.contains(key) || *value <= 0 {
                    return Err(raw.err(
                        format!("row[{i}].damage_points_by_column"),
                        "known column and positive damage required; omit printed dashes",
                    ));
                }
            }
            let value = |n: i32| {
                r.damage_points_by_column
                    .get(&if n == 0 {
                        "0*".into()
                    } else if n == 16 {
                        "16+".into()
                    } else {
                        n.to_string()
                    })
                    .copied()
                    .unwrap_or(0)
            };
            if (0..16).any(|n| value(n) > value(n + 1)) {
                return Err(raw.err(
                    format!("row[{i}].damage_points_by_column"),
                    "damage cannot decrease across columns",
                ));
            }
            if i > 0
                && keys.iter().any(|k| {
                    b.row[i - 1]
                        .damage_points_by_column
                        .get(k)
                        .copied()
                        .unwrap_or(0)
                        > r.damage_points_by_column.get(k).copied().unwrap_or(0)
                })
            {
                return Err(raw.err(
                    format!("row[{i}].damage_points_by_column"),
                    "damage cannot decrease down a column",
                ));
            }
        }
        Ok(Self { row: b.row })
    }
}
impl AntiArmorTable {
    /// Positive strength uses its chart column before shifts; strength above 16 uses the last.
    /// An unshifted zero requires 1-4 raw points. Terrain may shift positive strength to zero.
    /// The phasing side moves one two-reading row upward, stopping at the first row.
    /// Cases: land:14.6, land:14.31, land:14.32, land:14.35, land:14.41, land:11.33
    /// Interpretations: interp:land-0022
    pub fn damage(
        &self,
        actual_points: i32,
        raw_points: i32,
        shift_left: i32,
        phasing: bool,
        reading: TwoDiceReading,
    ) -> Option<i32> {
        if actual_points < 0 || raw_points < 0 || shift_left < 0 {
            return None;
        }
        if raw_points == 0 {
            return Some(0);
        }
        if actual_points == 0 && !(1..=4).contains(&raw_points) {
            return None;
        }
        let column = actual_points.min(16).saturating_sub(shift_left).max(0);
        let key = if column == 0 {
            "0*".into()
        } else if column == 16 {
            "16+".into()
        } else {
            column.to_string()
        };
        let index = self
            .row
            .iter()
            .position(|r| r.dice.contains(reading))
            .expect("validated dice");
        let index = index.saturating_sub(usize::from(phasing));
        Some(
            self.row[index]
                .damage_points_by_column
                .get(&key)
                .copied()
                .unwrap_or(0),
        )
    }
}
