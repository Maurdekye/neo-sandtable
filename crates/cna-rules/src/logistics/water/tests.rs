use super::super::{SupplySource, movement_restrictions, spend_activity_water};
use super::*;
use crate::Cna;
use crate::state::{Dump, DumpLocation, Location, WeatherState};
use cna_content::scenario::Supplies;
use cna_core::dice::CampaignRng;
use cna_core::engine::Ruleset;
use cna_core::visibility::Perspective;
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
    state.cursor.op_stage = Some(1);
    state.turn.weather = Some(WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    let hex = state.land.units[&id].location.hex().unwrap().clone();
    state.logistics.dumps.clear();
    state.logistics.dumps.insert(
        "water".into(),
        Dump {
            id: "water".into(),
            side: Side::Axis,
            location: DumpLocation::Hex { hex },
            supplies: Supplies {
                water: 100,
                ..Supplies::default()
            },
            active: true,
            dummy: false,
        },
    );
    (state, id)
}
fn allocation(water: i32) -> Vec<SupplyDraw> {
    vec![SupplyDraw {
        source: SupplySource::Dump("water".into()),
        amount: SupplyDemand {
            water: WaterPoints::new(water),
            ..SupplyDemand::default()
        },
    }]
}
/// Cases: airlog:52.41, airlog:52.42, airlog:52.6
#[test]
fn infantry_drinks_now_but_idle_trucks_retain_water_until_first_cp_use() {
    let (mut state, id) = setup();
    state.land.units.get_mut(&id).unwrap().trucks.light = 2;
    let history = state.logistics.rations.entry(id.clone()).or_default();
    history.issued_gt = Some(1);
    history.stores_received = 20;
    assert_eq!(
        requirements(content(), &state, &id).unwrap(),
        WaterRequirements {
            infantry: 1,
            activity: 2,
            pasta: 1
        }
    );
    issue_unit(content(), &mut state, &id, 1, 2, true, &allocation(4)).unwrap();
    assert_eq!(state.logistics.dumps["water"].supplies.water, 96);
    assert_eq!(state.logistics.unit_supply[&id].activity_water.get(), 2);
    assert!(
        movement_restrictions(content(), &state, &id)
            .unwrap()
            .may_move
    );
    state.cursor.op_stage = Some(2);
    assert_eq!(requirements(content(), &state, &id).unwrap().activity, 0);
    issue_unit(content(), &mut state, &id, 1, 0, false, &allocation(1)).unwrap();
    assert_eq!(state.logistics.unit_supply[&id].activity_water.get(), 2);
    spend_activity_water(content(), &mut state, &id).unwrap();
    spend_activity_water(content(), &mut state, &id).unwrap();
    assert_eq!(state.logistics.unit_supply[&id].activity_water.get(), 0);
    assert!(
        movement_restrictions(content(), &state, &id)
            .unwrap()
            .may_move
    );
    state.cursor.op_stage = Some(3);
    assert_eq!(requirements(content(), &state, &id).unwrap().activity, 2);
    assert!(
        !movement_restrictions(content(), &state, &id)
            .unwrap()
            .may_move
    );
}
/// Cases: airlog:52.43, land:29.31, airlog:52.6
#[test]
fn hot_weather_doubles_body_and_truck_water_but_not_weekly_pasta() {
    let (mut state, id) = setup();
    state.land.units.get_mut(&id).unwrap().trucks.medium = 3;
    state.turn.weather.as_mut().unwrap().kind = WeatherKind::Hot;
    let history = state.logistics.rations.entry(id.clone()).or_default();
    history.issued_gt = Some(1);
    history.stores_received = 20;
    assert_eq!(
        requirements(content(), &state, &id).unwrap(),
        WaterRequirements {
            infantry: 2,
            activity: 6,
            pasta: 1
        }
    );
    issue_unit(content(), &mut state, &id, 2, 6, true, &allocation(9)).unwrap();
    spend_activity_water(content(), &mut state, &id).unwrap();
    assert_eq!(state.logistics.unit_supply[&id].activity_water.get(), 0);
    assert_eq!(state.logistics.dumps["water"].supplies.water, 91);
}
/// Cases: airlog:52.41, airlog:52.51, airlog:52.52, airlog:51.23
#[test]
fn dry_foot_infantry_can_move_with_cpa_limit_but_dry_trucks_cannot() {
    let (mut state, id) = setup();
    let limits = movement_restrictions(content(), &state, &id).unwrap();
    assert!(limits.may_move);
    assert!(!limits.may_exceed_cpa);
    assert!(!limits.may_offensive_close_assault);
    assert_eq!(limits.defense_divisor, 2);
    state.land.units.get_mut(&id).unwrap().trucks.light = 1;
    let before = serde_json::to_value(&state).unwrap();
    assert!(
        !movement_restrictions(content(), &state, &id)
            .unwrap()
            .may_move
    );
    assert_eq!(
        spend_activity_water(content(), &mut state, &id),
        Err(SupplyError::Insufficient)
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    state.logistics.rations.entry(id.clone()).or_default().half = true;
    state.logistics.rations.get_mut(&id).unwrap().issued_gt = Some(1);
    assert!(
        !movement_restrictions(content(), &state, &id)
            .unwrap()
            .may_enter_enemy_zoc
    );
}
/// Cases: airlog:52.53
#[test]
fn shortage_streak_is_once_per_stage_crosses_week_boundary_and_resets_on_water() {
    let (mut state, id) = setup();
    for (gt, op, expected) in [(1, 1, 1), (1, 2, 2), (1, 3, 3), (2, 1, 4)] {
        state.cursor.game_turn = gt;
        state.cursor.op_stage = Some(op);
        finalize(content(), &mut state, Side::Axis, true).unwrap();
        finalize(content(), &mut state, Side::Axis, true).unwrap();
        assert_eq!(
            state.logistics.rations[&id].consecutive_short_water_stages,
            expected
        );
    }
    state.cursor.op_stage = Some(2);
    issue_unit(content(), &mut state, &id, 1, 0, false, &allocation(1)).unwrap();
    finalize(content(), &mut state, Side::Axis, true).unwrap();
    assert_eq!(
        state.logistics.rations[&id].consecutive_short_water_stages,
        0
    );
}
/// Cases: airlog:52.41, airlog:52.42, land:3.6
#[test]
fn rejected_draw_is_atomic_and_checkpoint_and_enemy_views_keep_quantities_private() {
    let (mut state, id) = setup();
    let before = serde_json::to_value(&state).unwrap();
    assert!(issue_unit(content(), &mut state, &id, 1, 0, false, &allocation(2)).is_err());
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    issue_unit(content(), &mut state, &id, 1, 0, false, &allocation(1)).unwrap();
    assert!(issue_unit(content(), &mut state, &id, 1, 0, false, &allocation(1)).is_err());
    let saved = serde_json::to_string(&state).unwrap();
    let restored: State = serde_json::from_str(&saved).unwrap();
    assert_eq!(
        restored.logistics.rations[&id],
        state.logistics.rations[&id]
    );
    let enemy = Cna::dev().observe(content(), &state, Perspective::Side(Side::Commonwealth));
    assert!(
        enemy["logistics"]["rations"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert!(
        Cna::dev()
            .inspect(
                content(),
                &state,
                Perspective::Side(Side::Commonwealth),
                id.as_str()
            )
            .is_err()
    );
}
/// Cases: airlog:52.42, land:3.6
/// Interpretations: interp:units-0005
#[test]
fn unknown_hq_composition_blocks_full_but_dev_reports_privately_and_continues() {
    let mut state = State::new(content()).unwrap();
    state.cursor.op_stage = Some(1);
    state.turn.weather = Some(WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    let mut rng = CampaignRng::from_seed([9; 32]);
    let mut events = vec![];
    let before = serde_json::to_value(&state).unwrap();
    let rng_before = rng.state();
    assert!(
        matches!(enter(content(), &mut state, &mut Cx { rng: &mut rng, events: &mut events }, true), Err(EngineError::Unsupported { case, .. }) if case == "airlog:52.42")
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    assert_eq!(rng.state(), rng_before);
    assert!(events.is_empty());
    enter(
        content(),
        &mut state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        false,
    )
    .unwrap();
    assert!(
        events
            .iter()
            .filter(|e| matches!(e.event, GameEvent::Note { .. }))
            .all(|e| matches!(e.audience, Audience::Side(_)))
    );
    assert!(
        state
            .decisions
            .pending
            .iter()
            .all(|p| p.secrecy == Secrecy::Secret)
    );
}

/// Cases: airlog:52.6
#[test]
fn pasta_disorganization_requires_pasta_to_clear_and_never_improves_worse_cohesion() {
    let (mut state, id) = setup();
    state.land.units.get_mut(&id).unwrap().cohesion_quarters = -120;
    rations::apply_pasta(content(), &mut state, &id);
    assert_eq!(state.land.units[&id].cohesion_quarters, -120);
    assert_eq!(
        state.logistics.rations[&id].pasta_saved_cohesion_quarters,
        Some(-120)
    );
    state.land.units.get_mut(&id).unwrap().cohesion_quarters = -20;
    assert!(
        !movement_restrictions(content(), &state, &id)
            .unwrap()
            .may_move
    );
    rations::apply_pasta(content(), &mut state, &id);
    assert_eq!(state.land.units[&id].cohesion_quarters, -104);
    rations::receive_pasta(&mut state, &id);
    assert_eq!(state.land.units[&id].cohesion_quarters, -120);
    assert!(
        state.logistics.rations[&id]
            .pasta_saved_cohesion_quarters
            .is_none()
    );
}
