//! Initial Logistics assignment only; no unit command, training credit or absorption.
use super::scheduling::{key, kind, side, side_mut};
use super::*;
use crate::{
    CnaContent, State,
    state::Pending,
    steps::{illegal, open},
};
use cna_content::scenario::Placement;
use cna_core::{
    decision::{ActionSchema, ActionSpace, FieldSchema, Secrecy, Trigger},
    engine::{Cx, Rejection},
    ids::SeatId,
};
use cna_protocol::Role;
use cna_tables::land::training::TrainingSubject;

pub(super) const ANCHOR: &str = "opstage.convoy_arrival";
pub(crate) const ARRIVAL: &str = "cna.replacements.arrival";

#[cfg(test)]
mod recipient_nation_tests {
    use super::*;

    /// Cases: land:20.41, land:20.67
    #[test]
    fn same_side_italian_recipient_does_not_accept_german_infantry() {
        let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut state = State::new(&content).unwrap();
        let id = UnitId::new("it.1_libyan_div.viii_libyan_bn");
        assert_eq!(content.units.units[&id].side, Side::Axis);
        assert_eq!(content.units.units[&id].nationality, "italian");
        // Explicit reduced-fixture placement isolates nationality from the side/live gates;
        // it establishes no scenario deployment or motion entitlement.
        state.land.units.get_mut(&id).unwrap().location =
            cities(&content, false).unwrap()[0].1.clone();
        state.land.units.get_mut(&id).unwrap().toe = Some(cna_content::units::Toe::Normal(
            cna_content::units::NormalToe::N,
        ));
        let before = serde_json::to_vec(&state).unwrap();
        assert!(
            recipient(
                &content,
                &state,
                Side::Axis,
                &id,
                &ReplacementKind::Infantry {
                    nation: ReplacementNation::Italian
                }
            )
            .is_ok()
        );
        assert!(
            matches!(recipient(&content, &state, Side::Axis, &id, &ReplacementKind::Infantry { nation: ReplacementNation::German }), Err(Rejection::Illegal { message }) if message.contains("nation"))
        );
        assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    }
}

fn stage(state: &State) -> Result<OpStageKey, EngineError> {
    let op_stage = state
        .cursor
        .op_stage
        .filter(|op| (1..=3).contains(op))
        .ok_or_else(|| invariant("replacement arrival outside an OpStage"))?;
    Ok(OpStageKey {
        game_turn: state.cursor.game_turn,
        op_stage,
    })
}

pub(super) fn due(state: &State, owner: Side) -> Result<Vec<ScheduledReplacement>, EngineError> {
    let stage = stage(state)?;
    Ok(side(state, owner)?
        .orders
        .iter()
        .filter(|order| order.due == stage)
        .cloned()
        .collect())
}

pub(super) fn cities(
    content: &CnaContent,
    strict: bool,
) -> Result<Vec<(String, Location)>, EngineError> {
    let mut choices = vec![];
    for city in ["cairo", "alexandria"] {
        let locations = crate::setup::placement::choices_with_profile(
            content,
            &Placement::City { city: city.into() },
            Side::Commonwealth,
            "land:20.76",
            strict,
        )?;
        choices.extend(
            locations
                .into_iter()
                .map(|location| (city.into(), location)),
        );
    }
    Ok(choices)
}

fn training_subject(
    content: &CnaContent,
    body: &ReplacementKind,
    recipient: &UnitId,
) -> Result<TrainingSubject, EngineError> {
    match body {
        ReplacementKind::Infantry { .. } => {
            let source = content
                .units
                .units
                .get(recipient)
                .ok_or_else(|| invariant("recipient source vanished"))?;
            // The source identifies this actual Layforce unit as the Commando composite.
            // Its generic infantry_kind is a counter subtype, not a training category.
            if source.id == UnitId::new("cw.unassigned_inf.a_bn_special_service_bde") {
                return Ok(TrainingSubject::Commando);
            }
            // Specialized training requires an explicit source classification in its consumer.
            // An unclassified special draft cannot inherit ordinary infantry's duration.
            if source.infantry_kind.is_none()
                || source
                    .class
                    .as_ref()
                    .is_some_and(|class| matches!(class.as_str(), "cw.h" | "cw.j"))
            {
                return Err(gap(
                    "land:20.43",
                    "recipient lacks a verified non-commando infantry training classification",
                ));
            }
            Ok(TrainingSubject::InfantryExceptCommando)
        }
        ReplacementKind::Gun { .. } => Ok(TrainingSubject::Gun),
        ReplacementKind::Tank { .. } | ReplacementKind::Recce { .. } => {
            Ok(TrainingSubject::TankOrRecce)
        }
    }
}

/// Initial assignment is Logistics authority, not an act by the receiving unit.
/// Later training/allocation/motion uses the recipient's resolved command owner.
/// Cases: land:20.41, land:20.42, land:20.67
fn recipient(
    content: &CnaContent,
    state: &State,
    owner: Side,
    id: &UnitId,
    body: &ReplacementKind,
) -> Result<(), Rejection> {
    let live = state
        .land
        .units
        .get(id)
        .filter(|unit| {
            unit.side == owner
                && matches!(
                    unit.location,
                    Location::Hex { .. } | Location::OffMap { .. }
                )
                && !state.land.arrivals.withdrawn_units.contains(id)
        })
        .ok_or_else(|| illegal("not an own living replacement recipient"))?;
    let source = content
        .units
        .units
        .get(&live.id)
        .ok_or_else(|| Rejection::Engine(invariant("recipient source missing")))?;
    let class = source
        .class
        .as_ref()
        .and_then(|id| content.units.classes.get(id))
        .ok_or_else(|| {
            Rejection::Engine(gap(
                "land:20.3",
                "recipient has no verified type for replacement assignment",
            ))
        })?;
    if class.unit_type != "headquarters" && crate::view::toe_points(content, live) == Some(0) {
        return Err(illegal("eliminated zero-strength replacement recipient"));
    }
    let nation = match body {
        ReplacementKind::Infantry { nation }
        | ReplacementKind::Gun { nation, .. }
        | ReplacementKind::Tank { nation, .. }
        | ReplacementKind::Recce { nation, .. } => *nation,
    };
    let compatible_nation = match nation {
        ReplacementNation::Commonwealth => {
            owner == Side::Commonwealth && source.side == Side::Commonwealth
        }
        ReplacementNation::German => source.nationality == "german",
        ReplacementNation::Italian => source.nationality == "italian",
    };
    if !compatible_nation {
        return Err(illegal(
            "recipient nation is incompatible with the production row",
        ));
    }
    let compatible_type = class.unit_type == "headquarters"
        || match body {
            ReplacementKind::Infantry { .. } => {
                matches!(class.unit_type.as_str(), "infantry" | "engineer" | "recce")
            }
            ReplacementKind::Gun { .. } => matches!(
                class.unit_type.as_str(),
                "artillery" | "anti_tank" | "anti_air"
            ),
            ReplacementKind::Tank { .. } => class.unit_type == "tank",
            ReplacementKind::Recce { .. } => class.unit_type == "recce",
        };
    if !compatible_type {
        return Err(illegal(
            "recipient type is incompatible with the replacement class",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(super) struct PreparedArrival {
    owner: Side,
    key: ProcedureKey,
    plan: ArrivalPlan,
    limit: CWArrivalLimit,
    cities: Vec<(String, Location)>,
}

/// The limit is trusted caller-owned live state, never an actor quota assertion.
/// Validate complete due-order conservation and the cumulative GT constraints.
/// Cases: land:20.41, land:20.76, land:20.78A, land:20.78B
pub(super) fn prepare(
    content: &CnaContent,
    state: &State,
    seat: SeatId,
    plan: &ArrivalPlan,
    limit: &CWArrivalLimit,
    strict: bool,
) -> Result<PreparedArrival, Rejection> {
    if seat.role != Role::Logistics || state.cursor.anchor() != ANCHOR {
        return Err(illegal("not the initial replacement arrival window"));
    }
    let current = key(state);
    let owned = side(state, seat.side).map_err(Rejection::Engine)?;
    if owned.arrival.key.as_ref() != Some(&current)
        || !matches!(
            owned.arrival.phase,
            ProcedurePhase::Awaiting { submitted: None }
        )
    {
        return Err(illegal("replacement arrival window is closed"));
    }
    let orders = due(state, seat.side).map_err(Rejection::Engine)?;
    if seat.side == Side::Axis {
        return Err(Rejection::Engine(gap(
            "land:20.63",
            "real replacement shipping, port reservations and equipment delivery remain a separate slice",
        )));
    }
    if limit.game_turn != state.cursor.game_turn
        || limit.total_trucks < 0
        || limit.alexandria_trucks < 0
        || i64::from(limit.alexandria_trucks) * 4 > i64::from(limit.total_trucks)
        || limit
            .infantry_city
            .as_deref()
            .is_some_and(|city| !matches!(city, "cairo" | "alexandria"))
    {
        return Err(Rejection::Engine(invariant(
            "invalid retained arrival constraint",
        )));
    }
    if plan.choices.len() != orders.len() {
        return Err(illegal("place every due replacement order exactly once"));
    }
    let mut next_limit = limit.clone();
    let locations = if orders.is_empty() {
        vec![]
    } else {
        cities(content, strict).map_err(Rejection::Engine)?
    };
    let mut used = BTreeSet::new();
    for choice in &plan.choices {
        let (id, location) = match choice {
            ArrivalChoice::Points {
                order, location, ..
            }
            | ArrivalChoice::Trucks {
                order, location, ..
            } => (order, location),
        };
        let order = orders
            .iter()
            .find(|order| order.id == *id)
            .ok_or_else(|| illegal("not a due own replacement order"))?;
        if order.points <= 0
            || order.row.side() != seat.side
            || order.kind != kind(content, &order.row).map_err(Rejection::Engine)?
        {
            return Err(Rejection::Engine(invariant(
                "corrupt due replacement order",
            )));
        }
        if owned.counters.iter().any(|counter| counter.id == order.id)
            || orders.iter().filter(|other| other.id == order.id).count() != 1
        {
            return Err(Rejection::Engine(invariant(
                "duplicate physical/order replacement identity",
            )));
        }
        if !used.insert(id) {
            return Err(illegal("replacement order selected twice"));
        }
        let matches: Vec<_> = locations
            .iter()
            .filter(|(_, candidate)| candidate == location)
            .collect();
        if matches.len() != 1 {
            return Err(illegal("not a verified replacement arrival city location"));
        }
        let city = &matches[0].0;
        match (choice, &order.kind) {
            (ArrivalChoice::Points { recipient: id, .. }, ProducedKind::Points { kind: body }) => {
                recipient(content, state, seat.side, id, body)?;
                training_subject(content, body, id).map_err(Rejection::Engine)?;
                if matches!(body, ReplacementKind::Infantry { .. }) {
                    if next_limit
                        .infantry_city
                        .as_ref()
                        .is_some_and(|prior| prior != city)
                    {
                        return Err(illegal("all infantry for this Game-Turn must use one city"));
                    }
                    next_limit.infantry_city = Some(city.clone());
                }
            }
            (
                ArrivalChoice::Trucks {
                    pool_target: ProductionPoolTarget::Create,
                    ..
                },
                ProducedKind::Trucks { .. },
            ) => {
                if city == "alexandria" {
                    next_limit.alexandria_trucks = next_limit
                        .alexandria_trucks
                        .checked_add(order.points)
                        .ok_or_else(|| illegal("arrival quota overflow"))?;
                    if i64::from(next_limit.alexandria_trucks) * 4
                        > i64::from(next_limit.total_trucks)
                    {
                        return Err(illegal(
                            "Alexandria exceeds one quarter of this Game-Turn's total produced trucks",
                        ));
                    }
                }
            }
            (
                ArrivalChoice::Trucks {
                    pool_target: ProductionPoolTarget::Existing { .. },
                    ..
                },
                ProducedKind::Trucks { .. },
            ) => {
                return Err(Rejection::Engine(gap(
                    "land:20.78A",
                    "merging produced trucks into a funded moving cohort remains an unimplemented physical adapter",
                )));
            }
            _ => return Err(illegal("arrival choice does not match the produced type")),
        }
    }
    Ok(PreparedArrival {
        owner: seat.side,
        key: current,
        plan: plan.clone(),
        limit: next_limit,
        cities: locations,
    })
}

/// Publish real unabsorbed counters or genuine new pools with no fabricated stock/funding.
/// Counter identity transitions from its due order; no second serial or UnitId is minted.
/// Cases: land:20.41, land:20.43, land:20.76, land:20.78A, land:20.78B
pub(super) fn finish(
    content: &CnaContent,
    state: &mut State,
    prepared: &PreparedArrival,
) -> Result<CWArrivalLimit, EngineError> {
    if key(state) != prepared.key {
        return Err(invariant("stale prepared replacement arrival"));
    }
    let ProcedurePhase::Awaiting {
        submitted: Some(ReplacementPlan::Arrival(accepted)),
    } = &side(state, prepared.owner)?.arrival.phase
    else {
        return Err(invariant("replacement arrival lacks accepted answer"));
    };
    if serde_json::to_value(accepted).ok() != serde_json::to_value(&prepared.plan).ok() {
        return Err(invariant("arrival differs from its accepted plan"));
    }
    let orders = due(state, prepared.owner)?;
    let current_stage = stage(state)?;
    let mut draft = state.clone();
    for choice in &prepared.plan.choices {
        let id = match choice {
            ArrivalChoice::Points { order, .. } | ArrivalChoice::Trucks { order, .. } => order,
        };
        let order = orders
            .iter()
            .find(|order| order.id == *id)
            .ok_or_else(|| invariant("prepared arrival order vanished"))?;
        match (choice, &order.kind) {
            (
                ArrivalChoice::Points {
                    recipient,
                    location,
                    ..
                },
                ProducedKind::Points { kind: body },
            ) => {
                let required_stages = content
                    .tables
                    .land
                    .training
                    .op_stages(training_subject(content, body, recipient)?)
                    .try_into()
                    .map_err(|_| invariant("replacement training duration overflow"))?;
                side_mut(&mut draft, prepared.owner)?
                    .counters
                    .push(ReplacementCounter {
                        id: order.id.clone(),
                        kind: body.clone(),
                        points: order.points,
                        location: location.clone(),
                        recipient: recipient.clone(),
                        training: Training::Training {
                            required_stages,
                            completed: 0,
                            last_counted: None,
                            candidate: None,
                        },
                        carriage: None,
                        physical: ReplacementPhysical {
                            stage: current_stage,
                            cp_quarters: 0,
                            cohesion_quarters: 0,
                            breakdown_points: 0,
                            checked_column: None,
                            on_network: false,
                            supplies: Supplies::default(),
                            tank_fuel: FuelTenths::ZERO,
                            water: WaterPoints::ZERO,
                            activity: None,
                            funding: None,
                        },
                    });
            }
            (
                ArrivalChoice::Trucks {
                    location,
                    pool_target: ProductionPoolTarget::Create,
                    ..
                },
                ProducedKind::Trucks { kind },
            ) => {
                let mut trucks = Trucks::default();
                match kind {
                    FuelTruckKind::Light => trucks.light = order.points,
                    FuelTruckKind::Medium => trucks.medium = order.points,
                    FuelTruckKind::Heavy => trucks.heavy = order.points,
                }
                let city = prepared
                    .cities
                    .iter()
                    .find(|(_, candidate)| candidate == location)
                    .ok_or_else(|| invariant("prepared pool city vanished"))?
                    .0
                    .clone();
                crate::logistics::pools::add_truck_pool(
                    &mut draft.logistics,
                    None,
                    prepared.owner,
                    Placement::City { city },
                    Some(location.clone()),
                    trucks,
                    Supplies::default(),
                )
                .map_err(invariant)?;
            }
            _ => return Err(invariant("prepared replacement type changed")),
        }
        side_mut(&mut draft, prepared.owner)?
            .orders
            .retain(|prior| prior.id != *id);
    }
    side_mut(&mut draft, prepared.owner)?.arrival.phase = ProcedurePhase::Resolved;
    *state = draft;
    Ok(prepared.limit.clone())
}

fn retained_limit(state: &State, owner: Side) -> Result<&CWArrivalLimit, EngineError> {
    let own = side(state, owner)?;
    let mut entries = own
        .cw_arrival_limits
        .iter()
        .filter(|limit| limit.game_turn == state.cursor.game_turn);
    let limit = entries.next().ok_or_else(|| gap("land:20.78A", "legacy arrival GT has no authenticated produced-total/city limit; current mobile stock cannot reconstruct it"))?;
    if entries.next().is_some() {
        return Err(invariant("duplicate retained arrival GT"));
    }
    Ok(limit)
}

fn space(orders: &[ScheduledReplacement], locations: &[(String, Location)]) -> ActionSpace {
    ActionSpace::new(ActionSchema::Record { fields: vec![FieldSchema {
        name: "choices".into(), doc: "One initial assignment per due order; no absorption, command or training credit.".into(), optional: false,
        schema: ActionSchema::List { min: orders.len() as u32, max: orders.len() as u32,
            item: Box::new(ActionSchema::Record { fields: vec![
                FieldSchema { name: "order".into(), doc: format!("Due own order IDs: {:?}", orders.iter().map(|order| &order.id).collect::<Vec<_>>()), optional: false,
                    schema: ActionSchema::Text { min_length: 1, max_length: 100 } },
                FieldSchema { name: "recipient".into(), doc: "Real own living unit for points; absent for a new truck pool.".into(), optional: true,
                    schema: ActionSchema::Text { min_length: 1, max_length: 200 } },
                FieldSchema { name: "location".into(), doc: format!("Canonical city locations by index: {locations:?}"), optional: false,
                    schema: ActionSchema::Integer { min: 0, max: locations.len().saturating_sub(1) as i64 } },
            ] }) },
    }] }).with_pass("Pass only when no replacements are due. Axis equipment delivery remains cited Unsupported.")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArrivalAnswer {
    choices: Vec<InitialAssignment>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialAssignment {
    order: String,
    recipient: Option<String>,
    location: usize,
}

fn decode(
    content: &CnaContent,
    state: &State,
    owner: Side,
    action: &serde_json::Value,
    strict: bool,
) -> Result<ArrivalPlan, Rejection> {
    let orders = due(state, owner).map_err(Rejection::Engine)?;
    if action.is_null() {
        if !orders.is_empty() {
            return Err(illegal("due replacement orders require initial assignment"));
        }
        return Ok(ArrivalPlan { choices: vec![] });
    }
    let answer: ArrivalAnswer = serde_json::from_value(action.clone())
        .map_err(|_| illegal("invalid initial assignment record"))?;
    let locations = if orders.is_empty() {
        vec![]
    } else {
        cities(content, strict).map_err(Rejection::Engine)?
    };
    let mut choices = vec![];
    for choice in answer.choices {
        let order = orders
            .iter()
            .find(|order| order.id == choice.order)
            .ok_or_else(|| illegal("not a due own order"))?;
        let location = locations
            .get(choice.location)
            .ok_or_else(|| illegal("not a declared city index"))?
            .1
            .clone();
        choices.push(match (&order.kind, choice.recipient) {
            (ProducedKind::Points { .. }, Some(recipient)) => ArrivalChoice::Points {
                order: choice.order,
                recipient: UnitId::new(recipient),
                location,
            },
            (ProducedKind::Trucks { .. }, None) => ArrivalChoice::Trucks {
                order: choice.order,
                location,
                pool_target: ProductionPoolTarget::Create,
            },
            _ => return Err(illegal("recipient field does not match produced type")),
        });
    }
    Ok(ArrivalPlan { choices })
}

/// Fixed public stage/Logistics schedule, including no-due and zero/pass cases.
/// The schema contains only that seat's due orders; no teammate order disclosure.
/// Cases: land:20.41, land:20.76, land:20.78A, land:20.78B, land:3.6
pub(crate) fn enter_arrival(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let current = key(state);
    stage(state)?;
    for owner in Side::ALL {
        if !state
            .land
            .replacements
            .sides
            .iter()
            .any(|own| own.side == owner)
        {
            state
                .land
                .replacements
                .sides
                .push(SideReplacements::new(owner));
        }
        if side(state, owner)?.arrival.key.as_ref() == Some(&current) {
            continue;
        }
        side_mut(state, owner)?
            .cw_arrival_limits
            .retain(|limit| limit.game_turn >= current.segment.game_turn);
        let orders = due(state, owner)?;
        let locations = if orders.is_empty() || owner == Side::Axis {
            vec![]
        } else {
            cities(content, strict)?
        };
        side_mut(state, owner)?.arrival = ReplacementProcedure {
            key: Some(current.clone()),
            phase: ProcedurePhase::Awaiting { submitted: None },
        };
        open(state, cx, SeatId::new(owner, Role::Logistics), ARRIVAL,
            "Initial replacement assignment. The fixed window includes a declared no-due pass; later training, command, carriage and absorption are separate.".into(),
            &["land:20.41", "land:20.76", "land:20.78A", "land:20.78B"], Trigger::Scheduled, Secrecy::SecretSimultaneous, space(&orders, &locations));
    }
    Ok(())
}

/// Buffer only; no produced-total/city update, order removal, stock or counter change.
/// Cases: land:20.41, land:20.78A, land:20.78B
pub(crate) fn answer_arrival(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &serde_json::Value,
    strict: bool,
) -> Result<String, Rejection> {
    if pending.kind != ARRIVAL
        || pending.seat.role != Role::Logistics
        || state.cursor.anchor() != ANCHOR
    {
        return Err(illegal("not the initial Logistics assignment window"));
    }
    let owner = pending.seat.side;
    let own = side(state, owner).map_err(Rejection::Engine)?;
    if own.arrival.key.as_ref() != Some(&key(state))
        || !matches!(
            own.arrival.phase,
            ProcedurePhase::Awaiting { submitted: None }
        )
    {
        return Err(illegal("initial assignment window is closed"));
    }
    if owner == Side::Axis && !due(state, owner).map_err(Rejection::Engine)?.is_empty() {
        return Err(Rejection::Engine(gap(
            "land:20.63",
            "Axis delivery requires the separately implemented real naval/port adapter",
        )));
    }
    let plan = decode(content, state, owner, action, strict)?;
    if !due(state, owner).map_err(Rejection::Engine)?.is_empty() {
        prepare(
            content,
            state,
            pending.seat,
            &plan,
            retained_limit(state, owner).map_err(Rejection::Engine)?,
            strict,
        )?;
    } else if !plan.choices.is_empty() {
        return Err(illegal("no replacements are due"));
    }
    side_mut(state, owner)
        .map_err(Rejection::Engine)?
        .arrival
        .phase = ProcedurePhase::Awaiting {
        submitted: Some(ReplacementPlan::Arrival(plan)),
    };
    Ok("Recorded initial assignment for finish, with no training or unit-command action.".into())
}

pub(crate) fn arrivals_complete(state: &State) -> bool {
    Side::ALL.iter().all(|owner| {
        side(state, *owner).is_ok_and(|own| {
            own.arrival.key.as_ref() == Some(&key(state))
                && matches!(own.arrival.phase, ProcedurePhase::Resolved)
        })
    })
}

/// Whole-side atomic finish: retained quota/city and actual bodies publish together.
/// Required legacy limits are never defaulted or reconstructed after movement/loss.
/// Cases: land:20.41, land:20.43, land:20.78A, land:20.78B
pub(crate) fn finish_arrivals(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
) -> Result<(), EngineError> {
    if state
        .decisions
        .pending
        .iter()
        .any(|pending| pending.kind == ARRIVAL)
    {
        return Ok(());
    }
    let mut draft = state.clone();
    for owner in Side::ALL {
        let own = side(&draft, owner)?;
        if own.arrival.key.as_ref() != Some(&key(&draft)) {
            return Err(invariant("stale arrival procedure"));
        }
        let phase = own.arrival.phase.clone();
        match phase {
            ProcedurePhase::Resolved => {}
            ProcedurePhase::Awaiting {
                submitted: Some(ReplacementPlan::Arrival(plan)),
            } => {
                if due(&draft, owner)?.is_empty() {
                    if !plan.choices.is_empty() {
                        return Err(invariant("accepted assignment has no due orders"));
                    }
                    side_mut(&mut draft, owner)?.arrival.phase = ProcedurePhase::Resolved;
                    continue;
                }
                let limit = retained_limit(&draft, owner)?.clone();
                let mut preflight = draft.clone();
                side_mut(&mut preflight, owner)?.arrival.phase =
                    ProcedurePhase::Awaiting { submitted: None };
                let prepared = prepare(
                    content,
                    &preflight,
                    SeatId::new(owner, Role::Logistics),
                    &plan,
                    &limit,
                    strict,
                )
                .map_err(|error| match error {
                    Rejection::Engine(error) => error,
                    _ => invariant("accepted assignment no longer meets prerequisites"),
                })?;
                let updated = finish(content, &mut draft, &prepared)?;
                let entry = side_mut(&mut draft, owner)?
                    .cw_arrival_limits
                    .iter_mut()
                    .find(|limit| limit.game_turn == updated.game_turn)
                    .ok_or_else(|| invariant("retained quota vanished during arrival"))?;
                *entry = updated;
            }
            _ => {
                return Err(invariant(
                    "initial arrival lacks a complete accepted answer",
                ));
            }
        }
    }
    *state = draft;
    Ok(())
}
