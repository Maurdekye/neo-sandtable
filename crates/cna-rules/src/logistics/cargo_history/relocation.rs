//! Trusted breakdown partitions preserve parcel history without relay handling.
use super::*;
use crate::{CnaContent, logistics::CargoPacking, state::Location};
use cna_content::units::Trucks;
use cna_core::engine::EngineError;

#[derive(Debug, Clone)]
pub struct PoolCargoShare {
    pub marker: String,
    pub goods: Supplies,
}

/// Temporary consequence witness; never serialized or accepted from an owner.
#[derive(Debug)]
pub struct PreparedPoolRelocation {
    side: Side,
    stage: WaterStage,
    pool: String,
    location: Option<Location>,
    working_trucks: Trucks,
    working_goods: Supplies,
    markers: Vec<(String, serde_json::Value)>,
    before_histories: BTreeMap<CargoSite, CargoHistory>,
    before_serials: BTreeMap<Side, u64>,
    after_histories: BTreeMap<CargoSite, CargoHistory>,
    after_serials: BTreeMap<Side, u64>,
}
fn invariant(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("pool breakdown cargo relocation: {detail}"),
    }
}
fn checked<T>(r: Result<T, CargoError>) -> Result<T, EngineError> {
    r.map_err(|_| invariant("invalid quantities or parcel history"))
}
fn packing_error(error: super::super::SupplyError) -> EngineError {
    match error {
        super::super::SupplyError::Unsupported { case } => EngineError::Unsupported {
            case: case.into(),
            detail: "pool breakdown cargo capacity is unsupported".into(),
        },
        _ => invariant("pool breakdown cargo lacks verified capacity"),
    }
}
fn pool<'a>(
    state: &'a State,
    id: &str,
    side: Side,
) -> Result<&'a crate::state::TruckPool, EngineError> {
    let mut found = state.logistics.truck_pools.iter().filter(|p| p.id == id);
    let p = found
        .next()
        .ok_or_else(|| invariant("missing source pool"))?;
    if id.is_empty() || found.next().is_some() || p.side != side {
        return Err(invariant("source identity or ownership changed"));
    }
    Ok(p)
}
fn count<'a>(
    t: &'a mut Trucks,
    equipment: &crate::land::breakdown::Equipment,
) -> Result<&'a mut i32, EngineError> {
    use crate::land::breakdown::Equipment;
    match equipment {
        Equipment::LightTruck => Ok(&mut t.light),
        Equipment::MediumTruck => Ok(&mut t.medium),
        Equipment::HeavyTruck => Ok(&mut t.heavy),
        _ => Err(invariant("non-truck pool marker")),
    }
}

/// Prepare after validated markers exist, while the pool still has its pre-loss stock/counts.
/// Land owns the loss outcome, cohort selection, fuel/reserve split and marker creation.
/// This helper checks carrying capacity and one uniform cargo partition, not movement rights.
/// Cases: land:21.43, airlog:53.25, airlog:54.2
pub fn prepare_pool_breakdown_relocation(
    content: &CnaContent,
    state: &State,
    side: Side,
    id: &str,
    working: &CargoPacking,
    shares: &[PoolCargoShare],
) -> Result<PreparedPoolRelocation, EngineError> {
    let p = pool(state, id, side)?;
    if p.location.as_ref().and_then(Location::hex).is_none()
        || [p.trucks.light, p.trucks.medium, p.trucks.heavy]
            .iter()
            .any(|n| *n < 0)
        || !valid(&p.cargo)
    {
        return Err(invariant("invalid source physical holdings"));
    }
    let stage = WaterStage::current(state);
    let source = CargoSite::Pool(id.into());
    let before = &state.logistics.cargo_history;
    let mut after = before.clone();
    let mut h = before
        .histories
        .get(&source)
        .filter(|h| h.stage == stage)
        .cloned()
        .unwrap_or(CargoHistory {
            stage,
            lots: vec![],
        });
    checked(validate(&h))?;
    h.lots.retain(|l| !empty(&l.goods));
    h.lots.sort_by(|a, b| a.id.cmp(&b.id));
    let mut fresh = p.cargo;
    checked(sub(&mut fresh, &checked(totals(&h))?))?;
    let history = h.lots.first().map(|l| {
        (
            l.spent_cp_quarters,
            l.ceiling_cp_quarters,
            l.continuous_first_line,
        )
    });
    if h.lots.iter().any(|lot| {
        before.histories.iter().any(|(site, other)| {
            *site != source && other.lots.iter().any(|existing| existing.id == lot.id)
        })
    }) {
        return Err(invariant(
            "source parcel identity is duplicated at another site",
        ));
    }
    if h.lots.iter().any(|l| {
        Some((
            l.spent_cp_quarters,
            l.ceiling_cp_quarters,
            l.continuous_first_line,
        )) != history
    }) || (history.is_some() && !empty(&fresh))
    {
        return Err(EngineError::Unsupported {
            case: "airlog:53.25".into(),
            detail: "pool breakdown partition requires distinct cargo histories".into(),
        });
    }
    let mut goods_left = p.cargo;
    let mut trucks_left = p.trucks;
    let mut destinations = vec![];
    let mut seen = BTreeSet::new();
    let mut physical_ids = BTreeSet::new();
    let mut ordered: Vec<_> = shares.iter().collect();
    ordered.sort_by(|a, b| a.marker.cmp(&b.marker));
    for share in ordered {
        if share.marker.is_empty() || !seen.insert(&share.marker) || !valid(&share.goods) {
            return Err(invariant("invalid or repeated marker share"));
        }
        let marker = state
            .land
            .breakdown
            .markers
            .get(&share.marker)
            .ok_or_else(|| invariant("prepared marker is missing"))?;
        let site = CargoSite::BrokenMarker(share.marker.clone());
        if marker.id != share.marker
            || marker.side != side
            || marker.source_pool.as_deref() != Some(id)
            || !marker.assets.is_empty()
            || !marker.passengers.is_empty()
            || marker.transport != Trucks::default()
            || !marker.fuel_cohorts.is_empty()
            || before.histories.contains_key(&site)
        {
            return Err(invariant("marker is not a fresh whole-pool consequence"));
        }
        let mut trucks = Trucks::default();
        for asset in &marker.pool_assets {
            if asset.pool != id
                || asset.points <= 0
                || asset.cohort.is_empty()
                || !physical_ids.insert(asset.cohort.clone())
            {
                return Err(invariant("invalid or repeated physical marker asset"));
            }
            let n = count(&mut trucks, &asset.equipment)?;
            *n = n
                .checked_add(asset.points)
                .ok_or_else(|| invariant("marker count overflow"))?;
        }
        if trucks == Trucks::default() {
            return Err(invariant("zero-equipment marker must not be created"));
        }
        for (left, n) in [
            (&mut trucks_left.light, trucks.light),
            (&mut trucks_left.medium, trucks.medium),
            (&mut trucks_left.heavy, trucks.heavy),
        ] {
            *left = left
                .checked_sub(n)
                .filter(|n| *n >= 0)
                .ok_or_else(|| invariant("marker trucks exceed source"))?;
        }
        super::super::capacity::validate_packing(
            content,
            &trucks,
            &Trucks::default(),
            &share.goods,
            &marker.cargo,
        )
        .map_err(packing_error)?;
        checked(sub(&mut goods_left, &share.goods))?;
        destinations.push((
            share,
            site,
            serde_json::to_value(marker)
                .map_err(|_| invariant("marker footprint cannot be encoded"))?,
        ));
    }
    super::super::capacity::validate_packing(
        content,
        &trucks_left,
        &Trucks::default(),
        &goods_left,
        working,
    )
    .map_err(packing_error)?;
    let mut used_ids: BTreeSet<_> = before
        .histories
        .values()
        .flat_map(|h| h.lots.iter().map(|l| l.id.clone()))
        .collect();
    for (share, site, _) in &destinations {
        let mut left = share.goods;
        let mut moved = vec![];
        for lot in &mut h.lots {
            let mut quantity = Supplies::default();
            for k in 0..4 {
                set(&mut quantity, k, get(&left, k).min(get(&lot.goods, k)));
            }
            if empty(&quantity) {
                continue;
            }
            let mut parcel = lot.clone();
            if quantity != lot.goods {
                parcel.id = checked(next_id(&mut after, side))?;
                if !used_ids.insert(parcel.id.clone()) {
                    return Err(invariant("cargo serial would reuse an existing identity"));
                }
            }
            parcel.goods = quantity;
            checked(sub(&mut left, &quantity))?;
            checked(sub(&mut lot.goods, &quantity))?;
            moved.push(parcel);
        }
        // Entirely fresh stock stays untagged; it acquires no fabricated CP ceiling.
        if !empty(&left) {
            checked(sub(&mut fresh, &left))?;
        }
        if !moved.is_empty() {
            after
                .histories
                .insert(site.clone(), CargoHistory { stage, lots: moved });
        }
    }
    h.lots.retain(|l| !empty(&l.goods));
    let mut remainder = checked(totals(&h))?;
    checked(add(&mut remainder, &fresh))?;
    if remainder != goods_left {
        return Err(invariant("partition does not conserve cargo"));
    }
    if h.lots.is_empty() {
        after.histories.remove(&source);
    } else {
        after.histories.insert(source, h);
    }
    Ok(PreparedPoolRelocation {
        side,
        stage,
        pool: id.into(),
        location: p.location.clone(),
        working_trucks: trucks_left,
        working_goods: goods_left,
        markers: destinations
            .into_iter()
            .map(|(s, _, v)| (s.marker.clone(), v))
            .collect(),
        before_histories: before.histories.clone(),
        before_serials: before.next_id.clone(),
        after_histories: after.histories,
        after_serials: after.next_id,
    })
}

/// Apply after exact pool inventory reduction; all checks precede history mutation.
/// Moving physical histories separately never overwrites them here.
/// Cases: land:21.43, airlog:53.25
pub fn apply_pool_breakdown_relocation(
    state: &mut State,
    prepared: PreparedPoolRelocation,
) -> Result<(), EngineError> {
    let p = pool(state, &prepared.pool, prepared.side)?;
    if WaterStage::current(state) != prepared.stage
        || p.location != prepared.location
        || p.trucks != prepared.working_trucks
        || p.cargo != prepared.working_goods
        || state.logistics.cargo_history.histories != prepared.before_histories
        || state.logistics.cargo_history.next_id != prepared.before_serials
    {
        return Err(invariant("prepared source/history footprint changed"));
    }
    for (id, footprint) in &prepared.markers {
        let marker = state
            .land
            .breakdown
            .markers
            .get(id)
            .ok_or_else(|| invariant("prepared marker disappeared"))?;
        if serde_json::to_value(marker)
            .map_err(|_| invariant("marker footprint cannot be encoded"))?
            != *footprint
        {
            return Err(invariant("prepared marker footprint changed"));
        }
    }
    state.logistics.cargo_history.histories = prepared.after_histories;
    state.logistics.cargo_history.next_id = prepared.after_serials;
    Ok(())
}

#[cfg(test)]
mod tests;
