//! Private between-turn boundary, not reader join, archive custody or settlement.
//! Lead mc3802726a48a: a canceled Windows read can remain Busy in Tokio's blocking
//! wrapper. Its bytes stay in that SAME ChildStdout/ChildStderr object. An ACK
//! means only no bytes delivered to the OLD Operation since its processed read.
//! Pipe objects must never be replaced/reopened/split at this boundary.
//!
//! Native reader helpers retain the SAME pipe objects across cooperative pause.
//! Prompt/resume/root adoption is still unfinished; no helper supplies an archive
//! join/custody/healthy witness or permission to replace the capture Operation.

use std::{
    future::Future,
    sync::{Arc, Mutex, MutexGuard},
};

use tokio::{sync::Notify, time::Instant};

use crate::operation::{Operation, ReaderIdentity};

/// Existing operator wait route must fit this creation-reserved inline space
/// BEFORE polling. It is not a new task/boxed future or a late allowance.
pub(super) const WAIT_BYTES: usize = 4096;

pub(crate) fn reserved_bytes() -> usize {
    // One allocation per ORIGINAL Operation, including Arc control header, Gate
    // mutex/Notify/state, retained/transient Arc headers and ChildProc request slot.
    // Physical child text already belongs to the child; only its Arc is cloned.
    std::mem::size_of::<Gate>()
        + 2 * std::mem::size_of::<usize>()
        + 5 * std::mem::size_of::<Option<Arc<Gate>>>()
        + std::mem::size_of::<Option<Binding>>()
        // Original state checkout plus bounded borrowed metadata and separately
        // retained child request projection overlap without a clone or lock.
        + std::mem::size_of::<ArchiveLease<'_>>()
        + std::mem::size_of::<BorrowedGate<'_>>()
        + std::mem::size_of::<BorrowedBinding<'_>>()
        // Operator wait plus BOTH reader read-or-pause/park futures overlap.
        + 3 * WAIT_BYTES
}

/// Already included in creation reserve, so ChildProc's later general header
/// admission subtracts these fields instead of charging them a second time.
pub(super) const fn child_header_bytes() -> usize {
    std::mem::size_of::<Option<Arc<Gate>>>() + std::mem::size_of::<Option<Binding>>()
}

#[derive(Clone)]
pub(super) struct Binding {
    original: ReaderIdentity,
    physical: Arc<str>,
    request: u64,
}

impl Binding {
    fn same(&self, other: &Self) -> bool {
        self.request == other.request
            && self.original.same(&other.original)
            && Arc::ptr_eq(&self.physical, &other.physical)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pipe {
    Stdout,
    Stderr,
}

impl Pipe {
    fn index(self) -> usize {
        match self {
            Self::Stdout => 0,
            Self::Stderr => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Refusal {
    Identity,
    Sequence,
    Bytes,
    Eof,
    ReadError,
    Deadline,
    Fit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Reading,
    Terminal,
    Requested,
    BothAcked,
    PrePrompt,
    Written,
    Failed(Refusal),
}

struct State {
    phase: Phase,
    binding: Option<Binding>,
    acknowledged: [bool; 2],
    processing: [bool; 2],
    physical: Option<Arc<str>>,
    child_exited: bool,
}

pub(crate) struct Gate {
    original: ReaderIdentity,
    deadline: Instant,
    state: Mutex<Option<State>>,
    changed: Notify,
}

// Normal access holds the lock only through existing non-I/O state checks.
// No method can mutate an archive-checked-out slot or read a neutral substitute.
struct StateAccess<'a>(MutexGuard<'a, Option<State>>);

impl std::ops::Deref for StateAccess<'_> {
    type Target = State;
    fn deref(&self) -> &State {
        self.0.as_ref().expect("checked live gate state")
    }
}

impl std::ops::DerefMut for StateAccess<'_> {
    fn deref_mut(&mut self) -> &mut State {
        self.0.as_mut().expect("checked live gate state")
    }
}

struct ArchiveLease<'a> {
    gate: &'a Gate,
    state: Option<State>,
}

impl Drop for ArchiveLease<'_> {
    fn drop(&mut self) {
        let mut slot = self.gate.state.lock().expect("reader handoff");
        assert!(slot.is_none());
        *slot = self.state.take();
        drop(slot);
        self.gate.changed.notify_waiters();
    }
}

#[derive(serde::Serialize)]
struct BorrowedBinding<'a> {
    generation: u64,
    request: &'a u64,
    physical: &'a str,
}

impl<'a> From<&'a Binding> for BorrowedBinding<'a> {
    fn from(binding: &'a Binding) -> Self {
        Self {
            generation: binding.original.generation(),
            request: &binding.request,
            physical: binding.physical.as_ref(),
        }
    }
}

// Debug encodes the retained absolute monotonic Instant identically in both
// passes. It is an opaque private clock value, never wall time or a new deadline.
struct BorrowedDeadline<'a>(&'a Instant);
impl std::fmt::Display for BorrowedDeadline<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.0, f)
    }
}
impl serde::Serialize for BorrowedDeadline<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[derive(serde::Serialize)]
struct BorrowedGate<'a> {
    generation: u64,
    deadline: BorrowedDeadline<'a>,
    phase: &'a Phase,
    binding: Option<BorrowedBinding<'a>>,
    acknowledged: &'a [bool; 2],
    processing: &'a [bool; 2],
    physical: Option<&'a str>,
    child_exited: &'a bool,
}

impl Gate {
    // Called only by ORIGINAL Operation creation after its complete reserve fits.
    pub(crate) fn new(original: ReaderIdentity, deadline: Instant) -> Self {
        Self {
            original,
            deadline,
            state: Mutex::new(Some(State {
                phase: Phase::Reading,
                binding: None,
                acknowledged: [false; 2],
                processing: [false; 2],
                physical: None,
                child_exited: false,
            })),
            changed: Notify::new(),
        }
    }

    fn lock_state(&self) -> Result<StateAccess<'_>, Refusal> {
        let slot = self.state.lock().expect("reader handoff");
        if slot.is_none() {
            return Err(Refusal::Sequence);
        }
        Ok(StateAccess(slot))
    }

    /// Caller establishes ALL actual original joins first. Attached metadata
    /// additionally requires the saved original child-exit fact and exact Arc
    /// association. This inspection lease supplies neither a join nor custody.
    fn borrow_archive(
        &self,
        original: &Operation,
    ) -> Result<ArchiveLease<'_>, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        if !std::ptr::eq(self, original.reader_handoff_ref())
            || self.identity(original).is_err()
            || self.deadline != original.allowance().deadline()
            || original.archive_plan().is_none()
        {
            return Err(ArchiveError::Identity);
        }
        let state = self
            .state
            .lock()
            .expect("reader handoff")
            .take()
            .ok_or(ArchiveError::Identity)?;
        Ok(ArchiveLease {
            gate: self,
            state: Some(state),
        })
    }

    fn check_archive_association(
        &self,
        state: &State,
        child: Option<(&Arc<str>, Option<&Binding>)>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        let child_request = match (state.physical.as_ref(), child) {
            (Some(saved), Some((physical, request)))
                if Arc::ptr_eq(saved, physical) && state.child_exited =>
            {
                request
            }
            (None, None) => None,
            _ => return Err(ArchiveError::Identity),
        };
        if state.physical.as_ref().is_some_and(|physical| {
            physical.len() != 36 || uuid::Uuid::parse_str(physical.as_ref()).is_err()
        }) {
            return Err(ArchiveError::Identity);
        }
        match (state.binding.as_ref(), child_request) {
            (Some(saved), Some(retained))
                if saved.same(retained)
                    && saved.original.same(&self.original)
                    && saved.request == 1
                    && state
                        .physical
                        .as_ref()
                        .is_some_and(|physical| Arc::ptr_eq(physical, &saved.physical)) => {}
            (None, None) => {}
            _ => return Err(ArchiveError::Identity),
        }
        Ok(())
    }

    /// Child traversal may enumerate retained bytes only with the SAME checked
    /// association. This borrows/restores state; it grants no release or resume.
    pub(super) fn check_archive_child(
        &self,
        original: &Operation,
        physical: &Arc<str>,
        request: Option<&Binding>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        let lease = self.borrow_archive(original)?;
        self.check_archive_association(
            lease.state.as_ref().expect("canonical gate checkout"),
            Some((physical, request)),
        )
    }

    pub(super) fn visit_archive(
        &self,
        original: &Operation,
        child: Option<(&Arc<str>, Option<&Binding>)>,
        sink: &mut impl super::archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::{EvidenceCategory, EvidenceRecord};
        let lease = self.borrow_archive(original)?;
        let state = lease.state.as_ref().expect("canonical gate checkout");
        self.check_archive_association(state, child)?;
        let child_request = child.and_then(|(_, request)| request);
        // Logical gate state records its SAVED physical association in payload,
        // with no synthesized read origin or current-child inference.
        sink.json(
            EvidenceRecord {
                operation: original,
                category: EvidenceCategory::Diagnostic,
                part: "reader_handoff_gate",
                index: 0,
                physical_child: None,
                origin: None,
            },
            &BorrowedGate {
                generation: self.original.generation(),
                deadline: BorrowedDeadline(&self.deadline),
                phase: &state.phase,
                binding: state.binding.as_ref().map(BorrowedBinding::from),
                acknowledged: &state.acknowledged,
                processing: &state.processing,
                physical: state.physical.as_deref(),
                child_exited: &state.child_exited,
            },
        )?;
        if let Some(request) = child_request {
            // Separately retained canonical child copy is another record, not a
            // newly collected/consumed ACK or reconstructed body.
            sink.json(
                EvidenceRecord {
                    operation: original,
                    category: EvidenceCategory::Diagnostic,
                    part: "child_pause_request",
                    index: 0,
                    physical_child: Some(request.physical.as_ref()),
                    origin: None,
                },
                &BorrowedBinding::from(request),
            )?;
        }
        Ok(())
    }

    fn identity(&self, original: &Operation) -> Result<(), Refusal> {
        if self.original.same(&original.reader_identity()) {
            Ok(())
        } else {
            Err(Refusal::Identity)
        }
    }

    /// Called only for the actual canonical session child before readers start.
    /// Preflight/helper children do not share this one Operation's pause gate.
    pub(super) fn attach(&self, original: &Operation, physical: &Arc<str>) -> Result<(), Refusal> {
        self.identity(original)?;
        let mut state = self.lock_state()?;
        if state.phase != Phase::Reading || state.physical.is_some() {
            return Err(Refusal::Sequence);
        }
        state.physical = Some(physical.clone());
        Ok(())
    }

    fn fail(state: &mut State, reason: Refusal) -> Refusal {
        if let Phase::Failed(first) = state.phase {
            first
        } else {
            state.phase = Phase::Failed(reason);
            reason
        }
    }

    fn bound(
        state: &mut State,
        binding: &Binding,
        now: Instant,
        deadline: Instant,
    ) -> Result<(), Refusal> {
        if let Phase::Failed(first) = state.phase {
            return Err(first);
        }
        if state.binding.as_ref().is_none_or(|old| !old.same(binding)) {
            return Err(Self::fail(state, Refusal::Identity));
        }
        if now >= deadline {
            return Err(Self::fail(state, Refusal::Deadline));
        }
        Ok(())
    }

    /// Actual parser terminal processing hook only; this is not a completed or
    /// healthy receipt. Request is impossible before this hook or during a read's
    /// unprocessed delivery. A new Operation gets its own creation-time gate.
    pub(super) fn terminal_processed(&self, original: &Operation) -> Result<(), Refusal> {
        self.identity(original)?;
        let mut state = self.lock_state()?;
        if let Phase::Failed(first) = state.phase {
            return Err(first);
        }
        if state.phase != Phase::Reading {
            return Err(Refusal::Sequence);
        }
        state.phase = Phase::Terminal;
        Ok(())
    }

    pub(super) fn request(
        &self,
        original: &Operation,
        physical: &Arc<str>,
    ) -> Result<Binding, Refusal> {
        self.identity(original)?;
        let mut state = self.lock_state()?;
        if let Phase::Failed(first) = state.phase {
            return Err(first);
        }
        if state.phase != Phase::Terminal {
            return Err(Refusal::Sequence);
        }
        if state
            .physical
            .as_ref()
            .is_none_or(|child| !Arc::ptr_eq(child, physical))
        {
            return Err(Self::fail(&mut state, Refusal::Identity));
        }
        if Instant::now() >= self.deadline {
            return Err(Self::fail(&mut state, Refusal::Deadline));
        }
        // Read-winning race: the canonical chunk still owns every returned byte.
        // Request refuses; the caller preserves that owner and pauses the seat.
        if state.processing.iter().any(|busy| *busy) {
            return Err(Self::fail(&mut state, Refusal::Bytes));
        }
        let binding = Binding {
            original: self.original.clone(),
            physical: physical.clone(),
            request: 1,
        };
        state.binding = Some(binding.clone());
        state.phase = Phase::Requested;
        drop(state);
        self.changed.notify_waiters();
        Ok(binding)
    }

    /// Reader obtains the SAME request token after observing a pause. It must
    /// still restore its fixed chunk through the cooperative cancel-safe path
    /// before ACK. Generic read guard Drop/abort is never sufficient.
    pub(super) fn requested(&self) -> Option<Binding> {
        let state = self.lock_state().ok()?;
        matches!(
            state.phase,
            Phase::Requested | Phase::BothAcked | Phase::PrePrompt
        )
        .then(|| state.binding.as_ref().expect("installed request").clone())
    }

    /// Borrowed reader-side select branch. Enable before observing request; no
    /// pipe read/lock/callback is held here and normal idle reads acquire no timer.
    pub(super) async fn wait_request(&self, original: &Operation) -> Result<Binding, Refusal> {
        self.identity(original)?;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.lock_state()?;
                match state.phase {
                    Phase::Failed(first) => return Err(first),
                    Phase::Requested | Phase::BothAcked => {
                        return Ok(state.binding.as_ref().expect("installed pause").clone());
                    }
                    Phase::PrePrompt | Phase::Written => return Err(Refusal::Sequence),
                    Phase::Reading | Phase::Terminal => {}
                }
            }
            changed.await;
        }
    }

    /// Pipe remains owned by its reader future during this wait. An error is not
    /// permission to drop that pipe: caller retains uncertain chunk ownership and
    /// needs the original supervisor's separately observed child/reader closure.
    pub(super) async fn wait_resume(&self, binding: &Binding) -> Result<bool, Refusal> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.lock_state()?;
                if state.child_exited {
                    return Ok(false);
                }
                Self::bound(&mut state, binding, Instant::now(), self.deadline)?;
                if state.phase == Phase::PrePrompt {
                    return Ok(true);
                }
                if !matches!(state.phase, Phase::Requested | Phase::BothAcked) {
                    return Err(Self::fail(&mut state, Refusal::Sequence));
                }
            }
            tokio::select! {
                _ = &mut changed => {},
                _ = tokio::time::sleep_until(self.deadline) => {},
            }
        }
    }

    pub(super) fn boundary_active(&self) -> bool {
        match self.lock_state() {
            Ok(state) => !matches!(state.phase, Phase::Reading | Phase::Written),
            Err(_) => true,
        }
    }

    pub(super) fn acknowledge(&self, binding: &Binding, pipe: Pipe) -> Result<(), Refusal> {
        let mut state = self.lock_state()?;
        let result = (|| {
            Self::bound(&mut state, binding, Instant::now(), self.deadline)?;
            if state.phase != Phase::Requested
                || state.acknowledged[pipe.index()]
                || state.processing[pipe.index()]
            {
                return Err(Self::fail(&mut state, Refusal::Sequence));
            }
            state.acknowledged[pipe.index()] = true;
            if state.acknowledged == [true; 2] {
                state.phase = Phase::BothAcked;
            }
            Ok(())
        })();
        drop(state);
        self.changed.notify_waiters();
        result
    }

    /// Call immediately after retaining an actual returned count in the OLD
    /// fixed chunk, before charge, parser, queue or normal transcript delivery.
    /// Err leaves the entire chunk with its OLD owner, operator-only evidence.
    /// Keep processing set through the last processed byte, including terminal.
    pub(super) fn delivered(
        &self,
        original: &Operation,
        pipe: Pipe,
        count: usize,
    ) -> Result<(), Refusal> {
        self.identity(original)?;
        let mut state = self.lock_state()?;
        let result = if count == 0 {
            Err(Self::fail(&mut state, Refusal::Eof))
        } else if matches!(
            state.phase,
            Phase::Requested | Phase::BothAcked | Phase::PrePrompt
        ) {
            Err(Self::fail(&mut state, Refusal::Bytes))
        } else if let Phase::Failed(first) = state.phase {
            Err(first)
        } else if state.processing[pipe.index()] {
            Err(Self::fail(&mut state, Refusal::Sequence))
        } else {
            state.processing[pipe.index()] = true;
            Ok(())
        };
        drop(state);
        self.changed.notify_waiters();
        result
    }

    pub(super) fn processed(&self, original: &Operation, pipe: Pipe) -> Result<(), Refusal> {
        self.identity(original)?;
        let mut state = self.lock_state()?;
        if let Phase::Failed(first) = state.phase {
            return Err(first);
        }
        if !state.processing[pipe.index()] {
            return Err(Refusal::Sequence);
        }
        state.processing[pipe.index()] = false;
        Ok(())
    }

    pub(super) fn read_error(&self, original: &Operation) -> Result<(), Refusal> {
        self.identity(original)?;
        let mut state = self.lock_state()?;
        let first = Self::fail(&mut state, Refusal::ReadError);
        drop(state);
        self.changed.notify_waiters();
        Err(first)
    }

    fn ready(&self, binding: &Binding, now: Instant) -> Result<bool, Refusal> {
        let mut state = self.lock_state()?;
        Self::bound(&mut state, binding, now, self.deadline)?;
        match state.phase {
            Phase::Requested => Ok(false),
            Phase::BothAcked => Ok(true),
            _ => Err(Self::fail(&mut state, Refusal::Sequence)),
        }
    }

    /// Synchronous original-deadline prerequisite, never an ACK collector or a
    /// healthy release. A missing second ACK does not renew/reset the request.
    pub(super) fn check_paused(
        &self,
        binding: &Binding,
        physical: &Arc<str>,
    ) -> Result<(), Refusal> {
        if !Arc::ptr_eq(&binding.physical, physical) {
            return Err(Refusal::Identity);
        }
        if self.ready(binding, Instant::now())? {
            Ok(())
        } else {
            Err(Refusal::Sequence)
        }
    }

    /// Unboxed, cancelable operator wait. Register/enable BEFORE checking state;
    /// enqueue/closure race cannot disappear between check and suspension.
    /// Deadline remains ORIGINAL. Drop changes no ACK/evidence/ownership state.
    pub(super) fn wait_both<'a>(
        &'a self,
        binding: &'a Binding,
    ) -> Result<impl Future<Output = Result<(), Refusal>> + 'a, Refusal> {
        let future = async move {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if self.ready(binding, Instant::now())? {
                    return Ok(());
                }
                tokio::select! {
                    _ = &mut changed => {},
                    _ = tokio::time::sleep_until(self.deadline) => {
                        return self.ready(binding, Instant::now()).and_then(|_| Err(Refusal::Deadline));
                    }
                }
            }
        };
        if std::mem::size_of_val(&future) > WAIT_BYTES {
            return Err(Refusal::Fit);
        }
        Ok(future)
    }

    /// Actual resume hook only AFTER BOTH ACKs. Reads resume on EXACT SAME pipe
    /// objects; buffered bytes before a successful prompt write still fail.
    pub(super) fn resume_before_prompt(&self, binding: &Binding) -> Result<(), Refusal> {
        let mut state = self.lock_state()?;
        Self::bound(&mut state, binding, Instant::now(), self.deadline)?;
        if state.phase != Phase::BothAcked {
            return Err(Self::fail(&mut state, Refusal::Sequence));
        }
        state.phase = Phase::PrePrompt;
        drop(state);
        self.changed.notify_waiters();
        Ok(())
    }

    /// Called only after actual new prompt write succeeds. This marker supplies
    /// no membership proof: subsequent bytes require parser turn-sequence checks.
    pub(super) fn prompt_written(&self, binding: &Binding) -> Result<(), Refusal> {
        let mut state = self.lock_state()?;
        Self::bound(&mut state, binding, Instant::now(), self.deadline)?;
        if state.phase != Phase::PrePrompt {
            return Err(Self::fail(&mut state, Refusal::Sequence));
        }
        state.phase = Phase::Written;
        drop(state);
        self.changed.notify_waiters();
        Ok(())
    }

    /// Called only AFTER the supervisor's actual original child wait, never a
    /// kill request or timeout. This lets parked readers drain their SAME pipes;
    /// it is not either reader's join or a failed boundary's recovery/healthy ACK.
    pub(super) fn child_exit_observed(&self) {
        // Attached checkout requires this fact already true and ALL readers
        // joined by its caller. Never replace checked-out canonical metadata.
        if let Ok(mut state) = self.lock_state() {
            state.child_exited = true;
        }
        self.changed.notify_waiters();
    }

    pub(super) async fn wait_original_exit(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.lock_state().is_ok_and(|state| state.child_exited) {
                return;
            }
            changed.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::{Allowance, OPERATION_BYTES};
    use std::time::Duration;

    fn archive_original() -> Operation {
        let root = std::env::temp_dir().join("synthetic-handoff-archive-no-io");
        crate::failed_evidence::CampaignArchive::new(
            &root.join("campaigns/11111111-1111-4111-8111-111111111111.sqlite"),
            &root.join("sandbox"),
            &root.join("run"),
            &[root.join("isolation")],
        )
        .unwrap()
        .operation(
            "axis.commander".parse().unwrap(),
            7,
            "22222222-2222-4222-8222-222222222222".into(),
            Allowance::required(Some(Instant::now() + Duration::from_secs(60))).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn joined_handoff_metadata_restores_first_failure_and_distinct_request_on_writer_refusal() {
        use super::super::archive_stdout::RecordSink;
        use crate::failed_evidence::{ArchiveError, EvidenceRecord};
        struct Refusing<'a> {
            gate: &'a Gate,
            original: &'a Operation,
            request: &'a Binding,
            rows: Vec<(&'static str, serde_json::Value)>,
        }
        impl RecordSink for Refusing<'_> {
            fn bytes(&mut self, _: EvidenceRecord<'_>, _: &[u8]) -> Result<(), ArchiveError> {
                panic!("metadata uses borrowed JSON");
            }
            fn json<T: serde::Serialize + ?Sized>(
                &mut self,
                record: EvidenceRecord<'_>,
                value: &T,
            ) -> Result<(), ArchiveError> {
                assert!(record.operation.same_instance(self.original));
                assert!(record.origin.is_none());
                assert!(self.gate.borrow_archive(self.original).is_err());
                assert_eq!(
                    self.gate.acknowledge(self.request, Pipe::Stderr),
                    Err(Refusal::Sequence)
                );
                assert!(self.gate.boundary_active()); // No lock across callback or neutral state.
                self.rows
                    .push((record.part, serde_json::to_value(value).unwrap())); // Test reference only.
                if record.part == "child_pause_request" {
                    Err(ArchiveError::Filesystem)
                } else {
                    Ok(())
                }
            }
        }
        let op = archive_original();
        let gate = op.reader_handoff_ref();
        let physical: Arc<str> = "33333333-3333-4333-8333-333333333333".into();
        gate.attach(&op, &physical).unwrap();
        gate.terminal_processed(&op).unwrap();
        let binding = gate.request(&op, &physical).unwrap();
        gate.acknowledge(&binding, Pipe::Stdout).unwrap();
        assert_eq!(gate.delivered(&op, Pipe::Stderr, 1), Err(Refusal::Bytes));
        gate.child_exit_observed(); // Synthetic fact, NOT an actual child/reader join witness.
        let before = op.captured_bytes();
        let mut sink = Refusing {
            gate,
            original: &op,
            request: &binding,
            rows: Vec::new(),
        };
        assert!(matches!(
            gate.visit_archive(&op, Some((&physical, Some(&binding))), &mut sink),
            Err(ArchiveError::Filesystem)
        ));
        assert_eq!(sink.rows.len(), 2);
        assert_eq!(sink.rows[0].0, "reader_handoff_gate");
        assert_eq!(
            sink.rows[0].1["phase"],
            serde_json::json!({"failed":"bytes"})
        );
        assert_eq!(
            sink.rows[0].1["acknowledged"],
            serde_json::json!([true, false])
        );
        assert_eq!(sink.rows[0].1["binding"], sink.rows[1].1);
        assert_eq!(sink.rows[1].0, "child_pause_request");
        let state = gate.lock_state().unwrap();
        assert_eq!(state.phase, Phase::Failed(Refusal::Bytes));
        assert!(state.binding.as_ref().unwrap().same(&binding));
        assert!(Arc::ptr_eq(state.physical.as_ref().unwrap(), &physical));
        drop(state);
        assert_eq!(
            gate.acknowledge(&binding, Pipe::Stderr),
            Err(Refusal::Bytes)
        );
        assert_eq!(op.captured_bytes(), before);
    }

    #[test]
    fn handoff_archive_rejects_stale_or_missing_associations_before_any_output() {
        let op = archive_original();
        let foreign = archive_original();
        let gate = op.reader_handoff_ref();
        let physical: Arc<str> = "33333333-3333-4333-8333-333333333333".into();
        gate.attach(&op, &physical).unwrap();
        gate.terminal_processed(&op).unwrap();
        let binding = gate.request(&op, &physical).unwrap();
        let same_text_other_arc: Arc<str> = physical.as_ref().into();
        let mut stale = binding.clone(); // Existing retained-copy field, test mutation only.
        stale.original = foreign.reader_identity();
        let mut coverage = crate::failed_evidence::EvidenceCoverageBuilder::new(&op).unwrap();
        assert!(gate.borrow_archive(&foreign).is_err());
        // Even attached matching identity cannot replace actual exit prerequisite.
        assert!(
            gate.visit_archive(&op, Some((&physical, Some(&binding))), &mut coverage)
                .is_err()
        );
        gate.child_exit_observed(); // Synthetic fact only.
        assert!(gate.visit_archive(&op, None, &mut coverage).is_err());
        assert!(
            gate.visit_archive(
                &op,
                Some((&same_text_other_arc, Some(&binding))),
                &mut coverage
            )
            .is_err()
        );
        assert!(
            gate.visit_archive(&op, Some((&physical, None)), &mut coverage)
                .is_err()
        );
        assert!(
            gate.visit_archive(&op, Some((&physical, Some(&stale))), &mut coverage)
                .is_err()
        );
        assert!(coverage.finish().is_err()); // Every refusal preceded its first record.
        assert!(
            gate.lock_state()
                .unwrap()
                .binding
                .as_ref()
                .unwrap()
                .same(&binding)
        );
        // Absolute expiry and overflow do not erase failed metadata or renew
        // prompt/reader effects. Inspection still uses the SAME original reserve.
        assert_eq!(gate.ready(&binding, gate.deadline), Err(Refusal::Deadline));
        op.charge(OPERATION_BYTES - op.captured_bytes()).unwrap();
        assert!(op.charge(1).is_err());
        let before = op.captured_bytes();
        let mut expired = crate::failed_evidence::EvidenceCoverageBuilder::new(&op).unwrap();
        gate.visit_archive(&op, Some((&physical, Some(&binding))), &mut expired)
            .unwrap();
        expired.finish().unwrap();
        assert_eq!(
            gate.lock_state().unwrap().phase,
            Phase::Failed(Refusal::Deadline)
        );
        assert_eq!(op.captured_bytes(), before);
    }

    #[test]
    fn unwritten_gate_absolute_deadline_serialization_and_unwind_keep_original_state() {
        use crate::failed_evidence::{ArchiveEncoder, EvidenceCoverageBuilder};
        let op = archive_original();
        let gate = op.reader_handoff_ref();
        let before = op.captured_bytes();
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _lease = gate.borrow_archive(&op).unwrap();
            panic!("synthetic synchronous inspection unwind");
        }));
        assert!(interrupted.is_err());
        let mut coverage = EvidenceCoverageBuilder::new(&op).unwrap();
        gate.visit_archive(&op, None, &mut coverage).unwrap();
        let coverage = coverage.finish().unwrap();
        let encode = || {
            let mut output = Vec::new(); // Test reference, no archive file or real thread.
            let mut encoder = ArchiveEncoder::new(&op, coverage, &mut output).unwrap();
            gate.visit_archive(&op, None, &mut encoder).unwrap();
            encoder.finish().unwrap();
            output
        };
        let first = encode();
        assert_eq!(encode(), first); // Retained absolute deadline, no remaining-time recalc.
        let row: serde_json::Value =
            serde_json::from_slice(first.split(|b| *b == b'\n').nth(1).unwrap()).unwrap();
        assert_eq!(row["payload"]["phase"], "reading");
        assert_eq!(row["payload"]["generation"], 7);
        assert!(row["payload"]["physical"].is_null());
        assert!(row["payload"]["binding"].is_null());
        assert_eq!(row["payload"]["child_exited"], false);
        assert_eq!(
            row["payload"]["processing"],
            serde_json::json!([false, false])
        );
        assert_eq!(op.captured_bytes(), before);
        assert_eq!(gate.lock_state().unwrap().phase, Phase::Reading);
        // No refund/renewal/new prompt, actual joins, accepted custody or healthy settlement.
    }

    fn original() -> Operation {
        Operation::new(
            "old".into(),
            7,
            Allowance::required(Some(Instant::now() + Duration::from_secs(10))).unwrap(),
        )
        .unwrap()
    }

    fn request(op: &Operation) -> (Arc<Gate>, Binding) {
        let gate = op.reader_handoff();
        let physical = Arc::from("physical-child");
        gate.attach(op, &physical).unwrap();
        gate.terminal_processed(op).unwrap();
        let binding = gate.request(op, &physical).unwrap();
        (gate, binding)
    }

    #[test]
    fn creation_reserve_and_single_use_bind_original_not_text() {
        let op = original();
        let used = op.captured_bytes();
        assert!(used >= reserved_bytes());
        let (gate, binding) = request(&op);
        assert!(gate.request(&op, &binding.physical).is_err());
        assert_eq!(gate.terminal_processed(&original()), Err(Refusal::Identity));
        assert!(binding.original.same(&op.reader_identity()));
        assert_eq!(op.captured_bytes(), used);
        op.charge(OPERATION_BYTES - used).unwrap();
        assert!(op.charge(1).is_err());
        assert!(gate.requested().unwrap().same(&binding));
        assert_eq!(op.captured_bytes(), OPERATION_BYTES); // no renewal/refund
    }

    #[test]
    fn both_sides_of_pause_read_race_refuse_without_consuming_old_bytes() {
        for pipe in [Pipe::Stdout, Pipe::Stderr] {
            let op = original();
            let gate = op.reader_handoff();
            let physical = Arc::from("child");
            gate.attach(&op, &physical).unwrap();
            let bytes = [b'x'; 3]; // caller's canonical fixed chunk; gate never takes it
            gate.delivered(&op, pipe, bytes.len()).unwrap();
            gate.terminal_processed(&op).unwrap();
            assert!(matches!(gate.request(&op, &physical), Err(Refusal::Bytes)));
            assert_eq!(bytes, [b'x'; 3]);
            let op = original();
            let (gate, binding) = request(&op);
            assert_eq!(gate.delivered(&op, pipe, bytes.len()), Err(Refusal::Bytes));
            assert_eq!(gate.acknowledge(&binding, pipe), Err(Refusal::Bytes));
            assert_eq!(bytes, [b'x'; 3]);
        }
    }

    #[test]
    fn stale_original_physical_generation_or_request_ack_fails_closed() {
        for corruption in 0..4 {
            let op = original();
            let (gate, binding) = request(&op);
            let mut stale = binding.clone();
            match corruption {
                0 => stale.original = original().reader_identity(),
                1 => stale.physical = Arc::from(binding.physical.as_ref()),
                2 => {
                    stale.original = Operation::new("old".into(), 8, op.allowance())
                        .unwrap()
                        .reader_identity()
                }
                _ => stale.request += 1,
            }
            assert_eq!(
                gate.acknowledge(&stale, Pipe::Stdout),
                Err(Refusal::Identity)
            );
            assert_eq!(
                gate.acknowledge(&binding, Pipe::Stderr),
                Err(Refusal::Identity)
            );
        }
    }

    #[test]
    fn one_ack_eof_error_and_original_deadline_never_mean_ready() {
        let op = original();
        let (gate, binding) = request(&op);
        gate.acknowledge(&binding, Pipe::Stdout).unwrap();
        assert_eq!(gate.ready(&binding, Instant::now()), Ok(false));
        assert_eq!(gate.resume_before_prompt(&binding), Err(Refusal::Sequence));
        for pipe in [Pipe::Stdout, Pipe::Stderr] {
            let op = original();
            let (gate, binding) = request(&op);
            assert_eq!(gate.delivered(&op, pipe, 0), Err(Refusal::Eof));
            assert_eq!(gate.acknowledge(&binding, pipe), Err(Refusal::Eof));
        }
        let op = original();
        let (gate, binding) = request(&op);
        assert_eq!(gate.read_error(&op), Err(Refusal::ReadError));
        assert_eq!(
            gate.acknowledge(&binding, Pipe::Stdout),
            Err(Refusal::ReadError)
        );
        let op = original();
        let (gate, binding) = request(&op);
        assert_eq!(gate.ready(&binding, gate.deadline), Err(Refusal::Deadline));
        assert_eq!(
            gate.acknowledge(&binding, Pipe::Stderr),
            Err(Refusal::Deadline)
        );
    }

    #[test]
    fn both_ack_is_only_boundary_and_pre_prompt_bytes_still_fail() {
        for pipe in [Pipe::Stdout, Pipe::Stderr] {
            let op = original();
            let (gate, binding) = request(&op);
            gate.acknowledge(&binding, Pipe::Stdout).unwrap();
            gate.acknowledge(&binding, Pipe::Stderr).unwrap();
            assert_eq!(gate.ready(&binding, Instant::now()), Ok(true));
            gate.resume_before_prompt(&binding).unwrap();
            assert_eq!(gate.delivered(&op, pipe, 1), Err(Refusal::Bytes));
            assert_eq!(gate.prompt_written(&binding), Err(Refusal::Bytes));
            assert_eq!(
                op.start_evidence(),
                crate::operation::StartEvidence::NotStarted
            );
        }
        let op = original();
        let gate = op.reader_handoff();
        let physical = Arc::from("child");
        gate.attach(&op, &physical).unwrap();
        gate.delivered(&op, Pipe::Stdout, 1).unwrap();
        gate.processed(&op, Pipe::Stdout).unwrap();
        gate.terminal_processed(&op).unwrap();
        let binding = gate.request(&op, &physical).unwrap();
        gate.acknowledge(&binding, Pipe::Stdout).unwrap();
        gate.acknowledge(&binding, Pipe::Stderr).unwrap();
        gate.resume_before_prompt(&binding).unwrap();
        gate.prompt_written(&binding).unwrap();
        assert!(gate.requested().is_none());
        assert_eq!(
            gate.acknowledge(&binding, Pipe::Stdout),
            Err(Refusal::Sequence)
        );
    }

    #[tokio::test]
    async fn canceled_registered_wait_keeps_single_ack_and_later_notification() {
        use std::{
            pin::pin,
            task::{Context, Poll, Wake, Waker},
        };
        struct CountWake(std::sync::atomic::AtomicUsize);
        impl Wake for CountWake {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let op = original();
        let used = op.captured_bytes();
        let (gate, binding) = request(&op);
        gate.acknowledge(&binding, Pipe::Stdout).unwrap();
        {
            let mut future = pin!(gate.wait_both(&binding).unwrap());
            let waker = Waker::from(Arc::new(CountWake(std::sync::atomic::AtomicUsize::new(0))));
            assert!(matches!(
                future.as_mut().poll(&mut Context::from_waker(&waker)),
                Poll::Pending
            ));
        } // cancellation removes only borrowed waiter, not retained request/ACK
        assert_eq!(gate.ready(&binding, Instant::now()), Ok(false));
        {
            let mut future = pin!(gate.wait_both(&binding).unwrap());
            let counter = Arc::new(CountWake(std::sync::atomic::AtomicUsize::new(0)));
            let waker = Waker::from(counter.clone());
            let mut context = Context::from_waker(&waker);
            assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
            gate.acknowledge(&binding, Pipe::Stderr).unwrap();
            assert!(counter.0.load(std::sync::atomic::Ordering::SeqCst) > 0);
            assert!(matches!(
                future.as_mut().poll(&mut context),
                Poll::Ready(Ok(()))
            ));
        }
        gate.wait_both(&binding).unwrap().await.unwrap();
        assert_eq!(op.captured_bytes(), used);
        assert!(gate.requested().unwrap().same(&binding));
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "real Windows child: only first separately admitted post-build validation slot"]
    async fn windows_canceled_inflight_reads_return_bytes_on_next_same_objects() {
        use std::process::Stdio;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut child = tokio::process::Command::new("powershell.exe")
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command",
                "$null=[Console]::ReadLine(); [Console]::Out.Write('out'); [Console]::Error.Write('err')"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .kill_on_drop(true).spawn().unwrap();
        // Take each pipe ONCE. Both canceled .read futures and all subsequent
        // reads borrow EXACT SAME objects; no reopen/rewrap/split or replacement.
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let result = tokio::time::timeout_at(deadline, async {
            let mut out = [0u8; 4];
            let mut err = [0u8; 4];
            tokio::select! {
                biased;
                value = stdout.read(&mut out) => return Err(format!("unexpected pre-go stdout: {value:?}")),
                _ = tokio::time::sleep(Duration::from_millis(20)) => {},
            }
            tokio::select! {
                biased;
                value = stderr.read(&mut err) => return Err(format!("unexpected pre-go stderr: {value:?}")),
                _ = tokio::time::sleep(Duration::from_millis(20)) => {},
            }
            // Cancellation has delivered zero bytes; it has NOT joined or proved
            // an empty pipe/no OS read. Child writes only after this input line.
            stdin.write_all(b"go\n").await.map_err(|e| e.to_string())?;
            stdin.shutdown().await.map_err(|e| e.to_string())?;
            let mut out_len = 0;
            let mut err_len = 0;
            loop {
                let n = stdout.read(&mut out[out_len..]).await.map_err(|e| e.to_string())?;
                if n == 0 { break; }
                out_len += n;
                if out_len == out.len() { return Err("extra stdout".into()); }
            }
            loop {
                let n = stderr.read(&mut err[err_len..]).await.map_err(|e| e.to_string())?;
                if n == 0 { break; }
                err_len += n;
                if err_len == err.len() { return Err("extra stderr".into()); }
            }
            if &out[..out_len] != b"out" || &err[..err_len] != b"err" {
                return Err("same-object bytes missing or changed".into());
            }
            Ok::<(), String>(())
        }).await;
        if !matches!(&result, Ok(Ok(()))) {
            let _ = child.start_kill();
        }
        // Actual child wait is separate from successful next-read bytes. The
        // admitted outer runner must supervise timeout/abort to original ALLJOIN;
        // this test supplies no archive custody or healthy settlement witness.
        let exit = tokio::time::timeout_at(deadline, child.wait()).await;
        assert!(
            matches!(exit, Ok(Ok(status)) if status.success()),
            "{exit:?}"
        );
        assert!(matches!(result, Ok(Ok(()))), "{result:?}");
    }
}
