//! Private SGSU observations. Actual CP/travel/applied-pin producers remain unwired.
//! Cases: airlog:35.11, airlog:35.12, land:24.22, land:24.23, land:24.72
use std::collections::BTreeMap;

use cna_core::engine::EngineError;
use cna_protocol::Side;
use serde::{Deserialize, Serialize};

use super::{SgsuId, SgsuOpStage};
use crate::air::state::AirRuntime;
use crate::land::engineering::{self, ActivityStage};
use crate::state::Location;
use crate::{CnaContent, State};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SgsuActivityState {
    pub(crate) current: Option<SgsuStageRecords>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) previous_closed: Option<SgsuStageRecords>,
}
impl SgsuActivityState {
    pub(crate) fn is_default(&self) -> bool {
        self.current.is_none() && self.previous_closed.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SgsuStageRecords {
    pub(crate) stage: SgsuOpStage,
    pub(crate) observed_opening: bool,
    pub(crate) closed: bool,
    pub(crate) entries: BTreeMap<SgsuId, SgsuActivityEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SgsuActivityEntry {
    pub owner: Side,
    pub opening_location: Option<Location>,
    pub closing_location: Option<Location>,
    pub gross_cp_quarters: i32,
    pub departed: bool,
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SgsuEvidence {
    Unknown {
        reason: SgsuEvidenceGap,
        recorded: Option<SgsuActivityEntry>,
    },
    Complete(SgsuActivityEntry),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SgsuEvidenceGap {
    UnobservedOpening,
    NotClosed,
    UnretainedStage,
    IntroducedDuringStage,
    IncompleteProducerCoverage,
}

fn invalid(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("air SGSU activity: {detail}"),
    }
}
fn key(stage: SgsuOpStage) -> (u16, u8) {
    (stage.game_turn, stage.op_stage)
}
fn label(stage: SgsuOpStage) -> ActivityStage {
    ActivityStage {
        game_turn: stage.game_turn,
        op_stage: stage.op_stage,
    }
}
fn validate_stage(stage: SgsuOpStage) -> Result<(), EngineError> {
    if stage.game_turn == 0 || !(1..=3).contains(&stage.op_stage) {
        return Err(invalid("invalid Operations Stage label"));
    }
    Ok(())
}
pub(crate) fn cursor_stage(state: &State) -> Result<SgsuOpStage, EngineError> {
    let stage = SgsuOpStage {
        game_turn: state.cursor.game_turn,
        op_stage: state
            .cursor
            .op_stage
            .ok_or_else(|| invalid("missing Operations Stage"))?,
    };
    validate_stage(stage)?;
    Ok(stage)
}
fn physical(content: &CnaContent, at: &Location) -> Result<(), EngineError> {
    match at {
        Location::Hex { hex } if content.map.canonical(hex) == Some(hex) => Ok(()),
        Location::OffMap { id }
            if content
                .areas
                .locations
                .get(id)
                .is_some_and(|site| site.off_map) =>
        {
            Ok(())
        }
        _ => Err(invalid("invalid physical historical observation")),
    }
}
fn validate_entry(
    content: &CnaContent,
    id: &SgsuId,
    entry: &SgsuActivityEntry,
) -> Result<(), EngineError> {
    if id.0.is_empty() || entry.gross_cp_quarters < 0 {
        return Err(invalid("corrupt selected activity"));
    }
    for at in [
        entry.opening_location.as_ref(),
        entry.closing_location.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        physical(content, at)?;
    }
    Ok(())
}
fn validate_records(
    content: &CnaContent,
    records: &SgsuStageRecords,
    end: SgsuOpStage,
) -> Result<(), EngineError> {
    validate_stage(records.stage)?;
    if key(records.stage) > key(end) {
        return Err(invalid("future recorded interval"));
    }
    for (id, entry) in &records.entries {
        validate_entry(content, id, entry)?;
    }
    if !records.closed
        && records
            .entries
            .values()
            .any(|entry| entry.closing_location.is_some())
    {
        return Err(invalid("unclosed interval contains a final observation"));
    }
    Ok(())
}
// Container semantics do not bound a newer current interval by an older query.
fn validate_container(history: &SgsuActivityState) -> Result<(), EngineError> {
    if let Some(previous) = &history.previous_closed {
        if !previous.closed {
            return Err(invalid("retained interval is not closed"));
        }
    }
    if let Some(current) = &history.current {
        if current.closed && history.previous_closed.as_ref() != Some(current) {
            return Err(invalid(
                "closed current interval differs from immutable retained interval",
            ));
        }
        if let Some(previous) = &history.previous_closed {
            if key(previous.stage) > key(current.stage) {
                return Err(invalid("retained interval is later than current history"));
            }
        }
    }
    Ok(())
}
fn validate_history(
    content: &CnaContent,
    history: &SgsuActivityState,
    end: SgsuOpStage,
) -> Result<(), EngineError> {
    if let Some(current) = &history.current {
        validate_records(content, current, end)?;
    }
    if let Some(previous) = &history.previous_closed {
        validate_records(content, previous, end)?;
    }
    validate_container(history)
}

/// Observe actual canonical IDs once; no import or retrospective history.
/// Cases: airlog:35.11, land:24.22, land:24.23
pub fn open_sgsu_stage(
    content: &CnaContent,
    state: &mut State,
    stage: SgsuOpStage,
) -> Result<(), EngineError> {
    validate_stage(stage)?;
    if cursor_stage(state)? != stage
        || state.cursor.block != crate::seq::Block::OpStage
        || state.cursor.index != 0
        || state.cursor.entered
    {
        return Err(invalid("opening outside the real public boundary"));
    }
    if state.air.runtime.bases_initialized {
        crate::air::inventory::check(content, &state.air)?;
    } else if !state.air.runtime.sgsus.is_empty() || !state.air.runtime.facilities.is_empty() {
        return Err(invalid("uninitialized canonical base maps contain records"));
    }
    let history = &state.air.runtime.sgsu_activity;
    validate_history(content, history, stage)?;
    if let Some(current) = &history.current {
        if current.stage == stage {
            if current.closed {
                return Err(invalid("rewound opening of a closed interval"));
            }
            return Ok(());
        }
        if !current.closed || history.previous_closed.as_ref() != Some(current) {
            return Err(EngineError::Unsupported {
                case: "land:24.11".into(),
                detail: "older SGSU interval has no authenticated closure".into(),
            });
        }
    }
    let observed_opening = state.air.runtime.bases_initialized;
    let mut entries = BTreeMap::new();
    if observed_opening {
        for (id, unit) in &state.air.runtime.sgsus {
            entries.insert(
                id.clone(),
                SgsuActivityEntry {
                    owner: unit.side()?,
                    opening_location: Some(unit.location(content, &state.air.runtime.facilities)?),
                    closing_location: None,
                    gross_cp_quarters: 0,
                    departed: false,
                    pinned: false,
                },
            );
        }
    }
    state.air.runtime.sgsu_activity.current = Some(SgsuStageRecords {
        stage,
        observed_opening,
        closed: false,
        entries,
    });
    Ok(())
}

fn retirement(state: &State, prior: Option<&SgsuStageRecords>) -> Result<(), EngineError> {
    match (prior, state.engineering.closed_activity.as_ref()) {
        (None, None) => Ok(()),
        (None, Some(land)) => engineering::validate_activity_retirement(state, land.stage),
        (Some(air), _) => engineering::validate_activity_retirement(state, label(air.stage)),
    }
}

/// Validate the ORIGINAL pair before overwrite; Engineering later reads the NEW close.
/// Cases: land:24.11, land:24.22, land:24.23, airlog:35.11
pub fn capture_sgsu_stage(
    content: &CnaContent,
    state: &mut State,
    stage: SgsuOpStage,
) -> Result<(), EngineError> {
    validate_stage(stage)?;
    if engineering::closing_stage(state)? != label(stage) {
        return Err(invalid("old-tail stage mismatch"));
    }
    if state.air.runtime.bases_initialized {
        crate::air::inventory::check(content, &state.air)?;
    } else if !state.air.runtime.sgsus.is_empty() || !state.air.runtime.facilities.is_empty() {
        return Err(invalid("uninitialized canonical base maps contain records"));
    }
    // The exact Engineering close signature has no content argument. Validate its
    // physical source observations here, before either new retained assignment.
    for entry in state.engineering.activity.values() {
        if let Some(at) = &entry.opening_location {
            physical(content, at)?;
        }
    }
    if let Some(closed) = &state.engineering.closed_activity {
        for entry in closed.entries.values() {
            for at in [
                entry.activity.opening_location.as_ref(),
                entry.closing_location.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                physical(content, at)?;
            }
        }
    }
    let history = &state.air.runtime.sgsu_activity;
    validate_history(content, history, stage)?;
    let mut snapshot = match &history.current {
        Some(current) if current.stage == stage => current.clone(),
        Some(current) if !current.closed => {
            return Err(EngineError::Unsupported {
                case: "land:24.11".into(),
                detail: "unclosed older SGSU interval would be discarded".into(),
            });
        }
        _ => SgsuStageRecords {
            stage,
            observed_opening: false,
            closed: false,
            entries: BTreeMap::new(),
        },
    };
    // Historical removed IDs survive; current new IDs never gain an opening witness.
    for (id, entry) in &mut snapshot.entries {
        entry.closing_location = match state.air.runtime.sgsus.get(id) {
            Some(unit) => {
                if unit.side()? != entry.owner {
                    return Err(invalid("historical owner was rewritten"));
                }
                Some(unit.location(content, &state.air.runtime.facilities)?)
            }
            None => None,
        };
    }
    for (id, unit) in &state.air.runtime.sgsus {
        if !snapshot.entries.contains_key(id) {
            snapshot.entries.insert(
                id.clone(),
                SgsuActivityEntry {
                    owner: unit.side()?,
                    opening_location: None,
                    closing_location: Some(unit.location(content, &state.air.runtime.facilities)?),
                    gross_cp_quarters: 0,
                    departed: false,
                    pinned: false,
                },
            );
        }
    }
    snapshot.closed = true;
    validate_records(content, &snapshot, stage)?;
    if let Some(previous) = &history.previous_closed {
        if previous.stage == stage {
            if previous != &snapshot {
                return Err(invalid("same-stage closure would rewrite history"));
            }
            return Ok(());
        }
    }
    retirement(state, history.previous_closed.as_ref())?;
    state.air.runtime.sgsu_activity.current = Some(snapshot.clone());
    state.air.runtime.sgsu_activity.previous_closed = Some(snapshot);
    Ok(())
}

/// Trusted implementation coverage is incomplete; never certify a quiet stage.
/// Cases: land:24.22, land:24.23, land:24.72, airlog:35.12
pub fn stage_evidence(
    content: &CnaContent,
    state: &State,
    owner: Side,
    id: &SgsuId,
    stage: SgsuOpStage,
) -> Result<SgsuEvidence, EngineError> {
    validate_stage(stage)?;
    if stage.game_turn > state.cursor.game_turn
        || (stage.game_turn == state.cursor.game_turn
            && state
                .cursor
                .op_stage
                .is_some_and(|op_stage| stage.op_stage > op_stage))
    {
        return Err(invalid("selected interval is in the future"));
    }
    let history = &state.air.runtime.sgsu_activity;
    validate_container(history)?;
    let selected = history
        .previous_closed
        .as_ref()
        .filter(|records| records.stage == stage)
        .or_else(|| {
            history
                .current
                .as_ref()
                .filter(|records| records.stage == stage)
        });
    let Some(records) = selected else {
        return Ok(SgsuEvidence::Unknown {
            reason: SgsuEvidenceGap::UnretainedStage,
            recorded: None,
        });
    };
    validate_records(content, records, stage)?;
    let recorded = records.entries.get(id).cloned();
    if let Some(entry) = &recorded {
        if entry.owner != owner {
            return Err(invalid("unauthorized selected history"));
        }
    }
    // Actual SGSU CP/travel/applied-pin procedures do not exist in this source slice.
    // This fixed trusted gap is neither saved state nor answer-supplied coverage.
    Ok(SgsuEvidence::Unknown {
        reason: SgsuEvidenceGap::IncompleteProducerCoverage,
        recorded,
    })
}

/// Canonical callback backstop, not coverage of intermediate travel or CP/pin.
/// Cases: airlog:35.11, airlog:35.12, land:24.22, land:24.23
pub(crate) fn check_and_record_transition(
    content: &CnaContent,
    old: &AirRuntime,
    new: &mut AirRuntime,
) -> Result<(), EngineError> {
    if old.sgsu_activity != new.sgsu_activity {
        return Err(invalid("canonical callback rewrote private history"));
    }
    let Some(current) = &old.sgsu_activity.current else {
        return Ok(());
    };
    validate_history(content, &old.sgsu_activity, current.stage)?;
    if current.closed {
        // Strategic-phase mutations do not append to the completed interval.
        return Ok(());
    }
    let mut next = current.clone();
    for (id, unit) in &new.sgsus {
        let to = unit.location(content, &new.facilities)?;
        if let Some(before) = old.sgsus.get(id) {
            if before.side()? != unit.side()? {
                return Err(invalid("canonical SGSU changed owner"));
            }
            let from = before.location(content, &old.facilities)?;
            if from != to {
                let entry = next.entries.entry(id.clone()).or_insert(SgsuActivityEntry {
                    owner: unit.side()?,
                    opening_location: None,
                    closing_location: None,
                    gross_cp_quarters: 0,
                    departed: false,
                    pinned: false,
                });
                entry.departed = true;
            }
        } else {
            next.entries.entry(id.clone()).or_insert(SgsuActivityEntry {
                owner: unit.side()?,
                opening_location: None,
                closing_location: None,
                gross_cp_quarters: 0,
                departed: false,
                pinned: false,
            });
        }
    }
    new.sgsu_activity.current = Some(next);
    Ok(())
}

#[cfg(test)]
mod tests;
