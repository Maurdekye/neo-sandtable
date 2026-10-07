//! Independently checked cells from the local construction, demolition and raider charts.
mod common;

use cna_tables::land::engineering::{
    ConstructionChart, ConstructionTerrain as T, CrewRequirement as C, DemolitionChart,
    PortCategory, WorkFootnote, WorkItem as I, WorkSituation as S,
};
use cna_tables::land::raids::{
    DesertRaiderRaids, GuardCondition, RaidEffect as E, RaidTarget as R,
};
use common::{bind_edited, replace_once, tables};

/// Cases: land:24.17
#[test]
fn construction_cells_match_chart_image_costs_and_durations() {
    let chart = &tables().land.construction;
    let checks = [
        (
            I::FortificationLevel,
            S::BuildOrRebuild,
            30,
            0,
            0,
            0,
            Some(3),
            None,
        ),
        (I::RealMinefield, S::Build, 15, 0, 0, 15, Some(1), None),
        (I::FakeMinefield, S::Build, 3, 0, 0, 0, Some(1), None),
        (I::Railroad, S::Build, 1, 0, 0, 0, Some(1), None),
        (I::Railroad, S::Rebuild, 0, 1, 0, 0, Some(1), None),
        (I::Road, S::BuildOrRebuild, 0, 2, 0, 0, Some(1), None),
        (
            I::TemporaryRepairFacility,
            S::Build,
            250,
            0,
            50,
            0,
            Some(1),
            None,
        ),
        (
            I::RepairFacility,
            S::Rebuild1Level,
            50,
            0,
            10,
            0,
            Some(1),
            None,
        ),
        (
            I::WaterPipeline,
            S::BuildOrRebuild,
            10,
            0,
            0,
            0,
            Some(1),
            None,
        ),
        (I::Airfield, S::Build, 100, 0, 50, 0, Some(3), None),
        (
            I::AirfieldOrAirLandingStrip,
            S::Rebuild1LevelAirfieldOrBuildStrip,
            20,
            0,
            10,
            0,
            Some(1),
            None,
        ),
        (I::FlyingBoatBasin, S::Build, 50, 0, 25, 0, Some(3), None),
        (
            I::FlyingBoatBasinOrAlightingArea,
            S::Rebuild1LevelBasinOrBuildAlightingArea,
            10,
            0,
            10,
            0,
            Some(1),
            None,
        ),
        (I::RealSupplyDump, S::Build, 10, 0, 0, 0, None, Some(3)),
        (I::FakeSupplyDump, S::Build, 0, 0, 0, 0, None, Some(2)),
    ];
    assert_eq!(chart.rows().count(), checks.len() + 1);
    for (item, situation, stores, stores_per_hex, fuel, ammo, stages, cp) in checks {
        let row = chart.row(item, situation).unwrap();
        let cost = row.supplies.as_ref().unwrap();
        assert_eq!(
            (cost.stores, cost.stores_per_hex, cost.fuel, cost.ammo),
            (stores, stores_per_hex, fuel, ammo),
            "{item:?}/{situation:?}"
        );
        assert_eq!((row.op_stages, row.cp_cost), (stages, cp));
    }
    let port = chart.row(I::Port, S::Block1Level).unwrap();
    assert_eq!(port.op_stages, Some(1));
    let tobruk = &port.supplies_by_port[&PortCategory::Tobruk];
    let other = &port.supplies_by_port[&PortCategory::Other];
    assert_eq!((tobruk.ammo, tobruk.stores), (50, 25));
    assert_eq!((other.ammo, other.stores), (25, 10));
    assert!(chart.row(I::Airfield, S::Destroy).is_none());
}

/// Cases: land:24.17
#[test]
fn crew_combinations_terrain_lists_and_footnotes_keep_the_chart_distinctions() {
    let chart = &tables().land.construction;
    let fort = chart.row(I::FortificationLevel, S::BuildOrRebuild).unwrap();
    assert_eq!(
        fort.units,
        [vec![C::AnyEngineer, C::InfantryBattalionThreeToe]]
    );
    assert_eq!(
        fort.terrain_forbidden,
        [T::SaltMarsh, T::Delta, T::MajorCity]
    );
    assert!(fort.footnotes.contains(&WorkFootnote::OneAtATime));
    assert!(fort.footnotes.contains(&WorkFootnote::HotWeatherWater));
    let mine = chart.row(I::RealMinefield, S::Build).unwrap();
    assert_eq!(
        mine.units,
        [
            vec![C::EngineerBattalion],
            vec![C::EngineerCompany],
            vec![C::CommonwealthEngineeringHq]
        ]
    );
    assert_eq!(
        mine.terrain_allowed,
        [T::Clear, T::SandGravel, T::SaltMarsh]
    );
    let road = chart.row(I::Road, S::BuildOrRebuild).unwrap();
    assert!(road.units.is_empty());
    assert_eq!(
        road.units_a,
        [
            vec![C::InfantryBattalionThreeToe],
            vec![C::EngineerCompany],
            vec![C::EngineeringHq]
        ]
    );
    assert_eq!(
        road.units_b,
        [
            vec![C::EngineerBattalion],
            vec![C::EngineerCompany, C::InfantryBattalionThreeToe],
            vec![C::EngineeringHq, C::InfantryBattalionThreeToe]
        ]
    );
    assert!(
        chart
            .row(I::FakeSupplyDump, S::Build)
            .unwrap()
            .footnotes
            .is_empty()
    );
    let airfield = chart.row(I::Airfield, S::Build).unwrap();
    assert_eq!(
        airfield.units,
        [vec![C::EngineerBattalion], vec![C::CommonwealthSgsu]]
    );
    assert_eq!(
        airfield.terrain_allowed,
        [T::Clear, T::MajorCity, T::Desert, T::SandGravel]
    );
}

/// Cases: land:24.18
#[test]
fn demolition_cells_match_chart_recovery_unblocking_and_timing() {
    let chart = &tables().land.demolition;
    assert_eq!(chart.rows().count(), 13);
    let repair = chart.row(I::RepairFacility, S::Dismantle).unwrap();
    assert_eq!(repair.op_stages, Some(1));
    assert_eq!(repair.units, [C::AnyEngineer]);
    let recovery = repair.recovered_supplies.as_ref().unwrap();
    assert_eq!((recovery.fuel, recovery.stores), (25, 120));
    for (item, ammo, stores) in [
        (I::PortOfTobruk, 25, 10),
        (I::PortOfBenghazi, 100, 50),
        (I::PortOther, 50, 25),
    ] {
        let row = chart.row(item, S::Unblock1Level).unwrap();
        assert_eq!(row.op_stages, Some(1));
        let supplies = row.supplies.as_ref().unwrap();
        assert_eq!((supplies.ammo, supplies.stores), (ammo, stores));
    }
    assert_eq!(
        chart
            .row(I::PortOfBenghazi, S::Unblock1Level)
            .unwrap()
            .units,
        [C::TwoEngineerBattalionsOrCommonwealthEngineeringHqs]
    );
    assert_eq!(
        chart.row(I::RealMinefield, S::Clear).unwrap().units,
        [C::AnyEngineer, C::ScorpionBattalion]
    );
    assert_eq!(
        chart.row(I::Fortification, S::Reduce1Level).unwrap().units,
        [C::NotAllowed]
    );
    assert_eq!(
        chart.row(I::AirFacility, S::Reduce1Level).unwrap().units,
        [C::NotAllowed]
    );
    assert!(
        chart
            .row(I::FakeSupplyDump, S::Destroy)
            .unwrap()
            .op_stages_note
            .is_some()
    );
    assert!(
        chart
            .row(I::RealSupplyDump, S::Blow)
            .unwrap()
            .cp_note
            .is_some()
    );
    assert!(chart.row(I::Railroad, S::Block1Level).is_none());
}

/// Cases: land:24.17, land:24.18
#[test]
fn corrupt_work_costs_durations_crew_shapes_and_missing_records_are_rejected() {
    let bad = bind_edited::<ConstructionChart>("land/24.17-", |t| {
        replace_once(t, "stores = 30", "stores = -30")
    })
    .unwrap_err()
    .to_string();
    assert!(bad.contains("row[0].supplies.stores"), "{bad}");
    assert!(
        bind_edited::<ConstructionChart>("land/24.17-", |t| replace_once(
            t,
            "op_stages = 3",
            "op_stages = 0"
        ))
        .is_err()
    );
    assert!(
        bind_edited::<ConstructionChart>("land/24.17-", |t| replace_once(
            t,
            "op_stages = 3",
            "op_stages = 3\ncp_cost = 1"
        ))
        .is_err()
    );
    assert!(
        bind_edited::<ConstructionChart>("land/24.17-", |t| replace_once(
            t,
            "units = [[\"any_e\", \"inf_bn_3\"]]",
            "units = [[]]"
        ))
        .is_err()
    );
    assert!(
        bind_edited::<ConstructionChart>("land/24.17-", |t| replace_once(
            t,
            "item = \"fortification_level\"",
            "item = \"fake_minefield\""
        ))
        .is_err()
    );
    assert!(
        bind_edited::<ConstructionChart>("land/24.17-", |t| replace_once(
            t,
            "star_hot_weather_water\"]",
            "star_hot_weather_water\", \"star_hot_weather_water\"]"
        ))
        .is_err()
    );
    assert!(
        bind_edited::<DemolitionChart>("land/24.18-", |t| replace_once(
            t,
            "fuel = 25",
            "fuel = -25"
        ))
        .is_err()
    );
    assert!(
        bind_edited::<DemolitionChart>("land/24.18-", |t| replace_once(
            t,
            "units = [\"not_allowed\"]",
            "units = [\"not_allowed\", \"any_unit\"]"
        ))
        .is_err()
    );
    assert!(
        bind_edited::<DemolitionChart>("land/24.18-", |t| replace_once(
            t,
            "op_stages = 1",
            "op_stages = 0"
        ))
        .is_err()
    );
}

/// Cases: land:27.91
#[test]
fn raider_chart_cells_match_each_die_face_and_guard_predicates() {
    let chart = &tables().land.desert_raider_raids;
    let cells = [
        (
            R::WaterPipeline,
            [
                E::TargetDestroyed,
                E::TargetDestroyed,
                E::TargetDestroyed,
                E::TargetDestroyed,
                E::NoEffect,
                E::NoEffect,
            ],
        ),
        (
            R::Airfield,
            [
                E::ReduceOneLevelOfEffectiveness,
                E::ReduceOneLevelOfEffectiveness,
                E::NoEffect,
                E::NoEffect,
                E::NoEffect,
                E::NoEffect,
            ],
        ),
        (
            R::AirplanesOnTheGround,
            [
                E::TenPercentOfPlanesDestroyedRaiderChooses,
                E::TenPercentOfPlanesDestroyedRaiderChooses,
                E::NoEffect,
                E::NoEffect,
                E::NoEffect,
                E::PlanesUnaffectedRaiderEliminated,
            ],
        ),
        (
            R::SupplyDumpWithoutCombatUnitsOrRaiderSurvived,
            [
                E::TenPercentOfSuppliesDestroyed,
                E::TenPercentOfSuppliesDestroyed,
                E::NoEffect,
                E::NoEffect,
                E::NoEffect,
                E::SuppliesUnaffectedRaiderEliminatedIfCombatUnitsPresent,
            ],
        ),
        (
            R::TrucksInConvoy,
            [
                E::OneTruckPointEliminated,
                E::OneTruckPointEliminated,
                E::NoEffect,
                E::NoEffect,
                E::NoEffect,
                E::TrucksUnaffectedRerollIfInfantryReplacementPointsCarried,
            ],
        ),
    ];
    for (target, expected) in cells {
        for (die, effect) in (1..=6).zip(expected) {
            assert_eq!(chart.result(target, die), Some(effect));
        }
        assert_eq!(chart.result(target, 0), None);
        assert_eq!(chart.result(target, 7), None);
    }
    assert_eq!(chart.result(R::Rommel, 1), None);
    assert_eq!(chart.result(R::SupplyDumpWithCombatUnitsInHex, 6), None);
    assert_eq!(
        chart.guard_result(GuardCondition::SumAtLeastGuardsDefense),
        E::RaiderSurvivesAndAttacksAsBelow
    );
    assert_eq!(
        chart.guard_result(GuardCondition::SumBelowGuardsDefense),
        E::RaiderEliminated
    );
    assert!(!chart.notes(R::Rommel).is_empty());
}

/// Cases: land:27.91
#[test]
fn raider_ranges_reject_gaps_overlaps_mixed_dice_and_duplicate_targets() {
    assert!(
        bind_edited::<DesertRaiderRaids>("land/27.91-", |t| replace_once(
            t,
            "die = [1, 4]",
            "die = [1, 3]"
        ))
        .is_err()
    );
    assert!(
        bind_edited::<DesertRaiderRaids>("land/27.91-", |t| replace_once(
            t,
            "die = [5, 6]",
            "die = [4, 6]"
        ))
        .is_err()
    );
    assert!(
        bind_edited::<DesertRaiderRaids>("land/27.91-", |t| replace_once(
            t,
            "id = \"water_pipeline\"",
            "id = \"water_pipeline\"\ntwo_dice_sum = true"
        ))
        .is_err()
    );
    assert!(
        bind_edited::<DesertRaiderRaids>("land/27.91-", |t| replace_once(
            t,
            "id = \"airfield\"",
            "id = \"water_pipeline\""
        ))
        .is_err()
    );
    assert!(
        bind_edited::<DesertRaiderRaids>("land/27.91-", |t| replace_once(
            t,
            "two_dice_sum = true",
            "two_dice_sum = false"
        ))
        .is_err()
    );
}
