//! TEST-LOCAL SOURCE PROPOSAL ONLY: real admission and per-cell negative routes.
//! No production API/hook, inventory transplant, invented identity or coverage admission.
use super::*;
use crate::state::Pending;
use cna_core::decision::DecisionResponse;
use cna_core::dice::CampaignRng;
use std::sync::OnceLock;

// COMPLETE unchanged private recipes from setup/decisions_tests.rs:
// opened13-21, submit22-43, first44-57, before_fresh_close77-99.
fn opened(c: &CnaContent) -> Game<Cna> {
    let game = Game {
        state: State::new(c).unwrap(),
        rng: CampaignRng::from_seed([5; 32]).state(),
    };
    evaluate(&Cna::dev(), c, &game, &Command::Advance)
        .unwrap()
        .game
}

fn submit(
    c: &CnaContent,
    game: &Game<Cna>,
    p: &Pending,
    action: Value,
) -> Result<Game<Cna>, Rejection> {
    evaluate(
        &Cna::dev(),
        c,
        game,
        &Command::Respond(DecisionResponse {
            decision_id: p.id.clone(),
            seat: p.seat,
            controller_epoch: 1,
            decision_revision: p.revision,
            idempotency_key: p.id.to_string(),
            action,
            public_explanation: None,
        }),
    )
    .map(|t| t.game)
}

fn first(schema: &ActionSchema) -> Value {
    match schema {
        ActionSchema::Choice { options } => json!(options[0].id),
        ActionSchema::Unit { among } => json!(among[0]),
        ActionSchema::Integer { max, .. } => json!(max),
        ActionSchema::Record { fields } => Value::Object(
            fields
                .iter()
                .map(|f| (f.name.clone(), first(&f.schema)))
                .collect(),
        ),
        _ => panic!("unexpected setup schema"),
    }
}

fn before_fresh_close(c: &CnaContent) -> Game<Cna> {
    let mut game = opened(c);
    for _ in 0..500 {
        assert!(!game.state.setup.closed);
        if let Some(p) = game.state.decisions.pending.first().cloned() {
            let air = serde_json::to_value(&game.state.air.runtime).unwrap();
            game = submit(c, &game, &p, first(&p.space.schema)).unwrap();
            // Respond may arrange setup aggregates; canonical admission is finish-only.
            assert_eq!(serde_json::to_value(&game.state.air.runtime).unwrap(), air);
        } else if game.state.setup.air_started
            && game.state.setup.pools_started
            && game.state.setup.preload_started
        {
            assert!(game.state.setup.tasks.is_empty());
            return game;
        } else {
            game = evaluate(&Cna::dev(), c, &game, &Command::Advance)
                .unwrap()
                .game;
        }
    }
    panic!("actual source setup did not reach final close");
}

/// Actual setup-close transition, not a completion flag or inventory injection.
/// Cases: scen:59.2, scen:59.32, scen:59.36, airlog:34.0, airlog:35.24
fn admitted_before_first_open() -> Game<Cna> {
    static ADMITTED: OnceLock<Game<Cna>> = OnceLock::new();
    ADMITTED
        .get_or_init(|| {
            let c = content();
            let pending_close = before_fresh_close(c);
            assert!(!pending_close.state.setup.closed);
            assert!(pending_close.state.decisions.pending.is_empty());
            assert!(matches!(
                pending_close.state.setup.initial_air_import,
                crate::setup::InitialAirImport::PendingOriginal { .. }
            ));
            let before = whole(&pending_close);
            let saved = checkpoint(&pending_close);
            let closed = evaluate(&Cna::dev(), c, &pending_close, &Command::Advance).unwrap();
            for original in [&saved, &pending_close] {
                let replay = evaluate(&Cna::dev(), c, original, &Command::Advance).unwrap();
                assert_eq!(whole(&closed.game), whole(&replay.game));
                assert_eq!(closed.events, replay.events);
                assert_eq!(closed.progress, replay.progress);
            }
            assert_eq!(whole(&pending_close), before);
            assert!(closed.game.state.setup.closed);
            assert!(matches!(
                closed.game.state.setup.initial_air_import,
                crate::setup::InitialAirImport::Imported
            ));
            crate::air::inventory::check(c, &closed.game.state.air).unwrap();
            assert!(!closed.game.state.air.runtime.sgsus.is_empty());
            let source = State::new(c).unwrap();
            for (force, pool) in &source.air.forces {
                if force == "malta" {
                    continue;
                }
                let placed = closed
                    .game
                    .state
                    .air
                    .squadrons
                    .values()
                    .filter(|squadron| &squadron.force == force)
                    .count();
                assert_eq!(
                    i32::try_from(placed).unwrap()
                        + closed.game.state.air.forces[force].sgsu_available,
                    pool.sgsu_available
                );
            }
            for (id, unit) in &closed.game.state.air.runtime.sgsus {
                let squadron = &closed.game.state.air.squadrons[&id.0];
                assert_ne!(squadron.force, "malta");
                assert_eq!(unit.side().unwrap(), squadron.side);
                assert_eq!(
                    unit.location(c, &closed.game.state.air.runtime.facilities)
                        .unwrap(),
                    closed.game.state.air.runtime.facilities
                        [&crate::air::facilities::FacilityId(squadron.facility.clone())]
                        .properties(
                            c,
                            &crate::air::facilities::FacilityId(squadron.facility.clone())
                        )
                        .unwrap()
                        .location
                );
            }
            // Public PRE decisions stop this transition before the FIRST OpStage opening.
            assert_eq!(closed.game.state.cursor.block, Block::Pre);
            assert!(closed.game.state.engineering.observed_opening.is_none());
            assert!(
                closed
                    .game
                    .state
                    .air
                    .runtime
                    .sgsu_activity
                    .current
                    .is_none()
            );
            assert!(
                closed
                    .game
                    .state
                    .air
                    .runtime
                    .sgsu_activity
                    .previous_closed
                    .is_none()
            );
            closed.game
        })
        .clone()
}

/// POSITIVE lane: public commands only, ending after the last real PRE answer.
/// The next Advance finishes stores and crosses the FIRST true opening.
pub(super) fn before_first_true_open() -> Game<Cna> {
    static PRE_OPEN: OnceLock<Game<Cna>> = OnceLock::new();
    PRE_OPEN
        .get_or_init(|| {
            let mut value = admitted_before_first_open();
            for serial in 0..4096 {
                assert!(value.state.engineering.observed_opening.is_none());
                assert!(value.state.air.runtime.sgsu_activity.current.is_none());
                assert!(
                    value
                        .state
                        .air
                        .runtime
                        .sgsu_activity
                        .previous_closed
                        .is_none()
                );
                assert_eq!(value.state.cursor.game_turn, content().bounds.start_gt);
                if value.state.cursor.block == Block::Pre
                    && value.state.cursor.anchor() == "logistics.stores_expenditure"
                    && value.state.cursor.entered
                    && value.state.decisions.pending.is_empty()
                {
                    assert!(!value.state.air.runtime.sgsus.is_empty());
                    return value;
                }
                let command = next_command(&value, serial);
                value = accepted_with_checkpoint_retry(&value, &command).game;
            }
            panic!("lawful source flow did not reach the pre-first-open stores checkpoint");
        })
        .clone()
}

/// G1/G2/G3: a genuine public Respond return, before the final empty convoy closes.
/// This lane changes no clock, pending list, inventory or history field.
/// Cases: land:24.11, land:24.12, airlog:53.12, airlog:53.21
pub(super) fn genuine_before_old_tail(op_stage: u8) -> Game<Cna> {
    static FIRST: OnceLock<Game<Cna>> = OnceLock::new();
    static SECOND: OnceLock<Game<Cna>> = OnceLock::new();
    static THIRD: OnceLock<Game<Cna>> = OnceLock::new();
    let cache = match op_stage {
        1 => &FIRST,
        2 => &SECOND,
        3 => &THIRD,
        _ => panic!("only the existing three negative components are selected"),
    };
    cache
        .get_or_init(|| {
            let mut value = before_first_true_open();
            for serial in 0..4096 {
                assert_eq!(value.state.cursor.game_turn, content().bounds.start_gt);
                if value.state.cursor.block == Block::PlayerHalf
                    && value.state.cursor.half == Some(Half::B)
                    && value.state.cursor.op_stage == Some(op_stage)
                    && value.state.cursor.anchor() == crate::logistics::truck_convoy::ANCHOR
                    && value.state.cursor.entered
                    && value.state.decisions.pending.is_empty()
                    && !value.state.logistics.truck_convoy.resolved
                    && value
                        .state
                        .logistics
                        .truck_convoy
                        .submitted
                        .as_ref()
                        .is_some_and(|orders| orders.is_empty())
                {
                    // answer437-465 records []; core engine246-248 returns Respond
                    // without advancing. The saved procedure scope is still authentic.
                    let key = value.state.logistics.truck_convoy.key.as_ref().unwrap();
                    assert_eq!(key.game_turn, value.state.cursor.game_turn);
                    assert_eq!(key.op_stage, op_stage);
                    assert_eq!(key.half, Half::B);
                    assert_eq!(key.cycle, value.state.cursor.cycle);
                    assert_eq!(
                        Some(key.side),
                        value.state.cursor.phasing(value.state.turn.player_a)
                    );
                    assert_eq!(value.state.logistics.truck_convoy.next_order, 0);
                    assert!(value.state.logistics.truck_convoy.waiting_pool.is_none());
                    assert!(!value.state.air.runtime.sgsus.is_empty());
                    let stage = ActivityStage {
                        game_turn: value.state.cursor.game_turn,
                        op_stage,
                    };
                    assert_eq!(value.state.engineering.observed_opening, Some(stage));
                    let air = value
                        .state
                        .air
                        .runtime
                        .sgsu_activity
                        .current
                        .as_ref()
                        .unwrap();
                    assert_eq!(air.stage.game_turn, stage.game_turn);
                    assert_eq!(air.stage.op_stage, stage.op_stage);
                    assert!(air.observed_opening);
                    assert!(!air.closed);
                    if op_stage == 1 {
                        assert!(
                            value
                                .state
                                .air
                                .runtime
                                .sgsu_activity
                                .previous_closed
                                .is_none()
                        );
                        assert!(value.state.engineering.closed_activity.is_none());
                    } else {
                        let prior = value
                            .state
                            .air
                            .runtime
                            .sgsu_activity
                            .previous_closed
                            .as_ref()
                            .unwrap();
                        let land = value.state.engineering.closed_activity.as_ref().unwrap();
                        assert_eq!(prior.stage.game_turn, stage.game_turn);
                        assert_eq!(prior.stage.op_stage, op_stage - 1);
                        assert!(prior.closed);
                        assert_eq!(land.stage.game_turn, prior.stage.game_turn);
                        assert_eq!(land.stage.op_stage, prior.stage.op_stage);
                    }
                    return value;
                }
                let command = next_command(&value, serial);
                value = accepted_with_checkpoint_retry(&value, &command).game;
            }
            panic!("lawful source flow did not return the selected empty convoy checkpoint");
        })
        .clone()
}

/// NEGATIVE-ONLY controlled component clock; NEVER a publicly returned old-tail Game.
/// Used ONLY for the six cells explicitly approved by the lead per-cell rulings.
/// Clock-label intercepts, duplicate-close setup, and first-closure prior-label cell.
/// Cases: land:24.11, land:24.12, airlog:35.11
pub(super) fn negative_component_old_tail(op_stage: u8) -> Game<Cna> {
    assert_eq!(op_stage, 1, "S1 is limited to the FIRST closure");
    let mut value = before_first_true_open();
    assert!(value.state.decisions.pending.is_empty());
    let original = serde_json::to_value(&value).unwrap();
    assert!(value.state.cursor.entered);
    let mut cursor = value.state.cursor.clone();
    let mut reached = false;
    // Cursor::advance/next_raw is the actual production sequence graph; no
    // hand-authored GT, half, index, cycle or stage label is inserted here.
    for _ in 0..4096 {
        if cursor.game_turn == content().bounds.start_gt
            && cursor.block == Block::PlayerHalf
            && cursor.half == Some(Half::B)
            && cursor.op_stage == Some(op_stage)
            && cursor.anchor() == "opstage.patrol"
        {
            reached = true;
            break;
        }
        assert!(!cursor.is_finished());
        cursor.advance(&content().bounds);
    }
    assert!(
        reached,
        "source sequence must contain this GT1 old-tail clock"
    );
    // Production run_steps sets entered=true at lib.rs193 BEFORE dispatch197,
    // including patrol. This reproduces that source-produced component clock;
    // it is NOT a publicly returned patrol Game or a real opening.
    cursor.entered = true;
    value.state.cursor = cursor;
    let mut only_clock = serde_json::to_value(&value).unwrap();
    only_clock["state"]["cursor"] = original["state"]["cursor"].clone();
    assert_eq!(only_clock, original);
    assert!(value.state.engineering.observed_opening.is_none());
    assert!(value.state.air.runtime.sgsu_activity.current.is_none());
    assert!(
        value
            .state
            .air
            .runtime
            .sgsu_activity
            .previous_closed
            .is_none()
    );
    value
}
