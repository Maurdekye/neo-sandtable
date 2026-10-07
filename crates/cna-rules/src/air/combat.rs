//! Individual matched air-combat resolution, callable only by finish adjudication.
//!
//! This module resolves one already-assigned wave. The caller owns disclosure,
//! mission/escort eligibility, mode/load and pilot entitlement, formation membership,
//! and recovery. Private IDs and results are not observer reports or events.
use std::collections::BTreeSet;

use cna_content::units::AircraftMode;
use cna_core::{
    dice::{CampaignRng, TwoDiceReading},
    engine::EngineError,
};

use super::state::PlaneId;
use crate::CnaContent;

/// Source-adjudicated combat role; a bomber/dive-bomber distinction must come
/// from the aircraft's actual classification, never guessed from bomb capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombatRole {
    Fighter,
    Bomber,
    DiveBomber,
    Other,
}

/// Caller-owned mission/load facts, not an answer-time permission or receipt.
/// Formation size counts bombers in this actual mission, not all planes in a hex.
#[derive(Debug, Clone, Copy)]
pub struct CombatContext {
    pub role: CombatRole,
    pub pilot_rating: u8,
    pub formation_bombers: usize,
    pub gun_ammunition: bool,
    pub night: bool,
}

/// Ephemeral source ratings, kept out of serialized Air truth. Gun ammunition
/// is distinct from bombs; setup's coarse armed bit is not proof of this fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combatant {
    pub id: PlaneId,
    role: CombatRole,
    tacair: i32,
    maneuver: i32,
    pilot_rating: u8,
    gun_ammunition: bool,
}

fn invalid(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("air combat: {detail}"),
    }
}
fn source_gap(case: &str, detail: &str) -> EngineError {
    EngineError::Unsupported {
        case: case.into(),
        detail: detail.into(),
    }
}

impl Combatant {
    /// Derive ratings after the caller establishes actual aircraft mode, trained
    /// pilot, guns and mission formation. Missing ratings are not zero defaults.
    /// Cases: airlog:34.12, airlog:34.13, airlog:40.15, airlog:45.0, airlog:45.36
    pub fn from_mode(
        id: PlaneId,
        mode: &AircraftMode,
        context: CombatContext,
    ) -> Result<Self, EngineError> {
        if id.0.is_empty() || ![0, 1, 2, 3, 4, 6].contains(&context.pilot_rating) {
            return Err(invalid("invalid plane or pilot rating"));
        }
        if context.role == CombatRole::Fighter && mode.tacair_paren {
            return Err(invalid(
                "parenthesized TacAir cannot be a fighter combatant",
            ));
        }
        if context.role != CombatRole::Fighter && context.pilot_rating != 0 {
            return Err(invalid("pilot combat bonus belongs to fighters"));
        }
        if context.role != CombatRole::Bomber && context.formation_bombers != 0 {
            return Err(invalid("only ordinary bombers receive formation bonuses"));
        }
        let tacair = mode
            .tacair
            .ok_or_else(|| source_gap("airlog:34.12", "Missing aircraft TacAir rating"))?;
        let maneuver = if context.night {
            mode.maneuver_night.or(mode.maneuver)
        } else {
            mode.maneuver
        }
        .ok_or_else(|| source_gap("airlog:34.13", "Missing aircraft maneuver rating"))?;
        if tacair < 0 || maneuver < 0 {
            return Err(invalid("negative source rating"));
        }
        let bonus = if context.role == CombatRole::Fighter {
            i32::from(context.pilot_rating)
        } else if context.formation_bombers >= 18 {
            2
        } else if context.formation_bombers >= 6 {
            1
        } else {
            0
        };
        let tacair = tacair
            .checked_add(bonus)
            .ok_or_else(|| invalid("TacAir overflow"))?;
        Ok(Self {
            id,
            role: context.role,
            tacair,
            maneuver,
            pilot_rating: context.pilot_rating,
            gun_ammunition: context.gun_ammunition,
        })
    }

    /// Proposed local reading: opposing fire uses normal ratings without gun ammo.
    /// Gameplay callers await the ruling on the maintenance/combat conflict.
    /// Cases: airlog:45.0, airlog:45.17, airlog:45.4
    /// Interpretations: interp:air-0016
    pub fn differential(&self, content: &CnaContent, opponent: &Self) -> Result<i32, EngineError> {
        let gap = self
            .maneuver
            .checked_sub(opponent.maneuver)
            .ok_or_else(|| invalid("maneuver overflow"))?;
        let adjustment = content.tables.airlog.maneuver_adjustment.adjustment(gap);
        let signed = if gap < 0 {
            -adjustment
        } else if gap > 0 {
            adjustment
        } else {
            0
        };
        self.tacair
            .checked_sub(opponent.tacair)
            .and_then(|n| n.checked_add(signed))
            .ok_or_else(|| invalid("differential overflow"))
    }
}

/// Matchups are irrevocable for this wave. Attacker order is the chosen
/// resolution order. A fighter or dive bomber selects one return-fire target
/// when several fighters attack it; ordinary non-fighters fire at each attacker.
#[derive(Debug, Clone)]
pub struct Engagement {
    pub defender: Combatant,
    pub attackers: Vec<Combatant>,
    pub return_fire_target: Option<PlaneId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    pub shooter: PlaneId,
    pub target: PlaneId,
    pub differential: i32,
    pub reading: TwoDiceReading,
    pub shot_down: bool,
}

/// Provisional shot-down results, before recovery and persistent inventory losses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WaveResult {
    pub shots: Vec<Shot>,
    pub shot_down: BTreeSet<PlaneId>,
}

fn validate(content: &CnaContent, engagements: &[Engagement]) -> Result<(), EngineError> {
    let mut used = BTreeSet::new();
    for group in engagements {
        if group.attackers.is_empty() {
            return Err(invalid("matchup has no attacker"));
        }
        for plane in std::iter::once(&group.defender).chain(&group.attackers) {
            if !used.insert(plane.id.clone()) {
                return Err(invalid("plane assigned to multiple matchups"));
            }
        }
        let one_target = matches!(
            group.defender.role,
            CombatRole::Fighter | CombatRole::DiveBomber
        );
        if one_target && group.attackers.len() > 1 && group.return_fire_target.is_none() {
            return Err(invalid("return-fire target required"));
        }
        if (!one_target && group.return_fire_target.is_some())
            || group
                .return_fire_target
                .as_ref()
                .is_some_and(|id| !group.attackers.iter().any(|p| &p.id == id))
        {
            return Err(invalid("return-fire target outside matchup"));
        }
        for plane in &group.attackers {
            if plane.role != CombatRole::Fighter {
                return Err(invalid("attacker is not a fighter"));
            }
            plane.differential(content, &group.defender)?;
            group.defender.differential(content, plane)?;
        }
    }
    Ok(())
}

fn fire(
    content: &CnaContent,
    rng: &mut CampaignRng,
    result: &mut WaveResult,
    shooter: &Combatant,
    target: &Combatant,
) -> Result<(), EngineError> {
    if !shooter.gun_ammunition
        || result.shot_down.contains(&shooter.id)
        || result.shot_down.contains(&target.id)
    {
        return Ok(());
    }
    let differential = shooter.differential(content, target)?;
    let reading = rng.two_dice_reading();
    let shot_down = content
        .tables
        .airlog
        .tacair_kill
        .kills(differential, reading);
    if shot_down {
        result.shot_down.insert(target.id.clone());
    }
    result.shots.push(Shot {
        shooter: shooter.id.clone(),
        target: target.id.clone(),
        differential,
        reading,
        shot_down,
    });
    Ok(())
}

/// Resolve a closed, trusted matchup plan. Validate the entire wave before dice;
/// publish RNG once on success. Never call this from Respond or preflight.
/// No fighter changes target when its assigned opponent is shot down. For a
/// multi-attacker group, the lone defender fires first; a non-fighter always
/// fires first, at all attackers except a dive bomber's one selected target.
/// Cases: airlog:45.0, airlog:45.17, airlog:45.19, airlog:45.24, airlog:45.34, airlog:45.5
pub fn resolve_wave(
    content: &CnaContent,
    rng: &mut CampaignRng,
    engagements: &[Engagement],
) -> Result<WaveResult, EngineError> {
    validate(content, engagements)?;
    let mut draft_rng = rng.clone();
    let mut result = WaveResult::default();
    for group in engagements {
        if group.defender.role == CombatRole::Fighter && group.attackers.len() == 1 {
            let attacker = &group.attackers[0];
            let diff = attacker.differential(content, &group.defender)?;
            let attacker_first =
                diff < 0 || (diff == 0 && attacker.pilot_rating >= group.defender.pilot_rating);
            let (first, second) = if attacker_first {
                (attacker, &group.defender)
            } else {
                (&group.defender, attacker)
            };
            fire(content, &mut draft_rng, &mut result, first, second)?;
            fire(content, &mut draft_rng, &mut result, second, first)?;
        } else {
            for attacker in &group.attackers {
                let selected = group
                    .return_fire_target
                    .as_ref()
                    .is_none_or(|id| id == &attacker.id);
                if selected {
                    fire(
                        content,
                        &mut draft_rng,
                        &mut result,
                        &group.defender,
                        attacker,
                    )?;
                }
            }
            for attacker in &group.attackers {
                fire(
                    content,
                    &mut draft_rng,
                    &mut result,
                    attacker,
                    &group.defender,
                )?;
            }
        }
    }
    *rng = draft_rng;
    Ok(result)
}

#[cfg(test)]
mod tests;
