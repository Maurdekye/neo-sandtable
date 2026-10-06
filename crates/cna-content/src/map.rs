//! The map grid: canonical hexes, their axial coordinates, terrain, and seam aliases.
//!
//! Schema: `data/map/README.md` (owned by the map digitization work).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cna_core::hex::Axial;
use cna_core::ids::HexId;
use serde::Deserialize;

use crate::ContentError;

/// One canonical hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexRecord {
    pub id: HexId,
    /// Map section letter, `A`..=`E`.
    pub section: char,
    pub axial: Axial,
    /// A Terrain Effects Chart class (`land:8.37`), or `None` while unclassified.
    pub terrain: Option<String>,
    /// Additional features (coast, port, village, …). Empty means unknown, not absent.
    pub flags: Vec<String>,
}

/// The whole map grid.
#[derive(Debug, Clone, Default)]
pub struct MapContent {
    hexes: BTreeMap<HexId, HexRecord>,
    by_axial: BTreeMap<Axial, HexId>,
    aliases: BTreeMap<HexId, HexId>,
}

#[derive(Debug, Deserialize)]
struct HexRow {
    hex_id: String,
    section: String,
    q: i32,
    r: i32,
    terrain: String,
    flags: String,
}

#[derive(Debug, Deserialize)]
struct AliasRow {
    alias_id: String,
    hex_id: String,
}

impl MapContent {
    /// Load `hexes.csv` and `aliases.csv` from a `data/map` folder.
    pub fn load(map_dir: &Path) -> Result<Self, ContentError> {
        let mut map = MapContent::default();

        let hexes_path = map_dir.join("hexes.csv");
        for row in read_csv::<HexRow>(&hexes_path)? {
            let invalid = |message: String| ContentError::Invalid {
                path: hexes_path.clone(),
                message,
            };
            let mut section_chars = row.section.chars();
            let section = match (section_chars.next(), section_chars.next()) {
                (Some(c @ 'A'..='E'), None) => c,
                _ => return Err(invalid(format!("bad section {:?}", row.section))),
            };
            let id = HexId::new(row.hex_id);
            let axial = Axial::new(row.q, row.r);
            if let Some(other) = map.by_axial.insert(axial, id.clone()) {
                return Err(invalid(format!("{id} and {other} share {axial:?}")));
            }
            let terrain = match row.terrain.as_str() {
                "" | "unclassified" => None,
                t => Some(t.to_owned()),
            };
            let flags = row
                .flags
                .split('|')
                .filter(|f| !f.is_empty())
                .map(str::to_owned)
                .collect();
            let record = HexRecord {
                id: id.clone(),
                section,
                axial,
                terrain,
                flags,
            };
            if map.hexes.insert(id.clone(), record).is_some() {
                return Err(invalid(format!("duplicate hex {id}")));
            }
        }

        let aliases_path = map_dir.join("aliases.csv");
        if aliases_path.exists() {
            for row in read_csv::<AliasRow>(&aliases_path)? {
                let alias = HexId::new(row.alias_id);
                let target = HexId::new(row.hex_id);
                if !map.hexes.contains_key(&target) || map.hexes.contains_key(&alias) {
                    return Err(ContentError::Invalid {
                        path: aliases_path.clone(),
                        message: format!("bad alias {alias} -> {target}"),
                    });
                }
                map.aliases.insert(alias, target);
            }
        }
        Ok(map)
    }

    pub fn len(&self) -> usize {
        self.hexes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hexes.is_empty()
    }

    /// The canonical id for a printed id, resolving seam aliases.
    pub fn canonical(&self, id: &HexId) -> Option<&HexId> {
        if let Some((k, _)) = self.hexes.get_key_value(id) {
            return Some(k);
        }
        self.aliases.get(id)
    }

    pub fn get(&self, id: &HexId) -> Option<&HexRecord> {
        self.canonical(id).and_then(|c| self.hexes.get(c))
    }

    pub fn at(&self, axial: Axial) -> Option<&HexRecord> {
        self.by_axial.get(&axial).and_then(|id| self.hexes.get(id))
    }

    /// Canonical hexes adjacent to `id` that exist on the map.
    pub fn neighbors(&self, id: &HexId) -> Vec<&HexRecord> {
        match self.get(id) {
            None => Vec::new(),
            Some(h) => h
                .axial
                .neighbors()
                .into_iter()
                .filter_map(|a| self.at(a))
                .collect(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &HexRecord> {
        self.hexes.values()
    }
}

fn read_csv<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>, ContentError> {
    let file = std::fs::File::open(path).map_err(|error| ContentError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let mut reader = csv::Reader::from_reader(file);
    reader
        .deserialize()
        .collect::<Result<Vec<T>, _>>()
        .map_err(|error| ContentError::Csv {
            path: PathBuf::from(path),
            error,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_map() -> MapContent {
        MapContent::load(&crate::repo_data_dir().join("map")).expect("data/map loads")
    }

    #[test]
    fn loads_the_published_grid() {
        let map = repo_map();
        assert_eq!(map.len(), 7023);
        // Anchors from data/map/README.md.
        let anchors = [
            ("C4807", 66, 15),
            ("C4321", 77, 20),
            ("C4218", 74, 21),
            ("D3714", 100, 26),
            ("E3613", 132, 27),
        ];
        for (id, q, r) in anchors {
            let hex = map.get(&HexId::new(id)).unwrap_or_else(|| panic!("{id}"));
            assert_eq!(hex.axial, Axial::new(q, r), "{id}");
            assert_eq!(map.at(Axial::new(q, r)).map(|h| h.id.as_str()), Some(id));
        }
    }

    #[test]
    fn aliases_resolve_to_canonical_hexes() {
        let map = repo_map();
        let alias = HexId::new("D0200");
        assert_eq!(
            map.canonical(&alias).map(HexId::as_str),
            Some("C0233"),
            "seam alias"
        );
        assert_eq!(map.get(&alias).map(|h| h.id.as_str()), Some("C0233"));
        assert!(map.get(&HexId::new("Z9999")).is_none());
    }

    #[test]
    fn interior_hexes_have_six_neighbors() {
        let map = repo_map();
        let bardia = HexId::new("C4321");
        let n = map.neighbors(&bardia);
        assert!(!n.is_empty() && n.len() <= 6);
        for h in n {
            assert_eq!(h.axial.distance(Axial::new(77, 20)), 1);
        }
    }
}
