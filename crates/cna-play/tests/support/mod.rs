//! Offline preparation: evaluate setup once, freeze saved bytes, restore independent campaigns.
use cna_core::{
    dice::CampaignRng,
    engine::{Command, Game, Ruleset, evaluate},
    ids::SeatId,
};
use cna_play::{
    Demo,
    config::{Controller, LaunchConfig, SessionLimits},
    journal::SessionJournal,
};
use cna_rules::{Cna, CnaContent, State};
use cna_server::{
    Campaign, CampaignStatus,
    scripted::{Mode, NoCandidates, answer},
};
use std::{collections::BTreeMap, path::Path, time::Duration};
use tokio::sync::OnceCell;

const MAX_PREPARATION_COMMANDS: u64 = 4096;
const HANG_GUARD: Duration = Duration::from_secs(180);
struct MovementCheckpoint {
    id: String,
    bytes: Vec<u8>,
    epochs: BTreeMap<SeatId, u64>,
    bindings: BTreeMap<SeatId, Controller>,
}
static MOVEMENT: OnceCell<MovementCheckpoint> = OnceCell::const_new();

fn fixture_limits(config: &mut LaunchConfig) {
    config.session.get_or_insert(SessionLimits {
        wall_seconds: 600,
        turn_seconds: 30,
        context_tokens: 100_000,
        recoveries: 3,
    });
}

/// Setup uses actual engine transitions with a pass-when-offered policy, never invented stock.
/// Counts bound preparation; the clock is a hang guard, not an expected runtime.
/// The cache contains bytes only: no writer/MCP survives its test runtime.
pub async fn movement_demo(root: &Path, repo: &Path, mut config: LaunchConfig) -> Demo {
    let checkpoint = MOVEMENT
        .get_or_try_init(|| async {
            let prepared_root = tempfile::tempdir().map_err(|e| e.to_string())?;
            let seed_root = prepared_root.path().join("seed");
            let mut preparation = config.clone();
            preparation.session = None;
            preparation.max_turns = 2;
            preparation.tool_calls = 40;
            let seed = Demo::with_config(
                &seed_root,
                &repo.join("data"),
                &repo.join("web/dist"),
                preparation,
            )
            .await?;
            let id = seed.campaign_id();
            let bindings: BTreeMap<_, _> = SeatId::all()
                .map(|s| (s, seed.handle.seat(s).binding))
                .collect();
            seed.shutdown().await?;
            // Obtain immutable pins from the real factory. Standard recovery verifies them again.
            let db = rusqlite::Connection::open(seed_root.join(format!("{id}.sqlite")))
                .map_err(|e| e.to_string())?;
            let (meta, pins): (String, String) = db
                .query_row("SELECT meta, pins FROM campaign WHERE id=1", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .map_err(|e| e.to_string())?;
            drop(db);
            let data = repo.join("data");
            let path = prepared_root.path().join(format!("{id}.sqlite"));
            let epochs =
                tokio::task::spawn_blocking(move || -> Result<BTreeMap<SeatId, u64>, String> {
                    let content = CnaContent::load(&data, "graziani")?;
                    let rules = Cna::dev();
                    let mut game = Game {
                        state: State::new(&content)?,
                        rng: CampaignRng::from_seed([7; 32]).state(),
                    };
                    let mut answered = 0;
                    loop {
                        game = evaluate(&rules, &content, &game, &Command::Advance)
                            .map_err(|e| e.to_string())?
                            .game;
                        if game.state.cursor.anchor().split('.').next() != Some("setup") {
                            break;
                        }
                        if answered >= MAX_PREPARATION_COMMANDS {
                            return Err("setup exceeded its accepted-decision safety bound".into());
                        }
                        let request = rules
                            .pending(&content, &game.state)
                            .into_iter()
                            .next()
                            .ok_or("setup has no pending decision")?;
                        let mut selected = None;
                        for attempt in 0..32 {
                            let response = answer(
                                &request,
                                bindings[&request.seat].controller_epoch,
                                Mode::PassWhenPossible,
                                attempt,
                                &NoCandidates,
                            )
                            .map_err(|e| e.to_string())?;
                            if let Ok(transition) =
                                evaluate(&rules, &content, &game, &Command::Respond(response))
                            {
                                selected = Some(transition.game);
                                break;
                            }
                        }
                        game = selected.ok_or_else(|| {
                            format!("no accepted fixture setup action for {}", request.kind)
                        })?;
                        answered += 1;
                    }
                    let mut campaign = Campaign::create(
                        &path,
                        rules,
                        content,
                        game,
                        serde_json::from_str(&meta).map_err(|e| e.to_string())?,
                        serde_json::from_str(&pins).map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
                    let mut epochs = BTreeMap::new();
                    for (seat, binding) in bindings {
                        let llm = binding
                            .controller
                            .as_ref()
                            .is_some_and(|c| c.kind == cna_protocol::ControllerKind::LlmCli);
                        let rebound = campaign
                            .handover(seat, binding.controller, binding.config)
                            .map_err(|e| e.to_string())?;
                        if llm {
                            epochs.insert(seat, rebound.controller_epoch);
                        }
                    }
                    campaign.set_paused(true).map_err(|e| e.to_string())?;
                    // Drop the owned SQLite writer before ordinary factory recovery.
                    drop(campaign);
                    Ok(epochs)
                })
                .await
                .map_err(|e| e.to_string())??;
            let mut preparation = config.clone();
            fixture_limits(&mut preparation);
            let journal = SessionJournal::create(prepared_root.path(), &id, preparation, &epochs)?;
            drop(journal);
            let demo = Demo::resume(
                &prepared_root.path().join(format!("{id}.sqlite")),
                &repo.join("data"),
                &repo.join("web/dist"),
            )
            .await?;
            let mut windows = demo.handle.watch_seat(demo.seat);
            let result = tokio::time::timeout(HANG_GUARD, async {
                demo.handle.pause(false).await.map_err(|e| e.to_string())?;
                loop {
                    if demo.handle.runtime_metrics().committed_commands > MAX_PREPARATION_COMMANDS {
                        return Err(
                            "movement preparation exceeded its committed-decision safety bound"
                                .into(),
                        );
                    }
                    let pending = windows.borrow_and_update().pending.clone();
                    if pending.iter().any(|p| p.kind == "cna.movement.orders") {
                        return demo.handle.pause(true).await.map_err(|e| e.to_string());
                    }
                    if !pending.is_empty() {
                        return Err("fixture seat opened a different window before movement".into());
                    }
                    if !matches!(demo.handle.status(), CampaignStatus::Running) {
                        return Err(format!(
                            "movement preparation stopped: {:?}",
                            demo.handle.status()
                        ));
                    }
                    windows.changed().await.map_err(|e| e.to_string())?;
                }
            })
            .await
            .map_err(|_| "movement preparation hang guard elapsed".to_string())
            .and_then(|r| r);
            let cleanup = demo.shutdown().await;
            cna_play::combine_results(result, cleanup)?;
            let saved = prepared_root.path().join(format!("{id}.sqlite"));
            // VACUUM INTO includes WAL contents in a self-contained snapshot after shutdown.
            let snapshot = prepared_root.path().join("movement.snapshot.sqlite");
            let db = rusqlite::Connection::open(saved).map_err(|e| e.to_string())?;
            db.execute("VACUUM INTO ?1", [snapshot.to_string_lossy().as_ref()])
                .map_err(|e| e.to_string())?;
            drop(db);
            Ok::<_, String>(MovementCheckpoint {
                id,
                bytes: std::fs::read(snapshot).map_err(|e| e.to_string())?,
                epochs,
                bindings: config.seats.clone(),
            })
        })
        .await
        .expect("count-bounded movement checkpoint");
    assert_eq!(
        config.seats, checkpoint.bindings,
        "cached fixture policy must match"
    );
    let path = root.join(format!("{}.sqlite", checkpoint.id));
    std::fs::write(&path, &checkpoint.bytes).unwrap();
    fixture_limits(&mut config);
    let journal = SessionJournal::create(root, &checkpoint.id, config, &checkpoint.epochs).unwrap();
    drop(journal);
    let demo = Demo::resume(&path, &repo.join("data"), &repo.join("web/dist"))
        .await
        .unwrap();
    assert!(
        demo.handle
            .seat(demo.seat)
            .pending
            .iter()
            .any(|p| p.kind == "cna.movement.orders")
    );
    assert!(
        demo.journal
            .as_ref()
            .unwrap()
            .snapshot()
            .unwrap()
            .seats
            .values()
            .all(|s| s.turns == 0 && s.calls == 0)
    );
    demo
}
