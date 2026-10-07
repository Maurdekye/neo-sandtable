//! Truck characteristics, equivalent weights and port capacity (sections 54-55).

use serde::{Deserialize, Serialize};

use super::supply::SupplyType;
use crate::units::Ratio;
use crate::{Bound, RawTable, TableError};

// ---------------------------------------------------------------------------------------------
// 54.2 Truck Characteristics
// ---------------------------------------------------------------------------------------------

/// The three truck types (`airlog:54.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruckType {
    Light,
    Medium,
    Heavy,
}

/// Characteristics of one Truck Point (ten trucks) of a type. A field the chart marks
/// `na` (not allowed) is `None`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TruckCharacteristics {
    pub truck_type: TruckType,
    /// CPA when motorizing infantry.
    pub cpa_inf: i32,
    /// CPA when motorizing guns; `None` for light trucks.
    #[serde(default)]
    pub cpa_guns: Option<i32>,
    /// CPA when carrying supplies in convoy.
    pub cpa_supplies: i32,
    /// Infantry TOE strength carried, in halves of a TOE point (1 = half a point).
    pub capacity_inf_toe_halves: i32,
    /// Artillery TOE points carried; `None` for light trucks.
    #[serde(default)]
    pub capacity_arty_toe: Option<i32>,
    pub capacity_aa_toe: i32,
    pub capacity_ammo_points: i32,
    pub capacity_fuel_points: i32,
    pub capacity_stores_points: i32,
    pub capacity_water_points: i32,
    /// Fuel the truck burns per fuel-capacity unit; see the chart footnotes.
    pub fuel_capacity_points: i32,
    pub fuel_consumption_factor: i32,
    /// Breakdown Adjustment Rating, in columns shifted to the left.
    pub bar_shift_left: i32,
    #[serde(default)]
    pub not_allowed: Vec<String>,
    #[serde(default)]
    pub footnotes: Vec<String>,
}

impl TruckCharacteristics {
    /// Supply points one Truck Point carries.
    pub fn supply_capacity(&self, supply: SupplyType) -> i32 {
        match supply {
            SupplyType::Ammo => self.capacity_ammo_points,
            SupplyType::Fuel => self.capacity_fuel_points,
            SupplyType::Stores => self.capacity_stores_points,
            SupplyType::Water => self.capacity_water_points,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct TruckRaw {
    row: Vec<TruckCharacteristics>,
}

/// The Truck Characteristics Chart (`airlog:54.2`).
#[derive(Debug, Clone)]
pub struct TruckTable {
    rows: Vec<TruckCharacteristics>,
}

impl Bound for TruckTable {
    const ID: &'static str = "airlog.54.2.truck_characteristics";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: TruckRaw = raw.deserialize()?;
        for t in [TruckType::Light, TruckType::Medium, TruckType::Heavy] {
            if r.row.iter().filter(|x| x.truck_type == t).count() != 1 {
                return Err(raw.err("row", format!("truck type {t:?} must appear exactly once")));
            }
        }
        Ok(Self { rows: r.row })
    }
}

impl TruckTable {
    /// The characteristics of one Truck Point of `truck_type`. `airlog:54.2`.
    pub fn truck(&self, truck_type: TruckType) -> &TruckCharacteristics {
        self.rows
            .iter()
            .find(|x| x.truck_type == truck_type)
            .expect("validated: every type present")
    }
}

// ---------------------------------------------------------------------------------------------
// 54.5 Equivalent Weights
// ---------------------------------------------------------------------------------------------

/// A weight cell: a tonnage, or one of the chart words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Tons(Ratio),
    /// The amount comes from the Axis Replacement Pool (case 20.6).
    Varies,
    /// Not applicable; the chart points to the stacking-point equivalents.
    NotApplicable,
    /// The chart prints P: this mode cannot carry it.
    Prohibited,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum WeightRaw {
    Tons(Ratio),
    Word(String),
}

impl WeightRaw {
    fn resolve(self) -> Result<Weight, String> {
        match self {
            WeightRaw::Tons(r) if r.den > 0 && r.num >= 0 => Ok(Weight::Tons(r)),
            WeightRaw::Tons(_) => Err("a tonnage needs num >= 0 and den > 0".to_string()),
            WeightRaw::Word(w) => match w.as_str() {
                "varies" => Ok(Weight::Varies),
                "na" => Ok(Weight::NotApplicable),
                "prohibited" => Ok(Weight::Prohibited),
                other => Err(format!("unknown weight word `{other}`")),
            },
        }
    }
}

/// How replacements and trucks travel (`airlog:54.5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    AxisNavalConvoy,
    InterportOrRailroad,
    Railroad,
    Interport,
    Air,
}

/// Things shipped by rail or interport that are priced in stacking points (`airlog:54.5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackingItem {
    TruckMotorizationPoint,
    ReplacementPoint,
    SquadronGroundSupportUnit,
    /// Printed label of the last row; its meaning is open (see `GAPS.md`).
    UnitOfStackingPoints,
}

#[derive(Debug, Clone, Deserialize)]
struct SupplyTonnageRaw {
    supply: SupplyType,
    tons: Ratio,
}

#[derive(Debug, Clone, Deserialize)]
struct ModeWeightRaw {
    by: TransportMode,
    tons: WeightRaw,
}

#[derive(Debug, Clone, Deserialize)]
struct StackingRowRaw {
    item: StackingItem,
    stacking_points: Ratio,
}

#[derive(Debug, Clone, Deserialize)]
struct WeightsRaw {
    supply_tonnage: Vec<SupplyTonnageRaw>,
    replacement_point_tonnage: Vec<ModeWeightRaw>,
    truck_motorization_point_tonnage: Vec<ModeWeightRaw>,
    stacking_point_equivalent: Vec<StackingRowRaw>,
}

/// The Equivalent Weights chart (`airlog:54.5`).
#[derive(Debug, Clone)]
pub struct EquivalentWeights {
    supply: Vec<(SupplyType, Ratio)>,
    replacement: Vec<(TransportMode, Weight)>,
    truck_motorization: Vec<(TransportMode, Weight)>,
    stacking: Vec<(StackingItem, Ratio)>,
}

impl Bound for EquivalentWeights {
    const ID: &'static str = "airlog.54.5.equivalent_weights";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: WeightsRaw = raw.deserialize()?;
        let mut supply = Vec::new();
        for (i, s) in r.supply_tonnage.iter().enumerate() {
            if s.tons.den <= 0 || s.tons.num <= 0 {
                return Err(raw.err(
                    format!("supply_tonnage[{i}].tons"),
                    "needs num > 0 and den > 0",
                ));
            }
            supply.push((s.supply, s.tons));
        }
        for t in [
            SupplyType::Ammo,
            SupplyType::Fuel,
            SupplyType::Stores,
            SupplyType::Water,
        ] {
            if supply.iter().filter(|(s, _)| *s == t).count() != 1 {
                return Err(raw.err("supply_tonnage", format!("{t:?} must appear exactly once")));
            }
        }
        let modes = |list: Vec<ModeWeightRaw>, name: &str| -> Result<Vec<_>, TableError> {
            list.into_iter()
                .enumerate()
                .map(|(i, m)| {
                    m.tons
                        .resolve()
                        .map(|w| (m.by, w))
                        .map_err(|msg| raw.err(format!("{name}[{i}].tons"), msg))
                })
                .collect()
        };
        let mut stacking = Vec::new();
        for (i, s) in r.stacking_point_equivalent.iter().enumerate() {
            if s.stacking_points.den <= 0 || s.stacking_points.num <= 0 {
                return Err(raw.err(
                    format!("stacking_point_equivalent[{i}].stacking_points"),
                    "needs num > 0 and den > 0",
                ));
            }
            stacking.push((s.item, s.stacking_points));
        }
        Ok(Self {
            supply,
            replacement: modes(r.replacement_point_tonnage, "replacement_point_tonnage")?,
            truck_motorization: modes(
                r.truck_motorization_point_tonnage,
                "truck_motorization_point_tonnage",
            )?,
            stacking,
        })
    }
}

impl EquivalentWeights {
    /// Tons in one point of `supply`. `airlog:54.5`.
    pub fn tons_per_point(&self, supply: SupplyType) -> Ratio {
        self.supply
            .iter()
            .find(|(s, _)| *s == supply)
            .map(|(_, r)| *r)
            .expect("validated: every supply present")
    }

    /// Tonnage of `points` of `supply`, rounded up to a whole ton. `airlog:54.5`.
    pub fn tons_for_points(&self, supply: SupplyType, points: i32) -> i32 {
        self.tons_per_point(supply).times(points).ceil()
    }

    /// Whole points of `supply` that fit in `tons`, rounded down. `airlog:54.5`.
    pub fn points_in_tons(&self, supply: SupplyType, tons: i32) -> i32 {
        let t = self.tons_per_point(supply);
        // tons / (num/den) = tons * den / num
        (tons * t.den).div_euclid(t.num)
    }

    /// Weight of one replacement point carried by `mode`, if the chart lists the mode.
    /// `airlog:54.5`.
    pub fn replacement_point_weight(&self, mode: TransportMode) -> Option<Weight> {
        self.replacement
            .iter()
            .find(|(m, _)| *m == mode)
            .map(|(_, w)| *w)
    }

    /// Weight of one truck or motorization point carried by `mode`, if the chart lists the
    /// mode. `airlog:54.5`.
    pub fn truck_point_weight(&self, mode: TransportMode) -> Option<Weight> {
        self.truck_motorization
            .iter()
            .find(|(m, _)| *m == mode)
            .map(|(_, w)| *w)
    }

    /// Stacking points one `item` counts as when moved by rail or interport. `airlog:54.5`.
    pub fn stacking_points(&self, item: StackingItem) -> Ratio {
        self.stacking
            .iter()
            .find(|(i, _)| *i == item)
            .map(|(_, r)| *r)
            .expect("validated: every stacking item present")
    }
}

// ---------------------------------------------------------------------------------------------
// 55.3 Port Capacity and Efficiency
// ---------------------------------------------------------------------------------------------

/// A port row of the capacity chart (`airlog:55.3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortName {
    Tripoli,
    Bizerta,
    Alexandria,
    Tobruk,
    Benghazi,
    MersaMatruh,
    Bardia,
    Sollum,
    Derna,
    AllOthers,
}

/// What one port can handle per Operations Stage.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PortCapacity {
    pub port: PortName,
    pub max_efficiency_level: i32,
    /// Stacking points that may be shipped in per OpStage; `None` where the chart prints
    /// `na` (not allowed).
    #[serde(default)]
    pub stacking_points_in: Option<i32>,
    #[serde(default)]
    pub stacking_points_out: Option<i32>,
    pub max_tonnage: i32,
    #[serde(default)]
    pub footnotes: Vec<String>,
    #[serde(default)]
    pub not_allowed: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PortsRaw {
    row: Vec<PortCapacity>,
}

/// The Port Capacity and Efficiency chart (`airlog:55.3`).
#[derive(Debug, Clone)]
pub struct PortCapacityTable {
    rows: Vec<PortCapacity>,
}

impl Bound for PortCapacityTable {
    const ID: &'static str = "airlog.55.3.port_capacity_and_efficiency";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: PortsRaw = raw.deserialize()?;
        for p in [
            PortName::Tripoli,
            PortName::Bizerta,
            PortName::Alexandria,
            PortName::Tobruk,
            PortName::Benghazi,
            PortName::MersaMatruh,
            PortName::Bardia,
            PortName::Sollum,
            PortName::Derna,
            PortName::AllOthers,
        ] {
            if r.row.iter().filter(|x| x.port == p).count() != 1 {
                return Err(raw.err("row", format!("port {p:?} must appear exactly once")));
            }
        }
        Ok(Self { rows: r.row })
    }
}

impl PortCapacityTable {
    /// The chart row for `port`. `airlog:55.3`.
    pub fn port(&self, port: PortName) -> &PortCapacity {
        self.rows
            .iter()
            .find(|x| x.port == port)
            .expect("validated: every port present")
    }
}
