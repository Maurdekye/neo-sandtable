use super::*;
use crate::logistics::port_initialization::PortInitializationErrorKind;
use cna_core::engine::EngineError;

fn content() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}
fn future_policy(c: &mut CnaContent) -> Port {
    let p = at(
        c,
        &Location::Hex {
            hex: "C4022".into(),
        },
    )
    .unwrap();
    let mut record = c.scenario.construction.port_overrides[0].clone();
    record.hex = "C4022".into();
    record.port = "Sollum".into();
    record.efficiency_level = 1;
    // Synthetic unsupported policy, not a claim about a published scenario.
    record.condition = None;
    record.src = vec!["scen:61.1".into()];
    c.scenario.construction.port_overrides = vec![record];
    p
}
fn known(c: &CnaContent, p: &Port) -> PortState {
    PortState {
        owner: Side::Axis,
        efficiency: c
            .tables
            .airlog
            .port_capacity
            .port(p.name)
            .max_efficiency_level,
        blocked_levels: 0,
        mined_levels: 0,
        bombed_stage: None,
        budget_stage: None,
        used_tons24: 0,
    }
}

/// Cases: airlog:55.14, airlog:55.18, scen:60.7
#[test]
fn policy_gate_precedes_every_legacy_numeric_efficiency_including_zero() {
    let mut c = content();
    let p = future_policy(&mut c);
    let mut s = State::new(&c).unwrap();
    let expected = crate::logistics::port_initialization::unsupported_port_policies(&c)
        .unwrap()
        .remove(0);
    for efficiency in [
        0,
        c.tables
            .airlog
            .port_capacity
            .port(p.name)
            .max_efficiency_level,
    ] {
        let mut old = known(&c, &p);
        old.efficiency = efficiency;
        old.blocked_levels = 1;
        old.mined_levels = 1;
        old.used_tons24 = 127;
        old.budget_stage = Some(WaterStage::current(&s));
        s.logistics.ports.insert(p.id.clone(), old);
        let before = serde_json::to_value(&s).unwrap();
        assert_eq!(
            state(&c, &s, &p),
            Err(PortOperationError::Policy(expected.clone()))
        );
        assert_eq!(
            capacity_tons(&c, &s, &p),
            Err(PortOperationError::Policy(expected.clone()))
        );
        s.logistics.unknown_ports.insert(p.id.clone(), Side::Axis);
        assert_eq!(
            state(&c, &s, &p),
            Err(PortOperationError::Policy(expected.clone()))
        );
        s.logistics.unknown_ports.remove(&p.id);
        assert_eq!(
            advance(&c, &mut s, &p),
            Err(PortOperationError::Policy(expected.clone()))
        );
        assert_eq!(
            charge(&c, &mut s, Side::Axis, &p, 127, false),
            Err(PortOperationError::Policy(expected.clone()))
        );
        assert_eq!(
            charge(&c, &mut s, Side::Axis, &p, -1, true),
            Err(PortOperationError::Policy(expected.clone()))
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        assert!(
            matches!(expected.clone().into_engine(), EngineError::Unsupported { case, detail }
            if case == "scen:61.1" && detail.contains("C4022") && detail.contains("raw=1"))
        );
    }
}

/// Cases: airlog:55.11, airlog:55.14, airlog:55.18
#[test]
fn unknown_owner_map_is_not_a_numeric_fallback_and_other_ports_remain_usable() {
    let mut c = content();
    let affected = future_policy(&mut c);
    let healthy = at(
        &c,
        &Location::OffMap {
            id: "box_tripoli".into(),
        },
    )
    .unwrap();
    let mut s = State::new(&c).unwrap();
    s.logistics
        .unknown_ports
        .insert(affected.id.clone(), Side::Axis);
    s.logistics
        .ports
        .insert(healthy.id.clone(), known(&c, &healthy));
    let before = serde_json::to_value(&s).unwrap();
    assert!(matches!(
        capacity_tons(&c, &s, &affected),
        Err(PortOperationError::Policy(_))
    ));
    assert_eq!(
        capacity_tons(&c, &s, &healthy).unwrap(),
        i64::from(c.tables.airlog.port_capacity.port(healthy.name).max_tonnage)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    // Even an old unknown marker without an authored record cannot unlock a numeric state.
    c.scenario.construction.port_overrides.clear();
    s.logistics
        .ports
        .insert(affected.id.clone(), known(&c, &affected));
    assert_eq!(
        capacity_tons(&c, &s, &affected),
        Err(PortOperationError::Supply(SupplyError::Unsupported {
            case: "airlog:55.18"
        }))
    );
}

/// Cases: airlog:55.18, scen:60.7
#[test]
fn full_preflight_uses_all_public_policies_without_a_port_state_or_icon() {
    let mut c = content();
    let p = future_policy(&mut c);
    assert!(
        matches!(preflight(&c, true), Err(EngineError::Unsupported { case, .. })
        if case == "scen:61.1")
    );
    preflight(&c, false).unwrap();
    c.places
        .places
        .retain(|_, record| record.kind != "port" || record.hex_id.as_str() != p.id);
    assert!(at(&c, &p.location).is_err());
    assert!(
        matches!(preflight(&c, true), Err(EngineError::Unsupported { case, .. })
        if case == "scen:61.1")
    );
    preflight(&c, false).unwrap();
}

/// Cases: airlog:55.18, scen:60.7
#[test]
fn malformed_source_keeps_provenance_before_numeric_or_unknown_owner_state() {
    let mut c = content();
    let p = future_policy(&mut c);
    let mut s = State::new(&c).unwrap();
    s.logistics.ports.insert(p.id.clone(), known(&c, &p));
    s.logistics.unknown_ports.insert(p.id.clone(), Side::Axis);
    c.scenario.construction.port_overrides[0].efficiency_level = i32::MAX;
    let before = serde_json::to_value(&s).unwrap();
    let PortOperationError::Policy(error) = state(&c, &s, &p).unwrap_err() else {
        panic!("policy category lost")
    };
    assert_eq!(error.kind, PortInitializationErrorKind::Malformed);
    assert_eq!(error.raw_efficiency, i32::MAX);
    assert_eq!(error.hex.as_str(), "C4022");
    assert_eq!(error.case, "scen:61.1");
    assert!(
        matches!(error.into_engine(), EngineError::Invariant { detail }
        if detail.contains("C4022") && detail.contains("2147483647"))
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}

/// Cases: land:3.6, airlog:55.18
#[test]
fn unknown_ownership_and_retained_numeric_damage_stay_private_and_checkpointed() {
    let mut c = content();
    let p = future_policy(&mut c);
    let mut a = State::new(&c).unwrap();
    let mut old = known(&c, &p);
    old.efficiency = 0;
    old.used_tons24 = 127;
    old.mined_levels = 1;
    a.logistics.ports.insert(p.id.clone(), old);
    let mut b = a.clone();
    b.logistics.unknown_ports.insert(p.id.clone(), Side::Axis);
    b.logistics.ports.get_mut(&p.id).unwrap().used_tons24 = 254;
    crate::testkit::assert_indistinguishable(&crate::Cna::dev(), &c, &a, &b, Side::Commonwealth);
    let saved = serde_json::to_value(&b).unwrap();
    let restored: State = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), saved);
    assert_eq!(capacity_tons(&c, &restored, &p), capacity_tons(&c, &b, &p));
    let mut legacy = saved;
    legacy["logistics"]
        .as_object_mut()
        .unwrap()
        .remove("unknown_ports");
    let legacy: State = serde_json::from_value(legacy).unwrap();
    assert!(legacy.logistics.unknown_ports.is_empty());
    // The immutable policy still denies old numeric state after legacy defaulting.
    assert!(matches!(
        capacity_tons(&c, &legacy, &p),
        Err(PortOperationError::Policy(_))
    ));
}

fn coastal_game(anchor: &str) -> (CnaContent, cna_core::engine::Game<crate::Cna>, Port) {
    let mut c = content();
    let affected = future_policy(&mut c);
    let healthy = at(
        &c,
        &Location::Hex {
            hex: "A4827".into(),
        },
    )
    .unwrap();
    let mut s = State::new(&c).unwrap();
    s.turn.player_a = Some(Side::Axis);
    s.cursor.op_stage = Some(1);
    s.cursor.entered = false;
    if let Some(index) = crate::seq::OPSTAGE.iter().position(|d| d.anchor == anchor) {
        s.cursor.block = crate::seq::Block::OpStage;
        s.cursor.index = index;
    } else {
        s.cursor.block = crate::seq::Block::PlayerHalf;
        s.cursor.half = Some(crate::seq::Half::A);
        s.cursor.index = crate::seq::PLAYER_HALF
            .iter()
            .position(|d| d.anchor == anchor)
            .unwrap();
    }
    for u in s.land.units.values_mut() {
        u.location = Location::NotArrived;
    }
    for port in [&affected, &healthy] {
        let mut ps = known(&c, port);
        ps.owner = Side::Commonwealth;
        s.logistics.ports.insert(port.id.clone(), ps);
        let Location::Hex { hex } = &port.location else {
            unreachable!()
        };
        let id = format!("fixture.{}", port.id);
        s.logistics.dumps.insert(
            id.clone(),
            crate::state::Dump {
                id,
                marker: format!("fixture-marker-{}", port.id),
                side: Side::Commonwealth,
                location: crate::state::DumpLocation::Hex { hex: hex.clone() },
                supplies: Supplies {
                    stores: 10,
                    ..Supplies::default()
                },
                active: true,
                dummy: false,
            },
        );
    }
    let game = cna_core::engine::Game {
        state: s,
        rng: cna_core::dice::CampaignRng::from_seed([22; 32]).state(),
    };
    (c, game, affected)
}

/// Cases: airlog:55.14, airlog:55.18, land:3.6
#[test]
fn coastal_menu_uses_only_known_ports_and_fixed_request_survives_hidden_capacity_and_recovery() {
    use cna_core::{
        decision::DecisionResponse,
        engine::{Command, evaluate},
    };
    let (c, a, affected) = coastal_game("opstage.organization.tactical_shipping");
    let mut b = a.clone();
    b.state
        .logistics
        .ports
        .get_mut(&affected.id)
        .unwrap()
        .efficiency = 0;
    b.state
        .logistics
        .ports
        .get_mut(&affected.id)
        .unwrap()
        .used_tons24 = 127;
    b.state
        .logistics
        .dumps
        .get_mut("fixture.C4022")
        .unwrap()
        .supplies
        .stores = 99;
    crate::testkit::assert_action_indistinguishable(
        &crate::Cna::dev(),
        &c,
        &a,
        &b,
        &Command::Advance,
        Side::Axis,
    );
    let mut advanced = vec![];
    for g in [a, b] {
        let before_rng = g.rng.clone();
        let saved = serde_json::to_value(&g).unwrap();
        let restored = serde_json::from_value(saved).unwrap();
        let t = evaluate(&crate::Cna::dev(), &c, &g, &Command::Advance).unwrap();
        let replay = evaluate(&crate::Cna::dev(), &c, &restored, &Command::Advance).unwrap();
        assert_eq!(
            serde_json::to_value(&t.game).unwrap(),
            serde_json::to_value(&replay.game).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&t.events).unwrap(),
            serde_json::to_value(&replay.events).unwrap()
        );
        assert_eq!(t.game.rng, before_rng);
        let pending = t
            .game
            .state
            .decisions
            .pending
            .iter()
            .find(|p| p.kind == crate::logistics::coastal::CW)
            .unwrap();
        let space = serde_json::to_value(&pending.space).unwrap().to_string();
        assert!(!space.contains("fixture.C4022") && !space.contains("new:C4022"));
        assert!(space.contains("fixture.A4827") && space.contains("new:A4827"));
        let notes = serde_json::to_value(&t.events).unwrap().to_string();
        assert!(notes.contains("scen:61.1") && notes.contains("raw=1"));
        let cmd = Command::Respond(DecisionResponse {
            decision_id: pending.id.clone(),
            seat: pending.seat,
            controller_epoch: 1,
            decision_revision: pending.revision,
            idempotency_key: "port-fixed-pass".into(),
            action: serde_json::Value::Null,
            public_explanation: None,
        });
        let stocks = t.game.state.logistics.dumps.clone();
        let response = evaluate(&crate::Cna::dev(), &c, &t.game, &cmd).unwrap();
        assert_eq!(response.game.state.logistics.dumps, stocks);
        advanced.push(response.game);
    }
    crate::testkit::assert_action_indistinguishable(
        &crate::Cna::dev(),
        &c,
        &advanced[0],
        &advanced[1],
        &Command::Advance,
        Side::Axis,
    );
    for g in advanced {
        let saved = serde_json::to_value(&g).unwrap();
        let restored = serde_json::from_value(saved).unwrap();
        let t = evaluate(&crate::Cna::dev(), &c, &g, &Command::Advance).unwrap();
        let replay = evaluate(&crate::Cna::dev(), &c, &restored, &Command::Advance).unwrap();
        assert_eq!(
            serde_json::to_value(&t.game).unwrap(),
            serde_json::to_value(&replay.game).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&t.events).unwrap(),
            serde_json::to_value(&replay.events).unwrap()
        );
        assert_eq!(t.game.state.logistics.dumps, g.state.logistics.dumps);
    }
}

/// Cases: airlog:55.18, airlog:56.0, land:3.6
#[test]
fn full_coastal_dispatcher_and_convoy_entry_preflight_before_private_noop_branches() {
    use cna_core::engine::{Command, evaluate};
    for anchor in [
        "opstage.organization.tactical_shipping",
        "opstage.truck_convoy_movement",
    ] {
        let (c, a, affected) = coastal_game(anchor);
        let mut b = a.clone();
        b.state.logistics.ports.clear();
        b.state
            .logistics
            .dumps
            .get_mut("fixture.C4022")
            .unwrap()
            .supplies
            .stores = 99;
        crate::testkit::assert_action_indistinguishable(
            &crate::Cna::full(),
            &c,
            &a,
            &b,
            &Command::Advance,
            Side::Axis,
        );
        for g in [&a, &b] {
            let saved = serde_json::to_value(g).unwrap();
            assert!(
                matches!(evaluate(&crate::Cna::full(), &c, g, &Command::Advance),
                Err(cna_core::engine::Rejection::Engine(EngineError::Unsupported { case, detail }))
                if case == "scen:61.1" && detail.contains(&affected.id))
            );
            assert_eq!(serde_json::to_value(g).unwrap(), saved);
        }
        for mode in [0, 1, 2, 3] {
            let mut s = a.state.clone();
            s.logistics.convoys_initialized = true;
            if mode < 2 {
                let kind = if mode == 0 {
                    crate::logistics::coastal::CW
                } else {
                    crate::logistics::coastal::AXIS
                };
                s.logistics
                    .allocation_batches
                    .completed
                    .insert(crate::logistics::batches::batch_key(&s, kind));
            }
            let saved = serde_json::to_value(&s).unwrap();
            let mut rng = cna_core::dice::CampaignRng::from_seed([23; 32]);
            let before_rng = rng.state();
            let mut events = vec![];
            let mut cx = cna_core::engine::Cx {
                rng: &mut rng,
                events: &mut events,
            };
            let result = match mode {
                0 => crate::logistics::coastal::enter_cw(&c, &mut s, true, &mut cx),
                1 => crate::logistics::coastal::enter_axis(&c, &mut s, true, &mut cx),
                2 => crate::logistics::convoys::initialize(&c, &mut s, true, &mut cx),
                _ => crate::logistics::convoys::schedule(&c, &mut s, true, &mut cx),
            };
            assert!(
                matches!(result, Err(EngineError::Unsupported { case, .. }) if case == "scen:61.1")
            );
            assert_eq!(serde_json::to_value(&s).unwrap(), saved);
            assert_eq!(rng.state(), before_rng);
            assert!(events.is_empty());
        }
    }
}

/// Missing authored setup cannot become listed maximum through legacy state.
/// Cases: airlog:55.18, scen:60.7, land:3.6
#[test]
fn missing_general_entry_is_owner_only_and_never_returns_legacy_capacity() {
    use cna_core::visibility::Perspective;
    let mut c = content();
    c.scenario.construction.port_policy = None;
    let p = at(
        &c,
        &Location::Hex {
            hex: "C4022".into(),
        },
    )
    .unwrap();
    let mut s = State::new(&c).unwrap();
    let mut old = known(&c, &p);
    old.efficiency = 0;
    old.used_tons24 = 127;
    old.mined_levels = 1;
    s.logistics.ports.insert(p.id.clone(), old.clone());
    let before = s.clone();
    let mut events = Vec::new();
    record_entry(
        &c,
        &mut s,
        Side::Commonwealth,
        &p.location,
        false,
        &mut events,
    )
    .unwrap();
    assert_eq!(s.logistics.unknown_ports[&p.id], Side::Commonwealth);
    assert_eq!(s.logistics.ports[&p.id], old);
    assert!(matches!(
        capacity_tons(&c, &s, &p),
        Err(PortOperationError::Starting(_))
    ));
    assert!(
        events
            .iter()
            .all(|e| !Perspective::Side(Side::Axis).can_see(&e.audience))
    );
    assert!(
        serde_json::to_value(&events)
            .unwrap()
            .to_string()
            .contains("initial port default has not been authored")
    );
    crate::testkit::assert_indistinguishable(&crate::Cna::dev(), &c, &before, &s, Side::Axis);
    let checkpoint = serde_json::to_value(&s).unwrap();
    let restored: State = serde_json::from_value(checkpoint.clone()).unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), checkpoint);
    assert_eq!(capacity_tons(&c, &restored, &p), capacity_tons(&c, &s, &p));
    events.clear();
    record_entry(&c, &mut s, Side::Axis, &p.location, false, &mut events).unwrap();
    assert_eq!(s.logistics.unknown_ports[&p.id], Side::Axis);
    assert_eq!(s.logistics.ports[&p.id], old);
    assert!(
        events
            .iter()
            .all(|e| e.audience == Audience::Side(Side::Axis))
    );
}

/// Content stops precede no-icon, already initialized, and no-inventory branches.
/// Cases: airlog:55.18, scen:60.7
#[test]
fn full_initializer_and_entry_are_uniform_and_preserve_state_rng_and_events() {
    let mut c = content();
    c.scenario.construction.port_policy = None;
    let p = at(
        &c,
        &Location::OffMap {
            id: "box_tripoli".into(),
        },
    )
    .unwrap();
    for existing in [false, true] {
        let mut s = State::new(&c).unwrap();
        if existing {
            s.logistics.ports.insert(p.id.clone(), known(&c, &p));
        }
        let before = serde_json::to_value(&s).unwrap();
        let mut rng = cna_core::dice::CampaignRng::from_seed([43; 32]);
        let rng_before = rng.state();
        let mut events = vec![EngineEvent::new(
            Audience::Side(Side::Axis),
            GameEvent::Note {
                text: "prior".into(),
            },
        )];
        let events_before = events.clone();
        let expected = super::preflight(&c, true).unwrap_err();
        assert_eq!(
            initialize(
                &c,
                &mut s,
                true,
                &mut cna_core::engine::Cx {
                    rng: &mut rng,
                    events: &mut events
                }
            ),
            Err(expected.clone())
        );
        for location in [&p.location, &Location::NotArrived] {
            assert_eq!(
                record_entry(&c, &mut s, Side::Axis, location, true, &mut events),
                Err(expected.clone())
            );
        }
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        assert_eq!(rng.state(), rng_before);
        assert_eq!(events, events_before);
    }
    c.scenario.construction.port_policy = content().scenario.construction.port_policy;
    let mut s = State::new(&c).unwrap();
    let before = serde_json::to_value(&s).unwrap();
    let mut events = Vec::new();
    record_entry(
        &c,
        &mut s,
        Side::Axis,
        &Location::NotArrived,
        true,
        &mut events,
    )
    .unwrap();
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert!(events.is_empty());
}

/// Authored listed maxima and the cited exception initialize once, preserving damage.
/// Synthetic port icon is a fixture premise, not a published map verification.
/// Cases: scen:60.7, scen:60.23, airlog:55.25, airlog:55.3, land:30.58
#[test]
fn authored_initialization_and_capture_preserve_damage_budget_and_blockage() {
    for scenario in ["graziani", "italian_campaign"] {
        let mut c = CnaContent::load(&cna_content::repo_data_dir(), scenario).unwrap();
        let mut icon = c.places.places["port-sollum"].clone();
        icon.id = "synthetic-tobruk-port".into();
        icon.name = "Tobruk".into();
        icon.hex_id = "C4807".into();
        c.places.places.insert(icon.id.clone(), icon);
        let p = at(
            &c,
            &Location::Hex {
                hex: "C4807".into(),
            },
        )
        .unwrap();
        let mut s = State::new(&c).unwrap();
        s.cursor.op_stage = Some(1);
        let mut events = Vec::new();
        record_entry(&c, &mut s, Side::Axis, &p.location, true, &mut events).unwrap();
        let maximum = c
            .tables
            .airlog
            .port_capacity
            .port(p.name)
            .max_efficiency_level;
        assert_eq!(s.logistics.ports[&p.id].efficiency, maximum - 3);
        assert_eq!(s.logistics.ports[&p.id].blocked_levels, 3);
        let row = s.logistics.ports.get_mut(&p.id).unwrap();
        row.efficiency = 1;
        row.used_tons24 = 127;
        row.bombed_stage = Some(WaterStage::current(&State::new(&c).unwrap()));
        let mut retained = row.clone();
        retained.owner = Side::Commonwealth;
        record_entry(
            &c,
            &mut s,
            Side::Commonwealth,
            &p.location,
            true,
            &mut events,
        )
        .unwrap();
        assert_eq!(s.logistics.ports[&p.id], retained);
        s.cursor.op_stage = Some(2);
        advance(&c, &mut s, &p).unwrap();
        assert_eq!(s.logistics.ports[&p.id].blocked_levels, 3);
        assert!(s.logistics.ports[&p.id].efficiency <= maximum - 3);
        assert!(events.is_empty());
    }
}

/// General Unknown setup preserves the same dispatcher requests and public stream.
/// Cases: airlog:55.18, land:3.6
#[test]
fn missing_general_fixed_coastal_rounds_survive_hidden_inventory_and_checkpoint() {
    use cna_core::engine::{Command, evaluate};
    for anchor in [
        "opstage.organization.tactical_shipping",
        "opstage.truck_convoy_movement",
    ] {
        let (mut c, a, _) = coastal_game(anchor);
        c.scenario.construction.port_policy = None;
        c.scenario.construction.port_overrides.clear();
        let mut b = a.clone();
        b.state.logistics.ports.clear();
        b.state
            .logistics
            .dumps
            .get_mut("fixture.C4022")
            .unwrap()
            .supplies
            .stores = 99;
        for rules in [crate::Cna::dev(), crate::Cna::full()] {
            crate::testkit::assert_action_indistinguishable(
                &rules,
                &c,
                &a,
                &b,
                &Command::Advance,
                Side::Axis,
            );
        }
        for g in [&a, &b] {
            let checkpoint = serde_json::to_value(g).unwrap();
            let restored = serde_json::from_value(checkpoint).unwrap();
            let transition = evaluate(&crate::Cna::dev(), &c, g, &Command::Advance).unwrap();
            let replay = evaluate(&crate::Cna::dev(), &c, &restored, &Command::Advance).unwrap();
            assert_eq!(
                serde_json::to_value(&transition.game).unwrap(),
                serde_json::to_value(&replay.game).unwrap()
            );
            assert_eq!(transition.events, replay.events);
            assert_eq!(transition.game.rng, g.rng);
            assert_eq!(
                transition.game.state.logistics.dumps,
                g.state.logistics.dumps
            );
            assert_eq!(transition.game.state.decisions.pending.len(), 1);
        }
    }
}

/// A fixture adjustment cannot weaken production's malformed geometry refusal.
/// Cases: airlog:55.18, scen:60.7
#[test]
fn malformed_anchor_refuses_runtime_before_any_entry_or_initialization_effect() {
    let mut c = content();
    let mut s = State::new(&c).unwrap();
    c.scenario.construction.port_policy = None;
    c.scenario.construction.port_overrides[0].hex = "outside-map-fixture".into();
    let expected = preflight(&c, false).unwrap_err();
    assert!(matches!(&expected, EngineError::Invariant { detail }
        if detail.contains("outside-map-fixture") && detail.contains("scen:60.7")));
    let before = serde_json::to_value(&s).unwrap();
    for strict in [false, true] {
        let mut rng = cna_core::dice::CampaignRng::from_seed([45; 32]);
        let rng_before = rng.state();
        let mut events = Vec::new();
        assert_eq!(
            initialize(
                &c,
                &mut s,
                strict,
                &mut cna_core::engine::Cx {
                    rng: &mut rng,
                    events: &mut events
                }
            ),
            Err(expected.clone())
        );
        assert_eq!(
            record_entry(
                &c,
                &mut s,
                Side::Axis,
                &Location::NotArrived,
                strict,
                &mut events
            ),
            Err(expected.clone())
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        assert_eq!(rng.state(), rng_before);
        assert!(events.is_empty());
    }
}
