//! Segment accounting tests on a real Graziani unit and supply location.
use super::*;
use crate::state::{Dump, DumpLocation, Location};
use cna_content::scenario::Supplies;
use cna_content::units::{Toe, WeaponPoints};
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
    assert_eq!(state.logistics.fuel_segments[&id].origin.as_str(), "C4020");
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
    assert_eq!(state.logistics.fuel_segments[&id].origin.as_str(), "C4021");
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
