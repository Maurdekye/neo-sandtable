//! Scenario setup decisions and their private simultaneous window.
mod decisions;
mod facilities;
pub mod placement;
mod pools;
mod stacking;
pub(crate) use decisions::{KIND_DUMP, KIND_TRUCKS, KIND_UNIT, answer, enter};

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
    pub dump_locations: BTreeMap<String, Location>,
    #[serde(default)]
    pub pools_started: bool,
    #[serde(default)]
    pub pool_locations: BTreeMap<String, Location>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SetupTask {
    Unit { unit: UnitId, case: String },
    Dump { dump: String, case: String },
    Trucks { group: String },
    Pool { pool: String, source: usize },
}

pub(crate) const KIND_POOL: &str = pools::KIND;
