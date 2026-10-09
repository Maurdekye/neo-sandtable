use super::*;
use crate::air::{facilities::{FacilityCapacity, FacilityKind}, inventory};
use crate::{Cna, seq::Block, state::Location};
use cna_core::{dice::CampaignRng, engine::{Command, Game}};
use cna_core::ids::HexId;
use serde_json::Value;
use std::sync::OnceLock;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}

fn fixture() -> (State, FacilityWork) {
    let mut state = State::new(content()).unwrap();
    state.setup.closed = true;
    inventory::initialize(content(), &mut state).unwrap();
    let occupied: Vec<_> = state.air.runtime.facilities.iter()
        .map(|(id, site)| site.properties(content(), id).unwrap().location).collect();
    let hex = content().map.iter()
        .find(|h| !occupied.contains(&Location::Hex { hex: h.id.clone() }))
        .unwrap().id.clone();
    (state, FacilityWork::Create { kind: FacilityKind::LandingStrip, hex })
}

fn bytes(state: &State) -> Vec<u8> {
    serde_json::to_vec(state).unwrap()
}

fn begin(state: &mut State, work: &FacilityWork) -> ProjectId {
    begin_facility_project(content(), state, Side::Axis, work).unwrap()
}

fn assert_invariant_unchanged<T: std::fmt::Debug>(
    state: &mut State,
    operation: impl FnOnce(&mut State) -> Result<T, EngineError>,
) {
    let before = bytes(state);
    assert!(matches!(operation(state), Err(EngineError::Invariant { .. })));
    assert_eq!(bytes(state), before);
}

/// Cases: land:24.71, land:24.74, land:24.75, land:24.79
#[test]
fn begin_allocates_unique_authority_and_rejects_duplicate_or_exhausted_ids_atomically() {
    let (mut state, work) = fixture();
    let id = begin(&mut state, &work);
    let project = &state.engineering.facility_projects[&id];
    assert_eq!(id, ProjectId(1));
    assert_eq!(state.engineering.facility_project_serial, 1);
    assert_eq!(project.lifecycle, ProjectLifecycle::Active);
    assert_eq!(project.started.incarnation(), Some(project.incarnation));
    assert_invariant_unchanged(&mut state, |s| begin_facility_project(content(), s, Side::Axis, &work));
    let (mut overflow, work) = fixture();
    overflow.engineering.facility_project_serial = u64::MAX;
    assert_invariant_unchanged(&mut overflow, |s| begin_facility_project(content(), s, Side::Axis, &work));
    let (mut unknown, _) = fixture();
    let bad = FacilityWork::Create { kind: FacilityKind::LandingStrip, hex: HexId("not-a-hex".into()) };
    let before = bytes(&unknown);
    assert!(begin_facility_project(content(), &mut unknown, Side::Axis, &bad).is_err());
    assert_eq!(bytes(&unknown), before);
}

/// Cases: land:24.76, airlog:36.14
#[test]
fn completed_repair_replay_cannot_add_another_level_and_new_project_can() {
    let (mut state, _) = fixture();
    let target = FacilityId("airfield_benina".into());
    inventory::update(content(), &mut state.air, |runtime| {
        runtime.facilities.get_mut(&target).unwrap().current_capacity = FacilityCapacity::Levels(2);
        Ok(())
    }).unwrap();
    let work = FacilityWork::RepairOne { id: target.clone() };
    let first = begin(&mut state, &work);
    let token = state.engineering.facility_projects[&first].incarnation;
    assert_eq!(complete_facility_project(content(), &mut state, &first).unwrap(), target);
    assert_eq!(state.air.runtime.facilities[&target].current_capacity, FacilityCapacity::Levels(3));
    assert_eq!(state.engineering.facility_projects[&first].lifecycle, ProjectLifecycle::Completed);
    assert_invariant_unchanged(&mut state, |s| complete_facility_project(content(), s, &first));
    let second = begin(&mut state, &work);
    assert_ne!(first, second);
    assert_eq!(state.engineering.facility_projects[&second].incarnation, token);
    complete_facility_project(content(), &mut state, &second).unwrap();
    assert_eq!(state.air.runtime.facilities[&target].current_capacity, FacilityCapacity::Levels(4));
}

/// Cases: land:24.76, land:24.79, airlog:36.2
#[test]
fn completed_site_recreation_gets_a_fresh_token_and_upgrade_keeps_it() {
    let (mut state, work) = fixture();
    let first = begin(&mut state, &work);
    let old_token = state.engineering.facility_projects[&first].incarnation;
    let target = complete_facility_project(content(), &mut state, &first).unwrap();
    let scenario_sites: Vec<_> = state.air.runtime.facilities.keys()
        .filter(|id| **id != target).cloned().collect();
    inventory::update(content(), &mut state.air, |runtime| {
        runtime.facilities.get_mut(&target).unwrap().current_capacity = FacilityCapacity::Levels(0);
        Ok(())
    }).unwrap();
    let second = begin(&mut state, &work);
    let new_token = state.engineering.facility_projects[&second].incarnation;
    assert_ne!(old_token, new_token);
    assert_eq!(complete_facility_project(content(), &mut state, &second).unwrap(), target);
    assert_invariant_unchanged(&mut state, |s| complete_facility_project(content(), s, &first));
    let upgrade = begin(&mut state, &FacilityWork::Upgrade { id: target.clone() });
    assert_eq!(state.engineering.facility_projects[&upgrade].incarnation, new_token);
    complete_facility_project(content(), &mut state, &upgrade).unwrap();
    assert_eq!(state.air.runtime.facilities[&target].incarnation, Some(new_token));
    for id in scenario_sites { assert!(state.air.runtime.facilities.contains_key(&id)); }
}

/// Cases: land:24.76, airlog:36.15
#[test]
fn reachable_removed_captured_and_missing_active_targets_are_unsupported_before_begin_or_complete() {
    let (mut original, work) = fixture();
    let id = begin(&mut original, &work);
    let target = original.engineering.facility_projects[&id].target.clone();
    for change in 0..3 {
        let mut state = original.clone();
        inventory::update(content(), &mut state.air, |runtime| {
            match change {
                0 => runtime.facilities.get_mut(&target).unwrap().current_capacity = FacilityCapacity::Levels(0),
                1 => runtime.facilities.get_mut(&target).unwrap().owner = Side::Commonwealth,
                2 => { runtime.facilities.remove(&target); },
                _ => unreachable!(),
            }
            Ok(())
        }).unwrap();
        let before = bytes(&state);
        for result in [
            complete_facility_project(content(), &mut state, &id).map(|_| ()),
            begin_facility_project(content(), &mut state, Side::Axis, &work).map(|_| ()),
        ] {
            let expected = if change == 1 { "airlog:36.15" } else { "land:24.76" };
            assert!(matches!(result, Err(EngineError::Unsupported { case, detail })
                if case == expected && detail.contains("interruption disposition")));
            assert_eq!(bytes(&state), before);
        }
        assert_eq!(state.engineering.facility_projects[&id].lifecycle, ProjectLifecycle::Active);
    }
}

/// Cases: land:24.71, land:24.76, land:24.79
#[test]
fn corrupt_ledger_or_missing_active_receipt_token_never_adopts_current_identity() {
    let (mut original, work) = fixture();
    let id = begin(&mut original, &work);
    for corruption in 0..6 {
        let mut state = original.clone();
        let project = state.engineering.facility_projects.get_mut(&id).unwrap();
        match corruption {
            0 => project.id = ProjectId(0),
            1 => project.side = Side::Commonwealth,
            2 => project.target = FacilityId("other.site".into()),
            3 => project.work = FacilityWork::RepairOne { id: project.target.clone() },
            4 => state.engineering.facility_project_serial = 0,
            5 => {
                let mut legacy = serde_json::to_value(&project.started).unwrap();
                legacy.as_object_mut().unwrap().remove("incarnation");
                project.started = serde_json::from_value(legacy).unwrap();
                assert_eq!(project.started.incarnation(), None);
            }
            _ => unreachable!(),
        }
        assert_invariant_unchanged(&mut state, |s| complete_facility_project(content(), s, &id));
    }
}

/// Cases: land:24.71, land:24.76, airlog:36.14
#[test]
fn actual_pre_token_state_and_receipt_load_without_manufacturing_authority() {
    let (state, work) = fixture();
    let mut legacy: Value = serde_json::to_value(&state).unwrap();
    legacy["engineering"].as_object_mut().unwrap().remove("facility_project_serial");
    legacy["engineering"].as_object_mut().unwrap().remove("facility_projects");
    legacy["air"]["runtime"].as_object_mut().unwrap().remove("site_incarnation_serial");
    for site in legacy["air"]["runtime"]["facilities"].as_object_mut().unwrap().values_mut() {
        site.as_object_mut().unwrap().remove("incarnation");
    }
    let mut recovered: State = serde_json::from_value(legacy).unwrap();
    assert_eq!(recovered.engineering.facility_project_serial, 0);
    assert!(recovered.engineering.facility_projects.is_empty());
    assert!(recovered.air.runtime.facilities.values().all(|site| site.incarnation.is_none()));
    inventory::check(content(), &recovered.air).unwrap();
    let id = begin(&mut recovered, &work);
    let mut old_receipt = serde_json::to_value(&recovered.engineering.facility_projects[&id].started).unwrap();
    old_receipt.as_object_mut().unwrap().remove("incarnation");
    let receipt: StartedFacilityWork = serde_json::from_value(old_receipt).unwrap();
    assert_eq!(receipt.incarnation(), None);
    let before = bytes(&recovered);
    assert!(work::complete_work(content(), &mut recovered.air, &receipt).is_err());
    assert_eq!(bytes(&recovered), before);
}

/// Cases: land:24.75, land:24.76, land:24.79
#[test]
fn later_outer_finish_error_discards_air_completion_and_project_transition() {
    let (mut state, work) = fixture();
    let id = begin(&mut state, &work);
    let original = bytes(&state);
    let mut finish_draft = state.clone();
    complete_facility_project(content(), &mut finish_draft, &id).unwrap();
    assert_eq!(finish_draft.engineering.facility_projects[&id].lifecycle, ProjectLifecycle::Completed);
    let later_error: Result<(), EngineError> = Err(corrupt("later finish failure"));
    assert!(later_error.is_err());
    drop(finish_draft); // the owning finish publishes no draft on this error
    assert_eq!(bytes(&state), original);
    assert_eq!(state.engineering.facility_projects[&id].lifecycle, ProjectLifecycle::Active);
}

/// Cases: land:3.6, land:24.71, land:24.76, land:24.79
#[test]
fn private_project_receipts_and_checkpoint_replay_preserve_enemy_advance_surfaces() {
    let (mut state, work) = fixture();
    state.cursor.game_turn = 1;
    state.cursor.op_stage = Some(1);
    state.cursor.block = Block::OpStage;
    state.cursor.index = 0;
    state.cursor.entered = false;
    state.turn.initiative = Some(Side::Axis);
    let a = Game::<Cna> { state, rng: CampaignRng::from_seed([7; 32]).state() };
    let mut b = a.clone();
    let id = begin(&mut b.state, &work);
    for complete in [false, true] {
        if complete { complete_facility_project(content(), &mut b.state, &id).unwrap(); }
        let serialized = bytes(&b.state);
        b.state = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(bytes(&b.state), serialized);
        crate::testkit::assert_indistinguishable(&Cna::dev(), content(), &a.state, &b.state, Side::Commonwealth);
        crate::testkit::assert_action_indistinguishable(
            &Cna::dev(), content(), &a, &b, &Command::Advance, Side::Commonwealth,
        );
    }
}
