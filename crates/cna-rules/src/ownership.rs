//! Default seat ownership. Garrison sheet ids and explicit logistics kinds are content data;
//! support units follow an attached parent's command, otherwise belong to the rear area.
//! Campaign overrides are a later layer. This helper does not grant information access.

use crate::{CnaContent, State};
use cna_core::ids::UnitId;
use cna_protocol::Role;
use std::collections::BTreeSet;

/// Single assignment resolver, usable by both the runtime and an owner unit projection.
/// Absent runtime units (including future arrivals) retain their printed assignment.
/// Cases: land:19.11, land:19.13, land:19.14
pub fn assigned_parent<'a>(
    content: &'a CnaContent,
    id: &UnitId,
    unit: Option<&'a crate::state::LandUnit>,
) -> Option<&'a UnitId> {
    match unit.map(|u| &u.assignment) {
        Some(crate::state::Assignment::Independent) => None,
        Some(crate::state::Assignment::Parent(parent)) => Some(parent),
        Some(crate::state::Assignment::Printed) | None => {
            content.units.units.get(id)?.parent.as_ref()
        }
    }
}

/// The effective assigned parent; attachment and physical location do not change it.
/// Cases: land:19.11, land:19.13, land:19.14
pub fn assigned_parent_for_unit<'a>(
    content: &'a CnaContent,
    state: &'a State,
    id: &UnitId,
) -> Option<&'a UnitId> {
    assigned_parent(content, id, state.land.units.get(id))
}

/// Single attachment resolver for runtime and owner view adapters.
/// Explicit detachment suppresses all fallback; an attachment may differ from assignment.
/// Cases: land:9.21, land:19.11, land:19.13, land:19.14
pub fn parent_for_land_unit<'a>(
    content: &'a CnaContent,
    unit: &'a crate::state::LandUnit,
) -> Option<&'a UnitId> {
    if unit.detached {
        return None;
    }
    unit.attached_to
        .as_ref()
        .or_else(|| assigned_parent(content, &unit.id, Some(unit)))
}

/// Resolve the current attachment, falling back to the effective assignment unless detached.
/// Cases: land:9.21, land:19.11
pub fn parent_for_unit<'a>(
    content: &'a CnaContent,
    state: &'a State,
    id: &UnitId,
) -> Option<&'a UnitId> {
    parent_for_land_unit(content, state.land.units.get(id)?)
}

/// The default command role for a valid land unit; unknown ids conservatively use rear area.
/// Sheet ids containing `garrison` and the Matruh garrison are rear-area assignments. Explicit
/// second/third-line and convoy kinds belong to Logistics; first-line trucks move with their unit.
/// Cases: land:3.31, land:8.18, land:8.98, land:19.11
pub fn seat_for_unit(content: &CnaContent, state: &State, unit_id: &UnitId) -> Role {
    let mut seen = BTreeSet::new();
    let mut id = unit_id;
    loop {
        if !seen.insert(id) {
            return Role::RearArea;
        }
        let Some(oa) = content.units.units.get(id) else {
            return Role::RearArea;
        };
        if oa.sheet.contains("garrison") || id.as_str() == "cw.selby_force.matruh_garrison_hq" {
            return Role::RearArea;
        }
        if matches!(
            oa.kind.as_deref(),
            Some("convoy" | "second_line_truck" | "third_line_truck")
        ) {
            return Role::Logistics;
        }
        let class = oa.class.as_ref().and_then(|c| content.units.classes.get(c));
        let support = matches!(
            class.map(|c| c.unit_type.as_str()),
            Some("engineer" | "construction" | "anti_air" | "coastal_defense")
        ) || oa.id.as_str().contains("coastal_def");
        if !support {
            return Role::FrontLine;
        }
        match parent_for_unit(content, state, id) {
            Some(parent) => id = parent,
            None => return Role::RearArea,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn content() -> CnaContent {
        CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
    }
    /// Cases: land:19.11, land:19.13, land:19.14
    #[test]
    fn assignment_and_attachment_are_distinct_and_old_checkpoints_keep_printed_fallback() {
        use crate::state::{Assignment, LandUnit};
        let c = content();
        let mut s = State::new(&c).unwrap();
        let child: UnitId = "it.1_libyan_div.viii_libyan_bn".into();
        let original = assigned_parent_for_unit(&c, &s, &child).unwrap().clone();
        let other: UnitId = "it.1_libyan_div.2nd_libyan_regt_hq".into();
        let mut old = serde_json::to_value(&s.land.units[&child]).unwrap();
        old.as_object_mut().unwrap().remove("assignment");
        let restored: LandUnit = serde_json::from_value(old).unwrap();
        assert_eq!(restored.assignment, Assignment::Printed);
        s.land.units.get_mut(&child).unwrap().assignment = Assignment::Parent(other.clone());
        assert_eq!(assigned_parent_for_unit(&c, &s, &child), Some(&other));
        assert_eq!(parent_for_unit(&c, &s, &child), Some(&other));
        s.land.units.get_mut(&child).unwrap().attached_to = Some(original.clone());
        assert_eq!(assigned_parent_for_unit(&c, &s, &child), Some(&other));
        assert_eq!(parent_for_unit(&c, &s, &child), Some(&original));
        s.land.units.get_mut(&child).unwrap().detached = true;
        assert!(parent_for_unit(&c, &s, &child).is_none());
        assert_eq!(assigned_parent_for_unit(&c, &s, &child), Some(&other));
        s.land.units.get_mut(&child).unwrap().assignment = Assignment::Independent;
        assert!(assigned_parent_for_unit(&c, &s, &child).is_none());
        assert_eq!(
            serde_json::from_value::<LandUnit>(
                serde_json::to_value(&s.land.units[&child]).unwrap()
            )
            .unwrap()
            .assignment,
            Assignment::Independent
        );
        // No runtime entry is also a printed fallback, not invented independence.
        s.land.units.remove(&child);
        assert_eq!(assigned_parent_for_unit(&c, &s, &child), Some(&original));
    }
    /// Cases: land:8.18, land:19.11
    #[test]
    fn default_roles_follow_real_sheets_and_current_attachments() {
        let mut content = content();
        let mut state = State::new(&content).unwrap();
        let field = UnitId::new("it.1_libyan_div.1st_libyan_infantry_hq");
        let convoy = UnitId::new("it.1_libyan_div.viii_libyan_bn");
        content.units.units.get_mut(&convoy).unwrap().kind = Some("second_line_truck".into());
        assert_eq!(seat_for_unit(&content, &state, &convoy), Role::Logistics);
        let garrison = UnitId::new("it.bardia_garrison.30th_gaf_s_di_c_hq");
        let engineer = UnitId::new("it.1ccnn_div.201st_engineer_bn");
        assert_eq!(seat_for_unit(&content, &state, &field), Role::FrontLine);
        assert_eq!(seat_for_unit(&content, &state, &garrison), Role::RearArea);
        assert_eq!(seat_for_unit(&content, &state, &engineer), Role::FrontLine);
        state.land.units.get_mut(&engineer).unwrap().detached = true;
        assert_eq!(seat_for_unit(&content, &state, &engineer), Role::RearArea);
        let support = state.land.units.get_mut(&engineer).unwrap();
        support.detached = false;
        support.attached_to = Some(garrison);
        assert_eq!(seat_for_unit(&content, &state, &engineer), Role::RearArea);
    }
}
