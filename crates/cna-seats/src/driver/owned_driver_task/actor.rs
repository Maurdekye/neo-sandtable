//! Sealed task/command boundary. The Host owns Root and the actual task handle;
//! the facade receives only Endpoint. No public registration or legacy fallback.

use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::driver::SeatDriver;
use crate::seat_host::ScopeControl;
use crate::seat_host::task::PublicationGate;

pub(crate) type DriverRoot = Root<OwnedCaller>;
pub(crate) type CommandEndpoint = Endpoint;

/// Fixed observation only. Canonical causes remain in caller/request/reply/join
/// roots, even when the driver body itself returned or its facade disappeared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DriverExit { Stopped, Unknown }

pub(super) struct ReplyWork {
    command: Option<Command>,
    start: Option<Result<SessionInfo, DriverError>>,
    turn: Option<Result<TurnOutcome, DriverError>>,
    stop: Option<Result<(), DriverError>>,
    observation: Option<Result<bool, DriverError>>,
}

impl ReplyWork {
    pub(super) fn empty() -> Self {
        Self { command: None, start: None, turn: None, stop: None, observation: None }
    }
}

/// Every accepted request has a distinct prepaid cell, retained until final
/// Host disposition. No ACK, receiver drop or dequeue removes a ledger node.
struct ReplyEntry { root: ReplyRoot, next: Option<Box<ReplyEntry>> }

struct ReplyLedger {
    ended: bool,
    inflight: bool,
    active: Option<ReplySender>,
    cleanup: Option<ReplySender>,
    roots: Option<Box<ReplyEntry>>,
}

struct Ingress {
    original: Operation,
    scope: Arc<ScopeControl>,
    ledger: Mutex<ReplyLedger>, // Separate from the poisonable command mutex.
    stop: AtomicBool,
    wake: Notify,
}

impl Ingress {
    fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.wake.notify_waiters();
    }

    async fn stopping(&self) {
        loop {
            let wake = self.wake.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            if self.stop.load(Ordering::Acquire) { return; }
            wake.await;
        }
    }

    /// Runs on the driver task, including task-future Drop. Recovering poison
    /// only extracts linear senders for terminal failure; it never admits work.
    fn ended(&self) {
        let (active, cleanup) = {
            let mut ledger = match self.ledger.lock() {
                Ok(ledger) => ledger,
                Err(cause) => cause.into_inner(),
            };
            ledger.ended = true;
            (ledger.active.take(), ledger.cleanup.take())
        };
        drop(active);
        drop(cleanup); // Caller wakes happen with NO ledger/mailbox guard.
    }
}

struct CallerState<C> {
    caller: Option<C>,
    cleanup: Option<Result<(), DriverError>>,
    cleanup_observation: Option<Result<bool, DriverError>>,
    undelivered: Option<ReplyBody>,
}

struct State<C> {
    original: Operation,
    caller: Mutex<CallerState<C>>,
    ingress: Arc<Ingress>,
    scope: Arc<ScopeControl>,
    gate: PublicationGate,
    uncertain: AtomicBool,
    refused_request: Mutex<Option<Request>>,
}

/// The Host retains this root independently of endpoint and task lifetimes.
/// The task captures State, never Root or the Host's entry/JoinHandle.
pub(crate) struct Root<C> {
    state: Arc<State<C>>,
    mailbox: MailboxRoot,
    cleanup: ReplyRoot,
}

pub(crate) struct Body<C> {
    state: Arc<State<C>>,
    receiver: MailboxReceiver,
}

/// No runtime Handle, caller, JoinHandle or public Notify/FIFO wait future.
pub(crate) struct Endpoint {
    ingress: Arc<Ingress>,
    mailbox: MailboxSender,
    cleanup: Option<ReplyReceiver>,
}

pub(crate) struct Prepared<C> {
    pub(crate) root: Root<C>,
    pub(crate) body: Body<C>,
    pub(crate) endpoint: Endpoint,
}

/// Production entry accepts only the concrete original caller. Its existing
/// full-creation check is unchanged and still CLOSED; no fixture can bypass it.
pub(crate) fn prepare(
    original: &Operation,
    scope: Arc<ScopeControl>,
    pending: &mut Option<OwnedCaller>,
) -> Result<Prepared<OwnedCaller>, OperationError> {
    pending.as_ref().ok_or(OperationError::CreditOwned)?
        .check_retained_trait_original(original)
        .map_err(|_| OperationError::CreditOwned)?;
    prepare_inner(original, scope, pending)
}

fn prepare_inner<C: SeatDriver + 'static>(
    original: &Operation,
    scope: Arc<ScopeControl>,
    pending: &mut Option<C>,
) -> Result<Prepared<C>, OperationError> {
    if pending.is_none() || scope.is_closed() { return Err(OperationError::CreditOwned); }
    // ALL own allocation layouts precede the respective allocation. No caller
    // MOVE until all fallible preparation and its cleanup channel have finished.
    prepay(original, own_arc_layout::<Ingress>()?)?;
    prepay(original, own_arc_layout::<State<C>>()?)?;
    let mut mailbox = MailboxRoot::new(original)?;
    let (sender, receiver) = mailbox.endpoints()?;
    let cleanup = ReplyRoot::new(original, CommandKind::Stop)?;
    let (cleanup_sender, cleanup_receiver) = cleanup.begin()?;
    let ingress = Arc::new(Ingress {
        original: original.clone(),
        scope: scope.clone(),
        ledger: Mutex::new(ReplyLedger {
            ended: false, inflight: false, active: None,
            cleanup: Some(cleanup_sender), roots: None,
        }),
        stop: AtomicBool::new(false), wake: Notify::new(),
    });
    let state = Arc::new(State {
        original: original.clone(),
        caller: Mutex::new(CallerState {
            caller: pending.take(), cleanup: None, cleanup_observation: None,
            undelivered: None,
        }),
        ingress: ingress.clone(), scope, gate: PublicationGate::closed(),
        uncertain: AtomicBool::new(false),
        refused_request: Mutex::new(None),
    });
    Ok(Prepared {
        root: Root { state: state.clone(), mailbox, cleanup },
        body: Body { state, receiver },
        endpoint: Endpoint { ingress, mailbox: sender, cleanup: Some(cleanup_receiver) },
    })
}

impl<C> Root<C> {
    /// Only the Host calls this AFTER roots and the actual handle are installed.
    pub(crate) fn publish(&self) { self.state.gate.publish(); }
    /// Private registration projection, not an entry or handle owner. Clone
    /// only the already prepaid Arc; invoke AFTER releasing registry guards.
    pub(crate) fn publication(&self) -> impl FnOnce() + use<C> {
        let state = self.state.clone();
        move || state.gate.publish()
    }
    pub(crate) fn request_stop(&self) { self.state.ingress.request_stop(); }
}

impl Endpoint {
    /// Synchronous bounded ingress, followed by exactly ONE caller-side oneshot
    /// await. Caller cancellation cannot remove a canonical request/reply root.
    fn submit(
        &mut self, command: Command,
    ) -> Result<ReplyReceiver, (OperationError, Option<Command>)> {
        if command.kind() == CommandKind::Stop || self.ingress.scope.is_closed()
            || self.ingress.stop.load(Ordering::Acquire)
        {
            return Err((OperationError::CreditOwned, Some(command)));
        }
        let original = &self.ingress.original;
        if let Err(cause) = prepay(original, Layout::new::<ReplyEntry>()) {
            return Err((cause, Some(command)));
        }
        let root = match ReplyRoot::new(original, command.kind()) {
            Ok(root) => root, Err(cause) => return Err((cause, Some(command))),
        };
        let (sender, receiver) = match root.begin() {
            Ok(pair) => pair, Err(cause) => return Err((cause, Some(command))),
        };
        // Node is already prepaid. The whole request can still be refused with
        // its original command, before any publication or driver dispatch.
        let mut entry = Box::new(ReplyEntry { root, next: None });
        let request = match Request::new(command, &entry.root) {
            Ok(request) => request, Err((cause, command)) => return Err((cause, Some(command))),
        };
        let mut pending = Some(request);
        {
            let mut ledger = match self.ingress.ledger.lock() {
                Ok(ledger) => ledger,
                Err(_) => {
                    return Err((OperationError::CreditOwned, pending.take().map(|request| request.command)));
                }
            };
            if ledger.ended || ledger.inflight || self.ingress.stop.load(Ordering::Acquire)
                || self.ingress.scope.is_closed()
            {
                drop(ledger);
                return Err((OperationError::CreditOwned, pending.take().map(|request| request.command)));
            }
            entry.next = ledger.roots.take();
            ledger.roots = Some(entry); // Root installed BEFORE sender/command publication.
            ledger.active = Some(sender);
            ledger.inflight = true;
        }
        // No ledger guard across Operation checks or Notify publication. A
        // concurrent task exit takes the sender; a queued request stays owned.
        if let Err(cause) = self.mailbox.try_send(&mut pending) {
            self.ingress.request_stop();
            return Err((cause, pending.take().map(|request| request.command)));
        }
        Ok(receiver)
    }

    async fn stop(&mut self) -> Result<ReplyBody, OperationError> {
        self.ingress.request_stop();
        let receiver = self.cleanup.take().ok_or(OperationError::CreditOwned)?;
        receiver.wait().await
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) { self.ingress.request_stop(); }
}

/// Checkout moves the concrete caller and cleanup result slots, not a mutex
/// guard, across await. Only this type restores the slot; concurrent checkout
/// sees None and refuses. Poison recovery is custody, never work permission.
struct CallerCheckout<C> { state: Arc<State<C>>, value: Option<CallerState<C>> }

impl<C> CallerCheckout<C> {
    fn take(state: &Arc<State<C>>) -> Result<Self, OperationError> {
        let mut stored = state.caller.lock().map_err(|_| OperationError::CreditOwned)?;
        let caller = stored.caller.take().ok_or(OperationError::CreditOwned)?;
        let value = CallerState {
            caller: Some(caller), cleanup: stored.cleanup.take(),
            cleanup_observation: stored.cleanup_observation.take(),
            undelivered: stored.undelivered.take(),
        };
        Ok(Self { state: state.clone(), value: Some(value) })
    }
}

impl<C> Drop for CallerCheckout<C> {
    fn drop(&mut self) {
        let Some(value) = self.value.take() else { return };
        let mut stored = match self.state.caller.lock() {
            Ok(stored) => stored,
            Err(cause) => {
                self.state.uncertain.store(true, Ordering::Release);
                cause.into_inner()
            }
        };
        // No other writer can restore a caller while this linear checkout
        // exists. Whole MOVE preserves nested owner/error/physical child roots.
        *stored = value;
    }
}

struct WorkCheckout { cell: Arc<ReplyCell>, value: Option<ReplyWork> }

impl WorkCheckout {
    fn take(request: Request) -> Result<Self, (OperationError, Request)> {
        let cell = request.reply.clone();
        let mut state = match cell.state.lock() {
            Ok(state) => state,
            Err(_) => return Err((OperationError::CreditOwned, request)),
        };
        if state.work.as_ref().is_none_or(|work| work.command.is_some()) {
            return Err((OperationError::CreditOwned, request));
        }
        let Some(mut work) = state.work.take() else {
            return Err((OperationError::CreditOwned, request));
        };
        work.command = Some(request.command);
        drop(state);
        Ok(Self { cell, value: Some(work) })
    }
}

impl Drop for WorkCheckout {
    fn drop(&mut self) {
        let Some(work) = self.value.take() else { return };
        let mut state = match self.cell.state.lock() {
            Ok(state) => state,
            Err(cause) => {
                let mut state = cause.into_inner();
                state.task_ended = true;
                state
            }
        };
        state.work = Some(work); // The ONLY restoration path, including unwind.
    }
}

fn unknown_reply(kind: CommandKind) -> ReplyBody {
    let cause = DriverError::Host(crate::host::HostRefusal::Unknown);
    match kind {
        CommandKind::Start => ReplyBody::Start(Err(cause)),
        CommandKind::Turn => ReplyBody::Turn(Err(cause)),
        CommandKind::Stop => ReplyBody::Stop(Err(cause)),
    }
}

impl<C: SeatDriver + 'static> Body<C> {
    pub(crate) async fn run(mut self) -> DriverExit {
        self.state.gate.wait().await;
        let mut exit = DriverExit::Stopped;
        loop {
            let received = tokio::select! {
                biased;
                _ = self.state.scope.closed() => break,
                _ = self.state.ingress.stopping() => break,
                received = self.receiver.recv() => received,
            };
            let request = match received {
                Ok(Some(request)) => request,
                Ok(None) => break,
                Err(_) => { exit = DriverExit::Unknown; break; }
            };
            let kind = request.command.kind();
            let mut work = match WorkCheckout::take(request) {
                Ok(work) => work,
                Err((_, request)) => {
                    // Restore refused command to its original reply work, even
                    // if that storage is poisoned. Do not execute it.
                    let mut stored = match self.state.refused_request.lock() {
                        Ok(stored) => stored, Err(cause) => cause.into_inner(),
                    };
                    *stored = Some(request);
                    exit = DriverExit::Unknown;
                    break;
                }
            };
            let mut caller = match CallerCheckout::take(&self.state) {
                Ok(caller) => caller,
                Err(_) => { exit = DriverExit::Unknown; break; }
            };
            let Some(value) = caller.value.as_mut() else { exit = DriverExit::Unknown; break };
            let Some(concrete) = value.caller.as_mut() else { exit = DriverExit::Unknown; break };
            let Some(frame) = work.value.as_mut() else { exit = DriverExit::Unknown; break };
            let original = &self.state.original;
            let future = match frame.command.as_ref() {
                Some(Command::Start { resume }) => concrete.start_retained(
                    original, resume.as_deref(), original, &mut frame.start),
                Some(Command::Turn { prompt, limit }) => concrete.turn_retained(
                    original, prompt, *limit, original, &mut frame.turn),
                Some(Command::Stop) => concrete.cleanup_retained(original, original, &mut frame.stop),
                None => Err(DriverError::Host(crate::host::HostRefusal::Unknown)),
            };
            let observation = match future {
                Err(cause) => Some(Err(cause)),
                Ok(future) => tokio::select! {
                    biased;
                    _ = self.state.scope.closed() => None,
                    _ = self.state.ingress.stopping() => None,
                    observation = future => Some(observation),
                },
            };
            frame.observation = observation;
            let body = match kind {
                CommandKind::Start => frame.start.take().map(ReplyBody::Start),
                CommandKind::Turn => frame.turn.take().map(ReplyBody::Turn),
                CommandKind::Stop => frame.stop.take().map(ReplyBody::Stop),
            }.unwrap_or_else(|| unknown_reply(kind));
            let interrupted = frame.observation.is_none();
            if !matches!(frame.observation, Some(Ok(true))) { exit = DriverExit::Unknown; }
            // Restore BOTH canonical records BEFORE a potentially synchronous
            // caller wake, including original future/observation failures.
            drop(work);
            drop(caller);
            let sender = {
                match self.state.ingress.ledger.lock() {
                    Ok(mut ledger) => ledger.active.take(),
                    Err(cause) => { exit = DriverExit::Unknown; cause.into_inner().active.take() }
                }
            };
            if let Some(sender) = sender {
                if sender.finish(body).is_err() { exit = DriverExit::Unknown; }
            } else {
                // A single body/active-sender pair is structural. Preserve the
                // whole output before reporting an invariant defect.
                let mut stored = match self.state.caller.lock() {
                    Ok(stored) => stored, Err(cause) => cause.into_inner(),
                };
                stored.undelivered = Some(body);
                self.state.uncertain.store(true, Ordering::Release);
                exit = DriverExit::Unknown;
            }
            if let Ok(mut ledger) = self.state.ingress.ledger.lock() { ledger.inflight = false; }
            if interrupted || exit == DriverExit::Unknown { break; }
        }
        // Cleanup uses the creation-prepaid reply/channel, not fresh ordinary
        // charge after expiration. Its exact returned cause stays in caller.
        if self.cleanup().await.is_err() { exit = DriverExit::Unknown; }
        if self.state.uncertain.load(Ordering::Acquire) { exit = DriverExit::Unknown; }
        exit
    }

    async fn cleanup(&self) -> Result<(), OperationError> {
        let sender = {
            let mut ledger = match self.state.ingress.ledger.lock() {
                Ok(ledger) => ledger,
                Err(cause) => {
                    self.state.uncertain.store(true, Ordering::Release);
                    cause.into_inner()
                }
            };
            ledger.cleanup.take().ok_or(OperationError::CreditOwned)?
        };
        let mut checkout = CallerCheckout::take(&self.state)?;
        let value = checkout.value.as_mut().ok_or(OperationError::CreditOwned)?;
        let caller = value.caller.as_mut().ok_or(OperationError::CreditOwned)?;
        let original = &self.state.original;
        let observation = match caller.cleanup_retained(original, original, &mut value.cleanup) {
            Ok(future) => future.await,
            Err(cause) => Err(cause),
        };
        value.cleanup_observation = Some(observation);
        let result = value.cleanup.take();
        let healthy = matches!(result, Some(Ok(())))
            && matches!(value.cleanup_observation, Some(Ok(true)));
        // The reply cell becomes the canonical body owner. Raw preparation or
        // observation errors remain in the original caller record above.
        drop(checkout);
        sender.finish(result.map(ReplyBody::Stop).unwrap_or_else(|| unknown_reply(CommandKind::Stop)))?;
        if healthy { Ok(()) } else { Err(OperationError::CreditOwned) }
    }
}

impl<C> Drop for Body<C> {
    fn drop(&mut self) { self.state.ingress.ended(); }
}

#[cfg(test)]
mod controls {
    use super::*;
    use crate::driver::{CliKind, RetainedDriverFuture};
    use crate::operation::Allowance;
    use std::sync::atomic::AtomicUsize;
    use tokio::runtime::Handle;
    use tokio::time::{Instant, timeout_at};

    struct Probe {
        entered: Notify, release: Notify, finished: Notify,
        calls: AtomicUsize, cleanup: AtomicUsize, drops: AtomicUsize,
    }

    impl Probe {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                entered: Notify::new(), release: Notify::new(), finished: Notify::new(),
                calls: AtomicUsize::new(0), cleanup: AtomicUsize::new(0), drops: AtomicUsize::new(0),
            })
        }
    }

    // Synthetic private caller, never a CLI/host-admission substitute. Probe
    // fixtures are external test bookkeeping, not an allocator-total witness.
    struct Fixture { probe: Arc<Probe>, payload: Option<SessionInfo> }

    impl Drop for Fixture {
        fn drop(&mut self) { self.probe.drops.fetch_add(1, Ordering::SeqCst); }
    }

    #[async_trait::async_trait]
    impl SeatDriver for Fixture {
        fn kind(&self) -> CliKind { CliKind::Claude }
        fn start_retained<'a>(
            &'a mut self, original: &'a Operation, _: Option<&'a str>,
            destination_original: &'a Operation,
            destination: &'a mut Option<Result<SessionInfo, DriverError>>,
        ) -> Result<RetainedDriverFuture<'a>, DriverError> {
            if !original.same_instance(destination_original) || destination.is_some() {
                return Err(DriverError::Host(crate::host::HostRefusal::Unknown));
            }
            let future = async move {
                self.probe.calls.fetch_add(1, Ordering::SeqCst);
                self.probe.entered.notify_one();
                self.probe.release.notified().await;
                *destination = self.payload.take().map(Ok);
                self.probe.finished.notify_one();
                Ok(destination.is_some()) // No await after whole result MOVE.
            };
            prepay(original, Layout::for_value(&future))
                .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))?;
            Ok(Box::pin(future))
        }
        fn cleanup_retained<'a>(
            &'a mut self, original: &'a Operation, destination_original: &'a Operation,
            destination: &'a mut Option<Result<(), DriverError>>,
        ) -> Result<RetainedDriverFuture<'a>, DriverError> {
            if !original.same_instance(destination_original) || destination.is_some() {
                return Err(DriverError::Host(crate::host::HostRefusal::Unknown));
            }
            let future = async move {
                self.probe.cleanup.fetch_add(1, Ordering::SeqCst);
                *destination = Some(Ok(()));
                Ok(true)
            };
            prepay(original, Layout::for_value(&future))
                .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))?;
            Ok(Box::pin(future))
        }
        async fn start(&mut self, _: Option<&str>) -> Result<SessionInfo, DriverError> {
            Err(DriverError::Host(crate::host::HostRefusal::Unknown))
        }
        async fn run_turn(&mut self, _: &str, _: Duration) -> Result<TurnOutcome, DriverError> {
            Err(DriverError::Host(crate::host::HostRefusal::Unknown))
        }
        fn session_id(&self) -> Option<String> { None }
        fn is_alive(&mut self) -> bool { false }
        async fn stop(&mut self) -> Result<(), DriverError> {
            Err(DriverError::Host(crate::host::HostRefusal::Unknown))
        }
    }

    fn fixture(original: &Operation, probe: Arc<Probe>) -> Fixture {
        prepay(original, Layout::array::<u8>(10).unwrap()).unwrap();
        Fixture { probe, payload: Some(SessionInfo {
            session_id: String::from("fixture-id"), model: None, cli_version: None, resumed: false,
        }) }
    }

    fn operation() -> Operation {
        Operation::new(String::from("actor-source-control"), 1,
            Allowance::required(Some(Instant::now() + Duration::from_secs(60))).unwrap()).unwrap()
    }

    // All producer/observer futures below run on the SAME multi-thread Tokio
    // runtime. No custom/noop Waker, foreign executor or stdio worker is added.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn publication_receiver_drop_and_join_keep_canonical_body_and_no_refund() {
        Handle::current().spawn(async {
            let original = operation();
            let probe = Probe::new();
            let scope = ScopeControl::source_control();
            let mut pending = Some(fixture(&original, probe.clone()));
            let pointer = pending.as_ref().unwrap().payload.as_ref().unwrap().session_id.as_ptr() as usize;
            let before = original.captured_bytes();
            let Prepared { root, body, mut endpoint } = prepare_inner(&original, scope, &mut pending).unwrap();
            assert!(pending.is_none());
            let exact = own_arc_layout::<Ingress>().unwrap().size()
                + own_arc_layout::<State<Fixture>>().unwrap().size()
                + own_arc_layout::<MailboxCell>().unwrap().size()
                + own_arc_layout::<ReplyCell>().unwrap().size()
                + Layout::new::<Option<ReplySignal>>().size();
            assert_eq!(original.captured_bytes() - before, exact);
            let reply = endpoint.submit(Command::start(&original, None).unwrap())
                .unwrap_or_else(|_| panic!("source request refused"));
            let cell = {
                let ledger = root.state.ingress.ledger.lock().unwrap();
                ledger.roots.as_ref().unwrap().root.0.clone()
            };
            assert!(Arc::ptr_eq(&reply.cell, &cell));
            assert!(root.state.original.same_instance(&original));
            assert!(root.mailbox.0.original.same_instance(&original));
            let future = body.run();
            prepay(&original, Layout::for_value(&future)).unwrap();
            prepay(&original, Layout::new::<Option<DriverExit>>()).unwrap();
            let mut handle = Handle::current().spawn(future);
            assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
            assert_eq!(probe.drops.load(Ordering::SeqCst), 0);
            root.publish(); // Root + actual handle are already retained above.
            timeout_at(original.allowance().deadline(), probe.entered.notified()).await.unwrap();
            drop(reply);
            let charged = original.captured_bytes();
            assert!(cell.state.lock().unwrap().receiver_dropped);
            probe.release.notify_one();
            timeout_at(original.allowance().deadline(), probe.finished.notified()).await.unwrap();
            drop(endpoint); // Only requests stop; never takes caller/join/reply.
            let mut returned = None;
            let output = timeout_at(original.allowance().deadline(), &mut handle).await.unwrap();
            returned.replace(output); // FIRST whole output MOVE before observation.
            assert_eq!(*returned.as_ref().unwrap().as_ref().unwrap(), DriverExit::Stopped);
            let state = cell.state.lock().unwrap();
            let Some(ReplyBody::Start(Ok(info))) = state.body.as_ref() else { panic!("missing canonical body") };
            assert_eq!(info.session_id.as_ptr() as usize, pointer);
            assert!(state.work.as_ref().unwrap().command.is_some());
            assert!(matches!(state.work.as_ref().unwrap().observation, Some(Ok(true))));
            assert!(!state.accepted);
            drop(state);
            assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
            assert_eq!(probe.cleanup.load(Ordering::SeqCst), 1);
            assert_eq!(probe.drops.load(Ordering::SeqCst), 0);
            assert!(original.captured_bytes() >= charged); // No ACK/drop/join refund.
            let final_charge = original.captured_bytes();
            drop(root);
            assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
            assert_eq!(original.captured_bytes(), final_charge);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scope_cancellation_restores_checked_out_caller_and_request_before_reply() {
        Handle::current().spawn(async {
            let original = operation();
            let probe = Probe::new();
            let scope = ScopeControl::source_control();
            let mut pending = Some(fixture(&original, probe.clone()));
            let pointer = pending.as_ref().unwrap().payload.as_ref().unwrap().session_id.as_ptr() as usize;
            let Prepared { root, body, mut endpoint } = prepare_inner(&original, scope.clone(), &mut pending).unwrap();
            let reply = endpoint.submit(Command::start(&original, None).unwrap())
                .unwrap_or_else(|_| panic!("source request refused"));
            let future = body.run();
            prepay(&original, Layout::for_value(&future)).unwrap();
            prepay(&original, Layout::new::<Option<DriverExit>>()).unwrap();
            let mut handle = Handle::current().spawn(future);
            root.publish();
            timeout_at(original.allowance().deadline(), probe.entered.notified()).await.unwrap();
            assert!(root.state.caller.try_lock().unwrap().caller.is_none());
            scope.request_shutdown(Instant::now());
            let result = timeout_at(original.allowance().deadline(), reply.wait()).await.unwrap().unwrap();
            assert!(matches!(result, ReplyBody::Start(Err(DriverError::Host(_)))));
            // The request is restored before signalling. The caller may already
            // be checked out AGAIN for the separately retained cleanup, so its
            // final pointer identity is observed only after the actual join.
            {
                let ledger = root.state.ingress.ledger.lock().unwrap();
                let state = ledger.roots.as_ref().unwrap().root.0.state.lock().unwrap();
                let work = state.work.as_ref().unwrap();
                assert!(work.command.is_some());
                assert!(work.observation.is_none());
            }
            let output = timeout_at(original.allowance().deadline(), &mut handle).await.unwrap();
            let returned = Some(output);
            assert_eq!(*returned.as_ref().unwrap().as_ref().unwrap(), DriverExit::Unknown);
            {
                let stored = root.state.caller.try_lock().unwrap();
                let caller = stored.caller.as_ref().unwrap();
                assert_eq!(caller.payload.as_ref().unwrap().session_id.as_ptr() as usize, pointer);
            }
            assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
            assert_eq!(probe.cleanup.load(Ordering::SeqCst), 1);
            assert_eq!(probe.drops.load(Ordering::SeqCst), 0);
            drop(endpoint);
            drop(root);
            assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unpolled_task_body_exit_refuses_commands_and_fails_cleanup_without_hanging() {
        Handle::current().spawn(async {
            let original = operation();
            let probe = Probe::new();
            let mut pending = Some(fixture(&original, probe.clone()));
            let Prepared { root, body, mut endpoint } = prepare_inner(
                &original, ScopeControl::source_control(), &mut pending).unwrap();
            drop(body); // No task/child was started; canonical caller survives.
            let refused = endpoint.submit(Command::start(&original, None).unwrap());
            assert!(matches!(refused, Err((OperationError::CreditOwned, Some(Command::Start { .. })))));
            assert!(timeout_at(original.allowance().deadline(), endpoint.stop()).await.unwrap().is_err());
            assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
            assert_eq!(probe.drops.load(Ordering::SeqCst), 0);
            assert!(root.state.caller.lock().unwrap().caller.is_some());
            drop(endpoint);
            drop(root);
            assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn checkout_unwind_restores_caller_before_canonical_join_error_observation() {
        Handle::current().spawn(async {
            let original = operation();
            let probe = Probe::new();
            let mut pending = Some(fixture(&original, probe.clone()));
            let pointer = pending.as_ref().unwrap().payload.as_ref().unwrap().session_id.as_ptr() as usize;
            let Prepared { root, body, endpoint } = prepare_inner(
                &original, ScopeControl::source_control(), &mut pending).unwrap();
            let state = root.state.clone();
            let future = async move {
                let _checkout = CallerCheckout::take(&state).unwrap();
                // Explicit synthetic task defect. This does not authorize an
                // ordinary worker panic, production hook or catch/retry path.
                std::panic::panic_any("synthetic caller checkout defect");
            };
            prepay(&original, Layout::for_value(&future)).unwrap();
            prepay(&original, Layout::new::<Option<()>>()).unwrap();
            let mut handle = Handle::current().spawn(future);
            let output = timeout_at(original.allowance().deadline(), &mut handle).await.unwrap();
            let returned = Some(output);
            assert!(returned.as_ref().unwrap().as_ref().unwrap_err().is_panic());
            assert_eq!(probe.drops.load(Ordering::SeqCst), 0);
            {
                let stored = root.state.caller.try_lock().unwrap();
                let caller = stored.caller.as_ref().unwrap();
                assert_eq!(caller.payload.as_ref().unwrap().session_id.as_ptr() as usize, pointer);
            }
            drop(body);
            drop(endpoint);
            drop(root);
            assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_preparation_preserves_original_caller_and_consumed_charge() {
        Handle::current().spawn(async {
            let original = operation();
            let probe = Probe::new();
            let mut pending = Some(fixture(&original, probe.clone()));
            let pointer = pending.as_ref().unwrap().payload.as_ref().unwrap().session_id.as_ptr() as usize;
            let first = own_arc_layout::<Ingress>().unwrap().size();
            original.charge(crate::operation::OPERATION_BYTES - original.captured_bytes() - first).unwrap();
            assert!(matches!(prepare_inner(
                &original, ScopeControl::source_control(), &mut pending),
                Err(OperationError::CaptureOverflow)));
            assert_eq!(original.captured_bytes(), crate::operation::OPERATION_BYTES);
            assert_eq!(pending.as_ref().unwrap().payload.as_ref().unwrap().session_id.as_ptr() as usize, pointer);
            assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
            assert_eq!(probe.cleanup.load(Ordering::SeqCst), 0);
            assert_eq!(probe.drops.load(Ordering::SeqCst), 0);
            drop(pending);
            assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
            assert_eq!(original.captured_bytes(), crate::operation::OPERATION_BYTES);
        }).await.unwrap();
    }
}
