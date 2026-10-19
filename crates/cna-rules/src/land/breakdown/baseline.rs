//! Conservative complete partitions for scripted owners; every returned plan validates.
use super::{
    Asset, RolledCheck, balanced_allocation,
    losses::{self, LossPlan, UnitPartition},
    private_encoder::{Attempt, PassengerAttempt},
};
use crate::{
    CnaContent, State,
    logistics::{self, capacity::CargoPacking},
};
use cna_content::{scenario::Supplies, units::Trucks};
use std::collections::BTreeSet;
fn sub(a: Trucks, b: Trucks) -> Option<Trucks> {
    Some(Trucks {
        light: a.light.checked_sub(b.light).filter(|n| *n >= 0)?,
        medium: a.medium.checked_sub(b.medium).filter(|n| *n >= 0)?,
        heavy: a.heavy.checked_sub(b.heavy).filter(|n| *n >= 0)?,
    })
}
fn trucks(a: &[Asset]) -> Trucks {
    let mut t = Trucks::default();
    for a in a {
        match a.equipment {
            super::Equipment::LightTruck => t.light += a.points,
            super::Equipment::MediumTruck => t.medium += a.points,
            super::Equipment::HeavyTruck => t.heavy += a.points,
            _ => {}
        }
    }
    t
}
fn capacity(c: &CnaContent, t: Trucks) -> i32 {
    use cna_tables::airlog::trucks::TruckType::*;
    let halves: i64 = [(Light, t.light), (Medium, t.medium), (Heavy, t.heavy)]
        .into_iter()
        .map(|(k, n)| {
            i64::from(n)
                * i64::from(
                    c.tables
                        .airlog
                        .truck_characteristics
                        .truck(k)
                        .capacity_inf_toe_halves,
                )
        })
        .sum();
    (halves / 2).try_into().unwrap_or(i32::MAX)
}
pub(super) fn pack_two(
    c: &CnaContent,
    a: Trucks,
    at: Trucks,
    b: Trucks,
    bt: Trucks,
    stock: Supplies,
) -> Option<(CargoPacking, CargoPacking)> {
    if let Some(p) = logistics::capacity::find_packing(c, &a, &at, stock) {
        return Some((p, CargoPacking::default()));
    }
    if let Some(p) = logistics::capacity::find_packing(c, &b, &bt, stock) {
        return Some((CargoPacking::default(), p));
    }
    use cna_tables::airlog::{supply::SupplyType, trucks::TruckType};
    let kinds = [TruckType::Heavy, TruckType::Medium, TruckType::Light];
    let mut remaining = stock;
    let mut pa = CargoPacking::default();
    let mut pb = CargoPacking::default();
    for (trucks, transport, out) in [(a, at, &mut pa), (b, bt, &mut pb)] {
        for k in kinds {
            let count = match k {
                TruckType::Light => trucks.light - transport.light,
                TruckType::Medium => trucks.medium - transport.medium,
                TruckType::Heavy => trucks.heavy - transport.heavy,
            };
            if count < 0 {
                return None;
            }
            let chart = c.tables.airlog.truck_characteristics.truck(k);
            let types = [
                SupplyType::Ammo,
                SupplyType::Fuel,
                SupplyType::Stores,
                SupplyType::Water,
            ];
            let den = types.iter().try_fold(1i64, |d, t| {
                d.checked_mul(i64::from(chart.supply_capacity(*t)))
            })?;
            if den <= 0 {
                return None;
            }
            let mut room = i64::from(count) * den;
            let target = match k {
                TruckType::Light => &mut out.light,
                TruckType::Medium => &mut out.medium,
                TruckType::Heavy => &mut out.heavy,
            };
            for t in types {
                let (left, dest) = match t {
                    SupplyType::Ammo => (&mut remaining.ammo, &mut target.ammo),
                    SupplyType::Fuel => (&mut remaining.fuel, &mut target.fuel),
                    SupplyType::Stores => (&mut remaining.stores, &mut target.stores),
                    SupplyType::Water => (&mut remaining.water, &mut target.water),
                };
                let per = den / i64::from(chart.supply_capacity(t));
                let n = i64::from(*left).min(room / per);
                if n < 0 {
                    return None;
                }
                *dest = n.try_into().ok()?;
                *left -= *dest;
                room -= n * per;
            }
        }
    }
    if remaining != Supplies::default() {
        return super::packing::two(c, a, at, b, bt, stock);
    }
    Some((pa, pb))
}
/// Keep every broken vehicle together, assigning enough men and reserve fuel to its partition.
/// Failure returns no fabricated allocation; callers must leave the mandatory choice to its owner.
/// Cases: land:21.35, land:21.36, land:21.41, land:21.43
pub fn plan(c: &CnaContent, s: &State, outcome: &RolledCheck) -> Option<LossPlan> {
    plan_inner(c, s, outcome, &mut Attempt::default())
}

/// Initial unit-window probe only. Record after the SAME solver returns None;
/// successful alternatives never publish an earlier rejected candidate.
/// Cases: land:21.43
pub(super) fn plan_for_stop(c: &CnaContent, s: &State, outcome: &RolledCheck) -> Option<LossPlan> {
    let mut attempt = Attempt::default();
    let result = plan_inner(c, s, outcome, &mut attempt);
    if result.is_none() {
        // Inactive/already-recorded scopes do not call this encoding closure.
        // The total empty fallback is private unavailability, never game error.
        cna_core::diagnostic::record(|| super::private_encoder::payload(outcome, &attempt));
    }
    result
}

// The sole initial-window stop producer calls the SAME algorithm once. Local
// fixed observations never affect its choices, State/RNG or validator count.
fn plan_inner(
    c: &CnaContent, s: &State, outcome: &RolledCheck, attempt: &mut Attempt,
) -> Option<LossPlan> {
    let preferred = balanced_allocation(&outcome.group.assets, outcome.broken)?;
    if let Some(plan) = with_losses(c, s, outcome, preferred, attempt) {
        return Some(plan);
    }
    let (cells, indices) = super::allocation::grouped(&outcome.group.assets)?;
    let total: i64 = cells.iter().map(|a| i64::from(a.points)).sum();
    if total <= 0 {
        return None;
    }
    let bounds: Vec<_> = cells
        .iter()
        .map(|a| {
            let n = i64::from(a.points) * i64::from(outcome.broken);
            ((n / total) as i32, ((n + total - 1) / total) as i32)
        })
        .collect();
    fn ties(
        cells: &[Asset],
        bounds: &[(i32, i32)],
        chosen: &mut Vec<i32>,
        left: i32,
        try_plan: &mut impl FnMut(&[i32]) -> Option<LossPlan>,
    ) -> Option<LossPlan> {
        let i = chosen.len();
        if i == bounds.len() {
            return (left == 0
                && super::valid_group_allocation(cells, chosen.iter().sum(), chosen))
            .then(|| try_plan(chosen))
            .flatten();
        }
        let lo: i64 = bounds[i + 1..].iter().map(|(l, _)| i64::from(*l)).sum();
        let hi: i64 = bounds[i + 1..].iter().map(|(_, h)| i64::from(*h)).sum();
        for n in bounds[i].0..=bounds[i].1 {
            let remainder = i64::from(left) - i64::from(n);
            if remainder < lo || remainder > hi {
                continue;
            }
            chosen.push(n);
            if let Some(p) = ties(cells, bounds, chosen, left - n, try_plan) {
                return Some(p);
            }
            chosen.pop();
        }
        None
    }
    ties(
        &cells,
        &bounds,
        &mut vec![],
        outcome.broken,
        &mut |chosen| {
            *attempt = Attempt::default();
            let expanded = super::allocation::expand(&outcome.group.assets, &indices, chosen)?;
            with_losses(c, s, outcome, expanded, attempt)
        },
    )
}
fn with_losses(
    c: &CnaContent,
    s: &State,
    outcome: &RolledCheck,
    losses: Vec<i32>,
    attempt: &mut Attempt,
) -> Option<LossPlan> {
    *attempt = Attempt::default();
    let at_origin: Vec<_> = outcome
        .group
        .assets
        .iter()
        .zip(&losses)
        .map(|(a, n)| {
            if outcome.require_origin.contains(&a.unit) {
                *n
            } else {
                0
            }
        })
        .collect();
    let mut partitions = vec![];
    let units: BTreeSet<_> = outcome
        .group
        .assets
        .iter()
        .map(|a| a.unit.clone())
        .collect();
    for id in units {
        let broken: Vec<_> = outcome
            .group
            .assets
            .iter()
            .zip(&losses)
            .filter(|(a, _)| a.unit == id)
            .filter(|(_, n)| **n > 0)
            .map(|(a, n)| Asset {
                points: *n,
                ..a.clone()
            })
            .collect();
        let old = &s.land.units[&id];
        let lost = trucks(&broken);
        let low = Trucks {
            light: (lost.light - (old.trucks.light - old.transport_trucks.light)).max(0),
            medium: (lost.medium - (old.trucks.medium - old.transport_trucks.medium)).max(0),
            heavy: (lost.heavy - (old.trucks.heavy - old.transport_trucks.heavy)).max(0),
        };
        let mut selected = None;
        'transport: for light in low.light..=lost.light.min(old.transport_trucks.light) {
            for medium in low.medium..=lost.medium.min(old.transport_trucks.medium) {
                for heavy in low.heavy..=lost.heavy.min(old.transport_trucks.heavy) {
                    if let Some(p) = partition(
                        c,
                        s,
                        outcome,
                        id.clone(),
                        &broken,
                        Trucks {
                            light,
                            medium,
                            heavy,
                        },
                        attempt,
                    ) {
                        selected = Some(p);
                        break 'transport;
                    }
                }
            }
        }
        partitions.push(selected?);
    }
    let plan = LossPlan {
        losses,
        at_origin,
        partitions,
    };
    *attempt = Attempt::default(); // final validation has no private leaf hook yet
    losses::apply(c, &mut s.clone(), outcome, &plan).ok()?;
    Some(plan)
}

fn partition(
    c: &CnaContent,
    s: &State,
    outcome: &RolledCheck,
    id: cna_core::ids::UnitId,
    broken: &[Asset],
    transport: Trucks,
    attempt: &mut Attempt,
) -> Option<UnitPartition> {
    *attempt = Attempt::default(); // unknown until a real leaf is reached
    let old = &s.land.units[&id];
    let lost = trucks(broken);
    let working = sub(old.trucks, lost)?;
    let working_transport = sub(old.transport_trucks, transport)?;
    let strength = super::super::formation::strength(c, s, &id);
    let infantry =
        super::super::formation::class(c, &id).is_some_and(|k| k.unit_type == "infantry");
    let obligation = if infantry {
        match losses::passenger_obligation(c, strength, old.transport_trucks) {
            Ok(obligation) => obligation,
            Err(reason) => {
                attempt.reason = Some(reason);
                attempt.passenger = Some(PassengerAttempt::input(
                    id.as_str(), old.trucks, old.transport_trucks, strength,
                    working_transport, transport,
                ));
                return None;
            }
        }
    } else {
        losses::PassengerObligation {
            carried: 0,
            walking: strength,
        }
    };
    let unresolved = if infantry {
        losses::unresolved_points(
            c,
            obligation.carried,
            working_transport,
            transport,
            Trucks::default(),
        )
    } else {
        0
    };
    let passengers = if infantry {
        (obligation.carried - unresolved - capacity(c, working_transport)).max(0)
    } else {
        0
    };
    let broken_capacity = capacity(c, transport);
    if passengers > broken_capacity {
        attempt.reason = Some(super::allocation_reason::AllocationReason::Passenger(
            super::allocation_reason::PassengerReason::BrokenPassengersExceedCapacity,
        ));
        attempt.passenger = Some(PassengerAttempt::input(
            id.as_str(), old.trucks, old.transport_trucks, strength,
            working_transport, transport,
        ).reached_capacity(
            obligation.carried, obligation.walking, unresolved, passengers, broken_capacity,
        ));
        return None;
    }
    let stock = s
        .logistics
        .unit_supply
        .get(&id)
        .cloned()
        .unwrap_or_default();
    let (working_cargo, broken_cargo) = pack_two(
        c,
        working,
        working_transport,
        lost,
        transport,
        stock.carried,
    )?;
    let mut draft = s.clone();
    let u = draft.land.units.get_mut(&id)?;
    u.trucks = working;
    u.transport_trucks = working_transport;
    if passengers + unresolved > 0 {
        u.toe = Some(cna_content::units::Toe::Under {
            under: obligation.walking + obligation.carried - passengers - unresolved,
        });
    }
    if let Some(cna_content::units::Toe::Weapons(ws)) = &mut u.toe {
        for a in broken {
            if let super::Equipment::Weapon(w) = &a.equipment {
                ws.iter_mut().find(|p| &p.weapon == w)?.n -= a.points;
            }
        }
    }
    let cap = logistics::capacity::fuel_capacity(c, &draft, &id)
        .ok()?
        .get();
    let broken_fuel = (stock.tank_fuel.get() - cap).max(0);
    if broken_fuel > losses::marker_fuel_capacity(c, broken).ok()? {
        return None;
    }
    let origin = outcome.require_origin.contains(&id);
    Some(UnitPartition {
        unit: id,
        working: working_cargo,
        origin: if origin {
            broken_cargo.clone()
        } else {
            CargoPacking::default()
        },
        destination: if origin {
            CargoPacking::default()
        } else {
            broken_cargo
        },
        origin_transport: if origin { transport } else { Trucks::default() },
        destination_transport: if origin { Trucks::default() } else { transport },
        origin_passengers: if origin { passengers } else { 0 },
        destination_passengers: if origin { 0 } else { passengers },
        origin_tank_fuel_tenths: if origin { broken_fuel } else { 0 },
        destination_tank_fuel_tenths: if origin { 0 } else { broken_fuel },
        origin_activity_water: 0,
        destination_activity_water: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Ammo, fuel, stores and water share each truck's fractional capacity.
    /// A large-first greedy pass strands cargo despite this hand-checked legal split.
    /// Cases: land:21.43, airlog:53.11, airlog:54.2
    #[test]
    fn exact_fallback_finds_mixed_cargo_partition_that_greedy_misses() {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let working = Trucks {
            medium: 1,
            light: 1,
            ..Trucks::default()
        };
        let broken = Trucks {
            light: 1,
            ..Trucks::default()
        };
        let stock = Supplies {
            ammo: 1,
            fuel: 99,
            stores: 2,
            water: 66,
        };
        let witness_a = CargoPacking {
            medium: Supplies {
                fuel: 44,
                stores: 2,
                water: 50,
                ..Supplies::default()
            },
            light: Supplies {
                ammo: 1,
                fuel: 25,
                ..Supplies::default()
            },
            ..CargoPacking::default()
        };
        let witness_b = CargoPacking {
            light: Supplies {
                fuel: 30,
                water: 16,
                ..Supplies::default()
            },
            ..CargoPacking::default()
        };
        for (trucks, packing) in [(working, witness_a), (broken, witness_b)] {
            logistics::capacity::validate_packing(
                &c,
                &trucks,
                &Trucks::default(),
                &packing.totals().unwrap(),
                &packing,
            )
            .unwrap();
        }
        let (a, b) = pack_two(
            &c,
            working,
            Trucks::default(),
            broken,
            Trucks::default(),
            stock,
        )
        .unwrap();
        for (trucks, packing) in [(working, &a), (broken, &b)] {
            logistics::capacity::validate_packing(
                &c,
                &trucks,
                &Trucks::default(),
                &packing.totals().unwrap(),
                packing,
            )
            .unwrap();
        }
        let aa = a.totals().unwrap();
        let bb = b.totals().unwrap();
        assert_eq!(
            Supplies {
                ammo: aa.ammo + bb.ammo,
                fuel: aa.fuel + bb.fuel,
                stores: aa.stores + bb.stores,
                water: aa.water + bb.water
            },
            stock
        );
        let mut s = State::new(&c).unwrap();
        s.turn.weather = Some(crate::state::WeatherState {
            kind: cna_tables::land::weather::WeatherKind::Normal,
            storm_sections: vec![],
        });
        let id: cna_core::ids::UnitId = "cw.unassigned_inf.1st_rnf_mg_bn".into();
        s.land.units.get_mut(&id).unwrap().location = crate::state::Location::Hex {
            hex: "C4021".into(),
        };
        s.land.units.get_mut(&id).unwrap().trucks = Trucks {
            medium: 1,
            light: 2,
            ..Trucks::default()
        };
        s.land.units.get_mut(&id).unwrap().transport_trucks = Trucks::default();
        s.logistics
            .unit_supply
            .entry(id.clone())
            .or_default()
            .carried = stock;
        let outcome = RolledCheck {
            group: super::super::CheckGroup {
                category: super::super::Category::Truck,
                bar: -2,
                column: 2,
                shift: -2,
                assets: vec![Asset {
                    unit: id.clone(),
                    equipment: super::super::Equipment::LightTruck,
                    points: 2,
                    cohort: None,
                }],
            },
            percent: 50,
            broken: 1,
            destination: "C4021".into(),
            origins: std::collections::BTreeMap::from([(id, "C4020".into())]),
            require_origin: BTreeSet::new(),
        };
        let selected =
            plan(&c, &s, &outcome).expect("mandatory baseline must find legal mixed cargo");
        losses::apply(&c, &mut s, &outcome, &selected).unwrap();
    }
    // Captured scalars copied; ALL location/weather/cohort/context fields below are synthetic.
    fn captured_quantities_synthetic_context() -> (CnaContent, State, RolledCheck) {
        use super::super::{Category, CheckGroup, Equipment};
        use cna_core::ids::UnitId;
        use std::collections::{BTreeMap, BTreeSet};
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let id: UnitId = "cw.2_nz_div.19th_nz_bn".into();
        let u = s.land.units.get_mut(&id).unwrap();
        u.location = crate::state::Location::Hex {
            hex: "C4021".into(),
        }; // synthetic, not captured
        u.detached = true;
        u.attached_to = None;
        // Synthetic native representation: cw.m's printed maximum is the captured six TOE.
        // Under { under: 6 } is INVALID at maximum and formation::strength hides that error as zero.
        // Normal(N) is chosen only for this ordinary fixture, not claimed as the captured TOE variant.
        u.toe = Some(cna_content::units::Toe::Normal(
            cna_content::units::NormalToe::N,
        ));
        assert_eq!(crate::logistics::toe_strength(&c, u).unwrap().get(), 6);
        u.trucks = Trucks {
            light: 3,
            medium: 6,
            heavy: 1,
        };
        u.transport_trucks = Trucks {
            medium: 2,
            ..Trucks::default()
        };
        s.turn.weather = Some(crate::state::WeatherState {
            kind: cna_tables::land::weather::WeatherKind::Normal,
            storm_sections: vec![],
        }); // synthetic, not captured
        let stock = s.logistics.unit_supply.entry(id.clone()).or_default();
        stock.carried = Supplies {
            stores: 12,
            water: 53,
            ..Supplies::default()
        };
        stock.tank_fuel = cna_core::quantity::FuelTenths::new(0);
        stock.activity_water = cna_core::quantity::WaterPoints::new(0);
        // The existing validator initializes ordinary unfunded cohorts from this synthetic context.
        // No captured cohort/account history is claimed; we do not create histories just for capture.
        let outcome = RolledCheck {
            group: CheckGroup {
                category: Category::Truck,
                bar: -2,
                column: 3,
                shift: -2,
                assets: vec![
                    Asset {
                        unit: id.clone(),
                        equipment: Equipment::LightTruck,
                        points: 3,
                        cohort: None,
                    },
                    Asset {
                        unit: id.clone(),
                        equipment: Equipment::MediumTruck,
                        points: 6,
                        cohort: None,
                    },
                    Asset {
                        unit: id.clone(),
                        equipment: Equipment::HeavyTruck,
                        points: 1,
                        cohort: None,
                    },
                ],
            },
            percent: 10,
            broken: 1,
            destination: "C4021".into(),
            origins: BTreeMap::from([(id, "C4020".into())]),
            require_origin: BTreeSet::new(),
        }; // column/origins are synthetic; percent/broken/assets/require_origin match capture
        (c, s, outcome)
    }

    // Rules407 additive SOURCE control: all passenger holdings linked to THIS source unit.
    // This is a small ordinary test State, never a captured/replayed full campaign.
    fn assert_source_unit_toe_conserved(
        c: &CnaContent,
        s: &State,
        id: &cna_core::ids::UnitId,
        before: i32,
    ) {
        let active = crate::land::formation::strength(c, s, id);
        let marked: i32 = s
            .land
            .breakdown
            .markers
            .values()
            .filter_map(|marker| marker.passengers.get(id).copied())
            .sum();
        let unresolved: i32 = s
            .land
            .breakdown
            .unresolved_passengers
            .get(id)
            .into_iter()
            .flatten()
            .map(|entry| entry.points)
            .sum();
        assert_eq!(active + marked + unresolved, before);
    }

    /// Cases: land:8.92, land:8.95, land:21.43
    /// Interpretations: interp:land-0041 (provisional).
    #[test]
    fn partial_carriage_nontransport_breakdown_keeps_six_toe_and_foot_cpa() {
        let (c, mut s, outcome) = captured_quantities_synthetic_context();
        let id = outcome.group.assets[0].unit.clone();
        let obligation =
            losses::passenger_obligation(&c, 6, s.land.units[&id].transport_trucks).unwrap();
        assert_eq!((obligation.carried, obligation.walking), (2, 4));
        let allowance = crate::land::formation::individual_allowance(&c, &s, &id).unwrap();
        assert!(!allowance.motorized);
        assert_eq!(allowance.cpa, 10);
        let plan = plan(&c, &s, &outcome).expect("legal partial carriage partition");
        assert_eq!(plan.losses, vec![1, 0, 0]);
        assert_eq!(
            plan.partitions[0].origin_passengers + plan.partitions[0].destination_passengers,
            0
        );
        assert_eq!(
            plan.partitions[0].origin_transport.total()
                + plan.partitions[0].destination_transport.total(),
            0
        );
        losses::apply(&c, &mut s, &outcome, &plan).unwrap();
        assert_eq!(crate::land::formation::strength(&c, &s, &id), 6);
        assert_eq!(s.land.units[&id].transport_trucks.medium, 2);
        assert_eq!(s.land.units[&id].trucks.light, 2);
        assert_eq!(
            s.logistics.unit_supply[&id].carried,
            Supplies {
                stores: 12,
                water: 53,
                ..Supplies::default()
            }
        );
        assert!(!s.land.breakdown.unresolved_passengers.contains_key(&id));
        assert!(
            !crate::land::formation::individual_allowance(&c, &s, &id)
                .unwrap()
                .motorized
        );
        assert_source_unit_toe_conserved(&c, &s, &id, 6);
    }

    // Same successful algorithm/validator path with and without fixed local
    // observations. Test-only comparison; production never solves twice.
    #[test]
    fn private_observation_keeps_same_plan_and_full_state_bytes() {
        let (c, s, outcome) = captured_quantities_synthetic_context();
        let before = serde_json::to_vec(&s).unwrap();
        let ordinary = plan(&c, &s, &outcome).unwrap();
        let mut attempt = Attempt::default();
        let observed = plan_inner(&c, &s, &outcome, &mut attempt).unwrap();
        assert_eq!(serde_json::to_vec(&ordinary).unwrap(), serde_json::to_vec(&observed).unwrap());
        assert_eq!(serde_json::to_vec(&s).unwrap(), before);
        assert!(attempt.reason.is_none());
        assert!(attempt.passenger.is_none());
    }

    fn diagnosed_window_game(fail_before_candidate: bool) -> (CnaContent, cna_core::engine::Game<crate::Cna>) {
        let (c, mut s, mut outcome) = captured_quantities_synthetic_context();
        if fail_before_candidate {
            // Ordinary synthetic malformed roll boundary, not observed holdings,
            // validator exhaustion or a reconstruction of the real stopped game.
            outcome.broken = 100;
        }
        s.cursor.block = crate::seq::Block::PlayerHalf;
        s.cursor.half = Some(crate::seq::Half::A);
        s.cursor.op_stage = Some(1);
        s.cursor.index = 1;
        s.cursor.entered = true;
        s.turn.player_a = Some(cna_protocol::Side::Commonwealth);
        s.decisions.pending.clear();
        s.land.breakdown.window.parked = true;
        s.land.breakdown.window.outcomes.push_back(outcome);
        let rng = cna_core::dice::CampaignRng::from_seed([7; 32]);
        (c, cna_core::engine::Game { state: s, rng: rng.state() })
    }

    #[test]
    fn evaluated_capture_on_off_success_keeps_state_rng_events_and_views() {
        use cna_core::engine::{Command, Ruleset, evaluate, evaluate_diagnosed};
        use cna_core::visibility::Perspective;
        use cna_protocol::Side;
        let (c, game) = diagnosed_window_game(false);
        let input = serde_json::to_vec(&game.state).unwrap();
        let ordinary = evaluate(&crate::Cna::dev(), &c, &game, &Command::Advance).unwrap();
        let (observed, payload) = evaluate_diagnosed(&crate::Cna::dev(), &c, &game, &Command::Advance);
        let observed = observed.unwrap();
        assert!(payload.is_none()); // earlier attempts are never stop evidence
        assert_eq!(serde_json::to_vec(&ordinary.game.state).unwrap(), serde_json::to_vec(&observed.game.state).unwrap());
        assert_eq!(serde_json::to_vec(&ordinary.game.rng).unwrap(), serde_json::to_vec(&observed.game.rng).unwrap());
        assert_eq!(serde_json::to_vec(&ordinary.events).unwrap(), serde_json::to_vec(&observed.events).unwrap());
        assert_eq!(serde_json::to_vec(&ordinary.progress).unwrap(), serde_json::to_vec(&observed.progress).unwrap());
        assert_eq!(serde_json::to_vec(&game.state).unwrap(), input);
        for perspective in [Perspective::Operator, Perspective::Side(Side::Axis), Perspective::Side(Side::Commonwealth)]
            .into_iter().chain(cna_core::ids::SeatId::all().map(Perspective::Seat)) {
            assert_eq!(
                serde_json::to_vec(&crate::Cna::dev().view(&c, &ordinary.game.state, perspective)).unwrap(),
                serde_json::to_vec(&crate::Cna::dev().view(&c, &observed.game.state, perspective)).unwrap(),
            );
        }
    }

    #[test]
    fn evaluated_failure_is_generic_with_unknown_candidate_and_input_rng_purity() {
        use cna_core::engine::{Command, EngineError, Rejection, evaluate, evaluate_diagnosed};
        let (c, game) = diagnosed_window_game(true);
        let input = serde_json::to_vec(&game.state).unwrap();
        let rng = serde_json::to_vec(&game.rng).unwrap();
        let ordinary = evaluate(&crate::Cna::dev(), &c, &game, &Command::Advance).unwrap_err();
        let (observed, payload) = evaluate_diagnosed(&crate::Cna::dev(), &c, &game, &Command::Advance);
        let observed = observed.unwrap_err();
        assert_eq!(ordinary, observed);
        assert_eq!(ordinary, Rejection::Engine(EngineError::Unsupported {
            case: "land:21.43".into(), detail: "breakdown allocation could not be resolved".into(),
        }));
        let record: serde_json::Value = serde_json::from_slice(payload.unwrap().as_bytes()).unwrap();
        assert!(record["representative_attempt_reason"].is_null());
        assert_eq!(record["candidate_available"], false);
        assert_eq!(record["candidate_complete"], false);
        assert_eq!(record["complete_explanation"], false);
        assert_eq!(record["result"], "unresolved");
        assert_eq!(serde_json::to_vec(&game.state).unwrap(), input);
        assert_eq!(serde_json::to_vec(&game.rng).unwrap(), rng);
    }

    #[test]
    fn evaluated_passenger_input_failure_captures_actual_hook_and_preserves_game() {
        use cna_core::engine::{Command, EngineError, Rejection, evaluate, evaluate_diagnosed};
        use cna_tables::Bound;
        let (mut c, game) = diagnosed_window_game(false);
        // Deliberately malformed SYNTHETIC chart input reaches the real
        // passenger_obligation guard through the unchanged evaluator/planner.
        // This is not a legal campaign, captured failure or hand-built Attempt.
        let path = cna_content::repo_data_dir().join("tables/airlog/54.2-truck-characteristics.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("capacity_inf_toe_halves = 2").count(), 1);
        let text = text.replace("capacity_inf_toe_halves = 2", "capacity_inf_toe_halves = -2");
        c.tables.airlog.truck_characteristics = cna_tables::airlog::trucks::TruckTable::from_raw(
            &cna_tables::RawTable::parse(&path, &text).unwrap(),
        ).unwrap();
        let input = serde_json::to_vec(&game.state).unwrap();
        let rng = serde_json::to_vec(&game.rng).unwrap();
        let ordinary = evaluate(&crate::Cna::dev(), &c, &game, &Command::Advance).unwrap_err();
        let (observed, payload) = evaluate_diagnosed(&crate::Cna::dev(), &c, &game, &Command::Advance);
        assert_eq!(ordinary, observed.unwrap_err());
        assert_eq!(ordinary, Rejection::Engine(EngineError::Unsupported {
            case: "land:21.43".into(), detail: "breakdown allocation could not be resolved".into(),
        }));
        let record: serde_json::Value = serde_json::from_slice(payload.unwrap().as_bytes()).unwrap();
        assert_eq!(record["representative_attempt_reason"], "passenger/invalid_input");
        assert_eq!(record["input_available"], true);
        assert_eq!(record["candidate_available"], true);
        assert_eq!(record["candidate_complete"], false);
        assert_eq!(record["complete_explanation"], false);
        assert_eq!(record["truncated"], false);
        let rows = record["rows"].as_array().unwrap();
        let input_row = rows.iter().find(|row| row["family"] == "passenger_input").unwrap();
        assert_eq!(input_row["unit"], "cw.2_nz_div.19th_nz_bn");
        assert_eq!(input_row["strength_toe"], 6);
        assert_eq!(input_row["old_transport"]["native_points_lmh"], serde_json::json!([0, 2, 0]));
        assert!(input_row["carried_toe"].is_null());
        assert!(input_row["walking_toe"].is_null());
        let candidate = rows.iter().find(|row| row["family"] == "passenger_candidate").unwrap();
        // Only partition transport tuples were reached; the failed helper did
        // not produce passenger accounting. Never convert those unknowns to 0.
        assert!(!candidate["working_transport"].is_null());
        assert!(!candidate["broken_transport"].is_null());
        for field in ["unresolved_toe", "broken_passengers_toe", "broken_capacity_toe"] {
            assert!(candidate[field].is_null());
        }
        assert_eq!(serde_json::to_vec(&game.state).unwrap(), input);
        assert_eq!(serde_json::to_vec(&game.rng).unwrap(), rng);
    }

    #[test]
    fn paired_hidden_asset_prefix_changes_only_operator_private_capture() {
        use cna_core::engine::{Command, evaluate_diagnosed};
        use cna_protocol::Side;
        let (c, a) = diagnosed_window_game(true);
        let mut b = cna_core::engine::Game { state: a.state.clone(), rng: a.rng.clone() };
        b.state.land.breakdown.window.outcomes.front_mut().unwrap().group.assets[0].points = 4;
        // Only a hidden synthetic queued check differs, not public geometry,
        // status, total unit strength or any accepted command/transcript.
        crate::testkit::assert_indistinguishable(&crate::Cna::dev(), &c, &a.state, &b.state, Side::Axis);
        crate::testkit::assert_action_indistinguishable(&crate::Cna::dev(), &c, &a, &b, &Command::Advance, Side::Axis);
        let (ar, ap) = evaluate_diagnosed(&crate::Cna::dev(), &c, &a, &Command::Advance);
        let (br, bp) = evaluate_diagnosed(&crate::Cna::dev(), &c, &b, &Command::Advance);
        assert_eq!(ar.unwrap_err(), br.unwrap_err());
        assert_ne!(ap.unwrap().as_bytes(), bp.unwrap().as_bytes());
        // HTTP/WS/transcripts/DB rows are tested by the joint Server owner diff;
        // this Rules test certifies no network/persistence surface by itself.
    }

    /// Small ordinary boundary control, not captured campaign history or RNG.
    #[test]
    fn sufficient_medium_carriage_preserves_broken_passenger_accounting() {
        let (c, mut s, mut outcome) = captured_quantities_synthetic_context();
        let id = outcome.group.assets[0].unit.clone();
        s.land.units.get_mut(&id).unwrap().toe = Some(cna_content::units::Toe::Under { under: 2 });
        let u = s.land.units.get_mut(&id).unwrap();
        u.trucks = Trucks {
            medium: 2,
            ..Trucks::default()
        };
        u.transport_trucks = u.trucks;
        s.logistics
            .unit_supply
            .insert(id.clone(), Default::default());
        outcome.group.assets = vec![Asset {
            unit: id.clone(),
            equipment: super::super::Equipment::MediumTruck,
            points: 2,
            cohort: None,
        }];
        outcome.broken = 1;
        outcome.percent = 50;
        let p = plan(&c, &s, &outcome).unwrap();
        assert_eq!(p.partitions[0].destination_passengers, 1);
        losses::apply(&c, &mut s, &outcome, &p).unwrap();
        assert_eq!(crate::land::formation::strength(&c, &s, &id), 1);
        assert_eq!(
            s.land
                .breakdown
                .markers
                .values()
                .map(|m| m.passengers.get(&id).copied().unwrap_or(0))
                .sum::<i32>(),
            1
        );
        assert_source_unit_toe_conserved(&c, &s, &id, 2);
    }

    /// Existing land-0028 stays controlling; walkers never become unresolved merely for having no seat.
    #[test]
    fn half_point_split_separates_carried_unresolved_from_preexisting_walkers() {
        for (strength, expected_walking) in [(1, 0), (3, 2)] {
            let (c, mut s, mut outcome) = captured_quantities_synthetic_context();
            let id = outcome.group.assets[0].unit.clone();
            let u = s.land.units.get_mut(&id).unwrap();
            u.toe = Some(cna_content::units::Toe::Under { under: strength });
            u.trucks = Trucks {
                light: 2,
                ..Trucks::default()
            };
            u.transport_trucks = u.trucks;
            s.logistics
                .unit_supply
                .insert(id.clone(), Default::default());
            outcome.group.assets = vec![Asset {
                unit: id.clone(),
                equipment: super::super::Equipment::LightTruck,
                points: 2,
                cohort: None,
            }];
            outcome.broken = 1;
            outcome.percent = 50;
            let o = losses::passenger_obligation(&c, strength, s.land.units[&id].transport_trucks)
                .unwrap();
            assert_eq!((o.carried, o.walking), (1, expected_walking));
            let p = plan(&c, &s, &outcome).unwrap();
            assert_eq!(
                p.partitions[0].origin_passengers + p.partitions[0].destination_passengers,
                0
            );
            losses::apply(&c, &mut s, &outcome, &p).unwrap();
            assert_eq!(
                crate::land::formation::strength(&c, &s, &id),
                expected_walking
            );
            assert_eq!(s.land.breakdown.unresolved_passengers[&id][0].points, 1);
            assert_source_unit_toe_conserved(&c, &s, &id, strength);
        }
    }

    /// Rules407: an unseated broken passenger must reject without ANY State byte change.
    /// Cases: land:21.43, land:21.45
    /// Interpretations: interp:land-0028, interp:land-0041 (provisional).
    #[test]
    fn partial_carriage_rejects_unseated_broken_passenger_without_state_change() {
        let (c, mut s, outcome) = captured_quantities_synthetic_context();
        let id = outcome.group.assets[0].unit.clone();
        let mut proposed = plan(&c, &s, &outcome).expect("ordinary valid partial carriage plan");
        assert_eq!(proposed.partitions.len(), 1);
        assert_eq!(proposed.partitions[0].unit, id);
        assert_eq!(
            proposed.partitions[0].origin_transport.total()
                + proposed.partitions[0].destination_transport.total(),
            0
        );
        assert_eq!(
            proposed.partitions[0].origin_passengers
                + proposed.partitions[0].destination_passengers,
            0
        );
        // Exactly one malformed field: one passenger has no broken transport seat.
        proposed.partitions[0].destination_passengers = 1;
        assert_source_unit_toe_conserved(&c, &s, &id, 6);
        let before = serde_json::to_vec(&s).unwrap();
        let rejected = losses::apply(&c, &mut s, &outcome, &proposed).unwrap_err();
        assert!(
            matches!(rejected, cna_core::engine::Rejection::Illegal { message }
        if message == "passenger allocation exceeds transported infantry")
        );
        assert_eq!(serde_json::to_vec(&s).unwrap(), before);
        assert_source_unit_toe_conserved(&c, &s, &id, 6);
    }
}
