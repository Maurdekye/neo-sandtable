//! Individual aircraft and rated pilots, without copied aircraft ratings.

use std::collections::BTreeMap;

use cna_protocol::Side;
use serde::{Deserialize, Serialize};

/// Private persistent identity; never an enemy-facing combat label.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlaneId(pub String);

/// Private persistent identity; zero-rated pilots need no record.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PilotId(pub String);

/// One possessed aircraft. `refitted` is independent of fuel and ammunition.
/// The setup aggregate's `ready` count means refitted, not a computed flight
/// entitlement. Reserve and mission eligibility belong to their procedures.
/// Cases: airlog:34.0, airlog:38.1, scen:59.32
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AircraftState {
    pub aircraft: String,
    pub force: String,
    pub squadron: Option<String>,
    /// Physical base; may differ from the assigned squadron's SGSU.
    /// None is an unplaced force reserve, not permission to fly.
    pub facility: Option<String>,
    pub refitted: bool,
    pub fuelled: bool,
    /// Compatibility with setup's arming count. Mission-specific gun and
    /// ordnance expenditures will refine this before operational flight.
    pub armed: bool,
}

/// One rated pilot's assignment and training provenance. No training type is
/// guessed for a reserve or an initially mixed squadron.
/// Source cases: airlog:35.24, airlog:40.12, airlog:40.16
/// Interpretations: interp:air-0005
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PilotState {
    pub force: String,
    pub squadron: Option<String>,
    pub rating: u8,
    pub trained_aircraft: Option<String>,
}

/// Procedure-owned inventory. Before `initialized`, setup owns its aggregates;
/// afterward these records are authoritative and aggregates are exact mirrors.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AirRuntime {
    pub(crate) initialized: bool,
    /// One-time migration independent of aircraft inventory import. Empty maps
    /// after this marker is true must never trigger reconstruction.
    #[serde(default)]
    pub(crate) bases_initialized: bool,
    pub facilities: BTreeMap<super::facilities::FacilityId, super::facilities::FacilityState>,
    pub sgsus: BTreeMap<super::sgsu::SgsuId, super::sgsu::SgsuState>,
    pub designation: super::designation::DesignationState,
    pub aircraft: BTreeMap<PlaneId, AircraftState>,
    pub pilots: BTreeMap<PilotId, PilotState>,
    /// Side-local monotone serials keep the other side's assets from affecting
    /// one's own IDs, and never reuse an ID after its record is removed.
    pub(crate) plane_serial: BTreeMap<Side, u64>,
    pub(crate) pilot_serial: BTreeMap<Side, u64>,
}

impl AirRuntime {
    pub fn initialized(&self) -> bool {
        self.initialized
    }
}
