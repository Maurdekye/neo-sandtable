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
// Explicit consumption fixtures test retirement mechanics, not a claim that
// these current tags arise at the earlier PRE/Water anchors in real play.
fn special_retirement_fixture() -> (State, UnitId, Location) {
    use super::super::cargo_history::{CargoHistory, CargoLot, CargoSite};
    let (mut state, id) = setup(20, 0);
    state.cursor.op_stage = Some(1);
    let location = state.land.units[&id].location.clone();
    state.land.units.get_mut(&id).unwrap().trucks.light = 1;
    state
        .logistics
        .unit_supply
        .entry(id.clone())
        .or_default()
        .carried
        .stores = 3;
    let lots = vec![
        CargoLot {
            id: "lower".into(),
            goods: Supplies {
                stores: 1,
                ..Default::default()
            },
            spent_cp_quarters: 12,
            ceiling_cp_quarters: 160,
            continuous_first_line: false,
        },
        CargoLot {
            id: "greater".into(),
            goods: Supplies {
                stores: 1,
                ..Default::default()
            },
            spent_cp_quarters: 48,
            ceiling_cp_quarters: 160,
            continuous_first_line: false,
        },
    ];
    let history = CargoHistory {
        stage: WaterStage::current(&state),
        lots,
    };
    state
        .logistics
        .cargo_history
        .histories
        .insert(CargoSite::Unit(id.clone()), history.clone());
    state
        .logistics
        .cargo_history
        .histories
        .insert(CargoSite::Dump("food".into()), history);
    (state, id, location)
}
/// Cases: airlog:51.12, airlog:51.17, airlog:53.25
/// Interpretations: interp:airlog-0020
#[test]
fn special_consumption_retires_greatest_cp_tags_then_fresh_and_preserves_stock_order() {
    use super::super::cargo_history::CargoSite;
    let (mut state, id, location) = special_retirement_fixture();
    assert_eq!(
        draw_special_stores(content(), &mut state, Side::Axis, &location, 1).unwrap(),
        0
    );
    let unit = &state.logistics.cargo_history.histories[&CargoSite::Unit(id.clone())];
    assert_eq!(unit.lots.len(), 1);
    assert_eq!(unit.lots[0].id, "lower");
    assert_eq!(state.logistics.unit_supply[&id].carried.stores, 2);
    assert_eq!(state.logistics.dumps["food"].supplies.stores, 20);
    assert_eq!(
        draw_special_stores(content(), &mut state, Side::Axis, &location, 2).unwrap(),
        0
    );
    assert_eq!(state.logistics.unit_supply[&id].carried.stores, 0);
    assert!(
        state.logistics.cargo_history.histories[&CargoSite::Unit(id)]
            .lots
            .is_empty()
    );
    assert_eq!(
        draw_special_stores(content(), &mut state, Side::Axis, &location, 1).unwrap(),
        0
    );
    assert_eq!(state.logistics.dumps["food"].supplies.stores, 19);
    let dump = &state.logistics.cargo_history.histories[&CargoSite::Dump("food".into())];
    assert_eq!(dump.lots.len(), 1);
    assert_eq!(dump.lots[0].id, "lower");
    let saved: State = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(&saved).unwrap(),
        serde_json::to_value(&state).unwrap()
    );
}
/// Cases: airlog:51.12, airlog:51.17, airlog:53.25, land:3.6
#[test]
fn special_consumption_late_malformed_dump_rolls_back_prior_unit_debit_and_expired_is_ignored() {
    use super::super::cargo_history::CargoSite;
    let (mut state, _, location) = special_retirement_fixture();
    state
        .logistics
        .cargo_history
        .histories
        .get_mut(&CargoSite::Dump("food".into()))
        .unwrap()
        .lots[0]
        .spent_cp_quarters = -1;
    let before = serde_json::to_value(&state).unwrap();
    assert!(matches!(
        draw_special_stores(content(), &mut state, Side::Axis, &location, 5),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    state.cursor.op_stage = Some(2);
    assert_eq!(
        draw_special_stores(content(), &mut state, Side::Axis, &location, 5).unwrap(),
        0
    );
    assert_eq!(state.logistics.dumps["food"].supplies.stores, 18);
}
/// Cases: airlog:51.12, airlog:51.17, airlog:53.25, land:3.6
#[test]
fn special_consumption_current_tags_never_refuse_valid_mandatory_stores_or_change_shortage() {
    let (mut tagged, id, location) = special_retirement_fixture();
    let mut plain = tagged.clone();
    plain.logistics.cargo_history.histories.clear();
    assert_eq!(
        draw_special_stores(content(), &mut tagged, Side::Axis, &location, 30).unwrap(),
        7
    );
    assert_eq!(
        draw_special_stores(content(), &mut plain, Side::Axis, &location, 30).unwrap(),
        7
    );
    assert_eq!(
        tagged.logistics.unit_supply[&id].carried,
        plain.logistics.unit_supply[&id].carried
    );
    assert_eq!(
        tagged.logistics.dumps["food"].supplies,
        plain.logistics.dumps["food"].supplies
    );
    crate::testkit::assert_indistinguishable(
        &Cna::dev(),
        content(),
        &tagged,
        &plain,
        Side::Commonwealth,
    );
}

// These games use real dispatchers with explicitly synthetic pre-entry tags.
// They do not assert that distribution can precede PRE or Water in real play.
fn feeding_entry_game(water: bool) -> (cna_core::engine::Game<Cna>, UnitId) {
    use crate::seq::Block;
    let (mut state, id, location) = special_retirement_fixture();
    state.setup.closed = true;
    state.cursor.block = if water { Block::OpStage } else { Block::Pre };
    let anchor = if water {
        "opstage.organization.water_distribution"
    } else {
        "logistics.stores_expenditure"
    };
    state.cursor.index = state
        .cursor
        .block
        .steps()
        .iter()
        .position(|step| step.anchor == anchor)
        .unwrap();
    state.cursor.entered = false;
    state.turn.weather = Some(crate::state::WeatherState {
        kind: cna_tables::land::weather::WeatherKind::Normal,
        storm_sections: vec![],
    });
    state.logistics.prisoners.insert(
        "feeding".into(),
        PrisonerGroup {
            owner: Side::Axis,
            location,
            prisoner_points: 1,
            guard_points: 1,
            food_stage: None,
            stores_short: 0,
            guards_fed_gt: None,
            guards_stores_short: 0,
        },
    );
    (
        cna_core::engine::Game {
            state,
            rng: CampaignRng::from_seed([27; 32]).state(),
        },
        id,
    )
}
/// Cases: airlog:51.12, airlog:51.17, airlog:53.25, land:3.6
#[test]
fn actual_pre_and_water_feeding_entries_retire_once_with_recovery_retry_and_private_streams() {
    use super::super::{batches, cargo_history::CargoSite};
    use cna_core::engine::{Command, evaluate};
    for (water, strict) in [(false, false), (false, true), (true, false)] {
        let rules = Cna { strict };
        let (game, id) = feeding_entry_game(water);
        let mut plain = game.clone();
        plain.state.logistics.cargo_history.histories.clear();
        crate::testkit::assert_action_indistinguishable(
            &rules,
            content(),
            &game,
            &plain,
            &Command::Advance,
            Side::Commonwealth,
        );
        let original = serde_json::to_value(&game).unwrap();
        let recovered = serde_json::from_value(original.clone()).unwrap();
        let first = evaluate(&rules, content(), &game, &Command::Advance).unwrap();
        let restored = evaluate(&rules, content(), &recovered, &Command::Advance).unwrap();
        assert_eq!(
            serde_json::to_value(&first.game).unwrap(),
            serde_json::to_value(&restored.game).unwrap()
        );
        assert_eq!(first.events, restored.events);
        assert_eq!(serde_json::to_value(&game).unwrap(), original);
        assert_eq!(first.game.rng, game.rng);
        let state = &first.game.state;
        assert_eq!(
            state.logistics.prisoners["feeding"].food_stage,
            Some(WaterStage::current(state))
        );
        assert_eq!(state.logistics.prisoners["feeding"].stores_short, 0);
        assert_eq!(
            state.logistics.prisoners["feeding"].guards_fed_gt,
            if water {
                None
            } else {
                Some(state.cursor.game_turn)
            }
        );
        assert_eq!(state.logistics.prisoners["feeding"].guards_stores_short, 0);
        assert_eq!(
            state.logistics.unit_supply[&id].carried.stores,
            if water { 2 } else { 0 }
        );
        assert_eq!(state.logistics.dumps["food"].supplies.stores, 20);
        let remaining = &state.logistics.cargo_history.histories[&CargoSite::Unit(id.clone())].lots;
        if water {
            assert_eq!(remaining.len(), 1);
            assert_eq!(remaining[0].id, "lower");
        } else {
            assert!(remaining.is_empty());
        }
        assert_eq!(
            state.logistics.cargo_history.histories[&CargoSite::Dump("food".into())]
                .lots
                .len(),
            2
        );
        let kind = if water {
            batches::WATER
        } else {
            batches::STORES
        };
        assert_eq!(
            state
                .decisions
                .pending
                .iter()
                .filter(|p| p.kind == kind)
                .count(),
            2
        );
        let saved = serde_json::from_value(serde_json::to_value(&first.game).unwrap()).unwrap();
        for source in [&first.game, &saved] {
            let retry = evaluate(&rules, content(), source, &Command::Advance).unwrap();
            assert_eq!(
                serde_json::to_value(&retry.game).unwrap(),
                serde_json::to_value(&first.game).unwrap()
            );
            assert!(retry.events.is_empty());
        }
        // The same mandatory-feeding ledger also protects direct entry retries.
        let mut retry = state.clone();
        let mut rng = CampaignRng::from_state(&first.game.rng);
        let mut events = first.events.clone();
        feed_prisoners(
            content(),
            &mut retry,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
            !water,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&retry).unwrap(),
            serde_json::to_value(state).unwrap()
        );
        assert_eq!(events, first.events);
        assert_eq!(rng.state(), first.game.rng);
    }
    // Full Water retains its earlier public-content composition stop; no bypass.
    let (game, _) = feeding_entry_game(true);
    let bytes = serde_json::to_value(&game).unwrap();
    assert!(matches!(evaluate(&Cna { strict: true }, content(), &game,
        &Command::Advance), Err(Rejection::Engine(EngineError::Unsupported { case, .. }))
        if case == "airlog:52.42"));
    assert_eq!(serde_json::to_value(&game).unwrap(), bytes);
}
/// Cases: airlog:51.12, airlog:51.17, airlog:53.25, land:3.6
#[test]
fn actual_feeding_late_group_malformed_history_rolls_back_whole_game_and_prior_events() {
    use super::super::cargo_history::CargoSite;
    use cna_core::engine::{Command, evaluate};
    for (water, strict) in [(false, false), (false, true), (true, false)] {
        let rules = Cna { strict };
        let (mut game, id) = feeding_entry_game(water);
        // Two synthetic groups use actual canonical neighboring sites. The first
        // succeeds; only the second group's current history is malformed.
        let first_location = Location::Hex {
            hex: "C4020".into(),
        };
        let second_location = Location::Hex {
            hex: "C4021".into(),
        };
        game.state.land.units.get_mut(&id).unwrap().location = first_location.clone();
        game.state.land.units.get_mut(&id).unwrap().trucks.light = 0;
        game.state
            .logistics
            .unit_supply
            .get_mut(&id)
            .unwrap()
            .carried
            .stores = 0;
        game.state
            .logistics
            .cargo_history
            .histories
            .remove(&CargoSite::Unit(id));
        game.state.logistics.dumps.get_mut("food").unwrap().location = DumpLocation::Hex {
            hex: "C4020".into(),
        };
        let mut second_dump = game.state.logistics.dumps["food"].clone();
        second_dump.id = "second".into();
        second_dump.location = DumpLocation::Hex {
            hex: "C4021".into(),
        };
        game.state
            .logistics
            .dumps
            .insert("second".into(), second_dump);
        let mut broken =
            game.state.logistics.cargo_history.histories[&CargoSite::Dump("food".into())].clone();
        broken.lots[0].spent_cp_quarters = -1;
        game.state
            .logistics
            .cargo_history
            .histories
            .insert(CargoSite::Dump("second".into()), broken);
        let first = game.state.logistics.prisoners.get_mut("feeding").unwrap();
        first.location = first_location;
        first.guard_points = 0;
        let mut second = first.clone();
        second.location = second_location;
        game.state
            .logistics
            .prisoners
            .insert("second".into(), second);
        let expected = EngineError::Invariant {
            detail: "invalid current cargo history at special stores consumption: Invalid".into(),
        };
        // Establish a genuinely late group error, rather than mere first-draw purity.
        let mut partial = game.state.clone();
        let mut rng = CampaignRng::from_state(&game.rng);
        let mut events = vec![EngineEvent::new(
            Audience::Side(Side::Axis),
            GameEvent::Note {
                text: "Previously received event".into(),
            },
        )];
        let prior_events = events.clone();
        assert_eq!(
            feed_prisoners(
                content(),
                &mut partial,
                &mut Cx {
                    rng: &mut rng,
                    events: &mut events
                },
                !water
            )
            .unwrap_err(),
            expected
        );
        assert_eq!(partial.logistics.dumps["food"].supplies.stores, 19);
        assert_eq!(
            partial.logistics.prisoners["feeding"].food_stage,
            Some(WaterStage::current(&partial))
        );
        assert!(events.len() > prior_events.len());
        assert_eq!(rng.state(), game.rng);
        // Only core evaluation is the whole-group transaction boundary. None of
        // the disposable partial consumption or notes may reach its caller.
        let bytes = serde_json::to_value(&game).unwrap();
        let restored = serde_json::from_value(bytes.clone()).unwrap();
        let prior = serde_json::to_value(&prior_events).unwrap();
        for source in [&game, &restored, &game] {
            assert_eq!(
                evaluate(&rules, content(), source, &Command::Advance).unwrap_err(),
                Rejection::Engine(expected.clone())
            );
            assert_eq!(serde_json::to_value(source).unwrap(), bytes);
            assert_eq!(source.rng, game.rng);
        }
        assert_eq!(serde_json::to_value(&prior_events).unwrap(), prior);
    }
}
