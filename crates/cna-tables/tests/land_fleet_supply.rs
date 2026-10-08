mod common;

use cna_tables::land::{
    fleet::{CommonwealthFleetReinforcement, MaltaGroup, ShipType},
    simplified_supply::{
        AxisSupplyAvailability, CommonwealthSupplyAvailability, CommonwealthSupplyPeriod,
        TonnageAvailability,
    },
};
use common::{bind_edited, replace_once, tables};

/// Cases: land:30.6
#[test]
fn native_fleet_groups_names_types_and_malta_limits() {
    use ShipType::*;
    let expected = [
        (
            (1, 1),
            vec![
                (Battleship, "Valiant", None),
                (LightCruiser, "Arethusa", Some(MaltaGroup::Star)),
                (AntiAircraftCruiser, "Dido", Some(MaltaGroup::Star)),
                (AntiAircraftCruiser, "Eurylaus", Some(MaltaGroup::Star)),
                (Destroyer, "Airedale", Some(MaltaGroup::Dagger)),
                (Destroyer, "Bedouin", Some(MaltaGroup::Dagger)),
                (Destroyer, "Jervis", Some(MaltaGroup::Dagger)),
                (Destroyer, "Lance", Some(MaltaGroup::Dagger)),
                (Destroyer, "Ledbury", Some(MaltaGroup::Dagger)),
                (Destroyer, "Mohawk", Some(MaltaGroup::Dagger)),
                (Destroyer, "Nubian", Some(MaltaGroup::Dagger)),
            ],
        ),
        (
            (8, 3),
            vec![
                (Battleship, "Barham", None),
                (HeavyCruiser, "York", None),
                (LightCruiser, "Ajax", None),
                (Destroyer, "Marne", None),
                (Destroyer, "Partridge", None),
            ],
        ),
        (
            (33, 1),
            vec![
                (Battleship, "Queen Elizabeth", None),
                (HeavyCruiser, "Fiji", None),
                (AntiAircraftCruiser, "Naiad", None),
            ],
        ),
    ];
    let chart = &tables().land.cw_fleet_reinforcement;
    for ((turn, stage), expected) in expected {
        let actual: Vec<_> = chart
            .ships(turn, stage)
            .unwrap()
            .iter()
            .map(|s| (s.ship_type, s.name.as_str(), s.footnotes.first().copied()))
            .collect();
        assert_eq!(actual, expected);
    }
    assert_eq!(MaltaGroup::Star.selection_limit(), 1);
    assert_eq!(MaltaGroup::Dagger.selection_limit(), 2);
    for (turn, stage) in [(0, 1), (1, 0), (1, 2), (8, 1), (33, 3), (u16::MAX, u8::MAX)] {
        assert!(chart.ships(turn, stage).is_none());
    }
}

/// Cases: land:32.44, land:32.46
#[test]
fn native_axis_supply_all_cells_include_zero_and_confirmed_three_c() {
    // Independent native-image readings; planning-month selection is outside this lookup.
    let expected = [
        [0, 0, 1, 1, 1, 1, 2],
        [0, 1, 1, 1, 2, 2, 3],
        [1, 1, 1, 2, 2, 3, 3],
        [1, 2, 2, 3, 3, 4, 4],
        [2, 2, 3, 3, 3, 4, 5],
        [2, 3, 3, 3, 4, 4, 6],
    ];
    let chart = &tables().land.axis_supply_availability;
    for die in 1..=6 {
        for (i, column) in TonnageAvailability::ALL.into_iter().enumerate() {
            assert_eq!(
                chart.supply_units(die, column),
                Some(expected[(die - 1) as usize][i])
            );
        }
    }
    for die in [i32::MIN, 0, 7, i32::MAX] {
        assert_eq!(chart.supply_units(die, TonnageAvailability::A), None);
    }
}

/// Cases: land:32.45, land:32.47
#[test]
fn native_commonwealth_supply_all_arrival_period_cells() {
    let expected = [
        [1, 2, 3],
        [1, 3, 4],
        [2, 3, 5],
        [2, 3, 5],
        [3, 4, 6],
        [3, 4, 7],
    ];
    let chart = &tables().land.cw_supply_availability;
    for die in 1..=6 {
        for (i, period) in CommonwealthSupplyPeriod::ALL.into_iter().enumerate() {
            assert_eq!(
                chart.supply_units(die, period),
                Some(expected[(die - 1) as usize][i])
            );
        }
    }
    for die in [i32::MIN, 0, 7, i32::MAX] {
        assert_eq!(
            chart.supply_units(die, CommonwealthSupplyPeriod::June1942Onward),
            None
        );
    }
}

/// Cases: land:30.6
#[test]
fn malformed_fleet_rejects_identity_arrival_and_allocation_conflicts() {
    for (from, to) in [
        ("name = \"Valiant\"", "name = \"\""),
        ("name = \"Barham\"", "name = \"Valiant\""),
        ("type = \"BB\"", "type = \"submarine\""),
        ("when = \"3/8\"", "when = \"3/9\""),
        ("op_stage = 3", "op_stage = 2"),
        ("game_turn = 33", "game_turn = 0"),
        ("footnotes = [\"star\"]", "footnotes = [\"star\", \"star\"]"),
        (
            "type = \"DD\", name = \"Airedale\"",
            "type = \"BB\", name = \"Airedale\"",
        ),
        (
            "name = \"Barham\"",
            "name = \"Barham\", footnotes = [\"star\"]",
        ),
        ("id = \"dagger\"", "id = \"star\""),
        ("  { type = \"CA\", name = \"Fiji\" },", ""),
        ("footnotes = [\"star\"]", "footnotes = []"),
    ] {
        let error = bind_edited::<CommonwealthFleetReinforcement>("land/30.6-", |text| {
            replace_once(text, from, to)
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("30.6-commonwealth-fleet-reinforcement.toml")
        );
        assert!(!error.field.is_empty());
    }
}

/// Cases: land:32.46
#[test]
fn malformed_axis_supply_rejects_dice_and_column_gaps() {
    for (from, to) in [
        ("die = 1", "die = 2"),
        ("die = 1", "die = 0"),
        ("die = 6", "die = 7"),
        ("A = 0", "A = -1"),
        ("A = 0, ", ""),
        ("A = 0", "H = 0"),
    ] {
        let error = bind_edited::<AxisSupplyAvailability>("land/32.46-", |text| {
            replace_once(text, from, to)
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("32.46-axis-simplified-supply-availability.toml")
        );
        assert!(!error.field.is_empty());
    }
    assert!(
        bind_edited::<AxisSupplyAvailability>("land/32.46-", |text| text
            [..text.rfind("[[row]]").unwrap()]
            .into())
        .is_err()
    );
}

/// Cases: land:32.47
#[test]
fn malformed_commonwealth_supply_rejects_period_and_quantity_errors() {
    for (from, to) in [
        ("die = 1", "die = 2"),
        ("die = 1", "die = 0"),
        ("die = 6", "die = 7"),
        ("period_I = 1", "period_I = -1"),
        ("period_I = 1, ", ""),
        ("period_I = 1", "period_IV = 1"),
    ] {
        let error = bind_edited::<CommonwealthSupplyAvailability>("land/32.47-", |text| {
            replace_once(text, from, to)
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("32.47-commonwealth-simplified-supply-availability.toml")
        );
        assert!(!error.field.is_empty());
    }
    assert!(
        bind_edited::<CommonwealthSupplyAvailability>("land/32.47-", |text| text
            [..text.rfind("[[row]]").unwrap()]
            .into())
        .is_err()
    );
}
