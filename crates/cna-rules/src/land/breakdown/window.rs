//! Completed moves suspend further orders until private breakdown choices finish.
use super::{
    RolledCheck,
    losses::{self, LossPlan},
};
use crate::{
    CnaContent, State,
    state::Pending,
    steps::{illegal, open},
};
use cna_core::{
    decision::{ActionSchema, ActionSpace, FieldSchema, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    ids::SeatId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;

pub const KIND: &str = "cna.movement.breakdown.losses";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Resume {
    Orders {
        seat: SeatId,
        orders: Vec<super::super::movement::Order>,
    },
    Reaction {
        seat: SeatId,
    },
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Window {
    pub held: Vec<Pending>,
    pub outcomes: VecDeque<RolledCheck>,
    pub submitted: Option<LossPlan>,
    pub resume: Option<Resume>,
    pub parked: bool,
}
/// Hide other windows until the completed move's checks have been adjudicated.
/// Cases: land:21.24, land:21.28, land:21.29
pub(crate) fn park(s: &mut State, resume: Option<Resume>) -> bool {
    if s.land.breakdown.stopped.is_empty() {
        return false;
    }
    let w = &mut s.land.breakdown.window;
    w.held.extend(std::mem::take(&mut s.decisions.pending));
    if resume.is_some() {
        w.resume = resume;
    }
    w.parked = true;
    true
}
fn field(name: &str, schema: ActionSchema) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        doc: name.replace('_', " "),
        schema,
        optional: false,
    }
}
fn record(names: &[&str], schema: ActionSchema) -> ActionSchema {
    ActionSchema::Record {
        fields: names.iter().map(|n| field(n, schema.clone())).collect(),
    }
}
fn space(outcome: &RolledCheck) -> ActionSpace {
    let count = ActionSchema::Integer {
        min: 0,
        max: i64::from(i32::MAX),
    };
    let cargo = record(&["fuel", "ammo", "stores", "water"], count.clone());
    let packing = record(&["light", "medium", "heavy"], cargo);
    let transport = record(&["light", "medium", "heavy"], count.clone());
    let mut units: Vec<_> = outcome
        .group
        .assets
        .iter()
        .map(|a| a.unit.clone())
        .collect();
    units.sort();
    units.dedup();
    let mut fields = vec![field(
        "unit",
        ActionSchema::Unit {
            among: units.clone(),
        },
    )];
    fields.extend(
        ["working", "origin", "destination"]
            .iter()
            .map(|n| field(n, packing.clone())),
    );
    fields.extend(
        ["origin_transport", "destination_transport"]
            .iter()
            .map(|n| field(n, transport.clone())),
    );
    fields.extend(
        [
            "origin_passengers",
            "destination_passengers",
            "origin_tank_fuel_tenths",
            "destination_tank_fuel_tenths",
            "origin_activity_water",
            "destination_activity_water",
        ]
        .iter()
        .map(|n| field(n, count.clone())),
    );
    let n = outcome.group.assets.len() as u32;
    ActionSpace::new(ActionSchema::Record {
        fields: vec![
            field(
                "losses",
                ActionSchema::List {
                    item: Box::new(count.clone()),
                    min: n,
                    max: n,
                },
            ),
            field(
                "at_origin",
                ActionSchema::List {
                    item: Box::new(count),
                    min: n,
                    max: n,
                },
            ),
            field(
                "partitions",
                ActionSchema::List {
                    item: Box::new(ActionSchema::Record { fields }),
                    min: units.len() as u32,
                    max: units.len() as u32,
                },
            ),
        ],
    })
    .with_context(json!({"breakdown":outcome}))
}
fn open_losses(c: &CnaContent, s: &mut State, cx: &mut Cx<'_>) {
    let outcome = s.land.breakdown.window.outcomes.front().unwrap().clone();
    let id = &outcome.group.assets[0].unit;
    let side = s.land.units[id].side;
    let role = crate::ownership::seat_for_unit(c, s, id);
    open(
        s,
        cx,
        SeatId::new(side, role),
        KIND,
        "Allocate own broken vehicles, their positions, passengers and conserved cargo.".into(),
        &["land:21.35", "land:21.36", "land:21.41", "land:21.43"],
        Trigger::Triggered,
        Secrecy::Secret,
        space(&outcome),
    );
}
/// Validate only the disclosed rolled result and this owner's holdings; store the selection.
/// Cases: land:21.35, land:21.36, land:21.41, land:21.43
pub fn answer(
    c: &CnaContent,
    s: &mut State,
    p: &Pending,
    action: &Value,
) -> Result<String, Rejection> {
    let outcome = s
        .land
        .breakdown
        .window
        .outcomes
        .front()
        .ok_or_else(|| illegal("breakdown choice is no longer open"))?;
    let id = &outcome.group.assets[0].unit;
    if p.seat
        != SeatId::new(
            s.land.units[id].side,
            crate::ownership::seat_for_unit(c, s, id),
        )
    {
        return Err(illegal("these broken vehicles belong to another seat"));
    }
    let plan: LossPlan = serde_json::from_value(action.clone())
        .map_err(|_| illegal("provide proportional losses and conserved vehicle holdings"))?;
    losses::apply(c, &mut s.clone(), outcome, &plan)?;
    s.land.breakdown.window.submitted = Some(plan);
    Ok("Own breakdown allocation recorded.".into())
}
fn engine_error(e: Rejection) -> EngineError {
    match e {
        Rejection::Engine(e) => e,
        e => EngineError::Invariant {
            detail: format!("accepted breakdown continuation failed: {e:?}"),
        },
    }
}
/// Dice and physical loss application happen after answer acceptance, before the mover resumes.
/// Cases: land:21.24, land:21.31, land:21.35, land:21.41, land:21.43
pub(crate) fn finish(
    c: &CnaContent,
    s: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    loop {
        if !s.land.breakdown.window.parked && s.land.breakdown.stopped.is_empty() {
            return Ok(());
        }
        park(s, None);
        if let Some(plan) = s.land.breakdown.window.submitted.take() {
            let outcome = s
                .land
                .breakdown
                .window
                .outcomes
                .pop_front()
                .ok_or_else(|| EngineError::Invariant {
                    detail: "breakdown answer lacks a rolled check".into(),
                })?;
            cx.events
                .extend(losses::apply(c, s, &outcome, &plan).map_err(engine_error)?);
        }
        loop {
            if !s.land.breakdown.window.outcomes.is_empty() {
                open_losses(c, s, cx);
                return Ok(());
            }
            if s.land.breakdown.stopped.is_empty() {
                break;
            }
            let stopped = s.land.breakdown.stopped.remove(0);
            let outcomes = super::roll_checks(c, s, &stopped, strict, cx)?;
            s.land.breakdown.window.outcomes.extend(outcomes);
        }
        let mut window = std::mem::take(&mut s.land.breakdown.window);
        s.decisions.pending.append(&mut window.held);
        match window.resume {
            Some(Resume::Orders { seat, orders }) => {
                super::super::movement::resume_orders(c, s, seat, &orders, strict, cx)
                    .map_err(engine_error)
            }
            Some(Resume::Reaction { seat }) => {
                super::super::reaction::resume_after_breakdown(c, s, seat, strict, cx)
                    .map_err(engine_error)
            }
            None => Ok(()),
        }?;
        // A resumed list can complete another move without opening a decision. Drain it here
        // before the engine advances this step; no recursive call grows with the order count.
        if !s.decisions.pending.is_empty()
            || (!s.land.breakdown.window.parked && s.land.breakdown.stopped.is_empty())
        {
            return Ok(());
        }
    }
}
