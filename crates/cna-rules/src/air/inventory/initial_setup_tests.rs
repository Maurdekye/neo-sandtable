use super::*;

fn content() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}

// Exercise the private builder without fabricating setup-close authority.
// Actual provenance, retry and whole-State close tests belong to setup.
/// Cases: scen:59.32, scen:59.36, airlog:34.0, airlog:35.24
/// Interpretations: interp:air-0018
#[test]
fn fresh_and_legacy_allocation_have_identical_inventory_but_distinct_knowledge() {
    let content = content();
    let state = State::new(&content).unwrap();
    let fresh = initial_inventory(&content, &state.air, InitialAdmission::FreshSetup).unwrap();
    let legacy = initial_inventory(&content, &state.air, InitialAdmission::Legacy).unwrap();
    assert_eq!(fresh.forces, state.air.forces);
    assert_eq!(fresh.squadrons, state.air.squadrons);
    assert_eq!(fresh.runtime.aircraft, legacy.runtime.aircraft);
    assert_eq!(fresh.runtime.pilots, legacy.runtime.pilots);
    assert_eq!(fresh.runtime.plane_serial, legacy.runtime.plane_serial);
    assert_eq!(fresh.runtime.pilot_serial, legacy.runtime.pilot_serial);
    assert_eq!(fresh.runtime.facilities, legacy.runtime.facilities);
    assert_eq!(fresh.runtime.sgsus, legacy.runtime.sgsus);
    assert!(fresh.runtime.preparation.initialized);
    assert_eq!(
        fresh.runtime.preparation.initial_choices.len(),
        fresh.runtime.aircraft.len()
    );
    for id in fresh.runtime.aircraft.keys() {
        assert_eq!(
            fresh.runtime.preparation.initial_choices[id],
            InitialChoice::Unselected
        );
        assert_eq!(
            fresh.runtime.preparation.current[id],
            CurrentPreparation::InitialUnspecified
        );
        assert_eq!(
            legacy.runtime.preparation.current[id],
            CurrentPreparation::UnknownLegacy
        );
    }
    assert!(legacy.runtime.preparation.initial_choices.is_empty());
}

/// Cases: scen:59.32, airlog:34.0
/// Interpretations: interp:air-0018
#[test]
fn fresh_builder_checkpoint_generic_initialize_and_ordinary_updates_preserve_ledger() {
    let content = content();
    let mut state = State::new(&content).unwrap();
    state.setup.closed = true;
    state.air = initial_inventory(&content, &state.air, InitialAdmission::FreshSetup).unwrap();
    let checkpoint = serde_json::to_vec(&state).unwrap();
    let mut restored: State = serde_json::from_slice(&checkpoint).unwrap();
    initialize(&content, &mut restored).unwrap();
    assert_eq!(checkpoint, serde_json::to_vec(&restored).unwrap());
    let id = restored.air.runtime.aircraft.keys().next().unwrap().clone();
    assert!(matches!(
        update(&content, &mut restored.air, |runtime| {
            runtime.preparation.initial_choices.remove(&id);
            runtime
                .preparation
                .current
                .insert(id, CurrentPreparation::UnknownLegacy);
            Ok(())
        }),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(checkpoint, serde_json::to_vec(&restored).unwrap());
    assert!(matches!(
        initial_inventory(&content, &restored.air, InitialAdmission::FreshSetup),
        Err(EngineError::Invariant { .. })
    ));
}

/// Cases: scen:59.32, airlog:34.0, airlog:35.24
/// Interpretations: interp:air-0018
#[test]
fn legacy_aircraft_cannot_gain_initial_history_through_ordinary_update() {
    let content = content();
    let mut state = State::new(&content).unwrap();
    state.setup.closed = true;
    initialize(&content, &mut state).unwrap();
    let before = serde_json::to_vec(&state).unwrap();
    let id = state.air.runtime.aircraft.keys().next().unwrap().clone();
    assert!(matches!(
        update(&content, &mut state.air, |runtime| {
            runtime
                .preparation
                .initial_choices
                .insert(id.clone(), InitialChoice::Unselected);
            runtime
                .preparation
                .current
                .insert(id, CurrentPreparation::InitialUnspecified);
            Ok(())
        }),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(before, serde_json::to_vec(&state).unwrap());
}
