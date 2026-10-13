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
        .filter_map(|id| {
            if content()
                .tables
                .airlog
                .aircraft_refit
                .plane_refitted(
                    nation,
                    i32::from(expected_rng.two_dice_reading().sum()),
                    true,
                ) {
                Some(id.clone())
            } else {
                None
            }
        })
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
    assert!(matches!(validate(content(), &first.state, Side::Axis,
        period(&first.state).unwrap(), &proposed),
        Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "scen:59.36"));
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
    assert!(
        matches!(validate(content(), &game.state, Side::Axis, current, &split),
        Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "airlog:38.36")
    );
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
    assert!(
        matches!(validate(content(), &game.state, Side::Axis, current, &plan(&game.state, true)),
        Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "airlog:36.5")
    );
    assert_eq!(serde_json::to_vec(&game).unwrap(), before);
}
