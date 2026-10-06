//! Real Graziani content through the same authoritative runner as the sandbox.
use crate::{
    Campaign, Error, Pins,
    actor::CampaignHandle,
    http::{CampaignKind, CreateRequest},
};
use cna_core::{dice::CampaignRng, engine::Game, ids::SeatId};
use cna_protocol::{CampaignMeta, ControllerInfo, ControllerKind, SeatInfo, SeatStatus};
use cna_rules::{Cna, CnaContent, State};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn invalid(error: std::io::Error) -> Error {
    Error::Invalid(error.to_string())
}
fn tomls(directory: &Path, recursive: bool, files: &mut Vec<PathBuf>) -> Result<(), Error> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(directory).map_err(invalid)? {
        let path = entry.map_err(invalid)?.path();
        if recursive && path.is_dir() {
            tomls(&path, true, files)?;
        } else if path.extension().is_some_and(|e| e == "toml") {
            files.push(path);
        }
    }
    Ok(())
}

// Mirrors the lead-owned loaders, including their non-recursive unit and registry folders.
// Notes, unused map metadata and unknown registry books are intentionally absent.
fn content_files(data: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut files = Vec::new();
    let map = data.join("map");
    let mut map_sources = vec!["hexes.csv", "aliases.csv"];
    let layers = [
        "layers.toml",
        "coverage.csv",
        "line_features.csv",
        "hexsides.csv",
    ];
    // The loader skips sections provenance when all movement-layer files are absent.
    if layers.iter().any(|name| map.join(name).exists()) {
        map_sources.extend(layers);
        map_sources.push("sections.toml");
    }
    for name in map_sources {
        let path = map.join(name);
        if path.exists() {
            files.push(path);
        }
    }
    for folder in ["weapons", "classes", "aircraft", "schedules"] {
        tomls(&data.join("units").join(folder), false, &mut files)?;
    }
    for entry in fs::read_dir(data.join("units/oa")).map_err(invalid)? {
        let path = entry.map_err(invalid)?.path();
        if path.is_dir() {
            tomls(&path, false, &mut files)?;
        }
    }
    for name in [
        "scenario.toml",
        "land_axis.toml",
        "land_cw.toml",
        "air_axis.toml",
        "air_cw.toml",
        "supply.toml",
        "facilities.toml",
        "construction.toml",
        "fleet.toml",
        "arrivals.toml",
    ] {
        let path = data.join("scenarios/graziani").join(name);
        if path.exists() {
            files.push(path);
        }
    }
    tomls(&data.join("tables"), true, &mut files)?;
    for book in ["land", "airlog", "scen"] {
        tomls(&data.join("rules").join(book), false, &mut files)?;
    }
    files.sort();
    Ok(files)
}
fn content_hash(data: &Path) -> Result<String, Error> {
    let mut digest = Sha256::new();
    digest.update(b"cna-graziani-inputs-v1");
    for path in content_files(data)? {
        let name = path
            .strip_prefix(data)
            .map_err(|e| Error::Invalid(e.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&path).map_err(invalid)?;
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn ruleset(profile: &str) -> Result<Cna, Error> {
    match profile {
        cna_rules::PROFILE_DEV => Ok(Cna::dev()),
        cna_rules::PROFILE_FULL => Ok(Cna::full()),
        _ => Err(Error::Invalid("unknown CNA rules profile".into())),
    }
}
fn inputs(data: &Path, profile: &str) -> Result<(CnaContent, Pins), Error> {
    let before = content_hash(data)?;
    let content = CnaContent::load(data, "graziani").map_err(Error::Invalid)?;
    if content.scenario_key() != "graziani" {
        return Err(Error::Invalid("scenario content ID is not graziani".into()));
    }
    if before != content_hash(data)? {
        return Err(Error::Invalid(
            "content changed while loading; retry creation/recovery".into(),
        ));
    }
    Ok((
        content,
        Pins {
            rules_profile: profile.into(),
            content_hash: before,
            engine_version: env!("CNA_ENGINE_SOURCE_HASH").into(),
        },
    ))
}
pub fn create(
    directory: &Path,
    data: &Path,
    request: CreateRequest,
) -> Result<CampaignHandle, Error> {
    if request.kind != CampaignKind::Cna {
        return Err(Error::Invalid("CNA factory requires kind cna".into()));
    }
    let ruleset = ruleset(&request.rules_profile)?;
    let kind = match request.controller.as_str() {
        "legal_random" | "pass_when_possible" => ControllerKind::Scripted,
        "human" => ControllerKind::Human,
        _ => {
            return Err(Error::Invalid(
                "CNA supports legal_random, pass_when_possible or human".into(),
            ));
        }
    };
    let (content, pins) = inputs(data, &request.rules_profile)?;
    let state = State::new(&content).map_err(Error::Invalid)?;
    fs::create_dir_all(directory).map_err(invalid)?;
    let id = uuid::Uuid::new_v4().to_string();
    let path = directory.join(format!("{id}.sqlite"));
    let meta = CampaignMeta {
        id,
        scenario_id: "graziani".into(),
        rules_profile: request.rules_profile,
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
        ruleset,
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
                    format!("scripted:{}", request.controller)
                } else {
                    "Human".into()
                },
            }),
            serde_json::json!({"mode": request.controller}),
        )?;
    }
    if request.paused {
        campaign.set_paused(true)?;
    }
    CampaignHandle::spawn(campaign, &path, None)
}
pub fn recover(path: &Path, data: &Path, profile: &str) -> Result<CampaignHandle, Error> {
    let ruleset = ruleset(profile)?;
    let (content, pins) = inputs(data, profile)?;
    let campaign = Campaign::recover(path, ruleset, content, &pins)?;
    CampaignHandle::spawn(campaign, path, None)
}
