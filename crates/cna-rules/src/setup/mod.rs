//! Scenario setup decisions and their private simultaneous window.
mod decisions;
mod facilities;
pub mod placement;
mod pools;
mod preload;
mod stacking;
pub(crate) use decisions::{KIND_DUMP, KIND_TRUCKS, KIND_UNIT, answer, enter, finish};

use crate::state::Location;
use cna_core::ids::{DecisionId, UnitId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Pending setup routing and owner-private destinations, never static unit characteristics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetupState {
    pub started: bool,
    pub closed: bool,
    pub tasks: BTreeMap<DecisionId, SetupTask>,
    pub unit_locations: BTreeMap<UnitId, Location>,
    #[serde(default)]
    pub placement_serial: u64,
    #[serde(default)]
    pub placement_order: BTreeMap<UnitId, u64>,
    pub dump_locations: BTreeMap<String, Location>,
    #[serde(default)]
    pub pools_started: bool,
    #[serde(default)]
    pub preload_started: bool,
    #[serde(default)]
    pub preload_packing: BTreeMap<String, crate::logistics::CargoPacking>,
    #[serde(default)]
    pub pool_sources: BTreeMap<String, usize>,
    #[serde(default)]
    pub pool_locations: BTreeMap<String, Location>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SetupTask {
    Unit {
        unit: UnitId,
        case: String,
    },
    Dump {
        dump: String,
        case: String,
    },
    Trucks {
        group: String,
    },
    Pool {
        pool: String,
        source: usize,
    },
    Preload {
        asset: preload::Asset,
        operation: String,
    },
}

pub(crate) const KIND_POOL: &str = pools::KIND;

pub(crate) const KIND_PRELOAD: &str = preload::KIND;
