//! Private preallocated host replies. No channel or payload box is created by
//! claim/send/receive/settle. Actual cell/handle/frame layouts belong in F; any
//! growing payload must be admitted to the SAME G before its construction.
use std::sync::{Arc, Mutex};

use cna_core::ids::SeatId;
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReplyOwner {
    pub(crate) operation: Uuid,
    pub(crate) seat: SeatId,
    pub(crate) epoch: u64,
    pub(crate) revision: u32,
    pub(crate) host_generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ticket {
    owner: ReplyOwner,
    generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReplyError {
    Busy,
    Foreign,
    Closed,
    Lost,
    Unknown,
}
pub(crate) fn refusal(error: ReplyError) -> cna_seats::host::HostRefusal {
    use cna_seats::host::HostRefusal;
    match error {
        ReplyError::Busy => HostRefusal::StoppedBeforeWork,
        ReplyError::Foreign => HostRefusal::IdentityChanged,
        ReplyError::Closed | ReplyError::Lost | ReplyError::Unknown => HostRefusal::Unknown,
    }
}

struct State<T> {
    ticket: Option<Ticket>,
    generation: u64,
    value: Option<T>,
    completed: bool,
    taken: bool,
    sender_live: bool,
    receiver_live: bool,
    settled: bool,
}
pub(crate) struct FixedReply<T> {
    state: Mutex<State<T>>,
    changed: Notify,
}
pub(crate) struct ReplySender<T> {
    cell: Arc<FixedReply<T>>,
    ticket: Ticket,
    live: bool,
}
pub(crate) struct ReplyReceiver<T> {
    cell: Arc<FixedReply<T>>,
    ticket: Ticket,
    live: bool,
}

impl<T> FixedReply<T> {
    /// Campaign startup only. This concrete body's complete Layout is counted
    /// by the original-operation descriptor; no per-use Arc allocation follows.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                ticket: None,
                generation: 0,
                value: None,
                completed: false,
                taken: false,
                sender_live: false,
                receiver_live: false,
                settled: false,
            }),
            changed: Notify::new(),
        })
    }
    pub(crate) fn claim(
        self: &Arc<Self>,
        owner: ReplyOwner,
    ) -> Result<(ReplySender<T>, ReplyReceiver<T>), ReplyError> {
        let mut state = self.state.lock().map_err(|_| ReplyError::Unknown)?;
        if let Some(prior) = state.ticket {
            // A completed/taken phase can reuse its fixed slot only within the
            // SAME original owner. A lost/unconsumed reply waits for settlement.
            if prior.owner != owner {
                return Err(ReplyError::Foreign);
            }
            if !state.completed || !state.taken || state.sender_live || state.receiver_live {
                return Err(ReplyError::Busy);
            }
        }
        let generation = state.generation.checked_add(1).ok_or(ReplyError::Unknown)?;
        let ticket = Ticket { owner, generation };
        state.generation = generation;
        state.ticket = Some(ticket);
        state.completed = false;
        state.taken = false;
        state.sender_live = true;
        state.receiver_live = true;
        state.settled = false;
        debug_assert!(state.value.is_none());
        Ok((
            ReplySender {
                cell: Arc::clone(self),
                ticket,
                live: true,
            },
            ReplyReceiver {
                cell: Arc::clone(self),
                ticket,
                live: true,
            },
        ))
    }
    /// Only the original ordered post-supervision transition calls this. Keep
    /// live terminal ACK handles valid; mark the owner closed and clear it only
    /// after those handles cease owning a reply. No growth credit is refunded.
    pub(crate) fn settle(&self, owner: ReplyOwner) -> Result<(), ReplyError> {
        let mut state = self.state.lock().map_err(|_| ReplyError::Unknown)?;
        if state.ticket.is_some_and(|ticket| ticket.owner != owner) {
            return Err(ReplyError::Foreign);
        }
        state.settled = true;
        Self::clear_settled(&mut state);
        drop(state);
        self.changed.notify_waiters();
        Ok(())
    }
    fn clear_settled(state: &mut State<T>) {
        if state.settled && !state.sender_live && !state.receiver_live {
            state.ticket = None;
            // Ordered settlement, never dequeue/drop/timeout, permits this
            // extra transport body to disappear. Canonical R has its own owner.
            state.value = None;
        }
    }
    pub(crate) fn available(&self) -> Result<bool, ReplyError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| ReplyError::Unknown)?
            .ticket
            .is_none())
    }
    pub(crate) fn accepts(&self, owner: ReplyOwner) -> Result<(), ReplyError> {
        let state = self.state.lock().map_err(|_| ReplyError::Unknown)?;
        if state.ticket.is_some_and(|ticket| ticket.owner != owner) {
            Err(ReplyError::Foreign)
        } else {
            Ok(())
        }
    }
}
impl<T> ReplySender<T> {
    /// The retained canonical sender owns closure. A queued request borrows
    /// its exact ticket without allocating; canceled enqueue cannot destroy
    /// the original settlement reply. Only the first send can complete it.
    pub(crate) fn reborrow(&self) -> Self {
        Self {
            cell: Arc::clone(&self.cell),
            ticket: self.ticket,
            live: false,
        }
    }
    pub(crate) fn send(mut self, value: T) -> Result<(), ReplyError> {
        let mut state = self.cell.state.lock().map_err(|_| ReplyError::Unknown)?;
        if state.ticket != Some(self.ticket) {
            return Err(ReplyError::Foreign);
        }
        if !state.sender_live || state.completed {
            return Err(ReplyError::Closed);
        }
        state.value = Some(value); // first canonical transport reply retained
        state.completed = true;
        state.sender_live = false;
        self.live = false;
        let result = if state.receiver_live {
            Ok(())
        } else {
            Err(ReplyError::Lost)
        };
        FixedReply::clear_settled(&mut state);
        drop(state);
        self.cell.changed.notify_waiters();
        result
    }
}
impl<T> Drop for ReplySender<T> {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        if let Ok(mut state) = self.cell.state.lock()
            && state.ticket == Some(self.ticket)
        {
            state.sender_live = false;
            FixedReply::clear_settled(&mut state);
        }
        self.cell.changed.notify_waiters();
    }
}
impl<T> ReplyReceiver<T> {
    fn take_ready(&mut self) -> Result<Option<T>, ReplyError> {
        let mut state = self.cell.state.lock().map_err(|_| ReplyError::Unknown)?;
        if state.ticket != Some(self.ticket) {
            return Err(ReplyError::Foreign);
        }
        if state.taken || !self.live {
            return Err(ReplyError::Closed);
        }
        if state.completed {
            let value = state.value.take().ok_or(ReplyError::Unknown)?;
            state.taken = true;
            state.receiver_live = false;
            self.live = false;
            FixedReply::clear_settled(&mut state);
            return Ok(Some(value));
        }
        if !state.sender_live {
            state.receiver_live = false;
            self.live = false;
            FixedReply::clear_settled(&mut state);
            return Err(ReplyError::Closed);
        }
        Ok(None)
    }
    pub(crate) fn try_recv(&mut self) -> Result<T, ReplyError> {
        self.take_ready()?.ok_or(ReplyError::Busy)
    }
    pub(crate) async fn receive(&mut self) -> Result<T, ReplyError> {
        loop {
            // Register before inspecting State. notify_waiters cannot fall into
            // the check-to-await gap; registration lives in the actual frame.
            let cell = Arc::clone(&self.cell);
            let notified = cell.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(value) = self.take_ready()? {
                return Ok(value);
            }
            notified.await;
        }
    }
}
impl<T> Drop for ReplyReceiver<T> {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        if let Ok(mut state) = self.cell.state.lock()
            && state.ticket == Some(self.ticket)
        {
            state.receiver_live = false;
            FixedReply::clear_settled(&mut state);
        }
        // No implicit owner release and no discard of a completed lost reply.
        self.cell.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn taken_refusal_moves_without_copy_and_can_outlive_reused_fixed_cell() {
        use cna_seats::host::HostRefusal;
        let cell = FixedReply::<Result<(), HostRefusal>>::new();
        let mut retained = Vec::new();
        // Three demonstrates coexistence; it is NOT a maximum live-body N.
        for _ in 0..3 {
            let body = String::from("prompt still owned");
            let original_pointer = body.as_ptr();
            let original_capacity = body.capacity();
            let (send, mut receive) = cell.claim(owner(1)).unwrap();
            send.send(Err(HostRefusal::Ownership(body))).unwrap();
            let Err(HostRefusal::Ownership(body)) = receive.try_recv().unwrap() else {
                panic!("refusal body changed");
            };
            assert_eq!(body.as_ptr(), original_pointer);
            assert_eq!(body.capacity(), original_capacity);
            assert!(cell.state.lock().unwrap().value.is_none());
            retained.push(body);
        }
        // All returned bodies remain live despite the same cell's reuse.
        assert_eq!(retained.len(), 3);
        assert!(retained.iter().all(|body| body == "prompt still owned"));
        cell.settle(owner(1)).unwrap();
        assert_eq!(retained.len(), 3);
    }
    fn owner(n: u128) -> ReplyOwner {
        ReplyOwner {
            operation: Uuid::from_u128(n),
            seat: SeatId::all().next().unwrap(),
            epoch: 3,
            revision: 4,
            host_generation: 5,
        }
    }
    #[tokio::test]
    async fn actual_reply_is_taken_once_and_generation_cannot_cross_owners() {
        let cell = FixedReply::new();
        let (send, mut receive) = cell.claim(owner(1)).unwrap();
        let old = send.ticket;
        send.send(7u64).unwrap();
        assert_eq!(receive.try_recv().unwrap(), 7);
        assert_eq!(receive.try_recv(), Err(ReplyError::Closed));
        assert!(matches!(cell.claim(owner(2)), Err(ReplyError::Foreign)));
        let (send, mut receive) = cell.claim(owner(1)).unwrap();
        assert_ne!(send.ticket.generation, old.generation);
        assert_eq!(cell.settle(owner(2)), Err(ReplyError::Foreign));
        send.send(8).unwrap();
        assert_eq!(receive.receive().await.unwrap(), 8);
        cell.settle(owner(1)).unwrap();
        assert!(cell.claim(owner(2)).is_ok());
    }
    #[test]
    fn lost_reply_is_retained_and_blocks_reuse_until_actual_settlement() {
        let cell = FixedReply::new();
        let (send, receive) = cell.claim(owner(1)).unwrap();
        drop(receive);
        assert_eq!(send.send(9u64), Err(ReplyError::Lost));
        assert_eq!(cell.state.lock().unwrap().value, Some(9));
        assert!(matches!(cell.claim(owner(1)), Err(ReplyError::Busy)));
        cell.settle(owner(1)).unwrap();
        assert!(cell.state.lock().unwrap().value.is_none());
        assert!(cell.claim(owner(2)).is_ok());
    }
    #[tokio::test]
    async fn sender_cancel_closes_wait_and_terminal_ack_survives_settlement_latch() {
        let cell = FixedReply::<u64>::new();
        let (send, mut receive) = cell.claim(owner(1)).unwrap();
        drop(send);
        assert_eq!(receive.receive().await, Err(ReplyError::Closed));
        cell.settle(owner(1)).unwrap();
        let (send, mut receive) = cell.claim(owner(2)).unwrap();
        cell.settle(owner(2)).unwrap();
        send.send(10).unwrap();
        assert_eq!(receive.receive().await.unwrap(), 10);
        assert!(cell.state.lock().unwrap().ticket.is_none());
    }
    #[tokio::test]
    async fn canceled_transient_and_borrowed_wait_preserve_original_reply() {
        let cell = FixedReply::new();
        let (root, mut receive) = cell.claim(owner(1)).unwrap();
        drop(root.reborrow());
        assert_eq!(receive.try_recv(), Err(ReplyError::Busy));
        {
            let wait = receive.receive();
            tokio::pin!(wait);
            tokio::select! {
                biased;
                _ = &mut wait => panic!("reply completed before send"),
                _ = tokio::task::yield_now() => {}
            }
        }
        root.send(11u64).unwrap();
        assert_eq!(receive.receive().await.unwrap(), 11);
        cell.settle(owner(1)).unwrap();
    }
    #[tokio::test]
    async fn reserved_unit_reply_ignores_exhausted_growth_and_original_deadline() {
        use cna_seats::operation::{Allowance, OPERATION_BYTES, Operation};
        let cell = FixedReply::new(); // preexisting fixed storage before Operation
        let storage = Arc::as_ptr(&cell);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        let operation = Operation::new(
            Uuid::from_u128(1).to_string(),
            1,
            Allowance::required(Some(deadline)).unwrap(),
        )
        .unwrap();
        operation.charge(OPERATION_BYTES).unwrap();
        assert!(operation.charge(1).is_err());
        tokio::time::sleep_until(deadline).await;
        assert!(tokio::time::Instant::now() >= operation.allowance().deadline());
        let before = operation.captured_bytes();
        let (send, mut receive) = cell.claim(owner(1)).unwrap();
        cell.settle(owner(1)).unwrap();
        send.send(Ok::<(), cna_seats::host::HostRefusal>(()))
            .unwrap();
        receive.receive().await.unwrap().unwrap();
        assert_eq!(operation.captured_bytes(), before);
        assert_eq!(Arc::as_ptr(&cell), storage);
        // This exercises the real fixed slot, not a complete F producer. The
        // whole adapter must still prove its original precreation F seed.
    }
}
