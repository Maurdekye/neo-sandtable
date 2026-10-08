//! Source qualification only; callers establish actual accompaniment separately.
use super::{EngineerCapability, source_capability};
use crate::{CnaContent, state::State};
use cna_content::units::{EngineeringRole, EngineeringScope};
use cna_core::{engine::EngineError, ids::UnitId};
use cna_protocol::Side;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinefieldCompanionBenefit {
    EnemyEntryCost,
    FriendlyEntryCost,
    EnemyVehicleLossProtection,
}

/// Qualifies a companion's source identity for one benefit under land-0037.
/// Restricted to existing original OA-backed UnitIds. A future authorized same-ID
/// side conversion requires renewed seam review; this is not a State validator.
/// Does not establish ownership, location, attachment, actual accompaniment, self
/// treatment, a CP price or a loss result. No caller/private surface is activated.
/// Cases: land:23.13, land:23.14, land:23.15, land:26.24, land:26.25
/// Interpretations: interp:land-0037
pub fn minefield_companion_qualifies(
    content: &CnaContent,
    state: &State,
    companion: &UnitId,
    benefit: MinefieldCompanionBenefit,
) -> Result<bool, EngineError> {
    // Raw errors take precedence even over a corrupted side or known-negative role.
    let capability = source_capability(content, state, companion)?;
    // The successful raw query validated both entries and their stored IDs.
    let side = state.land.units[companion].side;
    if side != content.units.units[companion].side {
        return Err(EngineError::Invariant {
            detail: format!("engineering companion {companion}: runtime/content side mismatch"),
        });
    }
    // Check the query-local identity guard BEFORE every verified negative result.
    let Some(EngineerCapability { scope, role }) = capability else {
        return Ok(false);
    };
    Ok(match (scope, role) {
        (
            EngineeringScope::General | EngineeringScope::AntiMineOnly,
            EngineeringRole::Battalion,
        ) => true,
        (EngineeringScope::General, EngineeringRole::Headquarters) => {
            side == Side::Commonwealth && benefit != MinefieldCompanionBenefit::FriendlyEntryCost
        }
        _ => false,
    })
}

#[cfg(test)]
mod tests;
