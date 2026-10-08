//! Printed attachment allowances, not assignment or reorganization procedures.
use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::ranges::IntRange;
use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentNation {
    Allied,
    German,
    Italian,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentParent {
    ArmorDivision,
    InfantryDivision,
    TankBrigade,
    OtherBrigade,
    AnyBattalion,
    MatruhGarrison,
    SelbyForce,
    InfantryOrArmorRegiment,
    BattleGroup,
    ArtilleryBrigadeHq,
    ArmorDivisionOrTankGroup,
    BrigadeOrRegiment,
}

/// Missing maxima are not zero restrictions. A procedure chooses one whole alternative.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentAllowance {
    pub max_units: Option<i32>,
    pub max_brigades: Option<i32>,
    pub max_units_besides_brigade: Option<i32>,
    pub max_companies: Option<i32>,
    pub max_artillery_units: Option<i32>,
    pub max_infantry: Option<i32>,
    pub max_tank: Option<i32>,
    pub max_recce: Option<i32>,
    pub max_gun_class: Option<i32>,
    #[serde(default)]
    pub infantry_and_or_tank_one_each: bool,
    #[serde(default)]
    pub one_infantry_or_one_tank: bool,
}
impl AttachmentAllowance {
    fn valid(&self) -> bool {
        let numbers = [
            self.max_units,
            self.max_brigades,
            self.max_units_besides_brigade,
            self.max_companies,
            self.max_artillery_units,
            self.max_infantry,
            self.max_tank,
            self.max_recce,
            self.max_gun_class,
        ];
        if numbers.into_iter().flatten().any(|n| n < 0) {
            return false;
        }
        let counts = [
            self.max_units,
            self.max_brigades,
            self.max_companies,
            self.max_artillery_units,
        ];
        if counts.into_iter().flatten().count() != 1
            || (self.max_units_besides_brigade.is_some() && self.max_brigades.is_none())
        {
            return false;
        }
        let restrictions = [
            self.max_infantry,
            self.max_tank,
            self.max_recce,
            self.max_gun_class,
        ];
        if (self.max_companies.is_some() || self.max_artillery_units.is_some())
            && restrictions.into_iter().any(|n| n.is_some())
        {
            return false;
        }
        if let Some(total) = self.max_units
            && restrictions.into_iter().flatten().any(|n| n > total)
        {
            return false;
        }
        if self.infantry_and_or_tank_one_each {
            return !self.one_infantry_or_one_tank
                && self.max_units == Some(3)
                && self.max_infantry == Some(1)
                && self.max_tank == Some(1);
        }
        if self.one_infantry_or_one_tank {
            return self.max_units == Some(2) && restrictions.into_iter().all(|n| n.is_none());
        }
        true
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    nation: AttachmentNation,
    parent: AttachmentParent,
    game_turn_range: Option<IntRange>,
    options: Vec<AttachmentAllowance>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Footnote {
    id: String,
    text: String,
}
#[derive(Deserialize)]
struct Body {
    row: Vec<Row>,
    footnote: Vec<Footnote>,
}
#[derive(Debug, Clone)]
struct Allowances {
    range: Option<IntRange>,
    options: Vec<AttachmentAllowance>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompanyAttachmentBonus {
    pub extra_company_equivalents: i32,
    pub company_equivalents_per_battalion: i32,
    pub non_shell_only: bool,
    pub type_restrictions_apply: bool,
}
#[derive(Debug, Clone)]
pub struct MaximumAttachment {
    rows: BTreeMap<(AttachmentNation, AttachmentParent), Vec<Allowances>>,
}
impl Bound for MaximumAttachment {
    const ID: &'static str = "land.19.5.maximum_attachment";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        use AttachmentNation::*;
        use AttachmentParent::*;
        let body: Body = raw.deserialize()?;
        let expected = BTreeSet::from([
            (Allied, ArmorDivision, Some((1, 67))),
            (Allied, ArmorDivision, Some((68, 111))),
            (Allied, InfantryDivision, None),
            (Allied, TankBrigade, Some((1, 67))),
            (Allied, TankBrigade, Some((68, 111))),
            (Allied, OtherBrigade, None),
            (Allied, AnyBattalion, None),
            (Allied, MatruhGarrison, Some((1, 12))),
            (Allied, SelbyForce, Some((13, 111))),
            (German, ArmorDivision, None),
            (German, InfantryDivision, None),
            (German, InfantryOrArmorRegiment, None),
            (German, BattleGroup, None),
            (German, ArtilleryBrigadeHq, None),
            (German, AnyBattalion, None),
            (Italian, ArmorDivisionOrTankGroup, None),
            (Italian, InfantryDivision, None),
            (Italian, BrigadeOrRegiment, None),
            (Italian, BattleGroup, None),
            (Italian, AnyBattalion, None),
        ]);
        let mut seen = BTreeSet::new();
        let mut rows = BTreeMap::<_, Vec<Allowances>>::new();
        for (i, row) in body.row.into_iter().enumerate() {
            let key = (
                row.nation,
                row.parent,
                row.game_turn_range.map(|r| (r.lo, r.hi)),
            );
            if !expected.contains(&key) || !seen.insert(key) {
                return Err(raw.err(
                    format!("row[{i}]"),
                    "unique printed nation/parent/date required",
                ));
            }
            let count =
                if row.nation == German && matches!(row.parent, ArmorDivision | InfantryDivision) {
                    2
                } else {
                    1
                };
            if row.options.len() != count
                || row.options.iter().any(|o| !o.valid())
                || (count == 2 && row.options[0] == row.options[1])
            {
                return Err(raw.err(
                    format!("row[{i}].options"),
                    "complete distinct valid alternatives required",
                ));
            }
            let and_or = key == (Allied, ArmorDivision, Some((68, 111)));
            let either_or = key == (Italian, InfantryDivision, None);
            if row.options.iter().any(|o| {
                o.infantry_and_or_tank_one_each != and_or || o.one_infantry_or_one_tank != either_or
            }) {
                return Err(raw.err(
                    format!("row[{i}].options"),
                    "printed mixed-type predicates must match their parent/date row",
                ));
            }
            rows.entry((row.nation, row.parent))
                .or_default()
                .push(Allowances {
                    range: row.game_turn_range,
                    options: row.options,
                });
        }
        if seen != expected {
            return Err(raw.err("row", "all twenty printed attachment rows required"));
        }
        let mut notes = BTreeSet::new();
        for (i, note) in body.footnote.into_iter().enumerate() {
            if note.text.trim().is_empty() || !notes.insert(note.id) {
                return Err(raw.err(
                    format!("footnote[{i}]"),
                    "nonempty unique footnote required",
                ));
            }
        }
        if notes != BTreeSet::from(["definitions".to_owned(), "company_bonus".to_owned()]) {
            return Err(raw.err("footnote", "definitions and company bonus required"));
        }
        Ok(Self { rows })
    }
}
impl MaximumAttachment {
    /// Extra non-assigned units, with full-strength and type definitions from the chart.
    /// Assignment, substitution, nesting, CP and timing remain procedure responsibilities.
    /// Cases: land:19.4, land:19.5
    pub fn allowances(
        &self,
        nation: AttachmentNation,
        parent: AttachmentParent,
        game_turn: i32,
    ) -> Option<&[AttachmentAllowance]> {
        if !(1..=111).contains(&game_turn) {
            return None;
        }
        self.rows
            .get(&(nation, parent))?
            .iter()
            .find(|r| r.range.is_none_or(|band| band.contains(game_turn)))
            .map(|r| r.options.as_slice())
    }
    /// Only real division/brigade parents qualify, including when a chart label also names
    /// a different formation class. A procedure must verify that class and child types.
    /// Cases: land:19.5
    pub fn division_or_brigade_company_bonus(&self) -> CompanyAttachmentBonus {
        CompanyAttachmentBonus {
            extra_company_equivalents: 2,
            company_equivalents_per_battalion: 3,
            non_shell_only: true,
            type_restrictions_apply: true,
        }
    }
}
