//! Conservative complete partitions for scripted owners; every returned plan validates.
use super::{
    Asset, RolledCheck, balanced_allocation,
    losses::{self, LossPlan, UnitPartition},
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
fn pack_two(
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
        return None;
    }
    Some((pa, pb))
}
/// Keep every broken vehicle together, assigning enough men and reserve fuel to its partition.
/// Failure returns no fabricated allocation; callers must leave the mandatory choice to its owner.
/// Cases: land:21.35, land:21.36, land:21.41, land:21.43
pub fn plan(c: &CnaContent, s: &State, outcome: &RolledCheck) -> Option<LossPlan> {
    let losses = balanced_allocation(&outcome.group.assets, outcome.broken)?;
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
        let working = sub(old.trucks, lost)?;
        let transport = Trucks {
            light: old.transport_trucks.light.min(lost.light),
            medium: old.transport_trucks.medium.min(lost.medium),
            heavy: old.transport_trucks.heavy.min(lost.heavy),
        };
        let working_transport = sub(old.transport_trucks, transport)?;
        let strength = super::super::formation::strength(c, s, &id);
        let passengers = if old.transport_trucks.total() > 0
            && super::super::formation::class(c, &id).is_some_and(|k| k.unit_type == "infantry")
        {
            (strength - capacity(c, working_transport)).max(0)
        } else {
            0
        };
        if passengers > capacity(c, transport) {
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
        if passengers > 0 {
            u.toe = Some(cna_content::units::Toe::Under {
                under: strength - passengers,
            });
        }
        if let Some(cna_content::units::Toe::Weapons(ws)) = &mut u.toe {
            for a in &broken {
                if let super::Equipment::Weapon(w) = &a.equipment {
                    ws.iter_mut().find(|p| &p.weapon == w)?.n -= a.points;
                }
            }
        }
        let cap = logistics::capacity::fuel_capacity(c, &draft, &id)
            .ok()?
            .get();
        let broken_fuel = (stock.tank_fuel.get() - cap).max(0);
        if broken_fuel > losses::marker_fuel_capacity(c, &broken).ok()? {
            return None;
        }
        let origin = outcome.require_origin.contains(&id);
        partitions.push(UnitPartition {
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
        });
    }
    let plan = LossPlan {
        losses,
        at_origin,
        partitions,
    };
    losses::apply(c, &mut s.clone(), outcome, &plan).ok()?;
    Some(plan)
}
