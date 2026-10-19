//! m5e70996b16e5: one private process original, separate from driver originals.
//! Shared budget arithmetic, distinct capability. No archive, child generation,
//! correlation, capture, GO, allowance deadline or conversion into Operation.
//! The COMPLETE actual Host fit producer and journal/namespace bridge are owed.

use std::alloc::Layout;
use std::sync::{Arc, Mutex};

use crate::driver::owned_driver_task::own_arc_layout;
use crate::operation::{OperationError, OriginalGrowthBudget};
#[cfg(windows)]
use crate::operation::HOST_CANONICAL_BYTES;

/// Sealed and nonclonable. Only this module can construct it. Deliberately NO
/// production constructor until the whole actual process ownership fit exists.
pub(super) struct HostFit { fixed: Layout }

/// Pure inline owner slot, installed by SeatHost::new before any effect.
/// Nothing can take/reset its original or produce a replacement for this owner.
pub(super) struct OwnerSlot {
    attempted: bool,
    original: Option<HostOriginal>,
    result: Option<Result<(), OperationError>>,
}

struct HostOriginal { reference: HostRef }

/// Borrowable charging capability ONLY. In particular this cannot be passed
/// where a driver Operation/OriginalCreationFit/ArchivePlan is required.
#[derive(Clone)]
pub(super) struct HostRef(Arc<Mutex<State>>);

// A retained charging reference, NOT another original or pool. Production has
// ONLY the Host variant. Old driver-backed observation controls remain cfg(test).
#[derive(Clone)]
pub(super) enum ChargeRef {
    Host(HostRef),
    #[cfg(test)]
    DriverFixture(crate::operation::Operation),
}

mod source_seal { pub trait Sealed {} }
pub(super) trait Source: source_seal::Sealed { fn charge_ref(&self) -> ChargeRef; }
impl source_seal::Sealed for HostRef {}
impl Source for HostRef { fn charge_ref(&self) -> ChargeRef { ChargeRef::Host(self.clone()) } }
impl source_seal::Sealed for ChargeRef {}
impl Source for ChargeRef { fn charge_ref(&self) -> ChargeRef { self.clone() } }
#[cfg(test)]
impl source_seal::Sealed for crate::operation::Operation {}
#[cfg(test)]
impl Source for crate::operation::Operation {
    fn charge_ref(&self) -> ChargeRef { ChargeRef::DriverFixture(self.clone()) }
}

pub(super) fn prepay(original: &impl Source, layout: Layout) -> Result<(), OperationError> {
    let original = original.charge_ref();
    original.check()?; original.charge(layout.size())?; original.check()
}

impl ChargeRef {
    pub(super) fn check(&self) -> Result<(), OperationError> {
        match self { Self::Host(host) => host.check(),
            #[cfg(test)] Self::DriverFixture(driver) => driver.check() }
    }
    pub(super) fn charge(&self, bytes: usize) -> Result<(), OperationError> {
        match self { Self::Host(host) => host.charge(bytes),
            #[cfg(test)] Self::DriverFixture(driver) => driver.charge(bytes) }
    }
    pub(super) fn same_instance(&self, other: &impl Source) -> bool {
        match (self, other.charge_ref()) {
            (Self::Host(left), Self::Host(right)) => left.same_instance(&right),
            #[cfg(test)]
            (Self::DriverFixture(left), Self::DriverFixture(right)) => left.same_instance(&right),
            #[cfg(test)] _ => false,
        }
    }
    #[cfg(test)]
    pub(super) fn is_driver_fixture(&self) -> bool { matches!(self, Self::DriverFixture(_)) }

    #[cfg(windows)]
    pub(super) fn reserve_host_canonical(&self) -> Result<CanonicalRefCredit, OperationError> {
        match self {
            Self::Host(host) => host.reserve_canonical().map(CanonicalRefCredit::Host),
            #[cfg(test)] Self::DriverFixture(driver) => driver.reserve_host_canonical().map(CanonicalRefCredit::DriverFixture),
        }
    }
}

#[cfg(windows)]
pub(super) enum CanonicalRefCredit {
    Host(CanonicalCharge),
    #[cfg(test)] DriverFixture(crate::operation::HostCanonicalCharge),
}

#[cfg(windows)]
impl CanonicalRefCredit {
    pub(super) fn release_after_retained_result(&mut self) -> Result<(), OperationError> {
        match self {
            Self::Host(host) => host.release_after_retained_result(),
            #[cfg(test)] Self::DriverFixture(driver) => driver.release_after_retained_result(),
        }
    }
}

struct State {
    budget: OriginalGrowthBudget,
    captured: usize,
    first_refusal: Option<OperationError>,
    #[cfg(windows)]
    canonical_active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Usage {
    pub(super) fixed: usize,
    pub(super) growth_limit: usize,
    pub(super) consumed: usize,
    pub(super) active_c: usize,
    pub(super) first_refusal: Option<OperationError>,
}

impl OwnerSlot {
    pub(super) const fn new() -> Self {
        Self { attempted: false, original: None, result: None }
    }

    /// Necessary known lower bound only, NOT a complete-fit producer. The
    /// actual outer Pending/Ready owner and all retained children still belong
    /// in F or prepaid growth. All terms are real types, not a caller budget.
    fn minimum_fixed_layout() -> Result<Layout, OperationError> {
        Layout::new::<super::SeatHost>()
            .extend(own_arc_layout::<Mutex<State>>()?)
            .and_then(|(layout, _)| layout.extend(Layout::new::<HostFit>()))
            .map(|(layout, _)| layout.pad_to_align())
            .map_err(|_| OperationError::CaptureOverflow)
    }

    pub(super) fn consume(&mut self, pending: &mut Option<HostFit>) -> Result<(), OperationError> {
        if self.attempted {
            return Err(match self.result.as_ref() {
                Some(Err(error)) => *error,
                _ => OperationError::CreditOwned,
            });
        }
        self.attempted = true;
        let checked = (|| {
            let fit = pending.as_ref().ok_or(OperationError::CreditOwned)?;
            let minimum = Self::minimum_fixed_layout()?;
            if fit.fixed.align() < minimum.align() || fit.fixed.pad_to_align().size() < minimum.size() {
                return Err(OperationError::CaptureOverflow);
            }
            OriginalGrowthBudget::from_complete_layout(fit.fixed)
        })();
        let budget = match checked {
            Ok(budget) => budget,
            Err(error) => {
                self.result = Some(Err(error)); return Err(error);
            }
        };
        // All checks precede the sole MOVE. Failed input stays pending; no
        // allocation or Host effect has occurred. This slot never retries.
        let _fit = pending.take().expect("checked sealed Host fit");
        let fixed = budget.fixed_bytes;
        self.original = Some(HostOriginal { reference: HostRef(Arc::new(Mutex::new(State {
            budget, captured: fixed, first_refusal: None,
            #[cfg(windows)] canonical_active: false,
        }))) });
        self.result = Some(Ok(()));
        Ok(())
    }

    pub(super) fn reference(&self) -> Result<HostRef, OperationError> {
        self.original.as_ref().map(|owner| owner.reference.clone())
            .ok_or_else(|| match self.result.as_ref() {
                Some(Err(error)) => *error,
                _ => OperationError::CreditOwned,
            })
    }
}

impl State {
    fn active_c(&self) -> usize {
        #[cfg(windows)]
        if self.canonical_active { return HOST_CANONICAL_BYTES; }
        0
    }

    fn admit(&mut self, bytes: usize) -> Result<usize, OperationError> {
        if let Some(error) = &self.first_refusal { return Err(*error); }
        let total = self.captured.checked_add(bytes);
        if let Some(total) = total {
            if total.checked_add(self.active_c()).is_some_and(|total| self.budget.admits(total)) {
                return Ok(total);
            }
        }
        self.first_refusal = Some(OperationError::CaptureOverflow);
        Err(OperationError::CaptureOverflow)
    }
}

impl HostRef {
    pub(super) fn same_instance(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) }

    pub(super) fn check(&self) -> Result<(), OperationError> {
        let state = self.0.lock().map_err(|_| OperationError::CreditOwned)?;
        match &state.first_refusal { Some(error) => Err(*error), None => Ok(()) }
    }

    pub(super) fn charge(&self, bytes: usize) -> Result<(), OperationError> {
        let mut state = self.0.lock().map_err(|_| OperationError::CreditOwned)?;
        state.captured = state.admit(bytes)?;
        Ok(())
    }

    pub(super) fn usage(&self) -> Result<Usage, OperationError> {
        let state = self.0.lock().map_err(|_| OperationError::CreditOwned)?;
        let consumed = state.captured.checked_sub(state.budget.fixed_bytes)
            .ok_or(OperationError::CaptureOverflow)?;
        Ok(Usage { fixed: state.budget.fixed_bytes, growth_limit: state.budget.growth_limit,
            consumed, active_c: state.active_c(), first_refusal: state.first_refusal })
    }

    #[cfg(windows)]
    pub(super) fn reserve_canonical(&self) -> Result<CanonicalCharge, OperationError> {
        let mut state = self.0.lock().map_err(|_| OperationError::CreditOwned)?;
        if let Some(error) = &state.first_refusal { return Err(*error); }
        if state.canonical_active { return Err(OperationError::CreditOwned); }
        // Use the same gate as normal growth. C is transient active liability,
        // so do not add it to the ordinary monotone captured prefix.
        state.admit(HOST_CANONICAL_BYTES)?;
        state.canonical_active = true;
        Ok(CanonicalCharge { original: self.clone(), active: true })
    }
}

#[cfg(windows)]
pub(super) struct CanonicalCharge { original: HostRef, active: bool }

#[cfg(windows)]
impl CanonicalCharge {
    /// m9aff's ONLY refund exception, after the actual whole Result is retained.
    /// No Clone/Drop refund, normal-prefix reset or first-refusal replacement.
    pub(super) fn release_after_retained_result(&mut self) -> Result<(), OperationError> {
        let mut state = self.original.0.lock().map_err(|_| OperationError::CreditOwned)?;
        if !self.active || !state.canonical_active { return Err(OperationError::CreditOwned); }
        state.canonical_active = false; self.active = false;
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod controls {
    use super::*;
    use crate::operation::OPERATION_BYTES;

    // Explicit partial-layout fixtures ONLY. These do not mint a production
    // complete Host fit or prove full owner/library/outer storage admission.
    pub(in crate::seat_host) fn fixture_fit() -> HostFit { HostFit { fixed: OwnerSlot::minimum_fixed_layout().unwrap() } }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fixed_transport_counter_is_host_charged_shared_and_never_replaced() {
        tokio::runtime::Handle::current().spawn(async {
            use super::super::{transport, ScopeControl};
            let mut host = super::super::SeatHost::new(tokio::runtime::Handle::current());
            host.install_host_original(&mut Some(fixture_fit())).unwrap();
            let original = host.host_original.reference().unwrap();
            let before = original.usage().unwrap();
            let mut root = transport::Root::reserve(&original).unwrap();
            assert_eq!(original.usage().unwrap().consumed - before.consumed,
                transport::Root::owner_layout().size());
            let scope = ScopeControl::source_control();
            let seat = cna_core::ids::SeatId::all().next().unwrap();
            let before = original.usage().unwrap();
            // Explicit partial Host meter/control fixture. Number1 is not a
            // produced durable lease, endpoint, complete fit or seat admission.
            root.reserve_counter(1, &scope, seat).unwrap();
            assert_eq!(original.usage().unwrap().consumed - before.consumed,
                transport::Root::counter_bytes().unwrap());
            let first = root.counter(1, &scope, seat).unwrap();
            let alias = root.counter(1, &scope, seat).unwrap();
            assert!(Arc::ptr_eq(&first, &alias));
            let retained = original.usage().unwrap();
            assert_eq!(root.reserve_counter(1, &scope, seat), Err(OperationError::CreditOwned));
            assert_eq!(root.refusal(), Some(OperationError::CreditOwned));
            assert!(Arc::ptr_eq(&first, &root.counter(1, &scope, seat).unwrap()));
            assert_eq!(original.usage().unwrap(), retained); // No replacement/clone allocation charge.
            assert_eq!(Arc::strong_count(&scope), 1); // Root keeps only Weak scope.
            drop(root);
            assert_eq!(original.usage().unwrap(), retained); // Never refund at last drop.
            assert_eq!(first.calls.load(std::sync::atomic::Ordering::Acquire), 0);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn transport_counter_refuses_before_allocation_if_whole_fixed_slot_cannot_fit() {
        tokio::runtime::Handle::current().spawn(async {
            use super::super::{transport, ScopeControl};
            let mut host = super::super::SeatHost::new(tokio::runtime::Handle::current());
            host.install_host_original(&mut Some(fixture_fit())).unwrap();
            let original = host.host_original.reference().unwrap();
            let mut root = transport::Root::reserve(&original).unwrap();
            let usage = original.usage().unwrap();
            let slot = transport::Root::counter_bytes().unwrap();
            let remaining = usage.growth_limit - usage.consumed;
            original.charge(remaining - (slot - 1)).unwrap();
            let scope = ScopeControl::source_control();
            let seat = cna_core::ids::SeatId::all().next().unwrap();
            assert_eq!(root.reserve_counter(1, &scope, seat), Err(OperationError::CaptureOverflow));
            assert!(root.counter(1, &scope, seat).is_none());
            assert_eq!(root.refusal(), Some(OperationError::CaptureOverflow));
            let refused = original.usage().unwrap();
            assert_eq!(refused.first_refusal, Some(OperationError::CaptureOverflow));
            assert_eq!(root.reserve_counter(1, &scope, seat), Err(OperationError::CaptureOverflow));
            assert_eq!(original.usage().unwrap(), refused); // Partial prepayment remains held.
            // Actual allocation refusal source only; no OS fault injection,
            // native work, production fit/lease or MCP task was constructed.
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dormant_scope_custody_is_host_charged_and_global_close_finds_it_without_admission() {
        tokio::runtime::Handle::current().spawn(async {
            use super::super::{registration::Registrar, ScopeEntry, SeatHostLease};
            use crate::operation::{Allowance, Operation};
            let mut host = super::super::SeatHost::new(tokio::runtime::Handle::current());
            host.install_host_original(&mut Some(fixture_fit())).unwrap();
            let original = host.host_original.reference().unwrap();
            prepay(&original, Registrar::creation_layout().unwrap()).unwrap();
            host.registration = Some(Arc::new(Registrar::new_owned(host.runtime.clone(), original.clone())));
            let before = original.usage().unwrap();
            host.scopes = Some(ScopeEntry::reserve_unpublished(&original).unwrap());
            let charged = original.usage().unwrap();
            assert_eq!(charged.consumed - before.consumed, ScopeEntry::custody_bytes().unwrap());
            let control = host.scopes.as_ref().unwrap().control.clone();
            assert!(!control.durable_open.load(std::sync::atomic::Ordering::Acquire));
            assert!(!control.is_closed()); assert!(control.join_boundary().is_none());
            // Explicit endpoint fixture to exercise the real registration gate;
            // this is NOT a produced lease, durable scope or complete fit.
            let lease = SeatHostLease { control: control.clone(),
                registration: Arc::downgrade(host.registration.as_ref().unwrap()) };
            let driver = Operation::new(String::from("dormant-scope-refuses"), 1,
                Allowance::required(Some(tokio::time::Instant::now() + std::time::Duration::from_secs(30))).unwrap()).unwrap();
            let driver_before = driver.captured_bytes(); let mut pending = None;
            assert!(matches!(lease.register_driver(&driver, &mut pending), Err(OperationError::CreditOwned)));
            assert_eq!(original.usage().unwrap(), charged); assert_eq!(driver.captured_bytes(), driver_before);
            assert!(host.drivers.is_none() && pending.is_none());
            let first = tokio::time::Instant::now();
            let deadline = first + std::time::Duration::from_secs(10);
            assert_eq!(host.close_global(first, deadline), Some(deadline));
            assert!(control.is_closed()); assert_eq!(control.join_ceiling(), Some(deadline));
            assert!(!control.durable_open.load(std::sync::atomic::Ordering::Acquire));
            drop(lease); drop(control); drop(host);
            assert_eq!(original.usage().unwrap(), charged);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scope_second_prepayment_refusal_retains_prefix_before_either_allocation() {
        tokio::runtime::Handle::current().spawn(async {
            use super::super::ScopeEntry;
            let mut host = super::super::SeatHost::new(tokio::runtime::Handle::current());
            host.install_host_original(&mut Some(fixture_fit())).unwrap();
            let original = host.host_original.reference().unwrap();
            let total = ScopeEntry::custody_bytes().unwrap();
            let before = original.usage().unwrap();
            let remaining = before.growth_limit.checked_sub(before.consumed).unwrap();
            original.charge(remaining.checked_sub(total.checked_sub(1).unwrap()).unwrap()).unwrap();
            let before = original.usage().unwrap();
            assert!(matches!(ScopeEntry::reserve_unpublished(&original), Err(OperationError::CaptureOverflow)));
            let refused = original.usage().unwrap();
            assert_eq!(refused.consumed - before.consumed, Layout::new::<ScopeEntry>().size());
            assert_eq!(refused.first_refusal, Some(OperationError::CaptureOverflow));
            assert!(host.scopes.is_none() && host.drivers.is_none() && host.registration.is_none());
            assert!(matches!(ScopeEntry::reserve_unpublished(&original), Err(OperationError::CaptureOverflow)));
            assert_eq!(original.usage().unwrap(), refused);
            drop(host); assert_eq!(original.usage().unwrap(), refused);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn registry_kept_entry_credit_is_host_owned_and_driver_preparation_refusal_refunds_nothing() {
        tokio::runtime::Handle::current().spawn(async {
            use super::super::{registration::Registrar, ScopeControl};
            use crate::operation::{Allowance, Operation};
            let mut host = super::super::SeatHost::new(tokio::runtime::Handle::current());
            host.install_host_original(&mut Some(fixture_fit())).unwrap();
            let original = host.host_original.reference().unwrap();
            prepay(&original, Registrar::creation_layout().unwrap()).unwrap();
            host.registration = Some(Arc::new(Registrar::new_owned(host.runtime.clone(), original.clone())));
            let registrar = host.registration.as_ref().unwrap().clone();
            assert!(registrar.owned_by(&original));
            // Scope and driver are external partial fixtures, NOT durable scope
            // issuance, complete original creation or a production caller hook.
            let scope = ScopeControl::source_control(); scope.durable_open.store(true, std::sync::atomic::Ordering::Release);
            let driver = Operation::new(String::from("host-record-driver-refusal"), 1,
                Allowance::required(Some(tokio::time::Instant::now() + std::time::Duration::from_secs(30))).unwrap()).unwrap();
            let driver_before = driver.captured_bytes(); let before = original.usage().unwrap();
            let mut pending = None;
            assert!(matches!(registrar.register(&scope, &driver, &mut pending), Err(OperationError::CreditOwned)));
            let charged = original.usage().unwrap();
            assert_eq!(charged.consumed - before.consumed, Registrar::retained_entry_bytes().unwrap());
            assert_eq!(driver.captured_bytes(), driver_before); assert!(pending.is_none());
            let (entries, uncertain, active) = registrar.drain(false, Some(&scope));
            assert!(entries.is_none() && !uncertain); assert_eq!(active, 0);
            assert_eq!(registrar.admission_failure(), None);
            drop(registrar); drop(host);
            assert_eq!(original.usage().unwrap(), charged); assert_eq!(driver.captured_bytes(), driver_before);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn registry_host_exhaustion_is_retained_before_preparing_driver_and_cannot_reopen() {
        tokio::runtime::Handle::current().spawn(async {
            use super::super::{registration::Registrar, ScopeControl};
            use crate::operation::{Allowance, Operation};
            let mut host = super::super::SeatHost::new(tokio::runtime::Handle::current());
            host.install_host_original(&mut Some(fixture_fit())).unwrap();
            let original = host.host_original.reference().unwrap();
            prepay(&original, Registrar::creation_layout().unwrap()).unwrap();
            host.registration = Some(Arc::new(Registrar::new_owned(host.runtime.clone(), original.clone())));
            let registrar = host.registration.as_ref().unwrap().clone();
            let scope = ScopeControl::source_control(); scope.durable_open.store(true, std::sync::atomic::Ordering::Release);
            let driver = Operation::new(String::from("host-exhausted-no-driver-charge"), 1,
                Allowance::required(Some(tokio::time::Instant::now() + std::time::Duration::from_secs(30))).unwrap()).unwrap();
            let usage = original.usage().unwrap(); original.charge(usage.growth_limit - usage.consumed).unwrap();
            let full = original.usage().unwrap(); let driver_before = driver.captured_bytes(); let mut pending = None;
            assert!(matches!(registrar.register(&scope, &driver, &mut pending), Err(OperationError::CaptureOverflow)));
            assert_eq!(registrar.admission_failure(), Some(OperationError::CaptureOverflow));
            assert!(matches!(registrar.register(&scope, &driver, &mut pending), Err(OperationError::CaptureOverflow)));
            assert_eq!(original.usage().unwrap().consumed, full.consumed);
            assert_eq!(driver.captured_bytes(), driver_before); assert!(pending.is_none());
            let (entries, uncertain, active) = registrar.drain(false, Some(&scope));
            assert!(entries.is_none() && !uncertain); assert_eq!(active, 0);
            assert_eq!(scope.registration_active.load(std::sync::atomic::Ordering::Acquire), 0);
            assert!(host.drivers.is_none());
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actual_open_revalidation_precedes_owned_registry_arc_creation_and_never_replaces_it() {
        tokio::runtime::Handle::current().spawn(async {
            use super::super::{journal, namespace, registration::Registrar};
            let base = std::env::temp_dir().join(format!("cna-host-registry-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&base).unwrap();
            let mut host = super::super::SeatHost::new(tokio::runtime::Handle::current());
            host.install_host_original(&mut Some(fixture_fit())).unwrap();
            let original = host.host_original.reference().unwrap();
            assert_eq!(host.observe_existing_namespace_layout(&original, &base, &[]).unwrap(), namespace::Observation::Observed);
            assert_eq!(host.prepare_observed_namespace().unwrap(), namespace::Observation::Observed);
            assert_eq!(host.canonicalize_observed_namespace_layout().unwrap(), namespace::Observation::Observed);
            host.reserve_journal(&original).unwrap();
            assert_eq!(host.open_observed_journal().unwrap(), journal::Observation::Configured);
            assert_eq!(host.establish_observed_open().unwrap(), journal::Observation::Configured);
            let before = original.usage().unwrap(); host.install_owned_registration().unwrap();
            let charged = original.usage().unwrap();
            assert_eq!(charged.consumed - before.consumed, Registrar::creation_layout().unwrap().size());
            assert!(host.registration.as_ref().unwrap().owned_by(&original));
            let identity = Arc::as_ptr(host.registration.as_ref().unwrap()) as usize;
            assert_eq!(host.install_owned_registration(), Err(OperationError::CreditOwned));
            assert_eq!(Arc::as_ptr(host.registration.as_ref().unwrap()) as usize, identity);
            assert_eq!(original.usage().unwrap(), charged); assert!(host.scopes.is_none() && host.drivers.is_none());
            drop(host); assert_eq!(original.usage().unwrap(), charged);
            std::fs::remove_file(base.join(".seat-host/host.sqlite")).unwrap();
            std::fs::remove_file(base.join(".seat-host/host.lock")).unwrap();
            std::fs::remove_dir(base.join(".seat-host")).unwrap(); std::fs::remove_dir(base).unwrap();
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn process_journal_charges_only_its_actual_host_and_refuses_another_owner_before_allocation() {
        tokio::runtime::Handle::current().spawn(async {
            let mut owner = super::super::SeatHost::new(tokio::runtime::Handle::current());
            owner.install_host_original(&mut Some(fixture_fit())).unwrap();
            let original = owner.host_original.reference().unwrap();
            let mut other = super::super::SeatHost::new(tokio::runtime::Handle::current());
            other.install_host_original(&mut Some(fixture_fit())).unwrap();
            let unrelated = other.host_original.reference().unwrap();
            let before = original.usage().unwrap();
            let other_before = unrelated.usage().unwrap();
            assert_eq!(other.reserve_journal(&original), Err(OperationError::CreditOwned));
            assert!(other.journal.is_none() && other.admission_closed);
            assert_eq!(other.host_source_refusal, Some(OperationError::CreditOwned));
            assert_eq!(original.usage().unwrap(), before);
            assert_eq!(unrelated.usage().unwrap(), other_before);
            assert_eq!(other.reserve_journal(&unrelated), Err(OperationError::CreditOwned));
            assert!(other.journal.is_none());
            assert_eq!(unrelated.usage().unwrap(), other_before);
            owner.reserve_journal(&original).unwrap();
            #[cfg(windows)]
            assert!(owner.journal.as_deref().unwrap().context_original().same_instance(&original));
            let charged = original.usage().unwrap();
            assert_eq!(charged.consumed - before.consumed,
                Layout::new::<super::super::journal::JournalRoot>().size() + super::super::journal::J_BYTES);
            assert_eq!(unrelated.usage().unwrap(), other_before);
            assert_eq!(owner.reserve_journal(&original), Err(OperationError::CreditOwned));
            assert_eq!(original.usage().unwrap(), charged);
            drop(owner); assert_eq!(original.usage().unwrap(), charged);
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn typed_host_reference_owns_namespace_canonical_journal_and_context_without_driver_meter() {
        tokio::runtime::Handle::current().spawn(async {
            use super::super::{journal, namespace};
            let base = std::env::temp_dir().join(format!("cna-host-charge-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&base).unwrap();
            let mut host = super::super::SeatHost::new(tokio::runtime::Handle::current());
            host.install_host_original(&mut Some(fixture_fit())).unwrap();
            let original = host.host_original.reference().unwrap();
            let before = original.usage().unwrap();
            assert_eq!(host.observe_existing_namespace_layout(&original, &base, &[]).unwrap(), namespace::Observation::Observed);
            assert_eq!(host.prepare_observed_namespace().unwrap(), namespace::Observation::Observed);
            assert_eq!(host.canonicalize_observed_namespace_layout().unwrap(), namespace::Observation::Observed);
            let paths = original.usage().unwrap();
            assert!(paths.consumed > before.consumed); assert_eq!(paths.active_c, 0);
            host.reserve_journal(&original).unwrap();
            let reserved = original.usage().unwrap();
            assert_eq!(reserved.consumed - paths.consumed, Layout::new::<journal::JournalRoot>().size() + journal::J_BYTES);
            assert_eq!(host.open_observed_journal().unwrap(), journal::Observation::Configured);
            assert_eq!(host.establish_observed_open().unwrap(), journal::Observation::Configured);
            let binding = *host.journal.as_ref().unwrap().open_binding().unwrap();
            assert!(matches!(host.journal.as_ref().unwrap().context_original(), ChargeRef::Host(_)));
            assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &base, &[]).unwrap(), namespace::Observation::Observed);
            let retained = original.usage().unwrap();
            assert!(retained.consumed > reserved.consumed); assert_eq!(retained.active_c, 0);
            assert_eq!(retained.fixed, before.fixed); assert_eq!(retained.growth_limit, before.growth_limit);
            assert!(host.journal.as_ref().unwrap().open_binding().unwrap().incarnation == binding.incarnation);
            assert!(host.scopes.is_none() && host.registration.is_none());
            // Partial actual-type fixture, NOT complete production fit/adoption.
            // This control never constructs any driver Operation or archive.
            drop(host); assert_eq!(original.usage().unwrap(), retained);
            std::fs::remove_file(base.join(".seat-host/host.sqlite")).unwrap();
            std::fs::remove_file(base.join(".seat-host/host.lock")).unwrap();
            std::fs::remove_dir(base.join(".seat-host")).unwrap(); std::fs::remove_dir(base).unwrap();
        }).await.unwrap();
    }

    // Compile-time ambiguity controls: adding an Into conversion to either
    // driver form makes type inference ambiguous and this control fail to build.
    // No public/private driver fixture API or new crate is needed.
    trait AmbiguousIfIntoDriver<A> { fn assert_no_driver_conversion() {} }
    impl<T> AmbiguousIfIntoDriver<()> for T {}
    impl<T: Into<crate::operation::Operation>> AmbiguousIfIntoDriver<u8> for T {}
    impl<T: Into<crate::failed_evidence::OriginalCreationFit>> AmbiguousIfIntoDriver<u16> for T {}

    #[test]
    fn host_capabilities_have_no_conversion_into_either_driver_original_form() {
        let _ = <HostFit as AmbiguousIfIntoDriver<_>>::assert_no_driver_conversion;
        let _ = <HostRef as AmbiguousIfIntoDriver<_>>::assert_no_driver_conversion;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actual_process_slot_consumes_once_and_independent_process_owners_do_not_share_growth() {
        tokio::runtime::Handle::current().spawn(async {
            let mut first = super::super::SeatHost::new(tokio::runtime::Handle::current());
            let mut pending = Some(fixture_fit());
            first.install_host_original(&mut pending).unwrap(); assert!(pending.is_none());
            let reference = first.host_original.reference().unwrap();
            let alias = reference.clone(); assert!(reference.same_instance(&alias));
            let usage = reference.usage().unwrap();
            assert_eq!(usage.fixed + usage.growth_limit, OPERATION_BYTES);
            assert_eq!(usage.consumed, 0); assert_eq!(usage.active_c, 0);
            let mut replacement = Some(fixture_fit());
            assert_eq!(first.install_host_original(&mut replacement), Err(OperationError::CreditOwned));
            assert!(replacement.is_some());
            assert!(reference.same_instance(&first.host_original.reference().unwrap()));
            let mut second = super::super::SeatHost::new(tokio::runtime::Handle::current());
            second.install_host_original(&mut Some(fixture_fit())).unwrap();
            let separate = second.host_original.reference().unwrap();
            assert!(!reference.same_instance(&separate));
            reference.charge(usage.growth_limit).unwrap();
            let full = reference.usage().unwrap();
            assert_eq!(full.consumed, usage.growth_limit);
            assert_eq!(alias.charge(1), Err(OperationError::CaptureOverflow));
            assert_eq!(reference.check(), Err(OperationError::CaptureOverflow));
            assert_eq!(reference.usage().unwrap().consumed, full.consumed);
            assert_eq!(separate.usage().unwrap().consumed, 0);
            drop(alias); assert_eq!(reference.usage().unwrap().consumed, full.consumed);
            assert!(first.journal.is_none() && first.scopes.is_none());
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_fit_is_retained_without_allocation_retry_or_replacement_original() {
        tokio::runtime::Handle::current().spawn(async {
            let mut host = super::super::SeatHost::new(tokio::runtime::Handle::current());
            let mut pending = Some(HostFit { fixed: Layout::new::<()>() });
            assert_eq!(host.install_host_original(&mut pending), Err(OperationError::CaptureOverflow));
            assert!(pending.is_some() && host.host_original.original.is_none());
            let first = std::ptr::from_ref(host.host_original.result.as_ref().unwrap()) as usize;
            let mut valid_fixture = Some(fixture_fit());
            assert_eq!(host.install_host_original(&mut valid_fixture), Err(OperationError::CaptureOverflow));
            assert!(valid_fixture.is_some() && host.host_original.original.is_none());
            assert_eq!(std::ptr::from_ref(host.host_original.result.as_ref().unwrap()) as usize, first);
            assert!(host.journal.is_none() && host.registration.is_none());
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_host_c_credit_keeps_liability_and_old_token_cannot_release_a_later_call() {
        tokio::runtime::Handle::current().spawn(async {
            let mut host = super::super::SeatHost::new(tokio::runtime::Handle::current());
            host.install_host_original(&mut Some(fixture_fit())).unwrap();
            let original = host.host_original.reference().unwrap();
            let mut first = original.reserve_canonical().unwrap();
            original.charge(17).unwrap();
            let retained: Result<(), std::io::Error> = Err(std::io::Error::from_raw_os_error(2));
            assert_eq!(retained.as_ref().unwrap_err().raw_os_error(), Some(2));
            first.release_after_retained_result().unwrap();
            assert_eq!(original.usage().unwrap().consumed, 17);
            let next = original.reserve_canonical().unwrap();
            assert_eq!(first.release_after_retained_result(), Err(OperationError::CreditOwned));
            assert_eq!(original.usage().unwrap().active_c, HOST_CANONICAL_BYTES);
            drop(next);
            assert_eq!(original.usage().unwrap().active_c, HOST_CANONICAL_BYTES);
            assert_eq!(original.usage().unwrap().consumed, 17);
            assert!(matches!(original.reserve_canonical(), Err(OperationError::CreditOwned)));
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_c_credit_shares_exact_original_growth_and_releases_only_c_after_retained_result() {
        tokio::runtime::Handle::current().spawn(async {
            let mut host = super::super::SeatHost::new(tokio::runtime::Handle::current());
            host.install_host_original(&mut Some(fixture_fit())).unwrap();
            let original = host.host_original.reference().unwrap();
            let mut credit = original.reserve_canonical().unwrap();
            assert!(credit.original.same_instance(&original));
            assert!(matches!(original.reserve_canonical(), Err(OperationError::CreditOwned)));
            let usage = original.usage().unwrap();
            assert_eq!(usage.consumed, 0); assert_eq!(usage.active_c, HOST_CANONICAL_BYTES);
            original.charge(usage.growth_limit - HOST_CANONICAL_BYTES).unwrap();
            assert_eq!(original.charge(1), Err(OperationError::CaptureOverflow));
            // Whole raw error fixture, NOT canonicalize or OS failure injection.
            let retained: Result<std::path::PathBuf, std::io::Error> = Err(std::io::Error::from_raw_os_error(2));
            let error = std::ptr::from_ref(retained.as_ref().unwrap_err()) as usize;
            credit.release_after_retained_result().unwrap();
            let released = original.usage().unwrap();
            assert_eq!(released.active_c, 0); assert_eq!(released.consumed, usage.growth_limit - HOST_CANONICAL_BYTES);
            assert_eq!(released.first_refusal, Some(OperationError::CaptureOverflow));
            assert_eq!(original.charge(0), Err(OperationError::CaptureOverflow));
            assert_eq!(std::ptr::from_ref(retained.as_ref().unwrap_err()) as usize, error);
            assert_eq!(credit.release_after_retained_result(), Err(OperationError::CreditOwned));
        }).await.unwrap();
    }
}
