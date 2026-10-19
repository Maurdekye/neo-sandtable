//! Private load facts and permanent initial-credit history.
//!
//! Generic inventory migration records only UnknownLegacy, terminal for free
//! initial credit. A future real setup-close import must construct brand-new
//! InitialUnspecified/Unselected records directly. Paid fuel admission belongs
//! to the transactional maintenance producer, not these structural records.
//! These source checks grant no arming, mission, payment or flight permission.

use std::collections::BTreeMap;

use cna_core::engine::EngineError;
use serde::{Deserialize, Serialize};

use super::state::{AircraftState, PlaneId};
use crate::CnaContent;

/// Missing state is a pre-feature checkpoint: unknown load, NO free credit.
/// Only the canonical private migration may promote its empty maps. Once true,
/// every live plane requires an explicit current entry, including legacy ones.
/// Initial ledger tombstones survive aircraft loss; current entries do not.
/// Cases: airlog:38.2, airlog:38.4, scen:59.32
/// Interpretations: interp:air-0018
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PreparationState {
    pub(crate) initialized: bool,
    pub(crate) initial_choices: BTreeMap<PlaneId, InitialChoice>,
    pub(crate) current: BTreeMap<PlaneId, CurrentPreparation>,
}

/// One whole initial choice, never independent fuel/gun/ordnance credits.
/// Only an authenticated real fresh import can create Unselected; this module
/// exposes no registration or commitment authority.
/// Cases: scen:59.32, scen:59.36
/// Interpretations: interp:air-0018
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum InitialChoice {
    Unselected,
    Committed,
    Forfeited,
}

/// Current facts are independent of who spent the initial credit. Paid later
/// loads may have no ledger; absence of a ledger never authenticates fresh setup.
/// Cases: airlog:38.2, airlog:38.4, scen:59.32
/// Interpretations: interp:air-0018
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CurrentPreparation {
    /// Only brand-new real setup-close import, paired with Unselected directly.
    InitialUnspecified,
    /// No typed ammunition/load and NO initial credit, including after later
    /// paid replacement. Never convertible to initial state by setup authority.
    UnknownLegacy,
    Unprepared,
    /// Paid fuel with ammunition still unknown; never a complete flight load.
    /// Cases: airlog:38.21, airlog:38.24, airlog:38.25
    KnownFuel {
        mode: usize,
        fuel_available: bool,
    },
    KnownLoad {
        load: KnownLoad,
    },
}

/// A trusted recorded load, not a request that buys one or authorizes flight.
/// No defaults: omitted mode/fuel/guns/ordnance cannot fabricate a complete load.
/// Cases: airlog:34.14, airlog:34.17, airlog:38.4
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownLoad {
    pub mode: usize,
    pub fuel_available: bool,
    pub gun_ammunition: bool,
    #[serde(deserialize_with = "deserialize_ordnance")]
    pub ordnance: Option<CarriedOrdnance>,
}

/// At most one carried ordnance choice. Values are source rating points, not
/// physical mine/torpedo counts or a new supply unit. No price or partial-load
/// permission follows from structural/source validation.
/// Cases: airlog:34.14, airlog:38.47
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CarriedOrdnance {
    Bombs {
        bomb_points: i32,
    },
    /// Exact selected-mode bombload rating, not mine quantity, maximum or
    /// countdown. Expending a carried choice belongs to a later flight procedure.
    /// Cases: airlog:34.14, airlog:38.47, airlog:41.39
    Mines {
        source_bombload_rating: i32,
    },
    Torpedoes {
        rating_points: i32,
    },
}

// A complete KnownLoad records an explicit carried choice, including null.
// Serde otherwise treats a missing Option field as None.
fn deserialize_ordnance<'de, D>(deserializer: D) -> Result<Option<CarriedOrdnance>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<CarriedOrdnance>::deserialize(deserializer)
}

fn invalid(detail: impl Into<String>) -> EngineError {
    EngineError::Invariant {
        detail: format!("air preparation: {}", detail.into()),
    }
}

/// Necessary source consistency only: exact aircraft mode and recorded payload.
/// Missing trusted ratings preserve Unsupported provenance even for zero payload;
/// no absent rating means a capability, zero cost or another mode's value.
/// No fuel/gun cost, mission eligibility or producer authority is evaluated.
/// Cases: airlog:34.14, airlog:38.47, airlog:41.39
pub fn validate_load(
    content: &CnaContent,
    plane: &AircraftState,
    load: &KnownLoad,
) -> Result<(), EngineError> {
    let aircraft = content
        .units
        .aircraft
        .get(&plane.aircraft)
        .ok_or_else(|| invalid("known load has no canonical aircraft source"))?;
    let mode = aircraft
        .modes
        .get(load.mode)
        .ok_or_else(|| invalid("known load mode is outside its aircraft source"))?;
    let Some(ordnance) = &load.ordnance else {
        return Ok(());
    };
    let (kind, recorded, source, exact) = match ordnance {
        CarriedOrdnance::Bombs { bomb_points } => {
            ("bombs", *bomb_points, mode.bomb_capacity, false)
        }
        CarriedOrdnance::Mines {
            source_bombload_rating,
        } => ("mines", *source_bombload_rating, mode.bomb_capacity, true),
        CarriedOrdnance::Torpedoes { rating_points } => {
            ("torpedoes", *rating_points, mode.torpedo_capacity, false)
        }
    };
    if recorded < 0 {
        return Err(invalid("negative recorded ordnance rating"));
    }
    let source = source.ok_or_else(|| EngineError::Unsupported {
        case: "airlog:34.14".into(),
        detail: format!(
            "{kind} rating missing for {} mode {}",
            plane.aircraft, load.mode
        ),
    })?;
    if source < 0 {
        return Err(invalid("negative trusted ordnance rating"));
    }
    if recorded > source || (exact && recorded != source) {
        return Err(invalid(
            "recorded ordnance disagrees with selected source mode",
        ));
    }
    Ok(())
}

/// Pure informational calculation; only inventory::update writes its result.
/// It supplies no typed gun ammunition or flight/mission eligibility. Old source
/// summaries survive unknown/unspecified loads without becoming free credit.
/// Cases: airlog:38.4, scen:59.32
/// Interpretations: interp:air-0018
pub fn armed_projection(current: &CurrentPreparation, old_bit: bool) -> bool {
    match current {
        CurrentPreparation::UnknownLegacy
        | CurrentPreparation::InitialUnspecified
        | CurrentPreparation::KnownFuel { .. } => old_bit,
        CurrentPreparation::Unprepared => false,
        CurrentPreparation::KnownLoad { load } => {
            load.gun_ammunition
                || load
                    .ordnance
                    .as_ref()
                    .is_some_and(|ordnance| match ordnance {
                        CarriedOrdnance::Bombs { bomb_points } => *bomb_points > 0,
                        CarriedOrdnance::Mines {
                            source_bombload_rating,
                        } => *source_bombload_rating > 0,
                        CarriedOrdnance::Torpedoes { rating_points } => *rating_points > 0,
                    })
        }
    }
}

/// Typed fuel knowledge, without inferring anything from a legacy coarse bit.
/// Cases: airlog:38.21, airlog:38.25
pub(in crate::air) fn known_fuel(current: &CurrentPreparation) -> Option<(usize, bool)> {
    match current {
        CurrentPreparation::KnownFuel {
            mode,
            fuel_available,
        } => Some((*mode, *fuel_available)),
        CurrentPreparation::KnownLoad { load } => Some((load.mode, load.fuel_available)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
