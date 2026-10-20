//! Closed operational assignment, path queries and interception commitments.
//! The actual component callers stay behind the uniform public prelaunch guard.

use super::*;
use cna_core::{
    decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger},
    dice::CampaignRng,
    engine::{Cx, Rejection},
    event::EngineEvent,
    ids::SeatId,
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role};
use serde_json::{Value, json};
use crate::{state::Pending, steps::{illegal, open}};
use crate::air::{inventory, missions::{self, DeclarationTarget, MissionDeclaration, MissionId, PlaneDeclaration}};

pub const ASSIGN: &str = "cna.air.flight_routes_and_cap";
pub const QUERY: &str = "cna.air.cap_path_queries";
pub const COMMIT: &str = "cna.air.cap_interception_commitments";
const CAP: &str = "combat_air_patrol_offensive_defensive";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Batch { Assignment, Query, Commit, Closed }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Posture { Offensive, Defensive }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CapOrder {
    plane: PlaneId,
    mode: usize,
    #[serde(default)]
    rated_pilot: Option<PilotId>,
    target: HexId,
    path: Vec<HexId>,
    posture: Posture,
    neighbors: Vec<HexId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Assignment {
    routes: Vec<RouteSelection>,
    caps: Vec<CapOrder>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireAssignment {
    routes: Option<Vec<RouteSelection>>,
    caps: Option<Vec<CapOrder>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AssignedCap {
    owner: Side,
    plane: PlaneId,
    hex: HexId,
    posture: Posture,
    neighbors: Vec<HexId>,
}

/// A public contact is the request key. Only its distinct paths are revealed.
/// Cases: airlog:34.73, airlog:40.27
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Disclosure {
    contact: Contact,
    paths: Vec<Vec<HexId>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InterceptOrder {
    route: String,
    hex: HexId,
    fighters: Vec<PlaneId>,
}

/// A queue for the later real encounter consumer, not a combat result or loss.
/// Identity is the entire cross-class enemy target/path, never a MissionId.
/// Cases: airlog:40.27
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Interception {
    pub owner: Side,
    pub enemy: Side,
    pub target: HexId,
    pub path: Vec<HexId>,
    pub hex: HexId,
    pub fighters: Vec<PlaneId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct PlanningState {
    batch: Option<Batch>,
    assignments: BTreeMap<Side, Assignment>,
    queries: BTreeMap<Side, Vec<Contact>>,
    disclosures: BTreeMap<Side, Vec<Disclosure>>,
    commitments: BTreeMap<Side, Vec<InterceptOrder>>,
    caps: Vec<AssignedCap>,
    interceptions: Vec<Interception>,
    used: BTreeSet<PlaneId>,
}

fn context(state: &State) -> Result<Context, EngineError> {
    let expected = Context::from_state(state)?;
    if expected.phase != Phase::Deployment { return Err(invalid("not the deployment window phase")); }
    Ok(expected)
}

fn b(n: usize) -> u32 { u32::try_from(n).unwrap_or(u32::MAX) }
fn field(name: &str, schema: ActionSchema, optional: bool) -> FieldSchema {
    FieldSchema { name: name.into(), doc: name.replace('_', " "), schema, optional }
}
fn choice(ids: impl IntoIterator<Item=String>) -> ActionSchema {
    ActionSchema::Choice { options: ids.into_iter().map(|id| ChoiceOption { label: id.clone(), id, detail: None }).collect() }
}
fn list(item: ActionSchema, min: u32, max: usize) -> ActionSchema {
    ActionSchema::List { item: Box::new(item), min, max: b(max) }
}
fn record(fields: Vec<FieldSchema>) -> ActionSchema { ActionSchema::Record { fields } }
fn own_missions(state: &State, owner: Side) -> Vec<MissionId> {
    state.air.runtime.tactical.declarations.iter().filter(|(_, m)| {
        !m.planes.is_empty() && m.planes.iter().all(|p| state.air.runtime.aircraft.get(&p.plane)
            .is_some_and(|p| side(&p.force) == Ok(owner)))
            && matches!(m.target, DeclarationTarget::Hex(_))
    }).map(|(id, _)| id.clone()).collect()
}
fn free_planes(state: &State, owner: Side) -> Vec<PlaneId> {
    let committed: BTreeSet<_> = state.air.runtime.tactical.declarations.values()
        .flat_map(|m| m.planes.iter().map(|p| &p.plane)).collect();
    missions::own_planes(state, owner).into_iter().filter(|id| !committed.contains(id)).collect()
}
fn neighbors(content: &CnaContent, hex: &HexId) -> BTreeSet<HexId> {
    content.map.neighbors(hex).iter().map(|h| h.id.clone()).collect()
}
fn known_routes(state: &State, owner: Side) -> Vec<(Side, HexId, Vec<HexId>)> {
    state.air.runtime.operations.planning.disclosures.get(&owner).into_iter().flatten()
        .flat_map(|d| d.paths.iter().map(|p| (d.contact.owner, d.contact.target.clone(), p.clone())))
        .collect::<BTreeSet<_>>().into_iter().collect()
}
fn offensive(state: &State, owner: Side) -> Vec<&AssignedCap> {
    state.air.runtime.operations.planning.caps.iter()
        .filter(|c| c.owner == owner && c.posture == Posture::Offensive).collect()
}

fn available(state: &State, owner: Side) -> Vec<&AssignedCap> {
    offensive(state, owner).into_iter().filter(|c| state.air.runtime.aircraft.contains_key(&c.plane)
        && !state.air.runtime.operations.planning.used.contains(&c.plane)).collect()
}

/// This domain reads own committed identities, public geometry/contacts and
/// answers already disclosed to this requester. No enemy membership is queried.
/// Cases: airlog:39.16, airlog:40.21, airlog:40.27
fn space(content: &CnaContent, state: &State, owner: Side, batch: Batch) -> ActionSpace {
    let ops = &state.air.runtime.operations;
    match batch {
        Batch::Assignment => {
            let own = own_missions(state, owner);
            let planes = free_planes(state, owner);
            let roster = missions::own_planes(state, owner);
            let max_range = missions::own_planes(state, owner).iter().filter_map(|id| {
                content.units.aircraft.get(&state.air.runtime.aircraft[id].aircraft)
            }).flat_map(|a| &a.modes).filter_map(|m| m.range_hexes).max().unwrap_or(0).max(0);
            let path = list(ActionSchema::Hex { among: None }, 1, usize::try_from(max_range).unwrap_or(0).saturating_add(1));
            let modes = planes.iter().filter_map(|id| content.units.aircraft.get(&state.air.runtime.aircraft[id].aircraft))
                .map(|a| a.modes.len().saturating_sub(1)).max().unwrap_or(0);
            let domain = ActionSpace::new(record(vec![
                field("routes", list(record(vec![
                    field("mission", choice(own.iter().map(|m| m.0.clone())), false),
                    field("path", path.clone(), false),
                ]), b(own.len()), own.len()), true),
                field("caps", list(record(vec![
                    field("plane", choice(planes.iter().map(|p| p.0.clone())), false),
                    field("mode", ActionSchema::Integer { min: 0, max: i64::try_from(modes).unwrap_or(i64::MAX) }, false),
                    field("rated_pilot", choice(state.air.runtime.pilots.iter().filter(|(_, p)| side(&p.force) == Ok(owner)).map(|(id, _)| id.0.clone())), true),
                    field("target", ActionSchema::Hex { among: None }, false),
                    field("path", path, false),
                    field("posture", choice(["offensive".into(), "defensive".into()]), false),
                    field("neighbors", list(ActionSchema::Hex { among: None }, 0, 6), false),
                ]), 0, planes.len()), true),
            ])).with_context(json!({"missions": own.iter().map(|id| (id, &state.air.runtime.tactical.declarations[id])).collect::<BTreeMap<_,_>>(),
                "free_planes":planes,
                "planes":roster.iter().map(|id| (id, &state.air.runtime.aircraft[id])).collect::<BTreeMap<_,_>>(),
                "preparation":roster.iter().filter_map(|id|state.air.runtime.preparation.current.get(id).map(|p|(id,p))).collect::<BTreeMap<_,_>>(),
                "initial_choices":roster.iter().filter_map(|id|state.air.runtime.preparation.initial_choices.get(id).map(|p|(id,p))).collect::<BTreeMap<_,_>>(),
                "pilots":state.air.runtime.pilots.iter().filter(|(_,p)|side(&p.force)==Ok(owner)).collect::<BTreeMap<_,_>>() }));
            if own.is_empty() { domain.with_pass("Assign no executable routes or CAP.") } else { domain }
        }
        Batch::Query => {
            let contacts: Vec<_> = ops.contacts.iter().filter(|c| c.owner != owner).cloned().collect();
            let max = if available(state, owner).is_empty() { 0 } else { contacts.len() };
            ActionSpace::new(list(choice((0..contacts.len()).map(|i| i.to_string())), 0, max))
                .with_pass("Ask for no enemy paths.").with_context(json!({"contacts":contacts,"own_caps":offensive(state,owner)}))
        }
        Batch::Commit => {
            let routes = known_routes(state, owner);
            let own = available(state, owner);
            let hexes = routes.iter().flat_map(|(_, _, p)| p.iter().cloned()).collect::<BTreeSet<_>>();
            let max = if routes.is_empty() || own.is_empty() { 0 } else { own.len() };
            ActionSpace::new(list(record(vec![
                field("route", choice((0..routes.len()).map(|i| i.to_string())), false),
                field("hex", ActionSchema::Hex { among: Some(hexes.iter().cloned().collect()) }, false),
                field("fighters", list(choice(own.iter().map(|c| c.plane.0.clone())), 1, own.len()), false),
            ]), 0, max)).with_pass("Commit no optional interceptions.")
                .with_context(json!({"routes":routes,"own_caps":own}))
        }
        Batch::Closed => ActionSpace::new(list(ActionSchema::Bool, 0, 0)).with_pass("No further commitment in this period."),
    }
}

fn path_check(content: &CnaContent, path: &[HexId], target: &HexId) -> Result<(), EngineError> {
    if path.is_empty() || path.last() != Some(target)
        || path.iter().any(|h| content.map.canonical(h) != Some(h))
        || path.windows(2).any(|p| !neighbors(content, &p[0]).contains(&p[1])) {
        return Err(invalid("route must be nonempty, canonical and adjacent with the exact target"));
    }
    Ok(())
}

// Only own launch prerequisites are inspected here, before buffering. Enemy
// interference remains exclusively in the closed combined launch producer.
fn own_launch_check(content: &CnaContent, state: &State, id: &PlaneId, mode: usize, path: &[HexId]) -> Result<(), EngineError> {
    use crate::air::preparation::{KnownAmmunition, OrdnanceKnowledge};
    let plane = &state.air.runtime.aircraft[id];
    let source = content.units.aircraft.get(&plane.aircraft).ok_or_else(|| unsupported("airlog:34.6", "own aircraft source is missing"))?;
    let selected = source.modes.get(mode).ok_or_else(|| invalid("not an own aircraft mode"))?;
    let range = selected.range_hexes.ok_or_else(|| unsupported("airlog:37.12", "own mode has no printed ordinary range"))?;
    if !plane.refitted || i32::try_from(path.len().saturating_sub(1)).ok().is_none_or(|n| n > range) {
        return Err(invalid("own launch exceeds its source range or lacks refit"));
    }
    let current = state.air.runtime.preparation.current.get(id).ok_or_else(|| invalid("own preparation missing"))?;
    let known = KnownAmmunition::from_current(current).ok_or_else(|| unsupported("airlog:38.4", "own load has unresolved initial choice"))?;
    if known.mode != Some(mode) || known.fuel_available != Some(true) || known.gun_ammunition.is_none() || matches!(known.ordnance, OrdnanceKnowledge::Unknown) {
        return Err(unsupported("airlog:38.4", "own flight requires known mode, fuel, guns and ordnance"));
    }
    let origin = ground_facility(content, state, id)?.ok_or_else(|| invalid("own flight is not grounded"))?;
    let facility = &state.air.runtime.facilities[&origin];
    let crate::state::Location::Hex { hex } = facility.properties(content, &origin)?.location else {
        return Err(unsupported("airlog:36.5", "off-map flight remains outside this slice"));
    };
    if path.first() != Some(&hex) || !facility.operational() { return Err(invalid("own path does not begin at its operational origin")); }
    for hex in path {
        if matches!(crate::logistics::weather::at_hex(content, state, hex)?, cna_tables::land::weather::WeatherKind::Sandstorm | cna_tables::land::weather::WeatherKind::Rainstorm) {
            return Err(invalid("own path enters a storm hex"));
        }
    }
    Ok(())
}

fn validate_assignment(content: &CnaContent, state: &State, owner: Side, plan: &Assignment) -> Result<(), EngineError> {
    let expected: BTreeSet<_> = own_missions(state, owner).into_iter().collect();
    let selected: BTreeSet<_> = plan.routes.iter().map(|r| r.mission.clone()).collect();
    if selected != expected || selected.len() != plan.routes.len() { return Err(invalid("each own committed hex mission needs exactly one route")); }
    for route in &plan.routes {
        let mission = &state.air.runtime.tactical.declarations[&route.mission];
        let DeclarationTarget::Hex(target) = &mission.target else { return Err(invalid("not an own hex mission")); };
        path_check(content, &route.path, target)?;
        for selected in &mission.planes {
            own_launch_check(content, state, &selected.plane, selected.mode, &route.path)?;
            let origin = ground_facility(content, state, &selected.plane)?.ok_or_else(|| invalid("own routed plane is not grounded"))?;
            let crate::state::Location::Hex { hex } = state.air.runtime.facilities[&origin].properties(content, &origin)?.location else {
                return Err(unsupported("airlog:36.5", "off-map route producer is not implemented"));
            };
            if route.path.first() != Some(&hex) { return Err(invalid("own route begins away from its actual origin")); }
        }
    }
    let free: BTreeSet<_> = free_planes(state, owner).into_iter().collect();
    let mut used = BTreeSet::new();
    let mut bound_neighbors = BTreeMap::new();
    for cap in &plan.caps {
        if !free.contains(&cap.plane) || !used.insert(&cap.plane) { return Err(invalid("CAP needs a distinct own uncommitted aircraft")); }
        path_check(content, &cap.path, &cap.target)?;
        own_launch_check(content, state, &cap.plane, cap.mode, &cap.path)?;
        let order: BTreeSet<_> = cap.neighbors.iter().cloned().collect();
        if order.len() != cap.neighbors.len() || match cap.posture {
            Posture::Offensive => order != neighbors(content, &cap.target),
            Posture::Defensive => !order.is_empty(),
        } { return Err(invalid("CAP-O needs a complete public-neighbor permutation; CAP-D has none")); }
        if cap.posture == Posture::Offensive {
            if let Some(old) = bound_neighbors.insert(&cap.target, &cap.neighbors)
                && old != &cap.neighbors { return Err(invalid("one CAP hex needs one bound neighbor preference")); }
        }
        let selected = PlaneDeclaration { plane: cap.plane.clone(), mode: cap.mode, rated_pilot: cap.rated_pilot.clone(), initial_choice: None };
        let mission = MissionDeclaration { mission: CAP.into(), light: missions::Light::Day,
            target: DeclarationTarget::Hex(cap.target.clone()), planes: vec![selected] };
        missions::validate_operational(content, state, owner, &[mission])?;
        let current = state.air.runtime.preparation.current.get(&cap.plane).ok_or_else(|| invalid("CAP preparation missing"))?;
        let known = crate::air::preparation::KnownAmmunition::from_current(current)
            .ok_or_else(|| unsupported("airlog:38.4", "CAP requires exact own preparation knowledge"))?;
        if known.mode != Some(cap.mode) || known.fuel_available != Some(true)
            || known.gun_ammunition.is_none() || matches!(known.ordnance, crate::air::preparation::OrdnanceKnowledge::Unknown) {
            return Err(unsupported("airlog:38.4", "CAP cannot infer unknown mode, fuel, guns or ordnance"));
        }
        // Full operational range/weather/refit/SGSU source checks run in the
        // shared launch producer after BOTH assignments have validated.
    }
    Ok(())
}

fn validate_intercepts(content: &CnaContent, state: &State, owner: Side, plan: &[InterceptOrder]) -> Result<Vec<Interception>, EngineError> {
    let known = known_routes(state, owner);
    let caps = offensive(state, owner);
    let mut used = state.air.runtime.operations.planning.used.clone();
    let mut encounters = BTreeSet::new();
    let mut result = Vec::new();
    for order in plan {
        let index: usize = order.route.parse().map_err(|_| invalid("not an authorized path index"))?;
        if index.to_string() != order.route { return Err(invalid("not a canonical path index")); }
        let (opponent, target, path) = known.get(index).ok_or_else(|| invalid("path was not disclosed to this owner"))?;
        if !path.contains(&order.hex) || order.fighters.is_empty()
            || !encounters.insert((target.clone(), path.clone(), order.hex.clone())) { return Err(invalid("distinct interception hex required per complete flight")); }
        for id in &order.fighters {
            let cap = caps.iter().find(|c| c.plane == *id && state.air.runtime.aircraft.contains_key(id)).ok_or_else(|| invalid("not an own offensive CAP fighter"))?;
            if !used.insert(id.clone()) { return Err(invalid("fighter may intercept only once per period")); }
            let assigned = caps.iter().filter(|c| c.hex == cap.hex).count();
            if cap.hex != order.hex && (assigned < 3 || !neighbors(content, &cap.hex).contains(&order.hex)) {
                return Err(invalid("optional adjacent interception needs three ASSIGNED CAP-O fighters"));
            }
        }
        result.push(Interception { owner, enemy: *opponent, target: target.clone(), path: path.clone(), hex: order.hex.clone(), fighters: order.fighters.clone() });
    }
    Ok(result)
}

fn batch_kind(batch: Batch) -> &'static str {
    match batch { Batch::Assignment => ASSIGN, Batch::Query => QUERY, Batch::Commit => COMMIT, Batch::Closed => "closed" }
}

fn open_batch(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>, batch: Batch) {
    for owner in Side::ALL {
        let domain = space(content, state, owner, batch);
        open(state, cx, SeatId::new(owner, Role::Air), batch_kind(batch),
            match batch { Batch::Assignment => "Choose actual own flight paths and CAP posture; no launch until finish.",
                Batch::Query => "Request distinct paths for public enemy contacts; disclosure occurs only at finish.",
                _ => "Commit own CAP-O fighters to disclosed flight paths; no combat or dice in the response." }.into(),
            &["airlog:40.21", "airlog:40.27"], Trigger::Scheduled, Secrecy::SecretSimultaneous, domain);
    }
}

fn apply(content: &CnaContent, state: &mut State, after: AirRuntime, expected: Context) -> Result<(), EngineError> {
    let before = state.air.runtime.clone();
    inventory::update_flight(content, state, FlightAdmission { before, after, expected })
}

/// Actual fixed route/CAP assignment producer, reached publicly only after the
/// static capability lifts. Component controls call this same producer directly.
/// Cases: airlog:39.16, airlog:40.21, airlog:40.24
pub(super) fn enter_assignment(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    let expected = context(state)?;
    if state.air.runtime.operations != OperationalState::default()
        || state.air.runtime.tactical.period != Some(expected.period) || !state.air.runtime.tactical.resolved
        || !state.decisions.pending.is_empty() { return Err(invalid("route assignment requires a closed current declaration and no prior operation")); }
    let mut draft = state.clone();
    let mut after = draft.air.runtime.clone();
    after.operations.period = Some(expected.period);
    after.operations.phase = Some(expected.phase);
    after.operations.planning.batch = Some(Batch::Assignment);
    apply(content, &mut draft, after, expected)?;
    let mut events = Vec::new();
    let mut rng = CampaignRng::from_state(&cx.rng.state());
    open_batch(content, &mut draft, &mut Cx { rng: &mut rng, events: &mut events }, Batch::Assignment);
    *state = draft;
    cx.events.extend(events);
    Ok(())
}

/// Record only own-known choices. Lookup domains never use enemy private
/// flight membership, interception reach, counts or undisclosed paths.
/// Cases: airlog:40.21, airlog:40.27
pub(crate) fn answer(content: &CnaContent, state: &mut State, pending: &Pending, action: &Value) -> Result<String, Rejection> {
    let expected = context(state).map_err(Rejection::Engine)?;
    let ops = &state.air.runtime.operations;
    let batch = ops.planning.batch.ok_or_else(|| illegal("no operational window"))?;
    let owner = pending.seat.side;
    if ops.period != Some(expected.period) || pending.seat.role != Role::Air || pending.kind != batch_kind(batch) || batch == Batch::Closed {
        return Err(illegal("this operational window is not current"));
    }
    space(content, state, owner, batch).check(action).map_err(illegal)?;
    match batch {
        Batch::Assignment => {
            if ops.planning.assignments.contains_key(&owner) { return Err(illegal("already answered assignment")); }
            let wire: WireAssignment = if action.is_null() { WireAssignment { routes: None, caps: None } }
                else { serde_json::from_value(action.clone()).map_err(|_| illegal("invalid route/CAP assignment"))? };
            let plan = Assignment { routes: wire.routes.unwrap_or_default(), caps: wire.caps.unwrap_or_default() };
            validate_assignment(content, state, owner, &plan).map_err(Rejection::Engine)?;
            state.air.runtime.operations.planning.assignments.insert(owner, plan);
        }
        Batch::Query => {
            if ops.planning.queries.contains_key(&owner) { return Err(illegal("already answered query")); }
            let indices: Vec<String> = if action.is_null() { Vec::new() } else { serde_json::from_value(action.clone()).map_err(|_| illegal("invalid contact list"))? };
            let contacts: Vec<_> = ops.contacts.iter().filter(|c| c.owner != owner).collect();
            let mut requests = BTreeSet::new();
            for id in indices {
                let n: usize = id.parse().map_err(|_| illegal("invalid public contact index"))?;
                if n.to_string() != id { return Err(illegal("noncanonical contact index")); }
                let c = contacts.get(n).ok_or_else(|| illegal("not a public enemy contact"))?;
                if !requests.insert((*c).clone()) { return Err(illegal("duplicate public contact query")); }
            }
            state.air.runtime.operations.planning.queries.insert(owner, requests.into_iter().collect());
        }
        Batch::Commit => {
            if ops.planning.commitments.contains_key(&owner) { return Err(illegal("already answered commitment")); }
            let plan: Vec<InterceptOrder> = if action.is_null() { Vec::new() } else { serde_json::from_value(action.clone()).map_err(|_| illegal("invalid interception commitments"))? };
            validate_intercepts(content, state, owner, &plan).map_err(Rejection::Engine)?;
            state.air.runtime.operations.planning.commitments.insert(owner, plan);
        }
        Batch::Closed => return Err(illegal("operational choices are closed")),
    }
    Ok("Recorded own operational choices; no paths disclosed, aircraft moved, dice rolled or combat resolved.".into())
}

/// Close BOTH answers on a disposable whole-State/RNG/events draft. Every
/// query and commitment window opens for both sides, including an empty side.
/// Cases: airlog:39.19, airlog:40.21, airlog:40.27
pub(super) fn finish(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    finish_transaction(content, state, cx, |_| {})
}

fn finish_transaction(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>, after_commit: impl FnOnce(&mut State)) -> Result<(), EngineError> {
    let expected = context(state)?;
    inventory::check_at(content, &state.air, Some(expected))?;
    if !state.decisions.pending.is_empty() { return Err(invalid("operational finish still has pending decisions")); }
    let before = &state.air.runtime.operations.planning;
    let batch = before.batch.ok_or_else(|| invalid("missing operational batch"))?;
    let answered = |owner: &Side| match batch { Batch::Assignment => before.assignments.contains_key(owner),
        Batch::Query => before.queries.contains_key(owner), Batch::Commit => before.commitments.contains_key(owner), Batch::Closed => true };
    if batch == Batch::Closed { return Ok(()); }
    if Side::ALL.iter().any(|s| !answered(s)) {
        return Err(invalid("closed fixed window lacks an explicit owner answer"));
    }
    let mut draft = state.clone();
    let mut events = Vec::new();
    let original_rng = cx.rng.state();
    let mut rng = CampaignRng::from_state(&original_rng);
    let next = match batch {
        Batch::Assignment => {
            for owner in Side::ALL { validate_assignment(content, state, owner, &before.assignments[&owner])?; }
            let mut caps = Vec::new();
            let mut routes = BTreeMap::new();
            for owner in Side::ALL {
                let plan = &before.assignments[&owner];
                let mut selected = plan.routes.clone();
                for (index, cap) in plan.caps.iter().enumerate() {
                    let id = MissionId(format!("{owner}-gt{}-op{}-cap-{index}", expected.period.game_turn, expected.period.op_stage));
                    if draft.air.runtime.tactical.declarations.contains_key(&id) { return Err(invalid("CAP declaration identity collision")); }
                    draft.air.runtime.tactical.declarations.insert(id.clone(), MissionDeclaration { mission: CAP.into(), light: missions::Light::Day,
                        target: DeclarationTarget::Hex(cap.target.clone()), planes: vec![PlaneDeclaration { plane: cap.plane.clone(), mode: cap.mode, rated_pilot: cap.rated_pilot.clone(), initial_choice: None }] });
                    selected.push(RouteSelection { mission: id, path: cap.path.clone() });
                    caps.push(AssignedCap { owner, plane: cap.plane.clone(), hex: cap.target.clone(), posture: cap.posture, neighbors: cap.neighbors.clone() });
                }
                routes.insert(owner, selected);
            }
            // The shared source producer checks every actual launch before
            // enemy interference. Bind the OUTER before record, including the
            // original declarations, to this single final composed admission.
            let launch = prepare_deployment(content, &draft, &routes)?;
            let mut after = launch.map_or_else(|| draft.air.runtime.clone(), |token| token.after);
            after.operations.routes = routes;
            after.operations.planning = PlanningState { batch: Some(Batch::Query), caps, ..Default::default() };
            draft = state.clone();
            apply(content, &mut draft, after, expected)?;
            Some(Batch::Query)
        }
        Batch::Query => {
            let mut after = state.air.runtime.clone();
            for owner in Side::ALL {
                let mut disclosures = Vec::new();
                for contact in &before.queries[&owner] {
                    if contact.owner == owner || !state.air.runtime.operations.contacts.contains(contact) { return Err(invalid("query is not a public enemy contact")); }
                    if available(state, owner).is_empty() { return Err(invalid("path query requires actual own offensive CAP assignment")); }
                    let paths = state.air.runtime.operations.flights.iter().filter(|f| f.owner == contact.owner && f.target == contact.target && f.classes.contains(&contact.class))
                        .map(|f| f.path.clone()).collect::<BTreeSet<_>>().into_iter().collect();
                    disclosures.push(Disclosure { contact: contact.clone(), paths });
                }
                events.push(EngineEvent::new(Audience::Side(owner), GameEvent::Note { text: format!("Authorized CAP paths: {}", serde_json::to_string(&disclosures).map_err(|_| invalid("path disclosure encoding failed"))?) }));
                after.operations.planning.disclosures.insert(owner, disclosures);
            }
            after.operations.planning.batch = Some(Batch::Commit);
            apply(content, &mut draft, after, expected)?;
            Some(Batch::Commit)
        }
        Batch::Commit => {
            let mut interceptions = Vec::new();
            for owner in Side::ALL { interceptions.extend(validate_intercepts(content, state, owner, &before.commitments[&owner])?); }
            interceptions.sort_by_key(|i| (i.enemy, i.target.clone(), i.path.clone(), i.path.iter().position(|h| h == &i.hex), i.owner));
            let mut after = state.air.runtime.clone();
            after.operations.planning.used.extend(interceptions.iter().flat_map(|i| i.fighters.iter().cloned()));
            after.operations.planning.interceptions = interceptions;
            after.operations.planning.batch = Some(Batch::Closed);
            apply(content, &mut draft, after, expected)?;
            None
        }
        Batch::Closed => None,
    };
    after_commit(&mut draft);
    inventory::check_at(content, &draft.air, Some(expected))?;
    if let Some(next) = next { open_batch(content, &mut draft, &mut Cx { rng: &mut rng, events: &mut events }, next); }
    if rng.state() != original_rng { return Err(invalid("operational planning must not consume RNG")); }
    *state = draft;
    cx.events.extend(events);
    Ok(())
}

/// Snapshot and checkpoint invariants; response legality does not inspect this
/// enemy-dependent proof. Generic callbacks already forbid any planning edit.
/// Cases: airlog:40.21, airlog:40.23, airlog:40.27
pub(super) fn check(content: &CnaContent, air: &AirState, expected: Context) -> Result<(), EngineError> {
    let p = &air.runtime.operations.planning;
    if p == &PlanningState::default() { return Ok(()); }
    if air.runtime.operations.period != Some(expected.period) || p.batch.is_none() { return Err(invalid("planning has no current canonical period/batch")); }
    if p.batch != Some(Batch::Closed) && expected.phase != Phase::Deployment {
        return Err(invalid("unfinished fixed planning window crossed the canonical phase"));
    }
    let mut assigned = BTreeSet::new();
    for cap in &p.caps {
        let order: BTreeSet<_> = cap.neighbors.iter().cloned().collect();
        if !assigned.insert(&cap.plane) || content.map.canonical(&cap.hex) != Some(&cap.hex)
            || order.len() != cap.neighbors.len() || match cap.posture { Posture::Offensive => order != neighbors(content, &cap.hex), Posture::Defensive => !order.is_empty() } {
            return Err(invalid("invalid CAP assignment snapshot"));
        }
        if let Some(plane) = air.runtime.aircraft.get(&cap.plane) {
            if side(&plane.force)? != cap.owner || !air.runtime.operations.flights.iter().any(|f| f.owner == cap.owner && f.target == cap.hex && f.members.iter().any(|m| m.plane == cap.plane)) {
                return Err(invalid("live CAP aircraft lacks its actual launched membership"));
            }
        }
    }
    let mut used = BTreeSet::new();
    for i in &p.interceptions {
        if i.enemy == i.owner || i.path.last() != Some(&i.target) || !i.path.contains(&i.hex) || i.fighters.is_empty() { return Err(invalid("invalid complete-flight interception")); }
        for id in &i.fighters {
            if !used.insert(id) || !p.caps.iter().any(|c| c.plane == *id && c.owner == i.owner && c.posture == Posture::Offensive) { return Err(invalid("interception fighter reused or not CAP-O")); }
        }
    }
    if used.into_iter().cloned().collect::<BTreeSet<_>>() != p.used { return Err(invalid("interception use differs from committed queue")); }
    Ok(())
}

#[cfg(test)]
mod tests;
