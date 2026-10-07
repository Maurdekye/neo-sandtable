//! Initial squadrons and pilot rosters, without publishing either side's air choices.
use super::{SetupTask, decisions, facilities};
use crate::{
    CnaContent, State,
    state::{AirSquadron, Pending},
    steps::illegal,
};
use cna_content::scenario::PlaneSetup;
use cna_core::{
    decision::{ActionSchema, ActionSpace, ChoiceOption},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::SeatId,
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role, Side};
use cna_tables::airlog::air::SquadronKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
pub(super) const KIND: &str = "cna.setup.air";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Phase {
    Planes { aircraft: String, ready: bool },
    Sgsu { aircraft: Option<String> },
    Pilots { rating: u8 },
}
#[derive(Clone)]
struct Pick {
    id: String,
    squadron: Option<String>,
    facility: String,
    count: i32,
}
fn side(force: &str) -> Side {
    if force == "axis" {
        Side::Axis
    } else {
        Side::Commonwealth
    }
}
fn nationality(force: &str) -> &'static str {
    if force == "axis" { "it" } else { "cw" }
}
fn invariant(s: &str) -> EngineError {
    EngineError::Invariant { detail: s.into() }
}
fn rows<'a>(content: &'a CnaContent, force: &str) -> Vec<&'a PlaneSetup> {
    if force == "malta" {
        content
            .scenario
            .air
            .iter()
            .filter_map(|f| f.malta.as_ref())
            .flat_map(|m| &m.planes)
            .collect()
    } else {
        content
            .scenario
            .air
            .iter()
            .filter(|f| f.force.side == side(force))
            .flat_map(|f| &f.planes)
            .collect()
    }
}
fn quota(content: &CnaContent, force: &str, aircraft: &str) -> Option<i32> {
    rows(content, force)
        .into_iter()
        .find(|r| r.aircraft == aircraft)
        .and_then(|r| r.sgsu)
}
fn capacity(content: &CnaContent, nation: &str) -> i32 {
    let k = match nation {
        "it" => SquadronKind::ItalianSquadriglia,
        "ge" => SquadronKind::GermanStaffel,
        _ => SquadronKind::CommonwealthSquadron194041,
    };
    content.tables.airlog.squadron_capacity.capacity(k).total
}
fn mask(role: &str) -> u8 {
    match role {
        "fighter" => 1,
        "bomber" | "flying_boat" => 2,
        "transport" => 4,
        "recon" | "reconnaissance" => 7,
        "fighter_bomber" => 3,
        _ => 0,
    }
}
/// Cases: airlog:35.21, airlog:35.23, airlog:35.26, airlog:35.28, scen:60.42, scen:60.46
fn compatible(content: &CnaContent, squadron: &AirSquadron, aircraft: &str) -> bool {
    if squadron
        .initial_aircraft
        .as_deref()
        .is_some_and(|a| a != aircraft)
    {
        return false;
    }
    let Some(new) = content.units.aircraft.get(aircraft) else {
        return false;
    };
    if squadron.nationality == "ge" && new.nation == "it" {
        return false;
    }
    let present: Vec<_> = squadron
        .planes
        .iter()
        .filter(|(_, p)| p.total > 0)
        .map(|(id, _)| id.as_str())
        .collect();
    if present.iter().any(|id| {
        matches!(content.units.aircraft[*id].nation.as_str(), "it" | "ge")
            && content.units.aircraft[*id].nation != new.nation
    }) {
        return false;
    }
    let common = present.iter().fold(mask(&new.role), |m, id| {
        m & mask(&content.units.aircraft[*id].role)
    });
    if common != 0 {
        return true;
    }
    let allowed = rows(content, &squadron.force)
        .into_iter()
        .find(|r| r.aircraft == aircraft);
    allowed.is_some_and(|r| {
        present
            .iter()
            .all(|id| *id == aircraft || r.composition_exception_with.iter().any(|a| a == id))
    })
}
fn has_room(state: &State, f: &facilities::Facility) -> bool {
    f.limit.is_none_or(|limit| {
        state
            .air
            .squadrons
            .values()
            .filter(|s| s.facility == f.id)
            .count()
            < usize::try_from(limit).unwrap_or(0)
    })
}
fn add_counts(
    out: &mut Vec<Pick>,
    base: String,
    squadron: Option<String>,
    facility: String,
    max: i32,
) {
    for count in (1..=max).rev() {
        out.push(Pick {
            id: format!("{base}/{count}"),
            squadron: squadron.clone(),
            facility: facility.clone(),
            count,
        });
    }
}
/// Cases: scen:59.33, scen:59.34, scen:59.35, airlog:35.21, airlog:35.23, airlog:35.28, airlog:36.12, airlog:36.3, airlog:36.4
fn picks(
    content: &CnaContent,
    state: &State,
    force: &str,
    phase: &Phase,
) -> Result<Vec<Pick>, EngineError> {
    let catalog = facilities::catalog(content)?;
    let mut out = vec![];
    let data = state
        .air
        .forces
        .get(force)
        .ok_or_else(|| invariant("unknown initial air force"))?;
    match phase {
        Phase::Planes { aircraft, ready } => {
            let def = content
                .units
                .aircraft
                .get(aircraft)
                .ok_or_else(|| invariant("unknown initial aircraft"))?;
            let n = data
                .planes
                .get(aircraft)
                .map_or(0, |p| if *ready { p.ready } else { p.total - p.ready });
            for s in state
                .air
                .squadrons
                .values()
                .filter(|s| s.force == force && compatible(content, s, aircraft))
            {
                let Some(f) = catalog.facilities.iter().find(|f| f.id == s.facility) else {
                    continue;
                };
                if !facilities::compatible(f, def.role == "flying_boat") {
                    continue;
                }
                let used: i32 = s.planes.values().map(|p| p.total).sum();
                add_counts(
                    &mut out,
                    format!("squadron:{}", s.id),
                    Some(s.id.clone()),
                    s.facility.clone(),
                    n.min(capacity(content, &s.nationality) - used),
                );
            }
            let assigned = state
                .air
                .squadrons
                .values()
                .filter(|s| s.force == force && s.initial_aircraft.as_deref() == Some(aircraft))
                .count();
            let can_create = data.sgsu_available > 0
                && quota(content, force, aircraft)
                    .is_none_or(|q| assigned < usize::try_from(q).unwrap_or(0));
            if can_create {
                for f in catalog.facilities.iter().filter(|f| {
                    f.force == force
                        && has_room(state, f)
                        && facilities::compatible(f, def.role == "flying_boat")
                        && !(force == "axis"
                            && matches!(f.location, crate::state::Location::OffMap { .. }))
                }) {
                    add_counts(
                        &mut out,
                        format!("new:{}", f.id),
                        None,
                        f.id.clone(),
                        n.min(capacity(content, nationality(force))),
                    );
                }
            }
        }
        Phase::Sgsu { aircraft } => {
            if data.sgsu_available > 0 {
                for f in catalog.facilities.iter().filter(|f| {
                    f.force == force
                        && has_room(state, f)
                        && aircraft.as_ref().is_none_or(|a| {
                            facilities::compatible(
                                f,
                                content.units.aircraft[a].role == "flying_boat",
                            )
                        })
                }) {
                    out.push(Pick {
                        id: f.id.clone(),
                        squadron: None,
                        facility: f.id.clone(),
                        count: 1,
                    });
                }
            }
        }
        Phase::Pilots { rating } => {
            let n = data.pilots.get(rating).copied().unwrap_or(0);
            for s in state.air.squadrons.values().filter(|s| s.force == force) {
                add_counts(
                    &mut out,
                    format!("squadron:{}", s.id),
                    Some(s.id.clone()),
                    s.facility.clone(),
                    n,
                );
            }
        }
    }
    Ok(out)
}
fn phase(content: &CnaContent, state: &State, force: &str) -> Option<Phase> {
    let data = &state.air.forces[force];
    for (aircraft, p) in &data.planes {
        if p.ready > 0 {
            return Some(Phase::Planes {
                aircraft: aircraft.clone(),
                ready: true,
            });
        }
        if p.total > 0 {
            return Some(Phase::Planes {
                aircraft: aircraft.clone(),
                ready: false,
            });
        }
    }
    if data.sgsu_available > 0 {
        let aircraft = rows(content, force)
            .into_iter()
            .find(|r| {
                r.sgsu.is_some_and(|n| {
                    state
                        .air
                        .squadrons
                        .values()
                        .filter(|s| {
                            s.force == force && s.initial_aircraft.as_deref() == Some(&r.aircraft)
                        })
                        .count()
                        < usize::try_from(n).unwrap_or(0)
                })
            })
            .map(|r| r.aircraft.clone());
        return Some(Phase::Sgsu { aircraft });
    }
    data.pilots
        .iter()
        .find(|(_, n)| **n > 0)
        .map(|(rating, _)| Phase::Pilots { rating: *rating })
}
fn next(
    content: &CnaContent,
    state: &mut State,
    force: &str,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    if state.setup.air_unavailable.contains(force) {
        return Ok(());
    }
    let Some(phase) = phase(content, state, force) else {
        return Ok(());
    };
    let choices = picks(content, state, force, &phase)?;
    let case = match phase {
        Phase::Planes { .. } => "scen:59.33",
        Phase::Sgsu { .. } => "scen:59.35",
        Phase::Pilots { .. } => "scen:59.34",
    };
    if choices.is_empty() {
        if strict {
            return Err(EngineError::Unsupported {
                case: case.into(),
                detail: format!("{force}: no capacity/composition-valid initial air destination"),
            });
        }
        state.setup.air_unavailable.insert(force.into());
        cx.emit(EngineEvent::new(Audience::Seat(SeatId::new(side(force),Role::Air)),GameEvent::Note{text:format!("{force} has air assets awaiting placement: no verified destination meets capacity/composition ({case}).")}));
        return Ok(());
    }
    let options = choices
        .iter()
        .map(|p| ChoiceOption {
            id: p.id.clone(),
            label: format!(
                "{}; {} {}",
                p.squadron.as_deref().unwrap_or(&p.facility),
                p.count,
                match phase {
                    Phase::Planes { .. } => "planes",
                    Phase::Sgsu { .. } => "SGSU",
                    Phase::Pilots { .. } => "pilots",
                }
            ),
            detail: Some(format!("Facility {}", p.facility)),
        })
        .collect();
    decisions::open_task(
        state,
        cx,
        SeatId::new(side(force), Role::Air),
        KIND,
        format!("Place initial {force} air assets."),
        case,
        SetupTask::Air {
            force: force.into(),
            phase,
        },
        ActionSpace::new(ActionSchema::Choice { options }),
    );
    Ok(())
}
/// Cases: scen:59.31, scen:59.32, scen:59.33, scen:59.34, scen:59.35, scen:60.32, scen:60.42, scen:60.46
pub(super) fn start(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    if state.setup.air_started {
        return Ok(());
    }
    state.setup.air_started = true;
    facilities::report(content, strict, cx)?;
    let forces: Vec<_> = state.air.forces.keys().cloned().collect();
    for force in forces {
        next(content, state, &force, strict, cx)?;
    }
    Ok(())
}
fn create(
    content: &CnaContent,
    state: &mut State,
    force: &str,
    facility: &str,
    aircraft: Option<String>,
) -> Result<String, EngineError> {
    let catalog = facilities::catalog(content)?;
    let f = catalog
        .facilities
        .iter()
        .find(|f| f.id == facility && f.force == force)
        .ok_or_else(|| invariant("unknown initial facility"))?;
    if !has_room(state, f) || state.air.forces[force].sgsu_available <= 0 {
        return Err(invariant("initial SGSU capacity exhausted"));
    }
    let serial = state.air.squadron_serial.entry(force.into()).or_default();
    *serial = serial
        .checked_add(1)
        .ok_or_else(|| invariant("squadron identity exhausted"))?;
    let id = format!("{force}.sgsu-{serial}");
    if state.air.squadrons.contains_key(&id) {
        return Err(invariant("squadron id reused"));
    }
    state
        .air
        .forces
        .get_mut(force)
        .expect("force")
        .sgsu_available -= 1;
    state.air.squadrons.insert(
        id.clone(),
        AirSquadron {
            id: id.clone(),
            force: force.into(),
            side: side(force),
            nationality: nationality(force).into(),
            facility: facility.into(),
            initial_aircraft: aircraft,
            planes: BTreeMap::new(),
            pilots: BTreeMap::new(),
        },
    );
    Ok(id)
}
/// Cases: scen:59.32, scen:59.33, scen:59.34, scen:59.35, airlog:35.24, airlog:35.26
pub(super) fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    task: (&str, &Phase),
    action: &Value,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), Rejection> {
    let (force, phase) = task;
    if side(force) != pending.seat.side {
        return Err(illegal("air force belongs to another side"));
    }
    let choices = picks(content, state, force, phase).map_err(Rejection::Engine)?;
    let p = choices
        .into_iter()
        .find(|p| Some(p.id.as_str()) == action.as_str())
        .ok_or_else(|| illegal("choose a currently legal squadron/facility allocation"))?;
    match phase {
        Phase::Planes { aircraft, ready } => {
            let id = if let Some(id) = p.squadron {
                id
            } else {
                create(
                    content,
                    state,
                    force,
                    &p.facility,
                    quota(content, force, aircraft).map(|_| aircraft.clone()),
                )
                .map_err(Rejection::Engine)?
            };
            let original = state
                .air
                .forces
                .get_mut(force)
                .expect("force")
                .planes
                .get_mut(aircraft)
                .expect("planes");
            original.total -= p.count;
            if *ready {
                original.ready -= p.count
            }
            original.fuelled -= p.count;
            original.armed -= p.count;
            let dest = state
                .air
                .squadrons
                .get_mut(&id)
                .expect("SGSU")
                .planes
                .entry(aircraft.clone())
                .or_default();
            dest.total += p.count;
            if *ready {
                dest.ready += p.count
            }
            dest.fuelled += p.count;
            dest.armed += p.count;
        }
        Phase::Sgsu { aircraft } => {
            create(content, state, force, &p.facility, aircraft.clone())
                .map_err(Rejection::Engine)?;
        }
        Phase::Pilots { rating } => {
            *state
                .air
                .forces
                .get_mut(force)
                .expect("force")
                .pilots
                .get_mut(rating)
                .expect("pilots") -= p.count;
            *state
                .air
                .squadrons
                .get_mut(p.squadron.as_ref().expect("pilot target"))
                .expect("SGSU")
                .pilots
                .entry(*rating)
                .or_default() += p.count;
        }
    }
    next(content, state, force, strict, cx).map_err(Rejection::Engine)
}

#[cfg(test)]
#[path = "air_tests.rs"]
mod tests;
