//! Focused proof source for the two actual Air970 findings; uncompiled and unrun.
use super::*;
use crate::{
    Cna,
    seq::{Block, Half, PLAYER_HALF},
};
use cna_core::{dice::CampaignRng, engine::Game};
use serde_json::json;
use std::sync::OnceLock;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn game() -> Game<Cna> {
    let mut state = State::new(content()).unwrap();
    state.cursor.game_turn = 1;
    state.cursor.op_stage = Some(2);
    Game {
        state,
        rng: CampaignRng::from_seed([72; 32]).state(),
    }
}
fn stage(op_stage: u8) -> SgsuOpStage {
    SgsuOpStage {
        game_turn: 1,
        op_stage,
    }
}
fn source_id_owner(game: &Game<Cna>) -> (SgsuId, Side) {
    let (id, squadron) = game
        .state
        .air
        .squadrons
        .iter()
        .find(|(_, squadron)| matches!(squadron.force.as_str(), "axis" | "commonwealth"))
        .unwrap();
    let owner = if squadron.force == "axis" {
        Side::Axis
    } else {
        Side::Commonwealth
    };
    (SgsuId(id.clone()), owner)
}
fn entry(owner: Side) -> SgsuActivityEntry {
    let id = content()
        .areas
        .locations
        .iter()
        .find(|(_, area)| area.off_map)
        .unwrap()
        .0;
    SgsuActivityEntry {
        owner,
        opening_location: Some(Location::OffMap { id: id.clone() }),
        closing_location: None,
        gross_cp_quarters: 0,
        departed: false,
        pinned: false,
    }
}
fn records(id: &SgsuId, entry: SgsuActivityEntry, op_stage: u8, closed: bool) -> SgsuStageRecords {
    SgsuStageRecords {
        stage: stage(op_stage),
        observed_opening: true,
        closed,
        entries: BTreeMap::from([(id.clone(), entry)]),
    }
}
fn query_exact_and_pure(
    game: &Game<Cna>,
    owner: Side,
    id: &SgsuId,
    period: SgsuOpStage,
    expected: Result<SgsuEvidence, EngineError>,
) {
    let before = serde_json::to_vec(&game.state).unwrap();
    let rng = game.rng.clone();
    assert_eq!(
        stage_evidence(content(), &game.state, owner, id, period),
        expected
    );
    assert_eq!(serde_json::to_vec(&game.state).unwrap(), before);
    assert_eq!(game.rng, rng);
}

/// Cases: airlog:35.11, land:24.22, land:24.23
#[test]
fn default_missing_history_and_missing_nested_previous_round_trip_without_opening_credit() {
    let game = game();
    let expected = serde_json::to_vec(&game.state).unwrap();
    let value = serde_json::to_value(&game.state).unwrap();
    assert!(value["air"]["runtime"].get("sgsu_activity").is_none());
    let recovered: State = serde_json::from_value(value.clone()).unwrap();
    assert!(recovered.air.runtime.sgsu_activity.is_default());
    assert_eq!(serde_json::to_vec(&recovered).unwrap(), expected);
    let mut explicit_default = value;
    explicit_default["air"]["runtime"]["sgsu_activity"] = json!({});
    let recovered: State = serde_json::from_value(explicit_default).unwrap();
    assert!(recovered.air.runtime.sgsu_activity.is_default());
    assert_eq!(serde_json::to_vec(&recovered).unwrap(), expected);
    let recovered: SgsuActivityState = serde_json::from_value(json!({"current": null})).unwrap();
    assert_eq!(recovered, SgsuActivityState::default());
    assert_eq!(
        serde_json::to_value(&recovered).unwrap(),
        json!({"current": null})
    );
}

/// Cases: airlog:35.11, land:24.11, land:24.22, land:24.23
#[test]
fn first_true_open_has_only_current_then_first_close_emits_retained_record_and_recovers() {
    let mut game = game();
    // Same closed initial-source fixture as Air inventory's existing import tests.
    game.state.setup.closed = true;
    crate::air::inventory::initialize(content(), &mut game.state).unwrap();
    game.state.cursor.op_stage = Some(1);
    game.state.cursor.block = Block::OpStage;
    game.state.cursor.index = 0;
    game.state.cursor.entered = false;
    let rng = game.rng.clone();
    open_sgsu_stage(content(), &mut game.state, stage(1)).unwrap();
    let current = game
        .state
        .air
        .runtime
        .sgsu_activity
        .current
        .as_ref()
        .unwrap();
    assert!(current.observed_opening);
    assert!(!current.closed);
    let expected_open = json!({"current": current});
    let open = serde_json::to_value(&game.state).unwrap();
    assert_eq!(open["air"]["runtime"]["sgsu_activity"], expected_open);
    assert!(
        open["air"]["runtime"]["sgsu_activity"]
            .get("previous_closed")
            .is_none()
    );
    let encoded = serde_json::to_vec(&game.state).unwrap();
    let recovered: State = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(serde_json::to_vec(&recovered).unwrap(), encoded);
    assert_eq!(
        recovered.air.runtime.sgsu_activity,
        game.state.air.runtime.sgsu_activity
    );
    assert!(
        recovered
            .air
            .runtime
            .sgsu_activity
            .previous_closed
            .is_none()
    );
    assert_eq!(game.rng, rng);
    game.state = recovered;
    game.state.cursor.block = Block::PlayerHalf;
    game.state.cursor.half = Some(Half::B);
    game.state.cursor.index = PLAYER_HALF.len() - 1;
    game.state.cursor.entered = true;
    assert!(game.state.decisions.pending.is_empty());
    capture_sgsu_stage(content(), &mut game.state, stage(1)).unwrap();
    let closed = game
        .state
        .air
        .runtime
        .sgsu_activity
        .previous_closed
        .as_ref()
        .unwrap();
    assert!(closed.closed);
    assert_eq!(closed.stage, stage(1));
    assert_eq!(
        serde_json::to_value(&game.state).unwrap()["air"]["runtime"]["sgsu_activity"],
        json!({"current": closed, "previous_closed": closed})
    );
    let encoded = serde_json::to_vec(&game.state).unwrap();
    let recovered: State = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(serde_json::to_vec(&recovered).unwrap(), encoded);
    assert_eq!(
        recovered.air.runtime.sgsu_activity,
        game.state.air.runtime.sgsu_activity
    );
    assert_eq!(game.rng, rng);
}

/// Cases: airlog:35.11, land:24.22, land:24.23
#[test]
fn selected_unclosed_retained_marker_is_exact_invariant_with_whole_state_rng_purity() {
    let mut game = game();
    let (id, owner) = source_id_owner(&game);
    game.state.air.runtime.sgsu_activity.previous_closed =
        Some(records(&id, entry(owner), 1, false));
    query_exact_and_pure(
        &game,
        owner,
        &id,
        stage(1),
        Err(invalid("retained interval is not closed")),
    );
}

/// Cases: airlog:35.11, land:24.22, land:24.23
#[test]
fn inconsistent_closed_current_marker_is_exact_invariant_for_both_selected_containers() {
    let mut game = game();
    let (id, owner) = source_id_owner(&game);
    game.state.air.runtime.sgsu_activity.current = Some(records(&id, entry(owner), 2, true));
    let error = Err(invalid(
        "closed current interval differs from immutable retained interval",
    ));
    query_exact_and_pure(&game, owner, &id, stage(2), error.clone());
    game.state.air.runtime.sgsu_activity.previous_closed =
        Some(records(&id, entry(owner), 1, true));
    query_exact_and_pure(&game, owner, &id, stage(1), error.clone());
    query_exact_and_pure(&game, owner, &id, stage(2), error);
}

/// Cases: airlog:35.11, airlog:35.12, land:24.22, land:24.23
#[test]
fn valid_older_retained_and_newer_open_current_each_remain_exact_incomplete_unknown() {
    let mut game = game();
    let (id, owner) = source_id_owner(&game);
    let old_entry = entry(owner);
    let mut new_entry = entry(owner);
    new_entry.gross_cp_quarters = 4;
    new_entry.departed = true;
    game.state.air.runtime.sgsu_activity.previous_closed =
        Some(records(&id, old_entry.clone(), 1, true));
    game.state.air.runtime.sgsu_activity.current = Some(records(&id, new_entry.clone(), 2, false));
    for (op_stage, recorded) in [(1, old_entry), (2, new_entry)] {
        query_exact_and_pure(
            &game,
            owner,
            &id,
            stage(op_stage),
            Ok(SgsuEvidence::Unknown {
                reason: SgsuEvidenceGap::IncompleteProducerCoverage,
                recorded: Some(recorded),
            }),
        );
    }
}

/// Cases: airlog:35.11, airlog:35.12, land:24.22, land:24.23
#[test]
fn valid_missing_opening_unpaired_legacy_and_no_record_preserve_exact_unknown_and_purity() {
    let mut game = game();
    let (id, owner) = source_id_owner(&game);
    query_exact_and_pure(
        &game,
        owner,
        &id,
        stage(1),
        Ok(SgsuEvidence::Unknown {
            reason: SgsuEvidenceGap::UnretainedStage,
            recorded: None,
        }),
    );
    game.state.engineering.closed_activity = Some(crate::land::engineering::ClosedActivity {
        stage: ActivityStage {
            game_turn: 1,
            op_stage: 1,
        },
        observed_opening: None,
        entries: BTreeMap::new(),
        required_facility_projects: Default::default(),
    });
    query_exact_and_pure(
        &game,
        owner,
        &id,
        stage(1),
        Ok(SgsuEvidence::Unknown {
            reason: SgsuEvidenceGap::UnretainedStage,
            recorded: None,
        }),
    );
    game.state.engineering.closed_activity = None;
    let mut retained = records(&id, entry(owner), 1, true);
    retained.observed_opening = false;
    retained.entries.get_mut(&id).unwrap().opening_location = None;
    let recorded = retained.entries[&id].clone();
    game.state.air.runtime.sgsu_activity.previous_closed = Some(retained);
    assert!(game.state.engineering.closed_activity.is_none());
    query_exact_and_pure(
        &game,
        owner,
        &id,
        stage(1),
        Ok(SgsuEvidence::Unknown {
            reason: SgsuEvidenceGap::IncompleteProducerCoverage,
            recorded: Some(recorded),
        }),
    );
    let roundtrip: State =
        serde_json::from_value(serde_json::to_value(&game.state).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_vec(&roundtrip).unwrap(),
        serde_json::to_vec(&game.state).unwrap()
    );
}

/// Cases: airlog:35.11, land:24.22, land:24.23
#[test]
fn selected_record_content_failure_retains_exact_error_and_whole_state_rng() {
    let mut game = game();
    let (id, owner) = source_id_owner(&game);
    let mut malformed = entry(owner);
    malformed.opening_location = Some(Location::NotArrived);
    game.state.air.runtime.sgsu_activity.previous_closed = Some(records(&id, malformed, 1, true));
    query_exact_and_pure(
        &game,
        owner,
        &id,
        stage(1),
        Err(invalid("invalid physical historical observation")),
    );
}
