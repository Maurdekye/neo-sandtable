//! Actual field-truck answers, production finish, physical return and history controls.
use super::*;
use crate::logistics::{cargo_history::{CargoHistory, CargoLot, CargoSite, LotSelection}, pools};
use crate::logistics::{FuelTruckKind, TruckFuelCohort, cargo_history::motion::{MotionEntry, TruckMotion}};
use cna_content::{scenario::Placement, units::Trucks};
use cna_core::quantity::WaterPoints;
use cna_tables::airlog::{supply::SupplyType, trucks::TruckType};

fn truck_fixture(points: i32, roll: u8) -> (CnaContent, Game<Cna>) {
    let (c, mut game) = super::fixture(points, roll, false);
    let marker = game.state.land.breakdown.markers.get_mut(MARKER).unwrap();
    marker.assets[0].equipment = Equipment::MediumTruck;
    (c, game)
}
fn caps(c: &CnaContent) -> (i32, i32) {
    let chart = c.tables.airlog.truck_characteristics.truck(TruckType::Medium);
    (chart.supply_capacity(SupplyType::Fuel), chart.supply_capacity(SupplyType::Stores))
}
fn cargo_fixture() -> (CnaContent, Game<Cna>) {
    let (c, mut game) = truck_fixture(4, 1);
    let (fuel, stores) = caps(&c);
    let source = game.state.land.breakdown.markers[MARKER].assets[0].unit.clone();
    let stage = WaterStage::current(&game.state);
    let segment = crate::logistics::SegmentKey::current(&game.state);
    let marker = game.state.land.breakdown.markers.get_mut(MARKER).unwrap();
    marker.cargo.medium = Supplies { fuel, stores: stores * 2, ..Default::default() };
    marker.passengers.insert(source.clone(), 1);
    marker.transport.medium = 1;
    marker.activity_water = WaterPoints::new(4);
    marker.tank_fuel = FuelTenths::new(c.tables.airlog.truck_characteristics.truck(TruckType::Medium).fuel_capacity_points * 40);
    marker.assets[0].cohort = Some("original-unit-trucks".into());
    marker.fuel_cohorts = vec![TruckFuelCohort { id: "original-unit-trucks".into(), parent: None,
        kind: FuelTruckKind::Medium, count: 4, cp_quarters: 8, account: source.clone(), segment: segment.clone() }];
    marker.paid_truck_water.medium = 4;
    marker.water_credit_stage = Some(stage);
    game.state.logistics.cargo_history.motion.entries.push(MotionEntry { site: CargoSite::BrokenMarker(MARKER.into()),
        stage, cohorts: vec![TruckMotion { id: "original-unit-trucks".into(), kind: FuelTruckKind::Medium, count: 4, spent_cp_quarters: 16 }] });
    game.state.logistics.fuel_accounts.insert(source, crate::logistics::FuelFundingAccount {
        side: Side::Axis, segment, origin: Location::Hex { hex: "C4807".into() },
        paid_cost: FuelTenths::new(30), draws: vec![],
    });
    game.state.logistics.cargo_history.histories.insert(CargoSite::BrokenMarker(MARKER.into()), CargoHistory {
        stage,
        lots: vec![
            CargoLot { id: "fuel-lot".into(), goods: Supplies { fuel, ..Default::default() }, spent_cp_quarters: 12, ceiling_cp_quarters: 40, continuous_first_line: false },
            CargoLot { id: "stores-lot".into(), goods: Supplies { stores: stores * 2, ..Default::default() }, spent_cp_quarters: 28, ceiling_cp_quarters: 60, continuous_first_line: false },
        ],
    });
    (c, game)
}
fn priority(c: &CnaContent, fuel_first: bool) -> Vec<LotSelection> {
    let (fuel, stores) = caps(c);
    let mut rows = vec![LotSelection { lot: "fuel-lot".into(), goods: Supplies { fuel, ..Default::default() } },
        LotSelection { lot: "stores-lot".into(), goods: Supplies { stores: stores * 2, ..Default::default() } }];
    if !fuel_first { rows.reverse(); }
    rows
}
fn truck_answer(c: &CnaContent, game: &Game<Cna>, points: i32, target: &str, rows: Vec<LotSelection>) -> Game<Cna> {
    let action = json!([{"marker": MARKER, "points": points, "return_pool": target, "cargo_priority": rows}]);
    evaluate(&Cna::dev(), c, game, &command(c, game, action)).unwrap().game
}
fn new_pool<'a>(game: &'a Game<Cna>, before: &std::collections::BTreeSet<String>) -> &'a crate::state::TruckPool {
    let mut pools = game.state.logistics.truck_pools.iter().filter(|p| !before.contains(&p.id));
    let pool = pools.next().expect("one returned pool");
    assert!(pools.next().is_none());
    pool
}

/// Cases: land:22.23, land:22.5, airlog:53.25
#[test]
fn partial_truck_return_uses_predice_priority_keeps_passengers_and_real_lot_cp() {
    for fuel_first in [true, false] {
        let (c, game) = cargo_fixture();
        let before_ids = game.state.logistics.truck_pool_ids.clone();
        let funding = serde_json::to_value((&game.state.logistics.fuel_accounts, &game.state.logistics.fuel_segments,
            &game.state.logistics.pool_fuel_accounts, &game.state.logistics.pool_fuel_segments)).unwrap();
        let original = game.state.land.breakdown.markers[MARKER].clone();
        let opened = opened(&c, &game);
        let accepted = truck_answer(&c, &opened, 4, "create_at_site", priority(&c, fuel_first));
        assert_eq!(accepted.rng, opened.rng);
        assert_eq!(serde_json::to_value(&accepted.state.logistics).unwrap(), serde_json::to_value(&opened.state.logistics).unwrap());
        let result = evaluate(&Cna::dev(), &c, &accepted, &Command::Advance).unwrap();
        let pool = new_pool(&result.game, &before_ids);
        assert_eq!(pool.trucks.medium, 2);
        assert_eq!(pool.activity_water.get(), 2);
        assert_eq!(pool.tank_fuel.get(), 0);
        let marker = &result.game.state.land.breakdown.markers[MARKER];
        assert_eq!(marker.assets[0].points, 2);
        assert_eq!(marker.passengers, original.passengers);
        assert_eq!(marker.activity_water.get(), 2);
        assert_eq!(marker.tank_fuel.get(), original.tank_fuel.get() / 2);
        let (fuel, stores) = caps(&c);
        let expected = if fuel_first { Supplies { fuel, stores, ..Default::default() } }
            else { Supplies { stores: stores * 2, ..Default::default() } };
        assert_eq!(pool.cargo, expected);
        let retained = marker.cargo.totals().unwrap();
        assert_eq!(retained.fuel + pool.cargo.fuel, original.cargo.totals().unwrap().fuel);
        assert_eq!(retained.stores + pool.cargo.stores, original.cargo.totals().unwrap().stores);
        let imported = &result.game.state.logistics.cargo_history.histories[&CargoSite::Pool(pool.id.clone())];
        assert!(imported.lots.iter().all(|lot| if lot.goods.fuel > 0 { lot.spent_cp_quarters == 12 && lot.ceiling_cp_quarters == 40 }
            else { lot.spent_cp_quarters == 28 && lot.ceiling_cp_quarters == 60 }));
        let motion = result.game.state.logistics.cargo_history.motion.entries.iter().find(|e| e.site == CargoSite::Pool(pool.id.clone())).unwrap();
        assert_eq!(motion.cohorts.len(), 1);
        assert_eq!(motion.cohorts[0].count, 2);
        assert_eq!(motion.cohorts[0].spent_cp_quarters, 16);
        assert_eq!(motion.cohorts[0].id, format!("pool.{}.fuel-trucks-2", pool.id));
        assert_eq!(marker.fuel_cohorts[0].count, 2);
        assert_eq!(marker.fuel_cohorts[0].cp_quarters, 8);
        assert_eq!(marker.paid_truck_water, original.paid_truck_water);
        assert_eq!(serde_json::to_value((&result.game.state.logistics.fuel_accounts, &result.game.state.logistics.fuel_segments,
            &result.game.state.logistics.pool_fuel_accounts, &result.game.state.logistics.pool_fuel_segments)).unwrap(), funding);
        assert_eq!(result.game.state.logistics.dumps["ax_dump_1"].supplies.fuel, 20);
        assert!(result.events.iter().any(|e| matches!(&e.event, GameEvent::Note { text } if text.contains("tank-fuel tenths lost"))));
    }
}

/// Cases: land:22.23, land:22.5
#[test]
fn full_return_keeps_stationary_passengers_and_creates_no_fuel_or_capacity_on_marker() {
    let (c, mut game) = truck_fixture(2, 1);
    let (_, stores) = caps(&c);
    let passenger = game.state.land.breakdown.markers[MARKER].assets[0].unit.clone();
    let marker = game.state.land.breakdown.markers.get_mut(MARKER).unwrap();
    marker.passengers.insert(passenger.clone(), 1);
    marker.transport.medium = 1;
    marker.cargo.medium.stores = stores;
    marker.activity_water = WaterPoints::new(2);
    marker.tank_fuel = FuelTenths::new(10);
    let before_ids = game.state.logistics.truck_pool_ids.clone();
    let game = truck_answer(&c, &opened(&c, &game), 2, "create_at_site", vec![]);
    let result = evaluate(&Cna::dev(), &c, &game, &Command::Advance).unwrap();
    let pool = new_pool(&result.game, &before_ids);
    assert_eq!(pool.trucks.medium, 2);
    assert_eq!(pool.cargo.stores, stores);
    assert_eq!(pool.activity_water.get(), 2);
    assert_eq!(pool.tank_fuel.get(), 0);
    let marker = &result.game.state.land.breakdown.markers[MARKER];
    assert!(marker.assets.is_empty());
    assert_eq!(marker.passengers[&passenger], 1);
    assert_eq!(marker.transport, Trucks::default());
    assert_eq!(marker.cargo.totals().unwrap(), Supplies::default());
    assert_eq!(marker.tank_fuel.get(), 0);
    assert_eq!(marker.activity_water.get(), 0);
}

/// Cases: land:22.23, land:22.5
#[test]
fn failed_truck_roll_creates_no_pool_consumes_no_supply_and_retains_exact_marker() {
    let (c, game) = truck_fixture(2, 3);
    let before_ids = game.state.logistics.truck_pool_ids.clone();
    let serials = game.state.logistics.truck_pool_serial.clone();
    let marker = serde_json::to_value(&game.state.land.breakdown.markers[MARKER]).unwrap();
    let mut game = truck_answer(&c, &opened(&c, &game), 2, "create_at_site", vec![]);
    let mut expected_rng = CampaignRng::from_state(&game.rng); expected_rng.d6();
    finish_step(&c, &mut game).unwrap();
    assert_eq!(game.rng, expected_rng.state());
    assert_eq!(game.state.logistics.truck_pool_ids, before_ids);
    assert_eq!(game.state.logistics.truck_pool_serial, serials);
    assert_eq!(serde_json::to_value(&game.state.land.breakdown.markers[MARKER]).unwrap(), marker);
    assert_eq!(game.state.logistics.dumps["ax_dump_1"].supplies.fuel, 20);
}

/// Cases: land:22.23, land:22.5, airlog:54.2
#[test]
fn actual_existing_pool_return_uses_whole_merged_capacity_and_rejects_overfull_target() {
    let (c, mut game) = truck_fixture(2, 1);
    let (_, stores) = caps(&c);
    game.state.land.breakdown.markers.get_mut(MARKER).unwrap().cargo.medium.stores = stores * 2;
    let id = pools::add_truck_pool(&mut game.state.logistics, Some("existing-return"), Side::Axis,
        Placement::Hex { hex: "C4807".into() }, Some(Location::Hex { hex: "C4807".into() }),
        Trucks { medium: 1, ..Default::default() }, Supplies { stores, ..Default::default() }).unwrap();
    let before_ids = game.state.logistics.truck_pool_ids.clone();
    let mut overfull: Game<Cna> = serde_json::from_value(serde_json::to_value(&game).unwrap()).unwrap();
    overfull.state.logistics.truck_pools.iter_mut().find(|p| p.id == id).unwrap().cargo.stores += 1;
    let overfull = opened(&c, &overfull);
    let before = serde_json::to_value(&overfull).unwrap();
    assert!(evaluate(&Cna::dev(), &c, &overfull, &command(&c, &overfull,
        json!([{"marker":MARKER,"points":2,"return_pool":id}]))).is_err());
    assert_eq!(serde_json::to_value(&overfull).unwrap(), before);
    let game = truck_answer(&c, &opened(&c, &game), 2, &id, vec![]);
    let result = evaluate(&Cna::dev(), &c, &game, &Command::Advance).unwrap();
    let pool = result.game.state.logistics.truck_pools.iter().find(|p| p.id == id).unwrap();
    assert_eq!(pool.trucks.medium, 3);
    assert_eq!(pool.cargo.stores, stores * 3);
    assert_eq!(result.game.state.logistics.truck_pool_ids, before_ids);
    assert!(!result.game.state.land.breakdown.markers.contains_key(MARKER));
}

/// Cases: land:22.23, land:22.5, airlog:53.25
#[test]
fn late_relocation_serial_failure_after_roll_pool_and_marker_writes_preserves_game_rng_events() {
    let (c, game) = cargo_fixture();
    let mut game = truck_answer(&c, &opened(&c, &game), 4, "create_at_site", priority(&c, true));
    game.state.logistics.cargo_history.next_id.insert(Side::Axis, u64::MAX);
    let before = serde_json::to_value(&game).unwrap();
    let mut rng = CampaignRng::from_state(&game.rng);
    let rng_before = rng.state();
    let mut events = vec![EngineEvent::new(Audience::Side(Side::Commonwealth), GameEvent::Note { text: "existing event".into() })];
    let previous = events.clone();
    assert!(matches!(Cna::full().finish_step(&c, &mut game.state, &mut Cx { rng: &mut rng, events: &mut events }), Err(EngineError::Invariant { .. })));
    assert_eq!(serde_json::to_value(&game).unwrap(), before);
    assert_eq!(rng.state(), rng_before);
    assert_eq!(events, previous);
}

/// Cases: land:22.23, land:22.5
#[test]
fn truck_answered_checkpoint_replays_identically_and_resolved_finish_returns_no_second_pool() {
    let (c, game) = cargo_fixture();
    let game = truck_answer(&c, &opened(&c, &game), 4, "create_at_site", priority(&c, true));
    let restored: Game<Cna> = serde_json::from_value(serde_json::to_value(&game).unwrap()).unwrap();
    let first = evaluate(&Cna::dev(), &c, &game, &Command::Advance).unwrap();
    let replay = evaluate(&Cna::dev(), &c, &restored, &Command::Advance).unwrap();
    assert_eq!(serde_json::to_value(&first.game).unwrap(), serde_json::to_value(&replay.game).unwrap());
    assert_eq!(first.events, replay.events);
    let mut same_step = game;
    finish_step(&c, &mut same_step).unwrap();
    let done = serde_json::to_value(&same_step).unwrap();
    assert!(finish_step(&c, &mut same_step).unwrap().is_empty());
    assert_eq!(serde_json::to_value(&same_step).unwrap(), done);
}

/// Cases: land:3.62, land:22.23, land:22.5, airlog:53.25
/// Interpretations: interp:land-0052
#[test]
fn actual_truck_observer_pairs_hide_different_private_histories_for_same_public_outcome() {
    let (c, mut left) = cargo_fixture();
    let former = left.state.land.units.values().find(|u| u.side == Side::Commonwealth).unwrap().id.clone();
    let old = left.state.land.breakdown.markers[MARKER].assets[0].unit.clone();
    let mut audit = left.state.logistics.fuel_accounts.remove(&old).unwrap();
    audit.side = Side::Commonwealth;
    left.state.logistics.fuel_accounts.insert(former.clone(), audit);
    let marker = left.state.land.breakdown.markers.get_mut(MARKER).unwrap();
    marker.assets[0].unit = former.clone();
    marker.fuel_cohorts[0].account = former;
    let mut right: Game<Cna> = serde_json::from_value(serde_json::to_value(&left).unwrap()).unwrap();
    right.state.logistics.cargo_history.histories.get_mut(&CargoSite::BrokenMarker(MARKER.into())).unwrap().lots[0].spent_cp_quarters = 20;
    let left = truck_answer(&c, &opened(&c, &left), 4, "create_at_site", priority(&c, true));
    let right = truck_answer(&c, &opened(&c, &right), 4, "create_at_site", priority(&c, true));
    let left = evaluate(&Cna::dev(), &c, &left, &Command::Advance).unwrap();
    let right = evaluate(&Cna::dev(), &c, &right, &Command::Advance).unwrap();
    for perspective in [Perspective::Side(Side::Commonwealth), Perspective::Seat(SeatId::new(Side::Commonwealth, Role::RearArea))] {
        let visible = |events: &[EngineEvent]| events.iter().filter(|e| e.visible_to(perspective)).cloned().collect::<Vec<_>>();
        assert_eq!(visible(&left.events), visible(&right.events));
        assert_eq!(Cna::dev().observe(&c, &left.game.state, perspective), Cna::dev().observe(&c, &right.game.state, perspective));
    }
    assert_ne!(serde_json::to_value(&left.game.state.logistics.cargo_history).unwrap(), serde_json::to_value(&right.game.state.logistics.cargo_history).unwrap());
}

/// Cases: land:22.23, land:22.5
#[test]
fn incomplete_priority_and_tracked_merge_refuse_without_changing_game_or_rng() {
    let (c, game) = cargo_fixture();
    let game = opened(&c, &game);
    let before = serde_json::to_value(&game).unwrap();
    let partial = vec![priority(&c, true).remove(0)];
    let action = json!([{"marker":MARKER,"points":4,"cargo_priority":partial}]);
    assert!(evaluate(&Cna::dev(), &c, &game, &command(&c, &game, action)).is_err());
    assert_eq!(serde_json::to_value(&game).unwrap(), before);
    let (c, mut tracked) = truck_fixture(2, 1);
    tracked.state.land.breakdown.markers.get_mut(MARKER).unwrap().assets[0].cohort = Some("recorded-cohort".into());
    let tracked = opened(&c, &tracked);
    let before = serde_json::to_value(&tracked).unwrap();
    assert!(matches!(evaluate(&Cna::dev(), &c, &tracked, &command(&c, &tracked,
        json!([{"marker":MARKER,"points":2}]))), Err(Rejection::Engine(EngineError::Unsupported { .. }))));
    assert_eq!(serde_json::to_value(&tracked).unwrap(), before);
    let (c, mut funded) = truck_fixture(2, 1);
    let id = pools::add_truck_pool(&mut funded.state.logistics, Some("funded-return"), Side::Axis,
        Placement::Hex { hex: "C4807".into() }, Some(Location::Hex { hex: "C4807".into() }),
        Trucks { medium: 1, ..Default::default() }, Supplies::default()).unwrap();
    let segment = crate::logistics::SegmentKey::current(&funded.state);
    funded.state.logistics.pool_fuel_accounts.insert(id.clone(), crate::logistics::FuelFundingAccount {
        side: Side::Axis, segment,
        origin: Location::Hex { hex: "C4807".into() }, paid_cost: FuelTenths::new(10), draws: vec![],
    });
    let funded = opened(&c, &funded);
    let before = serde_json::to_value(&funded).unwrap();
    assert!(matches!(evaluate(&Cna::dev(), &c, &funded, &command(&c, &funded,
        json!([{"marker":MARKER,"points":2,"return_pool":id}]))), Err(Rejection::Engine(EngineError::Unsupported { .. }))));
    assert_eq!(serde_json::to_value(&funded).unwrap(), before);
}

/// Cases: land:22.23, land:22.8
#[test]
fn actual_light_medium_heavy_groups_use_one_chart_draw_and_the_printed_budget() {
    for (equipment, roll, expected) in [
        (Equipment::LightTruck, 2, Trucks { light: 1, ..Default::default() }),
        (Equipment::MediumTruck, 1, Trucks { medium: 2, ..Default::default() }),
        (Equipment::HeavyTruck, 2, Trucks::default()),
        (Equipment::HeavyTruck, 1, Trucks { heavy: 1, ..Default::default() }),
    ] {
        let (c, mut game) = truck_fixture(2, roll);
        game.state.logistics.dumps.get_mut("ax_dump_1").unwrap().supplies.fuel = 0;
        game.state.land.breakdown.markers.get_mut(MARKER).unwrap().assets[0].equipment = equipment;
        let before = game.state.logistics.truck_pool_ids.clone();
        let mut game = truck_answer(&c, &opened(&c, &game), 2, "create_at_site", vec![]);
        let mut expected_rng = CampaignRng::from_state(&game.rng); expected_rng.d6();
        finish_step(&c, &mut game).unwrap();
        assert_eq!(game.rng, expected_rng.state());
        assert_eq!(game.state.logistics.dumps["ax_dump_1"].supplies.fuel, 0);
        if expected == Trucks::default() { assert_eq!(game.state.logistics.truck_pool_ids, before); }
        else {
            let pool = new_pool(&game, &before);
            assert_eq!(pool.trucks, expected);
            assert!(!game.state.logistics.cargo_history.motion.entries.iter().any(|e| e.site == CargoSite::Pool(pool.id.clone())));
        }
    }
}

/// Cases: land:22.23, land:22.5, airlog:53.25
#[test]
fn tracked_return_with_missing_motion_keeps_unknown_instead_of_initializing_zero_cp() {
    use crate::logistics::cargo_history::motion::{self, MotionError, PhysicalTrucks};
    let (c, mut game) = cargo_fixture();
    game.state.logistics.cargo_history.motion.entries.retain(|e| e.site != CargoSite::BrokenMarker(MARKER.into()));
    let before = game.state.logistics.truck_pool_ids.clone();
    let game = truck_answer(&c, &opened(&c, &game), 4, "create_at_site", priority(&c, true));
    let result = evaluate(&Cna::dev(), &c, &game, &Command::Advance).unwrap();
    let pool = new_pool(&result.game, &before);
    let site = CargoSite::Pool(pool.id.clone());
    assert!(!result.game.state.logistics.cargo_history.motion.entries.iter().any(|e| e.site == site));
    assert!(matches!(motion::query(&result.game.state, Side::Axis, &site, &[PhysicalTrucks {
        id: format!("pool.{}.fuel-trucks-2", pool.id), parent: None, kind: FuelTruckKind::Medium, count: 2,
    }]), Err(MotionError::Unknown)));
    assert!(!result.game.state.logistics.pool_fuel_accounts.contains_key(&pool.id));
    assert!(!result.game.state.logistics.pool_fuel_segments.contains_key(&pool.id));
}

/// Cases: land:22.23, land:22.5, land:3.6
#[test]
fn advertised_omitted_null_and_empty_priority_accept_identical_predice_plans_without_spend_or_rng() {
    let (c, game) = cargo_fixture();
    let game = opened(&c, &game);
    let request = Cna::dev().pending(&c, &game.state).into_iter().find(|r| r.kind == KIND).unwrap();
    let physical = serde_json::to_value((&game.state.land.units, &game.state.land.breakdown, &game.state.logistics)).unwrap();
    let mut canonical: Option<(Value, Vec<EngineEvent>)> = None;
    for action in [
        json!([{"marker": MARKER, "points": 4}]),
        json!([{"marker": MARKER, "points": 4, "cargo_priority": null}]),
        json!([{"marker": MARKER, "points": 4, "cargo_priority": []}]),
    ] {
        request.space.check(&action).unwrap();
        let accepted = evaluate(&Cna::dev(), &c, &game, &command(&c, &game, action)).unwrap();
        assert_eq!(accepted.game.rng, game.rng);
        assert_eq!(serde_json::to_value((&accepted.game.state.land.units,
            &accepted.game.state.land.breakdown, &accepted.game.state.logistics)).unwrap(), physical);
        let plan = accepted.game.state.land.recovery.repair_plan.as_ref().unwrap();
        assert_eq!(plan.markers[0].cargo_priority, priority(&c, true));
        assert!(matches!(plan.markers[0].return_pool, ReturnPoolTarget::CreateAtSite));
        let snapshot = serde_json::to_value(&accepted.game).unwrap();
        if let Some((expected_game, expected_events)) = &canonical {
            assert_eq!(&snapshot, expected_game);
            assert_eq!(&accepted.events, expected_events);
        } else {
            canonical = Some((snapshot, accepted.events));
        }
    }
}

/// Cases: land:22.5, airlog:49.13, airlog:49.16, airlog:53.25
#[test]
fn returned_pool_movement_requires_fresh_fuel_and_cannot_spend_original_unit_funding() {
    use crate::logistics::pool_fuel;
    let (c, game) = cargo_fixture();
    let source = game.state.land.breakdown.markers[MARKER].assets[0].unit.clone();
    assert_eq!(game.state.logistics.fuel_accounts[&source].paid_cost.get(), 30);
    let before_ids = game.state.logistics.truck_pool_ids.clone();
    let mut game = truck_answer(&c, &opened(&c, &game), 4, "create_at_site", priority(&c, true));
    finish_step(&c, &mut game).unwrap();
    let pool = new_pool(&game, &before_ids);
    let id = pool.id.clone();
    let site = CargoSite::Pool(id.clone());
    let before = serde_json::to_value(&game).unwrap();
    let plan = pool_fuel::plan_pool_segment_fuel(&c, &game.state, &id, 4).unwrap();
    assert!(plan.increment.get() > 0);
    assert!(plan.draws.iter().any(|draw| draw.amount.fuel.get() > 0));
    assert!(plan.ledger.cohorts.iter().all(|cohort| cohort.account == id));
    assert_eq!(serde_json::to_value(&game).unwrap(), before);
    assert!(!game.state.logistics.pool_fuel_accounts.contains_key(&id));
    assert!(!game.state.logistics.pool_fuel_segments.contains_key(&id));
    let motion = game.state.logistics.cargo_history.motion.entries.iter().find(|entry| entry.site == site).unwrap();
    assert_eq!(motion.cohorts[0].spent_cp_quarters, 16);

    // A consistent exhausted-stock world keeps all original paid funding facts.
    // Tagged fuel is retired from the pool together with its physical quantity.
    let mut empty: Game<Cna> = serde_json::from_value(before).unwrap();
    empty.state.logistics.truck_pools.iter_mut().find(|pool| pool.id == id).unwrap().cargo.fuel = 0;
    if let Some(history) = empty.state.logistics.cargo_history.histories.get_mut(&site) {
        for lot in &mut history.lots { lot.goods.fuel = 0; }
        history.lots.retain(|lot| lot.goods != Supplies::default());
    }
    for dump in empty.state.logistics.dumps.values_mut() { dump.supplies.fuel = 0; }
    for stock in empty.state.logistics.unit_supply.values_mut() { stock.carried.fuel = 0; }
    let origin = empty.state.logistics.truck_pools.iter().find(|pool| pool.id == id).unwrap().location.as_ref().unwrap();
    assert!(pool_fuel::available_pool_sources_at(&c, &empty.state, &id, origin).unwrap()
        .iter().all(|row| row.amount.fuel.get() == 0));
    assert_eq!(empty.state.logistics.fuel_accounts[&source].paid_cost.get(), 30);
    let empty_before = serde_json::to_value(&empty).unwrap();
    assert!(matches!(pool_fuel::plan_pool_segment_fuel(&c, &empty.state, &id, 4), Err(SupplyError::Insufficient)));
    assert_eq!(serde_json::to_value(&empty).unwrap(), empty_before);
}
