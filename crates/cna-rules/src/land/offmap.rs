//! Checkpointed journeys through the Tripoli/Tunisia region.
use std::collections::{BTreeMap, BTreeSet};

use cna_core::ids::UnitId;
use cna_protocol::Side;
use cna_tables::land::administration::OffMapPlace;
use serde::{Deserialize, Serialize};

use crate::state::{Location, State};

/// A journey keeps its original formation and rate until it ends or returns to its origin.
/// Cases: land:8.81, land:8.83, land:8.84
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transit {
    pub from: OffMapPlace,
    pub to: OffMapPlace,
    pub completed_stages: i32,
    pub required_stages: i32,
    pub basic_cpa: i32,
    /// Opaque physical co-location identity, outside the named-location catalogue.
    pub group: String,
    /// Exact represented members at departure, including the root map key.
    pub members: Vec<UnitId>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OffMapState {
    pub transit: BTreeMap<UnitId, Transit>,
    /// Serial for deterministic group identities, retained through recovery.
    pub next_group: u64,
}

/// Verify the unit's actual saved membership and group location; a name alone grants no rights.
/// The caller may use this fact for unlimited water consumption, never for creating supplies.
/// Cases: land:8.81, land:8.82, land:8.87
pub fn is_in_transit(state: &State, id: &UnitId) -> bool {
    transit_for_unit(state, id).is_some()
}

/// The owning side may inspect the saved journey of a represented member.
/// Cases: land:8.81, land:8.84, land:8.87
pub fn transit_for_unit<'a>(state: &'a State, id: &UnitId) -> Option<&'a Transit> {
    let unit = state.land.units.get(id)?;
    if unit.side != Side::Axis {
        return None;
    }
    state.land.off_map.transit.iter().find_map(|(root, leg)| {
        if leg.group.is_empty()
            || leg.from == leg.to
            || leg.basic_cpa <= 0
            || leg.required_stages <= 1
            || leg.completed_stages <= 0
            || leg.completed_stages >= leg.required_stages
            || !leg.members.contains(root)
            || !leg.members.contains(id)
            || !matches!(&unit.location, Location::OffMap { id } if id == &leg.group)
        {
            return None;
        }
        let mut seen = BTreeSet::new();
        leg.members
            .iter()
            .all(|member| {
                seen.insert(member)
                    && state
                        .land
                        .units
                        .get(member)
                        .is_some_and(|u| u.side == Side::Axis)
            })
            .then_some(leg)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cna, CnaContent};

    const ROOT: &str = "it.libyan_tank_command.xxi_l_tank_bn";
    const CHILD: &str = "it.libyan_tank_command.lxii_l_tank_bn";
    const ENEMY: &str = "cw.unassigned_inf.1st_rnf_mg_bn";

    fn setup() -> (CnaContent, State, UnitId, UnitId) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let root = UnitId::new(ROOT);
        let child = UnitId::new(CHILD);
        let group = "opaque-journey-example".to_string();
        for id in [&root, &child] {
            s.land.units.get_mut(id).unwrap().location = Location::OffMap { id: group.clone() };
        }
        s.land.off_map.transit.insert(
            root.clone(),
            Transit {
                from: OffMapPlace::Tripoli,
                to: OffMapPlace::Nofilia,
                completed_stages: 1,
                required_stages: 2,
                basic_cpa: 25,
                group,
                members: vec![root.clone(), child.clone()],
            },
        );
        (c, s, root, child)
    }

    /// Cases: land:8.81, land:8.82, land:8.87
    #[test]
    fn transit_rights_require_checkpointed_members_and_actual_group_location() {
        let (_, s, root, child) = setup();
        assert!(is_in_transit(&s, &root));
        assert!(is_in_transit(&s, &child));
        let mut forged = s.clone();
        forged.land.off_map.transit.clear();
        forged.land.units.get_mut(&root).unwrap().location = Location::OffMap {
            id: "land-transit:axis:0".into(),
        };
        assert!(!is_in_transit(&forged, &root));
        let mut separated = s.clone();
        separated.land.units.get_mut(&child).unwrap().location = Location::OffMap {
            id: "box_tripoli".into(),
        };
        assert!(!is_in_transit(&separated, &child));
        // A trip cannot progress with a changed formation, but those still physically
        // in its Transit box retain water consumption under 8.87.
        assert!(is_in_transit(&separated, &root));
        let mut duplicate = s.clone();
        duplicate
            .land
            .off_map
            .transit
            .get_mut(&root)
            .unwrap()
            .members
            .push(child.clone());
        assert!(!is_in_transit(&duplicate, &root));
        let mut bad_progress = s.clone();
        bad_progress
            .land
            .off_map
            .transit
            .get_mut(&root)
            .unwrap()
            .completed_stages = 2;
        assert!(!is_in_transit(&bad_progress, &root));
        bad_progress
            .land
            .off_map
            .transit
            .get_mut(&root)
            .unwrap()
            .completed_stages = 0;
        assert!(!is_in_transit(&bad_progress, &root));
        let enemy = UnitId::new(ENEMY);
        let mut wrong_side = s.clone();
        let group = wrong_side.land.off_map.transit[&root].group.clone();
        wrong_side.land.units.get_mut(&enemy).unwrap().location = Location::OffMap { id: group };
        wrong_side
            .land
            .off_map
            .transit
            .get_mut(&root)
            .unwrap()
            .members
            .push(enemy.clone());
        assert!(!is_in_transit(&wrong_side, &enemy));
        assert!(!is_in_transit(&wrong_side, &root));
    }

    /// Cases: land:8.84, land:8.87, land:3.6
    #[test]
    fn checkpoint_keeps_transit_private_and_old_state_has_no_journeys() {
        let (c, mut s, root, child) = setup();
        let restored: State = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert!(is_in_transit(&restored, &root));
        assert!(is_in_transit(&restored, &child));
        let own = crate::view::inspect(
            &c,
            &restored,
            cna_core::visibility::Perspective::Side(Side::Axis),
            child.as_str(),
            false,
        )
        .unwrap();
        assert_eq!(own["transit"]["completed_stages"], serde_json::json!(1));
        assert!(
            crate::view::inspect(
                &c,
                &restored,
                cna_core::visibility::Perspective::Side(Side::Commonwealth),
                root.as_str(),
                false,
            )
            .is_err()
        );
        assert_eq!(
            restored.land.off_map.transit[&root],
            s.land.off_map.transit[&root]
        );
        let mut old = serde_json::to_value(&s).unwrap();
        old["land"].as_object_mut().unwrap().remove("off_map");
        let old: State = serde_json::from_value(old).unwrap();
        assert!(old.land.off_map.transit.is_empty());
        s.land
            .off_map
            .transit
            .get_mut(&root)
            .unwrap()
            .completed_stages = 2;
        crate::testkit::assert_indistinguishable(
            &Cna::dev(),
            &c,
            &s,
            &restored,
            Side::Commonwealth,
        );
    }
}
