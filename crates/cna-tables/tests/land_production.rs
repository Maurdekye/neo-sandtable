mod common;
use cna_tables::airlog::trucks::TruckType;
use cna_tables::land::production::*;
use common::{bind_edited, replace_once, tables};

fn check_line(
    entry: &AxisProductionEntry,
    index: usize,
    pool: usize,
    maximum: ProductionMaximum,
    first: i32,
    last: Option<i32>,
    french: bool,
) {
    let line = &entry.availability[index];
    assert_eq!(line.pool_index, pool);
    assert_eq!(line.maximum, maximum);
    assert_eq!(
        line.dates,
        ProductionDates {
            first_game_turn: first,
            last_game_turn: last
        }
    );
    assert_eq!(line.french_tunis_exception, french);
    for turn in [first, last.unwrap_or(111)] {
        assert_eq!(entry.availability_at(turn), Some(line));
    }
    assert!(!line.dates.contains(first - 1));
    if let Some(last) = last {
        assert!(!line.dates.contains(last + 1));
    }
}

/// Cases: land:20.66
#[test]
fn native_axis_truck_and_german_rows_preserve_quantities_and_periods() {
    let chart = &tables().land.axis_replacement_pool;
    let entry = chart.truck(TruckType::Light);
    assert_eq!(entry.pools, vec![835]);
    assert_eq!(entry.tonnage_per_point, 45);
    assert_eq!(entry.availability.len(), 2);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 4,
            period: ProductionPeriod::GameTurn,
        },
        4,
        Some(12),
        false,
    );
    check_line(
        entry,
        1,
        0,
        ProductionMaximum::Points {
            points: 15,
            period: ProductionPeriod::GameTurn,
        },
        13,
        None,
        false,
    );
    let entry = chart.truck(TruckType::Medium);
    assert_eq!(entry.pools, vec![2890]);
    assert_eq!(entry.tonnage_per_point, 80);
    assert_eq!(entry.availability.len(), 2);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 20,
            period: ProductionPeriod::GameTurn,
        },
        6,
        Some(12),
        false,
    );
    check_line(
        entry,
        1,
        0,
        ProductionMaximum::Points {
            points: 50,
            period: ProductionPeriod::GameTurn,
        },
        13,
        None,
        false,
    );
    let entry = chart.truck(TruckType::Heavy);
    assert_eq!(entry.pools, vec![525]);
    assert_eq!(entry.tonnage_per_point, 150);
    assert_eq!(entry.availability.len(), 2);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 3,
            period: ProductionPeriod::GameTurn,
        },
        5,
        Some(12),
        false,
    );
    check_line(
        entry,
        1,
        0,
        ProductionMaximum::Points {
            points: 8,
            period: ProductionPeriod::GameTurn,
        },
        13,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "Infantry")
        .unwrap();
    assert_eq!(entry.pools, vec![400]);
    assert_eq!(entry.tonnage_per_point, 30);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 12,
            period: ProductionPeriod::GameTurn,
        },
        38,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "Armed Recce")
        .unwrap();
    assert_eq!(entry.pools, vec![25]);
    assert_eq!(entry.tonnage_per_point, 80);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::HalfCalendarMonth,
        },
        47,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "Light AA")
        .unwrap();
    assert_eq!(entry.pools, vec![40]);
    assert_eq!(entry.tonnage_per_point, 13);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::GameTurn,
        },
        59,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "Heavy AA")
        .unwrap();
    assert_eq!(entry.pools, vec![10]);
    assert_eq!(entry.tonnage_per_point, 65);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::CalendarMonth,
        },
        59,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "7.5cm IG18")
        .unwrap();
    assert_eq!(entry.pools, vec![20]);
    assert_eq!(entry.tonnage_per_point, 5);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        45,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "10.5cm K18")
        .unwrap();
    assert_eq!(entry.pools, vec![40]);
    assert_eq!(entry.tonnage_per_point, 66);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        45,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "15cm sFH18")
        .unwrap();
    assert_eq!(entry.pools, vec![10]);
    assert_eq!(entry.tonnage_per_point, 65);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        45,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "15cm sIG33")
        .unwrap();
    assert_eq!(entry.pools, vec![5]);
    assert_eq!(entry.tonnage_per_point, 18);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        45,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "15cm K18")
        .unwrap();
    assert_eq!(entry.pools, vec![18]);
    assert_eq!(entry.tonnage_per_point, 150);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        45,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "17cm K18")
        .unwrap();
    assert_eq!(entry.pools, vec![5]);
    assert_eq!(entry.tonnage_per_point, 206);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::CalendarMonth,
        },
        65,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "21cm Mrs.18")
        .unwrap();
    assert_eq!(entry.pools, vec![5]);
    assert_eq!(entry.tonnage_per_point, 197);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::CalendarMonth,
        },
        60,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "SP 10.5cm")
        .unwrap();
    assert_eq!(entry.pools, vec![3]);
    assert_eq!(entry.tonnage_per_point, 120);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 3,
            period: ProductionPeriod::CalendarMonth,
        },
        63,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "5cm Pak38")
        .unwrap();
    assert_eq!(entry.pools, vec![25]);
    assert_eq!(entry.tonnage_per_point, 12);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        45,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "2.8cm s.Pz.B.41 28/20 Pak")
        .unwrap();
    assert_eq!(entry.pools, vec![15]);
    assert_eq!(entry.tonnage_per_point, 5);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        45,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "7.62cm Pak(R)")
        .unwrap();
    assert_eq!(entry.pools, vec![22]);
    assert_eq!(entry.tonnage_per_point, 15);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        64,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "Marder III")
        .unwrap();
    assert_eq!(entry.pools, vec![9]);
    assert_eq!(entry.tonnage_per_point, 132);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::CalendarMonth,
        },
        64,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "PzII")
        .unwrap();
    assert_eq!(entry.pools, vec![5]);
    assert_eq!(entry.tonnage_per_point, 135);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        41,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "PzIII E")
        .unwrap();
    assert_eq!(entry.pools, vec![38]);
    assert_eq!(entry.tonnage_per_point, 190);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 5,
            period: ProductionPeriod::GameTurn,
        },
        53,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "PzIII H")
        .unwrap();
    assert_eq!(entry.pools, vec![31]);
    assert_eq!(entry.tonnage_per_point, 200);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 5,
            period: ProductionPeriod::GameTurn,
        },
        53,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "PzIII J Special")
        .unwrap();
    assert_eq!(entry.pools, vec![17]);
    assert_eq!(entry.tonnage_per_point, 220);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::GameTurn,
        },
        76,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "PzIV D")
        .unwrap();
    assert_eq!(entry.pools, vec![18]);
    assert_eq!(entry.tonnage_per_point, 215);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        59,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "PzIV E")
        .unwrap();
    assert_eq!(entry.pools, vec![15]);
    assert_eq!(entry.tonnage_per_point, 220);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::Unreadable,
        },
        59,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::German, "PzIV F2 Special")
        .unwrap();
    assert_eq!(entry.pools, vec![7]);
    assert_eq!(entry.tonnage_per_point, 235);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::HalfCalendarMonth,
        },
        81,
        None,
        false,
    );
    assert!(
        chart
            .equipment(AxisProductionNation::German, "M 11/39")
            .is_none()
    );
    assert!(chart.truck(TruckType::Light).availability_at(0).is_none());
    assert!(chart.truck(TruckType::Light).availability_at(112).is_none());
}

/// Cases: land:20.66
#[test]
fn native_italian_rows_preserve_shared_pools_and_french_exception() {
    let chart = &tables().land.axis_replacement_pool;
    let entry = chart
        .equipment(AxisProductionNation::Italian, "Infantry")
        .unwrap();
    assert_eq!(entry.pools, vec![100, 1100]);
    assert_eq!(entry.tonnage_per_point, 30);
    assert_eq!(entry.availability.len(), 3);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 5,
            period: ProductionPeriod::GameTurn,
        },
        5,
        Some(8),
        false,
    );
    check_line(
        entry,
        1,
        0,
        ProductionMaximum::Points {
            points: 10,
            period: ProductionPeriod::GameTurn,
        },
        9,
        Some(24),
        false,
    );
    check_line(
        entry,
        2,
        1,
        ProductionMaximum::Points {
            points: 25,
            period: ProductionPeriod::GameTurn,
        },
        25,
        None,
        false,
    );
    let entry = chart
        .equipment(
            AxisProductionNation::Italian,
            "Armored Reconnaissance/Armored Car",
        )
        .unwrap();
    assert_eq!(entry.pools, vec![25]);
    assert_eq!(entry.tonnage_per_point, 59);
    assert_eq!(entry.availability.len(), 1);
    assert!(entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        31,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "Light AA")
        .unwrap();
    assert_eq!(entry.pools, vec![15, 45]);
    assert_eq!(entry.tonnage_per_point, 6);
    assert_eq!(entry.availability.len(), 2);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::GameTurn,
        },
        2,
        Some(24),
        false,
    );
    check_line(
        entry,
        1,
        1,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        25,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "Heavy AA (75/46)")
        .unwrap();
    assert_eq!(entry.pools, vec![5, 25]);
    assert_eq!(entry.tonnage_per_point, 59);
    assert_eq!(entry.availability.len(), 2);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        10,
        Some(24),
        false,
    );
    check_line(
        entry,
        1,
        1,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        25,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "Heavy AA (90/53)")
        .unwrap();
    assert_eq!(entry.pools, vec![10]);
    assert_eq!(entry.tonnage_per_point, 92);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        73,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "47/32 Mod. 37")
        .unwrap();
    assert_eq!(entry.pools, vec![8, 25]);
    assert_eq!(entry.tonnage_per_point, 9);
    assert_eq!(entry.availability.len(), 2);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::GameTurn,
        },
        10,
        Some(24),
        false,
    );
    check_line(
        entry,
        1,
        1,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        25,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "65/17 Gun")
        .unwrap();
    assert_eq!(entry.pools, vec![5, 15]);
    assert_eq!(entry.tonnage_per_point, 3);
    assert_eq!(entry.availability.len(), 2);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        9,
        Some(24),
        false,
    );
    check_line(
        entry,
        1,
        1,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        25,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "75/27 Gun")
        .unwrap();
    assert_eq!(entry.pools, vec![12, 26]);
    assert_eq!(entry.tonnage_per_point, 12);
    assert_eq!(entry.availability.len(), 2);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::GameTurn,
        },
        9,
        Some(24),
        false,
    );
    check_line(
        entry,
        1,
        1,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::GameTurn,
        },
        25,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "75/18 Howitzer")
        .unwrap();
    assert_eq!(entry.pools, vec![6, 5]);
    assert_eq!(entry.tonnage_per_point, 12);
    assert_eq!(entry.availability.len(), 2);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        9,
        Some(24),
        false,
    );
    check_line(
        entry,
        1,
        1,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::CalendarMonth,
        },
        25,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "100/17 Howitzer")
        .unwrap();
    assert_eq!(entry.pools, vec![10, 22]);
    assert_eq!(entry.tonnage_per_point, 17);
    assert_eq!(entry.availability.len(), 2);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn,
        },
        9,
        Some(24),
        false,
    );
    check_line(
        entry,
        1,
        1,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::GameTurn,
        },
        25,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "105/28 Gun")
        .unwrap();
    assert_eq!(entry.pools, vec![3, 6]);
    assert_eq!(entry.tonnage_per_point, 48);
    assert_eq!(entry.availability.len(), 2);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::CalendarMonth,
        },
        9,
        Some(24),
        false,
    );
    check_line(
        entry,
        1,
        1,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::CalendarMonth,
        },
        25,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "Semovente 75/18 SP Gun")
        .unwrap();
    assert_eq!(entry.pools, vec![8]);
    assert_eq!(entry.tonnage_per_point, 60);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::CalendarMonth,
        },
        65,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "149/13 Howitzer")
        .unwrap();
    assert_eq!(entry.pools, vec![6]);
    assert_eq!(entry.tonnage_per_point, 84);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::CalendarMonth,
        },
        25,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "ParaArt")
        .unwrap();
    assert_eq!(entry.pools, vec![4]);
    assert_eq!(entry.tonnage_per_point, 3);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::GameTurn,
        },
        96,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "149mm (Fr.)")
        .unwrap();
    assert_eq!(entry.pools, vec![10]);
    assert_eq!(entry.tonnage_per_point, 50);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::GameTurn,
        },
        39,
        None,
        true,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "155mm Rimhailo (Fr.)")
        .unwrap();
    assert_eq!(entry.pools, vec![10]);
    assert_eq!(entry.tonnage_per_point, 53);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::GameTurn,
        },
        39,
        None,
        true,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "CV L.3 or CV 33/35")
        .unwrap();
    assert_eq!(entry.pools, vec![68]);
    assert_eq!(entry.tonnage_per_point, 16);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 5,
            period: ProductionPeriod::GameTurn,
        },
        3,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "CA L6/40")
        .unwrap();
    assert_eq!(entry.pools, vec![5, 21]);
    assert_eq!(entry.tonnage_per_point, 28);
    assert_eq!(entry.availability.len(), 2);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::CalendarMonth,
        },
        33,
        Some(68),
        false,
    );
    check_line(
        entry,
        1,
        1,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::GameTurn,
        },
        69,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "CA M 13/40")
        .unwrap();
    assert_eq!(entry.pools, vec![49]);
    assert_eq!(entry.tonnage_per_point, 70);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::GameTurn,
        },
        25,
        None,
        false,
    );
    let entry = chart
        .equipment(AxisProductionNation::Italian, "CA M 14/41")
        .unwrap();
    assert_eq!(entry.pools, vec![61]);
    assert_eq!(entry.tonnage_per_point, 73);
    assert_eq!(entry.availability.len(), 1);
    assert!(!entry.autobelinda_41_only);
    check_line(
        entry,
        0,
        0,
        ProductionMaximum::Points {
            points: 3,
            period: ProductionPeriod::GameTurn,
        },
        59,
        None,
        false,
    );
    let inf = chart
        .equipment(AxisProductionNation::Italian, "Infantry")
        .unwrap();
    assert_eq!(inf.pools.iter().sum::<i32>(), 1200);
    assert_eq!(
        inf.availability
            .iter()
            .map(|l| l.pool_index)
            .collect::<Vec<_>>(),
        vec![0, 0, 1]
    );
    assert!(
        chart
            .equipment(AxisProductionNation::Italian, "M 11/39")
            .is_none()
    );
}

/// Cases: land:20.78
#[test]
fn native_commonwealth_all_truck_and_infantry_cells_preserve_printed_none() {
    let chart = &tables().land.commonwealth_production;
    let expected = [
        [0, 10, 0],
        [2, 12, 0],
        [3, 15, 2],
        [4, 15, 3],
        [4, 18, 3],
        [5, 20, 4],
    ];
    for (die, row) in (1..=6).zip(expected) {
        for (kind, count) in [TruckType::Light, TruckType::Medium, TruckType::Heavy]
            .into_iter()
            .zip(row)
        {
            assert_eq!(
                chart.trucks(TruckProductionColumn::GameTurns1To30, kind, die),
                Some(count)
            );
        }
    }
    let expected = [
        [3, 30, 3],
        [5, 38, 4],
        [9, 44, 6],
        [11, 48, 8],
        [13, 55, 10],
        [14, 60, 13],
    ];
    for (die, row) in (1..=6).zip(expected) {
        for (kind, count) in [TruckType::Light, TruckType::Medium, TruckType::Heavy]
            .into_iter()
            .zip(row)
        {
            assert_eq!(
                chart.trucks(TruckProductionColumn::GameTurns31To107, kind, die),
                Some(count)
            );
        }
    }
    let expected = [
        [
            InfantryProductionResult::PrintedNone,
            InfantryProductionResult::PrintedNone,
            InfantryProductionResult::Points(5),
            InfantryProductionResult::PrintedNone,
        ],
        [
            InfantryProductionResult::Points(3),
            InfantryProductionResult::Points(4),
            InfantryProductionResult::Points(22),
            InfantryProductionResult::Points(5),
        ],
        [
            InfantryProductionResult::Points(15),
            InfantryProductionResult::Points(10),
            InfantryProductionResult::Points(25),
            InfantryProductionResult::Points(3),
        ],
        [
            InfantryProductionResult::PrintedNone,
            InfantryProductionResult::Points(22),
            InfantryProductionResult::Points(13),
            InfantryProductionResult::Points(2),
        ],
        [
            InfantryProductionResult::Points(5),
            InfantryProductionResult::Points(13),
            InfantryProductionResult::Points(28),
            InfantryProductionResult::Points(11),
        ],
        [
            InfantryProductionResult::Points(6),
            InfantryProductionResult::Points(15),
            InfantryProductionResult::Points(20),
            InfantryProductionResult::PrintedNone,
        ],
        [
            InfantryProductionResult::Points(4),
            InfantryProductionResult::Points(8),
            InfantryProductionResult::Points(16),
            InfantryProductionResult::Points(13),
        ],
        [
            InfantryProductionResult::Points(5),
            InfantryProductionResult::Points(18),
            InfantryProductionResult::Points(30),
            InfantryProductionResult::Points(1),
        ],
        [
            InfantryProductionResult::Points(8),
            InfantryProductionResult::Points(20),
            InfantryProductionResult::Points(19),
            InfantryProductionResult::Points(5),
        ],
        [
            InfantryProductionResult::Points(10),
            InfantryProductionResult::Points(25),
            InfantryProductionResult::Points(35),
            InfantryProductionResult::Points(7),
        ],
        [
            InfantryProductionResult::Points(20),
            InfantryProductionResult::PrintedNone,
            InfantryProductionResult::Points(8),
            InfantryProductionResult::PrintedNone,
        ],
    ];
    for (sum, row) in (2..=12).zip(expected) {
        for (column, result) in InfantryProductionColumn::ALL.into_iter().zip(row) {
            assert_eq!(chart.infantry(column, sum), Some(result));
        }
    }
    for bad in [i32::MIN, 0, 7, i32::MAX] {
        assert!(
            chart
                .trucks(
                    TruckProductionColumn::GameTurns1To30,
                    TruckType::Medium,
                    bad
                )
                .is_none()
        );
    }
    for bad in [i32::MIN, 1, 13, i32::MAX] {
        assert!(
            chart
                .infantry(InfantryProductionColumn::GameTurns3To30, bad)
                .is_none()
        );
    }
}

/// Cases: land:20.78
#[test]
fn native_commonwealth_equipment_limits_and_dates_keep_dash_distinct() {
    let chart = &tables().land.commonwealth_production;
    let entry = chart.equipment("Armored Recce/Armored Car").unwrap();
    assert_eq!(entry.pool_points, 90);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::GameTurn
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 3,
            last_game_turn: None
        }
    );
    let entry = chart.equipment("Light AA").unwrap();
    assert_eq!(entry.pool_points, 75);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 7,
            last_game_turn: None
        }
    );
    let entry = chart.equipment("Heavy AA").unwrap();
    assert_eq!(entry.pool_points, 15);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::CalendarMonth
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 11,
            last_game_turn: None
        }
    );
    let entry = chart.equipment("25-pounders").unwrap();
    assert_eq!(entry.pool_points, 250);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 4,
            period: ProductionPeriod::GameTurn
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 11,
            last_game_turn: None
        }
    );
    let entry = chart.equipment("4.5-inch Guns").unwrap();
    assert_eq!(entry.pool_points, 25);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 3,
            period: ProductionPeriod::GameTurn
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 76,
            last_game_turn: None
        }
    );
    let entry = chart.equipment("5.5-inch Howitzers").unwrap();
    assert_eq!(entry.pool_points, 12);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 83,
            last_game_turn: None
        }
    );
    let entry = chart.equipment("155mm Howitzers").unwrap();
    assert_eq!(entry.pool_points, 6);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 1,
            period: ProductionPeriod::GameTurn
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 51,
            last_game_turn: Some(59)
        }
    );
    let entry = chart.equipment("2-pounders").unwrap();
    assert_eq!(entry.pool_points, 60);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 5,
            period: ProductionPeriod::CalendarMonth
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 5,
            last_game_turn: Some(86)
        }
    );
    let entry = chart.equipment("6-pounders").unwrap();
    assert_eq!(entry.pool_points, 80);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 5,
            period: ProductionPeriod::GameTurn
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 75,
            last_game_turn: None
        }
    );
    let entry = chart.equipment("SP 6-pounders").unwrap();
    assert_eq!(entry.pool_points, 7);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 3,
            period: ProductionPeriod::GameTurn
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 76,
            last_game_turn: None
        }
    );
    let entry = chart.equipment("17-pounders").unwrap();
    assert_eq!(entry.pool_points, 6);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 3,
            period: ProductionPeriod::GameTurn
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 103,
            last_game_turn: None
        }
    );
    let entry = chart.equipment("Mark VI Light").unwrap();
    assert_eq!(entry.pool_points, 15);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 3,
            period: ProductionPeriod::CalendarMonth
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 3,
            last_game_turn: Some(38)
        }
    );
    let entry = chart.equipment("A9 Cruiser").unwrap();
    assert_eq!(entry.pool_points, 8);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::CalendarMonth
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 3,
            last_game_turn: Some(38)
        }
    );
    let entry = chart.equipment("A10 Cruiser").unwrap();
    assert_eq!(entry.pool_points, 10);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 3,
            period: ProductionPeriod::CalendarMonth
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 3,
            last_game_turn: Some(38)
        }
    );
    let entry = chart.equipment("A13 Cruiser").unwrap();
    assert_eq!(entry.pool_points, 8);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::CalendarMonth
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 9,
            last_game_turn: Some(38)
        }
    );
    let entry = chart.equipment("Crusader Mk I").unwrap();
    assert_eq!(entry.pool_points, 35);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 5,
            period: ProductionPeriod::CalendarMonth
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 31,
            last_game_turn: Some(76)
        }
    );
    let entry = chart.equipment("Crusader Mk II").unwrap();
    assert_eq!(entry.pool_points, 30);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 5,
            period: ProductionPeriod::CalendarMonth
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 41,
            last_game_turn: Some(78)
        }
    );
    let entry = chart.equipment("Crusader Mk III").unwrap();
    assert_eq!(entry.pool_points, 18);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 6,
            period: ProductionPeriod::HalfCalendarMonth
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 88,
            last_game_turn: None
        }
    );
    let entry = chart.equipment("Matilda").unwrap();
    assert_eq!(entry.pool_points, 25);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 2,
            period: ProductionPeriod::CalendarMonth
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 9,
            last_game_turn: Some(74)
        }
    );
    let entry = chart.equipment("Valentine").unwrap();
    assert_eq!(entry.pool_points, 20);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 3,
            period: ProductionPeriod::CalendarMonth
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 39,
            last_game_turn: None
        }
    );
    let entry = chart.equipment("Stuart").unwrap();
    assert_eq!(entry.pool_points, 44);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 10,
            period: ProductionPeriod::CalendarMonth
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 35,
            last_game_turn: Some(54)
        }
    );
    let entry = chart.equipment("Grant").unwrap();
    assert_eq!(entry.pool_points, 56);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 8,
            period: ProductionPeriod::CalendarMonth
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 66,
            last_game_turn: None
        }
    );
    let entry = chart.equipment("Sherman").unwrap();
    assert_eq!(entry.pool_points, 62);
    assert_eq!(
        entry.maximum,
        ProductionMaximum::Points {
            points: 12,
            period: ProductionPeriod::GameTurn
        }
    );
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 89,
            last_game_turn: None
        }
    );
    let entry = chart.equipment("Churchill").unwrap();
    assert_eq!(entry.pool_points, 1);
    assert_eq!(entry.maximum, ProductionMaximum::PrintedDash);
    assert_eq!(
        entry.dates,
        ProductionDates {
            first_game_turn: 90,
            last_game_turn: None
        }
    );
    assert!(chart.equipment("Tiger Convoy").is_none());
    let dates = chart.equipment("155mm Howitzers").unwrap().dates;
    assert!(!dates.contains(50));
    assert!(dates.contains(51));
    assert!(dates.contains(59));
    assert!(!dates.contains(60));
}

/// Cases: land:20.66
#[test]
fn malformed_axis_rows_refuse_loss_of_pool_identity_or_uncertainty() {
    for key in ["truck_row", "german_row", "italian_row"] {
        assert!(
            bind_edited::<AxisReplacementPool>("land/20.66-", |s| {
                let mut value: toml::Value = toml::from_str(s).unwrap();
                value[key].as_array_mut().unwrap().pop();
                toml::to_string(&value).unwrap()
            })
            .is_err()
        );
    }
    for (from, to) in [
        ("truck_type = \"medium\"", "truck_type = \"light\""),
        ("number = 835", "number = 0"),
        ("tonnage = 45", "tonnage = -1"),
        (
            "max = 4, first_game_turn = 4",
            "max = 836, first_game_turn = 4",
        ),
        (
            "max = 15, first_game_turn = 13",
            "max = 15, first_game_turn = 12",
        ),
        ("item = \"Infantry\"", "item = \"Unsupported\""),
        ("max_marker = \"unreadable\"", "max_marker = \"dagger\""),
        ("max_marker = \"star\"", "max_marker = \"double_dagger\""),
        ("numbers = [100, 1100]", "numbers = [100, 2147483647]"),
        (
            "last_game_turn = 8, pool = 100",
            "last_game_turn = 8, pool = 1100",
        ),
        (
            "first_game_turn = 25, pool = 1100",
            "first_game_turn = 25, pool = 999",
        ),
        ("pool = 25 },", "pool = 25, marker = \"unreadable\" },"),
        (
            "item_marker = \"star_all_autobelinda_41\"",
            "item_marker = \"invented\"",
        ),
        (
            "first_game_turn = 39, pool = 10",
            "first_game_turn = 40, pool = 10",
        ),
        ("marker = \"double_dagger\"", "marker = \"dagger\""),
    ] {
        let error =
            bind_edited::<AxisReplacementPool>("land/20.66-", |s| replace_once(s, from, to))
                .unwrap_err();
        assert!(error.to_string().contains("20.66"), "{error}");
    }
    assert!(
        bind_edited::<AxisReplacementPool>("land/20.66-", |s| s.replace(
            "[[german_row]]\nitem = \"PzIV F2 Special\"",
            "[[german_row]]\nitem = \"PzIV E\""
        ))
        .is_err()
    );
}

/// Cases: land:20.78
#[test]
fn malformed_commonwealth_rows_refuse_missing_none_dash_or_dice_cells() {
    for key in ["truck_row", "infantry_row", "chart_row"] {
        assert!(
            bind_edited::<CommonwealthProduction>("land/20.78-", |s| {
                let mut value: toml::Value = toml::from_str(s).unwrap();
                value[key].as_array_mut().unwrap().pop();
                toml::to_string(&value).unwrap()
            })
            .is_err()
        );
    }
    for (from, to) in [
        ("die = 1", "die = 2"),
        ("die = 1", "die = 0"),
        ("light = 0, medium = 10", "light = -1, medium = 10"),
        ("dice_sum = 2", "dice_sum = 3"),
        ("dice_sum = 2", "dice_sum = 13"),
        ("gt_3_to_30 = 0", "gt_3_to_30 = 1"),
        ("gt_31_to_46 = 0, ", ""),
        (
            "none_in = ['gt_3_to_30', 'gt_31_to_46', 'gt_103_to_107']",
            "none_in = []",
        ),
        ("none_in = []", "none_in = ['gt_3_to_30']"),
        ("item = \"Light AA\"", "item = \"Heavy AA\""),
        ("number = 90", "number = -1"),
        ("max_per_game_turn = 2", "max_per_game_turn = 0"),
        (
            "max_limit_period = \"month\"",
            "max_limit_period = \"unknown\"",
        ),
        ("last_game_turn = 59", "last_game_turn = 50"),
        (
            "max_per_game_turn_printed_dash = true",
            "max_per_game_turn_printed_dash = false",
        ),
        (
            "max_per_game_turn_printed_dash = true",
            "max_per_game_turn_printed_dash = true\nmax_per_game_turn = 1",
        ),
        (
            "max_per_game_turn_printed_dash = true",
            "max_per_game_turn = 1",
        ),
    ] {
        let error =
            bind_edited::<CommonwealthProduction>("land/20.78-", |s| replace_once(s, from, to))
                .unwrap_err();
        assert!(error.to_string().contains("20.78"), "{error}");
    }
}
