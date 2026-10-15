//! Private activity history for whole-stage engineering eligibility.
//! Feature effects and construction procedures are implemented separately.
//! Cases: land:23.22, land:24.38, land:26.13
use std::collections::{BTreeMap, BTreeSet};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_activity: Option<ClosedActivity>,
    pub activity: BTreeMap<UnitId, UnitActivity>,
    facility_project_serial: u64,
    facility_projects: BTreeMap<projects::ProjectId, projects::FacilityProject>,
}

/// Latest completed interval; opening and final presence are observations only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedActivity {
    pub stage: ActivityStage,
    pub observed_opening: Option<ActivityStage>,
    pub entries: BTreeMap<UnitId, ClosedUnitActivity>,
    pub required_facility_projects: BTreeSet<ProjectId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedUnitActivity {
    pub activity: UnitActivity,
    pub closing_location: Option<Location>,
}

fn valid_stage(stage: ActivityStage) -> bool {
    stage.game_turn > 0 && (1..=3).contains(&stage.op_stage)
}

fn physical_location(location: &Location) -> Option<Location> {
    match location {
        Location::Hex { .. } | Location::OffMap { .. } => Some(location.clone()),
        _ => None,
    }
}

fn validate_activity_entry(
    id: &UnitId,
    entry: &UnitActivity,
    end: ActivityStage,
) -> Result<(), EngineError> {
    if id.as_str().is_empty()
        || !valid_stage(entry.stage)
        || entry.stage > end
        || entry.gross_cp_quarters < 0
    {
        return Err(invariant(
            "engineering closure has corrupt selected activity",
        ));
    }
    if entry
        .opening_location
        .as_ref()
        .is_some_and(|at| physical_location(at).is_none())
    {
        return Err(invariant(
            "engineering closure has a nonphysical opening observation",
        ));
    }
    Ok(())
}

fn validate_closed(
    state: &State,
    closed: &ClosedActivity,
    end: ActivityStage,
) -> Result<(), EngineError> {
    if !valid_stage(closed.stage)
        || closed.stage > end
        || closed
            .observed_opening
            .is_some_and(|opening| !valid_stage(opening) || opening > closed.stage)
    {
        return Err(invariant(
            "engineering retained interval has invalid stage metadata",
        ));
    }
    for (id, entry) in &closed.entries {
        validate_activity_entry(id, &entry.activity, closed.stage)?;
        if entry
            .closing_location
            .as_ref()
            .is_some_and(|at| physical_location(at).is_none())
        {
            return Err(invariant(
                "engineering retained interval has a nonphysical closing observation",
            ));
        }
    }
    projects::validate_required_projects(state, &closed.required_facility_projects)
}

/// Shared old-tail authentication, independent of hidden actor availability.
/// Cases: land:24.11, land:24.12
pub(crate) fn closing_stage(state: &State) -> Result<ActivityStage, EngineError> {
    let stage = ActivityStage::current(state)
        .ok_or_else(|| invariant("engineering closure outside an Operations Stage"))?;
    if !valid_stage(stage)
        || state.cursor.block != Block::PlayerHalf
        || state.cursor.half != Some(crate::seq::Half::B)
        || state.cursor.index.checked_add(1) != Some(crate::seq::PLAYER_HALF.len())
        || !state.cursor.entered
        || !state.decisions.pending.is_empty()
    {
        return Err(invariant(
            "engineering closure outside the completed old-stage tail",
        ));
    }
    Ok(stage)
}

/// No progress consumer exists yet. Historical required identities remain blocking.
/// Cases: land:24.11
pub fn validate_activity_retirement(
    state: &State,
    prior: ActivityStage,
) -> Result<(), EngineError> {
    let old = closing_stage(state)?;
    if !valid_stage(prior) || prior > old {
        return Err(invariant(
            "engineering retirement has an invalid prior label",
        ));
    }
    let Some(closed) = &state.engineering.closed_activity else {
        return Err(EngineError::Unsupported {
            case: "land:24.11".into(),
            detail: "retained Air interval has no authenticated Engineering dependency record"
                .into(),
        });
    };
    validate_closed(state, closed, old)?;
    if closed.stage != prior {
        return Err(EngineError::Unsupported {
            case: "land:24.11".into(),
            detail: "legacy retained intervals have unresolved pairing".into(),
        });
    }
    if !closed.required_facility_projects.is_empty() {
        return Err(EngineError::Unsupported {
            case: "land:24.11".into(),
            detail: "required project progress has no persisted interval consumer".into(),
        });
    }
    Ok(())
}

/// Air has captured the new old interval; retire Engineering's OWN prior only.
/// Cases: land:24.11, land:24.12, land:24.22, land:24.23, land:23.22, land:26.13
pub fn close_stage(state: &mut State) -> Result<(), EngineError> {
    let stage = closing_stage(state)?;
    if state
        .engineering
        .observed_opening
        .is_some_and(|opening| !valid_stage(opening) || opening > stage)
    {
        return Err(invariant("engineering closure has invalid opening history"));
    }
    let mut entries = BTreeMap::new();
    for (id, activity) in &state.engineering.activity {
        validate_activity_entry(id, activity, stage)?;
        let closing_location = state
            .land
            .units
            .get(id)
            .and_then(|unit| physical_location(&unit.location));
        entries.insert(
            id.clone(),
            ClosedUnitActivity {
                activity: activity.clone(),
                closing_location,
            },
        );
    }
    let snapshot = ClosedActivity {
        stage,
        observed_opening: state.engineering.observed_opening,
        entries,
        required_facility_projects: projects::required_facility_projects(state)?,
    };
    let air = state
        .air
        .runtime
        .sgsu_activity
        .previous_closed
        .as_ref()
        .ok_or_else(|| invariant("Engineering close has no new Air closure"))?;
    if air.stage.game_turn != stage.game_turn || air.stage.op_stage != stage.op_stage || !air.closed
    {
        return Err(invariant(
            "Engineering close has an incompatible new Air closure",
        ));
    }
    if let Some(closed) = &state.engineering.closed_activity {
        validate_closed(state, closed, stage)?;
        if closed.stage == stage {
            if closed != &snapshot {
                return Err(invariant(
                    "engineering same-stage closure would rewrite history",
                ));
            }
            // An equal duplicate evicts nothing, even with required projects.
            return Ok(());
        }
        validate_activity_retirement(state, closed.stage)?;
    }
    state.engineering.closed_activity = Some(snapshot);
    Ok(())
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

/// Record actual applied pin, including another actual pin on an already pinned unit.
/// Caller establishes immunity and actual application on its transactional draft first.
/// Cases: land:23.22, land:26.13
pub fn record_pin(state: &mut State, id: &UnitId) -> Result<(), EngineError> {
    let mut next = candidate(state, id)?;
    next.pinned = true;
    state.engineering.activity.insert(id.clone(), next);
    Ok(())
}

/// Record accepted physical departure immediately before changing this unit's location.
/// Caller validates that old and new physical locations differ, on its draft.
/// Notify every represented member, including carried members.
/// Cases: land:23.22, land:26.13
pub fn record_departure(state: &mut State, id: &UnitId) -> Result<(), EngineError> {
    let mut next = candidate(state, id)?;
    next.departed = true;
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

#[cfg(test)]
mod integration_tests;

mod capability;
pub use capability::{EngineerCapability, source_capability};

mod companion;
pub use companion::{MinefieldCompanionBenefit, minefield_companion_qualifies};

mod city;
pub use city::source_city_fortification_level;

mod projects;
pub use projects::{ProjectId, begin_facility_project, complete_facility_project};
