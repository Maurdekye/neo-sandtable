//! Values checked against each combat chart image, plus paraphrased rule examples.
mod common;
use cna_core::dice::{Die, TwoDiceReading};
use cna_tables::land::{
    anti_armor::AntiArmorTable,
    assault::{CloseAssaultTable, CombatSide as Side},
    barrage::{BarrageResult, BarrageTable, BarrageTarget as Target},
    combat::{
        CombatCalculations, CombatModifier, OrganizationSize, PrisonersCaptured, StrengthActivity,
    },
    morale::{AdjustedMorale, MoraleModifier as Modifier, MoraleTable},
};
use common::{bind_edited, replace_once, tables};
fn roll(n: u8) -> TwoDiceReading {
    TwoDiceReading {
        tens: Die::new(n / 10).unwrap(),
        units: Die::new(n % 10).unwrap(),
    }
}

/// Cases: land:11.3, land:11.32, land:11.33, land:11.34, land:15.51
#[test]
fn combat_recipes_match_the_summary_and_pool_before_rounding() {
    let t = &tables().land.combat_calculations;
    for activity in [
        StrengthActivity::Barrage,
        StrengthActivity::AntiArmor,
        StrengthActivity::CloseAssault,
    ] {
        assert_eq!(t.recipe(activity).actual_divisor, 10);
        assert_eq!(t.actual_points(activity, 114), Some(11));
        assert_eq!(t.actual_points(activity, 115), Some(12));
        assert_eq!(t.actual_points(activity, 4), Some(0));
    }
    assert!(
        t.recipe(StrengthActivity::CloseAssault)
            .modifiers
            .contains(&CombatModifier::CombinedArms)
    );
    assert_eq!(t.raw_points([(4, 1), (4, 1)]), Some(8));
    assert_eq!(t.actual_points(StrengthActivity::AntiArmor, 8), Some(1));
    assert_eq!(t.assault_points(4, 6), Some((4, 6)));
    assert_eq!(t.assault_points(5, 10), Some((1, 1)));
    assert_eq!(t.assault_points(-1, 2), None);
    assert_eq!(t.raw_points([(i32::MAX, 2)]), None);
    assert_eq!(t.raw_points([(-1, 1)]), None);
}
/// Cases: land:11.35
#[test]
fn division_strength_example_totals_artillery_and_assault_separately() {
    let t = &tables().land.combat_calculations;
    let barrage = t.raw_points([(9, 3), (18, 3), (9, 3)]).unwrap();
    assert_eq!(barrage, 108);
    assert_eq!(
        t.actual_points(StrengthActivity::Barrage, barrage),
        Some(11)
    );
    // Three infantry groups contribute 42, 9 and 14 raw offensive points.
    assert_eq!(
        t.actual_points(StrengthActivity::CloseAssault, 42 + 9 + 14),
        Some(7)
    );
}
/// Cases: land:15.53, land:15.52
#[test]
fn organizational_size_uses_the_largest_counter_and_signed_favor() {
    let t = &tables().land.organization_size;
    assert_eq!(t.assault_shift(5, 0), Some(8));
    assert_eq!(t.assault_shift(3, 1), Some(2));
    assert_eq!(t.assault_shift(1, 3), Some(-2));
    assert_eq!(t.assault_shift(3, 2), Some(0));
    assert_eq!(t.assault_shift(5, 5), Some(0));
    assert_eq!(t.assault_shift(4, 1), None);
}
/// Cases: land:15.89, land:15.85
#[test]
fn prisoner_percentage_is_hand_checked_for_every_die_face() {
    let t = &tables().land.prisoners_captured;
    for (face, percentage) in (1..=6).zip([10, 25, 33, 50, 50, 75]) {
        assert_eq!(t.percent(Die::new(face).unwrap()), percentage);
    }
}
/// Cases: land:12.6, land:12.44, land:12.45, land:12.46
#[test]
fn barrage_distinguishes_pin_losses_and_carried_infantry_trucks() {
    let t = &tables().land.barrage;
    assert_eq!(
        t.result(5, 0, Target::Infantry, roll(66), true),
        Some(BarrageResult {
            toe_points_lost: 1,
            pinned: true,
            transport_truck_points_lost: 1
        })
    );
    assert_eq!(
        t.result(13, 0, Target::Armor, roll(64), false)
            .unwrap()
            .toe_points_lost,
        1
    );
    assert!(
        t.result(13, 0, Target::Armor, roll(64), false)
            .unwrap()
            .pinned
    );
    assert_eq!(
        t.result(17, 0, Target::Gun, roll(41), false),
        Some(BarrageResult {
            toe_points_lost: 2,
            pinned: false,
            transport_truck_points_lost: 0
        })
    );
    assert_eq!(
        t.result(1, 0, Target::Truck, roll(66), false),
        Some(BarrageResult::default())
    );
    assert_eq!(
        t.result(9, 99, Target::Truck, roll(64), false)
            .unwrap()
            .toe_points_lost,
        2
    );
    assert_eq!(
        t.result(1, 1, Target::Infantry, roll(66), false),
        Some(BarrageResult::default())
    );
    assert_eq!(t.result(-1, 0, Target::Gun, roll(11), false), None);
}
/// Cases: land:12.33, land:12.34
#[test]
fn fortified_gun_example_moves_twelve_points_two_columns_left() {
    let t = &tables().land.barrage;
    assert_eq!(
        t.result(12, 2, Target::Gun, roll(65), false)
            .unwrap()
            .toe_points_lost,
        1
    );
    assert_eq!(
        t.result(8, 0, Target::Gun, roll(65), false),
        t.result(12, 2, Target::Gun, roll(65), false)
    );
    assert_eq!(
        t.result(12, 0, Target::Gun, roll(65), false)
            .unwrap()
            .toe_points_lost,
        2
    );
}
/// Cases: land:14.6, land:14.35, land:14.41, land:11.33
#[test]
fn anti_armor_checks_dashes_last_column_and_phasing_row_penalty() {
    let t = &tables().land.anti_armor;
    assert_eq!(t.damage(16, 160, 0, false, roll(66)), Some(32));
    assert_eq!(t.damage(i32::MAX, 160, 0, false, roll(66)), Some(32));
    assert_eq!(t.damage(2, 20, 0, false, roll(11)), Some(0));
    assert_eq!(t.damage(0, 4, 0, false, roll(65)), Some(2));
    assert_eq!(t.damage(0, 4, 0, true, roll(65)), Some(1));
    assert_eq!(t.damage(0, 5, 0, false, roll(65)), None);
    assert_eq!(t.damage(1, 10, 2, false, roll(65)), Some(2));
    assert_eq!(t.damage(16, 160, 0, true, roll(11)), Some(22));
}
/// Cases: land:14.32
#[test]
fn rough_terrain_example_moves_nine_anti_armor_points_to_eight() {
    let t = &tables().land.anti_armor;
    assert_eq!(t.damage(9, 90, 1, false, roll(65)), Some(17));
    assert_eq!(t.damage(8, 80, 0, false, roll(65)), Some(17));
    assert_eq!(t.damage(9, 90, 0, false, roll(65)), Some(19));
}
/// Cases: land:15.79, land:15.73
#[test]
fn assault_worked_loss_example_and_sum_flags_use_one_dice_pair() {
    let t = &tables().land.close_assault;
    assert_eq!(t.resolve(Side::Attacker, -2, roll(16)).loss_percent, 20);
    assert_eq!(t.resolve(Side::Attacker, -2, roll(21)).loss_percent, 15);
    let a = t.resolve(Side::Attacker, 3, roll(34));
    assert_eq!(a.loss_percent, 5);
    assert!(!a.engaged && !a.captured && !a.overrun);
    assert!(t.resolve(Side::Attacker, 0, roll(45)).engaged); // sum9, loss5%
    let d = t.resolve(Side::Defender, 0, roll(23));
    assert_eq!(d.loss_percent, 15);
    assert_eq!(d.retreat_hexes, 1); // the same2and3 give sum5
    assert!(t.resolve(Side::Defender, 0, roll(11)).captured);
    assert_eq!(t.resolve(Side::Defender, 4, roll(56)).retreat_hexes, 2);
}
/// Cases: land:15.79, land:15.77
/// Interpretations: interp:land-0011
#[test]
fn assault_preserves_printed_errata_and_adopted_gap_with_open_extremes() {
    let t = &tables().land.close_assault;
    for n in [34, 35, 36] {
        assert_eq!(t.resolve(Side::Defender, 2, roll(n)).loss_percent, 10);
    }
    assert_eq!(t.resolve(Side::Defender, 4, roll(45)).loss_percent, 10);
    let d = t.resolve(Side::Defender, i32::MAX, roll(66));
    assert_eq!(d.loss_percent, 5);
    assert_eq!(d.retreat_hexes, 3);
    assert!(d.overrun);
    assert_eq!(
        t.resolve(Side::Attacker, i32::MIN, roll(11)).loss_percent,
        50
    );
}
/// Cases: land:17.4, land:17.22, land:17.23, land:17.24
/// Interpretations: interp:land-0012
#[test]
fn morale_matches_example_clamps_endpoints_and_fills_only_the_adopted_gap() {
    let t = &tables().land.morale;
    assert_eq!(t.adjusted(1, -4, roll(42)), AdjustedMorale::Rating(-1));
    assert_eq!(t.modifier(-4, roll(56)), Modifier::Change(-2));
    assert_eq!(t.modifier(i32::MAX, roll(11)), Modifier::Change(4));
    assert_eq!(t.modifier(i32::MIN, roll(66)), Modifier::Surrender);
    assert_eq!(t.adjusted(2, 8, roll(11)), AdjustedMorale::Rating(3));
    assert_eq!(t.adjusted(-2, -8, roll(45)), AdjustedMorale::Rating(-3));
    assert_eq!(t.adjusted(3, -7, roll(64)), AdjustedMorale::Surrender);
}
/// Cases: land:17.4, land:17.22
/// Interpretations: interp:land-0020, interp:land-0021
#[test]
fn fractional_morale_row_is_selected_without_rounding_stored_cohesion() {
    let t = &tables().land.morale;
    assert_eq!(t.modifier_quarters(-13, roll(34)), Modifier::Change(-1));
    assert_eq!(t.modifier_quarters(-12, roll(34)), Modifier::Change(0));
    assert_eq!(t.modifier_quarters(15, roll(14)), Modifier::Change(2));
    assert_eq!(t.modifier_quarters(-13, roll(56)), Modifier::Change(-2));
}
#[test]
fn malformed_combat_tables_name_the_file_and_bad_field() {
    let e = bind_edited::<CombatCalculations>("land/11.4-", |s| {
        replace_once(s, "raw points divided by 10", "raw points divided by 0")
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("11.4-") && e.contains("actual_points"), "{e}");
    let e = bind_edited::<OrganizationSize>("land/15.53-", |s| {
        replace_once(s, "larger_sp = [2, 3]", "larger_sp = [2]")
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("15.53-") && e.contains("row"), "{e}");
    let e = bind_edited::<PrisonersCaptured>("land/15.89-", |s| {
        replace_once(s, "percent_prisoners = 75", "percent_prisoners = 101")
    })
    .unwrap_err()
    .to_string();
    assert!(
        e.contains("15.89-") && e.contains("percent_prisoners"),
        "{e}"
    );
}
#[test]
fn barrage_rejects_bad_column_and_uncovered_reading() {
    let e = bind_edited::<BarrageTable>("land/12.6-", |s| {
        replace_once(
            s,
            "{ column = \"1-2\", dice = [11, 53] }",
            "{ column = \"unknown\", dice = [11, 53] }",
        )
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("12.6-") && e.contains("column"), "{e}");
    let e = bind_edited::<BarrageTable>("land/12.6-", |s| {
        replace_once(s, "dice = [11, 53]", "dice = [11, 52]")
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("12.6-") && e.contains("53"), "{e}");
}
#[test]
fn anti_armor_rejects_unknown_damage_column_and_reordered_dice_rows() {
    let e =
        bind_edited::<AntiArmorTable>("land/14.6-", |s| replace_once(s, "\"3\" = 1", "\"17\" = 1"))
            .unwrap_err()
            .to_string();
    assert!(
        e.contains("14.6-") && e.contains("damage_points_by_column"),
        "{e}"
    );
    let e = bind_edited::<AntiArmorTable>("land/14.6-", |s| {
        replace_once(s, "dice = [11, 12]", "dice = [11, 14]")
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("14.6-") && e.contains("dice"), "{e}");
}
#[test]
fn assault_rejects_missing_non_gap_reading_and_invalid_sum() {
    let e = bind_edited::<CloseAssaultTable>("land/15.79-", |s| {
        replace_once(s, "dice = [11, 15]", "dice = [11, 14]")
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("15.79-") && e.contains("15"), "{e}");
    let e = bind_edited::<CloseAssaultTable>("land/15.79-", |s| {
        replace_once(s, "sums = [2, 3, 4, 5, 6, 7]", "sums = [1, 3, 4, 5, 6, 7]")
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("15.79-") && e.contains("sums"), "{e}");
}
#[test]
fn morale_rejects_source_gap_fill_and_a_different_missing_reading() {
    let e = bind_edited::<MoraleTable>("land/17.4-", |s| {
        replace_once(s, "dice = [42, 55]", "dice = [42, 56]")
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("17.4-") && e.contains("gap"), "{e}");
    let e = bind_edited::<MoraleTable>("land/17.4-", |s| {
        replace_once(s, "dice = [11, 26]", "dice = [11, 25]")
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("17.4-") && e.contains("26"), "{e}");
}

/// Cases: land:15.52, land:15.53, land:15.79
#[test]
fn assault_column_shifts_cross_bands_without_changing_their_widths() {
    let t = &tables().land.close_assault;
    assert_eq!(t.resolve(Side::Defender, 5, roll(11)).loss_percent, 30);
    assert_eq!(
        t.resolve_shifted(Side::Defender, 5, 1, roll(11))
            .loss_percent,
        40
    );
    assert_eq!(
        t.resolve_shifted(Side::Defender, 5, -1, roll(11))
            .loss_percent,
        25
    );
    assert!(t.resolve_shifted(Side::Defender, 9, 1, roll(11)).overrun);
    assert_eq!(
        t.resolve_shifted(Side::Defender, i32::MAX, i32::MAX, roll(66)),
        t.resolve(Side::Defender, i32::MAX, roll(66))
    );
}
