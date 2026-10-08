//! Named placement domains and off-map identities supplied by the cartographer.
use std::collections::BTreeMap;
use std::path::Path;

use cna_core::ids::HexId;
use serde::Deserialize;

use crate::map::MapContent;
use crate::{ContentError, read_toml};

#[derive(Debug, Clone, Deserialize)]
pub struct Area {
    pub id: String,
    pub membership_status: String,
    #[serde(default)]
    pub requires_land: bool,
    #[serde(default)]
    pub hex_ids: Vec<HexId>,
    #[serde(default)]
    pub location_ids: Vec<String>,
    pub reason: Option<String>,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MapLocation {
    pub id: String,
    pub kind: String,
    pub off_map: bool,
    #[serde(default)]
    pub src: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AreasContent {
    pub areas: BTreeMap<String, Area>,
    pub locations: BTreeMap<String, MapLocation>,
}

#[derive(Deserialize)]
struct AreasFile {
    #[serde(default)]
    areas: Vec<Area>,
    #[serde(default)]
    locations: Vec<MapLocation>,
}

impl AreasContent {
    /// Load explicit membership only; unresolved and state-dependent selectors stay distinct.
    /// Cases: scen:59.2, scen:60.31, scen:60.41, land:8.81
    pub fn load(path: &Path, map: &MapContent) -> Result<Self, ContentError> {
        let file: AreasFile = read_toml(path)?;
        let invalid = |message| ContentError::Invalid {
            path: path.to_path_buf(),
            message,
        };
        let mut out = Self::default();
        for location in file.locations {
            if out
                .locations
                .insert(location.id.clone(), location)
                .is_some()
            {
                return Err(invalid("duplicate off-map location".into()));
            }
        }
        for mut area in file.areas {
            for hex in &mut area.hex_ids {
                *hex = map
                    .canonical(hex)
                    .ok_or_else(|| invalid(format!("area {}: unknown hex {hex}", area.id)))?
                    .clone();
            }
            area.hex_ids.sort();
            area.hex_ids.dedup();
            for id in &area.location_ids {
                if !out.locations.contains_key(id) {
                    return Err(invalid(format!("area {}: unknown location {id}", area.id)));
                }
            }
            if area.membership_status == "resolved"
                && area.hex_ids.is_empty()
                && area.location_ids.is_empty()
            {
                return Err(invalid(format!("resolved area {} is empty", area.id)));
            }
            if out.areas.insert(area.id.clone(), area).is_some() {
                return Err(invalid("duplicate area id".into()));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cases: scen:60.31, scen:60.34
    #[test]
    fn existing_land_requirement_is_decoded_and_omission_stays_false() {
        let record =
            "id='test'\nmembership_status='resolved'\nhex_ids=['C4020']\nsrc=['scen:60.31']\n";
        let omitted: Area = toml::from_str(record).unwrap();
        assert!(!omitted.requires_land);
        let flagged: Area = toml::from_str(&format!("{record}requires_land=true\n")).unwrap();
        assert!(flagged.requires_land);
        assert_eq!(flagged.hex_ids, omitted.hex_ids);
        assert_eq!(flagged.src, omitted.src);
    }

    /// Cases: scen:60.31, scen:60.41, land:8.81
    #[test]
    fn resolved_domains_and_unresolved_regions_remain_distinct() {
        let dir = crate::repo_data_dir().join("map");
        let map = MapContent::load(&dir).unwrap();
        let path = dir.join("areas.toml");
        let areas = AreasContent::load(&path, &map).unwrap();
        let generated: AreasFile = read_toml(&path).unwrap();
        assert_eq!(areas.areas["alexandria"].hex_ids.len(), 2);
        assert_eq!(areas.areas["tripoli"].location_ids, ["box_tripoli"]);
        for id in ["libya", "egypt", "map_c_libya", "map_c_or_d_egypt"] {
            let area = &areas.areas[id];
            let source = generated.areas.iter().find(|a| a.id == id).unwrap();
            assert_eq!(area.membership_status, "resolved");
            assert!(area.requires_land);
            assert!(!area.hex_ids.is_empty());
            assert_eq!(area.hex_ids, source.hex_ids);
            assert_eq!(area.src, source.src);
            assert!(area.location_ids.is_empty());
            assert!(area.hex_ids.iter().all(|h| map.canonical(h) == Some(h)));
        }
        // Check changing memberships from the grid and cited frontier, independently
        // of the generated area lists and the Python generator.
        use crate::map::Survey;
        use std::collections::{BTreeSet, VecDeque};

        #[derive(Deserialize)]
        struct Alias {
            alias_id: HexId,
            hex_id: HexId,
        }
        #[derive(Deserialize)]
        struct Frontier {
            trace_status: String,
            partition_policy: String,
            west_seeds: Vec<HexId>,
            east_seeds: Vec<HexId>,
            sides: Vec<FrontierSide>,
            src: Vec<String>,
        }
        #[derive(Deserialize)]
        struct FrontierSide {
            from_hex: HexId,
            to_hex: HexId,
            libya_side: HexId,
            egypt_side: HexId,
            src: Vec<String>,
            reason: String,
        }
        let aliases: Vec<Alias> = csv::Reader::from_path(dir.join("aliases.csv"))
            .unwrap()
            .deserialize()
            .map(Result::unwrap)
            .collect();
        let section = |letters: &str| -> BTreeSet<HexId> {
            map.iter()
                .filter(|h| letters.contains(h.section))
                .map(|h| h.id.clone())
                .chain(aliases.iter().filter_map(|a| {
                    assert_eq!(map.canonical(&a.alias_id), Some(&a.hex_id));
                    letters
                        .contains(a.alias_id.as_str().chars().next().unwrap())
                        .then(|| a.hex_id.clone())
                }))
                .collect()
        };
        let whole: BTreeSet<_> = map.iter().map(|h| h.id.clone()).collect();
        let sea: BTreeSet<_> = map
            .iter()
            .filter(|h| {
                h.flags == ["sea"]
                    && matches!(map.terrain_survey(&h.id), Survey::Present("sea"))
                    && matches!(map.coastal_survey(&h.id), Survey::Present(false))
            })
            .map(|h| h.id.clone())
            .collect();
        let members =
            |id: &str| -> BTreeSet<_> { areas.areas[id].hex_ids.iter().cloned().collect() };
        let libya = members("libya");
        let egypt = members("egypt");
        assert!(libya.is_disjoint(&egypt));
        assert!(libya.is_disjoint(&sea));
        assert!(egypt.is_disjoint(&sea));
        assert_eq!(&(&libya | &egypt) | &sea, whole);
        assert!(section("AB").difference(&sea).all(|h| libya.contains(h)));
        assert!(section("DE").difference(&sea).all(|h| egypt.contains(h)));
        assert_eq!(members("map_c_libya"), &section("C") & &libya);
        assert_eq!(members("map_c_or_d_egypt"), &section("CD") & &egypt);
        for (id, country) in [
            ("C4221", &libya),
            ("C4321", &libya),
            ("C4122", &egypt),
            ("C4022", &egypt),
        ] {
            let hex = map.canonical(&HexId::new(id)).unwrap().clone();
            if sea.contains(&hex) {
                assert!(!libya.contains(&hex) && !egypt.contains(&hex));
            } else {
                assert!(country.contains(&hex), "country sentinel {id}");
            }
        }
        let frontier: Frontier = read_toml(&dir.join("national-frontier.toml")).unwrap();
        assert_eq!(frontier.trace_status, "complete");
        assert_eq!(
            frontier.partition_policy,
            "exclude_verified_sea_include_unknown"
        );
        assert!(!frontier.src.is_empty());
        let edge = |a: &HexId, b: &HexId| {
            if a < b {
                (a.clone(), b.clone())
            } else {
                (b.clone(), a.clone())
            }
        };
        let mut walls = BTreeSet::new();
        for side in &frontier.sides {
            assert_eq!(map.canonical(&side.from_hex), Some(&side.from_hex));
            assert_eq!(map.canonical(&side.to_hex), Some(&side.to_hex));
            assert!(
                map.neighbors(&side.from_hex)
                    .iter()
                    .any(|h| h.id == side.to_hex)
            );
            assert!(!side.src.is_empty() && !side.reason.is_empty());
            assert!(walls.insert(edge(&side.from_hex, &side.to_hex)));
            assert!(libya.contains(&side.libya_side));
            assert!(egypt.contains(&side.egypt_side));
        }
        assert!(!walls.is_empty());
        let c_land = &section("C") - &sea;
        let fill = |seeds: &[HexId]| {
            assert!(!seeds.is_empty());
            let mut reached: BTreeSet<_> = seeds.iter().cloned().collect();
            assert!(reached.is_subset(&c_land));
            let mut queue: VecDeque<_> = reached.iter().cloned().collect();
            while let Some(hex) = queue.pop_front() {
                for neighbor in map.neighbors(&hex) {
                    if c_land.contains(&neighbor.id)
                        && !walls.contains(&edge(&hex, &neighbor.id))
                        && reached.insert(neighbor.id.clone())
                    {
                        queue.push_back(neighbor.id.clone());
                    }
                }
            }
            reached
        };
        let west = fill(&frontier.west_seeds);
        let east = fill(&frontier.east_seeds);
        assert!(west.is_disjoint(&east));
        assert_eq!(&west | &east, c_land);
        assert_eq!(&(&section("AB") | &west) - &sea, libya);
        assert_eq!(&(&section("DE") | &east) - &sea, egypt);
        assert!(!areas.areas["alexandria"].requires_land);
        let mut unresolved = areas.areas["libya"].clone();
        unresolved.id = "test_unresolved_libya".into();
        unresolved.membership_status = "unresolved".into();
        unresolved.hex_ids.clear();
        assert_eq!(unresolved.membership_status, "unresolved");
        assert!(unresolved.hex_ids.is_empty());
        assert!(areas.locations["offmap_abu_seier"].off_map);
    }
}
