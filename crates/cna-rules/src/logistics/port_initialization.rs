//! Immutable initial-port policies. Authored values retain their source provenance.
use super::ports::{self, Port};
use crate::{CnaContent, state::Location};
use cna_content::scenario::construction::PortOverride;
use cna_core::{engine::EngineError, ids::HexId};
use cna_tables::airlog::trucks::PortName;
use std::{collections::BTreeSet, fmt, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitialPortCondition {
    pub efficiency: i32,
    pub blocked_levels: i32,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitialPortPolicy {
    /// No authored override, including omitted/unknown construction metadata.
    NoAuthoredOverride,
    Known(InitialPortCondition),
    /// Authored policy is structurally valid but lacks an implemented source reading.
    Unknown(PortInitializationError),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortInitializationErrorKind {
    Malformed,
    UnsupportedPolicy,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInitializationError {
    pub kind: PortInitializationErrorKind,
    pub provenance: Box<PortInitializationProvenance>,
}
/// Heap-backed source context keeps fallible policy APIs compact without losing fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInitializationProvenance {
    pub source_path: Option<PathBuf>,
    pub src: Vec<String>,
    pub hex: HexId,
    pub port: String,
    pub raw_efficiency: i32,
    pub case: String,
    pub detail: String,
}
impl std::ops::Deref for PortInitializationError {
    type Target = PortInitializationProvenance;
    fn deref(&self) -> &Self::Target {
        &self.provenance
    }
}
impl fmt::Display for PortInitializationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}: source={:?}, src={:?}, port={:?}, hex={}, raw={}, case={}: {}",
            self.kind,
            self.source_path,
            self.src,
            self.port,
            self.hex,
            self.raw_efficiency,
            self.case,
            self.detail
        )
    }
}
impl std::error::Error for PortInitializationError {}
impl PortInitializationError {
    pub fn into_engine(self) -> EngineError {
        let detail = self.to_string();
        match self.kind {
            PortInitializationErrorKind::Malformed => EngineError::Invariant { detail },
            PortInitializationErrorKind::UnsupportedPolicy => EngineError::Unsupported {
                case: self.case.clone(),
                detail,
            },
        }
    }
}
fn error(
    content: &CnaContent,
    record: &PortOverride,
    kind: PortInitializationErrorKind,
    detail: &str,
) -> PortInitializationError {
    PortInitializationError {
        kind,
        provenance: Box::new(PortInitializationProvenance {
            source_path: content
                .scenario
                .construction
                .source_path()
                .map(PathBuf::from),
            src: record.src.clone(),
            hex: content
                .map
                .canonical(&record.hex)
                .unwrap_or(&record.hex)
                .clone(),
            port: record.port.clone(),
            raw_efficiency: record.efficiency_level,
            case: record
                .src
                .iter()
                .find(|case| case.starts_with("scen:"))
                .or_else(|| record.src.first())
                .cloned()
                .unwrap_or_default(),
            detail: detail.into(),
        }),
    }
}
fn malformed(content: &CnaContent, record: &PortOverride, detail: &str) -> PortInitializationError {
    error(
        content,
        record,
        PortInitializationErrorKind::Malformed,
        detail,
    )
}
fn same_name(a: &str, b: &str) -> bool {
    let category = ports::named(a);
    if category == PortName::AllOthers {
        a.eq_ignore_ascii_case(b)
    } else {
        category == ports::named(b)
    }
}

/// Validate public geometry, identity and chart range before selecting any policy.
/// Cases: scen:60.7, airlog:55.11, airlog:55.18, airlog:55.25, airlog:55.3
/// Interpretations: interp:scen-0007
fn validate_record(
    content: &CnaContent,
    record: &PortOverride,
) -> Result<InitialPortPolicy, PortInitializationError> {
    let canonical = content.map.canonical(&record.hex).ok_or_else(|| {
        malformed(
            content,
            record,
            "construction contains an unmapped port anchor",
        )
    })?;
    if record.port.trim().is_empty() || record.src.is_empty() {
        return Err(malformed(
            content,
            record,
            "port identity and source citations are required",
        ));
    }
    // Every available positive identity must agree. Missing icons remain Unknown.
    if content
        .places
        .at(canonical)
        .filter(|place| place.kind == "port")
        .any(|place| !same_name(&place.name, &record.port))
    {
        return Err(malformed(
            content,
            record,
            "authored port identity contradicts a verified port icon",
        ));
    }
    let category = ports::named(&record.port);
    let row = content.tables.airlog.port_capacity.port(category);
    let scenario_tobruk =
        category == PortName::Tobruk && record.src.iter().any(|case| case == "scen:60.7");
    let exception = scenario_tobruk
        && content.map.canonical(&HexId::new("C4807")) == Some(canonical)
        && record.efficiency_level == 7;
    if scenario_tobruk && !exception {
        return Err(malformed(
            content,
            record,
            "scen:60.7 Tobruk must retain its exact authored C4807 efficiency7",
        ));
    }
    if !exception && !(0..=row.max_efficiency_level).contains(&record.efficiency_level) {
        return Err(malformed(
            content,
            record,
            "authored efficiency is outside the bound port maximum",
        ));
    }
    if exception {
        if row
            .footnotes
            .iter()
            .any(|note| note == "tobruk_starts_below_max_san_giorgio")
        {
            // Case55.25 supplies blockage3; the chart supplies the maximum, never assumed10.
            let blocked_levels = 3;
            let efficiency = row
                .max_efficiency_level
                .checked_sub(blocked_levels)
                .filter(|level| *level >= 0)
                .ok_or_else(|| {
                    malformed(
                        content,
                        record,
                        "bound maximum cannot accommodate the cited ship blockage",
                    )
                })?;
            return Ok(InitialPortPolicy::Known(InitialPortCondition {
                efficiency,
                blocked_levels,
            }));
        }
        return Ok(InitialPortPolicy::Unknown(error(
            content,
            record,
            PortInitializationErrorKind::UnsupportedPolicy,
            "airlog:55.3 ship-blockage footnote is unavailable",
        )));
    }
    Ok(InitialPortPolicy::Unknown(error(
        content,
        record,
        PortInitializationErrorKind::UnsupportedPolicy,
        "authored initial-port policy has no implemented source reading",
    )))
}

/// Complete immutable scan. Err is ALWAYS Malformed; returned entries are ALWAYS
/// UnsupportedPolicy. An earlier diagnostic never suppresses a later malformed record.
/// Callers enforce FULL/DEV policy at public procedure entry before State branches.
/// Cases: scen:60.7, airlog:55.11, airlog:55.18, airlog:55.3
pub fn unsupported_port_policies(
    content: &CnaContent,
) -> Result<Vec<PortInitializationError>, PortInitializationError> {
    let mut diagnostics = Vec::new();
    let mut anchors = BTreeSet::new();
    for record in &content.scenario.construction.port_overrides {
        let policy = validate_record(content, record)?;
        let canonical = content
            .map
            .canonical(&record.hex)
            .expect("record validated above");
        if !anchors.insert(canonical) {
            return Err(malformed(
                content,
                record,
                "authored overrides share a canonical port anchor",
            ));
        }
        if let InitialPortPolicy::Unknown(diagnostic) = policy {
            diagnostics.push(diagnostic);
        }
    }
    Ok(diagnostics)
}
/// Reject malformed public content while retaining valid unsupported policies as diagnostics.
/// No authored override is not proof of ordinary/absent initial conditions.
/// Cases: scen:60.7, airlog:55.18, airlog:55.3
pub fn preflight_port_overrides(content: &CnaContent) -> Result<(), PortInitializationError> {
    unsupported_port_policies(content).map(|_| ())
}

/// Input MUST come from ports::at. AllOthers is a chart category, not a place identity;
/// actual positive printed-name evidence above validates those matching records.
/// Cases: scen:60.7, airlog:55.11, airlog:55.25, airlog:55.3
pub fn initial_port_policy(
    content: &CnaContent,
    port: &Port,
) -> Result<InitialPortPolicy, PortInitializationError> {
    // Same complete content check for every port, independent of existing inventory.
    preflight_port_overrides(content)?;
    let Location::Hex { hex } = &port.location else {
        return Ok(InitialPortPolicy::NoAuthoredOverride);
    };
    let record = content
        .scenario
        .construction
        .port_overrides
        .iter()
        .find(|record| content.map.canonical(&record.hex) == content.map.canonical(hex));
    let Some(record) = record else {
        return Ok(InitialPortPolicy::NoAuthoredOverride);
    };
    let canonical = content
        .map
        .canonical(&record.hex)
        .expect("preflight validated anchor");
    if port.id != canonical.as_str() || ports::named(&record.port) != port.name {
        return Err(malformed(
            content,
            record,
            "authored identity contradicts the trusted verified port",
        ));
    }
    validate_record(content, record)
}

#[cfg(test)]
#[path = "port_initialization_tests.rs"]
mod tests;
