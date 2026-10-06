//! Artillery barrage losses and pinning by target class.
use super::grid::{Cell, Grid};
use crate::ranges::{Band, band_index, check_bands};
use crate::{Bound, RawTable, TableError};
use cna_core::dice::TwoDiceReading;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BarrageTarget {
    Infantry,
    Armor,
    Gun,
    Truck,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Code {
    NoEffect,
    Pinned,
    #[serde(rename = "lose_1")]
    Lose1,
    #[serde(rename = "lose_2")]
    Lose2,
}
#[derive(Deserialize)]
struct Column {
    id: String,
    barrage_points_min: i32,
    barrage_points_max: Option<i32>,
}
#[derive(Deserialize)]
struct Row {
    target_class: BarrageTarget,
    result: Code,
    cells: Vec<Cell>,
}
#[derive(Debug, Clone, Deserialize)]
struct Effect {
    result: Code,
    toe_points_lost: i32,
    pins_unit: Option<bool>,
    #[serde(default)]
    pins_unit_classes: Vec<BarrageTarget>,
    extra_for_infantry_in_trucks: Option<String>,
}
#[derive(Deserialize)]
struct Body {
    column: Vec<Column>,
    row: Vec<Row>,
    result_effect: Vec<Effect>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarrageResult {
    pub toe_points_lost: i32,
    pub pinned: bool,
    pub transport_truck_points_lost: i32,
}
#[derive(Debug, Clone)]
pub struct BarrageTable {
    ids: Vec<String>,
    bands: Vec<Band>,
    grids: BTreeMap<BarrageTarget, Grid<Code>>,
    effects: BTreeMap<Code, Effect>,
}
impl Bound for BarrageTable {
    const ID: &'static str = "land.12.6.artillery_barrage";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let b: Body = raw.deserialize()?;
        let ids: Vec<_> = b.column.iter().map(|c| c.id.clone()).collect();
        let bands: Vec<_> = b
            .column
            .iter()
            .map(|c| Band {
                min: c.barrage_points_min,
                max: c.barrage_points_max,
            })
            .collect();
        check_bands(&bands, 1).map_err(|e| raw.err("column.barrage_points", e))?;
        let mut grids = BTreeMap::new();
        for target in [
            BarrageTarget::Infantry,
            BarrageTarget::Armor,
            BarrageTarget::Gun,
            BarrageTarget::Truck,
        ] {
            let rows: Vec<_> = b.row.iter().filter(|r| r.target_class == target).collect();
            let mut results = BTreeSet::new();
            for r in &rows {
                if !results.insert(r.result)
                    || (matches!(target, BarrageTarget::Gun | BarrageTarget::Truck)
                        && r.result == Code::Pinned)
                {
                    return Err(raw.err("row.result", "duplicate or inapplicable target result"));
                }
            }
            grids.insert(
                target,
                Grid::new(
                    raw,
                    "row",
                    &ids,
                    rows.into_iter().map(|r| (r.result, r.cells.clone())),
                )?,
            );
        }
        let mut effects = BTreeMap::new();
        for (i, e) in b.result_effect.into_iter().enumerate() {
            let lost = match e.result {
                Code::Pinned => 0,
                Code::Lose1 => 1,
                Code::Lose2 => 2,
                Code::NoEffect => {
                    return Err(raw.err(
                        format!("result_effect[{i}].result"),
                        "no-effect needs no effect record",
                    ));
                }
            };
            let classes: BTreeSet<_> = e.pins_unit_classes.iter().copied().collect();
            let valid = if e.result == Code::Pinned {
                e.pins_unit == Some(true)
                    && classes.is_empty()
                    && e.extra_for_infantry_in_trucks.is_none()
            } else {
                e.pins_unit.is_none_or(|p| !p)
                    && classes == BTreeSet::from([BarrageTarget::Infantry, BarrageTarget::Armor])
                    && classes.len() == e.pins_unit_classes.len()
                    && e.extra_for_infantry_in_trucks.as_deref()
                        == Some("eliminate one truck point")
            };
            if !valid || e.toe_points_lost != lost {
                return Err(raw.err(
                    format!("result_effect[{i}]"),
                    "loss count, pin classes or carried-infantry effect disagree",
                ));
            }
            if effects.insert(e.result, e).is_some() {
                return Err(raw.err("result_effect.result", "duplicate effect"));
            }
        }
        if effects.len() != 3 {
            return Err(raw.err("result_effect", "pin and both loss effects required"));
        }
        Ok(Self {
            ids,
            bands,
            grids,
            effects,
        })
    }
}
impl BarrageTable {
    /// Truck targets ignore terrain shifts. A shift left of the first column gives no effect.
    /// Actual points and shifts must be nonnegative; zero points cannot inflict barrage losses.
    /// Cases: land:12.6, land:12.33, land:12.34, land:12.41, land:12.43, land:12.44, land:12.45, land:12.46
    /// Interpretations: interp:land-0022
    pub fn result(
        &self,
        actual_points: i32,
        shift_left: i32,
        target: BarrageTarget,
        reading: TwoDiceReading,
        infantry_in_trucks: bool,
    ) -> Option<BarrageResult> {
        if actual_points < 0 || shift_left < 0 {
            return None;
        }
        let Some(index) = band_index(&self.bands, actual_points) else {
            return Some(BarrageResult::default());
        };
        let shift = if target == BarrageTarget::Truck {
            0
        } else {
            usize::try_from(shift_left).ok()?
        };
        let Some(index) = index.checked_sub(shift) else {
            return Some(BarrageResult::default());
        };
        let code = *self.grids[&target].get(&self.ids[index], i32::from(reading.value()));
        if code == Code::NoEffect {
            return Some(BarrageResult::default());
        }
        let e = &self.effects[&code];
        Some(BarrageResult {
            toe_points_lost: e.toe_points_lost,
            pinned: e.pins_unit.unwrap_or(false) || e.pins_unit_classes.contains(&target),
            transport_truck_points_lost: i32::from(
                target == BarrageTarget::Infantry
                    && infantry_in_trucks
                    && e.extra_for_infantry_in_trucks.is_some(),
            ),
        })
    }
}
