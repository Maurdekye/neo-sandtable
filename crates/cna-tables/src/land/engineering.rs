//! Construction and demolition chart records; procedures own eligibility, payment and timing.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkItem {
    FortificationLevel,
    Fortification,
    RealMinefield,
    FakeMinefield,
    Railroad,
    Road,
    TemporaryRepairFacility,
    RepairFacility,
    WaterPipeline,
    Airfield,
    AirfieldOrAirLandingStrip,
    FlyingBoatBasin,
    FlyingBoatBasinOrAlightingArea,
    AirFacility,
    Port,
    PortOfTobruk,
    PortOfBenghazi,
    PortOther,
    RealSupplyDump,
    FakeSupplyDump,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkSituation {
    Build,
    BuildOrRebuild,
    Rebuild,
    #[serde(rename = "rebuild_1_level")]
    Rebuild1Level,
    #[serde(rename = "rebuild_1_level_airfield_or_build_strip")]
    Rebuild1LevelAirfieldOrBuildStrip,
    #[serde(rename = "rebuild_1_level_basin_or_build_alighting_area")]
    Rebuild1LevelBasinOrBuildAlightingArea,
    #[serde(rename = "block_1_level")]
    Block1Level,
    #[serde(rename = "reduce_1_level")]
    Reduce1Level,
    Clear,
    Destroy,
    Dismantle,
    #[serde(rename = "unblock_1_level")]
    Unblock1Level,
    Blow,
}

/// Chart crew codes are alternatives; each nested construction group is a joint requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum CrewRequirement {
    #[serde(rename = "any_e")]
    AnyEngineer,
    #[serde(rename = "e_bn")]
    EngineerBattalion,
    #[serde(rename = "e_coy")]
    EngineerCompany,
    #[serde(rename = "hq_e")]
    EngineeringHq,
    #[serde(rename = "chq_e")]
    CommonwealthEngineeringHq,
    #[serde(rename = "sgsu")]
    Sgsu,
    #[serde(rename = "csgsu")]
    CommonwealthSgsu,
    #[serde(rename = "nzrrc")]
    NzRailroadCompany,
    #[serde(rename = "inf_bn_3")]
    InfantryBattalionThreeToe,
    #[serde(rename = "any_unit_1_toe")]
    AnyUnitOneToe,
    #[serde(rename = "any_unit")]
    AnyUnit,
    #[serde(rename = "scorpion_bn")]
    ScorpionBattalion,
    #[serde(rename = "two_e_bn_and_or_chq_e_total")]
    TwoEngineerBattalionsOrCommonwealthEngineeringHqs,
    #[serde(rename = "not_allowed")]
    NotAllowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstructionTerrain {
    Clear,
    SandGravel,
    SaltMarsh,
    Delta,
    MajorCity,
    Desert,
    VillageTown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum WorkFootnote {
    #[serde(rename = "dagger_enemy_zoc")]
    EnemyZoc,
    #[serde(rename = "star_hot_weather_water")]
    HotWeatherWater,
    #[serde(rename = "double_dagger_one_at_a_time")]
    OneAtATime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortCategory {
    Tobruk,
    Other,
}

/// Whole supply points, with per-hex stores kept separate from a fixed site cost.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkSupplies {
    pub stores: i32,
    pub stores_per_hex: i32,
    pub fuel: i32,
    pub ammo: i32,
}
impl WorkSupplies {
    fn validate(&self, raw: &RawTable, field: &str) -> Result<(), TableError> {
        for (name, value) in [
            ("stores", self.stores),
            ("stores_per_hex", self.stores_per_hex),
            ("fuel", self.fuel),
            ("ammo", self.ammo),
        ] {
            if value < 0 {
                return Err(raw.err(format!("{field}.{name}"), "must be nonnegative"));
            }
        }
        if self.stores > 0 && self.stores_per_hex > 0 {
            return Err(raw.err(field, "fixed and per-hex stores are mutually exclusive"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConstructionRow {
    pub item: WorkItem,
    pub situation: WorkSituation,
    #[serde(default)]
    pub footnotes: Vec<WorkFootnote>,
    #[serde(default)]
    pub units: Vec<Vec<CrewRequirement>>,
    #[serde(default)]
    pub units_a: Vec<Vec<CrewRequirement>>,
    #[serde(default)]
    pub units_b: Vec<Vec<CrewRequirement>>,
    pub supplies: Option<WorkSupplies>,
    #[serde(default)]
    pub supplies_by_port: BTreeMap<PortCategory, WorkSupplies>,
    pub op_stages: Option<i32>,
    pub cp_cost: Option<i32>,
    #[serde(default)]
    pub terrain_allowed: Vec<ConstructionTerrain>,
    #[serde(default)]
    pub terrain_forbidden: Vec<ConstructionTerrain>,
    #[serde(default)]
    pub restrictions: Vec<String>,
}

#[derive(Deserialize)]
struct Footnote {
    id: WorkFootnote,
    text: String,
}
#[derive(Deserialize)]
struct ConstructionBody {
    row: Vec<ConstructionRow>,
    footnote: Vec<Footnote>,
}
#[derive(Debug, Clone)]
pub struct ConstructionChart {
    rows: BTreeMap<(WorkItem, WorkSituation), ConstructionRow>,
}

impl Bound for ConstructionChart {
    const ID: &'static str = "land.24.17.construction";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: ConstructionBody = raw.deserialize()?;
        let mut notes = BTreeSet::new();
        for (i, note) in body.footnote.iter().enumerate() {
            if !notes.insert(note.id) || note.text.trim().is_empty() {
                return Err(raw.err(
                    format!("footnote[{i}]"),
                    "unique nonempty footnote required",
                ));
            }
        }
        let mut rows = BTreeMap::new();
        for (i, row) in body.row.into_iter().enumerate() {
            let field = format!("row[{i}]");
            let standard = !row.units.is_empty();
            let road_options = !row.units_a.is_empty() && !row.units_b.is_empty();
            if standard == road_options
                || (!road_options && (!row.units_a.is_empty() || !row.units_b.is_empty()))
            {
                return Err(raw.err(
                    format!("{field}.units"),
                    "use joint crew alternatives or both road options",
                ));
            }
            for group in row.units.iter().chain(&row.units_a).chain(&row.units_b) {
                let distinct: BTreeSet<_> = group.iter().collect();
                if group.is_empty()
                    || distinct.len() != group.len()
                    || group.contains(&CrewRequirement::NotAllowed)
                {
                    return Err(raw.err(
                        format!("{field}.units"),
                        "nonempty distinct construction crew required",
                    ));
                }
            }
            if row.op_stages.is_some() == row.cp_cost.is_some()
                || row.op_stages.is_some_and(|n| n <= 0)
                || row.cp_cost.is_some_and(|n| n <= 0)
            {
                return Err(raw.err(
                    format!("{field}.op_stages"),
                    "exactly one positive stage or CP duration required",
                ));
            }
            if row.supplies.is_some() != row.supplies_by_port.is_empty() {
                return Err(raw.err(
                    format!("{field}.supplies"),
                    "one fixed or per-port supply specification required",
                ));
            }
            if let Some(supplies) = &row.supplies {
                supplies.validate(raw, &format!("{field}.supplies"))?;
            } else {
                for (port, supplies) in &row.supplies_by_port {
                    supplies.validate(raw, &format!("{field}.supplies_by_port.{port:?}"))?;
                }
                if row.supplies_by_port.len() != 2 {
                    return Err(raw.err(
                        format!("{field}.supplies_by_port"),
                        "Tobruk and other-port costs required",
                    ));
                }
            }
            let footnotes: BTreeSet<_> = row.footnotes.iter().copied().collect();
            if footnotes.len() != row.footnotes.len() || !footnotes.is_subset(&notes) {
                return Err(raw.err(
                    format!("{field}.footnotes"),
                    "unique declared footnote ids required",
                ));
            }
            let allowed: BTreeSet<_> = row.terrain_allowed.iter().copied().collect();
            let forbidden: BTreeSet<_> = row.terrain_forbidden.iter().copied().collect();
            if allowed.len() != row.terrain_allowed.len()
                || forbidden.len() != row.terrain_forbidden.len()
                || (!allowed.is_empty() && !forbidden.is_empty())
            {
                return Err(raw.err(
                    format!("{field}.terrain"),
                    "use one distinct allowed or forbidden terrain list",
                ));
            }
            if rows.insert((row.item, row.situation), row).is_some() {
                return Err(raw.err(field, "duplicate item and situation"));
            }
        }
        let chart = Self { rows };
        let expected = construction_keys();
        if chart.rows.keys().copied().collect::<BTreeSet<_>>() != expected {
            return Err(raw.err("row", "all construction item/situation pairs required"));
        }
        Ok(chart)
    }
}

fn construction_keys() -> BTreeSet<(WorkItem, WorkSituation)> {
    use WorkItem as I;
    use WorkSituation as S;
    BTreeSet::from([
        (I::FortificationLevel, S::BuildOrRebuild),
        (I::RealMinefield, S::Build),
        (I::FakeMinefield, S::Build),
        (I::Railroad, S::Build),
        (I::Railroad, S::Rebuild),
        (I::Road, S::BuildOrRebuild),
        (I::TemporaryRepairFacility, S::Build),
        (I::RepairFacility, S::Rebuild1Level),
        (I::WaterPipeline, S::BuildOrRebuild),
        (I::Airfield, S::Build),
        (
            I::AirfieldOrAirLandingStrip,
            S::Rebuild1LevelAirfieldOrBuildStrip,
        ),
        (I::FlyingBoatBasin, S::Build),
        (
            I::FlyingBoatBasinOrAlightingArea,
            S::Rebuild1LevelBasinOrBuildAlightingArea,
        ),
        (I::Port, S::Block1Level),
        (I::RealSupplyDump, S::Build),
        (I::FakeSupplyDump, S::Build),
    ])
}
impl ConstructionChart {
    /// Chart rows carry costs, joint/alternative builders and terrain restrictions without adjudication.
    /// Cases: land:24.17
    /// Interpretations: interp:land-0018
    pub fn row(&self, item: WorkItem, situation: WorkSituation) -> Option<&ConstructionRow> {
        self.rows.get(&(item, situation))
    }
    /// Cases: land:24.17
    pub fn rows(&self) -> impl Iterator<Item = &ConstructionRow> {
        self.rows.values()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemolitionRow {
    pub item: WorkItem,
    pub situation: WorkSituation,
    pub units: Vec<CrewRequirement>,
    pub op_stages: Option<i32>,
    pub op_stages_note: Option<String>,
    pub cp_note: Option<String>,
    pub supplies: Option<WorkSupplies>,
    pub recovered_supplies: Option<WorkSupplies>,
    #[serde(default)]
    pub restrictions: Vec<String>,
}
#[derive(Deserialize)]
struct DemolitionBody {
    row: Vec<DemolitionRow>,
}
#[derive(Debug, Clone)]
pub struct DemolitionChart {
    rows: BTreeMap<(WorkItem, WorkSituation), DemolitionRow>,
}
impl Bound for DemolitionChart {
    const ID: &'static str = "land.24.18.demolition";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: DemolitionBody = raw.deserialize()?;
        let mut rows = BTreeMap::new();
        for (i, row) in body.row.into_iter().enumerate() {
            let field = format!("row[{i}]");
            let distinct: BTreeSet<_> = row.units.iter().collect();
            if row.units.is_empty()
                || distinct.len() != row.units.len()
                || (row.units.contains(&CrewRequirement::NotAllowed) && row.units.len() != 1)
            {
                return Err(raw.err(
                    format!("{field}.units"),
                    "distinct alternatives or not_allowed alone required",
                ));
            }
            let durations = usize::from(row.op_stages.is_some())
                + usize::from(row.op_stages_note.is_some())
                + usize::from(row.cp_note.is_some());
            if durations > 1
                || (durations == 0 && !row.units.contains(&CrewRequirement::NotAllowed))
                || row.op_stages.is_some_and(|n| n <= 0)
                || row
                    .op_stages_note
                    .as_ref()
                    .is_some_and(|n| n.trim().is_empty())
                || row.cp_note.as_ref().is_some_and(|n| n.trim().is_empty())
            {
                return Err(raw.err(
                    format!("{field}.op_stages"),
                    "one positive duration or explanatory duration required for unit work",
                ));
            }
            for (name, supplies) in [
                ("supplies", &row.supplies),
                ("recovered_supplies", &row.recovered_supplies),
            ] {
                if let Some(supplies) = supplies {
                    supplies.validate(raw, &format!("{field}.{name}"))?;
                }
            }
            if rows.insert((row.item, row.situation), row).is_some() {
                return Err(raw.err(field, "duplicate item and situation"));
            }
        }
        use WorkItem as I;
        use WorkSituation as S;
        let expected = BTreeSet::from([
            (I::Fortification, S::Reduce1Level),
            (I::FakeMinefield, S::Clear),
            (I::RealMinefield, S::Clear),
            (I::Railroad, S::Destroy),
            (I::Road, S::Destroy),
            (I::RepairFacility, S::Dismantle),
            (I::WaterPipeline, S::Destroy),
            (I::AirFacility, S::Reduce1Level),
            (I::PortOfTobruk, S::Unblock1Level),
            (I::PortOfBenghazi, S::Unblock1Level),
            (I::PortOther, S::Unblock1Level),
            (I::FakeSupplyDump, S::Destroy),
            (I::RealSupplyDump, S::Blow),
        ]);
        if rows.keys().copied().collect::<BTreeSet<_>>() != expected {
            return Err(raw.err("row", "all demolition item/situation pairs required"));
        }
        Ok(Self { rows })
    }
}
impl DemolitionChart {
    /// Cases: land:24.18
    pub fn row(&self, item: WorkItem, situation: WorkSituation) -> Option<&DemolitionRow> {
        self.rows.get(&(item, situation))
    }
    /// Cases: land:24.18
    pub fn rows(&self) -> impl Iterator<Item = &DemolitionRow> {
        self.rows.values()
    }
}
