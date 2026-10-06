//! Value tests for the Logistics-game tables (sections 49-58). Values marked "chart" were read
//! off the chart image cell by cell while writing the test.

mod common;

use cna_core::dice::Die;
use cna_core::quantity::{AmmoPoints, FuelPoints, WaterPoints};
use cna_tables::airlog::convoys::{AirOrigin, ConvoyLevel, RoadPlace, TruckLoss};
use cna_tables::airlog::fuel::FuelConsumption;
use cna_tables::airlog::supply::{
    AmmoAction, AmmoCost, AmmoMode, DemolitionCondition, DumpCapacity, DumpLocation,
    SupplyDumpDemolition, SupplyType, WaterAvailability, WellAttempt, WellEffect, WellSource,
};
use cna_tables::airlog::trucks::{PortName, StackingItem, TransportMode, TruckType, Weight};
use cna_tables::calendar::Month;
use cna_tables::units::{FuelTenths, Ratio};
use common::{bind_edited, replace_once, tables};

fn die(v: u8) -> Die {
    Die::new(v).expect("die face")
}

// ---- 49.19 fuel ----------------------------------------------------------------------------

#[test]
fn fuel_chart_cells_match_the_chart() {
    // chart: rate 7 at 50 CP is 70 whole points; rate 1 at 1 CP is a fifth of a point.
    let f = &tables().airlog.fuel_consumption;
    assert_eq!(f.printed(7, 50), Some(FuelTenths::new(700)));
    assert_eq!(f.printed(1, 1), Some(FuelTenths::new(2)));
    assert_eq!(f.printed(3, 20), Some(FuelTenths::new(120)));
    assert_eq!(f.printed(4, 6), None, "6 CP is not a printed row");
    assert_eq!(f.printed(8, 5), None, "there is no rate 8 column");
}

#[test]
fn fuel_rounding_follows_interpretation_airlog_0001() {
    // interp:airlog-0001 worked cases: 12 CP at rate 4 costs 12 fuel (three groups of five);
    // 3 CP at rate 5 costs 3; 7 CP at rate 2 is priced as 10 CP = 4.
    let f = &tables().airlog.fuel_consumption;
    assert_eq!(
        f.fuel_for(4, 12).unwrap().ceil_points(),
        FuelPoints::new(12)
    );
    assert_eq!(f.fuel_for(5, 3), Some(FuelTenths::new(30)));
    assert_eq!(f.fuel_for(2, 7), Some(FuelTenths::new(40)));
    // Beyond the last printed row the chart is applied in 50-CP pieces: 120 CP at rate 2 is
    // 20 + 20 + the 20-CP row (8) = 48 fuel.
    assert_eq!(f.fuel_for(2, 120), Some(FuelTenths::new(480)));
    assert_eq!(f.fuel_for(2, 0), Some(FuelTenths::ZERO));
    assert_eq!(f.fuel_for(9, 5), None);
    assert_eq!(f.fuel_for(2, -1), None);
}

#[test]
fn fuel_rounds_small_remainders_after_the_last_chart_row() {
    // interp:airlog-0001: above five CP the complete movement rounds to a multiple of five.
    let f = &tables().airlog.fuel_consumption;
    for (cp, tenths) in [
        (50, 100),
        (51, 110),
        (54, 110),
        (55, 110),
        (56, 120),
        (101, 210),
    ] {
        assert_eq!(f.fuel_for(1, cp), Some(FuelTenths::new(tenths)));
    }
    assert_eq!(
        f.fuel_for(7, i32::MAX),
        None,
        "cost cannot fit in the quantity"
    );
}

#[test]
fn fuel_totals_round_up_to_whole_points_when_drawn() {
    assert_eq!(FuelTenths::new(1).ceil_points(), FuelPoints::new(1));
    assert_eq!(FuelTenths::new(10).ceil_points(), FuelPoints::new(1));
    assert_eq!(FuelTenths::new(11).ceil_points(), FuelPoints::new(2));
    assert_eq!(FuelTenths::ZERO.ceil_points(), FuelPoints::ZERO);
}

#[test]
fn fuel_rows_that_do_not_ascend_are_rejected() {
    let err = bind_edited::<FuelConsumption>("airlog/49.19-", |t| {
        replace_once(t, "cp_expended = 2\n", "cp_expended = 1\n")
    })
    .unwrap_err();
    assert!(err.field.contains("cp_expended"), "{err}");
}

#[test]
fn fuel_rows_with_a_missing_column_are_rejected() {
    let err = bind_edited::<FuelConsumption>("airlog/49.19-", |t| {
        replace_once(
            t,
            "fuel_tenths = [2, 4, 6, 8, 10, 12, 14]",
            "fuel_tenths = [2, 4, 6]",
        )
    })
    .unwrap_err();
    assert!(err.field.contains("fuel_tenths"), "{err}");
}

// ---- 50.2 ammunition -----------------------------------------------------------------------

#[test]
fn ammunition_costs_match_the_chart() {
    let a = &tables().airlog.ammunition_consumption;
    // chart: Barrage 4, Anti-armor 3, Close Assault by armor-class etc. 2, by infantry-class 1.
    assert_eq!(
        a.points(AmmoMode::Played, AmmoAction::Barrage),
        Some(AmmoPoints::new(4))
    );
    assert_eq!(
        a.points(AmmoMode::Played, AmmoAction::AntiArmor),
        Some(AmmoPoints::new(3))
    );
    assert_eq!(
        a.points(
            AmmoMode::Played,
            AmmoAction::CloseAssaultArmorGunMgInfHvywpnInf
        ),
        Some(AmmoPoints::new(2))
    );
    assert_eq!(
        a.points(AmmoMode::Played, AmmoAction::CloseAssaultInfClass),
        Some(AmmoPoints::new(1))
    );
    // chart: abstracted barrage costs 4 phasing and 2 non-phasing.
    assert_eq!(
        a.points(
            AmmoMode::Abstracted,
            AmmoAction::BarrageNonPhasingBattalionEq
        ),
        Some(AmmoPoints::new(2))
    );
}

#[test]
fn ammunition_rating_costs_carry_no_number() {
    // chart: air-to-air/strafe prints "TacAir Bombload" and a non-fighter flight prints
    // "Bombload"; neither has a number (see GAPS.md).
    let a = &tables().airlog.ammunition_consumption;
    assert_eq!(
        a.cost(AmmoMode::Played, AmmoAction::AirToAirCombatOrStrafe),
        Some(&AmmoCost::Rating("tacair_bombload".to_string()))
    );
    assert_eq!(
        a.points(AmmoMode::Abstracted, AmmoAction::FlightByNonFighterPlane),
        None
    );
    assert_eq!(
        a.cost(AmmoMode::Abstracted, AmmoAction::Barrage),
        None,
        "not priced in that mode"
    );
}

// ---- 52.7 / 52.8 water ---------------------------------------------------------------------

#[test]
fn water_draws_match_the_chart() {
    let w: &WaterAvailability = &tables().airlog.water_availability;
    // chart: Town 100 150 200 300 350* 500*; Bir 50 100 150 200* 300* 400*.
    let town: Vec<i32> = (1..=6)
        .map(|d| w.draw(WellSource::Town, die(d)).water.get())
        .collect();
    assert_eq!(town, [100, 150, 200, 300, 350, 500]);
    let bir: Vec<i32> = (1..=6)
        .map(|d| w.draw(WellSource::Bir, die(d)).water.get())
        .collect();
    assert_eq!(bir, [50, 100, 150, 200, 300, 400]);
    assert!(!w.draw(WellSource::Town, die(4)).depletion_check);
    assert!(w.draw(WellSource::Town, die(5)).depletion_check);
    assert!(w.draw(WellSource::Bir, die(4)).depletion_check);
    assert_eq!(w.draw(WellSource::Bir, die(1)).water, WaterPoints::new(50));
    assert_eq!(w.depleted_on(), die(1));
}

#[test]
fn well_attempts_resolve_by_die() {
    // airlog:52.8: poisoning works on a 1; sweetening on 1-3.
    let p = &tables().airlog.poisoning_and_sweetening;
    assert_eq!(
        p.result(WellAttempt::PoisonWell, die(1)),
        WellEffect::WellPoisoned
    );
    assert_eq!(
        p.result(WellAttempt::PoisonWell, die(2)),
        WellEffect::NoEffect
    );
    assert_eq!(
        p.result(WellAttempt::SweetenWell, die(3)),
        WellEffect::WellSweetened
    );
    assert_eq!(
        p.result(WellAttempt::SweetenWell, die(4)),
        WellEffect::NoEffectStillPoisoned
    );
}

#[test]
fn a_water_table_with_a_repeated_die_face_is_rejected() {
    let err = bind_edited::<WaterAvailability>("airlog/52.7-", |t| {
        replace_once(
            t,
            "die = 2\nwater_points = 150",
            "die = 1\nwater_points = 150",
        )
    })
    .unwrap_err();
    assert!(err.field.contains("die"), "{err}");
}

#[test]
fn a_poisoning_table_that_leaves_a_die_face_uncovered_is_rejected() {
    use cna_tables::airlog::supply::PoisoningAndSweetening;
    let err = bind_edited::<PoisoningAndSweetening>("airlog/52.8", |t| {
        replace_once(t, "die_range = [2, 6]", "die_range = [2, 5]")
    })
    .unwrap_err();
    assert!(err.message.contains("6"), "{err}");
}

// ---- 54.12 / 54.17 dumps -------------------------------------------------------------------

#[test]
fn dump_capacities_match_the_chart() {
    let d = &tables().airlog.supply_dump_capacity;
    // chart: Village 2,500 / 8,000 / 3,000 / 1,000; Other Terrain 1,500 / 5,000 / 1,000 / 1,000.
    assert_eq!(
        d.capacity(DumpLocation::Village, SupplyType::Fuel),
        DumpCapacity::Max(8000)
    );
    assert_eq!(
        d.capacity(DumpLocation::Village, SupplyType::Ammo),
        DumpCapacity::Max(2500)
    );
    assert_eq!(
        d.capacity(DumpLocation::OtherTerrain, SupplyType::Stores),
        DumpCapacity::Max(1000)
    );
    assert_eq!(
        d.capacity(DumpLocation::NonDump, SupplyType::Ammo),
        DumpCapacity::Max(50)
    );
    assert_eq!(
        d.capacity(DumpLocation::NonDump, SupplyType::Fuel),
        DumpCapacity::Max(0)
    );
    assert_eq!(
        d.capacity(DumpLocation::MajorCity, SupplyType::Water),
        DumpCapacity::Unlimited
    );
    assert_eq!(
        d.capacity(DumpLocation::TunisTripoli, SupplyType::Fuel),
        DumpCapacity::Unlimited
    );
}

#[test]
fn demolition_percentages_match_the_chart() {
    let d: &SupplyDumpDemolition = &tables().airlog.supply_dump_demolition;
    // chart: -2 -1 0 -> 0%; 1 -> 10; 2 -> 20; 3 -> 33; 4 -> 50; 5 -> 75; 6 7 8+ -> 100.
    let got: Vec<i32> = (-2..=8).map(|r| d.percent_destroyed(r)).collect();
    assert_eq!(got, [0, 0, 0, 10, 20, 33, 50, 75, 100, 100, 100]);
}

#[test]
fn demolition_below_minus_two_is_zero_and_above_eight_is_total() {
    // interp:airlog-0002: any roll below the printed range destroys nothing.
    let d = &tables().airlog.supply_dump_demolition;
    assert_eq!(d.percent_destroyed(-5), 0);
    assert_eq!(d.percent_destroyed(-3), 0);
    assert_eq!(d.percent_destroyed(12), 100);
}

#[test]
fn demolition_modifiers_match_the_chart() {
    let d = &tables().airlog.supply_dump_demolition;
    // chart: +1 per extra third of CPA; +1 full non-shell division; -1 one stacking point or
    // less; -2 major city; +1 small dump; -1 big dump; +1 just captured; -1 enemy far away.
    assert_eq!(
        d.modifier_for(DemolitionCondition::PerAdditionalThirdOfBasicCpaExpended),
        1
    );
    assert_eq!(
        d.modifier_for(DemolitionCondition::AttemptingUnitIsFullNonShellDivision),
        1
    );
    assert_eq!(
        d.modifier_for(DemolitionCondition::AttemptingUnitsTotalOneStackingPointOrLess),
        -1
    );
    assert_eq!(
        d.modifier_for(DemolitionCondition::AttemptInMajorCityHex),
        -2
    );
    assert_eq!(
        d.modifier_for(DemolitionCondition::NotMajorCityAndDumpTotalSupplies500OrLess),
        1
    );
    assert_eq!(
        d.modifier_for(DemolitionCondition::NotMajorCityAndDumpTotalSupplies4000OrMore),
        -1
    );
    assert_eq!(
        d.modifier_for(DemolitionCondition::AttemptingUnitsJustCapturedTheDump),
        1
    );
    assert_eq!(
        d.modifier_for(
            DemolitionCondition::DumpNotJustCapturedAndNearestEnemyUnitAtLeast20CpViaMediumTruckAway
        ),
        -1
    );
    assert_eq!(d.modifiers().len(), 8);
}

#[test]
fn a_demolition_table_whose_percentages_fall_is_rejected() {
    let err = bind_edited::<SupplyDumpDemolition>("airlog/54.17-", |t| {
        replace_once(t, "percent_destroyed = 75", "percent_destroyed = 5")
    })
    .unwrap_err();
    assert!(err.field.contains("percent_destroyed"), "{err}");
}

#[test]
fn a_demolition_table_with_a_gap_in_its_columns_is_rejected() {
    let err = bind_edited::<SupplyDumpDemolition>("airlog/54.17-", |t| {
        replace_once(t, "modified_die = 3\n", "modified_die = 9\n")
    })
    .unwrap_err();
    assert!(err.field.contains("modified_die"), "{err}");
}

// ---- 54.2 trucks ---------------------------------------------------------------------------

#[test]
fn truck_characteristics_match_the_chart() {
    let t = &tables().airlog.truck_characteristics;
    let light = t.truck(TruckType::Light);
    // chart: Light truck CPA 25 infantry / na guns / 40 supplies; carries 1/2 inf TOE, 50 fuel.
    assert_eq!(
        (light.cpa_inf, light.cpa_guns, light.cpa_supplies),
        (25, None, 40)
    );
    assert_eq!(light.capacity_inf_toe_halves, 1);
    assert_eq!(light.capacity_arty_toe, None);
    assert_eq!(light.capacity_fuel_points, 50);
    assert_eq!(light.fuel_capacity_points, 8);
    let heavy = t.truck(TruckType::Heavy);
    // chart: Heavy 20 / 15 / 30; TOE 2 inf, 1 arty, 4 AA; 8 ammo, 250 fuel, 30 stores, 200 water.
    assert_eq!(
        (heavy.cpa_inf, heavy.cpa_guns, heavy.cpa_supplies),
        (20, Some(15), 30)
    );
    assert_eq!(heavy.capacity_inf_toe_halves, 4);
    assert_eq!(heavy.supply_capacity(SupplyType::Water), 200);
    assert_eq!(heavy.supply_capacity(SupplyType::Ammo), 8);
    assert_eq!(
        t.truck(TruckType::Medium)
            .supply_capacity(SupplyType::Stores),
        15
    );
    assert_eq!(t.truck(TruckType::Medium).bar_shift_left, 2);
}

// ---- 54.5 weights --------------------------------------------------------------------------

#[test]
fn equivalent_weights_match_the_chart() {
    let w = &tables().airlog.equivalent_weights;
    // chart: one point of Ammo = 4 tons, Fuel = 1/8, Stores = 1, Water = 1/6.
    assert_eq!(w.tons_per_point(SupplyType::Ammo), Ratio { num: 4, den: 1 });
    assert_eq!(w.tons_per_point(SupplyType::Fuel), Ratio { num: 1, den: 8 });
    assert_eq!(
        w.tons_per_point(SupplyType::Water),
        Ratio { num: 1, den: 6 }
    );
    // 80 fuel points weigh 10 tons; 9 water points weigh 1.5 tons, rounded up to 2.
    assert_eq!(w.tons_for_points(SupplyType::Fuel, 80), 10);
    assert_eq!(w.tons_for_points(SupplyType::Water, 9), 2);
    // A 15,000-ton port moves 120,000 fuel points but only 3,750 ammo points.
    assert_eq!(w.points_in_tons(SupplyType::Fuel, 15_000), 120_000);
    assert_eq!(w.points_in_tons(SupplyType::Ammo, 15_000), 3_750);
    assert_eq!(w.points_in_tons(SupplyType::Water, 100), 600);
}

#[test]
fn replacement_and_truck_weights_match_the_chart() {
    let w = &tables().airlog.equivalent_weights;
    // chart: replacement by air 2 tons; by convoy varies; by interport/rail n/a.
    assert_eq!(
        w.replacement_point_weight(TransportMode::Air),
        Some(Weight::Tons(Ratio { num: 2, den: 1 }))
    );
    assert_eq!(
        w.replacement_point_weight(TransportMode::AxisNavalConvoy),
        Some(Weight::Varies)
    );
    assert_eq!(
        w.replacement_point_weight(TransportMode::InterportOrRailroad),
        Some(Weight::NotApplicable)
    );
    // chart: truck/motorization point by interport 50 tons, by air P (prohibited).
    assert_eq!(
        w.truck_point_weight(TransportMode::Interport),
        Some(Weight::Tons(Ratio { num: 50, den: 1 }))
    );
    assert_eq!(
        w.truck_point_weight(TransportMode::Air),
        Some(Weight::Prohibited)
    );
    assert_eq!(
        w.truck_point_weight(TransportMode::InterportOrRailroad),
        None
    );
}

#[test]
fn stacking_point_equivalents_match_the_chart() {
    let w = &tables().airlog.equivalent_weights;
    // chart: truck/motorization 1/10, replacement 1/5, squadron ground support unit 1/2.
    assert_eq!(
        w.stacking_points(StackingItem::TruckMotorizationPoint),
        Ratio { num: 1, den: 10 }
    );
    assert_eq!(
        w.stacking_points(StackingItem::ReplacementPoint),
        Ratio { num: 1, den: 5 }
    );
    assert_eq!(
        w.stacking_points(StackingItem::SquadronGroundSupportUnit),
        Ratio { num: 1, den: 2 }
    );
    // 7 replacement points use 7/5 stacking points: 2 when rounded up, 1 when rounded down.
    let seven = w.stacking_points(StackingItem::ReplacementPoint).times(7);
    assert_eq!((seven.ceil(), seven.floor()), (2, 1));
}

// ---- 55.3 ports ----------------------------------------------------------------------------

#[test]
fn port_capacities_match_the_chart() {
    let p = &tables().airlog.port_capacity;
    // chart: Tripoli 10 / 10 in / 15 out / 15,000; Tobruk 5 / 1 / 3 / 1,700;
    // Bardia 1 / 0 in / 1 out / 400; Derna 1 / na / 1 / 300; All others 1 / na / na / 100.
    let tripoli = p.port(PortName::Tripoli);
    assert_eq!(
        (
            tripoli.max_efficiency_level,
            tripoli.stacking_points_in,
            tripoli.stacking_points_out
        ),
        (10, Some(10), Some(15))
    );
    assert_eq!(tripoli.max_tonnage, 15_000);
    let tobruk = p.port(PortName::Tobruk);
    assert_eq!(
        (tobruk.max_efficiency_level, tobruk.max_tonnage),
        (5, 1_700)
    );
    assert_eq!(
        (tobruk.stacking_points_in, tobruk.stacking_points_out),
        (Some(1), Some(3))
    );
    assert_eq!(p.port(PortName::Bizerta).max_tonnage, 3_333);
    let bardia = p.port(PortName::Bardia);
    assert_eq!(
        (bardia.stacking_points_in, bardia.stacking_points_out),
        (Some(0), Some(1))
    );
    let derna = p.port(PortName::Derna);
    assert_eq!(
        (derna.stacking_points_in, derna.stacking_points_out),
        (None, Some(1))
    );
    let others = p.port(PortName::AllOthers);
    assert_eq!(
        (
            others.stacking_points_in,
            others.stacking_points_out,
            others.max_tonnage
        ),
        (None, None, 100)
    );
    assert_eq!(p.port(PortName::Benghazi).max_efficiency_level, 3);
}

// ---- 56.4 / 56.5 convoys -------------------------------------------------------------------

#[test]
fn convoy_levels_match_the_chart() {
    let t = &tables().airlog.convoy_level;
    // chart: 1940 has dashes until September; Nov 1941 is E; Jan 1942 is C; Dec 1942 is C.
    assert_eq!(t.level(1940, Month::Aug), None);
    assert_eq!(t.level(1940, Month::Sep), Some(ConvoyLevel::B));
    assert_eq!(t.level(1941, Month::Jan), Some(ConvoyLevel::B));
    assert_eq!(t.level(1941, Month::Jun), Some(ConvoyLevel::G));
    assert_eq!(t.level(1941, Month::Nov), Some(ConvoyLevel::E));
    assert_eq!(t.level(1941, Month::Dec), Some(ConvoyLevel::A));
    assert_eq!(t.level(1942, Month::Apr), Some(ConvoyLevel::G));
    assert_eq!(t.level(1942, Month::Dec), Some(ConvoyLevel::C));
    assert_eq!(t.level(1943, Month::Jan), None, "the chart ends in 1942");
}

#[test]
fn convoy_capacity_matches_the_chart_and_the_worked_example() {
    let c = &tables().airlog.convoy_capacity;
    // chart: E is 11,000 + 2,500 x die. A roll of 4 gives 21,000 (the case 56.21 example).
    assert_eq!(c.capacity(ConvoyLevel::E, die(4)).get(), 21_000);
    // chart: A 6,000 + 1,000 x 1; G 32,000 + 3,000 x 6 = 50,000.
    assert_eq!(c.capacity(ConvoyLevel::A, die(1)).get(), 7_000);
    assert_eq!(c.capacity(ConvoyLevel::G, die(6)).get(), 50_000);
    // "Round fractions upward to the nearest 1,000": B with a roll of 3 is 7,000 + 4,500 = 11,500
    // which becomes 12,000.
    assert_eq!(c.capacity(ConvoyLevel::B, die(3)).get(), 12_000);
    assert_eq!(c.capacity(ConvoyLevel::C, die(2)).get(), 13_000);
}

// ---- 56.18 / 56.26 distances ---------------------------------------------------------------

#[test]
fn convoy_air_distances_match_the_chart() {
    let d = &tables().airlog.convoy_air_distance;
    // chart: lane 1 from Sicily 20, Tobruk 165; lane 5 from Crete 28, Derna 18; lane 1 Crete is a dash.
    assert_eq!(d.distance_hexes(1, AirOrigin::Sicily), Some(20));
    assert_eq!(d.distance_hexes(1, AirOrigin::Tobruk), Some(165));
    assert_eq!(d.distance_hexes(1, AirOrigin::Crete), None);
    assert_eq!(d.distance_hexes(5, AirOrigin::Crete), Some(28));
    assert_eq!(d.distance_hexes(5, AirOrigin::Derna), Some(18));
    assert_eq!(d.distance_hexes(6, AirOrigin::Italy), Some(101));
    assert_eq!(
        d.distance_hexes(7, AirOrigin::Italy),
        None,
        "there is no lane 7"
    );
    assert_eq!(d.route(4), Some("greece_to_benghazi"));
}

#[test]
fn road_distances_are_symmetric_and_match_the_chart() {
    let r = &tables().airlog.road_distance;
    assert_eq!(r.distance(RoadPlace::MarbleArch, RoadPlace::Nofilia), 9);
    assert_eq!(r.distance(RoadPlace::Nofilia, RoadPlace::MarbleArch), 9);
    assert_eq!(r.distance(RoadPlace::Tobruk, RoadPlace::Derna), 23);
    assert_eq!(r.distance(RoadPlace::Cairo, RoadPlace::Nofilia), 239);
    // Cairo from Mersa Matruh is shorter than going via Alexandria (39 + 31), as printed.
    assert_eq!(r.distance(RoadPlace::Cairo, RoadPlace::MersaMatruh), 65);
    assert_eq!(r.distance(RoadPlace::Bardia, RoadPlace::Bardia), 0);
}

// ---- 58.5 abstract truck loss --------------------------------------------------------------

#[test]
fn abstract_truck_losses_match_the_chart() {
    let t = &tables().airlog.abstract_truck_loss;
    // chart: Oct 1940 6/2; Mar 1941 10/1; Jun 1941 5/4; Nov 1942 4/6; Jan 1943 2/4; later dashes.
    assert_eq!(
        t.loss(1940, Month::Oct),
        Some(TruckLoss {
            cw_percent: 6,
            axis_percent: 2
        })
    );
    assert_eq!(
        t.loss(1941, Month::Mar),
        Some(TruckLoss {
            cw_percent: 10,
            axis_percent: 1
        })
    );
    assert_eq!(
        t.loss(1941, Month::Jun),
        Some(TruckLoss {
            cw_percent: 5,
            axis_percent: 4
        })
    );
    assert_eq!(
        t.loss(1942, Month::Nov),
        Some(TruckLoss {
            cw_percent: 4,
            axis_percent: 6
        })
    );
    assert_eq!(
        t.loss(1943, Month::Jan),
        Some(TruckLoss {
            cw_percent: 2,
            axis_percent: 4
        })
    );
    assert_eq!(t.loss(1943, Month::Feb), None);
    assert_eq!(t.loss(1940, Month::Sep), None);
}
