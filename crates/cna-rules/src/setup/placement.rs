//! Geographic choice domains for setup and scheduled arrivals.
//!
//! This provider enumerates geography and fixed-position exclusions. The decision handler owns
//! occupancy validation, private buffering and closing the simultaneous window.

use std::collections::BTreeMap;

use cna_content::scenario::Placement;
use cna_core::engine::{Cx, EngineError};
use cna_core::event::EngineEvent;
use cna_core::ids::HexId;
use cna_core::visibility::Audience;
use cna_protocol::{GameEvent, Side};

use crate::content::CnaContent;
use crate::state::Location;

fn unsupported(case: &str, detail: String) -> EngineError {
    EngineError::Unsupported {
        case: case.into(),
        detail,
    }
}

/// A stable action option id for either a map hex or a semantic off-map location.
pub fn destination_id(location: &Location) -> Option<String> {
    match location {
        Location::Hex { hex } => Some(hex.to_string()),
        Location::OffMap { id } => Some(id.clone()),
        _ => None,
    }
}

fn at_hex(content: &CnaContent, hex: &HexId) -> Result<Location, EngineError> {
    let hex = content
        .map
        .canonical(hex)
        .ok_or_else(|| EngineError::Invariant {
            detail: format!("placement references unknown hex {hex}"),
        })?;
    Ok(Location::Hex { hex: hex.clone() })
}

/// Enumerate a placement's geographic domain without guessing unresolved memberships.
/// Dynamic facility selectors are resolved by the air/logistics procedure, not by an empty set.
/// Capacity is checked separately on the owner's provisional stack (land:9).
/// Cases: scen:59.2, scen:60.31, scen:60.41
pub fn choices(
    content: &CnaContent,
    placement: &Placement,
    side: Side,
    case: &str,
) -> Result<Vec<Location>, EngineError> {
    let mut out = match placement {
        Placement::Hex { hex } => vec![at_hex(content, hex)?],
        Placement::HexesAny { hexes } => hexes
            .iter()
            .map(|h| at_hex(content, h))
            .collect::<Result<_, _>>()?,
        Placement::Within { hex, n } => {
            let center = content.map.get(hex).ok_or_else(|| EngineError::Invariant {
                detail: format!("within placement references unknown hex {hex}"),
            })?;
            content
                .map
                .iter()
                .filter(|h| center.axial.distance(h.axial) <= *n)
                .map(|h| Location::Hex { hex: h.id.clone() })
                .collect()
        }
        Placement::Area { area, .. } | Placement::City { city: area } => {
            let record = content.areas.areas.get(area).ok_or_else(|| {
                unsupported(
                    case,
                    format!("placement area {area} has no verified membership record"),
                )
            })?;
            if record.membership_status != "resolved" {
                return Err(unsupported(
                    case,
                    format!(
                        "placement area {area} is {}: {}",
                        record.membership_status,
                        record
                            .reason
                            .as_deref()
                            .unwrap_or("membership requires more content or campaign state")
                    ),
                ));
            }
            let mut locations = record
                .hex_ids
                .iter()
                .map(|h| at_hex(content, h))
                .collect::<Result<Vec<_>, _>>()?;
            for id in &record.location_ids {
                let location =
                    content
                        .areas
                        .locations
                        .get(id)
                        .ok_or_else(|| EngineError::Invariant {
                            detail: format!("area {area} references missing location {id}"),
                        })?;
                if !location.off_map {
                    return Err(unsupported(
                        case,
                        format!("location {id} is not an off-map placement"),
                    ));
                }
                locations.push(Location::OffMap { id: id.clone() });
            }
            locations
        }
    };
    if let Placement::Area {
        exclusion: Some(exclusion),
        ..
    } = placement
        && let Some(n) = exclusion.enemy_unit_within_hexes
    {
        // The adopted setup interpretation permits only scenario-fixed opposing hexes here.
        // Free placements remain private and cannot change this geographic domain.
        let fixed_enemy = fixed_enemy_hexes(content, side)?;
        out.retain(|location| match location {
            Location::Hex { hex } => {
                let candidate = content.map.get(hex).expect("domain hex was canonicalized");
                fixed_enemy.iter().all(|enemy| {
                    candidate.axial.distance(
                        content
                            .map
                            .get(enemy)
                            .expect("fixed hex was canonicalized")
                            .axial,
                    ) > n
                })
            }
            Location::OffMap { .. } => true,
            _ => false,
        });
    }
    let out: BTreeMap<_, _> = out
        .into_iter()
        .map(|l| (destination_id(&l).expect("domain destination"), l))
        .collect();
    if out.is_empty() {
        return Err(unsupported(
            case,
            "verified geographic placement domain has no legal destinations".into(),
        ));
    }
    Ok(out.into_values().collect())
}

/// Cases: scen:60.31
fn fixed_enemy_hexes(content: &CnaContent, side: Side) -> Result<Vec<HexId>, EngineError> {
    let mut fixed = Vec::new();
    for file in &content.scenario.land {
        let owner = file.file.side.ok_or_else(|| EngineError::Invariant {
            detail: "land setup file has no owning side".into(),
        })?;
        if owner != side.opponent() {
            continue;
        }
        for group in &file.groups {
            if let Placement::Hex { hex } = &group.placement {
                let Location::Hex { hex } = at_hex(content, hex)? else {
                    unreachable!()
                };
                fixed.push(hex);
            }
        }
    }
    fixed.sort();
    fixed.dedup();
    Ok(fixed)
}

/// Under development, unavailable domains are reported to the owner and left awaiting placement.
/// The strict profile returns Unsupported with the governing scenario case.
/// Cases: scen:59.2, scen:60.31, scen:60.41
pub fn for_profile(
    content: &CnaContent,
    placement: &Placement,
    side: Side,
    case: &str,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<Option<Vec<Location>>, EngineError> {
    match choices(content, placement, side, case) {
        Ok(domain) => Ok(Some(domain)),
        Err(EngineError::Unsupported { case, detail }) if !strict => {
            cx.emit(EngineEvent::new(
                Audience::Side(side),
                GameEvent::Note {
                    text: format!("Awaiting placement ({case}): {detail}."),
                },
            ));
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cna_content::scenario::Exclusion;
    use cna_core::dice::CampaignRng;
    use cna_core::visibility::Perspective;
    use std::sync::OnceLock;

    fn content() -> &'static CnaContent {
        static CONTENT: OnceLock<CnaContent> = OnceLock::new();
        CONTENT.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
    }

    /// Cases: scen:59.2
    #[test]
    fn within_and_listed_hex_domains_are_exact_and_stable() {
        let c = content();
        let domain = choices(
            c,
            &Placement::Within {
                hex: "C4020".into(),
                n: 2,
            },
            Side::Axis,
            "scen:59.2",
        )
        .unwrap();
        assert_eq!(domain.len(), 19);
        let center = c.map.get(&"C4020".into()).unwrap().axial;
        for l in &domain {
            assert!(center.distance(c.map.get(l.hex().unwrap()).unwrap().axial) <= 2);
        }
        let domain = choices(
            c,
            &Placement::HexesAny {
                hexes: vec!["D3714".into(), "C4020".into(), "D3714".into()],
            },
            Side::Axis,
            "scen:59.2",
        )
        .unwrap();
        assert_eq!(
            domain.iter().map(destination_id).collect::<Vec<_>>(),
            vec![Some("C4020".into()), Some("D3714".into())]
        );
    }

    /// Cases: scen:60.31, scen:60.41, land:8.81
    #[test]
    fn boxes_use_semantic_identity_and_regions_do_not_become_empty_sets() {
        let domain = choices(
            content(),
            &Placement::City {
                city: "tripoli".into(),
            },
            Side::Axis,
            "land:8.81",
        )
        .unwrap();
        assert_eq!(
            domain,
            vec![Location::OffMap {
                id: "box_tripoli".into()
            }]
        );
        let cairo = choices(
            content(),
            &Placement::City {
                city: "cairo".into(),
            },
            Side::Commonwealth,
            "scen:60.41",
        )
        .unwrap();
        assert_eq!(
            cairo.iter().map(destination_id).collect::<Vec<_>>(),
            ["E1730", "E1829", "E1830", "E1930", "E1931"]
                .into_iter()
                .map(|h| Some(h.to_owned()))
                .collect::<Vec<_>>()
        );
        for area in ["libya", "egypt", "map_c_libya", "map_c_or_d_egypt"] {
            assert!(
                matches!(choices(content(), &Placement::Area { area: area.into(), exclusion: None }, Side::Axis, "scen:60.31"), Err(EngineError::Unsupported { case, .. }) if case == "scen:60.31")
            );
        }
    }

    /// Cases: scen:60.31
    #[test]
    fn exclusions_use_only_fixed_enemy_hexes() {
        let c = content();
        let all = choices(
            c,
            &Placement::Area {
                area: "map_d_or_e".into(),
                exclusion: None,
            },
            Side::Axis,
            "scen:60.31",
        )
        .unwrap();
        let domain = choices(
            c,
            &Placement::Area {
                area: "map_d_or_e".into(),
                exclusion: Some(Exclusion {
                    enemy_unit_within_hexes: Some(2),
                }),
            },
            Side::Axis,
            "scen:60.31",
        )
        .unwrap();
        assert!(domain.len() < all.len());
        let fixed = fixed_enemy_hexes(c, Side::Axis).unwrap();
        for candidate in &all {
            let axial = c.map.get(candidate.hex().unwrap()).unwrap().axial;
            let legal = fixed
                .iter()
                .all(|h| axial.distance(c.map.get(h).unwrap().axial) > 2);
            assert_eq!(domain.contains(candidate), legal);
        }
        assert!(domain.contains(&Location::Hex {
            hex: "E0101".into()
        }));
    }

    /// Cases: scen:60.31
    #[test]
    fn development_reports_unresolved_area_privately_without_changing_state() {
        let c = content();
        let state = crate::State::new(c).unwrap();
        let before = serde_json::to_string(&state).unwrap();
        let mut rng = CampaignRng::from_seed([1; 32]);
        let mut events = Vec::new();
        let mut cx = Cx {
            rng: &mut rng,
            events: &mut events,
        };
        let p = Placement::Area {
            area: "libya".into(),
            exclusion: None,
        };
        assert!(
            for_profile(c, &p, Side::Axis, "scen:60.31", false, &mut cx)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            for_profile(c, &p, Side::Axis, "scen:60.31", true, &mut cx),
            Err(EngineError::Unsupported { .. })
        ));
        assert_eq!(serde_json::to_string(&state).unwrap(), before);
        assert_eq!(events.len(), 1);
        assert!(!Perspective::Side(Side::Commonwealth).can_see(&events[0].audience));
    }
}
