//! Source-only controls for the actual OriginalService settlement branch.
use super::*;
use crate::owned_ingress::CreationOwner;
use std::{future::Future, path::PathBuf, task::Poll, time::Duration};
use tokio::{runtime::Handle, sync::oneshot, time::Instant};

#[tokio::test]
async fn cancelled_settlement_borrow_preserves_pin_deadline_and_raw_output() {
    let creation = CreationOwner::<1, 512>::new_prepaid(
        Handle::current(), uuid::Uuid::from_u128(90), PathBuf::new(), PathBuf::new(),
    );
    let owner = OwnedAppOwner::new_prepaid(creation, 0);
    let (sender, receiver) = oneshot::channel::<Result<(), String>>();
    let serving = async { receiver.await.expect("original serving sender remains held") };
    tokio::pin!(serving);
    let mut original = OriginalService::bind_prepaid(owner, serving.as_mut());
    let pin_address = original.serving.as_ref().get_ref() as *const _;
    let cutoff = Instant::now() + Duration::from_secs(120);
    {
        let mut borrowed = Box::pin(original.settle_serving_until(cutoff));
        std::future::poll_fn(|cx| {
            assert!(borrowed.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        }).await;
        drop(borrowed); // No spawned task; only this borrowing future is canceled.
    }
    assert_eq!(original.serving.as_ref().get_ref() as *const _, pin_address);
    assert_eq!(original.serving_deadline, Some(cutoff));
    assert!(original.returned.observe().is_none());
    assert!(original.serving_timeout().is_none());
    let close_address = original.observe().close_started_at.unwrap() as *const _;
    let cause = String::from("canonical serving failure after canceled borrower");
    let payload_address = cause.as_ptr();
    sender.send(Err(cause)).unwrap();
    original.settle_serving_until(cutoff + Duration::from_secs(120)).await;
    assert_eq!(original.serving_deadline, Some(cutoff));
    assert_eq!(original.observe().close_started_at.unwrap() as *const _, close_address);
    match original.returned.observe().unwrap() {
        Err(raw) => {
            assert_eq!(raw, "canonical serving failure after canceled borrower");
            assert_eq!(raw.as_ptr(), payload_address);
        }
        _ => panic!("original serving error was replaced"),
    }
}

#[tokio::test]
async fn real_serving_timeout_is_first_held_and_cutoff_never_extends() {
    let creation = CreationOwner::<1, 512>::new_prepaid(
        Handle::current(), uuid::Uuid::from_u128(91), PathBuf::new(), PathBuf::new(),
    );
    let owner = OwnedAppOwner::new_prepaid(creation, 0);
    let serving = std::future::pending::<Result<(), String>>();
    tokio::pin!(serving);
    let mut original = OriginalService::bind_prepaid(owner, serving.as_mut());
    let pin_address = original.serving.as_ref().get_ref() as *const _;
    let cutoff = Instant::now() - Duration::from_secs(1);
    original.settle_serving_until(cutoff).await;
    let first = original.serving_timeout().unwrap() as *const _;
    let close_address = original.observe().close_started_at.unwrap() as *const _;
    original.settle_serving_until(cutoff + Duration::from_secs(240)).await;
    assert_eq!(original.serving_deadline, Some(cutoff));
    assert_eq!(original.serving_timeout().unwrap() as *const _, first);
    assert_eq!(original.serving.as_ref().get_ref() as *const _, pin_address);
    assert_eq!(original.observe().close_started_at.unwrap() as *const _, close_address);
    assert!(original.returned.observe().is_none());
}

#[tokio::test]
async fn late_serving_output_keeps_first_elapsed_and_same_allocation() {
    let creation = CreationOwner::<1, 512>::new_prepaid(
        Handle::current(), uuid::Uuid::from_u128(92), PathBuf::new(), PathBuf::new(),
    );
    let owner = OwnedAppOwner::new_prepaid(creation, 0);
    let (sender, receiver) = oneshot::channel::<Result<(), String>>();
    let serving = async { receiver.await.expect("held original sender") };
    tokio::pin!(serving);
    let mut original = OriginalService::bind_prepaid(owner, serving.as_mut());
    let cutoff = Instant::now() - Duration::from_secs(1);
    original.settle_serving_until(cutoff).await;
    let first = original.serving_timeout().unwrap() as *const _;
    let cause = String::from("original late serving output");
    let payload_address = cause.as_ptr();
    sender.send(Err(cause)).unwrap();
    // Tokio polls the ready original before rejecting an elapsed cutoff.
    original.settle_serving_until(cutoff + Duration::from_secs(240)).await;
    let returned_address = original.returned.observe().unwrap() as *const _;
    match original.returned.observe().unwrap() {
        Err(raw) => { assert_eq!(raw.as_ptr(), payload_address); assert_eq!(raw, "original late serving output"); }
        _ => panic!("late original output was normalized"),
    }
    assert_eq!(original.serving_timeout().unwrap() as *const _, first);
    assert_eq!(original.serving_deadline, Some(cutoff));
    original.settle_serving_until(cutoff + Duration::from_secs(480)).await;
    assert_eq!(original.returned.observe().unwrap() as *const _, returned_address);
    assert_eq!(original.serving_timeout().unwrap() as *const _, first);
}
