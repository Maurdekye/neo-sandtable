use super::*;
use crate::{
    CnaContent,
    state::{Dump, DumpLocation, Location},
};
use cna_content::{scenario::Placement, units::Trucks};
use std::sync::OnceLock;
fn content() -> &'static CnaContent {
    static C: OnceLock<CnaContent> = OnceLock::new();
    C.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn game() -> (State, CargoSite, CargoSite, CargoSite) {
    let mut s = State::new(content()).unwrap();
    s.cursor.op_stage = Some(1);
    let mut pools = vec![];
    for stores in [10, 0] {
        let id = super::super::pools::add_truck_pool(
            &mut s.logistics,
            None,
            Side::Axis,
            Placement::Hex {
                hex: "C4020".into(),
            },
            Some(Location::Hex {
                hex: "C4020".into(),
            }),
            Trucks {
                medium: 1,
                ..Trucks::default()
            },
            Supplies {
                stores,
                ..Supplies::default()
            },
        )
        .unwrap();
        pools.push(CargoSite::Pool(id));
    }
    let dump = CargoSite::Dump("cargo-test".into());
    s.logistics.dumps.insert(
        "cargo-test".into(),
        Dump {
            id: "cargo-test".into(),
            marker: "dump-900".into(),
            side: Side::Axis,
            location: DumpLocation::Hex {
                hex: "C4020".into(),
            },
            supplies: Supplies::default(),
            active: true,
            dummy: false,
        },
    );
    (s, pools.remove(0), pools.remove(0), dump)
}
fn goods(n: i32) -> Supplies {
    Supplies {
        stores: n,
        ..Supplies::default()
    }
}
fn physically_transfer(s: &mut State, from: &CargoSite, to: &CargoSite, n: i32) {
    let change = |s: &mut State, site: &CargoSite, delta: i32| match site {
        CargoSite::Pool(id) => {
            s.logistics
                .truck_pools
                .iter_mut()
                .find(|p| &p.id == id)
                .unwrap()
                .cargo
                .stores += delta
        }
        CargoSite::Dump(id) => s.logistics.dumps.get_mut(id).unwrap().supplies.stores += delta,
        CargoSite::Unit(id) => {
            s.logistics
                .unit_supply
                .entry(id.clone())
                .or_default()
                .carried
                .stores += delta
        }
        _ => panic!("test site"),
    };
    change(s, from, -n);
    change(s, to, n);
}
fn timing(cp: i32, cpa: i32) -> CarrierTiming {
    CarrierTiming {
        spent_cp_quarters: cp,
        cpa_quarters: cpa,
    }
}
fn all(s: &State, site: &CargoSite) -> Vec<LotSelection> {
    parcels(s, Side::Axis, site)
        .unwrap()
        .into_iter()
        .map(|p| LotSelection {
            lot: p.lot,
            goods: p.goods,
        })
        .collect()
}
/// Cases: airlog:53.24, airlog:53.25
/// Interpretations: interp:airlog-0020
#[test]
fn unload_reload_preserves_used_cp_and_first_allowance_and_rejection_is_atomic() {
    let (mut s, a, b, d) = game();
    advance(&mut s, Side::Axis, &a, timing(0, 120), 80).unwrap();
    let choice = all(&s, &a);
    transfer(&mut s, Side::Axis, &a, &d, goods(10), &choice, None).unwrap();
    physically_transfer(&mut s, &a, &d, 10);
    let choice = all(&s, &d);
    transfer(
        &mut s,
        Side::Axis,
        &d,
        &b,
        goods(10),
        &choice,
        Some(timing(8, 160)),
    )
    .unwrap();
    physically_transfer(&mut s, &d, &b, 10);
    let p = parcels(&s, Side::Axis, &b).unwrap();
    assert_eq!(p[0].spent_cp_quarters, 80);
    assert_eq!(p[0].ceiling_cp_quarters, Some(120));
    advance(&mut s, Side::Axis, &b, timing(8, 160), 40).unwrap();
    assert_eq!(
        parcels(&s, Side::Axis, &b).unwrap()[0].spent_cp_quarters,
        120
    );
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(
        advance(&mut s, Side::Axis, &b, timing(48, 160), 1),
        Err(CargoError::Ceiling)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    let text = format!("{:?}", rejection(CargoError::Ceiling));
    assert!(text.contains("53.25") && text.contains("ceiling"));
}
/// Cases: airlog:53.25
#[test]
fn recipient_past_cp_can_only_raise_the_load_history_and_partial_choices_conserve_every_type() {
    let (mut s, a, b, d) = game();
    advance(&mut s, Side::Axis, &a, timing(0, 120), 16).unwrap();
    let p = parcels(&s, Side::Axis, &a).unwrap()[0].clone();
    transfer(
        &mut s,
        Side::Axis,
        &a,
        &d,
        goods(4),
        &[LotSelection {
            lot: p.lot.clone(),
            goods: goods(4),
        }],
        None,
    )
    .unwrap();
    physically_transfer(&mut s, &a, &d, 4);
    let dlot = parcels(&s, Side::Axis, &d).unwrap()[0].clone();
    assert_ne!(p.lot, dlot.lot);
    assert_eq!(parcels(&s, Side::Axis, &a).unwrap()[0].goods.stores, 6);
    let choices = all(&s, &d);
    transfer(
        &mut s,
        Side::Axis,
        &d,
        &b,
        goods(4),
        &choices,
        Some(timing(32, 160)),
    )
    .unwrap();
    physically_transfer(&mut s, &d, &b, 4);
    assert_eq!(
        parcels(&s, Side::Axis, &b).unwrap()[0].spent_cp_quarters,
        32
    );
    let before = serde_json::to_value(&s).unwrap();
    let choices = all(&s, &a);
    let repeated = vec![choices[0].clone(), choices[0].clone()];
    assert_eq!(
        transfer(&mut s, Side::Axis, &a, &d, goods(12), &repeated, None),
        Err(CargoError::Invalid)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    assert!(transfer(&mut s, Side::Commonwealth, &a, &d, goods(1), &choices, None).is_err());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}
/// Cases: airlog:53.22, airlog:53.24, airlog:53.25
#[test]
fn continuous_first_line_overrun_can_deliver_but_cannot_reload_until_next_stage() {
    let (mut s, _, b, d) = game();
    let id = s
        .land
        .units
        .values()
        .find(|u| u.side == Side::Axis && u.location.hex().is_some())
        .unwrap()
        .id
        .clone();
    let a = CargoSite::Unit(id.clone());
    s.logistics.unit_supply.entry(id).or_default().carried = goods(10);
    advance(&mut s, Side::Axis, &a, timing(0, 80), 100).unwrap();
    advance(&mut s, Side::Axis, &a, timing(100, 80), 8).unwrap();
    let choices = all(&s, &a);
    transfer(&mut s, Side::Axis, &a, &d, goods(10), &choices, None).unwrap();
    physically_transfer(&mut s, &a, &d, 10);
    let p = parcels(&s, Side::Axis, &d).unwrap()[0].clone();
    assert_eq!(p.spent_cp_quarters, 108);
    assert_eq!(p.ceiling_cp_quarters, Some(80));
    let before = serde_json::to_value(&s).unwrap();
    let choices = all(&s, &d);
    assert_eq!(
        transfer(
            &mut s,
            Side::Axis,
            &d,
            &b,
            goods(10),
            &choices,
            Some(timing(0, 120))
        ),
        Err(CargoError::Ceiling)
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    s.cursor.op_stage = Some(2);
    assert_eq!(all(&s, &d)[0].lot, "fresh");
    let choices = all(&s, &d);
    transfer(
        &mut s,
        Side::Axis,
        &d,
        &b,
        goods(10),
        &choices,
        Some(timing(0, 120)),
    )
    .unwrap();
    physically_transfer(&mut s, &d, &b, 10);
    assert_eq!(parcels(&s, Side::Axis, &b).unwrap()[0].spent_cp_quarters, 0);
}
/// Cases: airlog:53.25
#[test]
fn automatic_consumption_prefers_most_spent_then_lower_ceiling_then_id_before_fresh() {
    let (mut s, _, _, d) = game();
    let stage = WaterStage::current(&s);
    s.logistics.dumps.get_mut("cargo-test").unwrap().supplies = goods(20);
    let lot = |id: &str, n, cp, cap| CargoLot {
        id: id.into(),
        goods: goods(n),
        spent_cp_quarters: cp,
        ceiling_cp_quarters: cap,
        continuous_first_line: false,
    };
    s.logistics.cargo_history.histories.insert(
        d.clone(),
        CargoHistory {
            stage,
            lots: vec![
                lot("axis.cargo-4", 2, 40, 120),
                lot("axis.cargo-3", 2, 40, 100),
                lot("axis.cargo-2", 2, 40, 100),
                lot("axis.cargo-1", 2, 60, 120),
            ],
        },
    );
    retire_debit(&mut s.logistics, &d, goods(5)).unwrap();
    s.logistics
        .dumps
        .get_mut("cargo-test")
        .unwrap()
        .supplies
        .stores -= 5;
    let p = parcels(&s, Side::Axis, &d).unwrap();
    assert!(
        p.iter()
            .all(|p| p.lot != "axis.cargo-1" && p.lot != "axis.cargo-2")
    );
    assert_eq!(
        p.iter()
            .find(|p| p.lot == "axis.cargo-3")
            .unwrap()
            .goods
            .stores,
        1
    );
    assert_eq!(
        p.iter().find(|p| p.lot == "fresh").unwrap().goods.stores,
        12
    );
}

/// Cases: airlog:53.25, land:3.6
#[test]
fn history_checkpoint_snapshot_and_enemy_views_preserve_identity_without_disclosing_lots() {
    let (mut a, p, _, _) = game();
    let before = a.clone();
    let snapshot = snapshot(&a, std::slice::from_ref(&p));
    advance(&mut a, Side::Axis, &p, timing(0, 120), 8).unwrap();
    crate::testkit::assert_indistinguishable(
        &crate::Cna::dev(),
        content(),
        &a,
        &before,
        Side::Commonwealth,
    );
    assert_eq!(disclosed(&a, Perspective::Side(Side::Axis)).len(), 1);
    assert!(disclosed(&a, Perspective::Side(Side::Commonwealth)).is_empty());
    let json = serde_json::to_value(&a).unwrap();
    let mut b: State = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(
        parcels(&a, Side::Axis, &p).unwrap(),
        parcels(&b, Side::Axis, &p).unwrap()
    );
    advance(&mut a, Side::Axis, &p, timing(8, 120), 4).unwrap();
    advance(&mut b, Side::Axis, &p, timing(8, 120), 4).unwrap();
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(&b).unwrap()
    );
    restore(&mut a, &snapshot);
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    let mut duplicate = json;
    let entries = duplicate["logistics"]["cargo_history"]["histories"]
        .as_array_mut()
        .unwrap();
    entries.push(entries[0].clone());
    assert!(serde_json::from_value::<State>(duplicate).is_err());
}

/// Cases: airlog:49.18, airlog:53.25
/// Interpretations: interp:airlog-0001, interp:airlog-0020
#[test]
fn shared_supply_debits_retire_exact_whole_goods_and_keep_fractional_fuel_credit() {
    use super::super::{SupplyDemand, SupplyDraw, SupplySource};
    use cna_core::quantity::{AmmoPoints, FuelTenths, StoresPoints, WaterPoints};
    let (mut s, a, _, _) = game();
    let CargoSite::Pool(id) = &a else {
        unreachable!()
    };
    let stock = Supplies {
        ammo: 20,
        fuel: 20,
        stores: 20,
        water: 20,
    };
    s.logistics
        .truck_pools
        .iter_mut()
        .find(|p| &p.id == id)
        .unwrap()
        .cargo = stock;
    advance(&mut s, Side::Axis, &a, timing(0, 120), 40).unwrap();
    let source = SupplySource::PoolStock(id.clone());
    let demand = SupplyDemand {
        fuel: FuelTenths::new(1),
        ammo: AmmoPoints::new(1),
        stores: StoresPoints::new(1),
        water: WaterPoints::new(1),
    };
    let capacities = BTreeMap::from([(
        source.clone(),
        SupplyDemand {
            fuel: FuelTenths::new(200),
            ammo: AmmoPoints::new(20),
            stores: StoresPoints::new(20),
            water: WaterPoints::new(20),
        },
    )]);
    s.logistics = super::super::supply::withdraw_draws(
        &s.logistics,
        None,
        demand,
        &[SupplyDraw {
            source: source.clone(),
            amount: demand,
        }],
        &capacities,
        &BTreeMap::new(),
    )
    .unwrap();
    let p = parcels(&s, Side::Axis, &a).unwrap();
    assert_eq!(p.len(), 1);
    assert_eq!(
        p[0].goods,
        Supplies {
            ammo: 19,
            fuel: 19,
            stores: 19,
            water: 19
        }
    );
    // Fresh additions cannot conceal the tagged debit that already happened.
    s.logistics
        .truck_pools
        .iter_mut()
        .find(|p| &p.id == id)
        .unwrap()
        .cargo
        .fuel += 7;
    let demand = SupplyDemand {
        fuel: FuelTenths::new(1),
        ..SupplyDemand::default()
    };
    s.logistics = super::super::supply::withdraw_draws(
        &s.logistics,
        None,
        demand,
        &[SupplyDraw {
            source: source.clone(),
            amount: demand,
        }],
        &capacities,
        &BTreeMap::from([(source, FuelTenths::new(1))]),
    )
    .unwrap();
    let p = parcels(&s, Side::Axis, &a).unwrap();
    assert_eq!(p.iter().find(|p| p.lot != "fresh").unwrap().goods.fuel, 19);
    assert_eq!(p.iter().find(|p| p.lot == "fresh").unwrap().goods.fuel, 7);
    let mut legacy = serde_json::to_value(&s).unwrap();
    legacy["logistics"]
        .as_object_mut()
        .unwrap()
        .remove("cargo_history");
    let legacy: State = serde_json::from_value(legacy).unwrap();
    assert!(legacy.logistics.cargo_history.histories.is_empty());
}
