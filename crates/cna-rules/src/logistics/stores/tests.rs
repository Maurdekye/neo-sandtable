use super::super::{PrisonerGroup, SupplySource};
use super::*;
use crate::Cna;
use crate::state::{Dump, UnitSupply};
use cna_content::scenario::Supplies;
use cna_core::decision::Secrecy;
use cna_core::dice::CampaignRng;
use cna_core::engine::Ruleset;
use cna_core::quantity::FuelTenths;
use cna_core::visibility::Perspective;
use std::sync::OnceLock;
fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn setup(stores: i32, water: i32) -> (State, UnitId) {
    let mut state = State::new(content()).unwrap();
    let id: UnitId = "it.1_libyan_div.viii_libyan_bn".into();
    let hex = state.land.units[&id].location.hex().unwrap().clone();
    state.logistics.dumps.clear();
    state.logistics.dumps.insert(
        "food".into(),
        Dump {
            marker: String::new(),
            id: "food".into(),
            side: Side::Axis,
            location: DumpLocation::Hex { hex },
            supplies: Supplies {
                stores,
                water,
                ..Supplies::default()
            },
            active: true,
            dummy: false,
        },
    );
    (state, id)
}
fn allocation(stores: i32, water: i32) -> Vec<SupplyDraw> {
    vec![SupplyDraw {
        source: SupplySource::Dump("food".into()),
        amount: SupplyDemand {
            stores: StoresPoints::new(stores),
            water: WaterPoints::new(water),
            ..SupplyDemand::default()
        },
    }]
}
fn finalize_one(state: &mut State, id: &UnitId) {
    for (other, unit) in &mut state.land.units {
        if other != id {
            unit.location = Location::NotArrived;
        }
    }
    finalize(content(), state, Side::Axis).unwrap();
}

/// Cases: airlog:51.11, airlog:51.15, airlog:51.21, airlog:52.6
#[test]
fn full_issue_consumes_current_toe_stores_and_pasta_once() {
    let (mut state, id) = setup(100, 2);
    let full = rations::stores_required(content(), &state, &id).unwrap();
    assert_eq!(full, 20);
    issue_unit(
        content(),
        &mut state,
        &id,
        full,
        false,
        true,
        &allocation(full, 1),
    )
    .unwrap();
    assert_eq!(state.logistics.dumps["food"].supplies.stores, 80);
    assert_eq!(state.logistics.dumps["food"].supplies.water, 1);
    assert!(
        issue_unit(
            content(),
            &mut state,
            &id,
            full,
            false,
            true,
            &allocation(full, 1)
        )
        .is_err()
    );
    finalize_one(&mut state, &id);
    assert_eq!(state.land.units[&id].cohesion_quarters, 0);
    assert_eq!(state.logistics.rations[&id].consecutive_short_gt, 0);
}
/// Cases: airlog:51.21, airlog:51.22, airlog:51.23
/// Interpretations: interp:airlog-0007
#[test]
fn half_rations_require_shortage_and_partial_full_rations_add_one_dp_per_gt() {
    let (mut state, id) = setup(20, 0);
    let before = serde_json::to_value(&state).unwrap();
    assert!(
        issue_unit(
            content(),
            &mut state,
            &id,
            10,
            true,
            false,
            &allocation(10, 0)
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    state
        .logistics
        .dumps
        .get_mut("food")
        .unwrap()
        .supplies
        .stores = 12;
    issue_unit(
        content(),
        &mut state,
        &id,
        10,
        true,
        false,
        &allocation(10, 0),
    )
    .unwrap();
    finalize_one(&mut state, &id);
    assert!(state.logistics.rations[&id].half);
    assert_eq!(state.land.units[&id].cohesion_quarters, 0);
    state.cursor.game_turn = 2;
    issue_unit(
        content(),
        &mut state,
        &id,
        2,
        false,
        false,
        &allocation(2, 0),
    )
    .unwrap();
    finalize_one(&mut state, &id);
    finalize_one(&mut state, &id);
    assert_eq!(state.land.units[&id].cohesion_quarters, -4);
    assert_eq!(state.logistics.rations[&id].consecutive_short_gt, 1);
    state.cursor.game_turn = 3;
    finalize_one(&mut state, &id);
    assert_eq!(state.land.units[&id].cohesion_quarters, -8);
    assert_eq!(state.logistics.rations[&id].consecutive_short_gt, 2);
}
/// Cases: airlog:51.13, airlog:51.15, airlog:51.23, land:8.84
#[test]
fn flat_rate_headquarters_and_offmap_sources_are_not_guessed_or_halved() {
    let (mut state, id) = setup(2, 0);
    let hq = state
        .land
        .units
        .keys()
        .find(|id| {
            content().units.units[*id]
                .class
                .as_ref()
                .and_then(|c| content().units.classes.get(c))
                .is_some_and(|c| c.unit_type == "headquarters")
                && state.land.units[id].side == Side::Axis
                && rations::in_play(&state.land.units[id].location)
        })
        .unwrap()
        .clone();
    assert_eq!(rations::stores_required(content(), &state, &hq).unwrap(), 1);
    state.land.units.get_mut(&hq).unwrap().location = Location::OffMap {
        id: "box_tripoli".into(),
    };
    state.logistics.dumps.get_mut("food").unwrap().location = DumpLocation::OffMap {
        id: "box_tripoli".into(),
    };
    assert!(
        issue_unit(
            content(),
            &mut state,
            &hq,
            1,
            true,
            false,
            &allocation(1, 0)
        )
        .is_err()
    );
    issue_unit(
        content(),
        &mut state,
        &hq,
        1,
        false,
        false,
        &allocation(1, 0),
    )
    .unwrap();
    assert_eq!(state.logistics.dumps["food"].supplies.stores, 1);
    // On-map unit cannot reach the off-map stock.
    assert!(
        issue_unit(
            content(),
            &mut state,
            &id,
            1,
            false,
            false,
            &allocation(1, 0)
        )
        .is_err()
    );
}
/// Cases: airlog:49.3, airlog:52.44
#[test]
fn weekly_losses_use_campaign_date_and_exclude_offmap_stocks() {
    let (mut state, id) = setup(100, 100);
    state.logistics.dumps.get_mut("food").unwrap().supplies.fuel = 100;
    let mut cw = state.logistics.dumps["food"].clone();
    cw.id = "cw".into();
    cw.side = Side::Commonwealth;
    state.logistics.dumps.insert("cw".into(), cw.clone());
    cw.id = "offmap".into();
    cw.location = DumpLocation::OffMap {
        id: "box_tripoli".into(),
    };
    state.logistics.dumps.insert("offmap".into(), cw);
    state.logistics.unit_supply.insert(
        id.clone(),
        UnitSupply {
            activity_water: WaterPoints::new(100),
            tank_fuel: FuelTenths::new(1003),
            ..UnitSupply::default()
        },
    );
    weekly_losses(content(), &mut state);
    assert_eq!(state.logistics.dumps["food"].supplies.fuel, 94);
    assert_eq!(state.logistics.dumps["cw"].supplies.water, 91);
    assert_eq!(state.logistics.dumps["offmap"].supplies.water, 100);
    assert_eq!(state.logistics.unit_supply[&id].tank_fuel.get(), 943);
    assert_eq!(state.logistics.unit_supply[&id].activity_water.get(), 94);
    state.cursor.game_turn = 53;
    state.logistics.dumps.get_mut("cw").unwrap().supplies.water = 100;
    weekly_losses(content(), &mut state);
    assert_eq!(state.logistics.dumps["cw"].supplies.water, 94);
}
/// Cases: airlog:51.12, airlog:51.17, land:28.15, land:3.6
/// Interpretations: interp:airlog-0007
#[test]
fn prisoners_are_aggregated_and_served_before_guards_and_units_without_enemy_leaks() {
    let (mut state, id) = setup(20, 0);
    let group = PrisonerGroup {
        owner: Side::Axis,
        location: state.land.units[&id].location.clone(),
        prisoner_points: 1,
        guard_points: 1,
        food_stage: None,
        stores_short: 0,
        guards_fed_gt: None,
        guards_stores_short: 0,
    };
    state.logistics.prisoners.insert("p1".into(), group.clone());
    let mut group2 = group;
    group2.guard_points = 0;
    state.logistics.prisoners.insert("p2".into(), group2);
    let mut rng = CampaignRng::from_seed([1; 32]);
    let mut events = Vec::new();
    enter(
        content(),
        &mut state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert_eq!(state.logistics.dumps["food"].supplies.stores, 17);
    assert!(
        events
            .iter()
            .filter(|e| matches!(e.event, GameEvent::Note { .. }))
            .all(|e| e.audience == Audience::Side(Side::Axis))
    );
    assert!(
        state
            .decisions
            .pending
            .iter()
            .all(|p| p.secrecy == Secrecy::SecretSimultaneous)
    );
    let before = state.logistics.dumps["food"].supplies.stores;
    feed_prisoners(
        content(),
        &mut state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        true,
    )
    .unwrap();
    assert_eq!(state.logistics.dumps["food"].supplies.stores, before);
    state.cursor.op_stage = Some(2);
    feed_prisoners(
        content(),
        &mut state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        false,
    )
    .unwrap();
    assert_eq!(state.logistics.dumps["food"].supplies.stores, before - 1);
    let enemy = Cna::dev().observe(content(), &state, Perspective::Side(Side::Commonwealth));
    assert!(
        enemy["logistics"]["prisoners"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert!(
        enemy["logistics"]["rations"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}
/// Cases: airlog:51.15, airlog:52.6
#[test]
fn invalid_sources_and_missing_pasta_keep_the_entire_state_unchanged() {
    let (mut state, id) = setup(20, 0);
    let before = serde_json::to_value(&state).unwrap();
    assert!(
        issue_unit(
            content(),
            &mut state,
            &id,
            12,
            false,
            true,
            &allocation(20, 1)
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    state.land.units.get_mut(&id).unwrap().cohesion_quarters = -40;
    issue_unit(
        content(),
        &mut state,
        &id,
        20,
        false,
        false,
        &allocation(20, 0),
    )
    .unwrap();
    finalize_one(&mut state, &id);
    assert_eq!(state.land.units[&id].cohesion_quarters, -104);
    state
        .logistics
        .dumps
        .get_mut("food")
        .unwrap()
        .supplies
        .water = 1;
    // A later water distribution can supply the missing pasta and restore the saved level.
    rations::receive_pasta(&mut state, &id);
    assert_eq!(state.land.units[&id].cohesion_quarters, -40);
}

/// Cases: airlog:51.15, land:3.6
#[test]
fn empty_stock_domain_requires_an_empty_allocation_list() {
    let empty = vec![SupplyDraw {
        source: SupplySource::Tank,
        amount: SupplyDemand::default(),
    }];
    let ActionSchema::List { min, max, .. } = draw_schema(&empty, 20, 1) else {
        panic!("list required")
    };
    assert_eq!((min, max), (0, 0));
    let source = allocation(1, 0);
    let ActionSchema::List { min, max, .. } = draw_schema(&source, 20, 1) else {
        panic!("list required")
    };
    assert_eq!((min, max), (0, 1));
}
