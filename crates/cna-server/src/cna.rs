//! Real Graziani content through the same authoritative runner as the sandbox.
use crate::{
    Campaign, Error, Pins,
    actor::{ActionPolicy, CampaignHandle},
    http::{CampaignKind, CreateRequest},
};
use cna_core::{dice::CampaignRng, engine::Game, ids::SeatId};
use cna_protocol::{CampaignMeta, ControllerInfo, ControllerKind, SeatInfo, SeatStatus};
use cna_rules::{Cna, CnaContent, State};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

fn invalid(error: std::io::Error) -> Error {
    Error::Invalid(error.to_string())
}
fn content_hash(data: &Path) -> Result<String, Error> {
    // The loader records actual reads, including conditional/reused scenario inputs.
    // Normalize the prefix exactly as those recorded paths before deriving relative names.
    let data = cna_content::normalize(data);
    let files = cna_rules::content::source_files(&data, "graziani").map_err(Error::Invalid)?;
    let mut digest = Sha256::new();
    digest.update(b"cna-graziani-inputs-v1");
    for path in files {
        let name = path
            .strip_prefix(&data)
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
    CampaignHandle::spawn_with_policy(campaign, &path, movement_policy())
}
pub fn recover(path: &Path, data: &Path, profile: &str) -> Result<CampaignHandle, Error> {
    let ruleset = ruleset(profile)?;
    let (content, pins) = inputs(data, profile)?;
    let campaign = Campaign::recover(path, ruleset, content, &pins)?;
    CampaignHandle::spawn_with_policy(campaign, path, movement_policy())
}

fn movement_policy() -> ActionPolicy<Cna> {
    Box::new(|content, state, request, epoch| {
        if request.kind != cna_rules::land::movement::KIND {
            return None;
        }
        // Derive a fresh controller-local stream from the versioned request/epoch.
        // Never read, advance or replace the campaign's adjudication dice.
        let mut hash = Sha256::new();
        hash.update(b"cna-scripted-movement-v1");
        hash.update(
            serde_json::to_vec(&(request, epoch)).expect("decision request is serializable"),
        );
        let seed = hash.finalize().into();
        let mut rng = CampaignRng::from_seed(seed);
        // Preserve legal-random's declared-pass choice before generating an order.
        // Reject face six to keep the existing one-in-five probability unbiased.
        if request.space.pass.is_some() {
            let choice = loop {
                let die = rng.d6().value();
                if die <= 5 {
                    break die;
                }
            };
            if choice == 1 {
                return Some(serde_json::Value::Null);
            }
        }
        Some(cna_rules::baseline::random_orders(
            content, state, request, &mut rng,
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cna_content::map::{LineKind, MapContent, SideKind};
    use cna_core::quantity::FuelTenths;
    use cna_core::{
        engine::{Command, Ruleset, evaluate},
        ids::UnitId,
    };
    use cna_rules::{
        seq::{Block, Half},
        state::{Location, UnitSupply, WeatherState},
    };
    use cna_tables::land::weather::WeatherKind;

    fn movement_fixture() -> (CnaContent, Game<Cna>) {
        let data = cna_content::repo_data_dir();
        let mut content = CnaContent::load(&data, "graziani").unwrap();
        let directory = tempfile::tempdir().unwrap();
        for file in ["layers.toml", "sections.toml"] {
            fs::copy(data.join("map").join(file), directory.path().join(file)).unwrap();
        }
        let mut hexes = String::from("hex_id,section,q,r,terrain,flags\n");
        for id in ["C4020", "C4021", "C4022"] {
            let h = content.map.get(&id.into()).unwrap();
            hexes += &format!("{id},C,{},{},clear,\n", h.axial.q, h.axial.r);
        }
        fs::write(directory.path().join("hexes.csv"), hexes).unwrap();
        fs::write(
            directory.path().join("aliases.csv"),
            "alias_id,hex_id,src\n",
        )
        .unwrap();
        let mut coverage = String::from("layer,hex_id,neighbour_id,src,review_batch\n");
        for id in ["C4020", "C4021", "C4022"] {
            coverage += &format!("terrain,{id},,land:8.37,test\n");
        }
        for (a, b) in [("C4020", "C4021"), ("C4021", "C4022")] {
            for k in LineKind::ALL {
                coverage += &format!("line:{},{a},{b},land:8.33,test\n", k.name());
            }
            for k in SideKind::ALL {
                coverage += &format!("side:{},{a},{b},land:8.35,test\n", k.name());
            }
        }
        fs::write(directory.path().join("coverage.csv"), coverage).unwrap();
        fs::write(directory.path().join("line_features.csv"), "from_hex,to_hex,kind,src,review_batch\nC4020,C4021,road,land:8.33,test\nC4021,C4022,road,land:8.33,test\n").unwrap();
        fs::write(
            directory.path().join("hexsides.csv"),
            "hex_id,direction,neighbour_id,feature,high_side,src,review_batch\n",
        )
        .unwrap();
        let mut state = State::new(&content).unwrap();
        content.map = MapContent::load(directory.path()).unwrap();
        for unit in state.land.units.values_mut() {
            unit.location = Location::Eliminated;
        }
        let id: UnitId = "it.libyan_tank_command.xxi_l_tank_bn".into();
        let unit = state.land.units.get_mut(&id).unwrap();
        unit.location = Location::Hex {
            hex: "C4020".into(),
        };
        unit.detached = true;
        unit.attached_to = None;
        state.logistics.unit_supply.insert(
            id,
            UnitSupply {
                tank_fuel: FuelTenths::new(10000),
                ..UnitSupply::default()
            },
        );
        state.turn.weather = Some(WeatherState {
            kind: WeatherKind::Normal,
            storm_sections: vec![],
        });
        state.turn.player_a = Some(cna_protocol::Side::Axis);
        state.cursor.block = Block::PlayerHalf;
        state.cursor.half = Some(Half::A);
        state.cursor.op_stage = Some(1);
        state.cursor.index = 1;
        state.cursor.entered = false;
        let rules = Cna::dev();
        let game = evaluate(
            &rules,
            &content,
            &Game {
                state,
                rng: CampaignRng::from_seed([3; 32]).state(),
            },
            &Command::Advance,
        )
        .unwrap()
        .game;
        (content, game)
    }

    #[test]
    fn movement_policy_is_deterministic_legal_and_leaves_adjudication_rng_untouched() {
        let (content, game) = movement_fixture();
        let rules = Cna::dev();
        let request = rules.pending(&content, &game.state)[0].clone();
        let before = serde_json::to_value(&game).unwrap();
        let policy = movement_policy();
        let (epoch, action) = (0..64)
            .find_map(|epoch| {
                let action = policy(&content, &game.state, &request, epoch).unwrap();
                action
                    .as_array()
                    .is_some_and(|a| !a.is_empty())
                    .then_some((epoch, action))
            })
            .expect("controller chooses an actual move");
        assert!((0..64).any(|epoch| {
            policy(&content, &game.state, &request, epoch)
                .unwrap()
                .is_null()
        }));
        assert_eq!(
            action,
            policy(&content, &game.state, &request, epoch).unwrap()
        );
        assert_eq!(before, serde_json::to_value(&game).unwrap());
        let accepted = evaluate(
            &rules,
            &content,
            &game,
            &Command::Respond(cna_core::decision::DecisionResponse {
                decision_id: request.id.clone(),
                seat: request.seat,
                controller_epoch: epoch,
                decision_revision: request.revision,
                idempotency_key: "test-movement".into(),
                action,
                public_explanation: None,
            }),
        )
        .unwrap();
        assert_eq!(game.rng, accepted.game.rng);
        let mut other = request;
        other.kind = "cna.initiative_declaration".into();
        assert_eq!(policy(&content, &game.state, &other, 7), None);
        assert_ne!(before, serde_json::to_value(&accepted.game).unwrap());
    }

    #[test]
    fn bounded_movers_dispatch_recovers_without_changing_dice_or_inventing_commands() {
        use crate::{
            actor::auto_step,
            scripted::{NoCandidates, Step},
        };
        use cna_core::visibility::Perspective;
        use std::time::{Duration, Instant};
        let (content, game) = movement_fixture();
        let initial_rng = game.rng.clone();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bounded-movers.sqlite");
        let pins = Pins {
            rules_profile: cna_rules::PROFILE_DEV.into(),
            content_hash: "real-roster-test-map".into(),
            engine_version: env!("CNA_ENGINE_SOURCE_HASH").into(),
        };
        let meta = CampaignMeta {
            id: "bounded-movers".into(),
            scenario_id: "graziani".into(),
            rules_profile: cna_rules::PROFILE_DEV.into(),
            title: "Bounded movement".into(),
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
            Cna::dev(),
            movement_fixture().0,
            game,
            meta,
            pins.clone(),
        )
        .unwrap();
        for seat in SeatId::all() {
            campaign
                .handover(
                    seat,
                    Some(ControllerInfo {
                        kind: ControllerKind::Scripted,
                        label: "scripted:pass_when_possible".into(),
                    }),
                    serde_json::json!({"mode":"pass_when_possible"}),
                )
                .unwrap();
        }
        let policy = movement_policy();
        let request = campaign.pending()[0].clone();
        // Select an epoch that exercises a real move, keeping the campaign dice untouched.
        let (epoch, first_action) = (2..64)
            .find_map(|epoch| {
                let action = policy(&content, &campaign.game.state, &request, epoch).unwrap();
                action
                    .as_array()
                    .is_some_and(|a| !a.is_empty())
                    .then_some((epoch, action))
            })
            .unwrap();
        while campaign.binding(request.seat).controller_epoch < epoch {
            campaign
                .handover(
                    request.seat,
                    Some(ControllerInfo {
                        kind: ControllerKind::Scripted,
                        label: "scripted:legal_random".into(),
                    }),
                    serde_json::json!({"mode":"legal_random"}),
                )
                .unwrap();
        }
        let started = Instant::now();
        let mut submitted = 0;
        let mut engine_time = Duration::ZERO;
        let mut writer_time = Duration::ZERO;
        let mut projection_time = Duration::ZERO;
        let mut boundaries = 0;
        for _ in 0..256 {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "bounded mover stalled"
            );
            let previous = campaign.game.clone();
            let db = rusqlite::Connection::open(&path).unwrap();
            let revision: u64 = db
                .query_row("SELECT revision FROM campaign", [], |r| r.get(0))
                .unwrap();
            let tick = Instant::now();
            let step = auto_step(&mut campaign, None, &NoCandidates, Some(&policy)).unwrap();
            writer_time += tick.elapsed();
            assert!(
                matches!(step, Step::Advanced | Step::Responded { .. }),
                "{step:?}"
            );
            let text: String = db
                .query_row(
                    "SELECT command FROM commands WHERE revision=?",
                    [revision + 1],
                    |r| r.get(0),
                )
                .unwrap();
            let command: Command = serde_json::from_str(&text).unwrap();
            let tick = Instant::now();
            let transition = evaluate(&Cna::dev(), &content, &previous, &command).unwrap();
            engine_time += tick.elapsed();
            assert_eq!(
                serde_json::to_value(&transition.game).unwrap(),
                serde_json::to_value(&campaign.game).unwrap()
            );
            if let Command::Respond(response) = &command {
                submitted += 1;
                assert!(
                    Cna::dev()
                        .pending(&content, &previous.state)
                        .iter()
                        .any(|r| r.id == response.decision_id && r.seat == response.seat)
                );
                if submitted == 1 {
                    assert_eq!(response.action, first_action);
                    assert_eq!(campaign.game.rng, initial_rng);
                    let unit: UnitId = first_action[0]["unit"].as_str().unwrap().into();
                    let destination = first_action[0]["path"]
                        .as_array()
                        .unwrap()
                        .last()
                        .unwrap()
                        .as_str()
                        .unwrap();
                    assert_eq!(
                        campaign.game.state.land.units[&unit].location,
                        Location::Hex {
                            hex: destination.into()
                        }
                    );
                }
            }
            drop(db);
            let tick = Instant::now();
            let expected: Vec<_> = Perspective::all()
                .map(|p| {
                    (
                        p,
                        campaign.view(p).unwrap(),
                        campaign.metadata(p),
                        campaign.current_seq(p).unwrap(),
                    )
                })
                .collect();
            projection_time += tick.elapsed();
            boundaries += 1;
            if submitted == 8 || submitted == 16 {
                let before_hash = campaign.state_hash().unwrap();
                let before_rng = campaign.game.rng.clone();
                let before_pending = campaign.pending();
                drop(campaign);
                campaign =
                    Campaign::recover(&path, Cna::dev(), movement_fixture().0, &pins).unwrap();
                assert_eq!(campaign.state_hash().unwrap(), before_hash);
                assert_eq!(campaign.game.rng, before_rng);
                assert_eq!(campaign.pending(), before_pending);
                for (p, view, meta, seq) in expected {
                    assert_eq!(campaign.view(p).unwrap(), view);
                    assert_eq!(campaign.metadata(p), meta);
                    assert_eq!(campaign.current_seq(p).unwrap(), seq);
                }
            }
            if submitted == 16 {
                break;
            }
        }
        assert_eq!(submitted, 16);
        let mut count = 0;
        for seat in SeatId::all() {
            let rows = campaign
                .transcripts_after(Perspective::Operator, seat, 0, 512)
                .unwrap();
            for (i, row) in rows.iter().enumerate() {
                assert!(
                    matches!(row, cna_protocol::ServerMessage::Transcript { tseq, entry: cna_protocol::TranscriptEntry::DecisionSubmitted { .. }, .. } if *tseq == i as u64 + 1)
                );
            }
            count += rows.len();
            for p in Perspective::all()
                .filter(|p| !p.can_see(&cna_core::visibility::Audience::Seat(seat)))
            {
                assert!(
                    campaign
                        .transcripts_after(p, seat, 0, 512)
                        .unwrap()
                        .is_empty()
                );
            }
        }
        assert_eq!(count, submitted);
        let db = rusqlite::Connection::open(&path).unwrap();
        let resolved: usize = db
            .query_row(
                "SELECT COUNT(*) FROM decisions WHERE resolved_revision IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let commands: usize = db
            .query_row("SELECT COUNT(*) FROM commands WHERE seat != ''", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(resolved, submitted);
        assert_eq!(commands, submitted);
        let mut rows = db.prepare("SELECT p.seq, e.payload FROM perspective_events p JOIN events e USING(event_id) WHERE p.perspective=? ORDER BY p.seq").unwrap();
        for perspective in Perspective::all() {
            for (index, row) in rows
                .query_map([perspective.to_string()], |r| {
                    Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?))
                })
                .unwrap()
                .enumerate()
            {
                let (seq, payload) = row.unwrap();
                let event: cna_core::event::EngineEvent = serde_json::from_str(&payload).unwrap();
                assert_eq!(seq, index as u64 + 1);
                assert!(event.visible_to(perspective));
            }
        }
        println!(
            "bounded-movers: {submitted} decisions, {boundaries} transitions; engine={}us/decision writer(including engine+SQLite)={}us/decision all13 projections={}us/decision",
            engine_time.as_micros() / submitted as u128,
            writer_time.as_micros() / submitted as u128,
            projection_time.as_micros() / submitted as u128
        );
    }
}
