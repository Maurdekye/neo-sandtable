//! Activity-water accounting shared by voluntary CP, forced CP and truck transfers.
use super::{
    SupplyError,
    rations::{self, WaterStage},
};
use crate::{CnaContent, State};
use cna_content::units::Trucks;
use cna_core::{ids::UnitId, quantity::WaterPoints};
use serde::{Deserialize, Serialize};

/// Values are water points for trucks of each type, never truck counts.
/// Cases: airlog:52.42, airlog:52.43
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruckWater {
    pub light: i32,
    pub medium: i32,
    pub heavy: i32,
}
impl TruckWater {
    fn sum(self) -> Result<i32, SupplyError> {
        [self.light, self.medium, self.heavy]
            .into_iter()
            .try_fold(0i32, |sum, n| {
                if n < 0 {
                    return Err(SupplyError::Invalid);
                }
                sum.checked_add(n).ok_or(SupplyError::Invalid)
            })
    }
    fn from_trucks(t: Trucks, m: i32) -> Result<Self, SupplyError> {
        if [t.light, t.medium, t.heavy].iter().any(|n| *n < 0) {
            return Err(SupplyError::Invalid);
        }
        Ok(Self {
            light: t.light.checked_mul(m).ok_or(SupplyError::Invalid)?,
            medium: t.medium.checked_mul(m).ok_or(SupplyError::Invalid)?,
            heavy: t.heavy.checked_mul(m).ok_or(SupplyError::Invalid)?,
        })
    }
}
/// Original body demand persists through casualties; truck obligations follow explicit splits.
/// Cases: airlog:52.42, airlog:52.43, airlog:52.51
/// Interpretations: interp:airlog-0015
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityWaterLedger {
    pub stage: WaterStage,
    pub body_required: i32,
    pub body_paid: i32,
    pub truck_required: TruckWater,
    pub truck_paid: TruckWater,
    pub forced_assessed: bool,
}
impl ActivityWaterLedger {
    fn remaining(&self) -> Result<i32, SupplyError> {
        if self.body_required < 0
            || self.body_paid < 0
            || self.body_paid > self.body_required
            || self.truck_paid.light > self.truck_required.light
            || self.truck_paid.medium > self.truck_required.medium
            || self.truck_paid.heavy > self.truck_required.heavy
        {
            return Err(SupplyError::Invalid);
        }
        let required = self
            .body_required
            .checked_add(self.truck_required.sum()?)
            .ok_or(SupplyError::Invalid)?;
        let paid = self
            .body_paid
            .checked_add(self.truck_paid.sum()?)
            .ok_or(SupplyError::Invalid)?;
        required.checked_sub(paid).ok_or(SupplyError::Invalid)
    }
    fn pay(&mut self, mut points: i32) -> Result<(), SupplyError> {
        if points < 0 || points > self.remaining()? {
            return Err(SupplyError::Invalid);
        }
        let n = points.min(self.body_required - self.body_paid);
        self.body_paid += n;
        points -= n;
        for (required, paid) in [
            (self.truck_required.light, &mut self.truck_paid.light),
            (self.truck_required.medium, &mut self.truck_paid.medium),
            (self.truck_required.heavy, &mut self.truck_paid.heavy),
        ] {
            let n = points.min(required - *paid);
            *paid += n;
            points -= n;
        }
        Ok(())
    }
}
fn ledger(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<ActivityWaterLedger, SupplyError> {
    let unit = state.land.units.get(id).ok_or(SupplyError::Invalid)?;
    let stage = WaterStage::current(state);
    let history = state.logistics.rations.get(id);
    if let Some(l) = history
        .and_then(|h| h.activity_water_ledger.as_ref())
        .filter(|l| l.stage == stage)
    {
        l.remaining()?;
        return Ok(l.clone());
    }
    let normal = rations::activity_points(content, state, id)?;
    let truck_count = TruckWater::from_trucks(unit.trucks, 1)?.sum()?;
    let m = rations::hot_multiplier(content, state, id)?;
    let body_required = normal
        .checked_sub(truck_count)
        .and_then(|n| n.checked_mul(m))
        .ok_or(SupplyError::Invalid)?;
    let truck_required = TruckWater::from_trucks(unit.trucks, m)?;
    let paid = history.is_some_and(|h| h.activity_used_stage == Some(stage));
    let l = ActivityWaterLedger {
        stage,
        body_required,
        body_paid: if paid { body_required } else { 0 },
        truck_required,
        truck_paid: if paid {
            truck_required
        } else {
            TruckWater::default()
        },
        forced_assessed: paid,
    };
    l.remaining()?;
    Ok(l)
}
/// Unpaid demand before taking the unit's current retained reserve into account.
/// Cases: airlog:52.42, airlog:52.43, airlog:52.51
/// Interpretations: interp:airlog-0015
pub fn activity_water_due(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<i32, SupplyError> {
    ledger(content, state, id)?.remaining()
}
fn record(state: &mut State, id: &UnitId, l: ActivityWaterLedger) -> Result<(), SupplyError> {
    let complete = l.remaining()? == 0;
    let h = state.logistics.rations.entry(id.clone()).or_default();
    h.activity_used_stage = if complete { Some(l.stage) } else { None };
    h.activity_water_ledger = Some(l);
    Ok(())
}
fn reserve(state: &State, id: &UnitId) -> Result<i32, SupplyError> {
    let n = state
        .logistics
        .unit_supply
        .get(id)
        .map_or(0, |s| s.activity_water.get());
    if n < 0 {
        Err(SupplyError::Invalid)
    } else {
        Ok(n)
    }
}
/// Voluntary CPA pays only the remaining balance, or leaves everything unchanged.
/// Cases: airlog:52.42, airlog:52.43
/// Interpretations: interp:airlog-0015
pub fn spend_activity_water(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
) -> Result<(), SupplyError> {
    let mut l = ledger(content, state, id)?;
    let due = l.remaining()?;
    if reserve(state, id)? < due {
        return Err(SupplyError::Insufficient);
    }
    l.pay(due)?;
    if due > 0 {
        state
            .logistics
            .unit_supply
            .get_mut(id)
            .ok_or(SupplyError::Invalid)?
            .activity_water -= WaterPoints::new(due);
    }
    record(state, id, l)
}
/// No known shortage can reject involuntary CP. Unknown composition remains a distinct error
/// for the full/dev caller policy, rather than an invented zero demand.
/// Cases: land:6.13, airlog:52.42, airlog:52.43, airlog:52.51
/// Interpretations: interp:airlog-0015
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityWaterPayment {
    pub consumed: WaterPoints,
    pub shortfall: WaterPoints,
}
/// The first forced use consumes as much as is held; subsequent forced uses do not
/// recharge. Rewatering and voluntary CPA may settle the original unpaid balance.
/// Cases: land:6.13, airlog:52.42, airlog:52.43, airlog:52.51
/// Interpretations: interp:airlog-0015
pub fn consume_activity_water_forced(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
) -> Result<ActivityWaterPayment, SupplyError> {
    let mut l = ledger(content, state, id)?;
    let held = reserve(state, id)?;
    let consumed = if l.forced_assessed {
        0
    } else {
        held.min(l.remaining()?)
    };
    l.pay(consumed)?;
    l.forced_assessed = true;
    let shortfall = l.remaining()?;
    if consumed > 0 {
        state
            .logistics
            .unit_supply
            .get_mut(id)
            .ok_or(SupplyError::Invalid)?
            .activity_water -= WaterPoints::new(consumed);
    }
    record(state, id, l)?;
    Ok(ActivityWaterPayment {
        consumed: WaterPoints::new(consumed),
        shortfall: WaterPoints::new(shortfall),
    })
}
/// Call before physically changing truck counts, on the caller's transactional draft.
/// Only truck credit travels; each body's requirement and payment remain independent.
/// Reserve water and cargo must be split explicitly by the caller.
/// Cases: land:8.56, airlog:52.42, airlog:52.43
/// Interpretations: interp:airlog-0015
pub fn transfer_activity_water_credit(
    content: &CnaContent,
    state: &mut State,
    from: &UnitId,
    to: &UnitId,
    trucks: Trucks,
) -> Result<TruckWater, SupplyError> {
    if from == to {
        return Err(SupplyError::Invalid);
    }
    let a = state.land.units.get(from).ok_or(SupplyError::Invalid)?;
    let b = state.land.units.get(to).ok_or(SupplyError::Invalid)?;
    if a.side != b.side
        || a.location != b.location
        || !rations::in_play(&a.location)
        || trucks.light > a.trucks.light
        || trucks.medium > a.trucks.medium
        || trucks.heavy > a.trucks.heavy
    {
        return Err(SupplyError::Invalid);
    }
    let moved = TruckWater::from_trucks(trucks, rations::hot_multiplier(content, state, from)?)?;
    let mut a = ledger(content, state, from)?;
    let mut b = ledger(content, state, to)?;
    let mut credit = TruckWater::default();
    for (required, paid, dest_required, dest_paid, n, out) in [
        (
            &mut a.truck_required.light,
            &mut a.truck_paid.light,
            &mut b.truck_required.light,
            &mut b.truck_paid.light,
            moved.light,
            &mut credit.light,
        ),
        (
            &mut a.truck_required.medium,
            &mut a.truck_paid.medium,
            &mut b.truck_required.medium,
            &mut b.truck_paid.medium,
            moved.medium,
            &mut credit.medium,
        ),
        (
            &mut a.truck_required.heavy,
            &mut a.truck_paid.heavy,
            &mut b.truck_required.heavy,
            &mut b.truck_paid.heavy,
            moved.heavy,
            &mut credit.heavy,
        ),
    ] {
        if n > *required {
            return Err(SupplyError::Invalid);
        }
        let p = n.min(*paid);
        *required -= n;
        *paid -= p;
        *dest_required = dest_required.checked_add(n).ok_or(SupplyError::Invalid)?;
        *dest_paid = dest_paid.checked_add(p).ok_or(SupplyError::Invalid)?;
        *out = p;
    }
    a.remaining()?;
    b.remaining()?;
    record(state, from, a)?;
    record(state, to, b)?;
    Ok(credit)
}
#[cfg(test)]
mod tests;
