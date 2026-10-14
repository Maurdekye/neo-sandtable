//! Recipient-owner declarations; Respond buffers, finish validates an atomic draft.
use super::scheduling::{key, side, side_mut};
use super::*;
use crate::{
    CnaContent, State, ownership,
    state::Pending,
    steps::{illegal, open},
};
use cna_core::{
    decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger},
    engine::{Cx, Rejection},
    ids::SeatId,
};
use cna_protocol::Role;
use serde_json::Value;

pub(crate) const TRAINING: &str = "cna.replacements.training";
pub(crate) const ALLOCATION: &str = "cna.replacements.allocation";
fn is_training(state: &State) -> bool {
    state.cursor.anchor() == "opstage.organization.training"
}
fn procedure(own: &SideReplacements, training: bool) -> &ReplacementProcedure {
    if training {
        &own.training
    } else {
        &own.reorganization
    }
}
fn procedure_mut(own: &mut SideReplacements, training: bool) -> &mut ReplacementProcedure {
    if training {
        &mut own.training
    } else {
        &mut own.reorganization
    }
}
fn ids(content: &CnaContent, state: &State, seat: SeatId, training: bool) -> Vec<String> {
    state
        .land
        .replacements
        .sides
        .iter()
        .filter(|own| own.side == seat.side)
        .flat_map(|own| &own.counters)
        .filter(|counter| {
            ownership::seat_for_unit(content, state, &counter.recipient) == seat.role
                && matches!(counter.training, Training::Training { .. }) == training
        })
        .map(|counter| counter.id.clone())
        .collect()
}
fn field(name: &str, schema: ActionSchema) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        doc: "Recipient-owned declaration; finish derives source eligibility and costs.".into(),
        schema,
        optional: false,
    }
}
fn choices(ids: &[String]) -> ActionSchema {
    ActionSchema::Choice {
        options: ids
            .iter()
            .map(|id| ChoiceOption {
                id: id.clone(),
                label: id.clone(),
                detail: None,
            })
            .collect(),
    }
}

/// Domain membership depends only on counters assigned to this seat's own recipients.
/// No counter, candidate or owned request means no state/decision-id mutation.
/// Cases: land:17.33, land:20.3, land:20.41, land:20.43, land:20.49, land:3.6
pub(crate) fn enter_units(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let training = is_training(state);
    let current = key(state);
    let mut openings = vec![];
    for owner in Side::ALL {
        let Some(own) = state
            .land
            .replacements
            .sides
            .iter()
            .find(|own| own.side == owner)
        else {
            continue;
        };
        if procedure(own, training).key.as_ref() == Some(&current) {
            continue;
        }
        for role in [Role::FrontLine, Role::RearArea, Role::Logistics] {
            let seat = SeatId::new(owner, role);
            let counters = ids(content, state, seat, training);
            if !counters.is_empty() {
                openings.push((seat, counters));
            }
        }
    }
    for (seat, counters) in openings {
        let own = side_mut(state, seat.side)?;
        let proc = procedure_mut(own, training);
        if proc.key.as_ref() != Some(&current) {
            proc.key = Some(current.clone());
            proc.phase = ProcedurePhase::Awaiting {
                submitted: Some(if training {
                    ReplacementPlan::Training(TrainingPlan::default())
                } else {
                    ReplacementPlan::Allocation(AllocationPlan::default())
                }),
            };
        }
        let space = if training {
            ActionSpace::new(ActionSchema::Record {
                fields: vec![field(
                    "counters",
                    ActionSchema::List {
                        min: 0,
                        max: counters
                            .len()
                            .try_into()
                            .map_err(|_| invariant("training domain overflow"))?,
                        item: Box::new(choices(&counters)),
                    },
                )],
            })
            .with_pass("Defer instruction; no training credit is granted.")
        } else {
            let recipients: Vec<_> = side(state, seat.side)?
                .counters
                .iter()
                .filter(|c| counters.contains(&c.id))
                .map(|c| c.recipient.to_string())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            ActionSpace::new(ActionSchema::Record {
                fields: vec![field(
                    "uses",
                    ActionSchema::List {
                        min: 0,
                        max: counters
                            .len()
                            .try_into()
                            .map_err(|_| invariant("allocation domain overflow"))?,
                        item: Box::new(ActionSchema::Record {
                            fields: vec![
                                field("counter", choices(&counters)),
                                field("recipient", choices(&recipients)),
                                field(
                                    "points",
                                    ActionSchema::Integer {
                                        min: 1,
                                        max: i32::MAX.into(),
                                    },
                                ),
                                field(
                                    "conversion",
                                    ActionSchema::Text {
                                        min_length: 1,
                                        max_length: 80,
                                    },
                                ),
                            ],
                        }),
                    },
                )],
            })
            .with_pass("Absorb no points; retain all counters and physical history.")
        };
        open(
            state,
            cx,
            seat,
            if training { TRAINING } else { ALLOCATION },
            "Replacement unit command",
            "Only your recipient-owned counters may be selected; answers do not train or absorb.",
            &[
                "land:17.33",
                "land:20.3",
                "land:20.41",
                "land:20.43",
                "land:20.49",
            ],
            Trigger::Scheduled,
            Secrecy::SecretSimultaneous,
            space,
        );
    }
    Ok(())
}

/// Accepted answers record only the existing side-local procedure payload.
/// Cases: land:3.6, land:17.33, land:20.41, land:20.43, land:20.49
pub(crate) fn answer_unit(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
) -> Result<String, Rejection> {
    let training = is_training(state);
    if pending.kind != if training { TRAINING } else { ALLOCATION }
        || procedure(
            side(state, pending.seat.side).map_err(Rejection::Engine)?,
            training,
        )
        .key
        .as_ref()
            != Some(&key(state))
    {
        return Err(illegal("not this recipient-owned replacement window"));
    }
    let plan = if training {
        let plan: TrainingPlan = if action.is_null() {
            TrainingPlan::default()
        } else {
            serde_json::from_value(action.clone())
                .map_err(|_| illegal("invalid training declaration"))?
        };
        super::training::prepare(content, state, pending.seat, &plan)?;
        ReplacementPlan::Training(plan)
    } else {
        let plan: AllocationPlan = if action.is_null() {
            AllocationPlan::default()
        } else {
            serde_json::from_value(action.clone())
                .map_err(|_| illegal("invalid absorption declaration"))?
        };
        super::allocation::prepare(content, state, pending.seat, &plan)?;
        ReplacementPlan::Allocation(plan)
    };
    let proc = procedure_mut(
        side_mut(state, pending.seat.side).map_err(Rejection::Engine)?,
        training,
    );
    match (&mut proc.phase, plan) {
        (
            ProcedurePhase::Awaiting {
                submitted: Some(ReplacementPlan::Training(accepted)),
            },
            ReplacementPlan::Training(plan),
        ) => {
            if plan
                .counters
                .iter()
                .any(|id| accepted.counters.contains(id))
            {
                return Err(illegal("counter already declared"));
            }
            accepted.counters.extend(plan.counters);
            accepted.counters.sort();
        }
        (
            ProcedurePhase::Awaiting {
                submitted: Some(ReplacementPlan::Allocation(accepted)),
            },
            ReplacementPlan::Allocation(plan),
        ) => {
            if plan
                .uses
                .iter()
                .any(|usage| accepted.uses.iter().any(|old| old.counter == usage.counter))
            {
                return Err(illegal("counter already declared"));
            }
            accepted.uses.extend(plan.uses);
            accepted.uses.sort_by(|a, b| a.counter.cmp(&b.counter));
        }
        _ => return Err(illegal("replacement command procedure is closed")),
    }
    Ok("Recorded recipient-owned replacement choices; no adjudication yet.".into())
}

/// Reprepare all current owner facts and publish BOTH sides only after full success.
/// Cases: land:17.33, land:20.3, land:20.41, land:20.43, land:20.49
pub(crate) fn finish_units(content: &CnaContent, state: &mut State) -> Result<(), EngineError> {
    if state
        .decisions
        .pending
        .iter()
        .any(|p| matches!(p.kind.as_str(), TRAINING | ALLOCATION))
    {
        return Ok(());
    }
    let training = is_training(state);
    let current = key(state);
    if !state.land.replacements.sides.iter().any(|own| {
        procedure(own, training).key.as_ref() == Some(&current)
            && matches!(
                procedure(own, training).phase,
                ProcedurePhase::Awaiting { .. }
            )
    }) {
        return Ok(());
    }
    let mut draft = state.clone();
    for owner in Side::ALL {
        let Some(own) = draft
            .land
            .replacements
            .sides
            .iter()
            .find(|own| own.side == owner)
        else {
            continue;
        };
        let proc = procedure(own, training);
        if proc.key.as_ref() != Some(&current) || matches!(proc.phase, ProcedurePhase::Resolved) {
            continue;
        }
        let ProcedurePhase::Awaiting {
            submitted: Some(plan),
        } = &proc.phase
        else {
            return Err(invariant("missing accepted replacement command"));
        };
        let plan = plan.clone();
        let accepted_ids: Vec<&str> = match &plan {
            ReplacementPlan::Training(plan) => plan.counters.iter().map(String::as_str).collect(),
            ReplacementPlan::Allocation(plan) => {
                plan.uses.iter().map(|u| u.counter.as_str()).collect()
            }
            _ => return Err(invariant("wrong replacement command payload")),
        };
        if accepted_ids
            .iter()
            .any(|id| own_counter_role(content, &draft, owner, id).is_none())
        {
            return Err(invariant(
                "accepted replacement counter vanished before finish",
            ));
        }
        for role in [Role::FrontLine, Role::RearArea, Role::Logistics] {
            let seat = SeatId::new(owner, role);
            match &plan {
                ReplacementPlan::Training(plan) => {
                    let plan = TrainingPlan {
                        counters: plan
                            .counters
                            .iter()
                            .filter(|id| own_counter_role(content, &draft, owner, id) == Some(role))
                            .cloned()
                            .collect(),
                    };
                    let prepared = super::training::prepare(content, &draft, seat, &plan)
                        .map_err(finish_error)?;
                    super::training::register(&mut draft, owner, &prepared)?;
                }
                ReplacementPlan::Allocation(plan) => {
                    let plan = AllocationPlan {
                        uses: plan
                            .uses
                            .iter()
                            .filter(|u| {
                                own_counter_role(content, &draft, owner, &u.counter) == Some(role)
                            })
                            .cloned()
                            .collect(),
                    };
                    draft = super::allocation::prepare(content, &draft, seat, &plan)
                        .map_err(finish_error)?;
                }
                _ => return Err(invariant("wrong replacement command payload")),
            }
        }
        procedure_mut(side_mut(&mut draft, owner)?, training).phase = ProcedurePhase::Resolved;
    }
    *state = draft;
    Ok(())
}

fn own_counter_role(content: &CnaContent, state: &State, owner: Side, id: &str) -> Option<Role> {
    side(state, owner)
        .ok()?
        .counters
        .iter()
        .find(|c| c.id == id)
        .map(|c| ownership::seat_for_unit(content, state, &c.recipient))
}
fn finish_error(error: Rejection) -> EngineError {
    match error {
        Rejection::Engine(error) => error,
        _ => invariant("accepted replacement command no longer satisfies its own prerequisites"),
    }
}
