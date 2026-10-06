//! Terrain Effects Chart instructions, with exact quarter-point arithmetic for track reductions.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerrainFeature {
    Clear,
    #[serde(rename = "gravel", alias = "rock_gravel")]
    RockGravel,
    SaltMarsh,
    HeavyVegetation,
    Rough,
    Mountain,
    Delta,
    Desert,
    MajorCity,
    Swamp,
    VillageBirOasis,
    Railroad,
    Road,
    Track,
    Ridge,
    UpSlope,
    DownSlope,
    UpEscarpment,
    DownEscarpment,
    Wadi,
    MajorRiver,
    MinorRiver,
    FortificationLevelOne,
    FortificationLevelTwo,
    FortificationLevelThree,
    FriendlyMinefield,
    EnemyMinefield,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureGroup {
    HexTerrain,
    LinearFeature,
    HexsideFeature,
    Fortification,
    Minefield,
}

impl TerrainFeature {
    fn group(self) -> FeatureGroup {
        match self {
            Self::Railroad | Self::Road | Self::Track => FeatureGroup::LinearFeature,
            Self::Ridge
            | Self::UpSlope
            | Self::DownSlope
            | Self::UpEscarpment
            | Self::DownEscarpment
            | Self::Wadi
            | Self::MajorRiver
            | Self::MinorRiver => FeatureGroup::HexsideFeature,
            Self::FortificationLevelOne
            | Self::FortificationLevelTwo
            | Self::FortificationLevelThree => FeatureGroup::Fortification,
            Self::FriendlyMinefield | Self::EnemyMinefield => FeatureGroup::Minefield,
            _ => FeatureGroup::HexTerrain,
        }
    }
}

/// Numeric effects use quarter CP/Breakdown Points: 4 means one whole point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainValue {
    EnterQuarters(i32),
    AddQuarters(i32),
    ValueQuarters(i32),
    AddWholeCpa,
    RoadOrRailOnly,
    Prohibited,
    NoEffect,
    HalveTerrain,
    InheritTerrain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombatShift {
    Columns(i32),
    Prohibited,
    UseFortifications,
    InheritTerrain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackingLimit {
    Points(i32),
    NoEffect,
    InheritTerrain,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawValue {
    enter_x2: Option<i32>,
    add_x2: Option<i32>,
    value_x2: Option<i32>,
    #[serde(default)]
    add_cpa: bool,
    #[serde(default)]
    enter_only_on_road_or_railroad: bool,
    #[serde(default)]
    prohibited: bool,
    #[serde(default)]
    none: bool,
    modifier: Option<String>,
    #[serde(default)]
    footnotes: Vec<u8>,
}

impl RawValue {
    fn bind(
        self,
        raw: &RawTable,
        field: &str,
        breakdown: bool,
    ) -> Result<TerrainValue, TableError> {
        check_footnotes(raw, field, &self.footnotes)?;
        let count = usize::from(self.enter_x2.is_some())
            + usize::from(self.add_x2.is_some())
            + usize::from(self.value_x2.is_some())
            + usize::from(self.add_cpa)
            + usize::from(self.enter_only_on_road_or_railroad)
            + usize::from(self.prohibited)
            + usize::from(self.none)
            + usize::from(self.modifier.is_some());
        if count != 1 {
            return Err(raw.err(field, "exactly one cell effect required"));
        }
        let number = |value: i32, key: &str| {
            value.checked_mul(2).filter(|n| *n >= 0).ok_or_else(|| {
                raw.err(
                    format!("{field}.{key}"),
                    "must be a nonnegative half-point value fitting quarter-point arithmetic",
                )
            })
        };
        if let Some(n) = self.enter_x2 {
            if breakdown {
                return Err(raw.err(field, "breakdown uses value_x2, not enter_x2"));
            }
            return Ok(TerrainValue::EnterQuarters(number(n, "enter_x2")?));
        }
        if let Some(n) = self.add_x2 {
            return Ok(TerrainValue::AddQuarters(number(n, "add_x2")?));
        }
        if let Some(n) = self.value_x2 {
            if !breakdown {
                return Err(raw.err(field, "movement uses enter_x2, not value_x2"));
            }
            return Ok(TerrainValue::ValueQuarters(number(n, "value_x2")?));
        }
        if self.add_cpa && !breakdown {
            return Ok(TerrainValue::AddWholeCpa);
        }
        if self.enter_only_on_road_or_railroad && !breakdown {
            return Ok(TerrainValue::RoadOrRailOnly);
        }
        if self.prohibited && !breakdown {
            return Ok(TerrainValue::Prohibited);
        }
        if self.none {
            return Ok(TerrainValue::NoEffect);
        }
        if self.modifier.as_deref() == Some("halve_feature_costs") {
            return Ok(TerrainValue::HalveTerrain);
        }
        Err(raw.err(field, "unknown or misplaced cell effect"))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawShift {
    dir: Option<String>,
    n: Option<i32>,
    #[serde(default)]
    prohibited: bool,
    #[serde(default)]
    see_fortifications: bool,
    #[serde(default)]
    footnotes: Vec<u8>,
}

impl RawShift {
    fn bind(self, raw: &RawTable, field: &str) -> Result<CombatShift, TableError> {
        check_footnotes(raw, field, &self.footnotes)?;
        match (
            self.dir.as_deref(),
            self.n,
            self.prohibited,
            self.see_fortifications,
        ) {
            (Some("none"), None, false, false) => Ok(CombatShift::Columns(0)),
            (Some("L"), Some(n), false, false) if n > 0 => Ok(CombatShift::Columns(-n)),
            (Some("R"), Some(n), false, false) if n > 0 => Ok(CombatShift::Columns(n)),
            (None, None, true, false) => Ok(CombatShift::Prohibited),
            (None, None, false, true) => Ok(CombatShift::UseFortifications),
            _ => Err(raw.err(
                field,
                "one valid column shift, prohibition or fortification reference required",
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLimit {
    value: Option<i32>,
    #[serde(default)]
    none: bool,
    #[serde(default)]
    footnotes: Vec<u8>,
}
impl RawLimit {
    fn bind(self, raw: &RawTable, field: &str) -> Result<StackingLimit, TableError> {
        check_footnotes(raw, field, &self.footnotes)?;
        match (self.value, self.none) {
            (Some(n), false) if n >= 0 => Ok(StackingLimit::Points(n)),
            (None, true) => Ok(StackingLimit::NoEffect),
            _ => Err(raw.err(field, "one nonnegative limit or no-effect marker required")),
        }
    }
}

fn check_footnotes(raw: &RawTable, field: &str, ids: &[u8]) -> Result<(), TableError> {
    let unique: std::collections::BTreeSet<_> = ids.iter().copied().collect();
    if unique.len() != ids.len() || ids.iter().any(|n| !(1..=13).contains(n)) {
        return Err(raw.err(
            format!("{field}.footnotes"),
            "unique footnotes in 1-13 required",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
struct RawRow {
    id: TerrainFeature,
    group: FeatureGroup,
    cp_non_mot: Option<RawValue>,
    cp_mot: Option<RawValue>,
    breakdown: Option<RawValue>,
    barrage_shift: Option<RawShift>,
    anti_armor_shift: Option<RawShift>,
    close_assault_shift: Option<RawShift>,
    stacking_limit: Option<RawLimit>,
    #[serde(default)]
    same_as_terrain_all_purposes: bool,
    #[serde(default)]
    same_as_terrain_for_cp_and_breakdown: bool,
    #[serde(default)]
    footnotes: Vec<u8>,
}
#[derive(Deserialize)]
struct Footnote {
    id: u8,
}
#[derive(Deserialize)]
struct Body {
    row: Vec<RawRow>,
    footnote: Vec<Footnote>,
}

/// One complete chart instruction. None means the chart leaves that field blank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerrainRow {
    pub group: FeatureGroup,
    pub cp_non_motorized: Option<TerrainValue>,
    pub cp_motorized: Option<TerrainValue>,
    pub breakdown: Option<TerrainValue>,
    pub barrage_shift: CombatShift,
    pub anti_armor_shift: CombatShift,
    pub close_assault_shift: CombatShift,
    pub stacking_limit: StackingLimit,
}

#[derive(Debug, Clone)]
pub struct TerrainEffects {
    rows: BTreeMap<TerrainFeature, TerrainRow>,
}

impl Bound for TerrainEffects {
    const ID: &'static str = "land.8.37.terrain_effects";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: Body = raw.deserialize()?;
        let notes: std::collections::BTreeSet<_> = body.footnote.iter().map(|n| n.id).collect();
        if notes != (1..=13).collect() || body.footnote.len() != 13 {
            return Err(raw.err(
                "footnote.id",
                "each of the thirteen numbered footnotes is required once",
            ));
        }
        let mut rows = BTreeMap::new();
        for (i, r) in body.row.into_iter().enumerate() {
            let prefix = format!("row[{i}]");
            if r.group != r.id.group() {
                return Err(raw.err(format!("{prefix}.group"), "group disagrees with feature"));
            }
            check_footnotes(raw, &prefix, &r.footnotes)?;
            let all = r.same_as_terrain_all_purposes;
            let cp = r.same_as_terrain_for_cp_and_breakdown;
            if all && cp {
                return Err(raw.err(&prefix, "inheritance flags are mutually exclusive"));
            }
            if (all || cp)
                && (r.cp_non_mot.is_some() || r.cp_mot.is_some() || r.breakdown.is_some())
            {
                return Err(raw.err(
                    &prefix,
                    "inherited movement/breakdown must not specify cells",
                ));
            }
            if all
                && (r.barrage_shift.is_some()
                    || r.anti_armor_shift.is_some()
                    || r.close_assault_shift.is_some()
                    || r.stacking_limit.is_some())
            {
                return Err(raw.err(&prefix, "all-purpose inheritance must not specify cells"));
            }
            let movement = |cell: Option<RawValue>, key: &str| {
                if all || cp {
                    Ok(Some(TerrainValue::InheritTerrain))
                } else {
                    cell.map(|v| v.bind(raw, &format!("{prefix}.{key}"), false))
                        .transpose()
                }
            };
            let shift = |cell: Option<RawShift>, key: &str| {
                if all {
                    Ok(CombatShift::InheritTerrain)
                } else {
                    cell.ok_or_else(|| raw.err(format!("{prefix}.{key}"), "missing shift"))?
                        .bind(raw, &format!("{prefix}.{key}"))
                }
            };
            let row = TerrainRow {
                group: r.group,
                cp_non_motorized: movement(r.cp_non_mot, "cp_non_mot")?,
                cp_motorized: movement(r.cp_mot, "cp_mot")?,
                breakdown: if all || cp {
                    Some(TerrainValue::InheritTerrain)
                } else {
                    r.breakdown
                        .map(|v| v.bind(raw, &format!("{prefix}.breakdown"), true))
                        .transpose()?
                },
                barrage_shift: shift(r.barrage_shift, "barrage_shift")?,
                anti_armor_shift: shift(r.anti_armor_shift, "anti_armor_shift")?,
                close_assault_shift: shift(r.close_assault_shift, "close_assault_shift")?,
                stacking_limit: if all {
                    StackingLimit::InheritTerrain
                } else {
                    r.stacking_limit
                        .ok_or_else(|| {
                            raw.err(format!("{prefix}.stacking_limit"), "missing limit")
                        })?
                        .bind(raw, &format!("{prefix}.stacking_limit"))?
                },
            };
            if row.cp_non_motorized.is_none() || row.cp_motorized.is_none() {
                return Err(raw.err(&prefix, "both movement cells are required"));
            }
            if row.breakdown.is_none() && r.id != TerrainFeature::Swamp {
                return Err(raw.err(
                    format!("{prefix}.breakdown"),
                    "only swamp has a blank breakdown value",
                ));
            }
            if rows.insert(r.id, row).is_some() {
                return Err(raw.err(format!("{prefix}.id"), "duplicate feature"));
            }
        }
        if rows.len() != 27 {
            return Err(raw.err("row.id", "all 27 chart features are required"));
        }
        Ok(Self { rows })
    }
}

impl TerrainEffects {
    /// Positive column shifts go right, negative shifts left. Restrictions are explicit variants.
    /// Cases: land:8.37, interp:land-0002
    pub fn feature(&self, feature: TerrainFeature) -> &TerrainRow {
        &self.rows[&feature]
    }

    /// Track halves hex/hexside values. A vehicle descending an escarpment keeps both full costs.
    /// This supplies one feature contribution; movement procedures combine features and restrictions.
    /// Cases: land:8.37, land:8.46, interp:land-0002
    pub fn track_values(
        &self,
        feature: TerrainFeature,
        motorized: bool,
    ) -> (Option<TerrainValue>, Option<TerrainValue>) {
        let row = self.feature(feature);
        let cp = if motorized {
            row.cp_motorized
        } else {
            row.cp_non_motorized
        };
        if (motorized && feature == TerrainFeature::DownEscarpment)
            || !matches!(
                row.group,
                FeatureGroup::HexTerrain | FeatureGroup::HexsideFeature
            )
        {
            return (cp, row.breakdown);
        }
        let halve = |v: TerrainValue| match v {
            TerrainValue::EnterQuarters(n) => TerrainValue::EnterQuarters(n / 2),
            TerrainValue::AddQuarters(n) => TerrainValue::AddQuarters(n / 2),
            TerrainValue::ValueQuarters(n) => TerrainValue::ValueQuarters(n / 2),
            other => other,
        };
        (cp.map(halve), row.breakdown.map(halve))
    }

    /// Cases: land:8.37, interp:land-0002
    pub fn city_fortification(&self, alexandria_or_cairo: bool) -> &TerrainRow {
        self.feature(if alexandria_or_cairo {
            TerrainFeature::FortificationLevelThree
        } else {
            TerrainFeature::FortificationLevelTwo
        })
    }
}
