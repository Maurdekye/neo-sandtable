//! Source-contract controls for the recovery disclosure question.
//! These exercise production Sync on fixture physical changes, not a repair driver.
use crate::land::breakdown::{Asset, Equipment, markers::BrokenMarker};
use crate::{CnaContent, State, state::Location};
use cna_content::units::Trucks;
use cna_core::{
    dice::CampaignRng,
    engine::Cx,
    event::EngineEvent,
    visibility::{Audience, Perspective},
};
use cna_protocol::{GameEvent, Side};

const MARKER: &str = "captured-recovery-contract";

fn fixture() -> (CnaContent, State) {
    let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut state = State::new(&content).unwrap();
    let original = state
        .land
        .units
        .values()
        .find(|u| u.side == Side::Axis)
        .unwrap()
        .id
        .clone();
    let friendly = state
        .land
        .units
        .values()
        .find(|u| u.side == Side::Commonwealth)
        .unwrap()
        .id
        .clone();
    for unit in state.land.units.values_mut() {
        unit.location = Location::Eliminated;
    }
    state.land.units.get_mut(&friendly).unwrap().location = Location::Hex {
        hex: "C4807".into(),
    };
    state.land.breakdown.markers.insert(
        MARKER.into(),
        BrokenMarker {
            id: MARKER.into(),
            side: Side::Commonwealth,
            hex: "C4807".into(),
            assets: vec![Asset {
                unit: original,
                equipment: Equipment::MediumTruck,
                points: 1,
                cohort: None,
            }],
            source_pool: None,
            pool_assets: vec![],
            pool_fuel_cohorts: vec![],
            passengers: Default::default(),
            transport: Trucks::default(),
            cargo: Default::default(),
            tank_fuel: Default::default(),
            activity_water: Default::default(),
            fuel_cohorts: vec![],
            paid_truck_water: Default::default(),
            water_credit_stage: None,
        },
    );
    (content, state)
}

fn former_owner_stream(content: &CnaContent, before: &State, after: &State) -> Vec<GameEvent> {
    let mut rng = CampaignRng::from_seed([79; 32]);
    let mut events = vec![];
    let mut cx = Cx {
        rng: &mut rng,
        events: &mut events,
    };
    let mut sync = crate::view::Sync::new(before, &cx);
    cx.emit(EngineEvent::new(
        Audience::Side(Side::Commonwealth),
        GameEvent::Note {
            text: "captor-only physical recovery result".into(),
        },
    ));
    sync.point(content, after, &mut cx);
    events
        .into_iter()
        .filter(|e| e.visible_to(Perspective::Side(Side::Axis)))
        .map(|e| e.event)
        .collect()
}

/// Cases: land:3.62, land:21.42, land:22.5
/// Interpretations: interp:land-0038
#[test]
fn removing_a_printed_captured_marker_emits_former_owner_unit_removed() {
    let (content, before) = fixture();
    let mut after = before.clone();
    after.land.breakdown.markers.remove(MARKER);
    let stream = former_owner_stream(&content, &before, &after);
    assert!(stream.iter().any(|event| matches!(event,
        GameEvent::UnitRemoved { unit_id, .. } if unit_id == MARKER)));
    assert!(
        !stream
            .iter()
            .any(|event| matches!(event, GameEvent::Note { .. }))
    );
}

/// Cases: land:3.62, land:21.63
/// Interpretations: interp:land-0038
#[test]
fn moving_a_printed_captured_marker_emits_its_new_hex_to_former_owner() {
    let (content, before) = fixture();
    let mut after = before.clone();
    after.land.breakdown.markers.get_mut(MARKER).unwrap().hex = "C4808".into();
    let stream = former_owner_stream(&content, &before, &after);
    assert!(stream.iter().any(|event| matches!(event,
        GameEvent::UnitUpdated { unit } if unit.id == MARKER && unit.hex.as_deref() == Some("C4808"))));
    assert!(
        !stream
            .iter()
            .any(|event| matches!(event, GameEvent::Note { .. }))
    );
}

/// Cases: land:3.62, land:22.5
/// Interpretations: interp:land-0038
#[test]
fn private_contents_are_hidden_and_equal_public_removals_have_equal_former_owner_streams() {
    let (content, left) = fixture();
    let mut right = left.clone();
    right.land.breakdown.markers.get_mut(MARKER).unwrap().assets[0].points = 2;
    assert!(former_owner_stream(&content, &left, &right).is_empty());
    let mut left_after = left.clone();
    left_after.land.breakdown.markers.remove(MARKER);
    let mut right_after = right.clone();
    right_after.land.breakdown.markers.remove(MARKER);
    assert_eq!(
        former_owner_stream(&content, &left, &left_after),
        former_owner_stream(&content, &right, &right_after)
    );
}
