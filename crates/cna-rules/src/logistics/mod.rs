//! Supply procedures and the interfaces used by movement and combat.
//!
//! Movement calls [`movement_fuel_cost`] with the CP spent moving in one segment,
//! measured in quarters. Combat calls [`ammunition_cost`] with the chart action and
//! participating TOE. Neither function spends supplies. The caller offers the legal
//! sources returned by [`available_sources`], then calls [`spend_for_unit`] with its
//! chosen allocation before moving or resolving fire. A failed allocation changes
//! nothing. The caller emits the resulting events to the owning side.
//!
//! Tanks and ready ammunition can serve only their own unit. Friendly first-line
//! truck cargo and active dumps can serve units in the same hex. Second-/third-line
//! cargo must be unloaded before it enters this interface. Emergency siphoning,
//! refuelling into tanks, captured stocks and unit carrying capacities are separate
//! procedures; this API spends existing holdings and cannot bypass those decisions.
//!
//! Cases: airlog:49.13, airlog:49.15, airlog:49.16, airlog:50.13, airlog:50.15
//! Interpretations: interp:airlog-0001

mod supply;
pub use supply::{
    SupplyDemand, SupplyDraw, SupplyError, SupplySource, ammunition_cost, available_sources,
    movement_fuel_cost, spend_for_unit, toe_strength,
};
