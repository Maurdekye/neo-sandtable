//! Private Host custody for fixed per-seat transport counters, m7ec4e50170f0.
//! No legacy map, public accessor, router/MCP constructor or task is connected.
//! Full Host fit, token/binding/task custody and lease production remain owed.

use super::{original, ScopeControl};
use crate::{mcp::SeatCounters, operation::OperationError};
use cna_core::ids::SeatId;
use std::{alloc::Layout, sync::{Arc, Weak}};

#[cfg(windows)]
mod token;
#[cfg(windows)]
pub(super) use token::Refusal as EndpointRefusal;

/// Borrowed binding facts and whole retained causes ONLY. No token/seed/key
/// reference, formatting, serialization or owned error copy crosses this view.
#[cfg(windows)]
pub(super) struct EndpointReadback<'a> {
    pub(super) number: i64,
    pub(super) scope: &'a Weak<ScopeControl>,
    pub(super) seat: SeatId,
    pub(super) epoch: u64,
    pub(super) preparation_observed: bool,
    pub(super) refusal: Option<&'a EndpointRefusal>,
    pub(super) entropy_error: Option<&'a getrandom::Error>,
}

#[cfg(windows)]
pub(super) struct Endpoints<'a> { next: Option<&'a token::EndpointAttempt> }

#[cfg(windows)]
impl<'a> Endpoints<'a> {
    pub(super) fn from_root(root: Option<&'a Root>) -> Self {
        Self { next: root.and_then(|root| root.endpoints.as_deref()) }
    }
}

#[cfg(windows)]
impl<'a> Iterator for Endpoints<'a> {
    type Item = EndpointReadback<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let attempt = self.next?; self.next = attempt.next.as_deref();
        Some(attempt.readback())
    }
}

pub(super) struct Root {
    original: original::HostRef,
    counters: Option<Box<CounterEntry>>,
    first_refusal: Option<OperationError>,
    #[cfg(windows)]
    endpoints: Option<Box<token::EndpointAttempt>>,
}

struct CounterEntry {
    number: i64,
    scope: Weak<ScopeControl>,
    seat: SeatId,
    counter: Arc<SeatCounters>,
    next: Option<Box<Self>>,
}

impl Drop for CounterEntry {
    fn drop(&mut self) {
        let mut next = self.next.take();
        while let Some(mut entry) = next { next = entry.next.take(); }
    }
}

impl Root {
    pub(super) fn owner_layout() -> Layout { Layout::new::<Self>() }

    /// These two allocations ONLY, not the complete per-attempt transport cost.
    pub(super) fn counter_bytes() -> Result<usize, OperationError> {
        Layout::new::<CounterEntry>().size()
            .checked_add(crate::driver::owned_driver_task::own_arc_layout::<SeatCounters>()?.size())
            .ok_or(OperationError::CaptureOverflow)
    }

    pub(super) fn reserve(original: &original::HostRef) -> Result<Box<Self>, OperationError> {
        original::prepay(original, Self::owner_layout())?;
        Ok(Box::new(Self { original: original.clone(), counters: None, first_refusal: None,
            #[cfg(windows)] endpoints: None }))
    }

    fn find(&self, number: i64, scope: &Arc<ScopeControl>, seat: SeatId) -> Option<&CounterEntry> {
        let supplied = Arc::downgrade(scope);
        let mut next = self.counters.as_deref();
        while let Some(entry) = next {
            if entry.number == number && entry.seat == seat
                && entry.scope.ptr_eq(&supplied)
            { return Some(entry); }
            next = entry.next.as_deref();
        }
        None
    }

    /// The original Host must validate its actual durable lease/context BEFORE
    /// this private cut. All fresh backing is charged before either allocation.
    /// This creates only a counter, never a seat endpoint/token/GO admission.
    pub(super) fn reserve_counter(&mut self, number: i64, scope: &Arc<ScopeControl>, seat: SeatId)
        -> Result<(), OperationError>
    {
        if let Some(error) = self.first_refusal { return Err(error); }
        let result = (|| {
            self.original.check()?;
            if number <= 0 || scope.is_closed() || self.find(number, scope, seat).is_some() {
                return Err(OperationError::CreditOwned);
            }
            original::prepay(&self.original, Layout::new::<CounterEntry>())?;
            original::prepay(&self.original,
                crate::driver::owned_driver_task::own_arc_layout::<SeatCounters>()?)?;
            let mut entry = Box::new(CounterEntry { number, scope: Arc::downgrade(scope), seat,
                counter: Arc::new(SeatCounters::default()), next: None });
            entry.next = self.counters.take();
            self.counters = Some(entry); // Whole actual owner before success.
            Ok(())
        })();
        if let Err(error) = result { self.first_refusal = Some(error); }
        result
    }

    /// Inline Arc clone only. This is not a new scope/member/endpoint capability
    /// or a public router accessor. The actual pointee stays Host charged.
    pub(super) fn counter(&self, number: i64, scope: &Arc<ScopeControl>, seat: SeatId)
        -> Option<Arc<SeatCounters>>
    {
        self.find(number, scope, seat).map(|entry| entry.counter.clone())
    }

    pub(super) fn refusal(&self) -> Option<OperationError> { self.first_refusal }

    /// Host-private preparation ONLY; no endpoint publication/server/GO. The
    /// actual Host lease/context gate precedes this just as for the counter.
    /// Each START/RESTART is a fresh charged retained attempt, never a refill.
    #[cfg(windows)]
    pub(super) fn prepare_endpoint(&mut self, number: i64, scope: &Arc<ScopeControl>,
        seat: SeatId, epoch: u64) -> Result<(), OperationError>
    {
        if let Some(error) = self.first_refusal { return Err(error); }
        let result = (|| {
            self.original.check()?;
            if number <= 0 || scope.is_closed() { return Err(OperationError::CreditOwned); }
            let mut previous = self.endpoints.as_deref();
            while let Some(entry) = previous {
                if entry.is_unobserved() { return Err(OperationError::CreditOwned); }
                previous = entry.next.as_deref();
            }
            let counter = self.counter(number, scope, seat).ok_or(OperationError::CreditOwned)?;
            let mut endpoint = token::EndpointAttempt::reserve(&self.original, number, scope, seat, epoch, counter)?;
            endpoint.next = self.endpoints.take();
            self.endpoints = Some(endpoint); // ACTUAL whole owner BEFORE RNG.
            let endpoint = self.endpoints.as_deref_mut().ok_or(OperationError::CreditOwned)?;
            endpoint.prepare().map_err(|_| OperationError::CreditOwned)
        })();
        if let Err(error) = result { self.first_refusal = Some(error); }
        result
    }
}
