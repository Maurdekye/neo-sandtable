//! Private tactical intentions. A declaration grants no flight or load entitlement.
//! Missing declaration shapes keep the full profile uniformly Unsupported.

use std::collections::{BTreeMap, BTreeSet};

use cna_core::{
    decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{HexId, SeatId},
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role, Side};
use cna_tables::airlog::air::MissionClass;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    designation::{self, Family},
    inventory,
    state::{PilotId, PlaneId},
};
use crate::{
    CnaContent, State,
    state::Pending,
    steps::{illegal, open},
};

pub const ANCHOR: &str = "opstage.land_support_air.assignment";
pub const KIND: &str = "cna.air.tactical_declarations";
const MISSING: &str = "transfer destinations, transport and airdrop cargo, dual missions, CAP posture, mining payloads, restricted tank strafing, strategic convoy/Malta targets";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TacticalPeriod {
    pub game_turn: u16,
    pub op_stage: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Light {
    Day,
    Night,
}

/// Private period-scoped identity; never a counter or enemy combat label.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MissionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeclarationTarget {
    Hex(HexId),
    ScrambleReserve,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaneDeclaration {
    pub plane: PlaneId,
    pub mode: usize,
    pub rated_pilot: Option<PilotId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionDeclaration {
    pub mission: String,
    pub light: Light,
    pub target: DeclarationTarget,
    pub planes: Vec<PlaneDeclaration>,
}

/// An answers entry means answered; an empty vector means pass. No serial or
/// duplicate closed ledger is needed. New periods discard only old intentions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TacticalState {
    pub period: Option<TacticalPeriod>,
    pub answers: BTreeMap<Side, Vec<MissionDeclaration>>,
    pub declarations: BTreeMap<MissionId, MissionDeclaration>,
    pub resolved: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireTarget {
    hex: Option<HexId>,
    scramble_reserve: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireMission {
    mission: String,
    light: Light,
    target: WireTarget,
    planes: Vec<PlaneDeclaration>,
}

fn invariant(detail: impl Into<String>) -> EngineError {
    EngineError::Invariant {
        detail: detail.into(),
    }
}
fn period(state: &State) -> Result<TacticalPeriod, EngineError> {
    let op_stage = state
        .cursor
        .op_stage
        .filter(|s| (1..=3).contains(s))
        .ok_or_else(|| invariant("tactical declaration has no Operations Stage"))?;
    if state.cursor.game_turn == 0 {
        return Err(invariant("zero tactical Game-Turn"));
    }
    Ok(TacticalPeriod {
        game_turn: state.cursor.game_turn,
        op_stage,
    })
}
fn owned(force: &str, side: Side) -> bool {
    matches!(
        (force, side),
        ("axis", Side::Axis) | ("commonwealth" | "malta", Side::Commonwealth)
    )
}
fn own_planes(state: &State, side: Side) -> Vec<PlaneId> {
    state
        .air
        .runtime
        .aircraft
        .iter()
        .filter(|(_, p)| {
            owned(&p.force, side)
                && p.squadron.as_deref().is_some_and(|s| {
                    designation::squadron_family(state, s) == Some(Family::LandSupport)
                })
        })
        .map(|(id, _)| id.clone())
        .collect()
}
fn supported(name: &str) -> bool {
    !matches!(
        name,
        "transfer"
            | "transport"
            | "airdrop"
            | "mining_harbors"
            | "combat_air_patrol_offensive_defensive"
            | "strafe_tanks"
    )
}
fn choice(ids: impl IntoIterator<Item = String>) -> ActionSchema {
    ActionSchema::Choice {
        options: ids
            .into_iter()
            .map(|id| ChoiceOption {
                label: id.clone(),
                id,
                detail: None,
            })
            .collect(),
    }
}
fn field(name: &str, doc: &str, schema: ActionSchema, optional: bool) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        doc: doc.into(),
        schema,
        optional,
    }
}
fn bound(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
fn space(content: &CnaContent, state: &State, side: Side) -> ActionSpace {
    let planes = own_planes(state, side);
    // A structural upper bound only; exact per-plane mode validation is own-known.
    let mode_max = planes
        .iter()
        .filter_map(|id| {
            content
                .units
                .aircraft
                .get(&state.air.runtime.aircraft[id].aircraft)
        })
        .map(|a| a.modes.len().saturating_sub(1))
        .max()
        .unwrap_or(0);
    let item = ActionSchema::Record {
        fields: vec![
            field(
                "mission",
                "One implemented land-support intention.",
                choice(
                    content
                        .tables
                        .airlog
                        .mission_summary
                        .missions()
                        .iter()
                        .filter(|m| m.class.is_some() && supported(&m.mission))
                        .map(|m| m.mission.clone()),
                ),
                false,
            ),
            field(
                "light",
                "Day or night; this does not grant night capability.",
                choice(["day".into(), "night".into()]),
                false,
            ),
            field(
                "target",
                "Use a hex, or reserve without a hex for scramble.",
                ActionSchema::Record {
                    fields: vec![
                        field(
                            "hex",
                            "Blind target hex; absent for scramble reserve.",
                            ActionSchema::Hex { among: None },
                            true,
                        ),
                        field(
                            "scramble_reserve",
                            "True only for a scramble reserve.",
                            ActionSchema::Bool,
                            false,
                        ),
                    ],
                },
                false,
            ),
            field(
                "planes",
                "Each own plane and rated pilot may occur once in the plan.",
                ActionSchema::List {
                    min: 1,
                    max: bound(planes.len()),
                    item: Box::new(ActionSchema::Record {
                        fields: vec![
                            field(
                                "plane",
                                "Own aircraft in a committed land-support squadron.",
                                choice(planes.iter().map(|p| p.0.clone())),
                                false,
                            ),
                            field(
                                "mode",
                                "Existing aircraft content mode, not a paid load.",
                                ActionSchema::Integer {
                                    min: 0,
                                    max: i64::try_from(mode_max).unwrap_or(i64::MAX),
                                },
                                false,
                            ),
                            field(
                                "rated_pilot",
                                "Optional own rated-pilot intention; no availability or training granted.",
                                choice(
                                    state
                                        .air
                                        .runtime
                                        .pilots
                                        .iter()
                                        .filter(|(_, p)| owned(&p.force, side))
                                        .map(|(id, _)| id.0.clone()),
                                ),
                                true,
                            ),
                        ],
                    }),
                },
                false,
            ),
        ],
    };
    ActionSpace::new(ActionSchema::List {
        item: Box::new(item),
        min: 0,
        max: bound(planes.len()),
    })
    .with_pass("Declare no tactical missions in this Operations Stage.")
}

/// Both private windows exist regardless of hidden eligibility. Full stops
/// before inspecting inventory while legal declaration shapes remain missing.
/// This development slice does not deploy, disclose or authorize a flight.
/// Cases: airlog:39.12, airlog:39.16
pub fn enter(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    if strict {
        return Err(EngineError::Unsupported {
            case: "airlog:39.12".into(),
            detail: format!("Tactical declaration shapes missing: {MISSING}"),
        });
    }
    let current = period(state)?;
    if state.air.runtime.tactical.period.is_some_and(|previous| {
        (current.game_turn, current.op_stage) <= (previous.game_turn, previous.op_stage)
    }) {
        return Err(invariant(
            "tactical declaration period cannot repeat or move backward",
        ));
    }
    state.air.runtime.tactical = TacticalState {
        period: Some(current),
        ..Default::default()
    };
    for side in Side::ALL {
        cx.emit(EngineEvent::new(Audience::Side(side), GameEvent::Note {
            text: format!("Development tactical declarations omit: {MISSING}. Declarations grant no flight or load."),
        }));
        let domain = space(content, state, side);
        open(state, cx, SeatId::new(side, Role::Air), KIND,
            "Declare private tactical intentions. Aircraft remain on the ground; no supplies or readiness are consumed.".into(),
            &["airlog:39.12", "airlog:39.16"], Trigger::Scheduled, Secrecy::SecretSimultaneous, domain);
    }
    Ok(())
}

/// Acceptance uses own identities, content and public hexes only; source
/// capability, dice, flight and all enemy constraints are not adjudicated here.
/// Cases: airlog:39.12, airlog:39.16
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
) -> Result<String, Rejection> {
    let side = pending.seat.side;
    let current = period(state).map_err(|_| illegal("no tactical declaration period"))?;
    let window = &state.air.runtime.tactical;
    if pending.kind != KIND
        || pending.seat.role != Role::Air
        || state.cursor.anchor() != ANCHOR
        || window.period != Some(current)
        || window.resolved
        || window.answers.contains_key(&side)
    {
        return Err(illegal("this tactical declaration window is not open"));
    }
    space(content, state, side).check(action).map_err(illegal)?;
    let wire: Vec<WireMission> = if action.is_null() {
        Vec::new()
    } else {
        serde_json::from_value(action.clone())
            .map_err(|_| illegal("invalid tactical declaration shape"))?
    };
    let mut planes = BTreeSet::new();
    let mut pilots = BTreeSet::new();
    let mut declarations = Vec::new();
    for m in wire {
        let target = match (m.target.hex, m.target.scramble_reserve, m.mission.as_str()) {
            (None, true, "scramble") => DeclarationTarget::ScrambleReserve,
            (Some(hex), false, name) if name != "scramble" => DeclarationTarget::Hex(
                content
                    .map
                    .canonical(&hex)
                    .cloned()
                    .ok_or_else(|| illegal("not a map hex"))?,
            ),
            _ => {
                return Err(illegal(
                    "use only a hex target, or a scramble reserve without a hex",
                ));
            }
        };
        for selected in &m.planes {
            // Schema has already authorized these own IDs, before any lookup.
            let p = &state.air.runtime.aircraft[&selected.plane];
            if !planes.insert(selected.plane.clone())
                || selected
                    .rated_pilot
                    .as_ref()
                    .is_some_and(|p| !pilots.insert(p.clone()))
            {
                return Err(illegal("a plane or rated pilot occurs more than once"));
            }
            if content
                .units
                .aircraft
                .get(&p.aircraft)
                .is_some_and(|a| selected.mode >= a.modes.len())
            {
                return Err(illegal("not a mode of this own aircraft"));
            }
        }
        declarations.push(MissionDeclaration {
            mission: m.mission,
            light: m.light,
            target,
            planes: m.planes,
        });
    }
    state
        .air
        .runtime
        .tactical
        .answers
        .insert(side, declarations);
    Ok("Recorded private tactical intentions; no flight or load was authorized.".into())
}

fn validate(
    content: &CnaContent,
    state: &State,
    side: Side,
    plan: &[MissionDeclaration],
) -> Result<(), EngineError> {
    let own: BTreeSet<_> = own_planes(state, side).into_iter().collect();
    let mut planes = BTreeSet::new();
    let mut pilots = BTreeSet::new();
    for m in plan {
        let row = content
            .tables
            .airlog
            .mission_summary
            .land_support(&m.mission)
            .filter(|m| supported(&m.mission))
            .ok_or_else(|| invariant("unknown committed tactical mission"))?;
        if m.planes.is_empty() || (m.light == Light::Night && !row.night) {
            return Err(invariant("empty mission or unsupported night declaration"));
        }
        match (&m.target, m.mission.as_str()) {
            (DeclarationTarget::ScrambleReserve, "scramble") => {}
            (DeclarationTarget::Hex(hex), name)
                if name != "scramble" && content.map.canonical(hex) == Some(hex) => {}
            _ => return Err(invariant("invalid canonical tactical target")),
        }
        for selected in &m.planes {
            if !own.contains(&selected.plane) || !planes.insert(&selected.plane) {
                return Err(invariant(
                    "declaration aircraft no longer has an own land-support assignment",
                ));
            }
            if let Some(id) = &selected.rated_pilot
                && (!state
                    .air
                    .runtime
                    .pilots
                    .get(id)
                    .is_some_and(|p| owned(&p.force, side))
                    || !pilots.insert(id))
            {
                return Err(invariant("invalid own pilot declaration"));
            }
            let plane = &state.air.runtime.aircraft[&selected.plane];
            let aircraft = content.units.aircraft.get(&plane.aircraft).ok_or_else(|| {
                EngineError::Unsupported {
                    case: "airlog:34.6".into(),
                    detail: "Declared aircraft has no source characteristics".into(),
                }
            })?;
            let mode = aircraft
                .modes
                .get(selected.mode)
                .ok_or_else(|| invariant("invalid declared aircraft mode"))?;
            let letters: &[&str] = match (m.mission.as_str(), row.class) {
                ("scramble", _) => &["s"],
                ("recon_land_units", _) => &["r"],
                (_, Some(MissionClass::Fighter)) => &["f", "d"],
                (_, Some(MissionClass::Bombing)) => &["b", "d"],
                _ => return Err(invariant("unsupported tactical capability category")),
            };
            let mut capable = false;
            for code in letters
                .iter()
                .filter_map(|letter| mode.missions.get(*letter))
            {
                capable |= match (m.light, code.as_str()) {
                    (Light::Day, "day" | "night") => true,
                    (Light::Day, "strafe_only") => row.class == Some(MissionClass::Fighter),
                    (Light::Night, "night" | "night_only") => true,
                    (Light::Day, "night_only") | (Light::Night, "day" | "strafe_only") => false,
                    _ => {
                        return Err(EngineError::Unsupported {
                            case: "airlog:34.6".into(),
                            detail: "Unrecognized source capability notation".into(),
                        });
                    }
                };
            }
            if !capable {
                return Err(invariant("aircraft mode lacks declared mission capability"));
            }
        }
    }
    Ok(())
}

/// Commit intentions once, after both answers, via the canonical update path.
/// No flight, pilot assignment, preparation, sortie use or readiness is changed.
/// Cases: airlog:39.12, airlog:39.16
pub fn finish(content: &CnaContent, state: &mut State) -> Result<(), EngineError> {
    let current = period(state)?;
    let window = &state.air.runtime.tactical;
    if window.period != Some(current) {
        return Err(invariant("tactical finish period mismatch"));
    }
    if window.resolved
        || Side::ALL
            .iter()
            .any(|side| !window.answers.contains_key(side))
    {
        return Ok(());
    }
    for side in Side::ALL {
        validate(content, state, side, &window.answers[&side])?;
    }
    inventory::update(content, &mut state.air, |runtime| {
        let window = &mut runtime.tactical;
        window.declarations = window
            .answers
            .iter()
            .flat_map(|(side, plan)| {
                plan.iter().enumerate().map(move |(i, m)| {
                    (
                        MissionId(format!(
                            "{side}-gt{}-op{}-{i}",
                            current.game_turn, current.op_stage
                        )),
                        m.clone(),
                    )
                })
            })
            .collect();
        window.resolved = true;
        Ok(())
    })
}

#[cfg(test)]
mod tests;
