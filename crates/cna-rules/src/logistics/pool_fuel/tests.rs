use super::*;
use crate::state::{Dump, DumpLocation};
use cna_content::{
    scenario::{Placement, Supplies},
    units::Trucks,
};
use cna_core::quantity::FuelTenths;
use cna_protocol::Side;
use std::sync::OnceLock;
fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn game(trucks: Trucks, fuel: i32) -> (State, String) {
    let mut s = State::new(content()).unwrap();
    s.logistics.dumps.clear();
    s.logistics.unit_supply.clear();
    s.logistics.truck_pools.clear();
    // This fixture replaces the complete pool roster, including its creation histories.
    // The replacement pool below deliberately models legacy missing timing.
    s.logistics
        .cargo_history
        .motion
        .entries
        .retain(|entry| !matches!(entry.site, CargoSite::Pool(_)));
    s.cursor.op_stage = Some(1);
    let id = super::super::pools::add_truck_pool(
        &mut s.logistics,
        None,
        Side::Axis,
        Placement::Hex {
            hex: "C4020".into(),
        },
        Some(Location::Hex {
            hex: "C4020".into(),
        }),
        trucks,
        Supplies {
            fuel,
            ..Supplies::default()
        },
    )
    .unwrap();
    (s, id)
}
fn single() -> Trucks {
    Trucks {
        light: 1,
        ..Trucks::default()
    }
}

fn at_stage_start(s: &mut State, stage: u8) {
    s.cursor.block = crate::seq::Block::OpStage;
    s.cursor.index = crate::seq::OPSTAGE
        .iter()
        .position(|step| step.anchor == "opstage.weather")
        .unwrap();
    s.cursor.op_stage = Some(stage);
    s.cursor.half = None;
}

/// Cases: land:6.13, airlog:53.25
#[test]
fn authoritative_stage_seed_preserves_surviving_ids_and_duplicate_entry() {
    let (mut s, id) = game(
        Trucks {
            light: 2,
            medium: 3,
            heavy: 1,
        },
        100,
    );
    pool_mut_for_test(&mut s, &id).location = None;
    seed_created_pool(&mut s, &id).unwrap();
    retire_pool_truck_counts(
        &mut s,
        &id,
        Trucks {
            light: 2,
            ..Trucks::default()
        },
    )
    .unwrap();
    pool_mut_for_test(&mut s, &id).trucks.light = 0;
    let before = serde_json::to_value(&s.logistics).unwrap();
    at_stage_start(&mut s, 2);
    seed_pool_opstage(&mut s).unwrap();
    let site = CargoSite::Pool(id.clone());
    let entry = s
        .logistics
        .cargo_history
        .motion
        .entries
        .iter()
        .find(|e| e.site == site)
        .unwrap();
    assert_eq!(entry.stage, super::super::water::WaterStage::current(&s));
    assert_eq!(
        entry
            .cohorts
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        [
            format!("pool.{id}.fuel-trucks-2"),
            format!("pool.{id}.fuel-trucks-3")
        ]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
    );
    assert!(entry.cohorts.iter().all(|c| c.spent_cp_quarters == 0));
    let after = serde_json::to_value(&s.logistics).unwrap();
    seed_pool_opstage(&mut s).unwrap();
    assert_eq!(serde_json::to_value(&s.logistics).unwrap(), after);
    assert_eq!(after["pool_fuel_segments"], before["pool_fuel_segments"]);
    assert_eq!(after["pool_fuel_accounts"], before["pool_fuel_accounts"]);
    assert_eq!(after["truck_pools"], before["truck_pools"]);
}

/// Cases: land:6.13, airlog:53.25
#[test]
fn stage_seed_late_corruption_is_atomic_and_queries_never_initialize_legacy() {
    let (mut s, id) = game(single(), 100);
    let other = super::super::pools::add_truck_pool(
        &mut s.logistics,
        None,
        Side::Axis,
        Placement::Hex {
            hex: "C4020".into(),
        },
        None,
        single(),
        Supplies::default(),
    )
    .unwrap();
    at_stage_start(&mut s, 1);
    assert!(matches!(
        motion::query(
            &s,
            Side::Axis,
            &CargoSite::Pool(id.clone()),
            &fresh_pool_physical_cohorts(&s, &id).unwrap()
        ),
        Err(MotionError::Unknown)
    ));
    // The first pool's prepared zero history must not publish if a later pool is corrupt.
    s.logistics
        .truck_pools
        .iter_mut()
        .find(|p| p.id == other)
        .unwrap()
        .trucks
        .light = -1;
    let before = serde_json::to_value(&s).unwrap();
    assert!(seed_pool_opstage(&mut s).is_err());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert!(s.logistics.cargo_history.motion.entries.is_empty());
}

/// Cases: land:6.13, airlog:49.16, airlog:53.25
#[test]
fn stage_seed_resets_only_physical_cp_and_preserves_funding_stock_posture() {
    let (mut s, id) = game(single(), 100);
    seed_created_pool(&mut s, &id).unwrap();
    spend_pool_segment_fuel(content(), &mut s, &id, 8).unwrap();
    let physical = pool_segment_fuel_cohorts(&s, &id)
        .unwrap()
        .iter()
        .map(PhysicalTrucks::from)
        .collect::<Vec<_>>();
    motion::advance(
        &mut s,
        Side::Axis,
        &CargoSite::Pool(id.clone()),
        &physical,
        8,
    )
    .unwrap();
    s.land.movement.pool_on_road.insert(id.clone());
    let accounts = serde_json::to_value(&s.logistics.pool_fuel_accounts).unwrap();
    let fuel = serde_json::to_value(&s.logistics.pool_fuel_segments).unwrap();
    let stock_before = stock(&s, &id);
    at_stage_start(&mut s, 2);
    seed_pool_opstage(&mut s).unwrap();
    assert_eq!(
        motion::query(&s, Side::Axis, &CargoSite::Pool(id.clone()), &physical).unwrap()[0]
            .spent_cp_quarters,
        0
    );
    assert_eq!(
        serde_json::to_value(&s.logistics.pool_fuel_accounts).unwrap(),
        accounts
    );
    assert_eq!(
        serde_json::to_value(&s.logistics.pool_fuel_segments).unwrap(),
        fuel
    );
    assert_eq!(stock(&s, &id), stock_before);
    assert!(s.land.movement.pool_on_road.contains(&id));
    motion::advance(
        &mut s,
        Side::Axis,
        &CargoSite::Pool(id.clone()),
        &physical,
        4,
    )
    .unwrap();
    let before = serde_json::to_value(&s).unwrap();
    seed_pool_opstage(&mut s).unwrap();
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    s.cursor.block = crate::seq::Block::PlayerHalf;
    let before = serde_json::to_value(&s).unwrap();
    assert!(seed_pool_opstage(&mut s).is_err());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}

/// Cases: land:20.83, airlog:49.16, airlog:53.25
#[test]
fn withdrawal_wrapper_retires_paid_split_motion_once_and_rolls_back_bad_amount() {
    let (mut s, id) = game(
        Trucks {
            medium: 3,
            ..Trucks::default()
        },
        100,
    );
    seed_created_pool(&mut s, &id).unwrap();
    spend_pool_segment_fuel(content(), &mut s, &id, 8).unwrap();
    let physical = pool_segment_fuel_cohorts(&s, &id)
        .unwrap()
        .iter()
        .map(PhysicalTrucks::from)
        .collect::<Vec<_>>();
    motion::advance(
        &mut s,
        Side::Axis,
        &CargoSite::Pool(id.clone()),
        &physical,
        8,
    )
    .unwrap();
    let accounts = serde_json::to_value(&s.logistics.pool_fuel_accounts).unwrap();
    for extra_record in [false, true] {
        let mut corrupt = s.clone();
        let entry = corrupt
            .logistics
            .cargo_history
            .motion
            .entries
            .iter_mut()
            .find(|e| e.site == CargoSite::Pool(id.clone()))
            .unwrap();
        if extra_record {
            let mut extra = entry.cohorts[0].clone();
            extra.id = "extra-unbound-positive-motion".into();
            extra.count = 1;
            entry.cohorts.push(extra);
        } else {
            entry.cohorts[0].count = 4;
        }
        let before = serde_json::to_value(&corrupt).unwrap();
        assert!(matches!(
            retire_pool_truck_counts(
                &mut corrupt,
                &id,
                Trucks {
                    medium: 1,
                    ..Trucks::default()
                }
            ),
            Err(cna_core::engine::EngineError::Invariant { .. })
        ));
        assert_eq!(serde_json::to_value(&corrupt).unwrap(), before);
    }
    let mut corrupt = s.clone();
    corrupt
        .logistics
        .cargo_history
        .motion
        .entries
        .iter_mut()
        .find(|e| e.site == CargoSite::Pool(id.clone()))
        .unwrap()
        .cohorts[0]
        .id = "unbound-id".into();
    let corrupt_before = serde_json::to_value(&corrupt).unwrap();
    assert!(
        retire_pool_truck_counts(
            &mut corrupt,
            &id,
            Trucks {
                medium: 1,
                ..Trucks::default()
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&corrupt).unwrap(), corrupt_before);
    retire_pool_truck_counts(
        &mut s,
        &id,
        Trucks {
            medium: 1,
            ..Trucks::default()
        },
    )
    .unwrap();
    pool_mut_for_test(&mut s, &id).trucks.medium -= 1;
    let surviving = pool_segment_fuel_cohorts(&s, &id)
        .unwrap()
        .iter()
        .map(PhysicalTrucks::from)
        .collect::<Vec<_>>();
    let records = motion::query(&s, Side::Axis, &CargoSite::Pool(id.clone()), &surviving).unwrap();
    assert_eq!(records[0].count, 2);
    assert_eq!(records[0].spent_cp_quarters, 8);
    assert_eq!(
        serde_json::to_value(&s.logistics.pool_fuel_accounts).unwrap(),
        accounts
    );
    let before = serde_json::to_value(&s).unwrap();
    assert!(
        retire_pool_truck_counts(
            &mut s,
            &id,
            Trucks {
                medium: 3,
                ..Trucks::default()
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    let mut recovered: State = serde_json::from_value(before).unwrap();
    assert_eq!(
        pool_segment_fuel_cohorts(&recovered, &id).unwrap(),
        pool_segment_fuel_cohorts(&s, &id).unwrap()
    );
    retire_pool_truck_counts(
        &mut recovered,
        &id,
        Trucks {
            medium: 2,
            ..Trucks::default()
        },
    )
    .unwrap();
    assert!(
        recovered
            .logistics
            .cargo_history
            .motion
            .entries
            .iter()
            .find(|e| e.site == CargoSite::Pool(id.clone()))
            .unwrap()
            .cohorts
            .is_empty()
    );
}
fn stock(s: &State, id: &str) -> i32 {
    pool(s, id).unwrap().cargo.fuel
}
fn at(s: &mut State, id: &str, hex: &str) {
    s.logistics
        .truck_pools
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap()
        .location = Some(Location::Hex { hex: hex.into() });
}
/// Cases: airlog:49.13, land:21.25, land:21.29
#[test]
fn location_free_creation_matches_fuel_genesis_without_allocating_funding() {
    let (mut s, id) = game(
        Trucks {
            light: 2,
            medium: 3,
            heavy: 1,
        },
        100,
    );
    pool_mut_for_test(&mut s, &id).location = None;
    let before = serde_json::to_value(&s).unwrap();
    let physical = fresh_pool_physical_cohorts(&s, &id).unwrap();
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert_eq!(
        physical.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
        (1..=3)
            .map(|n| format!("pool.{id}.fuel-trucks-{n}"))
            .collect::<Vec<_>>()
    );
    seed_created_pool(&mut s, &id).unwrap();
    assert!(!s.logistics.pool_fuel_segments.contains_key(&id));
    assert!(!s.logistics.pool_fuel_accounts.contains_key(&id));
    let seeded = serde_json::to_value(&s).unwrap();
    assert!(matches!(
        seed_created_pool(&mut s, &id),
        Err(cna_core::engine::EngineError::Invariant { .. })
    ));
    assert_eq!(serde_json::to_value(&s).unwrap(), seeded);
    assert_eq!(
        fresh_pool_physical_cohorts(&s, &id),
        Err(SupplyError::Invalid)
    );
    at(&mut s, &id, "C4020");
    let fuel = pool_segment_fuel_cohorts(&s, &id).unwrap();
    assert_eq!(
        fuel.iter().map(PhysicalTrucks::from).collect::<Vec<_>>(),
        physical
    );
    assert!(fuel.iter().all(|c| c.cp_quarters == 0 && c.account == id));
    assert!(!s.logistics.pool_fuel_segments.contains_key(&id));
}
fn pool_mut_for_test<'a>(state: &'a mut State, id: &str) -> &'a mut TruckPool {
    state
        .logistics
        .truck_pools
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap()
}
/// Cases: land:20.83, airlog:49.13, airlog:53.25
#[test]
fn unfunded_partial_withdrawal_keeps_survivor_ids_and_reserves_retired_genesis_serials() {
    let (mut s, id) = game(
        Trucks {
            light: 2,
            medium: 3,
            heavy: 1,
        },
        100,
    );
    pool_mut_for_test(&mut s, &id).location = None;
    let physical = fresh_pool_physical_cohorts(&s, &id).unwrap();
    let site = CargoSite::Pool(id.clone());
    motion::seed_fresh(&mut s, Side::Axis, &site, &physical).unwrap();
    retire_unfunded_pool_motion(
        &mut s,
        &id,
        &[
            FuelCohortSelection {
                id: physical[0].id.clone(),
                count: 1,
            },
            FuelCohortSelection {
                id: physical[2].id.clone(),
                count: 1,
            },
        ],
    )
    .unwrap();
    pool_mut_for_test(&mut s, &id).trucks = Trucks {
        light: 1,
        medium: 3,
        heavy: 0,
    };
    let before = serde_json::to_value(&s).unwrap();
    let mut restored: State = serde_json::from_value(before.clone()).unwrap();
    at(&mut s, &id, "C4020");
    at(&mut restored, &id, "C4020");
    let l = ledger(&s, &id).unwrap();
    assert_eq!(l, ledger(&restored, &id).unwrap());
    assert_eq!(
        l.cohorts.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        vec![physical[0].id.as_str(), physical[1].id.as_str()]
    );
    assert_eq!(l.next_cohort_serial, 3);
    assert_eq!(l.paid_cost, FuelTenths::ZERO);
    assert!(l.draws.is_empty());
    spend_pool_segment_fuel(content(), &mut s, &id, 4).unwrap();
    let split = remove_selected_pool_fuel_cohorts(
        &mut s,
        &id,
        &[FuelCohortSelection {
            id: physical[1].id.clone(),
            count: 1,
        }],
    )
    .unwrap();
    assert_eq!(split[0].id, format!("pool.{id}.fuel-trucks-4"));
    assert_eq!(split[0].parent.as_deref(), Some(physical[1].id.as_str()));
}
/// Cases: land:20.83, airlog:49.13, airlog:53.25
#[test]
fn unfunded_withdrawal_preserves_unknown_and_rejects_positive_cp_or_overdraw_atomically() {
    let (mut s, id) = game(
        Trucks {
            light: 2,
            ..Trucks::default()
        },
        100,
    );
    let physical = fresh_pool_physical_cohorts(&s, &id).unwrap();
    let selection = [FuelCohortSelection {
        id: physical[0].id.clone(),
        count: 1,
    }];
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(
        retire_unfunded_pool_motion(&mut s, &id, &selection),
        Err(MotionError::Unknown)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    let site = CargoSite::Pool(id.clone());
    motion::seed_fresh(&mut s, Side::Axis, &site, &physical).unwrap();
    let before = serde_json::to_value(&s).unwrap();
    let overdraw = [FuelCohortSelection {
        id: physical[0].id.clone(),
        count: 3,
    }];
    assert_eq!(
        retire_unfunded_pool_motion(&mut s, &id, &overdraw),
        Err(MotionError::Invalid)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    let mut corrupt = s.clone();
    corrupt
        .logistics
        .cargo_history
        .motion
        .entries
        .iter_mut()
        .find(|e| e.site == site)
        .unwrap()
        .cohorts[0]
        .id = "invented-physical-id".into();
    let corrupt_before = serde_json::to_value(&corrupt).unwrap();
    assert_eq!(
        unfunded_pool_physical_cohorts(&corrupt, &id),
        Err(MotionError::Invalid)
    );
    assert_eq!(serde_json::to_value(&corrupt).unwrap(), corrupt_before);
    motion::advance(&mut s, Side::Axis, &site, &physical, 4).unwrap();
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(
        retire_unfunded_pool_motion(&mut s, &id, &selection),
        Err(MotionError::Invalid)
    );
    assert_eq!(
        pool_segment_fuel_cohorts(&s, &id),
        Err(SupplyError::Invalid)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}
/// Cases: airlog:49.13, land:21.25, land:21.29
#[test]
fn location_free_genesis_rejects_negative_counts_and_serial_overflow() {
    let key = SegmentKey {
        game_turn: 1,
        op_stage: Some(1),
        half: None,
        cycle: 0,
    };
    assert_eq!(
        segment::initial_cohorts(
            &single(),
            &"pool".to_owned(),
            "pool.pool",
            &key,
            0,
            u64::MAX
        ),
        Err(SupplyError::Invalid)
    );
    assert_eq!(
        segment::initial_cohorts(
            &Trucks {
                light: -1,
                ..Trucks::default()
            },
            &"pool".to_owned(),
            "pool.pool",
            &key,
            0,
            0
        ),
        Err(SupplyError::Invalid)
    );
}
/// Cases: airlog:49.13, airlog:49.18, airlog:53.22
/// Interpretations: interp:airlog-0001, interp:airlog-0018
#[test]
fn own_cargo_follows_mixed_convoy_and_incremental_fractional_prices_match_whole_path() {
    let (mut incremental, id) = game(
        Trucks {
            light: 1,
            medium: 1,
            heavy: 1,
        },
        100,
    );
    let mut whole = incremental.clone();
    let units = incremental.logistics.fuel_segments.clone();
    let unit_accounts = incremental.logistics.fuel_accounts.clone();
    spend_pool_segment_fuel(content(), &mut whole, &id, 160).unwrap();
    for cp in 1..=160 {
        at(
            &mut incremental,
            &id,
            if cp % 2 == 0 { "C4021" } else { "C4020" },
        );
        let before = serde_json::to_value(&incremental).unwrap();
        let plan = plan_pool_segment_fuel(content(), &incremental, &id, cp).unwrap();
        assert_eq!(serde_json::to_value(&incremental).unwrap(), before);
        spend_pool_segment_fuel(content(), &mut incremental, &id, cp).unwrap();
        assert_eq!(incremental.logistics.pool_fuel_segments[&id], plan.ledger);
    }
    assert_eq!(stock(&whole, &id), 76);
    assert_eq!(stock(&incremental, &id), 76);
    assert_eq!(
        incremental.logistics.pool_fuel_segments,
        whole.logistics.pool_fuel_segments
    );
    assert_eq!(
        incremental.logistics.pool_fuel_accounts,
        whole.logistics.pool_fuel_accounts
    );
    assert_eq!(incremental.logistics.fuel_segments, units);
    assert_eq!(incremental.logistics.fuel_accounts, unit_accounts);
}

/// Cases: airlog:49.13, airlog:49.18
#[test]
fn tank_tenths_and_whole_cargo_credit_are_distinct_and_zero_increment_skips_stocks() {
    let (mut s, id) = game(single(), 10);
    s.logistics.truck_pools[0].tank_fuel = FuelTenths::new(3);
    spend_pool_segment_fuel(content(), &mut s, &id, 4).unwrap();
    assert_eq!(pool(&s, &id).unwrap().tank_fuel.get(), 1);
    assert_eq!(stock(&s, &id), 10);
    spend_pool_segment_fuel(content(), &mut s, &id, 8).unwrap();
    assert_eq!(pool(&s, &id).unwrap().tank_fuel.get(), 0);
    assert_eq!(stock(&s, &id), 9);
    spend_pool_segment_fuel(content(), &mut s, &id, 12).unwrap();
    assert_eq!(stock(&s, &id), 9);
    // Same rounded whole CP: no source conversion, even with invalid unused cargo.
    s.logistics.truck_pools[0].cargo.fuel = i32::MAX;
    spend_pool_segment_fuel(content(), &mut s, &id, 12).unwrap();
    assert_eq!(stock(&s, &id), i32::MAX);
    s.logistics.pool_fuel_segments.get_mut(&id).unwrap().draws[0].fuel = FuelTenths::new(-1);
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(
        spend_pool_segment_fuel(content(), &mut s, &id, 12),
        Err(SupplyError::Invalid)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}

/// Cases: airlog:49.15, airlog:49.16, airlog:49.18
#[test]
fn other_pools_must_unload_and_failed_spend_is_atomic() {
    let (mut s, id) = game(single(), 0);
    let other = super::super::pools::add_truck_pool(
        &mut s.logistics,
        None,
        Side::Axis,
        Placement::Hex {
            hex: "C4020".into(),
        },
        Some(Location::Hex {
            hex: "C4020".into(),
        }),
        single(),
        Supplies {
            fuel: 100,
            ..Supplies::default()
        },
    )
    .unwrap();
    let sources = available_pool_sources_at(
        content(),
        &s,
        &id,
        &Location::Hex {
            hex: "C4020".into(),
        },
    )
    .unwrap();
    assert!(!sources.iter().any(|d| matches!(&d.source,
        SupplySource::PoolStock(p)|SupplySource::PoolTank(p) if p==&other)));
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(
        spend_pool_segment_fuel(content(), &mut s, &id, 4),
        Err(SupplyError::Insufficient)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert_eq!(
        spend_pool_segment_fuel(content(), &mut s, &id, -1),
        Err(SupplyError::Invalid)
    );
    assert_eq!(
        spend_pool_segment_fuel(content(), &mut s, "not-a-pool", 4),
        Err(SupplyError::Invalid)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}
fn dump(s: &mut State, id: &str, hex: &str, fuel: i32) {
    s.logistics.dumps.insert(
        id.into(),
        Dump {
            id: id.into(),
            marker: format!("marker-{id}"),
            side: Side::Axis,
            location: DumpLocation::Hex { hex: hex.into() },
            supplies: Supplies {
                fuel,
                ..Supplies::default()
            },
            active: true,
            dummy: false,
        },
    );
}
/// Cases: airlog:49.16, airlog:49.18
#[test]
fn retained_origin_credit_survives_movement_and_checkpoint_without_new_destination_rights() {
    let (mut s, id) = game(single(), 0);
    dump(&mut s, "origin", "C4020", 1);
    dump(&mut s, "destination", "C4024", 100);
    spend_pool_segment_fuel(content(), &mut s, &id, 4).unwrap();
    at(&mut s, &id, "C4024");
    let mut recovered: State = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    spend_pool_segment_fuel(content(), &mut s, &id, 20).unwrap();
    spend_pool_segment_fuel(content(), &mut recovered, &id, 20).unwrap();
    assert_eq!(
        serde_json::to_value(&s).unwrap(),
        serde_json::to_value(&recovered).unwrap()
    );
    assert_eq!(s.logistics.dumps["origin"].supplies.fuel, 0);
    assert_eq!(s.logistics.dumps["destination"].supplies.fuel, 100);
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(
        spend_pool_segment_fuel(content(), &mut s, &id, 24),
        Err(SupplyError::Insufficient)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}

/// Cases: land:21.25, land:21.29, airlog:49.13, airlog:49.16
#[test]
fn broken_cohorts_retain_paid_history_and_restore_without_reset_or_double_rounding() {
    let (mut s, id) = game(
        Trucks {
            light: 2,
            ..Trucks::default()
        },
        10,
    );
    spend_pool_segment_fuel(content(), &mut s, &id, 16).unwrap();
    let original = pool_segment_fuel_cohorts(&s, &id).unwrap()[0].clone();
    let removed = remove_selected_pool_fuel_cohorts(
        &mut s,
        &id,
        &[FuelCohortSelection {
            id: original.id.clone(),
            count: 1,
        }],
    )
    .unwrap();
    assert_eq!(removed[0].parent.as_ref(), Some(&original.id));
    assert_eq!(removed[0].cp_quarters, 16);
    s.logistics.truck_pools[0].trucks.light -= 1;
    spend_pool_segment_fuel(content(), &mut s, &id, 20).unwrap();
    assert_eq!(stock(&s, &id), 8);
    restore_pool_fuel_cohorts(&mut s, &id, &removed).unwrap();
    s.logistics.truck_pools[0].trucks.light += 1;
    spend_pool_segment_fuel(content(), &mut s, &id, 24).unwrap();
    assert_eq!(s.logistics.pool_fuel_accounts[&id].paid_cost.get(), 30);
    assert_eq!(stock(&s, &id), 7);
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(
        restore_pool_fuel_cohorts(&mut s, &id, &removed),
        Err(SupplyError::Invalid)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    // Full retirement also permits recovery: zero surviving trucks is real state.
    let selections = pool_segment_fuel_cohorts(&s, &id)
        .unwrap()
        .into_iter()
        .map(|c| FuelCohortSelection {
            id: c.id,
            count: c.count,
        })
        .collect::<Vec<_>>();
    let removed = remove_selected_pool_fuel_cohorts(&mut s, &id, &selections).unwrap();
    s.logistics.truck_pools[0].trucks.light = 0;
    restore_pool_fuel_cohorts(&mut s, &id, &removed).unwrap();
    s.logistics.truck_pools[0].trucks.light = 2;
    assert_eq!(pool_segment_fuel_cohorts(&s, &id).unwrap().len(), 2);
}

/// Cases: land:3.6, airlog:49.18, airlog:49.16
#[test]
fn pool_identity_cannot_pollute_unit_accounts_or_enemy_views_and_old_checkpoints_default() {
    let (mut a, _) = game(single(), 10);
    let actual = a.land.units.keys().next().unwrap().clone();
    // Even text identical to a real UnitId remains in the pool-keyed domain.
    a.logistics.truck_pools[0].id = actual.to_string();
    let id = actual.to_string();
    spend_pool_segment_fuel(content(), &mut a, &id, 4).unwrap();
    assert!(a.logistics.fuel_accounts.is_empty());
    assert!(a.logistics.fuel_segments.is_empty());
    assert!(
        a.logistics.pool_fuel_segments[&id].cohorts[0]
            .id
            .starts_with("pool.")
    );
    let mut b = a.clone();
    b.logistics.truck_pools[0].tank_fuel = FuelTenths::new(73);
    b.logistics.truck_pools[0].activity_water = cna_core::quantity::WaterPoints::new(9);
    b.logistics.truck_pools[0].cargo.fuel = 99;
    crate::testkit::assert_indistinguishable(
        &crate::Cna::dev(),
        content(),
        &a,
        &b,
        Side::Commonwealth,
    );
    let mut value = serde_json::to_value(&a).unwrap();
    value["logistics"]
        .as_object_mut()
        .unwrap()
        .remove("pool_fuel_segments");
    value["logistics"]
        .as_object_mut()
        .unwrap()
        .remove("pool_fuel_accounts");
    for p in value["logistics"]["truck_pools"].as_array_mut().unwrap() {
        p.as_object_mut().unwrap().remove("tank_fuel");
        p.as_object_mut().unwrap().remove("activity_water");
    }
    let legacy: State = serde_json::from_value(value).unwrap();
    assert!(legacy.logistics.pool_fuel_accounts.is_empty());
    assert!(legacy.logistics.pool_fuel_segments.is_empty());
    assert!(legacy.logistics.truck_pools[0].tank_fuel.is_zero());
    assert!(legacy.logistics.truck_pools[0].activity_water.is_zero());
}

/// Cases: airlog:49.16, airlog:57.0, land:8.84
#[test]
fn scenario_sources_are_resolved_and_transit_never_certifies_a_dump_or_departed_box() {
    let (mut s, id) = game(single(), 10);
    s.logistics.truck_pools[0].side = Side::Commonwealth;
    at(&mut s, &id, "E1730");
    let cairo = available_pool_sources_at(
        content(),
        &s,
        &id,
        &Location::Hex {
            hex: "E1730".into(),
        },
    )
    .unwrap();
    let unlimited = cairo
        .iter()
        .find(|d| d.source == SupplySource::Unlimited)
        .unwrap();
    assert!(unlimited.amount.fuel.get() > 0);
    assert!(unlimited.amount.water.is_zero());
    s.logistics.truck_pools[0].side = Side::Axis;
    let axis = available_pool_sources_at(
        content(),
        &s,
        &id,
        &Location::Hex {
            hex: "E1730".into(),
        },
    )
    .unwrap();
    assert!(axis.iter().all(|d| d.source != SupplySource::Unlimited));
    let group = Location::OffMap {
        id: "land-transit:axis:1".into(),
    };
    s.logistics.truck_pools[0].location = Some(group.clone());
    s.logistics.dumps.insert(
        "not-a-transit-dump".into(),
        Dump {
            id: "not-a-transit-dump".into(),
            marker: "dump-9".into(),
            side: Side::Axis,
            location: DumpLocation::OffMap {
                id: "land-transit:axis:1".into(),
            },
            supplies: Supplies {
                fuel: 100,
                ..Supplies::default()
            },
            active: true,
            dummy: false,
        },
    );
    let transit = available_pool_sources_at(content(), &s, &id, &group).unwrap();
    assert!(
        transit
            .iter()
            .all(|d| !matches!(d.source, SupplySource::Dump(_) | SupplySource::Unlimited))
    );
    spend_pool_segment_fuel(content(), &mut s, &id, 4).unwrap();
    assert_eq!(stock(&s, &id), 9);
    assert_eq!(s.logistics.dumps["not-a-transit-dump"].supplies.fuel, 100);
}

/// Cases: land:21.25, land:21.29, airlog:49.16
#[test]
fn recovery_in_another_segment_resets_charge_but_preserves_physical_identity() {
    let (mut s, id) = game(single(), 10);
    spend_pool_segment_fuel(content(), &mut s, &id, 16).unwrap();
    let c = pool_segment_fuel_cohorts(&s, &id).unwrap()[0].clone();
    let removed = remove_selected_pool_fuel_cohorts(
        &mut s,
        &id,
        &[FuelCohortSelection {
            id: c.id.clone(),
            count: 1,
        }],
    )
    .unwrap();
    s.logistics.truck_pools[0].trucks.light = 0;
    s.cursor.cycle += 1;
    restore_pool_fuel_cohorts(&mut s, &id, &removed).unwrap();
    s.logistics.truck_pools[0].trucks.light = 1;
    let restored = pool_segment_fuel_cohorts(&s, &id).unwrap();
    assert_eq!(restored[0].id, c.id);
    assert_eq!(restored[0].cp_quarters, 0);
    spend_pool_segment_fuel(content(), &mut s, &id, 4).unwrap();
    assert_eq!(stock(&s, &id), 8);
    assert_eq!(s.logistics.pool_fuel_accounts[&id].paid_cost.get(), 2);
}
