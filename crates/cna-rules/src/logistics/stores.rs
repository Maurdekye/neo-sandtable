//! Weekly stores choices, mandatory prisoner/guard feeding, and storage losses.
use super::rations::{self, WaterStage};
use super::{
    SupplyDemand, SupplyDraw, SupplyError, available_sources_with_content,
    spend_for_unit_with_content,
};
use crate::content::CnaContent;
use crate::state::{DumpLocation, Location, Pending, State};
use crate::steps::{illegal, open};
use cna_core::decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger};
use cna_core::engine::{Cx, EngineError, Rejection};
use cna_core::event::EngineEvent;
use cna_core::ids::{SeatId, UnitId};
use cna_core::quantity::{StoresPoints, WaterPoints};
use cna_core::visibility::Audience;
use cna_protocol::{GameEvent, Role, Side};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

pub const KIND: &str = "cna.logistics.stores";
pub const ISSUE_PREFIX: &str = "cna.logistics.stores.issue:";

pub(super) fn engine(error: SupplyError) -> EngineError {
    match error {
        SupplyError::Unsupported { case } => EngineError::Unsupported {
            case: case.into(),
            detail: "required logistics content is unavailable".into(),
        },
        _ => EngineError::Invariant {
            detail: format!("invalid logistics state: {error:?}"),
        },
    }
}
pub(super) fn field(name: &str, doc: &str, schema: ActionSchema) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        doc: doc.into(),
        schema,
        optional: false,
    }
}
pub(super) fn option(id: String, label: String) -> ChoiceOption {
    ChoiceOption {
        id,
        label,
        detail: None,
    }
}

/// Full inventories lose whole points; exact fuel tanks lose whole Fuel Points,
/// preserving their fractional balance. Off-map stocks and sea convoys are excluded.
/// Cases: airlog:49.3, airlog:52.44
pub fn weekly_losses(content: &CnaContent, state: &mut State) {
    let date = crate::view::wire_clock(content, state).date;
    let rate = |side| {
        if side == Side::Commonwealth
            && date.as_str() >= "1940-09-01"
            && date.as_str() < "1941-09-01"
        {
            9i64
        } else {
            6i64
        }
    };
    let reduce = |stock: &mut cna_content::scenario::Supplies, side| {
        let r = rate(side);
        stock.fuel -= (i64::from(stock.fuel) * r / 100) as i32;
        stock.water -= (i64::from(stock.water) * r / 100) as i32;
    };
    for dump in state.logistics.dumps.values_mut() {
        if matches!(dump.location, DumpLocation::Hex { .. }) {
            reduce(&mut dump.supplies, dump.side);
        }
    }
    for (id, holding) in &mut state.logistics.unit_supply {
        if let Some(unit) = state.land.units.get(id)
            && unit.location.hex().is_some()
        {
            reduce(&mut holding.carried, unit.side);
            let fuel_loss = i64::from(holding.tank_fuel.get()) * rate(unit.side) / 1000 * 10;
            holding.tank_fuel -= cna_core::quantity::FuelTenths::new(fuel_loss as i32);
            let water_loss = i64::from(holding.activity_water.get()) * rate(unit.side) / 100;
            holding.activity_water -= WaterPoints::new(water_loss as i32);
        }
    }
    for (side, pool) in &mut state.logistics.air_supply_pool {
        reduce(pool, *side);
    }
}

/// Prisoners are served ahead of units, and guards receive their separate weekly rate.
/// Quantity decisions and reports remain private under Limited Intelligence.
/// Cases: airlog:48.0, airlog:51.11, airlog:51.12, airlog:51.13, airlog:51.17, land:3.6
pub fn enter(content: &CnaContent, state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    if state.logistics.stores_started_gt != Some(state.cursor.game_turn) {
        feed_prisoners(content, state, cx, true)?;
        weekly_losses(content, state);
        state.logistics.stores_started_gt = Some(state.cursor.game_turn);
    }
    for side in [Side::Axis, Side::Commonwealth] {
        open_menu(content, state, side, cx)?;
    }
    Ok(())
}

pub(super) fn eligible(
    content: &CnaContent,
    state: &State,
    side: Side,
) -> Result<Vec<UnitId>, SupplyError> {
    state
        .land
        .units
        .values()
        .filter(|u| u.side == side && rations::in_play(&u.location))
        .filter(|u| {
            state
                .logistics
                .rations
                .get(&u.id)
                .is_none_or(|r| r.issued_gt != Some(state.cursor.game_turn))
        })
        .filter_map(|u| match rations::stores_required(content, state, &u.id) {
            Ok(0) => None,
            Ok(_) => Some(Ok(u.id.clone())),
            Err(e) => Some(Err(e)),
        })
        .collect()
}

/// The registry's open default is overridden by the quantity secrecy of land:3.6.
/// Cases: airlog:51.0, airlog:51.23, land:3.6
fn open_menu(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let units = eligible(content, state, side).map_err(engine)?;
    if units.is_empty() {
        finalize(content, state, side)?;
        return Ok(());
    }
    let mut options = vec![option(
        "done".into(),
        "Finish; record all remaining units as short of stores".into(),
    )];
    options.extend(
        units
            .into_iter()
            .map(|id| option(id.to_string(), format!("Issue stores to {id}"))),
    );
    open(
        state,
        cx,
        SeatId::new(side, Role::Logistics),
        KIND,
        "Issue this week's stores. Prisoners have been served first. Choose a unit or finish."
            .into(),
        &["airlog:51.0", "airlog:51.12", "airlog:51.23", "land:3.6"],
        Trigger::Scheduled,
        Secrecy::Secret,
        ActionSpace::new(ActionSchema::Choice { options })
            .with_pass("Finish stores distribution and record remaining shortages"),
    );
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RationDraw {
    pub source: String,
    pub stores: i32,
    pub water: i32,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Issue {
    stores: i32,
    half: bool,
    pasta: bool,
    draws: Vec<RationDraw>,
}

/// Source identities and bounds are generated from this unit's friendly accessible stocks.
/// Cases: airlog:51.15, airlog:52.6, land:3.6
pub(super) fn draw_schema(sources: &[SupplyDraw], stores_max: i32, water_max: i32) -> ActionSchema {
    ActionSchema::List {
        min: 0,
        max: sources
            .iter()
            .filter(|s| s.amount.stores.get() > 0 || s.amount.water.get() > 0)
            .count() as u32,
        item: Box::new(ActionSchema::Record {
            fields: vec![
                field(
                    "source",
                    "Friendly stock to withdraw from",
                    ActionSchema::Choice {
                        options: sources
                            .iter()
                            .filter(|s| s.amount.stores.get() > 0 || s.amount.water.get() > 0)
                            .map(|s| ChoiceOption {
                                id: serde_json::to_string(&s.source).unwrap(),
                                label: format!("{:?}", s.source),
                                detail: Some(format!(
                                    "{} stores; {} water",
                                    s.amount.stores.get(),
                                    s.amount.water.get()
                                )),
                            })
                            .collect(),
                    },
                ),
                field(
                    "stores",
                    "Stores consumed from this source",
                    ActionSchema::Integer {
                        min: 0,
                        max: stores_max.into(),
                    },
                ),
                field(
                    "water",
                    "Water consumed from this source",
                    ActionSchema::Integer {
                        min: 0,
                        max: water_max.into(),
                    },
                ),
            ],
        }),
    }
}
pub(super) fn draws(values: Vec<RationDraw>) -> Result<Vec<SupplyDraw>, Rejection> {
    values
        .into_iter()
        .map(|draw| {
            Ok(SupplyDraw {
                source: serde_json::from_str(&draw.source)
                    .map_err(|_| illegal("unknown supply source"))?,
                amount: SupplyDemand {
                    stores: StoresPoints::new(draw.stores),
                    water: WaterPoints::new(draw.water),
                    ..SupplyDemand::default()
                },
            })
        })
        .collect()
}

/// Cases: airlog:51.11, airlog:51.13, airlog:51.15, airlog:51.23, airlog:52.6, land:3.6
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    let side = pending.seat.side;
    if pending.kind == KIND {
        let selected = if action.is_null() {
            "done"
        } else {
            action
                .as_str()
                .ok_or_else(|| illegal("select a unit or done"))?
        };
        if selected == "done" {
            finalize(content, state, side).map_err(Rejection::Engine)?;
            return Ok(
                "Stores distribution finished; remaining shortages recorded privately.".into(),
            );
        }
        let id = UnitId::new(selected);
        if !eligible(content, state, side)
            .map_err(|e| Rejection::Engine(engine(e)))?
            .contains(&id)
        {
            return Err(illegal("unit is not eligible for this week's stores"));
        }
        let required = rations::stores_required(content, state, &id)
            .map_err(|e| Rejection::Engine(engine(e)))?;
        let sources = available_sources_with_content(content, state, &id)
            .map_err(|e| Rejection::Engine(engine(e)))?;
        let schema = ActionSchema::Record {
            fields: vec![
                field(
                    "stores",
                    "Total stores issued, up to the weekly requirement",
                    ActionSchema::Integer {
                        min: 0,
                        max: required.into(),
                    },
                ),
                field(
                    "half",
                    "Use legal half rations only when local stores cannot cover the full ration",
                    ActionSchema::Bool,
                ),
                field(
                    "pasta",
                    "Include one extra water for an Italian battalion receiving stores",
                    ActionSchema::Bool,
                ),
                field(
                    "draws",
                    "Explicit withdrawals must exactly match the issue",
                    draw_schema(&sources, required, 1),
                ),
            ],
        };
        open(
            state,
            cx,
            pending.seat,
            &format!("{ISSUE_PREFIX}{id}"),
            format!("{id} requires {required} stores. Choose its ration and the sources."),
            &["airlog:51.11", "airlog:51.23", "airlog:52.6", "land:3.6"],
            Trigger::Scheduled,
            Secrecy::Secret,
            ActionSpace::new(schema)
                .with_pass("Leave this unit short of stores and return to unit selection"),
        );
        return Ok(format!("Selected {id} for stores."));
    }
    let id = UnitId::new(
        pending
            .kind
            .strip_prefix(ISSUE_PREFIX)
            .ok_or_else(|| illegal("unknown stores decision"))?,
    );
    if !eligible(content, state, side)
        .map_err(|e| Rejection::Engine(engine(e)))?
        .contains(&id)
    {
        return Err(illegal("unit is no longer eligible"));
    }
    if action.is_null() {
        issue_unit(content, state, &id, 0, false, false, &[])?;
    } else {
        let issue: Issue =
            serde_json::from_value(action.clone()).map_err(|_| illegal("invalid stores issue"))?;
        issue_unit(
            content,
            state,
            &id,
            issue.stores,
            issue.half,
            issue.pasta,
            &draws(issue.draws)?,
        )?;
    }
    open_menu(content, state, side, cx).map_err(Rejection::Engine)?;
    Ok(format!("{id}: stores allocation recorded privately."))
}

/// Issue once per unit per week. Partial full rations are recorded as shortages;
/// legal completed half rations satisfy food consumption but restrict movement.
/// Cases: airlog:51.11, airlog:51.13, airlog:51.15, airlog:51.21, airlog:51.23, airlog:52.6
/// Interpretations: interp:airlog-0007
pub fn issue_unit(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    stores: i32,
    half: bool,
    pasta: bool,
    allocation: &[SupplyDraw],
) -> Result<(), Rejection> {
    let side = state
        .land
        .units
        .get(id)
        .ok_or_else(|| illegal("unknown unit"))?
        .side;
    if !eligible(content, state, side)
        .map_err(|e| Rejection::Engine(engine(e)))?
        .contains(id)
    {
        return Err(illegal("unit already supplied or not in play"));
    }
    let full =
        rations::stores_required(content, state, id).map_err(|e| Rejection::Engine(engine(e)))?;
    let mut required = full;
    if half {
        if matches!(
            rations::class(content, id)
                .map_err(|e| Rejection::Engine(engine(e)))?
                .unit_type
                .as_str(),
            "headquarters" | "engineer"
        ) {
            return Err(illegal(
                "flat-rate HQ and engineer rations cannot be halved",
            ));
        }
        let available = available_sources_with_content(content, state, id)
            .map_err(|e| Rejection::Engine(engine(e)))?
            .iter()
            .map(|s| i64::from(s.amount.stores.get()))
            .sum::<i64>();
        if available >= i64::from(full) {
            return Err(illegal("half rations require a local shortage"));
        }
        required /= 2;
        if stores != required {
            return Err(illegal(
                "half rations must provide two stores per TOE point",
            ));
        }
    }
    if stores < 0 || stores > required {
        return Err(illegal("stores exceed the unit's requirement"));
    }
    if pasta && (!rations::pasta(content, id) || stores == 0) {
        return Err(illegal(
            "pasta water requires an Italian battalion receiving stores",
        ));
    }
    let water = i32::from(pasta);
    spend_for_unit_with_content(
        content,
        state,
        id,
        SupplyDemand {
            stores: StoresPoints::new(stores),
            water: WaterPoints::new(water),
            ..SupplyDemand::default()
        },
        allocation,
    )
    .map_err(|_| illegal("allocation cannot be drawn from these friendly sources"))?;
    let history = state.logistics.rations.entry(id.clone()).or_default();
    history.issued_gt = Some(state.cursor.game_turn);
    history.stores_received = stores;
    history.stores_required = required;
    history.half = half;
    if pasta {
        rations::receive_pasta(state, id);
    }
    Ok(())
}

/// Finalize once per GT; absent issues are shortages, not free full rations.
/// Cases: airlog:51.21, airlog:51.22, airlog:52.6
/// Interpretations: interp:airlog-0007
pub fn finalize(content: &CnaContent, state: &mut State, side: Side) -> Result<(), EngineError> {
    let gt = state.cursor.game_turn;
    let ids: Vec<_> = state
        .land
        .units
        .values()
        .filter(|u| u.side == side && rations::in_play(&u.location))
        .map(|u| u.id.clone())
        .collect();
    for id in ids {
        let full = rations::stores_required(content, state, &id).map_err(engine)?;
        if full == 0 {
            continue;
        }
        let r = state.logistics.rations.entry(id.clone()).or_default();
        if r.finalized_gt == Some(gt) {
            continue;
        }
        if r.issued_gt != Some(gt) {
            r.issued_gt = Some(gt);
            r.stores_required = full;
            r.stores_received = 0;
            r.half = false;
        }
        r.finalized_gt = Some(gt);
        if r.stores_received < r.stores_required {
            r.consecutive_short_gt = if r.last_short_gt == gt.checked_sub(1) {
                r.consecutive_short_gt.saturating_add(1)
            } else {
                1
            };
            r.last_short_gt = Some(gt);
            let unit = state.land.units.get_mut(&id).unwrap();
            unit.cohesion_quarters = unit
                .cohesion_quarters
                .checked_sub(4)
                .ok_or_else(|| engine(SupplyError::Invalid))?;
            if let Some(saved) = r.pasta_saved_cohesion_quarters.as_mut() {
                *saved -= 4;
            }
        } else {
            r.consecutive_short_gt = 0;
            r.last_short_gt = None;
        }
        rations::apply_pasta(content, state, &id);
    }
    Ok(())
}

/// Mandatory feeding aggregates prisoner points in a hex before rounding. Nearest
/// friendly active dumps are used only after the local stores are exhausted.
/// Cases: airlog:51.12, airlog:51.17, land:28.15, land:3.6
/// Interpretations: interp:airlog-0007
pub fn feed_prisoners(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
    guards: bool,
) -> Result<(), EngineError> {
    let stage = WaterStage::current(state);
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    for (id, p) in &state.logistics.prisoners {
        if p.prisoner_points < 0 || p.guard_points < 0 {
            return Err(engine(SupplyError::Invalid));
        }
        if p.food_stage != Some(stage) {
            groups
                .entry(format!(
                    "{:?}:{}",
                    p.owner,
                    serde_json::to_string(&p.location).unwrap()
                ))
                .or_default()
                .push(id.clone());
        }
    }
    for ids in groups.values() {
        let first = &state.logistics.prisoners[&ids[0]];
        let (side, location) = (first.owner, first.location.clone());
        let points: i64 = ids
            .iter()
            .map(|id| i64::from(state.logistics.prisoners[id].prisoner_points))
            .sum();
        let needed = i32::try_from((points + 4) / 5).map_err(|_| engine(SupplyError::Invalid))?;
        let short = draw_special_stores(content, state, side, &location, needed)?;
        for (index, id) in ids.iter().enumerate() {
            let p = state.logistics.prisoners.get_mut(id).unwrap();
            p.food_stage = Some(stage);
            p.stores_short = if index == 0 { short } else { 0 };
        }
        cx.emit(EngineEvent::new(
            Audience::Side(side),
            GameEvent::Note {
                text: format!("Prisoners received {} of {needed} stores.", needed - short),
            },
        ));
    }
    if guards {
        let ids: Vec<_> = state.logistics.prisoners.keys().cloned().collect();
        for id in ids {
            let p = &state.logistics.prisoners[&id];
            if p.guards_fed_gt == Some(stage.game_turn) {
                continue;
            }
            let (side, location) = (p.owner, p.location.clone());
            let needed = p
                .guard_points
                .checked_mul(2)
                .ok_or_else(|| engine(SupplyError::Invalid))?;
            let short = draw_special_stores(content, state, side, &location, needed)?;
            let p = state.logistics.prisoners.get_mut(&id).unwrap();
            p.guards_fed_gt = Some(stage.game_turn);
            p.guards_stores_short = short;
            cx.emit(EngineEvent::new(
                Audience::Side(side),
                GameEvent::Note {
                    text: format!("Guards received {} of {needed} stores.", needed - short),
                },
            ));
        }
    }
    Ok(())
}
fn same_dump(location: &Location, dump: &DumpLocation) -> bool {
    match (location, dump) {
        (Location::Hex { hex: a }, DumpLocation::Hex { hex: b }) => a == b,
        (Location::OffMap { id: a }, DumpLocation::OffMap { id: b }) => a == b,
        _ => false,
    }
}
/// The nearest-dump exception is confined to prisoners and guards.
/// Cases: airlog:51.12, airlog:51.17
fn draw_special_stores(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    location: &Location,
    needed: i32,
) -> Result<i32, EngineError> {
    let mut remaining = needed;
    for (id, holding) in &mut state.logistics.unit_supply {
        if state.land.units.get(id).is_some_and(|u| {
            u.side == side
                && &u.location == location
                && u.trucks.light + u.trucks.medium + u.trucks.heavy > 0
        }) {
            let n = remaining.min(holding.carried.stores).max(0);
            holding.carried.stores -= n;
            remaining -= n;
        }
    }
    let mut dumps: Vec<_> = state
        .logistics
        .dumps
        .iter()
        .filter(|(_, d)| d.side == side && d.active && !d.dummy && d.supplies.stores > 0)
        .filter_map(|(id, d)| {
            if same_dump(location, &d.location) {
                Some((0, id.clone()))
            } else if let (Location::Hex { hex: a }, DumpLocation::Hex { hex: b }) =
                (location, &d.location)
            {
                Some((
                    content
                        .map
                        .get(a)?
                        .axial
                        .distance(content.map.get(b)?.axial)
                        + 1,
                    id.clone(),
                ))
            } else {
                None
            }
        })
        .collect();
    dumps.sort();
    for (_, id) in dumps {
        if remaining == 0 {
            break;
        }
        let stock = &mut state.logistics.dumps.get_mut(&id).unwrap().supplies.stores;
        let n = remaining.min(*stock);
        *stock -= n;
        remaining -= n;
    }
    Ok(remaining)
}

#[cfg(test)]
mod tests;
