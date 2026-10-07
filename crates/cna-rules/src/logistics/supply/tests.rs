//! Integration tests against the real Graziani content and state.
use super::*;
use crate::Cna;
use crate::state::{Dump, Location, UnitSupply};
use cna_content::units::{Trucks, WeaponPoints};
use cna_core::engine::Ruleset;
use cna_core::visibility::Perspective;
use cna_protocol::Side;
use std::sync::OnceLock;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn state() -> (State, UnitId) {
    let state = State::new(content()).unwrap();
    let id = "it.1_libyan_div.viii_libyan_bn".into();
    (state, id)
}
fn fuel(n: i32) -> SupplyDemand {
    SupplyDemand {
        fuel: FuelTenths::new(n),
        ..SupplyDemand::default()
    }
}
fn dump(state: &mut State, id: &UnitId, name: &str, side: Side, n: i32) {
    state.logistics.dumps.insert(
        name.into(),
        Dump {
            marker: String::new(),
            id: name.into(),
            side,
            location: DumpLocation::Hex {
                hex: state.land.units[id].location.hex().unwrap().clone(),
            },
            supplies: Supplies {
                fuel: n,
                ..Supplies::default()
            },
            active: true,
            dummy: false,
        },
    );
}

/// Cases: airlog:49.12, airlog:49.13
/// Interpretations: interp:airlog-0001
#[test]
fn fractional_moves_mixed_weapons_and_trucks_keep_exact_fuel() {
    let (mut state, id) = state();
    let unit = state.land.units.get_mut(&id).unwrap();
    unit.toe = Some(Toe::Weapons(vec![
        WeaponPoints {
            weapon: "it.cv33".into(),
            n: 2,
        },
        WeaponPoints {
            weapon: "it.m11_39".into(),
            n: 1,
        },
    ]));
    unit.trucks = Trucks {
        light: 1,
        medium: 0,
        heavy: 0,
    };
    // 0.5 CP -> the printed 1-CP row, aggregated before source rounding.
    assert_eq!(
        movement_fuel_cost(content(), &state, &id, 2),
        Ok(FuelTenths::new(10))
    );
    assert_eq!(
        movement_fuel_cost(content(), &state, &id, 12),
        Ok(FuelTenths::new(30))
    );
    assert_eq!(
        movement_fuel_cost(content(), &state, &id, 24),
        Ok(FuelTenths::new(100))
    );
    assert_eq!(
        movement_fuel_cost(content(), &state, &id, 204),
        Ok(FuelTenths::new(550))
    );
    assert_eq!(
        movement_fuel_cost(content(), &state, &id, 0),
        Ok(FuelTenths::ZERO)
    );
    assert_eq!(
        movement_fuel_cost(content(), &state, &id, -1),
        Err(SupplyError::Invalid)
    );
}

/// Cases: airlog:49.12, airlog:49.13
#[test]
fn infantry_and_recce_use_current_strength_and_attached_trucks() {
    let (mut state, id) = state();
    assert_eq!(
        movement_fuel_cost(content(), &state, &id, 48),
        Ok(FuelTenths::ZERO)
    );
    state.land.units.get_mut(&id).unwrap().trucks.light = 2;
    assert_eq!(
        movement_fuel_cost(content(), &state, &id, 48),
        Ok(FuelTenths::new(60))
    );
    let recce = content()
        .units
        .units
        .values()
        .find(|oa| {
            oa.class
                .as_ref()
                .and_then(|c| content().units.classes.get(c))
                .is_some_and(|c| c.unit_type == "recce")
                && state
                    .land
                    .units
                    .get(&oa.id)
                    .is_some_and(|u| u.toe.is_some())
        })
        .unwrap()
        .id
        .clone();
    state.land.units.get_mut(&recce).unwrap().toe = Some(Toe::Under { under: 1 });
    let strength = toe_strength(content(), &state.land.units[&recce])
        .unwrap()
        .get();
    assert_eq!(strength, 1);
    assert_eq!(
        crate::logistics::rations::activity_points(content(), &state, &recce).unwrap(),
        1
    );
    assert_eq!(
        movement_fuel_cost(content(), &state, &recce, 20),
        Ok(FuelTenths::new(strength * 10))
    );
}

/// Cases: airlog:50.13, airlog:50.2
#[test]
fn ammo_counts_only_participating_toe_and_rejects_unsupported_modes() {
    assert_eq!(
        ammunition_cost(
            content(),
            AmmoMode::Played,
            AmmoAction::Barrage,
            ToeStrengthPoints::new(4)
        ),
        Ok(AmmoPoints::new(16))
    );
    assert_eq!(
        ammunition_cost(
            content(),
            AmmoMode::Played,
            AmmoAction::CloseAssaultInfClass,
            ToeStrengthPoints::new(2)
        ),
        Ok(AmmoPoints::new(2))
    );
    assert!(
        ammunition_cost(
            content(),
            AmmoMode::Played,
            AmmoAction::Barrage,
            ToeStrengthPoints::new(i32::MAX)
        )
        .is_err()
    );
    assert!(
        ammunition_cost(
            content(),
            AmmoMode::Abstracted,
            AmmoAction::BarragePhasingBattalionEq,
            ToeStrengthPoints::new(1)
        )
        .is_err()
    );
    assert!(
        ammunition_cost(
            content(),
            AmmoMode::Played,
            AmmoAction::AirToAirCombatOrStrafe,
            ToeStrengthPoints::new(1)
        )
        .is_err()
    );
}

/// Cases: airlog:49.15, airlog:49.16, airlog:50.15
/// Interpretations: interp:airlog-0001
#[test]
fn tanks_are_exact_and_repeated_dump_draws_round_once() {
    let (mut state, id) = state();
    state.logistics.unit_supply.insert(
        id.clone(),
        UnitSupply {
            activity_water: WaterPoints::ZERO,
            tank_fuel: FuelTenths::new(3),
            ..UnitSupply::default()
        },
    );
    dump(&mut state, &id, "local", Side::Axis, 2);
    spend_for_unit(
        &mut state,
        &id,
        fuel(13),
        &[
            SupplyDraw {
                source: SupplySource::Tank,
                amount: fuel(3),
            },
            SupplyDraw {
                source: SupplySource::Dump("local".into()),
                amount: fuel(6),
            },
            SupplyDraw {
                source: SupplySource::Dump("local".into()),
                amount: fuel(4),
            },
        ],
    )
    .unwrap();
    assert_eq!(state.logistics.unit_supply[&id].tank_fuel, FuelTenths::ZERO);
    assert_eq!(state.logistics.dumps["local"].supplies.fuel, 1);
    spend_for_unit(
        &mut state,
        &id,
        fuel(2),
        &[SupplyDraw {
            source: SupplySource::Dump("local".into()),
            amount: fuel(2),
        }],
    )
    .unwrap();
    assert_eq!(state.logistics.dumps["local"].supplies.fuel, 0);
}

/// Cases: airlog:49.15, airlog:49.16, airlog:50.15
#[test]
fn bad_allocations_leave_every_source_unchanged() {
    let (mut state, id) = state();
    dump(&mut state, &id, "a", Side::Axis, 1);
    dump(&mut state, &id, "b", Side::Axis, 0);
    let before = serde_json::to_value(&state).unwrap();
    let draws = [
        SupplyDraw {
            source: SupplySource::Dump("a".into()),
            amount: fuel(10),
        },
        SupplyDraw {
            source: SupplySource::Dump("b".into()),
            amount: fuel(10),
        },
    ];
    assert_eq!(
        spend_for_unit(&mut state, &id, fuel(20), &draws),
        Err(SupplyError::Insufficient)
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    assert_eq!(
        spend_for_unit(&mut state, &id, fuel(21), &draws),
        Err(SupplyError::Invalid)
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    assert_eq!(
        spend_for_unit(&mut state, &id, fuel(-1), &[]),
        Err(SupplyError::Invalid)
    );
}

/// Cases: airlog:49.15, airlog:49.16, airlog:50.15
#[test]
fn only_friendly_same_hex_active_real_sources_are_accessible() {
    let (mut state, id) = state();
    for name in ["local", "enemy", "inactive", "dummy", "distant"] {
        dump(&mut state, &id, name, Side::Axis, 100);
    }
    state.logistics.dumps.get_mut("enemy").unwrap().side = Side::Commonwealth;
    state.logistics.dumps.get_mut("inactive").unwrap().active = false;
    state.logistics.dumps.get_mut("dummy").unwrap().dummy = true;
    state.logistics.dumps.get_mut("distant").unwrap().location = DumpLocation::Hex {
        hex: "C4019".into(),
    };
    let sources = available_sources(&state, &id).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source, SupplySource::Dump("local".into()));
    for name in ["enemy", "inactive", "dummy", "distant", "missing"] {
        assert_eq!(
            spend_for_unit(
                &mut state,
                &id,
                fuel(10),
                &[SupplyDraw {
                    source: SupplySource::Dump(name.into()),
                    amount: fuel(10)
                }]
            ),
            Err(SupplyError::Invalid)
        );
    }
    state.land.units.get_mut(&id).unwrap().location = Location::NotArrived;
    assert_eq!(available_sources(&state, &id), Err(SupplyError::Invalid));
}

/// Cases: airlog:49.16, airlog:50.15
#[test]
fn first_line_cargo_and_ready_ammo_are_distinct_from_tanks() {
    let (mut state, id) = state();
    let carrier: UnitId = "it.1_libyan_div.1st_libyan_infantry_hq".into();
    state.land.units.get_mut(&carrier).unwrap().trucks.light = 1;
    state.logistics.unit_supply.insert(
        carrier.clone(),
        UnitSupply {
            activity_water: WaterPoints::ZERO,
            tank_fuel: FuelTenths::new(999),
            carried: Supplies {
                ammo: 8,
                fuel: 5,
                ..Supplies::default()
            },
            ..UnitSupply::default()
        },
    );
    state.logistics.unit_supply.insert(
        id.clone(),
        UnitSupply {
            ready_ammo: AmmoPoints::new(2),
            ..UnitSupply::default()
        },
    );
    let demand = SupplyDemand {
        ammo: AmmoPoints::new(6),
        fuel: FuelTenths::new(12),
        ..SupplyDemand::default()
    };
    spend_for_unit(
        &mut state,
        &id,
        demand,
        &[
            SupplyDraw {
                source: SupplySource::ReadyAmmo,
                amount: SupplyDemand {
                    ammo: AmmoPoints::new(2),
                    ..SupplyDemand::default()
                },
            },
            SupplyDraw {
                source: SupplySource::UnitStock(carrier.clone()),
                amount: SupplyDemand {
                    ammo: AmmoPoints::new(4),
                    fuel: FuelTenths::new(12),
                    ..SupplyDemand::default()
                },
            },
        ],
    )
    .unwrap();
    assert_eq!(state.logistics.unit_supply[&carrier].tank_fuel.get(), 999);
    assert_eq!(state.logistics.unit_supply[&carrier].carried.ammo, 4);
    assert_eq!(state.logistics.unit_supply[&carrier].carried.fuel, 3);
    state.land.units.get_mut(&carrier).unwrap().trucks.light = 0;
    assert!(
        !available_sources(&state, &id)
            .unwrap()
            .iter()
            .any(|s| matches!(s.source, SupplySource::UnitStock(_)))
    );
}

/// Cases: airlog:54.13, land:3.61, land:3.62
#[test]
fn owner_only_holdings_and_dump_inspection_do_not_leak_to_enemy() {
    let (mut state, id) = state();
    dump(&mut state, &id, "private_dump", Side::Axis, 123456);
    state.logistics.unit_supply.insert(
        id.clone(),
        UnitSupply {
            activity_water: WaterPoints::ZERO,
            tank_fuel: FuelTenths::new(98765),
            ..UnitSupply::default()
        },
    );
    let rules = Cna::dev();
    let own = rules.observe(content(), &state, Perspective::Side(Side::Axis));
    assert_eq!(
        own["logistics"]["unit_supply"][id.as_str()]["tank_fuel"],
        98765
    );
    assert!(
        rules
            .inspect(
                content(),
                &state,
                Perspective::Side(Side::Axis),
                "private_dump"
            )
            .is_ok()
    );
    assert_eq!(
        rules
            .inspect(
                content(),
                &state,
                Perspective::Side(Side::Axis),
                id.as_str()
            )
            .unwrap()["supplies"]["tank_fuel"],
        98765
    );
    let enemy = rules.observe(content(), &state, Perspective::Side(Side::Commonwealth));
    assert!(
        enemy["logistics"]["unit_supply"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert!(enemy["logistics"]["dumps"].get("private_dump").is_none());
    assert!(
        rules
            .inspect(
                content(),
                &state,
                Perspective::Side(Side::Commonwealth),
                "private_dump"
            )
            .is_err()
    );
    let restored: State = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(
        restored.logistics.unit_supply[&id],
        state.logistics.unit_supply[&id]
    );
}

/// Cases: airlog:49.12, airlog:49.13, airlog:49.14
/// Interpretations: interp:units-0005, interp:airlog-0001
#[test]
fn numeric_headquarters_use_exact_truck_factor_one_pricing_and_funding() {
    let (mut state, foot) = state();
    state.land.units.get_mut(&foot).unwrap().trucks = Trucks {
        light: 1,
        ..Default::default()
    };
    let ids: Vec<_> = state
        .land
        .units
        .values()
        .filter(|u| house_rule_hq_strength(content(), u).is_ok_and(|n| n.is_some()))
        .map(|u| u.id.clone())
        .collect();
    assert_eq!(ids.len(), 9);
    for id in ids {
        state.land.units.get_mut(&id).unwrap().trucks = Trucks::default();
        state.land.units.get_mut(&id).unwrap().location = Location::Hex {
            hex: "C4020".into(),
        };
        let n = toe_strength(content(), &state.land.units[&id])
            .unwrap()
            .get();
        for quarters in [1, 4, 8, 12, 16, 20, 28, 39, 201] {
            let one = movement_fuel_cost(content(), &state, &foot, quarters)
                .unwrap()
                .get();
            assert_eq!(
                movement_fuel_cost(content(), &state, &id, quarters)
                    .unwrap()
                    .get(),
                one * n
            );
        }
        let original = state.land.units[&id].toe.clone();
        let max = content().units.classes[content().units.units[&id].class.as_ref().unwrap()]
            .max_toe
            .unwrap();
        for (toe, strength) in [
            (Toe::Under { under: max - 1 }, max - 1),
            (Toe::Over { over: max + 1 }, max + 1),
        ] {
            state.land.units.get_mut(&id).unwrap().toe = Some(toe);
            assert_eq!(
                movement_fuel_cost(content(), &state, &id, 28)
                    .unwrap()
                    .get(),
                20 * strength
            );
        }
        state.land.units.get_mut(&id).unwrap().toe = original;
        let at_seven = movement_fuel_cost(content(), &state, &foot, 28)
            .unwrap()
            .get();
        assert_eq!(at_seven, 20);
        assert_eq!(
            movement_fuel_cost(content(), &state, &id, 28)
                .unwrap()
                .get(),
            at_seven * n
        );
        let cap = crate::logistics::fuel_capacity(content(), &state, &id).unwrap();
        state
            .logistics
            .unit_supply
            .entry(id.clone())
            .or_default()
            .tank_fuel = cap;
        let plan = crate::logistics::plan_segment_fuel(content(), &state, &id, 28).unwrap();
        assert_eq!(plan.increment.get(), at_seven * n);
        let result =
            crate::logistics::spend_segment_fuel_report(content(), &mut state, &id, 28).unwrap();
        assert_eq!(result.increment, plan.increment);
        assert_eq!(
            state.logistics.unit_supply[&id].tank_fuel.get(),
            cap.get() - at_seven * n
        );
        let checkpoint: State =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(
            crate::logistics::plan_segment_fuel(content(), &checkpoint, &id, 28)
                .unwrap()
                .increment,
            FuelTenths::ZERO
        );
    }
}

/// House billing never replaces explicit equipment or adds a second troop body charge.
/// Cases: airlog:49.12, airlog:49.13
/// Interpretations: interp:units-0005, interp:airlog-0001
#[test]
fn headquarters_identified_weapons_parenthesized_toe_and_absent_composition_keep_their_rules() {
    let (mut state, foot) = state();
    state.land.units.get_mut(&foot).unwrap().trucks = Trucks {
        light: 1,
        ..Default::default()
    };
    let id = state
        .land
        .units
        .values()
        .find(|u| content().units.units[&u.id].class.as_deref() == Some("it.g"))
        .unwrap()
        .id
        .clone();
    let weapon = content()
        .units
        .weapons
        .values()
        .find(|w| w.nation == "it" && w.kind == "gun" && w.fuel_rate.is_some_and(|r| r > 0))
        .unwrap();
    let point = content()
        .tables
        .airlog
        .fuel_consumption
        .fuel_for(weapon.fuel_rate.unwrap(), 7)
        .unwrap()
        .get();
    let u = state.land.units.get_mut(&id).unwrap();
    u.toe = Some(Toe::Weapons(vec![WeaponPoints {
        weapon: weapon.id.clone(),
        n: 1,
    }]));
    u.trucks = Trucks {
        medium: 1,
        ..Default::default()
    };
    assert_eq!(
        house_rule_hq_strength(content(), &state.land.units[&id]),
        Ok(None)
    );
    let truck = movement_fuel_cost(content(), &state, &foot, 28)
        .unwrap()
        .get();
    assert_eq!(
        movement_fuel_cost(content(), &state, &id, 28)
            .unwrap()
            .get(),
        point + truck
    );
    state.land.units.get_mut(&id).unwrap().toe = None;
    assert_eq!(
        movement_fuel_cost(content(), &state, &id, 28),
        Err(SupplyError::UnknownFuelRate)
    );
    let missing_strength = state
        .land
        .units
        .values()
        .find(|u| content().units.units[&u.id].class.as_deref() == Some("cw.a"))
        .unwrap()
        .id
        .clone();
    state.land.units.get_mut(&missing_strength).unwrap().toe =
        Some(Toe::Normal(cna_content::units::NormalToe::N));
    assert_eq!(
        house_rule_hq_strength(content(), &state.land.units[&missing_strength]),
        Err(SupplyError::UnknownFuelRate)
    );
    assert_eq!(
        movement_fuel_cost(content(), &state, &missing_strength, 28),
        Err(SupplyError::UnknownFuelRate)
    );
    assert_eq!(
        crate::logistics::fuel_capacity(content(), &state, &missing_strength),
        Err(SupplyError::UnknownFuelRate)
    );
    let paren = state
        .land
        .units
        .values()
        .find(|u| {
            content().units.units[&u.id]
                .class
                .as_ref()
                .and_then(|id| content().units.classes.get(id))
                .is_some_and(|c| c.unit_type == "headquarters" && c.max_toe_paren)
        })
        .unwrap()
        .id
        .clone();
    state.land.units.get_mut(&paren).unwrap().trucks = Trucks::default();
    state.land.units.get_mut(&paren).unwrap().toe =
        Some(Toe::Normal(cna_content::units::NormalToe::N));
    assert_eq!(
        house_rule_hq_strength(content(), &state.land.units[&paren]),
        Ok(None)
    );
    assert_eq!(
        movement_fuel_cost(content(), &state, &paren, 28).unwrap(),
        FuelTenths::ZERO
    );
}

/// Cases: airlog:49.13, airlog:49.16
#[test]
fn invalid_truck_quantities_reject_without_arithmetic_overflow() {
    let (mut state, id) = state();
    state
        .logistics
        .unit_supply
        .insert(id.clone(), UnitSupply::default());
    state.land.units.get_mut(&id).unwrap().trucks = Trucks {
        light: i32::MAX,
        medium: 1,
        heavy: 0,
    };
    assert_eq!(
        movement_fuel_cost(content(), &state, &id, 4),
        Err(SupplyError::Invalid)
    );
    assert_eq!(available_sources(&state, &id), Err(SupplyError::Invalid));
    state.land.units.get_mut(&id).unwrap().trucks = Trucks {
        light: -1,
        medium: 2,
        heavy: 0,
    };
    assert_eq!(available_sources(&state, &id), Err(SupplyError::Invalid));
}

/// Cases: scen:60.44, airlog:51.15, airlog:49.16, airlog:52.11
#[test]
fn unlimited_stocks_require_the_scenario_side_and_exact_resolved_city_membership() {
    let mut state = State::new(content()).unwrap();
    let cw: UnitId = "cw.unassigned_inf.1st_rnf_mg_bn".into();
    state.land.units.get_mut(&cw).unwrap().location = Location::Hex {
        hex: "E1730".into(),
    };
    state.land.units.get_mut(&cw).unwrap().trucks.light = 1;
    state.logistics.dumps.clear();
    let stocks = available_sources_with_content(content(), &state, &cw).unwrap();
    assert_eq!(stocks.len(), 1);
    assert_eq!(stocks[0].source, SupplySource::Unlimited);
    assert!(stocks[0].amount.water.is_zero());
    let amount = SupplyDemand {
        stores: StoresPoints::new(100),
        ..SupplyDemand::default()
    };
    let draws = vec![SupplyDraw {
        source: SupplySource::Unlimited,
        amount,
    }];
    assert!(spend_for_unit(&mut state, &cw, amount, &draws).is_err());
    spend_for_unit_with_content(content(), &mut state, &cw, amount, &draws).unwrap();
    let paid = super::super::spend_segment_fuel(content(), &mut state, &cw, 1).unwrap();
    assert_eq!(paid[0].source, SupplySource::Unlimited);
    state.land.units.get_mut(&cw).unwrap().location = Location::Hex {
        hex: "E1932".into(),
    };
    let before = serde_json::to_value(&state).unwrap();
    assert!(spend_for_unit_with_content(content(), &mut state, &cw, amount, &draws).is_err());
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    let axis: UnitId = "it.1_libyan_div.viii_libyan_bn".into();
    state.land.units.get_mut(&axis).unwrap().location = Location::Hex {
        hex: "E1730".into(),
    };
    assert!(
        !available_sources_with_content(content(), &state, &axis)
            .unwrap()
            .iter()
            .any(|s| s.source == SupplySource::Unlimited)
    );
}

/// Cases: land:4.45, airlog:50.14, airlog:51.11, airlog:52.41
#[test]
fn real_under_and_over_arrival_values_are_strength_and_consumption_agrees_with_board() {
    let mut state = State::new(content()).unwrap();
    state.turn.weather = Some(crate::state::WeatherState {
        kind: cna_tables::land::weather::WeatherKind::Normal,
        storm_sections: vec![],
    });
    state.cursor.op_stage = Some(1);
    for (name, actual) in [
        ("cw.2_nz_div.21st_nz_bn", 2),
        ("cw.unassigned_indian.6th_rajputana_rifles_bn", 4),
    ] {
        let id: UnitId = name.into();
        let oa = &content().units.units[&id];
        let unit = state.land.units.get_mut(&id).unwrap();
        unit.toe = oa.toe.clone();
        unit.location = Location::Hex {
            hex: "E1730".into(),
        };
        unit.trucks = Trucks::default();
        assert_eq!(toe_strength(content(), unit).unwrap().get(), actual);
        assert_eq!(crate::view::toe_points(content(), unit), Some(actual));
        assert_eq!(
            crate::logistics::rations::stores_required(content(), &state, &id).unwrap(),
            actual * 4
        );
        assert_eq!(
            ammunition_cost(
                content(),
                AmmoMode::Played,
                AmmoAction::CloseAssaultInfClass,
                ToeStrengthPoints::new(actual)
            )
            .unwrap()
            .get(),
            actual
        );
        assert_eq!(
            crate::logistics::water::requirements(content(), &state, &id)
                .unwrap()
                .infantry,
            1
        );
    }
}
/// Cases: land:4.45
#[test]
fn invalid_under_over_ranges_and_missing_maximum_are_rejected() {
    let (mut state, id) = state();
    let maximum = content().units.classes[content().units.units[&id].class.as_ref().unwrap()]
        .max_toe
        .unwrap();
    for toe in [
        Toe::Under { under: maximum },
        Toe::Under { under: -1 },
        Toe::Over { over: maximum },
        Toe::Over { over: -1 },
    ] {
        state.land.units.get_mut(&id).unwrap().toe = Some(toe);
        assert_eq!(
            toe_strength(content(), &state.land.units[&id]),
            Err(SupplyError::Invalid)
        );
        assert_eq!(
            crate::view::toe_points(content(), &state.land.units[&id]),
            None
        );
    }
    let mut unknown = state.land.units[&id].clone();
    unknown.id = "unknown-max".into();
    unknown.toe = Some(Toe::Under { under: 1 });
    assert_eq!(toe_strength(content(), &unknown), Err(SupplyError::Invalid));
}
