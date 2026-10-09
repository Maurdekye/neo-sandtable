//! Trusted production preparation and finish, without shipping entitlement.
use super::*;
use crate::{CnaContent, State};
use cna_core::{engine::{Cx, Rejection}, event::EngineEvent, ids::SeatId, visibility::Audience};
use cna_protocol::{GameEvent, Role};
use cna_tables::land::production::{InfantryProductionColumn, InfantryProductionResult, TruckProductionColumn};

pub(super) const ANCHOR: &str = "naval_convoy.schedule";

pub(super) fn key(state: &State) -> ProcedureKey {
    ProcedureKey {
        anchor: state.cursor.anchor().into(),
        segment: SegmentKey {
            game_turn: state.cursor.game_turn,
            op_stage: state.cursor.op_stage,
            half: state.cursor.half,
            cycle: state.cursor.cycle,
        },
    }
}

pub(super) fn side(state: &State, owner: Side) -> Result<&SideReplacements, EngineError> {
    let mut found = state.land.replacements.sides.iter().filter(|s| s.side == owner);
    let value = found.next().ok_or_else(|| invariant("missing replacement side"))?;
    if found.next().is_some() { return Err(invariant("duplicate replacement side")); }
    Ok(value)
}

pub(super) fn side_mut(state: &mut State, owner: Side) -> Result<&mut SideReplacements, EngineError> {
    side(state, owner)?;
    state.land.replacements.sides.iter_mut().find(|s| s.side == owner)
        .ok_or_else(|| invariant("missing replacement side"))
}

fn date(content: &CnaContent, state: &State, gt: u16) -> Result<String, EngineError> {
    let mut clock = state.clone();
    clock.cursor.game_turn = gt;
    let date = crate::view::wire_clock(content, &clock).date;
    if date.len() != 10 || !date.is_ascii() {
        return Err(gap("land:20.72", "production requires the actual campaign calendar"));
    }
    Ok(date)
}

/// Schedule only public calendar opportunities, irrespective of prior budgets/outcomes.
/// Commonwealth plans the following calendar month, with the adopted four-turn minimum.
/// Cases: land:20.21, land:20.63, land:20.72, scen:60.47
/// Interpretations: interp:land-0014, interp:airlog-0004
pub(super) fn arrival_turns(content: &CnaContent, state: &State, owner: Side) -> Result<Vec<u16>, EngineError> {
    let gt = state.cursor.game_turn;
    if owner == Side::Axis {
        let Some(first) = gt.checked_add(2) else { return Ok(vec![]); };
        return Ok((first..=content.bounds.end_gt).collect());
    }
    let now = date(content, state, gt)?;
    if (gt > content.bounds.start_gt && date(content, state, gt - 1)?[..7] == now[..7])
        || (gt == content.bounds.start_gt && now[8..10].parse::<u8>().map_err(|_| invariant("invalid calendar day"))? > 7) {
        return Ok(vec![]);
    }
    let year: u16 = now[..4].parse().map_err(|_| invariant("invalid calendar year"))?;
    let month: u8 = now[5..7].parse().map_err(|_| invariant("invalid calendar month"))?;
    let (year, month) = if month == 12 {
        (year.checked_add(1).ok_or_else(|| invariant("calendar overflow"))?, 1)
    } else { (year, month + 1) };
    let next = format!("{year:04}-{month:02}");
    let Some(first) = gt.checked_add(4) else { return Ok(vec![]); };
    let mut arrivals = vec![];
    for arrival in first..=content.bounds.end_gt {
        let when = date(content, state, arrival)?;
        if &when[..7] == next.as_str() && !(content.scenario.meta.id == "graziani" && when.as_str() < "1940-11-01") {
            arrivals.push(arrival);
        }
        if &when[..7] > next.as_str() { break; }
    }
    Ok(arrivals)
}

/// The printed monthly marks are actual calendar buckets, not rolling counters.
/// A fifth-week half-month interpretation is unresolved and cannot become a third allowance.
/// Cases: land:20.61, land:20.66, land:20.78
pub(super) fn bucket(content: &CnaContent, state: &State, gt: u16, period: ProductionPeriod) -> Result<ProductionBucket, EngineError> {
    if period == ProductionPeriod::GameTurn { return Ok(ProductionBucket::GameTurn { gt }); }
    let when = date(content, state, gt)?;
    let year = when[..4].parse().map_err(|_| invariant("invalid calendar year"))?;
    let month = when[5..7].parse().map_err(|_| invariant("invalid calendar month"))?;
    match period {
        ProductionPeriod::CalendarMonth => Ok(ProductionBucket::Month { year, month }),
        ProductionPeriod::HalfCalendarMonth => {
            let day: u8 = when[8..10].parse().map_err(|_| invariant("invalid calendar day"))?;
            let half = match day {
                1..=14 => CalendarHalf::First,
                15..=28 => CalendarHalf::Second,
                _ => return Err(gap("land:20.66", "fifth calendar week has no reviewed half-month production allocation")),
            };
            Ok(ProductionBucket::HalfMonth { year, month, half })
        }
        _ => Err(gap("land:20.66", "production period is unreadable")),
    }
}

pub(super) fn truck_column(gt: u16) -> Result<(&'static str, TruckProductionColumn), EngineError> {
    match gt {
        1..=30 => Ok(("gt_1_to_30", TruckProductionColumn::GameTurns1To30)),
        31..=107 => Ok(("gt_31_to_107", TruckProductionColumn::GameTurns31To107)),
        _ => Err(gap("land:20.78", "arrival date has no printed Commonwealth truck column")),
    }
}

pub(super) fn infantry_column(gt: u16) -> Result<(&'static str, InfantryProductionColumn), EngineError> {
    match gt {
        3..=30 => Ok(("gt_3_to_30", InfantryProductionColumn::GameTurns3To30)),
        31..=46 => Ok(("gt_31_to_46", InfantryProductionColumn::GameTurns31To46)),
        47..=102 => Ok(("gt_47_to_102", InfantryProductionColumn::GameTurns47To102)),
        103..=107 => Ok(("gt_103_to_107", InfantryProductionColumn::GameTurns103To107)),
        _ => Err(gap("land:20.78", "arrival date has no printed Commonwealth infantry column")),
    }
}

/// Exact unique source designation only; aliases/classes never guess a WeaponId.
/// Cases: land:20.23, land:20.24, land:20.67
pub(super) fn kind(content: &CnaContent, row: &ProductionRowRef) -> Result<ProducedKind, EngineError> {
    let (nation, item) = match row {
        ProductionRowRef::AxisTruck { kind } | ProductionRowRef::CommonwealthTruck { kind, .. } => return Ok(ProducedKind::Trucks { kind: *kind }),
        ProductionRowRef::CommonwealthInfantry { .. } => return Ok(ProducedKind::Points { kind: ReplacementKind::Infantry { nation: ReplacementNation::Commonwealth } }),
        ProductionRowRef::AxisEquipment { nation, item } => (*nation, item),
        ProductionRowRef::CommonwealthEquipment { item } => (ReplacementNation::Commonwealth, item),
    };
    if item == "Infantry" {
        return Ok(ProducedKind::Points { kind: ReplacementKind::Infantry { nation } });
    }
    let prefix = match nation { ReplacementNation::Commonwealth => "cw", ReplacementNation::German => "ge", ReplacementNation::Italian => "it" };
    let mut candidates = content.units.weapons.values().filter(|w| w.nation == prefix && w.name == *item);
    let weapon = candidates.next().ok_or_else(|| gap("land:20.67", format!("no verified exact weapon identity for production row {item}")))?;
    if candidates.next().is_some() { return Err(gap("land:20.67", "production designation is not a unique weapon identity")); }
    let kind = match weapon.kind.as_str() {
        "tank" => ReplacementKind::Tank { nation, weapon: weapon.id.clone() },
        "gun" | "sp_gun" | "anti_tank" | "sp_anti_tank" | "anti_air_light" | "anti_air_heavy" => ReplacementKind::Gun { nation, weapon: weapon.id.clone() },
        _ => return Err(gap("land:20.67", "printed production class needs a verified replacement class binding")),
    };
    Ok(ProducedKind::Points { kind })
}

#[derive(Debug, Clone)]
pub(super) struct PreparedSchedule {
    owner: Side,
    key: ProcedureKey,
    requests: Vec<ProductionRequest>,
}

/// Prepare reads the producing seat's own budget and public source calendar only.
/// No dice, order allocation or budget writes occur while accepting an answer.
/// Cases: land:20.61, land:20.66, land:20.72, land:20.73
pub(super) fn prepare_schedule(content: &CnaContent, state: &State, seat: SeatId, plan: &SchedulePlan) -> Result<PreparedSchedule, Rejection> {
    if seat.role != Role::Logistics || state.cursor.anchor() != ANCHOR {
        return Err(crate::steps::illegal("not the producing Logistics window"));
    }
    let current = key(state);
    let own = side(state, seat.side).map_err(Rejection::Engine)?;
    if own.planning.key.as_ref() != Some(&current) || !matches!(own.planning.phase, ProcedurePhase::Awaiting { submitted: None }) {
        return Err(crate::steps::illegal("production window is closed"));
    }
    let arrivals = arrival_turns(content, state, seat.side).map_err(Rejection::Engine)?;
    let mut preview = own.clone();
    for (i, request) in plan.requests.iter().enumerate() {
        if request.row.side() != seat.side || !arrivals.contains(&request.arrival_turn)
            || plan.requests[..i].iter().any(|p| p.row == request.row && p.arrival_turn == request.arrival_turn)
        { return Err(crate::steps::illegal("invalid or duplicate production selection")); }
        let known = match &request.row {
            ProductionRowRef::AxisEquipment { nation: ReplacementNation::German, item } => content.tables.land.axis_replacement_pool.equipment(AxisProductionNation::German, item).is_some(),
            ProductionRowRef::AxisEquipment { nation: ReplacementNation::Italian, item } => content.tables.land.axis_replacement_pool.equipment(AxisProductionNation::Italian, item).is_some(),
            ProductionRowRef::AxisEquipment { .. } => false,
            ProductionRowRef::CommonwealthEquipment { item } => content.tables.land.commonwealth_production.equipment(item).is_some(),
            _ => true,
        };
        if !known { return Err(crate::steps::illegal("unknown production row")); }
        kind(content, &request.row).map_err(Rejection::Engine)?;
        match (&request.row, &request.quantity) {
            (ProductionRowRef::CommonwealthTruck { column, .. }, ProductionQuantity::Roll) => {
                if *column != truck_column(request.arrival_turn).map_err(Rejection::Engine)?.0 {
                    return Err(crate::steps::illegal("truck column must match arrival date"));
                }
            }
            (ProductionRowRef::CommonwealthInfantry { column }, ProductionQuantity::Roll) => {
                if *column != infantry_column(request.arrival_turn).map_err(Rejection::Engine)?.0 {
                    return Err(crate::steps::illegal("infantry column must match arrival date"));
                }
            }
            (ProductionRowRef::CommonwealthTruck { .. } | ProductionRowRef::CommonwealthInfantry { .. }, _) => return Err(crate::steps::illegal("random production accepts a roll, not a requested count")),
            (_, ProductionQuantity::Roll) => return Err(crate::steps::illegal("finite production requires a determined count")),
            (_, ProductionQuantity::Fixed { points }) => {
                let limit = finite_limit(&content.tables.land.axis_replacement_pool, &content.tables.land.commonwealth_production,
                    &request.row, state.cursor.game_turn, request.arrival_turn).map_err(|error| match error {
                        EngineError::Invariant { detail } if detail.contains("unavailable at") => crate::steps::illegal("production row is unavailable at the required chart date"),
                        error => Rejection::Engine(error),
                    })?;
                let at = if seat.side == Side::Axis { state.cursor.game_turn } else { request.arrival_turn };
                let bucket = bucket(content, state, at, limit.period).map_err(Rejection::Engine)?;
                preview.commit_budget(&request.row, &limit, bucket, *points).map_err(|error| match error {
                    EngineError::Invariant { detail } if detail.contains("exceeds finite") || *points <= 0 => crate::steps::illegal("requested production exceeds availability"),
                    error => Rejection::Engine(error),
                })?;
            }
        }
    }
    // Random per-turn production is complete: omitting a type cannot save its roll for later.
    if seat.side == Side::Commonwealth {
        for arrival in arrivals {
            let column = truck_column(arrival).map_err(Rejection::Engine)?.0;
            for truck in [FuelTruckKind::Light, FuelTruckKind::Medium, FuelTruckKind::Heavy] {
                if !plan.requests.iter().any(|r| r.arrival_turn == arrival && r.row == (ProductionRowRef::CommonwealthTruck { column: column.into(), kind: truck })) {
                    return Err(crate::steps::illegal("include every truck roll for each scheduled turn"));
                }
            }
            let column = infantry_column(arrival).map_err(Rejection::Engine)?.0;
            if !plan.requests.iter().any(|r| r.arrival_turn == arrival && r.row == (ProductionRowRef::CommonwealthInfantry { column: column.into() })) {
                return Err(crate::steps::illegal("include the infantry roll for each scheduled turn"));
            }
        }
    }
    Ok(PreparedSchedule { owner: seat.side, key: current, requests: plan.requests.clone() })
}

/// Determine once on private cloned State/RNG; publish only after all rows succeed.
/// A zero result still leaves Distributing, so hidden counts never remove the fixed window.
/// Cases: land:20.61, land:20.73, land:20.74, land:20.78, land:3.6
pub(super) fn finish_schedule(content: &CnaContent, state: &mut State, prepared: &PreparedSchedule, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    if key(state) != prepared.key { return Err(invariant("stale production preparation")); }
    let own = side(state, prepared.owner)?;
    if !matches!(&own.planning.phase, ProcedurePhase::Awaiting { submitted: Some(ReplacementPlan::Schedule(plan)) }
        if serde_json::to_value(plan).ok() == serde_json::to_value(SchedulePlan { requests: prepared.requests.clone() }).ok()) {
        return Err(invariant("production finish is not bound to its accepted answer"));
    }
    let mut draft = state.clone();
    let mut rng = cx.rng.clone();
    let mut events = vec![];
    let mut determined = vec![];
    for request in &prepared.requests {
        let produced = kind(content, &request.row)?;
        let (points, dice) = match (&request.row, &request.quantity) {
            (ProductionRowRef::CommonwealthTruck { kind, .. }, ProductionQuantity::Roll) => {
                let die = rng.d6().value();
                let points = content.tables.land.commonwealth_production.trucks(truck_column(request.arrival_turn)?.1, truck(*kind), die.into())
                    .ok_or_else(|| invariant("bound truck production cell missing"))?;
                (points, vec![die])
            }
            (ProductionRowRef::CommonwealthInfantry { .. }, ProductionQuantity::Roll) => {
                let dice = vec![rng.d6().value(), rng.d6().value()];
                let result = content.tables.land.commonwealth_production.infantry(infantry_column(request.arrival_turn)?.1, i32::from(dice[0]) + i32::from(dice[1]))
                    .ok_or_else(|| invariant("bound infantry production cell missing"))?;
                (match result { InfantryProductionResult::Points(points) => points, InfantryProductionResult::PrintedNone => 0 }, dice)
            }
            (_, ProductionQuantity::Fixed { points }) => {
                let limit = finite_limit(&content.tables.land.axis_replacement_pool, &content.tables.land.commonwealth_production,
                    &request.row, state.cursor.game_turn, request.arrival_turn)?;
                let at = if prepared.owner == Side::Axis { state.cursor.game_turn } else { request.arrival_turn };
                side_mut(&mut draft, prepared.owner)?.commit_budget(&request.row, &limit, bucket(content, state, at, limit.period)?, *points)?;
                (*points, vec![])
            }
            _ => return Err(invariant("accepted random/finite production mismatch")),
        };
        if points < 0 { return Err(invariant("negative determined production")); }
        if !dice.is_empty() {
            events.push(EngineEvent::new(Audience::Seat(SeatId::new(prepared.owner, Role::Logistics)), GameEvent::DiceRolled {
                purpose: format!("Replacement production for GT{}: {:?}", request.arrival_turn, request.row),
                dice, reading: None, rule: Some("land:20.73".into()),
            }));
        }
        determined.push(DeterminedProduction { row: request.row.clone(), kind: produced, points, arrival_turn: request.arrival_turn });
    }
    side_mut(&mut draft, prepared.owner)?.planning.phase = ProcedurePhase::Distributing { determined, submitted: None };
    *state = draft;
    *cx.rng = rng;
    cx.events.extend(events);
    Ok(())
}

/// All-type stage balancing is independent of how the types are mixed among stages.
/// No partial allocation or duplicate zero row is accepted; no RNG is touched here.
/// Cases: land:20.74
pub(super) fn validate_distribution(owner: Side, determined: &[DeterminedProduction], plan: &DistributionPlan) -> Result<(), Rejection> {
    for (i, source) in determined.iter().enumerate() {
        if source.points < 0 || source.row.side() != owner
            || determined[..i].iter().any(|old| old.row == source.row && old.arrival_turn == source.arrival_turn) {
            return Err(Rejection::Engine(invariant("corrupt determined production")));
        }
    }
    let mut stages = std::collections::BTreeMap::<u16, [i32; 3]>::new();
    for (i, batch) in plan.batches.iter().enumerate() {
        if batch.points <= 0 || !(1..=3).contains(&batch.due.op_stage)
            || plan.batches[..i].iter().any(|old| old.row == batch.row && old.due == batch.due)
            || !determined.iter().any(|d| d.row == batch.row && d.kind == batch.kind && d.arrival_turn == batch.due.game_turn)
        { return Err(crate::steps::illegal("invalid replacement distribution row")); }
        let total = &mut stages.entry(batch.due.game_turn).or_default()[usize::from(batch.due.op_stage - 1)];
        *total = total.checked_add(batch.points).ok_or_else(|| crate::steps::illegal("distribution overflow"))?;
    }
    for source in determined {
        let total = plan.batches.iter().filter(|b| b.row == source.row && b.due.game_turn == source.arrival_turn)
            .try_fold(0_i32, |sum, b| sum.checked_add(b.points)).ok_or_else(|| crate::steps::illegal("distribution overflow"))?;
        if total != source.points { return Err(crate::steps::illegal("distribute exactly the determined production")); }
        stages.entry(source.arrival_turn).or_default();
    }
    if owner == Side::Commonwealth && stages.values().any(|s| s.iter().max().unwrap() - s.iter().min().unwrap() > 1) {
        return Err(crate::steps::illegal("combined replacements must be divided evenly across all three OpStages"));
    }
    Ok(())
}

/// Determined availability was debited at production, never again at allocation or arrival.
/// Cases: land:20.21, land:20.63, land:20.74
pub(super) fn finish_distribution(state: &mut State, owner: Side, plan: &DistributionPlan) -> Result<(), EngineError> {
    let own = side(state, owner)?;
    if own.planning.key.as_ref() != Some(&key(state)) {
        return Err(invariant("stale distribution preparation"));
    }
    let ProcedurePhase::Distributing { determined, submitted: Some(accepted) } = &own.planning.phase else {
        return Err(invariant("distribution has no accepted answer"));
    };
    if serde_json::to_value(accepted).ok() != serde_json::to_value(plan).ok() {
        return Err(invariant("distribution does not match accepted answer"));
    }
    validate_distribution(owner, determined, plan).map_err(|rejection| match rejection {
        Rejection::Engine(error) => error,
        _ => invariant("accepted distribution is no longer valid"),
    })?;
    let mut draft = own.clone();
    if owner == Side::Commonwealth {
        let mut totals = std::collections::BTreeMap::<u16, i32>::new();
        for source in determined {
            let total = totals.entry(source.arrival_turn).or_default();
            if matches!(source.kind, ProducedKind::Trucks { .. }) {
                *total = total.checked_add(source.points).ok_or_else(|| invariant("arrival total overflow"))?;
            }
        }
        for (gt, total_trucks) in totals {
            if draft.cw_arrival_limits.iter().any(|limit| limit.game_turn == gt) {
                return Err(invariant("scheduled arrival GT already has a retained limit"));
            }
            draft.cw_arrival_limits.push(CWArrivalLimit { game_turn: gt, total_trucks, alexandria_trucks: 0, infantry_city: None });
        }
        draft.cw_arrival_limits.sort_by_key(|limit| limit.game_turn);
    }
    for batch in &plan.batches {
        let id = draft.allocate_id()?;
        draft.orders.push(ScheduledReplacement { id, row: batch.row.clone(), kind: batch.kind.clone(), points: batch.points, due: batch.due, lane: None });
    }
    draft.planning.phase = ProcedurePhase::Resolved;
    *side_mut(state, owner)? = draft;
    Ok(())
}
