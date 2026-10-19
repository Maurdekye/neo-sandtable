//! Trusted repair prepayment and postphysical cargo/cohort history relocation.
//! Complete-attempt supply prepayment; private snapshots never enter an event.
use super::cargo_history::{
    self, CargoHistory, CargoHistoryState, CargoLot, CargoSite,
    motion::{MotionEntry, PhysicalTrucks, TruckMotion},
};
use super::{SupplyDemand, SupplyDraw, SupplyError, supply, water::WaterStage};
use crate::land::{
    breakdown::Equipment,
    recovery::payloads::{PreparedRepair, RepairAsset, ValidatedRepairOrTowOutcome},
};
use crate::{CnaContent, State, state::Location};
use cna_content::{scenario::Supplies, units::Trucks};
use cna_core::{
    engine::EngineError,
    ids::{HexId, UnitId},
};
use cna_protocol::Side;
use cna_tables::land::repair::{RepairCost, RepairLocation, RepairVehicle, VehicleState};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub(crate) enum RecoverySupplyError {
    Supply(SupplyError),
    Canonical(EngineError),
}

fn invariant() -> EngineError {
    EngineError::Invariant {
        detail: "invalid trusted recovery supply footprint".into(),
    }
}
fn canonical(error: EngineError) -> RecoverySupplyError {
    RecoverySupplyError::Canonical(error)
}
fn supply_error(error: SupplyError) -> RecoverySupplyError {
    match error {
        SupplyError::Invalid | SupplyError::UnknownFuelRate => canonical(invariant()),
        other => RecoverySupplyError::Supply(other),
    }
}
fn exact_state(a: &State, b: &State) -> Result<bool, EngineError> {
    // Transient comparison only: no Debug, logging or exported serialized value.
    let a = serde_json::to_value(a).map_err(|_| invariant())?;
    let b = serde_json::to_value(b).map_err(|_| invariant())?;
    Ok(a == b)
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum AssetIdentity {
    Unit(UnitId, Equipment, Option<String>),
    Pool(String, Equipment, String),
}
#[derive(Clone)]
struct AttemptBinding {
    marker: String,
    hex: HexId,
    identity: AssetIdentity,
    points: i32,
    mode: RepairLocation,
    vehicle_state: VehicleState,
    vehicle: RepairVehicle,
    cost: RepairCost,
    demand: SupplyDemand,
}
#[derive(Clone)]
struct SiteBinding {
    hex: HexId,
    demand: SupplyDemand,
    draws: Vec<SupplyDraw>,
}
/// No public constructor, Deserialize or Debug exposing private snapshots/draws.
pub(crate) struct TrustedRepairDebit {
    before: State,
    actor: Side,
    stage: WaterStage,
    attempts: Vec<AttemptBinding>,
    sites: Vec<SiteBinding>,
}

fn row_demand(points: i32, cost: RepairCost) -> Result<SupplyDemand, RecoverySupplyError> {
    if points <= 0 || cost.supplies.fuel < 0 || cost.supplies.stores < 0 {
        return Err(canonical(invariant()));
    }
    // The chart's Fuel Points are canonical stock units; stock_demand converts
    // whole fuel to tenths with the same checked arithmetic used by withdrawals.
    let fuel = points
        .checked_mul(cost.supplies.fuel)
        .ok_or_else(|| canonical(invariant()))?;
    let stores = points
        .checked_mul(cost.supplies.stores)
        .ok_or_else(|| canonical(invariant()))?;
    supply::stock_demand(cna_content::scenario::Supplies {
        fuel,
        stores,
        ammo: 0,
        water: 0,
    })
    .map_err(supply_error)
}

fn checked_marker_points(s: &State, row: &AttemptBinding, actor: Side) -> Result<i32, EngineError> {
    let marker = s
        .land
        .breakdown
        .markers
        .get(&row.marker)
        .ok_or_else(invariant)?;
    if marker.id != row.marker || marker.side != actor || marker.hex != row.hex {
        return Err(invariant());
    }
    // Original unit/pool ownership is not a captor entitlement check. The current
    // acting-owned physical marker is authoritative for this accepted asset.
    let counts: Vec<i32> = match &row.identity {
        AssetIdentity::Unit(unit, equipment, cohort) => marker
            .assets
            .iter()
            .filter(|a| &a.unit == unit && &a.equipment == equipment && &a.cohort == cohort)
            .map(|a| a.points)
            .collect(),
        AssetIdentity::Pool(pool, equipment, cohort) => marker
            .pool_assets
            .iter()
            .filter(|a| &a.pool == pool && &a.equipment == equipment && &a.cohort == cohort)
            .map(|a| a.points)
            .collect(),
    };
    if counts.is_empty() || counts.iter().any(|n| *n <= 0) {
        return Err(invariant());
    }
    counts
        .into_iter()
        .try_fold(0i32, |n, x| n.checked_add(x).ok_or_else(invariant))
}

fn prepare_withdrawal(
    c: &CnaContent,
    s: &State,
    actor: Side,
    stage: WaterStage,
    attempts: &[AttemptBinding],
    sites: &[SiteBinding],
) -> Result<crate::state::LogisticsState, RecoverySupplyError> {
    if WaterStage::current(s) != stage {
        return Err(canonical(invariant()));
    }
    let mut attempted = BTreeMap::<(String, AssetIdentity), i32>::new();
    let mut expected = BTreeMap::<HexId, SupplyDemand>::new();
    for row in attempts {
        if c.map.get(&row.hex).is_none() {
            return Err(canonical(invariant()));
        }
        let actual = c
            .tables
            .land
            .vehicle_repair_supply_costs
            .cost(row.mode, row.vehicle_state, row.vehicle)
            .ok_or_else(|| {
                canonical(EngineError::Unsupported {
                    case: "land:22.15".into(),
                    detail: "required recovery cost content is unavailable".into(),
                })
            })?;
        if actual != row.cost || row_demand(row.points, actual)? != row.demand {
            return Err(canonical(invariant()));
        }
        let key = (row.marker.clone(), row.identity.clone());
        let sum = attempted
            .get(&key)
            .copied()
            .unwrap_or(0)
            .checked_add(row.points)
            .ok_or_else(|| canonical(invariant()))?;
        if sum > checked_marker_points(s, row, actor).map_err(canonical)? {
            return Err(canonical(invariant()));
        }
        attempted.insert(key, sum);
        let old = expected.get(&row.hex).copied().unwrap_or_default();
        expected.insert(
            row.hex.clone(),
            old.checked_add(row.demand).map_err(supply_error)?,
        );
    }
    let mut capacities = BTreeMap::new();
    let mut all_draws = Vec::new();
    let mut total = SupplyDemand::default();
    for site in sites {
        let demand = expected
            .remove(&site.hex)
            .ok_or_else(|| canonical(invariant()))?;
        if demand != site.demand {
            return Err(canonical(invariant()));
        }
        let location = Location::Hex {
            hex: site.hex.clone(),
        };
        // Same canonical source authorization as supply: own first-line cargo,
        // active real own dumps and source-resolved scenario Unlimited only.
        // No fabricated consumer, own-tank assumption or second-line pool access.
        let mut available = supply::local_stocks(s, actor, &location).map_err(supply_error)?;
        supply::add_unlimited_source(c, actor, &location, &mut available).map_err(supply_error)?;
        let available: BTreeMap<_, _> = available
            .into_iter()
            .map(|x| (x.source, x.amount))
            .collect();
        let mut site_total = SupplyDemand::default();
        for draw in &site.draws {
            let capacity = available
                .get(&draw.source)
                .copied()
                .ok_or_else(|| canonical(invariant()))?;
            if let Some(previous) = capacities.insert(draw.source.clone(), capacity)
                && previous != capacity
            {
                return Err(canonical(invariant()));
            }
            site_total = site_total.checked_add(draw.amount).map_err(supply_error)?;
            all_draws.push(draw.clone());
        }
        if site_total != demand {
            return Err(canonical(invariant()));
        }
        total = total.checked_add(demand).map_err(supply_error)?;
    }
    if !expected.is_empty() {
        return Err(canonical(invariant()));
    }
    // ONE canonical withdrawal aggregates shared sources across ALL sites before
    // whole-point fuel rounding. No earlier movement rounding credit is supplied.
    supply::withdraw_draws(
        &s.logistics,
        None,
        total,
        &all_draws,
        &capacities,
        &BTreeMap::new(),
    )
    .map_err(supply_error)
}

/// Validate the complete accepted attempt before the first associated die.
/// Land's trusted producer owns repair/facility/type eligibility, including the
/// special organization requirement: table availability does not grant eligibility.
/// Cases: land:22.23, land:22.24, land:22.26, land:22.35, land:22.42
pub(crate) fn prepare_recovery_prepaid(
    c: &CnaContent,
    s: &State,
    prepared: &PreparedRepair,
) -> Result<TrustedRepairDebit, RecoverySupplyError> {
    if !exact_state(s, prepared.before()).map_err(canonical)? {
        return Err(canonical(invariant()));
    }
    let mut attempts = Vec::new();
    for row in prepared.attempts() {
        let hex = row
            .site()
            .hex()
            .cloned()
            .ok_or_else(|| canonical(invariant()))?;
        let (identity, points) = match row.asset() {
            RepairAsset::Unit(a) => (
                AssetIdentity::Unit(a.unit.clone(), a.equipment.clone(), a.cohort.clone()),
                a.points,
            ),
            RepairAsset::Pool(a) => (
                AssetIdentity::Pool(a.pool.clone(), a.equipment.clone(), a.cohort.clone()),
                a.points,
            ),
        };
        attempts.push(AttemptBinding {
            marker: row.marker().into(),
            hex,
            identity,
            points,
            mode: row.mode(),
            vehicle_state: row.vehicle_state(),
            vehicle: row.vehicle(),
            cost: *row.cost(),
            demand: row.demand(),
        });
    }
    let mut sites = Vec::new();
    for site in prepared.site_debits() {
        sites.push(SiteBinding {
            hex: site
                .site()
                .hex()
                .cloned()
                .ok_or_else(|| canonical(invariant()))?,
            demand: site.demand(),
            draws: site.draws().to_vec(),
        });
    }
    prepare_withdrawal(c, s, prepared.actor(), prepared.stage(), &attempts, &sites)?;
    Ok(TrustedRepairDebit {
        before: s.clone(),
        actor: prepared.actor(),
        stage: prepared.stage(),
        attempts,
        sites,
    })
}

/// Revalidate all-site content and authorization, then atomically publish the one
/// canonical logistics draft. Land calls this before dice on the outer full draft.
/// Cases: land:22.23, land:22.24, land:22.26, land:22.35, land:22.42
pub(crate) fn apply_recovery_prepaid(
    c: &CnaContent,
    s: &mut State,
    prepared: &TrustedRepairDebit,
) -> Result<(), RecoverySupplyError> {
    if !exact_state(s, &prepared.before).map_err(canonical)? {
        return Err(canonical(invariant()));
    }
    let next = prepare_withdrawal(
        c,
        s,
        prepared.actor,
        prepared.stage,
        &prepared.attempts,
        &prepared.sites,
    )?;
    s.logistics = next;
    Ok(())
}

/// Private synchronous witness; neither snapshots nor histories are exported.
pub(crate) struct PreparedRecoveryRelocation {
    before_apply: State,
    histories: CargoHistoryState,
}

fn encoded<T: serde::Serialize>(value: &T) -> Result<serde_json::Value, EngineError> {
    serde_json::to_value(value).map_err(|_| invariant())
}

fn valid_goods(goods: Supplies) -> Result<Supplies, EngineError> {
    if [goods.ammo, goods.fuel, goods.stores, goods.water]
        .iter()
        .any(|n| *n < 0)
    {
        return Err(invariant());
    }
    Ok(goods)
}
fn goods_add(a: Supplies, b: Supplies) -> Result<Supplies, EngineError> {
    valid_goods(a)?;
    valid_goods(b)?;
    Ok(Supplies {
        ammo: a.ammo.checked_add(b.ammo).ok_or_else(invariant)?,
        fuel: a.fuel.checked_add(b.fuel).ok_or_else(invariant)?,
        stores: a.stores.checked_add(b.stores).ok_or_else(invariant)?,
        water: a.water.checked_add(b.water).ok_or_else(invariant)?,
    })
}
fn goods_sub(a: Supplies, b: Supplies) -> Result<Supplies, EngineError> {
    valid_goods(a)?;
    valid_goods(b)?;
    valid_goods(Supplies {
        ammo: a.ammo.checked_sub(b.ammo).ok_or_else(invariant)?,
        fuel: a.fuel.checked_sub(b.fuel).ok_or_else(invariant)?,
        stores: a.stores.checked_sub(b.stores).ok_or_else(invariant)?,
        water: a.water.checked_sub(b.water).ok_or_else(invariant)?,
    })
}
fn goods_empty(goods: Supplies) -> bool {
    goods == Supplies::default()
}

fn add_goods(
    totals: &mut BTreeMap<CargoSite, Supplies>,
    site: &CargoSite,
    amount: Supplies,
) -> Result<(), EngineError> {
    totals.insert(
        site.clone(),
        goods_add(totals.get(site).copied().unwrap_or_default(), amount)?,
    );
    Ok(())
}

fn checked_trucks(trucks: Trucks) -> Result<Trucks, EngineError> {
    if [trucks.light, trucks.medium, trucks.heavy]
        .iter()
        .any(|n| *n < 0)
    {
        return Err(invariant());
    }
    Ok(trucks)
}
fn add_trucks(a: Trucks, b: Trucks) -> Result<Trucks, EngineError> {
    checked_trucks(a)?;
    checked_trucks(b)?;
    Ok(Trucks {
        light: a.light.checked_add(b.light).ok_or_else(invariant)?,
        medium: a.medium.checked_add(b.medium).ok_or_else(invariant)?,
        heavy: a.heavy.checked_add(b.heavy).ok_or_else(invariant)?,
    })
}
fn sub_trucks(a: Trucks, b: Trucks) -> Result<Trucks, EngineError> {
    checked_trucks(a)?;
    checked_trucks(b)?;
    checked_trucks(Trucks {
        light: a.light.checked_sub(b.light).ok_or_else(invariant)?,
        medium: a.medium.checked_sub(b.medium).ok_or_else(invariant)?,
        heavy: a.heavy.checked_sub(b.heavy).ok_or_else(invariant)?,
    })
}
fn physical_counts(cohorts: &[PhysicalTrucks]) -> Result<Trucks, EngineError> {
    let mut ids = BTreeSet::new();
    let mut trucks = Trucks::default();
    for cohort in cohorts {
        if cohort.id.is_empty()
            || cohort.count <= 0
            || cohort.parent.as_ref() == Some(&cohort.id)
            || !ids.insert(&cohort.id)
        {
            return Err(invariant());
        }
        let field = match cohort.kind {
            super::FuelTruckKind::Light => &mut trucks.light,
            super::FuelTruckKind::Medium => &mut trucks.medium,
            super::FuelTruckKind::Heavy => &mut trucks.heavy,
        };
        *field = field.checked_add(cohort.count).ok_or_else(invariant)?;
    }
    Ok(trucks)
}

struct PhysicalSite {
    goods: Supplies,
    trucks: Trucks,
    cohorts: Vec<PhysicalTrucks>,
}
/// A missing source marker is allowed only in AFTER; BEFORE proves its identity.
fn physical_site(
    state: &State,
    site: &CargoSite,
    actor: Side,
) -> Result<Option<PhysicalSite>, EngineError> {
    match site {
        CargoSite::BrokenMarker(id) => {
            let Some(marker) = state.land.breakdown.markers.get(id) else {
                return Ok(None);
            };
            if id.is_empty() || marker.id != *id || marker.side != actor {
                return Err(invariant());
            }
            let mut trucks = Trucks::default();
            for (equipment, points) in marker
                .assets
                .iter()
                .map(|a| (&a.equipment, a.points))
                .chain(marker.pool_assets.iter().map(|a| (&a.equipment, a.points)))
            {
                if points <= 0 {
                    return Err(invariant());
                }
                let field = match equipment {
                    Equipment::LightTruck => Some(&mut trucks.light),
                    Equipment::MediumTruck => Some(&mut trucks.medium),
                    Equipment::HeavyTruck => Some(&mut trucks.heavy),
                    _ => None,
                };
                if let Some(field) = field {
                    *field = field.checked_add(points).ok_or_else(invariant)?;
                }
            }
            let cohorts: Vec<_> = marker
                .fuel_cohorts
                .iter()
                .map(PhysicalTrucks::from)
                .chain(marker.pool_fuel_cohorts.iter().map(PhysicalTrucks::from))
                .collect();
            let counts = physical_counts(&cohorts)?;
            // A legacy untracked marker can exist. It cannot name a transferable
            // physical identity it does not actually contain.
            sub_trucks(trucks, counts)?;
            Ok(Some(PhysicalSite {
                goods: valid_goods(marker.cargo.totals().map_err(|_| invariant())?)?,
                trucks,
                cohorts,
            }))
        }
        CargoSite::Pool(id) => {
            let mut matches = state.logistics.truck_pools.iter().filter(|p| p.id == *id);
            let Some(pool) = matches.next() else {
                return Ok(None);
            };
            if id.is_empty() || matches.next().is_some() || pool.side != actor {
                return Err(invariant());
            }
            Ok(Some(PhysicalSite {
                goods: valid_goods(pool.cargo)?,
                trucks: checked_trucks(pool.trucks)?,
                cohorts: vec![],
            }))
        }
        _ => Err(invariant()),
    }
}

fn history_total(history: &CargoHistory) -> Result<Supplies, EngineError> {
    let mut ids = BTreeSet::new();
    let mut total = Supplies::default();
    for lot in &history.lots {
        if lot.id.is_empty()
            || lot.id == "fresh"
            || !ids.insert(&lot.id)
            || lot.spent_cp_quarters < 0
            || lot.ceiling_cp_quarters <= 0
        {
            return Err(invariant());
        }
        total = goods_add(total, lot.goods)?;
    }
    Ok(total)
}

fn new_lot_id(history: &mut CargoHistoryState, actor: Side) -> Result<String, EngineError> {
    let serial = history
        .next_id
        .get(&actor)
        .copied()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(invariant)?;
    // Same namespace as cargo_history, with explicit collision refusal.
    let id = format!("{}.cargo-{serial}", crate::state::side_key(actor));
    if history
        .histories
        .values()
        .any(|h| h.lots.iter().any(|lot| lot.id == id))
    {
        return Err(invariant());
    }
    history.next_id.insert(actor, serial);
    Ok(id)
}

/// Lead mf46d027a8591: retire only an expired destination entry required by
/// this successful current tagged import. No source or unrelated entry changes.
fn append_imported_lot(
    next: &mut CargoHistoryState,
    destination: CargoSite,
    current: WaterStage,
    incoming_stage: WaterStage,
    lot: CargoLot,
    destination_is_source: bool,
) -> Result<(), EngineError> {
    history_total(&CargoHistory {
        stage: incoming_stage,
        lots: vec![lot.clone()],
    })?;
    if goods_empty(lot.goods) {
        return Err(invariant());
    }
    if let Some(existing) = next.histories.get(&destination) {
        history_total(existing)?;
        if existing.stage != incoming_stage {
            if existing.stage < current && incoming_stage == current && !destination_is_source {
                next.histories.remove(&destination);
            } else {
                return Err(EngineError::Unsupported {
                    case: "airlog:53.25".into(),
                    detail: "recovery cannot merge independent cargo-history stages".into(),
                });
            }
        }
    }
    let dest = next
        .histories
        .entry(destination)
        .or_insert_with(|| CargoHistory {
            stage: incoming_stage,
            lots: vec![],
        });
    if dest.lots.iter().any(|old| old.id == lot.id) {
        return Err(invariant());
    }
    dest.lots.push(lot);
    Ok(())
}

/// Tagged transfers preserve all CP fields. Untagged goods never acquire a lot.
fn cargo_delta(
    outcome: &ValidatedRepairOrTowOutcome,
    next: &mut CargoHistoryState,
) -> Result<(), EngineError> {
    let before = outcome.before();
    let after = outcome.after();
    let stage = outcome.stage();
    let mut outgoing = BTreeMap::new();
    let mut incoming = BTreeMap::new();
    let mut moved = Vec::<(CargoSite, WaterStage, CargoLot)>::new();
    let mut remaining = BTreeMap::<CargoSite, CargoHistory>::new();
    let mut fresh = BTreeMap::<CargoSite, Supplies>::new();
    let mut sources = BTreeSet::new();
    for row in outcome.cargo_moves() {
        if !matches!(row.source(), CargoSite::BrokenMarker(_))
            || row.source() == row.destination()
            || goods_empty(valid_goods(row.goods())?)
        {
            return Err(invariant());
        }
        let source = physical_site(before, row.source(), outcome.actor())?.ok_or_else(invariant)?;
        physical_site(after, row.destination(), outcome.actor())?.ok_or_else(invariant)?;
        sources.insert(row.source().clone());
        if !fresh.contains_key(row.source()) {
            let known = match before.logistics.cargo_history.histories.get(row.source()) {
                Some(h) if h.stage == stage => {
                    let total = history_total(h)?;
                    remaining.insert(row.source().clone(), h.clone());
                    total
                }
                _ => Supplies::default(),
            };
            fresh.insert(row.source().clone(), goods_sub(source.goods, known)?);
        }
        let mut total = Supplies::default();
        let mut seen = BTreeSet::new();
        for choice in row.lots() {
            if choice.lot.is_empty()
                || !seen.insert(&choice.lot)
                || goods_empty(valid_goods(choice.goods)?)
            {
                return Err(invariant());
            }
            total = goods_add(total, choice.goods)?;
            if choice.lot == "fresh" {
                let available = fresh.get_mut(row.source()).ok_or_else(invariant)?;
                *available = goods_sub(*available, choice.goods)?;
            } else {
                let h = remaining.get_mut(row.source()).ok_or_else(invariant)?;
                let lot = h
                    .lots
                    .iter_mut()
                    .find(|lot| lot.id == choice.lot)
                    .ok_or_else(invariant)?;
                if before
                    .logistics
                    .cargo_history
                    .histories
                    .iter()
                    .any(|(site, other)| {
                        site != row.source()
                            && other.lots.iter().any(|existing| existing.id == lot.id)
                    })
                {
                    return Err(invariant());
                }
                let mut carried = lot.clone();
                lot.goods = goods_sub(lot.goods, choice.goods)?;
                carried.goods = choice.goods;
                // Partial parcels need a new physical lot identity, not fresh CP.
                if !goods_empty(lot.goods) {
                    carried.id = new_lot_id(next, outcome.actor())?;
                }
                moved.push((row.destination().clone(), h.stage, carried));
            }
        }
        if total != row.goods() {
            return Err(invariant());
        }
        add_goods(&mut outgoing, row.source(), total)?;
        add_goods(&mut incoming, row.destination(), total)?;
    }
    // Include actual discarded marker cargo even if no trucks succeeded.
    for (id, marker) in &before.land.breakdown.markers {
        if marker.side != outcome.actor() {
            continue;
        }
        let site = CargoSite::BrokenMarker(id.clone());
        let original = physical_site(before, &site, outcome.actor())?.ok_or_else(invariant)?;
        let current = physical_site(after, &site, outcome.actor())?
            .map(|p| p.goods)
            .unwrap_or_default();
        if original.goods != current {
            sources.insert(site);
        }
    }
    for site in sources {
        let original = physical_site(before, &site, outcome.actor())?
            .ok_or_else(invariant)?
            .goods;
        let current = physical_site(after, &site, outcome.actor())?
            .map(|p| p.goods)
            .unwrap_or_default();
        let exported = outgoing.get(&site).copied().unwrap_or_default();
        let imported = incoming.get(&site).copied().unwrap_or_default();
        let retained_original = goods_sub(current, imported)?;
        let discarded = goods_sub(goods_sub(original, exported)?, retained_original)?;
        if let Some(h) = before.logistics.cargo_history.histories.get(&site)
            && h.stage == stage
        {
            history_total(h)?;
            let mut remainder = remaining.remove(&site).unwrap_or_else(|| h.clone());
            remainder.lots.retain(|lot| !goods_empty(lot.goods));
            // The physically validated residual is destruction/excess, distinct
            // from selected exports. Canonical debit retirement supplies its order.
            if !goods_empty(discarded) {
                cargo_history::retire_debit_entry(Some(&mut remainder), discarded)
                    .map_err(|_| invariant())?;
            }
            goods_sub(retained_original, history_total(&remainder)?)?;
            // Preserve Some(empty), including at a now-deleted source marker.
            next.histories.insert(site, remainder);
        }
        // Stale and unmatched records remain byte-for-byte untouched. A fresh
        // selection from them has no current-stage entitlement to copy.
    }
    for (destination, incoming_stage, lot) in moved {
        let destination_is_source = outgoing.contains_key(&destination)
            || outcome
                .truck_moves()
                .iter()
                .any(|row| row.source() == &destination);
        append_imported_lot(
            next,
            destination,
            stage,
            incoming_stage,
            lot,
            destination_is_source,
        )?;
    }
    for (site, amount) in incoming {
        let old = physical_site(before, &site, outcome.actor())?
            .map(|p| p.goods)
            .unwrap_or_default();
        let current = physical_site(after, &site, outcome.actor())?
            .ok_or_else(invariant)?
            .goods;
        if !outgoing.contains_key(&site)
            && !matches!(site, CargoSite::BrokenMarker(_))
            && current != goods_add(old, amount)?
        {
            return Err(invariant());
        }
        if let Some(h) = next.histories.get(&site)
            && h.stage == stage
        {
            goods_sub(current, history_total(h)?)?;
        }
    }
    Ok(())
}

fn physical_origin<'a>(
    original: &'a [PhysicalTrucks],
    child: &PhysicalTrucks,
) -> Result<&'a PhysicalTrucks, EngineError> {
    let source = original
        .iter()
        .find(|c| c.id == child.id)
        .or_else(|| {
            child
                .parent
                .as_ref()
                .and_then(|id| original.iter().find(|c| &c.id == id))
        })
        .ok_or_else(invariant)?;
    if source.kind != child.kind
        || child.count > source.count
        || (child.id == source.id && child.parent != source.parent)
    {
        return Err(invariant());
    }
    Ok(source)
}

fn same_physical(a: &[PhysicalTrucks], b: &[PhysicalTrucks]) -> bool {
    a.len() == b.len() && a.iter().all(|item| b.iter().any(|other| item == other))
}

/// Only recorded matching physical histories are copied. Missing/mismatched
/// records stay Unknown; older and unrelated records are not normalized away.
fn motion_delta(
    outcome: &ValidatedRepairOrTowOutcome,
    next: &mut CargoHistoryState,
) -> Result<(), EngineError> {
    let mut rows = BTreeMap::new();
    let mut incoming = BTreeMap::<CargoSite, Trucks>::new();
    let mut destination_ids = BTreeSet::new();
    for row in outcome.truck_moves() {
        if !matches!(row.source(), CargoSite::BrokenMarker(_))
            || row.source() == row.destination()
            || row.relocated().is_empty()
            || rows.insert(row.source().clone(), row).is_some()
        {
            return Err(invariant());
        }
        let before = physical_site(outcome.before(), row.source(), outcome.actor())?
            .ok_or_else(invariant)?;
        let after = physical_site(outcome.after(), row.source(), outcome.actor())?;
        let remaining_trucks = after.as_ref().map(|p| p.trucks).unwrap_or_default();
        physical_site(outcome.after(), row.destination(), outcome.actor())?
            .ok_or_else(invariant)?;
        let remainder = after.map(|p| p.cohorts).unwrap_or_default();
        if !same_physical(row.remainder(), &remainder)
            || physical_counts(row.selected())? != physical_counts(row.relocated())?
        {
            return Err(invariant());
        }
        let mut selected = BTreeMap::<String, Trucks>::new();
        let mut relocated = BTreeMap::<String, Trucks>::new();
        let mut conserved = BTreeMap::<String, i32>::new();
        let mut partition_ids = BTreeSet::new();
        for (cohorts, totals) in [
            (row.selected(), &mut selected),
            (row.relocated(), &mut relocated),
        ] {
            for child in cohorts {
                let origin = physical_origin(&before.cohorts, child)?;
                let amount = physical_counts(std::slice::from_ref(child))?;
                let old = totals.get(&origin.id).copied().unwrap_or_default();
                totals.insert(origin.id.clone(), add_trucks(old, amount)?);
            }
        }
        if selected != relocated {
            return Err(invariant());
        }
        for child in row.remainder().iter().chain(row.relocated()) {
            if !partition_ids.insert(&child.id) {
                return Err(invariant());
            }
            let origin = physical_origin(&before.cohorts, child)?;
            let count = conserved
                .get(&origin.id)
                .copied()
                .unwrap_or(0)
                .checked_add(child.count)
                .ok_or_else(invariant)?;
            if count > origin.count {
                return Err(invariant());
            }
            conserved.insert(origin.id.clone(), count);
        }
        for child in row.relocated() {
            if !destination_ids.insert((row.destination().clone(), child.id.clone())) {
                return Err(invariant());
            }
        }
        let amount = physical_counts(row.relocated())?;
        let prior = incoming.get(row.destination()).copied().unwrap_or_default();
        incoming.insert(row.destination().clone(), add_trucks(prior, amount)?);
        let remaining_counts = physical_counts(row.remainder())?;
        sub_trucks(remaining_trucks, remaining_counts)?;
        // Any extra removed count is explicit physical junk, never a transfer.
        sub_trucks(sub_trucks(before.trucks, remaining_trucks)?, amount)?;
    }
    for (destination, moved) in incoming {
        let before = physical_site(outcome.before(), &destination, outcome.actor())?
            .map(|p| p.trucks)
            .unwrap_or_default();
        let after = physical_site(outcome.after(), &destination, outcome.actor())?
            .ok_or_else(invariant)?
            .trucks;
        if rows.contains_key(&destination) || after != add_trucks(before, moved)? {
            return Err(invariant());
        }
    }
    let original_entries = outcome
        .before()
        .logistics
        .cargo_history
        .motion
        .entries
        .clone();
    let mut entry_keys = BTreeSet::new();
    for entry in &original_entries {
        if !entry_keys.insert((entry.site.clone(), entry.stage)) {
            return Err(invariant());
        }
    }
    let mut additions = Vec::<(CargoSite, WaterStage, TruckMotion)>::new();
    for (index, entry) in original_entries.iter().enumerate() {
        let CargoSite::BrokenMarker(id) = &entry.site else {
            continue;
        };
        let Some(marker) = outcome.before().land.breakdown.markers.get(id) else {
            continue;
        };
        if marker.side != outcome.actor() {
            continue;
        }
        let original =
            physical_site(outcome.before(), &entry.site, outcome.actor())?.ok_or_else(invariant)?;
        let current = physical_site(outcome.after(), &entry.site, outcome.actor())?;
        let remaining: &[PhysicalTrucks] = current
            .as_ref()
            .map(|p| p.cohorts.as_slice())
            .unwrap_or(&[]);
        let row = rows.get(&entry.site).copied();
        let mut changed = Vec::new();
        let mut ids = BTreeSet::new();
        for record in &entry.cohorts {
            if record.id.is_empty()
                || record.count <= 0
                || record.spent_cp_quarters < 0
                || !ids.insert(&record.id)
            {
                return Err(invariant());
            }
            let Some(origin) = original
                .cohorts
                .iter()
                .find(|c| c.id == record.id && c.kind == record.kind && c.count == record.count)
            else {
                changed.push(record.clone()); // an unmatched record conveys no new right
                continue;
            };
            for child in remaining {
                let parent = physical_origin(&original.cohorts, child)?;
                if parent.id == origin.id {
                    changed.push(TruckMotion {
                        id: child.id.clone(),
                        kind: child.kind,
                        count: child.count,
                        spent_cp_quarters: record.spent_cp_quarters,
                    });
                }
            }
            if let Some(row) = row {
                for child in row.relocated() {
                    if physical_origin(&original.cohorts, child)?.id == origin.id {
                        additions.push((
                            row.destination().clone(),
                            entry.stage,
                            TruckMotion {
                                id: child.id.clone(),
                                kind: child.kind,
                                count: child.count,
                                spent_cp_quarters: record.spent_cp_quarters,
                            },
                        ));
                    }
                }
            }
        }
        let target = next.motion.entries.get_mut(index).ok_or_else(invariant)?;
        target.cohorts = changed; // preserve the entry's stage and present-empty shape
    }
    for (site, stage, record) in additions {
        if next.motion.entries.iter().any(|entry| {
            entry.stage == stage
                && cargo_history::owner(outcome.after(), &entry.site) == Some(outcome.actor())
                && entry.cohorts.iter().any(|h| h.id == record.id)
        }) {
            return Err(invariant());
        }
        let mut matching = next
            .motion
            .entries
            .iter_mut()
            .filter(|entry| entry.site == site && entry.stage == stage);
        if let Some(entry) = matching.next() {
            if matching.next().is_some() || entry.cohorts.iter().any(|h| h.id == record.id) {
                return Err(invariant());
            }
            entry.cohorts.push(record);
        } else {
            next.motion.entries.push(MotionEntry {
                site,
                stage,
                cohorts: vec![record],
            });
        }
    }
    Ok(())
}

/// Read AFTER the actual Oob pool allocation and Land physical partition.
/// Source identity and histories come from the post-debit BEFORE snapshot.
/// Cases: land:22.23, land:22.24, land:22.26, land:22.35, land:22.42, airlog:53.25
pub(crate) fn prepare_recovery_relocation(
    state: &State,
    outcome: &ValidatedRepairOrTowOutcome,
) -> Result<PreparedRecoveryRelocation, EngineError> {
    if !exact_state(state, outcome.after())?
        || WaterStage::current(state) != outcome.stage()
        || WaterStage::current(outcome.before()) != outcome.stage()
        || encoded(&outcome.before().logistics.cargo_history)?
            != encoded(&state.logistics.cargo_history)?
        || encoded(&(
            &outcome.before().logistics.fuel_accounts,
            &outcome.before().logistics.fuel_segments,
            &outcome.before().logistics.pool_fuel_accounts,
            &outcome.before().logistics.pool_fuel_segments,
        ))? != encoded(&(
            &state.logistics.fuel_accounts,
            &state.logistics.fuel_segments,
            &state.logistics.pool_fuel_accounts,
            &state.logistics.pool_fuel_segments,
        ))?
    {
        return Err(invariant());
    }
    let mut next = state.logistics.cargo_history.clone();
    cargo_delta(outcome, &mut next)?;
    motion_delta(outcome, &mut next)?;
    Ok(PreparedRecoveryRelocation {
        before_apply: state.clone(),
        histories: next,
    })
}

/// Checks precede the only write. Full caller rollback still covers RNG/events.
/// Cases: land:22.23, land:22.24, land:22.26, land:22.35, land:22.42, airlog:53.25
pub(crate) fn apply_recovery_relocation(
    state: &mut State,
    prepared: &PreparedRecoveryRelocation,
) -> Result<(), EngineError> {
    if !exact_state(state, &prepared.before_apply)? {
        return Err(invariant());
    }
    state.logistics.cargo_history = prepared.histories.clone();
    Ok(())
}

#[cfg(test)]
mod destination_retirement_tests {
    use super::*;

    fn stage(op_stage: u8) -> WaterStage {
        WaterStage {
            game_turn: 1,
            op_stage,
        }
    }
    fn lot(id: &str, stores: i32) -> CargoLot {
        CargoLot {
            id: id.into(),
            goods: Supplies {
                stores,
                ..Default::default()
            },
            spent_cp_quarters: 17,
            ceiling_cp_quarters: 160,
            continuous_first_line: true,
        }
    }
    fn history(at: WaterStage, lot: CargoLot) -> CargoHistory {
        CargoHistory {
            stage: at,
            lots: vec![lot],
        }
    }

    /// Cases: land:22.23, airlog:53.25
    #[test]
    fn only_older_destination_retires_for_current_tag_and_other_parcels_are_unchanged() {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        s.cursor.op_stage = Some(2);
        let current = WaterStage::current(&s);
        let destination = CargoSite::Pool("retirement-fixture".into());
        let unrelated = CargoSite::Dump("ax_dump_1".into());
        let source = CargoSite::Dump("ax_dump_2".into());
        s.logistics
            .cargo_history
            .histories
            .insert(destination.clone(), history(stage(1), lot("old", 1)));
        s.logistics
            .cargo_history
            .histories
            .insert(source.clone(), history(current, lot("source", 1)));
        s.logistics
            .cargo_history
            .histories
            .insert(unrelated.clone(), history(current, lot("other", 1)));
        // This is unrelated real authored stock, not a recovery supply entitlement.
        s.logistics
            .dumps
            .get_mut("ax_dump_1")
            .unwrap()
            .supplies
            .stores = 4;
        s.logistics
            .dumps
            .get_mut("ax_dump_2")
            .unwrap()
            .supplies
            .stores = 4;
        let normalized_before: Vec<_> = [&source, &unrelated]
            .into_iter()
            .map(|site| {
                (
                    site.clone(),
                    cargo_history::parcels(&s, Side::Axis, site).unwrap(),
                )
            })
            .collect();
        let raw_before = encoded(&s.logistics.cargo_history).unwrap();
        let fields_before = encoded(&(
            &s.logistics.fuel_accounts,
            &s.logistics.pool_fuel_accounts,
            &s.logistics.fuel_segments,
            &s.logistics.pool_fuel_segments,
            &s.logistics.dumps,
        ))
        .unwrap();
        let imported = lot("incoming-current", 2);
        let mut next = s.logistics.cargo_history.clone();
        append_imported_lot(
            &mut next,
            destination.clone(),
            current,
            current,
            imported.clone(),
            false,
        )
        .unwrap();
        assert_eq!(next.histories[&destination], history(current, imported));
        for (site, previous) in &s.logistics.cargo_history.histories {
            if site != &destination {
                assert_eq!(next.histories.get(site), Some(previous));
            }
        }
        assert_eq!(encoded(&next.next_id).unwrap(), raw_before["next_id"]);
        assert_eq!(encoded(&next.motion).unwrap(), raw_before["motion"]);
        s.logistics.cargo_history = next;
        for (site, parcels) in normalized_before {
            assert_eq!(
                cargo_history::parcels(&s, Side::Axis, &site).unwrap(),
                parcels
            );
        }
        assert_eq!(
            encoded(&(
                &s.logistics.fuel_accounts,
                &s.logistics.pool_fuel_accounts,
                &s.logistics.fuel_segments,
                &s.logistics.pool_fuel_segments,
                &s.logistics.dumps
            ))
            .unwrap(),
            fields_before
        );
    }

    /// Cases: land:22.23, airlog:53.25
    #[test]
    fn current_stage_unmergeable_destination_still_refuses_without_retiring_it() {
        let destination = CargoSite::Pool("current-destination".into());
        let mut h = CargoHistoryState::default();
        h.histories.insert(
            destination.clone(),
            history(stage(2), lot("existing-current", 3)),
        );
        let before = encoded(&h).unwrap();
        let result = append_imported_lot(
            &mut h,
            destination,
            stage(2),
            stage(1),
            lot("incoming-old", 2),
            false,
        );
        assert!(
            matches!(result, Err(EngineError::Unsupported { ref case, .. }) if case == "airlog:53.25")
        );
        assert_eq!(encoded(&h).unwrap(), before);
    }

    /// Cases: land:22.23, airlog:53.25
    #[test]
    fn exception_never_retires_a_source_or_newer_entry_and_empty_import_is_not_success() {
        for (entry_stage, is_source, incoming, unsupported) in [
            (stage(1), true, lot("incoming", 2), true),
            (stage(3), false, lot("incoming", 2), true),
            (stage(1), false, lot("empty", 0), false),
        ] {
            let destination = CargoSite::Pool("protected".into());
            let mut h = CargoHistoryState::default();
            h.histories.insert(
                destination.clone(),
                history(entry_stage, lot("existing", 3)),
            );
            let before = encoded(&h).unwrap();
            let result =
                append_imported_lot(&mut h, destination, stage(2), stage(2), incoming, is_source);
            if unsupported {
                assert!(
                    matches!(result, Err(EngineError::Unsupported { ref case, .. }) if case == "airlog:53.25")
                );
            } else {
                assert!(matches!(result, Err(EngineError::Invariant { .. })));
            }
            assert_eq!(encoded(&h).unwrap(), before);
        }
    }

    /// Cases: land:22.23, airlog:53.25
    #[test]
    fn malformed_expired_destination_retains_canonical_error_without_retirement() {
        let destination = CargoSite::Pool("corrupt-old".into());
        let mut h = CargoHistoryState::default();
        h.histories
            .insert(destination.clone(), history(stage(1), lot("bad", -1)));
        let before = encoded(&h).unwrap();
        let result = append_imported_lot(
            &mut h,
            destination,
            stage(2),
            stage(2),
            lot("incoming", 1),
            false,
        );
        assert!(matches!(result, Err(EngineError::Invariant { .. })));
        assert_eq!(encoded(&h).unwrap(), before);
    }
}
