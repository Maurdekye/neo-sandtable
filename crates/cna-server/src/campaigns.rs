//! Creation and read-only recovery dispatch for the supported campaign profiles.
use crate::{
    Error, Pins,
    actor::CampaignHandle,
    http::{CampaignKind, CreateRequest},
};
use cna_protocol::CampaignMeta;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

pub fn create(
    directory: &Path,
    data: &Path,
    request: CreateRequest,
) -> Result<CampaignHandle, Error> {
    match request.kind {
        CampaignKind::Sandbox => crate::sandbox::create(directory, data, request),
        CampaignKind::Cna => crate::cna::create(directory, data, request),
    }
}

/// Inspect the persisted discriminator before opening a writer or interpreting checkpoints.
pub fn recover(path: &Path, data: &Path) -> Result<CampaignHandle, Error> {
    let (meta, pins) = recovery_identity(path)?;
    match (meta.scenario_id.as_str(), pins.rules_profile.as_str()) {
        ("sandbox", cna_sandbox::PROFILE_ID) => crate::sandbox::recover(path, data),
        ("graziani", cna_rules::PROFILE_DEV | cna_rules::PROFILE_FULL) => {
            crate::cna::recover(path, data, &pins.rules_profile)
        }
        _ => Err(Error::Recovery("unsupported scenario/profile pair".into())),
    }
}

fn recovery_identity(path: &Path) -> Result<(CampaignMeta, Pins), Error> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let (meta, pins): (String, String) =
        db.query_row("SELECT meta, pins FROM campaign WHERE id=1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    let meta: CampaignMeta = serde_json::from_str(&meta)?;
    let pins: Pins = serde_json::from_str(&pins)?;
    drop(db);
    if meta.rules_profile != pins.rules_profile {
        return Err(Error::Recovery(
            "metadata and input profile disagree".into(),
        ));
    }
    Ok((meta, pins))
}

fn recovery_identity_owned(path: &Path)
    -> Result<(CampaignMeta, Pins), crate::owned_writer::WriterStartFailure>
{
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(Error::from)?;
    let (meta, pins) = crate::owned_row_envelope::recovery_identity(&db)?;
    drop(db);
    if meta.rules_profile != pins.rules_profile {
        return Err(Error::Recovery("metadata and input profile disagree".into()).into());
    }
    Ok((meta, pins))
}

/// Held concrete producer; never an arbitrary legacy HTTP Factory closure.
/// Its claim came from the original cleanup owner BEFORE any preparation/DB
/// effect. The returned writer remains blocked until original result storage
/// and the trusted parent explicitly release it. No live caller is wired yet.
pub(crate) fn create_owned<const N: usize>(
    directory: &Path,
    data: &Path,
    request: CreateRequest,
    claimed: crate::owned_writer::VacantStandard<'_, Result<(), Error>, N>,
) -> Result<CampaignHandle, crate::owned_writer::WriterStartFailure> {
    crate::owned_row_envelope::title(&request.title)?;
    match request.kind {
        CampaignKind::Sandbox => crate::sandbox::create_owned(directory, data, request, claimed),
        CampaignKind::Cna => crate::cna::create_owned(directory, data, request, claimed),
    }
}

pub(crate) fn recover_owned<const N: usize>(
    path: &Path,
    data: &Path,
    claimed: crate::owned_writer::VacantStandard<'_, Result<(), Error>, N>,
) -> Result<CampaignHandle, crate::owned_writer::WriterStartFailure> {
    let (meta, pins) = recovery_identity_owned(path)?;
    match (meta.scenario_id.as_str(), pins.rules_profile.as_str()) {
        ("sandbox", cna_sandbox::PROFILE_ID) => crate::sandbox::recover_owned(path, data, claimed),
        ("graziani", cna_rules::PROFILE_DEV | cna_rules::PROFILE_FULL) => {
            crate::cna::recover_owned(path, data, &pins.rules_profile, claimed)
        }
        _ => Err(Error::Recovery("unsupported scenario/profile pair".into()).into()),
    }
}

/// This plan remains on the SAME original creator worker across the custody gate.
pub(crate) enum GuardedFresh { Cna(crate::cna::GuardedFresh), Sandbox(crate::sandbox::GuardedFresh) }
pub(crate) fn prepare_guarded_fresh<const N: usize>(directory: &Path, data: &Path,
    request: CreateRequest, record: &crate::owned_writer::WriterRecord<N>) -> Result<GuardedFresh, crate::owned_writer::WriterStartFailure>
{
    crate::owned_row_envelope::title(&request.title)?;
    match request.kind {
        CampaignKind::Cna => crate::cna::prepare_guarded_fresh(directory, data, request, record).map(GuardedFresh::Cna),
        CampaignKind::Sandbox => crate::sandbox::prepare_guarded_fresh(directory, data, request, record).map(GuardedFresh::Sandbox),
    }
}
impl GuardedFresh {
    pub(crate) fn open_and_spawn<const N: usize>(self,
        custody: &std::sync::Arc<cna_seats::seat_host::CampaignFileCustody>,
        claimed: crate::owned_writer::VacantStandard<'_, Result<(), Error>, N>,
    ) -> Result<CampaignHandle, crate::owned_writer::WriterStartFailure> {
        match self {
            Self::Cna(original) => original.open_and_spawn(custody, claimed),
            Self::Sandbox(original) => original.open_and_spawn(custody, claimed),
        }
    }
}
pub(crate) fn recover_guarded_owned<const N: usize>(data: &Path,
    claimed: crate::owned_writer::VacantStandard<'_, Result<(), Error>, N>,
) -> Result<CampaignHandle, crate::owned_writer::WriterStartFailure> {
    // Parent Arc predates BOTH opens and spans reader close/profile loading.
    // No path-equality inference, second mint or legacy fallback exists here.
    let custody = claimed.record().connection_custody().cloned().ok_or(Error::NotRunning)?;
    let (meta, pins) = recovery_identity_owned(custody.protected_path())?;
    match (meta.scenario_id.as_str(), pins.rules_profile.as_str()) {
        ("sandbox", cna_sandbox::PROFILE_ID) => crate::sandbox::recover_guarded_owned(&custody, data, claimed),
        ("graziani", cna_rules::PROFILE_DEV | cna_rules::PROFILE_FULL) =>
            crate::cna::recover_guarded_owned(&custody, data, &pins.rules_profile, claimed),
        _ => Err(Error::Recovery("unsupported scenario/profile pair".into()).into()),
    }
}
