use super::*;
use crate::state::{Dump, DumpLocation, WeatherState};
use cna_content::{
    scenario::{Placement, Supplies},
    units::Trucks,
};
use cna_tables::land::weather::WeatherKind;
use std::sync::OnceLock;

fn content() -> &'static CnaContent {
    static CONTENT: OnceLock<CnaContent> = OnceLock::new();
    CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn fixture() -> (State, String) {
    let mut state = State::new(content()).unwrap();
    state.cursor.op_stage = Some(1);
    state.turn.weather = Some(WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    state.logistics.truck_pools.clear();
    state.logistics.unit_supply.clear();
    state.logistics.dumps.clear();
    let id = crate::logistics::pools::add_truck_pool(
        &mut state.logistics,
        None,
        Side::Axis,
        Placement::Hex {
            hex: "C4020".into(),
        },
        Some(Location::Hex {
            hex: "C4020".into(),
        }),
        Trucks {
            light: 2,
            medium: 1,
            heavy: 1,
        },
        Supplies {
            water: 999,
            stores: 3,
            ..Supplies::default()
        },
    )
    .unwrap();
    state.logistics.dumps.insert(
        "water".into(),
        Dump {
            id: "water".into(),
            marker: "dump-water".into(),
            side: Side::Axis,
            location: DumpLocation::Hex {
                hex: "C4020".into(),
            },
            active: true,
            dummy: false,
            supplies: Supplies {
                water: 20,
                ..Supplies::default()
            },
        },
    );
    (state, id)
}
fn draw(n: i32) -> Vec<SupplyDraw> {
    vec![SupplyDraw {
        source: super::super::super::SupplySource::Dump("water".into()),
        amount: SupplyDemand {
            water: WaterPoints::new(n),
            ..SupplyDemand::default()
        },
    }]
}

/// Cases: airlog:52.42, land:29.34, land:29.35
#[test]
fn normal_hot_and_existing_reserve_bound_exact_stock_issue_without_cargo_or_cp() {
    let (mut state, id) = fixture();
    let cargo = state.logistics.truck_pools[0].cargo;
    assert_eq!(
        activity_need(content(), &state, Side::Axis, &id).unwrap(),
        4
    );
    let old = serde_json::to_value(&state).unwrap();
    assert!(matches!(
        issue(content(), &mut state, Side::Axis, &id, 5, &draw(5)),
        Err(Rejection::Illegal { .. })
    ));
    assert_eq!(serde_json::to_value(&state).unwrap(), old);
    issue(content(), &mut state, Side::Axis, &id, 2, &draw(2)).unwrap();
    assert_eq!(
        activity_need(content(), &state, Side::Axis, &id).unwrap(),
        2
    );
    issue(content(), &mut state, Side::Axis, &id, 2, &draw(2)).unwrap();
    assert_eq!(state.logistics.truck_pools[0].activity_water.get(), 4);
    assert_eq!(state.logistics.dumps["water"].supplies.water, 16);
    assert_eq!(state.logistics.truck_pools[0].cargo, cargo);
    let mut expected: State = serde_json::from_value(old).unwrap();
    expected.logistics.truck_pools[0].activity_water = WaterPoints::new(4);
    expected
        .logistics
        .dumps
        .get_mut("water")
        .unwrap()
        .supplies
        .water = 16;
    assert_eq!(
        serde_json::to_value(&state).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    let (mut hot, id) = fixture();
    hot.turn.weather.as_mut().unwrap().kind = WeatherKind::Hot;
    assert_eq!(activity_need(content(), &hot, Side::Axis, &id).unwrap(), 8);
    issue(content(), &mut hot, Side::Axis, &id, 8, &draw(8)).unwrap();
    assert_eq!(hot.logistics.truck_pools[0].activity_water.get(), 8);
    assert!(candidates(content(), &hot, Side::Axis).unwrap().is_empty());
    let json = serde_json::to_value(&hot).unwrap();
    let restored: State = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), json);
}

/// Cases: airlog:52.42, land:3.6
#[test]
fn missing_foreign_sources_shortage_and_bad_arithmetic_refuse_atomically() {
    let (mut state, id) = fixture();
    state.turn.weather = None;
    let foreign = issue(content(), &mut state, Side::Commonwealth, &id, 1, &draw(1)).unwrap_err();
    let missing = issue(
        content(),
        &mut state,
        Side::Commonwealth,
        "missing",
        1,
        &draw(1),
    )
    .unwrap_err();
    assert_eq!(foreign, missing);
    assert!(
        matches!(issue(content(), &mut state, Side::Axis, &id, 1, &draw(1)), Err(Rejection::Engine(EngineError::Unsupported { case, .. })) if case == "land:29.1")
    );
    for mode in 0..5 {
        let (mut state, id) = fixture();
        match mode {
            0 => {
                state
                    .logistics
                    .dumps
                    .get_mut("water")
                    .unwrap()
                    .supplies
                    .water = 1
            }
            1 => state.logistics.dumps.get_mut("water").unwrap().side = Side::Commonwealth,
            2 => {
                state.logistics.dumps.get_mut("water").unwrap().location = DumpLocation::Hex {
                    hex: "C4021".into(),
                }
            }
            3 => state.logistics.truck_pools[0].trucks.light = i32::MAX,
            _ => state.logistics.truck_pools[0].activity_water = WaterPoints::new(-1),
        }
        let before = serde_json::to_value(&state).unwrap();
        assert!(issue(content(), &mut state, Side::Axis, &id, 4, &draw(4)).is_err());
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
    }
    let (mut no_stock, id) = fixture();
    no_stock.logistics.dumps.clear();
    let before = serde_json::to_value(&no_stock).unwrap();
    assert!(issue(content(), &mut no_stock, Side::Axis, &id, 4, &draw(4)).is_err());
    assert_eq!(serde_json::to_value(&no_stock).unwrap(), before);
    assert_eq!(no_stock.logistics.truck_pools[0].cargo.water, 999);
}

/// Cases: airlog:52.42, land:29.34, land:29.35, land:3.6
#[test]
fn baseline_joint_stock_choice_is_exact_and_fallible() {
    let (mut state, _) = fixture();
    state.logistics.truck_pools[0].activity_water = WaterPoints::new(1);
    let rows = baseline(content(), &mut state, Side::Axis).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["activity"], 3);
    assert_eq!(state.logistics.dumps["water"].supplies.water, 17);
    let (mut shortage, _) = fixture();
    shortage
        .logistics
        .dumps
        .get_mut("water")
        .unwrap()
        .supplies
        .water = 3;
    let before = serde_json::to_value(&shortage).unwrap();
    assert!(
        baseline(content(), &mut shortage, Side::Axis)
            .unwrap()
            .is_empty()
    );
    assert_eq!(serde_json::to_value(&shortage).unwrap(), before);
    shortage.turn.weather = None;
    assert!(
        matches!(baseline(content(), &mut shortage, Side::Axis), Err(EngineError::Unsupported { case, .. }) if case == "land:29.1")
    );
}
