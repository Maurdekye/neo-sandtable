use super::*;
use crate::{
    Cna,
    seq::{Block, Half},
    state::Location,
};
use cna_core::{
    decision::DecisionResponse,
    engine::{Command, Game, Ruleset, evaluate},
    quantity::WaterPoints,
};
use std::sync::OnceLock;
fn content() -> &'static CnaContent {
    static C: OnceLock<CnaContent> = OnceLock::new();
    C.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn game(seed: u8) -> Game<Cna> {
    let c = content();
    let mut s = State::new(c).unwrap();
    let mut guns = vec![];
    for side in Side::ALL {
        let id = available(c, &s, SeatId::new(side, Role::FrontLine))
            .into_iter()
            .find(|id| matches!(s.land.units[id].toe, Some(Toe::Weapons(_))))
            .unwrap();
        guns.push((side, id));
    }
    for u in s.land.units.values_mut() {
        u.location = Location::NotArrived;
        u.trucks = Default::default();
        u.transport_trucks = Default::default();
    }
    for (side, id) in guns {
        s.land.units.get_mut(&id).unwrap().location = Location::Hex {
            hex: if side == Side::Axis { "C4218" } else { "C4219" }.into(),
        };
        let supply = s.logistics.unit_supply.entry(id).or_default();
        supply.ready_ammo = AmmoPoints::new(10000);
        supply.activity_water = WaterPoints::new(10000);
    }
    s.turn.weather = Some(crate::state::WeatherState {
        kind: cna_tables::land::weather::WeatherKind::Normal,
        storm_sections: vec![],
    });
    s.cursor.block = Block::PlayerHalf;
    s.cursor.half = Some(Half::A);
    s.cursor.op_stage = Some(1);
    s.cursor.index = 3;
    s.cursor.entered = false;
    s.turn.player_a = Some(Side::Axis);
    Game {
        state: s,
        rng: CampaignRng::from_seed([seed; 32]).state(),
    }
}
fn command(g: &Game<Cna>, seat: SeatId, kind: &str, a: Value) -> Command {
    let r = Cna::dev()
        .pending(content(), &g.state)
        .into_iter()
        .find(|r| r.seat == seat && r.kind == kind)
        .unwrap();
    Command::Respond(DecisionResponse {
        decision_id: r.id.clone(),
        seat,
        controller_epoch: 1,
        decision_revision: r.revision,
        idempotency_key: r.id.to_string(),
        action: a,
        public_explanation: None,
    })
}
fn answer(g: Game<Cna>, seat: SeatId, kind: &str, a: Value) -> Game<Cna> {
    evaluate(&Cna::dev(), content(), &g, &command(&g, seat, kind, a))
        .unwrap()
        .game
}
fn plots(seed: u8) -> Game<Cna> {
    let mut g = evaluate(&Cna::dev(), content(), &game(seed), &Command::Advance)
        .unwrap()
        .game;
    for seat in seats() {
        g = answer(g, seat, super::super::POSITION_KIND, Value::Null);
    }
    g = evaluate(&Cna::dev(), content(), &g, &Command::Advance)
        .unwrap()
        .game;
    for seat in seats() {
        let hs = hexes(content(), &g.state, seat);
        g = answer(g, seat, DECLARE, json!(hs));
    }
    g
}
fn plan(g: &Game<Cna>, seat: SeatId) -> Value {
    let r = Cna::dev()
        .pending(content(), &g.state)
        .into_iter()
        .find(|r| r.seat == seat && r.kind == PLOT)
        .unwrap();
    random_plans(
        content(),
        &g.state,
        &r,
        &mut CampaignRng::from_seed([3; 32]),
    )
}
/// Cases: land:3.6,land:12.23,land:12.24,land:12.42,land:12.45,airlog:50.13
#[test]
fn simultaneous_real_guns_fire_before_any_losses_and_restore_checkpoint() {
    let mut g = plots(1);
    let initial = g.clone();
    let axis = SeatId::new(Side::Axis, Role::FrontLine);
    let cw = SeatId::new(Side::Commonwealth, Role::FrontLine);
    let ap = plan(&g, axis);
    let cp = plan(&g, cw);
    assert!(!ap.as_array().unwrap().is_empty());
    assert!(!cp.as_array().unwrap().is_empty());
    g = answer(g, axis, PLOT, ap.clone());
    assert_eq!(g.rng, initial.rng);
    assert_eq!(
        json!(g.state.logistics.unit_supply),
        json!(initial.state.logistics.unit_supply)
    );
    let enemy = Cna::dev().observe(content(), &g.state, Perspective::Side(Side::Commonwealth));
    assert_eq!(enemy["combat"]["barrage_plans"], json!({}));
    g = serde_json::from_value(serde_json::to_value(g).unwrap()).unwrap();
    g = answer(g, cw, PLOT, cp.clone());
    let before = serde_json::to_value(&g).unwrap();
    let bad = command(
        &g,
        SeatId::new(Side::Axis, Role::RearArea),
        PLOT,
        json!([{"target":"enemy-secret-designation","guns":[]} ]),
    );
    assert!(evaluate(&Cna::dev(), content(), &g, &bad).is_err());
    assert_eq!(serde_json::to_value(&g).unwrap(), before);
    let mut all_events = vec![];
    for seat in seats().filter(|s| s.role != Role::FrontLine) {
        let t = evaluate(
            &Cna::dev(),
            content(),
            &g,
            &command(&g, seat, PLOT, Value::Null),
        )
        .unwrap();
        all_events.extend(t.events);
        g = t.game;
    }
    assert_eq!(g.state.decisions.pending.len(), 6);
    assert!(g.state.decisions.pending.iter().all(|r| r.kind == LOSSES));
    let rolls: Vec<_> = all_events
        .iter()
        .filter_map(|e| match &e.event {
            GameEvent::DiceRolled { dice, reading, .. } => Some((dice.clone(), *reading)),
            _ => None,
        })
        .collect();
    assert_eq!(rolls.len(), 2);
    // Hand checked on printed Gun row 3-4: 66 destroys one TOE; 36 has no effect.
    // Real 75/27: 6 TOE x rating6 =36 raw ->4 Actual; 3.7in: 6 x7=42 raw ->4.
    assert_eq!(rolls, vec![(vec![6, 6], Some(66)), (vec![3, 6], Some(36))]);
    let cw_gun = UnitId::new("cw.4_indian_div.25th_field_artillery_regt");
    let axis_gun = UnitId::new("it.1_libyan_div.1st_libyan_artillery_regt");
    assert_eq!(g.state.land.combat.barrage.casualties[&cw_gun][0].loss, 1);
    assert_eq!(g.state.land.combat.barrage.casualties[&axis_gun][0].loss, 0);
    assert_eq!(g.state.land.units[&cw_gun].cp_spent_quarters, 12);
    assert_eq!(g.state.land.units[&axis_gun].cp_spent_quarters, 20);
    assert_eq!(
        g.state.logistics.unit_supply[&cw_gun].activity_water.get(),
        9994
    );
    assert_eq!(
        g.state.logistics.unit_supply[&axis_gun]
            .activity_water
            .get(),
        9994
    );
    let mut expected = CampaignRng::from_state(&initial.rng);
    let a = expected.two_dice_reading();
    let b = expected.two_dice_reading();
    assert_eq!(
        rolls,
        vec![
            (vec![a.tens.value(), a.units.value()], Some(a.value())),
            (vec![b.tens.value(), b.units.value()], Some(b.value()))
        ]
    );
    for p in [ap, cp] {
        let fs: Vec<Fire> = serde_json::from_value(p).unwrap();
        for f in fs {
            for gun in f.guns {
                let cost = logistics::ammunition_cost(
                    content(),
                    AmmoMode::Played,
                    AmmoAction::Barrage,
                    ToeStrengthPoints::new(gun.toe),
                )
                .unwrap()
                .get();
                assert_eq!(
                    g.state.logistics.unit_supply[&gun.unit].ready_ammo.get(),
                    10000 - cost
                );
            }
        }
    }

    for seat in seats() {
        let r = Cna::dev()
            .pending(content(), &g.state)
            .into_iter()
            .find(|r| r.seat == seat && r.kind == LOSSES)
            .unwrap();
        let a = random_plans(
            content(),
            &g.state,
            &r,
            &mut CampaignRng::from_seed([1; 32]),
        );
        g = answer(g, seat, LOSSES, a);
    }
    assert!(g.state.decisions.pending.is_empty());
    assert_eq!(strength(content(), &g.state, &cw_gun).unwrap(), 5);
    assert_eq!(strength(content(), &g.state, &axis_gun).unwrap(), 6);
}
/// Cases: land:3.22,land:12.24
#[test]
fn anonymous_ids_change_each_step_and_class_to_unit_assignment_is_shuffled() {
    let mut g = plots(1);
    let axis = Side::Axis;
    let first = g.state.land.combat.barrage.targets[&axis].clone();
    assert!(!first.is_empty());
    g.state.land.combat.barrage.generation += 1;
    let second = catalog(
        content(),
        &g.state,
        axis,
        &mut CampaignRng::from_seed([6; 32]),
    )
    .unwrap();
    for t in first {
        let next = second
            .iter()
            .find(|x| x.unit == t.unit && x.class == t.class)
            .unwrap();
        assert_ne!(t.label, next.label);
        assert!(!t.label.contains(t.unit.as_str()));
    }
    let private = Cna::dev().observe(content(), &g.state, Perspective::Side(axis));
    let text = private["combat"]["barrage_targets"].to_string();
    for u in g.state.units_of(axis.opponent()) {
        assert!(!text.contains(u.id.as_str()));
    }
}
/// Cases: land:12.16,land:12.32,airlog:50.13
#[test]
fn repeated_source_hex_overcommitted_ammo_and_foreign_guns_reject_without_change() {
    let g = plots(1);
    let seat = SeatId::new(Side::Axis, Role::FrontLine);
    let good = plan(&g, seat);
    let before = json!(g);
    let f = good[0].clone();
    let mut dry = f.clone();
    dry["guns"][0]["draws"][0]["ammo"] = json!(10001);
    let mut foreign = f.clone();
    foreign["guns"][0]["unit"] = json!(
        available(
            content(),
            &g.state,
            SeatId::new(Side::Commonwealth, Role::FrontLine)
        )[0]
    );
    for a in [json!([f.clone(), f]), json!([dry]), json!([foreign])] {
        assert!(evaluate(&Cna::dev(), content(), &g, &command(&g, seat, PLOT, a)).is_err());
        assert_eq!(json!(g), before)
    }
}
/// Cases: land:12.46
#[test]
fn truck_gap_stops_full_and_dev_reports_unapplied_losses_without_inventing_cargo() {
    let seed = (0..=255u8)
        .find(|seed| {
            let mut r = CampaignRng::from_seed([*seed; 32]);
            r.two_dice_reading();
            r.two_dice_reading().value() >= 65
        })
        .unwrap();
    let mut g = plots(seed);
    let id = UnitId::new("cw.4_indian_div.25th_field_artillery_regt");
    g.state.land.units.get_mut(&id).unwrap().trucks.light = 2;
    let axis = SeatId::new(Side::Axis, Role::FrontLine);
    let p = plan(&g, axis);
    assert!(!p.as_array().unwrap().is_empty());
    g = answer(g, axis, PLOT, p);
    let rest: Vec<_> = seats().filter(|seat| *seat != axis).collect();
    for seat in &rest[..rest.len() - 1] {
        g = answer(g, *seat, PLOT, Value::Null);
    }
    let last = rest[rest.len() - 1];
    let cmd = command(&g, last, PLOT, Value::Null);
    let mut full = g.clone();
    full.state.land.combat.barrage.strict = true;
    assert!(
        matches!(evaluate(&Cna::full(),content(),&full,&cmd),Err(Rejection::Engine(EngineError::Unsupported{case,..})) if case=="land:12.46")
    );
    let t = evaluate(&Cna::dev(), content(), &g, &cmd).unwrap();
    assert_eq!(t.game.state.land.units[&id].trucks.light, 2);
    assert_eq!(
        t.game.state.logistics.unit_supply[&id].carried,
        g.state.logistics.unit_supply[&id].carried
    );
    assert!(t.events.iter().any(
        |e| matches!(&e.event,GameEvent::Note{text} if text.contains("not been applied"))
            && e.audience == Audience::Side(Side::Commonwealth)
    ));
    assert_eq!(
        t.events
            .iter()
            .filter(|e| matches!(&e.event, GameEvent::DiceRolled { .. }))
            .count(),
        2
    );
}
/// Cases: land:3.6,land:12.24
#[test]
fn label_assignment_is_not_canonical_order_or_a_cross_step_tracking_key() {
    let mut g = plots(1);
    let id=content().units.units.keys().find(|id|id.as_str().starts_with("cw.")&&**id!=UnitId::new("cw.4_indian_div.25th_field_artillery_regt")&&matches!(g.state.land.units.get(*id).and_then(|u|u.toe.as_ref()),Some(Toe::Weapons(ps)) if ps.iter().any(|p|p.n>0))).unwrap().clone();
    g.state.land.units.get_mut(&id).unwrap().location = Location::Hex {
        hex: "C4219".into(),
    };
    let mut associations = BTreeSet::new();
    for seed in 0..32u8 {
        let ts = catalog(
            content(),
            &g.state,
            Side::Axis,
            &mut CampaignRng::from_seed([seed; 32]),
        )
        .unwrap();
        associations.insert(
            ts.into_iter()
                .map(|t| (t.label, t.unit))
                .collect::<Vec<_>>(),
        );
    }
    assert!(associations.len() > 1);
}
