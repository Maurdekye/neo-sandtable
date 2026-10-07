//! A scenario's starting state: clock, initiative, victory conditions, and where every unit,
//! plane, truck, dump and facility stands at the start.
//!
//! Schema: `data/scenarios/README.md` (owned by the `oob` area). Unit ids refer to
//! [`crate::units`]; [`ScenarioContent::check`] verifies those references. Files whose systems the
//! engine does not model yet (fleet, arrivals) are kept as raw TOML tables until
//! their owners type them.

pub mod construction;
pub mod fleet;
use construction::ScenarioConstruction;
use fleet::FleetLogistics;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cna_core::ids::{HexId, UnitId};
use cna_protocol::Side;
use serde::{Deserialize, Serialize};

use crate::units::{Trucks, UnitsContent};
use crate::{ContentError, read_toml};

/// Everything in one `data/scenarios/<id>/` folder.
#[derive(Debug, Clone)]
pub struct ScenarioContent {
    pub dir: PathBuf,
    pub meta: ScenarioMeta,
    pub initiative: InitiativeSetup,
    pub victory: Vec<VictoryLevel>,
    pub victory_points: Option<toml::Table>,
    /// One per side file (`land_axis.toml`, `land_cw.toml`).
    pub land: Vec<LandDeployment>,
    /// One per side file (`air_axis.toml`, `air_cw.toml`).
    pub air: Vec<AirForce>,
    pub supply: SupplySetup,
    pub facilities: FacilitiesSetup,
    pub construction: ScenarioConstruction,
    pub fleet: toml::Table,
    pub fleet_logistics: FleetLogistics,
    pub arrivals: toml::Table,
}

/// A point in the game clock: game-turn and OpStage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    pub setup_from: Option<String>,
    #[serde(default)]
    pub setup_files: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub composition_exception_with: Vec<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

/// Pilots by quality rating.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Stable source identity when the scenario supplies one.
    #[serde(default)]
    pub id: Option<String>,
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
    pub location: Option<String>,
    pub location_area: Option<String>,
    /// A separate off-map theatre, e.g. Malta; absent means the main force.
    pub theatre: Option<String>,
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
    victory_points: Option<toml::Table>,
}

impl ScenarioContent {
    /// Load a `data/scenarios/<id>` folder.
    pub fn load(dir: &Path) -> Result<Self, ContentError> {
        let main: ScenarioFile = read_toml(&dir.join("scenario.toml"))?;
        // Resolve one level of reuse explicitly: the authoritative metadata remains local.
        let mut paths = BTreeMap::<String, PathBuf>::new();
        let valid_name = |name: &str| {
            Path::new(name).components().count() == 1
                && !matches!(name, "." | "..")
                && !name.contains(['/', '\\'])
        };
        let invalid = |message| ContentError::Invalid {
            path: dir.to_path_buf(),
            message,
        };
        if let Some(from) = &main.scenario.setup_from {
            if !valid_name(from) {
                return Err(invalid("invalid setup_from folder".into()));
            }
            let base = dir.parent().unwrap_or(dir).join(from);
            for name in &main.scenario.setup_files {
                if !valid_name(name) {
                    return Err(invalid("invalid setup filename".into()));
                }
                let path = base.join(name);
                if !path.exists() {
                    return Err(invalid(format!("missing reused setup file {name}")));
                }
                paths.insert(name.clone(), path);
            }
        }
        for name in &main.scenario.files {
            if !valid_name(name) {
                return Err(invalid("invalid scenario filename".into()));
            }
            let path = dir.join(name);
            if !path.exists() {
                return Err(invalid(format!("missing scenario file {name}")));
            }
            paths.insert(name.clone(), path);
        }
        let resolve = |name: &str| paths.get(name).cloned().unwrap_or_else(|| dir.join(name));
        let mut land = Vec::new();
        let mut air = Vec::new();
        for side in ["axis", "cw"] {
            let path = resolve(&format!("land_{side}.toml"));
            if path.exists() {
                land.push(read_toml(&path)?);
            }
            let path = resolve(&format!("air_{side}.toml"));
            if path.exists() {
                air.push(read_toml(&path)?);
            }
        }
        let optional = |name: &str| -> Result<Option<PathBuf>, ContentError> {
            let path = resolve(name);
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
        let construction = match optional("construction.toml")? {
            Some(p) => ScenarioConstruction::load(&p)?,
            None => ScenarioConstruction::default(),
        };
        let raw = |name: &str| -> Result<toml::Table, ContentError> {
            match optional(name)? {
                Some(p) => read_toml(&p),
                None => Ok(toml::Table::new()),
            }
        };
        let fleet_logistics = match optional("fleet.toml")? {
            Some(p) => read_toml(&p)?,
            None => FleetLogistics::default(),
        };
        Ok(ScenarioContent {
            dir: dir.to_path_buf(),
            meta: main.scenario,
            initiative: main.initiative,
            victory: main.victory,
            victory_points: main.victory_points,
            land,
            air,
            supply,
            facilities,
            construction,
            fleet: raw("fleet.toml")?,
            fleet_logistics,
            arrivals: raw("arrivals.toml")?,
        })
    }

    /// Check every unit, sheet and aircraft reference against the units content.
    pub fn check(&self, units: &UnitsContent) -> Result<(), ContentError> {
        let invalid = |message: String| ContentError::Invalid {
            path: self.dir.clone(),
            message,
        };
        if let Some(setup) = &self.fleet_logistics.axis_convoys {
            let lanes: std::collections::BTreeSet<_> =
                setup.lanes_allowed.iter().copied().collect();
            if lanes.is_empty()
                || lanes.len() != setup.lanes_allowed.len()
                || lanes.iter().any(|n| !(1..=6).contains(n))
                || setup.src.is_empty()
            {
                return Err(invalid(
                    "Axis convoy lanes need unique values1-6 and a citation".into(),
                ));
            }
        }
        if let Some(setup) = &self.fleet_logistics.axis_coastal_shipping
            && (!units.coastal_rosters.contains_key(&setup.roster) || setup.src.is_empty())
        {
            return Err(invalid(format!(
                "unknown or uncited coastal roster under data/units: {}",
                setup.roster
            )));
        }
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
            for planes in std::iter::once(force.planes.as_slice())
                .chain(force.malta.iter().map(|m| m.planes.as_slice()))
            {
                let mut ids = std::collections::BTreeSet::new();
                for plane in planes {
                    if !units.aircraft.contains_key(&plane.aircraft) {
                        return Err(invalid(format!("unknown aircraft {}", plane.aircraft)));
                    }
                    if !ids.insert(&plane.aircraft)
                        || plane.total < 0
                        || plane.ready.is_some_and(|n| n < 0 || n > plane.total)
                        || plane.sgsu.is_some_and(|n| n < 0)
                    {
                        return Err(invalid(format!(
                            "invalid initial aircraft counts or duplicate {}",
                            plane.aircraft
                        )));
                    }
                    let mut exceptions = std::collections::BTreeSet::new();
                    for id in &plane.composition_exception_with {
                        if id == &plane.aircraft
                            || !exceptions.insert(id)
                            || !planes.iter().any(|other| {
                                &other.aircraft == id
                                    && other.composition_exception_with.contains(&plane.aircraft)
                            })
                        {
                            return Err(invalid(format!(
                                "{}: composition exceptions must be unique and symmetric within the force",
                                plane.aircraft
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod reuse_tests {
    use super::*;

    /// Cases: scen:60.23, scen:60.82
    #[test]
    fn italian_campaign_reuses_setup_but_keeps_own_clock_and_victory() {
        let base = crate::repo_data_dir().join("scenarios");
        let graziani = ScenarioContent::load(&base.join("graziani")).unwrap();
        let italian = ScenarioContent::load(&base.join("italian_campaign")).unwrap();
        assert_eq!(italian.land, graziani.land);
        assert_eq!(italian.air, graziani.air);
        assert_eq!(italian.supply, graziani.supply);
        assert_eq!(italian.facilities, graziani.facilities);
        assert_eq!(italian.meta.end.gt, 20);
        assert_eq!(graziani.meta.end.gt, 6);
        assert!(
            italian
                .victory_points
                .as_ref()
                .unwrap()
                .contains_key("place")
        );
        assert!(graziani.victory_points.is_none());
        assert_ne!(italian.arrivals, graziani.arrivals);
        let units = UnitsContent::load(&crate::repo_data_dir().join("units")).unwrap();
        italian.check(&units).unwrap();
    }
}
