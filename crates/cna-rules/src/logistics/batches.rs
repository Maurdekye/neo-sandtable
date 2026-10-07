//! Owner-private allocation lists. Validate the entire list on a draft before committing.
//! Well results are adjudicated only after both seats close their current lists.
use super::{
    CargoPacking, SupplyDraw, SupplyError, available_sources_with_content, capacity, distribution,
    rations,
    stores::{self, RationDraw, draws, engine, field, option},
    water, wells,
};
use crate::{
    CnaContent, State,
    state::Pending,
    steps::{illegal, open},
};
use cna_content::scenario::Supplies;
use cna_core::{
    decision::{ActionSchema, ActionSpace, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{SeatId, UnitId},
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role, Side};
use cna_tables::airlog::supply::SupplyType;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const STORES: &str = "cna.logistics.stores.batch";
pub const WATER: &str = "cna.logistics.water.batch";
pub const WELL_ALLOCATION: &str = "cna.logistics.well.allocate.batch";
pub const DISTRIBUTION: &str = "cna.logistics.distribution.batch";
const SIDES: [Side; 2] = [Side::Axis, Side::Commonwealth];
const GOODS: [SupplyType; 4] = [
    SupplyType::Ammo,
    SupplyType::Fuel,
    SupplyType::Stores,
    SupplyType::Water,
];

/// Closed well order lists and completion flags persist through checkpoints.
/// Cases: airlog:52.13, airlog:52.16, airlog:52.17, land:3.6
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WaterWindow {
    pub stage: Option<water::WaterStage>,
    pub done: BTreeSet<Side>,
    pub waiting: BTreeMap<Side, Vec<WellOrder>>,
    pub completed_wells: BTreeSet<UnitId>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WellOrder {
    pub operation: String,
    pub requested: i32,
    pub packing: CargoPacking,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreOrder {
    unit: UnitId,
    stores: i32,
    half: bool,
    pasta: bool,
    draws: Vec<RationDraw>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaterOrder {
    unit: UnitId,
    infantry: i32,
    activity: i32,
    pasta: bool,
    draws: Vec<RationDraw>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaterAnswer {
    allocations: Vec<WaterOrder>,
    wells: Vec<WellOrder>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WellAllocation {
    unit: UnitId,
    infantry: i32,
    activity: i32,
    pasta: bool,
    cargo: i32,
    packing: CargoPacking,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferOrder {
    from: String,
    to: String,
    amount: Supplies,
    packing: CargoPacking,
}

fn integer(max: i64) -> ActionSchema {
    ActionSchema::Integer { min: 0, max }
}
fn list(item: ActionSchema, max: usize) -> ActionSchema {
    ActionSchema::List {
        item: Box::new(item),
        min: 0,
        max: max as u32,
    }
}
fn notice(cx: &mut Cx<'_>, side: Side, text: String) {
    cx.emit(EngineEvent::new(
        Audience::Side(side),
        GameEvent::Note { text },
    ));
}
fn sources(
    content: &CnaContent,
    state: &State,
    ids: &[UnitId],
) -> Result<Vec<SupplyDraw>, EngineError> {
    let mut all = BTreeMap::new();
    for id in ids {
        for source in available_sources_with_content(content, state, id).map_err(engine)? {
            all.entry(serde_json::to_string(&source.source).unwrap())
                .or_insert(source);
        }
    }
    Ok(all.into_values().collect())
}
fn unit_schema(ids: &[UnitId], detail: impl Fn(&UnitId) -> String) -> ActionSchema {
    ActionSchema::Choice {
        options: ids
            .iter()
            .map(|id| {
                let mut c = option(id.to_string(), id.to_string());
                c.detail = Some(detail(id));
                c
            })
            .collect(),
    }
}
fn open_list(
    state: &mut State,
    cx: &mut Cx<'_>,
    side: Side,
    kind: &str,
    summary: &str,
    cases: &[&str],
    schema: ActionSchema,
) {
    open(
        state,
        cx,
        SeatId::new(side, Role::Logistics),
        kind,
        summary.into(),
        cases,
        Trigger::Scheduled,
        Secrecy::Secret,
        ActionSpace::new(schema).with_pass("Finish this step"),
    );
}
/// Cases: airlog:48.0, airlog:51.11, airlog:51.12, airlog:51.17, airlog:51.23, land:3.6
pub fn enter_stores(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    if state.logistics.stores_started_gt != Some(state.cursor.game_turn) {
        stores::feed_prisoners(content, state, cx, true)?;
        stores::weekly_losses(content, state);
        state.logistics.stores_started_gt = Some(state.cursor.game_turn);
    }
    for side in SIDES {
        open_stores(content, state, side, cx)?;
    }
    Ok(())
}
fn open_stores(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let ids = stores::eligible(content, state, side).map_err(engine)?;
    if ids.is_empty() {
        return stores::finalize(content, state, side);
    }
    let max = ids
        .iter()
        .map(|id| rations::stores_required(content, state, id).map(i64::from))
        .collect::<Result<Vec<_>, _>>()
        .map_err(engine)?
        .into_iter()
        .max()
        .unwrap_or(0);
    let schema = list(
        ActionSchema::Record {
            fields: vec![
                field(
                    "unit",
                    "One eligible friendly unit",
                    unit_schema(&ids, |id| {
                        format!(
                            "{} stores; at {:?}",
                            rations::stores_required(content, state, id).unwrap(),
                            state.land.units[id].location
                        )
                    }),
                ),
                field(
                    "stores",
                    "Up to this unit's weekly requirement",
                    integer(max),
                ),
                field(
                    "half",
                    "Half rations require a local shortage",
                    ActionSchema::Bool,
                ),
                field("pasta", "Italian battalion pasta water", ActionSchema::Bool),
                field(
                    "draws",
                    "Exact same-location withdrawals",
                    stores::draw_schema(&sources(content, state, &ids)?, max as i32, 1),
                ),
            ],
        },
        ids.len(),
    );
    open_list(
        state,
        cx,
        side,
        STORES,
        "Issue this week's stores in one list. Units must occur once; listed withdrawals share the available stocks.",
        &["airlog:51.11", "airlog:51.23", "land:3.6"],
        schema,
    );
    Ok(())
}
/// Atomic even when the last entry is invalid or earlier entries exhausted a shared source.
/// Cases: airlog:51.11, airlog:51.15, airlog:51.23, land:3.6
fn answer_stores(
    content: &CnaContent,
    state: &mut State,
    p: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    if action.is_null() || action.as_array().is_some_and(|a| a.is_empty()) {
        stores::finalize(content, state, p.seat.side).map_err(Rejection::Engine)?;
        return Ok("Finished stores distribution".into());
    }
    let orders: Vec<StoreOrder> = serde_json::from_value(action.clone())
        .map_err(|_| illegal("invalid stores allocation list"))?;
    let eligible =
        stores::eligible(content, state, p.seat.side).map_err(|e| Rejection::Engine(engine(e)))?;
    let mut draft = state.clone();
    let mut used = BTreeSet::new();
    for o in &orders {
        if !eligible.contains(&o.unit) || !used.insert(o.unit.clone()) {
            return Err(illegal("unit is foreign, repeated or already assessed"));
        }
    }
    for o in orders {
        stores::issue_unit(
            content,
            &mut draft,
            &o.unit,
            o.stores,
            o.half,
            o.pasta,
            &draws(o.draws)?,
        )?;
    }
    let mut emitted = Vec::new();
    open_stores(
        content,
        &mut draft,
        p.seat.side,
        &mut Cx {
            rng: cx.rng,
            events: &mut emitted,
        },
    )
    .map_err(Rejection::Engine)?;
    *state = draft;
    cx.events.extend(emitted);
    notice(
        cx,
        p.seat.side,
        format!("Recorded {} stores allocations.", used.len()),
    );
    Ok("Stores allocation list recorded".into())
}
/// Cases: airlog:52.0, airlog:52.41, airlog:52.42, land:3.6
pub fn enter_water(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    water::prepare(content, state, cx, strict)?;
    let stage = water::WaterStage::current(state);
    if state.logistics.water_window.stage != Some(stage) {
        state.logistics.water_window = WaterWindow {
            stage: Some(stage),
            ..Default::default()
        };
    }
    for side in SIDES {
        open_water(content, state, side, cx, strict)?;
    }
    Ok(())
}
fn open_water(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    if state.logistics.water_window.done.contains(&side) {
        return Ok(());
    }
    let ids = water::candidates(content, state, side, strict)?;
    let well_ids = wells::candidates(content, state, side);
    if ids.is_empty() && well_ids.is_empty() {
        return finish_side(content, state, side, strict);
    }
    let mut infantry = 0;
    let mut activity = 0;
    let mut total = 0;
    for id in &ids {
        let r = water::requirements(content, state, id).map_err(engine)?;
        infantry = infantry.max(r.infantry);
        activity = activity.max(r.activity);
        total = total.max(i64::from(r.infantry) + i64::from(r.activity) + i64::from(r.pasta));
    }
    let allocations = list(
        ActionSchema::Record {
            fields: vec![
                field(
                    "unit",
                    "Eligible friendly unit",
                    unit_schema(&ids, |id| {
                        let r = water::requirements(content, state, id).unwrap();
                        format!(
                            "infantry {}; activity {}; pasta {}; at {:?}",
                            r.infantry, r.activity, r.pasta, state.land.units[id].location
                        )
                    }),
                ),
                field("infantry", "Immediate body water", integer(infantry.into())),
                field("activity", "Reserve for CPA use", integer(activity.into())),
                field("pasta", "Missing weekly pasta water", ActionSchema::Bool),
                field(
                    "draws",
                    "Exact same-location stock withdrawals",
                    stores::draw_schema(
                        &sources(content, state, &ids)?,
                        0,
                        total.min(i64::from(i32::MAX)) as i32,
                    ),
                ),
            ],
        },
        ids.len(),
    );
    let mut options = Vec::new();
    let mut max = 0i64;
    for id in &well_ids {
        let r = water::requirements(content, state, id).map_err(engine)?;
        let held = state
            .logistics
            .unit_supply
            .get(id)
            .map_or(0, |s| s.carried.water);
        let cap = super::cargo_bound(content, state, id, SupplyType::Water).map_err(engine)?;
        let bound = i64::from(r.infantry)
            + i64::from(r.activity)
            + i64::from(r.pasta)
            + i64::from((cap - held).max(0));
        max = max.max(bound);
        for mut o in wells::operation_options(content, state, id, side) {
            o.id = format!("{}|{id}", o.id);
            o.label = format!("{id}: {}", o.label);
            o.detail = Some(format!(
                "draw at most {bound} water; at {:?}",
                state.land.units[id].location
            ));
            options.push(o);
        }
    }
    let packing = packing_schema(content, state, &well_ids);
    let schema = ActionSchema::Record {
        fields: vec![
            field(
                "allocations",
                "Issue available stocks to all selected units",
                allocations,
            ),
            field(
                "wells",
                "Accepted attempts resolve after both seats close; at most one draw per unit and one identical attempt per well in this list",
                list(
                    ActionSchema::Record {
                        fields: vec![
                            field(
                                "operation",
                                "Operation and its unit",
                                ActionSchema::Choice { options },
                            ),
                            field(
                                "requested",
                                "Positive for draw; zero for other operations",
                                integer(max.min(i64::from(i32::MAX))),
                            ),
                            field(
                                "packing",
                                "Final packing for the requested excess; zero for other operations",
                                packing,
                            ),
                        ],
                    },
                    well_ids.len().saturating_mul(3),
                ),
            ),
        ],
    };
    open_list(
        state,
        cx,
        side,
        WATER,
        "Distribute water and submit well attempts in lists. Empty lists or pass finish; hidden well results wait for the window to close.",
        &[
            "airlog:52.13",
            "airlog:52.16",
            "airlog:52.41",
            "airlog:52.42",
            "land:3.6",
        ],
        schema,
    );
    Ok(())
}
fn finish_side(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    strict: bool,
) -> Result<(), EngineError> {
    water::finalize(content, state, side, strict)?;
    state.logistics.water_window.done.insert(side);
    Ok(())
}
fn operation(order: &WellOrder) -> Result<(&str, UnitId), Rejection> {
    let (op, id) = order
        .operation
        .split_once('|')
        .ok_or_else(|| illegal("unknown well operation"))?;
    Ok((op, UnitId::new(id)))
}
fn apply_water(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    orders: Vec<WaterOrder>,
    strict: bool,
) -> Result<(), Rejection> {
    let eligible = water::candidates(content, state, side, strict).map_err(Rejection::Engine)?;
    let mut used = BTreeSet::new();
    for o in orders {
        if !eligible.contains(&o.unit) || !used.insert(o.unit.clone()) {
            return Err(illegal("unit is foreign, repeated or already assessed"));
        }
        water::issue_unit(
            content,
            state,
            &o.unit,
            o.infantry,
            o.activity,
            o.pasta,
            &draws(o.draws)?,
        )?;
    }
    Ok(())
}
/// Validate private stocks and reserve CP, without consulting undiscovered well conditions or dice.
/// Cases: airlog:52.13, airlog:52.16, airlog:52.17, airlog:52.41, airlog:52.42, land:3.6
fn answer_water(
    content: &CnaContent,
    state: &mut State,
    p: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<String, Rejection> {
    if action.is_null() {
        finish_side(content, state, p.seat.side, strict).map_err(Rejection::Engine)?;
        return Ok("Finished water distribution".into());
    }
    let orders: WaterAnswer = serde_json::from_value(action.clone())
        .map_err(|_| illegal("invalid water allocation lists"))?;
    if orders.allocations.is_empty() && orders.wells.is_empty() {
        finish_side(content, state, p.seat.side, strict).map_err(Rejection::Engine)?;
        return Ok("Finished water distribution".into());
    }
    let mut draft = state.clone();
    apply_water(content, &mut draft, p.seat.side, orders.allocations, strict)?;
    let mut seen = BTreeSet::new();
    let mut attempts = BTreeSet::new();
    for o in &orders.wells {
        let (op, id) = operation(o)?;
        let unit = draft
            .land
            .units
            .get(&id)
            .filter(|u| u.side == p.seat.side)
            .ok_or_else(|| illegal("foreign well unit"))?;
        if !seen.insert((op.to_string(), id.clone())) {
            return Err(illegal("repeated well operation"));
        }
        if !wells::operation_options(content, &draft, &id, p.seat.side)
            .iter()
            .any(|c| c.id == op)
        {
            return Err(illegal("well operation is not available"));
        }
        if op != "draw" {
            if !attempts.insert((op.to_string(), unit.location.clone().hex().cloned())) {
                return Err(illegal(
                    "await the previous attempt's result before repeating at this well",
                ));
            }
            if o.requested != 0 || o.packing != CargoPacking::default() {
                return Err(illegal(
                    "non-draw operation must have zero quantity and packing",
                ));
            }
            wells::prepare_attempt(content, &mut draft, &id, op == "sweeten")?;
        } else {
            wells::prepare_draw(content, &mut draft, &id, o.requested, &o.packing)?;
        }
    }
    draft
        .logistics
        .water_window
        .waiting
        .insert(p.seat.side, orders.wells);
    *state = draft;
    notice(
        cx,
        p.seat.side,
        "Water allocation list accepted; well results await closure.".into(),
    );
    Ok("Water lists recorded".into())
}
fn packing_schema(content: &CnaContent, state: &State, ids: &[UnitId]) -> ActionSchema {
    let mut schema = ActionSchema::Record {
        fields: ["light", "medium", "heavy"]
            .into_iter()
            .map(|name| {
                field(
                    name,
                    "Final cargo",
                    ActionSchema::Record {
                        fields: ["ammo", "fuel", "stores", "water"]
                            .into_iter()
                            .map(|name| field(name, "Whole supply points", integer(0)))
                            .collect(),
                    },
                )
            })
            .collect(),
    };
    for id in ids {
        let next = wells::packing_schema(content, state, id);
        widen(&mut schema, &next);
    }
    schema
}
fn widen(a: &mut ActionSchema, b: &ActionSchema) {
    match (a, b) {
        (ActionSchema::Integer { max: x, .. }, ActionSchema::Integer { max: y, .. }) => {
            *x = (*x).max(*y)
        }
        (ActionSchema::Record { fields: x }, ActionSchema::Record { fields: y }) => {
            for (x, y) in x.iter_mut().zip(y) {
                widen(&mut x.schema, &y.schema)
            }
        }
        _ => {}
    }
}
fn open_allocations(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    cx: &mut Cx<'_>,
) -> Result<bool, EngineError> {
    let ids: Vec<_> = state
        .logistics
        .drawn_water
        .keys()
        .filter(|id| state.land.units[*id].side == side)
        .cloned()
        .collect();
    if ids.is_empty() {
        return Ok(false);
    }
    let max = ids
        .iter()
        .map(|id| i64::from(state.logistics.drawn_water[id].points))
        .max()
        .unwrap_or(0);
    let schema = list(
        ActionSchema::Record {
            fields: vec![
                field(
                    "unit",
                    "Unit with a recorded draw",
                    unit_schema(&ids, |id| {
                        format!("{} water available", state.logistics.drawn_water[id].points)
                    }),
                ),
                field("infantry", "Consume body water", integer(max)),
                field("activity", "Reserve for CPA use", integer(max)),
                field("pasta", "Missing pasta water", ActionSchema::Bool),
                field("cargo", "Carry water on trucks", integer(max)),
                field(
                    "packing",
                    "Final packing after consumption",
                    packing_schema(content, state, &ids),
                ),
            ],
        },
        ids.len(),
    );
    open_list(
        state,
        cx,
        side,
        WELL_ALLOCATION,
        "Allocate all recorded well results in one list. Any unused water is left at the source.",
        &[
            "airlog:52.13",
            "airlog:52.41",
            "airlog:52.42",
            "airlog:54.2",
            "land:3.6",
        ],
        schema,
    );
    Ok(true)
}
/// Only after closure may hidden conditions, rolls and disclosures affect results.
/// Cases: airlog:52.13, airlog:52.14, airlog:52.16, airlog:52.17, land:3.6
/// Interpretations: interp:airlog-0016
pub fn finish_water(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    if state.logistics.water_window.stage != Some(water::WaterStage::current(state)) {
        return Ok(());
    }
    let mut order = SIDES;
    let has_orders = state
        .logistics
        .water_window
        .waiting
        .values()
        .any(|v| !v.is_empty());
    if let Some(a) = state.turn.player_a {
        if a == Side::Commonwealth {
            order.reverse();
        }
    } else if has_orders {
        let die = cx.rng.d6();
        cx.emit(EngineEvent::new(
            Audience::Operator,
            GameEvent::DiceRolled {
                purpose: "Well operation order without Player A".into(),
                dice: vec![die.value()],
                reading: None,
                rule: Some("interp:airlog-0016".into()),
            },
        ));
        if die.value() <= 3 {
            order.reverse();
        }
    }
    let mut waiting = std::mem::take(&mut state.logistics.water_window.waiting);
    for side in order {
        for o in waiting.remove(&side).unwrap_or_default() {
            let (op, id) = operation(&o).map_err(|_| EngineError::Invariant {
                detail: "invalid accepted well operation".into(),
            })?;
            if op == "draw" {
                wells::resolve_draw(content, state, &id, o.requested, cx)?;
            } else {
                wells::resolve_attempt(content, state, &id, op == "sweeten", cx)?;
            }
            state.logistics.water_window.completed_wells.insert(id);
        }
    }
    for side in SIDES {
        if !open_allocations(content, state, side, cx)? {
            open_water(content, state, side, cx, strict)?;
        }
    }
    Ok(())
}
/// Cases: airlog:52.13, airlog:52.41, airlog:52.42, airlog:54.2, land:3.6
fn answer_allocations(
    content: &CnaContent,
    state: &mut State,
    p: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<String, Rejection> {
    let mut draft = state.clone();
    if action.is_null() || action.as_array().is_some_and(|a| a.is_empty()) {
        draft
            .logistics
            .drawn_water
            .retain(|id, _| draft.land.units[id].side != p.seat.side);
        finish_side(content, &mut draft, p.seat.side, strict).map_err(Rejection::Engine)?;
    } else {
        let orders: Vec<WellAllocation> = serde_json::from_value(action.clone())
            .map_err(|_| illegal("invalid well allocation list"))?;
        let mut seen = BTreeSet::new();
        for o in orders {
            if !seen.insert(o.unit.clone())
                || !draft
                    .land
                    .units
                    .get(&o.unit)
                    .is_some_and(|u| u.side == p.seat.side)
            {
                return Err(illegal("foreign or repeated well allocation"));
            }
            wells::allocate(
                content,
                &mut draft,
                &o.unit,
                &wells::Allocation {
                    infantry: o.infantry,
                    activity: o.activity,
                    pasta: o.pasta,
                    cargo: o.cargo,
                    packing: o.packing,
                },
            )?;
        }
    }
    let mut events = vec![];
    open_allocations(
        content,
        &mut draft,
        p.seat.side,
        &mut Cx {
            rng: cx.rng,
            events: &mut events,
        },
    )
    .map_err(Rejection::Engine)?;
    *state = draft;
    cx.events.extend(events);
    Ok("Recorded well allocation list".into())
}
/// Cases: airlog:49.16, airlog:53.24, airlog:54.13, land:3.6
pub fn enter_distribution(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    enter_distribution_with_policy(content, state, cx, false)
}
/// Cases: airlog:50.17, airlog:53.24, land:3.6
pub fn enter_distribution_with_policy(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    for side in SIDES {
        open_distribution(content, state, side, cx, strict)?;
    }
    Ok(())
}
fn open_distribution(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    let mut receivers = Vec::new();
    let mut source_ids = BTreeSet::new();
    let mut sources = Vec::new();
    for to in distribution::endpoints(state, side) {
        if let distribution::Endpoint::Tank(id) = &to
            && !super::fuel_capacity(content, state, id).is_ok_and(|cap| {
                state
                    .logistics
                    .unit_supply
                    .get(id)
                    .map_or(0, |h| h.tank_fuel.get())
                    < cap.get()
            })
        {
            continue;
        }
        let accessible = distribution::sources(state, side, &to);
        if accessible.is_empty() {
            continue;
        }
        if let distribution::Endpoint::Ready(id) = &to {
            match super::ready_ammo_capacity(content, state, id) {
                Ok(cap)
                    if state
                        .logistics
                        .unit_supply
                        .get(id)
                        .map_or(0, |h| h.ready_ammo.get())
                        < cap.get() => {}
                Ok(_) => continue,
                Err(e) if strict => return Err(engine(e)),
                Err(_) => {
                    notice(
                        cx,
                        side,
                        format!(
                            "{id}: ready-ammunition capacity is unresolved (50.17); this unit's top-up is unavailable."
                        ),
                    );
                    continue;
                }
            }
        }
        receivers.push(to);
        for source in accessible {
            if source_ids.insert(serde_json::to_string(&source).unwrap()) {
                sources.push(source);
            }
        }
    }
    if receivers.is_empty() {
        return Ok(());
    }
    let choice = |ends: &[distribution::Endpoint]| ActionSchema::Choice {
        options: ends
            .iter()
            .map(|e| option(serde_json::to_string(e).unwrap(), format!("{e:?}")))
            .collect(),
    };
    let mut packing = distribution::packing_schema(content, state, &receivers[0]);
    for to in &receivers[1..] {
        widen(
            &mut packing,
            &distribution::packing_schema(content, state, to),
        );
    }
    let amount = ActionSchema::Record {
        fields: GOODS
            .into_iter()
            .zip(["ammo", "fuel", "stores", "water"])
            .map(|(t, name)| {
                let max = sources
                    .iter()
                    .filter_map(|e| distribution::stock(state, e).ok())
                    .map(|s| i64::from(capacity::points(&s, t)))
                    .max()
                    .unwrap_or(0);
                field(
                    name,
                    if t == SupplyType::Fuel {
                        "Whole points; tenths for tank receivers"
                    } else {
                        "Whole supply points"
                    },
                    integer(if t == SupplyType::Fuel { max * 10 } else { max }),
                )
            })
            .collect(),
    };
    let schema = list(
        ActionSchema::Record {
            fields: vec![
                field(
                    "from",
                    "One friendly same-location source",
                    choice(&sources),
                ),
                field(
                    "to",
                    "Receiver; trucks unload to a dump before another truck loads",
                    choice(&receivers),
                ),
                field("amount", "Exact transfer", amount),
                field("packing", "Receiver's final cargo packing", packing),
            ],
        },
        1024,
    );
    open_list(
        state,
        cx,
        side,
        DISTRIBUTION,
        "Redistribute supplies in an ordered list. All transfers share stocks and capacities and commit together. Pass finishes.",
        &["airlog:49.16", "airlog:53.24", "airlog:54.13", "land:3.6"],
        schema,
    );
    Ok(())
}
/// Cases: airlog:49.16, airlog:53.24, airlog:54.13, airlog:54.2, land:3.6
fn answer_distribution(
    content: &CnaContent,
    state: &mut State,
    p: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<String, Rejection> {
    if action.is_null() || action.as_array().is_some_and(|a| a.is_empty()) {
        return Ok("Finished redistribution".into());
    }
    let orders: Vec<TransferOrder> =
        serde_json::from_value(action.clone()).map_err(|_| illegal("invalid transfer list"))?;
    if orders.len() > 1024 {
        return Err(illegal("too many transfers"));
    }
    let mut draft = state.clone();
    for o in &orders {
        let from = serde_json::from_str(&o.from).map_err(|_| illegal("unknown source"))?;
        let to = serde_json::from_str(&o.to).map_err(|_| illegal("unknown receiver"))?;
        distribution::transfer(
            content,
            &mut draft,
            p.seat.side,
            &from,
            &to,
            o.amount,
            &o.packing,
        )
        .map_err(|e| match e {
            SupplyError::Unsupported { .. } | SupplyError::UnknownFuelRate => {
                Rejection::Engine(engine(e))
            }
            _ => illegal("transfer exceeds stocks, capacity or same-location rules"),
        })?;
    }
    let mut events = vec![];
    open_distribution(
        content,
        &mut draft,
        p.seat.side,
        &mut Cx {
            rng: cx.rng,
            events: &mut events,
        },
        strict,
    )
    .map_err(Rejection::Engine)?;
    *state = draft;
    cx.events.extend(events);
    notice(
        cx,
        p.seat.side,
        format!("Recorded {} supply transfers.", orders.len()),
    );
    Ok("Supply transfer list recorded".into())
}
/// Cases: airlog:51.11, airlog:52.13, airlog:52.41, airlog:53.24, land:3.6
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    p: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<String, Rejection> {
    match p.kind.as_str() {
        STORES => answer_stores(content, state, p, action, cx),
        WATER => answer_water(content, state, p, action, cx, strict),
        WELL_ALLOCATION => answer_allocations(content, state, p, action, cx, strict),
        DISTRIBUTION => answer_distribution(content, state, p, action, cx, strict),
        _ => Err(illegal("unknown logistics batch")),
    }
}
#[cfg(test)]
mod tests;
