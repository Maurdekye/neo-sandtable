//! First homogeneous Unit-origin truck return, including one real physical cohort.
//! Funded destination merges remain Unsupported; missing history never certifies zero CP.
use super::*;
use crate::land::breakdown::markers::BrokenMarker;
use crate::logistics::{capacity, cargo_history::{self, CargoSite, LotSelection, Parcel}};
use crate::logistics::{FuelTruckKind, TruckFuelCohort, cargo_history::motion::PhysicalTrucks};
use cna_content::units::Trucks;
use cna_core::quantity::WaterPoints;
use cna_tables::airlog::trucks::TruckType;

pub(super) fn kind(equipment: &Equipment) -> Option<TruckType> {
    match equipment {
        Equipment::LightTruck => Some(TruckType::Light),
        Equipment::MediumTruck => Some(TruckType::Medium),
        Equipment::HeavyTruck => Some(TruckType::Heavy),
        _ => None,
    }
}
fn counts(kind: TruckType, points: i32) -> Trucks {
    match kind {
        TruckType::Light => Trucks { light: points, ..Default::default() },
        TruckType::Medium => Trucks { medium: points, ..Default::default() },
        TruckType::Heavy => Trucks { heavy: points, ..Default::default() },
    }
}
fn get(goods: Supplies, component: usize) -> i32 {
    [goods.ammo, goods.fuel, goods.stores, goods.water][component]
}
fn set(goods: &mut Supplies, component: usize, amount: i32) {
    match component { 0 => goods.ammo = amount, 1 => goods.fuel = amount,
        2 => goods.stores = amount, _ => goods.water = amount }
}
fn sum(a: Supplies, b: Supplies) -> Result<Supplies, Rejection> {
    let mut result = Supplies::default();
    for component in 0..4 { set(&mut result, component, checked_add(get(a, component), get(b, component))?); }
    Ok(result)
}
fn subtract(a: Supplies, b: Supplies) -> Result<Supplies, Rejection> {
    let mut result = Supplies::default();
    for component in 0..4 {
        let value = get(a, component).checked_sub(get(b, component))
            .filter(|n| *n >= 0).ok_or_else(|| invariant("truck cargo partition is not conserved"))?;
        set(&mut result, component, value);
    }
    Ok(result)
}

/// Complete pre-dice lot priority, copied from owner-known current parcels.
/// Cases: land:22.23, land:22.5, airlog:53.25
pub(super) fn priority(s: &State, marker: &BrokenMarker, selected: &[LotSelection]) -> Result<Vec<LotSelection>, Rejection> {
    let mut available = cargo_history::parcels(s, marker.side, &CargoSite::BrokenMarker(marker.id.clone()))
        .map_err(cargo_history::rejection)?;
    if available.iter().any(|p| [p.goods.ammo, p.goods.fuel, p.goods.stores, p.goods.water].into_iter().filter(|n| *n > 0).count() > 1) {
        return Err(gap("land:22.5", "mixed-supply parcels need an explicit within-parcel priority adapter"));
    }
    available.sort_by(|a, b| a.lot.cmp(&b.lot));
    if selected.is_empty() {
        return Ok(available.into_iter().map(|p| LotSelection { lot: p.lot, goods: p.goods }).collect());
    }
    let mut seen = BTreeSet::new();
    if selected.len() != available.len() || selected.iter().any(|row|
        !seen.insert(&row.lot) || !available.iter().any(|p| p.lot == row.lot && p.goods == row.goods)) {
        return Err(illegal("truck cargo priority must name every current parcel exactly once"));
    }
    Ok(selected.to_vec())
}

fn target<'a>(s: &'a State, marker: &BrokenMarker, choice: &ReturnPoolTarget) -> Result<Option<&'a crate::state::TruckPool>, Rejection> {
    let ReturnPoolTarget::Existing { pool } = choice else { return Ok(None); };
    let mut matches = s.logistics.truck_pools.iter().filter(|p| &p.id == pool);
    let row = matches.next().ok_or_else(|| illegal("unknown own truck return pool"))?;
    if matches.next().is_some() || row.side != marker.side || row.location.as_ref().and_then(Location::hex) != Some(&marker.hex) {
        return Err(illegal("truck return pool must be owned and at the repair site"));
    }
    if row.box_handling.is_some() || s.logistics.pool_fuel_accounts.contains_key(pool)
        || s.logistics.pool_fuel_segments.contains_key(pool)
        || s.logistics.cargo_history.motion.entries.iter().any(|e| e.site == CargoSite::Pool(pool.clone())) {
        return Err(gap("land:22.5", "truck return into a tracked or funded pool requires its cohort merge adapter"));
    }
    if [row.trucks.light, row.trucks.medium, row.trucks.heavy, row.activity_water.get(), row.tank_fuel.get()].iter().any(|n| *n < 0) {
        return Err(invariant("negative destination truck holdings").into());
    }
    Ok(Some(row))
}

fn physical(marker: &BrokenMarker) -> Result<Option<&TruckFuelCohort>, Rejection> {
    let selected = &marker.assets[0];
    if marker.source_pool.is_some() || !marker.pool_fuel_cohorts.is_empty() || marker.fuel_cohorts.len() > 1 {
        return Err(gap("land:22.5", "mixed or pool-origin truck cohort return is not implemented"));
    }
    match (selected.cohort.as_deref(), marker.fuel_cohorts.first()) {
        (None, None) => Ok(None),
        (Some(id), Some(row)) => {
            let expected = match kind(&selected.equipment) {
                Some(TruckType::Light) => FuelTruckKind::Light,
                Some(TruckType::Medium) => FuelTruckKind::Medium,
                Some(TruckType::Heavy) => FuelTruckKind::Heavy,
                None => return Err(invariant("nontruck physical cohort").into()),
            };
            if id.is_empty() || id != row.id || row.parent.as_deref() == Some(id)
                || row.kind != expected || row.count != selected.points || row.cp_quarters < 0
                || row.account != selected.unit {
                return Err(invariant("truck physical cohort does not match its exact body").into());
            }
            Ok(Some(row))
        },
        _ => Err(gap("land:22.5", "truck cohort identity is incomplete")),
    }
}

struct Partition {
    after: BrokenMarker,
    trucks: Trucks,
    cargo: Supplies,
    parcels: Vec<Parcel>,
    lots: Vec<LotSelection>,
    water: WaterPoints,
    lost_fuel: i32,
}
fn repaired(kind: TruckType, points: i32, budget: i32) -> i32 {
    points.min(if kind == TruckType::Heavy { budget / 2 } else { budget })
}

fn partition(c: &CnaContent, s: &State, marker: &BrokenMarker, priority: &[LotSelection], points: i32) -> Result<Partition, Rejection> {
    let kind = kind(&marker.assets[0].equipment).ok_or_else(|| invariant("nontruck partition"))?;
    let trucks = counts(kind, points);
    let remaining = counts(kind, marker.assets[0].points - points);
    let available = cargo_history::parcels(s, marker.side, &CargoSite::BrokenMarker(marker.id.clone()))
        .map_err(cargo_history::rejection)?;
    let mut cargo = Supplies::default();
    let mut parcels = vec![];
    let mut lots = vec![];
    for choice in priority {
        let parcel = available.iter().find(|p| p.lot == choice.lot && p.goods == choice.goods)
            .ok_or_else(|| illegal("truck priority no longer matches its owner parcel"))?;
        let mut taken = Supplies::default();
        for component in 0..4 {
            let mut low = 0;
            let mut high = get(choice.goods, component);
            while low < high {
                let mid = low + (high - low) / 2 + 1;
                let mut proposed = taken;
                set(&mut proposed, component, mid);
                let total = sum(cargo, proposed)?;
                if capacity::find_packing(c, &trucks, &Trucks::default(), total).is_some() { low = mid; }
                else { high = mid - 1; }
            }
            set(&mut taken, component, low);
        }
        if taken != Supplies::default() {
            cargo = sum(cargo, taken)?;
            let mut moved = parcel.clone(); moved.goods = taken;
            parcels.push(moved);
            lots.push(LotSelection { lot: choice.lot.clone(), goods: taken });
        }
    }
    let mut after = marker.clone();
    after.assets[0].points -= points;
    after.assets.retain(|a| a.points > 0);
    if let Some(cohort) = after.fuel_cohorts.first_mut() { cohort.count -= points; }
    after.fuel_cohorts.retain(|cohort| cohort.count > 0);
    // Old paid-water fields stay audit-only on the retained marker; none of
    // these facts are copied into a returned pool or its activity entitlement.
    after.transport = Trucks { light: marker.transport.light.min(remaining.light),
        medium: marker.transport.medium.min(remaining.medium), heavy: marker.transport.heavy.min(remaining.heavy) };
    let residual = subtract(marker.cargo.totals().map_err(supply)?, cargo)?;
    after.cargo = capacity::find_packing(c, &remaining, &after.transport, residual)
        .ok_or_else(|| illegal("truck priority cannot preserve the failed marker's cargo capacity"))?;
    let water = if marker.activity_water.get() == 0 { 0 } else { points };
    after.activity_water = WaterPoints::new(marker.activity_water.get() - water);
    let per = c.tables.airlog.truck_characteristics.truck(kind).fuel_capacity_points;
    let cap = i64::from(per).checked_mul(10).and_then(|n| n.checked_mul(i64::from(marker.assets[0].points - points)))
        .and_then(|n| i32::try_from(n).ok()).filter(|n| *n >= 0).ok_or_else(|| invariant("truck tank capacity overflow"))?;
    after.tank_fuel = FuelTenths::new(marker.tank_fuel.get().min(cap));
    Ok(Partition { lost_fuel: marker.tank_fuel.get() - after.tank_fuel.get(), after,
        trucks, cargo, parcels, lots, water: WaterPoints::new(water) })
}

/// Derive all possible output partitions before dice; all inputs are own knowledge.
/// Cases: land:22.11, land:22.14, land:22.23, land:22.5
pub(super) fn validate(c: &CnaContent, s: &State, marker: &BrokenMarker, choice: &RepairMarkerPlan) -> Result<(), Rejection> {
    let cohort = physical(marker)?;
    if [marker.paid_truck_water.light, marker.paid_truck_water.medium, marker.paid_truck_water.heavy].iter().any(|n| *n < 0) {
        return Err(invariant("negative truck paid-water audit").into());
    }
    if marker.passengers.values().any(|n| *n <= 0) || marker.tank_fuel.get() < 0 || marker.activity_water.get() < 0 {
        return Err(invariant("invalid truck passengers or reserves").into());
    }
    let kind = kind(&marker.assets[0].equipment).ok_or_else(|| invariant("unexpected truck kind"))?;
    let body = counts(kind, marker.assets[0].points);
    let goods = marker.cargo.totals().map_err(supply)?;
    capacity::validate_packing(c, &body, &marker.transport, &goods, &marker.cargo).map_err(supply)?;
    let tank_capacity = i64::from(c.tables.airlog.truck_characteristics.truck(kind).fuel_capacity_points)
        .checked_mul(10).and_then(|n| n.checked_mul(i64::from(marker.assets[0].points)))
        .ok_or_else(|| invariant("original truck tank capacity overflow"))?;
    if i64::from(marker.tank_fuel.get()) > tank_capacity {
        return Err(invariant("truck marker exceeds its actual tank capacity").into());
    }
    // This first homogeneous body has either no reserve or a complete normal
    // one-point-per-truck reserve. Partial/crew/hot reserve splits stay explicit.
    if marker.activity_water.get() != 0 && (marker.activity_water.get() != marker.assets[0].points
        || logistics::weather::at_hex(c, s, &marker.hex)? != WeatherKind::Normal) {
        return Err(gap("land:22.5", "mixed or incomplete truck/crew reserve partition is not implemented"));
    }
    if priority(s, marker, &choice.cargo_priority)? != choice.cargo_priority {
        return Err(illegal("truck cargo priority is incomplete"));
    }
    let destination = target(s, marker, &choice.return_pool)?;
    if let (Some(cohort), Some(destination)) = (cohort, destination) {
        let known = s.logistics.cargo_history.motion.entries.iter().any(|entry|
            entry.site == CargoSite::BrokenMarker(marker.id.clone()) && entry.cohorts.iter().any(|old|
                old.id == cohort.id && old.kind == cohort.kind && old.count == cohort.count));
        if known && destination.trucks != Trucks::default() {
            return Err(gap("land:22.5", "known truck motion merging into a nonempty pool needs its complete cohort adapter"));
        }
    }
    for budget in 0..=2 {
        let points = repaired(kind, choice.assets[0].points, budget);
        let output = partition(c, s, marker, &choice.cargo_priority, points)?;
        if let Some(destination) = destination {
            let trucks = Trucks { light: checked_add(destination.trucks.light, output.trucks.light)?,
                medium: checked_add(destination.trucks.medium, output.trucks.medium)?,
                heavy: checked_add(destination.trucks.heavy, output.trucks.heavy)? };
            let cargo = sum(destination.cargo, output.cargo)?;
            if capacity::find_packing(c, &trucks, &Trucks::default(), cargo).is_none() {
                return Err(illegal("complete merged truck return exceeds pool capacity"));
            }
        }
    }
    Ok(())
}

/// One chart roll for the sole selected truck group at this hex; all writes
/// remain inside the existing whole-State/RNG/events transaction.
/// Cases: land:22.23, land:22.5, airlog:53.25
/// Interpretations: interp:land-0038, interp:land-0052
pub(super) fn resolve(c: &CnaContent, s: &mut State, cx: &mut Cx<'_>, row: &RepairAttempt, choice: &RepairOutputChoice) -> Result<(), Rejection> {
    let marker = s.land.breakdown.markers.get(row.marker()).cloned().ok_or_else(|| invariant("truck marker disappeared"))?;
    let RepairAsset::Unit(selected) = row.asset() else { return Err(invariant("unexpected truck pool asset").into()); };
    let kind = kind(&selected.equipment).ok_or_else(|| invariant("unexpected truck equipment"))?;
    let roll = cx.rng.d6();
    let Some(BrokenRepairResult::TruckSelectionBudget(budget)) = c.tables.land.broken_down_vehicle_repair.field_result(RepairVehicle::Truck, i32::from(roll.value())) else {
        return Err(invariant("field truck chart result missing").into());
    };
    let count = repaired(kind, selected.points, budget);
    if count == 0 {
        cx.emit(EngineEvent::new(Audience::Side(marker.side), GameEvent::Note { text: format!("Own field truck repair {}: die {}, no trucks returned; no pool created.", marker.id, roll.value()) }));
        return Ok(());
    }
    let before = s.clone();
    let selected_physical: Vec<_> = physical(&marker)?.into_iter().map(|cohort| {
        let mut row = PhysicalTrucks::from(cohort); row.count = count; row
    }).collect();
    let output = partition(c, s, &marker, choice.cargo_priority(), count)?;
    let remainder: Vec<_> = output.after.fuel_cohorts.iter().map(PhysicalTrucks::from).collect();
    let returned = ReturnedTrucks { marker: marker.id.clone(), target: choice.target().clone(), trucks: output.trucks,
        physical_cohorts: selected_physical.clone(), cargo: output.cargo, cargo_parcels: output.parcels,
        activity_water: output.water };
    let prepared = PreparedSiteTruckReturn { actor: marker.side, stage: WaterStage::current(s), site: row.site().clone(),
        target: choice.target().clone(), before: s.clone(), returns: vec![returned] };
    let pool = receipts::apply_site_truck_return(c, s, &prepared)?
        .ok_or_else(|| invariant("positive truck return created no pool"))?;
    let keep = !output.after.assets.is_empty() || !output.after.passengers.is_empty()
        || output.after.cargo.totals().map_err(supply)? != Supplies::default();
    if keep { s.land.breakdown.markers.insert(marker.id.clone(), output.after); }
    else { s.land.breakdown.markers.remove(&marker.id); }
    let cargo_moves = if output.cargo == Supplies::default() { vec![] } else {
        vec![RecoveryCargoMove { source: CargoSite::BrokenMarker(marker.id.clone()), destination: CargoSite::Pool(pool.clone()),
            goods: output.cargo, lots: output.lots }]
    };
    let truck_moves = if selected_physical.is_empty() { vec![] } else {
        let serial = match kind { TruckType::Light => 1, TruckType::Medium => 2, TruckType::Heavy => 3 };
        let relocated = selected_physical.iter().map(|row| PhysicalTrucks {
            id: format!("pool.{pool}.fuel-trucks-{serial}"), parent: Some(row.id.clone()),
            kind: row.kind, count: row.count,
        }).collect();
        vec![RecoveryTruckMove { source: CargoSite::BrokenMarker(marker.id.clone()), destination: CargoSite::Pool(pool.clone()),
            selected: selected_physical, remainder, relocated }]
    };
    let transition = ValidatedRepairOrTowOutcome { actor: marker.side, stage: WaterStage::current(s),
        before, after: s.clone(), cargo_moves, truck_moves };
    let relocation = logistics::prepare_recovery_relocation(s, &transition)?;
    logistics::apply_recovery_relocation(s, &relocation)?;
    cx.emit(EngineEvent::new(Audience::Side(marker.side), GameEvent::Note { text: format!(
        "Own field truck repair {}: die {}, returned {} trucks to pool {}; {} tank-fuel tenths lost. Passengers stay on the marker.",
        marker.id, roll.value(), count, pool, output.lost_fuel) }));
    Ok(())
}
