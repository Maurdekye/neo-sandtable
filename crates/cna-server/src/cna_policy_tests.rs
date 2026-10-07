//! Pre-submit failures use the actual CNA dispatcher and serialized campaign writer.
use super::{tests::movement_fixture, *};
use crate::{
    CampaignStatus,
    actor::auto_step,
    scripted::{Candidates, Step},
};
use cna_core::{
    decision::{ActionSchema, DecisionRequest},
    engine::{EngineError, Rejection},
    visibility::Perspective,
};
use rusqlite::{Connection, types::Value as SqlValue};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

// Compare every persisted table, omitting only the intended campaign status change.
fn rows(path: &Path) -> BTreeMap<String, Vec<Vec<SqlValue>>> {
    let db = Connection::open(path).unwrap();
    let names: Vec<String> = db.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
    ).unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            let columns = if name == "campaign" {
                "id,meta,pins,revision,rng,state_hash"
            } else {
                "*"
            };
            let mut statement = db
                .prepare(&format!("SELECT {columns} FROM \"{name}\""))
                .unwrap();
            let count = statement.column_count();
            let mut values: Vec<Vec<SqlValue>> = statement
                .query_map([], |r| (0..count).map(|i| r.get(i)).collect())
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            values.sort_by_key(|row| format!("{row:?}"));
            (name, values)
        })
        .collect()
}

fn fixture(path: &Path, mode: &str, mandatory: bool) -> (Campaign<Cna>, Pins, SeatId) {
    let (content, mut game) = movement_fixture();
    if mandatory {
        for pending in game.state.decisions.pending.iter_mut() {
            pending.space.pass = None;
        }
    }
    let pins = Pins {
        rules_profile: cna_rules::PROFILE_DEV.into(),
        content_hash: "real-roster-test-map".into(),
        engine_version: env!("CNA_ENGINE_SOURCE_HASH").into(),
    };
    let meta = CampaignMeta {
        id: "policy-failure".into(),
        scenario_id: "graziani".into(),
        rules_profile: pins.rules_profile.clone(),
        title: "Policy failure".into(),
        seats: vec![],
    };
    let mut campaign =
        Campaign::create(path, Cna::dev(), content, game, meta, pins.clone()).unwrap();
    let seat = campaign.pending()[0].seat;
    campaign
        .handover(
            seat,
            Some(ControllerInfo {
                kind: ControllerKind::Scripted,
                label: format!("scripted:{mode}"),
            }),
            json!({"mode":mode}),
        )
        .unwrap();
    (campaign, pins, seat)
}

fn failures() -> [EngineError; 2] {
    [
        EngineError::Unsupported {
            case: "land:23.22".into(),
            detail: "injected own policy source gap".into(),
        },
        EngineError::Invariant {
            detail: "injected engineering gross CP overflow".into(),
        },
    ]
}

struct NoSampling;
impl Candidates for NoSampling {
    fn candidate(&self, _: &DecisionRequest, _: &ActionSchema, _: u32) -> Option<Value> {
        panic!("a policy EngineError must never fall through to candidate sampling")
    }
}

#[test]
fn cna_policy_engine_errors_keep_exact_variants_before_sampling_or_any_command() {
    for error in failures() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("exact-error.sqlite");
        let (mut campaign, _, seat) = fixture(&path, "legal_random", false);
        let before = serde_json::to_value(campaign.game.as_ref()).unwrap();
        let bindings: Vec<_> = SeatId::all().map(|s| campaign.binding(s).clone()).collect();
        let before_rows = rows(&path);
        let injected = error.clone();
        let policy: ActionPolicy<Cna> = Box::new(move |_, _, _, _| Err(injected.clone()));
        assert!(
            matches!(auto_step(&mut campaign, None, &NoSampling, Some(&policy)),
            Err(Error::Rejected(Rejection::Engine(ref found))) if found == &error)
        );
        assert_eq!(
            campaign.status(),
            &CampaignStatus::Stopped {
                error: error.to_string()
            }
        );
        assert_eq!(rows(&path), before_rows);
        assert_eq!(
            serde_json::to_value(campaign.game.as_ref()).unwrap(),
            before
        );
        assert_eq!(
            SeatId::all()
                .map(|s| campaign.binding(s).clone())
                .collect::<Vec<_>>(),
            bindings
        );
        assert!(!campaign.binding(seat).paused);
        assert!(matches!(
            auto_step(&mut campaign, None, &NoSampling, Some(&policy)),
            Ok(Step::Idle)
        ));
    }
}

#[tokio::test]
async fn cna_policy_engine_errors_publish_durable_stops_with_healthy_bindings_and_recovery() {
    for mode in ["legal_random", "pass_when_possible", "aggressive"] {
        for error in failures() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("actor-error.sqlite");
            // The pass-mode hook only runs for mandatory windows. Legal-random is
            // deliberately offered a pass, proving it cannot replace a policy error.
            let (mut campaign, pins, seat) = fixture(&path, mode, mode == "pass_when_possible");
            campaign.set_paused(true).unwrap();
            let before = serde_json::to_value(campaign.game.as_ref()).unwrap();
            let bindings: Vec<_> = SeatId::all().map(|s| campaign.binding(s).clone()).collect();
            let views: Vec<_> = Perspective::all()
                .map(|p| campaign.view(p).unwrap())
                .collect();
            let before_rows = rows(&path);
            let calls = Arc::new(AtomicUsize::new(0));
            let policy_calls = calls.clone();
            let injected = error.clone();
            let handle = if mode == "aggressive" {
                CampaignHandle::spawn_with_fallible_baseline(
                    campaign,
                    &path,
                    Box::new(move |_, _, _| {
                        policy_calls.fetch_add(1, Ordering::SeqCst);
                        Err(injected.clone())
                    }),
                )
                .unwrap()
            } else {
                CampaignHandle::spawn_with_policy(
                    campaign,
                    &path,
                    Box::new(move |_, _, _, _| {
                        policy_calls.fetch_add(1, Ordering::SeqCst);
                        Err(injected.clone())
                    }),
                )
                .unwrap()
            };
            let mut status = handle.watch_status();
            handle.pause(false).await.unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
                while !matches!(*status.borrow_and_update(), CampaignStatus::Stopped { .. }) {
                    status.changed().await.unwrap();
                }
            })
            .await
            .expect("serialized policy failure must publish its stop");
            assert_eq!(
                handle.status(),
                CampaignStatus::Stopped {
                    error: error.to_string()
                }
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(rows(&path), before_rows);
            for (i, p) in Perspective::all().enumerate() {
                assert_eq!(handle.projection(p).view, views[i]);
            }
            for (i, s) in SeatId::all().enumerate() {
                assert_eq!(handle.seat(s).binding, bindings[i]);
            }
            assert!(!handle.seat(seat).binding.paused);
            assert!(matches!(handle.pause(false).await, Err(Error::NotRunning)));
            handle.shutdown().await.unwrap();
            let recovered =
                Campaign::recover(&path, Cna::dev(), movement_fixture().0, &pins).unwrap();
            assert_eq!(
                recovered.status(),
                &CampaignStatus::Stopped {
                    error: error.to_string()
                }
            );
            assert_eq!(
                serde_json::to_value(recovered.game.as_ref()).unwrap(),
                before
            );
            assert_eq!(rows(&path), before_rows);
            for (i, s) in SeatId::all().enumerate() {
                assert_eq!(recovered.binding(s), &bindings[i]);
            }
        }
    }
}

#[test]
fn cna_policy_stop_storage_failure_does_not_claim_persistence_or_submit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("failed-stop.sqlite");
    let (mut campaign, pins, _) = fixture(&path, "legal_random", false);
    let before = serde_json::to_value(campaign.game.as_ref()).unwrap();
    let before_rows = rows(&path);
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER deny_stop BEFORE UPDATE OF status ON campaign
        WHEN NEW.status LIKE '%stopped%' BEGIN SELECT RAISE(ABORT, 'injected stop failure'); END;",
    )
    .unwrap();
    let error = failures()[1].clone();
    let policy: ActionPolicy<Cna> = Box::new(move |_, _, _, _| Err(error.clone()));
    assert!(matches!(
        auto_step(&mut campaign, None, &NoSampling, Some(&policy)),
        Err(Error::Storage(_))
    ));
    assert_eq!(campaign.status(), &CampaignStatus::Running);
    assert_eq!(
        serde_json::to_value(campaign.game.as_ref()).unwrap(),
        before
    );
    assert_eq!(rows(&path), before_rows);
    drop(campaign);
    let recovered = Campaign::recover(&path, Cna::dev(), movement_fixture().0, &pins).unwrap();
    assert_eq!(recovered.status(), &CampaignStatus::Running);
    assert_eq!(
        serde_json::to_value(recovered.game.as_ref()).unwrap(),
        before
    );
    assert_eq!(rows(&path), before_rows);
}

#[tokio::test]
async fn cna_policy_stop_storage_failure_uses_fatal_actor_path_without_a_durable_claim() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fatal-stop.sqlite");
    let (mut campaign, pins, _) = fixture(&path, "legal_random", false);
    campaign.set_paused(true).unwrap();
    let before = serde_json::to_value(campaign.game.as_ref()).unwrap();
    let bindings: Vec<_> = SeatId::all().map(|s| campaign.binding(s).clone()).collect();
    let before_rows = rows(&path);
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER deny_stop BEFORE UPDATE OF status ON campaign
        WHEN NEW.status LIKE '%stopped%' BEGIN SELECT RAISE(ABORT, 'injected stop failure'); END;",
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let policy_calls = calls.clone();
    let handle = CampaignHandle::spawn_with_policy(
        campaign,
        &path,
        Box::new(move |_, _, _, _| {
            policy_calls.fetch_add(1, Ordering::SeqCst);
            Err(EngineError::Invariant {
                detail: "original policy failure".into(),
            })
        }),
    )
    .unwrap();
    let mut status = handle.watch_status();
    handle.pause(false).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(*status.borrow_and_update(), CampaignStatus::Stopped { .. }) {
            status.changed().await.unwrap();
        }
    })
    .await
    .expect("fatal actor status must be observable even with broken persistence");
    assert!(
        matches!(handle.status(), CampaignStatus::Stopped { ref error }
        if error.contains("storage failure") && error.contains("injected stop failure"))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    for (i, s) in SeatId::all().enumerate() {
        assert_eq!(handle.seat(s).binding, bindings[i]);
    }
    assert!(matches!(handle.shutdown().await, Err(Error::Storage(_))));
    assert_eq!(rows(&path), before_rows);
    let recovered = Campaign::recover(&path, Cna::dev(), movement_fixture().0, &pins).unwrap();
    assert_eq!(
        recovered.status(),
        &CampaignStatus::Running,
        "the failed durable stop must not survive recovery as if committed"
    );
    assert_eq!(
        serde_json::to_value(recovered.game.as_ref()).unwrap(),
        before
    );
    assert_eq!(rows(&path), before_rows);
}
