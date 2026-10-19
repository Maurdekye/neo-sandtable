//! SOURCE AUTHORED/UNRUN. Reuses the existing NullPolicyRules; no new Engine.
use super::*;
use crate::owned_retained::JoinObservation;
use crate::owned_writer::WriterJoin;

fn campaign(path: &Path) -> Campaign<NullPolicyRules> {
    let pins = Pins { rules_profile: "null-policy-test-v1".into(),
        content_hash: "empty".into(), engine_version: "policy-test-v1".into() };
    Campaign::create(path, NullPolicyRules {
        pass: false, responses: Arc::new(AtomicUsize::new(0)),
    }, (), Game { state: 0, rng: CampaignRng::from_seed([7; 32]).state() },
        CampaignMeta { id: "owned-writer-fixture".into(), scenario_id: "test".into(),
            rules_profile: pins.rules_profile.clone(), title: "Owned writer".into(), seats: vec![] }, pins).unwrap()
}

#[tokio::test]
async fn owned_actual_writer_publishes_before_go_and_captures_terminal_after_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owned-writer.sqlite");
    let campaign = campaign(&path);
    // Fixed row capacity is a fixture, not a full original F/G producer claim.
    let mut owner = WriterJoin::<Result<(), Error>, 4096>::new_prepaid();
    let handle = match CampaignHandle::spawn_owned_with_controllers(
        campaign, &path, None, Box::new(NoCandidates), None, &mut owner,
    ) {
        Ok(handle) => handle,
        Err(_) => panic!("owned source fixture constructor refused"),
    };
    assert!(!owner.can_publish());
    assert!(handle.inner.thread.lock().unwrap().is_none());
    assert!(owner.record().initial().is_none());
    assert!(owner.record().terminal().is_none());
    let observer = handle.clone();
    drop(observer); // Facade drop cannot take the original standard join handle.
    owner.release().unwrap();
    handle.pause(true).await.unwrap();
    assert!(owner.record().initial().unwrap().is_ok());
    assert!(owner.record().terminal().is_none());
    let shutdown: Result<(), Error> = handle.call(Op::Shutdown).await;
    shutdown.unwrap();
    owner.dispatch_join(&tokio::runtime::Handle::current()).unwrap();
    assert!(matches!(owner.join_until(tokio::time::Instant::now()
        + std::time::Duration::from_secs(120)).await,
        Ok(JoinObservation::Returned { original: Ok(Ok(Ok(()))), was_unknown: false })));
    let initial = owner.record().initial().unwrap().as_ref().unwrap();
    let terminal = owner.record().terminal().unwrap().as_ref().unwrap();
    let evidence = owner.terminal_rows().unwrap();
    assert!(std::ptr::eq(evidence.initial, initial));
    assert!(std::ptr::eq(evidence.terminal, terminal));
    assert!(matches!(evidence.original_join, Ok(Ok(Ok(())))));
    assert_eq!(initial.field(0), terminal.field(0));
    assert_eq!(initial.field(1), terminal.field(1));
    assert_eq!(initial.field(4), terminal.field(4));
    // Actual durable row, not captured viewer status or a regenerated game hash.
    let db = rusqlite::Connection::open(&path).unwrap();
    let stored: (String, String, u64, String, String) = db.query_row(
        "SELECT meta,pins,revision,state_hash,status FROM campaign WHERE id=1", [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    ).unwrap();
    assert_eq!(terminal.field(0).unwrap(), stored.0.as_bytes());
    assert_eq!(terminal.field(1).unwrap(), stored.1.as_bytes());
    assert_eq!(terminal.revision, stored.2);
    assert_eq!(terminal.field(2).unwrap(), stored.3.as_bytes());
    assert_eq!(terminal.field(3).unwrap(), stored.4.as_bytes());
}

#[tokio::test]
async fn actual_initial_row_failure_stays_raw_and_refuses_writer_evidence() {
    use crate::owned_writer::{RowFailure, WriterEvidenceRefusal};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owned-row-failure.sqlite");
    let mut owner = WriterJoin::<Result<(), Error>, 4096>::new_prepaid();
    let handle = match CampaignHandle::spawn_owned_with_controllers(
        campaign(&path), &path, None, Box::new(NoCandidates), None, &mut owner,
    ) {
        Ok(handle) => handle,
        Err(_) => panic!("owned source fixture constructor refused"),
    };
    assert!(owner.record().initial().is_none());
    // Synthetic fault BEFORE release: the writer must preserve the actual read
    // failure, not replace it with a stale initial projector or an empty row.
    let fault = rusqlite::Connection::open(&path).unwrap();
    fault.execute("DELETE FROM campaign WHERE id=1", []).unwrap();
    drop(fault);
    owner.release().unwrap();
    owner.dispatch_join(&tokio::runtime::Handle::current()).unwrap();
    assert!(matches!(owner.join_until(tokio::time::Instant::now()
        + std::time::Duration::from_secs(120)).await,
        Ok(JoinObservation::Returned { original: Ok(Ok(Err(Error::NotRunning))), was_unknown: false })));
    let raw = owner.record().initial().unwrap();
    assert!(matches!(raw, Err(RowFailure::Storage(rusqlite::Error::QueryReturnedNoRows))));
    assert!(std::ptr::eq(raw, owner.record().initial().unwrap()));
    assert!(owner.record().terminal().is_none());
    assert!(matches!(owner.terminal_rows(), Err(WriterEvidenceRefusal::WriterFailed)));
    assert!(handle.inner.thread.lock().unwrap().is_none());
}

// Retained-stop integration ADD; original two controls/fixture stay unchanged.
#[tokio::test]
async fn actual_writer_ack_survives_cancelled_stop_observer_before_original_join() {
    use crate::actor::{OwnedStop, StopObservation};
    use std::{future::Future, time::Duration};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owned-stop.sqlite");
    let mut owner = WriterJoin::<Result<(), Error>, 4096>::new_prepaid();
    let handle = match CampaignHandle::spawn_owned_with_controllers(
        campaign(&path), &path, None, Box::new(NoCandidates), None, &mut owner,
    ) {
        Ok(handle) => handle,
        Err(_) => panic!("owned stop control constructor refused"),
    };
    let mut stop = OwnedStop::new_prepaid(&handle); // Before actual writer GO.
    owner.release().unwrap();
    handle.pause(true).await.unwrap();
    let (entered, entry) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    let (reply, mut fence_reply) = tokio::sync::oneshot::channel();
    handle.inner.tx.send(Envelope {
        op: Op::WriterFence { entered, release: held }, reply,
    }).await.ok().unwrap();
    crate::host_gate::wake(&handle.inner.wake);
    entry.recv_timeout(Duration::from_secs(5)).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    {
        let mut observation = std::pin::pin!(stop.settle_until(deadline));
        std::future::poll_fn(|cx| {
            assert!(observation.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        }).await;
    }
    assert!(matches!(stop.observe(), StopObservation::Pending { queued: true }));
    assert!(matches!(owner.observe(), JoinObservation::Pending));
    release.send(()).unwrap();
    assert!(matches!((&mut fence_reply).await, Ok(Ok(Value::Null))));
    let address = match stop.settle_until(deadline + Duration::from_secs(120)).await {
        StopObservation::Reply { original: original @ Ok(Ok(Value::Null)), prior_timeout: None } => original as *const _,
        _ => panic!("actual writer stop reply was not preserved"),
    };
    // ACK alone did not dispatch/take the independent original std join.
    assert!(matches!(owner.observe(), JoinObservation::Pending));
    assert_eq!(stop.deadline(), Some(deadline));
    owner.dispatch_join(&tokio::runtime::Handle::current()).unwrap();
    assert!(matches!(owner.join_until(deadline).await,
        Ok(JoinObservation::Returned { original: Ok(Ok(Ok(()))), was_unknown: false })));
    match stop.settle_until(deadline + Duration::from_secs(120)).await {
        StopObservation::Reply { original, prior_timeout: None } => assert_eq!(original as *const _, address),
        _ => panic!("writer join replaced original ACK"),
    }
    assert!(owner.terminal_rows().is_ok());
    assert!(handle.inner.thread.lock().unwrap().is_none());
}

// Prepared-binding controls ADD; all preceding fixtures/assertions stay exact.
#[tokio::test]
async fn actual_prepared_identity_precedes_go_and_remains_same_original_in_evidence() {
    use crate::actor::{OwnedStop, StopObservation};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owned-prepared.sqlite");
    let mut owner = WriterJoin::<Result<(), Error>, 4096>::new_prepaid();
    let handle = match CampaignHandle::spawn_owned_with_controllers(
        campaign(&path), &path, None, Box::new(NoCandidates), None, &mut owner,
    ) {
        Ok(handle) => handle,
        Err(_) => panic!("prepared binding control constructor refused"),
    };
    let identity = owner.prepared_identity().unwrap();
    assert_eq!(identity.meta.id, "owned-writer-fixture");
    assert_eq!(identity.meta.rules_profile, "null-policy-test-v1");
    assert_eq!(identity.pins.rules_profile, identity.meta.rules_profile);
    assert_eq!(identity.pins.content_hash, "empty");
    assert_eq!(identity.pins.engine_version, "policy-test-v1");
    assert!(std::ptr::eq(identity.meta, owner.record().prepared_meta().unwrap().as_ref().unwrap()));
    assert!(std::ptr::eq(identity.pins, owner.record().prepared_pins().unwrap().as_ref().unwrap()));
    assert_eq!(identity.connection_path, owner.record().prepared_path().unwrap().as_ref().unwrap().as_path());
    let row_address = identity.row as *const _;
    assert!(owner.record().initial().is_none());
    assert!(owner.record().terminal().is_none());
    assert!(owner.record().start_binding().is_none());
    let mut stop = OwnedStop::new_prepaid(&handle);
    owner.release().unwrap();
    handle.pause(true).await.unwrap();
    assert!(matches!(owner.record().start_binding(), Some(Ok(()))));
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
    assert!(matches!(stop.settle_until(deadline).await,
        StopObservation::Reply { original: Ok(Ok(Value::Null)), prior_timeout: None }));
    owner.dispatch_join(&tokio::runtime::Handle::current()).unwrap();
    assert!(matches!(owner.join_until(deadline).await,
        Ok(JoinObservation::Returned { original: Ok(Ok(Ok(()))), was_unknown: false })));
    let evidence = owner.terminal_rows().unwrap();
    assert_eq!(evidence.prepared as *const _, row_address);
    for index in 0..5 { assert_eq!(evidence.prepared.field(index), evidence.initial.field(index)); }
    assert_eq!(evidence.prepared.revision, evidence.initial.revision);
}

#[tokio::test]
async fn actual_stored_pin_drift_before_go_retains_both_rows_and_refuses_writer_work() {
    use crate::owned_writer::{StartBindingFailure, WriterEvidenceRefusal};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owned-pin-drift.sqlite");
    let mut owner = WriterJoin::<Result<(), Error>, 4096>::new_prepaid();
    let handle = match CampaignHandle::spawn_owned_with_controllers(
        campaign(&path), &path, None, Box::new(NoCandidates), None, &mut owner,
    ) {
        Ok(handle) => handle,
        Err(_) => panic!("pin drift control constructor refused"),
    };
    let mut changed = owner.prepared_identity().unwrap().pins.clone();
    changed.content_hash = "different-stored-pin".into();
    let encoded = serde_json::to_string(&changed).unwrap();
    let fault = rusqlite::Connection::open(&path).unwrap();
    fault.execute("UPDATE campaign SET pins=?1 WHERE id=1", [&encoded]).unwrap();
    drop(fault);
    owner.release().unwrap();
    owner.dispatch_join(&tokio::runtime::Handle::current()).unwrap();
    assert!(matches!(owner.join_until(tokio::time::Instant::now()
        + std::time::Duration::from_secs(120)).await,
        Ok(JoinObservation::Returned { original: Ok(Ok(Err(Error::NotRunning))), was_unknown: false })));
    let prepared = owner.record().prepared().unwrap().as_ref().unwrap();
    let initial = owner.record().initial().unwrap().as_ref().unwrap();
    assert_ne!(prepared.field(1), initial.field(1));
    assert_eq!(initial.field(1), Some(encoded.as_bytes()));
    assert_eq!(prepared.revision, initial.revision);
    assert!(matches!(owner.record().start_binding(),
        Some(Err(StartBindingFailure::ChangedField { index: 1 }))));
    assert!(owner.record().terminal().is_none());
    assert!(matches!(owner.terminal_rows(), Err(WriterEvidenceRefusal::WriterFailed)));
    assert!(handle.inner.thread.lock().unwrap().is_none());
}

#[tokio::test]
async fn actual_prepared_serde_failure_stays_original_when_parent_refuses_go_and_joins() {
    use crate::owned_writer::WriterEvidenceRefusal;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owned-prepared-json.sqlite");
    let campaign = campaign(&path);
    let fault = rusqlite::Connection::open(&path).unwrap();
    fault.execute("UPDATE campaign SET pins='{' WHERE id=1", []).unwrap();
    drop(fault);
    let mut owner = WriterJoin::<Result<(), Error>, 4096>::new_prepaid();
    let handle = match CampaignHandle::spawn_owned_with_controllers(
        campaign, &path, None, Box::new(NoCandidates), None, &mut owner,
    ) {
        Ok(handle) => handle,
        Err(_) => panic!("invalid stored JSON control constructor refused"),
    };
    assert!(matches!(owner.prepared_identity(), Err(WriterEvidenceRefusal::PreparedPinsUnavailable)));
    let address = match owner.record().prepared_pins().unwrap() {
        Err(raw) => { assert!(raw.is_eof()); raw as *const _ }
        _ => panic!("actual prepared JSON error was replaced"),
    };
    assert_eq!(owner.record().prepared_meta().unwrap().as_ref().unwrap().id, "owned-writer-fixture");
    assert!(owner.record().prepared_path().unwrap().is_ok());
    assert!(owner.record().initial().is_none());
    owner.cancel_before_release().unwrap();
    owner.dispatch_join(&tokio::runtime::Handle::current()).unwrap();
    assert!(matches!(owner.join_until(tokio::time::Instant::now()
        + std::time::Duration::from_secs(120)).await,
        Ok(JoinObservation::Returned { original: Ok(Ok(Err(Error::NotRunning))), was_unknown: false })));
    match owner.record().prepared_pins().unwrap() {
        Err(raw) => assert_eq!(raw as *const _, address),
        _ => panic!("parent cancellation/join lost the original JSON cause"),
    }
    assert!(owner.record().initial().is_none());
    assert!(owner.record().terminal().is_none());
    assert!(handle.inner.thread.lock().unwrap().is_none());
}
