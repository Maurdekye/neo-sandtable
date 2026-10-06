//! Capability Point prices from the section 6 summary.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::units::Ratio;
use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpAction {
    DetachUnit,
    AttachAssignedUnit,
    AttachUnassignedUnit,
    AbsorbTwoToeReplacementPoints,
    EnterNonMinefieldHex,
    EnterFriendlyMinefieldHex,
    EnterEnemyMinefieldHex,
    BreakContact,
    Disengage,
    BePlacedInReserve,
    PhasingUnit,
    NonPhasingUnit,
    Patrol,
    DesertRaiderRaid,
    AttemptToPoisonWaterSource,
    AttemptToSweetenPoisonedWaterSource,
    AttemptToBlowSupplyDump,
    DrawWaterLoadUnloadInOrganizationPhase,
    DrawWaterOtherThanOrganizationPhase,
    LoadUnloadOtherThanOrganizationPhase,
    ConstructSupplyDump,
    ConstructDemolishOtherItems,
    RailOrInterPortTransportOfTroops,
    TransportOfTroopsByAir,
    ParadropParatroops,
    ParadropCommandos,
    CommandoAmphibiousLanding,
    ReadyAirplanes,
}

/// A price before the engine supplies CPA, terrain cost, rounding and action restrictions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpCost {
    Flat(i32),
    Terrain,
    FlatPlusTerrain(i32),
    WholeCpaPlusTerrain,
    CpaFraction(Ratio),
    /// Printed alternatives; use the named dump/landing methods to select one.
    Alternatives {
        cp: [i32; 2],
        plus_terrain: bool,
    },
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawCost {
    cp: Option<i32>,
    #[serde(default)]
    tec: bool,
    #[serde(default)]
    plus_tec: bool,
    #[serde(default)]
    cpa_plus_tec: bool,
    cpa_fraction: Option<[i32; 2]>,
    cp_pair: Option<[i32; 2]>,
}

impl RawCost {
    fn bind(self, raw: &RawTable, field: &str) -> Result<CpCost, TableError> {
        let shapes = usize::from(self.cp.is_some())
            + usize::from(self.tec)
            + usize::from(self.cpa_plus_tec)
            + usize::from(self.cpa_fraction.is_some())
            + usize::from(self.cp_pair.is_some());
        if shapes != 1 || (self.plus_tec && self.cp.is_none() && self.cp_pair.is_none()) {
            return Err(raw.err(
                field,
                "exactly one price shape is required; plus_tec needs cp or cp_pair",
            ));
        }
        if let Some(cp) = self.cp {
            if cp < 0 {
                return Err(raw.err(format!("{field}.cp"), "must be nonnegative"));
            }
            return Ok(if self.plus_tec {
                CpCost::FlatPlusTerrain(cp)
            } else {
                CpCost::Flat(cp)
            });
        }
        if self.tec {
            return Ok(CpCost::Terrain);
        }
        if self.cpa_plus_tec {
            return Ok(CpCost::WholeCpaPlusTerrain);
        }
        if let Some([num, den]) = self.cpa_fraction {
            if num <= 0 || den <= 0 || num > den {
                return Err(raw.err(
                    format!("{field}.cpa_fraction"),
                    "must be a positive fraction at most one",
                ));
            }
            return Ok(CpCost::CpaFraction(Ratio { num, den }));
        }
        let cp = self.cp_pair.expect("validated shape");
        if cp.iter().any(|n| *n < 0) {
            return Err(raw.err(format!("{field}.cp_pair"), "must be nonnegative"));
        }
        Ok(CpCost::Alternatives {
            cp,
            plus_terrain: self.plus_tec,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RawSub {
    who: String,
    cost: RawCost,
}
#[derive(Debug, Clone, Deserialize)]
struct RawRow {
    id: CpAction,
    cost: Option<RawCost>,
    #[serde(default)]
    sub: Vec<RawSub>,
}
#[derive(Deserialize)]
struct Body {
    row: Vec<RawRow>,
}

/// Situations printed under the minefield and combat group rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Subject {
    WithEngineer,
    NonMotorizedWithEngineer,
    MotorizedWithEngineer,
    NonMotorizedWithoutEngineer,
    MotorizedWithoutEngineer,
    BarrageOrAssault,
    UndergoBarrage,
    Probe,
    DefendOrBarrage,
    DefendProbe,
}

impl Subject {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "any unit with an Eng unit" => Self::WithEngineer,
            "non-motorized unit with an Eng unit" => Self::NonMotorizedWithEngineer,
            "motorized unit with an Eng unit" => Self::MotorizedWithEngineer,
            "non-motorized unit, no Eng unit" => Self::NonMotorizedWithoutEngineer,
            "motorized unit, no Eng unit" => Self::MotorizedWithoutEngineer,
            "Barrage and/or an Assault other than a Probe" => Self::BarrageOrAssault,
            "Undergo a Barrage" => Self::UndergoBarrage,
            "Probe" => Self::Probe,
            "Barrage and/or undergo a Barrage and/or defend against an Assault other than a Probe" => {
                Self::DefendOrBarrage
            }
            "Defend against a Probe" => Self::DefendProbe,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombatActivity {
    BarrageOrAssault,
    UndergoBarrage,
    Probe,
}

#[derive(Debug, Clone)]
pub struct CapabilityExpenditure {
    direct: BTreeMap<CpAction, CpCost>,
    grouped: BTreeMap<(CpAction, Subject), CpCost>,
}

impl Bound for CapabilityExpenditure {
    const ID: &'static str = "land.6.3.capability_point_expenditure_summary";
    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let body: Body = raw.deserialize()?;
        let mut table = Self {
            direct: BTreeMap::new(),
            grouped: BTreeMap::new(),
        };
        let mut ids = std::collections::BTreeSet::new();
        for (i, row) in body.row.into_iter().enumerate() {
            if !ids.insert(row.id) {
                return Err(raw.err(format!("row[{i}].id"), "duplicate action"));
            }
            let subjects: &[Subject] = match row.id {
                CpAction::EnterFriendlyMinefieldHex => &[
                    Subject::WithEngineer,
                    Subject::NonMotorizedWithoutEngineer,
                    Subject::MotorizedWithoutEngineer,
                ],
                CpAction::EnterEnemyMinefieldHex => &[
                    Subject::NonMotorizedWithEngineer,
                    Subject::MotorizedWithEngineer,
                    Subject::NonMotorizedWithoutEngineer,
                    Subject::MotorizedWithoutEngineer,
                ],
                CpAction::PhasingUnit => &[
                    Subject::BarrageOrAssault,
                    Subject::UndergoBarrage,
                    Subject::Probe,
                ],
                CpAction::NonPhasingUnit => &[Subject::DefendOrBarrage, Subject::DefendProbe],
                _ => &[],
            };
            if subjects.is_empty() {
                if !row.sub.is_empty() {
                    return Err(raw.err(format!("row[{i}].sub"), "this action needs a direct cost"));
                }
                let price = row
                    .cost
                    .ok_or_else(|| raw.err(format!("row[{i}].cost"), "missing price"))?
                    .bind(raw, &format!("row[{i}].cost"))?;
                let valid = match row.id {
                    CpAction::ConstructSupplyDump => matches!(
                        price,
                        CpCost::Alternatives {
                            plus_terrain: false,
                            ..
                        }
                    ),
                    CpAction::CommandoAmphibiousLanding => matches!(
                        price,
                        CpCost::Alternatives {
                            plus_terrain: true,
                            ..
                        }
                    ),
                    CpAction::EnterNonMinefieldHex => matches!(price, CpCost::Terrain),
                    CpAction::AttemptToBlowSupplyDump => matches!(price, CpCost::CpaFraction(_)),
                    _ => matches!(price, CpCost::Flat(_)),
                };
                if !valid {
                    return Err(raw.err(
                        format!("row[{i}].cost"),
                        "price shape disagrees with action",
                    ));
                }
                table.direct.insert(row.id, price);
            } else {
                if row.cost.is_some() || row.sub.len() != subjects.len() {
                    return Err(raw.err(
                        format!("row[{i}].sub"),
                        "this grouped action needs each subject once and no direct cost",
                    ));
                }
                for (j, sub) in row.sub.into_iter().enumerate() {
                    let subject = Subject::parse(&sub.who)
                        .filter(|s| subjects.contains(s))
                        .ok_or_else(|| {
                            raw.err(
                                format!("row[{i}].sub[{j}].who"),
                                "unknown subject for this action",
                            )
                        })?;
                    let price = sub.cost.bind(raw, &format!("row[{i}].sub[{j}].cost"))?;
                    let valid = match row.id {
                        CpAction::EnterFriendlyMinefieldHex => {
                            matches!(price, CpCost::FlatPlusTerrain(_))
                        }
                        CpAction::EnterEnemyMinefieldHex
                            if subject == Subject::MotorizedWithoutEngineer =>
                        {
                            matches!(price, CpCost::WholeCpaPlusTerrain)
                        }
                        CpAction::EnterEnemyMinefieldHex => {
                            matches!(price, CpCost::FlatPlusTerrain(_))
                        }
                        _ => matches!(price, CpCost::Flat(_)),
                    };
                    if !valid {
                        return Err(raw.err(
                            format!("row[{i}].sub[{j}].cost"),
                            "price shape disagrees with subject",
                        ));
                    }
                    if table.grouped.insert((row.id, subject), price).is_some() {
                        return Err(raw.err(format!("row[{i}].sub[{j}].who"), "duplicate subject"));
                    }
                }
            }
        }
        if ids.len() != 28 {
            return Err(raw.err("row.id", "all 28 summary actions are required"));
        }
        Ok(table)
    }
}

impl CapabilityExpenditure {
    /// Direct chart row; grouped actions are read with minefield_cost or combat_cost.
    /// Cases: land:6.3
    pub fn cost(&self, action: CpAction) -> Option<CpCost> {
        self.direct.get(&action).copied()
    }

    /// Cases: land:6.3, land:26.21, land:26.23
    /// Interpretations: interp:land-0015
    pub fn minefield_cost(
        &self,
        friendly: bool,
        motorized: bool,
        engineer_present: bool,
    ) -> CpCost {
        let action = if friendly {
            CpAction::EnterFriendlyMinefieldHex
        } else {
            CpAction::EnterEnemyMinefieldHex
        };
        let subject = match (friendly, motorized, engineer_present) {
            (true, _, true) => Subject::WithEngineer,
            (_, false, true) => Subject::NonMotorizedWithEngineer,
            (_, true, true) => Subject::MotorizedWithEngineer,
            (_, false, false) => Subject::NonMotorizedWithoutEngineer,
            (_, true, false) => Subject::MotorizedWithoutEngineer,
        };
        self.grouped[&(action, subject)]
    }

    /// Cases: land:6.3
    pub fn combat_cost(&self, phasing: bool, activity: CombatActivity) -> i32 {
        let action = if phasing {
            CpAction::PhasingUnit
        } else {
            CpAction::NonPhasingUnit
        };
        let subject = match (phasing, activity) {
            (true, CombatActivity::BarrageOrAssault) => Subject::BarrageOrAssault,
            (true, CombatActivity::UndergoBarrage) => Subject::UndergoBarrage,
            (true, CombatActivity::Probe) => Subject::Probe,
            (false, CombatActivity::Probe) => Subject::DefendProbe,
            (false, _) => Subject::DefendOrBarrage,
        };
        match self.grouped[&(action, subject)] {
            CpCost::Flat(cp) => cp,
            _ => unreachable!("validated combat cost"),
        }
    }

    /// Selects the printed 3/2 alternatives by dump kind.
    /// Cases: land:6.3, land:24.17
    /// Interpretations: interp:land-0006, interp:land-0018
    pub fn supply_dump_cost(&self, real: bool) -> i32 {
        match self.direct[&CpAction::ConstructSupplyDump] {
            CpCost::Alternatives { cp, .. } => cp[usize::from(!real)],
            _ => unreachable!("validated alternatives"),
        }
    }

    /// Ship movement must be within its 100-hex stage limit. Terrain cost is still to be added.
    /// Cases: land:6.3, land:27.73, land:30.15
    /// Interpretations: interp:land-0006
    pub fn commando_landing_cost(&self, ship_hexes_this_stage: i32) -> Option<CpCost> {
        if !(0..=100).contains(&ship_hexes_this_stage) {
            return None;
        }
        match self.direct[&CpAction::CommandoAmphibiousLanding] {
            CpCost::Alternatives { cp, .. } => Some(CpCost::FlatPlusTerrain(
                cp[usize::from(ship_hexes_this_stage > 50)],
            )),
            _ => unreachable!("validated alternatives"),
        }
    }

    /// CP returned after an unfavorable defense, if the unit neither fired nor suffered barrage.
    /// Cases: land:6.3
    pub fn defense_refund(
        &self,
        final_differential: i32,
        fired_barrage: bool,
        suffered_barrage: bool,
    ) -> i32 {
        if final_differential <= -4 && !fired_barrage && !suffered_barrage {
            2
        } else {
            0
        }
    }
}
