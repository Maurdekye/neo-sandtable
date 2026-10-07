use super::*;
use crate::{
    land::breakdown,
    state::{Location, UnitSupply},
};
use cna_tables::land::weather::WeatherKind;
fn setup() -> (CnaContent, State, UnitId, UnitId, Division) {
    let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut s = State::new(&c).unwrap();
    s.turn.weather = Some(crate::state::WeatherState {
        kind: WeatherKind::Normal,
        storm_sections: vec![],
    });
    let from: UnitId = "cw.unassigned_inf.1st_rnf_mg_bn".into();
    let to: UnitId = "cw.2_nz_div.21st_nz_bn".into();
    assert!(s.land.units.contains_key(&to));
    for id in [&from, &to] {
        let u = s.land.units.get_mut(id).unwrap();
        u.location = Location::Hex {
            hex: "C4022".into(),
        };
        u.detached = true;
        u.attached_to = None;
        u.trucks = Trucks::default();
        u.transport_trucks = Trucks::default();
        s.logistics
            .unit_supply
            .insert(id.clone(), UnitSupply::default());
    }
    s.land.units.get_mut(&to).unwrap().detached = false;
    s.land.units.get_mut(&to).unwrap().attached_to = Some(from.clone());
    s.land.units.get_mut(&from).unwrap().trucks.medium = 3;
    let stock = s.logistics.unit_supply.get_mut(&from).unwrap();
    stock.tank_fuel = FuelTenths::new(30);
    stock.activity_water = WaterPoints::new(8);
    stock.carried.fuel = 1;
    logistics::spend_activity_water(&c, &mut s, &from).unwrap();
    breakdown::record_edge(&mut s, &from, &"C4022".into(), 16, 4, WeatherKind::Normal).unwrap();
    let cohorts = logistics::segment_fuel_cohorts(&s, &from).unwrap();
    let division = Division {
        transfers: vec![Transfer {
            from: from.clone(),
            to: to.clone(),
            cohorts: vec![FuelCohortSelection {
                id: cohorts[0].id.clone(),
                count: 1,
            }],
            cargo: CargoPacking {
                medium: Supplies {
                    fuel: 1,
                    ..Supplies::default()
                },
                ..CargoPacking::default()
            },
            tank_fuel_tenths: 10,
            activity_water_points: 1,
        }],
        allocations: vec![
            Allocation {
                unit: from.clone(),
                transport: Trucks::default(),
                packing: CargoPacking::default(),
            },
            Allocation {
                unit: to.clone(),
                transport: Trucks::default(),
                packing: CargoPacking {
                    medium: Supplies {
                        fuel: 1,
                        ..Supplies::default()
                    },
                    ..CargoPacking::default()
                },
            },
        ],
    };
    (c, s, from, to, division)
}
/// Cases: land:8.56, land:8.95, land:21.25, airlog:49.16, airlog:52.42
#[test]
fn selected_trucks_keep_exact_cargo_fuel_water_and_breakdown_history() {
    let (c, s, from, to, d) = setup();
    let before = serde_json::to_value(&s).unwrap();
    let draft = preview_reaction_division(&c, &s, &to, &d, true).unwrap();
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert_eq!(draft.land.units[&from].trucks.medium, 2);
    assert_eq!(draft.land.units[&to].trucks.medium, 1);
    let a = &draft.logistics.unit_supply[&from];
    let b = &draft.logistics.unit_supply[&to];
    assert_eq!((a.carried.fuel, b.carried.fuel), (0, 1));
    assert_eq!((a.tank_fuel.get(), b.tank_fuel.get()), (20, 10));
    assert_eq!(
        a.activity_water.get() + b.activity_water.get(),
        s.logistics.unit_supply[&from].activity_water.get()
    );
    let moved = logistics::segment_fuel_cohorts(&draft, &to).unwrap();
    assert_eq!(moved.len(), 1);
    assert_eq!(
        draft.land.breakdown.truck_histories[&moved[0].id].base_quarters,
        16
    );
    assert_eq!(draft.land.breakdown.accumulated_quarters.get(&to), None);
    let credit = draft.logistics.rations[&to]
        .activity_water_ledger
        .as_ref()
        .unwrap();
    assert_eq!(credit.body_paid, 0);
    assert_eq!(credit.truck_paid.medium, 1);
    let restored: State = serde_json::from_value(serde_json::to_value(&draft).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(restored).unwrap(),
        serde_json::to_value(draft).unwrap()
    );
}
/// Cases: land:8.56, land:8.95, land:8.97, airlog:54.2
#[test]
fn invalid_divisions_leave_every_original_ledger_and_stock_unchanged() {
    let (c, s, _, to, d) = setup();
    let before = serde_json::to_value(&s).unwrap();
    let mut cases = vec![];
    let mut bad = d.clone();
    bad.transfers[0].cohorts[0].count = 4;
    cases.push(bad);
    let mut bad = d.clone();
    bad.transfers[0].tank_fuel_tenths = -1;
    cases.push(bad);
    let mut bad = d.clone();
    bad.transfers[0].cargo.medium.fuel = 2;
    cases.push(bad);
    let mut bad = d.clone();
    bad.allocations[1].packing.medium.fuel = 0;
    cases.push(bad);
    let mut bad = d.clone();
    bad.allocations.pop();
    cases.push(bad);
    let mut bad = d.clone();
    let duplicate = bad.transfers[0].cohorts[0].clone();
    bad.transfers[0].cohorts.push(duplicate);
    cases.push(bad);
    for bad in cases {
        assert!(preview_reaction_division(&c, &s, &to, &bad, true).is_err());
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
    }
}
/// Cases: land:8.56, land:8.97
/// Interpretations: interp:land-0005
#[test]
fn disorganized_parent_must_keep_a_truck_and_unrelated_units_cannot_receive() {
    let (c, mut s, from, to, mut d) = setup();
    s.land.units.get_mut(&from).unwrap().cohesion_quarters = -20;
    d.transfers[0].cohorts[0].count = 3;
    d.transfers[0].tank_fuel_tenths = 30;
    assert!(preview_reaction_division(&c, &s, &to, &d, true).is_err());
    let (c, s, _, to, mut d) = setup();
    d.transfers[0].to = "it.libyan_tank_command.xxi_l_tank_bn".into();
    assert!(preview_reaction_division(&c, &s, &to, &d, true).is_err());
}

/// Cases: land:8.56, land:8.92, land:8.95
#[test]
fn reachable_ratings_require_enough_own_trucks_and_room_for_the_actual_cargo() {
    let (c, mut s, from, to, _) = setup();
    s.land.units.get_mut(&to).unwrap().toe = Some(cna_content::units::Toe::Under { under: 1 });
    let rates = reachable_divisions(&c, &s, &to, true);
    assert!(rates.contains_key(&10));
    assert!(rates.contains_key(&20), "{:?}", rates.keys());
    assert!(!rates.contains_key(&25));
    for division in rates.values().flatten() {
        preview_reaction_division(&c, &s, &to, division, true).unwrap();
    }
    // All three medium points are needed to carry these stores, leaving no troop transport.
    s.logistics.unit_supply.get_mut(&from).unwrap().carried = Supplies {
        stores: 3 * c
            .tables
            .airlog
            .truck_characteristics
            .truck(cna_tables::airlog::trucks::TruckType::Medium)
            .capacity_stores_points,
        ..Supplies::default()
    };
    assert_eq!(
        reachable_divisions(&c, &s, &to, true)
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![10]
    );
}
