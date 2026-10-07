//! Typed starting works. Omitted facts remain unknown, independently of explicit absence.
use super::FileHeader;
use crate::{ContentError, map::MapContent, read_toml};
use cna_core::ids::HexId;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioConstruction {
    pub file: Option<FileHeader>,
    pub construction: Option<InitialConstruction>,
    pub port_policy: Option<PortStartingPolicy>,
    #[serde(default, rename = "port_override")]
    pub port_overrides: Vec<PortOverride>,
    #[serde(skip)]
    source_path: Option<PathBuf>,
}

/// Explicit source declarations of no starting minefields or built fortifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum NoInitialWorks {
    #[serde(rename = "none")]
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum PipelineExtent {
    #[serde(rename = "none beyond the railroad")]
    RailroadOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialConstruction {
    pub minefields: Option<NoInitialWorks>,
    pub fortifications: Option<NoInitialWorks>,
    pub pipeline: Option<PipelineExtent>,
    pub railroad: Option<InitialRailroad>,
    /// Retained source annotation. Engine policy comes from rules and typed overrides.
    pub ports: Option<String>,
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialRailroad {
    pub terminus: RailroadTerminus,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RailroadTerminus {
    Hex { hex: HexId, name: Option<String> },
}
impl RailroadTerminus {
    pub fn hex(&self) -> &HexId {
        match self {
            Self::Hex { hex, .. } => hex,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortOverride {
    pub port: String,
    pub hex: HexId,
    pub efficiency_level: i32,
    pub condition: Option<PortStartingCondition>,
    pub note: Option<String>,
    pub src: Vec<String>,
}

/// Authored scenario policy. Missing policy/default remains unknown; future tags
/// retain their source value for procedure diagnostics rather than a numeric default.
/// Cases: scen:60.7, scen:60.23
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortStartingPolicy {
    pub default: Option<PortDefaultPolicy>,
    pub src: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum PortDefaultPolicy {
    Known(KnownPortDefault),
    Unsupported(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnownPortDefault {
    ListedMax,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum PortStartingCondition {
    Known(KnownPortCondition),
    Unsupported(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnownPortCondition {
    ExactEfficiency,
    SanGiorgioPresent,
}

impl ScenarioConstruction {
    /// Actual tracked setup path, including inherited setup; omitted data has no path.
    /// Cases: scen:60.7, scen:60.23
    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    /// Decode through the tracked content reader, preserving inherited source provenance.
    /// Cases: scen:60.7, scen:60.23
    pub(crate) fn load(path: &Path) -> Result<Self, ContentError> {
        let mut setup: Self = read_toml(path)?;
        setup.source_path = Some(path.to_path_buf());
        setup.validate()?;
        Ok(setup)
    }

    fn invalid(&self, message: impl Into<String>) -> ContentError {
        ContentError::Invalid {
            path: self
                .source_path
                .clone()
                .unwrap_or_else(|| "construction.toml".into()),
            message: message.into(),
        }
    }

    /// Validate records without manufacturing defaults for any omitted initial works.
    /// Cases: scen:60.7
    pub fn validate(&self) -> Result<(), ContentError> {
        let cited = |src: &[String]| !src.is_empty() && src.iter().all(|s| !s.trim().is_empty());
        if self.file.as_ref().is_some_and(|header| !cited(&header.src)) {
            return Err(self.invalid("construction file needs citations"));
        }
        if (self.construction.is_some()
            || self.port_policy.is_some()
            || !self.port_overrides.is_empty())
            && self.file.is_none()
        {
            return Err(self.invalid("construction records need a cited file header"));
        }
        if let Some(policy) = &self.port_policy {
            if !cited(&policy.src) {
                return Err(self.invalid("port starting policy needs citations"));
            }
            if matches!(&policy.default, Some(PortDefaultPolicy::Unsupported(tag)) if tag.trim().is_empty())
            {
                return Err(self.invalid("port starting default must be a nonblank policy tag"));
            }
        }
        if let Some(initial) = &self.construction {
            if !cited(&initial.src) {
                return Err(self.invalid("initial construction needs citations"));
            }
            if let Some(railroad) = &initial.railroad
                && railroad.terminus.hex().as_str().trim().is_empty()
            {
                return Err(self.invalid("railroad terminus needs a hex reference"));
            }
        }
        let mut ports = BTreeSet::new();
        let mut hexes = BTreeSet::new();
        for entry in &self.port_overrides {
            if entry.port.trim().is_empty()
                || entry.hex.as_str().trim().is_empty()
                || entry.efficiency_level < 0
                || !cited(&entry.src)
                || matches!(&entry.condition, Some(PortStartingCondition::Unsupported(tag)) if tag.trim().is_empty())
            {
                return Err(self.invalid(
                    "port override needs identity, hex, citations and nonnegative efficiency",
                ));
            }
            if !ports.insert(&entry.port) || !hexes.insert(&entry.hex) {
                return Err(self.invalid("duplicate port override identity or hex"));
            }
        }
        Ok(())
    }

    /// Reject unmapped anchors; the consumer uses canonical map identity at initialization.
    /// Cases: scen:60.7
    pub fn check_map(&self, map: &MapContent) -> Result<(), ContentError> {
        self.validate()?;
        let terminus = self
            .construction
            .as_ref()
            .and_then(|c| c.railroad.as_ref())
            .map(|r| r.terminus.hex());
        if let Some(hex) = terminus
            && map.canonical(hex).is_none()
        {
            return Err(self.invalid(format!("construction anchor is not a map hex: {hex}")));
        }
        let mut canonical_ports = BTreeSet::new();
        for entry in &self.port_overrides {
            let canonical = map.canonical(&entry.hex).ok_or_else(|| {
                self.invalid(format!(
                    "construction anchor is not a map hex: {}",
                    entry.hex
                ))
            })?;
            if !canonical_ports.insert(canonical) {
                return Err(self.invalid("port override aliases collide at the same canonical hex"));
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn graziani() -> ScenarioConstruction {
        ScenarioConstruction::load(
            &crate::repo_data_dir().join("scenarios/graziani/construction.toml"),
        )
        .unwrap()
    }

    /// Cases: scen:60.7, scen:60.23
    #[test]
    fn exact_setup_and_inherited_read_provenance_are_preserved() {
        let base = crate::repo_data_dir().join("scenarios");
        let ((graziani, italian), reads) = crate::record_reads(|| {
            (
                super::super::ScenarioContent::load(&base.join("graziani")).unwrap(),
                super::super::ScenarioContent::load(&base.join("italian_campaign")).unwrap(),
            )
        });
        assert_eq!(graziani.construction, italian.construction);
        assert_eq!(
            graziani.construction.port_policy,
            Some(PortStartingPolicy {
                default: Some(PortDefaultPolicy::Known(KnownPortDefault::ListedMax)),
                src: vec!["scen:60.7".into()],
            })
        );
        let initial = graziani.construction.construction.as_ref().unwrap();
        assert_eq!(initial.minefields, Some(NoInitialWorks::Absent));
        assert_eq!(initial.fortifications, Some(NoInitialWorks::Absent));
        assert_eq!(initial.pipeline, Some(PipelineExtent::RailroadOnly));
        assert_eq!(
            initial.railroad.as_ref().unwrap().terminus.hex().as_str(),
            "D3714"
        );
        let port = &graziani.construction.port_overrides[0];
        assert_eq!(graziani.construction.port_overrides.len(), 1);
        assert_eq!(
            port.condition,
            Some(PortStartingCondition::Known(
                KnownPortCondition::SanGiorgioPresent
            ))
        );
        assert_eq!(
            (port.port.as_str(), port.hex.as_str(), port.efficiency_level),
            ("tobruk", "C4807", 7)
        );
        assert!(reads.contains(&crate::normalize(&base.join("graziani/construction.toml"))));
        assert!(!reads.contains(&crate::normalize(
            &base.join("italian_campaign/construction.toml")
        )));
    }

    /// Cases: scen:60.7
    #[test]
    fn omitted_file_and_omitted_individual_facts_remain_unknown() {
        let absent: ScenarioConstruction = toml::from_str("").unwrap();
        absent.validate().unwrap();
        assert!(absent.file.is_none());
        assert!(absent.construction.is_none());
        assert!(absent.port_policy.is_none());
        let partial: ScenarioConstruction =
            toml::from_str("[file]\nsrc=['scen:60.7']\n[construction]\nsrc=['scen:60.7']").unwrap();
        partial.validate().unwrap();
        let initial = partial.construction.unwrap();
        assert!(initial.minefields.is_none());
        assert!(initial.fortifications.is_none());
        assert!(initial.pipeline.is_none());
        assert!(initial.railroad.is_none());
    }

    /// Cases: scen:60.7, scen:60.23
    #[test]
    fn legacy_annotations_and_omitted_policy_fields_supply_no_default() {
        let source = "[file]\nsrc=['scen:60.7']\n[construction]\nports='source annotation only'\nsrc=['scen:60.7']\n[[port_override]]\nport='tobruk'\nhex='C4807'\nefficiency_level=7\nsrc=['scen:60.7']";
        let setup: ScenarioConstruction = toml::from_str(source).unwrap();
        setup.validate().unwrap();
        assert!(setup.port_policy.is_none());
        assert!(setup.port_overrides[0].condition.is_none());
        let mut setup: ScenarioConstruction =
            toml::from_str("[file]\nsrc=['scen:60.7']\n[port_policy]\nsrc=['scen:60.7']").unwrap();
        setup.validate().unwrap();
        assert!(setup.port_policy.as_ref().unwrap().default.is_none());
        setup.file = None;
        assert!(
            setup.validate().is_err(),
            "policy alone still needs a header"
        );
    }

    /// Cases: scen:60.7
    #[test]
    fn cited_future_tags_load_without_masking_later_malformed_records() {
        let fixture = Fixture::new("");
        let path = fixture.dir().join("construction.toml");
        let source = "[file]\nsrc=['scen:60.7']\n[port_policy]\ndefault='future_verified_policy'\nsrc=['scen:60.7']\n[[port_override]]\nport='tobruk'\nhex='C4807'\nefficiency_level=7\ncondition='future_verified_condition'\nsrc=['scen:60.7']";
        std::fs::write(&path, source).unwrap();
        let (setup, reads) = crate::record_reads(|| ScenarioConstruction::load(&path));
        let setup = setup.unwrap();
        assert_eq!(setup.source_path(), Some(path.as_path()));
        assert_eq!(reads, vec![crate::normalize(&path)]);
        assert_eq!(
            setup.port_policy.unwrap().default,
            Some(PortDefaultPolicy::Unsupported(
                "future_verified_policy".into()
            ))
        );
        assert_eq!(
            setup.port_overrides[0].condition,
            Some(PortStartingCondition::Unsupported(
                "future_verified_condition".into()
            ))
        );
        std::fs::write(
            &path,
            format!("{source}\n[[port_override]]\nport='bad'\nhex='C4218'\nefficiency_level=-1\nsrc=['scen:60.7']"),
        )
        .unwrap();
        assert!(matches!(
            ScenarioConstruction::load(&path),
            Err(ContentError::Invalid { path: actual, .. }) if actual == path
        ));
    }

    /// Cases: scen:60.7
    #[test]
    fn malformed_policy_shapes_blank_tags_and_missing_citations_are_rejected() {
        for default in ["12", "[]", "{kind='listed_max'}"] {
            assert!(
                toml::from_str::<ScenarioConstruction>(&format!(
                    "[file]\nsrc=['scen:60.7']\n[port_policy]\ndefault={default}\nsrc=['scen:60.7']"
                ))
                .is_err()
            );
        }
        for mutation in 0..5 {
            let mut setup = graziani();
            match mutation {
                0 => setup.port_policy.as_mut().unwrap().src.clear(),
                1 => setup.port_policy.as_mut().unwrap().src = vec![" ".into()],
                2 => {
                    setup.port_policy.as_mut().unwrap().default =
                        Some(PortDefaultPolicy::Unsupported(" ".into()));
                }
                3 => {
                    setup.port_overrides[0].condition =
                        Some(PortStartingCondition::Unsupported(" ".into()));
                }
                4 => setup.file = None,
                _ => unreachable!(),
            }
            let error = setup.validate().unwrap_err();
            assert!(error.to_string().contains("graziani"));
        }
        let mut setup = graziani();
        setup.port_overrides[0].condition = Some(PortStartingCondition::Known(
            KnownPortCondition::ExactEfficiency,
        ));
        setup.validate().unwrap();
    }

    /// Cases: scen:60.7
    #[test]
    fn invalid_citations_identity_efficiency_and_duplicates_are_rejected() {
        for mutation in 0..7 {
            let mut setup = graziani();
            match mutation {
                0 => setup.file.as_mut().unwrap().src.clear(),
                1 => setup.construction.as_mut().unwrap().src = vec![" ".into()],
                2 => setup.port_overrides[0].src.clear(),
                3 => setup.port_overrides[0].port.clear(),
                4 => setup.port_overrides[0].efficiency_level = -1,
                5 => {
                    let mut duplicate = setup.port_overrides[0].clone();
                    duplicate.hex = "D3714".into();
                    setup.port_overrides.push(duplicate);
                }
                6 => {
                    let mut duplicate = setup.port_overrides[0].clone();
                    duplicate.port = "another".into();
                    setup.port_overrides.push(duplicate);
                }
                _ => unreachable!(),
            }
            let error = setup.validate().unwrap_err();
            assert!(
                error.to_string().contains("construction.toml"),
                "mutation {mutation}: {error}"
            );
        }
    }

    /// Cases: scen:60.7, land:4.1
    #[test]
    fn alias_collision_fails_with_the_original_source_path() {
        let map = MapContent::load(&crate::repo_data_dir().join("map")).unwrap();
        let mut setup = graziani();
        setup.port_overrides[0].hex = "D0200".into();
        let mut second = setup.port_overrides[0].clone();
        second.port = "different_source_id".into();
        second.hex = "C0233".into();
        setup.port_overrides.push(second);
        setup.validate().unwrap();
        let error = setup.check_map(&map).unwrap_err();
        assert!(error.to_string().contains("aliases collide"));
        match error {
            ContentError::Invalid { path, .. } => {
                assert!(path.ends_with("graziani/construction.toml"))
            }
            _ => panic!("expected canonical-location validation error"),
        }
    }

    /// Cases: scen:60.7
    #[test]
    fn wrong_location_kind_and_unmapped_anchors_are_rejected() {
        assert!(
            toml::from_str::<InitialRailroad>("terminus={kind='city',city='Mersa Matruh'}")
                .is_err()
        );
        let map = MapContent::load(&crate::repo_data_dir().join("map")).unwrap();
        let mut setup = graziani();
        setup.port_overrides[0].hex = "Z9999".into();
        assert!(
            setup
                .check_map(&map)
                .unwrap_err()
                .to_string()
                .contains("not a map hex")
        );
        setup.port_overrides[0].hex = "".into();
        assert!(setup.validate().is_err());
    }

    struct Fixture {
        root: PathBuf,
    }
    impl Fixture {
        fn new(extra: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "cna-construction-{}-{stamp}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir(&root).unwrap();
            let fixture = Self { root };
            std::fs::create_dir(fixture.dir()).unwrap();
            let source = format!(
                "[scenario]\nid='construction_fixture'\nname='Construction fixture'\nstart={{gt=1,opstage=1}}\nend={{gt=1,opstage=1}}\n{extra}\n[initiative]\n"
            );
            std::fs::write(fixture.dir().join("scenario.toml"), source).unwrap();
            fixture
        }
        fn dir(&self) -> PathBuf {
            self.root.join("current")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let root = self.root.canonicalize().unwrap();
            let temp = std::env::temp_dir().canonicalize().unwrap();
            assert_eq!(root.parent(), Some(temp.as_path()));
            assert!(
                root.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("cna-construction-")
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    /// Cases: scen:60.7
    #[test]
    fn actual_missing_unlisted_construction_is_unknown_and_not_read() {
        let fixture = Fixture::new("");
        assert!(!fixture.dir().join("construction.toml").exists());
        let (loaded, reads) =
            crate::record_reads(|| super::super::ScenarioContent::load(&fixture.dir()));
        let scenario = loaded.unwrap();
        assert_eq!(scenario.construction, ScenarioConstruction::default());
        assert_eq!(scenario.construction.source_path(), None);
        assert_eq!(
            reads,
            vec![crate::normalize(&fixture.dir().join("scenario.toml"))]
        );
        let map = MapContent::load(&crate::repo_data_dir().join("map")).unwrap();
        scenario.construction.check_map(&map).unwrap();
    }

    /// Cases: scen:60.7, scen:60.23
    #[test]
    fn declared_or_inherited_missing_construction_remains_a_manifest_error() {
        let declared = Fixture::new("files=['construction.toml']");
        let error = super::super::ScenarioContent::load(&declared.dir()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing scenario file construction.toml")
        );
        let inherited = Fixture::new("setup_from='base'\nsetup_files=['construction.toml']");
        std::fs::create_dir(inherited.root.join("base")).unwrap();
        let error = super::super::ScenarioContent::load(&inherited.dir()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing reused setup file construction.toml")
        );
    }

    /// Cases: scen:60.7
    #[test]
    fn missing_header_with_present_records_is_not_an_unknown_empty_file() {
        let mut setup = graziani();
        setup.file = None;
        assert!(
            setup
                .validate()
                .unwrap_err()
                .to_string()
                .contains("file header")
        );
        setup.construction = None;
        assert!(
            setup.validate().is_err(),
            "port records still require a header"
        );
    }

    /// Cases: scen:60.7
    #[test]
    fn unmapped_railroad_terminus_keeps_its_original_source_error_path() {
        let mut setup = graziani();
        setup
            .construction
            .as_mut()
            .unwrap()
            .railroad
            .as_mut()
            .unwrap()
            .terminus = RailroadTerminus::Hex {
            hex: "Z9999".into(),
            name: Some("Unmapped fixture".into()),
        };
        let map = MapContent::load(&crate::repo_data_dir().join("map")).unwrap();
        let error = setup.check_map(&map).unwrap_err();
        assert!(error.to_string().contains("not a map hex: Z9999"));
        match error {
            ContentError::Invalid { path, .. } => {
                assert!(path.ends_with("graziani/construction.toml"))
            }
            _ => panic!("expected railroad reference validation error"),
        }
    }
}
