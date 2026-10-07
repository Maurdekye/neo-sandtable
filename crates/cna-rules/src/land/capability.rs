//! Capability expenditure and cohesion use quarters throughout. No answer-boundary rounding.
use crate::state::{LandUnit, State};
use crate::steps::illegal;
use cna_core::engine::Rejection;

/// A resolved allowance for the unit or its entire represented formation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allowance {
    pub cpa: i32,
    pub motorized: bool,
}

/// Validate movement eligibility and voluntary expenditure independently of reaction spending.
/// Cases: land:6.11, land:6.13, land:6.14, land:6.26, land:8.17
/// Interpretations: interp:land-0001, interp:land-0020
pub fn validate_move(
    unit: &LandUnit,
    allowance: Allowance,
    quarters: i32,
) -> Result<(), Rejection> {
    if quarters <= 0 || allowance.cpa <= 0 || unit.cohesion_quarters <= -26 * 4 {
        return Err(illegal("unit cannot move"));
    }
    super::reserve::validate_cp(unit, allowance, quarters, true)?;
    if !allowance.motorized {
        let ceiling = i64::from(allowance.cpa) * 6;
        if i64::from(unit.voluntary_cp_quarters) + i64::from(quarters) > ceiling {
            return Err(illegal("movement exceeds the voluntary CP ceiling"));
        }
    }
    Ok(())
}

/// Charge all CP immediately; apply only newly earned disorganization, preserving fractional DP.
/// Non-movement procedures can use this for involuntary movement or activity costs as well.
/// Cases: land:6.12, land:6.13, land:6.14, land:6.21, land:6.22, land:6.29
/// Interpretations: interp:land-0020
pub fn charge(
    unit: &mut LandUnit,
    allowance: Allowance,
    quarters: i32,
    voluntary: bool,
) -> Result<(), Rejection> {
    if quarters < 0 || allowance.cpa < 0 {
        return Err(illegal("invalid CP expenditure"));
    }
    super::reserve::validate_cp(unit, allowance, quarters, voluntary)?;
    let total = unit
        .cp_spent_quarters
        .checked_add(quarters)
        .ok_or_else(|| illegal("CP expenditure overflow"))?;
    let cpa = allowance
        .cpa
        .checked_mul(4)
        .ok_or_else(|| illegal("CPA overflow"))?;
    let newly_excess =
        total.saturating_sub(cpa).max(0) - unit.cp_spent_quarters.saturating_sub(cpa).max(0);
    let cohesion = unit
        .cohesion_quarters
        .checked_sub(newly_excess)
        .ok_or_else(|| illegal("cohesion overflow"))?;
    let voluntary_total = if voluntary {
        unit.voluntary_cp_quarters
            .checked_add(quarters)
            .ok_or_else(|| illegal("CP expenditure overflow"))?
    } else {
        unit.voluntary_cp_quarters
    };
    unit.cp_spent_quarters = total;
    unit.voluntary_cp_quarters = voluntary_total;
    unit.cohesion_quarters = cohesion;
    Ok(())
}

/// Idle units recover five cohesion, capped at zero. CP never carries into the next OpStage.
/// Training and rail travel disqualify idle recovery even when no CP was charged.
/// Cases: land:6.16, land:6.23, land:6.24, land:8.73
pub fn finish_opstage(state: &mut State) {
    state.land.assault_intentions.clear();
    state.land.breakdown.accumulated_quarters.clear();
    state.land.breakdown.light_extra_quarters.clear();
    state.land.breakdown.checked.clear();
    state.land.breakdown.moving.clear();
    for unit in state.land.units.values_mut() {
        let in_play = matches!(
            unit.location,
            crate::state::Location::Hex { .. } | crate::state::Location::OffMap { .. }
        );
        if in_play
            && unit.cp_spent_quarters == 0
            && !unit.no_idle_recovery
            && unit.cohesion_quarters < 0
        {
            unit.cohesion_quarters = (unit.cohesion_quarters + 5 * 4).min(0);
        }
        unit.reserve = super::reserve::ReserveState::default();
        unit.cp_spent_quarters = 0;
        unit.voluntary_cp_quarters = 0;
        unit.no_idle_recovery = false;
    }
}

/// Reaction and retreat charge stage CP and fatigue without the own-half voluntary ceiling.
/// Cases: land:6.14, land:6.26, land:8.17, land:8.52
pub fn validate_nonphasing_move(
    unit: &LandUnit,
    allowance: Allowance,
    quarters: i32,
) -> Result<(), Rejection> {
    if quarters <= 0 || allowance.cpa <= 0 || unit.cohesion_quarters <= -104 {
        return Err(illegal("unit cannot move"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CnaContent;
    fn unit() -> LandUnit {
        let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        State::new(&content).unwrap().land.units[&"cw.unassigned_inf.1st_rnf_mg_bn".into()].clone()
    }
    /// Cases: land:6.14, land:6.21, land:6.22, land:8.17
    /// Interpretations: interp:land-0001
    #[test]
    fn worked_examples_and_ceiling_keep_involuntary_spending_separate() {
        let mut u = unit();
        let a = Allowance {
            cpa: 8,
            motorized: false,
        };
        charge(&mut u, a, 6 * 4, false).unwrap();
        assert_eq!(u.cp_spent_quarters, 24);
        assert_eq!(u.voluntary_cp_quarters, 0);
        assert!(validate_move(&u, a, 12 * 4).is_ok());
        assert!(validate_move(&u, a, 13 * 4).is_err());
        u.cp_spent_quarters = 5 * 4;
        u.cohesion_quarters = -2 * 4;
        charge(&mut u, a, 5 * 4, true).unwrap();
        assert_eq!(u.cohesion_quarters, -4 * 4);
        charge(&mut u, a, 1, true).unwrap();
        assert_eq!(u.cohesion_quarters, -17);
    }
    /// Cases: land:6.22, land:6.26
    /// Interpretations: interp:land-0020
    #[test]
    fn split_fractional_actions_have_identical_fatigue_and_stop_threshold() {
        let mut split = unit();
        split.cp_spent_quarters = 8 * 4;
        let mut pooled = split.clone();
        let a = Allowance {
            cpa: 8,
            motorized: false,
        };
        for _ in 0..4 {
            charge(&mut split, a, 1, true).unwrap();
        }
        charge(&mut pooled, a, 4, true).unwrap();
        assert_eq!(split, pooled);
        pooled.cohesion_quarters = -104;
        assert!(validate_move(&pooled, a, 1).is_err());
        pooled.cohesion_quarters = -103;
        assert!(validate_move(&pooled, a, 1).is_ok());
    }
    /// Cases: land:6.16, land:6.23, land:6.24, land:8.73
    #[test]
    fn stage_reset_restores_only_eligible_idle_units_and_keeps_cohesion() {
        let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&content).unwrap();
        let id = s
            .land
            .units
            .values()
            .find(|u| matches!(u.location, crate::state::Location::Hex { .. }))
            .unwrap()
            .id
            .clone();
        let u = s.land.units.get_mut(&id).unwrap();
        u.cohesion_quarters = -4;
        finish_opstage(&mut s);
        assert_eq!(s.land.units[&id].cohesion_quarters, 0);
        let u = s.land.units.get_mut(&id).unwrap();
        u.cohesion_quarters = -28;
        u.cp_spent_quarters = 1;
        u.voluntary_cp_quarters = 1;
        finish_opstage(&mut s);
        assert_eq!(s.land.units[&id].cohesion_quarters, -28);
        assert_eq!(s.land.units[&id].cp_spent_quarters, 0);
        let u = s.land.units.get_mut(&id).unwrap();
        u.no_idle_recovery = true;
        finish_opstage(&mut s);
        assert_eq!(s.land.units[&id].cohesion_quarters, -28);
    }
}
