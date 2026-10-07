use super::*;
use crate::{
    seq::{Block, OPSTAGE},
    state::Location,
};
use cna_core::{dice::CampaignRng, visibility::Perspective};
use serde_json::json;
fn content() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}
fn pending(s: &mut State) -> Pending {
    let pos = s
        .decisions
        .pending
        .iter()
        .position(|p| p.kind.starts_with(PREFIX))
        .unwrap();
    s.decisions.pending.remove(pos)
}
fn finish(c: &CnaContent, s: &mut State, cx: &mut Cx<'_>) {
    while s
        .decisions
        .pending
        .iter()
        .any(|p| p.kind.starts_with(PREFIX))
    {
        let p = pending(s);
        answer(c, s, &p, &Value::Null, cx).unwrap();
    }
}
/// Cases: scen:60.37, airlog:56.21, airlog:56.25, airlog:56.28, land:3.6
/// Interpretations: interp:airlog-0003, interp:airlog-0010
#[test]
fn real_gt1_capacity_plan_arrival_and_enemy_secrecy_survive_checkpoint() {
    let c = content();
    let mut s = State::new(&c).unwrap();
    let mut rng = CampaignRng::from_seed([8; 32]);
    let mut events = vec![];
    // Initial identities are assigned during setup, before secret convoy planning.
    super::super::dump_markers::initialize(
        &mut s,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    let mut expected = rng.clone();
    let die = expected.d6();
    let enemy_before = crate::view::observe(&c, &s, Perspective::Side(Side::Commonwealth));
    let before_stock = s
        .logistics
        .dumps
        .values()
        .find(|d| matches!(&d.location,DumpLocation::OffMap{id} if id=="box_tripoli"))
        .unwrap()
        .supplies;
    initialize(
        &c,
        &mut s,
        false,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert_eq!(pre_game_turns(&c, &s).unwrap(), vec![1, 2, 3]);
    assert_eq!(s.cursor.block, Block::Setup);
    assert_eq!(s.logistics.convoy_turns[&1].level, ConvoyLevel::B);
    assert_eq!(
        s.logistics.convoy_turns[&1].capacity_tons,
        c.tables
            .airlog
            .convoy_capacity
            .capacity(ConvoyLevel::B, die)
            .get()
    );
    assert_eq!(rng.state(), expected.state());
    assert_eq!(
        crate::view::observe(&c, &s, Perspective::Side(Side::Commonwealth)),
        enemy_before
    );
    let p = pending(&mut s);
    assert_eq!(p.secrecy, Secrecy::Secret);
    let action =
        json!({"convoys":[{"lane":"2","arrival_opstage":2,"ammo":100,"fuel":800,"stores":100}]});
    answer(
        &c,
        &mut s,
        &p,
        &action,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    finish(
        &c,
        &mut s,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    );
    let mut restored: State = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    let mut restore_rng = rng.clone();
    let mut restored_events = vec![];
    initialize(
        &c,
        &mut restored,
        false,
        &mut Cx {
            rng: &mut restore_rng,
            events: &mut restored_events,
        },
    )
    .unwrap();
    assert!(restored_events.is_empty());
    assert_eq!(restore_rng.state(), rng.state());
    for state in [&mut s, &mut restored] {
        state.cursor.block = Block::OpStage;
        state.cursor.index = OPSTAGE
            .iter()
            .position(|s| s.anchor == "opstage.convoy_arrival")
            .unwrap();
        state.cursor.op_stage = Some(1);
        arrive(
            &c,
            state,
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        assert_eq!(
            state.logistics.convoy_turns[&1].convoys[&2].status,
            ConvoyStatus::Planned
        );
        state.cursor.op_stage = Some(2);
        arrive(
            &c,
            state,
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        let holding = state
            .logistics
            .dumps
            .values()
            .find(|d| matches!(&d.location,DumpLocation::OffMap{id} if id=="box_tripoli"))
            .unwrap()
            .supplies;
        assert_eq!(holding.ammo, before_stock.ammo + 100);
        assert_eq!(holding.fuel, before_stock.fuel + 800);
        assert_eq!(holding.stores, before_stock.stores + 100);
        assert_eq!(
            state.logistics.convoy_turns[&1].convoys[&2].status,
            ConvoyStatus::Arrived
        );
        let before = serde_json::to_value(&*state).unwrap();
        arrive(
            &c,
            state,
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&*state).unwrap(), before);
    }
    assert_eq!(
        serde_json::to_value(&s).unwrap(),
        serde_json::to_value(&restored).unwrap()
    );
    assert!(
        events
            .iter()
            .all(|e| e.audience == Audience::Side(Side::Axis)
                || e.audience == Audience::Seat(SeatId::new(Side::Axis, Role::Logistics)))
    );
}
/// Cases: airlog:56.12, airlog:56.15, airlog:56.22, airlog:56.25, airlog:56.27
#[test]
fn bad_capacity_duplicate_lane_unavailable_lane_negative_cargo_and_enemy_reject_atomically() {
    let c = content();
    let mut s = State::new(&c).unwrap();
    let mut rng = CampaignRng::from_seed([7; 32]);
    let mut events = vec![];
    initialize(
        &c,
        &mut s,
        false,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    let p = pending(&mut s);
    let before = serde_json::to_value(&s).unwrap();
    let rng_before = rng.state();
    let count = events.len();
    for ships in [
        json!([{"lane":"2","arrival_opstage":1,"ammo":50000,"fuel":0,"stores":0}]),
        json!([{"lane":"1","arrival_opstage":1,"ammo":1,"fuel":0,"stores":0}]),
        json!([{"lane":"2","arrival_opstage":4,"ammo":1,"fuel":0,"stores":0}]),
        json!([{"lane":"2","arrival_opstage":1,"ammo":-1,"fuel":0,"stores":0}]),
        json!([{"lane":"2","arrival_opstage":1,"ammo":1,"fuel":0,"stores":0},{"lane":"2","arrival_opstage":2,"ammo":1,"fuel":0,"stores":0}]),
    ] {
        assert!(
            answer(
                &c,
                &mut s,
                &p,
                &json!({"convoys":ships}),
                &mut Cx {
                    rng: &mut rng,
                    events: &mut events
                }
            )
            .is_err()
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        assert_eq!(rng.state(), rng_before);
        assert_eq!(events.len(), count);
    }
    let mut enemy = p.clone();
    enemy.seat.side = Side::Commonwealth;
    assert!(
        answer(
            &c,
            &mut s,
            &enemy,
            &Value::Null,
            &mut Cx {
                rng: &mut rng,
                events: &mut events
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}
/// Cases: airlog:56.15, airlog:56.28
#[test]
fn captured_port_cancels_and_congestion_turns_back_excess_cargo() {
    let c = content();
    for captured in [false, true] {
        let mut s = State::new(&c).unwrap();
        let mut rng = CampaignRng::from_seed([9; 32]);
        let mut events = vec![];
        initialize(
            &c,
            &mut s,
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        let p = pending(&mut s);
        answer(
            &c,
            &mut s,
            &p,
            &json!({"convoys":[{"lane":"2","arrival_opstage":1,"ammo":100,"fuel":0,"stores":0}]}),
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        finish(
            &c,
            &mut s,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        );
        s.cursor.op_stage = Some(1);
        if captured {
            ports::record_entry(
                &c,
                &mut s,
                Side::Commonwealth,
                &Location::OffMap {
                    id: "box_tripoli".into(),
                },
            );
        } else {
            s.logistics
                .ports
                .get_mut("box_tripoli")
                .unwrap()
                .used_tons24 = 14950 * 24;
        }
        let stock_before = s.logistics.dumps.clone();
        let result = arrive(
            &c,
            &mut s,
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        );
        if captured {
            result.unwrap();
            assert_eq!(
                s.logistics.convoy_turns[&1].convoys[&2].status,
                ConvoyStatus::Cancelled
            )
        } else {
            result.unwrap();
            assert_eq!(
                s.logistics.convoy_turns[&1].convoys[&2].status,
                ConvoyStatus::Arrived
            );
            assert_eq!(
                s.logistics.convoy_turns[&1].convoys[&2]
                    .delivered
                    .unwrap()
                    .ammo,
                12
            );
            let before: i32 = stock_before
                .values()
                .filter(|d| d.side == Side::Axis)
                .map(|d| d.supplies.ammo)
                .sum();
            let after: i32 = s
                .logistics
                .dumps
                .values()
                .filter(|d| d.side == Side::Axis)
                .map(|d| d.supplies.ammo)
                .sum();
            assert_eq!(after - before, 12);
            assert_eq!(s.logistics.ports["box_tripoli"].used_tons24, 14998 * 24);
        }
        if captured {
            assert_eq!(s.logistics.dumps, stock_before);
        }
    }
}
/// Cases: scen:60.37, land:20.63
#[test]
fn bounded_calendar_and_strict_replacement_gap_do_not_silently_replan_in_play() {
    let mut c = content();
    c.bounds.end_gt = 2;
    let mut s = State::new(&c).unwrap();
    assert_eq!(pre_game_turns(&c, &s).unwrap(), vec![1, 2]);
    let mut rng = CampaignRng::from_seed([8; 32]);
    let mut events = vec![];
    assert!(
        matches!(initialize(&c,&mut s,true,&mut Cx{rng:&mut rng,events:&mut events}),Err(EngineError::Unsupported{case,..})if case=="land:20.63")
    );
    assert!(!s.logistics.convoys_initialized);
    assert!(
        matches!(schedule(&c,&mut s,false,&mut Cx{rng:&mut rng,events:&mut events}),Err(EngineError::Unsupported{case,..})if case=="scen:60.37")
    );
}

fn unknown_port_arrivals_fixture() -> (CnaContent, State, ports::Port) {
    let mut c = content();
    let affected = ports::lane_destination(&c, 3).unwrap();
    assert_eq!(affected.id, "A4827");
    let mut record = c.scenario.construction.port_overrides[0].clone();
    record.hex = "A4827".into();
    record.port = "Benghazi".into();
    record.efficiency_level = 1;
    // Synthetic valid future policy exercises diagnostics, not source transcription.
    record.src = vec!["scen:61.1".into()];
    c.scenario.construction.port_overrides = vec![record];
    let mut s = State::new(&c).unwrap();
    s.cursor.block = Block::OpStage;
    s.cursor.op_stage = Some(1);
    s.cursor.index = OPSTAGE
        .iter()
        .position(|s| s.anchor == "opstage.convoy_arrival")
        .unwrap();
    for lane in [2, 3] {
        let port = ports::lane_destination(&c, lane).unwrap();
        s.logistics.ports.insert(
            port.id,
            ports::PortState {
                owner: Side::Axis,
                efficiency: c
                    .tables
                    .airlog
                    .port_capacity
                    .port(port.name)
                    .max_efficiency_level,
                blocked_levels: 0,
                mined_levels: 0,
                bombed_stage: None,
                budget_stage: None,
                used_tons24: 0,
            },
        );
    }
    let cargo = Supplies {
        stores: 10,
        ..Supplies::default()
    };
    s.logistics.convoy_turns.insert(
        s.cursor.game_turn,
        ConvoyTurn {
            level: ConvoyLevel::B,
            capacity_tons: 1000,
            replacement_tons: 0,
            planning_complete: true,
            convoys: [2, 3]
                .into_iter()
                .map(|lane| {
                    (
                        lane,
                        NavalConvoy {
                            lane,
                            arrival_opstage: 1,
                            cargo,
                            status: ConvoyStatus::Planned,
                            delivered: None,
                        },
                    )
                })
                .collect(),
        },
    );
    (c, s, affected)
}

/// Cases: airlog:55.14, airlog:55.18, airlog:56.28, land:3.6
#[test]
fn unknown_arrival_is_terminal_without_zero_delivery_while_healthy_port_unloads() {
    let (c, _, port) = unknown_port_arrivals_fixture();
    let maximum = c
        .tables
        .airlog
        .port_capacity
        .port(port.name)
        .max_efficiency_level;
    for efficiency in [0, maximum] {
        let (c, mut s, affected) = unknown_port_arrivals_fixture();
        let old = s.logistics.ports.get_mut(&affected.id).unwrap();
        assert!(efficiency <= old.efficiency);
        old.efficiency = efficiency;
        old.blocked_levels = 1;
        old.used_tons24 = 127;
        let retained = old.clone();
        let before: i32 = s.logistics.dumps.values().map(|d| d.supplies.stores).sum();
        let gt = s.cursor.game_turn;
        let mut rng = CampaignRng::from_seed([19; 32]);
        let rng_before = rng.state();
        let mut events = vec![];
        arrive(
            &c,
            &mut s,
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        let turn = &s.logistics.convoy_turns[&gt];
        assert_eq!(turn.convoys[&3].status, ConvoyStatus::Unassessed);
        assert_eq!(turn.convoys[&3].delivered, None);
        assert_eq!(turn.convoys[&3].cargo.stores, 10);
        assert_eq!(s.logistics.ports[&affected.id], retained);
        assert_eq!(turn.convoys[&2].status, ConvoyStatus::Arrived);
        assert_eq!(turn.convoys[&2].delivered.unwrap().stores, 10);
        assert_eq!(
            s.logistics
                .dumps
                .values()
                .map(|d| d.supplies.stores)
                .sum::<i32>()
                - before,
            10
        );
        assert_eq!(
            s.logistics.ports["box_tripoli"].used_tons24,
            ports::weight24(&c, &turn.convoys[&2].cargo).unwrap()
        );
        assert_eq!(rng.state(), rng_before);
        assert!(
            events
                .iter()
                .all(|e| !Perspective::Side(Side::Commonwealth).can_see(&e.audience))
        );
        let text = serde_json::to_value(&events).unwrap().to_string();
        assert!(text.contains("scen:61.1") && text.contains("A4827") && text.contains("raw=1"));
        let saved = serde_json::to_value(&s).unwrap();
        let mut restored: State = serde_json::from_value(saved.clone()).unwrap();
        let mut replay_events = vec![];
        arrive(
            &c,
            &mut restored,
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut replay_events,
            },
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), saved);
        restored.cursor.op_stage = Some(2);
        s.cursor.op_stage = Some(2);
        arrive(
            &c,
            &mut restored,
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut replay_events,
            },
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&restored).unwrap(),
            serde_json::to_value(&s).unwrap()
        );
        assert!(replay_events.is_empty());
        assert_eq!(rng.state(), rng_before);
    }
    for status in ["planned", "cancelled", "arrived", "unassessed"] {
        let parsed: ConvoyStatus = serde_json::from_value(json!(status)).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), json!(status));
    }
}

/// Cases: airlog:55.18, airlog:56.28, land:3.6
#[test]
fn unknown_arrival_cargo_and_private_diagnostic_are_indistinguishable_to_enemy() {
    let (c, mut a, _) = unknown_port_arrivals_fixture();
    let mut b = a.clone();
    b.logistics
        .convoy_turns
        .get_mut(&b.cursor.game_turn)
        .unwrap()
        .convoys
        .get_mut(&3)
        .unwrap()
        .cargo
        .stores = 99;
    crate::testkit::assert_indistinguishable(&crate::Cna::dev(), &c, &a, &b, Side::Commonwealth);
    let mut rng_a = CampaignRng::from_seed([20; 32]);
    let mut rng_b = rng_a.clone();
    let mut events_a = vec![];
    let mut events_b = vec![];
    arrive(
        &c,
        &mut a,
        false,
        &mut Cx {
            rng: &mut rng_a,
            events: &mut events_a,
        },
    )
    .unwrap();
    arrive(
        &c,
        &mut b,
        false,
        &mut Cx {
            rng: &mut rng_b,
            events: &mut events_b,
        },
    )
    .unwrap();
    crate::testkit::assert_indistinguishable(&crate::Cna::dev(), &c, &a, &b, Side::Commonwealth);
    let visible = |events: &[EngineEvent]| {
        events
            .iter()
            .filter(|e| Perspective::Side(Side::Commonwealth).can_see(&e.audience))
            .map(|e| serde_json::to_value(e).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(visible(&events_a), visible(&events_b));
    assert_eq!(rng_a.state(), rng_b.state());
}

/// Cases: airlog:55.18, airlog:56.28, scen:60.7
#[test]
fn strict_arrival_preflight_precedes_absent_turn_and_legacy_port_state() {
    let (c, a, _) = unknown_port_arrivals_fixture();
    let mut b = a.clone();
    b.logistics.convoy_turns.clear();
    b.logistics.ports.clear();
    let mut errors = vec![];
    for mut s in [a, b] {
        let saved = serde_json::to_value(&s).unwrap();
        let mut rng = CampaignRng::from_seed([21; 32]);
        let rng_before = rng.state();
        let mut events = vec![];
        let error = arrive(
            &c,
            &mut s,
            true,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap_err();
        assert!(matches!(&error, EngineError::Unsupported { case, detail }
            if case == "scen:61.1" && detail.contains("A4827") && detail.contains("raw=1")));
        errors.push(format!("{error:?}"));
        assert_eq!(serde_json::to_value(&s).unwrap(), saved);
        assert_eq!(rng.state(), rng_before);
        assert!(events.is_empty());
    }
    assert_eq!(errors[0], errors[1]);
}

/// Cases: airlog:55.18, airlog:56.25, land:3.6
#[test]
fn planning_menu_never_uses_unknown_numeric_state_and_nominal_ceiling_stays_printed() {
    let (c, mut s, affected) = unknown_port_arrivals_fixture();
    let mut notes = vec![];
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(lanes(&c, &s, 1, false, &mut notes).unwrap(), vec![2]);
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert!(
        notes
            .iter()
            .all(|e| !Perspective::Side(Side::Commonwealth).can_see(&e.audience))
    );
    assert!(
        serde_json::to_value(&notes)
            .unwrap()
            .to_string()
            .contains("scen:61.1")
    );
    let expected = ports::planning_capacity_tons(&c, &s, &affected).unwrap_err();
    s.logistics.ports.get_mut(&affected.id).unwrap().efficiency = 0;
    assert_eq!(
        ports::planning_capacity_tons(&c, &s, &affected),
        Err(expected)
    );
    assert_eq!(lanes(&c, &s, 1, false, &mut vec![]).unwrap(), vec![2]);
    let healthy = ports::lane_destination(&c, 2).unwrap();
    s.logistics.ports.get_mut(&healthy.id).unwrap().efficiency = 1;
    assert!(
        ports::capacity_tons(&c, &s, &healthy).unwrap()
            < ports::planning_capacity_tons(&c, &s, &healthy).unwrap()
    );
    assert_eq!(
        ports::planning_capacity_tons(&c, &s, &healthy).unwrap(),
        i64::from(c.tables.airlog.port_capacity.port(healthy.name).max_tonnage)
    );
}

fn planning_game() -> (CnaContent, cna_core::engine::Game<crate::Cna>) {
    let (c, mut s, _) = unknown_port_arrivals_fixture();
    s.cursor.block = Block::Setup;
    s.cursor.index = 0;
    s.cursor.entered = true;
    s.logistics
        .convoy_turns
        .get_mut(&1)
        .unwrap()
        .planning_complete = false;
    s.logistics
        .convoy_turns
        .get_mut(&1)
        .unwrap()
        .convoys
        .clear();
    s.logistics.convoy_planning_queue = vec![1];
    s.decisions.pending.clear();
    let mut rng = CampaignRng::from_seed([29; 32]);
    let before = rng.state();
    next(
        &c,
        &mut s,
        false,
        &mut Cx {
            rng: &mut rng,
            events: &mut vec![],
        },
    )
    .unwrap();
    assert_eq!(rng.state(), before); // Existing capacity is not rerolled.
    (
        c,
        cna_core::engine::Game {
            state: s,
            rng: before,
        },
    )
}

/// Cases: airlog:55.18, airlog:56.12, land:3.6
#[test]
fn real_convoy_respond_preserves_profile_errors_and_rejected_action_surfaces() {
    use cna_core::{
        decision::DecisionResponse,
        engine::{Command, evaluate},
    };
    let (c, a) = planning_game();
    let mut b = a.clone();
    b.state.logistics.ports.get_mut("A4827").unwrap().efficiency = 0;
    b.state
        .logistics
        .convoy_turns
        .get_mut(&1)
        .unwrap()
        .capacity_tons += 100;
    let p = &a.state.decisions.pending[0];
    let rejected = Command::Respond(DecisionResponse {
        seat: p.seat,
        decision_id: p.id.clone(),
        controller_epoch: 1,
        decision_revision: p.revision,
        idempotency_key: "convoy-policy-proof".into(),
        public_explanation: None,
        action: json!({"convoys":[{"lane":"3","arrival_opstage":1,"ammo":0,"fuel":0,"stores":1}]}),
    });
    crate::testkit::assert_action_indistinguishable(
        &crate::Cna::dev(),
        &c,
        &a,
        &b,
        &rejected,
        Side::Commonwealth,
    );
    for g in [&a, &b] {
        let saved = serde_json::to_value(g).unwrap();
        assert!(evaluate(&crate::Cna::dev(), &c, g, &rejected).is_err());
        assert_eq!(serde_json::to_value(g).unwrap(), saved);
    }
    let pass = Command::Respond(DecisionResponse {
        seat: p.seat,
        decision_id: p.id.clone(),
        controller_epoch: 1,
        decision_revision: p.revision,
        idempotency_key: "convoy-policy-proof".into(),
        public_explanation: None,
        action: Value::Null,
    });
    crate::testkit::assert_action_indistinguishable(
        &crate::Cna::full(),
        &c,
        &a,
        &b,
        &pass,
        Side::Commonwealth,
    );
    for mut g in [a, b] {
        g.state.logistics.ports.clear();
        let saved = serde_json::to_value(&g).unwrap();
        assert!(matches!(evaluate(&crate::Cna::full(), &c, &g, &pass),
            Err(Rejection::Engine(EngineError::Unsupported { case, detail }))
            if case == "scen:61.1" && detail.contains("A4827")));
        assert_eq!(serde_json::to_value(&g).unwrap(), saved);
    }
}

/// Cases: airlog:56.12, airlog:56.21, land:3.6
#[test]
fn dev_compatibility_answer_is_identical_to_explicit_profile_with_checkpoint_replay() {
    let (c, g) = planning_game();
    let mut a = g.state;
    let mut b: State = serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
    let pa = pending(&mut a);
    let pb = pending(&mut b);
    let mut ra = CampaignRng::from_seed([30; 32]);
    let mut rb = ra.clone();
    let mut ea = vec![];
    let mut eb = vec![];
    let before = a.logistics.dumps.clone();
    assert_eq!(
        answer(
            &c,
            &mut a,
            &pa,
            &Value::Null,
            &mut Cx {
                rng: &mut ra,
                events: &mut ea
            }
        )
        .unwrap(),
        answer_with_profile(
            &c,
            &mut b,
            &pb,
            &Value::Null,
            false,
            &mut Cx {
                rng: &mut rb,
                events: &mut eb
            }
        )
        .unwrap()
    );
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(&b).unwrap()
    );
    assert_eq!(a.logistics.dumps, before);
    assert_eq!(ra.state(), rb.state());
    assert_eq!(
        serde_json::to_value(ea).unwrap(),
        serde_json::to_value(eb).unwrap()
    );
}
