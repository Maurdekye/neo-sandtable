#![cfg(test)]

use super::*;
use crate::{
    Cna,
    state::AirSquadron,
    testkit::{assert_indistinguishable, visible_to},
};
use std::collections::BTreeSet;

fn content() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}

fn closed_initial(content: &CnaContent) -> State {
    let mut state = State::new(content).unwrap();
    state.setup.closed = true;
    state
}

fn serialized(air: &AirState) -> serde_json::Value {
    serde_json::to_value(air).unwrap()
}

/// Cases: airlog:34.0, airlog:35.24, scen:59.32
#[test]
fn real_scenario_counts_import_exactly_and_checkpoint_import_is_idempotent() {
    let content = content();
    let mut state = closed_initial(&content);
    let start = state.air.forces.clone();
    initialize(&content, &mut state).unwrap();
    assert_eq!(start, state.air.forces);
    let total: i32 = start
        .values()
        .flat_map(|f| f.planes.values())
        .map(|p| p.total)
        .sum();
    assert_eq!(
        state.air.runtime.aircraft.len(),
        usize::try_from(total).unwrap()
    );
    let pilots: i32 = start.values().flat_map(|f| f.pilots.values()).sum();
    assert_eq!(
        state.air.runtime.pilots.len(),
        usize::try_from(pilots).unwrap()
    );
    assert!(
        state
            .air
            .runtime
            .aircraft
            .values()
            .all(|p| p.fuelled && p.armed)
    );
    let checkpoint = serde_json::to_value(&state).unwrap();
    let mut restored: State = serde_json::from_value(checkpoint.clone()).unwrap();
    initialize(&content, &mut restored).unwrap();
    assert_eq!(checkpoint, serde_json::to_value(&restored).unwrap());
    let mut independent = closed_initial(&content);
    initialize(&content, &mut independent).unwrap();
    assert_eq!(serialized(&state.air), serialized(&independent.air));
}

/// Cases: airlog:34.0, scen:59.32
#[test]
fn import_rejects_open_setup_and_ambiguous_flags_without_partial_records() {
    let content = content();
    let mut state = State::new(&content).unwrap();
    let before = serialized(&state.air);
    assert!(initialize(&content, &mut state).is_err());
    assert_eq!(before, serialized(&state.air));
    state.setup.closed = true;
    state
        .air
        .forces
        .get_mut("axis")
        .unwrap()
        .planes
        .get_mut("it.cr42")
        .unwrap()
        .fuelled = 1;
    let before = serialized(&state.air);
    assert!(initialize(&content, &mut state).is_err());
    assert_eq!(before, serialized(&state.air));
}

fn add_squadron(state: &mut State, types: &[&str]) {
    let force = state.air.forces.get_mut("axis").unwrap();
    let mut planes = BTreeMap::new();
    for aircraft in types {
        let count = force.planes.get_mut(*aircraft).unwrap();
        count.total -= 1;
        count.ready -= 1;
        count.fuelled -= 1;
        count.armed -= 1;
        planes.insert(
            (*aircraft).into(),
            PlaneCount {
                total: 1,
                ready: 1,
                fuelled: 1,
                armed: 1,
            },
        );
    }
    *force.pilots.get_mut(&1).unwrap() -= 1;
    state.air.squadrons.insert(
        "axis.test".into(),
        AirSquadron {
            id: "axis.test".into(),
            force: "axis".into(),
            side: Side::Axis,
            nationality: "it".into(),
            facility: "test.facility".into(),
            initial_aircraft: None,
            planes,
            pilots: BTreeMap::from([(1, 1)]),
        },
    );
}

/// Cases: airlog:35.24
/// Source cases: airlog:40.12, airlog:40.16
/// Interpretations: interp:air-0005
#[test]
fn initial_pilot_training_is_known_only_for_a_single_present_type() {
    let content = content();
    let mut single = closed_initial(&content);
    add_squadron(&mut single, &["it.cr42"]);
    initialize(&content, &mut single).unwrap();
    let pilot = single
        .air
        .runtime
        .pilots
        .values()
        .find(|p| p.squadron.as_deref() == Some("axis.test"))
        .unwrap();
    assert_eq!(pilot.trained_aircraft.as_deref(), Some("it.cr42"));
    let mut mixed = closed_initial(&content);
    add_squadron(&mut mixed, &["it.cr42", "it.cr32"]);
    initialize(&content, &mut mixed).unwrap();
    let pilot = mixed
        .air
        .runtime
        .pilots
        .values()
        .find(|p| p.squadron.as_deref() == Some("axis.test"))
        .unwrap();
    assert_eq!(pilot.trained_aircraft, None);
    assert!(
        mixed
            .air
            .runtime
            .pilots
            .values()
            .filter(|p| p.squadron.is_none())
            .all(|p| p.trained_aircraft.is_none())
    );
}

/// Cases: airlog:34.0, airlog:35.24
#[test]
fn update_rebuilds_mirrors_and_rejects_invalid_edits_atomically() {
    let content = content();
    let mut state = closed_initial(&content);
    add_squadron(&mut state, &["it.cr42"]);
    initialize(&content, &mut state).unwrap();
    let id = state
        .air
        .runtime
        .aircraft
        .iter()
        .find(|(_, p)| p.squadron.is_some())
        .unwrap()
        .0
        .clone();
    update(&content, &mut state.air, |runtime| {
        let plane = runtime.aircraft.get_mut(&id).unwrap();
        plane.refitted = false;
        plane.fuelled = false;
        plane.armed = false;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        state.air.squadrons["axis.test"].planes["it.cr42"],
        PlaneCount {
            total: 1,
            ready: 0,
            fuelled: 0,
            armed: 0
        }
    );
    check(&content, &state.air).unwrap();
    let before = serialized(&state.air);
    assert!(
        update(&content, &mut state.air, |runtime| {
            runtime.aircraft.get_mut(&id).unwrap().squadron = Some("enemy-or-missing".into());
            Ok(())
        })
        .is_err()
    );
    assert_eq!(before, serialized(&state.air));
    assert!(
        update(&content, &mut state.air, |runtime| {
            runtime.aircraft.remove(&id);
            Err(invalid("test edit failed"))
        })
        .is_err()
    );
    assert_eq!(before, serialized(&state.air));
    state
        .air
        .squadrons
        .get_mut("axis.test")
        .unwrap()
        .planes
        .get_mut("it.cr42")
        .unwrap()
        .total += 1;
    assert!(check(&content, &state.air).is_err());
}

/// Cases: airlog:34.0, airlog:35.24
#[test]
fn private_serials_are_side_local_and_never_reused_after_removal() {
    let content = content();
    let mut a = closed_initial(&content);
    let mut b = a.clone();
    let axis = b
        .air
        .forces
        .get_mut("axis")
        .unwrap()
        .planes
        .get_mut("it.cr42")
        .unwrap();
    axis.total += 1;
    axis.ready += 1;
    axis.fuelled += 1;
    axis.armed += 1;
    initialize(&content, &mut a).unwrap();
    initialize(&content, &mut b).unwrap();
    let own = |state: &State| {
        state
            .air
            .runtime
            .aircraft
            .iter()
            .filter(|(_, p)| p.force != "axis")
            .map(|(id, p)| (id.clone(), p.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(own(&a), own(&b));
    let id = a.air.runtime.aircraft.keys().next().unwrap().clone();
    let plane = a.air.runtime.aircraft[&id].clone();
    let mut new_id = None;
    update(&content, &mut a.air, |runtime| {
        runtime.aircraft.remove(&id);
        new_id = Some(runtime.insert_aircraft(plane)?);
        Ok(())
    })
    .unwrap();
    assert_ne!(Some(id), new_id);
    let before = serialized(&a.air);
    assert!(
        update(&content, &mut a.air, |runtime| {
            runtime.aircraft.clear();
            runtime.plane_serial.clear();
            Ok(())
        })
        .is_err()
    );
    assert_eq!(before, serialized(&a.air));
}

/// Cases: land:3.62, airlog:34.0, airlog:35.24
#[test]
fn every_inventory_flag_assignment_and_private_id_stays_hidden_from_enemy() {
    let content = content();
    let mut a = closed_initial(&content);
    add_squadron(&mut a, &["it.cr42", "it.cr32"]);
    initialize(&content, &mut a).unwrap();
    let plane = a
        .air
        .runtime
        .aircraft
        .iter()
        .find(|(_, p)| p.squadron.is_some())
        .unwrap()
        .0
        .clone();
    let pilot = a
        .air
        .runtime
        .pilots
        .iter()
        .find(|(_, p)| p.squadron.is_some())
        .unwrap()
        .0
        .clone();
    for fact in 0..6 {
        let mut b = a.clone();
        update(&content, &mut b.air, |runtime| {
            match fact {
                0 => runtime.aircraft.get_mut(&plane).unwrap().refitted = false,
                1 => runtime.aircraft.get_mut(&plane).unwrap().fuelled = false,
                2 => runtime.aircraft.get_mut(&plane).unwrap().armed = false,
                3 => {
                    runtime.aircraft.get_mut(&plane).unwrap().facility =
                        Some("another.private.facility".into())
                }
                4 => {
                    runtime.pilots.get_mut(&pilot).unwrap().trained_aircraft =
                        Some("it.cr42".into())
                }
                5 => {
                    runtime.aircraft.remove(&plane);
                }
                _ => unreachable!(),
            }
            Ok(())
        })
        .unwrap();
        assert_indistinguishable(&Cna::dev(), &content, &a, &b, Side::Commonwealth);
        let targets: BTreeSet<_> = a
            .air
            .runtime
            .aircraft
            .keys()
            .map(|id| id.0.clone())
            .chain(a.air.runtime.pilots.keys().map(|id| id.0.clone()))
            .chain(["axis.test".into()])
            .collect();
        assert_eq!(
            visible_to(&Cna::dev(), &content, &a, Side::Commonwealth, &targets),
            visible_to(&Cna::dev(), &content, &b, Side::Commonwealth, &targets)
        );
    }
}

/// Cases: airlog:34.0
#[test]
fn old_checkpoint_without_runtime_defaults_to_uninitialized_inventory() {
    let content = content();
    let state = closed_initial(&content);
    let mut value = serde_json::to_value(&state).unwrap();
    value["air"].as_object_mut().unwrap().remove("runtime");
    let mut old: State = serde_json::from_value(value).unwrap();
    assert!(!old.air.runtime.initialized());
    initialize(&content, &mut old).unwrap();
    check(&content, &old.air).unwrap();
}

/// Cases: airlog:34.8, airlog:34.84
#[test]
fn unassigned_arrivals_preserve_old_total_only_semantics_and_checkpoint() {
    let content = content();
    let mut state = closed_initial(&content);
    initialize(&content, &mut state).unwrap();
    let before = state.air.forces["axis"].planes["it.cr42"];
    let pilots = state.air.runtime.pilots.clone();
    let ids = receive_unassigned(&content, &mut state.air, Side::Axis, "it.cr42", 2).unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(
        state.air.forces["axis"].planes["it.cr42"],
        PlaneCount {
            total: before.total + 2,
            ..before
        }
    );
    assert_eq!(state.air.runtime.pilots, pilots);
    for id in &ids {
        assert_eq!(
            state.air.runtime.aircraft[id],
            AircraftState {
                aircraft: "it.cr42".into(),
                force: "axis".into(),
                squadron: None,
                facility: None,
                refitted: false,
                fuelled: false,
                armed: false,
            }
        );
    }
    check(&content, &state.air).unwrap();
    let bytes = serde_json::to_vec(&state).unwrap();
    let mut restored: State = serde_json::from_slice(&bytes).unwrap();
    initialize(&content, &mut restored).unwrap();
    assert_eq!(bytes, serde_json::to_vec(&restored).unwrap());
}

/// Cases: airlog:34.8, airlog:34.84
#[test]
fn failed_arrivals_leave_records_mirrors_and_serials_unchanged() {
    let content = content();
    let mut state = closed_initial(&content);
    let bytes = serde_json::to_vec(&state).unwrap();
    assert!(receive_unassigned(&content, &mut state.air, Side::Axis, "it.cr42", 1).is_err());
    assert_eq!(bytes, serde_json::to_vec(&state).unwrap());
    initialize(&content, &mut state).unwrap();
    for (aircraft, n) in [("it.cr42", 0), ("it.cr42", -1), ("it.cr42", i32::MAX)] {
        let bytes = serde_json::to_vec(&state).unwrap();
        assert!(receive_unassigned(&content, &mut state.air, Side::Axis, aircraft, n).is_err());
        assert_eq!(bytes, serde_json::to_vec(&state).unwrap());
    }

    let bytes = serde_json::to_vec(&state).unwrap();
    assert!(
        matches!(receive_unassigned(&content, &mut state.air, Side::Axis, "unknown", 1),
        Err(EngineError::Unsupported { case, .. }) if case == "airlog:34.84")
    );
    assert_eq!(bytes, serde_json::to_vec(&state).unwrap());
    // One draft allocation succeeds before the second exhausts the serial;
    // neither allocation is published.
    state
        .air
        .runtime
        .plane_serial
        .insert(Side::Axis, u64::MAX - 1);
    check(&content, &state.air).unwrap();
    let bytes = serde_json::to_vec(&state).unwrap();
    assert!(receive_unassigned(&content, &mut state.air, Side::Axis, "it.cr42", 2).is_err());
    assert_eq!(bytes, serde_json::to_vec(&state).unwrap());
    let mut missing = state.clone();
    missing.air.forces.remove("axis");
    let bytes = serde_json::to_vec(&missing).unwrap();
    assert!(receive_unassigned(&content, &mut missing.air, Side::Axis, "it.cr42", 1).is_err());
    assert_eq!(bytes, serde_json::to_vec(&missing).unwrap());
}

/// Cases: land:3.62, airlog:34.8, airlog:34.84
#[test]
fn private_arrivals_are_hidden_and_other_side_serials_do_not_change_ids() {
    let content = content();
    let mut a = closed_initial(&content);
    initialize(&content, &mut a).unwrap();
    let mut b = a.clone();
    let targets: BTreeSet<_> = receive_unassigned(&content, &mut b.air, Side::Axis, "it.cr42", 2)
        .unwrap()
        .into_iter()
        .map(|id| id.0)
        .collect();
    assert_indistinguishable(&Cna::dev(), &content, &a, &b, Side::Commonwealth);
    assert_eq!(
        visible_to(&Cna::dev(), &content, &a, Side::Commonwealth, &targets),
        visible_to(&Cna::dev(), &content, &b, Side::Commonwealth, &targets)
    );
    let own_type = a.air.forces["commonwealth"]
        .planes
        .keys()
        .next()
        .unwrap()
        .clone();
    let ids_a = receive_unassigned(&content, &mut a.air, Side::Commonwealth, &own_type, 1).unwrap();
    let ids_b = receive_unassigned(&content, &mut b.air, Side::Commonwealth, &own_type, 1).unwrap();
    assert_eq!(ids_a, ids_b);
    assert_eq!(
        a.air.runtime.aircraft[&ids_a[0]],
        b.air.runtime.aircraft[&ids_b[0]]
    );
}
