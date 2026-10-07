//! Replacement-point costs per TOE strength point, with chart alternatives kept distinct.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplacementUnit {
    AnyHeadquartersUnit,
    Commando,
    ParatroopInfantry,
    ItalianBersaglieriInfantry,
    Machinegun,
    HeavyWeapons,
    AnyOtherInfantry,
    EngineerBattalion,
    EngineerCompany,
    RoadConstructionOrRailroadConstruction,
    MotorcycleReconnaissanceCamelCavalryOrReconnaissance,
    ArmoredReconnaissance,
    ArmoredCar,
    Tank,
    Artillery,
    AirdroppableArtillery,
    AntiTank,
    AirdroppableAntiTank,
    AntiAir,
}

impl ReplacementUnit {
    pub const ALL: [Self; 19] = [
        Self::AnyHeadquartersUnit,
        Self::Commando,
        Self::ParatroopInfantry,
        Self::ItalianBersaglieriInfantry,
        Self::Machinegun,
        Self::HeavyWeapons,
        Self::AnyOtherInfantry,
        Self::EngineerBattalion,
        Self::EngineerCompany,
        Self::RoadConstructionOrRailroadConstruction,
        Self::MotorcycleReconnaissanceCamelCavalryOrReconnaissance,
        Self::ArmoredReconnaissance,
        Self::ArmoredCar,
        Self::Tank,
        Self::Artillery,
        Self::AirdroppableArtillery,
        Self::AntiTank,
        Self::AirdroppableAntiTank,
        Self::AntiAir,
    ];

    fn chart_note(self) -> Option<ConversionNote> {
        match self {
            Self::Commando => Some(ConversionNote::LayforceOrSas),
            Self::ParatroopInfantry => Some(ConversionNote::FolgoreOrRamcke),
            Self::Machinegun => Some(ConversionNote::MachinegunMotorizationDoesNotMatter),
            Self::HeavyWeapons => Some(ConversionNote::HeavyWeaponsExcludesRamcke),
            Self::EngineerBattalion => Some(ConversionNote::IncludesAustralianPioneer),
            Self::RoadConstructionOrRailroadConstruction => {
                Some(ConversionNote::CommonwealthConstructionReturn)
            }
            Self::AirdroppableAntiTank => Some(ConversionNote::FolgoreOrRamckeHeadquartersAntiTank),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum ReplacementPointKind {
    #[serde(rename = "inf")]
    Infantry,
    /// Any artillery, anti-tank or anti-aircraft gun point, regardless of rating.
    #[serde(rename = "gun")]
    AnyGun,
    #[serde(rename = "tank")]
    Tank,
    /// Italian points use the Autobelinda 41 (vv) restriction in the chart key.
    #[serde(rename = "armr")]
    ArmoredRecon,
    /// Only the light-tank upgrade option permitted by land:20.5.
    #[serde(rename = "lt_tank")]
    UpgradeLightTank,
    #[serde(rename = "gun_artillery")]
    ArtilleryGun,
    #[serde(rename = "gun_anti_tank")]
    AntiTankGun,
    #[serde(rename = "gun_anti_air")]
    AntiAirGun,
    #[serde(rename = "italian_para_art")]
    ItalianParaArtillery,
    #[serde(rename = "german_7_5cm_light_gun")]
    German75CmLightGun,
    /// Restricted to Italian 47/32 or German 2.8cm/28/20 Pak equipment.
    #[serde(rename = "lt_at")]
    LightAntiTank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum ConversionNote {
    #[serde(rename = "a")]
    LayforceOrSas,
    #[serde(rename = "b")]
    FolgoreOrRamcke,
    #[serde(rename = "c")]
    MachinegunMotorizationDoesNotMatter,
    #[serde(rename = "d")]
    HeavyWeaponsExcludesRamcke,
    #[serde(rename = "e")]
    IncludesAustralianPioneer,
    #[serde(rename = "f")]
    CommonwealthConstructionReturn,
    #[serde(rename = "g")]
    FolgoreOrRamckeHeadquartersAntiTank,
}

impl ConversionNote {
    /// Construction units covered by note f return six Operations Stages after elimination.
    /// Cases: land:20.3
    pub fn return_delay_op_stages(self) -> Option<i32> {
        (self == Self::CommonwealthConstructionReturn).then_some(6)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReplacementCost {
    points: BTreeMap<ReplacementPointKind, i32>,
}

impl ReplacementCost {
    /// Counts of each replacement class in this one complete payment option.
    /// Cases: land:20.3
    pub fn points(&self) -> &BTreeMap<ReplacementPointKind, i32> {
        &self.points
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplacementRequirement {
    NoPoints,
    All(ReplacementCost),
    Alternatives(Vec<ReplacementCost>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoPoints {
    none: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Combined {
    and: BTreeMap<ReplacementPointKind, i32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Alternatives {
    alternatives: Vec<BTreeMap<ReplacementPointKind, i32>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawRequirement {
    NoPoints(NoPoints),
    Combined(Combined),
    Alternatives(Alternatives),
    Single(BTreeMap<ReplacementPointKind, i32>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    unit: ReplacementUnit,
    requires: RawRequirement,
    #[serde(default)]
    footnotes: Vec<ConversionNote>,
}

#[derive(Deserialize)]
struct Footnote {
    id: ConversionNote,
    text: String,
}

#[derive(Deserialize)]
struct Body {
    row: Vec<Row>,
    footnote: Vec<Footnote>,
}

#[derive(Debug, Clone)]
struct Entry {
    requirement: ReplacementRequirement,
    notes: Vec<ConversionNote>,
}

#[derive(Debug, Clone)]
pub struct ReplacementConversion {
    entries: BTreeMap<ReplacementUnit, Entry>,
}

fn cost(
    raw: &RawTable,
    field: &str,
    points: BTreeMap<ReplacementPointKind, i32>,
) -> Result<ReplacementCost, TableError> {
    if points.is_empty() || points.values().any(|n| *n <= 0) {
        return Err(raw.err(field, "a payment must contain positive point counts"));
    }
    Ok(ReplacementCost { points })
}

fn requirement(
    raw: &RawTable,
    field: &str,
    value: RawRequirement,
) -> Result<ReplacementRequirement, TableError> {
    match value {
        RawRequirement::NoPoints(NoPoints { none: true }) => Ok(ReplacementRequirement::NoPoints),
        RawRequirement::NoPoints(_) => Err(raw.err(field, "none must be true")),
        RawRequirement::Single(points) => {
            if points.len() != 1 {
                return Err(raw.err(field, "multiple classes require an explicit combination"));
            }
            Ok(ReplacementRequirement::All(cost(raw, field, points)?))
        }
        RawRequirement::Combined(Combined { and }) => {
            if and.len() < 2 {
                return Err(raw.err(field, "a combination requires at least two classes"));
            }
            Ok(ReplacementRequirement::All(cost(raw, field, and)?))
        }
        RawRequirement::Alternatives(Alternatives { alternatives }) => {
            if alternatives.len() < 2 {
                return Err(raw.err(field, "at least two alternatives are required"));
            }
            let mut choices = Vec::new();
            let mut seen = BTreeSet::new();
            for points in alternatives {
                let payment = cost(raw, field, points)?;
                if !seen.insert(payment.clone()) {
                    return Err(raw.err(field, "duplicate payment alternative"));
                }
                choices.push(payment);
            }
            Ok(ReplacementRequirement::Alternatives(choices))
        }
    }
}

impl Bound for ReplacementConversion {
    const ID: &'static str = "land.20.3.replacement_point_conversion";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: Body = raw.deserialize()?;
        let mut notes = BTreeSet::new();
        for note in body.footnote {
            if note.text.trim().is_empty() || !notes.insert(note.id) {
                return Err(raw.err("footnote", "identified notes must be nonempty and unique"));
            }
        }
        if notes.len() != 7 {
            return Err(raw.err("footnote", "all seven chart notes are required"));
        }
        let mut entries = BTreeMap::new();
        for (i, row) in body.row.into_iter().enumerate() {
            let field = format!("row[{i}]");
            let expected: Vec<_> = row.unit.chart_note().into_iter().collect();
            if row.footnotes != expected {
                return Err(raw.err(
                    format!("{field}.footnotes"),
                    "note references must match the chart row",
                ));
            }
            let entry = Entry {
                requirement: requirement(raw, &format!("{field}.requires"), row.requires)?,
                notes: row.footnotes,
            };
            if entries.insert(row.unit, entry).is_some() {
                return Err(raw.err(format!("{field}.unit"), "duplicate conversion unit"));
            }
        }
        if ReplacementUnit::ALL
            .iter()
            .any(|unit| !entries.contains_key(unit))
        {
            return Err(raw.err("row", "all nineteen conversion unit rows are required"));
        }
        Ok(Self { entries })
    }
}

impl ReplacementConversion {
    /// Replacement cost for one TOE strength point, before eligibility and training checks.
    /// The SGSU row is excluded by the incorporated erratum; it is not a payment option.
    /// Cases: land:20.3
    pub fn requirement(&self, unit: ReplacementUnit) -> &ReplacementRequirement {
        &self.entries[&unit].requirement
    }

    /// Footnote restrictions carried by the selected chart row; callers apply their scope.
    /// Cases: land:20.3
    pub fn notes(&self, unit: ReplacementUnit) -> &[ConversionNote] {
        &self.entries[&unit].notes
    }
}
