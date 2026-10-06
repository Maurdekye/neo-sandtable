//! The three two-dice-reading combat tables of the air game: Strafing (40.8), Air Bombardment
//! (41.5) and Anti-Aircraft Combat Results (46.3), plus the Flak Adjustment chart (46.4).

use cna_core::dice::TwoDiceReading;
use serde::Deserialize;

use crate::ranges::{Band, RollCell, band_index, check_bands, check_reading_tiling};
use crate::{Bound, RawTable, TableError};

fn check_column_indices(
    raw: &RawTable,
    indices: impl Iterator<Item = u8>,
) -> Result<(), TableError> {
    for (i, index) in indices.enumerate() {
        if usize::from(index) != i + 1 {
            return Err(raw.err(
                format!("column[{i}].index"),
                "indices must ascend from 1 in chart order",
            ));
        }
    }
    Ok(())
}

/// A grid of results by dice reading: each row is a result and lists, per column, the span of
/// readings that gives it. Validated so that every column tiles the 36 readings.
#[derive(Debug, Clone)]
struct Grid<R> {
    rows: Vec<(R, Vec<RollCell>)>,
    columns: usize,
}

impl<R: Clone + PartialEq> Grid<R> {
    fn new(
        raw: &RawTable,
        field: &str,
        columns: usize,
        rows: Vec<(R, Vec<RollCell>)>,
    ) -> Result<Self, TableError> {
        for (i, (_, cells)) in rows.iter().enumerate() {
            if cells.len() != columns {
                return Err(raw.err(
                    format!("{field}.row[{i}].rolls"),
                    format!(
                        "needs {columns} cells (one per column), found {}",
                        cells.len()
                    ),
                ));
            }
        }
        for col in 0..columns {
            check_reading_tiling(rows.iter().filter_map(|(_, c)| c[col].0))
                .map_err(|m| raw.err(format!("{field} column {}", col + 1), m))?;
        }
        Ok(Self { rows, columns })
    }

    fn lookup(&self, column: usize, reading: TwoDiceReading) -> &R {
        let column = column.min(self.columns - 1);
        self.rows
            .iter()
            .find(|(_, cells)| cells[column].contains(reading))
            .map(|(r, _)| r)
            .expect("validated: every column tiles the readings")
    }
}

// ---------------------------------------------------------------------------------------------
// 40.8 Strafing
// ---------------------------------------------------------------------------------------------

/// Situations that move the strafing column (`airlog:40.8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrafeShiftCase {
    TrucksInConvoy,
    SupplyDump,
    #[serde(rename = "infantry_in_fortification_level_0_or_1_hex")]
    InfantryInFortificationLevel0Or1Hex,
    #[serde(rename = "infantry_in_fortification_level_2_hex")]
    InfantryInFortificationLevel2Hex,
    #[serde(rename = "armor_in_fortification_level_2_hex")]
    ArmorInFortificationLevel2Hex,
}

#[derive(Debug, Clone, Deserialize)]
struct ColumnRaw {
    index: u8,
    tacair_points: Band,
}

#[derive(Debug, Clone, Deserialize)]
struct StrafeRowRaw {
    result: u8,
    rolls: Vec<RollCell>,
}

#[derive(Debug, Clone, Deserialize)]
struct StrafeShiftRaw {
    applies_to: StrafeShiftCase,
    columns: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct StrafeRaw {
    column: Vec<ColumnRaw>,
    row: Vec<StrafeRowRaw>,
    column_shift: Vec<StrafeShiftRaw>,
}

/// The Strafing table (`airlog:40.8`).
#[derive(Debug, Clone)]
pub struct Strafing {
    bands: Vec<Band>,
    grid: Grid<u8>,
    shifts: Vec<(StrafeShiftCase, i32)>,
}

impl Bound for Strafing {
    const ID: &'static str = "airlog.40.8.strafing";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: StrafeRaw = raw.deserialize()?;
        check_column_indices(raw, r.column.iter().map(|c| c.index))?;
        for case in [
            StrafeShiftCase::TrucksInConvoy,
            StrafeShiftCase::SupplyDump,
            StrafeShiftCase::InfantryInFortificationLevel0Or1Hex,
            StrafeShiftCase::InfantryInFortificationLevel2Hex,
            StrafeShiftCase::ArmorInFortificationLevel2Hex,
        ] {
            if r.column_shift
                .iter()
                .filter(|s| s.applies_to == case)
                .count()
                != 1
            {
                return Err(raw.err("column_shift", format!("{case:?} must appear exactly once")));
            }
        }
        let bands: Vec<Band> = r.column.iter().map(|c| c.tacair_points).collect();
        check_bands(&bands, 1).map_err(|m| raw.err("column.tacair_points", m))?;
        let grid = Grid::new(
            raw,
            "row",
            bands.len(),
            r.row.into_iter().map(|x| (x.result, x.rolls)).collect(),
        )?;
        Ok(Self {
            bands,
            grid,
            shifts: r
                .column_shift
                .into_iter()
                .map(|s| (s.applies_to, s.columns))
                .collect(),
        })
    }
}

impl Strafing {
    /// Result of a strafing attack: Truck Points, TOE points or (for a dump) the multiplier
    /// the chart footnote defines. `tacair_points` is the TacAir applied (pilot ratings not
    /// added); `column_shift` moves the column right (positive) or left (negative) and is
    /// clamped to the chart. `None` for fewer than one TacAir point. `airlog:40.8`.
    pub fn result(
        &self,
        tacair_points: i32,
        column_shift: i32,
        reading: TwoDiceReading,
    ) -> Option<u8> {
        let base = band_index(&self.bands, tacair_points)? as i32;
        let col = base
            .saturating_add(column_shift)
            .clamp(0, self.bands.len() as i32 - 1) as usize;
        Some(*self.grid.lookup(col, reading))
    }

    /// The column shift the chart gives for a situation. `airlog:40.8`.
    pub fn column_shift(&self, case: StrafeShiftCase) -> i32 {
        self.shifts
            .iter()
            .find(|(c, _)| *c == case)
            .map_or(0, |(_, s)| *s)
    }
}

// ---------------------------------------------------------------------------------------------
// 41.5 Air Bombardment
// ---------------------------------------------------------------------------------------------

/// What kind of attack points select the column (`airlog:41.5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Weapon {
    /// Torpedo points; only against Commonwealth fleet ships.
    Torpedo,
    /// Barrage points for secondary barrages, figured like the master barrage CRT.
    Barrage,
    /// Bomb points: every other bomb attack.
    Bomb,
}

/// The target blocks of the Air Bombardment table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BombTarget {
    AirfieldsAirLandingStripsPorts,
    SupplyDump,
    Fortification,
    Railroad,
    Road,
    TrucksFlakDestructionCombatUnitsCommonwealthFleet,
    AxisNavalConvoys,
}

/// The result of an air bombardment, in the unit the target block uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BombResult {
    /// Capacity or efficiency levels lost (airfields, air landing strips, ports).
    Levels(i32),
    /// Percentage of every supply type destroyed (supply dumps).
    PercentDestroyed(i32),
    /// Target-specific count: Truck Points lost, flak TOE lost, combat-unit equivalents pinned,
    /// or Commonwealth fleet Damage Points. The caller applies the selected target meaning.
    Count(i32),
    /// Percentage of the convoy cargo destroyed (Axis naval convoys).
    PercentCargoDestroyed(i32),
    NoEffect,
    FortificationReducedOneLevel,
    RailroadDestroyed,
    /// The road is reduced to a track.
    RoadReducedToTrack,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ResultRaw {
    Number(i32),
    Word(String),
}

#[derive(Debug, Clone, Deserialize)]
struct BombRowRaw {
    result: ResultRaw,
    rolls: Vec<RollCell>,
}

#[derive(Debug, Clone, Deserialize)]
struct BombBlockRaw {
    target: BombTarget,
    result_unit: String,
    row: Vec<BombRowRaw>,
}

#[derive(Debug, Clone, Deserialize)]
struct BombColumnRaw {
    index: u8,
    torpedo_points: Band,
    barrage_points: Band,
    bomb_points: Band,
}

#[derive(Debug, Clone, Deserialize)]
struct BombRaw {
    column: Vec<BombColumnRaw>,
    block: Vec<BombBlockRaw>,
}

fn bomb_result(target: BombTarget, unit: &str, value: &ResultRaw) -> Result<BombResult, String> {
    let expected = match target {
        BombTarget::AirfieldsAirLandingStripsPorts => "levels",
        BombTarget::SupplyDump => "percent_destroyed",
        BombTarget::TrucksFlakDestructionCombatUnitsCommonwealthFleet => "count",
        BombTarget::AxisNavalConvoys => "percent_cargo_destroyed",
        _ => "outcome",
    };
    if unit != expected {
        return Err(format!("expected result_unit {expected} for {target:?}"));
    }
    if let ResultRaw::Number(n) = value
        && (*n < 0 || (unit.starts_with("percent_") && *n > 100))
    {
        return Err("result must be nonnegative; percentages must not exceed 100".into());
    }
    match (unit, value) {
        ("levels", ResultRaw::Number(n)) => Ok(BombResult::Levels(*n)),
        ("percent_destroyed", ResultRaw::Number(n)) => Ok(BombResult::PercentDestroyed(*n)),
        ("count", ResultRaw::Number(n)) => Ok(BombResult::Count(*n)),
        ("percent_cargo_destroyed", ResultRaw::Number(n)) => {
            Ok(BombResult::PercentCargoDestroyed(*n))
        }
        ("outcome", ResultRaw::Word(w)) => match (target, w.as_str()) {
            (_, "no_effect") => Ok(BombResult::NoEffect),
            (BombTarget::Fortification, "reduced_one_level") => {
                Ok(BombResult::FortificationReducedOneLevel)
            }
            (BombTarget::Railroad, "destroyed") => Ok(BombResult::RailroadDestroyed),
            (BombTarget::Road, "destroyed_reduced_to_track") => Ok(BombResult::RoadReducedToTrack),
            _ => Err(format!("unknown outcome `{w}` for {target:?}")),
        },
        _ => Err(format!("result does not fit result_unit `{unit}`")),
    }
}

/// The Air Bombardment table (`airlog:41.5`).
#[derive(Debug, Clone)]
pub struct AirBombardment {
    torpedo: Vec<Band>,
    barrage: Vec<Band>,
    bomb: Vec<Band>,
    blocks: Vec<(BombTarget, Grid<BombResult>)>,
}

impl Bound for AirBombardment {
    const ID: &'static str = "airlog.41.5.air_bombardment";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: BombRaw = raw.deserialize()?;
        check_column_indices(raw, r.column.iter().map(|c| c.index))?;
        if r.block.len() != 7 {
            return Err(raw.err("block", "needs all seven target blocks"));
        }
        let torpedo: Vec<Band> = r.column.iter().map(|c| c.torpedo_points).collect();
        let barrage: Vec<Band> = r.column.iter().map(|c| c.barrage_points).collect();
        let bomb: Vec<Band> = r.column.iter().map(|c| c.bomb_points).collect();
        check_bands(&torpedo, 1).map_err(|m| raw.err("column.torpedo_points", m))?;
        check_bands(&barrage, 1).map_err(|m| raw.err("column.barrage_points", m))?;
        check_bands(&bomb, 1).map_err(|m| raw.err("column.bomb_points", m))?;
        let mut blocks: Vec<(BombTarget, Grid<BombResult>)> = Vec::new();
        for (b, block) in r.block.into_iter().enumerate() {
            if blocks.iter().any(|(t, _)| *t == block.target) {
                return Err(raw.err(format!("block[{b}].target"), "target listed twice"));
            }
            let mut rows = Vec::new();
            for (i, row) in block.row.iter().enumerate() {
                let res = bomb_result(block.target, &block.result_unit, &row.result)
                    .map_err(|m| raw.err(format!("block[{b}].row[{i}].result"), m))?;
                rows.push((res, row.rolls.clone()));
            }
            let grid = Grid::new(
                raw,
                &format!("block[{b}] ({:?})", block.target),
                bomb.len(),
                rows,
            )?;
            blocks.push((block.target, grid));
        }
        Ok(Self {
            torpedo,
            barrage,
            bomb,
            blocks,
        })
    }
}

impl AirBombardment {
    /// Result of an air bombardment of `target` with `points` of `weapon` on the given two-dice
    /// reading. `None` when `points` is below the first column or the target has no block.
    /// A mixed bomb-and-torpedo attack is two attacks; modifiers to the points or the result
    /// are applied by the caller. `airlog:41.5`.
    pub fn result(
        &self,
        target: BombTarget,
        weapon: Weapon,
        points: i32,
        reading: TwoDiceReading,
    ) -> Option<BombResult> {
        let bands = match weapon {
            Weapon::Torpedo => &self.torpedo,
            Weapon::Barrage => &self.barrage,
            Weapon::Bomb => &self.bomb,
        };
        let col = band_index(bands, points)?;
        let (_, grid) = self.blocks.iter().find(|(t, _)| *t == target)?;
        Some(*grid.lookup(col, reading))
    }

    /// The 1-based column a number of points selects for `weapon`. `airlog:41.5`.
    pub fn column(&self, weapon: Weapon, points: i32) -> Option<usize> {
        let bands = match weapon {
            Weapon::Torpedo => &self.torpedo,
            Weapon::Barrage => &self.barrage,
            Weapon::Bomb => &self.bomb,
        };
        band_index(bands, points).map(|i| i + 1)
    }
}

// ---------------------------------------------------------------------------------------------
// 46.3 Anti-Aircraft Combat Results and 46.4 Flak Adjustment
// ---------------------------------------------------------------------------------------------

/// Which planes a flak attack is aimed at (`airlog:46.3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AaGroup {
    PlanesOnOtherMissions,
    PlanesOnFighterMissions,
}

/// The effect an anti-aircraft roll resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AaEffect {
    PlanesDestroyed,
    PlanesAborted,
}

#[derive(Debug, Clone, Deserialize)]
struct AaColumnRaw {
    index: u8,
    flak_points: Band,
}

#[derive(Debug, Clone, Deserialize)]
struct AaSectionRaw {
    target: AaGroup,
    result_kind: AaEffect,
    row: Vec<StrafeRowRaw>,
}

#[derive(Debug, Clone, Deserialize)]
struct AaRaw {
    column: Vec<AaColumnRaw>,
    section: Vec<AaSectionRaw>,
}

/// The Anti-Aircraft Combat Results table (`airlog:46.3`).
#[derive(Debug, Clone)]
pub struct AaCombat {
    bands: Vec<Band>,
    sections: Vec<(AaGroup, AaEffect, Grid<u8>)>,
}

impl Bound for AaCombat {
    const ID: &'static str = "airlog.46.3.aa_combat_results";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: AaRaw = raw.deserialize()?;
        check_column_indices(raw, r.column.iter().map(|c| c.index))?;
        if r.section.len() != 3 {
            return Err(raw.err("section", "needs the three printed target/effect sections"));
        }
        let bands: Vec<Band> = r.column.iter().map(|c| c.flak_points).collect();
        check_bands(&bands, 1).map_err(|m| raw.err("column.flak_points", m))?;
        let mut sections = Vec::new();
        for (s, sec) in r.section.into_iter().enumerate() {
            if sections
                .iter()
                .any(|(g, e, _)| *g == sec.target && *e == sec.result_kind)
            {
                return Err(raw.err(format!("section[{s}]"), "target and result_kind repeat"));
            }
            let grid = Grid::new(
                raw,
                &format!("section[{s}] ({:?}, {:?})", sec.target, sec.result_kind),
                bands.len(),
                sec.row.into_iter().map(|x| (x.result, x.rolls)).collect(),
            )?;
            sections.push((sec.target, sec.result_kind, grid));
        }
        for (g, e) in [
            (AaGroup::PlanesOnOtherMissions, AaEffect::PlanesDestroyed),
            (AaGroup::PlanesOnOtherMissions, AaEffect::PlanesAborted),
            (AaGroup::PlanesOnFighterMissions, AaEffect::PlanesDestroyed),
        ] {
            if !sections.iter().any(|(sg, se, _)| *sg == g && *se == e) {
                return Err(raw.err("section", format!("missing section {g:?} / {e:?}")));
            }
        }
        Ok(Self { bands, sections })
    }
}

impl AaCombat {
    /// Number of planes destroyed or aborted by flak. `flak_points` selects the column;
    /// `shift_right` is the density shift of [`FlakAdjustment::column_shift`], clamped to the
    /// last column. `None` for no flak points, or an effect the chart has no section for
    /// (fighter missions are never aborted). `airlog:46.3`.
    pub fn planes(
        &self,
        group: AaGroup,
        effect: AaEffect,
        flak_points: i32,
        shift_right: i32,
        reading: TwoDiceReading,
    ) -> Option<u8> {
        let base = band_index(&self.bands, flak_points)? as i32;
        let col = base
            .saturating_add(shift_right)
            .clamp(0, self.bands.len() as i32 - 1) as usize;
        let (_, _, grid) = self
            .sections
            .iter()
            .find(|(g, e, _)| *g == group && *e == effect)?;
        Some(*grid.lookup(col, reading))
    }
}

#[derive(Debug, Clone, Deserialize)]
struct AdjustmentRaw {
    unit_size_aircraft: i32,
    columns_shift_right_per_unit_exceeding_first: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct FlakAdjustmentRaw {
    adjustment: AdjustmentRaw,
}

/// The Flak Adjustment chart (`airlog:46.4`, rule restated in the notes of 46.3).
#[derive(Debug, Clone)]
pub struct FlakAdjustment {
    unit_size: i32,
    shift_per_unit: i32,
}

impl Bound for FlakAdjustment {
    const ID: &'static str = "airlog.46.4.flak_adjustment";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: FlakAdjustmentRaw = raw.deserialize()?;
        if r.adjustment.unit_size_aircraft <= 0 {
            return Err(raw.err("adjustment.unit_size_aircraft", "must be positive"));
        }
        if r.adjustment.columns_shift_right_per_unit_exceeding_first < 0 {
            return Err(raw.err(
                "adjustment.columns_shift_right_per_unit_exceeding_first",
                "must not be negative",
            ));
        }
        Ok(Self {
            unit_size: r.adjustment.unit_size_aircraft,
            shift_per_unit: r.adjustment.columns_shift_right_per_unit_exceeding_first,
        })
    }
}

impl FlakAdjustment {
    /// Columns the flak table shifts right for a non-fighter target group of `aircraft` planes
    /// containing bomber-class or transport-class planes.
    ///
    /// `interp:airlog-0005`: shift = floor((n - 12) / 12) for n of 12 or more, else 0, so 12-23
    /// planes shift 0, 24-35 shift 1, and so on. `airlog:46.4`, `airlog:46.3`.
    pub fn column_shift(&self, aircraft: i32) -> i32 {
        if aircraft < self.unit_size {
            0
        } else {
            (aircraft - self.unit_size) / self.unit_size * self.shift_per_unit
        }
    }
}
