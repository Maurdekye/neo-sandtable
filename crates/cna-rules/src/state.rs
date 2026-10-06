//! The authoritative world state of a CNA campaign, and its construction from a scenario.
//!
//! The state is plain data: cloned per transition, serialized into checkpoints, iterated in a
//! stable order (`BTreeMap`). Each rules area owns a sub-state ([`LandState`],
//! [`LogisticsState`], [`AirState`]) and extends it as its procedures need; shared fields live
//! here. Never store anything derivable from content.

use std::collections::{BTreeMap, BTreeSet};

use cna_content::scenario::{Placement, Supplies};
use cna_content::units::{Toe, Trucks};
use cna_core::decision::{ActionSpace, Secrecy, Trigger};
use cna_core::ids::{DecisionId, HexId, SeatId, UnitId};
use cna_core::quantity::{AmmoPoints, FuelTenths, WaterPoints};
use cna_protocol::Side;
use cna_tables::land::weather::{MapSection, WeatherKind};
use serde::{Deserialize, Serialize};

use crate::content::CnaContent;
use crate::seq::Cursor;

/// The whole dynamic state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub cursor: Cursor,
    pub turn: TurnState,
    pub land: LandState,
    pub logistics: LogisticsState,
    pub air: AirState,
    pub decisions: Decisions,
    /// Owner-private setup choices until the simultaneous window closes.
    #[serde(default)]
    pub setup: crate::setup::SetupState,
    /// Set when the game is over.
    pub result: Option<String>,
}

/// Facts that hold for one game-turn or OpStage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnState {
    /// The side holding the Initiative this game-turn (`land:7.12`).
    pub initiative: Option<Side>,
    /// Player A of the current OpStage (`land:7.11`, `land:7.16`).
    pub player_a: Option<Side>,
    /// Weather rolled for the current OpStage (land:29.1).
    #[serde(default)]
    pub weather: Option<WeatherState>,
}

/// Weather result and the selected storm sections, not a copy of the weather table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeatherState {
    pub kind: WeatherKind,
    pub storm_sections: Vec<MapSection>,
}

/// Dynamic well conditions; depletion is not included in the public weather report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WellState {
    pub depleted: bool,
}

/// Where a land unit is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "at", rename_all = "snake_case")]
pub enum Location {
    /// On the map.
    Hex { hex: HexId },
    /// In an off-map box or location (`data/map/areas.toml` ids, e.g. `box_tripoli`).
    OffMap { id: String },
    /// Deployed at the start, but its exact placement is the owner's set-up choice.
    AwaitingSetup { group: String },
    /// Not yet arrived.
    NotArrived,
    /// Removed from play.
    Eliminated,
}

impl Location {
    pub fn hex(&self) -> Option<&HexId> {
        match self {
            Location::Hex { hex } => Some(hex),
            _ => None,
        }
    }
}

/// One land unit's dynamic state. Static characteristics (class, ratings, OA hierarchy) stay in
/// content and are looked up by id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LandUnit {
    pub id: UnitId,
    pub side: Side,
    pub location: Location,
    /// The parent the unit is attached to, when not with its assigned parent (`land:19`).
    pub attached_to: Option<UnitId>,
    /// Current TOE: starts as printed on the OA sheet / set-up.
    pub toe: Option<Toe>,
    /// Capability expenditure this OpStage, in quarter CP (`land:6`).
    pub cp_spent_quarters: i32,
    /// Voluntary CP in the owning half, separate from reaction/retreat, in quarters.
    #[serde(default)]
    pub voluntary_cp_quarters: i32,
    /// Cohesion in quarters of a point; -104 is the movement-stop threshold.
    #[serde(default)]
    pub cohesion_quarters: i32,
    /// Rail and training suppress idle reorganization.
    #[serde(default)]
    pub no_idle_recovery: bool,
    /// Explicit detachment suppresses the printed parent assignment.
    #[serde(default)]
    pub detached: bool,
    /// The set-up group the unit was deployed with, if any.
    pub setup_group: Option<String>,
    /// First-line truck points with the unit (`airlog:53`); set-up pools start on the group's HQ.
    pub trucks: Trucks,
    /// First-line trucks explicitly allocated to moving this unit, not to supply cargo.
    #[serde(default)]
    pub transport_trucks: Trucks,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LandState {
    pub units: BTreeMap<UnitId, LandUnit>,
    /// Only facts disclosed in the current Movement Segment, not enemy strength.
    #[serde(default)]
    pub movement: crate::land::movement::MovementState,
    /// First-line trucks a set-up group brought, not yet distributed among its units
    /// (`scen:59.42`), by group id.
    pub undistributed_trucks: BTreeMap<String, Trucks>,
}

/// A supply dump (`airlog:54`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dump {
    pub id: String,
    pub side: Side,
    pub location: DumpLocation,
    pub supplies: Supplies,
    pub active: bool,
    pub dummy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "at", rename_all = "snake_case")]
pub enum DumpLocation {
    Hex { hex: HexId },
    OffMap { id: String },
    AwaitingSetup { placement: Placement },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogisticsState {
    /// Private ration and water history; absent entries have not been supplied.
    #[serde(default)]
    pub rations: BTreeMap<UnitId, crate::logistics::Rations>,
    #[serde(default)]
    pub attrition_started_stage: Option<crate::logistics::water::WaterStage>,
    #[serde(default)]
    pub food_losses: Vec<crate::logistics::attrition::FoodLoss>,
    /// Captured infantry and their separately formed guard points.
    #[serde(default)]
    pub prisoners: BTreeMap<String, crate::logistics::PrisonerGroup>,
    #[serde(default)]
    pub stores_started_gt: Option<u16>,
    /// Movement fuel already charged in this unit's current segment.
    #[serde(default)]
    pub fuel_segments: BTreeMap<UnitId, crate::logistics::FuelSegmentLedger>,
    #[serde(default)]
    pub wells: BTreeMap<HexId, WellState>,
    /// Unit tanks, ready ammunition and first-line cargo (airlog:49-53).
    /// Absent entries mean empty holdings; ratings remain in content.
    #[serde(default)]
    pub unit_supply: BTreeMap<UnitId, UnitSupply>,
    pub dumps: BTreeMap<String, Dump>,
    /// Supplies freely distributable among a side's airfields (`scen:60.34`, `scen:60.44`).
    pub air_supply_pool: BTreeMap<Side, Supplies>,
    /// Second- and third-line truck pools at set-up, by side, with their placement.
    pub truck_pools: Vec<TruckPool>,
}

/// Dynamic holdings belonging to one land unit.
///
/// Cases: airlog:49.14, airlog:50.17, airlog:53.1
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitSupply {
    /// Water reserved for vehicle activity; idle vehicles retain their reserve.
    #[serde(default)]
    pub activity_water: WaterPoints,
    /// Fuel already in vehicle tanks, retained exactly in tenths.
    pub tank_fuel: FuelTenths,
    /// Ammunition carried by the firing unit, apart from truck cargo.
    pub ready_ammo: AmmoPoints,
    /// Cargo on this unit's first-line trucks; not vehicle tanks.
    pub carried: Supplies,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruckPool {
    pub side: Side,
    pub placement: Placement,
    pub trucks: Trucks,
}

/// One side's air force (or Malta's): planes by type, pilots and SGSUs (`airlog:34`, `35`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirForce {
    /// Aircraft type id -> (total, ready).
    pub planes: BTreeMap<String, PlaneCount>,
    pub pilots: BTreeMap<u8, i32>,
    pub sgsu_available: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaneCount {
    pub total: i32,
    pub ready: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AirState {
    /// Keyed by `axis`, `commonwealth`, or `malta`.
    pub forces: BTreeMap<String, AirForce>,
}

/// A decision waiting for its seat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    pub id: DecisionId,
    pub seat: SeatId,
    pub kind: String,
    pub summary: String,
    pub rules: Vec<String>,
    pub trigger: Trigger,
    pub secrecy: Secrecy,
    pub space: ActionSpace,
    pub revision: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Decisions {
    pub pending: Vec<Pending>,
    pub next_id: u32,
}

impl State {
    /// The scenario's starting state: every deployed unit placed (or awaiting its owner's set-up
    /// choice), dumps, truck pools and air forces, the cursor before set-up.
    pub fn new(content: &CnaContent) -> Result<Self, String> {
        let mut land = LandState::default();
        let units = &content.units;
        for file in &content.scenario.land {
            for group in &file.groups {
                let location = match &group.placement {
                    Placement::Hex { hex } => {
                        let canonical = content
                            .map
                            .canonical(hex)
                            .ok_or_else(|| format!("group {}: unknown hex {hex}", group.id))?;
                        Location::Hex {
                            hex: canonical.clone(),
                        }
                    }
                    Placement::City { city } if is_box(city) => Location::OffMap {
                        id: format!("box_{city}"),
                    },
                    _ => Location::AwaitingSetup {
                        group: group.id.clone(),
                    },
                };
                let mut group_hq: Option<UnitId> = None;
                for entry in &group.units {
                    let skip: BTreeSet<&UnitId> = entry.less.iter().chain(&entry.det).collect();
                    let mut ids: Vec<(UnitId, Option<UnitId>)> = Vec::new();
                    if let Some(sheet) = &entry.sheet {
                        for u in units.units.values().filter(|u| &u.sheet == sheet) {
                            if u.arrives.is_deployed() {
                                ids.push((u.id.clone(), None));
                            }
                        }
                    } else if let Some(root) = &entry.unit {
                        ids.push((root.clone(), None));
                        if !entry.hq_only {
                            for u in deployed_subtree(content, root, &skip) {
                                ids.push((u, None));
                            }
                        }
                        group_hq.get_or_insert_with(|| root.clone());
                    }
                    for att in &entry.att {
                        ids.push((att.clone(), entry.unit.clone()));
                        for u in deployed_subtree(content, att, &skip) {
                            ids.push((u, None));
                        }
                    }
                    for (id, attached_to) in ids {
                        if skip.contains(&id) {
                            continue;
                        }
                        let oa = units
                            .units
                            .get(&id)
                            .ok_or_else(|| format!("group {}: unknown unit {id}", group.id))?;
                        let unit = LandUnit {
                            id: id.clone(),
                            side: oa.side,
                            location: location.clone(),
                            attached_to,
                            toe: oa.toe.clone(),
                            cp_spent_quarters: 0,
                            voluntary_cp_quarters: 0,
                            cohesion_quarters: 0,
                            no_idle_recovery: false,
                            detached: false,
                            setup_group: Some(group.id.clone()),
                            trucks: Trucks::default(),
                            transport_trucks: Trucks::default(),
                        };
                        if land.units.insert(id.clone(), unit).is_some() {
                            return Err(format!("unit {id} is placed twice (group {})", group.id));
                        }
                    }
                }
                if let Some(trucks) = group.trucks
                    && trucks.total() > 0
                {
                    land.undistributed_trucks.insert(group.id.clone(), trucks);
                }
            }
        }
        // Everything on an OA sheet that is not deployed at the start has not arrived yet.
        for oa in units.units.values() {
            land.units.entry(oa.id.clone()).or_insert_with(|| LandUnit {
                id: oa.id.clone(),
                side: oa.side,
                location: if oa.arrives.is_deployed() {
                    Location::Eliminated
                } else {
                    Location::NotArrived
                },
                attached_to: None,
                toe: oa.toe.clone(),
                cp_spent_quarters: 0,
                voluntary_cp_quarters: 0,
                cohesion_quarters: 0,
                no_idle_recovery: false,
                detached: false,
                setup_group: None,
                trucks: Trucks::default(),
                transport_trucks: Trucks::default(),
            });
        }

        let mut logistics = LogisticsState::default();
        let supply = &content.scenario.supply;
        for d in &supply.dumps {
            let location = match &d.location {
                Placement::Hex { hex } => DumpLocation::Hex {
                    hex: content
                        .map
                        .canonical(hex)
                        .ok_or_else(|| format!("dump {}: unknown hex {hex}", d.id))?
                        .clone(),
                },
                Placement::City { city } if is_box(city) => DumpLocation::OffMap {
                    id: format!("box_{city}"),
                },
                other => DumpLocation::AwaitingSetup {
                    placement: other.clone(),
                },
            };
            logistics.dumps.insert(
                d.id.clone(),
                Dump {
                    id: d.id.clone(),
                    side: d.side,
                    location,
                    supplies: d.supplies,
                    active: d.active,
                    dummy: false,
                },
            );
        }
        for (n, d) in supply.dummy_dumps.iter().enumerate() {
            for k in 0..d.count {
                let id = format!("dummy_{}_{}_{}", side_key(d.side), n + 1, k + 1);
                logistics.dumps.insert(
                    id.clone(),
                    Dump {
                        id,
                        side: d.side,
                        location: DumpLocation::AwaitingSetup {
                            placement: d.location.clone(),
                        },
                        supplies: Supplies::default(),
                        active: false,
                        dummy: true,
                    },
                );
            }
        }
        logistics.air_supply_pool = supply.air_supply_pool.clone();
        for t in &supply.second_third_line_trucks {
            logistics.truck_pools.push(TruckPool {
                side: t.side,
                placement: t.placement.clone(),
                trucks: t.trucks,
            });
        }

        let mut air = AirState::default();
        for force in &content.scenario.air {
            air.forces.insert(
                side_key(force.force.side).to_owned(),
                air_force(
                    force.pilots.as_ref(),
                    &force.planes,
                    force.sgsu.as_ref().map(|s| s.available),
                ),
            );
            if let Some(malta) = &force.malta {
                air.forces.insert(
                    "malta".to_owned(),
                    air_force(
                        malta.pilots.as_ref(),
                        &malta.planes,
                        malta.facility_capacity_sgsu,
                    ),
                );
            }
        }

        Ok(State {
            cursor: Cursor::start(&content.bounds),
            turn: TurnState::default(),
            land,
            logistics,
            air,
            decisions: Decisions::default(),
            setup: crate::setup::SetupState::default(),
            result: None,
        })
    }

    pub fn units_of(&self, side: Side) -> impl Iterator<Item = &LandUnit> {
        self.land.units.values().filter(move |u| u.side == side)
    }

    /// Units on the map, grouped by hex and side, in id order.
    pub fn stacks(&self) -> BTreeMap<(HexId, Side), Vec<&LandUnit>> {
        let mut out: BTreeMap<(HexId, Side), Vec<&LandUnit>> = BTreeMap::new();
        for u in self.land.units.values() {
            if let Some(hex) = u.location.hex() {
                out.entry((hex.clone(), u.side)).or_default().push(u);
            }
        }
        out
    }
}

fn air_force(
    pilots: Option<&cna_content::scenario::Pilots>,
    planes: &[cna_content::scenario::PlaneSetup],
    sgsu: Option<i32>,
) -> AirForce {
    let mut force = AirForce {
        sgsu_available: sgsu.unwrap_or(0),
        ..AirForce::default()
    };
    if let Some(p) = pilots {
        force.pilots.insert(3, p.three);
        force.pilots.insert(2, p.two);
        force.pilots.insert(1, p.one);
    }
    for plane in planes {
        let entry = force.planes.entry(plane.aircraft.clone()).or_default();
        entry.total += plane.total;
        entry.ready += plane.ready.unwrap_or(0);
    }
    force
}

/// The deployed units assigned below `root`, excluding `skip` and everything below them.
fn deployed_subtree(content: &CnaContent, root: &UnitId, skip: &BTreeSet<&UnitId>) -> Vec<UnitId> {
    let mut out = Vec::new();
    for child in content.units.children(root) {
        if skip.contains(&child.id) || !child.arrives.is_deployed() {
            continue;
        }
        out.push(child.id.clone());
        out.extend(deployed_subtree(content, &child.id, skip));
    }
    out
}

/// The off-map holding boxes that a `city` placement can name (`land:8.81`).
fn is_box(city: &str) -> bool {
    matches!(city, "tripoli" | "tripolitania" | "gabes" | "tunis")
}

pub fn side_key(side: Side) -> &'static str {
    match side {
        Side::Axis => "axis",
        Side::Commonwealth => "commonwealth",
    }
}
