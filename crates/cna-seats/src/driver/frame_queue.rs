//! One canonical stdout FIFO. No archive mirror, body clone or dequeue/requeue.
//! After original joins and sender destruction, unique mutable ownership exposes
//! the queue by borrow without holding a synchronization lock during encoding/I/O.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use super::ChildFrame;
use crate::operation::{Operation, OperationError};

/// Actual empty owned endpoints staged BEFORE Operation creation. No send,
/// task, node allocation or live production selection is provided here. The
/// private one-shot binding is reachable only after canonical original creation;
/// consuming endpoint handoff remains disconnected from the child launcher.
#[allow(dead_code)]
pub(super) struct PendingOwnedChannel {
    sender: OwnedSender,
    receiver: OwnedReceiver,
    bind_started: bool,
    handoff_started: bool,
    receive_future: std::alloc::Layout,
    receive_output: std::alloc::Layout,
}

pub(super) struct OwnedSender(Arc<OwnedShared>);
pub(super) struct OwnedReceiver(Arc<OwnedShared>);

struct OwnedShared {
    state: Mutex<OwnedState>,
    ready: Notify,
}

struct OwnedState {
    frames: owned::Frames,
    original: Option<Operation>,
    sender_closed: bool,
    receiver_closed: bool,
}

/// Partial fixed facts for THIS retained empty owner, not a complete original F.
/// Endpoint headers occur inside owner_inline; waiter_inline belongs to the
/// actual future inventory and is NOT added to referenced_bytes here.
#[allow(dead_code)]
pub(super) struct OwnedCreationLayouts {
    pub(super) owner_inline: std::alloc::Layout,
    pub(super) sender_inline: std::alloc::Layout,
    pub(super) receiver_inline: std::alloc::Layout,
    pub(super) shared_body: std::alloc::Layout,
    pub(super) mutex_inline: std::alloc::Layout,
    pub(super) notify_inline: std::alloc::Layout,
    pub(super) waiter_inline: std::alloc::Layout,
    pub(super) receive_future: std::alloc::Layout,
    pub(super) receive_output: std::alloc::Layout,
    pub(super) receive_payload_bytes: usize,
    pub(super) arc_counter_bytes: usize,
    pub(super) referenced_bytes: usize,
}

#[allow(dead_code)]
impl PendingOwnedChannel {
    pub(super) fn new() -> Self {
        let shared = Arc::new(OwnedShared {
            state: Mutex::new(OwnedState {
                frames: owned::Frames::default(),
                original: None,
                sender_closed: false,
                receiver_closed: false,
            }),
            ready: Notify::new(),
        });
        let sender = OwnedSender(shared.clone());
        let mut receiver = OwnedReceiver(shared);
        let (receive_future, receive_output) = {
            // Construct the actual concrete future BEFORE Operation exists. No
            // polling/registration/queue access occurs; the borrow is dropped.
            let future = receiver.recv();
            future_layouts(&future)
        };
        Self {
            sender,
            receiver,
            bind_started: false,
            handoff_started: false,
            receive_future,
            receive_output,
        }
    }

    pub(super) fn creation_layout(
        &self,
    ) -> Result<OwnedCreationLayouts, crate::failed_evidence::ArchiveError> {
        if self.bind_started || self.handoff_started {
            return Err(crate::failed_evidence::ArchiveError::Identity);
        }
        self.pristine_layout()
    }

    fn pristine_layout(
        &self,
    ) -> Result<OwnedCreationLayouts, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        use std::alloc::Layout;
        if !Arc::ptr_eq(&self.sender.0, &self.receiver.0)
            || Arc::strong_count(&self.sender.0) != 2
            || Arc::weak_count(&self.sender.0) != 0
        {
            return Err(ArchiveError::Identity);
        }
        let state = self
            .sender
            .0
            .state
            .try_lock()
            .map_err(|_| ArchiveError::Identity)?;
        if state.original.is_some()
            || state.sender_closed
            || state.receiver_closed
            || !state.frames.pristine()
        {
            return Err(ArchiveError::Identity);
        }
        let shared_body = Layout::for_value(self.sender.0.as_ref());
        // Existing explicit strong/weak counter policy, not a private Arc layout
        // mirror or RSS bound. Mutex/Notify/Frames are INSIDE actual shared_body.
        let arc_counter_bytes = 2usize
            .checked_mul(std::mem::size_of::<usize>())
            .ok_or(ArchiveError::Fit)?;
        let referenced_bytes = shared_body
            .size()
            .checked_add(arc_counter_bytes)
            .ok_or(ArchiveError::Fit)?;
        let receive_payload_bytes = self
            .receive_future
            .size()
            .checked_add(self.receive_output.size())
            .ok_or(ArchiveError::Fit)?;
        Ok(OwnedCreationLayouts {
            owner_inline: Layout::for_value(self),
            sender_inline: Layout::new::<OwnedSender>(),
            receiver_inline: Layout::new::<OwnedReceiver>(),
            shared_body,
            mutex_inline: Layout::for_value(&self.sender.0.state),
            notify_inline: Layout::for_value(&self.sender.0.ready),
            waiter_inline: Layout::new::<tokio::sync::futures::Notified<'static>>(),
            receive_future: self.receive_future,
            receive_output: self.receive_output,
            receive_payload_bytes,
            arc_counter_bytes,
            referenced_bytes,
        })
    }

    /// The private caller supplies ONLY its actual canonical creation return.
    /// No external Operation ingress or live endpoint handoff is added here.
    pub(super) fn bind_original(
        &mut self,
        original: &Operation,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        if self.bind_started || self.handoff_started {
            return Err(ArchiveError::Identity);
        }
        self.bind_started = true; // Failed attempts cannot reopen, even if still empty.
        self.pristine_layout()?;
        original.check()?;
        {
            let mut state = self
                .sender
                .0
                .state
                .try_lock()
                .map_err(|_| ArchiveError::Identity)?;
            if state.original.is_some()
                || state.sender_closed
                || state.receiver_closed
                || !state.frames.pristine()
            {
                return Err(ArchiveError::Identity);
            }
            state.original = Some(original.clone()); // SAME handles, no new State/body.
        } // No State/queue lock during the postcheck, no wake/callback anywhere.
        original.check()?;
        Ok(())
    }

    fn check_bound_endpoints(
        &self,
        expected: &Operation,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        if !self.bind_started
            || !Arc::ptr_eq(&self.sender.0, &self.receiver.0)
            || Arc::strong_count(&self.sender.0) != 2
            || Arc::weak_count(&self.sender.0) != 0
        {
            return Err(ArchiveError::Identity);
        }
        {
            let state = self
                .sender
                .0
                .state
                .try_lock()
                .map_err(|_| ArchiveError::Identity)?;
            if !state
                .original
                .as_ref()
                .is_some_and(|original| original.same_instance(expected))
                || state.sender_closed
                || state.receiver_closed
                || !state.frames.pristine()
            {
                return Err(ArchiveError::Identity);
            }
        }
        expected.check()?; // No queue lock during the allowance check.
        Ok(())
    }

    /// Future trusted assembly must install BOTH returned endpoints in its SAME
    /// canonical child/task owners BEFORE spawn or any fallible effect. Refusal
    /// returns the WHOLE pending value; no projection may drop it or retry it.
    /// No caller currently connects this method to a physical child.
    pub(super) fn into_endpoints(
        mut self,
        expected: &Operation,
    ) -> Result<(OwnedSender, OwnedReceiver), (crate::failed_evidence::ArchiveError, Self)> {
        use crate::failed_evidence::ArchiveError;
        if self.handoff_started {
            return Err((ArchiveError::Identity, self));
        }
        self.handoff_started = true;
        if let Err(error) = self.check_bound_endpoints(expected) {
            return Err((error, self));
        }
        // Last fallible check ABOVE the complete MOVE. No allocation, notify,
        // callback, byte debit or independent endpoint copy follows it.
        let Self {
            sender, receiver, ..
        } = self;
        Ok((sender, receiver))
    }
}

fn future_layouts<F: std::future::Future>(future: &F) -> (std::alloc::Layout, std::alloc::Layout) {
    (
        std::alloc::Layout::for_value(future),
        std::alloc::Layout::new::<F::Output>(),
    )
}

#[allow(dead_code)] // Unactivated owned receive; physical child selection remains held.
impl OwnedReceiver {
    /// Storage-only archive bridge, NOT evidence of physical child/reader joins.
    /// The trusted ChildProc must establish those joins separately before using
    /// this helper. Sender destruction alone proves only channel closure.
    /// Unique access avoids a held synchronization guard during sink callbacks;
    /// every row is preflighted before any callback, with no pop or release.
    pub(super) fn visit_unique_storage(
        &mut self,
        original: &Operation,
        physical_child: &str,
        sink: &mut impl super::archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        let shared = Arc::get_mut(&mut self.0).ok_or(ArchiveError::Identity)?;
        let state = shared.state.get_mut().map_err(|_| ArchiveError::Identity)?;
        if !state.sender_closed
            || state.receiver_closed
            || !state
                .original
                .as_ref()
                .is_some_and(|bound| bound.same_instance(original))
        {
            return Err(ArchiveError::Identity);
        }
        // No live allowance check: SAME failed/expired evidence stays readable.
        // Normalization only relinks existing Boxes under exclusive access.
        state
            .frames
            .visit_normalized(original, physical_child, sink)
    }

    /// Transfer already-admitted evidence into the caller's actual pending slot.
    /// The caller must retain this output before any next receive/callback. This
    /// is no fresh byte admission, settlement/physical join or supervision cause.
    /// Failed/expired operations may still transfer their SAME retained evidence.
    pub(super) async fn recv(
        &mut self,
    ) -> Result<Option<ChildFrame>, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        loop {
            let notified = self.0.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable(); // Register BEFORE observing queue/closure.
            {
                let mut state = self.0.state.lock().map_err(|_| ArchiveError::Identity)?;
                let OwnedState {
                    frames,
                    original,
                    sender_closed,
                    receiver_closed,
                } = &mut *state;
                let original = original.as_ref().ok_or(ArchiveError::Identity)?;
                if *receiver_closed {
                    return Err(ArchiveError::Identity);
                }
                if let Some(frame) = frames.front() {
                    if !frame.operation.same_instance(original) {
                        return Err(ArchiveError::Identity); // Never pop a foreign body.
                    }
                    return Ok(frames.pop());
                }
                if *sender_closed {
                    return Ok(None); // Channel EOF ONLY, never a child/reader join.
                }
            } // No synchronous guard survives the await or a caller callback.
            notified.await;
        }
    }
}

impl Drop for OwnedSender {
    fn drop(&mut self) {
        let notify = match self.0.state.lock() {
            Ok(mut state) => {
                let notify = state.original.is_some() && !state.sender_closed;
                state.sender_closed = true;
                notify
            }
            // Poison is unknown closure/custody, never repaired or called EOF.
            Err(_) => false,
        }; // Guard gone BEFORE any registered waker can run.
        if notify {
            self.0.ready.notify_waiters();
        }
    }
}

impl Drop for OwnedReceiver {
    fn drop(&mut self) {
        let notify = match self.0.state.lock() {
            Ok(mut state) => {
                let notify = state.original.is_some() && !state.receiver_closed;
                state.receiver_closed = true;
                notify
            }
            Err(_) => false,
        }; // Never pop/clear evidence or refund credit on consumer destruction.
        if notify {
            self.0.ready.notify_waiters();
        }
    }
}

#[cfg(test)]
mod pending_owned_source_controls {
    // UNCOMPILED/UNRUN: no task, driver, worker, file or store. Legacy Operation
    // values below are identity/arithmetic fixtures, NEVER original Fit proof.
    use super::*;
    use crate::failed_evidence::ArchiveError;
    use std::alloc::Layout;

    #[test]
    fn fixed_owner_uses_actual_shared_body_without_adding_embedded_headers_twice() {
        let pending = PendingOwnedChannel::new();
        let layout = pending.creation_layout().unwrap();
        assert_eq!(layout.owner_inline, Layout::for_value(&pending));
        assert_eq!(
            layout.shared_body,
            Layout::for_value(pending.sender.0.as_ref())
        );
        assert_eq!(layout.sender_inline, Layout::for_value(&pending.sender));
        assert_eq!(layout.receiver_inline, Layout::for_value(&pending.receiver));
        assert_eq!(
            layout.mutex_inline,
            Layout::for_value(&pending.sender.0.state)
        );
        assert_eq!(
            layout.notify_inline,
            Layout::for_value(&pending.sender.0.ready)
        );
        assert_eq!(layout.arc_counter_bytes, 2 * std::mem::size_of::<usize>());
        assert_eq!(
            layout.referenced_bytes,
            layout.shared_body.size() + layout.arc_counter_bytes
        );
        assert!(Arc::ptr_eq(&pending.sender.0, &pending.receiver.0));
        assert_eq!(Arc::strong_count(&pending.sender.0), 2);
        let state = pending.sender.0.state.lock().unwrap();
        assert!(state.original.is_none() && state.frames.pristine());
        assert!(!state.sender_closed && !state.receiver_closed);
    }

    #[test]
    fn busy_shared_weak_foreign_and_closed_owners_refuse_without_repair() {
        let pending = PendingOwnedChannel::new();
        let locked = pending.sender.0.state.lock().unwrap();
        assert!(matches!(
            pending.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        assert!(locked.frames.pristine() && locked.original.is_none());
        drop(locked);
        let extra = pending.sender.0.clone();
        assert!(matches!(
            pending.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        drop(extra);
        let weak = Arc::downgrade(&pending.sender.0);
        assert!(matches!(
            pending.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        drop(weak);
        // Independent fixtures, not a retry or repair of a saved first result.
        let mut foreign = PendingOwnedChannel::new();
        foreign.receiver = OwnedReceiver(pending.sender.0.clone());
        assert!(matches!(
            foreign.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        let closed = PendingOwnedChannel::new();
        closed.sender.0.state.lock().unwrap().sender_closed = true;
        assert!(matches!(
            closed.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        assert!(closed.sender.0.state.lock().unwrap().sender_closed);
    }

    fn fixture_operation() -> Operation {
        // Legacy arithmetic/identity fixture only, NEVER a positive original Fit.
        Operation::new(
            "pending-fifo-binding-fixture".into(),
            7,
            crate::operation::Allowance::required(Some(
                tokio::time::Instant::now() + std::time::Duration::from_secs(60),
            ))
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn binding_retains_same_instance_and_never_rebinds_after_success_or_refusal() {
        let mut pending = PendingOwnedChannel::new(); // Actual cells BEFORE fixture Op.
        let original = fixture_operation();
        let foreign = fixture_operation(); // SAME text/epoch, different State.
        let before = original.captured_bytes();
        pending.bind_original(&original).unwrap();
        assert!(pending.bind_started);
        assert!(matches!(
            pending.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        assert_eq!(pending.bind_original(&foreign), Err(ArchiveError::Identity));
        let state = pending.sender.0.state.lock().unwrap();
        assert!(state.original.as_ref().unwrap().same_instance(&original));
        assert!(!state.original.as_ref().unwrap().same_instance(&foreign));
        assert!(state.frames.pristine());
        assert_eq!(original.captured_bytes(), before);
        drop(state);

        let mut refused = PendingOwnedChannel::new();
        let extra_owner = refused.sender.0.clone();
        assert_eq!(
            refused.bind_original(&original),
            Err(ArchiveError::Identity)
        );
        drop(extra_owner);
        assert!(refused.bind_started);
        assert_eq!(refused.bind_original(&foreign), Err(ArchiveError::Identity));
        assert!(refused.sender.0.state.lock().unwrap().original.is_none());
        assert_eq!(original.captured_bytes(), before);
    }

    #[test]
    fn consuming_handoff_retains_whole_owner_on_foreign_or_unbound_refusal() {
        let pending = PendingOwnedChannel::new();
        let original = fixture_operation();
        let (error, mut refused) = match pending.into_endpoints(&original) {
            Err(refused) => refused,
            Ok(_) => panic!("unbound handoff"),
        };
        assert_eq!(error, ArchiveError::Identity);
        assert!(refused.handoff_started);
        assert_eq!(
            refused.bind_original(&original),
            Err(ArchiveError::Identity)
        );
        assert!(refused.sender.0.state.lock().unwrap().original.is_none());

        let mut pending = PendingOwnedChannel::new();
        pending.bind_original(&original).unwrap();
        let pointer = Arc::as_ptr(&pending.sender.0);
        let foreign = fixture_operation();
        let (error, refused) = match pending.into_endpoints(&foreign) {
            Err(refused) => refused,
            Ok(_) => panic!("foreign handoff"),
        };
        assert_eq!(error, ArchiveError::Identity);
        assert_eq!(Arc::as_ptr(&refused.sender.0), pointer);
        assert!(Arc::ptr_eq(&refused.sender.0, &refused.receiver.0));
        assert!(
            refused
                .sender
                .0
                .state
                .lock()
                .unwrap()
                .original
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        assert!(refused.into_endpoints(&original).is_err()); // No retry after first refusal.

        let mut pending = PendingOwnedChannel::new();
        pending.bind_original(&original).unwrap();
        let pointer = Arc::as_ptr(&pending.sender.0);
        let before = original.captured_bytes();
        let (sender, receiver) = match pending.into_endpoints(&original) {
            Ok(pair) => pair,
            Err(_) => panic!("bound fixture handoff"),
        };
        assert_eq!(Arc::as_ptr(&sender.0), pointer);
        assert_eq!(Arc::as_ptr(&receiver.0), pointer);
        assert_eq!(Arc::strong_count(&sender.0), 2);
        assert_eq!(original.captured_bytes(), before);
    }

    #[test]
    fn bound_endpoint_drop_wakes_only_after_unlock_and_preserves_queued_body() {
        use std::future::Future;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::task::{Context, Poll, Wake, Waker};
        struct ClosedWake {
            shared: Arc<OwnedShared>,
            receiver_closed: bool,
            calls: AtomicUsize,
        }
        impl Wake for ClosedWake {
            fn wake(self: Arc<Self>) {
                self.wake_by_ref();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                let state = match self.shared.state.try_lock() {
                    Ok(state) => state,
                    Err(_) => panic!("endpoint notification under queue lock"),
                };
                assert!(state.original.is_some());
                assert_eq!(state.receiver_closed, self.receiver_closed);
                assert_eq!(state.sender_closed, !self.receiver_closed);
                assert!(!state.frames.pristine());
                self.calls.fetch_add(1, Ordering::SeqCst);
            }
        }
        for receiver_closed in [false, true] {
            let mut pending = PendingOwnedChannel::new();
            let original = fixture_operation();
            pending.bind_original(&original).unwrap();
            let (sender, receiver) = match pending.into_endpoints(&original) {
                Ok(pair) => pair,
                Err(_) => panic!("fixture handoff"),
            };
            let mut sender = Some(sender);
            let mut receiver = Some(receiver);
            let shared = &receiver.as_ref().unwrap().0;
            let text = String::from("queued-original-closure-evidence");
            let pointer = text.as_ptr();
            // Storage-only fixture, no child/body F/G admission claim. No
            // production allocator/debit or Sender::send exists for this owner.
            shared.state.lock().unwrap().frames = owned::Frames::single_frame_fixture(ChildFrame {
                text,
                origin: None,
                operation: original.clone(),
            });
            let observer = Arc::new(ClosedWake {
                shared: shared.clone(),
                receiver_closed,
                calls: AtomicUsize::new(0),
            });
            let waker = Waker::from(observer.clone());
            let mut cx = Context::from_waker(&waker);
            let before = original.captured_bytes();
            {
                let notified = observer.shared.ready.notified();
                let mut notified = std::pin::pin!(notified);
                notified.as_mut().enable();
                assert!(matches!(notified.as_mut().poll(&mut cx), Poll::Pending));
                if receiver_closed {
                    drop(receiver.take());
                } else {
                    drop(sender.take());
                }
                assert_eq!(observer.calls.load(Ordering::SeqCst), 1);
                assert!(matches!(notified.as_mut().poll(&mut cx), Poll::Ready(())));
            }
            let mut state = observer.shared.state.lock().unwrap();
            let frame = state.frames.pop().unwrap();
            assert_eq!(frame.text.as_ptr(), pointer);
            assert_eq!(frame.text, "queued-original-closure-evidence");
            assert!(frame.operation.same_instance(&original));
            assert!(state.frames.pristine());
            assert_eq!(original.captured_bytes(), before);
        }
    }

    #[test]
    fn receive_layout_is_the_actual_unpolled_future_and_output_before_operation() {
        let mut pending = PendingOwnedChannel::new();
        let layout = pending.creation_layout().unwrap();
        {
            let future = pending.receiver.recv();
            let (actual_future, actual_output) = future_layouts(&future);
            assert_eq!(layout.receive_future, actual_future);
            assert_eq!(layout.receive_output, actual_output);
            assert_eq!(
                actual_output,
                Layout::new::<Result<Option<ChildFrame>, ArchiveError>>()
            );
            assert_eq!(
                layout.receive_payload_bytes,
                actual_future.size() + actual_output.size()
            );
            // Drop without polling: no waiter is registered and no queue access.
        }
        assert!(!pending.bind_started && !pending.handoff_started);
        let state = pending.sender.0.state.lock().unwrap();
        assert!(state.original.is_none() && state.frames.pristine());
        assert!(!state.sender_closed && !state.receiver_closed);
    }

    #[test]
    fn receive_cancellation_and_registered_arrival_preserve_fifo_then_observe_eof_without_refund() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};
        let mut pending = PendingOwnedChannel::new();
        let original = fixture_operation();
        pending.bind_original(&original).unwrap();
        let (sender, mut receiver) = match pending.into_endpoints(&original) {
            Ok(pair) => pair,
            Err(_) => panic!("fixture handoff"),
        };
        let mut cx = Context::from_waker(Waker::noop());
        {
            let mut cancelled = std::pin::pin!(receiver.recv());
            assert!(matches!(cancelled.as_mut().poll(&mut cx), Poll::Pending));
        }
        assert!(sender.0.state.lock().unwrap().frames.pristine());
        let before = original.captured_bytes();
        for (text, registered) in [("after cancellation", false), ("after registration", true)] {
            let frame = ChildFrame {
                text: text.into(),
                origin: None,
                operation: original.clone(),
            };
            let pointer = frame.text.as_ptr();
            let mut receive = std::pin::pin!(receiver.recv());
            if registered {
                assert!(matches!(receive.as_mut().poll(&mut cx), Poll::Pending));
            }
            // Storage fixture only: the production Sender/debit is NOT wired.
            sender.0.state.lock().unwrap().frames = owned::Frames::single_frame_fixture(frame);
            sender.0.ready.notify_one(); // Outside queue guard, no overflow publication.
            match receive.as_mut().poll(&mut cx) {
                Poll::Ready(Ok(Some(frame))) => {
                    assert_eq!(frame.text, text);
                    assert_eq!(frame.text.as_ptr(), pointer);
                    assert!(frame.operation.same_instance(&original));
                }
                _ => panic!("queued frame must transfer exactly once"),
            }
        }
        // Existing failed evidence still drains; no new bytes or allowance.
        sender.0.state.lock().unwrap().frames = owned::Frames::single_frame_fixture(ChildFrame {
            text: "already admitted before refusal".into(),
            origin: None,
            operation: original.clone(),
        });
        original.overflow();
        drop(sender);
        {
            let mut receive = std::pin::pin!(receiver.recv());
            match receive.as_mut().poll(&mut cx) {
                Poll::Ready(Ok(Some(frame))) => {
                    assert_eq!(frame.text, "already admitted before refusal")
                }
                _ => panic!("retained frame precedes channel EOF"),
            }
        }
        let mut eof = std::pin::pin!(receiver.recv());
        assert!(matches!(eof.as_mut().poll(&mut cx), Poll::Ready(Ok(None))));
        assert_eq!(original.captured_bytes(), before);
        assert_eq!(original.check(), Err(OperationError::CaptureOverflow));
    }

    #[test]
    fn receive_refuses_unbound_foreign_and_poisoned_storage_without_healthy_eof() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};
        let mut cx = Context::from_waker(Waker::noop());
        let mut unbound = PendingOwnedChannel::new();
        {
            let mut receive = std::pin::pin!(unbound.receiver.recv());
            assert!(matches!(
                receive.as_mut().poll(&mut cx),
                Poll::Ready(Err(ArchiveError::Identity))
            ));
        }
        assert!(unbound.sender.0.state.lock().unwrap().original.is_none());
        let original = fixture_operation();
        let foreign = fixture_operation();
        let mut pending = PendingOwnedChannel::new();
        pending.bind_original(&original).unwrap();
        let (sender, mut receiver) = match pending.into_endpoints(&original) {
            Ok(pair) => pair,
            Err(_) => panic!("fixture handoff"),
        };
        let frame = ChildFrame {
            text: "foreign remains whole".into(),
            origin: None,
            operation: foreign.clone(),
        };
        let pointer = frame.text.as_ptr();
        sender.0.state.lock().unwrap().frames = owned::Frames::single_frame_fixture(frame);
        {
            let mut receive = std::pin::pin!(receiver.recv());
            assert!(matches!(
                receive.as_mut().poll(&mut cx),
                Poll::Ready(Err(ArchiveError::Identity))
            ));
        }
        {
            let mut state = sender.0.state.lock().unwrap();
            assert!(!state.frames.pristine());
            assert_eq!(state.frames.front().unwrap().text.as_ptr(), pointer);
            assert!(
                state
                    .frames
                    .front()
                    .unwrap()
                    .operation
                    .same_instance(&foreign)
            );
        }
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = sender.0.state.lock().unwrap();
            panic!("synthetic owned FIFO poison");
        }));
        assert!(poisoned.is_err());
        let mut receive = std::pin::pin!(receiver.recv());
        assert!(matches!(
            receive.as_mut().poll(&mut cx),
            Poll::Ready(Err(ArchiveError::Identity))
        ));
        assert!(sender.0.state.is_poisoned());
    }
}

#[cfg(test)]
mod unique_archive_source_controls {
    // UNCOMPILED/UNRUN. Unmetered storage and legacy Operation fixtures only;
    // no child/task joins, physical provenance or complete original Fit proof.
    use super::*;
    use crate::failed_evidence::{ArchiveError, EvidenceCategory, EvidenceRecord};

    struct Sink {
        calls: usize,
        pointer: *const u8,
        fail: bool,
    }

    impl super::super::archive_stdout::RecordSink for Sink {
        fn bytes(&mut self, record: EvidenceRecord<'_>, bytes: &[u8]) -> Result<(), ArchiveError> {
            assert!(matches!(record.category, EvidenceCategory::Stdout));
            assert_eq!(record.physical_child, Some("fixture-child"));
            assert_eq!(record.part, "queued_frame");
            assert_eq!(record.index, 0);
            assert_eq!(bytes, b"retained archive body");
            assert_eq!(bytes.as_ptr(), self.pointer);
            self.calls += 1;
            if self.fail {
                Err(ArchiveError::Fit)
            } else {
                Ok(())
            }
        }

        fn json<T: serde::Serialize + ?Sized>(
            &mut self,
            _: EvidenceRecord<'_>,
            _: &T,
        ) -> Result<(), ArchiveError> {
            panic!("queued frame remains raw bytes")
        }
    }

    fn operation() -> Operation {
        Operation::new(
            "unique-archive-storage-fixture".into(),
            7,
            crate::operation::Allowance::required(Some(
                tokio::time::Instant::now() + std::time::Duration::from_secs(60),
            ))
            .unwrap(),
        )
        .unwrap()
    }

    fn endpoints(original: &Operation) -> (OwnedSender, OwnedReceiver) {
        let mut pending = PendingOwnedChannel::new();
        pending.bind_original(original).unwrap();
        match pending.into_endpoints(original) {
            Ok(pair) => pair,
            Err(_) => panic!("fixture handoff"),
        }
    }

    fn body(original: &Operation) -> ChildFrame {
        ChildFrame {
            text: "retained archive body".into(),
            origin: None,
            operation: original.clone(),
        }
    }

    #[test]
    fn archive_requires_unique_closed_same_storage_and_retains_body_after_sink_failure() {
        let original = operation();
        let foreign = operation();
        let (sender, mut receiver) = endpoints(&original);
        let frame = body(&original);
        let pointer = frame.text.as_ptr();
        sender.0.state.lock().unwrap().frames = owned::Frames::single_frame_fixture(frame);
        let before = original.captured_bytes();
        let mut sink = Sink {
            calls: 0,
            pointer,
            fail: false,
        };
        assert_eq!(
            receiver.visit_unique_storage(&original, "fixture-child", &mut sink),
            Err(ArchiveError::Identity)
        );
        assert_eq!(sink.calls, 0); // A live sender prevents unique access.
        drop(sender); // Channel closure ONLY; no physical join is claimed.
        let extra = receiver.0.clone();
        assert_eq!(
            receiver.visit_unique_storage(&original, "fixture-child", &mut sink),
            Err(ArchiveError::Identity)
        );
        assert_eq!(sink.calls, 0);
        drop(extra);
        let weak = Arc::downgrade(&receiver.0);
        assert_eq!(
            receiver.visit_unique_storage(&original, "fixture-child", &mut sink),
            Err(ArchiveError::Identity)
        );
        assert_eq!(sink.calls, 0);
        drop(weak);
        assert_eq!(
            receiver.visit_unique_storage(&foreign, "fixture-child", &mut sink),
            Err(ArchiveError::Identity)
        );
        assert_eq!(sink.calls, 0);
        original.overflow(); // Failed evidence remains borrowable, never new credit.
        sink.fail = true;
        assert_eq!(
            receiver.visit_unique_storage(&original, "fixture-child", &mut sink),
            Err(ArchiveError::Fit)
        );
        assert_eq!(sink.calls, 1);
        {
            let state = Arc::get_mut(&mut receiver.0)
                .unwrap()
                .state
                .get_mut()
                .unwrap();
            assert!(!state.frames.pristine());
            assert_eq!(state.frames.front().unwrap().text.as_ptr(), pointer);
        }
        sink.fail = false;
        receiver
            .visit_unique_storage(&original, "fixture-child", &mut sink)
            .unwrap();
        assert_eq!(sink.calls, 2);
        assert_eq!(original.captured_bytes(), before);
        assert_eq!(original.check(), Err(OperationError::CaptureOverflow));
    }

    #[test]
    fn archive_refuses_unbound_foreign_row_poison_and_closed_receiver_before_callbacks() {
        let original = operation();
        let foreign = operation();
        let pending = PendingOwnedChannel::new();
        let mut unbound = pending.receiver;
        drop(pending.sender);
        let mut sink = Sink {
            calls: 0,
            pointer: std::ptr::null(),
            fail: false,
        };
        assert_eq!(
            unbound.visit_unique_storage(&original, "fixture-child", &mut sink),
            Err(ArchiveError::Identity)
        );
        assert_eq!(sink.calls, 0);

        let (sender, mut receiver) = endpoints(&original);
        let frame = body(&foreign);
        let pointer = frame.text.as_ptr();
        sender.0.state.lock().unwrap().frames = owned::Frames::single_frame_fixture(frame);
        drop(sender);
        assert_eq!(
            receiver.visit_unique_storage(&original, "fixture-child", &mut sink),
            Err(ArchiveError::Identity)
        );
        assert_eq!(sink.calls, 0);
        assert_eq!(
            receiver
                .0
                .state
                .lock()
                .unwrap()
                .frames
                .front()
                .unwrap()
                .text
                .as_ptr(),
            pointer
        );

        let (sender, mut poisoned) = endpoints(&original);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = sender.0.state.lock().unwrap();
            panic!("synthetic archive storage poison");
        }));
        assert!(result.is_err());
        drop(sender);
        assert_eq!(
            poisoned.visit_unique_storage(&original, "fixture-child", &mut sink),
            Err(ArchiveError::Identity)
        );
        assert!(poisoned.0.state.is_poisoned());
        assert_eq!(sink.calls, 0);

        let (sender, mut closed) = endpoints(&original);
        sender.0.state.lock().unwrap().receiver_closed = true;
        drop(sender);
        assert_eq!(
            closed.visit_unique_storage(&original, "fixture-child", &mut sink),
            Err(ArchiveError::Identity)
        );
        assert_eq!(sink.calls, 0);
    }
}

// me9c795490fee: SOURCE storage precursor ONLY. Empty pre-effect staging uses
// this representation; no live channel selects it. Original debit + publication
// protocol must be assembled in the whole joint, not replaced by legacy charge.
#[allow(dead_code)]
mod owned {
    use super::{ChildFrame, Operation, OperationError};
    use std::alloc::Layout;

    struct Node {
        frame: ChildFrame,
        next: Option<Box<Node>>,
    }

    /// Each frame is inline in exactly one concrete node. Referenced payloads
    /// remain separate; this is neither an allocator/private-layout factor nor F.
    pub(super) fn node_layout() -> Layout {
        Layout::new::<Node>()
    }

    /// Fixed owner metadata, to be included by the complete pre-effect owner
    /// graph. No queue allocation, Operation access or debit occurs here.
    pub(super) fn header_layout() -> Layout {
        Layout::new::<Frames>()
    }

    #[derive(Default)]
    pub(super) struct Frames {
        front: Option<Box<Node>>,
        back: Option<Box<Node>>,
        len: usize,
        #[cfg(test)]
        relink_steps: usize,
    }

    impl Frames {
        pub(super) fn visit_normalized(
            &mut self,
            original: &Operation,
            physical_child: &str,
            sink: &mut impl super::super::archive_stdout::RecordSink,
        ) -> Result<(), crate::failed_evidence::ArchiveError> {
            self.normalize().visit(original, physical_child, sink)
        }

        pub(super) fn front(&mut self) -> Option<&ChildFrame> {
            self.refill_front();
            self.front.as_deref().map(|node| &node.frame)
        }

        #[cfg(test)]
        pub(super) fn single_frame_fixture(frame: ChildFrame) -> Self {
            // Independent storage fixture ONLY, never a production admission.
            let mut frames = Self::default();
            frames.link(Box::new(Node { frame, next: None }), 1);
            frames
        }

        pub(super) fn pristine(&self) -> bool {
            self.len == 0 && self.front.is_none() && self.back.is_none()
        }

        /// Read-only preflight while the canonical pending slot still owns the
        /// whole frame. The future trusted caller must also check channel closure,
        /// admit ALL children and this exact layout to SAME original G, and keep
        /// the checked owner exclusively borrowed until linking. This function
        /// does not authorize allocation or prove that any debit happened.
        fn pending_layout(
            &self,
            original: &Operation,
            pending: &Option<ChildFrame>,
        ) -> Result<Layout, OperationError> {
            let frame = pending.as_ref().ok_or(OperationError::CreditOwned)?;
            if !frame.operation.same_instance(original) {
                return Err(OperationError::CreditOwned);
            }
            original.check()?;
            self.len
                .checked_add(1)
                .ok_or(OperationError::CaptureOverflow)?;
            Ok(node_layout())
        }

        // There deliberately is NO production allocation/debit method here.
        // The sole allocator below is a cfg(test) arithmetic/storage fixture.
        // Whole-joint wiring must allocate only after unpublished SAME-G admission
        // and must retain raw refusal before unlock/publication (m5be).
        fn link(&mut self, mut node: Box<Node>, checked_len: usize) {
            debug_assert!(node.next.is_none());
            debug_assert_eq!(self.len.checked_add(1), Some(checked_len));
            node.next = self.back.take();
            self.back = Some(node);
            self.len = checked_len;
        }

        /// Reverse only when front is empty: existing Boxes are relinked, with no
        /// allocation, body move/copy, debit or refund. Each node crosses once.
        fn refill_front(&mut self) {
            if self.front.is_some() {
                return;
            }
            while let Some(mut node) = self.back.take() {
                self.back = node.next.take();
                node.next = self.front.take();
                self.front = Some(node);
                #[cfg(test)]
                {
                    self.relink_steps += 1;
                }
            }
        }

        pub(super) fn pop(&mut self) -> Option<ChildFrame> {
            self.refill_front();
            let mut node = self.front.take()?;
            self.front = node.next.take();
            self.len -= 1;
            // Detaching next before moving the frame also makes node retirement
            // nonrecursive. The frame's existing body allocations are unchanged.
            Some(node.frame)
        }

        /// Future archive caller must establish actual joins/unique ownership and
        /// preflight EVERY row before any callback. This borrow grants no custody.
        /// Consolidate front then back into one FIFO chain, allocation-free. Tail
        /// traversal happens only during exclusive archive normalization, never on
        /// enqueue; one traversal per normalization, not quadratic queueing.
        fn normalize(&mut self) -> Normalized<'_> {
            if self.back.is_some() {
                let saved_front = self.front.take();
                self.refill_front();
                let ordered_back = self.front.take();
                self.front = saved_front;
                let mut tail = &mut self.front;
                while let Some(node) = tail {
                    tail = &mut node.next;
                }
                *tail = ordered_back;
            }
            Normalized {
                first: self.front.as_deref(),
                len: self.len,
            }
        }
    }

    /// Immutable post-normalization cursor. It allocates nothing and cannot pop,
    /// replace, release or acknowledge a frame. No second evidence collection.
    struct Normalized<'a> {
        first: Option<&'a Node>,
        len: usize,
    }

    impl Normalized<'_> {
        fn iter(&self) -> Iter<'_> {
            Iter(self.first)
        }

        fn validate(
            &self,
            original: &Operation,
            physical_child: &str,
        ) -> Result<(), crate::failed_evidence::ArchiveError> {
            use crate::failed_evidence::ArchiveError;
            let mut count = 0usize;
            for (index, frame) in self.iter().enumerate() {
                if !frame.operation.same_instance(original) {
                    return Err(ArchiveError::Identity);
                }
                // Existing record shape and checked ordinal, preflighted for ALL
                // rows. Deliberately no live/deadline check: failed evidence stays
                // borrowable after overflow; this still grants no release/ACK.
                super::super::queued_stdout_record(frame, physical_child, index)?;
                count = count.checked_add(1).ok_or(ArchiveError::Fit)?;
            }
            if count != self.len {
                return Err(ArchiveError::Identity);
            }
            Ok(())
        }

        fn visit(
            &self,
            original: &Operation,
            physical_child: &str,
            sink: &mut impl super::super::archive_stdout::RecordSink,
        ) -> Result<(), crate::failed_evidence::ArchiveError> {
            self.validate(original, physical_child)?;
            for (index, frame) in self.iter().enumerate() {
                sink.bytes(
                    super::super::queued_stdout_record(frame, physical_child, index)?,
                    frame.text.as_bytes(),
                )?;
            }
            Ok(())
        }
    }

    struct Iter<'a>(Option<&'a Node>);

    impl<'a> Iterator for Iter<'a> {
        type Item = &'a ChildFrame;

        fn next(&mut self) -> Option<Self::Item> {
            let node = self.0.take()?;
            self.0 = node.next.as_deref();
            Some(&node.frame)
        }
    }

    impl Drop for Frames {
        fn drop(&mut self) {
            // Explicitly detach BOTH chains before each Box is dropped. This is
            // destruction of the owner, not successful archive disposition or ACK.
            for stack in [&mut self.front, &mut self.back] {
                while let Some(mut node) = stack.take() {
                    *stack = node.next.take();
                }
            }
        }
    }

    #[cfg(test)]
    mod source_controls {
        // UNCOMPILED/UNRUN. Legacy Operation is an arithmetic fixture only, not
        // a sealed original Fit or the production m5be unpublished debit route.
        use super::*;
        use crate::operation::{Allowance, OPERATION_BYTES};
        use tokio::time::{Duration, Instant};

        fn operation() -> Operation {
            Operation::new(
                "owned-fifo-storage-fixture".into(),
                7,
                Allowance::required(Some(Instant::now() + Duration::from_secs(60))).unwrap(),
            )
            .unwrap()
        }

        fn frame(original: &Operation, text: String) -> Option<ChildFrame> {
            Some(ChildFrame {
                text,
                origin: None,
                operation: original.clone(),
            })
        }

        fn push_fixture(
            queue: &mut Frames,
            original: &Operation,
            pending: &mut Option<ChildFrame>,
        ) -> Result<(), OperationError> {
            let layout = queue.pending_layout(original, pending)?;
            let next = queue
                .len
                .checked_add(1)
                .ok_or(OperationError::CaptureOverflow)?;
            original.charge(layout.size())?; // BEFORE the fixture's ONLY Box allocation/MOVE.
            let node = Box::new(Node {
                frame: pending.take().unwrap(),
                next: None,
            });
            queue.link(node, next);
            Ok(())
        }

        #[test]
        fn interleaving_preserves_fifo_identity_and_charges_inline_node_once() {
            let original = operation();
            let before = original.captured_bytes();
            let mut queue = Frames::default();
            let mut first = frame(&original, "first".into());
            let pointer = first.as_ref().unwrap().text.as_ptr();
            push_fixture(&mut queue, &original, &mut first).unwrap();
            assert!(first.is_none());
            for text in ["second", "third"] {
                push_fixture(&mut queue, &original, &mut frame(&original, text.into())).unwrap();
            }
            let first = queue.pop().unwrap();
            assert_eq!(first.text.as_ptr(), pointer);
            assert!(first.operation.same_instance(&original));
            assert_eq!(first.text, "first");
            push_fixture(
                &mut queue,
                &original,
                &mut frame(&original, "fourth".into()),
            )
            .unwrap();
            let charged = original.captured_bytes();
            assert_eq!(charged - before, 4 * Layout::new::<Node>().size());
            assert_eq!(queue.pop().unwrap().text, "second");
            assert_eq!(queue.pop().unwrap().text, "third");
            assert_eq!(queue.pop().unwrap().text, "fourth");
            assert!(queue.pop().is_none());
            assert_eq!(queue.relink_steps, 4);
            assert_eq!(original.captured_bytes(), charged);
        }

        #[test]
        fn short_node_budget_and_foreign_identity_keep_whole_candidate_and_queue() {
            let original = operation();
            let foreign = operation();
            let mut queue = Frames::default();
            push_fixture(
                &mut queue,
                &original,
                &mut frame(&original, "prefix".into()),
            )
            .unwrap();
            let mut wrong = frame(&foreign, "foreign".into());
            let wrong_pointer = wrong.as_ref().unwrap().text.as_ptr();
            let before = original.captured_bytes();
            assert_eq!(
                push_fixture(&mut queue, &original, &mut wrong),
                Err(OperationError::CreditOwned)
            );
            assert_eq!(wrong.as_ref().unwrap().text.as_ptr(), wrong_pointer);
            assert_eq!(original.captured_bytes(), before);
            let remaining = node_layout().size() - 1;
            original
                .charge(OPERATION_BYTES - before - remaining)
                .unwrap();
            let mut pending = frame(&original, "short".into());
            let pointer = pending.as_ref().unwrap().text.as_ptr();
            let back_node = queue.back.as_deref().unwrap() as *const Node;
            assert_eq!(
                push_fixture(&mut queue, &original, &mut pending),
                Err(OperationError::CaptureOverflow)
            );
            assert_eq!(pending.as_ref().unwrap().text.as_ptr(), pointer);
            assert_eq!(queue.back.as_deref().unwrap() as *const Node, back_node);
            assert_eq!(queue.len, 1);
            assert!(queue.front.is_none());
            assert_eq!(queue.pop().unwrap().text, "prefix");
            assert_eq!(original.captured_bytes(), OPERATION_BYTES - remaining);
        }

        #[test]
        fn exclusive_normalization_appends_back_after_front_without_body_copy_or_debit() {
            let original = operation();
            let mut queue = Frames::default();
            for text in ["a", "b", "c"] {
                push_fixture(&mut queue, &original, &mut frame(&original, text.into())).unwrap();
            }
            assert_eq!(queue.pop().unwrap().text, "a");
            let second_node = queue.front.as_deref().unwrap() as *const Node;
            for text in ["d", "e"] {
                push_fixture(&mut queue, &original, &mut frame(&original, text.into())).unwrap();
            }
            let charged = original.captured_bytes();
            let rows = queue.normalize();
            assert_eq!(rows.len, 4);
            assert_eq!(rows.first.unwrap() as *const Node, second_node);
            assert!(
                rows.iter()
                    .map(|frame| frame.text.as_str())
                    .eq(["b", "c", "d", "e"])
            );
            assert!(
                rows.iter()
                    .all(|frame| frame.operation.same_instance(&original))
            );
            assert_eq!(original.captured_bytes(), charged);
            assert!(queue.back.is_none());
            assert!(
                queue
                    .normalize()
                    .iter()
                    .map(|frame| frame.text.as_str())
                    .eq(["b", "c", "d", "e"])
            );
            assert_eq!(queue.relink_steps, 5);
            assert_eq!(queue.len, 4);
        }

        #[test]
        fn long_backlog_relinks_once_per_node_and_both_deep_stacks_drop_iteratively() {
            let original = operation();
            let mut queue = Frames::default();
            const COUNT: usize = 16_384;
            for index in 0..COUNT {
                push_fixture(
                    &mut queue,
                    &original,
                    &mut frame(&original, index.to_string()),
                )
                .unwrap();
            }
            for index in 0..COUNT {
                assert_eq!(queue.pop().unwrap().text, index.to_string());
            }
            assert_eq!(queue.relink_steps, COUNT); // Structural work, NOT measured CPU time.
            let charged = original.captured_bytes();
            drop(queue);
            assert_eq!(original.captured_bytes(), charged);

            // Independent storage-only deep fixture, no F/G/body-fit claim.
            let mut deep = Frames::default();
            for _ in 0..COUNT {
                let node = Box::new(Node {
                    frame: frame(&original, String::new()).unwrap(),
                    next: None,
                });
                let next = deep.len + 1;
                deep.link(node, next);
            }
            deep.refill_front();
            for _ in 0..COUNT {
                let node = Box::new(Node {
                    frame: frame(&original, String::new()).unwrap(),
                    next: None,
                });
                let next = deep.len + 1;
                deep.link(node, next);
            }
            assert!(deep.front.is_some() && deep.back.is_some());
            drop(deep); // Both 16K chains, no recursive chain destructor.
            assert_eq!(original.captured_bytes(), charged);
        }

        #[test]
        fn archive_preflights_last_row_and_sink_failure_never_pops_or_refunds() {
            use crate::failed_evidence::{ArchiveError, EvidenceCategory, EvidenceRecord};
            struct Sink {
                calls: usize,
                fail_at: Option<usize>,
            }
            impl super::super::super::archive_stdout::RecordSink for Sink {
                fn bytes(
                    &mut self,
                    record: EvidenceRecord<'_>,
                    bytes: &[u8],
                ) -> Result<(), ArchiveError> {
                    assert!(matches!(record.category, EvidenceCategory::Stdout));
                    assert_eq!(record.part, "queued_frame");
                    assert_eq!(record.index, self.calls as u64);
                    assert_eq!(record.physical_child, Some("fixture-child"));
                    assert_eq!(bytes, [b'a' + self.calls as u8]);
                    let fail = self.fail_at == Some(self.calls);
                    self.calls += 1;
                    if fail { Err(ArchiveError::Fit) } else { Ok(()) }
                }
                fn json<T: serde::Serialize + ?Sized>(
                    &mut self,
                    _: EvidenceRecord<'_>,
                    _: &T,
                ) -> Result<(), ArchiveError> {
                    panic!("queued-frame records are raw bytes")
                }
            }
            let original = operation();
            let foreign = operation();
            let mut queue = Frames::default();
            for text in ["a", "b", "c"] {
                push_fixture(&mut queue, &original, &mut frame(&original, text.into())).unwrap();
            }
            let charged = original.captured_bytes();
            // Deliberately corrupt the final FIFO row in this private fixture.
            queue.back.as_mut().unwrap().frame.operation = foreign;
            let mut sink = Sink {
                calls: 0,
                fail_at: None,
            };
            assert_eq!(
                queue
                    .normalize()
                    .visit(&original, "fixture-child", &mut sink),
                Err(ArchiveError::Identity)
            );
            assert_eq!(sink.calls, 0);
            assert_eq!(queue.len, 3);
            let mut last = &mut queue.front;
            while let Some(node) = last {
                if node.next.is_none() {
                    node.frame.operation = original.clone();
                    break;
                }
                last = &mut node.next;
            }
            let mut sink = Sink {
                calls: 0,
                fail_at: Some(1),
            };
            assert_eq!(
                queue
                    .normalize()
                    .visit(&original, "fixture-child", &mut sink),
                Err(ArchiveError::Fit)
            );
            assert_eq!(sink.calls, 2);
            assert_eq!(queue.len, 3);
            let rows = queue.normalize();
            assert!(
                rows.iter()
                    .map(|frame| frame.text.as_str())
                    .eq(["a", "b", "c"])
            );
            assert_eq!(original.captured_bytes(), charged);
        }
    }
}

struct State {
    frames: VecDeque<ChildFrame>,
    sender_closed: bool,
    receiver_closed: bool,
}

struct Shared {
    state: Mutex<State>,
    ready: Notify,
}

// Exactly one reader task sends and exactly one ChildProc receives. Neither
// endpoint is Clone; Notify bookkeeping and the sole receive future are fixed.
pub(super) struct Sender(Arc<Shared>);
pub(super) struct Receiver(Arc<Shared>);

pub(super) fn channel(operation: &Operation) -> Result<(Sender, Receiver), OperationError> {
    operation.charge(
        std::mem::size_of::<Shared>()
            + std::mem::size_of::<Sender>()
            + std::mem::size_of::<Receiver>()
            + 2 * std::mem::size_of::<usize>() // Arc's strong/weak counters
            + std::mem::size_of::<tokio::sync::futures::Notified<'static>>(),
    )?;
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            frames: VecDeque::new(), // backing storage admitted on first enqueue
            sender_closed: false,
            receiver_closed: false,
        }),
        ready: Notify::new(),
    });
    Ok((Sender(shared.clone()), Receiver(shared)))
}

impl Sender {
    /// The existing pending owner retains its SAME frame on every refusal. The
    /// only move occurs after checks/growth succeed, immediately before enqueue.
    pub(super) fn send(&self, pending: &mut Option<ChildFrame>) -> Result<(), OperationError> {
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| OperationError::CreditOwned)?;
        if state.receiver_closed {
            return Err(OperationError::CreditOwned);
        }
        let frame = pending.as_ref().ok_or(OperationError::CreditOwned)?;
        let operation = &frame.operation;
        operation.check()?;
        let needed = state
            .frames
            .len()
            .checked_add(1)
            .ok_or_else(|| operation.overflow())?;
        if needed > state.frames.capacity() {
            let target = needed
                .checked_next_power_of_two()
                .ok_or_else(|| operation.overflow())?;
            // Count the whole requested replacement backing allocation and moved
            // old element headers BEFORE growth. Bodies remain in their original
            // allocations and were separately charged by capture/enqueue. Include
            // conservative capacity headroom; no post-overflow allocator call.
            let admitted_capacity = target.checked_mul(2).ok_or_else(|| operation.overflow())?;
            let bytes = admitted_capacity
                .checked_add(state.frames.capacity())
                .and_then(|n| n.checked_mul(std::mem::size_of::<ChildFrame>()))
                .and_then(|n| n.checked_add(std::mem::size_of::<VecDeque<ChildFrame>>()))
                .ok_or_else(|| operation.overflow())?;
            operation.charge(bytes)?;
            let additional = target - state.frames.len();
            state
                .frames
                .try_reserve_exact(additional)
                .map_err(|_| operation.overflow())?;
            if state.frames.capacity() > admitted_capacity {
                return Err(operation.overflow());
            }
        }
        state
            .frames
            .push_back(pending.take().expect("same checked frame"));
        drop(state);
        self.0.ready.notify_one(); // outside the queue lock
        Ok(())
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        // Closure never clears queued bodies. Wake empty waiters as well as a
        // waiter which registered immediately before checking sender_closed.
        self.0
            .state
            .lock()
            .expect("stdout sender closure")
            .sender_closed = true;
        self.0.ready.notify_waiters();
    }
}

impl Receiver {
    pub(super) fn len(&self) -> usize {
        self.0
            .state
            .lock()
            .expect("stdout queue length")
            .frames
            .len()
    }

    pub(super) async fn recv(&mut self) -> Option<ChildFrame> {
        loop {
            let notified = self.0.ready.notified();
            tokio::pin!(notified);
            // Register BEFORE checking the queue/EOF. Cancellation drops this
            // waiter only; it never pops a body or acknowledges ownership.
            notified.as_mut().enable();
            {
                let mut state = self.0.state.lock().expect("stdout receive");
                if let Some(frame) = state.frames.pop_front() {
                    return Some(frame);
                }
                if state.sender_closed {
                    return None;
                }
            }
            notified.await;
        }
    }

    /// Caller must first establish actual original child + ALL reader joins.
    /// Sender EOF alone is insufficient. No callback, await or I/O under a lock:
    /// unique Arc ownership permits Mutex::get_mut instead of a held guard.
    pub(super) fn joined_frames(&mut self) -> Result<&VecDeque<ChildFrame>, OperationError> {
        let shared = Arc::get_mut(&mut self.0).ok_or(OperationError::CreditOwned)?;
        let state = shared
            .state
            .get_mut()
            .map_err(|_| OperationError::CreditOwned)?;
        if !state.sender_closed || state.receiver_closed {
            return Err(OperationError::CreditOwned);
        }
        Ok(&state.frames)
    }

    /// Only the driver's synchronous verified-custody interval calls this,
    /// after joined_frames preflight of EVERY original owner. Exclusive access
    /// and no intervening callbacks/awaits preserve those unique-storage facts.
    /// EOF/consumer cancellation alone never enters this path; capacity and
    /// original cumulative charges are retained rather than refunded.
    pub(super) fn clear_archived_frames(&mut self) {
        let shared = Arc::get_mut(&mut self.0).expect("preflighted archive FIFO");
        let state = shared
            .state
            .get_mut()
            .expect("preflighted archive FIFO state");
        assert!(state.sender_closed && !state.receiver_closed);
        state.frames.clear();
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[cfg(test)]
    pub(super) fn try_recv(&mut self) -> Result<ChildFrame, OperationError> {
        self.0
            .state
            .lock()
            .expect("stdout try receive")
            .frames
            .pop_front()
            .ok_or(OperationError::CreditOwned)
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.0
            .state
            .lock()
            .expect("stdout receiver closure")
            .receiver_closed = true;
        // Queued evidence remains in the SAME shared state. The production
        // driver retains this receiver through uncertainty; Drop is no ACK.
        self.0.ready.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::{Allowance, OPERATION_BYTES};
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Waker};
    use tokio::time::{Duration, Instant};

    fn operation() -> Operation {
        Operation::new(
            "stdout-original".into(),
            7,
            Allowance::required(Some(Instant::now() + Duration::from_secs(60))).unwrap(),
        )
        .unwrap()
    }

    fn frame(operation: &Operation, text: &str) -> Option<ChildFrame> {
        Some(ChildFrame {
            text: text.into(),
            origin: None,
            operation: operation.clone(),
        })
    }

    struct WakeCount(AtomicUsize);
    impl std::task::Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn fifo_borrow_after_sender_close_keeps_original_bodies_and_eof_after_drain() {
        let operation = operation();
        let (tx, mut rx) = channel(&operation).unwrap();
        let mut first = frame(&operation, "first");
        let first_ptr = first.as_ref().unwrap().text.as_ptr();
        tx.send(&mut first).unwrap();
        tx.send(&mut frame(&operation, "second")).unwrap();
        assert!(rx.joined_frames().is_err()); // live sender is not joined custody
        drop(tx);
        let charged = operation.captured_bytes();
        let rows = rx.joined_frames().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].text.as_ptr(), first_ptr);
        assert!(
            rows.iter()
                .all(|row| row.operation.same_instance(&operation))
        );
        assert_eq!(rows[0].text, "first");
        assert_eq!(rows[1].text, "second");
        assert_eq!(rx.len(), 2); // borrowed enumeration consumed nothing
        assert_eq!(rx.try_recv().unwrap().text, "first");
        assert_eq!(rx.try_recv().unwrap().text, "second");
        let mut receive = std::pin::pin!(rx.recv());
        assert!(matches!(
            receive
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(None)
        ));
        assert_eq!(operation.captured_bytes(), charged); // no dequeue/EOF refund
    }

    #[test]
    fn receiver_close_refuses_growth_and_preserves_original_pending_and_queued_evidence() {
        let operation = operation();
        let (tx, mut rx) = channel(&operation).unwrap();
        tx.send(&mut frame(&operation, "already queued")).unwrap();
        assert_eq!(rx.try_recv().unwrap().text, "already queued");
        tx.send(&mut frame(&operation, "retained queued")).unwrap();
        drop(rx);
        let mut pending = frame(&operation, "refused pending");
        let ptr = pending.as_ref().unwrap().text.as_ptr();
        let charged = operation.captured_bytes();
        assert_eq!(tx.send(&mut pending), Err(OperationError::CreditOwned));
        assert_eq!(pending.as_ref().unwrap().text.as_ptr(), ptr);
        assert!(
            pending
                .as_ref()
                .unwrap()
                .operation
                .same_instance(&operation)
        );
        assert_eq!(operation.captured_bytes(), charged);
        assert_eq!(
            tx.0.state.lock().unwrap().frames.front().unwrap().text,
            "retained queued"
        );
    }

    #[test]
    fn registered_wait_enqueue_and_eof_wake_without_consuming_on_cancellation() {
        let operation = operation();
        let (tx, mut rx) = channel(&operation).unwrap();
        let wakes = Arc::new(WakeCount(AtomicUsize::new(0)));
        let waker = Waker::from(wakes.clone());
        let mut context = Context::from_waker(&waker);
        {
            let mut waiting = std::pin::pin!(rx.recv());
            assert!(matches!(waiting.as_mut().poll(&mut context), Poll::Pending));
            tx.send(&mut frame(&operation, "arrived during borrowed wait"))
                .unwrap();
            assert_eq!(wakes.0.load(Ordering::SeqCst), 1);
            // Drop the registered borrowed wait before it can pop the frame.
        }
        assert_eq!(rx.len(), 1);
        assert_eq!(rx.try_recv().unwrap().text, "arrived during borrowed wait");
        {
            let mut waiting = std::pin::pin!(rx.recv());
            assert!(matches!(waiting.as_mut().poll(&mut context), Poll::Pending));
            let before = wakes.0.load(Ordering::SeqCst);
            drop(tx);
            assert_eq!(wakes.0.load(Ordering::SeqCst), before + 1);
            assert!(matches!(
                waiting.as_mut().poll(&mut context),
                Poll::Ready(None)
            ));
        }
        assert!(rx.joined_frames().unwrap().is_empty());
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[test]
    fn pre_registration_enqueue_and_closure_are_observed_without_a_notification_permit() {
        let operation = operation();
        let (tx, mut rx) = channel(&operation).unwrap();
        tx.send(&mut frame(&operation, "before registration"))
            .unwrap();
        drop(tx);
        let mut context = Context::from_waker(Waker::noop());
        {
            let mut receive = std::pin::pin!(rx.recv());
            match receive.as_mut().poll(&mut context) {
                Poll::Ready(Some(row)) => assert_eq!(row.text, "before registration"),
                _ => panic!("queued original must precede EOF"),
            }
        }
        let mut receive = std::pin::pin!(rx.recv());
        assert!(matches!(
            receive.as_mut().poll(&mut context),
            Poll::Ready(None)
        ));
    }

    #[test]
    fn growth_refusal_keeps_capacity_prefix_and_original_pending_after_sticky_overflow() {
        let operation = operation();
        let (tx, mut rx) = channel(&operation).unwrap();
        tx.send(&mut frame(&operation, "original prefix")).unwrap();
        let capacity = tx.0.state.lock().unwrap().frames.capacity();
        while rx.len() < capacity {
            tx.send(&mut frame(&operation, "prefix")).unwrap();
        }
        operation
            .charge(OPERATION_BYTES - operation.captured_bytes())
            .unwrap();
        let mut pending = frame(&operation, "growth refused");
        let pointer = pending.as_ref().unwrap().text.as_ptr();
        assert_eq!(tx.send(&mut pending), Err(OperationError::CaptureOverflow));
        assert_eq!(pending.as_ref().unwrap().text.as_ptr(), pointer);
        assert_eq!(tx.0.state.lock().unwrap().frames.capacity(), capacity);
        assert_eq!(rx.len(), capacity);
        assert_eq!(operation.captured_bytes(), OPERATION_BYTES);
        drop(tx);
        assert_eq!(
            rx.joined_frames().unwrap().front().unwrap().text,
            "original prefix"
        );
        assert_eq!(operation.check(), Err(OperationError::CaptureOverflow));
    }

    #[test]
    fn queue_metadata_is_admitted_before_creation_and_failure_does_not_renew_allowance() {
        let operation = operation();
        operation
            .charge(OPERATION_BYTES - operation.captured_bytes())
            .unwrap();
        let deadline = operation.allowance().deadline();
        assert!(matches!(
            channel(&operation),
            Err(OperationError::CaptureOverflow)
        ));
        assert_eq!(operation.allowance().deadline(), deadline);
        assert_eq!(operation.captured_bytes(), OPERATION_BYTES);
    }

    #[test]
    fn retained_fifo_blocks_rebind_and_growth_charges_its_original_frame_instance() {
        let original = operation();
        let replacement = operation(); // same textual identity is not the same operation
        let capture = Mutex::new(original.clone());
        let replacement_bytes = replacement.captured_bytes();
        let (tx, mut rx) = channel(&original).unwrap();
        let before = original.captured_bytes();
        tx.send(&mut frame(&original, "unknown original raw"))
            .unwrap();
        assert!(super::super::rebind_capture(&capture, replacement.clone(), || rx.len()).is_err());
        assert!(capture.lock().unwrap().same_instance(&original));
        assert!(original.captured_bytes() > before);
        assert_eq!(replacement.captured_bytes(), replacement_bytes);
        drop(tx);
        let rows = rx.joined_frames().unwrap();
        assert!(rows[0].origin.is_none());
        assert!(rows[0].operation.same_instance(&original));
        assert!(!rows[0].operation.same_instance(&replacement));
        assert_eq!(rx.len(), 1);
        assert!(super::super::rebind_capture(&capture, replacement, || rx.len()).is_err());
        assert_eq!(
            original.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }
}
