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
    let mut c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    // Explicitly omit a numeric source maximum; this is not cw.a's printed dash.
    assert!(c.units.classes["cw.e"].max_toe.is_some());
    c.units.classes.get_mut("cw.e").unwrap().max_toe = None;
    let mut s = fixture(false);
    let id = s
        .land
        .units
        .values()
        .find(|u| c.units.units[&u.id].class.as_deref() == Some("cw.e"))
        .unwrap()
        .id
        .clone();
    // An ordinary synthetic missing-maximum HQ remains outside computable house billing.
    s.land.units.get_mut(&id).unwrap().toe = Some(Toe::Normal(cna_content::units::NormalToe::N));
    let before = s.clone();
    assert_eq!(
        consume_activity_water_forced(&c, &mut s, &id),
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

/// Cases: airlog:52.41, airlog:52.42, airlog:52.43
/// Interpretations: interp:units-0005, interp:units-0006, interp:airlog-0015
#[test]
fn nine_numeric_headquarters_pay_actual_strength_heat_and_separate_trucks_once_per_stage() {
    let original = fixture(false);
    let ids: Vec<_> = original
        .land
        .units
        .values()
        .filter(|u| {
            super::super::supply::house_rule_hq_strength(content(), u)
                .unwrap()
                .is_some()
                && rations::class(content(), &u.id).unwrap().max_toe.is_some()
        })
        .map(|u| u.id.clone())
        .collect();
    assert_eq!(ids.len(), 9);
    assert_eq!(
        ids.iter()
            .filter(|id| id.as_str().starts_with("cw."))
            .count(),
        6
    );
    assert_eq!(
        ids.iter()
            .filter(|id| id.as_str().starts_with("it."))
            .count(),
        3
    );
    for id in ids {
        let maximum = rations::class(content(), &id).unwrap().max_toe.unwrap();
        assert!(maximum > 0);
        for (toe, strength) in [
            (Toe::Normal(cna_content::units::NormalToe::N), maximum),
            (Toe::Under { under: maximum - 1 }, maximum - 1),
            (Toe::Over { over: maximum + 1 }, maximum + 1),
        ] {
            for hot in [false, true] {
                // Authored HQ identities/counts in ordinary synthetic weather and location.
                let mut s = fixture(hot);
                let u = s.land.units.get_mut(&id).unwrap();
                u.location = Location::Hex {
                    hex: "C4020".into(),
                };
                u.toe = Some(toe.clone());
                u.trucks = Trucks {
                    light: 2,
                    medium: 1,
                    heavy: 1,
                };
                u.transport_trucks = Trucks::default();
                s.logistics.rations.remove(&id);
                set_reserve(&mut s, id.as_str(), 0);
                let multiplier = if hot { 2 } else { 1 };
                let due = (strength + 4) * multiplier;
                assert_eq!(
                    super::super::toe_strength(content(), &s.land.units[&id])
                        .unwrap()
                        .get(),
                    strength
                );
                assert_eq!(
                    rations::activity_points(content(), &s, &id).unwrap(),
                    strength + 4
                );
                assert_eq!(activity_water_due(content(), &s, &id).unwrap(), due);
                let dry = serde_json::to_value(&s).unwrap();
                assert_eq!(
                    spend_activity_water(content(), &mut s, &id),
                    Err(SupplyError::Insufficient)
                );
                assert_eq!(serde_json::to_value(&s).unwrap(), dry);
                set_reserve(&mut s, id.as_str(), due);
                spend_activity_water(content(), &mut s, &id).unwrap();
                let l = s.logistics.rations[&id]
                    .activity_water_ledger
                    .as_ref()
                    .unwrap();
                assert_eq!(
                    (l.body_required, l.body_paid),
                    (strength * multiplier, strength * multiplier)
                );
                assert_eq!(
                    l.truck_required,
                    TruckWater {
                        light: 2 * multiplier,
                        medium: multiplier,
                        heavy: multiplier
                    }
                );
                assert_eq!(l.truck_paid, l.truck_required);
                assert_eq!(
                    s.logistics.unit_supply[&id].activity_water,
                    WaterPoints::ZERO
                );
                assert_eq!(activity_water_due(content(), &s, &id).unwrap(), 0);
                let paid = serde_json::to_value(&s).unwrap();
                spend_activity_water(content(), &mut s, &id).unwrap();
                assert_eq!(serde_json::to_value(&s).unwrap(), paid);
                s.cursor.op_stage = Some(2);
                assert_eq!(activity_water_due(content(), &s, &id).unwrap(), due);
            }
        }
    }
}

/// Cases: land:4.46, airlog:49.12, airlog:49.13, airlog:52.42, airlog:52.43
/// Interpretations: interp:units-0007, interp:airlog-0015
#[test]
fn eighteen_not_applicable_hqs_have_zero_body_and_pay_only_separate_trucks() {
    let original = fixture(false);
    let ids: Vec<_> = original
        .land
        .units
        .values()
        .filter(|u| content().units.units[&u.id].class.as_deref() == Some("cw.a"))
        .map(|u| u.id.clone())
        .collect();
    assert_eq!(ids.len(), 18);
    assert_eq!(
        ids.iter()
            .filter(|id| matches!(
                original.land.units[*id].toe,
                Some(Toe::Normal(cna_content::units::NormalToe::N))
            ))
            .count(),
        17
    );
    assert_eq!(
        ids.iter()
            .filter(|id| original.land.units[*id].toe.is_none())
            .count(),
        1
    );
    for id in ids {
        for toe in [
            original.land.units[&id].toe.clone(),
            None,
            Some(Toe::Normal(cna_content::units::NormalToe::N)),
        ] {
            for hot in [false, true] {
                for trucks in [
                    Trucks::default(),
                    Trucks {
                        light: 2,
                        medium: 1,
                        heavy: 1,
                    },
                ] {
                    let mut s = fixture(hot);
                    let u = s.land.units.get_mut(&id).unwrap();
                    u.toe = toe.clone();
                    u.location = Location::Hex {
                        hex: "C4020".into(),
                    };
                    u.trucks = trucks;
                    u.transport_trucks = Trucks::default();
                    s.logistics.rations.remove(&id);
                    set_reserve(&mut s, id.as_str(), 0);
                    let n = trucks.light + trucks.medium + trucks.heavy;
                    let multiplier = if hot { 2 } else { 1 };
                    let due = n * multiplier;
                    assert_eq!(
                        super::super::toe_strength(content(), &s.land.units[&id])
                            .unwrap()
                            .get(),
                        0
                    );
                    assert_eq!(rations::activity_points(content(), &s, &id).unwrap(), n);
                    assert_eq!(activity_water_due(content(), &s, &id).unwrap(), due);
                    for quarters in [0, 1, 4, 28, 196, 396] {
                        let cp = quarters / 4 + i32::from(quarters % 4 != 0);
                        let expected = if cp == 0 {
                            0
                        } else {
                            content()
                                .tables
                                .airlog
                                .fuel_consumption
                                .fuel_for(1, cp)
                                .unwrap()
                                .get()
                                * n
                        };
                        assert_eq!(
                            super::super::movement_fuel_cost(content(), &s, &id, quarters)
                                .unwrap()
                                .get(),
                            expected
                        );
                    }
                    let characteristics = &content().tables.airlog.truck_characteristics;
                    let capacity = [
                        (cna_tables::airlog::trucks::TruckType::Light, trucks.light),
                        (cna_tables::airlog::trucks::TruckType::Medium, trucks.medium),
                        (cna_tables::airlog::trucks::TruckType::Heavy, trucks.heavy),
                    ]
                    .into_iter()
                    .map(|(kind, count)| {
                        characteristics.truck(kind).fuel_capacity_points * count * 10
                    })
                    .sum::<i32>();
                    assert_eq!(
                        super::super::fuel_capacity(content(), &s, &id)
                            .unwrap()
                            .get(),
                        capacity
                    );
                    if due > 0 {
                        let dry = serde_json::to_value(&s).unwrap();
                        assert_eq!(
                            spend_activity_water(content(), &mut s, &id),
                            Err(SupplyError::Insufficient)
                        );
                        assert_eq!(serde_json::to_value(&s).unwrap(), dry);
                    }
                    set_reserve(&mut s, id.as_str(), due);
                    spend_activity_water(content(), &mut s, &id).unwrap();
                    let ledger = s.logistics.rations[&id]
                        .activity_water_ledger
                        .as_ref()
                        .unwrap();
                    assert_eq!((ledger.body_required, ledger.body_paid), (0, 0));
                    assert_eq!(
                        ledger.truck_required,
                        TruckWater {
                            light: trucks.light * multiplier,
                            medium: trucks.medium * multiplier,
                            heavy: trucks.heavy * multiplier,
                        }
                    );
                    assert_eq!(ledger.truck_paid, ledger.truck_required);
                    assert_eq!(
                        s.logistics.unit_supply[&id].activity_water,
                        WaterPoints::ZERO
                    );
                    let paid = serde_json::to_value(&s).unwrap();
                    spend_activity_water(content(), &mut s, &id).unwrap();
                    assert_eq!(serde_json::to_value(&s).unwrap(), paid);
                    s.cursor.op_stage = Some(2);
                    assert_eq!(activity_water_due(content(), &s, &id).unwrap(), due);
                }
            }
        }
    }
}

/// Cases: airlog:52.41, airlog:52.42, land:4.46
/// Interpretations: interp:units-0005, interp:units-0006
#[test]
fn numeric_hq_water_keeps_parenthesized_and_weapon_paths_and_rejects_invalid_strength() {
    let mut s = fixture(false);
    let id = s
        .land
        .units
        .values()
        .find(|u| content().units.units[&u.id].class.as_deref() == Some("it.g"))
        .unwrap()
        .id
        .clone();
    let maximum = rations::class(content(), &id).unwrap().max_toe.unwrap();
    for toe in [
        Toe::Under { under: -1 },
        Toe::Under { under: maximum },
        Toe::Over { over: maximum },
    ] {
        s.land.units.get_mut(&id).unwrap().toe = Some(toe);
        let before = serde_json::to_value(&s).unwrap();
        assert_eq!(
            spend_activity_water(content(), &mut s, &id),
            Err(SupplyError::Invalid)
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
    }
    let weapon = content()
        .units
        .weapons
        .values()
        .find(|w| w.nation == "it" && w.kind == "gun")
        .unwrap();
    let u = s.land.units.get_mut(&id).unwrap();
    u.toe = Some(Toe::Weapons(vec![cna_content::units::WeaponPoints {
        weapon: weapon.id.clone(),
        n: 2,
    }]));
    u.trucks = Trucks {
        light: 1,
        ..Trucks::default()
    };
    assert_eq!(
        super::super::supply::house_rule_hq_strength(content(), u),
        Ok(None)
    );
    assert_eq!(rations::activity_points(content(), &s, &id).unwrap(), 3);
    let paren = s
        .land
        .units
        .values()
        .find(|u| {
            rations::class(content(), &u.id)
                .is_ok_and(|c| c.unit_type == "headquarters" && c.max_toe_paren)
        })
        .unwrap()
        .id
        .clone();
    let u = s.land.units.get_mut(&paren).unwrap();
    u.toe = Some(Toe::Normal(cna_content::units::NormalToe::N));
    u.trucks = Trucks {
        medium: 1,
        ..Trucks::default()
    };
    assert_eq!(
        super::super::supply::house_rule_hq_strength(content(), u),
        Ok(None)
    );
    assert_eq!(rations::activity_points(content(), &s, &paren).unwrap(), 1);
}

/// Cases: land:21.25, land:21.29, airlog:52.42, airlog:52.43
/// Interpretations: interp:airlog-0015
#[test]
fn partial_hot_credit_travels_to_a_marker_and_returns_without_body_or_reserve_credit() {
    let mut s = fixture(true);
    let id = A.into();
    set_reserve(&mut s, A, 5);
    consume_activity_water_forced(content(), &mut s, &id).unwrap();
    let stage = WaterStage::current(&s);
    let trucks = Trucks {
        light: 1,
        ..Trucks::default()
    };
    let paid = remove_activity_water_credit(content(), &mut s, &id, trucks).unwrap();
    assert_eq!(
        paid,
        TruckWater {
            light: 1,
            ..TruckWater::default()
        }
    );
    s.land.units.get_mut(&id).unwrap().trucks.light -= 1;
    let l = s.logistics.rations[&id]
        .activity_water_ledger
        .as_ref()
        .unwrap();
    assert_eq!((l.body_required, l.body_paid), (4, 4));
    assert_eq!(activity_water_due(content(), &s, &id).unwrap(), 6);
    assert_eq!(s.logistics.unit_supply[&id].activity_water.get(), 0);
    let mut recovered: State = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    restore_activity_water_credit(content(), &mut recovered, &id, trucks, paid, stage).unwrap();
    recovered.land.units.get_mut(&id).unwrap().trucks.light += 1;
    assert_eq!(activity_water_due(content(), &recovered, &id).unwrap(), 7);
    let l = recovered.logistics.rations[&id]
        .activity_water_ledger
        .as_ref()
        .unwrap();
    assert_eq!((l.body_required, l.body_paid), (4, 4));
    assert_eq!(l.truck_paid.light, 1);
    assert_eq!(
        consume_activity_water_forced(content(), &mut recovered, &id)
            .unwrap()
            .consumed
            .get(),
        0
    );
    set_reserve(&mut recovered, A, 7);
    spend_activity_water(content(), &mut recovered, &id).unwrap();
    assert_eq!(activity_water_due(content(), &recovered, &id).unwrap(), 0);
    assert_eq!(recovered.logistics.unit_supply[&id].activity_water.get(), 0);
    // An old marker's payment cannot satisfy a new-stage requirement.
    s.cursor.op_stage = Some(2);
    s.turn.weather.as_mut().unwrap().kind = WeatherKind::Normal;
    restore_activity_water_credit(content(), &mut s, &id, trucks, paid, stage).unwrap();
    s.land.units.get_mut(&id).unwrap().trucks.light += 1;
    assert_eq!(activity_water_due(content(), &s, &id).unwrap(), 6);
}
/// Cases: land:21.25, airlog:52.42
#[test]
fn invalid_marker_credit_and_excess_removals_leave_water_history_unchanged() {
    let mut s = fixture(true);
    let id = A.into();
    let before = serde_json::to_value(&s).unwrap();
    assert!(
        remove_activity_water_credit(
            content(),
            &mut s,
            &id,
            Trucks {
                light: 3,
                ..Trucks::default()
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    let stage = WaterStage::current(&s);
    assert!(
        restore_activity_water_credit(
            content(),
            &mut s,
            &id,
            Trucks {
                light: 1,
                ..Trucks::default()
            },
            TruckWater {
                light: 3,
                ..TruckWater::default()
            },
            stage
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert!(
        restore_activity_water_credit(
            content(),
            &mut s,
            &id,
            Trucks {
                light: 1,
                ..Trucks::default()
            },
            TruckWater {
                light: -1,
                ..TruckWater::default()
            },
            stage
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}
