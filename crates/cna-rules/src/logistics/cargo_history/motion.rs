//! Actual physical carrying-truck CP within an OpStage. Fuel segment CP,
//! breakdown points and the parent body's CP are never substitutes.
//! Creation/stage-entry callers seed zero only when they certify fresh physical
//! trucks. Missing current-stage records stay unknown. Live hooks are coordinated
//! with Land; merely querying this module never initializes or resets a history.
use super::{CargoSite, CarrierTiming, WaterStage, owner};
use crate::State;
use crate::logistics::{FuelTruckKind, TruckFuelCohort};
use cna_protocol::Side;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MotionError {
    Invalid,
    Unknown,
    Mixed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalTrucks {
    pub id: String,
    pub parent: Option<String>,
    pub kind: FuelTruckKind,
    pub count: i32,
}
impl<A> From<&TruckFuelCohort<A>> for PhysicalTrucks {
    fn from(c: &TruckFuelCohort<A>) -> Self {
        Self {
            id: c.id.clone(),
            parent: c.parent.clone(),
            kind: c.kind,
            count: c.count,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruckMotion {
    pub id: String,
    pub kind: FuelTruckKind,
    pub count: i32,
    pub spent_cp_quarters: i32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotionEntry {
    pub site: CargoSite,
    pub stage: WaterStage,
    pub cohorts: Vec<TruckMotion>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionState {
    pub entries: Vec<MotionEntry>,
}
fn permitted(state: &State, side: Side, site: &CargoSite) -> Result<(), MotionError> {
    if !matches!(
        site,
        CargoSite::Unit(_) | CargoSite::Pool(_) | CargoSite::BrokenMarker(_)
    ) || owner(state, site) != Some(side)
    {
        return Err(MotionError::Invalid);
    }
    Ok(())
}
fn physical(cohorts: &[PhysicalTrucks]) -> Result<(), MotionError> {
    let mut ids = BTreeSet::new();
    for c in cohorts {
        if c.id.is_empty() || c.count <= 0 || c.parent.as_ref() == Some(&c.id) || !ids.insert(&c.id)
        {
            return Err(MotionError::Invalid);
        }
    }
    Ok(())
}
fn entry<'a>(
    motion: &'a MotionState,
    site: &CargoSite,
    stage: WaterStage,
) -> Result<&'a MotionEntry, MotionError> {
    let mut matches = motion
        .entries
        .iter()
        .filter(|e| &e.site == site && e.stage == stage);
    let e = matches.next().ok_or(MotionError::Unknown)?;
    if matches.next().is_some() {
        return Err(MotionError::Invalid);
    }
    let mut ids = BTreeSet::new();
    if e.cohorts
        .iter()
        .any(|c| c.count <= 0 || c.spent_cp_quarters < 0 || c.id.is_empty() || !ids.insert(&c.id))
    {
        return Err(MotionError::Invalid);
    }
    Ok(e)
}
fn save(motion: &mut MotionState, e: MotionEntry) {
    motion.entries.retain(|old| old.site != e.site);
    motion.entries.push(e);
    motion.entries.sort_by(|a, b| a.site.cmp(&b.site));
}
/// Exact current physical identities and counts are supplied by the carrier's
/// own validated fuel-cohort/inventory API. No parent-lineage fallback at query.
/// Cases: airlog:53.24, airlog:53.25, land:8.83, land:8.89
pub fn query(
    state: &State,
    side: Side,
    site: &CargoSite,
    cohorts: &[PhysicalTrucks],
) -> Result<Vec<TruckMotion>, MotionError> {
    permitted(state, side, site)?;
    physical(cohorts)?;
    if cohorts.is_empty() {
        return Ok(vec![]);
    }
    let e = entry(
        &state.logistics.cargo_history.motion,
        site,
        WaterStage::current(state),
    )?;
    if e.cohorts.len() != cohorts.len() {
        return Err(MotionError::Unknown);
    }
    let mut result = vec![];
    for c in cohorts {
        let h = e
            .cohorts
            .iter()
            .find(|h| h.id == c.id)
            .ok_or(MotionError::Unknown)?;
        if h.kind != c.kind || h.count != c.count {
            return Err(MotionError::Unknown);
        }
        result.push(h.clone());
    }
    Ok(result)
}
/// A single trusted timing exists only for homogeneous physical CP. Differing
/// histories need a selected packing witness; the parent body is irrelevant.
/// Cases: airlog:53.22, airlog:53.25
pub fn timing(
    state: &State,
    side: Side,
    site: &CargoSite,
    cohorts: &[PhysicalTrucks],
    cpa_quarters: i32,
) -> Result<CarrierTiming, MotionError> {
    if cpa_quarters <= 0 {
        return Err(MotionError::Invalid);
    }
    let records = query(state, side, site, cohorts)?;
    let cp = records
        .first()
        .ok_or(MotionError::Unknown)?
        .spent_cp_quarters;
    if records.iter().any(|c| c.spent_cp_quarters != cp) {
        return Err(MotionError::Mixed);
    }
    Ok(CarrierTiming {
        spent_cp_quarters: cp,
        cpa_quarters,
    })
}
/// Seed newly created physical groups, or all groups at authoritative new-stage
/// entry. Repeated seeding cannot clear a paid/current moving history. This is
/// not a fallback for a legacy checkpoint missing its current-stage records.
/// Cases: airlog:53.25, land:8.83, land:8.89
pub fn seed_fresh(
    state: &mut State,
    side: Side,
    site: &CargoSite,
    cohorts: &[PhysicalTrucks],
) -> Result<(), MotionError> {
    permitted(state, side, site)?;
    physical(cohorts)?;
    let stage = WaterStage::current(state);
    let mut next = state.logistics.cargo_history.motion.clone();
    let mut e = match entry(&next, site, stage) {
        Ok(e) => e.clone(),
        Err(MotionError::Unknown) => MotionEntry {
            site: site.clone(),
            stage,
            cohorts: vec![],
        },
        Err(err) => return Err(err),
    };
    for c in cohorts {
        if next
            .entries
            .iter()
            .filter(|other| other.stage == stage && owner(state, &other.site) == Some(side))
            .any(|other| other.cohorts.iter().any(|h| h.id == c.id))
        {
            return Err(MotionError::Invalid);
        }
        e.cohorts.push(TruckMotion {
            id: c.id.clone(),
            kind: c.kind,
            count: c.count,
            spent_cp_quarters: 0,
        });
    }
    save(&mut next, e);
    state.logistics.cargo_history.motion = next;
    Ok(())
}
/// Record actual carrying or cargo-handling CP only. Body-only fire, detachment
/// and other non-truck CP must not call this helper. Cycles and fuel segments
/// do not change the OpStage tag or erase this history.
/// Cases: airlog:53.24, airlog:53.25, land:8.83, land:8.89
pub fn advance(
    state: &mut State,
    side: Side,
    site: &CargoSite,
    cohorts: &[PhysicalTrucks],
    delta: i32,
) -> Result<(), MotionError> {
    if delta < 0 {
        return Err(MotionError::Invalid);
    }
    let mut records = query(state, side, site, cohorts)?;
    for c in &mut records {
        c.spent_cp_quarters = c
            .spent_cp_quarters
            .checked_add(delta)
            .ok_or(MotionError::Invalid)?;
    }
    if !records.is_empty() {
        let stage = WaterStage::current(state);
        save(
            &mut state.logistics.cargo_history.motion,
            MotionEntry {
                site: site.clone(),
                stage,
                cohorts: records,
            },
        );
    }
    Ok(())
}
/// Advance only exact current-stage histories present among the caller-validated
/// physical groups. Missing or stale groups remain unknown; untouched records
/// are retained. This is an edge writer, never a source of fresh histories.
/// Cases: airlog:53.24, airlog:53.25, land:8.83, land:8.89
pub fn advance_tracked(
    state: &mut State,
    side: Side,
    site: &CargoSite,
    cohorts: &[PhysicalTrucks],
    delta: i32,
) -> Result<(), MotionError> {
    if delta < 0 {
        return Err(MotionError::Invalid);
    }
    permitted(state, side, site)?;
    physical(cohorts)?;
    let mut e = match entry(
        &state.logistics.cargo_history.motion,
        site,
        WaterStage::current(state),
    ) {
        Ok(e) => e.clone(),
        Err(MotionError::Unknown) => return Ok(()),
        Err(err) => return Err(err),
    };
    let mut changed = false;
    for h in &mut e.cohorts {
        if cohorts
            .iter()
            .any(|c| c.id == h.id && c.kind == h.kind && c.count == h.count)
        {
            h.spent_cp_quarters = h
                .spent_cp_quarters
                .checked_add(delta)
                .ok_or(MotionError::Invalid)?;
            changed = true;
        }
    }
    if changed {
        save(&mut state.logistics.cargo_history.motion, e);
    }
    Ok(())
}

/// Move exactly the cohorts returned by an already validated physical split.
/// A partial split uses the returned new identity and its immediate parent;
/// this does not copy the whole parent's quantity or any body history. Markers
/// must exist before the transfer, on the caller's transactional draft.
/// Cases: land:8.56, land:21.25, land:21.29, airlog:53.25
pub fn transfer(
    state: &mut State,
    side: Side,
    from: &CargoSite,
    to: &CargoSite,
    selected: &[PhysicalTrucks],
) -> Result<(), MotionError> {
    permitted(state, side, from)?;
    permitted(state, side, to)?;
    physical(selected)?;
    if from == to {
        return Err(MotionError::Invalid);
    }
    let stage = WaterStage::current(state);
    let mut next = state.logistics.cargo_history.motion.clone();
    let mut source = entry(&next, from, stage)?.clone();
    let mut dest = match entry(&next, to, stage) {
        Ok(e) => e.clone(),
        Err(MotionError::Unknown) => MotionEntry {
            site: to.clone(),
            stage,
            cohorts: vec![],
        },
        Err(err) => return Err(err),
    };
    for c in selected {
        if next
            .entries
            .iter()
            .filter(|e| e.stage == stage && e.site != *from && owner(state, &e.site) == Some(side))
            .any(|e| e.cohorts.iter().any(|h| h.id == c.id))
        {
            return Err(MotionError::Invalid);
        }
        if dest.cohorts.iter().any(|h| h.id == c.id) {
            return Err(MotionError::Invalid);
        }
        let source_id = if source.cohorts.iter().any(|h| h.id == c.id) {
            c.id.as_str()
        } else {
            c.parent.as_deref().ok_or(MotionError::Unknown)?
        };
        let h = source
            .cohorts
            .iter_mut()
            .find(|h| h.id == source_id)
            .ok_or(MotionError::Unknown)?;
        if h.kind != c.kind || c.count > h.count {
            return Err(MotionError::Invalid);
        }
        if c.id == source_id && c.count != h.count {
            return Err(MotionError::Invalid);
        }
        let moved = TruckMotion {
            id: c.id.clone(),
            kind: c.kind,
            count: c.count,
            spent_cp_quarters: h.spent_cp_quarters,
        };
        h.count -= c.count;
        dest.cohorts.push(moved);
    }
    source.cohorts.retain(|c| c.count > 0);
    save(&mut next, source);
    save(&mut next, dest);
    state.logistics.cargo_history.motion = next;
    Ok(())
}

/// Retire destroyed or departing exact physical groups. A partial removal uses
/// the returned split identity. Broken trucks instead transfer to their marker
/// so later recovery can return the same actual CP history.
/// Cases: land:12.46, land:20.44, land:21.25, land:21.29, airlog:53.25
pub fn retire(
    state: &mut State,
    side: Side,
    site: &CargoSite,
    selected: &[PhysicalTrucks],
) -> Result<(), MotionError> {
    permitted(state, side, site)?;
    physical(selected)?;
    let stage = WaterStage::current(state);
    let mut source = entry(&state.logistics.cargo_history.motion, site, stage)?.clone();
    for c in selected {
        let source_id = if source.cohorts.iter().any(|h| h.id == c.id) {
            c.id.as_str()
        } else {
            c.parent.as_deref().ok_or(MotionError::Unknown)?
        };
        let h = source
            .cohorts
            .iter_mut()
            .find(|h| h.id == source_id)
            .ok_or(MotionError::Unknown)?;
        if h.kind != c.kind || c.count > h.count || (c.id == source_id && c.count != h.count) {
            return Err(MotionError::Invalid);
        }
        h.count -= c.count;
    }
    source.cohorts.retain(|c| c.count > 0);
    save(&mut state.logistics.cargo_history.motion, source);
    Ok(())
}

/// Destroy selected counts in place when no fuel split has been materialized.
/// This never creates a transferable child identity or initializes missing history.
/// Cases: land:20.83, airlog:53.25
pub(in crate::logistics) fn retire_selected_counts(
    state: &mut State,
    side: Side,
    site: &CargoSite,
    selected: &[PhysicalTrucks],
) -> Result<(), MotionError> {
    permitted(state, side, site)?;
    physical(selected)?;
    let mut source = entry(
        &state.logistics.cargo_history.motion,
        site,
        WaterStage::current(state),
    )?
    .clone();
    for c in selected {
        if c.parent.is_some() {
            return Err(MotionError::Invalid);
        }
        let h = source
            .cohorts
            .iter_mut()
            .find(|h| h.id == c.id)
            .ok_or(MotionError::Invalid)?;
        if h.kind != c.kind || c.count > h.count {
            return Err(MotionError::Invalid);
        }
        h.count -= c.count;
    }
    source.cohorts.retain(|c| c.count > 0);
    save(&mut state.logistics.cargo_history.motion, source);
    Ok(())
}

#[cfg(test)]
mod tests;
