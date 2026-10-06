//! Bindings for the Air & Logistics Games tables (`data/tables/airlog/`).

pub mod air;
pub mod convoys;
pub mod crt;
pub mod distance;
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
        strafing: crt::Strafing,
        air_bombardment: crt::AirBombardment,
        aa_combat: crt::AaCombat,
        flak_adjustment: crt::FlakAdjustment,
        commonwealth_pilots: air::CommonwealthPilotArrival,
        italian_pilots: air::ItalianPilotArrival,
        german_pilots: air::GermanPilotArrival,
        squadron_capacity: air::SquadronCapacityTable,
        offmap_air_facilities: air::OffMapAirFacilities,
        air_distance: distance::AirDistance,
        land_distance: distance::LandDistance,
        aircraft_refit: air::AircraftRefit,
        mission_summary: air::MissionSummary,
        scramble: air::Scramble,
        mining_harbor: air::MiningHarbor,
        recon_land_units: air::ReconLandUnits,
        recon_axis_convoys: air::ReconAxisConvoys,
        malta_commitment: air::MaltaCommitmentTable,
        malta_availability: air::MaltaAvailability,
        maltese_construction: air::MalteseConstruction,
        maneuver_adjustment: air::ManeuverAdjustment,
        tacair_kill: air::TacAirKill,
        pilot_and_plane_recovery: air::PilotAndPlaneRecovery,
    }
}
