use super::*;
use crate::air::preparation::{self, CurrentPreparation, InitialChoice, KnownLoad};
use refit::{FuelCohort, FuelSelection};

fn fueled_plan(game: &Game<Cna>, refit_too: bool, foreign: bool) -> RefitPlan {
    let mut selected = plan(&game.state, true);
    let planes = selected.cohorts[0].priority.iter().map(|id| FuelSelection { plane: id.clone(), mode: 0 }).collect::<Vec<_>>();
    let cost: i32 = planes.iter().map(|s| {
        let p = &game.state.air.runtime.aircraft[&s.plane];
        crate::air::maintenance::fuel_points(&content().units.aircraft[&p.aircraft], s.mode).unwrap() * 10
    }).sum();
    let servicer = SgsuId(if foreign { "axis.foreign" } else { "axis.refit" }.into());
    if !refit_too { selected.cohorts.clear(); }
    selected.dues = vec![servicer.clone()];
    selected.draws[0].servicer = servicer.clone();
    selected.draws[0].draws[0].amount.fuel = FuelTenths::new(cost + 10);
    selected.draws[0].draws[0].amount.stores = StoresPoints::new(if refit_too { 2 } else { 1 });
    selected.fueling = vec![FuelCohort { servicer, planes }];
    selected
}

fn game_with_fuel() -> Game<Cna> {
    game_with_fuel_at(false)
}

fn game_with_fuel_at(strategic: bool) -> Game<Cna> {
    let mut game = opened(strategic);
    // Finite synthetic test stocks, not a source-derived scenario quantity.
    game.state.logistics.air_dumps.get_mut("refit.stock").unwrap().supplies.fuel = 1000;
    game
}

fn accepted(game: &Game<Cna>, selected: &RefitPlan) -> Game<Cna> {
    let one = respond(game, Side::Axis, serde_json::to_value(selected).unwrap());
    respond(&one, Side::Commonwealth, Value::Null)
}

fn fuel_finish(game: &mut Game<Cna>) -> Vec<EngineEvent> {
    let mut rng = CampaignRng::from_state(&game.rng);
    let mut events = Vec::new();
    finish(content(), &mut game.state, &mut Cx { rng: &mut rng, events: &mut events }).unwrap();
    game.rng = rng.state();
    events
}

// Structural fixture only, with exact mirrors. This does not mint a paid token.
fn preparation_fixture(game: &mut Game<Cna>, id: &PlaneId, next: CurrentPreparation) {
    let old = game.state.air.runtime.aircraft[id].clone();
    let armed = preparation::armed_projection(&next, old.armed);
    let fuelled = preparation::known_fuel(&next).map_or(old.fuelled, |(_, b)| b);
    let p = game.state.air.runtime.aircraft.get_mut(id).unwrap();
    p.armed = armed;
    p.fuelled = fuelled;
    let count = game.state.air.forces.get_mut(&old.force).unwrap().planes.get_mut(&old.aircraft).unwrap();
    count.armed += i32::from(armed) - i32::from(old.armed);
    count.fuelled += i32::from(fuelled) - i32::from(old.fuelled);
    if let Some(squadron) = &old.squadron {
        let count = game.state.air.squadrons.get_mut(squadron).unwrap().planes.get_mut(&old.aircraft).unwrap();
        count.armed += i32::from(armed) - i32::from(old.armed);
        count.fuelled += i32::from(fuelled) - i32::from(old.fuelled);
    }
    game.state.air.runtime.preparation.current.insert(id.clone(), next);
    inventory::check(content(), &game.state.air).unwrap();
}

/// Cases: airlog:38.21, airlog:38.22, airlog:38.23, airlog:38.24, airlog:38.25, airlog:35.14
#[test]
fn actual_fuel_debit_foreign_service_preserves_unknown_ammo_and_recovery() {
    for strategic in [false, true] {
    for foreign in [false, true] {
        let game = game_with_fuel_at(strategic);
        let selected = fueled_plan(&game, false, foreign);
        let old_air = game.state.air.clone();
        let before_stock = game.state.logistics.air_dumps["refit.stock"].supplies;
        let mut done = accepted(&game, &selected);
        assert_eq!(done.state.air.runtime.aircraft, old_air.runtime.aircraft);
        assert_eq!(serde_json::to_value(&done.state.logistics).unwrap(), serde_json::to_value(&game.state.logistics).unwrap());
        assert_eq!(done.rng, game.rng);
        let restored: Game<Cna> = serde_json::from_slice(&serde_json::to_vec(&done).unwrap()).unwrap();
        done = restored;
        let events = fuel_finish(&mut done);
        assert_eq!(done.rng, game.rng);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].audience, Audience::Side(Side::Axis));
        assert!(events[0].hex.is_none() && events[0].unit_id.is_none());
        let stock = done.state.logistics.air_dumps["refit.stock"].supplies;
        assert_eq!(stock.fuel, before_stock.fuel - selected.draws[0].draws[0].amount.fuel.get() / 10);
        assert_eq!(stock.stores, before_stock.stores - 1);
        assert_eq!(stock.water, before_stock.water - 1);
        assert_eq!(stock.ammo, before_stock.ammo);
        for selection in &selected.fueling[0].planes {
            let p = &done.state.air.runtime.aircraft[&selection.plane];
            assert!(p.fuelled);
            assert_eq!(p.refitted, old_air.runtime.aircraft[&selection.plane].refitted);
            assert_eq!(p.armed, old_air.runtime.aircraft[&selection.plane].armed);
            assert_eq!(done.state.air.runtime.preparation.current[&selection.plane], CurrentPreparation::KnownFuel {
                mode: selection.mode, fuel_available: true,
            });
        }
        assert!(done.state.air.runtime.preparation.initial_choices.is_empty());
        assert_eq!(done.state.air.runtime.maintenance.fuel_receipts[&Side::Axis].len(), 1);
        inventory::check(content(), &done.state.air).unwrap();
        let bytes = serde_json::to_vec(&done).unwrap();
        assert!(fuel_finish(&mut done).is_empty());
        assert_eq!(serde_json::to_vec(&done).unwrap(), bytes);
        let recovered: Game<Cna> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(serde_json::to_vec(&recovered).unwrap(), bytes);
    }
    }
}

/// Cases: airlog:38.21, airlog:38.24, airlog:38.25, airlog:38.41, scen:59.32
#[test]
fn paid_known_load_preserves_ammo_unprepared_is_empty_and_initial_remains_joint() {
    for unprepared in [false, true] {
        let mut game = game_with_fuel();
        let selected = fueled_plan(&game, false, false);
        for s in &selected.fueling[0].planes {
            preparation_fixture(&mut game, &s.plane, if unprepared { CurrentPreparation::Unprepared } else {
                CurrentPreparation::KnownLoad { load: KnownLoad { mode: 0, fuel_available: false, gun_ammunition: true, ordnance: None } }
            });
        }
        let mut done = accepted(&game, &selected);
        fuel_finish(&mut done);
        for s in &selected.fueling[0].planes {
            let CurrentPreparation::KnownLoad { load } = &done.state.air.runtime.preparation.current[&s.plane] else { panic!("known load missing"); };
            assert!(load.fuel_available);
            assert_eq!(load.gun_ammunition, !unprepared);
            assert!(load.ordnance.is_none());
        }
    }
    let mut game = game_with_fuel();
    let selected = fueled_plan(&game, false, false);
    let id = selected.fueling[0].planes[0].plane.clone();
    game.state.air.runtime.preparation.initial_choices.insert(id.clone(), InitialChoice::Unselected);
    preparation_fixture(&mut game, &id, CurrentPreparation::InitialUnspecified);
    let before = serde_json::to_vec(&game).unwrap();
    assert!(matches!(validate(content(), &game.state, Side::Axis, period(&game.state).unwrap(), &selected),
        Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "scen:59.32"));
    assert_eq!(serde_json::to_vec(&game).unwrap(), before);
}

/// Cases: airlog:38.21, airlog:38.23, airlog:38.24, scen:59.36
#[test]
fn fuel_duplicates_source_gap_stock_and_first_period_reject_without_mutation() {
    let game = game_with_fuel();
    for failure in 0..6 {
        let mut game = game.clone();
        let mut selected = fueled_plan(&game, false, false);
        match failure {
            0 => selected.fueling[0].planes[1] = selected.fueling[0].planes[0].clone(),
            1 => selected.fueling[0].planes[0].mode = usize::MAX,
            2 => selected.dues.clear(),
            3 => game.state.logistics.air_dumps.get_mut("refit.stock").unwrap().supplies.fuel = 0,
            4 => game.state.logistics.air_dumps.get_mut("refit.stock").unwrap().side = Side::Commonwealth,
            _ => game.state.logistics.air_dumps.get_mut("refit.stock").unwrap().facility = FacilityId("airfield_deversoir".into()),
        }
        assert!(matches!(rejected_response(content(), &game, serde_json::to_value(selected).unwrap()), Rejection::Illegal { .. }));
    }
    let first = opened_first_period();
    assert!(matches!(rejected_response(content(), &first, serde_json::to_value(fueled_plan(&first, false, false)).unwrap()), Rejection::Illegal { .. }));
    let selected = fueled_plan(&game, false, false);
    let mut source = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let plane = &game.state.air.runtime.aircraft[&selected.fueling[0].planes[0].plane];
    source.units.aircraft.get_mut(&plane.aircraft).unwrap().modes[0].fuel_points = None;
    assert!(matches!(validate(&source, &game.state, Side::Axis, period(&game.state).unwrap(), &selected),
        Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "airlog:38.21"));
}

/// Cases: airlog:38.21, airlog:38.23, airlog:38.24, airlog:38.34, airlog:38.36
#[test]
fn refit_and_fuel_share_debit_but_not_throughput_or_rng_and_late_failure_rolls_back() {
    let game = game_with_fuel();
    let mut plain = accepted(&game, &plan(&game.state, true));
    let selected = fueled_plan(&game, true, false);
    let mut both = accepted(&game, &selected);
    finish_game(&mut plain);
    fuel_finish(&mut both);
    assert_eq!(both.rng, plain.rng);
    assert_eq!(both.state.air.runtime.maintenance.receipts, plain.state.air.runtime.maintenance.receipts);
    assert_eq!(both.state.air.runtime.maintenance.debits[&Side::Axis].len(), 1);
    let mut failed = accepted(&game, &selected);
    failed.state.air.forces.get_mut("axis").unwrap().planes.values_mut().next().unwrap().total += 1;
    let bytes = serde_json::to_vec(&failed).unwrap();
    let mut rng = CampaignRng::from_state(&failed.rng);
    let mut events = Vec::new();
    assert!(finish(content(), &mut failed.state, &mut Cx { rng: &mut rng, events: &mut events }).is_err());
    assert_eq!(rng.state(), failed.rng);
    assert!(events.is_empty());
    assert_eq!(serde_json::to_vec(&failed).unwrap(), bytes);
}

/// Cases: airlog:38.21, airlog:38.24, airlog:38.25, land:3.6
#[test]
fn fuel_stock_knowledge_and_hidden_service_preserve_enemy_acceptance_and_advance_stream() {
    let a = game_with_fuel();
    let mut b = a.clone();
    b.state.logistics.air_dumps.get_mut("refit.stock").unwrap().supplies.fuel += 3;
    let selected = fueled_plan(&a, false, false);
    let action = command(&a, Side::Axis, serde_json::to_value(&selected).unwrap());
    let pass = command(&a, Side::Axis, Value::Null);
    assert_indistinguishable(&Cna::dev(), content(), &a.state, &b.state, Side::Commonwealth);
    assert_action_indistinguishable(&Cna::dev(), content(), &a, &b, &action, Side::Commonwealth);
    assert_actions_indistinguishable(&Cna::dev(), content(), (&a, &action), (&a, &pass), Side::Commonwealth);
    let serviced = accepted(&a, &selected);
    let passed = respond(&respond(&a, Side::Axis, Value::Null), Side::Commonwealth, Value::Null);
    assert_action_indistinguishable(&Cna::dev(), content(), &serviced, &passed, &Command::Advance, Side::Commonwealth);
}

/// Cases: airlog:38.22, airlog:38.23, airlog:38.24, airlog:38.25, airlog:36.14
#[test]
fn fuel_capacity_counts_reserve_planes_and_all_servicers_without_recharging_history() {
    let mut state = base(false);
    state.logistics.air_dumps.get_mut("refit.stock").unwrap().supplies.fuel = 1000;
    let current = period(&state).unwrap();
    let original = plan(&state, true);
    let source_id = original.cohorts[0].priority[0].clone();
    let source = state.air.runtime.aircraft[&source_id].aircraft.clone();
    let nation = state.air.runtime.sgsus[&SgsuId("axis.refit".into())].nationality.clone();
    let limit = capacity(content(), &state, &nation).unwrap();
    let ids = inventory::receive_unassigned(content(), &mut state.air, Side::Axis, &source,
        i32::try_from(limit + 1).unwrap()).unwrap();
    inventory::update(content(), &mut state.air, |runtime| {
        for id in &ids {
            let p = runtime.aircraft.get_mut(id).unwrap();
            p.squadron = Some("axis.refit".into());
            p.facility = Some("airfield_benina".into());
        }
        Ok(())
    }).unwrap();
    let mut rng = CampaignRng::from_seed([41; 32]);
    enter(content(), &mut state, &mut Cx { rng: &mut rng, events: &mut Vec::new() }, false).unwrap();
    let game = Game::<Cna> { state, rng: rng.state() };
    let mut selected = fueled_plan(&game, false, false);
    selected.fueling[0].planes = ids.iter().map(|id| FuelSelection { plane: id.clone(), mode: 0 }).collect();
    let rate = crate::air::maintenance::fuel_points(&content().units.aircraft[&source], 0).unwrap() * 10;
    selected.draws[0].draws[0].amount.fuel = FuelTenths::new(rate * i32::try_from(limit + 1).unwrap() + 10);
    assert!(matches!(validate(content(), &game.state, Side::Axis, current, &selected), Err(Rejection::Illegal { .. })));
    selected.fueling[0].planes.pop();
    selected.draws[0].draws[0].amount.fuel = FuelTenths::new(rate * i32::try_from(limit).unwrap() + 10);
    // New arrivals remain unrefitted; that does not prohibit fueling reserves.
    assert!(selected.fueling[0].planes.iter().all(|s| !game.state.air.runtime.aircraft[&s.plane].refitted));
    validate(content(), &game.state, Side::Axis, current, &selected).unwrap();
    let mut done = accepted(&game, &selected);
    fuel_finish(&mut done);
    assert!(matches!(validate(content(), &done.state, Side::Axis, current, &selected), Err(Rejection::Illegal { .. })));
    let mut moved = selected.clone();
    moved.fueling[0].servicer = SgsuId("axis.foreign".into());
    moved.dues = vec![SgsuId("axis.foreign".into())];
    moved.draws[0].servicer = SgsuId("axis.foreign".into());
    assert!(matches!(validate(content(), &done.state, Side::Axis, current, &moved), Err(Rejection::Illegal { .. })));
    let mut unavailable = game;
    inventory::update(content(), &mut unavailable.state.air, |runtime| {
        runtime.facilities.get_mut(&FacilityId("airfield_benina".into())).unwrap().current_capacity = FacilityCapacity::Levels(0);
        Ok(())
    }).unwrap();
    assert!(matches!(validate(content(), &unavailable.state, Side::Axis, current, &selected), Err(Rejection::Illegal { .. })));
    let mut future = done;
    // Public period rollover retains load facts and history without consuming fuel.
    future.state.cursor.op_stage = Some(3);
    let old = future.state.air.runtime.aircraft.clone();
    inventory::update(content(), &mut future.state.air, |runtime| {
        runtime.maintenance.period = Some(MaintenancePeriod::Tactical { game_turn: current_game_turn(current), op_stage: 3 });
        runtime.maintenance.closed.clear();
        runtime.maintenance.finished = false;
        Ok(())
    }).unwrap();
    assert_eq!(future.state.air.runtime.aircraft, old);
}

fn current_game_turn(period: MaintenancePeriod) -> u16 {
    match period { MaintenancePeriod::Tactical { game_turn, .. } | MaintenancePeriod::Strategic { game_turn } => game_turn }
}

/// Cases: airlog:38.21, airlog:38.24, airlog:38.25, airlog:34.0
#[test]
fn only_successful_combined_spend_mints_admission_and_changed_aircraft_invalidates_it() {
    let game = game_with_fuel();
    let selected = fueled_plan(&game, false, false);
    let ready = accepted(&game, &selected);
    let current = period(&ready.state).unwrap();
    let mut failed = ready.clone();
    let mut v = validate(content(), &failed.state, Side::Axis, current, &selected).unwrap();
    failed.state.logistics.air_dumps.get_mut("refit.stock").unwrap().supplies.fuel = 0;
    let before = serde_json::to_vec(&failed.state.air).unwrap();
    assert!(fueling::spend_and_admit(content(), &mut failed.state, Side::Axis, current, &mut v).is_err());
    assert_eq!(serde_json::to_vec(&failed.state.air).unwrap(), before);
    let mut paid_state = ready.state;
    let mut v = validate(content(), &paid_state, Side::Axis, current, &selected).unwrap();
    let paid = fueling::spend_and_admit(content(), &mut paid_state, Side::Axis, current, &mut v).unwrap().unwrap();
    let id = &selected.fueling[0].planes[0].plane;
    // A canonical intervening change is not allowed to ride the fuel permission.
    inventory::update(content(), &mut paid_state.air, |runtime| {
        runtime.aircraft.get_mut(id).unwrap().refitted = true;
        Ok(())
    }).unwrap();
    let before = serde_json::to_vec(&paid_state.air).unwrap();
    assert!(inventory::update_fueling(content(), &mut paid_state.air, paid).is_err());
    assert_eq!(serde_json::to_vec(&paid_state.air).unwrap(), before);
}

/// Cases: airlog:38.21, airlog:38.24, airlog:38.34, airlog:38.36, land:3.6
#[test]
fn late_inventory_error_after_actual_fuel_commit_and_refit_draw_rolls_back_complete_game() {
    let game = game_with_fuel();
    let selected = fueled_plan(&game, true, false);
    let mut accepted = accepted(&game, &selected);
    let before = serde_json::to_vec(&accepted).unwrap();
    let mut expected_rng = CampaignRng::from_state(&accepted.rng);
    expected_rng.d6(); // Exactly one actual same-nationality squadron refit group.
    let mut rng = CampaignRng::from_state(&accepted.rng);
    let mut events = Vec::new();
    let mut reached_late_boundary = false;
    let result = finish_transaction(content(), &mut accepted.state,
        &mut Cx { rng: &mut rng, events: &mut events }, |draft, draft_rng| {
            reached_late_boundary = true;
            assert_eq!(draft_rng.state(), expected_rng.state());
            assert_ne!(draft_rng.state(), game.rng);
            assert!(draft.logistics.air_dumps["refit.stock"].supplies.fuel < game.state.logistics.air_dumps["refit.stock"].supplies.fuel);
            for s in &selected.fueling[0].planes {
                assert!(draft.air.runtime.aircraft[&s.plane].fuelled);
                assert!(matches!(draft.air.runtime.preparation.current[&s.plane], CurrentPreparation::KnownFuel { fuel_available: true, .. }));
            }
            // The original early-mirror control remains; inject the SAME defect
            // here only after production fuel updates and a real draft refit roll.
            draft.air.forces.get_mut("axis").unwrap().planes.values_mut().next().unwrap().total += 1;
        });
    assert!(reached_late_boundary);
    assert_eq!(result, Err(EngineError::Invariant {
        detail: "air inventory: aggregate mirrors disagree with individual inventory".into(),
    }));
    assert_eq!(rng.state(), accepted.rng);
    assert!(events.is_empty());
    assert_eq!(serde_json::to_vec(&accepted).unwrap(), before);
}

fn assert_private_pair_through_pass(a: &Game<Cna>, b: &Game<Cna>) {
    assert_indistinguishable(&Cna::dev(), content(), &a.state, &b.state, Side::Commonwealth);
    assert_action_indistinguishable(&Cna::dev(), content(), a, b,
        &command(a, Side::Commonwealth, Value::Null), Side::Commonwealth);
    let a = accepted(a, &RefitPlan::default());
    let b = accepted(b, &RefitPlan::default());
    assert_action_indistinguishable(&Cna::dev(), content(), &a, &b,
        &Command::Advance, Side::Commonwealth);
}

/// Cases: airlog:38.21, airlog:38.24, airlog:38.25, land:3.6
#[test]
fn same_public_pass_outcome_hides_source_modes_ammo_knowledge_and_real_fuel_receipts() {
    let a = game_with_fuel();
    let id = a.state.air.runtime.aircraft.iter().find(|(_, p)| {
        owned(&p.force, Side::Axis) && content().units.aircraft[&p.aircraft].modes.len() > 1
            && content().units.aircraft[&p.aircraft].modes.iter().take(2).all(|m| m.fuel_points.is_some())
    }).expect("source-backed Axis aircraft with alternative fuel modes").0.clone();
    for current in [
        CurrentPreparation::KnownFuel { mode: 0, fuel_available: true },
        CurrentPreparation::KnownFuel { mode: 1, fuel_available: true },
        CurrentPreparation::KnownFuel { mode: 0, fuel_available: false },
        CurrentPreparation::KnownLoad { load: KnownLoad { mode: 0, fuel_available: true, gun_ammunition: false, ordnance: None } },
        CurrentPreparation::KnownLoad { load: KnownLoad { mode: 0, fuel_available: true, gun_ammunition: true, ordnance: None } },
    ] {
        let mut b = a.clone();
        preparation_fixture(&mut b, &id, current);
        assert_private_pair_through_pass(&a, &b);
    }
    let selected = fueled_plan(&a, false, false);
    let mut serviced = accepted(&a, &selected);
    let mut passed = accepted(&a, &RefitPlan::default());
    fuel_finish(&mut serviced);
    finish_game(&mut passed);
    assert!(!serviced.state.air.runtime.maintenance.fuel_receipts.is_empty());
    assert!(passed.state.air.runtime.maintenance.fuel_receipts.is_empty());
    for game in [&mut serviced, &mut passed] {
        // Same public next-period component boundary; preserve genuine histories.
        game.state.cursor.op_stage = Some(3);
        let mut rng = CampaignRng::from_state(&game.rng);
        enter(content(), &mut game.state, &mut Cx { rng: &mut rng, events: &mut Vec::new() }, false).unwrap();
        assert_eq!(rng.state(), game.rng);
    }
    assert_private_pair_through_pass(&serviced, &passed);
}
