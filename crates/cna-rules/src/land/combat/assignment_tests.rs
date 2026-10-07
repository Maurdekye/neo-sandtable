use super::*;
use crate::{
    Cna,
    seq::{Block, Half},
    state::{Location, UnitSupply, WeatherState},
};
use cna_core::{
    decision::DecisionResponse,
    engine::{Command, Game, Ruleset, evaluate},
    quantity::WaterPoints,
};
const TANK: &str = "it.libyan_tank_command.i_m_tank_bn";
const INF: &str = "cw.unassigned_inf.1st_rnf_mg_bn";
fn fixture() -> (CnaContent, Game<Cna>) {
    let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut s = State::new(&c).unwrap();
    for u in s.land.units.values_mut() {
        u.location = Location::Eliminated;
    }
    s.turn.player_a = Some(Side::Axis);
    s.turn.weather = Some(WeatherState {
        kind: cna_tables::land::weather::WeatherKind::Normal,
        storm_sections: vec![],
    });
    s.cursor.block = Block::PlayerHalf;
    s.cursor.half = Some(Half::A);
    s.cursor.op_stage = Some(1);
    s.cursor.index = crate::seq::PLAYER_HALF
        .iter()
        .position(|x| x.anchor == ANCHOR)
        .unwrap();
    for (id, hex) in [(TANK, "C4020"), (INF, "C4021")] {
        let unit = s.land.units.get_mut(&id.into()).unwrap();
        unit.location = Location::Hex { hex: hex.into() };
        unit.detached = true;
        unit.attached_to = None;
        s.logistics.unit_supply.insert(
            id.into(),
            UnitSupply {
                ready_ammo: AmmoPoints::new(10000),
                activity_water: WaterPoints::new(10000),
                ..Default::default()
            },
        );
        s.logistics.rations.insert(
            id.into(),
            logistics::Rations {
                water_stage: Some(logistics::water::WaterStage::current(&s)),
                infantry_water_received: 2,
                issued_gt: Some(1),
                pasta_gt: Some(1),
                ..Default::default()
            },
        );
    }
    (
        c,
        Game {
            state: s,
            rng: CampaignRng::from_seed([5; 32]).state(),
        },
    )
}
fn seat(side: Side) -> SeatId {
    SeatId::new(side, Role::FrontLine)
}
fn opened(c: &CnaContent, g: &Game<Cna>) -> Game<Cna> {
    evaluate(&Cna::dev(), c, g, &Command::Advance).unwrap().game
}
fn response(c: &CnaContent, g: &Game<Cna>, side: Side, action: Value) -> Command {
    let r = Cna::dev()
        .pending(c, &g.state)
        .into_iter()
        .find(|r| r.seat == seat(side) && r.kind == KIND)
        .unwrap();
    Command::Respond(DecisionResponse {
        decision_id: r.id.clone(),
        seat: r.seat,
        controller_epoch: 1,
        decision_revision: r.revision,
        idempotency_key: r.id.to_string(),
        action,
        public_explanation: None,
    })
}
fn allocation(
    c: &CnaContent,
    s: &State,
    id: &str,
    role: CombatRole,
    target: &str,
    toe: i32,
) -> Assignment {
    let weapon = components(c, s, &id.into())[0].clone();
    let cost = logistics::ammunition_cost(
        c,
        AmmoMode::Played,
        ammo_action(c, &id.into(), &weapon, role).unwrap(),
        ToeStrengthPoints::new(toe),
    )
    .unwrap()
    .get();
    let attack = role == CombatRole::CloseAssault && phasing(s, s.land.units[&id.into()].side);
    Assignment {
        unit: id.into(),
        weapon,
        role,
        target: target.into(),
        toe,
        draws: vec![Draw {
            source: serde_json::to_string(&logistics::SupplySource::ReadyAmmo).unwrap(),
            ammo: cost,
        }],
        assault: attack.then_some(0),
        probe: attack.then_some(false),
    }
}
fn finish_game(c: &CnaContent, g: &mut Game<Cna>) {
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
    for event in events {
        let id: UnitId = event
            .unit_id
            .as_ref()
            .expect("identified own force unit")
            .as_str()
            .into();
        assert_eq!(event.audience, Audience::Side(g.state.land.units[&id].side));
        assert_eq!(
            event.hex,
            g.state.land.units[&id]
                .location
                .hex()
                .map(ToString::to_string)
        );
        assert!(matches!(event.event, GameEvent::Note { .. }));
    }
    g.rng = rng.state();
}
/// Cases: land:14.11,land:14.26,land:15.16,land:3.6,airlog:50.14
#[test]
fn disjoint_weapon_and_body_partitions_are_private_answer_only_and_checkpointable() {
    let (c, g) = fixture();
    let mut g = opened(&c, &g);
    assert_eq!(g.state.decisions.pending.len(), 2);
    let a = allocation(&c, &g.state, TANK, CombatRole::AntiArmor, "C4021", 1);
    let b = allocation(&c, &g.state, TANK, CombatRole::CloseAssault, "C4021", 1);
    let original = g.clone();
    g = evaluate(
        &Cna::dev(),
        &c,
        &g,
        &response(&c, &g, Side::Axis, json!([a, b])),
    )
    .unwrap()
    .game;
    assert_eq!(g.rng, original.rng);
    assert_eq!(
        serde_json::to_value(&g.state.logistics).unwrap(),
        serde_json::to_value(&original.state.logistics).unwrap()
    );
    assert!(!g.state.land.combat.assignment.frozen);
    let p = partitions(&c, &g.state, Side::Axis);
    assert_eq!(p[0].anti_armor, 1);
    assert_eq!(p[0].close_assault, 1);
    assert_eq!(
        p[0].withheld,
        formation::strength(&c, &g.state, &TANK.into()) - 2
    );
    let own = Cna::dev().observe(&c, &g.state, Perspective::Side(Side::Axis));
    let enemy = Cna::dev().observe(&c, &g.state, Perspective::Side(Side::Commonwealth));
    assert!(
        !own["combat"]["force_assignment"]["plans"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert!(
        !enemy["combat"]["force_assignment"]["plans"]
            .as_object()
            .unwrap()
            .contains_key("axis")
    );
    g = evaluate(
        &Cna::dev(),
        &c,
        &g,
        &response(&c, &g, Side::Commonwealth, Value::Null),
    )
    .unwrap()
    .game;
    assert!(g.state.land.combat.assignment.closed);
    let mut restored: Game<Cna> =
        serde_json::from_value(serde_json::to_value(&g).unwrap()).unwrap();
    finish_game(&c, &mut restored);
    assert!(restored.state.land.combat.assignment.frozen);
    assert_eq!(restored.rng, original.rng);
    assert_eq!(
        serde_json::to_value(&restored.state.logistics).unwrap(),
        serde_json::to_value(&original.state.logistics).unwrap()
    );
    let done = serde_json::to_value(&restored).unwrap();
    finish_game(&c, &mut restored);
    assert_eq!(serde_json::to_value(restored).unwrap(), done);
}
/// Cases: land:14.26,land:14.27,land:15.16,airlog:50.14
#[test]
fn overflow_wrong_component_foreign_units_and_shared_ammunition_reject_atomically() {
    let (c, g) = fixture();
    let g = opened(&c, &g);
    let a = allocation(&c, &g.state, TANK, CombatRole::AntiArmor, "C4021", 1);
    let mut too_many = a.clone();
    too_many.toe = component(&c, &g.state, &a.unit, &a.weapon, true)
        .unwrap()
        .toe;
    let mut wrong = a.clone();
    wrong.weapon = None;
    let mut foreign = a.clone();
    foreign.unit = INF.into();
    let mut target = a.clone();
    target.target = "C4030".into();
    let mut shortage = a.clone();
    shortage.draws[0].ammo = 0;
    let before = serde_json::to_value(&g).unwrap();
    for plan in [
        vec![too_many, a.clone()],
        vec![wrong],
        vec![foreign],
        vec![target],
        vec![shortage],
    ] {
        assert!(
            evaluate(
                &Cna::dev(),
                &c,
                &g,
                &response(&c, &g, Side::Axis, json!(plan))
            )
            .is_err()
        );
        assert_eq!(serde_json::to_value(&g).unwrap(), before);
    }
    // Two components draw from the same owner's ready-ammo pool; a bad final draw cannot reserve the first.
    let mut dry = g.clone();
    dry.state
        .logistics
        .unit_supply
        .get_mut(&TANK.into())
        .unwrap()
        .ready_ammo = AmmoPoints::new(a.draws[0].ammo);
    let before = serde_json::to_value(&dry).unwrap();
    assert!(
        evaluate(
            &Cna::dev(),
            &c,
            &dry,
            &response(&c, &dry, Side::Axis, json!([a.clone(), a]))
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(dry).unwrap(), before);
}
/// Cases: land:12.16,land:12.44,land:13.28,land:14.13,land:15.12
#[test]
fn pinned_retreated_and_back_units_cannot_assign_strength_but_residue_is_not_immunity() {
    let (c, g) = fixture();
    let g = opened(&c, &g);
    let a = allocation(&c, &g.state, INF, CombatRole::CloseAssault, "C4021", 1);
    for marker in 0..3 {
        let mut changed = g.clone();
        match marker {
            0 => {
                changed.state.land.combat.pinned.insert(INF.into());
            }
            1 => {
                changed
                    .state
                    .land
                    .combat
                    .retreat
                    .retreated
                    .insert(INF.into());
            }
            _ => {
                changed
                    .state
                    .land
                    .combat
                    .positions
                    .insert(INF.into(), Position::Back);
            }
        }
        assert!(
            evaluate(
                &Cna::dev(),
                &c,
                &changed,
                &response(&c, &changed, Side::Commonwealth, json!([a.clone()]))
            )
            .is_err()
        );
        assert!(partitions(&c, &changed.state, Side::Commonwealth)[0].withheld > 0);
    }
}
/// Cases: land:15.25,land:15.91
/// Interpretations: interp:land-0010
#[test]
fn probe_threshold_counts_withheld_points_and_all_assaults_once_per_unit() {
    let (c, mut g) = fixture();
    assert_eq!(formation::strength(&c, &g.state, &TANK.into()), 7);
    let mut a = allocation(&c, &g.state, TANK, CombatRole::CloseAssault, "C4020", 2);
    a.assault = Some(0);
    let mut b = allocation(&c, &g.state, TANK, CombatRole::CloseAssault, "C4022", 1);
    b.assault = Some(1);
    g.state
        .land
        .combat
        .assignment
        .plans
        .insert(Side::Axis, vec![a.clone(), b.clone()]);
    assert!(assaults(&c, &g.state, Side::Axis).iter().all(|g| g.probe));
    a.toe = 3;
    g.state
        .land
        .combat
        .assignment
        .plans
        .insert(Side::Axis, vec![a.clone(), b]);
    assert!(assaults(&c, &g.state, Side::Axis).iter().all(|g| !g.probe));
    a.probe = Some(true);
    g.state
        .land
        .combat
        .assignment
        .plans
        .insert(Side::Axis, vec![a]);
    assert!(assaults(&c, &g.state, Side::Axis)[0].probe);
}

/// Cases: land:14.23,land:3.6
#[test]
fn hidden_enemy_armor_and_composition_do_not_change_requests_or_preflight() {
    let (c, g) = fixture();
    let mut with_armor = g.clone();
    with_armor
        .state
        .land
        .units
        .get_mut(&INF.into())
        .unwrap()
        .toe = Some(Toe::Weapons(vec![cna_content::units::WeaponPoints {
        weapon: "it.m11_39".into(),
        n: 3,
    }]));
    let a = opened(&c, &g);
    let b = opened(&c, &with_armor);
    crate::testkit::assert_indistinguishable(&Cna::dev(), &c, &a.state, &b.state, Side::Axis);
    let order = allocation(&c, &a.state, TANK, CombatRole::AntiArmor, "C4021", 1);
    let command = response(&c, &a, Side::Axis, json!([order]));
    crate::testkit::assert_action_indistinguishable(&Cna::dev(), &c, &a, &b, &command, Side::Axis);
}
/// Cases: land:14.11,land:15.16,land:3.6
#[test]
fn private_plans_and_role_submission_order_do_not_reveal_enemy_assignments() {
    let (c, g) = fixture();
    let g = opened(&c, &g);
    let mut private = g.clone();
    private.state.land.combat.assignment.plans.insert(
        Side::Commonwealth,
        vec![allocation(
            &c,
            &g.state,
            INF,
            CombatRole::CloseAssault,
            "C4021",
            1,
        )],
    );
    crate::testkit::assert_indistinguishable(&Cna::dev(), &c, &g.state, &private.state, Side::Axis);
    let a = allocation(&c, &g.state, TANK, CombatRole::AntiArmor, "C4021", 1);
    let b = allocation(&c, &g.state, INF, CombatRole::CloseAssault, "C4021", 1);
    let run = |sides: [Side; 2]| {
        let mut game = g.clone();
        for side in sides {
            let action = if side == Side::Axis {
                json!([a.clone()])
            } else {
                json!([b.clone()])
            };
            game = evaluate(&Cna::dev(), &c, &game, &response(&c, &game, side, action))
                .unwrap()
                .game;
        }
        finish_game(&c, &mut game);
        serde_json::to_value(game).unwrap()
    };
    assert_eq!(run(Side::ALL), run([Side::Commonwealth, Side::Axis]));
}
/// Cases: land:14.11,land:14.26,land:15.16
#[test]
fn baseline_uses_only_controller_rng_and_every_seed_is_accepted() {
    let (c, g) = fixture();
    let g = opened(&c, &g);
    let before = serde_json::to_value(&g).unwrap();
    let mut nonempty = 0;
    for seed in 0..64u8 {
        let mut game = g.clone();
        let mut local = CampaignRng::from_seed([seed; 32]);
        for side in Side::ALL {
            let r = Cna::dev()
                .pending(&c, &game.state)
                .into_iter()
                .find(|r| r.seat == seat(side))
                .unwrap();
            let action = random_orders(&c, &game.state, &r, &mut local);
            if !action.as_array().unwrap().is_empty() {
                nonempty += 1;
            }
            game = evaluate(&Cna::dev(), &c, &game, &response(&c, &game, side, action))
                .unwrap()
                .game;
            assert_eq!(game.rng, g.rng);
        }
        finish_game(&c, &mut game);
    }
    assert!(nonempty > 0);
    assert_eq!(serde_json::to_value(g).unwrap(), before);
}
/// Cases: land:14.11,land:15.16,land:3.6
#[test]
fn absent_enemy_contents_cannot_remove_a_fixed_window_and_full_gap_is_uniform() {
    let (c, g) = fixture();
    let mut empty = g.clone();
    empty.state.land.units.get_mut(&INF.into()).unwrap().toe = Some(Toe::Under { under: 0 });
    let a = opened(&c, &g);
    let b = opened(&c, &empty);
    let requests = |g: &Game<Cna>| {
        Cna::dev()
            .pending(&c, &g.state)
            .into_iter()
            .filter(|r| r.seat.side == Side::Axis)
            .collect::<Vec<_>>()
    };
    assert_eq!(requests(&a), requests(&b));
    assert_eq!(b.state.decisions.pending.len(), 2);
    for game in [g, empty] {
        assert!(
            matches!(evaluate(&Cna::full(),&c,&game,&Command::Advance),Err(Rejection::Engine(EngineError::Unsupported {ref case,..})) if case=="land:14.3")
        );
    }
}

/// Different weapons retain disjoint actual pools; no whole-unit aggregate authorizes overdraw.
/// Cases: land:14.26,land:15.16,airlog:50.14
#[test]
fn mixed_weapon_counts_and_body_partitions_cannot_borrow_each_others_toe() {
    let (c, mut g) = fixture();
    g.state.land.units.get_mut(&TANK.into()).unwrap().toe = Some(Toe::Weapons(vec![
        cna_content::units::WeaponPoints {
            weapon: "it.m11_39".into(),
            n: 2,
        },
        cna_content::units::WeaponPoints {
            weapon: "it.cv33".into(),
            n: 5,
        },
    ]));
    let g = opened(&c, &g);
    let aa = allocation(&c, &g.state, TANK, CombatRole::AntiArmor, "C4021", 2);
    let mut ca = allocation(&c, &g.state, TANK, CombatRole::CloseAssault, "C4021", 1);
    ca.weapon = Some("it.cv33".into());
    assert!(validate(&c, &g.state, Side::Axis, &[aa.clone(), ca.clone()]).is_ok());
    let accepted = evaluate(
        &Cna::dev(),
        &c,
        &g,
        &response(&c, &g, Side::Axis, json!([aa.clone(), ca.clone()])),
    )
    .unwrap()
    .game;
    let ps = partitions(&c, &accepted.state, Side::Axis);
    assert_eq!(
        ps.iter()
            .find(|p| p.weapon.as_deref() == Some("it.m11_39"))
            .unwrap()
            .withheld,
        0
    );
    assert_eq!(
        ps.iter()
            .find(|p| p.weapon.as_deref() == Some("it.cv33"))
            .unwrap()
            .withheld,
        4
    );
    let mut bad = ca;
    bad.weapon = Some("it.m11_39".into());
    let before = serde_json::to_value(&g).unwrap();
    assert!(
        evaluate(
            &Cna::dev(),
            &c,
            &g,
            &response(&c, &g, Side::Axis, json!([aa, bad]))
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&g).unwrap(), before);
    let body = allocation(&c, &g.state, INF, CombatRole::CloseAssault, "C4021", 1);
    let accepted = evaluate(
        &Cna::dev(),
        &c,
        &g,
        &response(&c, &g, Side::Commonwealth, json!([body])),
    )
    .unwrap()
    .game;
    let ps = partitions(&c, &accepted.state, Side::Commonwealth);
    assert_eq!(ps[0].weapon, None);
    assert_eq!(ps[0].close_assault, 1);
    assert_eq!(
        ps[0].withheld,
        formation::strength(&c, &g.state, &INF.into()) - 1
    );
}

/// Ordered groups are own geometry and metadata, with no enemy contents in preflight.
/// Cases: land:15.23,land:15.24,land:15.25,land:15.91
#[test]
fn combined_assaults_validate_public_geometry_order_and_private_probe_consistency() {
    let (c, mut g) = fixture();
    let origin: HexId = "C4020".into();
    let ns: Vec<_> = c
        .map
        .neighbors(&origin)
        .iter()
        .map(|h| h.id.clone())
        .collect();
    let (adjacent, opposite) =
        ns.iter()
            .filter(|h| h.as_str() != "C4021")
            .fold((None, None), |(a, b), h| {
                if c.map.neighbors(&"C4021".into()).iter().any(|n| &n.id == h) {
                    (Some(h.clone()), b)
                } else {
                    (a, Some(h.clone()))
                }
            });
    let adjacent = adjacent.unwrap();
    let opposite = opposite.unwrap();
    let other = g
        .state
        .units_of(Side::Commonwealth)
        .find(|u| u.id.as_str() != INF)
        .unwrap()
        .id
        .clone();
    g.state.land.units.get_mut(&other).unwrap().location = Location::Hex {
        hex: adjacent.clone(),
    };
    let g = opened(&c, &g);
    let a = allocation(&c, &g.state, TANK, CombatRole::CloseAssault, "C4021", 2);
    let mut b = allocation(
        &c,
        &g.state,
        TANK,
        CombatRole::CloseAssault,
        adjacent.as_str(),
        2,
    );
    assert!(validate(&c, &g.state, Side::Axis, &[a.clone(), b.clone()]).is_ok());
    let mut inconsistent = b.clone();
    inconsistent.probe = Some(true);
    assert!(validate(&c, &g.state, Side::Axis, &[a.clone(), inconsistent]).is_err());
    b.assault = Some(1);
    assert!(validate(&c, &g.state, Side::Axis, &[a.clone(), b.clone()]).is_ok());
    let mut duplicate = b.clone();
    duplicate.target = a.target.clone();
    assert!(validate(&c, &g.state, Side::Axis, &[a.clone(), duplicate]).is_err());
    b.assault = Some(2);
    assert!(validate(&c, &g.state, Side::Axis, &[a.clone(), b.clone()]).is_err());
    let mut separated = g.clone();
    separated.state.land.units.get_mut(&other).unwrap().location = Location::Hex {
        hex: opposite.clone(),
    };
    b.assault = Some(0);
    b.target = opposite;
    assert!(targets(&c, &separated.state, &TANK.into(), Side::Axis).contains(&b.target));
    assert!(validate(&c, &separated.state, Side::Axis, &[a, b]).is_err());
}

/// Reservations across different units share one real dump and reject as a whole.
/// Cases: airlog:50.14,airlog:50.15,land:14.26
#[test]
fn separate_units_share_ammunition_and_a_bad_last_reservation_changes_nothing() {
    const SECOND: &str = "it.libyan_tank_command.ii_m_tank_bn";
    let (c, mut g) = fixture();
    g.state.land.units.get_mut(&SECOND.into()).unwrap().location = Location::Hex {
        hex: "C4020".into(),
    };
    g.state.land.units.get_mut(&SECOND.into()).unwrap().detached = true;
    let stock = g.state.logistics.unit_supply[&TANK.into()].clone();
    g.state.logistics.unit_supply.insert(SECOND.into(), stock);
    let mut a = allocation(&c, &g.state, TANK, CombatRole::AntiArmor, "C4021", 1);
    let mut b = allocation(&c, &g.state, SECOND, CombatRole::AntiArmor, "C4021", 1);
    g.state.logistics.dumps.insert(
        "shared".into(),
        crate::state::Dump {
            id: "shared".into(),
            marker: "opaque".into(),
            side: Side::Axis,
            location: crate::state::DumpLocation::Hex {
                hex: "C4020".into(),
            },
            supplies: cna_content::scenario::Supplies {
                ammo: a.draws[0].ammo,
                ..Default::default()
            },
            active: true,
            dummy: false,
        },
    );
    for row in [&mut a, &mut b] {
        row.draws[0].source =
            serde_json::to_string(&logistics::SupplySource::Dump("shared".into())).unwrap();
    }
    let g = opened(&c, &g);
    assert!(validate(&c, &g.state, Side::Axis, &[a.clone()]).is_ok());
    assert!(validate(&c, &g.state, Side::Axis, &[b.clone()]).is_ok());
    let before = serde_json::to_value(&g).unwrap();
    assert!(
        evaluate(
            &Cna::dev(),
            &c,
            &g,
            &response(&c, &g, Side::Axis, json!([a, b]))
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&g).unwrap(), before);
}

/// Reserve rules are previewed but their flags and DP are not spent by an answer.
/// Cases: land:18.22,land:18.24,land:18.25,land:15.16
#[test]
fn reserve_assault_limits_are_checked_without_recording_an_attack() {
    let (c, mut g) = fixture();
    g.state
        .land
        .units
        .get_mut(&TANK.into())
        .unwrap()
        .reserve
        .status = reserve::Status::ReleasedSecond;
    let g = opened(&c, &g);
    let a = allocation(&c, &g.state, TANK, CombatRole::CloseAssault, "C4021", 4);
    let accepted = evaluate(
        &Cna::dev(),
        &c,
        &g,
        &response(&c, &g, Side::Axis, json!([a.clone()])),
    )
    .unwrap()
    .game;
    assert_eq!(
        serde_json::to_value(&accepted.state.land.units).unwrap(),
        serde_json::to_value(&g.state.land.units).unwrap()
    );
    let mut spent = g.clone();
    spent
        .state
        .land
        .units
        .get_mut(&TANK.into())
        .unwrap()
        .reserve
        .offensive_assault_used = true;
    assert!(validate(&c, &spent.state, Side::Axis, std::slice::from_ref(&a)).is_err());
    let mut waiting = g.clone();
    waiting
        .state
        .land
        .units
        .get_mut(&TANK.into())
        .unwrap()
        .reserve
        .status = reserve::Status::Second;
    assert!(validate(&c, &waiting.state, Side::Axis, &[a]).is_err());
}
