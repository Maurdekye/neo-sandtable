//! Typed Land chart bindings. Lookups return chart data; procedures apply unit state and timing.

pub mod administration;
pub mod anti_armor;
pub mod assault;
pub mod barrage;
pub mod breakdown;
pub mod capability;
pub mod combat;
pub mod engineering;
mod grid;
pub mod morale;
pub mod raids;
pub mod terrain;
pub mod weather;

crate::tables_group! {
    /// Land tables currently bound; the loader test records the remaining tables explicitly.
    LandTables {
        desert_raider_raids: raids::DesertRaiderRaids,
        construction: engineering::ConstructionChart,
        demolition: engineering::DemolitionChart,
        combat_calculations: combat::CombatCalculations,
        organization_size: combat::OrganizationSize,
        prisoners_captured: combat::PrisonersCaptured,
        barrage: barrage::BarrageTable,
        anti_armor: anti_armor::AntiArmorTable,
        close_assault: assault::CloseAssaultTable,
        morale: morale::MoraleTable,
        breakdown: breakdown::BreakdownTable,
        capability_expenditure: capability::CapabilityExpenditure,
        initiative_ratings: administration::InitiativeRatings,
        terrain_effects: terrain::TerrainEffects,
        off_map_movement: administration::OffMapMovement,
        stacking_values: administration::StackingValues,
        weather: weather::WeatherTable,
        foul_weather_location: weather::FoulWeatherLocation,
    }
}
