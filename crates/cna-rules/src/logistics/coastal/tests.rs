use super::*;
use crate::seq::{Block, Half, OPSTAGE, PLAYER_HALF};
use cna_core::{dice::CampaignRng, engine::Ruleset, visibility::Perspective};
use serde_json::json;
use std::sync::OnceLock;
fn content() -> &'static CnaContent {
    static C: OnceLock<CnaContent> = OnceLock::new();
    C.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn setup() -> State {
    let c = content();
    let mut s = State::new(c).unwrap();
    s.cursor.op_stage = Some(1);
    s.cursor.block = Block::PlayerHalf;
    s.cursor.half = Some(Half::A);
    s.cursor.index = PLAYER_HALF
        .iter()
        .position(|p| p.anchor == "opstage.truck_convoy_movement")
        .unwrap();
    s.turn.player_a = Some(Side::Axis);
    ports::initialize(c, &mut s);
    initialize(c, &mut s).unwrap();
    s
}
fn dump(s: &mut State, id: &str, side: Side, at: Location, cargo: Supplies) {
    let location = match at {
        Location::Hex { hex } => DumpLocation::Hex { hex },
        Location::OffMap { id } => DumpLocation::OffMap { id },
        _ => panic!(),
    };
    let marker = super::super::dump_markers::next_marker(&mut s.logistics).unwrap();
    s.logistics.dumps.insert(
        id.into(),
        crate::state::Dump {
            id: id.into(),
            marker,
            side,
            location,
            supplies: cargo,
            active: true,
            dummy: false,
        },
    );
}
fn port_fixture() -> State {
    let c = content();
    let mut s = setup();
    let at = Location::Hex {
        hex: "C4022".into(),
    };
    for u in s
        .land
        .units
        .values_mut()
        .filter(|u| u.side == Side::Commonwealth)
    {
        u.location = Location::OffMap {
            id: "fixture".into(),
        };
    }
    for ship in s.logistics.coastal_ships.values_mut() {
        ship.location = at.clone();
    }
    ports::record_entry(c, &mut s, Side::Axis, &at);
    dump(
        &mut s,
        "fixture",
        Side::Axis,
        at,
        Supplies {
            stores: 1000,
            ..Default::default()
        },
    );
    assert!(
        c.map
            .neighbors(&"C4022".into())
            .iter()
            .any(|h| h.id == HexId::new("C4023"))
    );
    s
}
fn stores(n: i32) -> Supplies {
    Supplies {
        stores: n,
        ..Default::default()
    }
}
fn path(n: usize) -> Vec<HexId> {
    (0..n)
        .map(|i| HexId::new(if i % 2 == 0 { "C4023" } else { "C4022" }))
        .collect()
}
/// Cases: scen:59.54, airlog:56.31
#[test]
fn real_four_ships_start_empty_at_tripoli_and_unknown_join_stays_unsupported() {
    let mut s = setup();
    assert_eq!(s.logistics.coastal_ships.len(), 4);
    for (id, ship) in &s.logistics.coastal_ships {
        assert_eq!(
            ship.location,
            Location::OffMap {
                id: "box_tripoli".into()
            }
        );
        assert_eq!(ship.cargo, Supplies::default());
        assert_eq!(
            content().units.coastal_ships[id].capacity_tons,
            Some(if id.ends_with(".d") { 2000 } else { 1000 })
        );
    }
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(
        sail(content(), &mut s, "axis.coastal.a", &["C4023".into()]),
        Err(SupplyError::Unsupported {
            case: "airlog:56.31"
        }
        .into())
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}
/// Cases: airlog:56.31, airlog:56.34, airlog:56.35
/// Interpretations: interp:airlog-0011
#[test]
fn fifty_cp_includes_handling_and_checkpointed_unload() {
    let c = content();
    let mut s = port_fixture();
    load(c, &mut s, "axis.coastal.a", "fixture", stores(10)).unwrap();
    sail(c, &mut s, "axis.coastal.a", &path(40)).unwrap();
    let mut s: State = serde_json::from_value(serde_json::to_value(s).unwrap()).unwrap();
    unload(c, &mut s, "axis.coastal.a", "fixture", stores(10)).unwrap();
    assert_eq!(s.logistics.coastal_ships["axis.coastal.a"].cp_quarters, 200);
    assert_eq!(s.logistics.dumps["fixture"].supplies.stores, 1000);
    assert_eq!(s.logistics.ports["C4022"].used_tons24, 20 * 24);
    assert!(sail(c, &mut s, "axis.coastal.a", &["C4023".into()]).is_err());
    let mut s = port_fixture();
    load(c, &mut s, "axis.coastal.a", "fixture", stores(10)).unwrap();
    sail(c, &mut s, "axis.coastal.a", &path(42)).unwrap();
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(
        unload(c, &mut s, "axis.coastal.a", "fixture", stores(10)),
        Err(SupplyError::Insufficient.into())
    );
    assert_eq!(serde_json::to_value(s).unwrap(), before);
}
/// Cases: airlog:56.32, airlog:56.34
#[test]
fn loading_gate_is_phase_wide_and_resets_next_stage() {
    let c = content();
    let mut s = port_fixture();
    load(c, &mut s, "axis.coastal.a", "fixture", stores(10)).unwrap();
    load(c, &mut s, "axis.coastal.b", "fixture", stores(10)).unwrap();
    sail(c, &mut s, "axis.coastal.a", &path(2)).unwrap();
    assert_eq!(
        load(c, &mut s, "axis.coastal.c", "fixture", stores(10)),
        Err(SupplyError::Invalid.into())
    );
    s.cursor.op_stage = Some(2);
    load(c, &mut s, "axis.coastal.c", "fixture", stores(10)).unwrap();
    s.cursor.half = Some(Half::B);
    assert_eq!(
        load(c, &mut s, "axis.coastal.d", "fixture", stores(10)),
        Err(SupplyError::Invalid.into())
    );
}
/// Cases: airlog:55.14, airlog:56.31, airlog:56.33, airlog:56.34
#[test]
fn cargo_capacity_type_negative_and_port_rejections_are_atomic() {
    let c = content();
    let mut s = port_fixture();
    s.logistics
        .dumps
        .get_mut("fixture")
        .unwrap()
        .supplies
        .stores = 2000;
    for cargo in [
        stores(1001),
        stores(-1),
        Supplies {
            stores: 1,
            ammo: 1,
            ..Default::default()
        },
    ] {
        let before = serde_json::to_value(&s).unwrap();
        assert!(load(c, &mut s, "axis.coastal.a", "fixture", cargo).is_err());
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
    }
    s.logistics.ports.get_mut("C4022").unwrap().efficiency = 0;
    assert!(load(c, &mut s, "axis.coastal.a", "fixture", stores(1)).is_err());
    s.logistics.ports.get_mut("C4022").unwrap().efficiency = 5;
    s.logistics
        .coastal_ships
        .get_mut("axis.coastal.a")
        .unwrap()
        .location = Location::Hex {
        hex: "C4023".into(),
    };
    let enemy = s
        .land
        .units
        .values_mut()
        .find(|u| u.side == Side::Commonwealth)
        .unwrap();
    enemy.location = Location::Hex {
        hex: "C4022".into(),
    };
    assert_eq!(
        sail(c, &mut s, "axis.coastal.a", &["C4022".into()]),
        Err(SupplyError::Insufficient.into())
    );
}
/// Cases: airlog:56.34, land:3.6, land:3.62
#[test]
fn batched_list_rolls_back_and_new_dump_uses_opaque_marker() {
    let c = content();
    let mut s = port_fixture();
    let mut rng = CampaignRng::from_seed([4; 32]);
    let mut events = vec![];
    enter_axis(
        c,
        &mut s,
        false,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    let pending = s.decisions.pending.pop().unwrap();
    assert!(matches!(pending.space.schema, ActionSchema::List { .. }));
    let order = json!({"ship":"axis.coastal.a","operation":"load","dump":"fixture","cargo":stores(1),"path":[]});
    let invalid = json!([order,{"ship":"axis.coastal.invalid","operation":"load","dump":"fixture","cargo":stores(1),"path":[]}]);
    let before = serde_json::to_value(&s).unwrap();
    let rng_before = rng.state();
    events.clear();
    assert!(
        answer(
            c,
            &mut s,
            &pending,
            &invalid,
            &mut Cx {
                rng: &mut rng,
                events: &mut events
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert!(events.is_empty());
    assert_eq!(rng.state(), rng_before);
    let action = json!([order,{"ship":"axis.coastal.a","operation":"unload","dump":"new:C4022","cargo":stores(1),"path":[]}]);
    answer(
        c,
        &mut s,
        &pending,
        &action,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert!(!s.logistics.dumps.contains_key("axis.coastal.port.C4022"));
    assert_eq!(
        s.logistics.dumps["fixture"].supplies,
        serde_json::from_value::<State>(before.clone())
            .unwrap()
            .logistics
            .dumps["fixture"]
            .supplies
    );
    let saved: State = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    finish(
        c,
        &mut s,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    let closed = serde_json::to_value(&s).unwrap();
    finish(
        c,
        &mut s,
        &mut Cx {
            rng: &mut rng,
            events: &mut vec![],
        },
    )
    .unwrap();
    assert_eq!(serde_json::to_value(&s).unwrap(), closed);
    let mut recovered = saved;
    finish(
        c,
        &mut recovered,
        &mut Cx {
            rng: &mut CampaignRng::from_seed([4; 32]),
            events: &mut vec![],
        },
    )
    .unwrap();
    assert_eq!(serde_json::to_value(&recovered).unwrap(), closed);
    let dump = &s.logistics.dumps["axis.coastal.port.C4022"];
    assert!(dump.marker.starts_with("dump-"));
    assert_eq!(dump.supplies.stores, 1);
}
/// Cases: land:3.6, land:3.62, airlog:56.31
#[test]
fn enemy_sees_ship_presence_without_counter_id_or_cargo() {
    let c = content();
    let a = port_fixture();
    let mut b = a.clone();
    b.logistics
        .coastal_ships
        .get_mut("axis.coastal.a")
        .unwrap()
        .cargo = stores(13);
    b.logistics
        .coastal_ships
        .get_mut("axis.coastal.a")
        .unwrap()
        .cp_quarters = 28;
    crate::testkit::assert_indistinguishable(&crate::Cna::dev(), c, &a, &b, Side::Commonwealth);
    let view = crate::Cna::dev().view(c, &a, Perspective::Side(Side::Commonwealth));
    let v = serde_json::to_string(&view).unwrap();
    assert!(v.contains("coastal:axis:C4022"));
    assert!(!v.contains("axis.coastal.a"));
    assert!(
        crate::Cna::dev()
            .inspect(
                c,
                &a,
                Perspective::Side(Side::Commonwealth),
                "axis.coastal.a"
            )
            .is_err()
    );
    assert_eq!(
        crate::Cna::dev()
            .inspect(c, &a, Perspective::Side(Side::Axis), "axis.coastal.a")
            .unwrap()["capacity_tons"],
        1000
    );
}
/// Cases: airlog:55.13, airlog:55.14, land:8.82
#[test]
fn commonwealth_transfer_conserves_supplies_and_both_port_budgets() {
    let mut c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut extra = c
        .places
        .places
        .values()
        .find(|p| p.kind == "port")
        .unwrap()
        .clone();
    extra.id = "test-port".into();
    extra.hex_id = "C4024".into();
    extra.name = "Fixture".into();
    c.places.places.insert(extra.id.clone(), extra);
    let mut s = setup();
    s.cursor.block = Block::OpStage;
    s.cursor.half = None;
    s.cursor.index = OPSTAGE
        .iter()
        .position(|p| p.anchor == "opstage.organization.tactical_shipping")
        .unwrap();
    let a = Location::Hex {
        hex: "C4022".into(),
    };
    let b = Location::Hex {
        hex: "C4024".into(),
    };
    ports::record_entry(&c, &mut s, Side::Commonwealth, &a);
    ports::record_entry(&c, &mut s, Side::Commonwealth, &b);
    dump(&mut s, "origin", Side::Commonwealth, a, stores(20));
    commonwealth_transfer(&c, &mut s, "origin", "new:C4024", stores(7)).unwrap();
    assert_eq!(s.logistics.dumps["origin"].supplies.stores, 13);
    assert_eq!(
        s.logistics.dumps["commonwealth.coastal.port.C4024"]
            .supplies
            .stores,
        7
    );
    for hex in ["C4022", "C4024"] {
        assert_eq!(s.logistics.ports[hex].used_tons24, 7 * 24);
    }
    s.logistics.ports.get_mut("C4024").unwrap().used_tons24 = i64::MAX;
    let before = serde_json::to_value(&s).unwrap();
    assert!(commonwealth_transfer(&c, &mut s, "origin", "new:C4024", stores(1)).is_err());
    assert_eq!(serde_json::to_value(s).unwrap(), before);
}
