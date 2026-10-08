use super::*;
use crate::land::breakdown::{Equipment, markers::BrokenMarker, pools::PoolAsset};
use crate::logistics::cargo_history::tests::{content, game};
use cna_core::{
    ids::HexId,
    quantity::{FuelTenths, WaterPoints},
};

fn goods(n: i32) -> Supplies {
    Supplies {
        stores: n,
        ..Supplies::default()
    }
}
fn packing(n: i32) -> CargoPacking {
    CargoPacking {
        medium: goods(n),
        ..CargoPacking::default()
    }
}
fn fixture(tagged: bool) -> (State, String, Vec<PoolCargoShare>) {
    let (mut s, site, _, _) = game();
    let CargoSite::Pool(id) = site else {
        unreachable!()
    };
    let p = s
        .logistics
        .truck_pools
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap();
    p.trucks.medium = 3;
    if tagged {
        s.logistics.cargo_history.histories.insert(
            CargoSite::Pool(id.clone()),
            CargoHistory {
                stage: WaterStage::current(&s),
                lots: vec![CargoLot {
                    id: "axis.cargo-1".into(),
                    goods: goods(10),
                    spent_cp_quarters: 16,
                    ceiling_cp_quarters: 120,
                    continuous_first_line: true,
                }],
            },
        );
        s.logistics.cargo_history.next_id.insert(Side::Axis, 1);
    }
    let mut shares = vec![];
    for (marker_id, amount, hex) in [("broken-axis-1", 3, "C4020"), ("broken-axis-2", 4, "C4021")] {
        s.land.breakdown.markers.insert(
            marker_id.into(),
            BrokenMarker {
                id: marker_id.into(),
                side: Side::Axis,
                hex: HexId::new(hex),
                assets: vec![],
                source_pool: Some(id.clone()),
                pool_assets: vec![PoolAsset {
                    pool: id.clone(),
                    equipment: Equipment::MediumTruck,
                    points: 1,
                    cohort: format!("{marker_id}.truck"),
                }],
                pool_fuel_cohorts: vec![],
                passengers: BTreeMap::new(),
                transport: Trucks::default(),
                cargo: packing(amount),
                tank_fuel: FuelTenths::ZERO,
                activity_water: WaterPoints::ZERO,
                fuel_cohorts: vec![],
                paid_truck_water: Default::default(),
                water_credit_stage: None,
            },
        );
        shares.push(PoolCargoShare {
            marker: marker_id.into(),
            goods: goods(amount),
        });
    }
    (s, id, shares)
}
fn reduce(s: &mut State, id: &str) {
    let p = s
        .logistics
        .truck_pools
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap();
    p.trucks.medium = 1;
    p.cargo = goods(3);
}
/// Cases: land:21.43, airlog:53.25, airlog:54.2
#[test]
fn mixed_goods_partition_conserves_each_component_and_unrelated_history() {
    let (mut s, id, mut shares) = fixture(true);
    let all = |n| Supplies {
        ammo: n,
        fuel: n,
        stores: n,
        water: n,
    };
    let pack = |n| CargoPacking {
        medium: all(n),
        ..CargoPacking::default()
    };
    s.logistics
        .truck_pools
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap()
        .cargo = all(4);
    s.logistics
        .cargo_history
        .histories
        .get_mut(&CargoSite::Pool(id.clone()))
        .unwrap()
        .lots[0]
        .goods = all(4);
    for (share, n) in shares.iter_mut().zip([2, 1]) {
        share.goods = all(n);
        s.land
            .breakdown
            .markers
            .get_mut(&share.marker)
            .unwrap()
            .cargo = pack(n);
    }
    let unrelated = CargoSite::Dump("untouched".into());
    let extra = CargoHistory {
        stage: WaterStage::current(&s),
        lots: vec![CargoLot {
            id: "cw.cargo-77".into(),
            goods: all(1),
            spent_cp_quarters: 24,
            ceiling_cp_quarters: 160,
            continuous_first_line: false,
        }],
    };
    s.logistics
        .cargo_history
        .histories
        .insert(unrelated.clone(), extra.clone());
    let prepared =
        prepare_pool_breakdown_relocation(content(), &s, Side::Axis, &id, &pack(1), &shares)
            .unwrap();
    let p = s
        .logistics
        .truck_pools
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap();
    p.trucks.medium = 1;
    p.cargo = all(1);
    apply_pool_breakdown_relocation(&mut s, prepared).unwrap();
    let mut total = Supplies::default();
    for site in [
        CargoSite::Pool(id),
        CargoSite::BrokenMarker(shares[0].marker.clone()),
        CargoSite::BrokenMarker(shares[1].marker.clone()),
    ] {
        let h = &s.logistics.cargo_history.histories[&site];
        add(&mut total, &totals(h).unwrap()).unwrap();
        assert_eq!(totals(h).unwrap(), stock(&s, &site).unwrap());
        assert!(h.lots.iter().all(|l| l.spent_cp_quarters == 16
            && l.ceiling_cp_quarters == 120
            && l.continuous_first_line));
    }
    assert_eq!(total, all(4));
    assert_eq!(s.logistics.cargo_history.histories[&unrelated], extra);
}
/// Cases: land:21.43, airlog:53.25, airlog:54.2
#[test]
fn two_marker_partition_preserves_cp_ceiling_continuity_and_physical_history() {
    let (mut s, id, shares) = fixture(true);
    crate::logistics::pool_fuel::seed_created_pool(&mut s, &id).unwrap();
    let before = serde_json::to_value(&s).unwrap();
    let prepared =
        prepare_pool_breakdown_relocation(content(), &s, Side::Axis, &id, &packing(3), &shares)
            .unwrap();
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    let pool_site = CargoSite::Pool(id.clone());
    let parent = crate::logistics::pool_fuel::unfunded_pool_physical_cohorts(&s, &id).unwrap()[0]
        .id
        .clone();
    for share in &shares {
        motion::transfer(
            &mut s,
            Side::Axis,
            &pool_site,
            &CargoSite::BrokenMarker(share.marker.clone()),
            &[motion::PhysicalTrucks {
                id: format!("{}.truck", share.marker),
                parent: Some(parent.clone()),
                kind: crate::logistics::FuelTruckKind::Medium,
                count: 1,
            }],
        )
        .unwrap();
    }
    reduce(&mut s, &id);
    let motion_before = serde_json::to_value(&s.logistics.cargo_history.motion).unwrap();
    apply_pool_breakdown_relocation(&mut s, prepared).unwrap();
    assert_eq!(
        serde_json::to_value(&s.logistics.cargo_history.motion).unwrap(),
        motion_before
    );
    let sites = [
        pool_site.clone(),
        CargoSite::BrokenMarker(shares[0].marker.clone()),
        CargoSite::BrokenMarker(shares[1].marker.clone()),
    ];
    let mut quantity = Supplies::default();
    let mut ids = BTreeSet::new();
    for site in &sites {
        let history = &s.logistics.cargo_history.histories[site];
        assert_eq!(history.stage, WaterStage::current(&s));
        for lot in &history.lots {
            assert!(ids.insert(lot.id.clone()));
            assert_eq!(
                (
                    lot.spent_cp_quarters,
                    lot.ceiling_cp_quarters,
                    lot.continuous_first_line
                ),
                (16, 120, true)
            );
            add(&mut quantity, &lot.goods).unwrap();
        }
        assert_eq!(totals(history).unwrap(), stock(&s, site).unwrap());
    }
    assert_eq!(quantity, goods(10));
    assert_eq!(
        s.logistics.cargo_history.histories[&pool_site].lots[0].id,
        "axis.cargo-1"
    );
    let json = serde_json::to_value(&s).unwrap();
    let restored: State = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), json);
}
/// Cases: land:21.43, airlog:53.25, airlog:54.2
#[test]
fn fresh_stock_stays_untagged_and_a_full_parcel_moves_without_new_identity() {
    let (mut fresh, id, shares) = fixture(false);
    let prepared =
        prepare_pool_breakdown_relocation(content(), &fresh, Side::Axis, &id, &packing(3), &shares)
            .unwrap();
    let serials = fresh.logistics.cargo_history.next_id.clone();
    reduce(&mut fresh, &id);
    apply_pool_breakdown_relocation(&mut fresh, prepared).unwrap();
    assert_eq!(fresh.logistics.cargo_history.next_id, serials);
    for site in [
        CargoSite::Pool(id),
        CargoSite::BrokenMarker(shares[0].marker.clone()),
        CargoSite::BrokenMarker(shares[1].marker.clone()),
    ] {
        assert!(!fresh.logistics.cargo_history.histories.contains_key(&site));
        let p = parcels(&fresh, Side::Axis, &site).unwrap();
        assert_eq!(p[0].lot, "fresh");
        assert_eq!(p[0].ceiling_cp_quarters, None);
    }
    let (mut s, id, mut shares) = fixture(true);
    shares.truncate(1);
    shares[0].goods = goods(10);
    s.land
        .breakdown
        .markers
        .get_mut(&shares[0].marker)
        .unwrap()
        .cargo = packing(10);
    let prepared =
        prepare_pool_breakdown_relocation(content(), &s, Side::Axis, &id, &packing(0), &shares)
            .unwrap();
    let p = s
        .logistics
        .truck_pools
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap();
    p.trucks.medium = 2;
    p.cargo = Supplies::default();
    apply_pool_breakdown_relocation(&mut s, prepared).unwrap();
    let target = CargoSite::BrokenMarker(shares[0].marker.clone());
    assert_eq!(
        s.logistics.cargo_history.histories[&target].lots[0].id,
        "axis.cargo-1"
    );
    assert_eq!(s.logistics.cargo_history.next_id[&Side::Axis], 1);
}
/// Cases: land:21.43, airlog:53.25
#[test]
fn distinct_histories_are_exact_unsupported_and_prepare_never_mutates() {
    let (mut s, id, shares) = fixture(true);
    let site = CargoSite::Pool(id.clone());
    let h = s.logistics.cargo_history.histories.get_mut(&site).unwrap();
    h.lots[0].goods = goods(5);
    h.lots.push(CargoLot {
        id: "axis.cargo-2".into(),
        goods: goods(5),
        spent_cp_quarters: 20,
        ceiling_cp_quarters: 120,
        continuous_first_line: true,
    });
    let before = serde_json::to_value(&s).unwrap();
    let error =
        prepare_pool_breakdown_relocation(content(), &s, Side::Axis, &id, &packing(3), &shares)
            .unwrap_err();
    assert!(matches!(error, EngineError::Unsupported { case, .. } if case == "airlog:53.25"));
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}
/// Cases: land:21.43, airlog:53.25, airlog:54.2
#[test]
fn overflow_capacity_and_stale_footprints_fail_before_any_history_change() {
    let (mut overflow, id, shares) = fixture(true);
    overflow
        .logistics
        .cargo_history
        .next_id
        .insert(Side::Axis, u64::MAX);
    let before = serde_json::to_value(&overflow).unwrap();
    assert!(matches!(
        prepare_pool_breakdown_relocation(
            content(),
            &overflow,
            Side::Axis,
            &id,
            &packing(3),
            &shares
        ),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(serde_json::to_value(&overflow).unwrap(), before);
    let (s, id, shares) = fixture(true);
    for mode in 0..3 {
        let mut state = s.clone();
        let prepared = prepare_pool_breakdown_relocation(
            content(),
            &state,
            Side::Axis,
            &id,
            &packing(3),
            &shares,
        )
        .unwrap();
        reduce(&mut state, &id);
        match mode {
            0 => {
                state
                    .land
                    .breakdown
                    .markers
                    .get_mut(&shares[0].marker)
                    .unwrap()
                    .cargo = packing(2)
            }
            1 => state.cursor.op_stage = Some(2),
            _ => {
                state.logistics.cargo_history.next_id.insert(Side::Axis, 19);
            }
        }
        let before = serde_json::to_value(&state).unwrap();
        assert!(matches!(
            apply_pool_breakdown_relocation(&mut state, prepared),
            Err(EngineError::Invariant { .. })
        ));
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
    }
    let mut no_body = s.clone();
    no_body
        .land
        .breakdown
        .markers
        .get_mut(&shares[0].marker)
        .unwrap()
        .pool_assets
        .clear();
    let before = serde_json::to_value(&no_body).unwrap();
    assert!(matches!(
        prepare_pool_breakdown_relocation(
            content(),
            &no_body,
            Side::Axis,
            &id,
            &packing(3),
            &shares
        ),
        Err(EngineError::Invariant { .. })
    ));
    assert_eq!(serde_json::to_value(&no_body).unwrap(), before);
}
