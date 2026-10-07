use super::*;
use crate::{Cna, State, state::Location, testkit::assert_indistinguishable};
use cna_core::ids::HexId;

fn fixture() -> (CnaContent, State, HexId) {
    let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut state = State::new(&content).unwrap();
    state.setup.closed = true;
    inventory::initialize(&content, &mut state).unwrap();
    let occupied: Vec<_> = state
        .air
        .runtime
        .facilities
        .iter()
        .map(|(id, site)| site.properties(&content, id).unwrap().location)
        .collect();
    let hex = content
        .map
        .iter()
        .find(|h| !occupied.contains(&Location::Hex { hex: h.id.clone() }))
        .unwrap()
        .id
        .clone();
    (content, state, hex)
}

// TEST-ONLY stand-in for the required, still-unimplemented Engineering caller.
// It is not a live project field/API or proof of caller integration. Every
// completion authenticates unique ACTIVE id + exact side/work/target and commits
// Completed with the Air effect in one disposable draft before replay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Project {
    id: String,
    active: bool,
    side: Side,
    work: FacilityWork,
    target: FacilityId,
    receipt: StartedFacilityWork,
}

impl Project {
    fn start(
        content: &CnaContent,
        state: &mut State,
        id: &str,
        side: Side,
        work: FacilityWork,
    ) -> Self {
        assert!(!id.is_empty()); // tests use unique project IDs, never site IDs
        let receipt = start_work(content, &mut state.air, side, &work).unwrap();
        Self {
            id: id.into(),
            active: true,
            side,
            work,
            target: receipt.target.clone(),
            receipt,
        }
    }
    fn complete(
        &mut self,
        content: &CnaContent,
        state: &mut State,
        active_id: &str,
    ) -> Result<(), EngineError> {
        if !self.active
            || self.id != active_id
            || self.side != self.receipt.side
            || self.work != self.receipt.work
            || self.target != self.receipt.target
        {
            return Err(invalid("test caller requires exact ACTIVE project"));
        }
        let mut draft = state.clone();
        complete_work(content, &mut draft.air, &self.receipt)?;
        let mut project = self.clone();
        project.active = false;
        *state = draft;
        *self = project;
        Ok(())
    }
}

/// Cases: land:24.71, land:24.79, airlog:36.12, airlog:36.2, airlog:36.3, airlog:36.4
#[test]
fn create_derives_all_capacities_ids_and_active_project_checkpoint_completion() {
    for (kind, levels, class) in [
        (FacilityKind::Airfield, 6, "air"),
        (FacilityKind::LandingStrip, 1, "air"),
        (FacilityKind::FlyingBoatBasin, 3, "water"),
        (FacilityKind::FlyingBoatAlightingArea, 1, "water"),
    ] {
        let (content, mut state, hex) = fixture();
        let inventory_before = (
            state.air.runtime.aircraft.clone(),
            state.air.runtime.pilots.clone(),
        );
        let work = FacilityWork::Create {
            kind,
            hex: hex.clone(),
        };
        let mut project =
            Project::start(&content, &mut state, "project.create.1", Side::Axis, work);
        let target = FacilityId(format!("constructed.{class}.{hex}"));
        assert_eq!(project.target, target);
        let site = &state.air.runtime.facilities[&target];
        assert_eq!(site.current_capacity, FacilityCapacity::Levels(levels));
        assert!(site.project_unavailable);
        assert!(!site.operational());
        assert_eq!(site.intrinsic_aa(true).unwrap(), 0);
        let state_bytes = serde_json::to_vec(&state).unwrap();
        let project_bytes = serde_json::to_vec(&project).unwrap();
        state = serde_json::from_slice(&state_bytes).unwrap();
        project = serde_json::from_slice(&project_bytes).unwrap();
        project
            .complete(&content, &mut state, "project.create.1")
            .unwrap();
        assert!(!project.active);
        assert!(state.air.runtime.facilities[&target].operational());
        assert_eq!(
            state.air.runtime.facilities[&target]
                .intrinsic_aa(true)
                .unwrap(),
            1
        );
        assert_eq!(
            state.air.runtime.facilities[&target]
                .intrinsic_aa(false)
                .unwrap(),
            0
        );
        assert_eq!(
            inventory_before,
            (
                state.air.runtime.aircraft.clone(),
                state.air.runtime.pilots.clone()
            )
        );
        let after = serde_json::to_vec(&state).unwrap();
        assert!(
            project
                .complete(&content, &mut state, "project.create.1")
                .is_err()
        );
        assert_eq!(after, serde_json::to_vec(&state).unwrap());
    }
}

/// Cases: land:24.79, airlog:36.2, airlog:36.4
#[test]
fn class_limit_includes_started_projects_allows_water_plus_air_and_failed_starts_are_atomic() {
    let (content, mut state, hex) = fixture();
    let air = FacilityWork::Create {
        kind: FacilityKind::LandingStrip,
        hex: hex.clone(),
    };
    let mut project = Project::start(
        &content,
        &mut state,
        "project.air.1",
        Side::Axis,
        air.clone(),
    );
    let before = serde_json::to_vec(&state).unwrap();
    assert!(start_work(&content, &mut state.air, Side::Axis, &air).is_err());
    assert_eq!(before, serde_json::to_vec(&state).unwrap());
    let mut water_project = Project::start(
        &content,
        &mut state,
        "project.water.1",
        Side::Axis,
        FacilityWork::Create {
            kind: FacilityKind::FlyingBoatAlightingArea,
            hex: hex.clone(),
        },
    );
    assert_ne!(project.target, water_project.target);
    project
        .complete(&content, &mut state, "project.air.1")
        .unwrap();
    water_project
        .complete(&content, &mut state, "project.water.1")
        .unwrap();
    let before = serde_json::to_vec(&state).unwrap();
    for work in [
        air,
        FacilityWork::Create {
            kind: FacilityKind::Airfield,
            hex,
        },
        FacilityWork::Create {
            kind: FacilityKind::Airfield,
            hex: "unknown".into(),
        },
    ] {
        assert!(start_work(&content, &mut state.air, Side::Axis, &work).is_err());
        assert_eq!(before, serde_json::to_vec(&state).unwrap());
    }
}

/// Cases: land:24.79, airlog:36.2, airlog:36.3, airlog:36.4, airlog:36.18
/// Interpretations: interp:air-0007
#[test]
fn upgrades_keep_site_identity_suspend_aa_and_complete_under_exact_active_project() {
    for (kind, target_kind, levels) in [
        (FacilityKind::LandingStrip, FacilityKind::Airfield, 6),
        (
            FacilityKind::FlyingBoatAlightingArea,
            FacilityKind::FlyingBoatBasin,
            3,
        ),
    ] {
        let (content, mut state, hex) = fixture();
        let mut create = Project::start(
            &content,
            &mut state,
            "project.create.1",
            Side::Axis,
            FacilityWork::Create { kind, hex },
        );
        create
            .complete(&content, &mut state, "project.create.1")
            .unwrap();
        let target = create.target.clone();
        let origin = state.air.runtime.facilities[&target].origin.clone();
        let mut project = Project::start(
            &content,
            &mut state,
            "project.upgrade.2",
            Side::Axis,
            FacilityWork::Upgrade { id: target.clone() },
        );
        assert_eq!(
            state.air.runtime.facilities[&target]
                .intrinsic_aa(true)
                .unwrap(),
            0
        );
        let before = serde_json::to_vec(&state).unwrap();
        assert!(
            project
                .complete(&content, &mut state, "obsolete.project")
                .is_err()
        );
        assert_eq!(before, serde_json::to_vec(&state).unwrap());
        let mut tampered = project.clone();
        tampered.target = FacilityId("other.site".into());
        assert!(
            tampered
                .complete(&content, &mut state, "project.upgrade.2")
                .is_err()
        );
        assert_eq!(before, serde_json::to_vec(&state).unwrap());
        project
            .complete(&content, &mut state, "project.upgrade.2")
            .unwrap();
        let site = &state.air.runtime.facilities[&target];
        assert_eq!(site.origin, origin);
        assert_eq!(site.upgraded_kind, Some(target_kind));
        assert_eq!(site.current_capacity, FacilityCapacity::Levels(levels));
        assert!(site.operational());
        assert_eq!(site.intrinsic_aa(true).unwrap(), 1);
        assert_eq!(site.intrinsic_aa(false).unwrap(), 0);
        let after = serde_json::to_vec(&state).unwrap();
        assert!(
            project
                .complete(&content, &mut state, "project.upgrade.2")
                .is_err()
        );
        assert_eq!(after, serde_json::to_vec(&state).unwrap());
    }
}

/// Cases: land:24.76, airlog:36.14
#[test]
fn repair_one_requires_active_caller_guard_and_direct_air_replay_can_add_a_second_level() {
    let (content, mut state, _) = fixture();
    let id = FacilityId("airfield_benina".into());
    inventory::update(&content, &mut state.air, |runtime| {
        runtime.facilities.get_mut(&id).unwrap().current_capacity = FacilityCapacity::Levels(2);
        Ok(())
    })
    .unwrap();
    let mut project = Project::start(
        &content,
        &mut state,
        "project.repair.1",
        Side::Axis,
        FacilityWork::RepairOne { id: id.clone() },
    );
    assert!(state.air.runtime.facilities[&id].operational());
    let mut direct = state.clone();
    // Deliberately violating the documented caller contract demonstrates why
    // the receipt itself is NOT an exactly-once project capability.
    complete_work(&content, &mut direct.air, &project.receipt).unwrap();
    complete_work(&content, &mut direct.air, &project.receipt).unwrap();
    assert_eq!(
        direct.air.runtime.facilities[&id].current_capacity,
        FacilityCapacity::Levels(4)
    );
    project
        .complete(&content, &mut state, "project.repair.1")
        .unwrap();
    assert_eq!(
        state.air.runtime.facilities[&id].current_capacity,
        FacilityCapacity::Levels(3)
    );
    let bytes = serde_json::to_vec(&state).unwrap();
    assert!(
        project
            .complete(&content, &mut state, "project.repair.1")
            .is_err()
    );
    assert_eq!(bytes, serde_json::to_vec(&state).unwrap());
    let mut second = Project::start(
        &content,
        &mut state,
        "project.repair.2",
        Side::Axis,
        FacilityWork::RepairOne { id: id.clone() },
    );
    second
        .complete(&content, &mut state, "project.repair.2")
        .unwrap();
    assert_eq!(
        state.air.runtime.facilities[&id].current_capacity,
        FacilityCapacity::Levels(4)
    );
}

/// Cases: land:24.76, land:24.79, airlog:36.14
#[test]
fn failed_completions_preserve_active_project_air_bytes_and_do_not_infer_cancellation() {
    let (content, mut state, hex) = fixture();
    let mut create = Project::start(
        &content,
        &mut state,
        "project.create.1",
        Side::Axis,
        FacilityWork::Create {
            kind: FacilityKind::LandingStrip,
            hex,
        },
    );
    create
        .complete(&content, &mut state, "project.create.1")
        .unwrap();
    let id = create.target.clone();
    let mut upgrade = Project::start(
        &content,
        &mut state,
        "project.upgrade.2",
        Side::Axis,
        FacilityWork::Upgrade { id: id.clone() },
    );
    for captured in [false, true] {
        let mut changed = state.clone();
        inventory::update(&content, &mut changed.air, |runtime| {
            let site = runtime.facilities.get_mut(&id).unwrap();
            if captured {
                site.owner = Side::Commonwealth;
            } else {
                site.current_capacity = FacilityCapacity::Levels(0);
            }
            Ok(())
        })
        .unwrap();
        let before = serde_json::to_vec(&changed).unwrap();
        let project_before = serde_json::to_vec(&upgrade).unwrap();
        assert!(
            upgrade
                .complete(&content, &mut changed, "project.upgrade.2")
                .is_err()
        );
        assert_eq!(before, serde_json::to_vec(&changed).unwrap());
        assert_eq!(project_before, serde_json::to_vec(&upgrade).unwrap());
        assert!(changed.air.runtime.facilities[&id].project_unavailable);
    }
}

/// Cases: land:24.77, land:24.79, airlog:36.2, airlog:36.4
#[test]
fn reconstruction_preserves_scenario_tombstones_and_requires_caller_incarnation_guard() {
    let (content, mut state, hex) = fixture();
    let scenario_id = state
        .air
        .runtime
        .facilities
        .iter()
        .find_map(|(id, site)| {
            let p = site.properties(&content, id).unwrap();
            (p.kind == FacilityKind::LandingStrip && matches!(p.location, Location::Hex { .. }))
                .then_some(id.clone())
        })
        .unwrap();
    let p = state.air.runtime.facilities[&scenario_id]
        .properties(&content, &scenario_id)
        .unwrap();
    let Location::Hex { hex: source_hex } = p.location else {
        unreachable!()
    };
    inventory::update(&content, &mut state.air, |runtime| {
        runtime
            .facilities
            .get_mut(&scenario_id)
            .unwrap()
            .current_capacity = FacilityCapacity::Levels(0);
        Ok(())
    })
    .unwrap();
    let tombstone = state.air.runtime.facilities[&scenario_id].clone();
    let mut replacement = Project::start(
        &content,
        &mut state,
        "project.source.replacement.1",
        Side::Axis,
        FacilityWork::Create {
            kind: FacilityKind::LandingStrip,
            hex: source_hex,
        },
    );
    assert_ne!(replacement.target, scenario_id);
    replacement
        .complete(&content, &mut state, "project.source.replacement.1")
        .unwrap();
    assert_eq!(state.air.runtime.facilities[&scenario_id], tombstone);
    let work = FacilityWork::Create {
        kind: FacilityKind::LandingStrip,
        hex,
    };
    let mut old = Project::start(
        &content,
        &mut state,
        "project.constructed.1",
        Side::Axis,
        work.clone(),
    );
    old.complete(&content, &mut state, "project.constructed.1")
        .unwrap();
    let mut obsolete_active_copy = old.clone();
    obsolete_active_copy.active = true;
    let target = old.target.clone();
    inventory::update(&content, &mut state.air, |runtime| {
        runtime
            .facilities
            .get_mut(&target)
            .unwrap()
            .current_capacity = FacilityCapacity::Levels(0);
        Ok(())
    })
    .unwrap();
    // Guard retired the old project BEFORE reusing the same site identity.
    assert!(!old.active);
    let mut new = Project::start(
        &content,
        &mut state,
        "project.constructed.2",
        Side::Axis,
        work,
    );
    assert_eq!(new.target, target);
    let before = serde_json::to_vec(&state).unwrap();
    assert!(
        old.complete(&content, &mut state, "project.constructed.1")
            .is_err()
    );
    assert_eq!(before, serde_json::to_vec(&state).unwrap());
    // The actual current ProjectId/incarnation guard rejects an old ACTIVE
    // checkpoint too; its Air receipt and unchanged SITE id cannot prove this.
    assert!(
        obsolete_active_copy
            .complete(&content, &mut state, "project.constructed.2")
            .is_err()
    );
    assert_eq!(before, serde_json::to_vec(&state).unwrap());
    new.complete(&content, &mut state, "project.constructed.2")
        .unwrap();
}

/// Cases: land:3.62, land:24.76, land:24.79, airlog:36.18
#[test]
fn hidden_construction_and_active_completion_effects_stay_indistinguishable_to_enemy() {
    let (content, a, hex) = fixture();
    let mut b = a.clone();
    let mut create = Project::start(
        &content,
        &mut b,
        "project.create.private.1",
        Side::Axis,
        FacilityWork::Create {
            kind: FacilityKind::LandingStrip,
            hex,
        },
    );
    assert_indistinguishable(&Cna::dev(), &content, &a, &b, Side::Commonwealth);
    create
        .complete(&content, &mut b, "project.create.private.1")
        .unwrap();
    assert_indistinguishable(&Cna::dev(), &content, &a, &b, Side::Commonwealth);
    let mut upgrade = Project::start(
        &content,
        &mut b,
        "project.upgrade.private.2",
        Side::Axis,
        FacilityWork::Upgrade { id: create.target },
    );
    assert_indistinguishable(&Cna::dev(), &content, &a, &b, Side::Commonwealth);
    upgrade
        .complete(&content, &mut b, "project.upgrade.private.2")
        .unwrap();
    assert_indistinguishable(&Cna::dev(), &content, &a, &b, Side::Commonwealth);
    let id = FacilityId("airfield_benina".into());
    inventory::update(&content, &mut b.air, |runtime| {
        runtime.facilities.get_mut(&id).unwrap().current_capacity = FacilityCapacity::Levels(2);
        Ok(())
    })
    .unwrap();
    let mut repair = Project::start(
        &content,
        &mut b,
        "project.repair.private.3",
        Side::Axis,
        FacilityWork::RepairOne { id },
    );
    repair
        .complete(&content, &mut b, "project.repair.private.3")
        .unwrap();
    assert_indistinguishable(&Cna::dev(), &content, &a, &b, Side::Commonwealth);
}

/// Cases: land:24.79, airlog:36.2, airlog:36.18
/// Interpretations: interp:air-0007
#[test]
fn existing_scenario_upgrade_retains_source_identity_under_active_project_contract() {
    let (content, mut state, _) = fixture();
    let (id, side) = state
        .air
        .runtime
        .facilities
        .iter()
        .find_map(|(id, site)| {
            let p = site.properties(&content, id).unwrap();
            (p.kind == FacilityKind::LandingStrip && matches!(p.location, Location::Hex { .. }))
                .then_some((id.clone(), site.owner))
        })
        .unwrap();
    let before = state.air.runtime.facilities[&id]
        .properties(&content, &id)
        .unwrap();
    let mut project = Project::start(
        &content,
        &mut state,
        "project.scenario.upgrade.1",
        side,
        FacilityWork::Upgrade { id: id.clone() },
    );
    assert_eq!(project.target, id);
    assert_eq!(
        state.air.runtime.facilities[&id]
            .intrinsic_aa(true)
            .unwrap(),
        0
    );
    project
        .complete(&content, &mut state, "project.scenario.upgrade.1")
        .unwrap();
    let site = &state.air.runtime.facilities[&id];
    assert_eq!(site.origin, FacilityOrigin::Scenario);
    let after = site.properties(&content, &id).unwrap();
    assert_eq!(after.location, before.location);
    assert_eq!(after.kind, FacilityKind::Airfield);
    assert_eq!(after.printed_capacity, FacilityCapacity::Levels(6));
    assert_eq!(site.current_capacity, FacilityCapacity::Levels(6));
}

/// Cases: land:24.76, land:24.79, airlog:36.5, airlog:44.14
#[test]
fn unknown_offmap_malta_and_tampered_project_effects_reject_without_state_changes() {
    let (content, mut state, hex) = fixture();
    let offmap = state
        .air
        .runtime
        .facilities
        .iter()
        .find_map(|(id, site)| {
            let p = site.properties(&content, id).unwrap();
            matches!(p.location, Location::OffMap { .. }).then_some((id.clone(), site.owner))
        })
        .unwrap();
    for (side, id, unsupported_case) in [
        (Side::Axis, FacilityId("unknown.site".into()), None),
        (
            Side::Commonwealth,
            FacilityId("malta.initial".into()),
            Some("airlog:44.14"),
        ),
        (offmap.1, offmap.0, Some("airlog:36.5")),
    ] {
        let before = serde_json::to_vec(&state).unwrap();
        let result = start_work(
            &content,
            &mut state.air,
            side,
            &FacilityWork::RepairOne { id },
        );
        if let Some(expected) = unsupported_case {
            assert!(matches!(result,
                Err(EngineError::Unsupported { case, .. }) if case == expected));
        } else {
            assert!(result.is_err());
        }
        assert_eq!(before, serde_json::to_vec(&state).unwrap());
    }
    let project = Project::start(
        &content,
        &mut state,
        "project.exact.1",
        Side::Axis,
        FacilityWork::Create {
            kind: FacilityKind::LandingStrip,
            hex,
        },
    );
    let before = serde_json::to_vec(&state).unwrap();
    for fact in 0..3 {
        // The real caller must first authenticate its exact ACTIVE project and
        // side/work/target. Direct calls still revalidate bounded Air effects.
        let mut receipt = project.receipt.clone();
        match fact {
            0 => receipt.side = Side::Commonwealth,
            1 => receipt.target = FacilityId("airfield_benina".into()),
            2 => {
                receipt.work = FacilityWork::Upgrade {
                    id: FacilityId("other.site".into()),
                }
            }
            _ => unreachable!(),
        }
        assert!(complete_work(&content, &mut state.air, &receipt).is_err());
        assert_eq!(before, serde_json::to_vec(&state).unwrap());
    }
}
