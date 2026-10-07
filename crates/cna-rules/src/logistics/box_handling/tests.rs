use super::*;
use crate::{
    CnaContent,
    logistics::{
        CargoPacking,
        distribution::{self, Endpoint},
    },
    seq::{Block, OPSTAGE},
};
use cna_content::{
    scenario::Placement,
    units::{Toe, Trucks, WeaponPoints},
};
use cna_core::{dice::CampaignRng, visibility::Perspective};
use serde_json::json;
const UNIT: &str = "it.1_libyan_div.viii_libyan_bn";
fn fixture() -> (CnaContent, State, Carrier, Carrier) {
    let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut state = State::new(&content).unwrap();
    state.cursor.block = Block::OpStage;
    state.cursor.op_stage = Some(1);
    state.cursor.index = OPSTAGE
        .iter()
        .position(|s| s.anchor == "opstage.organization.supply_distribution")
        .unwrap();
    let location = Location::OffMap {
        id: "box_tripoli".into(),
    };
    let u = state.land.units.get_mut(&UNIT.into()).unwrap();
    u.location = location.clone();
    u.trucks = Trucks {
        heavy: 1,
        ..Default::default()
    };
    u.transport_trucks = Trucks::default();
    let pool = crate::logistics::pools::add_truck_pool(
        &mut state.logistics,
        None,
        Side::Axis,
        Placement::City {
            city: "tripoli".into(),
        },
        Some(location.clone()),
        Trucks {
            heavy: 1,
            ..Default::default()
        },
        Supplies {
            water: 20,
            ..Default::default()
        },
    )
    .unwrap();
    state.logistics.dumps.insert(
        "box-stock".into(),
        crate::state::Dump {
            id: "box-stock".into(),
            marker: "dump-1".into(),
            side: Side::Axis,
            location: crate::state::DumpLocation::OffMap {
                id: "box_tripoli".into(),
            },
            supplies: Supplies {
                fuel: 100,
                stores: 10,
                ..Default::default()
            },
            active: true,
            dummy: false,
        },
    );
    (
        content,
        state,
        Carrier::Unit(UNIT.into()),
        Carrier::Pool(pool),
    )
}
/// Cases: land:8.88, airlog:53.24
/// Interpretations: interp:airlog-0019
#[test]
fn actual_box_loads_and_unloads_stamp_carriers_without_changing_conservation() {
    let (c, mut s, unit, pool) = fixture();
    let goods = Supplies {
        stores: 3,
        ..Default::default()
    };
    distribution::transfer(
        &c,
        &mut s,
        Side::Axis,
        &Endpoint::Dump("box-stock".into()),
        &Endpoint::Cargo(UNIT.into()),
        goods,
        &CargoPacking {
            heavy: goods,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(blocks_movement(&s, &unit).is_some());
    assert_eq!(
        s.land.units[&UNIT.into()]
            .box_handling
            .as_ref()
            .unwrap()
            .loaded,
        goods
    );
    let unloaded = Supplies {
        stores: 1,
        ..Default::default()
    };
    distribution::transfer(
        &c,
        &mut s,
        Side::Axis,
        &Endpoint::Cargo(UNIT.into()),
        &Endpoint::Dump("box-stock".into()),
        unloaded,
        &CargoPacking::default(),
    )
    .unwrap();
    assert_eq!(s.logistics.dumps["box-stock"].supplies.stores, 8);
    assert_eq!(s.logistics.unit_supply[&UNIT.into()].carried.stores, 2);
    assert_eq!(
        s.land.units[&UNIT.into()]
            .box_handling
            .as_ref()
            .unwrap()
            .unloaded,
        unloaded
    );
    let Carrier::Pool(ref pool_id) = pool else {
        unreachable!()
    };
    let water = Supplies {
        water: 5,
        ..Default::default()
    };
    distribution::transfer(
        &c,
        &mut s,
        Side::Axis,
        &Endpoint::Pool(pool_id.clone()),
        &Endpoint::Dump("box-stock".into()),
        water,
        &CargoPacking::default(),
    )
    .unwrap();
    assert!(blocks_movement(&s, &pool).is_some());
    let p = s
        .logistics
        .truck_pools
        .iter()
        .find(|p| p.id == *pool_id)
        .unwrap();
    assert_eq!(p.cargo.water, 15);
    assert_eq!(p.box_handling.as_ref().unwrap().unloaded, water);
    assert_eq!(s.logistics.dumps["box-stock"].supplies.water, 5);
}
/// Cases: land:8.88, airlog:49.14, airlog:53.24
/// Interpretations: interp:airlog-0019
#[test]
fn tank_refill_and_on_map_cargo_do_not_stamp_the_box_rule() {
    let (c, mut s, unit, _) = fixture();
    s.land.units.get_mut(&UNIT.into()).unwrap().toe = Some(Toe::Weapons(vec![WeaponPoints {
        weapon: "it.cv33".into(),
        n: 1,
    }]));
    distribution::transfer(
        &c,
        &mut s,
        Side::Axis,
        &Endpoint::Dump("box-stock".into()),
        &Endpoint::Tank(UNIT.into()),
        Supplies {
            fuel: 5,
            ..Default::default()
        },
        &CargoPacking::default(),
    )
    .unwrap();
    assert_eq!(s.logistics.unit_supply[&UNIT.into()].tank_fuel.get(), 5);
    assert!(blocks_movement(&s, &unit).is_none());
    s.land.units.get_mut(&UNIT.into()).unwrap().location = Location::Hex {
        hex: "C4020".into(),
    };
    s.logistics.dumps.get_mut("box-stock").unwrap().location = crate::state::DumpLocation::Hex {
        hex: "C4020".into(),
    };
    let goods = Supplies {
        stores: 3,
        ..Default::default()
    };
    distribution::transfer(
        &c,
        &mut s,
        Side::Axis,
        &Endpoint::Dump("box-stock".into()),
        &Endpoint::Cargo(UNIT.into()),
        goods,
        &CargoPacking {
            heavy: goods,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(blocks_movement(&s, &unit).is_none());
}
/// Cases: land:8.88, land:3.6
#[test]
fn stage_expiry_checkpoint_defaults_and_enemy_views_preserve_private_history() {
    let (c, mut s, unit, pool) = fixture();
    let mut clear = s.clone();
    record_goods(
        &mut s,
        &unit,
        Supplies {
            fuel: 2,
            ..Default::default()
        },
        true,
    )
    .unwrap();
    record_goods(
        &mut s,
        &pool,
        Supplies {
            water: 3,
            ..Default::default()
        },
        false,
    )
    .unwrap();
    for world in [&mut s, &mut clear] {
        world.land.units.get_mut(&UNIT.into()).unwrap().location = Location::Hex {
            hex: "C4020".into(),
        };
    }
    assert!(blocks_movement(&s, &unit).is_some());
    crate::testkit::assert_indistinguishable(
        &crate::Cna::dev(),
        &c,
        &s,
        &clear,
        Side::Commonwealth,
    );
    assert!(
        crate::view::inspect(&c, &s, Perspective::Side(Side::Axis), UNIT, false)
            .unwrap()
            .to_string()
            .contains("box_handling")
    );
    let mut restored: State = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert!(blocks_movement(&restored, &unit).is_some());
    restored.cursor.op_stage = Some(2);
    assert!(blocks_movement(&restored, &unit).is_none());
    assert!(blocks_movement(&restored, &pool).is_none());
    restored.land.units.get_mut(&UNIT.into()).unwrap().location = Location::OffMap {
        id: "box_tripoli".into(),
    };
    record_goods(
        &mut restored,
        &unit,
        Supplies {
            stores: 1,
            ..Default::default()
        },
        true,
    )
    .unwrap();
    assert_eq!(
        restored.land.units[&UNIT.into()]
            .box_handling
            .as_ref()
            .unwrap()
            .loaded,
        Supplies {
            stores: 1,
            ..Default::default()
        }
    );
    let mut old = serde_json::to_value(&clear).unwrap();
    old["land"]["units"][UNIT]
        .as_object_mut()
        .unwrap()
        .remove("box_handling");
    for p in old["logistics"]["truck_pools"].as_array_mut().unwrap() {
        p.as_object_mut().unwrap().remove("box_handling");
    }
    let legacy: State = serde_json::from_value(old).unwrap();
    assert!(blocks_movement(&legacy, &unit).is_none());
}
/// Cases: land:8.88, land:3.6
/// Interpretations: interp:airlog-0019
#[test]
fn division_full_is_unsupported_and_dev_keeps_original_stamp_with_private_note() {
    let (_, mut s, unit, _) = fixture();
    record_goods(
        &mut s,
        &unit,
        Supplies {
            stores: 1,
            ..Default::default()
        },
        true,
    )
    .unwrap();
    let before = serde_json::to_value(&s).unwrap();
    let mut rng = CampaignRng::from_seed([1; 32]);
    let mut events = vec![];
    let mut cx = Cx {
        rng: &mut rng,
        events: &mut events,
    };
    assert!(
        matches!(prepare_division(&s,&unit,true,&mut cx),Err(EngineError::Unsupported{case,..}) if case=="land:8.88")
    );
    assert!(cx.events.is_empty());
    prepare_division(&s, &unit, false, &mut cx).unwrap();
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert_eq!(cx.events.len(), 1);
    assert_eq!(cx.events[0].audience, Audience::Side(Side::Axis));
}
/// Cases: land:8.88, airlog:53.24
#[test]
fn stamp_overflow_rolls_back_the_entire_stock_transfer() {
    let (c, mut s, unit, _) = fixture();
    record_goods(
        &mut s,
        &unit,
        Supplies {
            stores: i32::MAX,
            ..Default::default()
        },
        true,
    )
    .unwrap();
    let before = serde_json::to_value(&s).unwrap();
    let goods = Supplies {
        stores: 1,
        ..Default::default()
    };
    assert_eq!(
        distribution::transfer(
            &c,
            &mut s,
            Side::Axis,
            &Endpoint::Dump("box-stock".into()),
            &Endpoint::Cargo(UNIT.into()),
            goods,
            &CargoPacking {
                heavy: goods,
                ..Default::default()
            }
        ),
        Err(SupplyError::Invalid)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert!(is_supply_box(&Location::OffMap {
        id: "box_tunis".into()
    }));
    assert!(!is_supply_box(&Location::OffMap {
        id: "land-transit:axis:1".into()
    }));
    assert!(!is_supply_box(&Location::Hex {
        hex: "C4020".into()
    }));
    assert_eq!(
        json!(
            s.land.units[&UNIT.into()]
                .box_handling
                .as_ref()
                .unwrap()
                .loaded
                .stores
        ),
        json!(i32::MAX)
    );
}

/// Cases: land:8.88, airlog:53.24, land:3.6
#[test]
fn invalid_last_batch_item_rolls_back_prior_handling_stamps_and_stock_debits() {
    let (c, mut s, unit, _) = fixture();
    let mut rng = CampaignRng::from_seed([3; 32]);
    let mut events = vec![];
    let mut cx = Cx {
        rng: &mut rng,
        events: &mut events,
    };
    crate::logistics::batches::enter_distribution_with_policy(&c, &mut s, &mut cx, false).unwrap();
    let p = s
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == crate::logistics::batches::DISTRIBUTION && p.seat.side == Side::Axis)
        .unwrap()
        .clone();
    let from = serde_json::to_string(&Endpoint::Dump("box-stock".into())).unwrap();
    let to = serde_json::to_string(&Endpoint::Cargo(UNIT.into())).unwrap();
    let good = Supplies {
        stores: 3,
        ..Default::default()
    };
    let bad = Supplies {
        stores: 100,
        ..Default::default()
    };
    let action = json!([
        {"from":from,"to":to,"amount":good,"packing":CargoPacking{heavy:good,..Default::default()}},
        {"from":from,"to":to,"amount":bad,"packing":CargoPacking{heavy:bad,..Default::default()}}
    ]);
    let before = serde_json::to_value(&s).unwrap();
    assert!(crate::logistics::batches::answer(&c, &mut s, &p, &action, &mut cx, false).is_err());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert!(blocks_movement(&s, &unit).is_none());
}
