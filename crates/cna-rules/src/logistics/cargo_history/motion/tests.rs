use super::super::tests::{content, game};
use super::*;
fn cohort(id: &str, kind: FuelTruckKind, count: i32) -> PhysicalTrucks {
    PhysicalTrucks {
        id: id.into(),
        parent: None,
        kind,
        count,
    }
}
/// Cases: airlog:53.25, land:8.83, land:8.89
#[test]
fn stage_motion_outlives_segments_cycles_body_cp_and_recovery_but_never_guesses_legacy_zero() {
    let (mut s, a, _, _) = game();
    let trucks = [cohort("physical-a", FuelTruckKind::Medium, 2)];
    assert_eq!(
        query(&s, Side::Axis, &a, &trucks),
        Err(MotionError::Unknown)
    );
    seed_fresh(&mut s, Side::Axis, &a, &trucks).unwrap();
    advance(&mut s, Side::Axis, &a, &trucks, 16).unwrap();
    s.cursor.cycle += 1;
    s.cursor.half = Some(crate::seq::Half::B);
    assert_eq!(
        query(&s, Side::Axis, &a, &trucks).unwrap()[0].spent_cp_quarters,
        16
    );
    let json = serde_json::to_value(&s).unwrap();
    let mut replay: State = serde_json::from_value(json.clone()).unwrap();
    advance(&mut s, Side::Axis, &a, &trucks, 12).unwrap();
    advance(&mut replay, Side::Axis, &a, &trucks, 12).unwrap();
    assert_eq!(
        serde_json::to_value(&s).unwrap(),
        serde_json::to_value(&replay).unwrap()
    );
    assert_eq!(
        timing(&s, Side::Axis, &a, &trucks, 120)
            .unwrap()
            .spent_cp_quarters,
        28
    );
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(
        seed_fresh(&mut s, Side::Axis, &a, &trucks),
        Err(MotionError::Invalid)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    s.cursor.op_stage = Some(2);
    assert_eq!(
        query(&s, Side::Axis, &a, &trucks),
        Err(MotionError::Unknown)
    );
    seed_fresh(&mut s, Side::Axis, &a, &trucks).unwrap();
    assert_eq!(
        query(&s, Side::Axis, &a, &trucks).unwrap()[0].spent_cp_quarters,
        0
    );
    let mut legacy = json;
    legacy["logistics"]["cargo_history"]
        .as_object_mut()
        .unwrap()
        .remove("motion");
    let legacy: State = serde_json::from_value(legacy).unwrap();
    assert_eq!(
        query(&legacy, Side::Axis, &a, &trucks),
        Err(MotionError::Unknown)
    );
}
/// Cases: land:8.56, land:21.29, airlog:53.25
#[test]
fn selected_partial_split_preserves_kind_counts_and_actual_cp_without_moving_body_history() {
    let (mut s, a, b, _) = game();
    let original = cohort("physical-a", FuelTruckKind::Medium, 3);
    let existing = cohort("physical-b", FuelTruckKind::Light, 1);
    seed_fresh(&mut s, Side::Axis, &a, std::slice::from_ref(&original)).unwrap();
    seed_fresh(&mut s, Side::Axis, &b, std::slice::from_ref(&existing)).unwrap();
    advance(&mut s, Side::Axis, &a, std::slice::from_ref(&original), 16).unwrap();
    advance(&mut s, Side::Axis, &b, std::slice::from_ref(&existing), 32).unwrap();
    let selected = PhysicalTrucks {
        id: "physical-child".into(),
        parent: Some(original.id.clone()),
        kind: original.kind,
        count: 1,
    };
    transfer(&mut s, Side::Axis, &a, &b, std::slice::from_ref(&selected)).unwrap();
    let remaining = PhysicalTrucks {
        count: 2,
        ..original.clone()
    };
    assert_eq!(
        query(&s, Side::Axis, &a, &[remaining]).unwrap()[0].spent_cp_quarters,
        16
    );
    let receiving = [existing.clone(), selected.clone()];
    assert_eq!(
        timing(&s, Side::Axis, &b, &receiving, 120).unwrap_err(),
        MotionError::Mixed
    );
    advance(&mut s, Side::Axis, &b, &receiving, 12).unwrap();
    let moved = query(&s, Side::Axis, &b, &receiving).unwrap();
    assert_eq!(moved[0].spent_cp_quarters, 44);
    assert_eq!(moved[1].spent_cp_quarters, 28); // 4+3 CP, not parent body 8+3.
    let before = serde_json::to_value(&s).unwrap();
    let invalid = PhysicalTrucks {
        count: 3,
        ..selected
    };
    assert!(transfer(&mut s, Side::Axis, &a, &b, &[invalid]).is_err());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}
/// Cases: land:12.46, land:20.44, airlog:53.25
#[test]
fn exact_retirement_and_missing_split_hook_fail_closed_and_overflow_is_atomic() {
    let (mut s, a, _, _) = game();
    let original = cohort("physical-a", FuelTruckKind::Medium, 2);
    seed_fresh(&mut s, Side::Axis, &a, std::slice::from_ref(&original)).unwrap();
    let child = PhysicalTrucks {
        id: "physical-child".into(),
        parent: Some(original.id.clone()),
        count: 1,
        kind: original.kind,
    };
    assert_eq!(
        query(&s, Side::Axis, &a, std::slice::from_ref(&child)),
        Err(MotionError::Unknown)
    );
    advance(
        &mut s,
        Side::Axis,
        &a,
        std::slice::from_ref(&original),
        i32::MAX,
    )
    .unwrap();
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(
        advance(&mut s, Side::Axis, &a, std::slice::from_ref(&original), 1),
        Err(MotionError::Invalid)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    retire(&mut s, Side::Axis, &a, &[child]).unwrap();
    let remaining = PhysicalTrucks {
        count: 1,
        ..original
    };
    assert_eq!(query(&s, Side::Axis, &a, &[remaining]).unwrap()[0].count, 1);
}
/// Cases: airlog:53.25, land:3.6
#[test]
fn motion_snapshots_and_enemy_pairs_preserve_private_physical_history() {
    let (mut s, a, _, _) = game();
    let trucks = [cohort("physical-a", FuelTruckKind::Medium, 1)];
    let before = s.clone();
    let snap = super::super::snapshot(&s, std::slice::from_ref(&a));
    seed_fresh(&mut s, Side::Axis, &a, &trucks).unwrap();
    advance(&mut s, Side::Axis, &a, &trucks, 16).unwrap();
    crate::testkit::assert_indistinguishable(
        &crate::Cna::dev(),
        content(),
        &s,
        &before,
        Side::Commonwealth,
    );
    assert_eq!(
        query(&s, Side::Commonwealth, &a, &trucks),
        Err(MotionError::Invalid)
    );
    super::super::restore(&mut s, &snap);
    assert_eq!(
        serde_json::to_value(&s).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
}
