//! Actual owner response, production finish/Advance and checkpoint controls.
use super::*;
use crate::{
    Cna,
    land::breakdown::{Asset, markers::BrokenMarker},
    seq::{Block, Half, PLAYER_HALF, System},
    state::{DumpLocation, WeatherState},
};
use cna_core::{
    decision::DecisionResponse,
    dice::CampaignRng,
    engine::{Command, Game, Ruleset, evaluate},
    visibility::Perspective,
};
use serde_json::json;

const MARKER: &str = "field-fixture";

mod cw_recce_tests;
mod cw_truck_tests;
mod recce_tests;
mod truck_tests;

fn fixture(points: i32, roll: u8, captured: bool) -> (CnaContent, Game<Cna>) {
    let mut c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    c.bounds.end_gt = 1;
    c.bounds.end_opstage = 1;
    c.bounds.systems = vec![System::Land];
    let mut state = State::new(&c).unwrap();
    let source_side = if captured {
        Side::Commonwealth
    } else {
        Side::Axis
    };
    let source = state
        .land
        .units
        .values()
        .find(|u| u.side == source_side)
        .unwrap()
        .id
        .clone();
    let anchor = state
        .land
        .units
        .values()
        .find(|u| u.side == Side::Axis)
        .unwrap()
        .id
        .clone();
    for unit in state.land.units.values_mut() {
        unit.location = Location::Eliminated;
    }
    state.land.units.get_mut(&anchor).unwrap().location = Location::Hex {
        hex: "C4807".into(),
    };
    state.turn.player_a = Some(Side::Commonwealth);
    state.turn.weather = Some(WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    state.cursor.block = Block::PlayerHalf;
    state.cursor.half = Some(Half::B);
    state.cursor.op_stage = Some(1);
    state.cursor.index = PLAYER_HALF
        .iter()
        .position(|step| step.anchor == ANCHOR)
        .unwrap();
    state.cursor.entered = false;
    let weapon = c
        .units
        .weapons
        .iter()
        .find(|(_, w)| w.kind == "tank")
        .unwrap()
        .0
        .clone();
    state.land.breakdown.markers.insert(
        MARKER.into(),
        BrokenMarker {
            id: MARKER.into(),
            side: Side::Axis,
            hex: "C4807".into(),
            assets: vec![Asset {
                unit: source,
                equipment: Equipment::Weapon(weapon),
                points,
                cohort: None,
            }],
            source_pool: None,
            pool_assets: vec![],
            pool_fuel_cohorts: vec![],
            passengers: Default::default(),
            transport: Default::default(),
            cargo: Default::default(),
            tank_fuel: Default::default(),
            activity_water: Default::default(),
            fuel_cohorts: vec![],
            paid_truck_water: Default::default(),
            water_credit_stage: None,
        },
    );
    for stock in state.logistics.unit_supply.values_mut() {
        stock.carried = Supplies::default();
    }
    for dump in state.logistics.dumps.values_mut() {
        dump.active = false;
    }
    let dump = state.logistics.dumps.get_mut("ax_dump_1").unwrap();
    dump.side = Side::Axis;
    dump.active = true;
    dump.dummy = false;
    dump.location = DumpLocation::Hex {
        hex: "C4807".into(),
    };
    dump.supplies = Supplies {
        fuel: 20,
        ..Default::default()
    };
    let rng = (0..=255)
        .map(|n| CampaignRng::from_seed([n; 32]))
        .find(|rng| {
            let mut trial = rng.clone();
            trial.d6().value() == roll
        })
        .unwrap()
        .state();
    (c, Game { state, rng })
}

fn opened(c: &CnaContent, game: &Game<Cna>) -> Game<Cna> {
    evaluate(&Cna::dev(), c, game, &Command::Advance)
        .unwrap()
        .game
}
fn command(c: &CnaContent, game: &Game<Cna>, action: Value) -> Command {
    let request = Cna::dev()
        .pending(c, &game.state)
        .into_iter()
        .find(|r| r.kind == KIND)
        .unwrap();
    Command::Respond(DecisionResponse {
        decision_id: request.id.clone(),
        seat: request.seat,
        controller_epoch: 1,
        decision_revision: request.revision,
        idempotency_key: request.id.to_string(),
        action,
        public_explanation: None,
    })
}
fn answered(c: &CnaContent, game: &Game<Cna>, points: i32) -> Game<Cna> {
    evaluate(
        &Cna::dev(),
        c,
        game,
        &command(c, game, json!([{"marker":MARKER,"points":points}])),
    )
    .unwrap()
    .game
}
fn finish_step(c: &CnaContent, game: &mut Game<Cna>) -> Result<Vec<EngineEvent>, EngineError> {
    let mut rng = CampaignRng::from_state(&game.rng);
    let mut events = vec![];
    Cna::full().finish_step(
        c,
        &mut game.state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )?;
    game.rng = rng.state();
    Ok(events)
}

/// Cases: land:22.12, land:22.25, land:22.26, land:22.5
#[test]
fn respond_only_buffers_then_advance_prepays_rolls_and_returns_real_receipt() {
    let (c, game) = fixture(4, 1, false);
    let game = opened(&c, &game);
    let before = serde_json::to_value((&game.state.land.units, &game.state.logistics)).unwrap();
    let accepted = answered(&c, &game, 4);
    assert_eq!(accepted.rng, game.rng);
    assert_eq!(
        serde_json::to_value((&accepted.state.land.units, &accepted.state.logistics)).unwrap(),
        before
    );
    assert_eq!(
        accepted.state.land.breakdown.markers[MARKER].assets[0].points,
        4
    );
    let transition = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
    assert_eq!(
        transition.game.state.land.breakdown.markers[MARKER].assets[0].points,
        3
    );
    assert_eq!(
        transition
            .game
            .state
            .land
            .replacement_receipts
            .lots
            .values()
            .map(|r| r.points)
            .sum::<i32>(),
        1
    );
    assert_eq!(
        transition.game.state.logistics.dumps["ax_dump_1"]
            .supplies
            .fuel,
        16
    );
    assert_eq!(transition.game.state.land.units, game.state.land.units);
    assert!(transition.events.iter().any(
        |e| matches!(&e.event, GameEvent::Note { text } if text.contains("die 1, 1 tank points"))
    ));
}

/// Cases: land:22.25, land:22.26, land:22.5
#[test]
fn failed_roll_keeps_marker_and_spends_attempt_fuel() {
    let (c, game) = fixture(1, 5, false);
    let mut game = answered(&c, &opened(&c, &game), 1);
    let marker = serde_json::to_value(&game.state.land.breakdown.markers[MARKER]).unwrap();
    finish_step(&c, &mut game).unwrap();
    assert_eq!(
        serde_json::to_value(&game.state.land.breakdown.markers[MARKER]).unwrap(),
        marker
    );
    assert!(game.state.land.replacement_receipts.lots.is_empty());
    assert_eq!(game.state.logistics.dumps["ax_dump_1"].supplies.fuel, 19);
}

/// Cases: land:22.13, land:22.26
#[test]
fn shortage_and_excess_attempt_reject_actual_respond_without_game_or_rng_changes() {
    let (c, mut game) = fixture(4, 1, false);
    game.state
        .logistics
        .dumps
        .get_mut("ax_dump_1")
        .unwrap()
        .supplies
        .fuel = 3;
    let game = opened(&c, &game);
    let before = serde_json::to_value(&game).unwrap();
    for points in [4, 5] {
        assert!(
            evaluate(
                &Cna::dev(),
                &c,
                &game,
                &command(&c, &game, json!([{"marker":MARKER,"points":points}]))
            )
            .is_err()
        );
        assert_eq!(serde_json::to_value(&game).unwrap(), before);
    }
}

/// Cases: land:22.25, land:22.26, land:22.5
#[test]
fn late_receipt_failure_after_payment_roll_and_marker_write_preserves_full_game_and_events() {
    let (c, game) = fixture(1, 1, false);
    let mut game = answered(&c, &opened(&c, &game), 1);
    game.state
        .land
        .replacement_receipts
        .next_serial
        .insert(Side::Axis, u64::MAX);
    let before = serde_json::to_value(&game).unwrap();
    let mut rng = CampaignRng::from_state(&game.rng);
    let rng_before = rng.state();
    let mut events = vec![EngineEvent::new(
        Audience::Side(Side::Commonwealth),
        GameEvent::Note {
            text: "existing event".into(),
        },
    )];
    let previous = events.clone();
    assert!(matches!(
        Cna::full().finish_step(
            &c,
            &mut game.state,
            &mut Cx {
                rng: &mut rng,
                events: &mut events
            }
        ),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(serde_json::to_value(&game).unwrap(), before);
    assert_eq!(rng.state(), rng_before);
    assert_eq!(events, previous);
}

/// Cases: land:22.25, land:22.26, land:22.5
#[test]
fn answered_checkpoint_replays_identically_and_finished_closure_never_pays_or_rolls_twice() {
    let (c, game) = fixture(1, 1, false);
    let game = answered(&c, &opened(&c, &game), 1);
    let recovered: Game<Cna> =
        serde_json::from_value(serde_json::to_value(&game).unwrap()).unwrap();
    let first = evaluate(&Cna::dev(), &c, &game, &Command::Advance).unwrap();
    let replay = evaluate(&Cna::dev(), &c, &recovered, &Command::Advance).unwrap();
    assert_eq!(
        serde_json::to_value(&first.game).unwrap(),
        serde_json::to_value(&replay.game).unwrap()
    );
    assert_eq!(first.events, replay.events);
    let mut same_step = game;
    finish_step(&c, &mut same_step).unwrap();
    let done = serde_json::to_value(&same_step).unwrap();
    assert!(finish_step(&c, &mut same_step).unwrap().is_empty());
    assert_eq!(serde_json::to_value(&same_step).unwrap(), done);
}

/// Cases: land:3.62, land:22.11, land:22.25, land:22.5
/// Interpretations: interp:land-0052
#[test]
fn captured_actual_flow_pairs_reveal_only_equal_public_faces_not_private_repaired_counts() {
    let (c, left) = fixture(4, 1, true);
    let (other_content, right) = fixture(8, 1, true);
    let left = opened(&c, &left);
    let right = opened(&other_content, &right);
    let left = answered(&c, &left, 4);
    let right = answered(&other_content, &right, 8);
    let left = evaluate(&Cna::dev(), &c, &left, &Command::Advance).unwrap();
    let right = evaluate(&Cna::dev(), &other_content, &right, &Command::Advance).unwrap();
    assert_eq!(
        left.game
            .state
            .land
            .replacement_receipts
            .lots
            .values()
            .map(|r| r.points)
            .sum::<i32>(),
        1
    );
    assert_eq!(
        right
            .game
            .state
            .land
            .replacement_receipts
            .lots
            .values()
            .map(|r| r.points)
            .sum::<i32>(),
        2
    );
    for perspective in [
        Perspective::Side(Side::Commonwealth),
        Perspective::Seat(SeatId::new(Side::Commonwealth, Role::RearArea)),
    ] {
        let visible = |events: &[EngineEvent]| {
            events
                .iter()
                .filter(|e| e.visible_to(perspective))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(visible(&left.events), visible(&right.events));
        assert_eq!(
            Cna::dev().observe(&c, &left.game.state, perspective),
            Cna::dev().observe(&other_content, &right.game.state, perspective)
        );
    }
}

/// Cases: land:22.12, land:3.6
#[test]
fn empty_domain_still_opens_the_fixed_window_and_declared_pass_spends_and_rolls_nothing() {
    let (c, mut game) = fixture(1, 1, false);
    game.state.land.breakdown.markers.clear();
    let game = opened(&c, &game);
    assert_eq!(
        game.state
            .decisions
            .pending
            .iter()
            .filter(|p| p.kind == KIND)
            .count(),
        1
    );
    let accepted = evaluate(&Cna::dev(), &c, &game, &command(&c, &game, Value::Null))
        .unwrap()
        .game;
    let mut resolved = accepted.clone();
    let before = serde_json::to_value(&resolved.state.logistics).unwrap();
    assert!(finish_step(&c, &mut resolved).unwrap().is_empty());
    assert_eq!(resolved.rng, accepted.rng);
    assert_eq!(
        serde_json::to_value(&resolved.state.logistics).unwrap(),
        before
    );
    assert!(resolved.state.land.recovery.resolved);
}

/// Cases: land:3.61, land:10.15, land:22.13, land:22.26
#[test]
fn hidden_control_preserves_answer_knowledge_and_acceptance_then_privately_excludes_without_cost() {
    let (c, mut controlled) = fixture(4, 1, false);
    let enemies: Vec<cna_core::ids::UnitId> = [
        "cw.unassigned_inf.3rd_coldstream_guards",
        "cw.unassigned_inf.2nd_scots_guards",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    let site: HexId = "C4807".into();
    // Real printed infantry counters stay in the same place in both worlds.
    // Only their undisclosed cohesion changes whether they exert control.
    let neighbor = c
        .map
        .neighbors(&site)
        .into_iter()
        .find(|hex| {
            for id in &enemies {
                let unit = controlled.state.land.units.get_mut(id).unwrap();
                unit.location = Location::Hex {
                    hex: hex.id.clone(),
                };
                unit.detached = true;
                unit.toe = Some(cna_content::units::Toe::Under { under: 5 });
                unit.cohesion_quarters = 0;
            }
            for id in &enemies {
                assert_eq!(
                    crate::land::formation::strength(&c, &controlled.state, id),
                    5
                );
            }
            matches!(
                zoc::controlled(&c, &controlled.state, Side::Commonwealth, &site, false),
                Ok(true)
            )
        })
        .expect("a passable adjacent hex for the real control fixture");
    for id in &enemies {
        assert_eq!(
            controlled.state.land.units[id].location.hex(),
            Some(&neighbor.id)
        );
    }
    let mut clear: Game<Cna> =
        serde_json::from_value(serde_json::to_value(&controlled).unwrap()).unwrap();
    for id in &enemies {
        clear
            .state
            .land
            .units
            .get_mut(id)
            .unwrap()
            .cohesion_quarters = -104;
    }
    assert!(zoc::controlled(&c, &controlled.state, Side::Commonwealth, &site, false).unwrap());
    assert!(!zoc::controlled(&c, &clear.state, Side::Commonwealth, &site, false).unwrap());
    let perspective = Perspective::Seat(SeatId::new(Side::Axis, Role::RearArea));
    let controlled = opened(&c, &controlled);
    let clear = opened(&c, &clear);
    assert_eq!(
        Cna::dev().observe(&c, &controlled.state, perspective),
        Cna::dev().observe(&c, &clear.state, perspective)
    );
    assert_eq!(
        Cna::dev().view(&c, &controlled.state, perspective),
        Cna::dev().view(&c, &clear.state, perspective)
    );
    assert_eq!(
        serde_json::to_value(Cna::dev().pending(&c, &controlled.state)).unwrap(),
        serde_json::to_value(Cna::dev().pending(&c, &clear.state)).unwrap()
    );
    let response = command(&c, &controlled, json!([{"marker": MARKER, "points": 4}]));
    let blocked = evaluate(&Cna::dev(), &c, &controlled, &response).unwrap();
    let allowed = evaluate(&Cna::dev(), &c, &clear, &response).unwrap();
    assert_eq!(blocked.events, allowed.events);
    assert_eq!(blocked.game.rng, controlled.rng);
    assert_eq!(allowed.game.rng, clear.rng);
    assert_eq!(
        serde_json::to_value(&blocked.game.state.land.recovery.repair_plan).unwrap(),
        serde_json::to_value(&allowed.game.state.land.recovery.repair_plan).unwrap()
    );
    assert_eq!(
        Cna::dev().observe(&c, &blocked.game.state, perspective),
        Cna::dev().observe(&c, &allowed.game.state, perspective)
    );

    // Production finish dispatch resolves hidden truth only after the same answer
    // was accepted. Keep the cursor here to isolate this closure's exact effects.
    let mut blocked = blocked.game;
    let physical = serde_json::to_value((
        &blocked.state.land.units,
        &blocked.state.land.breakdown,
        &blocked.state.logistics,
    ))
    .unwrap();
    let mut rng = CampaignRng::from_state(&blocked.rng);
    let mut events = vec![];
    Cna::dev()
        .finish_step(
            &c,
            &mut blocked.state,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
    assert_eq!(rng.state(), blocked.rng);
    assert_eq!(
        serde_json::to_value((
            &blocked.state.land.units,
            &blocked.state.land.breakdown,
            &blocked.state.logistics
        ))
        .unwrap(),
        physical
    );
    assert!(blocked.state.land.replacement_receipts.lots.is_empty());
    assert!(blocked.state.land.recovery.resolved);
    assert!(blocked.state.land.recovery.repair_plan.is_none());
    assert_eq!(events.len(), 1);
    assert!(events[0].visible_to(perspective));
    assert!(!events[0].visible_to(Perspective::Side(Side::Commonwealth)));
    assert!(matches!(&events[0].event, GameEvent::Note { text }
        if text == "Field repair unavailable at own marker field-fixture; no attempt or payment made."));

    let mut allowed = allowed.game;
    let mut rng = CampaignRng::from_state(&allowed.rng);
    let mut events = vec![];
    Cna::dev()
        .finish_step(
            &c,
            &mut allowed.state,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
    assert_ne!(rng.state(), allowed.rng);
    assert_eq!(allowed.state.logistics.dumps["ax_dump_1"].supplies.fuel, 16);
    assert_eq!(
        allowed
            .state
            .land
            .replacement_receipts
            .lots
            .values()
            .map(|lot| lot.points)
            .sum::<i32>(),
        1
    );
    assert!(allowed.state.land.recovery.resolved);
}
