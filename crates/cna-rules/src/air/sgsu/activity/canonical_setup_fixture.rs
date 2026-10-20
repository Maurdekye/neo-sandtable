//! TEST-LOCAL fixture proposal: unchanged lawful setup recipe and public PRE flow.
//! No production visibility/API, invented inventory/history, or producer admission.
use super::*;
use crate::state::Pending;
use cna_core::decision::{ActionSchema, DecisionResponse};
use cna_core::dice::CampaignRng;
use cna_core::engine::{Command, Rejection, Ruleset, evaluate};
use serde_json::{Value, json};

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

fn whole(game: &Game<Cna>) -> Vec<u8> {
    serde_json::to_vec(game).unwrap()
}

fn checkpoint(game: &Game<Cna>) -> Game<Cna> {
    serde_json::from_slice(&whole(game)).unwrap()
}

fn schema_answer(schema: &ActionSchema) -> Value {
    match schema {
        ActionSchema::Choice { options } => {
            json!(options.first().expect("mandatory declared choice").id)
        }
        ActionSchema::Unit { among } => json!(among.first().expect("mandatory declared unit")),
        ActionSchema::Integer { min, .. } => json!(min),
        ActionSchema::Bool => json!(false),
        ActionSchema::Record { fields } => Value::Object(
            fields
                .iter()
                .filter(|f| !f.optional)
                .map(|f| (f.name.clone(), schema_answer(&f.schema)))
                .collect(),
        ),
        ActionSchema::List { min: 0, .. } => json!([]),
        other => panic!("no lawful mandatory component answer for {other:?}"),
    }
}

fn next_command(value: &Game<Cna>, serial: usize) -> Command {
    let Some(request) = Cna::dev()
        .pending(content(), &value.state)
        .into_iter()
        .next()
    else {
        return Command::Advance;
    };
    // Declared passes first. Mandatory source-conserving arrival/attrition answers
    // reuse public procedures; unknown/error/unsupported results are never skipped.
    let action = if request.space.pass.is_some() {
        Value::Null
    } else if let Some(answer) = crate::baseline::arrival_orders(content(), &value.state, &request)
    {
        answer
    } else if request.kind == crate::logistics::attrition::KIND {
        crate::logistics::attrition::baseline(content(), &value.state, request.seat.side)
            .expect("actual conserved mandatory attrition answer")
    } else {
        schema_answer(&request.space.schema)
    };
    request
        .space
        .check(&action)
        .expect("answer uses declared space");
    Command::Respond(DecisionResponse {
        decision_id: request.id,
        seat: request.seat,
        controller_epoch: 1,
        decision_revision: request.revision,
        idempotency_key: format!("engineering-stream-{serial}"),
        action,
        public_explanation: None,
    })
}

fn accepted_with_checkpoint_retry(
    value: &Game<Cna>,
    command: &Command,
) -> cna_core::engine::Transition<Cna> {
    let before = whole(value);
    let saved = checkpoint(value);
    let result = evaluate(&Cna::dev(), content(), value, command).unwrap_or_else(|error| {
        panic!(
            "component {} {command:?}: {error:?}",
            value.state.cursor.anchor()
        )
    });
    let recovered = evaluate(&Cna::dev(), content(), &saved, command).unwrap();
    let retried_from_original = evaluate(&Cna::dev(), content(), value, command).unwrap();
    for replay in [recovered, retried_from_original] {
        assert_eq!(whole(&replay.game), whole(&result.game));
        assert_eq!(replay.events, result.events);
        assert_eq!(replay.progress, result.progress);
    }
    assert_eq!(whole(value), before);
    if let Command::Respond(response) = command {
        let committed = whole(&result.game);
        assert_eq!(
            evaluate(&Cna::dev(), content(), &result.game, command).unwrap_err(),
            Rejection::UnknownDecision {
                decision_id: response.decision_id.clone()
            }
        );
        assert_eq!(whole(&result.game), committed);
    }
    if !result.game.state.decisions.pending.is_empty() {
        let waiting = evaluate(&Cna::dev(), content(), &result.game, &Command::Advance).unwrap();
        assert_eq!(whole(&waiting.game), whole(&result.game));
        assert!(waiting.events.is_empty());
    }
    result
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
            // Setup close may open pre-game convoy plans before the cursor leaves
            // Setup (setup/decisions.rs771; logistics/convoys.rs124-153/400-419).
            // Respond records each declared answer without advancing the cursor;
            // the next real Advance leaves Setup only after all plans are answered.
            let mut admitted = closed.game;
            for serial in 0..4096 {
                assert!(admitted.state.setup.closed);
                if admitted.state.cursor.block == Block::Pre {
                    break;
                }
                assert_eq!(admitted.state.cursor.block, Block::Setup);
                let command = next_command(&admitted, serial);
                admitted = accepted_with_checkpoint_retry(&admitted, &command).game;
            }
            // Preserve the original real PRE/no-opening admission obligations.
            assert_eq!(admitted.state.cursor.block, Block::Pre);
            assert!(admitted.state.engineering.observed_opening.is_none());
            assert!(admitted.state.air.runtime.sgsu_activity.current.is_none());
            assert!(
                admitted
                    .state
                    .air
                    .runtime
                    .sgsu_activity
                    .previous_closed
                    .is_none()
            );
            admitted
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
