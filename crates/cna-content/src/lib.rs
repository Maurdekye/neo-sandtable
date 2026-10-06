//! Loads game content from the repository's `data/` folder into typed, validated structures.
//!
//! Content is immutable for the life of a campaign and is pinned by hash (see
//! `docs/architecture.md` §3.1). This crate only reads and validates; it never invents a value
//! that the data does not contain.

pub mod map;
pub mod scenario;
pub mod units;

use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

/// A content file could not be read or did not validate.
#[derive(Debug)]
pub enum ContentError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    Csv {
        path: PathBuf,
        error: csv::Error,
    },
    Toml {
        path: PathBuf,
        error: toml::de::Error,
    },
    Invalid {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for ContentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContentError::Io { path, error } => write!(f, "{}: {error}", path.display()),
            ContentError::Csv { path, error } => write!(f, "{}: {error}", path.display()),
            ContentError::Toml { path, error } => write!(f, "{}: {error}", path.display()),
            ContentError::Invalid { path, message } => write!(f, "{}: {message}", path.display()),
        }
    }
}

impl std::error::Error for ContentError {}

/// The repository's `data/` folder, found from this crate's location. Tools and tests use it;
/// the server takes an explicit path instead.
pub fn repo_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

/// Parse one TOML file into `T`, naming the file in any error.
pub(crate) fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<T, ContentError> {
    let text = std::fs::read_to_string(path).map_err(|error| ContentError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    toml::from_str(&text).map_err(|error| ContentError::Toml {
        path: path.to_path_buf(),
        error,
    })
}

/// The `*.toml` files directly inside `dir`, sorted; none if `dir` does not exist.
pub(crate) fn toml_files(dir: &Path) -> Result<Vec<PathBuf>, ContentError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(dir).map_err(|error| ContentError::Io {
        path: dir.to_path_buf(),
        error,
    })?;
    let mut out = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| ContentError::Io {
                path: dir.to_path_buf(),
                error,
            })?
            .path();
        if path.extension().is_some_and(|e| e == "toml") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::ScenarioContent;
    use crate::units::UnitsContent;

    #[test]
    fn units_content_loads_and_resolves() {
        let units = UnitsContent::load(&repo_data_dir().join("units")).unwrap();
        assert!(units.weapons.len() >= 50, "{} weapons", units.weapons.len());
        assert!(units.classes.len() >= 90, "{} classes", units.classes.len());
        assert!(
            units.aircraft.len() >= 30,
            "{} aircraft",
            units.aircraft.len()
        );
        assert!(units.units.len() >= 400, "{} units", units.units.len());
        // A spot check against the 1st Libyan Infantry Division OA sheet (land:4.45).
        let hq = &units.units[&"it.1_libyan_div.1st_libyan_infantry_hq".into()];
        assert_eq!(hq.side, cna_protocol::Side::Axis);
        assert_eq!(hq.basic_morale, Some(-2));
        assert!(hq.engineer_hq && hq.arrives.is_deployed());
        let subtree = units.subtree(&hq.id);
        assert_eq!(
            subtree.len(),
            11,
            "HQ + 2 regiments x (HQ + 3 bns) + artillery + AT"
        );
    }

    #[test]
    fn graziani_scenario_loads_and_references_resolve() {
        let data = repo_data_dir();
        let units = UnitsContent::load(&data.join("units")).unwrap();
        let scenario = ScenarioContent::load(&data.join("scenarios/graziani")).unwrap();
        scenario.check(&units).unwrap();
        assert_eq!(scenario.meta.start.gt, 1);
        assert_eq!((scenario.meta.end.gt, scenario.meta.end.opstage), (6, 3));
        assert_eq!(scenario.land.len(), 2);
        assert_eq!(scenario.air.len(), 2);
        assert!(!scenario.supply.dumps.is_empty());
        assert!(!scenario.facilities.facilities.is_empty());
    }
}
