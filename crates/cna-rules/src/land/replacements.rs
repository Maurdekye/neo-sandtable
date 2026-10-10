//! Finite determined production, separate from whole-unit schedules and repair lots.
//! Slice (a) owns planning/distribution/arrival only. Training, absorption,
//! self-motion/carriage and Axis shipping remain separately implemented slices.
//! Cases: land:20.21, land:20.23, land:20.61, land:20.66, land:20.72, land:20.74

use crate::logistics::segment::{FuelFundingAccount, FuelTruckKind, SegmentKey};
use crate::state::Location;
use cna_content::scenario::Supplies;
use cna_content::units::Trucks;
use cna_core::engine::EngineError;
use cna_core::ids::{HexId, UnitId};
use cna_core::quantity::{FuelTenths, WaterPoints};
use cna_protocol::Side;
use cna_tables::airlog::trucks::TruckType;
use cna_tables::land::production::{
    AxisProductionNation, AxisReplacementPool, CommonwealthProduction, ProductionMaximum,
    ProductionPeriod,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReplacementState {
    pub sides: Vec<SideReplacements>,
}

/// A live arrival constraint, never reconstructed from mobile pools or counters.
/// Cases: land:20.78
/// Chart clauses: 20.78A (trucks) and 20.78B (infantry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CWArrivalLimit {
    pub game_turn: u16,
    pub total_trucks: i32,
    pub alexandria_trucks: i32,
    pub infantry_city: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideReplacements {
    pub side: Side,
    pub next_serial: u64,
    pub budgets: Vec<ProductionBudget>,
    pub orders: Vec<ScheduledReplacement>,
    pub counters: Vec<ReplacementCounter>,
    /// One live entry per scheduled Commonwealth arrival GT; omission is Unknown.
    /// Cases: land:20.78
    /// Chart clauses: 20.78A (trucks) and 20.78B (infantry).
    #[serde(default)]
    pub cw_arrival_limits: Vec<CWArrivalLimit>,
    pub planning: ReplacementProcedure,
    pub arrival: ReplacementProcedure,
    pub training: ReplacementProcedure,
    pub reorganization: ReplacementProcedure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplacementNation {
    Commonwealth,
    German,
    Italian,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProductionRowRef {
    AxisTruck {
        kind: FuelTruckKind,
    },
    AxisEquipment {
        nation: ReplacementNation,
        item: String,
    },
    CommonwealthTruck {
        column: String,
        kind: FuelTruckKind,
    },
    CommonwealthInfantry {
        column: String,
    },
    CommonwealthEquipment {
        item: String,
    },
}

impl ProductionRowRef {
    pub fn side(&self) -> Side {
        match self {
            Self::AxisTruck { .. } | Self::AxisEquipment { .. } => Side::Axis,
            _ => Side::Commonwealth,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProductionBucket {
    GameTurn {
        gt: u16,
    },
    HalfMonth {
        year: u16,
        month: u8,
        half: CalendarHalf,
    },
    Month {
        year: u16,
        month: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarHalf {
    First,
    Second,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeriodCommitment {
    pub bucket: ProductionBucket,
    pub committed: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductionBudget {
    pub row: ProductionRowRef,
    pub pool_index: u32,
    pub pool_committed: i32,
    pub periods: Vec<PeriodCommitment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OpStageKey {
    pub game_turn: u16,
    pub op_stage: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplacementKind {
    Infantry {
        nation: ReplacementNation,
    },
    Gun {
        nation: ReplacementNation,
        weapon: String,
    },
    Tank {
        nation: ReplacementNation,
        weapon: String,
    },
    Recce {
        nation: ReplacementNation,
        weapon: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProducedKind {
    Trucks { kind: FuelTruckKind },
    Points { kind: ReplacementKind },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledReplacement {
    pub id: String,
    pub row: ProductionRowRef,
    pub kind: ProducedKind,
    pub points: i32,
    pub due: OpStageKey,
    pub lane: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplacementCounter {
    pub id: String,
    pub kind: ReplacementKind,
    pub points: i32,
    pub location: Location,
    pub recipient: UnitId,
    pub training: Training,
    pub carriage: Option<Carriage>,
    pub physical: ReplacementPhysical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Carriage {
    pub pool: String,
    pub trucks: Trucks,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Training {
    Ready,
    Training {
        required_stages: u8,
        completed: u8,
        last_counted: Option<OpStageKey>,
        candidate: Option<TrainingCandidate>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingCandidate {
    pub stage: OpStageKey,
    pub instructor: UnitId,
    pub hex: HexId,
    pub interrupted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplacementPhysical {
    pub stage: OpStageKey,
    pub cp_quarters: i32,
    pub cohesion_quarters: i32,
    pub breakdown_points: i32,
    pub checked_column: Option<u8>,
    pub on_network: bool,
    pub supplies: Supplies,
    pub tank_fuel: FuelTenths,
    pub water: WaterPoints,
    pub activity: Option<ReplacementActivity>,
    pub funding: Option<FuelFundingAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplacementActivity {
    pub stage: OpStageKey,
    pub required: WaterPoints,
    pub paid: WaterPoints,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReplacementProcedure {
    pub key: Option<ProcedureKey>,
    pub phase: ProcedurePhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcedureKey {
    pub anchor: String,
    pub segment: SegmentKey,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ProcedurePhase {
    #[default]
    Idle,
    Awaiting {
        submitted: Option<ReplacementPlan>,
    },
    Distributing {
        determined: Vec<DeterminedProduction>,
        submitted: Option<DistributionPlan>,
    },
    Resolved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeterminedProduction {
    pub row: ProductionRowRef,
    pub kind: ProducedKind,
    pub points: i32,
    pub arrival_turn: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReplacementPlan {
    Schedule(SchedulePlan),
    Distribution(DistributionPlan),
    Arrival(ArrivalPlan),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulePlan {
    pub requests: Vec<ProductionRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductionRequest {
    pub row: ProductionRowRef,
    pub quantity: ProductionQuantity,
    pub arrival_turn: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProductionQuantity {
    Fixed { points: i32 },
    Roll,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionPlan {
    pub batches: Vec<DistributionBatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionBatch {
    pub row: ProductionRowRef,
    pub kind: ProducedKind,
    pub points: i32,
    pub due: OpStageKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrivalPlan {
    pub choices: Vec<ArrivalChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArrivalChoice {
    Points {
        order: String,
        recipient: UnitId,
        location: Location,
    },
    Trucks {
        order: String,
        location: Location,
        pool_target: ProductionPoolTarget,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProductionPoolTarget {
    Create,
    Existing { pool: String },
}

fn invariant(detail: impl Into<String>) -> EngineError {
    EngineError::Invariant {
        detail: format!("replacements: {}", detail.into()),
    }
}

fn gap(case: &str, detail: impl Into<String>) -> EngineError {
    EngineError::Unsupported {
        case: case.into(),
        detail: detail.into(),
    }
}

fn truck(kind: FuelTruckKind) -> TruckType {
    match kind {
        FuelTruckKind::Light => TruckType::Light,
        FuelTruckKind::Medium => TruckType::Medium,
        FuelTruckKind::Heavy => TruckType::Heavy,
    }
}

/// Trusted printed limits, not deserializable actor declarations.
#[derive(Debug, Clone)]
pub(crate) struct FiniteLimit {
    row: ProductionRowRef,
    pool_index: u32,
    pool: i32,
    maximum: i32,
    period: ProductionPeriod,
}

fn numeric_limit(
    row: &ProductionRowRef,
    pool_index: usize,
    pool: i32,
    maximum: ProductionMaximum,
) -> Result<FiniteLimit, EngineError> {
    match maximum {
        ProductionMaximum::Points {
            points,
            period: ProductionPeriod::Unreadable,
        } => Err(gap(
            "land:20.66",
            format!("printed period for maximum {points} is unreadable"),
        )),
        ProductionMaximum::Points { points, period } if pool > 0 && points > 0 => Ok(FiniteLimit {
            row: row.clone(),
            pool_index: pool_index
                .try_into()
                .map_err(|_| invariant("pool index overflow"))?,
            pool,
            maximum: points,
            period,
        }),
        ProductionMaximum::PrintedDash => Err(gap(
            "land:20.78",
            "printed dash has no numeric production maximum",
        )),
        _ => Err(invariant("invalid printed production limits")),
    }
}

/// Axis uses its planning chart date; Commonwealth uses the arrival chart date.
/// The caller separately binds the actual schedule and scenario restrictions.
/// Random rows have no invented finite pool and are not accepted by this resolver.
/// Cases: land:20.61, land:20.66, land:20.72, land:20.78
/// Interpretations: interp:land-0014, interp:airlog-0004
pub(crate) fn finite_limit(
    axis: &AxisReplacementPool,
    commonwealth: &CommonwealthProduction,
    row: &ProductionRowRef,
    planning_gt: u16,
    arrival_gt: u16,
) -> Result<FiniteLimit, EngineError> {
    if let ProductionRowRef::CommonwealthEquipment { item } = row {
        let entry = commonwealth
            .equipment(item)
            .ok_or_else(|| invariant("unknown Commonwealth production row"))?;
        if !entry.dates.contains(arrival_gt.into()) {
            return Err(invariant(
                "Commonwealth row unavailable at arrival chart date",
            ));
        }
        return numeric_limit(row, 0, entry.pool_points, entry.maximum);
    }
    let entry = match row {
        ProductionRowRef::AxisTruck { kind } => axis.truck(truck(*kind)),
        ProductionRowRef::AxisEquipment { nation, item } => {
            let nation = match nation {
                ReplacementNation::German => AxisProductionNation::German,
                ReplacementNation::Italian => AxisProductionNation::Italian,
                ReplacementNation::Commonwealth => {
                    return Err(invariant("Commonwealth is not an Axis chart nation"));
                }
            };
            axis.equipment(nation, item)
                .ok_or_else(|| invariant("unknown Axis production row"))?
        }
        _ => return Err(invariant("random production has no finite printed pool")),
    };
    let line = entry
        .availability_at(planning_gt.into())
        .ok_or_else(|| invariant("Axis row unavailable at planning chart date"))?;
    if line.french_tunis_exception {
        return Err(gap(
            "land:20.66",
            "French Tunis call-up is not ordinary delayed shipping",
        ));
    }
    let pool = *entry
        .pools
        .get(line.pool_index)
        .ok_or_else(|| invariant("printed pool index absent"))?;
    numeric_limit(row, line.pool_index, pool, line.maximum)
}

impl SideReplacements {
    pub(crate) fn new(side: Side) -> Self {
        Self {
            side,
            next_serial: 0,
            budgets: vec![],
            orders: vec![],
            counters: vec![],
            cw_arrival_limits: vec![],
            planning: ReplacementProcedure::default(),
            arrival: ReplacementProcedure::default(),
            training: ReplacementProcedure::default(),
            reorganization: ReplacementProcedure::default(),
        }
    }

    /// A determined order's budget debit never relies on shipping success.
    /// Validate BOTH caps before publishing any change. No refund API exists.
    /// Cases: land:20.61, land:20.63, land:20.73
    pub(crate) fn commit_budget(
        &mut self,
        row: &ProductionRowRef,
        limit: &FiniteLimit,
        bucket: ProductionBucket,
        points: i32,
    ) -> Result<(), EngineError> {
        for (i, budget) in self.budgets.iter().enumerate() {
            if budget.row.side() != self.side
                || budget.pool_committed < 0
                || self.budgets[..i]
                    .iter()
                    .any(|old| old.row == budget.row && old.pool_index == budget.pool_index)
            {
                return Err(invariant("invalid or duplicate stored finite pool"));
            }
            let mut sum = 0_i32;
            for (j, period) in budget.periods.iter().enumerate() {
                if period.committed <= 0
                    || budget.periods[..j]
                        .iter()
                        .any(|old| old.bucket == period.bucket)
                {
                    return Err(invariant("invalid or duplicate stored production period"));
                }
                sum = sum
                    .checked_add(period.committed)
                    .ok_or_else(|| invariant("stored production period overflow"))?;
            }
            if sum > budget.pool_committed {
                return Err(invariant("period commitments exceed finite pool debit"));
            }
        }
        if points <= 0 || row.side() != self.side || *row != limit.row {
            return Err(invariant("positive same-side production required"));
        }
        let compatible = matches!(
            (&bucket, limit.period),
            (
                ProductionBucket::GameTurn { gt: 1..=111 },
                ProductionPeriod::GameTurn
            ) | (
                ProductionBucket::HalfMonth {
                    year: 1..=9999,
                    month: 1..=12,
                    ..
                },
                ProductionPeriod::HalfCalendarMonth
            ) | (
                ProductionBucket::Month {
                    year: 1..=9999,
                    month: 1..=12
                },
                ProductionPeriod::CalendarMonth
            )
        );
        if !compatible {
            return Err(invariant("bucket does not match printed production period"));
        }
        let mut budget = self
            .budgets
            .iter()
            .find(|b| b.row == *row && b.pool_index == limit.pool_index)
            .cloned()
            .unwrap_or(ProductionBudget {
                row: row.clone(),
                pool_index: limit.pool_index,
                pool_committed: 0,
                periods: vec![],
            });
        if budget.pool_committed < 0
            || budget.pool_committed > limit.pool
            || budget.periods.iter().any(|p| p.committed < 0)
        {
            return Err(invariant("corrupt prior production budget"));
        }
        let old = budget
            .periods
            .iter()
            .find(|p| p.bucket == bucket)
            .map_or(0, |p| p.committed);
        let pool_total = budget
            .pool_committed
            .checked_add(points)
            .ok_or_else(|| invariant("finite pool arithmetic overflow"))?;
        let period_total = old
            .checked_add(points)
            .ok_or_else(|| invariant("period arithmetic overflow"))?;
        if pool_total > limit.pool || period_total > limit.maximum {
            return Err(invariant(
                "determined production exceeds finite availability",
            ));
        }
        budget.pool_committed = pool_total;
        if let Some(period) = budget.periods.iter_mut().find(|p| p.bucket == bucket) {
            period.committed = period_total;
        } else {
            budget.periods.push(PeriodCommitment {
                bucket,
                committed: period_total,
            });
        }
        if let Some(old) = self
            .budgets
            .iter_mut()
            .find(|b| b.row == *row && b.pool_index == limit.pool_index)
        {
            *old = budget;
        } else {
            self.budgets.push(budget);
        }
        Ok(())
    }

    /// One side-local namespace for orders and counter allocations; never fake UnitIds.
    /// Cases: land:20.21, land:20.42
    pub(crate) fn allocate_id(&mut self) -> Result<String, EngineError> {
        let prefix = format!("{}.replacement-", crate::state::side_key(self.side));
        let mut seen = BTreeSet::new();
        let mut serials = BTreeSet::new();
        for id in self
            .orders
            .iter()
            .map(|o| &o.id)
            .chain(self.counters.iter().map(|c| &c.id))
        {
            let serial = id
                .strip_prefix(&prefix)
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|&s| s > 0 && s <= self.next_serial);
            if serial.is_none_or(|n| *id != format!("{prefix}{n}") || !serials.insert(n))
                || !seen.insert(id)
            {
                return Err(invariant(
                    "stored replacement identity is invalid or reused",
                ));
            }
        }
        let serial = self
            .next_serial
            .checked_add(1)
            .ok_or_else(|| invariant("replacement identities exhausted"))?;
        let id = format!("{}.replacement-{serial}", crate::state::side_key(self.side));
        if self.orders.iter().any(|o| o.id == id) || self.counters.iter().any(|c| c.id == id) {
            return Err(invariant("replacement identity collision"));
        }
        self.next_serial = serial;
        Ok(id)
    }
}

#[cfg(test)]
mod arrival_tests;
#[cfg(test)]
mod tests;

mod arrival;
mod scheduling;
mod windows;
pub(crate) use arrival::{
    ARRIVAL, answer_arrival, arrivals_complete, enter_arrival, finish_arrivals,
};
pub(crate) use windows::{
    DISTRIBUTION, SCHEDULE, answer, enter_schedule, finish_planning, planning_complete,
};

/// The actual closing cursor step is the pruning witness; mobile stock is irrelevant.
/// Cases: land:20.78
/// Chart clauses: 20.78A (trucks) and 20.78B (infantry).
pub(crate) fn discard_completed_limits(state: &mut crate::State) -> Result<(), EngineError> {
    if !matches!(state.cursor.anchor(), "end_of_turn" | "end_of_game") {
        return Err(invariant(
            "arrival limits may be discarded only at actual GT/game close",
        ));
    }
    let completed = state.cursor.game_turn;
    for own in &mut state.land.replacements.sides {
        own.cw_arrival_limits
            .retain(|limit| limit.game_turn > completed);
    }
    Ok(())
}
