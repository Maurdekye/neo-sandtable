//! Whole-campaign tests on the real Graziani's Offensive content.

use std::sync::OnceLock;

use cna_core::decision::ActionSchema;
use cna_core::decision::DecisionResponse;
use cna_core::dice::CampaignRng;
use cna_core::engine::{Command, Game, Progress, Rejection, evaluate};
use cna_core::event::EngineEvent;
use cna_core::ids::SeatId;
use cna_core::visibility::Perspective;
use cna_protocol::{GameEvent, Role, Side};
use serde_json::{Value, json};

use super::*;
use crate::state::Location;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| {
        CnaContent::load(&cna_content::repo_data_dir(), "graziani").expect("graziani content loads")
    })
}

fn new_game(seed: u8) -> Game<Cna> {
    Game {
        state: State::new(content()).expect("initial state"),
        rng: CampaignRng::from_seed([seed; 32]).state(),
    }
}

/// Play a whole campaign, answering every decision with its first option. Returns the final
/// game, every event and the number of decisions answered.
fn play(
    ruleset: Cna,
    seed: u8,
) -> (
    Game<Cna>,
    Vec<EngineEvent>,
    usize,
    Result<Progress, Rejection>,
) {
    let content = content();
    let mut game = new_game(seed);
    let mut events = Vec::new();
    for answered in 0..10_000 {
        let t = match evaluate(&ruleset, content, &game, &Command::Advance) {
            Ok(t) => t,
            Err(e) => return (game, events, answered, Err(e)),
        };
        events.extend(t.events);
        game = t.game;
        if let Some(Progress::Finished { summary }) = t.progress {
            return (game, events, answered, Ok(Progress::Finished { summary }));
        }
        let request = ruleset.pending(content, &game.state).remove(0);
        let action = match &request.space.schema {
            ActionSchema::Choice { options } => json!(options[0].id),
            other => panic!("no test answer for {other:?}"),
        };
        let response = DecisionResponse {
            decision_id: request.id.clone(),
            seat: request.seat,
            controller_epoch: 1,
            decision_revision: request.revision,
            idempotency_key: format!("k{answered}"),
            action,
            public_explanation: None,
        };
        let t = evaluate(&ruleset, content, &game, &Command::Respond(response))
            .expect("first option is legal");
        events.extend(t.events);
        game = t.game;
    }
    panic!("campaign did not finish");
}

#[test]
fn graziani_initial_state_places_the_set_up() {
    let content = content();
    let state = State::new(content).unwrap();
    let on_map = |side: Side| {
        state
            .units_of(side)
            .filter(|u| matches!(u.location, Location::Hex { .. }))
            .count()
    };
    // Spot check: the 1st Libyan Division HQ starts at C4020 (scen:60.31).
    let hq = &state.land.units[&"it.1_libyan_div.1st_libyan_infantry_hq".into()];
    assert_eq!(
        hq.location,
        Location::Hex {
            hex: "C4020".into()
        }
    );
    // Its whole deployed subtree stands with it.
    let libyan_bn = &state.land.units[&"it.1_libyan_div.viii_libyan_bn".into()];
    assert_eq!(libyan_bn.location, hq.location);
    assert!(
        on_map(Side::Axis) > 100,
        "axis on map: {}",
        on_map(Side::Axis)
    );
    assert!(
        on_map(Side::Commonwealth) > 20,
        "cw on map: {}",
        on_map(Side::Commonwealth)
    );
    // Every deployed OA unit has a location other than "not arrived".
    for oa in content
        .units
        .units
        .values()
        .filter(|u| u.arrives.is_deployed())
    {
        let u = &state.land.units[&oa.id];
        assert_ne!(
            u.location,
            Location::NotArrived,
            "{} is deployed but not placed",
            oa.id
        );
    }
    assert!(!state.logistics.dumps.is_empty());
    assert!(state.air.forces.contains_key("axis") && state.air.forces.contains_key("commonwealth"));
}

#[test]
fn dev_profile_plays_graziani_to_the_end_with_initiative_decisions() {
    let (game, events, answered, result) = play(Cna::dev(), 7);
    let summary = match result {
        Ok(Progress::Finished { summary }) => summary,
        other => panic!("expected the campaign to finish, got {other:?}"),
    };
    assert!(summary.contains("Graziani"), "{summary}");
    assert!(game.state.cursor.is_finished());
    // 6 game-turns x 3 OpStages, one initiative declaration each.
    assert_eq!(answered, 18);
    // GT1 initiative is fixed by the scenario; GT2-6 are rolled (two dice per roll at least).
    let initiative_rolls = events
        .iter()
        .filter(
            |e| matches!(&e.event, GameEvent::DiceRolled { rule: Some(r), .. } if r == "land:7.14"),
        )
        .count();
    assert!(initiative_rolls >= 10, "{initiative_rolls} initiative dice");
    // The phase stream reaches every OpStage of the last game-turn.
    let last_op = events.iter().rev().find_map(|e| match &e.event {
        GameEvent::PhaseChanged { clock } if clock.op_stage.is_some() => {
            Some((clock.game_turn, clock.op_stage))
        }
        _ => None,
    });
    assert_eq!(last_op, Some((6, Some(3))));
}

#[test]
fn full_profile_stops_at_the_first_unimplemented_applicable_case() {
    let (_game, _events, _answered, result) = play(Cna::full(), 7);
    match result {
        Err(Rejection::Engine(cna_core::engine::EngineError::Unsupported { case, .. })) => {
            assert!(case.contains(':'), "a citation: {case}");
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn campaigns_are_deterministic_per_seed() {
    let (a, ea, _, _) = play(Cna::dev(), 3);
    let (b, eb, _, _) = play(Cna::dev(), 3);
    assert_eq!(a.rng, b.rng);
    assert_eq!(
        serde_json::to_string(&ea.iter().map(|e| &e.event).collect::<Vec<_>>()).unwrap(),
        serde_json::to_string(&eb.iter().map(|e| &e.event).collect::<Vec<_>>()).unwrap()
    );
}

#[test]
fn limited_intelligence_hides_enemy_stack_contents() {
    let content = content();
    let state = State::new(content).unwrap();
    let ruleset = Cna::dev();
    let axis = ruleset.view(content, &state, Perspective::Side(Side::Axis));
    let enemy: Vec<_> = axis
        .stacks
        .iter()
        .filter(|s| s.side == Side::Commonwealth)
        .collect();
    assert!(!enemy.is_empty());
    for s in &enemy {
        assert!(s.unit_ids.is_empty() && s.visible_count.is_none(), "{s:?}");
    }
    assert!(axis.units.values().all(|u| u.side == Side::Axis));
    // Own units off the map (e.g. the Tripoli box) are listed with their location.
    let off_map = axis
        .units
        .values()
        .filter(|u| u.hex.is_none())
        .filter_map(|u| u.detail.as_ref()?.get("location"))
        .count();
    assert!(
        off_map > 0,
        "axis units in off-map boxes or awaiting set-up are listed"
    );
    // The operator sees both sides in full.
    let op = ruleset.view(content, &state, Perspective::Operator);
    assert!(op.units.values().any(|u| u.side == Side::Commonwealth));
    // inspect refuses an enemy unit without saying whether it exists.
    let cw_unit = state
        .units_of(Side::Commonwealth)
        .next()
        .unwrap()
        .id
        .to_string();
    let seat = Perspective::Seat(SeatId::new(Side::Axis, Role::Commander));
    let refused = ruleset
        .inspect(content, &state, seat, &cw_unit)
        .unwrap_err();
    let unknown = ruleset
        .inspect(content, &state, seat, "no.such.unit")
        .unwrap_err();
    assert_eq!(
        refused.to_string().replace(&cw_unit, "X"),
        unknown.to_string().replace("no.such.unit", "X")
    );
}

#[test]
fn rejects_a_wrong_seat_and_a_stale_revision_without_change() {
    let content = content();
    let ruleset = Cna::dev();
    let mut game = new_game(1);
    let t = evaluate(&ruleset, content, &game, &Command::Advance).unwrap();
    game = t.game;
    let request = ruleset.pending(content, &game.state).remove(0);
    let base = DecisionResponse {
        decision_id: request.id.clone(),
        seat: request.seat,
        controller_epoch: 1,
        decision_revision: request.revision,
        idempotency_key: "k".into(),
        action: Value::String("player_a".into()),
        public_explanation: None,
    };
    let wrong_seat = DecisionResponse {
        seat: SeatId::new(request.seat.side.opponent(), Role::Commander),
        ..base.clone()
    };
    assert!(matches!(
        evaluate(&ruleset, content, &game, &Command::Respond(wrong_seat)),
        Err(Rejection::WrongSeat { .. })
    ));
    let stale = DecisionResponse {
        decision_revision: request.revision + 1,
        ..base.clone()
    };
    assert!(matches!(
        evaluate(&ruleset, content, &game, &Command::Respond(stale)),
        Err(Rejection::StaleRevision { .. })
    ));
    let bad = DecisionResponse {
        action: Value::String("both".into()),
        ..base
    };
    assert!(matches!(
        evaluate(&ruleset, content, &game, &Command::Respond(bad)),
        Err(Rejection::Illegal { .. })
    ));
}
