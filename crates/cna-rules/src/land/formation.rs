//! Attached formations count once on the map; component state remains individual.
use super::capability::Allowance;
use crate::ownership::parent_for_unit;
use crate::{CnaContent, State};
use cna_content::units::{Toe, UnitClass};
use cna_core::ids::{HexId, UnitId};
use cna_protocol::Side;
use cna_tables::land::administration::{OrganizationLevel as Level, ShellOf};
use std::collections::{BTreeMap, BTreeSet};

pub fn class<'a>(content: &'a CnaContent, id: &UnitId) -> Option<&'a UnitClass> {
    content
        .units
        .units
        .get(id)?
        .class
        .as_ref()
        .and_then(|c| content.units.classes.get(c))
}
/// Components still with their parent, including the root. A remote child is not represented.
/// Cases: land:9.21, land:19.46
pub fn members(content: &CnaContent, state: &State, root: &UnitId) -> Vec<UnitId> {
    let Some(unit) = state.land.units.get(root) else {
        return Vec::new();
    };
    let mut ids = BTreeSet::from([root.clone()]);
    loop {
        let add: Vec<_> = state
            .land
            .units
            .values()
            .filter(|u| {
                u.side == unit.side
                    && u.location == unit.location
                    && !ids.contains(&u.id)
                    && parent_for_unit(content, state, &u.id).is_some_and(|p| ids.contains(p))
            })
            .map(|u| u.id.clone())
            .collect();
        if add.is_empty() {
            break;
        }
        ids.extend(add);
    }
    ids.into_iter().collect()
}
/// A window-local attachment index. It is rebuilt from current state, never persisted.
/// Cases: land:6.15, land:9.21, land:19.46
pub(super) struct FormationIndex {
    children: BTreeMap<UnitId, Vec<UnitId>>,
}
impl FormationIndex {
    pub(super) fn new(content: &CnaContent, state: &State) -> Self {
        let mut children: BTreeMap<UnitId, Vec<UnitId>> = BTreeMap::new();
        for unit in state.land.units.values() {
            if let Some(parent) = parent_for_unit(content, state, &unit.id)
                && state
                    .land
                    .units
                    .get(parent)
                    .is_some_and(|p| p.side == unit.side && p.location == unit.location)
            {
                children
                    .entry(parent.clone())
                    .or_default()
                    .push(unit.id.clone());
            }
        }
        Self { children }
    }
    pub(super) fn members(&self, root: &UnitId) -> Vec<UnitId> {
        let mut todo = vec![root.clone()];
        let mut seen = BTreeSet::new();
        while let Some(id) = todo.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(children) = self.children.get(&id) {
                todo.extend(children.iter().rev().cloned());
            }
        }
        seen.into_iter().collect()
    }
    pub(super) fn allowance(
        &self,
        content: &CnaContent,
        state: &State,
        root: &UnitId,
    ) -> Option<Allowance> {
        let mut todo = vec![root.clone()];
        let mut seen = BTreeSet::new();
        let mut lowest: Option<Allowance> = None;
        while let Some(id) = todo.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let next = individual_allowance(content, state, &id)?;
            if lowest.is_none_or(|old| next.cpa < old.cpa) {
                lowest = Some(next);
            }
            if let Some(children) = self.children.get(&id) {
                todo.extend(children.iter().rev().cloned());
            }
        }
        lowest
    }
}
/// Only represented counters count. Independently stacked units are separate counters.
/// Cases: land:9.12, land:9.13, land:9.21
pub fn roots(content: &CnaContent, state: &State, hex: &HexId, side: Side) -> Vec<UnitId> {
    state
        .units_of(side)
        .filter(|u| {
            u.location.hex() == Some(hex)
                && !parent_for_unit(content, state, &u.id).is_some_and(|p| {
                    state
                        .land
                        .units
                        .get(p)
                        .is_some_and(|p| p.location == u.location && p.side == side)
                })
        })
        .map(|u| u.id.clone())
        .collect()
}
pub fn combat_unit(content: &CnaContent, id: &UnitId) -> bool {
    class(content, id).is_some_and(|c| {
        matches!(
            c.unit_type.as_str(),
            "infantry" | "tank" | "recce" | "artillery" | "anti_tank" | "anti_air"
        )
    })
}
pub fn strength(content: &CnaContent, state: &State, id: &UnitId) -> i32 {
    state
        .land
        .units
        .get(id)
        .and_then(|u| crate::view::toe_points(content, u))
        .unwrap_or(0)
}
/// Individual CPA reflects weapon choice unless fixed; an immobile gun has no movement CPA.
/// Cases: land:6.11, land:6.15, land:8.17, land:8.91, land:8.93
pub fn individual_allowance(content: &CnaContent, state: &State, id: &UnitId) -> Option<Allowance> {
    let oa = content.units.units.get(id)?;
    let c = class(content, id)?;
    let unit = state.land.units.get(id)?;
    let mut cpa = oa.cpa.unwrap_or(c.cpa);
    if oa.immobile || c.emplaced {
        cpa = 0;
    } else if !c.cpa_fixed
        && let Some(Toe::Weapons(w)) = &unit.toe
    {
        for point in w.iter().filter(|w| w.n > 0) {
            cpa = cpa.min(content.units.weapons.get(&point.weapon)?.cpa);
        }
    }
    if [
        unit.transport_trucks.light,
        unit.transport_trucks.medium,
        unit.transport_trucks.heavy,
        unit.trucks.light,
        unit.trucks.medium,
        unit.trucks.heavy,
    ]
    .iter()
    .any(|n| *n < 0)
    {
        return None;
    }
    // Truck motorization is resolved from an explicit transport assignment (not cargo ownership).
    if unit.transport_trucks.total() > 0 {
        if unit.transport_trucks.light > unit.trucks.light
            || unit.transport_trucks.medium > unit.trucks.medium
            || unit.transport_trucks.heavy > unit.trucks.heavy
        {
            return None;
        }
        let mut capacity: i32 = 0;
        let mut truck_cpa = i32::MAX;
        for (n, kind) in [
            (
                unit.transport_trucks.light,
                cna_tables::airlog::trucks::TruckType::Light,
            ),
            (
                unit.transport_trucks.medium,
                cna_tables::airlog::trucks::TruckType::Medium,
            ),
            (
                unit.transport_trucks.heavy,
                cna_tables::airlog::trucks::TruckType::Heavy,
            ),
        ] {
            if n <= 0 {
                continue;
            }
            let t = content.tables.airlog.truck_characteristics.truck(kind);
            let (cap, speed) = if c.unit_type == "infantry" {
                (t.capacity_inf_toe_halves, t.cpa_inf)
            } else if c.unit_type == "anti_air" {
                (t.capacity_aa_toe * 2, t.cpa_guns?)
            } else {
                (t.capacity_arty_toe? * 2, t.cpa_guns?)
            };
            capacity = capacity.checked_add(n.checked_mul(cap)?)?;
            truck_cpa = truck_cpa.min(speed);
        }
        if capacity >= strength(content, state, id).checked_mul(2)?
            && strength(content, state, id) > 0
        {
            cpa = truck_cpa;
        }
    }
    Some(Allowance {
        cpa,
        motorized: cpa > 10,
    })
}
/// A parent moves at the lowest CPA of its represented components.
/// Cases: land:6.15, land:6.17, land:8.91, land:8.92
pub fn allowance(content: &CnaContent, state: &State, root: &UnitId) -> Option<Allowance> {
    members(content, state, root)
        .into_iter()
        .map(|id| individual_allowance(content, state, &id))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .min_by_key(|a| a.cpa)
}
fn live_children(content: &CnaContent, state: &State, id: &UnitId) -> Vec<UnitId> {
    members(content, state, id)
        .into_iter()
        .filter(|u| u != id && parent_for_unit(content, state, u) == Some(id))
        .collect()
}
/// Evaluate TOE shells and the proportion of non-shell assigned equivalents with the parent.
/// Graziani uses the historical OA roster as its assigned maximum, including future arrivals.
/// Cases: land:9.26, land:9.27, land:9.28
pub fn is_shell(content: &CnaContent, state: &State, id: &UnitId) -> bool {
    fn inner(
        content: &CnaContent,
        state: &State,
        id: &UnitId,
        seen: &mut BTreeSet<UnitId>,
    ) -> bool {
        if !seen.insert(id.clone()) {
            return true;
        }
        let Some(oa) = content.units.units.get(id) else {
            return true;
        };
        let sp = oa.stacking_points.unwrap_or(0);
        if sp <= 1 {
            let max = class(content, id).and_then(|c| c.max_toe).unwrap_or(0);
            let current = strength(content, state, id);
            let factor = if class(content, id).is_some_and(|c| c.unit_type == "artillery") {
                4
            } else {
                2
            };
            return max > 0 && i64::from(current) * factor < i64::from(max);
        }
        let relevant = |u: &cna_content::units::OaUnit| {
            if sp == 5 {
                u.stacking_points.unwrap_or(0) >= 2
            } else {
                u.stacking_points == Some(1)
            }
        };
        let assigned = content.units.children(id).filter(|u| relevant(u)).count();
        let attached: Vec<_> = live_children(content, state, id)
            .into_iter()
            .filter(|u| content.units.units.get(u).is_some_and(relevant))
            .collect();
        let full = attached
            .iter()
            .filter(|u| !inner(content, state, u, &mut seen.clone()))
            .count();
        if assigned == 0 {
            return attached.is_empty();
        }
        if sp == 5 {
            full * 2 <= assigned
        } else if assigned == 2 && attached.len() >= 2 && full >= 1 {
            false
        } else {
            full * 3 < assigned * 2
        }
    }
    inner(content, state, id, &mut BTreeSet::new())
}
/// Stacking values in halves. Attached first-line trucks never add stacking points.
/// Cases: land:9.11, land:9.12, land:9.13, land:9.24, land:9.26, land:9.28, land:9.29, land:9.4
pub fn stacking_halves(content: &CnaContent, state: &State, id: &UnitId) -> i32 {
    let Some(oa) = content.units.units.get(id) else {
        return 0;
    };
    let sp = oa.stacking_points.unwrap_or(0);
    let has_combat = members(content, state, id)
        .iter()
        .any(|u| u != id && combat_unit(content, u) && strength(content, state, u) > 0);
    if class(content, id).is_some_and(|c| c.unit_type == "headquarters") && !has_combat {
        return 0;
    }
    let t = &content.tables.land.stacking_values;
    if is_shell(content, state, id) {
        return t.shell_halves(if sp == 5 {
            ShellOf::Division
        } else if sp >= 2 {
            ShellOf::AnyBrigade
        } else if sp == 1 {
            ShellOf::Battalion
        } else {
            ShellOf::Hq
        });
    }
    t.full_unit_halves(match sp {
        5 => Level::Division,
        3 => Level::SuperBrigade,
        2 => Level::StandardBrigadeOrBattleGroup,
        1 => Level::Battalion,
        _ => Level::Company,
    })
    .unwrap_or(0)
}
/// Use the smaller organizational reading for a pinner and the larger for its target.
/// This preserves a shell division's protection against battalion-sized pinning.
/// Cases: land:8.54, land:9.28
/// Interpretations: interp:land-0003
pub fn can_pin(content: &CnaContent, state: &State, pinner: &UnitId, pinned: &UnitId) -> bool {
    if !content.units.units.contains_key(pinner) || !content.units.units.contains_key(pinned) {
        return false;
    }
    let printed = |id: &UnitId| {
        content
            .units
            .units
            .get(id)
            .and_then(|u| u.stacking_points)
            .unwrap_or(0)
            * 2
    };
    let small = printed(pinner).min(stacking_halves(content, state, pinner));
    let large = printed(pinned).max(stacking_halves(content, state, pinned));
    !(small <= 2 && large >= 10 || small == 0 && large >= 4)
}
/// Defensive rating times TOE, counting every represented unit once; anti-tank points are up front.
/// Cases: land:10.15, land:11.31
pub fn raw_defense(content: &CnaContent, state: &State, id: &UnitId) -> i64 {
    members(content, state, id)
        .iter()
        .filter(|id| state.land.units[*id].cohesion_quarters > -104)
        .map(|id| {
            let u = &state.land.units[id];
            if let Some(Toe::Weapons(w)) = &u.toe {
                w.iter()
                    .map(|p| {
                        i64::from(p.n)
                            * i64::from(
                                content
                                    .units
                                    .weapons
                                    .get(&p.weapon)
                                    .and_then(|w| w.ca_def)
                                    .unwrap_or(0),
                            )
                    })
                    .sum()
            } else {
                i64::from(strength(content, state, id))
                    * i64::from(class(content, id).and_then(|c| c.ca_def).unwrap_or(0))
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (CnaContent, State) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let s = State::new(&c).unwrap();
        (c, s)
    }
    /// Cases: land:6.15, land:9.12, land:9.13, land:9.21, land:9.26, land:9.27, land:9.28
    #[test]
    fn real_division_counts_once_and_detachment_reduces_it_to_a_shell() {
        let (c, mut s) = setup();
        let hq: UnitId = "it.1_libyan_div.1st_libyan_infantry_hq".into();
        let hex = s.land.units[&hq].location.hex().unwrap().clone();
        assert_eq!(allowance(&c, &s, &hq).unwrap().cpa, 10);
        assert!(!is_shell(&c, &s, &hq));
        assert_eq!(stacking_halves(&c, &s, &hq), 10);
        assert!(roots(&c, &s, &hex, Side::Axis).contains(&hq));
        assert!(
            !roots(&c, &s, &hex, Side::Axis).contains(&"it.1_libyan_div.viii_libyan_bn".into())
        );
        s.land
            .units
            .get_mut(&"it.1_libyan_div.1st_libyan_regt_hq".into())
            .unwrap()
            .detached = true;
        assert!(is_shell(&c, &s, &hq));
        assert_eq!(stacking_halves(&c, &s, &hq), 6);
    }
    /// Cases: land:9.26, land:9.28, land:10.15
    #[test]
    fn shell_threshold_is_strict_and_defense_uses_actual_toe() {
        let (c, mut s) = setup();
        let id: UnitId = "cw.unassigned_inf.1st_rnf_mg_bn".into();
        let max = class(&c, &id).unwrap().max_toe.unwrap();
        let u = s.land.units.get_mut(&id).unwrap();
        u.toe = Some(Toe::Under {
            under: (max + 1) / 2,
        });
        assert!(!is_shell(&c, &s, &id));
        s.land.units.get_mut(&id).unwrap().toe = Some(Toe::Under { under: 0 });
        assert!(is_shell(&c, &s, &id));
        assert_eq!(stacking_halves(&c, &s, &id), 0);
        assert_eq!(raw_defense(&c, &s, &id), 0);
    }
    /// Cases: land:8.54, land:9.28
    /// Interpretations: interp:land-0003
    #[test]
    fn battalion_cannot_pin_a_shell_division_and_company_cannot_pin_a_brigade() {
        let c = crate::CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let division: UnitId = "it.1_libyan_div.1st_libyan_infantry_hq".into();
        let brigade: UnitId = "it.1_libyan_div.1st_libyan_regt_hq".into();
        let battalion: UnitId = "it.libyan_tank_command.xxi_l_tank_bn".into();
        for child in members(&c, &s, &division) {
            if child != division {
                s.land.units.get_mut(&child).unwrap().location = crate::state::Location::Eliminated;
            }
        }
        assert!(is_shell(&c, &s, &division));
        assert!(!can_pin(&c, &s, &battalion, &division));
        s.land.units.get_mut(&battalion).unwrap().toe = Some(Toe::Weapons(vec![]));
        assert!(!can_pin(&c, &s, &battalion, &brigade));
    }
}
