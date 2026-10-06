//! What units, weapons and planes ARE: weapon systems, ID-code classes, aircraft types, the OA
//! sheets (every unit's canonical identity and assigned hierarchy) and the reinforcement schedules.
//!
//! Schema: `data/units/README.md` (owned by the `oob` area). Where a unit stands at the start of a
//! scenario is [`crate::scenario`]. Unknown fields are ignored here; `tools/units/validate.py` is the
//! schema gate for the data itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cna_core::ids::UnitId;
use cna_protocol::Side;
use serde::{Deserialize, Serialize};

use crate::{ContentError, read_toml, toml_files};
mod ships;
pub use ships::{CoastalShip, ShipRoster};

/// Everything under `data/units/`.
#[derive(Debug, Clone, Default)]
pub struct UnitsContent {
    pub coastal_ships: BTreeMap<String, CoastalShip>,
    /// Validated relative data/units paths and their distinct counter ids.
    pub coastal_rosters: BTreeMap<String, Vec<String>>,
    pub weapons: BTreeMap<String, Weapon>,
    pub classes: BTreeMap<String, UnitClass>,
    pub aircraft: BTreeMap<String, Aircraft>,
    pub sheets: BTreeMap<String, OaSheet>,
    /// Every unit of every OA sheet, by canonical id.
    pub units: BTreeMap<UnitId, OaUnit>,
    /// Units printed on a sheet they are not assigned to (`[[mention]]`).
    pub mentions: Vec<Mention>,
    pub schedules: Vec<Schedule>,
}

/// A weapon system: tank, gun, anti-tank or anti-air (`land:4.47`–`4.49`). An omitted rating is
/// `None`: the chart prints "–".
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Weapon {
    pub id: String,
    pub nation: String,
    pub kind: String,
    pub name: String,
    pub cpa: i32,
    #[serde(default)]
    pub cpa_sited: bool,
    pub aa: Option<i32>,
    pub barrage: Option<i32>,
    pub anti_armor: Option<i32>,
    #[serde(default)]
    pub anti_armor_paren: bool,
    pub vulnerability: Option<i32>,
    pub armor_prot: Option<i32>,
    pub ca_off: Option<i32>,
    #[serde(default)]
    pub ca_off_paren: bool,
    pub ca_def: Option<i32>,
    #[serde(default)]
    pub ca_def_paren: bool,
    /// Fuel points per TOE point per 5 CP (or fraction) of movement (`airlog:49.13`).
    pub fuel_rate: Option<i32>,
    /// Breakdown adjustment (`land:21`).
    pub bar: Option<BreakdownAdjustment>,
    #[serde(default)]
    pub us_tank: bool,
    #[serde(default)]
    pub minesweeper: bool,
    #[serde(default)]
    pub airdroppable: bool,
    pub note: Option<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

/// `bar = { shift = 1, dir = "R" }`; `shift = 0` means no adjustment.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BreakdownAdjustment {
    pub shift: i32,
    pub dir: Option<String>,
}

/// One row of a Unit Characteristics chart: what a counter with this ID code can be
/// (`land:4.46a`–`c`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UnitClass {
    pub id: String,
    pub nation: String,
    pub code: String,
    pub unit_type: String,
    pub echelon: Option<String>,
    pub cpa: i32,
    #[serde(default)]
    pub cpa_fixed: bool,
    #[serde(default)]
    pub cpa_plus: bool,
    #[serde(default)]
    pub emplaced: bool,
    pub max_toe: Option<i32>,
    #[serde(default)]
    pub max_toe_paren: bool,
    pub max_toe_extra: Option<i32>,
    pub max_toe_kind: Option<String>,
    pub max_toe_if_all_us_tanks: Option<i32>,
    /// Weapon kinds an HQ / gun / tank class may hold, with maxima.
    #[serde(default)]
    pub assigns: Vec<ClassAssign>,
    pub aa: Option<i32>,
    pub barrage: Option<i32>,
    pub anti_armor: Option<i32>,
    #[serde(default)]
    pub anti_armor_paren: bool,
    pub vulnerability: Option<i32>,
    pub armor_prot: Option<i32>,
    #[serde(default)]
    pub armor_prot_paren: bool,
    #[serde(default)]
    pub armor_prot_only_when_truck_transported: bool,
    pub ca_off: Option<i32>,
    #[serde(default)]
    pub ca_off_paren: bool,
    pub ca_def: Option<i32>,
    #[serde(default)]
    pub ca_def_paren: bool,
    pub role: Option<String>,
    pub equipment_note: Option<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ClassAssign {
    pub kind: String,
    pub max: i32,
    pub note: Option<String>,
}

/// An aircraft type (`land:4.44a`–`c`). Rows printed with alternative lines are several modes;
/// the owner picks one each time the plane is readied.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Aircraft {
    pub id: String,
    pub nation: String,
    pub name: String,
    pub role: String,
    pub manufacturer: Option<String>,
    #[serde(default, rename = "mode")]
    pub modes: Vec<AircraftMode>,
    pub note: Option<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AircraftMode {
    pub range_hexes: Option<i32>,
    #[serde(default)]
    pub rng_is_transfer_range: bool,
    pub tacair: Option<i32>,
    #[serde(default)]
    pub tacair_paren: bool,
    pub maneuver: Option<i32>,
    pub maneuver_night: Option<i32>,
    pub fuel_points: Option<i32>,
    pub bomb_capacity: Option<i32>,
    pub torpedo_capacity: Option<i32>,
    pub transport: Option<TransportCapacity>,
    /// Mission letter (`f`, `s`, `r`, `d`, `b`) -> `day` | `night` | `night_only` | `strafe_only`.
    #[serde(default)]
    pub missions: BTreeMap<String, String>,
    #[serde(default)]
    pub strafe_armor: bool,
}

/// Transport capacity in quarter TOE points or half tons, so no fractions are needed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TransportCapacity {
    pub toe_quarter_points: Option<i32>,
    pub or_half_tons: Option<i32>,
    #[serde(default)]
    pub paradrop_only: bool,
}

/// One printed OA sheet (`land:4.45`): the roster of one parent formation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OaSheet {
    pub id: String,
    pub nation: String,
    pub side: Side,
    pub nationality: String,
    pub name: String,
    pub basic_morale: Option<i32>,
    pub basic_morale_untrained: Option<i32>,
    pub note: Option<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

/// A unit's TOE at the start: `"N"` (normal = class maximum), under/over strength, or an
/// explicit weapons list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Toe {
    Normal(NormalToe),
    Under { under: i32 },
    Over { over: i32 },
    Weapons(Vec<WeaponPoints>),
}

/// The literal `"N"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NormalToe {
    N,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeaponPoints {
    pub weapon: String,
    pub n: i32,
}

/// When a unit enters play: deployed at the start (`"D"`) or a game-turn and OpStage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Arrival {
    Deployed(DeployedCode),
    At { gt: u16, opstage: u8 },
}

/// The literal `"D"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeployedCode {
    D,
}

impl Arrival {
    pub fn is_deployed(self) -> bool {
        matches!(self, Arrival::Deployed(_))
    }
}

/// One unit row of an OA sheet, with the sheet's side, nationality and morale resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OaUnit {
    pub id: UnitId,
    pub sheet: String,
    pub side: Side,
    pub nationality: String,
    pub name: String,
    /// Printed counter text. Not unique; never an id.
    pub counter: String,
    pub class: Option<String>,
    pub echelon: Option<String>,
    pub toe: Option<Toe>,
    pub arrives: Arrival,
    /// The assigned parent in the OA hierarchy.
    pub parent: Option<UnitId>,
    pub stacking_points: Option<i32>,
    /// The unit's basic morale: its own row value, else the sheet's.
    pub basic_morale: Option<i32>,
    pub commander: bool,
    pub cpa: Option<i32>,
    pub vehicle: Option<String>,
    pub engineer_hq: bool,
    pub never_arrived_parent: bool,
    pub immobile: bool,
    pub group: Option<String>,
    pub kind: Option<String>,
    pub note: Option<String>,
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Mention {
    pub unit: UnitId,
    pub begins_attached_to: Option<UnitId>,
    #[serde(default)]
    pub src: Vec<String>,
}

/// A reinforcement / withdrawal schedule file (`land:4.43`, `airlog:34.8`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Schedule {
    #[serde(skip)]
    pub path: PathBuf,
    pub file: ScheduleHeader,
    #[serde(default, rename = "arrival")]
    pub arrivals: Vec<ScheduledArrival>,
    #[serde(default, rename = "withdrawal")]
    pub withdrawals: Vec<ScheduledWithdrawal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScheduleHeader {
    pub side: Side,
    #[serde(default)]
    pub partial: bool,
    #[serde(default)]
    pub covers_gt: Vec<u16>,
    pub truck_value_halves: Option<TruckValueHalves>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScheduledArrival {
    pub gt: Option<u16>,
    pub opstage: Option<u8>,
    pub gt_from: Option<u16>,
    pub gt_to: Option<u16>,
    pub label: Option<String>,
    pub location: Option<String>,
    pub nationality: Option<String>,
    pub distribution: Option<String>,
    #[serde(default)]
    pub units: Vec<ScheduledUnit>,
    #[serde(default)]
    pub planes: Vec<ScheduledPlanes>,
    pub trucks: Option<Trucks>,
    #[serde(default)]
    pub alone: bool,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScheduledUnit {
    pub unit: UnitId,
    #[serde(default)]
    pub subtree: bool,
    #[serde(default)]
    pub hq_only: bool,
    #[serde(default)]
    pub less: Vec<UnitId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScheduledPlanes {
    #[serde(rename = "type")]
    pub aircraft: String,
    pub n: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScheduledWithdrawal {
    pub gt: Option<u16>,
    pub opstage: Option<u8>,
    #[serde(default)]
    pub units: Vec<ScheduledUnit>,
    pub transport: Option<WithdrawalTransport>,
    pub when: Option<String>,
    pub label: Option<String>,
    pub note: Option<String>,
    #[serde(default)]
    pub squadrons: Vec<WithdrawnSquadrons>,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WithdrawnSquadrons {
    pub count: i32,
    pub role: Option<String>,
    pub min_planes: Option<i32>,
    pub min_bomb_points_each: Option<i32>,
}

/// The printed minimum full-Logistics truck value / abstract motorization pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct WithdrawalTransport {
    pub truck_value_points: i32,
    pub motorization_points: i32,
}

/// Truck value conversion in integer halves, read from the reinforcement chart footnote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct TruckValueHalves {
    pub light: i32,
    pub medium: i32,
    pub heavy: i32,
}

impl TruckValueHalves {
    /// The full-Logistics value of physical truck points, measured in half-value points.
    /// Cases: land:4.43a
    pub fn value(self, trucks: Trucks) -> i64 {
        i64::from(self.light) * i64::from(trucks.light)
            + i64::from(self.medium) * i64::from(trucks.medium)
            + i64::from(self.heavy) * i64::from(trucks.heavy)
    }
}

/// Truck points by type (`airlog:53`, `airlog:54.2`). Omitted types are zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trucks {
    #[serde(default)]
    pub light: i32,
    #[serde(default)]
    pub medium: i32,
    #[serde(default)]
    pub heavy: i32,
}

impl Trucks {
    pub fn total(self) -> i32 {
        self.light + self.medium + self.heavy
    }
}

#[derive(Deserialize)]
struct WeaponFile {
    #[serde(default)]
    weapon: Vec<Weapon>,
}

#[derive(Deserialize)]
struct ClassFile {
    #[serde(default)]
    class: Vec<UnitClass>,
}

#[derive(Deserialize)]
struct AircraftFile {
    #[serde(default)]
    aircraft: Vec<Aircraft>,
}

#[derive(Deserialize)]
struct SheetFile {
    sheet: OaSheet,
    #[serde(default)]
    unit: Vec<UnitRow>,
    #[serde(default)]
    mention: Vec<Mention>,
}

#[derive(Deserialize)]
struct UnitRow {
    id: UnitId,
    name: String,
    counter: String,
    class: Option<String>,
    echelon: Option<String>,
    toe: Option<Toe>,
    arrives: Arrival,
    parent: Option<UnitId>,
    stacking_points: Option<i32>,
    basic_morale: Option<i32>,
    nationality: Option<String>,
    #[serde(default)]
    commander: bool,
    cpa: Option<i32>,
    vehicle: Option<String>,
    #[serde(default)]
    engineer_hq: bool,
    #[serde(default)]
    never_arrived_parent: bool,
    #[serde(default)]
    immobile: bool,
    group: Option<String>,
    kind: Option<String>,
    note: Option<String>,
    #[serde(default)]
    src: Vec<String>,
}

impl UnitsContent {
    /// Load everything under a `data/units` folder and check its internal references.
    pub fn load(units_dir: &Path) -> Result<Self, ContentError> {
        let mut out = UnitsContent::default();
        for path in toml_files(&units_dir.join("weapons"))? {
            for w in read_toml::<WeaponFile>(&path)?.weapon {
                insert_unique(&mut out.weapons, w.id.clone(), w, &path)?;
            }
        }
        for path in toml_files(&units_dir.join("classes"))? {
            for c in read_toml::<ClassFile>(&path)?.class {
                insert_unique(&mut out.classes, c.id.clone(), c, &path)?;
            }
        }
        for path in toml_files(&units_dir.join("aircraft"))? {
            for a in read_toml::<AircraftFile>(&path)?.aircraft {
                insert_unique(&mut out.aircraft, a.id.clone(), a, &path)?;
            }
        }
        for path in toml_files(&units_dir.join("ships"))? {
            let roster = ShipRoster::load(&path)?;
            let key = path
                .strip_prefix(units_dir)
                .expect("units child")
                .to_string_lossy()
                .replace('\\', "/");
            out.coastal_rosters
                .insert(key, roster.ships.keys().cloned().collect());
            for ship in roster.ships.into_values() {
                insert_unique(&mut out.coastal_ships, ship.id.clone(), ship, &path)?;
            }
        }
        let oa_dir = units_dir.join("oa");
        let mut sheet_files = Vec::new();
        for nation in subdirs(&oa_dir)? {
            sheet_files.extend(toml_files(&nation)?);
        }
        for path in sheet_files {
            let file: SheetFile = read_toml(&path)?;
            let sheet = file.sheet;
            for row in file.unit {
                let unit = OaUnit {
                    sheet: sheet.id.clone(),
                    side: sheet.side,
                    nationality: row.nationality.unwrap_or_else(|| sheet.nationality.clone()),
                    basic_morale: row.basic_morale.or(sheet.basic_morale),
                    id: row.id,
                    name: row.name,
                    counter: row.counter,
                    class: row.class,
                    echelon: row.echelon,
                    toe: row.toe,
                    arrives: row.arrives,
                    parent: row.parent,
                    stacking_points: row.stacking_points,
                    commander: row.commander,
                    cpa: row.cpa,
                    vehicle: row.vehicle,
                    engineer_hq: row.engineer_hq,
                    never_arrived_parent: row.never_arrived_parent,
                    immobile: row.immobile,
                    group: row.group,
                    kind: row.kind,
                    note: row.note,
                    src: row.src,
                };
                insert_unique(&mut out.units, unit.id.clone(), unit, &path)?;
            }
            out.mentions.extend(file.mention);
            insert_unique(&mut out.sheets, sheet.id.clone(), sheet, &path)?;
        }
        for path in toml_files(&units_dir.join("schedules"))? {
            let mut schedule: Schedule = read_toml(&path)?;
            schedule.path = path;
            out.schedules.push(schedule);
        }
        out.check(units_dir)?;
        Ok(out)
    }

    fn check(&self, units_dir: &Path) -> Result<(), ContentError> {
        let invalid = |message: String| ContentError::Invalid {
            path: units_dir.to_path_buf(),
            message,
        };
        for unit in self.units.values() {
            if let Some(class) = &unit.class
                && !self.classes.contains_key(class)
            {
                return Err(invalid(format!("{}: unknown class {class}", unit.id)));
            }
            if let Some(parent) = &unit.parent
                && !self.units.contains_key(parent)
            {
                return Err(invalid(format!("{}: unknown parent {parent}", unit.id)));
            }
            if let Some(Toe::Weapons(list)) = &unit.toe {
                for wp in list {
                    if !self.weapons.contains_key(&wp.weapon) {
                        return Err(invalid(format!(
                            "{}: unknown weapon {}",
                            unit.id, wp.weapon
                        )));
                    }
                }
            }
        }
        for m in &self.mentions {
            let known = |id: &UnitId| self.units.contains_key(id);
            if !known(&m.unit) || m.begins_attached_to.as_ref().is_some_and(|p| !known(p)) {
                return Err(invalid(format!("mention of unknown unit {}", m.unit)));
            }
        }
        for s in &self.schedules {
            if s.withdrawals.iter().any(|w| w.transport.is_some())
                && s.file.truck_value_halves.is_none()
            {
                return Err(invalid(format!(
                    "{}: withdrawal transport lacks truck value weights",
                    s.path.display()
                )));
            }
            if let Some(weights) = s.file.truck_value_halves
                && [weights.light, weights.medium, weights.heavy]
                    .iter()
                    .any(|v| *v <= 0)
            {
                return Err(invalid(format!(
                    "{}: nonpositive truck value weight",
                    s.path.display()
                )));
            }
            for selector in s
                .arrivals
                .iter()
                .flat_map(|a| &a.units)
                .chain(s.withdrawals.iter().flat_map(|w| &w.units))
            {
                for id in std::iter::once(&selector.unit).chain(&selector.less) {
                    if !self.units.contains_key(id) {
                        return Err(invalid(format!(
                            "{}: unknown scheduled unit {id}",
                            s.path.display()
                        )));
                    }
                }
            }
            for a in &s.arrivals {
                for u in &a.units {
                    if !self.units.contains_key(&u.unit) {
                        return Err(invalid(format!(
                            "{}: arrival of unknown unit {}",
                            s.path.display(),
                            u.unit
                        )));
                    }
                }
                for p in &a.planes {
                    if !self.aircraft.contains_key(&p.aircraft) {
                        return Err(invalid(format!(
                            "{}: arrival of unknown aircraft {}",
                            s.path.display(),
                            p.aircraft
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// A unit's assigned subordinates (direct children in the OA hierarchy), in id order.
    pub fn children(&self, parent: &UnitId) -> impl Iterator<Item = &OaUnit> {
        let parent = parent.clone();
        self.units
            .values()
            .filter(move |u| u.parent.as_ref() == Some(&parent))
    }

    /// The unit and every unit assigned below it, depth-first, in id order at each level.
    pub fn subtree(&self, root: &UnitId) -> Vec<&OaUnit> {
        let mut out = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(id) = stack.pop() {
            if let Some(u) = self.units.get(&id) {
                out.push(u);
                let mut kids: Vec<_> = self.children(&id).map(|c| c.id.clone()).collect();
                kids.reverse();
                stack.extend(kids);
            }
        }
        out
    }
}

fn insert_unique<K: Ord + std::fmt::Display + Clone, V>(
    map: &mut BTreeMap<K, V>,
    key: K,
    value: V,
    path: &Path,
) -> Result<(), ContentError> {
    if map.insert(key.clone(), value).is_some() {
        return Err(ContentError::Invalid {
            path: path.to_path_buf(),
            message: format!("duplicate id {key}"),
        });
    }
    Ok(())
}

fn subdirs(dir: &Path) -> Result<Vec<PathBuf>, ContentError> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|error| ContentError::Io {
        path: dir.to_path_buf(),
        error,
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| ContentError::Io {
            path: dir.to_path_buf(),
            error,
        })?;
        if entry.path().is_dir() {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod schedule_tests {
    use super::*;

    /// Cases: land:4.43a, land:20.8, land:31.1, land:4.44b
    #[test]
    fn engine_fields_survive_typed_loading() {
        let units = UnitsContent::load(&crate::repo_data_dir().join("units")).unwrap();
        let guards = units
            .schedules
            .iter()
            .flat_map(|s| &s.arrivals)
            .find(|a| a.gt == Some(20) && a.units.iter().any(|u| u.hq_only))
            .unwrap();
        assert_eq!(guards.units.iter().filter(|u| u.hq_only).count(), 2);
        let withdrawal = units
            .schedules
            .iter()
            .flat_map(|s| &s.withdrawals)
            .find(|w| w.gt == Some(15) && !w.units.is_empty())
            .unwrap();
        assert_eq!(withdrawal.opstage, Some(2));
        assert_eq!(withdrawal.units[0].less.len(), 2);
        assert_eq!(
            withdrawal.transport.unwrap(),
            WithdrawalTransport {
                truck_value_points: 16,
                motorization_points: 12
            }
        );
        let weights = units
            .schedules
            .iter()
            .find_map(|s| s.file.truck_value_halves)
            .unwrap();
        assert_eq!(
            weights,
            TruckValueHalves {
                light: 1,
                medium: 2,
                heavy: 4
            }
        );
        assert_eq!(
            weights.value(Trucks {
                light: 10,
                medium: 5,
                heavy: 2
            }),
            28
        );
        let rommel = units.units.values().find(|u| u.commander).unwrap();
        assert_eq!(rommel.nationality, "german");
        assert_eq!(rommel.cpa, Some(60));
        assert!(rommel.vehicle.is_some());
        assert_eq!(units.aircraft["ge.bf110"].modes[0].maneuver_night, Some(32));
    }
}
