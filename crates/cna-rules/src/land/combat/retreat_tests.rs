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

/// Each represented member pays its own engaged/contact departure before its own cap is checked.
/// Cases: land:13.22,land:13.23,land:13.24
#[test]
fn nonadjacent_cap_uses_each_attached_or_stacked_members_preview_delta() {
    const ROOT: &str = "it.1_libyan_div.1st_libyan_regt_hq";
    const CHILD: &str = "it.1_libyan_div.viii_libyan_bn";
    for with_stack in [false, true] {
        let (c, mut g, _overlay) = fixture();
        place(&mut g.state, LEG, "C4030");
        g.state.turn.player_a = Some(Side::Commonwealth);
        place(&mut g.state, ROOT, "C4020");
        place(&mut g.state, CHILD, "C4020");
        for id in [ROOT, CHILD] {
            let fuel = crate::logistics::fuel_capacity(&c, &g.state, &id.into()).unwrap();
            g.state
                .logistics
                .unit_supply
                .get_mut(&id.into())
                .unwrap()
                .tank_fuel = fuel;
        }
        let child = g.state.land.units.get_mut(&CHILD.into()).unwrap();
        if !with_stack {
            child.detached = false;
            child.attached_to = Some(ROOT.into());
        }
        crate::land::engagement::engage(&mut g.state, &[CHILD.into()], &[LEG.into()]).unwrap();
        let g = open_game(&c, &g);
        let owner = SeatId::new(
            Side::Axis,
            ownership::seat_for_unit(&c, &g.state, &ROOT.into()),
        );
        let mut order = Order::new(ROOT.into(), vec!["C4021".into(), "C4022".into()]);
        order.with_stack = with_stack;
        let selected = members(&c, &g.state, owner, &order);
        assert!(selected.contains(&CHILD.into()));
        let (preview, cost) = movement::preview_nonphasing(
            &c,
            &g.state,
            &order,
            owner,
            false,
            NonPhasingMove::Retreat,
        )
        .unwrap();
        let child_delta = preview.land.units[&CHILD.into()].cp_spent_quarters
            - g.state.land.units[&CHILD.into()].cp_spent_quarters;
        assert!(cost.cp_quarters <= 16, "root must pass the old check");
        assert!(child_delta > 16, "child must fail its own cap");
        let before = serde_json::to_value(&g).unwrap();
        assert!(
            evaluate(
                &Cna::dev(),
                &c,
                &g,
                &command(&c, &g, owner, json!([order.clone()]))
            )
            .is_err()
        );
        assert_eq!(serde_json::to_value(&g).unwrap(), before);
        if !with_stack {
            assert!(
                !reachable(&c, &g.state, &ROOT.into(), false)
                    .iter()
                    .any(|p| p.path == order.path)
            );
        }
        // A one-hex retreat is still explicitly legal even when engaged departure alone uses 4CP.
        order.path.truncate(1);
        let accepted = answer(&c, &g, owner, json!([order]));
        assert_eq!(accepted.rng, g.rng);
        assert_eq!(
            accepted.state.land.units[&CHILD.into()].cp_spent_quarters,
            0
        );
    }
}

/// Loss allocation holds later accepted retreats; the durable cursor resumes without rerunning motion.
/// Cases: land:13.21,land:13.28,land:21.24,land:21.31,land:21.43
#[test]
fn mandatory_breakdown_parks_later_retreats_and_checkpoint_resumes_the_saved_order() {
    let (c, mut g, _overlay) = fixture();
    let later = g
        .state
        .units_of(Side::Commonwealth)
        .find(|u| {
            u.id.as_str() != LEG
                && formation::class(&c, &u.id).is_some_and(|cl| cl.unit_type == "infantry")
                && ownership::seat_for_unit(&c, &g.state, &u.id) == Role::FrontLine
        })
        .unwrap()
        .id
        .clone();
    place(&mut g.state, later.as_str(), "C4030");
    let first = g.state.land.units.get_mut(&LEG.into()).unwrap();
    first.trucks.medium = 10;
    first.transport_trucks = Default::default();
    for id in [UnitId::new(LEG), later.clone()] {
        let cap = crate::logistics::fuel_capacity(&c, &g.state, &id).unwrap();
        g.state
            .logistics
            .unit_supply
            .get_mut(&id)
            .unwrap()
            .tank_fuel = cap;
    }
    // An explicit prior travelled-edge exposure forces the interruption independently of the die face.
    crate::land::breakdown::record_edge(
        &mut g.state,
        &LEG.into(),
        &"C4020".into(),
        280,
        8,
        cna_tables::land::weather::WeatherKind::Normal,
    )
    .unwrap();
    let g = open_game(&c, &g);
    let g = answer(
        &c,
        &g,
        seat(),
        json!([
            Order::new(LEG.into(), vec!["C4021".into()]),
            Order::new(later.clone(), vec!["C4031".into()]),
        ]),
    );
    let mut g = close(&c, g);
    let events = finish(&c, &mut g);
    assert!(
        g.state
            .decisions
            .pending
            .iter()
            .any(|p| p.kind == crate::land::breakdown::window::KIND)
    );
    assert_eq!(g.state.land.combat.retreat.next_order, 1);
    assert!(!g.state.land.combat.retreat.resolved);
    assert_eq!(
        g.state.land.units[&later].location.hex(),
        Some(&"C4030".into())
    );
    assert_eq!(g.state.land.units[&later].cp_spent_quarters, 0);
    let private_choices: Vec<_> = events.iter().filter(|e| matches!(&e.event, GameEvent::DecisionOpened { decision } if decision.kind == crate::land::breakdown::window::KIND)).collect();
    assert!(!private_choices.is_empty());
    assert!(
        private_choices
            .iter()
            .all(|e| !Perspective::Side(Side::Axis).can_see(&e.audience))
    );
    let first_cp = g.state.land.units[&LEG.into()].cp_spent_quarters;
    let serialized = serde_json::to_value(&g).unwrap();
    let run = |mut game: Game<Cna>| {
        let mut events = vec![];
        let mut local = CampaignRng::from_seed([9; 32]);
        for _ in 0..16 {
            if game.state.land.combat.retreat.resolved {
                break;
            }
            for r in Cna::dev().pending(&c, &game.state) {
                assert_eq!(r.kind, crate::land::breakdown::window::KIND);
                assert!(r.space.pass.is_none());
                let action = crate::baseline::random_breakdown(&c, &game.state, &r, &mut local);
                assert!(
                    !action.is_null(),
                    "mandatory baseline must supply a real legal allocation"
                );
                let command = Command::Respond(DecisionResponse {
                    decision_id: r.id.clone(),
                    seat: r.seat,
                    controller_epoch: 1,
                    decision_revision: r.revision,
                    idempotency_key: r.id.to_string(),
                    action,
                    public_explanation: None,
                });
                let before = game.rng.clone();
                let batch = evaluate(&Cna::dev(), &c, &game, &command).unwrap();
                assert_eq!(batch.game.rng, before);
                game = batch.game;
            }
            events.extend(finish(&c, &mut game));
        }
        assert!(game.state.land.combat.retreat.resolved);
        assert_eq!(game.state.land.combat.retreat.next_order, 2);
        assert_eq!(
            game.state.land.units[&LEG.into()].cp_spent_quarters,
            first_cp
        );
        assert_eq!(
            game.state.land.units[&later].location.hex(),
            Some(&"C4031".into())
        );
        assert!(game.state.land.units[&later].cp_spent_quarters > 0);
        let before = serde_json::to_value(&game).unwrap();
        assert!(finish(&c, &mut game).is_empty());
        assert_eq!(serde_json::to_value(&game).unwrap(), before);
        (
            serde_json::to_value(game).unwrap(),
            serde_json::to_value(events).unwrap(),
        )
    };
    assert_eq!(run(g), run(serde_json::from_value(serialized).unwrap()));
}

/// Private breakdown paperwork may stutter, but never changes the other side's ordered stream.
/// Cases: land:3.6,land:13.21,land:21.22,land:21.24,land:21.25
#[test]
fn rba_private_breakdown_rounds_preserve_observer_sequence_through_assignment() {
    let (c, mut a, _overlay) = fixture();
    let anchors: Vec<_> = a
        .state
        .units_of(Side::Commonwealth)
        .filter(|u| {
            u.id.as_str() != LEG
                && formation::class(&c, &u.id).is_some_and(|cl| cl.unit_type == "infantry")
        })
        .take(2)
        .map(|u| u.id.clone())
        .collect();
    assert_eq!(anchors.len(), 2);
    // Existing public stacks at both endpoints hide no newly disclosed movement/marker presence.
    place(&mut a.state, anchors[0].as_str(), "C4020");
    place(&mut a.state, anchors[1].as_str(), "C4021");
    a.state
        .land
        .units
        .get_mut(&LEG.into())
        .unwrap()
        .trucks
        .medium = 10;
    let cap = crate::logistics::fuel_capacity(&c, &a.state, &LEG.into()).unwrap();
    a.state
        .logistics
        .unit_supply
        .get_mut(&LEG.into())
        .unwrap()
        .tank_fuel = cap;
    let mut b = a.clone();
    // Only hidden prior exposure differs. One world needs real mandatory loss allocations.
    crate::land::breakdown::record_edge(
        &mut b.state,
        &LEG.into(),
        &"C4020".into(),
        280,
        8,
        cna_tables::land::weather::WeatherKind::Normal,
    )
    .unwrap();
    crate::testkit::assert_indistinguishable(&Cna::dev(), &c, &a.state, &b.state, Side::Axis);
    let run = |mut g: Game<Cna>| {
        let mut streams: BTreeMap<String, Vec<Value>> =
            std::iter::once(Perspective::Side(Side::Axis))
                .chain(
                    Role::ALL
                        .into_iter()
                        .map(|r| Perspective::Seat(SeatId::new(Side::Axis, r))),
                )
                .map(|p| (p.to_string(), vec![]))
                .collect();
        let mut requests = vec![];
        let mut record = |events: &[EngineEvent]| {
            for (p, rows) in &mut streams {
                let p: Perspective = p.parse().unwrap();
                rows.extend(
                    events
                        .iter()
                        .filter(|e| p.can_see(&e.audience))
                        .map(|e| serde_json::to_value(e).unwrap()),
                );
            }
        };
        let opened = evaluate(&Cna::dev(), &c, &g, &Command::Advance).unwrap();
        record(&opened.events);
        g = opened.game;
        let cmd = command(&c, &g, seat(), plan(&["C4021"]));
        let answer = evaluate(&Cna::dev(), &c, &g, &cmd).unwrap();
        record(&answer.events);
        g = answer.game;
        while let Some(p) = g.state.decisions.pending.first() {
            let cmd = command(&c, &g, p.seat, Value::Null);
            let answer = evaluate(&Cna::dev(), &c, &g, &cmd).unwrap();
            record(&answer.events);
            g = answer.game;
        }
        let mut loss_rounds = 0;
        let mut rng = CampaignRng::from_seed([9; 32]);
        for _ in 0..32 {
            let advanced = evaluate(&Cna::dev(), &c, &g, &Command::Advance).unwrap();
            record(&advanced.events);
            g = advanced.game;
            let pending = Cna::dev().pending(&c, &g.state);
            requests.extend(
                pending
                    .iter()
                    .filter(|r| r.seat.side == Side::Axis)
                    .map(|r| serde_json::to_value(r).unwrap()),
            );
            if pending
                .iter()
                .any(|r| r.kind == crate::land::combat::assignment::KIND)
            {
                return (streams, requests, loss_rounds);
            }
            assert!(!pending.is_empty());
            for r in pending {
                assert_eq!(r.kind, crate::land::breakdown::window::KIND);
                let action = crate::baseline::random_breakdown(&c, &g.state, &r, &mut rng);
                assert!(!action.is_null());
                let command = Command::Respond(DecisionResponse {
                    decision_id: r.id.clone(),
                    seat: r.seat,
                    controller_epoch: 1,
                    decision_revision: r.revision,
                    idempotency_key: r.id.to_string(),
                    action,
                    public_explanation: None,
                });
                let answer = evaluate(&Cna::dev(), &c, &g, &command).unwrap();
                record(&answer.events);
                g = answer.game;
                loss_rounds += 1;
            }
        }
        panic!("retreat loss rounds did not finish");
    };
    let (stream_a, pending_a, rounds_a) = run(a);
    let (stream_b, pending_b, rounds_b) = run(b);
    assert_eq!(rounds_a, 0);
    assert!(rounds_b > 0);
    assert_eq!(
        stream_a, stream_b,
        "private paperwork changed observer event sequence"
    );
    assert_eq!(
        pending_a, pending_b,
        "private paperwork changed observer own requests"
    );
}

/// Cases: land:3.6,land:13.21,land:13.24
#[test]
fn retreat_advance_keeps_all_roles_when_private_pinning_removes_all_eligibility() {
    let (c, a, _overlay) = fixture();
    let mut b = a.clone();
    b.state.land.combat.pinned.insert(LEG.into());
    crate::testkit::assert_action_indistinguishable(
        &Cna::dev(),
        &c,
        &a,
        &b,
        &Command::Advance,
        Side::Axis,
    );
    for g in [a, b] {
        let t = evaluate(&Cna::dev(), &c, &g, &Command::Advance).unwrap();
        assert_eq!(t.game.state.decisions.pending.len(), 3);
        assert!(
            t.game
                .state
                .decisions
                .pending
                .iter()
                .all(|p| p.kind == KIND)
        );
    }
}
