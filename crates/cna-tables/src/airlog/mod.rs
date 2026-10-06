//! Bindings for the Air & Logistics Games tables (`data/tables/airlog/`).

pub mod convoys;
pub mod fuel;
pub mod supply;
pub mod trucks;

crate::tables_group! {
    /// Every Air & Logistics table, bound.
    AirlogTables {
        fuel_consumption: fuel::FuelConsumption,
        ammunition_consumption: supply::AmmunitionConsumption,
        water_availability: supply::WaterAvailability,
        poisoning_and_sweetening: supply::PoisoningAndSweetening,
        supply_dump_capacity: supply::SupplyDumpCapacity,
        supply_dump_demolition: supply::SupplyDumpDemolition,
        truck_characteristics: trucks::TruckTable,
        equivalent_weights: trucks::EquivalentWeights,
        port_capacity: trucks::PortCapacityTable,
        convoy_level: convoys::ConvoyLevelTable,
        convoy_capacity: convoys::ConvoyCapacityTable,
        convoy_air_distance: convoys::ConvoyAirDistance,
        road_distance: convoys::RoadDistance,
        abstract_truck_loss: convoys::AbstractTruckLoss,
    }
}
