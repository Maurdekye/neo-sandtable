//! Training durations for replacements and untrained Commonwealth morale.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingSubject {
    InfantryExceptCommando,
    TankOrRecce,
    Gun,
    Commando,
    CommonwealthUnitMoralePoint,
}

impl TrainingSubject {
    pub const ALL: [Self; 5] = [
        Self::InfantryExceptCommando,
        Self::TankOrRecce,
        Self::Gun,
        Self::Commando,
        Self::CommonwealthUnitMoralePoint,
    ];

    /// The chart restricts Commando replacement instruction to its assigned unit.
    /// Cases: land:17.6
    pub fn requires_assigned_unit(self) -> bool {
        self == Self::Commando
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    #[serde(rename = "type")]
    subject: TrainingSubject,
    op_stages: i32,
}

#[derive(Deserialize)]
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
pub struct TrainingChart {
    stages: BTreeMap<TrainingSubject, i32>,
}

impl Bound for TrainingChart {
    const ID: &'static str = "land.17.6.training_chart";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: Body = raw.deserialize()?;
        if body.footnote.len() != 1
            || body.footnote[0].id != "training_note"
            || body.footnote[0].text.trim().is_empty()
        {
            return Err(raw.err("footnote", "one identified training note is required"));
        }
        let mut stages = BTreeMap::new();
        for (i, row) in body.row.into_iter().enumerate() {
            if row.op_stages <= 0 {
                return Err(raw.err(format!("row[{i}].op_stages"), "must be positive"));
            }
            if stages.insert(row.subject, row.op_stages).is_some() {
                return Err(raw.err(format!("row[{i}].type"), "duplicate training subject"));
            }
        }
        if TrainingSubject::ALL.iter().any(|s| !stages.contains_key(s)) {
            return Err(raw.err("row", "each of the five training subjects is required"));
        }
        Ok(Self { stages })
    }
}

impl TrainingChart {
    /// Required Operations Stages for the named replacement or one morale point.
    /// Training eligibility and stage accumulation remain procedure responsibilities.
    /// Cases: land:17.34, land:17.6
    pub fn op_stages(&self, subject: TrainingSubject) -> i32 {
        self.stages[&subject]
    }
}
