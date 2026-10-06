//! Explicit, cited map places. An omitted record means an unverified place layer.
use crate::{ContentError, map::MapContent, read_toml};
use cna_core::ids::HexId;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Place {
    pub id: String,
    pub name: String,
    pub hex_id: HexId,
    #[serde(rename = "type")]
    pub kind: String,
    pub place_group: Option<String>,
    pub src: Vec<String>,
    pub note: Option<String>,
    pub review_batch: String,
}
#[derive(Debug, Clone, Default)]
pub struct PlacesContent {
    pub complete: bool,
    pub coordinate_profile: String,
    pub verification: String,
    pub places: BTreeMap<String, Place>,
}
#[derive(Deserialize)]
struct PlacesFile {
    schema_version: u32,
    coordinate_profile: String,
    complete: bool,
    verification: String,
    #[serde(default)]
    places: Vec<Place>,
}
impl PlacesContent {
    /// Load explicit records and canonicalize their anchors. Kind strings are
    /// retained; procedures offer only kinds whose rules they implement. Absence
    /// cannot establish that a hex has no port, village, bir or oasis.
    /// Cases: airlog:52.11, airlog:55.0, land:4.1
    pub fn load(path: &Path, map: &MapContent) -> Result<Self, ContentError> {
        Self::bind(read_toml(path)?, path, map)
    }
    fn bind(file: PlacesFile, path: &Path, map: &MapContent) -> Result<Self, ContentError> {
        let invalid = |message: &str| ContentError::Invalid {
            path: path.to_path_buf(),
            message: message.into(),
        };
        if file.schema_version != 1
            || file.coordinate_profile.is_empty()
            || file.verification.is_empty()
        {
            return Err(invalid("unsupported places metadata"));
        }
        let mut content = Self {
            complete: file.complete,
            coordinate_profile: file.coordinate_profile,
            verification: file.verification,
            places: BTreeMap::new(),
        };
        for mut place in file.places {
            if place.id.is_empty()
                || place.name.is_empty()
                || place.kind.is_empty()
                || place.review_batch.is_empty()
                || place.src.is_empty()
                || place.src.iter().any(String::is_empty)
            {
                return Err(invalid(
                    "place identity, kind, review and citations are required",
                ));
            }
            place.hex_id = map
                .canonical(&place.hex_id)
                .ok_or_else(|| invalid("place anchor is not a map hex"))?
                .clone();
            if content.places.insert(place.id.clone(), place).is_some() {
                return Err(invalid("duplicate place id"));
            }
        }
        Ok(content)
    }
    /// Verified records anchored at this canonical hex; an empty iterator is unknown.
    pub fn at<'a>(&'a self, hex: &'a HexId) -> impl Iterator<Item = &'a Place> {
        self.places
            .values()
            .filter(move |place| &place.hex_id == hex)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn map() -> MapContent {
        MapContent::load(&crate::repo_data_dir().join("map")).unwrap()
    }
    /// Cases: airlog:52.11, airlog:55.0
    #[test]
    fn loads_verified_city_and_port_anchors_without_inferring_other_places() {
        let map = map();
        let places =
            PlacesContent::load(&crate::repo_data_dir().join("map/places.toml"), &map).unwrap();
        assert!(!places.complete);
        let sollum = &places.places["port-sollum"];
        assert_eq!(sollum.hex_id.as_str(), "C4022");
        assert_eq!(sollum.kind, "port");
        assert!(!places.at(&HexId::new("C4021")).any(|p| p.kind == "port"));
        for hex in ["E1730", "E1829", "E1830", "E1930", "E1931"] {
            assert!(
                places
                    .at(&HexId::new(hex))
                    .any(|p| p.kind == "major_city" && p.place_group.as_deref() == Some("cairo"))
            );
        }
    }
    /// Cases: airlog:52.11, airlog:55.0
    #[test]
    fn rejects_duplicate_unmapped_and_uncited_places() {
        let map = map();
        let path = crate::repo_data_dir().join("map/places.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        let mut file: PlacesFile = toml::from_str(&text).unwrap();
        file.places.push(file.places[0].clone());
        assert!(PlacesContent::bind(file, &path, &map).is_err());
        let mut file: PlacesFile = toml::from_str(&text).unwrap();
        file.places[0].hex_id = "NO_HEX".into();
        assert!(PlacesContent::bind(file, &path, &map).is_err());
        let mut file: PlacesFile = toml::from_str(&text).unwrap();
        file.places[0].src.clear();
        assert!(PlacesContent::bind(file, &path, &map).is_err());
    }
    /// Cases: airlog:52.11, airlog:55.0
    #[test]
    fn tracked_loader_read_is_pinned() {
        let map = map();
        let path = crate::repo_data_dir().join("map/places.toml");
        let (places, reads) = crate::record_reads(|| PlacesContent::load(&path, &map).unwrap());
        assert!(!places.places.is_empty());
        assert_eq!(reads, vec![crate::normalize(&path)]);
    }
}
