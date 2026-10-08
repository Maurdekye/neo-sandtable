use super::*;
use crate::{
    CnaContent,
    logistics::{FuelTruckKind, SegmentKey, TruckFuelCohort, pools::add_truck_pool},
    state::Location,
};
use cna_content::scenario::{Placement, Supplies};
use std::sync::OnceLock;
fn content() -> &'static CnaContent {
    static C: OnceLock<CnaContent> = OnceLock::new();
    C.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn marker() -> (State, BrokenMarker) {
    let mut s = State::new(content()).unwrap();
    s.logistics.truck_pools.clear();
    let id = add_truck_pool(
        &mut s.logistics,
        None,
        Side::Axis,
        Placement::Hex {
            hex: "C4020".into(),
        },
        Some(Location::Hex {
            hex: "C4020".into(),
        }),
        Trucks {
            light: 1,
            ..Trucks::default()
        },
        Supplies::default(),
    )
    .unwrap();
    let cohort = "selected-real-cohort".to_string();
    let marker = BrokenMarker {
        id: String::new(),
        side: Side::Axis,
        hex: "C4020".into(),
        assets: vec![],
        source_pool: Some(id.clone()),
        pool_assets: vec![super::super::pools::PoolAsset {
            pool: id.clone(),
            equipment: Equipment::LightTruck,
            points: 1,
            cohort: cohort.clone(),
        }],
        pool_fuel_cohorts: vec![TruckFuelCohort {
            id: cohort,
            parent: None,
            kind: FuelTruckKind::Light,
            count: 1,
            cp_quarters: 12,
            account: id,
            segment: SegmentKey {
                game_turn: s.cursor.game_turn,
                op_stage: s.cursor.op_stage,
                half: s.cursor.half,
                cycle: s.cursor.cycle,
            },
        }],
        passengers: BTreeMap::new(),
        transport: Trucks::default(),
        cargo: Default::default(),
        tank_fuel: FuelTenths::ZERO,
        activity_water: WaterPoints::ZERO,
        fuel_cohorts: vec![],
        paid_truck_water: Default::default(),
        water_credit_stage: None,
    };
    (s, marker)
}
/// Cases: land:21.42, land:21.43
#[test]
fn pool_marker_batch_checks_every_serial_before_mutation() {
    let (mut s, m) = marker();
    s.land.breakdown.next_marker.insert(Side::Axis, u64::MAX);
    let before = serde_json::to_value(&s).unwrap();
    assert!(matches!(
        prepare_pool_markers(content(), &s, std::slice::from_ref(&m)),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    s.land
        .breakdown
        .next_marker
        .insert(Side::Axis, u64::MAX - 1);
    let mut second = m.clone();
    second.pool_assets[0].cohort = "another-selected-real-cohort".into();
    second.pool_fuel_cohorts[0].id = second.pool_assets[0].cohort.clone();
    let before = serde_json::to_value(&s).unwrap();
    assert!(prepare_pool_markers(content(), &s, &[m.clone(), second]).is_err());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    s.land.breakdown.next_marker.insert(Side::Axis, 0);
    let prepared = prepare_pool_markers(content(), &s, std::slice::from_ref(&m)).unwrap();
    s.land.breakdown.markers.insert("broken-axis-1".into(), m);
    let before = serde_json::to_value(&s).unwrap();
    assert!(apply_pool_markers(content(), &mut s, prepared).is_err());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}
/// Cases: land:21.25, land:21.29, land:21.43
#[test]
fn pool_marker_zero_equipment_and_physical_mismatch_never_allocate() {
    let (mut s, mut m) = marker();
    m.pool_assets.clear();
    m.pool_fuel_cohorts.clear();
    let before = serde_json::to_value(&s).unwrap();
    let p = prepare_pool_markers(content(), &s, std::slice::from_ref(&m)).unwrap();
    assert!(apply_pool_markers(content(), &mut s, p).unwrap().is_empty());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    m.activity_water = WaterPoints::new(1);
    assert!(prepare_pool_markers(content(), &s, &[m]).is_err());
    let (_, mut m) = marker();
    m.pool_fuel_cohorts[0].count = 2;
    assert!(prepare_pool_markers(content(), &s, &[m]).is_err());
}
/// Cases: land:21.42, land:3.62
#[test]
fn valid_pool_marker_preserves_paid_physical_history_and_private_details() {
    let (mut s, m) = marker();
    let cp = m.pool_fuel_cohorts[0].cp_quarters;
    let prepared = prepare_pool_markers(content(), &s, &[m]).unwrap();
    let events = apply_pool_markers(content(), &mut s, prepared).unwrap();
    assert_eq!(s.land.breakdown.next_marker[&Side::Axis], 1);
    assert_eq!(
        s.land.breakdown.markers["broken-axis-1"].pool_fuel_cohorts[0].cp_quarters,
        cp
    );
    for event in events {
        if matches!(
            event.event,
            GameEvent::Note { .. } | GameEvent::UnitUpdated { .. }
        ) {
            assert_eq!(event.audience, Audience::Side(Side::Axis));
        }
    }
}
