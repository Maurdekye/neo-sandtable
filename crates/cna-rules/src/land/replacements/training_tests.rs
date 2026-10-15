//! Controlled kernel fixtures and canonical Respond/Advance controls; no new production data.
use super::scheduling::{side, side_mut};
use super::*;
use crate::{Cna, CnaContent, State};
use cna_content::units::{InfantryKind, NormalToe, Toe};
use cna_core::{
    decision::DecisionResponse,
    dice::CampaignRng,
    engine::{Command, Cx, Game, evaluate},
    ids::SeatId,
};
use cna_protocol::Role;
use serde::Serialize;
use serde_json::{Value, json};

#[path = "fixed_window_tests.rs"]
mod fixed_windows;

const COUNTER: &str = "commonwealth.replacement-1";
fn bytes(value: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn anchor(state: &mut State, name: &str) {
    state.cursor.block = crate::seq::Block::OpStage;
    state.cursor.index = crate::seq::OPSTAGE
        .iter()
        .position(|step| step.anchor == name)
        .unwrap();
    state.cursor.half = None;
    state.cursor.entered = true;
}
fn tail(state: &mut State) {
    state.cursor.block = crate::seq::Block::PlayerHalf;
    state.cursor.index = crate::seq::PLAYER_HALF.len() - 1;
    state.cursor.half = Some(crate::seq::Half::B);
    state.cursor.entered = true;
}
fn fixture(ready: bool) -> (CnaContent, State, UnitId, SeatId) {
    let mut content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut state = State::new(&content).unwrap();
    let id = UnitId::new("cw.unassigned_inf.1st_rnf_mg_bn");
    // Explicit controlled eligibility facts, not a new OA entitlement or Graziani setup.
    let source = content.units.units.get_mut(&id).unwrap();
    source.echelon = Some("battalion".into());
    source.infantry_kind = Some(InfantryKind::MachineGun);
    source.basic_morale = Some(1);
    source.toe = Some(Toe::Normal(NormalToe::N));
    let class = source.class.clone().unwrap();
    let sheet = source.sheet.clone();
    content.units.classes.get_mut(&class).unwrap().max_toe = Some(6);
    content
        .units
        .sheets
        .get_mut(&sheet)
        .unwrap()
        .basic_morale_untrained = None;
    let hex = content.areas.areas["cairo"].hex_ids[0].clone();
    let unit = state.land.units.get_mut(&id).unwrap();
    unit.location = Location::Hex { hex: hex.clone() };
    unit.toe = Some(Toe::Under { under: 3 });
    unit.assignment = crate::state::Assignment::Independent;
    unit.attached_to = None;
    unit.cp_spent_quarters = 0;
    unit.voluntary_cp_quarters = 0;
    unit.cohesion_quarters = -12;
    state.cursor.game_turn = 3;
    state.cursor.op_stage = Some(1);
    anchor(
        &mut state,
        if ready {
            "opstage.organization.reorganization"
        } else {
            "opstage.organization.training"
        },
    );
    state.land.replacements.sides = Side::ALL.into_iter().map(SideReplacements::new).collect();
    let required = content
        .tables
        .land
        .training
        .op_stages(cna_tables::land::training::TrainingSubject::InfantryExceptCommando);
    let counter = ReplacementCounter {
        id: COUNTER.into(),
        kind: ReplacementKind::Infantry {
            nation: ReplacementNation::Commonwealth,
        },
        points: 4,
        location: Location::Hex { hex },
        recipient: id.clone(),
        training: if ready {
            Training::Ready
        } else {
            Training::Training {
                required_stages: required.try_into().unwrap(),
                completed: 0,
                last_counted: None,
                candidate: None,
            }
        },
        carriage: None,
        physical: ReplacementPhysical {
            stage: OpStageKey {
                game_turn: 3,
                op_stage: 1,
            },
            cp_quarters: 0,
            cohesion_quarters: -24,
            breakdown_points: 7,
            checked_column: Some(1),
            on_network: false,
            supplies: Supplies::default(),
            tank_fuel: FuelTenths::ZERO,
            water: WaterPoints::ZERO,
            activity: None,
            funding: None,
        },
    };
    side_mut(&mut state, Side::Commonwealth)
        .unwrap()
        .counters
        .push(counter);
    let seat = SeatId::new(
        Side::Commonwealth,
        crate::ownership::seat_for_unit(&content, &state, &id),
    );
    (content, state, id, seat)
}
fn counter(state: &State) -> &ReplacementCounter {
    &side(state, Side::Commonwealth).unwrap().counters[0]
}
fn counter_mut(state: &mut State) -> &mut ReplacementCounter {
    &mut side_mut(state, Side::Commonwealth).unwrap().counters[0]
}
fn candidate(content: &CnaContent, state: &mut State, seat: SeatId) {
    let prepared = training::prepare(
        content,
        state,
        seat,
        &TrainingPlan {
            counters: vec![COUNTER.into()],
        },
    )
    .unwrap();
    training::register(state, seat.side, &prepared).unwrap();
}
fn open_window(content: &CnaContent, state: &mut State) {
    let mut rng = CampaignRng::from_seed([91; 32]);
    let mut events = vec![];
    unit_windows::enter_units(
        content,
        state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
}
fn respond(state: &State, action: Value) -> Command {
    let pending = state
        .decisions
        .pending
        .iter()
        .find(|p| {
            p.seat.side == Side::Commonwealth
                && !matches!(&p.space.schema,
                    cna_core::decision::ActionSchema::Choice { options } if options.is_empty())
        })
        .unwrap();
    Command::Respond(DecisionResponse {
        decision_id: pending.id.clone(),
        seat: pending.seat,
        controller_epoch: 0,
        decision_revision: pending.revision,
        idempotency_key: format!("{}-training-control", pending.id),
        action,
        public_explanation: None,
    })
}
fn game(state: State) -> Game<Cna> {
    Game {
        state,
        rng: CampaignRng::from_seed([92; 32]).state(),
    }
}
fn pass_remaining(content: &CnaContent, mut game: Game<Cna>) -> Game<Cna> {
    // Fixed windows require actual answers from the other five seats, even if empty.
    for _ in 0..6 {
        let Some(pending) = game.state.decisions.pending.first() else {
            return game;
        };
        assert!(matches!(
            pending.kind.as_str(),
            unit_windows::TRAINING | unit_windows::ALLOCATION
        ));
        let command = move_response(&game.state, pending, Value::Null);
        game = evaluate(&Cna::dev(), content, &game, &command)
            .unwrap()
            .game;
    }
    assert!(game.state.decisions.pending.is_empty());
    game
}
fn allocation(id: &UnitId, points: i32) -> AllocationPlan {
    AllocationPlan {
        uses: vec![AllocationUse {
            counter: COUNTER.into(),
            recipient: id.clone(),
            points,
            conversion: "machinegun".into(),
        }],
    }
}

/// Cases: land:17.33, land:20.43, land:20.42
#[test]
fn replacement_training_credit_only_at_complete_tail_once_and_no_idle_recovery() {
    let (content, mut state, _, seat) = fixture(false);
    candidate(&content, &mut state, seat);
    let initial = bytes(&state);
    assert!(training::finish_training_stage(&content, &mut state).is_err());
    assert_eq!(bytes(&state), initial);
    tail(&mut state);
    training::finish_training_stage(&content, &mut state).unwrap();
    assert!(matches!(
        counter(&state).training,
        Training::Training {
            completed: 1,
            last_counted: Some(OpStageKey {
                game_turn: 3,
                op_stage: 1
            }),
            candidate: None,
            ..
        }
    ));
    assert_eq!(counter(&state).physical.cohesion_quarters, -24);
    let counted = bytes(&state);
    training::finish_training_stage(&content, &mut state).unwrap();
    assert_eq!(bytes(&state), counted);
}

/// Cases: land:17.33, land:17.35, land:20.43
#[test]
fn replacement_training_any_cp_and_instructor_loss_remain_interrupted_after_restoration() {
    for loss in [false, true] {
        let (content, mut state, id, seat) = fixture(false);
        candidate(&content, &mut state, seat);
        if loss {
            state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Under { under: 0 });
        } else {
            counter_mut(&mut state).physical.cp_quarters = 1;
        }
        training::invalidate_training(&content, &mut state).unwrap();
        state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Under { under: 3 });
        counter_mut(&mut state).physical.cp_quarters = 0;
        training::invalidate_training(&content, &mut state).unwrap();
        assert!(
            training::prepare(
                &content,
                &state,
                seat,
                &TrainingPlan {
                    counters: vec![COUNTER.into()]
                }
            )
            .is_err()
        );
        tail(&mut state);
        training::finish_training_stage(&content, &mut state).unwrap();
        assert!(matches!(
            counter(&state).training,
            Training::Training {
                completed: 0,
                last_counted: None,
                candidate: None,
                ..
            }
        ));
    }
}

/// Cases: land:17.33, land:17.35, land:20.43
#[test]
fn replacement_training_instructor_return_does_not_erase_real_movement_history() {
    let (content, mut state, id, seat) = fixture(false);
    candidate(&content, &mut state, seat);
    state.cursor.block = crate::seq::Block::PlayerHalf;
    state.cursor.index = crate::seq::PLAYER_HALF
        .iter()
        .position(|s| s.anchor == "opstage.movement_and_combat.movement")
        .unwrap();
    state.cursor.half = Some(crate::seq::Half::A);
    // The movement kernel's completed-prefix history survives a return to the original hex.
    state.land.movement.moved.insert(id);
    training::invalidate_training(&content, &mut state).unwrap();
    tail(&mut state);
    training::finish_training_stage(&content, &mut state).unwrap();
    assert!(matches!(
        counter(&state).training,
        Training::Training { completed: 0, .. }
    ));
}

/// Cases: land:17.33, land:20.43, land:3.6
#[test]
fn replacement_training_canonical_respond_only_buffers_and_rejections_keep_game_rng_pure() {
    let (content, mut state, _, _) = fixture(false);
    open_window(&content, &mut state);
    let game = game(state);
    let initial = bytes(&game);
    let cmd = respond(&game.state, json!({"counters":[COUNTER]}));
    let accepted = evaluate(&Cna::dev(), &content, &game, &cmd).unwrap();
    assert_eq!(
        bytes(counter(&accepted.game.state)),
        bytes(counter(&game.state))
    );
    assert_eq!(accepted.game.rng, game.rng);
    assert!(matches!(
        side(&accepted.game.state, Side::Commonwealth)
            .unwrap()
            .training
            .phase,
        ProcedurePhase::Awaiting {
            submitted: Some(ReplacementPlan::Training(_))
        }
    ));
    assert_eq!(bytes(&game), initial);
    for bad in [
        json!({"counters":[COUNTER,COUNTER]}),
        json!({"counters":["enemy-counter"]}),
        json!({"counters":[COUNTER],"completed":99}),
    ] {
        assert!(evaluate(&Cna::dev(), &content, &game, &respond(&game.state, bad)).is_err());
        assert_eq!(bytes(&game), initial);
    }
    let mut wrong = cmd.clone();
    let Command::Respond(response) = &mut wrong else {
        unreachable!()
    };
    response.seat.role = Role::Air;
    assert!(evaluate(&Cna::dev(), &content, &game, &wrong).is_err());
    assert_eq!(bytes(&game), initial);
    let mut hidden = game.clone();
    counter_mut(&mut hidden.state).physical.breakdown_points += 1;
    crate::testkit::assert_action_indistinguishable(
        &Cna::dev(),
        &content,
        &game,
        &hidden,
        &cmd,
        Side::Axis,
    );
    crate::testkit::assert_actions_indistinguishable(
        &Cna::dev(),
        &content,
        (&game, &cmd),
        (&game, &respond(&game.state, Value::Null)),
        Side::Axis,
    );
    let mut finished = pass_remaining(&content, accepted.game).state;
    unit_windows::finish_units(&content, &mut finished).unwrap();
    assert!(matches!(
        counter(&finished).training,
        Training::Training {
            completed: 0,
            candidate: Some(_),
            ..
        }
    ));
}

/// Cases: land:17.33, land:20.43, land:20.49
#[test]
fn replacement_training_canonical_tail_checkpoint_recovery_matches_events_state_and_rng() {
    let (content, mut state, _, seat) = fixture(false);
    if let Training::Training {
        completed,
        required_stages,
        ..
    } = &mut counter_mut(&mut state).training
    {
        *completed = *required_stages - 1;
    }
    candidate(&content, &mut state, seat);
    tail(&mut state);
    let game = game(state);
    let restored: Game<Cna> = serde_json::from_slice(&bytes(&game)).unwrap();
    let a = evaluate(&Cna::dev(), &content, &game, &Command::Advance).unwrap();
    let b = evaluate(&Cna::dev(), &content, &restored, &Command::Advance).unwrap();
    assert_eq!(bytes(&a.game), bytes(&b.game));
    assert_eq!(bytes(&a.events), bytes(&b.events));
    assert!(matches!(counter(&a.game.state).training, Training::Ready));
    assert_eq!(counter(&a.game.state).physical.cohesion_quarters, -24);
    assert_eq!(counter(&a.game.state).physical.cp_quarters, 0);
}

/// Cases: land:20.3, land:20.41, land:20.48, land:20.49
#[test]
fn replacement_absorption_derives_conversion_keeps_remainder_history_and_charges_exact_quarters() {
    let (content, state, id, seat) = fixture(true);
    let initial = bytes(&state);
    let after = allocation::prepare(&content, &state, seat, &allocation(&id, 2)).unwrap();
    assert_eq!(bytes(&state), initial);
    assert_eq!(
        crate::logistics::toe_strength(&content, &after.land.units[&id])
            .unwrap()
            .get(),
        4
    );
    assert_eq!(after.land.units[&id].cp_spent_quarters, 6);
    assert_eq!(counter(&after).points, 2);
    assert_eq!(counter(&after).id, COUNTER);
    assert_eq!(counter(&after).physical.cp_quarters, 4);
    let mut expected = counter(&state).clone();
    expected.points = 2;
    expected.physical.cp_quarters = 4;
    assert_eq!(bytes(counter(&after)), bytes(&expected));
    assert_eq!(bytes(&after.land.breakdown), bytes(&state.land.breakdown));
    assert_eq!(
        after.land.units[&id].cohesion_quarters,
        state.land.units[&id].cohesion_quarters
    );
}

/// Cases: land:20.3, land:20.41, land:20.43, land:20.49
#[test]
fn replacement_absorption_rejects_partial_conversion_untrained_wrong_nation_role_and_overcapacity()
{
    let (content, state, id, seat) = fixture(true);
    let initial = bytes(&state);
    assert!(allocation::prepare(&content, &state, seat, &allocation(&id, 1)).is_err());
    assert!(
        allocation::prepare(
            &content,
            &state,
            SeatId::new(seat.side, Role::Air),
            &allocation(&id, 2)
        )
        .is_err()
    );
    for reason in 0..4 {
        let mut bad = state.clone();
        match reason {
            0 => {
                counter_mut(&mut bad).training = Training::Training {
                    required_stages: 1,
                    completed: 0,
                    last_counted: None,
                    candidate: None,
                }
            }
            1 => {
                counter_mut(&mut bad).kind = ReplacementKind::Infantry {
                    nation: ReplacementNation::German,
                }
            }
            2 => bad.land.units.get_mut(&id).unwrap().toe = Some(Toe::Normal(NormalToe::N)),
            _ => counter_mut(&mut bad).recipient = UnitId::new("absent-unit"),
        }
        let frozen = bytes(&bad);
        assert!(allocation::prepare(&content, &bad, seat, &allocation(&id, 2)).is_err());
        assert_eq!(bytes(&bad), frozen);
    }
    assert_eq!(bytes(&state), initial);
}

/// Cases: land:20.3, land:20.41, land:20.49, land:3.6
#[test]
fn replacement_allocation_respond_checkpoint_and_late_finish_rollback_are_atomic() {
    let (content, mut state, id, _) = fixture(true);
    open_window(&content, &mut state);
    let game = game(state);
    let cmd = respond(
        &game.state,
        json!({"uses":[{"counter":COUNTER,"recipient":id,"points":2,"conversion":"machinegun"}]}),
    );
    let accepted = evaluate(&Cna::dev(), &content, &game, &cmd).unwrap();
    assert_eq!(
        bytes(counter(&accepted.game.state)),
        bytes(counter(&game.state))
    );
    assert_eq!(
        bytes(&accepted.game.state.land.units),
        bytes(&game.state.land.units)
    );
    assert_eq!(accepted.game.rng, game.rng);
    let accepted = pass_remaining(&content, accepted.game);
    let restored: Game<Cna> = serde_json::from_slice(&bytes(&accepted)).unwrap();
    let mut a = accepted.state.clone();
    let mut b = restored.state;
    unit_windows::finish_units(&content, &mut a).unwrap();
    unit_windows::finish_units(&content, &mut b).unwrap();
    assert_eq!(bytes(&a), bytes(&b));
    let mut invalid = accepted;
    counter_mut(&mut invalid.state).points = 0;
    let frozen = bytes(&invalid);
    assert!(unit_windows::finish_units(&content, &mut invalid.state).is_err());
    assert_eq!(bytes(&invalid), frozen);
    assert!(evaluate(&Cna::dev(), &content, &invalid, &Command::Advance).is_err());
    assert_eq!(bytes(&invalid), frozen);
    let mut hidden = game.clone();
    counter_mut(&mut hidden.state).physical.breakdown_points += 1;
    crate::testkit::assert_action_indistinguishable(
        &Cna::dev(),
        &content,
        &game,
        &hidden,
        &cmd,
        Side::Axis,
    );
}

/// Cases: land:17.33, land:20.43
#[test]
fn replacement_no_candidates_no_counters_is_byte_rng_event_identical() {
    let (content, mut state, _, _) = fixture(false);
    side_mut(&mut state, Side::Commonwealth)
        .unwrap()
        .counters
        .clear();
    let initial = bytes(&state);
    let mut rng = CampaignRng::from_seed([93; 32]);
    let original_rng = rng.state();
    let mut events = vec![];
    training::invalidate_training(&content, &mut state).unwrap();
    training::finish_training_stage(&content, &mut state).unwrap();
    assert_eq!(bytes(&state), initial);
    assert_eq!(rng.state(), original_rng);
    assert!(events.is_empty());
    unit_windows::enter_units(
        &content,
        &mut state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    unit_windows::finish_units(&content, &mut state).unwrap();
    // Lead's fixed-window ruling supersedes only the former no-window expectation.
    // Until six ordinary passes, finish must leave the whole opened State intact.
    assert_eq!(state.decisions.pending.len(), 6);
    assert_eq!(events.len(), 6);
    let opened = bytes(&state);
    unit_windows::finish_units(&content, &mut state).unwrap();
    assert_eq!(bytes(&state), opened);
    assert_eq!(rng.state(), original_rng);
    assert!(
        events
            .iter()
            .all(|event| matches!(event.event, cna_protocol::GameEvent::DecisionOpened { .. }))
    );
}

/// Cases: land:20.43, land:17.33
#[test]
fn replacement_verified_arrival_registers_candidate_without_credit_and_unknown_area_does_not() {
    let (content, mut state, _, _) = fixture(false);
    training::register_arrivals(&content, &mut state, Side::Commonwealth, &[COUNTER.into()])
        .unwrap();
    assert!(matches!(
        counter(&state).training,
        Training::Training {
            completed: 0,
            last_counted: None,
            candidate: Some(_),
            ..
        }
    ));
    let (content, mut unknown, _, _) = fixture(false);
    counter_mut(&mut unknown).location = Location::OffMap { id: "cairo".into() };
    let before = bytes(counter(&unknown));
    training::register_arrivals(
        &content,
        &mut unknown,
        Side::Commonwealth,
        &[COUNTER.into()],
    )
    .unwrap();
    assert_eq!(bytes(counter(&unknown)), before);
}

/// Cases: land:20.41, land:20.45, land:20.48, land:20.49
#[test]
fn replacement_full_absorption_drops_only_consumed_account_without_refund_or_stock_fabrication() {
    let (content, mut state, id, seat) = fixture(true);
    counter_mut(&mut state).physical.funding = Some(FuelFundingAccount {
        side: seat.side,
        segment: super::scheduling::key(&state).segment,
        origin: counter(&state).location.clone(),
        paid_cost: FuelTenths::new(17),
        draws: vec![],
    });
    let partial = allocation::prepare(&content, &state, seat, &allocation(&id, 2)).unwrap();
    assert_eq!(
        bytes(&counter(&partial).physical.funding),
        bytes(&counter(&state).physical.funding)
    );
    let stock = bytes(&state.logistics);
    let full = allocation::prepare(&content, &state, seat, &allocation(&id, 4)).unwrap();
    assert!(side(&full, seat.side).unwrap().counters.is_empty());
    assert_eq!(bytes(&full.logistics), stock);
    assert_eq!(
        crate::logistics::toe_strength(&content, &full.land.units[&id])
            .unwrap()
            .get(),
        5
    );
    counter_mut(&mut state).physical.tank_fuel = FuelTenths::new(1);
    let initial = bytes(&state);
    assert!(
        matches!(allocation::prepare(&content, &state, seat, &allocation(&id, 4)),
        Err(cna_core::engine::Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "land:20.45")
    );
    assert_eq!(bytes(&state), initial);
}

/// Cases: land:17.35, land:20.43
#[test]
fn replacement_revalidation_retains_exact_errors_and_publishes_no_earlier_interruption() {
    for missing_source in [true, false] {
        let (mut content, mut state, id, seat) = fixture(false);
        candidate(&content, &mut state, seat);
        let mut later = counter(&state).clone();
        later.id = "commonwealth.replacement-2".into();
        counter_mut(&mut state).physical.cp_quarters = 1;
        side_mut(&mut state, seat.side)
            .unwrap()
            .counters
            .push(later);
        let expected = if missing_source {
            content.units.units.remove(&id);
            invariant("recipient source missing")
        } else {
            state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Under { under: -1 });
            training::strength_error(
                crate::logistics::SupplyError::Invalid,
                "instructor current strength",
            )
        };
        let frozen = bytes(&state);
        assert_eq!(
            training::invalidate_training(&content, &mut state),
            Err(expected)
        );
        assert_eq!(bytes(&state), frozen);
        assert!(matches!(
            counter(&state).training,
            Training::Training {
                candidate: Some(TrainingCandidate {
                    interrupted: false,
                    ..
                }),
                ..
            }
        ));
    }
}

/// Cases: land:17.35, land:20.43, land:4.46
#[test]
fn replacement_instructor_corruption_precedes_later_eligible_and_first_unknown_is_retained() {
    let (mut content, mut state, id, seat) = fixture(false);
    let early = UnitId::new("aaa.fixture.instructor");
    let mut source = content.units.units[&id].clone();
    source.id = early.clone();
    let mut unit = state.land.units[&id].clone();
    unit.id = early.clone();
    unit.toe = Some(Toe::Under { under: -1 });
    content.units.units.insert(early.clone(), source);
    state.land.units.insert(early.clone(), unit);
    let selected = TrainingPlan {
        counters: vec![COUNTER.into()],
    };
    let frozen = bytes(&state);
    let expected = training::strength_error(
        crate::logistics::SupplyError::Invalid,
        "instructor current strength",
    );
    assert_eq!(
        training::prepare(&content, &state, seat, &selected).unwrap_err(),
        cna_core::engine::Rejection::Engine(expected)
    );
    assert_eq!(bytes(&state), frozen);
    // The first unknown is relevant/cohex; a later unknown must not overwrite it.
    content.units.units.get_mut(&early).unwrap().class = None;
    state.land.units.get_mut(&early).unwrap().toe = Some(Toe::Normal(NormalToe::N));
    let class = content.units.units[&id].class.clone().unwrap();
    content.units.classes.get_mut(&class).unwrap().max_toe = None;
    state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Normal(NormalToe::N));
    assert_eq!(
        training::prepare(&content, &state, seat, &selected).unwrap_err(),
        cna_core::engine::Rejection::Engine(gap("land:17.35", "instructor has no verified class"))
    );
}

/// Cases: land:4.46, land:17.35, land:20.3, land:20.43
#[test]
fn replacement_strength_queries_preserve_invalid_and_original_unsupported_cases() {
    let (mut content, mut state, id, seat) = fixture(false);
    let class = content.units.units[&id].class.clone().unwrap();
    content.units.classes.get_mut(&class).unwrap().max_toe = None;
    state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Normal(NormalToe::N));
    let original = crate::logistics::toe_strength(&content, &state.land.units[&id]).unwrap_err();
    assert!(matches!(
        original,
        crate::logistics::SupplyError::Unsupported { case: "land:4.46" }
    ));
    assert_eq!(
        training::prepare(
            &content,
            &state,
            seat,
            &TrainingPlan {
                counters: vec![COUNTER.into()]
            }
        )
        .unwrap_err(),
        cna_core::engine::Rejection::Engine(gap("land:4.46", "instructor current strength"))
    );
    anchor(&mut state, "opstage.organization.reorganization");
    counter_mut(&mut state).training = Training::Ready;
    let frozen = bytes(&state);
    assert_eq!(
        allocation::prepare(&content, &state, seat, &allocation(&id, 2)).unwrap_err(),
        cna_core::engine::Rejection::Engine(gap("land:4.46", "recipient live strength"))
    );
    assert_eq!(bytes(&state), frozen);
    state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Under { under: -1 });
    let frozen = bytes(&state);
    assert_eq!(
        allocation::prepare(&content, &state, seat, &allocation(&id, 2)).unwrap_err(),
        cna_core::engine::Rejection::Engine(training::strength_error(
            crate::logistics::SupplyError::Invalid,
            "recipient live strength"
        ))
    );
    assert_eq!(bytes(&state), frozen);
}

/// Cases: land:17.33, land:17.35, land:20.43, land:3.6
#[test]
fn replacement_exact_stop_survives_canonical_respond_advance_and_checkpoint_recovery() {
    let (mut content, mut state, id, seat) = fixture(false);
    candidate(&content, &mut state, seat);
    open_window(&content, &mut state);
    content.units.units.remove(&id);
    let game = game(state);
    let restored: Game<Cna> = serde_json::from_slice(&bytes(&game)).unwrap();
    let expected = cna_core::engine::Rejection::Engine(invariant("recipient source missing"));
    for input in [&game, &restored] {
        let frozen = bytes(input);
        assert_eq!(
            evaluate(
                &Cna::dev(),
                &content,
                input,
                &respond(&input.state, Value::Null)
            )
            .unwrap_err(),
            expected
        );
        assert_eq!(bytes(input), frozen);
    }
    let mut tail_game = game.clone();
    tail_game.state.decisions.pending.clear();
    tail(&mut tail_game.state);
    let restored: Game<Cna> = serde_json::from_slice(&bytes(&tail_game)).unwrap();
    for input in [&tail_game, &restored] {
        let frozen = bytes(input);
        assert_eq!(
            evaluate(&Cna::dev(), &content, input, &Command::Advance).unwrap_err(),
            expected
        );
        assert_eq!(bytes(input), frozen);
    }
    let mut wrong = respond(&game.state, Value::Null);
    let frozen = bytes(&game);
    let Command::Respond(response) = &mut wrong else {
        unreachable!()
    };
    response.seat.role = Role::Air;
    assert!(matches!(
        evaluate(&Cna::dev(), &content, &game, &wrong),
        Err(cna_core::engine::Rejection::WrongSeat { .. })
    ));
    assert_eq!(bytes(&game), frozen);
}
fn restored_from(game: &Game<Cna>) -> Game<Cna> {
    serde_json::from_slice(&bytes(game)).unwrap()
}

/// Cases: land:20.3, land:20.41, land:20.49
#[test]
fn replacement_equipment_allocation_sums_every_duplicate_row_and_keeps_distinct_identity() {
    use cna_content::units::WeaponPoints;
    let (mut content, mut state, id, seat) = fixture(true);
    let class = content
        .units
        .classes
        .values()
        .find(|c| c.nation == "cw" && c.unit_type == "tank")
        .unwrap()
        .id
        .clone();
    let weapons: Vec<_> = content
        .units
        .weapons
        .values()
        .filter(|w| w.kind == "tank" && w.nation == "cw")
        .take(2)
        .map(|w| w.id.clone())
        .collect();
    assert_eq!(weapons.len(), 2);
    let source = content.units.units.get_mut(&id).unwrap();
    source.class = Some(class.clone());
    content.units.classes.get_mut(&class).unwrap().max_toe = Some(8);
    let rows = vec![
        WeaponPoints {
            weapon: weapons[0].clone(),
            n: 2,
        },
        WeaponPoints {
            weapon: weapons[0].clone(),
            n: 1,
        },
        WeaponPoints {
            weapon: weapons[1].clone(),
            n: 1,
        },
    ];
    state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Weapons(rows.clone()));
    counter_mut(&mut state).kind = ReplacementKind::Tank {
        nation: ReplacementNation::Commonwealth,
        weapon: weapons[0].clone(),
    };
    let plan = AllocationPlan {
        uses: vec![AllocationUse {
            counter: COUNTER.into(),
            recipient: id.clone(),
            points: 1,
            conversion: "tank".into(),
        }],
    };
    let frozen = bytes(&state);
    let after = allocation::prepare(&content, &state, seat, &plan).unwrap();
    let Some(Toe::Weapons(actual)) = &after.land.units[&id].toe else {
        panic!("equipment identity disappeared")
    };
    assert_eq!(
        actual
            .iter()
            .map(|p| (p.weapon.clone(), p.n))
            .collect::<std::collections::BTreeMap<_, _>>(),
        std::collections::BTreeMap::from([(weapons[0].clone(), 4), (weapons[1].clone(), 1)])
    );
    assert_eq!(
        crate::logistics::toe_strength(&content, &after.land.units[&id])
            .unwrap()
            .get(),
        5
    );
    assert_eq!(bytes(&state), frozen);
    let restored = restored_from(&game(state.clone()));
    assert_eq!(
        bytes(&allocation::prepare(&content, &restored.state, seat, &plan).unwrap()),
        bytes(&after)
    );
    // Each actual row is validated; a negative row cannot cancel a positive row.
    state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Weapons(vec![
        WeaponPoints {
            weapon: weapons[0].clone(),
            n: 5,
        },
        WeaponPoints {
            weapon: weapons[0].clone(),
            n: -1,
        },
    ]));
    assert_eq!(
        allocation::prepare(&content, &state, seat, &plan).unwrap_err(),
        cna_core::engine::Rejection::Engine(training::strength_error(
            crate::logistics::SupplyError::Invalid,
            "recipient live strength"
        ))
    );
    // Missing physical identity is corrupt even if the additive strength still fits.
    state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Weapons(vec![WeaponPoints {
        weapon: "absent-weapon".into(),
        n: 1,
    }]));
    assert_eq!(
        allocation::prepare(&content, &state, seat, &plan).unwrap_err(),
        cna_core::engine::Rejection::Engine(invariant("existing equipment identity absent"))
    );
}

/// An explicitly surveyed two-hex test overlay on the real grid. Production map data stay intact.
struct TrainingMovementOverlay(std::path::PathBuf);
impl TrainingMovementOverlay {
    fn new(content: &mut CnaContent, home: &HexId, next: &HexId) -> Self {
        use cna_content::map::{LineKind, MapContent, SideKind};
        use std::sync::atomic::{AtomicU32, Ordering};
        static SERIAL: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cna-rp-training-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        let data = cna_content::repo_data_dir().join("map");
        for name in ["aliases.csv", "sections.toml", "layers.toml"] {
            std::fs::copy(data.join(name), dir.join(name)).unwrap();
        }
        let mut cells = String::from("hex_id,section,q,r,terrain,flags\n");
        for row in content.map.iter() {
            let terrain = if row.id == *home || row.id == *next {
                "clear"
            } else {
                row.terrain.as_deref().unwrap_or("unclassified")
            };
            cells.push_str(&format!(
                "{},{},{},{},{},{}\n",
                row.id,
                row.section,
                row.axial.q,
                row.axial.r,
                terrain,
                row.flags.join("|")
            ));
        }
        let (a, b) = if home < next {
            (home, next)
        } else {
            (next, home)
        };
        let mut coverage = format!(
            "layer,hex_id,neighbour_id,src,review_batch\nterrain,{a},,land:8.37,test\nterrain,{b},,land:8.37,test\n"
        );
        for kind in LineKind::ALL {
            coverage.push_str(&format!("line:{},{a},{b},land:8.33,test\n", kind.name()));
        }
        for kind in SideKind::ALL {
            coverage.push_str(&format!("side:{},{a},{b},land:8.35,test\n", kind.name()));
        }
        std::fs::write(dir.join("hexes.csv"), cells).unwrap();
        std::fs::write(dir.join("coverage.csv"), coverage).unwrap();
        std::fs::write(
            dir.join("line_features.csv"),
            "from_hex,to_hex,kind,src,review_batch\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("hexsides.csv"),
            "hex_id,direction,neighbour_id,feature,high_side,src,review_batch\n",
        )
        .unwrap();
        content.map = MapContent::load(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for TrainingMovementOverlay {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("cna-rp-training-")
        );
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn move_response(state: &State, pending: &crate::state::Pending, action: Value) -> Command {
    assert!(state.decisions.pending.iter().any(|p| p.id == pending.id));
    Command::Respond(DecisionResponse {
        decision_id: pending.id.clone(),
        seat: pending.seat,
        controller_epoch: 0,
        decision_revision: pending.revision,
        idempotency_key: format!("{}-rp-movement", pending.id),
        action,
        public_explanation: None,
    })
}
fn finish_training_move(
    content: &CnaContent,
    mut transition: cna_core::engine::Transition<Cna>,
    id: &UnitId,
    home: &HexId,
) -> cna_core::engine::Transition<Cna> {
    for _ in 0..20 {
        let request = transition
            .game
            .state
            .decisions
            .pending
            .iter()
            .find(|p| {
                p.kind == crate::land::reaction::KIND || p.kind == crate::land::reaction::CONTINUE
            })
            .cloned();
        let Some(pending) = request else {
            return transition;
        };
        let action = if pending.kind == crate::land::reaction::KIND {
            assert!(
                pending
                    .space
                    .context
                    .as_ref()
                    .is_some_and(|v| v["forced_pass"] == true)
            );
            Value::Null
        } else {
            assert_ne!(
                transition.game.state.land.units[id].location,
                Location::Hex { hex: home.clone() }
            );
            json!([{"unit":id,"path":[home]}])
        };
        let next = evaluate(
            &Cna::dev(),
            content,
            &transition.game,
            &move_response(&transition.game.state, &pending, action),
        )
        .unwrap();
        transition.events.extend(next.events);
        transition.game = next.game;
        transition.progress = next.progress;
    }
    panic!("training movement continuation exceeded the 20-action fixture guard");
}

/// Cases: land:17.33, land:17.35, land:20.43, land:8.11, land:8.13
#[test]
fn replacement_actual_dispatcher_depart_return_interrupts_training_and_replays_exactly() {
    let (mut content, mut state, id, seat) = fixture(false);
    let home = counter(&state).location.hex().unwrap().clone();
    let next = content
        .map
        .neighbors(&home)
        .into_iter()
        .find(|h| content.areas.areas["cairo"].hex_ids.contains(&h.id))
        .unwrap()
        .id
        .clone();
    let _overlay = TrainingMovementOverlay::new(&mut content, &home, &next);
    // Isolate the actual movement dispatcher, not a new published campaign setup.
    for unit in state.land.units.values_mut() {
        if unit.id != id {
            unit.location = Location::Eliminated;
        }
    }
    state.land.units.get_mut(&id).unwrap().detached = true;
    candidate(&content, &mut state, seat);
    state.cursor.block = crate::seq::Block::PlayerHalf;
    state.cursor.index = crate::seq::PLAYER_HALF
        .iter()
        .position(|s| s.anchor == "opstage.movement_and_combat.movement")
        .unwrap();
    state.cursor.half = Some(crate::seq::Half::A);
    state.cursor.entered = false;
    state.turn.player_a = Some(seat.side);
    state.turn.weather = Some(crate::state::WeatherState {
        kind: cna_tables::land::weather::WeatherKind::Normal,
        storm_sections: vec![],
    });
    state.logistics.unit_supply.insert(
        id.clone(),
        crate::state::UnitSupply {
            activity_water: WaterPoints::new(10000),
            ready_ammo: cna_core::quantity::AmmoPoints::new(10000),
            tank_fuel: FuelTenths::new(10000),
            ..crate::state::UnitSupply::default()
        },
    );
    let water_stage = crate::logistics::water::WaterStage::current(&state);
    state.logistics.rations.insert(
        id.clone(),
        crate::logistics::Rations {
            water_stage: Some(water_stage),
            infantry_water_received: 2,
            issued_gt: Some(state.cursor.game_turn),
            pasta_gt: Some(state.cursor.game_turn),
            ..crate::logistics::Rations::default()
        },
    );
    let initial = game(state);
    let opened = evaluate(&Cna::dev(), &content, &initial, &Command::Advance)
        .unwrap()
        .game;
    let request = opened
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == crate::land::movement::KIND && p.seat == seat)
        .unwrap();
    let command = move_response(
        &opened.state,
        request,
        json!([{"unit":id,"path":[next,home]}]),
    );
    let frozen = bytes(&opened);
    let recovered = restored_from(&opened);
    let a = finish_training_move(
        &content,
        evaluate(&Cna::dev(), &content, &opened, &command).unwrap(),
        &id,
        &home,
    );
    let b = finish_training_move(
        &content,
        evaluate(&Cna::dev(), &content, &recovered, &command).unwrap(),
        &id,
        &home,
    );
    assert_eq!(bytes(&a.game), bytes(&b.game));
    assert_eq!(bytes(&a.events), bytes(&b.events));
    assert_eq!(bytes(&opened), frozen);
    assert_eq!(
        a.game.state.land.units[&id].location,
        Location::Hex { hex: home }
    );
    assert!(a.game.state.land.units[&id].cp_spent_quarters > 0);
    assert!(a.game.state.land.movement.moved.contains(&id));
    assert!(matches!(
        counter(&a.game.state).training,
        Training::Training {
            completed: 0,
            candidate: Some(TrainingCandidate {
                interrupted: true,
                ..
            }),
            ..
        }
    ));
    let mut at_tail = a.game;
    at_tail.state.decisions.pending.clear();
    tail(&mut at_tail.state);
    training::finish_training_stage(&content, &mut at_tail.state).unwrap();
    assert!(matches!(
        counter(&at_tail.state).training,
        Training::Training {
            completed: 0,
            last_counted: None,
            ..
        }
    ));
}
