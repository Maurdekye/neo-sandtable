//! First Graziani Axis FIELD slice: one tank marker per hex, returned as typed receipts.
//! Other vehicle procedures and ancillary holding partitions remain explicit gaps.
use super::{payloads::*, plans::*, transaction};
use crate::{CnaContent, State, land::{breakdown::Equipment, replacements::receipts, zoc},
    logistics::{self, SupplyDemand, SupplyDraw, SupplyError, SupplySource, water::WaterStage},
    state::{Location, Pending}, steps::{illegal, open}};
use cna_content::scenario::Supplies;
use cna_core::{decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection}, event::EngineEvent, ids::{HexId, SeatId},
    quantity::{FuelTenths, StoresPoints}, visibility::Audience};
use cna_protocol::{GameEvent, Role, Side};
use cna_tables::land::{repair::{BrokenRepairResult, RepairLocation, RepairVehicle, VehicleState}, weather::WeatherKind};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const ANCHOR: &str = "opstage.repair.maintenance";
pub(crate) const KIND: &str = "cna.recovery.field_repair";

fn invariant(detail: &str) -> EngineError {
    EngineError::Invariant { detail: format!("field repair: {detail}") }
}
fn gap(case: &str, detail: &str) -> Rejection {
    EngineError::Unsupported { case: case.into(), detail: detail.into() }.into()
}
fn supply(error: SupplyError) -> Rejection {
    transaction::supply_error(logistics::RecoverySupplyError::Supply(error))
}
fn checked_add(a: i32, b: i32) -> Result<i32, Rejection> {
    a.checked_add(b).ok_or_else(|| invariant("supply total overflow").into())
}

/// One fixed phasing Rear Area window, even with no markers or no eligible repairs.
/// Cases: land:22.11, land:22.12, land:22.25, land:3.6
pub(crate) fn enter(state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    let key = transaction::maintenance_key(state)?;
    if state.land.recovery.opportunity.as_ref() == Some(&key) { return Ok(()); }
    if state.land.recovery.opportunity.as_ref().is_some_and(|old|
        old.procedure == RecoveryProcedure::Towing && old.game_turn == key.game_turn
            && old.op_stage == key.op_stage && old.half == key.half && old.phasing == key.phasing)
    {
        // Current towing is unimplemented and cannot move a marker. Do not later
        // activate it and silently treat an unrecorded tow as an untowed vehicle.
        return Err(EngineError::Unsupported { case: "land:22.13".into(),
            detail: "field repair after towing requires its phase eligibility witness".into() });
    }
    let options = state.land.breakdown.markers.values().filter(|m| m.side == key.phasing)
        .map(|m| ChoiceOption { id: m.id.clone(), label: m.id.clone(), detail: None }).collect();
    state.land.recovery = RecoveryProcedureState { opportunity: Some(key.clone()), ..Default::default() };
    let space = ActionSpace::new(ActionSchema::List { min: 0, max: u32::MAX,
        item: Box::new(ActionSchema::Record { fields: vec![
            FieldSchema { name: "marker".into(), doc: "Own marker selected for field repair".into(),
                schema: ActionSchema::Choice { options }, optional: false },
            FieldSchema { name: "points".into(), doc: "Tank TOE points to attempt, prepaid before dice".into(),
                schema: ActionSchema::Integer { min: 1, max: i32::MAX.into() }, optional: false },
        ] }) }).with_pass("Attempt no field repairs.");
    open(state, cx, SeatId::new(key.phasing, Role::RearArea), KIND,
        "Declare field repair attempts; tank receipts are supported, other repair procedures remain unfinished.".into(),
        &["land:22.11", "land:22.12", "land:22.25", "land:22.26"],
        Trigger::Scheduled, Secrecy::Secret, space);
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration { marker: String, points: i32 }

/// Answers select own actual assets and record only the approved plan buffer.
/// Cases: land:22.11, land:22.12, land:22.25, land:22.26
pub(crate) fn answer(c: &CnaContent, s: &mut State, pending: &Pending, action: &Value) -> Result<String, Rejection> {
    let key = transaction::maintenance_key(s)?;
    if pending.kind != KIND || pending.seat != SeatId::new(key.phasing, Role::RearArea)
        || s.land.recovery.opportunity.as_ref() != Some(&key) || s.land.recovery.resolved
        || s.land.recovery.repair_plan.is_some()
    { return Err(illegal("not the current field repair window")); }
    let declarations: Vec<Declaration> = if action.is_null() { vec![] } else {
        serde_json::from_value(action.clone()).map_err(|_| illegal("invalid field repair plan"))?
    };
    let mut plan = RepairPlan::default();
    for selected in declarations {
        let marker = s.land.breakdown.markers.get(&selected.marker)
            .filter(|m| m.side == key.phasing).ok_or_else(|| illegal("unknown own repair marker"))?;
        if marker.assets.len() != 1 || !marker.pool_assets.is_empty() {
            return Err(gap("land:22.25", "mixed repair marker allocation is not implemented"));
        }
        let mut asset = marker.assets[0].clone();
        if selected.points <= 0 || selected.points > asset.points { return Err(illegal("repair points exceed own marker")); }
        asset.points = selected.points;
        plan.markers.push(RepairMarkerPlan { marker: selected.marker, assets: vec![asset], pool_assets: vec![],
            mode: RepairMode::Field, cargo_priority: vec![], return_pool: ReturnPoolTarget::CreateAtSite });
    }
    prepare(c, s, &plan, false, false)?;
    s.land.recovery.repair_plan = Some(plan);
    Ok("Recorded private field repair plan; no supplies spent or dice rolled.".into())
}

struct PreparedField { repair: PreparedRepair, excluded: Vec<String> }

/// Derive every row from this exact plan and current physical markers/content.
/// Truth-side control is queried only after the answer window closes.
/// Cases: land:22.11, land:22.12, land:22.13, land:22.14, land:22.21, land:22.22, land:22.25, land:22.26
fn prepare(c: &CnaContent, s: &State, plan: &RepairPlan, adjudicate: bool, strict: bool) -> Result<PreparedField, Rejection> {
    let actor = transaction::maintenance_key(s)?.phasing;
    if c.scenario.meta.id != "graziani" && !plan.markers.is_empty() {
        return Err(gap("land:22.7", "field repair special-organization modifiers outside Graziani are not implemented"));
    }
    if actor == Side::Commonwealth && !plan.markers.is_empty() {
        return Err(gap("land:22.6", "Commonwealth field repair needs the source-bound TDS modifier adapter"));
    }
    let mut prepared = PreparedRepair { actor, stage: WaterStage::current(s), before: s.clone(),
        attempts: vec![], site_debits: vec![], output_choices: vec![] };
    let mut ids = BTreeSet::new();
    let mut sites = BTreeSet::new();
    let mut excluded = vec![];
    let mut debits = BTreeMap::<HexId, (cna_core::ids::UnitId, i32, i32)>::new();
    for choice in &plan.markers {
        if !ids.insert(&choice.marker) { return Err(illegal("duplicate repair marker")); }
        if !matches!(choice.mode, RepairMode::Field) { return Err(gap("land:22.31", "facility repair is not implemented")); }
        let marker = s.land.breakdown.markers.get(&choice.marker).filter(|m| m.side == actor)
            .ok_or_else(|| illegal("unknown own repair marker"))?;
        if marker.id != choice.marker || c.map.get(&marker.hex).is_none() { return Err(invariant("invalid marker identity/site").into()); }
        if !sites.insert(marker.hex.clone()) {
            return Err(gap("land:22.14", "multiple markers in one repair group need success-allocation support"));
        }
        if marker.assets.len() != 1 || choice.assets.len() != 1 || !marker.pool_assets.is_empty() || !choice.pool_assets.is_empty() {
            return Err(gap("land:22.25", "mixed repair marker allocation is not implemented"));
        }
        let actual = &marker.assets[0];
        let selected = &choice.assets[0];
        if actual.points <= 0 { return Err(invariant("nonpositive marker body").into()); }
        if selected.unit != actual.unit || selected.equipment != actual.equipment || selected.cohort != actual.cohort
            || selected.points <= 0 || selected.points > actual.points { return Err(illegal("repair selection changed")); }
        let Equipment::Weapon(weapon) = &actual.equipment else {
            return Err(gap("land:22.23", "field truck/recce repair allocation is not implemented"));
        };
        let equipment = c.units.weapons.get(weapon).ok_or_else(|| gap("land:22.25", "repair equipment is unresolved"))?;
        if equipment.kind != "tank" { return Err(gap("land:22.25", "this field equipment procedure is not implemented")); }
        if marker.tank_fuel.get() < 0 || marker.activity_water.get() < 0 { return Err(invariant("negative marker reserve").into()); }
        if marker.tank_fuel.get() != 0 || marker.activity_water.get() != 0 || !marker.passengers.is_empty()
            || marker.transport != Default::default() || !marker.fuel_cohorts.is_empty() || !marker.pool_fuel_cohorts.is_empty()
            || marker.paid_truck_water != Default::default()
            || marker.cargo.totals().map_err(supply)? != Supplies::default()
        { return Err(gap("land:22.5", "nontrivial repair holding partitions are not implemented")); }
        let weather = logistics::weather::at_hex(c, s, &marker.hex)?;
        if matches!(weather, WeatherKind::Rainstorm | WeatherKind::Sandstorm) { return Err(illegal("field repair is unavailable in this storm")); }
        let anchor = s.units_of(actor).find(|u| u.location.hex() == Some(&marker.hex))
            .ok_or_else(|| gap("land:22.22", "field repair without a co-located land unit needs the SGSU-only adapter"))?;
        if adjudicate && zoc::controlled(c, s, actor.opponent(), &marker.hex, strict)? {
            excluded.push(choice.marker.clone());
            continue;
        }
        let cost = c.tables.land.vehicle_repair_supply_costs.cost(RepairLocation::Field, VehicleState::BrokenDown, RepairVehicle::Tank)
            .ok_or_else(|| gap("land:22.15", "field tank repair cost is unavailable"))?;
        let fuel = selected.points.checked_mul(cost.supplies.fuel).ok_or_else(|| invariant("fuel demand overflow"))?;
        let stores = selected.points.checked_mul(cost.supplies.stores).ok_or_else(|| invariant("stores demand overflow"))?;
        let demand = demand(fuel, stores)?;
        let old = debits.entry(marker.hex.clone()).or_insert((anchor.id.clone(), 0, 0));
        old.1 = checked_add(old.1, fuel)?; old.2 = checked_add(old.2, stores)?;
        prepared.attempts.push(RepairAttempt { marker: marker.id.clone(), site: Location::Hex { hex: marker.hex.clone() },
            asset: RepairAsset::Unit(selected.clone()), mode: RepairLocation::Field, vehicle_state: VehicleState::BrokenDown,
            vehicle: RepairVehicle::Tank, cost, demand });
        prepared.output_choices.push(RepairOutputChoice { marker: marker.id.clone(), target: choice.return_pool.clone(), cargo_priority: choice.cargo_priority.clone() });
    }
    for (hex, (anchor, fuel, stores)) in debits {
        let site = Location::Hex { hex };
        let demand = demand(fuel, stores)?;
        let mut sources = logistics::available_sources_at_with_content(c, s, &anchor, &site).map_err(supply)?;
        sources.retain(|row| matches!(row.source, SupplySource::Unlimited | SupplySource::Dump(_) | SupplySource::UnitStock(_)));
        sources.sort_by_key(|row| (match row.source { SupplySource::Unlimited => 0, SupplySource::Dump(_) => 1, _ => 2 }, row.source.clone()));
        let mut remaining = demand;
        let mut draws = vec![];
        for row in sources {
            let amount = SupplyDemand { fuel: FuelTenths::new(remaining.fuel.get().min(row.amount.fuel.get())),
                stores: StoresPoints::new(remaining.stores.get().min(row.amount.stores.get())), ..Default::default() };
            if amount.fuel.get() > 0 || amount.stores.get() > 0 { draws.push(SupplyDraw { source: row.source, amount }); }
            remaining.fuel = FuelTenths::new(remaining.fuel.get() - amount.fuel.get());
            remaining.stores = StoresPoints::new(remaining.stores.get() - amount.stores.get());
        }
        if remaining != SupplyDemand::default() { return Err(illegal("insufficient supplies for complete repair plan")); }
        prepared.site_debits.push(RepairSiteDebit { site, demand, draws });
    }
    logistics::prepare_recovery_prepaid(c, s, &prepared).map_err(transaction::supply_error)?;
    Ok(PreparedField { repair: prepared, excluded })
}

fn demand(fuel: i32, stores: i32) -> Result<SupplyDemand, Rejection> {
    if fuel < 0 || stores < 0 { return Err(invariant("negative repair demand").into()); }
    Ok(SupplyDemand { fuel: FuelTenths::new(fuel.checked_mul(10).ok_or_else(|| invariant("fuel demand overflow"))?),
        stores: StoresPoints::new(stores), ..Default::default() })
}

fn accepted(error: Rejection) -> EngineError {
    match error { Rejection::Engine(error) => error, _ => invariant("accepted field plan no longer satisfies own prerequisites") }
}

/// Real outcomes, physical remainder and typed Oob receipts, on the prepaid draft.
/// Cases: land:22.11, land:22.13, land:22.14, land:22.25, land:22.26, land:22.5
/// Interpretations: interp:land-0038, interp:land-0052
pub(crate) fn finish(c: &CnaContent, s: &mut State, cx: &mut Cx<'_>, strict: bool) -> Result<(), EngineError> {
    if s.decisions.pending.iter().any(|p| p.kind == KIND) || s.land.recovery.resolved { return Ok(()); }
    let plan = s.land.recovery.repair_plan.clone().ok_or_else(|| invariant("missing accepted field plan"))?;
    let field = prepare(c, s, &plan, true, strict).map_err(accepted)?;
    let actor = field.repair.actor();
    transaction::finish_prepaid(c, s, cx, &field.repair, |draft, cx| {
        for id in &field.excluded {
            cx.emit(EngineEvent::new(Audience::Side(actor), GameEvent::Note { text: format!("Field repair unavailable at own marker {id}; no attempt or payment made.") }));
        }
        for row in field.repair.attempts() {
            let before = draft.clone();
            let marker = draft.land.breakdown.markers.get(row.marker()).cloned().ok_or_else(|| invariant("repair marker vanished"))?;
            let RepairAsset::Unit(selected) = row.asset() else { return Err(invariant("unexpected pool tank repair").into()); };
            let Equipment::Weapon(weapon) = &selected.equipment else { return Err(invariant("unexpected field equipment").into()); };
            let die = cx.rng.d6();
            let Some(BrokenRepairResult::Percentage(percent)) = c.tables.land.broken_down_vehicle_repair.field_result(RepairVehicle::Tank, i32::from(die.value())) else {
                return Err(invariant("validated field chart has no tank result").into());
            };
            let repaired = percent.repaired_points(selected.points).ok_or_else(|| invariant("repair result overflow"))?;
            let mut after = marker.clone();
            after.assets[0].points -= repaired;
            let after = (after.assets[0].points > 0).then_some(after);
            if let Some(after) = &after { draft.land.breakdown.markers.insert(marker.id.clone(), after.clone()); }
            else { draft.land.breakdown.markers.remove(&marker.id); }
            let transition = ValidatedRepairOrTowOutcome { actor, stage: field.repair.stage(), before,
                after: draft.clone(), cargo_moves: vec![], truck_moves: vec![] };
            let history = logistics::prepare_recovery_relocation(draft, &transition)?;
            logistics::apply_recovery_relocation(draft, &history)?;
            let mut receipt_ids = vec![];
            if repaired > 0 {
                let mut success = selected.clone(); success.points = repaired;
                let outcome = ValidatedRepairOutcome { actor, stage: field.repair.stage(), site: row.site().clone(),
                    markers: vec![MarkerRepairOutcome { before: marker.clone(), after, repaired_unit_assets: vec![success],
                        repaired_pool_assets: vec![], junk_unit_assets: vec![], junk_pool_assets: vec![],
                        receipts: vec![ReceiptReturn { equipment: ReturnedEquipment::Tank { equipment: weapon.clone() }, points: repaired }], returned_trucks: None }] };
                receipt_ids = receipts::insert_replacement_receipts(c, draft, &outcome)?;
            }
            cx.emit(EngineEvent::new(Audience::Side(actor), GameEvent::Note {
                text: format!("Own field repair {} ({weapon}): die {}, {} tank points returned as repair receipts {:?}.", marker.id, die.value(), repaired, receipt_ids),
            }));
        }
        Ok(())
    }).map_err(accepted)?;
    Ok(())
}

#[cfg(test)]
mod tests;
