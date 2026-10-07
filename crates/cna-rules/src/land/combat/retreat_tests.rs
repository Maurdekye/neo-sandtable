use super::*;
use crate::{
    Cna,
    seq::{Block, Half},
    state::{Location, UnitSupply},
};
use cna_content::map::{LineKind, MapContent, SideKind};
use cna_core::{
    decision::DecisionResponse,
    engine::{Command, Game, Ruleset, evaluate},
    quantity::{AmmoPoints, WaterPoints},
    visibility::Perspective,
};
use cna_protocol::Side;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
};
const LEG: &str = "cw.unassigned_inf.1st_rnf_mg_bn";
const TANK: &str = "it.libyan_tank_command.xxi_l_tank_bn";
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
            "cna-retreat-test-{}-{}",
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
                .starts_with("cna-retreat-test-")
        );
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
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
            ready_ammo: AmmoPoints::new(10000),
            ..Default::default()
        },
    );
    s.logistics.rations.insert(
        id,
        crate::logistics::Rations {
            water_stage: Some(crate::logistics::water::WaterStage::current(s)),
            infantry_water_received: 2,
            issued_gt: Some(s.cursor.game_turn),
            pasta_gt: Some(s.cursor.game_turn),
            ..Default::default()
        },
    );
}
fn fixture() -> (CnaContent, Game<Cna>, Overlay) {
    let mut c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut s = State::new(&c).unwrap();
    let overlay = Overlay::new(&mut c, None, false, None);
    for u in s.land.units.values_mut() {
        u.location = Location::Eliminated;
    }
    s.turn.player_a = Some(Side::Axis);
    s.turn.weather = Some(crate::state::WeatherState {
        kind: cna_tables::land::weather::WeatherKind::Normal,
        storm_sections: vec![],
    });
    s.cursor.block = Block::PlayerHalf;
    s.cursor.half = Some(Half::A);
    s.cursor.op_stage = Some(1);
    s.cursor.index = crate::seq::PLAYER_HALF
        .iter()
        .position(|step| step.anchor == ANCHOR)
        .unwrap();
    s.cursor.entered = false;
    place(&mut s, LEG, "C4020");
    let g = Game {
        state: s,
        rng: CampaignRng::from_seed([3; 32]).state(),
    };
    (c, g, overlay)
}
fn open_game(c: &CnaContent, g: &Game<Cna>) -> Game<Cna> {
    evaluate(&Cna::dev(), c, g, &Command::Advance).unwrap().game
}
fn seat() -> SeatId {
    SeatId::new(Side::Commonwealth, Role::FrontLine)
}
fn command(c: &CnaContent, g: &Game<Cna>, seat: SeatId, action: Value) -> Command {
    let r = Cna::dev()
        .pending(c, &g.state)
        .into_iter()
        .find(|r| r.kind == KIND && r.seat == seat)
        .unwrap();
    Command::Respond(DecisionResponse {
        decision_id: r.id.clone(),
        seat,
        controller_epoch: 1,
        decision_revision: r.revision,
        idempotency_key: r.id.to_string(),
        action,
        public_explanation: None,
    })
}
fn answer(c: &CnaContent, g: &Game<Cna>, seat: SeatId, action: Value) -> Game<Cna> {
    evaluate(&Cna::dev(), c, g, &command(c, g, seat, action))
        .unwrap()
        .game
}
fn close(c: &CnaContent, mut g: Game<Cna>) -> Game<Cna> {
    while let Some(p) = g.state.decisions.pending.first() {
        let owner = p.seat;
        g = answer(c, &g, owner, Value::Null)
    }
    g
}
fn finish(c: &CnaContent, g: &mut Game<Cna>) -> Vec<EngineEvent> {
    let mut rng = CampaignRng::from_state(&g.rng);
    let mut events = vec![];
    Cna::dev()
        .finish_step(
            c,
            &mut g.state,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
    g.rng = rng.state();
    events
}
fn plan(path: &[&str]) -> Value {
    json!([{"unit":LEG,"path":path}])
}

/// Cases: land:13.21,land:13.24,land:13.28
#[test]
fn answers_checkpoint_without_execution_then_retreat_resolves_exactly_once() {
    let (c, g, _o) = fixture();
    let g = open_game(&c, &g);
    let initial = g.clone();
    let g = answer(&c, &g, seat(), plan(&["C4021", "C4022"]));
    assert_eq!(
        g.state.land.units[&LEG.into()].location.hex(),
        Some(&"C4020".into())
    );
    assert_eq!(g.state.land.units[&LEG.into()].cp_spent_quarters, 0);
    assert_eq!(g.rng, initial.rng);
    assert_eq!(
        serde_json::to_value(&g.state.logistics).unwrap(),
        serde_json::to_value(&initial.state.logistics).unwrap()
    );
    let g = close(&c, g);
    assert!(g.state.land.combat.retreat.closed);
    let mut recovered: Game<Cna> =
        serde_json::from_value(serde_json::to_value(&g).unwrap()).unwrap();
    finish(&c, &mut recovered);
    assert_eq!(
        recovered.state.land.units[&LEG.into()].location.hex(),
        Some(&"C4022".into())
    );
    assert_eq!(
        recovered.state.land.units[&LEG.into()].cp_spent_quarters,
        16
    );
    assert!(
        recovered
            .state
            .land
            .combat
            .retreat
            .retreated
            .contains(&LEG.into())
    );
    assert_eq!(recovered.rng, g.rng);
    let done = serde_json::to_value(&recovered).unwrap();
    assert!(finish(&c, &mut recovered).is_empty());
    assert_eq!(serde_json::to_value(&recovered).unwrap(), done);
}
/// Cases: land:13.1,land:13.24
#[test]
fn bad_final_order_rejects_without_cp_stock_or_rng_mutation() {
    let (c, g, _o) = fixture();
    let g = open_game(&c, &g);
    let before = serde_json::to_value(&g).unwrap();
    for action in [
        plan(&["C4021", "C4022", "C4023"]),
        json!([{"unit":LEG,"path":["C4021"]},{"unit":LEG,"path":["C4022"]}]),
        json!([{"unit":LEG,"path":["C4021"],"close_assault":["C4022"]}]),
    ] {
        assert!(evaluate(&Cna::dev(), &c, &g, &command(&c, &g, seat(), action)).is_err());
        assert_eq!(serde_json::to_value(&g).unwrap(), before);
    }
}
/// Cases: land:13.22,land:13.24
#[test]
fn engaged_one_hex_exception_preserves_four_cp_breakoff() {
    let (c, mut g, _o) = fixture();
    g.state.land.units.get_mut(&LEG.into()).unwrap().engaged = true;
    let g = open_game(&c, &g);
    let mut g = close(&c, answer(&c, &g, seat(), plan(&["C4021"])));
    finish(&c, &mut g);
    assert_eq!(g.state.land.units[&LEG.into()].cp_spent_quarters, 24);
}
/// Cases: land:13.22,land:13.26
#[test]
fn actual_contact_pays_two_cp_and_direct_controlled_to_controlled_is_illegal() {
    let (c, mut g, _o) = fixture();
    place(&mut g.state, TANK, "C4019");
    place(
        &mut g.state,
        "it.libyan_tank_command.lxii_l_tank_bn",
        "C4019",
    );
    let g = open_game(&c, &g);
    assert_eq!(
        g.state.land.reaction.controls.get(&"C4020".into()),
        Some(&true)
    );
    let mut valid = close(&c, answer(&c, &g, seat(), plan(&["C4021"])));
    finish(&c, &mut valid);
    assert_eq!(valid.state.land.units[&LEG.into()].cp_spent_quarters, 16);
    let mut controlled = g.clone();
    controlled
        .state
        .land
        .reaction
        .controls
        .insert("C4021".into(), true);
    assert!(
        evaluate(
            &Cna::dev(),
            &c,
            &controlled,
            &command(&c, &controlled, seat(), plan(&["C4021"]))
        )
        .is_err()
    );
}
/// Cases: land:13.0,land:13.1
#[test]
fn fixed_empty_windows_accept_pass_and_exclude_pinned_or_exhausted_units() {
    for pinned in [true, false] {
        let (c, mut g, _o) = fixture();
        if pinned {
            g.state.land.combat.pinned.insert(LEG.into());
        } else {
            g.state
                .land
                .units
                .get_mut(&LEG.into())
                .unwrap()
                .cohesion_quarters = -104;
        }
        let g = open_game(&c, &g);
        let requests = Cna::dev().pending(&c, &g.state);
        assert_eq!(requests.len(), 3);
        assert!(requests.iter().all(|r| r.seat.side == Side::Commonwealth));
        assert!(ids(&c, &g.state, seat()).is_empty());
        assert!(
            evaluate(
                &Cna::dev(),
                &c,
                &g,
                &command(&c, &g, seat(), plan(&["C4021"]))
            )
            .is_err()
        );
        let g = close(&c, g);
        assert!(g.state.land.combat.retreat.closed);
    }
}
/// Cases: land:3.6,land:13.23,land:13.24
/// Interpretations: interp:land-0030
#[test]
fn hidden_adjacent_class_does_not_change_allowance_or_acceptance() {
    let (c, mut a, _o) = fixture();
    place(&mut a.state, "it.1_libyan_div.viii_libyan_bn", "C4019");
    let mut b = a.clone();
    b.state
        .land
        .units
        .get_mut(&"it.1_libyan_div.viii_libyan_bn".into())
        .unwrap()
        .location = Location::Eliminated;
    place(
        &mut b.state,
        "it.1_libyan_div.1st_libyan_infantry_hq",
        "C4019",
    );
    let a = open_game(&c, &a);
    let b = open_game(&c, &b);
    assert!(a.state.land.combat.retreat.units[&LEG.into()].adjacent);
    assert!(b.state.land.combat.retreat.units[&LEG.into()].adjacent);
    crate::testkit::assert_indistinguishable(
        &Cna::dev(),
        &c,
        &a.state,
        &b.state,
        Side::Commonwealth,
    );
    crate::testkit::assert_action_indistinguishable(
        &Cna::dev(),
        &c,
        &a,
        &b,
        &command(&c, &a, seat(), plan(&["C4021", "C4022", "C4023"])),
        Side::Commonwealth,
    );
    let mut moved = a.clone();
    moved
        .state
        .land
        .units
        .get_mut(&"it.1_libyan_div.viii_libyan_bn".into())
        .unwrap()
        .location = Location::Eliminated;
    assert!(
        evaluate(
            &Cna::dev(),
            &c,
            &moved,
            &command(&c, &moved, seat(), plan(&["C4021", "C4022", "C4023"]))
        )
        .is_ok()
    );
}
/// Cases: land:3.6,land:13.21,land:13.28
#[test]
fn plans_and_retreat_markers_are_owner_operator_only() {
    let (c, g, _o) = fixture();
    let g = open_game(&c, &g);
    let mut hidden = g.state.clone();
    hidden
        .land
        .combat
        .retreat
        .plans
        .insert(seat(), vec![Order::new(LEG.into(), vec!["C4021".into()])]);
    hidden.land.combat.retreat.retreated.insert(LEG.into());
    crate::testkit::assert_indistinguishable(&Cna::dev(), &c, &g.state, &hidden, Side::Axis);
    assert_ne!(
        Cna::dev().observe(&c, &g.state, Perspective::Operator),
        Cna::dev().observe(&c, &hidden, Perspective::Operator)
    );
}
/// Cases: land:13.21,land:13.24,land:13.25
#[test]
fn pure_validator_and_multi_seed_baseline_leave_adjudication_rng_untouched() {
    let (c, g, _o) = fixture();
    let g = open_game(&c, &g);
    let before = serde_json::to_value(&g).unwrap();
    let cost = movement::validate_nonphasing(
        &c,
        &g.state,
        &Order::new(LEG.into(), vec!["C4021".into()]),
        seat(),
        false,
        NonPhasingMove::Retreat,
    )
    .unwrap();
    assert_eq!(cost.cp_quarters, 8);
    assert_eq!(serde_json::to_value(&g).unwrap(), before);
    let r = Cna::dev()
        .pending(&c, &g.state)
        .into_iter()
        .find(|r| r.seat == seat())
        .unwrap();
    for seed in 0..32 {
        let a = random_orders(&c, &g.state, &r, &mut CampaignRng::from_seed([seed; 32]));
        let t = evaluate(&Cna::dev(), &c, &g, &command(&c, &g, seat(), a)).unwrap();
        assert_eq!(t.game.rng, g.rng);
    }
    let (c, g, _o) = fixture();
    assert!(
        matches!(evaluate(&Cna::full(),&c,&g,&Command::Advance),Err(Rejection::Engine(EngineError::Unsupported{case,..})) if case=="land:13.25")
    );
}

/// Cases: land:3.6,land:13.21,land:13.28
#[test]
fn owner_plan_is_hidden_during_response_and_role_submission_order_does_not_change_execution() {
    let (c, g, _o) = fixture();
    let g = open_game(&c, &g);
    crate::testkit::assert_actions_indistinguishable(
        &Cna::dev(),
        &c,
        (&g, &command(&c, &g, seat(), plan(&["C4021"]))),
        (&g, &command(&c, &g, seat(), Value::Null)),
        Side::Axis,
    );
    let seats: Vec<_> = g.state.decisions.pending.iter().map(|p| p.seat).collect();
    let run = |seats: Vec<SeatId>| {
        let mut g = g.clone();
        for owner in seats {
            let action = if owner == seat() {
                plan(&["C4021"])
            } else {
                Value::Null
            };
            g = answer(&c, &g, owner, action);
        }
        let events = finish(&c, &mut g);
        (
            serde_json::to_value(g).unwrap(),
            serde_json::to_value(events).unwrap(),
        )
    };
    assert_eq!(run(seats.clone()), run(seats.into_iter().rev().collect()));
}
/// Cases: land:13.24,land:13.26
#[test]
fn disclosed_hostile_destination_allows_one_hex_stop_and_reachable_respects_cap() {
    let (c, g, _o) = fixture();
    let mut g = open_game(&c, &g);
    for path in reachable(&c, &g.state, &LEG.into(), false) {
        assert!(path.cp_quarters <= 16 || path.path.len() == 1);
    }
    g.state.land.reaction.controls.insert("C4021".into(), true);
    assert!(
        evaluate(
            &Cna::dev(),
            &c,
            &g,
            &command(&c, &g, seat(), plan(&["C4021"]))
        )
        .is_ok()
    );
    assert!(
        evaluate(
            &Cna::dev(),
            &c,
            &g,
            &command(&c, &g, seat(), plan(&["C4021", "C4022"]))
        )
        .is_err()
    );
}

/// Cases: land:13.0,land:13.21
#[test]
fn no_public_phasing_player_skips_entry_and_finish_without_fabricating_a_window() {
    let (c, mut g, _o) = fixture();
    g.state.turn.player_a = None;
    let before = serde_json::to_value(&g).unwrap();
    let mut rng = CampaignRng::from_state(&g.rng);
    let mut events = vec![];
    enter(
        &c,
        &mut g.state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        false,
    )
    .unwrap();
    assert!(g.state.decisions.pending.is_empty());
    assert!(!g.state.land.combat.retreat.entered);
    finish(&c, &mut g);
    assert_eq!(serde_json::to_value(&g).unwrap(), before);
    assert!(events.is_empty());
}
