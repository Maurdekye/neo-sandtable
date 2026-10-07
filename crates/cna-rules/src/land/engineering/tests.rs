use super::*;
use crate::{Cna, CnaContent, seq::Half};
use cna_core::{
    dice::CampaignRng,
    engine::{Command, Game, evaluate},
};
use cna_protocol::Side;
use serde_json::Value;
use std::sync::OnceLock;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn fixture() -> (State, UnitId, UnitId) {
    let mut state = State::new(content()).unwrap();
    state.cursor.game_turn = 1;
    state.cursor.op_stage = Some(1);
    state.cursor.block = Block::OpStage;
    state.cursor.index = 0;
    state.cursor.entered = false;
    let ids: Vec<_> = state
        .land
        .units
        .values()
        .filter(|u| u.location.hex().is_some())
        .take(2)
        .map(|u| u.id.clone())
        .collect();
    (state, ids[0].clone(), ids[1].clone())
}
fn bytes(state: &State) -> Value {
    serde_json::to_value(state).unwrap()
}
fn assert_cp_failure(state: &mut State, id: &UnitId, quarters: i32, expected: &str) {
    let before = bytes(state);
    match record_cp(state, id, quarters) {
        Err(EngineError::Invariant { detail }) => assert_eq!(detail, expected),
        other => panic!("unexpected CP result: {other:?}"),
    }
    assert_eq!(bytes(state), before);
}

/// Cases: land:23.22, land:26.13
#[test]
fn constructor_and_legacy_checkpoint_do_not_invent_an_opening() {
    let (state, _, _) = fixture();
    assert_eq!(state.engineering, EngineeringState::default());
    let mut legacy = bytes(&state);
    legacy.as_object_mut().unwrap().remove("engineering");
    let recovered: State = serde_json::from_value(legacy).unwrap();
    assert_eq!(recovered.engineering, EngineeringState::default());
}

/// Cases: land:23.22, land:24.38, land:26.13
/// Interpretations: interp:land-0033
#[test]
fn real_opening_is_idempotent_and_gross_activity_survives_refunds_and_flags() {
    let (mut state, id, _) = fixture();
    let location = state.land.units[&id].location.clone();
    open_stage(&mut state);
    assert!(whole_stage_idle_at(&state, &id, &location));
    record_cp(&mut state, &id, 4).unwrap();
    state.land.units.get_mut(&id).unwrap().cp_spent_quarters = 0;
    state.engineering.activity.get_mut(&id).unwrap().departed = true;
    state.engineering.activity.get_mut(&id).unwrap().pinned = true;
    let before = bytes(&state);
    open_stage(&mut state);
    assert_eq!(bytes(&state), before);
    assert_eq!(state.engineering.activity[&id].gross_cp_quarters, 4);
    assert!(!whole_stage_idle_at(&state, &id, &location));
}

/// Cases: land:23.22, land:26.13
#[test]
fn late_arrival_at_an_observed_stage_never_acquires_opening_history() {
    let (mut state, id, _) = fixture();
    open_stage(&mut state);
    state.engineering.activity.remove(&id);
    record_cp(&mut state, &id, 1).unwrap();
    assert_eq!(state.engineering.activity[&id].opening_location, None);
    assert!(!whole_stage_idle_at(
        &state,
        &id,
        &state.land.units[&id].location
    ));
}

/// Cases: land:23.22, land:26.13
#[test]
fn midstage_recovery_and_partial_records_cannot_manufacture_observed_opening() {
    let (mut state, id, _) = fixture();
    open_stage(&mut state);
    state.cursor.block = Block::PlayerHalf;
    state.cursor.index = 1;
    for opening in [
        None,
        Some(ActivityStage {
            game_turn: 0,
            op_stage: 3,
        }),
    ] {
        state.engineering.observed_opening = opening;
        let before = bytes(&state);
        open_stage(&mut state);
        assert_eq!(bytes(&state), before);
        assert!(!whole_stage_idle_at(
            &state,
            &id,
            &state.land.units[&id].location
        ));
    }
    state.cursor.block = Block::OpStage;
    state.cursor.index = 0;
    state.cursor.entered = true;
    let before = bytes(&state);
    open_stage(&mut state);
    assert_eq!(bytes(&state), before);
}

/// Cases: land:23.22, land:26.13
#[test]
fn negative_zero_missing_unit_and_out_of_stage_callbacks_have_exact_purity() {
    let (mut state, id, _) = fixture();
    assert_cp_failure(
        &mut state,
        &id,
        -1,
        "engineering activity received negative CP",
    );
    let missing: UnitId = "missing.trusted.unit".into();
    assert_cp_failure(
        &mut state,
        &missing,
        1,
        "engineering activity: missing trusted unit missing.trusted.unit",
    );
    state.cursor.op_stage = None;
    assert_cp_failure(
        &mut state,
        &id,
        1,
        "engineering activity outside an Operations Stage",
    );
    let before = bytes(&state);
    record_cp(&mut state, &missing, 0).unwrap();
    assert_eq!(bytes(&state), before);
}

/// Cases: land:23.22, land:26.13
#[test]
fn overflow_and_malformed_current_or_stale_history_never_normalize_or_commit() {
    let (mut state, id, _) = fixture();
    open_stage(&mut state);
    state
        .engineering
        .activity
        .get_mut(&id)
        .unwrap()
        .gross_cp_quarters = i32::MAX;
    assert_cp_failure(&mut state, &id, 1, "engineering gross CP overflow");
    for stage in [
        ActivityStage {
            game_turn: 1,
            op_stage: 1,
        },
        ActivityStage {
            game_turn: 0,
            op_stage: 3,
        },
    ] {
        let entry = state.engineering.activity.get_mut(&id).unwrap();
        entry.stage = stage;
        entry.gross_cp_quarters = -1;
        assert_cp_failure(
            &mut state,
            &id,
            1,
            "engineering activity has corrupt selected history",
        );
    }
    let entry = state.engineering.activity.get_mut(&id).unwrap();
    entry.gross_cp_quarters = 0;
    entry.stage = ActivityStage {
        game_turn: 1,
        op_stage: 2,
    };
    assert_cp_failure(
        &mut state,
        &id,
        1,
        "engineering activity contains future selected history",
    );
    state
        .engineering
        .activity
        .get_mut(&id)
        .unwrap()
        .stage
        .op_stage = 0;
    assert_cp_failure(
        &mut state,
        &id,
        1,
        "engineering activity has corrupt selected history",
    );
}

/// Cases: land:23.22, land:26.13
#[test]
fn valid_stale_record_becomes_current_unknown_without_global_promotion() {
    let (mut state, id, _) = fixture();
    open_stage(&mut state);
    let old = ActivityStage {
        game_turn: 0,
        op_stage: 3,
    };
    state.engineering.observed_opening = Some(old);
    state.engineering.activity.get_mut(&id).unwrap().stage = old;
    record_cp(&mut state, &id, 2).unwrap();
    assert_eq!(state.engineering.observed_opening, Some(old));
    let entry = &state.engineering.activity[&id];
    assert_eq!(
        entry.stage,
        ActivityStage {
            game_turn: 1,
            op_stage: 1
        }
    );
    assert_eq!(entry.opening_location, None);
    assert_eq!(entry.gross_cp_quarters, 2);
}

/// Cases: land:23.22, land:26.13
#[test]
fn selected_absent_current_and_stale_snapshots_restore_without_touching_unmatched() {
    let (mut state, id, other) = fixture();
    open_stage(&mut state);
    let unmatched = state.engineering.activity[&other].clone();
    for original in [
        None,
        state.engineering.activity.get(&id).cloned(),
        Some(UnitActivity {
            stage: ActivityStage {
                game_turn: 0,
                op_stage: 3,
            },
            opening_location: None,
            gross_cp_quarters: 9,
            departed: true,
            pinned: true,
        }),
    ] {
        match &original {
            Some(entry) => {
                state.engineering.activity.insert(id.clone(), entry.clone());
            }
            None => {
                state.engineering.activity.remove(&id);
            }
        }
        let snapshot = activity_snapshot(&state, &[id.clone(), id.clone()]);
        record_cp(&mut state, &id, 1).unwrap();
        restore_activity(&mut state, &snapshot).unwrap();
        assert_eq!(state.engineering.activity.get(&id).cloned(), original);
        assert_eq!(state.engineering.activity[&other], unmatched);
        assert_eq!(
            state.engineering.observed_opening,
            Some(ActivityStage {
                game_turn: 1,
                op_stage: 1
            })
        );
    }
}

/// Cases: land:23.22, land:26.13
#[test]
fn snapshot_guard_ignores_half_cycle_step_but_preserves_gt_and_opening_context() {
    let (mut state, id, _) = fixture();
    open_stage(&mut state);
    let snapshot = activity_snapshot(&state, std::slice::from_ref(&id));
    state.cursor.half = Some(Half::B);
    state.cursor.cycle = 7;
    state.cursor.index = 4;
    restore_activity(&mut state, &snapshot).unwrap();
    for mismatch in [0, 1, 2] {
        let mut altered = state.clone();
        match mismatch {
            0 => {
                altered.cursor.game_turn += 1;
            }
            1 => {
                altered.cursor.op_stage = Some(2);
            }
            _ => {
                altered.engineering.observed_opening = None;
            }
        }
        let before = bytes(&altered);
        assert!(matches!(
            restore_activity(&mut altered, &snapshot),
            Err(EngineError::Invariant { .. })
        ));
        assert_eq!(bytes(&altered), before);
    }
    state.cursor.op_stage = None;
    let snapshot = activity_snapshot(&state, std::slice::from_ref(&id));
    state.cursor.game_turn += 1;
    let before = bytes(&state);
    assert!(matches!(
        restore_activity(&mut state, &snapshot),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(bytes(&state), before);
}

/// Cases: land:23.22, land:26.13
#[test]
fn post_end_opening_noop_and_checkpoint_recovery_preserve_history() {
    let (mut state, id, _) = fixture();
    open_stage(&mut state);
    record_cp(&mut state, &id, 3).unwrap();
    for block in [Block::Post, Block::End] {
        state.cursor.block = block;
        state.cursor.op_stage = None;
        let before = bytes(&state);
        open_stage(&mut state);
        assert_eq!(bytes(&state), before);
    }
    let recovered: State = serde_json::from_value(bytes(&state)).unwrap();
    assert_eq!(recovered.engineering, state.engineering);
}

/// Cases: land:23.22, land:26.13, land:7.11
#[test]
fn dispatcher_opens_first_stage_and_next_stage_after_reset() {
    let (mut state, id, _) = fixture();
    state.turn.initiative = Some(Side::Axis);
    let game = Game::<Cna> {
        state,
        rng: CampaignRng::from_seed([5; 32]).state(),
    };
    let first = evaluate(&Cna::dev(), content(), &game, &Command::Advance).unwrap();
    assert_eq!(
        first.game.state.engineering.observed_opening,
        Some(ActivityStage {
            game_turn: 1,
            op_stage: 1
        })
    );
    let mut state = first.game.state;
    state.decisions.pending.clear();
    state.cursor.block = Block::PlayerHalf;
    state.cursor.op_stage = Some(1);
    state.cursor.half = Some(Half::B);
    state.cursor.index = crate::seq::PLAYER_HALF.len() - 1;
    state.cursor.entered = true;
    state.land.units.get_mut(&id).unwrap().cp_spent_quarters = 4;
    record_cp(&mut state, &id, 4).unwrap();
    let game = Game::<Cna> {
        state,
        rng: first.game.rng,
    };
    let next = evaluate(&Cna::dev(), content(), &game, &Command::Advance).unwrap();
    assert_eq!(next.game.state.land.units[&id].cp_spent_quarters, 0);
    assert_eq!(
        next.game.state.engineering.observed_opening,
        Some(ActivityStage {
            game_turn: 1,
            op_stage: 2
        })
    );
    assert_eq!(
        next.game.state.engineering.activity[&id].gross_cp_quarters,
        0
    );
}
/// Cases: land:3.6, land:23.22, land:26.13
#[test]
fn private_activity_does_not_change_enemy_actions_views_or_checkpoint_visibility() {
    let (mut state, id, _) = fixture();
    state.turn.initiative = Some(Side::Axis);
    open_stage(&mut state);
    let enemy = state.land.units[&id].side.opponent();
    let a = Game::<Cna> {
        state,
        rng: CampaignRng::from_seed([6; 32]).state(),
    };
    let mut b = a.clone();
    record_cp(&mut b.state, &id, 3).unwrap();
    b.state.engineering.activity.get_mut(&id).unwrap().pinned = true;
    let recovered: State = serde_json::from_value(bytes(&b.state)).unwrap();
    b.state = recovered;
    crate::testkit::assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &a,
        &b,
        &Command::Advance,
        enemy,
    );
}

/// Cases: land:23.22, land:26.13
#[test]
fn observed_idle_history_requires_current_presence_at_the_site() {
    let (mut state, id, _) = fixture();
    open_stage(&mut state);
    let site = state.land.units[&id].location.clone();
    state.land.units.get_mut(&id).unwrap().location = Location::Eliminated;
    assert!(!whole_stage_idle_at(&state, &id, &site));
}

/// Cases: land:23.22, land:26.13
#[test]
fn corrupt_current_stage_and_future_global_opening_fail_without_mutation() {
    let (mut state, id, _) = fixture();
    state.cursor.op_stage = Some(0);
    assert_cp_failure(
        &mut state,
        &id,
        1,
        "engineering activity has an invalid current stage",
    );
    state.cursor.op_stage = Some(1);
    state.engineering.observed_opening = Some(ActivityStage {
        game_turn: 1,
        op_stage: 2,
    });
    assert_cp_failure(
        &mut state,
        &id,
        1,
        "engineering activity has corrupt opening history",
    );
}
