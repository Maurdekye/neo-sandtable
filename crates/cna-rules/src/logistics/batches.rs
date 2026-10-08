//! Owner-private allocation lists. Answers validate drafts and record plans only.
//! Stocks, rations and cargo effects commit once at the closed step.
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

/// Plain owner orders and public-slot completion keys survive checkpoints.
/// Effects are applied only after the fixed secret windows close.
/// Cases: airlog:51.11, airlog:53.24, airlog:54.11, airlog:56.32, land:3.6
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AllocationBatches {
    pub submitted: BTreeMap<String, BTreeMap<Side, Value>>,
    pub completed: BTreeSet<String>,
}
type ApplyBatch =
    fn(&CnaContent, &mut State, Side, &Value, &mut Cx<'_>) -> Result<String, Rejection>;
pub(super) fn batch_key(state: &State, kind: &str) -> String {
    format!(
        "{kind}:{}:{}",
        state.cursor.game_turn,
        state.cursor.op_stage.unwrap_or(1)
    )
}
/// Validation uses an isolated state and RNG. Only the submitted list is retained.
/// Cases: airlog:51.11, airlog:53.24, airlog:54.11, airlog:56.32, land:3.6
pub(super) fn record_batch(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
    apply: ApplyBatch,
) -> Result<String, Rejection> {
    let key = batch_key(state, &pending.kind);
    let window = &state.logistics.allocation_batches;
    if window.completed.contains(&key)
        || window
            .submitted
            .get(&key)
            .is_some_and(|v| v.contains_key(&pending.seat.side))
    {
        return Err(illegal("allocation list already recorded"));
    }
    let mut preview = state.clone();
    let mut rng = cna_core::dice::CampaignRng::from_state(&cx.rng.state());
    apply(
        content,
        &mut preview,
        pending.seat.side,
        action,
        &mut Cx {
            rng: &mut rng,
            events: &mut vec![],
        },
    )?;
    state
        .logistics
        .allocation_batches
        .submitted
        .entry(key)
        .or_default()
        .insert(pending.seat.side, action.clone());
    notice(
        cx,
        pending.seat.side,
        "Allocation list recorded; effects await step closure.".into(),
    );
    Ok("Allocation list recorded".into())
}
/// Closed windows commit atomically and only once; old checkpoints without a
/// retained list do not repeat stock effects that their earlier engine applied.
/// Cases: airlog:51.11, airlog:53.24, airlog:54.11, airlog:56.32, land:3.6
pub(super) fn finish_recorded(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
    kind: &str,
    sides: &[Side],
    apply: ApplyBatch,
) -> Result<(), EngineError> {
    let key = batch_key(state, kind);
    if !state.decisions.pending.is_empty()
        || state.logistics.allocation_batches.completed.contains(&key)
    {
        return Ok(());
    }
    let orders = state
        .logistics
        .allocation_batches
        .submitted
        .get(&key)
        .cloned()
        .unwrap_or_default();
    let mut draft = state.clone();
    let mut rng = cna_core::dice::CampaignRng::from_state(&cx.rng.state());
    let mut events = vec![];
    for side in sides {
        if let Some(action) = orders.get(side) {
            apply(
                content,
                &mut draft,
                *side,
                action,
                &mut Cx {
                    rng: &mut rng,
                    events: &mut events,
                },
            )
            .map_err(|e| match e {
                Rejection::Engine(e) => e,
                _ => EngineError::Invariant {
                    detail: "accepted logistics allocation failed at closure".into(),
                },
            })?;
        }
    }
    draft.logistics.allocation_batches.submitted.remove(&key);
    draft.logistics.allocation_batches.completed.insert(key);
    *state = draft;
    *cx.rng = rng;
    cx.events.extend(events);
    Ok(())
}
/// Cases: airlog:51.11, airlog:51.23, land:3.6
pub fn finish_stores(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    finish_recorded(content, state, cx, STORES, &SIDES, apply_stores)
}
/// Cases: airlog:53.24, airlog:54.11, airlog:54.12, land:3.6
pub fn finish_distribution(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    finish_recorded(content, state, cx, DISTRIBUTION, &SIDES, apply_distribution)
}

/// Closed well order lists and completion flags persist through checkpoints.
/// Cases: airlog:52.13, airlog:52.16, airlog:52.17, land:3.6
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterWindow {
    pub round: WaterRound,
    pub submitted: BTreeMap<Side, Value>,
    pub stage: Option<water::WaterStage>,
    pub done: BTreeSet<Side>,
    pub waiting: BTreeMap<Side, Vec<WellOrder>>,
    pub completed_wells: BTreeSet<UnitId>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaterRound {
    #[default]
    Supply,
    Allocation,
    Complete,
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
    #[serde(default)]
    pool_allocations: Vec<PoolWaterOrder>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PoolWaterOrder {
    pool: String,
    activity: i32,
    draws: Vec<RationDraw>,
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
        Secrecy::SecretSimultaneous,
        ActionSpace::new(schema).with_pass("Finish this step"),
    );
}
/// Cases: airlog:48.0, airlog:51.11, airlog:51.12, airlog:51.17, airlog:51.23, land:3.6
pub fn enter_stores(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    if state
        .logistics
        .allocation_batches
        .completed
        .contains(&batch_key(state, STORES))
    {
        return Ok(());
    }
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
        open_list(
            state,
            cx,
            side,
            STORES,
            "No stores allocations; pass closes the fixed side window.",
            &["airlog:51.11", "land:3.6"],
            ActionSchema::Choice { options: vec![] },
        );
        return Ok(());
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
fn apply_stores(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    action: &Value,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    if action.is_null() || action.as_array().is_some_and(|a| a.is_empty()) {
        stores::finalize(content, state, side).map_err(Rejection::Engine)?;
        return Ok("Finished stores distribution".into());
    }
    let orders: Vec<StoreOrder> = serde_json::from_value(action.clone())
        .map_err(|_| illegal("invalid stores allocation list"))?;
    let eligible =
        stores::eligible(content, state, side).map_err(|e| Rejection::Engine(engine(e)))?;
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
    stores::finalize(content, &mut draft, side).map_err(Rejection::Engine)?;
    *state = draft;
    notice(
        cx,
        side,
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
    if state.logistics.water_window.round == WaterRound::Supply {
        for side in SIDES {
            open_water(content, state, side, cx, strict)?;
        }
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
    let pool_ids = water::pools::candidates(content, state, side)?;
    if ids.is_empty() && well_ids.is_empty() && pool_ids.is_empty() {
        open_list(
            state,
            cx,
            side,
            WATER,
            "No water issues or well attempts; pass closes this fixed round.",
            &["airlog:52.13", "airlog:52.41", "land:3.6"],
            ActionSchema::Choice { options: vec![] },
        );
        return Ok(());
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
    let mut pool_max = 0;
    let mut pool_sources = BTreeMap::new();
    let mut pool_options = vec![];
    for id in &pool_ids {
        let need =
            water::pools::activity_need(content, state, side, id).map_err(water_answer_error)?;
        pool_max = pool_max.max(need);
        let mut choice = option(id.clone(), id.clone());
        choice.detail = Some(format!("{need} activity water needed"));
        pool_options.push(choice);
        for source in water::pools::sources(content, state, side, id).map_err(water_answer_error)? {
            pool_sources.entry(source.source).or_insert(source.amount);
        }
    }
    let pool_sources: Vec<_> = pool_sources
        .into_iter()
        .map(|(source, amount)| SupplyDraw { source, amount })
        .collect();
    let mut pool_field = field(
        "pool_allocations",
        "Stock issue to real convoy activity reserves",
        list(
            ActionSchema::Record {
                fields: vec![
                    field(
                        "pool",
                        "Owned resolved pool",
                        ActionSchema::Choice {
                            options: pool_options,
                        },
                    ),
                    field(
                        "activity",
                        "Current stage unmet activity water",
                        integer(i64::from(pool_max)),
                    ),
                    field(
                        "draws",
                        "Same-location friendly water stock",
                        stores::draw_schema(&pool_sources, 0, pool_max),
                    ),
                ],
            },
            pool_ids.len(),
        ),
    );
    pool_field.optional = true;
    let schema = ActionSchema::Record {
        fields: vec![
            field(
                "allocations",
                "Issue available stocks to all selected units",
                allocations,
            ),
            pool_field,
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
fn water_answer_error(error: Rejection) -> EngineError {
    match error {
        Rejection::Engine(error) => error,
        _ => EngineError::Invariant {
            detail: "accepted water list failed at closure".into(),
        },
    }
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
pub(super) fn apply_water_answer(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    action: &Value,
    strict: bool,
) -> Result<(), Rejection> {
    if action.is_null() {
        return Ok(());
    }
    let orders: WaterAnswer = serde_json::from_value(action.clone())
        .map_err(|_| illegal("invalid water allocation lists"))?;
    if orders.allocations.is_empty()
        && orders.wells.is_empty()
        && orders.pool_allocations.is_empty()
    {
        return Ok(());
    }
    let count = water::candidates(content, state, side, strict)
        .map_err(Rejection::Engine)?
        .len();
    let wells_count = wells::candidates(content, state, side).len();
    let pools = water::pools::candidates(content, state, side).map_err(Rejection::Engine)?;
    if orders.allocations.len() > count
        || orders.wells.len() > wells_count.saturating_mul(3)
        || orders.pool_allocations.len() > pools.len()
    {
        return Err(illegal("too many water allocations or well attempts"));
    }
    apply_water(content, state, side, orders.allocations, strict)?;
    let mut used_pools = BTreeSet::new();
    for order in orders.pool_allocations {
        if !pools.contains(&order.pool) || !used_pools.insert(order.pool.clone()) {
            return Err(illegal("pool is foreign, repeated or has no unmet demand"));
        }
        water::pools::issue(
            content,
            state,
            side,
            &order.pool,
            order.activity,
            &draws(order.draws)?,
        )?;
    }
    let mut seen = BTreeSet::new();
    let mut attempts = BTreeSet::new();
    for o in &orders.wells {
        let (op, id) = operation(o)?;
        let unit = state
            .land
            .units
            .get(&id)
            .filter(|u| u.side == side)
            .ok_or_else(|| illegal("foreign well unit"))?;
        if !seen.insert((op.to_string(), id.clone())) {
            return Err(illegal("repeated well operation"));
        }
        if !wells::operation_options(content, state, &id, side)
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
            wells::prepare_attempt(content, state, &id, op == "sweeten")?;
        } else {
            wells::prepare_draw(content, state, &id, o.requested, &o.packing)?;
        }
    }
    state
        .logistics
        .water_window
        .waiting
        .insert(side, orders.wells);
    Ok(())
}
fn answer_water(
    content: &CnaContent,
    state: &mut State,
    p: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<String, Rejection> {
    if state.logistics.water_window.round != WaterRound::Supply
        || state
            .logistics
            .water_window
            .submitted
            .contains_key(&p.seat.side)
    {
        return Err(illegal("this water round is already closed"));
    }
    let mut draft = state.clone();
    apply_water_answer(content, &mut draft, p.seat.side, action, strict)?;
    state
        .logistics
        .water_window
        .submitted
        .insert(p.seat.side, action.clone());
    notice(
        cx,
        p.seat.side,
        "Water list accepted; stock and well results await joint closure.".into(),
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
        .filter(|id| {
            state.land.units[*id].side == side && state.logistics.drawn_water[*id].points > 0
        })
        .cloned()
        .collect();
    if ids.is_empty() {
        open_list(
            state,
            cx,
            side,
            WELL_ALLOCATION,
            "No well water to allocate; pass closes the fixed second round.",
            &["airlog:52.13", "land:3.6"],
            ActionSchema::Choice { options: vec![] },
        );
        return Ok(true);
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
    if state.logistics.water_window.stage != Some(water::WaterStage::current(state))
        || state.logistics.water_window.round == WaterRound::Complete
        || !state.decisions.pending.is_empty()
        || SIDES
            .iter()
            .any(|side| !state.logistics.water_window.submitted.contains_key(side))
    {
        return Ok(());
    }
    let mut draft = state.clone();
    let submitted = std::mem::take(&mut draft.logistics.water_window.submitted);
    match draft.logistics.water_window.round {
        WaterRound::Supply => {
            for side in SIDES {
                apply_water_answer(content, &mut draft, side, &submitted[&side], strict)
                    .map_err(water_answer_error)?;
            }
            let mut order = SIDES;
            let has_orders = draft
                .logistics
                .water_window
                .waiting
                .values()
                .any(|v| !v.is_empty());
            if let Some(a) = draft.turn.player_a {
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
            let mut waiting = std::mem::take(&mut draft.logistics.water_window.waiting);
            for side in order {
                for o in waiting.remove(&side).unwrap_or_default() {
                    let (op, id) = operation(&o).map_err(|_| EngineError::Invariant {
                        detail: "invalid accepted well operation".into(),
                    })?;
                    if op == "draw" {
                        wells::resolve_draw(content, &mut draft, &id, o.requested, cx)?;
                    } else {
                        wells::resolve_attempt(content, &mut draft, &id, op == "sweeten", cx)?;
                    }
                    draft.logistics.water_window.completed_wells.insert(id);
                }
            }

            draft.logistics.water_window.round = WaterRound::Allocation;
            for side in SIDES {
                open_allocations(content, &mut draft, side, cx)?;
            }
        }
        WaterRound::Allocation => {
            for side in SIDES {
                apply_allocations_answer(content, &mut draft, side, &submitted[&side]).map_err(
                    |_| EngineError::Invariant {
                        detail: "accepted well allocation failed at closure".into(),
                    },
                )?;
                draft
                    .logistics
                    .drawn_water
                    .retain(|id, _| draft.land.units[id].side != side);
                finish_side(content, &mut draft, side, strict)?;
            }
            draft.logistics.water_window.round = WaterRound::Complete;
        }
        WaterRound::Complete => {}
    }
    *state = draft;
    Ok(())
}
/// Cases: airlog:52.13, airlog:52.41, airlog:52.42, airlog:54.2, land:3.6
fn apply_allocations_answer(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    action: &Value,
) -> Result<(), Rejection> {
    if action.is_null() || action.as_array().is_some_and(|a| a.is_empty()) {
        state
            .logistics
            .drawn_water
            .retain(|id, _| state.land.units[id].side != side);
    } else {
        let orders: Vec<WellAllocation> = serde_json::from_value(action.clone())
            .map_err(|_| illegal("invalid well allocation list"))?;
        let own_ids: BTreeSet<_> = state
            .logistics
            .drawn_water
            .iter()
            .filter(|(id, d)| {
                state.land.units.get(*id).is_some_and(|u| u.side == side) && d.points > 0
            })
            .map(|(id, _)| id.clone())
            .collect();
        if orders.len() > own_ids.len() {
            return Err(illegal("too many well allocations"));
        }
        if orders.iter().any(|o| !own_ids.contains(&o.unit)) {
            return Err(illegal("unknown own well allocation"));
        }
        let mut seen = BTreeSet::new();
        for o in orders {
            if !seen.insert(o.unit.clone())
                || !state
                    .land
                    .units
                    .get(&o.unit)
                    .is_some_and(|u| u.side == side)
            {
                return Err(illegal("foreign or repeated well allocation"));
            }
            wells::allocate(
                content,
                state,
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
    Ok(())
}
fn answer_allocations(
    content: &CnaContent,
    state: &mut State,
    p: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
    _strict: bool,
) -> Result<String, Rejection> {
    if state.logistics.water_window.round != WaterRound::Allocation
        || state
            .logistics
            .water_window
            .submitted
            .contains_key(&p.seat.side)
    {
        return Err(illegal("this well allocation round is already closed"));
    }
    let mut draft = state.clone();
    apply_allocations_answer(content, &mut draft, p.seat.side, action)?;
    state
        .logistics
        .water_window
        .submitted
        .insert(p.seat.side, action.clone());
    notice(
        cx,
        p.seat.side,
        "Well-water list accepted; allocation awaits joint closure.".into(),
    );
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
    if state
        .logistics
        .allocation_batches
        .completed
        .contains(&batch_key(state, DISTRIBUTION))
    {
        return Ok(());
    }
    if strict {
        super::ready::preflight(content).map_err(engine)?;
    }
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
        open_list(
            state,
            cx,
            side,
            DISTRIBUTION,
            "No supply transfers; pass closes the fixed side window.",
            &["airlog:54.13", "land:3.6"],
            ActionSchema::Choice { options: vec![] },
        );
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
fn apply_distribution(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    action: &Value,
    cx: &mut Cx<'_>,
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
        distribution::transfer(content, &mut draft, side, &from, &to, o.amount, &o.packing)
            .map_err(|e| match e {
                SupplyError::Unsupported { .. } | SupplyError::UnknownFuelRate => {
                    Rejection::Engine(engine(e))
                }
                _ => illegal("transfer exceeds stocks, capacity or same-location rules"),
            })?;
    }
    *state = draft;
    notice(
        cx,
        side,
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
        STORES => record_batch(content, state, p, action, cx, apply_stores),
        WATER => answer_water(content, state, p, action, cx, strict),
        WELL_ALLOCATION => answer_allocations(content, state, p, action, cx, strict),
        DISTRIBUTION => record_batch(content, state, p, action, cx, apply_distribution),
        _ => Err(illegal("unknown logistics batch")),
    }
}
#[cfg(test)]
mod tests;
