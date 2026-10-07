//! A post-arrival owner supply window, restricted to exact successfully placed unit ids.
//! Every stage has two simultaneous rounds for both side Logistics seats, including empty
//! forced-pass windows: stock issues and draw requests, then actual well-yield allocation.
//! Physical issues, CP, dice and shortages resolve at finish, after the fixed barrier.
//! Stocks are consumed by the ordinary supply APIs. Wells resolve after closure, so
//! answer acceptance cannot probe hidden conditions (docs/engine.md section3 rule7).
use super::{
    CargoPacking, SupplyDemand, SupplyDraw, SupplyError, SupplySource,
    available_sources_with_content, fuel_capacity, rations, spend_for_unit_with_content,
    stores::{self, engine, field, option},
    water, wells,
};
use crate::{
    CnaContent, State,
    state::Pending,
    steps::{illegal, open},
};
use cna_core::{
    decision::{ActionSchema, ActionSpace, Secrecy, Trigger},
    dice::CampaignRng,
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{SeatId, UnitId},
    quantity::{FuelTenths, StoresPoints, WaterPoints},
    visibility::{Audience, Perspective},
};
use cna_protocol::{GameEvent, Role, Side};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
pub const KIND: &str = "cna.logistics.arrivals.batch";
const SIDES: [Side; 2] = [Side::Axis, Side::Commonwealth];
/// Exact successful placements, completion flags and accepted draws survive checkpoints.
/// Cases: land:20.12, airlog:51.11, airlog:52.13, airlog:56.28, land:3.6
/// Interpretations: interp:airlog-0017
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArrivalSupplyWindow {
    #[serde(default)]
    pub round: ArrivalRound,
    #[serde(default)]
    pub submitted: BTreeMap<Side, Value>,
    pub stage: Option<water::WaterStage>,
    pub units: BTreeSet<UnitId>,
    pub done: BTreeSet<Side>,
    pub waiting: BTreeMap<Side, Vec<DrawOrder>>,
    pub completed_wells: BTreeSet<UnitId>,
}
/// Both sides receive the same two fixed rounds, including empty forced-pass batches.
/// Cases: land:20.12, airlog:52.13, land:3.6
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrivalRound {
    #[default]
    Supply,
    Water,
    Complete,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrawOrder {
    pub unit: UnitId,
    pub requested: i32,
    pub packing: CargoPacking,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Draw {
    source: String,
    stores: i32,
    water: i32,
    fuel_tenths: i32,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Issue {
    unit: UnitId,
    stores: i32,
    half: bool,
    pasta: bool,
    infantry: i32,
    activity: i32,
    fuel_tenths: i32,
    draws: Vec<Draw>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WellAllocation {
    unit: UnitId,
    infantry: i32,
    activity: i32,
    pasta: bool,
    cargo: i32,
    packing: CargoPacking,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    allocations: Vec<Issue>,
    well_allocations: Vec<WellAllocation>,
    wells: Vec<DrawOrder>,
}
fn integer(max: i64) -> ActionSchema {
    ActionSchema::Integer { min: 0, max }
}
fn list(schema: ActionSchema, max: usize) -> ActionSchema {
    ActionSchema::List {
        item: Box::new(schema),
        min: 0,
        max: u32::try_from(max).unwrap_or(u32::MAX),
    }
}
fn choices(content: &CnaContent, state: &State, ids: &[UnitId]) -> ActionSchema {
    ActionSchema::Choice {
        options: ids
            .iter()
            .map(|id| {
                let mut choice = option(id.to_string(), id.to_string());
                let history = state.logistics.rations.get(id);
                let pending_food =
                    history.is_none_or(|r| r.issued_gt != Some(state.cursor.game_turn));
                let pending_water = history
                    .is_none_or(|r| r.water_issue_stage != Some(water::WaterStage::current(state)));
                let need = water::requirements(content, state, id).ok();
                let full = if pending_food {
                    rations::stores_required(content, state, id).ok()
                } else {
                    Some(0)
                };
                let sources = available_sources_with_content(content, state, id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|s| !matches!(s.source, SupplySource::Tank | SupplySource::ReadyAmmo))
                    .collect::<Vec<_>>();
                let tank = state
                    .logistics
                    .unit_supply
                    .get(id)
                    .map_or(0, |h| h.tank_fuel.get());
                let room = fuel_capacity(content, state, id)
                    .ok()
                    .map(|c| (c.get() - tank).max(0));
                choice.detail = Some(
                    json!({
                        "location":state.land.units[id].location,
                        "full_stores_max":full,
                        "stores_already_assessed":!pending_food,
                        "infantry_water_remaining":need.map(|n|n.infantry),
                        "activity_water_remaining":need.map(|n|n.activity),
                        "pasta_water_remaining":need.map(|n|n.pasta),
                        "pasta_needed_if_fed":pending_food&&rations::pasta(content,id),
                        "water_already_assessed":!pending_water,
                        "tank_room_tenths":room,
                        "same_location_sources":sources,
                        "recorded_well_water":state.logistics.drawn_water.get(id).map(|w|w.points),
                        "trucks":state.land.units[id].trucks,
                        "transport_trucks":state.land.units[id].transport_trucks,
                    })
                    .to_string(),
                );
                choice
            })
            .collect(),
    }
}
fn owned(state: &State, side: Side, id: &UnitId) -> bool {
    state.logistics.arrival_supply.units.contains(id)
        && state
            .land
            .units
            .get(id)
            .is_some_and(|u| u.side == side && rations::in_play(&u.location))
}
fn ids(
    content: &CnaContent,
    state: &State,
    side: Side,
    strict: bool,
) -> Result<Vec<UnitId>, EngineError> {
    let mut out = vec![];
    for id in &state.logistics.arrival_supply.units {
        if !owned(state, side, id) {
            continue;
        }
        if state.land.units[id].toe.is_none() && content.units.units[id].class.is_none() {
            continue;
        }
        match water::requirements(content, state, id) {
            Ok(_) => {
                rations::stores_required(content, state, id).map_err(engine)?;
                out.push(id.clone());
            }
            Err(SupplyError::Unsupported {
                case: "airlog:52.42",
            }) if !strict => {}
            Err(e) => return Err(engine(e)),
        }
    }
    Ok(out)
}
/// Every answer domain is derived solely from this side's eligible arrivals.
struct OwnerDomains {
    units: Vec<UnitId>,
    drawn: Vec<UnitId>,
    wells: Vec<UnitId>,
}
fn owner_domains(
    content: &CnaContent,
    state: &State,
    side: Side,
    strict: bool,
) -> Result<OwnerDomains, EngineError> {
    let units = ids(content, state, side, strict)?;
    let drawn: Vec<_> = units
        .iter()
        .filter(|id| state.logistics.drawn_water.contains_key(*id))
        .cloned()
        .collect();
    let well_ids: Vec<_> = units
        .iter()
        .filter(|id| {
            !state.logistics.arrival_supply.completed_wells.contains(*id)
                && !state.logistics.drawn_water.contains_key(*id)
                && wells::operation_options(content, state, id, side)
                    .iter()
                    .any(|o| o.id == "draw")
        })
        .cloned()
        .collect();
    Ok(OwnerDomains {
        units,
        drawn,
        wells: well_ids,
    })
}
fn widen(a: &mut ActionSchema, b: &ActionSchema) {
    match (a, b) {
        (ActionSchema::Integer { max: x, .. }, ActionSchema::Integer { max: y, .. }) => {
            *x = (*x).max(*y)
        }
        (ActionSchema::Record { fields: x }, ActionSchema::Record { fields: y }) => {
            for (x, y) in x.iter_mut().zip(y) {
                widen(&mut x.schema, &y.schema);
            }
        }
        _ => {}
    }
}
fn packing(content: &CnaContent, state: &State, ids: &[UnitId]) -> ActionSchema {
    let mut schema = ActionSchema::Record { fields: vec![] };
    if let Some(id) = ids.first() {
        schema = wells::packing_schema(content, state, id);
    }
    for id in ids.iter().skip(1) {
        widen(&mut schema, &wells::packing_schema(content, state, id));
    }
    schema
}
fn open_side(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    if state.logistics.arrival_supply.round == ArrivalRound::Complete
        || state.logistics.arrival_supply.submitted.contains_key(&side)
    {
        return Ok(());
    }
    let OwnerDomains {
        mut units,
        mut drawn,
        wells: mut well_ids,
    } = owner_domains(content, state, side, strict)?;
    let water_round = state.logistics.arrival_supply.round == ArrivalRound::Water;
    if water_round {
        units.clear();
        well_ids.clear();
        drawn.retain(|id| {
            state
                .logistics
                .drawn_water
                .get(id)
                .is_some_and(|w| w.points > 0)
        });
    }
    if !strict && !water_round {
        for id in &state.logistics.arrival_supply.units {
            if owned(state, side, id)
                && matches!(
                    water::requirements(content, state, id),
                    Err(SupplyError::Unsupported {
                        case: "airlog:52.42"
                    })
                )
            {
                cx.emit(EngineEvent::new(
                    Audience::Side(side),
                    GameEvent::Note {
                        text: format!(
                            "{id}: arrival water composition is unknown (52.42); left unassessed."
                        ),
                    },
                ));
            }
        }
    }
    let mut source_keys = BTreeSet::new();
    for id in &units {
        for d in available_sources_with_content(content, state, id).map_err(engine)? {
            if !matches!(d.source, SupplySource::Tank | SupplySource::ReadyAmmo) {
                source_keys.insert(serde_json::to_string(&d.source).unwrap());
            }
        }
    }
    let draw = list(
        ActionSchema::Record {
            fields: vec![
                field(
                    "source",
                    "One enumerated friendly source",
                    ActionSchema::Choice {
                        options: source_keys
                            .iter()
                            .map(|s| option(s.clone(), s.clone()))
                            .collect(),
                    },
                ),
                field("stores", "Whole stores points", integer(i32::MAX.into())),
                field("water", "Whole water points", integer(i32::MAX.into())),
                field(
                    "fuel_tenths",
                    "Fuel tenths (stocks pay source ceiling)",
                    integer(i32::MAX.into()),
                ),
            ],
        },
        source_keys.len(),
    );
    let allocation = list(
        ActionSchema::Record {
            fields: vec![
                field(
                    "unit",
                    "Newly arrived unit only",
                    choices(content, state, &units),
                ),
                field(
                    "stores",
                    "This week's stores, within the actual requirement",
                    integer(i32::MAX.into()),
                ),
                field(
                    "half",
                    "Half rations only during an actual local shortage",
                    ActionSchema::Bool,
                ),
                field(
                    "pasta",
                    "One pasta water; omit if already supplied",
                    ActionSchema::Bool,
                ),
                field(
                    "infantry",
                    "Infantry water consumed",
                    integer(i32::MAX.into()),
                ),
                field(
                    "activity",
                    "Vehicle activity reserve",
                    integer(i32::MAX.into()),
                ),
                field(
                    "fuel_tenths",
                    "Own tank refill, exact tenths",
                    integer(i32::MAX.into()),
                ),
                field(
                    "draws",
                    "Sources must exactly equal the supplies issued",
                    draw,
                ),
            ],
        },
        units.len(),
    );
    let well_allocation = list(
        ActionSchema::Record {
            fields: vec![
                field(
                    "unit",
                    "Unit with a completed well draw",
                    choices(content, state, &drawn),
                ),
                field("infantry", "Body water consumed", integer(i32::MAX.into())),
                field(
                    "activity",
                    "Activity water reserved",
                    integer(i32::MAX.into()),
                ),
                field("pasta", "Missing pasta water", ActionSchema::Bool),
                field(
                    "cargo",
                    "Water retained as truck cargo",
                    integer(i32::MAX.into()),
                ),
                field(
                    "packing",
                    "Final cargo packing",
                    packing(content, state, &drawn),
                ),
            ],
        },
        drawn.len(),
    );
    let wells = list(
        ActionSchema::Record {
            fields: vec![
                field(
                    "unit",
                    "Arriving unit drawing at its actual well",
                    choices(content, state, &well_ids),
                ),
                field(
                    "requested",
                    "Positive request within need and carrying capacity",
                    integer(i32::MAX.into()),
                ),
                field(
                    "packing",
                    "Final cargo if the requested water is obtained",
                    packing(content, state, &well_ids),
                ),
            ],
        },
        well_ids.len(),
    );
    let schema = if units.is_empty() && drawn.is_empty() && well_ids.is_empty() {
        // A visibly empty choice domain plus declared pass lets the local driver avoid a
        // model call. A record of optional empty lists would not prove a forced action.
        ActionSchema::Choice { options: vec![] }
    } else {
        ActionSchema::Record {
            fields: vec![
                field("allocations", "Stores, water and fuel issues", allocation),
                field(
                    "well_allocations",
                    "Allocate recorded water before drawing again",
                    well_allocation,
                ),
                field("wells", "Draws resolve after both lists close", wells),
            ],
        }
    };
    let summary = if water_round {
        "Allocate actual arrival well yields. Both sides have this fixed second round; pass discards unallocated water."
    } else {
        "Supply newly arrived units from actual same-location stocks or request one well draw per unit. Both sides have this fixed first round; shortages are assessed after the allocation round."
    };
    open(
        state,
        cx,
        SeatId::new(side, Role::Logistics),
        KIND,
        summary.into(),
        &[
            "land:20.12",
            "airlog:49.14",
            "airlog:49.16",
            "airlog:51.11",
            "airlog:52.13",
            "airlog:52.41",
            "airlog:52.42",
            "airlog:56.28",
            "land:3.6",
            "interp:airlog-0017",
        ],
        Trigger::Scheduled,
        Secrecy::SecretSimultaneous,
        ActionSpace::new(schema).with_pass("Finish arrival supply"),
    );
    Ok(())
}
/// Call after all fixed land-role batches close and supply convoys arrive.
/// Entry opens the first of two fixed simultaneous rounds for both Logistics seats.
/// Cases: land:20.12, airlog:56.28, land:3.6
/// Interpretations: interp:airlog-0017
pub fn enter(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
    new_ids: &BTreeSet<UnitId>,
) -> Result<(), EngineError> {
    if state.cursor.anchor() != "opstage.convoy_arrival" {
        return Err(engine(SupplyError::Invalid));
    }
    let stage = water::WaterStage::current(state);
    if state.logistics.arrival_supply.stage == Some(stage) {
        return Ok(());
    }
    for id in new_ids {
        if !state
            .land
            .units
            .get(id)
            .is_some_and(|u| rations::in_play(&u.location))
        {
            return Err(engine(SupplyError::Invalid));
        }
    }
    state.logistics.arrival_supply = ArrivalSupplyWindow {
        stage: Some(stage),
        units: new_ids.clone(),
        ..Default::default()
    };
    for side in SIDES {
        open_side(content, state, side, strict, cx)?;
    }
    Ok(())
}
fn apply_issue(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    o: Issue,
) -> Result<(), Rejection> {
    if !owned(state, side, &o.unit) {
        return Err(illegal("unit is outside this arrival window"));
    }
    let gt = state.cursor.game_turn;
    let stage = water::WaterStage::current(state);
    let food_new = state
        .logistics
        .rations
        .get(&o.unit)
        .is_none_or(|r| r.issued_gt != Some(gt))
        && rations::stores_required(content, state, &o.unit)
            .map_err(|e| Rejection::Engine(engine(e)))?
            > 0;
    let water_new = state
        .logistics
        .rations
        .get(&o.unit)
        .is_none_or(|r| r.water_issue_stage != Some(stage));
    if !food_new && (o.stores != 0 || o.half) {
        return Err(illegal("arrival stores were already assessed"));
    }
    if !water_new && (o.infantry != 0 || o.activity != 0) {
        return Err(illegal("arrival water was already assessed"));
    }
    if o.fuel_tenths < 0 {
        return Err(illegal("negative arrival fuel"));
    }
    let cap = if o.fuel_tenths > 0 {
        fuel_capacity(content, state, &o.unit)
            .map_err(|e| Rejection::Engine(engine(e)))?
            .get()
    } else {
        0
    };
    let tank = state
        .logistics
        .unit_supply
        .get(&o.unit)
        .map_or(0, |h| h.tank_fuel.get());
    if tank < 0 {
        return Err(illegal("invalid arrival tank holding"));
    }
    let filled = tank
        .checked_add(o.fuel_tenths)
        .ok_or_else(|| illegal("fuel capacity overflow"))?;
    if o.fuel_tenths > 0 && filled > cap {
        return Err(illegal("arrival fuel exceeds tank capacity"));
    }
    let pasta_in_food = food_new && o.pasta;
    let mut pasta_left = i32::from(pasta_in_food);
    let mut food = vec![];
    let mut drink = vec![];
    let mut fuel = vec![];
    for d in o.draws {
        if d.stores < 0 || d.water < 0 || d.fuel_tenths < 0 {
            return Err(illegal("negative arrival allocation"));
        }
        let source: SupplySource = serde_json::from_str(&d.source)
            .map_err(|_| illegal("unknown arrival supply source"))?;
        if matches!(source, SupplySource::Tank | SupplySource::ReadyAmmo) {
            return Err(illegal(
                "arrival refill needs dumped or first-line supplies",
            ));
        }
        let pasta = pasta_left.min(d.water);
        pasta_left -= pasta;
        if d.stores > 0 || pasta > 0 {
            food.push(SupplyDraw {
                source: source.clone(),
                amount: SupplyDemand {
                    stores: StoresPoints::new(d.stores),
                    water: WaterPoints::new(pasta),
                    ..Default::default()
                },
            });
        }
        if d.water > pasta {
            drink.push(SupplyDraw {
                source: source.clone(),
                amount: SupplyDemand {
                    water: WaterPoints::new(d.water - pasta),
                    ..Default::default()
                },
            });
        }
        if d.fuel_tenths > 0 {
            fuel.push(SupplyDraw {
                source,
                amount: SupplyDemand {
                    fuel: FuelTenths::new(d.fuel_tenths),
                    ..Default::default()
                },
            });
        }
    }
    if food_new {
        stores::issue_unit(
            content,
            state,
            &o.unit,
            o.stores,
            o.half,
            pasta_in_food,
            &food,
        )?;
    } else if !food.is_empty() {
        return Err(illegal("stores were already assessed"));
    }
    if water_new {
        water::issue_unit(
            content,
            state,
            &o.unit,
            o.infantry,
            o.activity,
            o.pasta && !pasta_in_food,
            &drink,
        )?;
    } else if !drink.is_empty() || o.pasta && !pasta_in_food {
        return Err(illegal("water was already assessed"));
    }
    spend_for_unit_with_content(
        content,
        state,
        &o.unit,
        SupplyDemand {
            fuel: FuelTenths::new(o.fuel_tenths),
            ..Default::default()
        },
        &fuel,
    )
    .map_err(|_| illegal("arrival fuel unavailable at these sources"))?;
    state
        .logistics
        .unit_supply
        .entry(o.unit)
        .or_default()
        .tank_fuel = FuelTenths::new(filled);
    Ok(())
}
fn finalize_side(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    strict: bool,
) -> Result<(), EngineError> {
    // The established finalizers assess all units they receive. Restrict the draft to this
    // exact roster and merge only its body/ration changes, never other units' shortages.
    let mut scoped = state.clone();
    scoped
        .land
        .units
        .retain(|id, u| u.side == side && state.logistics.arrival_supply.units.contains(id));
    stores::finalize(content, &mut scoped, side)?;
    water::finalize(content, &mut scoped, side, strict)?;
    for (id, unit) in scoped.land.units {
        state.land.units.insert(id.clone(), unit);
        if let Some(r) = scoped.logistics.rations.remove(&id) {
            state.logistics.rations.insert(id, r);
        }
    }
    state.logistics.drawn_water.retain(|id, _| {
        !owned_for_retention(
            &state.logistics.arrival_supply.units,
            &state.land.units,
            side,
            id,
        )
    });
    state.logistics.arrival_supply.done.insert(side);
    Ok(())
}
fn owned_for_retention(
    ids: &BTreeSet<UnitId>,
    units: &BTreeMap<UnitId, crate::state::LandUnit>,
    side: Side,
    id: &UnitId,
) -> bool {
    ids.contains(id) && units.get(id).is_some_and(|u| u.side == side)
}
/// Answers use own stocks, own CP and public locations only. Well conditions and dice wait.
/// Cases: airlog:49.14, airlog:49.16, airlog:51.23, airlog:52.13, land:3.6
/// Interpretations: interp:airlog-0017
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    p: &Pending,
    action: &Value,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    if p.kind != KIND
        || p.seat.role != Role::Logistics
        || state.cursor.anchor() != "opstage.convoy_arrival"
        || state.logistics.arrival_supply.stage != Some(water::WaterStage::current(state))
        || state.logistics.arrival_supply.round == ArrivalRound::Complete
        || state
            .logistics
            .arrival_supply
            .submitted
            .contains_key(&p.seat.side)
    {
        return Err(illegal("not an active arrival supply window"));
    }
    // Validate only an owner's draft. No physical change, dice, shortage or successor window
    // is committed until both fixed side-role requests close (engine rule 7).
    apply_answer(content, &mut state.clone(), p.seat.side, action, strict)?;
    state
        .logistics
        .arrival_supply
        .submitted
        .insert(p.seat.side, action.clone());
    cx.emit(EngineEvent::new(
        Audience::Side(p.seat.side),
        GameEvent::Note {
            text: "Arrival supply list accepted; quantities and shortages remain private.".into(),
        },
    ));
    Ok("Recorded arrival supply".into())
}
fn apply_answer(
    content: &CnaContent,
    draft: &mut State,
    side: Side,
    action: &Value,
    strict: bool,
) -> Result<Vec<DrawOrder>, Rejection> {
    if action.is_null() {
        return Ok(vec![]);
    }
    let a: Answer = serde_json::from_value(action.clone())
        .map_err(|_| illegal("invalid arrival supply lists"))?;
    let mut domains = owner_domains(content, draft, side, strict).map_err(Rejection::Engine)?;
    if draft.logistics.arrival_supply.round == ArrivalRound::Water {
        domains.units.clear();
        domains.wells.clear();
        domains.drawn.retain(|id| {
            draft
                .logistics
                .drawn_water
                .get(id)
                .is_some_and(|w| w.points > 0)
        });
    }
    if a.allocations.len() > domains.units.len()
        || a.well_allocations.len() > domains.drawn.len()
        || a.wells.len() > domains.wells.len()
    {
        return Err(illegal("too many arrival allocations"));
    }
    let mut seen = BTreeSet::new();
    for o in a.allocations {
        if !domains.units.contains(&o.unit) {
            return Err(illegal("foreign arrival issue"));
        }
        if !seen.insert(o.unit.clone()) {
            return Err(illegal("repeated arrival issue"));
        }
        apply_issue(content, draft, side, o)?;
    }
    seen.clear();
    for o in a.well_allocations {
        if !domains.drawn.contains(&o.unit) || !seen.insert(o.unit.clone()) {
            return Err(illegal("foreign or repeated arrival well allocation"));
        }
        wells::allocate(
            content,
            draft,
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
    seen.clear();
    for o in &a.wells {
        if !domains.wells.contains(&o.unit) || !seen.insert(o.unit.clone()) {
            return Err(illegal("foreign or repeated arrival draw"));
        }
        wells::prepare_draw(content, draft, &o.unit, o.requested, &o.packing)?;
    }
    Ok(a.wells)
}
/// Both fixed side rounds close before adjudication. Well yields are seen before allocation.
/// Cases: airlog:52.13, airlog:52.14, airlog:52.16, land:20.12, land:3.6
/// Interpretations: interp:airlog-0009, interp:airlog-0016, interp:airlog-0017
pub fn finish(
    content: &CnaContent,
    state: &mut State,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    if state.logistics.arrival_supply.stage != Some(water::WaterStage::current(state))
        || !state.decisions.pending.is_empty()
        || state.logistics.arrival_supply.round == ArrivalRound::Complete
        || SIDES
            .iter()
            .any(|s| !state.logistics.arrival_supply.submitted.contains_key(s))
    {
        return Ok(());
    }
    let mut draft = state.clone();
    let answers = std::mem::take(&mut draft.logistics.arrival_supply.submitted);
    let mut waiting = BTreeMap::new();
    for side in SIDES {
        let draws = apply_answer(content, &mut draft, side, &answers[&side], strict)
            .map_err(|_| engine(SupplyError::Invalid))?;
        waiting.insert(side, draws);
    }
    if draft.logistics.arrival_supply.round == ArrivalRound::Supply {
        let mut order = SIDES;
        if draft.turn.player_a == Some(Side::Commonwealth) {
            order.reverse();
        }
        if draft.turn.player_a.is_none() && waiting.values().any(|v| !v.is_empty()) {
            let die = cx.rng.d6();
            if die.value() <= 3 {
                order.reverse();
            }
            cx.emit(EngineEvent::new(
                Audience::Operator,
                GameEvent::DiceRolled {
                    purpose: "Arrival well ordering without Player A".into(),
                    dice: vec![die.value()],
                    reading: None,
                    rule: Some("interp:airlog-0016".into()),
                },
            ));
        }
        for side in order {
            for o in waiting.remove(&side).unwrap_or_default() {
                wells::resolve_draw(content, &mut draft, &o.unit, o.requested, cx)?;
                draft
                    .logistics
                    .arrival_supply
                    .completed_wells
                    .insert(o.unit);
            }
        }
        draft.logistics.arrival_supply.round = ArrivalRound::Water;
        for side in SIDES {
            open_side(content, &mut draft, side, strict, cx)?;
        }
    } else {
        for side in SIDES {
            finalize_side(content, &mut draft, side, strict)?;
        }
        draft.logistics.arrival_supply.round = ArrivalRound::Complete;
    }
    *state = draft;
    Ok(())
}

/// Feed/water arriving units from owned stocks, then draw an actual known source if needed.
/// Controller randomness never consumes campaign adjudication dice.
/// Cases: airlog:49.14, airlog:51.11, airlog:51.23, airlog:52.13, land:3.6
/// Interpretations: interp:airlog-0017
pub(super) fn baseline(
    content: &CnaContent,
    state: &State,
    side: Side,
    rng: &mut CampaignRng,
) -> Value {
    if state.logistics.arrival_supply.done.contains(&side) {
        return Value::Null;
    }
    let mut draft = state.clone();
    let mut allocations = vec![];
    let mut well_allocations = vec![];
    let mut draws = vec![];
    let water_round = state.logistics.arrival_supply.round == ArrivalRound::Water;
    let mut units = ids(content, state, side, false)
        .unwrap_or_default()
        .into_iter()
        .map(|id| {
            let restricted = super::movement_restrictions(content, state, &id)
                .is_ok_and(|r| !r.may_move || !r.may_exceed_cpa);
            let need = rations::stores_required(content, state, &id).unwrap_or(i32::MAX);
            (u8::from(!restricted), need, rng.d6().value(), id)
        })
        .collect::<Vec<_>>();
    units.sort();
    for (_, _, _, id) in units {
        if !water_round {
            let gt = draft.cursor.game_turn;
            let stage = water::WaterStage::current(&draft);
            let food_new = draft
                .logistics
                .rations
                .get(&id)
                .is_none_or(|r| r.issued_gt != Some(gt));
            let water_new = draft
                .logistics
                .rations
                .get(&id)
                .is_none_or(|r| r.water_issue_stage != Some(stage));
            let Ok(required) = water::requirements(content, &draft, &id) else {
                continue;
            };
            let full = if food_new {
                rations::stores_required(content, &draft, &id).unwrap_or(0)
            } else {
                0
            };
            let sources = available_sources_with_content(content, &draft, &id)
                .unwrap_or_default()
                .into_iter()
                .filter(|d| !matches!(d.source, SupplySource::Tank | SupplySource::ReadyAmmo))
                .collect::<Vec<_>>();
            let available = |f: fn(&SupplyDemand) -> i32| {
                sources
                    .iter()
                    .map(|d| i64::from(f(&d.amount)))
                    .sum::<i64>()
                    .min(i64::from(i32::MAX)) as i32
            };
            let stores_available = available(|a| a.stores.get());
            let mut water_available = available(|a| a.water.get());
            let may_half = rations::class(content, &id)
                .is_ok_and(|c| !matches!(c.unit_type.as_str(), "headquarters" | "engineer"));
            let half = food_new
                && full > 0
                && stores_available < full
                && stores_available >= full / 2
                && may_half;
            let food = if half {
                full / 2
            } else {
                full.min(stores_available)
            };
            let pasta = water_available > 0
                && rations::pasta(content, &id)
                && ((food_new && food > 0) || (water_new && required.pasta == 1));
            water_available -= i32::from(pasta);
            let infantry = if water_new {
                required.infantry.min(water_available)
            } else {
                0
            };
            water_available -= infantry;
            let activity = if water_new {
                required.activity.min(water_available)
            } else {
                0
            };
            let tank = draft
                .logistics
                .unit_supply
                .get(&id)
                .map_or(0, |h| h.tank_fuel.get());
            let fuel = fuel_capacity(content, &draft, &id)
                .map_or(0, |cap| (cap.get() - tank).max(0))
                .min(available(|a| a.fuel.get()));
            if (food_new && full > 0)
                || water_new
                    && (required.infantry > 0 || required.activity > 0 || required.pasta > 0)
                || fuel > 0
            {
                let (mut f, mut w, mut g) = (food, infantry + activity + i32::from(pasta), fuel);
                let mut funding = vec![];
                for d in &sources {
                    let food = f.min(d.amount.stores.get());
                    let water = w.min(d.amount.water.get());
                    let fuel = g.min(d.amount.fuel.get());
                    if food > 0 || water > 0 || fuel > 0 {
                        funding.push(json!({"source":serde_json::to_string(&d.source).unwrap(),"stores":food,"water":water,"fuel_tenths":fuel}));
                        f -= food;
                        w -= water;
                        g -= fuel;
                    }
                }
                let value = json!({"unit":id,"stores":food,"half":half,"pasta":pasta,"infantry":infantry,"activity":activity,"fuel_tenths":fuel,"draws":funding});
                let issue: Issue = serde_json::from_value(value.clone()).unwrap();
                let mut next = draft.clone();
                if apply_issue(content, &mut next, side, issue).is_ok() {
                    draft = next;
                    allocations.push(value);
                }
            }
        }
        if let Some(result) = draft
            .logistics
            .drawn_water
            .get(&id)
            .filter(|r| r.points > 0)
        {
            let Ok(need) = water::requirements(content, &draft, &id) else {
                continue;
            };
            let mut available = result.points;
            let infantry = need.infantry.min(available);
            available -= infantry;
            let activity = need.activity.min(available);
            available -= activity;
            let pasta = need.pasta == 1 && available > 0;
            let unit = &draft.land.units[&id];
            let stock = draft
                .logistics
                .unit_supply
                .get(&id)
                .map_or(Default::default(), |h| h.carried);
            let Some(packing) =
                super::capacity::find_packing(content, &unit.trucks, &unit.transport_trucks, stock)
            else {
                continue;
            };
            let allocation = wells::Allocation {
                infantry,
                activity,
                pasta,
                cargo: 0,
                packing: packing.clone(),
            };
            let mut next = draft.clone();
            if wells::allocate(content, &mut next, &id, &allocation).is_ok() {
                draft = next;
                well_allocations.push(json!({"unit":id,"infantry":infantry,"activity":activity,"pasta":pasta,"cargo":0,"packing":packing}));
            }
        }
        if water_round {
            continue;
        }
        if draft.logistics.arrival_supply.completed_wells.contains(&id)
            || draft.logistics.drawn_water.contains_key(&id)
        {
            continue;
        }
        let Ok(need) = water::requirements(content, &draft, &id) else {
            continue;
        };
        let requested = need
            .infantry
            .checked_add(need.activity)
            .and_then(|n| n.checked_add(need.pasta))
            .unwrap_or(0);
        if requested <= 0
            || !wells::operation_options(content, &draft, &id, side)
                .iter()
                .any(|o| o.id == "draw")
        {
            continue;
        }
        if let Some(hex) = draft.land.units[&id].location.hex() {
            let known = wells::condition(&draft, hex, Perspective::Side(side));
            if known["depleted"] == true || known["poisoned"] == true {
                continue;
            }
        }
        let unit = &draft.land.units[&id];
        let stock = draft
            .logistics
            .unit_supply
            .get(&id)
            .map_or(Default::default(), |h| h.carried);
        let Some(packing) =
            super::capacity::find_packing(content, &unit.trucks, &unit.transport_trucks, stock)
        else {
            continue;
        };
        let mut next = draft.clone();
        if wells::prepare_draw(content, &mut next, &id, requested, &packing).is_ok() {
            draft = next;
            draws.push(json!({"unit":id,"requested":requested,"packing":packing}));
        }
    }
    if allocations.is_empty() && well_allocations.is_empty() && draws.is_empty() {
        Value::Null
    } else {
        json!({"allocations":allocations,"well_allocations":well_allocations,"wells":draws})
    }
}

#[cfg(test)]
mod tests;
