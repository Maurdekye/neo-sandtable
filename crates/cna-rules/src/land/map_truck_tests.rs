use super::*;
use cna_content::{map::MapContent, units::Trucks};
use cna_protocol::Side;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
};
fn content() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}
struct Overlay(PathBuf);
impl Drop for Overlay {
    fn drop(&mut self) {
        assert!(self.0.starts_with(std::env::temp_dir()));
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("cna-convoy-map-")
        );
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn fixture(
    base: &str,
    route: Option<&str>,
    surveyed: bool,
    edge: Option<&str>,
) -> (CnaContent, Overlay) {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let mut c = content();
    let dir = std::env::temp_dir().join(format!(
        "cna-convoy-map-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&dir).unwrap();
    for name in ["layers.toml", "sections.toml"] {
        std::fs::copy(
            cna_content::repo_data_dir().join("map").join(name),
            dir.join(name),
        )
        .unwrap();
    }
    let a: HexId = "C4020".into();
    let b: HexId = "C4021".into();
    let mut cells = String::from("hex_id,section,q,r,terrain,flags\n");
    for id in [&a, &b] {
        let h = c.map.get(id).unwrap();
        cells += &format!(
            "{},{},{},{},{},\n",
            h.id, h.section, h.axial.q, h.axial.r, base
        );
    }
    let mut coverage = format!(
        "layer,hex_id,neighbour_id,src,review_batch\nterrain,{a},,land:8.37,test\nterrain,{b},,land:8.37,test\n"
    );
    if surveyed {
        for kind in LineKind::ALL {
            coverage += &format!("line:{},{a},{b},land:8.33,test\n", kind.name());
        }
        for kind in SideKind::ALL {
            coverage += &format!("side:{},{a},{b},land:8.35,test\n", kind.name());
        }
    }
    let mut lines = String::from("from_hex,to_hex,kind,src,review_batch\n");
    if let Some(route) = route {
        lines += &format!("{a},{b},{route},land:8.33,test\n");
    }
    let mut sides =
        String::from("hex_id,direction,neighbour_id,feature,high_side,src,review_batch\n");
    if let Some(edge) = edge {
        let high = if matches!(edge, "slope" | "escarpment") {
            b.as_str()
        } else {
            ""
        };
        sides += &format!("{a},E,{b},{edge},{high},land:8.35,test\n");
    }
    for (name, data) in [
        ("hexes.csv", cells),
        ("coverage.csv", coverage),
        ("line_features.csv", lines),
        ("hexsides.csv", sides),
    ] {
        std::fs::write(dir.join(name), data).unwrap();
    }
    c.map = MapContent::load(&dir).unwrap();
    (c, Overlay(dir))
}
fn step(c: &CnaContent, t: Trucks, strict: bool, rain: bool) -> Result<StepCost, Rejection> {
    truck_step_cost(
        c,
        Side::Axis,
        &t,
        &"C4020".into(),
        &"C4021".into(),
        strict,
        rain,
    )
}

/// Cases: land:9.29, land:9.33, land:8.44
#[test]
fn convoy_network_repricing_keeps_plain_prohibitions() {
    let trucks = Trucks {
        medium: 1,
        ..Trucks::default()
    };
    let (c, _overlay) = fixture("clear", Some("road"), true, None);
    assert_eq!(step(&c, trucks, true, false).unwrap().cp_quarters, 2);
    let off = truck_step_cost_with_network(
        &c,
        Side::Axis,
        &trucks,
        &"C4020".into(),
        &"C4021".into(),
        true,
        false,
        false,
    )
    .unwrap();
    assert_eq!(off.cp_quarters, 8);
    assert!(!off.on_network);
    let (c, _overlay) = fixture("salt_marsh", Some("road"), true, None);
    assert!(step(&c, trucks, true, false).is_ok());
    assert!(matches!(
        truck_step_cost_with_network(
            &c,
            Side::Axis,
            &trucks,
            &"C4020".into(),
            &"C4021".into(),
            true,
            false,
            false
        ),
        Err(Rejection::Illegal { .. })
    ));
}

fn add_pool(s: &mut State, side: Side, hex: &str, count: i32) -> String {
    s.turn.weather.get_or_insert(crate::state::WeatherState {
        kind: cna_tables::land::weather::WeatherKind::Normal,
        storm_sections: vec![],
    });
    crate::logistics::pools::add_truck_pool(
        &mut s.logistics,
        None,
        side,
        cna_content::scenario::Placement::Hex { hex: hex.into() },
        Some(crate::state::Location::Hex { hex: hex.into() }),
        Trucks {
            light: count,
            ..Trucks::default()
        },
        cna_content::scenario::Supplies::default(),
    )
    .unwrap()
}

/// Cases: land:9.29, land:9.33, land:9.34, land:29.45
#[test]
fn convoy_preparation_reprices_shared_road_and_preserves_state() {
    let (c, _overlay) = fixture("clear", Some("road"), true, None);
    let mut s = State::new(&content()).unwrap();
    s.land.units.clear();
    s.logistics.truck_pools.clear();
    let moving = add_pool(&mut s, Side::Axis, "C4020", 5);
    let fixed = add_pool(&mut s, Side::Axis, "C4021", 50);
    crate::land::convoy_move::record_pool_posture(&mut s, &fixed, true).unwrap();
    add_pool(&mut s, Side::Commonwealth, "C4020", 500);
    let before = serde_json::to_value(&s).unwrap();
    let plan = crate::land::convoy_move::prepare_pool_route(
        &c,
        &s,
        Side::Axis,
        &moving,
        &["C4021".into()],
        true,
    )
    .unwrap();
    assert_eq!(plan.costs[0].cp_quarters, 8);
    assert!(!plan.costs[0].on_network);
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert_eq!(
        crate::land::stacking::road_halves(&c, &s, &"C4021".into(), Side::Axis, &[]).unwrap(),
        10
    );
    {
        let origin: HexId = "C4020".into();
        let stacks = crate::land::stacking::PlanningStacks::new(&c, &s, Side::Axis, &[], &origin);
        assert_eq!(stacks.road_halves(&"C4021".into()).unwrap(), 10);
    }
    s.land.movement.pool_on_road.remove(&fixed);
    let plan = crate::land::convoy_move::prepare_pool_route(
        &c,
        &s,
        Side::Axis,
        &moving,
        &["C4021".into()],
        true,
    )
    .unwrap();
    assert_eq!(plan.costs[0].cp_quarters, 2);
    assert!(plan.costs[0].on_network);
    s.turn.weather.as_mut().unwrap().kind = cna_tables::land::weather::WeatherKind::Sandstorm;
    s.turn.weather.as_mut().unwrap().storm_sections =
        vec![cna_tables::land::weather::MapSection::C];
    let plan = crate::land::convoy_move::prepare_pool_route(
        &c,
        &s,
        Side::Axis,
        &moving,
        &["C4021".into()],
        true,
    )
    .unwrap();
    assert_eq!(plan.costs[0].cp_quarters, 4);
    assert_eq!(plan.costs[0].breakdown_quarters, 2);
    assert!(crate::land::stacking::road_over_limit(i32::MAX, 1).is_err());
    s.logistics.truck_pools[0].trucks.light = -1;
    assert!(matches!(
        crate::land::convoy_move::prepare_pool_route(&c, &s, Side::Axis, &moving, &[], true),
        Err(Rejection::Engine(EngineError::Invariant { .. }))
    ));
}

/// Cases: land:9.34
#[test]
fn convoy_posture_old_checkpoint_default_and_zero_edge_are_preserved() {
    let (c, _overlay) = fixture("clear", Some("road"), true, None);
    let mut s = State::new(&content()).unwrap();
    let mut old = serde_json::to_value(&s.land.movement).unwrap();
    old.as_object_mut().unwrap().remove("pool_on_road");
    let old: crate::land::movement::MovementState = serde_json::from_value(old).unwrap();
    assert!(old.pool_on_road.is_empty());
    s.logistics.truck_pools.clear();
    let id = add_pool(&mut s, Side::Axis, "C4020", 1);
    assert!(!s.land.movement.pool_on_road.contains(&id));
    assert_eq!(
        crate::land::stacking::road_occupancy_halves(
            &c,
            &s,
            &"C4020".into(),
            Side::Axis,
            &[],
            None
        )
        .unwrap(),
        crate::land::formation::roots(&c, &s, &"C4020".into(), Side::Axis)
            .iter()
            .map(|id| crate::land::formation::stacking_halves(&c, &s, id))
            .sum::<i32>()
    );
    crate::land::convoy_move::record_pool_posture(&mut s, &id, true).unwrap();
    let plan =
        crate::land::convoy_move::prepare_pool_route(&c, &s, Side::Axis, &id, &[], true).unwrap();
    assert!(plan.costs.is_empty());
    assert!(s.land.movement.pool_on_road.contains(&id));
    crate::land::convoy_move::record_pool_posture(&mut s, &id, false).unwrap();
    assert!(!s.land.movement.pool_on_road.contains(&id));
}

/// Cases: land:10.23, land:10.24, land:10.26, land:10.29
#[test]
fn trusted_convoy_edges_keep_illegal_geometry_and_departure_separate() {
    let (c, _overlay) = fixture("clear", Some("road"), true, None);
    let mut s = State::new(&content()).unwrap();
    s.land.units.clear();
    s.logistics.truck_pools.clear();
    let id = add_pool(&mut s, Side::Axis, "C4020", 1);
    assert_eq!(
        crate::land::convoy_move::pool_departure_cp(&c, &s, &id, true).unwrap(),
        0
    );
    let before = serde_json::to_value(&s).unwrap();
    assert!(
        matches!(crate::land::convoy_move::adjudicate_pool_edge(&c, &s, &id, &"C4020".into(), &"C4021".into(), true).unwrap(), crate::land::convoy_move::PoolEdge::Pass(cost) if cost.cp_quarters == 2)
    );
    assert!(matches!(
        crate::land::convoy_move::adjudicate_pool_edge(
            &c,
            &s,
            &id,
            &"C4021".into(),
            &"C4020".into(),
            true
        ),
        Err(EngineError::Invariant { .. })
    ));
    assert!(matches!(
        crate::land::convoy_move::adjudicate_pool_edge(
            &c,
            &s,
            &id,
            &"C4020".into(),
            &"C4020".into(),
            true
        ),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}

/// Cases: land:9.29, land:9.33, land:9.34
#[test]
fn formations_and_real_pools_share_one_road_occupancy_in_planning_and_truth() {
    let c = content();
    let mut s = State::new(&c).unwrap();
    let id: UnitId = "it.libyan_tank_command.xxi_l_tank_bn".into();
    let mut unit = s.land.units[&id].clone();
    unit.location = crate::state::Location::Hex {
        hex: "C4021".into(),
    };
    s.land.units.clear();
    s.land.units.insert(id.clone(), unit);
    s.logistics.truck_pools.clear();
    let pool = add_pool(&mut s, Side::Axis, "C4021", 5);
    crate::land::convoy_move::record_pool_posture(&mut s, &pool, true).unwrap();
    let unit_halves = crate::land::formation::stacking_halves(&c, &s, &id);
    assert!(unit_halves > 0);
    let hex: HexId = "C4021".into();
    assert_eq!(
        crate::land::stacking::road_halves(&c, &s, &hex, Side::Axis, &[]).unwrap(),
        unit_halves + 1
    );
    {
        let stacks = crate::land::stacking::PlanningStacks::new(&c, &s, Side::Axis, &[], &hex);
        assert_eq!(stacks.road_halves(&hex).unwrap(), unit_halves + 1);
        assert_eq!(stacks.road_halves(&hex).unwrap(), unit_halves + 1);
        assert_eq!(stacks.road_halves(&"C4020".into()).unwrap(), 0);
    }
    assert_eq!(
        crate::land::stacking::road_occupancy_halves(
            &c,
            &s,
            &hex,
            Side::Axis,
            std::slice::from_ref(&id),
            Some(&pool)
        )
        .unwrap(),
        0
    );
    s.land.movement.pool_on_road.remove(&pool);
    s.land.movement.off_road.insert(id);
    assert_eq!(
        crate::land::stacking::road_halves(&c, &s, &hex, Side::Axis, &[]).unwrap(),
        0
    );
}
/// Cases: airlog:53.12, airlog:54.2, land:8.37
#[test]
fn convoy_costs_share_verified_unit_terrain_prices() {
    let t = Trucks {
        medium: 2,
        heavy: 1,
        ..Trucks::default()
    };
    for (route, expected) in [(None, 8), (Some("road"), 2), (Some("track"), 4)] {
        let (c, _overlay) = fixture("clear", route, true, None);
        let actual = step(&c, t, true, false).unwrap();
        assert_eq!(actual.cp_quarters, expected);
        let mut state = State::new(&content()).unwrap();
        let id: UnitId = "it.libyan_tank_command.xxi_l_tank_bn".into();
        state.land.units.get_mut(&id).unwrap().location = crate::state::Location::Hex {
            hex: "C4020".into(),
        };
        let unit = step_cost(
            &c,
            &state,
            &id,
            &"C4020".into(),
            &"C4021".into(),
            true,
            false,
        )
        .unwrap();
        assert_eq!(actual.cp_quarters, unit.cp_quarters);
        assert_eq!(actual.breakdown_quarters, unit.breakdown_quarters);
        assert!(!actual.assumed_edges);
    }
}
/// Cases: airlog:54.2, land:8.44, land:8.48
#[test]
fn mixed_trucks_preserve_every_physical_type_prohibition() {
    let light = Trucks {
        light: 1,
        ..Trucks::default()
    };
    let mixed = Trucks {
        light: 1,
        medium: 1,
        ..Trucks::default()
    };
    let medium = Trucks {
        medium: 1,
        ..Trucks::default()
    };
    let (c, _overlay) = fixture("desert", None, true, None);
    assert!(step(&c, light, true, false).is_err());
    assert!(step(&c, mixed, true, false).is_err());
    assert!(step(&c, medium, true, false).is_ok());
    let (c, _overlay) = fixture("salt_marsh", None, true, None);
    assert!(step(&c, light, true, false).is_ok());
    assert!(step(&c, mixed, true, false).is_err());
    assert!(step(&c, medium, true, false).is_err());
    let (c, _overlay) = fixture("salt_marsh", Some("track"), true, None);
    assert!(step(&c, mixed, true, false).is_ok());
}
/// Cases: land:8.37, land:8.45, land:8.46, land:29.56
#[test]
fn directional_and_rain_crossings_keep_existing_policy() {
    let trucks = Trucks {
        heavy: 1,
        ..Trucks::default()
    };
    let (c, _overlay) = fixture("clear", Some("road"), true, Some("escarpment"));
    assert!(step(&c, trucks, true, false).is_err());
    let (c, _overlay) = fixture("clear", Some("track"), true, Some("wadi"));
    assert!(step(&c, trucks, true, true).is_err());
    let (c, _overlay) = fixture("clear", Some("road"), true, Some("wadi"));
    assert_eq!(step(&c, trucks, true, true).unwrap().cp_quarters, 12);
}
/// Cases: land:8.37
#[test]
fn incomplete_masks_refuse_full_and_flag_dev_without_network_discount() {
    let trucks = Trucks {
        medium: 1,
        ..Trucks::default()
    };
    let (c, _overlay) = fixture("clear", None, false, None);
    assert!(matches!(
        step(&c, trucks, true, false),
        Err(Rejection::Engine(EngineError::Unsupported { .. }))
    ));
    let dev = step(&c, trucks, false, false).unwrap();
    assert_eq!(dev.cp_quarters, 8);
    assert!(dev.assumed_edges);
    assert!(!dev.on_network);
    assert!(step(&c, Trucks::default(), false, false).is_err());
    assert!(
        step(
            &c,
            Trucks {
                light: -1,
                medium: 1,
                ..Trucks::default()
            },
            false,
            false
        )
        .is_err()
    );
}

/// Cases: land:9.29, land:9.33
#[test]
fn planning_and_truth_reject_duplicate_pool_identity_and_checked_pool_overflow() {
    let c = content();
    let mut s = State::new(&c).unwrap();
    s.land.units.clear();
    s.logistics.truck_pools.clear();
    let id = add_pool(&mut s, Side::Axis, "C4021", 1);
    crate::land::convoy_move::record_pool_posture(&mut s, &id, true).unwrap();
    s.logistics.truck_pools[0].trucks.light = i32::MAX;
    s.logistics.truck_pools[0].trucks.medium = 1;
    let hex: HexId = "C4021".into();
    {
        let stacks = crate::land::stacking::PlanningStacks::new(&c, &s, Side::Axis, &[], &hex);
        assert!(matches!(
            stacks.road_halves(&hex),
            Err(EngineError::Invariant { .. })
        ));
        assert!(matches!(
            stacks.road_halves(&hex),
            Err(EngineError::Invariant { .. })
        ));
        assert!(crate::land::stacking::road_halves(&c, &s, &hex, Side::Axis, &[]).is_err());
    }
    let duplicate = s.logistics.truck_pools[0].clone();
    s.logistics.truck_pools.push(duplicate);
    let other: HexId = "C4020".into();
    let stacks = crate::land::stacking::PlanningStacks::new(&c, &s, Side::Axis, &[], &hex);
    assert!(matches!(
        stacks.road_halves(&other),
        Err(EngineError::Invariant { .. })
    ));
    assert!(crate::land::stacking::road_halves(&c, &s, &other, Side::Axis, &[]).is_err());
}
