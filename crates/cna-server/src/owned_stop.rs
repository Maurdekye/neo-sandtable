//! Held original writer stop request/reply. Child of actor for the SAME private
//! Envelope/queue/wake; no public protocol, new Op, task or legacy-call rewrite.
//! The outer owner creates this root before owned writer GO. It sends ONLY in
//! the final writer phase AFTER Host seat/child joins, keeping reserved ACK lanes
//! alive during the preceding Host phase. No live caller is wired yet.
//!
//! Full oneshot/queue waiter/future/Envelope/Value/Error/Sender/wake backing is
//! original F/G before creation, not a new cleanup admission or fit exemption.

use super::{CampaignHandle, Envelope, Op};
use crate::Error;
use serde_json::Value;
use tokio::{sync::{mpsc, oneshot}, time::{Instant, error::Elapsed}};

type OriginalReply = Result<Result<Value, Error>, oneshot::error::RecvError>;

pub(crate) struct OwnedStop {
    sender: mpsc::Sender<Envelope>,
    wake: crate::host_gate::WriterWake,
    envelope: Option<Envelope>,
    receiver: oneshot::Receiver<Result<Value, Error>>,
    queued: bool,
    queue_error: Option<mpsc::error::SendError<()>>,
    reply: Option<OriginalReply>,
    deadline: Option<Instant>,
    first_timeout: Option<Elapsed>,
}

pub(crate) enum StopObservation<'a> {
    Pending { queued: bool },
    Unknown { queued: bool, original: &'a Elapsed },
    QueueClosed {
        original: &'a mpsc::error::SendError<()>,
        prior_timeout: Option<&'a Elapsed>,
    },
    Reply { original: &'a OriginalReply, prior_timeout: Option<&'a Elapsed> },
}

impl OwnedStop {
    pub(crate) fn new_prepaid(handle: &CampaignHandle) -> Self {
        Self::for_mailbox_prepaid(handle.inner.tx.clone(), handle.inner.wake.clone())
    }

    fn for_mailbox_prepaid(sender: mpsc::Sender<Envelope>, wake: crate::host_gate::WriterWake) -> Self {
        let (reply, receiver) = oneshot::channel();
        Self {
            sender, wake, envelope: Some(Envelope { op: Op::Shutdown, reply }), receiver,
            queued: false, queue_error: None, reply: None, deadline: None, first_timeout: None,
        }
    }

    pub(crate) fn deadline(&self) -> Option<Instant> { self.deadline }

    pub(crate) fn observe(&self) -> StopObservation<'_> {
        if let Some(original) = self.reply.as_ref() {
            return StopObservation::Reply { original, prior_timeout: self.first_timeout.as_ref() };
        }
        if let Some(original) = self.queue_error.as_ref() {
            return StopObservation::QueueClosed { original, prior_timeout: self.first_timeout.as_ref() };
        }
        if let Some(original) = self.first_timeout.as_ref() {
            return StopObservation::Unknown { queued: self.queued, original };
        }
        StopObservation::Pending { queued: self.queued }
    }

    /// Borrow the actual root; never move the envelope into a cancellable send
    /// future or move the reply receiver into a timeout. A cancelled reserve may
    /// lose its queue position, but cannot lose or duplicate the original Op.
    /// Repeated observations cannot restart the original overall deadline.
    pub(crate) async fn settle_until(&mut self, requested: Instant) -> StopObservation<'_> {
        let deadline = self.deadline.map_or(requested, |old| old.min(requested));
        self.deadline = Some(deadline);
        if self.reply.is_some() || self.queue_error.is_some() { return self.observe(); }
        if !self.queued {
            // Once pre-send timeout is UNKNOWN, do not create a late new effect
            // merely because a later observer finds capacity. Original stays held.
            if self.first_timeout.is_some() { return self.observe(); }
            match tokio::time::timeout_at(deadline, self.sender.reserve()).await {
                Ok(Ok(permit)) => {
                    let original = self.envelope.take().expect("one original unsent stop envelope");
                    self.queued = true;
                    permit.send(original); // Synchronous once-only transfer.
                    crate::host_gate::wake(&self.wake);
                }
                Ok(Err(original)) => {
                    self.queue_error = Some(original); // First MOVE before report.
                    return self.observe();
                }
                Err(original) => {
                    self.first_timeout = Some(original);
                    return self.observe();
                }
            }
        }
        match tokio::time::timeout_at(deadline, &mut self.receiver).await {
            Ok(original) => self.reply = Some(original), // Entire raw result held.
            Err(original) => {
                if self.first_timeout.is_none() { self.first_timeout = Some(original); }
            }
        }
        self.observe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future::Future, sync::{Arc, OnceLock}, time::Duration};

    #[tokio::test]
    async fn cancelled_capacity_wait_keeps_original_envelope_then_sends_exactly_once() {
        let (sender, mut mailbox) = mpsc::channel(1);
        let (first_reply, first_wait) = oneshot::channel();
        sender.send(Envelope { op: Op::Pause(true), reply: first_reply }).await.ok().unwrap();
        let mut root = OwnedStop::for_mailbox_prepaid(sender, Arc::new(OnceLock::new()));
        let deadline = Instant::now() + Duration::from_secs(120);
        {
            let mut borrowing = std::pin::pin!(root.settle_until(deadline));
            std::future::poll_fn(|cx| {
                assert!(borrowing.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            }).await;
        }
        assert!(root.envelope.is_some());
        assert!(matches!(root.observe(), StopObservation::Pending { queued: false }));
        let first = mailbox.recv().await.unwrap();
        assert!(matches!(first.op, Op::Pause(true)));
        first.reply.send(Ok(Value::Null)).unwrap();
        assert!(matches!(first_wait.await, Ok(Ok(Value::Null))));
        let (observation, ()) = tokio::join!(
            root.settle_until(deadline + Duration::from_secs(120)),
            async {
                let actual = mailbox.recv().await.unwrap();
                assert!(matches!(actual.op, Op::Shutdown));
                actual.reply.send(Ok(Value::Null)).unwrap();
            },
        );
        assert!(matches!(observation, StopObservation::Reply {
            original: Ok(Ok(Value::Null)), prior_timeout: None,
        }));
        assert_eq!(root.deadline(), Some(deadline));
        assert!(root.envelope.is_none());
        assert!(mailbox.try_recv().is_err());
    }

    #[tokio::test]
    async fn closed_queue_preserves_original_send_error_and_unsent_envelope() {
        let (sender, mailbox) = mpsc::channel(1);
        let mut root = OwnedStop::for_mailbox_prepaid(sender, Arc::new(OnceLock::new()));
        drop(mailbox);
        let deadline = Instant::now() + Duration::from_secs(120);
        let address = match root.settle_until(deadline).await {
            StopObservation::QueueClosed { original, prior_timeout: None } => original as *const _,
            _ => panic!("actual reserve error was not retained"),
        };
        assert!(root.envelope.is_some());
        match root.settle_until(deadline + Duration::from_secs(120)).await {
            StopObservation::QueueClosed { original, prior_timeout: None } => assert_eq!(original as *const _, address),
            _ => panic!("repeat observation replaced original queue failure"),
        }
    }

    #[tokio::test]
    async fn pre_send_unknown_keeps_request_and_cannot_reopen_late_send() {
        let (sender, mut mailbox) = mpsc::channel(1);
        let (reply, wait) = oneshot::channel();
        sender.send(Envelope { op: Op::Pause(true), reply }).await.ok().unwrap();
        let mut root = OwnedStop::for_mailbox_prepaid(sender, Arc::new(OnceLock::new()));
        let expired = Instant::now() - Duration::from_secs(1);
        let address = match root.settle_until(expired).await {
            StopObservation::Unknown { queued: false, original } => original as *const _,
            _ => panic!("blocked original request did not retain UNKNOWN"),
        };
        let earlier = mailbox.recv().await.unwrap();
        earlier.reply.send(Ok(Value::Null)).unwrap();
        assert!(matches!(wait.await, Ok(Ok(Value::Null))));
        match root.settle_until(Instant::now() + Duration::from_secs(120)).await {
            StopObservation::Unknown { queued: false, original } => assert_eq!(original as *const _, address),
            _ => panic!("late capacity reopened an UNKNOWN request"),
        }
        assert!(root.envelope.is_some());
        assert_eq!(root.deadline(), Some(expired));
        assert!(mailbox.try_recv().is_err());
    }
}
