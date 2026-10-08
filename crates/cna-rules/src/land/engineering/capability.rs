//! Source identity and current-equipment gate only; action benefits are separate.
use crate::{CnaContent, state::State};
use cna_content::units::{EngineeringMetadata, EngineeringRole, EngineeringScope, Toe};
use cna_core::{engine::EngineError, ids::UnitId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineerCapability {
    pub scope: EngineeringScope,
    pub role: EngineeringRole,
}
fn invariant(id: &UnitId, detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("engineering capability {id}: {detail}"),
    }
}
fn unsupported(id: &UnitId, case: &str, detail: &str) -> EngineError {
    EngineError::Unsupported {
        case: case.into(),
        detail: format!("engineering capability {id}: {detail}"),
    }
}
/// Validate trusted metadata independently of any early negative result.
/// Cases: land:23.11, land:23.13, land:23.14, land:23.15, land:24.61
fn validate_metadata(
    content: &CnaContent,
    id: &UnitId,
    metadata: &EngineeringMetadata,
) -> Result<(), EngineError> {
    if metadata.src.is_empty() || metadata.src.iter().any(|case| case.trim().is_empty()) {
        return Err(invariant(id, "missing engineering source citations"));
    }
    if metadata.evidence.verification != "double"
        || metadata.evidence.transcribed_from.len() < 2
        || metadata
            .evidence
            .transcribed_from
            .iter()
            .any(|file| file.trim().is_empty())
    {
        return Err(invariant(id, "invalid engineering source evidence"));
    }
    match metadata.scope {
        EngineeringScope::None if metadata.role.is_some() || metadata.toe_requirement.is_some() => {
            return Err(invariant(
                id,
                "contradictory verified non-engineer metadata",
            ));
        }
        EngineeringScope::None => {}
        _ if metadata.role.is_none() => {
            return Err(invariant(id, "positive engineering scope has no role"));
        }
        _ => {}
    }
    if let Some(gate) = &metadata.toe_requirement
        && (metadata.scope != EngineeringScope::AntiMineOnly
            || gate.weapon != "cw.scorpion"
            || gate.min_points != 6
            || !content.units.weapons.contains_key(&gate.weapon))
    {
        return Err(invariant(id, "corrupt engineering TOE gate"));
    }
    Ok(())
}
/// Trusted current unit identity only. None is verified lack of usable engineering
/// capability, never an omitted source record or an unknown equipment composition.
/// Does not grant movement, construction, protection or assault benefits.
/// Cases: land:23.11, land:23.13, land:23.14, land:23.15, land:24.61
pub fn source_capability(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<Option<EngineerCapability>, EngineError> {
    let unit = state
        .land
        .units
        .get(id)
        .ok_or_else(|| invariant(id, "missing trusted runtime unit"))?;
    let source = content
        .units
        .units
        .get(id)
        .ok_or_else(|| invariant(id, "missing trusted content unit"))?;
    if &unit.id != id || &source.id != id {
        return Err(invariant(id, "stored runtime/content identity mismatch"));
    }
    let metadata = source.engineering.as_ref().ok_or_else(|| {
        unsupported(
            id,
            "land:23.11",
            "source-backed engineering identity is unresolved",
        )
    })?;
    validate_metadata(content, id, metadata)?;
    if metadata.scope == EngineeringScope::None {
        return Ok(None);
    }
    if metadata.scope == EngineeringScope::AntiMineOnly {
        let gate = metadata.toe_requirement.as_ref().ok_or_else(|| {
            unsupported(id, "land:23.15", "engineering equipment gate is unresolved")
        })?;
        let Some(Toe::Weapons(rows)) = &unit.toe else {
            return Err(unsupported(
                id,
                "land:23.15",
                "current identified equipment is unresolved",
            ));
        };
        // Check EVERY row first. A later/nonmatching invalid row cannot become a
        // verified zero or below-threshold result. Duplicate rows are additive.
        for row in rows {
            if row.n < 0 || !content.units.weapons.contains_key(&row.weapon) {
                return Err(invariant(id, "invalid current equipment row"));
            }
        }
        let points = rows
            .iter()
            .filter(|row| row.weapon == gate.weapon)
            .try_fold(0i32, |total, row| {
                total
                    .checked_add(row.n)
                    .ok_or_else(|| invariant(id, "current engineering weapon points overflow"))
            })?;
        if points < gate.min_points {
            return Ok(None);
        }
    }
    Ok(Some(EngineerCapability {
        scope: metadata.scope,
        role: metadata.role.expect("positive role validated above"),
    }))
}

#[cfg(test)]
mod tests;
