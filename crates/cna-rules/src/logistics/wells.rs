//! Well operations. Unknown map membership is never a fabricated water source.
//! Railroad/construction procedures populate the dynamic network only after resolving their
//! source data; every pipeline draw rechecks the intact chain back to a major-city source.
use super::stores::{engine, field, option};
use super::{
    capacity::{CargoPacking, cargo_bound, validate_packing},
    rations, water,
};
use crate::{
    CnaContent,
    state::{Location, Pending, State},
    steps::illegal,
};
use cna_content::scenario::Supplies;
use cna_core::{
    decision::ActionSchema,
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{HexId, UnitId},
    quantity::WaterPoints,
    visibility::{Audience, Perspective},
};
use cna_protocol::{GameEvent, Side};
use cna_tables::airlog::{
    supply::{SupplyType, WellAttempt, WellEffect, WellSource},
    trucks::TruckType,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const PREFIX: &str = "cna.logistics.well:";
pub const REQUEST_PREFIX: &str = "cna.logistics.well.draw:";
pub const ALLOCATE_PREFIX: &str = "cna.logistics.well.allocate:";

/// A constructed or subsequently destroyed pipeline hex; static chart values stay in content.
/// Cases: airlog:52.21, airlog:52.24, airlog:52.25
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineHex {
    pub upstream: HexId,
    pub destroyed: bool,
}
/// Water just drawn, awaiting immediate private allocation. This is not transportable stock.
/// Cases: airlog:52.13
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrawnWater {
    pub points: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    MajorCity,
    Oasis,
    Village,
    Bir,
    Pipeline,
    AxisBox,
}
impl Source {
    fn finite(self) -> bool {
        matches!(self, Self::Village | Self::Bir)
    }
}

/// Verified source classification, never inferred from an empty place/line layer.
/// Cases: airlog:52.11, airlog:52.21, airlog:52.22, airlog:52.23, airlog:52.25, airlog:52.3
pub fn source_at(
    content: &CnaContent,
    state: &State,
    side: Side,
    location: &Location,
) -> Result<Source, super::SupplyError> {
    match location {
        Location::OffMap { id } => {
            let boxes = content.areas.areas.get("tripoli_tunisia_boxes");
            if boxes
                .is_some_and(|a| a.membership_status == "resolved" && a.location_ids.contains(id))
            {
                Ok(Source::AxisBox)
            } else {
                Err(super::SupplyError::Unsupported {
                    case: "airlog:52.11",
                })
            }
        }
        Location::Hex { hex } => {
            let hex = content
                .map
                .canonical(hex)
                .ok_or(super::SupplyError::Invalid)?;
            if let Some(source) = place_source(content, hex) {
                return Ok(source);
            }
            if connected_pipeline(content, state, side, hex) {
                return Ok(Source::Pipeline);
            }
            Err(super::SupplyError::Unsupported {
                case: "airlog:52.11",
            })
        }
        _ => Err(super::SupplyError::Invalid),
    }
}
fn place_source(content: &CnaContent, hex: &HexId) -> Option<Source> {
    let places: Vec<_> = content.places.at(hex).map(|p| p.kind.as_str()).collect();
    if places.contains(&"major_city") {
        Some(Source::MajorCity)
    } else if places.contains(&"oasis") {
        Some(Source::Oasis)
    } else if places.contains(&"village") || places.contains(&"town") {
        Some(Source::Village)
    } else if places.contains(&"bir") {
        Some(Source::Bir)
    } else {
        None
    }
}
/// A broken, cyclic, disconnected or unverified source chain provides no pipeline water.
/// The railroad owner records operating/destroyed status after verifying the route.
/// Cases: airlog:52.21, airlog:52.22, airlog:52.23, airlog:52.25
pub fn connected_pipeline(content: &CnaContent, state: &State, side: Side, hex: &HexId) -> bool {
    let mut current = hex.clone();
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(current.clone()) {
            return false;
        }
        if place_source(content, &current) == Some(Source::MajorCity) {
            return true;
        }
        if side == Side::Commonwealth && state.logistics.operating_rail_water.contains(&current) {
            return true;
        }
        let Some(pipe) = state.logistics.pipelines.get(&current) else {
            return false;
        };
        if pipe.destroyed
            || content
                .map
                .get(&current)
                .zip(content.map.get(&pipe.upstream))
                .is_none_or(|(a, b)| a.axial.distance(b.axial) != 1)
        {
            return false;
        }
        current = pipe.upstream.clone();
    }
}

/// The visible condition contains no roll, stock, attempt history or undiscovered condition.
/// Cases: airlog:52.14, airlog:52.16, land:3.6
pub fn condition(state: &State, hex: &HexId, perspective: Perspective) -> Value {
    let Some(well) = state.logistics.wells.get(hex) else {
        return json!({});
    };
    let knows = |sides: &BTreeSet<Side>| {
        sides
            .iter()
            .any(|s| crate::view::sees_side(perspective, *s))
    };
    let mut data = serde_json::Map::new();
    if well.depleted
        && (well.depleted_revealed
            || knows(&well.depleted_known)
            || perspective == Perspective::Operator)
    {
        data.insert("depleted".into(), json!(true));
    }
    if well.poisoned
        && (well.poisoned_revealed
            || knows(&well.poisoned_known)
            || perspective == Perspective::Operator)
    {
        data.insert("poisoned".into(), json!(true));
    }
    Value::Object(data)
}
fn known_poison(state: &State, hex: &HexId, side: Side) -> bool {
    state
        .logistics
        .wells
        .get(hex)
        .is_some_and(|w| w.poisoned && (w.poisoned_revealed || w.poisoned_known.contains(&side)))
}

/// Only verified sources with resolved CPA and composition are offered.
/// Cases: airlog:52.11, airlog:52.13, airlog:52.16, airlog:52.17, land:3.6
pub(super) fn candidates(content: &CnaContent, state: &State, side: Side) -> Vec<UnitId> {
    state
        .land
        .units
        .values()
        .filter(|u| u.side == side)
        .filter(|u| {
            source_at(content, state, side, &u.location).is_ok()
                && !operation_options(content, state, &u.id, side).is_empty()
        })
        .map(|u| u.id.clone())
        .collect()
}
pub(super) fn operation_options(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    side: Side,
) -> Vec<cna_core::decision::ChoiceOption> {
    let Some(unit) = state.land.units.get(id) else {
        return Vec::new();
    };
    let Ok(source) = source_at(content, state, side, &unit.location) else {
        return Vec::new();
    };
    let mut options = Vec::new();
    let bound = cargo_bound(content, state, id, SupplyType::Water).ok();
    let held = state
        .logistics
        .unit_supply
        .get(id)
        .map_or(0, |h| h.carried.water);
    let room = bound
        .zip(required(content, state, id).ok())
        .is_some_and(|(bound, need)| i64::from(need) + i64::from((bound - held).max(0)) > 0);
    if room && charge_plan(content, state, id, 4, false).is_ok() {
        options.push(option("draw".into(), "Spend 1 CP and draw water".into()));
    }
    if source.finite() {
        let hex = unit.location.hex().expect("finite well");
        let failed = state
            .logistics
            .wells
            .get(hex)
            .and_then(|w| w.poison_failed_stage.get(&side))
            .copied()
            == Some(water::WaterStage::current(state));
        if !failed && charge_plan(content, state, id, 4, false).is_ok() {
            options.push(option(
                "poison".into(),
                "Spend 1 CP and attempt to poison this well".into(),
            ));
        }
        if known_poison(state, hex, side) && charge_plan(content, state, id, 20, true).is_ok() {
            options.push(option(
                "sweeten".into(),
                "Spend 5 CP and attempt to restore this poisoned well".into(),
            ));
        }
    }
    options
}

fn charge_plan(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    cost: i32,
    limited: bool,
) -> Result<crate::state::LandUnit, Rejection> {
    let mut unit = state
        .land
        .units
        .get(id)
        .ok_or_else(|| illegal("unknown unit"))?
        .clone();
    let allowance =
        crate::land::formation::individual_allowance(content, state, id).ok_or_else(|| {
            Rejection::Engine(EngineError::Unsupported {
                case: "land:6.15".into(),
                detail: "well operation requires a resolved capability allowance".into(),
            })
        })?;
    let limits = rations::movement_restrictions(content, state, id)
        .map_err(|e| Rejection::Engine(engine(e)))?;
    if (limited || !limits.may_exceed_cpa)
        && i64::from(unit.cp_spent_quarters) + i64::from(cost) > i64::from(allowance.cpa) * 4
    {
        return Err(illegal("well operation would exceed the unit's CPA"));
    }
    crate::land::capability::charge(&mut unit, allowance, cost, false)?;
    Ok(unit)
}
fn required(content: &CnaContent, state: &State, id: &UnitId) -> Result<i32, Rejection> {
    let r = water::requirements(content, state, id).map_err(|e| Rejection::Engine(engine(e)))?;
    r.infantry
        .checked_add(r.activity)
        .and_then(|n| n.checked_add(r.pasta))
        .ok_or_else(|| illegal("water requirement overflow"))
}

/// Private operation menu; hidden enemy conditions do not remove the draw option.
/// Cases: airlog:52.13, airlog:52.14, airlog:52.16, airlog:52.17, land:3.6

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub infantry: i32,
    pub activity: i32,
    pub pasta: bool,
    pub cargo: i32,
    pub packing: CargoPacking,
}

/// CP and storage are checked before any roll. Known bad wells still cost the
/// printed CP when probed; an opponent's first attempt reveals only the condition.
/// A dry vehicle may operate the well; activity water is consumed when it becomes available.
/// Cases: airlog:52.13, airlog:52.14, airlog:52.16, airlog:52.42
pub fn draw(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    requested: i32,
    packing: &CargoPacking,
    cx: &mut Cx<'_>,
) -> Result<i32, Rejection> {
    let mut draft = state.clone();
    prepare_draw(content, &mut draft, id, requested, packing)?;
    let actual = resolve_draw(content, &mut draft, id, requested, cx).map_err(Rejection::Engine)?;
    *state = draft;
    Ok(actual)
}
pub(super) fn prepare_draw(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    requested: i32,
    packing: &CargoPacking,
) -> Result<(), Rejection> {
    if requested <= 0 || state.logistics.drawn_water.contains_key(id) {
        return Err(illegal(
            "draw quantity must be positive and the previous draw must be allocated",
        ));
    }
    let unit = state
        .land
        .units
        .get(id)
        .ok_or_else(|| illegal("unknown unit"))?;
    let side = unit.side;
    let location = unit.location.clone();
    let _source =
        source_at(content, state, side, &location).map_err(|e| Rejection::Engine(engine(e)))?;
    let charged = charge_plan(content, state, id, 4, false)?;
    let mut expected = state
        .logistics
        .unit_supply
        .get(id)
        .map_or(Supplies::default(), |h| h.carried);
    expected.water = expected
        .water
        .checked_add((requested - required(content, state, id)?).max(0))
        .ok_or_else(|| illegal("water overflow"))?;
    validate_packing(
        content,
        &unit.trucks,
        &unit.transport_trucks,
        &expected,
        packing,
    )
    .map_err(|_| illegal("requested water cannot be carried after supplying this unit"))?;
    let mut next = state.clone();
    next.land.units.insert(id.clone(), charged);
    super::consume_activity_water_forced(content, &mut next, id)
        .map_err(|e| Rejection::Engine(engine(e)))?;
    *state = next;
    Ok(())
}
/// Resolve an accepted draw after the secret window closes; CP was already reserved.
/// Cases: airlog:52.13, airlog:52.14, airlog:52.16
pub(super) fn resolve_draw(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    requested: i32,
    cx: &mut Cx<'_>,
) -> Result<i32, EngineError> {
    let unit = state
        .land
        .units
        .get(id)
        .ok_or_else(|| engine(super::SupplyError::Invalid))?;
    let side = unit.side;
    let location = unit.location.clone();
    let source = source_at(content, state, side, &location).map_err(engine)?;
    let mut next = state.clone();
    let mut actual = requested;
    if source.finite() {
        let hex = location.hex().expect("finite well").clone();
        let well = next.logistics.wells.entry(hex.clone()).or_default();
        if well.depleted || well.poisoned {
            // The public disclosure is specifically required by the attempted draw.
            reveal(well, &hex, side, cx);
            actual = 0;
        } else {
            let die = private_die(cx, side, "Well water draw", "airlog:52.13");
            let kind = if source == Source::Village {
                WellSource::Town
            } else {
                WellSource::Bir
            };
            let result = content.tables.airlog.water_availability.draw(kind, die);
            actual = actual.min(result.water.get());
            if result.depletion_check {
                let check = private_die(cx, side, "Well depletion check", "airlog:52.7");
                if check == content.tables.airlog.water_availability.depleted_on() {
                    well.depleted = true;
                    well.depleted_known.insert(side);
                }
            }
        }
    }
    next.logistics
        .drawn_water
        .insert(id.clone(), DrawnWater { points: actual });
    *state = next;
    cx.emit(EngineEvent::new(
        Audience::Side(side),
        GameEvent::Note {
            text: format!("{id}: drew {actual} water after spending 1 CP."),
        },
    ));
    Ok(actual)
}
fn pay_if_available(content: &CnaContent, state: &mut State, id: &UnitId) -> Result<(), Rejection> {
    match rations::spend_activity_water(content, state, id) {
        Ok(()) | Err(super::SupplyError::Insufficient) => Ok(()),
        Err(e) => Err(Rejection::Engine(engine(e))),
    }
}
fn private_die(cx: &mut Cx<'_>, side: Side, purpose: &str, rule: &str) -> cna_core::dice::Die {
    let die = cx.rng.d6();
    cx.emit(EngineEvent::new(
        Audience::Side(side),
        GameEvent::DiceRolled {
            purpose: purpose.into(),
            dice: vec![die.value()],
            reading: None,
            rule: Some(rule.into()),
        },
    ));
    die
}
/// Both conditions can be revealed by the same failed draw, but nothing else is public.
/// Cases: airlog:52.14, airlog:52.16
fn reveal(well: &mut crate::state::WellState, hex: &HexId, side: Side, cx: &mut Cx<'_>) {
    for (condition, active, known, revealed) in [
        (
            "depleted",
            well.depleted,
            &mut well.depleted_known,
            &mut well.depleted_revealed,
        ),
        (
            "poisoned",
            well.poisoned,
            &mut well.poisoned_known,
            &mut well.poisoned_revealed,
        ),
    ] {
        if active {
            let opposing_discovery = !known.is_empty() && !known.contains(&side);
            known.insert(side);
            if opposing_discovery && !*revealed {
                *revealed = true;
                cx.emit(EngineEvent::public(GameEvent::Note {
                    text: format!("{hex}: a {condition} well was discovered."),
                }));
            }
        }
    }
}

/// Immediate allocation of the recorded result. The owning side chooses who
/// drinks and what is reserved/carried; excess may be left unused, not transported.
/// Cases: airlog:52.13, airlog:52.41, airlog:52.42, airlog:52.6, airlog:54.2
pub fn allocate(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    allocation: &Allocation,
) -> Result<(), Rejection> {
    let available = state
        .logistics
        .drawn_water
        .get(id)
        .ok_or_else(|| illegal("there is no unallocated well draw"))?
        .points;
    let r = water::requirements(content, state, id).map_err(|e| Rejection::Engine(engine(e)))?;
    if allocation.infantry < 0
        || allocation.infantry > r.infantry
        || allocation.activity < 0
        || allocation.activity > r.activity
        || allocation.cargo < 0
        || (allocation.pasta && r.pasta != 1)
    {
        return Err(illegal("water allocation exceeds its needs"));
    }
    let total = allocation
        .infantry
        .checked_add(allocation.activity)
        .and_then(|n| n.checked_add(i32::from(allocation.pasta)))
        .and_then(|n| n.checked_add(allocation.cargo))
        .ok_or_else(|| illegal("water overflow"))?;
    if total > available {
        return Err(illegal("allocation exceeds the recorded draw"));
    }
    let unit = &state.land.units[id];
    let mut holding = state
        .logistics
        .unit_supply
        .get(id)
        .cloned()
        .unwrap_or_default();
    holding.carried.water = holding
        .carried
        .water
        .checked_add(allocation.cargo)
        .ok_or_else(|| illegal("water overflow"))?;
    validate_packing(
        content,
        &unit.trucks,
        &unit.transport_trucks,
        &holding.carried,
        &allocation.packing,
    )
    .map_err(|_| illegal("water cargo exceeds carrying capacity"))?;
    holding.activity_water = WaterPoints::new(
        holding
            .activity_water
            .get()
            .checked_add(allocation.activity)
            .ok_or_else(|| illegal("water reserve overflow"))?,
    );
    let mut next = state.clone();
    next.logistics.unit_supply.insert(id.clone(), holding);
    let stage = water::WaterStage::current(state);
    let history = next.logistics.rations.entry(id.clone()).or_default();
    if history.water_stage != Some(stage) {
        history.infantry_water_received = 0;
    }
    history.water_stage = Some(stage);
    history.infantry_water_received += allocation.infantry;
    if allocation.pasta {
        rations::receive_pasta(&mut next, id);
    }
    pay_if_available(content, &mut next, id)?;
    next.logistics.drawn_water.remove(id);
    *state = next;
    Ok(())
}

/// Failed poisoning attempts cannot be repeated by that side in the same stage.
/// Sweetening may be repeated within CPA. Public markers clear when their condition clears.
/// Cases: airlog:52.16, airlog:52.17, airlog:52.8
/// Interpretations: interp:airlog-0009
pub fn attempt(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    sweeten: bool,
    cx: &mut Cx<'_>,
) -> Result<(), Rejection> {
    let mut draft = state.clone();
    prepare_attempt(content, &mut draft, id, sweeten)?;
    resolve_attempt(content, &mut draft, id, sweeten, cx).map_err(Rejection::Engine)?;
    *state = draft;
    Ok(())
}
pub(super) fn prepare_attempt(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    sweeten: bool,
) -> Result<(), Rejection> {
    let unit = state
        .land
        .units
        .get(id)
        .ok_or_else(|| illegal("unknown unit"))?;
    let side = unit.side;
    let hex = unit
        .location
        .hex()
        .ok_or_else(|| illegal("no finite well here"))?
        .clone();
    if !source_at(content, state, side, &unit.location)
        .map_err(|e| Rejection::Engine(engine(e)))?
        .finite()
    {
        return Err(illegal("this water source cannot be poisoned"));
    }
    let stage = water::WaterStage::current(state);
    if sweeten && !known_poison(state, &hex, side) {
        return Err(illegal("no known poisoned well here"));
    }
    if !sweeten
        && state
            .logistics
            .wells
            .get(&hex)
            .and_then(|w| w.poison_failed_stage.get(&side))
            .copied()
            == Some(stage)
    {
        return Err(illegal(
            "this side has already failed a poisoning attempt at this well this stage",
        ));
    }
    let charged = charge_plan(content, state, id, if sweeten { 20 } else { 4 }, sweeten)?;
    let mut next = state.clone();
    next.land.units.insert(id.clone(), charged);
    super::consume_activity_water_forced(content, &mut next, id)
        .map_err(|e| Rejection::Engine(engine(e)))?;
    *state = next;
    Ok(())
}
/// Resolve accepted poisoning/sweetening without rechecking a changed opposing condition.
/// Cases: airlog:52.16, airlog:52.17, airlog:52.8
pub(super) fn resolve_attempt(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    sweeten: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let unit = state
        .land
        .units
        .get(id)
        .ok_or_else(|| engine(super::SupplyError::Invalid))?;
    let side = unit.side;
    let hex = unit
        .location
        .hex()
        .ok_or_else(|| engine(super::SupplyError::Invalid))?
        .clone();
    let stage = water::WaterStage::current(state);
    let mut next = state.clone();
    let die = private_die(
        cx,
        side,
        if sweeten {
            "Well sweetening attempt"
        } else {
            "Well poisoning attempt"
        },
        "airlog:52.8",
    );
    let action = if sweeten {
        WellAttempt::SweetenWell
    } else {
        WellAttempt::PoisonWell
    };
    let result = content
        .tables
        .airlog
        .poisoning_and_sweetening
        .result(action, die);
    let well = next.logistics.wells.entry(hex.clone()).or_default();
    match result {
        WellEffect::WellPoisoned => {
            well.poisoned = true;
            well.poisoned_known.insert(side);
        }
        WellEffect::WellSweetened => {
            if well.poisoned_revealed {
                cx.emit(EngineEvent::public(GameEvent::Note {
                    text: format!("{hex}: the known poisoned well was restored."),
                }));
            }
            well.poisoned = false;
            well.poisoned_known.clear();
            well.poisoned_revealed = false;
        }
        WellEffect::NoEffect => {
            well.poison_failed_stage.insert(side, stage);
        }
        WellEffect::NoEffectStillPoisoned => {}
    }
    *state = next;
    cx.emit(EngineEvent::new(
        Audience::Side(side),
        GameEvent::Note {
            text: format!("{id}: well attempt result {result:?}."),
        },
    ));
    Ok(())
}

pub(super) fn packing_schema(content: &CnaContent, state: &State, id: &UnitId) -> ActionSchema {
    let unit = &state.land.units[id];
    let fields = [
        ("light", TruckType::Light),
        ("medium", TruckType::Medium),
        ("heavy", TruckType::Heavy),
    ]
    .into_iter()
    .map(|(name, kind)| {
        let truck_count = super::capacity::trucks(&unit.trucks, kind).max(0);
        let row = content.tables.airlog.truck_characteristics.truck(kind);
        let supplies = [
            ("ammo", SupplyType::Ammo),
            ("fuel", SupplyType::Fuel),
            ("stores", SupplyType::Stores),
            ("water", SupplyType::Water),
        ]
        .into_iter()
        .map(|(n, t)| {
            field(
                n,
                "Total final cargo assigned to this truck type",
                ActionSchema::Integer {
                    min: 0,
                    max: i64::from(truck_count) * i64::from(row.supply_capacity(t)),
                },
            )
        })
        .collect();
        field(
            name,
            "Final packing by truck type",
            ActionSchema::Record { fields: supplies },
        )
    })
    .collect();
    ActionSchema::Record { fields }
}

/// Cases: airlog:52.13, airlog:52.14, airlog:52.16, airlog:52.17, land:3.6
pub fn answer(
    _content: &CnaContent,
    _state: &mut State,
    _pending: &Pending,
    _action: &Value,
    _cx: &mut Cx<'_>,
    _strict: bool,
) -> Result<String, Rejection> {
    Err(illegal(
        "single-unit well windows are retired; use the fixed batched water step",
    ))
}
#[cfg(test)]
mod tests;
