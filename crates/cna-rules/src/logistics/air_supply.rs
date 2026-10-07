//! Exact facility Air-dump access; callers own costs and flight eligibility.
//! No source migration, inferred unlimited stocks or live Land routing.
use super::{SupplyDemand, SupplyDraw, SupplyError, SupplySource};
use crate::air::{
    facilities::FacilityId,
    sgsu::{SgsuId, SgsuPosition},
};
use crate::{CnaContent, State};
use cna_content::scenario::Supplies;
use cna_core::{engine::EngineError, quantity::FuelTenths};
use cna_protocol::Side;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Named finite stocks at exactly one canonical facility, not an African hex.
/// Any friendly SGSU at that facility may access its side's stock.
/// Cases: airlog:36.17
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirDump {
    pub id: String,
    pub facility: FacilityId,
    pub side: Side,
    pub supplies: Supplies,
}

/// This identifies the supply procedure, without granting its eligibility.
/// Facility-only exceptions need a distinct source-backed consumer; a missing
/// SGSU or an off-map location never becomes an implicit exemption here.
/// Cases: airlog:35.14, airlog:38.4
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirSupplyUse {
    SgsuOperation,
    AircraftServicing,
}

/// Foreign/missing consumers stay generic; own canonical errors retain case/detail.
#[derive(Debug, Clone, PartialEq)]
pub enum AirSupplyError {
    Supply(SupplyError),
    Canonical(EngineError),
}
impl From<SupplyError> for AirSupplyError {
    fn from(error: SupplyError) -> Self {
        Self::Supply(error)
    }
}

fn consumer_facility(
    content: &CnaContent,
    state: &State,
    owner: Side,
    consumer: &SgsuId,
) -> Result<FacilityId, AirSupplyError> {
    // Ownership comes before any hidden facility or stock resolution.
    let sgsu = state
        .air
        .runtime
        .sgsus
        .get(consumer)
        .ok_or(SupplyError::Invalid)?;
    if sgsu.side().ok() != Some(owner) {
        return Err(SupplyError::Invalid.into());
    }
    if !state.air.runtime.bases_initialized {
        return Err(SupplyError::Unsupported {
            case: "airlog:35.11",
        }
        .into());
    }
    let SgsuPosition::Facility(id) = &sgsu.position else {
        return Err(SupplyError::Invalid.into());
    };
    sgsu.location(content, &state.air.runtime.facilities)
        .map_err(AirSupplyError::Canonical)?;
    Ok(id.clone())
}

/// Owner-private sources; exact site identity survives co-location and upgrade.
/// Facility ownership does not restrict non-denominational use (36.15).
/// Prior is the caller's trusted period ledger, never an answer-provided credit.
/// Cases: airlog:36.15, airlog:36.17, airlog:49.15
pub fn preview(
    content: &CnaContent,
    state: &State,
    owner: Side,
    consumer: &SgsuId,
    _use_: AirSupplyUse,
    prior: &BTreeMap<SupplySource, FuelTenths>,
) -> Result<Vec<SupplyDraw>, AirSupplyError> {
    let facility = consumer_facility(content, state, owner, consumer)?;
    if prior.values().any(|n| n.get() < 0) {
        return Err(SupplyError::Invalid.into());
    }
    let mut sources = Vec::new();
    for (id, dump) in &state.logistics.air_dumps {
        if dump.side != owner || dump.facility != facility {
            continue;
        }
        if id != &dump.id {
            return Err(SupplyError::Invalid.into());
        }
        let source = SupplySource::AirDump(id.clone());
        let mut amount = super::supply::stock_demand(dump.supplies)?;
        let paid = prior.get(&source).copied().unwrap_or_default().get();
        let credit = (10 - paid % 10) % 10;
        amount.fuel = FuelTenths::new(
            amount
                .fuel
                .get()
                .checked_add(credit)
                .ok_or(SupplyError::Invalid)?,
        );
        sources.push(SupplyDraw { source, amount });
    }
    Ok(sources)
}

/// Caller-owned exact debit and its trusted source-rounding ledger.
pub struct AirSupplyDebit<'a> {
    pub demand: SupplyDemand,
    pub draws: &'a [SupplyDraw],
    pub prior: &'a BTreeMap<SupplySource, FuelTenths>,
}

/// Revalidate and debit on a disposable logistics draft, then publish once.
/// The enclosing finish also commits its source-derived costs/period ledger;
/// this facade neither opens windows nor mutates aircraft/SGSU readiness.
/// Cases: airlog:36.17, airlog:49.15, airlog:53.25
pub fn spend(
    content: &CnaContent,
    state: &mut State,
    owner: Side,
    consumer: &SgsuId,
    use_: AirSupplyUse,
    debit: AirSupplyDebit<'_>,
) -> Result<(), AirSupplyError> {
    let AirSupplyDebit {
        demand,
        draws,
        prior,
    } = debit;
    let sources = preview(content, state, owner, consumer, use_, prior)?
        .into_iter()
        .map(|d| (d.source, d.amount))
        .collect();
    let next =
        super::supply::withdraw_draws(&state.logistics, None, demand, draws, &sources, prior)?;
    state.logistics = next;
    Ok(())
}

#[cfg(test)]
mod tests;
