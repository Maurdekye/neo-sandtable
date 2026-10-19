//! Private SOURCE assembly for the ONE owned driver task, not legacy adoption.
//! Retain the mailbox/reply roots until genuine task and child disposition.
//! No actor spawn or successful creation/erasure path is connected yet.

#![allow(dead_code)]

use std::alloc::Layout;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{Notify, oneshot};

use super::{DriverError, SessionInfo, TurnOutcome};
use crate::operation::{Operation, OperationError};

/// mdc21045cd0eb: stable1.99 alloc/sync.rs B2F806B0, ArcInner392-405.
/// This is the approved public-type layout derivation, not a private mirror.
fn own_arc_layout<T>() -> Result<Layout, OperationError> {
    let (layout, _) = Layout::new::<[AtomicUsize; 2]>()
        .extend(Layout::new::<T>())
        .map_err(|_| OperationError::CaptureOverflow)?;
    layout
        .align_to(layout.align().max(2))
        .map(|layout| layout.pad_to_align())
        .map_err(|_| OperationError::CaptureOverflow)
}

fn prepay(operation: &Operation, layout: Layout) -> Result<(), OperationError> {
    operation.check()?;
    operation.charge(layout.size())?;
    operation.check()
}

/// Input bodies never grow. The whole request keeps its original backing.
fn input_text(operation: &Operation, text: &str) -> Result<Box<str>, OperationError> {
    let layout = Layout::array::<u8>(text.len())
        .map_err(|_| OperationError::CaptureOverflow)?;
    prepay(operation, layout)?;
    Ok(Box::<str>::from(text))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CommandKind {
    Start,
    Turn,
    Stop,
}

pub(super) enum Command {
    Start { resume: Option<Box<str>> },
    Turn { prompt: Box<str>, limit: Duration },
    Stop,
}

impl Command {
    pub(super) fn start(
        original: &Operation,
        resume: Option<&str>,
    ) -> Result<Self, OperationError> {
        let resume = resume.map(|text| input_text(original, text)).transpose()?;
        Ok(Self::Start { resume })
    }

    pub(super) fn turn(
        original: &Operation,
        prompt: &str,
        limit: Duration,
    ) -> Result<Self, OperationError> {
        Ok(Self::Turn { prompt: input_text(original, prompt)?, limit })
    }

    fn kind(&self) -> CommandKind {
        match self {
            Self::Start { .. } => CommandKind::Start,
            Self::Turn { .. } => CommandKind::Turn,
            Self::Stop => CommandKind::Stop,
        }
    }
}

/// The actual result lives in its canonical root, never in a wake payload.
pub(super) enum ReplyBody {
    Start(Result<SessionInfo, DriverError>),
    Turn(Result<TurnOutcome, DriverError>),
    Stop(Result<(), DriverError>),
}

impl ReplyBody {
    fn kind(&self) -> CommandKind {
        match self {
            Self::Start(_) => CommandKind::Start,
            Self::Turn(_) => CommandKind::Turn,
            Self::Stop(_) => CommandKind::Stop,
        }
    }
}

#[derive(Clone, Copy)]
enum ReplySignal {
    Ready,
    TaskEnded,
}

struct ReplyState {
    channel_started: bool,
    body: Option<ReplyBody>,
    accepted: bool,
    receiver_dropped: bool,
    task_ended: bool,
}

struct ReplyCell {
    original: Operation,
    kind: CommandKind,
    state: Mutex<ReplyState>,
}

/// This ORIGINAL owner must stay in the canonical task root through disposition.
/// The sender and receiver are projections, not the last owners of raw results.
pub(super) struct ReplyRoot(Arc<ReplyCell>);

pub(super) struct ReplySender {
    cell: Arc<ReplyCell>,
    signal: Option<oneshot::Sender<ReplySignal>>,
}

pub(super) struct ReplyReceiver {
    cell: Arc<ReplyCell>,
    signal: oneshot::Receiver<ReplySignal>,
}

impl ReplyRoot {
    pub(super) fn new(
        original: &Operation,
        kind: CommandKind,
    ) -> Result<Self, OperationError> {
        prepay(original, own_arc_layout::<ReplyCell>()?)?;
        Ok(Self(Arc::new(ReplyCell {
            original: original.clone(),
            kind,
            state: Mutex::new(ReplyState {
                channel_started: false,
                body: None,
                accepted: false,
                receiver_dropped: false,
                task_ended: false,
            }),
        })))
    }

    /// Only after THIS root has been retained by the canonical owner.
    pub(super) fn begin(&self) -> Result<(ReplySender, ReplyReceiver), OperationError> {
        {
            let mut state = self.0.state.lock().map_err(|_| OperationError::CreditOwned)?;
            if state.channel_started {
                return Err(OperationError::CreditOwned);
            }
            state.channel_started = true; // Failed prepayment cannot reopen.
        }
        // Pinned C9B28240 oneshot Inner386-411: charge the public payload slot.
        // ONLY its fixed wrapper has the explicit mdc bookkeeping classification.
        prepay(&self.0.original, Layout::new::<Option<ReplySignal>>())?;
        let (sender, receiver) = oneshot::channel();
        Ok((
            ReplySender { cell: self.0.clone(), signal: Some(sender) },
            ReplyReceiver { cell: self.0.clone(), signal: receiver },
        ))
    }
}

impl ReplySender {
    /// FIRST whole Result MOVE, then unlock, then caller wake on the DRIVER task.
    /// A failed send keeps the raw body in the SAME root; no formatting/refund.
    pub(super) fn finish(mut self, body: ReplyBody) -> Result<(), (OperationError, ReplyBody)> {
        if body.kind() != self.cell.kind {
            return Err((OperationError::CreditOwned, body));
        }
        {
            let mut state = match self.cell.state.lock() {
                Ok(state) => state,
                Err(_) => return Err((OperationError::CreditOwned, body)),
            };
            if state.body.is_some() || state.accepted || state.task_ended {
                drop(state);
                return Err((OperationError::CreditOwned, body));
            }
            state.body = Some(body);
        }
        if let Some(signal) = self.signal.take() {
            let _ = signal.send(ReplySignal::Ready); // Root retains body on receiver drop.
        }
        Ok(())
    }
}

impl Drop for ReplySender {
    fn drop(&mut self) {
        let Some(signal) = self.signal.take() else { return };
        if let Ok(mut state) = self.cell.state.lock() {
            state.task_ended = true;
        }
        // No cell guard survives this potentially synchronous foreign wake.
        let _ = signal.send(ReplySignal::TaskEnded);
    }
}

impl ReplyReceiver {
    /// The ONLY caller-side await is the Tokio oneshot, never a worker Notify.
    /// There is no await after taking the whole body into the caller's return.
    pub(super) async fn wait(mut self) -> Result<ReplyBody, OperationError> {
        match (&mut self.signal).await {
            Ok(ReplySignal::Ready) => {}
            Ok(ReplySignal::TaskEnded) | Err(_) => return Err(OperationError::CreditOwned),
        }
        let mut state = self.cell.state.lock().map_err(|_| OperationError::CreditOwned)?;
        if state.accepted || state.task_ended {
            return Err(OperationError::CreditOwned);
        }
        let body = state.body.take().ok_or(OperationError::CreditOwned)?;
        state.accepted = true;
        Ok(body)
    }
}

impl Drop for ReplyReceiver {
    fn drop(&mut self) {
        if let Ok(mut state) = self.cell.state.lock()
            && !state.accepted
        {
            state.receiver_dropped = true;
        }
        // No raw body take, replacement, refund, notification or actor join.
    }
}

pub(super) struct Request {
    pub(super) command: Command,
    reply: Arc<ReplyCell>,
}

impl Request {
    /// The actor keeps the SOLE oneshot sender outside this command mutex, so
    /// task exit can fail replies even if mailbox storage is poisoned/unknown.
    pub(super) fn new(command: Command, reply: &ReplyRoot) -> Result<Self, (OperationError, Command)> {
        if command.kind() != reply.0.kind {
            return Err((OperationError::CreditOwned, command));
        }
        Ok(Self { command, reply: reply.0.clone() })
    }
}

struct MailboxState {
    request: Option<Request>, // EXACT capacity ONE, no Vec or growing read-ahead.
    endpoints_started: bool,
    sender_closed: bool,
    receiver_closed: bool,
}

struct MailboxCell {
    original: Operation,
    state: Mutex<MailboxState>,
    ready: Notify,
}

/// Retain independently of endpoint projections until genuine driver-task join.
pub(super) struct MailboxRoot(Arc<MailboxCell>);
pub(super) struct MailboxSender(Arc<MailboxCell>);
pub(super) struct MailboxReceiver(Arc<MailboxCell>);

impl MailboxRoot {
    pub(super) fn new(original: &Operation) -> Result<Self, OperationError> {
        prepay(original, own_arc_layout::<MailboxCell>()?)?;
        Ok(Self(Arc::new(MailboxCell {
            original: original.clone(),
            state: Mutex::new(MailboxState {
                request: None,
                endpoints_started: false,
                sender_closed: false,
                receiver_closed: false,
            }),
            ready: Notify::new(),
        })))
    }

    pub(super) fn endpoints(
        &mut self,
    ) -> Result<(MailboxSender, MailboxReceiver), OperationError> {
        {
            let mut state = self.0.state.lock().map_err(|_| OperationError::CreditOwned)?;
            if state.endpoints_started {
                return Err(OperationError::CreditOwned);
            }
            state.endpoints_started = true;
        }
        Ok((MailboxSender(self.0.clone()), MailboxReceiver(self.0.clone())))
    }
}

impl MailboxSender {
    /// A refusal leaves the WHOLE command, sender and exact inputs with caller.
    pub(super) fn try_send(&self, pending: &mut Option<Request>) -> Result<(), OperationError> {
        let cleanup = pending.as_ref().is_some_and(|request| request.command.kind() == CommandKind::Stop);
        if !cleanup {
            self.0.original.check()?;
        }
        {
            let mut state = self.0.state.lock().map_err(|_| OperationError::CreditOwned)?;
            let request = pending.as_ref().ok_or(OperationError::CreditOwned)?;
            if state.sender_closed || state.receiver_closed || state.request.is_some()
                || request.command.kind() != request.reply.kind
                || !self.0.original.same_instance(&request.reply.original)
            {
                return Err(OperationError::CreditOwned);
            }
            state.request = pending.take();
        }
        self.0.ready.notify_one(); // ALL command/reply exclusions gone.
        Ok(())
    }
}

impl Drop for MailboxSender {
    fn drop(&mut self) {
        let notify = if let Ok(mut state) = self.0.state.lock() {
            state.sender_closed = true;
            true
        } else { false };
        if notify {
            self.0.ready.notify_waiters();
        }
    }
}

impl MailboxReceiver {
    /// This future is PRIVATE and must be polled ONLY inside the ONE driver task.
    pub(super) async fn recv(&mut self) -> Result<Option<Request>, OperationError> {
        loop {
            let notified = self.0.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self.0.state.lock().map_err(|_| OperationError::CreditOwned)?;
                if state.receiver_closed {
                    return Err(OperationError::CreditOwned);
                }
                if let Some(request) = state.request.take() {
                    return Ok(Some(request));
                }
                if state.sender_closed {
                    return Ok(None);
                }
            }
            notified.await;
        }
    }
}

impl Drop for MailboxReceiver {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.receiver_closed = true;
        }
        // Keep queued raw command in canonical mailbox. Actual reply senders
        // belong to the actor exit owner OUTSIDE this potentially poisoned lock.
    }
}
