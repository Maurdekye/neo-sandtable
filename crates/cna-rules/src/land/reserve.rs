//! Reserve time changes effective CPA and the immediately following cycle's movement entitlement.
use super::{capability::Allowance, cycles, formation};
use crate::{
    CnaContent, State,
    state::{LandUnit, Location, Pending},
    steps::{illegal, open},
    view,
};
use cna_core::{
    decision::{ActionSchema, ActionSpace, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{SeatId, UnitId},
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const DESIGNATE: &str = "cna.reserve.designate";
pub const RELEASE: &str = "cna.reserve.release";
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    None,
    First,
    Second,
    ReleasedFirst,
    ReleasedSecond,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReserveState {
    pub status: Status,
    pub released_for_cycle: Option<u16>,
    pub offensive_assault_used: bool,
    pub extra_dp_applied: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveCombat {
    Barrage,
    AntiArmor,
    CloseAssault,
    Probe,
}

/// Reserved units retain the proximity exception; a release supplies it for one ensuing segment.
/// Cases: land:18.22, land:18.25
pub fn bypass_proximity(unit: &LandUnit, cycle: u16) -> bool {
    matches!(unit.reserve.status, Status::First | Status::Second)
        || unit.reserve.released_for_cycle == Some(cycle)
}
/// Reserve II cannot move, while released II uses half of the resolved individual CPA, rounded down.
/// Cases: land:18.22, land:18.24
pub fn adjust_allowance(unit: &LandUnit, mut allowance: Allowance) -> Allowance {
    if unit.reserve.status == Status::ReleasedSecond {
        allowance.cpa /= 2;
    }
    allowance
}
/// A released reserve may not voluntarily exceed the effective CPA. Reserve I still pays normal CP.
/// Cases: land:18.22, land:18.23, land:18.24
pub fn validate_cp(
    unit: &LandUnit,
    allowance: Allowance,
    quarters: i32,
    voluntary: bool,
) -> Result<(), Rejection> {
    if voluntary
        && matches!(
            unit.reserve.status,
            Status::ReleasedFirst | Status::ReleasedSecond
        )
        && i64::from(unit.cp_spent_quarters) + i64::from(quarters) > i64::from(allowance.cpa) * 4
    {
        return Err(illegal(
            "released reserve cannot voluntarily exceed its effective CPA",
        ));
    }
    Ok(())
}
/// Limits apply to every represented member, including a reserved component of a larger counter.
/// Cases: land:18.22
pub fn validate_path(unit: &LandUnit, path_len: usize) -> Result<(), Rejection> {
    match unit.reserve.status {
        Status::Second => Err(illegal("Reserve II units cannot move")),
        Status::First if path_len > 1 => {
            Err(illegal("Reserve I units may move only one hex per segment"))
        }
        _ => Ok(()),
    }
}

fn ids(content: &CnaContent, state: &State, designate: bool) -> Vec<UnitId> {
    let Some(side) = state.cursor.phasing(state.turn.player_a) else {
        return vec![];
    };
    state
        .units_of(side)
        .filter(|u| {
            matches!(u.location, Location::Hex { .. } | Location::OffMap { .. })
                && if designate {
                    true
                } else {
                    matches!((state.cursor.cycle, u.reserve.status), (1, Status::First))
                        || state.cursor.cycle > 1 && u.reserve.status == Status::Second
                }
        })
        .map(|u| u.id.clone())
        .filter(|id| content.units.units.contains_key(id))
        .collect()
}
fn open_selection(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
    designate: bool,
) -> bool {
    let eligible = ids(content, state, designate);
    if eligible.is_empty() {
        return false;
    }
    let side = state.cursor.phasing(state.turn.player_a).unwrap();
    let max = eligible.len() as u32;
    let space = ActionSpace::new(ActionSchema::List {
        item: Box::new(ActionSchema::Unit { among: eligible }),
        min: 0,
        max,
    })
    .with_pass(if designate {
        "Designate no reserves."
    } else {
        "Keep these units in reserve."
    });
    open(
        state,
        cx,
        SeatId::new(side, Role::RearArea),
        if designate { DESIGNATE } else { RELEASE },
        if designate {
            "Select units for Reserve I. Their represented components share the status."
        } else {
            "Release selected reserves; unselected Reserve I units become Reserve II."
        }
        .into(),
        if designate {
            &["land:18.11", "land:18.12", "land:18.15", "land:18.26"]
        } else {
            &[
                "land:18.13",
                "land:18.14",
                "land:18.23",
                "land:18.24",
                "land:18.25",
                "land:18.26",
            ]
        },
        Trigger::Scheduled,
        Secrecy::Open,
        space,
    );
    true
}
/// Only the current phasing side designates reserves, before its first movement segment.
/// Cases: land:18.11, land:18.12, land:18.15, land:18.21
pub fn enter_designation(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    open_selection(content, state, cx, true);
    Ok(())
}
/// The first release converts remaining I to II; later releases can activate II.
/// Cases: land:18.13, land:18.14, land:18.23, land:18.24
pub fn enter_release(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    if !open_selection(content, state, cx, false) {
        cycles::enter(content, state, cx)?;
    }
    Ok(())
}
/// Designation/release costs no CP. Invalid lists do not alter any marker.
/// Cases: land:18.11, land:18.12, land:18.13, land:18.14, land:18.15, land:18.23, land:18.24, land:18.25, land:18.26
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    let designate = pending.kind == DESIGNATE;
    let anchor = if designate {
        "opstage.reserve_designation"
    } else {
        "opstage.movement_and_combat.reserve_release"
    };
    if state.cursor.anchor() != anchor {
        return Err(illegal("reserve action is not available at this step"));
    }
    let legal: BTreeSet<_> = ids(content, state, designate).into_iter().collect();
    let chosen: Vec<UnitId> = if action.is_null() {
        vec![]
    } else {
        serde_json::from_value(action.clone())
            .map_err(|_| illegal("choose a list of own reserve units"))?
    };
    let mut selected = BTreeSet::new();
    for id in chosen {
        if !legal.contains(&id) || !selected.insert(id.clone()) {
            return Err(illegal("invalid or duplicate reserve unit"));
        }
    }
    // Expand a chosen represented formation without charging detachment.
    let mut expanded = BTreeSet::new();
    for id in &selected {
        expanded.extend(formation::members(content, state, id));
    }
    let mut updates = Vec::new();
    for id in legal {
        let unit = &state.land.units[&id];
        let next = if designate && expanded.contains(&id) {
            Some(Status::First)
        } else if !designate && expanded.contains(&id) {
            Some(if unit.reserve.status == Status::First {
                Status::ReleasedFirst
            } else {
                Status::ReleasedSecond
            })
        } else if !designate && unit.reserve.status == Status::First {
            Some(Status::Second)
        } else {
            None
        };
        if let Some(status) = next {
            updates.push((id, status));
        }
    }
    let cycle = state
        .cursor
        .cycle
        .checked_add(1)
        .ok_or_else(|| illegal("movement cycle counter exhausted"))?;
    for (id, status) in updates {
        let u = state.land.units.get_mut(&id).unwrap();
        u.reserve.status = status;
        if matches!(status, Status::ReleasedFirst | Status::ReleasedSecond) {
            u.reserve.released_for_cycle = Some(cycle);
        }
        cx.emit(EngineEvent::new(
            Audience::Side(u.side),
            GameEvent::UnitUpdated {
                unit: view::unit_view(content, u),
            },
        ));
    }
    if !designate {
        cycles::enter(content, state, cx)?;
    }
    Ok(if designate {
        "Reserve designation complete."
    } else {
        "Reserve release complete."
    }
    .into())
}

/// Offensive reserve limits are assessed before combat mutation, in the caller's transaction.
/// The additional Reserve II DP is once per OpStage; defensive fighting does not call this hook.
/// Cases: land:18.23, land:18.24
/// Interpretations: interp:land-0025
pub fn record_offensive_action(
    state: &mut State,
    id: &UnitId,
    action: ReserveCombat,
) -> Result<(), Rejection> {
    let unit = state
        .land
        .units
        .get_mut(id)
        .ok_or_else(|| illegal("unknown reserve combat unit"))?;
    if matches!(unit.reserve.status, Status::First | Status::Second) {
        return Err(illegal("release a reserve before offensive combat"));
    }
    let limited = matches!(
        unit.reserve.status,
        Status::ReleasedFirst | Status::ReleasedSecond
    );
    let assault = matches!(action, ReserveCombat::CloseAssault | ReserveCombat::Probe);
    if limited && assault && unit.reserve.offensive_assault_used {
        return Err(illegal(
            "released reserve has already made its offensive assault this stage",
        ));
    }
    let extra = unit.reserve.status == Status::ReleasedSecond && !unit.reserve.extra_dp_applied;
    let next = if extra {
        unit.cohesion_quarters
            .checked_sub(4)
            .ok_or_else(|| illegal("cohesion overflow"))?
    } else {
        unit.cohesion_quarters
    };
    if limited && assault {
        unit.reserve.offensive_assault_used = true;
    }
    if extra {
        unit.cohesion_quarters = next;
        unit.reserve.extra_dp_applied = true;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (CnaContent, State, UnitId) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let s = State::new(&c).unwrap();
        (c, s, "it.libyan_tank_command.xxi_l_tank_bn".into())
    }
    /// Cases: land:18.22, land:18.23, land:18.24, land:18.26
    #[test]
    fn released_allowances_round_down_preserve_motorization_and_forbid_voluntary_excess() {
        let (c, mut s, id) = fixture();
        let u = s.land.units.get_mut(&id).unwrap();
        u.reserve.status = Status::ReleasedSecond;
        let base = Allowance {
            cpa: 15,
            motorized: true,
        };
        let half = adjust_allowance(u, base);
        assert_eq!(
            half,
            Allowance {
                cpa: 7,
                motorized: true
            }
        );
        super::super::capability::charge(u, half, 28, true).unwrap();
        let before = u.clone();
        assert!(super::super::capability::charge(u, half, 1, true).is_err());
        assert_eq!(*u, before);
        // Involuntary CP remain accountable and can exceed CPA.
        super::super::capability::charge(u, half, 4, false).unwrap();
        assert_eq!(u.cohesion_quarters, -4);
        u.reserve.status = Status::ReleasedFirst;
        assert_eq!(adjust_allowance(u, base), base);
        assert_eq!(
            formation::individual_allowance(&c, &s, &id).unwrap().cpa,
            25
        );
    }
    /// Cases: land:18.23, land:18.24
    /// Interpretations: interp:land-0025
    #[test]
    fn extra_dp_is_once_per_stage_and_second_offensive_assault_is_atomic() {
        let (_, mut s, id) = fixture();
        for waiting in [Status::First, Status::Second] {
            s.land.units.get_mut(&id).unwrap().reserve.status = waiting;
            let before = s.clone();
            for action in [
                ReserveCombat::Barrage,
                ReserveCombat::AntiArmor,
                ReserveCombat::CloseAssault,
                ReserveCombat::Probe,
            ] {
                assert!(record_offensive_action(&mut s, &id, action).is_err());
                assert_eq!(
                    serde_json::to_value(&s).unwrap(),
                    serde_json::to_value(&before).unwrap()
                );
            }
        }
        s.land.units.get_mut(&id).unwrap().reserve.status = Status::ReleasedSecond;
        record_offensive_action(&mut s, &id, ReserveCombat::Barrage).unwrap();
        record_offensive_action(&mut s, &id, ReserveCombat::AntiArmor).unwrap();
        record_offensive_action(&mut s, &id, ReserveCombat::CloseAssault).unwrap();
        assert_eq!(s.land.units[&id].cohesion_quarters, -4);
        let checkpoint = serde_json::to_value(&s).unwrap();
        let mut recovered: State = serde_json::from_value(checkpoint).unwrap();
        let before = recovered.clone();
        assert!(record_offensive_action(&mut recovered, &id, ReserveCombat::Probe).is_err());
        assert_eq!(
            serde_json::to_value(&recovered).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
        super::super::capability::finish_opstage(&mut recovered);
        assert_eq!(recovered.land.units[&id].reserve, ReserveState::default());
        assert_eq!(recovered.land.units[&id].cohesion_quarters, 0);
    }
    /// Cases: land:18.22, land:18.25
    #[test]
    fn released_proximity_exception_is_only_the_immediately_following_cycle() {
        let (_, mut s, id) = fixture();
        let u = s.land.units.get_mut(&id).unwrap();
        u.reserve.status = Status::ReleasedSecond;
        u.reserve.released_for_cycle = Some(3);
        assert!(bypass_proximity(u, 3));
        assert!(!bypass_proximity(u, 4));
        u.reserve.status = Status::First;
        assert!(bypass_proximity(u, 4));
        assert!(validate_path(u, 1).is_ok());
        assert!(validate_path(u, 2).is_err());
        u.reserve.status = Status::Second;
        assert!(validate_path(u, 1).is_err());
    }
}
