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
    ports::initialize_fixture(c, &mut s);
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
    ports::record_entry_fixture(c, &mut s, Side::Axis, &at);
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
    ports::record_entry_fixture(&c, &mut s, Side::Commonwealth, &a);
    ports::record_entry_fixture(&c, &mut s, Side::Commonwealth, &b);
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
// Component histories below are explicit injected guard fixtures, not scheduled
// ship-history entitlement or a source-backed coastal movement proof.
fn current_tag(state: &mut State, site: super::super::cargo_history::CargoSite, cp: i32) {
    use super::super::cargo_history::{CargoHistory, CargoLot};
    state.logistics.cargo_history.histories.insert(
        site,
        CargoHistory {
            stage: WaterStage::current(state),
            lots: vec![CargoLot {
                id: "fixture.current-lot".into(),
                goods: stores(1),
                spent_cp_quarters: cp,
                ceiling_cp_quarters: 160,
                continuous_first_line: false,
            }],
        },
    );
}
fn endpoint_fixture(alias: bool) -> State {
    let mut state = port_fixture();
    state.logistics.dumps.get_mut("fixture").unwrap().supplies = stores(20);
    state
        .logistics
        .coastal_ships
        .get_mut("axis.coastal.a")
        .unwrap()
        .cargo = stores(10);
    if alias {
        dump(
            &mut state,
            "axis.coastal.port.C4022",
            Side::Axis,
            Location::Hex {
                hex: "C4022".into(),
            },
            stores(20),
        );
    }
    state
}
/// Cases: airlog:53.25, airlog:56.34, land:3.6
#[test]
fn coastal_both_original_endpoints_guard_after_valid_stock_preview_and_alias_resolution() {
    use super::super::cargo_history::CargoSite;
    for unload_order in [false, true] {
        for alias in [false, true] {
            if alias && !unload_order {
                continue;
            }
            let base = endpoint_fixture(alias);
            let key = if alias { "new:C4022" } else { "fixture" };
            let resolved = if alias {
                "axis.coastal.port.C4022"
            } else {
                "fixture"
            };
            let execute = |state: &mut State| {
                if unload_order {
                    unload(content(), state, "axis.coastal.a", key, stores(1))
                } else {
                    load(content(), state, "axis.coastal.a", key, stores(1))
                }
            };
            execute(&mut base.clone()).unwrap(); // same otherwise-valid stock operation
            for endpoint in [
                CargoSite::Ship("axis.coastal.a".into()),
                CargoSite::Dump(resolved.into()),
            ] {
                let mut state = base.clone();
                current_tag(&mut state, endpoint, 12);
                let before = serde_json::to_value(&state).unwrap();
                assert_eq!(execute(&mut state), Err(CoastalOperationError::TaggedRoute));
                assert_eq!(serde_json::to_value(&state).unwrap(), before);
            }
        }
    }
}
/// Cases: airlog:53.25, airlog:56.34, land:3.6
#[test]
fn coastal_malformed_second_history_precedes_tag_but_generic_invalid_precedes_histories() {
    use super::super::cargo_history::{CargoError, CargoSite};
    let mut state = endpoint_fixture(false);
    current_tag(&mut state, CargoSite::Dump("fixture".into()), 12);
    current_tag(&mut state, CargoSite::Ship("axis.coastal.a".into()), -1);
    let before = serde_json::to_value(&state).unwrap();
    assert_eq!(
        load(
            content(),
            &mut state,
            "axis.coastal.a",
            "fixture",
            stores(1)
        ),
        Err(CoastalOperationError::History(CargoError::Invalid))
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    assert_eq!(
        load(
            content(),
            &mut state,
            "axis.coastal.a",
            "fixture",
            stores(-1)
        ),
        Err(SupplyError::Invalid.into())
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    // Valid originals are untagged after stage expiry. Even the old malformed
    // record is ignored by the same validated stage query, with no new seed.
    state.cursor.op_stage = Some(2);
    load(
        content(),
        &mut state,
        "axis.coastal.a",
        "fixture",
        stores(1),
    )
    .unwrap();
}
fn cw_endpoint_fixture() -> (CnaContent, State) {
    let mut c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut p = c
        .places
        .places
        .values()
        .find(|p| p.kind == "port")
        .unwrap()
        .clone();
    p.id = "coastal-history-test-port".into();
    p.hex_id = "C4024".into();
    p.name = "Fixture".into();
    c.places.places.insert(p.id.clone(), p);
    let mut state = setup();
    state.cursor.block = Block::OpStage;
    state.cursor.half = None;
    state.cursor.index = OPSTAGE
        .iter()
        .position(|p| p.anchor == "opstage.organization.tactical_shipping")
        .unwrap();
    for (key, hex) in [
        ("origin", "C4022"),
        ("commonwealth.coastal.port.C4024", "C4024"),
    ] {
        let at = Location::Hex { hex: hex.into() };
        ports::record_entry_fixture(&c, &mut state, Side::Commonwealth, &at);
        dump(&mut state, key, Side::Commonwealth, at, stores(20));
    }
    (c, state)
}
/// Cases: airlog:53.25, airlog:55.14, land:3.6
#[test]
fn cw_source_and_resolved_alias_target_guard_preserve_both_port_budgets() {
    use super::super::cargo_history::CargoSite;
    let (c, base) = cw_endpoint_fixture();
    commonwealth_transfer(&c, &mut base.clone(), "origin", "new:C4024", stores(1)).unwrap();
    for site in [
        CargoSite::Dump("origin".into()),
        CargoSite::Dump("commonwealth.coastal.port.C4024".into()),
    ] {
        let mut state = base.clone();
        current_tag(&mut state, site, 12);
        let before = serde_json::to_value(&state).unwrap();
        assert_eq!(
            commonwealth_transfer(&c, &mut state, "origin", "new:C4024", stores(1)),
            Err(CoastalOperationError::TaggedRoute)
        );
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
    }
    let mut state = base;
    current_tag(&mut state, CargoSite::Dump("origin".into()), 12);
    current_tag(
        &mut state,
        CargoSite::Dump("commonwealth.coastal.port.C4024".into()),
        -1,
    );
    assert!(matches!(
        commonwealth_transfer(&c, &mut state, "origin", "new:C4024", stores(1)),
        Err(CoastalOperationError::History(_))
    ));
    state.cursor.op_stage = Some(2);
    commonwealth_transfer(&c, &mut state, "origin", "new:C4024", stores(1)).unwrap();
}
fn guarded_response(p: &Pending, action: Value) -> cna_core::engine::Command {
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
fn guarded_open(strict: bool, state: State) -> cna_core::engine::Game<crate::Cna> {
    use cna_core::engine::{Command, Game, evaluate};
    let mut state = state;
    state.setup.closed = true;
    state.cursor.entered = false;
    state.turn.weather = Some(crate::state::WeatherState {
        kind: cna_tables::land::weather::WeatherKind::Normal,
        storm_sections: vec![],
    });
    evaluate(
        &crate::Cna { strict },
        content(),
        &Game {
            state,
            rng: CampaignRng::from_seed([4; 32]).state(),
        },
        &Command::Advance,
    )
    .unwrap()
    .game
}
/// Cases: airlog:53.25, airlog:56.34, land:3.6
#[test]
fn real_coastal_respond_guards_both_profiles_and_buffered_closure_checkpoint_late_fault() {
    use super::super::cargo_history::CargoSite;
    use cna_core::engine::{Command, evaluate};
    let order = json!({"operation":"load","ship":"axis.coastal.a","dump":"fixture","cargo":stores(1),"path":[]});
    for strict in [false, true] {
        let rules = crate::Cna { strict };
        let opened = guarded_open(strict, endpoint_fixture(false));
        let pending = opened
            .state
            .decisions
            .pending
            .iter()
            .find(|p| p.kind == AXIS)
            .unwrap();
        let bad = json!([order.clone(),{"operation":"load","ship":"missing","dump":"fixture","cargo":stores(1),"path":[]}]);
        let before = serde_json::to_value(&opened).unwrap();
        assert!(evaluate(&rules, content(), &opened, &guarded_response(pending, bad)).is_err());
        assert_eq!(serde_json::to_value(&opened).unwrap(), before);
        for site in [
            CargoSite::Dump("fixture".into()),
            CargoSite::Ship("axis.coastal.a".into()),
        ] {
            let mut tagged = opened.clone();
            current_tag(&mut tagged.state, site, 12);
            let bytes = serde_json::to_value(&tagged).unwrap();
            let error = evaluate(
                &rules,
                content(),
                &tagged,
                &guarded_response(pending, json!([order.clone()])),
            )
            .unwrap_err();
            assert!(
                matches!(error,Rejection::Engine(EngineError::Unsupported{case,..}) if case=="airlog:53.25")
            );
            assert_eq!(serde_json::to_value(&tagged).unwrap(), bytes);
        }
        let accepted_transition = evaluate(
            &rules,
            content(),
            &opened,
            &guarded_response(pending, json!([order.clone()])),
        )
        .unwrap();
        let prior_events = accepted_transition.events;
        let accepted = accepted_transition.game;
        assert_eq!(accepted.rng, opened.rng);
        assert_eq!(
            serde_json::to_value(&accepted.state.logistics.dumps).unwrap(),
            serde_json::to_value(&opened.state.logistics.dumps).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&accepted.state.logistics.coastal_ships).unwrap(),
            serde_json::to_value(&opened.state.logistics.coastal_ships).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&accepted.state.logistics.ports).unwrap(),
            serde_json::to_value(&opened.state.logistics.ports).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&accepted.state.logistics.cargo_history).unwrap(),
            serde_json::to_value(&opened.state.logistics.cargo_history).unwrap()
        );
        let saved = serde_json::from_value(serde_json::to_value(&accepted).unwrap()).unwrap();
        let a = evaluate(&rules, content(), &accepted, &Command::Advance).unwrap();
        let b = evaluate(&rules, content(), &saved, &Command::Advance).unwrap();
        assert_eq!(
            serde_json::to_value(&a.game).unwrap(),
            serde_json::to_value(&b.game).unwrap()
        );
        assert_eq!(a.events, b.events);
        assert_eq!(a.game.state.logistics.dumps["fixture"].supplies.stores, 19);
        assert_eq!(
            a.game.state.logistics.coastal_ships["axis.coastal.a"]
                .cargo
                .stores,
            11
        );
        assert_eq!(
            a.game.state.logistics.coastal_ships["axis.coastal.a"].cp_quarters,
            20
        );
        let repeated = evaluate(&rules, content(), &a.game, &Command::Advance).unwrap();
        assert_eq!(
            serde_json::to_value(&repeated.game.state.logistics.dumps).unwrap(),
            serde_json::to_value(&a.game.state.logistics.dumps).unwrap()
        );
        // Late faults are synthetic component witnesses at a real buffered closure.
        // Both the valid tag and malformed current tag must survive checkpoint/retry
        // as the same typed rejection, without publishing any draft effects/events.
        for cp in [12, -1] {
            let mut failed = accepted.clone();
            current_tag(&mut failed.state, CargoSite::Dump("fixture".into()), cp);
            let expected = if cp < 0 {
                Rejection::Engine(EngineError::Invariant {
                    detail: "invalid current owner cargo history at coastal endpoint: Invalid"
                        .into(),
                })
            } else {
                Rejection::Engine(EngineError::Unsupported {
                    case: "airlog:53.25".into(),
                    detail: "current tagged cargo touches unintegrated coastal transport".into(),
                })
            };
            let bytes = serde_json::to_value(&failed).unwrap();
            let recovered = serde_json::from_value(bytes.clone()).unwrap();
            let prior = serde_json::to_value(&prior_events).unwrap();
            for source in [&failed, &recovered, &failed] {
                assert_eq!(
                    evaluate(&rules, content(), source, &Command::Advance).unwrap_err(),
                    expected
                );
                assert_eq!(serde_json::to_value(source).unwrap(), bytes);
                assert_eq!(source.rng, accepted.rng);
            }
            assert_eq!(serde_json::to_value(&prior_events).unwrap(), prior);
            let mut state = failed.state.clone();
            let mut rng = CampaignRng::from_state(&failed.rng);
            let mut events = prior_events.clone();
            let error = rules
                .finish_step(
                    content(),
                    &mut state,
                    &mut Cx {
                        rng: &mut rng,
                        events: &mut events,
                    },
                )
                .unwrap_err();
            assert_eq!(Rejection::Engine(error), expected);
            assert_eq!(serde_json::to_value(&state).unwrap(), bytes["state"]);
            assert_eq!(rng.state(), failed.rng);
            assert_eq!(events, prior_events);
        }
    }
}
/// Cases: airlog:53.25, airlog:56.34, land:3.6
#[test]
fn coastal_foreign_hidden_tags_do_not_change_menu_respond_or_finish_observer_stream() {
    use super::super::cargo_history::CargoSite;
    use cna_core::engine::{Command, evaluate};
    let a = guarded_open(false, endpoint_fixture(false));
    let mut b = a.clone();
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
        .stores = stores(7).stores;
    current_tag(&mut b.state, CargoSite::Unit(enemy), 12);
    crate::testkit::assert_indistinguishable(
        &crate::Cna::dev(),
        content(),
        &a.state,
        &b.state,
        Side::Axis,
    );
    let pending = a
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == AXIS)
        .unwrap();
    let command = guarded_response(
        pending,
        json!([{"operation":"load","ship":"axis.coastal.a","dump":"fixture","cargo":stores(1),"path":[]}]),
    );
    crate::testkit::assert_action_indistinguishable(
        &crate::Cna::dev(),
        content(),
        &a,
        &b,
        &command,
        Side::Axis,
    );
    let accepted_a = evaluate(&crate::Cna::dev(), content(), &a, &command)
        .unwrap()
        .game;
    let accepted_b = evaluate(&crate::Cna::dev(), content(), &b, &command)
        .unwrap()
        .game;
    crate::testkit::assert_action_indistinguishable(
        &crate::Cna::dev(),
        content(),
        &accepted_a,
        &accepted_b,
        &Command::Advance,
        Side::Axis,
    );
    // Own tag histories differ but both owner actions reject with the same gate;
    // the foreign observer receives neither tag quantities nor a changed stream.
    let mut x = a.clone();
    let mut y = a;
    current_tag(&mut x.state, CargoSite::Dump("fixture".into()), 12);
    current_tag(&mut y.state, CargoSite::Ship("axis.coastal.a".into()), 16);
    crate::testkit::assert_action_indistinguishable(
        &crate::Cna::dev(),
        content(),
        &x,
        &y,
        &command,
        Side::Commonwealth,
    );
    crate::testkit::assert_action_indistinguishable(
        &crate::Cna::dev(),
        content(),
        &x,
        &y,
        &Command::Advance,
        Side::Commonwealth,
    );
}
/// Cases: scen:60.44, airlog:53.25, airlog:55.14, land:3.6
#[test]
fn actual_unlimited_base_has_no_fictitious_cargo_site_but_guard_checks_original_target() {
    use super::super::cargo_history::CargoSite;
    let (mut c, mut state) = cw_endpoint_fixture();
    let policy = c.scenario.supply.unlimited_supply.as_ref().unwrap();
    assert_eq!(policy.side, Side::Commonwealth);
    let hex = policy
        .locations
        .iter()
        .flat_map(|id| c.areas.areas[id].hex_ids.iter())
        .find(|hex| **hex != HexId::new("C4024"))
        .unwrap()
        .clone();
    // Explicit test-only port supplies the access fixture. Its position, policy
    // entitlement and debit rules remain actual content; no live port is inferred.
    let mut p = c
        .places
        .places
        .values()
        .find(|p| p.kind == "port")
        .unwrap()
        .clone();
    p.id = "unlimited-base-test-port".into();
    p.name = "Fixture source".into();
    p.hex_id = hex.clone();
    c.places.places.insert(p.id.clone(), p);
    ports::record_entry_fixture(
        &c,
        &mut state,
        Side::Commonwealth,
        &Location::Hex { hex: hex.clone() },
    );
    let source = format!("base:{hex}");
    // A deliberately malformed orphan is component evidence that an unlimited
    // base does not become a Dump endpoint. It is not a scheduled saved-state claim.
    current_tag(&mut state, CargoSite::Dump(source.clone()), -1);
    commonwealth_transfer(&c, &mut state, &source, "new:C4024", stores(1)).unwrap();
    assert!(!state.logistics.dumps.contains_key(&source));
    let mut tagged = state.clone();
    current_tag(
        &mut tagged,
        CargoSite::Dump("commonwealth.coastal.port.C4024".into()),
        12,
    );
    let before = serde_json::to_value(&tagged).unwrap();
    assert_eq!(
        commonwealth_transfer(&c, &mut tagged, &source, "new:C4024", stores(1)),
        Err(CoastalOperationError::TaggedRoute)
    );
    assert_eq!(serde_json::to_value(&tagged).unwrap(), before);
}
