use cna_core::{ids::SeatId, visibility::Perspective};
use cna_protocol::ServerMessage;
use cna_server::{
    CampaignStatus, Error,
    actor::CampaignHandle,
    campaigns,
    http::{CampaignKind, CreateRequest},
};
use rusqlite::{Connection, OpenFlags};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data")
}
fn request(profile: &str, mode: &str, paused: bool) -> CreateRequest {
    CreateRequest {
        kind: CampaignKind::Cna,
        rules_profile: profile.into(),
        seed: [7; 32],
        title: "Graziani integration".into(),
        controller: mode.into(),
        paused,
    }
}
// Completed CI f16f459/run37564332764: legal_random397.550s, pass_when_possible191.842s,
// HTTP/legal_random423.796s. Ignored-only ceilings are about twice each completed time,
// rounded up. This measurement predates force-assignment and mandatory breakdown windows;
// recalibrate against their next completed CI report. Default limits are unchanged.
const SLOW_RANDOM_LIMIT: Duration = Duration::from_secs(800);
const SLOW_PASS_LIMIT: Duration = Duration::from_secs(400);
const SLOW_HTTP_LIMIT: Duration = Duration::from_secs(850);
// Whole-roster benchmarks calibrate one campaign at a time on small hosted runners.
static SLOW_CAMPAIGN_SLOT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn report_campaign(handle: &CampaignHandle, path: &Path, label: &str, wall: Duration) {
    let metrics = handle.runtime_metrics();
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let commands: u64 = db
        .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0))
        .unwrap();
    let mut counts = std::collections::BTreeMap::new();
    for request in resolved_decisions(path) {
        let kind = request.kind.split(':').next().unwrap().to_owned();
        *counts.entry(kind).or_insert(0_u64) += 1;
    }
    // Direct stderr is intentional: successful ignored-test measurements must appear in CI,
    // where libtest normally captures successful eprintln output.
    let _ = writeln!(
        std::io::stderr(),
        "CNA_PROFILE {label} wall_s={:.3} commands={commands} measured_commands={} engine_s={:.3} durable_writer_s={:.3} projections_s={:.3} controllers_s={:.3} views_s={:.3} observe_s={:.3} opened_seq_s={:.3} events_after_s={:.3} decision_kinds={counts:?}",
        wall.as_secs_f64(),
        metrics.committed_commands,
        metrics.engine.as_secs_f64(),
        metrics.durable_writer.as_secs_f64(),
        metrics.projections.as_secs_f64(),
        metrics.controllers.as_secs_f64(),
        metrics.projection_views.as_secs_f64(),
        metrics.projection_observe.as_secs_f64(),
        metrics.projection_opened_seq.as_secs_f64(),
        metrics.projection_events_after.as_secs_f64()
    );
    if handle.status() != CampaignStatus::Running {
        assert_eq!(metrics.committed_commands, commands);
    }
}

async fn terminal(handle: &CampaignHandle, path: Option<&Path>, label: &str) -> CampaignStatus {
    let started = Instant::now();
    let mut status = handle.watch_status();
    let limit = if path.is_some() {
        if label == "pass_when_possible" {
            SLOW_PASS_LIMIT
        } else {
            SLOW_RANDOM_LIMIT
        }
    } else {
        Duration::from_secs(120)
    };
    let result = tokio::time::timeout(limit, async {
        loop {
            let current = status.borrow_and_update().clone();
            if matches!(
                current,
                CampaignStatus::Finished { .. } | CampaignStatus::Stopped { .. }
            ) {
                return current;
            }
            for seat in SeatId::all() {
                let binding = handle.seat(seat).binding;
                assert!(
                    !binding.paused,
                    "scripted seat {seat} paused: {:?}",
                    binding.failure
                );
            }
            tokio::select! {
                result = status.changed() => result.expect("writer closed unexpectedly"),
                _ = tokio::time::sleep(Duration::from_millis(100)) => {},
            }
        }
    })
    .await;
    if let Some(path) = path {
        report_campaign(handle, path, label, started.elapsed());
    }
    result.unwrap_or_else(|_| {
        panic!(
            "campaign timeout at {:?}; seats {:?}",
            handle.projection(Perspective::Operator).view.clock,
            SeatId::all()
                .map(|s| (s, handle.seat(s).binding))
                .collect::<Vec<_>>()
        )
    })
}
fn stored(path: &Path) -> (u64, String, String) {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    db.query_row(
        "SELECT revision, rng, state_hash FROM campaign WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .unwrap()
}
fn resolved_decisions(path: &Path) -> Vec<cna_core::decision::DecisionRequest> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut statement = db
        .prepare("SELECT request FROM decisions WHERE resolved_revision IS NOT NULL ORDER BY id")
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
        .collect()
}
fn assert_initiative_and_transcript_count(path: &Path, transcript_count: usize) {
    let requests = resolved_decisions(path);
    assert_eq!(
        requests
            .iter()
            .filter(|d| d.kind == "cna.initiative_declaration")
            .count(),
        18
    );
    assert_eq!(transcript_count, requests.len());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow: whole campaign"]
async fn real_graziani_baselines_finish_with_private_transcripts_and_exact_recovery() {
    let _slot = SLOW_CAMPAIGN_SLOT.lock().await;
    check_real_baselines(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_graziani_decisions_keep_private_transcripts_and_exact_recovery() {
    check_real_baselines(false).await;
}

async fn bounded_progress(handle: &CampaignHandle, path: &Path, count: usize) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while resolved_decisions(path).len() < count {
            assert_eq!(handle.status(), CampaignStatus::Running);
            for seat in SeatId::all() {
                assert!(
                    !handle.seat(seat).binding.paused,
                    "{seat}: {:?}",
                    handle.seat(seat).binding
                );
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("bounded decision window stalled");
    handle.pause(true).await.unwrap();
}

async fn check_real_baselines(whole: bool) {
    let directory = tempfile::tempdir().unwrap();
    for mode in if whole {
        vec!["legal_random", "pass_when_possible"]
    } else {
        vec!["pass_when_possible"]
    } {
        let handle = campaigns::create(
            directory.path(),
            &data(),
            request(cna_rules::PROFILE_DEV, mode, true),
        )
        .unwrap();
        let meta = handle.projection(Perspective::Operator).meta;
        assert_eq!(meta.scenario_id, "graziani");
        assert_eq!(meta.rules_profile, cna_rules::PROFILE_DEV);
        assert_eq!(meta.seats.len(), 10);
        let path = directory.path().join(format!("{}.sqlite", meta.id));
        // The initial board is the real scenario, not the sandbox's nine units.
        assert!(handle.projection(Perspective::Operator).view.units.len() > 100);
        handle.pause(false).await.unwrap();
        let handle = if whole {
            assert!(
                matches!(
                    terminal(&handle, Some(&path), mode).await,
                    CampaignStatus::Finished { .. }
                ),
                "{:?}",
                handle.status()
            );
            handle
        } else {
            bounded_progress(&handle, &path, 16).await;
            let expected: Vec<_> = Perspective::all()
                .map(|p| (p, handle.projection(p)))
                .collect();
            let observations: Vec<_> = SeatId::all()
                .map(|seat| (seat, handle.observation(seat)))
                .collect();
            handle.shutdown().await.unwrap();
            let before = stored(&path);
            let restored = campaigns::recover(&path, &data()).unwrap();
            assert_eq!(restored.status(), CampaignStatus::Paused);
            for (seat, expected) in observations {
                assert_eq!(restored.observation(seat), expected);
            }
            for (p, projection) in expected {
                assert_eq!(restored.projection(p), projection);
            }
            assert_eq!(stored(&path), before);
            restored.pause(false).await.unwrap();
            bounded_progress(&restored, &path, 32).await;
            restored
        };
        let projections: Vec<_> = Perspective::all()
            .map(|p| (p, handle.projection(p)))
            .collect();
        let mut count = 0;
        for seat in SeatId::all() {
            let mut rows = Vec::new();
            let mut after = 0;
            loop {
                let page = handle
                    .replay()
                    .transcripts(Perspective::Operator, seat, after)
                    .unwrap();
                let len = page.len();
                if let Some(ServerMessage::Transcript { tseq, .. }) = page.last() {
                    after = *tseq;
                }
                rows.extend(page);
                if len < 512 {
                    break;
                }
            }
            count += rows.len();
            for (index, row) in rows.iter().enumerate() {
                assert!(
                    matches!(row,ServerMessage::Transcript {tseq,entry:cna_protocol::TranscriptEntry::DecisionSubmitted {..},..}
                    if *tseq == index as u64+1)
                );
            }
            let enemy = if seat.side == cna_protocol::Side::Axis {
                cna_protocol::Side::Commonwealth
            } else {
                cna_protocol::Side::Axis
            };
            assert!(
                handle
                    .replay()
                    .transcripts(Perspective::Side(enemy), seat, 0)
                    .unwrap()
                    .is_empty()
            );
        }
        if whole {
            assert_initiative_and_transcript_count(&path, count);
        } else {
            assert_eq!(count, resolved_decisions(&path).len());
        }
        let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let submitted: usize = db
            .query_row("SELECT COUNT(*) FROM commands WHERE seat != ''", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            submitted, count,
            "every submitted decision has exactly one truthful transcript"
        );
        drop(db);
        let movement_ids: std::collections::BTreeSet<_> = resolved_decisions(&path)
            .into_iter()
            .filter(|d| d.kind == cna_rules::land::movement::KIND)
            .map(|d| d.id)
            .collect();
        if whole {
            assert!(!movement_ids.is_empty());
        }
        let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let mut statement = db
            .prepare("SELECT command FROM commands WHERE seat != ''")
            .unwrap();
        let commands: Vec<cna_core::engine::Command> = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect();
        let actions: Vec<_> = commands
            .iter()
            .filter_map(|command| match command {
                cna_core::engine::Command::Respond(r) if movement_ids.contains(&r.decision_id) => {
                    Some(&r.action)
                }
                _ => None,
            })
            .collect();
        if mode == "legal_random" {
            assert!(
                actions.iter().any(|action| action.is_array()),
                "movement policy actually dispatched"
            );
        } else {
            assert!(
                actions.iter().all(|action| action.is_null()),
                "pass controller remains separate"
            );
        }
        drop(statement);
        drop(db);
        let observations: Vec<_> = SeatId::all()
            .map(|seat| (seat, handle.observation(seat)))
            .collect();
        handle.shutdown().await.unwrap();
        let before = stored(&path);
        let db = Connection::open(&path).unwrap();
        let checkpoint: u64 = db
            .query_row("SELECT MAX(revision) FROM checkpoints", [], |r| r.get(0))
            .unwrap();
        assert!(checkpoint >= 32); // Exercise checkpoint plus replay of its later commands.
        drop(db);
        let restored = campaigns::recover(&path, &data()).unwrap();
        assert_eq!(restored.status(), handle.status());
        for (seat, expected) in observations {
            assert_eq!(restored.observation(seat), expected);
        }
        for (p, expected) in projections {
            assert_eq!(restored.projection(p), expected);
        }
        restored.shutdown().await.unwrap();
        assert_eq!(stored(&path), before);
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_profile_stops_without_guessing_and_mixed_recovery_keeps_profiles() {
    let directory = tempfile::tempdir().unwrap();
    let mut sandbox = request(cna_sandbox::PROFILE_ID, "human", true);
    sandbox.kind = CampaignKind::Sandbox;
    let sandbox = campaigns::create(directory.path(), &data(), sandbox).unwrap();
    let full = campaigns::create(
        directory.path(),
        &data(),
        request(cna_rules::PROFILE_FULL, "legal_random", true),
    )
    .unwrap();
    let full_path = directory.path().join(format!(
        "{}.sqlite",
        full.projection(Perspective::Operator).meta.id
    ));
    let before = stored(&full_path);
    let seat: SeatId = "axis.commander".parse().unwrap();
    let binding = full.seat(seat).binding;
    assert!(matches!(
        full.handover(
            seat,
            Some(cna_protocol::ControllerInfo {
                kind: cna_protocol::ControllerKind::Scripted,
                label: "scripted:aggressive".into(),
            }),
            serde_json::json!({"mode":"aggressive"})
        )
        .await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(full.seat(seat).binding, binding);
    assert!(
        full.replay()
            .transcripts(Perspective::Operator, seat, 0)
            .unwrap()
            .is_empty()
    );
    full.pause(false).await.unwrap();
    let status = terminal(&full, None, "full").await;
    assert!(
        matches!(&status,CampaignStatus::Stopped {error} if error.contains("unsupported")),
        "{status:?}"
    );
    assert_eq!(stored(&full_path), before); // Failed Advance never commits its partial effects/RNG.
    full.shutdown().await.unwrap();
    let sandbox_path = directory.path().join(format!(
        "{}.sqlite",
        sandbox.projection(Perspective::Operator).meta.id
    ));
    let expected = sandbox.projection(Perspective::Operator);
    sandbox.shutdown().await.unwrap();
    let recovered_sandbox = campaigns::recover(&sandbox_path, &data()).unwrap();
    assert_eq!(
        recovered_sandbox.projection(Perspective::Operator),
        expected
    );
    recovered_sandbox.shutdown().await.unwrap();
    let recovered_full = campaigns::recover(&full_path, &data()).unwrap();
    assert_eq!(recovered_full.status(), status);
    assert_eq!(
        recovered_full
            .projection(Perspective::Operator)
            .meta
            .rules_profile,
        cna_rules::PROFILE_FULL
    );
    recovered_full.shutdown().await.unwrap();
}

fn copy(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let path = entry.unwrap().path();
        let destination = target.join(path.file_name().unwrap());
        if path.is_dir() {
            copy(&path, &destination);
        } else {
            fs::copy(&path, &destination).unwrap();
        }
    }
}
fn first_toml(directory: &Path) -> PathBuf {
    let mut paths: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            return first_toml(&path);
        }
        if path.extension().is_some_and(|e| e == "toml") {
            return path;
        }
    }
    panic!("no TOML in {}", directory.display());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_pins_read_content_but_ignores_unread_notes_and_files() {
    let directory = tempfile::tempdir().unwrap();
    let cloned_data = directory.path().join("data");
    copy(&data(), &cloned_data);
    let handle = campaigns::create(
        directory.path(),
        &cloned_data,
        request(cna_rules::PROFILE_DEV, "human", true),
    )
    .unwrap();
    let path = directory.path().join(format!(
        "{}.sqlite",
        handle.projection(Perspective::Operator).meta.id
    ));
    handle.shutdown().await.unwrap();
    let original = stored(&path);
    for file in [
        cloned_data.join("map/aliases.csv"),
        cloned_data.join("map/layers.toml"),
        cloned_data.join("map/coverage.csv"),
        cloned_data.join("map/line_features.csv"),
        cloned_data.join("map/hexsides.csv"),
        cloned_data.join("map/sections.toml"),
        cloned_data.join("map/areas.toml"),
        first_toml(&cloned_data.join("units/weapons")),
        cloned_data.join("scenarios/graziani/scenario.toml"),
        first_toml(&cloned_data.join("tables")),
        first_toml(&cloned_data.join("rules/land")),
    ] {
        let original_bytes = fs::read(&file).unwrap();
        let mut changed = original_bytes.clone();
        changed.extend_from_slice(if file.extension().is_some_and(|e| e == "toml") {
            b"\n# input pin test\n"
        } else {
            b"\n"
        });
        fs::write(&file, &changed).unwrap();
        assert!(
            matches!(campaigns::recover(&path,&cloned_data),Err(Error::Recovery(reason)) if reason.contains("pins changed")),
            "{}",
            file.display()
        );
        assert_eq!(stored(&path), original);
        fs::write(&file, original_bytes).unwrap();
    }
    for unused in [
        "map/GAPS.md",
        "scenarios/graziani/unread.toml",
        "units/weapons/GAPS.md",
        "rules/land/nested/unread.toml",
        "rules/unread/book.toml",
    ] {
        let file = cloned_data.join(unused);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, "this is unread content and deliberately not TOML").unwrap();
    }
    let recovered = campaigns::recover(&path, &cloned_data).unwrap();
    recovered.shutdown().await.unwrap();
    assert_eq!(stored(&path), original);
    let db = Connection::open(&path).unwrap();
    let text: String = db
        .query_row("SELECT meta FROM campaign WHERE id=1", [], |r| r.get(0))
        .unwrap();
    let mut meta: cna_protocol::CampaignMeta = serde_json::from_str(&text).unwrap();
    meta.scenario_id = "sandbox".into();
    db.execute(
        "UPDATE campaign SET meta=? WHERE id=1",
        [serde_json::to_string(&meta).unwrap()],
    )
    .unwrap();
    drop(db);
    assert!(matches!(
        campaigns::recover(&path, &cloned_data),
        Err(Error::Recovery(_))
    ));
}
#[tokio::test]
async fn campaign_kind_and_profile_must_agree_before_creating_a_database() {
    let directory = tempfile::tempdir().unwrap();
    let mut wrong = request(cna_sandbox::PROFILE_ID, "human", true);
    assert!(matches!(
        campaigns::create(directory.path(), &data(), wrong.clone()),
        Err(Error::Invalid(_))
    ));
    wrong.kind = CampaignKind::Sandbox;
    wrong.rules_profile = cna_rules::PROFILE_DEV.into();
    assert!(matches!(
        campaigns::create(directory.path(), &data(), wrong),
        Err(Error::Invalid(_))
    ));
    for mode in ["aggressive", "scripted:aggressive", "unknown"] {
        assert!(matches!(
            campaigns::create(
                directory.path(),
                &data(),
                request(cna_rules::PROFILE_DEV, mode, true)
            ),
            Err(Error::Invalid(_))
        ));
    }
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    let legacy = serde_json::json!({"rules_profile":"sandbox-v1","seed":vec![0u8;32]});
    let legacy: CreateRequest = serde_json::from_value(legacy).unwrap();
    assert_eq!(legacy.kind, CampaignKind::Sandbox);
    assert!(
        serde_json::from_value::<CreateRequest>(
            serde_json::json!({"kind":"unknown","rules_profile":"cna-2021-dev","seed":vec![0u8;32]})
        )
        .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow: whole campaign"]
async fn http_creates_the_real_profile_and_serves_its_snapshot_and_transcripts() {
    let _slot = SLOW_CAMPAIGN_SLOT.lock().await;
    check_http_real_profile(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_bounded_real_profile_serves_private_transcripts_and_recovers() {
    check_http_real_profile(false).await;
}

async fn check_http_real_profile(whole: bool) {
    use cna_server::http::App;
    use serde_json::{Value, json};
    let directory = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let created = Arc::new(Mutex::new(None::<CampaignHandle>));
    let captured = Arc::clone(&created);
    let app = App::new(
        directory.path().to_owned(),
        port,
        Arc::new(move |request, dir| {
            let handle = campaigns::create(dir, &data(), request)?;
            *captured.lock().unwrap() = Some(handle.clone());
            Ok(handle)
        }),
    );
    let router = app.router(&directory.path().join("missing-dist"));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {}", app.operator_token()).parse().unwrap(),
    );
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();
    let response = client
        .post(format!("{base}/api/campaigns"))
        .json(&request(
            cna_rules::PROFILE_DEV,
            if whole {
                "legal_random"
            } else {
                "pass_when_possible"
            },
            true,
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let meta: Value = response.json().await.unwrap();
    assert_eq!(meta["scenario_id"], "graziani");
    assert_eq!(meta["rules_profile"], cna_rules::PROFILE_DEV);
    let id = meta["id"].as_str().unwrap();
    let detail: Value = client
        .get(format!("{base}/api/campaigns/{id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["status"]["state"], "paused");
    assert!(
        detail["snapshot"]["view"]["units"]
            .as_object()
            .unwrap()
            .len()
            > 100
    );
    let side: Value = client
        .get(format!("{base}/api/campaigns/{id}?perspective=side:axis"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Own units in full; Commonwealth counters on the map only by their printed face (land:3.62).
    for unit in side["snapshot"]["view"]["units"]
        .as_object()
        .unwrap()
        .values()
    {
        if unit["side"] != "axis" {
            assert!(unit["parent"].is_null(), "{unit}");
            for key in unit["detail"]
                .as_object()
                .into_iter()
                .flat_map(|d| d.keys())
            {
                assert!(
                    ["counter", "stacking_points"].contains(&key.as_str()),
                    "an enemy face carries {key}: {unit}"
                );
            }
        }
    }
    assert_eq!(
        client
            .post(format!("{base}/api/campaigns/{id}/resume"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let started = Instant::now();
    let limit = if whole {
        SLOW_HTTP_LIMIT
    } else {
        Duration::from_secs(20)
    };
    let result = tokio::time::timeout(limit, async {
        loop {
            let detail: Value = client
                .get(format!("{base}/api/campaigns/{id}"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if whole && detail["status"]["state"] == "finished"
                || !whole
                    && resolved_decisions(&directory.path().join(format!("{id}.sqlite"))).len()
                        >= 16
            {
                break;
            }
            assert_ne!(detail["status"]["state"], "stopped", "{detail}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if whole {
        let handle = created.lock().unwrap().as_ref().unwrap().clone();
        report_campaign(
            &handle,
            &directory.path().join(format!("{id}.sqlite")),
            "http/legal_random",
            started.elapsed(),
        );
    }
    result.unwrap();
    if !whole {
        assert_eq!(
            client
                .post(format!("{base}/api/campaigns/{id}/pause"))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    let mut count = 0;
    for seat in SeatId::all() {
        let mut after = 0;
        loop {
            let rows: Vec<ServerMessage> = client
                .get(format!("{base}/api/campaigns/{id}/transcripts?perspective=operator&seat={seat}&after={after}"))
                .send().await.unwrap().json().await.unwrap();
            count += rows.len();
            if let Some(ServerMessage::Transcript { tseq, .. }) = rows.last() {
                after = *tseq;
            }
            if rows.len() < 512 {
                break;
            }
        }
    }
    let path = directory.path().join(format!("{id}.sqlite"));
    if whole {
        assert_initiative_and_transcript_count(&path, count);
    } else {
        assert_eq!(count, resolved_decisions(&path).len());
    }
    let mut expected = Vec::new();
    for p in Perspective::all() {
        let detail: Value = client
            .get(format!("{base}/api/campaigns/{id}?perspective={p}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        expected.push((
            p,
            serde_json::from_value::<cna_server::actor::Projection>(detail["snapshot"].clone())
                .unwrap(),
        ));
    }
    let denied:Vec<Value> = client.get(format!("{base}/api/campaigns/{id}/transcripts?perspective=side:axis&seat=commonwealth.commander"))
        .send().await.unwrap().json().await.unwrap();
    assert!(denied.is_empty());
    let invalid = client
        .post(format!("{base}/api/campaigns"))
        .json(&json!({"kind":"cna","rules_profile":"sandbox-v1","seed":vec![0u8;32]}))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), 400);
    app.shutdown().await;
    task.abort();
    let _ = task.await;
    let before = stored(&path);
    let restored = campaigns::recover(&path, &data()).unwrap();
    for (p, snapshot) in expected {
        assert_eq!(restored.projection(p), snapshot);
    }
    restored.shutdown().await.unwrap();
    assert_eq!(stored(&path), before);
}

#[tokio::test]
async fn legacy_geometry_only_map_does_not_pin_unread_layer_provenance() {
    let directory = tempfile::tempdir().unwrap();
    let cloned_data = directory.path().join("data");
    copy(&data(), &cloned_data);
    for file in [
        "layers.toml",
        "coverage.csv",
        "line_features.csv",
        "hexsides.csv",
    ] {
        fs::remove_file(cloned_data.join("map").join(file)).unwrap();
    }
    let handle = campaigns::create(
        directory.path(),
        &cloned_data,
        request(cna_rules::PROFILE_DEV, "human", true),
    )
    .unwrap();
    let path = directory.path().join(format!(
        "{}.sqlite",
        handle.projection(Perspective::Operator).meta.id
    ));
    handle.shutdown().await.unwrap();
    let original = stored(&path);
    fs::write(
        cloned_data.join("map/sections.toml"),
        "unread provenance and deliberately not TOML",
    )
    .unwrap();
    let restored = campaigns::recover(&path, &cloned_data).unwrap();
    restored.shutdown().await.unwrap();
    assert_eq!(stored(&path), original);
}

#[tokio::test]
async fn loader_manifest_pins_reused_setup_files_outside_the_selected_scenario() {
    let directory = tempfile::tempdir().unwrap();
    let cloned_data = directory.path().join("data");
    copy(&data(), &cloned_data);
    let main = cloned_data.join("scenarios/graziani/scenario.toml");
    let metadata = fs::read_to_string(&main).unwrap();
    assert!(metadata.contains("[scenario]"));
    assert!(metadata.contains("\"land_axis.toml\","));
    let metadata = metadata
        .replace(
            "[scenario]",
            "[scenario]\nsetup_from = \"pin_source\"\nsetup_files = [\"land_axis.toml\"]",
        )
        .replace("\"land_axis.toml\",", "");
    fs::write(&main, metadata).unwrap();
    let source = cloned_data.join("scenarios/pin_source/land_axis.toml");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    let local_setup = cloned_data.join("scenarios/graziani/land_axis.toml");
    fs::copy(&local_setup, &source).unwrap();
    fs::remove_file(&local_setup).unwrap();
    let files = cna_rules::content::source_files(&cloned_data, "graziani").unwrap();
    assert!(files.contains(&source));
    assert!(!files.contains(&local_setup));
    // A component-normalized caller path must yield the same relative-name hash.
    let handle = campaigns::create(
        directory.path(),
        &cloned_data.join("."),
        request(cna_rules::PROFILE_DEV, "human", true),
    )
    .unwrap();
    let path = directory.path().join(format!(
        "{}.sqlite",
        handle.projection(Perspective::Operator).meta.id
    ));
    handle.shutdown().await.unwrap();
    let before = stored(&path);
    let recovered = campaigns::recover(&path, &cloned_data).unwrap();
    recovered.shutdown().await.unwrap();
    assert_eq!(stored(&path), before);
    let original = fs::read(&source).unwrap();
    let mut changed = original.clone();
    changed.extend_from_slice(b"\n# inherited setup pin drift\n");
    fs::write(&source, changed).unwrap();
    assert!(
        matches!(campaigns::recover(&path, &cloned_data), Err(Error::Recovery(reason)) if reason.contains("pins changed"))
    );
    assert_eq!(stored(&path), before);
    fs::write(source, original).unwrap();
}
