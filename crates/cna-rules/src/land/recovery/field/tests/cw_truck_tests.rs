//! Commonwealth single-marker field truck controls; shared-budget groups remain a gap.
use super::*;
use crate::logistics::{
    FuelTruckKind, TruckFuelCohort,
    cargo_history::{CargoHistory, CargoLot, CargoSite, LotSelection,
        motion::{MotionEntry, TruckMotion}},
    pools,
};
use cna_content::{scenario::Placement, units::Trucks};
use cna_core::quantity::WaterPoints;
use cna_tables::airlog::{supply::SupplyType, trucks::TruckType};

const BRITISH: &str = "cw.7_armd_div.11th_hussars";
const INDIAN: &str = "cw.4_indian_div.central_india_horse";

fn cw_fixture(source: &str, points: i32, face: u8) -> (CnaContent, Game<Cna>) {
    let (c, mut game) = fixture(points, face, false);
    assert_eq!(c.units.units[&source.into()].side, Side::Commonwealth);
    let anchor = game.state.land.units.values()
        .find(|unit| unit.side == Side::Commonwealth).unwrap().id.clone();
    for unit in game.state.land.units.values_mut() {
        unit.location = Location::Eliminated;
    }
    game.state.land.units.get_mut(&anchor).unwrap().location = Location::Hex {
        hex: "C4807".into(),
    };
    game.state.cursor.half = Some(Half::A);
    let marker = game.state.land.breakdown.markers.get_mut(MARKER).unwrap();
    marker.side = Side::Commonwealth;
    marker.assets[0].unit = source.into();
    marker.assets[0].equipment = Equipment::MediumTruck;
    for dump in game.state.logistics.dumps.values_mut() {
        dump.supplies = Supplies::default();
    }
    (c, game)
}

fn respond(c: &CnaContent, game: &Game<Cna>, action: Value) -> Game<Cna> {
    let request = Cna::dev().pending(c, &game.state).into_iter()
        .find(|row| row.kind == KIND).unwrap();
    assert_eq!(request.seat, SeatId::new(Side::Commonwealth, Role::RearArea));
    request.space.check(&action).unwrap();
    evaluate(&Cna::dev(), c, game, &command(c, game, action)).unwrap().game
}

fn one_draw(game: &Game<Cna>) -> cna_core::dice::RngState {
    let mut rng = CampaignRng::from_state(&game.rng);
    rng.d6();
    rng.state()
}

fn returned<'a>(game: &'a Game<Cna>, old: &BTreeSet<String>) -> &'a crate::state::TruckPool {
    let rows: Vec<_> = game.state.logistics.truck_pools.iter()
        .filter(|pool| !old.contains(&pool.id)).collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].side, Side::Commonwealth);
    assert_eq!(rows[0].location, Some(Location::Hex { hex: "C4807".into() }));
    rows[0]
}

fn caps(c: &CnaContent) -> (i32, i32) {
    let row = c.tables.airlog.truck_characteristics.truck(TruckType::Medium);
    (row.supply_capacity(SupplyType::Fuel), row.supply_capacity(SupplyType::Stores))
}

fn cargo_fixture(source: &str) -> (CnaContent, Game<Cna>) {
    let (c, mut game) = cw_fixture(source, 4, 1);
    let (fuel, stores) = caps(&c);
    let stage = WaterStage::current(&game.state);
    let segment = crate::logistics::SegmentKey {
        game_turn: game.state.cursor.game_turn, op_stage: game.state.cursor.op_stage,
        half: game.state.cursor.half, cycle: game.state.cursor.cycle,
    };
    let marker = game.state.land.breakdown.markers.get_mut(MARKER).unwrap();
    marker.cargo.medium = Supplies { fuel, stores: stores * 2, ..Default::default() };
    marker.passengers.insert(source.into(), 1);
    marker.transport.medium = 1;
    marker.activity_water = WaterPoints::new(4);
    marker.tank_fuel = FuelTenths::new(c.tables.airlog.truck_characteristics
        .truck(TruckType::Medium).fuel_capacity_points * 40);
    marker.assets[0].cohort = Some("cw-original-trucks".into());
    marker.fuel_cohorts = vec![TruckFuelCohort {
        id: "cw-original-trucks".into(), parent: None, kind: FuelTruckKind::Medium,
        count: 4, cp_quarters: 8, account: source.into(), segment: segment.clone(),
    }];
    marker.paid_truck_water.medium = 4;
    marker.water_credit_stage = Some(stage);
    game.state.logistics.cargo_history.motion.entries.push(MotionEntry {
        site: CargoSite::BrokenMarker(MARKER.into()), stage,
        cohorts: vec![TruckMotion { id: "cw-original-trucks".into(),
            kind: FuelTruckKind::Medium, count: 4, spent_cp_quarters: 16 }],
    });
    game.state.logistics.fuel_accounts.insert(source.into(), crate::logistics::FuelFundingAccount {
        side: Side::Commonwealth, segment, origin: Location::Hex { hex: "C4807".into() },
        paid_cost: FuelTenths::new(30), draws: vec![],
    });
    game.state.logistics.cargo_history.histories.insert(CargoSite::BrokenMarker(MARKER.into()),
        CargoHistory { stage, lots: vec![
            CargoLot { id: "fuel-lot".into(), goods: Supplies { fuel, ..Default::default() },
                spent_cp_quarters: 12, ceiling_cp_quarters: 40, continuous_first_line: false },
            CargoLot { id: "stores-lot".into(), goods: Supplies { stores: stores * 2, ..Default::default() },
                spent_cp_quarters: 28, ceiling_cp_quarters: 60, continuous_first_line: false },
        ] });
    (c, game)
}

fn priority(c: &CnaContent, fuel_first: bool) -> Vec<LotSelection> {
    let (fuel, stores) = caps(c);
    let mut rows = vec![
        LotSelection { lot: "fuel-lot".into(), goods: Supplies { fuel, ..Default::default() } },
        LotSelection { lot: "stores-lot".into(), goods: Supplies { stores: stores * 2, ..Default::default() } },
    ];
    if !fuel_first { rows.reverse(); }
    rows
}

/// Cases: land:22.11, land:22.14, land:22.23, land:22.5, land:22.8
#[test]
fn cw_truck_british_indian_light_medium_heavy_follow_all_six_faces_at_zero_cost() {
    for (source, nationality) in [(BRITISH, "british"), (INDIAN, "indian")] {
        for kind in [Equipment::LightTruck, Equipment::MediumTruck, Equipment::HeavyTruck] {
            for face in 1..=6 {
                let (c, mut game) = cw_fixture(source, 3, face);
                assert_eq!(c.units.units[&source.into()].nationality, nationality);
                game.state.land.breakdown.markers.get_mut(MARKER).unwrap().assets[0].equipment = kind.clone();
                let game = opened(&c, &game);
                let physical = serde_json::to_value((&game.state.land.units,
                    &game.state.land.breakdown, &game.state.logistics)).unwrap();
                let old_ids = game.state.logistics.truck_pool_ids.clone();
                let old_serials = game.state.logistics.truck_pool_serial.clone();
                let accepted = respond(&c, &game, json!([{"marker":MARKER,"points":3}]));
                assert_eq!(accepted.rng, game.rng);
                assert_eq!(serde_json::to_value((&accepted.state.land.units,
                    &accepted.state.land.breakdown, &accepted.state.logistics)).unwrap(), physical);
                let prepared = prepare(&c, &accepted.state,
                    accepted.state.land.recovery.repair_plan.as_ref().unwrap(), false, false).unwrap();
                assert_eq!(prepared.repair.attempts().len(), 1);
                assert_eq!(prepared.repair.attempts()[0].vehicle(), RepairVehicle::Truck);
                assert_eq!(prepared.repair.attempts()[0].demand(), SupplyDemand::default());
                assert_eq!(prepared.repair.site_debits().len(), 1);
                assert_eq!(prepared.repair.site_debits()[0].demand(), SupplyDemand::default());
                assert!(prepared.repair.site_debits()[0].draws().is_empty());
                let rng = one_draw(&accepted);
                let result = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
                assert_eq!(result.game.rng, rng);
                let count = match (&kind, face) {
                    (Equipment::HeavyTruck, 1) => 1,
                    (Equipment::HeavyTruck, _) => 0,
                    (_, 1) => 2,
                    (_, 2) => 1,
                    _ => 0,
                };
                assert_eq!(result.game.state.land.breakdown.markers[MARKER].assets[0].points, 3 - count);
                assert_eq!(result.game.state.land.breakdown.markers[MARKER].assets[0].unit, source.into());
                assert!(result.game.state.land.replacement_receipts.lots.is_empty());
                if count == 0 {
                    assert_eq!(result.game.state.logistics.truck_pool_ids, old_ids);
                    assert_eq!(result.game.state.logistics.truck_pool_serial, old_serials);
                } else {
                    let pool = returned(&result.game, &old_ids);
                    assert_eq!(pool.trucks.total(), count);
                    assert_eq!((pool.trucks.light, pool.trucks.medium, pool.trucks.heavy), match &kind {
                        Equipment::LightTruck => (count, 0, 0),
                        Equipment::MediumTruck => (0, count, 0),
                        _ => (0, 0, count),
                    });
                    assert_eq!(pool.tank_fuel.get(), 0);
                    assert_eq!(pool.activity_water.get(), 0);
                }
                assert_eq!(serde_json::to_value(&result.game.state.logistics.dumps).unwrap(),
                    serde_json::to_value(&game.state.logistics.dumps).unwrap());
                assert_eq!(c.units.units[&source.into()].nationality, nationality);
            }
        }
    }
}

/// Cases: land:22.23, land:22.5, airlog:53.25
#[test]
fn cw_truck_partial_priority_conserves_cargo_passengers_water_and_real_motion_without_funding() {
    for fuel_first in [true, false] {
        let (c, game) = cargo_fixture(INDIAN);
        let original = game.state.land.breakdown.markers[MARKER].clone();
        let before_ids = game.state.logistics.truck_pool_ids.clone();
        let funding = serde_json::to_value((&game.state.logistics.fuel_accounts,
            &game.state.logistics.fuel_segments, &game.state.logistics.pool_fuel_accounts,
            &game.state.logistics.pool_fuel_segments)).unwrap();
        let accepted = respond(&c, &opened(&c, &game), json!([{
            "marker":MARKER,"points":4,"cargo_priority":priority(&c, fuel_first)
        }]));
        let result = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
        let pool = returned(&result.game, &before_ids);
        assert_eq!(pool.trucks.medium, 2);
        assert_eq!(pool.activity_water.get(), 2);
        assert_eq!(pool.tank_fuel.get(), 0);
        let marker = &result.game.state.land.breakdown.markers[MARKER];
        assert_eq!(marker.assets[0].points, 2);
        assert_eq!(marker.assets[0].unit, INDIAN.into());
        assert_eq!(marker.passengers, original.passengers);
        assert_eq!(marker.activity_water.get(), 2);
        assert_eq!(marker.tank_fuel.get(), original.tank_fuel.get() / 2);
        assert_eq!(marker.paid_truck_water, original.paid_truck_water);
        let (fuel, stores) = caps(&c);
        assert_eq!(pool.cargo, if fuel_first {
            Supplies { fuel, stores, ..Default::default() }
        } else { Supplies { stores: stores * 2, ..Default::default() } });
        let remaining = marker.cargo.totals().unwrap();
        assert_eq!(remaining.fuel + pool.cargo.fuel, original.cargo.totals().unwrap().fuel);
        assert_eq!(remaining.stores + pool.cargo.stores, original.cargo.totals().unwrap().stores);
        let history = &result.game.state.logistics.cargo_history.histories[&CargoSite::Pool(pool.id.clone())];
        assert!(history.lots.iter().all(|lot| if lot.goods.fuel > 0 {
            lot.spent_cp_quarters == 12 && lot.ceiling_cp_quarters == 40
        } else { lot.spent_cp_quarters == 28 && lot.ceiling_cp_quarters == 60 }));
        let motion = result.game.state.logistics.cargo_history.motion.entries.iter()
            .find(|entry| entry.site == CargoSite::Pool(pool.id.clone())).unwrap();
        assert_eq!(motion.cohorts.len(), 1);
        assert_eq!(motion.cohorts[0].count, 2);
        assert_eq!(motion.cohorts[0].spent_cp_quarters, 16);
        assert_eq!(serde_json::to_value((&result.game.state.logistics.fuel_accounts,
            &result.game.state.logistics.fuel_segments, &result.game.state.logistics.pool_fuel_accounts,
            &result.game.state.logistics.pool_fuel_segments)).unwrap(), funding);
        assert!(result.game.state.land.replacement_receipts.lots.is_empty());
    }
}

/// Cases: land:22.23, land:22.5, land:22.8
#[test]
fn cw_truck_formerly_refused_one_point_attempt_keeps_exact_body_and_supplies_on_failure() {
    for face in 3..=6 {
        let (c, game) = cw_fixture(BRITISH, 2, face);
        let accepted = respond(&c, &opened(&c, &game), json!([{"marker":MARKER,"points":1}]));
        let physical = serde_json::to_value((&accepted.state.land.units,
            &accepted.state.land.breakdown, &accepted.state.logistics)).unwrap();
        let rng = one_draw(&accepted);
        let result = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
        assert_eq!(result.game.rng, rng);
        assert_eq!(serde_json::to_value((&result.game.state.land.units,
            &result.game.state.land.breakdown, &result.game.state.logistics)).unwrap(), physical);
        assert!(result.events.iter().filter(|e| matches!(&e.event, GameEvent::Note { .. }))
            .all(|e| !e.visible_to(Perspective::Side(Side::Axis))));
    }
}

/// Cases: land:22.23, land:22.5
#[test]
fn cw_truck_full_return_retains_stationary_passengers_and_exhausts_only_real_holdings() {
    let (c, mut game) = cw_fixture(BRITISH, 2, 1);
    let (_, stores) = caps(&c);
    let marker = game.state.land.breakdown.markers.get_mut(MARKER).unwrap();
    marker.passengers.insert(BRITISH.into(), 1);
    marker.transport.medium = 1;
    marker.cargo.medium.stores = stores;
    marker.activity_water = WaterPoints::new(2);
    marker.tank_fuel = FuelTenths::new(10);
    let ids = game.state.logistics.truck_pool_ids.clone();
    let accepted = respond(&c, &opened(&c, &game), json!([{"marker":MARKER,"points":2}]));
    let result = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
    let pool = returned(&result.game, &ids);
    assert_eq!(pool.trucks.medium, 2);
    assert_eq!(pool.cargo.stores, stores);
    assert_eq!(pool.activity_water.get(), 2);
    assert_eq!(pool.tank_fuel.get(), 0);
    let marker = &result.game.state.land.breakdown.markers[MARKER];
    assert!(marker.assets.is_empty());
    assert_eq!(marker.passengers[&BRITISH.into()], 1);
    assert_eq!(marker.transport, Trucks::default());
    assert_eq!(marker.cargo.totals().unwrap(), Supplies::default());
    assert_eq!(marker.tank_fuel.get(), 0);
    assert_eq!(marker.activity_water.get(), 0);
}

/// Cases: land:22.11, land:22.14, land:22.23, land:22.5
#[test]
fn cw_truck_captured_origin_is_preserved_and_native_or_captured_second_marker_is_atomic_gap() {
    for source in [INDIAN, "captured"] {
        let (c, mut game) = cw_fixture(BRITISH, 2, 1);
        let original = if source == "captured" {
            c.units.units.values().find(|unit| unit.side == Side::Axis && unit.nationality == "italian")
                .unwrap().id.clone()
        } else { source.into() };
        let nationality = c.units.units[&original].nationality.clone();
        let side = c.units.units[&original].side;
        let mut second = game.state.land.breakdown.markers[MARKER].clone();
        second.id = "cw-truck-second".into();
        second.assets[0].unit = original.clone();
        game.state.land.breakdown.markers.insert(second.id.clone(), second);
        let opened_game = opened(&c, &game);
        let before = serde_json::to_value(&opened_game).unwrap();
        assert!(matches!(evaluate(&Cna::dev(), &c, &opened_game, &command(&c, &opened_game,
            json!([{"marker":MARKER,"points":2},{"marker":"cw-truck-second","points":2}]))),
            Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "land:22.14"));
        assert_eq!(serde_json::to_value(&opened_game).unwrap(), before);
        let ids = opened_game.state.logistics.truck_pool_ids.clone();
        let accepted = respond(&c, &opened_game, json!([{"marker":"cw-truck-second","points":2}]));
        let rng = one_draw(&accepted);
        let result = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
        assert_eq!(result.game.rng, rng);
        assert_eq!(returned(&result.game, &ids).trucks.medium, 2);
        assert!(!result.game.state.land.breakdown.markers.contains_key("cw-truck-second"));
        assert_eq!(result.game.state.land.breakdown.markers[MARKER].assets[0].points, 2);
        assert_eq!(c.units.units[&original].side, side);
        assert_eq!(c.units.units[&original].nationality, nationality);
    }
}

/// Cases: land:22.14, land:22.23, land:22.5
#[test]
fn cw_truck_true_source_cohort_pool_and_reserve_gaps_refuse_without_game_or_rng_changes() {
    for variant in 0..5 {
        let (mut c, mut game) = cw_fixture(BRITISH, 2, 1);
        let marker = game.state.land.breakdown.markers.get_mut(MARKER).unwrap();
        let expected = match variant {
            0 => { c.units.units.get_mut(&marker.assets[0].unit).unwrap().nationality.clear(); "land:22.14" },
            1 => { marker.assets[0].unit = "unresolved-source".into(); "land:22.14" },
            2 => { marker.assets[0].cohort = Some("missing-physical-row".into()); "land:22.5" },
            3 => { marker.source_pool = Some("not-a-unit-origin".into()); "land:22.5" },
            _ => { marker.activity_water = WaterPoints::new(1); "land:22.5" },
        };
        let game = opened(&c, &game);
        let before = serde_json::to_value(&game).unwrap();
        assert!(matches!(evaluate(&Cna::dev(), &c, &game,
            &command(&c, &game, json!([{"marker":MARKER,"points":2}]))),
            Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == expected));
        assert_eq!(serde_json::to_value(&game).unwrap(), before);
        let mut pass = respond(&c, &game, Value::Null);
        let rng = pass.rng.clone();
        let physical = serde_json::to_value((&pass.state.land.breakdown, &pass.state.logistics)).unwrap();
        assert!(finish_step(&c, &mut pass).unwrap().is_empty());
        assert_eq!(pass.rng, rng);
        assert_eq!(serde_json::to_value((&pass.state.land.breakdown, &pass.state.logistics)).unwrap(), physical);
    }
}

/// Cases: land:22.23, land:22.5, airlog:54.2
#[test]
fn cw_truck_existing_pool_merges_real_capacity_and_rejects_funded_or_overfull_destination() {
    let (c, mut game) = cw_fixture(BRITISH, 2, 1);
    let (_, stores) = caps(&c);
    game.state.land.breakdown.markers.get_mut(MARKER).unwrap().cargo.medium.stores = stores * 2;
    let id = pools::add_truck_pool(&mut game.state.logistics, Some("cw-existing-return"),
        Side::Commonwealth, Placement::Hex { hex: "C4807".into() },
        Some(Location::Hex { hex: "C4807".into() }),
        Trucks { medium: 1, ..Default::default() }, Supplies { stores, ..Default::default() }).unwrap();
    let ids = game.state.logistics.truck_pool_ids.clone();
    for funded in [false, true] {
        let mut invalid: Game<Cna> = serde_json::from_value(serde_json::to_value(&game).unwrap()).unwrap();
        if funded {
            let segment = crate::logistics::SegmentKey { game_turn: invalid.state.cursor.game_turn,
                op_stage: invalid.state.cursor.op_stage, half: invalid.state.cursor.half,
                cycle: invalid.state.cursor.cycle };
            invalid.state.logistics.pool_fuel_accounts.insert(id.clone(), crate::logistics::FuelFundingAccount {
                side: Side::Commonwealth, segment, origin: Location::Hex { hex: "C4807".into() },
                paid_cost: FuelTenths::new(10), draws: vec![],
            });
        } else {
            invalid.state.logistics.truck_pools.iter_mut().find(|pool| pool.id == id).unwrap().cargo.stores += 1;
        }
        let invalid = opened(&c, &invalid);
        let before = serde_json::to_value(&invalid).unwrap();
        assert!(evaluate(&Cna::dev(), &c, &invalid,
            &command(&c, &invalid, json!([{"marker":MARKER,"points":2,"return_pool":id}]))).is_err());
        assert_eq!(serde_json::to_value(&invalid).unwrap(), before);
    }
    let accepted = respond(&c, &opened(&c, &game), json!([{"marker":MARKER,"points":2,"return_pool":id}]));
    let result = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
    assert_eq!(result.game.state.logistics.truck_pool_ids, ids);
    let pool = result.game.state.logistics.truck_pools.iter().find(|pool| pool.id == id).unwrap();
    assert_eq!(pool.trucks.medium, 3);
    assert_eq!(pool.cargo.stores, stores * 3);
    assert_eq!(pool.tank_fuel.get(), 0);
    assert!(!result.game.state.land.breakdown.markers.contains_key(MARKER));
}

/// Cases: land:22.23, land:22.5, airlog:53.25
#[test]
fn cw_truck_missing_motion_stays_unknown_without_creating_pool_funding() {
    use crate::logistics::cargo_history::motion::{self, MotionError, PhysicalTrucks};
    let (c, mut game) = cargo_fixture(BRITISH);
    game.state.logistics.cargo_history.motion.entries.clear();
    let ids = game.state.logistics.truck_pool_ids.clone();
    let accepted = respond(&c, &opened(&c, &game), json!([{"marker":MARKER,"points":4}]));
    let result = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
    let pool = returned(&result.game, &ids);
    assert!(!result.game.state.logistics.pool_fuel_accounts.contains_key(&pool.id));
    assert!(!result.game.state.logistics.pool_fuel_segments.contains_key(&pool.id));
    assert_eq!(result.game.state.logistics.fuel_accounts[&BRITISH.into()].paid_cost.get(), 30);
    let physical = vec![PhysicalTrucks { id: format!("pool.{}.fuel-trucks-2", pool.id),
        parent: Some("cw-original-trucks".into()), kind: FuelTruckKind::Medium, count: 2 }];
    assert!(matches!(motion::query(&result.game.state, Side::Commonwealth,
        &CargoSite::Pool(pool.id.clone()), &physical),
        Err(MotionError::Unknown)));
    assert_eq!(pool.tank_fuel.get(), 0);
}

/// Cases: land:22.5, airlog:49.13, airlog:49.16, airlog:53.25
#[test]
fn cw_truck_returned_pool_plans_new_fuel_and_refuses_empty_stocks_despite_paid_source_unit() {
    use crate::logistics::pool_fuel;
    let (c, game) = cargo_fixture(BRITISH);
    let ids = game.state.logistics.truck_pool_ids.clone();
    let mut accepted = respond(&c, &opened(&c, &game), json!([{
        "marker":MARKER,"points":4,"cargo_priority":priority(&c, true)
    }]));
    finish_step(&c, &mut accepted).unwrap();
    let id = returned(&accepted, &ids).id.clone();
    let site = CargoSite::Pool(id.clone());
    let before = serde_json::to_value(&accepted).unwrap();
    let plan = pool_fuel::plan_pool_segment_fuel(&c, &accepted.state, &id, 4).unwrap();
    assert!(plan.increment.get() > 0);
    assert!(plan.draws.iter().any(|draw| draw.amount.fuel.get() > 0));
    assert!(plan.ledger.cohorts.iter().all(|cohort| cohort.account == id && cohort.cp_quarters == 4));
    assert_eq!(serde_json::to_value(&accepted).unwrap(), before);
    assert!(!accepted.state.logistics.pool_fuel_accounts.contains_key(&id));
    assert!(!accepted.state.logistics.pool_fuel_segments.contains_key(&id));
    let motion = accepted.state.logistics.cargo_history.motion.entries.iter()
        .find(|entry| entry.site == site).unwrap();
    assert_eq!(motion.cohorts[0].spent_cp_quarters, 16);
    let mut empty: Game<Cna> = serde_json::from_value(before).unwrap();
    empty.state.logistics.truck_pools.iter_mut().find(|pool| pool.id == id).unwrap().cargo.fuel = 0;
    if let Some(history) = empty.state.logistics.cargo_history.histories.get_mut(&site) {
        for lot in &mut history.lots { lot.goods.fuel = 0; }
        history.lots.retain(|lot| lot.goods != Supplies::default());
    }
    for dump in empty.state.logistics.dumps.values_mut() { dump.supplies.fuel = 0; }
    for stock in empty.state.logistics.unit_supply.values_mut() { stock.carried.fuel = 0; }
    let origin = empty.state.logistics.truck_pools.iter().find(|pool| pool.id == id)
        .unwrap().location.as_ref().unwrap();
    assert!(pool_fuel::available_pool_sources_at(&c, &empty.state, &id, origin).unwrap()
        .iter().all(|row| row.amount.fuel.get() == 0));
    assert_eq!(empty.state.logistics.fuel_accounts[&BRITISH.into()].paid_cost.get(), 30);
    let before = serde_json::to_value(&empty).unwrap();
    assert!(matches!(pool_fuel::plan_pool_segment_fuel(&c, &empty.state, &id, 4),
        Err(SupplyError::Insufficient)));
    assert_eq!(serde_json::to_value(&empty).unwrap(), before);
}

/// Cases: land:22.23, land:22.5, land:3.6
#[test]
fn cw_truck_omitted_null_empty_priorities_buffer_identical_owner_known_plans() {
    let (c, game) = cargo_fixture(INDIAN);
    let game = opened(&c, &game);
    let before = serde_json::to_value((&game.state.land.breakdown, &game.state.logistics)).unwrap();
    let mut canonical: Option<Value> = None;
    for action in [json!([{"marker":MARKER,"points":4}]),
        json!([{"marker":MARKER,"points":4,"cargo_priority":null}]),
        json!([{"marker":MARKER,"points":4,"cargo_priority":[]}])] {
        let accepted = respond(&c, &game, action);
        assert_eq!(accepted.rng, game.rng);
        assert_eq!(serde_json::to_value((&accepted.state.land.breakdown, &accepted.state.logistics)).unwrap(), before);
        let plan = accepted.state.land.recovery.repair_plan.as_ref().unwrap();
        assert_eq!(plan.markers[0].cargo_priority, priority(&c, true));
        assert!(matches!(plan.markers[0].return_pool, ReturnPoolTarget::CreateAtSite));
        let snapshot = serde_json::to_value(&accepted).unwrap();
        if let Some(canonical) = &canonical { assert_eq!(&snapshot, canonical); }
        else { canonical = Some(snapshot); }
    }
}

/// Cases: land:22.23, land:22.5
#[test]
fn cw_truck_answered_checkpoint_replays_and_same_closure_never_returns_a_second_pool() {
    let (c, game) = cargo_fixture(INDIAN);
    let accepted = respond(&c, &opened(&c, &game), json!([{"marker":MARKER,"points":4}]));
    let checkpoint: Game<Cna> = serde_json::from_value(serde_json::to_value(&accepted).unwrap()).unwrap();
    let first = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
    let replay = evaluate(&Cna::dev(), &c, &checkpoint, &Command::Advance).unwrap();
    assert_eq!(serde_json::to_value(&first.game).unwrap(), serde_json::to_value(&replay.game).unwrap());
    assert_eq!(first.events, replay.events);
    let mut accepted = accepted;
    finish_step(&c, &mut accepted).unwrap();
    let finished = serde_json::to_value(&accepted).unwrap();
    assert!(finish_step(&c, &mut accepted).unwrap().is_empty());
    assert_eq!(serde_json::to_value(&accepted).unwrap(), finished);
}

/// Cases: land:22.23, land:22.5, airlog:53.25
#[test]
fn cw_truck_late_relocation_failure_restores_pool_marker_game_rng_and_nonempty_event_prefix() {
    let (c, game) = cargo_fixture(BRITISH);
    let mut game = respond(&c, &opened(&c, &game), json!([{"marker":MARKER,"points":4}]));
    game.state.logistics.cargo_history.next_id.insert(Side::Commonwealth, u64::MAX);
    let before = serde_json::to_value(&game).unwrap();
    let mut rng = CampaignRng::from_state(&game.rng);
    let rng_before = rng.state();
    let mut events = vec![EngineEvent::new(Audience::Side(Side::Axis),
        GameEvent::Note { text: "existing prefix".into() })];
    let prefix = events.clone();
    assert!(matches!(Cna::full().finish_step(&c, &mut game.state,
        &mut Cx { rng: &mut rng, events: &mut events }), Err(EngineError::Invariant { .. })));
    assert_eq!(serde_json::to_value(&game).unwrap(), before);
    assert_eq!(rng.state(), rng_before);
    assert_eq!(events, prefix);
}

/// Cases: land:3.62, land:22.11, land:22.23, land:22.5
/// Interpretations: interp:land-0038, interp:land-0052
#[test]
fn cw_truck_captured_private_history_pairs_keep_former_owner_response_and_advance_equal() {
    let (c, mut left) = cargo_fixture(BRITISH);
    let original = c.units.units.values().find(|unit| unit.side == Side::Axis && unit.nationality == "italian")
        .unwrap().id.clone();
    let mut account = left.state.logistics.fuel_accounts.remove(&BRITISH.into()).unwrap();
    account.side = Side::Axis;
    left.state.logistics.fuel_accounts.insert(original.clone(), account);
    let marker = left.state.land.breakdown.markers.get_mut(MARKER).unwrap();
    marker.assets[0].unit = original.clone();
    marker.fuel_cohorts[0].account = original.clone();
    let mut right: Game<Cna> = serde_json::from_value(serde_json::to_value(&left).unwrap()).unwrap();
    right.state.logistics.cargo_history.histories.get_mut(&CargoSite::BrokenMarker(MARKER.into()))
        .unwrap().lots[0].spent_cp_quarters = 20;
    let left = opened(&c, &left);
    let right = opened(&c, &right);
    let perspectives = [Perspective::Side(Side::Axis), Perspective::Seat(SeatId::new(Side::Axis, Role::RearArea))];
    for perspective in perspectives {
        assert_eq!(Cna::dev().observe(&c, &left.state, perspective), Cna::dev().observe(&c, &right.state, perspective));
    }
    assert_eq!(serde_json::to_value(Cna::dev().pending(&c, &left.state)).unwrap(),
        serde_json::to_value(Cna::dev().pending(&c, &right.state)).unwrap());
    let action = command(&c, &left, json!([{"marker":MARKER,"points":4}]));
    let left = evaluate(&Cna::dev(), &c, &left, &action).unwrap();
    let right = evaluate(&Cna::dev(), &c, &right, &action).unwrap();
    for perspective in perspectives {
        let visible = |rows: &[EngineEvent]| rows.iter().filter(|e| e.visible_to(perspective)).cloned().collect::<Vec<_>>();
        assert_eq!(visible(&left.events), visible(&right.events));
    }
    let left = evaluate(&Cna::dev(), &c, &left.game, &Command::Advance).unwrap();
    let right = evaluate(&Cna::dev(), &c, &right.game, &Command::Advance).unwrap();
    for perspective in perspectives {
        let visible = |rows: &[EngineEvent]| rows.iter().filter(|e| e.visible_to(perspective)).cloned().collect::<Vec<_>>();
        assert_eq!(visible(&left.events), visible(&right.events));
        assert_eq!(Cna::dev().observe(&c, &left.game.state, perspective), Cna::dev().observe(&c, &right.game.state, perspective));
    }
    assert_ne!(serde_json::to_value(&left.game.state.logistics.cargo_history).unwrap(),
        serde_json::to_value(&right.game.state.logistics.cargo_history).unwrap());
    assert_eq!(c.units.units[&original].nationality, "italian");
    assert_eq!(c.units.units[&original].side, Side::Axis);
}

/// Cases: land:3.61, land:10.15, land:22.13, land:22.23
#[test]
fn cw_truck_hidden_control_changes_only_finish_and_excluded_attempt_spends_and_rolls_nothing() {
    let (c, mut blocked) = cw_fixture(BRITISH, 2, 1);
    let enemies: Vec<cna_core::ids::UnitId> = ["it.1_libyan_div.viii_libyan_bn",
        "it.1_libyan_div.ix_libyan_bn", "it.1_libyan_div.x_libyan_bn", "it.1_libyan_div.xi_libyan_bn"]
        .into_iter().map(Into::into).collect();
    let site: HexId = "C4807".into();
    c.map.neighbors(&site).into_iter().find(|hex| {
        for id in &enemies {
            let unit = blocked.state.land.units.get_mut(id).unwrap();
            unit.location = Location::Hex { hex: hex.id.clone() };
            unit.detached = true;
            unit.toe = Some(cna_content::units::Toe::Normal(cna_content::units::NormalToe::N));
            unit.cohesion_quarters = 0;
        }
        zoc::controlled(&c, &blocked.state, Side::Axis, &site, false).unwrap_or(false)
    }).expect("real full-strength Axis battalions control the repair site");
    let mut clear: Game<Cna> = serde_json::from_value(serde_json::to_value(&blocked).unwrap()).unwrap();
    for id in &enemies { clear.state.land.units.get_mut(id).unwrap().cohesion_quarters = -104; }
    assert!(zoc::controlled(&c, &blocked.state, Side::Axis, &site, false).unwrap());
    assert!(!zoc::controlled(&c, &clear.state, Side::Axis, &site, false).unwrap());
    let blocked = opened(&c, &blocked);
    let clear = opened(&c, &clear);
    let seat = Perspective::Seat(SeatId::new(Side::Commonwealth, Role::RearArea));
    assert_eq!(Cna::dev().observe(&c, &blocked.state, seat), Cna::dev().observe(&c, &clear.state, seat));
    assert_eq!(serde_json::to_value(Cna::dev().pending(&c, &blocked.state)).unwrap(),
        serde_json::to_value(Cna::dev().pending(&c, &clear.state)).unwrap());
    let action = command(&c, &blocked, json!([{"marker":MARKER,"points":2}]));
    let blocked = evaluate(&Cna::dev(), &c, &blocked, &action).unwrap();
    let clear = evaluate(&Cna::dev(), &c, &clear, &action).unwrap();
    assert_eq!(blocked.events, clear.events);
    assert_eq!(blocked.game.rng, clear.game.rng);
    let physical = serde_json::to_value((&blocked.game.state.land.breakdown, &blocked.game.state.logistics)).unwrap();
    let rng = blocked.game.rng.clone();
    let clear_rng = one_draw(&clear.game);
    let blocked = evaluate(&Cna::dev(), &c, &blocked.game, &Command::Advance).unwrap();
    let clear = evaluate(&Cna::dev(), &c, &clear.game, &Command::Advance).unwrap();
    assert_eq!(blocked.game.rng, rng);
    assert_eq!(serde_json::to_value((&blocked.game.state.land.breakdown, &blocked.game.state.logistics)).unwrap(), physical);
    assert_eq!(clear.game.rng, clear_rng);
    assert!(!clear.game.state.land.breakdown.markers.contains_key(MARKER));
    assert!(blocked.events.iter().filter(|e| matches!(&e.event, GameEvent::Note { .. }))
        .all(|e| e.visible_to(seat) && !e.visible_to(Perspective::Side(Side::Axis))));
}
