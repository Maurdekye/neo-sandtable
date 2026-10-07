use super::*;
use crate::logistics::ports;
use cna_content::scenario::construction::{PortStartingPolicy, ScenarioConstruction};

fn content() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}
fn policy(c: &mut CnaContent, default: Option<PortDefaultPolicy>) {
    c.scenario.construction.port_policy = Some(PortStartingPolicy {
        default,
        src: vec!["scen:60.7".into()],
    });
}
fn listed(c: &mut CnaContent) {
    policy(
        c,
        Some(PortDefaultPolicy::Known(KnownPortDefault::ListedMax)),
    );
}
fn verified(c: &mut CnaContent, hex: &str, name: &str) -> Port {
    let canonical = c.map.canonical(&hex.into()).unwrap().clone();
    c.places
        .places
        .retain(|_, place| place.hex_id != canonical || place.kind != "port");
    let mut place = c.places.places["port-sollum"].clone();
    place.id = "00-start-policy-fixture".into();
    place.name = name.into();
    place.hex_id = canonical.clone();
    c.places.places.insert(place.id.clone(), place);
    ports::at(c, &Location::Hex { hex: canonical }).unwrap()
}
fn exception(c: &CnaContent, hex: &str, name: &str) -> PortOverride {
    let mut record = c.scenario.construction.port_overrides[0].clone();
    record.hex = hex.into();
    record.port = name.into();
    record.src = vec!["scen:99.1".into()];
    record.efficiency_level = 1;
    record.condition = Some(PortStartingCondition::Unsupported("future_rule".into()));
    record
}

/// Cases: scen:60.7, scen:60.23, airlog:55.18, airlog:55.25, airlog:55.3
#[test]
fn authored_general_maximum_and_exact_tobruk_exception_preserve_inherited_source() {
    for scenario in ["graziani", "italian_campaign"] {
        let mut c = CnaContent::load(&cna_content::repo_data_dir(), scenario).unwrap();
        listed(&mut c);
        c.scenario.construction.port_overrides[0].condition = Some(PortStartingCondition::Known(
            KnownPortCondition::SanGiorgioPresent,
        ));
        let tobruk = verified(&mut c, "C4807", "Tobruk");
        let ordinary = verified(&mut c, "C4021", "Sollum");
        let before = c.scenario.construction.clone();
        assert!(port_starting_diagnostics(&c).unwrap().is_empty());
        assert_eq!(
            initial_port_starting_policy(&c, &ordinary).unwrap(),
            InitialPortStartingPolicy::Known(InitialPortCondition {
                efficiency: c
                    .tables
                    .airlog
                    .port_capacity
                    .port(ordinary.name)
                    .max_efficiency_level,
                blocked_levels: 0,
            })
        );
        assert_eq!(
            initial_port_starting_policy(&c, &tobruk).unwrap(),
            InitialPortStartingPolicy::Known(InitialPortCondition {
                efficiency: c
                    .tables
                    .airlog
                    .port_capacity
                    .port(PortName::Tobruk)
                    .max_efficiency_level
                    - 3,
                blocked_levels: 3,
            })
        );
        assert_eq!(
            c.scenario.construction.port_overrides[0].efficiency_level,
            7
        );
        assert!(
            c.scenario
                .construction
                .source_path()
                .unwrap()
                .ends_with("graziani/construction.toml")
        );
        assert_eq!(c.scenario.construction, before);
    }
}
/// Cases: airlog:55.18
#[test]
fn missing_general_has_truthful_source_free_provenance_and_exact_fallback_error() {
    let mut c = content();
    let port = verified(&mut c, "C4021", "Sollum");
    c.scenario.construction = ScenarioConstruction::default();
    let diagnostics = port_starting_diagnostics(&c).unwrap();
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = diagnostics[0].clone();
    let PortStartingDiagnostic::General(general) = &diagnostic else {
        panic!("general gap");
    };
    assert_eq!(general.source_path, None);
    assert!(general.src.is_empty());
    assert_eq!(general.authored_policy, None);
    assert_eq!(general.scenario_id, "graziani");
    assert_eq!(general.case, "airlog:55.18");
    assert_eq!(
        initial_port_starting_policy(&c, &port).unwrap(),
        InitialPortStartingPolicy::Unknown(diagnostic.clone())
    );
    let detail = diagnostic.to_string();
    assert!(!detail.contains("raw="));
    assert!(!detail.contains("hex="));
    assert!(
        matches!(diagnostic.into_engine(), EngineError::Unsupported {case, detail: actual}
        if case == "airlog:55.18" && actual == detail)
    );
    preflight_port_starting(&c).unwrap();
}
/// Cases: scen:60.7, airlog:55.18, airlog:55.25
#[test]
fn healthy_explicit_exception_survives_missing_or_unknown_general_in_dev_query() {
    for default in [
        None,
        Some(PortDefaultPolicy::Unsupported("future_general".into())),
    ] {
        let mut c = content();
        policy(&mut c, default);
        let tobruk = verified(&mut c, "C4807", "Tobruk");
        assert!(matches!(
            initial_port_starting_policy(&c, &tobruk).unwrap(),
            InitialPortStartingPolicy::Known(_)
        ));
        assert_eq!(port_starting_diagnostics(&c).unwrap().len(), 1);
        assert!(matches!(
            port_starting_diagnostics(&c).unwrap()[0],
            PortStartingDiagnostic::General(_)
        ));
    }
}
/// Cases: airlog:55.18, airlog:55.3
#[test]
fn unknown_exception_never_falls_through_to_authored_general_maximum() {
    let mut c = content();
    listed(&mut c);
    let port = verified(&mut c, "C4021", "Sollum");
    c.scenario
        .construction
        .port_overrides
        .push(exception(&c, "C4021", "Sollum"));
    let result = initial_port_starting_policy(&c, &port).unwrap();
    assert!(matches!(
        result,
        InitialPortStartingPolicy::Unknown(PortStartingDiagnostic::Override(_))
    ));
    assert_eq!(port_starting_diagnostics(&c).unwrap().len(), 1);
}
/// Cases: scen:60.7, airlog:55.18
#[test]
fn complete_scan_prioritizes_malformed_after_general_and_override_diagnostics() {
    for default in [
        None,
        Some(PortDefaultPolicy::Unsupported("future_general".into())),
    ] {
        let mut c = content();
        policy(&mut c, default);
        let future = exception(&c, "C4021", "Sollum");
        c.scenario.construction.port_overrides.insert(0, future);
        c.scenario.construction.port_overrides[1].efficiency_level = 99;
        let before = c.scenario.construction.clone();
        let error = port_starting_diagnostics(&c).unwrap_err();
        let PortStartingDiagnostic::Override(record) = error else {
            panic!("raw99 failure");
        };
        assert_eq!(record.kind, PortInitializationErrorKind::Malformed);
        assert_eq!(record.raw_efficiency, 99);
        assert_eq!(record.case, "scen:60.7");
        assert_eq!(c.scenario.construction, before);
    }
}
/// Cases: scen:60.7, airlog:55.18
#[test]
fn malformed_general_and_future_policy_preserve_actual_provenance_and_error_variant() {
    let mut c = content();
    policy(&mut c, Some(PortDefaultPolicy::Unsupported(" ".into())));
    let error = preflight_port_starting(&c).unwrap_err();
    let PortStartingDiagnostic::General(general) = &error else {
        panic!("general malformed");
    };
    assert_eq!(general.case, "scen:60.7");
    assert_eq!(general.src, vec!["scen:60.7"]);
    assert!(
        general
            .source_path
            .as_ref()
            .unwrap()
            .ends_with("graziani/construction.toml")
    );
    assert_eq!(general.kind, PortInitializationErrorKind::Malformed);
    let detail = error.to_string();
    assert!(
        matches!(error.into_engine(), EngineError::Invariant{detail:actual} if actual == detail)
    );
    policy(
        &mut c,
        Some(PortDefaultPolicy::Unsupported("future_general".into())),
    );
    preflight_port_starting(&c).unwrap();
    assert_eq!(port_starting_diagnostics(&c).unwrap().len(), 1);
}
/// Cases: scen:60.7, airlog:55.18, airlog:55.25
#[test]
fn legacy_exception_apis_and_raw7_provenance_are_unchanged() {
    let mut c = content();
    let port = verified(&mut c, "C4807", "Tobruk");
    let before = super::super::initial_port_policy(&c, &port).unwrap();
    policy(
        &mut c,
        Some(PortDefaultPolicy::Unsupported("future_general".into())),
    );
    assert_eq!(
        super::super::initial_port_policy(&c, &port).unwrap(),
        before
    );
    c.scenario.construction.port_overrides[0].efficiency_level = 99;
    let legacy = super::super::preflight_port_overrides(&c).unwrap_err();
    assert_eq!(
        preflight_port_starting(&c).unwrap_err(),
        PortStartingDiagnostic::Override(legacy)
    );
}
/// Cases: scen:60.7, airlog:55.18, airlog:55.25
#[test]
fn explicit_condition_does_not_normalize_changed_raw_or_unknown_legacy_tag() {
    let mut c = content();
    listed(&mut c);
    c.scenario.construction.port_overrides[0].condition = Some(PortStartingCondition::Known(
        KnownPortCondition::SanGiorgioPresent,
    ));
    c.scenario.construction.port_overrides[0].efficiency_level = 2;
    assert_eq!(
        preflight_port_starting(&c).unwrap_err().kind(),
        PortInitializationErrorKind::Malformed
    );
    c.scenario.construction.port_overrides[0].efficiency_level = 7;
    c.scenario.construction.port_overrides[0].condition =
        Some(PortStartingCondition::Unsupported("future_rule".into()));
    assert!(matches!(
        port_starting_diagnostics(&c).unwrap()[0],
        PortStartingDiagnostic::Override(_)
    ));
}

/// Cases: scen:60.7, scen:60.23, airlog:55.18
#[test]
fn actual_loader_preserves_missing_future_and_malformed_starting_policy() {
    let fixture = super::super::tests::DataFixture::new();
    let path = fixture.root.join("scenarios/graziani/construction.toml");
    let original = std::fs::read_to_string(&path).unwrap();
    // Strip only the optional general table from the copied source fixture.
    let mut missing = String::new();
    let mut in_policy = false;
    for line in original.lines() {
        if line.trim() == "[port_policy]" {
            in_policy = true;
        } else if in_policy && line.trim().starts_with('[') {
            in_policy = false;
        }
        if !in_policy {
            missing.push_str(line);
            missing.push('\n');
        }
    }
    std::fs::write(&path, &missing).unwrap();
    let omitted = CnaContent::load(&fixture.root, "graziani").unwrap();
    assert!(omitted.scenario.construction.port_policy.is_none());
    let gaps = port_starting_diagnostics(&omitted).unwrap();
    assert_eq!(gaps.len(), 1);
    let PortStartingDiagnostic::General(gap) = &gaps[0] else {
        panic!("missing general policy");
    };
    assert_eq!(gap.case, "scen:60.7");
    assert_eq!(gap.source_path.as_deref(), Some(path.as_path()));
    assert_eq!(gap.authored_policy, None);
    assert!(
        omitted
            .source_files()
            .contains(&cna_content::normalize(&path))
    );
    let future =
        format!("{missing}\n[port_policy]\ndefault = \"future_general\"\nsrc = [\"scen:99.1\"]\n");
    std::fs::write(&path, &future).unwrap();
    for scenario in ["graziani", "italian_campaign"] {
        let loaded = CnaContent::load(&fixture.root, scenario).unwrap();
        let diagnostics = port_starting_diagnostics(&loaded).unwrap();
        assert_eq!(diagnostics.len(), 1);
        let PortStartingDiagnostic::General(gap) = &diagnostics[0] else {
            panic!("future general policy");
        };
        assert_eq!(gap.scenario_id, scenario);
        assert_eq!(gap.source_path.as_deref(), Some(path.as_path()));
        assert_eq!(gap.case, "scen:99.1");
        assert_eq!(gap.authored_policy.as_deref(), Some("future_general"));
        assert_eq!(gap.kind, PortInitializationErrorKind::UnsupportedPolicy);
    }
    // A diagnostic general policy cannot hide a malformed later exception.
    std::fs::write(
        &path,
        future.replace("efficiency_level = 7", "efficiency_level = 99"),
    )
    .unwrap();
    let error = CnaContent::load(&fixture.root, "graziani").unwrap_err();
    assert!(error.contains("Malformed"), "{error}");
    assert!(error.contains("raw=99"), "{error}");
    assert!(error.contains("scen:60.7"), "{error}");
    std::fs::write(&path, future.replace("future_general", " ")).unwrap();
    let error = CnaContent::load(&fixture.root, "graziani").unwrap_err();
    assert!(error.contains("construction.toml"), "{error}");
}

/// Cases: scen:60.7, airlog:55.18
#[test]
fn authored_rows_need_header_and_general_malformed_wins_over_override_diagnostic() {
    let mut c = content();
    listed(&mut c);
    c.scenario.construction.file = None;
    assert_eq!(
        preflight_port_starting(&c).unwrap_err().kind(),
        PortInitializationErrorKind::Malformed
    );
    c.scenario.construction.port_policy = None;
    assert_eq!(
        preflight_port_starting(&c).unwrap_err().kind(),
        PortInitializationErrorKind::Malformed
    );
    let mut c = content();
    listed(&mut c);
    c.scenario.construction.port_overrides[0].condition = None;
    c.scenario.construction.port_overrides[0].src = vec![" ".into()];
    assert_eq!(
        preflight_port_starting(&c).unwrap_err().kind(),
        PortInitializationErrorKind::Malformed
    );
    let mut c = content();
    policy(&mut c, Some(PortDefaultPolicy::Unsupported(" ".into())));
    c.scenario.construction.port_overrides[0].condition =
        Some(PortStartingCondition::Unsupported("future_rule".into()));
    assert!(matches!(
        port_starting_diagnostics(&c),
        Err(PortStartingDiagnostic::General(_))
    ));
}
