//! Tactical flight placement and deployment. Intermediate source slice: no
//! operational activation until its real combat/return consumers are present.
//! Historical launch sites are not physical positions or servicing authority.

use std::collections::{BTreeMap, BTreeSet};

use cna_core::{engine::EngineError, ids::HexId};
use cna_protocol::{Marker, Side};
use serde::{Deserialize, Serialize};

use super::{
    facilities::{FacilityId, FacilityKind, SiteIncarnation},
    missions::TacticalPeriod,
    state::{AirRuntime, PilotId, PlaneId},
};
use crate::{CnaContent, State, state::AirState};

pub(crate) mod planning;

pub const DEPLOYMENT: &str = "opstage.land_support_air.deployment";
// Uniform implementation capability, never a query of either hidden plan.
// Do not expose executable payloads until combat/return consumers are complete.
const EXECUTABLE_FLIGHTS: bool = false;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Phase {
    Deployment,
    Combat,
    Flak,
    Completion,
    Return,
}

/// Not serialized authority. Always obtained independently from State's cursor,
/// never by wrapping the period or phase from OperationalState.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    period: TacticalPeriod,
    phase: Phase,
}

impl Context {
    /// Cases: airlog:33.0, airlog:37.13
    pub fn from_state(state: &State) -> Result<Self, EngineError> {
        let phase = match state.cursor.anchor() {
            DEPLOYMENT => Phase::Deployment,
            "opstage.land_support_air.air_combat" => Phase::Combat,
            "opstage.land_support_air.flak" => Phase::Flak,
            "opstage.land_support_air.completion" => Phase::Completion,
            "opstage.land_support_air.return" => Phase::Return,
            _ => return Err(invalid("not a tactical flight phase")),
        };
        let op_stage = state
            .cursor
            .op_stage
            .filter(|s| (1..=3).contains(s))
            .ok_or_else(|| invalid("missing canonical Operations Stage"))?;
        if state.cursor.game_turn == 0 {
            return Err(invalid("zero Game-Turn"));
        }
        Ok(Self {
            period: TacticalPeriod {
                game_turn: state.cursor.game_turn,
                op_stage,
            },
            phase,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Fighter,
    Bomber,
    Transport,
}

impl Class {
    fn name(self) -> &'static str {
        match self {
            Self::Fighter => "fighter",
            Self::Bomber => "bomber",
            Self::Transport => "transport",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LaunchSite {
    pub facility: FacilityId,
    pub incarnation: Option<SiteIncarnation>,
    pub hex: HexId,
    pub kind: FacilityKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Member {
    pub plane: PlaneId,
    pub mode: usize,
    pub pilot: Option<PilotId>,
    pub origin: LaunchSite,
}

/// One side's complete cross-class flight at one target along one exact path.
/// Cases: airlog:39.16, airlog:39.17, airlog:40.27
/// Interpretations: interp:air-0021
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Flight {
    pub owner: Side,
    pub target: HexId,
    pub path: Vec<HexId>,
    pub members: Vec<Member>,
    // Immutable deployment classes: casualties never change a query answer.
    #[serde(default)]
    pub classes: BTreeSet<Class>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Contact {
    pub owner: Side,
    pub class: Class,
    pub target: HexId,
}

/// Deployment snapshot survives private casualties until scheduled Return.
/// No persistent plane/mission identity is included in the public key.
/// Cases: airlog:34.73, airlog:34.74
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OperationalState {
    pub(crate) period: Option<TacticalPeriod>,
    pub(crate) phase: Option<Phase>,
    pub(crate) flights: Vec<Flight>,
    pub(crate) contacts: BTreeSet<Contact>,
    pub(crate) routes: BTreeMap<Side, Vec<RouteSelection>>,
    pub(crate) planning: planning::PlanningState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RouteSelection {
    pub mission: super::missions::MissionId,
    pub path: Vec<HexId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "at", content = "value", rename_all = "snake_case")]
pub enum Position {
    Ground(FacilityId),
    Airborne { target: HexId },
    UnplacedReserve,
}

fn invalid(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("air operations: {detail}"),
    }
}

fn side(force: &str) -> Result<Side, EngineError> {
    match force {
        "axis" => Ok(Side::Axis),
        "commonwealth" | "malta" => Ok(Side::Commonwealth),
        _ => Err(invalid("unknown aircraft owner")),
    }
}

/// Validate an active snapshot against independent expected cursor context.
/// Later cursor phases may retain an earlier closed phase, never vice versa;
/// an old period cannot become current by agreeing with its own stored fields.
/// Cases: airlog:33.0, airlog:37.13, airlog:39.19
pub(crate) fn check(
    content: &CnaContent,
    air: &AirState,
    expected: Option<Context>,
) -> Result<(), EngineError> {
    let ops = &air.runtime.operations;
    if ops == &OperationalState::default() {
        return Ok(());
    }
    let expected =
        expected.ok_or_else(|| invalid("active operations need canonical State context"))?;
    if ops.period != Some(expected.period) || ops.phase.is_none_or(|p| p > expected.phase) {
        return Err(invalid(
            "operation period/phase differs from canonical cursor",
        ));
    }
    planning::check(content, air, expected)?;
    let mut ids = BTreeSet::new();
    let mut flight_keys = BTreeSet::new();
    for flight in &ops.flights {
        if content.map.canonical(&flight.target) != Some(&flight.target)
            || flight.path.is_empty()
            || flight.path.last() != Some(&flight.target)
            || flight
                .path
                .iter()
                .any(|h| content.map.canonical(h) != Some(h))
            || !flight_keys.insert((flight.owner, flight.target.clone(), flight.path.clone()))
        {
            return Err(invalid("invalid or duplicate canonical flight"));
        }
        if flight.classes.is_empty() {
            return Err(unsupported("scen:59.32", "active saved flight lacks its immutable deployment classes"));
        }
        if flight.classes.iter().any(|class| !ops.contacts.contains(&Contact { owner: flight.owner, class: *class, target: flight.target.clone() })) {
            return Err(invalid("flight classes lack their immutable deployment contacts"));
        }
        for member in &flight.members {
            let plane = air
                .runtime
                .aircraft
                .get(&member.plane)
                .ok_or_else(|| invalid("airborne member has no live aircraft"))?;
            if !flight.classes.contains(&class(&content.units.aircraft.get(&plane.aircraft).ok_or_else(|| invalid("missing airborne aircraft source"))?.role)?)
                || !ids.insert(&member.plane)
                || side(&plane.force)? != flight.owner
                || plane.facility.as_deref() != Some(member.origin.facility.0.as_str())
                || content.map.canonical(&member.origin.hex) != Some(&member.origin.hex)
                || flight.path.first() != Some(&member.origin.hex)
                || super::preparation::known_fuel(
                    air.runtime
                        .preparation
                        .current
                        .get(&member.plane)
                        .ok_or_else(|| invalid("airborne preparation missing"))?,
                ) != Some((member.mode, false))
                || member.origin.incarnation.is_some_and(|i| {
                    i.serial() == 0 || i.serial() > air.runtime.site_incarnation_serial
                })
                || !content
                    .units
                    .aircraft
                    .get(&plane.aircraft)
                    .is_some_and(|a| member.mode < a.modes.len())
            {
                return Err(invalid("invalid airborne identity or historical origin"));
            }
            if let Some(pilot) = &member.pilot {
                let p = air
                    .runtime
                    .pilots
                    .get(pilot)
                    .ok_or_else(|| invalid("missing airborne pilot"))?;
                if side(&p.force)? != flight.owner {
                    return Err(invalid("foreign airborne pilot"));
                }
            }
        }
    }
    for contact in &ops.contacts {
        if content.map.canonical(&contact.target) != Some(&contact.target) {
            return Err(invalid("contact is not at a canonical target"));
        }
    }
    Ok(())
}

/// Exact position; unknown IDs are errors, never an unplaced reserve.
/// Cases: airlog:37.0, airlog:37.13, airlog:42.11
pub fn position(
    content: &CnaContent,
    state: &State,
    id: &PlaneId,
) -> Result<Position, EngineError> {
    position_at(content, &state.air, Context::from_state(state).ok(), id)
}

pub(crate) fn position_at(
    content: &CnaContent,
    air: &AirState,
    expected: Option<Context>,
    id: &PlaneId,
) -> Result<Position, EngineError> {
    check(content, air, expected)?;
    let plane = air
        .runtime
        .aircraft
        .get(id)
        .ok_or_else(|| invalid("unknown aircraft ID"))?;
    if let Some(flight) = air
        .runtime
        .operations
        .flights
        .iter()
        .find(|f| f.members.iter().any(|m| &m.plane == id))
    {
        return Ok(Position::Airborne {
            target: flight.target.clone(),
        });
    }
    let Some(id) = &plane.facility else {
        return Ok(Position::UnplacedReserve);
    };
    let id = FacilityId(id.clone());
    let site = air
        .runtime
        .facilities
        .get(&id)
        .ok_or_else(|| invalid("ground facility is not canonical"))?;
    let properties = site.properties(content, &id)?;
    if site.removed(&properties) {
        return Err(invalid("ground aircraft is at a removed facility"));
    }
    Ok(Position::Ground(id))
}

pub(crate) fn airborne_ids(
    content: &CnaContent,
    air: &AirState,
    expected: Option<Context>,
) -> Result<BTreeSet<PlaneId>, EngineError> {
    check(content, air, expected)?;
    Ok(air
        .runtime
        .operations
        .flights
        .iter()
        .flat_map(|f| f.members.iter().map(|m| m.plane.clone()))
        .collect())
}

pub(crate) fn check_unchanged_members(
    before: &AirRuntime,
    after: &AirRuntime,
) -> Result<(), EngineError> {
    if before.operations != after.operations {
        return Err(invalid("operation changes require exact flight admission"));
    }
    for member in before.operations.flights.iter().flat_map(|f| &f.members) {
        if before.aircraft.get(&member.plane) != after.aircraft.get(&member.plane)
            || before.preparation.current.get(&member.plane)
                != after.preparation.current.get(&member.plane)
            || member
                .pilot
                .as_ref()
                .is_some_and(|p| before.pilots.get(p) != after.pilots.get(p))
        {
            return Err(invalid(
                "ordinary callback cannot edit an airborne participant",
            ));
        }
    }
    Ok(())
}

/// None is ineligible for all ground service, not permission to choose a site.
/// Cases: airlog:38.24, airlog:38.33, airlog:38.43
pub fn ground_facility(
    content: &CnaContent,
    state: &State,
    id: &PlaneId,
) -> Result<Option<FacilityId>, EngineError> {
    Ok(match position(content, state, id)? {
        Position::Ground(id) => Some(id),
        _ => None,
    })
}

/// Pure immutable contact projection. Casualty and private member maps are not
/// read. The existing all-perspective marker comparison remains authoritative.
/// Cases: airlog:34.73, airlog:34.74
/// Interpretations: interp:air-0021
pub fn markers(state: &State) -> Vec<Marker> {
    let ops = &state.air.runtime.operations;
    let Some(period) = ops.period else {
        return Vec::new();
    };
    if Context::from_state(state).is_err()
        || state.cursor.game_turn != period.game_turn
        || state.cursor.op_stage != Some(period.op_stage)
    {
        return Vec::new();
    }
    ops.contacts
        .iter()
        .map(|c| {
            let class = c.class.name();
            Marker {
                id: format!(
                    "air:{}:{}:{}:{class}:{}",
                    period.game_turn, period.op_stage, c.owner, c.target
                ),
                kind: format!("air_{class}"),
                hex: c.target.to_string(),
                side: Some(c.owner),
                label: Some(class.into()),
            }
        })
        .collect()
}

fn unsupported(case: &str, detail: &str) -> EngineError {
    EngineError::Unsupported {
        case: case.into(),
        detail: detail.into(),
    }
}

fn ready_limit(
    content: &CnaContent,
    state: &State,
    nationality: &str,
) -> Result<usize, EngineError> {
    use cna_tables::airlog::air::SquadronKind;
    let date = crate::view::wire_clock(content, state).date;
    let kind = match nationality {
        "it" => SquadronKind::ItalianSquadriglia,
        "ge" => SquadronKind::GermanStaffel,
        "cw" if date.len() == 10 && date.as_str() < "1941-07-01" => {
            SquadronKind::CommonwealthSquadron194041
        }
        "cw" if date.len() == 10 => SquadronKind::CommonwealthSquadron194243,
        _ => {
            return Err(unsupported(
                "airlog:35.23",
                "flight ready capacity has no source row/date",
            ));
        }
    };
    usize::try_from(content.tables.airlog.squadron_capacity.capacity(kind).ready)
        .map_err(|_| invalid("negative source ready capacity"))
}

fn class(role: &str) -> Result<Class, EngineError> {
    match role {
        "fighter" => Ok(Class::Fighter),
        "bomber" => Ok(Class::Bomber),
        "transport" => Ok(Class::Transport),
        _ => Err(unsupported(
            "airlog:34.73",
            "specialized aircraft class is outside the intermediate deployment producer",
        )),
    }
}

/// Actual closed deployment producer. Both owners and every source prerequisite
/// are checked before a token exists. This intermediate component deliberately
/// does not implement query/combat/return or enable executable public choices.
/// Cases: airlog:34.73, airlog:35.23, airlog:37.11, airlog:37.12, airlog:37.13,
/// airlog:37.15, airlog:37.16, airlog:37.17, airlog:37.22, airlog:37.24, airlog:38.31
fn prepare_deployment(
    content: &CnaContent,
    state: &State,
    routes: &BTreeMap<Side, Vec<RouteSelection>>,
) -> Result<Option<FlightAdmission>, EngineError> {
    use super::{
        inventory,
        missions::{DeclarationTarget, Light},
        preparation::{KnownAmmunition, OrdnanceKnowledge},
        sgsu::{SgsuId, SgsuPosition},
    };
    use crate::state::Location;
    use cna_tables::land::weather::WeatherKind;
    let expected = Context::from_state(state)?;
    if expected.phase != Phase::Deployment {
        return Err(invalid("not a deployment finish"));
    }
    inventory::check_at(content, &state.air, Some(expected))?;
    let tactical = &state.air.runtime.tactical;
    if tactical.period != Some(expected.period)
        || !tactical.resolved
        || Side::ALL.iter().any(|side| !routes.contains_key(side))
        || !state.air.runtime.operations.flights.is_empty()
        || !state.air.runtime.operations.contacts.is_empty()
    {
        return Err(invalid(
            "deployment is incomplete, stale or already committed",
        ));
    }
    let mut after = state.air.runtime.clone();
    after.operations = OperationalState {
        period: Some(expected.period),
        phase: Some(Phase::Deployment),
        ..OperationalState::default()
    };
    let mut used_missions = BTreeSet::new();
    let mut used_planes = BTreeSet::new();
    let mut used_pilots = BTreeSet::new();
    let mut active = BTreeMap::<FacilityId, BTreeSet<SgsuId>>::new();
    let mut launched = BTreeMap::<SgsuId, usize>::new();
    for owner in Side::ALL {
        for route in &routes[&owner] {
            if !used_missions.insert(&route.mission) {
                return Err(invalid("duplicate mission route"));
            }
            let mission = tactical
                .declarations
                .get(&route.mission)
                .ok_or_else(|| invalid("route has no committed declaration"))?;
            super::missions::validate_operational(content, state, owner, std::slice::from_ref(mission))?;
            let DeclarationTarget::Hex(target) = &mission.target else {
                return Err(unsupported(
                    "airlog:40.33",
                    "scramble is a later actual producer",
                ));
            };
            if mission.light != Light::Day {
                return Err(unsupported(
                    "airlog:41.47",
                    "night search is a later actual producer",
                ));
            }
            if route.path.is_empty()
                || route.path.last() != Some(target)
                || route
                    .path
                    .iter()
                    .any(|h| content.map.canonical(h) != Some(h))
                || route.path.windows(2).any(|pair| {
                    !content
                        .map
                        .neighbors(&pair[0])
                        .iter()
                        .any(|n| n.id == pair[1])
                })
            {
                return Err(invalid(
                    "route is not a nonempty canonical adjacent flight path",
                ));
            }
            let distance =
                i32::try_from(route.path.len() - 1).map_err(|_| invalid("flight path overflow"))?;
            for hex in &route.path {
                if matches!(
                    crate::logistics::weather::at_hex(content, state, hex)?,
                    WeatherKind::Sandstorm | WeatherKind::Rainstorm
                ) {
                    return Err(invalid("flight enters or leaves a storm hex"));
                }
            }
            let mut members = Vec::new();
            for selected in &mission.planes {
                if !used_planes.insert(&selected.plane) {
                    return Err(invalid("plane launched twice"));
                }
                let plane = &state.air.runtime.aircraft[&selected.plane];
                let site_id = ground_facility(content, state, &selected.plane)?
                    .ok_or_else(|| invalid("launch plane is not grounded"))?;
                let site = &state.air.runtime.facilities[&site_id];
                let properties = site.properties(content, &site_id)?;
                let Location::Hex { hex } = properties.location else {
                    return Err(unsupported(
                        "airlog:36.5",
                        "off-map flight is outside the intermediate producer",
                    ));
                };
                if !site.operational() || route.path.first() != Some(&hex) {
                    return Err(invalid("route does not leave an operational actual base"));
                }
                let aircraft = &content.units.aircraft[&plane.aircraft];
                if !properties.kind.accepts(aircraft.role == "flying_boat") {
                    return Err(invalid(
                        "launch aircraft is incompatible with the actual facility",
                    ));
                }
                let mode = &aircraft.modes[selected.mode];
                if mode.rng_is_transfer_range {
                    return Err(unsupported(
                        "airlog:42.13",
                        "printed transfer-only range needs the transfer producer",
                    ));
                }
                let range = mode.range_hexes.ok_or_else(|| {
                    unsupported("airlog:34.11", "selected mode has no source range")
                })?;
                if range < distance || !plane.refitted {
                    return Err(invalid("flight exceeds source range or lacks refit"));
                }
                let mut knowledge = state
                    .air
                    .runtime
                    .preparation
                    .current
                    .get(&selected.plane)
                    .and_then(KnownAmmunition::from_current)
                    .ok_or_else(|| unsupported("airlog:38.2", "flight load is not known"))?;
                if knowledge.mode != Some(selected.mode)
                    || knowledge.fuel_available != Some(true)
                    || knowledge.gun_ammunition.is_none()
                    || matches!(knowledge.ordnance, OrdnanceKnowledge::Unknown)
                {
                    return Err(unsupported(
                        "airlog:38.4",
                        "flight needs exact own mode fuel gun and ordnance knowledge",
                    ));
                }
                if let Some(pilot_id) = &selected.rated_pilot {
                    if !used_pilots.insert(pilot_id) {
                        return Err(invalid("pilot launched twice"));
                    }
                    let pilot = &state.air.runtime.pilots[pilot_id];
                    if pilot.squadron != plane.squadron
                        || pilot.trained_aircraft.as_deref() != Some(plane.aircraft.as_str())
                    {
                        return Err(invalid(
                            "rated launch pilot is not assigned and trained for this aircraft",
                        ));
                    }
                }
                let squadron = plane
                    .squadron
                    .as_ref()
                    .ok_or_else(|| invalid("launch plane is not assigned"))?;
                let sgsu_id = SgsuId(squadron.clone());
                let sgsu = state
                    .air
                    .runtime
                    .sgsus
                    .get(&sgsu_id)
                    .ok_or_else(|| unsupported("airlog:35.11", "no actual launch SGSU"))?;
                if sgsu.side()? != owner || sgsu.position != SgsuPosition::Facility(site_id.clone())
                {
                    return Err(invalid("actual launch SGSU is not at the plane's base"));
                }
                let count = launched.entry(sgsu_id.clone()).or_default();
                *count += 1;
                if *count > ready_limit(content, state, &sgsu.nationality)? {
                    return Err(invalid("flight exceeds source SGSU ready capacity"));
                }
                let assigned = active.entry(site_id.clone()).or_default();
                assigned.insert(sgsu_id);
                if let super::facilities::FacilityCapacity::Levels(levels) = site.current_capacity
                    && usize::try_from(levels)
                        .ok()
                        .is_none_or(|levels| assigned.len() > levels)
                {
                    return Err(invalid("launch SGSU union exceeds facility levels"));
                }
                let class = class(&aircraft.role)?;
                after.operations.contacts.insert(Contact {
                    owner,
                    class,
                    target: target.clone(),
                });
                members.push(Member {
                    plane: selected.plane.clone(),
                    mode: selected.mode,
                    pilot: selected.rated_pilot.clone(),
                    origin: LaunchSite {
                        facility: site_id,
                        incarnation: site.incarnation,
                        hex,
                        kind: properties.kind,
                    },
                });
                // Launch spends fuel/refit, never paid supply or initial credit.
                // Payload is retained for actual combat/jettison/completion.
                knowledge.fuel_available = Some(false);
                after
                    .preparation
                    .current
                    .insert(selected.plane.clone(), knowledge.into_current());
                let plane = after
                    .aircraft
                    .get_mut(&selected.plane)
                    .expect("validated live aircraft");
                plane.fuelled = false;
                plane.refitted = false;
            }
            if let Some(flight) = after
                .operations
                .flights
                .iter_mut()
                .find(|f| f.owner == owner && f.target == *target && f.path == route.path)
            {
                flight.classes.extend(members.iter().map(|m| class(&content.units.aircraft[&state.air.runtime.aircraft[&m.plane].aircraft].role)).collect::<Result<BTreeSet<_>, _>>()?);
                flight.members.extend(members);
            } else {
                after.operations.flights.push(Flight {
                    owner,
                    target: target.clone(),
                    path: route.path.clone(),
                    classes: members.iter().map(|m| class(&content.units.aircraft[&state.air.runtime.aircraft[&m.plane].aircraft].role)).collect::<Result<_, _>>()?,
                    members,
                });
            }
        }
    }
    if tactical
        .declarations
        .iter()
        .any(|(id, m)| matches!(m.target, DeclarationTarget::Hex(_)) && !used_missions.contains(id))
    {
        return Err(invalid("committed flight declaration lacks its exact path"));
    }
    // Both owners' source-known preparation/site/range checks precede actual
    // ground interference. This is ordinary launch, not emergency escape.
    // Cases: airlog:39.11, airlog:37.31
    for flight in &after.operations.flights {
        for member in &flight.members {
            let origin = &member.origin.hex;
            let major_city = content.map.get(origin).is_some_and(|h| {
                h.terrain.as_deref() == Some("major_city")
                    || h.flags.iter().any(|f| f == "major_city")
            });
            if !major_city
                && content.map.neighbors(origin).iter().any(|hex| {
                    state.land.units.values().any(|unit| {
                        unit.side != flight.owner
                            && unit.location.hex() == Some(&hex.id)
                            && crate::land::formation::combat_unit(content, &unit.id)
                    })
                })
            {
                return Err(invalid(
                    "ordinary launch is blocked by adjacent enemy combat unit",
                ));
            }
        }
    }
    if after.operations.flights.is_empty() {
        return Ok(None);
    }
    Ok(Some(FlightAdmission {
        before: state.air.runtime.clone(),
        after,
        expected,
    }))
}

/// Existing dispatcher entry: executable payloads are statically unavailable in
/// both profiles until the real downstream consumers are complete. Do not stop
/// or mutate according to either side's hidden intention/nonempty-plan status.
/// Cases: airlog:37.13, airlog:39.12
pub fn finish(content: &CnaContent, state: &mut State) -> Result<(), EngineError> {
    if !EXECUTABLE_FLIGHTS {
        return Ok(());
    }
    finish_deployment(content, state)
}

/// Static prelaunch boundary: both profiles retain their old public flow.
/// Cases: airlog:39.12, airlog:40.27
pub fn enter(content: &CnaContent, state: &mut State, cx: &mut cna_core::engine::Cx<'_>) -> Result<(), EngineError> {
    if !EXECUTABLE_FLIGHTS { return Ok(()); }
    planning::enter_assignment(content, state, cx)
}

/// Fixed internal route/query/commit progression; never hidden-conditioned.
/// Cases: airlog:40.27
pub fn finish_step(content: &CnaContent, state: &mut State, cx: &mut cna_core::engine::Cx<'_>) -> Result<(), EngineError> {
    if !EXECUTABLE_FLIGHTS { return Ok(()); }
    planning::finish(content, state, cx)
}

fn finish_deployment(content: &CnaContent, state: &mut State) -> Result<(), EngineError> {
    let routes = state.air.runtime.operations.routes.clone();
    finish_transaction(content, state, &routes, |_| {})
}

fn finish_transaction(
    content: &CnaContent,
    state: &mut State,
    routes: &BTreeMap<Side, Vec<RouteSelection>>,
    after_launch: impl FnOnce(&mut State),
) -> Result<(), EngineError> {
    let admission = prepare_deployment(content, state, routes)?;
    let mut draft = state.clone();
    if let Some(admission) = admission {
        super::inventory::update_flight(content, &mut draft, admission)?;
    }
    after_launch(&mut draft);
    super::inventory::check_at(content, &draft.air, Some(Context::from_state(&draft)?))?;
    *state = draft;
    Ok(())
}

#[cfg(test)]
mod tests;

/// Exact nonserializable admission; only an actual closed flight finish may
/// construct it. Neither callers nor generic inventory callbacks edit its delta.
pub(in crate::air) struct FlightAdmission {
    before: AirRuntime,
    after: AirRuntime,
    expected: Context,
}

impl FlightAdmission {
    pub(in crate::air) fn context(&self) -> Context {
        self.expected
    }
    pub(in crate::air) fn apply(&self, runtime: &mut AirRuntime) -> Result<(), EngineError> {
        if runtime != &self.before {
            return Err(invalid("stale flight admission"));
        }
        *runtime = self.after.clone();
        Ok(())
    }
    pub(in crate::air) fn permits(&self, before: &AirRuntime, after: &AirRuntime) -> bool {
        before == &self.before && after == &self.after
    }
}
