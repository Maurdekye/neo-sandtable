//! Supply procedures and the interfaces used by movement and combat.
//!
//! Movement calls [`movement_fuel_cost`] with the CP spent moving in one segment,
//! measured in quarters. Combat calls [`ammunition_cost`] with the chart action and
//! participating TOE. Neither function spends supplies. The caller offers the legal
//! sources returned by [`available_sources`], then calls [`spend_for_unit`] with its
//! chosen allocation before moving or resolving fire. Repeated movement answers
//! use [`plan_segment_fuel`] and [`spend_segment_fuel`] with cumulative segment CP;
//! these preserve physical movement history and one shared source-rounding account.
//! Fuel origins are actual Locations; legacy hex-string checkpoints are migrated.
//! Named off-map boxes retain their source-bound stocks. Transit uses a distinct
//! opaque traveling-group location: own tanks and same-group first-line cargo,
//! with no box dumps or unlimited fuel after departure. New segments capture the
//! current location without resetting physical cohort identities.
//! Truck division calls transfer_selected_segment_fuel_cohorts before physical
//! counts change; removals/recovery use the matching cohort helpers. Store removed
//! cohorts with broken vehicles. Unit-body CP stays independent, and lost vehicle
//! TOE never refunds historical fuel. Planning uses snapshot_fuel_accounts and
//! restore_fuel_accounts alongside ordinary holdings/ledger snapshots.
//! A failed allocation changes
//! nothing. The caller emits the resulting events to the owning side.
//!
//! Movement also calls [`movement_restrictions`] before accepting a path. Enforce
//! its CPA, enemy-ZOC, movement and offensive-assault flags. Call
//! [`spend_activity_water`] once immediately before the first CPA use; it retains
//! idle vehicle reserves and does not recharge activity water on repeated moves.
//! CPA actions allowed while dry use [`consume_activity_water_forced`]: a shortage
//! consumes available water and preserves the unpaid balance without rejecting the action.
//! The same water call is needed by other CPA-consuming procedures, and combat
//! uses the restrictions' defense divisor and offensive-assault flag.
//!
//! Tanks and ready ammunition can serve only their own unit. Friendly first-line
//! truck cargo and active dumps can serve units in the same hex. Second-/third-line
//! cargo must be unloaded before it enters this interface. Emergency siphoning,
//! refuelling into tanks, captured stocks and unit carrying capacities are separate
//! procedures; this API spends existing holdings and cannot bypass those decisions.
//! Truck convoys use pool_fuel::plan_pool_segment_fuel/spend_pool_segment_fuel.
//! Pool truck cohorts and funding accounts have separate stable-pool-ID maps;
//! existing UnitId maps contain only land units. Own convoy fuel cargo follows its
//! carrier (49.18), while other pools still unload before supplying it. Tank fuel
//! stays exact in tenths; shared pure pricing preserves cumulative source credit.
//! Breakdown retires/restores explicit pool cohorts before changing truck counts.
//! Cargo history uses explicit Unit/Pool/Dump/Ship sites and side-local lot identities.
//! advance charges trusted carrier CP; transfer moves selected histories before physical
//! inventory changes on the same draft. retire_debit retires automatic stock losses;
//! supply withdrawals already call it. Planning snapshots the carrier and source sites.
//! These primitives do not make transfer decisions or authorize cargo movement.
//!
//! Answer acceptance uses only the answering side's units, stocks and known conditions,
//! plus public facts. Hidden opposing state belongs to adjudication after closed windows;
//! a failed well result remains an accepted attempt with its rules-required disclosure.
//! Scheduling is fixed independently of hidden quantities and eligibility: stores,
//! distribution and each scheduled coastal side take one complete list; water takes
//! exactly two joint rounds (issue/draw, then actual-yield allocation); attrition takes
//! one complete mandatory casualty list per side. Empty windows have a declared pass.
//! Arrival supply uses the same fixed two-round pattern. Answers never reopen these
//! windows; only joint closure adjudicates wells and food casualties. Naval planning's
//! number of slots follows the public scenario calendar, not secret cargo or port stocks.
//! Weather rolls and storm areas are public automatic procedures; ports, box handling,
//! pool accounting and cargo-history helpers do not open decisions. Legacy single-unit
//! menu entry points route to batches and their old response kinds are retired.
//! This preserves the observer's ordered requests and phase stream under engine rule 7.

//!
//! Cases: airlog:49.13, airlog:49.15, airlog:49.16, airlog:50.13, airlog:50.15
//! Interpretations: interp:airlog-0001

pub mod activity;
pub mod air_supply;
pub use activity::{
    ActivityWaterLedger, ActivityWaterPayment, TruckWater, activity_water_due,
    consume_activity_water_forced, remove_activity_water_credit, restore_activity_water_credit,
    transfer_activity_water_credit,
};
pub mod arrivals;
pub mod attrition;
pub mod baseline;
pub mod batches;
pub mod box_handling;
pub mod capacity;
pub mod cargo_history;
pub mod coastal;
pub mod convoys;
pub mod distribution;
pub mod dump_markers;
pub mod pool_fuel;
pub mod pools;
pub mod port_initialization;
pub mod ports;
pub use capacity::{CargoPacking, cargo_bound, fuel_capacity, validate_packing};
mod rations;
pub mod ready;
pub use ready::{close_assault_ammo_action, ready_ammo_capacity};
mod segment;
pub mod stores;
mod supply;
pub mod truck_convoy;
pub use segment::{
    FuelAccountSnapshot, FuelCohortSelection, FuelDraw, FuelFundingAccount, FuelSegmentLedger,
    FuelTruckKind, SegmentFuelPlan, SegmentFuelSpent, SegmentKey, TruckFuelCohort,
    plan_segment_fuel, remove_segment_fuel_cohorts, remove_selected_segment_fuel_cohorts,
    restore_fuel_accounts, restore_segment_fuel_cohorts, segment_fuel_cohorts,
    snapshot_fuel_accounts, spend_segment_fuel, spend_segment_fuel_report,
    transfer_segment_fuel_cohorts, transfer_selected_segment_fuel_cohorts,
};
pub mod water;
pub mod weather;
pub mod wells;
pub use supply::{
    SupplyDemand, SupplyDraw, SupplyError, SupplySource, ammunition_cost, available_sources,
    available_sources_at, available_sources_at_location, available_sources_at_with_content,
    available_sources_with_content, movement_fuel_cost, spend_for_unit,
    spend_for_unit_with_content, toe_strength,
};

pub use rations::{
    MovementRestrictions, PrisonerGroup, Rations, movement_restrictions, spend_activity_water,
};

#[cfg(test)]
mod privacy;

#[cfg(test)]
mod scheduling_tests;

#[cfg(test)]
pub(crate) use segment::QueryFuelPath;
pub(crate) use segment::spend_query_segment_fuel_report;
pub(crate) use supply::PreparedMovementFuel;

pub(crate) use supply::query_withdrawal_history_sites;
