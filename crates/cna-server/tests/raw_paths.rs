//! RAW printed-counter paths must survive the real SQLite command boundary.
//! Cases: land:3.62, land:8.11, land:13.21, land:13.24; interp:land-0030.
use cna_content::map::{LineKind, MapContent, SideKind};
use cna_core::{
    decision::{DecisionRequest, DecisionResponse},
    dice::CampaignRng,
    engine::{Command, Game, Ruleset, evaluate},
    ids::{SeatId, UnitId},
    quantity::{AmmoPoints, FuelTenths, WaterPoints},
    visibility::Perspective,
};
use cna_protocol::{
    CampaignMeta, ControllerInfo, ControllerKind, GameEvent, Role, ServerMessage, Side,
};
use cna_rules::{
    Cna, CnaContent, State,
    seq::{Block, Half, PLAYER_HALF},
    state::{Location, UnitSupply, WeatherState},
};
use cna_server::{Campaign, Pins};
use rusqlite::{Connection, types::Value as SqlValue};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};

const ROOT: &str = "cw.unassigned_inf.1st_rnf_mg_bn";
const COUNTER: &str = "cw.2_nz_div.21st_nz_bn";
const ATTACHED: &str = "cw.2_nz_div.22nd_nz_bn";
const PARENT: &str = "cw.2_nz_div.5th_new_zealand_bde_hq";
const RBA: &str = "opstage.movement_and_combat.combat.retreat_before_assault";

fn retain_fixture_port_geometry(content: &mut CnaContent) {
    // Keep port metadata within this synthetic fixture's geometry; not production normalization.
    content
        .scenario
        .construction
        .port_overrides
        .retain(|record| content.map.canonical(&record.hex) == Some(&record.hex));
    content.places.places.retain(|_, place| {
        place.kind != "port" || content.map.canonical(&place.hex_id) == Some(&place.hex_id)
    });
}

fn copy_content(content: &CnaContent) -> CnaContent {
    let mut copy = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    copy.map = content.map.clone();
    retain_fixture_port_geometry(&mut copy);
    copy
}
fn game_hash(game: &Game<Cna>) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(game).unwrap()))
}
fn rows(path: &Path) -> BTreeMap<String, Vec<Vec<SqlValue>>> {
    let db = Connection::open(path).unwrap();
    let names = db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap()
        .query_map([], |r| r.get::<_, String>(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            let mut statement = db.prepare(&format!("SELECT * FROM \"{name}\"")).unwrap();
            let count = statement.column_count();
            let mut rows = statement
                .query_map([], |r| {
                    (0..count)
                        .map(|i| r.get(i))
                        .collect::<Result<Vec<SqlValue>, _>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            rows.sort_by_key(|row| format!("{row:?}"));
            (name, rows)
        })
        .collect()
}

fn fixture(retreat: bool) -> (CnaContent, Game<Cna>, Option<Game<Cna>>) {
    let data = cna_content::repo_data_dir();
    let mut content = CnaContent::load(&data, "graziani").unwrap();
    let (mut game, expected) = if let Some(dir) = std::env::var_os("CNA_RAW_PATH_FIXTURE_DIR") {
        // Optional exact owner-supplied scratch fixture. Never copy its whole roster into git.
        let dir = std::path::PathBuf::from(dir);
        let supplied: Value =
            serde_json::from_slice(&fs::read(dir.join("fixture.json")).unwrap()).unwrap();
        content.map = MapContent::load(&dir.join("map")).unwrap();
        let response: DecisionResponse =
            serde_json::from_value(supplied["response"].clone()).unwrap();
        assert_eq!(
            response.action,
            json!([{"unit":ROOT,"with_stack":true,"path":["C4021","C4022"]}])
        );
        (
            serde_json::from_value::<Game<Cna>>(supplied["game"].clone()).unwrap(),
            Some(serde_json::from_value::<Game<Cna>>(supplied["expected_game"].clone()).unwrap()),
        )
    } else {
        let mut state = State::new(&content).unwrap();
        // Synthetic known road on the real coordinate grid; no published map inference.
        let dir = tempfile::tempdir().unwrap();
        for file in ["layers.toml", "sections.toml"] {
            fs::copy(data.join("map").join(file), dir.path().join(file)).unwrap();
        }
        let mut hexes = String::from("hex_id,section,q,r,terrain,flags\n");
        let mut coverage = String::from("layer,hex_id,neighbour_id,src,review_batch\n");
        for id in ["C4020", "C4021", "C4022"] {
            let h = content.map.get(&id.into()).unwrap();
            hexes += &format!("{id},C,{},{},clear,\n", h.axial.q, h.axial.r);
            coverage += &format!("terrain,{id},,land:8.37,test\n");
        }
        for (a, b) in [("C4020", "C4021"), ("C4021", "C4022")] {
            for kind in LineKind::ALL {
                coverage += &format!("line:{},{a},{b},land:8.33,test\n", kind.name());
            }
            for kind in SideKind::ALL {
                coverage += &format!("side:{},{a},{b},land:8.35,test\n", kind.name());
            }
        }
        fs::write(dir.path().join("hexes.csv"), hexes).unwrap();
        fs::write(dir.path().join("coverage.csv"), coverage).unwrap();
        fs::write(dir.path().join("aliases.csv"), "alias_id,hex_id,src\n").unwrap();
        fs::write(dir.path().join("line_features.csv"),"from_hex,to_hex,kind,src,review_batch\nC4020,C4021,road,land:8.33,test\nC4021,C4022,road,land:8.33,test\n").unwrap();
        fs::write(
            dir.path().join("hexsides.csv"),
            "hex_id,direction,neighbour_id,feature,high_side,src,review_batch\n",
        )
        .unwrap();
        content.map = MapContent::load(dir.path()).unwrap();
        // The default synthetic fixture contains only these three hexes.
        state
            .logistics
            .dumps
            .retain(|_, dump| match &dump.location {
                cna_rules::state::DumpLocation::Hex { hex } => content.map.canonical(hex).is_some(),
                _ => true,
            });
        for unit in state.land.units.values_mut() {
            unit.location = Location::Eliminated;
        }
        state.turn.player_a = Some(Side::Commonwealth);
        state.turn.weather = Some(WeatherState {
            kind: cna_tables::land::weather::WeatherKind::Normal,
            storm_sections: vec![],
        });
        state.cursor.block = Block::PlayerHalf;
        state.cursor.half = Some(Half::A);
        state.cursor.op_stage = Some(1);
        state.cursor.index = 1;
        state.cursor.entered = false;
        for id in [ROOT, COUNTER, ATTACHED] {
            let id = UnitId::new(id);
            let unit = state.land.units.get_mut(&id).unwrap();
            unit.location = Location::Hex {
                hex: "C4020".into(),
            };
            unit.detached = true;
            unit.attached_to = None;
            state.logistics.unit_supply.insert(
                id.clone(),
                UnitSupply {
                    activity_water: WaterPoints::new(10000),
                    ready_ammo: AmmoPoints::new(10000),
                    tank_fuel: FuelTenths::new(10000),
                    ..Default::default()
                },
            );
            state.logistics.rations.insert(
                id,
                cna_rules::logistics::Rations {
                    water_stage: Some(cna_rules::logistics::water::WaterStage::current(&state)),
                    infantry_water_received: 2,
                    issued_gt: Some(state.cursor.game_turn),
                    pasta_gt: Some(state.cursor.game_turn),
                    ..Default::default()
                },
            );
        }
        let attached = state.land.units.get_mut(&ATTACHED.into()).unwrap();
        attached.attached_to = Some(ROOT.into());
        attached.detached = false;
        let game = Game {
            state,
            rng: CampaignRng::from_seed([3; 32]).state(),
        };
        (
            evaluate(&Cna::dev(), &content, &game, &Command::Advance)
                .unwrap()
                .game,
            None,
        )
    };
    retain_fixture_port_geometry(&mut content);
    if retreat {
        game.state.decisions.pending.clear();
        game.state.turn.player_a = Some(Side::Axis);
        game.state.cursor.index = PLAYER_HALF.iter().position(|s| s.anchor == RBA).unwrap();
        game.state.cursor.entered = false;
        game = evaluate(&Cna::dev(), &content, &game, &Command::Advance)
            .unwrap()
            .game;
    }
    (content, game, expected)
}
// A real printed child can be visible while remote, then disappear into its own parent.
// The parent is stationary: this tests disclosure across attachment, never HQ movement.
fn joining_fixture(retreat: bool) -> (CnaContent, Game<Cna>, Option<Game<Cna>>) {
    let (content, mut game, _) = fixture(false);
    assert_eq!(
        content.units.units[&COUNTER.into()].parent.as_ref(),
        Some(&PARENT.into())
    );
    for id in [ROOT, ATTACHED] {
        game.state.land.units.get_mut(&id.into()).unwrap().location = Location::NotArrived;
    }
    let child = game.state.land.units.get_mut(&COUNTER.into()).unwrap();
    child.detached = false;
    child.attached_to = None;
    let parent = game.state.land.units.get_mut(&PARENT.into()).unwrap();
    parent.location = Location::Hex {
        hex: "C4022".into(),
    };
    parent.detached = true;
    parent.attached_to = None;
    game.state.decisions.pending.clear();
    game.state.turn.player_a = Some(if retreat {
        Side::Axis
    } else {
        Side::Commonwealth
    });
    game.state.cursor.index = if retreat {
        PLAYER_HALF.iter().position(|s| s.anchor == RBA).unwrap()
    } else {
        1
    };
    game.state.cursor.entered = false;
    game = evaluate(&Cna::dev(), &content, &game, &Command::Advance)
        .unwrap()
        .game;
    for p in Perspective::all().filter(|p| p.side() == Some(Side::Axis)) {
        assert!(
            Cna::dev()
                .view(&content, &game.state, p)
                .units
                .contains_key(COUNTER)
        );
    }
    (content, game, None)
}
fn response(request: &DecisionRequest, action: Value, key: &str) -> DecisionResponse {
    DecisionResponse {
        decision_id: request.id.clone(),
        seat: request.seat,
        controller_epoch: 1,
        decision_revision: request.revision,
        idempotency_key: key.into(),
        action,
        public_explanation: None,
    }
}
fn streams(campaign: &Campaign<Cna>) -> Vec<(Perspective, Vec<ServerMessage>)> {
    Perspective::all()
        .map(|p| {
            let frames = campaign.events_after(p, 0, 512).unwrap();
            assert_eq!(
                frames.len() as u64,
                campaign.current_seq(p).unwrap(),
                "full {p} stream"
            );
            (p, frames)
        })
        .collect()
}
fn assert_paths(campaign: &Campaign<Cna>, joining: bool) {
    for (p, stream) in streams(campaign) {
        for (index, message) in stream.iter().enumerate() {
            assert!(
                matches!(message,ServerMessage::Event{seq,..} if *seq==index as u64+1),
                "{p}"
            );
        }
        let enemy = p.side() == Some(Side::Axis);
        let mut actual = stream
            .iter()
            .filter_map(|message| match message {
                ServerMessage::Event {
                    event:
                        GameEvent::UnitMoved {
                            unit_id,
                            path,
                            cp_spent,
                        },
                    hex,
                    unit_id: locator,
                    ..
                } => {
                    if enemy {
                        assert_eq!(hex.as_deref(), Some("C4022"));
                        assert_eq!(locator.as_deref(), Some(unit_id.as_str()));
                        assert_ne!(unit_id, ATTACHED);
                    }
                    Some((unit_id.clone(), path.clone(), *cp_spent))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        actual.sort();
        let mut expected = if joining {
            vec![COUNTER]
        } else if enemy {
            vec![ROOT, COUNTER]
        } else {
            vec![ROOT, COUNTER, ATTACHED]
        }
        .into_iter()
        .map(|id| {
            (
                id.to_owned(),
                vec!["C4021".into(), "C4022".into()],
                if enemy { None } else { Some(2) },
            )
        })
        .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(
            actual, expected,
            "{p}: exact paths, CP redaction and no duplicate member copies"
        );
        if enemy {
            assert!(!serde_json::to_string(&stream).unwrap().contains(ATTACHED));
        }
        if joining && enemy {
            assert!(!campaign.view(p).unwrap().units.contains_key(COUNTER));
            let removals = stream
                .iter()
                .filter_map(|m| match m {
                    ServerMessage::Event {
                        event: GameEvent::UnitRemoved { unit_id, reason },
                        ..
                    } if unit_id == COUNTER => Some(reason.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(removals, vec![""], "{p}: attachment removal is reasonless");
            let moved = stream
                .iter()
                .position(|m| {
                    matches!(m,
                ServerMessage::Event { event: GameEvent::UnitMoved { unit_id, .. }, .. }
                    if unit_id == COUNTER)
                })
                .unwrap();
            let removed = stream
                .iter()
                .position(|m| {
                    matches!(m,
                ServerMessage::Event { event: GameEvent::UnitRemoved { unit_id, .. }, .. }
                    if unit_id == COUNTER)
                })
                .unwrap();
            assert!(
                moved < removed,
                "{p}: actual route precedes visibility removal"
            );
        }
    }
}
fn apply(
    campaign: &mut Campaign<Cna>,
    content: &CnaContent,
    reference: &mut Game<Cna>,
    command: Command,
) {
    let next = evaluate(&Cna::dev(), content, reference, &command)
        .unwrap()
        .game;
    match command {
        Command::Respond(response) => {
            assert!(!campaign.submit(response).unwrap().duplicate);
        }
        Command::Advance => {
            campaign.advance().unwrap();
        }
    }
    *reference = next;
    assert_eq!(campaign.state_hash().unwrap(), game_hash(reference));
}
fn recover_exact(
    path: &Path,
    campaign: Campaign<Cna>,
    content: &CnaContent,
    pins: &Pins,
) -> Campaign<Cna> {
    let hash = campaign.state_hash().unwrap();
    let status = campaign.status().clone();
    let frames = streams(&campaign);
    let views = Perspective::all()
        .map(|p| (p, campaign.view(p).unwrap()))
        .collect::<Vec<_>>();
    let bindings = SeatId::all()
        .map(|s| (s, campaign.binding(s).clone()))
        .collect::<Vec<_>>();
    let before = rows(path);
    drop(campaign);
    let restored = Campaign::recover(path, Cna::dev(), copy_content(content), pins).unwrap();
    assert_eq!(restored.state_hash().unwrap(), hash);
    assert_eq!(restored.status(), &status);
    assert_eq!(streams(&restored), frames);
    for (p, view) in views {
        assert_eq!(restored.view(p).unwrap(), view, "{p}");
    }
    for (s, binding) in bindings {
        assert_eq!(restored.binding(s), &binding);
    }
    assert_eq!(rows(path), before);
    restored
}
#[test]
fn raw_counter_paths_survive_sqlite_submit_retry_checkpoint_and_tail() {
    prove_raw_paths(false, &[false, true]);
}
#[test]
fn visible_child_rejoining_real_parent_keeps_path_through_sqlite() {
    prove_raw_paths(true, &[false]);
}
#[test]
fn visible_child_rejoining_real_parent_keeps_path_through_sqlite_rba() {
    prove_raw_paths(true, &[true]);
}
fn prove_raw_paths(joining: bool, retreats: &[bool]) {
    for &retreat in retreats {
        let (content, mut reference, supplied_expected) = if joining {
            joining_fixture(retreat)
        } else {
            fixture(retreat)
        };
        let mover = if joining { COUNTER } else { ROOT };
        let parent_before = joining
            .then(|| serde_json::to_value(&reference.state.land.units[&PARENT.into()]).unwrap());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("raw-paths.sqlite");
        let pins = Pins {
            rules_profile: cna_rules::PROFILE_DEV.into(),
            content_hash: "synthetic-counter-path-grid".into(),
            engine_version: env!("CNA_ENGINE_SOURCE_HASH").into(),
        };
        let meta = CampaignMeta {
            id: "raw-paths".into(),
            scenario_id: "graziani".into(),
            rules_profile: pins.rules_profile.clone(),
            title: "RAW path persistence".into(),
            seats: vec![],
        };
        let mut campaign = Campaign::create(
            &path,
            Cna::dev(),
            copy_content(&content),
            serde_json::from_value(serde_json::to_value(&reference).unwrap()).unwrap(),
            meta,
            pins.clone(),
        )
        .unwrap();
        for seat in SeatId::all() {
            campaign
                .handover(
                    seat,
                    Some(ControllerInfo {
                        kind: ControllerKind::Human,
                        label: "own fixture seat".into(),
                    }),
                    Value::Null,
                )
                .unwrap();
        }
        let seat = SeatId::new(Side::Commonwealth, Role::FrontLine);
        let request = campaign
            .pending()
            .into_iter()
            .find(|r| r.seat == seat)
            .unwrap();
        let move_response = response(
            &request,
            json!([{"unit":mover,"with_stack":!joining,"path":["C4021","C4022"]}]),
            "raw-counter-path",
        );
        let before_assets = json!({"units":reference.state.land.units,"logistics":reference.state.logistics,"rng":reference.rng});
        let before = rows(&path);
        let hash = campaign.state_hash().unwrap();
        campaign.validate(&move_response).unwrap();
        assert_eq!(rows(&path), before);
        assert_eq!(campaign.state_hash().unwrap(), hash);
        apply(
            &mut campaign,
            &content,
            &mut reference,
            Command::Respond(move_response.clone()),
        );
        if !retreat && let Some(expected) = supplied_expected {
            assert_eq!(game_hash(&reference), game_hash(&expected));
        }
        let retry_rows = rows(&path);
        let retry_hash = campaign.state_hash().unwrap();
        assert!(campaign.submit(move_response.clone()).unwrap().duplicate);
        assert_eq!(rows(&path), retry_rows);
        assert_eq!(campaign.state_hash().unwrap(), retry_hash);
        if retreat {
            assert_eq!(
                json!({"units":reference.state.land.units,"logistics":reference.state.logistics,"rng":reference.rng}),
                before_assets
            );
            // Actual RBA answers only buffer; finish_step executes the shared Land path once.
            assert_eq!(
                reference.state.land.units[&mover.into()].location.hex(),
                Some(&"C4020".into())
            );
            assert!(
                streams(&campaign)
                    .iter()
                    .all(|(_, s)| s.iter().all(|m| !matches!(
                        m,
                        ServerMessage::Event {
                            event: GameEvent::UnitMoved { .. },
                            ..
                        }
                    )))
            );
            // Recover the accepted private plan before any retreat is adjudicated.
            campaign = recover_exact(&path, campaign, &content, &pins);
            let plan_rows = rows(&path);
            assert!(campaign.submit(move_response.clone()).unwrap().duplicate);
            assert_eq!(rows(&path), plan_rows);
            while let Some(request) = campaign.pending().first() {
                assert!(request.space.pass.is_some());
                let answer = response(request, Value::Null, &format!("rba-pass-{}", request.id));
                apply(
                    &mut campaign,
                    &content,
                    &mut reference,
                    Command::Respond(answer),
                );
            }
            apply(&mut campaign, &content, &mut reference, Command::Advance);
        }
        for id in if joining {
            vec![COUNTER]
        } else {
            vec![ROOT, COUNTER, ATTACHED]
        } {
            assert_eq!(
                reference.state.land.units[&id.into()].location.hex(),
                Some(&"C4022".into())
            );
        }
        if let Some(parent_before) = parent_before {
            assert_eq!(
                serde_json::to_value(&reference.state.land.units[&PARENT.into()]).unwrap(),
                parent_before
            );
            assert!(!reference.state.land.units[&COUNTER.into()].detached);
        }
        assert_paths(&campaign, joining);
        // Replay movement from the initial checkpoint and retained command tail.
        campaign = recover_exact(&path, campaign, &content, &pins);
        let retry_rows = rows(&path);
        assert!(campaign.submit(move_response.clone()).unwrap().duplicate);
        assert_eq!(rows(&path), retry_rows);
        // Reach a real automatic checkpoint using only existing declared passes/Advance.
        while Connection::open(&path)
            .unwrap()
            .query_row("SELECT revision FROM campaign", [], |r| r.get::<_, u64>(0))
            .unwrap()
            < 33
        {
            let command = if let Some(request) = campaign.pending().first() {
                let action = if request.kind == cna_rules::logistics::truck_convoy::KIND {
                    json!([])
                } else {
                    assert!(
                        request.space.pass.is_some(),
                        "unexpected compulsory fixture window: {}",
                        request.kind
                    );
                    Value::Null
                };
                Command::Respond(response(
                    request,
                    action,
                    &format!("checkpoint-pass-{}", request.id),
                ))
            } else {
                Command::Advance
            };
            apply(&mut campaign, &content, &mut reference, command);
        }
        let db = Connection::open(&path).unwrap();
        let checkpoint = db
            .query_row("SELECT MAX(revision) FROM checkpoints", [], |r| {
                r.get::<_, u64>(0)
            })
            .unwrap();
        let revision = db
            .query_row("SELECT revision FROM campaign", [], |r| r.get::<_, u64>(0))
            .unwrap();
        assert_eq!(checkpoint, 32);
        assert_eq!(revision, 33);
        assert_paths(&campaign, joining);
        campaign = recover_exact(&path, campaign, &content, &pins);
        let before = rows(&path);
        assert!(campaign.submit(move_response).unwrap().duplicate);
        assert_eq!(rows(&path), before);
        assert_paths(&campaign, joining);
    }
}
