//! Numbered broken vehicles preserve every physical holding; enemy views disclose presence only.
use super::{Asset, Equipment};
use crate::State;
use cna_content::units::Trucks;
use cna_core::{
    engine::EngineError,
    event::EngineEvent,
    ids::{HexId, UnitId},
    quantity::{FuelTenths, WaterPoints},
    visibility::Audience,
};
use cna_protocol::{GameEvent, Side, Stack};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A trusted, validated allocation batch; never decoded from an actor answer.
pub struct PreparedPoolMarkers {
    markers: Vec<BrokenMarker>,
    expected_serials: BTreeMap<Side, u64>,
    ids: Vec<String>,
}

fn marker_invariant() -> EngineError {
    EngineError::Invariant {
        detail: "invalid convoy marker allocation or holdings".into(),
    }
}

fn marker_supply(error: crate::logistics::SupplyError) -> EngineError {
    match error {
        crate::logistics::SupplyError::Unsupported { case } => EngineError::Unsupported {
            case: case.into(),
            detail: "convoy marker capacity is unavailable".into(),
        },
        _ => marker_invariant(),
    }
}

/// Validate all marker counts and serials before invoking the legacy infallible writer.
/// The outer loss transaction must already bind exact selected physical cohorts and
/// conserve source/working/marker holdings; this helper creates no selection entitlement.
/// Empty candidates consume no id. Pool-only candidates carry no Unit paid-water credit.
/// Cases: land:21.25, land:21.29, land:21.42, land:21.43, airlog:54.2
pub fn prepare_pool_markers(
    c: &crate::CnaContent,
    s: &State,
    candidates: &[BrokenMarker],
) -> Result<PreparedPoolMarkers, EngineError> {
    use crate::logistics::{FuelTruckKind, capacity};
    use cna_tables::airlog::trucks::TruckType;
    let mut prepared = PreparedPoolMarkers {
        markers: vec![],
        expected_serials: BTreeMap::new(),
        ids: vec![],
    };
    let mut serials = s.land.breakdown.next_marker.clone();
    let mut physical_ids = BTreeSet::new();
    for marker in candidates {
        if !marker.id.is_empty()
            || !marker.assets.is_empty()
            || !marker.passengers.is_empty()
            || marker.transport != Trucks::default()
            || !marker.fuel_cohorts.is_empty()
            || marker.paid_truck_water != crate::logistics::TruckWater::default()
            || marker.water_credit_stage.is_some()
            || c.map.get(&marker.hex).is_none()
            || marker.tank_fuel.get() < 0
            || marker.activity_water.get() < 0
        {
            return Err(marker_invariant());
        }
        let source = marker.source_pool.as_deref().ok_or_else(marker_invariant)?;
        let mut pools = s.logistics.truck_pools.iter().filter(|p| p.id == source);
        let p = pools.next().ok_or_else(marker_invariant)?;
        if source.is_empty() || pools.next().is_some() || p.side != marker.side {
            return Err(marker_invariant());
        }
        let mut counts = BTreeMap::new();
        let mut trucks = Trucks::default();
        for asset in &marker.pool_assets {
            let (kind, count) = match asset.equipment {
                Equipment::LightTruck => (FuelTruckKind::Light, &mut trucks.light),
                Equipment::MediumTruck => (FuelTruckKind::Medium, &mut trucks.medium),
                Equipment::HeavyTruck => (FuelTruckKind::Heavy, &mut trucks.heavy),
                _ => return Err(marker_invariant()),
            };
            if asset.pool != source
                || asset.points <= 0
                || asset.cohort.is_empty()
                || counts
                    .insert(asset.cohort.clone(), (kind, asset.points))
                    .is_some()
            {
                return Err(marker_invariant());
            }
            *count = count
                .checked_add(asset.points)
                .ok_or_else(marker_invariant)?;
        }
        let mut remaining = counts;
        for cohort in &marker.pool_fuel_cohorts {
            if cohort.id.is_empty()
                || cohort.count <= 0
                || cohort.cp_quarters < 0
                || cohort.parent.as_ref() == Some(&cohort.id)
                || !physical_ids.insert(cohort.id.clone())
                || remaining.remove(&cohort.id) != Some((cohort.kind, cohort.count))
            {
                return Err(marker_invariant());
            }
        }
        if !remaining.is_empty() {
            return Err(marker_invariant());
        }
        let cargo = marker.cargo.totals().map_err(marker_supply)?;
        capacity::validate_packing(c, &trucks, &Trucks::default(), &cargo, &marker.cargo)
            .map_err(marker_supply)?;
        let total = trucks
            .light
            .checked_add(trucks.medium)
            .and_then(|n| n.checked_add(trucks.heavy))
            .ok_or_else(marker_invariant)?;
        if total == 0 {
            if cargo != cna_content::scenario::Supplies::default()
                || marker.tank_fuel.get() != 0
                || marker.activity_water.get() != 0
            {
                return Err(marker_invariant());
            }
            continue;
        }
        let capacity = [
            (TruckType::Light, trucks.light),
            (TruckType::Medium, trucks.medium),
            (TruckType::Heavy, trucks.heavy),
        ]
        .into_iter()
        .try_fold(0_i64, |sum, (kind, count)| {
            let per = c
                .tables
                .airlog
                .truck_characteristics
                .truck(kind)
                .fuel_capacity_points;
            if per < 0 {
                return Err(marker_invariant());
            }
            let value = i64::from(count)
                .checked_mul(i64::from(per))
                .and_then(|n| n.checked_mul(10))
                .ok_or_else(marker_invariant)?;
            sum.checked_add(value).ok_or_else(marker_invariant)
        })?;
        if i64::from(marker.tank_fuel.get()) > capacity {
            return Err(marker_invariant());
        }
        prepared
            .expected_serials
            .entry(marker.side)
            .or_insert_with(|| {
                s.land
                    .breakdown
                    .next_marker
                    .get(&marker.side)
                    .copied()
                    .unwrap_or(0)
            });
        let n = serials.entry(marker.side).or_default();
        *n = n.checked_add(1).ok_or_else(marker_invariant)?;
        let id = format!("broken-{}-{n}", marker.side);
        if s.land.breakdown.markers.contains_key(&id) {
            return Err(marker_invariant());
        }
        prepared.ids.push(id);
        prepared.markers.push(marker.clone());
    }
    Ok(prepared)
}

/// Recheck the entire allocation footprint before the first marker/event is written.
/// Caller applies this only on its complete unpublished loss draft.
/// Cases: land:21.42, land:21.43
pub fn apply_pool_markers(
    c: &crate::CnaContent,
    s: &mut State,
    prepared: PreparedPoolMarkers,
) -> Result<Vec<EngineEvent>, EngineError> {
    if prepared
        .expected_serials
        .iter()
        .any(|(side, n)| s.land.breakdown.next_marker.get(side).copied().unwrap_or(0) != *n)
        || prepared
            .ids
            .iter()
            .any(|id| s.land.breakdown.markers.contains_key(id))
    {
        return Err(marker_invariant());
    }
    let rechecked = prepare_pool_markers(c, s, &prepared.markers)?;
    if rechecked.ids != prepared.ids {
        return Err(marker_invariant());
    }
    let mut events = Vec::new();
    for marker in prepared.markers {
        events.extend(add(s, marker));
    }
    Ok(events)
}

#[cfg(test)]
#[path = "pool_marker_tests.rs"]
mod pool_marker_tests;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokenMarker {
    pub id: String,
    pub side: Side,
    pub hex: HexId,
    pub assets: Vec<Asset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_pool: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pool_assets: Vec<super::pools::PoolAsset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pool_fuel_cohorts: Vec<crate::logistics::TruckFuelCohort<String>>,
    /// Men remain physically with these vehicles, separate from the source unit's working body.
    pub passengers: BTreeMap<UnitId, i32>,
    pub transport: Trucks,
    pub cargo: crate::logistics::capacity::CargoPacking,
    pub tank_fuel: FuelTenths,
    pub activity_water: WaterPoints,
    #[serde(default)]
    pub fuel_cohorts: Vec<crate::logistics::TruckFuelCohort>,
    #[serde(default)]
    pub paid_truck_water: crate::logistics::TruckWater,
    #[serde(default)]
    pub water_credit_stage: Option<crate::logistics::water::WaterStage>,
}
impl BrokenMarker {
    pub fn trucks(&self) -> Trucks {
        let mut t = Trucks::default();
        for (equipment, points) in self
            .assets
            .iter()
            .map(|a| (&a.equipment, a.points))
            .chain(self.pool_assets.iter().map(|a| (&a.equipment, a.points)))
        {
            match equipment {
                Equipment::LightTruck => t.light += points,
                Equipment::MediumTruck => t.medium += points,
                Equipment::HeavyTruck => t.heavy += points,
                _ => {}
            }
        }
        t
    }
}
/// Record a newly numbered counter and emit an enemy copy only when presence appears.
/// The caller validates equipment/cargo conservation before creating this physical record.
/// Cases: land:21.42, land:21.43, land:21.44, land:3.62
/// Interpretations: interp:land-0028
pub fn add(s: &mut State, mut marker: BrokenMarker) -> Vec<EngineEvent> {
    let was = s.stack_presence(&marker.hex, marker.side);
    let n = s.land.breakdown.next_marker.entry(marker.side).or_default();
    *n = n.checked_add(1).expect("physical marker sequence fits u64");
    marker.id = format!("broken-{}-{n}", marker.side);
    let side = marker.side;
    let hex = marker.hex.clone();
    let mut events = vec![EngineEvent::new(
        Audience::Side(side),
        GameEvent::Note {
            text: format!(
                "Own broken vehicles recorded: {}",
                serde_json::to_string(&marker).expect("marker serializes")
            ),
        },
    )];
    // A newly visible own counter is delivered with its contents to live viewers as well.
    events.push(EngineEvent::new(
        Audience::Side(side),
        GameEvent::UnitUpdated {
            unit: unit_view(&marker),
        },
    ));
    s.land.breakdown.markers.insert(marker.id.clone(), marker);
    let mut ids: Vec<_> = s
        .units_of(side)
        .filter(|u| u.location.hex() == Some(&hex))
        .map(|u| u.id.to_string())
        .collect();
    ids.extend(
        s.land
            .breakdown
            .markers
            .values()
            .filter(|m| m.side == side && m.hex == hex)
            .map(|m| m.id.clone()),
    );
    ids.sort();
    events.push(EngineEvent::new(
        Audience::Side(side),
        GameEvent::StackUpdated {
            stack: Stack {
                hex: hex.to_string(),
                side,
                visible_count: Some(ids.len() as u32),
                unit_ids: ids,
            },
        },
    ));
    if !was {
        events.push(EngineEvent::new(
            Audience::SideOnly(side.opponent()),
            GameEvent::StackUpdated {
                stack: Stack {
                    hex: hex.to_string(),
                    side,
                    visible_count: None,
                    unit_ids: vec![],
                },
            },
        ));
    }
    events
}
/// The complete marker detail is available only to its side and the operator.
/// Cases: land:21.42, land:21.44, land:3.62
pub(crate) fn unit_view(marker: &BrokenMarker) -> cna_protocol::UnitView {
    cna_protocol::UnitView {
        id: marker.id.clone(),
        side: marker.side,
        name: format!("Broken vehicles {}", marker.id),
        kind: "broken_vehicle".into(),
        size: "marker".into(),
        nationality: String::new(),
        hex: Some(marker.hex.to_string()),
        parent: None,
        detail: Some(BTreeMap::from([
            (
                "broken_vehicles".into(),
                serde_json::to_value(marker).expect("marker serializes"),
            ),
            ("cpa".into(), serde_json::json!(0)),
        ])),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cna, CnaContent};
    use cna_core::{engine::Ruleset, visibility::Perspective};
    fn marker(hex: &str) -> BrokenMarker {
        BrokenMarker {
            id: String::new(),
            side: Side::Axis,
            hex: hex.into(),
            source_pool: None,
            pool_assets: vec![],
            pool_fuel_cohorts: vec![],
            assets: vec![Asset {
                unit: "source".into(),
                equipment: Equipment::MediumTruck,
                points: 3,
                cohort: None,
            }],
            passengers: BTreeMap::from([("source".into(), 1)]),
            transport: Trucks {
                light: 0,
                medium: 1,
                heavy: 0,
            },
            cargo: Default::default(),
            tank_fuel: FuelTenths::new(12),
            activity_water: WaterPoints::new(2),
            fuel_cohorts: vec![],
            paid_truck_water: Default::default(),
            water_credit_stage: None,
        }
    }
    /// Cases: land:21.42, land:3.62
    #[test]
    fn hidden_opponent_marker_count_cannot_change_own_counter_identity() {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut a = State::new(&c).unwrap();
        let mut b = a.clone();
        let mut enemy = marker("C4020");
        enemy.side = Side::Commonwealth;
        add(&mut b, enemy.clone());
        add(&mut b, enemy);
        let ea = add(&mut a, marker("C4021"));
        let eb = add(&mut b, marker("C4021"));
        let own = Perspective::Side(Side::Axis);
        let ea: Vec<_> = ea.iter().filter(|e| own.can_see(&e.audience)).collect();
        let eb: Vec<_> = eb.iter().filter(|e| own.can_see(&e.audience)).collect();
        assert_eq!(
            serde_json::to_value(ea).unwrap(),
            serde_json::to_value(eb).unwrap()
        );
    }
    /// Cases: land:21.42, land:21.43, land:3.62
    /// Interpretations: interp:land-0028
    #[test]
    fn marker_contents_and_counter_traffic_are_private_but_presence_is_public() {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut a = State::new(&c).unwrap();
        for u in a.land.units.values_mut() {
            u.location = crate::state::Location::Eliminated;
        }
        let events = add(&mut a, marker("C4020"));
        assert_eq!(
            events
                .iter()
                .filter(|e| Perspective::Side(Side::Commonwealth).can_see(&e.audience))
                .count(),
            1
        );
        let id = a.land.breakdown.markers.keys().next().unwrap().clone();
        // The marker is a counter on the map: the other side sees it, never its contents.
        let seen = Cna::dev().inspect(&c, &a, Perspective::Side(Side::Commonwealth), &id);
        crate::testkit::assert_face_only(&seen);
        assert_eq!(seen.unwrap()["unit"]["kind"], "broken_vehicle");
        assert_eq!(
            Cna::dev()
                .inspect(&c, &a, Perspective::Side(Side::Axis), &id)
                .unwrap()["broken_vehicles"]["passengers"]["source"],
            1
        );
        let mut b = a.clone();
        b.land.breakdown.markers.get_mut(&id).unwrap().assets[0].points = 1;
        b.land
            .breakdown
            .markers
            .get_mut(&id)
            .unwrap()
            .passengers
            .clear();
        crate::testkit::assert_indistinguishable(&Cna::dev(), &c, &a, &b, Side::Commonwealth);
        let events = add(&mut a, marker("C4020"));
        assert!(
            events
                .iter()
                .all(|e| !Perspective::Side(Side::Commonwealth).can_see(&e.audience))
        );
        let recovered: State = serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
        assert_eq!(recovered.land.breakdown.markers.len(), 2);
        assert_eq!(
            recovered.land.breakdown.markers[&id].passengers[&UnitId::new("source")],
            1
        );
    }
    /// Cases: land:3.62, land:21.42, land:21.43
    #[test]
    fn pool_marker_histories_are_additive_and_enemy_sees_presence_only() {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut a = State::new(&c).unwrap();
        let mut m = marker("C4020");
        m.assets.clear();
        m.passengers.clear();
        m.transport = Default::default();
        m.source_pool = Some("source-convoy".into());
        m.pool_assets = vec![super::super::pools::PoolAsset {
            pool: "source-convoy".into(),
            equipment: Equipment::MediumTruck,
            points: 3,
            cohort: "real-history".into(),
        }];
        assert_eq!(m.trucks().medium, 3);
        add(&mut a, m);
        let mut b = a.clone();
        let m = b.land.breakdown.markers.values_mut().next().unwrap();
        m.pool_assets[0].points = 8;
        m.cargo.medium.water += 1;
        crate::testkit::assert_indistinguishable(&Cna::dev(), &c, &a, &b, Side::Commonwealth);
        let restored: State = serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(&restored.land.breakdown.markers).unwrap(),
            serde_json::to_value(&a.land.breakdown.markers).unwrap()
        );
        let old = serde_json::to_value(marker("C4020")).unwrap();
        assert!(
            old.get("source_pool").is_none()
                && old.get("pool_assets").is_none()
                && old.get("pool_fuel_cohorts").is_none()
        );
    }
}
