//! Source-constructor provenance and the owned successful setup-close authority.
//! Cases: scen:59.2, scen:59.32, scen:59.36, airlog:34.0, airlog:35.24
//! Interpretations: interp:air-0018

use crate::air::state::AirRuntime;
use crate::state::{AirState, State, side_key};
use crate::CnaContent;
use cna_core::engine::EngineError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(crate) type OriginalAircraftCounts = BTreeMap<String, BTreeMap<String, i32>>;

/// Only fresh source construction seeds PendingOriginal. Checkpoint omission
/// remains legacy; flags, turns and empty inventory never reconstruct it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) enum InitialAirImport {
    #[default]
    LegacyUnknown,
    PendingOriginal { aircraft: OriginalAircraftCounts },
    Imported,
}

/// Consumed once by Air's direct original-ID allocation. There is deliberately
/// no Clone, Serialize or public constructor, and no existing PlaneId payload.
pub(crate) struct FreshAirSetupClose {
    original: OriginalAircraftCounts,
}

impl FreshAirSetupClose {
    pub(crate) fn original_aircraft(&self) -> &BTreeMap<String, BTreeMap<String, i32>> {
        &self.original
    }
}

fn invariant(detail: impl Into<String>) -> EngineError {
    EngineError::Invariant { detail: detail.into() }
}

fn add(counts: &mut BTreeMap<String, i32>, aircraft: &str, n: i32) -> Result<(), String> {
    if n < 0 {
        return Err("negative original aircraft count".into());
    }
    let old = counts.entry(aircraft.to_owned()).or_default();
    *old = old.checked_add(n).ok_or("original aircraft count overflow")?;
    Ok(())
}

pub(crate) fn original_aircraft_counts(content: &CnaContent) -> Result<OriginalAircraftCounts, String> {
    let mut result = BTreeMap::new();
    for force in &content.scenario.air {
        let mut counts = BTreeMap::new();
        for plane in &force.planes {
            add(&mut counts, &plane.aircraft, plane.total)?;
        }
        // Match State::new's existing last-force overwrite semantics exactly.
        result.insert(side_key(force.force.side).to_owned(), counts);
        if let Some(malta) = &force.malta {
            let mut counts = BTreeMap::new();
            for plane in &malta.planes {
                add(&mut counts, &plane.aircraft, plane.total)?;
            }
            result.insert("malta".to_owned(), counts);
        }
    }
    Ok(result)
}

fn aggregate_counts(air: &AirState) -> Result<OriginalAircraftCounts, String> {
    let mut result = BTreeMap::new();
    for (force, pool) in &air.forces {
        let mut counts = BTreeMap::new();
        for (aircraft, count) in &pool.planes {
            add(&mut counts, aircraft, count.total)?;
        }
        result.insert(force.clone(), counts);
    }
    for squadron in air.squadrons.values() {
        let counts = result.get_mut(&squadron.force).ok_or("unknown initial squadron force")?;
        for (aircraft, count) in &squadron.planes {
            add(counts, aircraft, count.total)?;
        }
    }
    Ok(result)
}

/// Called only from State::new after the original source pools are constructed.
/// This records counts, never canonical identities, readiness or load choices.
pub(crate) fn initial_air_seed(
    content: &CnaContent,
    air: &AirState,
) -> Result<InitialAirImport, String> {
    let aircraft = original_aircraft_counts(content)?;
    if air.runtime != AirRuntime::default() || aggregate_counts(air)? != aircraft {
        return Err("fresh source aircraft pools do not match original counts".into());
    }
    Ok(InitialAirImport::PendingOriginal { aircraft })
}

/// Oob validates before closing; Air repeats the same checks after closing and
/// before any allocation. The private token constructor additionally requires
/// the final branch still to be unclosed.
pub(crate) fn validate_initial_air_close(
    content: &CnaContent,
    state: &State,
    original: &OriginalAircraftCounts,
) -> Result<(), EngineError> {
    let InitialAirImport::PendingOriginal { aircraft } = &state.setup.initial_air_import else {
        return Err(invariant("original aircraft provenance is not pending"));
    };
    if aircraft != original || original_aircraft_counts(content).map_err(invariant)? != *original {
        return Err(invariant("original aircraft source snapshot changed"));
    }
    if !state.setup.started
        || !state.setup.air_started
        || !state.setup.pools_started
        || !state.setup.preload_started
        || !state.setup.tasks.is_empty()
        || state.decisions.pending.iter().any(|p| p.kind.starts_with("cna.setup."))
        || !state.setup.unit_locations.is_empty()
        || !state.setup.dump_locations.is_empty()
        || !state.setup.pool_locations.is_empty()
    {
        return Err(invariant("original aircraft require completed actual setup"));
    }
    if state.air.runtime != AirRuntime::default() {
        return Err(invariant("original aircraft already have canonical runtime"));
    }
    if aggregate_counts(&state.air).map_err(invariant)? != *original {
        return Err(invariant("original aircraft counts are not conserved"));
    }
    Ok(())
}

/// No caller outside setup's final-close path can construct this authority.
pub(super) fn authorize_close(
    content: &CnaContent,
    state: &State,
) -> Result<Option<FreshAirSetupClose>, EngineError> {
    match &state.setup.initial_air_import {
        InitialAirImport::LegacyUnknown => Ok(None),
        InitialAirImport::Imported => Err(invariant("original aircraft already imported")),
        InitialAirImport::PendingOriginal { aircraft } => {
            if state.setup.closed {
                return Err(invariant("cannot authenticate a previously closed setup"));
            }
            validate_initial_air_close(content, state, aircraft)?;
            Ok(Some(FreshAirSetupClose { original: aircraft.clone() }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> CnaContent {
        CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
    }

    // A guard fixture, not a positive whole-engine setup witness.
    fn completed_fixture(content: &CnaContent) -> State {
        let mut state = State::new(content).unwrap();
        state.setup.started = true;
        state.setup.air_started = true;
        state.setup.pools_started = true;
        state.setup.preload_started = true;
        state
    }

    /// Cases: scen:59.32, scen:60.46
    #[test]
    fn fresh_source_seed_preserves_original_force_type_totals_including_malta() {
        let content = content();
        let state = State::new(&content).unwrap();
        let InitialAirImport::PendingOriginal { aircraft } = &state.setup.initial_air_import else {
            panic!("fresh source must explicitly record original counts");
        };
        assert_eq!(*aircraft, original_aircraft_counts(&content).unwrap());
        assert_eq!(*aircraft, aggregate_counts(&state.air).unwrap());
        assert!(aircraft.contains_key("malta"));
        assert_eq!(state.air.runtime, AirRuntime::default());
        assert!(authorize_close(&content, &state).is_err());
    }

    /// Cases: scen:59.32, airlog:34.0
    #[test]
    fn omitted_checkpoint_provenance_and_forged_flags_never_mint_close_authority() {
        let content = content();
        let mut value = serde_json::to_value(State::new(&content).unwrap()).unwrap();
        value["setup"].as_object_mut().unwrap().remove("initial_air_import");
        let mut old: State = serde_json::from_value(value).unwrap();
        old.setup.started = true;
        old.setup.air_started = true;
        old.setup.pools_started = true;
        old.setup.preload_started = true;
        assert!(matches!(old.setup.initial_air_import, InitialAirImport::LegacyUnknown));
        assert!(authorize_close(&content, &old).unwrap().is_none());
        old.setup.closed = true;
        assert!(authorize_close(&content, &old).unwrap().is_none());
        let mut fresh = completed_fixture(&content);
        fresh.setup.closed = true;
        assert!(authorize_close(&content, &fresh).is_err());
    }

    /// Cases: scen:59.2, scen:59.32, airlog:34.0
    #[test]
    fn resumed_pending_snapshot_is_read_only_and_checks_both_close_contexts() {
        let content = content();
        let state = completed_fixture(&content);
        let restored: State = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        let before = serde_json::to_value(&restored).unwrap();
        let authority = authorize_close(&content, &restored).unwrap().unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), before);
        validate_initial_air_close(&content, &restored, authority.original_aircraft()).unwrap();
        let mut draft = restored.clone();
        draft.setup.closed = true;
        validate_initial_air_close(&content, &draft, authority.original_aircraft()).unwrap();
        draft.setup.initial_air_import = InitialAirImport::Imported;
        assert!(validate_initial_air_close(&content, &draft, authority.original_aircraft()).is_err());
        assert!(authorize_close(&content, &draft).is_err());
    }

    /// Cases: scen:59.32, airlog:34.0
    #[test]
    fn changed_source_counts_or_canonical_serials_refuse_without_mutation() {
        let content = content();
        let mut state = completed_fixture(&content);
        state.air.runtime.plane_serial.insert(cna_protocol::Side::Axis, 1);
        let before = serde_json::to_value(&state).unwrap();
        assert!(authorize_close(&content, &state).is_err());
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        state.air.runtime = AirRuntime::default();
        let force = state.air.forces.values_mut().next().unwrap();
        force.planes.values_mut().next().unwrap().total += 1;
        let before = serde_json::to_value(&state).unwrap();
        assert!(authorize_close(&content, &state).is_err());
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        let mut state = completed_fixture(&content);
        let InitialAirImport::PendingOriginal { aircraft } = &mut state.setup.initial_air_import else {
            panic!();
        };
        aircraft.clear();
        assert!(authorize_close(&content, &state).is_err());
    }

    /// Cases: scen:59.32, airlog:34.0
    #[test]
    fn source_empty_and_zero_type_rows_create_no_aircraft_and_addition_is_checked() {
        let mut content = content();
        content.scenario.air.clear();
        let state = completed_fixture(&content);
        let token = authorize_close(&content, &state).unwrap().unwrap();
        assert!(token.original_aircraft().is_empty());
        assert!(state.air.runtime.aircraft.is_empty());
        let mut rows = BTreeMap::new();
        add(&mut rows, "source.type", 0).unwrap();
        assert_eq!(rows["source.type"], 0);
        add(&mut rows, "source.type", i32::MAX).unwrap();
        assert!(add(&mut rows, "source.type", 1).is_err());
        assert!(add(&mut rows, "source.type", -1).is_err());
        assert_eq!(rows["source.type"], i32::MAX);
    }
}
