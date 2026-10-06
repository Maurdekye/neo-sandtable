//! A scenario's starting state: clock, initiative, victory conditions, and where every unit,
//! plane, truck, dump and facility stands at the start.
//!
//! Schema: `data/scenarios/README.md` (owned by the `oob` area). Unit ids refer to
//! [`crate::units`]; [`ScenarioContent::check`] verifies those references. Files whose systems the
//! engine does not model yet (construction, fleet, arrivals) are kept as raw TOML tables until
//! their owners type them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cna_core::ids::{HexId, UnitId};
use cna_protocol::Side;
use serde::Deserialize;

use crate::units::{Trucks, UnitsContent};
use crate::{ContentError, read_toml};

/// Everything in one `data/scenarios/<id>/` folder.
#[derive(Debug, Clone)]
pub struct ScenarioContent {
    pub dir: PathBuf,
    pub meta: ScenarioMeta,
    pub initiative: InitiativeSetup,
    pub victory: Vec<VictoryLevel>,
    /// One per side file (`land_axis.toml`, `land_cw.toml`).
    pub land: Vec<LandDeployment>,
    /// One per side file (`air_axis.toml`, `air_cw.toml`).
    pub air: Vec<AirForce>,
    pub supply: SupplySetup,
    pub facilities: FacilitiesSetup,
    pub construction: toml::Table,
    pub fleet: toml::Table,
    pub arrivals: toml::Table,
}

/// A point in the game clock: game-turn and OpStage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub struct GtOpStage {
    pub gt: u16,
    pub opstage: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScenarioMeta {
    pub id: String,
    pub name: String,
    pub start: GtOpStage,
    pub end: GtOpStage,
    /// `land`, `air`, `logistics`: the systems in play.
    #[serde(default)]
    pub systems: Vec<String>,
    /// ISO date of Game-Turn 1's first day, when the data gives it.
    pub campaign_start_date: Option<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InitiativeSetup {
    /// Who holds the initiative on Game-Turn 1 (`scen:60.6`).
    pub gt1: Option<Side>,
    /// How initiative is determined from Game-Turn 2 (`normal_rules` = `land:7`).
    pub from_gt2: Option<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

/// One victory level for one side; every clause must hold at the end of the game.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VictoryLevel {
    pub side: Side,
    pub level: String,
    #[serde(default)]
    pub require: Vec<VictoryClause>,
    pub note: Option<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

/// A victory clause (`data/scenarios/README.md`, "Victory clause vocabulary"). Exactly one of
/// the clause keys is present; the others qualify it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct VictoryClause {
    pub occupy_all: Option<Vec<String>>,
    pub occupy_any: Option<Vec<String>>,
    pub retain: Option<Vec<String>>,
    pub qualifying_unit: Option<String>,
    pub supply_trace: Option<String>,
    pub to: Option<String>,
    pub from: Option<String>,
    pub home_base: Option<Vec<String>>,
}

/// Where something is placed at the start. Anything but `Hex` leaves a choice to the player.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Placement {
    /// Exactly this hex.
    Hex { hex: HexId },
    /// Split freely among these hexes.
    HexesAny { hexes: Vec<HexId> },
    /// Anywhere in a named area (hex sets: `data/map/areas.toml`).
    Area {
        area: String,
        exclusion: Option<Exclusion>,
    },
    /// Within `n` hexes of a hex.
    Within { hex: HexId, n: u32 },
    /// In a named city or off-map box.
    City { city: String },
}

impl Placement {
    /// The single hex when the placement leaves no choice.
    pub fn fixed_hex(&self) -> Option<&HexId> {
        match self {
            Placement::Hex { hex } => Some(hex),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Exclusion {
    /// No enemy unit within this many hexes.
    pub enemy_unit_within_hexes: Option<u32>,
}

/// `land_<side>.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LandDeployment {
    pub file: FileHeader,
    #[serde(default, rename = "group")]
    pub groups: Vec<DeployGroup>,
    #[serde(default)]
    pub bonus_toe: Vec<BonusToe>,
    #[serde(default)]
    pub broken_down_vehicles: Vec<BrokenDownVehicles>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileHeader {
    pub side: Option<Side>,
    #[serde(default)]
    pub src: Vec<String>,
    pub verification: Option<String>,
}

/// One set-up line: a placement, its first-line trucks, and the units in it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeployGroup {
    pub id: String,
    pub placement: Placement,
    /// First-line truck points the player distributes among the group's units (`scen:59.42`).
    pub trucks: Option<Trucks>,
    /// e.g. `in_training`.
    pub state: Option<String>,
    pub note: Option<String>,
    #[serde(default, rename = "unit")]
    pub units: Vec<GroupUnit>,
    #[serde(default)]
    pub src: Vec<String>,
}

/// One entry of a set-up line, in the booklet's deviation vocabulary (`scen:59.2`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct GroupUnit {
    /// An OA unit with its assigned subtree that arrives deployed, minus `less` and `det`.
    pub unit: Option<UnitId>,
    /// Every deployed unit of a whole OA sheet.
    pub sheet: Option<String>,
    /// Only the HQ counter itself.
    #[serde(default)]
    pub hq_only: bool,
    #[serde(default)]
    pub att: Vec<UnitId>,
    #[serde(default)]
    pub assg: Vec<UnitId>,
    #[serde(default)]
    pub less: Vec<UnitId>,
    #[serde(default)]
    pub det: Vec<UnitId>,
    #[serde(default)]
    pub consists_of: Vec<UnitId>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BonusToe {
    pub id: String,
    pub kind: String,
    pub count: i32,
    pub id_code: Option<String>,
    pub rule: Option<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BrokenDownVehicles {
    pub location: Placement,
    #[serde(default)]
    pub vehicles: Vec<BrokenDownWeapon>,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BrokenDownWeapon {
    pub weapon: String,
    pub toe_points: i32,
}

/// `air_<side>.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AirForce {
    pub force: AirForceHeader,
    #[serde(default, rename = "plane")]
    pub planes: Vec<PlaneSetup>,
    pub pilots: Option<Pilots>,
    pub sgsu: Option<SgsuSetup>,
    pub malta: Option<MaltaSetup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AirForceHeader {
    pub side: Side,
    pub theatre: Option<String>,
    /// No refit attempts before this point.
    pub refit_not_before: Option<GtOpStage>,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PlaneSetup {
    #[serde(rename = "type")]
    pub aircraft: String,
    pub total: i32,
    /// Planes that start refitted (ready).
    pub ready: Option<i32>,
    pub sgsu: Option<i32>,
    pub note: Option<String>,
    pub squadron_note: Option<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

/// Pilots by quality rating.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct Pilots {
    #[serde(default)]
    pub three: i32,
    #[serde(default)]
    pub two: i32,
    #[serde(default)]
    pub one: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SgsuSetup {
    pub available: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MaltaSetup {
    pub aa_points: Option<i32>,
    pub facility_capacity_sgsu: Option<i32>,
    pub pilots: Option<Pilots>,
    #[serde(default, rename = "plane")]
    pub planes: Vec<PlaneSetup>,
    #[serde(default)]
    pub src: Vec<String>,
}

/// `supply.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SupplySetup {
    #[serde(default, rename = "dump")]
    pub dumps: Vec<DumpSetup>,
    #[serde(default, rename = "dummy_dump")]
    pub dummy_dumps: Vec<DummyDumpSetup>,
    /// Freely distributable among a side's airfields, by side.
    #[serde(default)]
    pub air_supply_pool: BTreeMap<Side, Supplies>,
    #[serde(default)]
    pub second_third_line_trucks: Vec<SecondThirdLineTrucks>,
    pub unlimited_supply: Option<UnlimitedSupply>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DumpSetup {
    pub id: String,
    pub side: Side,
    pub location: Placement,
    #[serde(flatten)]
    pub supplies: Supplies,
    #[serde(default = "yes")]
    pub active: bool,
    #[serde(default)]
    pub src: Vec<String>,
}

fn yes() -> bool {
    true
}

/// Supply points by type. Omitted types are zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct Supplies {
    #[serde(default)]
    pub ammo: i32,
    #[serde(default)]
    pub fuel: i32,
    #[serde(default)]
    pub stores: i32,
    #[serde(default)]
    pub water: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DummyDumpSetup {
    pub side: Side,
    pub count: i32,
    pub location: Placement,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SecondThirdLineTrucks {
    pub side: Side,
    pub placement: Placement,
    #[serde(flatten)]
    pub trucks: Trucks,
    pub purpose: Option<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UnlimitedSupply {
    pub side: Side,
    #[serde(default)]
    pub locations: Vec<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

/// `facilities.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct FacilitiesSetup {
    #[serde(default, rename = "facility")]
    pub facilities: Vec<FacilitySetup>,
    #[serde(default, rename = "repair_facility")]
    pub repair_facilities: Vec<RepairFacilitySetup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FacilitySetup {
    pub id: String,
    /// `airfield`, `landing_strip`, `flying_boat_basin`, `alighting_area`, …
    pub kind: String,
    pub name: Option<String>,
    pub hex: Option<HexId>,
    #[serde(default)]
    pub hexes: Vec<HexId>,
    pub count: Option<i32>,
    pub owner: Option<String>,
    #[serde(default)]
    pub owner_by_country: bool,
    #[serde(default)]
    pub off_map: bool,
    pub printed_location: Option<String>,
    pub note: Option<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RepairFacilitySetup {
    pub side: Side,
    pub class: String,
    pub location: Placement,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Deserialize)]
struct ScenarioFile {
    scenario: ScenarioMeta,
    initiative: InitiativeSetup,
    #[serde(default)]
    victory: Vec<VictoryLevel>,
}

impl ScenarioContent {
    /// Load a `data/scenarios/<id>` folder.
    pub fn load(dir: &Path) -> Result<Self, ContentError> {
        let main: ScenarioFile = read_toml(&dir.join("scenario.toml"))?;
        let mut land = Vec::new();
        let mut air = Vec::new();
        for side in ["axis", "cw"] {
            let path = dir.join(format!("land_{side}.toml"));
            if path.exists() {
                land.push(read_toml(&path)?);
            }
            let path = dir.join(format!("air_{side}.toml"));
            if path.exists() {
                air.push(read_toml(&path)?);
            }
        }
        let optional = |name: &str| -> Result<Option<PathBuf>, ContentError> {
            let path = dir.join(name);
            Ok(path.exists().then_some(path))
        };
        let supply = match optional("supply.toml")? {
            Some(p) => read_toml(&p)?,
            None => SupplySetup::default(),
        };
        let facilities = match optional("facilities.toml")? {
            Some(p) => read_toml(&p)?,
            None => FacilitiesSetup::default(),
        };
        let raw = |name: &str| -> Result<toml::Table, ContentError> {
            match optional(name)? {
                Some(p) => read_toml(&p),
                None => Ok(toml::Table::new()),
            }
        };
        Ok(ScenarioContent {
            dir: dir.to_path_buf(),
            meta: main.scenario,
            initiative: main.initiative,
            victory: main.victory,
            land,
            air,
            supply,
            facilities,
            construction: raw("construction.toml")?,
            fleet: raw("fleet.toml")?,
            arrivals: raw("arrivals.toml")?,
        })
    }

    /// Check every unit, sheet and aircraft reference against the units content.
    pub fn check(&self, units: &UnitsContent) -> Result<(), ContentError> {
        let invalid = |message: String| ContentError::Invalid {
            path: self.dir.clone(),
            message,
        };
        for file in &self.land {
            for group in &file.groups {
                for entry in &group.units {
                    let ids = entry
                        .unit
                        .iter()
                        .chain(&entry.att)
                        .chain(&entry.assg)
                        .chain(&entry.less)
                        .chain(&entry.det)
                        .chain(&entry.consists_of);
                    for id in ids {
                        if !units.units.contains_key(id) {
                            return Err(invalid(format!("group {}: unknown unit {id}", group.id)));
                        }
                    }
                    if let Some(sheet) = &entry.sheet
                        && !units.sheets.contains_key(sheet)
                    {
                        return Err(invalid(format!(
                            "group {}: unknown sheet {sheet}",
                            group.id
                        )));
                    }
                    if entry.unit.is_none() && entry.sheet.is_none() {
                        return Err(invalid(format!("group {}: entry with no unit", group.id)));
                    }
                }
            }
        }
        for force in &self.air {
            let planes = force
                .planes
                .iter()
                .chain(force.malta.iter().flat_map(|m| &m.planes));
            for plane in planes {
                if !units.aircraft.contains_key(&plane.aircraft) {
                    return Err(invalid(format!("unknown aircraft {}", plane.aircraft)));
                }
            }
        }
        Ok(())
    }
}
