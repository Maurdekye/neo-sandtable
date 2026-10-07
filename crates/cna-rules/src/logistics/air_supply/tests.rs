use super::super::{
    cargo_history::{CargoHistory, CargoLot, CargoSite},
    water::WaterStage,
};
use super::*;
use crate::{
    Cna,
    air::{facilities::FacilityOrigin, inventory},
    state::{AirSquadron, Location},
    testkit::assert_indistinguishable,
};
use cna_core::quantity::{AmmoPoints, StoresPoints};

fn fixture() -> (CnaContent, State, SgsuId) {
    let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut state = State::new(&content).unwrap();
    state.setup.closed = true;
    let id = SgsuId("axis.supply-test".into());
    state.air.squadrons.insert(
        id.0.clone(),
        AirSquadron {
            id: id.0.clone(),
            force: "axis".into(),
            side: Side::Axis,
            nationality: "it".into(),
            facility: "airfield_benina".into(),
            initial_aircraft: None,
            planes: BTreeMap::new(),
            pilots: BTreeMap::new(),
        },
    );
    inventory::initialize(&content, &mut state).unwrap();
    state.logistics.air_dumps.insert(
        "air-stock".into(),
        AirDump {
            id: "air-stock".into(),
            facility: FacilityId("airfield_benina".into()),
            side: Side::Axis,
            supplies: Supplies {
                fuel: 2,
                ammo: 5,
                stores: 3,
                water: 4,
            },
        },
    );
    (content, state, id)
}
fn bytes(state: &State) -> Vec<u8> {
    serde_json::to_vec(state).unwrap()
}
fn draw(demand: SupplyDemand) -> Vec<SupplyDraw> {
    vec![SupplyDraw {
        source: SupplySource::AirDump("air-stock".into()),
        amount: demand,
    }]
}

/// Cases: airlog:36.15, airlog:36.17, airlog:35.17
#[test]
fn exact_site_and_answering_owner_are_required_without_facility_owner_gate() {
    let (content, mut state, id) = fixture();
    let original = FacilityId("airfield_benina".into());
    let colocated = FacilityId("constructed.test-colocated".into());
    inventory::update(&content, &mut state.air, |runtime| {
        let facility = runtime.facilities.get_mut(&original).unwrap();
        facility.owner = Side::Commonwealth;
        let props = facility.properties(&content, &original)?;
        let mut other = facility.clone();
        other.origin = FacilityOrigin::Constructed {
            kind: props.kind,
            location: props.location,
        };
        runtime.facilities.insert(colocated.clone(), other);
        Ok(())
    })
    .unwrap();
    let prior = BTreeMap::new();
    assert_eq!(
        preview(
            &content,
            &state,
            Side::Axis,
            &id,
            AirSupplyUse::SgsuOperation,
            &prior
        )
        .unwrap()
        .len(),
        1
    );
    let before = bytes(&state);
    for candidate in [&id, &SgsuId("missing".into())] {
        assert_eq!(
            preview(
                &content,
                &state,
                Side::Commonwealth,
                candidate,
                AirSupplyUse::SgsuOperation,
                &prior
            ),
            Err(AirSupplyError::Supply(SupplyError::Invalid))
        );
    }
    assert_eq!(before, bytes(&state));
    state
        .logistics
        .air_dumps
        .get_mut("air-stock")
        .unwrap()
        .facility = colocated;
    assert!(
        preview(
            &content,
            &state,
            Side::Axis,
            &id,
            AirSupplyUse::SgsuOperation,
            &prior
        )
        .unwrap()
        .is_empty()
    );
    state
        .logistics
        .air_dumps
        .get_mut("air-stock")
        .unwrap()
        .facility = original;
    state.logistics.air_dumps.get_mut("air-stock").unwrap().id = "wrong-map-key".into();
    assert_eq!(
        preview(
            &content,
            &state,
            Side::Axis,
            &id,
            AirSupplyUse::SgsuOperation,
            &prior
        ),
        Err(AirSupplyError::Supply(SupplyError::Invalid))
    );
}

/// Cases: airlog:36.17, airlog:49.15, airlog:53.25
#[test]
fn cumulative_fuel_rounding_retires_cargo_and_checkpoint_preserves_credit() {
    let (content, mut state, id) = fixture();
    let site = CargoSite::AirDump("air-stock".into());
    state.logistics.cargo_history.histories.insert(
        site.clone(),
        CargoHistory {
            stage: WaterStage::current(&state),
            lots: vec![CargoLot {
                id: "axis.tagged".into(),
                goods: state.logistics.air_dumps["air-stock"].supplies,
                spent_cp_quarters: 3,
                ceiling_cp_quarters: 16,
                continuous_first_line: false,
            }],
        },
    );
    let mut prior = BTreeMap::new();
    for (amount, cumulative, remaining) in [(2, 2, 1), (8, 10, 1), (1, 11, 0), (9, 20, 0)] {
        let demand = SupplyDemand {
            fuel: FuelTenths::new(amount),
            ..SupplyDemand::default()
        };
        spend(
            &content,
            &mut state,
            Side::Axis,
            &id,
            AirSupplyUse::AircraftServicing,
            AirSupplyDebit {
                demand,
                draws: &draw(demand),
                prior: &prior,
            },
        )
        .unwrap();
        prior.insert(
            SupplySource::AirDump("air-stock".into()),
            FuelTenths::new(cumulative),
        );
        assert_eq!(
            state.logistics.air_dumps["air-stock"].supplies.fuel,
            remaining
        );
        assert_eq!(
            state.logistics.cargo_history.histories[&site].lots[0]
                .goods
                .fuel,
            remaining
        );
        let checkpoint = bytes(&state);
        state = serde_json::from_slice(&checkpoint).unwrap();
        assert_eq!(checkpoint, bytes(&state));
    }
    let goods = SupplyDemand {
        ammo: AmmoPoints::new(2),
        stores: StoresPoints::new(1),
        water: cna_core::quantity::WaterPoints::new(3),
        ..SupplyDemand::default()
    };
    spend(
        &content,
        &mut state,
        Side::Axis,
        &id,
        AirSupplyUse::AircraftServicing,
        AirSupplyDebit {
            demand: goods,
            draws: &draw(goods),
            prior: &prior,
        },
    )
    .unwrap();
    assert_eq!(
        state.logistics.air_dumps["air-stock"].supplies,
        Supplies {
            fuel: 0,
            ammo: 3,
            stores: 2,
            water: 1
        }
    );
    assert_eq!(
        state.logistics.cargo_history.histories[&site].lots[0].goods,
        state.logistics.air_dumps["air-stock"].supplies
    );
    let before = bytes(&state);
    let demand = SupplyDemand {
        fuel: FuelTenths::new(1),
        ..SupplyDemand::default()
    };
    assert_eq!(
        spend(
            &content,
            &mut state,
            Side::Axis,
            &id,
            AirSupplyUse::AircraftServicing,
            AirSupplyDebit {
                demand,
                draws: &draw(demand),
                prior: &prior
            }
        ),
        Err(AirSupplyError::Supply(SupplyError::Insufficient))
    );
    assert_eq!(before, bytes(&state));
}

/// Cases: airlog:36.17, airlog:49.15, airlog:53.25
#[test]
fn failed_debits_are_byte_atomic_and_missing_stock_cannot_spend_old_credit() {
    let (content, mut state, id) = fixture();
    let before = bytes(&state);
    let demand = SupplyDemand {
        ammo: AmmoPoints::new(6),
        stores: StoresPoints::new(1),
        ..SupplyDemand::default()
    };
    assert_eq!(
        spend(
            &content,
            &mut state,
            Side::Axis,
            &id,
            AirSupplyUse::AircraftServicing,
            AirSupplyDebit {
                demand,
                draws: &draw(demand),
                prior: &BTreeMap::new()
            }
        ),
        Err(AirSupplyError::Supply(SupplyError::Insufficient))
    );
    assert_eq!(before, bytes(&state));
    let mut forged = draw(SupplyDemand::default());
    forged[0].source = SupplySource::AirDump("missing".into());
    assert_eq!(
        spend(
            &content,
            &mut state,
            Side::Axis,
            &id,
            AirSupplyUse::AircraftServicing,
            AirSupplyDebit {
                demand: SupplyDemand::default(),
                draws: &forged,
                prior: &BTreeMap::new()
            }
        ),
        Err(AirSupplyError::Supply(SupplyError::Invalid))
    );
    assert_eq!(before, bytes(&state));
    let source = SupplySource::AirDump("missing".into());
    let demand = SupplyDemand {
        fuel: FuelTenths::new(2),
        ..SupplyDemand::default()
    };
    let sources = BTreeMap::from([(source.clone(), demand)]);
    let prior = BTreeMap::from([(source.clone(), FuelTenths::new(2))]);
    let draws = [SupplyDraw {
        source,
        amount: demand,
    }];
    assert!(matches!(
        super::super::supply::withdraw_draws(
            &state.logistics,
            None,
            demand,
            &draws,
            &sources,
            &prior
        ),
        Err(SupplyError::Invalid)
    ));
    assert_eq!(before, bytes(&state));
}

/// Cases: land:3.6, airlog:36.17, airlog:49.15, airlog:49.18, airlog:53.25
#[test]
fn enemy_stock_and_history_stay_private_and_old_land_pool_sources_exclude_air() {
    let (content, a, id) = fixture();
    let mut b = a.clone();
    b.logistics
        .air_dumps
        .get_mut("air-stock")
        .unwrap()
        .supplies
        .fuel = 50;
    let site = CargoSite::AirDump("air-stock".into());
    b.logistics.cargo_history.histories.insert(
        site,
        CargoHistory {
            stage: WaterStage::current(&b),
            lots: vec![CargoLot {
                id: "axis.hidden-tag".into(),
                goods: Supplies {
                    stores: 1,
                    ..Supplies::default()
                },
                spent_cp_quarters: 5,
                ceiling_cp_quarters: 16,
                continuous_first_line: false,
            }],
        },
    );
    assert_indistinguishable(&Cna::dev(), &content, &a, &b, Side::Commonwealth);
    assert_eq!(
        preview(
            &content,
            &a,
            Side::Commonwealth,
            &id,
            AirSupplyUse::SgsuOperation,
            &BTreeMap::new()
        ),
        preview(
            &content,
            &b,
            Side::Commonwealth,
            &id,
            AirSupplyUse::SgsuOperation,
            &BTreeMap::new()
        )
    );
    let location = a.air.runtime.sgsus[&id]
        .location(&content, &a.air.runtime.facilities)
        .unwrap();
    assert!(
        super::super::supply::local_stocks(&b, Side::Axis, &location)
            .unwrap()
            .iter()
            .all(|d| !matches!(d.source, SupplySource::AirDump(_)))
    );
    let mut old = b.clone();
    let unit_id: cna_core::ids::UnitId = "it.1_libyan_div.viii_libyan_bn".into();
    old.land.units.get_mut(&unit_id).unwrap().location = location.clone();
    assert!(
        super::super::available_sources_with_content(&content, &old, &unit_id)
            .unwrap()
            .iter()
            .all(|d| !matches!(d.source, SupplySource::AirDump(_)))
    );
    let fuel = SupplyDemand {
        fuel: FuelTenths::new(1),
        ..SupplyDemand::default()
    };
    let old_before = bytes(&old);
    assert_eq!(
        super::super::spend_for_unit_with_content(&content, &mut old, &unit_id, fuel, &draw(fuel)),
        Err(SupplyError::Invalid)
    );
    assert_eq!(old_before, bytes(&old));
    let Location::Hex { hex } = &location else {
        unreachable!()
    };
    let pool = super::super::pools::add_truck_pool(
        &mut old.logistics,
        None,
        Side::Axis,
        cna_content::scenario::Placement::Hex { hex: hex.clone() },
        Some(location.clone()),
        cna_content::units::Trucks {
            light: 1,
            ..Default::default()
        },
        Supplies::default(),
    )
    .unwrap();
    assert!(
        super::super::pool_fuel::available_pool_sources_at(&content, &old, &pool, &location)
            .unwrap()
            .iter()
            .all(|d| !matches!(d.source, SupplySource::AirDump(_)))
    );
    let mut legacy = bytes(&a);
    let mut value: serde_json::Value = serde_json::from_slice(&legacy).unwrap();
    value["logistics"]
        .as_object_mut()
        .unwrap()
        .remove("air_dumps");
    legacy = serde_json::to_vec(&value).unwrap();
    let restored: State = serde_json::from_slice(&legacy).unwrap();
    assert!(restored.logistics.air_dumps.is_empty());
    assert!(matches!(location, Location::Hex { .. }));
}

/// Cases: scen:59.35, airlog:36.17, airlog:35.11
#[test]
fn own_source_gap_retains_case_and_detail_after_owner_authorization() {
    let (mut content, mut state, id) = fixture();
    let source = content
        .scenario
        .facilities
        .facilities
        .iter_mut()
        .find(|f| f.id == "airfield_benina")
        .unwrap();
    source.hex = Some("unverified.air-source-hex".into());
    source.hexes.clear();
    let original = state.air.runtime.sgsus[&id]
        .location(&content, &state.air.runtime.facilities)
        .unwrap_err();
    assert!(matches!(&original, EngineError::Unsupported { case, .. } if case == "scen:59.35"));
    let prior = BTreeMap::new();
    assert_eq!(
        preview(
            &content,
            &state,
            Side::Axis,
            &id,
            AirSupplyUse::SgsuOperation,
            &prior
        ),
        Err(AirSupplyError::Canonical(original.clone()))
    );
    let before = bytes(&state);
    let demand = SupplyDemand {
        fuel: FuelTenths::new(1),
        ..Default::default()
    };
    assert_eq!(
        spend(
            &content,
            &mut state,
            Side::Axis,
            &id,
            AirSupplyUse::SgsuOperation,
            AirSupplyDebit {
                demand,
                draws: &draw(demand),
                prior: &prior
            }
        ),
        Err(AirSupplyError::Canonical(original))
    );
    assert_eq!(bytes(&state), before);
    for candidate in [&id, &SgsuId("missing".into())] {
        assert_eq!(
            preview(
                &content,
                &state,
                Side::Commonwealth,
                candidate,
                AirSupplyUse::SgsuOperation,
                &prior
            ),
            Err(AirSupplyError::Supply(SupplyError::Invalid))
        );
    }
}

/// Cases: airlog:36.17, airlog:35.11, airlog:36.12
#[test]
fn own_malformed_canonical_site_retains_invariant_and_failed_spend_is_atomic() {
    let (content, mut state, id) = fixture();
    state
        .air
        .runtime
        .facilities
        .get_mut(&FacilityId("airfield_benina".into()))
        .unwrap()
        .current_capacity = crate::air::facilities::FacilityCapacity::Levels(7);
    let original = state.air.runtime.sgsus[&id]
        .location(&content, &state.air.runtime.facilities)
        .unwrap_err();
    assert!(matches!(&original, EngineError::Invariant { detail } if detail.contains("capacity")));
    let prior = BTreeMap::new();
    assert_eq!(
        preview(
            &content,
            &state,
            Side::Axis,
            &id,
            AirSupplyUse::AircraftServicing,
            &prior
        ),
        Err(AirSupplyError::Canonical(original.clone()))
    );
    let before = bytes(&state);
    let demand = SupplyDemand {
        fuel: FuelTenths::new(1),
        ..Default::default()
    };
    assert_eq!(
        spend(
            &content,
            &mut state,
            Side::Axis,
            &id,
            AirSupplyUse::AircraftServicing,
            AirSupplyDebit {
                demand,
                draws: &draw(demand),
                prior: &prior
            }
        ),
        Err(AirSupplyError::Canonical(original))
    );
    assert_eq!(bytes(&state), before);
    for candidate in [&id, &SgsuId("missing".into())] {
        assert_eq!(
            preview(
                &content,
                &state,
                Side::Commonwealth,
                candidate,
                AirSupplyUse::AircraftServicing,
                &prior
            ),
            Err(AirSupplyError::Supply(SupplyError::Invalid))
        );
    }
}
