use super::*;
use crate::logistics::SupplyError;
use cna_content::scenario::construction::ScenarioConstruction;
use std::path::{Path, PathBuf};

fn content(scenario: &str) -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), scenario).unwrap()
}
fn verified(content: &mut CnaContent, hex: &str, name: &str) -> Port {
    let canonical = content.map.canonical(&hex.into()).unwrap().clone();
    content
        .places
        .places
        .retain(|_, place| place.hex_id != canonical || place.kind != "port");
    let mut place = content.places.places["port-sollum"].clone();
    place.id = "00-test-port".into();
    place.name = name.into();
    place.hex_id = canonical.clone();
    content.places.places.insert(place.id.clone(), place);
    ports::at(content, &Location::Hex { hex: canonical }).unwrap()
}
fn future_record(content: &CnaContent, hex: &str, name: &str, raw: i32) -> PortOverride {
    let mut record = content.scenario.construction.port_overrides[0].clone();
    record.hex = hex.into();
    record.port = name.into();
    record.efficiency_level = raw;
    record.src = vec!["scen:61.1".into()];
    record
}

/// Cases: scen:60.7, scen:60.23, airlog:55.25, airlog:55.3
/// Interpretations: interp:scen-0007
#[test]
fn raw7_derives_bound_maximum_without_rewriting_inherited_source() {
    for scenario in ["graziani", "italian_campaign"] {
        let mut c = content(scenario);
        let before = c.scenario.construction.clone();
        assert_eq!(before.port_overrides[0].efficiency_level, 7);
        assert!(
            before
                .source_path()
                .unwrap()
                .ends_with("graziani/construction.toml")
        );
        assert!(
            c.source_files()
                .contains(&cna_content::normalize(before.source_path().unwrap()))
        );
        let port = verified(&mut c, "C4807", "Tobruk");
        preflight_port_overrides(&c).unwrap();
        assert!(unsupported_port_policies(&c).unwrap().is_empty());
        assert_eq!(
            initial_port_policy(&c, &port).unwrap(),
            InitialPortPolicy::Known(InitialPortCondition {
                blocked_levels: 3,
                efficiency: c
                    .tables
                    .airlog
                    .port_capacity
                    .port(PortName::Tobruk)
                    .max_efficiency_level
                    - 3
            })
        );
        assert_eq!(c.scenario.construction, before);
    }
}

/// Cases: scen:60.7, airlog:55.18, airlog:55.3
#[test]
fn altered_scenario_values_are_malformed_with_exact_provenance() {
    let mut c = content("graziani");
    let port = verified(&mut c, "C4807", "Tobruk");
    for raw in [-1, 0, 2, 5, 6, 8, 99, i32::MAX] {
        c.scenario.construction.port_overrides[0].efficiency_level = raw;
        let before = c.scenario.construction.clone();
        let error = preflight_port_overrides(&c).unwrap_err();
        assert_eq!(error.kind, PortInitializationErrorKind::Malformed);
        assert_eq!(error.raw_efficiency, raw);
        assert_eq!(error.port, "tobruk");
        assert_eq!(error.hex.as_str(), "C4807");
        assert_eq!(error.case, "scen:60.7");
        assert_eq!(error.src, vec!["scen:60.7"]);
        assert_eq!(error.source_path.as_deref(), before.source_path());
        assert_eq!(initial_port_policy(&c, &port).unwrap_err(), error);
        let detail = error.to_string();
        assert!(
            matches!(error.into_engine(),EngineError::Invariant{detail: actual} if actual==detail)
        );
        assert_eq!(c.scenario.construction, before);
    }
}

/// Cases: airlog:55.11, airlog:55.18, airlog:55.3
#[test]
fn complete_scan_collects_future_policies_but_never_hides_later_malformed_record() {
    let mut c = content("graziani");
    let mut second = future_record(&c, "C4807", "tobruk", 5);
    c.scenario.construction.port_overrides =
        vec![future_record(&c, "C4022", "sollum", 1), second.clone()];
    preflight_port_overrides(&c).unwrap();
    let diagnostics = unsupported_port_policies(&c).unwrap();
    assert_eq!(diagnostics.len(), 2);
    assert!(
        diagnostics
            .iter()
            .all(|e| e.kind == PortInitializationErrorKind::UnsupportedPolicy)
    );
    second.efficiency_level = 99;
    c.scenario.construction.port_overrides[1] = second;
    let error = unsupported_port_policies(&c).unwrap_err();
    assert_eq!(error.kind, PortInitializationErrorKind::Malformed);
    assert_eq!(error.case, "scen:61.1");
    assert_eq!(error.raw_efficiency, 99);
    assert_eq!(preflight_port_overrides(&c).unwrap_err(), error);
}

/// Cases: airlog:55.11, airlog:55.18, airlog:55.3
#[test]
fn matching_future_port_is_explicit_unknown_and_other_port_has_no_authored_override() {
    let mut c = content("graziani");
    let sollum = ports::at(
        &c,
        &Location::Hex {
            hex: "C4022".into(),
        },
    )
    .unwrap();
    c.scenario.construction.port_overrides = vec![future_record(&c, "C4022", "sollum", 1)];
    let expected = unsupported_port_policies(&c).unwrap().remove(0);
    assert_eq!(
        initial_port_policy(&c, &sollum).unwrap(),
        InitialPortPolicy::Unknown(expected.clone())
    );
    let detail = expected.to_string();
    assert!(
        matches!(expected.into_engine(),EngineError::Unsupported{case,detail: actual}
        if case=="scen:61.1" && detail==actual)
    );
    let other = ports::at(
        &c,
        &Location::OffMap {
            id: "box_tripoli".into(),
        },
    )
    .unwrap();
    assert_eq!(
        initial_port_policy(&c, &other).unwrap(),
        InitialPortPolicy::NoAuthoredOverride
    );
    // A trusted category mismatch remains an actual contradiction, unlike unknown policy.
    let mut wrong = sollum;
    wrong.name = PortName::Bardia;
    assert_eq!(
        initial_port_policy(&c, &wrong).unwrap_err().kind,
        PortInitializationErrorKind::Malformed
    );
}

/// Cases: scen:60.7, airlog:55.11
#[test]
fn every_available_port_identity_is_checked_before_state_exists() {
    let mut c = content("graziani");
    let port = verified(&mut c, "C4807", "Tobruk");
    let mut contradiction = c.places.places["port-sollum"].clone();
    contradiction.id = "zz-test-contradiction".into();
    contradiction.hex_id = "C4807".into();
    c.places
        .places
        .insert(contradiction.id.clone(), contradiction);
    assert_eq!(
        ports::at(&c, &port.location).unwrap().name,
        PortName::Tobruk
    );
    let error = preflight_port_overrides(&c).unwrap_err();
    assert_eq!(error.kind, PortInitializationErrorKind::Malformed);
    assert_eq!(initial_port_policy(&c, &port).unwrap_err(), error);
}

/// Cases: airlog:55.11, airlog:55.3
#[test]
fn named_aliases_match_but_all_others_do_not_collapse_printed_identities() {
    let mut c = content("graziani");
    let port = verified(&mut c, "C4807", "Tunis");
    c.scenario.construction.port_overrides = vec![future_record(&c, "C4807", "bizerta", 1)];
    assert!(matches!(
        initial_port_policy(&c, &port).unwrap(),
        InitialPortPolicy::Unknown(_)
    ));
    let port = verified(&mut c, "C4807", "Port Alpha");
    c.scenario.construction.port_overrides[0].port = "Port Alpha".into();
    assert!(matches!(
        initial_port_policy(&c, &port).unwrap(),
        InitialPortPolicy::Unknown(_)
    ));
    c.scenario.construction.port_overrides[0].port = "Port Beta".into();
    assert_eq!(
        preflight_port_overrides(&c).unwrap_err().kind,
        PortInitializationErrorKind::Malformed
    );
}

/// Cases: scen:60.7, airlog:55.11
#[test]
fn absent_icons_and_omitted_metadata_remain_unknown_without_fabricating_ports() {
    let mut c = content("graziani");
    c.places
        .places
        .retain(|_, p| p.hex_id.as_str() != "C4807" || p.kind != "port");
    let before = c.scenario.construction.clone();
    preflight_port_overrides(&c).unwrap();
    assert_eq!(
        ports::at(
            &c,
            &Location::Hex {
                hex: "C4807".into()
            }
        ),
        Err(SupplyError::Unsupported {
            case: "airlog:55.11"
        })
    );
    assert_eq!(c.scenario.construction, before);
    c.scenario.construction = ScenarioConstruction::default();
    let port = ports::at(
        &c,
        &Location::Hex {
            hex: "C4022".into(),
        },
    )
    .unwrap();
    assert_eq!(
        initial_port_policy(&c, &port).unwrap(),
        InitialPortPolicy::NoAuthoredOverride
    );
    assert!(unsupported_port_policies(&c).unwrap().is_empty());
    assert_eq!(c.scenario.construction.source_path(), None);
    assert!(c.scenario.construction.construction.is_none());
}

/// Cases: scen:60.7, land:4.1, airlog:55.11
#[test]
fn bad_geometry_and_alias_collisions_keep_actual_future_source_case() {
    let mut c = content("graziani");
    c.scenario.construction.port_overrides = vec![future_record(&c, "Z9999", "Port Alpha", 1)];
    let e = preflight_port_overrides(&c).unwrap_err();
    assert_eq!(e.case, "scen:61.1");
    assert_eq!(e.hex.as_str(), "Z9999");
    c.scenario.construction.port_overrides = vec![
        future_record(&c, "D0200", "Port Alpha", 1),
        future_record(&c, "C0233", "Port Alpha", 1),
    ];
    assert_eq!(
        preflight_port_overrides(&c).unwrap_err().kind,
        PortInitializationErrorKind::Malformed
    );
}

pub(super) struct DataFixture {
    pub(super) root: PathBuf,
}
impl DataFixture {
    pub(super) fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "cna-port-policy-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fn copy(source: &Path, target: &Path) {
            std::fs::create_dir(target).unwrap();
            for entry in std::fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                let dest = target.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy(&entry.path(), &dest);
                } else {
                    std::fs::copy(entry.path(), dest).unwrap();
                }
            }
        }
        copy(&cna_content::repo_data_dir(), &root);
        Self { root }
    }
}
impl Drop for DataFixture {
    fn drop(&mut self) {
        let root = self.root.canonicalize().unwrap();
        assert_eq!(
            root.parent(),
            Some(std::env::temp_dir().canonicalize().unwrap().as_path())
        );
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("cna-port-policy-")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Cases: scen:60.7, airlog:55.18, airlog:55.3
#[test]
fn actual_loader_rejects_malformed_but_accepts_future_diagnostics_with_tracked_path() {
    let fixture = DataFixture::new();
    let path = fixture.root.join("scenarios/graziani/construction.toml");
    let original = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        original.replace("efficiency_level = 7", "efficiency_level = 99"),
    )
    .unwrap();
    let error = CnaContent::load(&fixture.root, "graziani").unwrap_err();
    assert!(error.contains("Malformed"));
    assert!(error.contains("raw=99"));
    assert!(
        error.contains(&format!("{:?}", Some(cna_content::normalize(&path)))),
        "{error}"
    );
    // A known contradictory icon is malformed at load, before any port-state history.
    std::fs::write(&path, original.replace("C4807", "C4022")).unwrap();
    let error = CnaContent::load(&fixture.root, "graziani").unwrap_err();
    assert!(error.contains("Malformed"));
    assert!(error.contains("verified port icon"));
    assert!(error.contains("scen:60.7"));
    let future = original
        .replace("scen:60.7", "scen:61.1")
        .replace("efficiency_level = 7", "efficiency_level = 5")
        .replace(
            "condition = \"san_giorgio_present\"",
            "condition = \"future_condition\"",
        );
    std::fs::write(&path, future).unwrap();
    let c = CnaContent::load(&fixture.root, "graziani").unwrap();
    let diagnostics = unsupported_port_policies(&c).unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].kind,
        PortInitializationErrorKind::UnsupportedPolicy
    );
    assert_eq!(diagnostics[0].source_path.as_deref(), Some(path.as_path()));
    assert_eq!(diagnostics[0].case, "scen:61.1");
    assert_eq!(diagnostics[0].raw_efficiency, 5);
    assert!(c.source_files().contains(&cna_content::normalize(&path)));
}
