//! Private activity history for whole-stage engineering eligibility.
//! Feature effects and construction procedures are implemented separately.
//! Cases: land:23.22, land:24.38, land:26.13
use std::collections::BTreeMap;

use cna_core::engine::EngineError;
use cna_core::ids::UnitId;
use serde::{Deserialize, Serialize};

use crate::seq::Block;
use crate::state::{Location, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ActivityStage {
    pub game_turn: u16,
    pub op_stage: u8,
}

impl ActivityStage {
    pub fn current(state: &State) -> Option<Self> {
        state.cursor.op_stage.map(|op_stage| Self {
            game_turn: state.cursor.game_turn,
            op_stage,
        })
    }
}

/// Bookkeeping never supplies unobserved opening history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitActivity {
    pub stage: ActivityStage,
    pub opening_location: Option<Location>,
    pub gross_cp_quarters: i32,
    pub departed: bool,
    pub pinned: bool,
}

impl UnitActivity {
    fn unknown(stage: ActivityStage) -> Self {
        Self {
            stage,
            opening_location: None,
            gross_cp_quarters: 0,
            departed: false,
            pinned: false,
        }
    }
}

/// Engineering activity foundation. Missing observed opening is not retroactive history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineeringState {
    pub observed_opening: Option<ActivityStage>,
    pub activity: BTreeMap<UnitId, UnitActivity>,
}

/// Called only at the approved public boundary, after any prior-stage CP reset.
/// Cases: land:23.22, land:24.38, land:26.13
pub fn open_stage(state: &mut State) {
    let Some(stage) = ActivityStage::current(state) else {
        return;
    };
    if state.engineering.observed_opening == Some(stage) {
        return;
    }
    if state.cursor.entered || state.cursor.block != Block::OpStage || state.cursor.index != 0 {
        return;
    }
    let activity = state
        .land
        .units
        .iter()
        .filter(|(_, unit)| {
            matches!(
                unit.location,
                Location::Hex { .. } | Location::OffMap { .. }
            )
        })
        .map(|(id, unit)| {
            (
                id.clone(),
                UnitActivity {
                    stage,
                    opening_location: Some(unit.location.clone()),
                    gross_cp_quarters: 0,
                    departed: false,
                    pinned: false,
                },
            )
        })
        .collect();
    state.engineering.observed_opening = Some(stage);
    state.engineering.activity = activity;
}

fn invariant(detail: impl Into<String>) -> EngineError {
    EngineError::Invariant {
        detail: detail.into(),
    }
}

fn candidate(state: &State, id: &UnitId) -> Result<UnitActivity, EngineError> {
    if !state.land.units.contains_key(id) {
        return Err(invariant(format!(
            "engineering activity: missing trusted unit {id}"
        )));
    }
    let stage = ActivityStage::current(state)
        .ok_or_else(|| invariant("engineering activity outside an Operations Stage"))?;
    if !(1..=3).contains(&stage.op_stage) {
        return Err(invariant(
            "engineering activity has an invalid current stage",
        ));
    }
    if let Some(opening) = state.engineering.observed_opening
        && (!(1..=3).contains(&opening.op_stage) || opening > stage)
    {
        return Err(invariant(
            "engineering activity has corrupt opening history",
        ));
    }
    let Some(entry) = state.engineering.activity.get(id) else {
        return Ok(UnitActivity::unknown(stage));
    };
    if entry.gross_cp_quarters < 0 || !(1..=3).contains(&entry.stage.op_stage) {
        return Err(invariant(
            "engineering activity has corrupt selected history",
        ));
    }
    if entry.stage > stage {
        return Err(invariant(
            "engineering activity contains future selected history",
        ));
    }
    if entry.stage == stage {
        Ok(entry.clone())
    } else {
        Ok(UnitActivity::unknown(stage))
    }
}

/// Commit only successful new positive expenditure, before refunds.
/// Missing observed opening stays missing, including during planning queries.
/// Cases: land:23.22, land:24.38, land:26.13
pub fn record_cp(state: &mut State, id: &UnitId, quarters: i32) -> Result<(), EngineError> {
    if quarters < 0 {
        return Err(invariant("engineering activity received negative CP"));
    }
    if quarters == 0 {
        return Ok(());
    }
    let mut next = candidate(state, id)?;
    next.gross_cp_quarters = next
        .gross_cp_quarters
        .checked_add(quarters)
        .ok_or_else(|| invariant("engineering gross CP overflow"))?;
    state.engineering.activity.insert(id.clone(), next);
    Ok(())
}

/// Requested members only; a missing entry is part of the captured state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivitySnapshot {
    game_turn: u16,
    op_stage: Option<u8>,
    observed_opening: Option<ActivityStage>,
    entries: BTreeMap<UnitId, Option<UnitActivity>>,
}

/// The stage guard deliberately excludes half, cycle and step.
/// Cases: land:23.22, land:26.13
pub fn activity_snapshot(state: &State, ids: &[UnitId]) -> ActivitySnapshot {
    ActivitySnapshot {
        game_turn: state.cursor.game_turn,
        op_stage: state.cursor.op_stage,
        observed_opening: state.engineering.observed_opening,
        entries: ids
            .iter()
            .map(|id| (id.clone(), state.engineering.activity.get(id).cloned()))
            .collect(),
    }
}

/// A failed guard mutates nothing. Unmatched entries and opening evidence survive.
/// Cases: land:23.22, land:26.13
pub fn restore_activity(state: &mut State, snapshot: &ActivitySnapshot) -> Result<(), EngineError> {
    if snapshot.game_turn != state.cursor.game_turn
        || snapshot.op_stage != state.cursor.op_stage
        || snapshot.observed_opening != state.engineering.observed_opening
    {
        return Err(invariant(
            "engineering activity snapshot belongs to another stage",
        ));
    }
    for (id, entry) in &snapshot.entries {
        match entry {
            Some(entry) => {
                state.engineering.activity.insert(id.clone(), entry.clone());
            }
            None => {
                state.engineering.activity.remove(id);
            }
        }
    }
    Ok(())
}

/// Necessary idle-history condition only; callers separately verify engineer applicability.
/// Missing global opening, missing record or missing opening location cannot qualify.
/// Cases: land:23.22, land:24.38, land:26.13
pub fn whole_stage_idle_at(state: &State, id: &UnitId, location: &Location) -> bool {
    let Some(stage) = ActivityStage::current(state) else {
        return false;
    };
    state.engineering.observed_opening == Some(stage)
        && state
            .land
            .units
            .get(id)
            .is_some_and(|unit| &unit.location == location)
        && state.engineering.activity.get(id).is_some_and(|entry| {
            entry.stage == stage
                && entry.opening_location.as_ref() == Some(location)
                && entry.gross_cp_quarters == 0
                && !entry.departed
                && !entry.pinned
        })
}

#[cfg(test)]
mod tests;
