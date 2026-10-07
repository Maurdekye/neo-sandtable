//! Operator-only run controls. Estimates never enter game adjudication.
#![allow(clippy::float_arithmetic)]
use cna_core::ids::SeatId;
use cna_server::RunBoundary;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunControl {
    pub boundary: RunBoundary,
    pub budget: SpendBudget,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "unit", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpendBudget {
    ReportedUsd {
        global: f64,
        allocations: BTreeMap<SeatId, f64>,
    },
    Tokens {
        global: u64,
        allocations: BTreeMap<SeatId, u64>,
    },
}
#[derive(Debug, PartialEq)]
pub enum Admission {
    Ready {
        remaining_usd: Option<f64>,
        remaining_tokens: Option<u64>,
    },
    Stop(String),
}
impl RunControl {
    pub fn validate(&self, seats: &[SeatId]) -> Result<(), String> {
        if self.boundary.game_turn == 0
            || self
                .boundary
                .op_stage
                .is_some_and(|s| !(1..=3).contains(&s))
        {
            return Err("invalid game-turn/OpStage boundary".into());
        }
        self.budget.validate(seats)
    }
}
impl SpendBudget {
    pub fn usd(
        global: f64,
        seats: &[SeatId],
        explicit: BTreeMap<SeatId, f64>,
    ) -> Result<Self, String> {
        if seats.is_empty() || !global.is_finite() || global <= 0. {
            return Err("positive reported-USD cap and model seats required".into());
        }
        let allocations = if explicit.is_empty() {
            {
                let share = global / seats.len() as f64;
                let mut assigned = 0.;
                seats
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let value = if i + 1 == seats.len() {
                            global - assigned
                        } else {
                            share
                        };
                        assigned += value;
                        (*s, value)
                    })
                    .collect()
            }
        } else {
            explicit
        };
        let budget = Self::ReportedUsd {
            global,
            allocations,
        };
        budget.validate(seats)?;
        Ok(budget)
    }
    pub fn tokens(
        global: u64,
        seats: &[SeatId],
        explicit: BTreeMap<SeatId, u64>,
    ) -> Result<Self, String> {
        if seats.is_empty() || global == 0 {
            return Err("positive token cap and model seats required".into());
        }
        let allocations = if explicit.is_empty() {
            seats
                .iter()
                .map(|s| (*s, global / seats.len() as u64))
                .collect()
        } else {
            explicit
        };
        let budget = Self::Tokens {
            global,
            allocations,
        };
        budget.validate(seats)?;
        Ok(budget)
    }
    pub fn validate(&self, seats: &[SeatId]) -> Result<(), String> {
        let invalid_roster = |keys: Vec<SeatId>| {
            keys.len() != seats.len() || seats.iter().any(|s| !keys.contains(s))
        };
        match self {
            Self::ReportedUsd {
                global,
                allocations,
            } => {
                let total: f64 = allocations.values().sum();
                if invalid_roster(allocations.keys().copied().collect())
                    || !global.is_finite()
                    || *global <= 0.
                    || allocations.values().any(|v| !v.is_finite() || *v <= 0.)
                    || !total.is_finite()
                    || total > *global
                {
                    return Err("invalid seat allocations or total above global USD cap".into());
                }
            }
            Self::Tokens {
                global,
                allocations,
            } => {
                let total = allocations
                    .values()
                    .try_fold(0u64, |n, v| n.checked_add(*v))
                    .ok_or("token allocation overflow")?;
                if invalid_roster(allocations.keys().copied().collect())
                    || *global == 0
                    || allocations.values().any(|v| *v == 0)
                    || total > *global
                {
                    return Err("invalid seat allocations or total above global token cap".into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistent_equal_and_explicit_shares_reject_extra_authority() {
        let seats = SeatId::all().collect::<Vec<_>>();
        for cap in [0.3, 1., 10.1] {
            SpendBudget::usd(cap, &seats, BTreeMap::new()).unwrap();
        }
        let shares = seats.iter().map(|s| (*s, 1.)).collect();
        assert!(SpendBudget::usd(9., &seats, shares).is_err());
        assert!(SpendBudget::usd(f64::NAN, &seats, BTreeMap::new()).is_err());
        assert!(SpendBudget::tokens(9, &seats, BTreeMap::new()).is_err());
        assert!(SpendBudget::tokens(10, &seats, BTreeMap::new()).is_ok());
    }
}
