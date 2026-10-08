mod common;
use cna_tables::land::{
    attachment::{AttachmentAllowance, AttachmentNation, AttachmentParent, MaximumAttachment},
    convoy_bombing::{AxisConvoyBombing, BombingLocation, ConvoyRoute, ConvoyRowLocation},
};
use common::{bind_edited, replace_once, tables};

/// Cases: land:19.5
#[test]
fn native_attachment_rows_keep_all_alternatives_and_restrictions() {
    use AttachmentNation::*;
    use AttachmentParent::*;
    let chart = &tables().land.maximum_attachment;
    let check = |nation, parent, turn, specs: &[&str]| {
        let expected: Vec<AttachmentAllowance> = specs
            .iter()
            .map(|s| toml::from_str(s).expect("manual native allowance"))
            .collect();
        assert_eq!(
            chart.allowances(nation, parent, turn),
            Some(expected.as_slice())
        );
    };
    for turn in [1, 67] {
        for parent in [ArmorDivision, TankBrigade] {
            check(
                Allied,
                parent,
                turn,
                &["max_units=2\nmax_infantry=1\nmax_tank=0"],
            );
        }
    }
    for turn in [68, 111] {
        check(
            Allied,
            ArmorDivision,
            turn,
            &["max_units=3\nmax_infantry=1\nmax_tank=1\ninfantry_and_or_tank_one_each=true"],
        );
        check(Allied, TankBrigade, turn, &["max_units=1\nmax_tank=0"]);
    }
    check(
        Allied,
        InfantryDivision,
        40,
        &["max_units=2\nmax_infantry=1\nmax_tank=0"],
    );
    check(Allied, OtherBrigade, 40, &["max_units=1"]);
    check(Allied, AnyBattalion, 40, &["max_companies=1"]);
    for turn in [1, 12] {
        check(
            Allied,
            MatruhGarrison,
            turn,
            &["max_units=6\nmax_tank=0\nmax_gun_class=1"],
        );
    }
    for turn in [13, 111] {
        check(
            Allied,
            SelbyForce,
            turn,
            &["max_units=5\nmax_tank=0\nmax_recce=0\nmax_infantry=3"],
        );
    }
    check(
        German,
        ArmorDivision,
        40,
        &[
            "max_brigades=1\nmax_units_besides_brigade=1\nmax_tank=0",
            "max_units=4\nmax_tank=0",
        ],
    );
    check(
        German,
        InfantryDivision,
        40,
        &["max_brigades=1\nmax_tank=0", "max_units=3\nmax_tank=0"],
    );
    check(German, InfantryOrArmorRegiment, 40, &["max_units=1"]);
    check(German, BattleGroup, 40, &["max_units=4"]);
    check(German, ArtilleryBrigadeHq, 40, &["max_artillery_units=1"]);
    check(German, AnyBattalion, 40, &["max_companies=1"]);
    check(Italian, ArmorDivisionOrTankGroup, 40, &["max_units=2"]);
    check(
        Italian,
        InfantryDivision,
        40,
        &["max_units=2\none_infantry_or_one_tank=true"],
    );
    check(Italian, BrigadeOrRegiment, 40, &["max_units=1"]);
    check(Italian, BattleGroup, 40, &["max_units=3"]);
    check(Italian, AnyBattalion, 40, &["max_units=0"]);
    assert!(chart.allowances(Allied, MatruhGarrison, 13).is_none());
    assert!(chart.allowances(Allied, SelbyForce, 12).is_none());
    assert!(chart.allowances(German, TankBrigade, 40).is_none());
    for turn in [-1, 0, 112, i32::MAX] {
        assert!(chart.allowances(Italian, BattleGroup, turn).is_none());
    }
}

/// Cases: land:19.5
#[test]
fn native_company_bonus_preserves_equivalents_and_type_limits() {
    let bonus = tables()
        .land
        .maximum_attachment
        .division_or_brigade_company_bonus();
    assert_eq!(bonus.extra_company_equivalents, 2);
    assert_eq!(bonus.company_equivalents_per_battalion, 3);
    assert!(bonus.non_shell_only && bonus.type_restrictions_apply);
}

/// Cases: land:19.5
#[test]
fn malformed_attachment_rejects_missing_rows_options_and_contradictions() {
    for (from, to) in [
        ("nation = \"allied\"", "nation = \"neutral\""),
        ("parent = \"tank_brigade\"", "parent = \"armor_division\""),
        ("game_turn_range = [68, 111]", "game_turn_range = [67, 111]"),
        ("max_units = 2", "max_units = -1"),
        (
            "max_units = 2, max_infantry = 1",
            "max_units = 2, max_infantry = 3",
        ),
        (
            "max_units = 2, max_infantry = 1",
            "max_units = 2, max_companies = 1, max_infantry = 1",
        ),
        (
            "max_units = 2, max_infantry = 1",
            "max_units_besides_brigade = 2, max_infantry = 1",
        ),
        (
            "infantry_and_or_tank_one_each = true",
            "infantry_and_or_tank_one_each = true, one_infantry_or_one_tank = true",
        ),
        (
            "infantry_and_or_tank_one_each = true",
            "infantry_and_or_tank_one_each = false",
        ),
        (
            "one_infantry_or_one_tank = true",
            "one_infantry_or_one_tank = false",
        ),
        (", { max_units = 4, max_tank = 0 }", ""),
        ("id = \"company_bonus\"", "id = \"definitions\""),
    ] {
        let err = bind_edited::<MaximumAttachment>("land/19.5-", |s| replace_once(s, from, to))
            .unwrap_err();
        assert!(err.to_string().contains("19.5-maximum-attachment.toml"));
        assert!(!err.field.is_empty());
    }
    assert!(bind_edited::<MaximumAttachment>("land/19.5-",|s|replace_once(s,
        "[[row]]\nnation = \"italian\"\nparent = \"any_battalion\"\noptions = [{ max_units = 0 }]", "")).is_err());
}

/// Cases: land:32.66
#[test]
fn native_convoy_matrix_preserves_forty_nine_locations_and_eleven_dashes() {
    let expected = [
        ["Cxx01", "Dxx01", "Dxx01", "Exx01", "Exx11", "Dxx11"],
        ["Bxx16", "Cxx16", "Cxx21", "Dxx21", "Exx01", "Dxx01"],
        ["Bxx01", "Cxx01", "Cxx11", "Dxx11", "Dxx21", "Cxx21"],
        ["Axx18", "Bxx16", "Cxx01", "Dxx01", "Dxx11", "Cxx11"],
        ["-", "Bxx01", "Bxx26", "Cxx21", "Dxx01", "Cxx01"],
        ["-", "Axx18", "Bxx20", "Cxx11", "Cxx21", "Bxx26"],
        ["-", "-", "Bxx10", "Cxx01", "Cxx11", "Bxx20"],
        ["-", "-", "Bxx01", "Bxx26", "Cxx01", "Bxx10"],
        ["-", "-", "Axx18", "Bxx20", "Bxx26", "Bxx01"],
        ["-", "-", "-", "Bxx10", "Bxx20", "Axx18"],
    ];
    let bands = [
        (21, 40),
        (41, 80),
        (81, 120),
        (121, 160),
        (161, 200),
        (201, 260),
        (261, 320),
        (321, 390),
        (391, 470),
        (471, i32::MAX),
    ];
    let chart = &tables().land.axis_convoy_bombing;
    let mut locations = 0;
    let mut dashes = 0;
    for (column, route) in ConvoyRoute::ALL.into_iter().enumerate() {
        let cells: Vec<_> = chart.column(route).collect();
        assert_eq!(cells.len(), 10);
        for (row, (lo, hi)) in bands.into_iter().enumerate() {
            let code = expected[row][column];
            let cell = if code == "-" {
                dashes += 1;
                BombingLocation::Dash
            } else {
                locations += 1;
                BombingLocation::Row(ConvoyRowLocation {
                    map_section: code.chars().next().unwrap(),
                    east_west_row: code[3..].parse().unwrap(),
                })
            };
            assert_eq!(chart.location(route, lo), Some(cell));
            assert_eq!(chart.location(route, hi), Some(cell));
            assert_eq!(cells[row].1, cell);
            assert_eq!(cells[row].0.min, lo);
            assert_eq!(cells[row].0.max, (row != 9).then_some(hi));
        }
        for points in [-1, 0, 20] {
            assert!(chart.location(route, points).is_none());
        }
    }
    assert_eq!((locations, dashes), (49, 11));
}

/// Cases: land:32.66
#[test]
fn malformed_convoy_rejects_wrong_bands_routes_and_location_codes() {
    for (from, to) in [
        ("bomb_points = [21, 40]", "bomb_points = [20, 40]"),
        ("bomb_points = [41, 80]", "bomb_points = [21, 40]"),
        ("bomb_points_min = 471", "bomb_points_min = 472"),
        (
            "bomb_points_min = 471",
            "bomb_points_min = 471\nbomb_points = [471, 500]",
        ),
        ("r1 = \"Cxx01\", ", ""),
        ("r1 = \"Cxx01\"", "r7 = \"Cxx01\""),
        ("\"Cxx01\"", "\"C0101\""),
        ("\"Cxx01\"", "\"Fxx01\""),
        ("\"Cxx01\"", "\"Cxx00\""),
        ("\"Cxx01\"", "\"Cxx34\""),
        (
            "location = { r2 = \"Bxx01\"",
            "location = { r1 = \"Axx18\", r2 = \"Bxx01\"",
        ),
    ] {
        let err = bind_edited::<AxisConvoyBombing>("land/32.66-", |s| replace_once(s, from, to))
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("32.66-simplified-axis-naval-convoy-bombing.toml")
        );
        assert!(!err.field.is_empty());
    }
    assert!(
        bind_edited::<AxisConvoyBombing>("land/32.66-", |s| s[..s.rfind("[[row]]").unwrap()]
            .into())
        .is_err()
    );
}
