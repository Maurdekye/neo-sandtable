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

// These checkpoints exercise the production policy/recover boundary, not setup reachability.
// The separately owned real Graziani dispatcher test proves source-created setup/stock/Move.
fn stock_policy_checkpoint(
    path: &Path,
    profile: &str,
    fault: Option<bool>,
) -> (Campaign<Cna>, Pins, SeatId, EngineError) {
    use cna_core::engine::Ruleset;
    use cna_protocol::Side;
    use cna_rules::{
        seq::{Block, OPSTAGE},
        state::{Location, WeatherState},
    };
    let (content, pins) = inputs(&cna_content::repo_data_dir(), profile).unwrap();
    let mut state = State::new(&content).unwrap();
    state.cursor.block = Block::OpStage;
    state.cursor.op_stage = Some(1);
    state.cursor.index = OPSTAGE
        .iter()
        .position(|p| p.anchor == "opstage.organization.water_distribution")
        .unwrap();
    state.cursor.entered = true;
    state.turn.weather = Some(WeatherState {
        kind: cna_tables::land::weather::WeatherKind::Normal,
        storm_sections: vec![],
    });
    for unit in state.land.units.values_mut() {
        unit.location = Location::Eliminated;
    }
    let id = cna_rules::logistics::pools::add_truck_pool(
        &mut state.logistics,
        None,
        Side::Axis,
        cna_content::scenario::Placement::Hex {
            hex: "C4020".into(),
        },
        Some(Location::Hex {
            hex: "C4020".into(),
        }),
        cna_content::units::Trucks {
            medium: 4,
            ..Default::default()
        },
        Default::default(),
    )
    .unwrap();
    cna_rules::logistics::pool_fuel::seed_created_pool(&mut state, &id).unwrap();
    let mut rng = CampaignRng::from_seed([37; 32]);
    cna_rules::logistics::batches::enter_water(
        &content,
        &mut state,
        &mut cna_core::engine::Cx {
            rng: &mut rng,
            events: &mut vec![],
        },
        false,
    )
    .unwrap();
    let expected = if fault == Some(false) {
        state.turn.weather = None;
        EngineError::Unsupported {
            case: "land:29.1".into(),
            detail: "required logistics content is unavailable".into(),
        }
    } else {
        if fault == Some(true) {
            state
                .logistics
                .truck_pools
                .iter_mut()
                .find(|p| p.id == id)
                .unwrap()
                .activity_water = cna_core::quantity::WaterPoints::new(-1);
        }
        EngineError::Invariant {
            detail: "pool water issue: negative activity reserve".into(),
        }
    };
    let rules = ruleset(profile).unwrap();
    let seat = rules
        .pending(&content, &state)
        .into_iter()
        .find(|p| p.seat.side == Side::Axis)
        .unwrap()
        .seat;
    let meta = CampaignMeta {
        id: "stock-policy-checkpoint".into(),
        scenario_id: "graziani".into(),
        rules_profile: profile.into(),
        title: "Explicit stock policy checkpoint".into(),
        seats: vec![],
    };
    let mut campaign = Campaign::create(
        path,
        rules,
        content,
        Game {
            state,
            rng: rng.state(),
        },
        meta,
        pins.clone(),
    )
    .unwrap();
    campaign
        .handover(
            seat,
            Some(ControllerInfo {
                kind: ControllerKind::Scripted,
                label: "scripted:legal_random".into(),
            }),
            json!({"mode":"legal_random"}),
        )
        .unwrap();
    (campaign, pins, seat, expected)
}

/// Cases: land:3.6, airlog:52.42
#[tokio::test]
async fn production_create_recover_preserve_full_dev_profile_pins_and_initial_game() {
    for profile in [cna_rules::PROFILE_FULL, cna_rules::PROFILE_DEV] {
        let directory = tempfile::tempdir().unwrap();
        let data = cna_content::repo_data_dir();
        let (content, pins) = inputs(&data, profile).unwrap();
        let expected = Game::<Cna> {
            state: State::new(&content).unwrap(),
            rng: CampaignRng::from_seed([17; 32]).state(),
        };
        let handle = create(
            directory.path(),
            &data,
            CreateRequest {
                kind: CampaignKind::Cna,
                rules_profile: profile.into(),
                seed: [17; 32],
                title: "Actual factory profile".into(),
                paused: true,
                controller: "legal_random".into(),
            },
        )
        .unwrap();
        let header = handle.snapshot(Perspective::Operator).header();
        assert_eq!(header.meta.rules_profile, profile);
        let path = directory.path().join(format!("{}.sqlite", header.meta.id));
        let before = rows(&path);
        let bindings: Vec<_> = SeatId::all()
            .map(|seat| handle.seat(seat).binding)
            .collect();
        handle.shutdown().await.unwrap();
        let loaded = Campaign::recover(&path, ruleset(profile).unwrap(), content, &pins).unwrap();
        assert_eq!(
            serde_json::to_value(loaded.game.as_ref()).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        drop(loaded);
        let wrong = if profile == cna_rules::PROFILE_DEV {
            cna_rules::PROFILE_FULL
        } else {
            cna_rules::PROFILE_DEV
        };
        assert!(matches!(
            recover(&path, &data, wrong),
            Err(Error::Recovery(_))
        ));
        assert_eq!(rows(&path), before);
        let restored = recover(&path, &data, profile).unwrap();
        assert_eq!(restored.snapshot(Perspective::Operator).header(), header);
        for (i, seat) in SeatId::all().enumerate() {
            assert_eq!(restored.seat(seat).binding, bindings[i]);
        }
        restored.shutdown().await.unwrap();
        assert_eq!(rows(&path), before);
    }
}

/// Cases: land:3.6, land:29.1, airlog:52.42
#[tokio::test]
async fn production_recover_policy_propagates_real_water_errors_before_any_command() {
    for profile in [cna_rules::PROFILE_FULL, cna_rules::PROFILE_DEV] {
        for fault in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("real-policy-stop.sqlite");
            let (mut campaign, pins, seat, expected) =
                stock_policy_checkpoint(&path, profile, Some(fault));
            let request = campaign
                .pending()
                .into_iter()
                .find(|p| p.seat == seat)
                .unwrap();
            let policy = movement_policy_with_profile(profile == cna_rules::PROFILE_FULL);
            let before_game = serde_json::to_value(campaign.game.as_ref()).unwrap();
            let actual = policy(
                &campaign.content,
                &campaign.game.state,
                &request,
                campaign.binding(seat).controller_epoch,
            )
            .unwrap_err();
            assert_eq!(actual, expected);
            assert_eq!(
                serde_json::to_value(campaign.game.as_ref()).unwrap(),
                before_game
            );
            let before_rows = rows(&path);
            assert!(
                matches!(auto_step(&mut campaign,None,&NoSampling,Some(&policy)),
                Err(Error::Rejected(Rejection::Engine(ref error))) if error==&expected)
            );
            assert_eq!(rows(&path), before_rows);
            assert_eq!(
                serde_json::to_value(campaign.game.as_ref()).unwrap(),
                before_game
            );
            assert!(!campaign.binding(seat).paused);
            drop(campaign);
            let handle = recover(&path, &cna_content::repo_data_dir(), profile).unwrap();
            assert_eq!(
                handle.status(),
                CampaignStatus::Stopped {
                    error: expected.to_string()
                }
            );
            handle.shutdown().await.unwrap();
            assert_eq!(rows(&path), before_rows);
            // Independently exercise the actual factory-installed actor policy from a running checkpoint.
            let path = directory.path().join("real-policy-actor.sqlite");
            let (mut campaign, _, seat, _) = stock_policy_checkpoint(&path, profile, Some(fault));
            campaign.set_paused(true).unwrap();
            let before_game = serde_json::to_value(campaign.game.as_ref()).unwrap();
            let bindings: Vec<_> = SeatId::all().map(|s| campaign.binding(s).clone()).collect();
            let before_rows = rows(&path);
            drop(campaign);
            let handle = recover(&path, &cna_content::repo_data_dir(), profile).unwrap();
            let before_view =
                serde_json::to_value(handle.projection(Perspective::Operator).view).unwrap();
            let mut status = handle.watch_status();
            handle.pause(false).await.unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
                while !matches!(*status.borrow_and_update(), CampaignStatus::Stopped { .. }) {
                    status.changed().await.unwrap();
                }
            })
            .await
            .expect("actual profile policy must publish its durable typed stop");
            assert_eq!(
                handle.status(),
                CampaignStatus::Stopped {
                    error: expected.to_string()
                }
            );
            assert_eq!(
                serde_json::to_value(handle.projection(Perspective::Operator).view).unwrap(),
                before_view
            );
            for (i, s) in SeatId::all().enumerate() {
                assert_eq!(handle.seat(s).binding, bindings[i]);
            }
            assert!(!handle.seat(seat).binding.paused);
            handle.shutdown().await.unwrap();
            assert_eq!(rows(&path), before_rows);
            let (content, _) = inputs(&cna_content::repo_data_dir(), profile).unwrap();
            let restored =
                Campaign::recover(&path, ruleset(profile).unwrap(), content, &pins).unwrap();
            assert_eq!(
                restored.status(),
                &CampaignStatus::Stopped {
                    error: expected.to_string()
                }
            );
            assert_eq!(
                serde_json::to_value(restored.game.as_ref()).unwrap(),
                before_game
            );
            assert_eq!(rows(&path), before_rows);
        }
    }
}

/// Cases: land:3.6, land:29.1, airlog:52.42
#[tokio::test]
async fn production_recover_policy_storage_failure_keeps_game_and_command_tables_uncommitted() {
    for profile in [cna_rules::PROFILE_FULL, cna_rules::PROFILE_DEV] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("real-policy-storage.sqlite");
        let (mut campaign, pins, _, _) = stock_policy_checkpoint(&path, profile, Some(true));
        let before = serde_json::to_value(campaign.game.as_ref()).unwrap();
        let before_rows = rows(&path);
        Connection::open(&path).unwrap().execute_batch(
            "CREATE TRIGGER deny_stop BEFORE UPDATE OF status ON campaign WHEN NEW.status LIKE '%stopped%'
             BEGIN SELECT RAISE(ABORT,'injected stop failure'); END;").unwrap();
        let policy = movement_policy_with_profile(profile == cna_rules::PROFILE_FULL);
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
        campaign.set_paused(true).unwrap();
        drop(campaign);
        let handle = recover(&path, &cna_content::repo_data_dir(), profile).unwrap();
        let mut status = handle.watch_status();
        handle.pause(false).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !matches!(*status.borrow_and_update(), CampaignStatus::Stopped { .. }) {
                status.changed().await.unwrap();
            }
        })
        .await
        .expect("storage failure must publish the fatal actor status");
        assert!(
            matches!(handle.status(),CampaignStatus::Stopped {error} if error.contains("storage failure")&&error.contains("injected stop failure"))
        );
        assert!(matches!(handle.shutdown().await, Err(Error::Storage(_))));
        assert_eq!(rows(&path), before_rows);
        let (content, _) = inputs(&cna_content::repo_data_dir(), profile).unwrap();
        let restored = Campaign::recover(&path, ruleset(profile).unwrap(), content, &pins).unwrap();
        assert_eq!(restored.status(), &CampaignStatus::Running);
        assert_eq!(
            serde_json::to_value(restored.game.as_ref()).unwrap(),
            before
        );
        assert_eq!(rows(&path), before_rows);
    }
}

/// Cases: land:3.6, airlog:52.42
#[test]
fn production_water_policy_healthy_answer_is_deterministic_and_leaves_campaign_rng_untouched() {
    for profile in [cna_rules::PROFILE_FULL, cna_rules::PROFILE_DEV] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("healthy-policy.sqlite");
        let (campaign, _, seat, _) = stock_policy_checkpoint(&path, profile, None);
        let before = serde_json::to_value(campaign.game.as_ref()).unwrap();
        let before_rows = rows(&path);
        let request = campaign
            .pending()
            .into_iter()
            .find(|p| p.seat == seat)
            .unwrap();
        let policy = movement_policy_with_profile(profile == cna_rules::PROFILE_FULL);
        let action = policy(
            &campaign.content,
            &campaign.game.state,
            &request,
            campaign.binding(seat).controller_epoch,
        )
        .unwrap()
        .unwrap();
        assert!(!action.is_null());
        assert!(action["pool_allocations"].is_array());
        assert_eq!(
            policy(
                &campaign.content,
                &campaign.game.state,
                &request,
                campaign.binding(seat).controller_epoch
            )
            .unwrap(),
            Some(action)
        );
        assert_eq!(
            serde_json::to_value(campaign.game.as_ref()).unwrap(),
            before
        );
        assert_eq!(rows(&path), before_rows);
    }
}

/// Cases: land:3.6, scen:60.23
#[tokio::test]
async fn production_full_create_publishes_its_actual_initial_source_error_without_command() {
    use cna_core::engine::{Command, evaluate};
    let directory = tempfile::tempdir().unwrap();
    let data = cna_content::repo_data_dir();
    let (content, pins) = inputs(&data, cna_rules::PROFILE_FULL).unwrap();
    let initial = Game::<Cna> {
        state: State::new(&content).unwrap(),
        rng: CampaignRng::from_seed([17; 32]).state(),
    };
    let Rejection::Engine(expected) =
        evaluate(&Cna::full(), &content, &initial, &Command::Advance).unwrap_err()
    else {
        panic!("actual incomplete source must retain a typed engine error")
    };
    assert!(matches!(expected, EngineError::Unsupported { .. }));
    let handle = create(
        directory.path(),
        &data,
        CreateRequest {
            kind: CampaignKind::Cna,
            rules_profile: cna_rules::PROFILE_FULL.into(),
            seed: [17; 32],
            title: "Full source stop".into(),
            paused: true,
            controller: "legal_random".into(),
        },
    )
    .unwrap();
    let path = directory.path().join(format!(
        "{}.sqlite",
        handle.header(Perspective::Operator).meta.id
    ));
    let before = rows(&path);
    let bindings: Vec<_> = SeatId::all()
        .map(|seat| handle.seat(seat).binding)
        .collect();
    let mut status = handle.watch_status();
    handle.pause(false).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(*status.borrow_and_update(), CampaignStatus::Stopped { .. }) {
            status.changed().await.unwrap();
        }
    })
    .await
    .expect("actual FULL factory must publish the initial source stop");
    assert_eq!(
        handle.status(),
        CampaignStatus::Stopped {
            error: expected.to_string()
        }
    );
    for (i, seat) in SeatId::all().enumerate() {
        assert_eq!(handle.seat(seat).binding, bindings[i]);
    }
    handle.shutdown().await.unwrap();
    assert_eq!(rows(&path), before);
    let restored = Campaign::recover(&path, Cna::full(), content, &pins).unwrap();
    assert_eq!(
        restored.status(),
        &CampaignStatus::Stopped {
            error: expected.to_string()
        }
    );
    assert_eq!(
        serde_json::to_value(restored.game.as_ref()).unwrap(),
        serde_json::to_value(initial).unwrap()
    );
}
