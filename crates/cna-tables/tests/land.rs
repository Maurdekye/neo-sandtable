//! Hand checks against the local chart images; no source artwork is part of the repository.

mod common;

use cna_tables::land::administration::{
    InitiativeRatings, OffMapMovement, OffMapPlace, OrganizationLevel, ShellOf, StackingValues,
};
use cna_tables::land::capability::{CapabilityExpenditure, CombatActivity, CpAction, CpCost};
use cna_tables::land::terrain::{
    CombatShift, StackingLimit, TerrainEffects, TerrainFeature, TerrainValue,
};
use cna_tables::units::Ratio;
use common::{bind_edited, replace_once, tables};

/// Cases: land:6.3
#[test]
fn capability_prices_match_hand_checked_chart_cells() {
    let t = &tables().land.capability_expenditure;
    assert_eq!(t.cost(CpAction::DetachUnit), Some(CpCost::Flat(1)));
    assert_eq!(
        t.cost(CpAction::AttachUnassignedUnit),
        Some(CpCost::Flat(2))
    );
    assert_eq!(t.cost(CpAction::BePlacedInReserve), Some(CpCost::Flat(0)));
    assert_eq!(t.cost(CpAction::ReadyAirplanes), Some(CpCost::Flat(10)));
    assert_eq!(
        t.cost(CpAction::AttemptToBlowSupplyDump),
        Some(CpCost::CpaFraction(Ratio { num: 1, den: 3 }))
    );
    assert_eq!(t.combat_cost(true, CombatActivity::BarrageOrAssault), 5);
    assert_eq!(t.combat_cost(true, CombatActivity::UndergoBarrage), 3);
    assert_eq!(t.combat_cost(true, CombatActivity::Probe), 2);
    assert_eq!(t.combat_cost(false, CombatActivity::BarrageOrAssault), 3);
    assert_eq!(t.combat_cost(false, CombatActivity::UndergoBarrage), 3);
    assert_eq!(t.combat_cost(false, CombatActivity::Probe), 2);
    assert_eq!(t.defense_refund(-4, false, false), 2);
    assert_eq!(t.defense_refund(-5, true, false), 0);
    assert_eq!(t.defense_refund(-5, false, true), 0);
    assert_eq!(t.defense_refund(-3, false, false), 0);
}

/// Cases: land:6.3, land:8.37
#[test]
fn section_six_worked_examples_use_the_chart_prices() {
    // The 6.14 example spends six of an eight-CP allowance retreating through three clear hexes.
    let terrain = &tables().land.terrain_effects;
    let Some(TerrainValue::EnterQuarters(clear)) =
        terrain.feature(TerrainFeature::Clear).cp_non_motorized
    else {
        panic!("clear entry price")
    };
    assert_eq!(8 * 4 - 3 * clear, 2 * 4);
    // In 6.22, five CP already spent plus an assault at five leaves a two-CP overrun.
    let assault = tables()
        .land
        .capability_expenditure
        .combat_cost(true, CombatActivity::BarrageOrAssault);
    assert_eq!(5 + assault - 8, 2);
}

/// Cases: land:6.3, land:26.21, land:26.23
/// Interpretations: interp:land-0015
#[test]
fn engineer_minefield_prices_follow_the_summary() {
    let t = &tables().land.capability_expenditure;
    assert_eq!(
        t.minefield_cost(true, true, true),
        CpCost::FlatPlusTerrain(0)
    );
    assert_eq!(
        t.minefield_cost(true, false, false),
        CpCost::FlatPlusTerrain(1)
    );
    assert_eq!(
        t.minefield_cost(true, true, false),
        CpCost::FlatPlusTerrain(4)
    );
    assert_eq!(
        t.minefield_cost(false, false, true),
        CpCost::FlatPlusTerrain(2)
    );
    assert_eq!(
        t.minefield_cost(false, true, true),
        CpCost::FlatPlusTerrain(4)
    );
    assert_eq!(
        t.minefield_cost(false, false, false),
        CpCost::FlatPlusTerrain(4)
    );
    assert_eq!(
        t.minefield_cost(false, true, false),
        CpCost::WholeCpaPlusTerrain
    );
}

/// Cases: land:6.3
/// Interpretations: interp:land-0006, interp:land-0018
#[test]
fn compressed_cp_entries_have_named_selections() {
    let t = &tables().land.capability_expenditure;
    assert_eq!(t.supply_dump_cost(true), 3);
    assert_eq!(t.supply_dump_cost(false), 2);
    assert_eq!(
        t.commando_landing_cost(50),
        Some(CpCost::FlatPlusTerrain(5))
    );
    assert_eq!(
        t.commando_landing_cost(51),
        Some(CpCost::FlatPlusTerrain(10))
    );
    // Interpretation example: a ship moving sixty hexes gives ten CP, then terrain entry.
    assert_eq!(
        t.commando_landing_cost(60),
        Some(CpCost::FlatPlusTerrain(10))
    );
    assert_eq!(
        t.commando_landing_cost(100),
        Some(CpCost::FlatPlusTerrain(10))
    );
    assert_eq!(t.commando_landing_cost(101), None);
    assert_eq!(t.commando_landing_cost(-1), None);
}

/// Cases: land:7.2, land:7.14
#[test]
fn initiative_matches_all_chart_boundaries_and_axis_situations() {
    let t = &tables().land.initiative_ratings;
    for (turn, rating) in [(1, 3), (42, 3), (43, 4), (90, 4), (91, 5), (111, 5)] {
        assert_eq!(t.commonwealth_rating(turn), Some(rating));
    }
    assert_eq!(t.commonwealth_rating(0), None);
    assert_eq!(t.commonwealth_rating(112), None);
    assert_eq!(t.axis_rating(true, true), 6);
    assert_eq!(t.axis_rating(true, false), 6);
    assert_eq!(t.axis_rating(false, true), 3);
    assert_eq!(t.axis_rating(false, false), 1);
}

/// Cases: land:8.37
/// Interpretations: interp:land-0002
#[test]
fn terrain_cells_distinguish_fractions_additions_and_restrictions() {
    let t = &tables().land.terrain_effects;
    let city = t.feature(TerrainFeature::MajorCity);
    assert_eq!(city.cp_motorized, Some(TerrainValue::EnterQuarters(2)));
    assert_eq!(city.breakdown, Some(TerrainValue::ValueQuarters(2)));
    assert_eq!(city.stacking_limit, StackingLimit::Points(8));
    assert_eq!(city.barrage_shift, CombatShift::UseFortifications);
    let mountain = t.feature(TerrainFeature::Mountain);
    assert_eq!(mountain.cp_motorized, Some(TerrainValue::EnterQuarters(24)));
    assert_eq!(mountain.breakdown, Some(TerrainValue::ValueQuarters(48)));
    assert_eq!(mountain.close_assault_shift, CombatShift::Columns(-3));
    assert_eq!(mountain.stacking_limit, StackingLimit::Points(3));
    let ridge = t.feature(TerrainFeature::Ridge);
    assert_eq!(ridge.cp_motorized, Some(TerrainValue::AddQuarters(16)));
    assert_eq!(ridge.anti_armor_shift, CombatShift::Columns(-2));
    assert_eq!(
        t.feature(TerrainFeature::UpEscarpment).cp_motorized,
        Some(TerrainValue::Prohibited)
    );
    assert_eq!(
        t.feature(TerrainFeature::UpEscarpment).anti_armor_shift,
        CombatShift::Prohibited
    );
    assert_eq!(
        t.feature(TerrainFeature::EnemyMinefield).cp_motorized,
        Some(TerrainValue::AddWholeCpa)
    );
    assert_eq!(t.feature(TerrainFeature::Swamp).breakdown, None);
    assert_eq!(
        t.feature(TerrainFeature::Swamp).cp_motorized,
        Some(TerrainValue::RoadOrRailOnly)
    );
    assert_eq!(
        t.feature(TerrainFeature::Road).cp_motorized,
        Some(TerrainValue::EnterQuarters(2))
    );
    assert_eq!(
        t.feature(TerrainFeature::Road).stacking_limit,
        StackingLimit::Points(5)
    );
}

/// Cases: land:8.37, land:8.46
/// Interpretations: interp:land-0002
#[test]
fn tracks_and_city_fortifications_use_corrected_footnotes() {
    let t = &tables().land.terrain_effects;
    assert_eq!(
        t.track_values(TerrainFeature::Clear, false),
        (
            Some(TerrainValue::EnterQuarters(4)),
            Some(TerrainValue::ValueQuarters(8))
        )
    );
    assert_eq!(
        t.track_values(TerrainFeature::Rough, true),
        (
            Some(TerrainValue::EnterQuarters(8)),
            Some(TerrainValue::ValueQuarters(16))
        )
    );
    assert_eq!(
        t.track_values(TerrainFeature::Rough, false).0,
        Some(TerrainValue::EnterQuarters(6))
    );
    assert_eq!(
        t.track_values(TerrainFeature::DownEscarpment, true),
        (
            Some(TerrainValue::AddQuarters(32)),
            Some(TerrainValue::AddQuarters(24))
        )
    );
    assert_eq!(
        t.track_values(TerrainFeature::DownEscarpment, false).0,
        Some(TerrainValue::AddQuarters(8))
    );
    assert_eq!(
        t.city_fortification(true).close_assault_shift,
        CombatShift::Columns(-4)
    );
    assert_eq!(
        t.city_fortification(false).close_assault_shift,
        CombatShift::Columns(-3)
    );
    assert_eq!(
        t.feature(TerrainFeature::RockGravel).breakdown,
        Some(TerrainValue::ValueQuarters(24))
    );
}

/// Cases: land:8.89, land:8.83
#[test]
fn off_map_triangle_is_symmetric_and_preserves_all_three_columns() {
    use OffMapPlace::{Gabes, Nofilia, Tripoli, Tunis};
    let t = &tables().land.off_map_movement;
    assert_eq!(t.stages(Tunis, Nofilia, 25), Some(4));
    assert_eq!(t.stages(Tunis, Nofilia, 20), Some(5));
    assert_eq!(t.stages(Tunis, Nofilia, 14), Some(14));
    assert_eq!(t.stages(Nofilia, Tunis, 15), Some(5));
    assert_eq!(t.stages(Gabes, Tripoli, 15), Some(1));
    assert_eq!(t.stages(Tunis, Tunis, 25), Some(0));
    assert_eq!(t.stages(Tunis, Nofilia, 22), None);
    assert_eq!(t.stages(Tunis, Nofilia, 0), None);
}

/// Cases: land:9.4, land:9.26, land:9.28, land:9.29, land:9.33
#[test]
fn stacking_preserves_shell_mapping_and_partial_five_point_blocks() {
    let t = &tables().land.stacking_values;
    assert_eq!(t.full_unit_halves(OrganizationLevel::Division), Some(10));
    assert_eq!(t.full_unit_halves(OrganizationLevel::SuperBrigade), Some(6));
    assert_eq!(
        t.full_unit_halves(OrganizationLevel::StandardBrigadeOrBattleGroup),
        Some(4)
    );
    assert_eq!(t.shell_halves(ShellOf::Division), 6);
    assert_eq!(t.shell_halves(ShellOf::AnyBrigade), 2);
    assert_eq!(t.shell_halves(ShellOf::BattleGroup), 2);
    assert_eq!(t.shell_halves(ShellOf::Battalion), 0);
    assert_eq!(t.shell_halves(ShellOf::Hq), 0);
    assert_eq!(t.shell_halves(ShellOf::AnyAttachedUnits), 0);
    assert_eq!(
        t.blocks_halves(OrganizationLevel::TruckPointsInConvoy, 5),
        Some(1)
    );
    assert_eq!(
        t.blocks_halves(OrganizationLevel::TruckPointsInConvoy, 6),
        Some(2)
    );
    assert_eq!(
        t.blocks_halves(OrganizationLevel::ReplacementPoints, 6),
        Some(4)
    );
    assert_eq!(
        t.blocks_halves(OrganizationLevel::ReplacementPoints, 0),
        Some(0)
    );
    assert_eq!(
        t.blocks_halves(OrganizationLevel::ReplacementPoints, -1),
        None
    );
    assert_eq!(t.blocks_halves(OrganizationLevel::Battalion, 5), None);
}

#[test]
fn malformed_land_tables_name_the_file_and_field() {
    let err = bind_edited::<CapabilityExpenditure>("land/6.3-", |s| {
        replace_once(s, "cost = { cp = 1 }", "cost = { cp = 1, tec = true }")
    })
    .unwrap_err();
    assert!(
        err.file
            .ends_with("6.3-capability-point-expenditure-summary.toml"),
        "{err}"
    );
    assert!(err.field.contains("cost"), "{err}");
    let err = bind_edited::<InitiativeRatings>("land/7.2-", |s| {
        replace_once(
            s,
            "game_turn_range = [43, 90]",
            "game_turn_range = [44, 90]",
        )
    })
    .unwrap_err();
    assert!(err.field.contains("game_turn_range"), "{err}");
    let err = bind_edited::<TerrainEffects>("land/8.37-", |s| {
        replace_once(
            s,
            "cp_non_mot = { enter_x2 = 4 }",
            "cp_non_mot = { enter_x2 = 4, prohibited = true }",
        )
    })
    .unwrap_err();
    assert!(err.field.contains("cp_non_mot"), "{err}");
    let err = bind_edited::<TerrainEffects>("land/8.37-", |s| {
        replace_once(s, "footnotes = [13]", "footnotes = [14]")
    })
    .unwrap_err();
    assert!(err.field.contains("footnotes"), "{err}");
    let err = bind_edited::<OffMapMovement>("land/8.89-", |s| {
        replace_once(
            s,
            "from = \"tunis\"\nto = \"tripoli\"",
            "from = \"tunis\"\nto = \"gabes\"",
        )
    })
    .unwrap_err();
    assert!(err.field.contains("from"), "{err}");
    let err = bind_edited::<StackingValues>("land/9.4-", |s| {
        replace_once(s, "per_block_of_points = 5", "per_block_of_points = 0")
    })
    .unwrap_err();
    assert!(err.field.contains("row"), "{err}");
}

/// Cases: land:29.6, land:29.61, land:29.1
/// Interpretations: interp:land-0019
#[test]
fn weather_preserves_seasonal_chart_cells_and_boundaries() {
    use cna_tables::land::weather::{Season, WeatherKind};
    let read = |tens, units| cna_core::dice::TwoDiceReading {
        tens: cna_core::dice::Die::new(tens).unwrap(),
        units: cna_core::dice::Die::new(units).unwrap(),
    };
    let t = &tables().land.weather;
    for (turn, season) in [
        (1, Season::Fall),
        (12, Season::Fall),
        (13, Season::Winter),
        (24, Season::Winter),
        (25, Season::Spring),
        (36, Season::Spring),
        (37, Season::Summer),
        (48, Season::Summer),
        (49, Season::Fall),
        (110, Season::Winter),
    ] {
        assert_eq!(t.season(turn), Some(season));
    }
    assert_eq!(t.result(1, read(4, 2)), Some(WeatherKind::Normal));
    assert_eq!(t.result(1, read(4, 3)), Some(WeatherKind::Hot));
    assert_eq!(t.result(1, read(5, 6)), Some(WeatherKind::Sandstorm));
    assert_eq!(t.result(1, read(6, 5)), Some(WeatherKind::Rainstorm));
    assert_eq!(t.result(37, read(6, 1)), Some(WeatherKind::Rainstorm));
    assert_eq!(t.result(13, read(3, 1)), Some(WeatherKind::Hot));
    assert_eq!(t.season(0), None);
    assert_eq!(t.season(111), None);
    assert_eq!(t.result(111, read(1, 1)), None);
    // Check every supported turn and every actual sequential reading, without inventing 17-20, etc.
    for turn in 1..=110 {
        for tens in 1..=6 {
            for units in 1..=6 {
                assert!(t.result(turn, read(tens, units)).is_some());
            }
        }
    }
}

/// Cases: land:29.7, land:29.1
#[test]
fn storm_sections_match_every_hand_checked_chart_cell() {
    use cna_tables::land::weather::MapSection::{A, B, C, D, E};
    let t = &tables().land.foul_weather_location;
    for (face, expected) in [
        (1, vec![A, B]),
        (2, vec![C, D]),
        (3, vec![D, E]),
        (4, vec![B, C]),
        (5, vec![B, D]),
        (6, vec![B, C, D]),
    ] {
        assert_eq!(
            t.sections(cna_core::dice::Die::new(face).unwrap()),
            expected
        );
    }
}

#[test]
fn weather_tables_reject_new_roll_gaps_and_duplicate_sections() {
    use cna_tables::land::weather::{FoulWeatherLocation, WeatherTable};
    let err = bind_edited::<WeatherTable>("land/29.6-", |s| {
        replace_once(s, "normal = [11, 42]", "normal = [11, 41]")
    })
    .unwrap_err();
    assert!(err.field.contains("row"), "{err}");
    assert!(err.message.contains("42"), "{err}");
    let err = bind_edited::<FoulWeatherLocation>("land/29.7-", |s| {
        replace_once(
            s,
            "map_sections = [\"A\", \"B\"]",
            "map_sections = [\"A\", \"A\"]",
        )
    })
    .unwrap_err();
    assert!(err.field.contains("map_sections"), "{err}");
}
