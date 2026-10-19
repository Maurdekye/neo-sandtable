//! Private borrowed closure facts, not a frozen writer set or CLEAN report.
//! No public report, finalization capability, task helper or executor is added.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadbackRefusal { AdmissionOpen, NoGlobalBoundary, ActiveRegistrations }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReportBoundary {
    pub(super) first_requested: Instant,
    pub(super) deadline: Instant,
    pub(super) registration_uncertain: bool,
    pub(super) global_unknown: bool,
}

/// This shared borrow prevents owner dispatch/drain/join/result mutation while
/// inspected. Closed admission plus zero active factories makes the entry list
/// stable; actual task completion still needs its canonical owner observation.
pub(super) struct ClosedHostView<'a> { host: &'a SeatHost }

impl<'a> ClosedHostView<'a> {
    pub(super) fn from_closed(host: &'a SeatHost) -> Self { Self { host } }

    pub(super) fn boundary(&self) -> Option<&'a ReportBoundary> { self.host.report_boundary.as_ref() }

    pub(super) fn scopes(&self) -> Scopes<'a> { Scopes { next: self.host.scopes.as_deref() } }

    pub(super) fn drivers(&self) -> Drivers<'a> { Drivers { next: self.host.drivers.as_deref() } }

    pub(super) fn registration_timeout(&self) -> Option<&'a tokio::time::error::Elapsed> {
        self.host.registration.as_deref().and_then(registration::Registrar::quiescence_timeout)
    }

    #[cfg(windows)]
    pub(super) fn custodies(&self) -> Custodies<'a> { Custodies { next: self.host.custody_attempts.as_deref() } }

    /// Generic root refusal accompanies, never replaces, actual endpoint causes.
    pub(super) fn transport_refusal(&self) -> Option<crate::operation::OperationError> {
        self.host.transport.as_deref().and_then(transport::Root::refusal)
    }

    #[cfg(windows)]
    pub(super) fn endpoints(&self) -> transport::Endpoints<'a> {
        transport::Endpoints::from_root(self.host.transport.as_deref())
    }

    pub(super) fn journal_failure(&self) -> Option<journal::FailureReadback<'a>> {
        self.host.journal.as_deref().map(journal::JournalRoot::failure)
    }
}

pub(super) struct Scopes<'a> { next: Option<&'a ScopeEntry> }

#[cfg(windows)]
pub(super) struct Custodies<'a> { next: Option<&'a custody::Attempt> }

#[cfg(windows)]
impl<'a> Iterator for Custodies<'a> {
    type Item = custody::Readback<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let attempt = self.next?; self.next = attempt.next.as_deref();
        Some(attempt.readback())
    }
}

pub(super) struct ScopeReadback<'a> { control: &'a ScopeControl }

impl ScopeReadback<'_> {
    pub(super) fn closed(&self) -> bool { self.control.is_closed() }
    pub(super) fn durable_open(&self) -> bool { self.control.durable_open.load(Ordering::Acquire) }
    pub(super) fn uncertain(&self) -> bool { self.control.uncertain.load(Ordering::Acquire) }
    pub(super) fn boundary(&self) -> Option<ScopeJoinBoundary> { self.control.join_boundary() }
    pub(super) fn registration_timeout(&self) -> Option<&tokio::time::error::Elapsed> {
        self.control.registration_timeout.get()
    }
}

impl<'a> Iterator for Scopes<'a> {
    type Item = ScopeReadback<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.next?; self.next = entry.next.as_deref();
        Some(ScopeReadback { control: &entry.control })
    }
}

pub(super) struct Drivers<'a> { next: Option<&'a DriverEntry> }

pub(super) struct DriverReadback<'a> {
    pub(super) registration_refusal: Option<&'a crate::operation::OperationError>,
    pub(super) task: task::TaskReadback<'a, crate::driver::owned_driver_task::DriverExit>,
    pub(super) scope: ScopeReadback<'a>,
}

impl<'a> Iterator for Drivers<'a> {
    type Item = DriverReadback<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.next?; self.next = entry.next.as_deref();
        Some(DriverReadback { registration_refusal: entry.registration_refusal.as_ref(),
            task: entry.task.readback(), scope: ScopeReadback { control: &entry.scope } })
    }
}

#[cfg(test)]
mod controls {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn closed_view_borrows_original_boundary_and_all_scopes_without_claiming_settlement() {
        Handle::current().spawn(async {
            let mut host = SeatHost::new(Handle::current());
            assert!(matches!(host.closed_readback(), Err(ReadbackRefusal::AdmissionOpen)));
            host.registration = Some(Arc::new(registration::Registrar::new(host.runtime.clone())));
            let first = ScopeControl::source_control(); let second = ScopeControl::source_control();
            host.scopes = Some(Box::new(ScopeEntry { control: first.clone(),
                next: Some(Box::new(ScopeEntry { control: second.clone(), next: None })) }));
            let requested = Instant::now(); let deadline = requested + std::time::Duration::from_secs(10);
            assert_eq!(host.close_global(requested, deadline), Some(deadline));
            let view = host.closed_readback().unwrap();
            let pointer = std::ptr::from_ref(view.boundary().unwrap()) as usize;
            assert_eq!(view.boundary().unwrap().first_requested, requested);
            assert_eq!(view.boundary().unwrap().deadline, deadline);
            let mut scopes = view.scopes();
            for scope in [scopes.next().unwrap(), scopes.next().unwrap()] {
                assert!(scope.closed() && !scope.durable_open() && !scope.uncertain());
                assert_eq!(scope.boundary().unwrap().deadline, deadline);
            }
            assert!(scopes.next().is_none() && view.drivers().next().is_none() && view.journal_failure().is_none());
            assert_eq!(std::ptr::from_ref(view.boundary().unwrap()) as usize, pointer);
            let _ = view; // Release only the borrow; owner and scope roots remain.
            assert!(Arc::ptr_eq(&host.scopes.as_ref().unwrap().control, &first));
            assert!(Arc::ptr_eq(&host.scopes.as_ref().unwrap().next.as_ref().unwrap().control, &second));
            assert_eq!(host.report_boundary.unwrap().deadline, deadline);
            // Explicit registry/scope fixture, no production fit, journal,
            // writer census, child settlement or permission for CLEAN.
        }).await.unwrap();
    }
}
