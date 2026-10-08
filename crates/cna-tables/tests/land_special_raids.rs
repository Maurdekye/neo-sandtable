mod common;

use cna_tables::land::raids::{
    ChariotRaid, RaidOnRommel, RommelRaidResult as Rommel, SasBrigadeRaid,
};
use common::{bind_edited, replace_once, tables};

/// Cases: land:27.63, land:27.92
#[test]
fn native_rommel_all_sum_cells_and_unprinted_totals() {
    // Independently read native 27.54 image, renumbered by case 27.63.
    let expected = [
        Rommel::TemporaryAxisInitiativeThree,
        Rommel::RaiderEliminated,
        Rommel::NoEffect,
        Rommel::NoEffect,
        Rommel::NoEffect,
        Rommel::NoEffect,
        Rommel::NoEffect,
        Rommel::RaiderEliminated,
        Rommel::NoEffect,
        Rommel::RaiderEliminated,
        Rommel::RemoveRommel,
    ];
    let chart = &tables().land.raid_on_rommel;
    for total in 2..=12 {
        assert_eq!(chart.result(total), Some(expected[(total - 2) as usize]));
    }
    // No invented thirteenth cell or automatic application of elimination exceptions.
    for invalid in [i32::MIN, 0, 1, 13, 66, i32::MAX] {
        assert_eq!(chart.result(invalid), None);
    }
}

/// Cases: land:27.86, land:27.93
#[test]
fn native_sas_percentages_preserve_thirty_three() {
    let chart = &tables().land.sas_brigade_raid;
    let expected = [0, 10, 25, 25, 33, 50];
    for die in 1..=6 {
        assert_eq!(
            chart.percentage_destroyed(die),
            Some(expected[(die - 1) as usize])
        );
    }
    for invalid in [i32::MIN, 0, 7, i32::MAX] {
        assert_eq!(chart.percentage_destroyed(invalid), None);
    }
}

/// Cases: land:30.44, land:30.46
#[test]
fn native_chariot_secondary_dice_including_no_effect() {
    let chart = &tables().land.chariot_raid;
    let expected = [3, 2, 2, 1, 1, 0];
    for die in 1..=6 {
        assert_eq!(chart.damage_dice(die), Some(expected[(die - 1) as usize]));
    }
    for invalid in [i32::MIN, 0, 7, i32::MAX] {
        assert_eq!(chart.damage_dice(invalid), None);
    }
}

/// Cases: land:27.92
#[test]
fn malformed_rommel_rejects_holes_overlap_bounds_and_unknown_outcomes() {
    for (from, to) in [
        ("dice_max = 2", "dice_max = 3"),
        ("dice_min = 4", "dice_min = 5"),
        ("dice_min = 2", "dice_min = 1"),
        ("dice_max = 12", "dice_max = 13"),
        ("dice_max = 2", "dice_max = 2147483647"),
        ("dice_min = 4", "dice_min = 9"),
        ("result = \"ai_perm_3\"", "result = \"unknown\""),
    ] {
        let err = bind_edited::<RaidOnRommel>("land/27.92-", |text| replace_once(text, from, to))
            .unwrap_err();
        assert!(err.to_string().contains("27.92-raid-on-rommel.toml"));
        assert!(!err.field.is_empty());
    }
}

/// Cases: land:27.93
#[test]
fn malformed_sas_rejects_missing_duplicate_die_and_invalid_percentages() {
    for (from, to) in [
        ("die = 1", "die = 2"),
        ("die = 1", "die = 0"),
        (
            "percent_planes_destroyed = 0",
            "percent_planes_destroyed = -1",
        ),
        (
            "percent_planes_destroyed = 50",
            "percent_planes_destroyed = 101",
        ),
        (
            "percent_planes_destroyed = 0",
            "percent_planes_destroyed = 0\nextra_column = 1",
        ),
        ("[[row]]\ndie = 1\npercent_planes_destroyed = 0\n", ""),
    ] {
        let err = bind_edited::<SasBrigadeRaid>("land/27.93-", |text| replace_once(text, from, to))
            .unwrap_err();
        assert!(err.to_string().contains("27.93-sas-brigade-raid.toml"));
        assert!(!err.field.is_empty());
    }
}

/// Cases: land:30.46
#[test]
fn malformed_chariot_rejects_holes_overlap_bounds_and_invalid_dice_count() {
    for (from, to) in [
        ("die_max = 1", "die_max = 2"),
        ("die_min = 2", "die_min = 3"),
        ("die_min = 1", "die_min = 0"),
        ("die_max = 6", "die_max = 7"),
        ("die_max = 1", "die_max = 2147483647"),
        ("die_min = 2", "die_min = 4"),
        ("dice_to_roll = 3", "dice_to_roll = 4"),
        ("dice_to_roll = 0", "dice_to_roll = -1"),
    ] {
        let err = bind_edited::<ChariotRaid>("land/30.46-", |text| replace_once(text, from, to))
            .unwrap_err();
        assert!(err.to_string().contains("30.46-chariot-raid.toml"));
        assert!(!err.field.is_empty());
    }
}
