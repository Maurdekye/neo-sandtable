use super::*;
use crate::state::{Location, WeatherState};
use cna_content::units::Toe;
use cna_tables::land::weather::WeatherKind;
use std::sync::OnceLock;
const A: &str = "it.libyan_tank_command.xxi_l_tank_bn";
const B: &str = "it.libyan_tank_command.lxii_l_tank_bn";
fn content() -> &'static CnaContent {
    static C: OnceLock<CnaContent> = OnceLock::new();
    C.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn fixture(hot: bool) -> State {
    let mut s = State::new(content()).unwrap();
    s.cursor.op_stage = Some(1);
    s.turn.weather = Some(WeatherState {
        kind: if hot {
            WeatherKind::Hot
        } else {
            WeatherKind::Normal
        },
        storm_sections: vec![],
    });
    for (id, strength) in [(A, 2), (B, 1)] {
        let u = s.land.units.get_mut(&id.into()).unwrap();
        u.location = Location::Hex {
            hex: "C4020".into(),
        };
        u.toe = Some(Toe::Under { under: strength });
        u.trucks = Trucks::default();
        u.transport_trucks = Trucks::default();
        s.logistics
            .unit_supply
            .entry(id.into())
            .or_default()
            .activity_water = WaterPoints::ZERO;
    }
    s.land.units.get_mut(&A.into()).unwrap().trucks = Trucks {
        light: 2,
        medium: 1,
        heavy: 1,
    };
    s
}
fn set_reserve(s: &mut State, id: &str, n: i32) {
    s.logistics
        .unit_supply
        .entry(id.into())
        .or_default()
        .activity_water = WaterPoints::new(n);
}
/// Cases: land:6.13, airlog:52.42, airlog:52.51
/// Interpretations: interp:airlog-0015
#[test]
fn forced_shortfall_survives_recovery_casualties_repeated_cp_and_rewatering() {
    let mut s = fixture(false);
    let id = A.into();
    set_reserve(&mut s, A, 3);
    let paid = consume_activity_water_forced(content(), &mut s, &id).unwrap();
    assert_eq!(
        paid,
        ActivityWaterPayment {
            consumed: WaterPoints::new(3),
            shortfall: WaterPoints::new(3)
        }
    );
    let l = s.logistics.rations[&id]
        .activity_water_ledger
        .as_ref()
        .unwrap();
    assert_eq!(l.body_required, 2);
    assert_eq!(l.body_paid, 2);
    assert_eq!(l.truck_paid.light, 1);
    assert!(
        !super::super::movement_restrictions(content(), &s, &id)
            .unwrap()
            .may_move
    );
    assert_eq!(
        super::super::movement_restrictions(content(), &s, &id)
            .unwrap()
            .defense_divisor,
        2
    );
    s = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    s.land.units.get_mut(&id).unwrap().toe = Some(Toe::Under { under: 1 });
    set_reserve(&mut s, A, 1);
    assert_eq!(
        consume_activity_water_forced(content(), &mut s, &id).unwrap(),
        ActivityWaterPayment {
            consumed: WaterPoints::ZERO,
            shortfall: WaterPoints::new(3)
        }
    );
    let before = s.clone();
    assert_eq!(
        spend_activity_water(content(), &mut s, &id),
        Err(SupplyError::Insufficient)
    );
    assert_eq!(
        serde_json::to_value(&s).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    set_reserve(&mut s, A, 3);
    assert_eq!(
        super::super::water::requirements(content(), &s, &id)
            .unwrap()
            .activity,
        0
    );
    spend_activity_water(content(), &mut s, &id).unwrap();
    assert_eq!(activity_water_due(content(), &s, &id).unwrap(), 0);
    assert_eq!(
        s.logistics.unit_supply[&id].activity_water,
        WaterPoints::ZERO
    );
    assert!(
        super::super::movement_restrictions(content(), &s, &id)
            .unwrap()
            .may_move
    );
    set_reserve(&mut s, A, 9);
    assert_eq!(
        consume_activity_water_forced(content(), &mut s, &id)
            .unwrap()
            .consumed,
        WaterPoints::ZERO
    );
    assert_eq!(s.logistics.unit_supply[&id].activity_water.get(), 9);
    s.cursor.op_stage = Some(2);
    assert_eq!(activity_water_due(content(), &s, &id).unwrap(), 5);
    assert_eq!(
        consume_activity_water_forced(content(), &mut s, &id)
            .unwrap()
            .consumed
            .get(),
        5
    );
    assert_eq!(s.logistics.unit_supply[&id].activity_water.get(), 4);
}
/// Cases: land:8.56, airlog:52.42, airlog:52.43
/// Interpretations: interp:airlog-0015
#[test]
fn paid_trucks_transfer_without_paying_the_receivers_body() {
    let mut s = fixture(false);
    set_reserve(&mut s, A, 6);
    spend_activity_water(content(), &mut s, &A.into()).unwrap();
    let trucks = Trucks {
        light: 1,
        medium: 1,
        heavy: 0,
    };
    assert_eq!(
        transfer_activity_water_credit(content(), &mut s, &A.into(), &B.into(), trucks).unwrap(),
        TruckWater {
            light: 1,
            medium: 1,
            heavy: 0
        }
    );
    s.land.units.get_mut(&A.into()).unwrap().trucks = Trucks {
        light: 1,
        medium: 0,
        heavy: 1,
    };
    s.land.units.get_mut(&B.into()).unwrap().trucks = trucks;
    assert_eq!(activity_water_due(content(), &s, &A.into()).unwrap(), 0);
    assert_eq!(activity_water_due(content(), &s, &B.into()).unwrap(), 1);
    assert_eq!(
        s.logistics.rations[&B.into()]
            .activity_water_ledger
            .as_ref()
            .unwrap()
            .body_paid,
        0
    );
    assert_eq!(
        spend_activity_water(content(), &mut s, &B.into()),
        Err(SupplyError::Insufficient)
    );
    set_reserve(&mut s, B, 1);
    spend_activity_water(content(), &mut s, &B.into()).unwrap();
    assert_eq!(s.logistics.unit_supply[&B.into()].activity_water.get(), 0);
    s = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert_eq!(activity_water_due(content(), &s, &B.into()).unwrap(), 0);
    s.cursor.op_stage = Some(2);
    assert_eq!(activity_water_due(content(), &s, &B.into()).unwrap(), 3);
}
/// Cases: land:8.56, airlog:52.42, airlog:52.43
/// Interpretations: interp:airlog-0015
#[test]
fn hot_partial_truck_credit_is_exact_and_transfer_errors_are_atomic() {
    let mut s = fixture(true);
    set_reserve(&mut s, A, 5);
    let p = consume_activity_water_forced(content(), &mut s, &A.into()).unwrap();
    assert_eq!(p.shortfall.get(), 7);
    assert_eq!(
        transfer_activity_water_credit(
            content(),
            &mut s,
            &A.into(),
            &B.into(),
            Trucks {
                light: 1,
                medium: 0,
                heavy: 0
            }
        )
        .unwrap(),
        TruckWater {
            light: 1,
            medium: 0,
            heavy: 0
        }
    );
    s.land.units.get_mut(&A.into()).unwrap().trucks.light = 1;
    s.land.units.get_mut(&B.into()).unwrap().trucks.light = 1;
    assert_eq!(activity_water_due(content(), &s, &A.into()).unwrap(), 6);
    assert_eq!(activity_water_due(content(), &s, &B.into()).unwrap(), 3);
    for t in [
        Trucks {
            light: 2,
            medium: 0,
            heavy: 0,
        },
        Trucks {
            light: -1,
            medium: 0,
            heavy: 0,
        },
    ] {
        let before = s.clone();
        assert_eq!(
            transfer_activity_water_credit(content(), &mut s, &A.into(), &B.into(), t),
            Err(SupplyError::Invalid)
        );
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
    }
    s.land.units.get_mut(&B.into()).unwrap().location = Location::Hex {
        hex: "C4021".into(),
    };
    let before = s.clone();
    assert_eq!(
        transfer_activity_water_credit(content(), &mut s, &A.into(), &B.into(), Trucks::default()),
        Err(SupplyError::Invalid)
    );
    assert_eq!(
        serde_json::to_value(&s).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
}
/// Cases: airlog:52.42
#[test]
fn unknown_hq_is_not_treated_as_zero_and_legacy_paid_markers_remain_paid() {
    let mut s = fixture(false);
    let id = s
        .land
        .units
        .values()
        .find(|u| {
            u.toe.is_some()
                && rations::class(content(), &u.id)
                    .is_ok_and(|c| c.unit_type == "headquarters" && !c.max_toe_paren)
        })
        .unwrap()
        .id
        .clone();
    let before = s.clone();
    assert_eq!(
        consume_activity_water_forced(content(), &mut s, &id),
        Err(SupplyError::Unsupported {
            case: "airlog:52.42"
        })
    );
    assert_eq!(
        serde_json::to_value(&s).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    let stage = WaterStage::current(&s);
    s.logistics
        .rations
        .entry(A.into())
        .or_default()
        .activity_used_stage = Some(stage);
    assert_eq!(activity_water_due(content(), &s, &A.into()).unwrap(), 0);
    assert_eq!(
        consume_activity_water_forced(content(), &mut s, &A.into())
            .unwrap()
            .consumed,
        WaterPoints::ZERO
    );
}
