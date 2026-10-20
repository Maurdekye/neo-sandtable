//! Exact one-time initial load commitment at the real declaration finish.
//! Setup alone authenticates rights; this module cannot create an initial ID.

use std::collections::BTreeSet;

use cna_core::{
    decision::{ActionSchema, FieldSchema},
    engine::{EngineError, Rejection},
};
use cna_protocol::Side;
use serde::{Deserialize, Serialize};

use super::{PlaneDeclaration, TacticalPeriod};
use crate::{
    CnaContent, State,
    air::{
        maintenance,
        preparation::{self, CarriedOrdnance, CurrentPreparation, InitialChoice, KnownLoad},
        state::{AirRuntime, PlaneId},
    },
    steps::illegal,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitialDecision {
    Load,
    Forfeit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitialOrdnanceKind {
    None,
    Bombs,
    Mines,
    Torpedoes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialOrdnance {
    pub kind: InitialOrdnanceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialChoiceSelection {
    pub decision: InitialDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guns: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordnance: Option<InitialOrdnance>,
}

pub(super) fn schema() -> ActionSchema {
    ActionSchema::Record { fields: vec![
        super::field("decision", "Use the whole initial load, or forfeit the whole right.", super::choice(["load".into(), "forfeit".into()]), false),
        super::field("guns", "Required explicit gun ammunition for load; absent/null for forfeit.", ActionSchema::Bool, true),
        super::field("ordnance", "Required explicit carried choice for load; absent/null for forfeit.", ActionSchema::Record { fields: vec![
            super::field("kind", "Exactly one explicit carried choice; none is not omitted knowledge.", super::choice(["none".into(), "bombs".into(), "mines".into(), "torpedoes".into()]), false),
            FieldSchema { name: "points".into(), doc: "Positive selected-source rating points; absent/null only for none. Mines use the exact bombload rating, not a mine count.".into(), optional: true, schema: ActionSchema::Integer { min: 1, max: i64::from(i32::MAX) } },
        ] }, true),
    ] }
}

fn invariant(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("initial declaration load: {detail}"),
    }
}

/// Own-known validation only. No selected mode, ammunition or credit is inferred.
/// Cases: scen:59.32, scen:59.36, airlog:34.14, airlog:38.41, airlog:41.75
/// Interpretations: interp:air-0018
fn selection(
    content: &CnaContent,
    state: &State,
    side: Side,
    selected: &PlaneDeclaration,
    finishing: bool,
) -> Result<Option<(InitialChoice, CurrentPreparation)>, Rejection> {
    let runtime = &state.air.runtime;
    let plane = runtime
        .aircraft
        .get(&selected.plane)
        .filter(|plane| super::owned(&plane.force, side))
        .ok_or_else(|| illegal("unknown own initial-choice aircraft"))?;
    let entitled = matches!(
        runtime.preparation.initial_choices.get(&selected.plane),
        Some(InitialChoice::Unselected)
    ) && matches!(
        runtime.preparation.current.get(&selected.plane),
        Some(CurrentPreparation::InitialUnspecified)
    );
    let Some(choice) = &selected.initial_choice else {
        if entitled {
            if finishing {
                return Err(Rejection::Engine(EngineError::Unsupported {
                    case: "scen:59.32".into(),
                    detail: "Buffered initial aircraft declaration needs an explicit complete load or forfeiture; no load is inferred".into(),
                }));
            }
            return Err(illegal(
                "choose a complete initial load or explicitly forfeit its whole right",
            ));
        }
        return Ok(None);
    };
    if !entitled {
        return Err(illegal(
            "this own aircraft has no unselected authenticated initial load",
        ));
    }
    if choice.decision == InitialDecision::Forfeit {
        if choice.guns.is_some() || choice.ordnance.is_some() {
            return Err(illegal(
                "forfeiture must not include gun or ordnance selections",
            ));
        }
        return Ok(Some((
            InitialChoice::Forfeited,
            CurrentPreparation::Unprepared,
        )));
    }
    let guns = choice
        .guns
        .ok_or_else(|| illegal("initial load requires explicit gun ammunition"))?;
    let carried = choice.ordnance.as_ref().ok_or_else(|| {
        illegal("initial load requires an explicit carried choice, including none")
    })?;
    let ordnance = match (carried.kind, carried.points) {
        (InitialOrdnanceKind::None, None) => None,
        (InitialOrdnanceKind::Bombs, Some(points)) if points > 0 => Some(CarriedOrdnance::Bombs {
            bomb_points: points,
        }),
        (InitialOrdnanceKind::Mines, Some(points)) if points > 0 => Some(CarriedOrdnance::Mines {
            source_bombload_rating: points,
        }),
        (InitialOrdnanceKind::Torpedoes, Some(points)) if points > 0 => {
            Some(CarriedOrdnance::Torpedoes {
                rating_points: points,
            })
        }
        _ => {
            return Err(illegal(
                "initial ordnance needs an explicit none or positive source rating",
            ));
        }
    };
    let aircraft = content.units.aircraft.get(&plane.aircraft).ok_or_else(|| {
        Rejection::Engine(EngineError::Unsupported {
            case: "airlog:34.6".into(),
            detail: "Initial aircraft has no source characteristics".into(),
        })
    })?;
    let source = aircraft
        .modes
        .get(selected.mode)
        .ok_or_else(|| illegal("initial mode is outside the own aircraft source"))?;
    maintenance::fuel_points(aircraft, selected.mode).map_err(Rejection::Engine)?;
    let (requested, limit, exact) = match &ordnance {
        None => (None, None, false),
        Some(CarriedOrdnance::Bombs { bomb_points }) => {
            (Some(*bomb_points), source.bomb_capacity, false)
        }
        Some(CarriedOrdnance::Mines {
            source_bombload_rating,
        }) => (Some(*source_bombload_rating), source.bomb_capacity, true),
        Some(CarriedOrdnance::Torpedoes { rating_points }) => {
            (Some(*rating_points), source.torpedo_capacity, false)
        }
    };
    if let Some(requested) = requested {
        let limit = limit.ok_or_else(|| {
            Rejection::Engine(EngineError::Unsupported {
                case: "airlog:34.14".into(),
                detail: format!(
                    "Initial carried rating missing for {} mode {}",
                    plane.aircraft, selected.mode
                ),
            })
        })?;
        if limit < 0 {
            return Err(Rejection::Engine(invariant(
                "negative trusted initial ordnance rating",
            )));
        }
        if requested > limit || (exact && requested != limit) {
            return Err(illegal(
                "initial payload disagrees with the selected source mode",
            ));
        }
    }
    let load = KnownLoad {
        mode: selected.mode,
        fuel_available: true,
        gun_ammunition: guns,
        ordnance,
    };
    preparation::validate_load(content, plane, &load).map_err(Rejection::Engine)?;
    Ok(Some((
        InitialChoice::Committed,
        CurrentPreparation::KnownLoad { load },
    )))
}

pub(super) fn validate_answer(
    content: &CnaContent,
    state: &State,
    side: Side,
    selected: &PlaneDeclaration,
) -> Result<(), Rejection> {
    selection(content, state, side, selected, false).map(|_| ())
}

/// Not serialized or answer-provided. Its only constructor below is parent-only
/// and called by the actual declaration finish after both plans validate.
pub(in crate::air) struct InitialLoadAdmission {
    before: AirRuntime,
    after: AirRuntime,
    changed: BTreeSet<PlaneId>,
}

impl InitialLoadAdmission {
    pub(in crate::air) fn apply(&self, runtime: &mut AirRuntime) -> Result<(), EngineError> {
        if *runtime != self.before {
            return Err(invariant("stale initial load admission"));
        }
        *runtime = self.after.clone();
        Ok(())
    }
    pub(in crate::air) fn permits_history(&self, old: &AirRuntime, new: &AirRuntime) -> bool {
        old == &self.before && new == &self.after
    }
    pub(in crate::air) fn permits_plane(
        &self,
        id: &PlaneId,
        old: &AirRuntime,
        new: &AirRuntime,
    ) -> bool {
        self.changed.contains(id) && self.permits_history(old, new)
    }
}

/// Exact whole initial choices, never a payment or flight entitlement.
/// Cases: scen:59.32, scen:59.36, airlog:39.12
/// Interpretations: interp:air-0018
pub(super) fn prepare(
    content: &CnaContent,
    state: &State,
    period: TacticalPeriod,
) -> Result<Option<InitialLoadAdmission>, EngineError> {
    let before = &state.air.runtime;
    if super::period(state)? != period
        || before.tactical.period != Some(period)
        || before.tactical.resolved
        || Side::ALL
            .iter()
            .any(|side| !before.tactical.answers.contains_key(side))
    {
        return Err(invariant(
            "initial admission requires both current declaration answers",
        ));
    }
    let mut after = before.clone();
    let mut changed = BTreeSet::new();
    for side in Side::ALL {
        for mission in &before.tactical.answers[&side] {
            for plane in &mission.planes {
                let selected =
                    selection(content, state, side, plane, true).map_err(|error| match error {
                        Rejection::Engine(error) => error,
                        Rejection::Illegal { message } => EngineError::Invariant {
                            detail: format!("accepted initial choice became invalid: {message}"),
                        },
                        _ => invariant("unexpected initial selection refusal"),
                    })?;
                if let Some((ledger, current)) = selected {
                    if !changed.insert(plane.plane.clone()) {
                        return Err(invariant("duplicate initial choice aircraft"));
                    }
                    let record = after
                        .aircraft
                        .get_mut(&plane.plane)
                        .ok_or_else(|| invariant("initial aircraft missing"))?;
                    record.fuelled = if current == CurrentPreparation::Unprepared {
                        false
                    } else {
                        preparation::fuel_availability(&current)
                            .ok_or_else(|| invariant("initial final fuel is unknown"))?
                    };
                    record.armed = preparation::armed_projection(&current, record.armed);
                    after
                        .preparation
                        .current
                        .insert(plane.plane.clone(), current);
                    after
                        .preparation
                        .initial_choices
                        .insert(plane.plane.clone(), ledger);
                }
            }
        }
    }
    Ok((!changed.is_empty()).then_some(InitialLoadAdmission {
        before: before.clone(),
        after,
        changed,
    }))
}

#[cfg(test)]
mod tests;
