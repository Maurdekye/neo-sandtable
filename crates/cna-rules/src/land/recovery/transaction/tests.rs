//! Transaction controls. The fixture represents an already trusted prepared attempt;
//! it is not a test of the still-missing gameplay eligibility/outcome producer.
use super::*;
use crate::land::{breakdown::{Asset, Equipment, markers::BrokenMarker}, recovery::{
    RepairPlan, ReturnPoolTarget,
    payloads::{RepairAsset, RepairAttempt, RepairSiteDebit},
    plans::{RepairMarkerPlan, RepairMode},
}};
use crate::logistics::{SupplyDemand, SupplyDraw, SupplySource, water::WaterStage};
use crate::seq::{Block, Half, PLAYER_HALF};
use crate::state::{DumpLocation, Location};
use cna_content::units::Trucks;
use cna_core::{dice::CampaignRng, event::EngineEvent, quantity::FuelTenths, visibility::Audience};
use cna_protocol::{GameEvent, Side};
use cna_tables::land::repair::{RepairLocation, RepairVehicle, VehicleState};
use std::cell::Cell;

fn fixture() -> (CnaContent, State, PreparedRepair) {
    let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut state = State::new(&content).unwrap();
    state.cursor.block = Block::PlayerHalf;
    state.cursor.index = PLAYER_HALF.iter().position(|s| s.anchor == "opstage.repair.maintenance").unwrap();
    state.cursor.op_stage = Some(1);
    state.cursor.half = Some(Half::A);
    state.turn.player_a = Some(Side::Axis);
    let unit = state.land.units.values().find(|u| u.side == Side::Axis).unwrap().id.clone();
    let weapon = content.units.weapons.iter().find(|(_, w)| w.kind == "tank").unwrap().0.clone();
    let cost = content.tables.land.vehicle_repair_supply_costs.cost(
        RepairLocation::Field, VehicleState::BrokenDown, RepairVehicle::Tank,
    ).unwrap();
    assert_eq!((cost.supplies.fuel, cost.supplies.stores), (1, 0));
    let mut attempts = Vec::new();
    let mut site_debits = Vec::new();
    let mut plan = RepairPlan::default();
    for (id, hex, dump_id) in [("repair-a", "C4807", "ax_dump_1"), ("repair-b", "C4808", "ax_dump_2")] {
        let hex = hex.into();
        assert!(content.map.get(&hex).is_some());
        let asset = Asset { unit: unit.clone(), equipment: Equipment::Weapon(weapon.clone()), points: 1, cohort: None };
        let marker = BrokenMarker {
            id: id.into(), side: Side::Axis, hex: hex.clone(), assets: vec![asset.clone()],
            source_pool: None, pool_assets: vec![], pool_fuel_cohorts: vec![],
            passengers: Default::default(), transport: Trucks::default(), cargo: Default::default(),
            tank_fuel: Default::default(), activity_water: Default::default(), fuel_cohorts: vec![],
            paid_truck_water: Default::default(), water_credit_stage: None,
        };
        state.land.breakdown.markers.insert(id.into(), marker);
        let dump = state.logistics.dumps.get_mut(dump_id).unwrap();
        dump.side = Side::Axis;
        dump.active = true;
        dump.dummy = false;
        dump.location = DumpLocation::Hex { hex: hex.clone() };
        dump.supplies.fuel = 3;
        let site = Location::Hex { hex };
        let demand = SupplyDemand { fuel: FuelTenths::new(10), ..Default::default() };
        attempts.push(RepairAttempt {
            marker: id.into(), site: site.clone(), asset: RepairAsset::Unit(asset.clone()),
            mode: RepairLocation::Field, vehicle_state: VehicleState::BrokenDown,
            vehicle: RepairVehicle::Tank, cost, demand,
        });
        site_debits.push(RepairSiteDebit {
            site, demand, draws: vec![SupplyDraw { source: SupplySource::Dump(dump_id.into()), amount: demand }],
        });
        plan.markers.push(RepairMarkerPlan {
            marker: id.into(), assets: vec![asset], pool_assets: vec![], mode: RepairMode::Field,
            cargo_priority: vec![], return_pool: ReturnPoolTarget::CreateAtSite,
        });
    }
    state.land.recovery.opportunity = Some(maintenance_key(&state).unwrap());
    state.land.recovery.repair_plan = Some(plan);
    let prepared = PreparedRepair {
        actor: Side::Axis, stage: WaterStage::current(&state), before: state.clone(),
        attempts, site_debits, output_choices: vec![],
    };
    (content, state, prepared)
}

fn prefix() -> Vec<EngineEvent> {
    vec![EngineEvent::new(Audience::Side(Side::Commonwealth), GameEvent::Note { text: "prior event".into() })]
}

fn snapshot(state: &State, rng: &CampaignRng, events: &[EngineEvent]) -> serde_json::Value {
    serde_json::to_value((state, rng.state(), events)).unwrap()
}

/// Cases: land:22.26, land:22.35, land:22.42
#[test]
fn all_sites_are_debited_before_the_continuation_can_roll() {
    let (content, mut state, prepared) = fixture();
    let mut rng = CampaignRng::from_seed([71; 32]);
    let mut events = prefix();
    let result = finish_prepaid(&content, &mut state, &mut Cx { rng: &mut rng, events: &mut events }, &prepared,
        |draft, cx| {
            for id in ["ax_dump_1", "ax_dump_2"] {
                assert_eq!(draft.logistics.dumps[id].supplies.fuel, 2);
            }
            Ok(cx.rng.d6().value())
        }).unwrap();
    assert!(result.is_some());
    assert!(state.land.recovery.resolved);
    assert!(state.land.recovery.repair_plan.is_none());
    assert_eq!(serde_json::to_value(&events).unwrap(), serde_json::to_value(prefix()).unwrap());
}

/// Cases: land:22.26, land:22.42
#[test]
fn last_site_shortage_enters_no_continuation_and_preserves_everything() {
    let (content, mut state, mut prepared) = fixture();
    state.logistics.dumps.get_mut("ax_dump_2").unwrap().supplies.fuel = 0;
    prepared.before = state.clone();
    let mut rng = CampaignRng::from_seed([72; 32]);
    let mut events = prefix();
    let before = snapshot(&state, &rng, &events);
    let called = Cell::new(false);
    let result = finish_prepaid(&content, &mut state, &mut Cx { rng: &mut rng, events: &mut events }, &prepared,
        |_, _| { called.set(true); Ok(()) });
    assert!(matches!(result, Err(Rejection::Illegal { .. })));
    assert!(!called.get());
    assert_eq!(snapshot(&state, &rng, &events), before);
}

/// Cases: land:22.26, land:22.42, land:22.43
#[test]
fn late_error_after_payment_dice_and_outputs_rolls_back_full_state_rng_and_events() {
    for error in [
        EngineError::Unsupported { case: "airlog:53.25".into(), detail: "exact late history cause".into() },
        EngineError::Invariant { detail: "exact late physical cause".into() },
    ] {
        let (content, mut state, prepared) = fixture();
        let mut rng = CampaignRng::from_seed([73; 32]);
        let mut events = prefix();
        let before = snapshot(&state, &rng, &events);
        let expected = Rejection::Engine(error.clone());
        let result = finish_prepaid::<()>(&content, &mut state, &mut Cx { rng: &mut rng, events: &mut events }, &prepared,
            |draft, cx| {
                assert_eq!(draft.logistics.dumps["ax_dump_1"].supplies.fuel, 2);
                cx.rng.d6();
                draft.land.breakdown.markers.remove("repair-a");
                draft.land.replacement_receipts.next_serial.insert(Side::Axis, 9);
                cx.emit(EngineEvent::new(Audience::Side(Side::Axis), GameEvent::Note { text: "private draft output".into() }));
                Err(error)
            });
        assert_eq!(result, Err(expected));
        assert_eq!(snapshot(&state, &rng, &events), before);
    }
}

/// Cases: land:22.26, land:22.42, land:22.43
#[test]
fn unsuccessful_accepted_result_keeps_costs_and_same_closure_never_rerolls() {
    let (content, mut state, prepared) = fixture();
    let mut rng = CampaignRng::from_seed([74; 32]);
    let mut events = prefix();
    assert_eq!(finish_prepaid(&content, &mut state, &mut Cx { rng: &mut rng, events: &mut events }, &prepared,
        |_, cx| { cx.rng.d6(); Ok(false) }).unwrap(), Some(false));
    for id in ["ax_dump_1", "ax_dump_2"] {
        assert_eq!(state.logistics.dumps[id].supplies.fuel, 2);
    }
    assert!(state.land.breakdown.markers.contains_key("repair-a"));
    let after = snapshot(&state, &rng, &events);
    assert_eq!(finish_prepaid::<bool>(&content, &mut state, &mut Cx { rng: &mut rng, events: &mut events }, &prepared,
        |_, _| panic!("closed attempt must not roll or pay again")).unwrap(), None);
    assert_eq!(snapshot(&state, &rng, &events), after);
}

/// Cases: land:22.12, land:22.26
#[test]
fn stale_footprint_and_changed_closure_refuse_without_writes_or_draws() {
    for change_clock in [false, true] {
        let (content, mut state, prepared) = fixture();
        if change_clock { state.cursor.cycle += 1; }
        else { state.logistics.dumps.get_mut("ax_dump_1").unwrap().supplies.fuel += 1; }
        let mut rng = CampaignRng::from_seed([75; 32]);
        let mut events = prefix();
        let before = snapshot(&state, &rng, &events);
        let result = finish_prepaid::<()>(&content, &mut state, &mut Cx { rng: &mut rng, events: &mut events }, &prepared,
            |_, _| panic!("stale attempt must not adjudicate"));
        assert!(matches!(result, Err(Rejection::Engine(EngineError::Invariant { .. }))));
        assert_eq!(snapshot(&state, &rng, &events), before);
    }
}

/// Cases: land:22.26, land:22.42, land:22.43
#[test]
fn checkpointed_state_and_rng_reproduce_the_complete_prepaid_transaction() {
    let (content, mut state, prepared) = fixture();
    let mut restored: State = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    let mut rng = CampaignRng::from_seed([76; 32]);
    rng.d6();
    let mut restored_rng = CampaignRng::from_state(&rng.state());
    let mut events = prefix();
    let mut restored_events = prefix();
    fn failed_attempt(_: &mut State, cx: &mut Cx<'_>) -> Result<u8, Rejection> {
        let roll = cx.rng.d6().value();
        cx.emit(EngineEvent::new(Audience::Side(Side::Axis), GameEvent::Note {
            text: format!("fixture accepted failure, die {roll}"),
        }));
        Ok(roll)
    }
    let first = finish_prepaid(&content, &mut state, &mut Cx { rng: &mut rng, events: &mut events }, &prepared, failed_attempt);
    let replay = finish_prepaid(&content, &mut restored, &mut Cx { rng: &mut restored_rng, events: &mut restored_events }, &prepared, failed_attempt);
    assert_eq!(first, replay);
    assert!(first.is_ok());
    assert_eq!(snapshot(&state, &rng, &events), snapshot(&restored, &restored_rng, &restored_events));
}
