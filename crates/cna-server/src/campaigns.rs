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
    match (meta.scenario_id.as_str(), pins.rules_profile.as_str()) {
        ("sandbox", cna_sandbox::PROFILE_ID) => crate::sandbox::recover(path, data),
        ("graziani", cna_rules::PROFILE_DEV | cna_rules::PROFILE_FULL) => {
            crate::cna::recover(path, data, &pins.rules_profile)
        }
        _ => Err(Error::Recovery("unsupported scenario/profile pair".into())),
    }
}
