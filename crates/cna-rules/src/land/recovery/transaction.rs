//! The private transaction boundary for a trusted, complete repair attempt.
//! Eligibility, grouping, physical outputs and disclosures remain caller obligations.
use super::{PreparedRepair, RecoveryClosureKey, RecoveryProcedure};
use crate::{CnaContent, State, logistics};
use cna_core::engine::{Cx, EngineError, Rejection};

fn invariant(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("repair transaction: {detail}"),
    }
}

/// Bind closure to the real cursor, never to an answer or an old snapshot.
/// Cases: land:22.12
pub(super) fn maintenance_key(state: &State) -> Result<RecoveryClosureKey, EngineError> {
    if state.cursor.anchor() != "opstage.repair.maintenance" {
        return Err(invariant("not at maintenance"));
    }
    let op_stage = state.cursor.op_stage.filter(|n| (1..=3).contains(n))
        .ok_or_else(|| invariant("missing Operations Stage"))?;
    let half = state.cursor.half.ok_or_else(|| invariant("missing player half"))?;
    let phasing = state.cursor.phasing(state.turn.player_a)
        .ok_or_else(|| invariant("missing phasing player"))?;
    Ok(RecoveryClosureKey {
        game_turn: state.cursor.game_turn,
        op_stage,
        half,
        cycle: state.cursor.cycle,
        phasing,
        procedure: RecoveryProcedure::Maintenance,
    })
}

pub(super) fn supply_error(error: logistics::RecoverySupplyError) -> Rejection {
    use logistics::{RecoverySupplyError, SupplyError};
    match error {
        RecoverySupplyError::Canonical(error) => Rejection::Engine(error),
        RecoverySupplyError::Supply(SupplyError::Insufficient) => Rejection::Illegal {
            message: "insufficient supplies for the complete repair attempt".into(),
        },
        RecoverySupplyError::Supply(SupplyError::Unsupported { case }) =>
            Rejection::Engine(EngineError::Unsupported {
                case: case.into(),
                detail: "repair requires an unavailable supply procedure".into(),
            }),
        RecoverySupplyError::Supply(SupplyError::Invalid | SupplyError::UnknownFuelRate) =>
            Rejection::Engine(invariant("invalid trusted supply footprint")),
    }
}

/// Prepay the WHOLE accepted attempt before allowing its first adjudication draw.
/// Only the recovery module can provide the trusted continuation. It must finish
/// rolls, all Oob outputs, physical writes, Airlog history relocation and safe Notes
/// before returning success. This helper grants no eligibility or outcome itself.
///
/// Any error, including one after a die or final output, preserves the caller's
/// entire State, RNG and existing event buffer. An unsuccessful repair RESULT is
/// still an accepted attempt: its continuation returns Ok and keeps paid costs.
/// Closure consumes the buffered plan; repeating that same closure draws nothing.
/// Cases: land:22.12, land:22.26, land:22.35, land:22.42, land:22.43
pub(super) fn finish_prepaid<T>(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
    prepared: &PreparedRepair,
    resolve: impl FnOnce(&mut State, &mut Cx<'_>) -> Result<T, Rejection>,
) -> Result<Option<T>, Rejection> {
    let key = maintenance_key(state)?;
    if prepared.actor() != key.phasing
        || state.land.recovery.opportunity.as_ref() != Some(&key)
    {
        return Err(invariant("actor or current opportunity changed").into());
    }
    if state.land.recovery.resolved {
        return Ok(None);
    }
    if state.land.recovery.repair_plan.is_none() {
        return Err(invariant("no buffered repair plan").into());
    }

    let debit = logistics::prepare_recovery_prepaid(content, state, prepared)
        .map_err(supply_error)?;
    let mut draft = state.clone();
    let mut rng = cx.rng.clone();
    let mut events = Vec::new();
    logistics::apply_recovery_prepaid(content, &mut draft, &debit).map_err(supply_error)?;
    let result = resolve(&mut draft, &mut Cx { rng: &mut rng, events: &mut events })?;
    // No continuation may change the clock or replace the opportunity it closes.
    if maintenance_key(&draft)? != key
        || draft.land.recovery.opportunity.as_ref() != Some(&key)
    {
        return Err(invariant("adjudication changed its closure").into());
    }
    draft.land.recovery.repair_plan = None;
    draft.land.recovery.resolved = true;
    *state = draft;
    *cx.rng = rng;
    cx.events.extend(events);
    Ok(Some(result))
}

#[cfg(test)]
mod tests;
