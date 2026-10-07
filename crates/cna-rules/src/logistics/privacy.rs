//! Shared side/seat secrecy checks on real Graziani state.
use super::{
    convoys::{ConvoyStatus, ConvoyTurn, NavalConvoy},
    dump_markers,
    rations::WaterStage,
};
use crate::{Cna, CnaContent, State, state::WellState};
use cna_core::{dice::CampaignRng, engine::Cx, quantity::WaterPoints};
use cna_protocol::Side;
use cna_tables::airlog::convoys::ConvoyLevel;
use std::{collections::BTreeMap, sync::OnceLock};
fn content() -> &'static CnaContent {
    static C: OnceLock<CnaContent> = OnceLock::new();
    C.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn state() -> State {
    let mut s = State::new(content()).unwrap();
    let mut rng = CampaignRng::from_seed([7; 32]);
    dump_markers::initialize(
        &mut s,
        &mut Cx {
            rng: &mut rng,
            events: &mut vec![],
        },
    )
    .unwrap();
    s
}
fn indistinguishable(a: &State, b: &State) {
    crate::testkit::assert_indistinguishable(&Cna::dev(), content(), a, b, Side::Commonwealth);
}
/// Cases: land:3.6, land:3.62, airlog:54.11
#[test]
fn hidden_dummy_identity_passes_every_enemy_seat_surface() {
    let a = state();
    let mut b = a.clone();
    for d in b
        .logistics
        .dumps
        .values_mut()
        .filter(|d| d.side == Side::Axis)
    {
        d.dummy = !d.dummy;
    }
    indistinguishable(&a, &b);
}
/// Cases: airlog:56.12, airlog:56.15, airlog:56.25, land:3.6
#[test]
fn hidden_convoy_cargo_passes_every_enemy_seat_surface() {
    let mut a = state();
    a.logistics.convoy_turns.insert(
        1,
        ConvoyTurn {
            level: ConvoyLevel::B,
            capacity_tons: 10000,
            replacement_tons: 0,
            planning_complete: true,
            convoys: BTreeMap::from([(
                2,
                NavalConvoy {
                    lane: 2,
                    arrival_opstage: 2,
                    cargo: Default::default(),
                    status: ConvoyStatus::Planned,
                    delivered: None,
                },
            )]),
        },
    );
    let mut b = a.clone();
    b.logistics
        .convoy_turns
        .get_mut(&1)
        .unwrap()
        .convoys
        .get_mut(&2)
        .unwrap()
        .cargo
        .fuel = 1700;
    indistinguishable(&a, &b);
}
/// Cases: airlog:51.21, airlog:51.23, land:3.6
#[test]
fn hidden_rations_pass_every_enemy_seat_surface() {
    let mut a = state();
    let id = "it.1_libyan_div.viii_libyan_bn".into();
    a.logistics.rations.entry(id).or_default().issued_gt = Some(1);
    let mut b = a.clone();
    let r = b.logistics.rations.values_mut().next().unwrap();
    r.half = true;
    r.stores_received = 6;
    r.stores_required = 6;
    r.consecutive_short_gt = 2;
    indistinguishable(&a, &b);
}
/// Cases: airlog:52.41, airlog:52.42, airlog:52.53, land:3.6
#[test]
fn hidden_water_and_shortages_pass_every_enemy_seat_surface() {
    let mut a = state();
    let id: cna_core::ids::UnitId = "it.1_libyan_div.viii_libyan_bn".into();
    a.logistics.unit_supply.entry(id.clone()).or_default();
    a.logistics
        .rations
        .entry(id.clone())
        .or_default()
        .water_stage = Some(WaterStage {
        game_turn: 1,
        op_stage: 1,
    });
    let mut b = a.clone();
    b.logistics.unit_supply.get_mut(&id).unwrap().activity_water = WaterPoints::new(43);
    let r = b.logistics.rations.get_mut(&id).unwrap();
    r.infantry_water_received = 1;
    r.consecutive_short_water_stages = 2;
    indistinguishable(&a, &b);
}
/// Cases: airlog:52.14, airlog:52.16, land:3.6
#[test]
fn undiscovered_well_conditions_pass_every_enemy_seat_surface() {
    let mut a = state();
    let hex = "C4020".into();
    a.logistics.wells.insert(hex, WellState::default());
    let mut b = a.clone();
    let w = b.logistics.wells.values_mut().next().unwrap();
    w.depleted = true;
    w.poisoned = true;
    w.depleted_known.insert(Side::Axis);
    w.poisoned_known.insert(Side::Axis);
    w.poison_failed_stage.insert(
        Side::Axis,
        WaterStage {
            game_turn: 1,
            op_stage: 1,
        },
    );
    indistinguishable(&a, &b);
}
