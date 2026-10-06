//! Air Distance (37.4) and Land Distance (37.42) charts.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{Bound, RawTable, TableError};

// ---------------------------------------------------------------------------------------------
// 37.4 Air Distance
// ---------------------------------------------------------------------------------------------

/// The four Axis off-map areas of part A (`airlog:37.4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxisArea {
    Tripolitania,
    Tripoli,
    Gabes,
    Tunis,
}

/// The places part A measures from the four areas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NorthAfricaPlace {
    Malta,
    Nofilia,
    Benghazi,
}

/// The northern Mediterranean off-map areas of part B.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NorthMedArea {
    Crete,
    Malta,
    Sicily,
    Italy,
}

/// The map places part B measures from the northern Mediterranean areas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AirPlace {
    Malta,
    Nofilia,
    Benghazi,
    Derna,
    Tobruk,
    Bardia,
    MersaMatruh,
    Alexandria,
}

/// A part-B air distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirLeg {
    Hexes(i32),
    /// The chart prints P: the distance cannot be flown directly.
    Prohibited,
}

#[derive(Debug, Clone, Deserialize)]
struct PairRaw {
    from: AxisArea,
    to: AxisArea,
    hexes: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct PartARowRaw {
    place: NorthAfricaPlace,
    to: BTreeMap<AxisArea, i32>,
}

#[derive(Debug, Clone, Deserialize)]
struct PartBRowRaw {
    place: AirPlace,
    #[serde(default)]
    crete: Option<i32>,
    #[serde(default)]
    malta: Option<i32>,
    #[serde(default)]
    sicily: Option<i32>,
    #[serde(default)]
    italy: Option<i32>,
    #[serde(default)]
    prohibited: Vec<NorthMedArea>,
    #[serde(default)]
    dash_columns: Vec<NorthMedArea>,
}

#[derive(Debug, Clone, Deserialize)]
struct AirDistanceRaw {
    part_a_pair: Vec<PairRaw>,
    part_a_row: Vec<PartARowRaw>,
    part_b_row: Vec<PartBRowRaw>,
}

/// The Air Distance Chart (`airlog:37.4`).
#[derive(Debug, Clone)]
pub struct AirDistance {
    areas: BTreeMap<(AxisArea, AxisArea), i32>,
    places: BTreeMap<(NorthAfricaPlace, AxisArea), i32>,
    legs: BTreeMap<(AirPlace, NorthMedArea), Option<AirLeg>>,
}

const AXIS_AREAS: [AxisArea; 4] = [
    AxisArea::Tripolitania,
    AxisArea::Tripoli,
    AxisArea::Gabes,
    AxisArea::Tunis,
];

const NORTH_MED: [NorthMedArea; 4] = [
    NorthMedArea::Crete,
    NorthMedArea::Malta,
    NorthMedArea::Sicily,
    NorthMedArea::Italy,
];

impl Bound for AirDistance {
    const ID: &'static str = "airlog.37.4.air_distance";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: AirDistanceRaw = raw.deserialize()?;
        let mut areas = BTreeMap::new();
        for (i, p) in r.part_a_pair.iter().enumerate() {
            if p.from == p.to || p.hexes <= 0 {
                return Err(raw.err(
                    format!("part_a_pair[{i}]"),
                    "needs two different areas and a positive distance",
                ));
            }
            if areas.insert((p.from, p.to), p.hexes).is_some()
                || areas.contains_key(&(p.to, p.from))
            {
                return Err(raw.err(format!("part_a_pair[{i}]"), "area pair listed twice"));
            }
            areas.insert((p.to, p.from), p.hexes);
        }
        for a in AXIS_AREAS {
            for b in AXIS_AREAS {
                if a != b && !areas.contains_key(&(a, b)) {
                    return Err(raw.err("part_a_pair", format!("missing the {a:?}-{b:?} distance")));
                }
            }
        }
        let mut places = BTreeMap::new();
        for (i, row) in r.part_a_row.iter().enumerate() {
            for a in AXIS_AREAS {
                match row.to.get(&a) {
                    Some(&d) if d > 0 => {
                        places.insert((row.place, a), d);
                    }
                    _ => {
                        return Err(raw.err(
                            format!("part_a_row[{i}].to"),
                            format!("needs a positive distance to {a:?}"),
                        ));
                    }
                }
            }
        }
        for p in [
            NorthAfricaPlace::Malta,
            NorthAfricaPlace::Nofilia,
            NorthAfricaPlace::Benghazi,
        ] {
            if r.part_a_row.iter().filter(|x| x.place == p).count() != 1 {
                return Err(raw.err("part_a_row", format!("{p:?} must appear exactly once")));
            }
        }
        if r.part_b_row.len() != 8 {
            return Err(raw.err("part_b_row", "needs all eight places"));
        }
        let mut legs = BTreeMap::new();
        for (i, row) in r.part_b_row.iter().enumerate() {
            for area in NORTH_MED {
                let hexes = match area {
                    NorthMedArea::Crete => row.crete,
                    NorthMedArea::Malta => row.malta,
                    NorthMedArea::Sicily => row.sicily,
                    NorthMedArea::Italy => row.italy,
                };
                let prohibited = row.prohibited.contains(&area);
                let dash = row.dash_columns.contains(&area);
                let leg = match (hexes, prohibited, dash) {
                    (Some(d), false, false) if d > 0 => Some(AirLeg::Hexes(d)),
                    (None, true, false) => Some(AirLeg::Prohibited),
                    (None, false, true) => None,
                    _ => {
                        return Err(raw.err(
                            format!("part_b_row[{i}] ({:?}, {area:?})", row.place),
                            "needs exactly one of a distance, a prohibited entry, or a dash",
                        ));
                    }
                };
                if legs.insert((row.place, area), leg).is_some() {
                    return Err(raw.err(format!("part_b_row[{i}].place"), "place listed twice"));
                }
            }
        }
        Ok(Self {
            areas,
            places,
            legs,
        })
    }
}

impl AirDistance {
    /// Hexes between two Axis off-map areas (zero from an area to itself). `airlog:37.4`.
    pub fn between_axis_areas(&self, a: AxisArea, b: AxisArea) -> i32 {
        if a == b { 0 } else { self.areas[&(a, b)] }
    }

    /// Hexes from Malta, Nofilia or Benghazi to an Axis off-map area. `airlog:37.4`.
    pub fn place_to_axis_area(&self, place: NorthAfricaPlace, area: AxisArea) -> i32 {
        self.places[&(place, area)]
    }

    /// Air distance from a northern Mediterranean off-map area to a map place: a number of
    /// hexes or `Prohibited`; `None` where the chart prints a dash (Malta to itself) or lists
    /// no row for the place. `airlog:37.4`.
    pub fn from_north_med(&self, area: NorthMedArea, place: AirPlace) -> Option<AirLeg> {
        self.legs.get(&(place, area)).copied().flatten()
    }
}

// ---------------------------------------------------------------------------------------------
// 37.42 Land Distance
// ---------------------------------------------------------------------------------------------

/// The twenty places of the Land Distance Chart, in chart order (`airlog:37.42`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandPlace {
    Nofilia,
    ElAgheila,
    Benghazi,
    Agedabia,
    Soluch,
    Barce,
    BenGania,
    Mechili,
    Derna,
    C2803,
    Tobruk,
    Giarabub,
    FtMaddalena,
    Bardia,
    BirElQatrani,
    MersaMatruh,
    Gerawla,
    Alexandria,
    WadiNatrun,
    Cairo,
}

#[derive(Debug, Clone, Deserialize)]
struct PlaceHexRaw {
    id: LandPlace,
    hex: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LandRowRaw {
    place: LandPlace,
    to: Vec<i32>,
}

#[derive(Debug, Clone, Deserialize)]
struct LandDistanceRaw {
    places: Vec<PlaceHexRaw>,
    row: Vec<LandRowRaw>,
}

/// The Land Distance Chart (`airlog:37.42`): shortest distance in hexes between named places.
#[derive(Debug, Clone)]
pub struct LandDistance {
    hexes: BTreeMap<LandPlace, String>,
    pairs: BTreeMap<(LandPlace, LandPlace), i32>,
}

impl Bound for LandDistance {
    const ID: &'static str = "airlog.37.42.land_distance";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: LandDistanceRaw = raw.deserialize()?;
        if r.places.len() != 20 {
            return Err(raw.err("places", "needs all twenty named places"));
        }
        if r.row.len() + 1 != r.places.len() {
            return Err(raw.err("row", "needs one row per place after the first"));
        }
        let mut hexes = BTreeMap::new();
        for (i, p) in r.places.iter().enumerate() {
            if hexes.insert(p.id, p.hex.clone()).is_some() {
                return Err(raw.err(format!("places[{i}].id"), "place listed twice"));
            }
        }
        let mut pairs = BTreeMap::new();
        for (i, row) in r.row.iter().enumerate() {
            if row.place != r.places[i + 1].id {
                return Err(raw.err(
                    format!("row[{i}].place"),
                    "rows must follow the places order",
                ));
            }
            if row.to.len() != i + 1 {
                return Err(raw.err(
                    format!("row[{i}].to"),
                    format!("needs {} distances (one per earlier place)", i + 1),
                ));
            }
            for (j, &d) in row.to.iter().enumerate() {
                if d <= 0 {
                    return Err(raw.err(format!("row[{i}].to[{j}]"), "distances must be positive"));
                }
                let other = r.places[j].id;
                pairs.insert((row.place, other), d);
                pairs.insert((other, row.place), d);
            }
        }
        Ok(Self { hexes, pairs })
    }
}

impl LandDistance {
    /// Shortest distance in hexes between two places (symmetric; zero from a place to itself).
    /// `airlog:37.42`.
    pub fn distance(&self, a: LandPlace, b: LandPlace) -> i32 {
        if a == b { 0 } else { self.pairs[&(a, b)] }
    }

    /// The map hex id of a place, as the chart lists it. `airlog:37.42`.
    pub fn hex_of(&self, place: LandPlace) -> &str {
        &self.hexes[&place]
    }
}
