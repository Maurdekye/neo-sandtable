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
