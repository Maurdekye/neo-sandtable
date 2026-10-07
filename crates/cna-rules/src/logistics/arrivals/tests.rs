use super::*;
use crate::{
    Cna,
    logistics::Rations,
    seq::{Block, OPSTAGE},
    state::{Dump, DumpLocation, Location, WeatherState},
};
use cna_content::{places::Place, scenario::Supplies, units::Trucks};
use cna_core::{
    decision::DecisionResponse,
    engine::{Command, Game, Ruleset, evaluate},
};
use cna_tables::land::weather::WeatherKind;
const AX: &str = "it.1_libyan_div.viii_libyan_bn";
const CW: &str = "cw.2_nz_div.21st_nz_bn";
fn fixture() -> (CnaContent, State) {
    let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut s = State::new(&c).unwrap();
    s.cursor.block = Block::OpStage;
    s.cursor.op_stage = Some(1);
    s.cursor.entered = true;
    s.cursor.index = OPSTAGE
        .iter()
        .position(|p| p.anchor == "opstage.convoy_arrival")
        .unwrap();
    s.turn.player_a = Some(Side::Axis);
    s.turn.weather = Some(WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    s.logistics.dumps.clear();
    s.logistics.rations.clear();
    s.logistics.unit_supply.clear();
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
            Location::NotArrived
        };
        u.trucks = Trucks::default();
        u.transport_trucks = Trucks::default();
        u.cp_spent_quarters = 0;
    }
    s.land.units.get_mut(&UnitId::new(AX)).unwrap().trucks.light = 1;
    s.logistics.rations.insert(
        CW.into(),
        Rations {
            issued_gt: Some(1),
            stores_received: 8,
            stores_required: 8,
            water_issue_stage: Some(water::WaterStage::current(&s)),
            water_stage: Some(water::WaterStage::current(&s)),
            infantry_water_received: 1,
            ..Default::default()
        },
    );
    s.logistics.dumps.insert(
        "arrival-stock".into(),
        Dump {
            id: "arrival-stock".into(),
            marker: "dump-test".into(),
            side: Side::Axis,
            location: DumpLocation::Hex {
                hex: "C4020".into(),
            },
            supplies: Supplies {
                stores: 30,
                water: 10,
                fuel: 10,
                ..Default::default()
            },
            active: true,
            dummy: false,
        },
    );
    (c, s)
}
fn start(c: &CnaContent, s: &mut State, ids: BTreeSet<UnitId>, rng: &mut CampaignRng) {
    enter(
        c,
        s,
        false,
        &mut Cx {
            rng,
            events: &mut vec![],
        },
        &ids,
    )
    .unwrap();
}
fn respond(
    c: &CnaContent,
    s: &mut State,
    p: &Pending,
    value: &Value,
    rng: &mut CampaignRng,
) -> Result<String, Rejection> {
    s.decisions.pending.retain(|other| other.id != p.id);
    answer(
        c,
        s,
        p,
        value,
        false,
        &mut Cx {
            rng,
            events: &mut vec![],
        },
    )
}
fn empty() -> Value {
    json!({"allocations":[],"well_allocations":[],"wells":[]})
}
fn issue(stores: i32, infantry: i32, activity: i32, pasta: bool, fuel: i32) -> Value {
    json!({"unit":AX,"stores":stores,"half":false,"infantry":infantry,"activity":activity,"pasta":pasta,"fuel_tenths":fuel,"draws":[{"source":serde_json::to_string(&SupplySource::Dump("arrival-stock".into())).unwrap(),"stores":stores,"water":infantry+activity+i32::from(pasta),"fuel_tenths":fuel}]})
}
fn command(p: &Pending, action: Value) -> Command {
    Command::Respond(DecisionResponse {
        decision_id: p.id.clone(),
        seat: p.seat,
        controller_epoch: 1,
        decision_revision: p.revision,
        idempotency_key: "arrival-test".into(),
        action,
        public_explanation: None,
    })
}
fn drain(
    c: &CnaContent,
    s: &mut State,
    rng: &mut CampaignRng,
    controller: &mut CampaignRng,
) -> usize {
    for count in 0..12 {
        if s.decisions.pending.is_empty() {
            finish(
                c,
                s,
                false,
                &mut Cx {
                    rng,
                    events: &mut vec![],
                },
            )
            .unwrap();
            if s.decisions.pending.is_empty() {
                return count;
            }
        }
        let p = s.decisions.pending[0].clone();
        let request = Cna::dev()
            .pending(c, s)
            .into_iter()
            .find(|r| r.id == p.id)
            .unwrap();
        let dice = rng.state();
        let value = crate::baseline::logistics_orders(c, s, &request, controller).unwrap();
        assert_eq!(rng.state(), dice);
        respond(c, s, &p, &value, rng).unwrap_or_else(|e| panic!("{value}: {e:?}"));
        assert_eq!(rng.state(), dice, "answers must not adjudicate wells");
    }
    panic!("arrival baseline did not close")
}
/// Cases: land:20.12, airlog:49.14, airlog:51.11, airlog:52.41, airlog:52.42, airlog:52.6, airlog:56.28
/// Interpretations: interp:airlog-0017
#[test]
fn real_arrival_consumes_actual_stocks_and_does_not_reassess_prior_units() {
    let (c, mut s) = fixture();
    let mut rng = CampaignRng::from_seed([3; 32]);
    let old = s.land.units[&UnitId::new(CW)].clone();
    let old_r = s.logistics.rations[&UnitId::new(CW)].clone();
    start(&c, &mut s, [AX.into()].into(), &mut rng);
    let pending = s.decisions.pending[0].clone();
    let full = rations::stores_required(&c, &s, &AX.into()).unwrap();
    let need = water::requirements(&c, &s, &AX.into()).unwrap();
    let value = json!({"allocations":[issue(full,need.infantry,need.activity,true,10)],"well_allocations":[],"wells":[]});
    respond(&c, &mut s, &pending, &value, &mut rng).unwrap();
    assert!(
        s.decisions.pending.len() <= 1,
        "one list handles all quantities"
    );
    // Fuel refill remains available, so pass any remaining optional refill window.
    if let Some(p) = s.decisions.pending.first().cloned() {
        respond(&c, &mut s, &p, &Value::Null, &mut rng).unwrap();
    }
    assert_eq!(
        s.logistics.dumps["arrival-stock"].supplies.stores,
        30 - full
    );
    assert_eq!(
        s.logistics.dumps["arrival-stock"].supplies.water,
        10 - need.infantry - need.activity - 1
    );
    assert_eq!(s.logistics.dumps["arrival-stock"].supplies.fuel, 9);
    assert_eq!(s.logistics.unit_supply[&AX.into()].tank_fuel.get(), 10);
    assert_eq!(
        s.logistics.unit_supply[&AX.into()].activity_water.get(),
        need.activity
    );
    let restrictions = super::super::movement_restrictions(&c, &s, &AX.into()).unwrap();
    assert!(restrictions.may_move && restrictions.may_exceed_cpa);
    assert_eq!(s.land.units[&CW.into()], old);
    assert_eq!(s.logistics.rations[&CW.into()], old_r);
    let saved = serde_json::to_value(&s).unwrap();
    let mut checkpoint: State = serde_json::from_value(saved.clone()).unwrap();
    start(&c, &mut checkpoint, [AX.into()].into(), &mut rng);
    assert_eq!(
        serde_json::to_value(checkpoint).unwrap(),
        saved,
        "entry is idempotent"
    );
}
/// Cases: land:20.12, airlog:51.22, airlog:52.53
/// Interpretations: interp:airlog-0017
#[test]
fn pass_does_not_create_supplies_and_shortages_are_scoped_to_arrivals() {
    let (c, mut s) = fixture();
    let mut rng = CampaignRng::from_seed([3; 32]);
    s.logistics.dumps.clear();
    let old = s.logistics.rations[&UnitId::new(CW)].clone();
    start(&c, &mut s, [AX.into()].into(), &mut rng);
    let p = s.decisions.pending[0].clone();
    respond(&c, &mut s, &p, &Value::Null, &mut rng).unwrap();
    assert_eq!(s.logistics.rations[&AX.into()].stores_received, 0);
    assert_eq!(s.logistics.rations[&AX.into()].consecutive_short_gt, 1);
    assert_eq!(
        s.logistics.rations[&AX.into()].consecutive_short_water_stages,
        1
    );
    assert_eq!(s.logistics.rations[&CW.into()], old);
    assert!(
        s.logistics
            .unit_supply
            .get(&AX.into())
            .is_none_or(|h| h == &Default::default())
    );
    assert!(
        !super::super::movement_restrictions(&c, &s, &AX.into())
            .unwrap()
            .may_move
    );
    assert!(s.decisions.pending.is_empty());
}
/// Cases: airlog:49.14, airlog:51.11, airlog:52.41, land:3.6
#[test]
fn bad_final_item_foreign_ids_and_duplicate_issues_are_atomic() {
    let (c, mut s) = fixture();
    let mut rng = CampaignRng::from_seed([4; 32]);
    start(&c, &mut s, [AX.into()].into(), &mut rng);
    let p = s.decisions.pending[0].clone();
    let full = rations::stores_required(&c, &s, &AX.into()).unwrap();
    let good = issue(full, 1, 1, true, 10);
    let mut foreign = good.clone();
    foreign["unit"] = json!(CW);
    let mut excessive = good.clone();
    excessive["fuel_tenths"] = json!(i32::MAX);
    for allocations in [
        json!([good.clone(), good.clone()]),
        json!([foreign]),
        json!([excessive]),
    ] {
        let before = serde_json::to_value(&s).unwrap();
        let dice = rng.state();
        let mut events = vec![];
        let result = answer(
            &c,
            &mut s,
            &p,
            &json!({"allocations":allocations,"well_allocations":[],"wells":[]}),
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        );
        assert!(result.is_err());
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        assert_eq!(rng.state(), dice);
        assert!(events.is_empty());
    }
    // Two real arrival units share a dump: an individually legal final item overdraws it.
    let second: UnitId = s
        .land
        .units
        .keys()
        .find(|id| {
            id.as_str() != AX
                && c.units.units[*id]
                    .class
                    .as_ref()
                    .is_some_and(|class| c.units.classes[class].unit_type == "infantry")
                && s.land.units[*id].side == Side::Axis
        })
        .unwrap()
        .clone();
    let u = s.land.units.get_mut(&second).unwrap();
    u.location = Location::Hex {
        hex: "C4020".into(),
    };
    u.trucks = Default::default();
    u.transport_trucks = Default::default();
    s.logistics.arrival_supply.units.insert(second.clone());
    let needed = rations::stores_required(&c, &s, &second).unwrap();
    s.logistics
        .dumps
        .get_mut("arrival-stock")
        .unwrap()
        .supplies
        .stores = full;
    let mut last = issue(needed, 0, 0, false, 0);
    last["unit"] = json!(second);
    let before = serde_json::to_value(&s).unwrap();
    assert!(
        answer(
            &c,
            &mut s,
            &p,
            &json!({"allocations":[good,last],"well_allocations":[],"wells":[]}),
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut vec![]
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}
/// Cases: land:20.12, airlog:51.11, airlog:52.13, airlog:52.41, land:3.6
#[test]
fn arrival_baselines_feed_real_units_and_finish_across_64_seeds() {
    let (c, template) = fixture();
    for seed in 0..64 {
        let mut s = template.clone();
        let mut rng = CampaignRng::from_seed([seed; 32]);
        let mut controller = CampaignRng::from_seed([seed ^ 63; 32]);
        start(&c, &mut s, [AX.into()].into(), &mut rng);
        assert!(drain(&c, &mut s, &mut rng, &mut controller) <= 3);
        assert_eq!(
            s.logistics.rations[&AX.into()].stores_received,
            rations::stores_required(&c, &s, &AX.into()).unwrap()
        );
        assert_eq!(s.logistics.rations[&AX.into()].infantry_water_received, 1);
        assert!(s.logistics.unit_supply[&AX.into()].tank_fuel.get() > 0);
        assert!(
            super::super::movement_restrictions(&c, &s, &AX.into())
                .unwrap()
                .may_move
        );
    }
}
/// Cases: airlog:52.13, airlog:52.14, airlog:52.16, land:3.6
#[test]
fn hidden_well_conditions_cannot_change_arrival_answer_acceptance() {
    let (mut c, mut s) = fixture();
    // Explicit source fixture on the real grid; it is not a map transcription.
    c.places.places.insert(
        "arrival-fixture-bir".into(),
        Place {
            id: "arrival-fixture-bir".into(),
            name: "Test source".into(),
            hex_id: "C4020".into(),
            kind: "bir".into(),
            place_group: None,
            src: vec!["airlog:52.11".into()],
            note: None,
            review_batch: "test-fixture".into(),
        },
    );
    let u = s.land.units.get_mut(&UnitId::new(CW)).unwrap();
    u.location = Location::Hex {
        hex: "C4020".into(),
    };
    u.trucks = Default::default();
    s.logistics.rations.remove(&CW.into());
    s.logistics.unit_supply.remove(&CW.into());
    let mut rng = CampaignRng::from_seed([5; 32]);
    start(&c, &mut s, [CW.into()].into(), &mut rng);
    let p = s.decisions.pending[0].clone();
    let a = Game::<Cna> {
        state: s,
        rng: rng.state(),
    };
    let mut b = a.clone();
    let well = b.state.logistics.wells.entry("C4020".into()).or_default();
    well.poisoned = true;
    well.poisoned_known.insert(Side::Axis);
    let value = json!({"allocations":[],"well_allocations":[],"wells":[{"unit":CW,"requested":1,"packing":CargoPacking::default()}]});
    let command = command(&p, value);
    crate::testkit::assert_action_indistinguishable(
        &Cna::dev(),
        &c,
        &a,
        &b,
        &command,
        Side::Commonwealth,
    );
    let t = evaluate(&Cna::dev(), &c, &b, &command).unwrap().game;
    assert_eq!(t.rng, a.rng);
    assert!(t.state.logistics.drawn_water.is_empty());
    assert_eq!(t.state.land.units[&CW.into()].cp_spent_quarters, 4);
    let mut checkpoint: Game<Cna> =
        serde_json::from_value(serde_json::to_value(&t).unwrap()).unwrap();
    let mut rng = CampaignRng::from_state(&checkpoint.rng);
    let mut events = vec![];
    finish(
        &c,
        &mut checkpoint.state,
        false,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert_eq!(checkpoint.state.logistics.drawn_water[&CW.into()].points, 0);
    assert_eq!(
        wells::condition(
            &checkpoint.state,
            &"C4020".into(),
            Perspective::Side(Side::Commonwealth)
        )["poisoned"],
        true
    );
    assert!(
        events
            .iter()
            .filter(|e| Perspective::Side(Side::Axis).can_see(&e.audience))
            .all(|e| !matches!(&e.event,GameEvent::Note{text}if text.contains("requested")))
    );
}
/// Cases: airlog:52.11, airlog:52.13, land:20.12
#[test]
fn actual_city_well_baseline_resolves_then_pays_body_and_truck_obligations() {
    let (c, mut s) = fixture();
    let mut rng = CampaignRng::from_seed([6; 32]);
    let mut controller = CampaignRng::from_seed([2; 32]);
    let unit = s.land.units.get_mut(&UnitId::new(CW)).unwrap();
    unit.trucks.light = 1;
    s.logistics.rations.remove(&CW.into());
    s.logistics.unit_supply.remove(&CW.into());
    start(&c, &mut s, [CW.into()].into(), &mut rng);
    assert!(drain(&c, &mut s, &mut rng, &mut controller) <= 4);
    assert_eq!(s.logistics.rations[&CW.into()].infantry_water_received, 1);
    assert_eq!(s.land.units[&CW.into()].cp_spent_quarters, 4);
    assert_eq!(
        super::super::activity_water_due(&c, &s, &CW.into()).unwrap(),
        0
    );
    assert!(!s.logistics.drawn_water.contains_key(&CW.into()));
}
/// Cases: land:3.6, airlog:51.11, airlog:52.41
#[test]
fn enemy_learns_nothing_from_private_arrival_quantities() {
    let (c, mut s) = fixture();
    let mut rng = CampaignRng::from_seed([3; 32]);
    start(&c, &mut s, [AX.into()].into(), &mut rng);
    let p = s.decisions.pending[0].clone();
    let a = Game::<Cna> {
        state: s,
        rng: rng.state(),
    };
    let mut fed = empty();
    fed["allocations"] = json!([issue(
        rations::stores_required(&c, &a.state, &AX.into()).unwrap(),
        1,
        1,
        true,
        10
    )]);
    crate::testkit::assert_actions_indistinguishable(
        &Cna::dev(),
        &c,
        (&a, &command(&p, fed)),
        (&a, &command(&p, Value::Null)),
        Side::Commonwealth,
    );
}
/// Cases: land:20.12, airlog:56.28
#[test]
fn empty_arrival_roster_opens_nothing_and_old_stage_flags_do_not_repeat() {
    let (c, mut s) = fixture();
    let mut rng = CampaignRng::from_seed([2; 32]);
    start(&c, &mut s, BTreeSet::new(), &mut rng);
    assert!(s.decisions.pending.is_empty());
    finish(
        &c,
        &mut s,
        false,
        &mut Cx {
            rng: &mut rng,
            events: &mut vec![],
        },
    )
    .unwrap();
    assert!(s.decisions.pending.is_empty());
    s.cursor.op_stage = Some(2);
    start(&c, &mut s, [AX.into()].into(), &mut rng);
    assert_eq!(s.decisions.pending.len(), 1);
}

/// Cases: airlog:52.42, land:3.6
#[test]
fn unknown_arriving_hq_is_full_unsupported_or_dev_private_unassessed() {
    let (c, mut s) = fixture();
    let id = s
        .land
        .units
        .keys()
        .find(|id| {
            matches!(
                water::requirements(&c, &s, id),
                Err(SupplyError::Unsupported {
                    case: "airlog:52.42"
                })
            )
        })
        .unwrap()
        .clone();
    s.land.units.get_mut(&id).unwrap().location = Location::Hex {
        hex: "C4020".into(),
    };
    let before = s.land.units[&id].clone();
    let side = before.side;
    let mut strict = s.clone();
    let mut rng = CampaignRng::from_seed([1; 32]);
    assert!(
        matches!(enter(&c,&mut strict,true,&mut Cx{rng:&mut rng,events:&mut vec![]},&[id.clone()].into()),Err(EngineError::Unsupported{case,..})if case=="airlog:52.42")
    );
    let mut events = vec![];
    enter(
        &c,
        &mut s,
        false,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        &[id.clone()].into(),
    )
    .unwrap();
    assert!(s.decisions.pending.is_empty());
    assert_eq!(s.land.units[&id], before);
    assert!(!s.logistics.rations.contains_key(&id));
    assert!(!s.logistics.unit_supply.contains_key(&id));
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event,GameEvent::Note{text}if text.contains("unassessed")))
    );
    assert!(events.iter().all(|e| {
        Perspective::Side(side).can_see(&e.audience)
            && !Perspective::Side(if side == Side::Axis {
                Side::Commonwealth
            } else {
                Side::Axis
            })
            .can_see(&e.audience)
    }));
}

/// Cases: land:20.12, land:4.48, airlog:49.12, airlog:49.14, airlog:50.17, airlog:51.11, airlog:52.42
/// Interpretations: interp:airlog-0017
#[test]
fn actual_classless_v_medium_tank_arrival_has_known_consumption_and_accepted_baselines() {
    const TANK: &str = "it.unassigned_armored.v_m_tank_bn";
    for strict in [false, true] {
        for seed in 0..8 {
            let (c, mut s) = fixture();
            let row = &c.units.units[&UnitId::new(TANK)];
            assert!(row.class.is_none());
            assert_eq!(
                row.arrives,
                cna_content::units::Arrival::At { gt: 13, opstage: 3 }
            );
            let unit = s.land.units.get_mut(&TANK.into()).unwrap();
            unit.location = Location::Hex {
                hex: "C4020".into(),
            };
            unit.trucks = Trucks::default();
            unit.transport_trucks = Trucks::default();
            s.cursor.game_turn = 13;
            s.cursor.op_stage = Some(3);
            s.logistics.dumps.get_mut("arrival-stock").unwrap().supplies = Supplies {
                stores: 60,
                water: 30,
                fuel: 100,
                ..Default::default()
            };
            assert_eq!(rations::stores_required(&c, &s, &TANK.into()).unwrap(), 28);
            let need = water::requirements(&c, &s, &TANK.into()).unwrap();
            assert_eq!((need.infantry, need.activity), (0, 7));
            assert_eq!(
                super::super::fuel_capacity(&c, &s, &TANK.into())
                    .unwrap()
                    .get(),
                560
            );
            assert_eq!(
                super::super::movement_fuel_cost(&c, &s, &TANK.into(), 4)
                    .unwrap()
                    .get(),
                28
            );
            assert_eq!(
                super::super::ready_ammo_capacity(&c, &s, &TANK.into())
                    .unwrap()
                    .get(),
                21
            );
            assert!(matches!(
                super::super::close_assault_ammo_action(&c, &TANK.into()).unwrap(),
                cna_tables::airlog::supply::AmmoAction::CloseAssaultArmorGunMgInfHvywpnInf
            ));
            let mut rng = CampaignRng::from_seed([seed; 32]);
            let mut controller = CampaignRng::from_seed([seed + 20; 32]);
            enter(
                &c,
                &mut s,
                strict,
                &mut Cx {
                    rng: &mut rng,
                    events: &mut vec![],
                },
                &[TANK.into()].into(),
            )
            .unwrap();
            for _ in 0..4 {
                if s.decisions.pending.is_empty() {
                    break;
                }
                let p = s.decisions.pending.remove(0);
                let action = baseline(&c, &s, p.seat.side, &mut controller);
                let dice = rng.state();
                answer(
                    &c,
                    &mut s,
                    &p,
                    &action,
                    strict,
                    &mut Cx {
                        rng: &mut rng,
                        events: &mut vec![],
                    },
                )
                .unwrap_or_else(|e| panic!("strict={strict}, {action}: {e:?}"));
                assert_eq!(rng.state(), dice);
            }
            assert!(s.decisions.pending.is_empty());
            finish(
                &c,
                &mut s,
                strict,
                &mut Cx {
                    rng: &mut rng,
                    events: &mut vec![],
                },
            )
            .unwrap();
            assert_eq!(s.logistics.dumps["arrival-stock"].supplies.stores, 32);
            assert_eq!(s.logistics.dumps["arrival-stock"].supplies.water, 22);
            assert_eq!(s.logistics.dumps["arrival-stock"].supplies.fuel, 44);
            assert_eq!(
                s.logistics.unit_supply[&TANK.into()].activity_water.get(),
                7
            );
            assert!(
                super::super::movement_restrictions(&c, &s, &TANK.into())
                    .unwrap()
                    .may_move
            );
        }
    }
}

/// Cases: land:4.48, airlog:49.12, airlog:50.17, airlog:51.11, airlog:52.42
#[test]
fn all_nine_real_classless_weapon_rows_price_composition_without_a_class_default() {
    let (c, mut s) = fixture();
    let ids: Vec<_> = c
        .units
        .units
        .values()
        .filter(|row| {
            row.class.is_none() && matches!(row.toe, Some(cna_content::units::Toe::Weapons(_)))
        })
        .map(|row| row.id.clone())
        .collect();
    assert_eq!(ids.len(), 9);
    for id in ids {
        s.land.units.get_mut(&id).unwrap().location = Location::Hex {
            hex: "C4020".into(),
        };
        let strength = super::super::toe_strength(&c, &s.land.units[&id])
            .unwrap()
            .get();
        assert_eq!(rations::stores_required(&c, &s, &id).unwrap(), strength * 4);
        assert_eq!(water::requirements(&c, &s, &id).unwrap().activity, strength);
        assert!(super::super::fuel_capacity(&c, &s, &id).unwrap().get() > 0);
        assert!(
            super::super::movement_fuel_cost(&c, &s, &id, 4)
                .unwrap()
                .get()
                > 0
        );
        assert!(
            super::super::ready_ammo_capacity(&c, &s, &id)
                .unwrap()
                .get()
                > 0
        );
    }
}

/// Malformed own lists must not disclose the enemy's privately placed arrival count.
/// Cases: land:3.6, land:20.12, airlog:52.13
#[test]
fn rejected_arrival_lists_are_independent_of_hidden_enemy_rosters() {
    let (c, mut base) = fixture();
    let second: UnitId = base
        .land
        .units
        .iter()
        .find_map(|(id, u)| {
            (u.side == Side::Axis
                && id.as_str() != AX
                && c.units.units[id]
                    .class
                    .as_ref()
                    .is_some_and(|cl| c.units.classes[cl].unit_type == "infantry"))
            .then(|| id.clone())
        })
        .unwrap();
    for id in [UnitId::new(AX), second.clone()] {
        let u = base.land.units.get_mut(&id).unwrap();
        u.location = Location::OffMap {
            id: "box_tripoli".into(),
        };
        u.trucks = Trucks::default();
        u.transport_trucks = Trucks::default();
    }
    base.land.units.get_mut(&UnitId::new(CW)).unwrap().location = Location::NotArrived;
    for list_name in ["allocations", "well_allocations", "wells"] {
        let mut s = base.clone();
        if list_name == "well_allocations" {
            for id in [UnitId::new(AX), second.clone()] {
                s.logistics
                    .drawn_water
                    .insert(id, wells::DrawnWater { points: 2 });
            }
        }
        let mut rng = CampaignRng::from_seed([71; 32]);
        start(&c, &mut s, [AX.into(), second.clone()].into(), &mut rng);
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
        b.state
            .land
            .units
            .get_mut(&UnitId::new(CW))
            .unwrap()
            .location = Location::OffMap {
            id: "box_tripoli".into(),
        };
        b.state.logistics.arrival_supply.units.insert(CW.into());
        crate::testkit::assert_indistinguishable(&Cna::dev(), &c, &a.state, &b.state, Side::Axis);
        let item = match list_name {
            "allocations" => json!({"unit":AX,"stores":0,"half":false,"pasta":false,
               "infantry":0,"activity":0,"fuel_tenths":0,"draws":[]}),
            "well_allocations" => json!({"unit":AX,"infantry":0,"activity":0,"pasta":false,
               "cargo":0,"packing":CargoPacking::default()}),
            _ => json!({"unit":AX,"requested":1,"packing":CargoPacking::default()}),
        };
        let mut foreign = item.clone();
        foreign["unit"] = json!(CW);
        for (kind, values) in [
            (
                "oversized",
                json!([item.clone(), item.clone(), item.clone()]),
            ),
            ("duplicate", json!([item.clone(), item.clone()])),
            ("foreign", json!([foreign])),
        ] {
            let mut value = empty();
            value[list_name] = values;
            let cmd = command(&p, value.clone());
            for rules in [Cna::dev(), Cna::full()] {
                crate::testkit::assert_action_indistinguishable(
                    &rules,
                    &c,
                    &a,
                    &b,
                    &cmd,
                    Side::Axis,
                );
                assert!(
                    evaluate(&rules, &c, &a, &cmd).is_err(),
                    "{list_name}/{kind} accepted"
                );
                // Exercise the module directly too: the central action-space guard
                // must not hide a regression in this independent validation layer.
                let errors = [&a, &b].map(|g| {
                    let mut s = g.state.clone();
                    let before = serde_json::to_value(&s).unwrap();
                    let mut rng = CampaignRng::from_state(&g.rng);
                    let mut events = vec![];
                    let error = answer(
                        &c,
                        &mut s,
                        &p,
                        &value,
                        rules.strict,
                        &mut Cx {
                            rng: &mut rng,
                            events: &mut events,
                        },
                    )
                    .unwrap_err();
                    assert_eq!(serde_json::to_value(&s).unwrap(), before);
                    assert_eq!(rng.state(), g.rng);
                    assert!(events.is_empty());
                    format!("{error:?}")
                });
                assert_eq!(errors[0], errors[1], "{list_name}/{kind}");
                assert!(
                    errors[0].contains(if kind == "oversized" {
                        "too many arrival allocations"
                    } else if kind == "duplicate" {
                        "repeated"
                    } else {
                        "foreign"
                    }),
                    "{list_name}/{kind}: {}",
                    errors[0]
                );
            }
        }
    }
}
