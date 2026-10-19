//! Source controls only: UNCOMPILED and UNRUN. No positive custody fixture.
use super::*;
use std::time::Duration;
use std::future::Future;

fn owner(directory: PathBuf, data: PathBuf) -> CreationOwner<1, 512> {
    CreationOwner::new_prepaid(Handle::current(), uuid::Uuid::from_u128(804), directory, data)
}

async fn custody_requested(shared: &Shared<1>) {
    loop {
        let changed = shared.owner_changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if shared.replies[0].custody_requested.get().is_some() { return; }
        changed.await;
    }
}

async fn reach_custody_gate(owner: &mut CreationOwner<1, 512>) {
    let shared = Arc::clone(&owner.shared);
    tokio::select! {
        () = custody_requested(&shared) => {},
        joined = std::future::poll_fn(|cx| owner.tasks[0].poll_completion(cx)) => {
            assert!(joined.is_ok());
            // A real input-loader failure must fail this fixture after the
            // actual join, rather than hanging its marker waiter indefinitely.
            assert!(shared.replies[0].custody_requested.get().is_some());
        }
    }
}

#[tokio::test]
async fn queued_guarded_recovery_close_keeps_sole_path_and_no_task() {
    let mut owner = owner(PathBuf::new(), PathBuf::new());
    let path = PathBuf::from("never-open-guarded-recovery.sqlite");
    let pointer = path.as_os_str().as_encoded_bytes().as_ptr();
    let key = owner.reserve_guarded_recovery(path).unwrap();
    assert_eq!(owner.require_custody_window(key), Err(IngressRefusal::MissingAttempt));
    let observer = owner.observer();
    drop(observer);
    let cutoff = Instant::now() + Duration::from_secs(120);
    owner.close(cutoff).unwrap();
    owner.dispatch_guarded_pending().unwrap();
    assert!(owner.tasks[0].is_unpublished());
    assert!(owner.writers[0].can_publish()); // No claim because dispatch never started.
    assert_eq!(owner.custody_record(key).unwrap().attempt_path().unwrap()
        .as_os_str().as_encoded_bytes().as_ptr(), pointer);
    assert!(matches!(owner.observer().observe(key).unwrap(), CreationObservation::CancelledBeforeEffects));
}

#[tokio::test]
async fn guarded_recovery_cancellation_waits_before_reader_and_retains_raw_result() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("absent.sqlite");
    let pointer = path.as_os_str().as_encoded_bytes().as_ptr();
    let mut owner = owner(directory.path().to_owned(), PathBuf::from("unused-data"));
    let key = owner.reserve_guarded_recovery(path).unwrap();
    owner.dispatch_guarded_pending().unwrap();
    reach_custody_gate(&mut owner).await;
    let observer = owner.observer();
    {
        let wait = observer.wait(key);
        tokio::pin!(wait);
        assert!(std::future::poll_fn(|cx| Poll::Ready(wait.as_mut().poll(cx).is_pending())).await);
    }
    drop(observer); // Borrow cancellation cannot take the worker or parent path.
    let cutoff = Instant::now() + Duration::from_secs(120);
    owner.close(cutoff).unwrap();
    std::future::poll_fn(|cx| owner.poll_guarded_creators(cx)).await.unwrap();
    let original = owner.shared.replies[0].product.get().unwrap();
    assert!(matches!(original, Err(WriterStartFailure::Setup(Error::NotRunning))));
    let pointer_result = original as *const _;
    assert!(!directory.path().join("absent.sqlite").exists());
    assert!(owner.custody_record(key).unwrap().connection_custody().is_none());
    assert!(owner.custody_record(key).unwrap().prepared().is_none());
    assert!(!owner.writers[0].can_publish()); // Original claim remains consumed, never recycled.
    owner.join_creators_until(cutoff).await.unwrap();
    assert_eq!(owner.shared.replies[0].product.get().unwrap() as *const _, pointer_result);
    assert_eq!(owner.custody_record(key).unwrap().attempt_path().unwrap()
        .as_os_str().as_encoded_bytes().as_ptr(), pointer);
}

#[tokio::test]
async fn fresh_plan_keeps_generated_path_before_mint_without_creating_directory_or_db() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("not-created-by-server");
    let mut owner = owner(root.clone(), cna_content::repo_data_dir());
    let request = CreateRequest { title: "guarded pending".into(), kind: crate::http::CampaignKind::Sandbox,
        rules_profile: cna_sandbox::PROFILE_ID.into(), controller: "human".into(), seed: [81; 32], paused: true };
    let key = owner.observer().submit(CreationInput::Fresh(request)).unwrap();
    owner.dispatch_guarded_pending().unwrap();
    reach_custody_gate(&mut owner).await;
    let path = owner.custody_record(key).unwrap().attempt_path().unwrap();
    let pointer = path.as_os_str().as_encoded_bytes().as_ptr();
    assert_eq!(path.parent(), Some(root.as_path()));
    assert_eq!(path.extension().and_then(|s| s.to_str()), Some("sqlite"));
    assert!(uuid::Uuid::parse_str(path.file_stem().unwrap().to_str().unwrap()).is_ok());
    assert!(owner.shared.replies[0].product.get().is_none());
    assert!(!owner.writers[0].can_publish());
    owner.close(Instant::now() + Duration::from_secs(120)).unwrap();
    std::future::poll_fn(|cx| owner.poll_guarded_creators(cx)).await.unwrap();
    assert!(!root.exists());
    assert_eq!(owner.custody_record(key).unwrap().attempt_path().unwrap()
        .as_os_str().as_encoded_bytes().as_ptr(), pointer);
    assert!(matches!(owner.shared.replies[0].product.get(), Some(Err(WriterStartFailure::Setup(Error::NotRunning)))));
}

#[tokio::test]
async fn invalid_guarded_cna_preparation_retains_original_string_without_path_or_writer() {
    let mut owner = owner(PathBuf::from("unused"), PathBuf::from("never-read-content"));
    let request = CreateRequest { title: "invalid".into(), kind: crate::http::CampaignKind::Cna,
        rules_profile: cna_rules::PROFILE_FULL.into(), controller: "unsupported".into(), seed: [9; 32], paused: false };
    let key = owner.observer().submit(CreationInput::Fresh(request)).unwrap();
    owner.dispatch_guarded_pending().unwrap();
    std::future::poll_fn(|cx| owner.poll_guarded_creators(cx)).await.unwrap();
    let pointer = match owner.shared.replies[0].product.get().unwrap() {
        Err(WriterStartFailure::Setup(Error::Invalid(original))) => {
            assert_eq!(original, "CNA supports legal_random, pass_when_possible or human");
            original.as_ptr()
        }
        _ => panic!("actual guarded preparation failure was replaced"),
    };
    let record = owner.custody_record(key).unwrap();
    assert!(record.attempt_path().is_none());
    assert!(record.connection_custody().is_none());
    assert!(record.prepared().is_none());
    assert!(!owner.writers[0].can_publish());
    let cutoff = Instant::now() + Duration::from_secs(120);
    owner.join_creators_until(cutoff).await.unwrap();
    owner.join_creators_until(cutoff + Duration::from_secs(120)).await.unwrap();
    match owner.shared.replies[0].product.get().unwrap() {
        Err(WriterStartFailure::Setup(Error::Invalid(original))) => assert_eq!(original.as_ptr(), pointer),
        _ => panic!("repeat cleanup replaced raw failure"),
    }
}

#[tokio::test]
async fn guarded_recovery_without_custody_refuses_before_readonly_open() {
    let mut writer = crate::owned_writer::WriterJoin::<Result<(), Error>, 512>::new_prepaid();
    let path = PathBuf::from("no-readonly-or-writer-open.sqlite");
    let pointer = path.as_os_str().as_encoded_bytes().as_ptr();
    assert!(writer.record().retain_attempt_path(path).is_ok());
    writer.record().require_guarded_connection();
    let claimed = writer.vacant_standard().unwrap();
    let result = crate::campaigns::recover_guarded_owned(std::path::Path::new("unused-data"), claimed);
    assert!(matches!(result, Err(WriterStartFailure::Setup(Error::NotRunning))));
    assert!(!writer.can_publish());
    assert!(writer.record().prepared().is_none());
    assert_eq!(writer.record().attempt_path().unwrap().as_os_str().as_encoded_bytes().as_ptr(), pointer);
}

#[tokio::test]
async fn actual_custody_wait_timeout_then_late_creator_return_keeps_unknown_and_same_roots() {
    let mut owner = owner(PathBuf::new(), PathBuf::new());
    let key = owner.reserve_guarded_recovery(PathBuf::from("never-open-late.sqlite")).unwrap();
    owner.dispatch_guarded_pending().unwrap();
    reach_custody_gate(&mut owner).await; // Actual worker is blocked before first SQLite.
    let cutoff = Instant::now();
    assert!(matches!(owner.tasks[0].join_until(cutoff).await.unwrap(), JoinObservation::Unknown));
    assert!(owner.tasks[0].handle_for_poll().is_ok());
    owner.close(cutoff).unwrap();
    std::future::poll_fn(|cx| owner.poll_guarded_creators(cx)).await.unwrap();
    let pointer = match owner.tasks[0].observe() {
        JoinObservation::Returned { original: Ok(CreatorExit::Produced), was_unknown: true } =>
            owner.shared.replies[0].product.get().unwrap() as *const _,
        _ => panic!("late actual creator result cleared uncertainty"),
    };
    assert!(matches!(owner.observer().observe(key).unwrap(), CreationObservation::Unknown));
    owner.join_creators_until(cutoff + Duration::from_secs(120)).await.unwrap();
    assert_eq!(owner.tasks[0].deadline(), Some(cutoff));
    assert_eq!(owner.shared.replies[0].product.get().unwrap() as *const _, pointer);
    assert!(matches!(owner.tasks[0].observe(), JoinObservation::Returned { was_unknown: true, .. }));
}
