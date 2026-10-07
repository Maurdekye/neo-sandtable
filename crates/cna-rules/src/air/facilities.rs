//! Canonical facility identities and source-derived facility mechanics.
//!
//! These helpers do not open windows, consume supplies, or imply permission to
//! construct. Engineering owns construction eligibility, duration and payment.

use cna_core::engine::EngineError;
use cna_protocol::Side;
use serde::{Deserialize, Serialize};

use crate::{CnaContent, state::Location};
use cna_content::scenario::Placement;
use cna_tables::airlog::air::{OffMapFacility, SquadronLimit};

/// The same stable identity used by setup, supply consumers and engineering.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FacilityId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FacilityKind {
    Airfield,
    LandingStrip,
    FlyingBoatBasin,
    FlyingBoatAlightingArea,
}

impl FacilityKind {
    /// The ordinary facility ceiling, before source-backed off-map exceptions.
    /// Cases: airlog:36.12, airlog:36.2, airlog:36.3, airlog:36.4
    pub fn standard_levels(self) -> i32 {
        match self {
            Self::Airfield => 6,
            Self::LandingStrip | Self::FlyingBoatAlightingArea => 1,
            Self::FlyingBoatBasin => 3,
        }
    }

    /// Cases: airlog:36.3, airlog:36.4
    pub fn accepts(self, flying_boat: bool) -> bool {
        matches!(self, Self::FlyingBoatBasin | Self::FlyingBoatAlightingArea) == flying_boat
    }

    /// The engineering upgrade is a full new project, with no time credit.
    /// Cases: land:24.79
    pub fn upgrade_to(self) -> Option<Self> {
        match self {
            Self::LandingStrip => Some(Self::Airfield),
            Self::FlyingBoatAlightingArea => Some(Self::FlyingBoatBasin),
            _ => None,
        }
    }
}

/// Unlimited is a source value, never inferred from an off-map location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "levels", rename_all = "snake_case")]
pub enum FacilityCapacity {
    Levels(i32),
    Unlimited,
}

/// Static scenario properties stay in content. Constructed facilities need
/// their own kind/location because no scenario record exists for them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum FacilityOrigin {
    Scenario,
    Constructed {
        kind: FacilityKind,
        location: Location,
    },
}

/// Source provenance determines the exceptions; an arbitrary off-map id does
/// not grant Mediterranean bombing immunity or Malta maintenance privileges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacilityTheatre {
    Africa,
    CommonwealthOffMap,
    Malta,
    AxisMediterranean,
}

/// Resolved from one source catalog, never stored as a second runtime catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacilityProperties {
    pub kind: FacilityKind,
    pub location: Location,
    pub printed_capacity: FacilityCapacity,
    pub theatre: FacilityTheatre,
}

/// Mutable facility truth. A completed upgrade changes the kind; the original
/// scenario identity/location remain intact. Zero capacity has source-specific
/// meaning, derived by `removed` rather than an independent destroyed flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacilityState {
    pub origin: FacilityOrigin,
    pub owner: Side,
    pub current_capacity: FacilityCapacity,
    pub upgraded_kind: Option<FacilityKind>,
    pub project_unavailable: bool,
}

fn invalid(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("air facility: {detail}"),
    }
}

impl FacilityState {
    /// Checks dynamic capacity against the resolved printed ceiling. It does
    /// not infer ownership access or source properties from a caller's flags.
    pub fn check(&self, properties: &FacilityProperties) -> Result<(), EngineError> {
        match (self.current_capacity, properties.printed_capacity) {
            (FacilityCapacity::Levels(n), FacilityCapacity::Levels(max)) if n >= 0 && n <= max => {
                Ok(())
            }
            (FacilityCapacity::Unlimited, FacilityCapacity::Unlimited) => Ok(()),
            _ => Err(invalid("capacity disagrees with source ceiling")),
        }
    }

    /// Normal strips and water facilities disappear when destroyed. Airfields
    /// remain rebuildable; Malta and genuine off-map facilities persist at zero.
    /// Cases: airlog:36.14, airlog:36.2, airlog:36.5, airlog:44.12, land:24.76
    pub fn removed(&self, properties: &FacilityProperties) -> bool {
        self.current_capacity == FacilityCapacity::Levels(0)
            && properties.theatre == FacilityTheatre::Africa
            && matches!(properties.location, Location::Hex { .. })
            && properties.kind != FacilityKind::Airfield
    }

    /// Construction and zero capacity deny operations; ownership alone does
    /// not, since facilities can be captured and used by either side.
    /// Cases: airlog:36.14, airlog:36.15, land:24.79
    pub fn operational(&self) -> bool {
        !self.project_unavailable && self.current_capacity != FacilityCapacity::Levels(0)
    }

    /// Apply an already-resolved bombing/barrage level loss. This helper owns
    /// no attack roll. Alighting areas ignore artillery; Med bases ignore air
    /// bombing. Unlimited capacities require a separately specified rule.
    /// Cases: airlog:36.14, airlog:36.4, airlog:36.5, airlog:44.12
    pub fn damage(
        &mut self,
        properties: &FacilityProperties,
        loss: i32,
        artillery: bool,
    ) -> Result<(), EngineError> {
        self.check(properties)?;
        if loss < 0 {
            return Err(invalid("negative damage"));
        }
        if (artillery && properties.kind == FacilityKind::FlyingBoatAlightingArea)
            || (!artillery && properties.theatre == FacilityTheatre::AxisMediterranean)
            || loss == 0
        {
            return Ok(());
        }
        let FacilityCapacity::Levels(levels) = self.current_capacity else {
            return Err(EngineError::Unsupported {
                case: "airlog:36.5".into(),
                detail: "No finite level-damage procedure for an unlimited-capacity facility"
                    .into(),
            });
        };
        self.current_capacity = FacilityCapacity::Levels(levels.saturating_sub(loss).max(0));
        Ok(())
    }

    /// Engineering has already paid and completed one repair project. This is
    /// only the capacity change; destroyed strips/basins need new construction.
    /// Cases: land:24.76, airlog:36.14
    pub fn repair_one(&mut self, properties: &FacilityProperties) -> Result<(), EngineError> {
        self.check(properties)?;
        if properties.kind != FacilityKind::Airfield || self.project_unavailable {
            return Err(invalid("facility cannot receive a one-level repair"));
        }
        let (FacilityCapacity::Levels(levels), FacilityCapacity::Levels(max)) =
            (self.current_capacity, properties.printed_capacity)
        else {
            return Err(invalid("one-level repair requires finite capacity"));
        };
        if levels == max {
            return Err(invalid("facility already at its ceiling"));
        }
        self.current_capacity = FacilityCapacity::Levels(levels + 1);
        Ok(())
    }

    /// Intrinsic AA is relevant only to strafing and dive-bombing missions.
    /// Cases: airlog:36.18, airlog:36.3
    pub fn intrinsic_aa(&self, strafing_or_dive_bombing: bool) -> i32 {
        i32::from(self.operational() && strafing_or_dive_bombing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(
        kind: FacilityKind,
        theatre: FacilityTheatre,
    ) -> (FacilityState, FacilityProperties) {
        let capacity = FacilityCapacity::Levels(kind.standard_levels());
        (
            FacilityState {
                origin: FacilityOrigin::Scenario,
                owner: Side::Axis,
                current_capacity: capacity,
                upgraded_kind: None,
                project_unavailable: false,
            },
            FacilityProperties {
                kind,
                location: Location::Hex {
                    hex: "A4829".into(),
                },
                printed_capacity: capacity,
                theatre,
            },
        )
    }

    /// Cases: airlog:36.12, airlog:36.2, airlog:36.3, airlog:36.4
    #[test]
    fn ordinary_capacity_and_aircraft_classes_are_distinct() {
        for (kind, levels, boat) in [
            (FacilityKind::Airfield, 6, false),
            (FacilityKind::LandingStrip, 1, false),
            (FacilityKind::FlyingBoatBasin, 3, true),
            (FacilityKind::FlyingBoatAlightingArea, 1, true),
        ] {
            assert_eq!(kind.standard_levels(), levels);
            assert!(kind.accepts(boat));
            assert!(!kind.accepts(!boat));
        }
    }

    /// Cases: airlog:36.14, airlog:36.2, airlog:36.5, airlog:44.12, land:24.76
    #[test]
    fn zero_capacity_removal_and_repair_follow_facility_source() {
        let (mut field, properties) = fixture(FacilityKind::Airfield, FacilityTheatre::Africa);
        field.damage(&properties, 20, false).unwrap();
        assert!(!field.operational());
        assert!(!field.removed(&properties));
        field.repair_one(&properties).unwrap();
        assert_eq!(field.current_capacity, FacilityCapacity::Levels(1));
        let (mut strip, properties) = fixture(FacilityKind::LandingStrip, FacilityTheatre::Africa);
        strip.damage(&properties, 1, false).unwrap();
        assert!(strip.removed(&properties));
        assert!(strip.repair_one(&properties).is_err());
        for theatre in [FacilityTheatre::Malta, FacilityTheatre::CommonwealthOffMap] {
            let (mut strip, properties) = fixture(FacilityKind::LandingStrip, theatre);
            strip.damage(&properties, 1, false).unwrap();
            assert!(!strip.removed(&properties));
        }
    }

    /// Cases: airlog:36.4, airlog:36.5, airlog:36.18, airlog:36.3
    #[test]
    fn immunity_and_intrinsic_aa_are_limited_to_their_attack_types() {
        let (mut basin, properties) = fixture(
            FacilityKind::FlyingBoatAlightingArea,
            FacilityTheatre::Africa,
        );
        basin.damage(&properties, 1, true).unwrap();
        assert!(basin.operational());
        assert_eq!(basin.intrinsic_aa(false), 0);
        assert_eq!(basin.intrinsic_aa(true), 1);
        basin.damage(&properties, 1, false).unwrap();
        assert!(!basin.operational());
        assert_eq!(basin.intrinsic_aa(true), 0);
        let (mut med, properties) =
            fixture(FacilityKind::Airfield, FacilityTheatre::AxisMediterranean);
        med.damage(&properties, 6, false).unwrap();
        assert!(med.operational());
    }

    /// Cases: airlog:36.14, land:24.76, land:24.79
    #[test]
    fn invalid_capacity_and_repairs_leave_state_unchanged() {
        let (mut field, properties) = fixture(FacilityKind::Airfield, FacilityTheatre::Africa);
        let before = field.clone();
        assert!(field.damage(&properties, -1, false).is_err());
        assert_eq!(field, before);
        assert!(field.repair_one(&properties).is_err());
        assert_eq!(field, before);
        field.current_capacity = FacilityCapacity::Levels(7);
        let before = field.clone();
        assert!(field.damage(&properties, 1, false).is_err());
        assert_eq!(field, before);
        field.current_capacity = FacilityCapacity::Levels(5);
        field.project_unavailable = true;
        assert!(!field.operational());
        let before = field.clone();
        assert!(field.repair_one(&properties).is_err());
        assert_eq!(field, before);
        assert_eq!(
            FacilityKind::LandingStrip.upgrade_to(),
            Some(FacilityKind::Airfield)
        );
        assert_eq!(
            FacilityKind::FlyingBoatAlightingArea.upgrade_to(),
            Some(FacilityKind::FlyingBoatBasin)
        );
        assert_eq!(FacilityKind::Airfield.upgrade_to(), None);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Facility {
    pub id: String,
    pub force: String,
    pub side: Side,
    pub location: Location,
    pub kind: String,
    pub limit: Option<i32>,
}
#[derive(Default)]
pub(crate) struct Catalog {
    pub facilities: Vec<Facility>,
    pub unresolved: Vec<String>,
}
fn unsupported(detail: impl Into<String>) -> EngineError {
    EngineError::Unsupported {
        case: "scen:59.35".into(),
        detail: detail.into(),
    }
}
/// Cases: scen:60.5, scen:60.46, airlog:36.12, airlog:36.2, airlog:36.3, airlog:36.4, airlog:36.5
pub(crate) fn catalog(content: &CnaContent) -> Result<Catalog, EngineError> {
    let mut out = Catalog::default();
    for f in &content.scenario.facilities.facilities {
        let side = match f.owner.as_deref() {
            Some("axis") => Side::Axis,
            Some("commonwealth") => Side::Commonwealth,
            _ => {
                out.unresolved.push(format!(
                    "{}: initial national ownership is not verified",
                    f.id
                ));
                continue;
            }
        };
        let force = f.theatre.clone().unwrap_or_else(|| {
            match side {
                Side::Axis => "axis",
                Side::Commonwealth => "commonwealth",
            }
            .into()
        });
        let mut locations = Vec::new();
        for hex in f.hex.iter().chain(&f.hexes) {
            let h = content
                .map
                .canonical(hex)
                .ok_or_else(|| unsupported(format!("{}: unknown facility hex", f.id)))?;
            locations.push(Location::Hex { hex: h.clone() });
        }
        if let Some(id) = &f.location {
            if content.areas.locations.get(id).is_some_and(|l| l.off_map) {
                locations.push(Location::OffMap { id: id.clone() });
            } else {
                out.unresolved
                    .push(format!("{}: off-map location is not verified", f.id));
                continue;
            }
        }
        if let Some(area) = &f.location_area {
            match crate::setup::placement::choices(
                content,
                &Placement::Area {
                    area: area.clone(),
                    exclusion: None,
                },
                side,
                "scen:59.35",
            ) {
                Ok(list) => locations.extend(list),
                Err(EngineError::Unsupported { detail, .. }) => {
                    out.unresolved.push(format!("{}: {detail}", f.id));
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        if locations.is_empty() {
            out.unresolved
                .push(format!("{}: facility has no verified location", f.id));
            continue;
        }
        let many = locations.len() > 1;
        for location in locations {
            let mut limit = Some(match f.kind.as_str() {
                "airfield" => 6,
                "air_landing_strip" | "landing_strip" => 1,
                "flying_boat_basin" => 3,
                "flying_boat_alighting_area" | "alighting_area" => 1,
                _ => {
                    return Err(unsupported(format!(
                        "{}: facility kind is unsupported",
                        f.id
                    )));
                }
            });
            if force == "malta" {
                limit = content
                    .scenario
                    .air
                    .iter()
                    .filter_map(|a| a.malta.as_ref())
                    .find_map(|m| m.facility_capacity_sgsu);
                if limit.is_none() {
                    return Err(unsupported("Malta aggregate has no initial capacity"));
                }
            } else if let Location::OffMap { id } = &location {
                let chart_id = match id.as_str() {
                    "offmap_port_said" => Some(OffMapFacility::PortSaid),
                    "offmap_abu_seier" => Some(OffMapFacility::AbuSeier),
                    "offmap_ismailia" => Some(OffMapFacility::Ismailia),
                    "offmap_fayid" => Some(OffMapFacility::Fayid),
                    "offmap_deversoir" => Some(OffMapFacility::Deversoir),
                    "offmap_kabrit" => Some(OffMapFacility::Kabrit),
                    _ => None,
                };
                if let Some(chart_id) = chart_id {
                    let row = content
                        .tables
                        .airlog
                        .offmap_air_facilities
                        .facility(chart_id);
                    if row.available_from.is_some_and(|s| {
                        (s.game_turn, s.opstage)
                            > (
                                i32::from(content.scenario.meta.start.gt),
                                i32::from(content.scenario.meta.start.opstage),
                            )
                    }) {
                        continue;
                    }
                    limit = match row.max_squadrons {
                        SquadronLimit::Squadrons(n) => Some(n),
                        SquadronLimit::Unlimited => None,
                    };
                }
            }
            let id = if many {
                format!(
                    "{}@{}",
                    f.id,
                    crate::setup::placement::destination_id(&location).expect("facility location")
                )
            } else {
                f.id.clone()
            };
            out.facilities.push(Facility {
                id,
                force: force.clone(),
                side,
                location,
                kind: f.kind.clone(),
                limit,
            });
        }
    }
    out.facilities.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Flying boats use their own water facilities; ordinary aircraft use land facilities.
/// Cases: airlog:36.3, airlog:36.4
pub(crate) fn compatible(facility: &Facility, flying_boat: bool) -> bool {
    let water = matches!(
        facility.kind.as_str(),
        "flying_boat_basin" | "flying_boat_alighting_area" | "alighting_area"
    );
    water == flying_boat || (facility.force == "malta" && !flying_boat)
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    /// Exact pre-move catalog captured from setup::facilities on the same content.
    /// Includes both Alexandria locations, verified off-map facilities, Malta
    /// aggregate, unresolved ownership, output order, limits and compatibility.
    /// Cases: scen:60.5, scen:60.46, airlog:36.12, airlog:36.2, airlog:36.3, airlog:36.4, airlog:36.5
    #[test]
    fn canonical_catalog_matches_pre_move_setup_snapshot() {
        let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let data = catalog(&content).unwrap();
        let actual = serde_json::json!({
            "facilities": data.facilities.iter().map(|f| serde_json::json!({
                "id": f.id, "force": f.force, "side": f.side,
                "location": f.location, "kind": f.kind, "limit": f.limit,
                "normal": compatible(f, false), "flying_boat": compatible(f, true),
            })).collect::<Vec<_>>(),
            "unresolved": data.unresolved,
        });
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/facility-catalog-graziani.json")).unwrap();
        assert_eq!(actual, expected);
    }
}
