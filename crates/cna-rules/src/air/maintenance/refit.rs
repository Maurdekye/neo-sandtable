//! Owned refit/dues records and pure trusted-finish accounting.
//! No runtime field, decision handler, RNG, debit or readiness change here.
use std::collections::{BTreeMap, BTreeSet};

use cna_core::{
    engine::EngineError,
    quantity::{FuelTenths, StoresPoints},
};
use cna_protocol::Side;
use serde::{Deserialize, Serialize};

use super::invalid;
use crate::air::{
    facilities::FacilityId,
    sgsu::{SgsuId, SgsuOpStage},
    state::PlaneId,
};
use crate::logistics::{SupplyDraw, SupplySource};

/// Derived from the public cursor by the eventual caller, never an answer.
/// Cases: airlog:38.23, airlog:38.33
/// Interpretations: interp:airlog-0022
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "period", rename_all = "snake_case")]
pub enum MaintenancePeriod {
    Tactical { game_turn: u16, op_stage: u8 },
    Strategic { game_turn: u16 },
}

impl MaintenancePeriod {
    pub fn check(self) -> Result<(), EngineError> {
        match self {
            Self::Tactical {
                game_turn,
                op_stage,
            } if game_turn > 0 && (1..=3).contains(&op_stage) => Ok(()),
            Self::Strategic { game_turn } if game_turn > 0 => Ok(()),
            _ => Err(invalid("invalid trusted maintenance period")),
        }
    }

    /// Names the existing SGSU dues receipt, without creating a new payment.
    /// Strategic callers supply the actual completed stage; no legacy credit
    /// or invented fourth OpStage is inferred from an absent receipt.
    /// Cases: airlog:35.14
    /// Interpretations: interp:airlog-0025
    pub fn dues_stage(
        self,
        last_completed: Option<SgsuOpStage>,
    ) -> Result<SgsuOpStage, EngineError> {
        self.check()?;
        match self {
            Self::Tactical {
                game_turn,
                op_stage,
            } => Ok(SgsuOpStage {
                game_turn,
                op_stage,
            }),
            Self::Strategic { game_turn } => last_completed
                .filter(|stage| stage.game_turn == game_turn && (1..=3).contains(&stage.op_stage))
                .ok_or_else(|| EngineError::Unsupported {
                    case: "airlog:35.14".into(),
                    detail: "strategic refit has no verified completed-stage dues identity".into(),
                }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefitMethod {
    Squadron,
    PerPlane,
}

/// Aircraft nationality grouping is resolved from content before rolling.
/// This assigned-squadron identity also owns the single Stores charge; the
/// servicer identity is not a second squadron pricing identity.
/// Cases: airlog:38.34, airlog:38.35, airlog:38.36
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefitCohort {
    pub servicer: SgsuId,
    pub assigned_squadron: String,
    pub priority: Vec<PlaneId>,
    pub method: RefitMethod,
}

/// One exact finite consumer's draws, not an answer-supplied cost or credit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsumerDraws {
    pub servicer: SgsuId,
    pub draws: Vec<SupplyDraw>,
}

/// Refit, finite fuel and actual SGSU dues; no gun or ordnance loading fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefitPlan {
    pub dues: Vec<SgsuId>,
    pub cohorts: Vec<RefitCohort>,
    pub draws: Vec<ConsumerDraws>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fueling: Vec<FuelCohort>,
}

/// Explicit source mode and identity, never an answer-provided cost.
/// Cases: airlog:38.21, airlog:38.24
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FuelSelection {
    pub plane: PlaneId,
    pub mode: usize,
}

/// Refueling throughput is independent of refit attempts at the same SGSU.
/// Cases: airlog:38.22, airlog:38.23, airlog:38.25
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FuelCohort {
    pub servicer: SgsuId,
    pub planes: Vec<FuelSelection>,
}

/// Paid fuel service history, not ammunition or flight authorization.
/// Cases: airlog:38.23, airlog:38.24, airlog:38.25
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuelReceipt {
    pub period: MaintenancePeriod,
    pub cohort: FuelCohort,
}

/// Recorded only by a successful whole-State finish, including failed rolls.
/// Results are exact successful IDs, not a second aircraft readiness truth.
/// Cases: airlog:38.33, airlog:38.34, airlog:38.36
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefitReceipt {
    pub period: MaintenancePeriod,
    pub cohort: RefitCohort,
    pub successful: BTreeSet<PlaneId>,
    /// Absent in legacy and finite receipts: no source exemption is inferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::air) stores_entitlement: Option<SourceEntitlement>,
}

/// The logical refit Stores requirement satisfied at an exact 36.5 site.
/// This is not stock, an SGSU dues payment or a future service credit.
/// Cases: airlog:36.5, airlog:38.36
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::air) struct SourceEntitlement {
    pub facility: FacilityId,
    pub stores: StoresPoints,
}

pub(super) enum ComponentSupply {
    Finite,
    Entitled(SourceEntitlement),
}

/// Trusted debit history for the existing Air supply facade. A vector avoids
/// JSON object keys made from structured SupplySource values. Neither this
/// history nor an answer substitutes for SGSU paid-GT/paid-OpStage receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebitReceipt {
    pub servicer: SgsuId,
    pub draws: Vec<SupplyDraw>,
    pub fuel_rounding: Vec<(SupplySource, FuelTenths)>,
}

/// Only buffer waiting answers during Respond; service receipts and finished
/// belong to atomic finish. Fuel history is separate from refit throughput.
/// Existing SgsuState fields remain the sole dues truth.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaintenanceState {
    pub period: Option<MaintenancePeriod>,
    pub closed: BTreeSet<Side>,
    pub waiting: BTreeMap<Side, RefitPlan>,
    pub receipts: BTreeMap<Side, Vec<RefitReceipt>>,
    pub debits: BTreeMap<Side, Vec<DebitReceipt>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fuel_receipts: BTreeMap<Side, Vec<FuelReceipt>>,
    pub finished: bool,
}

fn period_key(period: MaintenancePeriod) -> (u16, u8) {
    match period {
        MaintenancePeriod::Tactical {
            game_turn,
            op_stage,
        } => (game_turn, op_stage),
        MaintenancePeriod::Strategic { game_turn } => (game_turn, 4),
    }
}

/// Structural receipts only: retained histories may name retired aircraft or
/// dumps. Current source/side/site eligibility belongs to the actual finish.
/// Cases: airlog:38.33, airlog:38.36
/// Interpretations: interp:airlog-0022
pub(crate) fn check(window: &MaintenanceState) -> Result<(), EngineError> {
    let Some(period) = window.period else {
        return if window == &MaintenanceState::default() {
            Ok(())
        } else {
            Err(invalid("maintenance state without a period"))
        };
    };
    period.check()?;
    if (window.finished && (!window.waiting.is_empty() || window.closed.len() != Side::ALL.len()))
        || (!window.finished
            && window.waiting.keys().copied().collect::<BTreeSet<_>>() != window.closed)
    {
        return Err(invalid("maintenance answer/finish markers disagree"));
    }
    let mut attempted = BTreeSet::new();
    let mut entitled_squadrons = BTreeSet::new();
    for receipt in window.receipts.values().flatten() {
        receipt.period.check()?;
        if period_key(receipt.period) > period_key(period) {
            return Err(invalid("future maintenance receipt"));
        }
        let ids: BTreeSet<_> = receipt.cohort.priority.iter().cloned().collect();
        if receipt.cohort.servicer.0.is_empty()
            || receipt.cohort.assigned_squadron.is_empty()
            || ids.iter().any(|id| id.0.is_empty())
            || ids.len() != receipt.cohort.priority.len()
            || !receipt.successful.is_subset(&ids)
        {
            return Err(invalid("invalid maintenance outcome IDs"));
        }
        if let Some(entitlement) = &receipt.stores_entitlement
            && (entitlement.facility.0.is_empty()
                || entitlement.stores.get() != 1
                || ids.is_empty()
                || !entitled_squadrons.insert((
                    period_key(receipt.period),
                    &receipt.cohort.assigned_squadron,
                )))
        {
            return Err(invalid("invalid or duplicate refit Stores entitlement"));
        }
        for id in ids {
            if !attempted.insert((period_key(receipt.period), id)) {
                return Err(invalid("duplicate period attempt in maintenance history"));
            }
        }
    }
    let mut fueled = BTreeSet::new();
    for receipt in window.fuel_receipts.values().flatten() {
        receipt.period.check()?;
        if period_key(receipt.period) > period_key(period)
            || receipt.cohort.servicer.0.is_empty()
            || receipt.cohort.planes.is_empty()
        {
            return Err(invalid("invalid fuel service history"));
        }
        for selected in &receipt.cohort.planes {
            if selected.plane.0.is_empty()
                || !fueled.insert((period_key(receipt.period), &selected.plane))
            {
                return Err(invalid("duplicate period fuel service in history"));
            }
        }
    }
    for receipt in window.debits.values().flatten() {
        let expected: Vec<_> = receipt
            .draws
            .iter()
            .map(|draw| (draw.source.clone(), draw.amount.fuel))
            .collect();
        if expected != receipt.fuel_rounding
            || receipt.draws.iter().any(|draw| {
                !matches!(&draw.source, SupplySource::AirDump(_))
                    || draw.amount.fuel.get() < 0
                    || draw.amount.stores.get() < 0
                    || draw.amount.water.get() < 0
                    || draw.amount.ammo.get() != 0
            })
        {
            return Err(invalid("finite maintenance debit/rounding history differs"));
        }
    }
    Ok(())
}

/// Histories stay append-only; completed periods cannot be reopened or moved
/// backward through the generic inventory callback. This is not an entitlement
/// to create a receipt: the trusted finish derives every debit/outcome.
/// Cases: airlog:38.33, airlog:38.36
/// Interpretations: interp:airlog-0022
pub(crate) fn check_transition(
    old: &MaintenanceState,
    new: &MaintenanceState,
) -> Result<(), EngineError> {
    check(new)?;
    if let Some(previous) = old.period {
        let current = new
            .period
            .ok_or_else(|| invalid("maintenance period cannot be cleared"))?;
        if period_key(current) < period_key(previous)
            || (current == previous && old.finished && !new.finished)
            || (current != previous && !old.finished)
        {
            return Err(invalid(
                "maintenance period or completed marker rolled backward",
            ));
        }
        if current == previous && old.finished && new != old {
            return Err(invalid("completed maintenance period is immutable"));
        }
    }
    for side in Side::ALL {
        let prior = old.receipts.get(&side).map(Vec::as_slice).unwrap_or(&[]);
        let next = new.receipts.get(&side).map(Vec::as_slice).unwrap_or(&[]);
        let old_debits = old.debits.get(&side).map(Vec::as_slice).unwrap_or(&[]);
        let new_debits = new.debits.get(&side).map(Vec::as_slice).unwrap_or(&[]);
        if !next.starts_with(prior) || !new_debits.starts_with(old_debits) {
            return Err(invalid("maintenance attempt/debit history rewritten"));
        }
        let old_fuel = old.fuel_receipts.get(&side).map(Vec::as_slice).unwrap_or(&[]);
        let new_fuel = new.fuel_receipts.get(&side).map(Vec::as_slice).unwrap_or(&[]);
        if !new_fuel.starts_with(old_fuel) {
            return Err(invalid("maintenance fuel history rewritten"));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefitAccounting {
    pub attempted: BTreeMap<SgsuId, BTreeSet<PlaneId>>,
    pub newly_charged_squadrons: BTreeSet<String>,
}

/// Validate trusted, already source/site/side-resolved cohorts against actual
/// bound ready+reserve limits and committed receipts. Never grants eligibility.
/// The caller separately validates physical sites, nationality, assignments,
/// dues, exemptions and stock; limits cannot originate in an answer.
/// One global ID union spans ALL servicers, including unsuccessful attempts.
/// Cases: airlog:38.33, airlog:38.36
/// Interpretations: interp:airlog-0022
pub fn account_refit(
    period: MaintenancePeriod,
    previous: &[RefitReceipt],
    cohorts: &[RefitCohort],
    limits: &BTreeMap<SgsuId, u32>,
) -> Result<RefitAccounting, EngineError> {
    period.check()?;
    let mut attempted: BTreeMap<SgsuId, BTreeSet<PlaneId>> = BTreeMap::new();
    let mut global = BTreeSet::new();
    let mut charged = BTreeSet::new();
    let mut servicers: BTreeMap<String, SgsuId> = BTreeMap::new();
    for receipt in previous {
        receipt.period.check()?;
        if receipt.period != period {
            continue;
        }
        let ids: BTreeSet<_> = receipt.cohort.priority.iter().cloned().collect();
        if !receipt.successful.is_subset(&ids) {
            return Err(invalid("refit receipt successes outside attempted IDs"));
        }
        include_cohort(&receipt.cohort, &mut attempted, &mut global, &mut servicers)?;
        if !receipt.cohort.priority.is_empty() {
            charged.insert(receipt.cohort.assigned_squadron.clone());
        }
    }
    let mut newly_charged_squadrons = BTreeSet::new();
    for cohort in cohorts {
        include_cohort(cohort, &mut attempted, &mut global, &mut servicers)?;
        if !cohort.priority.is_empty() && charged.insert(cohort.assigned_squadron.clone()) {
            newly_charged_squadrons.insert(cohort.assigned_squadron.clone());
        }
    }
    for (sgsu, ids) in &attempted {
        let limit = limits
            .get(sgsu)
            .ok_or_else(|| invalid("missing trusted actual-SGSU refit capacity"))?;
        if u64::try_from(ids.len()).map_err(|_| invalid("attempt count overflow"))?
            > u64::from(*limit)
        {
            return Err(invalid("refit attempts exceed SGSU ready+reserve"));
        }
    }
    Ok(RefitAccounting {
        attempted,
        newly_charged_squadrons,
    })
}

fn include_cohort(
    cohort: &RefitCohort,
    attempted: &mut BTreeMap<SgsuId, BTreeSet<PlaneId>>,
    global: &mut BTreeSet<PlaneId>,
    servicers: &mut BTreeMap<String, SgsuId>,
) -> Result<(), EngineError> {
    if cohort.assigned_squadron.is_empty() || cohort.servicer.0.is_empty() {
        return Err(invalid("empty trusted refit identity"));
    }
    if cohort.priority.is_empty() {
        return Ok(());
    }
    if servicers
        .get(&cohort.assigned_squadron)
        .is_some_and(|prior| prior != &cohort.servicer)
    {
        return Err(EngineError::Unsupported {
            case: "airlog:38.36".into(),
            detail: "assigned-squadron refit split across SGSUs needs a shared Stores allocation"
                .into(),
        });
    }
    servicers.insert(cohort.assigned_squadron.clone(), cohort.servicer.clone());
    for id in &cohort.priority {
        if !global.insert(id.clone()) {
            return Err(invalid(
                "PlaneId attempted more than once in maintenance period",
            ));
        }
        attempted
            .entry(cohort.servicer.clone())
            .or_default()
            .insert(id.clone());
    }
    Ok(())
}

/// Apply the predeclared priority to a trusted squadron success count. Per-plane
/// mode uses its actual individual roll results instead; no default thresholds.
/// Cases: airlog:38.34
/// Interpretations: interp:airlog-0023
pub fn successful_ids(priority: &[PlaneId], count: u32) -> Result<BTreeSet<PlaneId>, EngineError> {
    let distinct: BTreeSet<_> = priority.iter().collect();
    let count = usize::try_from(count).map_err(|_| invalid("success count exceeds usize"))?;
    if distinct.len() != priority.len() || count > priority.len() {
        return Err(invalid("invalid success priority or count"));
    }
    Ok(priority.iter().take(count).cloned().collect())
}

#[cfg(test)]
mod tests;
