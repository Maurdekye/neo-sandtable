use super::*;

fn period() -> MaintenancePeriod {
    MaintenancePeriod::Tactical {
        game_turn: 2,
        op_stage: 1,
    }
}
fn cohort(sgsu: &str, squadron: &str, ids: &[&str]) -> RefitCohort {
    RefitCohort {
        servicer: SgsuId(sgsu.into()),
        assigned_squadron: squadron.into(),
        priority: ids.iter().map(|id| PlaneId((*id).into())).collect(),
        method: RefitMethod::Squadron,
    }
}
fn limits() -> BTreeMap<SgsuId, u32> {
    BTreeMap::from([(SgsuId("a".into()), 3), (SgsuId("b".into()), 3)])
}

/// Cases: airlog:38.33, airlog:38.36
/// Interpretations: interp:airlog-0022
#[test]
fn failed_attempts_consume_period_capacity_and_cannot_retry_at_another_servicer() {
    // Deliberately tiny trusted bound for accounting, not a printed SGSU rating.
    let prior = vec![RefitReceipt {
        period: period(),
        cohort: cohort("a", "first", &["p1", "p2"]),
        successful: BTreeSet::new(),
    }];
    let before = serde_json::to_vec(&prior).unwrap();
    let one = account_refit(
        period(),
        &prior,
        &[cohort("a", "second", &["p3"])],
        &limits(),
    )
    .unwrap();
    assert_eq!(one.attempted[&SgsuId("a".into())].len(), 3);
    assert_eq!(
        one.newly_charged_squadrons,
        BTreeSet::from(["second".into()])
    );
    assert!(matches!(
        account_refit(
            period(),
            &prior,
            &[cohort("a", "second", &["p3", "p4"])],
            &limits()
        ),
        Err(EngineError::Invariant { .. })
    ));
    assert!(matches!(
        account_refit(
            period(),
            &prior,
            &[cohort("b", "second", &["p1"])],
            &limits()
        ),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(serde_json::to_vec(&prior).unwrap(), before);
    let next = MaintenancePeriod::Tactical {
        game_turn: 2,
        op_stage: 2,
    };
    assert!(account_refit(next, &prior, &[cohort("a", "first", &["p1"])], &limits()).is_ok());
}

/// Cases: airlog:38.34, airlog:38.35, airlog:38.36
/// Interpretations: interp:airlog-0022
#[test]
fn nationality_partitions_share_one_squadron_store_and_split_is_exact_unsupported() {
    let partitions = vec![cohort("a", "mixed", &["p1"]), cohort("a", "mixed", &["p2"])];
    let accounting = account_refit(period(), &[], &partitions, &limits()).unwrap();
    assert_eq!(
        accounting.newly_charged_squadrons,
        BTreeSet::from(["mixed".into()])
    );
    let paid = vec![RefitReceipt {
        period: period(),
        cohort: partitions[0].clone(),
        successful: BTreeSet::new(),
    }];
    assert!(
        account_refit(period(), &paid, &partitions[1..], &limits())
            .unwrap()
            .newly_charged_squadrons
            .is_empty()
    );
    assert!(matches!(account_refit(period(), &paid,
        &[cohort("b", "mixed", &["p2"])], &limits()),
        Err(EngineError::Unsupported { case, .. }) if case == "airlog:38.36"));
    assert!(
        account_refit(
            period(),
            &[],
            &[cohort("a", "empty", &[])],
            &BTreeMap::new()
        )
        .unwrap()
        .newly_charged_squadrons
        .is_empty()
    );
}

/// Cases: airlog:38.34
/// Interpretations: interp:airlog-0023
#[test]
fn success_priority_keeps_owner_order_and_rejects_duplicate_or_excess_results() {
    let priority = vec![
        PlaneId("z".into()),
        PlaneId("a".into()),
        PlaneId("m".into()),
    ];
    assert_eq!(
        successful_ids(&priority, 2).unwrap(),
        BTreeSet::from([PlaneId("z".into()), PlaneId("a".into())])
    );
    assert!(successful_ids(&priority, 0).unwrap().is_empty());
    assert_eq!(successful_ids(&priority, 3).unwrap().len(), 3);
    assert!(successful_ids(&priority, u32::MAX).is_err());
    assert!(successful_ids(&[priority[0].clone(), priority[0].clone()], 0).is_err());
    let bad = RefitReceipt {
        period: period(),
        cohort: cohort("a", "first", &["p1"]),
        successful: BTreeSet::from([PlaneId("unattempted".into())]),
    };
    assert!(matches!(
        account_refit(period(), &[bad], &[], &limits()),
        Err(EngineError::Invariant { .. })
    ));
}

/// Cases: airlog:35.14
/// Interpretations: interp:airlog-0025
#[test]
fn strategic_dues_require_actual_completed_stage_without_a_fourth_or_legacy_credit() {
    let strategic = MaintenancePeriod::Strategic { game_turn: 2 };
    let completed = SgsuOpStage {
        game_turn: 2,
        op_stage: 3,
    };
    assert_eq!(strategic.dues_stage(Some(completed)).unwrap(), completed);
    assert_eq!(
        period().dues_stage(None).unwrap(),
        SgsuOpStage {
            game_turn: 2,
            op_stage: 1
        }
    );
    for absent in [
        None,
        Some(SgsuOpStage {
            game_turn: 1,
            op_stage: 3,
        }),
        Some(SgsuOpStage {
            game_turn: 2,
            op_stage: 4,
        }),
    ] {
        assert!(matches!(strategic.dues_stage(absent),
            Err(EngineError::Unsupported { case, .. }) if case == "airlog:35.14"));
    }
    assert!(matches!(
        MaintenancePeriod::Tactical {
            game_turn: 2,
            op_stage: 4
        }
        .check(),
        Err(EngineError::Invariant { .. })
    ));
}

/// Cases: airlog:38.33, airlog:38.36, airlog:49.15
#[test]
fn standalone_receipts_checkpoint_with_structured_source_and_no_dues_or_load_defaults() {
    let legacy: MaintenanceState = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy, MaintenanceState::default());
    assert!(legacy.period.is_none());
    let state = MaintenanceState {
        period: Some(period()),
        receipts: BTreeMap::from([(
            Side::Axis,
            vec![RefitReceipt {
                period: period(),
                cohort: cohort("a", "first", &["p1"]),
                successful: BTreeSet::new(),
            }],
        )]),
        debits: BTreeMap::from([(
            Side::Axis,
            vec![DebitReceipt {
                servicer: SgsuId("a".into()),
                draws: vec![SupplyDraw {
                    source: SupplySource::AirDump("exact.dump".into()),
                    amount: crate::logistics::SupplyDemand {
                        fuel: FuelTenths::new(3),
                        ..Default::default()
                    },
                }],
                fuel_rounding: vec![(
                    SupplySource::AirDump("exact.dump".into()),
                    FuelTenths::new(3),
                )],
            }],
        )]),
        finished: true,
        closed: Side::ALL.into_iter().collect(),
        ..Default::default()
    };
    let encoded = serde_json::to_vec(&state).unwrap();
    check(&state).unwrap();
    assert_eq!(
        serde_json::from_slice::<MaintenanceState>(&encoded).unwrap(),
        state
    );
    assert!(!String::from_utf8(encoded).unwrap().contains("fuelled"));
}
