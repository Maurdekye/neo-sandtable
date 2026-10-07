//! Secret Axis naval planning, including scenario pre-game plans and timed unloading.
//! Replacement production is a separate subsystem. Its cargo must reserve capacity before
//! supply choices (land:20.64); scheduled reinforcements use neither this cargo nor this budget.
//! Reconnaissance/bombing procedures may reveal and reduce individual convoy cargo later.
use super::{
    SupplyError, ports,
    stores::{field, option},
};
use crate::{
    CnaContent,
    state::{Dump, DumpLocation, Pending, State},
    steps::{illegal, open},
};
use cna_content::scenario::Supplies;
use cna_core::{
    decision::{ActionSchema, ActionSpace, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::SeatId,
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role, Side};
use cna_tables::{airlog::convoys::ConvoyLevel, calendar::Month};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
fn engine(error: impl Into<ports::PortOperationError>) -> EngineError {
    error.into().into_engine()
}
pub const PREFIX: &str = "cna.logistics.convoy.plan:";

/// Cargo remains private until Air rules reveal it; arriving supply stock is also private.
/// Cases: airlog:56.12, airlog:56.15, airlog:56.16, airlog:56.25, land:3.6
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NavalConvoy {
    pub lane: u8,
    pub arrival_opstage: u8,
    pub cargo: Supplies,
    pub status: ConvoyStatus,
    #[serde(default)]
    pub delivered: Option<Supplies>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConvoyStatus {
    Planned,
    Cancelled,
    Arrived,
    /// Terminal dev diagnostic: cargo never entered play and cannot retry.
    Unassessed,
}
/// Capacity dice are rolled once for this arrival turn. Production will fill replacement_tons
/// before the supply choice; zero is deliberate until that subsystem exists.
/// Cases: airlog:56.21, airlog:56.24, land:20.64
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConvoyTurn {
    pub level: ConvoyLevel,
    pub capacity_tons: i32,
    pub replacement_tons: i32,
    pub planning_complete: bool,
    pub convoys: BTreeMap<u8, NavalConvoy>,
}
fn date(content: &CnaContent, state: &State, gt: u16) -> Result<String, SupplyError> {
    let mut clock = state.clone();
    clock.cursor.game_turn = gt;
    let date = crate::view::wire_clock(content, &clock).date;
    if date.len() != 10 {
        return Err(SupplyError::Unsupported {
            case: "airlog:56.21",
        });
    }
    Ok(date)
}
fn level(content: &CnaContent, state: &State, gt: u16) -> Result<ConvoyLevel, SupplyError> {
    let d = date(content, state, gt)?;
    let y = d[..4].parse().map_err(|_| SupplyError::Invalid)?;
    let m = Month::from_number(d[5..7].parse().map_err(|_| SupplyError::Invalid)?)
        .ok_or(SupplyError::Invalid)?;
    content
        .tables
        .airlog
        .convoy_level
        .level(y, m)
        .ok_or(SupplyError::Unsupported {
            case: "airlog:56.4",
        })
}
/// Derive all remaining start-month turns from the campaign calendar and scenario bounds.
/// Cases: scen:60.37
pub fn pre_game_turns(content: &CnaContent, state: &State) -> Result<Vec<u16>, SupplyError> {
    let start = content.bounds.start_gt;
    let first = date(content, state, start)?;
    let mut turns = vec![];
    for gt in start..=content.bounds.end_gt {
        if date(content, state, gt)?[..7] != first[..7] {
            break;
        }
        turns.push(gt);
    }
    Ok(turns)
}
fn queue(
    content: &CnaContent,
    state: &mut State,
    turns: Vec<u16>,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    state.logistics.convoy_planning_queue = turns
        .into_iter()
        .filter(|gt| {
            !state
                .logistics
                .convoy_turns
                .get(gt)
                .is_some_and(|t| t.planning_complete)
        })
        .collect();
    next(content, state, strict, cx)
}
/// Called exactly once when placement closes, before GT1 can leave setup. Idempotent state
/// prevents rerolling capacities or reopening completed plans after checkpoint recovery.
/// Cases: scen:60.37, airlog:56.21, airlog:56.24, land:20.63
pub fn initialize(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    ports::preflight(content, strict)?;
    if state.logistics.convoys_initialized {
        return Ok(());
    }
    super::dump_markers::initialize(state, cx)?;
    ports::initialize(content, state);
    super::coastal::initialize(content, state).map_err(engine)?;
    let Some(setup) = &content.scenario.fleet_logistics.axis_convoys else {
        state.logistics.convoys_initialized = true;
        return Ok(());
    };
    // Every currently typed Axis convoy scenario includes a replacement pool. A future typed
    // production contract must supersede this seam rather than silently cargoing scheduled units.
    if strict {
        return Err(EngineError::Unsupported{case:"land:20.63".into(),detail:"Axis replacement production is not implemented; supply planning cannot allocate its priority cargo yet".into()});
    }
    cx.emit(EngineEvent::new(Audience::Side(Side::Axis),GameEvent::Note{text:"Replacement production is not implemented (land:20.63); these supply convoys reserve zero replacement tonnage.".into()}));
    let turns = if setup.pre_game_plan_remaining_start_month {
        pre_game_turns(content, state).map_err(engine)?
    } else {
        vec![]
    };
    state.logistics.convoys_initialized = true;
    queue(content, state, turns, strict, cx)
}
/// Normal plans concern next Game-Turn; scenario pre-game plans take precedence.
/// Cases: airlog:48.0, airlog:56.0, airlog:56.21, airlog:55.17
pub fn schedule(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    ports::preflight(content, strict)?;
    if !state.logistics.convoys_initialized
        && content
            .scenario
            .fleet_logistics
            .axis_convoys
            .as_ref()
            .is_some_and(|s| s.pre_game_plan_remaining_start_month)
    {
        return Err(EngineError::Unsupported {
            case: "scen:60.37".into(),
            detail: "initial convoy planning must finish during setup".into(),
        });
    }
    initialize(content, state, strict, cx)?;
    if state
        .decisions
        .pending
        .iter()
        .any(|p| p.kind.starts_with(PREFIX))
    {
        return Ok(());
    }
    let gt = state.cursor.game_turn;
    if !state.logistics.bizerta_open
        && state.logistics.bizerta_roll_gt != Some(gt)
        && date(content, state, gt).map_err(engine)?.as_str() >= "1941-06-01"
    {
        let dice = [cx.rng.d6().value(), cx.rng.d6().value()];
        state.logistics.bizerta_roll_gt = Some(gt);
        state.logistics.bizerta_open = dice[0] + dice[1] == 12;
        cx.emit(EngineEvent::new(
            Audience::Side(Side::Axis),
            GameEvent::DiceRolled {
                purpose: "Bizerta availability".into(),
                dice: dice.to_vec(),
                reading: None,
                rule: Some("airlog:55.17".into()),
            },
        ));
    }
    if content.scenario.fleet_logistics.axis_convoys.is_some() && gt < content.bounds.end_gt {
        queue(content, state, vec![gt + 1], strict, cx)?;
    }
    Ok(())
}
/// Enumerate only canonically assessed controlled destinations. A source gap
/// remains an error under full; dev excludes the affected lane with a private
/// diagnostic rather than accepting a legacy numeric efficiency.
fn lanes(
    content: &CnaContent,
    state: &State,
    gt: u16,
    strict: bool,
    events: &mut Vec<EngineEvent>,
) -> Result<Vec<u8>, EngineError> {
    ports::preflight(content, strict)?;
    let Some(setup) = &content.scenario.fleet_logistics.axis_convoys else {
        return Ok(vec![]);
    };
    let when = date(content, state, gt).map_err(engine)?;
    let mut allowed = vec![];
    for &lane in &setup.lanes_allowed {
        if matches!(lane, 4 | 5) && when.as_str() < "1941-05-08" {
            continue;
        }
        let port = match ports::lane_destination(content, lane) {
            Ok(port) => port,
            Err(error @ SupplyError::Unsupported { .. }) if !strict => {
                events.push(EngineEvent::new(
                    Audience::Side(Side::Axis),
                    GameEvent::Note {
                        text: format!("Convoy lane {lane} is unavailable: {error:?}"),
                    },
                ));
                continue;
            }
            Err(error) => return Err(engine(error)),
        };
        let condition = match ports::state(content, state, &port) {
            Ok(condition) => condition,
            // No accepted entry means there is no controlled endpoint to offer.
            Err(ports::PortOperationError::Supply(SupplyError::Unsupported {
                case: "airlog:55.11",
            })) => continue,
            Err(error) if !strict && error.is_unknown() => {
                let last_owner = state
                    .logistics
                    .unknown_ports
                    .get(&port.id)
                    .copied()
                    .or_else(|| state.logistics.ports.get(&port.id).map(|p| p.owner));
                if last_owner == Some(Side::Axis) {
                    let source = match error {
                        ports::PortOperationError::Policy(source) => source.to_string(),
                        ports::PortOperationError::Supply(source) => format!("{source:?}"),
                    };
                    let mut note = EngineEvent::new(
                        Audience::Side(Side::Axis),
                        GameEvent::Note {
                            text: format!(
                                "Convoy lane {lane} at port {} is unavailable: unknown efficiency. {source}",
                                port.id
                            ),
                        },
                    );
                    if let Some(hex) = port.location.hex() {
                        note = note.at(hex.clone());
                    }
                    events.push(note);
                }
                continue;
            }
            Err(error) => return Err(engine(error)),
        };
        if condition.owner == Side::Axis
            && condition.efficiency > 0
            && (port.name != cna_tables::airlog::trucks::PortName::Bizerta
                || state.logistics.bizerta_open)
        {
            // Validate the numeric condition as well as the source policy.
            ports::planning_capacity_tons(content, state, &port).map_err(engine)?;
            allowed.push(lane);
        }
    }
    Ok(allowed)
}
fn next(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    ports::preflight(content, strict)?;
    let Some(gt) = state.logistics.convoy_planning_queue.first().copied() else {
        return Ok(());
    };
    let mut menu_events = vec![];
    let allowed = lanes(content, state, gt, strict, &mut menu_events)?;
    if !state.logistics.convoy_turns.contains_key(&gt) {
        let level = level(content, state, gt).map_err(engine)?;
        let die = cx.rng.d6();
        let capacity = content
            .tables
            .airlog
            .convoy_capacity
            .capacity(level, die)
            .get();
        state.logistics.convoy_turns.insert(
            gt,
            ConvoyTurn {
                level,
                capacity_tons: capacity,
                replacement_tons: 0,
                planning_complete: false,
                convoys: BTreeMap::new(),
            },
        );
        cx.emit(EngineEvent::new(
            Audience::Side(Side::Axis),
            GameEvent::DiceRolled {
                purpose: format!("Axis convoy capacity for GT{gt} ({level:?}): {capacity} tons"),
                dice: vec![die.value()],
                reading: None,
                rule: Some("airlog:56.5".into()),
            },
        ));
    }
    let turn = &state.logistics.convoy_turns[&gt];
    let cap = i64::from(turn.capacity_tons - turn.replacement_tons);
    let options = allowed
        .into_iter()
        .map(|n| {
            option(
                n.to_string(),
                format!(
                    "Lane {n}: {}",
                    content.tables.airlog.convoy_air_distance.route(n).unwrap()
                ),
            )
        })
        .collect::<Vec<_>>();
    let max = options.len() as u32;
    let schema = ActionSchema::Record {
        fields: vec![field(
            "convoys",
            "At most one convoy per allowed lane",
            ActionSchema::List {
                min: 0,
                max,
                item: Box::new(ActionSchema::Record {
                    fields: vec![
                        field(
                            "lane",
                            "Fixed lane and destination",
                            ActionSchema::Choice { options },
                        ),
                        field(
                            "arrival_opstage",
                            "Arrival in this Game-Turn",
                            ActionSchema::Integer {
                                min: 1,
                                max: if gt == content.bounds.end_gt {
                                    content.bounds.end_opstage.into()
                                } else {
                                    3
                                },
                            },
                        ),
                        field(
                            "ammo",
                            "Ammunition points (4tons each)",
                            ActionSchema::Integer {
                                min: 0,
                                max: cap / 4,
                            },
                        ),
                        field(
                            "fuel",
                            "Fuel points (1/8ton each)",
                            ActionSchema::Integer {
                                min: 0,
                                max: cap * 8,
                            },
                        ),
                        field(
                            "stores",
                            "Stores points (1ton each)",
                            ActionSchema::Integer { min: 0, max: cap },
                        ),
                    ],
                }),
            },
        )],
    };
    cx.events.extend(menu_events);
    open(
        state,
        cx,
        SeatId::new(Side::Axis, Role::Logistics),
        &format!("{PREFIX}{gt}"),
        format!(
            "Plan secret Axis supply convoys for GT{gt}: {cap} tons after replacement reservation. Missing port anchors are not offered."
        ),
        &[
            "airlog:56.12",
            "airlog:56.21",
            "airlog:56.22",
            "airlog:56.24",
            "airlog:56.25",
            "scen:60.37",
            "land:3.6",
        ],
        Trigger::Scheduled,
        Secrecy::Secret,
        ActionSpace::new(schema).with_pass("Send no supply convoys this turn"),
    );
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    convoys: Vec<Shipment>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Shipment {
    lane: String,
    arrival_opstage: u8,
    ammo: i32,
    fuel: i32,
    stores: i32,
}
/// Validate the whole turn before replacing its plan. No rejection consumes dice or stock.
/// Each lane is immutable after this answer. Supplies from Europe have unlimited availability.
/// Cases: airlog:56.12, airlog:56.15, airlog:56.22, airlog:56.24, airlog:56.25, airlog:56.27
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    answer_with_profile(content, state, pending, action, false, cx)
}
/// The dispatcher supplies the actual profile, including when an accepted plan
/// opens the next pre-game turn. The compatibility helper above uses dev.
pub fn answer_with_profile(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    ports::preflight(content, strict).map_err(Rejection::Engine)?;
    if pending.seat != SeatId::new(Side::Axis, Role::Logistics) {
        return Err(illegal("Axis Logistics plans these convoys"));
    }
    let gt: u16 = pending
        .kind
        .strip_prefix(PREFIX)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| illegal("unknown convoy turn"))?;
    let old = state
        .logistics
        .convoy_turns
        .get(&gt)
        .ok_or_else(|| illegal("convoy capacity has not been rolled"))?;
    if old.planning_complete {
        return Err(illegal("convoy lanes and cargo are already fixed"));
    }
    let plan = if action.is_null() {
        Plan { convoys: vec![] }
    } else {
        serde_json::from_value(action.clone()).map_err(|_| illegal("invalid convoy plan"))?
    };
    let allowed = lanes(content, state, gt, strict, &mut vec![]).map_err(Rejection::Engine)?;
    let mut convoys = BTreeMap::new();
    let mut total24 = i64::from(old.replacement_tons) * 24;
    let mut by_stage = BTreeMap::<(String, u8), i64>::new();
    for ship in plan.convoys {
        let lane: u8 = ship.lane.parse().map_err(|_| illegal("unknown lane"))?;
        let last = if gt == content.bounds.end_gt {
            content.bounds.end_opstage
        } else {
            3
        };
        if !allowed.contains(&lane)
            || ship.arrival_opstage < 1
            || ship.arrival_opstage > last
            || convoys.contains_key(&lane)
        {
            return Err(illegal(
                "lane unavailable, duplicated, or arrival outside scenario",
            ));
        }
        let cargo = Supplies {
            ammo: ship.ammo,
            fuel: ship.fuel,
            stores: ship.stores,
            water: 0,
        };
        let weight =
            ports::weight24(content, &cargo).map_err(|_| illegal("cargo quantities invalid"))?;
        if weight == 0 {
            return Err(illegal("omit empty convoys"));
        }
        total24 = total24
            .checked_add(weight)
            .ok_or_else(|| illegal("cargo overflow"))?;
        let port =
            ports::lane_destination(content, lane).map_err(|e| Rejection::Engine(engine(e)))?;
        let w = by_stage
            .entry((port.id.clone(), ship.arrival_opstage))
            .or_default();
        *w = w
            .checked_add(weight)
            .ok_or_else(|| illegal("cargo overflow"))?;
        let capacity = ports::planning_capacity_tons(content, state, &port)
            .map_err(|error| Rejection::Engine(engine(error)))?
            * 24;
        if *w > capacity
            && !(old.level == ConvoyLevel::G
                && port.name == cna_tables::airlog::trucks::PortName::Tripoli)
        {
            return Err(illegal(
                "planned arrivals exceed the port's maximum stage capacity",
            ));
        }
        convoys.insert(
            lane,
            NavalConvoy {
                lane,
                arrival_opstage: ship.arrival_opstage,
                cargo,
                status: ConvoyStatus::Planned,
                delivered: None,
            },
        );
    }
    if total24 > i64::from(old.capacity_tons) * 24 {
        return Err(illegal("cargo exceeds the rolled Game-Turn tonnage"));
    }
    let turn = state.logistics.convoy_turns.get_mut(&gt).unwrap();
    turn.convoys = convoys;
    turn.planning_complete = true;
    state.logistics.convoy_planning_queue.retain(|n| *n != gt);
    cx.emit(EngineEvent::new(
        Audience::Side(Side::Axis),
        GameEvent::Note {
            text: format!(
                "Fixed GT{gt} convoy lanes, cargo and arrival stages; {}tons/24 allocated.",
                total24
            ),
        },
    ));
    next(content, state, strict, cx).map_err(Rejection::Engine)?;
    Ok(format!("Convoys planned for GT{gt}"))
}
/// OOB invokes land arrivals first, then this supply handler. Mandatory reinforcements are
/// independent. A captured destination cancels the convoy; no rerouting or replacement loss
/// is invented. Cargo beyond the remaining stage capacity is turned back, never carried over.
/// Cases: airlog:55.14, airlog:55.15, airlog:56.15, airlog:56.24, airlog:56.27, airlog:56.28
/// Interpretations: interp:airlog-0010, interp:airlog-0013
pub fn arrive(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    ports::preflight(content, strict)?;
    let gt = state.cursor.game_turn;
    let op = state.cursor.op_stage.unwrap_or(1);
    let Some(turn) = state.logistics.convoy_turns.get(&gt).cloned() else {
        return Ok(());
    };
    for ship in turn
        .convoys
        .values()
        .filter(|s| s.status == ConvoyStatus::Planned && s.arrival_opstage == op)
    {
        let port = ports::lane_destination(content, ship.lane).map_err(engine)?;
        let owner = match ports::state(content, state, &port) {
            Ok(port_state) => port_state.owner,
            Err(error) if !strict && error.is_unknown() => {
                let convoy = state
                    .logistics
                    .convoy_turns
                    .get_mut(&gt)
                    .unwrap()
                    .convoys
                    .get_mut(&ship.lane)
                    .unwrap();
                convoy.status = ConvoyStatus::Unassessed;
                convoy.delivered = None;
                let mut event = EngineEvent::new(
                    Audience::Side(Side::Axis),
                    GameEvent::Note {
                        text: format!(
                            "GT{gt} lane{} did not enter play: port efficiency unassessed; no retry or carry-over. {}",
                            ship.lane,
                            match error {
                                ports::PortOperationError::Policy(source) => source.to_string(),
                                ports::PortOperationError::Supply(source) => format!("{source:?}"),
                            }
                        ),
                    },
                );
                if let Some(hex) = port.location.hex() {
                    event = event.at(hex.clone());
                }
                cx.emit(event);
                continue;
            }
            Err(error) => return Err(engine(error)),
        };
        if owner != Side::Axis {
            state
                .logistics
                .convoy_turns
                .get_mut(&gt)
                .unwrap()
                .convoys
                .get_mut(&ship.lane)
                .unwrap()
                .status = ConvoyStatus::Cancelled;
            cx.emit(EngineEvent::new(
                Audience::Side(Side::Axis),
                GameEvent::Note {
                    text: format!(
                        "GT{gt} lane{} cancelled: destination captured (56.15).",
                        ship.lane
                    ),
                },
            ));
            continue;
        }
        let mut draft = state.clone();
        ports::advance(content, &mut draft, &port).map_err(engine)?;
        let weight = ports::weight24(content, &ship.cargo).map_err(engine)?;
        let ps = ports::state(content, &draft, &port).map_err(engine)?;
        let g = turn.level == ConvoyLevel::G
            && port.name == cna_tables::airlog::trucks::PortName::Tripoli;
        let remaining = if g && ps.efficiency > 0 {
            weight
        } else {
            (ports::capacity_tons(content, &draft, &port).map_err(engine)? * 24 - ps.used_tons24)
                .max(0)
        };
        let allowed = remaining.min(weight);
        let mut delivered = ship.cargo;
        if allowed < weight {
            for t in [
                cna_tables::airlog::supply::SupplyType::Ammo,
                cna_tables::airlog::supply::SupplyType::Fuel,
                cna_tables::airlog::supply::SupplyType::Stores,
            ] {
                let n = i64::from(super::capacity::points(&ship.cargo, t)) * allowed / weight;
                super::capacity::set_points(
                    &mut delivered,
                    t,
                    i32::try_from(n).map_err(|_| engine(SupplyError::Invalid))?,
                );
            }
            cx.emit(EngineEvent::new(Audience::Side(Side::Axis),GameEvent::Note{text:format!("GT{gt} lane{}: {:?} delivered from {:?}; the remaining cargo was turned back by port capacity (55.14/56.28).",ship.lane,delivered,ship.cargo)}));
        }
        let delivered_weight = ports::weight24(content, &delivered).map_err(engine)?;
        if delivered_weight > 0 {
            ports::charge(content, &mut draft, Side::Axis, &port, delivered_weight, g)
                .map_err(engine)?;
        }
        if delivered_weight == 0 {
            let convoy = draft
                .logistics
                .convoy_turns
                .get_mut(&gt)
                .unwrap()
                .convoys
                .get_mut(&ship.lane)
                .unwrap();
            convoy.status = ConvoyStatus::Arrived;
            convoy.delivered = Some(delivered);
            state.logistics = draft.logistics;
            continue;
        }
        super::distribution::validate_dump_capacity(
            content,
            &draft,
            Side::Axis,
            &port.location,
            &delivered,
        )
        .map_err(engine)?;
        let id = draft
            .logistics
            .dumps
            .values()
            .find(|d| {
                d.side == Side::Axis
                    && d.active
                    && !d.dummy
                    && match (&d.location, &port.location) {
                        (DumpLocation::Hex { hex: a }, crate::state::Location::Hex { hex: b }) => {
                            a == b
                        }
                        (
                            DumpLocation::OffMap { id: a },
                            crate::state::Location::OffMap { id: b },
                        ) => a == b,
                        _ => false,
                    }
            })
            .map(|d| d.id.clone())
            .unwrap_or_else(|| format!("axis.port.{}", port.id));
        let marker = if let Some(d) = draft.logistics.dumps.get(&id) {
            d.marker.clone()
        } else {
            super::dump_markers::next_marker(&mut draft.logistics).map_err(engine)?
        };
        let dump = draft
            .logistics
            .dumps
            .entry(id.clone())
            .or_insert_with(|| Dump {
                marker,
                id,
                side: Side::Axis,
                location: match &port.location {
                    crate::state::Location::Hex { hex } => DumpLocation::Hex { hex: hex.clone() },
                    crate::state::Location::OffMap { id } => {
                        DumpLocation::OffMap { id: id.clone() }
                    }
                    _ => unreachable!(),
                },
                supplies: Supplies::default(),
                active: true,
                dummy: false,
            });
        for t in [
            cna_tables::airlog::supply::SupplyType::Ammo,
            cna_tables::airlog::supply::SupplyType::Fuel,
            cna_tables::airlog::supply::SupplyType::Stores,
        ] {
            let n = super::capacity::points(&dump.supplies, t)
                .checked_add(super::capacity::points(&delivered, t))
                .ok_or_else(|| engine(SupplyError::Invalid))?;
            super::capacity::set_points(&mut dump.supplies, t, n);
        }
        draft
            .logistics
            .convoy_turns
            .get_mut(&gt)
            .unwrap()
            .convoys
            .get_mut(&ship.lane)
            .unwrap()
            .status = ConvoyStatus::Arrived;
        draft
            .logistics
            .convoy_turns
            .get_mut(&gt)
            .unwrap()
            .convoys
            .get_mut(&ship.lane)
            .unwrap()
            .delivered = Some(delivered);
        state.logistics = draft.logistics;
        cx.emit(EngineEvent::new(
            Audience::Side(Side::Axis),
            GameEvent::Note {
                text: format!(
                    "GT{gt} lane{} unloaded {:?} into its port dump.",
                    ship.lane, delivered
                ),
            },
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
