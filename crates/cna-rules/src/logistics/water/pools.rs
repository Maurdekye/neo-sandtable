//! Stock-only activity reserve issue for real pools; no CPA or cargo conversion.
use super::*;
use crate::state::{Location, TruckPool};
use std::collections::{BTreeMap, BTreeSet};

fn invariant(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("pool water issue: {detail}"),
    }
}

fn own_pool<'a>(state: &'a State, actor: Side, id: &str) -> Result<&'a TruckPool, Rejection> {
    // Missing and foreign ids have the same refusal before site or stock resolution.
    if id.is_empty()
        || !state
            .logistics
            .truck_pools
            .iter()
            .any(|p| p.id == id && p.side == actor)
    {
        return Err(illegal("unknown own water pool"));
    }
    let mut matches = state.logistics.truck_pools.iter().filter(|p| p.id == id);
    let p = matches.next().unwrap();
    if matches.next().is_some() {
        return Err(Rejection::Engine(invariant("duplicate pool identity")));
    }
    Ok(p)
}

fn location<'a>(content: &CnaContent, p: &'a TruckPool) -> Result<&'a Location, Rejection> {
    let l = p
        .location
        .as_ref()
        .filter(|l| l.hex().is_some())
        .ok_or_else(|| illegal("pool water issue requires a resolved on-map pool"))?;
    if l.hex().and_then(|h| content.map.get(h)).is_none() {
        return Err(Rejection::Engine(invariant("pool location is unmapped")));
    }
    Ok(l)
}

/// One-stage demand is global weather, with no source or paid-use ledger.
/// Cases: airlog:52.42, airlog:52.43, land:29.34, land:29.35
/// Interpretations: interp:airlog-0021
pub fn activity_need(
    content: &CnaContent,
    state: &State,
    actor: Side,
    id: &str,
) -> Result<i32, Rejection> {
    let p = own_pool(state, actor, id)?;
    location(content, p)?;
    let mut total = 0_i32;
    for count in [p.trucks.light, p.trucks.medium, p.trucks.heavy] {
        if count < 0 {
            return Err(Rejection::Engine(invariant("negative truck count")));
        }
        total = total
            .checked_add(count)
            .ok_or_else(|| Rejection::Engine(invariant("truck count overflow")))?;
    }
    if p.activity_water.get() < 0 {
        return Err(Rejection::Engine(invariant("negative activity reserve")));
    }
    let weather =
        state.turn.weather.as_ref().ok_or_else(|| {
            Rejection::Engine(engine(SupplyError::Unsupported { case: "land:29.1" }))
        })?;
    let multiplier = if weather.kind == cna_tables::land::weather::WeatherKind::Hot {
        2
    } else {
        1
    };
    let demand = total
        .checked_mul(multiplier)
        .ok_or_else(|| Rejection::Engine(invariant("water demand overflow")))?;
    Ok(demand.saturating_sub(p.activity_water.get()).max(0))
}

fn trusted(error: Rejection) -> EngineError {
    match error {
        Rejection::Engine(error) => error,
        _ => invariant("eligible pool validation failed"),
    }
}

/// Unresolved and off-map pools are outside this stock-issue window.
/// Cases: airlog:52.42, land:3.6
pub fn candidates(
    content: &CnaContent,
    state: &State,
    actor: Side,
) -> Result<Vec<String>, EngineError> {
    let mut ids = vec![];
    let mut seen = BTreeSet::new();
    for p in state
        .logistics
        .truck_pools
        .iter()
        .filter(|p| p.side == actor)
    {
        if p.id.is_empty() || !seen.insert(p.id.clone()) {
            return Err(invariant("duplicate or empty own pool identity"));
        }
        if !p.location.as_ref().is_some_and(|l| l.hex().is_some()) {
            continue;
        }
        if activity_need(content, state, actor, &p.id).map_err(trusted)? > 0 {
            ids.push(p.id.clone());
        }
    }
    ids.sort();
    Ok(ids)
}

/// Same-location first-line cargo and active dumps only, never pool cargo or wells.
/// Cases: airlog:52.0, airlog:52.42
pub fn sources(
    content: &CnaContent,
    state: &State,
    actor: Side,
    id: &str,
) -> Result<Vec<SupplyDraw>, Rejection> {
    let p = own_pool(state, actor, id)?;
    let l = location(content, p)?;
    super::super::supply::local_stocks(state, actor, l).map_err(|e| Rejection::Engine(engine(e)))
}

/// Validate/debit a disposable owner draft or a trusted whole finish draft.
/// This helper creates no opportunity to issue again; the fixed batch owns that guard.
/// Cases: airlog:52.42, land:29.34, land:29.35, land:3.6
/// Interpretations: interp:airlog-0021
pub fn issue(
    content: &CnaContent,
    state: &mut State,
    actor: Side,
    id: &str,
    activity: i32,
    draws: &[SupplyDraw],
) -> Result<(), Rejection> {
    let need = activity_need(content, state, actor, id)?;
    if activity < 0 || activity > need {
        return Err(illegal("pool water issue exceeds current unmet demand"));
    }
    let held = own_pool(state, actor, id)?.activity_water.get();
    let new = held
        .checked_add(activity)
        .ok_or_else(|| Rejection::Engine(invariant("reserve overflow")))?;
    let available: BTreeMap<_, _> = sources(content, state, actor, id)?
        .into_iter()
        .map(|d| (d.source, d.amount))
        .collect();
    let mut draft = super::super::supply::withdraw_draws(
        &state.logistics,
        None,
        SupplyDemand {
            water: WaterPoints::new(activity),
            ..SupplyDemand::default()
        },
        draws,
        &available,
        &BTreeMap::new(),
    )
    .map_err(|e| match e {
        SupplyError::Unsupported { .. } => Rejection::Engine(engine(e)),
        SupplyError::UnknownFuelRate => {
            Rejection::Engine(invariant("water-only issue requested a fuel rate"))
        }
        _ => illegal("water cannot be drawn from these friendly stocks"),
    })?;
    draft
        .truck_pools
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap()
        .activity_water = WaterPoints::new(new);
    state.logistics = draft;
    Ok(())
}

/// Exact unmet demand, stable ids and finite authorized stock; shortage skips a row.
/// Errors remain fallible rather than being converted to a pass by a controller.
/// Cases: airlog:52.42, land:29.34, land:29.35, land:3.6
pub fn baseline(
    content: &CnaContent,
    state: &mut State,
    actor: Side,
) -> Result<Vec<Value>, EngineError> {
    let mut rows = vec![];
    for id in candidates(content, state, actor)? {
        let need = activity_need(content, state, actor, &id).map_err(trusted)?;
        let mut available = sources(content, state, actor, &id).map_err(trusted)?;
        available.sort_by(|a, b| a.source.cmp(&b.source));
        let mut left = need;
        let mut allocation = vec![];
        let mut encoded = vec![];
        for source in available {
            let n = left.min(source.amount.water.get());
            if n <= 0 {
                continue;
            }
            left -= n;
            encoded.push(serde_json::json!({"source":serde_json::to_string(&source.source).map_err(|_| invariant("source cannot be encoded"))?,"stores":0,"water":n}));
            allocation.push(SupplyDraw {
                source: source.source,
                amount: SupplyDemand {
                    water: WaterPoints::new(n),
                    ..SupplyDemand::default()
                },
            });
        }
        if left != 0 {
            continue;
        }
        issue(content, state, actor, &id, need, &allocation).map_err(trusted)?;
        rows.push(serde_json::json!({"pool":id,"activity":need,"draws":encoded}));
    }
    Ok(rows)
}

#[cfg(test)]
mod tests;
