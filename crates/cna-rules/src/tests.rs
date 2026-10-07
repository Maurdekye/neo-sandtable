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

fn first_answer(schema: &ActionSchema) -> Value {
    match schema {
        ActionSchema::Choice { options } => json!(options[0].id),
        ActionSchema::Unit { among } => json!(among[0]),
        ActionSchema::Integer { max, .. } => json!(max),
        ActionSchema::Record { fields } => Value::Object(
            fields
                .iter()
                .map(|f| (f.name.clone(), first_answer(&f.schema)))
                .collect(),
        ),
        other => panic!("no test answer for {other:?}"),
    }
}

/// Arrival batches conserve row totals instead of sampling each field independently.
fn scripted_answer(
    content: &CnaContent,
    state: &State,
    request: &cna_core::decision::DecisionRequest,
) -> Value {
    baseline::arrival_orders(content, state, request).unwrap_or_else(|| {
        match &request.space.schema {
            ActionSchema::Choice { options } if !options.is_empty() => json!(options[0].id),
            _ if request.space.pass.is_some() => Value::Null,
            other => first_answer(other),
        }
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
    play_until(ruleset, seed, u16::MAX)
}

/// Like `play`, but stop (reporting `AwaitingDecisions`) once the cursor moves past game-turn
/// `last_gt`, so default tests stay quick as the rules grow (CONTRIBUTING: test time).
fn play_until(
    ruleset: Cna,
    seed: u8,
    last_gt: u16,
) -> (
    Game<Cna>,
    Vec<EngineEvent>,
    usize,
    Result<Progress, Rejection>,
) {
    let content = content();
    let mut game = new_game(seed);
    let mut events = Vec::new();
    for answered in 0..100_000 {
        if game.state.cursor.game_turn > last_gt {
            return (game, events, answered, Ok(Progress::AwaitingDecisions));
        }
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
        let action = scripted_answer(content, &game.state, &request);
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

/// Game-Turn 1 of a dev campaign: set-up closes, trucks are distributed, and each of the
/// three OpStages opens its initiative declaration. The bounded default counterpart of the
/// whole-campaign test below.
#[test]
fn dev_profile_plays_game_turn_one_through_setup_and_initiative() {
    let (game, events, answered, result) = play_until(Cna::dev(), 7, 1);
    assert!(result.is_ok(), "{result:?}");
    assert!(game.state.setup.closed);
    assert!(game.state.land.undistributed_trucks.is_empty());
    assert!(answered > 3, "setup adds decisions before initiative");
    let declarations = events
        .iter()
        .filter(|e| matches!(&e.event, GameEvent::DecisionOpened { decision } if decision.kind == "cna.initiative_declaration"))
        .count();
    assert_eq!(
        declarations, 3,
        "one declaration per OpStage of Game-Turn 1"
    );
    assert!(
        game.state.cursor.game_turn >= 2,
        "the campaign moved past Game-Turn 1"
    );
}

#[test]
#[ignore = "slow: whole campaign"]
fn dev_profile_plays_graziani_to_the_end_with_initiative_decisions() {
    let (game, events, answered, result) = play(Cna::dev(), 7);
    let summary = match result {
        Ok(Progress::Finished { summary }) => summary,
        other => panic!("expected the campaign to finish, got {other:?}"),
    };
    assert!(summary.contains("Graziani"), "{summary}");
    assert!(game.state.cursor.is_finished());
    // 6 game-turns x 3 OpStages, one initiative declaration each.
    assert!(answered > 18, "setup adds decisions before initiative");
    assert_eq!(events.iter().filter(|e| matches!(&e.event, GameEvent::DecisionOpened { decision } if decision.kind == "cna.initiative_declaration")).count(), 18);
    assert!(game.state.setup.closed);
    assert!(game.state.land.undistributed_trucks.is_empty());
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

/// Two runs of the same seed agree exactly through Game-Turn 2 (state, RNG and every event).
#[test]
fn campaigns_are_deterministic_per_seed() {
    let (a, ea, _, _) = play_until(Cna::dev(), 3, 2);
    let (b, eb, _, _) = play_until(Cna::dev(), 3, 2);
    assert_eq!(a.rng, b.rng);
    assert_eq!(
        serde_json::to_value(&a.state).unwrap(),
        serde_json::to_value(&b.state).unwrap()
    );
    assert_eq!(
        serde_json::to_string(&ea.iter().map(|e| &e.event).collect::<Vec<_>>()).unwrap(),
        serde_json::to_string(&eb.iter().map(|e| &e.event).collect::<Vec<_>>()).unwrap()
    );
}

#[test]
#[ignore = "slow: whole campaign twice"]
fn whole_campaigns_are_deterministic_per_seed() {
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
    // Own units on the map say whether they have used their move this segment.
    let on_map = axis.units.values().find(|u| u.hex.is_some()).unwrap();
    assert_eq!(
        on_map.detail.as_ref().unwrap().get("moved_this_segment"),
        Some(&serde_json::json!(false))
    );
    // Pending decisions carry their action space as JSON Schema.
    let request = ruleset.pending(content, &state);
    assert!(
        request.is_empty(),
        "no decision is open before the first advance"
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

#[test]
fn source_files_lists_exactly_what_the_loaders_read() {
    let data = cna_content::repo_data_dir();
    let base = cna_content::normalize(&data);
    let files = crate::content::source_files(&data, "graziani").unwrap();
    let rel: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(&base)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    let has = |f: &str| rel.iter().any(|r| r == f);
    for f in [
        "map/hexes.csv",
        "map/areas.toml",
        "map/places.toml",
        "map/coverage.csv",
        "units/oa/it/1_libyan_div.toml",
        "scenarios/graziani/scenario.toml",
        "scenarios/graziani/land_axis.toml",
        "tables/land/14.6-anti-armor-results.toml",
        "rules/land/07-initiative.toml",
    ] {
        assert!(has(f), "{f} is read but not listed: {rel:?}");
    }
    for f in [
        "map/README.md",
        "map/GAPS.md",
        "scenarios/italian_campaign/scenario.toml",
    ] {
        assert!(!has(f), "{f} is not read but is listed");
    }
    // Sorted, deduplicated and stable.
    let mut sorted = files.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(files, sorted);
    assert_eq!(
        files,
        crate::content::source_files(&data, "graziani").unwrap()
    );
    // A scenario reusing another's set-up lists the reused files.
    let italian = crate::content::source_files(&data, "italian_campaign").unwrap();
    assert!(
        italian
            .iter()
            .any(|p| p.ends_with("graziani/land_axis.toml"))
    );
    assert!(
        italian
            .iter()
            .any(|p| p.ends_with("italian_campaign/scenario.toml"))
    );
}

/// A board change that touches no unit (an 8.88 box movement ban expiring with its Operations
/// Stage) is announced to its owner, and whether the enemy had CP to reset at the same moment
/// changes nothing the owner receives (server-review: the emitter's no-change shortcut once
/// missed the cursor, so the expiry was announced only when hidden enemy CP also changed).
#[test]
fn stage_end_board_changes_reach_the_owner_whatever_the_enemy_spent() {
    use crate::logistics::box_handling::{Carrier, record_goods};
    use crate::seq::{Block, Half};
    let content = content();
    let mut s = State::new(content).expect("initial state");
    for u in s.land.units.values_mut() {
        u.location = Location::NotArrived;
        u.detached = true;
        u.attached_to = None;
    }
    s.turn.weather = Some(crate::state::WeatherState {
        kind: cna_tables::land::weather::WeatherKind::Normal,
        storm_sections: vec![],
    });
    s.turn.player_a = Some(Side::Commonwealth);
    s.cursor.block = Block::PlayerHalf;
    s.cursor.half = Some(Half::B);
    s.cursor.index = crate::seq::PLAYER_HALF.len() - 1;
    s.cursor.entered = true;
    s.cursor.op_stage = Some(1);
    let own: cna_core::ids::UnitId = "cw.unassigned_inf.1st_rnf_mg_bn".into();
    s.land.units.get_mut(&own).unwrap().location = Location::OffMap {
        id: "box_tripoli".into(),
    };
    let water = cna_content::scenario::Supplies {
        water: 1,
        ..Default::default()
    };
    record_goods(&mut s, &Carrier::Unit(own.clone()), water, true).expect("box loading");
    let enemy: cna_core::ids::UnitId = "it.1_libyan_div.viii_libyan_bn".into();
    s.land.units.get_mut(&enemy).unwrap().location = Location::Hex {
        hex: "C4020".into(),
    };
    let quiet: Game<Cna> = Game {
        state: s,
        rng: CampaignRng::from_seed([7; 32]).state(),
    };
    let mut spent = quiet.clone();
    spent
        .state
        .land
        .units
        .get_mut(&enemy)
        .unwrap()
        .cp_spent_quarters = 4;
    let ruleset = Cna::dev();
    crate::testkit::assert_indistinguishable(
        &ruleset,
        content,
        &quiet.state,
        &spent.state,
        Side::Commonwealth,
    );
    let p = Perspective::Side(Side::Commonwealth);
    let ban = |view: &cna_protocol::ViewState| {
        view.units[own.as_str()].detail.as_ref().unwrap()["box_movement_block"].clone()
    };
    assert!(ban(&ruleset.view(content, &quiet.state, p)).is_string());
    let readable = |game: &Game<Cna>| {
        let t = evaluate(&ruleset, content, game, &Command::Advance).expect("advance");
        assert_eq!(t.game.state.cursor.op_stage, Some(2));
        assert!(ban(&ruleset.view(content, &t.game.state, p)).is_null());
        assert!(t.events.iter().any(|e| p.can_see(&e.audience)
            && matches!(&e.event, GameEvent::UnitUpdated { unit } if unit.id == own.as_str())));
        json!(
            t.events
                .iter()
                .filter(|e| p.can_see(&e.audience))
                .collect::<Vec<_>>()
        )
    };
    assert_eq!(readable(&quiet), readable(&spent));
}

/// A seat's commentary on its accepted answer reaches its side and the operator with the
/// resolution, trimmed and bounded, and never the enemy; an answer without one carries none.
#[test]
fn seat_commentary_travels_with_its_own_resolution_only() {
    let ruleset = Cna::dev();
    let content = content();
    let mut game = new_game(4);
    loop {
        let t = evaluate(&ruleset, content, &game, &Command::Advance).expect("advance");
        game = t.game;
        if !ruleset.pending(content, &game.state).is_empty() {
            break;
        }
    }
    let request = ruleset.pending(content, &game.state).remove(0);
    let respond = |explanation: Option<String>| {
        let action = scripted_answer(content, &game.state, &request);
        evaluate(
            &ruleset,
            content,
            &game,
            &Command::Respond(DecisionResponse {
                decision_id: request.id.clone(),
                seat: request.seat,
                controller_epoch: 1,
                decision_revision: request.revision,
                idempotency_key: "c1".into(),
                action,
                public_explanation: explanation,
            }),
        )
        .expect("legal answer")
    };
    let said = format!("  {}  ", "x".repeat(2_500));
    let t = respond(Some(said));
    let resolved: Vec<&EngineEvent> = t
        .events
        .iter()
        .filter(|e| matches!(e.event, GameEvent::DecisionResolved { .. }))
        .collect();
    assert_eq!(resolved.len(), 1);
    let GameEvent::DecisionResolved { explanation, .. } = &resolved[0].event else {
        unreachable!()
    };
    assert_eq!(explanation.as_deref(), Some("x".repeat(2_000).as_str()));
    let side = request.seat.side;
    assert!(Perspective::Side(side).can_see(&resolved[0].audience));
    assert!(Perspective::Operator.can_see(&resolved[0].audience));
    assert!(!Perspective::Side(side.opponent()).can_see(&resolved[0].audience));
    let t = respond(Some("   ".into()));
    assert!(t.events.iter().any(|e| matches!(
        &e.event,
        GameEvent::DecisionResolved {
            explanation: None,
            ..
        }
    )));
}

/// Live unit updates carry `moved_this_segment` like snapshots, a new Movement Segment sends the
/// reset, and decision ids count per seat so they reveal nothing about other seats.
#[test]
fn moved_flags_stay_live_and_decision_ids_count_per_seat() {
    let ruleset = Cna::dev();
    let content = content();
    let mut game = new_game(3);
    let mut rng = CampaignRng::from_seed([7; 32]);
    let mut mover: Option<String> = None;
    let mut reset_seen = false;
    let mut opened_per_seat = std::collections::BTreeMap::<String, u32>::new();
    let check = |events: &[EngineEvent],
                 mover: &Option<String>,
                 reset_seen: &mut bool,
                 opened: &mut std::collections::BTreeMap<String, u32>| {
        for e in events {
            match &e.event {
                GameEvent::UnitUpdated { unit } => {
                    let flag = unit
                        .detail
                        .as_ref()
                        .and_then(|d| d.get("moved_this_segment"));
                    assert!(flag.is_some(), "UnitUpdated for {} lacks the flag", unit.id);
                    if mover.as_deref() == Some(unit.id.as_str()) && flag == Some(&json!(false)) {
                        *reset_seen = true;
                    }
                }
                GameEvent::DecisionOpened { decision } => {
                    let n = opened.entry(decision.seat.clone()).or_default();
                    *n += 1;
                    assert_eq!(decision.id, format!("{}-{n}", decision.seat));
                }
                _ => {}
            }
        }
    };
    for answered in 0..20_000 {
        let t = evaluate(&ruleset, content, &game, &Command::Advance).unwrap();
        check(&t.events, &mover, &mut reset_seen, &mut opened_per_seat);
        game = t.game;
        if reset_seen || matches!(t.progress, Some(Progress::Finished { .. })) {
            break;
        }
        let request = ruleset.pending(content, &game.state).remove(0);
        let action = if request.kind == land::movement::KIND && mover.is_none() {
            baseline::random_orders(content, &game.state, &request, &mut rng)
        } else {
            scripted_answer(content, &game.state, &request)
        };
        let moving = action
            .as_array()
            .and_then(|orders| orders.first())
            .and_then(|order| order["unit"].as_str())
            .map(str::to_owned);
        let response = DecisionResponse {
            decision_id: request.id.clone(),
            seat: request.seat,
            controller_epoch: 1,
            decision_revision: request.revision,
            idempotency_key: format!("k{answered}"),
            action,
            public_explanation: None,
        };
        let t = evaluate(&ruleset, content, &game, &Command::Respond(response)).unwrap();
        if let Some(unit) = moving {
            let flagged = t.events.iter().any(|e| {
                matches!(&e.event, GameEvent::UnitUpdated { unit: u }
                    if u.id == unit
                        && u.detail.as_ref().unwrap()["moved_this_segment"] == json!(true))
            });
            assert!(flagged, "the move's live update says the unit moved");
            mover = Some(unit);
        }
        check(&t.events, &mover, &mut reset_seen, &mut opened_per_seat);
        game = t.game;
    }
    assert!(mover.is_some(), "a baseline move happened");
    assert!(
        reset_seen,
        "the next Movement Segment sent the mover's reset"
    );
    assert!(
        opened_per_seat.len() > 2,
        "several seats received decisions"
    );
}

/// Change hidden facts of one on-map unit of `side`: strength, fatigue, spent CP and supply.
fn perturb_hidden(state: &mut State, side: Side) {
    let id = state
        .land
        .units
        .values()
        .find(|u| u.side == side && u.location.hex().is_some())
        .map(|u| u.id.clone())
        .expect("an on-map unit");
    let unit = state.land.units.get_mut(&id).unwrap();
    unit.toe = Some(cna_content::units::Toe::Under { under: 1 });
    unit.cohesion_quarters -= 20;
    unit.cp_spent_quarters += 12;
    let supply = state.logistics.unit_supply.entry(id).or_default();
    supply.activity_water = cna_core::quantity::WaterPoints::new(supply.activity_water.get() + 7);
}

/// The enemy learns nothing from a unit's strength, fatigue, spent CP or supply, before and
/// after set-up opens (land:3.6). Uses the shared indistinguishability harness.
/// Cases: land:3.61, land:3.62
#[test]
fn hidden_unit_facts_are_indistinguishable_to_the_enemy() {
    let ruleset = Cna::dev();
    let content = content();
    let mut game = new_game(5);
    for _ in 0..2 {
        for (hidden, observer) in [
            (Side::Commonwealth, Side::Axis),
            (Side::Axis, Side::Commonwealth),
        ] {
            let mut other = game.state.clone();
            perturb_hidden(&mut other, hidden);
            crate::testkit::assert_indistinguishable(
                &ruleset,
                content,
                &game.state,
                &other,
                observer,
            );
        }
        game = evaluate(&ruleset, content, &game, &Command::Advance)
            .unwrap()
            .game;
    }
}

/// The harness itself: a fact the enemy may see (a stack appearing in a new hex) is caught.
#[test]
#[should_panic(expected = "can tell the states apart")]
fn indistinguishability_harness_catches_visible_differences() {
    let ruleset = Cna::dev();
    let content = content();
    let game = new_game(5);
    let mut other = game.state.clone();
    let unit = other
        .land
        .units
        .values_mut()
        .find(|u| u.side == Side::Commonwealth && u.location.hex().is_some())
        .unwrap();
    unit.location = Location::Hex {
        hex: cna_core::ids::HexId::new("C4119"),
    };
    crate::testkit::assert_indistinguishable(&ruleset, content, &game.state, &other, Side::Axis);
}

/// Advancing the game teaches the enemy nothing about hidden unit facts: same events per seat,
/// same resulting views (set-up opens the same windows either way).
/// Cases: land:3.61, land:3.62
#[test]
fn advancing_reveals_no_hidden_unit_facts() {
    let ruleset = Cna::dev();
    let content = content();
    let game = new_game(9);
    for (hidden, observer) in [
        (Side::Commonwealth, Side::Axis),
        (Side::Axis, Side::Commonwealth),
    ] {
        let mut other = game.clone();
        perturb_hidden(&mut other.state, hidden);
        crate::testkit::assert_action_indistinguishable(
            &ruleset,
            content,
            &game,
            &other,
            &Command::Advance,
            observer,
        );
    }
}

/// The action harness itself: an action outcome the enemy may see differently is caught.
#[test]
#[should_panic(expected = "can tell the")]
fn action_harness_catches_visible_differences() {
    let ruleset = Cna::dev();
    let content = content();
    let game = new_game(9);
    let mut other = game.clone();
    let unit = other
        .state
        .land
        .units
        .values_mut()
        .find(|u| u.side == Side::Commonwealth && u.location.hex().is_some())
        .unwrap();
    unit.location = Location::Hex {
        hex: cna_core::ids::HexId::new("C4119"),
    };
    crate::testkit::assert_action_indistinguishable(
        &ruleset,
        content,
        &game,
        &other,
        &Command::Advance,
        Side::Axis,
    );
}

/// `views` shares one board and one schema per decision between perspectives; it must equal each
/// perspective's own `view`, from set-up through the first movement decisions.
#[test]
fn shared_views_equal_each_perspectives_own_view() {
    let ruleset = Cna::dev();
    let content = content();
    let mut game = new_game(4);
    let all: Vec<Perspective> = Perspective::all().collect();
    let mut checked = 0;
    for answered in 0..2_000 {
        let at_movement = game.state.cursor.anchor() == "opstage.movement_and_combat.movement";
        if answered % 25 == 0 || at_movement {
            let shared = ruleset.views(content, &game.state, &all);
            for (p, view) in all.iter().zip(&shared) {
                assert_eq!(
                    view,
                    &ruleset.view(content, &game.state, *p),
                    "{p:?} at {answered}"
                );
                assert_eq!(view.clock, ruleset.clock(content, &game.state));
            }
            checked += 1;
        }
        if at_movement {
            break;
        }
        let command = if ruleset.pending(content, &game.state).is_empty() {
            Command::Advance
        } else {
            let request = ruleset.pending(content, &game.state).remove(0);
            let action = scripted_answer(content, &game.state, &request);
            Command::Respond(DecisionResponse {
                decision_id: request.id.clone(),
                seat: request.seat,
                controller_epoch: 1,
                decision_revision: request.revision,
                idempotency_key: format!("k{answered}"),
                action,
                public_explanation: None,
            })
        };
        game = evaluate(&ruleset, content, &game, &command)
            .expect("legal play")
            .game;
    }
    assert_eq!(
        game.state.cursor.anchor(),
        "opstage.movement_and_combat.movement",
        "reached the first movement decisions ({checked} checks)"
    );
    assert!(
        !game.state.decisions.pending.is_empty() && game.state.land.units.len() > 100,
        "the last check saw pending decisions and a populated board"
    );
    // An AI seat reads `observe` before most decisions, so its size is paid in model tokens on
    // every one. Per-unit bookkeeping belongs in `inspect`: listing every unit's ration record
    // once made an Axis seat's observation 85 KB, 68 KB of it ration history.
    for p in all.iter().filter(|p| matches!(p, Perspective::Seat(_))) {
        let bytes = serde_json::to_string(&ruleset.observe(content, &game.state, *p))
            .expect("observations serialize")
            .len();
        assert!(bytes < 30_000, "{p} observation grew to {bytes} bytes");
    }
}

/// Every visible change in Game-Turn 1 is announced by an event its viewer receives (both
/// sides and the operator), so live boards never go stale between snapshots.
#[test]
fn every_visible_change_in_game_turn_one_is_announced() {
    let ruleset = Cna::dev();
    let content = content();
    let mut game = new_game(4);
    let perspectives = [
        Perspective::Side(Side::Axis),
        Perspective::Side(Side::Commonwealth),
        Perspective::Operator,
    ];
    let mut gaps = Vec::new();
    for answered in 0..100_000 {
        if game.state.cursor.game_turn > 1 || game.state.cursor.is_finished() {
            break;
        }
        let command = if ruleset.pending(content, &game.state).is_empty() {
            Command::Advance
        } else {
            let request = ruleset.pending(content, &game.state).remove(0);
            let action = scripted_answer(content, &game.state, &request);
            Command::Respond(DecisionResponse {
                decision_id: request.id.clone(),
                seat: request.seat,
                controller_epoch: 1,
                decision_revision: request.revision,
                idempotency_key: format!("k{answered}"),
                action,
                public_explanation: None,
            })
        };
        let t = evaluate(&ruleset, content, &game, &command).expect("legal play");
        for p in perspectives {
            if let Err(gap) = crate::testkit::events_explain_view_changes(
                &ruleset,
                content,
                &game.state,
                &t.game.state,
                &t.events,
                p,
            ) {
                gaps.push(format!(
                    "{} @ {}: {p:?}: {gap}",
                    answered,
                    game.state.cursor.anchor()
                ));
            }
        }
        game = t.game;
    }
    assert!(
        gaps.is_empty(),
        "{} gaps, first: {:#?}",
        gaps.len(),
        &gaps[..]
    );
}
