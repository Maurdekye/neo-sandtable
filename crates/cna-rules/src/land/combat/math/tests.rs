//! Authored source tests, not executed during Rules920 source preparation.
use super::*;
use crate::CnaContent;
use cna_core::dice::{Die, TwoDiceReading};
use cna_tables::land::morale::MoraleModifier;
use std::sync::OnceLock;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn unit(size: i32, cohesion: i32) -> CohesionGroup {
    CohesionGroup {
        stacking_points: size,
        cohesion_quarters: cohesion,
        children: vec![],
    }
}
fn roll(tens: u8, units: u8) -> TwoDiceReading {
    TwoDiceReading {
        tens: Die::new(tens).unwrap(),
        units: Die::new(units).unwrap(),
    }
}

/// Cases: land:11.31, land:11.32, land:11.33, land:11.34, land:11.35, land:11.37, land:15.28
#[test]
fn source_strength_examples_pool_before_rounding_and_keep_small_forces_raw() {
    let table = &content().tables.land.combat_calculations;
    assert_eq!(
        strength(table, StrengthActivity::Barrage, &[(9, 3), (18, 3), (9, 3)]),
        Ok((108, 11))
    );
    assert_eq!(
        strength(
            table,
            StrengthActivity::CloseAssault,
            &[(2, 21), (1, 9), (2, 7)]
        ),
        Ok((65, 7))
    );
    // Two independent 4-raw components pool to 8 raw/1 actual, not two zeroes.
    assert_eq!(
        strength(table, StrengthActivity::AntiArmor, &[(4, 1), (4, 1)]),
        Ok((8, 1))
    );
    assert_eq!(assault_strengths(table, 4, 9), Ok((4, 9)));
    assert_eq!(assault_strengths(table, 4, 10), Ok((0, 1)));
    assert_eq!(assault_strengths(table, 5, 10), Ok((1, 1)));
    assert_eq!(flak_strength(table, &[(4, 2)]), Ok(8));
    assert_eq!(
        strength(table, StrengthActivity::Barrage, &[(-1, 1)]),
        Err(MathError::Invalid)
    );
    assert_eq!(
        flak_strength(table, &[(i32::MAX, 2)]),
        Err(MathError::Overflow)
    );
}

/// Cases: land:11.21, land:11.22, land:11.23, land:11.24, land:11.25, land:11.26, land:11.27, land:15.96, land:17.21
/// Interpretations: interp:land-0043
#[test]
fn ordinary_morale_input_and_actual_defense_charge_have_no_feedback_or_refund() {
    assert_eq!(ordinary_cp(true, Activity::FireBarrage), 20);
    assert_eq!(ordinary_cp(false, Activity::FireBarrage), 12);
    assert_eq!(ordinary_cp(true, Activity::ReceiveBarrage), 12);
    assert_eq!(ordinary_cp(true, Activity::AntiArmor), 20);
    assert_eq!(ordinary_cp(false, Activity::Assault { probe: true }), 8);
    // At the CPA boundary: ordinary defense previews -3, actual -4 differential costs only 1 CP.
    assert_eq!(morale_cohesion(false, false, 0, 0, 32, 8), Ok(-12));
    let actual = actual_defender_cp(false, -4, 0).unwrap();
    assert_eq!(actual, 4);
    assert_eq!(cohesion_after_cp(0, 32, 8, actual), Ok(-4));
    // A probe uses ordinary two CP for morale even when the fixed result makes actual defense free.
    assert_eq!(morale_cohesion(false, true, 0, 0, 32, 8), Ok(-8));
    assert_eq!(actual_defender_cp(true, -4, 0), Ok(0));
    assert_eq!(actual_defender_cp(true, -3, 0), Ok(8));
    assert_eq!(actual_defender_cp(false, -3, 0), Ok(12));
    // Prior barrage charges survive a cheap defense. No negative charge/refund is generated.
    assert_eq!(actual_defender_cp(false, -9, 12), Ok(0));
    assert_eq!(cp_delta(true, 12, 20), Ok(8));
    assert_eq!(cp_delta(true, 20, 8), Ok(0));
    assert_eq!(cp_delta(false, 16, 12), Err(MathError::Invalid));
    assert_eq!(
        cohesion_after_cp(i32::MIN, i32::MAX, 0, 4),
        Err(MathError::Overflow)
    );
}

/// Cases: land:6.27, land:6.28, land:6.29, land:17.22, land:17.24
/// Interpretations: interp:land-0021
#[test]
fn largest_unit_averaging_matches_source_and_nested_parents_without_changing_members() {
    let groups = [unit(2, -16), unit(2, -4), unit(2, 12), unit(1, 40)];
    assert_eq!(largest_cohesion(&groups), Ok(-4)); // (-4-1+3)/3 rounds to -1.
    let parent = CohesionGroup {
        stacking_points: 5,
        cohesion_quarters: 99,
        children: groups.to_vec(),
    };
    assert_eq!(largest_cohesion(&[parent, unit(2, -80)]), Ok(-4));
    assert_eq!(groups[0].cohesion_quarters, -16);
    assert_eq!(largest_cohesion(&[unit(2, -13)]), Ok(-13));
    assert_eq!(morale_row(-13), -4);
    assert_eq!(morale_row(15), 3);
    assert_eq!(morale_row(-1000), -17);
    assert_eq!(largest_cohesion(&[]), Err(MathError::Invalid));
}

/// Cases: land:6.27, land:6.28
/// Interpretations: interp:land-0051, interp:land-0021
#[test]
fn cohesion_half_ties_round_away_from_zero_only_when_averaging() {
    for (left, right, expected) in [
        (0, 4, 4),    // +0.5 -> +1
        (0, -4, -4),  // -0.5 -> -1
        (4, 8, 8),    // +1.5 -> +2
        (-4, -8, -8), // -1.5 -> -2
        (0, 3, 0),    // +0.375 -> 0
        (0, -3, 0),   // -0.375 -> 0
        (0, 5, 4),    // +0.625 -> +1
        (0, -5, -4),  // -0.625 -> -1
    ] {
        assert_eq!(
            largest_cohesion(&[unit(2, left), unit(2, right)]),
            Ok(expected)
        );
    }
    for quarters in [2, -2, 6, -6, 13, -13] {
        assert_eq!(largest_cohesion(&[unit(2, quarters)]), Ok(quarters));
    }
    // Resolve the inner brigade's half first, then the equal-sized outer parents.
    let parent = CohesionGroup {
        stacking_points: 5,
        cohesion_quarters: 99,
        children: vec![unit(2, -4), unit(2, -8)],
    };
    assert_eq!(largest_cohesion(&[parent, unit(5, -4)]), Ok(-8));
    // Signed morale-row flooring remains distinct from mean rounding.
    assert_eq!(morale_row(-2), -1);
    assert_eq!(morale_row(2), 0);
}

/// Cases: land:14.32, land:14.35, land:15.4, land:15.51, land:15.52, land:15.53, land:15.54, land:15.56
#[test]
fn column_and_combined_arms_examples_use_real_size_table_and_special_defenses() {
    assert_eq!(shifted_anti_armor_row(9, -1), Ok(8));
    assert_eq!(shifted_anti_armor_row(1, -3), Ok(0));
    assert_eq!(combined_arms_penalty(&[(2, 0)], 1), Ok(1)); // 14 raw rounds to 1, then zero.
    assert_eq!(combined_arms_penalty(&[(4, 0)], 3), Ok(2)); // 28 raw rounds to 3, then one.
    assert_eq!(combined_arms_penalty(&[(20, 0)], 10), Ok(4));
    assert_eq!(combined_arms_penalty(&[(3, 0), (0, 20)], 3), Ok(1)); // Remote infantry cannot support tanks.
    let t = &content().tables.land.organization_size;
    assert_eq!(
        strength_shifts(t, 24, 12, 1, 1, Defense::Ordinary)
            .unwrap()
            .total(),
        Ok(2)
    );
    assert_eq!(
        strength_shifts(t, 12, 24, 1, 1, Defense::Ordinary)
            .unwrap()
            .total(),
        Ok(-2)
    );
    assert_eq!(
        strength_shifts(t, 0, 0, 1, 1, Defense::Ordinary)
            .unwrap()
            .raw_ratio,
        0
    );
    assert_eq!(
        strength_shifts(t, 10, 8, 5, 1, Defense::Ordinary)
            .unwrap()
            .organization,
        4
    );
    let zero = strength_shifts(t, 10, 0, 1, 1, Defense::ZeroRatedAlone).unwrap();
    assert_eq!(
        (zero.raw_ratio, zero.special, zero.attacker_immune),
        (0, 3, true)
    );
    assert_eq!(
        strength_shifts(t, 10, 0, 1, 1, Defense::AllPinned)
            .unwrap()
            .total(),
        Ok(2)
    );
}

/// Cases: land:6.27, land:15.61, land:15.62, land:15.63, land:15.64
#[test]
fn morale_worked_example_uses_largest_force_input_and_fixed_source_rolls() {
    let a_cohesion = largest_cohesion(&[unit(5, -16), unit(5, -16)]).unwrap();
    let d_cohesion = largest_cohesion(&[unit(5, 8), unit(1, -60)]).unwrap();
    let table = &content().tables.land.morale;
    assert_eq!(
        table.modifier_quarters(a_cohesion, roll(4, 3)),
        MoraleModifier::Change(-2)
    );
    assert_eq!(
        table.modifier_quarters(d_cohesion, roll(6, 3)),
        MoraleModifier::Change(0)
    );
    // The source inputs are averaged basic +2 and defender +1: resulting net shift is -1.
    let attacker_adjusted = (2i32 - 2).clamp(-3, 3);
    let defender_adjusted = 1i32.clamp(-3, 3);
    assert_eq!(attacker_adjusted - defender_adjusted, -1);
}

/// Cases: land:11.32, land:15.4, land:15.77, land:15.82, land:15.83, land:15.85, land:15.86
/// Interpretations: interp:land-0046
#[test]
fn extended_example_raw_totals_losses_and_captured_subset_are_hand_calculated() {
    let table = &content().tables.land.combat_calculations;
    assert_eq!(
        strength(table, StrengthActivity::CloseAssault, &[(2, 21), (7, 3)]),
        Ok((63, 6))
    );
    assert_eq!(
        strength(table, StrengthActivity::CloseAssault, &[(2, 5), (4, 7)]),
        Ok((38, 4))
    );
    assert_eq!(combined_arms_penalty(&[(7, 5)], 3), Ok(1));
    // Pinned defenders add 32 raw only to the casualty basis: 38+32=70, not the firing total.
    assert_eq!(raw_loss(63, 5, true, false), Ok(4));
    assert_eq!(raw_loss(70, 15, false, false), Ok(10));
    assert_eq!(raw_loss(70, 15, false, true), Ok(11));
    assert_eq!(capture_obligation(10, 33), Ok(4));
    assert_eq!(minimal_captured_subset(&[3], &[2], &[2], 4), Ok(true));
    assert_eq!(minimal_captured_subset(&[3], &[3], &[2], 4), Ok(false));
    assert_eq!(
        minimal_captured_subset(&[1], &[2], &[2], 4),
        Err(MathError::Invalid)
    );
    assert_eq!(retreat_loss(70, 15, 1, false, false), Ok(17));
    // A short split group's extra obligation is computed on that group's raw only.
    assert_eq!(retreat_loss(21, 0, 1, false, false), Ok(2));
}

/// Cases: land:15.83, land:15.84, land:15.86
/// Interpretations: interp:land-0046
#[test]
fn proportional_floors_leave_owner_remainders_and_selection_requires_every_toe() {
    assert_eq!(origin_floors(7, &[30, 20]), Ok(vec![4, 2])); // Owner chooses the seventh point.
    let limits = [
        Obligation {
            minimum: 7,
            maximum: None,
        },
        Obligation {
            minimum: 4,
            maximum: None,
        },
        Obligation {
            minimum: 2,
            maximum: None,
        },
    ];
    let first = [
        SelectionRow {
            available: 8,
            selected: 5,
            weights: &[1, 1, 0],
        },
        SelectionRow {
            available: 8,
            selected: 2,
            weights: &[1, 0, 1],
        },
    ];
    let second = [
        SelectionRow {
            available: 8,
            selected: 4,
            weights: &[1, 1, 0],
        },
        SelectionRow {
            available: 8,
            selected: 3,
            weights: &[1, 0, 1],
        },
    ];
    assert_eq!(minimal_selection(&first, &limits), Ok(true));
    assert_eq!(minimal_selection(&second, &limits), Ok(true));
    let excess = [
        SelectionRow {
            available: 8,
            selected: 5,
            weights: &[1, 1, 0],
        },
        SelectionRow {
            available: 8,
            selected: 3,
            weights: &[1, 0, 1],
        },
    ];
    assert_eq!(minimal_selection(&excess, &limits), Ok(false));
    // Necessary overshoot is legal; removing either selected TOE would miss the obligation.
    let mixed = [
        SelectionRow {
            available: 1,
            selected: 1,
            weights: &[3],
        },
        SelectionRow {
            available: 1,
            selected: 1,
            weights: &[2],
        },
    ];
    assert_eq!(
        minimal_selection(
            &mixed,
            &[Obligation {
                minimum: 4,
                maximum: None
            }]
        ),
        Ok(true)
    );
    assert_eq!(
        minimal_selection(
            &mixed,
            &[Obligation {
                minimum: 4,
                maximum: Some(4)
            }]
        ),
        Ok(false)
    );
    assert_eq!(
        raw_loss(i32::MAX, i32::MAX, true, false),
        Err(MathError::Overflow)
    );
    assert_eq!(origin_floors(1, &[0, 0]), Err(MathError::Invalid));
}

/// Cases: land:14.23, land:15.15, land:15.21, land:16.46
/// Interpretations: interp:land-0042
#[test]
fn reassignment_uses_raw_budget_survivors_target_ammo_and_every_armor_exclusion() {
    let facts = ReassignmentFacts {
        armor_anywhere: false,
        dummy_only: false,
        existing_assault: true,
    };
    let row = ReassignmentRow {
        fired_here: true,
        fired: 3,
        surviving: 2,
        moved: 2,
        aa_rating: 5,
        ca_ammo: true,
    };
    assert_eq!(reassignment_allowed(3, facts, &[row]), Ok(true)); // budget 10 raw, not rounded candidate actual.
    let too_much = ReassignmentRow {
        fired_here: true,
        fired: 3,
        surviving: 3,
        moved: 3,
        aa_rating: 4,
        ca_ammo: true,
    };
    assert_eq!(reassignment_allowed(3, facts, &[too_much]), Ok(false)); // 12 raw cannot round down to fit.
    for facts in [
        ReassignmentFacts {
            armor_anywhere: true,
            ..facts
        },
        ReassignmentFacts {
            dummy_only: true,
            ..facts
        },
        ReassignmentFacts {
            existing_assault: false,
            ..facts
        },
    ] {
        assert_eq!(reassignment_allowed(4, facts, &[]), Ok(false));
    }
    for (fired_here, fired, surviving, moved, ca_ammo) in [
        (false, 2, 2, 1, true),
        (true, 2, 0, 1, true),
        (true, 0, 2, 1, true),
        (true, 2, 2, 1, false),
    ] {
        assert_eq!(
            reassignment_allowed(
                4,
                facts,
                &[ReassignmentRow {
                    fired_here,
                    fired,
                    surviving,
                    moved,
                    aa_rating: 2,
                    ca_ammo
                }]
            ),
            Ok(false)
        );
    }
    assert_eq!(
        reassignment_allowed(
            1,
            facts,
            &[ReassignmentRow {
                fired_here: true,
                fired: 1,
                surviving: 1,
                moved: 1,
                aa_rating: 1,
                ca_ammo: true
            }]
        ),
        Ok(false)
    );
}

/// Cases: land:10.34
#[test]
fn holding_off_counts_real_nongun_battalion_equivalents_not_toe_or_hqs() {
    let mut units = vec![
        HoldingOffUnit {
            stacking_points: 0,
            combat: true,
            gun: false,
        },
        HoldingOffUnit {
            stacking_points: 1,
            combat: true,
            gun: true,
        },
        HoldingOffUnit {
            stacking_points: 5,
            combat: false,
            gun: false,
        },
    ];
    for _ in 0..11 {
        units.push(HoldingOffUnit {
            stacking_points: 1,
            combat: true,
            gun: false,
        });
    }
    assert_eq!(holding_off_count(&units), Ok(11));
}
