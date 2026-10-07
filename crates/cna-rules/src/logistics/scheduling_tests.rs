//! Fixed private schedules through the real dispatcher, including checkpoint replay.
use super::*;
use crate::{
    Cna, CnaContent, State,
    seq::{Block, Half, OPSTAGE, PLAYER_HALF, PRE},
    state::{Dump, DumpLocation, Location, Pending, WeatherState},
};
use cna_core::{
    decision::DecisionResponse,
    dice::CampaignRng,
    engine::{Command, Game, Ruleset, evaluate},
};
use cna_protocol::Side;
use cna_tables::land::weather::WeatherKind;
use serde_json::{Value, json};
use std::sync::OnceLock;
const AX: &str = "it.1_libyan_div.viii_libyan_bn";
const CW: &str = "cw.2_nz_div.21st_nz_bn";
fn content() -> &'static CnaContent {
    static C: OnceLock<CnaContent> = OnceLock::new();
    C.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn game(anchor: &str) -> Game<Cna> {
    let mut s = State::new(content()).unwrap();
    s.turn.player_a = Some(Side::Axis);
    s.turn.weather = Some(WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    s.cursor.op_stage = Some(1);
    s.cursor.entered = false;
    if let Some(i) = PRE.iter().position(|d| d.anchor == anchor) {
        s.cursor.block = Block::Pre;
        s.cursor.index = i;
    } else if let Some(i) = OPSTAGE.iter().position(|d| d.anchor == anchor) {
        s.cursor.block = Block::OpStage;
        s.cursor.index = i;
    } else {
        s.cursor.block = Block::PlayerHalf;
        s.cursor.half = Some(Half::A);
        s.cursor.index = PLAYER_HALF.iter().position(|d| d.anchor == anchor).unwrap();
    }
    for u in s.land.units.values_mut() {
        u.location = Location::NotArrived;
        u.trucks = Default::default();
        u.transport_trucks = Default::default();
    }
    s.land.units.get_mut(&CW.into()).unwrap().location = Location::OffMap {
        id: "box_tripoli".into(),
    };
    Game {
        state: s,
        rng: CampaignRng::from_seed([83; 32]).state(),
    }
}
fn command(p: &Pending, action: Value) -> Command {
    Command::Respond(DecisionResponse {
        decision_id: p.id.clone(),
        seat: p.seat,
        controller_epoch: 1,
        decision_revision: p.revision,
        idempotency_key: format!("schedule-{}", p.id),
        action,
        public_explanation: None,
    })
}
fn advance_pair(a: &mut Game<Cna>, b: &mut Game<Cna>, observer: Side) {
    crate::testkit::assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        a,
        b,
        &Command::Advance,
        observer,
    );
    for g in [a, b] {
        let saved: Game<Cna> = serde_json::from_value(serde_json::to_value(&*g).unwrap()).unwrap();
        let t = evaluate(&Cna::dev(), content(), g, &Command::Advance).unwrap();
        let replay = evaluate(&Cna::dev(), content(), &saved, &Command::Advance).unwrap();
        assert_eq!(
            serde_json::to_value(&t.game).unwrap(),
            serde_json::to_value(&replay.game).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&t.events).unwrap(),
            serde_json::to_value(&replay.events).unwrap()
        );
        *g = t.game;
    }
}
fn answer_pair(
    a: &mut Game<Cna>,
    b: &mut Game<Cna>,
    side: Side,
    action_a: Value,
    action_b: Value,
    observer: Side,
) {
    let pa = a
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == side)
        .unwrap()
        .clone();
    let pb = b
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == side)
        .unwrap()
        .clone();
    let ca = command(&pa, action_a);
    let cb = command(&pb, action_b);
    crate::testkit::assert_actions_indistinguishable(
        &Cna::dev(),
        content(),
        (a, &ca),
        (b, &cb),
        observer,
    );
    *a = evaluate(&Cna::dev(), content(), a, &ca).unwrap().game;
    *b = evaluate(&Cna::dev(), content(), b, &cb).unwrap().game;
}
/// Empty enemy rosters never skip a side request or delay the public phase.
/// Cases: land:3.6, airlog:48.0, airlog:51.11, airlog:54.0
#[test]
fn stores_and_distribution_have_fixed_side_windows_and_no_eligibility_reopen() {
    for anchor in [
        "logistics.stores_expenditure",
        "opstage.organization.supply_distribution",
    ] {
        let mut a = game(anchor);
        let mut b = a.clone();
        b.state.land.units.get_mut(&AX.into()).unwrap().location = Location::OffMap {
            id: "box_tripoli".into(),
        };
        advance_pair(&mut a, &mut b, Side::Commonwealth);
        assert_eq!(a.state.decisions.pending.len(), 2);
        assert_eq!(b.state.decisions.pending.len(), 2);
        answer_pair(
            &mut a,
            &mut b,
            Side::Axis,
            Value::Null,
            Value::Null,
            Side::Commonwealth,
        );
        assert!(
            !a.state
                .decisions
                .pending
                .iter()
                .any(|p| p.seat.side == Side::Axis)
        );
        answer_pair(
            &mut a,
            &mut b,
            Side::Commonwealth,
            Value::Null,
            Value::Null,
            Side::Commonwealth,
        );
        advance_pair(&mut a, &mut b, Side::Commonwealth);
        assert_ne!(a.state.cursor.anchor(), anchor);
        assert_ne!(b.state.cursor.anchor(), anchor);
    }
}
/// A secret draw produces the same two rounds as a pass, including an empty allocation.
/// Cases: land:3.6, airlog:52.13, airlog:52.41, airlog:52.42
#[test]
fn water_has_two_fixed_rounds_across_hidden_rosters_and_actual_draws() {
    let mut a = game("opstage.organization.water_distribution");
    let mut b = a.clone();
    b.state.land.units.get_mut(&AX.into()).unwrap().location = Location::OffMap {
        id: "box_tripoli".into(),
    };
    advance_pair(&mut a, &mut b, Side::Commonwealth);
    assert_eq!(a.state.decisions.pending.len(), 2);
    assert_eq!(b.state.decisions.pending.len(), 2);
    let draw = json!({"allocations":[],"wells":[{"operation":format!("draw|{AX}"),"requested":1,"packing":CargoPacking::default()}]});
    answer_pair(
        &mut a,
        &mut b,
        Side::Axis,
        Value::Null,
        draw,
        Side::Commonwealth,
    );
    assert!(b.state.logistics.drawn_water.is_empty());
    answer_pair(
        &mut a,
        &mut b,
        Side::Commonwealth,
        Value::Null,
        Value::Null,
        Side::Commonwealth,
    );
    advance_pair(&mut a, &mut b, Side::Commonwealth);
    assert_eq!(
        a.state.logistics.water_window.round,
        batches::WaterRound::Allocation
    );
    assert_eq!(
        b.state.logistics.water_window.round,
        batches::WaterRound::Allocation
    );
    assert_eq!(a.state.decisions.pending.len(), 2);
    assert_eq!(b.state.decisions.pending.len(), 2);
    assert_eq!(b.state.logistics.drawn_water[&AX.into()].points, 1);
    answer_pair(
        &mut a,
        &mut b,
        Side::Axis,
        Value::Null,
        Value::Null,
        Side::Commonwealth,
    );
    answer_pair(
        &mut a,
        &mut b,
        Side::Commonwealth,
        Value::Null,
        Value::Null,
        Side::Commonwealth,
    );
    advance_pair(&mut a, &mut b, Side::Commonwealth);
    assert!(b.state.logistics.drawn_water.is_empty());
    assert_eq!(
        b.state.logistics.water_window.round,
        batches::WaterRound::Complete
    );
}
/// Private shortage magnitude does not change the simultaneous mandatory batch schedule.
/// Cases: land:3.6, airlog:51.22
#[test]
fn attrition_batches_hide_shortages_and_apply_only_at_recovered_closure() {
    let mut a = game("opstage.organization.attrition");
    a.state.land.units.get_mut(&AX.into()).unwrap().location = Location::OffMap {
        id: "box_tripoli".into(),
    };
    let mut b = a.clone();
    let r = b.state.logistics.rations.entry(AX.into()).or_default();
    r.finalized_gt = Some(1);
    r.last_short_gt = Some(1);
    r.consecutive_short_gt = 20;
    advance_pair(&mut a, &mut b, Side::Commonwealth);
    assert_eq!(a.state.decisions.pending.len(), 2);
    assert_eq!(b.state.decisions.pending.len(), 2);
    let action = attrition::baseline(content(), &b.state, Side::Axis).unwrap();
    assert!(action.is_array());
    let before = b.state.land.units[&AX.into()].toe.clone();
    answer_pair(
        &mut a,
        &mut b,
        Side::Axis,
        Value::Null,
        action,
        Side::Commonwealth,
    );
    assert_eq!(before, b.state.land.units[&AX.into()].toe);
    answer_pair(
        &mut a,
        &mut b,
        Side::Commonwealth,
        Value::Null,
        Value::Null,
        Side::Commonwealth,
    );
    advance_pair(&mut a, &mut b, Side::Commonwealth);
    assert_ne!(before, b.state.land.units[&AX.into()].toe);
}
/// Dummy status and private coastal quantities cannot determine whether a scheduled window exists.
/// Cases: land:3.6, airlog:55.14, airlog:56.31
#[test]
fn coastal_schedules_are_fixed_across_private_port_domains_and_ship_stocks() {
    let mut a = game("opstage.organization.tactical_shipping");
    ports::initialize(content(), &mut a.state);
    ports::record_entry(
        content(),
        &mut a.state,
        Side::Commonwealth,
        &Location::Hex {
            hex: "C4022".into(),
        },
    );
    a.state.logistics.dumps.insert(
        "private-port-stock".into(),
        Dump {
            id: "private-port-stock".into(),
            marker: "dump-private".into(),
            side: Side::Commonwealth,
            location: DumpLocation::Hex {
                hex: "C4022".into(),
            },
            supplies: cna_content::scenario::Supplies {
                stores: 100,
                ..Default::default()
            },
            active: true,
            dummy: true,
        },
    );
    let mut b = a.clone();
    b.state
        .logistics
        .dumps
        .get_mut("private-port-stock")
        .unwrap()
        .dummy = false;
    advance_pair(&mut a, &mut b, Side::Axis);
    assert_eq!(a.state.decisions.pending.len(), 1);
    assert_eq!(b.state.decisions.pending.len(), 1);
    answer_pair(
        &mut a,
        &mut b,
        Side::Commonwealth,
        Value::Null,
        Value::Null,
        Side::Axis,
    );
    advance_pair(&mut a, &mut b, Side::Axis);
    let mut a = game("opstage.truck_convoy_movement");
    coastal::initialize(content(), &mut a.state).unwrap();
    let mut b = a.clone();
    for ship in b.state.logistics.coastal_ships.values_mut() {
        ship.cargo.stores = 100;
        ship.cp_quarters = 200;
    }
    advance_pair(&mut a, &mut b, Side::Commonwealth);
    assert_eq!(a.state.decisions.pending.len(), 1);
    assert_eq!(b.state.decisions.pending.len(), 1);
    answer_pair(
        &mut a,
        &mut b,
        Side::Axis,
        Value::Null,
        Value::Null,
        Side::Commonwealth,
    );
    advance_pair(&mut a, &mut b, Side::Commonwealth);
}
/// The compulsory allocator is source-complete and conforms to the actual request schema.
/// Cases: airlog:51.22, land:3.6
#[test]
fn mandatory_attrition_baselines_are_accepted_across_32_campaign_seeds() {
    for seed in 0..32 {
        let mut g = game("opstage.organization.attrition");
        g.rng = CampaignRng::from_seed([seed; 32]).state();
        g.state.land.units.get_mut(&AX.into()).unwrap().location = Location::OffMap {
            id: "box_tripoli".into(),
        };
        let r = g.state.logistics.rations.entry(AX.into()).or_default();
        r.finalized_gt = Some(1);
        r.last_short_gt = Some(1);
        r.consecutive_short_gt = 40;
        g = evaluate(&Cna::dev(), content(), &g, &Command::Advance)
            .unwrap()
            .game;
        let initial = toe_strength(content(), &g.state.land.units[&AX.into()])
            .unwrap()
            .get();
        let mut rng = CampaignRng::from_seed([seed; 32]);
        for _ in 0..2 {
            let request = Cna::dev().pending(content(), &g.state).remove(0);
            let p = g.state.decisions.pending[0].clone();
            let action =
                crate::baseline::logistics_orders(content(), &g.state, &request, &mut rng).unwrap();
            g = evaluate(&Cna::dev(), content(), &g, &command(&p, action))
                .unwrap()
                .game;
        }
        g = evaluate(&Cna::dev(), content(), &g, &Command::Advance)
            .unwrap()
            .game;
        assert_eq!(
            toe_strength(content(), &g.state.land.units[&AX.into()])
                .unwrap()
                .get(),
            initial - 2
        );
    }
}

/// Invalid casualty lists cannot probe the opposing hidden shortage groups.
/// Cases: airlog:51.22, land:3.6
#[test]
fn rejected_attrition_lists_use_only_the_submitting_owner_domain() {
    let mut a = game("opstage.organization.attrition");
    a.state.land.units.get_mut(&AX.into()).unwrap().location = Location::OffMap {
        id: "box_tripoli".into(),
    };
    let r = a.state.logistics.rations.entry(AX.into()).or_default();
    r.finalized_gt = Some(1);
    r.last_short_gt = Some(1);
    r.consecutive_short_gt = 20;
    let mut b = a.clone();
    let r = b.state.logistics.rations.entry(CW.into()).or_default();
    r.finalized_gt = Some(1);
    r.last_short_gt = Some(1);
    r.consecutive_short_gt = 40;
    advance_pair(&mut a, &mut b, Side::Axis);
    let p = a
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap()
        .clone();
    let own = json!({"unit":AX,"points":1});
    for action in [
        json!([own.clone(), own]),
        json!([{"unit":CW,"points":1}]),
        json!([{"unit":AX,"points":2}]),
    ] {
        let cmd = command(&p, action.clone());
        crate::testkit::assert_action_indistinguishable(
            &Cna::dev(),
            content(),
            &a,
            &b,
            &cmd,
            Side::Axis,
        );
        assert!(evaluate(&Cna::dev(), content(), &a, &cmd).is_err());
        let errors = [&a, &b].map(|g| {
            let mut state = g.state.clone();
            let before = serde_json::to_value(&state).unwrap();
            let mut rng = CampaignRng::from_state(&g.rng);
            let mut events = vec![];
            let error = attrition::answer(
                content(),
                &mut state,
                &p,
                &action,
                &mut cna_core::engine::Cx {
                    rng: &mut rng,
                    events: &mut events,
                },
            )
            .unwrap_err();
            assert_eq!(before, serde_json::to_value(&state).unwrap());
            assert!(events.is_empty());
            format!("{error:?}")
        });
        assert_eq!(errors[0], errors[1]);
    }
}

/// Full source-data stops occur at the same entry whatever hidden equipment and stocks are held.
/// Cases: airlog:52.42, airlog:50.17, land:3.6
#[test]
fn full_source_preflights_depend_on_public_content_not_hidden_current_inventory() {
    for (anchor, case) in [
        ("opstage.organization.water_distribution", "airlog:52.42"),
        ("opstage.organization.supply_distribution", "airlog:50.17"),
    ] {
        let a = game(anchor);
        let mut b = a.clone();
        for unit in b
            .state
            .land
            .units
            .values_mut()
            .filter(|u| u.side == Side::Axis)
        {
            unit.location = Location::OffMap {
                id: "box_tripoli".into(),
            };
            unit.toe = Some(cna_content::units::Toe::Weapons(vec![
                cna_content::units::WeaponPoints {
                    weapon: "it.cv33".into(),
                    n: 1,
                },
            ]));
        }
        let cmd = Command::Advance;
        crate::testkit::assert_action_indistinguishable(
            &Cna::full(),
            content(),
            &a,
            &b,
            &cmd,
            Side::Commonwealth,
        );
        for g in [&a, &b] {
            let err = evaluate(&Cna::full(), content(), g, &cmd).unwrap_err();
            assert!(format!("{err:?}").contains(case), "{err:?}");
        }
    }
}

/// Cases: airlog:51.11, airlog:53.24, airlog:54.11, airlog:56.34, land:3.6
#[test]
fn accepted_stock_lists_wait_for_joint_closure_and_recover_without_enemy_probes() {
    use cna_content::scenario::Supplies;
    for anchor in [
        "logistics.stores_expenditure",
        "opstage.organization.supply_distribution",
        "opstage.truck_convoy_movement",
    ] {
        let mut a = game(anchor);
        let coastal_order = anchor == "opstage.truck_convoy_movement";
        let hex = if coastal_order { "C4022" } else { "C4020" };
        a.state.land.units.get_mut(&AX.into()).unwrap().location =
            Location::Hex { hex: hex.into() };
        a.state.logistics.dumps.clear();
        for (id, side, location, marker, supplies) in [
            (
                "own-stock",
                Side::Axis,
                DumpLocation::Hex { hex: hex.into() },
                "dump-own",
                Supplies {
                    stores: 100,
                    water: 100,
                    ..Default::default()
                },
            ),
            (
                "enemy-stock",
                Side::Commonwealth,
                DumpLocation::OffMap {
                    id: "box_tripoli".into(),
                },
                "dump-enemy",
                Supplies {
                    stores: 1,
                    ..Default::default()
                },
            ),
        ] {
            a.state.logistics.dumps.insert(
                id.into(),
                Dump {
                    id: id.into(),
                    marker: marker.into(),
                    side,
                    location,
                    supplies,
                    active: true,
                    dummy: false,
                },
            );
        }
        if coastal_order {
            ports::initialize(content(), &mut a.state);
            coastal::initialize(content(), &mut a.state).unwrap();
            ports::record_entry(
                content(),
                &mut a.state,
                Side::Axis,
                &Location::Hex { hex: hex.into() },
            );
            a.state
                .logistics
                .coastal_ships
                .get_mut("axis.coastal.a")
                .unwrap()
                .location = Location::Hex { hex: hex.into() };
        }
        let mut b = a.clone();
        b.state
            .logistics
            .dumps
            .get_mut("enemy-stock")
            .unwrap()
            .supplies
            .stores = 71;
        advance_pair(&mut a, &mut b, Side::Axis);
        let before = a.state.logistics.dumps["own-stock"].supplies;
        let mut holdings_before = serde_json::to_value(&a.state.logistics).unwrap();
        holdings_before
            .as_object_mut()
            .unwrap()
            .remove("allocation_batches");
        let action = match anchor {
            "logistics.stores_expenditure" => {
                json!([{"unit":AX,"stores":20,"half":false,"pasta":true,"draws":[{"source":serde_json::to_string(&SupplySource::Dump("own-stock".into())).unwrap(),"stores":20,"water":1}]}])
            }
            "opstage.organization.supply_distribution" => {
                json!([{"from":serde_json::to_string(&distribution::Endpoint::Dump("own-stock".into())).unwrap(),"to":serde_json::to_string(&distribution::Endpoint::Ground(Location::Hex{hex:hex.into()})).unwrap(),"amount":Supplies { stores:5, ..Default::default() },"packing":CargoPacking::default()}])
            }
            _ => {
                json!([{"ship":"axis.coastal.a","operation":"load","dump":"own-stock","cargo":Supplies { stores:1, ..Default::default() },"path":[]}])
            }
        };
        let p = a
            .state
            .decisions
            .pending
            .iter()
            .find(|p| p.seat.side == Side::Axis)
            .unwrap();
        let bad = command(p, json!([{"unit":"foreign"}]));
        let saved = serde_json::to_value(&a).unwrap();
        crate::testkit::assert_action_indistinguishable(
            &Cna::dev(),
            content(),
            &a,
            &b,
            &bad,
            Side::Axis,
        );
        assert!(evaluate(&Cna::dev(), content(), &a, &bad).is_err());
        assert_eq!(serde_json::to_value(&a).unwrap(), saved);
        answer_pair(
            &mut a,
            &mut b,
            Side::Axis,
            action.clone(),
            action,
            Side::Axis,
        );
        let mut holdings_after = serde_json::to_value(&a.state.logistics).unwrap();
        holdings_after
            .as_object_mut()
            .unwrap()
            .remove("allocation_batches");
        assert_eq!(holdings_before, holdings_after);
        assert_eq!(a.state.logistics.dumps["own-stock"].supplies, before);
        assert_eq!(b.state.logistics.dumps["own-stock"].supplies, before);
        if !coastal_order {
            answer_pair(
                &mut a,
                &mut b,
                Side::Commonwealth,
                Value::Null,
                Value::Null,
                Side::Axis,
            );
        }
        assert_eq!(a.state.logistics.dumps["own-stock"].supplies, before);
        let closed_cursor = a.state.cursor.clone();
        advance_pair(&mut a, &mut b, Side::Axis);
        let spent = match anchor {
            "logistics.stores_expenditure" => 20,
            "opstage.organization.supply_distribution" => 5,
            _ => 1,
        };
        assert_eq!(
            a.state.logistics.dumps["own-stock"].supplies.stores,
            before.stores - spent
        );
        if anchor == "logistics.stores_expenditure" {
            assert_eq!(
                a.state.logistics.dumps["own-stock"].supplies.water,
                before.water - 1
            );
            let h = &a.state.logistics.rations[&AX.into()];
            assert_eq!(h.finalized_gt, Some(1));
            assert_eq!(h.pasta_gt, Some(1));
        }
        // Repeated finishing at the saved anchor cannot debit the accepted list again.
        let mut finished = a.state.clone();
        finished.cursor = closed_cursor;
        finished.decisions.pending.clear();
        let stock = finished.logistics.dumps["own-stock"].supplies;
        let mut rng = CampaignRng::from_state(&a.rng);
        let mut cx = cna_core::engine::Cx {
            rng: &mut rng,
            events: &mut vec![],
        };
        match anchor {
            "logistics.stores_expenditure" => {
                batches::finish_stores(content(), &mut finished, &mut cx).unwrap()
            }
            "opstage.organization.supply_distribution" => {
                batches::finish_distribution(content(), &mut finished, &mut cx).unwrap()
            }
            _ => coastal::finish(content(), &mut finished, &mut cx).unwrap(),
        }
        assert_eq!(finished.logistics.dumps["own-stock"].supplies, stock);
        let mut legacy = serde_json::to_value(&finished).unwrap();
        legacy["logistics"]
            .as_object_mut()
            .unwrap()
            .remove("allocation_batches");
        let mut legacy: State = serde_json::from_value(legacy).unwrap();
        match anchor {
            "logistics.stores_expenditure" => {
                batches::finish_stores(content(), &mut legacy, &mut cx).unwrap()
            }
            "opstage.organization.supply_distribution" => {
                batches::finish_distribution(content(), &mut legacy, &mut cx).unwrap()
            }
            _ => coastal::finish(content(), &mut legacy, &mut cx).unwrap(),
        }
        assert_eq!(legacy.logistics.dumps["own-stock"].supplies, stock);
    }
}
