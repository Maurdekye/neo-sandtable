//! Private Flak target-group calculation on a trusted finish draft.
//!
//! The caller establishes the exact mission/day/night group, eligible AA units
//! and facilities, source strength, ammunition and density classification.
//! This module grants none of those rights, spends no supplies and publishes no
//! events or persistent losses. Fighter recovery follows this provisional loss.
use std::collections::BTreeSet;

use cna_core::{
    dice::{CampaignRng, Die, TwoDiceReading},
    engine::EngineError,
};
use cna_tables::airlog::crt::{AaEffect, AaGroup};

use super::state::PlaneId;
use crate::CnaContent;

/// Already-adjudicated target and attacking strength. Persistent IDs are private.
/// `density_applies` means the other-mission group contains the source-required
/// bomber/transport class; it is not inferred from a squadron or gun/armed bit.
#[derive(Debug, Clone)]
pub struct Attack {
    pub group: AaGroup,
    pub planes: Vec<PlaneId>,
    pub flak_points: i32,
    pub density_applies: bool,
}

/// Provisional aircraft outcomes. These are not public labels, inventory deaths
/// or pilot casualties: the caller owns disclosure, recovery and final mutation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub destroyed: BTreeSet<PlaneId>,
    pub aborted: BTreeSet<PlaneId>,
    pub destroyed_roll: Option<TwoDiceReading>,
    pub aborted_roll: Option<TwoDiceReading>,
    pub column_shift: i32,
}

fn invalid(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("air flak: {detail}"),
    }
}

// Use base-six campaign dice and rejection so every remaining index has the
// same number of representations. The plan bounds n to i32::MAX; its next
// base-six power fits u64. No random choice is needed when only one remains.
fn index_with(n: usize, mut die: impl FnMut() -> Die) -> usize {
    assert!(n > 0 && n <= i32::MAX as usize);
    if n == 1 {
        return 0;
    }
    let n = n as u64;
    let mut space = 1_u64;
    let mut digits = 0;
    while space < n {
        space *= 6;
        digits += 1;
    }
    let limit = space - space % n;
    loop {
        let mut value = 0_u64;
        for _ in 0..digits {
            value = value * 6 + u64::from(die().value() - 1);
        }
        if value < limit {
            return (value % n) as usize;
        }
    }
}

fn select(rng: &mut CampaignRng, remaining: &mut Vec<PlaneId>, count: u8) -> BTreeSet<PlaneId> {
    let count = usize::from(count).min(remaining.len());
    (0..count)
        .map(|_| {
            let index = index_with(remaining.len(), || rng.d6());
            remaining.remove(index)
        })
        .collect()
}

/// Resolve a trusted group; reject malformed plans before dice and publish the
/// cloned RNG once. Sort private IDs for reproducible uniform sampling without
/// replacement. Other-mission losses are selected before its abort roll and
/// abort identities; destroyed planes never absorb aborts. Both chart rolls use
/// the original column even when losses leave fewer surviving targets.
/// Cases: airlog:46.0, airlog:46.25, airlog:46.26, airlog:46.3, airlog:46.4
/// Interpretations: interp:air-0004, interp:airlog-0005
pub fn resolve(
    content: &CnaContent,
    rng: &mut CampaignRng,
    attack: &Attack,
) -> Result<Outcome, EngineError> {
    if attack.flak_points < 0 {
        return Err(invalid("negative attacking strength"));
    }
    if attack.group == AaGroup::PlanesOnFighterMissions && attack.density_applies {
        return Err(invalid("fighter target cannot receive density shift"));
    }
    let count =
        i32::try_from(attack.planes.len()).map_err(|_| invalid("target group too large"))?;
    let mut ids = BTreeSet::new();
    for id in &attack.planes {
        if id.0.is_empty() || !ids.insert(id.clone()) {
            return Err(invalid("invalid or repeated aircraft"));
        }
    }
    if count == 0 || attack.flak_points == 0 {
        return Ok(Outcome::default());
    }
    // Adopted airlog-0005 pins density to this target group, not all aircraft in
    // the hex. Eligibility of its bomber/transport classification is caller-owned.
    let shift = if attack.density_applies {
        content.tables.airlog.flak_adjustment.column_shift(count)
    } else {
        0
    };
    let mut draft_rng = rng.clone();
    let destroyed_roll = draft_rng.two_dice_reading();
    let destroyed_count = content
        .tables
        .airlog
        .aa_combat
        .planes(
            attack.group,
            AaEffect::PlanesDestroyed,
            attack.flak_points,
            shift,
            destroyed_roll,
        )
        .ok_or_else(|| invalid("validated chart has no destroyed result"))?;
    let mut remaining: Vec<_> = ids.into_iter().collect();
    let destroyed = select(&mut draft_rng, &mut remaining, destroyed_count);
    let (aborted_roll, aborted) = if attack.group == AaGroup::PlanesOnOtherMissions {
        let roll = draft_rng.two_dice_reading();
        let count = content
            .tables
            .airlog
            .aa_combat
            .planes(
                attack.group,
                AaEffect::PlanesAborted,
                attack.flak_points,
                shift,
                roll,
            )
            .ok_or_else(|| invalid("validated chart has no aborted result"))?;
        (Some(roll), select(&mut draft_rng, &mut remaining, count))
    } else {
        (None, BTreeSet::new())
    };
    *rng = draft_rng;
    Ok(Outcome {
        destroyed,
        aborted,
        destroyed_roll: Some(destroyed_roll),
        aborted_roll,
        column_shift: shift,
    })
}

#[cfg(test)]
mod tests;
