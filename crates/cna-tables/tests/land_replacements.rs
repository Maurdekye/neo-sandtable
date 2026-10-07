mod common;

use std::collections::BTreeMap;

use cna_tables::land::replacements::{
    ConversionNote as Note, ReplacementConversion, ReplacementPointKind as Kind,
    ReplacementRequirement as Requirement, ReplacementUnit as Unit,
};
use common::{bind_edited, replace_once, tables};

fn assert_cost(actual: &Requirement, expected: &[(Kind, i32)]) {
    let Requirement::All(cost) = actual else {
        panic!("expected one complete payment");
    };
    assert_eq!(
        cost.points(),
        &expected.iter().copied().collect::<BTreeMap<_, _>>()
    );
}

fn assert_choices(actual: &Requirement, expected: &[&[(Kind, i32)]]) {
    let Requirement::Alternatives(choices) = actual else {
        panic!("expected mutually exclusive payments");
    };
    assert_eq!(choices.len(), expected.len());
    for (cost, points) in choices.iter().zip(expected) {
        assert_eq!(
            cost.points(),
            &points.iter().copied().collect::<BTreeMap<_, _>>()
        );
    }
}

/// Cases: land:20.3
#[test]
fn native_conversion_chart_all_nineteen_rows_and_seven_notes() {
    let chart = &tables().land.replacement_conversion;
    // Read twice from the native chart, including its key; not from the authored TOML.
    for (unit, kind, count) in [
        (Unit::AnyHeadquartersUnit, Kind::Infantry, 2),
        (Unit::Commando, Kind::Infantry, 3),
        (Unit::ParatroopInfantry, Kind::Infantry, 2),
        (Unit::ItalianBersaglieriInfantry, Kind::Infantry, 2),
        (Unit::Machinegun, Kind::Infantry, 2),
        (Unit::AnyOtherInfantry, Kind::Infantry, 1),
        (Unit::EngineerBattalion, Kind::Infantry, 2),
        (Unit::EngineerCompany, Kind::Infantry, 1),
        (
            Unit::MotorcycleReconnaissanceCamelCavalryOrReconnaissance,
            Kind::Infantry,
            2,
        ),
        (Unit::Tank, Kind::Tank, 1),
        (Unit::Artillery, Kind::ArtilleryGun, 1),
        (Unit::AntiTank, Kind::AntiTankGun, 1),
        (Unit::AirdroppableAntiTank, Kind::LightAntiTank, 1),
        (Unit::AntiAir, Kind::AntiAirGun, 1),
    ] {
        assert_cost(chart.requirement(unit), &[(kind, count)]);
    }
    assert_cost(
        chart.requirement(Unit::HeavyWeapons),
        &[(Kind::Infantry, 1), (Kind::AnyGun, 1)],
    );
    assert_eq!(
        chart.requirement(Unit::RoadConstructionOrRailroadConstruction),
        &Requirement::NoPoints
    );
    assert_choices(
        chart.requirement(Unit::ArmoredReconnaissance),
        &[&[(Kind::ArmoredRecon, 1)], &[(Kind::UpgradeLightTank, 1)]],
    );
    assert_choices(
        chart.requirement(Unit::ArmoredCar),
        &[&[(Kind::ArmoredRecon, 2)], &[(Kind::UpgradeLightTank, 1)]],
    );
    assert_choices(
        chart.requirement(Unit::AirdroppableArtillery),
        &[
            &[(Kind::ItalianParaArtillery, 1)],
            &[(Kind::German75CmLightGun, 1)],
        ],
    );

    let notes = [
        (Unit::Commando, Note::LayforceOrSas),
        (Unit::ParatroopInfantry, Note::FolgoreOrRamcke),
        (Unit::Machinegun, Note::MachinegunMotorizationDoesNotMatter),
        (Unit::HeavyWeapons, Note::HeavyWeaponsExcludesRamcke),
        (Unit::EngineerBattalion, Note::IncludesAustralianPioneer),
        (
            Unit::RoadConstructionOrRailroadConstruction,
            Note::CommonwealthConstructionReturn,
        ),
        (
            Unit::AirdroppableAntiTank,
            Note::FolgoreOrRamckeHeadquartersAntiTank,
        ),
    ];
    for unit in Unit::ALL {
        let expected: Vec<_> = notes
            .iter()
            .filter(|(u, _)| *u == unit)
            .map(|(_, n)| *n)
            .collect();
        assert_eq!(chart.notes(unit), expected);
    }
    assert_eq!(
        Note::CommonwealthConstructionReturn.return_delay_op_stages(),
        Some(6)
    );
    assert_eq!(Note::LayforceOrSas.return_delay_op_stages(), None);
}

/// Cases: land:20.3
#[test]
fn malformed_conversion_rows_and_payment_forms_are_rejected() {
    for (from, to) in [
        ("unit = \"commando\"", "unit = \"any_headquarters_unit\""),
        ("unit = \"tank\"", "unit = \"sgsu\""),
        ("requires = { inf = 2 }", "requires = { inf = 0 }"),
        ("requires = { inf = 2 }", "requires = { inf = -1 }"),
        ("requires = { inf = 2 }", "requires = { inf = 2147483648 }"),
        ("requires = { inf = 2 }", "requires = { invented = 2 }"),
        ("requires = { none = true }", "requires = { none = false }"),
        (
            "requires = { none = true }",
            "requires = { none = true, inf = 1 }",
        ),
        (
            "requires = { and = { inf = 1, gun = 1 } }",
            "requires = { and = {} }",
        ),
        (
            "requires = { and = { inf = 1, gun = 1 } }",
            "requires = { inf = 1, gun = 1 }",
        ),
        (
            "alternatives = [{ armr = 1 }, { lt_tank = 1 }]",
            "alternatives = []",
        ),
        (
            "alternatives = [{ armr = 1 }, { lt_tank = 1 }]",
            "alternatives = [{ armr = 1 }, { armr = 1 }]",
        ),
        (
            "alternatives = [{ armr = 1 }, { lt_tank = 1 }]",
            "alternatives = [{ armr = 1 }, { lt_tank = 0 }]",
        ),
        ("footnotes = [\"a\"]", "footnotes = [\"b\"]"),
        ("footnotes = [\"a\"]", "footnotes = []"),
        ("footnotes = [\"a\"]", "footnotes = [\"a\", \"a\"]"),
        ("id = \"g\"", "id = \"a\""),
        ("id = \"g\"", "id = \"unknown\""),
    ] {
        let error =
            bind_edited::<ReplacementConversion>("land/20.3-", |text| replace_once(text, from, to))
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("20.3-replacement-point-conversion.toml")
        );
    }
    let missing = bind_edited::<ReplacementConversion>("land/20.3-", |text| {
        replace_once(
            text,
            "[[row]]\nunit = \"engineer_company\"\nrequires = { inf = 1 }\n",
            "",
        )
    })
    .unwrap_err();
    assert!(missing.to_string().contains("row"));
}
