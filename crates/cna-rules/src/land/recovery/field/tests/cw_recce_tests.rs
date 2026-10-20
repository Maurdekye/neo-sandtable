//! Commonwealth actual-flow controls; original nationality is content, not actor ownership.
use super::*;

const BRITISH: &str = "cw.7_armd_div.11th_hussars";
const INDIAN: &str = "cw.4_indian_div.central_india_horse";
const SECOND: &str = "cw-recce-second";

fn cw_fixture(points: i32, face: u8) -> (CnaContent, Game<Cna>) {
    let (c, mut game) = fixture(points, face, false);
    let source = c.units.units.get(&BRITISH.into()).unwrap();
    assert_eq!(source.side, Side::Commonwealth);
    assert_eq!(source.nationality, "british");
    assert_eq!(
        crate::land::formation::class(&c, &source.id)
            .unwrap()
            .unit_type,
        "recce"
    );
    let anchor = game
        .state
        .land
        .units
        .values()
        .find(|u| u.side == Side::Commonwealth)
        .unwrap()
        .id
        .clone();
    for unit in game.state.land.units.values_mut() {
        unit.location = Location::Eliminated;
    }
    game.state.land.units.get_mut(&anchor).unwrap().location = Location::Hex {
        hex: "C4807".into(),
    };
    game.state.cursor.half = Some(Half::A);
    let marker = game.state.land.breakdown.markers.get_mut(MARKER).unwrap();
    marker.side = Side::Commonwealth;
    marker.assets[0].unit = source.id.clone();
    marker.assets[0].equipment = Equipment::ArmoredRecce;
    for dump in game.state.logistics.dumps.values_mut() {
        dump.supplies = Supplies::default();
    }
    (c, game)
}

fn add_indian(c: &CnaContent, game: &mut Game<Cna>) {
    let source = c.units.units.get(&INDIAN.into()).unwrap();
    assert_eq!(source.side, Side::Commonwealth);
    assert_eq!(source.nationality, "indian");
    assert_eq!(
        crate::land::formation::class(c, &source.id)
            .unwrap()
            .unit_type,
        "recce"
    );
    let mut marker = game.state.land.breakdown.markers[MARKER].clone();
    marker.id = SECOND.into();
    marker.assets[0].unit = source.id.clone();
    marker.assets[0].points = 1;
    game.state
        .land
        .breakdown
        .markers
        .insert(marker.id.clone(), marker);
}

fn respond(c: &CnaContent, game: &Game<Cna>, rows: Value) -> Game<Cna> {
    let request = Cna::dev()
        .pending(c, &game.state)
        .into_iter()
        .find(|r| r.kind == KIND)
        .unwrap();
    assert_eq!(
        request.seat,
        SeatId::new(Side::Commonwealth, Role::RearArea)
    );
    request.space.check(&rows).unwrap();
    evaluate(&Cna::dev(), c, game, &command(c, game, rows))
        .unwrap()
        .game
}

fn one_draw(game: &Game<Cna>) -> cna_core::dice::RngState {
    let mut rng = CampaignRng::from_state(&game.rng);
    rng.d6();
    rng.state()
}

fn italian_source(c: &CnaContent, points: i32) -> cna_core::ids::UnitId {
    c.units
        .units
        .values()
        .find(|u| {
            u.side == Side::Axis
                && u.nationality == "italian"
                && crate::land::formation::class(c, &u.id).is_some_and(|cl| {
                    cl.unit_type == "recce"
                        && cl.cpa >= 25
                        && cl.max_toe.is_some_and(|max| max >= points)
                })
        })
        .unwrap()
        .id
        .clone()
}

/// Cases: land:22.12, land:22.14, land:22.24, land:22.5
#[test]
fn cw_recce_schema_respond_and_advance_return_partial_exact_receipt_at_zero_cost() {
    let (c, game) = cw_fixture(3, 1);
    let game = opened(&c, &game);
    let physical = serde_json::to_value((
        &game.state.land.units,
        &game.state.land.breakdown,
        &game.state.logistics,
    ))
    .unwrap();
    let accepted = respond(&c, &game, json!([{"marker":MARKER,"points":2}]));
    assert_eq!(accepted.rng, game.rng);
    assert_eq!(
        serde_json::to_value((
            &accepted.state.land.units,
            &accepted.state.land.breakdown,
            &accepted.state.logistics,
        ))
        .unwrap(),
        physical
    );
    let prepared = prepare(
        &c,
        &accepted.state,
        accepted.state.land.recovery.repair_plan.as_ref().unwrap(),
        false,
        false,
    )
    .unwrap();
    assert_eq!(prepared.repair.attempts().len(), 1);
    assert_eq!(
        prepared.repair.attempts()[0].vehicle(),
        RepairVehicle::Recce
    );
    assert_eq!(
        prepared.repair.attempts()[0].demand(),
        SupplyDemand::default()
    );
    assert_eq!(prepared.repair.site_debits().len(), 1);
    assert_eq!(
        prepared.repair.site_debits()[0].demand(),
        SupplyDemand::default()
    );
    assert!(prepared.repair.site_debits()[0].draws().is_empty());
    let expected_rng = one_draw(&accepted);
    let result = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
    assert_eq!(result.game.rng, expected_rng);
    let mut expected_marker = accepted.state.land.breakdown.markers[MARKER].clone();
    expected_marker.assets[0].points -= 1;
    assert_eq!(
        serde_json::to_value(&result.game.state.land.breakdown.markers[MARKER]).unwrap(),
        serde_json::to_value(expected_marker).unwrap()
    );
    assert_eq!(result.game.state.land.replacement_receipts.lots.len(), 1);
    let lot = result
        .game
        .state
        .land
        .replacement_receipts
        .lots
        .values()
        .next()
        .unwrap();
    assert_eq!(lot.owner, Side::Commonwealth);
    assert_eq!(lot.points, 1);
    assert_eq!(
        lot.site,
        Location::Hex {
            hex: "C4807".into()
        }
    );
    assert_eq!(
        lot.equipment,
        receipts::ReplacementEquipment::RecceAc {
            equipment: "cw.ll".into()
        }
    );
    assert_eq!(result.game.state.land.units, accepted.state.land.units);
    assert_eq!(
        serde_json::to_value(&result.game.state.logistics).unwrap(),
        serde_json::to_value(&accepted.state.logistics).unwrap()
    );
    assert_eq!(c.units.units[&BRITISH.into()].nationality, "british");
}

/// Cases: land:22.24, land:22.62
#[test]
fn cw_recce_faces_two_through_six_fail_without_tank_modifier_or_supply_debit() {
    for face in 2..=6 {
        let (c, game) = cw_fixture(3, face);
        let accepted = respond(
            &c,
            &opened(&c, &game),
            json!([{"marker":MARKER,"points":3}]),
        );
        let before = serde_json::to_value((
            &accepted.state.land.breakdown,
            &accepted.state.land.replacement_receipts,
            &accepted.state.logistics,
            &accepted.state.land.units,
        ))
        .unwrap();
        let expected_rng = one_draw(&accepted);
        let result = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
        assert_eq!(result.game.rng, expected_rng);
        assert_eq!(
            serde_json::to_value((
                &result.game.state.land.breakdown,
                &result.game.state.land.replacement_receipts,
                &result.game.state.logistics,
                &result.game.state.land.units,
            ))
            .unwrap(),
            before
        );
    }
}

/// Cases: land:22.24, land:22.5
#[test]
fn cw_recce_full_return_uses_commonwealth_serial_without_touching_other_side_receipts() {
    let (c, mut game) = cw_fixture(1, 1);
    game.state
        .land
        .replacement_receipts
        .next_serial
        .insert(Side::Axis, 9);
    game.state
        .land
        .replacement_receipts
        .next_serial
        .insert(Side::Commonwealth, 6);
    let mut game = respond(
        &c,
        &opened(&c, &game),
        json!([{"marker":MARKER,"points":1}]),
    );
    let logistics = serde_json::to_value(&game.state.logistics).unwrap();
    let rng = one_draw(&game);
    let events = finish_step(&c, &mut game).unwrap();
    assert_eq!(game.rng, rng);
    assert!(!game.state.land.breakdown.markers.contains_key(MARKER));
    assert_eq!(
        game.state.land.replacement_receipts.next_serial[&Side::Commonwealth],
        7
    );
    assert_eq!(
        game.state.land.replacement_receipts.next_serial[&Side::Axis],
        9
    );
    let lot = &game.state.land.replacement_receipts.lots["commonwealth.repair-receipt-7"];
    assert_eq!(lot.points, 1);
    assert_eq!(lot.owner, Side::Commonwealth);
    assert_eq!(
        serde_json::to_value(&game.state.logistics).unwrap(),
        logistics
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, GameEvent::Note { .. }))
    );
    assert!(
        events
            .iter()
            .filter(|e| matches!(&e.event, GameEvent::Note { .. }))
            .all(|e| e.visible_to(Perspective::Side(Side::Commonwealth))
                && !e.visible_to(Perspective::Side(Side::Axis)))
    );
}

/// Cases: land:22.14, land:22.24, land:22.5
#[test]
fn cw_recce_british_and_indian_sources_share_one_die_and_preserve_original_provenance() {
    for reverse in [false, true] {
        let (c, mut game) = cw_fixture(2, 1);
        add_indian(&c, &mut game);
        let before = game.state.land.breakdown.markers.clone();
        let order = if reverse {
            [SECOND, MARKER]
        } else {
            [MARKER, SECOND]
        };
        let mut game = respond(
            &c,
            &opened(&c, &game),
            json!([
                {"marker":order[0],"points":1}, {"marker":order[1],"points":1},
            ]),
        );
        let rng = one_draw(&game);
        let events = finish_step(&c, &mut game).unwrap();
        assert_eq!(game.rng, rng);
        let mut expected = before;
        let chosen = expected.get_mut(order[0]).unwrap();
        chosen.assets[0].points -= 1;
        if chosen.assets[0].points == 0 {
            expected.remove(order[0]);
        }
        assert_eq!(
            serde_json::to_value(&game.state.land.breakdown.markers).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        assert_eq!(game.state.land.replacement_receipts.lots.len(), 1);
        assert_eq!(
            game.state
                .land
                .replacement_receipts
                .lots
                .values()
                .next()
                .unwrap()
                .points,
            1
        );
        assert_eq!(events.len(), 1);
        assert_eq!(c.units.units[&BRITISH.into()].nationality, "british");
        assert_eq!(c.units.units[&INDIAN.into()].nationality, "indian");
    }
}

/// Cases: land:22.11, land:22.14, land:22.24
#[test]
fn cw_recce_captured_italian_stays_separate_but_its_own_group_returns_actual_class() {
    let (c, mut game) = cw_fixture(2, 1);
    let captured = italian_source(&c, 1);
    let class = crate::land::formation::class(&c, &captured)
        .unwrap()
        .id
        .clone();
    add_indian(&c, &mut game);
    game.state
        .land
        .breakdown
        .markers
        .get_mut(SECOND)
        .unwrap()
        .assets[0]
        .unit = captured.clone();
    let opened_game = opened(&c, &game);
    let before = serde_json::to_value(&opened_game).unwrap();
    let combined = command(
        &c,
        &opened_game,
        json!([
            {"marker":MARKER,"points":1}, {"marker":SECOND,"points":1},
        ]),
    );
    assert!(matches!(evaluate(&Cna::dev(), &c, &opened_game, &combined),
        Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "land:22.14"));
    assert_eq!(serde_json::to_value(&opened_game).unwrap(), before);
    let mut accepted = respond(&c, &opened_game, json!([{"marker":SECOND,"points":1}]));
    let rng = one_draw(&accepted);
    finish_step(&c, &mut accepted).unwrap();
    assert_eq!(accepted.rng, rng);
    assert!(!accepted.state.land.breakdown.markers.contains_key(SECOND));
    assert_eq!(
        accepted.state.land.breakdown.markers[MARKER].assets[0].points,
        2
    );
    assert_eq!(accepted.state.land.replacement_receipts.lots.len(), 1);
    assert_eq!(
        accepted
            .state
            .land
            .replacement_receipts
            .lots
            .values()
            .next()
            .unwrap()
            .equipment,
        receipts::ReplacementEquipment::RecceAc { equipment: class }
    );
    assert_eq!(c.units.units[&captured].side, Side::Axis);
    assert_eq!(c.units.units[&captured].nationality, "italian");
}

/// Cases: land:22.23, land:22.24, land:22.25, land:22.62
#[test]
fn cw_trucks_are_admitted_while_tank_guard_and_pass_invariants_remain() {
    for truck in [false, true] {
        let (c, mut game) = cw_fixture(2, 1);
        let marker = game.state.land.breakdown.markers.get_mut(MARKER).unwrap();
        if truck {
            marker.assets[0].equipment = Equipment::MediumTruck;
        } else {
            marker.assets[0].unit = c
                .units
                .units
                .values()
                .find(|u| {
                    u.side == Side::Commonwealth
                        && crate::land::formation::class(&c, &u.id)
                            .is_some_and(|cl| cl.unit_type == "tank")
                })
                .unwrap()
                .id
                .clone();
            marker.assets[0].equipment = Equipment::Weapon(
                c.units
                    .weapons
                    .values()
                    .find(|w| w.nation == "cw" && w.kind == "tank")
                    .unwrap()
                    .id
                    .clone(),
            );
        }
        let game = opened(&c, &game);
        let before = serde_json::to_value(&game).unwrap();
        if truck {
            // The formerly unsupported exact body is now within land:22.23.
            // Retain the actual tank refusal and the shared pass/no-write controls.
            let mut accepted = respond(&c, &game, json!([{"marker":MARKER,"points":1}]));
            assert_eq!(accepted.rng, game.rng);
            assert_eq!(
                serde_json::to_value(&accepted.state.logistics).unwrap(),
                serde_json::to_value(&game.state.logistics).unwrap()
            );
            let before_pools = accepted.state.logistics.truck_pool_ids.clone();
            let expected_rng = one_draw(&accepted);
            finish_step(&c, &mut accepted).unwrap();
            assert_eq!(accepted.rng, expected_rng);
            assert_eq!(
                accepted.state.land.breakdown.markers[MARKER].assets[0].points,
                1
            );
            let pools: Vec<_> = accepted
                .state
                .logistics
                .truck_pools
                .iter()
                .filter(|pool| !before_pools.contains(&pool.id))
                .collect();
            assert_eq!(pools.len(), 1);
            assert_eq!(pools[0].side, Side::Commonwealth);
            assert_eq!(pools[0].trucks.medium, 1);
            assert_eq!(pools[0].tank_fuel.get(), 0);
            assert!(accepted.state.land.replacement_receipts.lots.is_empty());
        } else {
            assert!(matches!(evaluate(&Cna::dev(), &c, &game,
                &command(&c, &game, json!([{"marker":MARKER,"points":1}]))),
                Err(Rejection::Engine(EngineError::Unsupported { case, .. }))
                    if case == "land:22.6"));
        }
        assert_eq!(serde_json::to_value(&game).unwrap(), before);
        let mut pass = respond(&c, &game, Value::Null);
        let rng = pass.rng.clone();
        let physical =
            serde_json::to_value((&pass.state.land.breakdown, &pass.state.logistics)).unwrap();
        assert!(finish_step(&c, &mut pass).unwrap().is_empty());
        assert_eq!(pass.rng, rng);
        assert_eq!(
            serde_json::to_value((&pass.state.land.breakdown, &pass.state.logistics)).unwrap(),
            physical
        );
    }
}

/// Cases: land:22.14, land:22.24, land:22.5
#[test]
fn cw_recce_unresolved_class_nationality_or_ancillary_partition_remains_explicit_and_atomic() {
    for variant in 0..6 {
        let (mut c, mut game) = cw_fixture(2, 1);
        let marker = game.state.land.breakdown.markers.get_mut(MARKER).unwrap();
        let case = match variant {
            0 => {
                c.units.units.get_mut(&marker.assets[0].unit).unwrap().class = None;
                "land:22.24"
            }
            1 => {
                c.units
                    .units
                    .get_mut(&marker.assets[0].unit)
                    .unwrap()
                    .nationality
                    .clear();
                "land:22.14"
            }
            2 => {
                marker.tank_fuel = FuelTenths::new(1);
                "land:22.5"
            }
            3 => {
                marker.assets[0].cohort = Some("unpartitioned".into());
                "land:22.5"
            }
            4 => {
                marker.passengers.insert(marker.assets[0].unit.clone(), 1);
                "land:22.5"
            }
            _ => {
                marker.assets.push(marker.assets[0].clone());
                "land:22.25"
            }
        };
        let game = opened(&c, &game);
        let before = serde_json::to_value(&game).unwrap();
        assert!(matches!(evaluate(&Cna::dev(), &c, &game,
            &command(&c, &game, json!([{"marker":MARKER,"points":1}]))),
            Err(Rejection::Engine(EngineError::Unsupported { case: actual, .. })) if actual == case));
        assert_eq!(serde_json::to_value(&game).unwrap(), before);
    }
}

/// Cases: land:22.12, land:22.24, land:22.5
#[test]
fn cw_recce_checkpoint_and_resolved_reentry_keep_receipt_serial_and_roll_exactly_once() {
    let (c, game) = cw_fixture(2, 1);
    let accepted = respond(
        &c,
        &opened(&c, &game),
        json!([{"marker":MARKER,"points":2}]),
    );
    let replay: Game<Cna> =
        serde_json::from_value(serde_json::to_value(&accepted).unwrap()).unwrap();
    let first = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
    let again = evaluate(&Cna::dev(), &c, &replay, &Command::Advance).unwrap();
    assert_eq!(
        serde_json::to_value(&first.game).unwrap(),
        serde_json::to_value(&again.game).unwrap()
    );
    assert_eq!(first.events, again.events);
    let mut accepted = accepted;
    finish_step(&c, &mut accepted).unwrap();
    let before = serde_json::to_value(&accepted).unwrap();
    assert!(finish_step(&c, &mut accepted).unwrap().is_empty());
    let mut rng = CampaignRng::from_state(&accepted.rng);
    let mut events = vec![];
    super::super::enter(
        &mut accepted.state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert!(events.is_empty());
    assert_eq!(rng.state(), accepted.rng);
    assert_eq!(serde_json::to_value(&accepted).unwrap(), before);
}

/// Cases: land:22.24, land:22.5
#[test]
fn cw_recce_late_receipt_failure_rolls_back_post_die_marker_write_and_existing_event_prefix() {
    let (c, game) = cw_fixture(1, 1);
    let mut game = respond(
        &c,
        &opened(&c, &game),
        json!([{"marker":MARKER,"points":1}]),
    );
    game.state
        .land
        .replacement_receipts
        .next_serial
        .insert(Side::Commonwealth, u64::MAX);
    let before = serde_json::to_value(&game).unwrap();
    let mut rng = CampaignRng::from_state(&game.rng);
    let rng_before = rng.state();
    let mut events = vec![EngineEvent::new(
        Audience::Side(Side::Axis),
        GameEvent::Note {
            text: "existing event prefix".into(),
        },
    )];
    let prefix = events.clone();
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
    assert_eq!(rng.state(), rng_before);
    assert_eq!(events, prefix);
    assert_eq!(serde_json::to_value(&game).unwrap(), before);
}

/// Cases: land:3.62, land:22.11, land:22.24, land:22.5
/// Interpretations: interp:land-0038, interp:land-0052
#[test]
fn cw_recce_captured_private_body_pairs_keep_axis_response_and_advance_streams_equal() {
    let (c, mut left) = cw_fixture(2, 1);
    let (_, mut right) = cw_fixture(3, 1);
    let original = italian_source(&c, 3);
    for game in [&mut left, &mut right] {
        game.state
            .land
            .breakdown
            .markers
            .get_mut(MARKER)
            .unwrap()
            .assets[0]
            .unit = original.clone();
    }
    let left = opened(&c, &left);
    let right = opened(&c, &right);
    let perspective = Perspective::Side(Side::Axis);
    assert_eq!(
        Cna::dev().observe(&c, &left.state, perspective),
        Cna::dev().observe(&c, &right.state, perspective)
    );
    assert_eq!(
        serde_json::to_value(Cna::dev().pending(&c, &left.state)).unwrap(),
        serde_json::to_value(Cna::dev().pending(&c, &right.state)).unwrap()
    );
    let visible = |events: &[EngineEvent]| {
        events
            .iter()
            .filter(|e| e.visible_to(perspective))
            .cloned()
            .collect::<Vec<_>>()
    };
    let action = command(&c, &left, json!([{"marker":MARKER,"points":1}]));
    let left = evaluate(&Cna::dev(), &c, &left, &action).unwrap();
    let right = evaluate(&Cna::dev(), &c, &right, &action).unwrap();
    assert_eq!(visible(&left.events), visible(&right.events));
    let left = evaluate(&Cna::dev(), &c, &left.game, &Command::Advance).unwrap();
    let right = evaluate(&Cna::dev(), &c, &right.game, &Command::Advance).unwrap();
    assert_eq!(visible(&left.events), visible(&right.events));
    assert_eq!(
        Cna::dev().observe(&c, &left.game.state, perspective),
        Cna::dev().observe(&c, &right.game.state, perspective)
    );
    assert_ne!(
        left.game.state.land.breakdown.markers[MARKER].assets[0].points,
        right.game.state.land.breakdown.markers[MARKER].assets[0].points
    );
    assert_eq!(left.game.state.land.replacement_receipts.lots.len(), 1);
    assert_eq!(right.game.state.land.replacement_receipts.lots.len(), 1);
}

/// Cases: land:3.61, land:10.15, land:22.13, land:22.24
#[test]
fn cw_recce_hidden_axis_control_changes_only_finish_not_owner_response_or_schema() {
    // Match the established dev hidden-control fixture's map-coverage contract.
    let (c, mut blocked) = cw_fixture(2, 1);
    let enemies: Vec<cna_core::ids::UnitId> = [
        "it.1_libyan_div.viii_libyan_bn",
        "it.1_libyan_div.ix_libyan_bn",
        "it.1_libyan_div.x_libyan_bn",
        "it.1_libyan_div.xi_libyan_bn",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    let site: HexId = "C4807".into();
    c.map
        .neighbors(&site)
        .into_iter()
        .find(|hex| {
            for id in &enemies {
                let unit = blocked.state.land.units.get_mut(id).unwrap();
                unit.location = Location::Hex {
                    hex: hex.id.clone(),
                };
                unit.detached = true;
                // These battalions have a printed maximum of five. Under(5)
                // is invalid at that maximum; Normal is the real full body.
                unit.toe = Some(cna_content::units::Toe::Normal(
                    cna_content::units::NormalToe::N,
                ));
                unit.cohesion_quarters = 0;
            }
            zoc::controlled(&c, &blocked.state, Side::Axis, &site, false).unwrap_or(false)
        })
        .expect("real Axis infantry can control an adjacent repair site");
    for id in &enemies {
        assert_eq!(crate::land::formation::strength(&c, &blocked.state, id), 5);
    }
    let mut clear: Game<Cna> =
        serde_json::from_value(serde_json::to_value(&blocked).unwrap()).unwrap();
    for id in &enemies {
        clear
            .state
            .land
            .units
            .get_mut(id)
            .unwrap()
            .cohesion_quarters = -104;
    }
    assert!(zoc::controlled(&c, &blocked.state, Side::Axis, &site, false).unwrap());
    assert!(!zoc::controlled(&c, &clear.state, Side::Axis, &site, false).unwrap());
    let blocked = opened(&c, &blocked);
    let clear = opened(&c, &clear);
    let seat = Perspective::Seat(SeatId::new(Side::Commonwealth, Role::RearArea));
    assert_eq!(
        Cna::dev().observe(&c, &blocked.state, seat),
        Cna::dev().observe(&c, &clear.state, seat)
    );
    assert_eq!(
        serde_json::to_value(Cna::dev().pending(&c, &blocked.state)).unwrap(),
        serde_json::to_value(Cna::dev().pending(&c, &clear.state)).unwrap()
    );
    let action = command(&c, &blocked, json!([{"marker":MARKER,"points":2}]));
    let blocked = evaluate(&Cna::dev(), &c, &blocked, &action).unwrap();
    let clear = evaluate(&Cna::dev(), &c, &clear, &action).unwrap();
    assert_eq!(blocked.events, clear.events);
    assert_eq!(blocked.game.rng, clear.game.rng);
    assert_eq!(
        serde_json::to_value(&blocked.game.state.land.recovery.repair_plan).unwrap(),
        serde_json::to_value(&clear.game.state.land.recovery.repair_plan).unwrap()
    );
    let before = serde_json::to_value((
        &blocked.game.state.land.breakdown,
        &blocked.game.state.logistics,
    ))
    .unwrap();
    let rng = blocked.game.rng.clone();
    let expected_clear_rng = one_draw(&clear.game);
    let blocked = evaluate(&Cna::dev(), &c, &blocked.game, &Command::Advance).unwrap();
    let clear = evaluate(&Cna::dev(), &c, &clear.game, &Command::Advance).unwrap();
    assert_eq!(blocked.game.rng, rng);
    assert_eq!(
        serde_json::to_value((
            &blocked.game.state.land.breakdown,
            &blocked.game.state.logistics
        ))
        .unwrap(),
        before
    );
    assert!(blocked.game.state.land.replacement_receipts.lots.is_empty());
    assert!(
        blocked
            .events
            .iter()
            .filter(|e| matches!(&e.event, GameEvent::Note { .. }))
            .all(|e| e.visible_to(seat) && !e.visible_to(Perspective::Side(Side::Axis)))
    );
    assert_eq!(clear.game.rng, expected_clear_rng);
    assert_eq!(clear.game.state.land.replacement_receipts.lots.len(), 1);
}
