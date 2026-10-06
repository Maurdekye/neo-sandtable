//! Source-independent movement-layer validation. Coverage is per kind, never per rectangle.
use super::{MapContent, read_csv};
use crate::{ContentError, read_toml};
use cna_core::ids::HexId;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Distinguish a surveyed absence from missing content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Survey<T> {
    Unknown,
    Absent,
    Present(T),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Road,
    UnfinishedRoad,
    Track,
    Railroad,
    UnfinishedRailroad,
    Pipeline,
}
impl LineKind {
    pub const ALL: [Self; 6] = [
        Self::Road,
        Self::UnfinishedRoad,
        Self::Track,
        Self::Railroad,
        Self::UnfinishedRailroad,
        Self::Pipeline,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Road => "road",
            Self::UnfinishedRoad => "unfinished_road",
            Self::Track => "track",
            Self::Railroad => "railroad",
            Self::UnfinishedRailroad => "unfinished_railroad",
            Self::Pipeline => "pipeline",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideKind {
    Escarpment,
    Slope,
    Ridge,
    Wadi,
    MajorRiver,
    MinorRiver,
    Border,
    AllSea,
}
impl SideKind {
    pub const ALL: [Self; 8] = [
        Self::Escarpment,
        Self::Slope,
        Self::Ridge,
        Self::Wadi,
        Self::MajorRiver,
        Self::MinorRiver,
        Self::Border,
        Self::AllSea,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Escarpment => "escarpment",
            Self::Slope => "slope",
            Self::Ridge => "ridge",
            Self::Wadi => "wadi",
            Self::MajorRiver => "major_river",
            Self::MinorRiver => "minor_river",
            Self::Border => "border",
            Self::AllSea => "all_sea",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexsideFeature {
    pub kind: SideKind,
    pub high_side: Option<HexId>,
}
#[derive(Debug, Clone, Default)]
pub(super) struct Layers {
    coverage: BTreeSet<(String, HexId, Option<HexId>)>,
    lines: BTreeSet<(HexId, HexId, LineKind)>,
    sides: BTreeMap<(HexId, HexId, SideKind), HexsideFeature>,
}
#[derive(Deserialize)]
struct Metadata {
    schema_version: u32,
    coordinate_profile: String,
    build_file_sha256: String,
    line_kinds: Vec<LineKind>,
    hexside_kinds: Vec<SideKind>,
    cell_layers: Vec<String>,
    edge_coverage: String,
    unknown_policy: String,
}
#[derive(Deserialize)]
struct GridMetadata {
    coordinate_profile: String,
    build_file_sha256: String,
}
#[derive(Deserialize)]
struct CoverageRow {
    layer: String,
    hex_id: HexId,
    neighbour_id: String,
    src: String,
    review_batch: String,
}
#[derive(Deserialize)]
struct LineRow {
    from_hex: HexId,
    to_hex: HexId,
    kind: LineKind,
    src: String,
    review_batch: String,
}
#[derive(Deserialize)]
struct SideRow {
    hex_id: HexId,
    direction: String,
    neighbour_id: HexId,
    feature: SideKind,
    high_side: String,
    src: String,
    review_batch: String,
}

impl Layers {
    pub(super) fn load(dir: &Path, map: &MapContent) -> Result<Self, ContentError> {
        let path = dir.join("layers.toml");
        // Older content packages have no surveyed movement layers; absence never implies none.
        if !path.exists()
            && ["coverage.csv", "line_features.csv", "hexsides.csv"]
                .iter()
                .all(|n| !dir.join(n).exists())
        {
            return Ok(Self::default());
        }
        let meta: Metadata = read_toml(&path)?;
        let grid: GridMetadata = read_toml(&dir.join("sections.toml"))?;
        let invalid = |file: &str, message: String| ContentError::Invalid {
            path: dir.join(file),
            message,
        };
        if meta.schema_version != 1
            || meta.coordinate_profile != grid.coordinate_profile
            || meta.build_file_sha256 != grid.build_file_sha256
            || meta.line_kinds.len() != 6
            || meta.line_kinds.iter().copied().collect::<BTreeSet<_>>()
                != LineKind::ALL.into_iter().collect()
            || meta.hexside_kinds.len() != 8
            || meta.hexside_kinds.iter().copied().collect::<BTreeSet<_>>()
                != SideKind::ALL.into_iter().collect()
            || meta.cell_layers.len() != 2
            || meta
                .cell_layers
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != BTreeSet::from(["terrain", "coastal"])
            || meta.edge_coverage != "per_feature_kind"
            || meta.unknown_policy != "outside_mask_unknown"
        {
            return Err(invalid(
                "layers.toml",
                "schema, vocabulary, coverage policy or grid provenance differs".into(),
            ));
        }
        let valid_layers: BTreeSet<_> = LineKind::ALL
            .iter()
            .map(|k| format!("line:{}", k.name()))
            .chain(SideKind::ALL.iter().map(|k| format!("side:{}", k.name())))
            .collect();
        let published_edge = |a: &HexId, b: &HexId, file: &str| {
            if a >= b
                || map.canonical(a) != Some(a)
                || map.canonical(b) != Some(b)
                || map
                    .get(a)
                    .zip(map.get(b))
                    .is_none_or(|(a, b)| a.axial.distance(b.axial) != 1)
            {
                return Err(invalid(
                    file,
                    "edge endpoints must be sorted canonical adjacent hexes".into(),
                ));
            }
            Ok(())
        };
        let evidence = |src: &str, review: &str, file: &str| {
            if src.trim().is_empty() || review.trim().is_empty() {
                Err(invalid(file, "src and review_batch required".into()))
            } else {
                Ok(())
            }
        };
        let mut layers = Self::default();
        header(
            &dir.join("coverage.csv"),
            &["layer", "hex_id", "neighbour_id", "src", "review_batch"],
        )?;
        for row in read_csv::<CoverageRow>(&dir.join("coverage.csv"))? {
            evidence(&row.src, &row.review_batch, "coverage.csv")?;
            let b = if valid_layers.contains(&row.layer) {
                let b = HexId::new(row.neighbour_id);
                published_edge(&row.hex_id, &b, "coverage.csv")?;
                Some(b)
            } else if ["terrain", "coastal"].contains(&row.layer.as_str()) {
                let h = map
                    .get(&row.hex_id)
                    .ok_or_else(|| invalid("coverage.csv", "unknown coverage hex".into()))?;
                if map.canonical(&row.hex_id) != Some(&row.hex_id)
                    || !row.neighbour_id.is_empty()
                    || (row.layer == "terrain" && h.terrain.is_none())
                    || (row.layer == "coastal"
                        && !h
                            .flags
                            .iter()
                            .any(|f| matches!(f.as_str(), "land" | "sea" | "coastal")))
                {
                    return Err(invalid(
                        "coverage.csv",
                        "cell coverage requires a canonical observed domain".into(),
                    ));
                }
                None
            } else {
                return Err(invalid(
                    "coverage.csv",
                    format!("unknown layer {}", row.layer),
                ));
            };
            if !layers.coverage.insert((row.layer, row.hex_id, b)) {
                return Err(invalid("coverage.csv", "duplicate coverage key".into()));
            }
        }
        header(
            &dir.join("line_features.csv"),
            &["from_hex", "to_hex", "kind", "src", "review_batch"],
        )?;
        for row in read_csv::<LineRow>(&dir.join("line_features.csv"))? {
            evidence(&row.src, &row.review_batch, "line_features.csv")?;
            published_edge(&row.from_hex, &row.to_hex, "line_features.csv")?;
            if !layers.covered(
                format!("line:{}", row.kind.name()),
                &row.from_hex,
                Some(&row.to_hex),
            ) || !layers.lines.insert((row.from_hex, row.to_hex, row.kind))
            {
                return Err(invalid(
                    "line_features.csv",
                    "feature requires matching coverage and a unique key".into(),
                ));
            }
        }
        header(
            &dir.join("hexsides.csv"),
            &[
                "hex_id",
                "direction",
                "neighbour_id",
                "feature",
                "high_side",
                "src",
                "review_batch",
            ],
        )?;
        for row in read_csv::<SideRow>(&dir.join("hexsides.csv"))? {
            evidence(&row.src, &row.review_batch, "hexsides.csv")?;
            published_edge(&row.hex_id, &row.neighbour_id, "hexsides.csv")?;
            let direction = match row.direction.as_str() {
                "E" => cna_core::hex::Direction::East,
                "NE" => cna_core::hex::Direction::NorthEast,
                "NW" => cna_core::hex::Direction::NorthWest,
                "W" => cna_core::hex::Direction::West,
                "SW" => cna_core::hex::Direction::SouthWest,
                "SE" => cna_core::hex::Direction::SouthEast,
                _ => return Err(invalid("hexsides.csv", "unknown direction".into())),
            };
            if map.get(&row.hex_id).unwrap().axial.neighbor(direction)
                != map.get(&row.neighbour_id).unwrap().axial
            {
                return Err(invalid(
                    "hexsides.csv",
                    "direction disagrees with endpoints".into(),
                ));
            }
            let high = if matches!(row.feature, SideKind::Slope | SideKind::Escarpment) {
                let h = HexId::new(row.high_side);
                if h != row.hex_id && h != row.neighbour_id {
                    return Err(invalid(
                        "hexsides.csv",
                        "high_side must be an endpoint".into(),
                    ));
                }
                Some(h)
            } else {
                if !row.high_side.is_empty() {
                    return Err(invalid(
                        "hexsides.csv",
                        "high_side only applies to slopes and escarpments".into(),
                    ));
                }
                None
            };
            if !layers.covered(
                format!("side:{}", row.feature.name()),
                &row.hex_id,
                Some(&row.neighbour_id),
            ) || layers
                .sides
                .insert(
                    (row.hex_id, row.neighbour_id, row.feature),
                    HexsideFeature {
                        kind: row.feature,
                        high_side: high,
                    },
                )
                .is_some()
            {
                return Err(invalid(
                    "hexsides.csv",
                    "feature requires matching coverage and a unique key".into(),
                ));
            }
        }
        Ok(layers)
    }
    fn covered(&self, layer: String, a: &HexId, b: Option<&HexId>) -> bool {
        self.coverage.contains(&(layer, a.clone(), b.cloned()))
    }
}
fn header(path: &Path, fields: &[&str]) -> Result<(), ContentError> {
    let mut reader = csv::Reader::from_path(path).map_err(|error| ContentError::Csv {
        path: path.into(),
        error,
    })?;
    let h = reader.headers().map_err(|error| ContentError::Csv {
        path: path.into(),
        error,
    })?;
    if h.iter().ne(fields.iter().copied()) {
        return Err(ContentError::Invalid {
            path: path.into(),
            message: "unexpected layer CSV header".into(),
        });
    }
    Ok(())
}
impl MapContent {
    fn layer_edge(&self, a: &HexId, b: &HexId) -> Option<(HexId, HexId)> {
        let (a, b) = (self.canonical(a)?, self.canonical(b)?);
        if self.get(a)?.axial.distance(self.get(b)?.axial) != 1 {
            return None;
        }
        Some(if a < b {
            (a.clone(), b.clone())
        } else {
            (b.clone(), a.clone())
        })
    }
    pub fn terrain_survey(&self, id: &HexId) -> Survey<&str> {
        let Some(id) = self.canonical(id) else {
            return Survey::Unknown;
        };
        if !self.layers.covered("terrain".into(), id, None) {
            return Survey::Unknown;
        }
        self.get(id)
            .and_then(|h| h.terrain.as_deref())
            .map_or(Survey::Unknown, Survey::Present)
    }
    pub fn coastal_survey(&self, id: &HexId) -> Survey<bool> {
        let Some(id) = self.canonical(id) else {
            return Survey::Unknown;
        };
        if !self.layers.covered("coastal".into(), id, None) {
            return Survey::Unknown;
        }
        Survey::Present(self.get(id).unwrap().flags.iter().any(|f| f == "coastal"))
    }
    pub fn line(&self, a: &HexId, b: &HexId, kind: LineKind) -> Survey<()> {
        let Some((a, b)) = self.layer_edge(a, b) else {
            return Survey::Unknown;
        };
        if !self
            .layers
            .covered(format!("line:{}", kind.name()), &a, Some(&b))
        {
            Survey::Unknown
        } else if self.layers.lines.contains(&(a, b, kind)) {
            Survey::Present(())
        } else {
            Survey::Absent
        }
    }
    pub fn hexside(&self, a: &HexId, b: &HexId, kind: SideKind) -> Survey<&HexsideFeature> {
        let Some((a, b)) = self.layer_edge(a, b) else {
            return Survey::Unknown;
        };
        if !self
            .layers
            .covered(format!("side:{}", kind.name()), &a, Some(&b))
        {
            Survey::Unknown
        } else {
            self.layers
                .sides
                .get(&(a, b, kind))
                .map_or(Survey::Absent, Survey::Present)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    struct Fixture {
        dir: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "cna-map-layers-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).unwrap();
            let f = Self { dir };
            f.write("hexes.csv","hex_id,section,q,r,terrain,flags\nC4020,C,0,0,clear,land\nC4021,C,1,0,clear,land\nC4022,C,2,0,clear,land\n");
            f.write("aliases.csv", "alias_id,hex_id\nD0200,C4020\n");
            f.write(
                "sections.toml",
                "coordinate_profile='test'\nbuild_file_sha256='fixture'\n",
            );
            f.write("layers.toml","schema_version=1\ncoordinate_profile='test'\nbuild_file_sha256='fixture'\nline_kinds=['road','unfinished_road','track','railroad','unfinished_railroad','pipeline']\nhexside_kinds=['escarpment','slope','ridge','wadi','major_river','minor_river','border','all_sea']\ncell_layers=['terrain','coastal']\nedge_coverage='per_feature_kind'\nunknown_policy='outside_mask_unknown'\n");
            f.write("coverage.csv","layer,hex_id,neighbour_id,src,review_batch\nterrain,C4020,,land:8.37,test\nline:road,C4020,C4021,land:8.33,test\nside:slope,C4020,C4021,land:8.35,test\n");
            f.write(
                "line_features.csv",
                "from_hex,to_hex,kind,src,review_batch\n",
            );
            f.write("hexsides.csv","hex_id,direction,neighbour_id,feature,high_side,src,review_batch\nC4020,E,C4021,slope,C4021,land:8.35,test\n");
            f
        }
        fn write(&self, name: &str, text: &str) {
            std::fs::write(self.dir.join(name), text).unwrap();
        }
        fn edit(&self, name: &str, from: &str, to: &str) {
            let p = self.dir.join(name);
            let s = std::fs::read_to_string(&p).unwrap();
            assert!(s.contains(from));
            std::fs::write(p, s.replacen(from, to, 1)).unwrap();
        }
        fn load(&self) -> Result<MapContent, ContentError> {
            MapContent::load(&self.dir)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert!(self.dir.starts_with(std::env::temp_dir()));
            assert!(
                self.dir
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("cna-map-layers-")
            );
            std::fs::remove_dir_all(&self.dir).unwrap();
        }
    }
    #[test]
    fn real_surface_masks_do_not_certify_any_edge_absence() {
        let map = MapContent::load(&crate::repo_data_dir().join("map")).unwrap();
        let a = HexId::new("C4020");
        let b = map.neighbors(&a)[0].id.clone();
        assert!(matches!(map.terrain_survey(&a), Survey::Present(_)));
        for k in LineKind::ALL {
            assert_eq!(map.line(&a, &b, k), Survey::Unknown);
        }
        for k in SideKind::ALL {
            assert_eq!(map.hexside(&a, &b, k), Survey::Unknown);
        }
        assert_eq!(map.terrain_survey(&HexId::new("C4026")), Survey::Unknown);
    }
    #[test]
    fn coverage_is_per_kind_and_queries_resolve_aliases_and_reverse_edges() {
        let f = Fixture::new();
        let m = f.load().unwrap();
        let a = "C4020".into();
        let b = "C4021".into();
        assert_eq!(m.line(&a, &b, LineKind::Road), Survey::Absent);
        assert_eq!(m.line(&a, &b, LineKind::Track), Survey::Unknown);
        assert_eq!(m.line(&"D0200".into(), &b, LineKind::Road), Survey::Absent);
        assert_eq!(m.terrain_survey(&b), Survey::Unknown);
        assert_eq!(
            m.hexside(&b, &a, SideKind::Slope),
            Survey::Present(&HexsideFeature {
                kind: SideKind::Slope,
                high_side: Some(b.clone())
            })
        );
        f.write(
            "line_features.csv",
            "from_hex,to_hex,kind,src,review_batch\nC4020,C4021,road,land:8.33,test\n",
        );
        assert_eq!(
            f.load().unwrap().line(&b, &a, LineKind::Road),
            Survey::Present(())
        );
    }
    #[test]
    fn unknown_or_duplicate_coverage_and_unsurveyed_features_are_rejected() {
        let f = Fixture::new();
        f.edit("coverage.csv", "line:road", "line:made_up");
        assert!(f.load().unwrap_err().to_string().contains("coverage.csv"));
        let f = Fixture::new();
        f.edit(
            "coverage.csv",
            "line:road,C4020,C4021,land:8.33,test",
            "line:road,C4020,C4021,land:8.33,test\nline:road,C4020,C4021,land:8.33,test",
        );
        assert!(f.load().unwrap_err().to_string().contains("duplicate"));
        let f = Fixture::new();
        f.write(
            "line_features.csv",
            "from_hex,to_hex,kind,src,review_batch\nC4020,C4021,track,land:8.33,test\n",
        );
        assert!(f.load().unwrap_err().to_string().contains("coverage"));
    }
    #[test]
    fn high_side_direction_canonical_endpoints_and_provenance_are_checked() {
        for (file, from, to, expected) in [
            ("hexsides.csv", "slope,C4021", "slope,C4022", "high_side"),
            ("hexsides.csv", ",E,", ",W,", "direction"),
            (
                "coverage.csv",
                "line:road,C4020",
                "line:road,D0200",
                "canonical",
            ),
            ("layers.toml", "'fixture'", "'wrong'", "provenance"),
        ] {
            let f = Fixture::new();
            f.edit(file, from, to);
            assert!(f.load().unwrap_err().to_string().contains(expected));
        }
    }
}
