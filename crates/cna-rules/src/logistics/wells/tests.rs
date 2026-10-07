use super::*;
use crate::state::{UnitSupply, WeatherState, WellState};
use cna_content::{places::Place, units::Trucks};
use cna_core::{dice::CampaignRng, quantity::FuelTenths};
use cna_protocol::Role;
use cna_tables::land::weather::{MapSection, WeatherKind};

fn setup(kind: Option<&str>) -> (CnaContent, State, UnitId, HexId) {
    let mut content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut state = State::new(&content).unwrap();
    let id: UnitId = "it.1_libyan_div.viii_libyan_bn".into();
    let hex = state.land.units[&id].location.hex().unwrap().clone();
    for (key, u) in &mut state.land.units {
        if key != &id {
            u.location = Location::NotArrived;
        }
    }
    state.land.units.get_mut(&id).unwrap().trucks = Trucks::default();
    state.turn.weather = Some(WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    state.cursor.op_stage = Some(1);
    if let Some(kind) = kind {
        // An explicit hand-built finite-well fixture on a real grid hex; not a map transcription.
        content.places.places.insert(
            "fixture-well".into(),
            Place {
                id: "fixture-well".into(),
                name: "Test source".into(),
                hex_id: hex.clone(),
                kind: kind.into(),
                place_group: None,
                src: vec!["airlog:52.11".into()],
                note: None,
                review_batch: "test-fixture".into(),
            },
        );
    }
    (content, state, id, hex)
}
fn run_draw(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    amount: i32,
    packing: &CargoPacking,
    seed: u8,
) -> (i32, CampaignRng, Vec<EngineEvent>) {
    let mut rng = CampaignRng::from_seed([seed; 32]);
    let mut events = Vec::new();
    let result = draw(
        content,
        state,
        id,
        amount,
        packing,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    (result, rng, events)
}
fn seed_for(value: u8) -> u8 {
    (0..=255)
        .find(|s| CampaignRng::from_seed([*s; 32]).d6().value() == value)
        .unwrap()
}

/// Cases: airlog:52.11, airlog:52.13, airlog:52.41, airlog:52.42, airlog:54.2
#[test]
fn verified_city_draws_exact_carrying_limit_without_a_die_and_pays_activity_water() {
    let (content, mut state, id, _) = setup(None);
    state.land.units.get_mut(&id).unwrap().location = Location::Hex {
        hex: "E1730".into(),
    };
    state.land.units.get_mut(&id).unwrap().trucks = Trucks {
        light: 1,
        ..Trucks::default()
    };
    state.logistics.unit_supply.insert(
        id.clone(),
        UnitSupply {
            tank_fuel: FuelTenths::new(10),
            ..UnitSupply::default()
        },
    );
    let packing = CargoPacking {
        light: Supplies {
            water: 40,
            ..Supplies::default()
        },
        ..CargoPacking::default()
    };
    let before = serde_json::to_value(&state).unwrap();
    let mut rng = CampaignRng::from_seed([7; 32]);
    let expected_rng = rng.clone();
    let mut events = Vec::new();
    assert!(
        draw(
            &content,
            &mut state,
            &id,
            43,
            &packing,
            &mut Cx {
                rng: &mut rng,
                events: &mut events
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    assert_eq!(rng.state(), expected_rng.state());
    assert!(events.is_empty());
    let (result, _, events) = run_draw(&content, &mut state, &id, 42, &packing, 7);
    assert_eq!(result, 42);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.event, GameEvent::DiceRolled { .. }))
    );
    assert_eq!(state.land.units[&id].cp_spent_quarters, 4);
    assert_eq!(state.logistics.unit_supply[&id].tank_fuel.get(), 10);
    allocate(
        &content,
        &mut state,
        &id,
        &Allocation {
            infantry: 1,
            activity: 1,
            pasta: false,
            cargo: 40,
            packing,
        },
    )
    .unwrap();
    assert_eq!(state.logistics.unit_supply[&id].carried.water, 40);
    assert_eq!(state.logistics.unit_supply[&id].activity_water.get(), 0);
    assert_eq!(
        state.logistics.rations[&id].activity_used_stage,
        Some(water::WaterStage::current(&state))
    );
    assert_eq!(
        water::requirements(&content, &state, &id).unwrap().infantry,
        0
    );
    // A well drink does not prevent a later distribution assessment or reset the drink to zero.
    water::issue_unit(&content, &mut state, &id, 0, 0, false, &[]).unwrap();
    water::finalize(&content, &mut state, Side::Axis, false).unwrap();
    assert_eq!(
        state.logistics.rations[&id].consecutive_short_water_stages,
        0
    );
    assert!(!candidates(&content, &state, Side::Axis).contains(&id));
}

/// Cases: airlog:52.13, airlog:52.7, airlog:52.14
#[test]
fn every_finite_well_face_follows_the_bound_chart_and_second_roll_depletes_only_on_one() {
    for source in [Source::Village, Source::Bir] {
        for face in 1..=6 {
            let (content, mut state, id, hex) = setup(Some(if source == Source::Village {
                "village"
            } else {
                "bir"
            }));
            state.land.units.get_mut(&id).unwrap().trucks = Trucks {
                heavy: 1,
                ..Trucks::default()
            };
            let packing = CargoPacking {
                heavy: Supplies {
                    water: 98,
                    ..Supplies::default()
                },
                ..CargoPacking::default()
            };
            let seed = seed_for(face);
            let mut rng = CampaignRng::from_seed([seed; 32]);
            let die = rng.d6();
            let expected = content.tables.airlog.water_availability.draw(
                if source == Source::Village {
                    WellSource::Town
                } else {
                    WellSource::Bir
                },
                die,
            );
            let depleted = expected.depletion_check
                && rng.d6() == content.tables.airlog.water_availability.depleted_on();
            let (water, _, events) = run_draw(&content, &mut state, &id, 100, &packing, seed);
            assert_eq!(water, 100.min(expected.water.get()));
            assert_eq!(state.logistics.wells[&hex].depleted, depleted);
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(e.event, GameEvent::DiceRolled { .. }))
                    .count(),
                1 + usize::from(expected.depletion_check)
            );
            assert!(
                events
                    .iter()
                    .all(|e| e.audience == Audience::Side(Side::Axis))
            );
        }
    }
}

/// Cases: airlog:52.14, airlog:52.16, land:3.6
#[test]
fn an_opposing_attempt_charges_cp_then_reveals_only_condition_markers() {
    let (content, mut state, id, hex) = setup(Some("bir"));
    let mut known = BTreeSet::new();
    known.insert(Side::Commonwealth);
    state.logistics.wells.insert(
        hex.clone(),
        WellState {
            depleted: true,
            poisoned: true,
            depleted_known: known.clone(),
            poisoned_known: known,
            ..WellState::default()
        },
    );
    let axis = Perspective::Side(Side::Axis);
    let cw = Perspective::Side(Side::Commonwealth);
    assert_eq!(condition(&state, &hex, axis), json!({}));
    assert!(
        !crate::view::view(&content, &state, axis)
            .markers
            .iter()
            .any(|m| m.kind.starts_with("well_"))
    );
    let (water, _, events) = run_draw(&content, &mut state, &id, 1, &CargoPacking::default(), 7);
    assert_eq!(water, 0);
    assert_eq!(state.land.units[&id].cp_spent_quarters, 4);
    assert_eq!(
        condition(&state, &hex, axis),
        json!({"depleted":true,"poisoned":true})
    );
    for viewer in [axis, cw] {
        let markers = crate::view::view(&content, &state, viewer).markers;
        assert_eq!(
            markers
                .iter()
                .filter(|m| m.kind.starts_with("well_"))
                .count(),
            2
        );
        let detail = crate::view::inspect(&content, &state, viewer, hex.as_str(), false).unwrap();
        assert_eq!(detail["well"], json!({"depleted":true,"poisoned":true}));
    }
    let public: Vec<_> = events
        .iter()
        .filter(|e| e.audience == Audience::Public)
        .collect();
    assert_eq!(public.len(), 2);
    assert!(public.iter().all(|e|matches!(&e.event,GameEvent::Note {text} if !text.contains("water")&&!text.contains(&id.to_string()))));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.event, GameEvent::DiceRolled { .. }))
    );
    assert!(
        crate::view::observe(&content, &state, cw)["logistics"]["drawn_water"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}

/// Cases: airlog:52.13, airlog:54.2, land:3.6
#[test]
fn rejected_allocation_keeps_the_same_checkpointed_roll_and_cannot_overload_cargo() {
    let (content, mut state, id, _) = setup(Some("village"));
    state.land.units.get_mut(&id).unwrap().trucks = Trucks {
        light: 1,
        ..Trucks::default()
    };
    let packing = CargoPacking {
        light: Supplies {
            water: 40,
            ..Supplies::default()
        },
        ..CargoPacking::default()
    };
    run_draw(&content, &mut state, &id, 42, &packing, seed_for(6));
    let mut restored: State =
        serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    let before = serde_json::to_value(&restored).unwrap();
    assert!(
        allocate(
            &content,
            &mut restored,
            &id,
            &Allocation {
                infantry: 1,
                activity: 1,
                pasta: false,
                cargo: 41,
                packing: CargoPacking {
                    light: Supplies {
                        water: 41,
                        ..Supplies::default()
                    },
                    ..CargoPacking::default()
                }
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&restored).unwrap(), before);
    let actual = restored.logistics.drawn_water[&id].points;
    let cargo = (actual - 2).max(0);
    allocate(
        &content,
        &mut restored,
        &id,
        &Allocation {
            infantry: 1,
            activity: 1,
            pasta: false,
            cargo,
            packing: CargoPacking {
                light: Supplies {
                    water: cargo,
                    ..Supplies::default()
                },
                ..CargoPacking::default()
            },
        },
    )
    .unwrap();
    assert!(!restored.logistics.drawn_water.contains_key(&id));
}

/// Cases: airlog:52.16, airlog:52.17, airlog:52.8
/// Interpretations: interp:airlog-0009
#[test]
fn poisoning_failure_is_private_and_retry_limited_but_sweetening_cannot_exceed_cpa() {
    let (content, mut state, id, hex) = setup(Some("bir"));
    let mut rng = CampaignRng::from_seed([seed_for(2); 32]);
    let mut events = Vec::new();
    attempt(
        &content,
        &mut state,
        &id,
        false,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    let before = serde_json::to_value(&state).unwrap();
    let rng_before = rng.clone();
    assert!(
        attempt(
            &content,
            &mut state,
            &id,
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut events
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    assert_eq!(rng.state(), rng_before.state());
    assert!(
        events
            .iter()
            .all(|e| e.audience == Audience::Side(Side::Axis))
    );
    assert_eq!(
        condition(&state, &hex, Perspective::Side(Side::Commonwealth)),
        json!({})
    );
    state.cursor.op_stage = Some(2);
    let mut rng = CampaignRng::from_seed([seed_for(1); 32]);
    attempt(
        &content,
        &mut state,
        &id,
        false,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert!(state.logistics.wells[&hex].poisoned);
    let cpa = crate::land::formation::individual_allowance(&content, &state, &id)
        .unwrap()
        .cpa;
    state.land.units.get_mut(&id).unwrap().cp_spent_quarters = cpa * 4 - 19;
    let before = serde_json::to_value(&state).unwrap();
    assert!(
        attempt(
            &content,
            &mut state,
            &id,
            true,
            &mut Cx {
                rng: &mut rng,
                events: &mut events
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    state.land.units.get_mut(&id).unwrap().cp_spent_quarters = cpa * 4 - 20;
    let mut rng = CampaignRng::from_seed([seed_for(1); 32]);
    attempt(
        &content,
        &mut state,
        &id,
        true,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert!(!state.logistics.wells[&hex].poisoned);
    assert_eq!(state.land.units[&id].cp_spent_quarters, cpa * 4);
}

/// Cases: airlog:52.11, airlog:52.21, airlog:52.22, airlog:52.23, airlog:52.25, airlog:52.3
#[test]
fn source_membership_is_exact_and_pipeline_damage_or_cycles_break_the_chain() {
    let (content, mut state, id, unknown) = setup(None);
    assert!(
        source_at(
            &content,
            &state,
            Side::Axis,
            &Location::Hex { hex: unknown }
        )
        .is_err()
    );
    for id in ["box_tripoli", "box_tripolitania", "box_gabes", "box_tunis"] {
        assert_eq!(
            source_at(
                &content,
                &state,
                Side::Axis,
                &Location::OffMap { id: id.into() }
            )
            .unwrap(),
            Source::AxisBox
        );
    }
    assert!(
        source_at(
            &content,
            &state,
            Side::Axis,
            &Location::OffMap {
                id: "box_crete".into()
            }
        )
        .is_err()
    );
    let root: HexId = "E1730".into();
    let first = content
        .map
        .neighbors(&root)
        .into_iter()
        .find(|h| place_source(&content, &h.id).is_none())
        .unwrap()
        .id
        .clone();
    let second = content
        .map
        .neighbors(&first)
        .into_iter()
        .find(|h| place_source(&content, &h.id).is_none() && h.id != root)
        .unwrap()
        .id
        .clone();
    state.logistics.pipelines.insert(
        first.clone(),
        PipelineHex {
            upstream: root,
            destroyed: false,
        },
    );
    state.logistics.pipelines.insert(
        second.clone(),
        PipelineHex {
            upstream: first.clone(),
            destroyed: false,
        },
    );
    assert!(connected_pipeline(&content, &state, Side::Axis, &second));
    state.logistics.pipelines.get_mut(&first).unwrap().destroyed = true;
    assert!(!connected_pipeline(&content, &state, Side::Axis, &second));
    state.logistics.pipelines.get_mut(&first).unwrap().destroyed = false;
    state.logistics.pipelines.get_mut(&first).unwrap().upstream = second.clone();
    assert!(!connected_pipeline(&content, &state, Side::Axis, &second));
    state.logistics.pipelines.clear();
    state.logistics.operating_rail_water.insert(second.clone());
    assert!(connected_pipeline(
        &content,
        &state,
        Side::Commonwealth,
        &second
    ));
    assert!(!connected_pipeline(&content, &state, Side::Axis, &second));
    state.land.units.get_mut(&id).unwrap().location = Location::Hex { hex: second };
}

/// Cases: airlog:52.15, land:29.53
#[test]
fn rain_clears_the_public_depletion_marker_without_clearing_poison() {
    let (content, mut state, _id, hex) = setup(Some("bir"));
    state.logistics.wells.insert(
        hex.clone(),
        WellState {
            depleted: true,
            poisoned: true,
            depleted_revealed: true,
            poisoned_revealed: true,
            ..WellState::default()
        },
    );
    let section = content.map.get(&hex).unwrap().section;
    let seed = (0..=255)
        .find(|s| {
            let mut rng = CampaignRng::from_seed([*s; 32]);
            let kind = content
                .tables
                .land
                .weather
                .result(1, rng.two_dice_reading());
            kind == Some(WeatherKind::Rainstorm)
                && content
                    .tables
                    .land
                    .foul_weather_location
                    .sections(rng.d6())
                    .iter()
                    .any(|x| match x {
                        MapSection::A => section == 'A',
                        MapSection::B => section == 'B',
                        MapSection::C => section == 'C',
                        MapSection::D => section == 'D',
                        MapSection::E => section == 'E',
                    })
        })
        .unwrap();
    let mut rng = CampaignRng::from_seed([seed; 32]);
    let mut events = Vec::new();
    super::super::weather::determine(
        &content,
        &mut state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert!(!state.logistics.wells[&hex].depleted_revealed);
    assert!(state.logistics.wells[&hex].poisoned);
    assert_eq!(
        condition(&state, &hex, Perspective::Side(Side::Axis)),
        json!({"poisoned":true})
    );
}

/// Cases: airlog:52.13, airlog:52.41
#[test]
fn dispatcher_routes_draw_and_allocation_after_the_well_menu() {
    let (content, mut state, id, _) = setup(None);
    state.land.units.get_mut(&id).unwrap().location = Location::Hex {
        hex: "E1730".into(),
    };
    let seat = SeatId::new(Side::Axis, Role::Logistics);
    let mut rng = CampaignRng::from_seed([7; 32]);
    let mut events = Vec::new();
    let mut cx = Cx {
        rng: &mut rng,
        events: &mut events,
    };
    open_request(&content, &mut state, &id, seat, &mut cx).unwrap();
    let pending = state.decisions.pending.pop().unwrap();
    assert!(pending.kind.starts_with(REQUEST_PREFIX));
    crate::Cna::dev()
        .respond_to(
            &content,
            &mut state,
            &pending,
            &serde_json::json!({"requested":1,"packing":CargoPacking::default()}),
            &mut cx,
        )
        .unwrap();
    let pending = state.decisions.pending.pop().unwrap();
    assert!(pending.kind.starts_with(ALLOCATE_PREFIX));
    crate::Cna::dev()
        .respond_to(
            &content,
            &mut state,
            &pending,
            &serde_json::json!({"infantry":1,"activity":0,"pasta":false,"cargo":0,
            "packing":CargoPacking::default()}),
            &mut cx,
        )
        .unwrap();
    assert!(!state.logistics.drawn_water.contains_key(&id));
    assert_eq!(state.logistics.rations[&id].infantry_water_received, 1);
    assert_eq!(state.land.units[&id].cp_spent_quarters, 4);
}
