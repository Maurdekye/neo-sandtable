//! Breakdown exposure and proportional allocation; all rolls belong to adjudication.
use crate::{CnaContent, State};
use cna_core::{
    engine::EngineError,
    ids::{HexId, UnitId},
};
use cna_tables::land::weather::WeatherKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BreakdownState {
    pub accumulated_quarters: BTreeMap<UnitId, i32>,
    pub moving: BTreeMap<UnitId, Motion>,
    /// A completed check does not discharge exposure; the unadjusted band is remembered.
    pub checked: BTreeMap<UnitId, BTreeMap<String, usize>>,
    pub stopped: Vec<StoppedMove>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Motion {
    pub origin: HexId,
    pub travel_cp_quarters: i32,
    pub sandstorm_cp_quarters: i32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoppedMove {
    pub members: Vec<UnitId>,
    pub exposures: BTreeMap<UnitId, Motion>,
    pub destination: HexId,
}
fn overflow() -> EngineError {
    EngineError::Invariant {
        detail: "breakdown exposure overflow".into(),
    }
}
/// Only travelled edges count: detachment, combat and breaking-off expenditure are excluded.
/// One motion continues through reaction interrupts and its revised remaining path.
/// Cases: land:21.21, land:21.22, land:21.23, land:21.25, land:21.37
/// Interpretations: interp:land-0027
pub fn record_edge(
    s: &mut State,
    id: &UnitId,
    from: &HexId,
    bp_quarters: i32,
    cp_quarters: i32,
    weather: WeatherKind,
) -> Result<(), EngineError> {
    if bp_quarters < 0 || cp_quarters <= 0 {
        return Err(overflow());
    }
    let bp = s
        .land
        .breakdown
        .accumulated_quarters
        .get(id)
        .copied()
        .unwrap_or(0)
        .checked_add(bp_quarters)
        .ok_or_else(overflow)?;
    let mut motion = s.land.breakdown.moving.get(id).cloned().unwrap_or(Motion {
        origin: from.clone(),
        travel_cp_quarters: 0,
        sandstorm_cp_quarters: 0,
    });
    motion.travel_cp_quarters = motion
        .travel_cp_quarters
        .checked_add(cp_quarters)
        .ok_or_else(overflow)?;
    if weather == WeatherKind::Sandstorm {
        motion.sandstorm_cp_quarters = motion
            .sandstorm_cp_quarters
            .checked_add(cp_quarters)
            .ok_or_else(overflow)?;
    }
    s.land.breakdown.accumulated_quarters.insert(id.clone(), bp);
    s.land.breakdown.moving.insert(id.clone(), motion);
    Ok(())
}
/// Queue one check when a complete ordinary, reaction or retreat move stops.
/// Cases: land:21.24, land:21.25, land:21.28, land:21.29
pub fn stop(s: &mut State, members: &[UnitId], destination: &HexId) {
    let exposures: BTreeMap<_, _> = members
        .iter()
        .filter_map(|id| s.land.breakdown.moving.remove(id).map(|m| (id.clone(), m)))
        .collect();
    if !exposures.is_empty() {
        s.land.breakdown.stopped.push(StoppedMove {
            members: exposures.keys().cloned().collect(),
            exposures,
            destination: destination.clone(),
        });
    }
}
/// Hot weather and the majority of actual movement CP each contribute a right shift.
/// Cases: land:21.32, land:21.37, land:29.33, land:29.45
/// Interpretations: interp:land-0027
pub fn weather_shift(motion: &Motion, hot: bool) -> i32 {
    i32::from(hot)
        + i32::from(
            motion.travel_cp_quarters > 0
                && i64::from(motion.sandstorm_cp_quarters) * 2
                    >= i64::from(motion.travel_cp_quarters),
        )
}
/// A new roll requires exposure above three BP and a strictly higher unadjusted band.
/// Negative BAR shifts may remove a roll without erasing the accumulated points.
/// Cases: land:21.26, land:21.27, land:21.31, land:21.33
pub fn needs_check(c: &CnaContent, s: &State, id: &UnitId, category: &str) -> bool {
    let n = s
        .land
        .breakdown
        .accumulated_quarters
        .get(id)
        .copied()
        .unwrap_or(0);
    n > 12
        && c.tables
            .land
            .breakdown
            .column_quarters(n)
            .is_some_and(|band| {
                s.land
                    .breakdown
                    .checked
                    .get(id)
                    .and_then(|k| k.get(category))
                    .is_none_or(|old| band > *old)
            })
}
/// An allocation uses each member's proportional quota, with all integer remainders conserved.
/// Players may choose ties; the baseline uses largest remainder then stable input order.
/// Cases: land:21.35, land:21.36
pub fn proportional_allocation(points: &[i32], broken: i32) -> Option<Vec<i32>> {
    if points.iter().any(|n| *n < 0) || broken < 0 {
        return None;
    }
    let total: i64 = points.iter().map(|n| i64::from(*n)).sum();
    if i64::from(broken) > total {
        return None;
    }
    if total == 0 {
        return Some(vec![0; points.len()]);
    }
    let mut answer: Vec<i32> = points
        .iter()
        .map(|n| (i64::from(*n) * i64::from(broken) / total) as i32)
        .collect();
    let mut order: Vec<_> = (0..points.len()).collect();
    order.sort_by_key(|i| {
        (
            std::cmp::Reverse(i64::from(points[*i]) * i64::from(broken) % total),
            *i,
        )
    });
    let remaining = broken - answer.iter().sum::<i32>();
    for i in order.into_iter().take(remaining as usize) {
        answer[i] += 1;
    }
    Some(answer)
}
/// Accept any player-selected floor/ceiling allocation that conserves the rolled total.
/// Cases: land:21.36
pub fn valid_allocation(points: &[i32], broken: i32, allocation: &[i32]) -> bool {
    if points.len() != allocation.len() || proportional_allocation(points, broken).is_none() {
        return false;
    }
    let total: i64 = points.iter().map(|n| i64::from(*n)).sum();
    allocation.iter().map(|n| i64::from(*n)).sum::<i64>() == i64::from(broken)
        && points.iter().zip(allocation).all(|(n, l)| {
            if total == 0 {
                return *l == 0;
            }
            let product = i64::from(*n) * i64::from(broken);
            *l >= 0
                && *l <= *n
                && i64::from(*l) >= product / total
                && i64::from(*l) <= (product + total - 1) / total
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Cases: land:21.21, land:21.22, land:21.25, land:21.26, land:21.27
    #[test]
    fn stopping_retains_bp_and_only_a_new_band_requires_another_roll() {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let id = UnitId::new("it.libyan_tank_command.xxi_l_tank_bn");
        record_edge(&mut s, &id, &"C4020".into(), 12, 8, WeatherKind::Normal).unwrap();
        assert!(!needs_check(&c, &s, &id, "tank:2"));
        record_edge(&mut s, &id, &"C4021".into(), 48, 8, WeatherKind::Normal).unwrap();
        stop(&mut s, std::slice::from_ref(&id), &"C4022".into());
        assert_eq!(s.land.breakdown.accumulated_quarters[&id], 60);
        assert_eq!(
            s.land.breakdown.stopped[0].exposures[&id].origin,
            HexId::new("C4020")
        );
        s.land
            .breakdown
            .checked
            .entry(id.clone())
            .or_default()
            .insert("tank:2".into(), 2);
        assert!(!needs_check(&c, &s, &id, "tank:2"));
        record_edge(&mut s, &id, &"C4022".into(), 8, 8, WeatherKind::Normal).unwrap();
        assert!(!needs_check(&c, &s, &id, "tank:2"));
        record_edge(&mut s, &id, &"C4023".into(), 40, 8, WeatherKind::Normal).unwrap();
        assert!(needs_check(&c, &s, &id, "tank:2"));
    }
    /// Cases: land:21.37, land:29.45
    /// Interpretations: interp:land-0027
    #[test]
    fn sandstorm_share_uses_travel_cp_including_the_exact_half_boundary() {
        let mut m = Motion {
            origin: "C4020".into(),
            travel_cp_quarters: 16,
            sandstorm_cp_quarters: 8,
        };
        assert_eq!(weather_shift(&m, false), 1);
        assert_eq!(weather_shift(&m, true), 2);
        m.sandstorm_cp_quarters = 7;
        assert_eq!(weather_shift(&m, false), 0);
        m.travel_cp_quarters = 0;
        assert_eq!(weather_shift(&m, false), 0);
    }
    /// Cases: land:21.35, land:21.36
    #[test]
    fn proportional_losses_conserve_unequal_strengths_and_allow_player_ties() {
        assert_eq!(proportional_allocation(&[10, 20], 3), Some(vec![1, 2]));
        assert_eq!(proportional_allocation(&[1, 1, 1], 2), Some(vec![1, 1, 0]));
        assert!(valid_allocation(&[1, 1, 1], 2, &[0, 1, 1]));
        assert!(!valid_allocation(&[10, 20], 3, &[3, 0]));
        for a in 0..=20 {
            for b in 0..=20 {
                for lost in 0..=a + b {
                    let out = proportional_allocation(&[a, b], lost).unwrap();
                    assert!(valid_allocation(&[a, b], lost, &out));
                }
            }
        }
    }
}
