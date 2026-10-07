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
    quantity::FuelTenths,
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
        id,
        UnitSupply {
            tank_fuel: FuelTenths::new(10000),
            ..UnitSupply::default()
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
    let mut g = Game {
        state: s,
        rng: CampaignRng::from_seed([8; 32]).state(),
    };
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
#[ignore = "manual movement profiling on the full Graziani roster"]
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
#[ignore = "manual full-roster campaign timing"]
fn profile_movers_full_graziani_campaign() {
    let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut g: Game<Cna> = Game {
        state: State::new(&c).unwrap(),
        rng: CampaignRng::from_seed([8; 32]).state(),
    };
    let rules = Cna::dev();
    let mut rng = CampaignRng::from_seed([19; 32]);
    let begin = std::time::Instant::now();
    let mut moves = 0;
    for n in 0..10000 {
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
            begin.elapsed().as_secs() < 180,
            "movement campaign benchmark exceeded three minutes"
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
        let action = if request.kind == KIND {
            let action = crate::baseline::random_orders(&c, &g.state, &request, &mut rng);
            moves += action.as_array().unwrap().len();
            action
        } else if let ActionSchema::Choice { options } = &request.space.schema {
            json!(options[0].id)
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
    panic!("profile campaign did not finish");
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
