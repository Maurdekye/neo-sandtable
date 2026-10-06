//! Private sparse chart validation; every real dice reading must have exactly one result.
//! Printed ranges can end on a non-reading number (assault 13-18); only real readings count.
use crate::ranges::{IntRange, all_readings};
use crate::{RawTable, TableError};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Cell {
    pub column: String,
    pub dice: IntRange,
}
#[derive(Debug, Clone)]
pub(super) struct Grid<R> {
    values: BTreeMap<(String, i32), R>,
}
impl<R: Clone> Grid<R> {
    pub fn new(
        raw: &RawTable,
        field: &str,
        columns: &[String],
        rows: impl IntoIterator<Item = (R, Vec<Cell>)>,
    ) -> Result<Self, TableError> {
        let ids: BTreeSet<_> = columns.iter().cloned().collect();
        if ids.len() != columns.len() || columns.is_empty() {
            return Err(raw.err("column.id", "unique nonempty columns required"));
        }
        let mut values = BTreeMap::new();
        for (i, (result, cells)) in rows.into_iter().enumerate() {
            let mut seen = BTreeSet::new();
            for cell in cells {
                if !ids.contains(&cell.column) || !seen.insert(cell.column.clone()) {
                    return Err(raw.err(
                        format!("{field}[{i}].cells.column"),
                        "unknown or repeated column",
                    ));
                }
                let span = cell.dice;
                if span.lo < 11 || span.hi > 66 || !all_readings().any(|n| span.contains(n)) {
                    return Err(raw.err(
                        format!("{field}[{i}].cells.dice"),
                        "range must contain a real dice reading within 11-66",
                    ));
                }
                for n in all_readings().filter(|n| span.contains(*n)) {
                    if values
                        .insert((cell.column.clone(), n), result.clone())
                        .is_some()
                    {
                        return Err(raw.err(
                            format!("{field}[{i}].cells.dice"),
                            format!("column {} repeats reading {n}", cell.column),
                        ));
                    }
                }
            }
        }
        for id in columns {
            for n in all_readings() {
                if !values.contains_key(&(id.clone(), n)) {
                    return Err(raw.err(field, format!("column {id} is missing reading {n}")));
                }
            }
        }
        Ok(Self { values })
    }
    pub fn get(&self, column: &str, reading: i32) -> &R {
        &self.values[&(column.to_owned(), reading)]
    }
}
pub(super) fn span_cell(column: &str, lo: i32, hi: i32) -> Cell {
    Cell {
        column: column.into(),
        dice: IntRange::new(lo, hi),
    }
}
