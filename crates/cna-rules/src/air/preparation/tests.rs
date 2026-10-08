use super::*;
use serde_json::json;

fn content() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}

fn plane(aircraft: &str) -> AircraftState {
    AircraftState {
        aircraft: aircraft.into(),
        force: "axis".into(),
        squadron: None,
        facility: None,
        refitted: true,
        fuelled: true,
        armed: true,
    }
}

fn load(mode: usize, ordnance: Option<CarriedOrdnance>) -> KnownLoad {
    KnownLoad {
        mode,
        fuel_available: true,
        gun_ammunition: false,
        ordnance,
    }
}

/// Cases: scen:59.32, scen:59.36
/// Interpretations: interp:air-0018
#[test]
fn omitted_preparation_is_legacy_unknown_without_any_initial_credit() {
    for value in [json!({}), json!({"current": {}, "initial_choices": {}})] {
        let state: PreparationState = serde_json::from_value(value).unwrap();
        assert!(!state.initialized);
        assert!(state.initial_choices.is_empty());
        assert!(state.current.is_empty());
        assert_eq!(state, PreparationState::default());
    }
    let state = PreparationState {
        initialized: true,
        initial_choices: BTreeMap::from([(
            PlaneId("axis.aircraft-1".into()),
            InitialChoice::Forfeited,
        )]),
        current: BTreeMap::new(),
    };
    let restored: PreparationState =
        serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(state, restored);
    // Serde preserves tombstones; strict canonical live/dead validation is
    // inventory-owned, not fabricated by deserialization.
}

/// Cases: airlog:34.14, airlog:38.47
#[test]
fn serialized_load_requires_facts_and_one_unambiguous_ordnance_kind() {
    for value in [
        json!({"mode":0}),
        json!({"mode":0,"fuel_available":true,"gun_ammunition":false}),
    ] {
        assert!(serde_json::from_value::<KnownLoad>(value).is_err());
    }
    assert!(serde_json::from_value::<CurrentPreparation>(json!({"state":"known_load"})).is_err());
    assert!(
        serde_json::from_value::<CarriedOrdnance>(json!({"kind":"mines","rating_points":8}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<CarriedOrdnance>(
            json!({"kind":"bombs","bomb_points":8,"rating_points":8})
        )
        .is_err()
    );
    assert!(serde_json::from_value::<PreparationState>(json!({"free_credit":true})).is_err());
    for current in [
        CurrentPreparation::UnknownLegacy,
        CurrentPreparation::InitialUnspecified,
        CurrentPreparation::Unprepared,
        CurrentPreparation::KnownLoad {
            load: load(
                0,
                Some(CarriedOrdnance::Mines {
                    source_bombload_rating: 11,
                }),
            ),
        },
    ] {
        let value = serde_json::to_value(&current).unwrap();
        assert_eq!(
            serde_json::from_value::<CurrentPreparation>(value).unwrap(),
            current
        );
    }
}

/// Cases: airlog:34.14, airlog:38.47, airlog:41.39
#[test]
fn mines_preserve_exact_selected_source_rating_without_a_quantity_or_countdown() {
    let content = content();
    let plane = plane("cw.wellington_i");
    assert_eq!(
        content.units.aircraft[&plane.aircraft].modes[0].bomb_capacity,
        Some(5)
    );
    assert_eq!(
        content.units.aircraft[&plane.aircraft].modes[1].bomb_capacity,
        Some(23)
    );
    for (mode, rating) in [(0, 5), (1, 23)] {
        let known = load(
            mode,
            Some(CarriedOrdnance::Mines {
                source_bombload_rating: rating,
            }),
        );
        let before = known.clone();
        validate_load(&content, &plane, &known).unwrap();
        assert_eq!(known, before);
        for recorded in [-1, 0, rating - 1, rating + 1] {
            assert!(matches!(
                validate_load(
                    &content,
                    &plane,
                    &load(
                        mode,
                        Some(CarriedOrdnance::Mines {
                            source_bombload_rating: recorded
                        })
                    )
                ),
                Err(EngineError::Invariant { .. })
            ));
        }
    }
}

/// Cases: airlog:34.14, airlog:38.47
#[test]
fn payload_source_gaps_and_malformed_ratings_keep_exact_error_kind_and_mode() {
    let mut content = content();
    let plane = plane("it.z506b");
    for ordnance in [
        CarriedOrdnance::Bombs { bomb_points: 11 },
        CarriedOrdnance::Mines {
            source_bombload_rating: 11,
        },
        CarriedOrdnance::Torpedoes { rating_points: 8 },
    ] {
        validate_load(&content, &plane, &load(0, Some(ordnance))).unwrap();
    }
    for ordnance in [
        CarriedOrdnance::Bombs { bomb_points: 12 },
        CarriedOrdnance::Torpedoes { rating_points: 9 },
        CarriedOrdnance::Torpedoes { rating_points: -1 },
    ] {
        assert!(matches!(
            validate_load(&content, &plane, &load(0, Some(ordnance))),
            Err(EngineError::Invariant { .. })
        ));
    }
    for (kind, ordnance) in [
        ("bombs", CarriedOrdnance::Bombs { bomb_points: 0 }),
        (
            "mines",
            CarriedOrdnance::Mines {
                source_bombload_rating: 0,
            },
        ),
        ("torpedoes", CarriedOrdnance::Torpedoes { rating_points: 0 }),
    ] {
        let mode = &mut content
            .units
            .aircraft
            .get_mut(&plane.aircraft)
            .unwrap()
            .modes[0];
        mode.bomb_capacity = None;
        mode.torpedo_capacity = None;
        assert!(
            matches!(validate_load(&content,&plane,&load(0,Some(ordnance))),Err(EngineError::Unsupported {case,detail}) if case=="airlog:34.14" && detail==format!("{kind} rating missing for it.z506b mode 0"))
        );
    }
    let mode = &mut content
        .units
        .aircraft
        .get_mut(&plane.aircraft)
        .unwrap()
        .modes[0];
    mode.bomb_capacity = Some(-1);
    assert!(matches!(
        validate_load(
            &content,
            &plane,
            &load(0, Some(CarriedOrdnance::Bombs { bomb_points: 0 }))
        ),
        Err(EngineError::Invariant { .. })
    ));
    assert!(matches!(
        validate_load(&content, &plane, &load(usize::MAX, None)),
        Err(EngineError::Invariant { .. })
    ));
    assert!(matches!(
        validate_load(&content, &self::plane("unknown.source"), &load(0, None)),
        Err(EngineError::Invariant { .. })
    ));
    // No carried choice needs no missing ordnance rating and invents nothing.
    validate_load(&content, &plane, &load(0, None)).unwrap();
}

/// Cases: airlog:38.4, scen:59.32
/// Interpretations: interp:air-0018
#[test]
fn armed_is_only_an_informational_projection_and_unknown_never_supplies_ammo() {
    for coarse in [false, true] {
        assert_eq!(
            armed_projection(&CurrentPreparation::UnknownLegacy, coarse),
            coarse
        );
        assert_eq!(
            armed_projection(&CurrentPreparation::InitialUnspecified, coarse),
            coarse
        );
        assert!(!armed_projection(&CurrentPreparation::Unprepared, coarse));
        for guns in [false, true] {
            for (ordnance, positive) in [
                (None, false),
                (Some(CarriedOrdnance::Bombs { bomb_points: 0 }), false),
                (Some(CarriedOrdnance::Bombs { bomb_points: 5 }), true),
                (
                    Some(CarriedOrdnance::Mines {
                        source_bombload_rating: 0,
                    }),
                    false,
                ),
                (
                    Some(CarriedOrdnance::Mines {
                        source_bombload_rating: 11,
                    }),
                    true,
                ),
                (Some(CarriedOrdnance::Torpedoes { rating_points: 0 }), false),
                (Some(CarriedOrdnance::Torpedoes { rating_points: 8 }), true),
            ] {
                let mut known = load(0, ordnance);
                known.gun_ammunition = guns;
                assert_eq!(
                    armed_projection(&CurrentPreparation::KnownLoad { load: known }, coarse),
                    guns || positive
                );
            }
        }
    }
    // This pure calculator accepts already-validated records; it never builds
    // Combatant gun ammunition or claims source compatibility/mission rights.
}
