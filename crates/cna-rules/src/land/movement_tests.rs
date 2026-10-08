#![cfg(test)]

use super::*;
use crate::{
    Cna,
    seq::{Block, Half},
    state::UnitSupply,
};
use cna_content::map::{LineKind, MapContent, SideKind};
use cna_core::{
    decision::DecisionResponse,
    dice::CampaignRng,
    engine::{Command, Game, Progress, Ruleset, evaluate},
    quantity::{FuelTenths, WaterPoints},
    visibility::Perspective,
};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
};
const TANK: &str = "it.libyan_tank_command.xxi_l_tank_bn";
const LEG: &str = "cw.unassigned_inf.1st_rnf_mg_bn";
struct Overlay {
    dir: PathBuf,
}
impl Overlay {
    fn new(
        content: &mut CnaContent,
        route: Option<&str>,
        desert: bool,
        side: Option<&str>,
    ) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cna-movement-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        let source = cna_content::repo_data_dir().join("map");
        for name in ["hexes.csv", "aliases.csv", "sections.toml", "layers.toml"] {
            std::fs::copy(source.join(name), dir.join(name)).unwrap();
        }
        // A test overlay on the real coordinate grid; no publication movement data are inferred.
        let cells: BTreeSet<_> = (0..=38).map(|n| HexId::new(format!("C40{n:02}"))).collect();
        let mut rows = String::from("hex_id,section,q,r,terrain,flags\n");
        for h in content.map.iter() {
            let terrain = if cells.contains(&h.id) {
                if desert { "desert" } else { "clear" }
            } else {
                h.terrain.as_deref().unwrap_or("unclassified")
            };
            rows += &format!(
                "{},{},{},{},{},{}\n",
                h.id,
                h.section,
                h.axial.q,
                h.axial.r,
                terrain,
                h.flags.join("|")
            );
        }
        std::fs::write(dir.join("hexes.csv"), rows).unwrap();
        let mut coverage = String::from("layer,hex_id,neighbour_id,src,review_batch\n");
        let mut lines = String::from("from_hex,to_hex,kind,src,review_batch\n");
        let mut sides =
            String::from("hex_id,direction,neighbour_id,feature,high_side,src,review_batch\n");
        let mut edges = BTreeSet::new();
        for hex in &cells {
            if content.map.get(hex).is_none() {
                continue;
            }
            coverage += &format!("terrain,{hex},,land:8.37,test\n");
            for h in content.map.neighbors(hex) {
                if !cells.contains(&h.id) {
                    continue;
                }
                let pair = if hex < &h.id {
                    (hex.clone(), h.id.clone())
                } else {
                    (h.id.clone(), hex.clone())
                };
                edges.insert(pair);
            }
        }
        for (a, b) in edges {
            for k in LineKind::ALL {
                coverage += &format!("line:{},{a},{b},land:8.33,test\n", k.name());
            }
            for k in SideKind::ALL {
                coverage += &format!("side:{},{a},{b},land:8.35,test\n", k.name());
            }
            if let Some(route) = route {
                lines += &format!("{a},{b},{route},land:8.33,test\n");
            }
            if let Some(side) = side {
                sides += &format!("{a},E,{b},{side},{b},land:8.35,test\n");
            }
        }
        std::fs::write(dir.join("coverage.csv"), coverage).unwrap();
        std::fs::write(dir.join("line_features.csv"), lines).unwrap();
        std::fs::write(dir.join("hexsides.csv"), sides).unwrap();
        content.map = MapContent::load(&dir).unwrap();
        Self { dir }
    }
}
impl Drop for Overlay {
    fn drop(&mut self) {
        assert!(self.dir.starts_with(std::env::temp_dir()));
        assert!(
            self.dir
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("cna-movement-test-")
        );
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}
fn setup(
    id: &str,
    route: Option<&str>,
    desert: bool,
    side: Option<&str>,
) -> (CnaContent, State, Overlay) {
    let mut c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut s = State::new(&c).unwrap();
    let overlay = Overlay::new(&mut c, route, desert, side);
    for u in s.land.units.values_mut() {
        u.location = Location::Eliminated;
    }
    place(&mut s, id, "C4020");
    let owner = s.land.units[&id.into()].side;
    s.turn.weather = Some(crate::state::WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    s.turn.player_a = Some(owner);
    s.cursor.block = Block::PlayerHalf;
    s.cursor.half = Some(Half::A);
    s.cursor.op_stage = Some(1);
    s.cursor.index = 1;
    s.cursor.entered = false;
    (c, s, overlay)
}
fn place(s: &mut State, id: &str, hex: &str) {
    let id = UnitId::new(id);
    let u = s.land.units.get_mut(&id).unwrap();
    u.location = Location::Hex { hex: hex.into() };
    u.detached = true;
    u.attached_to = None;
    s.logistics.unit_supply.insert(
        id.clone(),
        UnitSupply {
            activity_water: WaterPoints::new(10000),
            ready_ammo: cna_core::quantity::AmmoPoints::new(10000),
            tank_fuel: FuelTenths::new(10000),
            ..UnitSupply::default()
        },
    );
    // Terrain/CP fixtures begin fully supplied; shortage tests explicitly remove the relevant stock.
    s.logistics.rations.insert(
        id,
        logistics::Rations {
            water_stage: Some(logistics::water::WaterStage::current(s)),
            infantry_water_received: 2,
            issued_gt: Some(s.cursor.game_turn),
            pasta_gt: Some(s.cursor.game_turn),
            ..logistics::Rations::default()
        },
    );
}
fn start(c: &CnaContent, s: State, strict: bool) -> Game<Cna> {
    evaluate(
        &Cna { strict },
        c,
        &Game {
            state: s,
            rng: CampaignRng::from_seed([3; 32]).state(),
        },
        &Command::Advance,
    )
    .unwrap()
    .game
}
fn respond(
    c: &CnaContent,
    g: &Game<Cna>,
    seat: SeatId,
    action: Value,
    strict: bool,
) -> Result<cna_core::engine::Transition<Cna>, Rejection> {
    let p = g
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat == seat)
        .unwrap();
    evaluate(
        &Cna { strict },
        c,
        g,
        &Command::Respond(DecisionResponse {
            decision_id: p.id.clone(),
            seat,
            controller_epoch: 1,
            decision_revision: p.revision,
            idempotency_key: "test".into(),
            action,
            public_explanation: None,
        }),
    )
}
// Fixed empty reaction roles are real private passes; these older movement assertions
// inspect the subsequent window, after the engine's local forced-pass controller acts.
fn decline_empty_reactions(
    c: &CnaContent,
    mut t: cna_core::engine::Transition<Cna>,
    strict: bool,
) -> cna_core::engine::Transition<Cna> {
    while let Some(p) = t
        .game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| {
            p.kind == super::super::reaction::KIND
                && p.space
                    .context
                    .as_ref()
                    .is_some_and(|v| v["forced_pass"] == true)
        })
        .cloned()
    {
        let next = evaluate(
            &Cna { strict },
            c,
            &t.game,
            &Command::Respond(DecisionResponse {
                decision_id: p.id,
                seat: p.seat,
                controller_epoch: 1,
                decision_revision: p.revision,
                idempotency_key: "forced-reaction-test".into(),
                action: Value::Null,
                public_explanation: None,
            }),
        )
        .unwrap();
        t.events.extend(next.events);
        t.game = next.game;
        t.progress = next.progress;
    }
    t
}
fn seat(g: &Game<Cna>) -> SeatId {
    g.state.decisions.pending[0].seat
}
/// Cases: land:8.11, land:8.31, land:8.33, land:8.37, land:8.46, airlog:49.13, land:19.44
/// Interpretations: interp:airlog-0001, interp:land-0002
#[test]
fn real_motor_and_leg_units_pay_hand_checked_road_track_and_desert_costs() {
    for (id, route, desert, quarters, fuel) in [
        (TANK, Some("road"), false, 4, 18),
        (TANK, Some("track"), false, 8, 36),
        (TANK, None, true, 32, 180),
        (LEG, Some("road"), false, 8, 0),
        (LEG, Some("track"), false, 8, 0),
        (LEG, None, true, 24, 0),
    ] {
        let (c, s, _overlay) = setup(id, route, desert, None);
        let g = start(&c, s, true);
        let t = respond(
            &c,
            &g,
            seat(&g),
            json!([{"unit":id,"path":["C4021","C4022"]}]),
            true,
        )
        .unwrap();
        let u = &t.game.state.land.units[&id.into()];
        assert_eq!(u.cp_spent_quarters, quarters, "{id}/{route:?}");
        assert_eq!(
            t.game.state.logistics.unit_supply[&id.into()]
                .tank_fuel
                .get(),
            10000 - fuel
        );
        assert_eq!(u.location.hex(), Some(&"C4022".into()));
        assert!(t.game.state.decisions.pending.is_empty());
    }
}
/// Cases: land:8.43, land:8.46
/// Interpretations: interp:land-0002
#[test]
fn verified_track_halves_slope_price_in_direction_of_crossing() {
    let (c, s, _o) = setup(TANK, Some("track"), false, Some("slope"));
    let g = start(&c, s, true);
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":TANK,"path":["C4021"]}]),
        true,
    )
    .unwrap();
    // One CP track entry plus half the four-CP vehicle uphill slope addition.
    assert_eq!(t.game.state.land.units[&TANK.into()].cp_spent_quarters, 12);
}
/// Cases: land:6.22, land:6.26, land:8.17, land:8.13
#[test]
fn exceeding_cpa_costs_cohesion_and_overlong_or_disconnected_orders_are_atomic() {
    let (c, mut s, _o) = setup(LEG, None, true, None);
    s.land.units.get_mut(&LEG.into()).unwrap().cp_spent_quarters = 40;
    let g = start(&c, s, true);
    let before = serde_json::to_value(&g).unwrap();
    assert!(
        respond(
            &c,
            &g,
            seat(&g),
            json!([{"unit":LEG,"path":["C4021","C4022","C4023","C4024","C4025","C4026"]}]),
            true
        )
        .is_err()
    );
    assert!(
        respond(
            &c,
            &g,
            seat(&g),
            json!([{"unit":LEG,"path":["C4022"]}]),
            true
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&g).unwrap(), before);
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":LEG,"path":["C4021"]}]),
        true,
    )
    .unwrap();
    assert_eq!(t.game.state.land.units[&LEG.into()].cohesion_quarters, -12);
}
/// Cases: land:9.31, land:9.32, land:9.33
#[test]
fn terrain_end_limit_rejects_whole_list_and_vehicle_congestion_uses_plain_cost() {
    let (c, mut s, _o) = setup(TANK, Some("road"), false, None);
    let others: Vec<_> = s
        .units_of(Side::Axis)
        .filter(|u| {
            u.id.as_str() != TANK
                && c.units.units[&u.id].stacking_points == Some(1)
                && formation::combat_unit(&c, &u.id)
        })
        .take(6)
        .map(|u| u.id.to_string())
        .collect();
    for id in &others {
        place(&mut s, id, "C4021");
    }
    let g = start(&c, s, true);
    assert!(
        respond(
            &c,
            &g,
            seat(&g),
            json!([{"unit":TANK,"path":["C4021"]}]),
            true
        )
        .is_err()
    );
    let mut s = g.state.clone();
    s.land
        .units
        .get_mut(&others[5].as_str().into())
        .unwrap()
        .location = Location::Eliminated;
    let g = Game {
        state: s,
        rng: g.rng.clone(),
    };
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":TANK,"path":["C4021","C4022"]}]),
        true,
    )
    .unwrap();
    // Bypass five occupied road points through clear terrain: 2 CP; next road hex 0.5 CP.
    assert_eq!(t.game.state.land.units[&TANK.into()].cp_spent_quarters, 10);
}
/// Cases: land:10.11, land:10.15, land:10.6, land:10.23, land:10.24, land:10.29
#[test]
fn undisclosed_zone_stops_on_entry_without_hypothetical_strength_leak() {
    let (c, mut s, _o) = setup(LEG, None, false, None);
    place(&mut s, TANK, "C4023");
    place(&mut s, "it.libyan_tank_command.lxii_l_tank_bn", "C4023");
    assert!(zoc::controlled(&c, &s, Side::Axis, &"C4022".into(), true).unwrap());
    let g = start(&c, s, false);
    let own = seat(&g);
    let r = Cna::dev()
        .inspect(&c, &g.state, Perspective::Seat(own), LEG)
        .unwrap();
    let mut weak = g.state.clone();
    weak.land
        .units
        .get_mut(&TANK.into())
        .unwrap()
        .cohesion_quarters = -104;
    weak.land
        .units
        .get_mut(&"it.libyan_tank_command.lxii_l_tank_bn".into())
        .unwrap()
        .cohesion_quarters = -104;
    assert_eq!(
        r,
        Cna::dev()
            .inspect(&c, &weak, Perspective::Seat(own), LEG)
            .unwrap()
    );
    let t = respond(
        &c,
        &g,
        own,
        json!([{"unit":LEG,"path":["C4021","C4022","C4021","C4020"]}]),
        false,
    )
    .unwrap();
    assert_eq!(
        t.game.state.land.units[&LEG.into()].location.hex(),
        Some(&"C4022".into())
    );
    assert_eq!(t.game.state.land.units[&LEG.into()].cp_spent_quarters, 16);
}
/// Cases: land:3.61, land:3.62, land:19.44
#[test]
fn complete_move_reopens_for_other_units_and_enemy_receives_only_faces() {
    let (c, mut s, _o) = setup(TANK, Some("road"), false, None);
    let other = "it.libyan_tank_command.lxii_l_tank_bn";
    place(&mut s, other, "C4020");
    let g = start(&c, s, true);
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":TANK,"path":["C4021"]}]),
        true,
    )
    .unwrap();
    assert_eq!(t.game.state.decisions.pending.len(), 1);
    assert!(
        respond(
            &c,
            &t.game,
            seat(&t.game),
            json!([{"unit":TANK,"path":["C4022"]}]),
            true
        )
        .is_err()
    );
    let enemy = Perspective::Side(Side::Commonwealth);
    let observed: Vec<_> = t
        .events
        .iter()
        .filter(|e| enemy.can_see(&e.audience))
        .collect();
    // Rules as written, the enemy watches the counter move by its printed face (land:3.62).
    for e in &observed {
        match &e.event {
            GameEvent::StackUpdated { stack } => {
                assert_eq!(stack.visible_count, Some(stack.unit_ids.len() as u32))
            }
            GameEvent::UnitUpdated { unit } => {
                crate::testkit::assert_face(&serde_json::to_value(unit).unwrap())
            }
            GameEvent::StackRemoved { .. } | GameEvent::UnitMoved { .. } => {}
            other => panic!("the enemy saw {other:?}"),
        }
    }
    assert!(!observed.is_empty());
    crate::testkit::assert_face_only(&Cna::dev().inspect(&c, &t.game.state, enemy, TANK));
    let t = respond(&c, &t.game, seat(&t.game), Value::Null, true).unwrap();
    assert!(t.game.state.decisions.pending.is_empty());
}
/// Cases: land:8.37, airlog:49.12, airlog:52.42
/// Interpretations: interp:units-0005
#[test]
fn unknown_surface_and_numeric_hq_water_gaps_follow_profiles() {
    let (mut c, s, _o) = setup(LEG, None, false, None);
    let original = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    c.map = original.map;
    let mut s = s;
    place(&mut s, LEG, "C1020");
    assert!(matches!(
        c.map.terrain_survey(&"C1021".into()),
        cna_content::map::Survey::Unknown
    ));
    let g = start(&c, s, false);
    let e = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":LEG,"path":["C1021"]}]),
        false,
    )
    .expect_err("expected rejection");
    assert!(e.to_string().contains("terrain not yet digitized"));
    let (c, mut s, _o) = setup(TANK, Some("road"), false, None);
    let actual = s
        .land
        .units
        .keys()
        .find(|id| {
            formation::class(&c, id).is_some_and(|cl| {
                cl.unit_type == "headquarters" && !cl.max_toe_paren && cl.code == "e"
            })
        })
        .unwrap()
        .clone();
    s.land.units.get_mut(&TANK.into()).unwrap().location = Location::Eliminated;
    place(&mut s, actual.as_str(), "C4020");
    s.turn.player_a = Some(Side::Commonwealth);
    let g = start(&c, s, false);
    let action = json!([{"unit":actual,"path":["C4021"]}]);
    assert!(
        respond(&c, &g, seat(&g), action.clone(), false)
            .expect_err("expected rejection")
            .to_string()
            .contains("airlog:52.42")
    );
    assert!(
        matches!(respond(&c,&g,seat(&g),action,true),Err(Rejection::Engine(EngineError::Unsupported {case,..})) if case=="airlog:52.42")
    );
}
/// Cases: land:8.11, land:19.44
#[test]
fn dev_campaign_finishes_with_scripted_real_unit_moves() {
    let (c, s, _o) = setup(LEG, Some("road"), false, None);
    let mut g: Game<Cna> = Game {
        state: s,
        rng: CampaignRng::from_seed([8; 32]).state(),
    };
    // This synthetic world is already past setup; it carries no unclosed choices.
    assert!(g.state.setup.tasks.is_empty());
    assert!(
        g.state
            .decisions
            .pending
            .iter()
            .all(|pending| !pending.kind.starts_with("cna.setup."))
    );
    g.state.setup.closed = true;
    crate::air::inventory::initialize(&c, &mut g.state).unwrap();
    // This synthetic fixture starts mid-half. Prepare its empty pre-game convoy plans
    // through the real logistics API before running movement, preserving its actual RNG.
    let mut fixture_rng = CampaignRng::from_state(&g.rng);
    let mut fixture_events = vec![];
    let mut cx = Cx {
        rng: &mut fixture_rng,
        events: &mut fixture_events,
    };
    crate::logistics::convoys::initialize(&c, &mut g.state, false, &mut cx).unwrap();
    while let Some(pos) = g
        .state
        .decisions
        .pending
        .iter()
        .position(|p| p.kind.starts_with(crate::logistics::convoys::PREFIX))
    {
        let pending = g.state.decisions.pending.remove(pos);
        crate::logistics::convoys::answer(&c, &mut g.state, &pending, &Value::Null, &mut cx)
            .unwrap();
    }
    g.rng = fixture_rng.state();
    let rules = Cna::dev();
    let mut moves = 0;
    // Fixed combat role windows add answers while preserving the same bounded fixture.
    for n in 0..2000 {
        let t = evaluate(&rules, &c, &g, &Command::Advance).unwrap();
        g = t.game;
        if matches!(t.progress, Some(Progress::Finished { .. })) {
            assert!(moves > 10);
            eprintln!("synthetic movement campaign finished after {n} answers");
            return;
        }
        let p = rules.pending(&c, &g.state).remove(0);
        let action = if p.kind == KIND && eligible(&c, &g.state, &LEG.into(), p.seat) {
            let hex = g.state.land.units[&LEG.into()].location.hex().unwrap();
            let to = if hex.as_str() == "C4020" {
                "C4021"
            } else {
                "C4020"
            };
            moves += 1;
            json!([{"unit":LEG,"path":[to]}])
        } else if p.kind == logistics::truck_convoy::KIND {
            // The fixed convoy batch declares an explicit empty order list as its pass.
            json!([])
        } else if p.kind == "cna.arrivals.batch" {
            crate::baseline::arrival_orders(&c, &g.state, &p)
                .expect("mandatory arrival batch has a source-conserving plan")
        } else if p.space.pass.is_some()
            && matches!(&p.space.schema, ActionSchema::Choice { options } if options.is_empty())
        {
            Value::Null
        } else if let ActionSchema::Choice { options } = &p.space.schema {
            json!(options[0].id)
        } else {
            Value::Null
        };
        g = evaluate(
            &rules,
            &c,
            &g,
            &Command::Respond(DecisionResponse {
                decision_id: p.id,
                seat: p.seat,
                controller_epoch: 1,
                decision_revision: p.revision,
                idempotency_key: format!("m{n}"),
                action,
                public_explanation: None,
            }),
        )
        .unwrap()
        .game;
    }
    panic!("campaign did not finish at {:?}", g.state.cursor);
}

/// Cases: land:8.15, land:8.24, land:8.65, land:10.24, land:10.26
#[test]
fn known_contact_requires_break_cost_and_forbids_direct_controlled_to_controlled_movement() {
    let (c, mut s, _o) = setup(LEG, None, false, None);
    place(&mut s, TANK, "C4019");
    place(&mut s, "it.libyan_tank_command.lxii_l_tank_bn", "C4019");
    let g = start(&c, s, false);
    assert_eq!(
        g.state.land.movement.controls.get(&"C4020".into()),
        Some(&true)
    );
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":LEG,"path":["C4021"]}]),
        false,
    )
    .unwrap();
    assert_eq!(t.game.state.land.units[&LEG.into()].cp_spent_quarters, 16); // two to break, two clear entry
    let mut g = g;
    g.state.land.movement.controls.insert("C4021".into(), true);
    assert!(
        respond(
            &c,
            &g,
            seat(&g),
            json!([{"unit":LEG,"path":["C4021"]}]),
            false
        )
        .is_err()
    );
    let companion = "cw.unassigned_inf.3rd_coldstream_guards";
    place(&mut g.state, companion, "C4021");
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":LEG,"path":["C4021"]}]),
        false,
    )
    .unwrap();
    assert_eq!(
        t.game.state.land.units[&LEG.into()].location.hex(),
        Some(&"C4021".into())
    );
}
/// Cases: land:10.23, land:10.29, land:19.44
#[test]
fn noncombat_unit_stops_before_undisclosed_enemy_control() {
    let (c, mut s, _o) = setup(LEG, None, false, None);
    let actual = s
        .land
        .units
        .keys()
        .find(|id| {
            formation::class(&c, id).is_some_and(|cl| cl.unit_type == "engineer" && cl.cpa > 0)
                && s.land.units[*id].side == Side::Commonwealth
        })
        .unwrap()
        .clone();
    s.land.units.get_mut(&LEG.into()).unwrap().location = Location::Eliminated;
    place(&mut s, actual.as_str(), "C4020");
    place(&mut s, TANK, "C4023");
    place(&mut s, "it.libyan_tank_command.lxii_l_tank_bn", "C4023");
    let g = start(&c, s, false);
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":actual,"path":["C4021","C4022","C4021","C4020"]}]),
        false,
    )
    .unwrap();
    assert_eq!(
        t.game.state.land.units[&actual].location.hex(),
        Some(&"C4021".into())
    );
    assert_eq!(t.game.state.land.units[&actual].cp_spent_quarters, 8);
}
/// Cases: land:19.43, land:19.44, land:9.21
#[test]
fn stack_orders_move_all_counters_and_detachment_updates_parent_cp() {
    let (c, mut s, _o) = setup(TANK, Some("road"), false, None);
    let other = "it.libyan_tank_command.lxii_l_tank_bn";
    place(&mut s, other, "C4020");
    let g = start(&c, s, true);
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":TANK,"with_stack":true,"path":["C4021"]}]),
        true,
    )
    .unwrap();
    assert_eq!(
        t.game.state.land.units[&other.into()].location.hex(),
        Some(&"C4021".into())
    );
    assert!(t.game.state.land.movement.moved.contains(&other.into()));
    let parent = "it.libyan_tank_command.aresca_1st_regt_hq";
    let mut s = g.state.clone();
    place(&mut s, parent, "C4020");
    let u = s.land.units.get_mut(&TANK.into()).unwrap();
    u.detached = false;
    u.attached_to = None;
    let g = Game {
        state: s,
        rng: g.rng.clone(),
    };
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":TANK,"path":["C4021"]}]),
        true,
    )
    .unwrap();
    assert_eq!(t.game.state.land.units[&parent.into()].cp_spent_quarters, 4);
    assert_eq!(t.game.state.land.units[&TANK.into()].cp_spent_quarters, 6);
    assert!(t.game.state.land.units[&TANK.into()].detached);
    assert!(
        t.events
            .iter()
            .any(|e| matches!(&e.event,GameEvent::UnitUpdated {unit} if unit.id==parent))
    );
}
/// Cases: land:8.13, land:19.44, airlog:49.13
#[test]
fn invalid_second_order_or_fuel_shortage_rejects_every_move() {
    let (c, mut s, _o) = setup(TANK, Some("road"), false, None);
    let other = "it.libyan_tank_command.lxii_l_tank_bn";
    place(&mut s, other, "C4020");
    let g = start(&c, s, true);
    let before = serde_json::to_value(&g).unwrap();
    assert!(
        respond(
            &c,
            &g,
            seat(&g),
            json!([{"unit":TANK,"path":["C4021"]},{"unit":other,"path":["C4022"]}]),
            true
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&g).unwrap(), before);
    let mut g = g;
    g.state
        .logistics
        .unit_supply
        .get_mut(&TANK.into())
        .unwrap()
        .tank_fuel = FuelTenths::ZERO;
    assert!(
        respond(
            &c,
            &g,
            seat(&g),
            json!([{"unit":TANK,"path":["C4021"]}]),
            true
        )
        .is_err()
    );
    assert_eq!(g.state.land.units[&TANK.into()].cp_spent_quarters, 0);
}

/// Cases: land:29.44, land:29.51, land:29.56
#[test]
fn local_storm_scope_changes_road_prices_only_in_affected_sections() {
    use cna_tables::land::weather::MapSection;
    for (kind, section, cost) in [
        (WeatherKind::Sandstorm, MapSection::C, 4),
        (WeatherKind::Sandstorm, MapSection::D, 2),
        (WeatherKind::Rainstorm, MapSection::C, 4),
        (WeatherKind::Rainstorm, MapSection::D, 2),
    ] {
        let (c, mut s, _o) = setup(TANK, Some("road"), false, None);
        s.turn.weather = Some(crate::state::WeatherState {
            kind,
            storm_sections: vec![section],
        });
        let g = start(&c, s, true);
        let t = respond(
            &c,
            &g,
            seat(&g),
            json!([{"unit":TANK,"path":["C4021"]}]),
            true,
        )
        .unwrap();
        assert_eq!(
            t.game.state.land.units[&TANK.into()].cp_spent_quarters,
            cost
        );
    }
}

/// Cases: land:8.11, land:8.13, land:19.44
#[test]
fn randomized_baseline_orders_are_accepted_across_many_seeds_in_both_profiles() {
    for strict in [false, true] {
        let (c, mut s, _o) = setup(TANK, Some("road"), false, None);
        let other = "it.libyan_tank_command.lxii_l_tank_bn";
        place(&mut s, other, "C4020");
        let g = start(&c, s, strict);
        let request = Cna { strict }.pending(&c, &g.state)[0].clone();
        let mut destinations = BTreeSet::new();
        for seed in 0..48u8 {
            let mut rng = CampaignRng::from_seed([seed; 32]);
            let action = crate::baseline::random_orders(&c, &g.state, &request, &mut rng);
            assert!(!action.as_array().unwrap().is_empty());
            destinations.insert(action[0]["path"].clone().to_string());
            respond(&c, &g, request.seat, action, strict).unwrap();
        }
        assert!(destinations.len() > 3);
        let mut unmoving = g.state.clone();
        unmoving
            .land
            .movement
            .moved
            .extend([TANK.into(), other.into()]);
        let action = crate::baseline::random_orders(
            &c,
            &unmoving,
            &request,
            &mut CampaignRng::from_seed([1; 32]),
        );
        assert_eq!(action, json!([]));
    }
}
/// Cases: land:9.31, land:9.32
#[test]
fn inspect_includes_a_legal_destination_beyond_an_overfull_transit_hex() {
    let (c, mut s, _o) = setup(LEG, Some("road"), false, None);
    let blockers: Vec<_> = s
        .units_of(Side::Commonwealth)
        .filter(|u| {
            u.id.as_str() != LEG
                && c.units.units[&u.id].stacking_points == Some(1)
                && formation::combat_unit(&c, &u.id)
        })
        .take(6)
        .map(|u| u.id.to_string())
        .collect();
    assert_eq!(blockers.len(), 6);
    for id in blockers {
        place(&mut s, &id, "C4021");
    }
    let g = start(&c, s, true);
    let paths = reachable(&c, &g.state, &LEG.into(), true);
    assert!(!paths.iter().any(|r| r.hex.as_str() == "C4021"));
    let path = paths.iter().find(|r| r.hex.as_str() == "C4022").unwrap();
    assert_eq!(path.cp_quarters, 8);
    respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":LEG,"path":path.path}]),
        true,
    )
    .unwrap();
}
/// Cases: land:8.11, land:8.13, land:19.44
#[test]
#[ignore = "slow: benchmark"]
fn profile_real_roster_movement() {
    use std::time::Instant;
    let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut s = State::new(&c).unwrap();
    s.turn.weather = Some(crate::state::WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    s.turn.player_a = Some(Side::Axis);
    s.cursor.block = Block::PlayerHalf;
    s.cursor.half = Some(Half::A);
    s.cursor.op_stage = Some(1);
    s.cursor.index = 1;
    let owner = SeatId::new(Side::Axis, Role::FrontLine);
    let t = Instant::now();
    for _ in 0..20 {
        std::hint::black_box(available(&c, &s, owner));
    }
    eprintln!(
        "available 20 windows: {:?}; roster {}",
        t.elapsed(),
        s.land.units.len()
    );
    let g = start(&c, s, false);
    let request = Cna::dev()
        .pending(&c, &g.state)
        .into_iter()
        .find(|p| p.seat == owner)
        .unwrap();
    for seed in 0..3 {
        let t = Instant::now();
        let action = crate::baseline::random_orders(
            &c,
            &g.state,
            &request,
            &mut CampaignRng::from_seed([seed; 32]),
        );
        eprintln!(
            "baseline seed{seed}: {:?}, {} orders",
            t.elapsed(),
            action.as_array().unwrap().len()
        );
    }
}
/// Cases: land:8.11, land:8.13, land:19.44
#[test]
#[ignore = "slow: benchmark"]
fn profile_movers_full_graziani_campaign() {
    // A timing test: production never builds the two boards that verify a board-sync skip, and
    // every other test still verifies them.
    let _timing = crate::view::BoardSkipVerificationOff::new();
    real_roster_movers(10000, true);
}
/// Cases: land:8.11, land:8.13, land:19.44
#[test]
fn real_roster_baseline_executes_a_bounded_movement_window() {
    real_roster_movers(40, false);
}
fn real_roster_movers(answer_limit: usize, must_finish: bool) {
    let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut g: Game<Cna> = Game {
        state: State::new(&c).unwrap(),
        rng: CampaignRng::from_seed([8; 32]).state(),
    };
    let rules = Cna::dev();
    let mut rng = CampaignRng::from_seed([19; 32]);
    let begin = std::time::Instant::now();
    let mut moves = 0;
    let mut after_setup = 0;
    for n in 0..answer_limit + 4096 {
        if after_setup == answer_limit {
            break;
        }
        if n % 100 == 0 {
            eprintln!(
                "answer {n}, GT{}, Op{:?}, {:?}, {} moves",
                g.state.cursor.game_turn,
                g.state.cursor.op_stage,
                begin.elapsed(),
                moves
            );
        }
        assert!(
            begin.elapsed().as_secs() < if must_finish { 180 } else { 60 },
            "movement campaign exceeded its bounded test budget"
        );
        let t = evaluate(&rules, &c, &g, &Command::Advance).unwrap();
        g = t.game;
        if matches!(t.progress, Some(Progress::Finished { .. })) {
            eprintln!(
                "full Graziani: {:?}, {n} answers, {moves} movement orders",
                begin.elapsed()
            );
            assert!(moves > 0);
            return;
        }
        let request = rules.pending(&c, &g.state).remove(0);
        if !request.kind.starts_with("cna.setup.") {
            after_setup += 1;
        }
        let action = profiling_answer(&c, &g.state, &request, &mut rng);
        if request.kind == KIND {
            moves += action.as_array().unwrap().len();
        }
        g = evaluate(
            &rules,
            &c,
            &g,
            &Command::Respond(DecisionResponse {
                decision_id: request.id,
                seat: request.seat,
                controller_epoch: 1,
                decision_revision: request.revision,
                idempotency_key: format!("profile{n}"),
                action,
                public_explanation: None,
            }),
        )
        .unwrap()
        .game;
    }
    assert!(
        moves > 0,
        "bounded campaign must execute a real movement order"
    );
    assert_eq!(after_setup, answer_limit);
    assert!(!must_finish, "profile campaign did not finish");
}
fn profiling_answer(
    c: &CnaContent,
    state: &State,
    request: &cna_core::decision::DecisionRequest,
    rng: &mut CampaignRng,
) -> Value {
    if matches!(
        request.kind.as_str(),
        KIND | super::super::reaction::KIND | super::super::reaction::CONTINUE
    ) {
        // A mandatory continuation needs a legal remaining path, not a schema placeholder.
        crate::baseline::random_orders(c, state, request, rng)
    } else if request.kind == logistics::truck_convoy::KIND {
        // The fixed convoy batch declares an explicit empty order list as its pass.
        json!([])
    } else if request.kind == logistics::attrition::KIND {
        crate::baseline::logistics_orders(c, state, request, rng)
            .expect("mandatory attrition baseline must allocate every casualty group")
    } else if request.kind == super::super::breakdown::window::KIND {
        let action = crate::baseline::random_breakdown(c, state, request, rng);
        assert!(
            !action.is_null(),
            "mandatory breakdown baseline must preserve every holding"
        );
        action
    } else if request.kind == "cna.arrivals.batch" {
        crate::baseline::arrival_orders(c, state, request)
            .expect("mandatory arrival batch has a source-conserving plan")
    } else if request.space.pass.is_some()
        && matches!(&request.space.schema, ActionSchema::Choice { options } if options.is_empty())
    {
        Value::Null
    } else if matches!(request.space.schema, ActionSchema::Choice { .. })
        || request.space.pass.is_none()
    {
        mandatory_profile_answer(&request.space.schema)
    } else {
        Value::Null
    }
}
fn mandatory_profile_answer(schema: &ActionSchema) -> Value {
    match schema {
        ActionSchema::Choice { options } => json!(options[0].id),
        ActionSchema::Unit { among } => json!(among[0]),
        ActionSchema::Integer { max, .. } => json!(max),
        ActionSchema::Record { fields } => Value::Object(
            fields
                .iter()
                .map(|field| (field.name.clone(), mandatory_profile_answer(&field.schema)))
                .collect(),
        ),
        other => panic!("no profiling answer for {other:?}"),
    }
}
/// Cases: land:8.13, land:9.31, land:9.33
#[test]
fn planning_rebuilds_occupancy_after_a_nearby_move_and_checkpoint() {
    let (c, s, _o) = setup(TANK, Some("road"), false, None);
    let g = start(&c, s, true);
    let id: UnitId = TANK.into();
    let original = serde_json::to_value(reachable(&c, &g.state, &id, true)).unwrap();
    let mut congested = g.state.clone();
    let others: Vec<_> = congested
        .units_of(Side::Axis)
        .filter(|u| {
            u.id != id
                && c.units.units[&u.id].stacking_points == Some(1)
                && formation::combat_unit(&c, &u.id)
        })
        .take(5)
        .map(|u| u.id.to_string())
        .collect();
    assert_eq!(others.len(), 5);
    for other in &others {
        place(&mut congested, other, "C4021");
    }
    let paths = reachable(&c, &congested, &id, true);
    let dest = paths.iter().find(|r| r.hex.as_str() == "C4021").unwrap();
    assert_eq!(dest.cp_quarters, 8);
    let encoded = serde_json::to_value(&congested).unwrap();
    let recovered: State = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        serde_json::to_value(reachable(&c, &recovered, &id, true)).unwrap(),
        serde_json::to_value(&paths).unwrap()
    );
    assert_eq!(
        serde_json::to_value(reachable(&c, &g.state, &id, true)).unwrap(),
        original
    );
    let changed = Game::<Cna> {
        state: congested,
        rng: g.rng.clone(),
    };
    let t = respond(
        &c,
        &changed,
        seat(&changed),
        json!([{"unit":TANK,"path":dest.path}]),
        true,
    )
    .unwrap();
    assert_eq!(
        t.game.state.land.units[&id].cp_spent_quarters,
        dest.cp_quarters
    );
}
/// Cases: land:6.15, land:8.13, land:8.17, land:19.43, land:19.44
#[test]
fn planning_detaches_once_for_a_complete_path_and_stops_at_remaining_cp() {
    let child = "it.1_libyan_div.viii_libyan_bn";
    let parent = "it.1_libyan_div.1st_libyan_regt_hq";
    let (c, mut s, _o) = setup(child, Some("road"), false, None);
    place(&mut s, parent, "C4020");
    s.land.units.get_mut(&child.into()).unwrap().detached = false;
    let g = start(&c, s, true);
    let before = serde_json::to_value(&g.state).unwrap();
    let paths = reachable(&c, &g.state, &child.into(), true);
    let dest = paths.iter().find(|r| r.hex.as_str() == "C4023").unwrap();
    assert_eq!(dest.cp_quarters, 16); // one CP detach, three one-CP road entries
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":child,"path":dest.path}]),
        true,
    )
    .unwrap();
    assert_eq!(t.game.state.land.units[&child.into()].cp_spent_quarters, 16);
    assert_eq!(t.game.state.land.units[&parent.into()].cp_spent_quarters, 4);
    assert_eq!(serde_json::to_value(&g.state).unwrap(), before);
    let mut exhausted = g.state.clone();
    let ceiling = formation::allowance(&c, &exhausted, &child.into())
        .unwrap()
        .cpa
        * 6;
    exhausted
        .land
        .units
        .get_mut(&child.into())
        .unwrap()
        .voluntary_cp_quarters = ceiling - 5;
    assert!(reachable(&c, &exhausted, &child.into(), true).is_empty());
}
/// Cases: land:6.15, land:9.21, land:19.46
#[test]
fn indexed_allowances_preserve_real_attachments_and_remote_children() {
    let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut s = State::new(&c).unwrap();
    let child: UnitId = "it.1_libyan_div.viii_libyan_bn".into();
    s.land.units.get_mut(&child).unwrap().attached_to = Some("it.1ccnn_div.1st_ccnn_div_hq".into());
    s.land.units.get_mut(&child).unwrap().location = Location::Hex {
        hex: "C4021".into(),
    };
    let index = formation::FormationIndex::new(&c, &s);
    for u in s.land.units.values().filter(|u| u.location.hex().is_some()) {
        assert_eq!(
            index.allowance(&c, &s, &u.id),
            formation::allowance(&c, &s, &u.id),
            "{}",
            u.id
        );
    }
}
/// Cases: land:6.15, airlog:51.23, airlog:52.52, airlog:52.6
#[test]
fn restricted_infantry_members_set_the_whole_formations_cpa_ceiling() {
    let parent = "it.1_libyan_div.1st_libyan_regt_hq";
    let child = "it.1_libyan_div.viii_libyan_bn";
    for shortage in ["half", "water", "pasta"] {
        let (c, mut s, _o) = setup(parent, Some("road"), false, None);
        place(&mut s, child, "C4020");
        s.land.units.get_mut(&child.into()).unwrap().detached = false;
        let a = formation::allowance(&c, &s, &parent.into()).unwrap();
        for member in formation::members(&c, &s, &parent.into()) {
            let u = s.land.units.get_mut(&member).unwrap();
            u.cp_spent_quarters = a.cpa * 4 - 4;
            u.voluntary_cp_quarters = u.cp_spent_quarters;
        }
        let history = s.logistics.rations.get_mut(&child.into()).unwrap();
        match shortage {
            "half" => history.half = true,
            "water" => history.infantry_water_received = 0,
            "pasta" => history.pasta_gt = None,
            _ => unreachable!(),
        }
        let g = start(&c, s, true);
        let owner = seat(&g);
        let before = serde_json::to_value(&g.state).unwrap();
        let err = respond(
            &c,
            &g,
            owner,
            json!([{"unit":parent,"path":["C4021","C4022"]}]),
            true,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("limits movement to CPA"),
            "{shortage}: {err}"
        );
        assert_eq!(serde_json::to_value(&g.state).unwrap(), before);
        let accepted = respond(
            &c,
            &g,
            owner,
            json!([{"unit":parent,"path":["C4021"]}]),
            true,
        )
        .unwrap();
        assert_eq!(
            accepted.game.state.land.units[&child.into()].cp_spent_quarters,
            a.cpa * 4
        );
        assert_eq!(
            accepted.game.state.land.units[&parent.into()].cp_spent_quarters,
            a.cpa * 4
        );
        let paths = reachable(&c, &g.state, &parent.into(), true);
        assert!(!paths.is_empty());
        assert!(paths.iter().all(|p| p.cp_quarters <= 4));
    }
}
/// Cases: airlog:52.42, airlog:52.51, land:8.13
#[test]
fn dry_vehicle_stops_its_entire_stack_without_spending_fuel_cp_or_water() {
    let other = "it.libyan_tank_command.lxii_l_tank_bn";
    let (c, mut s, _o) = setup(TANK, Some("road"), false, None);
    place(&mut s, other, "C4020");
    let mut g = start(&c, s, true);
    g.state
        .logistics
        .unit_supply
        .get_mut(&TANK.into())
        .unwrap()
        .activity_water = WaterPoints::ZERO;
    let owner = seat(&g);
    assert!(!available(&c, &g.state, owner).contains(&TANK.into()));
    assert!(reachable(&c, &g.state, &TANK.into(), true).is_empty());
    let before = serde_json::to_value(&g.state).unwrap();
    let err = respond(
        &c,
        &g,
        owner,
        json!([{"unit":other,"path":["C4021"],"with_stack":true}]),
        true,
    )
    .unwrap_err();
    assert!(err.to_string().contains("water restrictions"));
    assert_eq!(serde_json::to_value(&g.state).unwrap(), before);
}
/// Cases: airlog:51.23, land:10.6, land:10.24
#[test]
fn half_rations_forbid_disclosed_zoc_and_stop_before_new_control() {
    let (c, mut s, _o) = setup(LEG, Some("road"), false, None);
    place(&mut s, TANK, "C4022");
    place(&mut s, "it.libyan_tank_command.lxii_l_tank_bn", "C4022");
    s.logistics.rations.get_mut(&LEG.into()).unwrap().half = true;
    let mut g = start(&c, s, true);
    let owner = seat(&g);
    g.state.land.movement.controls.insert("C4021".into(), true);
    let err = respond(&c, &g, owner, json!([{"unit":LEG,"path":["C4021"]}]), true).unwrap_err();
    assert!(err.to_string().contains("half-ration"));
    g.state.land.movement.controls.remove(&"C4021".into());
    let t = respond(&c, &g, owner, json!([{"unit":LEG,"path":["C4021"]}]), true).unwrap();
    assert_eq!(
        t.game.state.land.units[&LEG.into()].location.hex(),
        Some(&"C4020".into())
    );
    assert_eq!(t.game.state.land.units[&LEG.into()].cp_spent_quarters, 0);
    assert_eq!(
        t.game.state.land.movement.controls.get(&"C4021".into()),
        Some(&true)
    );
    assert!(
        t.events
            .iter()
            .any(|e| matches!(e.event, GameEvent::Note { .. }))
    );
}
/// Cases: airlog:52.42, airlog:52.43
#[test]
fn activity_water_covers_all_attached_trucks_once_per_stage_even_after_recovery() {
    let (c, mut s, _o) = setup(TANK, Some("road"), false, None);
    place(&mut s, LEG, "C4019");
    s.land.units.get_mut(&TANK.into()).unwrap().trucks = cna_content::units::Trucks {
        light: 2,
        medium: 1,
        heavy: 0,
    };
    let need = logistics::toe_strength(&c, &s.land.units[&TANK.into()])
        .unwrap()
        .get()
        + 3;
    s.logistics
        .unit_supply
        .get_mut(&TANK.into())
        .unwrap()
        .activity_water = WaterPoints::new(need);
    let g = start(&c, s, true);
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":TANK,"path":["C4021"]}]),
        true,
    )
    .unwrap();
    assert_eq!(
        t.game.state.logistics.unit_supply[&TANK.into()].activity_water,
        WaterPoints::ZERO
    );
    assert_eq!(
        t.game.state.logistics.rations[&TANK.into()].activity_used_stage,
        Some(logistics::water::WaterStage::current(&t.game.state))
    );
    let mut recovered: State =
        serde_json::from_value(serde_json::to_value(&t.game.state).unwrap()).unwrap();
    recovered.cursor.cycle += 1;
    recovered.cursor.entered = false;
    let second = start(&c, recovered, true);
    let t = respond(
        &c,
        &second,
        seat(&second),
        json!([{"unit":TANK,"path":["C4022"]}]),
        true,
    )
    .unwrap();
    assert_eq!(
        t.game.state.logistics.unit_supply[&TANK.into()].activity_water,
        WaterPoints::ZERO
    );
    let mut next_stage = t.game.state;
    next_stage.cursor.op_stage = Some(2);
    next_stage.land.movement.moved.clear();
    assert!(reachable(&c, &next_stage, &TANK.into(), true).is_empty());
}
/// Cases: land:3.62, airlog:51.23, airlog:52.51, airlog:52.52
#[test]
fn supplied_baseline_remains_legal_with_half_rations_and_a_dry_candidate() {
    for strict in [false, true] {
        let other = "it.libyan_tank_command.lxii_l_tank_bn";
        let (c, mut s, _o) = setup(TANK, Some("road"), false, None);
        place(&mut s, other, "C4020");
        let mut g = start(&c, s, strict);
        let request = Cna { strict }.pending(&c, &g.state)[0].clone();
        g.state
            .logistics
            .unit_supply
            .get_mut(&TANK.into())
            .unwrap()
            .activity_water = WaterPoints::ZERO;
        g.state
            .logistics
            .rations
            .get_mut(&other.into())
            .unwrap()
            .half = true;
        for seed in 0..16u8 {
            let action = crate::baseline::random_orders(
                &c,
                &g.state,
                &request,
                &mut CampaignRng::from_seed([seed; 32]),
            );
            assert!(!action.as_array().unwrap().is_empty());
            assert_eq!(action[0]["unit"], other);
            respond(&c, &g, request.seat, action, strict).unwrap();
        }
        let report = Cna { strict }
            .inspect(&c, &g.state, Perspective::Seat(request.seat), TANK)
            .unwrap();
        assert_eq!(
            report["movement_restrictions"][0]["restrictions"]["may_move"],
            false
        );
        crate::testkit::assert_face_only(&Cna { strict }.inspect(
            &c,
            &g.state,
            Perspective::Side(Side::Commonwealth),
            TANK,
        ));
    }
}

/// Cases: land:8.21, land:8.22, land:8.23
/// Interpretations: interp:land-0023
#[test]
fn either_phasing_side_repeats_without_resetting_cp_or_stage_water() {
    for own in [TANK, LEG] {
        let (c, mut s, _o) = setup(own, Some("road"), false, None);
        let enemy = if own == TANK { LEG } else { TANK };
        place(&mut s, enemy, "C4023");
        let g = start(&c, s, false);
        let t = respond(
            &c,
            &g,
            seat(&g),
            json!([{"unit":own,"path":["C4021"]}]),
            false,
        )
        .unwrap();
        let mut g = t.game;
        assert!(g.state.land.movement.ended);
        assert!(!g.state.land.movement.cycle_blocked.contains(&own.into()));
        let before = g.state.land.units[&own.into()].clone();
        let water = g.state.logistics.rations[&own.into()].clone();
        g.state.cursor.index = crate::seq::PLAYER_HALF
            .iter()
            .position(|p| p.anchor == "opstage.movement_and_combat.reserve_release")
            .unwrap();
        g.state.cursor.entered = false;
        let g = evaluate(&Cna::dev(), &c, &g, &Command::Advance)
            .unwrap()
            .game;
        assert_eq!(
            g.state.decisions.pending[0].kind,
            crate::land::reserve::RELEASE
        );
        let g = respond(&c, &g, seat(&g), Value::Null, false).unwrap().game;
        assert_eq!(
            g.state.decisions.pending[0].kind,
            super::super::cycles::KIND
        );
        let t = respond(&c, &g, seat(&g), json!(true), false).unwrap();
        let g = t.game;
        assert_eq!(g.state.cursor.cycle, 2);
        assert_eq!(g.state.land.units[&own.into()], before);
        assert_eq!(g.state.logistics.rations[&own.into()], water);
        let recovered: Game<Cna> =
            serde_json::from_value(serde_json::to_value(&g).unwrap()).unwrap();
        let g = evaluate(&Cna::dev(), &c, &recovered, &Command::Advance)
            .unwrap()
            .game;
        assert_eq!(g.state.decisions.pending[0].kind, KIND);
        let t = respond(
            &c,
            &g,
            seat(&g),
            json!([{"unit":own,"path":["C4022"]}]),
            false,
        )
        .unwrap();
        assert!(t.game.state.land.units[&own.into()].cp_spent_quarters > before.cp_spent_quarters);
        assert_eq!(
            t.game.state.logistics.fuel_segments[&own.into()]
                .origin
                .hex()
                .unwrap()
                .as_str(),
            "C4021"
        );
        assert_eq!(
            t.game.state.logistics.fuel_segments[&own.into()]
                .segment
                .cycle,
            2
        );
    }
}
/// Cases: land:8.23
/// Interpretations: interp:land-0023
#[test]
fn cycle_proximity_uses_printed_combat_faces_and_precombat_positions() {
    let (c, mut s, _o) = setup(TANK, Some("road"), false, None);
    place(&mut s, LEG, "C4023");
    s.decisions.pending.clear();
    super::super::cycles::finish_movement(&c, &mut s);
    assert!(s.land.movement.cycle_blocked.contains(&TANK.into())); // three hexes
    place(&mut s, LEG, "C4022");
    super::super::cycles::finish_movement(&c, &mut s); // snapshot cannot be refreshed by retreat
    assert!(s.land.movement.cycle_blocked.contains(&TANK.into()));
    let recovered: State = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert_eq!(
        recovered.land.movement.cycle_blocked,
        s.land.movement.cycle_blocked
    );
    s.land.movement.cycle_blocked.clear();
    s.land.movement.ended = false;
    super::super::cycles::finish_movement(&c, &mut s);
    assert!(!s.land.movement.cycle_blocked.contains(&TANK.into())); // distance two
    let other = c
        .units
        .units
        .values()
        .find(|u| {
            u.side == Side::Commonwealth
                && u.class
                    .as_ref()
                    .and_then(|id| c.units.classes.get(id))
                    .is_some_and(|c| c.unit_type == "headquarters")
        })
        .unwrap()
        .id
        .clone();
    s.land.units.get_mut(&LEG.into()).unwrap().location = Location::Eliminated;
    place(&mut s, other.as_str(), "C4021");
    s.land.movement.cycle_blocked.clear();
    s.land.movement.ended = false;
    super::super::cycles::finish_movement(&c, &mut s);
    assert!(s.land.movement.cycle_blocked.contains(&TANK.into())); // an HQ face is not combat
    s.cursor.cycle = 2;
    assert!(reachable(&c, &s, &TANK.into(), false).is_empty());
}

/// Cases: land:18.11, land:18.12, land:18.13, land:18.14, land:18.22, land:18.23, land:18.24, land:18.25, land:18.26
#[test]
fn reserve_designation_movement_and_release_survive_recovery() {
    use super::super::reserve::{self, Status};
    let (c, mut s, _o) = setup(TANK, Some("road"), false, None);
    s.cursor.index = 0;
    let g = start(&c, s, false);
    assert_eq!(g.state.decisions.pending[0].kind, reserve::DESIGNATE);
    assert_eq!(seat(&g).role, Role::RearArea);
    let before = g.clone();
    assert!(respond(&c, &g, seat(&g), json!([TANK, TANK]), false).is_err());
    assert_eq!(
        serde_json::to_value(&g).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    let t = respond(&c, &g, seat(&g), json!([TANK]), false).unwrap();
    assert_eq!(
        t.game.state.land.units[&TANK.into()].reserve.status,
        Status::First
    );
    assert_eq!(t.game.state.land.units[&TANK.into()].cp_spent_quarters, 0);
    let encoded = serde_json::to_value(&t.game).unwrap();
    let recovered: Game<Cna> = serde_json::from_value(encoded).unwrap();
    let g = evaluate(&Cna::dev(), &c, &recovered, &Command::Advance)
        .unwrap()
        .game;
    let reach = reachable(&c, &g.state, &TANK.into(), false);
    assert!(reach.iter().all(|r| r.path.len() == 1));
    assert!(
        respond(
            &c,
            &g,
            seat(&g),
            json!([{"unit":TANK,"path":["C4021","C4022"]}]),
            false
        )
        .is_err()
    );
    let moved = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":TANK,"path":["C4021"]}]),
        false,
    )
    .unwrap()
    .game;
    assert_eq!(moved.state.land.units[&TANK.into()].cp_spent_quarters, 2);
    let mut release = moved;
    release.state.cursor.index = 9;
    release.state.cursor.entered = false;
    release.state.decisions.pending.clear();
    let release = evaluate(&Cna::dev(), &c, &release, &Command::Advance)
        .unwrap()
        .game;
    assert_eq!(release.state.decisions.pending[0].kind, reserve::RELEASE);
    let kept = respond(&c, &release, seat(&release), Value::Null, false)
        .unwrap()
        .game;
    assert_eq!(
        kept.state.land.units[&TANK.into()].reserve.status,
        Status::Second
    );
    assert_eq!(kept.state.land.units[&TANK.into()].cp_spent_quarters, 2);
    let repeated = respond(&c, &kept, seat(&kept), json!(true), false)
        .unwrap()
        .game;
    let g = evaluate(&Cna::dev(), &c, &repeated, &Command::Advance)
        .unwrap()
        .game;
    assert!(g.state.decisions.pending.iter().all(|p| p.kind != KIND));
    assert!(reachable(&c, &g.state, &TANK.into(), false).is_empty());
    let mut release = g;
    // This fixture advances only the reserve/movement windows, not the separate combat windows.
    release.state.decisions.pending.clear();
    release.state.cursor.index = 9;
    release.state.cursor.entered = false;
    let release = evaluate(&Cna::dev(), &c, &release, &Command::Advance)
        .unwrap()
        .game;
    let active = respond(&c, &release, seat(&release), json!([TANK]), false)
        .unwrap()
        .game;
    let u = &active.state.land.units[&TANK.into()];
    assert_eq!(u.reserve.status, Status::ReleasedSecond);
    assert_eq!(u.reserve.released_for_cycle, Some(3));
    assert_eq!(
        formation::allowance(&c, &active.state, &TANK.into())
            .unwrap()
            .cpa,
        12
    );
    let active = respond(&c, &active, seat(&active), json!(true), false)
        .unwrap()
        .game;
    let g = evaluate(&Cna::dev(), &c, &active, &Command::Advance)
        .unwrap()
        .game;
    assert_eq!(g.state.cursor.cycle, 3);
    assert!(!reachable(&c, &g.state, &TANK.into(), false).is_empty());
}

fn reaction_fixture() -> (CnaContent, Game<Cna>, Overlay, UnitId) {
    let (c, mut s, o) = setup(TANK, Some("road"), false, None);
    let defender = s
        .units_of(Side::Commonwealth)
        .find(|u| {
            formation::class(&c, &u.id).is_some_and(|a| a.unit_type == "tank")
                && formation::individual_allowance(&c, &s, &u.id).is_some_and(|a| a.motorized)
        })
        .unwrap()
        .id
        .clone();
    place(&mut s, defender.as_str(), "C4022");
    {
        let g = start(&c, s, true);
        (c, g, o, defender)
    }
}
/// Cases: land:8.51, land:8.52, land:8.55, land:8.13, land:3.62
/// Interpretations: interp:land-0026
#[test]
fn reaction_interrupt_replans_and_survives_checkpoint_without_breaking_off_cost() {
    let (c, g, _o, defender) = reaction_fixture();
    let mover = seat(&g);
    let t = respond(
        &c,
        &g,
        mover,
        json!([{"unit":TANK,"path":["C4021","C4022","C4023"]}]),
        true,
    );
    // A path cannot include the public occupied enemy hex.
    assert!(t.is_err());
    let t = respond(
        &c,
        &g,
        mover,
        json!([{"unit":TANK,"path":["C4021","C4020"]}]),
        true,
    )
    .unwrap();
    assert_eq!(
        t.game.state.land.units[&TANK.into()].location.hex(),
        Some(&"C4021".into())
    );
    assert!(
        t.game
            .state
            .decisions
            .pending
            .iter()
            .all(|p| p.kind == super::super::reaction::KIND)
    );
    assert_eq!(t.game.state.land.units[&TANK.into()].cp_spent_quarters, 2);
    let serialized = serde_json::to_vec(&t.game).unwrap();
    let recovered: Game<Cna> = serde_json::from_slice(&serialized).unwrap();
    let defender_seat = seat(&recovered);
    let t = respond(
        &c,
        &recovered,
        defender_seat,
        json!([{"unit":defender,"path":["C4023"]}]),
        true,
    )
    .unwrap();
    let t = decline_empty_reactions(&c, t, true);
    assert_eq!(t.game.state.land.units[&defender].cp_spent_quarters, 2);
    assert_eq!(t.game.state.land.units[&defender].voluntary_cp_quarters, 0);
    assert_eq!(
        t.game.state.decisions.pending[0].kind,
        super::super::reaction::CONTINUE
    );
    let t = respond(
        &c,
        &t.game,
        mover,
        json!([{"unit":TANK,"path":["C4020"]}]),
        true,
    )
    .unwrap();
    assert_eq!(t.game.state.land.units[&TANK.into()].cp_spent_quarters, 4);
    assert_eq!(
        t.game.state.land.units[&TANK.into()].location.hex(),
        Some(&"C4020".into())
    );
    assert!(t.game.state.land.reaction.continuation.is_none());
    let enemy = Cna::full().inspect(&c, &t.game.state, Perspective::Seat(defender_seat), TANK);
    crate::testkit::assert_face_only(&enemy);
}
/// Cases: land:8.51, land:8.52, land:8.55, land:10.6
#[test]
fn reaction_baseline_answers_are_accepted_for_many_seeds() {
    let (c, g, _o, _) = reaction_fixture();
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":TANK,"path":["C4021"]}]),
        true,
    )
    .unwrap();
    let request = Cna::full().pending(&c, &t.game.state)[0].clone();
    assert_eq!(request.kind, super::super::reaction::KIND);
    for n in 0..96u8 {
        let mut rng = CampaignRng::from_seed([n; 32]);
        let action = crate::baseline::random_orders(&c, &t.game.state, &request, &mut rng);
        let accepted = respond(&c, &t.game, request.seat, action.clone(), true);
        assert!(accepted.is_ok(), "seed {n}: {action}: {:?}", accepted.err());
    }
}
/// Cases: land:8.53, land:8.54
#[test]
fn reaction_eligibility_excludes_engaged_pinned_and_unmotorized_defenders() {
    let (c, g, _o, id) = reaction_fixture();
    let moving = vec![TANK.into()];
    assert!(
        super::super::reaction::candidates(&c, &g.state, &moving, &"C4021".into(), &[], true)
            .unwrap()
            .contains(&id)
    );
    for engaged in [false, true] {
        let mut s = g.state.clone();
        if engaged {
            s.land.units.get_mut(&id).unwrap().engaged = true
        } else {
            s.land.combat.pinned.insert(id.clone());
        }
        assert!(
            !super::super::reaction::candidates(&c, &s, &moving, &"C4021".into(), &[], true)
                .unwrap()
                .contains(&id)
        );
    }
    let mut s = g.state.clone();
    place(&mut s, LEG, "C4022");
    assert!(
        !super::super::reaction::candidates(&c, &s, &moving, &"C4021".into(), &[], true)
            .unwrap()
            .contains(&LEG.into())
    );
}

/// Cases: land:10.6, land:3.62, airlog:50.12
#[test]
fn hidden_strength_and_ammunition_never_change_movement_answer_acceptance() {
    let (c, mut s, _o) = setup(LEG, Some("road"), false, None);
    let enemy2 = "it.libyan_tank_command.lxii_l_tank_bn";
    for id in [TANK, enemy2] {
        place(&mut s, id, "C4023");
    }
    let g = start(&c, s, true);
    let own = seat(&g);
    let inspect = Cna::full()
        .inspect(&c, &g.state, Perspective::Seat(own), LEG)
        .unwrap();
    let action = json!([{"unit":LEG,"path":["C4021","C4022","C4021"]}]);
    for (strength, ammo) in [(0, 0), (0, 10000), (8, 0), (8, 10000)] {
        let mut changed = g.clone();
        for id in [TANK, enemy2] {
            changed.state.land.units.get_mut(&id.into()).unwrap().toe =
                Some(cna_content::units::Toe::Under { under: strength });
            changed
                .state
                .logistics
                .unit_supply
                .get_mut(&id.into())
                .unwrap()
                .ready_ammo = cna_core::quantity::AmmoPoints::new(ammo);
        }
        assert_eq!(
            inspect,
            Cna::full()
                .inspect(&c, &changed.state, Perspective::Seat(own), LEG)
                .unwrap()
        );
        let t = respond(&c, &changed, own, action.clone(), true).unwrap();
        assert!(t.events.iter().any(|e|matches!(&e.event,GameEvent::DecisionResolved{summary,..} if summary=="Accepted 1 planned movement orders.")));
    }
}
/// Cases: land:10.21, land:10.6, land:3.62
#[test]
fn hidden_qualifying_control_gap_accepts_the_answer_then_stops_adjudication() {
    let (mut c, mut s, o) = setup(LEG, Some("road"), false, None);
    for id in [TANK, "it.libyan_tank_command.lxii_l_tank_bn"] {
        place(&mut s, id, "C4023");
    }
    let path = o.dir.join("coverage.csv");
    let rows = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        rows.lines()
            .filter(|line| !line.starts_with("side:") || !line.contains("C4022,C4023"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    c.map = MapContent::load(&o.dir).unwrap();
    let g = start(&c, s, true);
    let own = seat(&g);
    let mut weak = g.clone();
    for id in [TANK, "it.libyan_tank_command.lxii_l_tank_bn"] {
        weak.state.land.units.get_mut(&id.into()).unwrap().toe =
            Some(cna_content::units::Toe::Under { under: 0 });
    }
    let action = json!([{"unit":LEG,"path":["C4021","C4022"]}]);
    assert_eq!(
        Cna::full()
            .inspect(&c, &g.state, Perspective::Seat(own), LEG)
            .unwrap(),
        Cna::full()
            .inspect(&c, &weak.state, Perspective::Seat(own), LEG)
            .unwrap()
    );
    assert!(respond(&c, &weak, own, action.clone(), true).is_ok());
    let t = respond(&c, &g, own, action, true).unwrap();
    assert!(t.game.state.decisions.pending.is_empty());
    assert!(
        matches!(t.game.state.land.reaction.adjudication_stop,Some(EngineError::Unsupported{ref case,..}) if case=="land:10.21")
    );
    assert!(
        matches!(evaluate(&Cna::full(),&c,&t.game,&Command::Advance),Err(Rejection::Engine(EngineError::Unsupported{case,..})) if case=="land:10.21")
    );
}

/// Cases: land:8.55, land:10.21, land:10.6, airlog:50.12
#[test]
fn reaction_validation_is_equivalent_for_hidden_ammo_strength_and_edge_gap() {
    let (mut c, g, o, defender) = reaction_fixture();
    let mut initial = g.state.clone();
    let blockers = [
        "it.libyan_tank_command.lxii_l_tank_bn",
        "it.libyan_tank_command.lxiii_l_tank_bn",
    ];
    for id in blockers {
        place(&mut initial, id, "C4024");
    }
    let initial = Game {
        state: initial,
        rng: g.rng.clone(),
    };
    let t = respond(
        &c,
        &initial,
        seat(&initial),
        json!([{"unit":TANK,"path":["C4021"]}]),
        true,
    )
    .unwrap();
    let own = seat(&t.game);
    let original = Cna::full()
        .inspect(&c, &t.game.state, Perspective::Seat(own), defender.as_str())
        .unwrap();
    for (strength, ammo) in [(0, 0), (0, 10000), (8, 0), (8, 10000)] {
        let mut changed = t.game.clone();
        for id in blockers {
            changed.state.land.units.get_mut(&id.into()).unwrap().toe =
                Some(cna_content::units::Toe::Under { under: strength });
            changed
                .state
                .logistics
                .unit_supply
                .get_mut(&id.into())
                .unwrap()
                .ready_ammo = cna_core::quantity::AmmoPoints::new(ammo);
        }
        assert_eq!(
            original,
            Cna::full()
                .inspect(
                    &c,
                    &changed.state,
                    Perspective::Seat(own),
                    defender.as_str()
                )
                .unwrap()
        );
        assert!(
            respond(
                &c,
                &changed,
                own,
                json!([{"unit":defender,"path":["C4023"]}]),
                true
            )
            .is_ok()
        );
    }
    let path = o.dir.join("coverage.csv");
    let rows = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        rows.lines()
            .filter(|line| !line.starts_with("side:") || !line.contains("C4023,C4024"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    c.map = MapContent::load(&o.dir).unwrap();
    let accepted = respond(
        &c,
        &t.game,
        own,
        json!([{"unit":defender,"path":["C4023"]}]),
        true,
    )
    .unwrap();
    assert!(accepted.game.state.decisions.pending.is_empty());
    assert!(
        matches!(evaluate(&Cna::full(),&c,&accepted.game,&Command::Advance),Err(Rejection::Engine(EngineError::Unsupported{case,..})) if case=="land:10.21")
    );
}

/// Cases: land:3.61, land:3.62, land:8.51, land:10.6, land:18.0
#[test]
fn movement_and_reaction_windows_hide_enemy_cp_cohesion_reserves_and_intentions() {
    let (c, g, _o, defender) = reaction_fixture();
    let alter = |s: &mut State, id: &UnitId| {
        let u = s.land.units.get_mut(id).unwrap();
        u.cp_spent_quarters = 17;
        u.voluntary_cp_quarters = 9;
        u.cohesion_quarters = -31;
        u.engaged = true;
        u.reserve.status = super::super::reserve::Status::Second;
        s.land
            .assault_intentions
            .insert(id.clone(), BTreeSet::from(["C4024".into()]));
        s.logistics.unit_supply.get_mut(id).unwrap().ready_ammo =
            cna_core::quantity::AmmoPoints::ZERO;
    };
    let mut hidden = g.state.clone();
    alter(&mut hidden, &defender);
    crate::testkit::assert_indistinguishable(&Cna::full(), &c, &g.state, &hidden, Side::Axis);
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":TANK,"path":["C4021"]}]),
        true,
    )
    .unwrap();
    let mut hidden = t.game.state.clone();
    alter(&mut hidden, &TANK.into());
    crate::testkit::assert_indistinguishable(
        &Cna::full(),
        &c,
        &t.game.state,
        &hidden,
        Side::Commonwealth,
    );
}
/// Cases: land:8.53, land:8.54
#[test]
fn faster_announced_assault_blocks_reaction_and_discloses_only_stack_hexes() {
    let (c, g, _o, defender) = reaction_fixture();
    let mut s = g.state.clone();
    // Choose a real recce with the source's faster CPA and a battalion-sized target.
    let fast = s
        .units_of(Side::Axis)
        .find(|u| {
            formation::class(&c, &u.id).is_some_and(|a| a.unit_type == "recce" && a.cpa >= 40)
                && formation::individual_allowance(&c, &s, &u.id).is_some_and(|a| a.motorized)
        })
        .unwrap()
        .id
        .clone();
    s.land.units.get_mut(&TANK.into()).unwrap().location = Location::Eliminated;
    place(&mut s, fast.as_str(), "C4020");
    let a = formation::allowance(&c, &s, &fast).unwrap();
    let b = formation::allowance(&c, &s, &defender).unwrap();
    assert!(a.cpa >= b.cpa + 6, "{} versus {}", a.cpa, b.cpa);
    assert!(
        super::super::reaction::candidates(
            &c,
            &s,
            std::slice::from_ref(&fast),
            &"C4021".into(),
            &[],
            true
        )
        .unwrap()
        .contains(&defender)
    );
    assert!(
        !super::super::reaction::candidates(
            &c,
            &s,
            std::slice::from_ref(&fast),
            &"C4021".into(),
            &["C4022".into()],
            true
        )
        .unwrap()
        .contains(&defender)
    );
    s.cursor.entered = false;
    s.decisions.pending.clear();
    let g = start(&c, s, true);
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([{"unit":fast,"path":["C4021"],"close_assault":["C4022"]}]),
        true,
    )
    .unwrap();
    assert_eq!(
        t.game.state.land.assault_intentions[&fast],
        BTreeSet::from(["C4022".into()])
    );
    let announcement=t.events.iter().find(|e|matches!(&e.event,GameEvent::Note{text} if text.contains("announces close assault"))).unwrap();
    assert_eq!(announcement.audience, Audience::Public);
    let text = serde_json::to_string(&announcement.event).unwrap();
    assert!(
        text.contains("C4021")
            && text.contains("C4022")
            && !text.contains(fast.as_str())
            && !text.contains(defender.as_str())
    );
}

/// Cases: land:8.13, land:10.6
#[test]
#[ignore = "slow: benchmark"]
fn profile_large_mobile_unit_inspect_on_real_graziani() {
    let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut s = State::new(&c).unwrap();
    s.turn.weather = Some(crate::state::WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    s.turn.player_a = Some(Side::Axis);
    s.cursor.block = Block::PlayerHalf;
    s.cursor.half = Some(Half::A);
    s.cursor.op_stage = Some(1);
    s.cursor.index = 1;
    let ids: Vec<_> = s.land.units.keys().cloned().collect();
    for id in &ids {
        s.logistics.unit_supply.insert(
            id.clone(),
            UnitSupply {
                tank_fuel: FuelTenths::new(10000),
                ready_ammo: cna_core::quantity::AmmoPoints::new(10000),
                activity_water: WaterPoints::new(10000),
                ..Default::default()
            },
        );
        s.logistics.rations.insert(
            id.clone(),
            logistics::Rations {
                water_stage: Some(logistics::water::WaterStage::current(&s)),
                infantry_water_received: 2,
                issued_gt: Some(s.cursor.game_turn),
                pasta_gt: Some(s.cursor.game_turn),
                ..Default::default()
            },
        );
    }
    let g = start(&c, s, false);
    let chosen = g
        .state
        .units_of(Side::Axis)
        .filter(|u| {
            u.location.hex().is_some()
                && formation::allowance(&c, &g.state, &u.id).is_some_and(|a| a.motorized)
                && moving_limits(&c, &g.state, &u.id, false).is_ok()
        })
        .max_by_key(|u| {
            (
                formation::members(&c, &g.state, &u.id).len(),
                formation::allowance(&c, &g.state, &u.id).unwrap().cpa,
            )
        })
        .unwrap();
    let owner = SeatId::new(
        chosen.side,
        ownership::seat_for_unit(&c, &g.state, &chosen.id),
    );
    let t = std::time::Instant::now();
    for query in 0..5 {
        let query_started = std::time::Instant::now();
        std::hint::black_box(
            Cna::dev()
                .inspect(&c, &g.state, Perspective::Seat(owner), chosen.id.as_str())
                .unwrap(),
        );
        eprintln!("inspect query {}: {:?}", query + 1, query_started.elapsed());
    }
    eprintln!(
        "inspect {}: {} represented members, CPA {}, five queries {:?}",
        chosen.id,
        formation::members(&c, &g.state, &chosen.id).len(),
        formation::allowance(&c, &g.state, &chosen.id).unwrap().cpa,
        t.elapsed()
    );
    // CI run 37561141299 measured 342 ms/query on its slower runner (47337f3).
    // Allow roughly twice that measurement; the local optimization target stays 200 ms.
    assert!(
        t.elapsed() < std::time::Duration::from_millis(3500),
        "mean inspect should remain below the 700 ms CI regression limit"
    );
}

/// Cases: land:8.51, land:9.31, land:9.32
/// Interpretations: interp:land-0026
#[test]
fn reaction_in_overfull_transit_hex_requires_a_legal_continuation() {
    let (c, g, _o, defender) = reaction_fixture();
    let mut s = g.state.clone();
    let extra = s
        .units_of(Side::Axis)
        .filter(|u| {
            u.id.as_str() != TANK
                && c.units.units[&u.id].stacking_points == Some(1)
                && formation::combat_unit(&c, &u.id)
        })
        .take(6)
        .map(|u| u.id.clone())
        .collect::<Vec<_>>();
    for id in &extra {
        place(&mut s, id.as_str(), "C4021");
        s.land.units.get_mut(id).unwrap().cohesion_quarters = -104;
    }
    s.cursor.entered = false;
    s.decisions.pending.clear();
    let g = start(&c, s, true);
    let mover = seat(&g);
    let t = respond(
        &c,
        &g,
        mover,
        json!([{"unit":TANK,"path":["C4021","C4020"]}]),
        true,
    )
    .unwrap();
    assert_eq!(
        t.game.state.land.units[&TANK.into()].location.hex(),
        Some(&"C4021".into())
    );
    let reactor = seat(&t.game);
    let t = respond(&c, &t.game, reactor, Value::Null, true).unwrap();
    let t = decline_empty_reactions(&c, t, true);
    assert_eq!(
        t.game.state.decisions.pending[0].kind,
        super::super::reaction::CONTINUE
    );
    assert!(t.game.state.decisions.pending[0].space.pass.is_none());
    assert!(respond(&c, &t.game, mover, Value::Null, true).is_err());
    let request = Cna::full().pending(&c, &t.game.state)[0].clone();
    for n in 0..24u8 {
        let action = profiling_answer(
            &c,
            &t.game.state,
            &request,
            &mut CampaignRng::from_seed([n; 32]),
        );
        assert!(respond(&c, &t.game, mover, action, true).is_ok());
    }
    let result = respond(
        &c,
        &t.game,
        mover,
        json!([{"unit":TANK,"path":["C4020"]}]),
        true,
    )
    .unwrap();
    assert!(result.game.state.land.reaction.continuation.is_none());
    assert_eq!(result.game.state.land.units[&defender].cp_spent_quarters, 0);
}

/// Cases: land:3.61, land:3.62, land:8.23
/// Interpretations: interp:land-0023
#[test]
fn repeated_movement_does_not_disclose_combat_contents_of_nearby_enemy_stack() {
    let (c, mut a, _o) = setup(TANK, Some("road"), false, None);
    place(&mut a, LEG, "C4022");
    let hq = c
        .units
        .units
        .values()
        .find(|u| {
            u.side == Side::Commonwealth
                && u.class
                    .as_ref()
                    .and_then(|id| c.units.classes.get(id))
                    .is_some_and(|cl| cl.unit_type == "headquarters")
        })
        .unwrap()
        .id
        .clone();
    // The same printed HQ face in both worlds; only what is attached to it differs.
    place(&mut a, hq.as_str(), "C4022");
    let leg = a.land.units.get_mut(&LEG.into()).unwrap();
    leg.attached_to = Some(hq.clone());
    leg.detached = false;
    let mut b = a.clone();
    b.land.units.get_mut(&LEG.into()).unwrap().location = Location::Eliminated;
    for s in [&mut a, &mut b] {
        super::super::cycles::finish_movement(&c, s);
        s.cursor.cycle = 2;
    }
    crate::testkit::assert_indistinguishable(&Cna::dev(), &c, &a, &b, Side::Axis);
    assert!(a.land.movement.cycle_blocked.contains(&TANK.into()));
    assert!(b.land.movement.cycle_blocked.contains(&TANK.into()));
}

/// Cases: land:3.62, land:8.23
/// Interpretations: interp:land-0023
#[test]
fn printed_combat_proximity_ignores_variable_strength_in_both_phasing_halves() {
    for own in [TANK, LEG] {
        for half in [Half::A, Half::B] {
            let (c, mut a, _o) = setup(own, Some("road"), false, None);
            let enemy = if own == TANK { LEG } else { TANK };
            a.cursor.half = Some(half);
            let side = a.land.units[&own.into()].side;
            a.turn.player_a = Some(if half == Half::A {
                side
            } else {
                side.opponent()
            });
            place(&mut a, enemy, "C4022");
            assert!(crate::view::is_map_counter(
                &c,
                &a,
                &a.land.units[&enemy.into()]
            ));
            assert!(crate::view::printed_combat_face(&c, &enemy.into()));
            let mut b = a.clone();
            let unit = b.land.units.get_mut(&enemy.into()).unwrap();
            unit.toe = Default::default();
            unit.cohesion_quarters = -104;
            unit.cp_spent_quarters = 8;
            for state in [&mut a, &mut b] {
                super::super::cycles::finish_movement(&c, state);
                state.cursor.cycle = 2;
                assert!(!state.land.movement.cycle_blocked.contains(&own.into()));
            }
            crate::testkit::assert_indistinguishable(&Cna::full(), &c, &a, &b, side);
        }
    }
}
/// Rules as written, the enemy watches a counter move between occupied hexes by its printed
/// face; a pass tells it nothing, and neither does anything hidden about the mover.
/// Cases: land:3.61, land:3.62, land:8.13
#[test]
fn counter_traffic_shows_only_faces_and_private_pass_emits_no_enemy_events() {
    let (c, mut s, _o) = setup(TANK, Some("road"), false, None);
    place(&mut s, "it.libyan_tank_command.lxii_l_tank_bn", "C4020");
    place(&mut s, "it.libyan_tank_command.lxiii_l_tank_bn", "C4021");
    let g = start(&c, s, true);
    let own = seat(&g);
    let p = g
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat == own)
        .unwrap();
    let command = |action| {
        Command::Respond(DecisionResponse {
            decision_id: p.id.clone(),
            decision_revision: p.revision,
            seat: own,
            action,
            controller_epoch: 1,
            idempotency_key: "counter-traffic-pair".into(),
            public_explanation: None,
        })
    };
    let move_command = command(json!([{"unit":TANK,"path":["C4021"]}]));
    let mut tired = g.clone();
    tired
        .state
        .land
        .units
        .get_mut(&TANK.into())
        .unwrap()
        .cohesion_quarters -= 8;
    crate::testkit::assert_actions_indistinguishable(
        &Cna::full(),
        &c,
        (&g, &move_command),
        (&tired, &move_command),
        Side::Commonwealth,
    );
    let enemy = Perspective::Side(Side::Commonwealth);
    let pass = respond(&c, &g, own, Value::Null, true).unwrap();
    let moved = respond(&c, &g, own, json!([{"unit":TANK,"path":["C4021"]}]), true).unwrap();
    assert_eq!(
        pass.events
            .iter()
            .filter(|e| enemy.can_see(&e.audience))
            .count(),
        0
    );
    for e in moved.events.iter().filter(|e| enemy.can_see(&e.audience)) {
        if let GameEvent::UnitUpdated { unit } = &e.event {
            assert_eq!(unit.hex.as_deref(), Some("C4021"));
            crate::testkit::assert_face(&serde_json::to_value(unit).unwrap());
        }
    }
    assert!(
        moved
            .events
            .iter()
            .any(|e| matches!(e.event, GameEvent::UnitMoved { .. }))
    );
    let mut moved_tired = moved.game.state.clone();
    moved_tired
        .land
        .units
        .get_mut(&TANK.into())
        .unwrap()
        .cohesion_quarters -= 8;
    crate::testkit::assert_indistinguishable(
        &Cna::full(),
        &c,
        &moved.game.state,
        &moved_tired,
        Side::Commonwealth,
    );
}
/// Cases: land:8.51, land:8.53, land:3.62
#[test]
fn organized_and_disorganized_adjacent_tanks_do_not_change_preflight_acceptance() {
    let (c, g, _o, defender) = reaction_fixture();
    let mut disorganized = g.clone();
    disorganized
        .state
        .land
        .units
        .get_mut(&defender)
        .unwrap()
        .cohesion_quarters = -104;
    crate::testkit::assert_indistinguishable(
        &Cna::full(),
        &c,
        &g.state,
        &disorganized.state,
        Side::Axis,
    );
    let pending = &g.state.decisions.pending[0];
    let command = Command::Respond(DecisionResponse {
        decision_id: pending.id.clone(),
        seat: pending.seat,
        controller_epoch: 1,
        decision_revision: pending.revision,
        idempotency_key: "audit-pair".into(),
        action: json!([{"unit":TANK,"path":["C4021"]}]),
        public_explanation: None,
    });
    crate::testkit::assert_action_indistinguishable(
        &Cna::full(),
        &c,
        &g,
        &disorganized,
        &command,
        Side::Axis,
    );
    for initial in [&g, &disorganized] {
        assert!(
            respond(
                &c,
                initial,
                seat(initial),
                json!([{"unit":TANK,"path":["C4021"]}]),
                true
            )
            .is_ok()
        );
    }
}

/// Cases: land:8.13, land:21.24, land:21.31, land:21.35, land:21.43
#[test]
fn later_order_waits_for_the_first_units_breakdown_allocation() {
    let (c, mut s, _overlay) = setup(TANK, Some("road"), false, None);
    let other = "it.libyan_tank_command.lxii_l_tank_bn";
    place(&mut s, other, "C4020");
    let id: UnitId = TANK.into();
    let cap = logistics::capacity::fuel_capacity(&c, &s, &id).unwrap();
    s.logistics.unit_supply.get_mut(&id).unwrap().tank_fuel = cap;
    s.land
        .breakdown
        .accumulated_quarters
        .insert(id.clone(), 280);
    let g = start(&c, s, false);
    let t = respond(
        &c,
        &g,
        seat(&g),
        json!([
            {"unit":TANK,"path":["C4021"]},
            {"unit":other,"path":["C4019"]}
        ]),
        false,
    )
    .unwrap();
    assert_eq!(
        t.game.state.land.units[&other.into()].location.hex(),
        Some(&"C4020".into())
    );
    assert!(
        t.events
            .iter()
            .all(|e| !matches!(e.event, GameEvent::DiceRolled { .. }))
    );
    assert!(t.game.state.decisions.pending.is_empty());
    let g = evaluate(&Cna::dev(), &c, &t.game, &Command::Advance)
        .unwrap()
        .game;
    assert_eq!(
        g.state.decisions.pending[0].kind,
        super::super::breakdown::window::KIND
    );
    let request = Cna::dev().pending(&c, &g.state)[0].clone();
    let mut local = CampaignRng::from_seed([31; 32]);
    let action = crate::baseline::random_breakdown(&c, &g.state, &request, &mut local);
    assert!(!action.is_null());
    let g = respond(&c, &g, seat(&g), action, false).unwrap().game;
    assert_eq!(
        g.state.land.units[&other.into()].location.hex(),
        Some(&"C4020".into())
    );
    let g = evaluate(&Cna::dev(), &c, &g, &Command::Advance)
        .unwrap()
        .game;
    assert_eq!(
        g.state.land.units[&other.into()].location.hex(),
        Some(&"C4019".into())
    );
    assert!(!g.state.land.breakdown.markers.is_empty());
    assert!(g.state.land.breakdown.window.resume.is_none());
}

/// Cases: land:8.51, land:3.62
#[test]
fn public_reaction_windows_hide_readiness_and_private_passes_converge_after_restore() {
    let (c, mut s, _overlay) = setup(LEG, Some("road"), false, None);
    let second = "cw.2_nz_div.21st_nz_bn";
    place(&mut s, second, "C4025");
    place(&mut s, TANK, "C4022");
    s.land.units.get_mut(&TANK.into()).unwrap().toe =
        Some(cna_content::units::Toe::Under { under: 1 });
    let a = start(&c, s, true);
    let mut b = a.clone();
    b.state
        .land
        .units
        .get_mut(&TANK.into())
        .unwrap()
        .cohesion_quarters = -104;
    let p = &a.state.decisions.pending[0];
    let command = Command::Respond(DecisionResponse {
        decision_id: p.id.clone(),
        seat: p.seat,
        controller_epoch: 1,
        decision_revision: p.revision,
        idempotency_key: "public-reaction-pair".into(),
        action: json!([{"unit":LEG,"path":["C4021"]}]),
        public_explanation: None,
    });
    crate::testkit::assert_action_indistinguishable(
        &Cna::full(),
        &c,
        &a,
        &b,
        &command,
        Side::Commonwealth,
    );
    let mut a = evaluate(&Cna::full(), &c, &a, &command).unwrap().game;
    let mut b = evaluate(&Cna::full(), &c, &b, &command).unwrap().game;
    assert_eq!(a.state.decisions.pending.len(), 3);
    assert_eq!(b.state.decisions.pending.len(), 3);
    assert!(
        a.state
            .decisions
            .pending
            .iter()
            .all(|p| p.seat.side == Side::Axis)
    );
    assert!(
        b.state
            .decisions
            .pending
            .iter()
            .all(|p| p.space.context.as_ref().unwrap()["forced_pass"] == true)
    );
    for role in [Role::FrontLine, Role::RearArea, Role::Logistics] {
        // Simulate a checkpoint at every interrupt boundary, retaining the exact ids.
        a = serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
        b = serde_json::from_value(serde_json::to_value(&b).unwrap()).unwrap();
        let p = a
            .state
            .decisions
            .pending
            .iter()
            .find(|p| p.seat.role == role)
            .unwrap();
        let command = Command::Respond(DecisionResponse {
            decision_id: p.id.clone(),
            seat: p.seat,
            controller_epoch: 1,
            decision_revision: p.revision,
            idempotency_key: format!("pass-{role}"),
            action: Value::Null,
            public_explanation: None,
        });
        crate::testkit::assert_action_indistinguishable(
            &Cna::full(),
            &c,
            &a,
            &b,
            &command,
            Side::Commonwealth,
        );
        a = evaluate(&Cna::full(), &c, &a, &command).unwrap().game;
        b = evaluate(&Cna::full(), &c, &b, &command).unwrap().game;
    }
    assert_eq!(
        a.state.decisions.pending[0].kind,
        super::super::reaction::CONTINUE
    );
    let p = &a.state.decisions.pending[0];
    let command = Command::Respond(DecisionResponse {
        decision_id: p.id.clone(),
        seat: p.seat,
        controller_epoch: 1,
        decision_revision: p.revision,
        idempotency_key: "stop-after-private-passes".into(),
        action: Value::Null,
        public_explanation: None,
    });
    crate::testkit::assert_action_indistinguishable(
        &Cna::full(),
        &c,
        &a,
        &b,
        &command,
        Side::Commonwealth,
    );
    let a = evaluate(&Cna::full(), &c, &a, &command).unwrap().game;
    assert_eq!(a.state.decisions.pending[0].kind, KIND);
    assert!(eligible(
        &c,
        &a.state,
        &second.into(),
        a.state.decisions.pending[0].seat
    ));
}

/// Cases: land:8.63, land:8.64, land:8.66, land:8.67
#[test]
fn last_engaged_opponent_retreat_clears_both_tanks_and_recovers_relationships() {
    let (c, mut s, _overlay) = setup(LEG, Some("road"), false, None);
    let other = UnitId::new("it.libyan_tank_command.lxii_l_tank_bn");
    place(&mut s, LEG, "C4021");
    place(&mut s, TANK, "C4020");
    place(&mut s, other.as_str(), "C4020");
    let defenders = [UnitId::new(TANK), other.clone()];
    let id = UnitId::new(LEG);
    super::super::engagement::engage(&mut s, std::slice::from_ref(&id), &defenders).unwrap();
    let order = Order {
        unit: id.clone(),
        path: vec!["C4022".into(), "C4023".into()],
        with_stack: false,
        close_assault: vec![],
    };
    let seat = SeatId::new(
        Side::Commonwealth,
        crate::ownership::seat_for_unit(&c, &s, &id),
    );
    let before = serde_json::to_value(&s).unwrap();
    let preview = validate_nonphasing(&c, &s, &order, seat, true, NonPhasingMove::Retreat).unwrap();
    assert_eq!(preview.cp_quarters, 24); // 4 CP breakoff + two 1-CP foot-unit road entries.
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    let mut rng = CampaignRng::from_seed([72; 32]);
    let mut events = vec![];
    execute_nonphasing(
        &c,
        &mut s,
        &order,
        seat,
        true,
        NonPhasingMove::Retreat,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert_eq!(s.land.units[&id].location.hex(), Some(&"C4023".into()));
    assert!(s.land.engagements.is_empty());
    for unit in std::iter::once(&id).chain(&defenders) {
        assert!(!s.land.units[unit].engaged);
    }
    let initial: State = serde_json::from_value(before).unwrap();
    let game = start(&c, initial, true);
    let accepted = respond(
        &c,
        &game,
        seat,
        json!([{"unit":id,"path":["C4022","C4023"]}]),
        true,
    )
    .unwrap();
    for d in &defenders {
        assert!(accepted.events.iter().any(|e| e.audience==Audience::SideOnly(Side::Axis)
            && matches!(&e.event,cna_protocol::GameEvent::UnitUpdated {unit} if unit.id==d.as_str())));
        assert!(!accepted.events.iter().any(|e| e.visible_to(Perspective::Side(Side::Commonwealth))
            && matches!(&e.event,cna_protocol::GameEvent::UnitUpdated {unit} if unit.id==d.as_str())));
    }
    let restored: State = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(&restored).unwrap(),
        serde_json::to_value(&s).unwrap()
    );
    // The former opponents now owe only travel CP, not a stale engagement surcharge.
    let tank_order = Order {
        unit: defenders[0].clone(),
        path: vec!["C4019".into()],
        with_stack: false,
        close_assault: vec![],
    };
    let tank_seat = SeatId::new(
        Side::Axis,
        crate::ownership::seat_for_unit(&c, &restored, &defenders[0]),
    );
    assert_eq!(
        validate_nonphasing(
            &c,
            &restored,
            &tank_order,
            tank_seat,
            true,
            NonPhasingMove::Retreat
        )
        .unwrap()
        .cp_quarters,
        2
    );
}

/// Cases: land:8.63, land:8.66, land:8.67, land:8.68
#[test]
fn one_remaining_engaged_opponent_preserves_tank_flags_until_its_own_breakoff() {
    let (c, mut s, _overlay) = setup(LEG, Some("road"), false, None);
    let second = UnitId::new("cw.2_nz_div.21st_nz_bn");
    let other = UnitId::new("it.libyan_tank_command.lxii_l_tank_bn");
    for id in [LEG, second.as_str()] {
        place(&mut s, id, "C4021");
    }
    for id in [TANK, other.as_str()] {
        place(&mut s, id, "C4020");
    }
    let defenders = [UnitId::new(TANK), other];
    let attackers = [UnitId::new(LEG), second.clone()];
    super::super::engagement::engage(&mut s, &attackers, &defenders).unwrap();
    let original: State = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    let mut rng = CampaignRng::from_seed([73; 32]);
    for (n, id) in attackers.iter().enumerate() {
        let order = Order {
            unit: id.clone(),
            path: vec!["C4022".into(), "C4023".into()],
            with_stack: false,
            close_assault: vec![],
        };
        let seat = SeatId::new(
            Side::Commonwealth,
            crate::ownership::seat_for_unit(&c, &s, id),
        );
        let mut events = vec![];
        execute_nonphasing(
            &c,
            &mut s,
            &order,
            seat,
            true,
            NonPhasingMove::Retreat,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        for d in &defenders {
            assert_eq!(s.land.units[d].engaged, n == 0);
            if n == 0 {
                assert_eq!(s.land.engagements[d], BTreeSet::from([second.clone()]));
            }
        }
        if n == 0 {
            assert!(
                !events
                    .iter()
                    .any(|e| e.audience == Audience::Side(Side::Axis)
                        && matches!(&e.event, cna_protocol::GameEvent::UnitUpdated { .. }))
            );
        }
        s = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    }
    assert!(s.land.engagements.is_empty());
    // Invalid participant sides are checked before any relation or flag changes.
    let before = serde_json::to_value(&s).unwrap();
    assert!(super::super::engagement::engage(&mut s, &attackers, &attackers).is_err());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    // The graph's identities do not appear in the opponent's readable state.
    let mut graph_only = original.clone();
    graph_only.land.engagements.clear();
    crate::testkit::assert_indistinguishable(&Cna::full(), &c, &original, &graph_only, Side::Axis);
}

/// Cases: land:15.81, land:8.53, land:8.67, land:6.16
#[test]
fn authoritative_patrol_boundary_expires_engagement_before_the_next_stage() {
    let (c, mut s, _overlay) = setup(LEG, Some("road"), false, None);
    place(&mut s, LEG, "C4021");
    place(&mut s, TANK, "C4020");
    let leg = UnitId::new(LEG);
    let tank = UnitId::new(TANK);
    super::super::engagement::engage(
        &mut s,
        std::slice::from_ref(&leg),
        std::slice::from_ref(&tank),
    )
    .unwrap();
    s.cursor.index = crate::seq::PLAYER_HALF.len() - 1;
    s.cursor.half = Some(Half::B);
    s.cursor.entered = false;
    s.turn.initiative = Some(Side::Axis);
    let g = Game {
        state: s,
        rng: CampaignRng::from_seed([81; 32]).state(),
    };
    let t = evaluate(&Cna::dev(), &c, &g, &Command::Advance).unwrap();
    assert_eq!(t.game.state.cursor.op_stage, Some(2));
    assert_eq!(t.game.state.cursor.block, Block::OpStage);
    assert!(t.game.state.land.engagements.is_empty());
    assert!(!t.game.state.land.units[&leg].engaged && !t.game.state.land.units[&tank].engaged);
    let candidates =
        super::super::reaction::candidates(&c, &t.game.state, &[leg], &"C4021".into(), &[], true)
            .unwrap();
    assert!(candidates.contains(&tank));
    let order = Order {
        unit: tank.clone(),
        path: vec!["C4019".into()],
        with_stack: false,
        close_assault: vec![],
    };
    assert_eq!(
        validate_nonphasing(
            &c,
            &t.game.state,
            &order,
            SeatId::new(Side::Axis, Role::FrontLine),
            true,
            NonPhasingMove::Retreat
        )
        .unwrap()
        .cp_quarters,
        2
    );
    assert!(t.events.iter().any(|e|e.audience==Audience::SideOnly(Side::Axis) && matches!(&e.event,cna_protocol::GameEvent::UnitUpdated {unit} if unit.id==TANK && unit.detail.as_ref().unwrap()["engaged"]==false)));
    assert!(
        !t.events
            .iter()
            .any(|e| e.visible_to(Perspective::Side(Side::Commonwealth))
                && matches!(&e.event,cna_protocol::GameEvent::UnitUpdated {unit} if unit.id==TANK))
    );
}
/// Cases: land:12.45, land:19.62, land:8.67, land:13.21
#[test]
fn terminal_barrage_loss_reconciles_engagement_before_the_same_advance_opens_retreat() {
    use super::super::combat::barrage;
    use cna_core::decision::{ActionSpace, Secrecy, Trigger};
    let (c, mut s, _overlay) = setup(LEG, Some("road"), false, None);
    place(&mut s, LEG, "C4021");
    place(&mut s, TANK, "C4020");
    let leg = UnitId::new(LEG);
    let tank = UnitId::new(TANK);
    s.land.units.get_mut(&leg).unwrap().toe = Some(cna_content::units::Toe::Under { under: 1 });
    super::super::engagement::engage(
        &mut s,
        std::slice::from_ref(&leg),
        std::slice::from_ref(&tank),
    )
    .unwrap();
    s.cursor.index = 4;
    s.cursor.entered = true;
    s.land.combat.barrage.resolved = true;
    s.land.combat.barrage.casualties.insert(
        leg.clone(),
        vec![barrage::Casualty {
            weapons: vec![],
            loss: 1,
            pinned: false,
            trucks: 0,
        }],
    );
    let seat = SeatId::new(Side::Commonwealth, Role::FrontLine);
    let mut rng = CampaignRng::from_seed([82; 32]);
    let mut events = vec![];
    let (losses, _) = barrage::loss_space(&c, &s, seat).unwrap();
    crate::steps::open(
        &mut s,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        seat,
        barrage::LOSSES,
        "Allocate the battalion's own recorded casualty.".into(),
        &["land:12.45"],
        Trigger::Triggered,
        Secrecy::Secret,
        ActionSpace::new(losses),
    );
    let g = Game {
        state: s,
        rng: rng.state(),
    };
    let answered = respond(
        &c,
        &g,
        seat,
        json!([{"unit":LEG,"weapon":null,"toe":1}]),
        false,
    )
    .unwrap();
    assert_eq!(formation::strength(&c, &answered.game.state, &leg), 0);
    // An own answer records casualties; it does not adjudicate the enemy's engagement status.
    assert!(answered.game.state.land.units[&tank].engaged);
    let t = evaluate(&Cna::dev(), &c, &answered.game, &Command::Advance).unwrap();
    assert_eq!(
        t.game.state.cursor.step().unwrap().anchor,
        super::super::combat::retreat::ANCHOR
    );
    assert!(!t.game.state.land.units[&tank].engaged);
    assert!(t.game.state.land.engagements.is_empty());
    let request = Cna::dev()
        .pending(&c, &t.game.state)
        .into_iter()
        .find(|r| r.seat.side == Side::Axis && r.kind == super::super::combat::retreat::KIND)
        .unwrap();
    let order = Order {
        unit: tank.clone(),
        path: vec!["C4019".into()],
        with_stack: false,
        close_assault: vec![],
    };
    assert_eq!(
        validate_nonphasing(
            &c,
            &t.game.state,
            &order,
            request.seat,
            true,
            NonPhasingMove::Retreat
        )
        .unwrap()
        .cp_quarters,
        2
    );
    assert!(t.events.iter().any(|e|e.audience==Audience::SideOnly(Side::Axis) && matches!(&e.event,cna_protocol::GameEvent::UnitUpdated {unit} if unit.id==TANK && unit.detail.as_ref().unwrap()["engaged"]==false)));
    let restored: State =
        serde_json::from_value(serde_json::to_value(&t.game.state).unwrap()).unwrap();
    assert!(!restored.land.units[&tank].engaged && restored.land.engagements.is_empty());
}

/// Cases: land:8.56, land:8.92, airlog:49.16
#[test]
fn incoming_cohort_search_restores_foreign_origin_stock_and_shared_rounding_credit() {
    use crate::state::{Dump, DumpLocation};
    use cna_content::{
        scenario::Supplies,
        units::{Toe, Trucks},
    };
    let (c, mut s, _overlay) = setup(LEG, Some("road"), false, None);
    let donor = UnitId::new(LEG);
    let child = UnitId::new("cw.2_nz_div.21st_nz_bn");
    place(&mut s, LEG, "C4020");
    place(&mut s, child.as_str(), "C4024");
    s.logistics.dumps.clear();
    for id in [&donor, &child] {
        s.land.units.get_mut(id).unwrap().trucks = Trucks::default();
        s.land.units.get_mut(id).unwrap().transport_trucks = Trucks::default();
        s.logistics.unit_supply.get_mut(id).unwrap().tank_fuel = FuelTenths::new(0);
    }
    s.land.units.get_mut(&child).unwrap().toe = Some(Toe::Under { under: 1 });
    s.land.units.get_mut(&donor).unwrap().trucks.medium = 1;
    s.logistics.dumps.insert(
        "foreign-origin".into(),
        Dump {
            id: "foreign-origin".into(),
            marker: String::new(),
            side: Side::Commonwealth,
            location: DumpLocation::Hex {
                hex: "C4020".into(),
            },
            supplies: Supplies {
                fuel: 3,
                ..Supplies::default()
            },
            active: true,
            dummy: false,
        },
    );
    logistics::spend_segment_fuel(&c, &mut s, &donor, 4).unwrap();
    s.land.units.get_mut(&donor).unwrap().location = Location::Hex {
        hex: "C4024".into(),
    };
    let cohorts = logistics::segment_fuel_cohorts(&s, &donor).unwrap();
    logistics::transfer_selected_segment_fuel_cohorts(
        &mut s,
        &donor,
        &child,
        &[logistics::FuelCohortSelection {
            id: cohorts[0].id.clone(),
            count: 1,
        }],
    )
    .unwrap();
    s.land.units.get_mut(&donor).unwrap().trucks.medium = 0;
    s.land.units.get_mut(&child).unwrap().trucks.medium = 1;
    s.land
        .units
        .get_mut(&child)
        .unwrap()
        .transport_trucks
        .medium = 1;
    assert_eq!(
        s.logistics.fuel_segments[&child].origin.hex(),
        Some(&"C4024".into())
    );
    assert_eq!(
        s.logistics.fuel_accounts[&donor].origin.hex(),
        Some(&"C4020".into())
    );
    let before = serde_json::to_value(&s).unwrap();
    let paths = reachable(&c, &s, &child, true);
    assert!(paths.iter().any(|p| p.hex.as_str() == "C4033"));
    assert!(paths.iter().any(|p| p.hex.as_str() == "C4010"));
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    let seat = SeatId::new(Side::Commonwealth, Role::FrontLine);
    for r in &paths {
        let order = Order {
            unit: child.clone(),
            path: r.path.clone(),
            with_stack: false,
            close_assault: vec![],
        };
        let cost =
            validate_nonphasing(&c, &s, &order, seat, true, NonPhasingMove::Retreat).unwrap();
        assert_eq!(cost.cp_quarters, r.cp_quarters, "{}", r.hex);
    }
    assert_eq!(
        serde_json::to_value(reachable(&c, &s, &child, true)).unwrap(),
        serde_json::to_value(&paths).unwrap()
    );
    let restored: State = serde_json::from_value(before).unwrap();
    assert_eq!(
        serde_json::to_value(reachable(&c, &restored, &child, true)).unwrap(),
        serde_json::to_value(&paths).unwrap()
    );
}

/// Handling bans follow every represented physical carrier, with no profile exemption.
/// Cases: land:8.88, land:9.21
#[test]
fn box_handling_blocks_each_represented_member_and_expires_next_stage() {
    let (c, s, _o) = setup(LEG, Some("road"), false, None);
    let child: UnitId = "cw.2_nz_div.21st_nz_bn".into();
    for strict in [false, true] {
        for blocked in [UnitId::new(LEG), child.clone()] {
            let mut a = s.clone();
            place(&mut a, child.as_str(), "C4020");
            a.land.units.get_mut(&child).unwrap().attached_to = Some(LEG.into());
            a.land.units.get_mut(&child).unwrap().detached = false;
            a.land.units.get_mut(&blocked).unwrap().box_handling =
                Some(logistics::box_handling::BoxHandling {
                    stage: logistics::water::WaterStage::current(&a),
                    loaded: cna_content::scenario::Supplies {
                        stores: 1,
                        ..Default::default()
                    },
                    unloaded: Default::default(),
                });
            let order = Order {
                unit: LEG.into(),
                path: vec!["C4021".into()],
                with_stack: false,
                close_assault: vec![],
            };
            let before = serde_json::to_value(&a).unwrap();
            assert!(
                validate_nonphasing(
                    &c,
                    &a,
                    &order,
                    SeatId::new(Side::Commonwealth, Role::FrontLine),
                    strict,
                    NonPhasingMove::Retreat
                )
                .is_err()
            );
            assert_eq!(serde_json::to_value(&a).unwrap(), before);
            assert!(
                nonphasing_reachable(&c, &a, &LEG.into(), strict, NonPhasingMove::Retreat)
                    .is_empty()
            );
            a.cursor.op_stage = Some(2);
            place(&mut a, child.as_str(), "C4020");
            a.land.units.get_mut(&child).unwrap().attached_to = Some(LEG.into());
            a.land.units.get_mut(&child).unwrap().detached = false;
            assert!(
                validate_nonphasing(
                    &c,
                    &a,
                    &order,
                    SeatId::new(Side::Commonwealth, Role::FrontLine),
                    strict,
                    NonPhasingMove::Retreat
                )
                .is_ok()
            );
        }
    }
}
fn truck_reaction_fixture(native: i32) -> (CnaContent, Game<Cna>, Overlay, UnitId) {
    use cna_content::units::{Toe, WeaponPoints};
    use cna_tables::Bound;
    let (mut c, mut s, overlay) = setup(TANK, Some("road"), false, None);
    let path = cna_content::repo_data_dir().join("tables/airlog/54.2-truck-characteristics.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("cpa_inf = 20", "cpa_inf = 40");
    c.tables.airlog.truck_characteristics = cna_tables::airlog::trucks::TruckTable::from_raw(
        &cna_tables::RawTable::parse(&path, &text).unwrap(),
    )
    .unwrap();
    let child: UnitId = "cw.2_nz_div.21st_nz_bn".into();
    for id in [UnitId::new(LEG), child.clone()] {
        place(&mut s, id.as_str(), "C4022");
        s.land.units.get_mut(&id).unwrap().toe = Some(Toe::Under { under: 1 });
        s.land.units.get_mut(&id).unwrap().trucks = Default::default();
        s.land.units.get_mut(&id).unwrap().transport_trucks = Default::default();
        s.logistics.unit_supply.get_mut(&id).unwrap().tank_fuel = FuelTenths::new(0);
        c.units.units.get_mut(&id).unwrap().cpa = Some(native);
    }
    s.land.units.get_mut(&child).unwrap().attached_to = Some(LEG.into());
    s.land.units.get_mut(&child).unwrap().detached = false;
    s.land.units.get_mut(&LEG.into()).unwrap().trucks.medium = 1;
    s.logistics
        .unit_supply
        .get_mut(&LEG.into())
        .unwrap()
        .tank_fuel = FuelTenths::new(10);
    let class_id = c.units.units[&TANK.into()].class.clone().unwrap();
    c.units.classes.get_mut(&class_id).unwrap().cpa_fixed = false;
    c.units.classes.get_mut(&class_id).unwrap().cpa = 46;
    c.units.units.get_mut(&TANK.into()).unwrap().cpa = None;
    let weapon = match &s.land.units[&TANK.into()].toe {
        Some(Toe::Weapons(ws)) => c.units.weapons[&ws[0].weapon].clone(),
        _ => panic!("tank fixture has weapons"),
    };
    for cpa in [30, 35, 45, 46] {
        let mut w = weapon.clone();
        w.cpa = cpa;
        c.units.weapons.insert(format!("test_cpa_{cpa}"), w);
    }
    s.land.units.get_mut(&TANK.into()).unwrap().toe = Some(Toe::Weapons(vec![WeaponPoints {
        weapon: "test_cpa_30".into(),
        n: 1,
    }]));
    {
        let g = start(&c, s, true);
        (c, g, overlay, child)
    }
}
fn change_mover_cpa(g: &Game<Cna>, cpa: i32) -> Game<Cna> {
    let mut g = g.clone();
    g.state.land.units.get_mut(&TANK.into()).unwrap().toe =
        Some(cna_content::units::Toe::Weapons(vec![
            cna_content::units::WeaponPoints {
                weapon: format!("test_cpa_{cpa}"),
                n: 1,
            },
        ]));
    g
}
fn announced_reaction_command(g: &Game<Cna>) -> Command {
    let p = &g.state.decisions.pending[0];
    Command::Respond(DecisionResponse {
        decision_id: p.id.clone(),
        seat: p.seat,
        controller_epoch: 1,
        decision_revision: p.revision,
        idempotency_key: "truck-cpa-pair".into(),
        action: json!([{"unit":TANK,"path":["C4021"],"close_assault":["C4022"]}]),
        public_explanation: None,
    })
}
/// The reacting side learns only eligibility for CPA options it can obtain from its own trucks.
/// Cases: land:8.53, land:8.54, land:8.56, land:3.62
#[test]
fn reaction_reachable_cpa_options_hide_thresholds_between_equivalent_movers() {
    let (c, a, _o, child) = truck_reaction_fixture(10);
    let b = change_mover_cpa(&a, 35);
    let command = announced_reaction_command(&a);
    crate::testkit::assert_action_indistinguishable(
        &Cna::full(),
        &c,
        &a,
        &b,
        &command,
        Side::Commonwealth,
    );
    let t = evaluate(&Cna::full(), &c, &a, &command).unwrap();
    assert_eq!(
        t.game
            .state
            .land
            .reaction
            .window
            .as_ref()
            .unwrap()
            .cpa_options[&child],
        BTreeMap::from([(10, false), (40, true)])
    );
}
/// This is the source-authorized yes/no disclosure for physically reachable equipment options.
/// Cases: land:8.53, land:8.54, land:8.56, land:3.62
#[test]
#[should_panic]
fn reaction_cpa_pair_harness_detects_source_authorized_eligibility_difference() {
    let (c, a, _o, _) = truck_reaction_fixture(20);
    let b = change_mover_cpa(&a, 46);
    crate::testkit::assert_action_indistinguishable(
        &Cna::full(),
        &c,
        &a,
        &b,
        &announced_reaction_command(&a),
        Side::Commonwealth,
    );
}

/// Motorization, division, detachment and the path commit together after own-known preflight.
/// Cases: land:8.56, land:8.95, land:8.96, airlog:49.16, airlog:52.42
#[test]
fn reaction_divides_trucks_then_moves_component_with_checkpoint_and_rollback() {
    let (c, g, _o, child) = truck_reaction_fixture(10);
    let t = evaluate(&Cna::full(), &c, &g, &announced_reaction_command(&g)).unwrap();
    let recovered: Game<Cna> =
        serde_json::from_slice(&serde_json::to_vec(&t.game).unwrap()).unwrap();
    let division = super::super::trucks::reachable_divisions(&c, &recovered.state, &child, true)
        [&40]
        .clone()
        .unwrap();
    let answer = json!([{"unit":child,"path":["C4023"],"truck_division":division}]);
    let seat = SeatId::new(Side::Commonwealth, Role::FrontLine);
    let original = serde_json::to_value(&recovered).unwrap();
    let mut invalid = answer.clone();
    invalid[0]["path"] = json!(["C4029"]);
    assert!(respond(&c, &recovered, seat, invalid, true).is_err());
    assert_eq!(serde_json::to_value(&recovered).unwrap(), original);
    let moved = respond(&c, &recovered, seat, answer.clone(), true).unwrap();
    assert_eq!(
        moved.game.state.land.units[&child].location.hex(),
        Some(&"C4023".into())
    );
    assert!(moved.game.state.land.units[&child].detached);
    assert_eq!(moved.game.state.land.units[&child].trucks.medium, 1);
    assert_eq!(moved.game.state.land.units[&LEG.into()].trucks.medium, 0);
    assert_eq!(moved.game.state.land.units[&child].cp_spent_quarters, 6);
    assert_eq!(
        moved.game.state.land.units[&LEG.into()].cp_spent_quarters,
        4
    );
    assert_eq!(
        serde_json::to_value(respond(&c, &t.game, seat, answer, true).unwrap().game).unwrap(),
        serde_json::to_value(&moved.game).unwrap()
    );
    // The split-off component is a new counter on the map: the other side sees its face only.
    for e in moved
        .events
        .iter()
        .filter(|e| e.audience == Audience::SideOnly(Side::Axis))
    {
        if let GameEvent::UnitUpdated { unit } = &e.event {
            crate::testkit::assert_face(&serde_json::to_value(unit).unwrap());
        }
    }
}
/// The scripted reactor uses a declared, physically motorized option; it never probes the mover.
/// Cases: land:8.53, land:8.55, land:8.56
#[test]
fn reaction_division_baseline_is_seeded_and_always_accepted() {
    let (c, g, _o, child) = truck_reaction_fixture(10);
    let t = evaluate(&Cna::full(), &c, &g, &announced_reaction_command(&g)).unwrap();
    let request = Cna::full()
        .pending(&c, &t.game.state)
        .into_iter()
        .find(|p| {
            p.kind == super::super::reaction::KIND
                && p.seat.side == Side::Commonwealth
                && p.seat.role == Role::FrontLine
        })
        .unwrap();
    for n in 0..32u8 {
        let mut rng = CampaignRng::from_seed([n; 32]);
        let action = crate::baseline::random_orders(&c, &t.game.state, &request, &mut rng);
        assert!(
            action.as_array().is_some_and(|a| !a.is_empty()),
            "seed {n}: {action}"
        );
        assert_eq!(action[0]["unit"], json!(child));
        assert!(
            respond(&c, &t.game, request.seat, action.clone(), true).is_ok(),
            "seed {n}: {action}"
        );
        let mut same = CampaignRng::from_seed([n; 32]);
        assert_eq!(
            action,
            crate::baseline::random_orders(&c, &t.game.state, &request, &mut same)
        );
    }
}

/// The announced-assault CPA gap is six, so a rating of40 is blocked at46 and permitted at45.
/// Cases: land:8.53, land:8.54, land:8.56
#[test]
fn reaction_truck_option_uses_exact_six_cpa_announcement_boundary() {
    let (c, a, _o, child) = truck_reaction_fixture(20);
    let b = change_mover_cpa(&a, 45);
    let command = announced_reaction_command(&a);
    crate::testkit::assert_action_indistinguishable(
        &Cna::full(),
        &c,
        &a,
        &b,
        &command,
        Side::Commonwealth,
    );
    let t = evaluate(&Cna::full(), &c, &b, &command).unwrap();
    assert_eq!(
        t.game
            .state
            .land
            .reaction
            .window
            .as_ref()
            .unwrap()
            .cpa_options[&child],
        BTreeMap::from([(20, false), (40, true)])
    );
    let slower = change_mover_cpa(&a, 46);
    let t = evaluate(&Cna::full(), &c, &slower, &command).unwrap();
    assert_eq!(
        t.game
            .state
            .land
            .reaction
            .window
            .as_ref()
            .unwrap()
            .cpa_options[&child],
        BTreeMap::from([(20, false), (40, false)])
    );
}

/// The advertised schema and serde defaults agree, including optional nulls and nested text.
/// Cases: land:8.56, land:8.95, airlog:49.16
#[test]
fn reaction_division_space_accepts_defaults_and_rejects_unadvertised_history() {
    let (c, g, _, child) = truck_reaction_fixture(10);
    let t = evaluate(&Cna::full(), &c, &g, &announced_reaction_command(&g)).unwrap();
    let request = Cna::full()
        .pending(&c, &t.game.state)
        .into_iter()
        .find(|p| {
            p.kind == super::super::reaction::KIND
                && p.seat == SeatId::new(Side::Commonwealth, Role::FrontLine)
        })
        .unwrap();
    let division = super::super::trucks::reachable_divisions(&c, &t.game.state, &child, true)[&40]
        .clone()
        .unwrap();
    let mut answer = json!([{"unit":child,"path":["C4023"],"with_stack":null,"close_assault":null,"truck_division":division}]);
    answer[0]["truck_division"]["transfers"][0]["cargo"]["heavy"] = Value::Null;
    assert!(request.space.schema.check(&answer).is_ok());
    assert!(respond(&c, &t.game, request.seat, answer.clone(), true).is_ok());
    let original = serde_json::to_value(&t.game).unwrap();
    let mut unknown = answer.clone();
    unknown[0]["truck_division"]["transfers"][0]["past_paid_cp"] = json!(0);
    assert!(request.space.schema.check(&unknown).is_err());
    assert!(respond(&c, &t.game, request.seat, unknown, true).is_err());
    let mut empty = answer.clone();
    empty[0]["truck_division"]["transfers"][0]["cohorts"][0]["id"] = json!("");
    assert!(respond(&c, &t.game, request.seat, empty, true).is_err());
    let mut missing = answer;
    missing[0]["truck_division"]["transfers"][0]
        .as_object_mut()
        .unwrap()
        .remove("from");
    assert!(respond(&c, &t.game, request.seat, missing, true).is_err());
    assert_eq!(serde_json::to_value(&t.game).unwrap(), original);
}

/// Cases: land:3.62, land:8.13, land:8.65, land:13.21, land:19.12
#[test]
fn raw_counter_paths_are_exact_for_all_perspectives_and_attached_contents_stay_private() {
    let (c, mut s, overlay) = setup(LEG, Some("road"), false, None);
    let other = "cw.2_nz_div.21st_nz_bn";
    let attached = "cw.2_nz_div.22nd_nz_bn";
    for id in [other, attached] {
        place(&mut s, id, "C4020");
    }
    s.land.units.get_mut(&attached.into()).unwrap().attached_to = Some(LEG.into());
    s.land.units.get_mut(&attached.into()).unwrap().detached = false;
    assert!(view::is_map_counter(&c, &s, &s.land.units[&LEG.into()]));
    assert!(view::is_map_counter(&c, &s, &s.land.units[&other.into()]));
    assert!(!view::is_map_counter(
        &c,
        &s,
        &s.land.units[&attached.into()]
    ));
    let g = start(&c, s, true);
    let action = json!([{"unit":LEG,"with_stack":true,"path":["C4021","C4022"]}]);
    let primary = respond(&c, &g, seat(&g), action.clone(), true).unwrap();
    let mut nonphasing = g.state.clone();
    let mut rng = CampaignRng::from_state(&g.rng);
    let mut retreat_events = vec![];
    execute_nonphasing(
        &c,
        &mut nonphasing,
        &Order {
            unit: LEG.into(),
            path: vec!["C4021".into(), "C4022".into()],
            with_stack: true,
            close_assault: vec![],
        },
        seat(&g),
        true,
        NonPhasingMove::Retreat,
        &mut Cx {
            rng: &mut rng,
            events: &mut retreat_events,
        },
    )
    .unwrap();
    for (events, state) in [
        (&primary.events, &primary.game.state),
        (&retreat_events, &nonphasing),
    ] {
        for id in [LEG, other, attached] {
            assert_eq!(
                state.land.units[&id.into()].location.hex(),
                Some(&"C4022".into())
            );
        }
        for perspective in Perspective::all() {
            let mut paths = events
                .iter()
                .filter(|e| perspective.can_see(&e.audience))
                .filter_map(|e| match &e.event {
                    GameEvent::UnitMoved {
                        unit_id,
                        path,
                        cp_spent,
                    } => Some((unit_id.as_str(), path.clone(), *cp_spent)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            paths.sort();
            let enemy = perspective.side() == Some(Side::Axis);
            let mut expected = if enemy {
                vec![LEG, other]
            } else {
                vec![LEG, other, attached]
            }
            .into_iter()
            .map(|id| {
                (
                    id,
                    vec!["C4021".to_string(), "C4022".to_string()],
                    if enemy { None } else { Some(2) },
                )
            })
            .collect::<Vec<_>>();
            expected.sort();
            assert_eq!(paths, expected, "{perspective}");
        }
    }
    // Optional scratch export supplies the persistence owner an exact public-engine fixture.
    if let Some(dir) = std::env::var_os("CNA_LAND_FOW_FIXTURE_DIR") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let request = &g.state.decisions.pending[0];
        let response = DecisionResponse {
            decision_id: request.id.clone(),
            seat: request.seat,
            controller_epoch: 1,
            decision_revision: request.revision,
            idempotency_key: "raw-counter-path".into(),
            action,
            public_explanation: None,
        };
        std::fs::write(
            dir.join("fixture.json"),
            serde_json::to_vec_pretty(&json!({
                "game":g,"response":response,"expected_game":primary.game,
                "expected_events":primary.events,"visible":[LEG,other],"attached":attached,
            }))
            .unwrap(),
        )
        .unwrap();
        let map_dir = dir.join("map");
        std::fs::create_dir_all(&map_dir).unwrap();
        for file in std::fs::read_dir(&overlay.dir).unwrap() {
            let file = file.unwrap();
            if file.file_type().unwrap().is_file() {
                std::fs::copy(file.path(), map_dir.join(file.file_name())).unwrap();
            }
        }
    }
}

/// Cases: airlog:51.22, land:3.62
#[test]
fn profiling_chooser_allocates_mandatory_attrition_through_the_real_baseline() {
    let (c, mut s, _overlay) = setup(LEG, Some("road"), false, None);
    let r = s.logistics.rations.get_mut(&LEG.into()).unwrap();
    r.finalized_gt = Some(s.cursor.game_turn);
    r.last_short_gt = Some(s.cursor.game_turn);
    r.consecutive_short_gt = 100;
    let mut game_rng = CampaignRng::from_seed([16; 32]);
    let mut events = vec![];
    logistics::attrition::enter(
        &c,
        &mut s,
        &mut Cx {
            rng: &mut game_rng,
            events: &mut events,
        },
    )
    .unwrap();
    let pending = s
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == logistics::attrition::KIND && p.seat.side == Side::Commonwealth)
        .unwrap()
        .clone();
    let request = Cna::dev()
        .pending(&c, &s)
        .into_iter()
        .find(|r| r.id == pending.id)
        .unwrap();
    assert!(request.space.pass.is_none());
    let before = serde_json::to_value(&s.land.units).unwrap();
    let mut controller = CampaignRng::from_seed([42; 32]);
    let action = profiling_answer(&c, &s, &request, &mut controller);
    assert!(action.as_array().is_some_and(|items| !items.is_empty()));
    let g = Game {
        state: s,
        rng: game_rng.state(),
    };
    let original = serde_json::to_value(&g).unwrap();
    let accepted = respond(&c, &g, request.seat, action.clone(), false).unwrap();
    assert_eq!(
        accepted.game.state.logistics.attrition_window.submitted[&Side::Commonwealth],
        action
    );
    assert_eq!(
        serde_json::to_value(&accepted.game.state.land.units).unwrap(),
        before
    );
    assert_eq!(serde_json::to_value(&g).unwrap(), original);
}

const REJOINING_CHILD: &str = "cw.2_nz_div.21st_nz_bn";
const REJOINING_PARENT: &str = "cw.2_nz_div.5th_new_zealand_bde_hq";
const REJOINING_RIDER: &str = "cw.2_nz_div.22nd_nz_bn";

fn rejoining_counter_fixture() -> (CnaContent, State, Overlay) {
    let (c, mut s, overlay) = setup(REJOINING_CHILD, Some("road"), false, None);
    assert_eq!(
        c.units.units[&REJOINING_CHILD.into()].parent.as_ref(),
        Some(&REJOINING_PARENT.into())
    );
    s.land
        .units
        .get_mut(&REJOINING_CHILD.into())
        .unwrap()
        .detached = false;
    place(&mut s, REJOINING_PARENT, "C4022");
    place(&mut s, REJOINING_RIDER, "C4020");
    let rider = s.land.units.get_mut(&REJOINING_RIDER.into()).unwrap();
    rider.detached = false;
    rider.attached_to = Some(REJOINING_CHILD.into());
    assert!(view::is_map_counter(
        &c,
        &s,
        &s.land.units[&REJOINING_CHILD.into()]
    ));
    assert!(!view::is_map_counter(
        &c,
        &s,
        &s.land.units[&REJOINING_RIDER.into()]
    ));
    (c, s, overlay)
}

fn assert_rejoining_paths(events: &[EngineEvent], path: &[&str], cp: i32) {
    for perspective in Perspective::all() {
        let enemy = perspective.side() == Some(Side::Axis);
        let mut actual = events
            .iter()
            .filter(|e| perspective.can_see(&e.audience))
            .filter_map(|e| match &e.event {
                GameEvent::UnitMoved {
                    unit_id,
                    path,
                    cp_spent,
                } => {
                    if enemy {
                        assert_eq!(e.hex.as_deref(), Some("C4022"));
                        assert_eq!(e.unit_id.as_deref(), Some(REJOINING_CHILD));
                    }
                    Some((unit_id.as_str(), path.clone(), *cp_spent))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        actual.sort();
        let mut expected = if enemy {
            vec![REJOINING_CHILD]
        } else {
            vec![REJOINING_CHILD, REJOINING_RIDER]
        }
        .into_iter()
        .map(|id| {
            (
                id,
                path.iter().map(|h| h.to_string()).collect::<Vec<_>>(),
                if enemy { None } else { Some(cp) },
            )
        })
        .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(
            actual, expected,
            "{perspective}: visible start survives rejoining; hidden rider stays private"
        );
    }
}

/// Cases: land:3.62, land:8.13, land:8.65, land:19.12
#[test]
fn visible_child_rejoining_parent_retains_path_in_ordinary_and_nonphasing_moves() {
    let (c, s, _overlay) = rejoining_counter_fixture();
    let parent_before = serde_json::to_value(&s.land.units[&REJOINING_PARENT.into()]).unwrap();
    let g = start(&c, s, false);
    let action = json!([{"unit":REJOINING_CHILD,"with_stack":true,"path":["C4021","C4022"]}]);
    let primary = respond(&c, &g, seat(&g), action, false).unwrap();
    let mut nonphasing = g.state.clone();
    let mut rng = CampaignRng::from_state(&g.rng);
    let mut events = vec![];
    execute_nonphasing(
        &c,
        &mut nonphasing,
        &Order {
            unit: REJOINING_CHILD.into(),
            path: vec!["C4021".into(), "C4022".into()],
            with_stack: true,
            close_assault: vec![],
        },
        seat(&g),
        false,
        NonPhasingMove::Retreat,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    for (events, state) in [
        (&primary.events, &primary.game.state),
        (&events, &nonphasing),
    ] {
        assert_eq!(
            serde_json::to_value(&state.land.units[&REJOINING_PARENT.into()]).unwrap(),
            parent_before
        );
        for id in [REJOINING_CHILD, REJOINING_RIDER] {
            assert_eq!(
                state.land.units[&id.into()].location.hex(),
                Some(&"C4022".into())
            );
            assert!(!view::is_map_counter(
                &c,
                state,
                &state.land.units[&id.into()]
            ));
        }
        assert!(!state.land.units[&REJOINING_CHILD.into()].detached);
        assert_rejoining_paths(events, &["C4021", "C4022"], 2);
    }
}

/// Cases: land:3.62, land:8.13, land:8.51, land:8.52
#[test]
fn reaction_continuation_retains_visible_start_path_when_child_rejoins_parent() {
    let (c, mut s, _overlay) = rejoining_counter_fixture();
    // A disclosed but incapable enemy still opens the fixed reaction roles.
    // All choices are declared forced passes; eligibility does not govern scheduling.
    place(&mut s, TANK, "C4122");
    s.land
        .units
        .get_mut(&TANK.into())
        .unwrap()
        .cohesion_quarters = -104;
    let parent_before = serde_json::to_value(&s.land.units[&REJOINING_PARENT.into()]).unwrap();
    let g = start(&c, s, false);
    let mover = seat(&g);
    let interrupted = respond(
        &c,
        &g,
        mover,
        json!([{"unit":REJOINING_CHILD,"with_stack":true,"path":["C4021","C4022"]}]),
        false,
    )
    .unwrap();
    assert_eq!(
        interrupted.game.state.land.units[&REJOINING_CHILD.into()]
            .location
            .hex(),
        Some(&"C4021".into())
    );
    assert_eq!(interrupted.game.state.decisions.pending.len(), 3);
    assert!(
        interrupted
            .game
            .state
            .decisions
            .pending
            .iter()
            .all(|p| p.kind == super::super::reaction::KIND
                && p.space.context.as_ref().unwrap()["forced_pass"] == true)
    );
    let continued = decline_empty_reactions(&c, interrupted, false);
    assert_eq!(
        continued.game.state.decisions.pending[0].kind,
        super::super::reaction::CONTINUE
    );
    // Resume through the real dispatcher from a serialized interruption boundary.
    let recovered: Game<Cna> =
        serde_json::from_value(serde_json::to_value(&continued.game).unwrap()).unwrap();
    assert!(view::is_map_counter(
        &c,
        &recovered.state,
        &recovered.state.land.units[&REJOINING_CHILD.into()]
    ));
    let resumed = respond(
        &c,
        &recovered,
        mover,
        json!([{"unit":REJOINING_CHILD,"with_stack":true,"path":["C4022"]}]),
        false,
    )
    .unwrap();
    assert_rejoining_paths(&resumed.events, &["C4022"], 1);
    assert_eq!(
        serde_json::to_value(&resumed.game.state.land.units[&REJOINING_PARENT.into()]).unwrap(),
        parent_before
    );
    assert!(!view::is_map_counter(
        &c,
        &resumed.game.state,
        &resumed.game.state.land.units[&REJOINING_CHILD.into()]
    ));
    assert!(!view::is_map_counter(
        &c,
        &resumed.game.state,
        &resumed.game.state.land.units[&REJOINING_RIDER.into()]
    ));
    assert_eq!(
        resumed.game.state.land.units[&REJOINING_CHILD.into()].cp_spent_quarters,
        8
    );
    let done = decline_empty_reactions(&c, resumed, false);
    assert_eq!(
        done.game.state.decisions.pending[0].kind,
        super::super::reaction::CONTINUE
    );
    let done = respond(&c, &done.game, mover, Value::Null, false).unwrap();
    assert!(done.game.state.land.reaction.continuation.is_none());
    assert!(
        done.events
            .iter()
            .all(|e| !matches!(e.event, GameEvent::UnitMoved { .. }))
    );
}
