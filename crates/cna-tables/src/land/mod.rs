//! Typed Land chart bindings. Lookups return chart data; procedures apply unit state and timing.

pub mod administration;
pub mod capability;
pub mod terrain;

crate::tables_group! {
    /// Land tables currently bound; the loader test records the remaining tables explicitly.
    LandTables {
        capability_expenditure: capability::CapabilityExpenditure,
        initiative_ratings: administration::InitiativeRatings,
        terrain_effects: terrain::TerrainEffects,
        off_map_movement: administration::OffMapMovement,
        stacking_values: administration::StackingValues,
    }
}
