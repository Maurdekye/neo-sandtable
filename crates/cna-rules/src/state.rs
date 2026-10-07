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
    /// Private engineering commitments and observed stage activity.
    #[serde(default)]
    pub engineering: crate::land::engineering::EngineeringState,
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
#[serde(default)]
pub struct WellState {
    pub depleted: bool,
    pub poisoned: bool,
    pub depleted_known: BTreeSet<Side>,
    pub poisoned_known: BTreeSet<Side>,
    pub depleted_revealed: bool,
    pub poisoned_revealed: bool,
    pub poison_failed_stage: BTreeMap<Side, crate::logistics::water::WaterStage>,
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

/// Assignment reserves an organization slot even when the unit is detached.
/// Printed is distinct from explicit independence and survives old checkpoints.
/// Cases: land:19.11, land:19.13, land:19.14
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "parent", rename_all = "snake_case")]
pub enum Assignment {
    #[default]
    Printed,
    Independent,
    Parent(UnitId),
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
    #[serde(default)]
    pub assignment: Assignment,
    /// Current TOE: starts as printed on the OA sheet / set-up.
    pub toe: Option<Toe>,
    /// Capability expenditure this OpStage, in quarter CP (`land:6`).
    pub cp_spent_quarters: i32,
    #[serde(default)]
    pub reserve: crate::land::reserve::ReserveState,
    #[serde(default)]
    pub engaged: bool,
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
    /// Owner-private cargo handling at an off-map supply box (land:8.88).
    #[serde(default)]
    pub box_handling: Option<crate::logistics::box_handling::BoxHandling>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LandState {
    /// Owner-private off-map journeys and physical Transit group membership.
    #[serde(default)]
    pub off_map: crate::land::offmap::OffMapState,
    /// Symmetric truthful links; unit.engaged is the owner-facing derived status.
    #[serde(default)]
    pub engagements: BTreeMap<UnitId, BTreeSet<UnitId>>,
    /// Applied schedule rows and owner decisions; supply delivery waits for this window.
    #[serde(default)]
    pub arrivals: crate::land::arrivals::ArrivalState,
    #[serde(default)]
    pub breakdown: crate::land::breakdown::BreakdownState,
    #[serde(default)]
    pub reaction: crate::land::reaction::ReactionState,
    /// Public target-hex assault announcements, retained until the OpStage ends.
    #[serde(default)]
    pub assault_intentions: BTreeMap<UnitId, BTreeSet<HexId>>,
    #[serde(default)]
    pub combat: crate::land::combat::CombatState,
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
    /// Opaque public counter label, independent of internal identity and kind.
    #[serde(default)]
    pub marker: String,
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
    /// Exact newly arrived units, closed batches and unresolved well operations.
    #[serde(default)]
    pub arrival_supply: crate::logistics::arrivals::ArrivalSupplyWindow,
    /// Private ration and water history; absent entries have not been supplied.
    #[serde(default)]
    pub rations: BTreeMap<UnitId, crate::logistics::Rations>,
    #[serde(default)]
    pub attrition_started_stage: Option<crate::logistics::water::WaterStage>,
    #[serde(default)]
    pub food_losses: Vec<crate::logistics::attrition::FoodLoss>,
    #[serde(default)]
    pub attrition_window: crate::logistics::attrition::AttritionWindow,
    #[serde(default)]
    pub allocation_batches: crate::logistics::batches::AllocationBatches,
    /// Captured infantry and their separately formed guard points.
    #[serde(default)]
    pub prisoners: BTreeMap<String, crate::logistics::PrisonerGroup>,
    #[serde(default)]
    pub stores_started_gt: Option<u16>,
    /// Movement fuel already charged in this unit's current segment.
    #[serde(default)]
    pub fuel_segments: BTreeMap<UnitId, crate::logistics::FuelSegmentLedger>,
    /// Original moving groups retain a single source rounding account after truck splits.
    #[serde(default)]
    pub fuel_accounts: BTreeMap<UnitId, crate::logistics::FuelFundingAccount>,
    /// Separate carrier accounts: pool identities never masquerade as land units.
    #[serde(default)]
    pub pool_fuel_segments: BTreeMap<String, crate::logistics::FuelSegmentLedger<String>>,
    #[serde(default)]
    pub pool_fuel_accounts: BTreeMap<String, crate::logistics::FuelFundingAccount>,
    /// Owner-private loads retain their first carrier's CP allowance across handling.
    #[serde(default)]
    pub cargo_history: crate::logistics::cargo_history::CargoHistoryState,
    #[serde(default)]
    pub wells: BTreeMap<HexId, WellState>,
    /// Pipeline connectivity and destroyed status; construction procedures own updates.
    #[serde(default)]
    pub pipelines: BTreeMap<HexId, crate::logistics::wells::PipelineHex>,
    /// Verified currently operating CW railroad water hexes; unknown routes are not filled.
    #[serde(default)]
    pub operating_rail_water: BTreeSet<HexId>,
    /// Draw results await immediate owner allocation; never available to movement sources.
    #[serde(default)]
    pub water_window: crate::logistics::batches::WaterWindow,
    #[serde(default)]
    pub drawn_water: BTreeMap<UnitId, crate::logistics::wells::DrawnWater>,
    /// Unit tanks, ready ammunition and first-line cargo (airlog:49-53).
    /// Absent entries mean empty holdings; ratings remain in content.
    #[serde(default)]
    pub unit_supply: BTreeMap<UnitId, UnitSupply>,
    pub dumps: BTreeMap<String, Dump>,
    #[serde(default)]
    pub next_dump_marker: u64,
    #[serde(default)]
    pub dump_markers_initialized: bool,
    #[serde(default)]
    pub ports: BTreeMap<String, crate::logistics::ports::PortState>,
    #[serde(default)]
    pub bizerta_open: bool,
    #[serde(default)]
    pub bizerta_roll_gt: Option<u16>,
    #[serde(default)]
    pub convoys_initialized: bool,
    #[serde(default)]
    pub convoy_turns: BTreeMap<u16, crate::logistics::convoys::ConvoyTurn>,
    #[serde(default)]
    pub convoy_planning_queue: Vec<u16>,
    #[serde(default)]
    pub coastal_ships: BTreeMap<String, crate::logistics::coastal::CoastalShipState>,
    #[serde(default)]
    pub coastal_loading_closed_stage: Option<crate::logistics::water::WaterStage>,
    /// Supplies freely distributable among a side's airfields (`scen:60.34`, `scen:60.44`).
    pub air_supply_pool: BTreeMap<Side, Supplies>,
    /// Second- and third-line truck pools at set-up, by side, with their placement.
    pub truck_pools: Vec<TruckPool>,
    /// Per-side monotonic pool serials; checkpoints preserve identity allocation.
    #[serde(default)]
    pub truck_pool_serial: BTreeMap<Side, u64>,
    /// Includes retired pools so a removed id cannot be assigned again.
    #[serde(default)]
    pub truck_pool_ids: BTreeSet<String>,
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
    pub id: String,
    pub side: Side,
    /// Immutable starting placement, retained as provenance after setup.
    pub placement: Placement,
    /// Current resolved location; None leaves placement to the owning setup seat.
    #[serde(default)]
    pub location: Option<Location>,
    pub trucks: Trucks,
    /// Cargo stays with this identity through placement, movement and removal.
    #[serde(default)]
    pub cargo: Supplies,
    /// Vehicle tanks and activity reserve are independent of cargo.
    #[serde(default)]
    pub tank_fuel: FuelTenths,
    #[serde(default)]
    pub activity_water: WaterPoints,
    #[serde(default)]
    pub box_handling: Option<crate::logistics::box_handling::BoxHandling>,
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
    #[serde(default)]
    pub fuelled: i32,
    #[serde(default)]
    pub armed: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AirState {
    /// Imported individual records; see air::inventory for the mirror invariant.
    #[serde(default)]
    pub runtime: crate::air::state::AirRuntime,
    /// Keyed by `axis`, `commonwealth`, or `malta`.
    pub forces: BTreeMap<String, AirForce>,
    #[serde(default)]
    pub squadrons: BTreeMap<String, AirSquadron>,
    #[serde(default)]
    pub squadron_serial: BTreeMap<String, u64>,
}

/// A placed SGSU with its stable squadron composition and pilot roster.
/// Cases: airlog:35.1, airlog:35.2, scen:59.3
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirSquadron {
    pub id: String,
    pub force: String,
    pub side: Side,
    pub nationality: String,
    pub facility: String,
    /// Scenario-prescribed type, when the initial SGSU row fixes its composition.
    pub initial_aircraft: Option<String>,
    pub planes: BTreeMap<String, PlaneCount>,
    pub pilots: BTreeMap<u8, i32>,
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
    /// Decisions opened so far, per seat. Ids count per seat (`axis.front_line-3`) so that a
    /// seat's ids reveal nothing about how many decisions other seats, above all the enemy's,
    /// were given (land:3.6).
    #[serde(default)]
    pub opened: BTreeMap<SeatId, u32>,
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
                            assignment: Assignment::default(),
                            toe: oa.toe.clone(),
                            cp_spent_quarters: 0,
                            reserve: crate::land::reserve::ReserveState::default(),
                            engaged: false,
                            voluntary_cp_quarters: 0,
                            cohesion_quarters: 0,
                            no_idle_recovery: false,
                            detached: false,
                            setup_group: Some(group.id.clone()),
                            trucks: Trucks::default(),
                            transport_trucks: Trucks::default(),
                            box_handling: None,
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
                assignment: Assignment::default(),
                toe: oa.toe.clone(),
                cp_spent_quarters: 0,
                reserve: crate::land::reserve::ReserveState::default(),
                engaged: false,
                voluntary_cp_quarters: 0,
                cohesion_quarters: 0,
                no_idle_recovery: false,
                detached: false,
                setup_group: None,
                trucks: Trucks::default(),
                transport_trucks: Trucks::default(),
                box_handling: None,
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
                    marker: String::new(),
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
                        marker: String::new(),
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
            let location = match &t.placement {
                Placement::Hex { hex } => Some(Location::Hex {
                    hex: content
                        .map
                        .canonical(hex)
                        .ok_or_else(|| format!("truck pool: unknown hex {hex}"))?
                        .clone(),
                }),
                Placement::City { city } if is_box(city) => Some(Location::OffMap {
                    id: format!("box_{city}"),
                }),
                _ => None,
            };
            crate::logistics::pools::add_truck_pool(
                &mut logistics,
                t.id.as_deref(),
                t.side,
                t.placement.clone(),
                location,
                t.trucks,
                Supplies::default(),
            )?;
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

        let logistics_pool_sources = logistics
            .truck_pools
            .iter()
            .enumerate()
            .map(|(source, p)| (p.id.clone(), source))
            .collect();
        Ok(State {
            cursor: Cursor::start(&content.bounds),
            turn: TurnState::default(),
            land,
            logistics,
            air,
            engineering: crate::land::engineering::EngineeringState::default(),
            decisions: Decisions::default(),
            setup: crate::setup::SetupState {
                pool_sources: logistics_pool_sources,
                ..crate::setup::SetupState::default()
            },
            result: None,
        })
    }

    pub fn units_of(&self, side: Side) -> impl Iterator<Item = &LandUnit> {
        self.land.units.values().filter(move |u| u.side == side)
    }

    /// All land counter presence, including broken vehicles, without disclosing contents.
    pub fn stack_presence(&self, hex: &HexId, side: Side) -> bool {
        self.units_of(side).any(|u| u.location.hex() == Some(hex))
            || self
                .land
                .breakdown
                .markers
                .values()
                .any(|m| m.side == side && &m.hex == hex)
    }
    /// Units on the map, grouped by hex and side, in id order.
    pub fn stacks(&self) -> BTreeMap<(HexId, Side), Vec<&LandUnit>> {
        let mut out: BTreeMap<(HexId, Side), Vec<&LandUnit>> = BTreeMap::new();
        for u in self.land.units.values() {
            if let Some(hex) = u.location.hex() {
                out.entry((hex.clone(), u.side)).or_default().push(u);
            }
        }
        for m in self.land.breakdown.markers.values() {
            out.entry((m.hex.clone(), m.side)).or_default();
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
        entry.fuelled += plane.total;
        entry.armed += plane.total;
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
