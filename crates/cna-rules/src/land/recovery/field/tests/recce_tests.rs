//! Actual owner schema/Respond, group finish, receipt, rollback and privacy controls.
use super::*;

fn source(
    c: &CnaContent,
    side: Side,
    different: Option<&str>,
    points: i32,
) -> cna_core::ids::UnitId {
    c.units
        .units
        .values()
        .find(|unit| {
            unit.side == side
                && unit.class.as_deref() != different
                && unit
                    .class
                    .as_ref()
                    .and_then(|id| c.units.classes.get(id))
                    .is_some_and(|class| {
                        class.unit_type == "recce"
                            && class.cpa >= 25
                            && class.max_toe.is_some_and(|max| max >= points)
                    })
                && (side != Side::Axis || unit.nationality == "italian")
        })
        .expect("real Graziani recce source class")
        .id
        .clone()
}
fn recce_fixture(points: i32, roll: u8, captured: bool) -> (CnaContent, Game<Cna>) {
    let (c, mut game) = fixture(points, roll, captured);
    let side = if captured {
        Side::Commonwealth
    } else {
        Side::Axis
    };
    let unit = source(&c, side, None, points);
    let marker = game.state.land.breakdown.markers.get_mut(MARKER).unwrap();
    marker.assets[0].unit = unit.clone();
    marker.assets[0].equipment = Equipment::ArmoredRecce;
    for dump in game.state.logistics.dumps.values_mut() {
        dump.supplies = Supplies::default();
    }
    game.state.logistics.fuel_accounts.insert(
        unit,
        crate::logistics::FuelFundingAccount {
            side,
            segment: crate::logistics::SegmentKey {
                game_turn: game.state.cursor.game_turn,
                op_stage: game.state.cursor.op_stage,
                half: game.state.cursor.half,
                cycle: game.state.cursor.cycle,
            },
            origin: Location::Hex {
                hex: "C4807".into(),
            },
            paid_cost: FuelTenths::new(30),
            draws: vec![],
        },
    );
    (c, game)
}
fn second(c: &CnaContent, game: &mut Game<Cna>, captured: bool, different_class: bool) {
    let mut marker = game.state.land.breakdown.markers[MARKER].clone();
    marker.id = "recce-second".into();
    let original = crate::land::formation::class(c, &marker.assets[0].unit).unwrap();
    marker.assets[0].unit = source(
        c,
        if captured {
            Side::Commonwealth
        } else {
            Side::Axis
        },
        different_class.then_some(original.id.as_str()),
        1,
    );
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

/// Cases: land:22.12, land:22.24, land:22.5
#[test]
fn recce_schema_respond_buffers_and_advance_returns_verified_class_receipt_without_supply() {
    let (c, game) = recce_fixture(3, 1, false);
    let original = game.state.land.breakdown.markers[MARKER].assets[0]
        .unit
        .clone();
    let class = crate::land::formation::class(&c, &original)
        .unwrap()
        .id
        .clone();
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
            &accepted.state.logistics
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
    assert_eq!(
        prepared.repair.site_debits()[0].demand(),
        SupplyDemand::default()
    );
    assert!(prepared.repair.site_debits()[0].draws().is_empty());
    let transition = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
    assert_eq!(
        transition.game.state.land.breakdown.markers[MARKER].assets[0].points,
        2
    );
    let receipts: Vec<_> = transition
        .game
        .state
        .land
        .replacement_receipts
        .lots
        .values()
        .collect();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].points, 1);
    assert_eq!(receipts[0].owner, Side::Axis);
    assert_eq!(
        receipts[0].site,
        Location::Hex {
            hex: "C4807".into()
        }
    );
    assert_eq!(
        receipts[0].equipment,
        receipts::ReplacementEquipment::RecceAc { equipment: class }
    );
    assert_eq!(transition.game.state.land.units, accepted.state.land.units);
    assert_eq!(
        serde_json::to_value(&transition.game.state.logistics).unwrap(),
        serde_json::to_value(&accepted.state.logistics).unwrap()
    );
}

/// Cases: land:22.24, land:22.5
#[test]
fn recce_full_single_point_removes_marker_and_allocates_real_monotonic_serial() {
    let (c, game) = recce_fixture(1, 1, false);
    let mut game = respond(
        &c,
        &opened(&c, &game),
        json!([{"marker":MARKER,"points":1}]),
    );
    game.state
        .land
        .replacement_receipts
        .next_serial
        .insert(Side::Axis, 7);
    let before = game.state.logistics.clone();
    let rng = one_draw(&game);
    finish_step(&c, &mut game).unwrap();
    assert_eq!(game.rng, rng);
    assert!(!game.state.land.breakdown.markers.contains_key(MARKER));
    assert!(
        game.state
            .land
            .replacement_receipts
            .lots
            .contains_key("axis.repair-receipt-8")
    );
    assert_eq!(
        game.state.land.replacement_receipts.next_serial[&Side::Axis],
        8
    );
    assert_eq!(
        serde_json::to_value(&game.state.logistics).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
}

/// Cases: land:22.24
#[test]
fn recce_all_other_die_faces_keep_exact_body_no_receipt_no_debit_and_one_draw() {
    for face in 2..=6 {
        let (c, game) = recce_fixture(3, face, false);
        let mut game = respond(
            &c,
            &opened(&c, &game),
            json!([{"marker":MARKER,"points":3}]),
        );
        let before = serde_json::to_value((
            &game.state.land.breakdown,
            &game.state.logistics,
            &game.state.land.replacement_receipts,
        ))
        .unwrap();
        let rng = one_draw(&game);
        finish_step(&c, &mut game).unwrap();
        assert_eq!(game.rng, rng);
        assert_eq!(
            serde_json::to_value((
                &game.state.land.breakdown,
                &game.state.logistics,
                &game.state.land.replacement_receipts
            ))
            .unwrap(),
            before
        );
    }
}

/// Cases: land:22.14, land:22.24, land:22.5
#[test]
fn recce_two_marker_types_share_one_die_and_predice_list_priority_not_one_roll_each() {
    for reverse in [false, true] {
        let (c, mut game) = recce_fixture(2, 1, false);
        second(&c, &mut game, false, true);
        let left = crate::land::formation::class(
            &c,
            &game.state.land.breakdown.markers[MARKER].assets[0].unit,
        )
        .unwrap()
        .id
        .clone();
        let right = crate::land::formation::class(
            &c,
            &game.state.land.breakdown.markers["recce-second"].assets[0].unit,
        )
        .unwrap()
        .id
        .clone();
        assert_ne!(left, right);
        let order = if reverse {
            ["recce-second", MARKER]
        } else {
            [MARKER, "recce-second"]
        };
        let mut game = respond(
            &c,
            &opened(&c, &game),
            json!([
                {"marker":order[0],"points":1}, {"marker":order[1],"points":1}
            ]),
        );
        let rng = one_draw(&game);
        let events = finish_step(&c, &mut game).unwrap();
        assert_eq!(game.rng, rng);
        if reverse {
            assert!(
                !game
                    .state
                    .land
                    .breakdown
                    .markers
                    .contains_key("recce-second")
            );
            assert_eq!(
                game.state.land.breakdown.markers[MARKER].assets[0].points,
                2
            );
        } else {
            assert_eq!(
                game.state.land.breakdown.markers[MARKER].assets[0].points,
                1
            );
            assert_eq!(
                game.state.land.breakdown.markers["recce-second"].assets[0].points,
                1
            );
        }
        assert_eq!(game.state.land.replacement_receipts.lots.len(), 1);
        let receipt = game
            .state
            .land
            .replacement_receipts
            .lots
            .values()
            .next()
            .unwrap();
        assert_eq!(
            receipt.equipment,
            receipts::ReplacementEquipment::RecceAc {
                equipment: if reverse { right } else { left }
            }
        );
        assert_eq!(events.len(), 1);
    }
}

/// Cases: land:22.14, land:22.24
#[test]
fn recce_nationality_or_captured_groups_and_mixed_families_never_silently_merge() {
    for variant in 0..3 {
        let (mut c, mut game) = recce_fixture(2, 1, false);
        second(&c, &mut game, variant == 0, true);
        let unit = game.state.land.breakdown.markers["recce-second"].assets[0]
            .unit
            .clone();
        if variant == 1 {
            c.units.units.get_mut(&unit).unwrap().nationality = "german".into();
        }
        if variant == 2 {
            game.state
                .land
                .breakdown
                .markers
                .get_mut("recce-second")
                .unwrap()
                .assets[0]
                .equipment = Equipment::MediumTruck;
        }
        let game = opened(&c, &game);
        let before = serde_json::to_value(&game).unwrap();
        let action = json!([{"marker":MARKER,"points":1},{"marker":"recce-second","points":1}]);
        assert!(matches!(
            evaluate(&Cna::dev(), &c, &game, &command(&c, &game, action)),
            Err(Rejection::Engine(EngineError::Unsupported { .. }))
        ));
        assert_eq!(serde_json::to_value(&game).unwrap(), before);
    }
}

/// Cases: land:22.24, land:22.5
#[test]
fn recce_class_and_ancillary_gaps_preserve_whole_game_before_dice() {
    for variant in 0..5 {
        let (mut c, mut game) = recce_fixture(2, 1, false);
        let marker = game.state.land.breakdown.markers.get_mut(MARKER).unwrap();
        match variant {
            0 => {
                c.units.units.get_mut(&marker.assets[0].unit).unwrap().class = None;
            }
            1 => marker.tank_fuel = FuelTenths::new(1),
            2 => {
                marker.passengers.insert(marker.assets[0].unit.clone(), 1);
            }
            3 => marker.assets[0].cohort = Some("unresolved-recce-body".into()),
            _ => marker.activity_water = cna_core::quantity::WaterPoints::new(1),
        }
        let game = opened(&c, &game);
        let before = serde_json::to_value(&game).unwrap();
        assert!(matches!(
            evaluate(
                &Cna::dev(),
                &c,
                &game,
                &command(&c, &game, json!([{"marker":MARKER,"points":1}]))
            ),
            Err(Rejection::Engine(EngineError::Unsupported { .. }))
        ));
        assert_eq!(serde_json::to_value(&game).unwrap(), before);
    }
}

/// Cases: land:22.12, land:22.24, land:22.5
#[test]
fn recce_answered_checkpoint_replays_and_resolved_reentry_never_rolls_or_returns_twice() {
    let (c, game) = recce_fixture(2, 1, false);
    let game = respond(
        &c,
        &opened(&c, &game),
        json!([{"marker":MARKER,"points":2}]),
    );
    let replay: Game<Cna> = serde_json::from_value(serde_json::to_value(&game).unwrap()).unwrap();
    let first = evaluate(&Cna::dev(), &c, &game, &Command::Advance).unwrap();
    let again = evaluate(&Cna::dev(), &c, &replay, &Command::Advance).unwrap();
    assert_eq!(
        serde_json::to_value(&first.game).unwrap(),
        serde_json::to_value(&again.game).unwrap()
    );
    assert_eq!(first.events, again.events);
    let mut game = game;
    finish_step(&c, &mut game).unwrap();
    let before = serde_json::to_value(&game).unwrap();
    assert!(finish_step(&c, &mut game).unwrap().is_empty());
    let mut rng = CampaignRng::from_state(&game.rng);
    let mut events = vec![];
    super::super::enter(
        &mut game.state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert!(events.is_empty());
    assert_eq!(rng.state(), game.rng);
    assert_eq!(serde_json::to_value(&game).unwrap(), before);
}

/// Cases: land:22.24, land:22.5
#[test]
fn recce_receipt_overflow_after_die_and_physical_write_rolls_back_existing_event_prefix() {
    let (c, game) = recce_fixture(1, 1, false);
    let mut game = respond(
        &c,
        &opened(&c, &game),
        json!([{"marker":MARKER,"points":1}]),
    );
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
            text: "prior event".into(),
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
/// Interpretations: interp:land-0052
#[test]
fn recce_captured_private_body_pairs_accept_same_response_and_have_same_public_outcome() {
    let (c, left) = recce_fixture(2, 1, true);
    let (_, right) = recce_fixture(4, 1, true);
    let left = opened(&c, &left);
    let right = opened(&c, &right);
    let perspective = Perspective::Side(Side::Commonwealth);
    assert_eq!(
        Cna::dev().observe(&c, &left.state, perspective),
        Cna::dev().observe(&c, &right.state, perspective)
    );
    assert_eq!(
        serde_json::to_value(Cna::dev().pending(&c, &left.state)).unwrap(),
        serde_json::to_value(Cna::dev().pending(&c, &right.state)).unwrap()
    );
    let action = command(&c, &left, json!([{"marker":MARKER,"points":1}]));
    let left = evaluate(&Cna::dev(), &c, &left, &action).unwrap();
    let right = evaluate(&Cna::dev(), &c, &right, &action).unwrap();
    let visible = |events: &[EngineEvent]| {
        events
            .iter()
            .filter(|event| event.visible_to(perspective))
            .cloned()
            .collect::<Vec<_>>()
    };
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
fn recce_hidden_control_does_not_change_schema_or_respond_acceptance_and_excludes_only_at_finish() {
    // This pair exercises the existing dev seat path on the source map, just
    // like the tank hidden-control pair; strict hexside completeness is separate.
    let finish = |game: &mut Game<Cna>, c: &CnaContent| {
        let mut rng = CampaignRng::from_state(&game.rng);
        let mut events = vec![];
        Cna::dev()
            .finish_step(
                c,
                &mut game.state,
                &mut Cx {
                    rng: &mut rng,
                    events: &mut events,
                },
            )
            .unwrap();
        game.rng = rng.state();
        events
    };
    let (c, mut controlled) = recce_fixture(2, 1, false);
    let enemies: Vec<cna_core::ids::UnitId> = [
        "cw.unassigned_inf.3rd_coldstream_guards",
        "cw.unassigned_inf.2nd_scots_guards",
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
                let unit = controlled.state.land.units.get_mut(id).unwrap();
                unit.location = Location::Hex {
                    hex: hex.id.clone(),
                };
                unit.toe = Some(cna_content::units::Toe::Under { under: 5 });
                unit.cohesion_quarters = 0;
            }
            zoc::controlled(&c, &controlled.state, Side::Commonwealth, &site, false)
                .unwrap_or(false)
        })
        .expect("real adjacent enemy control");
    for id in &enemies {
        assert_eq!(
            crate::land::formation::strength(&c, &controlled.state, id),
            5
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
    let seat = Perspective::Seat(SeatId::new(Side::Axis, Role::RearArea));
    let controlled = opened(&c, &controlled);
    let clear = opened(&c, &clear);
    assert_eq!(
        Cna::dev().observe(&c, &controlled.state, seat),
        Cna::dev().observe(&c, &clear.state, seat)
    );
    assert_eq!(
        serde_json::to_value(Cna::dev().pending(&c, &controlled.state)).unwrap(),
        serde_json::to_value(Cna::dev().pending(&c, &clear.state)).unwrap()
    );
    let action = command(&c, &controlled, json!([{"marker":MARKER,"points":2}]));
    let blocked = evaluate(&Cna::dev(), &c, &controlled, &action).unwrap();
    let allowed = evaluate(&Cna::dev(), &c, &clear, &action).unwrap();
    assert_eq!(blocked.events, allowed.events);
    assert_eq!(blocked.game.rng, controlled.rng);
    assert_eq!(allowed.game.rng, clear.rng);
    assert_eq!(
        serde_json::to_value(&blocked.game.state.land.recovery.repair_plan).unwrap(),
        serde_json::to_value(&allowed.game.state.land.recovery.repair_plan).unwrap()
    );
    let mut blocked = blocked.game;
    let mut allowed = allowed.game;
    let before =
        serde_json::to_value((&blocked.state.land.breakdown, &blocked.state.logistics)).unwrap();
    let rng = blocked.rng.clone();
    let hidden_events = finish(&mut blocked, &c);
    assert_eq!(blocked.rng, rng);
    assert_eq!(
        serde_json::to_value((&blocked.state.land.breakdown, &blocked.state.logistics)).unwrap(),
        before
    );
    assert!(blocked.state.land.replacement_receipts.lots.is_empty());
    assert!(
        hidden_events.iter().all(|event| event.visible_to(seat)
            && !event.visible_to(Perspective::Side(Side::Commonwealth)))
    );
    finish(&mut allowed, &c);
    assert_ne!(allowed.rng, clear.rng);
    assert_eq!(allowed.state.land.replacement_receipts.lots.len(), 1);
}
