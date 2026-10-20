use super::*;
use crate::{
    Cna,
    air::preparation::{CarriedOrdnance, KnownLoad, PreparationState},
    testkit::{assert_indistinguishable, visible_to},
};
use std::collections::BTreeSet;

fn content() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}

fn initialized(content: &CnaContent) -> State {
    let mut state = State::new(content).unwrap();
    state.setup.closed = true;
    initialize(content, &mut state).unwrap();
    state
}

// Structural fixtures are not admissions of initial credit or paid preparation.
fn single(content: &CnaContent, aircraft: &str) -> (State, PlaneId) {
    let mut state = initialized(content);
    // Source-validation fixtures may exercise a source type absent at scenario start.
    // Reuse a same-side private fixture identity; this is not an arrival/import path.
    let force = if aircraft.starts_with("cw.") {
        "commonwealth"
    } else {
        "axis"
    };
    let id = state
        .air
        .runtime
        .aircraft
        .iter()
        .find(|(_, p)| p.aircraft == aircraft)
        .or_else(|| {
            state
                .air
                .runtime
                .aircraft
                .iter()
                .find(|(_, p)| p.force == force)
        })
        .unwrap()
        .0
        .clone();
    let plane = state.air.runtime.aircraft.get_mut(&id).unwrap();
    plane.aircraft = aircraft.into();
    plane.squadron = None;
    plane.facility = None;
    state.air.runtime.aircraft.retain(|key, _| key == &id);
    state
        .air
        .runtime
        .preparation
        .current
        .retain(|key, _| key == &id);
    state.air.runtime.pilots.clear();
    state.air.runtime.sgsus.clear();
    state.air.squadrons.clear();
    refresh(content, &mut state.air);
    (state, id)
}

fn refresh(content: &CnaContent, air: &mut AirState) {
    *air = mirrors(content, air, None).unwrap();
}

fn known(mode: usize, guns: bool, ordnance: Option<CarriedOrdnance>) -> CurrentPreparation {
    CurrentPreparation::KnownLoad {
        load: KnownLoad {
            mode,
            fuel_available: true,
            gun_ammunition: guns,
            ordnance,
        },
    }
}

fn fixture_current(
    content: &CnaContent,
    air: &mut AirState,
    id: &PlaneId,
    current: CurrentPreparation,
) {
    let bit = preparation::armed_projection(&current, air.runtime.aircraft[id].armed);
    air.runtime.aircraft.get_mut(id).unwrap().armed = bit;
    if let Some((_, available)) = preparation::known_fuel(&current) {
        air.runtime.aircraft.get_mut(id).unwrap().fuelled = available;
    }
    air.runtime.preparation.current.insert(id.clone(), current);
    refresh(content, air);
    check(content, air).unwrap();
}

fn bytes(air: &AirState) -> Vec<u8> {
    serde_json::to_vec(air).unwrap()
}

fn rejects_edit<F>(content: &CnaContent, air: &AirState, edit: F)
where
    F: FnOnce(&mut AirRuntime) -> Result<(), EngineError>,
{
    let mut draft = air.clone();
    let before = bytes(&draft);
    assert!(matches!(
        update(content, &mut draft, edit),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(before, bytes(&draft));
}

/// Cases: airlog:34.0, airlog:34.14, scen:59.32
/// Interpretations: interp:air-0018
#[test]
fn every_live_matrix_cell_and_nonlive_current_is_strict_without_repair() {
    let content = content();
    let (base, id) = single(&content, "it.cr42");
    let ledger = [
        Some(InitialChoice::Unselected),
        Some(InitialChoice::Committed),
        Some(InitialChoice::Forfeited),
        None,
    ];
    let current = [
        Some(CurrentPreparation::InitialUnspecified),
        Some(CurrentPreparation::UnknownLegacy),
        Some(CurrentPreparation::Unprepared),
        Some(known(0, false, None)),
        Some(CurrentPreparation::KnownFuel {
            mode: 0,
            fuel_available: true,
        }),
        None,
    ];
    for (row, credit) in ledger.iter().enumerate() {
        for (column, carried) in current.iter().enumerate() {
            let mut state = base.clone();
            if let Some(credit) = credit {
                state
                    .air
                    .runtime
                    .preparation
                    .initial_choices
                    .insert(id.clone(), credit.clone());
            }
            state.air.runtime.preparation.current.clear();
            if let Some(carried) = carried {
                state.air.runtime.aircraft.get_mut(&id).unwrap().armed =
                    preparation::armed_projection(carried, true);
                if let Some((_, available)) = preparation::known_fuel(carried) {
                    state.air.runtime.aircraft.get_mut(&id).unwrap().fuelled = available;
                }
                state
                    .air
                    .runtime
                    .preparation
                    .current
                    .insert(id.clone(), carried.clone());
            }
            refresh(&content, &mut state.air);
            let before = bytes(&state.air);
            let result = check(&content, &state.air);
            let valid = matches!((row, column), (0, 0) | (1 | 2, 2 | 3) | (3, 1..=4));
            if valid {
                result.unwrap();
            } else {
                assert!(
                    matches!(result, Err(EngineError::Invariant { .. })),
                    "row {row}, column {column}"
                );
            }
            assert_eq!(before, bytes(&state.air));
        }
    }
    for credit in &ledger {
        let mut state = base.clone();
        state.air.runtime.aircraft.clear();
        state.air.runtime.preparation.current.clear();
        if let Some(credit) = credit {
            state
                .air
                .runtime
                .preparation
                .initial_choices
                .insert(id.clone(), credit.clone());
        }
        refresh(&content, &mut state.air);
        check(&content, &state.air).unwrap();
        for carried in current.iter().flatten() {
            state
                .air
                .runtime
                .preparation
                .current
                .insert(id.clone(), carried.clone());
            let before = bytes(&state.air);
            assert!(matches!(
                check(&content, &state.air),
                Err(EngineError::Invariant { .. })
            ));
            assert_eq!(before, bytes(&state.air));
            state.air.runtime.preparation.current.clear();
        }
    }
    for bad_id in [
        "axis.aircraft-0",
        "axis.aircraft-01",
        "axis.aircraft-18446744073709551615",
        "enemy.aircraft-1",
    ] {
        let mut air = base.air.clone();
        air.runtime
            .preparation
            .initial_choices
            .insert(PlaneId(bad_id.into()), InitialChoice::Forfeited);
        let before = bytes(&air);
        assert!(matches!(
            check(&content, &air),
            Err(EngineError::Invariant { .. })
        ));
        assert_eq!(before, bytes(&air));
    }
}

/// Cases: airlog:34.0, scen:59.32, scen:59.36
/// Interpretations: interp:air-0018
#[test]
fn absent_checkpoint_fields_migrate_once_without_credit_or_coarse_changes() {
    let content = content();
    let base = initialized(&content);
    for remove_whole in [true, false] {
        let mut value = serde_json::to_value(&base).unwrap();
        let runtime = value["air"]["runtime"].as_object_mut().unwrap();
        if remove_whole {
            runtime.remove("preparation");
        } else {
            runtime.insert(
                "preparation".into(),
                serde_json::json!({"initial_choices":{},"current":{}}),
            );
        }
        let mut old: State = serde_json::from_value(value).unwrap();
        assert!(!old.air.runtime.preparation.initialized);
        let before = bytes(&old.air);
        check(&content, &old.air).unwrap();
        assert_eq!(before, bytes(&old.air));
        initialize(&content, &mut old).unwrap();
        assert_eq!(old.air.runtime.aircraft, base.air.runtime.aircraft);
        assert_eq!(old.air.runtime.pilots, base.air.runtime.pilots);
        assert_eq!(old.air.runtime.plane_serial, base.air.runtime.plane_serial);
        assert_eq!(old.air.runtime.pilot_serial, base.air.runtime.pilot_serial);
        assert_eq!(old.air.forces, base.air.forces);
        assert_eq!(old.air.squadrons, base.air.squadrons);
        assert!(old.air.runtime.preparation.initialized);
        assert!(old.air.runtime.preparation.initial_choices.is_empty());
        assert_eq!(
            old.air.runtime.preparation.current.len(),
            old.air.runtime.aircraft.len()
        );
        assert!(
            old.air
                .runtime
                .preparation
                .current
                .values()
                .all(|p| *p == CurrentPreparation::UnknownLegacy)
        );
        let checkpoint = serde_json::to_vec(&old).unwrap();
        let mut restored: State = serde_json::from_slice(&checkpoint).unwrap();
        initialize(&content, &mut restored).unwrap();
        update(&content, &mut restored.air, |_| Ok(())).unwrap();
        assert_eq!(checkpoint, serde_json::to_vec(&restored).unwrap());
    }
    // Generic closed setup, original GT1 and source armed bits never authenticate credit.
    assert!(base.air.runtime.aircraft.values().all(|p| p.armed));
    assert!(base.air.runtime.preparation.initial_choices.is_empty());
    assert!(
        base.air
            .runtime
            .preparation
            .current
            .values()
            .all(|p| *p == CurrentPreparation::UnknownLegacy)
    );
}

/// Cases: airlog:34.0, scen:59.32
/// Interpretations: interp:air-0018
#[test]
fn false_populated_strict_missing_and_failed_migration_preserve_complete_bytes() {
    let content = content();
    let (base, id) = single(&content, "it.cr42");
    for ledger in [false, true] {
        let mut state = base.clone();
        state.air.runtime.preparation = PreparationState::default();
        if ledger {
            state
                .air
                .runtime
                .preparation
                .initial_choices
                .insert(id.clone(), InitialChoice::Unselected);
        } else {
            state
                .air
                .runtime
                .preparation
                .current
                .insert(id.clone(), CurrentPreparation::UnknownLegacy);
        }
        let before = bytes(&state.air);
        assert!(matches!(
            initialize(&content, &mut state),
            Err(EngineError::Invariant { .. })
        ));
        assert!(matches!(
            update(&content, &mut state.air, |_| Ok(())),
            Err(EngineError::Invariant { .. })
        ));
        assert_eq!(before, bytes(&state.air));
        assert!(!state.air.runtime.preparation.initialized);
    }
    let mut state = base.clone();
    state.air.runtime.preparation.current.remove(&id);
    let before = bytes(&state.air);
    assert!(matches!(
        initialize(&content, &mut state),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(before, bytes(&state.air));
    let mut legacy = base.clone();
    legacy.air.runtime.preparation = PreparationState::default();
    let before = bytes(&legacy.air);
    assert!(matches!(
        update(&content, &mut legacy.air, |runtime| {
            assert!(runtime.preparation.initialized);
            assert_eq!(
                runtime.preparation.current[&id],
                CurrentPreparation::UnknownLegacy
            );
            runtime.aircraft.get_mut(&id).unwrap().squadron = Some("invalid".into());
            Ok(())
        }),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(before, bytes(&legacy.air));
    assert!(!legacy.air.runtime.preparation.initialized);
}

/// Cases: airlog:34.0, scen:59.32
/// Interpretations: interp:air-0018
#[test]
fn zero_aircraft_migration_is_permanent_and_empty_does_not_reimport() {
    let content = content();
    let (mut state, _) = single(&content, "it.cr42");
    state.air.runtime.aircraft.clear();
    state.air.runtime.preparation = PreparationState::default();
    refresh(&content, &mut state.air);
    let serials = state.air.runtime.plane_serial.clone();
    initialize(&content, &mut state).unwrap();
    assert!(state.air.runtime.preparation.initialized);
    assert!(state.air.runtime.preparation.current.is_empty());
    assert_eq!(serials, state.air.runtime.plane_serial);
    let checkpoint = serde_json::to_vec(&state).unwrap();
    initialize(&content, &mut state).unwrap();
    update(&content, &mut state.air, |_| Ok(())).unwrap();
    assert_eq!(checkpoint, serde_json::to_vec(&state).unwrap());
    assert!(state.air.runtime.aircraft.is_empty());
    rejects_edit(&content, &state.air, |r| {
        r.preparation.initialized = false;
        Ok(())
    });
}

/// Cases: airlog:34.8, airlog:34.84, scen:59.32
/// Interpretations: interp:air-0018
#[test]
fn mixed_legacy_receipt_bootstraps_old_ids_before_new_and_late_overflow_rolls_back() {
    let content = content();
    let mut state = initialized(&content);
    state.air.runtime.preparation = PreparationState::default();
    let planes = state.air.runtime.aircraft.clone();
    let pilots = state.air.runtime.pilots.clone();
    let old_serial = state.air.runtime.plane_serial[&Side::Axis];
    let old_count = state.air.forces["axis"].planes["it.cr42"];
    let original = state.clone();
    let ids = receive_unassigned(&content, &mut state.air, Side::Axis, "it.cr42", 2).unwrap();
    assert_eq!(
        ids,
        [
            PlaneId(format!("axis.aircraft-{}", old_serial + 1)),
            PlaneId(format!("axis.aircraft-{}", old_serial + 2))
        ]
    );
    for (id, plane) in planes {
        assert_eq!(state.air.runtime.aircraft[&id], plane);
        assert_eq!(
            state.air.runtime.preparation.current[&id],
            CurrentPreparation::UnknownLegacy
        );
    }
    for id in ids {
        assert_eq!(
            state.air.runtime.preparation.current[&id],
            CurrentPreparation::Unprepared
        );
        let p = &state.air.runtime.aircraft[&id];
        assert!(!p.armed && !p.refitted && !p.fuelled);
        assert!(p.facility.is_none() && p.squadron.is_none());
    }
    assert_eq!(pilots, state.air.runtime.pilots);
    assert_eq!(
        state.air.forces["axis"].planes["it.cr42"],
        PlaneCount {
            total: old_count.total + 2,
            ..old_count
        }
    );
    assert!(state.air.runtime.preparation.initial_choices.is_empty());
    let checkpoint = serde_json::to_vec(&state).unwrap();
    let mut restored: State = serde_json::from_slice(&checkpoint).unwrap();
    initialize(&content, &mut restored).unwrap();
    assert_eq!(checkpoint, serde_json::to_vec(&restored).unwrap());
    let mut overflow = original;
    overflow
        .air
        .runtime
        .plane_serial
        .insert(Side::Axis, u64::MAX - 1);
    let before = bytes(&overflow.air);
    assert!(matches!(
        receive_unassigned(&content, &mut overflow.air, Side::Axis, "it.cr42", 2),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(before, bytes(&overflow.air));
    assert!(!overflow.air.runtime.preparation.initialized);
}

/// Cases: airlog:34.0, scen:59.32, scen:59.36
/// Interpretations: interp:air-0018
#[test]
fn initial_ledger_is_frozen_and_tombstones_cannot_resurrect() {
    let content = content();
    let (base, id) = single(&content, "it.cr42");
    for credit in [
        InitialChoice::Unselected,
        InitialChoice::Committed,
        InitialChoice::Forfeited,
    ] {
        let mut state = base.clone();
        state
            .air
            .runtime
            .preparation
            .initial_choices
            .insert(id.clone(), credit.clone());
        let current = if credit == InitialChoice::Unselected {
            CurrentPreparation::InitialUnspecified
        } else {
            known(0, true, None)
        };
        fixture_current(&content, &mut state.air, &id, current);
        rejects_edit(&content, &state.air, |r| {
            r.preparation.initial_choices.remove(&id);
            Ok(())
        });
        for replacement in [
            InitialChoice::Unselected,
            InitialChoice::Committed,
            InitialChoice::Forfeited,
        ] {
            if replacement != credit {
                rejects_edit(&content, &state.air, |r| {
                    r.preparation
                        .initial_choices
                        .insert(id.clone(), replacement);
                    Ok(())
                });
            }
        }
        let plane = state.air.runtime.aircraft[&id].clone();
        update(&content, &mut state.air, |r| {
            r.aircraft.remove(&id);
            r.preparation.current.remove(&id);
            Ok(())
        })
        .unwrap();
        assert_eq!(state.air.runtime.preparation.initial_choices[&id], credit);
        check(&content, &state.air).unwrap();
        rejects_edit(&content, &state.air, |r| {
            r.aircraft.insert(id.clone(), plane);
            r.preparation.current.insert(
                id.clone(),
                if credit == InitialChoice::Unselected {
                    CurrentPreparation::InitialUnspecified
                } else {
                    known(0, true, None)
                },
            );
            Ok(())
        });
        rejects_edit(&content, &state.air, |r| {
            r.preparation.initial_choices.clear();
            Ok(())
        });
    }
    for current in [CurrentPreparation::UnknownLegacy, known(0, true, None)] {
        let mut air = base.air.clone();
        fixture_current(&content, &mut air, &id, current);
        for credit in [
            InitialChoice::Unselected,
            InitialChoice::Committed,
            InitialChoice::Forfeited,
        ] {
            rejects_edit(&content, &air, |r| {
                r.preparation.initial_choices.insert(id.clone(), credit);
                r.preparation
                    .current
                    .insert(id.clone(), CurrentPreparation::InitialUnspecified);
                Ok(())
            });
        }
        rejects_edit(&content, &air, |r| {
            r.preparation
                .current
                .insert(id.clone(), CurrentPreparation::InitialUnspecified);
            Ok(())
        });
    }
    // No real setup-admission API exists here; its positive and attempted token paths stay B3-held.
}

/// Cases: airlog:34.0, scen:59.32
/// Interpretations: interp:air-0018
#[test]
fn preparation_and_identity_guards_compare_the_post_bootstrap_snapshot() {
    let content = content();
    let (base, id) = single(&content, "it.cr42");
    for initial in [
        CurrentPreparation::UnknownLegacy,
        CurrentPreparation::Unprepared,
        known(0, true, None),
    ] {
        let mut air = base.air.clone();
        fixture_current(&content, &mut air, &id, initial.clone());
        let rejected: Vec<Option<CurrentPreparation>> = match initial {
            CurrentPreparation::KnownLoad { .. } => vec![
                None,
                Some(CurrentPreparation::UnknownLegacy),
                Some(CurrentPreparation::Unprepared),
            ],
            CurrentPreparation::Unprepared => vec![Some(CurrentPreparation::UnknownLegacy)],
            _ => vec![Some(CurrentPreparation::Unprepared)],
        };
        for next in rejected {
            rejects_edit(&content, &air, |r| {
                if let Some(next) = next {
                    r.preparation.current.insert(id.clone(), next);
                } else {
                    r.preparation.current.remove(&id);
                }
                Ok(())
            });
        }
        rejects_edit(&content, &air, |r| {
            r.aircraft.get_mut(&id).unwrap().aircraft = "it.cr32".into();
            Ok(())
        });
        rejects_edit(&content, &air, |r| {
            r.aircraft.get_mut(&id).unwrap().force = "commonwealth".into();
            Ok(())
        });
        rejects_edit(&content, &air, |r| {
            r.preparation.initialized = false;
            Ok(())
        });
        rejects_edit(&content, &air, |r| {
            r.preparation.current.clear();
            Ok(())
        });
    }
    for legacy in [false, true] {
        let mut air = base.air.clone();
        if legacy {
            air.runtime.preparation = PreparationState::default();
        }
        rejects_edit(&content, &air, |r| {
            r.preparation
                .current
                .insert(id.clone(), CurrentPreparation::Unprepared);
            Ok(())
        });
        rejects_edit(&content, &air, |r| {
            r.aircraft.get_mut(&id).unwrap().armed = false;
            Ok(())
        });
        let plane = air.runtime.aircraft[&id].clone();
        rejects_edit(&content, &air, |r| {
            let new = r.insert_aircraft(plane)?;
            r.preparation
                .current
                .insert(new, CurrentPreparation::UnknownLegacy);
            Ok(())
        });
    }
    let mut initial = base.air.clone();
    initial
        .runtime
        .preparation
        .initial_choices
        .insert(id.clone(), InitialChoice::Unselected);
    fixture_current(
        &content,
        &mut initial,
        &id,
        CurrentPreparation::InitialUnspecified,
    );
    rejects_edit(&content, &initial, |r| {
        r.aircraft.get_mut(&id).unwrap().armed = false;
        Ok(())
    });
}

/// Cases: airlog:34.14, airlog:38.4, airlog:38.47, airlog:41.39
/// Interpretations: interp:air-0018
#[test]
fn sole_projection_updates_mirrors_preserves_refit_and_propagates_source_errors_atomically() {
    let mut content = content();
    let (base, id) = single(&content, "it.z506b");
    let mode = &content.units.aircraft["it.z506b"].modes[0];
    let bomb = mode.bomb_capacity.unwrap();
    let torpedo = mode.torpedo_capacity.unwrap();
    assert!(bomb > 0 && torpedo > 0);
    for ordnance in [
        None,
        Some(CarriedOrdnance::Bombs { bomb_points: 0 }),
        Some(CarriedOrdnance::Bombs { bomb_points: bomb }),
        Some(CarriedOrdnance::Mines {
            source_bombload_rating: bomb,
        }),
        Some(CarriedOrdnance::Torpedoes {
            rating_points: torpedo,
        }),
    ] {
        for guns in [false, true] {
            let mut air = base.air.clone();
            let before = air.runtime.aircraft[&id].clone();
            // Private fixture establishes a known record, never a paid procedure.
            fixture_current(&content, &mut air, &id, known(0, true, None));
            // Structural projection fixture, not permission to purchase a load
            // through an ordinary callback. Exercise the canonical projection
            // directly, then prove the resulting record survives an ordinary no-op.
            let old = air.runtime.clone();
            air.runtime
                .preparation
                .current
                .insert(id.clone(), known(0, guns, ordnance.clone()));
            project_preparation(&old, &mut air.runtime).unwrap();
            refresh(&content, &mut air);
            update(&content, &mut air, |_| Ok(())).unwrap();
            let expected = guns
                || ordnance
                    .as_ref()
                    .is_some_and(|p| !matches!(p, CarriedOrdnance::Bombs { bomb_points: 0 }));
            let plane = &air.runtime.aircraft[&id];
            assert_eq!(plane.armed, expected);
            assert_eq!(plane.refitted, before.refitted);
            assert_eq!(plane.fuelled, before.fuelled);
            assert_eq!(
                air.forces[&plane.force].planes[&plane.aircraft].armed,
                i32::from(expected)
            );
            assert!(air.runtime.preparation.initial_choices.is_empty());
            check(&content, &air).unwrap();
        }
    }
    let mut known_air = base.air.clone();
    fixture_current(&content, &mut known_air, &id, known(0, true, None));
    for bad in [
        known(usize::MAX, true, None),
        known(0, true, Some(CarriedOrdnance::Bombs { bomb_points: -1 })),
        known(
            0,
            true,
            Some(CarriedOrdnance::Bombs {
                bomb_points: bomb + 1,
            }),
        ),
        known(
            0,
            true,
            Some(CarriedOrdnance::Mines {
                source_bombload_rating: bomb - 1,
            }),
        ),
    ] {
        rejects_edit(&content, &known_air, |r| {
            r.preparation.current.insert(id.clone(), bad);
            Ok(())
        });
    }
    // Existing carried source facts remain checked before any later edit;
    // this setup does not use the ordinary callback to create unpaid mines.
    fixture_current(
        &content,
        &mut known_air,
        &id,
        known(
            0,
            true,
            Some(CarriedOrdnance::Mines {
                source_bombload_rating: bomb,
            }),
        ),
    );
    content.units.aircraft.get_mut("it.z506b").unwrap().modes[0].bomb_capacity = None;
    let before = bytes(&known_air);
    assert!(
        matches!(update(&content, &mut known_air, |r| { r.preparation.current.insert(id.clone(), known(0, false, Some(CarriedOrdnance::Mines { source_bombload_rating: bomb }))); Ok(()) }), Err(EngineError::Unsupported { case, detail }) if case == "airlog:34.14" && detail == "mines rating missing for it.z506b mode 0")
    );
    assert_eq!(before, bytes(&known_air));
}

/// Cases: land:3.62, airlog:34.0, airlog:34.14, scen:59.32
/// Interpretations: interp:air-0018
#[test]
fn preparation_mode_guns_ordnance_credit_and_current_variants_stay_private() {
    let content = content();
    let (mut base, id) = single(&content, "cw.wellington_i");
    // This Air privacy fixture retains every Air observation and stream check;
    // unrelated ground inspection is bounded as in the designation fixtures.
    base.land.units.clear();
    base.logistics.unit_supply.clear();
    base.logistics.truck_pools.clear();
    base.logistics.dumps.clear();
    let mut variants = vec![base.clone()];
    for current in [
        CurrentPreparation::Unprepared,
        known(0, false, None),
        known(0, true, None),
        known(0, false, Some(CarriedOrdnance::Bombs { bomb_points: 1 })),
        known(1, false, None),
        CurrentPreparation::KnownFuel {
            mode: 0,
            fuel_available: true,
        },
        CurrentPreparation::KnownFuel {
            mode: 1,
            fuel_available: true,
        },
        CurrentPreparation::KnownFuel {
            mode: 0,
            fuel_available: false,
        },
    ] {
        let mut state = base.clone();
        fixture_current(&content, &mut state.air, &id, current);
        variants.push(state);
    }
    for credit in [
        InitialChoice::Unselected,
        InitialChoice::Committed,
        InitialChoice::Forfeited,
    ] {
        let mut state = base.clone();
        state
            .air
            .runtime
            .preparation
            .initial_choices
            .insert(id.clone(), credit.clone());
        fixture_current(
            &content,
            &mut state.air,
            &id,
            if credit == InitialChoice::Unselected {
                CurrentPreparation::InitialUnspecified
            } else {
                known(0, true, None)
            },
        );
        variants.push(state);
    }
    let targets = BTreeSet::from([id.0.clone()]);
    use cna_core::{
        dice::CampaignRng,
        engine::{Command, Game},
    };
    let rng = CampaignRng::from_seed([23; 32]).state();
    for state in variants {
        crate::testkit::assert_action_indistinguishable(
            &Cna::dev(),
            &content,
            &Game {
                state: base.clone(),
                rng: rng.clone(),
            },
            &Game {
                state: state.clone(),
                rng: rng.clone(),
            },
            &Command::Advance,
            Side::Axis,
        );
        assert_indistinguishable(&Cna::dev(), &content, &base, &state, Side::Axis);
        assert_eq!(
            visible_to(&Cna::dev(), &content, &base, Side::Axis, &targets),
            visible_to(&Cna::dev(), &content, &state, Side::Axis, &targets)
        );
    }
}

/// Cases: airlog:34.0, scen:59.32, scen:59.36
/// Interpretations: interp:air-0018
#[test]
fn gt1_closed_setup_and_both_legacy_coarse_bits_never_credit_or_rearm() {
    let content = content();
    let (base, id) = single(&content, "it.cr42");
    for bit in [false, true] {
        let mut state = base.clone();
        state.cursor.game_turn = 1;
        state.air.runtime.preparation = PreparationState::default();
        state.air.runtime.aircraft.get_mut(&id).unwrap().armed = bit;
        state.air.runtime.aircraft.get_mut(&id).unwrap().refitted = bit;
        refresh(&content, &mut state.air);
        let before = state.air.runtime.aircraft.clone();
        initialize(&content, &mut state).unwrap();
        assert_eq!(state.air.runtime.aircraft, before);
        assert!(state.air.runtime.preparation.initial_choices.is_empty());
        assert_eq!(
            state.air.runtime.preparation.current[&id],
            CurrentPreparation::UnknownLegacy
        );
        update(&content, &mut state.air, |_| Ok(())).unwrap();
        assert_eq!(state.air.runtime.aircraft, before);
        rejects_edit(&content, &state.air, |r| {
            r.aircraft.get_mut(&id).unwrap().armed = !bit;
            Ok(())
        });
        // Synthetic source-initial record: validates old-bit preservation only,
        // not a real setup producer or permission to credit migrated aircraft.
        state
            .air
            .runtime
            .preparation
            .initial_choices
            .insert(id.clone(), InitialChoice::Unselected);
        fixture_current(
            &content,
            &mut state.air,
            &id,
            CurrentPreparation::InitialUnspecified,
        );
        let before = bytes(&state.air);
        update(&content, &mut state.air, |_| Ok(())).unwrap();
        assert_eq!(before, bytes(&state.air));
        assert_eq!(state.air.runtime.aircraft[&id].armed, bit);
    }
}

/// Cases: airlog:34.0, airlog:34.14, airlog:38.47, airlog:41.39
/// Interpretations: interp:air-0018
#[test]
fn late_source_failure_preserves_known_state_and_old_serial_ids_cannot_appear() {
    let mut content = content();
    let (mut state, id) = single(&content, "it.z506b");
    let rating = content.units.aircraft["it.z506b"].modes[0]
        .bomb_capacity
        .unwrap();
    fixture_current(&content, &mut state.air, &id, known(0, true, None));
    // Existing carried source facts remain checked before any later edit;
    // this setup does not use the ordinary callback to create unpaid mines.
    fixture_current(
        &content,
        &mut state.air,
        &id,
        known(
            0,
            true,
            Some(CarriedOrdnance::Mines {
                source_bombload_rating: rating,
            }),
        ),
    );
    content.units.aircraft.get_mut("it.z506b").unwrap().modes[0].bomb_capacity = None;
    let before = bytes(&state.air);
    assert!(matches!(update(&content, &mut state.air, |r| {
        assert!(matches!(r.preparation.current[&id], CurrentPreparation::KnownLoad { .. }));
        r.preparation.current.insert(id.clone(), known(0, false, Some(CarriedOrdnance::Mines { source_bombload_rating: rating })));
        Ok(())
    }), Err(EngineError::Unsupported { case, detail }) if case == "airlog:34.14" && detail == "mines rating missing for it.z506b mode 0"));
    assert_eq!(before, bytes(&state.air));
    assert!(state.air.runtime.preparation.initialized);
    // Restore the source for the independent old-ID invariant below, so the
    // deliberately missing rating cannot mask that rejection.
    content.units.aircraft.get_mut("it.z506b").unwrap().modes[0].bomb_capacity = Some(rating);
    let old_unused = PlaneId(format!(
        "axis.aircraft-{}",
        state.air.runtime.plane_serial[&Side::Axis]
    ));
    assert_ne!(old_unused, id);
    let plane = state.air.runtime.aircraft[&id].clone();
    rejects_edit(&content, &state.air, |r| {
        r.aircraft.insert(old_unused.clone(), plane);
        r.preparation
            .current
            .insert(old_unused, CurrentPreparation::Unprepared);
        Ok(())
    });
}

/// Cases: airlog:34.14, airlog:38.4, airlog:45.0, airlog:45.17
#[test]
fn informational_armed_from_ordnance_never_supplies_combat_guns_or_dice() {
    use crate::air::combat::{CombatContext, CombatRole, Combatant, Engagement, resolve_wave};
    use cna_core::dice::CampaignRng;
    let content = content();
    let (mut state, id) = single(&content, "it.cr42as");
    let rating = content.units.aircraft["it.cr42as"].modes[0]
        .bomb_capacity
        .unwrap();
    fixture_current(
        &content,
        &mut state.air,
        &id,
        known(
            0,
            false,
            Some(CarriedOrdnance::Bombs {
                bomb_points: rating,
            }),
        ),
    );
    assert!(state.air.runtime.aircraft[&id].armed);
    let CurrentPreparation::KnownLoad { load } = &state.air.runtime.preparation.current[&id] else {
        unreachable!()
    };
    // Kernel-only supplied facts, not a flight/combat producer or mission claim.
    let context = CombatContext {
        role: CombatRole::Fighter,
        pilot_rating: 0,
        formation_bombers: 0,
        gun_ammunition: load.gun_ammunition,
        night: false,
    };
    let defender = Combatant::from_mode(
        id,
        &content.units.aircraft["it.cr42as"].modes[load.mode],
        context,
    )
    .unwrap();
    let attacker = Combatant::from_mode(
        PlaneId("private.opponent".into()),
        &content.units.aircraft["cw.hurricane_i"].modes[0],
        context,
    )
    .unwrap();
    let mut rng = CampaignRng::from_seed([31; 32]);
    let before = rng.state();
    let result = resolve_wave(
        &content,
        &mut rng,
        &[Engagement {
            defender,
            attackers: vec![attacker],
            return_fire_target: None,
        }],
    )
    .unwrap();
    assert!(result.shots.is_empty());
    assert_eq!(before, rng.state());
}

/// Cases: airlog:34.0, scen:59.32
/// Interpretations: interp:air-0018
#[test]
fn default_unknown_to_known_establishment_rejects_even_during_legacy_bootstrap() {
    let content = content();
    let (base, id) = single(&content, "it.cr42");
    for legacy in [false, true] {
        let mut state = base.clone();
        if legacy {
            state.air.runtime.preparation = PreparationState::default();
        }
        rejects_edit(&content, &state.air, |r| {
            assert!(r.preparation.initialized);
            assert_eq!(
                r.preparation.current[&id],
                CurrentPreparation::UnknownLegacy
            );
            r.preparation
                .current
                .insert(id.clone(), known(0, true, None));
            Ok(())
        });
        assert_eq!(state.air.runtime.preparation.initialized, !legacy);
        if legacy {
            assert!(state.air.runtime.preparation.current.is_empty());
        }
    }
}

/// Cases: airlog:34.0, airlog:34.8, scen:59.32
#[test]
fn default_unprepared_to_known_establishment_rejects_without_record_or_mirror_change() {
    let content = content();
    let (mut state, id) = single(&content, "it.cr42");
    fixture_current(
        &content,
        &mut state.air,
        &id,
        CurrentPreparation::Unprepared,
    );
    rejects_edit(&content, &state.air, |r| {
        r.preparation.current.insert(id, known(0, true, None));
        Ok(())
    });
}

/// Cases: airlog:34.0, airlog:34.8, scen:59.32
#[test]
fn default_new_id_known_establishment_rejects_and_rolls_back_zero_or_populated_migration() {
    let content = content();
    let (base, id) = single(&content, "it.cr42");
    let plane = base.air.runtime.aircraft[&id].clone();
    for empty in [false, true] {
        for legacy in [false, true] {
            let mut state = base.clone();
            if empty {
                state.air.runtime.aircraft.clear();
                state.air.runtime.preparation.current.clear();
                refresh(&content, &mut state.air);
            }
            if legacy {
                state.air.runtime.preparation = PreparationState::default();
            }
            rejects_edit(&content, &state.air, |r| {
                let allocated = r.insert_aircraft(plane.clone())?;
                r.preparation
                    .current
                    .insert(allocated, known(0, true, None));
                Ok(())
            });
            assert_eq!(state.air.runtime.preparation.initialized, !legacy);
            if legacy {
                assert!(state.air.runtime.preparation.current.is_empty());
            }
        }
    }
}

/// Cases: airlog:38.21, airlog:38.24, airlog:38.25, land:3.6
#[test]
fn generic_known_fuel_and_known_load_updates_cannot_buy_fuel_or_change_fueled_mode() {
    let content = content();
    let (base, id) = single(&content, "cw.wellington_i");
    for partial in [false, true] {
        for available in [false, true] {
            let mut air = base.air.clone();
            let current = if partial {
                CurrentPreparation::KnownFuel {
                    mode: 0,
                    fuel_available: available,
                }
            } else {
                CurrentPreparation::KnownLoad {
                    load: KnownLoad {
                        mode: 0,
                        fuel_available: available,
                        gun_ammunition: false,
                        ordnance: None,
                    },
                }
            };
            fixture_current(&content, &mut air, &id, current);
            if !available {
                rejects_edit(&content, &air, |runtime| {
                    match runtime.preparation.current.get_mut(&id).unwrap() {
                        CurrentPreparation::KnownFuel { fuel_available, .. } => {
                            *fuel_available = true
                        }
                        CurrentPreparation::KnownLoad { load } => load.fuel_available = true,
                        _ => unreachable!(),
                    }
                    runtime.aircraft.get_mut(&id).unwrap().fuelled = true;
                    Ok(())
                });
            } else {
                rejects_edit(&content, &air, |runtime| {
                    match runtime.preparation.current.get_mut(&id).unwrap() {
                        CurrentPreparation::KnownFuel { mode, .. } => *mode = 1,
                        CurrentPreparation::KnownLoad { load } => load.mode = 1,
                        _ => unreachable!(),
                    }
                    Ok(())
                });
            }
            let mut broken = air.clone();
            broken.runtime.aircraft.get_mut(&id).unwrap().fuelled = !available;
            let before = bytes(&broken);
            assert!(matches!(
                check(&content, &broken),
                Err(EngineError::Invariant { .. })
            ));
            assert_eq!(bytes(&broken), before);
        }
    }
    let mut air = base.air.clone();
    air.runtime.aircraft.get_mut(&id).unwrap().fuelled = false;
    refresh(&content, &mut air);
    rejects_edit(&content, &air, |runtime| {
        runtime.aircraft.get_mut(&id).unwrap().fuelled = true;
        Ok(())
    });
    rejects_edit(&content, &air, |runtime| {
        runtime.preparation.current.insert(
            id.clone(),
            CurrentPreparation::KnownFuel {
                mode: 0,
                fuel_available: true,
            },
        );
        runtime.aircraft.get_mut(&id).unwrap().fuelled = true;
        Ok(())
    });
}
