//! Truthful close-assault relationships; only each owner's status is disclosed.
use crate::{CnaContent, State};
use cna_core::{engine::EngineError, ids::UnitId};
/// An Engaged CRT result links every involved unit to the involved opposing units.
/// Call only in adjudication. No opponent identities are added to any readable view.
/// Cases: land:8.63, land:15.81
pub fn engage(
    s: &mut State,
    attackers: &[UnitId],
    defenders: &[UnitId],
) -> Result<(), EngineError> {
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
        u.engaged = true;
    }
    Ok(())
}
/// Breaking off ends this unit's relationships. An opponent stays engaged while any
/// other involved friendly unit remains. Bare fixture flags identify no opponent.
/// Call only on authoritative execution, after the unit has paid its own breakoff CP.
/// Cases: land:8.64, land:8.66, land:8.67
pub fn break_off(s: &mut State, id: &UnitId) {
    if let Some(u) = s.land.units.get_mut(id)
        && u.engaged
    {
        u.engaged = false;
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
        }
    }
}

/// Engaged status expires for both sides at the authoritative Operations Stage boundary.
/// Cases: land:15.81
pub fn clear(s: &mut State) {
    s.land.engagements.clear();
    for unit in s.land.units.values_mut() {
        unit.engaged = false;
    }
}
/// Terminal losses remove relationships before later steps advertise movement or reaction.
/// A surviving HQ remains a cadre even without battalions. Counters without TOE ratings are
/// not declared dead merely because the strength lookup has no value.
/// This removes relationships only; equipment, locations and repair rights belong to their procedures.
/// Call only from automatic adjudication, never from answer validation.
/// Cases: land:8.67, land:15.81, land:19.62, land:19.67
pub fn reconcile(c: &CnaContent, s: &mut State) {
    let dead: Vec<_> = s
        .land
        .engagements
        .keys()
        .filter(|id| {
            let Some(u) = s.land.units.get(*id) else {
                return true;
            };
            if !matches!(
                u.location,
                crate::state::Location::Hex { .. } | crate::state::Location::OffMap { .. }
            ) {
                return true;
            }
            if super::formation::class(c, id).is_some_and(|cl| cl.unit_type == "headquarters") {
                return false;
            }
            crate::view::toe_points(c, u) == Some(0)
        })
        .cloned()
        .collect();
    for id in dead {
        break_off(s, &id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Location;
    use cna_content::units::Toe;
    /// Cases: land:8.67, land:19.62, land:19.67
    #[test]
    fn terminal_participant_is_removed_but_another_live_opponent_and_hq_cadre_persist() {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let a: UnitId = "cw.unassigned_inf.1st_rnf_mg_bn".into();
        let b: UnitId = "cw.2_nz_div.21st_nz_bn".into();
        let tank: UnitId = "it.libyan_tank_command.xxi_l_tank_bn".into();
        let hq: UnitId = "cw.22_guards_bde.22nd_guards_bde_hq".into();
        for id in [&a, &b, &hq, &tank] {
            let u = s.land.units.get_mut(id).unwrap();
            u.location = Location::Hex {
                hex: "C4020".into(),
            };
            u.detached = true;
            u.attached_to = None;
        }
        engage(
            &mut s,
            &[a.clone(), b.clone(), hq.clone()],
            std::slice::from_ref(&tank),
        )
        .unwrap();
        s.land.units.get_mut(&a).unwrap().toe = Some(Toe::Under { under: 0 });
        s.land.units.get_mut(&hq).unwrap().toe = Some(Toe::Under { under: 0 });
        reconcile(&c, &mut s);
        assert!(!s.land.units[&a].engaged);
        assert!(
            s.land.units[&b].engaged && s.land.units[&tank].engaged && s.land.units[&hq].engaged
        );
        assert_eq!(
            s.land.engagements[&tank],
            std::collections::BTreeSet::from([b.clone(), hq.clone()])
        );
        s.land.units.get_mut(&b).unwrap().location = Location::Eliminated;
        reconcile(&c, &mut s);
        assert!(s.land.units[&tank].engaged);
        assert_eq!(
            s.land.engagements[&tank],
            std::collections::BTreeSet::from([hq.clone()])
        );
        s.land.units.get_mut(&hq).unwrap().location = Location::Eliminated;
        reconcile(&c, &mut s);
        assert!(s.land.engagements.is_empty());
        assert!(!s.land.units[&tank].engaged);
        assert_eq!(s.land.units[&a].location.hex(), Some(&"C4020".into()));
    }
}
