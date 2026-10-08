//! Abstract-logistics route thresholds; no full hex or division eligibility is inferred.
use crate::ranges::IntRange;
use crate::{Bound, RawTable, TableError};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum ConvoyRoute {
    #[serde(rename = "r1")]
    One,
    #[serde(rename = "r2")]
    Two,
    #[serde(rename = "r3")]
    Three,
    #[serde(rename = "r4")]
    Four,
    #[serde(rename = "r5")]
    Five,
    #[serde(rename = "r6")]
    Six,
}
impl ConvoyRoute {
    pub const ALL: [Self; 6] = [
        Self::One,
        Self::Two,
        Self::Three,
        Self::Four,
        Self::Five,
        Self::Six,
    ];
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConvoyRowLocation {
    pub map_section: char,
    pub east_west_row: u8,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BombingLocation {
    Row(ConvoyRowLocation),
    Dash,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BombPointsBand {
    pub min: i32,
    pub max: Option<i32>,
}
impl BombPointsBand {
    fn contains(self, points: i32) -> bool {
        points >= self.min && self.max.is_none_or(|max| points <= max)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    bomb_points: Option<IntRange>,
    bomb_points_min: Option<i32>,
    location: BTreeMap<ConvoyRoute, String>,
}
#[derive(Deserialize)]
struct Body {
    row: Vec<Row>,
}
#[derive(Debug, Clone)]
pub struct AxisConvoyBombing {
    rows: Vec<(BombPointsBand, BTreeMap<ConvoyRoute, ConvoyRowLocation>)>,
}
impl Bound for AxisConvoyBombing {
    const ID: &'static str = "land.32.66.axis_convoy_bombing";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: Body = raw.deserialize()?;
        let expected = [
            (21, Some(40)),
            (41, Some(80)),
            (81, Some(120)),
            (121, Some(160)),
            (161, Some(200)),
            (201, Some(260)),
            (261, Some(320)),
            (321, Some(390)),
            (391, Some(470)),
            (471, None),
        ];
        let mut rows = BTreeMap::new();
        for (i, row) in body.row.into_iter().enumerate() {
            let band = match (row.bomb_points, row.bomb_points_min) {
                (Some(r), None) => (r.lo, Some(r.hi)),
                (None, Some(min)) => (min, None),
                _ => {
                    return Err(raw.err(
                        format!("row[{i}].bomb_points"),
                        "one bounded range or open minimum required",
                    ));
                }
            };
            let Some(index) = expected.iter().position(|e| *e == band) else {
                return Err(raw.err(format!("row[{i}].bomb_points"), "unknown printed bomb band"));
            };
            let first_route = match index {
                0..=3 => 0,
                4..=5 => 1,
                6..=8 => 2,
                _ => 3,
            };
            if row.location.keys().copied().collect::<BTreeSet<_>>()
                != ConvoyRoute::ALL[first_route..].iter().copied().collect()
            {
                return Err(raw.err(
                    format!("row[{i}].location"),
                    "exact printed route cells and dashes required",
                ));
            }
            let mut locations = BTreeMap::new();
            for (route, code) in row.location {
                let bytes = code.as_bytes();
                let valid = bytes.len() == 5
                    && (b'A'..=b'E').contains(&bytes[0])
                    && bytes[1..3] == *b"xx"
                    && bytes[3..].iter().all(u8::is_ascii_digit);
                if !valid {
                    return Err(raw.err(
                        format!("row[{i}].location"),
                        "expected section plus xx plus two row digits",
                    ));
                }
                let number = (bytes[3] - b'0') * 10 + bytes[4] - b'0';
                if !(1..=33).contains(&number) {
                    return Err(
                        raw.err(format!("row[{i}].location"), "east-west row outside 01..33")
                    );
                }
                locations.insert(
                    route,
                    ConvoyRowLocation {
                        map_section: char::from(bytes[0]),
                        east_west_row: number,
                    },
                );
            }
            if rows.insert(band, locations).is_some() {
                return Err(raw.err(format!("row[{i}]"), "duplicate bomb band"));
            }
        }
        if rows.len() != expected.len() {
            return Err(raw.err("row", "all ten bomb bands required"));
        }
        Ok(Self {
            rows: rows
                .into_iter()
                .map(|((min, max), cells)| (BombPointsBand { min, max }, cells))
                .collect(),
        })
    }
}
impl AxisConvoyBombing {
    /// Printed location or dash at a bomb-point band; points below 21 return None.
    /// Cases: land:32.64, land:32.66
    pub fn location(&self, route: ConvoyRoute, bomb_points: i32) -> Option<BombingLocation> {
        let (_, cells) = self
            .rows
            .iter()
            .find(|(band, _)| band.contains(bomb_points))?;
        Some(
            cells
                .get(&route)
                .copied()
                .map_or(BombingLocation::Dash, BombingLocation::Row),
        )
    }
    /// Ascending printed bands for a procedure to find the qualifying division threshold.
    /// Row codes are not ordered real hexes; eligibility and the subsequent CRT roll are external.
    /// Cases: land:32.64, land:32.66
    pub fn column(
        &self,
        route: ConvoyRoute,
    ) -> impl Iterator<Item = (BombPointsBand, BombingLocation)> + '_ {
        self.rows.iter().map(move |(band, cells)| {
            (
                *band,
                cells
                    .get(&route)
                    .copied()
                    .map_or(BombingLocation::Dash, BombingLocation::Row),
            )
        })
    }
}
