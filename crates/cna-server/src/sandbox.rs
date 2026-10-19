//! Runnable synthetic campaigns on the lead's real-map sandbox ruleset.
use crate::{
    Campaign, Error, Pins,
    actor::CampaignHandle,
    http::{CampaignKind, CreateRequest},
};
use cna_content::map::MapContent;
use cna_core::{dice::CampaignRng, engine::Game, ids::SeatId};
use cna_protocol::{CampaignMeta, ControllerInfo, ControllerKind, SeatInfo, SeatStatus};
use cna_sandbox::{Sandbox, SandboxContent, State};
use sha2::{Digest, Sha256};
use std::path::Path;

fn inputs(data: &Path) -> Result<(SandboxContent, Pins), Error> {
    let directory = data.join("map");
    let map = MapContent::load(&directory).map_err(|e| Error::Invalid(e.to_string()))?;
    let content = SandboxContent::from_map(&map).map_err(Error::Invalid)?;
    let mut digest = Sha256::new();
    for file in ["hexes.csv", "aliases.csv"] {
        let path = directory.join(file);
        if path.exists() {
            digest.update(file.as_bytes());
            digest.update(std::fs::read(path).map_err(|e| Error::Invalid(e.to_string()))?);
        }
    }
    Ok((
        content,
        Pins {
            rules_profile: cna_sandbox::PROFILE_ID.into(),
            content_hash: format!("{:x}", digest.finalize()),
            engine_version: env!("CNA_ENGINE_SOURCE_HASH").into(),
        },
    ))
}
pub fn create(
    directory: &Path,
    data: &Path,
    request: CreateRequest,
) -> Result<CampaignHandle, Error> {
    let (campaign, path) = prepare_create(directory, data, request)?;
    CampaignHandle::spawn(campaign, &path, Some(Box::new(cna_sandbox::baseline::aggressive)))
}

fn prepare_create(
    directory: &Path,
    data: &Path,
    request: CreateRequest,
) -> Result<(Campaign<Sandbox>, std::path::PathBuf), Error> {
    if request.kind != CampaignKind::Sandbox || request.rules_profile != cna_sandbox::PROFILE_ID {
        return Err(Error::Invalid("only sandbox-v1 is implemented".into()));
    }
    let (kind, mode) = match request.controller.as_str() {
        "legal_random" => (ControllerKind::Scripted, "legal_random"),
        "pass_when_possible" => (ControllerKind::Scripted, "pass_when_possible"),
        "aggressive" | "scripted:aggressive" => (ControllerKind::Scripted, "aggressive"),
        "human" => (ControllerKind::Human, "human"),
        _ => return Err(Error::Invalid("unknown controller".into())),
    };
    std::fs::create_dir_all(directory).map_err(|e| Error::Invalid(e.to_string()))?;
    let id = uuid::Uuid::new_v4().to_string();
    let path = directory.join(format!("{id}.sqlite"));
    let (content, pins) = inputs(data)?;
    let state = State::new(&content);
    let meta = CampaignMeta {
        id,
        scenario_id: "sandbox".into(),
        rules_profile: cna_sandbox::PROFILE_ID.into(),
        title: request.title,
        seats: SeatId::all()
            .map(|s| SeatInfo {
                id: s.to_string(),
                side: s.side,
                role: s.role,
                controller: None,
                status: SeatStatus::Idle,
            })
            .collect(),
    };
    let mut campaign = Campaign::create(
        &path,
        Sandbox,
        content,
        Game {
            state,
            rng: CampaignRng::from_seed(request.seed).state(),
        },
        meta,
        pins,
    )?;
    for seat in SeatId::all() {
        campaign.handover(
            seat,
            Some(ControllerInfo {
                kind,
                label: if kind == ControllerKind::Scripted {
                    format!("scripted:{mode}")
                } else {
                    "Human".into()
                },
            }),
            serde_json::json!({"mode":mode}),
        )?;
    }
    if request.paused {
        campaign.set_paused(true)?;
    }
    Ok((campaign, path))
}
pub fn recover(path: &Path, data: &Path) -> Result<CampaignHandle, Error> {
    let campaign = prepare_recover(path, data)?;
    CampaignHandle::spawn(campaign, path, Some(Box::new(cna_sandbox::baseline::aggressive)))
}

fn prepare_recover(path: &Path, data: &Path) -> Result<Campaign<Sandbox>, Error> {
    let (content, pins) = inputs(data)?;
    Campaign::recover(path, Sandbox, content, &pins)
}

pub(crate) fn create_owned<const N: usize>(
    directory: &Path,
    data: &Path,
    request: CreateRequest,
    claimed: crate::owned_writer::VacantStandard<'_, Result<(), Error>, N>,
) -> Result<CampaignHandle, crate::owned_writer::WriterStartFailure> {
    crate::owned_row_envelope::title(&request.title)?;
    let (campaign, path) = prepare_create(directory, data, request)?;
    let baseline = Box::new(|content: &SandboxContent, state: &State, pending: &cna_core::decision::DecisionRequest| {
        Ok(cna_sandbox::baseline::aggressive(content, state, pending))
    }) as crate::actor::FallibleBaseline<Sandbox>;
    CampaignHandle::spawn_owned_claimed(campaign, &path, Some(baseline),
        Box::new(crate::scripted::NoCandidates), None, claimed)
}

pub(crate) fn recover_owned<const N: usize>(
    path: &Path,
    data: &Path,
    claimed: crate::owned_writer::VacantStandard<'_, Result<(), Error>, N>,
) -> Result<CampaignHandle, crate::owned_writer::WriterStartFailure> {
    let (content, pins) = inputs(data)?;
    let campaign = Campaign::recover_owned(path, Sandbox, content, &pins)?;
    let baseline = Box::new(|content: &SandboxContent, state: &State, pending: &cna_core::decision::DecisionRequest| {
        Ok(cna_sandbox::baseline::aggressive(content, state, pending))
    }) as crate::actor::FallibleBaseline<Sandbox>;
    CampaignHandle::spawn_owned_claimed(campaign, path, Some(baseline),
        Box::new(crate::scripted::NoCandidates), None, claimed)
}

// PRIVATE guarded preparation. Host admits the existing storage directory;
// it alone creates/opens the leaf and retains its namespace guards.
pub(crate) struct GuardedFresh {
    content: SandboxContent, game: Game<Sandbox>, meta: CampaignMeta, pins: Pins,
    kind: ControllerKind, mode: &'static str, paused: bool,
}
pub(crate) fn prepare_guarded_fresh<const N: usize>(
    directory: &Path, data: &Path, request: CreateRequest,
    record: &crate::owned_writer::WriterRecord<N>,
) -> Result<GuardedFresh, crate::owned_writer::WriterStartFailure> {
    crate::owned_row_envelope::title(&request.title)?;
    if request.kind != CampaignKind::Sandbox || request.rules_profile != cna_sandbox::PROFILE_ID {
        return Err(Error::Invalid("only sandbox-v1 is implemented".into()).into());
    }
    let (kind, mode) = match request.controller.as_str() {
        "legal_random" => (ControllerKind::Scripted, "legal_random"),
        "pass_when_possible" => (ControllerKind::Scripted, "pass_when_possible"),
        "aggressive" | "scripted:aggressive" => (ControllerKind::Scripted, "aggressive"),
        "human" => (ControllerKind::Human, "human"),
        _ => return Err(Error::Invalid("unknown controller".into()).into()),
    };
    if record.attempt_path().is_some() { return Err(Error::NotRunning.into()); }
    let id = uuid::Uuid::new_v4().to_string();
    let path = directory.join(format!("{id}.sqlite"));
    record.retain_attempt_path(path).map_err(|_| Error::NotRunning)?;
    let (content, pins) = inputs(data)?;
    let state = State::new(&content);
    let meta = CampaignMeta {
        id,
        scenario_id: "sandbox".into(),
        rules_profile: cna_sandbox::PROFILE_ID.into(),
        title: request.title,
        seats: SeatId::all()
            .map(|s| SeatInfo {
                id: s.to_string(),
                side: s.side,
                role: s.role,
                controller: None,
                status: SeatStatus::Idle,
            })
            .collect(),
    };
    Ok(GuardedFresh { content, game: Game { state, rng: CampaignRng::from_seed(request.seed).state() },
        meta, pins, kind, mode, paused: request.paused })
}

impl GuardedFresh {
    pub(crate) fn open_and_spawn<const N: usize>(self,
        custody: &std::sync::Arc<cna_seats::seat_host::CampaignFileCustody>,
        claimed: crate::owned_writer::VacantStandard<'_, Result<(), Error>, N>,
    ) -> Result<CampaignHandle, crate::owned_writer::WriterStartFailure> {
        let mut campaign = Campaign::create_guarded(std::sync::Arc::clone(custody),
            Sandbox, self.content, self.game, self.meta, self.pins)?;
        let mode = self.mode;
    for seat in SeatId::all() {
        campaign.handover(
            seat,
            Some(ControllerInfo {
                kind: self.kind,
                label: if self.kind == ControllerKind::Scripted {
                    format!("scripted:{mode}")
                } else {
                    "Human".into()
                },
            }),
            serde_json::json!({"mode":mode}),
        )?;
    }
    if self.paused {
        campaign.set_paused(true)?;
    }
    let baseline = Box::new(|content: &SandboxContent, state: &State, pending: &cna_core::decision::DecisionRequest| {
        Ok(cna_sandbox::baseline::aggressive(content, state, pending))
    }) as crate::actor::FallibleBaseline<Sandbox>;
        CampaignHandle::spawn_owned_claimed(campaign, custody.protected_path(), Some(baseline),
            Box::new(crate::scripted::NoCandidates), None, claimed)
    }
}
pub(crate) fn recover_guarded_owned<const N: usize>(
    custody: &std::sync::Arc<cna_seats::seat_host::CampaignFileCustody>,
    data: &Path,
    claimed: crate::owned_writer::VacantStandard<'_, Result<(), Error>, N>,
) -> Result<CampaignHandle, crate::owned_writer::WriterStartFailure> {
    let (content, pins) = inputs(data)?;
    let campaign = Campaign::recover_guarded(std::sync::Arc::clone(custody), Sandbox, content, &pins)?;
    let baseline = Box::new(|content: &SandboxContent, state: &State, pending: &cna_core::decision::DecisionRequest| {
        Ok(cna_sandbox::baseline::aggressive(content, state, pending))
    }) as crate::actor::FallibleBaseline<Sandbox>;
    CampaignHandle::spawn_owned_claimed(campaign, custody.protected_path(), Some(baseline),
        Box::new(crate::scripted::NoCandidates), None, claimed)
}

