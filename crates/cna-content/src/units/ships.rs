//! Printed counter characteristics; cargo and CP expenditure belong to campaign state.
use crate::{ContentError, read_toml};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoastalShip {
    pub id: String,
    pub designation: String,
    /// Missing means unreadable or unresolved, never an empty ship or zero capacity.
    pub capacity_tons: Option<i32>,
    pub src: Vec<String>,
    pub transcribed_from: Vec<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    src: Vec<String>,
    transcribed_from: Vec<String>,
    verification: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShipFile {
    file: Provenance,
    ships: Vec<CoastalShip>,
}
#[derive(Debug, Clone)]
pub struct ShipRoster {
    pub ships: BTreeMap<String, CoastalShip>,
}
impl ShipRoster {
    /// Read and validate only printed ship identity and capacity data.
    /// Cases: airlog:56.31
    pub fn load(path: &Path) -> Result<Self, ContentError> {
        Self::bind(read_toml(path)?, path)
    }
    fn bind(file: ShipFile, path: &Path) -> Result<Self, ContentError> {
        let invalid = |message: &str| ContentError::Invalid {
            path: path.to_path_buf(),
            message: message.into(),
        };
        if file.file.src.is_empty()
            || file.file.transcribed_from.is_empty()
            || !matches!(file.file.verification.as_str(), "single" | "double")
        {
            return Err(invalid(
                "ship roster requires citations, source filenames and verification",
            ));
        }
        let mut ships = BTreeMap::new();
        for ship in file.ships {
            if ship.id.is_empty()
                || ship.designation.is_empty()
                || !ship.src.iter().any(|s| s == "airlog:56.31")
                || ship.transcribed_from.is_empty()
                || ship.transcribed_from.iter().any(String::is_empty)
                || ship.capacity_tons.is_some_and(|n| n <= 0)
            {
                return Err(invalid(
                    "ship identity, defining case, provenance or capacity is invalid",
                ));
            }
            if ships.insert(ship.id.clone(), ship).is_some() {
                return Err(invalid("duplicate ship counter id"));
            }
        }
        if ships.is_empty() {
            return Err(invalid("empty ship roster"));
        }
        Ok(Self { ships })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Cases: airlog:56.31
    #[test]
    fn four_distinct_counters_have_double_read_printed_capacities_and_tracked_sources() {
        let path = crate::repo_data_dir().join("units/ships/axis_coastal.toml");
        let (roster, reads) = crate::record_reads(|| ShipRoster::load(&path).unwrap());
        assert_eq!(reads, vec![crate::normalize(&path)]);
        assert_eq!(roster.ships.len(), 4);
        for (letter, tons) in [('a', 1000), ('b', 1000), ('c', 1000), ('d', 2000)] {
            let ship = &roster.ships[&format!("axis.coastal.{letter}")];
            assert_eq!(ship.capacity_tons, Some(tons));
            assert_eq!(ship.designation, letter.to_ascii_uppercase().to_string());
            assert_eq!(ship.transcribed_from.len(), 1);
        }
    }
    /// Cases: airlog:56.31
    #[test]
    fn duplicate_uncited_and_nonpositive_counters_fail_but_missing_capacity_stays_unknown() {
        let path = crate::repo_data_dir().join("units/ships/axis_coastal.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        let mut file: ShipFile = toml::from_str(&text).unwrap();
        file.ships.push(file.ships[0].clone());
        assert!(ShipRoster::bind(file, &path).is_err());
        let mut file: ShipFile = toml::from_str(&text).unwrap();
        file.ships[0].src.clear();
        assert!(ShipRoster::bind(file, &path).is_err());
        let mut file: ShipFile = toml::from_str(&text).unwrap();
        file.ships[0].capacity_tons = Some(0);
        assert!(ShipRoster::bind(file, &path).is_err());
        let mut file: ShipFile = toml::from_str(&text).unwrap();
        file.ships[0].capacity_tons = None;
        assert_eq!(
            ShipRoster::bind(file, &path).unwrap().ships["axis.coastal.a"].capacity_tons,
            None
        );
    }
    /// Cases: airlog:56.31, scen:59.54
    #[test]
    fn scenario_roster_reference_is_validated_without_reading_external_paths() {
        let data = crate::repo_data_dir();
        let units = crate::units::UnitsContent::load(&data.join("units")).unwrap();
        let mut scenario =
            crate::scenario::ScenarioContent::load(&data.join("scenarios/graziani")).unwrap();
        scenario.check(&units).unwrap();
        assert_eq!(units.coastal_rosters["ships/axis_coastal.toml"].len(), 4);
        scenario
            .fleet
            .get_mut("axis_coastal_shipping")
            .unwrap()
            .as_table_mut()
            .unwrap()
            .insert(
                "roster".into(),
                toml::Value::String("../outside.toml".into()),
            );
        assert!(scenario.check(&units).is_err());
    }
}
