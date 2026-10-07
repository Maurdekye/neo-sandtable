use super::super::water;
use super::*;
use crate::state::WeatherState;
use cna_core::dice::CampaignRng;
use cna_tables::land::weather::WeatherKind;
use std::sync::OnceLock;
fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn setup() -> (State, UnitId) {
    let mut state = State::new(content()).unwrap();
    let id: UnitId = "it.1_libyan_div.viii_libyan_bn".into();
    for (other, unit) in &mut state.land.units {
        if other != &id {
            unit.location = Location::NotArrived;
        }
    }
    state.turn.weather = Some(WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    state.cursor.op_stage = Some(1);
    (state, id)
}
fn run(state: &mut State) -> Vec<EngineEvent> {
    let mut rng = CampaignRng::from_seed([4; 32]);
    let mut events = vec![];
    enter(
        content(),
        state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    events
}
/// Cases: airlog:52.53, land:3.6
#[test]
fn first_dry_stage_loses_nothing_then_each_consecutive_stage_loses_one_infantry_toe() {
    let (mut state, id) = setup();
    for (op, left) in [(1, 5), (2, 4), (3, 3)] {
        state.cursor.op_stage = Some(op);
        water::finalize(content(), &mut state, Side::Axis, true).unwrap();
        let events = run(&mut state);
        run(&mut state);
        assert_eq!(strength(content(), &state, &id).unwrap(), left);
        assert!(
            events
                .iter()
                .all(|e| e.audience == Audience::Side(Side::Axis))
        );
    }
}
/// Cases: airlog:51.22, airlog:52.53
#[test]
fn explicit_weapons_and_trucks_are_not_infantry_casualties() {
    let (mut state, id) = setup();
    state.land.units.get_mut(&id).unwrap().toe =
        Some(Toe::Weapons(vec![cna_content::units::WeaponPoints {
            weapon: "it.cv33".into(),
            n: 4,
        }]));
    state.land.units.get_mut(&id).unwrap().trucks.light = 2;
    let stage = WaterStage::current(&state);
    let r = state.logistics.rations.entry(id.clone()).or_default();
    r.water_finalized_stage = Some(stage);
    state
        .logistics
        .rations
        .get_mut(&id)
        .unwrap()
        .last_short_water_stage = Some(WaterStage::current(&state));
    state
        .logistics
        .rations
        .get_mut(&id)
        .unwrap()
        .consecutive_short_water_stages = 3;
    let unit_before = state.land.units[&id].clone();
    run(&mut state);
    assert_eq!(state.land.units[&id], unit_before);
}
/// Cases: airlog:51.22, land:3.6
/// Interpretations: interp:airlog-0007
#[test]
fn food_loss_rounds_the_hex_total_once_and_owner_selects_the_casualties() {
    let mut state = State::new(content()).unwrap();
    state.cursor.op_stage = Some(1);
    state.cursor.game_turn = 2;
    let hex = state.land.units[&UnitId::new("it.1_libyan_div.viii_libyan_bn")]
        .location
        .clone();
    let ids: Vec<_> = state
        .land
        .units
        .values()
        .filter(|u| u.side == Side::Axis && rations::in_play(&u.location))
        .filter(|u| rations::infantry(content(), &u.id).unwrap_or(false))
        .map(|u| u.id.clone())
        .collect();
    let mut total = 0;
    for id in &ids {
        state.land.units.get_mut(id).unwrap().location = hex.clone();
        total += strength(content(), &state, id).unwrap();
        let r = state.logistics.rations.entry(id.clone()).or_default();
        r.finalized_gt = Some(2);
        r.last_short_gt = Some(2);
        r.consecutive_short_gt = 2;
    }
    let expected = (total * 2 + 50) / 100;
    assert!(expected > 0);
    run(&mut state);
    assert_eq!(
        state
            .logistics
            .food_losses
            .iter()
            .map(|l| l.remaining)
            .sum::<i32>(),
        expected
    );
    assert!(
        state
            .decisions
            .pending
            .iter()
            .all(|p| p.secrecy == Secrecy::Secret)
    );
    let mut remaining = expected;
    while remaining > 0 {
        let pending = state.decisions.pending.remove(0);
        let ActionSchema::Choice { options } = &pending.space.schema else {
            panic!("expected infantry choices")
        };
        let action = serde_json::json!(options[0].id);
        let mut rng = CampaignRng::from_seed([5; 32]);
        let mut events = vec![];
        if remaining == expected {
            let before = serde_json::to_value(&state).unwrap();
            assert!(
                answer(
                    content(),
                    &mut state,
                    &pending,
                    &serde_json::json!("not-an-infantry-unit"),
                    &mut Cx {
                        rng: &mut rng,
                        events: &mut events
                    }
                )
                .is_err()
            );
            assert_eq!(serde_json::to_value(&state).unwrap(), before);
        }
        answer(
            content(),
            &mut state,
            &pending,
            &action,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        remaining -= 1;
    }
    let after: i32 = ids
        .iter()
        .map(|id| strength(content(), &state, id).unwrap())
        .sum();
    assert_eq!(after, total - expected);
    assert!(state.decisions.pending.is_empty());
}

/// Cases: land:4.45, airlog:51.22, airlog:52.53
#[test]
fn casualties_write_actual_remaining_strength_including_normal_boundary() {
    let (mut state, id) = setup();
    let max = rations::class(content(), &id).unwrap().max_toe.unwrap();
    state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Over { over: max + 2 });
    lose(content(), &mut state, &id, 1).unwrap();
    assert_eq!(state.land.units[&id].toe, Some(Toe::Over { over: max + 1 }));
    lose(content(), &mut state, &id, 1).unwrap();
    assert_eq!(
        state.land.units[&id].toe,
        Some(Toe::Normal(cna_content::units::NormalToe::N))
    );
    lose(content(), &mut state, &id, 1).unwrap();
    assert_eq!(
        state.land.units[&id].toe,
        Some(Toe::Under { under: max - 1 })
    );
    assert_eq!(
        crate::view::toe_points(content(), &state.land.units[&id]),
        Some(max - 1)
    );
    let mut restored: State =
        serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    lose(content(), &mut restored, &id, max - 1).unwrap();
    assert_eq!(restored.land.units[&id].toe, Some(Toe::Under { under: 0 }));
    assert_eq!(
        toe_strength(content(), &restored.land.units[&id])
            .unwrap()
            .get(),
        0
    );
}
