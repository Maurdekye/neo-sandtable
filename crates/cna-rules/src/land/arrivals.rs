//! Scheduled order-of-battle changes, before supply convoy arrivals.
use crate::{
    CnaContent, State,
    state::{Location, Pending},
    steps::{illegal, open},
};
use cna_content::{
    scenario::{Placement, Supplies},
    units::{Arrival, ScheduledUnit, Trucks},
};
use cna_core::{
    decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema as Field, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{DecisionId, SeatId, UnitId},
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role, Side};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const PLACE: &str = "cna.arrivals.place";
pub(crate) const TRUCKS: &str = "cna.arrivals.trucks";
pub(crate) const SUBSTITUTE: &str = "cna.withdrawals.substitute";
pub(crate) const TRANSPORT: &str = "cna.withdrawals.transport";
pub(crate) const AIR: &str = "cna.arrivals.air";
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArrivalState {
    pub entered: BTreeSet<String>,
    pub supply_finished: BTreeSet<String>,
    /// Exact unit ids placed during each stage, for the subsequent actual-stock supply window.
    pub newly_arrived: BTreeMap<String, BTreeSet<UnitId>>,
    pub applied: BTreeSet<String>,
    pub unsupported_air_withdrawals: BTreeSet<String>,
    pub tasks: BTreeMap<DecisionId, Task>,
    pub batches: BTreeMap<String, Batch>,
    pub withdrawals: BTreeMap<String, Withdrawal>,
    pub withdrawn_units: BTreeSet<UnitId>,
    pub air_remaining: BTreeMap<String, BTreeMap<String, i32>>,
    pub air_week: BTreeMap<String, (u16, i32)>,
    pub air_deferred: BTreeMap<String, (u16, u8)>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Batch {
    pub side: Side,
    pub units: Vec<UnitId>,
    pub city: String,
    pub trucks: Trucks,
    pub alone: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Withdrawal {
    pub selected: BTreeSet<UnitId>,
    pub eliminated: BTreeSet<UnitId>,
    pub resolved: BTreeSet<UnitId>,
    pub removed: bool,
    pub needed_halves: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Task {
    Place { row: String, unit: UnitId },
    Trucks { row: String },
    Substitute { row: String, named: UnitId },
    Transport { row: String, source: Option<String> },
    Air { row: String, quota: bool },
}
fn invariant(detail: impl Into<String>) -> EngineError {
    EngineError::Invariant {
        detail: detail.into(),
    }
}
fn stage(state: &State) -> String {
    format!(
        "{}:{}",
        state.cursor.game_turn,
        state.cursor.op_stage.unwrap_or(0)
    )
}
fn row_id(file: &std::path::Path, kind: &str, i: usize) -> String {
    format!(
        "{}:{kind}:{i}",
        file.file_name()
            .expect("schedule filename")
            .to_string_lossy()
    )
}
fn note(cx: &mut Cx<'_>, side: Side, text: String) {
    cx.emit(EngineEvent::new(
        Audience::Side(side),
        GameEvent::Note { text },
    ));
}
/// Notes may identify only the owning side's counter and its known position.
/// Cases: land:3.6, land:20.12, land:20.84
fn unit_note(cx: &mut Cx<'_>, side: Side, id: &UnitId, location: &Location, text: String) {
    let mut event = EngineEvent::new(Audience::Side(side), GameEvent::Note { text }).about(id);
    if let Some(hex) = location.hex() {
        event = event.at(hex);
    }
    cx.emit(event);
}
fn options(items: Vec<(String, String)>) -> ActionSpace {
    ActionSpace::new(ActionSchema::Choice {
        options: items
            .into_iter()
            .map(|(id, label)| ChoiceOption {
                id,
                label,
                detail: None,
            })
            .collect(),
    })
}
// The shared decision opener keeps these fields explicit for auditability.
#[allow(clippy::too_many_arguments)]
fn ask(
    state: &mut State,
    cx: &mut Cx<'_>,
    side: Side,
    role: Role,
    kind: &str,
    task: Task,
    summary: String,
    case: &str,
    space: ActionSpace,
) {
    open(
        state,
        cx,
        SeatId::new(side, role),
        kind,
        summary,
        &[case],
        Trigger::Scheduled,
        Secrecy::Secret,
        space,
    );
    state
        .land
        .arrivals
        .tasks
        .insert(state.decisions.pending.last().unwrap().id.clone(), task);
}
/// Assigned OA trees, never player-added attachments. A `less` counter excludes its tree.
/// Cases: land:4.43, land:20.11, land:20.81
fn descendants(content: &CnaContent, root: &UnitId) -> BTreeSet<UnitId> {
    let mut ids = BTreeSet::from([root.clone()]);
    loop {
        let add: Vec<_> = content
            .units
            .units
            .values()
            .filter(|u| !ids.contains(&u.id) && u.parent.as_ref().is_some_and(|p| ids.contains(p)))
            .map(|u| u.id.clone())
            .collect();
        if add.is_empty() {
            break;
        }
        ids.extend(add);
    }
    ids
}
/// Arriving subtrees include only children assigned to this exact printed stage.
/// Cases: land:4.43, land:20.11
fn expand(
    content: &CnaContent,
    selectors: &[ScheduledUnit],
    at: Option<(u16, u8)>,
) -> BTreeSet<UnitId> {
    let mut result = BTreeSet::new();
    for selector in selectors {
        let skipped: BTreeSet<_> = selector
            .less
            .iter()
            .flat_map(|id| descendants(content, id))
            .collect();
        let members = if selector.subtree && !selector.hq_only {
            descendants(content, &selector.unit)
        } else {
            BTreeSet::from([selector.unit.clone()])
        };
        result.extend(members.into_iter().filter(|id| {
            !skipped.contains(id)
                && (id == &selector.unit
                    || at.is_none_or(|(gt, opstage)| {
                        content.units.units[id].arrives == Arrival::At { gt, opstage }
                    }))
        }));
    }
    result
}
fn city_domain(content: &CnaContent, city: &str) -> Result<Vec<Location>, EngineError> {
    crate::setup::placement::choices(
        content,
        &Placement::City { city: city.into() },
        Side::Commonwealth,
        "land:20.14",
    )
}
/// Arrival placement costs no CP; unknown stacking constraints use the agreed profile split.
/// Cases: land:20.12, land:9.12, land:8.37, land:9.16
fn valid_destination(
    content: &CnaContent,
    state: &State,
    unit: &UnitId,
    location: &Location,
    strict: bool,
) -> Result<Option<String>, Rejection> {
    let Some(hex) = location.hex() else {
        return Ok(None);
    };
    let side = state.land.units[unit].side;
    if state
        .units_of(side.opponent())
        .any(|u| u.location.hex() == Some(hex))
    {
        return Err(illegal("not a legal arrival destination"));
    }
    let mut copy = state.clone();
    copy.land.units.get_mut(unit).unwrap().location = location.clone();
    match crate::land::stacking::validate_end(content, &copy, hex, side, true) {
        Ok(()) => Ok(None),
        Err(Rejection::Engine(EngineError::Unsupported { case, .. }))
            if !strict && matches!(case.as_str(), "land:8.37" | "land:9.16") =>
        {
            Ok(Some(case))
        }
        Err(e) => Err(e),
    }
}
/// Cases: land:20.11, land:20.12, land:20.14, land:20.15
pub(crate) fn enter(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let key = stage(state);
    if !state.land.arrivals.entered.insert(key) {
        return Ok(());
    }
    let gt = state.cursor.game_turn;
    let opstage = state
        .cursor
        .op_stage
        .ok_or_else(|| invariant("arrival without OpStage"))?;
    for schedule in &content.units.schedules {
        for (i, row) in schedule.arrivals.iter().enumerate() {
            if row.units.is_empty() && row.trucks.is_none() {
                continue;
            }
            if row.gt != Some(gt) || row.opstage != Some(opstage) {
                continue;
            }
            let id = row_id(&schedule.path, "land", i);
            if state.land.arrivals.applied.contains(&id) {
                continue;
            }
            let side = schedule.file.side;
            let units: Vec<_> = expand(content, &row.units, Some((gt, opstage)))
                .into_iter()
                .filter(|u| matches!(state.land.units[u].location, Location::NotArrived))
                .collect();
            let city = row.location.clone().unwrap_or_else(|| {
                if side == Side::Axis {
                    "tripoli".into()
                } else {
                    "cairo".into()
                }
            });
            if side == Side::Axis {
                if strict {
                    return Err(EngineError::Unsupported {
                        case: "land:20.15".into(),
                        detail: "Benghazi diversion requires verified port throughput and anchors"
                            .into(),
                    });
                }
                note(cx,side,"Benghazi diversion is unavailable until its port capacity and arrival anchor are verified (land:20.15); printed Tripoli arrivals remain available.".into());
            }
            state.land.arrivals.batches.insert(
                id,
                Batch {
                    side,
                    units,
                    city,
                    trucks: row.trucks.unwrap_or_default(),
                    alone: row.alone,
                },
            );
        }
    }
    advance_arrivals(content, state, strict, cx)?;
    air_start(content, state, cx)
}
fn waiting_at_arrival(location: &Location) -> bool {
    matches!(location, Location::NotArrived)
        || matches!(location,Location::AwaitingSetup{group} if group.starts_with("arrival:"))
}
/// Cases: land:20.12, land:20.14, land:4.43
fn advance_arrivals(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let rows: Vec<_> = state.land.arrivals.batches.keys().cloned().collect();
    for row in rows {
        if state.land.arrivals.applied.contains(&row) {
            continue;
        }
        let b = state.land.arrivals.batches[&row].clone();
        for unit in &b.units {
            if !waiting_at_arrival(&state.land.units[unit].location) {
                continue;
            }
            let domain = city_domain(content, &b.city)?;
            let mut legal = Vec::new();
            for destination in domain {
                match valid_destination(content, state, unit, &destination, strict) {
                    Ok(_) => legal.push(destination),
                    Err(Rejection::Illegal { .. }) => {}
                    Err(Rejection::Engine(e)) => return Err(e),
                    Err(_) => return Err(invariant("unexpected arrival validation result")),
                }
            }
            if legal.is_empty() {
                if strict {
                    return Err(EngineError::Unsupported {
                        case: "land:20.14".into(),
                        detail: format!("no capacity-valid arrival location for {unit}"),
                    });
                }
                unit_note(
                    cx,
                    b.side,
                    unit,
                    &state.land.units[unit].location,
                    format!(
                        "{unit} awaits arrival placement: no verified capacity-valid destination at {} (land:20.14).",
                        b.city
                    ),
                );
                state.land.units.get_mut(unit).unwrap().location = Location::AwaitingSetup {
                    group: format!("arrival:{row}"),
                };
                continue;
            }
            if legal.len() == 1 && matches!(legal[0], Location::OffMap { .. }) {
                place(state, unit, legal.remove(0), cx);
                continue;
            }
            let choices = legal
                .iter()
                .map(|l| {
                    let id = crate::setup::placement::destination_id(l).unwrap();
                    (id.clone(), id)
                })
                .collect();
            ask(
                state,
                cx,
                b.side,
                Role::Commander,
                PLACE,
                Task::Place {
                    row: row.clone(),
                    unit: unit.clone(),
                },
                format!("Place arriving {unit} in {}.", b.city),
                "land:20.14",
                options(choices).with_context(json!({"unit":unit,"arrival":row})),
            );
            return Ok(());
        }
        if b.trucks.total() > 0 {
            if b.alone || b.units.is_empty() {
                let domain = city_domain(content, &b.city)?;
                if domain.len() != 1 {
                    open_pool_arrival(state, cx, &row, &b, domain);
                    return Ok(());
                }
                crate::logistics::pools::add_truck_pool(
                    &mut state.logistics,
                    None,
                    b.side,
                    Placement::City {
                        city: b.city.clone(),
                    },
                    Some(domain[0].clone()),
                    b.trucks,
                    Supplies::default(),
                )
                .map_err(invariant)?;
                state.land.arrivals.batches.get_mut(&row).unwrap().trucks = Trucks::default();
            } else {
                open_arrival_trucks(state, cx, &row, &b);
                return Ok(());
            }
        }
        if !b
            .units
            .iter()
            .any(|id| waiting_at_arrival(&state.land.units[id].location))
        {
            state.land.arrivals.applied.insert(row);
        }
    }
    Ok(())
}
fn place(state: &mut State, id: &UnitId, destination: Location, cx: &mut Cx<'_>) {
    let unit = state.land.units.get_mut(id).unwrap();
    unit.location = destination;
    unit.cp_spent_quarters = 0;
    unit.voluntary_cp_quarters = 0;
    let side = unit.side;
    state.land.arrivals.withdrawn_units.remove(id);
    let arrival_stage = stage(state);
    state
        .land
        .arrivals
        .newly_arrived
        .entry(arrival_stage)
        .or_default()
        .insert(id.clone());
    unit_note(
        cx,
        side,
        id,
        &state.land.units[id].location,
        format!("{id} has arrived without debarkation CP expenditure (land:20.12)."),
    );
}
fn truck_fields(trucks: Trucks) -> Vec<Field> {
    [
        ("light", trucks.light),
        ("medium", trucks.medium),
        ("heavy", trucks.heavy),
    ]
    .into_iter()
    .map(|(name, max)| Field {
        name: name.into(),
        doc: "Printed truck points".into(),
        schema: ActionSchema::Integer {
            min: 0,
            max: i64::from(max),
        },
        optional: false,
    })
    .collect()
}
/// Cases: land:4.43, airlog:53.11
fn open_arrival_trucks(state: &mut State, cx: &mut Cx<'_>, row: &str, b: &Batch) {
    let mut fields = vec![Field {
        name: "unit".into(),
        doc: "Unit arriving on this row".into(),
        schema: ActionSchema::Unit {
            among: b.units.clone(),
        },
        optional: false,
    }];
    fields.extend(truck_fields(b.trucks));
    ask(
        state,
        cx,
        b.side,
        Role::Logistics,
        TRUCKS,
        Task::Trucks { row: row.into() },
        "Attach arriving truck points to the units on their printed arrival row.".into(),
        "land:4.43",
        ActionSpace::new(ActionSchema::Record { fields })
            .with_context(json!({"arrival":row,"units":b.units})),
    );
}
/// Cases: land:4.43
fn open_pool_arrival(
    state: &mut State,
    cx: &mut Cx<'_>,
    row: &str,
    b: &Batch,
    domain: Vec<Location>,
) {
    ask(
        state,
        cx,
        b.side,
        Role::Logistics,
        TRUCKS,
        Task::Trucks { row: row.into() },
        format!("Place arriving unattached trucks in {}.", b.city),
        "land:4.43",
        options(
            domain
                .iter()
                .map(|l| {
                    let id = crate::setup::placement::destination_id(l).unwrap();
                    (id.clone(), id)
                })
                .collect(),
        )
        .with_context(json!({"arrival":row,"pool":"new"})),
    );
}
fn parse_trucks(action: &Value, available: Trucks) -> Result<Trucks, Rejection> {
    let count = |name: &str, max: i32| {
        action
            .get(name)
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok())
            .filter(|n| *n >= 0 && *n <= max)
            .ok_or_else(|| illegal("truck allocation exceeds available points"))
    };
    let t = Trucks {
        light: count("light", available.light)?,
        medium: count("medium", available.medium)?,
        heavy: count("heavy", available.heavy)?,
    };
    if i64::from(t.light) + i64::from(t.medium) + i64::from(t.heavy) == 0 {
        return Err(illegal("allocate at least one truck point"));
    }
    Ok(t)
}
fn subtract(trucks: &mut Trucks, amount: Trucks) {
    trucks.light -= amount.light;
    trucks.medium -= amount.medium;
    trucks.heavy -= amount.heavy;
}
fn add(trucks: &mut Trucks, amount: Trucks) -> Result<(), Rejection> {
    trucks.light = trucks
        .light
        .checked_add(amount.light)
        .ok_or_else(|| illegal("truck holding overflow"))?;
    trucks.medium = trucks
        .medium
        .checked_add(amount.medium)
        .ok_or_else(|| illegal("truck holding overflow"))?;
    trucks.heavy = trucks
        .heavy
        .checked_add(amount.heavy)
        .ok_or_else(|| illegal("truck holding overflow"))?;
    Ok(())
}
fn ready_for_withdrawal(content: &CnaContent, state: &State, id: &UnitId) -> bool {
    let Some(class) = crate::land::formation::class(content, id) else {
        return false;
    };
    if class.unit_type == "headquarters"
        && class.max_toe.is_none()
        && matches!(
            state.land.units[id].toe,
            None | Some(cna_content::units::Toe::Normal(_))
        )
    {
        return true;
    }
    let strength = crate::land::formation::strength(content, state, id);
    i64::from(strength) * 4 >= i64::from(class.max_toe.unwrap_or(0)) * 3
        && class.max_toe.is_some_and(|n| n > 0)
}
fn in_withdrawal_city(content: &CnaContent, location: &Location) -> bool {
    ["cairo", "alexandria"]
        .iter()
        .any(|city| city_domain(content, city).is_ok_and(|d| d.contains(location)))
}
/// The substitution route uses an equivalent printed type and echelon, at the required TOE.
/// Replacement production is a separate procedure, so no replacement answer is offered here.
/// Cases: land:20.82, land:20.83, land:20.85
fn substitutes(content: &CnaContent, state: &State, named: &UnitId, row: &str) -> Vec<UnitId> {
    let oa = &content.units.units[named];
    let Some(class) = crate::land::formation::class(content, named) else {
        return vec![];
    };
    let reserved: BTreeSet<_> = state
        .land
        .arrivals
        .withdrawals
        .values()
        .flat_map(|w| w.selected.iter().chain(w.eliminated.iter()).cloned())
        .collect();
    state
        .units_of(Side::Commonwealth)
        .filter(|u| {
            u.id != *named
                && !reserved.contains(&u.id)
                && !state.land.arrivals.withdrawals[row]
                    .resolved
                    .contains(&u.id)
                && in_withdrawal_city(content, &u.location)
                && ready_for_withdrawal(content, state, &u.id)
                && content.units.units[&u.id].echelon == oa.echelon
                && (class.unit_type != "infantry"
                    || (oa.infantry_kind.is_some()
                        && content.units.units[&u.id].infantry_kind == oa.infantry_kind))
                && crate::land::formation::class(content, &u.id).is_some_and(|c| {
                    c.unit_type == class.unit_type
                        && (class.unit_type != "headquarters" || c.id == class.id)
                })
        })
        .map(|u| u.id.clone())
        .collect()
}
/// Named deadlines are adjudicated in the land arrival phase before supply is delivered.
/// Cases: land:20.81, land:20.83, land:20.84, land:20.85
fn prepare_withdrawals(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let gt = state.cursor.game_turn;
    let opstage = state.cursor.op_stage.unwrap();
    for schedule in &content.units.schedules {
        for (i, row) in schedule.withdrawals.iter().enumerate() {
            if row.gt != Some(gt) || row.opstage != Some(opstage) || row.units.is_empty() {
                continue;
            }
            let id = row_id(&schedule.path, "withdrawal", i);
            if state.land.arrivals.applied.contains(&id) {
                continue;
            }
            state
                .land
                .arrivals
                .withdrawals
                .entry(id.clone())
                .or_default();
            let units = expand(content, &row.units, None);
            for named in units {
                if state.land.arrivals.withdrawals[&id]
                    .resolved
                    .contains(&named)
                {
                    continue;
                }
                let oa = &content.units.units[&named];
                if matches!(oa.arrives,Arrival::At{gt:g,opstage:s}if(g,s)>(gt,opstage)) {
                    continue;
                }
                let class = crate::land::formation::class(content, &named);
                let unknown = class.is_none_or(|c| {
                    c.max_toe.is_none_or(|n| n <= 0)
                        && !(c.unit_type == "headquarters"
                            && c.max_toe.is_none()
                            && matches!(
                                state.land.units[&named].toe,
                                None | Some(cna_content::units::Toe::Normal(_))
                            ))
                });
                if unknown {
                    if strict {
                        return Err(EngineError::Unsupported {
                            case: "land:20.85".into(),
                            detail: format!("{named} has no verified numerical TOE maximum"),
                        });
                    }
                    unit_note(
                        cx,
                        schedule.file.side,
                        &named,
                        &state.land.units[&named].location,
                        format!(
                            "Withdrawal {named} is unassessed because its printed TOE maximum is missing (land:20.85); no strength or elimination is invented."
                        ),
                    );
                    state
                        .land
                        .arrivals
                        .withdrawals
                        .get_mut(&id)
                        .unwrap()
                        .resolved
                        .insert(named);
                    continue;
                }
                if ready_for_withdrawal(content, state, &named)
                    && in_withdrawal_city(content, &state.land.units[&named].location)
                {
                    state
                        .land
                        .arrivals
                        .withdrawals
                        .get_mut(&id)
                        .unwrap()
                        .selected
                        .insert(named.clone());
                } else if !ready_for_withdrawal(content, state, &named) {
                    let candidates = substitutes(content, state, &named, &id);
                    if !candidates.is_empty() {
                        let mut choices: Vec<_> = candidates
                            .into_iter()
                            .map(|id| (id.to_string(), format!("Substitute {id}")))
                            .collect();
                        choices.push((
                            "eliminate".into(),
                            format!("Eliminate {named} for the missed withdrawal deadline"),
                        ));
                        ask(
                            state,
                            cx,
                            schedule.file.side,
                            Role::Commander,
                            SUBSTITUTE,
                            Task::Substitute {
                                row: id.clone(),
                                named: named.clone(),
                            },
                            format!(
                                "Choose an equivalent substitute for understrength {named}, or accept its elimination."
                            ),
                            "land:20.85",
                            options(choices).with_context(json!({"unit":named,"withdrawal":id})),
                        );
                        return Ok(());
                    }
                    state
                        .land
                        .arrivals
                        .withdrawals
                        .get_mut(&id)
                        .unwrap()
                        .eliminated
                        .insert(named.clone());
                } else {
                    state
                        .land
                        .arrivals
                        .withdrawals
                        .get_mut(&id)
                        .unwrap()
                        .eliminated
                        .insert(named.clone());
                }
                state
                    .land
                    .arrivals
                    .withdrawals
                    .get_mut(&id)
                    .unwrap()
                    .resolved
                    .insert(named);
            }
            if !state.land.arrivals.withdrawals[&id].removed {
                let weights = schedule
                    .file
                    .truck_value_halves
                    .ok_or_else(|| invariant("withdrawal schedule lacks truck-value weights"))?;
                let mut accompanying = 0i64;
                let w = state.land.arrivals.withdrawals[&id].clone();
                for unit in &w.selected {
                    accompanying += weights.value(state.land.units[unit].trucks);
                    let trucks = state.land.units[unit].trucks;
                    let departure = state.land.units[unit].location.clone();
                    if trucks.total() > 0 {
                        crate::logistics::box_handling::prepare_division(
                            state,
                            &crate::logistics::box_handling::Carrier::Unit(unit.clone()),
                            strict,
                            cx,
                        )?;
                    }
                    retire_truck_history(content, state, unit, trucks)
                        .map_err(|r| invariant(format!("{r:?}")))?;
                    state.logistics.rations.remove(unit);
                    let u = state.land.units.get_mut(unit).unwrap();
                    u.location = Location::NotArrived;
                    u.trucks = Trucks::default();
                    u.transport_trucks = Trucks::default();
                    state.logistics.unit_supply.remove(unit);
                    state.land.arrivals.withdrawn_units.insert(unit.clone());
                    unit_note(
                        cx,
                        schedule.file.side,
                        unit,
                        &departure,
                        format!("{unit} has been withdrawn (land:20.84)."),
                    );
                }
                for unit in &w.eliminated {
                    let trucks = state.land.units[unit].trucks;
                    let departure = state.land.units[unit].location.clone();
                    if trucks.total() > 0 {
                        crate::logistics::box_handling::prepare_division(
                            state,
                            &crate::logistics::box_handling::Carrier::Unit(unit.clone()),
                            strict,
                            cx,
                        )?;
                    }
                    retire_truck_history(content, state, unit, trucks)
                        .map_err(|r| invariant(format!("{r:?}")))?;
                    state.logistics.rations.remove(unit);
                    let u = state.land.units.get_mut(unit).unwrap();
                    u.location = Location::Eliminated;
                    u.trucks = Trucks::default();
                    u.transport_trucks = Trucks::default();
                    state.logistics.unit_supply.remove(unit);
                    state.land.arrivals.withdrawn_units.remove(unit);
                    unit_note(
                        cx,
                        schedule.file.side,
                        unit,
                        &departure,
                        format!(
                            "{unit} is permanently eliminated after missing the required withdrawal location or TOE (land:20.83)."
                        ),
                    );
                }
                let minimum = i64::from(row.transport.map_or(0, |t| t.truck_value_points)) * 2;
                let w = state.land.arrivals.withdrawals.get_mut(&id).unwrap();
                w.needed_halves = (minimum - accompanying).max(0);
                w.removed = true;
            }
            if state.land.arrivals.withdrawals[&id].needed_halves > 0 {
                open_withdrawal_transport(content, state, &id, strict, cx)?;
                if state
                    .land
                    .arrivals
                    .tasks
                    .values()
                    .any(|t| matches!(t,Task::Transport{row,..}if row==&id))
                {
                    return Ok(());
                }
            }
            state.land.arrivals.applied.insert(id);
        }
    }
    Ok(())
}
fn empty_trucks(content: &CnaContent, state: &State) -> BTreeMap<String, Trucks> {
    let mut assets = BTreeMap::new();
    for unit in state.units_of(Side::Commonwealth) {
        if !in_withdrawal_city(content, &unit.location)
            || state
                .logistics
                .unit_supply
                .get(&unit.id)
                .is_some_and(|s| s.carried != Supplies::default())
        {
            continue;
        }
        let free = Trucks {
            light: unit.trucks.light - unit.transport_trucks.light,
            medium: unit.trucks.medium - unit.transport_trucks.medium,
            heavy: unit.trucks.heavy - unit.transport_trucks.heavy,
        };
        if free.total() > 0 {
            assets.insert(format!("unit:{}", unit.id), free);
        }
    }
    for pool in &state.logistics.truck_pools {
        if pool.side == Side::Commonwealth
            && pool.cargo == Supplies::default()
            && pool.tank_fuel.is_zero()
            && pool.activity_water.is_zero()
            && pool
                .location
                .as_ref()
                .is_some_and(|l| in_withdrawal_city(content, l))
            && pool.trucks.total() > 0
        {
            assets.insert(format!("pool:{}", pool.id), pool.trucks);
        }
    }
    assets
}
fn weights(content: &CnaContent) -> Result<cna_content::units::TruckValueHalves, EngineError> {
    content
        .units
        .schedules
        .iter()
        .find(|s| s.file.side == Side::Commonwealth && s.file.truck_value_halves.is_some())
        .and_then(|s| s.file.truck_value_halves)
        .ok_or_else(|| invariant("missing Commonwealth truck-value conversion"))
}
/// Only empty extra trucks at a verified withdrawal city are selectable.
/// Cases: land:4.43, land:20.83
/// Interpretations: interp:scen-0006
fn open_withdrawal_transport(
    content: &CnaContent,
    state: &mut State,
    row: &str,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let assets = empty_trucks(content, state);
    let weights = weights(content)?;
    let total: i64 = assets.values().map(|t| weights.value(*t)).sum();
    let needed = state.land.arrivals.withdrawals[row].needed_halves;
    if total < needed {
        // The full-profile stop is adjudicated by finish_step, never during answer validation.
        if strict {
            return Err(EngineError::Unsupported{case:"land:20.83".into(),detail:"printed withdrawal truck minimum cannot be satisfied; no penalty is specified in land:4.43a".into()});
        }
        for (asset, trucks) in assets {
            remove_empty_trucks(content, state, &asset, trucks, strict, cx)
                .map_err(|r| invariant(format!("{r:?}")))?;
        }
        state
            .land
            .arrivals
            .withdrawals
            .get_mut(row)
            .unwrap()
            .needed_halves = 0;
        note(
            cx,
            Side::Commonwealth,
            format!(
                "Withdrawal {row} lacks {} half truck-value points; all eligible empty trucks have withdrawn. No unit penalty is invented (land:4.43a / 20.83, proposed interp:scen-0006).",
                needed - total
            ),
        );
        return Ok(());
    }
    ask(
        state,
        cx,
        Side::Commonwealth,
        Role::Logistics,
        TRANSPORT,
        Task::Transport {
            row: row.into(),
            source: None,
        },
        format!(
            "Choose an empty truck holding for {row}: {needed} half-value points still required."
        ),
        "land:20.83",
        options(assets.keys().map(|a| (a.clone(), a.clone())).collect())
            .with_context(json!({"withdrawal":row})),
    );
    Ok(())
}

/// Retire departing cohorts before changing physical counts; paid funding remains spent.
/// Cases: land:20.83, airlog:49.13, airlog:49.16, airlog:52.42
fn retire_truck_history(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    amount: Trucks,
) -> Result<(), Rejection> {
    if amount.total() == 0 {
        return Ok(());
    }
    if state.logistics.fuel_segments.contains_key(id) {
        crate::logistics::remove_segment_fuel_cohorts(state, id, amount)
            .map_err(|_| illegal("truck fuel history cannot be reconciled"))?;
    }
    let stage = crate::logistics::water::WaterStage::current(state);
    if state.logistics.rations.get(id).is_some_and(|r| {
        r.activity_used_stage == Some(stage)
            || r.activity_water_ledger
                .as_ref()
                .is_some_and(|l| l.stage == stage)
    }) {
        crate::logistics::remove_activity_water_credit(content, state, id, amount)
            .map_err(|_| illegal("truck water history cannot be reconciled"))?;
    }
    Ok(())
}
/// Retire the exact departing pool trucks while retaining already-paid fuel.
/// Cases: land:20.83, airlog:49.13, airlog:49.16
fn retire_pool_truck_history(
    state: &mut State,
    id: &str,
    mut amount: Trucks,
) -> Result<(), Rejection> {
    use crate::logistics::{FuelCohortSelection, FuelTruckKind};
    if amount.total() == 0 || !state.logistics.pool_fuel_segments.contains_key(id) {
        return Ok(());
    }
    let cohorts = crate::logistics::pool_fuel::pool_segment_fuel_cohorts(state, id)
        .map_err(|_| illegal("truck pool fuel history cannot be reconciled"))?;
    let mut selection = Vec::new();
    for cohort in cohorts {
        let remaining = match cohort.kind {
            FuelTruckKind::Light => &mut amount.light,
            FuelTruckKind::Medium => &mut amount.medium,
            FuelTruckKind::Heavy => &mut amount.heavy,
        };
        let count = (*remaining).min(cohort.count);
        if count > 0 {
            selection.push(FuelCohortSelection {
                id: cohort.id,
                count,
            });
            *remaining -= count;
        }
    }
    if amount != Trucks::default() {
        return Err(illegal("truck pool fuel history cannot be reconciled"));
    }
    crate::logistics::pool_fuel::remove_selected_pool_fuel_cohorts(state, id, &selection)
        .map_err(|_| illegal("truck pool fuel history cannot be reconciled"))?;
    Ok(())
}
fn remove_empty_trucks(
    content: &CnaContent,
    state: &mut State,
    asset: &str,
    amount: Trucks,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), Rejection> {
    if let Some(id) = asset.strip_prefix("unit:") {
        crate::logistics::box_handling::prepare_division(
            state,
            &crate::logistics::box_handling::Carrier::Unit(UnitId::new(id)),
            strict,
            cx,
        )
        .map_err(Rejection::Engine)?;
        retire_truck_history(content, state, &UnitId::new(id), amount)?;
        let unit = state
            .land
            .units
            .get_mut(&UnitId::new(id))
            .ok_or_else(|| illegal("unknown empty truck holding"))?;
        subtract(&mut unit.trucks, amount);
    } else if let Some(id) = asset.strip_prefix("pool:") {
        crate::logistics::box_handling::prepare_division(
            state,
            &crate::logistics::box_handling::Carrier::Pool(id.into()),
            strict,
            cx,
        )
        .map_err(Rejection::Engine)?;
        retire_pool_truck_history(state, id, amount)?;
        let pool = state
            .logistics
            .truck_pools
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| illegal("unknown empty truck holding"))?;
        subtract(&mut pool.trucks, amount);
    } else {
        return Err(illegal("unknown empty truck holding"));
    }
    Ok(())
}
fn air_row<'a>(
    content: &'a CnaContent,
    id: &str,
) -> Option<(Side, &'a cna_content::units::ScheduledArrival)> {
    content
        .units
        .schedules
        .iter()
        .flat_map(|s| {
            s.arrivals
                .iter()
                .enumerate()
                .map(move |(i, r)| (s.file.side, row_id(&s.path, "air", i), r))
        })
        .find(|(_, key, _)| key == id)
        .map(|(side, _, r)| (side, r))
}
/// A monthly total is balanced across weeks as a whole. Aircraft types remain the owner's choice.
/// All of a week's quota may arrive in OpStage one; delaying to two or three is also explicit.
/// Cases: airlog:34.84
fn air_start(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    let gt = state.cursor.game_turn;
    let opstage = state.cursor.op_stage.unwrap();
    for schedule in &content.units.schedules {
        for (i, row) in schedule.arrivals.iter().enumerate() {
            if row.planes.is_empty() {
                continue;
            }
            let from = row
                .gt_from
                .or(row.gt)
                .ok_or_else(|| invariant("air arrival lacks start turn"))?;
            let to = row
                .gt_to
                .or(row.gt)
                .ok_or_else(|| invariant("air arrival lacks end turn"))?;
            if gt < from || gt > to || row.opstage.is_some_and(|s| s != opstage) {
                continue;
            }
            let id = row_id(&schedule.path, "air", i);
            if state.land.arrivals.applied.contains(&id)
                || state.land.arrivals.air_deferred.get(&id) == Some(&(gt, opstage))
                || state
                    .land
                    .arrivals
                    .tasks
                    .values()
                    .any(|t| matches!(t,Task::Air{row,..}if row==&id))
            {
                continue;
            }
            state
                .land
                .arrivals
                .air_remaining
                .entry(id.clone())
                .or_insert_with(|| {
                    row.planes
                        .iter()
                        .map(|p| (p.aircraft.clone(), p.n))
                        .collect()
                });
            if !state
                .land
                .arrivals
                .air_week
                .get(&id)
                .is_some_and(|(g, _)| *g == gt)
            {
                let remaining: i32 = state.land.arrivals.air_remaining[&id].values().sum();
                let weeks = i32::from(to - gt + 1);
                let base = row.planes.iter().map(|p| p.n).sum::<i32>() / i32::from(to - from + 1);
                let extra = remaining - base * weeks;
                let choices = if extra == 0 {
                    vec![base]
                } else if extra == weeks {
                    vec![base + 1]
                } else {
                    vec![base + 1, base]
                };
                if choices.len() == 1 {
                    state
                        .land
                        .arrivals
                        .air_week
                        .insert(id.clone(), (gt, choices[0]));
                } else {
                    ask(state,cx,schedule.file.side,Role::Air,AIR,Task::Air{row:id.clone(),quota:true},format!("Choose this week's balanced aircraft quota for {}.",row.label.as_deref().unwrap_or("the printed interval")),"airlog:34.84",options(choices.iter().map(|n|(n.to_string(),format!("{n} aircraft this week"))).collect()).with_context(json!({"arrival":id,"force":crate::state::side_key(schedule.file.side)})));
                    continue;
                }
            }
            open_air_types(state, cx, &id, schedule.file.side)?;
        }
    }
    Ok(())
}
fn open_air_types(
    state: &mut State,
    cx: &mut Cx<'_>,
    row: &str,
    side: Side,
) -> Result<(), EngineError> {
    let quota = state.land.arrivals.air_week[row].1;
    if quota == 0 {
        return Ok(());
    }
    let mut choices = Vec::new();
    for (plane, n) in &state.land.arrivals.air_remaining[row] {
        for count in (1..=(*n).min(quota)).rev() {
            choices.push((format!("{plane}:{count}"), format!("{count} {plane}")));
        }
    }
    if state.cursor.op_stage.unwrap() < 3 {
        choices.push((
            "defer".into(),
            "Leave this week's remainder for the next OpStage".into(),
        ));
    }
    if choices.is_empty() {
        return Err(invariant("positive air quota has no aircraft"));
    }
    ask(
        state,
        cx,
        side,
        Role::Air,
        AIR,
        Task::Air {
            row: row.into(),
            quota: false,
        },
        format!("Choose aircraft types to arrive; {quota} remain in this week's quota."),
        "airlog:34.84",
        options(choices).with_context(json!({"arrival":row,"force":crate::state::side_key(side)})),
    );
    Ok(())
}
/// Squadron withdrawal is outside the land schedule procedure; never silently mark it applied.
/// Unsupported: airlog:34.85 - withdrawing squadron selections belong to the air procedure.
fn report_air_withdrawals(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    for schedule in &content.units.schedules {
        for (i, row) in schedule.withdrawals.iter().enumerate() {
            if row.squadrons.is_empty()
                || row.gt != Some(state.cursor.game_turn)
                || state.cursor.op_stage != Some(1)
            {
                continue;
            }
            let id = row_id(&schedule.path, "air-withdrawal", i);
            if strict {
                return Err(EngineError::Unsupported{case:"airlog:34.85".into(),detail:"squadron withdrawal selection is not implemented by the land arrivals procedure".into()});
            }
            if state.land.arrivals.unsupported_air_withdrawals.insert(id) {
                note(cx,schedule.file.side,"Scheduled squadron withdrawal remains unimplemented by the air procedure (airlog:34.85); no planes or pilots are silently removed.".into());
            }
        }
    }
    Ok(())
}
/// Supply convoy delivery follows all recorded land decisions and deadline adjudication.
/// Cases: land:20.12, land:20.83, airlog:48.0
pub(crate) fn finish(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    if !state.land.arrivals.tasks.is_empty() {
        return Ok(());
    }
    advance_arrivals(content, state, strict, cx)?;
    if !state.land.arrivals.tasks.is_empty() {
        return Ok(());
    }
    prepare_withdrawals(content, state, strict, cx)?;
    report_air_withdrawals(content, state, strict, cx)?;
    if !state.land.arrivals.tasks.is_empty() {
        return Ok(());
    }
    if state.land.arrivals.supply_finished.insert(stage(state)) {
        crate::logistics::convoys::arrive(content, state, cx)?;
    }
    Ok(())
}
/// Answers use only owner holdings and public arrival geography; deadlines resolve in finish.
/// Cases: land:20.12, land:20.14, land:20.85, land:4.43, airlog:34.84
pub(crate) fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    let task = state
        .land
        .arrivals
        .tasks
        .get(&pending.id)
        .cloned()
        .ok_or_else(|| illegal("arrival decision is no longer current"))?;
    match task {
        Task::Place { row, unit } => {
            let b = state.land.arrivals.batches[&row].clone();
            if pending.seat.side != b.side || !waiting_at_arrival(&state.land.units[&unit].location)
            {
                return Err(illegal("not an owned arriving unit"));
            }
            let selected = action
                .as_str()
                .ok_or_else(|| illegal("choose a listed arrival location"))?;
            let location = city_domain(content, &b.city)
                .map_err(Rejection::Engine)?
                .into_iter()
                .find(|l| crate::setup::placement::destination_id(l).as_deref() == Some(selected))
                .ok_or_else(|| illegal("not a legal arrival destination"))?;
            if let Some(case) = valid_destination(content, state, &unit, &location, strict)? {
                unit_note(
                    cx,
                    b.side,
                    &unit,
                    &location,
                    format!("Arrival stacking at {selected} is unassessed ({case})."),
                );
            }
            place(state, &unit, location, cx);
        }
        Task::Trucks { row } => {
            let b = state.land.arrivals.batches[&row].clone();
            if b.side != pending.seat.side {
                return Err(illegal("not an owned truck arrival"));
            }
            if b.alone || b.units.is_empty() {
                let chosen = action
                    .as_str()
                    .ok_or_else(|| illegal("choose an arrival destination"))?;
                let location = city_domain(content, &b.city)
                    .map_err(Rejection::Engine)?
                    .into_iter()
                    .find(|l| crate::setup::placement::destination_id(l).as_deref() == Some(chosen))
                    .ok_or_else(|| illegal("not a legal truck arrival location"))?;
                crate::logistics::pools::add_truck_pool(
                    &mut state.logistics,
                    None,
                    b.side,
                    Placement::City { city: b.city },
                    Some(location),
                    b.trucks,
                    Supplies::default(),
                )
                .map_err(|e| Rejection::Engine(invariant(e)))?;
                state.land.arrivals.batches.get_mut(&row).unwrap().trucks = Trucks::default();
            } else {
                let unit = UnitId::new(
                    action
                        .get("unit")
                        .and_then(Value::as_str)
                        .ok_or_else(|| illegal("choose an arriving unit"))?,
                );
                if !b.units.contains(&unit) {
                    return Err(illegal("truck recipient is outside its arrival row"));
                }
                let amount = parse_trucks(action, b.trucks)?;
                add(&mut state.land.units.get_mut(&unit).unwrap().trucks, amount)?;
                subtract(
                    &mut state.land.arrivals.batches.get_mut(&row).unwrap().trucks,
                    amount,
                );
            }
        }
        Task::Substitute { row, named } => {
            let selected = action
                .as_str()
                .ok_or_else(|| illegal("choose a withdrawal substitute"))?;
            if pending.seat.side != Side::Commonwealth {
                return Err(illegal("not an owned withdrawal"));
            }
            if selected == "eliminate" {
                state
                    .land
                    .arrivals
                    .withdrawals
                    .get_mut(&row)
                    .unwrap()
                    .eliminated
                    .insert(named.clone());
            } else {
                let substitute = UnitId::new(selected);
                if !substitutes(content, state, &named, &row).contains(&substitute) {
                    return Err(illegal("not an eligible withdrawal substitute"));
                }
                state
                    .land
                    .arrivals
                    .withdrawals
                    .get_mut(&row)
                    .unwrap()
                    .selected
                    .insert(substitute);
            }
            state
                .land
                .arrivals
                .withdrawals
                .get_mut(&row)
                .unwrap()
                .resolved
                .insert(named);
        }
        Task::Transport { row, source } => {
            if pending.seat.side != Side::Commonwealth {
                return Err(illegal("not an owned withdrawal"));
            }
            let assets = empty_trucks(content, state);
            if let Some(source) = source {
                let available = *assets
                    .get(&source)
                    .ok_or_else(|| illegal("empty truck source is no longer eligible"))?;
                let amount = parse_trucks(action, available)?;
                remove_empty_trucks(content, state, &source, amount, strict, cx)?;
                let value = weights(content).map_err(Rejection::Engine)?.value(amount);
                let w = state.land.arrivals.withdrawals.get_mut(&row).unwrap();
                w.needed_halves = (w.needed_halves - value).max(0);
            } else {
                let source = action
                    .as_str()
                    .filter(|id| assets.contains_key(*id))
                    .ok_or_else(|| illegal("choose an eligible empty truck source"))?
                    .to_owned();
                let fields = truck_fields(assets[&source]);
                ask(
                    state,
                    cx,
                    Side::Commonwealth,
                    Role::Logistics,
                    TRANSPORT,
                    Task::Transport {
                        row: row.clone(),
                        source: Some(source.clone()),
                    },
                    format!("Choose empty truck points from {source}."),
                    "land:20.83",
                    ActionSpace::new(ActionSchema::Record { fields })
                        .with_context(json!({"withdrawal":row,"asset":source})),
                );
            }
        }
        Task::Air { row, quota } => {
            let (side, record) =
                air_row(content, &row).ok_or_else(|| illegal("unknown air arrival"))?;
            if side != pending.seat.side {
                return Err(illegal("not an owned air arrival"));
            }
            let choice = action
                .as_str()
                .ok_or_else(|| illegal("choose an aircraft arrival option"))?;
            if !matches!(&pending.space.schema,ActionSchema::Choice{options}if options.iter().any(|o|o.id==choice))
            {
                return Err(illegal("not an offered aircraft arrival option"));
            }
            if quota {
                let n = choice
                    .parse::<i32>()
                    .map_err(|_| illegal("choose a weekly quota"))?;
                state
                    .land
                    .arrivals
                    .air_week
                    .insert(row.clone(), (state.cursor.game_turn, n));
            } else if choice == "defer" {
                state.land.arrivals.air_deferred.insert(
                    row.clone(),
                    (state.cursor.game_turn, state.cursor.op_stage.unwrap()),
                );
            } else {
                let (plane, n) = choice
                    .rsplit_once(':')
                    .ok_or_else(|| illegal("choose an aircraft type and count"))?;
                let n = n
                    .parse::<i32>()
                    .map_err(|_| illegal("invalid aircraft count"))?;
                let remaining = state
                    .land
                    .arrivals
                    .air_remaining
                    .get_mut(&row)
                    .and_then(|r| r.get_mut(plane))
                    .ok_or_else(|| illegal("unknown aircraft type"))?;
                if n <= 0 || n > *remaining || n > state.land.arrivals.air_week[&row].1 {
                    return Err(illegal("aircraft count exceeds the weekly quota"));
                }
                *remaining -= n;
                state.land.arrivals.air_week.get_mut(&row).unwrap().1 -= n;
                let force = state
                    .air
                    .forces
                    .entry(crate::state::side_key(side).into())
                    .or_default();
                let count = force.planes.entry(plane.into()).or_default();
                count.total = count
                    .total
                    .checked_add(n)
                    .ok_or_else(|| illegal("aircraft count overflow"))?;
                note(
                    cx,
                    side,
                    format!("{n} {plane} arrived in the unassigned air-force pool (airlog:34.84)."),
                );
                if state.land.arrivals.air_remaining[&row]
                    .values()
                    .all(|n| *n == 0)
                {
                    state.land.arrivals.applied.insert(row.clone());
                }
            }
            let _ = record;
        }
    }
    state.land.arrivals.tasks.remove(&pending.id);
    let _ = strict;
    if pending.kind == AIR {
        air_start(content, state, cx).map_err(Rejection::Engine)?;
    }
    Ok("Order-of-battle choice recorded.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Cna,
        seq::{Block, OPSTAGE},
    };
    use cna_core::{
        decision::DecisionResponse,
        dice::CampaignRng,
        engine::{Command, Game, Ruleset, evaluate},
        visibility::Perspective,
    };
    fn fixture() -> (CnaContent, Game<Cna>) {
        let mut c = CnaContent::load(&cna_content::repo_data_dir(), "italian_campaign").unwrap();
        c.units.schedules.retain(|s| {
            s.arrivals
                .iter()
                .any(|r| !r.units.is_empty() || r.trucks.is_some())
        });
        let mut s = State::new(&c).unwrap();
        s.setup.closed = true;
        s.setup.started = true;
        s.logistics.convoys_initialized = true;
        crate::logistics::ports::initialize(&c, &mut s);
        s.turn.player_a = Some(Side::Axis);
        s.turn.weather = Some(crate::state::WeatherState {
            kind: cna_tables::land::weather::WeatherKind::Normal,
            storm_sections: vec![],
        });
        s.logistics.dumps.clear();
        s.logistics.truck_pools.clear();
        s.land.undistributed_trucks.clear();
        s.air.forces.clear();
        for u in s.land.units.values_mut() {
            u.location = Location::NotArrived;
            u.trucks = Trucks::default();
            u.transport_trucks = Trucks::default();
        }
        (
            c,
            Game {
                state: s,
                rng: CampaignRng::from_seed([6; 32]).state(),
            },
        )
    }
    fn at(game: &mut Game<Cna>, gt: u16, opstage: u8) {
        game.state.cursor.game_turn = gt;
        game.state.cursor.op_stage = Some(opstage);
        game.state.cursor.block = Block::OpStage;
        game.state.cursor.index = OPSTAGE
            .iter()
            .position(|s| s.anchor == "opstage.convoy_arrival")
            .unwrap();
        game.state.cursor.entered = false;
        game.state.decisions.pending.clear();
        game.state.land.arrivals.tasks.clear();
    }
    fn first(schema: &ActionSchema) -> Value {
        match schema {
            ActionSchema::Choice { options } => json!(options[0].id),
            ActionSchema::Unit { among } => json!(among[0]),
            ActionSchema::Integer { max, .. } => json!(max),
            ActionSchema::Record { fields } => Value::Object(
                fields
                    .iter()
                    .map(|f| (f.name.clone(), first(&f.schema)))
                    .collect(),
            ),
            _ => panic!("unexpected schedule schema"),
        }
    }
    fn response(
        c: &CnaContent,
        game: &Game<Cna>,
        p: &Pending,
        action: Value,
    ) -> Result<Game<Cna>, Rejection> {
        evaluate(
            &Cna::dev(),
            c,
            game,
            &Command::Respond(DecisionResponse {
                decision_id: p.id.clone(),
                seat: p.seat,
                controller_epoch: 1,
                decision_revision: p.revision,
                idempotency_key: p.id.to_string(),
                action,
                public_explanation: None,
            }),
        )
        .map(|t| t.game)
    }
    fn drain(c: &CnaContent, mut game: Game<Cna>) -> Game<Cna> {
        for _ in 0..300 {
            if let Some(p) = game
                .state
                .decisions
                .pending
                .iter()
                .find(|p| game.state.land.arrivals.tasks.contains_key(&p.id))
                .cloned()
            {
                let recovered: Game<Cna> =
                    serde_json::from_value(serde_json::to_value(&game).unwrap()).unwrap();
                let action = if let Some(Task::Transport {
                    row,
                    source: Some(source),
                }) = game.state.land.arrivals.tasks.get(&p.id)
                {
                    let t = empty_trucks(c, &game.state)[source];
                    if t.light == 0 && t.heavy == 0 {
                        json!({"light":0,"medium":((game.state.land.arrivals.withdrawals[row].needed_halves+1)/2).min(i64::from(t.medium)),"heavy":0})
                    } else {
                        first(&p.space.schema)
                    }
                } else {
                    first(&p.space.schema)
                };
                let a = response(c, &game, &p, action.clone()).unwrap();
                let b = response(c, &recovered, &p, action).unwrap();
                assert_eq!(
                    serde_json::to_value(&a).unwrap(),
                    serde_json::to_value(&b).unwrap()
                );
                game = a;
                continue;
            }
            game = evaluate(&Cna::dev(), c, &game, &Command::Advance)
                .unwrap()
                .game;
            if game.state.land.arrivals.tasks.is_empty() {
                return game;
            }
        }
        panic!("schedule decisions did not finish")
    }
    fn finish_supply(c: &CnaContent, mut game: Game<Cna>) -> Game<Cna> {
        let mut controller = CampaignRng::from_seed([91; 32]);
        let mut completed = false;
        for _ in 0..16 {
            if let Some(p) = game.state.decisions.pending.first().cloned() {
                assert_eq!(p.kind, crate::logistics::arrivals::KIND);
                let request = Cna::dev()
                    .pending(c, &game.state)
                    .into_iter()
                    .find(|r| r.id == p.id)
                    .unwrap();
                let action =
                    crate::baseline::logistics_orders(c, &game.state, &request, &mut controller)
                        .unwrap();
                let recovered: Game<Cna> =
                    serde_json::from_value(serde_json::to_value(&game).unwrap()).unwrap();
                let replayed = response(c, &recovered, &p, action.clone()).unwrap();
                game = response(c, &game, &p, action).unwrap();
                assert_eq!(
                    serde_json::to_value(&game).unwrap(),
                    serde_json::to_value(replayed).unwrap()
                );
            } else {
                let mut rng = CampaignRng::from_state(&game.rng);
                Cna::dev()
                    .finish_step(
                        c,
                        &mut game.state,
                        &mut Cx {
                            rng: &mut rng,
                            events: &mut vec![],
                        },
                    )
                    .unwrap();
                game.rng = rng.state();
                if game.state.decisions.pending.is_empty()
                    && game.state.logistics.arrival_supply.done.len() == 2
                    && game.state.logistics.arrival_supply.waiting.is_empty()
                {
                    completed = true;
                    break;
                }
            }
        }
        assert!(
            completed,
            "the actual-stock arrival supply window did not finish"
        );
        game
    }

    /// Cases: land:3.6, land:20.12, land:20.84
    #[test]
    fn arrival_and_withdrawal_note_locators_are_owner_only_and_use_known_positions() {
        let (mut c, mut game) = fixture();
        let id = UnitId::new("cw.4_indian_div.5th_indian_bde_hq");
        let cairo = city_domain(&c, "cairo").unwrap()[0].clone();
        let hex = cairo.hex().unwrap().to_string();
        let mut events = vec![];
        let mut rng = CampaignRng::from_state(&game.rng);
        place(
            &mut game.state,
            &id,
            cairo,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].unit_id.as_deref(), Some(id.as_str()));
        assert_eq!(events[0].hex.as_deref(), Some(hex.as_str()));
        assert!(!events[0].visible_to(Perspective::Side(Side::Axis)));
        for schedule in &mut c.units.schedules {
            schedule.arrivals.clear();
            for row in &mut schedule.withdrawals {
                row.units.retain(|u| u.unit == id);
                for unit in &mut row.units {
                    unit.subtree = false;
                }
            }
            schedule.withdrawals.retain(|r| !r.units.is_empty());
        }
        game.state.land.units.get_mut(&id).unwrap().trucks.medium = 10;
        at(&mut game, 13, 3);
        events.clear();
        prepare_withdrawals(
            &c,
            &mut game.state,
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        assert_eq!(game.state.land.units[&id].location, Location::NotArrived);
        let departure = events
            .iter()
            .find(|e| e.unit_id.as_deref() == Some(id.as_str()))
            .unwrap();
        assert_eq!(departure.hex.as_deref(), Some(hex.as_str()));
        assert_eq!(departure.audience, Audience::Side(Side::Commonwealth));
        assert!(!departure.visible_to(Perspective::Side(Side::Axis)));
    }
    /// Cases: land:4.43, land:20.11
    #[test]
    fn subtree_exact_stage_and_hq_only_are_distinct_and_less_removes_full_trees() {
        let (c, _) = fixture();
        let root = UnitId::new("cw.4_indian_div.4th_indian_div_hq");
        let selector = ScheduledUnit {
            unit: root.clone(),
            subtree: true,
            hq_only: false,
            less: vec![],
        };
        let exact = expand(&c, std::slice::from_ref(&selector), Some((5, 1)));
        assert!(exact.contains(&root));
        assert!(exact.iter().all(
            |id| id == &root || c.units.units[id].arrives == Arrival::At { gt: 5, opstage: 1 }
        ));
        let hq = expand(
            &c,
            &[ScheduledUnit {
                hq_only: true,
                ..selector.clone()
            }],
            Some((5, 1)),
        );
        assert_eq!(hq, BTreeSet::from([root.clone()]));
        let removed = UnitId::new("cw.4_indian_div.5th_indian_bde_hq");
        let less = expand(
            &c,
            &[ScheduledUnit {
                less: vec![removed.clone()],
                ..selector
            }],
            None,
        );
        assert!(less.contains(&root));
        assert!(less.is_disjoint(&descendants(&c, &removed)));
    }
    /// Cases: land:20.11, land:20.12, land:20.14, land:3.62
    #[test]
    fn a_real_arrival_uses_its_exact_stage_and_recovery_preserves_private_choices() {
        let (c, mut game) = fixture();
        let id = UnitId::new("cw.polish_bde.polish_brigade_hq");
        at(&mut game, 1, 2);
        let earlier = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
            .unwrap()
            .game;
        assert!(matches!(
            earlier.state.land.units[&id].location,
            Location::NotArrived
        ));
        let mut game = earlier;
        at(&mut game, 1, 3);
        game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
            .unwrap()
            .game;
        assert!(game.state.decisions.pending.iter().any(|p| p.kind == PLACE));
        let game = drain(&c, game);
        let u = &game.state.land.units[&id];
        assert!(in_withdrawal_city(&c, &u.location));
        assert_eq!(u.cp_spent_quarters, 0);
        let enemy = Cna::dev().view(&c, &game.state, Perspective::Side(Side::Axis));
        assert!(!enemy.units.contains_key(id.as_str()));
        for stack in enemy.stacks.iter().filter(|s| s.side == Side::Commonwealth) {
            assert!(stack.unit_ids.is_empty());
            assert_eq!(stack.visible_count, None);
        }
        assert!(game.state.land.arrivals.supply_finished.contains("1:3"));
    }
    /// Cases: land:4.43, land:20.12, airlog:53.11
    #[test]
    fn row_trucks_attach_exactly_once_and_alone_trucks_get_fresh_pool_ids() {
        let (mut c, mut game) = fixture();
        at(&mut game, 5, 1);
        game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
            .unwrap()
            .game;
        let game = drain(&c, game);
        let points: Trucks =
            game.state
                .land
                .units
                .values()
                .fold(Trucks::default(), |mut total, u| {
                    total.light += u.trucks.light;
                    total.medium += u.trucks.medium;
                    total.heavy += u.trucks.heavy;
                    total
                });
        assert_eq!(
            points,
            Trucks {
                light: 5,
                medium: 20,
                heavy: 0
            }
        );
        let source = c
            .units
            .schedules
            .iter_mut()
            .find(|s| s.file.side == Side::Commonwealth)
            .unwrap();
        let row = source
            .arrivals
            .iter_mut()
            .find(|r| r.gt == Some(5) && r.opstage == Some(1))
            .unwrap();
        row.alone = true;
        row.units.clear();
        let (_, mut isolated) = fixture();
        at(&mut isolated, 5, 1);
        isolated = evaluate(&Cna::dev(), &c, &isolated, &Command::Advance)
            .unwrap()
            .game;
        let isolated = drain(&c, isolated);
        assert_eq!(isolated.state.logistics.truck_pools.len(), 1);
        let pool = &isolated.state.logistics.truck_pools[0];
        assert_eq!(pool.trucks, points);
        assert!(pool.location.is_some());
        assert!(isolated.state.logistics.truck_pool_ids.contains(&pool.id));
    }
    /// Cases: land:20.81, land:20.83, land:20.84, land:4.43
    #[test]
    fn real_named_withdrawal_removes_present_units_and_permanently_eliminates_late_units() {
        let (c, mut game) = fixture();
        let root = UnitId::new("cw.4_indian_div.5th_indian_bde_hq");
        let units = descendants(&c, &root);
        let cairo = city_domain(&c, "cairo").unwrap()[0].clone();
        for id in &units {
            let u = game.state.land.units.get_mut(id).unwrap();
            u.location = cairo.clone();
            u.toe = Some(cna_content::units::Toe::Normal(
                cna_content::units::NormalToe::N,
            ));
        }
        game.state.land.units.get_mut(&root).unwrap().trucks.medium = 10;
        let late = units.iter().find(|id| *id != &root).unwrap().clone();
        game.state.land.units.get_mut(&late).unwrap().location = Location::Hex {
            hex: "A0101".into(),
        };
        at(&mut game, 13, 3);
        game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
            .unwrap()
            .game;
        let game = drain(&c, game);
        assert_eq!(game.state.land.units[&late].location, Location::Eliminated);
        assert!(!game.state.land.arrivals.withdrawn_units.contains(&late));
        for id in units.into_iter().filter(|id| *id != late) {
            assert_eq!(game.state.land.units[&id].location, Location::NotArrived);
            assert!(game.state.land.arrivals.withdrawn_units.contains(&id));
        }
        assert!(game.state.land.arrivals.supply_finished.contains("13:3"));
    }
    /// Cases: land:20.82, land:20.85, land:3.33, land:3.34
    #[test]
    fn weak_named_counter_offers_only_equivalent_three_quarter_substitutes() {
        let (c, mut game) = fixture();
        let root = UnitId::new("cw.4_indian_div.5th_indian_bde_hq");
        let units = descendants(&c, &root);
        let cairo = city_domain(&c, "cairo").unwrap()[0].clone();
        for id in &units {
            let u = game.state.land.units.get_mut(id).unwrap();
            u.location = cairo.clone();
            u.toe = Some(cna_content::units::Toe::Normal(
                cna_content::units::NormalToe::N,
            ));
        }
        assert!(ready_for_withdrawal(&c, &game.state, &root));
        let weak = UnitId::new("cw.4_indian_div.1st_royal_fusiliers");
        game.state.land.units.get_mut(&weak).unwrap().toe =
            Some(cna_content::units::Toe::Under { under: 4 });
        let candidate = UnitId::new("cw.2_nz_div.18th_nz_bn");
        let u = game.state.land.units.get_mut(&candidate).unwrap();
        u.location = cairo;
        u.toe = Some(cna_content::units::Toe::Under { under: 5 });
        game.state.land.units.get_mut(&root).unwrap().trucks.medium = 10;
        at(&mut game, 13, 3);
        game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
            .unwrap()
            .game;
        let p = game
            .state
            .decisions
            .pending
            .iter()
            .find(|p| p.kind == SUBSTITUTE)
            .unwrap()
            .clone();
        assert!(
            matches!(&p.space.schema,ActionSchema::Choice{options}if options.iter().any(|o|o.id==candidate.as_str()))
        );
        let mut below = game.clone();
        below.state.land.units.get_mut(&candidate).unwrap().toe =
            Some(cna_content::units::Toe::Under { under: 4 });
        assert!(response(&c, &below, &p, json!(candidate)).is_err());
        game = response(&c, &game, &p, json!(candidate)).unwrap();
        let game = drain(&c, game);
        assert!(in_withdrawal_city(
            &c,
            &game.state.land.units[&weak].location
        ));
        assert!(
            game.state
                .land
                .arrivals
                .withdrawn_units
                .contains(&candidate)
        );
    }
    /// Cases: land:4.43, land:20.83
    /// Interpretations: interp:scen-0006
    #[test]
    fn truck_shortfall_is_a_private_dev_note_and_full_stop_without_eliminating_eligible_units() {
        let (mut c, mut game) = fixture();
        for schedule in &mut c.units.schedules {
            schedule.arrivals.clear();
        }
        let root = UnitId::new("cw.4_indian_div.5th_indian_bde_hq");
        let units = descendants(&c, &root);
        let cairo = city_domain(&c, "cairo").unwrap()[0].clone();
        for id in &units {
            let u = game.state.land.units.get_mut(id).unwrap();
            u.location = cairo.clone();
            u.toe = Some(cna_content::units::Toe::Normal(
                cna_content::units::NormalToe::N,
            ));
        }
        crate::logistics::pools::add_truck_pool(
            &mut game.state.logistics,
            None,
            Side::Commonwealth,
            Placement::City {
                city: "cairo".into(),
            },
            Some(cairo),
            Trucks {
                light: 0,
                medium: 1,
                heavy: 0,
            },
            Supplies::default(),
        )
        .unwrap();
        at(&mut game, 13, 3);
        let before = game.clone();
        game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
            .unwrap()
            .game;
        game = drain(&c, game);
        assert!(
            units
                .iter()
                .all(|u| game.state.land.arrivals.withdrawn_units.contains(u))
        );
        assert_eq!(
            game.state.logistics.truck_pools[0].trucks,
            Trucks::default()
        );
        let error = evaluate(&Cna::full(), &c, &before, &Command::Advance).unwrap_err();
        assert!(
            matches!(&error,Rejection::Engine(EngineError::Unsupported{case,..})if case=="land:20.83"),
            "{error:?}"
        );
    }
    /// Cases: airlog:34.84
    #[test]
    fn monthly_aircraft_balance_total_counts_while_owner_chooses_types_and_stages() {
        let (mut c, mut game) = fixture();
        let all = CnaContent::load(&cna_content::repo_data_dir(), "italian_campaign").unwrap();
        let mut schedule = all
            .units
            .schedules
            .iter()
            .find(|s| {
                s.file.side == Side::Commonwealth
                    && s.arrivals.iter().any(|r| r.gt_from == Some(15))
            })
            .unwrap()
            .clone();
        schedule.arrivals.retain(|r| r.gt_from == Some(15));
        schedule.withdrawals.clear();
        c.units.schedules = vec![schedule];
        let totals: BTreeMap<_, _> = c.units.schedules[0].arrivals[0]
            .planes
            .iter()
            .map(|p| (p.aircraft.clone(), p.n))
            .collect();
        let mut weekly = Vec::new();
        for gt in 15..=18 {
            at(&mut game, gt, 1);
            let old: i32 = game
                .state
                .air
                .forces
                .values()
                .flat_map(|f| f.planes.values())
                .map(|p| p.total)
                .sum();
            game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
                .unwrap()
                .game;
            // On the first week, the owner may fill the entire quota with Hurricanes.
            if gt == 15 {
                let p = game
                    .state
                    .decisions
                    .pending
                    .iter()
                    .find(|p| p.kind == AIR)
                    .unwrap()
                    .clone();
                game = response(&c, &game, &p, first(&p.space.schema)).unwrap();
                let p = game
                    .state
                    .decisions
                    .pending
                    .iter()
                    .find(|p| p.kind == AIR)
                    .unwrap()
                    .clone();
                game = response(&c, &game, &p, json!("cw.hurricane_i:15")).unwrap();
            }
            game = drain(&c, game);
            let new: i32 = game
                .state
                .air
                .forces
                .values()
                .flat_map(|f| f.planes.values())
                .map(|p| p.total)
                .sum();
            weekly.push(new - old);
        }
        assert_eq!(weekly, vec![15, 14, 14, 14]);
        for (plane, n) in totals {
            assert_eq!(
                game.state.air.forces["commonwealth"].planes[&plane].total,
                n
            );
        }
        assert!(
            game.state.air.forces["commonwealth"]
                .planes
                .values()
                .all(|p| p.ready == 0 && p.armed == 0 && p.fuelled == 0)
        );
    }
    /// Cases: land:20.11, land:20.12, land:20.14, land:20.15, land:4.43
    #[test]
    fn all_126_scheduled_land_counters_arrive_at_their_exact_printed_stage() {
        let (mut c, mut game) = fixture();
        for schedule in &mut c.units.schedules {
            schedule.withdrawals.clear();
        }
        let expected: BTreeSet<_> = c
            .units
            .units
            .values()
            .filter(|u| matches!(u.arrives,Arrival::At{gt,opstage:_} if gt<=20))
            .map(|u| u.id.clone())
            .collect();
        assert_eq!(expected.len(), 126);
        for u in game.state.land.units.values_mut() {
            if c.units.units[&u.id].arrives.is_deployed() {
                u.location = Location::AwaitingSetup {
                    group: "already-present-fixture".into(),
                };
            }
        }
        for gt in 1..=20 {
            for opstage in 1..=3 {
                at(&mut game, gt, opstage);
                game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
                    .unwrap()
                    .game;
                game = drain(&c, game);
                for id in &expected {
                    let Arrival::At { gt: g, opstage: o } = c.units.units[id].arrives else {
                        unreachable!()
                    };
                    assert_eq!(
                        game.state.land.units[id].location != Location::NotArrived,
                        (g, o) <= (gt, opstage),
                        "{id} at {gt}/{opstage}"
                    );
                }
                // Redeploy each arrival cohort before the next window, so this schedule
                // test does not fill every Cairo receiving hex with an unmoving army.
                for id in &expected {
                    let u = game.state.land.units.get_mut(id).unwrap();
                    if u.location.hex().is_some() {
                        u.location = Location::Hex {
                            hex: "C3418".into(),
                        };
                    }
                }
            }
        }
        let arrived: BTreeSet<_> = game
            .state
            .land
            .arrivals
            .newly_arrived
            .values()
            .flat_map(|ids| ids.iter().cloned())
            .collect();
        assert!(
            arrived == expected,
            "unexpected={:?}; missing={:?}",
            arrived.difference(&expected).collect::<Vec<_>>(),
            expected.difference(&arrived).collect::<Vec<_>>()
        );
        assert!(
            expected
                .iter()
                .all(|id| game.state.land.units[id].cp_spent_quarters == 0)
        );
        let printed = c
            .units
            .schedules
            .iter()
            .flat_map(|s| s.arrivals.iter().filter_map(|r| r.trucks))
            .fold(Trucks::default(), |mut all, t| {
                all.light += t.light;
                all.medium += t.medium;
                all.heavy += t.heavy;
                all
            });
        let actual = game
            .state
            .land
            .units
            .values()
            .map(|u| u.trucks)
            .chain(game.state.logistics.truck_pools.iter().map(|p| p.trucks))
            .fold(Trucks::default(), |mut all, t| {
                all.light += t.light;
                all.medium += t.medium;
                all.heavy += t.heavy;
                all
            });
        assert_eq!(actual, printed);
    }
    /// Cases: land:20.81, land:20.83, land:20.84, land:4.43
    #[test]
    fn all_17_named_withdrawals_resolve_in_the_three_deadline_stages() {
        let (mut c, mut game) = fixture();
        let selectors: Vec<_> = c
            .units
            .schedules
            .iter()
            .flat_map(|s| s.withdrawals.iter().flat_map(|r| r.units.iter().cloned()))
            .collect();
        let expected = expand(&c, &selectors, None);
        assert_eq!(expected.len(), 17);
        let cairo = city_domain(&c, "cairo").unwrap()[0].clone();
        for id in &expected {
            let u = game.state.land.units.get_mut(id).unwrap();
            u.location = cairo.clone();
            u.toe = Some(cna_content::units::Toe::Normal(
                cna_content::units::NormalToe::N,
            ));
        }
        for schedule in &mut c.units.schedules {
            schedule.arrivals.clear();
        }
        crate::logistics::pools::add_truck_pool(
            &mut game.state.logistics,
            None,
            Side::Commonwealth,
            Placement::City {
                city: "cairo".into(),
            },
            Some(cairo),
            Trucks {
                light: 0,
                medium: 49,
                heavy: 0,
            },
            Supplies::default(),
        )
        .unwrap();
        let mut prior = 0;
        for (gt, opstage) in [(13, 3), (14, 3), (15, 2)] {
            at(&mut game, gt, opstage);
            game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
                .unwrap()
                .game;
            game = drain(&c, game);
            assert!(game.state.land.arrivals.withdrawn_units.len() > prior);
            prior = game.state.land.arrivals.withdrawn_units.len();
        }
        assert_eq!(game.state.land.arrivals.withdrawn_units, expected);
        assert_eq!(
            game.state.logistics.truck_pools[0].trucks,
            Trucks::default()
        );
    }
    /// Cases: airlog:34.84
    #[test]
    fn owner_can_defer_a_weekly_quota_but_must_complete_it_in_opstage_three() {
        let (mut c, mut game) = fixture();
        let all = CnaContent::load(&cna_content::repo_data_dir(), "italian_campaign").unwrap();
        let mut schedule = all
            .units
            .schedules
            .iter()
            .find(|s| {
                s.file.side == Side::Commonwealth
                    && s.arrivals.iter().any(|r| r.gt_from == Some(15))
            })
            .unwrap()
            .clone();
        schedule.arrivals.retain(|r| r.gt_from == Some(15));
        schedule.withdrawals.clear();
        c.units.schedules = vec![schedule];
        for op in 1..=3 {
            at(&mut game, 15, op);
            game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
                .unwrap()
                .game;
            if op == 1 {
                let p = game
                    .state
                    .decisions
                    .pending
                    .iter()
                    .find(|p| p.kind == AIR)
                    .unwrap()
                    .clone();
                game = response(&c, &game, &p, first(&p.space.schema)).unwrap();
            }
            let p = game
                .state
                .decisions
                .pending
                .iter()
                .find(|p| p.kind == AIR)
                .unwrap()
                .clone();
            if op < 3 {
                game = response(&c, &game, &p, json!("defer")).unwrap();
                game = drain(&c, game);
                assert!(game.state.air.forces.is_empty());
            } else {
                assert!(response(&c, &game, &p, json!("defer")).is_err());
                game = drain(&c, game);
            }
        }
        assert_eq!(
            game.state.air.forces["commonwealth"]
                .planes
                .values()
                .map(|p| p.total)
                .sum::<i32>(),
            15
        );
    }

    /// Cases: land:20.12, land:20.14, land:9.12
    #[test]
    fn a_capacity_delayed_counter_remains_pending_for_a_later_receiving_window() {
        let (mut c, mut game) = fixture();
        let id = UnitId::new("cw.2_nz_div.24th_nz_bn");
        let city = c.areas.areas.get_mut("cairo").unwrap();
        city.hex_ids = vec!["E1730".into()];
        let blockers: Vec<_> = c
            .units
            .units
            .values()
            .filter(|u| {
                u.side == Side::Commonwealth
                    && u.stacking_points == Some(1)
                    && u.id != id
                    && crate::land::formation::class(&c, &u.id)
                        .is_some_and(|cl| cl.unit_type == "infantry")
            })
            .take(8)
            .map(|u| u.id.clone())
            .collect();
        assert_eq!(blockers.len(), 8);
        for blocker in &blockers {
            let u = game.state.land.units.get_mut(blocker).unwrap();
            u.location = Location::Hex {
                hex: "E1730".into(),
            };
            u.detached = true;
            u.toe = Some(cna_content::units::Toe::Normal(
                cna_content::units::NormalToe::N,
            ));
        }
        for schedule in &mut c.units.schedules {
            schedule
                .arrivals
                .retain(|r| r.gt == Some(6) && r.opstage == Some(3));
            schedule.withdrawals.clear();
            for row in &mut schedule.arrivals {
                row.units = vec![ScheduledUnit {
                    unit: id.clone(),
                    subtree: false,
                    hq_only: false,
                    less: vec![],
                }];
                row.trucks = None;
            }
        }
        at(&mut game, 6, 3);
        game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
            .unwrap()
            .game;
        game = drain(&c, game);
        assert!(
            matches!(&game.state.land.units[&id].location,Location::AwaitingSetup{group}if group.starts_with("arrival:")),
            "{:?}",
            game.state.land.units[&id].location
        );
        assert!(
            game.state
                .land
                .arrivals
                .newly_arrived
                .values()
                .all(|ids| !ids.contains(&id))
        );
        for blocker in blockers {
            game.state.land.units.get_mut(&blocker).unwrap().location = Location::Hex {
                hex: "C3418".into(),
            };
        }
        at(&mut game, 7, 1);
        game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
            .unwrap()
            .game;
        assert!(game.state.decisions.pending.iter().any(|p| p.kind == PLACE));
        game = drain(&c, game);
        assert_eq!(
            game.state.land.units[&id].location,
            Location::Hex {
                hex: "E1730".into()
            }
        );
        assert!(game.state.land.arrivals.newly_arrived["7:1"].contains(&id));
    }
    /// Cases: land:4.43, airlog:53.11
    #[test]
    fn truck_answer_rejects_wrong_recipients_zero_and_wide_overdraw_without_mutation() {
        let (c, mut game) = fixture();
        at(&mut game, 5, 1);
        game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
            .unwrap()
            .game;
        for _ in 0..80 {
            if game
                .state
                .decisions
                .pending
                .iter()
                .any(|p| p.kind == TRUCKS)
            {
                break;
            }
            if let Some(p) = game.state.decisions.pending.first().cloned() {
                game = response(&c, &game, &p, first(&p.space.schema)).unwrap();
            } else {
                game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
                    .unwrap()
                    .game;
            }
        }
        let p = game
            .state
            .decisions
            .pending
            .iter()
            .find(|p| p.kind == TRUCKS)
            .unwrap()
            .clone();
        let row = match &game.state.land.arrivals.tasks[&p.id] {
            Task::Trucks { row } => row,
            _ => unreachable!(),
        };
        let unit = game.state.land.arrivals.batches[row].units[0].clone();
        let before = serde_json::to_value(&game).unwrap();
        for action in [
            json!({"unit":"cw.polish_bde.polish_brigade_hq","light":1,"medium":0,"heavy":0}),
            json!({"unit":unit,"light":0,"medium":0,"heavy":0}),
            json!({"unit":unit,"light":i64::MAX,"medium":0,"heavy":0}),
        ] {
            assert!(response(&c, &game, &p, action).is_err());
            assert_eq!(serde_json::to_value(&game).unwrap(), before);
        }
    }
    /// Cases: land:20.83, airlog:49.13, airlog:49.16, airlog:52.42
    #[test]
    fn empty_truck_withdrawal_retires_cohorts_without_refunding_paid_body_history() {
        use cna_content::units::{Toe, WeaponPoints};
        use cna_core::quantity::WaterPoints;
        let (c, mut game) = fixture();
        game.state.cursor.op_stage = Some(1);
        game.state.turn.weather = Some(crate::state::WeatherState {
            kind: cna_tables::land::weather::WeatherKind::Normal,
            storm_sections: vec![],
        });
        let id = UnitId::new("cw.4_indian_div.1st_royal_fusiliers");
        let location = city_domain(&c, "cairo").unwrap()[0].clone();
        let u = game.state.land.units.get_mut(&id).unwrap();
        u.location = location.clone();
        u.toe = Some(Toe::Weapons(vec![WeaponPoints {
            weapon: "it.cv33".into(),
            n: 1,
        }]));
        u.trucks.light = 3;
        game.state.logistics.dumps.insert(
            "funding".into(),
            crate::state::Dump {
                marker: "funding-marker".into(),
                id: "funding".into(),
                side: Side::Commonwealth,
                location: crate::state::DumpLocation::Hex {
                    hex: location.hex().unwrap().clone(),
                },
                supplies: Supplies {
                    fuel: 10,
                    ..Supplies::default()
                },
                active: true,
                dummy: false,
            },
        );
        crate::logistics::spend_segment_fuel(&c, &mut game.state, &id, 4).unwrap();
        game.state
            .logistics
            .unit_supply
            .entry(id.clone())
            .or_default()
            .activity_water = WaterPoints::new(100);
        crate::logistics::spend_activity_water(&c, &mut game.state, &id).unwrap();
        let paid_fuel = game.state.logistics.fuel_accounts[&id].paid_cost;
        let water = game.state.logistics.rations[&id]
            .activity_water_ledger
            .clone()
            .unwrap();
        remove_empty_trucks(
            &c,
            &mut game.state,
            &format!("unit:{id}"),
            Trucks {
                light: 1,
                ..Trucks::default()
            },
            false,
            &mut Cx {
                rng: &mut CampaignRng::from_state(&game.rng),
                events: &mut vec![],
            },
        )
        .unwrap();
        let stock = game.state.logistics.dumps["funding"].supplies;
        assert_eq!(game.state.land.units[&id].trucks.light, 2);
        assert_eq!(game.state.logistics.fuel_accounts[&id].paid_cost, paid_fuel);
        assert_eq!(
            game.state.logistics.fuel_segments[&id]
                .cohorts
                .iter()
                .map(|c| c.count)
                .sum::<i32>(),
            2
        );
        let after = game.state.logistics.rations[&id]
            .activity_water_ledger
            .as_ref()
            .unwrap();
        assert_eq!(after.body_paid, water.body_paid);
        assert_eq!(after.truck_required.light, water.truck_required.light - 1);
        assert_eq!(after.truck_paid.light, water.truck_paid.light - 1);
        assert_eq!(
            crate::logistics::activity_water_due(&c, &game.state, &id).unwrap(),
            0
        );
        crate::logistics::spend_segment_fuel(&c, &mut game.state, &id, 4).unwrap();
        assert_eq!(game.state.logistics.dumps["funding"].supplies, stock);
        crate::logistics::spend_segment_fuel(&c, &mut game.state, &id, 8).unwrap();
    }
    /// Cases: land:20.83, airlog:49.13, airlog:49.16
    #[test]
    fn empty_pool_withdrawal_retires_exact_kinds_and_keeps_paid_fuel_through_recovery() {
        use cna_core::quantity::{FuelTenths, WaterPoints};
        let (c, mut game) = fixture();
        game.state.cursor.op_stage = Some(1);
        let location = city_domain(&c, "cairo").unwrap()[0].clone();
        let id = crate::logistics::pools::add_truck_pool(
            &mut game.state.logistics,
            None,
            Side::Commonwealth,
            Placement::City {
                city: "cairo".into(),
            },
            Some(location.clone()),
            Trucks {
                light: 1,
                medium: 2,
                heavy: 1,
            },
            Supplies::default(),
        )
        .unwrap();
        game.state.logistics.dumps.insert(
            "pool-funding".into(),
            crate::state::Dump {
                marker: "pool-funding-marker".into(),
                id: "pool-funding".into(),
                side: Side::Commonwealth,
                location: crate::state::DumpLocation::Hex {
                    hex: location.hex().unwrap().clone(),
                },
                supplies: Supplies {
                    fuel: 10,
                    ..Supplies::default()
                },
                active: true,
                dummy: false,
            },
        );
        crate::logistics::pool_fuel::spend_pool_segment_fuel(&c, &mut game.state, &id, 16).unwrap();
        let paid = game.state.logistics.pool_fuel_accounts[&id].clone();
        let stock = game.state.logistics.dumps["pool-funding"].supplies;
        let key = format!("pool:{id}");
        assert!(empty_trucks(&c, &game.state).contains_key(&key));
        let pool = game
            .state
            .logistics
            .truck_pools
            .iter_mut()
            .find(|p| p.id == id)
            .unwrap();
        pool.tank_fuel = FuelTenths::new(1);
        assert!(!empty_trucks(&c, &game.state).contains_key(&key));
        let pool = game
            .state
            .logistics
            .truck_pools
            .iter_mut()
            .find(|p| p.id == id)
            .unwrap();
        pool.tank_fuel = FuelTenths::ZERO;
        pool.activity_water = WaterPoints::new(1);
        assert!(!empty_trucks(&c, &game.state).contains_key(&key));
        game.state
            .logistics
            .truck_pools
            .iter_mut()
            .find(|p| p.id == id)
            .unwrap()
            .activity_water = WaterPoints::ZERO;
        remove_empty_trucks(
            &c,
            &mut game.state,
            &key,
            Trucks {
                medium: 1,
                ..Trucks::default()
            },
            false,
            &mut Cx {
                rng: &mut CampaignRng::from_state(&game.rng),
                events: &mut vec![],
            },
        )
        .unwrap();
        assert_eq!(game.state.logistics.pool_fuel_accounts[&id], paid);
        assert_eq!(game.state.logistics.dumps["pool-funding"].supplies, stock);
        let mut remaining = Trucks::default();
        for cohort in
            crate::logistics::pool_fuel::pool_segment_fuel_cohorts(&game.state, &id).unwrap()
        {
            match cohort.kind {
                crate::logistics::FuelTruckKind::Light => remaining.light += cohort.count,
                crate::logistics::FuelTruckKind::Medium => remaining.medium += cohort.count,
                crate::logistics::FuelTruckKind::Heavy => remaining.heavy += cohort.count,
            }
        }
        assert_eq!(
            remaining,
            Trucks {
                light: 1,
                medium: 1,
                heavy: 1
            }
        );
        let mut recovered: State =
            serde_json::from_value(serde_json::to_value(&game.state).unwrap()).unwrap();
        crate::logistics::pool_fuel::spend_pool_segment_fuel(&c, &mut game.state, &id, 16).unwrap();
        assert_eq!(game.state.logistics.dumps["pool-funding"].supplies, stock);
        crate::logistics::pool_fuel::spend_pool_segment_fuel(&c, &mut recovered, &id, 16).unwrap();
        assert_eq!(
            serde_json::to_value(&game.state).unwrap(),
            serde_json::to_value(&recovered).unwrap()
        );
        crate::logistics::pool_fuel::spend_pool_segment_fuel(&c, &mut game.state, &id, 20).unwrap();
        crate::logistics::pool_fuel::spend_pool_segment_fuel(&c, &mut recovered, &id, 20).unwrap();
        assert_eq!(
            serde_json::to_value(&game.state).unwrap(),
            serde_json::to_value(&recovered).unwrap()
        );
    }
    /// Cases: land:8.88, land:20.83, land:3.6
    /// Interpretations: interp:airlog-0019
    #[test]
    fn stamped_withdrawal_trucks_stop_in_full_and_keep_private_history_in_dev() {
        use crate::logistics::box_handling::BoxHandling;
        let (c, mut game) = fixture();
        at(&mut game, 13, 3);
        let stamp = BoxHandling {
            stage: crate::logistics::water::WaterStage::current(&game.state),
            loaded: Supplies {
                stores: 1,
                ..Default::default()
            },
            unloaded: Supplies::default(),
        };
        let cairo = city_domain(&c, "cairo").unwrap()[0].clone();
        let pool = crate::logistics::pools::add_truck_pool(
            &mut game.state.logistics,
            None,
            Side::Commonwealth,
            Placement::City {
                city: "cairo".into(),
            },
            Some(cairo),
            Trucks {
                medium: 3,
                ..Default::default()
            },
            Supplies::default(),
        )
        .unwrap();
        game.state.logistics.truck_pools[0].box_handling = Some(stamp.clone());
        let before = serde_json::to_value(&game.state).unwrap();
        let mut rng = CampaignRng::from_state(&game.rng);
        let mut events = vec![];
        let amount = Trucks {
            medium: 1,
            ..Default::default()
        };
        let result = remove_empty_trucks(
            &c,
            &mut game.state,
            &format!("pool:{pool}"),
            amount,
            true,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        );
        assert!(
            matches!(result, Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "land:8.88")
        );
        assert_eq!(serde_json::to_value(&game.state).unwrap(), before);
        assert!(events.is_empty());
        remove_empty_trucks(
            &c,
            &mut game.state,
            &format!("pool:{pool}"),
            amount,
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        assert_eq!(game.state.logistics.truck_pools[0].trucks.medium, 2);
        assert_eq!(
            game.state.logistics.truck_pools[0].box_handling,
            Some(stamp)
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].audience, Audience::Side(Side::Commonwealth));
        assert!(matches!(events[0].event, GameEvent::Note { .. }));
    }
    /// Cases: land:20.85, land:3.33, land:3.34
    /// Interpretations: interp:scen-0006
    #[test]
    fn artillery_hq_substitutes_never_include_tank_or_unarmed_hq_classes() {
        use cna_content::units::{NormalToe, Toe};
        let (mut c, mut game) = fixture();
        let by_class = |class: &str| -> Vec<UnitId> {
            c.units
                .units
                .values()
                .filter(|u| u.class.as_deref() == Some(class))
                .map(|u| u.id.clone())
                .collect()
        };
        let artillery = by_class("cw.c");
        assert!(artillery.len() >= 2);
        let named = artillery[0].clone();
        let equivalent = artillery[1].clone();
        let tank = by_class("cw.b")[0].clone();
        let unarmed = by_class("cw.a")[0].clone();
        // Keep the printed classes, while equalizing echelon to isolate the class constraint.
        let echelon = c.units.units[&named].echelon.clone();
        let cairo = city_domain(&c, "cairo").unwrap()[0].clone();
        for id in [&named, &equivalent, &tank, &unarmed] {
            c.units.units.get_mut(id).unwrap().echelon = echelon.clone();
            let u = game.state.land.units.get_mut(id).unwrap();
            u.location = cairo.clone();
            u.toe = Some(Toe::Normal(NormalToe::N));
        }
        game.state.land.units.get_mut(&named).unwrap().toe = Some(Toe::Under { under: 2 });
        assert!(!ready_for_withdrawal(&c, &game.state, &named));
        assert!(ready_for_withdrawal(&c, &game.state, &tank));
        assert!(ready_for_withdrawal(&c, &game.state, &unarmed));
        game.state
            .land
            .arrivals
            .withdrawals
            .insert("test-row".into(), Withdrawal::default());
        let offered = substitutes(&c, &game.state, &named, "test-row");
        assert!(offered.contains(&equivalent));
        assert!(!offered.contains(&tank));
        assert!(!offered.contains(&unarmed));
    }

    /// Cases: land:20.12, airlog:51.11, airlog:52.13, airlog:56.28, land:3.6
    /// Interpretations: interp:airlog-0017
    #[test]
    fn dispatcher_opens_actual_stock_supply_only_after_land_choices_and_preserves_old_units() {
        let (c, mut game) = fixture();
        let cairo = city_domain(&c, "cairo").unwrap()[0].clone();
        let old = UnitId::new("cw.2_nz_div.18th_nz_bn");
        game.state.land.units.get_mut(&old).unwrap().location = cairo.clone();
        let old_unit = game.state.land.units[&old].clone();
        let old_rations = game.state.logistics.rations.get(&old).cloned();
        let old_supply = game.state.logistics.unit_supply.get(&old).cloned();
        let hex = cairo.hex().unwrap().clone();
        game.state.logistics.dumps.insert(
            "arrival-stock".into(),
            crate::state::Dump {
                id: "arrival-stock".into(),
                marker: "dump-stock".into(),
                side: Side::Commonwealth,
                location: crate::state::DumpLocation::Hex { hex },
                supplies: Supplies {
                    stores: 1000,
                    water: 1000,
                    fuel: 0,
                    ..Default::default()
                },
                active: true,
                dummy: false,
            },
        );
        at(&mut game, 1, 3);
        game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
            .unwrap()
            .game;
        assert!(game.state.decisions.pending.iter().any(|p| p.kind == PLACE));
        assert!(game.state.logistics.arrival_supply.stage.is_none());
        game = drain(&c, game);
        let arrived = game.state.land.arrivals.newly_arrived["1:3"].clone();
        let infantry = UnitId::new("cw.polish_bde.1st_polish_bn");
        assert!(arrived.contains(&infantry));
        assert!(!arrived.contains(&old));
        assert_eq!(game.state.logistics.arrival_supply.units, arrived);
        assert!(game.state.land.arrivals.supply_finished.contains("1:3"));
        assert_eq!(game.state.cursor.anchor(), "opstage.convoy_arrival");
        assert!(
            game.state
                .decisions
                .pending
                .iter()
                .any(|p| p.kind == crate::logistics::arrivals::KIND)
        );
        assert_eq!(
            game.state.logistics.dumps["arrival-stock"].supplies.stores,
            1000
        );
        assert_eq!(
            game.state.logistics.dumps["arrival-stock"].supplies.water,
            1000
        );
        assert_eq!(game.state.land.units[&infantry].cp_spent_quarters, 0);
        game = finish_supply(&c, game);
        let ration = &game.state.logistics.rations[&infantry];
        assert!(ration.stores_required > 0);
        assert_eq!(ration.stores_received, ration.stores_required);
        assert_eq!(
            game.state.logistics.rations[&infantry].infantry_water_received,
            1
        );
        let consumed: i32 = arrived
            .iter()
            .filter_map(|id| game.state.logistics.rations.get(id))
            .map(|r| r.stores_received)
            .sum();
        assert!(consumed > 0);
        assert_eq!(
            game.state.logistics.dumps["arrival-stock"].supplies.stores,
            1000 - consumed
        );
        assert_eq!(game.state.land.units[&old], old_unit);
        assert_eq!(game.state.logistics.rations.get(&old).cloned(), old_rations);
        assert_eq!(
            game.state.logistics.unit_supply.get(&old).cloned(),
            old_supply
        );
        let snapshot = serde_json::to_value(&game).unwrap();
        let mut rng = CampaignRng::from_state(&game.rng);
        Cna::dev()
            .finish_step(
                &c,
                &mut game.state,
                &mut Cx {
                    rng: &mut rng,
                    events: &mut vec![],
                },
            )
            .unwrap();
        game.rng = rng.state();
        assert_eq!(serde_json::to_value(&game).unwrap(), snapshot);
    }

    /// Cases: land:20.12, airlog:56.28, airlog:51.11, airlog:52.13
    /// Interpretations: interp:airlog-0017
    #[test]
    fn same_stage_convoy_stock_is_available_in_the_new_unit_supply_window() {
        use crate::logistics::convoys::{ConvoyStatus, ConvoyTurn, NavalConvoy};
        let (mut c, mut game) = fixture();
        let id = UnitId::new("it.unassigned_blackshirt.140th_ccnn_bn");
        for schedule in &mut c.units.schedules {
            schedule
                .arrivals
                .retain(|r| r.gt == Some(13) && r.opstage == Some(3));
            for row in &mut schedule.arrivals {
                row.units.retain(|u| u.unit == id);
            }
            schedule.arrivals.retain(|r| !r.units.is_empty());
            schedule.withdrawals.clear();
        }
        let cargo = Supplies {
            stores: 200,
            ..Default::default()
        };
        game.state.logistics.convoy_turns.insert(
            13,
            ConvoyTurn {
                level: cna_tables::airlog::convoys::ConvoyLevel::G,
                capacity_tons: 2000,
                replacement_tons: 0,
                planning_complete: true,
                convoys: BTreeMap::from([(
                    2,
                    NavalConvoy {
                        lane: 2,
                        arrival_opstage: 3,
                        cargo,
                        status: ConvoyStatus::Planned,
                        delivered: None,
                    },
                )]),
            },
        );
        assert!(game.state.logistics.dumps.is_empty());
        at(&mut game, 13, 3);
        game = evaluate(&Cna::dev(), &c, &game, &Command::Advance)
            .unwrap()
            .game;
        game = drain(&c, game);
        assert_eq!(
            game.state.logistics.convoy_turns[&13].convoys[&2].status,
            ConvoyStatus::Arrived
        );
        assert_eq!(
            game.state.logistics.convoy_turns[&13].convoys[&2].delivered,
            Some(cargo)
        );
        assert_eq!(
            game.state.logistics.arrival_supply.units,
            BTreeSet::from([id.clone()])
        );
        assert!(
            game.state
                .decisions
                .pending
                .iter()
                .any(|p| p.kind == crate::logistics::arrivals::KIND)
        );
        let source = game
            .state
            .logistics
            .dumps
            .values()
            .find(|d| d.side == Side::Axis && d.active && !d.dummy)
            .unwrap()
            .id
            .clone();
        assert_eq!(game.state.logistics.dumps[&source].supplies, cargo);
        assert_eq!(game.state.land.units[&id].cp_spent_quarters, 0);
        game = finish_supply(&c, game);
        let ration = &game.state.logistics.rations[&id];
        assert!(ration.stores_required > 0);
        assert_eq!(ration.stores_received, ration.stores_required);
        assert_eq!(ration.infantry_water_received, 1);
        assert_eq!(
            game.state.logistics.dumps[&source].supplies.stores,
            cargo.stores - ration.stores_received
        );
        assert_eq!(game.state.logistics.dumps[&source].supplies.water, 0);
        assert_eq!(
            game.state.logistics.convoy_turns[&13].convoys[&2].delivered,
            Some(cargo)
        );
    }
}
