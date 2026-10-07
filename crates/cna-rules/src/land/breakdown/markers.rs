//! Numbered broken vehicles preserve every physical holding; enemy views disclose presence only.
use super::{Asset, Equipment};
use crate::State;
use cna_content::units::Trucks;
use cna_core::{
    event::EngineEvent,
    ids::{HexId, UnitId},
    quantity::{FuelTenths, WaterPoints},
    visibility::Audience,
};
use cna_protocol::{GameEvent, Side, Stack};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
