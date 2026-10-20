use super::*;
use crate::{
    Cna,
    air::{
        inventory, missions,
        preparation::{CurrentPreparation, InitialChoice},
    },
    testkit::{assert_action_indistinguishable, assert_indistinguishable},
};
use cna_core::{
    engine::{Command, Game, evaluate},
    visibility::Perspective,
};
use serde_json::{Value, json};
use std::sync::OnceLock;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}

// Reuse the existing genuinely authenticated public setup/designation fixture;
// only its cfg(test), Air-internal helper visibility changes, not any assertion.
fn opened() -> Game<Cna> {
    missions::initial::tests::opened()
}

fn plane(game: &Game<Cna>) -> (PlaneId, usize, HexId) {
    game.state
        .air
        .runtime
        .aircraft
        .iter()
        .find_map(|(id, p)| {
            if p.force != "axis"
                || p.squadron.as_ref().is_none_or(|s| {
                    crate::air::designation::squadron_family(&game.state, s)
                        != Some(crate::air::designation::Family::LandSupport)
                })
            {
                return None;
            }
            let source = &content().units.aircraft[&p.aircraft];
            if source.role != "fighter" {
                return None;
            }
            let facility = ground_facility(content(), &game.state, id).ok().flatten()?;
            let site = &game.state.air.runtime.facilities[&facility];
            let crate::state::Location::Hex { hex } =
                site.properties(content(), &facility).ok()?.location
            else {
                return None;
            };
            let mode = source.modes.iter().position(|m| {
                m.range_hexes.is_some_and(|r| r > 0)
                    && m.fuel_points.is_some()
                    && m.missions
                        .get("f")
                        .is_some_and(|v| matches!(v.as_str(), "day" | "night"))
            })?;
            Some((id.clone(), mode, hex))
        })
        .expect("actual assigned initial Axis fighter with source day/range/fuel/on-map base")
}

fn answered(game: &Game<Cna>) -> (Game<Cna>, PlaneId, Vec<HexId>) {
    answered_at(game, false)
}

fn answered_at(game: &Game<Cna>, same_base: bool) -> (Game<Cna>, PlaneId, Vec<HexId>) {
    let (id, mode, origin) = plane(game);
    let target = if same_base {
        origin.clone()
    } else {
        content()
            .map
            .neighbors(&origin)
            .first()
            .expect("source map neighbor")
            .id
            .clone()
    };
    let action = json!([{"mission":"strafe_grounded_aircraft","light":"day",
        "target":{"hex":target,"scramble_reserve":false},
        "planes":[{"plane":id,"mode":mode,"initial_choice":{"decision":"load","guns":true,"ordnance":{"kind":"none"}}}]}]);
    let one = evaluate(
        &Cna::dev(),
        content(),
        game,
        &missions::initial::tests::response(game, Side::Axis, action),
    )
    .unwrap()
    .game;
    let two = evaluate(
        &Cna::dev(),
        content(),
        &one,
        &missions::initial::tests::response(&one, Side::Commonwealth, Value::Null),
    )
    .unwrap()
    .game;
    (
        two,
        id,
        if same_base {
            vec![origin]
        } else {
            vec![origin, target]
        },
    )
}

fn staged() -> (Game<Cna>, PlaneId, BTreeMap<Side, Vec<RouteSelection>>) {
    staged_at(false)
}

// Explicit source-prerequisite fixture, NOT a maintenance or initial-refit
// entitlement. The real initial producer above leaves these aircraft unrefitted.
// Deployment consumes this independently staged ready state through its actual
// private component producer; no intervening campaign trajectory is claimed.
fn refit_component_input(state: &mut State) {
    let ids: Vec<_> = state
        .air
        .runtime
        .tactical
        .declarations
        .values()
        .flat_map(|m| m.planes.iter().map(|p| p.plane.clone()))
        .collect();
    assert!(
        ids.iter()
            .all(|id| !state.air.runtime.aircraft[id].refitted)
    );
    let preparation = state.air.runtime.preparation.clone();
    let history = state.air.runtime.maintenance.clone();
    inventory::update(content(), &mut state.air, |runtime| {
        for id in &ids {
            runtime.aircraft.get_mut(id).unwrap().refitted = true;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(state.air.runtime.preparation, preparation);
    assert_eq!(state.air.runtime.maintenance, history);
}

fn staged_at(same_base: bool) -> (Game<Cna>, PlaneId, BTreeMap<Side, Vec<RouteSelection>>) {
    let (mut game, id, path) = answered_at(&opened(), same_base);
    // Real declaration finish, then explicit component clock entry; this is NOT
    // a publicly enabled launch or a whole intervening campaign trajectory.
    missions::finish(content(), &mut game.state).unwrap();
    assert_eq!(
        game.state.air.runtime.preparation.initial_choices[&id],
        InitialChoice::Committed
    );
    assert!(matches!(
        game.state.air.runtime.preparation.current[&id],
        CurrentPreparation::KnownLoad { .. }
    ));
    refit_component_input(&mut game.state);
    let mission = game
        .state
        .air
        .runtime
        .tactical
        .declarations
        .iter()
        .find(|(_, m)| m.planes.iter().any(|p| p.plane == id))
        .unwrap()
        .0
        .clone();
    assert!(game.state.decisions.pending.is_empty());
    game.state.cursor.index = crate::seq::OPSTAGE
        .iter()
        .position(|s| s.anchor == DEPLOYMENT)
        .unwrap();
    game.state.cursor.entered = true;
    // Deterministic normal weather is a component fixture, not a claimed roll.
    game.state.turn.weather = Some(crate::state::WeatherState {
        kind: cna_tables::land::weather::WeatherKind::Normal,
        storm_sections: Vec::new(),
    });
    let routes = BTreeMap::from([
        (Side::Axis, vec![RouteSelection { mission, path }]),
        (Side::Commonwealth, Vec::new()),
    ]);
    (game, id, routes)
}

fn deployed() -> (Game<Cna>, PlaneId) {
    let (mut game, id, routes) = staged();
    let admission = prepare_deployment(content(), &game.state, &routes)
        .unwrap()
        .unwrap();
    inventory::update_flight(content(), &mut game.state, admission).unwrap();
    (game, id)
}

/// Cases: scen:59.32, airlog:37.13, airlog:37.16, airlog:38.31, airlog:39.12
/// Interpretations: interp:air-0018, interp:air-0020
#[test]
fn genuine_initial_respond_and_component_deployment_commit_exactly_once_without_payment_or_ammo_reset()
 {
    let (mut game, id, routes) = staged();
    let before = game.clone();
    let admission = prepare_deployment(content(), &game.state, &routes)
        .unwrap()
        .unwrap();
    inventory::update_flight(content(), &mut game.state, admission).unwrap();
    assert!(matches!(
        position(content(), &game.state, &id).unwrap(),
        Position::Airborne { .. }
    ));
    assert_eq!(ground_facility(content(), &game.state, &id).unwrap(), None);
    assert!(!game.state.air.runtime.aircraft[&id].fuelled);
    assert!(!game.state.air.runtime.aircraft[&id].refitted);
    assert_eq!(
        game.state.air.runtime.aircraft[&id].facility,
        before.state.air.runtime.aircraft[&id].facility
    );
    let mut expected_load = before.state.air.runtime.preparation.current[&id].clone();
    if let CurrentPreparation::KnownLoad { load } = &mut expected_load {
        load.fuel_available = false;
    } else {
        unreachable!();
    }
    assert_eq!(
        game.state.air.runtime.preparation.current[&id],
        expected_load
    );
    assert_eq!(
        game.state.air.runtime.preparation.initial_choices,
        before.state.air.runtime.preparation.initial_choices
    );
    assert_eq!(
        game.state.air.runtime.maintenance,
        before.state.air.runtime.maintenance
    );
    assert_eq!(
        serde_json::to_value(&game.state.logistics).unwrap(),
        serde_json::to_value(&before.state.logistics).unwrap()
    );
    assert_eq!(game.rng, before.rng);
    for (other, plane) in &before.state.air.runtime.aircraft {
        if other != &id {
            assert_eq!(&game.state.air.runtime.aircraft[other], plane);
        }
    }
    assert!(prepare_deployment(content(), &game.state, &routes).is_err());
    let restored: Game<Cna> = serde_json::from_slice(&serde_json::to_vec(&game).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_vec(&restored).unwrap(),
        serde_json::to_vec(&game).unwrap()
    );
    inventory::check_at(
        content(),
        &restored.state.air,
        Some(Context::from_state(&restored.state).unwrap()),
    )
    .unwrap();
}

/// Cases: airlog:33.0, airlog:37.13, airlog:39.19
#[test]
fn independent_period_phase_and_exact_before_record_reject_stale_or_forged_admissions_atomically() {
    let (game, id, routes) = staged();
    let mut wrong = game.clone();
    let token = prepare_deployment(content(), &game.state, &routes)
        .unwrap()
        .unwrap();
    wrong.state.cursor.op_stage = Some(2);
    let bytes = serde_json::to_vec(&wrong).unwrap();
    assert!(inventory::update_flight(content(), &mut wrong.state, token).is_err());
    assert_eq!(serde_json::to_vec(&wrong).unwrap(), bytes);
    let token = prepare_deployment(content(), &game.state, &routes)
        .unwrap()
        .unwrap();
    wrong = game.clone();
    wrong
        .state
        .air
        .runtime
        .aircraft
        .get_mut(&id)
        .unwrap()
        .refitted = false;
    assert!(inventory::update_flight(content(), &mut wrong.state, token).is_err());
    let (airborne, id) = deployed();
    assert!(inventory::check(content(), &airborne.state.air).is_err());
    let mut stale = airborne.clone();
    stale.state.cursor.op_stage = Some(2);
    assert!(position(content(), &stale.state, &id).is_err());
    stale = airborne.clone();
    stale.state.cursor.index = crate::seq::OPSTAGE
        .iter()
        .position(|s| s.anchor == missions::ANCHOR)
        .unwrap();
    assert!(position(content(), &stale.state, &id).is_err());
    assert!(
        position(
            content(),
            &airborne.state,
            &PlaneId("axis.aircraft-not-present".into())
        )
        .is_err()
    );
}

/// Cases: airlog:34.0, airlog:37.13, airlog:38.24, airlog:38.33, airlog:38.43
#[test]
fn generic_callbacks_cannot_forge_airborne_history_move_or_service_a_launched_plane() {
    let (mut game, id) = deployed();
    for attempted in 0..4 {
        let before = serde_json::to_vec(&game).unwrap();
        assert!(
            inventory::update_from_state(content(), &mut game.state, |runtime| {
                match attempted {
                    0 => runtime.operations = OperationalState::default(),
                    1 => runtime.aircraft.get_mut(&id).unwrap().fuelled = true,
                    2 => runtime.aircraft.get_mut(&id).unwrap().facility = None,
                    _ => {
                        runtime
                            .preparation
                            .current
                            .insert(id.clone(), CurrentPreparation::Unprepared);
                    }
                }
                Ok(())
            })
            .is_err()
        );
        assert_eq!(serde_json::to_vec(&game).unwrap(), before);
    }
    assert_eq!(ground_facility(content(), &game.state, &id).unwrap(), None);
    let mut grounded = game.clone();
    grounded.state.air.runtime.operations = OperationalState::default();
    assert!(
        ground_facility(content(), &grounded.state, &id)
            .unwrap()
            .is_some()
    );
}

/// Cases: airlog:34.73, airlog:34.74, land:3.6
/// Interpretations: interp:air-0021
#[test]
fn contact_projection_is_class_hex_only_and_survives_private_zero_survivors_without_inspect_oracles()
 {
    let (game, id) = deployed();
    let public = markers(&game.state);
    assert_eq!(public.len(), 1);
    assert_eq!(public[0].kind, "air_fighter");
    assert!(!serde_json::to_string(&public).unwrap().contains(&id.0));
    let mut lost = game.clone();
    // Structural post-loss component fixture, not an implemented loss consumer.
    let token = FlightAdmission {
        before: lost.state.air.runtime.clone(),
        after: {
            let mut runtime = lost.state.air.runtime.clone();
            runtime.operations.flights[0].members.clear();
            runtime.aircraft.remove(&id);
            runtime.preparation.current.remove(&id);
            runtime
        },
        expected: Context::from_state(&lost.state).unwrap(),
    };
    inventory::update_flight(content(), &mut lost.state, token).unwrap();
    assert_eq!(markers(&lost.state), public);
    assert_indistinguishable(
        &Cna::dev(),
        content(),
        &game.state,
        &lost.state,
        Side::Commonwealth,
    );
    let seat = Perspective::Seat(cna_core::ids::SeatId::new(
        Side::Commonwealth,
        cna_protocol::Role::Air,
    ));
    assert_eq!(
        crate::view::inspect(content(), &game.state, seat, &public[0].id, false).unwrap()["marker"],
        serde_json::to_value(&public[0]).unwrap()
    );
    let real = crate::view::inspect(content(), &game.state, seat, &id.0, false);
    let fake = crate::view::inspect(
        content(),
        &game.state,
        seat,
        "axis.aircraft-not-present",
        false,
    );
    assert!(real.is_err() && fake.is_err());
}

/// Cases: airlog:37.11, airlog:37.12, airlog:37.15, airlog:37.22, airlog:37.24
#[test]
fn both_owner_source_validation_precedes_any_commit_and_rejects_unknown_load_bad_path_or_duplicate()
{
    let (game, id, routes) = staged();
    for defect in 0..5 {
        let mut trial = game.clone();
        let mut selected = routes.clone();
        match defect {
            0 => {
                selected.remove(&Side::Commonwealth);
            }
            1 => {
                let repeated = selected[&Side::Axis][0].clone();
                selected.get_mut(&Side::Axis).unwrap().push(repeated);
            }
            2 => selected.get_mut(&Side::Axis).unwrap()[0].path.reverse(),
            3 => {
                trial
                    .state
                    .air
                    .runtime
                    .preparation
                    .current
                    .insert(id.clone(), CurrentPreparation::UnknownLegacy);
            }
            _ => inventory::update(content(), &mut trial.state.air, |runtime| {
                runtime.aircraft.get_mut(&id).unwrap().refitted = false;
                Ok(())
            })
            .unwrap(),
        }
        let before = serde_json::to_vec(&trial).unwrap();
        let result = prepare_deployment(content(), &trial.state, &selected);
        assert!(result.is_err());
        if defect == 4 {
            assert!(
                matches!(result, Err(EngineError::Invariant { detail }) if detail.contains("lacks refit"))
            );
        }
        assert_eq!(serde_json::to_vec(&trial).unwrap(), before);
    }
}

/// Cases: airlog:37.13, airlog:39.12, land:3.6
#[test]
fn static_disabled_public_deployment_preserves_intentions_pass_and_enemy_hidden_pairs_without_flight_effects()
 {
    let (answered, id, _) = answered(&opened());
    let actual = evaluate(&Cna::dev(), content(), &answered, &Command::Advance).unwrap();
    assert_eq!(
        actual.game.state.air.runtime.operations,
        OperationalState::default()
    );
    assert!(actual.game.state.air.runtime.aircraft[&id].fuelled);
    assert!(markers(&actual.game.state).is_empty());
    let mut other = answered.clone();
    // Same replying seat's knowledge, different own opponent initial gun choice.
    let initial = &mut other
        .state
        .air
        .runtime
        .tactical
        .answers
        .get_mut(&Side::Axis)
        .unwrap()[0]
        .planes[0]
        .initial_choice;
    initial.as_mut().unwrap().guns = Some(false);
    assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &answered,
        &other,
        &Command::Advance,
        Side::Commonwealth,
    );
    let (mut game, _, routes) = staged();
    let before = serde_json::to_vec(&game).unwrap();
    finish(content(), &mut game.state).unwrap();
    assert_eq!(serde_json::to_vec(&game).unwrap(), before);
    assert!(game.state.air.runtime.operations.flights.is_empty());
    assert!(game.state.air.runtime.aircraft[&id].fuelled);
    assert!(
        prepare_deployment(content(), &game.state, &routes)
            .unwrap()
            .is_some()
    );
}

/// Cases: airlog:34.0, airlog:36.15, airlog:37.13, airlog:38.24, airlog:38.43
#[test]
fn historical_origin_removal_does_not_corrupt_airborne_inventory_or_create_remote_ground_service() {
    let (mut game, id) = deployed();
    let origin = game.state.air.runtime.operations.flights[0].members[0]
        .origin
        .clone();
    let alternate = game
        .state
        .air
        .runtime
        .facilities
        .iter()
        .find_map(|(id, site)| (id != &origin.facility && site.operational()).then_some(id.clone()))
        .unwrap();
    let snapshot = game.state.air.runtime.operations.clone();
    // Component site-loss arrangement: move other grounded references away
    // first, so the test does not bypass a genuine dangling ground/SGSU error.
    inventory::update_from_state(content(), &mut game.state, |runtime| {
        for (other, plane) in &mut runtime.aircraft {
            if other != &id && plane.facility.as_deref() == Some(origin.facility.0.as_str()) {
                plane.facility = Some(alternate.0.clone());
            }
        }
        for unit in runtime.sgsus.values_mut() {
            if unit.position == super::super::sgsu::SgsuPosition::Facility(origin.facility.clone())
            {
                unit.position =
                    super::super::sgsu::SgsuPosition::Ground(crate::state::Location::Hex {
                        hex: origin.hex.clone(),
                    });
            }
        }
        runtime.facilities.remove(&origin.facility);
        Ok(())
    })
    .unwrap();
    assert_eq!(game.state.air.runtime.operations, snapshot);
    assert!(matches!(
        position(content(), &game.state, &id).unwrap(),
        Position::Airborne { .. }
    ));
    assert_eq!(ground_facility(content(), &game.state, &id).unwrap(), None);
    inventory::check_at(
        content(),
        &game.state.air,
        Some(Context::from_state(&game.state).unwrap()),
    )
    .unwrap();
    let mut unproven_ground = game.clone();
    unproven_ground.state.air.runtime.operations = OperationalState::default();
    assert!(position(content(), &unproven_ground.state, &id).is_err());
    assert!(
        inventory::check_at(
            content(),
            &unproven_ground.state.air,
            Some(Context::from_state(&unproven_ground.state).unwrap())
        )
        .is_err()
    );
}

/// Cases: airlog:37.13, airlog:37.16, airlog:38.31
#[test]
fn failed_final_inventory_boundary_rolls_back_actual_component_launch_and_original_game_rng() {
    let (mut game, id, routes) = staged();
    let before = serde_json::to_vec(&game).unwrap();
    let mut observed = false;
    let error = finish_transaction(content(), &mut game.state, &routes, |draft| {
        assert!(matches!(
            position(content(), draft, &id).unwrap(),
            Position::Airborne { .. }
        ));
        assert!(!draft.air.runtime.aircraft[&id].fuelled);
        assert!(!draft.air.runtime.aircraft[&id].refitted);
        // No flight die is invented: launch has no random result. The complete
        // Game byte comparison includes the unchanged original RNG checkpoint.
        let aircraft = draft.air.runtime.aircraft[&id].aircraft.clone();
        let squadron = draft.air.runtime.aircraft[&id].squadron.clone().unwrap();
        draft
            .air
            .squadrons
            .get_mut(&squadron)
            .unwrap()
            .planes
            .get_mut(&aircraft)
            .unwrap()
            .total += 1;
        observed = true;
    })
    .unwrap_err();
    assert!(observed);
    assert!(matches!(error, EngineError::Invariant { .. }));
    assert_eq!(serde_json::to_vec(&game).unwrap(), before);
}

/// Cases: scen:59.32, airlog:34.73, airlog:39.16, airlog:39.17, airlog:40.27
/// Interpretations: interp:air-0021
#[test]
fn actual_split_declarations_with_one_target_and_path_create_one_flight_and_one_class_contact() {
    let mut game = opened();
    let (first, mode, origin) = plane(&game);
    let old = &game.state.air.runtime.aircraft[&first];
    let second = game
        .state
        .air
        .runtime
        .aircraft
        .iter()
        .find_map(|(id, p)| {
            (id != &first
                && p.aircraft == old.aircraft
                && p.squadron == old.squadron
                && p.facility == old.facility)
                .then_some(id.clone())
        })
        .expect("real second aircraft in the source initial squadron");
    let target = content().map.neighbors(&origin).first().unwrap().id.clone();
    let declarations: Vec<_> = [&first, &second].into_iter().map(|id|
        json!({"mission":"strafe_grounded_aircraft","light":"day","target":{"hex":target,"scramble_reserve":false},
        "planes":[{"plane":id,"mode":mode,"initial_choice":{"decision":"load","guns":true,"ordnance":{"kind":"none"}}}]})).collect();
    game = evaluate(
        &Cna::dev(),
        content(),
        &game,
        &missions::initial::tests::response(&game, Side::Axis, json!(declarations)),
    )
    .unwrap()
    .game;
    game = evaluate(
        &Cna::dev(),
        content(),
        &game,
        &missions::initial::tests::response(&game, Side::Commonwealth, Value::Null),
    )
    .unwrap()
    .game;
    missions::finish(content(), &mut game.state).unwrap();
    refit_component_input(&mut game.state);
    game.state.cursor.index = crate::seq::OPSTAGE
        .iter()
        .position(|s| s.anchor == DEPLOYMENT)
        .unwrap();
    game.state.cursor.entered = true;
    game.state.turn.weather = Some(crate::state::WeatherState {
        kind: cna_tables::land::weather::WeatherKind::Normal,
        storm_sections: Vec::new(),
    });
    let routes = BTreeMap::from([
        (
            Side::Axis,
            game.state
                .air
                .runtime
                .tactical
                .declarations
                .keys()
                .map(|mission| RouteSelection {
                    mission: mission.clone(),
                    path: vec![origin.clone(), target.clone()],
                })
                .collect(),
        ),
        (Side::Commonwealth, Vec::new()),
    ]);
    finish_transaction(content(), &mut game.state, &routes, |_| {}).unwrap();
    assert_eq!(game.state.air.runtime.operations.flights.len(), 1);
    assert_eq!(
        game.state.air.runtime.operations.flights[0].members.len(),
        2
    );
    assert_eq!(markers(&game.state).len(), 1);
    assert_eq!(
        game.state.air.runtime.preparation.initial_choices[&first],
        InitialChoice::Committed
    );
    assert_eq!(
        game.state.air.runtime.preparation.initial_choices[&second],
        InitialChoice::Committed
    );
}

/// Cases: airlog:39.12, land:3.6
#[test]
fn disabled_own_flight_payload_is_unadvertised_and_refuses_before_buffering_independent_of_enemy_credit()
 {
    let game = opened();
    let (id, mode, target) = plane(&game);
    let mut legacy_enemy = game.clone();
    let enemy = legacy_enemy
        .state
        .air
        .runtime
        .aircraft
        .iter()
        .find(|(_, p)| p.force == "commonwealth")
        .unwrap()
        .0
        .clone();
    // Same opposing roster/coarse bits; only its private initial right/load
    // provenance differs. No source payment or initial credit is manufactured.
    legacy_enemy
        .state
        .air
        .runtime
        .preparation
        .initial_choices
        .remove(&enemy);
    legacy_enemy
        .state
        .air
        .runtime
        .preparation
        .current
        .insert(enemy, CurrentPreparation::UnknownLegacy);
    inventory::check(content(), &legacy_enemy.state.air).unwrap();
    let action = json!([{"mission":"transfer","light":"day","target":{"hex":target,"scramble_reserve":false},
        "planes":[{"plane":id,"mode":mode}]}]);
    let pending = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap();
    assert!(pending.space.check(&action).is_err());
    let command = missions::initial::tests::response(&game, Side::Axis, action);
    assert!(evaluate(&Cna::dev(), content(), &game, &command).is_err());
    assert!(evaluate(&Cna::dev(), content(), &legacy_enemy, &command).is_err());
    assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &game,
        &legacy_enemy,
        &command,
        Side::Axis,
    );
    assert_eq!(
        game.state.air.runtime.operations,
        OperationalState::default()
    );
    assert_eq!(
        legacy_enemy.state.air.runtime.operations,
        OperationalState::default()
    );
}

/// Cases: airlog:37.12, airlog:37.13, airlog:40.24
#[test]
fn singleton_canonical_origin_target_is_a_valid_component_flight_and_empty_route_still_refuses() {
    let (mut game, id, routes) = staged_at(true);
    let path = &routes[&Side::Axis][0].path;
    assert_eq!(path.len(), 1);
    let target = path[0].clone();
    let mut empty = routes.clone();
    empty.get_mut(&Side::Axis).unwrap()[0].path.clear();
    assert!(prepare_deployment(content(), &game.state, &empty).is_err());
    finish_transaction(content(), &mut game.state, &routes, |_| {}).unwrap();
    assert_eq!(
        position(content(), &game.state, &id).unwrap(),
        Position::Airborne {
            target: target.clone()
        }
    );
    assert_eq!(markers(&game.state)[0].hex, target.to_string());
    assert!(!game.state.air.runtime.aircraft[&id].fuelled);
}

/// Cases: airlog:34.73, airlog:34.74, airlog:37.13, land:3.6
/// Interpretations: interp:air-0021
#[test]
fn central_sync_derives_contact_birth_persistence_and_scheduled_return_removal_for_all_board_perspectives()
 {
    use cna_core::{dice::CampaignRng, engine::Cx, event::EngineEvent, visibility::Audience};
    use cna_protocol::GameEvent;
    let (mut game, id, routes) = staged();
    let mut rng = CampaignRng::from_state(&game.rng);
    let prefix = EngineEvent::new(
        Audience::Public,
        GameEvent::Note {
            text: "Preexisting event prefix.".into(),
        },
    );
    let mut events = vec![prefix.clone()];
    let mut cx = Cx {
        rng: &mut rng,
        events: &mut events,
    };
    let mut sync = crate::view::Sync::new(&game.state, &cx);
    finish_transaction(content(), &mut game.state, &routes, |_| {}).unwrap();
    sync.point(content(), &game.state, &mut cx);
    let contact = markers(&game.state)[0].clone();
    for perspective in Perspective::all() {
        let placed: Vec<_> = cx
            .events
            .iter()
            .filter(|e| e.visible_to(perspective))
            .filter_map(|e| match &e.event {
                GameEvent::MarkerPlaced { marker } if marker.id == contact.id => {
                    Some(marker.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(placed, vec![contact.clone()]);
        let snapshot: Vec<_> = crate::view::view(content(), &game.state, perspective)
            .markers
            .into_iter()
            .filter(|m| m.id == contact.id)
            .collect();
        assert_eq!(snapshot, placed);
    }
    let birth_end = cx.events.len();
    // Component post-loss fixture through the exact canonical update. No real
    // combat or recovery consumer is claimed by constructing this closed delta.
    let mut after = game.state.air.runtime.clone();
    after.operations.flights[0].members.clear();
    after.aircraft.remove(&id);
    after.preparation.current.remove(&id);
    let admission = FlightAdmission {
        before: game.state.air.runtime.clone(),
        after,
        expected: Context::from_state(&game.state).unwrap(),
    };
    inventory::update_flight(content(), &mut game.state, admission).unwrap();
    sync.point(content(), &game.state, &mut cx);
    assert!(!cx.events[birth_end..].iter().any(|e| matches!(
        e.event,
        GameEvent::MarkerRemoved { .. } | GameEvent::MarkerPlaced { .. }
    )));
    for perspective in Perspective::all() {
        assert_eq!(
            crate::view::view(content(), &game.state, perspective)
                .markers
                .into_iter()
                .filter(|m| m.id == contact.id)
                .collect::<Vec<_>>(),
            vec![contact.clone()]
        );
    }
    let loss_end = cx.events.len();
    // Explicit scheduled-return fixture only; there is no live return consumer
    // in this intermediate slice. Never emit a MarkerRemoved event by hand.
    game.state.cursor.index = crate::seq::OPSTAGE
        .iter()
        .position(|s| s.anchor == "opstage.land_support_air.return")
        .unwrap();
    game.state.air.runtime.operations = OperationalState::default();
    sync.point(content(), &game.state, &mut cx);
    for perspective in Perspective::all() {
        let removed: Vec<_> = cx.events[loss_end..]
            .iter()
            .filter(|e| e.visible_to(perspective))
            .filter_map(|e| match &e.event {
                GameEvent::MarkerRemoved { marker_id } => Some(marker_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(removed, vec![contact.id.clone()]);
        assert!(
            crate::view::view(content(), &game.state, perspective)
                .markers
                .iter()
                .all(|m| m.id != contact.id)
        );
    }
    assert_eq!(cx.events[0], prefix);
    assert_eq!(cx.rng.state(), game.rng);
}
