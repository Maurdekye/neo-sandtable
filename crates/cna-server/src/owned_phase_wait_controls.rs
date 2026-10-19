//! SOURCE controls: never executed by this packet. No fake positive custody.
use super::*;
use std::{future::{Future, poll_fn}, time::Duration};

fn owner<const N: usize>() -> CreationOwner<N, 512> {
    CreationOwner::new_prepaid(Handle::current(), uuid::Uuid::from_u128(917),
        PathBuf::from("never-open-phase-waits"), PathBuf::from("never-load-phase-data"))
}

fn hold_preparation<const N: usize, const R: usize>(owner: &CreationOwner<N, R>, key: CreatorKey) {
    assert!(owner.shared.replies[key.index].test_preparation_hold.set(FactoryGate::new()).is_ok());
}

async fn marker<const N: usize>(shared: &Shared<N>, index: usize, after_notify: bool) {
    loop {
        let marked = if after_notify { shared.replies[index].test_custody_notified.get().is_some() }
            else { shared.replies[index].test_preparation_reached.get().is_some() };
        if marked { return; }
        // Does not consume owner_changed's retained notification permit.
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn close_during_actual_preparation_wait_keeps_original_close_and_join() {
    let mut owner = owner::<1>();
    let key = owner.reserve_guarded_recovery(PathBuf::from("never-open-prep.sqlite")).unwrap();
    hold_preparation(&owner, key);
    let observer = owner.observer();
    let pending;
    let result;
    {
        let wait = owner.wait_guarded_preparation(key);
        tokio::pin!(wait);
        pending = poll_fn(|cx| Poll::Ready(wait.as_mut().poll(cx).is_pending())).await;
        observer.request_close().unwrap(); // Same clone main can retain outside Host borrow.
        result = wait.await;
    }
    let close = observer.close_result().unwrap() as *const _;
    let cutoff = *observer.close_started_at().unwrap() + Duration::from_secs(120);
    owner.join_creators_until(cutoff).await.unwrap();
    assert!(pending);
    assert_eq!(result, Err(PreparedAccessRefusal::Ingress(IngressRefusal::Closed)));
    assert_eq!(owner.close_result().unwrap() as *const _, close);
    assert!(matches!(owner.tasks[0].observe(), JoinObservation::Returned { original: Ok(CreatorExit::Produced), was_unknown: false }));
    assert!(matches!(owner.shared.replies[0].product.get(), Some(Err(WriterStartFailure::Setup(Error::NotRunning)))));
    assert!(owner.writers[0].record().connection_custody().is_none());
}

#[tokio::test]
async fn close_during_actual_writer_park_wait_keeps_both_original_roots() {
    let mut owner = owner::<1>();
    let key = owner.reserve_guarded_recovery(PathBuf::from("never-open-park.sqlite")).unwrap();
    hold_preparation(&owner, key);
    owner.dispatch_guarded_selected(key).unwrap();
    let observer = owner.observer();
    let pending;
    let result;
    {
        let wait = owner.wait_guarded_writer_parked(key);
        tokio::pin!(wait);
        pending = poll_fn(|cx| Poll::Ready(wait.as_mut().poll(cx).is_pending())).await;
        observer.request_close().unwrap();
        result = wait.await;
    }
    let cutoff = *observer.close_started_at().unwrap() + Duration::from_secs(120);
    owner.join_creators_until(cutoff).await.unwrap();
    assert!(pending);
    assert_eq!(result, Err(PreparedAccessRefusal::Ingress(IngressRefusal::Closed)));
    assert!(matches!(owner.tasks[0].observe(), JoinObservation::Returned { original: Ok(CreatorExit::Produced), was_unknown: false }));
    assert!(!owner.writers[0].can_publish()); // Original claim is not recycled.
    assert!(owner.writers[0].record().prepared().is_none());
    // This isolates the actual wait's CLOSE branch before a real Host fixture;
    // it does NOT claim a positive post-mint writer path was exercised.
}

async fn failure_in_wait(writer_wait: bool, panic_claim: bool) {
    let mut owner = owner::<1>();
    let request = CreateRequest { kind: crate::http::CampaignKind::Cna,
        rules_profile: cna_rules::PROFILE_FULL.into(), seed: [7; 32], title: "actual wait failure".into(),
        controller: "unsupported".into(), paused: false };
    let key = owner.observer().submit(CreationInput::Fresh(request)).unwrap();
    let publication = owner.writers[0].creator_publication();
    let first_claim = if panic_claim { Some(publication.claim().unwrap()) } else { None };
    let result = if writer_wait {
        owner.dispatch_guarded_selected(key).unwrap();
        owner.wait_guarded_writer_parked(key).await
    } else { owner.wait_guarded_preparation(key).await };
    let raw = if panic_claim {
        assert_eq!(result, Err(PreparedAccessRefusal::CreatorJoinFailed));
        match owner.tasks[0].observe() {
            JoinObservation::Returned { original: Err(cause), was_unknown: false } => {
                assert!(cause.is_panic()); cause as *const _ as usize
            }
            _ => panic!("actual creator JoinError not retained"),
        }
    } else {
        assert_eq!(result, Err(PreparedAccessRefusal::UnsuccessfulProduct));
        match owner.shared.replies[0].product.get().unwrap() {
            Err(WriterStartFailure::Setup(Error::Invalid(cause))) => {
                assert_eq!(cause, "CNA supports legal_random, pass_when_possible or human"); cause.as_ptr() as usize
            }
            _ => panic!("actual preparation error not retained"),
        }
    };
    let cutoff = Instant::now() + Duration::from_secs(120);
    owner.join_creators_until(cutoff).await.unwrap();
    if panic_claim {
        match owner.tasks[0].observe() {
            JoinObservation::Returned { original: Err(cause), was_unknown: false } => assert_eq!(cause as *const _ as usize, raw),
            _ => panic!("repeat cleanup replaced raw panic join"),
        }
    } else {
        match owner.shared.replies[0].product.get().unwrap() {
            Err(WriterStartFailure::Setup(Error::Invalid(cause))) => assert_eq!(cause.as_ptr() as usize, raw),
            _ => panic!("repeat cleanup replaced raw product error"),
        }
    }
    drop(first_claim);
}

#[tokio::test]
async fn preparation_wait_retains_real_product_failure_and_actual_join_error() {
    failure_in_wait(false, false).await;
    failure_in_wait(false, true).await;
}

#[tokio::test]
async fn writer_wait_retains_real_product_failure_and_actual_join_error() {
    failure_in_wait(true, false).await;
    failure_in_wait(true, true).await;
}

async fn unknown_in_wait(writer_wait: bool) {
    let mut owner = owner::<1>();
    let key = owner.reserve_guarded_recovery(PathBuf::from("never-open-unknown.sqlite")).unwrap();
    hold_preparation(&owner, key);
    owner.dispatch_guarded_selected(key).unwrap();
    let cutoff = Instant::now();
    assert!(matches!(owner.tasks[0].join_until(cutoff).await.unwrap(), JoinObservation::Unknown));
    let result = if writer_wait { owner.wait_guarded_writer_parked(key).await }
        else { owner.wait_guarded_preparation(key).await };
    assert_eq!(result, Err(PreparedAccessRefusal::UnknownCreator));
    assert!(owner.tasks[0].handle_for_poll().is_ok());
    owner.close(cutoff).unwrap();
    poll_fn(|cx| owner.poll_guarded_creators(cx)).await.unwrap();
    owner.join_creators_until(cutoff + Duration::from_secs(120)).await.unwrap();
    assert_eq!(owner.tasks[0].deadline(), Some(cutoff));
    assert!(matches!(owner.tasks[0].observe(), JoinObservation::Returned { was_unknown: true, .. }));
    assert!(matches!(owner.observer().observe(key).unwrap(), CreationObservation::Unknown));
}

#[tokio::test]
async fn preparation_wait_keeps_actual_timeout_unknown_after_late_return() { unknown_in_wait(false).await; }

#[tokio::test]
async fn writer_wait_keeps_actual_timeout_unknown_after_late_return() { unknown_in_wait(true).await; }

#[tokio::test]
async fn actual_custody_request_between_check_and_wait_leaves_a_permit() {
    let mut owner = owner::<1>();
    let key = owner.reserve_guarded_recovery(PathBuf::from("never-open-permit.sqlite")).unwrap();
    hold_preparation(&owner, key);
    owner.dispatch_guarded_selected(key).unwrap();
    marker(&owner.shared, 0, false).await;
    let observer = owner.observer();
    assert!(owner.shared.replies[0].custody_requested.get().is_none()); // Condition check.
    // Consume the older test-barrier notification so ONLY custody supplies the permit.
    observer.owner_changed().await;
    owner.shared.replies[0].test_preparation_hold.get().unwrap().decide(true);
    owner.shared.replies[0].test_preparation_hold.get().unwrap().wake();
    marker(&owner.shared, 0, true).await; // Actual custody notify_one has RETURNED.
    observer.owner_changed().await; // Registration happens AFTER the signal.
    assert!(owner.wait_guarded_preparation(key).await.is_ok());
    let cutoff = Instant::now() + Duration::from_secs(120);
    owner.join_creators_until(cutoff).await.unwrap();
    assert!(owner.writers[0].record().connection_custody().is_none());
}

#[tokio::test]
async fn actual_close_between_check_and_wait_leaves_a_permit() {
    let mut owner = owner::<1>();
    let key = owner.reserve_guarded_recovery(PathBuf::from("never-open-close-permit.sqlite")).unwrap();
    let observer = owner.observer();
    observer.owner_changed().await; // Drain reservation notification.
    assert!(owner.close_result().is_none()); // Condition check.
    observer.request_close().unwrap(); // Real close+notify BEFORE waiter registration.
    observer.owner_changed().await;
    assert_eq!(owner.wait_guarded_preparation(key).await,
        Err(PreparedAccessRefusal::Ingress(IngressRefusal::Closed)));
    assert_eq!(owner.wait_guarded_writer_parked(key).await,
        Err(PreparedAccessRefusal::UnsuccessfulProduct)); // Never dispatched, never a parked writer.
    owner.join_creators_until(*observer.close_started_at().unwrap() + Duration::from_secs(120)).await.unwrap();
    assert!(owner.tasks[0].is_unpublished());
}

#[tokio::test]
async fn selected_dispatch_does_not_start_second_queued_original() {
    let mut owner = owner::<2>();
    let first = owner.reserve_guarded_recovery(PathBuf::from("never-open-first.sqlite")).unwrap();
    let second_path = PathBuf::from("never-open-second.sqlite");
    let original = second_path.as_os_str().as_encoded_bytes().as_ptr();
    let second = owner.reserve_guarded_recovery(second_path).unwrap();
    owner.wait_guarded_preparation(first).await.unwrap();
    assert!(owner.started[0] && owner.guarded_started[0]);
    assert!(!owner.started[1] && !owner.guarded_started[1]);
    assert!(owner.tasks[1].is_unpublished() && owner.writers[1].can_publish());
    assert!(matches!(owner.shared.admission.lock().unwrap().inputs[1], Some(CreationInput::GuardedRecovery)));
    assert_eq!(owner.custody_record(second).unwrap().attempt_path().unwrap()
        .as_os_str().as_encoded_bytes().as_ptr(), original);
    owner.join_creators_until(Instant::now() + Duration::from_secs(120)).await.unwrap();
    assert!(owner.tasks[1].is_unpublished());
    assert!(matches!(owner.observer().observe(second).unwrap(), CreationObservation::CancelledBeforeEffects));
}
