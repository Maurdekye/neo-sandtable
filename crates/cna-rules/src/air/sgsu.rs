//! Actual squadron ground-service units; Malta roster groups are not SGSUs.

use std::collections::BTreeMap;

use cna_core::engine::EngineError;
use cna_protocol::Side;
use serde::{Deserialize, Serialize};

use super::facilities::{FacilityId, FacilityState};
use crate::{CnaContent, state::Location};

/// Exactly the stable AirSquadron.id, not a separately allocated identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SgsuId(pub String);

/// An SGSU can move away from its facility without duplicating site/location.
/// Cases: airlog:35.11, airlog:35.12
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "at", content = "value", rename_all = "snake_case")]
pub enum SgsuPosition {
    Facility(FacilityId),
    Ground(Location),
}

/// Receipts name the period actually paid, never a permanent supply entitlement.
/// Debit and publication belong to the atomic maintenance/logistics finish.
/// Cases: airlog:35.14
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SgsuOpStage {
    pub game_turn: u16,
    pub op_stage: u8,
}

/// Mutable canonical SGSU record; immutable ratings remain in source tables.
/// Cases: airlog:35.11, airlog:35.14
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SgsuState {
    pub force: String,
    pub nationality: String,
    pub position: SgsuPosition,
    pub stores_paid_game_turn: Option<u16>,
    pub fuel_water_paid: Option<SgsuOpStage>,
}

fn invalid(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("air SGSU: {detail}"),
    }
}

impl SgsuState {
    /// Resolve the consumer's physical position from the one canonical site.
    /// A Malta grouping or unverified ground location cannot be an SGSU.
    /// Facility ownership does not restrict RAW non-denominational use.
    /// Cases: airlog:35.11, airlog:35.17, airlog:36.15, airlog:44.14
    pub fn location(
        &self,
        content: &CnaContent,
        facilities: &BTreeMap<FacilityId, FacilityState>,
    ) -> Result<Location, EngineError> {
        self.side()?;
        match &self.position {
            SgsuPosition::Facility(id) => {
                let facility = facilities
                    .get(id)
                    .ok_or_else(|| invalid("unknown canonical facility"))?;
                let properties = facility.properties(content, id)?;
                if facility.removed(&properties) {
                    return Err(invalid("SGSU still references a removed facility"));
                }
                Ok(properties.location)
            }
            SgsuPosition::Ground(Location::Hex { hex })
                if content.map.canonical(hex) == Some(hex) =>
            {
                Ok(Location::Hex { hex: hex.clone() })
            }
            SgsuPosition::Ground(Location::OffMap { id })
                if content
                    .areas
                    .locations
                    .get(id)
                    .is_some_and(|site| site.off_map) =>
            {
                Ok(Location::OffMap { id: id.clone() })
            }
            _ => Err(invalid(
                "ground position is not a verified physical location",
            )),
        }
    }

    /// This is the force of the actual SGSU, independent of facility ownership.
    pub fn side(&self) -> Result<Side, EngineError> {
        match self.force.as_str() {
            "axis" => Ok(Side::Axis),
            "commonwealth" => Ok(Side::Commonwealth),
            _ => Err(invalid("force does not identify an actual SGSU")),
        }
    }

    /// No due payment grants aircraft supplies or facility throughput.
    /// Cases: airlog:35.14
    pub fn operation_paid(&self, period: SgsuOpStage) -> bool {
        period.game_turn > 0
            && (1..=3).contains(&period.op_stage)
            && self.stores_paid_game_turn == Some(period.game_turn)
            && self.fuel_water_paid == Some(period)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::air::facilities::{FacilityCapacity, FacilityKind, FacilityOrigin};

    fn site() -> FacilityState {
        FacilityState {
            origin: FacilityOrigin::Scenario,
            owner: Side::Commonwealth,
            current_capacity: FacilityCapacity::Levels(6),
            upgraded_kind: None,
            project_unavailable: false,
        }
    }
    fn sgsu(id: &FacilityId) -> SgsuState {
        SgsuState {
            force: "axis".into(),
            nationality: "it".into(),
            position: SgsuPosition::Facility(id.clone()),
            stores_paid_game_turn: None,
            fuel_water_paid: None,
        }
    }

    /// Cases: airlog:35.11, airlog:35.17, airlog:36.15, airlog:44.14
    #[test]
    fn consumer_uses_canonical_site_without_owner_or_malta_inference() {
        let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let id = FacilityId("airfield_benina".into());
        let sites = BTreeMap::from([(id.clone(), site())]);
        let mut consumer = sgsu(&id);
        assert_eq!(consumer.side().unwrap(), Side::Axis);
        assert_eq!(
            consumer.location(&content, &sites).unwrap(),
            sites[&id].properties(&content, &id).unwrap().location
        );
        // Source default owner is not the current owner or a use restriction.
        assert_eq!(sites[&id].owner, Side::Commonwealth);
        consumer.force = "malta".into();
        assert!(consumer.location(&content, &sites).is_err());
        consumer.force = "axis".into();
        consumer.position = SgsuPosition::Facility(FacilityId("unknown".into()));
        assert!(consumer.location(&content, &sites).is_err());
        let malta = FacilityId("malta.initial".into());
        let mut aggregate = site();
        aggregate.current_capacity = FacilityCapacity::Levels(5);
        assert!(matches!(
            aggregate.properties(&content, &malta),
            Err(EngineError::Unsupported { .. })
        ));
    }

    /// Cases: airlog:36.2, land:24.79
    #[test]
    fn upgrades_resolve_target_ceiling_and_reject_spoofed_source_or_location() {
        let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let id = FacilityId("air_landing_strip_augila".into());
        let mut state = site();
        state.current_capacity = FacilityCapacity::Levels(1);
        let original = state.properties(&content, &id).unwrap();
        assert_eq!(original.kind, FacilityKind::LandingStrip);
        state.upgraded_kind = Some(FacilityKind::Airfield);
        state.current_capacity = FacilityCapacity::Levels(6);
        let upgraded = state.properties(&content, &id).unwrap();
        assert_eq!(upgraded.location, original.location);
        assert_eq!(upgraded.kind, FacilityKind::Airfield);
        state.upgraded_kind = Some(FacilityKind::FlyingBoatBasin);
        assert!(state.properties(&content, &id).is_err());
        state.upgraded_kind = None;
        state.current_capacity = FacilityCapacity::Levels(1);
        state.origin = FacilityOrigin::Constructed {
            kind: FacilityKind::LandingStrip,
            location: original.location,
        };
        assert!(state.properties(&content, &id).is_err());
        assert!(
            state
                .properties(&content, &FacilityId("constructed.test".into()))
                .is_ok()
        );
        state.origin = FacilityOrigin::Constructed {
            kind: FacilityKind::LandingStrip,
            location: Location::OffMap {
                id: "offmap_port_said".into(),
            },
        };
        assert!(
            state
                .properties(&content, &FacilityId("constructed.test".into()))
                .is_err()
        );
    }

    /// Cases: airlog:35.14
    #[test]
    fn due_receipts_expire_by_period_and_survive_checkpoint() {
        let mut unit = sgsu(&FacilityId("airfield_benina".into()));
        let period = SgsuOpStage {
            game_turn: 1,
            op_stage: 1,
        };
        assert!(!unit.operation_paid(period));
        unit.stores_paid_game_turn = Some(1);
        assert!(!unit.operation_paid(period));
        unit.fuel_water_paid = Some(period);
        assert!(unit.operation_paid(period));
        assert!(!unit.operation_paid(SgsuOpStage {
            op_stage: 2,
            ..period
        }));
        assert!(!unit.operation_paid(SgsuOpStage {
            game_turn: 2,
            ..period
        }));
        let restored: SgsuState =
            serde_json::from_value(serde_json::to_value(&unit).unwrap()).unwrap();
        assert_eq!(restored, unit);
        assert!(restored.operation_paid(period));
        assert_eq!(
            serde_json::to_string(&SgsuId("axis.squadron-1".into())).unwrap(),
            "\"axis.squadron-1\""
        );
    }
}
