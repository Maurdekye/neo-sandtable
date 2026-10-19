//! S1 arithmetic only. No engine caller, State access, dice, events or resource writes.
//! Callers in later slices must establish the source-defined participating force.
use cna_tables::land::combat::{CombatCalculations, OrganizationSize, StrengthActivity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MathError {
    Invalid,
    Overflow,
}
type Result<T> = std::result::Result<T, MathError>;

fn nonnegative(n: i32) -> Result<i32> {
    if n < 0 {
        Err(MathError::Invalid)
    } else {
        Ok(n)
    }
}
fn narrow(n: i64) -> Result<i32> {
    i32::try_from(n).map_err(|_| MathError::Overflow)
}

/// Pool component products before the chart recipe's one rounding operation.
/// Cases: land:11.31, land:11.32, land:11.33, land:11.34
pub(crate) fn strength(
    table: &CombatCalculations,
    activity: StrengthActivity,
    components: &[(i32, i32)],
) -> Result<(i32, i32)> {
    for &(rating, toe) in components {
        nonnegative(rating)?;
        nonnegative(toe)?;
    }
    let raw = table
        .raw_points(components.iter().copied())
        .ok_or(MathError::Overflow)?;
    let actual = table
        .actual_points(activity, raw)
        .ok_or(MathError::Overflow)?;
    Ok((raw, actual))
}

/// The two-small-forces exception precedes ordinary actual-strength rounding.
/// Cases: land:11.33, land:15.28
pub(crate) fn assault_strengths(table: &CombatCalculations, a: i32, d: i32) -> Result<(i32, i32)> {
    nonnegative(a)?;
    nonnegative(d)?;
    table.assault_points(a, d).ok_or(MathError::Overflow)
}

/// Flak retains raw strength as Actual Points.
/// Cases: land:11.37
pub(crate) fn flak_strength(table: &CombatCalculations, components: &[(i32, i32)]) -> Result<i32> {
    for &(rating, toe) in components {
        nonnegative(rating)?;
        nonnegative(toe)?;
    }
    table
        .raw_points(components.iter().copied())
        .ok_or(MathError::Overflow)
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Activity {
    FireBarrage,
    ReceiveBarrage,
    AntiArmor,
    Assault { probe: bool },
}

/// Required segment total, in CP quarters; affected-hex membership is a caller obligation.
/// Cases: land:11.21, land:11.22, land:11.23, land:11.24, land:11.25, land:11.26
pub(crate) fn ordinary_cp(phasing: bool, activity: Activity) -> i32 {
    match activity {
        Activity::ReceiveBarrage => 12,
        Activity::Assault { probe: true } => 8,
        Activity::FireBarrage | Activity::AntiArmor | Activity::Assault { probe: false } => {
            if phasing {
                20
            } else {
                12
            }
        }
    }
}

/// Already-paid segment CP is never refunded, including when a later result is cheaper.
/// Cases: land:11.24, land:11.27, land:15.96
pub(crate) fn cp_delta(phasing: bool, already: i32, required: i32) -> Result<i32> {
    let cap = if phasing { 20 } else { 12 };
    if !(0..=cap).contains(&already) || !(0..=cap).contains(&required) {
        return Err(MathError::Invalid);
    }
    Ok((required - already).max(0))
}

/// Pure fatigue preview; returns cohesion quarters, with no charge or ledger mutation.
/// Cases: land:6.21, land:6.22, land:17.21
pub(crate) fn cohesion_after_cp(
    cohesion: i32,
    stage_spent: i32,
    cpa: i32,
    delta: i32,
) -> Result<i32> {
    nonnegative(stage_spent)?;
    nonnegative(cpa)?;
    nonnegative(delta)?;
    let spent = i64::from(stage_spent);
    let cap = i64::from(cpa) * 4;
    let extra = (spent + i64::from(delta) - cap).max(0) - (spent - cap).max(0);
    narrow(i64::from(cohesion) - extra)
}

/// Use ordinary cost for morale input once; this value is never an actual charge.
/// Cases: land:11.21, land:11.22, land:11.24, land:11.27, land:15.96, land:17.21
/// Interpretations: interp:land-0043
pub(crate) fn morale_cohesion(
    phasing: bool,
    probe: bool,
    already: i32,
    cohesion: i32,
    stage_spent: i32,
    cpa: i32,
) -> Result<i32> {
    let delta = cp_delta(
        phasing,
        already,
        ordinary_cp(phasing, Activity::Assault { probe }),
    )?;
    cohesion_after_cp(cohesion, stage_spent, cpa, delta)
}

/// Called with the final differential fixed by morale; never feeds its answer back into morale.
/// Cases: land:11.21, land:11.24, land:11.27, land:15.96
/// Interpretations: interp:land-0043
pub(crate) fn actual_defender_cp(probe: bool, differential: i32, already: i32) -> Result<i32> {
    let required = if probe && differential < -3 {
        0
    } else if !probe && differential <= -4 {
        4
    } else {
        ordinary_cp(false, Activity::Assault { probe })
    };
    cp_delta(false, already, required)
}

#[derive(Debug, Clone)]
pub(crate) struct CohesionGroup {
    pub stacking_points: i32,
    pub cohesion_quarters: i32,
    /// Nonempty children resolve the parent's own input before the outer comparison.
    pub children: Vec<CohesionGroup>,
}

/// Resolve parents recursively, then average only the largest participating counters.
/// Single inputs retain their exact quarters. Multi-counter means round to whole
/// cohesion points, with exact half ties away from zero.
/// Cases: land:6.27, land:6.28, land:6.29
/// Interpretations: interp:land-0051
pub(crate) fn largest_cohesion(groups: &[CohesionGroup]) -> Result<i32> {
    let largest = groups
        .iter()
        .map(|g| g.stacking_points)
        .max()
        .ok_or(MathError::Invalid)?;
    for g in groups {
        nonnegative(g.stacking_points)?;
    }
    let mut levels = Vec::new();
    for g in groups.iter().filter(|g| g.stacking_points == largest) {
        levels.push(if g.children.is_empty() {
            g.cohesion_quarters
        } else {
            largest_cohesion(&g.children)?
        });
    }
    if levels.len() == 1 {
        return Ok(levels[0]);
    }
    let total = levels.into_iter().try_fold(0i64, |a, b| {
        a.checked_add(i64::from(b)).ok_or(MathError::Overflow)
    })?;
    let count = i64::try_from(
        groups
            .iter()
            .filter(|g| g.stacking_points == largest)
            .count(),
    )
    .map_err(|_| MathError::Overflow)?;
    let denominator = count.checked_mul(4).ok_or(MathError::Overflow)?;
    let magnitude = total.checked_abs().ok_or(MathError::Overflow)?;
    let whole = magnitude / denominator;
    let remainder = magnitude % denominator;
    let twice = remainder.checked_mul(2).ok_or(MathError::Overflow)?;
    let rounded = whole + i64::from(twice >= denominator);
    narrow(
        rounded
            .checked_mul(4)
            .and_then(|n| n.checked_mul(total.signum()))
            .ok_or(MathError::Overflow)?,
    )
}

/// Land-0021 floors signed cohesion only at morale-table lookup, never in stored accounting.
/// Cases: land:17.22, land:17.24
/// Interpretations: interp:land-0021
pub(crate) fn morale_row(quarters: i32) -> i32 {
    quarters.div_euclid(4).clamp(-17, 8)
}

/// Support is local to each origin; only tank TOE belongs in the first count.
/// The force-wide penalty is capped at four and cannot erase non-tank strength.
/// Cases: land:15.4
pub(crate) fn combined_arms_penalty(origins: &[(i32, i32)], tank_actual: i32) -> Result<i32> {
    nonnegative(tank_actual)?;
    let mut unsupported = 0i64;
    for &(tank, infantry_mg_hw) in origins {
        nonnegative(tank)?;
        nonnegative(infantry_mg_hw)?;
        unsupported = unsupported
            .checked_add(i64::from((tank - infantry_mg_hw).max(0)))
            .ok_or(MathError::Overflow)?;
    }
    narrow(((unsupported + 2) / 3).min(4).min(i64::from(tank_actual)))
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Defense {
    Ordinary,
    ZeroRatedAlone,
    AllPinned,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StrengthShifts {
    pub raw_ratio: i32,
    pub special: i32,
    pub organization: i32,
    pub attacker_immune: bool,
}
impl StrengthShifts {
    pub fn total(self) -> Result<i32> {
        self.raw_ratio
            .checked_add(self.special)
            .and_then(|n| n.checked_add(self.organization))
            .ok_or(MathError::Overflow)
    }
}

/// Classification/size-equivalence is supplied by a later trusted caller, not inferred here.
/// Special zero-defense rules replace the ordinary raw-ratio shift.
/// Cases: land:15.51, land:15.52, land:15.53, land:15.54, land:15.56
pub(crate) fn strength_shifts(
    table: &OrganizationSize,
    attacker_raw: i32,
    defender_raw: i32,
    attacker_size: i32,
    defender_size: i32,
    defense: Defense,
) -> Result<StrengthShifts> {
    nonnegative(attacker_raw)?;
    nonnegative(defender_raw)?;
    let organization = table
        .assault_shift(attacker_size, defender_size)
        .ok_or(MathError::Invalid)?;
    let (raw_ratio, special, attacker_immune) = match defense {
        Defense::ZeroRatedAlone => (0, 3, true),
        Defense::AllPinned => (0, 2, false),
        Defense::Ordinary => {
            let a = i64::from(attacker_raw);
            let d = i64::from(defender_raw);
            (
                if a > 0 && a >= 2 * d {
                    2
                } else if d > 0 && d >= 2 * a {
                    -2
                } else {
                    0
                },
                0,
                false,
            )
        }
    };
    Ok(StrengthShifts {
        raw_ratio,
        special,
        organization,
        attacker_immune,
    })
}

/// AA row numbers are linear, unlike barrage/assault column bands.
/// Cases: land:14.32, land:14.35
pub(crate) fn shifted_anti_armor_row(actual: i32, shift: i32) -> Result<i32> {
    nonnegative(actual)?;
    narrow((i64::from(actual) + i64::from(shift)).max(0))
}

/// Rounded raw casualty obligation; defender overrun uses the attacker's upward rule.
/// Cases: land:15.77, land:15.83
pub(crate) fn raw_loss(raw: i32, percent: i32, attacker: bool, overrun: bool) -> Result<i32> {
    nonnegative(raw)?;
    nonnegative(percent)?;
    let product = i64::from(raw) * i64::from(percent);
    narrow((product + if attacker || overrun { 99 } else { 0 }) / 100)
}

/// Floors are compulsory minima only. The owner allocates the remaining obligation.
/// Cases: land:15.83
/// Interpretations: interp:land-0046
pub(crate) fn origin_floors(loss: i32, raw_by_origin: &[i32]) -> Result<Vec<i32>> {
    nonnegative(loss)?;
    let total = raw_by_origin.iter().try_fold(0i64, |sum, &raw| {
        nonnegative(raw)?;
        sum.checked_add(i64::from(raw)).ok_or(MathError::Overflow)
    })?;
    if total == 0 {
        return if loss == 0 {
            Ok(vec![0; raw_by_origin.len()])
        } else {
            Err(MathError::Invalid)
        };
    }
    raw_by_origin
        .iter()
        .map(|&raw| narrow(i64::from(loss) * i64::from(raw) / total))
        .collect()
}

/// Combined retreating force rounds once; split groups call this separately for extras.
/// Cases: land:15.82, land:15.83
/// Interpretations: interp:land-0046
pub(crate) fn retreat_loss(
    raw: i32,
    crt_percent: i32,
    missing_hexes: i32,
    attacker: bool,
    overrun: bool,
) -> Result<i32> {
    nonnegative(crt_percent)?;
    nonnegative(missing_hexes)?;
    let percent = missing_hexes
        .checked_mul(10)
        .and_then(|n| n.checked_add(crt_percent))
        .ok_or(MathError::Overflow)?;
    raw_loss(raw, percent, attacker, overrun)
}

/// Captured raw points are part of the actual losses, not another casualty removal.
/// Cases: land:15.85, land:15.86
/// Interpretations: interp:land-0046
pub(crate) fn capture_obligation(raw_loss: i32, percent: i32) -> Result<i32> {
    if !(0..=100).contains(&percent) {
        return Err(MathError::Invalid);
    }
    raw_loss_checked(raw_loss, percent)
}
fn raw_loss_checked(raw: i32, percent: i32) -> Result<i32> {
    raw_loss(raw, percent, true, false)
}

/// Each row represents indistinguishable TOE with one contribution vector.
/// Callers encode EVERY obligation (total, origin, vulnerability, capture) and
/// applicable upper bound. Split captured/uncaptured buckets when weights differ.
pub(crate) struct SelectionRow<'a> {
    pub available: i32,
    pub selected: i32,
    pub weights: &'a [i32],
}
pub(crate) struct Obligation {
    pub minimum: i32,
    pub maximum: Option<i32>,
}

/// A legal selection meets all bounds; every selected TOE must be essential to some minimum.
/// Does not pick an allocation or impose a largest-remainder algorithm.
/// Cases: land:15.83, land:15.84, land:15.86
/// Interpretations: interp:land-0046
pub(crate) fn minimal_selection(
    rows: &[SelectionRow<'_>],
    obligations: &[Obligation],
) -> Result<bool> {
    for o in obligations {
        nonnegative(o.minimum)?;
        if o.maximum.is_some_and(|m| m < o.minimum) {
            return Err(MathError::Invalid);
        }
    }
    let mut totals = vec![0i64; obligations.len()];
    for row in rows {
        nonnegative(row.available)?;
        if row.selected < 0
            || row.selected > row.available
            || row.weights.len() != obligations.len()
        {
            return Err(MathError::Invalid);
        }
        for (total, &weight) in totals.iter_mut().zip(row.weights) {
            nonnegative(weight)?;
            *total = total
                .checked_add(i64::from(weight) * i64::from(row.selected))
                .ok_or(MathError::Overflow)?;
        }
    }
    if totals
        .iter()
        .zip(obligations)
        .any(|(&n, o)| n < i64::from(o.minimum) || o.maximum.is_some_and(|m| n > i64::from(m)))
    {
        return Ok(false);
    }
    for row in rows.iter().filter(|row| row.selected > 0) {
        if totals
            .iter()
            .zip(row.weights)
            .zip(obligations)
            .all(|((&n, &weight), o)| n - i64::from(weight) >= i64::from(o.minimum))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Subset check keeps the casualty count fixed while testing captured-TOE minimality.
/// Cases: land:15.85, land:15.86
/// Interpretations: interp:land-0046
pub(crate) fn minimal_captured_subset(
    casualties: &[i32],
    captured: &[i32],
    raw_ratings: &[i32],
    required: i32,
) -> Result<bool> {
    if casualties.len() != captured.len() || captured.len() != raw_ratings.len() {
        return Err(MathError::Invalid);
    }
    let weights: Vec<_> = raw_ratings.iter().map(|&n| [n]).collect();
    let rows: Vec<_> = casualties
        .iter()
        .zip(captured)
        .zip(&weights)
        .map(|((&available, &selected), weights)| SelectionRow {
            available,
            selected,
            weights,
        })
        .collect();
    minimal_selection(
        &rows,
        &[Obligation {
            minimum: required,
            maximum: None,
        }],
    )
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReassignmentFacts {
    pub armor_anywhere: bool,
    pub dummy_only: bool,
    pub existing_assault: bool,
}
pub(crate) struct ReassignmentRow {
    pub fired_here: bool,
    pub fired: i32,
    pub surviving: i32,
    pub moved: i32,
    pub aa_rating: i32,
    pub ca_ammo: bool,
}

/// Pure raw-AA budget and eligibility check; commitments, probe and CP remain unchanged here.
/// Armor includes withheld/pinned/RBA. For defensive fire, caller checks the actual
/// phasing commitments against this target. It must not expose these facts in Respond.
/// Cases: land:14.23, land:15.15, land:15.21, land:16.46
/// Interpretations: interp:land-0042
pub(crate) fn reassignment_allowed(
    actual_aa: i32,
    facts: ReassignmentFacts,
    rows: &[ReassignmentRow],
) -> Result<bool> {
    nonnegative(actual_aa)?;
    let mut raw = 0i64;
    for row in rows {
        for n in [row.fired, row.surviving, row.moved, row.aa_rating] {
            nonnegative(n)?;
        }
        if row.moved > row.fired.min(row.surviving)
            || (row.moved > 0 && (row.aa_rating == 0 || !row.fired_here || !row.ca_ammo))
        {
            return Ok(false);
        }
        raw = raw
            .checked_add(i64::from(row.moved) * i64::from(row.aa_rating))
            .ok_or(MathError::Overflow)?;
    }
    Ok(!facts.armor_anywhere
        && !facts.dummy_only
        && facts.existing_assault
        && raw <= 10 * i64::from(actual_aa / 2))
}

pub(crate) struct HoldingOffUnit {
    pub stacking_points: i32,
    pub combat: bool,
    pub gun: bool,
}

/// Count eligible battalion equivalents, supplied without double-counted parent counters.
/// No public threshold or hidden-roster disclosure is produced by this function.
/// Cases: land:10.34
pub(crate) fn holding_off_count(units: &[HoldingOffUnit]) -> Result<i32> {
    let mut n = 0i32;
    for u in units {
        nonnegative(u.stacking_points)?;
        if u.stacking_points >= 1 && u.combat && !u.gun {
            n = n.checked_add(1).ok_or(MathError::Overflow)?;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests;
