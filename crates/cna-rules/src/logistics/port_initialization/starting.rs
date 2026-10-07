//! Cited authored starting conditions, with no numeric policy for missing setup.
use super::{
    InitialPortCondition, InitialPortPolicy, PortInitializationError, PortInitializationErrorKind,
    error, malformed, validate_record, validate_record_identity,
};
use crate::{CnaContent, logistics::ports::Port, state::Location};
use cna_content::scenario::construction::{
    KnownPortCondition, KnownPortDefault, PortDefaultPolicy, PortOverride, PortStartingCondition,
};
use cna_core::engine::EngineError;
use cna_tables::airlog::trucks::PortName;
use std::{collections::BTreeSet, fmt, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortStartingDiagnostic {
    Override(PortInitializationError),
    General(Box<GeneralPortStartingDiagnostic>),
}
/// General setup gaps have no fabricated port, hex, or efficiency value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneralPortStartingDiagnostic {
    pub kind: PortInitializationErrorKind,
    pub scenario_id: String,
    pub source_path: Option<PathBuf>,
    pub src: Vec<String>,
    pub authored_policy: Option<String>,
    pub case: String,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitialPortStartingPolicy {
    Known(InitialPortCondition),
    Unknown(PortStartingDiagnostic),
}
impl fmt::Display for PortStartingDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Override(error) => error.fmt(f),
            Self::General(error) => write!(
                f,
                "{:?}: scenario={}, source={:?}, src={:?}, policy={:?}, case={}: {}",
                error.kind,
                error.scenario_id,
                error.source_path,
                error.src,
                error.authored_policy,
                error.case,
                error.detail,
            ),
        }
    }
}
impl std::error::Error for PortStartingDiagnostic {}
impl PortStartingDiagnostic {
    pub fn kind(&self) -> PortInitializationErrorKind {
        match self {
            Self::Override(error) => error.kind,
            Self::General(error) => error.kind,
        }
    }
    pub fn into_engine(self) -> EngineError {
        match self {
            Self::Override(error) => error.into_engine(),
            Self::General(error) => {
                let detail = Self::General(error.clone()).to_string();
                match error.kind {
                    PortInitializationErrorKind::Malformed => EngineError::Invariant { detail },
                    PortInitializationErrorKind::UnsupportedPolicy => EngineError::Unsupported {
                        case: error.case.clone(),
                        detail,
                    },
                }
            }
        }
    }
}
fn general_diagnostic(
    content: &CnaContent,
    kind: PortInitializationErrorKind,
    detail: &str,
) -> PortStartingDiagnostic {
    let setup = &content.scenario.construction;
    let src = setup
        .port_policy
        .as_ref()
        .map(|policy| policy.src.clone())
        .or_else(|| setup.file.as_ref().map(|header| header.src.clone()))
        .unwrap_or_default();
    let case = src
        .iter()
        .find(|case| case.starts_with("scen:"))
        .or_else(|| src.iter().find(|case| !case.trim().is_empty()))
        .cloned()
        .unwrap_or_else(|| "airlog:55.18".into());
    let authored_policy = setup
        .port_policy
        .as_ref()
        .and_then(|policy| policy.default.as_ref())
        .map(|policy| match policy {
            PortDefaultPolicy::Known(KnownPortDefault::ListedMax) => "listed_max".into(),
            PortDefaultPolicy::Unsupported(policy) => policy.clone(),
        });
    PortStartingDiagnostic::General(Box::new(GeneralPortStartingDiagnostic {
        kind,
        scenario_id: content.scenario.meta.id.clone(),
        source_path: setup.source_path().map(PathBuf::from),
        src,
        authored_policy,
        case,
        detail: detail.into(),
    }))
}
fn cited(src: &[String]) -> bool {
    !src.is_empty() && src.iter().all(|case| !case.trim().is_empty())
}
/// Missing authored general policy never authorizes an ordinary numeric condition.
/// Cases: scen:60.7, airlog:55.18
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeneralPolicy {
    Missing,
    ListedMax,
    Unsupported,
}
fn general_policy(content: &CnaContent) -> Result<GeneralPolicy, PortStartingDiagnostic> {
    let setup = &content.scenario.construction;
    let Some(policy) = &setup.port_policy else {
        return Ok(GeneralPolicy::Missing);
    };
    if setup.file.as_ref().is_none_or(|header| !cited(&header.src)) || !cited(&policy.src) {
        return Err(general_diagnostic(
            content,
            PortInitializationErrorKind::Malformed,
            "authored port policy requires a cited file header and policy provenance",
        ));
    }
    match &policy.default {
        None => Ok(GeneralPolicy::Missing),
        Some(PortDefaultPolicy::Known(KnownPortDefault::ListedMax)) => Ok(GeneralPolicy::ListedMax),
        Some(PortDefaultPolicy::Unsupported(policy)) if policy.trim().is_empty() => {
            Err(general_diagnostic(
                content,
                PortInitializationErrorKind::Malformed,
                "authored port default tag is blank",
            ))
        }
        Some(PortDefaultPolicy::Unsupported(_)) => Ok(GeneralPolicy::Unsupported),
    }
}
fn general_gap(content: &CnaContent, supported: GeneralPolicy) -> PortStartingDiagnostic {
    general_diagnostic(
        content,
        PortInitializationErrorKind::UnsupportedPolicy,
        if supported == GeneralPolicy::Missing {
            "initial port default has not been authored for this scenario"
        } else {
            "authored general port policy has no implemented source reading"
        },
    )
}
/// Tagged exceptions remain separate from the general policy; no unknown falls through.
/// Cases: scen:60.7, airlog:55.18, airlog:55.25, airlog:55.3
fn override_policy(
    content: &CnaContent,
    record: &PortOverride,
) -> Result<InitialPortStartingPolicy, PortStartingDiagnostic> {
    if !cited(&record.src) {
        return Err(PortStartingDiagnostic::Override(malformed(
            content,
            record,
            "authored port exception has invalid citations",
        )));
    }
    let condition = record.condition.as_ref();
    if condition.is_none() {
        return validate_record(content, record)
            .map(|policy| match policy {
                InitialPortPolicy::Known(condition) => InitialPortStartingPolicy::Known(condition),
                InitialPortPolicy::Unknown(error) => {
                    InitialPortStartingPolicy::Unknown(PortStartingDiagnostic::Override(error))
                }
                InitialPortPolicy::NoAuthoredOverride => {
                    unreachable!("record resolution has a record")
                }
            })
            .map_err(PortStartingDiagnostic::Override);
    }
    let (hex, category) =
        validate_record_identity(content, record).map_err(PortStartingDiagnostic::Override)?;
    let row = content.tables.airlog.port_capacity.port(category);
    let legacy_tobruk =
        category == PortName::Tobruk && record.src.iter().any(|case| case == "scen:60.7");
    let legacy_raw = legacy_tobruk && hex.as_str() == "C4807" && record.efficiency_level == 7;
    if legacy_tobruk && !legacy_raw {
        return Err(PortStartingDiagnostic::Override(malformed(
            content,
            record,
            "scen:60.7 Tobruk must retain its exact authored C4807 efficiency7",
        )));
    }
    let san_giorgio = matches!(
        condition,
        Some(PortStartingCondition::Known(
            KnownPortCondition::SanGiorgioPresent
        ))
    );
    if san_giorgio {
        if category != PortName::Tobruk || hex.as_str() != "C4807" || record.efficiency_level != 7 {
            return Err(PortStartingDiagnostic::Override(malformed(
                content,
                record,
                "San Giorgio condition requires exact Tobruk C4807 transcribed efficiency7",
            )));
        }
        if !row
            .footnotes
            .iter()
            .any(|note| note == "tobruk_starts_below_max_san_giorgio")
        {
            return Ok(InitialPortStartingPolicy::Unknown(
                PortStartingDiagnostic::Override(error(
                    content,
                    record,
                    PortInitializationErrorKind::UnsupportedPolicy,
                    "bound San Giorgio port footnote is unavailable",
                )),
            ));
        }
        // Case55.25 blockage is source-defined; the maximum is chart-derived.
        let blocked_levels = 3;
        let efficiency = row
            .max_efficiency_level
            .checked_sub(blocked_levels)
            .filter(|level| *level >= 0)
            .ok_or_else(|| {
                PortStartingDiagnostic::Override(malformed(
                    content,
                    record,
                    "bound maximum cannot accommodate cited blockage",
                ))
            })?;
        return Ok(InitialPortStartingPolicy::Known(InitialPortCondition {
            efficiency,
            blocked_levels,
        }));
    }
    if !(0..=row.max_efficiency_level).contains(&record.efficiency_level)
        && !(legacy_raw && matches!(condition, Some(PortStartingCondition::Unsupported(_))))
    {
        return Err(PortStartingDiagnostic::Override(malformed(
            content,
            record,
            "authored efficiency is outside the bound port maximum",
        )));
    }
    match condition {
        Some(PortStartingCondition::Known(KnownPortCondition::ExactEfficiency)) => {
            Ok(InitialPortStartingPolicy::Known(InitialPortCondition {
                efficiency: record.efficiency_level,
                blocked_levels: 0,
            }))
        }
        Some(PortStartingCondition::Unsupported(tag)) if tag.trim().is_empty() => {
            Err(PortStartingDiagnostic::Override(malformed(
                content,
                record,
                "authored port condition tag is blank",
            )))
        }
        Some(PortStartingCondition::Unsupported(_)) => Ok(InitialPortStartingPolicy::Unknown(
            PortStartingDiagnostic::Override(error(
                content,
                record,
                PortInitializationErrorKind::UnsupportedPolicy,
                "authored port condition has no implemented source reading",
            )),
        )),
        _ => unreachable!("San Giorgio condition resolved above"),
    }
}
/// Complete source scan: Err only Malformed, success entries only UnsupportedPolicy.
/// Missing or future setup is diagnostic and never rejects scenario loading.
/// Cases: scen:60.7, airlog:55.18, airlog:55.3
pub fn port_starting_diagnostics(
    content: &CnaContent,
) -> Result<Vec<PortStartingDiagnostic>, PortStartingDiagnostic> {
    let general = general_policy(content);
    let mut failure = general.as_ref().err().cloned();
    let mut diagnostics = Vec::new();
    if let Ok(supported) = general
        && supported != GeneralPolicy::ListedMax
    {
        diagnostics.push(general_gap(content, supported));
    }
    let mut anchors = BTreeSet::new();
    for record in &content.scenario.construction.port_overrides {
        if content
            .scenario
            .construction
            .file
            .as_ref()
            .is_none_or(|header| !cited(&header.src))
            && failure.is_none()
        {
            failure = Some(PortStartingDiagnostic::Override(malformed(
                content,
                record,
                "authored port exceptions require a cited file header",
            )));
        }
        match override_policy(content, record) {
            Ok(InitialPortStartingPolicy::Known(_)) => {}
            Ok(InitialPortStartingPolicy::Unknown(error)) => diagnostics.push(error),
            Err(error) => {
                if failure.is_none() {
                    failure = Some(error);
                }
            }
        }
        if let Some(hex) = content.map.canonical(&record.hex)
            && !anchors.insert(hex)
        {
            let duplicate = PortStartingDiagnostic::Override(malformed(
                content,
                record,
                "authored overrides share a canonical port anchor",
            ));
            if failure.is_none() {
                failure = Some(duplicate);
            }
        }
    }
    failure.map_or(Ok(diagnostics), Err)
}
/// Same single complete-content load hook, extended to truthful general provenance.
/// Cases: scen:60.7, airlog:55.18
pub fn preflight_port_starting(content: &CnaContent) -> Result<(), PortStartingDiagnostic> {
    port_starting_diagnostics(content).map(|_| ())
}
/// Input MUST be ports::at-derived. No state/ownership/inventory branch is inspected.
/// A missing/unsupported default does not prevent a healthy exact exception in DEV.
/// Cases: scen:60.7, airlog:55.18, airlog:55.25, airlog:55.3
pub fn initial_port_starting_policy(
    content: &CnaContent,
    port: &Port,
) -> Result<InitialPortStartingPolicy, PortStartingDiagnostic> {
    preflight_port_starting(content)?;
    if let Location::Hex { hex } = &port.location
        && let Some(record) = content
            .scenario
            .construction
            .port_overrides
            .iter()
            .find(|record| content.map.canonical(&record.hex) == content.map.canonical(hex))
    {
        let (canonical, category) =
            validate_record_identity(content, record).map_err(PortStartingDiagnostic::Override)?;
        if port.id != canonical.as_str() || port.name != category {
            return Err(PortStartingDiagnostic::Override(malformed(
                content,
                record,
                "authored identity contradicts the trusted verified port",
            )));
        }
        return override_policy(content, record);
    }
    let general = general_policy(content)?;
    if general == GeneralPolicy::ListedMax {
        let row = content.tables.airlog.port_capacity.port(port.name);
        Ok(InitialPortStartingPolicy::Known(InitialPortCondition {
            efficiency: row.max_efficiency_level,
            blocked_levels: 0,
        }))
    } else {
        Ok(InitialPortStartingPolicy::Unknown(general_gap(
            content, general,
        )))
    }
}

#[cfg(test)]
mod tests;
