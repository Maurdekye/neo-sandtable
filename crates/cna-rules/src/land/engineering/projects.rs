//! Private project authority for trusted construction finish drafts.
//! Worker eligibility, payment and completion time are separately adjudicated.
//! No answer handler or public view may use this ledger as hidden acceptance.
//! Cases: land:24.71, land:24.74, land:24.75, land:24.76, land:24.79

use std::collections::{BTreeMap, BTreeSet};

use cna_core::engine::EngineError;
use cna_protocol::Side;
use serde::{Deserialize, Serialize};

use crate::air::facilities::work::StartedFacilityWork;
use crate::air::facilities::{FacilityId, FacilityWork, SiteIncarnation, work};
use crate::{CnaContent, State};

/// Campaign-local, never reused, distinct from a stable facility SITE identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProjectLifecycle {
    Active,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FacilityProject {
    id: ProjectId,
    side: Side,
    work: FacilityWork,
    target: FacilityId,
    incarnation: SiteIncarnation,
    started: StartedFacilityWork,
    lifecycle: ProjectLifecycle,
}

fn corrupt(detail: impl Into<String>) -> EngineError {
    EngineError::Invariant {
        detail: format!("engineering facility project: {}", detail.into()),
    }
}

/// Validate persisted authority before treating any record as a project.
/// Completed history remains valid after its physical site is replaced.
fn check_ledger(state: &State) -> Result<(), EngineError> {
    let mut active_sites = BTreeSet::new();
    let mut token_sites = BTreeMap::new();
    for (id, project) in &state.engineering.facility_projects {
        if id.0 == 0 || id.0 > state.engineering.facility_project_serial || id != &project.id {
            return Err(corrupt("invalid project identity or allocator"));
        }
        if project.incarnation.serial() == 0
            || project.incarnation.serial() > state.air.runtime.site_incarnation_serial
        {
            return Err(corrupt("invalid project incarnation or Air allocator"));
        }
        if token_sites
            .insert(project.incarnation, &project.target)
            .is_some_and(|old| old != &project.target)
        {
            return Err(corrupt(
                "one incarnation references different SITE identities",
            ));
        }
        if project.side != project.started.side()
            || &project.work != project.started.work()
            || &project.target != project.started.target()
            || project.target != work::target_id(&project.work)
            || Some(project.incarnation) != project.started.incarnation()
        {
            return Err(corrupt("project differs from its stored receipt"));
        }
        if project.lifecycle == ProjectLifecycle::Active
            && !active_sites.insert(project.target.clone())
        {
            return Err(corrupt("multiple Active projects reference one SITE"));
        }
    }
    Ok(())
}

/// Begin only a newly adjudicated project on an owning finish draft.
/// All Active SITE references are checked before Air may establish a legacy
/// token. New Create instances receive a fresh token in Air, never a reused one.
/// This operation confers no worker, supply, duration or terrain entitlement.
/// Cases: land:24.71, land:24.74, land:24.75, land:24.76, land:24.79
pub fn begin_facility_project(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    work: &FacilityWork,
) -> Result<ProjectId, EngineError> {
    check_ledger(state)?;
    // Identity only; Air still validates canonical content and availability.
    let target = work::target_id(work);
    if let Some(active) = state
        .engineering
        .facility_projects
        .values()
        .find(|project| project.lifecycle == ProjectLifecycle::Active && project.target == target)
    {
        check_active_site(content, state, active)?;
        return Err(corrupt("SITE already has an Active project"));
    }
    let serial = state
        .engineering
        .facility_project_serial
        .checked_add(1)
        .ok_or_else(|| corrupt("project identities exhausted"))?;
    let id = ProjectId(serial);
    if state.engineering.facility_projects.contains_key(&id) {
        return Err(corrupt("project identity collision"));
    }
    let mut draft = state.clone();
    // start_work's NEW-begin legacy establishment is guarded above on the
    // whole State, not by an Air-only boolean or a caller receipt.
    let started = work::start_work(content, &mut draft.air, side, work)?;
    let incarnation = started
        .incarnation()
        .ok_or_else(|| corrupt("new Air receipt has no incarnation"))?;
    if started.side() != side || started.work() != work || started.target() != &target {
        return Err(corrupt("Air start returned a different project"));
    }
    let project = FacilityProject {
        id,
        side,
        work: work.clone(),
        target,
        incarnation,
        started,
        lifecycle: ProjectLifecycle::Active,
    };
    check_active_site(content, &draft, &project)?;
    draft.engineering.facility_project_serial = serial;
    draft.engineering.facility_projects.insert(id, project);
    *state = draft;
    Ok(id)
}

fn interrupted(project: &FacilityProject, case: &str, reason: &str) -> EngineError {
    EngineError::Unsupported {
        case: case.into(),
        detail: format!(
            "engineering project {} at {}: {reason}; project interruption disposition is not implemented",
            project.id.0, project.target.0,
        ),
    }
}

/// Removal and capture can be legal game effects, not corrupt-ledger bugs.
/// Retain Active history without choosing cancellation, refunds or resumption.
fn check_active_site(
    content: &CnaContent,
    state: &State,
    project: &FacilityProject,
) -> Result<(), EngineError> {
    let Some(site) = state.air.runtime.facilities.get(&project.target) else {
        return Err(interrupted(project, "land:24.76", "target is absent"));
    };
    if site.owner != project.side {
        return Err(interrupted(project, "airlog:36.15", "target was captured"));
    }
    let properties = site.properties(content, &project.target)?;
    if site.removed(&properties) {
        return Err(interrupted(project, "land:24.76", "target was removed"));
    }
    let incarnation = work::current_site_incarnation(content, &state.air, &project.target)?
        .ok_or_else(|| corrupt("Active target has no incarnation"))?;
    if incarnation != project.incarnation {
        return Err(interrupted(
            project,
            "land:24.76",
            "target instance was replaced",
        ));
    }
    Ok(())
}

/// Trusted finish-only operation after worker/payment/time adjudication.
/// A canonical ID authenticates the stored Active record; no caller receipt or
/// ready boolean can substitute for it. The caller's whole finish remains a
/// disposable State draft so any subsequent failure discards these effects.
/// Cases: land:24.74, land:24.76, land:24.79, airlog:36.14, airlog:36.15
pub fn complete_facility_project(
    content: &CnaContent,
    state: &mut State,
    id: &ProjectId,
) -> Result<FacilityId, EngineError> {
    check_ledger(state)?;
    let project = state
        .engineering
        .facility_projects
        .get(id)
        .ok_or_else(|| corrupt("completion references an unknown project"))?
        .clone();
    if project.lifecycle != ProjectLifecycle::Active {
        return Err(corrupt("completion replay of a Completed project"));
    }
    check_active_site(content, state, &project)?;
    let mut draft = state.clone();
    let target = work::complete_work(content, &mut draft.air, &project.started)?;
    if target != project.target {
        return Err(corrupt("Air completion returned a different SITE"));
    }
    draft
        .engineering
        .facility_projects
        .get_mut(id)
        .expect("authenticated project in cloned draft")
        .lifecycle = ProjectLifecycle::Completed;
    *state = draft;
    Ok(target)
}

#[cfg(test)]
mod tests;
