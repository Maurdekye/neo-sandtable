//! Truthful close-assault relationships; only each owner's status is disclosed.
use crate::{CnaContent, State};
use cna_core::{engine::EngineError, event::EngineEvent, ids::UnitId, visibility::Audience};
use cna_protocol::GameEvent;
use std::collections::BTreeSet;
fn updates(c: &CnaContent, s: &State, changed: &BTreeSet<UnitId>) -> Vec<EngineEvent> {
    changed
        .iter()
        .map(|id| {
            EngineEvent::new(
                Audience::Side(s.land.units[id].side),
                GameEvent::UnitUpdated {
                    unit: crate::view::unit_view(c, &s.land.units[id]),
                },
            )
        })
        .collect()
}
/// An Engaged CRT result links every involved unit to the involved opposing units.
/// Call only in adjudication. No opponent identities are added to any readable view.
/// Cases: land:8.63, land:15.81
pub fn engage(
    c: &CnaContent,
    s: &mut State,
    attackers: &[UnitId],
    defenders: &[UnitId],
) -> Result<Vec<EngineEvent>, EngineError> {
    if attackers.is_empty()
        || defenders.is_empty()
        || attackers
            .iter()
            .chain(defenders)
            .any(|id| !s.land.units.contains_key(id))
    {
        return Err(EngineError::Invariant {
            detail: "engagement requires identified opposing units".into(),
        });
    }
    let side = s.land.units[&attackers[0]].side;
    if attackers.iter().any(|id| s.land.units[id].side != side)
        || defenders
            .iter()
            .any(|id| s.land.units[id].side != side.opponent())
    {
        return Err(EngineError::Invariant {
            detail: "engagement participants must be opposing sides".into(),
        });
    }
    let mut changed = BTreeSet::new();
    for a in attackers {
        for d in defenders {
            s.land
                .engagements
                .entry(a.clone())
                .or_default()
                .insert(d.clone());
            s.land
                .engagements
                .entry(d.clone())
                .or_default()
                .insert(a.clone());
        }
    }
    for id in attackers.iter().chain(defenders) {
        let u = s.land.units.get_mut(id).unwrap();
        if !u.engaged {
            u.engaged = true;
            changed.insert(id.clone());
        }
    }
    Ok(updates(c, s, &changed))
}
/// Breaking off ends this unit's relationships. An opponent stays engaged while any
/// other involved friendly unit remains. Bare fixture flags identify no opponent.
/// Call only on authoritative execution, after the unit has paid its own breakoff CP.
/// Cases: land:8.64, land:8.66, land:8.67
pub fn break_off(c: &CnaContent, s: &mut State, id: &UnitId) -> Vec<EngineEvent> {
    let mut changed = BTreeSet::new();
    if let Some(u) = s.land.units.get_mut(id)
        && u.engaged
    {
        u.engaged = false;
        changed.insert(id.clone());
    }
    for other in s.land.engagements.remove(id).unwrap_or_default() {
        if let Some(links) = s.land.engagements.get_mut(&other) {
            links.remove(id);
        }
        let engaged = s
            .land
            .engagements
            .get(&other)
            .is_some_and(|links| !links.is_empty());
        if !engaged {
            s.land.engagements.remove(&other);
        }
        if let Some(u) = s.land.units.get_mut(&other)
            && u.engaged != engaged
        {
            u.engaged = engaged;
            changed.insert(other);
        }
    }
    updates(c, s, &changed)
}
