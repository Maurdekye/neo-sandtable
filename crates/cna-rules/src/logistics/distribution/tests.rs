use super::*;
use crate::seq::{Block, OPSTAGE};
use cna_core::visibility::Perspective;
use cna_core::{decision::Secrecy, visibility::Audience};
fn setup() -> (CnaContent, State, UnitId, Endpoint) {
    let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut state = State::new(&content).unwrap();
    state.cursor.block = Block::OpStage;
    state.cursor.index = OPSTAGE
        .iter()
        .position(|s| s.anchor == "opstage.organization.supply_distribution")
        .unwrap();
    let id: UnitId = "it.1_libyan_div.viii_libyan_bn".into();
    let at = Location::Hex {
        hex: "C4020".into(),
    };
    let u = state.land.units.get_mut(&id).unwrap();
    u.location = at.clone();
    u.trucks = Trucks {
        heavy: 1,
        ..Trucks::default()
    };
    u.transport_trucks = Trucks::default();
    // Component stock/placement fixture, actual initial physical counts retained.
    let source_pool = state
        .logistics
        .truck_pools
        .iter_mut()
        .find(|p| p.side == Side::Axis)
        .unwrap();
    source_pool.location = Some(at);
    source_pool.cargo = Supplies {
        fuel: 250,
        stores: 20,
        ..Default::default()
    };
    let pool = source_pool.id.clone();
    (content, state, id, Endpoint::Pool(pool))
}
/// Cases: airlog:49.14, airlog:49.16, airlog:50.15, airlog:53.24, airlog:54.11, land:3.6
/// Interpretations: interp:airlog-0001, interp:airlog-0008
#[test]
fn untagged_unit_creates_dump_then_reloads_and_refills_fractional_tank() {
    let (c, mut s, id, pool) = setup();
    let ground = Endpoint::Ground(location(&s, Side::Axis, &pool).unwrap());
    let a = Supplies {
        fuel: 100,
        ..Supplies::default()
    };
    // Component untagged first-line stock preserves new-dump/debit/reload/tank
    // coverage; Pool->Ground now has a separate Unsupported/purity witness.
    s.logistics
        .unit_supply
        .entry(id.clone())
        .or_default()
        .carried
        .fuel = 100;
    assert!(
        transfer(
            &c,
            &mut s,
            Side::Axis,
            &pool,
            &Endpoint::Cargo(id.clone()),
            a,
            &CargoPacking {
                heavy: a,
                ..Default::default()
            }
        )
        .is_err()
    );
    transfer(
        &c,
        &mut s,
        Side::Axis,
        &Endpoint::Cargo(id.clone()),
        &ground,
        a,
        &CargoPacking::default(),
    )
    .unwrap();
    assert_eq!(stock(&s, &pool).unwrap().fuel, 250);
    assert_eq!(s.logistics.unit_supply[&id].carried.fuel, 0);
    let dump = Endpoint::Dump("axis.dump-1".into());
    let cargo = Supplies {
        fuel: 50,
        ..Supplies::default()
    };
    transfer(
        &c,
        &mut s,
        Side::Axis,
        &dump,
        &Endpoint::Cargo(id.clone()),
        cargo,
        &CargoPacking {
            heavy: cargo,
            ..Default::default()
        },
    )
    .unwrap();
    transfer(
        &c,
        &mut s,
        Side::Axis,
        &Endpoint::Cargo(id.clone()),
        &Endpoint::Tank(id.clone()),
        Supplies {
            fuel: 19,
            ..Default::default()
        },
        &CargoPacking::default(),
    )
    .unwrap();
    assert_eq!(s.logistics.unit_supply[&id].tank_fuel.get(), 19);
    assert_eq!(s.logistics.unit_supply[&id].carried.fuel, 48);
    let enemy = crate::view::inspect(
        &c,
        &s,
        Perspective::Side(Side::Commonwealth),
        "axis.dump-1",
        false,
    )
    .unwrap_err();
    assert!(format!("{enemy:?}").contains("unknown or not visible"));
}
/// Cases: airlog:53.24, airlog:54.2
/// Interpretations: interp:airlog-0008
#[test]
fn bad_packing_enemy_and_other_location_reject_atomically() {
    let (c, mut s, id, pool) = setup();
    let at = location(&s, Side::Axis, &pool).unwrap();
    s.logistics
        .unit_supply
        .entry(id.clone())
        .or_default()
        .carried
        .stores = 20;
    transfer(
        &c,
        &mut s,
        Side::Axis,
        &Endpoint::Cargo(id.clone()),
        &Endpoint::Ground(at),
        Supplies {
            stores: 20,
            ..Default::default()
        },
        &CargoPacking::default(),
    )
    .unwrap();
    let before = serde_json::to_value(&s).unwrap();
    let from = Endpoint::Dump("axis.dump-1".into());
    let to = Endpoint::Cargo(id.clone());
    for (side, amount, packing) in [
        (
            Side::Commonwealth,
            Supplies {
                stores: 1,
                ..Default::default()
            },
            CargoPacking::default(),
        ),
        (
            Side::Axis,
            Supplies {
                stores: 21,
                ..Default::default()
            },
            CargoPacking {
                heavy: Supplies {
                    stores: 21,
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
        (
            Side::Axis,
            Supplies {
                stores: 1,
                ..Default::default()
            },
            CargoPacking::default(),
        ),
    ] {
        assert!(transfer(&c, &mut s, side, &from, &to, amount, &packing).is_err());
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
    }
    s.land.units.get_mut(&id).unwrap().location = Location::Hex {
        hex: "C4021".into(),
    };
    let before = serde_json::to_value(&s).unwrap();
    assert!(
        transfer(
            &c,
            &mut s,
            Side::Axis,
            &from,
            &to,
            Supplies {
                stores: 1,
                ..Default::default()
            },
            &CargoPacking::default()
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}
/// Cases: airlog:54.11, airlog:54.13
#[test]
fn extra_dump_counters_cannot_multiply_hex_capacity() {
    let (c, mut s, id, _) = setup();
    let at = s.land.units[&id].location.clone();
    for n in 1..=2 {
        let id = format!("test-{n}");
        s.logistics.dumps.insert(
            id.clone(),
            crate::state::Dump {
                marker: String::new(),
                id,
                side: Side::Axis,
                location: DumpLocation::Hex {
                    hex: at.hex().unwrap().clone(),
                },
                supplies: Supplies {
                    water: 500,
                    ..Default::default()
                },
                active: true,
                dummy: false,
            },
        );
    }
    assert_eq!(
        validate_dump_capacity(
            &c,
            &s,
            Side::Axis,
            &at,
            &Supplies {
                water: 1,
                ..Default::default()
            }
        ),
        Err(SupplyError::Unsupported {
            case: "airlog:54.13"
        })
    );
    transfer(
        &c,
        &mut s,
        Side::Axis,
        &Endpoint::Dump("test-1".into()),
        &Endpoint::Dump("test-2".into()),
        Supplies {
            water: 500,
            ..Default::default()
        },
        &CargoPacking::default(),
    )
    .unwrap();
    assert_eq!(s.logistics.dumps["test-2"].supplies.water, 1000);
    assert_eq!(s.logistics.dumps["test-1"].supplies.water, 0);
}
/// Cases: airlog:53.24
#[test]
fn loading_outside_distribution_cannot_bypass_capability_cost_or_leapfrogging() {
    let (c, mut s, _, pool) = setup();
    let at = location(&s, Side::Axis, &pool).unwrap();
    s.cursor.index += 1;
    let before = serde_json::to_value(&s).unwrap();
    assert!(
        transfer(
            &c,
            &mut s,
            Side::Axis,
            &pool,
            &Endpoint::Ground(at),
            Supplies {
                stores: 1,
                ..Default::default()
            },
            &CargoPacking::default()
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}

/// Cases: airlog:53.24, land:3.6
#[test]
fn dispatch_opens_owner_secret_transfer_and_commits_one_transaction() {
    use cna_core::dice::CampaignRng;
    let (c, mut s, id, pool) = setup();
    s.logistics
        .unit_supply
        .entry(id.clone())
        .or_default()
        .carried
        .stores = 5;
    let at = location(&s, Side::Axis, &pool).unwrap();
    let mut rng = CampaignRng::from_seed([3; 32]);
    let mut events = vec![];
    let mut cx = Cx {
        rng: &mut rng,
        events: &mut events,
    };
    enter(&c, &mut s, &mut cx).unwrap();
    let pos = s
        .decisions
        .pending
        .iter()
        .position(|p| p.seat.side == Side::Axis)
        .unwrap();
    let pending = s.decisions.pending.remove(pos);
    assert_eq!(pending.kind, super::super::batches::DISTRIBUTION);
    assert_eq!(pending.secrecy, Secrecy::SecretSimultaneous);
    let receiver = serde_json::to_string(&Endpoint::Ground(at)).unwrap();
    let request = serde_json::json!([{"from":serde_json::to_string(&Endpoint::Cargo(id.clone())).unwrap(),
        "to":receiver,"amount":{"ammo":0,"fuel":0,"stores":5,"water":0},
        "packing":CargoPacking::default()}]);
    crate::Cna::dev()
        .respond_to(&c, &mut s, &pending, &request, &mut cx)
        .unwrap();
    assert!(
        s.decisions
            .pending
            .iter()
            .all(|p| p.seat.side != Side::Axis)
    );
    assert!(!s.logistics.dumps.contains_key("axis.dump-1"));
    let other = s.decisions.pending.remove(0);
    crate::Cna::dev()
        .respond_to(&c, &mut s, &other, &serde_json::Value::Null, &mut cx)
        .unwrap();
    super::super::batches::finish_distribution(&c, &mut s, &mut cx).unwrap();
    assert_eq!(s.logistics.dumps["axis.dump-1"].supplies.stores, 5);
    assert_eq!(s.logistics.unit_supply[&id].carried.stores, 0);
    assert_eq!(stock(&s, &pool).unwrap().stores, 20);
    assert!(events.iter().all(|e| e.audience != Audience::Public));
    assert!(
        crate::view::observe(&c, &s, Perspective::Side(Side::Commonwealth))["logistics"]["dumps"]
            .get("axis.dump-1")
            .is_none()
    );
    let _ = id;
}

// Component fixtures retain source-created physical inventory; stock/location
// edits below are explicit component inputs, not authored setup witnesses.
fn history_fixture() -> (CnaContent, State, UnitId, Endpoint, Endpoint) {
    let (c, mut s, unit, pool) = setup();
    let dump = "component.history.dump".to_string();
    s.logistics.dumps.insert(
        dump.clone(),
        crate::state::Dump {
            id: dump.clone(),
            marker: String::new(),
            side: Side::Axis,
            location: DumpLocation::Hex {
                hex: "C4020".into(),
            },
            supplies: Supplies {
                fuel: 100,
                ammo: 10,
                stores: 20,
                water: 0,
            },
            active: true,
            dummy: false,
        },
    );
    (c, s, unit, pool, Endpoint::Dump(dump))
}
fn history_tag(
    s: &mut State,
    site: super::super::cargo_history::CargoSite,
    goods: Supplies,
    spent: i32,
    ceiling: i32,
) {
    use super::super::cargo_history::{CargoHistory, CargoLot};
    s.logistics.cargo_history.histories.insert(
        site,
        CargoHistory {
            stage: super::super::water::WaterStage::current(s),
            lots: vec![CargoLot {
                id: "component.tag".into(),
                goods,
                spent_cp_quarters: spent,
                ceiling_cp_quarters: ceiling,
                continuous_first_line: false,
            }],
        },
    );
}
fn final_pool_packing(c: &CnaContent, s: &State, pool: &str, amount: Supplies) -> CargoPacking {
    let p = s
        .logistics
        .truck_pools
        .iter()
        .find(|p| p.id == pool)
        .unwrap();
    let target = Supplies {
        ammo: p.cargo.ammo + amount.ammo,
        fuel: p.cargo.fuel + amount.fuel,
        stores: p.cargo.stores + amount.stores,
        water: p.cargo.water + amount.water,
    };
    capacity::find_packing(c, &p.trucks, &Trucks::default(), target)
        .expect("component final cargo must have a physical packing")
}
fn history_error_pure(
    c: &CnaContent,
    s: &mut State,
    from: &Endpoint,
    to: &Endpoint,
    amount: Supplies,
    packing: &CargoPacking,
) -> DistributionError {
    let before = serde_json::to_value(&s).unwrap();
    let error = transfer(c, s, Side::Axis, from, to, amount, packing).unwrap_err();
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    error
}
/// Cases: airlog:53.24, airlog:53.25
/// Interpretations: interp:airlog-0020
#[test]
fn uniform_pool_dump_roundtrip_preserves_goods_first_ceiling_and_serials() {
    use super::super::cargo_history::{self, CargoSite};
    let (c, mut s, _, pool, dump) = history_fixture();
    let Endpoint::Pool(id) = &pool else { panic!() };
    let Endpoint::Dump(dump_id) = &dump else {
        panic!()
    };
    let before = serde_json::to_value(&s.logistics).unwrap();
    let amount = Supplies {
        stores: 5,
        ..Default::default()
    };
    transfer(
        &c,
        &mut s,
        Side::Axis,
        &pool,
        &dump,
        amount,
        &CargoPacking::default(),
    )
    .unwrap();
    assert_eq!(stock(&s, &pool).unwrap().stores, 15);
    assert_eq!(stock(&s, &dump).unwrap().stores, 25);
    let tags = cargo_history::parcels(&s, Side::Axis, &CargoSite::Dump(dump_id.clone())).unwrap();
    let tagged = tags
        .iter()
        .find(|p| p.ceiling_cp_quarters.is_some())
        .unwrap();
    assert_eq!(tagged.goods, amount);
    assert_eq!(tagged.spent_cp_quarters, 0);
    assert_eq!(
        tagged.ceiling_cp_quarters,
        Some(pool_timing(&c, &s, Side::Axis, id).unwrap().cpa_quarters)
    );
    // Receiving a dump containing fresh and tagged stores needs choices even
    // when CP agrees: first-carrier ceilings differ. No silent None selection.
    let packing = final_pool_packing(&c, &s, id, amount);
    let error = history_error_pure(&c, &mut s, &dump, &pool, amount, &packing);
    assert_eq!(
        error,
        DistributionError::Cargo(DistributionCargoError::History(
            cargo_history::CargoError::ChoiceRequired
        ))
    );
    assert!(
        matches!(rejection(error),cna_core::engine::Rejection::Engine(EngineError::Unsupported { case,.. }) if case=="airlog:53.25")
    );
    // Separate uniform destination fixture: no fresh stores remainder.
    let (c, mut s, _, pool, dump) = history_fixture();
    let Endpoint::Pool(id) = &pool else { panic!() };
    let Endpoint::Dump(dump_id) = &dump else {
        panic!()
    };
    s.logistics.dumps.get_mut(dump_id).unwrap().supplies = Supplies::default();
    let initial = stock(&s, &pool).unwrap();
    transfer(
        &c,
        &mut s,
        Side::Axis,
        &pool,
        &dump,
        amount,
        &CargoPacking::default(),
    )
    .unwrap();
    let serial = s.logistics.cargo_history.next_id[&Side::Axis];
    let lot =
        s.logistics.cargo_history.histories[&CargoSite::Dump(dump_id.clone())].lots[0].clone();
    let packing = final_pool_packing(&c, &s, id, amount);
    transfer(&c, &mut s, Side::Axis, &dump, &pool, amount, &packing).unwrap();
    assert_eq!(stock(&s, &pool).unwrap(), initial);
    assert_eq!(stock(&s, &dump).unwrap(), Supplies::default());
    assert_eq!(s.logistics.cargo_history.next_id[&Side::Axis], serial);
    assert_eq!(
        s.logistics.cargo_history.histories[&CargoSite::Pool(id.clone())].lots[0],
        lot
    );
    assert_eq!(
        before["pool_fuel_segments"],
        serde_json::to_value(&s.logistics).unwrap()["pool_fuel_segments"]
    );
}
/// Cases: airlog:53.24, airlog:53.25, land:3.6
#[test]
fn excluded_routes_validate_first_and_block_current_tags_at_both_endpoints() {
    use super::super::cargo_history::CargoSite;
    let (c, s, unit, pool, dump) = history_fixture();
    let Endpoint::Dump(dump_id) = &dump else {
        panic!()
    };
    let ground = Endpoint::Ground(Location::Hex {
        hex: "C4020".into(),
    });
    let amount = Supplies {
        stores: 1,
        ..Default::default()
    };
    let mut state = s.clone();
    let error = history_error_pure(
        &c,
        &mut state,
        &pool,
        &ground,
        amount,
        &CargoPacking::default(),
    );
    assert!(matches!(
        error,
        DistributionError::Cargo(DistributionCargoError::UnintegratedRoute(_))
    ));
    // Original generic invalid-input order remains authoritative.
    let invalid = history_error_pure(
        &c,
        &mut state,
        &pool,
        &ground,
        Supplies {
            stores: 1_000_000,
            ..Default::default()
        },
        &CargoPacking::default(),
    );
    assert!(matches!(invalid, DistributionError::Supply(_)));
    let other = "component.history.other".to_string();
    let mut state = s.clone();
    let mut d = state.logistics.dumps[dump_id].clone();
    d.id = other.clone();
    state.logistics.dumps.insert(other.clone(), d);
    let routes = [
        (
            dump.clone(),
            Endpoint::Dump(other.clone()),
            CargoPacking::default(),
        ),
        (dump.clone(), ground.clone(), CargoPacking::default()),
        (
            dump.clone(),
            Endpoint::Cargo(unit.clone()),
            CargoPacking {
                heavy: Supplies {
                    stores: 11,
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
        (
            Endpoint::Cargo(unit.clone()),
            dump.clone(),
            CargoPacking::default(),
        ),
    ];
    state
        .logistics
        .unit_supply
        .entry(unit.clone())
        .or_default()
        .carried
        .stores = 10;
    for (from, to, packing) in routes {
        for tag_destination in [false, true] {
            let mut sample = state.clone();
            let tagged = if tag_destination { &to } else { &from };
            let site = match tagged {
                Endpoint::Dump(id) => CargoSite::Dump(id.clone()),
                Endpoint::Cargo(id) => CargoSite::Unit(id.clone()),
                Endpoint::Ground(_) => continue,
                _ => panic!(),
            };
            history_tag(&mut sample, site, amount, 4, 80);
            let mut stock = sample.clone();
            transfer_stock_only(&c, &mut stock, Side::Axis, &from, &to, amount, &packing)
                .expect("negative tag control must otherwise be a legal old-stock transfer");
            let error = history_error_pure(&c, &mut sample, &from, &to, amount, &packing);
            assert!(matches!(
                error,
                DistributionError::Cargo(DistributionCargoError::UnintegratedRoute(_))
            ));
        }
    }
    // Cargo-to-tank and dump-to-ready are otherwise valid, but tagged source
    // goods cannot be silently erased through a non-cargo receiver.
    let mut tank = s.clone();
    tank.logistics
        .unit_supply
        .entry(unit.clone())
        .or_default()
        .carried
        .fuel = 10;
    let fuel = Supplies {
        fuel: 1,
        ..Default::default()
    }; // one tenth at tank endpoint
    history_tag(
        &mut tank,
        CargoSite::Unit(unit.clone()),
        Supplies {
            fuel: 10,
            ..Default::default()
        },
        4,
        80,
    );
    let tank_to = Endpoint::Tank(unit.clone());
    transfer_stock_only(
        &c,
        &mut tank.clone(),
        Side::Axis,
        &Endpoint::Cargo(unit.clone()),
        &tank_to,
        fuel,
        &CargoPacking::default(),
    )
    .unwrap();
    assert!(matches!(
        history_error_pure(
            &c,
            &mut tank,
            &Endpoint::Cargo(unit.clone()),
            &tank_to,
            fuel,
            &CargoPacking::default()
        ),
        DistributionError::Cargo(DistributionCargoError::UnintegratedRoute(_))
    ));
    let ready = s
        .land
        .units
        .values()
        .find(|u| {
            u.side == Side::Axis
                && super::super::ready_ammo_capacity(&c, &s, &u.id).is_ok_and(|cap| cap.get() > 0)
        })
        .unwrap()
        .id
        .clone();
    let mut ammo = s.clone();
    ammo.land.units.get_mut(&ready).unwrap().location = Location::Hex {
        hex: "C4020".into(),
    };
    let ammunition = Supplies {
        ammo: 1,
        ..Default::default()
    };
    history_tag(
        &mut ammo,
        CargoSite::Dump(dump_id.clone()),
        ammunition,
        4,
        80,
    );
    let ready_to = Endpoint::Ready(ready);
    transfer_stock_only(
        &c,
        &mut ammo.clone(),
        Side::Axis,
        &dump,
        &ready_to,
        ammunition,
        &CargoPacking::default(),
    )
    .unwrap();
    assert!(matches!(
        history_error_pure(
            &c,
            &mut ammo,
            &dump,
            &ready_to,
            ammunition,
            &CargoPacking::default()
        ),
        DistributionError::Cargo(DistributionCargoError::UnintegratedRoute(_))
    ));
}
/// Cases: airlog:53.25
#[test]
fn expired_tags_do_not_gate_and_malformed_current_history_is_invariant() {
    use super::super::cargo_history::CargoSite;
    let (c, mut s, _, _, dump) = history_fixture();
    let Endpoint::Dump(id) = &dump else { panic!() };
    let other = "component.history.other".to_string();
    let mut d = s.logistics.dumps[id].clone();
    d.id = other.clone();
    s.logistics.dumps.insert(other.clone(), d);
    let amount = Supplies {
        stores: 1,
        ..Default::default()
    };
    history_tag(&mut s, CargoSite::Dump(id.clone()), amount, 4, 80);
    let mut expired = s.clone();
    expired.cursor.op_stage = Some(2); // actual earlier-stage tag in this component fixture
    transfer(
        &c,
        &mut expired,
        Side::Axis,
        &dump,
        &Endpoint::Dump(other.clone()),
        amount,
        &CargoPacking::default(),
    )
    .unwrap();
    assert_eq!(expired.logistics.dumps[id].supplies.stores, 19);
    // Both endpoints are checked; a tagged source must not mask corrupt target.
    history_tag(&mut s, CargoSite::Dump(other.clone()), amount, -1, 80);
    let error = history_error_pure(
        &c,
        &mut s,
        &dump,
        &Endpoint::Dump(other),
        amount,
        &CargoPacking::default(),
    );
    assert!(matches!(
        rejection(error),
        cna_core::engine::Rejection::Engine(EngineError::Invariant { .. })
    ));
}
/// Cases: airlog:53.25
#[test]
fn present_funded_motion_mismatches_are_invariant_absent_stale_unknown_and_valid_mixed_unsupported()
{
    use super::super::{
        cargo_history::{CargoSite, motion},
        pool_fuel,
    };
    let (c, mut s, _, pool, dump) = history_fixture();
    let Endpoint::Pool(id) = &pool else { panic!() };
    // Component funding at zero consumes no fuel. This is not a scheduled Move.
    pool_fuel::spend_pool_segment_fuel(&c, &mut s, id, 0).unwrap();
    assert!(s.logistics.pool_fuel_segments[id].cohorts_initialized);
    let stage = super::super::water::WaterStage::current(&s);
    let entry = s
        .logistics
        .cargo_history
        .motion
        .entries
        .iter()
        .position(|e| e.site == CargoSite::Pool(id.clone()) && e.stage == stage)
        .unwrap();
    assert!(
        s.logistics.cargo_history.motion.entries[entry]
            .cohorts
            .len()
            >= 2
    );
    for fault in 0..7 {
        let mut sample = s.clone();
        let e = &mut sample.logistics.cargo_history.motion.entries[entry];
        match fault {
            0 => {
                e.cohorts.pop();
            }
            1 => e.cohorts[0].id = "wrong.identity".into(),
            2 => {
                e.cohorts[0].kind = if e.cohorts[0].kind == super::super::FuelTruckKind::Light {
                    super::super::FuelTruckKind::Heavy
                } else {
                    super::super::FuelTruckKind::Light
                }
            }
            3 => e.cohorts[0].count += 1,
            4 => e.cohorts.push(e.cohorts[0].clone()),
            5 => e.cohorts[0].spent_cp_quarters = -1,
            _ => {
                let duplicate = e.clone();
                sample
                    .logistics
                    .cargo_history
                    .motion
                    .entries
                    .push(duplicate);
            }
        }
        let error = history_error_pure(
            &c,
            &mut sample,
            &pool,
            &dump,
            Supplies {
                stores: 1,
                ..Default::default()
            },
            &CargoPacking::default(),
        );
        assert!(
            matches!(
                rejection(error),
                cna_core::engine::Rejection::Engine(EngineError::Invariant { .. })
            ),
            "fault {fault}"
        );
    }
    for stale in [false, true] {
        let mut sample = s.clone();
        if stale {
            sample.logistics.cargo_history.motion.entries[entry]
                .stage
                .game_turn += 1;
        } else {
            sample.logistics.cargo_history.motion.entries.remove(entry);
        }
        let error = history_error_pure(
            &c,
            &mut sample,
            &pool,
            &dump,
            Supplies {
                stores: 1,
                ..Default::default()
            },
            &CargoPacking::default(),
        );
        assert_eq!(
            error,
            DistributionError::Cargo(DistributionCargoError::Motion(motion::MotionError::Unknown))
        );
        assert!(
            matches!(rejection(error),cna_core::engine::Rejection::Engine(EngineError::Unsupported {case,..}) if case=="airlog:53.25")
        );
    }
    let mut mixed = s.clone();
    mixed.logistics.cargo_history.motion.entries[entry].cohorts[0].spent_cp_quarters = 4;
    let error = history_error_pure(
        &c,
        &mut mixed,
        &pool,
        &dump,
        Supplies {
            stores: 1,
            ..Default::default()
        },
        &CargoPacking::default(),
    );
    assert_eq!(
        error,
        DistributionError::Cargo(DistributionCargoError::Motion(motion::MotionError::Mixed))
    );
    assert!(
        matches!(rejection(error),cna_core::engine::Rejection::Engine(EngineError::Unsupported {case,..}) if case=="airlog:53.25")
    );
}
/// Cases: airlog:53.25
#[test]
fn retained_over_ceiling_dump_lot_refuses_reload_without_stock_or_serial_changes() {
    use super::super::cargo_history::CargoSite;
    let (c, mut s, _, pool, dump) = history_fixture();
    let Endpoint::Pool(id) = &pool else { panic!() };
    let Endpoint::Dump(d) = &dump else { panic!() };
    let amount = Supplies {
        stores: 5,
        ..Default::default()
    };
    s.logistics.dumps.get_mut(d).unwrap().supplies = amount;
    history_tag(&mut s, CargoSite::Dump(d.clone()), amount, 84, 80);
    let packing = final_pool_packing(&c, &s, id, amount);
    let error = history_error_pure(&c, &mut s, &dump, &pool, amount, &packing);
    assert!(
        matches!(rejection(error),cna_core::engine::Rejection::Illegal {message} if message.contains("53.25"))
    );
}

fn history_game(s: State) -> cna_core::engine::Game<crate::Cna> {
    let mut state = s;
    state.setup.closed = true; // explicit component dispatcher fixture, not setup proof
    state.cursor.op_stage = Some(1);
    state.cursor.entered = false;
    state.turn.weather = Some(crate::state::WeatherState {
        kind: cna_tables::land::weather::WeatherKind::Normal,
        storm_sections: vec![],
    });
    cna_core::engine::Game {
        state,
        rng: cna_core::dice::CampaignRng::from_seed([3; 32]).state(),
    }
}
fn history_response(p: &Pending, action: Value) -> cna_core::engine::Command {
    cna_core::engine::Command::Respond(cna_core::decision::DecisionResponse {
        decision_id: p.id.clone(),
        seat: p.seat,
        controller_epoch: 1,
        decision_revision: p.revision,
        idempotency_key: p.id.to_string(),
        action,
        public_explanation: None,
    })
}
fn history_order(from: &Endpoint, to: &Endpoint, amount: Supplies, packing: CargoPacking) -> Value {
    serde_json::json!({"from":serde_json::to_string(from).unwrap(),
        "to":serde_json::to_string(to).unwrap(),"amount":amount,"packing":packing})
}
/// Cases: airlog:53.24, airlog:53.25, land:3.6
#[test]
fn real_dispatcher_buffer_rejection_closure_recovery_retry_and_late_fault_are_atomic() {
    use cna_core::engine::{Command, Ruleset, evaluate};
    let (c, s, _, pool, dump) = history_fixture();
    let rules = crate::Cna::dev();
    let opened = evaluate(&rules, &c, &history_game(s), &Command::Advance)
        .unwrap()
        .game;
    let p = opened
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap()
        .clone();
    assert_eq!(p.kind, super::super::batches::DISTRIBUTION);
    let amount = Supplies {
        stores: 5,
        ..Default::default()
    };
    let good = history_order(&pool, &dump, amount, CargoPacking::default());
    let ground = Endpoint::Ground(Location::Hex {
        hex: "C4020".into(),
    });
    let bad = history_order(
        &pool,
        &ground,
        Supplies {
            stores: 1,
            ..Default::default()
        },
        CargoPacking::default(),
    );
    let before = serde_json::to_value(&opened).unwrap();
    let error = evaluate(
        &rules,
        &c,
        &opened,
        &history_response(&p, serde_json::json!([good.clone(), bad])),
    )
    .unwrap_err();
    assert!(
        matches!(error,cna_core::engine::Rejection::Engine(EngineError::Unsupported {case,..}) if case=="airlog:53.25")
    );
    assert_eq!(serde_json::to_value(&opened).unwrap(), before);
    let command = history_response(&p, serde_json::json!([good]));
    let accepted = evaluate(&rules, &c, &opened, &command).unwrap();
    assert_eq!(accepted.game.rng, opened.rng);
    assert_eq!(
        serde_json::to_value(&accepted.game.state.logistics.truck_pools).unwrap(),
        serde_json::to_value(&opened.state.logistics.truck_pools).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&accepted.game.state.logistics.dumps).unwrap(),
        serde_json::to_value(&opened.state.logistics.dumps).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&accepted.game.state.logistics.cargo_history).unwrap(),
        serde_json::to_value(&opened.state.logistics.cargo_history).unwrap()
    );
    assert!(evaluate(&rules, &c, &accepted.game, &command).is_err());
    let other = accepted
        .game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == super::super::batches::DISTRIBUTION)
        .unwrap();
    let submitted = evaluate(
        &rules,
        &c,
        &accepted.game,
        &history_response(other, Value::Null),
    )
    .unwrap()
    .game;
    let recovered: cna_core::engine::Game<crate::Cna> =
        serde_json::from_value(serde_json::to_value(&submitted).unwrap()).unwrap();
    crate::testkit::assert_action_indistinguishable(
        &rules,
        &c,
        &submitted,
        &recovered,
        &Command::Advance,
        Side::Commonwealth,
    );
    let closed = evaluate(&rules, &c, &submitted, &Command::Advance).unwrap();
    let restored = evaluate(&rules, &c, &recovered, &Command::Advance).unwrap();
    assert_eq!(
        serde_json::to_value(&closed.game).unwrap(),
        serde_json::to_value(&restored.game).unwrap()
    );
    assert_eq!(closed.events, restored.events);
    assert_eq!(stock(&closed.game.state, &pool).unwrap().stores, 15);
    assert_eq!(stock(&closed.game.state, &dump).unwrap().stores, 25);
    let key =
        super::super::batches::batch_key(&submitted.state, super::super::batches::DISTRIBUTION);
    assert!(
        !closed
            .game
            .state
            .logistics
            .allocation_batches
            .submitted
            .contains_key(&key)
    );
    assert!(
        closed
            .game
            .state
            .logistics
            .allocation_batches
            .completed
            .contains(&key)
    );
    assert_eq!(closed.game.rng, submitted.rng);
    let repeated = evaluate(&rules, &c, &closed.game, &Command::Advance).unwrap();
    assert_eq!(
        serde_json::to_value(&repeated.game.state.logistics).unwrap(),
        serde_json::to_value(&closed.game.state.logistics).unwrap()
    );
    assert_eq!(repeated.game.rng, closed.game.rng);
    // Fault injected ONLY after accepted plan, before trusted adjudication.
    let mut failed = submitted.state.clone();
    let Endpoint::Pool(id) = &pool else { panic!() };
    failed
        .logistics
        .cargo_history
        .motion
        .entries
        .iter_mut()
        .find(|e| e.site == super::super::cargo_history::CargoSite::Pool(id.clone()))
        .unwrap()
        .cohorts[0]
        .count += 1;
    let before = serde_json::to_value(&failed).unwrap();
    let mut rng = cna_core::dice::CampaignRng::from_state(&submitted.rng);
    let dice = rng.state();
    let mut events = vec![];
    let error = rules
        .finish_step(
            &c,
            &mut failed,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap_err();
    assert!(matches!(error, EngineError::Invariant { .. }));
    assert_eq!(serde_json::to_value(&failed).unwrap(), before);
    assert_eq!(rng.state(), dice);
    assert!(events.is_empty());
    let _ = rules.pending(&c, &closed.game.state);
}
/// Cases: land:3.6, airlog:53.25
#[test]
fn hidden_foreign_inventory_does_not_change_own_guard_or_accepted_streams() {
    use cna_core::engine::{Command, evaluate};
    let (c, s, _, pool, dump) = history_fixture();
    let rules = crate::Cna::dev();
    let a = evaluate(&rules, &c, &history_game(s), &Command::Advance)
        .unwrap()
        .game;
    let mut b: cna_core::engine::Game<crate::Cna> =
        serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
    let enemy = b
        .state
        .land
        .units
        .values()
        .find(|u| u.side == Side::Commonwealth)
        .unwrap()
        .id
        .clone();
    b.state
        .logistics
        .unit_supply
        .entry(enemy.clone())
        .or_default()
        .carried
        .stores += 7;
    history_tag(
        &mut b.state,
        super::super::cargo_history::CargoSite::Unit(enemy),
        Supplies {
            stores: 7,
            ..Default::default()
        },
        4,
        80,
    );
    let p = a
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap();
    let amount = Supplies {
        stores: 1,
        ..Default::default()
    };
    for to in [
        dump.clone(),
        Endpoint::Ground(Location::Hex {
            hex: "C4020".into(),
        }),
    ] {
        let command = history_response(
            p,
            serde_json::json!([history_order(&pool, &to, amount, CargoPacking::default())]),
        );
        crate::testkit::assert_action_indistinguishable(&rules, &c, &a, &b, &command, Side::Axis);
    }
    // Own uniform history variations are hidden from the enemy. Both actions
    // are accepted; no rejected-own-action vs successful-pass comparison.
    let mut tagged: cna_core::engine::Game<crate::Cna> =
        serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
    let Endpoint::Pool(id) = &pool else { panic!() };
    history_tag(
        &mut tagged.state,
        super::super::cargo_history::CargoSite::Pool(id.clone()),
        Supplies {
            stores: 20,
            ..Default::default()
        },
        4,
        80,
    );
    let command = history_response(
        p,
        serde_json::json!([history_order(&pool, &dump, amount, CargoPacking::default())]),
    );
    crate::testkit::assert_action_indistinguishable(
        &rules,
        &c,
        &a,
        &tagged,
        &command,
        Side::Commonwealth,
    );
    let left = evaluate(&rules, &c, &a, &command).unwrap().game;
    let right = evaluate(&rules, &c, &tagged, &command).unwrap().game;
    let other = left
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == super::super::batches::DISTRIBUTION)
        .unwrap();
    let pass = history_response(other, Value::Null);
    crate::testkit::assert_action_indistinguishable(
        &rules,
        &c,
        &left,
        &right,
        &pass,
        Side::Commonwealth,
    );
    let left = evaluate(&rules, &c, &left, &pass).unwrap().game;
    let right = evaluate(&rules, &c, &right, &pass).unwrap().game;
    crate::testkit::assert_action_indistinguishable(
        &rules,
        &c,
        &left,
        &right,
        &Command::Advance,
        Side::Commonwealth,
    );
}
