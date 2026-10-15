//! Fixed-window seam controls. Strict Respond/Advance executes after owned entry;
//! the separate, broader full-profile pre-entry guard is deliberately retained.
use super::*;
use cna_core::{
    decision::ActionSchema,
    engine::{Progress, Ruleset},
    event::EngineEvent,
    visibility::{Audience, Perspective},
};
use std::collections::{BTreeMap, BTreeSet};

const ROLES: [Role; 3] = [Role::FrontLine, Role::RearArea, Role::Logistics];
const ANCHORS: [&str; 2] = [
    "opstage.organization.reorganization",
    "opstage.organization.training",
];
const GUN: &str = "commonwealth.replacement-2";

fn empty(pending: &crate::state::Pending) -> bool {
    matches!(&pending.space.schema, ActionSchema::Choice { options } if options.is_empty())
}

fn entered(content: &CnaContent, input: &Game<Cna>) -> (Game<Cna>, Vec<EngineEvent>) {
    let frozen = bytes(input);
    let mut output = input.clone();
    let mut rng = CampaignRng::from_state(&input.rng);
    let mut events = vec![];
    unit_windows::enter_units(
        content,
        &mut output.state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert_eq!(rng.state(), input.rng);
    assert_eq!(bytes(input), frozen);
    assert_eq!(
        bytes(&output.state.land.units),
        bytes(&input.state.land.units)
    );
    assert_eq!(
        bytes(&output.state.logistics),
        bytes(&input.state.logistics)
    );
    assert_eq!(bytes(&output.state.air), bytes(&input.state.air));
    for owner in Side::ALL {
        assert_eq!(
            bytes(&side(&output.state, owner).unwrap().counters),
            bytes(&side(&input.state, owner).unwrap().counters)
        );
    }
    (output, events)
}

fn schedule(state: &State) -> Vec<(SeatId, String, String, u32)> {
    state
        .decisions
        .pending
        .iter()
        .map(|p| (p.seat, p.id.to_string(), p.kind.clone(), p.revision))
        .collect()
}

fn viewers(owner: SeatId) -> Vec<Perspective> {
    // Both complete opponent side/seat views and every non-owner seat are compared.
    // The own-side aggregate and operator intentionally contain the owner's domain.
    Perspective::all()
        .filter(|p| match p {
            Perspective::Operator => false,
            Perspective::Side(side) => *side != owner.side,
            Perspective::Seat(seat) => *seat != owner,
        })
        .collect()
}

fn output(
    content: &CnaContent,
    game: &Game<Cna>,
    perspective: Perspective,
    targets: &BTreeSet<String>,
) -> Value {
    let rules = Cna::full();
    let pending: Vec<_> = rules
        .pending(content, &game.state)
        .into_iter()
        .filter(|request| perspective.can_see(&Audience::Seat(request.seat)))
        .collect();
    // The same union of actual inspect targets is queried for every non-owner
    // teammate/opponent; preserve complete Ok payloads and typed Err payloads.
    let inspected: BTreeMap<_, _> = targets
        .iter()
        .map(|target| {
            (
                target.clone(),
                rules.inspect(content, &game.state, perspective, target),
            )
        })
        .collect();
    json!({
        "view": rules.view(content, &game.state, perspective),
        "observe": rules.observe(content, &game.state, perspective),
        "inspect": inspected,
        "pending": pending,
        "clock": rules.clock(content, &game.state),
    })
}

fn observed_events(events: &[EngineEvent], perspective: Perspective) -> Vec<&EngineEvent> {
    events
        .iter()
        .filter(|event| event.visible_to(perspective))
        .collect()
}

fn same_outputs(
    content: &CnaContent,
    owner: SeatId,
    games: &[Game<Cna>],
    histories: &[Vec<EngineEvent>],
) {
    let states: Vec<_> = games.iter().map(|game| &game.state).collect();
    let targets = crate::testkit::inspect_targets(&states);
    let opponent = if owner.side == Side::Axis {
        Side::Commonwealth
    } else {
        Side::Axis
    };
    // Add the standard full state-pair proof at actual entry, after each Respond,
    // and after atomic finish; retain the stronger ordered byte comparisons below.
    for game in games.iter().skip(1) {
        crate::testkit::assert_indistinguishable(
            &Cna::full(),
            content,
            &games[0].state,
            &game.state,
            opponent,
        );
    }
    for perspective in viewers(owner) {
        let reference = bytes(&output(content, &games[0], perspective, &targets));
        let reference_events = bytes(&observed_events(&histories[0], perspective));
        for (game, history) in games.iter().zip(histories).skip(1) {
            assert_eq!(
                bytes(&output(content, game, perspective, &targets)),
                reference,
                "full ordered output for {perspective}"
            );
            assert_eq!(
                bytes(&observed_events(history, perspective)),
                reference_events,
                "entire ordered replay stream for {perspective}"
            );
        }
    }
    let public = |events: &[EngineEvent]| {
        bytes(
            &events
                .iter()
                .filter(|event| event.audience == Audience::Public)
                .collect::<Vec<_>>(),
        )
    };
    for history in histories.iter().skip(1) {
        assert_eq!(public(history), public(&histories[0]));
    }
    // Retain the full ordered pending schedule, not only each filtered domain.
    for game in games.iter().skip(1) {
        assert_eq!(
            bytes(&schedule(&game.state)),
            bytes(&schedule(&games[0].state))
        );
        assert_eq!(game.rng, games[0].rng);
    }
}

fn same_action(content: &CnaContent, owner: SeatId, games: &[Game<Cna>], command: &Command) {
    let opponent = if owner.side == Side::Axis {
        Side::Commonwealth
    } else {
        Side::Axis
    };
    let frozen = bytes(&games);
    let reference_recovered = restored_from(&games[0]);
    for game in games.iter().skip(1) {
        crate::testkit::assert_action_indistinguishable(
            &Cna::full(),
            content,
            &games[0],
            game,
            command,
            opponent,
        );
        crate::testkit::assert_action_indistinguishable(
            &Cna::full(),
            content,
            &reference_recovered,
            &restored_from(game),
            command,
            opponent,
        );
    }
    assert_eq!(bytes(&games), frozen);
}

fn waiting(content: &CnaContent, input: &Game<Cna>) {
    assert!(!input.state.decisions.pending.is_empty());
    let frozen = bytes(input);
    for game in [input.clone(), restored_from(input)] {
        let transition = evaluate(&Cna::full(), content, &game, &Command::Advance).unwrap();
        assert_eq!(transition.progress, Some(Progress::AwaitingDecisions));
        assert_eq!(bytes(&transition.game), frozen);
        assert!(transition.events.is_empty());
    }
    assert_eq!(bytes(input), frozen);
}

fn pass(content: &CnaContent, input: &Game<Cna>) -> (Game<Cna>, Vec<EngineEvent>) {
    let pending = input.state.decisions.pending.first().unwrap();
    let command = move_response(&input.state, pending, Value::Null);
    let frozen = bytes(input);
    let a = evaluate(&Cna::full(), content, input, &command).unwrap();
    let b = evaluate(&Cna::full(), content, &restored_from(input), &command).unwrap();
    assert_eq!(bytes(&a.game), bytes(&b.game));
    assert_eq!(bytes(&a.events), bytes(&b.events));
    assert_eq!(a.progress, b.progress);
    // A pass consumes exactly its pending request. All units, stocks, candidates,
    // procedures, CP, campaign RNG and other bytes are checked by this whole Game.
    let mut expected = input.clone();
    expected.state.decisions.pending.remove(0);
    assert_eq!(bytes(&a.game), bytes(&expected));
    assert_eq!(bytes(input), frozen);
    (a.game, a.events)
}

fn strict_pairs(role: Role) {
    for owner in Side::ALL {
        for ready in [false, true] {
            for point in ANCHORS {
                let (mut content, mut present, id, _) = fixture(ready);
                let source = content.units.units.get_mut(&id).unwrap();
                source.side = owner;
                source.nationality = if owner == Side::Axis {
                    "italian"
                } else {
                    "british"
                }
                .into();
                match role {
                    Role::RearArea => source.sheet = "controlled.garrison".into(),
                    Role::Logistics => source.kind = Some("second_line_truck".into()),
                    Role::FrontLine => {}
                    _ => unreachable!(),
                }
                present.land.units.get_mut(&id).unwrap().side = owner;
                let mut selected = counter(&present).clone();
                selected.kind = ReplacementKind::Infantry {
                    nation: if owner == Side::Axis {
                        ReplacementNation::Italian
                    } else {
                        ReplacementNation::Commonwealth
                    },
                };
                for own in &mut present.land.replacements.sides {
                    own.counters.clear();
                }
                side_mut(&mut present, owner)
                    .unwrap()
                    .counters
                    .push(selected);
                anchor(&mut present, point);
                let seat = SeatId::new(owner, role);
                assert_eq!(
                    crate::ownership::seat_for_unit(&content, &present, &id),
                    role
                );
                let mut absent = present.clone();
                side_mut(&mut absent, owner).unwrap().counters.clear();
                let mut opposite = present.clone();
                side_mut(&mut opposite, owner).unwrap().counters[0].training = if ready {
                    Training::Training {
                        required_stages: 1,
                        completed: 0,
                        last_counted: None,
                        candidate: None,
                    }
                } else {
                    Training::Ready
                };
                let mut games = vec![];
                let mut histories = vec![];
                for input in [present, absent, opposite] {
                    let (opened, events) = entered(&content, &game(input));
                    assert_eq!(opened.state.decisions.pending.len(), 6);
                    games.push(opened);
                    histories.push(events);
                }
                let training_point = point == ANCHORS[1];
                let domain = |game: &Game<Cna>| {
                    !empty(
                        game.state
                            .decisions
                            .pending
                            .iter()
                            .find(|p| p.seat == seat)
                            .unwrap(),
                    )
                };
                assert_eq!(domain(&games[0]), training_point != ready);
                assert!(!domain(&games[1]));
                assert_eq!(domain(&games[2]), training_point == ready);
                same_outputs(&content, seat, &games, &histories);
                for _ in 0..6 {
                    // Compare actual waiting Advance and the next canonical seat
                    // response across both hidden-fact pairs and their recoveries.
                    same_action(&content, seat, &games, &Command::Advance);
                    let command = move_response(
                        &games[0].state,
                        &games[0].state.decisions.pending[0],
                        Value::Null,
                    );
                    same_action(&content, seat, &games, &command);
                    for index in 0..games.len() {
                        waiting(&content, &games[index]);
                        let recovered = restored_from(&games[index]);
                        assert_eq!(bytes(&games[index]), bytes(&recovered));
                        let opponent = if owner == Side::Axis {
                            Side::Commonwealth
                        } else {
                            Side::Axis
                        };
                        crate::testkit::assert_indistinguishable(
                            &Cna::full(),
                            &content,
                            &games[index].state,
                            &recovered.state,
                            opponent,
                        );
                        let targets = crate::testkit::inspect_targets(&[
                            &games[index].state,
                            &recovered.state,
                        ]);
                        for perspective in viewers(seat) {
                            assert_eq!(
                                bytes(&output(&content, &games[index], perspective, &targets)),
                                bytes(&output(&content, &recovered, perspective, &targets))
                            );
                        }
                        let (next, events) = pass(&content, &games[index]);
                        games[index] = next;
                        histories[index].extend(events);
                    }
                    same_outputs(&content, seat, &games, &histories);
                }
                for current in &mut games {
                    let before = bytes(current);
                    let mut recovered = restored_from(current);
                    unit_windows::finish_units(&content, &mut current.state).unwrap();
                    unit_windows::finish_units(&content, &mut recovered.state).unwrap();
                    assert_eq!(bytes(current), bytes(&recovered));
                    let opponent = if owner == Side::Axis {
                        Side::Commonwealth
                    } else {
                        Side::Axis
                    };
                    crate::testkit::assert_indistinguishable(
                        &Cna::full(),
                        &content,
                        &current.state,
                        &recovered.state,
                        opponent,
                    );
                    let targets =
                        crate::testkit::inspect_targets(&[&current.state, &recovered.state]);
                    for perspective in viewers(seat) {
                        assert_eq!(
                            bytes(&output(&content, current, perspective, &targets)),
                            bytes(&output(&content, &recovered, perspective, &targets))
                        );
                    }
                    assert_ne!(bytes(current), before); // resolved only after six actual answers
                    let finished = bytes(current);
                    unit_windows::finish_units(&content, &mut current.state).unwrap();
                    assert_eq!(bytes(current), finished);
                }
                same_outputs(&content, seat, &games, &histories);
            }
        }
    }
}

/// Cases: land:3.6, land:17.33, land:20.41, land:20.43, land:20.49
#[ignore = "slow: full-roster thirteen-perspective present/absent/opposite-status privacy pairs with ordered recovery"]
#[test]
fn replacement_fixed_front_line_strict_present_absent_empty_status_ordered_recovery_pairs() {
    strict_pairs(Role::FrontLine);
}
/// Cases: land:3.6, land:17.33, land:20.41, land:20.43, land:20.49
#[ignore = "slow: full-roster thirteen-perspective present/absent/opposite-status privacy pairs with ordered recovery"]
#[test]
fn replacement_fixed_rear_area_strict_present_absent_empty_status_ordered_recovery_pairs() {
    strict_pairs(Role::RearArea);
}
/// Cases: land:3.6, land:17.33, land:20.41, land:20.43, land:20.49
#[ignore = "slow: full-roster thirteen-perspective present/absent/opposite-status privacy pairs with ordered recovery"]
#[test]
fn replacement_fixed_logistics_strict_present_absent_empty_status_ordered_recovery_pairs() {
    strict_pairs(Role::Logistics);
}

/// Cases: land:3.6, land:20.41, land:20.43, land:20.49
#[test]
fn replacement_empty_fixed_windows_require_six_passes_at_both_anchors_and_keep_cumulative_ids() {
    let (content, mut state, _, _) = fixture(false);
    for own in &mut state.land.replacements.sides {
        own.counters.clear();
    }
    let mut current = game(state);
    let initial_counts = current.state.decisions.opened.clone();
    for (index, point) in ANCHORS.into_iter().enumerate() {
        anchor(&mut current.state, point);
        let before = current.clone();
        let (opened, events) = entered(&content, &current);
        assert_eq!(events.len(), 6);
        let expected_seats: Vec<_> = Side::ALL
            .into_iter()
            .flat_map(|side| ROLES.into_iter().map(move |role| SeatId::new(side, role)))
            .collect();
        assert_eq!(
            opened
                .state
                .decisions
                .pending
                .iter()
                .map(|p| p.seat)
                .collect::<Vec<_>>(),
            expected_seats
        );
        for pending in &opened.state.decisions.pending {
            assert!(empty(pending));
            assert!(pending.space.pass.is_some());
            assert!(pending.space.check(&Value::Null).is_ok());
            assert!(pending.space.check(&json!({})).is_err());
            let count = initial_counts.get(&pending.seat).copied().unwrap_or(0) + index as u32 + 1;
            assert_eq!(pending.id.to_string(), format!("{}-{count}", pending.seat));
        }
        // Re-entering the same procedure never opens extra IDs/events.
        let (again, repeated) = entered(&content, &opened);
        assert_eq!(bytes(&again), bytes(&opened));
        assert!(repeated.is_empty());
        current = opened;
        for remaining in (1..=6).rev() {
            waiting(&content, &current);
            let frozen = bytes(&current);
            unit_windows::finish_units(&content, &mut current.state).unwrap();
            assert_eq!(bytes(&current), frozen);
            assert_eq!(current.state.decisions.pending.len(), remaining);
            current = pass(&content, &current).0;
        }
        unit_windows::finish_units(&content, &mut current.state).unwrap();
        assert_eq!(
            bytes(&current.state.land.units),
            bytes(&before.state.land.units)
        );
        assert_eq!(
            bytes(&current.state.logistics),
            bytes(&before.state.logistics)
        );
        assert_eq!(current.rng, before.rng);
        // Exact complete-State delta: only decisions plus the one existing procedure.
        let mut expected = before;
        expected.state.decisions = current.state.decisions.clone();
        for owner in Side::ALL {
            let actual = side(&current.state, owner).unwrap();
            let own = side_mut(&mut expected.state, owner).unwrap();
            if index == 0 {
                own.reorganization = actual.reorganization.clone();
            } else {
                own.training = actual.training.clone();
            }
            assert!(matches!(
                if index == 0 {
                    &actual.reorganization.phase
                } else {
                    &actual.training.phase
                },
                ProcedurePhase::Resolved
            ));
        }
        assert_eq!(bytes(&current), bytes(&expected));
    }
}

/// Cases: land:3.6, land:20.41, land:20.43
#[test]
fn replacement_empty_window_rejects_shape_wrong_seat_stale_and_repeated_responses_purely() {
    for point in ANCHORS {
        let (content, mut state, _, _) = fixture(false);
        for own in &mut state.land.replacements.sides {
            own.counters.clear();
        }
        anchor(&mut state, point);
        let (input, _) = entered(&content, &game(state));
        let pending = &input.state.decisions.pending[0];
        for reason in 0..3 {
            let mut command = move_response(&input.state, pending, Value::Null);
            let Command::Respond(response) = &mut command else {
                unreachable!()
            };
            match reason {
                0 => response.action = json!({"counters": []}),
                1 => response.seat.role = Role::Air,
                _ => response.decision_revision += 1,
            }
            let expected = evaluate(&Cna::full(), &content, &input, &command).unwrap_err();
            assert!(matches!(
                (&expected, reason),
                (cna_core::engine::Rejection::Illegal { .. }, 0)
                    | (cna_core::engine::Rejection::WrongSeat { .. }, 1)
                    | (cna_core::engine::Rejection::StaleRevision { .. }, 2)
            ));
            for candidate in [&input, &restored_from(&input)] {
                let frozen = bytes(candidate);
                assert_eq!(
                    evaluate(&Cna::full(), &content, candidate, &command).unwrap_err(),
                    expected
                );
                assert_eq!(bytes(candidate), frozen);
            }
        }
        let command = move_response(&input.state, pending, Value::Null);
        let accepted = pass(&content, &input).0;
        let frozen = bytes(&accepted);
        assert!(matches!(
            evaluate(&Cna::full(), &content, &accepted, &command),
            Err(cna_core::engine::Rejection::UnknownDecision { .. })
        ));
        assert_eq!(bytes(&accepted), frozen);
    }
}

/// Cases: land:3.6, land:20.41, land:20.43
#[test]
fn replacement_fixed_windows_initialize_default_empty_sides_and_refuse_duplicate_sides_atomically()
{
    let (content, mut state, _, _) = fixture(false);
    state.land.replacements.sides.clear();
    let mut rng = CampaignRng::from_seed([94; 32]);
    let initial_rng = rng.state();
    let mut events = vec![];
    unit_windows::enter_units(
        &content,
        &mut state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert_eq!(state.decisions.pending.len(), 6);
    assert_eq!(events.len(), 6);
    assert_eq!(state.land.replacements.sides.len(), 2);
    assert!(state.decisions.pending.iter().all(empty));
    assert_eq!(rng.state(), initial_rng);
    let (content, mut duplicate, _, _) = fixture(false);
    duplicate
        .land
        .replacements
        .sides
        .push(SideReplacements::new(Side::Commonwealth));
    let frozen = bytes(&duplicate);
    events.clear();
    assert_eq!(
        unit_windows::enter_units(
            &content,
            &mut duplicate,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            }
        )
        .unwrap_err(),
        invariant("duplicate replacement side")
    );
    assert_eq!(bytes(&duplicate), frozen);
    assert_eq!(rng.state(), initial_rng);
    assert!(events.is_empty());
}

fn hw(weapon: &str) -> (CnaContent, State, UnitId, SeatId, AllocationPlan) {
    let (mut content, mut state, id, seat) = fixture(true);
    content.units.units.get_mut(&id).unwrap().infantry_kind = Some(InfantryKind::HeavyWeapons);
    assert!(content.units.weapons.contains_key(weapon));
    let mut gun = counter(&state).clone();
    gun.id = GUN.into();
    gun.kind = ReplacementKind::Gun {
        nation: ReplacementNation::Commonwealth,
        weapon: weapon.into(),
    };
    side_mut(&mut state, seat.side).unwrap().counters.push(gun);
    let plan = AllocationPlan {
        uses: [COUNTER, GUN]
            .into_iter()
            .map(|counter| AllocationUse {
                counter: counter.into(),
                recipient: id.clone(),
                points: 1,
                conversion: "heavy_weapons".into(),
            })
            .collect(),
    };
    (content, state, id, seat, plan)
}

/// Cases: land:19.64d, land:20.3, land:20.41, land:20.48, land:20.49
#[test]
fn replacement_heavy_weapons_consumes_both_inputs_into_whole_scalar_toe_and_keeps_payment_history()
{
    for weapon in [
        "cw.18_pounder_gun",
        "cw.2_pounder",
        "cw.aa_light_bofors_40mm",
    ] {
        for consumed in [false, true] {
            let (content, mut state, id, seat, plan) = hw(weapon);
            if consumed {
                for counter in &mut side_mut(&mut state, seat.side).unwrap().counters {
                    counter.points = 1;
                }
            }
            let frozen = bytes(&state);
            let after = allocation::prepare(&content, &state, seat, &plan).unwrap();
            assert_eq!(bytes(&state), frozen);
            assert_eq!(after.land.units[&id].toe, Some(Toe::Under { under: 4 }));
            assert_eq!(after.land.units[&id].cp_spent_quarters, 10);
            assert_eq!(bytes(&after.logistics), bytes(&state.logistics));
            assert_eq!(
                side(&after, seat.side).unwrap().counters.len(),
                if consumed { 0 } else { 2 }
            );
            for (old, remaining) in side(&state, seat.side)
                .unwrap()
                .counters
                .iter()
                .zip(&side(&after, seat.side).unwrap().counters)
            {
                let mut expected = old.clone();
                expected.points -= 1;
                expected.physical.cp_quarters += 4;
                assert_eq!(bytes(remaining), bytes(&expected));
            }
            open_window(&content, &mut state);
            let input = game(state);
            let command = respond(&input.state, serde_json::to_value(&plan).unwrap());
            let accepted = evaluate(&Cna::full(), &content, &input, &command).unwrap();
            assert_eq!(
                bytes(&accepted.game.state.land.units),
                bytes(&input.state.land.units)
            );
            for owner in Side::ALL {
                assert_eq!(
                    bytes(&side(&accepted.game.state, owner).unwrap().counters),
                    bytes(&side(&input.state, owner).unwrap().counters)
                );
            }
            assert_eq!(accepted.game.rng, input.rng);
            assert!(accepted.events.iter().any(|event| matches!(
                &event.event,
                cna_protocol::GameEvent::DecisionResolved { .. }
            )));
            let own = side(&accepted.game.state, seat.side).unwrap();
            assert_eq!(
                bytes(&own.reorganization.phase),
                bytes(&ProcedurePhase::Awaiting {
                    submitted: Some(ReplacementPlan::Allocation(plan.clone()))
                })
            );
            let mut current = accepted.game;
            while !current.state.decisions.pending.is_empty() {
                current = pass(&content, &current).0;
            }
            let mut recovered = restored_from(&current);
            unit_windows::finish_units(&content, &mut current.state).unwrap();
            unit_windows::finish_units(&content, &mut recovered.state).unwrap();
            assert_eq!(bytes(&current), bytes(&recovered));
            assert_eq!(current.state.land.units[&id].toe, after.land.units[&id].toe);
            assert_eq!(
                bytes(&side(&current.state, seat.side).unwrap().counters),
                bytes(&side(&after, seat.side).unwrap().counters)
            );
            assert_eq!(current.rng, input.rng);
        }
    }
}

/// Cases: land:19.64d, land:20.3, land:20.41, land:20.43, land:20.49
#[test]
fn replacement_heavy_weapons_refuses_plain_infantry_nation_partial_assignment_and_invalid_equipment_atomically()
 {
    for reason in 0..10 {
        let (mut content, mut state, id, seat, mut plan) = hw("cw.2_pounder");
        match reason {
            0 => {
                content.units.units.get_mut(&id).unwrap().infantry_kind =
                    Some(InfantryKind::Ordinary)
            }
            1 => {
                side_mut(&mut state, seat.side).unwrap().counters[1].kind = ReplacementKind::Gun {
                    nation: ReplacementNation::German,
                    weapon: "cw.2_pounder".into(),
                }
            }
            2 => {
                plan.uses.pop();
            }
            3 => {
                plan.uses.remove(0);
            }
            4 => plan.uses[1].points = 2,
            5 => {
                side_mut(&mut state, seat.side).unwrap().counters[1].recipient =
                    UnitId::new("absent-recipient")
            }
            6 => state.land.units.get_mut(&id).unwrap().location = Location::Eliminated,
            7 => {
                side_mut(&mut state, seat.side).unwrap().counters[1].kind = ReplacementKind::Gun {
                    nation: ReplacementNation::Commonwealth,
                    weapon: "cw.a9_cruiser".into(),
                }
            }
            8 => state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Normal(NormalToe::N)),
            _ => plan.uses[1].conversion = "any_other_infantry".into(),
        }
        let input = game(state);
        let frozen = bytes(&input);
        let expected = allocation::prepare(&content, &input.state, seat, &plan).unwrap_err();
        for candidate in [&input, &restored_from(&input)] {
            assert_eq!(
                allocation::prepare(&content, &candidate.state, seat, &plan).unwrap_err(),
                expected
            );
            assert_eq!(bytes(candidate), frozen);
        }
        let (opened, _) = entered(&content, &input);
        let command = respond(&opened.state, serde_json::to_value(&plan).unwrap());
        let rejected = evaluate(&Cna::full(), &content, &opened, &command).unwrap_err();
        if reason == 5 {
            let pending = opened.state.decisions.pending.iter()
                .find(|pending| pending.seat == seat).unwrap();
            assert!(matches!(&pending.space.schema, ActionSchema::Record { fields }
                if fields.iter().any(|field| field.name == "uses"
                    && matches!(&field.schema, ActionSchema::List { min: 0, max: 1, .. }))));
            assert_eq!(rejected, cna_core::engine::Rejection::Illegal {
                message: "the answer does not fit this decision's action space: action.uses: expected 0..=1 items".into(),
            });
        } else {
            assert_eq!(rejected, expected);
        }
        let frozen = bytes(&opened);
        for checkpoint in [&opened, &restored_from(&opened)] {
            assert_eq!(
                evaluate(&Cna::full(), &content, checkpoint, &command).unwrap_err(),
                rejected
            );
            assert_eq!(bytes(checkpoint), frozen);
        }
        if reason == 5 {
            // Keep the original exact handler refusal after a genuine max-two opening.
            // Only the recipient changes after declaration; the pending schema is untouched.
            let (healthy_content, healthy_state, _, healthy_seat, healthy_plan) = hw("cw.2_pounder");
            assert_eq!(healthy_seat, seat);
            assert_eq!(bytes(&healthy_plan), bytes(&plan));
            let (mut late, _) = entered(&healthy_content, &game(healthy_state));
            let pending = late.state.decisions.pending.iter()
                .find(|pending| pending.seat == healthy_seat).unwrap();
            assert!(matches!(&pending.space.schema, ActionSchema::Record { fields }
                if fields.iter().any(|field| field.name == "uses"
                    && matches!(&field.schema, ActionSchema::List { min: 0, max: 2, .. }))));
            let action = serde_json::to_value(&healthy_plan).unwrap();
            pending.space.check(&action).unwrap();
            let command = respond(&late.state, action);
            side_mut(&mut late.state, healthy_seat.side).unwrap().counters[1].recipient =
                UnitId::new("absent-recipient");
            assert_eq!(expected, cna_core::engine::Rejection::Illegal {
                message: "not an own living replacement recipient".into(),
            });
            assert_eq!(
                allocation::prepare(&healthy_content, &late.state, healthy_seat, &healthy_plan).unwrap_err(),
                expected
            );
            let frozen = bytes(&late);
            for checkpoint in [&late, &restored_from(&late)] {
                assert_eq!(
                    evaluate(&Cna::full(), &healthy_content, checkpoint, &command).unwrap_err(),
                    expected
                );
                assert_eq!(bytes(checkpoint), frozen);
            }
        }
    }
}
