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
        let areas = AreasContent::load(&dir.join("areas.toml"), &map).unwrap();
        assert_eq!(areas.areas["alexandria"].hex_ids.len(), 2);
        assert_eq!(areas.areas["tripoli"].location_ids, ["box_tripoli"]);
        assert_eq!(areas.areas["libya"].membership_status, "unresolved");
        assert!(areas.areas["libya"].requires_land);
        assert!(!areas.areas["alexandria"].requires_land);
        assert!(areas.areas["libya"].hex_ids.is_empty());
        assert!(areas.locations["offmap_abu_seier"].off_map);
    }
}
