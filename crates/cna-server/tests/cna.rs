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
    path::{Path, PathBuf},
    time::Duration,
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
async fn terminal(handle: &CampaignHandle) -> CampaignStatus {
    let mut status = handle.watch_status();
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let current = status.borrow_and_update().clone();
            if matches!(
                current,
                CampaignStatus::Finished { .. } | CampaignStatus::Stopped { .. }
            ) {
                return current;
            }
            status.changed().await.expect("writer closed unexpectedly");
        }
    })
    .await
    .expect("campaign timeout")
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
async fn real_graziani_baselines_finish_with_private_transcripts_and_exact_recovery() {
    let directory = tempfile::tempdir().unwrap();
    for mode in ["legal_random", "pass_when_possible"] {
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
        assert!(
            matches!(terminal(&handle).await, CampaignStatus::Finished { .. }),
            "{:?}",
            handle.status()
        );
        let projections: Vec<_> = Perspective::all()
            .map(|p| (p, handle.projection(p)))
            .collect();
        let mut count = 0;
        for seat in SeatId::all() {
            let rows = handle
                .replay()
                .transcripts(Perspective::Operator, seat, 0)
                .unwrap();
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
        assert_initiative_and_transcript_count(&path, count);
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
    let status = terminal(&full).await;
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
async fn http_creates_the_real_profile_and_serves_its_snapshot_and_transcripts() {
    use cna_server::http::App;
    use serde_json::{Value, json};
    use std::sync::Arc;
    let directory = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = App::new(
        directory.path().to_owned(),
        port,
        Arc::new(|request, dir| campaigns::create(dir, &data(), request)),
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
        .json(&request(cna_rules::PROFILE_DEV, "legal_random", true))
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
    for unit in side["snapshot"]["view"]["units"]
        .as_object()
        .unwrap()
        .values()
    {
        assert_eq!(unit["side"], "axis");
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
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let detail: Value = client
                .get(format!("{base}/api/campaigns/{id}"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if detail["status"]["state"] == "finished" {
                break;
            }
            assert_ne!(detail["status"]["state"], "stopped", "{detail}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let mut count = 0;
    for seat in SeatId::all() {
        let rows: Vec<ServerMessage> = client
            .get(format!(
                "{base}/api/campaigns/{id}/transcripts?perspective=operator&seat={seat}"
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        count += rows.len();
    }
    assert_initiative_and_transcript_count(&directory.path().join(format!("{id}.sqlite")), count);
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
