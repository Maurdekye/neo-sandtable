//! Missing m78/m86 boundaries only; existing Engine fixtures/assertions remain intact.
use super::*;
use cna_core::engine::Ruleset;
use cna_seats::game::ToolError;

fn bytes(value: &(impl serde::Serialize + ?Sized)) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn projections(campaign: &Campaign<Cna>) -> Vec<crate::actor::Projection> {
    Perspective::all()
        .map(|p| crate::actor::Projection {
            meta: campaign.metadata(p),
            seq: campaign.current_seq(p).unwrap(),
            view: campaign.view(p).unwrap(),
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
struct Database {
    schema: Vec<(String, String, String, Option<String>)>,
    columns: BTreeMap<String, Vec<String>>,
    tables: BTreeMap<String, Vec<Vec<SqlValue>>>,
}

fn database(path: &Path) -> Database {
    let db = Connection::open(path).unwrap();
    let schema = db
        .prepare("SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let names: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut columns = BTreeMap::new();
    let mut tables = BTreeMap::new();
    for name in names {
        let mut statement = db.prepare(&format!("SELECT * FROM \"{name}\"")).unwrap();
        columns.insert(
            name.clone(),
            statement
                .column_names()
                .into_iter()
                .map(str::to_owned)
                .collect(),
        );
        let count = statement.column_count();
        let mut rows = statement
            .query_map([], |row| (0..count).map(|i| row.get(i)).collect())
            .unwrap()
            .collect::<Result<Vec<Vec<SqlValue>>, _>>()
            .unwrap();
        rows.sort_by_key(|row| format!("{row:?}"));
        tables.insert(name, rows);
    }
    Database {
        schema,
        columns,
        tables,
    }
}

impl Database {
    fn with_status(mut self, status: &CampaignStatus) -> Self {
        let index = self.columns["campaign"]
            .iter()
            .position(|name| name == "status")
            .unwrap();
        let campaign = self.tables.get_mut("campaign").unwrap();
        assert_eq!(campaign.len(), 1);
        // Expected status is explicit. Every other column/table/schema byte is retained.
        campaign[0][index] = SqlValue::Text(serde_json::to_string(status).unwrap());
        self
    }
}

fn non_engine_failures() -> [Rejection; 4] {
    [
        Rejection::UnknownDecision {
            decision_id: "injected-policy-unknown".into(),
        },
        Rejection::WrongSeat {
            decision_id: "injected-policy-other-seat".into(),
            seat: SeatId::new(
                cna_protocol::Side::Commonwealth,
                cna_protocol::Role::Logistics,
            ),
        },
        Rejection::StaleRevision {
            expected: 23,
            got: 7,
        },
        Rejection::Illegal {
            message: "injected TOP-LEVEL policy defect, not candidate infeasibility".into(),
        },
    ]
}

async fn stopped_and_joined(handle: &CampaignHandle) -> (bool, Result<(), Error>) {
    let mut status = handle.watch_status();
    let observed = tokio::time::timeout(Duration::from_secs(10), async {
        status
            .wait_for(|status| matches!(status, CampaignStatus::Stopped { .. }))
            .await
            .map(|_| ())
    })
    .await;
    // Retain the ORIGINAL writer result, even if call(Shutdown) sees a closed mailbox.
    // Assertions/DB recovery occur only after shutdown has taken and joined that thread.
    let joined = handle.shutdown().await;
    (matches!(observed, Ok(Ok(()))), joined)
}

fn assert_redaction(rejection: &Rejection) {
    let public = crate::seats::seat_error(Error::Rejected(rejection.clone()));
    match (rejection, public) {
        (Rejection::UnknownDecision { decision_id }, ToolError::UnknownDecision(found))
        | (Rejection::WrongSeat { decision_id, .. }, ToolError::UnknownDecision(found)) => {
            assert_eq!(found, decision_id.to_string());
        }
        (Rejection::StaleRevision { expected, got }, ToolError::Stale(found)) => {
            assert_eq!(
                found,
                format!("decision revision {got}; current is {expected}")
            );
        }
        (Rejection::Illegal { message }, ToolError::Illegal(found)) => assert_eq!(found, *message),
        (Rejection::Engine(_), ToolError::Other(found)) => {
            assert_eq!(found, "campaign unavailable; retry or ask the operator");
        }
        (_, other) => panic!("existing seat redaction changed: {other:?}"),
    }
}

/// Cases: land:3.6, land:8.88, land:23.22
#[tokio::test]
async fn escaped_policy_rejections_keep_every_field_through_original_terminal_writer() {
    for mode in ["legal_random", "pass_when_possible"] {
        for expected in non_engine_failures() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("escaped-rejection.sqlite");
            let (mut campaign, pins, _) = fixture(&path, mode, mode == "pass_when_possible");
            let before_game = bytes(campaign.game.as_ref());
            let before_bindings: Vec<_> = SeatId::all()
                .map(|seat| campaign.binding(seat).clone())
                .collect();
            let before_views = projections(&campaign);
            let before = database(&path);
            let calls = Arc::new(AtomicUsize::new(0));
            let called = calls.clone();
            let injected = expected.clone();
            let policy: ActionPolicy<Cna> = Box::new(move |_, _, _, _| {
                called.fetch_add(1, Ordering::SeqCst);
                Err(injected.clone())
            });
            match auto_step(&mut campaign, None, &NoSampling, Some(&policy)) {
                Err(Error::Rejected(found)) => assert_eq!(found, expected),
                other => panic!("non-engine carrier changed: {other:?}"),
            }
            // The helper does NOT call stop_engine for these defects. Runtime owns stop.
            assert_eq!(campaign.status(), &CampaignStatus::Running);
            assert_eq!(database(&path), before);
            assert_eq!(bytes(campaign.game.as_ref()), before_game);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            campaign.set_paused(true).unwrap();
            let before_actor = database(&path);
            let handle = CampaignHandle::spawn_with_policy(campaign, &path, policy).unwrap();
            let resumed = handle.pause(false).await;
            let (observed, joined) = stopped_and_joined(&handle).await;
            // First assertions on the live path follow the original join.
            resumed.unwrap();
            assert!(observed);
            match joined {
                Err(Error::Rejected(found)) => assert_eq!(found, expected),
                other => panic!("original joined typed rejection lost: {other:?}"),
            }
            let stopped = CampaignStatus::Stopped {
                error: Error::Rejected(expected.clone()).to_string(),
            };
            assert_eq!(handle.status(), stopped);
            assert_eq!(
                calls.load(Ordering::SeqCst),
                2,
                "one direct helper call and one terminal actor call"
            );
            assert_eq!(database(&path), before_actor.with_status(&stopped));
            for (index, p) in Perspective::all().enumerate() {
                assert_eq!(handle.projection(p), before_views[index]);
            }
            for (index, seat) in SeatId::all().enumerate() {
                assert_eq!(handle.seat(seat).binding, before_bindings[index]);
            }
            let after = database(&path);
            let recovered =
                Campaign::recover(&path, Cna::dev(), movement_fixture().0, &pins).unwrap();
            assert_eq!(recovered.status(), &stopped);
            assert_eq!(bytes(recovered.game.as_ref()), before_game);
            assert_eq!(projections(&recovered), before_views);
            for (index, seat) in SeatId::all().enumerate() {
                assert_eq!(recovered.binding(seat), &before_bindings[index]);
            }
            assert_eq!(database(&path), after);
            assert_redaction(&expected);
        }
    }
}

/// Cases: land:3.6, land:8.88, land:21.25, land:23.22
#[tokio::test]
async fn canonical_inspect_rejections_are_exact_nonfatal_and_keep_seat_redaction() {
    for profile in [cna_rules::PROFILE_DEV, cna_rules::PROFILE_FULL] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nonfatal-inspect.sqlite");
        let (content, mut game) = movement_fixture();
        let unit: cna_core::ids::UnitId = "it.libyan_tank_command.xxi_l_tank_bn".into();
        // Build the actual public ledger, then inject ONLY its explicit overflow fault.
        let mut ledger = cna_rules::logistics::plan_segment_fuel(&content, &game.state, &unit, 0)
            .unwrap()
            .ledger;
        ledger.cp_quarters = i32::MAX;
        game.state
            .logistics
            .fuel_segments
            .insert(unit.clone(), ledger);
        let rules = ruleset(profile).unwrap();
        let request = rules
            .pending(&content, &game.state)
            .into_iter()
            .find(|request| request.seat.side == cna_protocol::Side::Axis)
            .unwrap();
        let seat = request.seat;
        let targets = [
            (
                unit.to_string(),
                Rejection::Engine(EngineError::Invariant {
                    detail: "movement query fuel CP overflow".into(),
                }),
            ),
            (
                "not-a-real-inspect-target".into(),
                Rejection::Illegal {
                    message: "not-a-real-inspect-target: unknown or not visible".into(),
                },
            ),
        ];
        let before_game = bytes(&game);
        for (target, expected) in &targets {
            assert_eq!(
                rules
                    .inspect(&content, &game.state, Perspective::Seat(seat), target)
                    .unwrap_err(),
                *expected
            );
            assert_eq!(bytes(&game), before_game);
        }
        let pins = Pins {
            rules_profile: profile.into(),
            content_hash: "real-roster-test-map".into(),
            engine_version: env!("CNA_ENGINE_SOURCE_HASH").into(),
        };
        let mut campaign = Campaign::create(
            &path,
            rules,
            content,
            game,
            CampaignMeta {
                id: "canonical-inspect-fault".into(),
                scenario_id: "graziani".into(),
                rules_profile: profile.into(),
                title: "Nonfatal canonical inspect".into(),
                seats: vec![],
            },
            pins.clone(),
        )
        .unwrap();
        campaign.set_paused(true).unwrap();
        let before = database(&path);
        let bindings: Vec<_> = SeatId::all()
            .map(|seat| campaign.binding(seat).clone())
            .collect();
        let views = projections(&campaign);
        let handle = CampaignHandle::spawn_with_policy(
            campaign,
            &path,
            Box::new(|_, _, _, _| panic!("paused inspect must never invoke a response policy")),
        )
        .unwrap();
        let first = handle.inspect(seat, &targets[0].0).await;
        let second = handle.inspect(seat, &targets[1].0).await;
        let still_alive = handle.pause(true).await;
        let joined = handle.shutdown().await;
        // A successful original join and following control call distinguish nonfatal inspect.
        joined.unwrap();
        still_alive.unwrap();
        for (actual, (_, expected)) in [first, second].into_iter().zip(&targets) {
            match actual {
                Err(Error::Rejected(found)) => assert_eq!(found, *expected),
                other => panic!("actor inspect must preserve exact canonical rejection: {other:?}"),
            }
            assert_redaction(expected);
        }
        assert_eq!(handle.status(), CampaignStatus::Paused);
        assert_eq!(database(&path), before);
        for (index, p) in Perspective::all().enumerate() {
            assert_eq!(handle.projection(p), views[index]);
        }
        for (index, seat) in SeatId::all().enumerate() {
            assert_eq!(handle.seat(seat).binding, bindings[index]);
        }
        let recovered = Campaign::recover(
            &path,
            ruleset(profile).unwrap(),
            movement_fixture().0,
            &pins,
        )
        .unwrap();
        assert_eq!(bytes(recovered.game.as_ref()), before_game);
        assert_eq!(recovered.status(), &CampaignStatus::Paused);
        assert_eq!(projections(&recovered), views);
        for (index, seat) in SeatId::all().enumerate() {
            assert_eq!(recovered.binding(seat), &bindings[index]);
        }
        assert_eq!(database(&path), before);
    }
    for engine in failures() {
        assert_redaction(&Rejection::Engine(engine));
    }
}

fn assert_stop_storage_error(error: &Error) {
    let Error::Storage(rusqlite::Error::SqliteFailure(code, message)) = error else {
        panic!("failed stop must retain its typed SQLite error: {error:?}")
    };
    assert_eq!(code.code, rusqlite::ErrorCode::ConstraintViolation);
    assert_eq!(code.extended_code, 1811); // SQLITE_CONSTRAINT_TRIGGER
    assert_eq!(message.as_deref(), Some("typed-control-stop-denied"));
}

/// Cases: land:3.6, land:29.1, airlog:52.42
#[tokio::test]
async fn real_unpaid_policy_reraises_same_error_after_failed_stops_and_running_recovery() {
    for profile in [cna_rules::PROFILE_DEV, cna_rules::PROFILE_FULL] {
        for fault in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("running-reraise.sqlite");
            let (mut campaign, pins, seat, expected) =
                stock_policy_checkpoint(&path, profile, Some(fault));
            let before_game = bytes(campaign.game.as_ref());
            let bindings: Vec<_> = SeatId::all()
                .map(|seat| campaign.binding(seat).clone())
                .collect();
            let views = projections(&campaign);
            let initial_rows = database(&path);
            let request = campaign
                .pending()
                .into_iter()
                .find(|request| request.seat == seat)
                .unwrap();
            let policy = movement_policy_with_profile(profile == cna_rules::PROFILE_FULL);
            assert_eq!(
                policy(
                    &campaign.content,
                    &campaign.game.state,
                    &request,
                    campaign.binding(seat).controller_epoch
                )
                .unwrap_err(),
                Rejection::Engine(expected.clone())
            );
            assert_eq!(bytes(campaign.game.as_ref()), before_game);
            assert_eq!(database(&path), initial_rows);
            campaign.set_paused(true).unwrap();
            drop(campaign);
            Connection::open(&path).unwrap().execute_batch(
                "CREATE TRIGGER deny_typed_stop BEFORE UPDATE OF status ON campaign
                 WHEN NEW.status LIKE '%stopped%' BEGIN SELECT RAISE(ABORT,'typed-control-stop-denied'); END;"
            ).unwrap();
            let before = database(&path);
            let data = cna_content::repo_data_dir();
            let first = recover(&path, &data, profile).unwrap();
            let resumed = first.pause(false).await;
            let (observed, joined) = stopped_and_joined(&first).await;
            resumed.unwrap();
            assert!(observed);
            let joined = joined.unwrap_err();
            assert_stop_storage_error(&joined);
            assert_eq!(
                first.status(),
                CampaignStatus::Stopped {
                    error: joined.to_string()
                }
            );
            for (index, p) in Perspective::all().enumerate() {
                assert_eq!(first.projection(p), views[index]);
            }
            for (index, s) in SeatId::all().enumerate() {
                assert_eq!(first.seat(s).binding, bindings[index]);
            }
            let running = before.with_status(&CampaignStatus::Running);
            assert_eq!(database(&path), running);

            // Existing public Running recovery is preserved, not replaced by STRICT's barrier.
            // Repeat the ACTUAL deterministic unpaid production query before any new writer.
            for attempt in 0..2 {
                let (content, _) = inputs(&data, profile).unwrap();
                let recovered =
                    Campaign::recover(&path, ruleset(profile).unwrap(), content, &pins).unwrap();
                assert_eq!(recovered.status(), &CampaignStatus::Running);
                assert_eq!(bytes(recovered.game.as_ref()), before_game);
                assert_eq!(projections(&recovered), views);
                for (index, s) in SeatId::all().enumerate() {
                    assert_eq!(recovered.binding(s), &bindings[index]);
                }
                let request = recovered
                    .pending()
                    .into_iter()
                    .find(|request| request.seat == seat)
                    .unwrap();
                let policy = movement_policy_with_profile(profile == cna_rules::PROFILE_FULL);
                assert_eq!(
                    policy(
                        &recovered.content,
                        &recovered.game.state,
                        &request,
                        recovered.binding(seat).controller_epoch
                    )
                    .unwrap_err(),
                    Rejection::Engine(expected.clone())
                );
                assert_eq!(bytes(recovered.game.as_ref()), before_game);
                assert_eq!(database(&path), running);
                drop(recovered);
                if attempt == 0 {
                    // Both writes still fail after restart. It must terminate without a command.
                    let second = recover(&path, &data, profile).unwrap();
                    let (observed, joined) = stopped_and_joined(&second).await;
                    assert!(observed);
                    let joined = joined.unwrap_err();
                    assert_stop_storage_error(&joined);
                    assert_eq!(
                        second.status(),
                        CampaignStatus::Stopped {
                            error: joined.to_string()
                        }
                    );
                    for (index, p) in Perspective::all().enumerate() {
                        assert_eq!(second.projection(p), views[index]);
                    }
                    for (index, s) in SeatId::all().enumerate() {
                        assert_eq!(second.seat(s).binding, bindings[index]);
                    }
                    assert_eq!(database(&path), running);
                }
            }
            // Once storage is restored, ordinary public recovery retries this SAME error's stop.
            Connection::open(&path)
                .unwrap()
                .execute_batch("DROP TRIGGER deny_typed_stop")
                .unwrap();
            let healthy_before = database(&path);
            let third = recover(&path, &data, profile).unwrap();
            let (observed, joined) = stopped_and_joined(&third).await;
            joined.unwrap();
            assert!(observed);
            let stopped = CampaignStatus::Stopped {
                error: expected.to_string(),
            };
            assert_eq!(third.status(), stopped);
            for (index, p) in Perspective::all().enumerate() {
                assert_eq!(third.projection(p), views[index]);
            }
            for (index, s) in SeatId::all().enumerate() {
                assert_eq!(third.seat(s).binding, bindings[index]);
            }
            assert_eq!(database(&path), healthy_before.with_status(&stopped));
            let committed = database(&path);
            let (content, _) = inputs(&data, profile).unwrap();
            let recovered =
                Campaign::recover(&path, ruleset(profile).unwrap(), content, &pins).unwrap();
            assert_eq!(recovered.status(), &stopped);
            assert_eq!(bytes(recovered.game.as_ref()), before_game);
            assert_eq!(projections(&recovered), views);
            for (index, s) in SeatId::all().enumerate() {
                assert_eq!(recovered.binding(s), &bindings[index]);
            }
            assert_eq!(database(&path), committed);
        }
    }
}
