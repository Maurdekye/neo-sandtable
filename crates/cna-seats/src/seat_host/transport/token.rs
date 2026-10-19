//! UNAPPLIED proposal: private Host-owned fixed token/endpoint attempt custody.
//! No public accessor, caller text, protocol change, server/task or GO path.

use super::{original, ScopeControl};
use crate::{mcp::SeatCounters, operation::OperationError};
use cna_core::ids::SeatId;
use std::{alloc::Layout, sync::{Arc, Weak}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::seat_host) enum Refusal { Closed, Entropy, Zero, Collision }

struct Key([u8; 64]);

impl Key {
    fn from_seed(seed: &[u8; 32]) -> Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = [0; 64];
        for (index, byte) in seed.iter().copied().enumerate() {
            encoded[index * 2] = HEX[(byte >> 4) as usize];
            encoded[index * 2 + 1] = HEX[(byte & 15) as usize];
        }
        Self(encoded)
    }

    fn matches(&self, other: &Self) -> bool { self.0 == other.0 }
}

struct TokenFill {
    seed: [u8; 32],
    returned: Option<Result<(), getrandom::Error>>,
    key: Option<Key>,
}

impl TokenFill {
    fn empty() -> Self { Self { seed: [0; 32], returned: None, key: None } }

    fn request_once(&mut self) -> Result<(), Refusal> {
        if self.returned.is_some() || self.key.is_some() { return Err(Refusal::Closed); }
        // Synthetic bytes and entry counters exist ONLY in unit-test builds.
        // No fake source, extra fill site or selectable backend exists in production.
        #[cfg(test)]
        let fixture_used = {
            controls::REQUEST_ENTRIES.with(|count| count.set(count.get() + 1));
            if let Some(seed) = controls::SEED_FIXTURE.with(std::cell::Cell::take) {
                self.seed = seed; self.returned = Some(Ok(())); true
            } else { false }
        };
        #[cfg(not(test))]
        let fixture_used = false;
        if !fixture_used {
            #[cfg(test)]
            controls::BACKEND_ENTRIES.with(|count| count.set(count.get() + 1));
            self.returned = Some(getrandom::fill(&mut self.seed)); // FIRST whole Result retained.
        }
        if !matches!(self.returned.as_ref(), Some(Ok(()))) { return Err(Refusal::Entropy); }
        if self.seed.iter().all(|byte| *byte == 0) { return Err(Refusal::Zero); }
        self.key = Some(Key::from_seed(&self.seed)); // Fixed inline encoding, no allocation.
        Ok(())
    }
}

pub(super) struct EndpointAttempt {
    number: i64,
    scope: Weak<ScopeControl>,
    seat: SeatId,
    epoch: u64,
    _counter: Arc<SeatCounters>,
    token: TokenFill,
    preparation: Option<Result<(), Refusal>>,
    pub(super) next: Option<Box<Self>>,
}

impl Drop for EndpointAttempt {
    fn drop(&mut self) {
        let mut next = self.next.take();
        while let Some(mut entry) = next { next = entry.next.take(); }
    }
}

impl EndpointAttempt {
    /// Actual node includes fixed32/64byte arrays, raw Result and binding facts.
    /// The counter is an inline Arc alias, never another pointee allocation.
    pub(super) fn reserve(original: &original::HostRef, number: i64, scope: &Arc<ScopeControl>,
        seat: SeatId, epoch: u64, counter: Arc<SeatCounters>) -> Result<Box<Self>, OperationError>
    {
        original::prepay(original, Layout::new::<Self>())?;
        Ok(Box::new(Self { number, scope: Arc::downgrade(scope), seat, epoch,
            _counter: counter, token: TokenFill::empty(), preparation: None, next: None }))
    }

    pub(super) fn prepare(&mut self) -> Result<(), Refusal> {
        if let Some(original) = self.preparation { return original; }
        let result = (|| {
            self.token.request_once()?;
            let key = self.token.key.as_ref().ok_or(Refusal::Closed)?;
            let mut previous = self.next.as_deref();
            while let Some(entry) = previous {
                // Include all earlier retained keys, even an unpublished attempt.
                if entry.token.key.as_ref().is_some_and(|old| old.matches(key)) {
                    return Err(Refusal::Collision);
                }
                previous = entry.next.as_deref();
            }
            Ok(())
        })();
        self.preparation = Some(result); // FIRST complete token/collision result.
        result
    }

    pub(super) fn readback(&self) -> super::EndpointReadback<'_> {
        super::EndpointReadback { number: self.number, scope: &self.scope,
            seat: self.seat, epoch: self.epoch,
            preparation_observed: self.preparation.is_some(),
            refusal: self.refusal(), entropy_error: self.entropy_error() }
    }

    pub(super) fn is_unobserved(&self) -> bool { self.preparation.is_none() }

    pub(super) fn refusal(&self) -> Option<&Refusal> {
        self.preparation.as_ref().and_then(|original| original.as_ref().err())
    }

    pub(super) fn entropy_error(&self) -> Option<&getrandom::Error> {
        self.token.returned.as_ref().and_then(|original| original.as_ref().err())
    }
}

#[cfg(test)]
mod controls {
    use super::*;

    // Thread-local, one-use fixture and observations, unreachable without cfg(test).
    // Counts are unit-test path observations, NOT provider/request-fence proof.
    std::thread_local! {
        pub(super) static SEED_FIXTURE: std::cell::Cell<Option<[u8; 32]>> = const { std::cell::Cell::new(None) };
        pub(super) static REQUEST_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        pub(super) static BACKEND_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    #[test]
    fn encoding_is_fixed64_lower_hex_without_uuid_or_string_allocation() {
        let seed = std::array::from_fn(|index| index as u8);
        let key = Key::from_seed(&seed);
        assert_eq!(&key.0, b"000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
        // Pure encoding fixture only, not entropy/unpredictability or admission.
    }

    #[test]
    fn retained_error_fixture_cannot_retry_or_replace_partially_written_bytes() {
        // Retained-result fixture, NOT OS failure injection or a backend call.
        let mut token = TokenFill { seed: [17; 32], returned: Some(Err(getrandom::Error::UNEXPECTED)), key: None };
        assert_eq!(token.request_once(), Err(Refusal::Closed));
        assert_eq!(token.seed, [17; 32]);
        assert!(matches!(token.returned.as_ref(), Some(Err(original)) if *original == getrandom::Error::UNEXPECTED));
        assert!(token.key.is_none());
        assert_eq!(token.request_once(), Err(Refusal::Closed));
    }

    #[test]
    fn borrowed_failure_readback_keeps_actual_cause_and_binding_without_secret_access() {
        // Explicit retained-error/partial-byte fixture, NOT entropy execution,
        // production fit/lease/admission, or complete Host report integration.
        let scope = ScopeControl::source_control();
        let seat = SeatId::all().next().unwrap();
        let attempt = EndpointAttempt { number: 7, scope: Arc::downgrade(&scope),
            seat, epoch: 3, _counter: Arc::new(SeatCounters::default()),
            token: TokenFill { seed: [17; 32],
                returned: Some(Err(getrandom::Error::UNEXPECTED)), key: None },
            preparation: Some(Err(Refusal::Entropy)), next: None };
        let view = attempt.readback();
        assert_eq!(view.number, 7); assert_eq!(view.seat, seat); assert_eq!(view.epoch, 3);
        assert!(view.scope.ptr_eq(&Arc::downgrade(&scope)));
        assert!(view.preparation_observed);
        assert!(matches!(view.refusal, Some(Refusal::Entropy)));
        assert!(std::ptr::eq(view.refusal.unwrap(), attempt.refusal().unwrap()));
        assert!(std::ptr::eq(view.entropy_error.unwrap(), attempt.entropy_error().unwrap()));
        // One shared borrow, no replacement/move/retry and no seed/key in view.
        assert!(attempt.token.key.is_none());
        assert!(attempt.token.seed.iter().all(|byte| *byte == 17));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_windows_backend_two_retained_attempts_have_distinct_nonzero_lower_hex_keys() {
        // Actual Root/token backend; partial Host fit/control scope ONLY.
        // Not a produced lease, active endpoint, full fit or native CLI.
        SEED_FIXTURE.with(|fixture| fixture.set(None));
        REQUEST_ENTRIES.with(|count| count.set(0)); BACKEND_ENTRIES.with(|count| count.set(0));
        let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
        host.install_host_original(&mut Some(original::controls::fixture_fit())).unwrap();
        let original = host.host_original.reference().unwrap();
        let mut root = super::super::Root::reserve(&original).unwrap();
        let scope = ScopeControl::source_control(); let seat = SeatId::all().next().unwrap();
        root.reserve_counter(1, &scope, seat).unwrap();
        let before = original.usage().unwrap();
        root.prepare_endpoint(1, &scope, seat, 5).unwrap();
        root.prepare_endpoint(1, &scope, seat, 5).unwrap(); // Same-epoch restart is charged.
        let charged = original.usage().unwrap();
        assert_eq!(charged.consumed - before.consumed, 2 * Layout::new::<EndpointAttempt>().size());
        let second = root.endpoints.as_deref().unwrap(); let first = second.next.as_deref().unwrap();
        assert!(first.next.is_none());
        for attempt in [first, second] {
            assert!(matches!(attempt.preparation, Some(Ok(()))));
            assert!(matches!(attempt.token.returned, Some(Ok(()))));
            assert!(attempt.token.seed.iter().any(|byte| *byte != 0));
            let key = attempt.token.key.as_ref().unwrap();
            assert!(key.0.len() == 64);
            assert!(key.0.iter().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)));
        }
        assert!(!first.token.key.as_ref().unwrap().matches(second.token.key.as_ref().unwrap()));
        assert_eq!(REQUEST_ENTRIES.with(std::cell::Cell::get), 2);
        assert_eq!(BACKEND_ENTRIES.with(std::cell::Cell::get), 2);
        assert!(root.refusal().is_none());
        drop(root); assert_eq!(original.usage().unwrap(), charged); // No refund.
    }

    #[tokio::test(flavor = "current_thread")]
    async fn zero_seed_test_only_fixture_latches_root_without_a_second_request_or_backend_entry() {
        // Synthetic returned bytes ONLY, not an OS zero injection/unpredictability proof.
        SEED_FIXTURE.with(|fixture| fixture.set(None));
        REQUEST_ENTRIES.with(|count| count.set(0)); BACKEND_ENTRIES.with(|count| count.set(0));
        let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
        host.install_host_original(&mut Some(original::controls::fixture_fit())).unwrap();
        let original = host.host_original.reference().unwrap();
        let mut root = super::super::Root::reserve(&original).unwrap();
        let scope = ScopeControl::source_control(); let seat = SeatId::all().next().unwrap();
        root.reserve_counter(1, &scope, seat).unwrap();
        let before = original.usage().unwrap();
        SEED_FIXTURE.with(|fixture| fixture.set(Some([0; 32])));
        assert_eq!(root.prepare_endpoint(1, &scope, seat, 5), Err(OperationError::CreditOwned));
        let charged = original.usage().unwrap();
        assert_eq!(charged.consumed - before.consumed, Layout::new::<EndpointAttempt>().size());
        let attempt = root.endpoints.as_deref().unwrap();
        let identity = std::ptr::from_ref(attempt);
        assert!(matches!(attempt.refusal(), Some(Refusal::Zero)));
        assert!(matches!(attempt.token.returned, Some(Ok(()))));
        assert!(attempt.token.seed.iter().all(|byte| *byte == 0));
        assert!(attempt.token.key.is_none() && attempt.next.is_none());
        assert_eq!(root.refusal(), Some(OperationError::CreditOwned));
        assert_eq!(root.prepare_endpoint(1, &scope, seat, 5), Err(OperationError::CreditOwned));
        let other = SeatId::all().nth(1).unwrap();
        assert_eq!(root.reserve_counter(1, &scope, other), Err(OperationError::CreditOwned));
        assert!(root.counter(1, &scope, other).is_none());
        assert_eq!(original.usage().unwrap(), charged);
        let attempt = root.endpoints.as_deref().unwrap();
        assert!(std::ptr::eq(identity, attempt));
        assert!(matches!(attempt.refusal(), Some(Refusal::Zero)) && attempt.next.is_none());
        assert_eq!(REQUEST_ENTRIES.with(std::cell::Cell::get), 1);
        assert_eq!(BACKEND_ENTRIES.with(std::cell::Cell::get), 0);
        assert!(SEED_FIXTURE.with(std::cell::Cell::take).is_none());
        drop(root); assert_eq!(original.usage().unwrap(), charged);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn collision_test_only_fixture_latches_root_and_keeps_both_same_epoch_attempts() {
        // Synthetic returned bytes ONLY; actual collision branch and Root latch.
        SEED_FIXTURE.with(|fixture| fixture.set(None));
        REQUEST_ENTRIES.with(|count| count.set(0)); BACKEND_ENTRIES.with(|count| count.set(0));
        let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
        host.install_host_original(&mut Some(original::controls::fixture_fit())).unwrap();
        let original = host.host_original.reference().unwrap();
        let mut root = super::super::Root::reserve(&original).unwrap();
        let scope = ScopeControl::source_control(); let seat = SeatId::all().next().unwrap();
        root.reserve_counter(1, &scope, seat).unwrap();
        let before = original.usage().unwrap();
        SEED_FIXTURE.with(|fixture| fixture.set(Some([19; 32])));
        root.prepare_endpoint(1, &scope, seat, 5).unwrap();
        let first_identity = std::ptr::from_ref(root.endpoints.as_deref().unwrap());
        SEED_FIXTURE.with(|fixture| fixture.set(Some([19; 32])));
        assert_eq!(root.prepare_endpoint(1, &scope, seat, 5), Err(OperationError::CreditOwned));
        let charged = original.usage().unwrap();
        assert_eq!(charged.consumed - before.consumed, 2 * Layout::new::<EndpointAttempt>().size());
        let second = root.endpoints.as_deref().unwrap(); let second_identity = std::ptr::from_ref(second);
        let first = second.next.as_deref().unwrap();
        assert!(std::ptr::eq(first_identity, first) && first.next.is_none());
        assert!(matches!(first.preparation, Some(Ok(()))));
        assert!(matches!(second.refusal(), Some(Refusal::Collision)));
        assert!(first.token.key.as_ref().unwrap().matches(second.token.key.as_ref().unwrap()));
        assert!(matches!(first.token.returned, Some(Ok(()))) && matches!(second.token.returned, Some(Ok(()))));
        assert_eq!(root.refusal(), Some(OperationError::CreditOwned));
        assert_eq!(root.prepare_endpoint(1, &scope, seat, 5), Err(OperationError::CreditOwned));
        let other = SeatId::all().nth(1).unwrap();
        assert_eq!(root.reserve_counter(1, &scope, other), Err(OperationError::CreditOwned));
        assert!(root.counter(1, &scope, other).is_none());
        assert_eq!(original.usage().unwrap(), charged);
        let second = root.endpoints.as_deref().unwrap();
        assert!(std::ptr::eq(second_identity, second));
        assert!(std::ptr::eq(first_identity, second.next.as_deref().unwrap()));
        assert!(matches!(second.refusal(), Some(Refusal::Collision)));
        assert_eq!(REQUEST_ENTRIES.with(std::cell::Cell::get), 2);
        assert_eq!(BACKEND_ENTRIES.with(std::cell::Cell::get), 0);
        assert!(SEED_FIXTURE.with(std::cell::Cell::take).is_none());
        drop(root); assert_eq!(original.usage().unwrap(), charged);
    }
}
