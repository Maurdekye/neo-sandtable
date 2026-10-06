//! The immutable content a CNA campaign reads: map, units, scenario, charts and tables, and the
//! rule-case registry.

use std::path::Path;

use cna_content::areas::AreasContent;
use cna_content::map::MapContent;
use cna_content::registry::Registry;
use cna_content::scenario::ScenarioContent;
use cna_content::units::UnitsContent;
use cna_protocol::Side;
use cna_tables::Tables;

use crate::seq::{Bounds, System};

/// Everything a campaign of one scenario reads.
pub struct CnaContent {
    pub map: MapContent,
    pub areas: AreasContent,
    pub units: UnitsContent,
    pub scenario: ScenarioContent,
    pub tables: Tables,
    pub registry: Registry,
    pub bounds: Bounds,
    /// `land:7.2`, read from the raw table until `cna-tables` binds it.
    pub initiative_ratings: InitiativeRatings,
}

impl std::fmt::Debug for CnaContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CnaContent")
            .field("scenario", &self.scenario.meta.id)
            .field("hexes", &self.map.len())
            .field("units", &self.units.units.len())
            .finish_non_exhaustive()
    }
}

impl CnaContent {
    /// Load `data_dir` (the repository's `data/`) for scenario `scenario_id`.
    pub fn load(data_dir: &Path, scenario_id: &str) -> Result<Self, String> {
        let map = MapContent::load(&data_dir.join("map")).map_err(|e| e.to_string())?;
        let areas = AreasContent::load(&data_dir.join("map/areas.toml"), &map)
            .map_err(|e| e.to_string())?;
        let units = UnitsContent::load(&data_dir.join("units")).map_err(|e| e.to_string())?;
        let scenario = ScenarioContent::load(&data_dir.join("scenarios").join(scenario_id))
            .map_err(|e| e.to_string())?;
        scenario.check(&units).map_err(|e| e.to_string())?;
        let raw = cna_tables::RawSet::new(
            cna_tables::raw::read_all(data_dir).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let tables = Tables::bind_all(&raw).map_err(|e| e.to_string())?;
        let initiative_ratings = InitiativeRatings::from_raw(&raw)?;
        let registry = Registry::load(&data_dir.join("rules")).map_err(|e| e.to_string())?;
        let systems = scenario
            .meta
            .systems
            .iter()
            .map(|s| match s.as_str() {
                "land" => Ok(System::Land),
                "air" => Ok(System::Air),
                "logistics" => Ok(System::Logistics),
                other => Err(format!(
                    "unknown system {other:?} in scenario {scenario_id}"
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let bounds = Bounds {
            start_gt: scenario.meta.start.gt,
            end_gt: scenario.meta.end.gt,
            end_opstage: scenario.meta.end.opstage,
            systems,
        };
        Ok(CnaContent {
            map,
            areas,
            units,
            scenario,
            tables,
            registry,
            bounds,
            initiative_ratings,
        })
    }

    /// The scenario id the registry's `applies` keys use (`graziani`).
    pub fn scenario_key(&self) -> &str {
        &self.scenario.meta.id
    }
}

/// The Initiative Ratings Chart (`land:7.2`).
#[derive(Debug, Clone, Default)]
pub struct InitiativeRatings {
    /// Commonwealth: (first game-turn, last game-turn, rating).
    pub commonwealth: Vec<(u16, u16, i32)>,
    /// Axis rating by situation id.
    pub axis: Vec<(String, i32)>,
}

impl InitiativeRatings {
    const ID: &'static str = "land.7.2.initiative_ratings";
    /// The Axis situation with no German combat units or Rommel on the game maps.
    pub const AXIS_NO_GERMANS: &'static str =
        "no_german_combat_units_or_rommel_counter_on_game_maps";

    fn from_raw(raw: &cna_tables::RawSet) -> Result<Self, String> {
        let table = raw
            .get(Self::ID)
            .ok_or_else(|| format!("missing table {}", Self::ID))?;
        let rows = table
            .body
            .get("row")
            .and_then(|r| r.as_array())
            .ok_or_else(|| format!("{}: no rows", Self::ID))?;
        let mut out = InitiativeRatings::default();
        for row in rows {
            let field = |k: &str| row.get(k);
            let rating = field("rating")
                .and_then(|v| v.as_integer())
                .ok_or_else(|| format!("{}: row without rating", Self::ID))?;
            let rating = i32::try_from(rating).map_err(|e| e.to_string())?;
            match field("player").and_then(|v| v.as_str()) {
                Some("commonwealth") => {
                    let range = field("game_turn_range")
                        .and_then(|v| v.as_array())
                        .ok_or_else(|| format!("{}: CW row without range", Self::ID))?;
                    let bound = |i: usize| {
                        range
                            .get(i)
                            .and_then(|v| v.as_integer())
                            .and_then(|v| u16::try_from(v).ok())
                            .ok_or_else(|| format!("{}: bad range", Self::ID))
                    };
                    out.commonwealth.push((bound(0)?, bound(1)?, rating));
                }
                Some("axis") => {
                    let situation = field("situation")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| format!("{}: axis row without situation", Self::ID))?;
                    out.axis.push((situation.to_owned(), rating));
                }
                other => return Err(format!("{}: unknown player {other:?}", Self::ID)),
            }
        }
        Ok(out)
    }

    /// A side's rating on `game_turn`. For the Axis, `axis_situation` names the chart row.
    /// Cases: land:7.13
    pub fn rating(&self, side: Side, game_turn: u16, axis_situation: &str) -> Option<i32> {
        match side {
            Side::Commonwealth => self
                .commonwealth
                .iter()
                .find(|(lo, hi, _)| (*lo..=*hi).contains(&game_turn))
                .map(|(_, _, r)| *r),
            Side::Axis => self
                .axis
                .iter()
                .find(|(s, _)| s == axis_situation)
                .map(|(_, r)| *r),
        }
    }
}
