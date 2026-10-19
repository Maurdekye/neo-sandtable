use super::fueling_tests::{accepted, fuel_finish, preparation_fixture};
use super::*;
use crate::air::preparation::{CurrentPreparation, KnownAmmunition, OrdnanceKnowledge};
use arming::{ArmingCohort, ArmingSelection, BombLoad, GunSelection};

fn armed_game() -> (Game<Cna>, Vec<PlaneId>) {
    armed_game_count(2)
}

fn armed_game_count(count: i32) -> (Game<Cna>, Vec<PlaneId>) {
    let aircraft = content()
        .units
        .aircraft
        .values()
        .find(|a| {
            a.nation == "it"
                && a.role != "flying_boat"
                && a.modes.first().is_some_and(|m| {
                    m.bomb_capacity.is_some_and(|n| n > 0) && m.fuel_points.is_some()
                })
        })
        .unwrap()
        .id
        .clone();
    armed_game_source(count, &aircraft)
}

fn armed_game_source(count: i32, aircraft: &str) -> (Game<Cna>, Vec<PlaneId>) {
    let mut state = base(false);
    // Explicit source-backed replacement fixture; no scenario quantity asserted.
    let ids =
        inventory::receive_unassigned(content(), &mut state.air, Side::Axis, aircraft, count)
            .unwrap();
    inventory::update(content(), &mut state.air, |runtime| {
        for id in &ids {
            let p = runtime.aircraft.get_mut(id).unwrap();
            p.squadron = Some("axis.refit".into());
            p.facility = Some("airfield_benina".into());
        }
        Ok(())
    })
    .unwrap();
    let stock = &mut state
        .logistics
        .air_dumps
        .get_mut("refit.stock")
        .unwrap()
        .supplies;
    stock.fuel = 1000;
    stock.ammo = 1000;
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
    (
        Game {
            state,
            rng: rng.state(),
        },
        ids,
    )
}

fn arm_plan(
    game: &Game<Cna>,
    ids: &[PlaneId],
    guns: bool,
    bombs: bool,
    fuel: bool,
    refit_too: bool,
) -> RefitPlan {
    let servicer = SgsuId("axis.refit".into());
    let mut selected = RefitPlan {
        dues: vec![servicer.clone()],
        arming: vec![ArmingCohort {
            servicer: servicer.clone(),
            planes: ids
                .iter()
                .map(|id| ArmingSelection {
                    plane: id.clone(),
                    guns: if guns {
                        GunSelection::Load
                    } else {
                        GunSelection::Preserve
                    },
                    bombs: bombs.then_some(BombLoad {
                        mode: 0,
                        bomb_points: 1,
                    }),
                })
                .collect(),
        }],
        ..Default::default()
    };
    if refit_too {
        selected.cohorts = plan(&game.state, true).cohorts;
        selected.cohorts[0].priority = ids.to_vec();
    }
    let mut fuel_cost = 10;
    if fuel {
        selected.fueling = vec![refit::FuelCohort {
            servicer: servicer.clone(),
            planes: ids
                .iter()
                .map(|id| {
                    let p = &game.state.air.runtime.aircraft[id];
                    fuel_cost += crate::air::maintenance::fuel_points(
                        &content().units.aircraft[&p.aircraft],
                        0,
                    )
                    .unwrap()
                        * 10;
                    refit::FuelSelection {
                        plane: id.clone(),
                        mode: 0,
                    }
                })
                .collect(),
        }];
    }
    selected.draws = vec![refit::ConsumerDraws {
        servicer,
        draws: vec![crate::logistics::SupplyDraw {
            source: SupplySource::AirDump("refit.stock".into()),
            amount: SupplyDemand {
                fuel: FuelTenths::new(fuel_cost),
                stores: StoresPoints::new(if refit_too { 2 } else { 1 }),
                water: WaterPoints::new(1),
                ammo: AmmoPoints::new(
                    i32::from(guns)
                        + if bombs {
                            i32::try_from(ids.len()).unwrap()
                        } else {
                            0
                        },
                ),
            },
        }],
    }];
    selected
}

fn component_next_period(game: &mut Game<Cna>) {
    // Component-period staging, not a whole-campaign advancement assertion.
    game.state.cursor.op_stage = Some(3);
    let mut rng = CampaignRng::from_state(&game.rng);
    enter(
        content(),
        &mut game.state,
        &mut Cx {
            rng: &mut rng,
            events: &mut Vec::new(),
        },
        false,
    )
    .unwrap();
    assert_eq!(rng.state(), game.rng);
}

/// Cases: airlog:38.41, airlog:38.43, airlog:38.21, scen:59.32
#[test]
fn guns_only_unprepared_preserves_empty_fuel_without_mode_and_legacy_stays_unknown() {
    for legacy in [false, true] {
        let (mut game, ids) = armed_game();
        if legacy {
            for id in &ids {
                preparation_fixture(&mut game, id, CurrentPreparation::UnknownLegacy);
            }
        }
        let selected = arm_plan(&game, &ids, true, false, false, false);
        let stock = game.state.logistics.air_dumps["refit.stock"].supplies;
        let mut done = accepted(&game, &selected);
        assert_eq!(
            done.state.air.runtime.aircraft,
            game.state.air.runtime.aircraft
        );
        assert_eq!(
            done.state.logistics.air_dumps,
            game.state.logistics.air_dumps
        );
        let events = fuel_finish(&mut done);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].audience, Audience::Side(Side::Axis));
        assert!(events[0].hex.is_none() && events[0].unit_id.is_none());
        assert_eq!(done.rng, game.rng);
        assert_eq!(
            done.state.logistics.air_dumps["refit.stock"].supplies.ammo,
            stock.ammo - 1
        );
        for id in &ids {
            let CurrentPreparation::KnownAmmunition { knowledge } =
                &done.state.air.runtime.preparation.current[id]
            else {
                panic!("partial ammunition missing")
            };
            assert_eq!(knowledge.mode, None);
            assert_eq!(knowledge.gun_ammunition, Some(true));
            assert_eq!(
                knowledge.fuel_available,
                if legacy { None } else { Some(false) }
            );
            assert_eq!(
                knowledge.ordnance,
                if legacy {
                    OrdnanceKnowledge::Unknown
                } else {
                    OrdnanceKnowledge::Known { carried: None }
                }
            );
            assert_eq!(
                crate::air::preparation::known_fuel(
                    &done.state.air.runtime.preparation.current[id]
                ),
                None
            );
            assert!(done.state.air.runtime.aircraft[id].armed);
            assert!(!done.state.air.runtime.aircraft[id].fuelled);
        }
        assert!(
            done.state
                .air
                .runtime
                .preparation
                .initial_choices
                .is_empty()
        );
        let mut corrupt = done.state.air.clone();
        corrupt.runtime.aircraft.get_mut(&ids[0]).unwrap().fuelled = true;
        let aircraft = corrupt.runtime.aircraft[&ids[0]].aircraft.clone();
        corrupt
            .squadrons
            .get_mut("axis.refit")
            .unwrap()
            .planes
            .get_mut(&aircraft)
            .unwrap()
            .fuelled += 1;
        if !legacy {
            assert!(
                matches!(inventory::check(content(), &corrupt), Err(EngineError::Invariant { detail })
                if detail.contains("typed fuel and informational fuel mirror disagree"))
            );
        }
        let bytes = serde_json::to_vec(&done).unwrap();
        assert!(fuel_finish(&mut done).is_empty());
        assert_eq!(serde_json::to_vec(&done).unwrap(), bytes);
        let restored: Game<Cna> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(serde_json::to_vec(&restored).unwrap(), bytes);
    }
}

/// Cases: airlog:38.21, airlog:38.43, airlog:38.44, airlog:38.36
#[test]
fn simultaneous_fuel_guns_bombs_refit_share_exact_debit_without_extra_rng() {
    let (game, ids) = armed_game();
    let selected = arm_plan(&game, &ids, true, true, true, true);
    let mut refit_only = selected.clone();
    refit_only.fueling.clear();
    refit_only.arming.clear();
    refit_only.draws[0].draws[0].amount.fuel = FuelTenths::new(10);
    refit_only.draws[0].draws[0].amount.ammo = AmmoPoints::new(0);
    let mut control = accepted(&game, &refit_only);
    finish_game(&mut control);
    let mut done = accepted(&game, &selected);
    fuel_finish(&mut done);
    assert_eq!(done.rng, control.rng);
    assert_eq!(
        done.state.air.runtime.maintenance.receipts,
        control.state.air.runtime.maintenance.receipts
    );
    assert_eq!(
        done.state.air.runtime.maintenance.debits[&Side::Axis].len(),
        1
    );
    let stock = game.state.logistics.air_dumps["refit.stock"].supplies;
    let after = done.state.logistics.air_dumps["refit.stock"].supplies;
    assert_eq!(after.ammo, stock.ammo - 3);
    assert_eq!(
        after.fuel,
        stock.fuel - selected.draws[0].draws[0].amount.fuel.get() / 10
    );
    assert_eq!(after.stores, stock.stores - 2);
    assert_eq!(after.water, stock.water - 1);
    let charges = &done.state.air.runtime.maintenance.gun_charges[&Side::Axis];
    assert_eq!(charges.len(), 1);
    assert_eq!(charges[0].planes, ids);
    for id in &ids {
        let CurrentPreparation::KnownLoad { load } =
            &done.state.air.runtime.preparation.current[id]
        else {
            panic!("complete load missing")
        };
        assert!(load.fuel_available && load.gun_ammunition);
        assert_eq!(
            load.ordnance,
            Some(crate::air::preparation::CarriedOrdnance::Bombs { bomb_points: 1 })
        );
    }
}

/// Cases: airlog:38.21, airlog:38.43, airlog:38.44, airlog:38.25
#[test]
fn separate_fuel_and_ammunition_merge_in_both_orders_and_preserve_later_without_fee() {
    for fuel_first in [false, true] {
        let (game, ids) = armed_game();
        let mut first = arm_plan(&game, &ids, !fuel_first, !fuel_first, fuel_first, false);
        if fuel_first {
            first.arming.clear();
        }
        let mut done = accepted(&game, &first);
        fuel_finish(&mut done);
        component_next_period(&mut done);
        let mut second = arm_plan(&done, &ids, fuel_first, fuel_first, !fuel_first, false);
        if !fuel_first {
            second.arming.clear();
        }
        second.draws[0].draws[0].amount.stores = StoresPoints::new(0); // Same GT Stores already paid.
        let mut complete = accepted(&done, &second);
        fuel_finish(&mut complete);
        for id in &ids {
            let CurrentPreparation::KnownLoad { load } =
                &complete.state.air.runtime.preparation.current[id]
            else {
                panic!("complete load missing")
            };
            assert!(load.fuel_available && load.gun_ammunition && load.ordnance.is_some());
        }
        let stock = complete.state.logistics.air_dumps.clone();
        // New public strategic component period, retaining the real paid load.
        complete.state.cursor.block = crate::seq::Block::Post;
        complete.state.cursor.op_stage = None;
        complete.state.cursor.index = crate::seq::Block::Post
            .steps()
            .iter()
            .position(|s| s.anchor == STRATEGIC)
            .unwrap();
        let mut rng = CampaignRng::from_state(&complete.rng);
        enter(
            content(),
            &mut complete.state,
            &mut Cx {
                rng: &mut rng,
                events: &mut Vec::new(),
            },
            false,
        )
        .unwrap();
        let mut preserved = accepted(&complete, &RefitPlan::default());
        assert!(fuel_finish(&mut preserved).is_empty());
        assert_eq!(preserved.state.logistics.air_dumps, stock);
        assert_eq!(
            preserved.state.air.runtime.preparation.current,
            complete.state.air.runtime.preparation.current
        );
    }
}

/// Cases: airlog:38.43, airlog:38.42, airlog:38.23
/// Interpretations: interp:airlog-0024
#[test]
fn same_site_split_servicers_pay_one_squadron_point_in_stable_payer_order() {
    let (game, ids) = armed_game();
    let mut selected = arm_plan(&game, &ids, true, false, false, false);
    let foreign = SgsuId("axis.foreign".into());
    let mut other = selected.arming[0].clone();
    other.servicer = foreign.clone();
    other.planes = vec![other.planes.pop().unwrap()];
    selected.arming[0].planes.truncate(1);
    selected.arming.push(other);
    selected.dues.push(foreign.clone());
    selected.draws[0].draws[0].amount.ammo = AmmoPoints::new(0);
    let mut draws = selected.draws[0].clone();
    draws.servicer = foreign.clone();
    draws.draws[0].amount.ammo = AmmoPoints::new(1);
    selected.draws.push(draws);
    for reverse in [false, true] {
        let mut selected = selected.clone();
        if reverse {
            selected.arming.reverse();
            selected.draws.reverse();
            selected.dues.reverse();
        }
        let mut done = accepted(&game, &selected);
        fuel_finish(&mut done);
        assert_eq!(
            done.state.logistics.air_dumps["refit.stock"].supplies.ammo,
            game.state.logistics.air_dumps["refit.stock"].supplies.ammo - 1
        );
        let charge = &done.state.air.runtime.maintenance.gun_charges[&Side::Axis][0];
        assert_eq!(charge.payer, foreign);
        assert_eq!(charge.facility, FacilityId("airfield_benina".into()));
        assert_eq!(
            charge.planes.iter().cloned().collect::<BTreeSet<_>>(),
            ids.iter().cloned().collect()
        );
    }
}

/// Cases: airlog:38.43, airlog:38.42
/// Interpretations: interp:airlog-0026
#[test]
fn real_charge_history_binds_original_site_after_sgsu_moves() {
    let (game, ids) = armed_game();
    let selected = arm_plan(&game, &ids[..1], true, false, false, false);
    let accepted = accepted(&game, &selected);
    let current = period(&accepted.state).unwrap();
    let mut draft = accepted.state;
    let mut v = validate(content(), &draft, Side::Axis, current, &selected).unwrap();
    let paid = fueling::spend_and_admit(content(), &mut draft, Side::Axis, current, &mut v)
        .unwrap()
        .unwrap();
    inventory::update_servicing(content(), &mut draft.air, paid).unwrap();
    // Disposable component state after real payment/admission, before final
    // closure. This is not a claim that a closed campaign window reopens.
    let old_charge = draft.air.runtime.maintenance.gun_charges[&Side::Axis][0].clone();
    let site = draft
        .air
        .runtime
        .facilities
        .iter()
        .find(|(id, f)| {
            **id != old_charge.facility
                && f.operational()
                && f.properties(content(), id).is_ok_and(|p| {
                    p.theatre == FacilityTheatre::Africa
                        && matches!(p.location, Location::Hex { .. })
                        && p.kind.accepts(false)
                })
        })
        .unwrap()
        .0
        .clone();
    inventory::update(content(), &mut draft.air, |runtime| {
        runtime
            .sgsus
            .get_mut(&SgsuId("axis.refit".into()))
            .unwrap()
            .position = SgsuPosition::Facility(site.clone());
        runtime.aircraft.get_mut(&ids[1]).unwrap().facility = Some(site.0.clone());
        Ok(())
    })
    .unwrap();
    draft
        .logistics
        .air_dumps
        .get_mut("refit.stock")
        .unwrap()
        .facility = site;
    let game = Game::<Cna> {
        state: draft,
        rng: game.rng,
    };
    let later = arm_plan(&game, &ids[1..], true, false, false, false);
    assert!(
        matches!(validate(content(), &game.state, Side::Axis, current, &later),
        Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "airlog:38.43")
    );
    assert_eq!(
        game.state.air.runtime.maintenance.gun_charges[&Side::Axis][0],
        old_charge
    );
}

/// Cases: airlog:38.41, airlog:38.43, airlog:38.44, scen:59.32
#[test]
fn mode_conflicts_payload_bounds_missing_stock_and_initial_choice_reject_atomically() {
    let (game, ids) = armed_game();
    for failure in 0..5 {
        let mut game = game.clone();
        let mut selected = arm_plan(&game, &ids, true, true, true, false);
        match failure {
            0 => selected.arming[0].planes[0].bombs.as_mut().unwrap().mode = usize::MAX,
            1 => {
                selected.arming[0].planes[0]
                    .bombs
                    .as_mut()
                    .unwrap()
                    .bomb_points = i32::MAX
            }
            2 => {
                game.state
                    .logistics
                    .air_dumps
                    .get_mut("refit.stock")
                    .unwrap()
                    .supplies
                    .ammo = 0
            }
            3 => selected.arming[0].planes[1] = selected.arming[0].planes[0].clone(),
            _ => selected.dues.clear(),
        }
        assert!(matches!(
            rejected_response(content(), &game, serde_json::to_value(selected).unwrap()),
            Rejection::Illegal { .. }
        ));
    }
    let mut initial = game.clone();
    initial
        .state
        .air
        .runtime
        .preparation
        .initial_choices
        .insert(
            ids[0].clone(),
            crate::air::preparation::InitialChoice::Unselected,
        );
    preparation_fixture(
        &mut initial,
        &ids[0],
        CurrentPreparation::InitialUnspecified,
    );
    let selected = arm_plan(&initial, &ids, true, true, false, false);
    assert!(
        matches!(validate(content(), &initial.state, Side::Axis, period(&initial.state).unwrap(), &selected),
        Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "scen:59.32")
    );
}

/// Cases: airlog:38.43, airlog:38.44, airlog:38.34, land:3.6
#[test]
fn late_real_ammo_fuel_and_refit_draw_failure_preserves_game_rng_and_existing_events() {
    let (game, ids) = armed_game();
    let selected = arm_plan(&game, &ids, true, true, true, true);
    let mut ready = accepted(&game, &selected);
    let before = serde_json::to_vec(&ready).unwrap();
    let mut expected = CampaignRng::from_state(&ready.rng);
    expected.d6();
    let mut rng = CampaignRng::from_state(&ready.rng);
    let prefix = vec![EngineEvent::new(
        Audience::Side(Side::Axis),
        GameEvent::Note {
            text: "Existing event".into(),
        },
    )];
    let mut events = prefix.clone();
    let mut reached = false;
    assert!(
        finish_transaction(
            content(),
            &mut ready.state,
            &mut Cx {
                rng: &mut rng,
                events: &mut events
            },
            |draft, rolled| {
                reached = true;
                assert_eq!(rolled.state(), expected.state());
                assert_eq!(
                    draft.logistics.air_dumps["refit.stock"].supplies.ammo,
                    game.state.logistics.air_dumps["refit.stock"].supplies.ammo - 3
                );
                assert_eq!(
                    draft.air.runtime.maintenance.gun_charges[&Side::Axis].len(),
                    1
                );
                assert!(ids.iter().all(|id| draft.air.runtime.aircraft[id].fuelled
                    && draft.air.runtime.aircraft[id].armed));
                draft
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
        )
        .is_err()
    );
    assert!(reached);
    assert_eq!(serde_json::to_vec(&ready).unwrap(), before);
    assert_eq!(rng.state(), ready.rng);
    assert_eq!(events, prefix);
}

/// Cases: airlog:38.41, airlog:38.43, airlog:38.44, land:3.6
#[test]
fn enemy_windows_acceptance_and_advance_hide_real_ammunition_and_charge_receipts() {
    for legacy in [false, true] {
    for (guns, bombs, fuel) in [(true, false, false), (false, true, false), (true, true, true)] {
    let (mut game, ids) = armed_game();
    if legacy {
        for id in &ids { preparation_fixture(&mut game, id, CurrentPreparation::UnknownLegacy); }
    }
    let selected = arm_plan(&game, &ids, guns, bombs, fuel, false);
    let mut loaded = accepted(&game, &selected);
    let mut passed = accepted(&game, &RefitPlan::default());
    assert_indistinguishable(
        &Cna::dev(),
        content(),
        &loaded.state,
        &passed.state,
        Side::Commonwealth,
    );
    assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &loaded,
        &passed,
        &Command::Advance,
        Side::Commonwealth,
    );
    fuel_finish(&mut loaded);
    finish_game(&mut passed);
    component_next_period(&mut loaded);
    component_next_period(&mut passed);
    assert_indistinguishable(
        &Cna::dev(),
        content(),
        &loaded.state,
        &passed.state,
        Side::Commonwealth,
    );
    assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &loaded,
        &passed,
        &command(&loaded, Side::Commonwealth, Value::Null),
        Side::Commonwealth,
    );
    let a = accepted(&loaded, &RefitPlan::default());
    let b = accepted(&passed, &RefitPlan::default());
    assert_action_indistinguishable(
        &Cna::dev(),
        content(),
        &a,
        &b,
        &Command::Advance,
        Side::Commonwealth,
    );
    }
    }
}

/// Cases: airlog:38.41, airlog:38.43, airlog:38.44
#[test]
fn generic_updates_cannot_forge_ammunition_or_future_charge_credit() {
    let (mut game, ids) = armed_game();
    let before = serde_json::to_vec(&game).unwrap();
    assert!(
        inventory::update(content(), &mut game.state.air, |runtime| {
            runtime.preparation.current.insert(
                ids[0].clone(),
                CurrentPreparation::KnownAmmunition {
                    knowledge: KnownAmmunition {
                        mode: None,
                        fuel_available: Some(false),
                        gun_ammunition: Some(true),
                        ordnance: OrdnanceKnowledge::Known { carried: None },
                    },
                },
            );
            Ok(())
        })
        .is_err()
    );
    assert_eq!(serde_json::to_vec(&game).unwrap(), before);
    let mut ready = accepted(&game, &arm_plan(&game, &ids, true, false, false, false));
    fuel_finish(&mut ready);
    let before = serde_json::to_vec(&ready).unwrap();
    assert!(
        inventory::update(content(), &mut ready.state.air, |runtime| {
            runtime.maintenance.gun_charges.clear();
            Ok(())
        })
        .is_err()
    );
    assert_eq!(serde_json::to_vec(&ready).unwrap(), before);
    let paid_history = ready.state.air.runtime.maintenance.clone();
    assert!(
        inventory::update(content(), &mut game.state.air, |runtime| {
            runtime.maintenance.arming_receipts = paid_history.arming_receipts.clone();
            runtime.maintenance.gun_charges = paid_history.gun_charges.clone();
            Ok(())
        })
        .is_err()
    );
}

/// Cases: airlog:38.42, airlog:38.23, airlog:36.14
#[test]
fn arming_counts_guns_plus_bombs_once_and_unions_all_service_sgsus_at_positive_levels() {
    let (game, ids) = armed_game();
    let limit = capacity(
        content(),
        &game.state,
        &game.state.air.runtime.sgsus[&SgsuId("axis.refit".into())].nationality,
    )
    .unwrap();
    let (large, many) = armed_game_count(i32::try_from(limit + 1).unwrap());
    let too_many = arm_plan(&large, &many, true, true, false, false);
    assert!(
        matches!(validate(content(), &large.state, Side::Axis, period(&large.state).unwrap(), &too_many),
        Err(Rejection::Illegal { message }) if message.contains("arming service exceeds SGSU ready+reserve"))
    );
    let at_limit = arm_plan(
        &large,
        &many[..usize::try_from(limit).unwrap()],
        true,
        true,
        false,
        false,
    );
    validate(
        content(),
        &large.state,
        Side::Axis,
        period(&large.state).unwrap(),
        &at_limit,
    )
    .unwrap();
    for (levels, separate, legal) in [(1, false, true), (1, true, false), (2, true, true)] {
        let mut game = game.clone();
        inventory::update(content(), &mut game.state.air, |runtime| {
            runtime
                .facilities
                .get_mut(&FacilityId("airfield_benina".into()))
                .unwrap()
                .current_capacity = FacilityCapacity::Levels(levels);
            Ok(())
        })
        .unwrap();
        let mut selected = arm_plan(&game, &ids, true, true, true, true);
        if separate {
            let foreign = SgsuId("axis.foreign".into());
            selected.arming[0].servicer = foreign.clone();
            selected.dues.push(foreign.clone());
            selected.draws[0].draws[0].amount.ammo = AmmoPoints::new(0);
            selected.draws.push(refit::ConsumerDraws {
                servicer: foreign,
                draws: vec![crate::logistics::SupplyDraw {
                    source: SupplySource::AirDump("refit.stock".into()),
                    amount: SupplyDemand {
                        fuel: FuelTenths::new(10),
                        water: WaterPoints::new(1),
                        stores: StoresPoints::new(1),
                        ammo: AmmoPoints::new(3),
                    },
                }],
            });
        }
        let result = validate(
            content(),
            &game.state,
            Side::Axis,
            period(&game.state).unwrap(),
            &selected,
        );
        if legal {
            result.unwrap();
            let mut done = accepted(&game, &selected);
            fuel_finish(&mut done);
        } else {
            assert!(
                matches!(result, Err(Rejection::Illegal { message }) if message.contains("active servicing SGSUs exceed damaged facility capacity"))
            );
        }
    }
}

/// Cases: airlog:38.43, airlog:38.42
/// Interpretations: interp:airlog-0024
#[test]
fn distinct_squadrons_and_new_loading_in_later_period_have_distinct_charges() {
    let (mut game, ids) = armed_game();
    // Component fixture sets a second tactical assignment; all exact Air mirrors
    // are still updated canonically, and this is not a declaration producer.
    game.state
        .air
        .runtime
        .designation
        .assignments
        .insert("axis.foreign".into(), Family::LandSupport);
    inventory::update(content(), &mut game.state.air, |runtime| {
        runtime.aircraft.get_mut(&ids[1]).unwrap().squadron = Some("axis.foreign".into());
        Ok(())
    })
    .unwrap();
    let mut selected = arm_plan(&game, &ids, true, false, false, false);
    selected.draws[0].draws[0].amount.ammo = AmmoPoints::new(2);
    let mut done = accepted(&game, &selected);
    fuel_finish(&mut done);
    assert_eq!(
        done.state.air.runtime.maintenance.gun_charges[&Side::Axis].len(),
        2
    );
    assert_eq!(
        done.state.logistics.air_dumps["refit.stock"].supplies.ammo,
        game.state.logistics.air_dumps["refit.stock"].supplies.ammo - 2
    );
    let (game, ids) = armed_game();
    let mut first = accepted(
        &game,
        &arm_plan(&game, &ids[..1], true, false, false, false),
    );
    fuel_finish(&mut first);
    component_next_period(&mut first);
    let mut next = arm_plan(&first, &ids[1..], true, false, false, false);
    next.draws[0].draws[0].amount.stores = StoresPoints::new(0);
    let before = first.state.logistics.air_dumps["refit.stock"].supplies.ammo;
    let mut second = accepted(&first, &next);
    fuel_finish(&mut second);
    assert_eq!(
        second.state.logistics.air_dumps["refit.stock"]
            .supplies
            .ammo,
        before - 1
    );
    assert_eq!(
        second.state.air.runtime.maintenance.gun_charges[&Side::Axis].len(),
        2
    );
}

/// Cases: airlog:38.41, airlog:38.25
#[test]
fn optional_null_service_lists_and_bomb_preserve_fit_exact_advertised_schema() {
    let (game, ids) = armed_game();
    let empty = serde_json::json!({ "dues": [], "cohorts": [], "draws": [], "fueling": null, "arming": null });
    let schema = space(
        content(),
        &game.state,
        Side::Axis,
        period(&game.state).unwrap(),
    )
    .unwrap();
    schema.check(&empty).unwrap();
    let one = respond(&game, Side::Axis, empty);
    let mut done = respond(&one, Side::Commonwealth, Value::Null);
    assert!(fuel_finish(&mut done).is_empty());
    assert_eq!(
        done.state.air.runtime.aircraft,
        game.state.air.runtime.aircraft
    );
    let selected = arm_plan(&game, &ids, true, false, false, false);
    let mut omitted_bombs = serde_json::to_value(&selected).unwrap();
    for plane in omitted_bombs["arming"][0]["planes"].as_array_mut().unwrap() {
        plane.as_object_mut().unwrap().remove("bombs");
    }
    schema.check(&omitted_bombs).unwrap();
    respond(&game, Side::Axis, omitted_bombs);
}

/// Cases: airlog:38.21, airlog:38.41, airlog:38.43
#[test]
fn actual_two_source_modes_cannot_hide_original_known_fuel_mode_in_joint_arming() {
    let source = content().units.aircraft.values().find(|a| a.role != "flying_boat"
        && a.modes.len() >= 2 && a.modes.iter().take(2).all(|m| m.fuel_points.is_some())).unwrap();
    // Source-validation holding fixture; no scenario arrival or nationality
    // entitlement is claimed. Both modes and all costs are actual source data.
    let (mut game, ids) = armed_game_source(1, &source.id);
    preparation_fixture(&mut game, &ids[0], CurrentPreparation::KnownFuel { mode: 0, fuel_available: true });
    let unchanged = arm_plan(&game, &ids, true, false, true, false);
    let mut changed = unchanged.clone();
    changed.fueling[0].planes[0].mode = 1;
    changed.draws[0].draws[0].amount.fuel = FuelTenths::new(10 + crate::air::maintenance::fuel_points(source, 1).unwrap() * 10);
    let before = serde_json::to_vec(&game).unwrap();
    assert!(matches!(rejected_response(content(), &game, serde_json::to_value(&changed).unwrap()),
        Rejection::Illegal { message } if message.contains("original known mode")));
    assert_eq!(serde_json::to_vec(&game).unwrap(), before);
    // Defensive finish control for a corrupted accepted buffer, without
    // pretending that the real Respond accepted the illegal selection above.
    let mut invalid = accepted(&game, &RefitPlan::default());
    invalid.state.air.runtime.maintenance.waiting.insert(Side::Axis, changed.clone());
    let before = serde_json::to_vec(&invalid).unwrap();
    let mut rng = CampaignRng::from_state(&invalid.rng);
    let prefix = vec![EngineEvent::new(Audience::Side(Side::Axis), GameEvent::Note { text: "Existing event".into() })];
    let mut events = prefix.clone();
    assert!(finish(content(), &mut invalid.state, &mut Cx { rng: &mut rng, events: &mut events }).is_err());
    assert_eq!(serde_json::to_vec(&invalid).unwrap(), before);
    assert_eq!(rng.state(), invalid.rng);
    assert_eq!(events, prefix);
    let mut done = accepted(&game, &unchanged); fuel_finish(&mut done);
    assert_eq!(crate::air::preparation::known_fuel(&done.state.air.runtime.preparation.current[&ids[0]]), Some((0, true)));
    assert!(done.state.air.runtime.aircraft[&ids[0]].armed);
    // Existing fuel-only KnownFuel behavior remains supported without new ammo.
    changed.arming.clear(); changed.draws[0].draws[0].amount.ammo = AmmoPoints::new(0);
    let mut fuel_only = accepted(&game, &changed); fuel_finish(&mut fuel_only);
    assert_eq!(crate::air::preparation::known_fuel(&fuel_only.state.air.runtime.preparation.current[&ids[0]]), Some((1, true)));
}

/// Cases: airlog:38.43, airlog:38.42
/// Interpretations: interp:airlog-0024
#[test]
fn same_period_same_site_new_id_reuses_actual_paid_gun_charge_with_zero_new_ammo() {
    let (game, ids) = armed_game();
    let selected = arm_plan(&game, &ids[..1], true, false, false, false);
    let mut component = accepted(&game, &selected);
    let current = period(&component.state).unwrap();
    let mut v = validate(content(), &component.state, Side::Axis, current, &selected).unwrap();
    let paid = fueling::spend_and_admit(content(), &mut component.state, Side::Axis, current, &mut v).unwrap().unwrap();
    inventory::update_servicing(content(), &mut component.state.air, paid).unwrap();
    // Actual payment/admission component seam before the enclosing finish's
    // final closure. Copy only the dues/debit effects it would record; no closed
    // campaign window is reopened and no stock/charge credit is invented.
    inventory::update(content(), &mut component.state.air, |runtime| {
        let s = runtime.sgsus.get_mut(&SgsuId("axis.refit".into())).unwrap();
        s.stores_paid_game_turn = Some(v.paid.game_turn); s.fuel_water_paid = Some(v.paid);
        runtime.maintenance.debits.entry(Side::Axis).or_default().push(refit::DebitReceipt {
            servicer: SgsuId("axis.refit".into()), draws: selected.draws[0].draws.clone(),
            fuel_rounding: selected.draws[0].draws.iter().map(|d| (d.source.clone(), d.amount.fuel)).collect(),
        });
        Ok(())
    }).unwrap();
    let old_charge = component.state.air.runtime.maintenance.gun_charges[&Side::Axis].clone();
    let stock = component.state.logistics.air_dumps.clone();
    let mut later = arm_plan(&component, &ids[1..], true, false, false, false);
    later.dues.clear(); later.draws.clear();
    let mut v = validate(content(), &component.state, Side::Axis, current, &later).unwrap();
    assert_eq!(v.demands[&SgsuId("axis.refit".into())].ammo.get(), 0);
    let paid = fueling::spend_and_admit(content(), &mut component.state, Side::Axis, current, &mut v).unwrap().unwrap();
    inventory::update_servicing(content(), &mut component.state.air, paid).unwrap();
    assert_eq!(component.state.logistics.air_dumps, stock);
    assert_eq!(component.state.air.runtime.maintenance.gun_charges[&Side::Axis], old_charge);
    assert_eq!(component.state.air.runtime.maintenance.arming_receipts[&Side::Axis].len(), 2);
    assert!(component.state.air.runtime.aircraft[&ids[1]].armed);
}

/// Cases: airlog:38.44, airlog:38.41
#[test]
fn identical_existing_bomb_load_and_preserve_cost_zero_but_different_valid_payload_costs_one() {
    let source = content().units.aircraft.values().find(|a| a.role != "flying_boat"
        && a.modes.first().is_some_and(|m| m.fuel_points.is_some() && m.bomb_capacity.is_some_and(|n| n > 1))).unwrap();
    let (game, ids) = armed_game_source(1, &source.id);
    let first = arm_plan(&game, &ids, false, true, false, false);
    let mut done = accepted(&game, &first); fuel_finish(&mut done); component_next_period(&mut done);
    for identical in [false, true] {
        let mut unchanged = arm_plan(&done, &ids, false, identical, false, false);
        unchanged.dues.clear(); unchanged.draws.clear();
        let ready = validate(content(), &done.state, Side::Axis, period(&done.state).unwrap(), &unchanged).unwrap();
        assert!(ready.arming.changes.is_empty() && ready.demands.is_empty());
        let stock = done.state.logistics.air_dumps.clone();
        let mut preserved = accepted(&done, &unchanged); assert!(fuel_finish(&mut preserved).is_empty());
        assert_eq!(preserved.state.logistics.air_dumps, stock);
        assert_eq!(preserved.state.air.runtime.preparation.current, done.state.air.runtime.preparation.current);
    }
    let mut changed = arm_plan(&done, &ids, false, true, false, false);
    changed.arming[0].planes[0].bombs.as_mut().unwrap().bomb_points = source.modes[0].bomb_capacity.unwrap();
    changed.draws[0].draws[0].amount.stores = StoresPoints::new(0);
    let ready = validate(content(), &done.state, Side::Axis, period(&done.state).unwrap(), &changed).unwrap();
    assert_eq!(ready.demands[&SgsuId("axis.refit".into())].ammo.get(), 1);
    let stock = done.state.logistics.air_dumps["refit.stock"].supplies.ammo;
    let mut changed_game = accepted(&done, &changed); fuel_finish(&mut changed_game);
    assert_eq!(changed_game.state.logistics.air_dumps["refit.stock"].supplies.ammo, stock - 1);
}
