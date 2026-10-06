//! Value tests for the air-game tables of sections 34-45. Values marked "chart" were read off the
//! chart image cell by cell while writing the test.

mod common;

use cna_core::dice::{Die, TwoDiceReading};
use cna_tables::airlog::air::{
    AircraftRefit, KillThreshold, LossCause, MaltaCommitment, MaltaForces, MaltaScenario,
    ManeuverAdjustment, MissionSide, Nationality, OffMapFacility, PilotFate, PlaneFate, Reveal,
    SquadronKind, SquadronLimit, TacAirKill,
};
use cna_tables::airlog::distance::{
    AirLeg, AirPlace, AxisArea, LandPlace, NorthAfricaPlace, NorthMedArea,
};
use cna_tables::calendar::Month;
use common::{bind_edited, replace_once, tables};

fn die(v: u8) -> Die {
    Die::new(v).expect("die face")
}

fn roll(v: u8) -> TwoDiceReading {
    TwoDiceReading {
        tens: die(v / 10),
        units: die(v % 10),
    }
}

// ---- 34 pilots -----------------------------------------------------------------------------

#[test]
fn commonwealth_pilot_arrivals_match_the_chart() {
    let t = &tables().airlog.commonwealth_pilots;
    // chart: 5 -> 1,2,1,-; 7 -> 2,4,-,-; 3 -> -,3,-,-; 2 -> all dashes; 10 -> -,1,-,1.
    let five = t.pilots(5).unwrap();
    assert_eq!(
        (five.count(1), five.count(2), five.count(3), five.count(4)),
        (1, 2, 1, 0)
    );
    let seven = t.pilots(7).unwrap();
    assert_eq!((seven.count(1), seven.count(2)), (2, 4));
    assert_eq!(t.pilots(3).unwrap().count(2), 3);
    assert_eq!(t.pilots(2).unwrap().total(), 0);
    assert_eq!(t.pilots(10).unwrap().count(4), 1);
    assert_eq!(t.pilots(13), None);
    assert_eq!(t.pilots(5).unwrap().count(6), 0, "no rating-six column");
}

#[test]
fn pilot_rolls_happen_only_in_their_months() {
    // chart notes: Commonwealth November 1940 - October 1942; Axis (Italian) the same; German
    // March 1941 - October 1942.
    let cw = &tables().airlog.commonwealth_pilots;
    assert!(!cw.rolls_in(1940, Month::Oct));
    assert!(cw.rolls_in(1940, Month::Nov));
    assert!(cw.rolls_in(1942, Month::Oct));
    assert!(!cw.rolls_in(1942, Month::Nov));
    let de = &tables().airlog.german_pilots;
    assert!(!de.rolls_in(1941, Month::Feb));
    assert!(de.rolls_in(1941, Month::Mar));
    assert!(tables().airlog.italian_pilots.rolls_in(1941, Month::Jan));
}

#[test]
fn italian_and_german_pilot_arrivals_match_the_chart() {
    // chart: Italian 12 -> -,3,2,-; 11 -> -,-,2,1. German 3 -> 3,-,1,1; 10 -> 3,-,-,2.
    let it = &tables().airlog.italian_pilots;
    let twelve = it.pilots(12).unwrap();
    assert_eq!(
        (
            twelve.count(1),
            twelve.count(2),
            twelve.count(3),
            twelve.count(4)
        ),
        (0, 3, 2, 0)
    );
    assert_eq!(it.pilots(11).unwrap().count(4), 1);
    let de = &tables().airlog.german_pilots;
    let three = de.pilots(3).unwrap();
    assert_eq!((three.count(1), three.count(3), three.count(4)), (3, 1, 1));
    assert_eq!(de.pilots(10).unwrap().count(4), 2);
}

#[test]
fn the_german_ace_arrives_on_game_turn_29() {
    // chart note: in any OpStage of Game-Turn 29 the Germans receive one rating-six pilot.
    let extra = tables()
        .airlog
        .german_pilots
        .scheduled_extra()
        .expect("extra pilot");
    assert_eq!(
        (extra.game_turn, extra.pilot_rating, extra.count),
        (29, 6, 1)
    );
    assert!(tables().airlog.italian_pilots.scheduled_extra().is_none());
}

#[test]
fn a_pilot_table_missing_a_dice_total_is_rejected() {
    use cna_tables::airlog::air::CommonwealthPilotArrival;
    let err = bind_edited::<CommonwealthPilotArrival>("airlog/34.86", |t| {
        replace_once(
            t,
            "dice_total = 12\npilots = [0, 0, 1, 0]",
            "dice_total = 11\npilots = [0, 0, 1, 0]",
        )
    })
    .unwrap_err();
    assert_eq!(err.field, "row.dice_total");
}

// ---- 35 / 36 squadrons and facilities ------------------------------------------------------

#[test]
fn squadron_capacities_match_the_chart() {
    let s = &tables().airlog.squadron_capacity;
    // chart: Italian 9/3/12, German 12/4/16, CW 1940-41 12/4/16, CW 1942-43 18/6/24.
    let it = s.capacity(SquadronKind::ItalianSquadriglia);
    assert_eq!((it.ready, it.reserve, it.total), (9, 3, 12));
    let de = s.capacity(SquadronKind::GermanStaffel);
    assert_eq!((de.ready, de.reserve, de.total), (12, 4, 16));
    let late = s.capacity(SquadronKind::CommonwealthSquadron194243);
    assert_eq!((late.ready, late.reserve, late.total), (18, 6, 24));
}

#[test]
fn a_squadron_total_that_does_not_add_up_is_rejected() {
    use cna_tables::airlog::air::SquadronCapacityTable;
    let err = bind_edited::<SquadronCapacityTable>("airlog/35.23", |t| {
        replace_once(t, "total = 12", "total = 13")
    })
    .unwrap_err();
    assert!(err.field.contains("total"), "{err}");
}

#[test]
fn off_map_facilities_match_the_chart() {
    let f = &tables().airlog.offmap_air_facilities;
    // chart: Port Said E4033, 12 hexes, 3 squadrons (flying boat basin); Kabrit E1833, 13, 9;
    // Ethiopia E0127, 60, unlimited, from Game-Turn 35 OpStage 1.
    let ps = f.facility(OffMapFacility::PortSaid);
    assert_eq!(
        (ps.entry_exit_hex.as_str(), ps.distance_hexes),
        ("E4033", 12)
    );
    assert_eq!(ps.max_squadrons, SquadronLimit::Squadrons(3));
    assert_eq!(ps.facility_type.as_deref(), Some("flying_boat_basin"));
    let kabrit = f.facility(OffMapFacility::Kabrit);
    assert_eq!(
        (kabrit.entry_exit_hex.as_str(), kabrit.distance_hexes),
        ("E1833", 13)
    );
    assert_eq!(kabrit.max_squadrons, SquadronLimit::Squadrons(9));
    let eth = f.facility(OffMapFacility::Ethiopia);
    assert_eq!(eth.max_squadrons, SquadronLimit::Unlimited);
    assert_eq!(eth.distance_hexes, 60);
    let from = eth.available_from.expect("Ethiopia opens late");
    assert_eq!((from.game_turn, from.opstage), (35, 1));
    assert_eq!(f.all().len(), 7);
}

// ---- 37 distances --------------------------------------------------------------------------

#[test]
fn air_distances_match_the_chart() {
    let d = &tables().airlog.air_distance;
    // chart part A: Tripoli-Tripolitania 32, Tunis-Tripolitania 91, Tunis-Gabes 40,
    // Malta-Gabes 58, Benghazi-Tunis 136.
    assert_eq!(
        d.between_axis_areas(AxisArea::Tripoli, AxisArea::Tripolitania),
        32
    );
    assert_eq!(
        d.between_axis_areas(AxisArea::Tripolitania, AxisArea::Tunis),
        91
    );
    assert_eq!(d.between_axis_areas(AxisArea::Gabes, AxisArea::Tunis), 40);
    assert_eq!(d.between_axis_areas(AxisArea::Gabes, AxisArea::Gabes), 0);
    assert_eq!(
        d.place_to_axis_area(NorthAfricaPlace::Malta, AxisArea::Gabes),
        58
    );
    assert_eq!(
        d.place_to_axis_area(NorthAfricaPlace::Benghazi, AxisArea::Tunis),
        136
    );
    // chart part B: Malta-Crete 105, Malta-Malta dash, Nofilia-Malta 80, Nofilia-Sicily P,
    // Benghazi-Italy 138, Mersa Matruh-Crete 66, Alexandria-Malta P.
    assert_eq!(
        d.from_north_med(NorthMedArea::Crete, AirPlace::Malta),
        Some(AirLeg::Hexes(105))
    );
    assert_eq!(d.from_north_med(NorthMedArea::Malta, AirPlace::Malta), None);
    assert_eq!(
        d.from_north_med(NorthMedArea::Malta, AirPlace::Nofilia),
        Some(AirLeg::Hexes(80))
    );
    assert_eq!(
        d.from_north_med(NorthMedArea::Sicily, AirPlace::Nofilia),
        Some(AirLeg::Prohibited)
    );
    assert_eq!(
        d.from_north_med(NorthMedArea::Italy, AirPlace::Benghazi),
        Some(AirLeg::Hexes(138))
    );
    assert_eq!(
        d.from_north_med(NorthMedArea::Crete, AirPlace::MersaMatruh),
        Some(AirLeg::Hexes(66))
    );
    assert_eq!(
        d.from_north_med(NorthMedArea::Malta, AirPlace::Alexandria),
        Some(AirLeg::Prohibited)
    );
}

#[test]
fn land_distances_are_symmetric_and_match_the_chart() {
    let d = &tables().airlog.land_distance;
    // chart: El Agheila-Nofilia 18; Soluch-Benghazi 7; Cairo-Nofilia 163; Cairo-Wadi Natrun 14;
    // Gerawla-Mersa Matruh 3; Tobruk-Bardia 16.
    assert_eq!(d.distance(LandPlace::ElAgheila, LandPlace::Nofilia), 18);
    assert_eq!(d.distance(LandPlace::Nofilia, LandPlace::ElAgheila), 18);
    assert_eq!(d.distance(LandPlace::Soluch, LandPlace::Benghazi), 7);
    assert_eq!(d.distance(LandPlace::Cairo, LandPlace::Nofilia), 163);
    assert_eq!(d.distance(LandPlace::Cairo, LandPlace::WadiNatrun), 14);
    assert_eq!(d.distance(LandPlace::Gerawla, LandPlace::MersaMatruh), 3);
    assert_eq!(d.distance(LandPlace::Bardia, LandPlace::Tobruk), 16);
    assert_eq!(d.distance(LandPlace::Tobruk, LandPlace::Tobruk), 0);
    // chart row headers: Tobruk is hex C4807, Alexandria E3613, C2803 is its own hex.
    assert_eq!(d.hex_of(LandPlace::Tobruk), "C4807");
    assert_eq!(d.hex_of(LandPlace::Alexandria), "E3613");
    assert_eq!(d.hex_of(LandPlace::C2803), "C2803");
}

#[test]
fn a_land_distance_row_of_the_wrong_length_is_rejected() {
    use cna_tables::airlog::distance::LandDistance;
    let err = bind_edited::<LandDistance>("airlog/37.42", |t| {
        replace_once(t, "to = [35, 30]", "to = [35]")
    })
    .unwrap_err();
    assert!(err.field.contains("to"), "{err}");
}

// ---- 38 refit ------------------------------------------------------------------------------

#[test]
fn refit_matches_the_chart() {
    let r: &AircraftRefit = &tables().airlog.aircraft_refit;
    // chart part A: Commonwealth 2-8, German 2-7, Italian 2-6; +2 if not assigned to the SGSU.
    assert!(r.plane_refitted(Nationality::Commonwealth, 8, true));
    assert!(!r.plane_refitted(Nationality::Commonwealth, 9, true));
    assert!(r.plane_refitted(Nationality::German, 7, true));
    assert!(
        !r.plane_refitted(Nationality::German, 6, false),
        "6 + 2 = 8 is past 2-7"
    );
    assert!(
        r.plane_refitted(Nationality::Italian, 4, false),
        "4 + 2 = 6 is within 2-6"
    );
    assert_eq!(r.plane_unassigned_modifier(), 2);
    // chart part B: 1 -> 100%, 2 -> 80, 3 -> 70, 4 -> 60, 5 -> 50, 6-7 -> 40, 8-9 -> 33.
    let got: Vec<i32> = (1..=9)
        .map(|d| r.squadron_percent_refitted(d).unwrap())
        .collect();
    assert_eq!(got, [100, 80, 70, 60, 50, 40, 40, 33, 33]);
    assert_eq!(r.squadron_percent_refitted(10), None);
    let m = r.squadron_modifiers();
    assert_eq!(
        (
            m.german_sgsu,
            m.italian_sgsu,
            m.planes_not_assigned_to_refitting_sgsu
        ),
        (1, 2, 1)
    );
}

// ---- 39 missions ---------------------------------------------------------------------------

#[test]
fn mission_summary_lists_each_mission_with_its_case() {
    let m = &tables().airlog.mission_summary;
    // chart: scramble is a fighter mission under 40.3 that may be flown at night; Hurricane IID
    // tank strafing is restricted; ports are bombed under 41.39.
    let scramble = m.land_support("scramble").expect("scramble");
    assert_eq!(scramble.case.as_deref(), Some("40.3"));
    assert!(scramble.night);
    assert!(
        m.land_support("strafe_tanks")
            .unwrap()
            .restriction
            .is_some()
    );
    assert_eq!(
        m.land_support("bomb_ports").unwrap().case.as_deref(),
        Some("41.39")
    );
    assert_eq!(
        m.strategic(MissionSide::Axis, "bomb_maltese_air_facilities")
            .unwrap()
            .case
            .as_deref(),
        Some("44.2")
    );
    assert!(
        m.strategic(MissionSide::Commonwealth, "cap_over_malta")
            .is_some()
    );
    assert!(m.strategic(MissionSide::Axis, "cap_over_malta").is_some());
    assert!(m.land_support("no_such_mission").is_none());
}

// ---- 40.4 / 41.39 --------------------------------------------------------------------------

#[test]
fn scramble_thresholds_match_the_chart() {
    let s = &tables().airlog.scramble;
    // chart: distance 0-2 -> 6, 3-4 -> 5, 5-6 -> 4, 7-8 -> 3, 9-10 -> 2, 11-15 -> 1.
    let got: Vec<u8> = [0, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 15]
        .into_iter()
        .map(|d| s.scramble_on_die_at_most(d).unwrap())
        .collect();
    assert_eq!(got, [6, 6, 5, 5, 4, 4, 3, 3, 2, 2, 1, 1]);
    assert_eq!(s.scramble_on_die_at_most(16), None);
    assert!(s.scrambles(4, die(5)));
    assert!(!s.scrambles(4, die(6)));
    assert!(!s.scrambles(20, die(1)));
}

#[test]
fn mining_thresholds_match_the_chart() {
    let m = &tables().airlog.mining_harbor;
    // chart: bombload 0-5 -> 0, 6-10 -> 1, 11-15 -> 2, 16-20 -> 3, 21-25 -> 4, 26-30 -> 5, 31+ -> 6.
    let got: Vec<u8> = [0, 5, 6, 10, 11, 15, 16, 20, 21, 25, 26, 30, 31, 100]
        .into_iter()
        .map(|b| m.mined_on_die_at_most(b))
        .collect();
    assert_eq!(got, [0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6]);
    assert!(m.mines(31, die(6)));
    assert!(!m.mines(5, die(1)));
}

// ---- 42 recon ------------------------------------------------------------------------------

#[test]
fn land_recon_matches_the_chart() {
    let r = &tables().airlog.recon_land_units;
    // chart: die 1,2 -> 0; 3,4 -> 1; 5,6 -> 2; 7 -> 3 ... 12 -> 8; 13+ -> all. One is added per
    // four planes completing the mission, fractions ignored.
    assert_eq!(r.reveal(die(2), 0), Reveal::BattalionEquivalents(0));
    assert_eq!(r.reveal(die(3), 0), Reveal::BattalionEquivalents(1));
    assert_eq!(r.reveal(die(6), 0), Reveal::BattalionEquivalents(2));
    // 6 + 1 (four planes) = 7 -> 3; 6 + 1 with seven planes still adds just one.
    assert_eq!(r.reveal(die(6), 4), Reveal::BattalionEquivalents(3));
    assert_eq!(r.reveal(die(6), 7), Reveal::BattalionEquivalents(3));
    // 6 + 6 (24 planes) = 12 -> 8; 6 + 7 (28 planes) = 13 -> all.
    assert_eq!(r.reveal(die(6), 24), Reveal::BattalionEquivalents(8));
    assert_eq!(r.reveal(die(6), 28), Reveal::All);
}

#[test]
fn convoy_recon_matches_the_chart() {
    let r = &tables().airlog.recon_axis_convoys;
    // chart: 1 plane -> 12, 2 -> 14, 3 -> 22, 4 -> 32, 5 -> 43, 6 -> 53, 7 -> 62, 8+ -> 66.
    let got: Vec<i32> = (1..=9)
        .map(|p| r.respond_on_roll_at_most(p).unwrap())
        .collect();
    assert_eq!(got, [12, 14, 22, 32, 43, 53, 62, 66, 66]);
    assert_eq!(r.respond_on_roll_at_most(0), None);
    assert!(r.must_respond(1, roll(12)));
    assert!(!r.must_respond(1, roll(13)));
    assert!(r.must_respond(8, roll(66)));
    // chart: Small 5,000 tons or less, Medium 3,000-10,000, Large 7,000+; the ranges overlap.
    let classes = r.size_classes();
    assert_eq!(classes.len(), 3);
    assert_eq!(classes[1].min_tons, Some(3000));
    assert_eq!(classes[1].max_tons, Some(10000));
    assert_eq!(classes[2].max_tons, None);
}

// ---- 44 Malta ------------------------------------------------------------------------------

#[test]
fn malta_commitment_matches_the_chart() {
    let c = &tables().airlog.malta_commitment;
    // chart: Campaign Game U / 25 / 12 / 12; Graziani U then na; Race for Tobruk U/6/3/1;
    // Last Chance all na; Long Retreat U / 3 / na / na.
    assert_eq!(
        c.commitment(MaltaScenario::CampaignGame, 1),
        Some(MaltaCommitment::Unlimited)
    );
    assert_eq!(
        c.commitment(MaltaScenario::CampaignGame, 2),
        Some(MaltaCommitment::GameTurns(25))
    );
    assert_eq!(
        c.commitment(MaltaScenario::RaceForTobruk, 4),
        Some(MaltaCommitment::GameTurns(1))
    );
    assert_eq!(
        c.commitment(MaltaScenario::Crusader, 3),
        Some(MaltaCommitment::GameTurns(2))
    );
    assert_eq!(
        c.commitment(MaltaScenario::GrazianiOffensive, 2),
        Some(MaltaCommitment::NotApplicable)
    );
    assert_eq!(
        c.commitment(MaltaScenario::LastChance, 1),
        Some(MaltaCommitment::NotApplicable)
    );
    assert_eq!(
        c.commitment(MaltaScenario::LongRetreat, 2),
        Some(MaltaCommitment::GameTurns(3))
    );
    assert_eq!(
        c.commitment(MaltaScenario::LongRetreat, 3),
        Some(MaltaCommitment::NotApplicable)
    );
    assert_eq!(c.commitment(MaltaScenario::CampaignGame, 5), None);
    assert_eq!(c.commitment(MaltaScenario::CampaignGame, 0), None);
}

#[test]
fn malta_availability_matches_the_chart() {
    let a = &tables().airlog.malta_availability;
    let f = |in_play, strategic| MaltaForces {
        in_play_percent: in_play,
        strategic_percent: strategic,
    };
    // chart: level I on 2 -> 10/30; level II on 2 -> 100/200; level III on 2 -> na; level IV on
    // 12 -> 25/75; level I on 4 -> na; level III on 11 -> 100/300; level II on 9 -> 25/25.
    assert_eq!(a.forces(1, 2), Some(f(10, 30)));
    assert_eq!(a.forces(2, 2), Some(f(100, 200)));
    assert_eq!(a.forces(3, 2), None);
    assert_eq!(a.forces(4, 12), Some(f(25, 75)));
    assert_eq!(a.forces(1, 4), None);
    assert_eq!(a.forces(3, 11), Some(f(100, 300)));
    assert_eq!(a.forces(2, 9), Some(f(25, 25)));
    assert_eq!(a.forces(5, 7), None);
    assert_eq!(a.forces(1, 13), None);
}

#[test]
fn maltese_construction_matches_the_chart() {
    let c = &tables().airlog.maltese_construction;
    // chart: die 1 -> 0 levels, 2-5 -> 1, 6 -> 2.
    let got: Vec<i32> = (1..=6).map(|d| c.levels(die(d))).collect();
    assert_eq!(got, [0, 1, 1, 1, 1, 2]);
}

// ---- 45 air-to-air -------------------------------------------------------------------------

#[test]
fn maneuver_adjustment_matches_the_chart() {
    let m: &ManeuverAdjustment = &tables().airlog.maneuver_adjustment;
    // chart: 0 -> 0; 1-3 -> 1; 4-7 -> 2; 8-14 -> 3; 15-20 -> 4; 21+ -> 5.
    let got: Vec<i32> = [0, 1, 3, 4, 7, 8, 14, 15, 20, 21, 40]
        .into_iter()
        .map(|g| m.adjustment(g))
        .collect();
    assert_eq!(got, [0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5]);
    assert_eq!(m.adjustment(-8), 3, "the gap is the absolute difference");
}

#[test]
fn tacair_kill_thresholds_match_the_chart() {
    let k: &TacAirKill = &tables().airlog.tacair_kill;
    // chart: -13 or less na; -12..-5 -> 11; -4..-2 -> 12; -1 -> 13; 0 -> 14; +1 -> 15; +2 -> 16;
    // +3 -> 22; +4 -> 24; +5 -> 26; +6 -> 32; +7 -> 34; +8 -> 36; +9 -> 43; +10 -> 46;
    // +11 -> 53; +12 or more -> 56.
    assert_eq!(k.threshold(-20), KillThreshold::Never);
    assert_eq!(k.threshold(-13), KillThreshold::Never);
    let at = |d| k.threshold(d);
    assert_eq!(at(-12), KillThreshold::AtMost(11));
    assert_eq!(at(-5), KillThreshold::AtMost(11));
    assert_eq!(at(-4), KillThreshold::AtMost(12));
    assert_eq!(at(-2), KillThreshold::AtMost(12));
    let got: Vec<KillThreshold> = (-1..=11).map(at).collect();
    let want: Vec<KillThreshold> = [13, 14, 15, 16, 22, 24, 26, 32, 34, 36, 43, 46, 53]
        .into_iter()
        .map(KillThreshold::AtMost)
        .collect();
    assert_eq!(got, want);
    assert_eq!(at(12), KillThreshold::AtMost(56));
    assert_eq!(at(30), KillThreshold::AtMost(56));
}

#[test]
fn tacair_kill_follows_the_table_not_the_worked_example() {
    // interp:airlog-0005: at +2 a plane dies on readings up to 16 only (the example that says
    // 22 is a typo); at +5 up to 26 (the other example agrees with the chart).
    let k = &tables().airlog.tacair_kill;
    assert!(k.kills(2, roll(16)));
    assert!(!k.kills(2, roll(21)));
    assert!(k.kills(5, roll(26)));
    assert!(!k.kills(5, roll(31)));
    assert!(!k.kills(-13, roll(11)), "na never kills");
    assert!(k.kills(-12, roll(11)));
    assert!(!k.kills(-12, roll(12)));
}

#[test]
fn a_kill_table_whose_thresholds_fall_is_rejected() {
    let err = bind_edited::<TacAirKill>("airlog/45.5", |t| {
        replace_once(t, "kill_on_roll_at_most = 22", "kill_on_roll_at_most = 12")
    })
    .unwrap_err();
    assert!(err.field.contains("kill_on_roll_at_most"), "{err}");
}

#[test]
fn a_kill_table_with_a_gap_between_differentials_is_rejected() {
    let err = bind_edited::<TacAirKill>("airlog/45.5", |t| {
        replace_once(t, "differential = [4, 4]", "differential = [5, 5]")
    })
    .unwrap_err();
    assert!(err.field.contains("differential"), "{err}");
}

#[test]
fn recovery_matches_the_chart() {
    let r = &tables().airlog.pilot_and_plane_recovery;
    // chart: strafed 1,2 -> plane repairable (R), 3-6 -> lost (L); flak/air-air 1 -> R&A,
    // 2 -> L&A, 3-6 -> L&K.
    for d in [1, 2] {
        let s = r.recover(LossCause::Strafed, die(d));
        assert_eq!((s.plane, s.pilot), (PlaneFate::Repairable, None));
    }
    assert_eq!(r.recover(LossCause::Strafed, die(3)).plane, PlaneFate::Lost);
    let one = r.recover(LossCause::FlakAirAir, die(1));
    assert_eq!(
        (one.plane, one.pilot),
        (PlaneFate::Repairable, Some(PilotFate::Survives))
    );
    let two = r.recover(LossCause::FlakAirAir, die(2));
    assert_eq!(
        (two.plane, two.pilot),
        (PlaneFate::Lost, Some(PilotFate::Survives))
    );
    let six = r.recover(LossCause::FlakAirAir, die(6));
    assert_eq!(
        (six.plane, six.pilot),
        (PlaneFate::Lost, Some(PilotFate::Killed))
    );
}
