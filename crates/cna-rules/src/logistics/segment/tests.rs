//! Segment accounting tests on a real Graziani unit and supply location.
use super::*;
use crate::state::{Dump, DumpLocation, Location};
use cna_content::scenario::Supplies;
use cna_content::units::{Toe, WeaponPoints};
use cna_core::ids::HexId;
use cna_protocol::Side;
use std::sync::OnceLock;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn game() -> (State, UnitId) {
    let mut state = State::new(content()).unwrap();
    let id: UnitId = "it.1_libyan_div.viii_libyan_bn".into();
    // An identified fuel-using composition, at the real initial supply hex.
    state.land.units.get_mut(&id).unwrap().toe = Some(Toe::Weapons(vec![WeaponPoints {
        weapon: "it.cv33".into(),
        n: 1,
    }]));
    let origin = state.land.units[&id].location.hex().unwrap().clone();
    state.logistics.dumps.insert(
        "origin".into(),
        Dump {
            marker: String::new(),
            id: "origin".into(),
            side: Side::Axis,
            location: DumpLocation::Hex { hex: origin },
            supplies: Supplies {
                fuel: 3,
                ..Supplies::default()
            },
            active: true,
            dummy: false,
        },
    );
    (state, id)
}

/// Cases: airlog:49.13, airlog:49.16
/// Interpretations: interp:airlog-0001
#[test]
fn small_increments_charge_the_same_source_as_one_whole_path() {
    let (mut incremental, id) = game();
    let mut whole = incremental.clone();
    spend_segment_fuel(content(), &mut whole, &id, 40).unwrap();
    for cp in (2..=40).step_by(2) {
        spend_segment_fuel(content(), &mut incremental, &id, cp).unwrap();
    }
    assert_eq!(
        incremental.logistics.dumps["origin"].supplies,
        whole.logistics.dumps["origin"].supplies
    );
    assert_eq!(incremental.logistics.dumps["origin"].supplies.fuel, 1);
    assert_eq!(
        incremental.logistics.fuel_segments[&id],
        whole.logistics.fuel_segments[&id]
    );
    assert_eq!(incremental.logistics.fuel_segments[&id].paid_cost.get(), 20);
}

/// Cases: airlog:49.15, airlog:49.16
#[test]
fn moving_away_keeps_origin_sourcing_and_rejection_is_atomic() {
    let (mut state, id) = game();
    state
        .logistics
        .dumps
        .get_mut("origin")
        .unwrap()
        .supplies
        .fuel = 1;
    spend_segment_fuel(content(), &mut state, &id, 2).unwrap();
    let destination: HexId = "C4021".into();
    state.land.units.get_mut(&id).unwrap().location = Location::Hex {
        hex: destination.clone(),
    };
    let mut later = state.logistics.dumps["origin"].clone();
    later.id = "later".into();
    later.location = DumpLocation::Hex { hex: destination };
    later.supplies.fuel = 100;
    state.logistics.dumps.insert(later.id.clone(), later);
    let before = serde_json::to_value(&state).unwrap();
    assert_eq!(
        spend_segment_fuel(content(), &mut state, &id, 24),
        Err(SupplyError::Insufficient)
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    assert_eq!(
        state.logistics.fuel_segments[&id]
            .origin
            .hex()
            .unwrap()
            .as_str(),
        "C4020"
    );
}

/// Cases: airlog:49.13, airlog:49.16
/// Interpretations: interp:airlog-0001
#[test]
fn rounded_credit_survives_departure_of_the_source_but_cannot_draw_fresh_fuel() {
    let (mut state, id) = game();
    state
        .logistics
        .dumps
        .get_mut("origin")
        .unwrap()
        .supplies
        .fuel = 1;
    spend_segment_fuel(content(), &mut state, &id, 2).unwrap();
    state.logistics.dumps.remove("origin");
    spend_segment_fuel(content(), &mut state, &id, 12).unwrap();
    assert_eq!(state.logistics.fuel_segments[&id].paid_cost.get(), 6);
    assert_eq!(
        spend_segment_fuel(content(), &mut state, &id, 24),
        Err(SupplyError::Insufficient)
    );
}

/// Cases: airlog:49.16
/// Interpretations: interp:airlog-0001
#[test]
fn a_new_segment_resets_origin_and_does_not_reuse_old_credit() {
    let (mut state, id) = game();
    spend_segment_fuel(content(), &mut state, &id, 2).unwrap();
    let destination: HexId = "C4021".into();
    state.land.units.get_mut(&id).unwrap().location = Location::Hex {
        hex: destination.clone(),
    };
    let mut later = state.logistics.dumps["origin"].clone();
    later.id = "later".into();
    later.location = DumpLocation::Hex { hex: destination };
    later.supplies.fuel = 1;
    state.logistics.dumps.insert(later.id.clone(), later);
    state.cursor.cycle += 1;
    spend_segment_fuel(content(), &mut state, &id, 2).unwrap();
    assert_eq!(state.logistics.dumps["later"].supplies.fuel, 0);
    assert_eq!(state.logistics.dumps["origin"].supplies.fuel, 2);
    assert_eq!(
        state.logistics.fuel_segments[&id]
            .origin
            .hex()
            .unwrap()
            .as_str(),
        "C4021"
    );
    assert_eq!(
        state.logistics.fuel_segments[&id].draws[0].source,
        SupplySource::Dump("later".into())
    );
}

/// Cases: airlog:49.13, airlog:49.16
/// Interpretations: interp:airlog-0001
#[test]
fn checkpoint_preserves_source_credit_and_preview_changes_nothing() {
    let (mut state, id) = game();
    spend_segment_fuel(content(), &mut state, &id, 2).unwrap();
    let before = serde_json::to_value(&state).unwrap();
    let plan = plan_segment_fuel(content(), &state, &id, 12).unwrap();
    assert_eq!(plan.increment.get(), 4);
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    let mut restored: State = serde_json::from_value(before).unwrap();
    spend_segment_fuel(content(), &mut restored, &id, 12).unwrap();
    assert_eq!(restored.logistics.dumps["origin"].supplies.fuel, 2);
    assert_eq!(restored.logistics.fuel_segments[&id].paid_cost.get(), 6);
    let before = serde_json::to_value(&restored).unwrap();
    assert_eq!(
        spend_segment_fuel(content(), &mut restored, &id, 4),
        Err(SupplyError::Invalid)
    );
    assert_eq!(serde_json::to_value(&restored).unwrap(), before);
}

/// Cases: airlog:49.12, airlog:49.13, airlog:49.16
#[test]
fn foot_movement_updates_only_the_ledger_without_reading_unused_stocks() {
    let (mut state, id) = game();
    state.land.units.get_mut(&id).unwrap().toe =
        State::new(content()).unwrap().land.units[&id].toe.clone();
    state.land.units.get_mut(&id).unwrap().trucks = cna_content::units::Trucks::default();
    // Converting this unused source to fuel tenths would overflow. A zero draw
    // must not inspect it or require any source to exist.
    state
        .logistics
        .dumps
        .get_mut("origin")
        .unwrap()
        .supplies
        .fuel = i32::MAX;
    let holdings = serde_json::to_value(&state.logistics.dumps).unwrap();
    let before = serde_json::to_value(&state).unwrap();
    let plan = plan_segment_fuel(content(), &state, &id, 9).unwrap();
    assert!(plan.increment.is_zero());
    assert!(plan.draws.is_empty());
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    assert!(
        spend_segment_fuel(content(), &mut state, &id, 9)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        serde_json::to_value(&state.logistics.dumps).unwrap(),
        holdings
    );
    assert_eq!(state.logistics.fuel_segments[&id].cp_quarters, 9);
    assert_eq!(state.logistics.fuel_segments[&id], plan.ledger);
    state.land.units.remove(&id);
    assert_eq!(
        plan_segment_fuel(content(), &state, &id, 10),
        Err(SupplyError::Invalid)
    );
}

/// Cases: airlog:49.13, airlog:49.16
/// Interpretations: interp:airlog-0001
#[test]
fn unchanged_chart_bucket_preserves_credit_and_still_rejects_invalid_prior_draws() {
    let (mut state, id) = game();
    spend_segment_fuel(content(), &mut state, &id, 2).unwrap();
    let prior_draws = state.logistics.fuel_segments[&id].draws.clone();
    state
        .logistics
        .dumps
        .get_mut("origin")
        .unwrap()
        .supplies
        .fuel = i32::MAX;
    assert!(
        spend_segment_fuel(content(), &mut state, &id, 4)
            .unwrap()
            .is_empty()
    );
    assert_eq!(state.logistics.fuel_segments[&id].cp_quarters, 4);
    assert_eq!(state.logistics.fuel_segments[&id].draws, prior_draws);
    assert_eq!(state.logistics.dumps["origin"].supplies.fuel, i32::MAX);
    state.logistics.fuel_segments.get_mut(&id).unwrap().draws[0].fuel = FuelTenths::new(-1);
    let before = serde_json::to_value(&state).unwrap();
    assert_eq!(
        spend_segment_fuel(content(), &mut state, &id, 4),
        Err(SupplyError::Invalid)
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    state.logistics.fuel_segments.get_mut(&id).unwrap().draws = prior_draws;
    state
        .logistics
        .fuel_segments
        .get_mut(&id)
        .unwrap()
        .paid_cost = FuelTenths::new(100);
    assert_eq!(
        plan_segment_fuel(content(), &state, &id, 4),
        Err(SupplyError::Invalid)
    );
}

fn truck_game() -> (State, UnitId, UnitId) {
    let (mut s, a) = game();
    s.land.units.get_mut(&a).unwrap().toe =
        State::new(content()).unwrap().land.units[&a].toe.clone();
    s.land.units.get_mut(&a).unwrap().trucks = Trucks {
        light: 1,
        ..Trucks::default()
    };
    let b = s
        .land
        .units
        .values()
        .find(|u| {
            u.side == Side::Axis
                && u.id != a
                && super::super::rations::class(content(), &u.id)
                    .is_ok_and(|c| c.unit_type == "infantry")
        })
        .unwrap()
        .id
        .clone();
    let location = s.land.units[&a].location.clone();
    let u = s.land.units.get_mut(&b).unwrap();
    u.location = location;
    u.trucks = Trucks::default();
    s.logistics.unit_supply.clear();
    (s, a, b)
}
fn move_truck(s: &mut State, from: &UnitId, to: &UnitId, n: i32) -> Vec<TruckFuelCohort> {
    let moved = transfer_segment_fuel_cohorts(
        s,
        from,
        to,
        Trucks {
            light: n,
            ..Trucks::default()
        },
    )
    .unwrap();
    s.land.units.get_mut(from).unwrap().trucks.light -= n;
    s.land.units.get_mut(to).unwrap().trucks.light += n;
    moved
}
/// Cases: land:8.56, airlog:49.13, airlog:49.16
#[test]
fn transferred_truck_keeps_four_cp_and_uses_remaining_rounding_credit_once() {
    let (mut s, a, b) = truck_game();
    spend_segment_fuel(content(), &mut s, &a, 16).unwrap();
    assert_eq!(s.logistics.dumps["origin"].supplies.fuel, 2);
    let old = s.logistics.fuel_segments[&a].cohorts[0].id.clone();
    let moved = move_truck(&mut s, &a, &b, 1);
    assert_eq!(moved[0].id, old);
    assert_eq!(moved[0].cp_quarters, 16);
    assert_eq!(s.logistics.fuel_segments[&b].cp_quarters, 0);
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(
        plan_segment_fuel(content(), &s, &b, 4)
            .unwrap()
            .increment
            .get(),
        2
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    let mut restored: State = serde_json::from_value(before).unwrap();
    spend_segment_fuel(content(), &mut restored, &b, 4).unwrap();
    assert_eq!(restored.logistics.dumps["origin"].supplies.fuel, 2);
    assert_eq!(restored.logistics.fuel_accounts[&a].paid_cost.get(), 10);
    assert_eq!(
        restored.logistics.fuel_segments[&b].cohorts[0].cp_quarters,
        20
    );
    spend_segment_fuel(content(), &mut restored, &a, 20).unwrap();
    assert_eq!(restored.logistics.fuel_segments[&a].paid_cost.get(), 8);
}
/// Cases: land:8.56, airlog:49.13, airlog:49.16
#[test]
fn two_split_descendants_cannot_each_reuse_the_same_source_credit() {
    let (mut s, a, b) = truck_game();
    s.land.units.get_mut(&a).unwrap().trucks.light = 3;
    let c = s
        .land
        .units
        .values()
        .find(|u| {
            u.side == Side::Axis
                && u.id != a
                && u.id != b
                && super::super::rations::class(content(), &u.id)
                    .is_ok_and(|c| c.unit_type == "infantry")
        })
        .unwrap()
        .id
        .clone();
    let location = s.land.units[&a].location.clone();
    let u = s.land.units.get_mut(&c).unwrap();
    u.location = location;
    u.trucks = Trucks::default();
    spend_segment_fuel(content(), &mut s, &a, 4).unwrap(); // .6, one point withdrawn
    let root = s.logistics.fuel_segments[&a].cohorts[0].id.clone();
    let mb = move_truck(&mut s, &a, &b, 1);
    let mc = move_truck(&mut s, &a, &c, 1);
    assert_eq!(mb[0].parent.as_deref(), Some(root.as_str()));
    assert_eq!(mc[0].parent.as_deref(), Some(root.as_str()));
    assert_ne!(mb[0].id, mc[0].id);
    spend_segment_fuel(content(), &mut s, &b, 4).unwrap();
    spend_segment_fuel(content(), &mut s, &c, 4).unwrap();
    assert_eq!(s.logistics.fuel_accounts[&a].paid_cost.get(), 10);
    assert_eq!(s.logistics.dumps["origin"].supplies.fuel, 2);
    spend_segment_fuel(content(), &mut s, &a, 8).unwrap();
    assert_eq!(s.logistics.fuel_accounts[&a].paid_cost.get(), 12);
    assert_eq!(s.logistics.dumps["origin"].supplies.fuel, 1);
}
/// Cases: airlog:49.13, airlog:49.16
#[test]
fn incoming_truck_uses_original_origin_while_child_body_uses_its_own_origin() {
    let (mut s, a, b) = truck_game();
    spend_segment_fuel(content(), &mut s, &a, 16).unwrap();
    let destination = Location::Hex {
        hex: "C4021".into(),
    };
    s.land.units.get_mut(&a).unwrap().location = destination.clone();
    s.land.units.get_mut(&b).unwrap().location = destination.clone();
    s.land.units.get_mut(&b).unwrap().toe = Some(Toe::Weapons(vec![WeaponPoints {
        weapon: "it.cv33".into(),
        n: 1,
    }]));
    let mut later = s.logistics.dumps["origin"].clone();
    later.id = "later".into();
    later.location = DumpLocation::Hex {
        hex: "C4021".into(),
    };
    later.supplies.fuel = 3;
    s.logistics.dumps.insert("later".into(), later);
    move_truck(&mut s, &a, &b, 1);
    spend_segment_fuel(content(), &mut s, &b, 4).unwrap();
    assert_eq!(s.logistics.dumps["origin"].supplies.fuel, 2); // truck's old .2 credit
    assert_eq!(s.logistics.dumps["later"].supplies.fuel, 2); // body .2, separately rounded
    assert_eq!(s.logistics.fuel_accounts[&a].paid_cost.get(), 10);
    assert_eq!(s.logistics.fuel_accounts[&b].paid_cost.get(), 2);
}
/// Cases: land:21.25, land:21.29, airlog:49.13, airlog:49.16
#[test]
fn broken_trucks_freeze_paid_history_and_recovery_restores_their_own_cp() {
    let (mut s, a, _) = truck_game();
    s.land.units.get_mut(&a).unwrap().trucks.light = 3;
    spend_segment_fuel(content(), &mut s, &a, 4).unwrap();
    let broken = remove_segment_fuel_cohorts(
        &mut s,
        &a,
        Trucks {
            light: 1,
            ..Trucks::default()
        },
    )
    .unwrap();
    s.land.units.get_mut(&a).unwrap().trucks.light -= 1;
    assert_eq!(broken[0].cp_quarters, 4);
    spend_segment_fuel(content(), &mut s, &a, 8).unwrap();
    assert_eq!(s.logistics.fuel_accounts[&a].paid_cost.get(), 10);
    restore_segment_fuel_cohorts(&mut s, &a, &broken).unwrap();
    s.land.units.get_mut(&a).unwrap().trucks.light += 1;
    let before = serde_json::to_value(&s).unwrap();
    assert!(restore_segment_fuel_cohorts(&mut s, &a, &broken).is_err());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    spend_segment_fuel(content(), &mut s, &a, 12).unwrap();
    assert_eq!(s.logistics.fuel_accounts[&a].paid_cost.get(), 16);
    assert_eq!(s.logistics.dumps["origin"].supplies.fuel, 1);
    assert_eq!(
        s.logistics.fuel_segments[&a]
            .cohorts
            .iter()
            .find(|c| c.id == broken[0].id)
            .unwrap()
            .cp_quarters,
        8
    );
}
/// Cases: land:21.25, land:21.29, airlog:49.12, airlog:49.16
#[test]
fn lost_vehicle_toe_never_refunds_or_reprices_its_earlier_body_movement() {
    let (mut s, a) = game();
    s.land.units.get_mut(&a).unwrap().trucks = Trucks::default();
    s.land.units.get_mut(&a).unwrap().toe = Some(Toe::Weapons(vec![WeaponPoints {
        weapon: "it.cv33".into(),
        n: 2,
    }]));
    spend_segment_fuel(content(), &mut s, &a, 16).unwrap(); // 2*.8
    s.land.units.get_mut(&a).unwrap().toe = Some(Toe::Weapons(vec![WeaponPoints {
        weapon: "it.cv33".into(),
        n: 1,
    }]));
    let plan = plan_segment_fuel(content(), &s, &a, 20).unwrap();
    assert_eq!(plan.increment.get(), 2);
    assert_eq!(plan.ledger.paid_cost.get(), 18);
    spend_segment_fuel(content(), &mut s, &a, 20).unwrap();
    assert_eq!(s.logistics.dumps["origin"].supplies.fuel, 1);
    s.land.units.get_mut(&a).unwrap().toe = Some(Toe::Weapons(vec![]));
    spend_segment_fuel(content(), &mut s, &a, 24).unwrap();
    assert_eq!(s.logistics.fuel_segments[&a].paid_cost.get(), 18);
    assert_eq!(s.logistics.dumps["origin"].supplies.fuel, 1);
}
/// Cases: land:8.56, airlog:49.16
#[test]
fn planning_snapshot_restores_shared_foreign_account_and_invalid_transfer_is_atomic() {
    let (mut s, a, b) = truck_game();
    spend_segment_fuel(content(), &mut s, &a, 16).unwrap();
    move_truck(&mut s, &a, &b, 1);
    let snapshot = snapshot_fuel_accounts(&s, std::slice::from_ref(&b));
    let before = s.logistics.clone();
    spend_segment_fuel(content(), &mut s, &b, 4).unwrap();
    restore_fuel_accounts(&mut s, &snapshot);
    assert_eq!(s.logistics.fuel_accounts, before.fuel_accounts);
    s.logistics = before;
    let before = serde_json::to_value(&s).unwrap();
    assert!(
        transfer_segment_fuel_cohorts(
            &mut s,
            &b,
            &a,
            Trucks {
                light: 2,
                ..Trucks::default()
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    s.land.units.get_mut(&a).unwrap().side = Side::Commonwealth;
    let before = serde_json::to_value(&s).unwrap();
    assert!(
        transfer_segment_fuel_cohorts(
            &mut s,
            &b,
            &a,
            Trucks {
                light: 1,
                ..Trucks::default()
            }
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}
/// Cases: airlog:49.16, land:21.25
#[test]
fn segment_reset_preserves_physical_id_and_discards_only_prior_segment_fuel_credit() {
    let (mut s, a, b) = truck_game();
    spend_segment_fuel(content(), &mut s, &a, 16).unwrap();
    let moved = move_truck(&mut s, &a, &b, 1);
    s.cursor.cycle += 1;
    spend_segment_fuel(content(), &mut s, &b, 4).unwrap();
    let cohort = &s.logistics.fuel_segments[&b].cohorts[0];
    assert_eq!(cohort.id, moved[0].id);
    assert_eq!(cohort.cp_quarters, 4);
    assert_eq!(cohort.account, b);
    assert_eq!(s.logistics.dumps["origin"].supplies.fuel, 1);
}
/// Cases: airlog:49.16, land:3.6
#[test]
fn legacy_checkpoint_migrates_without_changing_cost_or_enemy_knowledge() {
    let (mut s, a) = game();
    spend_segment_fuel(content(), &mut s, &a, 2).unwrap();
    let mut encoded = serde_json::to_value(&s).unwrap();
    encoded["logistics"]
        .as_object_mut()
        .unwrap()
        .remove("fuel_accounts");
    let l = encoded["logistics"]["fuel_segments"][a.as_str()]
        .as_object_mut()
        .unwrap();
    l.remove("cohorts");
    l.remove("cohorts_initialized");
    l.remove("next_cohort_serial");
    let mut legacy: State = serde_json::from_value(encoded).unwrap();
    spend_segment_fuel(content(), &mut legacy, &a, 12).unwrap();
    spend_segment_fuel(content(), &mut s, &a, 12).unwrap();
    assert_eq!(
        s.logistics.dumps["origin"].supplies,
        legacy.logistics.dumps["origin"].supplies
    );
    crate::testkit::assert_indistinguishable(
        &crate::Cna::dev(),
        content(),
        &s,
        &legacy,
        Side::Commonwealth,
    );
}

/// Cases: land:21.25, land:21.29, airlog:49.16
#[test]
fn exact_cohort_selection_preserves_different_histories_and_rejects_duplicate_choices() {
    let (mut s, a, b) = truck_game();
    s.land.units.get_mut(&a).unwrap().trucks.light = 2;
    spend_segment_fuel(content(), &mut s, &a, 16).unwrap();
    let moved = move_truck(&mut s, &a, &b, 1);
    spend_segment_fuel(content(), &mut s, &b, 4).unwrap();
    let choice = FuelCohortSelection {
        id: moved[0].id.clone(),
        count: 1,
    };
    transfer_selected_segment_fuel_cohorts(&mut s, &b, &a, std::slice::from_ref(&choice)).unwrap();
    s.land.units.get_mut(&b).unwrap().trucks.light = 0;
    s.land.units.get_mut(&a).unwrap().trucks.light = 2;
    let current = segment_fuel_cohorts(&s, &a).unwrap();
    assert_eq!(current.len(), 2);
    assert_eq!(current[0].cp_quarters, 16);
    assert_eq!(current[1].cp_quarters, 20);
    let before = serde_json::to_value(&s).unwrap();
    assert!(
        remove_selected_segment_fuel_cohorts(&mut s, &a, &[choice.clone(), choice.clone()])
            .is_err()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    let removed = remove_selected_segment_fuel_cohorts(&mut s, &a, &[choice]).unwrap();
    s.land.units.get_mut(&a).unwrap().trucks.light = 1;
    assert_eq!(removed[0].cp_quarters, 20);
    assert_eq!(segment_fuel_cohorts(&s, &a).unwrap()[0].cp_quarters, 16);
    spend_segment_fuel(content(), &mut s, &a, 20).unwrap();
    assert_eq!(s.logistics.fuel_accounts[&a].paid_cost.get(), 20);
    assert_eq!(s.logistics.dumps["origin"].supplies.fuel, 1);
}
/// Cases: land:8.56, airlog:49.15, airlog:49.16
#[test]
fn funding_a_body_cannot_borrow_the_transferred_trucks_unused_account_credit() {
    let (mut s, a, b) = truck_game();
    s.logistics.dumps.get_mut("origin").unwrap().supplies.fuel = 1;
    spend_segment_fuel(content(), &mut s, &a, 16).unwrap(); // .2 credit remains
    s.land.units.get_mut(&b).unwrap().toe = Some(Toe::Weapons(vec![WeaponPoints {
        weapon: "it.cv33".into(),
        n: 1,
    }]));
    move_truck(&mut s, &a, &b, 1);
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(
        spend_segment_fuel(content(), &mut s, &b, 4),
        Err(SupplyError::Insufficient)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    // Once the child's own tank funds its body, the retained truck credit suffices.
    s.logistics
        .unit_supply
        .entry(b.clone())
        .or_default()
        .tank_fuel = FuelTenths::new(2);
    spend_segment_fuel(content(), &mut s, &b, 4).unwrap();
    assert_eq!(s.logistics.unit_supply[&b].tank_fuel.get(), 0);
    assert_eq!(s.logistics.dumps["origin"].supplies.fuel, 0);
    assert_eq!(s.logistics.fuel_accounts[&a].paid_cost.get(), 10);
    assert_eq!(s.logistics.fuel_accounts[&b].paid_cost.get(), 2);
}

/// Cases: land:8.83, land:8.84, airlog:49.13, airlog:49.16
#[test]
fn box_origin_preserves_source_credit_and_physical_cohorts_across_checkpoint() {
    let mut limited_content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    limited_content.scenario.supply.unlimited_supply = None;
    let (mut state, id, _) = truck_game();
    state.logistics.dumps.retain(|key, _| key == "origin");
    let box_location = Location::OffMap {
        id: "box_tripoli".into(),
    };
    state.land.units.get_mut(&id).unwrap().location = box_location.clone();
    state.logistics.dumps.get_mut("origin").unwrap().location = DumpLocation::OffMap {
        id: "box_tripoli".into(),
    };
    spend_segment_fuel(&limited_content, &mut state, &id, 2).unwrap();
    let cohort = state.logistics.fuel_segments[&id].cohorts[0].id.clone();
    let mut restored: State =
        serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    spend_segment_fuel(&limited_content, &mut state, &id, 12).unwrap();
    spend_segment_fuel(&limited_content, &mut restored, &id, 12).unwrap();
    assert_eq!(
        serde_json::to_value(&state.logistics).unwrap(),
        serde_json::to_value(&restored.logistics).unwrap()
    );
    assert_eq!(restored.logistics.fuel_segments[&id].origin, box_location);
    assert_eq!(restored.logistics.fuel_segments[&id].cohorts[0].id, cohort);
    assert_eq!(restored.logistics.dumps["origin"].supplies.fuel, 2);
    assert_eq!(restored.logistics.fuel_segments[&id].paid_cost.get(), 6);
}

/// Cases: land:8.83, land:8.84, airlog:49.15, airlog:49.16
#[test]
fn next_transit_stage_uses_own_tank_and_group_cargo_without_departed_box_supply() {
    let mut limited_content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    limited_content.scenario.supply.unlimited_supply = None;
    let (mut state, id, companion) = truck_game();
    state.logistics.dumps.retain(|key, _| key == "origin");
    state.land.units.get_mut(&id).unwrap().location = Location::OffMap {
        id: "box_tripoli".into(),
    };
    state.logistics.dumps.get_mut("origin").unwrap().location = DumpLocation::OffMap {
        id: "box_tripoli".into(),
    };
    spend_segment_fuel(&limited_content, &mut state, &id, 2).unwrap();
    let cohort = state.logistics.fuel_segments[&id].cohorts[0].id.clone();
    let group = Location::OffMap {
        id: "travel-group-1".into(),
    };
    state.land.units.get_mut(&id).unwrap().location = group.clone();
    state.land.units.get_mut(&companion).unwrap().location = Location::OffMap {
        id: "travel-group-2".into(),
    };
    state.land.units.get_mut(&companion).unwrap().trucks.light = 1;
    state
        .logistics
        .unit_supply
        .entry(companion.clone())
        .or_default()
        .carried
        .fuel = 2;
    state.cursor.op_stage = Some(2);
    let mut impossible_dump = state.logistics.dumps["origin"].clone();
    impossible_dump.id = "not-a-transit-base".into();
    impossible_dump.location = DumpLocation::OffMap {
        id: "travel-group-1".into(),
    };
    impossible_dump.supplies.fuel = 100;
    state
        .logistics
        .dumps
        .insert(impossible_dump.id.clone(), impossible_dump);
    let before = serde_json::to_value(&state).unwrap();
    assert_eq!(
        spend_segment_fuel(&limited_content, &mut state, &id, 24),
        Err(SupplyError::Insufficient)
    );
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
    state.land.units.get_mut(&companion).unwrap().location = group.clone();
    state
        .logistics
        .unit_supply
        .entry(id.clone())
        .or_default()
        .tank_fuel = FuelTenths::new(2);
    spend_segment_fuel(&limited_content, &mut state, &id, 24).unwrap();
    assert_eq!(state.logistics.fuel_segments[&id].origin, group);
    assert_eq!(state.logistics.fuel_segments[&id].paid_cost.get(), 20);
    assert_eq!(state.logistics.fuel_segments[&id].cohorts[0].id, cohort);
    assert_eq!(state.logistics.unit_supply[&id].tank_fuel.get(), 0);
    assert_eq!(state.logistics.unit_supply[&companion].carried.fuel, 0);
    assert_eq!(state.logistics.dumps["origin"].supplies.fuel, 2);
    assert!(
        state.logistics.fuel_segments[&id]
            .draws
            .iter()
            .all(|d| !matches!(d.source, SupplySource::Dump(_) | SupplySource::Unlimited))
    );
}

/// Cases: airlog:49.16, land:3.6
#[test]
fn hex_string_account_checkpoint_retains_rounded_credit_and_rejects_invalid_origins() {
    let (mut state, id) = game();
    spend_segment_fuel(content(), &mut state, &id, 2).unwrap();
    let mut encoded = serde_json::to_value(&state).unwrap();
    for key in ["fuel_accounts", "fuel_segments"] {
        encoded["logistics"][key][id.as_str()]["origin"] = serde_json::json!("C4020");
    }
    let mut legacy: State = serde_json::from_value(encoded.clone()).unwrap();
    spend_segment_fuel(content(), &mut state, &id, 12).unwrap();
    spend_segment_fuel(content(), &mut legacy, &id, 12).unwrap();
    assert_eq!(
        serde_json::to_value(&state.logistics).unwrap(),
        serde_json::to_value(&legacy.logistics).unwrap()
    );
    crate::testkit::assert_indistinguishable(
        &crate::Cna::dev(),
        content(),
        &state,
        &legacy,
        Side::Commonwealth,
    );
    encoded["logistics"]["fuel_accounts"][id.as_str()]["origin"] =
        serde_json::json!({"at":"eliminated"});
    assert!(serde_json::from_value::<State>(encoded).is_err());
}
