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
    CampaignHandle::spawn(
        campaign,
        &path,
        Some(Box::new(cna_sandbox::baseline::aggressive)),
    )
}
pub fn recover(path: &Path, data: &Path) -> Result<CampaignHandle, Error> {
    let (content, pins) = inputs(data)?;
    let campaign = Campaign::recover(path, Sandbox, content, &pins)?;
    CampaignHandle::spawn(
        campaign,
        path,
        Some(Box::new(cna_sandbox::baseline::aggressive)),
    )
}
