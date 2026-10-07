//! Split and place starting second-/third-line pools using persistent identities.
use super::{SetupTask, decisions, facilities, placement};
use crate::{
    CnaContent, State,
    state::{DumpLocation, Location, Pending},
    steps::illegal,
};
use cna_content::{
    scenario::{Placement, Supplies},
    units::Trucks,
};
use cna_core::{
    decision::{ActionSchema, ActionSpace, FieldSchema},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::SeatId,
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role, Side};
use serde_json::Value;
use std::collections::BTreeMap;
pub(super) const KIND: &str = "cna.setup.truck_pool";
fn note(side: Side, text: String, cx: &mut Cx<'_>) {
    cx.emit(EngineEvent::new(
        Audience::Seat(SeatId::new(side, Role::Logistics)),
        GameEvent::Note { text },
    ));
}
fn invariant(s: &str) -> EngineError {
    EngineError::Invariant { detail: s.into() }
}
fn pool<'a>(state: &'a State, id: &str) -> Result<&'a crate::state::TruckPool, EngineError> {
    state
        .logistics
        .truck_pools
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| invariant("starting truck pool is missing"))
}
/// Cases: scen:59.43, scen:59.44
fn domain(
    content: &CnaContent,
    state: &State,
    id: &str,
    source: usize,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<Option<Vec<Location>>, EngineError> {
    let p = pool(state, id)?;
    let record = &content.scenario.supply.second_third_line_trucks[source];
    let case = if record.purpose.as_deref() == Some("air_facilities") {
        "scen:59.43"
    } else {
        "scen:59.44"
    };
    let mut choices = if record.purpose.as_deref() == Some("air_facilities") {
        facilities::catalog(content)?
            .facilities
            .into_iter()
            .filter(|f| f.side == p.side && f.force != "malta")
            .map(|f| f.location)
            .collect::<Vec<_>>()
    } else {
        let Some(choices) =
            placement::for_profile(content, &p.placement, p.side, case, strict, cx)?
        else {
            return Ok(None);
        };
        choices
    };
    if record.purpose.as_deref() != Some("air_facilities")
        && !matches!(p.placement, Placement::City { .. })
    {
        choices.retain(|l| match l {
            Location::OffMap { .. } => p.side == Side::Axis,
            Location::Hex { hex } => {
                state.units_of(p.side).any(|u| {
                    crate::land::formation::combat_unit(content, &u.id)
                        && state
                            .setup
                            .unit_locations
                            .get(&u.id)
                            .unwrap_or(&u.location)
                            .hex()
                            == Some(hex)
                }) || state.logistics.dumps.values().any(|d| {
                    d.side == p.side
                        && !d.dummy
                        && d.active
                        && (state
                            .setup
                            .dump_locations
                            .get(&d.id)
                            .is_some_and(|l| l.hex() == Some(hex))
                            || matches!(&d.location,DumpLocation::Hex{hex:h} if h==hex))
                }) || content.places.places.values().any(|p| {
                    &p.hex_id == hex && matches!(p.kind.as_str(), "city" | "village" | "oasis")
                })
            }
            _ => false,
        });
    }
    let choices: BTreeMap<_, _> = choices
        .into_iter()
        .map(|l| (placement::destination_id(&l).expect("pool domain"), l))
        .collect();
    if choices.is_empty() {
        if strict {
            return Err(EngineError::Unsupported {
                case: case.into(),
                detail: "no verified permitted initial truck location".into(),
            });
        }
        note(
            p.side,
            format!("Truck pool {id} awaits placement: no verified permitted location ({case})."),
            cx,
        );
        return Ok(None);
    }
    Ok(Some(choices.into_values().collect()))
}
fn field(name: &str, max: i32) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        doc: format!("{name} truck points in this split"),
        schema: ActionSchema::Integer {
            min: 0,
            max: i64::from(max),
        },
        optional: false,
    }
}
fn open(
    content: &CnaContent,
    state: &mut State,
    id: &str,
    source: usize,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let Some(domain) = domain(content, state, id, source, strict, cx)? else {
        return Ok(());
    };
    let p = pool(state, id)?;
    let side = p.side;
    let trucks = p.trucks;
    let case = if content.scenario.supply.second_third_line_trucks[source]
        .purpose
        .as_deref()
        == Some("air_facilities")
    {
        "scen:59.43"
    } else {
        "scen:59.44"
    };
    let fields = vec![
        FieldSchema {
            name: "destination".into(),
            doc: "Initial location for these truck points".into(),
            schema: decisions::option_space(&domain).schema,
            optional: false,
        },
        field("light", trucks.light),
        field("medium", trucks.medium),
        field("heavy", trucks.heavy),
    ];
    decisions::open_task(
        state,
        cx,
        SeatId::new(side, Role::Logistics),
        KIND,
        format!("Place any positive portion of truck pool {id}."),
        case,
        SetupTask::Pool {
            pool: id.into(),
            source,
        },
        ActionSpace::new(ActionSchema::Record { fields }),
    );
    Ok(())
}
/// Wait for the land/dump choices so freely placed friendly combat units and dumps are usable.
/// Initial source references are paired once with their constructor's pools; later access uses id.
/// Cases: scen:59.43, scen:59.44
pub(super) fn start(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    if state.setup.pools_started
        || state
            .setup
            .tasks
            .values()
            .any(|t| matches!(t, SetupTask::Unit { .. } | SetupTask::Dump { .. }))
    {
        return Ok(());
    }
    state.setup.pools_started = true;
    let originals: Vec<_> = state
        .setup
        .pool_sources
        .iter()
        .filter_map(|(id, source)| {
            state
                .logistics
                .truck_pools
                .iter()
                .find(|p| &p.id == id && p.location.is_none() && p.trucks.total() > 0)
                .map(|_| (*source, id.clone()))
        })
        .collect();
    for (source, id) in originals {
        open(content, state, &id, source, strict, cx)?;
    }
    Ok(())
}
/// Split only empty initial pools; loaded cargo decisions run after all placement is complete.
/// Cases: scen:59.43, scen:59.44, scen:59.45
pub(super) fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    id: &str,
    source: usize,
    action: &Value,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), Rejection> {
    let p = pool(state, id).map_err(Rejection::Engine)?.clone();
    if p.side != pending.seat.side {
        return Err(illegal("truck pool belongs to another side"));
    }
    let Some(domain) = domain(content, state, id, source, strict, cx).map_err(Rejection::Engine)?
    else {
        return Err(illegal("pool placement is unavailable"));
    };
    let fields = action
        .as_object()
        .filter(|f| f.len() == 4)
        .ok_or_else(|| illegal("provide destination and three truck counts"))?;
    let destination = decisions::resolved_destination(
        &domain,
        fields
            .get("destination")
            .ok_or_else(|| illegal("choose a destination"))?,
    )?;
    let count = |key: &str| max_value(fields.get(key));
    let trucks = Trucks {
        light: count("light")?,
        medium: count("medium")?,
        heavy: count("heavy")?,
    };
    if trucks.light > p.trucks.light
        || trucks.medium > p.trucks.medium
        || trucks.heavy > p.trucks.heavy
        || i64::from(trucks.light) + i64::from(trucks.medium) + i64::from(trucks.heavy) == 0
    {
        return Err(illegal("choose a positive portion within the pool"));
    }
    if p.cargo != Supplies::default() {
        return Err(Rejection::Engine(EngineError::Unsupported {
            case: "scen:59.44".into(),
            detail: "a loaded initial pool needs an explicit cargo split".into(),
        }));
    }
    if trucks == p.trucks {
        state.setup.pool_locations.insert(id.into(), destination);
    } else {
        let original = state
            .logistics
            .truck_pools
            .iter_mut()
            .find(|p| p.id == id)
            .expect("pool");
        original.trucks.light -= trucks.light;
        original.trucks.medium -= trucks.medium;
        original.trucks.heavy -= trucks.heavy;
        let fresh = crate::logistics::pools::add_truck_pool(
            &mut state.logistics,
            None,
            p.side,
            p.placement,
            None,
            trucks,
            Supplies::default(),
        )
        .map_err(|s| Rejection::Engine(invariant(&s)))?;
        state.setup.pool_locations.insert(fresh, destination);
        open(content, state, id, source, strict, cx).map_err(Rejection::Engine)?;
    }
    Ok(())
}
fn max_value(v: Option<&Value>) -> Result<i32, Rejection> {
    v.and_then(Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
        .filter(|n| *n >= 0)
        .ok_or_else(|| illegal("truck counts must be nonnegative integers"))
}
