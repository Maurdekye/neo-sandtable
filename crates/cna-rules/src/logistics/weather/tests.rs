//! Weather integration tests on Graziani, with deterministic campaign dice.
use super::*;
use crate::state::{Location, UnitSupply, WellState};
use cna_core::dice::CampaignRng;
use cna_core::quantity::FuelTenths;
use cna_core::visibility::Audience;
use std::sync::OnceLock;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn seed_for(kind: WeatherKind) -> u8 {
    (0..=255)
        .find(|b| {
            let roll = CampaignRng::from_seed([*b; 32]).two_dice_reading();
            content().tables.land.weather.result(1, roll) == Some(kind)
        })
        .unwrap()
}
fn run(state: &mut State, seed: u8) -> (CampaignRng, Vec<EngineEvent>) {
    let mut rng = CampaignRng::from_seed([seed; 32]);
    let mut events = Vec::new();
    determine(
        content(),
        state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    (rng, events)
}

/// Cases: land:29.1, land:29.7
/// Interpretations: interp:land-0019
#[test]
fn weather_and_location_follow_tables_with_every_roll_recorded() {
    for kind in [
        WeatherKind::Normal,
        WeatherKind::Hot,
        WeatherKind::Sandstorm,
        WeatherKind::Rainstorm,
    ] {
        let seed = seed_for(kind);
        let mut expected = CampaignRng::from_seed([seed; 32]);
        let reading = expected.two_dice_reading();
        let storm = matches!(kind, WeatherKind::Sandstorm | WeatherKind::Rainstorm);
        let sections = if storm {
            content()
                .tables
                .land
                .foul_weather_location
                .sections(expected.d6())
                .to_vec()
        } else {
            Vec::new()
        };
        let mut state = State::new(content()).unwrap();
        let (rng, events) = run(&mut state, seed);
        assert_eq!(
            state.turn.weather,
            Some(WeatherState {
                kind,
                storm_sections: sections
            })
        );
        assert_eq!(rng.state(), expected.state());
        assert!(events.iter().all(|e| e.audience == Audience::Public));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.event, GameEvent::DiceRolled { .. }))
                .count(),
            if storm { 2 } else { 1 }
        );
        assert!(
            matches!(&events[0].event, GameEvent::DiceRolled { dice, reading: r, rule, .. }
            if dice == &vec![reading.tens.value(), reading.units.value()] && *r == Some(reading.value()) && rule.as_deref() == Some("land:29.1"))
        );
    }
}

/// Cases: land:29.34, airlog:49.3, airlog:52.44
#[test]
fn hot_weather_loses_cargo_and_dump_stocks_but_not_tanks_or_offmap_stocks() {
    let mut state = State::new(content()).unwrap();
    let id = state
        .land
        .units
        .values()
        .find(|u| u.location.hex().is_some())
        .unwrap()
        .id
        .clone();
    state.logistics.unit_supply.insert(
        id.clone(),
        UnitSupply {
            tank_fuel: FuelTenths::new(31),
            carried: Supplies {
                fuel: 99,
                water: 200,
                ammo: 7,
                stores: 8,
            },
            ..UnitSupply::default()
        },
    );
    let dump = state
        .logistics
        .dumps
        .values_mut()
        .find(|d| matches!(d.location, DumpLocation::Hex { .. }))
        .unwrap();
    let dump_id = dump.id.clone();
    dump.supplies = Supplies {
        fuel: 99,
        water: 39,
        ammo: 7,
        stores: 8,
    };
    let mut offmap = dump.clone();
    offmap.id = "offmap_test".into();
    offmap.location = DumpLocation::OffMap {
        id: "box_tripoli".into(),
    };
    state
        .logistics
        .dumps
        .insert(offmap.id.clone(), offmap.clone());
    let (_, events) = run(&mut state, seed_for(WeatherKind::Hot));
    let holdings = state.logistics.unit_supply[&id].clone();
    assert_eq!(holdings.tank_fuel.get(), 31);
    assert_eq!(
        holdings.carried,
        Supplies {
            fuel: 95,
            water: 190,
            ammo: 7,
            stores: 8
        }
    );
    assert_eq!(
        state.logistics.dumps[&dump_id].supplies,
        Supplies {
            fuel: 95,
            water: 38,
            ammo: 7,
            stores: 8
        }
    );
    assert_eq!(state.logistics.dumps["offmap_test"], offmap);
    assert!(
        !events
            .iter()
            .any(|e| serde_json::to_string(e).unwrap().contains(&dump_id))
    );
    state.land.units.get_mut(&id).unwrap().location = Location::OffMap {
        id: "box_tripoli".into(),
    };
    run(&mut state, seed_for(WeatherKind::Hot));
    assert_eq!(state.logistics.unit_supply[&id].carried, holdings.carried);
}

/// Cases: land:29.41, land:29.46, land:29.51, land:29.52
#[test]
fn storm_scope_excludes_sea_and_delta_only_for_sandstorms() {
    let mut state = State::new(content()).unwrap();
    for kind in [WeatherKind::Sandstorm, WeatherKind::Rainstorm] {
        state.turn.weather = Some(WeatherState {
            kind,
            storm_sections: vec![MapSection::C, MapSection::E],
        });
        assert_eq!(
            at_hex(content(), &state, &"C4023".into()).unwrap(),
            if kind == WeatherKind::Sandstorm {
                WeatherKind::Normal
            } else {
                WeatherKind::Rainstorm
            }
        );
        assert_eq!(at_hex(content(), &state, &"C4020".into()).unwrap(), kind);
        assert_eq!(
            at_hex(content(), &state, &"A2802".into()).unwrap(),
            WeatherKind::Normal
        );
        // Delta terrain is not yet transcribed in the real map. Exercise that
        // branch with a classified copy of a real record rather than guessing a hex.
        let mut delta = content().map.get(&"C4020".into()).unwrap().clone();
        delta.section = 'E';
        delta.terrain = Some("delta".into());
        assert_eq!(
            local_weather_for_record(state.turn.weather.as_ref().unwrap(), &delta).unwrap(),
            if kind == WeatherKind::Sandstorm {
                WeatherKind::Normal
            } else {
                WeatherKind::Rainstorm
            }
        );
    }
}

/// Cases: land:29.53
#[test]
fn rain_refills_only_wells_in_affected_sections() {
    let mut state = State::new(content()).unwrap();
    let seed = seed_for(WeatherKind::Rainstorm);
    let mut expected = CampaignRng::from_seed([seed; 32]);
    expected.two_dice_reading();
    let sections = content()
        .tables
        .land
        .foul_weather_location
        .sections(expected.d6());
    let wet = content()
        .map
        .iter()
        .find(|r| sections.iter().any(|s| section_letter(*s) == r.section))
        .unwrap()
        .id
        .clone();
    let dry = content()
        .map
        .iter()
        .find(|r| !sections.iter().any(|s| section_letter(*s) == r.section))
        .unwrap()
        .id
        .clone();
    state
        .logistics
        .wells
        .insert(wet.clone(), WellState { depleted: true });
    state
        .logistics
        .wells
        .insert(dry.clone(), WellState { depleted: true });
    run(&mut state, seed);
    assert!(!state.logistics.wells[&wet].depleted);
    assert!(state.logistics.wells[&dry].depleted);
}

/// Cases: land:29.1, land:29.6
#[test]
fn unknown_weather_turn_rejects_without_mutation_or_roll() {
    let mut state = State::new(content()).unwrap();
    state.cursor.game_turn = 111;
    let before = serde_json::to_value(&state).unwrap();
    let mut rng = CampaignRng::from_seed([8; 32]);
    let rng_before = rng.state();
    let mut events = Vec::new();
    assert!(
        matches!(determine(content(), &mut state, &mut Cx { rng: &mut rng, events: &mut events }), Err(EngineError::Unsupported { case, .. }) if case == "land:29.6")
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    assert_eq!(rng.state(), rng_before);
    assert!(events.is_empty());
}
