use super::*;
use super::scheduling::*;
use cna_core::{dice::CampaignRng, engine::Cx, ids::SeatId, visibility::Audience};
use cna_protocol::Role;

fn scheduling_state(content: &crate::CnaContent, gt: u16) -> crate::State {
    let mut state = crate::State::new(content).unwrap();
    state.cursor.block = crate::seq::Block::Pre;
    state.cursor.index = crate::seq::PRE.iter().position(|step| step.anchor == ANCHOR).unwrap();
    state.cursor.game_turn = gt;
    state.cursor.op_stage = None;
    state.cursor.entered = true;
    let current = key(&state);
    state.land.replacements.sides = Side::ALL.into_iter().map(|side| {
        let mut owned = SideReplacements::new(side);
        owned.planning = ReplacementProcedure { key: Some(current.clone()), phase: ProcedurePhase::Awaiting { submitted: None } };
        owned
    }).collect();
    state
}

fn accept(state: &mut crate::State, owner: Side, plan: &SchedulePlan) {
    side_mut(state, owner).unwrap().planning.phase = ProcedurePhase::Awaiting {
        submitted: Some(ReplacementPlan::Schedule(plan.clone())),
    };
}

/// Cases: land:20.21, land:20.72, scen:60.47
#[test]
fn graziani_calendar_has_no_commonwealth_production_within_its_real_bounds() {
    let c = content();
    for gt in c.bounds.start_gt..=c.bounds.end_gt {
        assert!(arrival_turns(&c, &scheduling_state(&c, gt), Side::Commonwealth).unwrap().is_empty());
    }
    // Extending bounds is a reduced procedure fixture, not additional Graziani entitlement.
    let mut future = content();
    future.bounds.end_gt = 12;
    assert_eq!(arrival_turns(&future, &scheduling_state(&future, 4), Side::Commonwealth).unwrap(), vec![8, 9, 10, 11]);
    assert!(arrival_turns(&future, &scheduling_state(&future, 5), Side::Commonwealth).unwrap().is_empty());
}

/// Cases: land:20.61, land:20.63, land:20.66
#[test]
fn preparation_is_pure_and_determined_finish_debits_before_distribution() {
    let mut c = content();
    c.bounds.end_gt = 14; // Isolated future-date procedure fixture only.
    let mut state = scheduling_state(&c, 9);
    let row = italian_infantry();
    let plan = SchedulePlan { requests: vec![ProductionRequest { row: row.clone(), quantity: ProductionQuantity::Fixed { points: 10 }, arrival_turn: 11 }] };
    let before = serde_json::to_vec(&state).unwrap();
    let prepared = prepare_schedule(&c, &state, SeatId::new(Side::Axis, Role::Logistics), &plan).unwrap();
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    assert!(prepare_schedule(&c, &state, SeatId::new(Side::Axis, Role::FrontLine), &plan).is_err());
    accept(&mut state, Side::Axis, &plan);
    let mut rng = CampaignRng::from_seed([7; 32]);
    let rng_before = rng.state();
    let mut events = vec![];
    finish_schedule(&c, &mut state, &prepared, &mut Cx { rng: &mut rng, events: &mut events }).unwrap();
    let owned = side(&state, Side::Axis).unwrap();
    assert_eq!(owned.budgets[0].pool_committed, 10);
    assert!(owned.orders.is_empty());
    assert!(matches!(owned.planning.phase, ProcedurePhase::Distributing { .. }));
    assert_eq!(rng.state(), rng_before);
    assert!(events.is_empty());
    let snapshot = serde_json::to_vec(&state).unwrap();
    assert!(finish_schedule(&c, &mut state, &prepared, &mut Cx { rng: &mut rng, events: &mut events }).is_err());
    assert_eq!(serde_json::to_vec(&state).unwrap(), snapshot);
    assert_eq!(rng.state(), rng_before);
}

fn random_plan(gt: u16) -> Vec<ProductionRequest> {
    let mut requests: Vec<_> = [FuelTruckKind::Light, FuelTruckKind::Medium, FuelTruckKind::Heavy].into_iter().map(|kind| ProductionRequest {
        row: ProductionRowRef::CommonwealthTruck { column: truck_column(gt).unwrap().0.into(), kind },
        quantity: ProductionQuantity::Roll, arrival_turn: gt,
    }).collect();
    requests.push(ProductionRequest { row: ProductionRowRef::CommonwealthInfantry { column: infantry_column(gt).unwrap().0.into() }, quantity: ProductionQuantity::Roll, arrival_turn: gt });
    requests
}

/// Cases: land:20.73, land:20.74, land:20.78, land:3.6
#[test]
fn random_finish_uses_three_independent_truck_dice_and_two_infantry_dice_per_turn() {
    let mut c = content();
    c.bounds.end_gt = 12; // Reduced fixture for the first genuinely eligible calendar month.
    let mut state = scheduling_state(&c, 4);
    let plan = SchedulePlan { requests: (8..=11).flat_map(random_plan).collect() };
    let prepared = prepare_schedule(&c, &state, SeatId::new(Side::Commonwealth, Role::Logistics), &plan).unwrap();
    let mut rng = CampaignRng::from_seed([21; 32]);
    let mut expected = rng.clone();
    let mut expected_counts = vec![];
    for gt in 8..=11 {
        for kind in [FuelTruckKind::Light, FuelTruckKind::Medium, FuelTruckKind::Heavy] {
            expected_counts.push(c.tables.land.commonwealth_production.trucks(truck_column(gt).unwrap().1, truck(kind), expected.d6().value().into()).unwrap());
        }
        let sum = i32::from(expected.d6().value()) + i32::from(expected.d6().value());
        expected_counts.push(match c.tables.land.commonwealth_production.infantry(infantry_column(gt).unwrap().1, sum).unwrap() {
            cna_tables::land::production::InfantryProductionResult::Points(points) => points,
            cna_tables::land::production::InfantryProductionResult::PrintedNone => 0,
        });
    }
    accept(&mut state, Side::Commonwealth, &plan);
    let mut events = vec![];
    finish_schedule(&c, &mut state, &prepared, &mut Cx { rng: &mut rng, events: &mut events }).unwrap();
    assert_eq!(rng.state(), expected.state());
    let own = side(&state, Side::Commonwealth).unwrap();
    let ProcedurePhase::Distributing { determined, submitted: None } = &own.planning.phase else { panic!("fixed distribution phase missing") };
    assert_eq!(determined.iter().map(|d| d.points).collect::<Vec<_>>(), expected_counts);
    assert!(own.budgets.is_empty());
    assert!(own.orders.is_empty());
    assert_eq!(events.len(), 16);
    assert!(events.iter().all(|event| event.audience == Audience::Seat(SeatId::new(Side::Commonwealth, Role::Logistics))));
    let checkpoint: crate::State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(serde_json::to_vec(&checkpoint).unwrap(), serde_json::to_vec(&state).unwrap());
}

/// Cases: land:20.74
#[test]
fn combined_all_type_distribution_is_even_but_individual_types_need_not_be() {
    let trucks = ProductionRowRef::CommonwealthTruck { column: "gt_1_to_30".into(), kind: FuelTruckKind::Medium };
    let infantry = ProductionRowRef::CommonwealthInfantry { column: "gt_3_to_30".into() };
    let truck_kind = ProducedKind::Trucks { kind: FuelTruckKind::Medium };
    let foot_kind = ProducedKind::Points { kind: ReplacementKind::Infantry { nation: ReplacementNation::Commonwealth } };
    let determined = vec![DeterminedProduction { row: trucks.clone(), kind: truck_kind.clone(), points: 10, arrival_turn: 8 }, DeterminedProduction { row: infantry.clone(), kind: foot_kind.clone(), points: 11, arrival_turn: 8 }];
    let plan = DistributionPlan { batches: vec![
        DistributionBatch { row: trucks.clone(), kind: truck_kind.clone(), points: 7, due: OpStageKey { game_turn: 8, op_stage: 1 } },
        DistributionBatch { row: trucks, kind: truck_kind, points: 3, due: OpStageKey { game_turn: 8, op_stage: 2 } },
        DistributionBatch { row: infantry.clone(), kind: foot_kind.clone(), points: 4, due: OpStageKey { game_turn: 8, op_stage: 2 } },
        DistributionBatch { row: infantry, kind: foot_kind, points: 7, due: OpStageKey { game_turn: 8, op_stage: 3 } },
    ] };
    validate_distribution(Side::Commonwealth, &determined, &plan).unwrap();
    let mut wrong = plan.clone();
    wrong.batches[0].points = 8;
    wrong.batches[1].points = 2;
    assert!(validate_distribution(Side::Commonwealth, &determined, &wrong).is_err());
    wrong = plan.clone();
    wrong.batches.push(wrong.batches[0].clone());
    assert!(validate_distribution(Side::Commonwealth, &determined, &wrong).is_err());
    wrong = plan.clone();
    wrong.batches[0].kind = ProducedKind::Trucks { kind: FuelTruckKind::Heavy };
    assert!(validate_distribution(Side::Commonwealth, &determined, &wrong).is_err());
}

/// Cases: land:20.73, land:20.74
#[test]
fn zero_outcome_still_has_one_distribution_and_cannot_reroll_after_checkpoint() {
    let c = content();
    let mut state = scheduling_state(&c, 4);
    let row = ProductionRowRef::CommonwealthInfantry { column: "gt_3_to_30".into() };
    let empty = DistributionPlan { batches: vec![] };
    side_mut(&mut state, Side::Commonwealth).unwrap().planning.phase = ProcedurePhase::Distributing {
        determined: vec![DeterminedProduction { row, kind: ProducedKind::Points { kind: ReplacementKind::Infantry { nation: ReplacementNation::Commonwealth } }, points: 0, arrival_turn: 8 }],
        submitted: Some(empty.clone()),
    };
    let mut restored: crate::State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    finish_distribution(&mut restored, Side::Commonwealth, &empty).unwrap();
    let own = side(&restored, Side::Commonwealth).unwrap();
    assert!(own.orders.is_empty());
    assert_eq!(own.next_serial, 0);
    assert_eq!(own.cw_arrival_limits.len(), 1);
    assert_eq!((own.cw_arrival_limits[0].game_turn, own.cw_arrival_limits[0].total_trucks), (8, 0));
    assert!(matches!(own.planning.phase, ProcedurePhase::Resolved));
    let before = serde_json::to_vec(&restored).unwrap();
    assert!(finish_distribution(&mut restored, Side::Commonwealth, &empty).is_err());
    assert_eq!(serde_json::to_vec(&restored).unwrap(), before);
}

fn content() -> crate::CnaContent {
    crate::CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}

/// Cases: land:20.74, land:20.78A, land:20.78B
#[test]
fn each_scheduled_gt_retains_all_stage_trucks_and_prunes_only_at_actual_gt_close() {
    let c = content();
    let mut state = scheduling_state(&c, 4);
    let row = ProductionRowRef::CommonwealthTruck { column: "gt_1_to_30".into(), kind: FuelTruckKind::Medium };
    let kind = ProducedKind::Trucks { kind: FuelTruckKind::Medium };
    let determined: Vec<_> = (8..=11).map(|arrival_turn| DeterminedProduction { row: row.clone(), kind: kind.clone(), points: 6, arrival_turn }).collect();
    let plan = DistributionPlan { batches: (8..=11).flat_map(|gt| (1..=3).map({
        let row = row.clone(); let kind = kind.clone();
        move |op_stage| DistributionBatch { row: row.clone(), kind: kind.clone(), points: 2, due: OpStageKey { game_turn: gt, op_stage } }
    })).collect() };
    side_mut(&mut state, Side::Commonwealth).unwrap().planning.phase = ProcedurePhase::Distributing { determined, submitted: Some(plan.clone()) };
    finish_distribution(&mut state, Side::Commonwealth, &plan).unwrap();
    let own = side(&state, Side::Commonwealth).unwrap();
    assert_eq!(own.orders.len(), 12);
    assert_eq!(own.cw_arrival_limits.iter().map(|limit| (limit.game_turn, limit.total_trucks)).collect::<Vec<_>>(), vec![(8,6),(9,6),(10,6),(11,6)]);
    let before = serde_json::to_vec(&state).unwrap();
    assert!(discard_completed_limits(&mut state).is_err());
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    state.cursor.block = crate::seq::Block::Post;
    state.cursor.index = crate::seq::POST.iter().position(|step| step.anchor == "end_of_turn").unwrap();
    state.cursor.game_turn = 8;
    discard_completed_limits(&mut state).unwrap();
    assert_eq!(side(&state, Side::Commonwealth).unwrap().cw_arrival_limits.iter().map(|limit| limit.game_turn).collect::<Vec<_>>(), vec![9,10,11]);
    state.cursor.block = crate::seq::Block::End;
    state.cursor.index = 0;
    state.cursor.game_turn = 11;
    discard_completed_limits(&mut state).unwrap();
    assert!(side(&state, Side::Commonwealth).unwrap().cw_arrival_limits.is_empty());
}

fn italian_infantry() -> ProductionRowRef {
    ProductionRowRef::AxisEquipment { nation: ReplacementNation::Italian, item: "Infantry".into() }
}

fn limit(content: &crate::CnaContent, row: &ProductionRowRef, planning: u16, arrival: u16) -> FiniteLimit {
    finite_limit(&content.tables.land.axis_replacement_pool,
        &content.tables.land.commonwealth_production, row, planning, arrival).unwrap()
}

/// Cases: land:20.61, land:20.66
#[test]
fn consecutive_availability_lines_share_the_printed_pool_without_refill() {
    let c = content();
    let row = italian_infantry();
    let early = limit(&c, &row, 8, 10);
    let later = limit(&c, &row, 9, 11);
    assert_eq!((early.pool_index, early.pool, early.maximum), (0, 100, 5));
    assert_eq!((later.pool_index, later.pool, later.maximum), (0, 100, 10));
    let mut state = SideReplacements::new(Side::Axis);
    state.commit_budget(&row, &early, ProductionBucket::GameTurn { gt: 8 }, 5).unwrap();
    state.commit_budget(&row, &later, ProductionBucket::GameTurn { gt: 9 }, 10).unwrap();
    assert_eq!(state.budgets.len(), 1);
    assert_eq!(state.budgets[0].pool_committed, 15);
    assert_eq!(state.budgets[0].periods.len(), 2);
    let next = limit(&c, &row, 25, 27);
    assert_eq!((next.pool_index, next.pool), (1, 1100));
    state.commit_budget(&row, &next, ProductionBucket::GameTurn { gt: 25 }, 25).unwrap();
    assert_eq!(state.budgets.len(), 2);
    assert_eq!(state.budgets[0].pool_committed, 15);
    assert_eq!(state.budgets[1].pool_committed, 25);
}

/// Cases: land:20.61, land:20.63
#[test]
fn determined_order_debit_refuses_period_and_pool_excess_before_any_mutation() {
    let c = content();
    let row = italian_infantry();
    let printed = limit(&c, &row, 9, 11);
    let mut state = SideReplacements::new(Side::Axis);
    for gt in 9..19 {
        state.commit_budget(&row, &printed, ProductionBucket::GameTurn { gt }, 10).unwrap();
    }
    assert_eq!(state.budgets[0].pool_committed, 100);
    let before = serde_json::to_vec(&state).unwrap();
    assert!(state.commit_budget(&row, &printed, ProductionBucket::GameTurn { gt: 19 }, 1).is_err());
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    let mut state = SideReplacements::new(Side::Axis);
    state.commit_budget(&row, &printed, ProductionBucket::GameTurn { gt: 9 }, 10).unwrap();
    let before = serde_json::to_vec(&state).unwrap();
    assert!(state.commit_budget(&row, &printed, ProductionBucket::GameTurn { gt: 9 }, 1).is_err());
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    assert!(state.commit_budget(&row, &printed, ProductionBucket::Month { year: 1940, month: 9 }, 1).is_err());
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
}

/// Cases: land:20.61, land:20.66, land:20.78
#[test]
fn unreadable_period_printed_dash_and_random_rows_never_become_unlimited() {
    let c = content();
    let row = ProductionRowRef::AxisEquipment { nation: ReplacementNation::German, item: "PzIV E".into() };
    assert!(matches!(finite_limit(&c.tables.land.axis_replacement_pool, &c.tables.land.commonwealth_production, &row, 59, 61),
        Err(EngineError::Unsupported { case, .. }) if case == "land:20.66"));
    assert!(matches!(numeric_limit(&italian_infantry(), 0, 100, ProductionMaximum::PrintedDash),
        Err(EngineError::Unsupported { case, .. }) if case == "land:20.78"));
    let random = ProductionRowRef::CommonwealthInfantry { column: "gt_3_to_30".into() };
    assert!(finite_limit(&c.tables.land.axis_replacement_pool, &c.tables.land.commonwealth_production, &random, 10, 14).is_err());
    assert!(finite_limit(&c.tables.land.axis_replacement_pool, &c.tables.land.commonwealth_production, &italian_infantry(), 4, 6).is_err());
}

/// Cases: land:20.61, land:20.66
#[test]
fn corrupt_duplicate_budgets_and_periods_and_foreign_rows_fail_atomically() {
    let c = content();
    let row = italian_infantry();
    let printed = limit(&c, &row, 9, 11);
    let mut state = SideReplacements::new(Side::Axis);
    state.commit_budget(&row, &printed, ProductionBucket::GameTurn { gt: 9 }, 1).unwrap();
    let good = state.clone();
    state.budgets.push(state.budgets[0].clone());
    let before = serde_json::to_vec(&state).unwrap();
    assert!(state.commit_budget(&row, &printed, ProductionBucket::GameTurn { gt: 9 }, 1).is_err());
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    state = good.clone();
    let duplicate = state.budgets[0].periods[0].clone();
    state.budgets[0].periods.push(duplicate);
    let before = serde_json::to_vec(&state).unwrap();
    assert!(state.commit_budget(&row, &printed, ProductionBucket::GameTurn { gt: 9 }, 1).is_err());
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    state = good;
    let foreign = ProductionRowRef::CommonwealthEquipment { item: "Matilda".into() };
    let before = serde_json::to_vec(&state).unwrap();
    assert!(state.commit_budget(&foreign, &printed, ProductionBucket::GameTurn { gt: 9 }, 1).is_err());
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
}

/// Cases: land:20.21, land:20.42, land:20.61
#[test]
fn absent_legacy_subsystem_is_empty_and_serials_survive_checkpoint_without_reuse() {
    let c = content();
    let mut value = serde_json::to_value(crate::State::new(&c).unwrap()).unwrap();
    value["land"].as_object_mut().unwrap().remove("replacements");
    let restored: crate::State = serde_json::from_value(value).unwrap();
    assert!(restored.land.replacements.sides.is_empty());
    let mut axis = SideReplacements::new(Side::Axis);
    let a = axis.allocate_id().unwrap();
    let mut restored: SideReplacements = serde_json::from_value(serde_json::to_value(axis).unwrap()).unwrap();
    let b = restored.allocate_id().unwrap();
    assert_eq!(a, "axis.replacement-1");
    assert_eq!(b, "axis.replacement-2");
    assert_eq!(SideReplacements::new(Side::Commonwealth).allocate_id().unwrap(), "commonwealth.replacement-1");
    restored.next_serial = u64::MAX;
    let before = serde_json::to_vec(&restored).unwrap();
    assert!(restored.allocate_id().is_err());
    assert_eq!(serde_json::to_vec(&restored).unwrap(), before);
}
