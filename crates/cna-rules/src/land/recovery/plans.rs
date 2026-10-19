//! Buffered owner choices and current closure only; no outcomes or replay archive.
//! Cases: land:21.5, land:21.6, land:22.32, land:22.44, interp:land-0038
use super::ReturnPoolTarget;
use crate::{
    land::breakdown::{Asset, pools::PoolAsset},
    logistics::cargo_history::LotSelection,
    seq::Half,
};
use cna_core::ids::HexId;
use cna_protocol::Side;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryProcedure {
    Towing,
    Maintenance,
}

/// Trusted entry derives this from the actual cursor; Respond never supplies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryClosureKey {
    pub game_turn: u16,
    pub op_stage: u8,
    pub half: Half,
    pub cycle: u16,
    pub phasing: Side,
    pub procedure: RecoveryProcedure,
}

/// Only the actual current opportunity, its buffered plans and resolved flag.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RecoveryProcedureState {
    pub opportunity: Option<RecoveryClosureKey>,
    pub repair_plan: Option<RepairPlan>,
    pub tow_plan: Option<TowPlan>,
    pub resolved: bool,
}

/// Choice of procedure, not an actor-supplied eligibility verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairMode {
    Field,
    Facility,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairPlan {
    pub markers: Vec<RepairMarkerPlan>,
}

/// Identities/counts are answer choices, revalidated against the actual marker.
/// No demand, paid credit, verdict, roll, closure key or history is accepted here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairMarkerPlan {
    pub marker: String,
    pub assets: Vec<Asset>,
    pub pool_assets: Vec<PoolAsset>,
    pub mode: RepairMode,
    pub cargo_priority: Vec<LotSelection>,
    #[serde(with = "return_pool_choice")]
    pub(crate) return_pool: ReturnPoolTarget,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TowPlan {
    pub markers: Vec<TowMarkerPlan>,
}

/// The route and physical partitions are selected before trusted validation.
/// A route does not certify direction, capacity, ownership or admissibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TowMarkerPlan {
    pub marker: String,
    pub assets: Vec<Asset>,
    pub pool_assets: Vec<PoolAsset>,
    pub cargo: Vec<LotSelection>,
    pub route: Vec<HexId>,
}

/// Serialize ONLY the approved owner target choice, keeping the exact agreed
/// trusted payload body free of Deserialize and public actor constructors.
mod return_pool_choice {
    use super::ReturnPoolTarget;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "target", rename_all = "snake_case", deny_unknown_fields)]
    enum Choice {
        Existing { pool: String },
        CreateAtSite,
    }

    pub(super) fn serialize<S: Serializer>(
        target: &ReturnPoolTarget,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match target {
            ReturnPoolTarget::Existing { pool } => Choice::Existing { pool: pool.clone() },
            ReturnPoolTarget::CreateAtSite => Choice::CreateAtSite,
        }
        .serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ReturnPoolTarget, D::Error> {
        Ok(match Choice::deserialize(deserializer)? {
            Choice::Existing { pool } => ReturnPoolTarget::Existing { pool },
            Choice::CreateAtSite => ReturnPoolTarget::CreateAtSite,
        })
    }
}
