//! Repair charts: attempted-point costs, selection budgets and separate repair outcomes.
//! The caller applies eligibility, corrected die modifiers and prepaid resource checks.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::ranges::{IntRange, check_int_tiling};
use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairLocation {
    Field,
    Facility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum VehicleState {
    #[serde(rename = "bd")]
    BrokenDown,
    #[serde(rename = "de")]
    Destroyed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairVehicle {
    Truck,
    ArmoredCar,
    Recce,
    Tank,
    SelfPropelledArtillery,
    TankDestroyer,
    Gun,
}

const VEHICLES: [RepairVehicle; 7] = [
    RepairVehicle::Truck,
    RepairVehicle::ArmoredCar,
    RepairVehicle::Recce,
    RepairVehicle::Tank,
    RepairVehicle::SelfPropelledArtillery,
    RepairVehicle::TankDestroyer,
    RepairVehicle::Gun,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct RepairSupplies {
    pub fuel: i32,
    pub stores: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairCost {
    /// Per point attempted, independent of success or failure.
    pub supplies: RepairSupplies,
    /// The field destroyed-tank row requires the source-authorized special repair organization.
    pub special_field_tank_organization: bool,
}

#[derive(Deserialize)]
struct CostRow {
    location: RepairLocation,
    state: VehicleState,
    vehicle_types: Vec<String>,
    supplies: RepairSupplies,
    #[serde(default)]
    footnotes: Vec<String>,
}

#[derive(Deserialize)]
struct CostBody {
    row: Vec<CostRow>,
}

#[derive(Debug, Clone)]
pub struct VehicleRepairSupplyCosts {
    costs: BTreeMap<(RepairLocation, VehicleState, RepairVehicle), RepairCost>,
}

impl Bound for VehicleRepairSupplyCosts {
    const ID: &'static str = "land.22.15.vehicle_repair_supply_costs";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: CostBody = raw.deserialize()?;
        let mut costs = BTreeMap::new();
        for (i, row) in body.row.into_iter().enumerate() {
            let field = |name: &str| format!("row[{i}].{name}");
            if row.supplies.fuel < 0 || row.supplies.stores < 0 {
                return Err(raw.err(field("supplies"), "costs must be nonnegative"));
            }
            let special =
                row.location == RepairLocation::Field && row.state == VehicleState::Destroyed;
            if row.footnotes != if special { vec!["star"] } else { vec![] } {
                return Err(raw.err(
                    field("footnotes"),
                    "only the field destroyed-tank row has star",
                ));
            }
            let vehicles = if row.vehicle_types == ["all"] {
                if row.location != RepairLocation::Facility || row.state != VehicleState::BrokenDown
                {
                    return Err(raw.err(
                        field("vehicle_types"),
                        "all applies only to facility breakdown repair",
                    ));
                }
                VEHICLES.to_vec()
            } else {
                row.vehicle_types
                    .iter()
                    .map(|name| match name.as_str() {
                        "truck" => Ok(RepairVehicle::Truck),
                        "armored_car" => Ok(RepairVehicle::ArmoredCar),
                        "recce" => Ok(RepairVehicle::Recce),
                        "tank" => Ok(RepairVehicle::Tank),
                        "self_propelled_artillery" => Ok(RepairVehicle::SelfPropelledArtillery),
                        "tank_destroyer" => Ok(RepairVehicle::TankDestroyer),
                        "gun" => Ok(RepairVehicle::Gun),
                        _ => {
                            Err(raw.err(field("vehicle_types"), format!("unknown vehicle {name}")))
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?
            };
            if vehicles.is_empty() {
                return Err(raw.err(field("vehicle_types"), "at least one vehicle is required"));
            }
            for vehicle in vehicles {
                let key = (row.location, row.state, vehicle);
                if costs
                    .insert(
                        key,
                        RepairCost {
                            supplies: row.supplies,
                            special_field_tank_organization: special,
                        },
                    )
                    .is_some()
                {
                    return Err(raw.err(field("vehicle_types"), "duplicate repair combination"));
                }
            }
        }
        let mut expected = BTreeSet::new();
        for vehicle in VEHICLES {
            expected.insert((RepairLocation::Facility, VehicleState::BrokenDown, vehicle));
            if vehicle != RepairVehicle::Gun {
                expected.insert((RepairLocation::Field, VehicleState::BrokenDown, vehicle));
            }
        }
        for location in [RepairLocation::Field, RepairLocation::Facility] {
            expected.insert((location, VehicleState::Destroyed, RepairVehicle::Tank));
        }
        if costs.keys().copied().collect::<BTreeSet<_>>() != expected {
            return Err(raw.err(
                "row",
                "repair combinations must match the chart; unlisted combinations are unavailable",
            ));
        }
        Ok(Self { costs })
    }
}

impl VehicleRepairSupplyCosts {
    /// A listed cost is not a grant of eligibility; the caller verifies the special organization.
    /// Cases: land:22.23, land:22.24, land:22.26, land:22.35, land:22.42
    pub fn cost(
        &self,
        location: RepairLocation,
        state: VehicleState,
        vehicle: RepairVehicle,
    ) -> Option<RepairCost> {
        self.costs.get(&(location, state, vehicle)).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum TankRepairOutcome {
    #[serde(rename = "R")]
    Repaired,
    #[serde(rename = "J")]
    Junked,
    #[serde(rename = "none")]
    NoEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestroyedTankColumn {
    Field,
    AxisFacilityGerman,
    AxisFacilityItalian,
    AxisFacilityCommonwealth,
    CommonwealthFacility,
}

#[derive(Deserialize, Debug, Clone)]
struct AxisOutcomes {
    german: TankRepairOutcome,
    italian: TankRepairOutcome,
    commonwealth: TankRepairOutcome,
}

#[derive(Deserialize, Debug, Clone)]
struct DestroyedRow {
    die_min: i32,
    die_max: i32,
    field: TankRepairOutcome,
    axis_facility: AxisOutcomes,
    cw_facility_all_nationalities: TankRepairOutcome,
}

#[derive(Deserialize)]
struct DestroyedBody {
    row: Vec<DestroyedRow>,
}

#[derive(Debug, Clone)]
pub struct DestroyedTanksRepair {
    rows: Vec<DestroyedRow>,
}

fn validate_rolls(
    raw: &RawTable,
    rows: impl Iterator<Item = (i32, i32)>,
    lo: i32,
    hi: i32,
) -> Result<(), TableError> {
    let spans = rows
        .enumerate()
        .map(|(i, (min, max))| {
            if min < lo || max > hi || min > max {
                Err(raw.err(
                    format!("row[{i}].die"),
                    format!("ordered bounds within {lo}..={hi} required"),
                ))
            } else {
                Ok(IntRange::new(min, max))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    check_int_tiling(spans, lo, hi).map_err(|e| raw.err("row.die", e))
}

impl Bound for DestroyedTanksRepair {
    const ID: &'static str = "land.22.44.destroyed_tanks_repair";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: DestroyedBody = raw.deserialize()?;
        validate_rolls(raw, body.row.iter().map(|r| (r.die_min, r.die_max)), 1, 7)?;
        Ok(Self { rows: body.row })
    }
}

impl DestroyedTanksRepair {
    /// One outcome per attempted tank point; `adjusted_die` includes the minor-facility modifier.
    /// The field column still requires the authorized special organization and tank nationality.
    /// Cases: land:22.41, land:22.43, land:22.44, land:22.6, land:22.7
    pub fn outcome(
        &self,
        column: DestroyedTankColumn,
        adjusted_die: i32,
    ) -> Option<TankRepairOutcome> {
        let row = self
            .rows
            .iter()
            .find(|r| (r.die_min..=r.die_max).contains(&adjusted_die))?;
        Some(match column {
            DestroyedTankColumn::Field => row.field,
            DestroyedTankColumn::AxisFacilityGerman => row.axis_facility.german,
            DestroyedTankColumn::AxisFacilityItalian => row.axis_facility.italian,
            DestroyedTankColumn::AxisFacilityCommonwealth => row.axis_facility.commonwealth,
            DestroyedTankColumn::CommonwealthFacility => row.cw_facility_all_nationalities,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairPercent {
    percent: i32,
    singleton_zero: bool,
}

impl RepairPercent {
    /// Cases: land:22.8
    pub fn percent(self) -> i32 {
        self.percent
    }
    /// Cases: land:22.8
    pub fn singleton_zero(self) -> bool {
        self.singleton_zero
    }

    /// Integer ceiling of the printed percentage, with the starred singleton exception.
    /// Cases: land:22.25, land:22.34, land:22.8
    pub fn repaired_points(self, attempted: i32) -> Option<i32> {
        if attempted < 0 {
            return None;
        }
        if attempted == 1 && self.singleton_zero {
            return Some(0);
        }
        i32::try_from((i64::from(attempted) * i64::from(self.percent) + 99) / 100).ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokenRepairResult {
    /// Budget in light/medium points: a heavy point consumes two. The owner selects the points.
    TruckSelectionBudget(i32),
    ArmoredCarReccePoints(i32),
    Percentage(RepairPercent),
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairFacility {
    Temporary,
    Major,
}

#[derive(Deserialize)]
struct FieldRepair {
    truck: Option<i32>,
    armored_car_recce: Option<i32>,
    tank_spa_td_percent: Option<i32>,
    #[serde(default)]
    tank_spa_td_star: bool,
    #[serde(default)]
    not_applicable: bool,
}

#[derive(Deserialize)]
struct FacilityRepair {
    temporary_percent: i32,
    #[serde(default)]
    temporary_star: bool,
    major_percent: i32,
}

#[derive(Deserialize)]
struct BrokenRow {
    die_min: i32,
    die_max: i32,
    field: FieldRepair,
    facility: FacilityRepair,
}

#[derive(Deserialize)]
struct BrokenBody {
    row: Vec<BrokenRow>,
}

#[derive(Debug, Clone)]
struct BoundBrokenRow {
    die: IntRange,
    truck: BrokenRepairResult,
    armored: BrokenRepairResult,
    tank: BrokenRepairResult,
    temporary: RepairPercent,
    major: RepairPercent,
}

#[derive(Debug, Clone)]
pub struct BrokenDownVehicleRepair {
    rows: Vec<BoundBrokenRow>,
}

fn percent(
    raw: &RawTable,
    field: String,
    value: i32,
    star: bool,
) -> Result<RepairPercent, TableError> {
    if ![0, 10, 25, 33, 50, 75].contains(&value) || (star && value != 10) {
        return Err(raw.err(
            field,
            "printed percentage required; star applies only to ten percent",
        ));
    }
    Ok(RepairPercent {
        percent: value,
        singleton_zero: star,
    })
}

impl Bound for BrokenDownVehicleRepair {
    const ID: &'static str = "land.22.8.broken_down_vehicle_repair";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: BrokenBody = raw.deserialize()?;
        validate_rolls(raw, body.row.iter().map(|r| (r.die_min, r.die_max)), 0, 8)?;
        let mut rows = Vec::new();
        for (i, row) in body.row.into_iter().enumerate() {
            let f = row.field;
            let (truck, armored, tank) = if f.not_applicable {
                if f.truck.is_some()
                    || f.armored_car_recce.is_some()
                    || f.tank_spa_td_percent.is_some()
                    || f.tank_spa_td_star
                    || row.die_min < 7
                {
                    return Err(raw.err(
                        format!("row[{i}].field"),
                        "not-applicable is exclusive and only covers seven/eight",
                    ));
                }
                (
                    BrokenRepairResult::NotApplicable,
                    BrokenRepairResult::NotApplicable,
                    BrokenRepairResult::NotApplicable,
                )
            } else {
                let (Some(truck), Some(armored), Some(tank)) =
                    (f.truck, f.armored_car_recce, f.tank_spa_td_percent)
                else {
                    return Err(raw.err(
                        format!("row[{i}].field"),
                        "all three field columns are required",
                    ));
                };
                if !(0..=2).contains(&truck) || !(0..=1).contains(&armored) || row.die_max > 6 {
                    return Err(raw.err(
                        format!("row[{i}].field"),
                        "printed selection budgets required for zero through six",
                    ));
                }
                (
                    BrokenRepairResult::TruckSelectionBudget(truck),
                    BrokenRepairResult::ArmoredCarReccePoints(armored),
                    BrokenRepairResult::Percentage(percent(
                        raw,
                        format!("row[{i}].field.tank_spa_td_percent"),
                        tank,
                        f.tank_spa_td_star,
                    )?),
                )
            };
            rows.push(BoundBrokenRow {
                die: IntRange::new(row.die_min, row.die_max),
                truck,
                armored,
                tank,
                temporary: percent(
                    raw,
                    format!("row[{i}].facility.temporary_percent"),
                    row.facility.temporary_percent,
                    row.facility.temporary_star,
                )?,
                major: percent(
                    raw,
                    format!("row[{i}].facility.major_percent"),
                    row.facility.major_percent,
                    false,
                )?,
            });
        }
        Ok(Self { rows })
    }
}

impl BrokenDownVehicleRepair {
    /// Already adjusted roll; the procedure applies land:22.34 and the land:22.8 correction.
    /// An unlisted field vehicle is distinct from a printed not-applicable roll.
    /// Cases: land:22.23, land:22.24, land:22.25, land:22.8
    pub fn field_result(
        &self,
        vehicle: RepairVehicle,
        adjusted_die: i32,
    ) -> Option<BrokenRepairResult> {
        let row = self.rows.iter().find(|r| r.die.contains(adjusted_die))?;
        match vehicle {
            RepairVehicle::Truck => Some(row.truck),
            RepairVehicle::ArmoredCar | RepairVehicle::Recce => Some(row.armored),
            RepairVehicle::Tank
            | RepairVehicle::SelfPropelledArtillery
            | RepairVehicle::TankDestroyer => Some(row.tank),
            RepairVehicle::Gun => None,
        }
    }

    /// Already adjusted roll; facility eligibility and corrected modifiers remain with the caller.
    /// Cases: land:22.34, land:22.8
    pub fn facility_percent(
        &self,
        facility: RepairFacility,
        adjusted_die: i32,
    ) -> Option<RepairPercent> {
        let row = self.rows.iter().find(|r| r.die.contains(adjusted_die))?;
        Some(match facility {
            RepairFacility::Temporary => row.temporary,
            RepairFacility::Major => row.major,
        })
    }
}
