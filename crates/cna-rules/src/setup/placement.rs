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
    choices_with_profile(content, placement, side, case, true)
}

/// Flagged geographic areas require surveyed land before fixed-enemy exclusions.
/// Development omits unknown terrain; full rules refuse an incomplete domain.
/// Cases: scen:59.2, scen:60.31, scen:60.34, scen:60.41, land:8.37
pub fn choices_with_profile(
    content: &CnaContent,
    placement: &Placement,
    side: Side,
    case: &str,
    strict: bool,
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
            let mut locations = Vec::new();
            for member in &record.hex_ids {
                let location = at_hex(content, member)?;
                if record.requires_land {
                    let hex = location.hex().expect("canonical area member");
                    match content.map.terrain_survey(hex) {
                        cna_content::map::Survey::Present("sea") => continue,
                        cna_content::map::Survey::Present(_) => {}
                        _ if !strict => continue,
                        _ => {
                            return Err(unsupported(
                                case,
                                format!(
                                    "placement area {area} has an incomplete land domain: terrain at {hex} is unassessed ({}; land:8.37)",
                                    record.src.join(", ")
                                ),
                            ));
                        }
                    }
                }
                locations.push(location);
            }
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
    match choices_with_profile(content, placement, side, case, strict) {
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

    fn surveyed_fixture(members: &[&str], requires_land: bool) -> CnaContent {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cna-setup-land-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
        write(
            "hexes.csv",
            "hex_id,section,q,r,terrain,flags\nC0101,C,0,0,clear,land\nC0102,C,1,0,coast,land\nC0103,C,2,0,sea,\nC0104,C,3,0,unclassified,\nC0105,C,6,0,clear,land\nC0106,C,5,0,clear,land\nC0107,C,4,0,clear,land\n",
        );
        write("aliases.csv", "alias_id,hex_id\nD0100,C0102\n");
        write(
            "sections.toml",
            "coordinate_profile='test'\nbuild_file_sha256='fixture'\n",
        );
        write(
            "layers.toml",
            "schema_version=1\ncoordinate_profile='test'\nbuild_file_sha256='fixture'\nline_kinds=['road','unfinished_road','track','railroad','unfinished_railroad','pipeline']\nhexside_kinds=['escarpment','slope','ridge','wadi','major_river','minor_river','border','all_sea']\ncell_layers=['terrain','coastal']\nedge_coverage='per_feature_kind'\nunknown_policy='outside_mask_unknown'\n",
        );
        write(
            "coverage.csv",
            "layer,hex_id,neighbour_id,src,review_batch\nterrain,C0101,,land:8.37,test\nterrain,C0102,,land:8.37,test\nterrain,C0103,,land:8.37,test\nterrain,C0106,,land:8.37,test\nterrain,C0107,,land:8.37,test\n",
        );
        write(
            "line_features.csv",
            "from_hex,to_hex,kind,src,review_batch\n",
        );
        write(
            "hexsides.csv",
            "hex_id,direction,neighbour_id,feature,high_side,src,review_batch\n",
        );
        let map = cna_content::map::MapContent::load(&dir).unwrap();
        assert!(dir.starts_with(std::env::temp_dir()));
        assert!(
            dir.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("cna-setup-land-")
        );
        std::fs::remove_dir_all(&dir).unwrap();
        let mut c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        c.map = map;
        let mut area = c.areas.areas["libya"].clone();
        area.id = "test_land".into();
        area.membership_status = "resolved".into();
        area.requires_land = requires_land;
        area.hex_ids = members.iter().map(|h| HexId::new(*h)).collect();
        area.location_ids.clear();
        area.src = vec!["scen:60.34".into()];
        c.areas.areas.insert(area.id.clone(), area);
        // Synthetic fixed enemy positions isolate the exclusion distance from real geography.
        for file in &mut c.scenario.land {
            if file.file.side == Some(Side::Commonwealth) {
                for group in &mut file.groups {
                    group.placement = Placement::Hex {
                        hex: "C0101".into(),
                    };
                }
            }
        }
        c
    }

    fn test_area(exclude: bool) -> Placement {
        Placement::Area {
            area: "test_land".into(),
            exclusion: exclude.then_some(Exclusion {
                enemy_unit_within_hexes: Some(4),
            }),
        }
    }

    fn ids(domain: &[Location]) -> Vec<String> {
        domain.iter().map(|l| destination_id(l).unwrap()).collect()
    }

    /// Cases: scen:60.34, land:8.37
    #[test]
    fn flagged_domains_offer_land_and_canonical_coast_but_never_sea() {
        let mut c = surveyed_fixture(&["C0101", "D0100", "C0103"], true);
        c.areas.areas.get_mut("test_land").unwrap().location_ids = vec!["box_tripoli".into()];
        for strict in [false, true] {
            let domain =
                choices_with_profile(&c, &test_area(false), Side::Axis, "scen:60.34", strict)
                    .unwrap();
            assert_eq!(ids(&domain), ["C0101", "C0102", "box_tripoli"]);
            let offmap = choices_with_profile(
                &c,
                &Placement::City {
                    city: "tripoli".into(),
                },
                Side::Axis,
                "scen:60.34",
                strict,
            )
            .unwrap();
            assert_eq!(ids(&offmap), ["box_tripoli"]);
        }
    }

    /// Cases: scen:60.34, land:8.37
    #[test]
    fn unknown_land_is_omitted_in_dev_and_refuses_full_before_exclusion() {
        for unknown in ["C0104", "C0105"] {
            let c = surveyed_fixture(&["C0101", "C0106", unknown], true);
            assert_eq!(
                c.map.terrain_survey(&unknown.into()),
                cna_content::map::Survey::Unknown
            );
            for exclude in [false, true] {
                let expected = if exclude {
                    vec!["C0106"]
                } else {
                    vec!["C0101", "C0106"]
                };
                assert_eq!(
                    ids(&choices_with_profile(
                        &c,
                        &test_area(exclude),
                        Side::Axis,
                        "scen:60.34",
                        false
                    )
                    .unwrap()),
                    expected
                );
                let error =
                    choices_with_profile(&c, &test_area(exclude), Side::Axis, "scen:60.34", true)
                        .unwrap_err();
                assert!(
                    matches!(error, EngineError::Unsupported { case, detail } if case == "scen:60.34" && detail == format!("placement area test_land has an incomplete land domain: terrain at {unknown} is unassessed (scen:60.34; land:8.37)"))
                );
            }
        }
    }

    /// Cases: scen:60.34, land:8.37
    #[test]
    fn empty_land_domain_keeps_refusal_and_private_dev_note() {
        let c = surveyed_fixture(&["C0103"], true);
        for strict in [false, true] {
            assert!(
                matches!(choices_with_profile(&c, &test_area(false), Side::Axis, "scen:60.34", strict), Err(EngineError::Unsupported { case, detail }) if case == "scen:60.34" && detail == "verified geographic placement domain has no legal destinations")
            );
        }
        let mut rng = CampaignRng::from_seed([7; 32]);
        let before = rng.state();
        let mut events = Vec::new();
        let mut cx = Cx {
            rng: &mut rng,
            events: &mut events,
        };
        assert!(
            for_profile(
                &c,
                &test_area(false),
                Side::Axis,
                "scen:60.34",
                false,
                &mut cx
            )
            .unwrap()
            .is_none()
        );
        assert!(matches!(
            for_profile(
                &c,
                &test_area(false),
                Side::Axis,
                "scen:60.34",
                true,
                &mut cx
            ),
            Err(EngineError::Unsupported { .. })
        ));
        assert_eq!(rng.state(), before);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].audience, Audience::Side(Side::Axis));
        assert!(!Perspective::Side(Side::Commonwealth).can_see(&events[0].audience));
        let c = surveyed_fixture(&["C0104"], true);
        assert!(
            matches!(choices_with_profile(&c, &test_area(false), Side::Axis, "scen:60.34", false), Err(EngineError::Unsupported { detail, .. }) if detail == "verified geographic placement domain has no legal destinations")
        );
        assert!(
            matches!(choices_with_profile(&c, &test_area(false), Side::Axis, "scen:60.34", true), Err(EngineError::Unsupported { detail, .. }) if detail.contains("incomplete land domain"))
        );
    }

    /// Cases: scen:59.2, scen:60.34
    #[test]
    fn unflagged_and_within_spaces_keep_sea_and_unknown_members() {
        let c = surveyed_fixture(&["C0101", "C0102", "C0103", "C0104", "C0105"], false);
        for strict in [false, true] {
            assert_eq!(
                ids(
                    &choices_with_profile(&c, &test_area(false), Side::Axis, "scen:60.34", strict)
                        .unwrap()
                ),
                ["C0101", "C0102", "C0103", "C0104", "C0105"]
            );
            let within = Placement::Within {
                hex: "C0101".into(),
                n: 6,
            };
            assert_eq!(
                ids(&choices_with_profile(&c, &within, Side::Axis, "scen:59.2", strict).unwrap()),
                [
                    "C0101", "C0102", "C0103", "C0104", "C0105", "C0106", "C0107"
                ]
            );
        }
    }

    /// Cases: scen:60.34, land:8.37
    #[test]
    fn surveyed_dump_domain_keeps_strictly_more_than_four_hex_exclusion() {
        let mut c = surveyed_fixture(&["C0103", "C0107", "C0106"], true);
        for strict in [false, true] {
            assert_eq!(
                ids(
                    &choices_with_profile(&c, &test_area(true), Side::Axis, "scen:60.34", strict)
                        .unwrap()
                ),
                ["C0106"]
            );
        }
        c.areas.areas.get_mut("test_land").unwrap().hex_ids = vec!["C0107".into()];
        for strict in [false, true] {
            assert!(
                matches!(choices_with_profile(&c, &test_area(true), Side::Axis, "scen:60.34", strict), Err(EngineError::Unsupported { detail, .. }) if detail == "verified geographic placement domain has no legal destinations")
            );
        }
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
