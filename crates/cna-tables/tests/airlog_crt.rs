//! Value tests for the air-game two-dice combat tables (Strafing 40.8, Air Bombardment 41.5,
//! Anti-Aircraft Combat Results 46.3, Flak Adjustment 46.4). Values marked "chart" were read off
//! the chart image while writing the test.

mod common;

use cna_core::dice::{Die, TwoDiceReading};
use cna_tables::airlog::crt::{
    AaCombat, AaEffect, AaGroup, AirBombardment, BombResult, BombTarget, StrafeShiftCase, Strafing,
    Weapon,
};
use common::{bind_edited, replace_once, tables};

fn roll(v: u8) -> TwoDiceReading {
    TwoDiceReading {
        tens: Die::new(v / 10).expect("tens"),
        units: Die::new(v % 10).expect("units"),
    }
}

// ---- 40.8 strafing -------------------------------------------------------------------------

#[test]
fn strafing_results_match_the_chart() {
    let s = &tables().airlog.strafing;
    // chart: 1-5 TacAir points: 11-63 is 0, 64-66 is 1, never more.
    assert_eq!(s.result(3, 0, roll(63)), Some(0));
    assert_eq!(s.result(3, 0, roll(64)), Some(1));
    assert_eq!(s.result(5, 0, roll(66)), Some(1));
    // chart: 11-15 points: reading 66 alone is a 2.
    assert_eq!(s.result(12, 0, roll(65)), Some(1));
    assert_eq!(s.result(12, 0, roll(66)), Some(2));
    // chart: 46 or more points: 11-46 is 1, 51-64 is 2, 65-66 is 3.
    assert_eq!(s.result(60, 0, roll(11)), Some(1));
    assert_eq!(s.result(60, 0, roll(51)), Some(2));
    assert_eq!(s.result(60, 0, roll(65)), Some(3));
    // chart: 36-40 points, 11-16 is 0.
    assert_eq!(s.result(38, 0, roll(16)), Some(0));
    assert_eq!(s.result(38, 0, roll(21)), Some(1));
    assert_eq!(
        s.result(0, 0, roll(11)),
        None,
        "no TacAir points, no attack"
    );
}

#[test]
fn strafing_shifts_move_the_column_and_clamp_at_the_ends() {
    let s: &Strafing = &tables().airlog.strafing;
    // chart: trucks in convoy and supply dumps shift one column right; infantry in a level 2
    // fortification two columns left; armor in a level 2 fortification one left.
    assert_eq!(s.column_shift(StrafeShiftCase::TrucksInConvoy), 1);
    assert_eq!(s.column_shift(StrafeShiftCase::SupplyDump), 1);
    assert_eq!(
        s.column_shift(StrafeShiftCase::InfantryInFortificationLevel0Or1Hex),
        -1
    );
    assert_eq!(
        s.column_shift(StrafeShiftCase::InfantryInFortificationLevel2Hex),
        -2
    );
    assert_eq!(
        s.column_shift(StrafeShiftCase::ArmorInFortificationLevel2Hex),
        -1
    );
    // 3 points shifted right reads the 6-10 column: reading 61 is a 1 there but a 0 unshifted.
    assert_eq!(s.result(3, 0, roll(61)), Some(0));
    assert_eq!(s.result(3, 1, roll(61)), Some(1));
    // A shift past either end stays on the first or last column.
    assert_eq!(s.result(60, 1, roll(65)), Some(3));
    assert_eq!(s.result(3, -2, roll(63)), Some(0));
}

#[test]
fn a_strafing_table_with_a_gap_in_a_column_is_rejected() {
    let err = bind_edited::<Strafing>("airlog/40.8", |t| {
        replace_once(t, "[[64,66],[61,66],[54,65]", "[[65,66],[61,66],[54,65]")
    })
    .unwrap_err();
    assert!(err.message.contains("64"), "{err}");
}

#[test]
fn a_strafing_table_with_overlapping_bands_is_rejected() {
    let err = bind_edited::<Strafing>("airlog/40.8", |t| {
        replace_once(t, "{ min = 6, max = 10 }", "{ min = 7, max = 10 }")
    })
    .unwrap_err();
    assert!(err.field.contains("tacair_points"), "{err}");
}

// ---- 41.5 air bombardment ------------------------------------------------------------------

#[test]
fn bombardment_results_match_the_chart() {
    let b: &AirBombardment = &tables().airlog.air_bombardment;
    // chart: airfields, 81-120 bomb points: 11-42 loses nothing, 43-66 loses one level.
    assert_eq!(
        b.result(
            BombTarget::AirfieldsAirLandingStripsPorts,
            Weapon::Bomb,
            120,
            roll(42)
        ),
        Some(BombResult::Levels(0))
    );
    assert_eq!(
        b.result(
            BombTarget::AirfieldsAirLandingStripsPorts,
            Weapon::Bomb,
            120,
            roll(43)
        ),
        Some(BombResult::Levels(1))
    );
    // chart: airfields, 471+ bomb points: 11-25 is 1, 26-52 is 2, 53-61 is 3, 62-66 is 4.
    for (r, levels) in [(25, 1), (26, 2), (53, 3), (62, 4)] {
        assert_eq!(
            b.result(
                BombTarget::AirfieldsAirLandingStripsPorts,
                Weapon::Bomb,
                500,
                roll(r)
            ),
            Some(BombResult::Levels(levels))
        );
    }
    // chart: supply dump, 161-200 bomb points: 65-66 destroys 30%.
    assert_eq!(
        b.result(BombTarget::SupplyDump, Weapon::Bomb, 200, roll(65)),
        Some(BombResult::PercentDestroyed(30))
    );
    // chart: Axis convoy, 81-120 bomb points: 55-64 destroys 10%, 65-66 destroys 20%.
    assert_eq!(
        b.result(BombTarget::AxisNavalConvoys, Weapon::Bomb, 100, roll(55)),
        Some(BombResult::PercentCargoDestroyed(10))
    );
    assert_eq!(
        b.result(BombTarget::AxisNavalConvoys, Weapon::Bomb, 100, roll(65)),
        Some(BombResult::PercentCargoDestroyed(20))
    );
    // chart: fortification, 391-470 points: only a reading of 11 has no effect.
    assert_eq!(
        b.result(BombTarget::Fortification, Weapon::Bomb, 400, roll(11)),
        Some(BombResult::NoEffect)
    );
    assert_eq!(
        b.result(BombTarget::Fortification, Weapon::Bomb, 400, roll(12)),
        Some(BombResult::FortificationReducedOneLevel)
    );
    // chart: road, 1-20 bomb points: 66 reduces the road to a track; railroad 64-66 destroys it.
    assert_eq!(
        b.result(BombTarget::Road, Weapon::Bomb, 10, roll(66)),
        Some(BombResult::RoadReducedToTrack)
    );
    assert_eq!(
        b.result(BombTarget::Road, Weapon::Bomb, 10, roll(65)),
        Some(BombResult::NoEffect)
    );
    assert_eq!(
        b.result(BombTarget::Railroad, Weapon::Bomb, 10, roll(64)),
        Some(BombResult::RailroadDestroyed)
    );
}

#[test]
fn bombardment_columns_depend_on_the_weapon() {
    let b = &tables().airlog.air_bombardment;
    // chart: torpedo 341+ is column 11 (same as 471+ bombs or 21+ barrage points); the fleet
    // block gives 7 on 65-66 there.
    assert_eq!(b.column(Weapon::Torpedo, 341), Some(11));
    assert_eq!(b.column(Weapon::Bomb, 471), Some(11));
    assert_eq!(b.column(Weapon::Barrage, 21), Some(11));
    assert_eq!(b.column(Weapon::Bomb, 470), Some(10));
    assert_eq!(b.column(Weapon::Barrage, 2), Some(1));
    assert_eq!(b.column(Weapon::Barrage, 3), Some(2));
    assert_eq!(b.column(Weapon::Bomb, 0), None);
    let fleet = BombTarget::TrucksFlakDestructionCombatUnitsCommonwealthFleet;
    assert_eq!(
        b.result(fleet, Weapon::Torpedo, 400, roll(65)),
        Some(BombResult::Count(7))
    );
    // chart: 1-5 torpedo points: 63-66 sinks one.
    assert_eq!(
        b.result(fleet, Weapon::Torpedo, 5, roll(63)),
        Some(BombResult::Count(1))
    );
    assert_eq!(
        b.result(fleet, Weapon::Torpedo, 5, roll(62)),
        Some(BombResult::Count(0))
    );
}

#[test]
fn a_bombardment_table_with_overlapping_rolls_is_rejected() {
    let err = bind_edited::<AirBombardment>("airlog/41.5", |t| {
        replace_once(t, "[[63,66],[56,66],[52,66]", "[[62,66],[56,66],[52,66]")
    })
    .unwrap_err();
    assert!(err.message.contains("62"), "{err}");
}

#[test]
fn a_bombardment_table_with_a_short_row_is_rejected() {
    let err = bind_edited::<AirBombardment>("airlog/41.5", |t| {
        replace_once(
            t,
            "rolls = [[11,66],[11,54],[11,33],[11,22],[],[],[],[],[],[],[]]",
            "rolls = [[11,66]]",
        )
    })
    .unwrap_err();
    assert!(err.field.contains("rolls"), "{err}");
}

// ---- 46.3 flak -----------------------------------------------------------------------------

#[test]
fn aa_results_match_the_chart() {
    let a: &AaCombat = &tables().airlog.aa_combat;
    let other = AaGroup::PlanesOnOtherMissions;
    let fighters = AaGroup::PlanesOnFighterMissions;
    // chart: 1-4 flak points, other missions: 11-55 destroys none, 56-66 destroys one.
    assert_eq!(
        a.planes(other, AaEffect::PlanesDestroyed, 3, 0, roll(55)),
        Some(0)
    );
    assert_eq!(
        a.planes(other, AaEffect::PlanesDestroyed, 3, 0, roll(56)),
        Some(1)
    );
    // chart: 37+ flak points: 65-66 destroys 5; aborts reach 7 on a 66.
    assert_eq!(
        a.planes(other, AaEffect::PlanesDestroyed, 40, 0, roll(65)),
        Some(5)
    );
    assert_eq!(
        a.planes(other, AaEffect::PlanesAborted, 40, 0, roll(66)),
        Some(7)
    );
    assert_eq!(
        a.planes(other, AaEffect::PlanesAborted, 40, 0, roll(11)),
        Some(2)
    );
    // chart: 9-12 points aborts: 11-31 none, 32-62 one.
    assert_eq!(
        a.planes(other, AaEffect::PlanesAborted, 10, 0, roll(31)),
        Some(0)
    );
    assert_eq!(
        a.planes(other, AaEffect::PlanesAborted, 10, 0, roll(32)),
        Some(1)
    );
    // chart: fighter missions, 17-20 points: 11-31 none, 32-65 one, 66 two.
    assert_eq!(
        a.planes(fighters, AaEffect::PlanesDestroyed, 18, 0, roll(31)),
        Some(0)
    );
    assert_eq!(
        a.planes(fighters, AaEffect::PlanesDestroyed, 18, 0, roll(65)),
        Some(1)
    );
    assert_eq!(
        a.planes(fighters, AaEffect::PlanesDestroyed, 18, 0, roll(66)),
        Some(2)
    );
    // Fighter missions have no abort section.
    assert_eq!(
        a.planes(fighters, AaEffect::PlanesAborted, 18, 0, roll(66)),
        None
    );
    assert_eq!(
        a.planes(other, AaEffect::PlanesDestroyed, 0, 0, roll(66)),
        None
    );
}

#[test]
fn flak_density_shift_moves_the_column_right_and_clamps() {
    let a = &tables().airlog.aa_combat;
    let other = AaGroup::PlanesOnOtherMissions;
    // 5-8 points read 11-45 as none; shifted one right (9-12 column) 41-62 is one plane.
    assert_eq!(
        a.planes(other, AaEffect::PlanesDestroyed, 6, 0, roll(41)),
        Some(0)
    );
    assert_eq!(
        a.planes(other, AaEffect::PlanesDestroyed, 6, 1, roll(41)),
        Some(1)
    );
    // A shift past the last column stays on the 37+ column.
    assert_eq!(
        a.planes(other, AaEffect::PlanesDestroyed, 40, 3, roll(65)),
        Some(5)
    );
}

#[test]
fn flak_adjustment_follows_interpretation_airlog_0005() {
    let f = &tables().airlog.flak_adjustment;
    // chart/notes: 24 aircraft shift one column; interp:airlog-0005 extends it to every count.
    assert_eq!(f.column_shift(11), 0);
    assert_eq!(f.column_shift(12), 0);
    assert_eq!(f.column_shift(23), 0);
    assert_eq!(f.column_shift(24), 1);
    assert_eq!(f.column_shift(35), 1);
    assert_eq!(f.column_shift(36), 2);
    assert_eq!(f.column_shift(48), 3);
    assert_eq!(f.column_shift(0), 0);
}

#[test]
fn an_aa_table_missing_a_section_is_rejected() {
    let err = bind_edited::<AaCombat>("airlog/46.3", |t| {
        replace_once(
            t,
            "result_kind = \"planes_aborted\"",
            "result_kind = \"planes_destroyed\"",
        )
    })
    .unwrap_err();
    assert!(err.field.starts_with("section"), "{err}");
}
