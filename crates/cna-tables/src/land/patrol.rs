//! Patrol chart results. Eligibility, casualty allocation and disclosure stay with procedures.

use serde::Deserialize;

use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct PatrolLosses {
    pub killed: i32,
    pub captured: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LossRow {
    die_min: i32,
    die_max: i32,
    killed: i32,
    captured: i32,
    #[serde(default)]
    footnotes: Vec<String>,
}

#[derive(Deserialize)]
struct Footnote {
    id: String,
    text: String,
}

#[derive(Deserialize)]
struct LossBody {
    row: Vec<LossRow>,
    footnote: Vec<Footnote>,
}

fn losses(raw: &RawTable, minimum: i32, note: &str) -> Result<Vec<PatrolLosses>, TableError> {
    let body: LossBody = raw.deserialize()?;
    if body.footnote.len() != 1
        || body.footnote[0].id != note
        || body.footnote[0].text.trim().is_empty()
    {
        return Err(raw.err(
            "footnote",
            format!("one identified {note} note is required"),
        ));
    }
    let mut values = vec![None; (7 - minimum) as usize];
    for (i, row) in body.row.into_iter().enumerate() {
        if row.die_min < minimum || row.die_max > 6 || row.die_min > row.die_max {
            return Err(raw.err(
                format!("row[{i}].die_min/die_max"),
                "range outside chart domain",
            ));
        }
        if !(0..=1).contains(&row.killed) || !(0..=1).contains(&row.captured) {
            return Err(raw.err(
                format!("row[{i}].killed/captured"),
                "each loss must be zero or one",
            ));
        }
        let expected_notes = if note == "patrol_eliminated" && row.captured == 1 {
            vec![note]
        } else {
            vec![]
        };
        if row.footnotes != expected_notes {
            return Err(raw.err(
                format!("row[{i}].footnotes"),
                "only objective capture carries the elimination note",
            ));
        }
        for die in row.die_min..=row.die_max {
            let slot = &mut values[(die - minimum) as usize];
            if slot
                .replace(PatrolLosses {
                    killed: row.killed,
                    captured: row.captured,
                })
                .is_some()
            {
                return Err(raw.err(
                    format!("row[{i}].die_min/die_max"),
                    "overlapping die ranges",
                ));
            }
        }
    }
    values
        .into_iter()
        .map(|value| value.ok_or_else(|| raw.err("row", "missing die result")))
        .collect()
}

#[derive(Debug, Clone)]
pub struct PatrolSurvival {
    values: Vec<PatrolLosses>,
}

impl Bound for PatrolSurvival {
    const ID: &'static str = "land.16.6.patrol_survival";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        Ok(Self {
            values: losses(raw, 0, "recce_modifier")?,
        })
    }
}

impl PatrolSurvival {
    /// Losses after the all-Recce minus-one modifier; the input is the unmodified die.
    /// A procedure decides whether the survival roll is required at all.
    /// Cases: land:16.32, land:16.33, land:16.6
    pub fn losses(&self, die: i32, all_recce: bool) -> Option<PatrolLosses> {
        if !(1..=6).contains(&die) {
            return None;
        }
        Some(self.values[(die - i32::from(all_recce)) as usize])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnaissanceResult {
    Units(i32),
    All,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ReconCell {
    Units(i32),
    Word(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReconColumns {
    net_points_1: ReconCell,
    net_points_2: ReconCell,
    net_points_3: ReconCell,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReconRow {
    die: i32,
    units_revealed: ReconColumns,
}

#[derive(Deserialize)]
struct ReconBody {
    row: Vec<ReconRow>,
}

#[derive(Debug, Clone)]
pub struct PatrolReconnaissance {
    values: Vec<[ReconnaissanceResult; 3]>,
}

impl Bound for PatrolReconnaissance {
    const ID: &'static str = "land.16.7.patrol_recon";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: ReconBody = raw.deserialize()?;
        let mut values = vec![None; 6];
        for (i, row) in body.row.into_iter().enumerate() {
            if !(1..=6).contains(&row.die) {
                return Err(raw.err(format!("row[{i}].die"), "must be within 1-6"));
            }
            let mut results = [ReconnaissanceResult::Units(0); 3];
            for (column, cell) in [
                row.units_revealed.net_points_1,
                row.units_revealed.net_points_2,
                row.units_revealed.net_points_3,
            ]
            .into_iter()
            .enumerate()
            {
                results[column] = match cell {
                    ReconCell::Units(n) if (0..=4).contains(&n) => ReconnaissanceResult::Units(n),
                    ReconCell::Word(word) if word == "all" => ReconnaissanceResult::All,
                    _ => {
                        return Err(raw.err(
                            format!("row[{i}].units_revealed.net_points_{}", column + 1),
                            "must be a count within 0-4 or all",
                        ));
                    }
                };
            }
            if values[(row.die - 1) as usize].replace(results).is_some() {
                return Err(raw.err(format!("row[{i}].die"), "duplicate die result"));
            }
        }
        Ok(Self {
            values: values
                .into_iter()
                .map(|value| {
                    value.ok_or_else(|| raw.err("row", "each die result within 1-6 is required"))
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl PatrolReconnaissance {
    /// Battalion-equivalent units to report for the surviving patrol points.
    /// Selection and the permitted information remain governed by the patrol procedure.
    /// Cases: land:16.5, land:16.7
    pub fn revealed(&self, die: i32, net_points: i32) -> Option<ReconnaissanceResult> {
        if !(1..=6).contains(&die) || !(1..=3).contains(&net_points) {
            return None;
        }
        Some(self.values[(die - 1) as usize][(net_points - 1) as usize])
    }
}

#[derive(Debug, Clone)]
pub struct ObjectiveLoss {
    values: Vec<PatrolLosses>,
}

impl Bound for ObjectiveLoss {
    const ID: &'static str = "land.16.8.objective_loss";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        Ok(Self {
            values: losses(raw, 1, "patrol_eliminated")?,
        })
    }
}

impl ObjectiveLoss {
    /// Losses in the objective hex; an eliminated patrol converts capture to death.
    /// Cases: land:16.34, land:16.8
    pub fn losses(&self, die: i32, patrol_eliminated: bool) -> Option<PatrolLosses> {
        if !(1..=6).contains(&die) {
            return None;
        }
        let mut result = self.values[(die - 1) as usize];
        if patrol_eliminated {
            result.killed += result.captured;
            result.captured = 0;
        }
        Some(result)
    }
}
