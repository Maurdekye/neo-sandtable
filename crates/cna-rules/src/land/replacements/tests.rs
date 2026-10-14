use super::scheduling::*;
use super::*;
use cna_core::{dice::CampaignRng, engine::Cx, ids::SeatId, visibility::Audience};
use cna_protocol::Role;

fn scheduling_state(content: &crate::CnaContent, gt: u16) -> crate::State {
    let mut state = crate::State::new(content).unwrap();
    state.cursor.block = crate::seq::Block::Pre;
    state.cursor.index = crate::seq::PRE
        .iter()
        .position(|step| step.anchor == ANCHOR)
        .unwrap();
    state.cursor.game_turn = gt;
    state.cursor.op_stage = None;
    state.cursor.entered = true;
    let current = key(&state);
    state.land.replacements.sides = Side::ALL
        .into_iter()
        .map(|side| {
            let mut owned = SideReplacements::new(side);
            owned.planning = ReplacementProcedure {
                key: Some(current.clone()),
                phase: ProcedurePhase::Awaiting { submitted: None },
            };
            owned
        })
        .collect();
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
        assert!(
            arrival_turns(&c, &scheduling_state(&c, gt), Side::Commonwealth)
                .unwrap()
                .is_empty()
        );
    }
    // Extending bounds is a reduced procedure fixture, not additional Graziani entitlement.
    let mut future = content();
    future.bounds.end_gt = 12;
    assert_eq!(
        arrival_turns(&future, &scheduling_state(&future, 4), Side::Commonwealth).unwrap(),
        vec![8, 9, 10, 11]
    );
    assert!(
        arrival_turns(&future, &scheduling_state(&future, 5), Side::Commonwealth)
            .unwrap()
            .is_empty()
    );
}

/// Cases: land:20.61, land:20.63, land:20.66
#[test]
fn preparation_is_pure_and_determined_finish_debits_before_distribution() {
    let mut c = content();
    c.bounds.end_gt = 14; // Isolated future-date procedure fixture only.
    let mut state = scheduling_state(&c, 9);
    let row = italian_infantry();
    let plan = SchedulePlan {
        requests: vec![ProductionRequest {
            row: row.clone(),
            quantity: ProductionQuantity::Fixed { points: 10 },
            arrival_turn: 11,
        }],
    };
    let before = serde_json::to_vec(&state).unwrap();
    let prepared =
        prepare_schedule(&c, &state, SeatId::new(Side::Axis, Role::Logistics), &plan).unwrap();
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    assert!(prepare_schedule(&c, &state, SeatId::new(Side::Axis, Role::FrontLine), &plan).is_err());
    accept(&mut state, Side::Axis, &plan);
    let mut rng = CampaignRng::from_seed([7; 32]);
    let rng_before = rng.state();
    let mut events = vec![];
    finish_schedule(
        &c,
        &mut state,
        &prepared,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    let owned = side(&state, Side::Axis).unwrap();
    assert_eq!(owned.budgets[0].pool_committed, 10);
    assert!(owned.orders.is_empty());
    assert!(matches!(
        owned.planning.phase,
        ProcedurePhase::Distributing { .. }
    ));
    assert_eq!(rng.state(), rng_before);
    assert!(events.is_empty());
    let snapshot = serde_json::to_vec(&state).unwrap();
    assert!(
        finish_schedule(
            &c,
            &mut state,
            &prepared,
            &mut Cx {
                rng: &mut rng,
                events: &mut events
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_vec(&state).unwrap(), snapshot);
    assert_eq!(rng.state(), rng_before);
}

fn random_plan(gt: u16) -> Vec<ProductionRequest> {
    let mut requests: Vec<_> = [
        FuelTruckKind::Light,
        FuelTruckKind::Medium,
        FuelTruckKind::Heavy,
    ]
    .into_iter()
    .map(|kind| ProductionRequest {
        row: ProductionRowRef::CommonwealthTruck {
            column: truck_column(gt).unwrap().0.into(),
            kind,
        },
        quantity: ProductionQuantity::Roll,
        arrival_turn: gt,
    })
    .collect();
    requests.push(ProductionRequest {
        row: ProductionRowRef::CommonwealthInfantry {
            column: infantry_column(gt).unwrap().0.into(),
        },
        quantity: ProductionQuantity::Roll,
        arrival_turn: gt,
    });
    requests
}

/// Cases: land:20.73, land:20.74, land:20.78, land:3.6
#[test]
fn random_finish_uses_three_independent_truck_dice_and_two_infantry_dice_per_turn() {
    let mut c = content();
    c.bounds.end_gt = 12; // Reduced fixture for the first genuinely eligible calendar month.
    let mut state = scheduling_state(&c, 4);
    let plan = SchedulePlan {
        requests: (8..=11).flat_map(random_plan).collect(),
    };
    let prepared = prepare_schedule(
        &c,
        &state,
        SeatId::new(Side::Commonwealth, Role::Logistics),
        &plan,
    )
    .unwrap();
    let mut rng = CampaignRng::from_seed([21; 32]);
    let mut expected = rng.clone();
    let mut expected_counts = vec![];
    for gt in 8..=11 {
        for kind in [
            FuelTruckKind::Light,
            FuelTruckKind::Medium,
            FuelTruckKind::Heavy,
        ] {
            expected_counts.push(
                c.tables
                    .land
                    .commonwealth_production
                    .trucks(
                        truck_column(gt).unwrap().1,
                        truck(kind),
                        expected.d6().value().into(),
                    )
                    .unwrap(),
            );
        }
        let sum = i32::from(expected.d6().value()) + i32::from(expected.d6().value());
        expected_counts.push(
            match c
                .tables
                .land
                .commonwealth_production
                .infantry(infantry_column(gt).unwrap().1, sum)
                .unwrap()
            {
                cna_tables::land::production::InfantryProductionResult::Points(points) => points,
                cna_tables::land::production::InfantryProductionResult::PrintedNone => 0,
            },
        );
    }
    accept(&mut state, Side::Commonwealth, &plan);
    let mut events = vec![];
    finish_schedule(
        &c,
        &mut state,
        &prepared,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert_eq!(rng.state(), expected.state());
    let own = side(&state, Side::Commonwealth).unwrap();
    let ProcedurePhase::Distributing {
        determined,
        submitted: None,
    } = &own.planning.phase
    else {
        panic!("fixed distribution phase missing")
    };
    assert_eq!(
        determined.iter().map(|d| d.points).collect::<Vec<_>>(),
        expected_counts
    );
    assert!(own.budgets.is_empty());
    assert!(own.orders.is_empty());
    assert_eq!(events.len(), 16);
    assert!(
        events.iter().all(|event| event.audience
            == Audience::Seat(SeatId::new(Side::Commonwealth, Role::Logistics)))
    );
    let checkpoint: crate::State =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_vec(&checkpoint).unwrap(),
        serde_json::to_vec(&state).unwrap()
    );
}

/// Cases: land:20.74
#[test]
fn combined_all_type_distribution_is_even_but_individual_types_need_not_be() {
    let trucks = ProductionRowRef::CommonwealthTruck {
        column: "gt_1_to_30".into(),
        kind: FuelTruckKind::Medium,
    };
    let infantry = ProductionRowRef::CommonwealthInfantry {
        column: "gt_3_to_30".into(),
    };
    let truck_kind = ProducedKind::Trucks {
        kind: FuelTruckKind::Medium,
    };
    let foot_kind = ProducedKind::Points {
        kind: ReplacementKind::Infantry {
            nation: ReplacementNation::Commonwealth,
        },
    };
    let determined = vec![
        DeterminedProduction {
            row: trucks.clone(),
            kind: truck_kind.clone(),
            points: 10,
            arrival_turn: 8,
        },
        DeterminedProduction {
            row: infantry.clone(),
            kind: foot_kind.clone(),
            points: 11,
            arrival_turn: 8,
        },
    ];
    let plan = DistributionPlan {
        batches: vec![
            DistributionBatch {
                row: trucks.clone(),
                kind: truck_kind.clone(),
                points: 7,
                due: OpStageKey {
                    game_turn: 8,
                    op_stage: 1,
                },
            },
            DistributionBatch {
                row: trucks,
                kind: truck_kind,
                points: 3,
                due: OpStageKey {
                    game_turn: 8,
                    op_stage: 2,
                },
            },
            DistributionBatch {
                row: infantry.clone(),
                kind: foot_kind.clone(),
                points: 4,
                due: OpStageKey {
                    game_turn: 8,
                    op_stage: 2,
                },
            },
            DistributionBatch {
                row: infantry,
                kind: foot_kind,
                points: 7,
                due: OpStageKey {
                    game_turn: 8,
                    op_stage: 3,
                },
            },
        ],
    };
    validate_distribution(Side::Commonwealth, &determined, &plan).unwrap();
    let mut wrong = plan.clone();
    wrong.batches[0].points = 8;
    wrong.batches[1].points = 2;
    assert!(validate_distribution(Side::Commonwealth, &determined, &wrong).is_err());
    wrong = plan.clone();
    wrong.batches.push(wrong.batches[0].clone());
    assert!(validate_distribution(Side::Commonwealth, &determined, &wrong).is_err());
    wrong = plan.clone();
    wrong.batches[0].kind = ProducedKind::Trucks {
        kind: FuelTruckKind::Heavy,
    };
    assert!(validate_distribution(Side::Commonwealth, &determined, &wrong).is_err());
}

/// Cases: land:20.73, land:20.74
#[test]
fn zero_outcome_still_has_one_distribution_and_cannot_reroll_after_checkpoint() {
    let c = content();
    let mut state = scheduling_state(&c, 4);
    let row = ProductionRowRef::CommonwealthInfantry {
        column: "gt_3_to_30".into(),
    };
    let empty = DistributionPlan { batches: vec![] };
    side_mut(&mut state, Side::Commonwealth)
        .unwrap()
        .planning
        .phase = ProcedurePhase::Distributing {
        determined: vec![DeterminedProduction {
            row,
            kind: ProducedKind::Points {
                kind: ReplacementKind::Infantry {
                    nation: ReplacementNation::Commonwealth,
                },
            },
            points: 0,
            arrival_turn: 8,
        }],
        submitted: Some(empty.clone()),
    };
    let mut restored: crate::State =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    finish_distribution(&mut restored, Side::Commonwealth, &empty).unwrap();
    let own = side(&restored, Side::Commonwealth).unwrap();
    assert!(own.orders.is_empty());
    assert_eq!(own.next_serial, 0);
    assert_eq!(own.cw_arrival_limits.len(), 1);
    assert_eq!(
        (
            own.cw_arrival_limits[0].game_turn,
            own.cw_arrival_limits[0].total_trucks
        ),
        (8, 0)
    );
    assert!(matches!(own.planning.phase, ProcedurePhase::Resolved));
    let before = serde_json::to_vec(&restored).unwrap();
    assert!(finish_distribution(&mut restored, Side::Commonwealth, &empty).is_err());
    assert_eq!(serde_json::to_vec(&restored).unwrap(), before);
}

fn content() -> crate::CnaContent {
    crate::CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}

/// Cases: land:20.74, land:20.78
/// Chart clauses: 20.78A (trucks) and 20.78B (infantry).
#[test]
fn each_scheduled_gt_retains_all_stage_trucks_and_prunes_only_at_actual_gt_close() {
    let c = content();
    let mut state = scheduling_state(&c, 4);
    let row = ProductionRowRef::CommonwealthTruck {
        column: "gt_1_to_30".into(),
        kind: FuelTruckKind::Medium,
    };
    let kind = ProducedKind::Trucks {
        kind: FuelTruckKind::Medium,
    };
    let determined: Vec<_> = (8..=11)
        .map(|arrival_turn| DeterminedProduction {
            row: row.clone(),
            kind: kind.clone(),
            points: 6,
            arrival_turn,
        })
        .collect();
    let plan = DistributionPlan {
        batches: (8..=11)
            .flat_map(|gt| {
                (1..=3).map({
                    let row = row.clone();
                    let kind = kind.clone();
                    move |op_stage| DistributionBatch {
                        row: row.clone(),
                        kind: kind.clone(),
                        points: 2,
                        due: OpStageKey {
                            game_turn: gt,
                            op_stage,
                        },
                    }
                })
            })
            .collect(),
    };
    side_mut(&mut state, Side::Commonwealth)
        .unwrap()
        .planning
        .phase = ProcedurePhase::Distributing {
        determined,
        submitted: Some(plan.clone()),
    };
    finish_distribution(&mut state, Side::Commonwealth, &plan).unwrap();
    let own = side(&state, Side::Commonwealth).unwrap();
    assert_eq!(own.orders.len(), 12);
    assert_eq!(
        own.cw_arrival_limits
            .iter()
            .map(|limit| (limit.game_turn, limit.total_trucks))
            .collect::<Vec<_>>(),
        vec![(8, 6), (9, 6), (10, 6), (11, 6)]
    );
    let before = serde_json::to_vec(&state).unwrap();
    assert!(discard_completed_limits(&mut state).is_err());
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    state.cursor.block = crate::seq::Block::Post;
    state.cursor.index = crate::seq::POST
        .iter()
        .position(|step| step.anchor == "end_of_turn")
        .unwrap();
    state.cursor.game_turn = 8;
    discard_completed_limits(&mut state).unwrap();
    assert_eq!(
        side(&state, Side::Commonwealth)
            .unwrap()
            .cw_arrival_limits
            .iter()
            .map(|limit| limit.game_turn)
            .collect::<Vec<_>>(),
        vec![9, 10, 11]
    );
    state.cursor.block = crate::seq::Block::End;
    state.cursor.index = 0;
    state.cursor.game_turn = 11;
    discard_completed_limits(&mut state).unwrap();
    assert!(
        side(&state, Side::Commonwealth)
            .unwrap()
            .cw_arrival_limits
            .is_empty()
    );
}

fn italian_infantry() -> ProductionRowRef {
    ProductionRowRef::AxisEquipment {
        nation: ReplacementNation::Italian,
        item: "Infantry".into(),
    }
}

fn limit(
    content: &crate::CnaContent,
    row: &ProductionRowRef,
    planning: u16,
    arrival: u16,
) -> FiniteLimit {
    finite_limit(
        &content.tables.land.axis_replacement_pool,
        &content.tables.land.commonwealth_production,
        row,
        planning,
        arrival,
    )
    .unwrap()
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
    state
        .commit_budget(&row, &early, ProductionBucket::GameTurn { gt: 8 }, 5)
        .unwrap();
    state
        .commit_budget(&row, &later, ProductionBucket::GameTurn { gt: 9 }, 10)
        .unwrap();
    assert_eq!(state.budgets.len(), 1);
    assert_eq!(state.budgets[0].pool_committed, 15);
    assert_eq!(state.budgets[0].periods.len(), 2);
    let next = limit(&c, &row, 25, 27);
    assert_eq!((next.pool_index, next.pool), (1, 1100));
    state
        .commit_budget(&row, &next, ProductionBucket::GameTurn { gt: 25 }, 25)
        .unwrap();
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
        state
            .commit_budget(&row, &printed, ProductionBucket::GameTurn { gt }, 10)
            .unwrap();
    }
    assert_eq!(state.budgets[0].pool_committed, 100);
    let before = serde_json::to_vec(&state).unwrap();
    assert!(
        state
            .commit_budget(&row, &printed, ProductionBucket::GameTurn { gt: 19 }, 1)
            .is_err()
    );
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    let mut state = SideReplacements::new(Side::Axis);
    state
        .commit_budget(&row, &printed, ProductionBucket::GameTurn { gt: 9 }, 10)
        .unwrap();
    let before = serde_json::to_vec(&state).unwrap();
    assert!(
        state
            .commit_budget(&row, &printed, ProductionBucket::GameTurn { gt: 9 }, 1)
            .is_err()
    );
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    assert!(
        state
            .commit_budget(
                &row,
                &printed,
                ProductionBucket::Month {
                    year: 1940,
                    month: 9
                },
                1
            )
            .is_err()
    );
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
}

/// Cases: land:20.61, land:20.66, land:20.78
#[test]
fn unreadable_period_printed_dash_and_random_rows_never_become_unlimited() {
    let c = content();
    let row = ProductionRowRef::AxisEquipment {
        nation: ReplacementNation::German,
        item: "PzIV E".into(),
    };
    assert!(
        matches!(finite_limit(&c.tables.land.axis_replacement_pool, &c.tables.land.commonwealth_production, &row, 59, 61),
        Err(EngineError::Unsupported { case, .. }) if case == "land:20.66")
    );
    assert!(
        matches!(numeric_limit(&italian_infantry(), 0, 100, ProductionMaximum::PrintedDash),
        Err(EngineError::Unsupported { case, .. }) if case == "land:20.78")
    );
    let random = ProductionRowRef::CommonwealthInfantry {
        column: "gt_3_to_30".into(),
    };
    assert!(
        finite_limit(
            &c.tables.land.axis_replacement_pool,
            &c.tables.land.commonwealth_production,
            &random,
            10,
            14
        )
        .is_err()
    );
    assert!(
        finite_limit(
            &c.tables.land.axis_replacement_pool,
            &c.tables.land.commonwealth_production,
            &italian_infantry(),
            4,
            6
        )
        .is_err()
    );
}

/// Cases: land:20.61, land:20.66
#[test]
fn corrupt_duplicate_budgets_and_periods_and_foreign_rows_fail_atomically() {
    let c = content();
    let row = italian_infantry();
    let printed = limit(&c, &row, 9, 11);
    let mut state = SideReplacements::new(Side::Axis);
    state
        .commit_budget(&row, &printed, ProductionBucket::GameTurn { gt: 9 }, 1)
        .unwrap();
    let good = state.clone();
    state.budgets.push(state.budgets[0].clone());
    let before = serde_json::to_vec(&state).unwrap();
    assert!(
        state
            .commit_budget(&row, &printed, ProductionBucket::GameTurn { gt: 9 }, 1)
            .is_err()
    );
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    state = good.clone();
    let duplicate = state.budgets[0].periods[0].clone();
    state.budgets[0].periods.push(duplicate);
    let before = serde_json::to_vec(&state).unwrap();
    assert!(
        state
            .commit_budget(&row, &printed, ProductionBucket::GameTurn { gt: 9 }, 1)
            .is_err()
    );
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    state = good;
    let foreign = ProductionRowRef::CommonwealthEquipment {
        item: "Matilda".into(),
    };
    let before = serde_json::to_vec(&state).unwrap();
    assert!(
        state
            .commit_budget(&foreign, &printed, ProductionBucket::GameTurn { gt: 9 }, 1)
            .is_err()
    );
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
}

/// Cases: land:20.21, land:20.42, land:20.61
#[test]
fn absent_legacy_subsystem_is_empty_and_serials_survive_checkpoint_without_reuse() {
    let c = content();
    let mut value = serde_json::to_value(crate::State::new(&c).unwrap()).unwrap();
    value["land"]
        .as_object_mut()
        .unwrap()
        .remove("replacements");
    let restored: crate::State = serde_json::from_value(value).unwrap();
    assert!(restored.land.replacements.sides.is_empty());
    let mut axis = SideReplacements::new(Side::Axis);
    let a = axis.allocate_id().unwrap();
    let mut restored: SideReplacements =
        serde_json::from_value(serde_json::to_value(axis).unwrap()).unwrap();
    let b = restored.allocate_id().unwrap();
    assert_eq!(a, "axis.replacement-1");
    assert_eq!(b, "axis.replacement-2");
    assert_eq!(
        SideReplacements::new(Side::Commonwealth)
            .allocate_id()
            .unwrap(),
        "commonwealth.replacement-1"
    );
    restored.next_serial = u64::MAX;
    let before = serde_json::to_vec(&restored).unwrap();
    assert!(restored.allocate_id().is_err());
    assert_eq!(serde_json::to_vec(&restored).unwrap(), before);
}

fn open_schedule(c: &crate::CnaContent, state: &mut crate::State) {
    state.decisions.pending.clear();
    state.land.replacements.sides.clear();
    let mut rng = CampaignRng::from_seed([51; 32]);
    let mut events = vec![];
    windows::enter_schedule(
        c,
        state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
}

fn production_command(
    state: &crate::State,
    owner: Side,
    kind: &str,
    action: serde_json::Value,
) -> cna_core::engine::Command {
    let pending = state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == owner && p.kind == kind)
        .unwrap();
    cna_core::engine::Command::Respond(cna_core::decision::DecisionResponse {
        decision_id: pending.id.clone(),
        seat: pending.seat,
        controller_epoch: 0,
        decision_revision: pending.revision,
        idempotency_key: format!("{}-production", pending.id),
        action,
        public_explanation: None,
    })
}

// Side-level visibility intentionally includes all that side's seats. These controls use
// an exact seat perspective so a teammate cannot inherit Logistics' private authority.
pub(super) fn seat_snapshot(
    c: &crate::CnaContent,
    state: &crate::State,
    seat: SeatId,
    targets: &BTreeSet<String>,
) -> serde_json::Value {
    use cna_core::{engine::Ruleset, visibility::Perspective};
    let rules = crate::Cna::dev();
    let perspective = Perspective::Seat(seat);
    let inspected: serde_json::Map<String, serde_json::Value> = targets
        .iter()
        .map(|id| {
            let result = rules
                .inspect(c, state, perspective, id)
                .unwrap_or_else(|error| serde_json::json!({"rejected":format!("{error:?}")}));
            (id.clone(), result)
        })
        .collect();
    serde_json::json!({
        "observe":rules.observe(c, state, perspective), "view":rules.view(c, state, perspective),
        "inspect":inspected,
        "pending":rules.pending(c, state).into_iter().filter(|p| p.seat == seat).collect::<Vec<_>>()
    })
}

pub(super) fn assert_private_transition(
    c: &crate::CnaContent,
    owner: Side,
    a: &cna_core::engine::Transition<crate::Cna>,
    b: &cna_core::engine::Transition<crate::Cna>,
) {
    use cna_core::visibility::Perspective;
    let targets = crate::testkit::inspect_targets(&[&a.game.state, &b.game.state]);
    for observer in Side::ALL {
        for role in Role::ALL {
            let seat = SeatId::new(observer, role);
            if seat == SeatId::new(owner, Role::Logistics) {
                continue;
            }
            assert_eq!(
                seat_snapshot(c, &a.game.state, seat, &targets),
                seat_snapshot(c, &b.game.state, seat, &targets),
                "private replacement fact leaked to {seat}"
            );
            let visible = |t: &cna_core::engine::Transition<crate::Cna>| {
                t.events
                    .iter()
                    .filter(|e| Perspective::Seat(seat).can_see(&e.audience))
                    .map(|e| serde_json::json!({"event":e.event,"hex":e.hex,"unit_id":e.unit_id}))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                visible(a),
                visible(b),
                "private replacement stream leaked to {seat}"
            );
        }
    }
    assert_eq!(a.progress, b.progress);
}

/// Cases: land:20.61, land:20.66, land:20.73, land:3.6
#[test]
fn actual_scheduling_respond_is_buffer_only_for_finite_and_random_production() {
    use cna_core::engine::{Game, evaluate};
    for (gt, owner, action) in [
        (
            9,
            Side::Axis,
            serde_json::json!({"requests":[{"family":"axis_equipment","nation":"italian","item":"Infantry","points":5,"arrival_turn":11}]}),
        ),
        (4, Side::Commonwealth, serde_json::Value::Null),
    ] {
        let mut c = content();
        c.bounds.end_gt = 14; // Reduced future-calendar procedure fixture, not Graziani entitlement.
        let mut state = scheduling_state(&c, gt);
        open_schedule(&c, &mut state);
        let game = Game::<crate::Cna> {
            state,
            rng: CampaignRng::from_seed([52; 32]).state(),
        };
        let before_land = serde_json::to_vec(&game.state.land.units).unwrap();
        let before_other = serde_json::to_vec(
            side(
                &game.state,
                if owner == Side::Axis {
                    Side::Commonwealth
                } else {
                    Side::Axis
                },
            )
            .unwrap(),
        )
        .unwrap();
        let before_logistics = serde_json::to_vec(&game.state.logistics).unwrap();
        let before = side(&game.state, owner).unwrap().clone();
        let result = evaluate(
            &crate::Cna::dev(),
            &c,
            &game,
            &production_command(&game.state, owner, windows::SCHEDULE, action),
        )
        .unwrap();
        let after = side(&result.game.state, owner).unwrap();
        assert!(matches!(
            after.planning.phase,
            ProcedurePhase::Awaiting {
                submitted: Some(ReplacementPlan::Schedule(_))
            }
        ));
        let mut without_answer = after.clone();
        without_answer.planning = before.planning.clone();
        assert_eq!(
            serde_json::to_vec(&without_answer).unwrap(),
            serde_json::to_vec(&before).unwrap()
        );
        assert_eq!(
            serde_json::to_vec(&result.game.state.land.units).unwrap(),
            before_land
        );
        assert_eq!(
            serde_json::to_vec(
                side(
                    &result.game.state,
                    if owner == Side::Axis {
                        Side::Commonwealth
                    } else {
                        Side::Axis
                    }
                )
                .unwrap()
            )
            .unwrap(),
            before_other
        );
        assert_eq!(
            serde_json::to_vec(&result.game.state.logistics).unwrap(),
            before_logistics
        );
        assert_eq!(result.game.rng, game.rng);
        assert!(
            !result
                .events
                .iter()
                .any(|event| matches!(event.event, cna_protocol::GameEvent::DiceRolled { .. }))
        );
    }
}

/// Cases: land:20.61, land:20.74, land:3.6
#[test]
fn actual_private_orders_and_zero_nonzero_distribution_have_fixed_observer_streams() {
    use cna_core::engine::{Command, Game, evaluate};
    let mut c = content();
    c.bounds.end_gt = 14;
    let mut state = scheduling_state(&c, 9);
    open_schedule(&c, &mut state);
    let base = Game::<crate::Cna> {
        state,
        rng: CampaignRng::from_seed([53; 32]).state(),
    };
    let empty = production_command(
        &base.state,
        Side::Axis,
        windows::SCHEDULE,
        serde_json::Value::Null,
    );
    let positive = production_command(
        &base.state,
        Side::Axis,
        windows::SCHEDULE,
        serde_json::json!({"requests":[{"family":"axis_equipment","nation":"italian","item":"Infantry","points":5,"arrival_turn":11}]}),
    );
    let answered_a = evaluate(&crate::Cna::dev(), &c, &base, &empty).unwrap();
    let answered_b = evaluate(&crate::Cna::dev(), &c, &base, &positive).unwrap();
    assert_private_transition(&c, Side::Axis, &answered_a, &answered_b);
    let a = evaluate(&crate::Cna::dev(), &c, &answered_a.game, &Command::Advance).unwrap();
    let b = evaluate(&crate::Cna::dev(), &c, &answered_b.game, &Command::Advance).unwrap();
    assert_private_transition(&c, Side::Axis, &a, &b);
    let public_windows = |state: &crate::State| {
        state
            .decisions
            .pending
            .iter()
            .map(|p| (p.seat, p.kind.clone(), p.summary.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(public_windows(&a.game.state), public_windows(&b.game.state));
    assert_eq!(a.game.state.decisions.pending.len(), 1);
    let targets = crate::testkit::inspect_targets(&[&a.game.state, &b.game.state]);
    let owner = SeatId::new(Side::Axis, Role::Logistics);
    assert_ne!(
        seat_snapshot(&c, &a.game.state, owner, &targets),
        seat_snapshot(&c, &b.game.state, owner, &targets)
    );
    let pass_a = production_command(
        &a.game.state,
        Side::Axis,
        windows::DISTRIBUTION,
        serde_json::Value::Null,
    );
    let pass_b = production_command(
        &b.game.state,
        Side::Axis,
        windows::DISTRIBUTION,
        serde_json::Value::Null,
    );
    let buffered_a = evaluate(&crate::Cna::dev(), &c, &a.game, &pass_a).unwrap();
    let buffered_b = evaluate(&crate::Cna::dev(), &c, &b.game, &pass_b).unwrap();
    assert_private_transition(&c, Side::Axis, &buffered_a, &buffered_b);
    // Stop at the owned finish boundary: subsequent Axis shipping remains Unsupported.
    let mut orders_a = buffered_a.game.state.clone();
    let mut orders_b = buffered_b.game.state.clone();
    finish_distribution(
        &mut orders_a,
        Side::Axis,
        &DistributionPlan { batches: vec![] },
    )
    .unwrap();
    let ProcedurePhase::Distributing {
        submitted: Some(plan),
        ..
    } = side(&orders_b, Side::Axis).unwrap().planning.phase.clone()
    else {
        unreachable!()
    };
    finish_distribution(&mut orders_b, Side::Axis, &plan).unwrap();
    assert!(side(&orders_a, Side::Axis).unwrap().orders.is_empty());
    assert_eq!(
        side(&orders_b, Side::Axis)
            .unwrap()
            .orders
            .iter()
            .map(|order| order.points)
            .sum::<i32>(),
        5
    );
    let targets = crate::testkit::inspect_targets(&[&orders_a, &orders_b]);
    for observer in Side::ALL {
        for role in Role::ALL {
            let seat = SeatId::new(observer, role);
            if seat != owner {
                assert_eq!(
                    seat_snapshot(&c, &orders_a, seat, &targets),
                    seat_snapshot(&c, &orders_b, seat, &targets)
                );
            }
        }
    }
}

/// Cases: land:20.73, land:20.74, land:3.6
#[test]
fn actual_raw_rolls_are_private_to_logistics_including_from_same_side_teammates() {
    use cna_core::engine::{Command, Game, evaluate};
    let mut c = content();
    c.bounds.end_gt = 12;
    let mut state = scheduling_state(&c, 4);
    open_schedule(&c, &mut state);
    let mut game = Game::<crate::Cna> {
        state,
        rng: CampaignRng::from_seed([54; 32]).state(),
    };
    for owner in Side::ALL {
        let cmd = production_command(
            &game.state,
            owner,
            windows::SCHEDULE,
            serde_json::Value::Null,
        );
        game = evaluate(&crate::Cna::dev(), &c, &game, &cmd).unwrap().game;
    }
    let mut other = game.clone();
    other.rng = CampaignRng::from_seed([55; 32]).state();
    let a = evaluate(&crate::Cna::dev(), &c, &game, &Command::Advance).unwrap();
    let b = evaluate(&crate::Cna::dev(), &c, &other, &Command::Advance).unwrap();
    assert_private_transition(&c, Side::Commonwealth, &a, &b);
    let owner = SeatId::new(Side::Commonwealth, Role::Logistics);
    let dice = |t: &cna_core::engine::Transition<crate::Cna>| {
        t.events
            .iter()
            .filter(|e| matches!(e.event, cna_protocol::GameEvent::DiceRolled { .. }))
            .map(|e| {
                assert_eq!(e.audience, Audience::Seat(owner));
                serde_json::to_value(&e.event).unwrap()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(dice(&a).len(), 16);
    assert_ne!(dice(&a), dice(&b));
    let targets = crate::testkit::inspect_targets(&[&a.game.state, &b.game.state]);
    assert_ne!(
        seat_snapshot(&c, &a.game.state, owner, &targets),
        seat_snapshot(&c, &b.game.state, owner, &targets)
    );
}

fn distribution_game(c: &crate::CnaContent, points: i32) -> cna_core::engine::Game<crate::Cna> {
    use cna_core::decision::{ActionSchema, ActionSpace, Secrecy, Trigger};
    let mut state = scheduling_state(c, 4);
    state.decisions.pending.clear();
    side_mut(&mut state, Side::Axis).unwrap().planning.phase = ProcedurePhase::Resolved;
    side_mut(&mut state, Side::Commonwealth)
        .unwrap()
        .planning
        .phase = ProcedurePhase::Distributing {
        determined: vec![DeterminedProduction {
            row: ProductionRowRef::CommonwealthInfantry {
                column: "gt_3_to_30".into(),
            },
            kind: ProducedKind::Points {
                kind: ReplacementKind::Infantry {
                    nation: ReplacementNation::Commonwealth,
                },
            },
            points,
            arrival_turn: 8,
        }],
        submitted: None,
    };
    let mut rng = CampaignRng::from_seed([56; 32]);
    let mut events = vec![];
    // A checkpoint-corruption control uses the real Respond dispatcher and declared pass.
    crate::steps::open(
        &mut state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        SeatId::new(Side::Commonwealth, Role::Logistics),
        windows::DISTRIBUTION,
        "Distribution fixture".into(),
        &["land:20.74"],
        Trigger::Scheduled,
        Secrecy::SecretSimultaneous,
        ActionSpace::new(ActionSchema::Record { fields: vec![] })
            .with_pass("Deterministic distribution"),
    );
    cna_core::engine::Game {
        state,
        rng: rng.state(),
    }
}

/// Cases: land:20.74
#[test]
fn canonical_distribution_error_keeps_exact_variant_detail_at_respond_and_advance() {
    use cna_core::engine::{Command, Rejection, evaluate};
    let c = content();
    let game = distribution_game(&c, -1);
    let before = serde_json::to_vec(&game).unwrap();
    let expected = Rejection::Engine(invariant("corrupt determined production"));
    let cmd = production_command(
        &game.state,
        Side::Commonwealth,
        windows::DISTRIBUTION,
        serde_json::Value::Null,
    );
    assert_eq!(
        evaluate(&crate::Cna::dev(), &c, &game, &cmd).err(),
        Some(expected.clone())
    );
    assert_eq!(serde_json::to_vec(&game).unwrap(), before);
    let mut accepted = game.clone();
    accepted.state.decisions.pending.clear();
    let ProcedurePhase::Distributing { submitted, .. } =
        &mut side_mut(&mut accepted.state, Side::Commonwealth)
            .unwrap()
            .planning
            .phase
    else {
        unreachable!()
    };
    *submitted = Some(DistributionPlan { batches: vec![] });
    let checkpoint: cna_core::engine::Game<crate::Cna> =
        serde_json::from_slice(&serde_json::to_vec(&accepted).unwrap()).unwrap();
    let before = serde_json::to_vec(&checkpoint).unwrap();
    assert_eq!(
        evaluate(&crate::Cna::dev(), &c, &checkpoint, &Command::Advance).err(),
        Some(expected)
    );
    assert_eq!(serde_json::to_vec(&checkpoint).unwrap(), before);
}

/// Cases: land:20.74
#[test]
fn late_ordinary_distribution_refusal_is_adjudication_invariant_with_whole_game_rollback() {
    use cna_core::engine::{Command, Rejection, evaluate};
    let c = content();
    let game = distribution_game(&c, 2);
    let cmd = production_command(
        &game.state,
        Side::Commonwealth,
        windows::DISTRIBUTION,
        serde_json::Value::Null,
    );
    let mut accepted = evaluate(&crate::Cna::dev(), &c, &game, &cmd).unwrap().game;
    assert_eq!(accepted.rng, game.rng);
    let ProcedurePhase::Distributing {
        determined,
        submitted: Some(plan),
    } = &mut side_mut(&mut accepted.state, Side::Commonwealth)
        .unwrap()
        .planning
        .phase
    else {
        unreachable!()
    };
    determined[0].points = 3; // Corrupt the trusted history after the valid buffered answer.
    assert!(matches!(
        validate_distribution(Side::Commonwealth, determined, plan),
        Err(Rejection::Illegal { .. })
    ));
    let before = serde_json::to_vec(&accepted).unwrap();
    assert_eq!(
        evaluate(&crate::Cna::dev(), &c, &accepted, &Command::Advance).err(),
        Some(Rejection::Engine(invariant(
            "accepted distribution is no longer valid"
        )))
    );
    assert_eq!(serde_json::to_vec(&accepted).unwrap(), before);
    assert!(matches!(
        side(&accepted.state, Side::Commonwealth)
            .unwrap()
            .planning
            .phase,
        ProcedurePhase::Distributing {
            submitted: Some(_),
            ..
        }
    ));
}

fn assert_contract_roundtrip<T>(value: T, expected: serde_json::Value)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let encoded = serde_json::to_value(&value).unwrap();
    assert_eq!(encoded, expected);
    let restored: T = serde_json::from_value(encoded).unwrap();
    assert_eq!(restored, value);
    assert_eq!(serde_json::to_value(restored).unwrap(), expected);
}

/// Cases: land:20.61, land:20.72
#[test]
fn production_row_wire_contract_roundtrips_all_five_variants() {
    use serde_json::json;
    for (kind, wire) in [
        (FuelTruckKind::Light, "light"),
        (FuelTruckKind::Medium, "medium"),
        (FuelTruckKind::Heavy, "heavy"),
    ] {
        assert_contract_roundtrip(
            ProductionRowRef::AxisTruck { kind },
            json!({"kind":"axis_truck","truck_kind":wire}),
        );
        assert_contract_roundtrip(
            ProductionRowRef::CommonwealthTruck {
                column: truck_column(8).unwrap().0.into(),
                kind,
            },
            json!({"kind":"commonwealth_truck","column":truck_column(8).unwrap().0,"truck_kind":wire}),
        );
    }
    assert_contract_roundtrip(
        italian_infantry(),
        json!({"kind":"axis_equipment","nation":"italian","item":"Infantry"}),
    );
    assert_contract_roundtrip(
        ProductionRowRef::CommonwealthInfantry {
            column: infantry_column(8).unwrap().0.into(),
        },
        json!({"kind":"commonwealth_infantry","column":infantry_column(8).unwrap().0}),
    );
    // Read an actual native finite designation; this check grants no production entitlement.
    let c = content();
    let item = c
        .tables
        .land
        .commonwealth_production
        .equipment_rows()
        .next()
        .unwrap()
        .0
        .to_owned();
    assert_contract_roundtrip(
        ProductionRowRef::CommonwealthEquipment { item: item.clone() },
        json!({"kind":"commonwealth_equipment","item":item}),
    );
}

/// Cases: land:20.61, land:20.72
#[test]
fn produced_kind_wire_contract_roundtrips_both_variants_with_nested_discriminator() {
    use serde_json::json;
    for (kind, wire) in [
        (FuelTruckKind::Light, "light"),
        (FuelTruckKind::Medium, "medium"),
        (FuelTruckKind::Heavy, "heavy"),
    ] {
        assert_contract_roundtrip(
            ProducedKind::Trucks { kind },
            json!({"kind":"trucks","truck_kind":wire}),
        );
    }
    assert_contract_roundtrip(
        ProducedKind::Points {
            kind: ReplacementKind::Infantry {
                nation: ReplacementNation::Commonwealth,
            },
        },
        json!({"kind":"points","replacement_kind":{"kind":"infantry","nation":"commonwealth"}}),
    );
}

/// Cases: land:20.21, land:20.74, land:20.78, land:3.6
#[test]
fn buffered_truck_and_points_game_checkpoint_resumes_exact_owned_finish() {
    use cna_core::engine::{Command, Game, Rejection, evaluate};
    let mut c = content();
    c.bounds.end_gt = 12; // Reduced future-calendar fixture, not Graziani entitlement.
    let mut game = distribution_game(&c, 3);
    let truck_row = ProductionRowRef::CommonwealthTruck {
        column: truck_column(8).unwrap().0.into(),
        kind: FuelTruckKind::Medium,
    };
    let truck_kind = ProducedKind::Trucks {
        kind: FuelTruckKind::Medium,
    };
    let ProcedurePhase::Distributing { determined, .. } =
        &mut side_mut(&mut game.state, Side::Commonwealth)
            .unwrap()
            .planning
            .phase
    else {
        unreachable!()
    };
    determined.push(DeterminedProduction {
        row: truck_row,
        kind: truck_kind,
        points: 3,
        arrival_turn: 8,
    });
    let cmd = production_command(
        &game.state,
        Side::Commonwealth,
        windows::DISTRIBUTION,
        serde_json::Value::Null,
    );
    let accepted = evaluate(&crate::Cna::dev(), &c, &game, &cmd).unwrap().game;
    assert_eq!(accepted.rng, game.rng);
    let ProcedurePhase::Distributing {
        determined,
        submitted: Some(plan),
    } = &side(&accepted.state, Side::Commonwealth)
        .unwrap()
        .planning
        .phase
    else {
        panic!("nonempty distribution buffer missing")
    };
    assert_eq!(determined.len(), 2);
    assert_eq!(plan.batches.len(), 6);
    assert!(
        plan.batches
            .iter()
            .any(|b| matches!(b.kind, ProducedKind::Trucks { .. }))
    );
    assert!(
        plan.batches
            .iter()
            .any(|b| matches!(b.kind, ProducedKind::Points { .. }))
    );
    let plan = plan.clone();
    let encoded = serde_json::to_vec(&accepted).unwrap();
    let restored: Game<crate::Cna> = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(serde_json::to_vec(&restored).unwrap(), encoded);

    // The isolated distribution fixture has not completed the genuine convoy setup.
    // The full dispatcher must retain that refusal and both buffered checkpoints unchanged.
    for checkpoint in [&accepted, &restored] {
        assert_eq!(
            evaluate(&crate::Cna::dev(), &c, checkpoint, &Command::Advance).err(),
            Some(Rejection::Engine(EngineError::Unsupported {
                case: "scen:60.37".into(),
                detail: "initial convoy planning must finish during setup".into(),
            }))
        );
        assert_eq!(serde_json::to_vec(checkpoint).unwrap(), encoded);
    }

    // Compare successful resumes at the actual owned finalizer, preserving all Game fields.
    let mut direct = accepted.clone();
    let mut resumed = restored;
    for checkpoint in [&mut direct, &mut resumed] {
        let before_rng = checkpoint.rng.clone();
        let before_units = serde_json::to_vec(&checkpoint.state.land.units).unwrap();
        let before_logistics = serde_json::to_vec(&checkpoint.state.logistics).unwrap();
        let mut rng = CampaignRng::from_state(&checkpoint.rng);
        let mut events = vec![];
        windows::finish_planning(
            &c,
            &mut checkpoint.state,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        checkpoint.rng = rng.state();
        assert_eq!(checkpoint.rng, before_rng);
        assert!(events.is_empty());
        assert_eq!(
            serde_json::to_vec(&checkpoint.state.land.units).unwrap(),
            before_units
        );
        assert_eq!(
            serde_json::to_vec(&checkpoint.state.logistics).unwrap(),
            before_logistics
        );
        let owned = side(&checkpoint.state, Side::Commonwealth).unwrap();
        assert_eq!(owned.orders.len(), 6);
        assert_eq!(owned.next_serial, 6);
        assert_eq!(owned.orders.iter().map(|o| o.points).sum::<i32>(), 6);
        assert_eq!(
            owned
                .orders
                .iter()
                .filter(|o| matches!(o.kind, ProducedKind::Trucks { .. }))
                .map(|o| o.points)
                .sum::<i32>(),
            3
        );
        assert_eq!(
            owned
                .orders
                .iter()
                .filter(|o| matches!(o.kind, ProducedKind::Points { .. }))
                .map(|o| o.points)
                .sum::<i32>(),
            3
        );
        assert!(owned.counters.is_empty());
        assert_eq!(owned.cw_arrival_limits.len(), 1);
        assert_eq!(owned.cw_arrival_limits[0].total_trucks, 3);
        assert_eq!(owned.cw_arrival_limits[0].alexandria_trucks, 0);
        assert!(owned.cw_arrival_limits[0].infantry_city.is_none());
        assert!(matches!(owned.planning.phase, ProcedurePhase::Resolved));
    }
    assert_eq!(
        serde_json::to_vec(&resumed).unwrap(),
        serde_json::to_vec(&direct).unwrap()
    );
    let closed: Game<crate::Cna> =
        serde_json::from_slice(&serde_json::to_vec(&resumed).unwrap()).unwrap();
    let before_retry = serde_json::to_vec(&closed).unwrap();
    let mut retry = closed;
    assert!(finish_distribution(&mut retry.state, Side::Commonwealth, &plan).is_err());
    assert_eq!(serde_json::to_vec(&retry).unwrap(), before_retry);
}
