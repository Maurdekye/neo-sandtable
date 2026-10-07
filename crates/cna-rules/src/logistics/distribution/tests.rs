use super::*;
use crate::seq::{Block, OPSTAGE};
use cna_content::scenario::Placement;
use cna_core::visibility::Perspective;
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
    let pool = super::super::pools::add_truck_pool(
        &mut state.logistics,
        None,
        Side::Axis,
        Placement::Hex {
            hex: "C4020".into(),
        },
        Some(at),
        Trucks {
            heavy: 1,
            ..Trucks::default()
        },
        Supplies {
            fuel: 250,
            stores: 20,
            ..Supplies::default()
        },
    )
    .unwrap();
    (content, state, id, Endpoint::Pool(pool))
}
/// Cases: airlog:49.14, airlog:49.16, airlog:50.15, airlog:53.24, airlog:54.11, land:3.6
/// Interpretations: interp:airlog-0001, interp:airlog-0008
#[test]
fn unload_pool_create_dump_then_load_first_line_and_refill_fractional_tank() {
    let (c, mut s, id, pool) = setup();
    let ground = Endpoint::Ground(location(&s, Side::Axis, &pool).unwrap());
    let a = Supplies {
        fuel: 100,
        ..Supplies::default()
    };
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
        &pool,
        &ground,
        a,
        &CargoPacking::default(),
    )
    .unwrap();
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
    transfer(
        &c,
        &mut s,
        Side::Axis,
        &pool,
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
        .position(|p| p.seat.side == Side::Axis && p.kind == KIND)
        .unwrap();
    let pending = s.decisions.pending.remove(pos);
    assert_eq!(pending.secrecy, Secrecy::Secret);
    let receiver = serde_json::to_string(&Endpoint::Ground(at)).unwrap();
    crate::Cna::dev()
        .respond_to(&c, &mut s, &pending, &Value::String(receiver), &mut cx)
        .unwrap();
    let pos = s
        .decisions
        .pending
        .iter()
        .position(|p| p.seat.side == Side::Axis && p.kind.starts_with(PREFIX))
        .unwrap();
    let pending = s.decisions.pending.remove(pos);
    let request = serde_json::json!({"source":serde_json::to_string(&pool).unwrap(),"amount":{"ammo":0,"fuel":0,"stores":5,"water":0},"packing":CargoPacking::default()});
    crate::Cna::dev()
        .respond_to(&c, &mut s, &pending, &request, &mut cx)
        .unwrap();
    assert_eq!(s.logistics.dumps["axis.dump-1"].supplies.stores, 5);
    assert!(events.iter().all(|e| e.audience != Audience::Public));
    assert!(
        crate::view::observe(&c, &s, Perspective::Side(Side::Commonwealth))["logistics"]["dumps"]
            .get("axis.dump-1")
            .is_none()
    );
    let _ = id;
}
