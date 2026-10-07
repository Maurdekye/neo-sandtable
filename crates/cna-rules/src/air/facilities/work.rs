//! Bounded canonical facility effects for trusted Engineering finish drafts.
//!
//! Engineering authenticates the unique ACTIVE ProjectId and exact side/work/
//! target before calling. It transitions that same project to Completed in the
//! same disposable State draft before any replay can call Air. These receipts
//! and Air flags are coordination data, never project authority. Engineering
//! also guards obsolete project incarnations before reusing a constructed site.
//! No cancellation/capture/pause/refund behavior is selected here.

use cna_core::engine::EngineError;
use cna_protocol::Side;
use serde::{Deserialize, Serialize};

use super::{
    FacilityCapacity, FacilityId, FacilityKind, FacilityOrigin, FacilityProperties, FacilityState,
    FacilityWork, invalid,
};
use crate::{CnaContent, air::inventory, state::AirState};

/// This serializable receipt identifies an Air effect, not a unique project.
/// The caller must match it to its actual ACTIVE Engineering ProjectId, exact
/// side/work/target and current site incarnation. Project IDs are never reused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartedFacilityWork {
    side: Side,
    work: FacilityWork,
    target: FacilityId,
}

impl StartedFacilityWork {
    pub fn side(&self) -> Side {
        self.side
    }
    pub fn work(&self) -> &FacilityWork {
        &self.work
    }
    pub fn target(&self) -> &FacilityId {
        &self.target
    }
}

fn water(kind: FacilityKind) -> bool {
    matches!(
        kind,
        FacilityKind::FlyingBoatBasin | FacilityKind::FlyingBoatAlightingArea
    )
}

fn constructed_target(properties: &FacilityProperties) -> Result<FacilityId, EngineError> {
    let crate::state::Location::Hex { hex } = &properties.location else {
        return Err(invalid("constructed target is not on map"));
    };
    Ok(FacilityId(format!(
        "constructed.{}.{hex}",
        if water(properties.kind) {
            "water"
        } else {
            "air"
        }
    )))
}

/// The limit concerns surviving sites, including one currently under work.
/// Scenario tombstones remain untouched, even when a new constructed site is
/// placed in the same hex. No location for an unresolved source is guessed.
/// Cases: land:24.79
fn class_available(
    content: &CnaContent,
    air: &AirState,
    properties: &FacilityProperties,
    except: Option<&FacilityId>,
) -> Result<(), EngineError> {
    for (id, site) in &air.runtime.facilities {
        if except == Some(id) {
            continue;
        }
        let existing = site.properties(content, id)?;
        if existing.location == properties.location
            && water(existing.kind) == water(properties.kind)
            && (!site.removed(&existing) || site.project_unavailable)
        {
            return Err(invalid("facility class already occupies this hex"));
        }
    }
    Ok(())
}

/// Source/class/identity check only, inside a trusted Engineering finish draft.
/// Engineering separately validates worker/terrain/coast/control, payment,
/// duration, actual ProjectId and obsolete-project/incarnation guards.
/// Cases: land:24.71, land:24.76, land:24.79, airlog:36.12, airlog:36.2,
/// airlog:36.3, airlog:36.4
pub fn validate_work(
    content: &CnaContent,
    air: &AirState,
    side: Side,
    work: &FacilityWork,
) -> Result<FacilityId, EngineError> {
    let properties = work.source_properties(content, air, side)?;
    match work {
        FacilityWork::Create { .. } => {
            let target = constructed_target(&properties)?;
            class_available(content, air, &properties, None)?;
            if let Some(old) = air.runtime.facilities.get(&target) {
                let old_properties = old.properties(content, &target)?;
                if !matches!(old.origin, FacilityOrigin::Constructed { .. })
                    || old_properties.location != properties.location
                    || water(old_properties.kind) != water(properties.kind)
                    || !old.removed(&old_properties)
                    || old.project_unavailable
                {
                    return Err(invalid(
                        "constructed identity is not a reusable removed site",
                    ));
                }
                // Reuse is legal only after the caller's real Engineering
                // incarnation guard. An Air record cannot prove that guard.
            }
            Ok(target)
        }
        FacilityWork::RepairOne { id } | FacilityWork::Upgrade { id } => {
            class_available(content, air, &properties, Some(id))?;
            Ok(id.clone())
        }
    }
}

/// Start only a newly authorized unique Engineering project in its disposable
/// finish draft. The caller verifies actual ACTIVE ProjectId/exact side/work/
/// target and site incarnation; this helper does not authenticate any of them.
/// Engineering owns costs and duration. Repair does not suspend a surviving
/// airfield. New construction and upgrades suspend the one canonical site.
/// Cases: land:24.71, land:24.76, land:24.79, airlog:36.12, airlog:36.2,
/// airlog:36.3, airlog:36.4
/// Interpretations: interp:air-0007
pub fn start_work(
    content: &CnaContent,
    air: &mut AirState,
    side: Side,
    work: &FacilityWork,
) -> Result<StartedFacilityWork, EngineError> {
    let target = validate_work(content, air, side, work)?;
    let properties = work.source_properties(content, air, side)?;
    inventory::update(content, air, |runtime| {
        match work {
            FacilityWork::Create { kind, .. } => {
                runtime.facilities.insert(
                    target.clone(),
                    FacilityState {
                        origin: FacilityOrigin::Constructed {
                            kind: *kind,
                            location: properties.location.clone(),
                        },
                        owner: side,
                        current_capacity: properties.printed_capacity,
                        upgraded_kind: None,
                        project_unavailable: true,
                    },
                );
            }
            FacilityWork::Upgrade { .. } => {
                runtime
                    .facilities
                    .get_mut(&target)
                    .expect("validated target")
                    .project_unavailable = true;
            }
            FacilityWork::RepairOne { .. } => {}
        }
        Ok(())
    })?;
    Ok(StartedFacilityWork {
        side,
        work: work.clone(),
        target,
    })
}

/// Apply a completed project's bounded Air effect in a disposable State draft.
/// REQUIRED caller contract: authenticate the unique ACTIVE Engineering
/// ProjectId/lifecycle, exact side/work/target and current incarnation; after a
/// successful call transition that project to Completed in the SAME draft
/// before publishing/replaying. This receipt/Air bool never proves authority.
/// Direct replay of RepairOne can add another level while still damaged; the
/// helper does not deduplicate receipts. Engineering prevents that call.
/// No interruption, cancellation, refund or inferred flag clearing occurs.
/// Cases: land:24.76, land:24.79, airlog:36.12, airlog:36.2, airlog:36.3,
/// airlog:36.4, airlog:36.14, airlog:36.18
/// Interpretations: interp:air-0007
pub fn complete_work(
    content: &CnaContent,
    air: &mut AirState,
    started: &StartedFacilityWork,
) -> Result<FacilityId, EngineError> {
    inventory::check(content, air)?;
    let site = air
        .runtime
        .facilities
        .get(&started.target)
        .ok_or_else(|| invalid("completed work target is not canonical"))?;
    let properties = site.properties(content, &started.target)?;
    if site.owner != started.side {
        return Err(invalid("project no longer owns its target"));
    }
    class_available(content, air, &properties, Some(&started.target))?;
    match &started.work {
        FacilityWork::Create { kind, hex } => {
            if started.target != constructed_target(&properties)?
                || site.origin
                    != (FacilityOrigin::Constructed {
                        kind: *kind,
                        location: crate::state::Location::Hex { hex: hex.clone() },
                    })
                || site.upgraded_kind.is_some()
                || properties.kind != *kind
                || site.current_capacity != FacilityCapacity::Levels(kind.standard_levels())
                || !site.project_unavailable
            {
                return Err(invalid(
                    "new project target or source changed before completion",
                ));
            }
        }
        FacilityWork::Upgrade { id } => {
            if id != &started.target
                || !site.project_unavailable
                || site.removed(&properties)
                || properties.kind.upgrade_to().is_none()
            {
                return Err(invalid(
                    "upgrade target is no longer an active original facility",
                ));
            }
        }
        FacilityWork::RepairOne { id } => {
            if id != &started.target {
                return Err(invalid("repair target differs from receipt"));
            }
            started.work.source_properties(content, air, started.side)?;
        }
    }
    inventory::update(content, air, |runtime| {
        let site = runtime
            .facilities
            .get_mut(&started.target)
            .expect("validated target");
        match &started.work {
            FacilityWork::Create { .. } => {
                site.project_unavailable = false;
            }
            FacilityWork::Upgrade { .. } => {
                let kind = properties
                    .kind
                    .upgrade_to()
                    .expect("validated original kind");
                site.upgraded_kind = Some(kind);
                site.current_capacity = FacilityCapacity::Levels(kind.standard_levels());
                site.project_unavailable = false;
            }
            FacilityWork::RepairOne { .. } => {
                site.repair_one(&properties)?;
            }
        }
        Ok(())
    })?;
    Ok(started.target.clone())
}

#[cfg(test)]
mod tests;
