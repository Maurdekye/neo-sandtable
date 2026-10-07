//! Whole-squadron Game-Turn planning. Flight and mission permissions are separate.
//! Source cases: airlog:41.0, airlog:39.16, airlog:39.19
use crate::{
    CnaContent, State,
    state::{AirState, Pending},
    steps::{illegal, open},
};
use cna_core::{
    decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    ids::SeatId,
};
use cna_protocol::{Role, Side};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const ANCHOR: &str = "strategic_air.designation";
pub const KIND: &str = "cna.air.designation";
const SIDES: [Side; 2] = [Side::Axis, Side::Commonwealth];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    LandSupport,
    MaltaRaid,
    Convoy,
}

/// Answers remain private and separate from committed assignments.
/// Source cases: airlog:41.0, airlog:39.16
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesignationState {
    pub game_turn: Option<u16>,
    pub closed: BTreeSet<Side>,
    pub waiting: BTreeMap<Side, BTreeMap<String, Family>>,
    pub assignments: BTreeMap<String, Family>,
    pub finished: bool,
}

fn options(side: Side) -> Vec<ChoiceOption> {
    let mut options = vec![
        ChoiceOption {
            id: "land_support".into(),
            label: "Land support this Game-Turn".into(),
            detail: None,
        },
        ChoiceOption {
            id: "convoy".into(),
            label: match side {
                Side::Axis => "Convoy protection this Game-Turn",
                Side::Commonwealth => "Convoy reconnaissance or bombing this Game-Turn",
            }
            .into(),
            detail: None,
        },
    ];
    if side == Side::Axis {
        options.push(ChoiceOption {
            id: "malta_raid".into(),
            label: "Malta raid this Game-Turn".into(),
            detail: None,
        });
    }
    options
}
fn squadrons(air: &AirState, side: Side) -> Vec<String> {
    air.squadrons
        .values()
        .filter(|s| s.side == side && s.force != "malta")
        .map(|s| s.id.clone())
        .collect()
}
fn space(air: &AirState, side: Side) -> ActionSpace {
    ActionSpace::new(ActionSchema::Record {
        fields: squadrons(air, side)
            .into_iter()
            .map(|id| FieldSchema {
                name: id.clone(),
                doc: format!("Planning family for the whole squadron {id}."),
                schema: ActionSchema::Choice {
                    options: options(side),
                },
                optional: false,
            })
            .collect(),
    })
    .with_pass("Assign all these squadrons to land support.")
}

/// Both Air roles get a window, including when their own list is empty.
/// The declaration does not authorize a sortie or inspect enemy eligibility.
/// Source cases: airlog:41.0, airlog:39.16
pub fn enter(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    super::inventory::initialize(content, state)?;
    if state.air.runtime.designation.game_turn == Some(state.cursor.game_turn) {
        return Err(EngineError::Invariant {
            detail: "air designation entered twice in one Game-Turn".into(),
        });
    }
    state.air.runtime.designation = DesignationState {
        game_turn: Some(state.cursor.game_turn),
        ..Default::default()
    };
    for side in SIDES {
        let domain = space(&state.air, side);
        open(state,cx,SeatId::new(side,Role::Air),KIND,
            "Assign each whole squadron to its Game-Turn planning family. Individual missions follow later.".into(),
            &["airlog:41.0","airlog:39.16"],Trigger::Scheduled,Secrecy::SecretSimultaneous,domain);
    }
    Ok(())
}

/// Store only an own-known declaration. Adjudication runs in finish.
/// Source cases: airlog:41.0, airlog:39.16
pub fn answer(state: &mut State, pending: &Pending, action: &Value) -> Result<String, Rejection> {
    let side = pending.seat.side;
    let window = &state.air.runtime.designation;
    if pending.kind != KIND
        || pending.seat.role != Role::Air
        || state.cursor.anchor() != ANCHOR
        || window.game_turn != Some(state.cursor.game_turn)
        || window.finished
        || window.closed.contains(&side)
    {
        return Err(illegal("this Air designation window is not open"));
    }
    space(&state.air, side).check(action).map_err(illegal)?;
    let assignments = if action.is_null() {
        squadrons(&state.air, side)
            .into_iter()
            .map(|id| (id, Family::LandSupport))
            .collect()
    } else {
        serde_json::from_value::<BTreeMap<String, Family>>(action.clone())
            .map_err(|_| illegal("invalid own squadron designation"))?
    };
    state
        .air
        .runtime
        .designation
        .waiting
        .insert(side, assignments);
    state.air.runtime.designation.closed.insert(side);
    Ok("Recorded the private Game-Turn squadron designations.".into())
}

/// Publish a single validated inventory draft after both fixed windows close.
/// Source cases: airlog:41.0, airlog:39.16
pub fn finish(content: &CnaContent, state: &mut State) -> Result<(), EngineError> {
    let window = &state.air.runtime.designation;
    if window.game_turn != Some(state.cursor.game_turn) {
        return Err(EngineError::Invariant {
            detail: "air designation has no current Game-Turn window".into(),
        });
    }
    if window.finished || SIDES.iter().any(|s| !window.closed.contains(s)) {
        return Ok(());
    }
    super::inventory::update(content, &mut state.air, |runtime| {
        runtime.designation.assignments = runtime
            .designation
            .waiting
            .values()
            .flat_map(|m| m.iter().map(|(id, f)| (id.clone(), *f)))
            .collect();
        runtime.designation.waiting.clear();
        runtime.designation.finished = true;
        Ok(())
    })
}

/// Current own assignments for an already-authorized observer.
/// The view layer retains its existing side and Air/Commander detail guards.
pub fn own_report(state: &State, side: Side) -> Value {
    let window = &state.air.runtime.designation;
    if window.game_turn != Some(state.cursor.game_turn) || !window.finished {
        return json!({"game_turn":state.cursor.game_turn,"assignments":{}});
    }
    // The owning observer already has the squadron list. Omit its default
    // families, but retain explicit nulls outside the committed snapshot.
    let overrides: BTreeMap<_, _> = state
        .air
        .squadrons
        .values()
        .filter(|squadron| squadron.side == side)
        .filter_map(|squadron| {
            let family = window.assignments.get(&squadron.id).copied();
            (family != Some(Family::LandSupport)).then_some((&squadron.id, family))
        })
        .collect();
    json!({"game_turn":state.cursor.game_turn,
        "default_family":Family::LandSupport,"overrides":overrides})
}

/// No implicit family is invented for a newly arrived or unplaced squadron.
pub fn squadron_family(state: &State, squadron: &str) -> Option<Family> {
    let window = &state.air.runtime.designation;
    (window.finished && window.game_turn == Some(state.cursor.game_turn))
        .then(|| window.assignments.get(squadron).copied())
        .flatten()
}

#[cfg(test)]
mod tests;
