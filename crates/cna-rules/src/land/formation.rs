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
    Some(super::reserve::adjust_allowance(
        unit,
        Allowance {
            cpa,
            motorized: cpa > 10,
        },
    ))
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
/// Error context is local to a query or a submitted truck allocation, not persisted.
#[derive(Clone, Copy)]
pub(super) enum QueryAssignment {
    Trusted,
    Submitted,
}
pub(super) fn query_invariant(detail: &str) -> cna_core::engine::Rejection {
    cna_core::engine::Rejection::Engine(cna_core::engine::EngineError::Invariant {
        detail: detail.into(),
    })
}
fn query_unsupported(case: &str, detail: &str) -> cna_core::engine::Rejection {
    cna_core::engine::Rejection::Engine(cna_core::engine::EngineError::Unsupported {
        case: case.into(),
        detail: detail.into(),
    })
}
fn assignment_error(context: QueryAssignment, detail: &str) -> cna_core::engine::Rejection {
    match context {
        QueryAssignment::Trusted => query_invariant(detail),
        QueryAssignment::Submitted => crate::steps::illegal(detail),
    }
}
/// Typed boundary for the affected movement queries; other Option callers stay unchanged.
/// Cases: land:4.46, land:4.48, land:6.11, land:6.15, land:8.17, land:8.91, land:8.93
pub(super) fn query_class<'a>(
    content: &'a CnaContent,
    id: &UnitId,
) -> Result<&'a UnitClass, cna_core::engine::Rejection> {
    class(content, id).ok_or_else(|| {
        query_unsupported(
            "land:4.46",
            "movement class is not identified in source data",
        )
    })
}
/// Preserve canonical TOE interpretation, including the adopted not-applicable HQ body.
/// Cases: land:4.45, land:4.46, land:4.48, land:8.91, land:8.93
/// Interpretations: interp:units-0007
pub(super) fn query_strength(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<i32, cna_core::engine::Rejection> {
    let unit =
        state.land.units.get(id).ok_or_else(|| {
            query_invariant("movement strength references a missing trusted unit")
        })?;
    // The supply helper maps a missing U/O maximum to Invalid. Establish its source cause first.
    if matches!(&unit.toe, Some(Toe::Under { .. } | Toe::Over { .. }))
        && query_class(content, id)?.max_toe.is_none()
    {
        return Err(query_unsupported(
            "land:4.46",
            "U/O movement strength lacks a source maximum",
        ));
    }
    crate::logistics::toe_strength(content, unit)
        .map(|n| n.get())
        .map_err(|e| match e {
            crate::logistics::SupplyError::Unsupported { case } => {
                query_unsupported(case, "movement strength lacks cited source data")
            }
            // toe_strength emits Invalid only for malformed trusted quantities after the check above.
            _ => query_invariant("invalid trusted movement strength or strength arithmetic"),
        })
}
/// None is the chart's known not-allowed cell, never an inferred missing numeric value.
/// Cases: airlog:54.2, land:8.91, land:8.93
pub(super) fn query_transport_cell(
    content: &CnaContent,
    class: &UnitClass,
    kind: cna_tables::airlog::trucks::TruckType,
) -> Result<Option<(i32, i32)>, cna_core::engine::Rejection> {
    let t = content.tables.airlog.truck_characteristics.truck(kind);
    let cell = if class.unit_type == "infantry" {
        Some((t.capacity_inf_toe_halves, t.cpa_inf))
    } else if class.unit_type == "anti_air" {
        match t.cpa_guns {
            Some(speed) => Some((
                t.capacity_aa_toe
                    .checked_mul(2)
                    .ok_or_else(|| query_invariant("truck capacity arithmetic overflow"))?,
                speed,
            )),
            None => None,
        }
    } else {
        match (t.capacity_arty_toe, t.cpa_guns) {
            (Some(cap), Some(speed)) => Some((
                cap.checked_mul(2)
                    .ok_or_else(|| query_invariant("truck capacity arithmetic overflow"))?,
                speed,
            )),
            _ => None,
        }
    };
    if cell.is_some_and(|(cap, speed)| cap < 0 || speed < 0) {
        return Err(query_invariant("negative validated truck characteristic"));
    }
    Ok(cell)
}
/// Same successful arithmetic as individual_allowance, with each absent cause retained.
/// Cases: land:4.46, land:4.48, land:6.11, land:6.15, land:8.17, land:8.91, land:8.93, airlog:54.2
pub(super) fn query_individual_allowance(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    context: QueryAssignment,
) -> Result<Allowance, cna_core::engine::Rejection> {
    let unit =
        state.land.units.get(id).ok_or_else(|| {
            query_invariant("movement allowance references a missing trusted unit")
        })?;
    let oa = content
        .units
        .units
        .get(id)
        .ok_or_else(|| query_unsupported("land:4.46", "movement OA row is missing"))?;
    let c = query_class(content, id)?;
    let mut cpa = oa.cpa.unwrap_or(c.cpa);
    if cpa < 0 {
        return Err(query_invariant("negative validated movement rating"));
    }
    if oa.immobile || c.emplaced {
        cpa = 0;
    } else if !c.cpa_fixed
        && let Some(Toe::Weapons(w)) = &unit.toe
    {
        for point in w {
            if point.n < 0 {
                return Err(query_invariant("negative trusted weapon quantity"));
            }
            if point.n == 0 {
                continue;
            }
            let weapon =
                content.units.weapons.get(&point.weapon).ok_or_else(|| {
                    query_unsupported("land:4.48", "movement weapon row is missing")
                })?;
            if weapon.cpa < 0 {
                return Err(query_invariant("negative validated weapon CPA"));
            }
            cpa = cpa.min(weapon.cpa);
        }
    }
    let holdings = [unit.trucks.light, unit.trucks.medium, unit.trucks.heavy];
    let transport = [
        unit.transport_trucks.light,
        unit.transport_trucks.medium,
        unit.transport_trucks.heavy,
    ];
    if holdings.iter().any(|n| *n < 0) {
        return Err(query_invariant("negative trusted truck holding"));
    }
    if (0..3).any(|k| transport[k] < 0 || transport[k] > holdings[k]) {
        return Err(assignment_error(
            context,
            "transport assignment exceeds its owned trucks",
        ));
    }
    if transport.iter().any(|n| *n > 0) {
        let mut capacity = 0i32;
        let mut truck_cpa = i32::MAX;
        for (n, kind) in [
            (transport[0], cna_tables::airlog::trucks::TruckType::Light),
            (transport[1], cna_tables::airlog::trucks::TruckType::Medium),
            (transport[2], cna_tables::airlog::trucks::TruckType::Heavy),
        ] {
            if n == 0 {
                continue;
            }
            let (cap, speed) = query_transport_cell(content, c, kind)?.ok_or_else(|| {
                assignment_error(
                    context,
                    "truck chart does not allow this transport assignment",
                )
            })?;
            capacity = n
                .checked_mul(cap)
                .and_then(|v| capacity.checked_add(v))
                .ok_or_else(|| {
                    assignment_error(context, "transport capacity arithmetic overflow")
                })?;
            truck_cpa = truck_cpa.min(speed);
        }
        let strength = query_strength(content, state, id)?;
        let need = strength
            .checked_mul(2)
            .ok_or_else(|| query_invariant("trusted transport strength arithmetic overflow"))?;
        if capacity >= need && strength > 0 {
            cpa = truck_cpa;
        }
    }
    Ok(super::reserve::adjust_allowance(
        unit,
        Allowance {
            cpa,
            motorized: cpa > 10,
        },
    ))
}
/// Require every represented member; a partial minimum is never a movement rating.
/// Cases: land:6.15, land:6.17, land:8.91, land:8.92
pub(super) fn query_members_allowance(
    content: &CnaContent,
    state: &State,
    ids: &[UnitId],
) -> Result<Allowance, cna_core::engine::Rejection> {
    let mut lowest: Option<Allowance> = None;
    for id in ids {
        let next = query_individual_allowance(content, state, id, QueryAssignment::Trusted)?;
        if lowest.is_none_or(|old| next.cpa < old.cpa) {
            lowest = Some(next);
        }
    }
    lowest.ok_or_else(|| query_invariant("trusted moving formation has no represented members"))
}
/// Cases: land:6.15, land:6.17, land:8.91, land:8.92
pub(super) fn query_allowance(
    content: &CnaContent,
    state: &State,
    root: &UnitId,
) -> Result<Allowance, cna_core::engine::Rejection> {
    if !state.land.units.contains_key(root) {
        return Err(query_invariant(
            "movement formation references a missing trusted root",
        ));
    }
    query_members_allowance(content, state, &members(content, state, root))
}
fn live_children(content: &CnaContent, state: &State, id: &UnitId) -> Vec<UnitId> {
    members(content, state, id)
        .into_iter()
        .filter(|u| u != id && parent_for_unit(content, state, u) == Some(id))
        .collect()
}
/// Evaluate TOE shells and the proportion of non-shell assigned equivalents with the parent.
/// Capacity gap U-029 (land:9.26, land:19.3): until parent mappings are populated,
/// the printed OA roster is the fixed maximum approximation, including future arrivals.
/// Runtime assignment overrides must never shrink this maximum.
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
    /// Cases: land:19.13, land:19.14, land:9.27, land:9.28
    #[test]
    fn assignment_changes_cannot_shrink_a_shells_fixed_capacity() {
        use crate::state::Assignment;
        let (c, mut s) = setup();
        let parent: UnitId = "it.1_libyan_div.1st_libyan_infantry_hq".into();
        let child: UnitId = "it.1_libyan_div.1st_libyan_regt_hq".into();
        let printed_capacity = c.units.children(&parent).count();
        s.land.units.get_mut(&child).unwrap().detached = true;
        assert!(is_shell(&c, &s, &parent));
        assert_eq!(
            crate::ownership::assigned_parent_for_unit(&c, &s, &child),
            Some(&parent)
        );
        s.land.units.get_mut(&child).unwrap().assignment = Assignment::Independent;
        assert!(is_shell(&c, &s, &parent));
        assert_eq!(
            c.units.children(&parent).count(),
            printed_capacity,
            "runtime never rewrites source capacity"
        );
        let a = s.clone();
        let mut b = s;
        b.land.units.get_mut(&child).unwrap().assignment = Assignment::Printed;
        crate::testkit::assert_indistinguishable(
            &crate::Cna::dev(),
            &c,
            &a,
            &b,
            Side::Commonwealth,
        );
        let owner = crate::view::unit_view(&c, &a.land.units[&child]);
        assert!(owner.parent.is_none());
        assert!(owner.detail.unwrap()["assigned_to"].is_null());
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

#[cfg(test)]
mod query_error_controls {
    use super::*;
    use cna_core::engine::{EngineError, Rejection};

    fn setup() -> (CnaContent, State, UnitId) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let id: UnitId = "cw.2_nz_div.21st_nz_bn".into();
        let u = s.land.units.get_mut(&id).unwrap();
        u.transport_trucks = cna_content::units::Trucks::default();
        u.trucks = cna_content::units::Trucks::default();
        (c, s, id)
    }
    /// Cases: land:6.11, land:8.17
    #[test]
    fn verified_immobility_is_a_successful_zero_rating() {
        let (mut c, s, id) = setup();
        c.units.units.get_mut(&id).unwrap().immobile = true;
        let a = query_individual_allowance(&c, &s, &id, QueryAssignment::Trusted).unwrap();
        assert_eq!(a.cpa, 0);
        assert!(!a.motorized);
    }
    /// Cases: land:4.46, land:6.15, land:8.92
    #[test]
    fn missing_member_source_never_becomes_a_partial_group_minimum() {
        let (mut c, s, id) = setup();
        let other: UnitId = "cw.unassigned_inf.1st_rnf_mg_bn".into();
        assert!(s.land.units.contains_key(&other));
        c.units.units.remove(&other);
        let before = serde_json::to_value(&s).unwrap();
        assert!(matches!(query_members_allowance(&c, &s, &[id, other]),
            Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "land:4.46"));
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
    }
    /// Cases: land:8.91, land:8.93, airlog:54.2
    #[test]
    fn bad_submitted_assignment_and_corrupt_trusted_assignment_are_distinct() {
        let (c, mut s, id) = setup();
        s.land.units.get_mut(&id).unwrap().transport_trucks.medium = 1;
        assert!(matches!(
            query_individual_allowance(&c, &s, &id, QueryAssignment::Submitted),
            Err(Rejection::Illegal { .. })
        ));
        assert!(matches!(
            query_individual_allowance(&c, &s, &id, QueryAssignment::Trusted),
            Err(Rejection::Engine(EngineError::Invariant { .. }))
        ));
        s.land.units.get_mut(&id).unwrap().trucks.medium = -1;
        assert!(matches!(
            query_individual_allowance(&c, &s, &id, QueryAssignment::Submitted),
            Err(Rejection::Engine(EngineError::Invariant { .. }))
        ));
    }
    /// Cases: land:6.15, land:6.17, land:8.91, land:8.92
    #[test]
    fn known_group_minimum_matches_the_unchanged_allowance_algorithm() {
        let (c, s, id) = setup();
        let old = allowance(&c, &s, &id).unwrap();
        let checked = query_allowance(&c, &s, &id).unwrap();
        assert_eq!(old.cpa, checked.cpa);
        assert_eq!(old.motorized, checked.motorized);
        let absent: UnitId = "query-control-missing-member".into();
        assert!(matches!(
            query_members_allowance(&c, &s, &[id, absent]),
            Err(Rejection::Engine(EngineError::Invariant { .. }))
        ));
    }
    /// Cases: land:4.48, land:6.11, airlog:54.2
    #[test]
    fn unknown_weapon_is_source_gap_but_light_gun_transport_is_known_prohibited() {
        let (mut c, mut s, id) = setup();
        let class_id = c.units.units[&id].class.clone().unwrap();
        c.units.classes.get_mut(&class_id).unwrap().cpa_fixed = false;
        c.units.classes.get_mut(&class_id).unwrap().emplaced = false;
        c.units.units.get_mut(&id).unwrap().immobile = false;
        s.land.units.get_mut(&id).unwrap().toe =
            Some(Toe::Weapons(vec![cna_content::units::WeaponPoints {
                weapon: "source-control-missing-weapon".into(),
                n: 1,
            }]));
        assert!(
            matches!(query_individual_allowance(&c, &s, &id, QueryAssignment::Trusted),
            Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "land:4.48")
        );
        let gun = s
            .land
            .units
            .keys()
            .find(|id| class(&c, id).is_some_and(|cl| cl.unit_type == "artillery"))
            .unwrap()
            .clone();
        let u = s.land.units.get_mut(&gun).unwrap();
        u.trucks = cna_content::units::Trucks {
            light: 1,
            medium: 0,
            heavy: 0,
        };
        u.transport_trucks = u.trucks;
        assert!(matches!(
            query_individual_allowance(&c, &s, &gun, QueryAssignment::Submitted),
            Err(Rejection::Illegal { .. })
        ));
    }
}
