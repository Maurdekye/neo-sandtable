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
        if !matches!(
            request.kind.as_str(),
            cna_rules::land::movement::KIND
                | cna_rules::land::reaction::KIND
                | cna_rules::land::reaction::CONTINUE
        ) && request.kind != cna_rules::land::combat::assignment::KIND
            && request.kind != cna_rules::land::breakdown::window::KIND
            && request.kind != cna_rules::land::combat::retreat::KIND
            && request.kind != cna_rules::land::combat::POSITION_KIND
            && !request.kind.starts_with("cna.combat.barrage")
            && !request.kind.starts_with("cna.logistics.")
        {
            return None;
        }
        // Derive a fresh controller-local stream from the versioned request/epoch.
        // Never read, advance or replace the campaign's adjudication dice.
        let mut hash = Sha256::new();
        hash.update(
            if matches!(
                request.kind.as_str(),
                cna_rules::land::movement::KIND
                    | cna_rules::land::reaction::KIND
                    | cna_rules::land::reaction::CONTINUE
            ) {
                b"cna-scripted-movement-v1".as_slice()
            } else if request.kind == cna_rules::land::breakdown::window::KIND {
                b"cna-scripted-breakdown-v1".as_slice()
            } else if request.kind.starts_with("cna.logistics.") {
                b"cna-scripted-logistics-v1".as_slice()
            } else {
                b"cna-scripted-combat-v1".as_slice()
            },
        );
        hash.update(
            serde_json::to_vec(&(request, epoch)).expect("decision request is serializable"),
        );
        let seed = hash.finalize().into();
        let mut rng = CampaignRng::from_seed(seed);
        if let Some(action) =
            cna_rules::baseline::logistics_orders(content, state, request, &mut rng)
        {
            return Some(action);
        }
        if request.kind.starts_with("cna.combat.barrage") {
            return Some(cna_rules::baseline::random_barrages(
                content, state, request, &mut rng,
            ));
        }
        if request.kind.starts_with("cna.logistics.") {
            return None;
        }
        if request.kind == cna_rules::land::breakdown::window::KIND {
            return Some(cna_rules::baseline::random_breakdown(
                content, state, request, &mut rng,
            ));
        }
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
        if request.kind == cna_rules::land::combat::assignment::KIND {
            return Some(cna_rules::baseline::random_assignments(
                content, state, request, &mut rng,
            ));
        }
        if request.kind == cna_rules::land::combat::retreat::KIND {
            return Some(cna_rules::baseline::random_retreats(
                content, state, request, &mut rng,
            ));
        }
        if request.kind == cna_rules::land::combat::POSITION_KIND {
            return Some(cna_rules::baseline::random_positions(request, &mut rng));
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
    use cna_core::quantity::{AmmoPoints, WaterPoints};
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
            id.clone(),
            UnitSupply {
                tank_fuel: cna_rules::logistics::fuel_capacity(&content, &state, &id).unwrap(),
                ready_ammo: AmmoPoints::new(10000),
                activity_water: WaterPoints::new(10000),
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
        state.logistics.rations.insert(
            id,
            cna_rules::logistics::Rations {
                water_stage: Some(cna_rules::logistics::water::WaterStage::current(&state)),
                infantry_water_received: 2,
                issued_gt: Some(state.cursor.game_turn),
                pasta_gt: Some(state.cursor.game_turn),
                ..cna_rules::logistics::Rations::default()
            },
        );
        // Prepare the real pre-game convoy window before this synthetic movement start.
        let mut fixture_rng = CampaignRng::from_seed([3; 32]);
        let mut fixture_events = Vec::new();
        let mut cx = cna_core::engine::Cx {
            rng: &mut fixture_rng,
            events: &mut fixture_events,
        };
        cna_rules::logistics::convoys::initialize(&content, &mut state, false, &mut cx).unwrap();
        while let Some(pos) = state
            .decisions
            .pending
            .iter()
            .position(|p| p.kind.starts_with(cna_rules::logistics::convoys::PREFIX))
        {
            let pending = state.decisions.pending.remove(pos);
            cna_rules::logistics::convoys::answer(
                &content,
                &mut state,
                &pending,
                &serde_json::Value::Null,
                &mut cx,
            )
            .unwrap();
        }
        let rules = Cna::dev();
        let game = evaluate(
            &rules,
            &content,
            &Game {
                state,
                rng: fixture_rng.state(),
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

    /// Cases: land:3.6, airlog:51.11, airlog:52.13, airlog:52.41
    #[test]
    fn logistics_policy_feeds_without_spending_adjudication_rng() {
        use cna_content::scenario::Supplies;
        use cna_rules::state::{Dump, DumpLocation};
        let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut state = State::new(&content).unwrap();
        let id = UnitId::new("it.1_libyan_div.viii_libyan_bn");
        for unit in state.land.units.values_mut() {
            unit.location = if unit.id == id {
                Location::Hex {
                    hex: "C4020".into(),
                }
            } else {
                Location::NotArrived
            };
            unit.trucks = Default::default();
            unit.transport_trucks = Default::default();
        }
        state.turn.weather = Some(WeatherState {
            kind: WeatherKind::Normal,
            storm_sections: vec![],
        });
        state.cursor.op_stage = Some(1);
        state.logistics.dumps.clear();
        state.logistics.dumps.insert(
            "fixture-stock".into(),
            Dump {
                id: "fixture-stock".into(),
                marker: "fixture-marker".into(),
                side: cna_protocol::Side::Axis,
                location: DumpLocation::Hex {
                    hex: "C4020".into(),
                },
                supplies: Supplies {
                    stores: 40,
                    water: 40,
                    ..Supplies::default()
                },
                active: true,
                dummy: false,
            },
        );
        let mut dice = CampaignRng::from_seed([7; 32]);
        let mut events = vec![];
        cna_rules::logistics::stores::enter(
            &content,
            &mut state,
            &mut cna_core::engine::Cx {
                rng: &mut dice,
                events: &mut events,
            },
        )
        .unwrap();
        let request = Cna::dev()
            .pending(&content, &state)
            .into_iter()
            .find(|r| r.seat.side == cna_protocol::Side::Axis)
            .unwrap();
        let before = serde_json::to_value(&state).unwrap();
        let rng_before = dice.state();
        let policy = movement_policy();
        for epoch in 1..32 {
            let action = policy(&content, &state, &request, epoch).unwrap();
            assert_eq!(action, serde_json::json!(id));
            assert_eq!(action, policy(&content, &state, &request, epoch).unwrap());
            evaluate(
                &Cna::dev(),
                &content,
                &Game {
                    state: serde_json::from_value(before.clone()).unwrap(),
                    rng: rng_before.clone(),
                },
                &Command::Respond(cna_core::decision::DecisionResponse {
                    decision_id: request.id.clone(),
                    seat: request.seat,
                    controller_epoch: epoch,
                    decision_revision: request.revision,
                    idempotency_key: "feed".into(),
                    action,
                    public_explanation: None,
                }),
            )
            .unwrap();
        }
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        assert_eq!(dice.state(), rng_before);
    }

    /// Cases: land:3.6, land:12.23, land:12.42, land:12.45, airlog:52.42
    #[test]
    fn closed_barrage_recovers_and_adjudicates_once_with_private_persisted_streams() {
        use cna_core::{decision::DecisionResponse, visibility::Perspective};
        use cna_protocol::{GameEvent, Role, ServerMessage, Side};
        let data = cna_content::repo_data_dir();
        let content = CnaContent::load(&data, "graziani").unwrap();
        let mut state = State::new(&content).unwrap();
        let guns = [
            (
                Side::Axis,
                UnitId::new("it.1_libyan_div.1st_libyan_artillery_regt"),
                "C4218",
            ),
            (
                Side::Commonwealth,
                UnitId::new("cw.4_indian_div.25th_field_artillery_regt"),
                "C4219",
            ),
        ];
        for unit in state.land.units.values_mut() {
            unit.location = Location::NotArrived;
            unit.trucks = Default::default();
            unit.transport_trucks = Default::default();
        }
        state.logistics.dumps.clear();
        for (_, id, hex) in &guns {
            state.land.units.get_mut(id).unwrap().location = Location::Hex { hex: (*hex).into() };
            let supply = state.logistics.unit_supply.entry(id.clone()).or_default();
            supply.ready_ammo = AmmoPoints::new(10000);
            supply.activity_water = WaterPoints::new(10000);
        }
        state.turn.weather = Some(WeatherState {
            kind: WeatherKind::Normal,
            storm_sections: vec![],
        });
        state.turn.player_a = Some(Side::Axis);
        state.cursor.block = Block::PlayerHalf;
        state.cursor.half = Some(Half::A);
        state.cursor.index = 3;
        state.cursor.op_stage = Some(1);
        state.cursor.entered = false;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("closed-barrage.sqlite");
        let pins = Pins {
            rules_profile: cna_rules::PROFILE_DEV.into(),
            content_hash: "real-graziani-barrage-fixture".into(),
            engine_version: env!("CNA_ENGINE_SOURCE_HASH").into(),
        };
        let meta = CampaignMeta {
            id: "closed-barrage".into(),
            scenario_id: "graziani".into(),
            rules_profile: cna_rules::PROFILE_DEV.into(),
            title: "Closed barrage recovery".into(),
            seats: SeatId::all()
                .map(|seat| SeatInfo {
                    id: seat.to_string(),
                    side: seat.side,
                    role: seat.role,
                    controller: None,
                    status: SeatStatus::Idle,
                })
                .collect(),
        };
        let mut campaign = Campaign::create(
            &path,
            Cna::dev(),
            content,
            Game {
                state,
                rng: CampaignRng::from_seed([1; 32]).state(),
            },
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
                        label: "scripted:legal_random".into(),
                    }),
                    serde_json::json!({"mode":"legal_random"}),
                )
                .unwrap();
        }
        let submit = |campaign: &mut Campaign<Cna>,
                      request: cna_core::decision::DecisionRequest,
                      action: serde_json::Value| {
            let response = DecisionResponse {
                decision_id: request.id.clone(),
                seat: request.seat,
                controller_epoch: campaign.binding(request.seat).controller_epoch,
                decision_revision: request.revision,
                idempotency_key: request.id.to_string(),
                action,
                public_explanation: None,
            };
            campaign.validate(&response).unwrap();
            campaign.submit(response.clone()).unwrap();
            response
        };
        campaign.advance().unwrap();
        for request in campaign.pending() {
            submit(&mut campaign, request, serde_json::Value::Null);
        }
        campaign.advance().unwrap();
        for request in campaign.pending() {
            let action = if request.seat.role == Role::FrontLine {
                serde_json::json!([guns.iter().find(|g| g.0 == request.seat.side).unwrap().2])
            } else {
                serde_json::Value::Null
            };
            submit(&mut campaign, request, action);
        }
        let before_catalog_rng = campaign.game.rng.clone();
        assert!(campaign.game.state.land.combat.barrage.targets.is_empty());
        let hash = campaign.state_hash().unwrap();
        drop(campaign);
        let mut campaign = Campaign::recover(
            &path,
            Cna::dev(),
            CnaContent::load(&data, "graziani").unwrap(),
            &pins,
        )
        .unwrap();
        assert_eq!(campaign.state_hash().unwrap(), hash);
        campaign.advance().unwrap();
        assert!(!campaign.game.state.land.combat.barrage.targets.is_empty());
        let rng = campaign.game.rng.clone();
        assert_eq!(rng, before_catalog_rng);
        let mut retry = None;
        for request in campaign.pending() {
            let action = movement_policy()(
                &campaign.content,
                &campaign.game.state,
                &request,
                campaign.binding(request.seat).controller_epoch,
            )
            .unwrap();
            retry = Some(submit(&mut campaign, request, action));
        }
        assert_eq!(campaign.game.rng, rng);
        let hash = campaign.state_hash().unwrap();
        let views: Vec<_> = Perspective::all()
            .map(|p| {
                (
                    p,
                    campaign.view(p).unwrap(),
                    campaign.events_after(p, 0, 512).unwrap(),
                )
            })
            .collect();
        drop(campaign);
        let mut campaign = Campaign::recover(
            &path,
            Cna::dev(),
            CnaContent::load(&data, "graziani").unwrap(),
            &pins,
        )
        .unwrap();
        assert_eq!(campaign.state_hash().unwrap(), hash);
        assert!(campaign.submit(retry.unwrap()).unwrap().duplicate);
        for (p, view, events) in &views {
            assert_eq!(campaign.view(*p).unwrap(), *view);
            assert_eq!(campaign.events_after(*p, 0, 512).unwrap(), *events);
        }
        // A failed atomic adjudication commit must retain the accepted, closed plans,
        // with no ammo/TOE/dice or persisted command delta. Recovery then retries Advance.
        let db = rusqlite::Connection::open(&path).unwrap();
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0))
            .unwrap();
        db.execute_batch("CREATE TRIGGER fail_barrage_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'fail barrage commit'); END;").unwrap();
        assert!(campaign.advance().is_err());
        assert_eq!(campaign.state_hash().unwrap(), hash);
        assert_eq!(campaign.game.rng, rng);
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            count
        );
        db.execute_batch("DROP TRIGGER fail_barrage_event").unwrap();
        drop(db);
        drop(campaign);
        let mut campaign = Campaign::recover(
            &path,
            Cna::dev(),
            CnaContent::load(&data, "graziani").unwrap(),
            &pins,
        )
        .unwrap();
        campaign.advance().unwrap();
        assert!(campaign.game.state.land.combat.barrage.resolved);
        assert_ne!(campaign.game.rng, rng);
        for (_, id, _) in &guns {
            assert_eq!(
                campaign.game.state.logistics.unit_supply[id]
                    .ready_ammo
                    .get(),
                9976
            );
            assert_eq!(
                campaign.game.state.logistics.unit_supply[id]
                    .activity_water
                    .get(),
                9994
            );
        }
        for p in Perspective::all() {
            let events = campaign.events_after(p, 0, 512).unwrap();
            let mut expected = 1;
            for event in events {
                if let ServerMessage::Event { seq, event, .. } = event {
                    assert_eq!(seq, expected);
                    expected += 1;
                    if let GameEvent::Note { text } = event
                        && text.contains("Own barrage plot")
                    {
                        let enemy = match p {
                            Perspective::Side(side) => Some(side.opponent()),
                            Perspective::Seat(seat) => Some(seat.side.opponent()),
                            _ => None,
                        };
                        if let Some(enemy) = enemy {
                            let enemy_id = &guns.iter().find(|g| g.0 == enemy).unwrap().1;
                            assert!(!text.contains(enemy_id.as_str()));
                        }
                    }
                }
            }
        }
        let game = serde_json::to_value(&campaign.game).unwrap();
        let hash = campaign.state_hash().unwrap();
        drop(campaign);
        let campaign = Campaign::recover(
            &path,
            Cna::dev(),
            CnaContent::load(&data, "graziani").unwrap(),
            &pins,
        )
        .unwrap();
        assert_eq!(campaign.state_hash().unwrap(), hash);
        assert_eq!(serde_json::to_value(&campaign.game).unwrap(), game);
    }

    /// Cases: land:3.6,land:13.21,land:13.24,land:13.28
    #[test]
    fn closed_retreat_recovers_before_atomic_execution_and_baseline_is_rng_isolated() {
        use cna_core::decision::DecisionResponse;
        use cna_core::visibility::Perspective;
        let (content, mut game) = movement_fixture();
        let id: UnitId = "cw.unassigned_inf.1st_rnf_mg_bn".into();
        for unit in game.state.land.units.values_mut() {
            unit.location = Location::Eliminated;
        }
        let unit = game.state.land.units.get_mut(&id).unwrap();
        unit.location = Location::Hex {
            hex: "C4020".into(),
        };
        unit.detached = true;
        unit.attached_to = None;
        game.state.logistics.unit_supply.insert(
            id.clone(),
            UnitSupply {
                ready_ammo: AmmoPoints::new(10000),
                activity_water: WaterPoints::new(10000),
                ..Default::default()
            },
        );
        game.state.logistics.rations.insert(
            id.clone(),
            cna_rules::logistics::Rations {
                water_stage: Some(cna_rules::logistics::water::WaterStage::current(
                    &game.state,
                )),
                infantry_water_received: 2,
                issued_gt: Some(game.state.cursor.game_turn),
                pasta_gt: Some(game.state.cursor.game_turn),
                ..Default::default()
            },
        );
        game.state.decisions.pending.clear();
        game.state.cursor.index = cna_rules::seq::PLAYER_HALF
            .iter()
            .position(|step| step.anchor == cna_rules::land::combat::retreat::ANCHOR)
            .unwrap();
        game.state.cursor.entered = false;
        let rng = game.rng.clone();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("retreat.sqlite");
        let pins = Pins {
            rules_profile: cna_rules::PROFILE_DEV.into(),
            content_hash: "real-roster-retreat-test-map".into(),
            engine_version: env!("CNA_ENGINE_SOURCE_HASH").into(),
        };
        let meta = CampaignMeta {
            id: "retreat".into(),
            scenario_id: "graziani".into(),
            rules_profile: cna_rules::PROFILE_DEV.into(),
            title: "Retreat recovery".into(),
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
        let mut campaign =
            Campaign::create(&path, Cna::dev(), content, game, meta, pins.clone()).unwrap();
        campaign.advance().unwrap();
        let policy = movement_policy();
        let mut retry = None;
        let mut expected_move = None;
        for request in campaign.pending() {
            let (epoch, action) = if request.seat.role == cna_protocol::Role::FrontLine {
                (1..64)
                    .find_map(|epoch| {
                        let action =
                            policy(&campaign.content, &campaign.game.state, &request, epoch)
                                .unwrap();
                        action
                            .as_array()
                            .is_some_and(|a| !a.is_empty())
                            .then_some((epoch, action))
                    })
                    .expect("scripted retreat chooses a real path")
            } else {
                (1, serde_json::Value::Null)
            };
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
            if let Some(path) = action
                .as_array()
                .and_then(|a| a.first())
                .and_then(|a| a["path"].as_array())
            {
                expected_move = Some(path.last().unwrap().as_str().unwrap().to_owned());
            }
            let response = DecisionResponse {
                decision_id: request.id.clone(),
                seat: request.seat,
                controller_epoch: campaign.binding(request.seat).controller_epoch,
                decision_revision: request.revision,
                idempotency_key: request.id.to_string(),
                action,
                public_explanation: None,
            };
            campaign.validate(&response).unwrap();
            campaign.submit(response.clone()).unwrap();
            retry = Some(response);
            assert_eq!(campaign.game.rng, rng);
            assert_eq!(
                campaign.game.state.land.units[&id].location.hex(),
                Some(&"C4020".into())
            );
        }
        let hash = campaign.state_hash().unwrap();
        let expected: Vec<_> = Perspective::all()
            .map(|p| {
                (
                    p,
                    campaign.view(p).unwrap(),
                    campaign.events_after(p, 0, 512).unwrap(),
                )
            })
            .collect();
        drop(campaign);
        let mut campaign =
            Campaign::recover(&path, Cna::dev(), movement_fixture().0, &pins).unwrap();
        assert_eq!(campaign.state_hash().unwrap(), hash);
        assert!(campaign.submit(retry.unwrap()).unwrap().duplicate);
        for (p, view, events) in expected {
            assert_eq!(campaign.view(p).unwrap(), view);
            assert_eq!(campaign.events_after(p, 0, 512).unwrap(), events);
        }
        let db = rusqlite::Connection::open(&path).unwrap();
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0))
            .unwrap();
        db.execute_batch("CREATE TRIGGER fail_retreat BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'fail retreat'); END;").unwrap();
        assert!(campaign.advance().is_err());
        assert_eq!(campaign.state_hash().unwrap(), hash);
        assert_eq!(campaign.game.rng, rng);
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            count
        );
        db.execute_batch("DROP TRIGGER fail_retreat").unwrap();
        drop(db);
        campaign.advance().unwrap();
        assert_eq!(
            campaign.game.state.land.units[&id]
                .location
                .hex()
                .unwrap()
                .as_str(),
            expected_move.unwrap()
        );
        assert!(
            campaign
                .game
                .state
                .land
                .combat
                .retreat
                .retreated
                .contains(&id)
        );
        assert_eq!(campaign.game.rng, rng);
        let hash = campaign.state_hash().unwrap();
        let cp = campaign.game.state.land.units[&id].cp_spent_quarters;
        assert!(cp > 0);
        drop(campaign);
        let campaign = Campaign::recover(&path, Cna::dev(), movement_fixture().0, &pins).unwrap();
        assert_eq!(campaign.state_hash().unwrap(), hash);
        assert_eq!(campaign.game.state.land.units[&id].cp_spent_quarters, cp);
        let db = rusqlite::Connection::open(&path).unwrap();
        let mut rows=db.prepare("SELECT p.seq,e.payload FROM perspective_events p JOIN events e USING(event_id) WHERE p.perspective=? ORDER BY p.seq").unwrap();
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
    }

    /// Real force plans survive a closed-window checkpoint, exact retry and failed commits.
    /// Cases: land:3.6,land:14.26,land:15.16,airlog:50.14
    #[test]
    fn closed_force_assignment_is_durable_private_and_spends_no_stocks_or_dice() {
        use cna_core::{decision::DecisionResponse, visibility::Perspective};
        use cna_protocol::{Role, Side};
        let (content, mut game) = movement_fixture();
        for unit in game.state.land.units.values_mut() {
            unit.location = Location::Eliminated;
        }
        for (id, hex) in [
            ("it.libyan_tank_command.i_m_tank_bn", "C4020"),
            ("cw.unassigned_inf.1st_rnf_mg_bn", "C4021"),
        ] {
            let id: UnitId = id.into();
            let unit = game.state.land.units.get_mut(&id).unwrap();
            unit.location = Location::Hex { hex: hex.into() };
            unit.detached = true;
            unit.attached_to = None;
            game.state.logistics.unit_supply.insert(
                id.clone(),
                UnitSupply {
                    ready_ammo: AmmoPoints::new(10000),
                    activity_water: WaterPoints::new(10000),
                    ..Default::default()
                },
            );
            game.state.logistics.rations.insert(
                id,
                cna_rules::logistics::Rations {
                    water_stage: Some(cna_rules::logistics::water::WaterStage::current(
                        &game.state,
                    )),
                    infantry_water_received: 2,
                    issued_gt: Some(game.state.cursor.game_turn),
                    pasta_gt: Some(game.state.cursor.game_turn),
                    ..Default::default()
                },
            );
        }
        game.state.turn.player_a = Some(Side::Axis);
        game.state.decisions.pending.clear();
        game.state.cursor.index = cna_rules::seq::PLAYER_HALF
            .iter()
            .position(|x| x.anchor == cna_rules::land::combat::assignment::ANCHOR)
            .unwrap();
        game.state.cursor.entered = false;
        let rng = game.rng.clone();
        let holdings = serde_json::to_value(&game.state.logistics).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("force.sqlite");
        let pins = Pins {
            rules_profile: cna_rules::PROFILE_DEV.into(),
            content_hash: "force-real-roster-test-map".into(),
            engine_version: env!("CNA_ENGINE_SOURCE_HASH").into(),
        };
        let meta = CampaignMeta {
            id: "force".into(),
            scenario_id: "graziani".into(),
            rules_profile: cna_rules::PROFILE_DEV.into(),
            title: "Force plan recovery".into(),
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
        let mut campaign =
            Campaign::create(&path, Cna::dev(), content, game, meta, pins.clone()).unwrap();
        campaign.advance().unwrap();
        let policy = movement_policy();
        let mut retry = None;
        for request in campaign.pending() {
            assert_eq!(request.seat.role, Role::FrontLine);
            let (epoch, action) = (1..64)
                .find_map(|epoch| {
                    let action =
                        policy(&campaign.content, &campaign.game.state, &request, epoch).unwrap();
                    assert_eq!(
                        action,
                        policy(&campaign.content, &campaign.game.state, &request, epoch).unwrap()
                    );
                    action
                        .as_array()
                        .is_some_and(|a| !a.is_empty())
                        .then_some((epoch, action))
                })
                .expect("actual force baseline supplies a nonempty plan");
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
            let response = DecisionResponse {
                decision_id: request.id.clone(),
                seat: request.seat,
                controller_epoch: campaign.binding(request.seat).controller_epoch,
                decision_revision: request.revision,
                idempotency_key: request.id.to_string(),
                action,
                public_explanation: None,
            };
            campaign.validate(&response).unwrap();
            let before = campaign.state_hash().unwrap();
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch("CREATE TRIGGER fail_force BEFORE INSERT ON commands BEGIN SELECT RAISE(ABORT, 'force failure'); END;").unwrap();
            assert!(campaign.submit(response.clone()).is_err());
            assert_eq!(campaign.state_hash().unwrap(), before);
            db.execute_batch("DROP TRIGGER fail_force").unwrap();
            drop(db);
            campaign.submit(response.clone()).unwrap();
            retry = Some(response);
            assert_eq!(campaign.game.rng, rng);
            assert_eq!(
                serde_json::to_value(&campaign.game.state.logistics).unwrap(),
                holdings
            );
        }
        assert!(campaign.game.state.land.combat.assignment.closed);
        assert!(!campaign.game.state.land.combat.assignment.frozen);
        let hash = campaign.state_hash().unwrap();
        let expected: Vec<_> = Perspective::all()
            .map(|p| {
                (
                    p,
                    campaign.view(p).unwrap(),
                    campaign.events_after(p, 0, 512).unwrap(),
                )
            })
            .collect();
        drop(campaign);
        let mut campaign =
            Campaign::recover(&path, Cna::dev(), movement_fixture().0, &pins).unwrap();
        assert_eq!(campaign.state_hash().unwrap(), hash);
        assert!(campaign.submit(retry.unwrap()).unwrap().duplicate);
        for (p, view, events) in expected {
            assert_eq!(campaign.view(p).unwrap(), view);
            assert_eq!(campaign.events_after(p, 0, 512).unwrap(), events);
            let observation = Cna::dev().observe(&campaign.content, &campaign.game.state, p);
            let disclosed = &observation["combat"]["force_assignment"]["plans"];
            for side in Side::ALL {
                if p != Perspective::Operator
                    && !p.can_see(&cna_core::visibility::Audience::Side(side))
                {
                    assert!(
                        !disclosed
                            .as_object()
                            .unwrap()
                            .contains_key(&side.to_string())
                    );
                }
            }
        }
        let db = rusqlite::Connection::open(&path).unwrap();
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0))
            .unwrap();
        db.execute_batch("CREATE TRIGGER fail_freeze BEFORE INSERT ON commands BEGIN SELECT RAISE(ABORT, 'freeze failure'); END;").unwrap();
        assert!(campaign.advance().is_err());
        assert_eq!(campaign.state_hash().unwrap(), hash);
        assert_eq!(campaign.game.rng, rng);
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            count
        );
        db.execute_batch("DROP TRIGGER fail_freeze").unwrap();
        drop(db);
        campaign.advance().unwrap();
        assert!(campaign.game.state.land.combat.assignment.frozen);
        assert_eq!(campaign.game.rng, rng);
        assert_eq!(
            serde_json::to_value(&campaign.game.state.logistics).unwrap(),
            holdings
        );
        let hash = campaign.state_hash().unwrap();
        drop(campaign);
        let campaign = Campaign::recover(&path, Cna::dev(), movement_fixture().0, &pins).unwrap();
        assert_eq!(campaign.state_hash().unwrap(), hash);
        let db = rusqlite::Connection::open(&path).unwrap();
        let mut rows=db.prepare("SELECT p.seq,e.payload FROM perspective_events p JOIN events e USING(event_id) WHERE p.perspective=? ORDER BY p.seq").unwrap();
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
    }

    /// Exercise the real actor dispatcher, SQLite recovery, and mandatory loss continuation.
    /// Cases: land:21.24, land:21.31, land:21.43, land:3.6
    #[test]
    fn mandatory_breakdown_actor_dispatch_allocates_and_resumes_in_both_scripted_modes() {
        use crate::{
            actor::auto_step,
            scripted::{NoCandidates, Step},
        };
        use cna_core::decision::DecisionResponse;
        for mode in ["pass_when_possible", "legal_random"] {
            let (content, mut game) = movement_fixture();
            let id: UnitId = "it.libyan_tank_command.xxi_l_tank_bn".into();
            // A saved earlier journey makes the next completed actual move require a roll.
            cna_rules::land::breakdown::record_edge(
                &mut game.state,
                &id,
                &"C4020".into(),
                280,
                8,
                WeatherKind::Normal,
            )
            .unwrap();
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("mandatory-dispatch.sqlite");
            let pins = Pins {
                rules_profile: cna_rules::PROFILE_DEV.into(),
                content_hash: "real-roster-test-map".into(),
                engine_version: env!("CNA_ENGINE_SOURCE_HASH").into(),
            };
            let meta = CampaignMeta {
                id: "mandatory-dispatch".into(),
                scenario_id: "graziani".into(),
                rules_profile: pins.rules_profile.clone(),
                title: "Mandatory dispatch".into(),
                seats: vec![],
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
            let request = campaign
                .pending()
                .into_iter()
                .find(|r| r.kind == cna_rules::land::movement::KIND)
                .unwrap();
            campaign
                .handover(
                    request.seat,
                    Some(ControllerInfo {
                        kind: ControllerKind::Scripted,
                        label: format!("scripted:{mode}"),
                    }),
                    serde_json::json!({"mode":mode}),
                )
                .unwrap();
            let before_roll = campaign.game.rng.clone();
            campaign
                .submit(DecisionResponse {
                    decision_id: request.id,
                    seat: request.seat,
                    controller_epoch: campaign.binding(request.seat).controller_epoch,
                    decision_revision: request.revision,
                    idempotency_key: "actual-move".into(),
                    action: serde_json::json!([{"unit":id,"path":["C4021"]}]),
                    public_explanation: None,
                })
                .unwrap();
            let policy = movement_policy();
            for _ in 0..4 {
                if campaign
                    .pending()
                    .iter()
                    .any(|r| r.kind == cna_rules::land::breakdown::window::KIND)
                {
                    break;
                }
                assert!(matches!(
                    auto_step(&mut campaign, None, &NoCandidates, Some(&policy)).unwrap(),
                    Step::Advanced
                ));
            }
            let loss = campaign
                .pending()
                .into_iter()
                .find(|r| r.kind == cna_rules::land::breakdown::window::KIND)
                .expect("real completed move opens a mandatory loss window");
            assert!(loss.space.pass.is_none());
            assert_eq!(loss.seat, request.seat);
            assert_ne!(
                campaign.game.rng, before_roll,
                "breakdown dice must actually roll"
            );
            let hash = campaign.state_hash().unwrap();
            let rng = campaign.game.rng.clone();
            drop(campaign);
            campaign = Campaign::recover(&path, Cna::dev(), movement_fixture().0, &pins).unwrap();
            assert_eq!(campaign.state_hash().unwrap(), hash);
            assert_eq!(campaign.game.rng, rng);
            assert_eq!(campaign.pending()[0], loss);
            let mut allocations = 0;
            for _ in 0..16 {
                let pending_loss = campaign
                    .pending()
                    .into_iter()
                    .find(|r| r.kind == cna_rules::land::breakdown::window::KIND);
                let previous = campaign.game.clone();
                let step = auto_step(&mut campaign, None, &NoCandidates, Some(&policy)).unwrap();
                if let Some(loss) = pending_loss {
                    assert!(
                        matches!(step, Step::Responded { seat } if seat == loss.seat),
                        "{mode}: {step:?}"
                    );
                    assert_eq!(
                        campaign.game.rng, previous.rng,
                        "allocation cannot draw campaign dice"
                    );
                    let db = rusqlite::Connection::open(&path).unwrap();
                    let text: String = db
                        .query_row(
                            "SELECT command FROM commands ORDER BY revision DESC LIMIT 1",
                            [],
                            |r| r.get(0),
                        )
                        .unwrap();
                    let command: Command = serde_json::from_str(&text).unwrap();
                    let Command::Respond(response) = &command else {
                        panic!("allocator must submit a real response")
                    };
                    assert_eq!(response.decision_id, loss.id);
                    assert!(
                        !response.action.is_null(),
                        "compulsory losses have no fabricated pass"
                    );
                    let expected = evaluate(&Cna::dev(), &content, &previous, &command).unwrap();
                    assert_eq!(
                        serde_json::to_value(expected.game).unwrap(),
                        serde_json::to_value(&campaign.game).unwrap()
                    );
                    allocations += 1;
                } else {
                    assert!(matches!(step, Step::Advanced), "{mode}: {step:?}");
                }
                assert!(
                    !campaign.binding(request.seat).paused,
                    "{mode}: allocator must not park the driver"
                );
                if !campaign.game.state.land.breakdown.window.parked
                    && campaign.game.state.land.breakdown.stopped.is_empty()
                    && campaign
                        .game
                        .state
                        .land
                        .breakdown
                        .window
                        .outcomes
                        .is_empty()
                {
                    break;
                }
            }
            assert!(allocations > 0);
            assert!(!campaign.game.state.land.breakdown.window.parked);
            assert!(campaign.game.state.land.breakdown.stopped.is_empty());
            assert!(
                !campaign.game.state.land.breakdown.markers.is_empty(),
                "physical loss application must complete"
            );
            assert!(
                !campaign
                    .pending()
                    .iter()
                    .any(|r| r.kind == cna_rules::land::breakdown::window::KIND)
            );
            let hash = campaign.state_hash().unwrap();
            let rng = campaign.game.rng.clone();
            let views: Vec<_> = cna_core::visibility::Perspective::all()
                .map(|p| {
                    (
                        p,
                        campaign.view(p).unwrap(),
                        campaign.current_seq(p).unwrap(),
                    )
                })
                .collect();
            drop(campaign);
            let campaign = Campaign::recover(&path, Cna::dev(), content, &pins).unwrap();
            assert_eq!(campaign.state_hash().unwrap(), hash);
            assert_eq!(campaign.game.rng, rng);
            for (p, view, seq) in views {
                assert_eq!(campaign.view(p).unwrap(), view);
                assert_eq!(campaign.current_seq(p).unwrap(), seq);
            }
        }
    }

    /// Mandatory loss policies return exact real allocations before generic pass/generation handling.
    /// Cases: land:21.24,land:21.31,land:21.43,land:3.6
    #[test]
    fn mandatory_breakdown_policy_is_legal_deterministic_and_rng_isolated() {
        use cna_core::decision::DecisionResponse;
        let (content, mut game) = movement_fixture();
        let id: UnitId = "it.libyan_tank_command.xxi_l_tank_bn".into();
        cna_rules::land::breakdown::record_edge(
            &mut game.state,
            &id,
            &"C4020".into(),
            280,
            8,
            WeatherKind::Normal,
        )
        .unwrap();
        let r = Cna::dev()
            .pending(&content, &game.state)
            .into_iter()
            .find(|r| r.kind == cna_rules::land::movement::KIND)
            .unwrap();
        game = evaluate(
            &Cna::dev(),
            &content,
            &game,
            &Command::Respond(DecisionResponse {
                decision_id: r.id.clone(),
                seat: r.seat,
                controller_epoch: 1,
                decision_revision: r.revision,
                idempotency_key: "actual-move".into(),
                action: serde_json::json!([{"unit":id,"path":["C4021"]}]),
                public_explanation: None,
            }),
        )
        .unwrap()
        .game;
        if !Cna::dev()
            .pending(&content, &game.state)
            .iter()
            .any(|r| r.kind == cna_rules::land::breakdown::window::KIND)
        {
            game = evaluate(&Cna::dev(), &content, &game, &Command::Advance)
                .unwrap()
                .game;
        }
        let r = Cna::dev()
            .pending(&content, &game.state)
            .into_iter()
            .find(|r| r.kind == cna_rules::land::breakdown::window::KIND)
            .expect("actual move opens private mandatory losses");
        assert!(r.space.pass.is_none());
        let before = serde_json::to_value(&game).unwrap();
        let policy = movement_policy();
        for epoch in 1..16 {
            let action = policy(&content, &game.state, &r, epoch).unwrap();
            assert!(!action.is_null());
            assert_eq!(action, policy(&content, &game.state, &r, epoch).unwrap());
            let batch = evaluate(
                &Cna::dev(),
                &content,
                &game,
                &Command::Respond(DecisionResponse {
                    decision_id: r.id.clone(),
                    seat: r.seat,
                    controller_epoch: epoch,
                    decision_revision: r.revision,
                    idempotency_key: "actual-loss-choice".into(),
                    action,
                    public_explanation: None,
                }),
            )
            .unwrap();
            assert_eq!(batch.game.rng, game.rng);
        }
        assert_eq!(serde_json::to_value(&game).unwrap(), before);
    }
}
