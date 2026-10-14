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
        .find(|p| p.seat.side == Side::Commonwealth)
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
        training::invalidate_training(&content, &mut state);
        state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Under { under: 3 });
        counter_mut(&mut state).physical.cp_quarters = 0;
        training::invalidate_training(&content, &mut state);
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
    training::invalidate_training(&content, &mut state);
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
    let mut finished = accepted.game.state;
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
    let restored: Game<Cna> = serde_json::from_slice(&bytes(&accepted.game)).unwrap();
    let mut a = accepted.game.state.clone();
    let mut b = restored.state;
    unit_windows::finish_units(&content, &mut a).unwrap();
    unit_windows::finish_units(&content, &mut b).unwrap();
    assert_eq!(bytes(&a), bytes(&b));
    let mut invalid = accepted.game;
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
    training::invalidate_training(&content, &mut state);
    training::finish_training_stage(&content, &mut state).unwrap();
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
    assert_eq!(bytes(&state), initial);
    assert_eq!(rng.state(), original_rng);
    assert!(events.is_empty());
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
