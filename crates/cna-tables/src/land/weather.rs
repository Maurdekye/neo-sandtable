//! Seasonal weather and storm map sections (section 29).

use std::collections::BTreeSet;

use cna_core::dice::{Die, TwoDiceReading};
use serde::{Deserialize, Serialize};

use crate::ranges::{IntRange, check_int_tiling, check_reading_tiling};
use crate::{Bound, DiceKind, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Season {
    Fall,
    Winter,
    Spring,
    Summer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherKind {
    Normal,
    Hot,
    Sandstorm,
    Rainstorm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MapSection {
    A,
    B,
    C,
    D,
    E,
}

#[derive(Debug, Clone, Deserialize)]
struct WeatherRow {
    season: Season,
    game_turns: Vec<IntRange>,
    normal: Option<IntRange>,
    hot: Option<IntRange>,
    sandstorm: Option<IntRange>,
    rainstorm: Option<IntRange>,
}

impl WeatherRow {
    fn cells(&self) -> [(WeatherKind, Option<IntRange>); 4] {
        [
            (WeatherKind::Normal, self.normal),
            (WeatherKind::Hot, self.hot),
            (WeatherKind::Sandstorm, self.sandstorm),
            (WeatherKind::Rainstorm, self.rainstorm),
        ]
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WeatherTable {
    row: Vec<WeatherRow>,
}

impl Bound for WeatherTable {
    const ID: &'static str = "land.29.6.weather";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        if raw.envelope.dice != DiceKind::TwoD6Reading {
            return Err(raw.err(
                "table.dice",
                "weather requires sequential two-dice readings",
            ));
        }
        let table: Self = raw.deserialize()?;
        let mut seasons = BTreeSet::new();
        let mut turns = Vec::new();
        for (i, row) in table.row.iter().enumerate() {
            if !seasons.insert(row.season) {
                return Err(raw.err(format!("row[{i}].season"), "duplicate season"));
            }
            if row.game_turns.is_empty() {
                return Err(raw.err(
                    format!("row[{i}].game_turns"),
                    "at least one turn range required",
                ));
            }
            turns.extend(row.game_turns.iter().copied());
            let mut ranges = Vec::new();
            for (kind, cell) in row.cells() {
                if let Some(range) = cell {
                    if !crate::ranges::is_reading(range.lo) || !crate::ranges::is_reading(range.hi)
                    {
                        return Err(raw.err(
                            format!("row[{i}].{kind:?}"),
                            "range endpoints must be real dice readings",
                        ));
                    }
                    ranges.push(range);
                }
            }
            check_reading_tiling(ranges).map_err(|e| raw.err(format!("row[{i}]"), e))?;
        }
        if seasons.len() != 4 {
            return Err(raw.err("row.season", "all four seasons required"));
        }
        // The printed chart stops at 110; 111 stays explicitly unsupported.
        check_int_tiling(turns, 1, 110).map_err(|e| raw.err("row.game_turns", e))?;
        Ok(table)
    }
}

impl WeatherTable {
    /// No season is printed for turn 111; return None for that turn or any out-of-chart input.
    /// Cases: land:29.6, land:29.61, land:29.1
    /// Interpretations: interp:land-0019
    pub fn season(&self, game_turn: i32) -> Option<Season> {
        self.row
            .iter()
            .find(|r| r.game_turns.iter().any(|span| span.contains(game_turn)))
            .map(|r| r.season)
    }

    /// Printed seasonal cells are preserved, including winter hot weather and summer rain.
    /// A storm result requires the separate location roll; it is not global weather.
    /// Cases: land:29.6, land:29.61, land:29.1
    /// Interpretations: interp:land-0019
    pub fn result(&self, game_turn: i32, reading: TwoDiceReading) -> Option<WeatherKind> {
        let row = self
            .row
            .iter()
            .find(|r| r.game_turns.iter().any(|span| span.contains(game_turn)))?;
        row.cells()
            .into_iter()
            .find(|(_, cell)| cell.is_some_and(|span| span.contains(i32::from(reading.value()))))
            .map(|(kind, _)| kind)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct LocationRow {
    die: u8,
    map_sections: Vec<MapSection>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FoulWeatherLocation {
    row: Vec<LocationRow>,
}

impl Bound for FoulWeatherLocation {
    const ID: &'static str = "land.29.7.foul_weather_location";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        if raw.envelope.dice != DiceKind::OneD6 {
            return Err(raw.err("table.dice", "storm location requires one six-sided die"));
        }
        let table: Self = raw.deserialize()?;
        let mut faces = BTreeSet::new();
        for (i, row) in table.row.iter().enumerate() {
            if !(1..=6).contains(&row.die) || !faces.insert(row.die) {
                return Err(raw.err(format!("row[{i}].die"), "unique faces in 1-6 required"));
            }
            let sections: BTreeSet<_> = row.map_sections.iter().copied().collect();
            if sections.is_empty() || sections.len() != row.map_sections.len() {
                return Err(raw.err(
                    format!("row[{i}].map_sections"),
                    "at least one unique map section required",
                ));
            }
        }
        if faces.len() != 6 {
            return Err(raw.err("row.die", "each die face required"));
        }
        Ok(table)
    }
}

impl FoulWeatherLocation {
    /// Sections receiving the storm. Procedures exclude offshore areas and normalize delta sandstorms.
    /// Cases: land:29.7, land:29.1
    pub fn sections(&self, die: Die) -> &[MapSection] {
        &self
            .row
            .iter()
            .find(|r| r.die == die.value())
            .expect("validated die face")
            .map_sections
    }
}
