#[path = "../build_fingerprint.rs"]
mod build_fingerprint;
use cna_core::visibility::Perspective;
use cna_server::{
    Error,
    http::{CampaignKind, CreateRequest},
    sandbox,
};
use std::path::PathBuf;
#[test]
fn complete_build_fingerprint_tracks_helpers_manifests_and_dependency_versions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("Cargo.lock"), "lock-v1").unwrap();
    std::fs::write(root.join("Cargo.toml"), "workspace-v1").unwrap();
    for name in build_fingerprint::CRATES {
        let package = root.join("crates").join(name);
        std::fs::create_dir_all(package.join("src")).unwrap();
        std::fs::write(package.join("Cargo.toml"), name).unwrap();
        std::fs::write(package.join("src/lib.rs"), "version1").unwrap();
    }
    let initial = build_fingerprint::fingerprint(root).unwrap().0;
    for file in [
        "crates/cna-core/src/hex.rs",
        "crates/cna-protocol/src/lib.rs",
        "crates/cna-content/src/lib.rs",
        "crates/cna-tables/src/lib.rs",
        "crates/cna-rules/src/lib.rs",
        "crates/cna-sandbox/src/lib.rs",
        "crates/cna-core/Cargo.toml",
        "Cargo.lock",
        "Cargo.toml",
    ] {
        let path = root.join(file);
        let old = std::fs::read(&path).ok();
        std::fs::write(&path, "semantic-change").unwrap();
        assert_ne!(
            initial,
            build_fingerprint::fingerprint(root).unwrap().0,
            "{file}"
        );
        match old {
            Some(bytes) => std::fs::write(path, bytes).unwrap(),
            None => std::fs::remove_file(path).unwrap(),
        }
        assert_eq!(initial, build_fingerprint::fingerprint(root).unwrap().0);
    }
}
#[tokio::test]
async fn changed_sandbox_pins_are_rejected_before_checkpoint_interpretation() {
    let dir = tempfile::tempdir().unwrap();
    let data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let handle = sandbox::create(
        dir.path(),
        &data,
        CreateRequest {
            kind: CampaignKind::Sandbox,
            rules_profile: "sandbox-v1".into(),
            seed: [7; 32],
            title: "Pins".into(),
            paused: true,
            controller: "human".into(),
        },
    )
    .unwrap();
    let id = handle.projection(Perspective::Operator).meta.id;
    handle.shutdown().await.unwrap();
    let path = dir.path().join(format!("{id}.sqlite"));
    let db = rusqlite::Connection::open(&path).unwrap();
    let text: String = db
        .query_row("SELECT pins FROM campaign", [], |r| r.get(0))
        .unwrap();
    let mut pins: cna_server::Pins = serde_json::from_str(&text).unwrap();
    assert_eq!(pins.engine_version, env!("CNA_ENGINE_SOURCE_HASH"));
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        0
    );
    pins.engine_version = "obsolete-partial-fingerprint".into();
    db.execute(
        "UPDATE campaign SET pins=?",
        [serde_json::to_string(&pins).unwrap()],
    )
    .unwrap();
    db.execute("UPDATE checkpoints SET game='malformed checkpoint'", [])
        .unwrap();
    drop(db);
    assert!(
        matches!(sandbox::recover(&path,&data), Err(Error::Recovery(message)) if message == "immutable input pins changed")
    );
}
