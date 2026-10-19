//! Private pre-Operation physical storage, not a launcher or complete fit.
//! No child/task can be installed here yet. FIFO remains separately staged;
//! the existing stderr ring MOVEs here, alongside the reader-failure pair.

use std::{
    alloc::Layout,
    sync::{Arc, Mutex},
};
use tokio::{
    process::{Child, ChildStderr, ChildStdin, ChildStdout},
    sync::{Mutex as PipeMutex, OwnedMutexGuard},
    task::JoinHandle,
};

use super::{
    ChildFrame, FrameAssembly, PendingStderrLines, StderrCapture, StderrStats, StdoutChunk,
};
use crate::{failed_evidence::ArchiveError, operation::Operation};

#[allow(dead_code)]
pub(super) struct PendingPhysicalChild {
    original: Option<Operation>,
    bind_started: bool,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout_pipe: PendingPipe<ChildStdout>,
    stderr_pipe: PendingPipe<ChildStderr>,
    // The existing stdout/stderr pair, fixed destinations BEFORE either spawn.
    // No Vec backing or additional reader/error/transport slot is introduced.
    readers: [Option<JoinHandle<std::io::Result<()>>>; 2],
    stdout_assembly: Arc<Mutex<FrameAssembly>>,
    stdout_pending: Arc<Mutex<Option<ChildFrame>>>,
    stdout_chunk: Arc<Mutex<Option<StdoutChunk>>>,
    stderr_chunk: Arc<Mutex<Option<StdoutChunk>>>,
    stderr_stats: Arc<Mutex<StderrStats>>,
    reader_failures: Arc<Mutex<super::reader_failure::ReaderFailures>>,
    capture: Arc<Mutex<Option<Operation>>>,
    stderr_tail: Arc<Mutex<PendingStderrTail>>,
}

/// Canonical destination for one pipe, precreated BEFORE Operation/effects.
/// A read lease borrows the pipe IN this cell across await; it never takes the
/// pipe into a task-local Option that cancellation could close. No installer,
/// production read or activation path exists yet. No public escape API.
struct PendingPipe<T> {
    storage: Arc<PipeMutex<Option<T>>>,
}

#[allow(dead_code)] // Actual reader futures and full original F still unready.
impl<T> PendingPipe<T> {
    fn new() -> Self {
        Self {
            storage: Arc::new(PipeMutex::new(None)),
        }
    }

    fn is_none(&self) -> bool {
        // Busy is UNKNOWN, never proof that a physical pipe is absent.
        self.storage.try_lock().is_ok_and(|slot| slot.is_none())
    }

    fn is_some(&self) -> bool {
        !self.is_none()
    }

    fn creation_layout(&self) -> Result<Layout, ArchiveError> {
        if Arc::strong_count(&self.storage) != 1 || Arc::weak_count(&self.storage) != 0 {
            return Err(ArchiveError::Identity);
        }
        let slot = self
            .storage
            .try_lock()
            .map_err(|_| ArchiveError::Identity)?;
        if slot.is_some() {
            return Err(ArchiveError::Identity);
        }
        Ok(Layout::for_value(self.storage.as_ref()))
    }

    /// Internal STORAGE lease only. Future caller must first establish SAME
    /// original binding/admission. Nonblocking try ONLY: no waiter/lock future
    /// is queued, no task/read/Operation check/charge/notification is performed.
    fn try_lease(&self) -> Result<PipeLease<T>, ArchiveError> {
        let guard = self
            .storage
            .clone()
            .try_lock_owned()
            .map_err(|_| ArchiveError::Identity)?;
        if guard.is_none() {
            return Err(ArchiveError::Identity);
        }
        Ok(PipeLease { guard })
    }
}

/// Only a guarded mutable borrow; the Option slot stays private. While the
/// root retains this cell, dropping the guard unlocks and leaves its SAME
/// pipe in place. This is NOT proof that the root survives until actual joins.
/// Drop the lease BEFORE any capture/stats/failure locks or overflow publish.
#[allow(dead_code)]
struct PipeLease<T> {
    guard: OwnedMutexGuard<Option<T>>,
}

#[allow(dead_code)]
impl<T> PipeLease<T> {
    fn pipe(&mut self) -> &mut T {
        self.guard
            .as_mut()
            .expect("preflighted canonical pipe lease")
    }
}

/// One ring in either pending or bound form, never two evidence collections.
/// Both fixed header destinations exist before Operation; ring backing MOVEs.
struct PendingStderrTail {
    transfer_started: bool,
    pending: Option<PendingStderrLines>,
    capture: Option<StderrCapture>,
}

impl PendingStderrTail {
    fn stage_ring(
        &mut self,
        candidate: &mut Option<PendingStderrLines>,
        expected: Layout,
    ) -> Result<(), ArchiveError> {
        if self.transfer_started {
            return Err(ArchiveError::Identity);
        }
        self.transfer_started = true; // Refusal cannot reopen/retry this target.
        if self.pending.is_some()
            || self.capture.is_some()
            || candidate
                .as_ref()
                .ok_or(ArchiveError::Identity)?
                .creation_layout()?
                != expected
        {
            return Err(ArchiveError::Identity);
        }
        self.pending = candidate.take(); // WHOLE canonical ring, no allocation/copy.
        Ok(())
    }

    fn pristine(&self) -> bool {
        self.transfer_started
            && self.capture.is_none()
            && self
                .pending
                .as_ref()
                .is_some_and(|ring| ring.creation_layout().is_ok())
    }

    /// Outer caller checks allowance BEFORE and AFTER all guards. Inside this
    /// lock, only pristine metadata checks then infallible WHOLE ring MOVE.
    fn bind_original(&mut self, original: &Operation) -> Result<(), ArchiveError> {
        if !self.pristine() {
            return Err(ArchiveError::Identity);
        }
        let PendingStderrLines {
            rows, capacity_pin, ..
        } = self
            .pending
            .take()
            .expect("preflighted canonical pending stderr ring");
        self.capture = Some(StderrCapture {
            lines: rows,
            owned_pin: Some(super::OwnedStderrPin {
                capacity: capacity_pin,
                original: original.clone(),
                first_refusal: None,
            }),
            partial: Vec::new(),
            partial_operation: None,
            dropping: false,
            stats_operation: None,
        }); // FIRST retained bound destination, before outer postcheck/unlock.
        Ok(())
    }
}

#[allow(dead_code)]
pub(super) struct PhysicalStorageLayouts {
    pub(super) owner_inline: Layout,
    pub(super) bodies: [Layout; 5],
    pub(super) referenced_headers_bytes: usize,
    pub(super) arc_counter_bytes: usize,
    pub(super) reader_failures_body: Layout,
    pub(super) reader_failures_counter_bytes: usize,
    pub(super) capture_body: Layout,
    pub(super) capture_counter_bytes: usize,
    pub(super) stderr_tail_body: Layout,
    pub(super) stderr_tail_counter_bytes: usize,
    pub(super) pipe_bodies: [Layout; 2],
    pub(super) pipe_counter_bytes: usize,
}

fn unique_empty<T>(
    owner: &Arc<Mutex<T>>,
    pristine: impl FnOnce(&T) -> bool,
) -> Result<Layout, ArchiveError> {
    if Arc::strong_count(owner) != 1 || Arc::weak_count(owner) != 0 {
        return Err(ArchiveError::Identity);
    }
    let body = owner.try_lock().map_err(|_| ArchiveError::Identity)?;
    if !pristine(&body) {
        return Err(ArchiveError::Identity);
    }
    Ok(Layout::for_value(owner.as_ref()))
}

fn pristine_chunk(chunk: &Option<StdoutChunk>) -> bool {
    chunk.as_ref().is_some_and(|chunk| {
        chunk.count == 0
            && chunk.cursor == 0
            && !chunk.read_uncertain
            && !chunk.seen_accounted
            && chunk.operation.is_none()
            && chunk.bytes.iter().all(|byte| *byte == 0)
    })
}

#[allow(dead_code)]
impl PendingPhysicalChild {
    pub(super) fn new() -> Self {
        Self {
            original: None,
            bind_started: false,
            child: None,
            stdin: None,
            stdout_pipe: PendingPipe::new(),
            stderr_pipe: PendingPipe::new(),
            readers: [None, None],
            stdout_assembly: Arc::new(Mutex::new(FrameAssembly(Vec::new()))),
            stdout_pending: Arc::new(Mutex::new(None)),
            stdout_chunk: Arc::new(Mutex::new(Some(StdoutChunk::empty()))),
            stderr_chunk: Arc::new(Mutex::new(Some(StdoutChunk::empty()))),
            stderr_stats: Arc::new(Mutex::new(StderrStats::default())),
            reader_failures: Arc::new(Mutex::new(super::reader_failure::ReaderFailures::pending())),
            capture: Arc::new(Mutex::new(None)), // No placeholder Operation/State.
            stderr_tail: Arc::new(Mutex::new(PendingStderrTail {
                transfer_started: false,
                pending: None,
                capture: None,
            })),
        }
    }

    pub(super) fn creation_layout(&self) -> Result<PhysicalStorageLayouts, ArchiveError> {
        if self.bind_started {
            return Err(ArchiveError::Identity);
        }
        self.pristine_layout()
    }

    pub(super) fn stage_stderr_ring(
        &mut self,
        candidate: &mut Option<PendingStderrLines>,
        expected: Layout,
    ) -> Result<(), ArchiveError> {
        if self.bind_started
            || self.original.is_some()
            || Arc::strong_count(&self.stderr_tail) != 1
            || Arc::weak_count(&self.stderr_tail) != 0
        {
            return Err(ArchiveError::Identity);
        }
        self.stderr_tail
            .try_lock()
            .map_err(|_| ArchiveError::Identity)?
            .stage_ring(candidate, expected)
    }

    /// Private trusted caller supplies ONLY the actual canonical creation Ok.
    /// A failed/unfinished attempt never reopens, including if still unbound.
    /// No resource install/spawn, State creation, debit or wake occurs here.
    pub(super) fn bind_original(&mut self, original: &Operation) -> Result<(), ArchiveError> {
        if self.bind_started {
            return Err(ArchiveError::Identity);
        }
        self.bind_started = true; // FIRST, before any fallible prerequisite.
        self.pristine_layout()?;
        original.check()?; // All ten cell guards have already been released.
        {
            // Capture -> tail (then unlock tail), then capture -> failure. NO
            // allowance/meter/wake/callback under guards; no repair or rebind.
            let mut capture = self
                .capture
                .try_lock()
                .map_err(|_| ArchiveError::Identity)?;
            if capture.is_some() {
                return Err(ArchiveError::Identity);
            }
            self.stderr_tail
                .try_lock()
                .map_err(|_| ArchiveError::Identity)?
                .bind_original(original)?;
            self.reader_failures
                .try_lock()
                .map_err(|_| ArchiveError::Identity)?
                .bind_pending(original)?;
            *capture = Some(original.clone()); // SAME State handle, not new State.
        } // BOTH guards released before owner association/postcheck; no wake.
        self.original = Some(original.clone()); // SAME State handle, no copied body.
        original.check()?; // Postcheck outside every storage lock; no replacement.
        Ok(())
    }

    /// Unused owned-reader precursor. Checkout alone grants no pipe read or
    /// byte acceptance: actual original-F/task admission and the m5be refusal
    /// publication protocol remain separate, unfinished adoption gates.
    /// Capture -> failure -> chunk exclusion; no check/charge/notify/callback
    /// or await under locks. All refusal checks precede the WHOLE chunk MOVE.
    pub(super) fn checkout_read_storage(
        &self,
        expected: &Operation,
        stream: super::reader_failure::ReaderStream,
    ) -> Result<(Operation, super::StdoutRead), ArchiveError> {
        if !self.bind_started
            || !self
                .original
                .as_ref()
                .is_some_and(|owner| owner.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        let capture = self
            .capture
            .try_lock()
            .map_err(|_| ArchiveError::Identity)?;
        if !capture
            .as_ref()
            .is_some_and(|owner| owner.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        let failures = self
            .reader_failures
            .try_lock()
            .map_err(|_| ArchiveError::Identity)?;
        failures.check_owned_checkout(expected)?;
        let owner = match stream {
            super::reader_failure::ReaderStream::Stdout => &self.stdout_chunk,
            super::reader_failure::ReaderStream::Stderr => &self.stderr_chunk,
        };
        let mut slot = owner.try_lock().map_err(|_| ArchiveError::Identity)?;
        let chunk = slot.as_ref().ok_or(ArchiveError::Identity)?;
        if chunk.has_pending()
            || chunk.count > chunk.bytes.len()
            || chunk.cursor > chunk.count
            || chunk
                .operation
                .as_ref()
                .is_some_and(|old| !old.same_instance(expected))
            || (chunk.operation.is_none()
                && (chunk.count != 0
                    || chunk.cursor != 0
                    || chunk.seen_accounted
                    || chunk.bytes.iter().any(|byte| *byte != 0)))
        {
            return Err(ArchiveError::Identity);
        }
        let mut chunk = slot.take().expect("preflighted owned reader chunk");
        chunk.count = 0;
        chunk.cursor = 0;
        chunk.read_uncertain = true;
        chunk.seen_accounted = false;
        chunk.operation = Some(expected.clone());
        let read = super::StdoutRead {
            owner: owner.clone(),
            chunk: Some(chunk),
        };
        drop(slot);
        drop(failures);
        drop(capture);
        Ok((expected.clone(), read)) // ALL guards gone before any eventual read.
    }

    /// Storage projection ONLY: actual physical child/reader joins, census and
    /// archive custody remain separate prerequisites, not inferred here.
    /// Exclusive ownership replaces guards; ALL stderr sources are validated
    /// by the existing borrowed visitor before its first callback. No live
    /// allowance check may hide evidence after overflow or deadline expiry.
    pub(super) fn visit_stderr_storage(
        &mut self,
        expected: &Operation,
        physical_child: &str,
        sink: &mut impl super::archive_stdout::RecordSink,
    ) -> Result<(), ArchiveError> {
        use super::archive_stdout::unique_storage;
        if !self.bind_started
            || !self
                .original
                .as_ref()
                .is_some_and(|owner| owner.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        let capture = unique_storage(&mut self.capture)?;
        if !capture
            .as_ref()
            .is_some_and(|owner| owner.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        let tail = unique_storage(&mut self.stderr_tail)?;
        if !tail.transfer_started || tail.pending.is_some() {
            return Err(ArchiveError::Identity);
        }
        let retained = tail.capture.as_ref().ok_or(ArchiveError::Identity)?;
        // This path requires the canonical owned pin, never a legacy tail.
        if retained.owned_pin.is_none() {
            return Err(ArchiveError::Identity);
        }
        let stats = unique_storage(&mut self.stderr_stats)?;
        let chunk = unique_storage(&mut self.stderr_chunk)?
            .as_ref()
            .ok_or(ArchiveError::Identity)?;
        super::archive_stderr::BorrowedStderr {
            operation: expected,
            physical_child,
            retained,
            stats,
            chunk,
        }
        .visit(sink) // No pop, replacement, copy, refund or release on failure.
    }

    fn pristine_layout(&self) -> Result<PhysicalStorageLayouts, ArchiveError> {
        if self.original.is_some()
            || self.child.is_some()
            || self.stdin.is_some()
            || self.stdout_pipe.is_some()
            || self.stderr_pipe.is_some()
            || self.readers.iter().any(Option::is_some)
        {
            return Err(ArchiveError::Identity);
        }
        let bodies = [
            unique_empty(&self.stdout_assembly, |assembly| {
                assembly.0.is_empty() && assembly.0.capacity() == 0
            })?,
            unique_empty(&self.stdout_pending, Option::is_none)?,
            unique_empty(&self.stdout_chunk, pristine_chunk)?,
            unique_empty(&self.stderr_chunk, pristine_chunk)?,
            unique_empty(&self.stderr_stats, |stats| {
                stats.seen_bytes == 0
                    && stats.retained_bytes == 0
                    && stats.discarded_bytes == 0
                    && !stats.truncated
            })?,
        ];
        let referenced_headers_bytes = bodies
            .iter()
            .try_fold(0usize, |bytes, layout| bytes.checked_add(layout.size()))
            .ok_or(ArchiveError::Fit)?;
        // Existing explicit two-counter policy PER actual Arc body; not private
        // Arc allocation/padding/RSS proof. Inline arrays/chunk bytes are INSIDE
        // actual owner/body Layouts, so never added as another allocation.
        let arc_counter_bytes = bodies
            .len()
            .checked_mul(2)
            .and_then(|count| count.checked_mul(std::mem::size_of::<usize>()))
            .ok_or(ArchiveError::Fit)?;
        let reader_failures_body = unique_empty(
            &self.reader_failures,
            super::reader_failure::ReaderFailures::pristine_pending,
        )?;
        let reader_failures_counter_bytes = 2usize
            .checked_mul(std::mem::size_of::<usize>())
            .ok_or(ArchiveError::Fit)?;
        let capture_body = unique_empty(&self.capture, Option::is_none)?;
        let capture_counter_bytes = 2usize
            .checked_mul(std::mem::size_of::<usize>())
            .ok_or(ArchiveError::Fit)?;
        let stderr_tail_body = unique_empty(&self.stderr_tail, PendingStderrTail::pristine)?;
        let stderr_tail_counter_bytes = 2usize
            .checked_mul(std::mem::size_of::<usize>())
            .ok_or(ArchiveError::Fit)?;
        let pipe_bodies = [
            self.stdout_pipe.creation_layout()?,
            self.stderr_pipe.creation_layout()?,
        ];
        let pipe_counter_bytes = pipe_bodies
            .len()
            .checked_mul(2)
            .and_then(|count| count.checked_mul(std::mem::size_of::<usize>()))
            .ok_or(ArchiveError::Fit)?;
        Ok(PhysicalStorageLayouts {
            owner_inline: Layout::for_value(self),
            bodies,
            referenced_headers_bytes,
            arc_counter_bytes,
            reader_failures_body,
            reader_failures_counter_bytes,
            capture_body,
            capture_counter_bytes,
            stderr_tail_body,
            stderr_tail_counter_bytes,
            pipe_bodies,
            pipe_counter_bytes,
        })
    }
}

#[cfg(test)]
mod source_controls {
    // UNCOMPILED/UNRUN; no child/task/native effects or positive original Fit.
    use super::*;

    fn pending_with_ring() -> PendingPhysicalChild {
        let mut pending = PendingPhysicalChild::new();
        let mut ring = Some(PendingStderrLines::new().unwrap());
        let layout = ring.as_ref().unwrap().creation_layout().unwrap();
        pending.stage_stderr_ring(&mut ring, layout).unwrap();
        assert!(ring.is_none());
        pending
    }

    #[test] // UNCOMPILED/UNRUN: actual EMPTY cells, no OS pipe/child/task.
    fn canonical_pipe_cells_have_actual_preoperation_bodies_and_move_without_reallocation() {
        let pending = pending_with_ring();
        let stdout_pointer = Arc::as_ptr(&pending.stdout_pipe.storage);
        let stderr_pointer = Arc::as_ptr(&pending.stderr_pipe.storage);
        let layouts = pending.creation_layout().unwrap();
        assert_eq!(
            layouts.pipe_bodies,
            [
                Layout::for_value(pending.stdout_pipe.storage.as_ref()),
                Layout::for_value(pending.stderr_pipe.storage.as_ref()),
            ]
        );
        assert_eq!(
            layouts.pipe_bodies[0],
            Layout::new::<PipeMutex<Option<ChildStdout>>>()
        );
        assert_eq!(
            layouts.pipe_bodies[1],
            Layout::new::<PipeMutex<Option<ChildStderr>>>()
        );
        assert_eq!(layouts.pipe_counter_bytes, 4 * std::mem::size_of::<usize>());
        let moved = pending; // WHOLE owner; the SAME destinations, not copied handles.
        assert_eq!(Arc::as_ptr(&moved.stdout_pipe.storage), stdout_pointer);
        assert_eq!(Arc::as_ptr(&moved.stderr_pipe.storage), stderr_pointer);
        assert!(moved.stdout_pipe.is_none() && moved.stderr_pipe.is_none());
        assert!(moved.original.is_none() && moved.child.is_none());
        assert_eq!(
            moved.creation_layout().unwrap().pipe_bodies,
            layouts.pipe_bodies
        );
    }

    #[test]
    fn shared_weak_or_busy_pipe_destinations_cannot_bind_or_reopen() {
        for defect in 0..3 {
            let mut pending = pending_with_ring();
            let original = operation();
            let pointer = Arc::as_ptr(&pending.stdout_pipe.storage);
            let mut alias = None;
            let mut weak = None;
            match defect {
                0 => alias = Some(pending.stdout_pipe.storage.clone()),
                1 => weak = Some(Arc::downgrade(&pending.stderr_pipe.storage)),
                _ => {}
            }
            // Clone solely to hold an independent test guard while mutably
            // borrowing the owner. It also remains a deliberately shared cell.
            let busy_owner = (defect == 2).then(|| pending.stdout_pipe.storage.clone());
            let held = busy_owner.as_ref().map(|owner| owner.try_lock().unwrap());
            assert!(matches!(
                pending.creation_layout(),
                Err(ArchiveError::Identity)
            ));
            assert_eq!(
                pending.bind_original(&original),
                Err(ArchiveError::Identity)
            );
            assert!(pending.bind_started && pending.original.is_none());
            assert!(pending.reader_failures.lock().unwrap().pristine_pending());
            drop(held);
            drop(busy_owner);
            drop((alias, weak));
            assert_eq!(
                pending.bind_original(&original),
                Err(ArchiveError::Identity)
            );
            assert_eq!(Arc::as_ptr(&pending.stdout_pipe.storage), pointer);
            assert!(pending.child.is_none() && pending.readers.iter().all(Option::is_none));
        }
        let cell = PendingPipe::<u8>::new(); // Private representation fixture only.
        *cell.storage.try_lock().unwrap() = Some(7);
        assert!(matches!(
            cell.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        assert_eq!(*cell.storage.try_lock().unwrap(), Some(7));
    }

    #[test] // UNCOMPILED/UNRUN; synthetic resource, no IO/Tokio runtime/task.
    fn pipe_lease_cancel_and_unwind_leave_same_resource_in_retained_canonical_cell() {
        use std::future::Future;
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct SyntheticPipe {
            bytes: [u8; 3],
            drops: Arc<AtomicUsize>,
        }
        impl Drop for SyntheticPipe {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }
        let drops = Arc::new(AtomicUsize::new(0));
        let cell = PendingPipe::<SyntheticPipe>::new();
        *cell.storage.try_lock().unwrap() = Some(SyntheticPipe {
            bytes: [0xff, 0, b'x'],
            drops: drops.clone(),
        });
        let owner = Arc::as_ptr(&cell.storage);
        let mut lease = cell.try_lease().unwrap();
        let pipe = std::ptr::from_mut(lease.pipe());
        assert!(!cell.is_none()); // Busy is not absent.
        assert!(matches!(cell.try_lease(), Err(ArchiveError::Identity)));
        {
            let mut future = std::pin::pin!(async move {
                let _lease = lease;
                std::future::pending::<()>().await;
            });
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(future.as_mut().poll(&mut context).is_pending());
            assert!(cell.storage.try_lock().is_err());
        } // Actual future Drop cancels the pending borrow; canonical root lives.
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        let mut next = cell.try_lease().unwrap();
        assert_eq!(std::ptr::from_mut(next.pipe()), pipe);
        assert_eq!(next.pipe().bytes, [0xff, 0, b'x']);
        drop(next);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut held = cell.try_lease().unwrap();
            assert_eq!(std::ptr::from_mut(held.pipe()), pipe);
            panic!("synthetic pipe lease unwind");
        }));
        assert!(result.is_err());
        let mut after = cell.try_lease().unwrap();
        assert_eq!(std::ptr::from_mut(after.pipe()), pipe);
        assert_eq!(after.pipe().bytes, [0xff, 0, b'x']);
        assert_eq!(Arc::as_ptr(&cell.storage), owner);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(after);
        assert_eq!(Arc::strong_count(&cell.storage), 1);
        drop(cell); // ONLY destruction of the final retained owner drops it.
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn pipe_lease_empty_refusal_keeps_cell_and_typed_guards_are_send() {
        fn requires_send<T: Send>() {}
        requires_send::<PipeLease<ChildStdout>>();
        requires_send::<PipeLease<ChildStderr>>();
        let cell = PendingPipe::<u8>::new();
        let pointer = Arc::as_ptr(&cell.storage);
        assert!(matches!(cell.try_lease(), Err(ArchiveError::Identity)));
        assert!(cell.is_none());
        assert_eq!(Arc::strong_count(&cell.storage), 1);
        assert_eq!(Arc::as_ptr(&cell.storage), pointer);
        assert_eq!(
            cell.creation_layout().unwrap(),
            Layout::new::<PipeMutex<Option<u8>>>()
        );
    }

    #[test] // UNCOMPILED/UNRUN: actual ring MOVE only; no task/reader/State effect.
    fn canonical_stderr_ring_moves_once_with_all_spare_and_same_pin_before_binding() {
        let mut pending = PendingPhysicalChild::new();
        assert!(matches!(
            pending.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        let backing = Vec::with_capacity(crate::operation::STDERR_LINES + 7);
        let pin = backing.capacity();
        let mut ring = Some(PendingStderrLines {
            rows: std::collections::VecDeque::from(backing),
            capacity_pin: pin,
            used: false,
            original: None,
            first_refusal: None,
        });
        let pointer = ring.as_ref().unwrap().rows.as_slices().0.as_ptr();
        let layout = ring.as_ref().unwrap().creation_layout().unwrap();
        assert_eq!(
            layout.size(),
            pin * std::mem::size_of::<super::super::StderrLine>()
        );
        pending.stage_stderr_ring(&mut ring, layout).unwrap();
        assert!(ring.is_none());
        let fixed = pending.creation_layout().unwrap();
        assert_eq!(
            fixed.stderr_tail_body,
            Layout::for_value(pending.stderr_tail.as_ref())
        );
        assert_eq!(
            fixed.stderr_tail_counter_bytes,
            2 * std::mem::size_of::<usize>()
        );
        {
            let tail = pending.stderr_tail.lock().unwrap();
            assert!(tail.capture.is_none());
            assert_eq!(
                tail.pending.as_ref().unwrap().rows.as_slices().0.as_ptr(),
                pointer
            );
            assert_eq!(tail.pending.as_ref().unwrap().capacity_pin, pin);
        }
        let original = operation();
        let charged = original.captured_bytes();
        pending.bind_original(&original).unwrap();
        {
            let tail = pending.stderr_tail.lock().unwrap();
            assert!(tail.pending.is_none());
            let capture = tail.capture.as_ref().unwrap();
            assert_eq!(capture.lines.as_slices().0.as_ptr(), pointer);
            assert_eq!(capture.lines.capacity(), pin);
            assert_eq!(capture.owned_pin.as_ref().unwrap().capacity, pin);
            assert!(
                capture
                    .owned_pin
                    .as_ref()
                    .unwrap()
                    .original
                    .same_instance(&original)
            );
            assert!(capture.check_legacy_archive_release().is_err());
        }
        let mut replacement = Some(PendingStderrLines::new().unwrap());
        let replacement_layout = replacement.as_ref().unwrap().creation_layout().unwrap();
        assert_eq!(
            pending.stage_stderr_ring(&mut replacement, replacement_layout),
            Err(ArchiveError::Identity)
        );
        assert!(replacement.is_some());
        assert_eq!(original.captured_bytes(), charged);
    }

    #[test]
    fn failed_ring_transfer_keeps_candidate_and_closes_target_without_repair() {
        let mut pending = PendingPhysicalChild::new();
        let mut ring = Some(PendingStderrLines::new().unwrap());
        let layout = ring.as_ref().unwrap().creation_layout().unwrap();
        let pointer = ring.as_ref().unwrap().rows.as_slices().0.as_ptr();
        ring.as_mut().unwrap().capacity_pin += 1; // TEST-ONLY foreign metadata.
        assert_eq!(
            pending.stage_stderr_ring(&mut ring, layout),
            Err(ArchiveError::Identity)
        );
        assert!(ring.is_some());
        assert_eq!(ring.as_ref().unwrap().rows.as_slices().0.as_ptr(), pointer);
        assert!(pending.stderr_tail.lock().unwrap().transfer_started);
        assert!(pending.stderr_tail.lock().unwrap().pending.is_none());
        let mut replacement = Some(PendingStderrLines::new().unwrap());
        let next_layout = replacement.as_ref().unwrap().creation_layout().unwrap();
        assert_eq!(
            pending.stage_stderr_ring(&mut replacement, next_layout),
            Err(ArchiveError::Identity)
        );
        assert!(replacement.is_some());
        assert!(matches!(
            pending.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        assert!(pending.original.is_none() && pending.child.is_none());
    }

    fn operation() -> Operation {
        // Identity/arithmetic fixture ONLY, not canonical original creation Ok.
        Operation::new(
            "pending-physical-binding-fixture".into(),
            7,
            crate::operation::Allowance::required(Some(
                tokio::time::Instant::now() + std::time::Duration::from_secs(60),
            ))
            .unwrap(),
        )
        .unwrap()
    }

    // TEST-ONLY synthetic bytes, not production admission, Fit or join proof.
    #[test] // UNCOMPILED/UNRUN: no pipe/future polling/task or byte admission.
    fn owned_checkout_returns_both_guards_and_preserves_cancelled_read_prefix() {
        use super::super::reader_failure::ReaderStream;
        let mut pending = pending_with_ring();
        let original = operation();
        pending.bind_original(&original).unwrap();
        let charged = original.captured_bytes();
        let stdout_owner = Arc::as_ptr(&pending.stdout_chunk);
        let stderr_owner = Arc::as_ptr(&pending.stderr_chunk);
        let (stdout_op, mut stdout) = pending
            .checkout_read_storage(&original, ReaderStream::Stdout)
            .unwrap();
        let (stderr_op, stderr) = pending
            .checkout_read_storage(&original, ReaderStream::Stderr)
            .unwrap();
        assert!(stdout_op.same_instance(&original) && stderr_op.same_instance(&original));
        assert!(pending.stdout_chunk.lock().unwrap().is_none());
        assert!(pending.stderr_chunk.lock().unwrap().is_none());
        // Every exclusion is released before returning either read guard.
        assert!(pending.capture.try_lock().is_ok());
        assert!(pending.reader_failures.try_lock().is_ok());
        stdout.buffer()[..3].copy_from_slice(&[0xff, 0, b'x']); // Synthetic returned prefix.
        stdout.returned(3);
        original.overflow();
        drop((stdout, stderr)); // Existing StdoutRead Drop, no ACK/acceptance/refund.
        let out = pending.stdout_chunk.lock().unwrap();
        let out = out.as_ref().unwrap();
        assert_eq!(out.count, 3);
        assert_eq!(out.cursor, 0);
        assert_eq!(&out.bytes[..3], &[0xff, 0, b'x']);
        assert!(out.operation.as_ref().unwrap().same_instance(&original));
        assert!(!out.read_uncertain);
        let err = pending.stderr_chunk.lock().unwrap();
        let err = err.as_ref().unwrap();
        assert!(err.read_uncertain && err.operation.as_ref().unwrap().same_instance(&original));
        assert_eq!(Arc::as_ptr(&pending.stdout_chunk), stdout_owner);
        assert_eq!(Arc::as_ptr(&pending.stderr_chunk), stderr_owner);
        assert_eq!(original.captured_bytes(), charged);
        assert!(pending.child.is_none() && pending.readers.iter().all(Option::is_none));
    }

    #[test]
    fn owned_checkout_foreign_unbound_failure_and_busy_refuse_before_chunk_move() {
        use super::super::reader_failure::ReaderStream;
        for defect in 0..5 {
            let mut pending = pending_with_ring();
            let original = operation();
            pending.bind_original(&original).unwrap();
            let foreign = operation();
            match defect {
                0 => *pending.capture.lock().unwrap() = Some(foreign.clone()),
                1 => pending.original = None,
                2 => {
                    pending
                        .reader_failures
                        .lock()
                        .unwrap()
                        .retain_io_for_original(
                            ReaderStream::Stderr,
                            &original,
                            std::io::ErrorKind::BrokenPipe.into(),
                        )
                        .unwrap();
                }
                _ => {}
            }
            let busy_capture = (defect == 3).then(|| pending.capture.lock().unwrap());
            let busy_failure = (defect == 4).then(|| pending.reader_failures.lock().unwrap());
            for stream in [ReaderStream::Stdout, ReaderStream::Stderr] {
                assert!(matches!(
                    pending.checkout_read_storage(&original, stream),
                    Err(ArchiveError::Identity)
                ));
            }
            assert!(
                pending
                    .stdout_chunk
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .operation
                    .is_none()
            );
            assert!(
                pending
                    .stderr_chunk
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .operation
                    .is_none()
            );
            drop((busy_capture, busy_failure));
            if defect == 2 {
                assert!(pending.reader_failures.lock().unwrap().has_failure());
            }
        }
        let mut pending = pending_with_ring();
        let original = operation();
        pending.bind_original(&original).unwrap();
        assert!(matches!(
            pending.checkout_read_storage(&operation(), ReaderStream::Stdout),
            Err(ArchiveError::Identity)
        ));
        assert!(
            pending
                .stdout_chunk
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .operation
                .is_none()
        );
    }

    #[test]
    fn owned_checkout_keeps_invalid_uncertain_foreign_or_checked_out_chunks_unchanged() {
        use super::super::reader_failure::ReaderStream;
        for defect in 0..6 {
            let mut pending = pending_with_ring();
            let original = operation();
            pending.bind_original(&original).unwrap();
            let mut checked_out = None;
            {
                let mut slot = pending.stdout_chunk.lock().unwrap();
                let chunk = slot.as_mut().unwrap();
                match defect {
                    0 => chunk.read_uncertain = true,
                    1 => chunk.cursor = 1,
                    2 => chunk.operation = Some(operation()),
                    3 => chunk.count = chunk.bytes.len() + 1,
                    4 => chunk.bytes[0] = 9, // Unassociated bytes are NOT pristine.
                    _ => checked_out = slot.take(),
                }
            }
            assert!(matches!(
                pending.checkout_read_storage(&original, ReaderStream::Stdout),
                Err(ArchiveError::Identity)
            ));
            let slot = pending.stdout_chunk.lock().unwrap();
            match defect {
                0 => assert!(slot.as_ref().unwrap().read_uncertain),
                1 => assert_eq!(slot.as_ref().unwrap().cursor, 1),
                2 => assert!(
                    !slot
                        .as_ref()
                        .unwrap()
                        .operation
                        .as_ref()
                        .unwrap()
                        .same_instance(&original)
                ),
                3 => assert_eq!(
                    slot.as_ref().unwrap().count,
                    crate::operation::STDOUT_CHUNK_BYTES + 1
                ),
                4 => assert_eq!(slot.as_ref().unwrap().bytes[0], 9),
                _ => assert!(slot.is_none() && checked_out.is_some()),
            }
        }
    }

    // TEST-ONLY synthetic bytes, not production admission, Fit or join proof.
    fn archive_fixture() -> (PendingPhysicalChild, Operation) {
        let mut pending = pending_with_ring();
        let original = operation();
        pending.bind_original(&original).unwrap();
        let tail = Arc::get_mut(&mut pending.stderr_tail)
            .unwrap()
            .get_mut()
            .unwrap();
        let capture = tail.capture.as_mut().unwrap();
        capture.lines.push_back(super::super::StderrLine {
            operation: original.clone(),
            bytes: vec![0xff, b'\n'],
        });
        capture.partial = vec![0xfe];
        capture.partial_operation = Some(original.clone());
        capture.stats_operation = Some(original.clone());
        let stats = Arc::get_mut(&mut pending.stderr_stats)
            .unwrap()
            .get_mut()
            .unwrap();
        stats.seen_bytes = 3;
        stats.retained_bytes = 3;
        let chunk = Arc::get_mut(&mut pending.stderr_chunk)
            .unwrap()
            .get_mut()
            .unwrap()
            .as_mut()
            .unwrap();
        chunk.bytes[..3].copy_from_slice(&[0xfd, 0, b'x']);
        chunk.count = 3;
        chunk.cursor = 1;
        chunk.read_uncertain = true;
        chunk.operation = Some(original.clone());
        (pending, original)
    }

    #[derive(Default)]
    struct StorageSink {
        parts: Vec<String>,
        pointers: Vec<*const u8>,
        fail: bool,
    }
    impl super::super::archive_stdout::RecordSink for StorageSink {
        fn bytes(
            &mut self,
            record: crate::failed_evidence::EvidenceRecord<'_>,
            bytes: &[u8],
        ) -> Result<(), ArchiveError> {
            self.parts.push(record.part.to_owned());
            self.pointers.push(bytes.as_ptr()); // Witness only, never dereferenced.
            if self.fail {
                Err(ArchiveError::Filesystem)
            } else {
                Ok(())
            }
        }
        fn json<T: serde::Serialize + ?Sized>(
            &mut self,
            record: crate::failed_evidence::EvidenceRecord<'_>,
            _: &T,
        ) -> Result<(), ArchiveError> {
            self.parts.push(record.part.to_owned());
            if self.fail {
                Err(ArchiveError::Filesystem)
            } else {
                Ok(())
            }
        }
    }

    #[test] // UNCOMPILED/UNRUN: borrowed storage only, no physical joins.
    fn bound_stderr_borrows_original_sources_after_overflow_and_sink_failure() {
        let (mut pending, original) = archive_fixture();
        let (row, partial, pin) = {
            let tail = pending.stderr_tail.lock().unwrap();
            let capture = tail.capture.as_ref().unwrap();
            (
                capture.lines[0].bytes.as_ptr(),
                capture.partial.as_ptr(),
                capture.lines.capacity(),
            )
        };
        let chunk = pending
            .stderr_chunk
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .bytes
            .as_ptr();
        let charged = original.captured_bytes();
        original.overflow();
        let mut failed = StorageSink {
            fail: true,
            ..Default::default()
        };
        assert_eq!(
            pending.visit_stderr_storage(&original, "fixture-child", &mut failed),
            Err(ArchiveError::Filesystem)
        );
        assert_eq!(failed.parts, ["stderr_line"]);
        assert_eq!(failed.pointers, [row]);
        let mut sink = StorageSink::default();
        pending
            .visit_stderr_storage(&original, "fixture-child", &mut sink)
            .unwrap();
        assert_eq!(
            sink.parts,
            [
                "stderr_line",
                "stderr_partial",
                "stderr_stats",
                "stderr_read_state",
                "stderr_read_bytes"
            ]
        );
        assert_eq!(sink.pointers, [row, partial, chunk]);
        let tail = pending.stderr_tail.lock().unwrap();
        let capture = tail.capture.as_ref().unwrap();
        assert_eq!(capture.lines.len(), 1);
        assert_eq!(capture.lines.capacity(), pin);
        assert_eq!(capture.lines[0].bytes, [0xff, b'\n']);
        assert_eq!(capture.partial, [0xfe]);
        assert_eq!(capture.owned_pin.as_ref().unwrap().capacity, pin);
        assert_eq!(original.captured_bytes(), charged);
        assert!(pending.child.is_none() && pending.readers.iter().all(Option::is_none));
    }

    #[test] // A valid first row must NOT escape before a later source fails.
    fn bound_stderr_preflights_partial_stats_chunk_and_pin_before_first_callback() {
        for defect in 0..6 {
            let (mut pending, original) = archive_fixture();
            let foreign = operation();
            match defect {
                0 => {
                    pending
                        .stderr_tail
                        .lock()
                        .unwrap()
                        .capture
                        .as_mut()
                        .unwrap()
                        .partial_operation = Some(foreign)
                }
                1 => pending.stderr_stats.lock().unwrap().retained_bytes += 1,
                2 => {
                    pending
                        .stderr_chunk
                        .lock()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .cursor = 4
                }
                3 => {
                    pending
                        .stderr_tail
                        .lock()
                        .unwrap()
                        .capture
                        .as_mut()
                        .unwrap()
                        .owned_pin
                        .as_mut()
                        .unwrap()
                        .capacity += 1
                }
                4 => {
                    pending
                        .stderr_tail
                        .lock()
                        .unwrap()
                        .capture
                        .as_mut()
                        .unwrap()
                        .owned_pin = None
                }
                _ => {
                    pending
                        .stderr_chunk
                        .lock()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .operation = Some(foreign)
                }
            }
            let charged = original.captured_bytes();
            let mut sink = StorageSink::default();
            assert_eq!(
                pending.visit_stderr_storage(&original, "fixture-child", &mut sink),
                Err(ArchiveError::Identity)
            );
            assert!(sink.parts.is_empty());
            assert_eq!(
                pending
                    .stderr_tail
                    .lock()
                    .unwrap()
                    .capture
                    .as_ref()
                    .unwrap()
                    .lines
                    .len(),
                1
            );
            assert_eq!(original.captured_bytes(), charged);
        }
    }

    #[test]
    fn bound_stderr_shared_weak_poison_foreign_unbound_and_checkout_refuse_without_callback() {
        for defect in 0..6 {
            let (mut pending, original) = archive_fixture();
            let mut alias = None;
            let mut weak = None;
            let mut checked_out = None;
            match defect {
                0 => alias = Some(pending.stderr_tail.clone()),
                1 => weak = Some(Arc::downgrade(&pending.stderr_stats)),
                2 => {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let _held = pending.stderr_chunk.lock().unwrap();
                        panic!("synthetic stderr storage poison");
                    }));
                    assert!(result.is_err());
                }
                3 => *pending.capture.lock().unwrap() = Some(operation()),
                4 => pending.original = None,
                _ => checked_out = pending.stderr_chunk.lock().unwrap().take(),
            }
            let mut sink = StorageSink::default();
            assert_eq!(
                pending.visit_stderr_storage(&original, "fixture-child", &mut sink),
                Err(ArchiveError::Identity)
            );
            assert!(sink.parts.is_empty());
            assert_eq!(
                pending
                    .stderr_tail
                    .lock()
                    .unwrap()
                    .capture
                    .as_ref()
                    .unwrap()
                    .lines
                    .len(),
                1
            );
            if defect == 2 {
                assert!(pending.stderr_chunk.is_poisoned());
            }
            if defect == 5 {
                assert!(pending.stderr_chunk.lock().unwrap().is_none());
            }
            drop((alias, weak, checked_out)); // No repair or archive release.
        }
    }

    #[test]
    fn same_instance_binding_is_one_shot_without_tasks_resources_or_credit_change() {
        let mut pending = pending_with_ring(); // BEFORE fixture Op.
        let before_layout = pending.creation_layout().unwrap();
        let original = operation();
        let foreign = operation(); // Same text/epoch, a different State.
        let charged = original.captured_bytes();
        pending.bind_original(&original).unwrap();
        assert!(pending.bind_started);
        assert!(pending.original.as_ref().unwrap().same_instance(&original));
        assert!(!pending.original.as_ref().unwrap().same_instance(&foreign));
        assert_eq!(pending.bind_original(&foreign), Err(ArchiveError::Identity));
        assert_eq!(
            pending.bind_original(&original),
            Err(ArchiveError::Identity)
        );
        assert!(matches!(
            pending.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        assert!(pending.child.is_none() && pending.stdin.is_none());
        assert!(pending.stdout_pipe.is_none() && pending.stderr_pipe.is_none());
        assert!(pending.readers.iter().all(Option::is_none));
        assert_eq!(before_layout.owner_inline, Layout::for_value(&pending));
        assert_eq!(original.captured_bytes(), charged);
        assert!(
            pending
                .capture
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        let mut failures = pending.reader_failures.lock().unwrap();
        assert!(!failures.pristine_pending());
        assert!(
            failures
                .begin(super::super::reader_failure::ReaderStream::Stdout, &foreign)
                .is_err()
        );
        assert!(
            failures
                .begin(
                    super::super::reader_failure::ReaderStream::Stderr,
                    &original
                )
                .is_ok()
        );
    }

    #[test]
    fn failed_binding_keeps_the_owner_closed_after_alias_or_sticky_refusal() {
        let mut pending = pending_with_ring();
        let original = operation();
        let alias = pending.stdout_chunk.clone();
        let body_pointer = Arc::as_ptr(&alias);
        assert_eq!(
            pending.bind_original(&original),
            Err(ArchiveError::Identity)
        );
        assert!(pending.bind_started && pending.original.is_none());
        drop(alias);
        assert_eq!(
            pending.bind_original(&original),
            Err(ArchiveError::Identity)
        );
        assert!(pending.original.is_none());
        assert_eq!(Arc::as_ptr(&pending.stdout_chunk), body_pointer);
        assert!(pending.child.is_none() && pending.readers.iter().all(Option::is_none));
        assert!(matches!(
            pending.creation_layout(),
            Err(ArchiveError::Identity)
        ));

        let mut refused = pending_with_ring();
        let stopped = operation();
        let charged = stopped.captured_bytes();
        stopped.overflow();
        assert!(matches!(
            refused.bind_original(&stopped),
            Err(ArchiveError::Operation(
                crate::operation::OperationError::CaptureOverflow
            ))
        ));
        assert!(refused.bind_started && refused.original.is_none());
        assert_eq!(
            refused.bind_original(&original),
            Err(ArchiveError::Identity)
        );
        assert_eq!(stopped.captured_bytes(), charged);
        assert!(refused.child.is_none() && refused.readers.iter().all(Option::is_none));
    }

    #[test]
    fn actual_empty_bodies_include_chunk_arrays_and_fixed_reader_destinations_once() {
        let pending = pending_with_ring();
        let layout = pending.creation_layout().unwrap();
        assert_eq!(layout.owner_inline, Layout::for_value(&pending));
        assert_eq!(
            layout.bodies,
            [
                Layout::for_value(pending.stdout_assembly.as_ref()),
                Layout::for_value(pending.stdout_pending.as_ref()),
                Layout::for_value(pending.stdout_chunk.as_ref()),
                Layout::for_value(pending.stderr_chunk.as_ref()),
                Layout::for_value(pending.stderr_stats.as_ref()),
            ]
        );
        assert_eq!(
            layout.referenced_headers_bytes,
            layout.bodies.iter().map(Layout::size).sum::<usize>()
        );
        assert_eq!(
            layout.arc_counter_bytes,
            5 * 2 * std::mem::size_of::<usize>()
        );
        assert!(layout.bodies[2].size() >= crate::operation::STDOUT_CHUNK_BYTES);
        assert!(layout.bodies[3].size() >= crate::operation::STDOUT_CHUNK_BYTES);
        assert!(pending.original.is_none() && pending.child.is_none());
        assert!(
            pending.stdin.is_none()
                && pending.stdout_pipe.is_none()
                && pending.stderr_pipe.is_none()
        );
        assert_eq!(pending.readers.len(), 2);
        assert!(pending.readers.iter().all(Option::is_none));
        assert_eq!(
            layout.reader_failures_body,
            Layout::for_value(pending.reader_failures.as_ref())
        );
        assert_eq!(
            layout.reader_failures_counter_bytes,
            2 * std::mem::size_of::<usize>()
        );
        assert!(pending.reader_failures.lock().unwrap().pristine_pending());
        assert_eq!(
            layout.capture_body,
            Layout::for_value(pending.capture.as_ref())
        );
        assert_eq!(
            layout.capture_counter_bytes,
            2 * std::mem::size_of::<usize>()
        );
        assert!(pending.capture.lock().unwrap().is_none());
    }

    #[test] // UNCOMPILED/UNRUN: no task/read/State creation or original Fit.
    fn capture_alias_foreign_and_poison_refuse_without_replacing_or_binding_failures() {
        let original = operation();
        let foreign = operation();
        let mut aliased = pending_with_ring();
        let alias = aliased.capture.clone();
        let pointer = Arc::as_ptr(&alias);
        assert_eq!(
            aliased.bind_original(&original),
            Err(ArchiveError::Identity)
        );
        drop(alias);
        assert!(aliased.bind_started && aliased.original.is_none());
        assert_eq!(
            aliased.bind_original(&original),
            Err(ArchiveError::Identity)
        );
        assert_eq!(Arc::as_ptr(&aliased.capture), pointer);
        assert!(aliased.capture.lock().unwrap().is_none());
        assert!(aliased.reader_failures.lock().unwrap().pristine_pending());

        let mut occupied = pending_with_ring();
        *occupied.capture.lock().unwrap() = Some(foreign.clone());
        assert_eq!(
            occupied.bind_original(&original),
            Err(ArchiveError::Identity)
        );
        assert!(
            occupied
                .capture
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .same_instance(&foreign)
        );
        assert!(occupied.reader_failures.lock().unwrap().pristine_pending());
        assert!(occupied.original.is_none());

        let mut poisoned = pending_with_ring();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = poisoned.capture.lock().unwrap();
            panic!("synthetic pending capture poison");
        }));
        assert!(result.is_err());
        assert_eq!(
            poisoned.bind_original(&original),
            Err(ArchiveError::Identity)
        );
        assert!(poisoned.capture.is_poisoned());
        assert!(poisoned.reader_failures.lock().unwrap().pristine_pending());
        assert!(poisoned.bind_started && poisoned.original.is_none());
        assert!(poisoned.child.is_none() && poisoned.readers.iter().all(Option::is_none));
    }

    #[test]
    fn alias_busy_checkout_capacity_and_used_bytes_refuse_without_repair() {
        let pending = pending_with_ring();
        let alias = pending.stdout_chunk.clone();
        assert!(matches!(
            pending.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        drop(alias);
        let weak = Arc::downgrade(&pending.stderr_chunk);
        assert!(matches!(
            pending.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        drop(weak);
        let held = pending.stderr_stats.lock().unwrap();
        assert!(matches!(
            pending.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        drop(held);
        // Separate independent fixtures; never a repair/retry of a saved result.
        let checked_out = pending_with_ring();
        let body = checked_out.stdout_chunk.lock().unwrap().take().unwrap();
        assert!(matches!(
            checked_out.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        assert!(checked_out.stdout_chunk.lock().unwrap().is_none());
        assert_eq!(body.count, 0);
        let backed = pending_with_ring();
        backed.stdout_assembly.lock().unwrap().0.reserve_exact(1);
        assert!(matches!(
            backed.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        assert!(backed.stdout_assembly.lock().unwrap().0.capacity() > 0);
        let used = pending_with_ring();
        used.stderr_chunk.lock().unwrap().as_mut().unwrap().bytes[0] = 9;
        assert!(matches!(
            used.creation_layout(),
            Err(ArchiveError::Identity)
        ));
        assert_eq!(
            used.stderr_chunk.lock().unwrap().as_ref().unwrap().bytes[0],
            9
        );
    }
}
