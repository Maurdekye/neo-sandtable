use super::*;
use crate::{
    Cna,
    seq::{Block, OPSTAGE},
    state::{Dump, DumpLocation, Location, WeatherState},
};
use cna_core::{
    decision::DecisionResponse,
    dice::CampaignRng,
    engine::{Command, Game, Ruleset, evaluate},
    visibility::Perspective,
};
use cna_tables::{
    airlog::supply::{WellAttempt, WellEffect},
    land::weather::WeatherKind,
};
use serde_json::json;
use std::sync::OnceLock;
const AX: &str = "it.1_libyan_div.viii_libyan_bn";
const CW: &str = "cw.2_nz_div.21st_nz_bn";
fn content() -> &'static CnaContent {
    static C: OnceLock<CnaContent> = OnceLock::new();
    C.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn fixture() -> State {
    let mut s = State::new(content()).unwrap();
    s.cursor.block = Block::OpStage;
    s.cursor.index = OPSTAGE
        .iter()
        .position(|s| s.anchor == "opstage.organization.water_distribution")
        .unwrap();
    s.cursor.op_stage = Some(1);
    s.cursor.entered = true;
    s.turn.player_a = Some(Side::Commonwealth);
    s.turn.weather = Some(WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    for (id, u) in &mut s.land.units {
        u.location = if id.as_str() == AX {
            Location::Hex {
                hex: "C4020".into(),
            }
        } else if id.as_str() == CW {
            Location::Hex {
                hex: "E1730".into(),
            }
        } else {
            Location::Eliminated
        };
        u.trucks = Default::default();
        u.transport_trucks = Default::default();
    }
    s.logistics.dumps.insert(
        "test-stock".into(),
        Dump {
            id: "test-stock".into(),
            marker: "dump-test".into(),
            side: Side::Axis,
            location: DumpLocation::Hex {
                hex: "C4020".into(),
            },
            supplies: Supplies {
                stores: 100,
                water: 100,
                ..Default::default()
            },
            active: true,
            dummy: false,
        },
    );
    s
}
fn drain(s: &mut State, controller: &mut CampaignRng, dice: &mut CampaignRng) -> usize {
    for count in 0..30 {
        if s.decisions.pending.is_empty() {
            if s.cursor.anchor() == "opstage.organization.water_distribution" {
                finish_water(
                    content(),
                    s,
                    &mut Cx {
                        rng: dice,
                        events: &mut vec![],
                    },
                    false,
                )
                .unwrap();
            }
            if s.decisions.pending.is_empty() {
                return count;
            }
        }
        let p = s.decisions.pending.first().unwrap().clone();
        let request = Cna::dev().pending(content(), s).remove(0);
        let before = dice.state();
        let action = crate::baseline::logistics_orders(content(), s, &request, controller).unwrap();
        assert_eq!(before, dice.state());
        s.decisions.pending.remove(0);
        answer(
            content(),
            s,
            &p,
            &action,
            &mut Cx {
                rng: dice,
                events: &mut vec![],
            },
            false,
        )
        .unwrap_or_else(|e| panic!("{}: {action}: {e:?}", p.kind));
    }
    panic!("batch policy did not finish")
}
/// Cases: airlog:51.11, airlog:51.23, airlog:52.13, airlog:52.41, airlog:52.42, land:3.6
#[test]
fn whole_list_baselines_are_accepted_across_64_seeds() {
    for seed in 0..64 {
        let mut s = fixture();
        let mut controller = CampaignRng::from_seed([seed; 32]);
        let mut dice = CampaignRng::from_seed([seed ^ 127; 32]);
        s.cursor.block = Block::Pre;
        enter_stores(
            content(),
            &mut s,
            &mut Cx {
                rng: &mut dice,
                events: &mut vec![],
            },
        )
        .unwrap();
        assert!(drain(&mut s, &mut controller, &mut dice) <= 2);
        for raw in [AX, CW] {
            let id = UnitId::new(raw);
            assert_eq!(
                s.logistics.rations[&id].stores_received,
                rations::stores_required(content(), &s, &id).unwrap()
            );
        }
        s.cursor.block = Block::OpStage;
        enter_water(
            content(),
            &mut s,
            &mut Cx {
                rng: &mut dice,
                events: &mut vec![],
            },
            false,
        )
        .unwrap();
        assert!(drain(&mut s, &mut controller, &mut dice) <= 8);
        for raw in [AX, CW] {
            assert_eq!(
                s.logistics.rations[&UnitId::new(raw)].infantry_water_received,
                1
            );
        }
    }
}
/// Cases: airlog:51.15, airlog:51.23, land:3.6
#[test]
fn invalid_later_allocation_foreign_or_duplicate_unit_is_atomic() {
    let mut s = fixture();
    let mut dice = CampaignRng::from_seed([3; 32]);
    enter_stores(
        content(),
        &mut s,
        &mut Cx {
            rng: &mut dice,
            events: &mut vec![],
        },
    )
    .unwrap();
    let p = s
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap()
        .clone();
    let n = rations::stores_required(content(), &s, &AX.into()).unwrap();
    let source =
        serde_json::to_string(&super::super::SupplySource::Dump("test-stock".into())).unwrap();
    let valid = json!({"unit":AX,"stores":n,"half":false,"pasta":false,"draws":[{"source":source,"stores":n,"water":0}]});
    for second in [
        json!({"unit":CW,"stores":1,"half":false,"pasta":false,"draws":[]}),
        valid.clone(),
    ] {
        let before = serde_json::to_value(&s).unwrap();
        let rng = dice.state();
        let mut events = vec![];
        assert!(
            answer(
                content(),
                &mut s,
                &p,
                &json!([valid, second]),
                &mut Cx {
                    rng: &mut dice,
                    events: &mut events
                },
                false
            )
            .is_err()
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        assert_eq!(dice.state(), rng);
        assert!(events.is_empty());
    }
}
fn well_fixture() -> (CnaContent, State) {
    let mut c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut s = fixture();
    s.land.units.get_mut(&CW.into()).unwrap().location = Location::Hex {
        hex: "C4020".into(),
    };
    c.places.places.insert(
        "fixture-bir".into(),
        cna_content::places::Place {
            id: "fixture-bir".into(),
            name: "Test source".into(),
            hex_id: "C4020".into(),
            kind: "bir".into(),
            place_group: None,
            src: vec!["airlog:52.11".into()],
            note: None,
            review_batch: "test-fixture".into(),
        },
    );
    (c, s)
}
fn response(p: &Pending, action: Value) -> Command {
    Command::Respond(DecisionResponse {
        decision_id: p.id.clone(),
        seat: p.seat,
        controller_epoch: 1,
        decision_revision: p.revision,
        idempotency_key: "well-batch-test".into(),
        action,
        public_explanation: None,
    })
}
/// Same acceptance and observations before closure, even for the last answering seat.
/// Cases: airlog:52.13, airlog:52.14, airlog:52.16, land:3.6
#[test]
fn well_answer_is_indistinguishable_before_adjudication_then_reveals_only_condition() {
    let (c, mut s) = well_fixture();
    let mut dice = CampaignRng::from_seed([8; 32]);
    enter_water(
        &c,
        &mut s,
        &mut Cx {
            rng: &mut dice,
            events: &mut vec![],
        },
        false,
    )
    .unwrap();
    let ax = s
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap()
        .clone();
    s.decisions.pending.retain(|p| p.seat.side != Side::Axis);
    answer(
        &c,
        &mut s,
        &ax,
        &Value::Null,
        &mut Cx {
            rng: &mut dice,
            events: &mut vec![],
        },
        false,
    )
    .unwrap();
    let p = s.decisions.pending.first().unwrap().clone();
    let a = Game::<Cna> {
        state: s,
        rng: dice.state(),
    };
    let mut b = a.clone();
    let w = b.state.logistics.wells.entry("C4020".into()).or_default();
    w.poisoned = true;
    w.poisoned_known.insert(Side::Axis);
    let action = json!({"allocations":[],"wells":[{"operation":format!("draw|{CW}"),"requested":1,"packing":CargoPacking::default()}]});
    let command = response(&p, action);
    crate::testkit::assert_action_indistinguishable(
        &Cna::dev(),
        &c,
        &a,
        &b,
        &command,
        Side::Commonwealth,
    );
    let mut t = evaluate(&Cna::dev(), &c, &b, &command).unwrap().game;
    let before = serde_json::to_value(&t.state).unwrap();
    assert!(t.state.logistics.drawn_water.is_empty());
    t = serde_json::from_value(serde_json::to_value(&t).unwrap()).unwrap();
    assert_eq!(before, serde_json::to_value(&t.state).unwrap());
    let mut rng = CampaignRng::from_state(&t.rng);
    let mut events = vec![];
    finish_water(
        &c,
        &mut t.state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        false,
    )
    .unwrap();
    assert_eq!(t.state.logistics.drawn_water[&CW.into()].points, 0);
    assert_eq!(
        wells::condition(
            &t.state,
            &"C4020".into(),
            Perspective::Side(Side::Commonwealth)
        )["poisoned"],
        true
    );
    assert!(events.iter().any(|e| e.audience == Audience::Public));
    assert!(
        t.state
            .decisions
            .pending
            .iter()
            .all(|p| p.kind == WELL_ALLOCATION)
    );
}
/// Cases: airlog:52.13, airlog:52.16, airlog:52.8, land:3.6
/// Interpretations: interp:airlog-0016
#[test]
fn opposing_well_lists_follow_player_a_order_independent_of_submission_order() {
    let (c, base) = well_fixture();
    let seed = (0..=255)
        .find(|seed| {
            let d = CampaignRng::from_seed([*seed; 32]).d6();
            c.tables
                .airlog
                .poisoning_and_sweetening
                .result(WellAttempt::PoisonWell, d)
                == WellEffect::WellPoisoned
        })
        .unwrap();
    let mut results = vec![];
    for order in [
        [Side::Axis, Side::Commonwealth],
        [Side::Commonwealth, Side::Axis],
    ] {
        let mut s = base.clone();
        let mut rng = CampaignRng::from_seed([seed; 32]);
        let mut events = vec![];
        enter_water(
            &c,
            &mut s,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
            false,
        )
        .unwrap();
        for side in order {
            let pos = s
                .decisions
                .pending
                .iter()
                .position(|p| p.seat.side == side)
                .unwrap();
            let p = s.decisions.pending.remove(pos);
            let action = if side == Side::Axis {
                json!({"allocations":[],"wells":[{"operation":format!("draw|{AX}"),"requested":1,"packing":CargoPacking::default()}]})
            } else {
                json!({"allocations":[],"wells":[{"operation":format!("poison|{CW}"),"requested":0,"packing":CargoPacking::default()}]})
            };
            answer(
                &c,
                &mut s,
                &p,
                &action,
                &mut Cx {
                    rng: &mut rng,
                    events: &mut events,
                },
                false,
            )
            .unwrap();
        }
        assert!(s.logistics.drawn_water.is_empty());
        let before = rng.state();
        finish_water(
            &c,
            &mut s,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
            false,
        )
        .unwrap();
        assert_ne!(before, rng.state());
        assert_eq!(s.logistics.drawn_water[&AX.into()].points, 0);
        results.push((serde_json::to_value(&s.logistics).unwrap(), rng.state()));
    }
    assert_eq!(results[0], results[1]);
}

/// Cases: airlog:51.11, airlog:51.15, land:3.6
#[test]
fn later_shared_stock_overdraft_rejects_the_entire_list() {
    let mut s = fixture();
    let second = s
        .land
        .units
        .keys()
        .find(|id| {
            id.as_str() != AX
                && content().units.units[*id].nationality == "italian"
                && rations::infantry(content(), id).unwrap_or(false)
        })
        .unwrap()
        .clone();
    s.land.units.get_mut(&second).unwrap().location = Location::Hex {
        hex: "C4020".into(),
    };
    let n1 = rations::stores_required(content(), &s, &AX.into()).unwrap();
    let n2 = rations::stores_required(content(), &s, &second).unwrap();
    s.logistics
        .dumps
        .get_mut("test-stock")
        .unwrap()
        .supplies
        .stores = n1 + n2 - 1;
    let mut rng = CampaignRng::from_seed([7; 32]);
    enter_stores(
        content(),
        &mut s,
        &mut Cx {
            rng: &mut rng,
            events: &mut vec![],
        },
    )
    .unwrap();
    let p = s
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap()
        .clone();
    let source =
        serde_json::to_string(&super::super::SupplySource::Dump("test-stock".into())).unwrap();
    let issue = |id: UnitId, n| json!({"unit":id,"stores":n,"half":false,"pasta":false,"draws":[{"source":source,"stores":n,"water":0}]});
    let before = serde_json::to_value(&s).unwrap();
    let mut events = vec![];
    assert!(
        answer(
            content(),
            &mut s,
            &p,
            &json!([issue(AX.into(), n1), issue(second, n2)]),
            &mut Cx {
                rng: &mut rng,
                events: &mut events
            },
            false
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert!(events.is_empty());
}
/// Cases: airlog:51.11, airlog:51.15, land:3.6
#[test]
fn stores_list_cannot_probe_enemy_supply_quantities() {
    let mut s = fixture();
    let mut rng = CampaignRng::from_seed([7; 32]);
    enter_stores(
        content(),
        &mut s,
        &mut Cx {
            rng: &mut rng,
            events: &mut vec![],
        },
    )
    .unwrap();
    let p = s
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap()
        .clone();
    let a = Game::<Cna> {
        state: s,
        rng: rng.state(),
    };
    let mut b = a.clone();
    for d in b
        .state
        .logistics
        .dumps
        .values_mut()
        .filter(|d| d.side == Side::Commonwealth)
    {
        d.supplies.stores += 23;
        d.supplies.water += 19;
    }
    let n = rations::stores_required(content(), &a.state, &AX.into()).unwrap();
    let source =
        serde_json::to_string(&super::super::SupplySource::Dump("test-stock".into())).unwrap();
    let action = json!([{"unit":AX,"stores":n,"half":false,"pasta":false,"draws":[{"source":source,"stores":n,"water":0}]}]);
    crate::testkit::assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &a,
        &b,
        &response(&p, action),
        Side::Axis,
    );
}
/// Cases: airlog:53.24, airlog:54.11, airlog:54.13, land:3.6
#[test]
fn failed_later_transfer_rolls_back_stock_and_new_public_marker() {
    let mut s = fixture();
    s.cursor.index = OPSTAGE
        .iter()
        .position(|s| s.anchor == "opstage.organization.supply_distribution")
        .unwrap();
    let mut rng = CampaignRng::from_seed([7; 32]);
    enter_distribution(
        content(),
        &mut s,
        &mut Cx {
            rng: &mut rng,
            events: &mut vec![],
        },
    )
    .unwrap();
    let p = s
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap()
        .clone();
    let order = |n| {
        json!({
            "from":serde_json::to_string(&distribution::Endpoint::Dump("test-stock".into())).unwrap(),
            "to":serde_json::to_string(&distribution::Endpoint::Ground(Location::Hex{hex:"C4020".into()})).unwrap(),
            "amount":Supplies{stores:n,..Default::default()},"packing":CargoPacking::default()
        })
    };
    let before = serde_json::to_value(&s).unwrap();
    let mut events = vec![];
    assert!(
        answer(
            content(),
            &mut s,
            &p,
            &json!([order(5), order(999)]),
            &mut Cx {
                rng: &mut rng,
                events: &mut events
            },
            false
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert!(events.is_empty());
    answer(
        content(),
        &mut s,
        &p,
        &json!([order(5), order(7)]),
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        false,
    )
    .unwrap();
    assert_eq!(s.logistics.dumps["test-stock"].supplies.stores, 88);
    assert_eq!(s.logistics.next_dump_marker, 2);
}
