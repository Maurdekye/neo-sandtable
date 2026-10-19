use super::*;
use crate::{
    Cna,
    state::{AirSquadron, PlaneCount},
    testkit::{
        assert_action_indistinguishable, assert_actions_indistinguishable, assert_indistinguishable,
    },
};
use cna_content::scenario::Supplies;
use cna_core::{
    decision::DecisionResponse,
    dice::CampaignRng,
    engine::{Command, Game, evaluate},
};
use std::sync::OnceLock;

mod fueling_tests;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn base(strategic: bool) -> State {
    let mut state = State::new(content()).unwrap();
    state.setup.closed = true;
    // Bound unrelated ground inspection while retaining every Air assertion.
    state.land.units.clear();
    state.logistics.unit_supply.clear();
    state.logistics.truck_pools.clear();
    state.logistics.dumps.clear();
    let force = state.air.forces.get_mut("axis").unwrap();
    let aircraft = force
        .planes
        .iter()
        .find(|(_, c)| c.total >= 2 && c.ready >= 2 && c.fuelled >= 2 && c.armed >= 2)
        .unwrap()
        .0
        .clone();
    let counts = force.planes.get_mut(&aircraft).unwrap();
    counts.total -= 2;
    counts.ready -= 2;
    counts.fuelled -= 2;
    counts.armed -= 2;
    let nation = content().units.aircraft[&aircraft].nation.clone();
    state.air.squadrons.insert(
        "axis.refit".into(),
        AirSquadron {
            id: "axis.refit".into(),
            force: "axis".into(),
            side: Side::Axis,
            nationality: nation,
            facility: "airfield_benina".into(),
            initial_aircraft: None,
            planes: BTreeMap::from([(
                aircraft,
                PlaneCount {
                    total: 2,
                    ready: 0,
                    fuelled: 0,
                    armed: 0,
                },
            )]),
            pilots: BTreeMap::new(),
        },
    );
    state.air.squadrons.insert(
        "axis.foreign".into(),
        AirSquadron {
            id: "axis.foreign".into(),
            force: "axis".into(),
            side: Side::Axis,
            nationality: "ge".into(),
            facility: "airfield_benina".into(),
            initial_aircraft: None,
            planes: BTreeMap::new(),
            pilots: BTreeMap::new(),
        },
    );
    inventory::initialize(content(), &mut state).unwrap();
    state.air.runtime.designation.game_turn = Some(state.cursor.game_turn);
    state.air.runtime.designation.finished = true;
    state.air.runtime.designation.assignments.insert(
        "axis.refit".into(),
        if strategic {
            Family::Convoy
        } else {
            Family::LandSupport
        },
    );
    state.logistics.air_dumps.insert(
        "refit.stock".into(),
        air_supply::AirDump {
            id: "refit.stock".into(),
            facility: FacilityId("airfield_benina".into()),
            side: Side::Axis,
            supplies: Supplies {
                fuel: 4,
                stores: 8,
                water: 4,
                ammo: 0,
            },
        },
    );
    state.cursor.block = if strategic {
        crate::seq::Block::Post
    } else {
        crate::seq::Block::OpStage
    };
    state.cursor.index = state
        .cursor
        .block
        .steps()
        .iter()
        .position(|step| step.anchor == if strategic { STRATEGIC } else { TACTICAL })
        .unwrap();
    state.cursor.op_stage = if strategic { None } else { Some(2) };
    state.cursor.entered = true;
    state.turn.player_a = Some(Side::Axis);
    state
}
fn opened(strategic: bool) -> Game<Cna> {
    let mut state = base(strategic);
    let mut rng = CampaignRng::from_seed([41; 32]);
    let before = rng.state();
    let mut events = Vec::new();
    enter(
        content(),
        &mut state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        false,
    )
    .unwrap();
    assert_eq!(rng.state(), before);
    assert_eq!(state.decisions.pending.len(), 2);
    Game {
        state,
        rng: rng.state(),
    }
}
fn plan(state: &State, dues: bool) -> RefitPlan {
    let priority = state
        .air
        .runtime
        .aircraft
        .iter()
        .filter(|(_, p)| p.squadron.as_deref() == Some("axis.refit"))
        .map(|(id, _)| id.clone())
        .collect();
    RefitPlan {
        fueling: Vec::new(),
        dues: if dues {
            vec![SgsuId("axis.refit".into())]
        } else {
            Vec::new()
        },
        cohorts: vec![refit::RefitCohort {
            servicer: SgsuId("axis.refit".into()),
            assigned_squadron: "axis.refit".into(),
            priority,
            method: RefitMethod::Squadron,
        }],
        draws: vec![refit::ConsumerDraws {
            servicer: SgsuId("axis.refit".into()),
            draws: vec![crate::logistics::SupplyDraw {
                source: SupplySource::AirDump("refit.stock".into()),
                amount: SupplyDemand {
                    fuel: FuelTenths::new(if dues { 10 } else { 0 }),
                    stores: StoresPoints::new(if dues { 2 } else { 1 }),
                    water: WaterPoints::new(if dues { 1 } else { 0 }),
                    ..Default::default()
                },
            }],
        }],
    }
}
fn command(game: &Game<Cna>, side: Side, action: Value) -> Command {
    let pending = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == side)
        .unwrap();
    Command::Respond(DecisionResponse {
        decision_id: pending.id.clone(),
        seat: pending.seat,
        controller_epoch: 1,
        decision_revision: pending.revision,
        idempotency_key: pending.id.to_string(),
        action,
        public_explanation: None,
    })
}
fn respond(game: &Game<Cna>, side: Side, action: Value) -> Game<Cna> {
    evaluate(&Cna::dev(), content(), game, &command(game, side, action))
        .unwrap()
        .game
}
fn finish_game(game: &mut Game<Cna>) {
    let mut rng = CampaignRng::from_state(&game.rng);
    let mut events = Vec::new();
    finish(
        content(),
        &mut game.state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert!(events.is_empty());
    game.rng = rng.state();
}

fn offmap_game(pay_dues: bool) -> Game<Cna> {
    let mut state = base(false);
    let site = FacilityId("airfield_deversoir".into());
    let paid = dues_stage(period(&state).unwrap()).unwrap();
    inventory::update(content(), &mut state.air, |runtime| {
        for s in runtime
            .sgsus
            .values_mut()
            .filter(|s| owned(&s.force, Side::Axis))
        {
            s.position = SgsuPosition::Facility(site.clone());
            if !pay_dues {
                s.stores_paid_game_turn = Some(paid.game_turn);
                s.fuel_water_paid = Some(paid);
            }
        }
        for p in runtime
            .aircraft
            .values_mut()
            .filter(|p| p.squadron.as_deref() == Some("axis.refit"))
        {
            p.facility = Some(site.0.clone());
        }
        Ok(())
    })
    .unwrap();
    state
        .logistics
        .air_dumps
        .get_mut("refit.stock")
        .unwrap()
        .facility = site;
    let mut rng = CampaignRng::from_seed([41; 32]);
    enter(
        content(),
        &mut state,
        &mut Cx {
            rng: &mut rng,
            events: &mut Vec::new(),
        },
        false,
    )
    .unwrap();
    Game {
        state,
        rng: rng.state(),
    }
}

fn offmap_plan(state: &State, pay_dues: bool) -> RefitPlan {
    let mut selected = plan(state, pay_dues);
    if pay_dues {
        selected.draws[0].draws[0].amount.stores = StoresPoints::new(1);
    } else {
        selected.draws.clear();
    }
    selected
}

/// Cases: airlog:35.14, airlog:36.5, airlog:38.33, airlog:38.36, land:3.6
#[test]
fn offmap_refit_records_logical_stores_and_only_spends_finite_dues() {
    for pay_dues in [false, true] {
        let mut game = offmap_game(pay_dues);
        if !pay_dues {
            game.state.logistics.air_dumps.clear();
        }
        let before = game.clone();
        let action = serde_json::to_value(offmap_plan(&game.state, pay_dues)).unwrap();
        let one = respond(&game, Side::Axis, action);
        assert_eq!(one.rng, before.rng);
        assert_eq!(
            serde_json::to_value(&one.state.logistics).unwrap(),
            serde_json::to_value(&before.state.logistics).unwrap()
        );
        assert_eq!(
            one.state.air.runtime.aircraft,
            before.state.air.runtime.aircraft
        );
        assert!(one.state.air.runtime.maintenance.receipts.is_empty());
        let restored: Game<Cna> =
            serde_json::from_slice(&serde_json::to_vec(&one).unwrap()).unwrap();
        let mut a = respond(&one, Side::Commonwealth, Value::Null);
        let mut b = respond(&restored, Side::Commonwealth, Value::Null);
        finish_game(&mut a);
        finish_game(&mut b);
        assert_eq!(
            serde_json::to_vec(&a).unwrap(),
            serde_json::to_vec(&b).unwrap()
        );
        let receipt = &a.state.air.runtime.maintenance.receipts[&Side::Axis][0];
        assert_eq!(
            receipt.stores_entitlement,
            Some(refit::SourceEntitlement {
                facility: FacilityId("airfield_deversoir".into()),
                stores: StoresPoints::new(1),
            })
        );
        if pay_dues {
            assert_eq!(
                a.state.logistics.air_dumps["refit.stock"].supplies,
                Supplies {
                    fuel: 3,
                    stores: 7,
                    water: 3,
                    ammo: 0
                }
            );
        } else {
            assert_eq!(
                serde_json::to_value(&a.state.logistics).unwrap(),
                serde_json::to_value(&before.state.logistics).unwrap()
            );
            assert!(
                a.state
                    .air
                    .runtime
                    .maintenance
                    .debits
                    .values()
                    .all(Vec::is_empty)
            );
        }
        for id in &receipt.cohort.priority {
            assert_eq!(
                a.state.air.runtime.aircraft[id].refitted,
                receipt.successful.contains(id)
            );
            assert!(!a.state.air.runtime.aircraft[id].fuelled);
            assert!(!a.state.air.runtime.aircraft[id].armed);
        }
        let completed = serde_json::to_vec(&a).unwrap();
        finish_game(&mut a);
        assert_eq!(serde_json::to_vec(&a).unwrap(), completed);
    }
}

/// Cases: airlog:36.5, airlog:38.36, land:3.6
#[test]
fn offmap_private_service_and_provenance_preserve_enemy_streams_through_advance() {
    let a = offmap_game(true);
    let mut b = a.clone();
    b.state
        .logistics
        .air_dumps
        .get_mut("refit.stock")
        .unwrap()
        .supplies
        .stores += 3;
    assert_indistinguishable(
        &Cna::dev(),
        content(),
        &a.state,
        &b.state,
        Side::Commonwealth,
    );
    let act = command(
        &a,
        Side::Axis,
        serde_json::to_value(offmap_plan(&a.state, true)).unwrap(),
    );
    assert_action_indistinguishable(&Cna::dev(), content(), &a, &b, &act, Side::Commonwealth);
    let pass = command(&a, Side::Axis, Value::Null);
    assert_actions_indistinguishable(
        &Cna::dev(),
        content(),
        (&a, &act),
        (&a, &pass),
        Side::Commonwealth,
    );
    let serviced = respond(
        &respond(
            &a,
            Side::Axis,
            serde_json::to_value(offmap_plan(&a.state, true)).unwrap(),
        ),
        Side::Commonwealth,
        Value::Null,
    );
    let passed = respond(
        &respond(&a, Side::Axis, Value::Null),
        Side::Commonwealth,
        Value::Null,
    );
    assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &serviced,
        &passed,
        &Command::Advance,
        Side::Commonwealth,
    );
}

/// Cases: airlog:36.5, airlog:38.36, airlog:34.0
#[test]
fn late_offmap_inventory_failure_rolls_back_state_rng_and_entitlement() {
    let game = offmap_game(true);
    let one = respond(
        &game,
        Side::Axis,
        serde_json::to_value(offmap_plan(&game.state, true)).unwrap(),
    );
    let mut accepted = respond(&one, Side::Commonwealth, Value::Null);
    // Trusted mirror corruption is discovered by canonical commit after dice.
    accepted
        .state
        .air
        .forces
        .get_mut("axis")
        .unwrap()
        .planes
        .values_mut()
        .next()
        .unwrap()
        .total += 1;
    let before = serde_json::to_vec(&accepted).unwrap();
    let mut rng = CampaignRng::from_state(&accepted.rng);
    let mut events = Vec::new();
    assert!(
        finish(
            content(),
            &mut accepted.state,
            &mut Cx {
                rng: &mut rng,
                events: &mut events
            }
        )
        .is_err()
    );
    assert_eq!(rng.state(), accepted.rng);
    assert!(events.is_empty());
    assert_eq!(serde_json::to_vec(&accepted).unwrap(), before);
}

fn rejected_response(content: &CnaContent, game: &Game<Cna>, action: Value) -> Rejection {
    let before = serde_json::to_vec(game).unwrap();
    let rejection = match evaluate(
        &Cna::dev(),
        content,
        game,
        &command(game, Side::Axis, action),
    ) {
        Err(rejection) => rejection,
        Ok(_) => panic!("invalid maintenance response was accepted"),
    };
    // Includes pending answers, closed markers, stock, aircraft and RNG.
    assert_eq!(serde_json::to_vec(game).unwrap(), before);
    rejection
}

fn opened_first_period() -> Game<Cna> {
    let mut state = base(false);
    state.cursor.game_turn = content().scenario.meta.start.gt;
    state.cursor.op_stage = Some(content().scenario.meta.start.opstage);
    let mut rng = CampaignRng::from_seed([41; 32]);
    let before_rng = rng.state();
    let mut events = Vec::new();
    enter(
        content(),
        &mut state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        false,
    )
    .unwrap();
    assert_eq!(rng.state(), before_rng);
    assert_eq!(state.decisions.pending.len(), 2);
    Game {
        state,
        rng: rng.state(),
    }
}

fn cohort_maximum(space: &ActionSpace) -> u32 {
    let ActionSchema::Record { fields } = &space.schema else {
        panic!("refit record required")
    };
    let ActionSchema::List { max, .. } =
        &fields.iter().find(|f| f.name == "cohorts").unwrap().schema
    else {
        panic!("cohort list required")
    };
    *max
}

/// Cases: scen:59.36, airlog:35.14, airlog:38.33
#[test]
fn first_period_refit_domain_is_zero_without_changing_dues_draws_or_later_domains() {
    let first = opened_first_period();
    let later = opened(false);
    let strategic = opened(true);
    assert!(first_bar(content(), period(&first.state).unwrap()));
    assert!(!first_bar(content(), period(&later.state).unwrap()));
    assert!(!first_bar(content(), period(&strategic.state).unwrap()));
    for owner in Side::ALL {
        let pending = first
            .state
            .decisions
            .pending
            .iter()
            .find(|p| p.seat.side == owner)
            .unwrap();
        assert_eq!(pending.kind, KIND);
        assert_eq!(pending.seat.role, Role::Air);
        assert!(pending.space.pass.is_some());
        assert_eq!(cohort_maximum(&pending.space), 0);
        let positive = u32::try_from(
            later
                .state
                .air
                .runtime
                .aircraft
                .values()
                .filter(|p| owned(&p.force, owner))
                .count(),
        )
        .unwrap();
        assert!(positive > 0);
        let mut expected = space(
            content(),
            &later.state,
            owner,
            period(&later.state).unwrap(),
        )
        .unwrap();
        // The finite DEV procedure advertises no cohort when this owner has
        // planes but no implemented servicer; its full roster bound is kept
        // whenever an operational finite consumer exists.
        let later_max = if finite_cohort_servicers(content(), &later.state, owner)
            .unwrap()
            .is_empty()
        {
            0
        } else {
            positive
        };
        assert_eq!(cohort_maximum(&expected), later_max);
        assert_eq!(
            cohort_maximum(
                &space(
                    content(),
                    &strategic.state,
                    owner,
                    period(&strategic.state).unwrap()
                )
                .unwrap()
            ),
            later_max
        );
        let ActionSchema::Record { fields } = &mut expected.schema else {
            unreachable!()
        };
        for name in ["cohorts", "fueling"] {
            let ActionSchema::List { max, .. } = &mut fields
                .iter_mut().find(|f| f.name == name).unwrap().schema
            else { unreachable!() };
            *max = 0;
        }
        // Full domain equality preserves dues/draws, inner priority and all
        // own context; only the public initial-period aircraft service bounds change.
        assert_eq!(pending.space, expected);
        assert!(pending.space.check(&Value::Null).is_ok());
        assert!(
            pending
                .space
                .check(&serde_json::to_value(RefitPlan::default()).unwrap())
                .is_ok()
        );
    }
    let proposed = plan(&first.state, true);
    let mut dues_only = proposed.clone();
    dues_only.cohorts.clear();
    let own_space = space(
        content(),
        &first.state,
        Side::Axis,
        period(&first.state).unwrap(),
    )
    .unwrap();
    assert!(
        own_space
            .check(&serde_json::to_value(dues_only).unwrap())
            .is_ok()
    );
    assert!(
        own_space
            .check(&serde_json::to_value(proposed).unwrap())
            .is_err()
    );
    let recorded = respond(
        &first,
        Side::Axis,
        serde_json::to_value(RefitPlan::default()).unwrap(),
    );
    let mut expected_air = first.state.air.clone();
    expected_air.runtime.maintenance.closed.insert(Side::Axis);
    expected_air
        .runtime
        .maintenance
        .waiting
        .insert(Side::Axis, RefitPlan::default());
    assert_eq!(
        serde_json::to_value(&recorded.state.air).unwrap(),
        serde_json::to_value(&expected_air).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&recorded.state.logistics).unwrap(),
        serde_json::to_value(&first.state.logistics).unwrap()
    );
    assert_eq!(recorded.rng, first.rng);
}

/// Cases: scen:59.36, airlog:38.33
#[test]
fn first_period_manual_refit_refusal_is_cited_and_answer_and_finish_fail_atomically() {
    let game = opened_first_period();
    let proposed = plan(&game.state, true);
    let action = serde_json::to_value(&proposed).unwrap();
    let current = period(&game.state).unwrap();
    let expected = illegal("scen:59.36: aircraft maintenance barred in first scenario OpStage");
    let rejection = match validate(content(), &game.state, Side::Axis, current, &proposed) {
        Err(rejection) => rejection,
        Ok(_) => panic!("first-period refit was accepted"),
    };
    assert_eq!(rejection, expected);
    assert!(matches!(
        rejected_response(content(), &game, action.clone()),
        Rejection::Illegal { .. }
    ));
    let pending = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap();
    let mut refused = game.clone();
    let before = serde_json::to_vec(&refused).unwrap();
    assert!(matches!(
        answer(content(), &mut refused.state, pending, &action),
        Err(Rejection::Illegal { .. })
    ));
    assert_eq!(serde_json::to_vec(&refused).unwrap(), before);

    // A forged accepted plan bypasses Respond, so the existing adjudication
    // contract classifies its ordinary prohibition as a contextual invariant.
    let mut forged = game.clone();
    forged
        .state
        .air
        .runtime
        .maintenance
        .closed
        .extend(Side::ALL);
    forged
        .state
        .air
        .runtime
        .maintenance
        .waiting
        .insert(Side::Axis, proposed);
    forged
        .state
        .air
        .runtime
        .maintenance
        .waiting
        .insert(Side::Commonwealth, RefitPlan::default());
    let before = serde_json::to_vec(&forged).unwrap();
    let mut rng = CampaignRng::from_state(&forged.rng);
    let before_rng = rng.state();
    let mut events = Vec::new();
    assert!(matches!(finish(content(), &mut forged.state,
        &mut Cx { rng: &mut rng, events: &mut events }),
        Err(EngineError::Invariant { detail }) if detail.contains("scen:59.36")
            && detail.contains("accepted finite refit plan became invalid")));
    assert_eq!(serde_json::to_vec(&forged).unwrap(), before);
    assert_eq!(rng.state(), before_rng);
    assert!(events.is_empty());
}

/// Cases: scen:59.36, land:3.6, airlog:35.14, airlog:38.33
#[test]
fn public_first_period_bound_preserves_hidden_enemy_pairs_and_pass_acceptance() {
    for a in [opened_first_period(), opened(false)] {
        let mut b = a.clone();
        b.state
            .logistics
            .air_dumps
            .get_mut("refit.stock")
            .unwrap()
            .supplies
            .stores = 7;
        inventory::update(content(), &mut b.state.air, |runtime| {
            for p in runtime.aircraft.values_mut().filter(|p| p.force == "axis") {
                p.refitted = !p.refitted;
            }
            Ok(())
        })
        .unwrap();
        let current = period(&a.state).unwrap();
        assert_eq!(
            space(content(), &a.state, Side::Commonwealth, current),
            space(content(), &b.state, Side::Commonwealth, current)
        );
        assert_eq!(
            serde_json::to_value(&a.state.decisions.pending).unwrap(),
            serde_json::to_value(&b.state.decisions.pending).unwrap()
        );
        assert_indistinguishable(
            &Cna::dev(),
            content(),
            &a.state,
            &b.state,
            Side::Commonwealth,
        );
        let pass = command(&a, Side::Commonwealth, Value::Null);
        assert_action_indistinguishable(&Cna::dev(), content(), &a, &b, &pass, Side::Commonwealth);
    }
}

/// Cases: airlog:35.14, airlog:36.12, airlog:36.17, airlog:38.35, land:3.6
#[test]
fn respond_preserves_exact_canonical_and_source_errors_without_side_effects() {
    let game = opened(false);
    let proposed = plan(&game.state, true);
    let id = FacilityId("airfield_benina".into());
    let mut malformed = game.clone();
    // Corrupt a trusted own record, not an answer-supplied capacity. Test both
    // direct facility properties and the dues-only AirSupplyError::Canonical.
    malformed
        .state
        .air
        .runtime
        .facilities
        .get_mut(&id)
        .unwrap()
        .current_capacity = FacilityCapacity::Levels(i32::MAX);
    let expected = malformed.state.air.runtime.facilities[&id]
        .properties(content(), &id)
        .unwrap_err();
    assert!(matches!(&expected, EngineError::Invariant { detail }
        if detail == "air facility: capacity disagrees with source ceiling"));
    for dues_only in [false, true] {
        let mut candidate = proposed.clone();
        if dues_only {
            candidate.cohorts.clear();
            candidate.draws[0].draws[0].amount.stores = StoresPoints::new(1);
        }
        assert_eq!(
            rejected_response(
                content(),
                &malformed,
                serde_json::to_value(candidate).unwrap()
            ),
            Rejection::Engine(expected.clone())
        );
    }
    let mut source = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let aircraft = &game.state.air.runtime.aircraft[&proposed.cohorts[0].priority[0]].aircraft;
    source.units.aircraft.remove(aircraft).unwrap();
    assert_eq!(
        rejected_response(&source, &game, serde_json::to_value(proposed).unwrap()),
        Rejection::Engine(unsupported("airlog:38.35", "refit aircraft source missing"))
    );
}

/// Cases: airlog:35.14, airlog:36.17, airlog:38.33, airlog:38.36, land:3.6
#[test]
fn respond_ordinary_stock_foreign_missing_dues_and_duplicate_choices_are_illegal() {
    let game = opened(false);
    for refusal in 0..5 {
        let mut own = game.clone();
        let mut candidate = plan(&own.state, true);
        match refusal {
            0 => {
                own.state
                    .logistics
                    .air_dumps
                    .get_mut("refit.stock")
                    .unwrap()
                    .supplies
                    .stores = 0
            }
            1 => {
                own.state
                    .logistics
                    .air_dumps
                    .get_mut("refit.stock")
                    .unwrap()
                    .side = Side::Commonwealth
            }
            2 => candidate.cohorts[0].priority[0] = PlaneId("missing.own.choice".into()),
            3 => candidate.dues.clear(),
            _ => candidate.cohorts[0].priority[1] = candidate.cohorts[0].priority[0].clone(),
        }
        assert!(matches!(
            rejected_response(content(), &own, serde_json::to_value(candidate).unwrap()),
            Rejection::Illegal { .. }
        ));
    }
}

/// Cases: airlog:35.14, airlog:36.12, airlog:38.35, airlog:38.36, land:3.6
#[test]
fn finish_retains_exact_engine_failures_and_promotes_late_ordinary_refusal_to_invariant() {
    let game = opened(false);
    let proposed = plan(&game.state, true);
    let mut accepted = respond(&game, Side::Axis, serde_json::to_value(&proposed).unwrap());
    accepted = respond(&accepted, Side::Commonwealth, Value::Null);
    let mut source = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let aircraft = &game.state.air.runtime.aircraft[&proposed.cohorts[0].priority[0]].aircraft;
    source.units.aircraft.remove(aircraft).unwrap();
    for failure in 0..3 {
        let mut failed = accepted.clone();
        let id = FacilityId("airfield_benina".into());
        let expected = match failure {
            0 => {
                failed
                    .state
                    .air
                    .runtime
                    .facilities
                    .get_mut(&id)
                    .unwrap()
                    .current_capacity = FacilityCapacity::Levels(i32::MAX);
                failed.state.air.runtime.facilities[&id]
                    .properties(content(), &id)
                    .unwrap_err()
            }
            1 => unsupported("airlog:38.35", "refit aircraft source missing"),
            _ => {
                failed
                    .state
                    .logistics
                    .air_dumps
                    .get_mut("refit.stock")
                    .unwrap()
                    .supplies
                    .stores = 0;
                invalid(
                    "accepted finite refit plan became invalid: invalid or insufficient own finite refit stock allocation",
                )
            }
        };
        let before = serde_json::to_vec(&failed).unwrap();
        let mut rng = CampaignRng::from_state(&failed.rng);
        let before_rng = rng.state();
        let mut events = Vec::new();
        let actual = finish(
            if failure == 1 { &source } else { content() },
            &mut failed.state,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        );
        assert_eq!(actual, Err(expected));
        assert_eq!(serde_json::to_vec(&failed).unwrap(), before);
        assert_eq!(rng.state(), before_rng);
        assert!(events.is_empty());
    }
}

/// Cases: airlog:35.14, airlog:38.33, airlog:38.34, airlog:38.36, land:3.6
#[test]
fn respond_only_buffers_then_actual_finish_debits_rolls_and_checkpoints_once() {
    let game = opened(false);
    let aircraft = game.state.air.runtime.aircraft.clone();
    let stock = game.state.logistics.air_dumps.clone();
    let rng = game.rng.clone();
    let action = serde_json::to_value(plan(&game.state, true)).unwrap();
    let one = respond(&game, Side::Axis, action);
    assert_eq!(one.state.air.runtime.aircraft, aircraft);
    assert_eq!(one.state.logistics.air_dumps, stock);
    assert_eq!(one.rng, rng);
    assert!(one.state.air.runtime.maintenance.receipts.is_empty());
    assert!(
        one.state.air.runtime.sgsus[&SgsuId("axis.refit".into())]
            .stores_paid_game_turn
            .is_none()
    );
    let restored: Game<Cna> = serde_json::from_slice(&serde_json::to_vec(&one).unwrap()).unwrap();
    let mut a = respond(&one, Side::Commonwealth, Value::Null);
    let mut b = respond(&restored, Side::Commonwealth, Value::Null);
    finish_game(&mut a);
    finish_game(&mut b);
    assert_eq!(
        serde_json::to_vec(&a).unwrap(),
        serde_json::to_vec(&b).unwrap()
    );
    assert_ne!(a.rng, rng);
    assert_eq!(
        a.state.logistics.air_dumps["refit.stock"].supplies,
        Supplies {
            fuel: 3,
            stores: 6,
            water: 3,
            ammo: 0
        }
    );
    let paid = SgsuOpStage {
        game_turn: a.state.cursor.game_turn,
        op_stage: 2,
    };
    assert!(a.state.air.runtime.sgsus[&SgsuId("axis.refit".into())].operation_paid(paid));
    for receipt in &a.state.air.runtime.maintenance.receipts[&Side::Axis] {
        for id in &receipt.cohort.priority {
            assert_eq!(
                a.state.air.runtime.aircraft[id].refitted,
                receipt.successful.contains(id)
            );
            assert!(!a.state.air.runtime.aircraft[id].fuelled);
            assert!(!a.state.air.runtime.aircraft[id].armed);
        }
    }
    let before = serde_json::to_vec(&a).unwrap();
    finish_game(&mut a);
    assert_eq!(serde_json::to_vec(&a).unwrap(), before);
    inventory::check(content(), &a.state.air).unwrap();
}

/// Cases: airlog:35.14, airlog:38.36, airlog:38.39
/// Interpretations: interp:airlog-0022
#[test]
fn wire_partition_cannot_add_squadron_rolls_or_stores_and_per_plane_uses_bound_rolls() {
    let game = opened(false);
    let whole = plan(&game.state, true);
    let mut split = whole.clone();
    let second = split.cohorts[0].priority.pop().unwrap();
    let mut another = split.cohorts[0].clone();
    another.priority = vec![second];
    split.cohorts.push(another);
    let v = validate(
        content(),
        &game.state,
        Side::Axis,
        period(&game.state).unwrap(),
        &split,
    )
    .unwrap();
    assert_eq!(v.groups.len(), 1);
    assert_eq!(v.groups[0].0.priority.len(), 2);
    assert_eq!(v.demands[&SgsuId("axis.refit".into())].stores.get(), 2);
    let mut a = respond(&game, Side::Axis, serde_json::to_value(whole).unwrap());
    let mut b = respond(&game, Side::Axis, serde_json::to_value(split).unwrap());
    a = respond(&a, Side::Commonwealth, Value::Null);
    b = respond(&b, Side::Commonwealth, Value::Null);
    finish_game(&mut a);
    finish_game(&mut b);
    assert_eq!(
        serde_json::to_vec(&a).unwrap(),
        serde_json::to_vec(&b).unwrap()
    );
    let mut optional = plan(&game.state, true);
    optional.cohorts[0].method = RefitMethod::PerPlane;
    let ids = optional.cohorts[0].priority.clone();
    let nation = nationality(
        &content().units.aircraft[&game.state.air.runtime.aircraft[&ids[0]].aircraft].nation,
    )
    .unwrap();
    let mut expected_rng = CampaignRng::from_state(&game.rng);
    let successes: BTreeSet<_> = ids
        .iter()
        .filter(|_| {
            content().tables.airlog.aircraft_refit.plane_refitted(
                nation,
                i32::from(expected_rng.two_dice_reading().sum()),
                true,
            )
        })
        .cloned()
        .collect();
    let mut c = respond(&game, Side::Axis, serde_json::to_value(optional).unwrap());
    c = respond(&c, Side::Commonwealth, Value::Null);
    finish_game(&mut c);
    assert_eq!(
        c.state.air.runtime.maintenance.receipts[&Side::Axis][0].successful,
        successes
    );
    assert_eq!(c.rng, expected_rng.state());
}

/// Cases: airlog:35.14, airlog:38.36, land:24.79, scen:59.36
#[test]
fn late_stock_or_capacity_failure_preserves_whole_state_rng_and_events() {
    let game = opened(false);
    let mut accepted = respond(
        &game,
        Side::Axis,
        serde_json::to_value(plan(&game.state, true)).unwrap(),
    );
    accepted = respond(&accepted, Side::Commonwealth, Value::Null);
    for failure in 0..3 {
        let mut failed = accepted.clone();
        if failure == 1 {
            inventory::update(content(), &mut failed.state.air, |runtime| {
                runtime
                    .facilities
                    .get_mut(&FacilityId("airfield_benina".into()))
                    .unwrap()
                    .project_unavailable = true;
                Ok(())
            })
            .unwrap();
        } else if failure == 0 {
            failed
                .state
                .logistics
                .air_dumps
                .get_mut("refit.stock")
                .unwrap()
                .supplies
                .stores = 0;
        } else {
            // Deliberately corrupt the trusted aggregate, after accepted answers.
            // Validation/stock succeeds; final inventory check fails after rolls.
            failed
                .state
                .air
                .forces
                .get_mut("axis")
                .unwrap()
                .planes
                .values_mut()
                .next()
                .unwrap()
                .total += 1;
        }
        let before = serde_json::to_vec(&failed).unwrap();
        let mut rng = CampaignRng::from_state(&failed.rng);
        let before_rng = rng.state();
        let mut events = Vec::new();
        assert!(
            finish(
                content(),
                &mut failed.state,
                &mut Cx {
                    rng: &mut rng,
                    events: &mut events
                }
            )
            .is_err()
        );
        assert_eq!(serde_json::to_vec(&failed).unwrap(), before);
        assert_eq!(rng.state(), before_rng);
        assert!(events.is_empty());
    }
    let mut first = game.clone();
    first.state.cursor.op_stage = Some(1);
    let proposed = plan(&first.state, true);
    let before_first = serde_json::to_vec(&first).unwrap();
    assert!(matches!(validate(content(), &first.state, Side::Axis,
        period(&first.state).unwrap(), &proposed),
        Err(Rejection::Illegal { message }) if message.contains("scen:59.36")));
    assert_eq!(serde_json::to_vec(&first).unwrap(), before_first);
}

/// Cases: airlog:35.14, airlog:38.32
/// Interpretations: interp:airlog-0025
#[test]
fn strategic_requires_stage_three_and_cannot_use_stage_two_or_unknown_legacy_dues() {
    let mut game = opened(true);
    let game_turn = game.state.cursor.game_turn;
    for stage in [None, Some(2), Some(3)] {
        inventory::update(content(), &mut game.state.air, |runtime| {
            let s = runtime.sgsus.get_mut(&SgsuId("axis.refit".into())).unwrap();
            s.stores_paid_game_turn = Some(game_turn);
            s.fuel_water_paid = stage.map(|op_stage| SgsuOpStage {
                game_turn,
                op_stage,
            });
            Ok(())
        })
        .unwrap();
        let v = validate(
            content(),
            &game.state,
            Side::Axis,
            period(&game.state).unwrap(),
            &plan(&game.state, false),
        );
        assert_eq!(v.is_ok(), stage == Some(3));
    }
    let old = game.state.air.runtime.maintenance.clone();
    let mut without = serde_json::to_value(&game).unwrap();
    without["state"]["air"]["runtime"]
        .as_object_mut()
        .unwrap()
        .remove("maintenance");
    let migrated: Game<Cna> = serde_json::from_value(without).unwrap();
    assert_eq!(
        migrated.state.air.runtime.maintenance,
        refit::MaintenanceState::default()
    );
    assert_eq!(game.state.air.runtime.maintenance, old);
}

/// Cases: airlog:38.33, airlog:38.36
/// Interpretations: interp:airlog-0022
#[test]
fn actual_advance_dispatches_finish_and_period_history_cannot_be_reset_or_reopened() {
    let game = opened(false);
    let mut ready = respond(
        &game,
        Side::Axis,
        serde_json::to_value(plan(&game.state, true)).unwrap(),
    );
    ready = respond(&ready, Side::Commonwealth, Value::Null);
    let advanced = evaluate(&Cna::dev(), content(), &ready, &Command::Advance)
        .unwrap()
        .game;
    assert!(advanced.state.air.runtime.maintenance.finished);
    assert_eq!(
        advanced.state.logistics.air_dumps["refit.stock"]
            .supplies
            .stores,
        6
    );
    let mut finished = ready.clone();
    finish_game(&mut finished);
    for reset_history in [false, true] {
        let mut s = finished.state.clone();
        let before = serde_json::to_vec(&s).unwrap();
        assert!(matches!(
            inventory::update(content(), &mut s.air, |runtime| {
                if reset_history {
                    runtime.maintenance.receipts.clear();
                } else {
                    runtime.maintenance.finished = false;
                }
                Ok(())
            }),
            Err(EngineError::Invariant { .. })
        ));
        assert_eq!(serde_json::to_vec(&s).unwrap(), before);
    }
    let prior = finished.state.air.runtime.maintenance.receipts.clone();
    finished.state.cursor.op_stage = Some(3);
    let mut rng = CampaignRng::from_state(&finished.rng);
    let mut events = Vec::new();
    enter(
        content(),
        &mut finished.state,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
        false,
    )
    .unwrap();
    assert_eq!(finished.state.air.runtime.maintenance.receipts, prior);
    assert!(!finished.state.air.runtime.maintenance.finished);
    assert_eq!(finished.state.decisions.pending.len(), 2);
}

/// Cases: land:3.6, airlog:35.14, airlog:38.33
#[test]
fn hidden_enemy_stock_answers_and_refit_flags_do_not_change_own_windows_or_acceptance() {
    let a = opened(false);
    let mut b = a.clone();
    b.state
        .logistics
        .air_dumps
        .get_mut("refit.stock")
        .unwrap()
        .supplies
        .stores = 7;
    inventory::update(content(), &mut b.state.air, |runtime| {
        for p in runtime.aircraft.values_mut().filter(|p| p.force == "axis") {
            p.refitted = !p.refitted;
        }
        Ok(())
    })
    .unwrap();
    assert_indistinguishable(
        &Cna::dev(),
        content(),
        &a.state,
        &b.state,
        Side::Commonwealth,
    );
    let cmd = command(&a, Side::Commonwealth, Value::Null);
    assert_action_indistinguishable(&Cna::dev(), content(), &a, &b, &cmd, Side::Commonwealth);
    let refit = command(
        &a,
        Side::Axis,
        serde_json::to_value(plan(&a.state, true)).unwrap(),
    );
    let pass = command(&a, Side::Axis, Value::Null);
    assert_actions_indistinguishable(
        &Cna::dev(),
        content(),
        (&a, &refit),
        (&a, &pass),
        Side::Commonwealth,
    );
    for state in [&a.state, &b.state] {
        let mut draft = state.clone();
        let before = serde_json::to_vec(&draft).unwrap();
        let mut rng = CampaignRng::from_state(&a.rng);
        let rng_before = rng.state();
        let mut events = Vec::new();
        assert!(
            matches!(enter(content(), &mut draft, &mut Cx { rng: &mut rng, events: &mut events }, true),
            Err(EngineError::Unsupported { case, .. }) if case == "airlog:38.1")
        );
        assert_eq!(serde_json::to_vec(&draft).unwrap(), before);
        assert_eq!(rng.state(), rng_before);
        assert!(events.is_empty());
    }
}

/// Cases: airlog:35.17, airlog:36.14, airlog:36.15, airlog:36.5, airlog:38.35, airlog:38.36
#[test]
fn actual_foreign_servicer_uses_aircraft_nationality_and_damaged_active_capacity() {
    let mut game = opened(false);
    inventory::update(content(), &mut game.state.air, |runtime| {
        let site = runtime
            .facilities
            .get_mut(&FacilityId("airfield_benina".into()))
            .unwrap();
        site.owner = Side::Commonwealth;
        site.current_capacity = FacilityCapacity::Levels(1);
        Ok(())
    })
    .unwrap();
    let mut foreign = plan(&game.state, true);
    foreign.dues[0] = SgsuId("axis.foreign".into());
    foreign.cohorts[0].servicer = foreign.dues[0].clone();
    foreign.draws[0].servicer = foreign.dues[0].clone();
    let current = period(&game.state).unwrap();
    assert!(validate(content(), &game.state, Side::Axis, current, &foreign).is_ok());
    let ids = foreign.cohorts[0].priority.clone();
    let nation = nationality(
        &content().units.aircraft[&game.state.air.runtime.aircraft[&ids[0]].aircraft].nation,
    )
    .unwrap();
    let mut expected_rng = CampaignRng::from_state(&game.rng);
    let count = super::super::squadron_refitted(
        &content().tables.airlog.aircraft_refit,
        nation,
        false,
        2,
        expected_rng.d6(),
    )
    .unwrap();
    let mut received = respond(&game, Side::Axis, serde_json::to_value(foreign).unwrap());
    received = respond(&received, Side::Commonwealth, Value::Null);
    finish_game(&mut received);
    assert_eq!(received.rng, expected_rng.state());
    assert_eq!(
        received.state.air.runtime.maintenance.receipts[&Side::Axis][0].successful,
        refit::successful_ids(&ids, count).unwrap()
    );
    // Two parked SGSUs did not consume two active slots; only one serviced.
    assert_eq!(received.state.air.runtime.sgsus.len(), 2);
    let mut split = plan(&game.state, true);
    let second = split.cohorts[0].priority.pop().unwrap();
    let mut other = split.cohorts[0].clone();
    other.servicer = SgsuId("axis.foreign".into());
    other.priority = vec![second];
    split.cohorts.push(other);
    split.dues.push(SgsuId("axis.foreign".into()));
    assert!(validate(content(), &game.state, Side::Axis, current, &split).is_err());
    inventory::update(content(), &mut game.state.air, |runtime| {
        runtime
            .facilities
            .get_mut(&FacilityId("airfield_benina".into()))
            .unwrap()
            .current_capacity = FacilityCapacity::Levels(6);
        Ok(())
    })
    .unwrap();
    // Both active slots now fit. Missing foreign SGSU dues are independently
    // Illegal before the unresolved cross-servicer squadron Stores procedure.
    let before_split = serde_json::to_vec(&game).unwrap();
    assert!(matches!(
        validate(content(), &game.state, Side::Axis, current, &split),
        Err(Rejection::Illegal { message })
            if message == "invalid or insufficient own finite refit stock allocation"
    ));
    assert_eq!(serde_json::to_vec(&game).unwrap(), before_split);
    // Preserve the original draw and add the second SGSU's actual dues only.
    // This tests the missing split procedure without selecting its allocation.
    let mut foreign_dues = split.draws[0].clone();
    foreign_dues.servicer = SgsuId("axis.foreign".into());
    foreign_dues.draws[0].amount.stores = StoresPoints::new(1);
    split.draws.push(foreign_dues);
    assert!(
        matches!(validate(content(), &game.state, Side::Axis, current, &split),
        Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "airlog:38.36")
    );
    assert_eq!(serde_json::to_vec(&game).unwrap(), before_split);
    let offmap = game
        .state
        .air
        .runtime
        .facilities
        .iter()
        .find_map(|(id, s)| {
            s.properties(content(), id)
                .ok()
                .filter(|p| p.theatre == FacilityTheatre::CommonwealthOffMap)
                .map(|_| id.clone())
        })
        .unwrap();
    inventory::update(content(), &mut game.state.air, |runtime| {
        runtime
            .sgsus
            .get_mut(&SgsuId("axis.refit".into()))
            .unwrap()
            .position = SgsuPosition::Facility(offmap.clone());
        for p in runtime
            .aircraft
            .values_mut()
            .filter(|p| p.squadron.as_deref() == Some("axis.refit"))
        {
            p.facility = Some(offmap.0.clone());
        }
        Ok(())
    })
    .unwrap();
    let before = serde_json::to_vec(&game).unwrap();
    assert!(matches!(
        validate(
            content(),
            &game.state,
            Side::Axis,
            current,
            &plan(&game.state, true)
        ),
        Err(Rejection::Illegal { .. })
    ));
    assert_eq!(serde_json::to_vec(&game).unwrap(), before);
}

fn cohort_servicers(space: &ActionSpace) -> Vec<String> {
    let ActionSchema::Record { fields } = &space.schema else {
        panic!("refit record required")
    };
    let ActionSchema::List { item, .. } =
        &fields.iter().find(|f| f.name == "cohorts").unwrap().schema
    else {
        panic!("cohort list required")
    };
    let ActionSchema::Record { fields } = item.as_ref() else {
        panic!("cohort record required")
    };
    let ActionSchema::Choice { options } =
        &fields.iter().find(|f| f.name == "servicer").unwrap().schema
    else {
        panic!("servicer choice required")
    };
    options.iter().map(|o| o.id.clone()).collect()
}

/// Cases: airlog:36.5, airlog:36.14, airlog:36.17, airlog:43.21, airlog:44.14, airlog:44.16, land:3.6
#[test]
fn refit_domain_zero_and_verified_offmap_inclusion_preserve_other_domains_and_recovery() {
    let game = opened(false);
    let current = period(&game.state).unwrap();
    let original = space(content(), &game.state, Side::Axis, current).unwrap();
    assert_eq!(
        cohort_servicers(&original),
        vec!["axis.foreign", "axis.refit"]
    );
    let planes = game
        .state
        .air
        .runtime
        .aircraft
        .values()
        .filter(|p| owned(&p.force, Side::Axis))
        .count();
    assert!(planes > 0);
    assert_eq!(cohort_maximum(&original), u32::try_from(planes).unwrap());
    for case in [
        "airlog:36.5",
        "airlog:43.21",
        "airlog:44.14",
        "airlog:44.16",
    ] {
        assert!(
            original.context.as_ref().unwrap()["development_limitation"]
                .as_str()
                .unwrap()
                .contains(case)
        );
    }
    let offmap = game
        .state
        .air
        .runtime
        .facilities
        .iter()
        .find_map(|(id, s)| {
            (id.0 == "airfield_deversoir"
                && s.properties(content(), id).unwrap().theatre
                    == FacilityTheatre::CommonwealthOffMap)
                .then(|| id.clone())
        })
        .unwrap();
    for exempt in [false, true] {
        let mut changed = Game::<Cna> {
            state: base(false),
            rng: game.rng.clone(),
        };
        inventory::update(content(), &mut changed.state.air, |runtime| {
            if exempt {
                for s in runtime
                    .sgsus
                    .values_mut()
                    .filter(|s| owned(&s.force, Side::Axis))
                {
                    s.position = SgsuPosition::Facility(offmap.clone());
                }
                for p in runtime
                    .aircraft
                    .values_mut()
                    .filter(|p| p.squadron.as_deref() == Some("axis.refit"))
                {
                    p.facility = Some(offmap.0.clone());
                }
            } else {
                runtime
                    .facilities
                    .get_mut(&FacilityId("airfield_benina".into()))
                    .unwrap()
                    .project_unavailable = true;
            }
            Ok(())
        })
        .unwrap();
        let mut rng = CampaignRng::from_state(&changed.rng);
        let mut events = Vec::new();
        enter(
            content(),
            &mut changed.state,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
            false,
        )
        .unwrap();
        assert_eq!(rng.state(), changed.rng);
        let domain = space(content(), &changed.state, Side::Axis, current).unwrap();
        assert_eq!(
            cohort_maximum(&domain),
            if exempt {
                u32::try_from(planes).unwrap()
            } else {
                0
            }
        );
        assert_eq!(
            cohort_servicers(&domain),
            if exempt {
                vec!["axis.foreign", "axis.refit"]
            } else {
                Vec::new()
            }
        );
        assert!(domain.check(&Value::Null).is_ok());
        assert!(
            domain
                .check(&serde_json::to_value(RefitPlan::default()).unwrap())
                .is_ok()
        );
        assert_eq!(
            domain
                .check(&serde_json::to_value(plan(&changed.state, true)).unwrap())
                .is_ok(),
            exempt
        );
        let ActionSchema::Record { fields: old } = &original.schema else {
            unreachable!()
        };
        let ActionSchema::Record { fields: new } = &domain.schema else {
            unreachable!()
        };
        for name in ["dues", "draws"] {
            assert_eq!(
                old.iter().find(|f| f.name == name),
                new.iter().find(|f| f.name == name)
            );
        }
        let inner = |fields: &[FieldSchema]| {
            let ActionSchema::List { item, .. } =
                &fields.iter().find(|f| f.name == "cohorts").unwrap().schema
            else {
                unreachable!()
            };
            let ActionSchema::Record { fields } = item.as_ref() else {
                unreachable!()
            };
            fields
                .iter()
                .filter(|f| f.name != "servicer")
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(inner(old), inner(new));
        assert_eq!(domain.pass, original.pass);
        assert_eq!(
            domain.context.as_ref().unwrap()["development_limitation"],
            original.context.as_ref().unwrap()["development_limitation"]
        );
        let restored: Game<Cna> =
            serde_json::from_slice(&serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_eq!(
            space(content(), &restored.state, Side::Axis, current).unwrap(),
            domain
        );
        let before = serde_json::to_vec(&changed).unwrap();
        let pending = changed
            .state
            .decisions
            .pending
            .iter()
            .find(|p| p.seat.side == Side::Axis)
            .unwrap()
            .clone();
        assert_eq!(pending.space, domain);
        let proposed = serde_json::to_value(plan(&changed.state, true)).unwrap();
        assert!(matches!(
            rejected_response(content(), &changed, proposed.clone()),
            Rejection::Illegal { .. }
        ));
        assert!(matches!(
            answer(content(), &mut changed.state, &pending, &proposed),
            Err(Rejection::Illegal { .. })
        ));
        assert_eq!(serde_json::to_vec(&changed).unwrap(), before);
        let proposed = plan(&changed.state, true);
        let refusal = match validate(content(), &changed.state, Side::Axis, current, &proposed) {
            Err(error) => error,
            Ok(_) => panic!("excluded consumer refit was accepted"),
        };
        if exempt {
            assert_eq!(
                refusal,
                illegal("invalid or insufficient own finite refit stock allocation")
            );
        } else {
            assert_eq!(refusal, illegal("own refit facility is unavailable"));
        }
        assert_eq!(serde_json::to_vec(&changed).unwrap(), before);
        let aircraft = changed.state.air.runtime.aircraft.clone();
        let stock = serde_json::to_value(&changed.state.logistics).unwrap();
        let one = respond(&changed, Side::Axis, Value::Null);
        let restored: Game<Cna> =
            serde_json::from_slice(&serde_json::to_vec(&one).unwrap()).unwrap();
        let mut a = respond(&one, Side::Commonwealth, Value::Null);
        let mut b = respond(&restored, Side::Commonwealth, Value::Null);
        finish_game(&mut a);
        finish_game(&mut b);
        assert_eq!(
            serde_json::to_vec(&a).unwrap(),
            serde_json::to_vec(&b).unwrap()
        );
        assert_eq!(a.rng, changed.rng);
        assert_eq!(a.state.air.runtime.aircraft, aircraft);
        assert_eq!(serde_json::to_value(&a.state.logistics).unwrap(), stock);
        let resolved = serde_json::to_vec(&a).unwrap();
        finish_game(&mut a);
        assert_eq!(serde_json::to_vec(&a).unwrap(), resolved);
    }
}

/// Cases: airlog:36.12, airlog:36.17, land:3.6
#[test]
fn second_owner_domain_error_and_actual_entry_fail_without_partial_publication() {
    let mut state = base(false);
    let site = state
        .air
        .runtime
        .facilities
        .iter()
        .find_map(|(id, s)| {
            let props = s.properties(content(), id).unwrap();
            (s.owner == Side::Commonwealth
                && props.theatre == FacilityTheatre::Africa
                && matches!(props.location, Location::Hex { .. }))
            .then(|| id.clone())
        })
        .unwrap();
    let mut squadron = state.air.squadrons["axis.foreign"].clone();
    squadron.id = "cw.domain".into();
    squadron.force = "commonwealth".into();
    squadron.side = Side::Commonwealth;
    squadron.nationality = "cw".into();
    squadron.facility = site.0.clone();
    state.air.squadrons.insert(squadron.id.clone(), squadron);
    // Restore an already consistent empty test roster and its canonical SGSU
    // together; generic update checks its input before invoking a callback.
    let mut s = state.air.runtime.sgsus[&SgsuId("axis.foreign".into())].clone();
    s.force = "commonwealth".into();
    s.nationality = "cw".into();
    s.position = SgsuPosition::Facility(site.clone());
    state
        .air
        .runtime
        .sgsus
        .insert(SgsuId("cw.domain".into()), s);
    inventory::check(content(), &state.air).unwrap();
    state = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    inventory::check(content(), &state.air).unwrap();
    // Corrupt only the second owner's trusted property. The pure batch builder
    // must return the exact error without yielding the already valid Axis menu.
    state
        .air
        .runtime
        .facilities
        .get_mut(&site)
        .unwrap()
        .current_capacity = FacilityCapacity::Levels(i32::MAX);
    let current = period(&state).unwrap();
    let expected = state.air.runtime.facilities[&site]
        .properties(content(), &site)
        .unwrap_err();
    let before = serde_json::to_vec(&state).unwrap();
    assert!(space(content(), &state, Side::Axis, current).is_ok());
    assert_eq!(spaces(content(), &state, current), Err(expected.clone()));
    let mut rng = CampaignRng::from_seed([41; 32]);
    let before_rng = rng.state();
    let mut events = Vec::new();
    // Actual enter may detect the same canonical corruption during inventory
    // validation earlier; either path must publish no window, period or Note.
    assert_eq!(
        enter(
            content(),
            &mut state,
            &mut Cx {
                rng: &mut rng,
                events: &mut events
            },
            false
        ),
        Err(expected)
    );
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    assert_eq!(rng.state(), before_rng);
    assert!(events.is_empty());
}

/// Cases: airlog:35.14, airlog:36.17, airlog:38.32, airlog:38.33, airlog:38.36, airlog:38.39, land:3.6
#[test]
fn advertised_illegal_choices_precede_missing_designation_method_and_stores_shapes() {
    let game = opened(false);
    let current = period(&game.state).unwrap();
    for refusal in 0..4 {
        let mut own = game.clone();
        own.state
            .air
            .runtime
            .designation
            .assignments
            .remove("axis.refit");
        let mut proposed = plan(&own.state, true);
        match refusal {
            0 => proposed.cohorts[0].priority[1] = proposed.cohorts[0].priority[0].clone(),
            1 => proposed.draws[0].draws[0].amount.stores = StoresPoints::new(1),
            2 => {
                let mut other = proposed.cohorts[0].clone();
                other.servicer = SgsuId("axis.foreign".into());
                proposed.cohorts.push(other);
                proposed.dues.push(SgsuId("axis.foreign".into()));
            }
            _ => {
                let mut other = proposed.cohorts[0].clone();
                other.method = RefitMethod::PerPlane;
                proposed.cohorts.push(other);
            }
        }
        let action = serde_json::to_value(&proposed).unwrap();
        assert!(
            space(content(), &own.state, Side::Axis, current)
                .unwrap()
                .check(&action)
                .is_ok()
        );
        assert!(matches!(
            rejected_response(content(), &own, action),
            Rejection::Illegal { .. }
        ));
        let before = serde_json::to_vec(&own).unwrap();
        assert!(matches!(
            validate(content(), &own.state, Side::Axis, current, &proposed),
            Err(Rejection::Illegal { .. })
        ));
        assert_eq!(serde_json::to_vec(&own).unwrap(), before);
    }
    let mut missing = game.clone();
    missing
        .state
        .air
        .runtime
        .designation
        .assignments
        .remove("axis.refit");
    let proposed = plan(&missing.state, true);
    assert!(
        matches!(validate(content(), &missing.state, Side::Axis, current, &proposed),
        Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "airlog:38.32")
    );

    let mut ground = game.clone();
    let location = ground.state.air.runtime.facilities[&FacilityId("airfield_benina".into())]
        .properties(content(), &FacilityId("airfield_benina".into()))
        .unwrap()
        .location;
    // An assigned aircraft still needs its actual physical facility. Preserve
    // the rejected old setup as a negative canonical/atomicity control.
    let before_ground = serde_json::to_vec(&ground).unwrap();
    let mut invalid_ground_air = ground.state.air.clone();
    let before_invalid = serde_json::to_vec(&invalid_ground_air).unwrap();
    assert!(matches!(
        inventory::update(content(), &mut invalid_ground_air, |runtime| {
            runtime
                .sgsus
                .get_mut(&SgsuId("axis.refit".into()))
                .unwrap()
                .position = SgsuPosition::Ground(location.clone());
            for p in runtime
                .aircraft
                .values_mut()
                .filter(|p| p.squadron.as_deref() == Some("axis.refit"))
            {
                p.facility = None;
            }
            Ok(())
        }),
        Err(EngineError::Invariant { detail })
            if detail == "air inventory: unknown aircraft or missing physical facility"
    ));
    assert_eq!(
        serde_json::to_vec(&invalid_ground_air).unwrap(),
        before_invalid
    );
    assert_eq!(serde_json::to_vec(&ground).unwrap(), before_ground);
    let parked_aircraft = ground.state.air.runtime.aircraft.clone();
    // Only the SGSU has the verified ground position; parked planes retain
    // their original assignment and canonical Benina physical facility.
    inventory::update(content(), &mut ground.state.air, |runtime| {
        runtime
            .sgsus
            .get_mut(&SgsuId("axis.refit".into()))
            .unwrap()
            .position = SgsuPosition::Ground(location.clone());
        Ok(())
    })
    .unwrap();
    inventory::check(content(), &ground.state.air).unwrap();
    assert_eq!(ground.state.air.runtime.aircraft, parked_aircraft);
    let legal_ground_dues = RefitPlan {
        dues: vec![SgsuId("axis.refit".into())],
        ..Default::default()
    };
    let mut duplicate_dues = legal_ground_dues.clone();
    duplicate_dues.dues.push(duplicate_dues.dues[0].clone());
    assert!(
        space(content(), &ground.state, Side::Axis, current)
            .unwrap()
            .check(&serde_json::to_value(&duplicate_dues).unwrap())
            .is_ok()
    );
    assert!(matches!(
        rejected_response(
            content(),
            &ground,
            serde_json::to_value(duplicate_dues).unwrap()
        ),
        Rejection::Illegal { .. }
    ));
    let before = serde_json::to_vec(&ground).unwrap();
    assert!(
        matches!(validate(content(), &ground.state, Side::Axis, current, &legal_ground_dues),
        Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "airlog:35.14")
    );
    assert_eq!(serde_json::to_vec(&ground).unwrap(), before);

    let mut missing_site = game.clone();
    missing_site
        .state
        .air
        .runtime
        .sgsus
        .get_mut(&SgsuId("axis.refit".into()))
        .unwrap()
        .position = SgsuPosition::Facility(FacilityId("missing.canonical.site".into()));
    let expected = invalid("missing SGSU site");
    assert_eq!(
        space(content(), &missing_site.state, Side::Axis, current),
        Err(expected.clone())
    );
    assert_eq!(
        rejected_response(content(), &missing_site, Value::Null),
        Rejection::Engine(expected)
    );
}

/// Cases: airlog:36.14, airlog:36.17, land:3.6
#[test]
fn enemy_consumer_eligibility_does_not_change_own_domains_pass_streams_or_fixed_windows() {
    let a = opened(false);
    let mut b = a.clone();
    inventory::update(content(), &mut b.state.air, |runtime| {
        runtime
            .facilities
            .get_mut(&FacilityId("airfield_benina".into()))
            .unwrap()
            .project_unavailable = true;
        Ok(())
    })
    .unwrap();
    let current = period(&a.state).unwrap();
    assert_ne!(
        cohort_servicers(&space(content(), &a.state, Side::Axis, current).unwrap()),
        cohort_servicers(&space(content(), &b.state, Side::Axis, current).unwrap())
    );
    assert_eq!(
        space(content(), &a.state, Side::Commonwealth, current).unwrap(),
        space(content(), &b.state, Side::Commonwealth, current).unwrap()
    );
    assert_indistinguishable(
        &Cna::dev(),
        content(),
        &a.state,
        &b.state,
        Side::Commonwealth,
    );
    assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &a,
        &b,
        &command(&a, Side::Commonwealth, Value::Null),
        Side::Commonwealth,
    );
    for game in [&a, &b] {
        let mut fresh = game.clone();
        fresh.state.decisions.pending.clear();
        fresh.state.air.runtime.maintenance = refit::MaintenanceState::default();
        let mut rng = CampaignRng::from_state(&fresh.rng);
        let mut events = Vec::new();
        enter(
            content(),
            &mut fresh.state,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
            false,
        )
        .unwrap();
        assert_eq!(
            fresh
                .state
                .decisions
                .pending
                .iter()
                .map(|p| (p.seat, p.kind.as_str()))
                .collect::<Vec<_>>(),
            Side::ALL
                .into_iter()
                .map(|side| (SeatId::new(side, Role::Air), KIND))
                .collect::<Vec<_>>()
        );
        assert_eq!(rng.state(), game.rng);
    }
}

/// Cases: airlog:35.14, airlog:36.17, airlog:38.33, airlog:38.36, land:3.6
#[test]
fn unresolved_consumer_or_split_never_masks_known_finite_dues_and_squadron_costs() {
    let game = opened(false);
    let current = period(&game.state).unwrap();
    let mut split = plan(&game.state, true);
    let second_id = split.cohorts[0].priority.pop().unwrap();
    let mut second = split.cohorts[0].clone();
    second.servicer = SgsuId("axis.foreign".into());
    second.priority = vec![second_id];
    split.cohorts.push(second);
    split.dues.push(SgsuId("axis.foreign".into()));
    let mut second_draw = split.draws[0].clone();
    second_draw.servicer = SgsuId("axis.foreign".into());
    second_draw.draws[0].amount.stores = StoresPoints::new(1);
    split.draws.push(second_draw);
    assert_eq!(split.cohorts[0].priority.len(), 1);
    assert_eq!(split.cohorts[1].priority.len(), 1);
    assert_ne!(split.cohorts[0].priority[0], split.cohorts[1].priority[0]);

    let mut ground = game.clone();
    let location = ground.state.air.runtime.facilities[&FacilityId("airfield_benina".into())]
        .properties(content(), &FacilityId("airfield_benina".into()))
        .unwrap()
        .location;
    inventory::update(content(), &mut ground.state.air, |runtime| {
        runtime
            .sgsus
            .get_mut(&SgsuId("axis.foreign".into()))
            .unwrap()
            .position = SgsuPosition::Ground(location.clone());
        Ok(())
    })
    .unwrap();
    let mut mixed = plan(&ground.state, true);
    mixed.dues.insert(0, SgsuId("axis.foreign".into()));

    for (own, valid, case) in [
        (&game, split, "airlog:38.36"),
        (&ground, mixed, "airlog:35.14"),
    ] {
        for refusal in 0..4 {
            let mut candidate = valid.clone();
            if refusal == 1 {
                candidate.draws[0].draws[0].amount.fuel = FuelTenths::new(0);
            } else if refusal == 2 {
                // In the mixed plan this pays SGSU Stores but omits the fully
                // determinate single-servicer assigned-squadron Store.
                candidate.draws[0].draws[0].amount.stores =
                    StoresPoints::new(if case == "airlog:35.14" { 1 } else { 0 });
            } else if refusal == 3 {
                if case == "airlog:38.36" {
                    candidate.draws[1].draws[0].amount.stores = StoresPoints::new(0);
                } else {
                    candidate.draws.clear();
                }
            }
            let action = serde_json::to_value(&candidate).unwrap();
            assert!(
                space(content(), &own.state, Side::Axis, current)
                    .unwrap()
                    .check(&action)
                    .is_ok()
            );
            let expected = if refusal == 0 {
                match validate(content(), &own.state, Side::Axis, current, &candidate) {
                    Err(Rejection::Engine(error @ EngineError::Unsupported { .. })) => {
                        assert!(
                            matches!(&error, EngineError::Unsupported { case: found, .. } if found == case)
                        );
                        Rejection::Engine(error)
                    }
                    _ => panic!("otherwise-legal missing procedure did not retain Unsupported"),
                }
            } else {
                illegal("invalid or insufficient own finite refit stock allocation")
            };
            let before = serde_json::to_vec(own).unwrap();
            assert_eq!(rejected_response(content(), own, action.clone()), expected);
            let pending = own
                .state
                .decisions
                .pending
                .iter()
                .find(|p| p.seat.side == Side::Axis)
                .unwrap();
            let mut direct = own.clone();
            assert_eq!(
                answer(content(), &mut direct.state, pending, &action),
                Err(expected)
            );
            assert_eq!(serde_json::to_vec(&direct).unwrap(), before);
            assert_eq!(serde_json::to_vec(own).unwrap(), before);
        }
        if case == "airlog:38.36" {
            // Moving the unresolved single Store between the two stated
            // allocations cannot select a new allocation policy in preflight.
            let mut alternative = valid.clone();
            alternative.draws[0].draws[0].amount.stores = StoresPoints::new(1);
            alternative.draws[1].draws[0].amount.stores = StoresPoints::new(2);
            let action = serde_json::to_value(&alternative).unwrap();
            assert!(
                space(content(), &own.state, Side::Axis, current)
                    .unwrap()
                    .check(&action)
                    .is_ok()
            );
            assert!(matches!(rejected_response(content(), own, action),
                Rejection::Engine(EngineError::Unsupported { case, .. }) if case == "airlog:38.36"));
        }
    }
}
