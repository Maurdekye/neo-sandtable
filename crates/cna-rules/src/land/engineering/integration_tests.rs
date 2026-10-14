//! Engine-path and retained-boundary controls. These are authored source, not run receipts.
use super::*;
use crate::{
    Cna, CnaContent,
    air::{
        inventory,
        sgsu::{
            SgsuOpStage,
            activity::{self, SgsuStageRecords},
        },
    },
    seq::{Half, PLAYER_HALF},
    testkit::{assert_action_indistinguishable, assert_indistinguishable},
};
use cna_core::{
    dice::CampaignRng,
    engine::{Command, Game, Rejection, Ruleset, evaluate},
};
use cna_protocol::Side;
use std::sync::OnceLock;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn game(op_stage: u8) -> Game<Cna> {
    let mut state = State::new(content()).unwrap();
    state.setup.closed = true;
    inventory::initialize(content(), &mut state).unwrap();
    state.cursor.game_turn = 1;
    state.cursor.op_stage = Some(op_stage);
    state.cursor.block = Block::PlayerHalf;
    state.cursor.half = Some(Half::B);
    state.cursor.index = PLAYER_HALF.len() - 1;
    state.cursor.entered = true;
    assert!(state.decisions.pending.is_empty());
    Game {
        state,
        rng: CampaignRng::from_seed([78; 32]).state(),
    }
}
fn land_closed(op_stage: u8) -> ClosedActivity {
    ClosedActivity {
        stage: ActivityStage {
            game_turn: 1,
            op_stage,
        },
        observed_opening: None,
        entries: BTreeMap::new(),
        required_facility_projects: BTreeSet::new(),
    }
}
fn air_closed(op_stage: u8) -> SgsuStageRecords {
    SgsuStageRecords {
        stage: SgsuOpStage {
            game_turn: 1,
            op_stage,
        },
        observed_opening: false,
        closed: true,
        entries: BTreeMap::new(),
    }
}
fn bytes(state: &State) -> Vec<u8> {
    serde_json::to_vec(state).unwrap()
}
fn close_pair(state: &mut State) -> Result<(), EngineError> {
    let old = SgsuOpStage {
        game_turn: state.cursor.game_turn,
        op_stage: state.cursor.op_stage.unwrap(),
    };
    activity::capture_sgsu_stage(content(), state, old)?;
    close_stage(state)
}

/// Cases: land:24.11, land:24.12, land:24.22, land:24.23
#[test]
fn genuine_advance_late_engineering_failure_discards_air_capture_state_rng_and_pending() {
    let mut game = game(1);
    game.state.engineering.observed_opening = Some(ActivityStage {
        game_turn: 0,
        op_stage: 1,
    });
    let before = bytes(&game.state);
    let rng = game.rng.clone();
    let expected = Rejection::Engine(EngineError::Invariant {
        detail: "engineering closure has invalid opening history".into(),
    });
    let result = evaluate(&Cna::dev(), content(), &game, &Command::Advance);
    match result {
        Err(error) => assert_eq!(error, expected),
        Ok(_) => panic!("corrupt late Engineering closure unexpectedly committed"),
    }
    assert_eq!(bytes(&game.state), before);
    assert_eq!(game.rng, rng);
    assert!(
        game.state
            .air
            .runtime
            .sgsu_activity
            .previous_closed
            .is_none()
    );
    assert!(game.state.engineering.closed_activity.is_none());
}

/// Cases: land:24.11, land:24.22, land:24.23, airlog:35.11
#[test]
fn actual_advance_stage_three_retains_final_interval_and_recovery_matches_complete_tail() {
    let game = game(3);
    let first = evaluate(&Cna::dev(), content(), &game, &Command::Advance).unwrap();
    let land = first
        .game
        .state
        .engineering
        .closed_activity
        .as_ref()
        .unwrap();
    let air = first
        .game
        .state
        .air
        .runtime
        .sgsu_activity
        .previous_closed
        .as_ref()
        .unwrap();
    assert_eq!(
        land.stage,
        ActivityStage {
            game_turn: 1,
            op_stage: 3
        }
    );
    assert_eq!(
        air.stage,
        SgsuOpStage {
            game_turn: 1,
            op_stage: 3
        }
    );
    assert!(air.closed);
    // A Post block has no invented new opening. No next real boundary is inferred.
    if first.game.state.cursor.block == Block::Post {
        assert_eq!(first.game.state.cursor.op_stage, None);
        assert_eq!(
            first.game.state.air.runtime.sgsu_activity.current.as_ref(),
            Some(air)
        );
    }
    let encoded = bytes(&first.game.state);
    let recovered = Game {
        state: serde_json::from_slice::<State>(&encoded).unwrap(),
        rng: first.game.rng.clone(),
    };
    assert_eq!(bytes(&recovered.state), encoded);
    let uninterrupted = evaluate(&Cna::dev(), content(), &first.game, &Command::Advance);
    let restored = evaluate(&Cna::dev(), content(), &recovered, &Command::Advance);
    match (uninterrupted, restored) {
        (Ok(a), Ok(b)) => {
            assert_eq!(bytes(&a.game.state), bytes(&b.game.state));
            assert_eq!(a.game.rng, b.game.rng);
            assert_eq!(
                serde_json::to_vec(&a.events).unwrap(),
                serde_json::to_vec(&b.events).unwrap()
            );
            assert_eq!(
                serde_json::to_vec(&Cna::dev().pending(content(), &a.game.state)).unwrap(),
                serde_json::to_vec(&Cna::dev().pending(content(), &b.game.state)).unwrap()
            );
        }
        (Err(a), Err(b)) => assert_eq!(a, b),
        _ => panic!("serialized recovery changed accepted/rejected tail"),
    }
}

/// Cases: land:3.6, land:24.12, land:24.22, land:24.23, land:26.13
#[test]
fn private_activity_differences_keep_all_seats_and_ordered_genuine_advance_identical() {
    let a = game(1);
    let mut b = game(1);
    let (id, unit) = a
        .state
        .land
        .units
        .iter()
        .find(|(_, unit)| unit.location.hex().is_some())
        .unwrap();
    b.state.engineering.activity.insert(
        id.clone(),
        UnitActivity {
            stage: ActivityStage {
                game_turn: 1,
                op_stage: 1,
            },
            opening_location: Some(unit.location.clone()),
            gross_cp_quarters: 4,
            departed: true,
            pinned: true,
        },
    );
    let rules = Cna::dev();
    for side in [Side::Axis, Side::Commonwealth] {
        assert_indistinguishable(&rules, content(), &a.state, &b.state, side);
        assert_action_indistinguishable(&rules, content(), &a, &b, &Command::Advance, side);
    }
    let left = evaluate(&rules, content(), &a, &Command::Advance).unwrap();
    let right = evaluate(&rules, content(), &b, &Command::Advance).unwrap();
    for side in [Side::Axis, Side::Commonwealth] {
        assert_indistinguishable(&rules, content(), &left.game.state, &right.game.state, side);
    }
    assert_eq!(left.game.rng, right.game.rng);
    use cna_core::visibility::Perspective;
    assert_eq!(
        serde_json::to_vec(&rules.view(content(), &left.game.state, Perspective::Operator))
            .unwrap(),
        serde_json::to_vec(&rules.view(content(), &right.game.state, Perspective::Operator))
            .unwrap()
    );
    assert_eq!(
        rules.observe(content(), &left.game.state, Perspective::Operator),
        rules.observe(content(), &right.game.state, Perspective::Operator)
    );
}

/// Cases: scen:60.22, land:24.11, land:24.22, land:24.23
#[test]
fn genuine_final_campaign_tail_captures_before_end_without_opening_a_fabricated_stage() {
    let mut game = game(content().bounds.end_opstage);
    game.state.cursor.game_turn = content().bounds.end_gt;
    let transition = evaluate(&Cna::dev(), content(), &game, &Command::Advance).unwrap();
    let expected = ActivityStage {
        game_turn: content().bounds.end_gt,
        op_stage: content().bounds.end_opstage,
    };
    assert_eq!(
        transition
            .game
            .state
            .engineering
            .closed_activity
            .as_ref()
            .unwrap()
            .stage,
        expected
    );
    let air = transition
        .game
        .state
        .air
        .runtime
        .sgsu_activity
        .previous_closed
        .as_ref()
        .unwrap();
    assert_eq!(
        (air.stage.game_turn, air.stage.op_stage),
        (expected.game_turn, expected.op_stage)
    );
    assert_eq!(
        transition
            .game
            .state
            .air
            .runtime
            .sgsu_activity
            .current
            .as_ref(),
        Some(air)
    );
    assert!(transition.game.state.cursor.is_finished());
    assert_eq!(transition.game.state.cursor.op_stage, None);
}

/// Cases: land:24.11, land:24.22, land:24.23
#[test]
fn legacy_empty_and_land_only_can_retire_but_air_only_and_mismatched_labels_stop_exactly() {
    for land_only in [false, true] {
        let mut game = game(2);
        if land_only {
            game.state.engineering.closed_activity = Some(land_closed(1));
        }
        close_pair(&mut game.state).unwrap();
        assert_eq!(
            game.state
                .engineering
                .closed_activity
                .as_ref()
                .unwrap()
                .stage,
            ActivityStage {
                game_turn: 1,
                op_stage: 2
            }
        );
        assert_eq!(
            game.state
                .air
                .runtime
                .sgsu_activity
                .previous_closed
                .as_ref()
                .unwrap()
                .stage,
            SgsuOpStage {
                game_turn: 1,
                op_stage: 2
            }
        );
    }
    for mismatched in [false, true] {
        let mut game = game(3);
        game.state.air.runtime.sgsu_activity.previous_closed = Some(air_closed(2));
        if mismatched {
            game.state.engineering.closed_activity = Some(land_closed(1));
        }
        let before = bytes(&game.state);
        let rng = game.rng.clone();
        let detail = if mismatched {
            "legacy retained intervals have unresolved pairing"
        } else {
            "retained Air interval has no authenticated Engineering dependency record"
        };
        assert_eq!(
            close_pair(&mut game.state),
            Err(EngineError::Unsupported {
                case: "land:24.11".into(),
                detail: detail.into()
            })
        );
        assert_eq!(bytes(&game.state), before);
        assert_eq!(game.rng, rng);
    }
}

/// Cases: land:24.11, land:24.71, land:24.76, land:24.79
#[test]
fn exact_duplicate_with_required_project_is_noop_and_completed_history_still_blocks_eviction() {
    use crate::air::facilities::{FacilityKind, FacilityWork};
    let mut game = game(1);
    let occupied: Vec<_> = game
        .state
        .air
        .runtime
        .facilities
        .iter()
        .map(|(id, site)| site.properties(content(), id).unwrap().location)
        .collect();
    let hex = content()
        .map
        .iter()
        .find(|hex| {
            !occupied.contains(&Location::Hex {
                hex: hex.id.clone(),
            })
        })
        .unwrap()
        .id
        .clone();
    // Existing trusted helper invoked by a test only, never a production creator.
    let id = begin_facility_project(
        content(),
        &mut game.state,
        Side::Axis,
        &FacilityWork::Create {
            kind: FacilityKind::LandingStrip,
            hex,
        },
    )
    .unwrap();
    close_pair(&mut game.state).unwrap();
    assert!(
        game.state
            .engineering
            .closed_activity
            .as_ref()
            .unwrap()
            .required_facility_projects
            .contains(&id)
    );
    let before = bytes(&game.state);
    close_pair(&mut game.state).unwrap();
    assert_eq!(bytes(&game.state), before);
    complete_facility_project(content(), &mut game.state, &id).unwrap();
    game.state.cursor.op_stage = Some(2);
    game.state.cursor.block = Block::OpStage;
    game.state.cursor.index = 0;
    game.state.cursor.entered = false;
    open_stage(&mut game.state);
    activity::open_sgsu_stage(
        content(),
        &mut game.state,
        SgsuOpStage {
            game_turn: 1,
            op_stage: 2,
        },
    )
    .unwrap();
    game.state.cursor.block = Block::PlayerHalf;
    game.state.cursor.half = Some(Half::B);
    game.state.cursor.index = PLAYER_HALF.len() - 1;
    game.state.cursor.entered = true;
    let before = bytes(&game.state);
    assert_eq!(
        close_pair(&mut game.state),
        Err(EngineError::Unsupported {
            case: "land:24.11".into(),
            detail: "required project progress has no persisted interval consumer".into()
        })
    );
    assert_eq!(bytes(&game.state), before);
}

/// Cases: land:24.11, land:24.12, land:24.22, land:24.23
#[test]
fn malformed_air_retained_marker_stops_actual_advance_without_capturing_either_history() {
    let mut game = game(2);
    let mut malformed = air_closed(1);
    malformed.closed = false;
    game.state.air.runtime.sgsu_activity.previous_closed = Some(malformed);
    let before = bytes(&game.state);
    let rng = game.rng.clone();
    let result = evaluate(&Cna::dev(), content(), &game, &Command::Advance);
    match result {
        Err(error) => assert_eq!(
            error,
            Rejection::Engine(EngineError::Invariant {
                detail: "air SGSU activity: retained interval is not closed".into()
            })
        ),
        Ok(_) => panic!("malformed retained Air marker unexpectedly advanced"),
    }
    assert_eq!(bytes(&game.state), before);
    assert_eq!(game.rng, rng);
    assert!(game.state.engineering.closed_activity.is_none());
}
