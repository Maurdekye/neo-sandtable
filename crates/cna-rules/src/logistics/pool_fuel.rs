//! Fuel for actual second-/third-line carriers. Pool identities have separate
//! checkpointed maps; no land-unit record or troop-body rate is fabricated.
use super::{
    cargo_history::{
        CargoSite,
        motion::{self, MotionError, PhysicalTrucks, TruckMotion},
    },
    segment::{
        self, FuelCohortSelection, FuelFundingAccount, FuelSegmentLedger, SegmentFuelPlan,
        SegmentKey, TruckFuelCohort,
    },
    supply::{self, SupplyDemand, SupplyDraw, SupplyError, SupplySource},
};
use crate::{
    CnaContent, State,
    state::{Location, TruckPool},
};
use std::collections::BTreeSet;

fn pool<'a>(state: &'a State, id: &str) -> Result<&'a TruckPool, SupplyError> {
    let mut matches = state.logistics.truck_pools.iter().filter(|p| p.id == id);
    let pool = matches.next().ok_or(SupplyError::Invalid)?;
    if id.is_empty()
        || matches.next().is_some()
        || pool.tank_fuel.get() < 0
        || pool.activity_water.get() < 0
    {
        return Err(SupplyError::Invalid);
    }
    segment::truck_total(&pool.trucks)?;
    Ok(pool)
}
fn ledger(state: &State, id: &str) -> Result<FuelSegmentLedger<String>, SupplyError> {
    let p = pool(state, id)?;
    let mut result = segment::carrier_ledger(
        state.logistics.pool_fuel_segments.get(id),
        SegmentKey::current(state),
        p.location.as_ref().ok_or(SupplyError::Invalid)?,
        &p.trucks,
        &id.to_owned(),
        &format!("pool.{id}"),
    )?;
    if !state.logistics.pool_fuel_segments.contains_key(id) {
        match unfunded_motion(state, id) {
            Ok(records) => {
                result.cohorts = records
                    .into_iter()
                    .map(|r| TruckFuelCohort {
                        id: r.id,
                        parent: None,
                        kind: r.kind,
                        count: r.count,
                        cp_quarters: 0,
                        account: id.to_owned(),
                        segment: result.segment.clone(),
                    })
                    .collect();
                // Genesis uses at most three serials. Reserve all of them even
                // after an unfunded withdrawal removed the highest identity.
                result.next_cohort_serial = 3;
            }
            Err(MotionError::Unknown) => {}
            Err(_) => return Err(SupplyError::Invalid),
        }
    }
    Ok(result)
}

fn unfunded_motion(state: &State, id: &str) -> Result<Vec<TruckMotion>, MotionError> {
    let p = pool(state, id).map_err(|_| MotionError::Invalid)?;
    if state.logistics.pool_fuel_accounts.contains_key(id) {
        return Err(MotionError::Invalid);
    }
    let site = CargoSite::Pool(id.into());
    let stage = super::water::WaterStage::current(state);
    let mut matches = state
        .logistics
        .cargo_history
        .motion
        .entries
        .iter()
        .filter(|e| e.site == site && e.stage == stage);
    let entry = matches.next().ok_or(MotionError::Unknown)?;
    if matches.next().is_some() {
        return Err(MotionError::Invalid);
    }
    let physical: Vec<_> = entry
        .cohorts
        .iter()
        .map(|h| PhysicalTrucks {
            id: h.id.clone(),
            parent: None,
            kind: h.kind,
            count: h.count,
        })
        .collect();
    let records = motion::query(state, p.side, &site, &physical)?;
    validate_unfunded_motion(p, id, records)
}

fn validate_unfunded_motion(
    p: &TruckPool,
    id: &str,
    records: Vec<TruckMotion>,
) -> Result<Vec<TruckMotion>, MotionError> {
    let prefix = format!("pool.{id}.fuel-trucks-");
    let mut identities = vec![];
    let mut kinds = BTreeSet::new();
    for r in &records {
        let limit = match r.kind {
            segment::FuelTruckKind::Light => 1,
            segment::FuelTruckKind::Medium => 2,
            segment::FuelTruckKind::Heavy => 3,
        };
        let serial =
            r.id.strip_prefix(&prefix)
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|n| (1..=limit).contains(n))
                .ok_or(MotionError::Invalid)?;
        if r.id != format!("{prefix}{serial}") || !kinds.insert(r.kind) {
            return Err(MotionError::Invalid);
        }
        identities.push((r.kind, serial));
    }
    identities.sort();
    if identities.windows(2).any(|pair| pair[0].1 >= pair[1].1) {
        return Err(MotionError::Invalid);
    }
    for (kind, expected) in [
        (segment::FuelTruckKind::Light, p.trucks.light),
        (segment::FuelTruckKind::Medium, p.trucks.medium),
        (segment::FuelTruckKind::Heavy, p.trucks.heavy),
    ] {
        let count = records
            .iter()
            .filter(|r| r.kind == kind)
            .try_fold(0i32, |n, r| {
                n.checked_add(r.count).ok_or(MotionError::Invalid)
            })?;
        if count != expected {
            return Err(MotionError::Invalid);
        }
    }
    if records
        .iter()
        .any(|r| r.spent_cp_quarters != 0 || r.count <= 0)
    {
        return Err(MotionError::Invalid);
    }
    Ok(records)
}

/// Exact surviving genesis groups for an unfunded withdrawal, without seeding.
/// Cases: land:20.83, airlog:49.13, airlog:53.25
pub fn unfunded_pool_physical_cohorts(
    state: &State,
    id: &str,
) -> Result<Vec<PhysicalTrucks>, MotionError> {
    if state.logistics.pool_fuel_segments.contains_key(id) {
        return Err(MotionError::Invalid);
    }
    Ok(unfunded_motion(state, id)?
        .into_iter()
        .map(|r| PhysicalTrucks {
            id: r.id,
            parent: None,
            kind: r.kind,
            count: r.count,
        })
        .collect())
}

/// Location-free identities for a trusted fresh allocation, never a legacy fallback.
/// Cases: airlog:49.13, land:21.25, land:21.29
pub fn fresh_pool_physical_cohorts(
    state: &State,
    id: &str,
) -> Result<Vec<PhysicalTrucks>, SupplyError> {
    let p = pool(state, id)?;
    if state.logistics.pool_fuel_segments.contains_key(id)
        || state.logistics.pool_fuel_accounts.contains_key(id)
        || state
            .logistics
            .cargo_history
            .motion
            .entries
            .iter()
            .any(|e| e.site == CargoSite::Pool(id.into()))
    {
        return Err(SupplyError::Invalid);
    }
    let (cohorts, _) = segment::initial_cohorts(
        &p.trucks,
        &id.to_owned(),
        &format!("pool.{id}"),
        &SegmentKey::current(state),
        0,
        0,
    )?;
    Ok(cohorts.iter().map(PhysicalTrucks::from).collect())
}

/// Seed only a trusted newly allocated real pool, including unresolved setup placement.
/// No creation caller is activated by this helper alone.
/// Cases: airlog:53.25, land:6.13
pub fn seed_created_pool(state: &mut State, id: &str) -> Result<(), cna_core::engine::EngineError> {
    let invalid = || cna_core::engine::EngineError::Invariant {
        detail: format!("fresh pool {id}: physical creation identity cannot be reconciled"),
    };
    if !state.logistics.truck_pool_ids.contains(id) {
        return Err(invalid());
    }
    let cohorts = fresh_pool_physical_cohorts(state, id).map_err(|_| invalid())?;
    let side = pool(state, id).map_err(|_| invalid())?.side;
    motion::seed_fresh(state, side, &CargoSite::Pool(id.into()), &cohorts).map_err(|_| invalid())
}

/// Authoritative early-OpStage preparation only, not a query or a mid-stage repair.
/// Retained fuel identities stay authoritative; historical funding is never rewritten.
/// Duplicate entry preserves the complete current physical history.
/// Cases: land:6.13, airlog:53.25
pub fn seed_pool_opstage(state: &mut State) -> Result<(), cna_core::engine::EngineError> {
    use cna_core::engine::EngineError;
    let invalid = || EngineError::Invariant {
        detail: "pool OpStage physical identities cannot be reconciled".into(),
    };
    if state.cursor.anchor() != "opstage.weather"
        || !matches!(state.cursor.op_stage, Some(1..=3))
        || state.cursor.half.is_some()
    {
        return Err(invalid());
    }
    let stage = super::water::WaterStage::current(state);
    let mut ids = BTreeSet::new();
    for p in &state.logistics.truck_pools {
        if !ids.insert(p.id.clone()) || !state.logistics.truck_pool_ids.contains(&p.id) {
            return Err(invalid());
        }
    }
    let mut draft = state.clone();
    let mut physical_ids = BTreeSet::new();
    for id in ids {
        let p = pool(&draft, &id).map_err(|_| invalid())?;
        let side = p.side;
        let site = CargoSite::Pool(id.clone());
        let groups: Vec<PhysicalTrucks> = if let Some(old) =
            draft.logistics.pool_fuel_segments.get(&id)
        {
            // Validate against the retained segment rather than creating a new fuel account.
            segment::carrier_ledger(
                Some(old),
                old.segment.clone(),
                &old.origin,
                &p.trucks,
                &id,
                &format!("pool.{id}"),
            )
            .map_err(|_| invalid())?
            .cohorts
            .iter()
            .map(PhysicalTrucks::from)
            .collect()
        } else {
            if draft.logistics.pool_fuel_accounts.contains_key(&id) {
                return Err(invalid());
            }
            let mut old = draft
                .logistics
                .cargo_history
                .motion
                .entries
                .iter()
                .filter(|e| e.site == site);
            let records = if let Some(entry) = old.next() {
                if old.next().is_some() {
                    return Err(invalid());
                }
                validate_unfunded_motion(p, &id, entry.cohorts.clone()).map_err(|_| invalid())?
            } else {
                segment::initial_cohorts(
                    &p.trucks,
                    &id,
                    &format!("pool.{id}"),
                    &SegmentKey::current(&draft),
                    0,
                    0,
                )
                .map_err(|_| invalid())?
                .0
                .iter()
                .map(|c| TruckMotion {
                    id: c.id.clone(),
                    kind: c.kind,
                    count: c.count,
                    spent_cp_quarters: 0,
                })
                .collect()
            };
            records
                .iter()
                .map(|r| PhysicalTrucks {
                    id: r.id.clone(),
                    parent: None,
                    kind: r.kind,
                    count: r.count,
                })
                .collect()
        };
        for group in &groups {
            if !physical_ids.insert((side, group.id.clone()))
                || draft
                    .logistics
                    .cargo_history
                    .motion
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.site != site
                            && entry.stage == stage
                            && super::cargo_history::owner(&draft, &entry.site) == Some(side)
                    })
                    .any(|entry| entry.cohorts.iter().any(|cohort| cohort.id == group.id))
            {
                return Err(invalid());
            }
        }
        let current: Vec<_> = draft
            .logistics
            .cargo_history
            .motion
            .entries
            .iter()
            .filter(|e| e.site == site && e.stage == stage)
            .collect();
        if current.len() > 1 {
            return Err(invalid());
        }
        if let Some(entry) = current.first() {
            if entry.cohorts.len() != groups.len() {
                return Err(invalid());
            }
            motion::query(&draft, side, &site, &groups).map_err(|_| invalid())?;
        } else {
            motion::seed_fresh(&mut draft, side, &site, &groups).map_err(|_| invalid())?;
        }
    }
    // Cargo history already expires by WaterStage; no old parcels or stock are deleted.
    state.logistics.cargo_history.motion = draft.logistics.cargo_history.motion;
    Ok(())
}

/// Retire exact departing counts before inventory subtraction. No caller is activated here.
/// Unknown legacy physical history stays Unknown; invalid known history rolls back.
/// Cases: land:20.83, airlog:49.13, airlog:49.16, airlog:53.25
pub fn retire_pool_truck_counts(
    state: &mut State,
    id: &str,
    mut amount: cna_content::units::Trucks,
) -> Result<(), cna_core::engine::EngineError> {
    use cna_core::engine::EngineError;
    let invalid = || EngineError::Invariant {
        detail: "departing pool physical truck counts cannot be reconciled".into(),
    };
    let p = pool(state, id).map_err(|_| invalid())?;
    let side = p.side;
    segment::truck_total(&amount).map_err(|_| invalid())?;
    if amount.light > p.trucks.light
        || amount.medium > p.trucks.medium
        || amount.heavy > p.trucks.heavy
    {
        return Err(invalid());
    }
    if amount == cna_content::units::Trucks::default() {
        return Ok(());
    }
    let funded = state.logistics.pool_fuel_segments.contains_key(id);
    let groups: Vec<PhysicalTrucks> = if funded {
        pool_segment_fuel_cohorts(state, id)
            .map_err(|_| invalid())?
            .iter()
            .map(PhysicalTrucks::from)
            .collect()
    } else {
        match unfunded_pool_physical_cohorts(state, id) {
            Ok(groups) => groups,
            Err(MotionError::Unknown) => return Ok(()),
            Err(_) => return Err(invalid()),
        }
    };
    let has_current_motion = state
        .logistics
        .cargo_history
        .motion
        .entries
        .iter()
        .any(|entry| {
            entry.site == CargoSite::Pool(id.into())
                && entry.stage == super::water::WaterStage::current(state)
        });
    if funded && has_current_motion {
        // Selection-only retirement cannot detect oversized or unbound survivors.
        // Reconcile the complete current footprint before splitting any fuel identity.
        motion::query(state, side, &CargoSite::Pool(id.into()), &groups).map_err(|_| invalid())?;
    }
    let mut selection = Vec::new();
    for g in groups {
        let remaining = match g.kind {
            segment::FuelTruckKind::Light => &mut amount.light,
            segment::FuelTruckKind::Medium => &mut amount.medium,
            segment::FuelTruckKind::Heavy => &mut amount.heavy,
        };
        let count = (*remaining).min(g.count);
        if count > 0 {
            selection.push(FuelCohortSelection { id: g.id, count });
            *remaining -= count;
        }
    }
    if amount != cna_content::units::Trucks::default() {
        return Err(invalid());
    }
    let mut draft = state.clone();
    if funded {
        let retired =
            remove_selected_pool_fuel_cohorts(&mut draft, id, &selection).map_err(|_| invalid())?;
        let physical: Vec<_> = retired.iter().map(PhysicalTrucks::from).collect();
        match motion::retire(&mut draft, side, &CargoSite::Pool(id.into()), &physical) {
            Ok(()) => {}
            Err(MotionError::Unknown) if !has_current_motion => {}
            Err(_) => return Err(invalid()),
        }
    } else {
        retire_unfunded_pool_motion(&mut draft, id, &selection).map_err(|_| invalid())?;
    }
    state.logistics = draft.logistics;
    Ok(())
}

/// Retire known zero-CP physical counts before an unfunded inventory withdrawal.
/// Missing legacy history remains Unknown; this never makes a fuel record or child.
/// Cases: land:20.83, airlog:49.13, airlog:53.25
pub fn retire_unfunded_pool_motion(
    state: &mut State,
    id: &str,
    selection: &[FuelCohortSelection],
) -> Result<(), MotionError> {
    if state.logistics.pool_fuel_segments.contains_key(id) {
        return Err(MotionError::Invalid);
    }
    let records = unfunded_motion(state, id)?;
    let p = pool(state, id).map_err(|_| MotionError::Invalid)?;
    let side = p.side;
    let selected = selection
        .iter()
        .map(|s| {
            let record = records
                .iter()
                .find(|r| r.id == s.id)
                .ok_or(MotionError::Invalid)?;
            Ok(PhysicalTrucks {
                id: record.id.clone(),
                parent: None,
                kind: record.kind,
                count: s.count,
            })
        })
        .collect::<Result<Vec<_>, MotionError>>()?;
    motion::retire_selected_counts(state, side, &CargoSite::Pool(id.into()), &selected)
}
fn account(
    state: &State,
    id: &str,
    owner: &str,
    ledger: &FuelSegmentLedger<String>,
) -> Result<FuelFundingAccount, SupplyError> {
    let side = pool(state, owner)?.side;
    let a = if let Some(a) = state
        .logistics
        .pool_fuel_accounts
        .get(id)
        .filter(|a| a.segment == ledger.segment)
    {
        a.clone()
    } else if id == owner {
        FuelFundingAccount {
            side,
            segment: ledger.segment.clone(),
            origin: ledger.origin.clone(),
            paid_cost: ledger.paid_cost,
            draws: ledger.draws.clone(),
        }
    } else {
        return Err(SupplyError::Invalid);
    };
    if a.side != side {
        return Err(SupplyError::Invalid);
    }
    segment::validate_origin(&a.origin)?;
    segment::draws_map(&a.draws, a.paid_cost)?;
    Ok(a)
}

/// Only the consuming convoy's tanks and cargo follow it along its path. Other
/// pools' cargo is not available until unloaded. Retained origins grant access
/// to real friendly local stocks, with resolved scenario rights unchanged.
/// Cases: airlog:49.15, airlog:49.16, airlog:49.18, airlog:49.2, airlog:57.0
pub fn available_pool_sources_at(
    content: &CnaContent,
    state: &State,
    id: &str,
    origin: &Location,
) -> Result<Vec<SupplyDraw>, SupplyError> {
    let p = pool(state, id)?;
    segment::validate_origin(origin)?;
    segment::validate_origin(p.location.as_ref().ok_or(SupplyError::Invalid)?)?;
    let mut sources = vec![
        SupplyDraw {
            source: SupplySource::PoolTank(id.into()),
            amount: SupplyDemand {
                fuel: p.tank_fuel,
                ..SupplyDemand::default()
            },
        },
        SupplyDraw {
            source: SupplySource::PoolStock(id.into()),
            amount: supply::stock_demand(p.cargo)?,
        },
    ];
    let mut local = supply::local_stocks(state, p.side, origin)?;
    if matches!(origin, Location::OffMap { id } if !content.areas.locations.contains_key(id)) {
        local.retain(|s| matches!(s.source, SupplySource::UnitStock(_)));
    }
    sources.extend(local);
    supply::add_unlimited_source(content, p.side, origin, &mut sources)?;
    Ok(sources)
}

/// Preview the cumulative movement CP of genuine pool truck cohorts. No troop
/// body contributes fuel; each truck point uses the printed truck rate.
/// Cases: airlog:49.13, airlog:49.16, airlog:49.18, airlog:53.12, airlog:53.22
/// Interpretations: interp:airlog-0001, interp:airlog-0018
pub fn plan_pool_segment_fuel(
    content: &CnaContent,
    state: &State,
    id: &str,
    total_cp_quarters: i32,
) -> Result<SegmentFuelPlan<String>, SupplyError> {
    let mut ledger = ledger(state, id)?;
    let costs = segment::cohort_costs(content, &mut ledger, &id.to_owned(), total_cp_quarters, 0)?;
    segment::fund_plan(
        ledger,
        total_cp_quarters,
        costs,
        |key, ledger| account(state, key, id, ledger),
        |account, used| {
            let sources = available_pool_sources_at(content, state, id, &account.origin)?
                .into_iter()
                .map(|s| (s.source, s.amount))
                .collect();
            segment::source_capacities(sources, account, used)
        },
    )
}

/// Atomically debit only the incremental fuel and preserve all historical draws.
/// Failed previews leave the caller's pools, shared accounts and unit maps untouched.
/// Cases: airlog:49.13, airlog:49.16, airlog:49.18
/// Interpretations: interp:airlog-0001, interp:airlog-0018
pub fn spend_pool_segment_fuel(
    content: &CnaContent,
    state: &mut State,
    id: &str,
    total_cp_quarters: i32,
) -> Result<Vec<SupplyDraw>, SupplyError> {
    let plan = plan_pool_segment_fuel(content, state, id, total_cp_quarters)?;
    let mut next = state.logistics.clone();
    for funding in &plan.funding {
        supply::withdraw_into(
            &mut next,
            None,
            SupplyDemand {
                fuel: funding.increment,
                ..SupplyDemand::default()
            },
            &funding.draws,
            &funding.sources,
            &funding.prior,
        )?;
        next.pool_fuel_accounts
            .insert(funding.id.clone(), funding.account.clone());
    }
    next.pool_fuel_segments.insert(id.into(), plan.ledger);
    state.logistics = next;
    Ok(plan.draws)
}

/// Physical pool identities for breakdown grouping, without changing inventory.
/// Cases: land:21.25, land:21.29, airlog:49.13
pub fn pool_segment_fuel_cohorts(
    state: &State,
    id: &str,
) -> Result<Vec<TruckFuelCohort<String>>, SupplyError> {
    Ok(ledger(state, id)?.cohorts)
}

/// Retire selected physical history before subtracting actual broken trucks.
/// Funding credit is retained once; returned cohorts accompany the broken marker.
/// Cases: land:21.25, land:21.29, airlog:49.13, airlog:49.16
/// Interpretations: interp:airlog-0018
pub fn remove_selected_pool_fuel_cohorts(
    state: &mut State,
    id: &str,
    selection: &[FuelCohortSelection],
) -> Result<Vec<TruckFuelCohort<String>>, SupplyError> {
    let mut ledger = ledger(state, id)?;
    let a = account(state, id, id, &ledger)?;
    let cohorts = segment::select_named(&mut ledger, &format!("pool.{id}"), selection)?;
    state.logistics.pool_fuel_accounts.insert(id.into(), a);
    state.logistics.pool_fuel_segments.insert(id.into(), ledger);
    Ok(cohorts)
}

/// Restore retained history before adding recovered trucks. Other segments reset
/// moving CP and funding responsibility, while physical identities remain stable.
/// Cases: land:21.25, land:21.29, airlog:49.13, airlog:49.16
/// Interpretations: interp:airlog-0018
pub fn restore_pool_fuel_cohorts(
    state: &mut State,
    id: &str,
    cohorts: &[TruckFuelCohort<String>],
) -> Result<(), SupplyError> {
    let mut ledger = ledger(state, id)?;
    let a = account(state, id, id, &ledger)?;
    let side = a.side;
    let mut known: BTreeSet<_> = state
        .logistics
        .pool_fuel_segments
        .values()
        .flat_map(|l| l.cohorts.iter().map(|c| c.id.clone()))
        .collect();
    known.extend(ledger.cohorts.iter().map(|c| c.id.clone()));
    for original in cohorts {
        if original.id.is_empty()
            || original.count <= 0
            || original.cp_quarters < 0
            || !known.insert(original.id.clone())
        {
            return Err(SupplyError::Invalid);
        }
        let mut c = original.clone();
        if c.segment != ledger.segment {
            c.segment = ledger.segment.clone();
            c.cp_quarters = 0;
            c.account = id.into();
        } else if !state
            .logistics
            .pool_fuel_accounts
            .get(&c.account)
            .is_some_and(|a| a.segment == ledger.segment && a.side == side)
        {
            return Err(SupplyError::Invalid);
        }
        ledger.cohorts.push(c);
    }
    state.logistics.pool_fuel_accounts.insert(id.into(), a);
    state.logistics.pool_fuel_segments.insert(id.into(), ledger);
    Ok(())
}

#[cfg(test)]
mod tests;
