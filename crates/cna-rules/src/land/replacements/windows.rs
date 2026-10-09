//! Fixed private production windows; actor answers only record declarations.
use super::*;
use super::scheduling::*;
use crate::{CnaContent, State, state::Pending, steps::{illegal, open}};
use cna_core::{decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger}, engine::{Cx, Rejection}, ids::SeatId};
use cna_protocol::Role;
use serde_json::Value;

pub(crate) const SCHEDULE: &str = "cna.replacements.schedule";
pub(crate) const DISTRIBUTION: &str = "cna.replacements.distribution";

fn field(name: &str, doc: &str, schema: ActionSchema, optional: bool) -> FieldSchema {
    FieldSchema { name: name.into(), doc: doc.into(), schema, optional }
}
fn choices(values: &[(&str, &str)]) -> ActionSchema {
    ActionSchema::Choice { options: values.iter().map(|(id, label)| ChoiceOption {
        id: (*id).into(), label: (*label).into(), detail: None,
    }).collect() }
}

fn schedule_space(owner: Side, end: u16) -> ActionSpace {
    let families = if owner == Side::Axis {
        choices(&[("axis_truck", "Printed Axis truck pool"), ("axis_equipment", "Printed Axis equipment pool")])
    } else { choices(&[("commonwealth_equipment", "Printed Commonwealth finite equipment pool")]) };
    ActionSpace::new(ActionSchema::Record { fields: vec![field("requests", "Optional finite production; Commonwealth random rows are determined automatically for the public scheduled turns.", ActionSchema::List {
        min: 0, max: u32::MAX,
        item: Box::new(ActionSchema::Record { fields: vec![
            field("family", "Source production family", families, false),
            field("item", "Exact native equipment designation; required for equipment, absent for trucks", ActionSchema::Text { min_length: 1, max_length: 100 }, true),
            field("nation", "German or Italian equipment; required only for Axis equipment", choices(&[("german", "German"), ("italian", "Italian")]), true),
            field("truck", "Required only for Axis truck production", choices(&[("light", "Light"), ("medium", "Medium"), ("heavy", "Heavy")]), true),
            field("points", "Positive determined production within printed period/pool limits", ActionSchema::Integer { min: 1, max: i32::MAX.into() }, false),
            field("arrival_turn", "Source-permitted arrival Game-Turn", ActionSchema::Integer { min: 1, max: end.into() }, false),
        ] }),
    }, false)] }).with_pass(if owner == Side::Axis { "Schedule no optional Axis production." } else { "Choose no finite equipment; still determine the scheduled random production." })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SchedulingAnswer { requests: Vec<FiniteRequest> }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FiniteRequest {
    family: String,
    item: Option<String>,
    nation: Option<ReplacementNation>,
    truck: Option<FuelTruckKind>,
    points: i32,
    arrival_turn: u16,
}

fn decode_schedule(content: &CnaContent, state: &State, owner: Side, action: &Value) -> Result<SchedulePlan, Rejection> {
    let answer = if action.is_null() { SchedulingAnswer { requests: vec![] } }
        else { serde_json::from_value(action.clone()).map_err(|_| illegal("invalid finite production plan"))? };
    let mut requests = vec![];
    for request in answer.requests {
        let row = match (request.family.as_str(), request.item, request.nation, request.truck) {
            ("axis_truck", None, None, Some(kind)) => ProductionRowRef::AxisTruck { kind },
            ("axis_equipment", Some(item), Some(nation @ (ReplacementNation::German | ReplacementNation::Italian)), None) => ProductionRowRef::AxisEquipment { nation, item },
            ("commonwealth_equipment", Some(item), None, None) => ProductionRowRef::CommonwealthEquipment { item },
            _ => return Err(illegal("production row fields do not match its family")),
        };
        requests.push(ProductionRequest { row, quantity: ProductionQuantity::Fixed { points: request.points }, arrival_turn: request.arrival_turn });
    }
    if owner == Side::Commonwealth {
        for arrival_turn in arrival_turns(content, state, owner).map_err(Rejection::Engine)? {
            for kind in [FuelTruckKind::Light, FuelTruckKind::Medium, FuelTruckKind::Heavy] {
                requests.push(ProductionRequest { row: ProductionRowRef::CommonwealthTruck { column: truck_column(arrival_turn).map_err(Rejection::Engine)?.0.into(), kind }, quantity: ProductionQuantity::Roll, arrival_turn });
            }
            requests.push(ProductionRequest { row: ProductionRowRef::CommonwealthInfantry { column: infantry_column(arrival_turn).map_err(Rejection::Engine)?.0.into() }, quantity: ProductionQuantity::Roll, arrival_turn });
        }
    }
    Ok(SchedulePlan { requests })
}

/// Both producing seats' public calendar opportunities are opened together.
/// No finite balance or hidden production result removes a scheduled window.
/// Cases: land:20.61, land:20.63, land:20.72, land:20.73, land:3.6
pub(crate) fn enter_schedule(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    let current = key(state);
    for owner in Side::ALL {
        if state.land.replacements.sides.iter().filter(|s| s.side == owner).count() > 1 {
            return Err(invariant("duplicate replacement side"));
        }
        if !state.land.replacements.sides.iter().any(|s| s.side == owner) {
            state.land.replacements.sides.push(SideReplacements::new(owner));
        }
        // Reaching this GT's pre-stage proves every earlier GT completed.
        side_mut(state, owner)?.cw_arrival_limits.retain(|limit| limit.game_turn >= current.segment.game_turn);
        if side(state, owner)?.planning.key.as_ref() == Some(&current) { continue; }
        let arrivals = arrival_turns(content, state, owner)?;
        side_mut(state, owner)?.planning = ReplacementProcedure { key: Some(current.clone()), phase: if arrivals.is_empty() {
            ProcedurePhase::Resolved
        } else { ProcedurePhase::Awaiting { submitted: None } } };
        if !arrivals.is_empty() {
            let space = schedule_space(owner, content.bounds.end_gt);
            open(state, cx, SeatId::new(owner, Role::Logistics), SCHEDULE,
                format!("Plan replacement production for the public arrival turns {arrivals:?}. Shipping and later physical procedures retain their cited holds."),
                &["land:20.61", "land:20.63", "land:20.72", "land:20.73"], Trigger::Scheduled, Secrecy::SecretSimultaneous, space);
        }
    }
    Ok(())
}

fn distribution_space(determined: &[DeterminedProduction]) -> ActionSpace {
    let fields = determined.iter().enumerate().map(|(i, source)| {
        field(&i.to_string(), &format!("GT{} {:?}: distribute exactly {} determined points; balance the combined total across all types.", source.arrival_turn, source.row, source.points),
            ActionSchema::Record { fields: (1..=3).map(|stage| field(&stage.to_string(), "Points arriving in this OpStage", ActionSchema::Integer { min: 0, max: source.points.into() }, false)).collect() }, false)
    }).collect();
    ActionSpace::new(ActionSchema::Record { fields }).with_pass("Use the deterministic even all-type distribution, including a zero result.")
}

fn decode_distribution(determined: &[DeterminedProduction], action: &Value) -> Result<DistributionPlan, Rejection> {
    let mut batches = vec![];
    if action.is_null() {
        let mut next_stage = std::collections::BTreeMap::<u16, u8>::new();
        for source in determined {
            let stage = next_stage.entry(source.arrival_turn).or_insert(0);
            let base = source.points / 3;
            let remainder = source.points % 3;
            let mut counts = [base; 3];
            for _ in 0..remainder { counts[usize::from(*stage)] += 1; *stage = (*stage + 1) % 3; }
            for (i, points) in counts.into_iter().enumerate().filter(|(_, points)| *points > 0) {
                batches.push(DistributionBatch { row: source.row.clone(), kind: source.kind.clone(), points, due: OpStageKey { game_turn: source.arrival_turn, op_stage: (i + 1) as u8 } });
            }
        }
    } else {
        let object = action.as_object().ok_or_else(|| illegal("invalid distribution record"))?;
        if object.len() != determined.len() { return Err(illegal("answer every determined production row")); }
        for (i, source) in determined.iter().enumerate() {
            let stages = object.get(&i.to_string()).and_then(Value::as_object).ok_or_else(|| illegal("missing distribution row"))?;
            if stages.len() != 3 { return Err(illegal("answer all three OpStages")); }
            for stage in 1..=3 {
                let points: i32 = stages.get(&stage.to_string()).and_then(Value::as_i64).and_then(|n| n.try_into().ok()).filter(|n| *n >= 0).ok_or_else(|| illegal("invalid distribution count"))?;
                if points > 0 { batches.push(DistributionBatch { row: source.row.clone(), kind: source.kind.clone(), points, due: OpStageKey { game_turn: source.arrival_turn, op_stage: stage } }); }
            }
        }
    }
    Ok(DistributionPlan { batches })
}

/// Answers write only the private procedure buffer, never dice/budgets/orders/counters.
/// Cases: land:20.61, land:20.73, land:20.74, land:3.6
pub(crate) fn answer(content: &CnaContent, state: &mut State, pending: &Pending, action: &Value) -> Result<String, Rejection> {
    if pending.seat.role != Role::Logistics || key(state).anchor != ANCHOR || side(state, pending.seat.side).map_err(Rejection::Engine)?.planning.key.as_ref() != Some(&key(state)) {
        return Err(illegal("not the current producing seat's window"));
    }
    let owner = pending.seat.side;
    match pending.kind.as_str() {
        SCHEDULE => {
            let plan = decode_schedule(content, state, owner, action)?;
            prepare_schedule(content, state, pending.seat, &plan)?;
            side_mut(state, owner).map_err(Rejection::Engine)?.planning.phase = ProcedurePhase::Awaiting { submitted: Some(ReplacementPlan::Schedule(plan)) };
        }
        DISTRIBUTION => {
            let ProcedurePhase::Distributing { determined, submitted: None } = &side(state, owner).map_err(Rejection::Engine)?.planning.phase else { return Err(illegal("distribution is closed")); };
            let plan = decode_distribution(determined, action)?;
            validate_distribution(owner, determined, &plan)?;
            let ProcedurePhase::Distributing { submitted, .. } = &mut side_mut(state, owner).map_err(Rejection::Engine)?.planning.phase else { return Err(Rejection::Engine(invariant("distribution phase changed"))); };
            *submitted = Some(plan);
        }
        _ => return Err(illegal("unknown replacement window")),
    }
    Ok("Recorded the private replacement plan for finish.".into())
}

pub(crate) fn planning_complete(state: &State) -> bool {
    Side::ALL.iter().all(|owner| side(state, *owner).is_ok_and(|s| s.planning.key.as_ref() == Some(&key(state)) && matches!(s.planning.phase, ProcedurePhase::Resolved)))
}

/// Resolve both complete scheduled answers on one disposable State/RNG/events draft.
/// The one fixed distribution round precedes supply planning; there is no interstage reroll.
/// Cases: land:20.61, land:20.64, land:20.73, land:20.74, land:3.6
pub(crate) fn finish_planning(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    if state.decisions.pending.iter().any(|p| matches!(p.kind.as_str(), SCHEDULE | DISTRIBUTION)) { return Ok(()); }
    let mut draft = state.clone();
    let mut rng = cx.rng.clone();
    let mut events = vec![];
    for owner in Side::ALL {
        if side(&draft, owner)?.planning.key.as_ref() != Some(&key(&draft)) {
            return Err(invariant("replacement finish has a stale procedure key"));
        }
        let phase = side(&draft, owner)?.planning.phase.clone();
        match phase {
            ProcedurePhase::Awaiting { submitted: Some(ReplacementPlan::Schedule(plan)) } => {
                let mut preparation = draft.clone();
                side_mut(&mut preparation, owner)?.planning.phase = ProcedurePhase::Awaiting { submitted: None };
                let prepared = prepare_schedule(content, &preparation, SeatId::new(owner, Role::Logistics), &plan).map_err(|e| match e { Rejection::Engine(error) => error, _ => invariant("accepted production no longer satisfies public/own prerequisites") })?;
                finish_schedule(content, &mut draft, &prepared, &mut Cx { rng: &mut rng, events: &mut events })?;
                let ProcedurePhase::Distributing { determined, .. } = &side(&draft, owner)?.planning.phase else { return Err(invariant("production did not enter distribution")); };
                let space = distribution_space(determined);
                open(&mut draft, &mut Cx { rng: &mut rng, events: &mut events }, SeatId::new(owner, Role::Logistics), DISTRIBUTION,
                    "Distribute the committed replacement counts across arrival OpStages. Zero results use the same window.".into(),
                    &["land:20.73", "land:20.74"], Trigger::Scheduled, Secrecy::SecretSimultaneous, space);
            }
            ProcedurePhase::Distributing { submitted: Some(plan), .. } => finish_distribution(&mut draft, owner, &plan)?,
            ProcedurePhase::Resolved => {},
            _ => return Err(invariant("replacement finish lacks its complete accepted answer")),
        }
    }
    *state = draft;
    *cx.rng = rng;
    cx.events.extend(events);
    Ok(())
}
