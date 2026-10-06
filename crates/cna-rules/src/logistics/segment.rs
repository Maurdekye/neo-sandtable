//! Incremental movement fuel, retaining whole-point source-rounding credit.

use super::supply::{
    SupplyDemand, SupplyDraw, SupplyError, SupplySource, apply_draws, available_sources_at,
    movement_fuel_cost,
};
use crate::seq::Half;
use crate::{CnaContent, State};
use cna_core::ids::{HexId, UnitId};
use cna_core::quantity::FuelTenths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A segment identity from the engine cursor; a later segment starts a fresh ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentKey {
    pub game_turn: u16,
    pub op_stage: Option<u8>,
    pub half: Option<Half>,
    pub cycle: u16,
}
impl SegmentKey {
    fn current(state: &State) -> Self {
        Self {
            game_turn: state.cursor.game_turn,
            op_stage: state.cursor.op_stage,
            half: state.cursor.half,
            cycle: state.cursor.cycle,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuelDraw {
    pub source: SupplySource,
    pub fuel: FuelTenths,
}

/// Exact accumulated cost and source draws for a single unit's movement segment.
/// Source draws use a vector because tagged source identities are not JSON object keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuelSegmentLedger {
    pub segment: SegmentKey,
    pub origin: HexId,
    pub cp_quarters: i32,
    pub paid_cost: FuelTenths,
    pub draws: Vec<FuelDraw>,
}

/// Read-only plan; committing recomputes it from the current trusted state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentFuelPlan {
    pub ledger: FuelSegmentLedger,
    pub increment: FuelTenths,
    pub draws: Vec<SupplyDraw>,
}

fn prior_draws(
    ledger: &FuelSegmentLedger,
) -> Result<BTreeMap<SupplySource, FuelTenths>, SupplyError> {
    let mut prior = BTreeMap::new();
    for draw in &ledger.draws {
        if draw.fuel.get() < 0 || matches!(draw.source, SupplySource::ReadyAmmo) {
            return Err(SupplyError::Invalid);
        }
        let old: FuelTenths = prior.get(&draw.source).copied().unwrap_or_default();
        let new = old
            .get()
            .checked_add(draw.fuel.get())
            .ok_or(SupplyError::Invalid)?;
        prior.insert(draw.source.clone(), FuelTenths::new(new));
    }
    Ok(prior)
}

fn rounded_credit(source: &SupplySource, previous: FuelTenths) -> FuelTenths {
    if matches!(source, SupplySource::Tank | SupplySource::ReadyAmmo) {
        return FuelTenths::ZERO;
    }
    // Divide before multiplying so the credit stays representable at i32::MAX.
    FuelTenths::new((10 - previous.get().rem_euclid(10)) % 10)
}

fn ledger_for(state: &State, id: &UnitId) -> Result<FuelSegmentLedger, SupplyError> {
    let segment = SegmentKey::current(state);
    if let Some(ledger) = state
        .logistics
        .fuel_segments
        .get(id)
        .filter(|l| l.segment == segment)
    {
        return Ok(ledger.clone());
    }
    let origin = state
        .land
        .units
        .get(id)
        .and_then(|u| u.location.hex())
        .ok_or(SupplyError::Invalid)?
        .clone();
    Ok(FuelSegmentLedger {
        segment,
        origin,
        cp_quarters: 0,
        paid_cost: FuelTenths::ZERO,
        draws: Vec::new(),
    })
}

fn capacities(
    state: &State,
    id: &UnitId,
    ledger: &FuelSegmentLedger,
) -> Result<BTreeMap<SupplySource, SupplyDemand>, SupplyError> {
    let mut sources: BTreeMap<_, _> = available_sources_at(state, id, &ledger.origin)?
        .into_iter()
        .map(|s| (s.source, s.amount))
        .collect();
    for (source, previous) in prior_draws(ledger)? {
        let credit = rounded_credit(&source, previous);
        if credit.is_zero() {
            continue;
        }
        let amount = sources.entry(source).or_default();
        amount.fuel = FuelTenths::new(
            amount
                .fuel
                .get()
                .checked_add(credit.get())
                .ok_or(SupplyError::Invalid)?,
        );
    }
    Ok(sources)
}

/// Price cumulative movement CP within this segment and allocate only the increase.
/// Previously rounded source credit is consumed before new fuel. Origin stocks
/// remain the only external sources, even after the unit moves elsewhere.
/// Cases: airlog:49.13, airlog:49.15, airlog:49.16
/// Interpretations: interp:airlog-0001
pub fn plan_segment_fuel(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    total_cp_quarters: i32,
) -> Result<SegmentFuelPlan, SupplyError> {
    let mut ledger = ledger_for(state, id)?;
    if total_cp_quarters < ledger.cp_quarters {
        return Err(SupplyError::Invalid);
    }
    let total = movement_fuel_cost(content, state, id, total_cp_quarters)?;
    let increment = total
        .checked_sub(ledger.paid_cost)
        .ok_or(SupplyError::Invalid)?;
    let prior = prior_draws(&ledger)?;
    let mut sources: Vec<_> = capacities(state, id, &ledger)?.into_iter().collect();
    sources.sort_by_key(|(source, _)| {
        (
            rounded_credit(source, prior.get(source).copied().unwrap_or_default()).is_zero(),
            source.clone(),
        )
    });
    let mut need = increment;
    let mut draws = Vec::new();
    for (source, capacity) in sources {
        let fuel = FuelTenths::new(capacity.fuel.get().min(need.get()));
        if fuel.is_zero() {
            continue;
        }
        need -= fuel;
        draws.push(SupplyDraw {
            source,
            amount: SupplyDemand {
                fuel,
                ..SupplyDemand::default()
            },
        });
    }
    if !need.is_zero() {
        return Err(SupplyError::Insufficient);
    }
    let mut combined = prior;
    for draw in &draws {
        let old = combined.get(&draw.source).copied().unwrap_or_default();
        let new = old
            .get()
            .checked_add(draw.amount.fuel.get())
            .ok_or(SupplyError::Invalid)?;
        combined.insert(draw.source.clone(), FuelTenths::new(new));
    }
    ledger.cp_quarters = total_cp_quarters;
    ledger.paid_cost = total;
    ledger.draws = combined
        .into_iter()
        .map(|(source, fuel)| FuelDraw { source, fuel })
        .collect();
    Ok(SegmentFuelPlan {
        ledger,
        increment,
        draws,
    })
}

/// Commit a recomputed segment plan, preserving exact tanks and cumulative
/// whole-point source charges. Failure leaves both holdings and ledger untouched.
/// The movement caller passes total moving CP, excluding combat/non-movement CP.
/// Cases: airlog:49.13, airlog:49.15, airlog:49.16
/// Interpretations: interp:airlog-0001
pub fn spend_segment_fuel(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    total_cp_quarters: i32,
) -> Result<Vec<SupplyDraw>, SupplyError> {
    let before = ledger_for(state, id)?;
    let plan = plan_segment_fuel(content, state, id, total_cp_quarters)?;
    let sources = capacities(state, id, &before)?;
    let prior = prior_draws(&before)?;
    let mut next = apply_draws(
        state,
        id,
        SupplyDemand {
            fuel: plan.increment,
            ..SupplyDemand::default()
        },
        &plan.draws,
        &sources,
        &prior,
    )?;
    next.fuel_segments.insert(id.clone(), plan.ledger);
    state.logistics = next;
    Ok(plan.draws)
}

#[cfg(test)]
mod tests;
