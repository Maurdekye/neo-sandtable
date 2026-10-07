//! Source-backed initial air facility identities and readying capacities.
use crate::{CnaContent, state::Location};
use cna_content::scenario::Placement;
use cna_core::{
    engine::{Cx, EngineError},
    event::EngineEvent,
    ids::SeatId,
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role, Side};
use cna_tables::airlog::air::{OffMapFacility, SquadronLimit};

#[derive(Clone, Debug)]
pub(super) struct Facility {
    pub id: String,
    pub force: String,
    pub side: Side,
    pub location: Location,
    pub kind: String,
    pub limit: Option<i32>,
}
#[derive(Default)]
pub(super) struct Catalog {
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
pub(super) fn catalog(content: &CnaContent) -> Result<Catalog, EngineError> {
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
            match super::placement::choices(
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
                    super::placement::destination_id(&location).expect("facility location")
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
/// Unknown ownership/locations remain unavailable; only the owner receives development notes.
/// Cases: scen:59.35, scen:60.5
pub(super) fn report(
    content: &CnaContent,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let data = catalog(content)?;
    if strict && let Some(issue) = data.unresolved.first() {
        return Err(unsupported(issue));
    }
    if !data.unresolved.is_empty() {
        for side in [Side::Axis, Side::Commonwealth] {
            cx.emit(EngineEvent::new(Audience::Seat(SeatId::new(side,Role::Air)),GameEvent::Note {
                text:format!("Some initial air facilities are unavailable until their ownership or location is verified (scen:59.35): {}.",data.unresolved.join("; ")),
            }));
        }
    }
    Ok(())
}
/// Flying boats use their own water facilities; ordinary aircraft use land facilities.
/// Cases: airlog:36.3, airlog:36.4
pub(super) fn compatible(facility: &Facility, flying_boat: bool) -> bool {
    let water = matches!(
        facility.kind.as_str(),
        "flying_boat_basin" | "flying_boat_alighting_area" | "alighting_area"
    );
    water == flying_boat || (facility.force == "malta" && !flying_boat)
}
