//! Candidate-only instruction, with credit at the complete old OpStage boundary.
use super::*;
use crate::{CnaContent, State, ownership, state::LandUnit, steps::illegal};
use cna_core::{engine::Rejection, ids::SeatId};

pub(super) fn stage(state: &State) -> Result<OpStageKey, EngineError> {
    Ok(OpStageKey {
        game_turn: state.cursor.game_turn,
        op_stage: state
            .cursor
            .op_stage
            .filter(|op| (1..=3).contains(op))
            .ok_or_else(|| invariant("training outside an OpStage"))?,
    })
}

pub(super) fn owned<'a>(
    content: &CnaContent,
    state: &'a State,
    seat: SeatId,
    id: &str,
) -> Result<&'a ReplacementCounter, Rejection> {
    let counter = super::scheduling::side(state, seat.side)
        .map_err(Rejection::Engine)?
        .counters
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| illegal("not an owned replacement counter"))?;
    super::arrival::recipient(content, state, seat.side, &counter.recipient, &counter.kind)?;
    if ownership::seat_for_unit(content, state, &counter.recipient) != seat.role {
        return Err(illegal(
            "replacement command belongs to its recipient's owner",
        ));
    }
    if counter.points <= 0 {
        return Err(Rejection::Engine(invariant("nonpositive counter")));
    }
    Ok(counter)
}

/// Geographic identity comes from surveyed map/area records, never a printed-number guess.
/// Cases: land:17.32, land:20.43
fn training_hex(
    content: &CnaContent,
    owner: Side,
    location: &Location,
) -> Result<HexId, EngineError> {
    let Location::Hex { hex } = location else {
        return Err(gap(
            "land:17.32",
            "replacement instruction needs a verified training hex",
        ));
    };
    let record = content
        .map
        .get(hex)
        .ok_or_else(|| invariant("training hex absent"))?;
    let allowed = if owner == Side::Axis {
        record.terrain.as_deref() == Some("major_city")
            || record.flags.iter().any(|flag| flag == "major_city")
            || content
                .areas
                .areas
                .get("tripoli")
                .is_some_and(|a| a.membership_status == "resolved" && a.hex_ids.contains(hex))
    } else {
        [
            "cairo",
            "alexandria",
            "helwan",
            "amiriya",
            "abouqir",
            "deghelia",
        ]
        .iter()
        .any(|id| {
            content
                .areas
                .areas
                .get(*id)
                .is_some_and(|a| a.membership_status == "resolved" && a.hex_ids.contains(hex))
        })
    };
    if !allowed {
        return Err(gap(
            "land:17.32",
            "no verified permitted training-area membership",
        ));
    }
    Ok(hex.clone())
}

/// Static morale cannot certify that an explicitly untrained instructor reached its maximum.
/// Cases: land:17.35, land:17.6, land:20.43
fn qualifies(
    content: &CnaContent,
    state: &State,
    owner: Side,
    counter: &ReplacementCounter,
    instructor: &LandUnit,
) -> Result<bool, EngineError> {
    if instructor.side != owner
        || instructor.location != counter.location
        || state.land.arrivals.withdrawn_units.contains(&instructor.id)
    {
        return Ok(false);
    }
    let source = content
        .units
        .units
        .get(&instructor.id)
        .ok_or_else(|| invariant("instructor source absent"))?;
    let class = crate::land::formation::class(content, &instructor.id)
        .ok_or_else(|| gap("land:17.35", "instructor has no verified class"))?;
    let subject = super::arrival::training_subject(content, &counter.kind, &counter.recipient)?;
    if subject.requires_assigned_unit() {
        if instructor.id != counter.recipient {
            return Ok(false);
        }
    } else if source.echelon.as_deref().or(class.echelon.as_deref()) != Some("battalion") {
        return Ok(false);
    }
    if !crate::land::formation::combat_unit(content, &instructor.id) {
        return Ok(false);
    }
    let needs_tank = match &counter.kind {
        ReplacementKind::Tank { .. } => true,
        ReplacementKind::Recce { weapon, .. } => content
            .units
            .weapons
            .get(weapon)
            .is_some_and(|weapon| weapon.kind == "tank"),
        _ => false,
    };
    if needs_tank && class.unit_type != "tank"
        || matches!(counter.kind, ReplacementKind::Infantry { .. }) && class.unit_type != "infantry"
    {
        return Ok(false);
    }
    let sheet = content
        .units
        .sheets
        .get(&source.sheet)
        .ok_or_else(|| invariant("instructor sheet absent"))?;
    let base = source
        .basic_morale
        .or(sheet.basic_morale)
        .ok_or_else(|| gap("land:17.35", "instructor basic morale is unknown"))?;
    if sheet
        .basic_morale_untrained
        .is_some_and(|untrained| untrained != base)
    {
        return Err(gap(
            "land:17.35",
            "unit-morale training progress is not a replacement duration or static entitlement",
        ));
    }
    let current = crate::logistics::toe_strength(content, instructor)
        .map_err(|_| gap("land:17.35", "instructor current strength is unknown"))?
        .get();
    let mut assigned = instructor.clone();
    assigned.toe = source.toe.clone();
    let assigned = crate::logistics::toe_strength(content, &assigned)
        .map_err(|_| gap("land:17.35", "instructor assigned strength is unknown"))?
        .get();
    Ok(assigned > 0 && i64::from(current) * 3 >= i64::from(assigned))
}

fn instructor(
    content: &CnaContent,
    state: &State,
    owner: Side,
    counter: &ReplacementCounter,
) -> Result<Option<UnitId>, EngineError> {
    training_hex(content, owner, &counter.location)?;
    let mut unknown = None;
    for unit in state.land.units.values() {
        match qualifies(content, state, owner, counter, unit) {
            Ok(true) => return Ok(Some(unit.id.clone())),
            Ok(false) => (),
            Err(error) => unknown = Some(error),
        }
    }
    if let Some(error) = unknown {
        return Err(error);
    }
    Ok(None)
}

/// Prepare uses only the commanding seat's own recipients/counters/instructors.
/// Cases: land:17.33, land:17.35, land:20.43
pub(super) fn prepare(
    content: &CnaContent,
    state: &State,
    seat: SeatId,
    plan: &TrainingPlan,
) -> Result<Vec<(String, TrainingCandidate)>, Rejection> {
    let current = stage(state).map_err(Rejection::Engine)?;
    let mut seen = BTreeSet::new();
    let mut prepared = vec![];
    for id in &plan.counters {
        if !seen.insert(id) {
            return Err(illegal("training counter selected twice"));
        }
        let counter = owned(content, state, seat, id)?;
        let Training::Training {
            required_stages,
            completed,
            candidate,
            ..
        } = &counter.training
        else {
            return Err(illegal("counter is already trained"));
        };
        let expected = content.tables.land.training.op_stages(
            super::arrival::training_subject(content, &counter.kind, &counter.recipient)
                .map_err(Rejection::Engine)?,
        );
        if i32::from(*required_stages) != expected || completed >= required_stages {
            return Err(Rejection::Engine(invariant(
                "invalid live training duration/progress",
            )));
        }
        if counter.physical.stage > current {
            return Err(Rejection::Engine(invariant(
                "future counter physical stage",
            )));
        }
        if counter.physical.stage == current && counter.physical.cp_quarters != 0
            || candidate
                .as_ref()
                .is_some_and(|c| c.stage == current && c.interrupted)
        {
            return Err(illegal(
                "any current-stage CP or interrupted candidate prevents training",
            ));
        }
        let next = TrainingCandidate {
            stage: current,
            instructor: instructor(content, state, seat.side, counter)
                .map_err(Rejection::Engine)?
                .ok_or_else(|| illegal("no qualified cohex instruction battalion"))?,
            hex: training_hex(content, seat.side, &counter.location).map_err(Rejection::Engine)?,
            interrupted: false,
        };
        // Re-answering cannot repair an interruption or switch this stage's witness.
        if candidate.as_ref().is_some_and(|c| {
            c.stage == current && (c.instructor != next.instructor || c.hex != next.hex)
        }) {
            return Err(illegal("training candidate changed within its OpStage"));
        }
        prepared.push((id.clone(), next));
    }
    Ok(prepared)
}

pub(super) fn register(
    state: &mut State,
    owner: Side,
    prepared: &[(String, TrainingCandidate)],
) -> Result<(), EngineError> {
    for (id, witness) in prepared {
        let counter = super::scheduling::side_mut(state, owner)?
            .counters
            .iter_mut()
            .find(|c| c.id == *id)
            .ok_or_else(|| invariant("prepared training counter vanished"))?;
        if counter.physical.stage < witness.stage {
            counter.physical.stage = witness.stage;
            counter.physical.cp_quarters = 0;
        }
        let Training::Training { candidate, .. } = &mut counter.training else {
            return Err(invariant(
                "prepared training became Ready before stage exit",
            ));
        };
        *candidate = Some(witness.clone());
    }
    Ok(())
}

/// Arrival can register a verified candidate; absence of an instructor never grants credit.
/// Cases: land:20.43
pub(super) fn register_arrivals(
    content: &CnaContent,
    state: &mut State,
    owner: Side,
    ids: &[String],
) -> Result<(), EngineError> {
    for id in ids {
        let Some(counter) = state
            .land
            .replacements
            .sides
            .iter()
            .find(|s| s.side == owner)
            .and_then(|s| s.counters.iter().find(|c| c.id == *id))
        else {
            continue;
        };
        let seat = SeatId::new(
            owner,
            ownership::seat_for_unit(content, state, &counter.recipient),
        );
        match instructor(content, state, owner, counter) {
            Ok(None) => continue,
            Ok(Some(_)) => (),
            Err(EngineError::Unsupported { case, .. })
                if case == "land:17.32" || case == "land:17.35" =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
        // The arrival procedure does not invent missing instruction facts; selection in the
        // subsequent training window returns their exact cited refusal.
        match prepare(
            content,
            state,
            seat,
            &TrainingPlan {
                counters: vec![id.clone()],
            },
        ) {
            Ok(prepared) => register(state, owner, &prepared)?,
            Err(Rejection::Engine(EngineError::Unsupported { case, .. }))
                if case == "land:17.32" || case == "land:17.35" =>
            {
                ()
            }
            Err(Rejection::Engine(error)) => return Err(error),
            Err(_) => {
                return Err(invariant(
                    "fresh arrival failed its training-candidate prerequisites",
                ));
            }
        }
    }
    Ok(())
}

fn valid(
    content: &CnaContent,
    state: &State,
    owner: Side,
    counter: &ReplacementCounter,
    witness: &TrainingCandidate,
) -> bool {
    if counter.physical.stage != witness.stage
        || counter.physical.cp_quarters != 0
        || counter.location
            != (Location::Hex {
                hex: witness.hex.clone(),
            })
    {
        return false;
    }
    if super::arrival::recipient(content, state, owner, &counter.recipient, &counter.kind).is_err()
    {
        return false;
    }
    // The real movement kernel records executed prefixes, including a trip that
    // ends back in the training hex. Ignore stale prior-window history in Organization.
    if state.cursor.anchor() == "opstage.movement_and_combat.movement"
        && state.land.movement.moved.contains(&witness.instructor)
    {
        return false;
    }
    state
        .land
        .units
        .get(&witness.instructor)
        .is_some_and(|u| qualifies(content, state, owner, counter, u).unwrap_or(false))
}

/// Monotone bookkeeping in the canonical draft. No events, RNG, or new candidate creation.
/// Cases: land:17.33, land:17.35, land:20.43
pub(crate) fn invalidate_training(content: &CnaContent, state: &mut State) {
    let interrupted: Vec<_> = state
        .land
        .replacements
        .sides
        .iter()
        .flat_map(|own| {
            own.counters
                .iter()
                .filter_map(|counter| match &counter.training {
                    Training::Training {
                        candidate: Some(candidate),
                        ..
                    } if !candidate.interrupted
                        && !valid(content, state, own.side, counter, candidate) =>
                    {
                        Some((own.side, counter.id.clone()))
                    }
                    _ => None,
                })
        })
        .collect();
    for (owner, id) in interrupted {
        if let Some(counter) = state
            .land
            .replacements
            .sides
            .iter_mut()
            .find(|s| s.side == owner)
            .and_then(|s| s.counters.iter_mut().find(|c| c.id == id))
            && let Training::Training {
                candidate: Some(candidate),
                ..
            } = &mut counter.training
        {
            candidate.interrupted = true;
        }
    }
}

/// Exactly one call at the complete OLD stage exit, before cursor/CP reset.
/// No candidates means byte-identical state; unabsorbed points receive no idle cohesion recovery.
/// Cases: land:17.33, land:20.43, land:20.42
pub(crate) fn finish_training_stage(
    content: &CnaContent,
    state: &mut State,
) -> Result<(), EngineError> {
    if !state.land.replacements.sides.iter().any(|s| {
        s.counters.iter().any(|c| {
            matches!(
                c.training,
                Training::Training {
                    candidate: Some(_),
                    ..
                }
            )
        })
    }) {
        return Ok(());
    }
    let current = stage(state)?;
    if state.cursor.block != crate::seq::Block::PlayerHalf
        || state.cursor.half != Some(crate::seq::Half::B)
        || state.cursor.index + 1 != crate::seq::PLAYER_HALF.len()
        || !state.decisions.pending.is_empty()
    {
        return Err(invariant(
            "training credit requires complete old OpStage exit",
        ));
    }
    invalidate_training(content, state);
    for own in &mut state.land.replacements.sides {
        for counter in &mut own.counters {
            let Training::Training {
                required_stages,
                completed,
                last_counted,
                candidate,
            } = &mut counter.training
            else {
                continue;
            };
            let Some(witness) = candidate.as_ref() else {
                continue;
            };
            if witness.stage != current {
                continue;
            }
            if !witness.interrupted && *last_counted != Some(current) {
                *completed = completed
                    .checked_add(1)
                    .ok_or_else(|| invariant("training progress overflow"))?;
                *last_counted = Some(current);
            }
            *candidate = None;
            counter.physical.cp_quarters = 0;
            if *completed == *required_stages {
                counter.training = Training::Ready;
            }
        }
    }
    Ok(())
}
