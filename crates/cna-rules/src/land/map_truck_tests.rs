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
