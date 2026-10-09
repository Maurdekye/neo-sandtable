use super::*;

fn content() -> crate::CnaContent {
    crate::CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
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
