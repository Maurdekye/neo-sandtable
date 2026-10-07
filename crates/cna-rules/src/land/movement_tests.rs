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
fn complete_move_reopens_for_other_units_and_enemy_receives_only_presence() {
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
    assert!(observed.iter().all(|e|matches!(&e.event,GameEvent::StackUpdated {stack} if stack.unit_ids.is_empty() && stack.visible_count.is_none()) || matches!(&e.event,GameEvent::StackRemoved {..})));
    assert!(!observed.is_empty());
    assert!(Cna::dev().inspect(&c, &t.game.state, enemy, TANK).is_err());
    let t = respond(&c, &t.game, seat(&t.game), Value::Null, true).unwrap();
    assert!(t.game.state.decisions.pending.is_empty());
}
/// Cases: land:8.37, airlog:49.12
/// Interpretations: interp:units-0005
#[test]
fn unknown_surface_and_unknown_hq_rates_follow_profiles() {
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
            .contains("units-0005")
    );
    assert!(
        matches!(respond(&c,&g,seat(&g),action,true),Err(Rejection::Engine(EngineError::Unsupported {case,..})) if case=="airlog:49.12")
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
    for n in 0..1000 {
        let t = evaluate(&rules, &c, &g, &Command::Advance).unwrap();
        g = t.game;
        if matches!(t.progress, Some(Progress::Finished { .. })) {
            assert!(moves > 10);
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
    panic!("campaign did not finish");
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
        let action = if request.kind == KIND {
            let action = crate::baseline::random_orders(&c, &g.state, &request, &mut rng);
            moves += action.as_array().unwrap().len();
            action
        } else if matches!(request.space.schema, ActionSchema::Choice { .. })
            || request.space.pass.is_none()
        {
            mandatory_profile_answer(&request.space.schema)
        } else {
            Value::Null
        };
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
        assert!(
            Cna { strict }
                .inspect(&c, &g.state, Perspective::Side(Side::Commonwealth), TANK)
                .is_err()
        );
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
fn cycle_proximity_counts_combat_and_is_captured_before_combat_changes_positions() {
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
    assert!(s.land.movement.cycle_blocked.contains(&TANK.into())); // a nearby HQ alone does not qualify
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
