//! Values independently read from the three native repair chart images.
mod common;

use cna_tables::land::repair::{
    BrokenDownVehicleRepair, BrokenRepairResult as B, DestroyedTankColumn as C,
    DestroyedTanksRepair, RepairFacility as F, RepairLocation as L, RepairVehicle as V,
    TankRepairOutcome as O, VehicleRepairSupplyCosts, VehicleState as S,
};
use common::{bind_edited, replace_once, tables};

/// Cases: land:22.23, land:22.24, land:22.26, land:22.35, land:22.42
#[test]
fn attempted_point_costs_and_special_field_condition_match_native_chart() {
    let chart = &tables().land.vehicle_repair_supply_costs;
    for vehicle in [V::Truck, V::ArmoredCar, V::Recce] {
        let cost = chart.cost(L::Field, S::BrokenDown, vehicle).unwrap();
        assert_eq!((cost.supplies.fuel, cost.supplies.stores), (0, 0));
        assert!(!cost.special_field_tank_organization);
    }
    for vehicle in [V::Tank, V::SelfPropelledArtillery, V::TankDestroyer] {
        let cost = chart.cost(L::Field, S::BrokenDown, vehicle).unwrap();
        assert_eq!((cost.supplies.fuel, cost.supplies.stores), (1, 0));
    }
    for vehicle in [
        V::Truck,
        V::ArmoredCar,
        V::Recce,
        V::Tank,
        V::SelfPropelledArtillery,
        V::TankDestroyer,
        V::Gun,
    ] {
        let cost = chart.cost(L::Facility, S::BrokenDown, vehicle).unwrap();
        assert_eq!((cost.supplies.fuel, cost.supplies.stores), (1, 1));
        assert!(!cost.special_field_tank_organization);
        if vehicle != V::Tank {
            assert_eq!(chart.cost(L::Facility, S::Destroyed, vehicle), None);
            assert_eq!(chart.cost(L::Field, S::Destroyed, vehicle), None);
        }
    }
    assert_eq!(chart.cost(L::Field, S::BrokenDown, V::Gun), None);
    for location in [L::Field, L::Facility] {
        let cost = chart.cost(location, S::Destroyed, V::Tank).unwrap();
        assert_eq!((cost.supplies.fuel, cost.supplies.stores), (2, 2));
        assert_eq!(cost.special_field_tank_organization, location == L::Field);
    }
}

/// Cases: land:22.43, land:22.44
#[test]
fn all_destroyed_tank_cells_and_adjusted_roll_boundaries_match_native_chart() {
    let chart = &tables().land.destroyed_tanks_repair;
    for (column, outcomes) in [
        (
            C::Field,
            [
                O::Repaired,
                O::NoEffect,
                O::NoEffect,
                O::NoEffect,
                O::NoEffect,
                O::NoEffect,
                O::NoEffect,
            ],
        ),
        (
            C::AxisFacilityGerman,
            [
                O::Repaired,
                O::Repaired,
                O::Repaired,
                O::NoEffect,
                O::NoEffect,
                O::Junked,
                O::Junked,
            ],
        ),
        (
            C::AxisFacilityItalian,
            [
                O::Repaired,
                O::Repaired,
                O::NoEffect,
                O::NoEffect,
                O::Junked,
                O::Junked,
                O::Junked,
            ],
        ),
        (
            C::AxisFacilityCommonwealth,
            [
                O::Repaired,
                O::Repaired,
                O::NoEffect,
                O::NoEffect,
                O::NoEffect,
                O::Junked,
                O::Junked,
            ],
        ),
        (
            C::CommonwealthFacility,
            [
                O::Repaired,
                O::Repaired,
                O::NoEffect,
                O::NoEffect,
                O::NoEffect,
                O::Junked,
                O::Junked,
            ],
        ),
    ] {
        for (roll, expected) in (1..=7).zip(outcomes) {
            assert_eq!(
                chart.outcome(column, roll),
                Some(expected),
                "{column:?}/{roll}"
            );
        }
        for roll in [i32::MIN, 0, 8, i32::MAX] {
            assert_eq!(chart.outcome(column, roll), None);
        }
    }
}

/// Cases: land:22.23, land:22.24, land:22.25, land:22.34, land:22.8
#[test]
fn broken_repair_cells_preserve_selection_budget_and_starred_percentages() {
    let chart = &tables().land.broken_down_vehicle_repair;
    // roll, truck budget, AC/recce points, field tank percent/star, temporary percent/star, major
    for (roll, truck, ac, tank, tank_star, temporary, temp_star, major) in [
        (0, 2, 1, 25, false, 50, false, 75),
        (1, 2, 1, 25, false, 50, false, 75),
        (2, 1, 0, 10, true, 33, false, 50),
        (3, 0, 0, 10, true, 25, false, 50),
        (4, 0, 0, 10, true, 25, false, 50),
        (5, 0, 0, 0, false, 10, true, 33),
        (6, 0, 0, 0, false, 10, true, 33),
    ] {
        assert_eq!(
            chart.field_result(V::Truck, roll),
            Some(B::TruckSelectionBudget(truck))
        );
        for vehicle in [V::ArmoredCar, V::Recce] {
            assert_eq!(
                chart.field_result(vehicle, roll),
                Some(B::ArmoredCarReccePoints(ac))
            );
        }
        for vehicle in [V::Tank, V::SelfPropelledArtillery, V::TankDestroyer] {
            let Some(B::Percentage(rate)) = chart.field_result(vehicle, roll) else {
                panic!("tank percentage");
            };
            assert_eq!((rate.percent(), rate.singleton_zero()), (tank, tank_star));
        }
        let rate = chart.facility_percent(F::Temporary, roll).unwrap();
        assert_eq!(
            (rate.percent(), rate.singleton_zero()),
            (temporary, temp_star)
        );
        let rate = chart.facility_percent(F::Major, roll).unwrap();
        assert_eq!((rate.percent(), rate.singleton_zero()), (major, false));
        assert_eq!(chart.field_result(V::Gun, roll), None);
    }
    for (roll, major) in [(7, 25), (8, 10)] {
        for vehicle in [
            V::Truck,
            V::ArmoredCar,
            V::Recce,
            V::Tank,
            V::SelfPropelledArtillery,
            V::TankDestroyer,
        ] {
            assert_eq!(chart.field_result(vehicle, roll), Some(B::NotApplicable));
        }
        let temp = chart.facility_percent(F::Temporary, roll).unwrap();
        assert_eq!((temp.percent(), temp.singleton_zero()), (10, true));
        assert_eq!(
            chart.facility_percent(F::Major, roll).unwrap().percent(),
            major
        );
    }
    for roll in [i32::MIN, -1, 9, i32::MAX] {
        assert_eq!(chart.field_result(V::Truck, roll), None);
        assert_eq!(chart.facility_percent(F::Major, roll), None);
    }
}

/// Cases: land:22.25, land:22.34, land:22.8
#[test]
fn integer_ceiling_singleton_exception_and_large_counts_are_exact() {
    let chart = &tables().land.broken_down_vehicle_repair;
    let star = chart.facility_percent(F::Temporary, 5).unwrap();
    assert_eq!(star.repaired_points(1), Some(0));
    assert_eq!(star.repaired_points(2), Some(1));
    let plain_ten = chart.facility_percent(F::Major, 8).unwrap();
    assert_eq!(plain_ten.repaired_points(1), Some(1));
    let thirty_three = chart.facility_percent(F::Temporary, 2).unwrap();
    assert_eq!(thirty_three.repaired_points(3), Some(1));
    assert_eq!(thirty_three.repaired_points(4), Some(2));
    assert_eq!(thirty_three.repaired_points(100), Some(33));
    let seventy_five = chart.facility_percent(F::Major, 1).unwrap();
    assert_eq!(seventy_five.repaired_points(i32::MAX), Some(1_610_612_736));
    assert_eq!(seventy_five.repaired_points(0), Some(0));
    assert_eq!(seventy_five.repaired_points(-1), None);
}

/// Cases: land:22.23, land:22.24, land:22.26, land:22.35, land:22.42
#[test]
fn supply_binding_rejects_corrupt_or_unlisted_combinations() {
    for (from, to) in [
        ("fuel = 1, stores = 0", "fuel = -1, stores = 0"),
        (
            "[\"truck\", \"armored_car\", \"recce\"]",
            "[\"truck\", \"truck\", \"recce\"]",
        ),
        ("[\"all\"]", "[\"all\", \"tank\"]"),
        ("[\"all\"]", "[\"tank\"]"),
        ("footnotes = [\"star\"]", "footnotes = []"),
        ("[\"tank\"]", "[\"gun\"]"),
    ] {
        let error =
            bind_edited::<VehicleRepairSupplyCosts>("land/22.15-", |t| replace_once(t, from, to))
                .unwrap_err()
                .to_string();
        assert!(
            error.contains("22.15-vehicle-repair-supply-costs.toml"),
            "{error}"
        );
        assert!(error.contains("row"), "{error}");
    }
}

/// Cases: land:22.44, land:22.8
#[test]
fn dice_bindings_reject_gaps_overlaps_invalid_cells_and_mixed_na() {
    for (from, to) in [
        ("die_min = 2", "die_min = 3"),
        ("die_max = 1", "die_max = 2"),
        ("die_max = 7", "die_max = 2147483647"),
        ("field = \"R\"", "field = \"repair_maybe\""),
    ] {
        assert!(
            bind_edited::<DestroyedTanksRepair>("land/22.44-", |t| replace_once(t, from, to))
                .is_err()
        );
    }
    for (from, to) in [
        ("die_min = 2", "die_min = 3"),
        ("die_max = 1", "die_max = 2"),
        ("die_max = 8", "die_max = 2147483647"),
        ("truck = 2", "truck = 3"),
        ("armored_car_recce = 1", "armored_car_recce = -1"),
        ("tank_spa_td_percent = 25", "tank_spa_td_percent = 17"),
        (
            "tank_spa_td_percent = 25",
            "tank_spa_td_percent = 25, tank_spa_td_star = true",
        ),
        ("not_applicable = true", "not_applicable = true, truck = 0"),
        (
            "truck = 2, armored_car_recce = 1, tank_spa_td_percent = 25",
            "truck = 2, tank_spa_td_percent = 25",
        ),
    ] {
        let error =
            bind_edited::<BrokenDownVehicleRepair>("land/22.8-", |t| replace_once(t, from, to))
                .unwrap_err()
                .to_string();
        assert!(
            error.contains("22.8-broken-down-vehicle-repair.toml"),
            "{error}"
        );
        assert!(error.contains("row"), "{error}");
    }
}
