//! Explicit divisions preserve physical trucks, cargo and historical payments.
use super::{breakdown, formation};
use crate::{CnaContent, State, logistics, ownership, steps::illegal};
use cna_content::{scenario::Supplies, units::Trucks};
use cna_core::{
    engine::{EngineError, Rejection},
    ids::UnitId,
    quantity::{FuelTenths, WaterPoints},
};
use logistics::{CargoPacking, FuelCohortSelection, FuelTruckKind, SupplyError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Division {
    pub transfers: Vec<Transfer>,
    pub allocations: Vec<Allocation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transfer {
    pub from: UnitId,
    pub to: UnitId,
    pub cohorts: Vec<FuelCohortSelection>,
    pub cargo: CargoPacking,
    pub tank_fuel_tenths: i32,
    pub activity_water_points: i32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub unit: UnitId,
    pub transport: Trucks,
    pub packing: CargoPacking,
}
fn supply(error: SupplyError, strict: bool) -> Rejection {
    match error {
        SupplyError::Unsupported { case } => Rejection::Engine(EngineError::Unsupported {
            case: case.into(),
            detail: "truck division needs unresolved equipment data".into(),
        }),
        SupplyError::UnknownFuelRate if strict => Rejection::Engine(EngineError::Unsupported {
            case: "airlog:49.12".into(),
            detail: "truck division fuel capacity is unknown (units-0005)".into(),
        }),
        SupplyError::UnknownFuelRate => {
            illegal("truck division fuel capacity is unknown (units-0005)")
        }
        _ => illegal("truck division exceeds its own trucks, cargo or capacity"),
    }
}
fn arithmetic() -> Rejection {
    illegal("invalid truck division quantities")
}
fn transfer_points(a: &mut i32, b: &mut i32, n: i32) -> Result<(), Rejection> {
    if n < 0 || n > *a {
        return Err(arithmetic());
    }
    *a -= n;
    *b = b.checked_add(n).ok_or_else(arithmetic)?;
    Ok(())
}
fn transfer_goods(a: &mut Supplies, b: &mut Supplies, goods: Supplies) -> Result<(), Rejection> {
    transfer_points(&mut a.ammo, &mut b.ammo, goods.ammo)?;
    transfer_points(&mut a.fuel, &mut b.fuel, goods.fuel)?;
    transfer_points(&mut a.stores, &mut b.stores, goods.stores)?;
    transfer_points(&mut a.water, &mut b.water, goods.water)
}
fn apply_transfer(
    c: &CnaContent,
    draft: &mut State,
    t: &Transfer,
    strict: bool,
) -> Result<(), Rejection> {
    let cohorts = breakdown::cohorts::ensure(draft, &t.from).map_err(Rejection::Engine)?;
    breakdown::cohorts::ensure(draft, &t.to).map_err(Rejection::Engine)?;
    let mut selected = Trucks::default();
    let mut seen = BTreeSet::new();
    for choice in &t.cohorts {
        if choice.count <= 0 || !seen.insert(&choice.id) {
            return Err(arithmetic());
        }
        let cohort = cohorts
            .iter()
            .find(|g| g.id == choice.id && g.count >= choice.count)
            .ok_or_else(arithmetic)?;
        let count = match cohort.kind {
            FuelTruckKind::Light => &mut selected.light,
            FuelTruckKind::Medium => &mut selected.medium,
            FuelTruckKind::Heavy => &mut selected.heavy,
        };
        *count = count.checked_add(choice.count).ok_or_else(arithmetic)?;
    }
    let cargo = t.cargo.totals().map_err(|e| supply(e, strict))?;
    logistics::validate_packing(c, &selected, &Trucks::default(), &cargo, &t.cargo)
        .map_err(|e| supply(e, strict))?;
    let fuel_capacity = [
        (selected.light, cna_tables::airlog::trucks::TruckType::Light),
        (
            selected.medium,
            cna_tables::airlog::trucks::TruckType::Medium,
        ),
        (selected.heavy, cna_tables::airlog::trucks::TruckType::Heavy),
    ]
    .into_iter()
    .try_fold(0i64, |sum, (n, kind)| {
        sum.checked_add(
            i64::from(n)
                * i64::from(
                    c.tables
                        .airlog
                        .truck_characteristics
                        .truck(kind)
                        .fuel_capacity_points,
                )
                * 10,
        )
    })
    .ok_or_else(arithmetic)?;
    if i64::from(t.tank_fuel_tenths) > fuel_capacity {
        return Err(arithmetic());
    }
    logistics::transfer_activity_water_credit(c, draft, &t.from, &t.to, selected)
        .map_err(|e| supply(e, strict))?;
    let moved =
        logistics::transfer_selected_segment_fuel_cohorts(draft, &t.from, &t.to, &t.cohorts)
            .map_err(|e| supply(e, strict))?;
    breakdown::cohorts::inherit(draft, &moved).map_err(Rejection::Engine)?;
    let mut a = draft.land.units[&t.from].trucks;
    let mut b = draft.land.units[&t.to].trucks;
    transfer_points(&mut a.light, &mut b.light, selected.light)?;
    transfer_points(&mut a.medium, &mut b.medium, selected.medium)?;
    transfer_points(&mut a.heavy, &mut b.heavy, selected.heavy)?;
    draft.land.units.get_mut(&t.from).unwrap().trucks = a;
    draft.land.units.get_mut(&t.to).unwrap().trucks = b;
    let mut a = draft
        .logistics
        .unit_supply
        .get(&t.from)
        .cloned()
        .unwrap_or_default();
    let mut b = draft
        .logistics
        .unit_supply
        .get(&t.to)
        .cloned()
        .unwrap_or_default();
    transfer_goods(&mut a.carried, &mut b.carried, cargo)?;
    let (mut af, mut bf) = (a.tank_fuel.get(), b.tank_fuel.get());
    transfer_points(&mut af, &mut bf, t.tank_fuel_tenths)?;
    a.tank_fuel = FuelTenths::new(af);
    b.tank_fuel = FuelTenths::new(bf);
    let (mut aw, mut bw) = (a.activity_water.get(), b.activity_water.get());
    transfer_points(&mut aw, &mut bw, t.activity_water_points)?;
    a.activity_water = WaterPoints::new(aw);
    b.activity_water = WaterPoints::new(bw);
    draft.logistics.unit_supply.insert(t.from.clone(), a);
    draft.logistics.unit_supply.insert(t.to.clone(), b);
    Ok(())
}

/// The highest co-located parent owns its represented family's trucks.
/// Cases: land:8.56, land:8.96
pub fn family(c: &CnaContent, s: &State, reactor: &UnitId) -> Vec<UnitId> {
    let Some(unit) = s.land.units.get(reactor) else {
        return vec![];
    };
    let mut root = reactor;
    let mut seen = BTreeSet::new();
    while seen.insert(root) {
        let Some(parent) = ownership::parent_for_unit(c, s, root) else {
            break;
        };
        if !s
            .land
            .units
            .get(parent)
            .is_some_and(|p| p.side == unit.side && p.location == unit.location)
        {
            break;
        }
        root = parent;
    }
    formation::members(c, s, root)
}
/// Preview only own equipment. The caller separately checks the eligibility saved at the interrupt.
/// A division is transactional, including shared fuel rounding accounts and paid water credit.
/// Cases: land:8.56, land:8.91, land:8.92, land:8.95, land:8.96, land:8.97, airlog:49.16, airlog:52.42, airlog:54.2
/// Interpretations: interp:land-0005, interp:airlog-0015, interp:airlog-0018
pub fn preview_reaction_division(
    c: &CnaContent,
    s: &State,
    reactor: &UnitId,
    division: &Division,
    strict: bool,
) -> Result<State, Rejection> {
    let parent = ownership::parent_for_unit(c, s, reactor)
        .ok_or_else(|| illegal("only an attached reacting component can divide parent trucks"))?;
    let unit = s.land.units.get(reactor).ok_or_else(arithmetic)?;
    if unit.location.hex().is_none() || s.land.units[parent].location != unit.location {
        return Err(illegal("reaction trucks must be with their parent"));
    }
    let family: BTreeSet<_> = family(c, s, reactor).into_iter().collect();
    let mut allocations = BTreeMap::new();
    for a in &division.allocations {
        if !family.contains(&a.unit) || allocations.insert(a.unit.clone(), a).is_some() {
            return Err(illegal(
                "truck allocations must name each affected family unit once",
            ));
        }
    }
    let mut changed = BTreeSet::new();
    let mut draft = s.clone();
    for t in &division.transfers {
        if t.from == t.to
            || !family.contains(&t.from)
            || !family.contains(&t.to)
            || t.cohorts.is_empty()
        {
            return Err(illegal(
                "reaction truck transfers must stay within the attached family",
            ));
        }
        apply_transfer(c, &mut draft, t, strict)?;
        changed.extend([t.from.clone(), t.to.clone()]);
    }
    changed.extend(allocations.keys().cloned());
    for id in changed {
        let a = allocations.get(&id).ok_or_else(|| {
            illegal("provide final transport and cargo packing for every changed unit")
        })?;
        let u = draft.land.units.get_mut(&id).unwrap();
        if s.land.units[&id].cohesion_quarters <= -20
            && s.land.units[&id].trucks.total() > 0
            && u.trucks.total() == 0
        {
            return Err(illegal(
                "a unit at cohesion -5 or worse must retain trucks (land:8.97)",
            ));
        }
        u.transport_trucks = a.transport;
        let stock = draft
            .logistics
            .unit_supply
            .get(&id)
            .cloned()
            .unwrap_or_default();
        logistics::validate_packing(
            c,
            &u.trucks,
            &u.transport_trucks,
            &stock.carried,
            &a.packing,
        )
        .map_err(|e| supply(e, strict))?;
        if formation::individual_allowance(c, &draft, &id).is_none() {
            return Err(illegal("transport assignment is unresolved"));
        }
        let capacity = logistics::fuel_capacity(c, &draft, &id).map_err(|e| supply(e, strict))?;
        if stock.tank_fuel.get() < 0 || stock.tank_fuel.get() > capacity.get() {
            return Err(illegal(
                "final tank fuel exceeds remaining vehicle capacity",
            ));
        }
    }
    Ok(draft)
}

mod planning;
pub use planning::reachable_divisions;

#[cfg(test)]
mod tests;
