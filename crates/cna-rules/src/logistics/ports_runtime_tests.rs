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
            capacity_at(&c, &s, &p),
            Err(PortOperationError::Policy(expected.clone()))
        );
        s.logistics.unknown_ports.insert(p.id.clone(), Side::Axis);
        assert_eq!(
            state(&c, &s, &p),
            Err(PortOperationError::Policy(expected.clone()))
        );
        s.logistics.unknown_ports.remove(&p.id);
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
        capacity_at(&c, &s, &affected),
        Err(PortOperationError::Policy(_))
    ));
    assert_eq!(
        capacity_at(&c, &s, &healthy).unwrap(),
        i64::from(c.tables.airlog.port_capacity.port(healthy.name).max_tonnage)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    // Even an old unknown marker without an authored record cannot unlock a numeric state.
    c.scenario.construction.port_overrides.clear();
    s.logistics
        .ports
        .insert(affected.id.clone(), known(&c, &affected));
    assert_eq!(
        capacity_at(&c, &s, &affected),
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
    assert_eq!(capacity_at(&c, &restored, &p), capacity_at(&c, &b, &p));
    let mut legacy = saved;
    legacy["logistics"]
        .as_object_mut()
        .unwrap()
        .remove("unknown_ports");
    let legacy: State = serde_json::from_value(legacy).unwrap();
    assert!(legacy.logistics.unknown_ports.is_empty());
    // The immutable policy still denies old numeric state after legacy defaulting.
    assert!(matches!(
        capacity_at(&c, &legacy, &p),
        Err(PortOperationError::Policy(_))
    ));
}
