//! Private synchronous ingress only. The process Host owns the strong Registrar
//! and drains its entries before borrowed joins; leases carry only Weak.

use std::alloc::Layout;

use super::*;
use crate::driver::owned_driver_task::{self, CommandEndpoint, DriverExit, OwnedCaller};
use crate::operation::{Operation, OperationError};

pub(super) struct Registrar {
    runtime: Handle,
    owner: RegistryOwner,
    state: Mutex<RegistrationState>,
    changed: Notify,
    quiescence_timeout: std::sync::OnceLock<tokio::time::error::Elapsed>,
}

enum RegistryOwner {
    Host(original::HostRef),
    #[cfg(test)] Fixture,
}

struct RegistrationState {
    closed: bool,
    uncertain: bool,
    pending: Option<Box<DriverEntry>>,
    identities: Option<Box<IdentityEntry>>,
    active: usize,
    admission_refusal: Option<OperationError>,
}

struct Attempt<'a> { registrar: &'a Registrar, scope: &'a ScopeControl, finished: bool }

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        let mut state = match self.registrar.state.lock() {
            Ok(state) => state,
            Err(cause) => {
                let mut state = cause.into_inner();
                state.closed = true;
                state.uncertain = true;
                state
            }
        };
        match state.active.checked_sub(1) {
            Some(active) => state.active = active,
            None => state.uncertain = true,
        }
        match self.scope.registration_active.load(Ordering::Acquire).checked_sub(1) {
            Some(active) => self.scope.registration_active.store(active, Ordering::Release),
            None => {
                state.uncertain = true;
                self.scope.uncertain.store(true, Ordering::Release);
            }
        }
        if !self.finished {
            state.closed = true;
            state.uncertain = true;
            self.scope.uncertain.store(true, Ordering::Release);
        }
        drop(state);
        self.registrar.changed.notify_waiters(); // No guard survives Host wake.
    }
}

/// This small identity stays indexed after its driver entry is drained, so a
/// second factory cannot mint another task for the SAME Operation. No task,
/// caller or entry reference points back to the Registrar.
struct IdentityEntry { original: Operation, next: Option<Box<IdentityEntry>> }

impl Drop for IdentityEntry {
    fn drop(&mut self) {
        // Detached driver originals keep their creator's charge; destroying
        // this index is neither a refund nor evidence that a task was joined.
        let mut next = self.next.take();
        while let Some(mut entry) = next { next = entry.next.take(); }
    }
}

impl RegistrationState {
    fn contains(&self, original: &Operation) -> bool {
        let mut entry = self.identities.as_deref();
        while let Some(identity) = entry {
            if identity.original.same_instance(original) { return true; }
            entry = identity.next.as_deref();
        }
        false
    }
}

impl Registrar {
    /// Type-only contribution to the still-unfinished complete creation fit.
    /// It does not create a meter/Operation, accept a numeric budget or allocate.
    pub(super) fn creation_layout() -> Result<Layout, OperationError> {
        owned_driver_task::own_arc_layout::<Self>()
    }

    pub(super) fn owned_by(&self, owner: &original::HostRef) -> bool {
        match &self.owner {
            RegistryOwner::Host(actual) => actual.same_instance(owner),
            #[cfg(test)] RegistryOwner::Fixture => false,
        }
    }

    /// Fixed Host-kept nodes only. This is NOT the complete per-seat Host cost
    /// (attempt/member/evidence/job/terminal slots are still owed).
    pub(super) fn retained_entry_bytes() -> Result<usize, OperationError> {
        Layout::new::<DriverEntry>().size().checked_add(Layout::new::<IdentityEntry>().size())
            .ok_or(OperationError::CaptureOverflow)
    }

    /// Pure inline value. The durable lease producer MUST prepay the actual Arc
    /// layout as part of full original F/G/N before allocating this once. That
    /// producer/journal fit is not connected yet; Host::new stays allocation-free.
    pub(super) fn new_owned(runtime: Handle, owner: original::HostRef) -> Self {
        Self { runtime, owner: RegistryOwner::Host(owner), changed: Notify::new(),
            quiescence_timeout: std::sync::OnceLock::new(), state: Mutex::new(RegistrationState {
            closed: false, uncertain: false, pending: None, identities: None, active: 0, admission_refusal: None,
        }) }
    }

    #[cfg(test)]
    pub(super) fn new(runtime: Handle) -> Self {
        Self { runtime, owner: RegistryOwner::Fixture, changed: Notify::new(),
            quiescence_timeout: std::sync::OnceLock::new(), state: Mutex::new(RegistrationState {
            closed: false, uncertain: false, pending: None, identities: None, active: 0, admission_refusal: None,
        }) }
    }

    fn prepay_retained_entries(&self, driver_fixture: &Operation) -> Result<(), OperationError> {
        #[cfg(not(test))]
        let _ = driver_fixture;
        let result = match &self.owner {
            RegistryOwner::Host(host) => original::prepay(host, Layout::new::<DriverEntry>())
                .and_then(|()| original::prepay(host, Layout::new::<IdentityEntry>())),
            #[cfg(test)]
            RegistryOwner::Fixture => owned_driver_task::prepay(driver_fixture, Layout::new::<DriverEntry>())
                .and_then(|()| owned_driver_task::prepay(driver_fixture, Layout::new::<IdentityEntry>())),
        };
        if let Err(error) = &result {
            // No original check/charge or wake occurs while this guard lives.
            // A known pre-effect refusal closes admission, not false UNKNOWN.
            if let Ok(mut state) = self.state.lock() {
                state.closed = true;
                if matches!(&self.owner, RegistryOwner::Host(_)) && *error == OperationError::CreditOwned {
                    state.uncertain = true; // Host meter mutex refusal, not known budget exhaustion.
                }
                if state.admission_refusal.is_none() { state.admission_refusal = Some(*error); }
            }
        }
        result
    }

    pub(super) fn admission_failure(&self) -> Option<OperationError> {
        match self.state.lock() {
            Ok(state) => state.admission_refusal,
            Err(_) => Some(OperationError::CreditOwned),
        }
    }

    pub(super) fn register(
        &self,
        scope: &Arc<ScopeControl>,
        original: &Operation,
        pending: &mut Option<OwnedCaller>,
    ) -> Result<CommandEndpoint, OperationError> {
        if scope.is_closed() || !scope.durable_open.load(Ordering::Acquire) {
            return Err(OperationError::CreditOwned);
        }
        {
            let mut state = self.state.lock().map_err(|_| OperationError::CreditOwned)?;
            if let Some(error) = &state.admission_refusal { return Err(*error); }
            if state.closed || state.contains(original) || scope.is_closed()
                || !scope.durable_open.load(Ordering::Acquire)
            { return Err(OperationError::CreditOwned); }
            let active = state.active.checked_add(1).ok_or(OperationError::CaptureOverflow)?;
            let scope_active = scope.registration_active.load(Ordering::Acquire)
                .checked_add(1).ok_or(OperationError::CaptureOverflow)?;
            state.active = active;
            scope.registration_active.store(scope_active, Ordering::Release);
        }
        let mut attempt = Attempt { registrar: self, scope, finished: false };
        let result = self.register_attempt(scope, original, pending);
        attempt.finished = true;
        drop(attempt); // Counter settles BEFORE returning any endpoint/refusal.
        result
    }

    fn register_attempt(
        &self, scope: &Arc<ScopeControl>, original: &Operation,
        pending: &mut Option<OwnedCaller>,
    ) -> Result<CommandEndpoint, OperationError> {
        // No registry guard around an Operation check/charge or potential wake.
        // Refusal here leaves the original caller in its incoming owner slot.
        self.prepay_retained_entries(original)?;
        let prepared = owned_driver_task::prepare(original, scope.clone(), pending)?;
        let publish = prepared.root.publication();
        let mut entry = Box::new(DriverEntry {
            root: prepared.root, task: task::RetainedTask::empty(),
            registration_refusal: None, scope: scope.clone(),
            original: original.clone(), next: None,
        });
        let mut identity = Box::new(IdentityEntry { original: original.clone(), next: None });
        let body = prepared.body.run();
        let admitted = owned_driver_task::prepay(original, Layout::for_value(&body))
            .and_then(|()| owned_driver_task::prepay(original, Layout::new::<Option<DriverExit>>()));
        let mut body = Some(body);
        let refusal = {
            // Even closed/duplicate/poison refusal retains the prepared caller
            // in this Host-owned queue BEFORE its fixed error observation.
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(cause) => {
                    let mut state = cause.into_inner();
                    state.closed = true;
                    state.uncertain = true;
                    state
                }
            };
            let refusal = admitted.err().or_else(|| {
                if state.closed || state.contains(original) || scope.is_closed()
                    || !scope.durable_open.load(Ordering::Acquire)
                { Some(OperationError::CreditOwned) } else { None }
            });
            entry.registration_refusal = refusal; // Copy fixed metadata; no body allocation.
            if refusal.is_none() {
                identity.next = state.identities.take();
                state.identities = Some(identity);
            }
            entry.next = state.pending.take();
            state.pending = Some(entry); // Actual roots FIRST, never facade custody.
            match (refusal, state.pending.as_mut(), body.take()) {
                (None, Some(entry), Some(future)) => {
                    // Closed publication gate permits ONLY its own runtime-task
                    // waiter. No caller callback or worker can run under lock.
                    match entry.task.spawn_closed(&self.runtime, future) {
                        Ok(()) => None,
                        Err(future) => {
                            body = Some(future);
                            entry.registration_refusal = Some(OperationError::CreditOwned);
                            Some(OperationError::CreditOwned)
                        }
                    }
                }
                (cause, _, future) => {
                    body = future;
                    cause.or(Some(OperationError::CreditOwned))
                }
            }
        };
        // Dropping an unstarted body can fail replies. Publication can wake a
        // driver task. BOTH occur only after ALL registry exclusions end.
        drop(body);
        if let Some(cause) = refusal { return Err(cause); }
        publish(); // The actual handle and roots are already in the Host queue.
        Ok(prepared.endpoint)
    }

    /// Synchronous whole-owner transfer into the original process Host. This
    /// function never observes/moves a returned body or waits on a handle.
    pub(super) fn drain(
        &self, close: bool, scope: Option<&ScopeControl>,
    ) -> (Option<Box<DriverEntry>>, bool, usize) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(cause) => {
                let mut state = cause.into_inner();
                state.closed = true;
                state.uncertain = true;
                state
            }
        };
        state.closed |= close;
        let active = scope.map_or(state.active, |scope| scope.registration_active.load(Ordering::Acquire));
        (state.pending.take(), state.uncertain, active)
    }

    /// Only the process owner's shutdown body waits here. Factory completion
    /// must include every scope when the process owner closes globally. The
    /// global count is read under the SAME original registry lock as admission;
    /// no copied list of current scopes can omit a late already-begun attempt.
    pub(super) async fn quiesce_all(&self, deadline: Instant) -> bool {
        loop {
            if self.quiescence_timeout.get().is_some() { return false; }
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let settled = match self.state.lock() {
                Ok(state) => {
                    if !state.closed || state.uncertain { return false; }
                    state.active == 0
                }
                Err(_) => return false,
            };
            if settled { return true; }
            if let Err(original) = tokio::time::timeout_at(deadline, changed).await {
                // FIRST raw timeout is in the process owner before refusal.
                // The closure only moves the actual error; no wake or retry.
                let _ = self.quiescence_timeout.get_or_init(|| original);
                return false;
            }
        }
    }

    /// Only the process owner's shutdown body waits here. Factory completion
    /// notifies it after releasing exclusions; stdio workers never use this.
    pub(super) async fn quiesce(&self, scope: &ScopeControl, deadline: Instant) -> bool {
        loop {
            if scope.registration_timeout.get().is_some() { return false; }
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let settled = {
                match self.state.lock() {
                    Ok(state) => {
                        if state.uncertain || scope.uncertain.load(Ordering::Acquire) { return false; }
                        scope.registration_active.load(Ordering::Acquire) == 0
                    }
                    Err(_) => return false,
                }
            };
            if settled { return true; }
            if let Err(original) = tokio::time::timeout_at(deadline, changed).await {
                // This selected scope owns its error, not another scope's or
                // the global boundary's slot. Never replace the first cause.
                let _ = scope.registration_timeout.get_or_init(|| original);
                return false;
            }
        }
    }

    pub(super) fn quiescence_timeout(&self) -> Option<&tokio::time::error::Elapsed> {
        self.quiescence_timeout.get()
    }
}

#[cfg(test)]
mod controls {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_quiescence_keeps_first_real_timeouts_after_actual_factory_finishes() {
        Handle::current().spawn(async {
            let registrar = Arc::new(Registrar::new(Handle::current()));
            let scope = ScopeControl::source_control();
            let other = ScopeControl::source_control();
            // Partial source fixture only: an actual linear Attempt owns the
            // manually seeded already-begun count. No driver/lease admission.
            registrar.state.lock().unwrap().active = 1;
            scope.registration_active.store(1, Ordering::Release);
            let root = registrar.clone(); let active = scope.clone();
            let (finish, finished) = tokio::sync::oneshot::channel();
            let (entered, entering) = tokio::sync::oneshot::channel();
            let mut factory = Handle::current().spawn(async move {
                let mut attempt = Attempt { registrar: &root, scope: &active, finished: false };
                entered.send(()).unwrap(); finished.await.unwrap(); attempt.finished = true;
            });
            entering.await.unwrap();
            let first = Instant::now(); let deadline = first + Duration::from_secs(30);
            {
                let observation = registrar.quiesce(&scope, deadline);
                tokio::pin!(observation);
                poll_fn(|cx| { assert!(observation.as_mut().poll(cx).is_pending()); Poll::Ready(()) }).await;
            }
            assert!(scope.registration_timeout.get().is_none());
            assert!(registrar.quiescence_timeout().is_none());
            let expired = first - Duration::from_secs(1);
            assert!(!registrar.quiesce(&scope, expired).await);
            assert!(scope.registration_timeout.get().is_some());
            assert!(registrar.quiescence_timeout().is_none());
            assert!(registrar.quiesce(&other, deadline).await);
            let (_, uncertain, active_count) = registrar.drain(true, None);
            assert!(!uncertain);
            assert_eq!(active_count, 1);
            assert!(!registrar.quiesce_all(expired).await);
            assert!(registrar.quiescence_timeout().is_some());
            finish.send(()).unwrap();
            let returned = Some(tokio::time::timeout_at(deadline, &mut factory).await.unwrap());
            returned.as_ref().unwrap().as_ref().unwrap(); // Actual fixture task joined.
            assert_eq!(registrar.state.lock().unwrap().active, 0);
            assert_eq!(scope.registration_active.load(Ordering::Acquire), 0);
            // Neither a later completion nor a longer request erases a timeout.
            assert!(!registrar.quiesce(&scope, deadline).await);
            assert!(!registrar.quiesce_all(deadline).await);
            assert!(registrar.quiesce(&other, deadline).await);
            let mut host = SeatHost::new(Handle::current());
            host.registration = Some(registrar.clone());
            host.scopes = Some(Box::new(ScopeEntry { control: scope.clone(), next: None }));
            let lease = SeatHostLease { control: scope, registration: Arc::downgrade(&registrar) };
            assert_eq!(host.observe_scope(&lease, first, deadline).await, task::TaskObservation::Unknown);
            assert_eq!(host.observe_global(first, deadline).await, task::TaskObservation::Unknown);
            let view = host.closed_readback().unwrap();
            assert!(view.registration_timeout().is_some());
            assert!(view.scopes().next().unwrap().registration_timeout().is_some());
            assert!(view.boundary().unwrap().global_unknown);
            // Borrowed retained causes only; not full fit, frozen writer census,
            // native child settlement, injected OS failure or CLEAN evidence.
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn long_scope_and_identity_fixture_lists_drop_without_refund_or_closing_aliases() {
        Handle::current().spawn(async {
            let original = Operation::new(String::from("list-drop-fixture"), 1,
                crate::operation::Allowance::required(Some(Instant::now()
                    + std::time::Duration::from_secs(30))).unwrap()).unwrap();
            let control = ScopeControl::source_control();
            let mut host = SeatHost::new(Handle::current());
            let mut identities = None;
            for _ in 0..8192 {
                // Deliberate duplicate-reference fixture, not registry ingress
                // or production scope admission. Creator/meter is unchanged.
                original.charge(Layout::new::<ScopeEntry>().size()).unwrap();
                host.scopes = Some(Box::new(ScopeEntry { control: control.clone(), next: host.scopes.take() }));
                original.charge(Layout::new::<IdentityEntry>().size()).unwrap();
                identities = Some(Box::new(IdentityEntry { original: original.clone(), next: identities }));
            }
            assert_eq!(Arc::strong_count(&control), 8193);
            let charged = original.captured_bytes();
            drop((host, identities));
            assert_eq!(Arc::strong_count(&control), 1);
            assert!(!control.is_closed());
            assert_eq!(original.captured_bytes(), charged);
            original.check().unwrap();
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn closed_report_refuses_actual_active_attempt_until_original_guard_settles_without_deadline_renewal() {
        Handle::current().spawn(async {
            let registrar = Arc::new(Registrar::new(Handle::current()));
            let scope = ScopeControl::source_control();
            // Already-begun factory fixture, with completion owned by the real
            // linear Attempt guard below. No production registration is minted.
            registrar.state.lock().unwrap().active = 1;
            scope.registration_active.store(1, Ordering::Release);
            let root = registrar.clone(); let active = scope.clone();
            let (release, released) = tokio::sync::oneshot::channel();
            let (entered, entering) = tokio::sync::oneshot::channel();
            let mut factory = Handle::current().spawn(async move {
                let mut attempt = Attempt { registrar: &root, scope: &active, finished: false };
                entered.send(()).unwrap(); released.await.unwrap(); attempt.finished = true;
            });
            entering.await.unwrap();
            let mut host = SeatHost::new(Handle::current());
            host.registration = Some(registrar.clone());
            host.scopes = Some(Box::new(ScopeEntry { control: scope.clone(), next: None }));
            let first = Instant::now(); let deadline = first + std::time::Duration::from_secs(30);
            assert_eq!(host.close_global(first, deadline), Some(deadline));
            assert!(matches!(host.closed_readback(), Err(report::ReadbackRefusal::ActiveRegistrations)));
            assert!(host.report_boundary.is_none());
            assert_eq!(registrar.state.lock().unwrap().active, 1);
            release.send(()).unwrap();
            let returned = Some(tokio::time::timeout_at(deadline, &mut factory).await.unwrap());
            returned.as_ref().unwrap().as_ref().unwrap();
            let view = host.closed_readback().unwrap();
            assert_eq!(view.boundary().unwrap().first_requested, first);
            assert_eq!(view.boundary().unwrap().deadline, deadline);
            assert!(!view.boundary().unwrap().registration_uncertain);
            assert!(view.drivers().next().is_none());
            assert_eq!(view.scopes().next().unwrap().boundary().unwrap().deadline, deadline);
            // Zero factories/empty drivers remains closure readback only, not
            // complete member/writer/child settlement or permission for CLEAN.
        }).await.unwrap();
    }
    use crate::operation::Allowance;
    use std::future::{Future, poll_fn};
    use std::task::Poll;
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn global_quiescence_keeps_all_scope_attempts_and_original_close_across_cancel() {
        Handle::current().spawn(async {
            let registrar = Arc::new(Registrar::new(Handle::current()));
            let scope = ScopeControl::source_control();
            let deadline = Instant::now() + Duration::from_secs(30);
            assert!(!registrar.quiesce_all(deadline).await); // Open is not closed-empty.
            registrar.state.lock().unwrap().active = 1;
            scope.registration_active.store(1, Ordering::Release);
            let root = registrar.clone();
            let active = scope.clone();
            let (release, released) = tokio::sync::oneshot::channel();
            let (entered, entering) = tokio::sync::oneshot::channel();
            let mut factory = Handle::current().spawn(async move {
                let mut attempt = Attempt { registrar: &root, scope: &active, finished: false };
                entered.send(()).unwrap();
                released.await.unwrap();
                attempt.finished = true;
            });
            entering.await.unwrap();
            let (_, uncertain, count) = registrar.drain(true, None);
            assert!(!uncertain); assert_eq!(count, 1);
            {
                let wait = registrar.quiesce_all(deadline);
                tokio::pin!(wait);
                poll_fn(|cx| {
                    assert!(wait.as_mut().poll(cx).is_pending()); Poll::Ready(())
                }).await;
            }
            assert_eq!(registrar.state.lock().unwrap().active, 1);
            assert_eq!(scope.registration_active.load(Ordering::Acquire), 1);
            assert!(registrar.state.lock().unwrap().closed);
            release.send(()).unwrap();
            let returned = Some(tokio::time::timeout_at(deadline, &mut factory).await.unwrap());
            returned.as_ref().unwrap().as_ref().unwrap();
            assert!(registrar.quiesce_all(deadline).await);
            assert_eq!(registrar.state.lock().unwrap().active, 0);
            assert!(registrar.state.lock().unwrap().closed);
        }).await.unwrap();
    }

    // Registry/lease fixtures only, never production OPEN/fit/job admission.
    // Each probe runs on the same multi-thread runtime as its completion task.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn weak_scope_aliases_cannot_keep_process_registration_owner_alive() {
        Handle::current().spawn(async {
            let mut host = SeatHost::new(Handle::current());
            assert!(host.registration.is_none());
            // External fixture storage, NOT a production prepayment/fit claim.
            host.registration = Some(Arc::new(Registrar::new(host.runtime.clone())));
            let weak = Arc::downgrade(host.registration.as_ref().unwrap());
            let lease = SeatHostLease {
                control: ScopeControl::source_control(), registration: weak.clone(),
            };
            let alias = lease.clone();
            assert_eq!(weak.strong_count(), 1);
            drop(host);
            assert!(weak.upgrade().is_none());
            assert!(lease.registration.upgrade().is_none());
            assert!(alias.registration.upgrade().is_none());
            let original = Operation::new(String::from("stale-lease-source"), 1,
                Allowance::required(Some(Instant::now() + Duration::from_secs(30))).unwrap()).unwrap();
            let charged = original.captured_bytes();
            let mut caller: Option<OwnedCaller> = None;
            assert!(matches!(lease.register_driver(&original, &mut caller), Err(OperationError::CreditOwned)));
            assert_eq!(original.captured_bytes(), charged); // No fallback/meter/task creation.
            assert!(caller.is_none());
            assert_eq!(alias.request_shutdown(), ScopeObservation::HostJoinPending);
            assert!(lease.control.is_closed());
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn selected_scope_quiescence_ignores_other_scope_and_cancel_keeps_actual_attempt() {
        Handle::current().spawn(async {
            let registrar = Arc::new(Registrar::new(Handle::current()));
            let empty = ScopeControl::source_control();
            let active = ScopeControl::source_control();
            // One already-begun factory fixture. Its actual linear guard below
            // owns completion; no game/driver/caller/task registration is minted.
            registrar.state.lock().unwrap().active = 1;
            active.registration_active.store(1, Ordering::Release);
            let root = registrar.clone();
            let scope = active.clone();
            let (finish, finished) = tokio::sync::oneshot::channel();
            let (entered, entry) = tokio::sync::oneshot::channel();
            let mut factory = Handle::current().spawn(async move {
                let mut attempt = Attempt { registrar: &root, scope: &scope, finished: false };
                entered.send(()).unwrap();
                finished.await.unwrap();
                attempt.finished = true;
            });
            entry.await.unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            assert!(registrar.quiesce(&empty, deadline).await);
            {
                let observation = registrar.quiesce(&active, deadline);
                tokio::pin!(observation);
                poll_fn(|cx| {
                    assert!(observation.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                }).await;
            } // Observer cancellation does not finish/drop the real attempt.
            assert_eq!(active.registration_active.load(Ordering::Acquire), 1);
            assert_eq!(registrar.state.lock().unwrap().active, 1);
            finish.send(()).unwrap();
            let output = tokio::time::timeout_at(deadline, &mut factory).await.unwrap();
            let returned = Some(output); // FIRST retain actual task Result.
            returned.as_ref().unwrap().as_ref().unwrap();
            assert!(registrar.quiesce(&active, deadline).await);
            let (pending, uncertain, active_count) = registrar.drain(true, None);
            assert!(pending.is_none());
            assert!(!uncertain);
            assert_eq!(active_count, 0);
            assert!(registrar.state.lock().unwrap().closed);
            assert_eq!(active.registration_active.load(Ordering::Acquire), 0);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unfinished_attempt_unwind_retains_unknown_after_counter_reaches_zero() {
        Handle::current().spawn(async {
            let registrar = Arc::new(Registrar::new(Handle::current()));
            let scope = ScopeControl::source_control();
            registrar.state.lock().unwrap().active = 1;
            scope.registration_active.store(1, Ordering::Release);
            let root = registrar.clone();
            let owned_scope = scope.clone();
            let mut factory = Handle::current().spawn(async move {
                let _attempt = Attempt { registrar: &root, scope: &owned_scope, finished: false };
                // Explicit synthetic factory defect, not ordinary worker policy.
                std::panic::panic_any("synthetic registration attempt defect");
            });
            let deadline = Instant::now() + Duration::from_secs(30);
            let output = tokio::time::timeout_at(deadline, &mut factory).await.unwrap();
            let returned = Some(output);
            assert!(returned.as_ref().unwrap().as_ref().unwrap_err().is_panic());
            assert!(!registrar.quiesce(&scope, deadline).await);
            let (pending, uncertain, active) = registrar.drain(true, Some(&scope));
            assert!(pending.is_none());
            assert!(uncertain);
            assert_eq!(active, 0); // Zero active is NEVER completion proof alone.
            assert!(scope.uncertain.load(Ordering::Acquire));
            assert!(registrar.state.lock().unwrap().closed);
        }).await.unwrap();
    }
}
