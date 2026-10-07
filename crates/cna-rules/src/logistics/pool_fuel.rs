//! Fuel for actual second-/third-line carriers. Pool identities have separate
//! checkpointed maps; no land-unit record or troop-body rate is fabricated.
use super::{
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
    segment::carrier_ledger(
        state.logistics.pool_fuel_segments.get(id),
        SegmentKey::current(state),
        p.location.as_ref().ok_or(SupplyError::Invalid)?,
        &p.trucks,
        &id.to_owned(),
        &format!("pool.{id}"),
    )
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
        next = supply::withdraw_draws(
            &next,
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
