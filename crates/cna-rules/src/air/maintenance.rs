//! Pure source calculations, without selection, RNG, stock or readiness effects.
//! Cohorts, throughput, exact consumers and fixed maintenance windows belong to
//! later reviewed procedures; these results grant no maintenance eligibility.

use cna_content::units::Aircraft;
use cna_core::{dice::Die, engine::EngineError};
use cna_tables::airlog::air::{AircraftRefit, Nationality};

fn invalid(detail: impl Into<String>) -> EngineError {
    EngineError::Invariant {
        detail: format!("air maintenance: {}", detail.into()),
    }
}

/// Fuel for exactly the selected printed aircraft mode. Missing source data
/// cannot become a default cost, another mode, or an exemption from payment.
/// No fuel is debited and no aircraft is marked ready.
/// Cases: airlog:38.21, land:4.44
pub fn fuel_points(aircraft: &Aircraft, mode: usize) -> Result<i32, EngineError> {
    let selected = aircraft
        .modes
        .get(mode)
        .ok_or_else(|| invalid("mode index is outside the aircraft source"))?;
    let points = selected
        .fuel_points
        .ok_or_else(|| EngineError::Unsupported {
            case: "airlog:38.21".into(),
            detail: format!("fuel rating missing for {} mode {mode}", aircraft.id),
        })?;
    if points < 0 {
        return Err(invalid("negative trusted aircraft fuel rating"));
    }
    Ok(points)
}

/// Count successes for a trusted source-resolved cohort and an existing roll.
/// `nationality` is that of the aircraft, independent of the servicing SGSU.
/// Assignment is to that actual SGSU; table member names do not change this.
/// The caller owns grouping, identities of successes, Stores and throughput.
/// This function draws no RNG, mutates no state and does not authorize refit.
/// Cases: airlog:35.17, airlog:38.33, airlog:38.34, airlog:38.35, airlog:38.38
pub fn squadron_refitted(
    table: &AircraftRefit,
    nationality: Nationality,
    assigned_to_servicing_sgsu: bool,
    attempted: u32,
    roll: Die,
) -> Result<u32, EngineError> {
    if attempted == 0 {
        return Ok(0);
    }
    let modifiers = table.squadron_modifiers();
    let national = match nationality {
        Nationality::Commonwealth => 0,
        Nationality::German => modifiers.german_sgsu,
        Nationality::Italian => modifiers.italian_sgsu,
    };
    let foreign = if assigned_to_servicing_sgsu {
        0
    } else {
        modifiers.planes_not_assigned_to_refitting_sgsu
    };
    let modified = i32::from(roll.value())
        .checked_add(national)
        .and_then(|n| n.checked_add(foreign))
        .ok_or_else(|| invalid("refit roll modifier overflow"))?;
    let percent = table.squadron_percent_refitted(modified).ok_or_else(|| {
        invalid(format!(
            "validated refit table has no modified roll {modified}"
        ))
    })?;
    if !(0..=100).contains(&percent) {
        return Err(invalid(format!(
            "refit percentage {percent} outside 0..100"
        )));
    }
    let product = u64::from(attempted)
        .checked_mul(u64::try_from(percent).map_err(|_| invalid("negative refit percentage"))?)
        .ok_or_else(|| invalid("refit multiplication overflow"))?;
    let rounded = product
        .checked_add(99)
        .ok_or_else(|| invalid("refit rounding overflow"))?
        / 100;
    let success = u32::try_from(rounded).map_err(|_| invalid("refit result exceeds u32"))?;
    if success > attempted {
        return Err(invalid("refit result exceeds attempted aircraft"));
    }
    Ok(success)
}

#[cfg(test)]
mod tests;
