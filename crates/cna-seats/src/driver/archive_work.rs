//! A movable ORIGINAL driver box and synchronous archive callback result.
//! The private std producer launches only from an installed canonical root and
//! retains the actual handle before registration. Callback return is not a join.

use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
};

use tokio::{
    sync::Notify,
    task::{Id, JoinError, JoinHandle},
};

use crate::{
    failed_evidence::{ArchiveError, ArchiveFileObservation, JoinedArchiveSource},
    operation::Operation,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Interruption {
    Timeout,
    OperatorStop,
}

/// Separate original cleanup refusal; neither variant changes the retained
/// publication Result. Move typed errors without formatting or body copies.
pub(super) enum CleanupError {
    Archive(ArchiveError),
    Driver(super::DriverError),
}

/// Owned dispatch refusal, distinct from physical cleanup and archive result.
/// Returning this does not settle the original operation or its canonical Box.
pub(super) enum DispatchError {
    Archive(ArchiveError),
    Driver(super::DriverError),
}

/// Canonical post-custody capture storage. A restoration wake may unwind after
/// the driver's MOVE, so both the body and first return stay with the SAME Box
/// until a separate, non-notifying handoff. No second driver call on retry.
struct CustodyCapture {
    started: bool,
    body: Option<super::TurnOutcome>,
    returned: Option<Result<bool, ArchiveError>>,
    handed_off: bool,
}

impl CustodyCapture {
    const fn empty() -> Self {
        Self {
            started: false,
            body: None,
            returned: None,
            handed_off: false,
        }
    }

    fn begin(&mut self) -> Result<(), ArchiveError> {
        if self.started || self.body.is_some() || self.returned.is_some() || self.handed_off {
            return Err(ArchiveError::Identity);
        }
        self.started = true; // BEFORE invocation; unwind never renews the attempt.
        Ok(())
    }

    fn take_return(&mut self) -> Result<Option<super::TurnOutcome>, CleanupError> {
        if !self.started || self.handed_off {
            return Err(CleanupError::Archive(ArchiveError::Identity));
        }
        match self.returned.as_ref() {
            Some(Ok(had_body)) if *had_body == self.body.is_some() => (),
            Some(Err(_)) if self.body.is_none() => (),
            // Unknown return (including unwind after a body MOVE) preserves
            // every saved byte. It cannot be promoted to successful capture.
            _ => return Err(CleanupError::Archive(ArchiveError::Identity)),
        }
        self.handed_off = true;
        // Last fallible checks are ABOVE the MOVE. No callbacks, allocation,
        // notify, formatting, replacement or new allowance follows it.
        match self
            .returned
            .take()
            .expect("checked canonical capture return")
        {
            Ok(_) => Ok(self.body.take()),
            Err(cause) => Err(CleanupError::Archive(cause)),
        }
    }
}

/// Exactly one box/publication result, with separate post-custody capture
/// disposition. No evidence collection, mirror, output channel, archive
/// destination or automatic discard/release is added.
pub(super) struct ArchiveWork<D> {
    original: Operation,
    driver: Box<D>,
    attempted: bool,
    first_interruption: Option<Interruption>,
    result: Option<Result<ArchiveFileObservation, ArchiveError>>,
    panic: Option<Box<dyn Any + Send>>,
    worker_refusal: Option<ArchiveError>,
    // Set ONLY after actual SAME-source custody acceptance. This does not
    // settle a prompt/actor/journal, release ownership or authorize rebinding.
    custody_accepted: bool,
    custody_capture: CustodyCapture,
}

struct RetainedState<D> {
    work: Option<ArchiveWork<D>>,
    checkout_started: bool,
    in_service: bool,
    first_interruption: Option<Interruption>,
    registered_worker: Option<Id>,
    observed_worker: Option<Id>,
}

struct RetainedShared<D> {
    state: Mutex<RetainedState<D>>,
    returned: Notify,
    std_registration: super::archive_thread::ThreadRegistration,
}

/// The outer root retains this slot BEFORE a future worker takes its sole box.
/// None while checked out means ORIGINAL ownership in service, never empty or
/// replaceable. Preparation launches nothing; only the private producer may
/// launch from this installed root. Ordinary drop is not discard permission.
/// The supervisor retains this root and actual handle, without waiting at exit.
pub(super) struct RetainedArchive<D> {
    original: Operation,
    shared: Arc<RetainedShared<D>>,
    worker: Option<JoinHandle<()>>,
    worker_join: Option<Result<(), JoinError>>,
    std_worker: Option<super::archive_thread::StdArchiveWorker>,
    std_registration_error: Option<ArchiveError>,
    std_start_failure: Option<super::archive_thread::RefusedStdStart>,
    std_pending_slot: Option<super::archive_thread::ThreadSlot>,
    std_launch_started: bool,
    std_launch_error: Option<ArchiveError>,
}

/// Private supervisor storage for an ORIGINAL root, including opaque Send-only
/// panic payloads. Mutex makes the owner shareable without claiming its contents
/// are Sync. It is NOT a task, queue, new worker or ownership-release channel.
/// Native construction/launcher installation still needs the complete assembly.
pub(super) struct SupervisorRoot<D> {
    original: Operation,
    slot: Mutex<Option<RetainedArchive<D>>>,
}

/// Exclusive MOVE of the SAME root. `get_mut` needs no lock/guard across await,
/// I/O or a caller callback. None in the saved slot means owned in service.
/// Borrow lifetime prevents replacement until synchronous restoration on Drop.
pub(super) struct SupervisorLease<'a, D> {
    slot: &'a mut Option<RetainedArchive<D>>,
    root: Option<RetainedArchive<D>>,
}

/// One preparation attempt retains whichever ORIGINAL owner the refused step
/// returned. These are ownership stages, never independent copies or retries.
enum PreparationStage<D> {
    Driver(Box<D>),
    Work(ArchiveWork<D>),
    Root(RetainedArchive<D>),
    Ready(SupervisorRoot<D>),
}

struct PreparedOwner<D> {
    stage: PreparationStage<D>,
    first_refusal: Option<ArchiveError>,
}

/// Module-private projection for physical supervision only. It borrows from
/// the whole preparation restore lease; no refused owner is extracted/promoted.
enum PreparationCleanup<'a, D> {
    Driver(&'a mut D),
    Root(&'a mut RetainedArchive<D>),
    Ready(&'a mut SupervisorRoot<D>),
}

/// Fixed metadata projection only; a busy slot is unknown, not empty/healthy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreparationPosition {
    Driver,
    Work,
    Root,
    Ready,
}

/// Private factory preparation storage. Mutex permits opaque Send-only panic
/// contents without exposing them through a Sync reference. No native entry,
/// public SeatDriver adapter, new allowance or automatic retry is supplied.
pub(super) struct PreparationOwner<D> {
    original: Operation,
    slot: Mutex<Option<PreparedOwner<D>>>,
}

/// Private storage for the future facade, installed BEFORE host binding/root
/// preparation. A panic or refusal retains both the SAME concrete Box and its
/// original context (either here or in that Box). No SeatDriver adapter or
/// public factory/main adoption is supplied by this source-only constructor.
pub(super) struct OriginalClaudeOwner {
    slot: Mutex<OriginalClaudeState>,
    // Actual configuration measurement lands in this SAME factory owner before
    // any original Operation exists. It is NOT a full creation-fit receipt.
    configuration_layout: Result<super::claude::ConfigCreationLayout, ArchiveError>,
    transcript_layout: Result<crate::transcript::TranscriptCreationLayout, ArchiveError>,
    pending_transcript: Option<crate::transcript::PendingTranscriptInputs>,
    creation_context: Option<super::claude_caller::PendingContextInputs>,
    creation_context_layout:
        Option<Result<super::claude_caller::ContextCreationLayout, ArchiveError>>,
    owner_layout_started: bool,
    owner_layout: Option<crate::host::BoundHostOperationLayout>,
    owner_layout_result: Option<Result<usize, ArchiveError>>,
    // Outside the mutex so observation-error retention cannot itself fail on
    // poison. Still canonical SAME-root storage, never fallback authority.
    start_observation_refusal: Option<ArchiveError>,
    turn_observation_refusal: Option<ArchiveError>,
    cleanup_observation_refusal: Option<ArchiveError>,
}

/// Private erased observation only. Bodies remain in the original owner;
/// bool records a returned disposition, never original joins or settlement.
pub(super) type OriginalControllerFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, ArchiveError>> + Send + 'a>>;

/// Actual concrete observation-frame layouts, measured before an Operation
/// exists. These describe storage only; referenced/prompt/library/allocator
/// payloads and the external campaign frame are NOT admitted by this value.
#[allow(dead_code)]
pub(super) struct ControllerCreationLayouts {
    pub(super) start: std::alloc::Layout,
    pub(super) turn: std::alloc::Layout,
    pub(super) cleanup: std::alloc::Layout,
    pub(super) overlap_bytes: usize,
}

#[derive(Clone, Copy)]
enum ControllerAction<'a> {
    Start(Option<&'a str>),
    Turn(&'a str, std::time::Duration),
    Cleanup,
}

/// Borrow the caller's already creation-admitted slot, never a new result body.
/// The exclusive borrow prevents replacement while the bridge is in service.
enum ControllerDestination<'a> {
    Start(&'a mut Option<Result<super::SessionInfo, DispatchError>>),
    Turn(&'a mut Option<Result<super::TurnOutcome, DispatchError>>),
    Cleanup(&'a mut Option<Result<(), CleanupError>>),
    DriverStart(&'a mut Option<Result<super::SessionInfo, super::DriverError>>),
    DriverTurn(&'a mut Option<Result<super::TurnOutcome, super::DriverError>>),
    DriverCleanup(&'a mut Option<Result<(), super::DriverError>>),
}

impl ControllerDestination<'_> {
    fn matches_empty(&self, action: ControllerAction<'_>) -> bool {
        match (action, self) {
            (ControllerAction::Start(_), Self::Start(slot)) => slot.is_none(),
            (ControllerAction::Turn(_, _), Self::Turn(slot)) => slot.is_none(),
            (ControllerAction::Cleanup, Self::Cleanup(slot)) => slot.is_none(),
            (ControllerAction::Start(_), Self::DriverStart(slot)) => slot.is_none(),
            (ControllerAction::Turn(_, _), Self::DriverTurn(slot)) => slot.is_none(),
            (ControllerAction::Cleanup, Self::DriverCleanup(slot)) => slot.is_none(),
            _ => false,
        }
    }

    fn reborrow(&mut self) -> ControllerDestination<'_> {
        match self {
            Self::Start(slot) => ControllerDestination::Start(slot),
            Self::Turn(slot) => ControllerDestination::Turn(slot),
            Self::Cleanup(slot) => ControllerDestination::Cleanup(slot),
            Self::DriverStart(slot) => ControllerDestination::DriverStart(slot),
            Self::DriverTurn(slot) => ControllerDestination::DriverTurn(slot),
            Self::DriverCleanup(slot) => ControllerDestination::DriverCleanup(slot),
        }
    }
}

/// Check the actual concrete unpolled family before the erasing allocation.
/// The prototype is then dropped without effects; final construction uses the
/// SAME function and arguments. Count inline/heap/return headers conservatively;
/// this is not allocator/library/input-growing payload or full fit evidence.
fn check_original_controller_erasure<F>(fixed: usize, future: &F) -> Result<(), ArchiveError>
where
    F: std::future::Future<Output = Result<bool, ArchiveError>> + Send,
{
    fixed
        .checked_add(std::mem::size_of_val(future))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of_val(future)))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<OriginalControllerFuture<'_>>()))
        .and_then(|bytes| {
            bytes.checked_add(std::mem::size_of::<
                Result<OriginalControllerFuture<'_>, ArchiveError>,
            >())
        })
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Result<(), ArchiveError>>()))
        .and_then(|bytes| bytes.checked_add(4 * std::mem::size_of::<usize>()))
        .ok_or(ArchiveError::Fit)
        .and_then(|bytes| {
            crate::failed_evidence::check_retained_publication_workspace::<
                super::claude::ClaudeDriver,
            >(bytes)
        })
}

/// The actual approved observation family has its OWN output/erased-return
/// headers. Check its inline+heap overlap before the sole Box::pin; counting
/// only the inner ArchiveError-output future would miss this outer frame.
/// This fixed-shape check is not FULL growing/library/allocator admission.
fn check_driver_observation_erasure<F>(fixed: usize, future: &F) -> Result<(), ArchiveError>
where
    F: std::future::Future<Output = Result<bool, super::DriverError>> + Send,
{
    fixed
        .checked_add(std::mem::size_of_val(future))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of_val(future)))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<super::RetainedDriverFuture<'_>>()))
        .and_then(|bytes| {
            bytes.checked_add(std::mem::size_of::<
                Result<super::RetainedDriverFuture<'_>, super::DriverError>,
            >())
        })
        .and_then(|bytes| {
            bytes.checked_add(std::mem::size_of::<Result<bool, super::DriverError>>())
        })
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Result<(), ArchiveError>>()))
        .and_then(|bytes| bytes.checked_add(4 * std::mem::size_of::<usize>()))
        .ok_or(ArchiveError::Fit)
        .and_then(|bytes| {
            crate::failed_evidence::check_retained_publication_workspace::<
                super::claude::ClaudeDriver,
            >(bytes)
        })
}

struct OriginalClaudeState {
    original: Option<Operation>,
    // Private concrete factory installs its value HERE before boxing/checks.
    // No replacement-ready interval or extra concrete driver is manufactured.
    unboxed: Option<super::claude::ClaudeDriver>,
    boxing_started: bool,
    boxing_slot: Option<Box<std::mem::MaybeUninit<super::claude::ClaudeDriver>>>,
    driver: Option<Box<super::claude::ClaudeDriver>>,
    pending_context: Option<crate::host::HostDriverContext>,
    prepared: Option<PreparationOwner<super::claude::ClaudeDriver>>,
    first_refusal: Option<DispatchError>,
    start_started: bool,
    // None is unfinished. Some(true/false) records ONLY the original returned
    // Result disposition, surviving its body MOVE; never joined/custody/healthy.
    start_returned_ok: Option<bool>,
    start_result: Option<Result<super::SessionInfo, DispatchError>>,
    turn_started: bool,
    turn_result: Option<Result<super::TurnOutcome, DispatchError>>,
    cleanup_started: bool,
    // Return disposition survives body handoff. It records neither original
    // child/reader joins nor archive verification, custody or settlement.
    cleanup_returned_ok: Option<bool>,
    cleanup_result: Option<Result<(), CleanupError>>,
}

/// Disjoint borrows of the ORIGINAL state, never a second returned-body owner.
/// The checked variant is fixed before invoking the concrete driver.
enum DispatchReturnSlot<'a> {
    Start {
        result: &'a mut Option<Result<super::SessionInfo, DispatchError>>,
        returned_ok: &'a mut Option<bool>,
    },
    Turn(&'a mut Option<Result<super::TurnOutcome, DispatchError>>),
}

fn retain_dispatch_return<T, E>(
    slot: &mut Option<Result<T, E>>,
    returned_ok: Option<&mut Option<bool>>,
    outcome: Result<T, E>,
) -> bool {
    *slot = Some(outcome); // FIRST MOVE, before restore/drop/wake or observation.
    let ok = slot.as_ref().is_some_and(Result::is_ok);
    if let Some(disposition) = returned_ok {
        *disposition = Some(ok);
    }
    ok
}

impl DispatchReturnSlot<'_> {
    fn matches_empty(&self, action: ControllerAction<'_>) -> bool {
        match (self, action) {
            (
                Self::Start {
                    result,
                    returned_ok,
                },
                ControllerAction::Start(_),
            ) => result.is_none() && returned_ok.is_none(),
            (Self::Turn(result), ControllerAction::Turn(_, _)) => result.is_none(),
            _ => false,
        }
    }

    fn retain_refusal(&mut self, cause: ArchiveError) -> bool {
        match self {
            Self::Start {
                result,
                returned_ok,
            } => retain_dispatch_return(
                result,
                Some(returned_ok),
                Err(DispatchError::Archive(cause)),
            ),
            Self::Turn(result) => {
                retain_dispatch_return(result, None, Err(DispatchError::Archive(cause)))
            }
        }
    }
}

struct CleanupReturnSlot<'a> {
    result: &'a mut Option<Result<(), CleanupError>>,
    returned_ok: &'a mut Option<bool>,
}

impl CleanupReturnSlot<'_> {
    fn empty(&self) -> bool {
        self.result.is_none() && self.returned_ok.is_none()
    }

    fn reborrow(&mut self) -> CleanupReturnSlot<'_> {
        CleanupReturnSlot {
            result: self.result,
            returned_ok: self.returned_ok,
        }
    }

    fn retain(&mut self, outcome: Result<(), CleanupError>) -> bool {
        retain_dispatch_return(self.result, Some(self.returned_ok), outcome)
    }

    fn refuse(&mut self, cause: ArchiveError) -> bool {
        self.retain(Err(CleanupError::Archive(cause)))
    }
}

fn check_cleanup_frame<F>(fixed: usize, future: &F) -> Result<(), ArchiveError> {
    let required = fixed
        .checked_add(std::mem::size_of_val(future))
        .ok_or(ArchiveError::Fit)?;
    crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
        required,
    )
}

/// Every caller retains its original physical-cleanup lease while this concrete
/// future writes directly into the ORIGINAL cleanup Result, before restoration.
async fn physical_cleanup_into(
    driver: &mut super::claude::ClaudeDriver,
    expected: &Operation,
    fixed: usize,
    mut destination: CleanupReturnSlot<'_>,
) -> Result<bool, ArchiveError> {
    if !destination.empty() {
        return Err(ArchiveError::Identity);
    }
    if let Err(cause) = driver.check_physical_cleanup_original(expected) {
        return Ok(destination.refuse(cause));
    }
    let cleanup = driver.supervise_retained_children();
    if let Err(cause) = check_cleanup_frame(fixed, &cleanup) {
        return Ok(destination.refuse(cause));
    }
    let outcome = cleanup.await;
    Ok(destination.retain(outcome.map_err(|source| {
        CleanupError::Driver(super::DriverError::Spawn {
            cli: "claude cleanup",
            source,
        })
    }))) // FIRST typed MOVE before any caller's restoration guard drops.
}

impl OriginalClaudeOwner {
    /// Plain MOVE into fixed inline storage; no fallible checks or allocation
    /// follow the Box transfer. Original creation later admits these headers.
    pub(super) fn new(driver: Box<super::claude::ClaudeDriver>) -> Self {
        Self::from_slots(Some(driver), None)
    }

    /// Private concrete factory: plain MOVE into original inline root BEFORE
    /// the heap Box, host binding or preparation. Public factory stays held.
    pub(super) fn from_concrete(driver: super::claude::ClaudeDriver) -> Self {
        Self::from_slots(None, Some(driver))
    }

    fn from_slots(
        driver: Option<Box<super::claude::ClaudeDriver>>,
        unboxed: Option<super::claude::ClaudeDriver>,
    ) -> Self {
        let configuration_layout = driver
            .as_deref()
            .or(unboxed.as_ref())
            .ok_or(ArchiveError::Identity)
            .and_then(super::claude::ClaudeDriver::original_configuration_layout);
        let transcript_layout = driver
            .as_deref()
            .or(unboxed.as_ref())
            .ok_or(ArchiveError::Identity)
            .and_then(super::claude::ClaudeDriver::original_transcript_layout);
        Self {
            slot: Mutex::new(OriginalClaudeState {
                original: None,
                unboxed,
                boxing_started: false,
                boxing_slot: None,
                driver,
                pending_context: None,
                prepared: None,
                first_refusal: None,
                start_started: false,
                start_returned_ok: None,
                start_result: None,
                turn_started: false,
                turn_result: None,
                cleanup_started: false,
                cleanup_returned_ok: None,
                cleanup_result: None,
            }),
            configuration_layout,
            transcript_layout,
            pending_transcript: None,
            creation_context: None,
            creation_context_layout: None,
            owner_layout_started: false,
            owner_layout: None,
            owner_layout_result: None,
            start_observation_refusal: None,
            turn_observation_refusal: None,
            cleanup_observation_refusal: None,
        }
    }

    pub(super) fn configuration_layout(
        &self,
    ) -> &Result<super::claude::ConfigCreationLayout, ArchiveError> {
        &self.configuration_layout
    }

    pub(super) fn transcript_layout(
        &self,
    ) -> &Result<crate::transcript::TranscriptCreationLayout, ArchiveError> {
        &self.transcript_layout
    }

    /// Associate the retained worker inputs with their SAME concrete producer
    /// already moved into this unboxed driver. No task/bind/Box is authorized.
    pub(super) fn attach_pending_transcript(
        &mut self,
        candidate: &mut Option<crate::transcript::PendingTranscriptInputs>,
    ) -> Result<(), ArchiveError> {
        if self.pending_transcript.is_some()
            || self.configuration_layout.is_err()
            || self.transcript_layout.is_err()
            || self.creation_context.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        let input = candidate.as_ref().ok_or(ArchiveError::Identity)?;
        let state = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        let driver = state.unboxed.as_ref().ok_or(ArchiveError::Identity)?;
        if state.original.is_some()
            || state.driver.is_some()
            || state.boxing_started
            || state.boxing_slot.is_some()
            || state.pending_context.is_some()
            || state.prepared.is_some()
            || state.first_refusal.is_some()
            || state.start_started
            || state.turn_started
            || state.cleanup_started
            || !driver.original_factory_unbound()
            || !driver.pending_transcript_matches(input)
        {
            return Err(ArchiveError::Identity);
        }
        self.pending_transcript = candidate.take(); // Canonical MOVE after exact checks.
        Ok(())
    }

    pub(super) fn pending_transcript_layout(
        &self,
    ) -> Option<&Result<crate::transcript::PendingTranscriptCreationLayout, ArchiveError>> {
        self.pending_transcript
            .as_ref()
            .and_then(|pending| pending.creation_layout())
    }

    #[cfg(test)]
    pub(super) fn pending_transcript_for_source_control(
        &self,
    ) -> Option<&crate::transcript::PendingTranscriptInputs> {
        self.pending_transcript.as_ref()
    }

    #[cfg(test)]
    pub(super) fn pending_matches_driver_for_source_control(&mut self) -> bool {
        let Some(input) = self.pending_transcript.as_ref() else {
            return false;
        };
        let Ok(state) = self.slot.get_mut() else {
            return false;
        };
        state
            .unboxed
            .as_ref()
            .is_some_and(|driver| driver.pending_transcript_matches(input))
    }

    /// Move trusted metadata into the SAME concrete factory before measuring.
    /// No Operation, binding, driver Box, future or native effect is created.
    /// Repeated/foreign/unprepared input remains with its caller. An accepted
    /// input and first layout Result stay owned even if measurement refuses.
    pub(super) fn stage_creation_context(
        &mut self,
        candidate: &mut Option<super::claude_caller::PendingContextInputs>,
    ) -> Result<(), ArchiveError> {
        self.stage_creation_context_boundary(candidate, |input| input.creation_layout())
    }

    fn stage_creation_context_boundary(
        &mut self,
        candidate: &mut Option<super::claude_caller::PendingContextInputs>,
        measure: impl FnOnce(
            &super::claude_caller::PendingContextInputs,
        )
            -> Result<super::claude_caller::ContextCreationLayout, ArchiveError>,
    ) -> Result<(), ArchiveError> {
        if self.creation_context.is_some()
            || self.creation_context_layout.is_some()
            || self.configuration_layout.is_err()
            || self.transcript_layout.is_err()
        {
            return Err(ArchiveError::Identity);
        }
        let input = candidate.as_ref().ok_or(ArchiveError::Identity)?;
        let state = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        let driver = state.unboxed.as_ref().ok_or(ArchiveError::Identity)?;
        if state.original.is_some()
            || state.driver.is_some()
            || state.pending_context.is_some()
            || state.prepared.is_some()
            || state.first_refusal.is_some()
            || state.boxing_started
            || state.boxing_slot.is_some()
            || state.start_started
            || state.turn_started
            || state.cleanup_started
            || !driver.original_factory_unbound()
            || driver.original_factory_seat() != input.identity.seat
            || input.identity.operation_id.len() != 36
            || uuid::Uuid::parse_str(&input.identity.operation_id).is_err()
        {
            return Err(ArchiveError::Identity);
        }
        self.creation_context = candidate.take(); // Canonical MOVE before measurement.
        let result = measure(
            self.creation_context
                .as_ref()
                .expect("checked creation context"),
        );
        self.creation_context_layout = Some(result); // First actual Result before fixed observation.
        if matches!(self.creation_context_layout, Some(Ok(_))) {
            Ok(())
        } else {
            Err(ArchiveError::Identity)
        }
    }

    pub(super) fn creation_context(&self) -> Option<&super::claude_caller::PendingContextInputs> {
        self.creation_context.as_ref()
    }

    pub(super) fn creation_context_layout(
        &self,
    ) -> Option<&Result<super::claude_caller::ContextCreationLayout, ArchiveError>> {
        self.creation_context_layout.as_ref()
    }

    /// Stage ONLY a SAME-instance owner descriptor, before original creation.
    /// Missing/foreign input remains with its caller and changes NO root state.
    pub(super) fn stage_owner_layout(
        &mut self,
        candidate: &mut Option<crate::host::BoundHostOperationLayout>,
    ) -> Result<(), ArchiveError> {
        if self.owner_layout_started
            || self.owner_layout.is_some()
            || self.owner_layout_result.is_some()
            || !matches!(self.creation_context_layout, Some(Ok(_)))
        {
            return Err(ArchiveError::Identity);
        }
        let context = self
            .creation_context
            .as_ref()
            .ok_or(ArchiveError::Identity)?;
        let input = candidate.as_ref().ok_or(ArchiveError::Identity)?;
        if !input.matches_owner(&context.admission) {
            return Err(ArchiveError::Identity);
        }
        let state = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        let driver = state.unboxed.as_ref().ok_or(ArchiveError::Identity)?;
        if state.original.is_some()
            || state.driver.is_some()
            || state.boxing_started
            || state.boxing_slot.is_some()
            || state.prepared.is_some()
            || state.pending_context.is_some()
            || state.first_refusal.is_some()
            || state.start_started
            || state.turn_started
            || state.cleanup_started
            || !driver.original_factory_unbound()
        {
            return Err(ArchiveError::Identity);
        }
        self.owner_layout_started = true;
        self.owner_layout = candidate.take(); // SAME owner descriptor MOVE first.
        let result = self
            .owner_layout
            .as_ref()
            .expect("checked owner layout")
            .creation_bytes_for(&context.admission);
        self.owner_layout_result = Some(result); // First actual Result before observation.
        if matches!(self.owner_layout_result, Some(Ok(_))) {
            Ok(())
        } else {
            Err(ArchiveError::Identity)
        }
    }

    pub(super) fn owner_layout_bytes(&self) -> Option<&Result<usize, ArchiveError>> {
        self.owner_layout_result.as_ref()
    }

    /// Measure the SAME concrete async function used by the later erasure.
    /// None originals are private, UNPOLLED layout witnesses, never a minted
    /// Operation or an executable fallback. Their frame TYPE includes all
    /// states/inner awaited frames, independent of these argument values.
    /// Production erasure still checks a prototype with Some(SAME originals)
    /// and the actual action/destination before its sole Box::pin.
    #[allow(dead_code)]
    pub(super) fn creation_frame_layouts(
        &mut self,
    ) -> Result<ControllerCreationLayouts, ArchiveError> {
        if self.configuration_layout.is_err()
            || self.transcript_layout.is_err()
            || self.creation_context.is_none()
            || !matches!(self.creation_context_layout, Some(Ok(_)))
            || self.start_observation_refusal.is_some()
            || self.turn_observation_refusal.is_some()
            || self.cleanup_observation_refusal.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        let state = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        let driver = state.unboxed.as_ref().ok_or(ArchiveError::Identity)?;
        if state.original.is_some()
            || state.driver.is_some()
            || state.pending_context.is_some()
            || state.prepared.is_some()
            || state.first_refusal.is_some()
            || state.boxing_started
            || state.boxing_slot.is_some()
            || state.start_started
            || state.start_returned_ok.is_some()
            || state.start_result.is_some()
            || state.turn_started
            || state.turn_result.is_some()
            || state.cleanup_started
            || state.cleanup_returned_ok.is_some()
            || state.cleanup_result.is_some()
            || !driver.original_factory_unbound()
        {
            return Err(ArchiveError::Identity);
        }
        let mut start_slot = None;
        let start = {
            let future = self.observe_driver_started_to_slot(
                None,
                ControllerAction::Start(None),
                None,
                ControllerDestination::DriverStart(&mut start_slot),
            );
            std::alloc::Layout::for_value(&future).pad_to_align()
        }; // Drop UNPOLLED, before taking another exclusive root borrow.
        let mut turn_slot = None;
        let turn = {
            let future = self.observe_driver_started_to_slot(
                None,
                ControllerAction::Turn("", std::time::Duration::ZERO),
                None,
                ControllerDestination::DriverTurn(&mut turn_slot),
            );
            std::alloc::Layout::for_value(&future).pad_to_align()
        };
        let mut cleanup_slot = None;
        let cleanup = {
            let future = self.observe_driver_started_to_slot(
                None,
                ControllerAction::Cleanup,
                None,
                ControllerDestination::DriverCleanup(&mut cleanup_slot),
            );
            std::alloc::Layout::for_value(&future).pad_to_align()
        };
        // Conservatively count ALL THREE inline+heap pairs, not an assumed
        // lifetime maximum. Allocator overhead is still a separate obligation.
        let overlap_bytes = [start, turn, cleanup]
            .into_iter()
            .try_fold(0usize, |bytes, layout| {
                layout
                    .size()
                    .checked_mul(2)
                    .and_then(|pair| bytes.checked_add(pair))
            })
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<(
                    Option<Result<super::SessionInfo, super::DriverError>>,
                    Option<Result<super::TurnOutcome, super::DriverError>>,
                    Option<Result<(), super::DriverError>>,
                    ControllerCreationLayouts,
                    Result<ControllerCreationLayouts, ArchiveError>,
                    Result<super::RetainedDriverFuture<'_>, super::DriverError>,
                    Result<bool, super::DriverError>,
                )>())
            })
            .ok_or(ArchiveError::Fit)?;
        Ok(ControllerCreationLayouts {
            start,
            turn,
            cleanup,
            overlap_bytes,
        })
    }

    #[cfg(test)]
    pub(super) fn stage_creation_context_for_source_control(
        &mut self,
        candidate: &mut Option<super::claude_caller::PendingContextInputs>,
        measure: impl FnOnce(
            &super::claude_caller::PendingContextInputs,
        )
            -> Result<super::claude_caller::ContextCreationLayout, ArchiveError>,
    ) -> Result<(), ArchiveError> {
        self.stage_creation_context_boundary(candidate, measure)
    }

    /// Private original-only context derivation. The caller supplies ONLY its
    /// retained actual creation result. This is not binding/worker activation:
    /// the pending worker inputs remain HERE and the old binder stays closed.
    pub(super) fn derive_created_context(
        &mut self,
        original: &Operation,
    ) -> Result<(), ArchiveError> {
        if !matches!(self.creation_context_layout, Some(Ok(_)))
            || !matches!(self.owner_layout_result, Some(Ok(_)))
            || self.configuration_layout.is_err()
            || self.transcript_layout.is_err()
        {
            return Err(ArchiveError::Identity);
        }
        let input = self
            .creation_context
            .as_ref()
            .ok_or(ArchiveError::Identity)?;
        let pending = self
            .pending_transcript
            .as_ref()
            .ok_or(ArchiveError::Identity)?;
        let descriptor = self.owner_layout.as_ref().ok_or(ArchiveError::Identity)?;
        let state = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        let driver = state.unboxed.as_ref().ok_or(ArchiveError::Identity)?;
        if state.original.is_some()
            || state.pending_context.is_some()
            || state.driver.is_some()
            || state.prepared.is_some()
            || state.first_refusal.is_some()
            || state.boxing_started
            || state.boxing_slot.is_some()
            || state.start_started
            || state.turn_started
            || state.cleanup_started
            || !driver.original_factory_unbound()
            || !driver.pending_transcript_matches(pending)
            || !descriptor.matches_owner(&input.admission)
            || input.identity.operation_id != original.id()
        {
            return Err(ArchiveError::Identity);
        }
        let input = self
            .creation_context
            .take()
            .expect("checked staged context");
        // Arc/inline handle copies only, no new State or growing body. Move the
        // complete context into its canonical slot BEFORE any post-MOVE check.
        state.pending_context = Some(crate::host::HostDriverContext {
            admission: input.admission,
            identity: input.identity,
            operation: original.clone(),
        });
        state.original = Some(original.clone());
        let context = state
            .pending_context
            .as_ref()
            .expect("retained created context");
        if !context.operation.same_instance(original) {
            return Err(ArchiveError::Identity);
        }
        original.check()?;
        Ok(()) // Context custody ONLY, never bound/prepared/worker/healthy proof.
    }

    pub(super) fn bind_original_context(
        &mut self,
        context: crate::host::HostDriverContext,
    ) -> Result<(), ArchiveError> {
        self.bind_original_context_boundary(context, || {}, || {})
    }

    /// Borrowed caller ingress: every refusal BEFORE acceptance preserves the
    /// entire input slot. Unique &mut excludes mutation/poison between the two
    /// private checks and the immediate original-context retention below.
    pub(super) fn bind_original_context_from_slot(
        &mut self,
        candidate: &mut Option<crate::host::HostDriverContext>,
    ) -> Result<(), ArchiveError> {
        if candidate.is_none()
            || self.creation_context.is_some()
            || self.pending_transcript.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        let state = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        if state.original.is_some()
            || state.pending_context.is_some()
            || state.prepared.is_some()
            || state.first_refusal.is_some()
            || state.boxing_started
            || state.boxing_slot.is_some()
            || (state.driver.is_none() == state.unboxed.is_none())
        {
            return Err(ArchiveError::Identity);
        }
        let context = candidate.take().expect("checked original caller context");
        self.bind_original_context_boundary(context, || {}, || {})
    }

    #[cfg(test)]
    pub(super) fn bind_original_context_for_source_control(
        &mut self,
        context: crate::host::HostDriverContext,
        before_take: impl FnOnce(),
    ) -> Result<(), ArchiveError> {
        self.bind_original_context_boundary(context, || {}, before_take)
    }

    #[cfg(test)]
    pub(super) fn bind_original_box_for_source_control(
        &mut self,
        context: crate::host::HostDriverContext,
        before_box_move: impl FnOnce(),
    ) -> Result<(), ArchiveError> {
        self.bind_original_context_boundary(context, before_box_move, || {})
    }

    fn bind_original_context_boundary(
        &mut self,
        context: crate::host::HostDriverContext,
        before_box_move: impl FnOnce(),
        before_take: impl FnOnce(),
    ) -> Result<(), ArchiveError> {
        // Staged precreation metadata cannot be bypassed by a legacy binding
        // path. The eventual admitted creation transition must consume it first.
        if self.creation_context.is_some() || self.pending_transcript.is_some() {
            return Err(ArchiveError::Identity);
        }
        let state = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        // ANY earlier attempt, even an unwind without a typed error, is sticky.
        // No retry, operation replacement, context clone, renewal or rebinding.
        if state.original.is_some()
            || state.pending_context.is_some()
            || state.prepared.is_some()
            || state.first_refusal.is_some()
            || state.boxing_started
            || state.boxing_slot.is_some()
            || (state.driver.is_none() == state.unboxed.is_none())
        {
            return Err(ArchiveError::Identity);
        }
        // Retain the entire context BEFORE any original/host/frame check. Its
        // only body transfer is the checked assignment into the SAME driver.
        state.pending_context = Some(context);
        state.original = Some(
            state
                .pending_context
                .as_ref()
                .ok_or(ArchiveError::Identity)?
                .operation
                .clone(),
        );
        let expected = state.original.as_ref().ok_or(ArchiveError::Identity)?;
        let required =
            RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes().and_then(|bytes| {
                bytes
                    .checked_add(std::mem::size_of_val(&before_box_move))
                    .and_then(|bytes| bytes.checked_add(std::mem::size_of_val(&before_take)))
                    .ok_or(ArchiveError::Fit)
            });
        if let Err(error) = required.and_then(|bytes| {
            crate::failed_evidence::check_retained_publication_workspace::<
                super::claude::ClaudeDriver,
            >(bytes)
        }) {
            state.first_refusal = Some(DispatchError::Archive(error));
            return Err(ArchiveError::Identity); // Original typed cause stays HERE.
        }
        if state.unboxed.is_some() {
            // Check ORIGINAL creation-reserved fixed workspace and allowance
            // before allocating. Growing config/outer/library fit is still a
            // separate FULL-plan obligation; this is not a late charge.
            if let Err(error) = expected.check() {
                state.first_refusal = Some(DispatchError::Driver(super::DriverError::Host(
                    crate::host::operation_refusal(error),
                )));
                return Err(ArchiveError::Identity);
            }
            state.boxing_started = true; // BEFORE allocation; unwind cannot retry.
            state.boxing_slot = Some(Box::new_uninit()); // Retain empty allocation FIRST.
            before_box_move(); // SOURCE control boundary; production no-op.
            // Both slots are private and sealed by the checks above. Safe
            // Box::write fills the retained allocation; no callback/fallible
            // check follows either body MOVE before storing the SAME Box.
            let driver = state
                .unboxed
                .take()
                .expect("original concrete factory driver");
            let slot = state
                .boxing_slot
                .take()
                .expect("original concrete factory box");
            state.driver = Some(Box::write(slot, driver));
        }
        let driver = state.driver.as_mut().ok_or(ArchiveError::Identity)?;
        if let Err(error) = driver.bind_host_from_slot(&mut state.pending_context, expected) {
            state.first_refusal = Some(DispatchError::Driver(error));
            return Err(ArchiveError::Identity);
        }
        match PreparationOwner::prepare_from_slot_boundary(&mut state.driver, expected, before_take)
        {
            Err(error) => {
                state.first_refusal = Some(DispatchError::Archive(error));
                Err(ArchiveError::Identity)
            }
            Ok(prepared) => {
                // The returned complete root is installed BEFORE any caller
                // callback/await; this assignment cannot drop an earlier owner.
                state.prepared = Some(prepared);
                Ok(())
            }
        }
    }

    /// Exclusive trusted assembly borrow only. A pending/refused/unwound bind
    /// never projects as ready; original interruption/overflow gates remain in
    /// PreparationOwner, so this borrow itself grants no dispatch or settlement.
    pub(super) fn prepared_original(
        &mut self,
        expected: &Operation,
    ) -> Result<&mut PreparationOwner<super::claude::ClaudeDriver>, ArchiveError> {
        let state = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        if !state
            .original
            .as_ref()
            .is_some_and(|original| original.same_instance(expected))
            || state.first_refusal.is_some()
            || state.pending_context.is_some()
            || state.driver.is_some()
            || state.unboxed.is_some()
            || state.boxing_slot.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        state.prepared.as_mut().ok_or(ArchiveError::Identity)
    }

    pub(super) fn first_refusal(
        &mut self,
        expected: &Operation,
    ) -> Result<Option<&DispatchError>, ArchiveError> {
        let state = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        if !state
            .original
            .as_ref()
            .is_some_and(|original| original.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        Ok(state.first_refusal.as_ref())
    }

    fn state_original(
        &mut self,
        expected: &Operation,
    ) -> Result<&mut OriginalClaudeState, ArchiveError> {
        let state = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        if !state
            .original
            .as_ref()
            .is_some_and(|original| original.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        Ok(state)
    }

    fn begin_start(&mut self, expected: &Operation) -> Result<(), ArchiveError> {
        let state = self.state_original(expected)?;
        if state.start_started
            || state.start_returned_ok.is_some()
            || state.start_result.is_some()
            || state.turn_started
            || state.turn_result.is_some()
            || state.cleanup_started
            || state.first_refusal.is_some()
            || state.pending_context.is_some()
            || state.driver.is_some()
            || state.prepared.is_none()
        {
            return Err(ArchiveError::Identity);
        }
        // Install BEFORE checks/awaits that can unwind or suspend. Cancellation
        // leaves this marker and the canonical owner; None result is unknown.
        state.start_started = true;
        Ok(())
    }

    #[cfg(test)]
    fn retain_start(
        &mut self,
        expected: &Operation,
        outcome: Result<super::SessionInfo, DispatchError>,
    ) -> Result<bool, ArchiveError> {
        let state = self.state_original(expected)?;
        if !state.start_started || state.start_returned_ok.is_some() || state.start_result.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        Ok(Self::install_start(state, outcome))
    }

    fn install_start(
        state: &mut OriginalClaudeState,
        outcome: Result<super::SessionInfo, DispatchError>,
    ) -> bool {
        // Caller has already checked the empty slot before dispatch. This is
        // the first operation after await: no fallible lookup or callback.
        retain_dispatch_return(
            &mut state.start_result,
            Some(&mut state.start_returned_ok),
            outcome,
        )
    }

    fn begin_turn(&mut self, expected: &Operation) -> Result<(), ArchiveError> {
        let state = self.state_original(expected)?;
        if !state.start_started
            || state.start_returned_ok != Some(true)
            || state.turn_started
            || state.turn_result.is_some()
            || state.cleanup_started
            || state.first_refusal.is_some()
            || state.pending_context.is_some()
            || state.driver.is_some()
            || state.prepared.is_none()
        {
            return Err(ArchiveError::Identity);
        }
        state.turn_started = true;
        Ok(())
    }

    #[cfg(test)]
    fn retain_turn(
        &mut self,
        expected: &Operation,
        outcome: Result<super::TurnOutcome, DispatchError>,
    ) -> Result<bool, ArchiveError> {
        let state = self.state_original(expected)?;
        if !state.turn_started || state.turn_result.is_some() {
            return Err(ArchiveError::Identity);
        }
        Ok(Self::install_turn(state, outcome))
    }

    fn install_turn(
        state: &mut OriginalClaudeState,
        outcome: Result<super::TurnOutcome, DispatchError>,
    ) -> bool {
        retain_dispatch_return(&mut state.turn_result, None, outcome)
    }

    /// Synchronous pre-erasure check; concrete futures are never polled here.
    /// All returned body slots/markers are included by fixed_bytes at original
    /// creation. Full payload/library/allocator/transient fit is still separate.
    pub(super) fn check_controller_frames(
        &mut self,
        expected: &Operation,
    ) -> Result<(), ArchiveError> {
        self.check_controller_frames_with_caller_storage(expected, 0)
    }

    fn check_controller_frames_with_caller_storage(
        &mut self,
        expected: &Operation,
        caller_bytes: usize,
    ) -> Result<(), ArchiveError> {
        self.state_original(expected)?;
        let fixed = RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes()?;
        let start = std::mem::size_of_val(&self.observe_start(expected, None));
        let turn =
            std::mem::size_of_val(&self.observe_turn(expected, "", std::time::Duration::ZERO));
        let cleanup = std::mem::size_of_val(&self.observe_cleanup(expected));
        let required = fixed
            .checked_add(caller_bytes)
            .and_then(|bytes| bytes.checked_add(start))
            .and_then(|bytes| bytes.checked_add(turn))
            .and_then(|bytes| bytes.checked_add(cleanup))
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    std::future::Ready<Result<bool, ArchiveError>>,
                >())
            })
            .and_then(|bytes| {
                bytes.checked_add(
                    3 * std::mem::size_of::<
                        std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
                    >(),
                )
            })
            .ok_or(ArchiveError::Fit)?;
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            required,
        )
    }

    /// Private unboxed controller path. bool projects only Ok/Err of the stored
    /// return, never joined/custody/healthy. Started-without-result cannot retry.
    pub(super) async fn observe_start(
        &mut self,
        expected: &Operation,
        resume: Option<&str>,
    ) -> Result<bool, ArchiveError> {
        self.begin_start(expected)?;
        self.observe_started_start(expected, resume).await
    }

    async fn observe_started_start(
        &mut self,
        expected: &Operation,
        resume: Option<&str>,
    ) -> Result<bool, ArchiveError> {
        let frames = self.check_controller_frames(expected);
        let state = self.state_original(expected)?;
        if let Err(error) = frames {
            return Ok(Self::install_start(
                state,
                Err(DispatchError::Archive(error)),
            ));
        }
        let prepared = state.prepared.as_mut().ok_or(ArchiveError::Identity)?;
        prepared
            .dispatch_original_into(
                expected,
                ControllerAction::Start(resume),
                DispatchReturnSlot::Start {
                    result: &mut state.start_result,
                    returned_ok: &mut state.start_returned_ok,
                },
            )
            .await
    }

    pub(super) async fn observe_turn(
        &mut self,
        expected: &Operation,
        prompt: &str,
        limit: std::time::Duration,
    ) -> Result<bool, ArchiveError> {
        self.begin_turn(expected)?;
        self.observe_started_turn(expected, prompt, limit).await
    }

    async fn observe_started_turn(
        &mut self,
        expected: &Operation,
        prompt: &str,
        limit: std::time::Duration,
    ) -> Result<bool, ArchiveError> {
        let frames = self.check_controller_frames(expected);
        let state = self.state_original(expected)?;
        if let Err(error) = frames {
            return Ok(Self::install_turn(
                state,
                Err(DispatchError::Archive(error)),
            ));
        }
        let prepared = state.prepared.as_mut().ok_or(ArchiveError::Identity)?;
        prepared
            .dispatch_original_into(
                expected,
                ControllerAction::Turn(prompt, limit),
                DispatchReturnSlot::Turn(&mut state.turn_result),
            )
            .await
    }

    fn begin_cleanup(&mut self, expected: &Operation) -> Result<(), ArchiveError> {
        let state = self.state_original(expected)?;
        if state.cleanup_started
            || state.cleanup_returned_ok.is_some()
            || state.cleanup_result.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        state.cleanup_started = true;
        Ok(())
    }

    fn install_cleanup(state: &mut OriginalClaudeState, outcome: Result<(), CleanupError>) -> bool {
        // Called first after await using the already checked exclusive state.
        // No formatting/callback/fallible lookup separates return and retention.
        retain_dispatch_return(
            &mut state.cleanup_result,
            Some(&mut state.cleanup_returned_ok),
            outcome,
        )
    }

    /// Physical cleanup retains its own result, including a refused/unwound
    /// preparation's direct Box. It neither replaces the dispatch cause nor
    /// promotes a retained start/turn result. No automatic cleanup retry.
    pub(super) async fn observe_cleanup(
        &mut self,
        expected: &Operation,
    ) -> Result<bool, ArchiveError> {
        self.begin_cleanup(expected)?;
        self.observe_started_cleanup(expected).await
    }

    async fn observe_started_cleanup(
        &mut self,
        expected: &Operation,
    ) -> Result<bool, ArchiveError> {
        let frames = self.check_controller_frames(expected);
        let state = self.state_original(expected)?;
        if let Err(error) = frames {
            return Ok(Self::install_cleanup(
                state,
                Err(CleanupError::Archive(error)),
            ));
        }
        let fixed = match RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes() {
            Ok(fixed) => fixed,
            Err(error) => {
                return Ok(Self::install_cleanup(
                    state,
                    Err(CleanupError::Archive(error)),
                ));
            }
        };
        let mut destination = CleanupReturnSlot {
            result: &mut state.cleanup_result,
            returned_ok: &mut state.cleanup_returned_ok,
        };
        match (state.prepared.as_mut(), state.driver.as_mut()) {
            (Some(prepared), None) => {
                prepared
                    .supervise_before_archive_into(expected, destination)
                    .await
            }
            (None, Some(driver)) => {
                physical_cleanup_into(driver, expected, fixed, destination).await
            }
            _ => Ok(destination.refuse(ArchiveError::Identity)),
        }
    }

    /// Synchronous private facade entry. Mark before any checks/allocation;
    /// pre-box refusal retains the original typed cause, returns only a fixed
    /// projection and never permits retry. Dropping an unpolled accepted future
    /// keeps the attempt incomplete. Public trait/factory adoption is separate.
    pub(super) fn start_future<'a>(
        &'a mut self,
        expected: &'a Operation,
        resume: Option<&'a str>,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.start_future_boundary(expected, resume, 0)
    }

    fn start_future_boundary<'a>(
        &'a mut self,
        expected: &'a Operation,
        resume: Option<&'a str>,
        extra: usize,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.begin_start(expected)?;
        let fixed = match self
            .check_controller_frames(expected)
            .and_then(|()| RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes())
        {
            Ok(fixed) => fixed,
            Err(error) => {
                Self::install_start(
                    self.state_original(expected)?,
                    Err(DispatchError::Archive(error)),
                );
                return Err(ArchiveError::Identity); // Typed cause remains canonical.
            }
        };
        let checked = {
            let future = self.observe_started_start(expected, resume);
            fixed
                .checked_add(extra)
                .ok_or(ArchiveError::Fit)
                .and_then(|bytes| check_original_controller_erasure(bytes, &future))
        }; // Unpolled prototype borrow ends BEFORE retaining a refusal.
        if let Err(error) = checked {
            Self::install_start(
                self.state_original(expected)?,
                Err(DispatchError::Archive(error)),
            );
            return Err(ArchiveError::Identity);
        }
        Ok(Box::pin(self.observe_started_start(expected, resume)))
    }

    pub(super) fn turn_future<'a>(
        &'a mut self,
        expected: &'a Operation,
        prompt: &'a str,
        limit: std::time::Duration,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.turn_future_boundary(expected, prompt, limit, 0)
    }

    fn turn_future_boundary<'a>(
        &'a mut self,
        expected: &'a Operation,
        prompt: &'a str,
        limit: std::time::Duration,
        extra: usize,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.begin_turn(expected)?;
        let fixed = match self
            .check_controller_frames(expected)
            .and_then(|()| RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes())
        {
            Ok(fixed) => fixed,
            Err(error) => {
                Self::install_turn(
                    self.state_original(expected)?,
                    Err(DispatchError::Archive(error)),
                );
                return Err(ArchiveError::Identity);
            }
        };
        let checked = {
            let future = self.observe_started_turn(expected, prompt, limit);
            fixed
                .checked_add(extra)
                .ok_or(ArchiveError::Fit)
                .and_then(|bytes| check_original_controller_erasure(bytes, &future))
        };
        if let Err(error) = checked {
            Self::install_turn(
                self.state_original(expected)?,
                Err(DispatchError::Archive(error)),
            );
            return Err(ArchiveError::Identity);
        }
        Ok(Box::pin(self.observe_started_turn(expected, prompt, limit)))
    }

    pub(super) fn cleanup_future<'a>(
        &'a mut self,
        expected: &'a Operation,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.cleanup_future_boundary(expected, 0)
    }

    fn cleanup_future_boundary<'a>(
        &'a mut self,
        expected: &'a Operation,
        extra: usize,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.begin_cleanup(expected)?;
        let fixed = match self
            .check_controller_frames(expected)
            .and_then(|()| RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes())
        {
            Ok(fixed) => fixed,
            Err(error) => {
                Self::install_cleanup(
                    self.state_original(expected)?,
                    Err(CleanupError::Archive(error)),
                );
                return Err(ArchiveError::Identity);
            }
        };
        let checked = {
            let future = self.observe_started_cleanup(expected);
            fixed
                .checked_add(extra)
                .ok_or(ArchiveError::Fit)
                .and_then(|bytes| check_original_controller_erasure(bytes, &future))
        };
        if let Err(error) = checked {
            Self::install_cleanup(
                self.state_original(expected)?,
                Err(CleanupError::Archive(error)),
            );
            return Err(ArchiveError::Identity);
        }
        Ok(Box::pin(self.observe_started_cleanup(expected)))
    }

    /// Private caller-slot bridge: check BEFORE erasure, retain the original
    /// controller return, then MOVE directly into its SAME-original empty slot.
    /// No result body lives in the erased future's returned bool. This is not
    /// public trait/factory adoption or complete caller/payload fit evidence.
    pub(super) fn start_to_slot_future<'a>(
        &'a mut self,
        expected: &'a Operation,
        resume: Option<&'a str>,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<super::SessionInfo, DispatchError>>,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.start_to_slot_with_caller_storage(
            expected,
            resume,
            destination_original,
            destination,
            0,
        )
    }

    /// Add the actual simultaneously retained caller headers to the SAME
    /// pre-erasure workspace check. Separate successful checks do not prove
    /// that their combined live storage fits. Growing/full fit remains held.
    pub(super) fn start_to_slot_with_caller_storage<'a>(
        &'a mut self,
        expected: &'a Operation,
        resume: Option<&'a str>,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<super::SessionInfo, DispatchError>>,
        caller_bytes: usize,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.to_slot_future_boundary(
            expected,
            ControllerAction::Start(resume),
            destination_original,
            ControllerDestination::Start(destination),
            caller_bytes,
        )
    }

    pub(super) fn turn_to_slot_future<'a>(
        &'a mut self,
        expected: &'a Operation,
        prompt: &'a str,
        limit: std::time::Duration,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<super::TurnOutcome, DispatchError>>,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.turn_to_slot_with_caller_storage(
            expected,
            prompt,
            limit,
            destination_original,
            destination,
            0,
        )
    }

    pub(super) fn turn_to_slot_with_caller_storage<'a>(
        &'a mut self,
        expected: &'a Operation,
        prompt: &'a str,
        limit: std::time::Duration,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<super::TurnOutcome, DispatchError>>,
        caller_bytes: usize,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.to_slot_future_boundary(
            expected,
            ControllerAction::Turn(prompt, limit),
            destination_original,
            ControllerDestination::Turn(destination),
            caller_bytes,
        )
    }

    pub(super) fn cleanup_to_slot_future<'a>(
        &'a mut self,
        expected: &'a Operation,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<(), CleanupError>>,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.cleanup_to_slot_with_caller_storage(expected, destination_original, destination, 0)
    }

    pub(super) fn cleanup_to_slot_with_caller_storage<'a>(
        &'a mut self,
        expected: &'a Operation,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<(), CleanupError>>,
        caller_bytes: usize,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.to_slot_future_boundary(
            expected,
            ControllerAction::Cleanup,
            destination_original,
            ControllerDestination::Cleanup(destination),
            caller_bytes,
        )
    }

    /// Private typed bridge for the approved trait destinations. The public
    /// facade/complete original fit is not adopted by this SOURCE entry.
    pub(super) fn start_to_driver_slot_future<'a>(
        &'a mut self,
        expected: &'a Operation,
        resume: Option<&'a str>,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<super::SessionInfo, super::DriverError>>,
        caller_bytes: usize,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.to_slot_future_boundary(
            expected,
            ControllerAction::Start(resume),
            destination_original,
            ControllerDestination::DriverStart(destination),
            caller_bytes,
        )
    }

    pub(super) fn turn_to_driver_slot_future<'a>(
        &'a mut self,
        expected: &'a Operation,
        prompt: &'a str,
        limit: std::time::Duration,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<super::TurnOutcome, super::DriverError>>,
        caller_bytes: usize,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.to_slot_future_boundary(
            expected,
            ControllerAction::Turn(prompt, limit),
            destination_original,
            ControllerDestination::DriverTurn(destination),
            caller_bytes,
        )
    }

    pub(super) fn cleanup_to_driver_slot_future<'a>(
        &'a mut self,
        expected: &'a Operation,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<(), super::DriverError>>,
        caller_bytes: usize,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.to_slot_future_boundary(
            expected,
            ControllerAction::Cleanup,
            destination_original,
            ControllerDestination::DriverCleanup(destination),
            caller_bytes,
        )
    }

    /// Private adapter for the already-approved trait observation type. Public
    /// factory/override activation still requires FULL original fit/lifecycle.
    pub(super) fn start_retained_observation<'a>(
        &'a mut self,
        expected: &'a Operation,
        resume: Option<&'a str>,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<super::SessionInfo, super::DriverError>>,
        caller_bytes: usize,
    ) -> Result<super::RetainedDriverFuture<'a>, super::DriverError> {
        self.driver_observation_boundary(
            expected,
            ControllerAction::Start(resume),
            destination_original,
            ControllerDestination::DriverStart(destination),
            caller_bytes,
        )
    }

    pub(super) fn turn_retained_observation<'a>(
        &'a mut self,
        expected: &'a Operation,
        prompt: &'a str,
        limit: std::time::Duration,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<super::TurnOutcome, super::DriverError>>,
        caller_bytes: usize,
    ) -> Result<super::RetainedDriverFuture<'a>, super::DriverError> {
        self.driver_observation_boundary(
            expected,
            ControllerAction::Turn(prompt, limit),
            destination_original,
            ControllerDestination::DriverTurn(destination),
            caller_bytes,
        )
    }

    pub(super) fn cleanup_retained_observation<'a>(
        &'a mut self,
        expected: &'a Operation,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<(), super::DriverError>>,
        caller_bytes: usize,
    ) -> Result<super::RetainedDriverFuture<'a>, super::DriverError> {
        self.driver_observation_boundary(
            expected,
            ControllerAction::Cleanup,
            destination_original,
            ControllerDestination::DriverCleanup(destination),
            caller_bytes,
        )
    }

    fn driver_observation_boundary<'a>(
        &'a mut self,
        expected: &'a Operation,
        action: ControllerAction<'a>,
        destination_original: &'a Operation,
        mut destination: ControllerDestination<'a>,
        caller_bytes: usize,
    ) -> Result<super::RetainedDriverFuture<'a>, super::DriverError> {
        // Foreign/occupied/wrong-kind refusal consumes no attempt or body.
        self.begin_slot_action(expected, action, destination_original, &destination)
            .map_err(|_| Self::fixed_driver_observation_refusal())?;
        let fixed = self
            .check_controller_frames_with_caller_storage(expected, caller_bytes)
            .and_then(|()| RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes())
            .and_then(|bytes| bytes.checked_add(caller_bytes).ok_or(ArchiveError::Fit));
        let checked = match fixed {
            Err(cause) => Err(cause),
            Ok(fixed) => {
                let future = self.observe_driver_started_to_slot(
                    Some(expected),
                    action,
                    Some(destination_original),
                    destination.reborrow(),
                );
                check_driver_observation_erasure(fixed, &future)
            }
        }; // Drop the SAME-argument UNPOLLED prototype before root mutation.
        if let Err(cause) = checked {
            self.retain_driver_fit_refusal(expected, action, cause);
            return Err(Self::fixed_driver_observation_refusal());
        }
        // ONE erasing allocation. The inner controller remains inline in this
        // checked outer frame; no boxed-inner/late-map/legacy future fallback.
        Ok(Box::pin(self.observe_driver_started_to_slot(
            Some(expected),
            action,
            Some(destination_original),
            destination,
        )))
    }

    fn fixed_driver_observation_refusal() -> super::DriverError {
        super::DriverError::Host(crate::host::HostRefusal::Unknown)
    }

    fn retain_driver_observation_refusal(
        &mut self,
        action: ControllerAction<'_>,
        cause: ArchiveError,
    ) {
        let slot = match action {
            ControllerAction::Start(_) => &mut self.start_observation_refusal,
            ControllerAction::Turn(_, _) => &mut self.turn_observation_refusal,
            ControllerAction::Cleanup => &mut self.cleanup_observation_refusal,
        };
        if slot.is_none() {
            *slot = Some(cause);
        } // First MOVE, no fallible lookup.
    }

    /// Save the actual sizing cause BEFORE any state lookup/projection. Move it
    /// into the usual action result only while that original slot is pristine.
    /// Poison/refused association leaves the SAME cause in this outer root.
    fn retain_driver_fit_refusal(
        &mut self,
        expected: &Operation,
        action: ControllerAction<'_>,
        cause: ArchiveError,
    ) {
        self.retain_driver_observation_refusal(action, cause);
        let Ok(state) = self.slot.get_mut() else {
            return;
        };
        if !state
            .original
            .as_ref()
            .is_some_and(|original| original.same_instance(expected))
        {
            return;
        }
        let saved = match action {
            ControllerAction::Start(_)
                if state.start_result.is_none() && state.start_returned_ok.is_none() =>
            {
                &mut self.start_observation_refusal
            }
            ControllerAction::Turn(_, _) if state.turn_result.is_none() => {
                &mut self.turn_observation_refusal
            }
            ControllerAction::Cleanup
                if state.cleanup_result.is_none() && state.cleanup_returned_ok.is_none() =>
            {
                &mut self.cleanup_observation_refusal
            }
            _ => return,
        };
        let Some(cause) = saved.take() else {
            return;
        };
        match action {
            ControllerAction::Start(_) => {
                Self::install_start(state, Err(DispatchError::Archive(cause)));
            }
            ControllerAction::Turn(_, _) => {
                Self::install_turn(state, Err(DispatchError::Archive(cause)));
            }
            ControllerAction::Cleanup => {
                Self::install_cleanup(state, Err(CleanupError::Archive(cause)));
            }
        }
    }

    async fn observe_driver_started_to_slot(
        &mut self,
        expected: Option<&Operation>,
        action: ControllerAction<'_>,
        destination_original: Option<&Operation>,
        destination: ControllerDestination<'_>,
    ) -> Result<bool, super::DriverError> {
        let (Some(expected), Some(destination_original)) = (expected, destination_original) else {
            // Creation witnesses are never polled/exposed. A missing original
            // still refuses closed if a future caller violates that boundary.
            return self.project_driver_observation(action, Err(ArchiveError::Identity));
        };
        let observed = self
            .observe_started_to_slot(expected, action, destination_original, destination)
            .await;
        self.project_driver_observation(action, observed)
    }

    fn project_driver_observation(
        &mut self,
        action: ControllerAction<'_>,
        observed: Result<bool, ArchiveError>,
    ) -> Result<bool, super::DriverError> {
        match observed {
            Ok(moved) => {
                let refused = match action {
                    ControllerAction::Start(_) => self.start_observation_refusal.is_some(),
                    ControllerAction::Turn(_, _) => self.turn_observation_refusal.is_some(),
                    ControllerAction::Cleanup => self.cleanup_observation_refusal.is_some(),
                };
                if refused {
                    Err(Self::fixed_driver_observation_refusal())
                } else {
                    Ok(moved)
                }
            }
            Err(cause) => {
                // Inner callback Result is canonical before handoff. Retain any
                // observation refusal too, with no await/callback/format/lookup
                // between its return and this first root MOVE.
                self.retain_driver_observation_refusal(action, cause);
                Err(Self::fixed_driver_observation_refusal())
            }
        }
    }

    #[cfg(test)]
    pub(super) fn project_driver_observation_for_source_control(
        &mut self,
        expected: &Operation,
        mode: usize,
        observed: Result<bool, ArchiveError>,
    ) -> Result<bool, super::DriverError> {
        self.state_original(expected)
            .map_err(|_| Self::fixed_driver_observation_refusal())?;
        let action = match mode {
            0 => ControllerAction::Start(None),
            1 => ControllerAction::Turn("", std::time::Duration::ZERO),
            2 => ControllerAction::Cleanup,
            _ => return Err(Self::fixed_driver_observation_refusal()),
        };
        self.project_driver_observation(action, observed)
    }

    fn begin_slot_action(
        &mut self,
        expected: &Operation,
        action: ControllerAction<'_>,
        destination_original: &Operation,
        destination: &ControllerDestination<'_>,
    ) -> Result<(), ArchiveError> {
        if !expected.same_instance(destination_original) || !destination.matches_empty(action) {
            return Err(ArchiveError::Identity);
        }
        let refused = match action {
            ControllerAction::Start(_) => self.start_observation_refusal.is_some(),
            ControllerAction::Turn(_, _) => self.turn_observation_refusal.is_some(),
            ControllerAction::Cleanup => self.cleanup_observation_refusal.is_some(),
        };
        if refused {
            return Err(ArchiveError::Identity);
        }
        match action {
            ControllerAction::Start(_) => self.begin_start(expected),
            ControllerAction::Turn(_, _) => self.begin_turn(expected),
            ControllerAction::Cleanup => self.begin_cleanup(expected),
        }
    }

    fn to_slot_future_boundary<'a>(
        &'a mut self,
        expected: &'a Operation,
        action: ControllerAction<'a>,
        destination_original: &'a Operation,
        mut destination: ControllerDestination<'a>,
        caller_bytes: usize,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        // A foreign/full/wrong-kind destination cannot consume an attempt or
        // erase its earlier body. No callback, clone or allocation here.
        self.begin_slot_action(expected, action, destination_original, &destination)?;
        let fixed = self
            .check_controller_frames_with_caller_storage(expected, caller_bytes)
            .and_then(|()| RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes())
            .and_then(|bytes| bytes.checked_add(caller_bytes).ok_or(ArchiveError::Fit));
        let checked = match fixed {
            Err(error) => Err(error),
            Ok(fixed) => {
                let future = self.observe_started_to_slot(
                    expected,
                    action,
                    destination_original,
                    destination.reborrow(),
                );
                check_original_controller_erasure(fixed, &future)
            }
        }; // Drop the unpolled prototype and both borrows BEFORE refusal storage.
        if let Err(error) = checked {
            let state = self.state_original(expected)?;
            match action {
                ControllerAction::Start(_) => {
                    Self::install_start(state, Err(DispatchError::Archive(error)));
                }
                ControllerAction::Turn(_, _) => {
                    Self::install_turn(state, Err(DispatchError::Archive(error)));
                }
                ControllerAction::Cleanup => {
                    Self::install_cleanup(state, Err(CleanupError::Archive(error)));
                }
            }
            return Err(ArchiveError::Identity); // Typed cause stays canonical, slot untouched.
        }
        Ok(Box::pin(self.observe_started_to_slot(
            expected,
            action,
            destination_original,
            destination,
        )))
    }

    async fn observe_started_to_slot(
        &mut self,
        expected: &Operation,
        action: ControllerAction<'_>,
        destination_original: &Operation,
        destination: ControllerDestination<'_>,
    ) -> Result<bool, ArchiveError> {
        match (action, destination) {
            (ControllerAction::Start(resume), ControllerDestination::Start(slot)) => {
                self.observe_started_start(expected, resume).await?;
                self.handoff_start_result(expected, destination_original, slot)
            }
            (ControllerAction::Turn(prompt, limit), ControllerDestination::Turn(slot)) => {
                self.observe_started_turn(expected, prompt, limit).await?;
                self.handoff_turn_result(expected, destination_original, slot)
            }
            (ControllerAction::Cleanup, ControllerDestination::Cleanup(slot)) => {
                self.observe_started_cleanup(expected).await?;
                self.handoff_cleanup_result(expected, destination_original, slot)
            }
            (ControllerAction::Start(resume), ControllerDestination::DriverStart(slot)) => {
                self.observe_started_start(expected, resume).await?;
                self.handoff_start_driver_result(expected, destination_original, slot)
            }
            (ControllerAction::Turn(prompt, limit), ControllerDestination::DriverTurn(slot)) => {
                self.observe_started_turn(expected, prompt, limit).await?;
                self.handoff_turn_driver_result(expected, destination_original, slot)
            }
            (ControllerAction::Cleanup, ControllerDestination::DriverCleanup(slot)) => {
                self.observe_started_cleanup(expected).await?;
                self.handoff_cleanup_driver_result(expected, destination_original, slot)
            }
            _ => Err(ArchiveError::Identity),
        }
        // Handoff failure leaves the retained return in its original owner;
        // success means ONLY slot transfer, not dispatch success or settlement.
    }

    /// Read-only exclusive projections. None is unfinished, not success. These
    /// are controller returns, distinct from Claude's captured terminal body.
    pub(super) fn retained_results(
        &mut self,
        expected: &Operation,
    ) -> Result<ControllerResults<'_>, ArchiveError> {
        let state = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        if !state
            .original
            .as_ref()
            .is_some_and(|original| original.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        Ok(ControllerResults {
            start: state.start_result.as_ref(),
            turn: state.turn_result.as_ref(),
            cleanup: state.cleanup_result.as_ref(),
            start_started: state.start_started,
            start_returned_ok: state.start_returned_ok,
            turn_started: state.turn_started,
            cleanup_started: state.cleanup_started,
            cleanup_returned_ok: state.cleanup_returned_ok,
            start_observation_refusal: self.start_observation_refusal.as_ref(),
            turn_observation_refusal: self.turn_observation_refusal.as_ref(),
            cleanup_observation_refusal: self.cleanup_observation_refusal.as_ref(),
        })
    }

    /// MOVE the original start return to the caller's creation-admitted empty
    /// slot. All fallible checks precede the transfer. Keep the one-shot marker
    /// and return disposition: removing a failed result cannot permit a turn,
    /// and removing a successful body cannot require cloning it for dispatch.
    /// This disposition does not override any inner original lifetime gates.
    pub(super) fn handoff_start_result(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<Result<super::SessionInfo, DispatchError>>,
    ) -> Result<bool, ArchiveError> {
        if !expected.same_instance(destination_original) || destination.is_some() {
            return Err(ArchiveError::Identity);
        }
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes()?,
        )?;
        let state = self.state_original(expected)?;
        if !state.start_started {
            return Err(ArchiveError::Identity);
        }
        if state.start_result.is_none() {
            return Ok(false); // Interrupted or already handed off, never settled.
        }
        // Refuse inconsistent state BEFORE taking either owned body or error.
        if state.start_returned_ok != state.start_result.as_ref().map(|outcome| outcome.is_ok()) {
            return Err(ArchiveError::Identity);
        }
        *destination = state.start_result.take();
        Ok(true)
    }

    /// MOVE the controller return directly into an already creation-admitted
    /// SAME-original empty caller slot. Captured terminal evidence is a separate
    /// transfer. Checks precede take; no callback/await/format after the MOVE.
    /// Keeping turn_started forbids another dispatch even after body transfer.
    pub(super) fn handoff_turn_result(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<Result<super::TurnOutcome, DispatchError>>,
    ) -> Result<bool, ArchiveError> {
        if !expected.same_instance(destination_original) || destination.is_some() {
            return Err(ArchiveError::Identity);
        }
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes()?,
        )?;
        let state = self.state_original(expected)?;
        if !state.turn_started {
            return Err(ArchiveError::Identity);
        }
        if state.turn_result.is_none() {
            return Ok(false); // No result, not successful dispatch/cleanup.
        }
        *destination = state.turn_result.take();
        Ok(true)
    }

    /// MOVE a cleanup return into a creation-admitted SAME-original empty slot.
    /// The one-shot attempt and return disposition remain installed; removing
    /// either success or error body never permits another cleanup or dispatch.
    /// This is NOT a join, archive custody, prompt ACK or healthy-settlement gate.
    pub(super) fn handoff_cleanup_result(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<Result<(), CleanupError>>,
    ) -> Result<bool, ArchiveError> {
        if !expected.same_instance(destination_original) || destination.is_some() {
            return Err(ArchiveError::Identity);
        }
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes()?,
        )?;
        let state = self.state_original(expected)?;
        if !state.cleanup_started {
            return Err(ArchiveError::Identity);
        }
        if state.cleanup_result.is_none() {
            return Ok(false); // Unfinished or transferred; never a join witness.
        }
        if state.cleanup_returned_ok != state.cleanup_result.as_ref().map(|outcome| outcome.is_ok())
        {
            return Err(ArchiveError::Identity);
        }
        // All checks precede the body transfer; no callback/await/format follows.
        *destination = state.cleanup_result.take();
        Ok(true)
    }

    /// A DriverError body MOVEs directly from its first canonical return into
    /// the approved typed trait destination. An ArchiveError is kept HERE,
    /// never formatted, cloned or replaced by a synthetic DriverError body.
    pub(super) fn handoff_start_driver_result(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<Result<super::SessionInfo, super::DriverError>>,
    ) -> Result<bool, ArchiveError> {
        if !expected.same_instance(destination_original) || destination.is_some() {
            return Err(ArchiveError::Identity);
        }
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes()?,
        )?;
        let state = self.state_original(expected)?;
        if !state.start_started {
            return Err(ArchiveError::Identity);
        }
        if state.start_result.is_none() {
            return Ok(false);
        }
        if state.start_returned_ok != state.start_result.as_ref().map(|outcome| outcome.is_ok()) {
            return Err(ArchiveError::Identity);
        }
        match state
            .start_result
            .take()
            .expect("checked original startup return")
        {
            Ok(body) => *destination = Some(Ok(body)),
            Err(DispatchError::Driver(body)) => *destination = Some(Err(body)),
            Err(DispatchError::Archive(cause)) => {
                state.start_result = Some(Err(DispatchError::Archive(cause)));
                return Err(ArchiveError::Identity);
            }
        }
        Ok(true) // MOVE only; return disposition/one-shot markers remain.
    }

    pub(super) fn handoff_turn_driver_result(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<Result<super::TurnOutcome, super::DriverError>>,
    ) -> Result<bool, ArchiveError> {
        if !expected.same_instance(destination_original) || destination.is_some() {
            return Err(ArchiveError::Identity);
        }
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes()?,
        )?;
        let state = self.state_original(expected)?;
        if !state.turn_started {
            return Err(ArchiveError::Identity);
        }
        let Some(outcome) = state.turn_result.take() else {
            return Ok(false);
        };
        match outcome {
            Ok(body) => *destination = Some(Ok(body)),
            Err(DispatchError::Driver(body)) => *destination = Some(Err(body)),
            Err(DispatchError::Archive(cause)) => {
                state.turn_result = Some(Err(DispatchError::Archive(cause)));
                return Err(ArchiveError::Identity);
            }
        }
        Ok(true)
    }

    pub(super) fn handoff_cleanup_driver_result(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<Result<(), super::DriverError>>,
    ) -> Result<bool, ArchiveError> {
        if !expected.same_instance(destination_original) || destination.is_some() {
            return Err(ArchiveError::Identity);
        }
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes()?,
        )?;
        let state = self.state_original(expected)?;
        if !state.cleanup_started {
            return Err(ArchiveError::Identity);
        }
        if state.cleanup_result.is_none() {
            return Ok(false);
        }
        if state.cleanup_returned_ok != state.cleanup_result.as_ref().map(|outcome| outcome.is_ok())
        {
            return Err(ArchiveError::Identity);
        }
        match state
            .cleanup_result
            .take()
            .expect("checked original cleanup return")
        {
            Ok(()) => *destination = Some(Ok(())),
            Err(CleanupError::Driver(body)) => *destination = Some(Err(body)),
            Err(CleanupError::Archive(cause)) => {
                state.cleanup_result = Some(Err(CleanupError::Archive(cause)));
                return Err(ArchiveError::Identity);
            }
        }
        Ok(true)
    }

    /// Separately captured terminal evidence goes straight into the caller's
    /// creation-admitted ORIGINAL slot before archive. The saved start/turn/
    /// cleanup results are never replaced, and failure/overflow does not erase
    /// an earlier original captured body. Every inner owner/physical association
    /// and pending archive/custody check remains in the existing transfer path.
    pub(super) fn handoff_captured_before_archive(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<super::TurnOutcome>,
    ) -> Result<bool, ArchiveError> {
        if !expected.same_instance(destination_original) || destination.is_some() {
            return Err(ArchiveError::Identity);
        }
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes()?,
        )?;
        let state = self.state_original(expected)?;
        match (state.prepared.as_mut(), state.driver.as_mut()) {
            (Some(prepared), None) => prepared.handoff_captured_before_archive(
                expected,
                destination_original,
                destination,
            ),
            (None, Some(driver)) => {
                driver.handoff_original_captured(expected, destination_original, destination)
            }
            _ => Err(ArchiveError::Identity),
        }
    }

    #[cfg(test)]
    pub(super) fn start_future_for_source_control<'a>(
        &'a mut self,
        expected: &'a Operation,
        extra: usize,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.start_future_boundary(expected, None, extra)
    }

    #[cfg(test)]
    pub(super) fn turn_future_for_source_control<'a>(
        &'a mut self,
        expected: &'a Operation,
        extra: usize,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.turn_future_boundary(
            expected,
            "unpolled synthetic prompt",
            std::time::Duration::ZERO,
            extra,
        )
    }

    #[cfg(test)]
    pub(super) fn cleanup_future_for_source_control<'a>(
        &'a mut self,
        expected: &'a Operation,
        extra: usize,
    ) -> Result<OriginalControllerFuture<'a>, ArchiveError> {
        self.cleanup_future_boundary(expected, extra)
    }

    #[cfg(test)]
    pub(super) fn begin_start_for_source_control(
        &mut self,
        expected: &Operation,
    ) -> Result<(), ArchiveError> {
        self.begin_start(expected)
    }

    #[cfg(test)]
    pub(super) fn retain_start_for_source_control(
        &mut self,
        expected: &Operation,
        outcome: Result<super::SessionInfo, DispatchError>,
    ) -> Result<bool, ArchiveError> {
        self.retain_start(expected, outcome)
    }

    /// Synthetic return injected at the SAME first-MOVE helper, followed by
    /// actual foreground/supervisor/preparation restoration and Notify wake.
    /// No native dispatch, task, thread, timer or store callback is invoked.
    #[cfg(test)]
    pub(super) fn retained_start_wake_for_source_control(
        &mut self,
        expected: &Operation,
        outcome: Result<super::SessionInfo, DispatchError>,
        waker: &std::task::Waker,
    ) -> Result<bool, ArchiveError> {
        self.begin_start(expected)?;
        let state = self.state_original(expected)?;
        let mut preparation = state
            .prepared
            .as_mut()
            .ok_or(ArchiveError::Identity)?
            .borrow(expected)?;
        let mut supervision = preparation.supervisor()?.borrow(expected)?;
        let root = supervision.root_mut()?;
        let shared = Arc::clone(&root.shared); // Handle only, no evidence/body copy.
        let foreground = root.borrow_foreground(expected)?;
        let mut notification = std::pin::pin!(shared.returned.notified());
        let mut context = std::task::Context::from_waker(waker);
        assert!(std::future::Future::poll(notification.as_mut(), &mut context).is_pending());
        let returned = retain_dispatch_return(
            &mut state.start_result,
            Some(&mut state.start_returned_ok),
            outcome,
        );
        drop(foreground); // Waker may unwind; typed body is ALREADY in root.
        Ok(returned)
    }

    /// Task-free waiter owned by a source control. It registers only when the
    /// control manually polls it, after these temporary read leases restore.
    #[cfg(test)]
    pub(super) fn restoration_wait_for_source_control(
        &mut self,
        expected: &Operation,
    ) -> Result<impl std::future::Future<Output = ()> + Send + 'static + use<>, ArchiveError> {
        let mut preparation = self.prepared_original(expected)?.borrow(expected)?;
        let mut supervision = preparation.supervisor()?.borrow(expected)?;
        let shared = Arc::clone(&supervision.root_mut()?.shared);
        Ok(async move { shared.returned.notified().await })
    }

    #[cfg(test)]
    pub(super) fn begin_turn_for_source_control(
        &mut self,
        expected: &Operation,
    ) -> Result<(), ArchiveError> {
        self.begin_turn(expected)
    }

    #[cfg(test)]
    pub(super) fn retain_turn_for_source_control(
        &mut self,
        expected: &Operation,
        outcome: Result<super::TurnOutcome, DispatchError>,
    ) -> Result<bool, ArchiveError> {
        self.retain_turn(expected, outcome)
    }

    #[cfg(test)]
    pub(super) fn begin_cleanup_for_source_control(
        &mut self,
        expected: &Operation,
    ) -> Result<(), ArchiveError> {
        self.begin_cleanup(expected)
    }

    #[cfg(test)]
    pub(super) fn retain_cleanup_for_source_control(
        &mut self,
        expected: &Operation,
        outcome: Result<(), CleanupError>,
    ) -> Result<bool, ArchiveError> {
        let state = self.state_original(expected)?;
        if !state.cleanup_started
            || state.cleanup_returned_ok.is_some()
            || state.cleanup_result.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        Ok(Self::install_cleanup(state, outcome))
    }

    #[cfg(test)]
    pub(super) fn unprepared_for_source_control(
        &mut self,
        expected: &Operation,
    ) -> Result<
        (
            &super::claude::ClaudeDriver,
            Option<&crate::host::HostDriverContext>,
        ),
        ArchiveError,
    > {
        let state = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        if !state
            .original
            .as_ref()
            .is_some_and(|original| original.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        Ok((
            state
                .driver
                .as_deref()
                .or(state.unboxed.as_ref())
                .ok_or(ArchiveError::Identity)?,
            state.pending_context.as_ref(),
        ))
    }

    #[cfg(test)]
    pub(super) fn creation_untouched_for_source_control(&mut self) -> bool {
        let Ok(state) = self.slot.get_mut() else {
            return false;
        };
        state.original.is_none()
            && state.unboxed.is_some()
            && state.driver.is_none()
            && !state.boxing_started
            && state.boxing_slot.is_none()
            && state.pending_context.is_none()
            && state.prepared.is_none()
            && state.first_refusal.is_none()
            && !state.start_started
            && state.start_returned_ok.is_none()
            && state.start_result.is_none()
            && !state.turn_started
            && state.turn_result.is_none()
            && !state.cleanup_started
            && state.cleanup_returned_ok.is_none()
            && state.cleanup_result.is_none()
            && self.start_observation_refusal.is_none()
            && self.turn_observation_refusal.is_none()
            && self.cleanup_observation_refusal.is_none()
    }

    #[cfg(test)]
    pub(super) fn concrete_factory_for_source_control(
        &mut self,
    ) -> Result<(&mut super::claude::ClaudeDriver, bool, Option<usize>), ArchiveError> {
        let state = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        let started = state.boxing_started;
        let allocation = state
            .boxing_slot
            .as_ref()
            .map(|slot| slot.as_ptr() as usize);
        let driver = state
            .unboxed
            .as_mut()
            .or(state.driver.as_deref_mut())
            .ok_or(ArchiveError::Identity)?;
        Ok((driver, started, allocation))
    }
}

pub(super) struct ControllerResults<'a> {
    pub(super) start: Option<&'a Result<super::SessionInfo, DispatchError>>,
    pub(super) turn: Option<&'a Result<super::TurnOutcome, DispatchError>>,
    pub(super) cleanup: Option<&'a Result<(), CleanupError>>,
    pub(super) start_started: bool,
    pub(super) start_returned_ok: Option<bool>,
    pub(super) turn_started: bool,
    pub(super) cleanup_started: bool,
    pub(super) cleanup_returned_ok: Option<bool>,
    pub(super) start_observation_refusal: Option<&'a ArchiveError>,
    pub(super) turn_observation_refusal: Option<&'a ArchiveError>,
    pub(super) cleanup_observation_refusal: Option<&'a ArchiveError>,
}

/// MOVE/restore the whole preparation, including a typed refused step. No
/// mutex guard crosses a callback, await or I/O; None means owned in service.
pub(super) struct PreparationLease<'a, D> {
    slot: &'a mut Option<PreparedOwner<D>>,
    prepared: Option<PreparedOwner<D>>,
}

impl<D> Drop for PreparationLease<'_, D> {
    fn drop(&mut self) {
        assert!(self.slot.is_none(), "original preparation slot replaced");
        *self.slot = self.prepared.take();
    }
}

impl<D> PreparationLease<'_, D> {
    fn physical_cleanup(
        &mut self,
        expected: &Operation,
    ) -> Result<PreparationCleanup<'_, D>, ArchiveError> {
        let prepared = self.prepared.as_mut().ok_or(ArchiveError::Identity)?;
        match &mut prepared.stage {
            PreparationStage::Driver(driver) => Ok(PreparationCleanup::Driver(driver.as_mut())),
            PreparationStage::Work(work) => {
                if !work.original.same_instance(expected)
                    || work.attempted
                    || work.result.is_some()
                    || work.panic.is_some()
                    || work.worker_refusal.is_some()
                    || work.custody_accepted
                {
                    return Err(ArchiveError::Identity);
                }
                Ok(PreparationCleanup::Driver(work.driver.as_mut()))
            }
            PreparationStage::Root(root) => Ok(PreparationCleanup::Root(root)),
            PreparationStage::Ready(root) => Ok(PreparationCleanup::Ready(root)),
        }
    }

    pub(super) fn first_refusal(&self) -> Result<Option<&ArchiveError>, ArchiveError> {
        Ok(self
            .prepared
            .as_ref()
            .ok_or(ArchiveError::Identity)?
            .first_refusal
            .as_ref())
    }

    /// A refused driver/work/root cannot become foreground-ready through
    /// inspection. The original typed refusal remains separately retained.
    pub(super) fn supervisor(&mut self) -> Result<&mut SupervisorRoot<D>, ArchiveError> {
        let prepared = self.prepared.as_mut().ok_or(ArchiveError::Identity)?;
        if prepared.first_refusal.is_some() {
            return Err(ArchiveError::Identity);
        }
        match &mut prepared.stage {
            PreparationStage::Ready(root) => Ok(root),
            _ => Err(ArchiveError::Identity),
        }
    }
}

impl<D: JoinedArchiveSource> PreparationOwner<D> {
    /// Consume the existing ORIGINAL-box preparation result exactly once.
    /// Every error MOVE keeps its returned owner and typed first cause. No
    /// effect occurs and there is no method to retry/rebind a refused owner.
    /// The caller must check complete original workspace fit BEFORE invoking
    /// the box preparer; RetainedArchive::fixed_bytes includes this holder.
    pub(super) fn prepare(
        work: Result<ArchiveWork<D>, (ArchiveError, Box<D>)>,
        expected: &Operation,
    ) -> Self {
        let prepared = match work {
            Err((error, driver)) => PreparedOwner {
                stage: PreparationStage::Driver(driver),
                first_refusal: Some(error),
            },
            Ok(work) if !work.original.same_instance(expected) => PreparedOwner {
                stage: PreparationStage::Work(work),
                first_refusal: Some(ArchiveError::Identity),
            },
            Ok(work) => Self::prepare_work(work, expected),
        };
        Self {
            original: expected.clone(),
            slot: Mutex::new(Some(prepared)),
        }
    }

    fn prepare_work(work: ArchiveWork<D>, expected: &Operation) -> PreparedOwner<D> {
        match RetainedArchive::prepare(work) {
            Err((error, work)) => PreparedOwner {
                stage: PreparationStage::Work(work),
                first_refusal: Some(error),
            },
            Ok(root) => Self::prepare_root(root, expected),
        }
    }

    fn prepare_root(root: RetainedArchive<D>, expected: &Operation) -> PreparedOwner<D> {
        match SupervisorRoot::prepare(root, expected) {
            Err((error, root)) => PreparedOwner {
                stage: PreparationStage::Root(root),
                first_refusal: Some(error),
            },
            Ok(root) => PreparedOwner {
                stage: PreparationStage::Ready(root),
                first_refusal: None,
            },
        }
    }

    /// Inspection/cleanup borrow after overflow does not renew dispatch. The
    /// outer lifetime excludes replacement; each inner root gate stays intact.
    pub(super) fn borrow(
        &mut self,
        expected: &Operation,
    ) -> Result<PreparationLease<'_, D>, ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        let slot = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        if slot.is_none() {
            return Err(ArchiveError::Identity);
        }
        let mut lease = PreparationLease {
            slot,
            prepared: None,
        };
        lease.prepared = lease.slot.take();
        Ok(lease)
    }

    /// No evidence/telemetry/body clone or opaque payload reference escapes.
    /// A poisoned/busy/in-service slot refuses without waiting or inference.
    pub(super) fn position(
        &self,
        expected: &Operation,
    ) -> Result<(PreparationPosition, bool), ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        let slot = self.slot.try_lock().map_err(|_| ArchiveError::Identity)?;
        let prepared = slot.as_ref().ok_or(ArchiveError::Identity)?;
        let position = match &prepared.stage {
            PreparationStage::Driver(_) => PreparationPosition::Driver,
            PreparationStage::Work(_) => PreparationPosition::Work,
            PreparationStage::Root(_) => PreparationPosition::Root,
            PreparationStage::Ready(_) => PreparationPosition::Ready,
        };
        Ok((position, prepared.first_refusal.is_some()))
    }
}

impl PreparationOwner<super::claude::ClaudeDriver> {
    #[cfg(test)]
    pub(super) fn prepare_from_slot_for_source_control(
        slot: &mut Option<Box<super::claude::ClaudeDriver>>,
        original: &Operation,
        before_take: impl FnOnce(),
    ) -> Result<Self, ArchiveError> {
        Self::prepare_from_slot_boundary(slot, original, before_take)
    }

    /// Facade preparation from an installed caller slot. All checks, operation
    /// clones, Arc allocation and nested-slot access precede the sole Box MOVE.
    /// Refusal/unwind before that move leaves the SAME Box in the caller slot;
    /// the final work installation is plain assignment, no callback/allocation.
    /// The caller retains this returned owner before any other fallible step.
    pub(super) fn prepare_from_slot(
        slot: &mut Option<Box<super::claude::ClaudeDriver>>,
        original: &Operation,
    ) -> Result<Self, ArchiveError> {
        Self::prepare_from_slot_boundary(slot, original, || {})
    }

    /// Private constructor boundary; production supplies ONLY the empty closure
    /// above. Controls may unwind immediately before the canonical MOVE.
    fn prepare_from_slot_boundary(
        slot: &mut Option<Box<super::claude::ClaudeDriver>>,
        original: &Operation,
        before_take: impl FnOnce(),
    ) -> Result<Self, ArchiveError> {
        slot.as_mut()
            .ok_or(ArchiveError::Identity)?
            .check_archive_preparation(original)?;
        original.check()?;
        let required = RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes()?
            .checked_add(std::mem::size_of_val(&before_take))
            .ok_or(ArchiveError::Fit)?;
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            required,
        )?;
        let work_original = original.clone();
        let supervisor_original = original.clone();
        let preparation_original = original.clone();
        // Empty local prototype cannot escape or launch. Its fresh shared Arc
        // is still exclusive when the canonical work is installed below.
        let root = RetainedArchive {
            original: original.clone(),
            worker: None,
            worker_join: None,
            std_worker: None,
            std_registration_error: None,
            std_start_failure: None,
            std_pending_slot: None,
            std_launch_started: false,
            std_launch_error: None,
            shared: Arc::new(RetainedShared {
                state: Mutex::new(RetainedState {
                    work: None,
                    checkout_started: false,
                    in_service: false,
                    first_interruption: None,
                    registered_worker: None,
                    observed_worker: None,
                }),
                returned: Notify::new(),
                std_registration: super::archive_thread::ThreadRegistration::new(original),
            }),
        };
        let mut owner = Self {
            original: preparation_original,
            slot: Mutex::new(Some(PreparedOwner {
                first_refusal: None,
                stage: PreparationStage::Ready(SupervisorRoot {
                    original: supervisor_original,
                    slot: Mutex::new(Some(root)),
                }),
            })),
        };
        let prepared = owner
            .slot
            .get_mut()
            .map_err(|_| ArchiveError::Identity)?
            .as_mut()
            .ok_or(ArchiveError::Identity)?;
        let PreparationStage::Ready(supervisor) = &mut prepared.stage else {
            return Err(ArchiveError::Identity);
        };
        let root = supervisor
            .slot
            .get_mut()
            .map_err(|_| ArchiveError::Identity)?
            .as_mut()
            .ok_or(ArchiveError::Identity)?;
        let state = Arc::get_mut(&mut root.shared)
            .ok_or(ArchiveError::Identity)?
            .state
            .get_mut()
            .map_err(|_| ArchiveError::Identity)?;
        if state.work.is_some() {
            return Err(ArchiveError::Identity);
        }
        before_take(); // Every closure capture/drop finishes BEFORE the Box MOVE.
        // LAST fallible step. None refuses without removing anything. Once
        // Some is taken, no operation check/clone/lock/allocation/await follows.
        let driver = slot.take().ok_or(ArchiveError::Identity)?;
        state.work = Some(ArchiveWork {
            original: work_original,
            driver,
            attempted: false,
            first_interruption: None,
            result: None,
            panic: None,
            worker_refusal: None,
            custody_accepted: false,
            custody_capture: CustodyCapture::empty(),
        });
        Ok(owner)
    }

    /// Separate captured-evidence MOVE BEFORE archival, straight into the
    /// caller's already creation-admitted original journal slot. The caller's
    /// first supervisor Err/timeout is a DIFFERENT slot and stays untouched.
    /// This does not authorize archive/joins/actor/CAS or another operation.
    pub(super) fn handoff_captured_before_archive(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<super::TurnOutcome>,
    ) -> Result<bool, ArchiveError> {
        if !self.original.same_instance(expected)
            || !expected.same_instance(destination_original)
            || destination.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes()?,
        )?;
        let mut preparation = self.borrow(expected)?;
        let cleanup = preparation.physical_cleanup(expected)?;
        match cleanup {
            PreparationCleanup::Driver(driver) => {
                driver.handoff_original_captured(expected, destination_original, destination)
            }
            PreparationCleanup::Root(root) => {
                root.handoff_captured_before_archive(expected, destination_original, destination)
            }
            PreparationCleanup::Ready(root) => {
                let mut supervision = root.borrow(expected)?;
                supervision.root_mut()?.handoff_captured_before_archive(
                    expected,
                    destination_original,
                    destination,
                )
            }
        }
    }

    /// Synchronous frame-only check for a future erasing/boxing boundary.
    /// Construct/drop concrete futures WITHOUT polling or allocating them.
    /// The eventual facade must call this BEFORE Box::pin, not merely await an
    /// async check. It still needs complete host/payload/library/transient fit.
    /// No public factory/main adoption or native authority follows from this.
    pub(super) fn check_dispatch_frames(
        &mut self,
        expected: &Operation,
    ) -> Result<(), ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        let fixed = RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes()?;
        let start = std::mem::size_of_val(&self.start_original(expected, None));
        let turn =
            std::mem::size_of_val(&self.turn_original(expected, "", std::time::Duration::ZERO));
        let cleanup = std::mem::size_of_val(&self.supervise_before_archive(expected));
        let mut start_result = None;
        let mut start_returned_ok = None;
        let direct = std::mem::size_of_val(&self.dispatch_original_into(
            expected,
            ControllerAction::Start(None),
            DispatchReturnSlot::Start {
                result: &mut start_result,
                returned_ok: &mut start_returned_ok,
            },
        )); // SAME async family includes BOTH concrete action branches.
        let mut cleanup_result = None;
        let mut cleanup_returned_ok = None;
        let retained_cleanup = std::mem::size_of_val(&self.supervise_before_archive_into(
            expected,
            CleanupReturnSlot {
                result: &mut cleanup_result,
                returned_ok: &mut cleanup_returned_ok,
            },
        ));
        // Count the owned refusal-ready frames and erased pointer headers too;
        // insufficient fit never grants permission to allocate even a refusal.
        let start_refusal =
            std::mem::size_of::<std::future::Ready<Result<super::SessionInfo, DispatchError>>>();
        let turn_refusal =
            std::mem::size_of::<std::future::Ready<Result<super::TurnOutcome, DispatchError>>>();
        let cleanup_refusal = std::mem::size_of::<std::future::Ready<Result<(), CleanupError>>>();
        let erased_headers = 3 * std::mem::size_of::<
            std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
        >();
        let required = fixed
            .checked_add(start)
            .and_then(|bytes| bytes.checked_add(turn))
            .and_then(|bytes| bytes.checked_add(cleanup))
            .and_then(|bytes| bytes.checked_add(direct))
            .and_then(|bytes| bytes.checked_add(retained_cleanup))
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<(
                    CleanupReturnSlot<'_>,
                    Option<Result<(), CleanupError>>,
                    Option<bool>,
                )>())
            })
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<(
                    DispatchReturnSlot<'_>,
                    Option<Result<super::SessionInfo, DispatchError>>,
                    Option<bool>,
                    Result<bool, ArchiveError>,
                )>())
            })
            .and_then(|bytes| bytes.checked_add(start_refusal))
            .and_then(|bytes| bytes.checked_add(turn_refusal))
            .and_then(|bytes| bytes.checked_add(cleanup_refusal))
            .and_then(|bytes| bytes.checked_add(erased_headers))
            .ok_or(ArchiveError::Fit)?;
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            required,
        )
    }

    /// Actual retained controller path: write its ORIGINAL result slots while
    /// all nested restore leases still exist. In particular a restoration wake
    /// callback/unwind cannot consume the returned body before root retention.
    async fn dispatch_original_into(
        &mut self,
        expected: &Operation,
        action: ControllerAction<'_>,
        mut destination: DispatchReturnSlot<'_>,
    ) -> Result<bool, ArchiveError> {
        if !destination.matches_empty(action) {
            return Err(ArchiveError::Identity);
        }
        if let Err(cause) = self.check_dispatch_frames(expected) {
            return Ok(destination.retain_refusal(cause));
        }
        if let Err(cause) = expected.check() {
            return Ok(destination.retain_refusal(ArchiveError::Operation(cause)));
        }
        let mut preparation = match self.borrow(expected) {
            Ok(lease) => lease,
            Err(cause) => return Ok(destination.retain_refusal(cause)),
        };
        let supervisor = match preparation.supervisor() {
            Ok(supervisor) => supervisor,
            Err(cause) => return Ok(destination.retain_refusal(cause)),
        };
        let mut supervision = match supervisor.borrow(expected) {
            Ok(lease) => lease,
            Err(cause) => return Ok(destination.retain_refusal(cause)),
        };
        let root = match supervision.root_mut() {
            Ok(root) => root,
            Err(cause) => return Ok(destination.retain_refusal(cause)),
        };
        let mut foreground = match root.borrow_foreground(expected) {
            Ok(lease) => lease,
            Err(cause) => return Ok(destination.retain_refusal(cause)),
        };
        let driver = match foreground.driver_mut() {
            Ok(driver) => driver,
            Err(cause) => return Ok(destination.retain_refusal(cause)),
        };
        if let Err(cause) = driver.check_physical_cleanup_original(expected) {
            return Ok(destination.retain_refusal(cause));
        }
        match (action, destination) {
            (
                ControllerAction::Start(resume),
                DispatchReturnSlot::Start {
                    result,
                    returned_ok,
                },
            ) => {
                let outcome = driver.start_original_owner(resume).await;
                // Infallible typed wrap and first MOVE BEFORE any guard drops.
                Ok(retain_dispatch_return(
                    result,
                    Some(returned_ok),
                    outcome.map_err(DispatchError::Driver),
                ))
            }
            (ControllerAction::Turn(prompt, limit), DispatchReturnSlot::Turn(result)) => {
                let outcome = driver.run_turn_original_owner(prompt, limit).await;
                Ok(retain_dispatch_return(
                    result,
                    None,
                    outcome.map_err(DispatchError::Driver),
                ))
            }
            _ => unreachable!("checked original dispatch slot/action"),
        }
    }

    /// Private unboxed dispatch through the SAME installed canonical root.
    /// All guards restore on cancellation/unwind; no lock crosses native await.
    /// The existing concrete driver body retains its own original host, effect,
    /// parser, prompt, capture and quota checks. This is not a boxing adapter.
    pub(super) async fn start_original(
        &mut self,
        expected: &Operation,
        resume: Option<&str>,
    ) -> Result<super::SessionInfo, DispatchError> {
        self.check_dispatch_frames(expected)
            .map_err(DispatchError::Archive)?;
        expected
            .check()
            .map_err(|error| DispatchError::Archive(ArchiveError::Operation(error)))?;
        let mut preparation = self.borrow(expected).map_err(DispatchError::Archive)?;
        let mut supervision = preparation
            .supervisor()
            .map_err(DispatchError::Archive)?
            .borrow(expected)
            .map_err(DispatchError::Archive)?;
        let mut foreground = supervision
            .root_mut()
            .map_err(DispatchError::Archive)?
            .borrow_foreground(expected)
            .map_err(DispatchError::Archive)?;
        let driver = foreground.driver_mut().map_err(DispatchError::Archive)?;
        driver
            .check_physical_cleanup_original(expected)
            .map_err(DispatchError::Archive)?;
        driver
            .start_original_owner(resume)
            .await
            .map_err(DispatchError::Driver)
    }

    pub(super) async fn turn_original(
        &mut self,
        expected: &Operation,
        prompt: &str,
        limit: std::time::Duration,
    ) -> Result<super::TurnOutcome, DispatchError> {
        self.check_dispatch_frames(expected)
            .map_err(DispatchError::Archive)?;
        expected
            .check()
            .map_err(|error| DispatchError::Archive(ArchiveError::Operation(error)))?;
        let mut preparation = self.borrow(expected).map_err(DispatchError::Archive)?;
        let mut supervision = preparation
            .supervisor()
            .map_err(DispatchError::Archive)?
            .borrow(expected)
            .map_err(DispatchError::Archive)?;
        let mut foreground = supervision
            .root_mut()
            .map_err(DispatchError::Archive)?
            .borrow_foreground(expected)
            .map_err(DispatchError::Archive)?;
        let driver = foreground.driver_mut().map_err(DispatchError::Archive)?;
        driver
            .check_physical_cleanup_original(expected)
            .map_err(DispatchError::Archive)?;
        driver
            .run_turn_original_owner(prompt, limit)
            .await
            .map_err(DispatchError::Driver)
    }

    /// Retained cleanup at EVERY preparation stage. All results/refusals move
    /// into the original root before this stage's leases can restore or wake.
    async fn supervise_before_archive_into(
        &mut self,
        expected: &Operation,
        mut destination: CleanupReturnSlot<'_>,
    ) -> Result<bool, ArchiveError> {
        if !destination.empty() {
            return Err(ArchiveError::Identity);
        }
        let fixed = match RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes() {
            Ok(fixed) => fixed,
            Err(cause) => return Ok(destination.refuse(cause)),
        };
        let mut preparation = match self.borrow(expected) {
            Ok(lease) => lease,
            Err(cause) => return Ok(destination.refuse(cause)),
        };
        let cleanup = match preparation.physical_cleanup(expected) {
            Ok(cleanup) => cleanup,
            Err(cause) => return Ok(destination.refuse(cause)),
        };
        match cleanup {
            PreparationCleanup::Driver(driver) => {
                physical_cleanup_into(driver, expected, fixed, destination).await
            }
            PreparationCleanup::Root(root) => {
                root.supervise_before_archive_into(expected, destination)
                    .await
            }
            PreparationCleanup::Ready(root) => {
                let mut supervision = match root.borrow(expected) {
                    Ok(lease) => lease,
                    Err(cause) => return Ok(destination.refuse(cause)),
                };
                let root = match supervision.root_mut() {
                    Ok(root) => root,
                    Err(cause) => return Ok(destination.refuse(cause)),
                };
                root.supervise_before_archive_into(expected, destination)
                    .await
            }
        }
    }

    /// Physical cleanup of the SAME owner at ANY retained preparation stage.
    /// A first construction refusal remains sticky, even when the empty/real
    /// physical pass succeeds. No retry, promotion, actor/custody or dispatch.
    /// Cancellation restores every nested owner through its existing lease.
    pub(super) async fn supervise_before_archive(
        &mut self,
        expected: &Operation,
    ) -> Result<(), CleanupError> {
        let fixed = RetainedArchive::<super::claude::ClaudeDriver>::fixed_bytes()
            .map_err(CleanupError::Archive)?;
        let mut preparation = self.borrow(expected).map_err(CleanupError::Archive)?;
        let cleanup = preparation
            .physical_cleanup(expected)
            .map_err(CleanupError::Archive)?;
        match cleanup {
            PreparationCleanup::Driver(driver) => {
                driver
                    .check_physical_cleanup_original(expected)
                    .map_err(CleanupError::Archive)?;
                let cleanup = driver.supervise_retained_children();
                let required = fixed
                    .checked_add(std::mem::size_of_val(&cleanup))
                    .ok_or(CleanupError::Archive(ArchiveError::Fit))?;
                crate::failed_evidence::check_retained_publication_workspace::<
                    super::claude::ClaudeDriver,
                >(required)
                .map_err(CleanupError::Archive)?;
                cleanup.await.map_err(|source| {
                    CleanupError::Driver(super::DriverError::Spawn {
                        cli: "claude cleanup",
                        source,
                    })
                })
            }
            PreparationCleanup::Root(root) => {
                let cleanup = root.supervise_before_archive(expected);
                let required = fixed
                    .checked_add(std::mem::size_of_val(&cleanup))
                    .ok_or(CleanupError::Archive(ArchiveError::Fit))?;
                crate::failed_evidence::check_retained_publication_workspace::<
                    super::claude::ClaudeDriver,
                >(required)
                .map_err(CleanupError::Archive)?;
                cleanup.await
            }
            PreparationCleanup::Ready(root) => {
                let mut supervision = root.borrow(expected).map_err(CleanupError::Archive)?;
                let cleanup = supervision
                    .root_mut()
                    .map_err(CleanupError::Archive)?
                    .supervise_before_archive(expected);
                let required = fixed
                    .checked_add(std::mem::size_of_val(&cleanup))
                    .ok_or(CleanupError::Archive(ArchiveError::Fit))?;
                crate::failed_evidence::check_retained_publication_workspace::<
                    super::claude::ClaudeDriver,
                >(required)
                .map_err(CleanupError::Archive)?;
                cleanup.await
            }
        }
    }
}

impl<D> SupervisorLease<'_, D> {
    pub(super) fn root_mut(&mut self) -> Result<&mut RetainedArchive<D>, ArchiveError> {
        self.root.as_mut().ok_or(ArchiveError::Identity)
    }
}

impl<D> Drop for SupervisorLease<'_, D> {
    fn drop(&mut self) {
        assert!(self.slot.is_none(), "original supervisor slot replaced");
        *self.slot = self.root.take();
    }
}

impl<D: JoinedArchiveSource> SupervisorRoot<D> {
    /// Install only a pristine ORIGINAL root, BEFORE native capture/dispatch.
    /// Any refusal returns the entire SAME root; no box/handle/error is discarded.
    /// Fit uses the already creation-reserved workspace, not a late charge.
    pub(super) fn prepare(
        mut root: RetainedArchive<D>,
        expected: &Operation,
    ) -> Result<Self, (ArchiveError, RetainedArchive<D>)> {
        let checked = (|| {
            if !root.original.same_instance(expected) {
                return Err(ArchiveError::Identity);
            }
            expected.check()?;
            crate::failed_evidence::check_retained_publication_workspace::<D>(
                RetainedArchive::<D>::fixed_bytes()?,
            )?;
            // The existing exclusive foreground gate rejects an attempted,
            // checked-out, interrupted or worker-owned root. Construct/drop
            // only its restoration guard: no driver operation or effect.
            drop(root.borrow_foreground(expected)?);
            Ok(())
        })();
        if let Err(error) = checked {
            return Err((error, root));
        }
        Ok(Self {
            original: expected.clone(),
            slot: Mutex::new(Some(root)),
        })
    }

    /// Original cleanup/inspection stays available after overflow/deadline.
    /// Every root method keeps its own effect-specific gates; borrowing is not
    /// admission, custody, settlement or permission for a new Operation.
    pub(super) fn borrow(
        &mut self,
        expected: &Operation,
    ) -> Result<SupervisorLease<'_, D>, ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        let slot = self.slot.get_mut().map_err(|_| ArchiveError::Identity)?;
        if slot
            .as_ref()
            .is_none_or(|root| !root.original.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        // Complete restoration guard FIRST, then synchronous ownership MOVE.
        // No callback, lock or suspension can observe a replacement-ready slot.
        let mut lease = SupervisorLease { slot, root: None };
        lease.root = lease.slot.take();
        Ok(lease)
    }

    /// Bounded metadata only, never a reference to a Send-only panic result.
    /// A busy/poisoned/in-service outer slot refuses immediately. Existing short
    /// root metadata exclusions remain; no worker completion or join is awaited.
    pub(super) fn status(&self, expected: &Operation) -> Result<RetainedStatus, ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        let slot = self.slot.try_lock().map_err(|_| ArchiveError::Identity)?;
        slot.as_ref()
            .ok_or(ArchiveError::Identity)?
            .status(expected)
    }
}

/// Exactly one checkout, with the SAME box moved rather than copied. Restoration
/// is synchronous on return/unwind; it releases no liability, credit or permit.
/// No synchronous lock is held during a callback, filesystem I/O or await.
pub(super) struct ArchiveCheckout<D> {
    shared: Arc<RetainedShared<D>>,
    work: Option<ArchiveWork<D>>,
}

/// Foreground borrow of the original driver BEFORE archive checkout. The root
/// stays installed and exclusively borrowed; cancellation restores the SAME Box.
/// This grants no native effect, archive attempt, settlement or replacement.
/// Driver/host methods must still check their ORIGINAL effect-specific allowance.
pub(super) struct ForegroundDriver<'a, D> {
    checkout: ArchiveCheckout<D>,
    root_borrow: std::marker::PhantomData<&'a mut RetainedArchive<D>>,
}

/// Exclusive returned driver cleanup lease. Actual join/custody gates precede
/// its construction; the mutable root lifetime keeps those retained facts fixed.
/// Do not borrow an opaque panic Result across an await: its payload is Send,
/// not necessarily Sync. This guard owns the SAME restore checkout instead.
struct CustodyCleanupDriver<'a, D> {
    checkout: ArchiveCheckout<D>,
    root_borrow: std::marker::PhantomData<&'a mut RetainedArchive<D>>,
}

/// Pre-archive physical supervision ONLY. The private constructor may retain
/// an interruption, but never exposes a general foreground driver operation.
/// Every native child/reader remains in the SAME Box when this guard restores.
struct PhysicalCleanupDriver<'a, D> {
    checkout: ArchiveCheckout<D>,
    root_borrow: std::marker::PhantomData<&'a mut RetainedArchive<D>>,
}

impl<D> ForegroundDriver<'_, D> {
    pub(super) fn driver(&self) -> Result<&D, ArchiveError> {
        self.checkout
            .work
            .as_ref()
            .map(|work| work.driver.as_ref())
            .ok_or(ArchiveError::Identity)
    }

    pub(super) fn driver_mut(&mut self) -> Result<&mut D, ArchiveError> {
        self.checkout
            .work
            .as_mut()
            .map(|work| work.driver.as_mut())
            .ok_or(ArchiveError::Identity)
    }
}

/// A read-only lease of returned canonical work AFTER the actual worker join.
/// The borrowed join Result keeps the mutable root borrow live. The same guard
/// restores the box on drop/unwind; no result, panic or evidence is consumed.
/// A failed/canceled join may expose diagnostics, never archive custody.
pub(super) struct ReturnedArchive<'a, D> {
    checkout: ArchiveCheckout<D>,
    worker_join: &'a Result<(), JoinError>,
    worker_associated: bool,
    first_interruption: Option<Interruption>,
}

/// Borrowed inspection of SAME canonical work after its actual std thread join.
/// The actual opaque join Result and registration error remain in the root;
/// this mutable-root lease exposes diagnostics only, never mutable custody.
pub(super) struct ReturnedStdArchive<'a, D> {
    checkout: ArchiveCheckout<D>,
    worker_join: &'a std::thread::Result<()>,
    registration_error: Option<&'a ArchiveError>,
    launch_error: Option<&'a ArchiveError>,
    worker_id: std::thread::ThreadId,
    worker_associated: bool,
    first_interruption: Option<Interruption>,
}

#[derive(Clone, Copy)]
pub(super) struct RetainedStatus {
    pub(super) in_service: bool,
    pub(super) returned: bool,
    pub(super) result_retained: bool,
    pub(super) panic_retained: bool,
    pub(super) first_interruption: Option<Interruption>,
    /// Handle storage is still owned, even AFTER its exact join Result arrived.
    pub(super) worker_owned: bool,
    pub(super) worker_join_retained: bool,
    /// Exact task/checkout association only; NOT physical joins or custody.
    pub(super) worker_associated: bool,
    /// Refused START retained separately; NOT a callback result or join.
    pub(super) std_start_refused: bool,
    /// Original launch obligation, including slot in service without a handle.
    /// This is NOT proof that a worker exists, ran or finished.
    pub(super) std_start_pending: bool,
}

impl<D: JoinedArchiveSource> RetainedArchive<D> {
    pub(super) fn fixed_bytes() -> Result<usize, ArchiveError> {
        std::mem::size_of::<Self>()
            .checked_add(std::mem::size_of::<RetainedShared<D>>())
            // Supervisor's Sync storage and exclusive root lease overlap with
            // the canonical root/foreground frames. Conservatively count the
            // inline return variant as well, BEFORE original capture/effects.
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<SupervisorRoot<D>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<SupervisorLease<'_, D>>()))
            // Stageful factory refusal storage and its overlapping MOVE/return
            // headers fit the SAME creation-reserved workspace before capture.
            // No new Box/body/queue is allocated for a refused owner.
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PreparationOwner<D>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PreparedOwner<D>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PreparationStage<D>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PreparationLease<'_, D>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PreparationCleanup<'_, D>>()))
            // Private facade storage exists BEFORE binding/root preparation;
            // all context/refusal/owner headers fit the original workspace.
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<OriginalClaudeOwner>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<OriginalClaudeState>()))
            // Pending factory staging/return storage exists BEFORE original
            // admission. Payloads/worker libraries remain separate obligations.
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    super::claude_caller::PendingClaudeFactory,
                >())
            })
            // Concrete factory root retains inline body until the counted
            // heap allocation is stored. Count both plus safe Box-fill headers
            // in the SAME creation workspace before either body transfer.
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<super::claude::ClaudeDriver>()))
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    Box<std::mem::MaybeUninit<super::claude::ClaudeDriver>>,
                >())
            })
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Result<(), ArchiveError>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<ControllerResults<'_>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<CleanupReturnSlot<'_>>()))
            // Post-custody capture's canonical storage is inside ArchiveWork;
            // count its overlapping return/borrow headers as well. Referenced
            // bodies still require ORIGINAL prepaid growth, not this inline sum.
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<(
                    CustodyCapture,
                    Result<Option<super::TurnOutcome>, CleanupError>,
                    Result<bool, ArchiveError>,
                    &mut CustodyCapture,
                    &mut Option<Result<Option<super::TurnOutcome>, CleanupError>>,
                    &Operation,
                )>())
            })
            // Caller-slot bridge action/destination, return and argument headers
            // overlap at original creation. Concrete inline/heap frames are
            // checked before boxing; owned payloads remain separate obligations.
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<(
                    ControllerAction<'_>,
                    ControllerDestination<'_>,
                    &Operation,
                    &mut OriginalClaudeOwner,
                    usize,
                    Result<usize, ArchiveError>,
                    Result<(), ArchiveError>,
                )>())
            })
            // Approved typed trait destinations and conversion/observation
            // headers coexist with the canonical root until direct MOVE.
            // No new payload copy is permitted by this fixed shape count.
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<(
                    Option<Result<super::SessionInfo, super::DriverError>>,
                    Option<Result<super::TurnOutcome, super::DriverError>>,
                    Option<Result<(), super::DriverError>>,
                    super::RetainedDriverFuture<'_>,
                    Result<super::RetainedDriverFuture<'_>, super::DriverError>,
                    Result<bool, super::DriverError>,
                )>())
            })
            // Existing public capture adapter now uses the SAME checked private
            // MOVE. Count its temporary/returned slot and argument headers at
            // original creation; body MOVE is not a second payload allocation.
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<(
                    Option<super::TurnOutcome>,
                    Result<Option<super::TurnOutcome>, super::DriverError>,
                    Result<bool, ArchiveError>,
                    &mut super::claude::ClaudeDriver,
                    &Operation,
                )>())
            })
            // Manual private erasure entry's caller return and argument/check
            // headers. Concrete prototype/boxed frame overlap is checked by
            // check_original_controller_erasure BEFORE any Box::pin.
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<(
                    Result<OriginalControllerFuture<'_>, ArchiveError>,
                    Result<(), ArchiveError>,
                    &mut OriginalClaudeOwner,
                    &Operation,
                    Option<&str>,
                    &str,
                    std::time::Duration,
                    usize,
                    usize,
                )>())
            })
            // Synchronous controller capture adds one borrowed original-state
            // projection and return/argument headers above the inner transfer.
            // No captured body is copied; admit overlaps at original creation.
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    Result<&mut OriginalClaudeState, ArchiveError>,
                >())
            })
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Result<bool, ArchiveError>>()))
            .and_then(|bytes| bytes.checked_add(6 * std::mem::size_of::<usize>()))
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    Option<Result<super::TurnOutcome, DispatchError>>,
                >())
            })
            // The start body is MOVED to its caller, not cloned alongside a
            // retained success body. Count the destination header and its
            // synchronous handoff overlap before original capture/effects.
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    Option<Result<super::SessionInfo, DispatchError>>,
                >())
            })
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Result<bool, ArchiveError>>()))
            .and_then(|bytes| bytes.checked_add(6 * std::mem::size_of::<usize>()))
            // Separate cleanup caller slot and synchronous handoff overlap.
            // Error bodies MOVE once; this header reserve is not an allocator,
            // input-growing payload or whole outer/library fit proof.
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<Option<Result<(), CleanupError>>>())
            })
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Result<bool, ArchiveError>>()))
            .and_then(|bytes| bytes.checked_add(6 * std::mem::size_of::<usize>()))
            // Borrowed-slot constructor's returned owner and temporary tokens
            // overlap BEFORE any canonical Box is removed from its caller.
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<Result<PreparationOwner<D>, ArchiveError>>())
            })
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Option<Box<D>>>()))
            .and_then(|bytes| bytes.checked_add(3 * std::mem::size_of::<Operation>()))
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    Result<PreparationCleanup<'_, D>, ArchiveError>,
                >())
            })
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    Result<ArchiveWork<D>, (ArchiveError, Box<D>)>,
                >())
            })
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    Result<RetainedArchive<D>, (ArchiveError, ArchiveWork<D>)>,
                >())
            })
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    Result<SupervisorRoot<D>, (ArchiveError, RetainedArchive<D>)>,
                >())
            })
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<ArchiveCheckout<D>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<ForegroundDriver<'_, D>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<CustodyCleanupDriver<'_, D>>()))
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<PhysicalCleanupDriver<'_, D>>())
            })
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<ReturnedArchive<'_, D>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<ReturnedStdArchive<'_, D>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<RetainedStatus>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<CleanupError>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<DispatchError>()))
            // Original journal destination header and synchronous handoff
            // parameters/results overlap with the canonical restoration lease.
            // Payload is MOVED, not mirrored or newly charged after overflow.
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Option<super::TurnOutcome>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Result<bool, ArchiveError>>()))
            .and_then(|bytes| bytes.checked_add(4 * std::mem::size_of::<usize>()))
            // Include overlapping custody measurement/lease headers BEFORE
            // this original root's Arc allocation, never after capture overflow.
            .and_then(|bytes| {
                bytes.checked_add(super::claude::ClaudeDriver::archive_custody_fixed_bytes())
            })
            .and_then(|bytes| bytes.checked_add(super::archive_thread::fixed_metadata_bytes()))
            // Concrete producer's overlapping inline headers are admitted at
            // root creation. The settled per-thread policy separately covers
            // name/boxed closure and std/OS/allocator storage, not measured fit.
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<std::thread::Builder>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Operation>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<ArchiveCallback<D>>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<ArchiveSpawn<D>>()))
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    std::io::Result<std::thread::JoinHandle<()>>,
                >())
            })
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    Result<(super::archive_thread::ThreadSlot, ArchiveCheckout<D>), ArchiveError>,
                >())
            })
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Result<(), JoinError>>()))
            .and_then(|bytes| bytes.checked_add(4 * std::mem::size_of::<usize>()))
            .and_then(|bytes| {
                bytes
                    .checked_add(2 * std::mem::size_of::<tokio::sync::futures::Notified<'static>>())
            })
            .ok_or(ArchiveError::Fit)
    }

    /// Creation-time preparation, BEFORE capture can overflow. All inline slot,
    /// lease, Arc counters/alignment and borrowed notification state fit in the
    /// SAME previously reserved fixed workspace before the Arc allocation.
    /// Actual runtime/task/allocator/library storage still needs complete fit.
    pub(super) fn prepare(work: ArchiveWork<D>) -> Result<Self, (ArchiveError, ArchiveWork<D>)> {
        let checked = (|| {
            work.original.check()?;
            if work.attempted || work.custody_accepted {
                return Err(ArchiveError::Identity);
            }
            crate::failed_evidence::check_retained_publication_workspace::<D>(Self::fixed_bytes()?)
        })();
        if let Err(error) = checked {
            return Err((error, work));
        }
        let first_interruption = work.first_interruption;
        let std_registration = super::archive_thread::ThreadRegistration::new(&work.original);
        Ok(Self {
            original: work.original.clone(),
            worker: None,
            worker_join: None,
            std_worker: None,
            std_registration_error: None,
            std_start_failure: None,
            std_pending_slot: None,
            std_launch_started: false,
            std_launch_error: None,
            shared: Arc::new(RetainedShared {
                state: Mutex::new(RetainedState {
                    work: Some(work),
                    checkout_started: false,
                    in_service: false,
                    first_interruption,
                    registered_worker: None,
                    observed_worker: None,
                }),
                returned: Notify::new(),
                std_registration,
            }),
        })
    }

    /// Borrow for existing foreground inspection/driver operations only. The
    /// original reserve/root must already exist BEFORE native capture. Do not
    /// use an expired/overflowed borrow as new dispatch permission: this method
    /// intentionally preserves cleanup/inspection access to retained evidence.
    /// It touches no deadline/credit/attempt state and holds no lock over caller
    /// code, an await or I/O. A later archive start requires this guard restored.
    pub(super) fn borrow_foreground(
        &mut self,
        expected: &Operation,
    ) -> Result<ForegroundDriver<'_, D>, ArchiveError> {
        if !self.original.same_instance(expected)
            || self.worker.is_some()
            || self.worker_join.is_some()
            || self.std_worker.is_some()
            || self.std_registration_error.is_some()
            || self.std_start_failure.is_some()
            || self.std_pending_slot.is_some()
            || self.std_launch_started
            || self.std_launch_error.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| ArchiveError::Identity)?;
        if state.checkout_started
            || state.in_service
            || state.first_interruption.is_some()
            || state.work.as_ref().is_none_or(|work| {
                !work.original.same_instance(expected)
                    || work.attempted
                    || work.result.is_some()
                    || work.panic.is_some()
                    || work.worker_refusal.is_some()
                    || work.custody_accepted
                    || work.first_interruption.is_some()
            })
        {
            return Err(ArchiveError::Identity);
        }
        // Complete guard FIRST, then synchronous MOVE. Arc clone allocates
        // nothing; guard headers were included before root creation allocation.
        let mut borrowed = ForegroundDriver {
            checkout: ArchiveCheckout {
                shared: self.shared.clone(),
                work: None,
            },
            root_borrow: std::marker::PhantomData,
        };
        state.in_service = true;
        borrowed.checkout.work = state.work.take();
        drop(state);
        Ok(borrowed)
    }

    pub(super) fn checkout(
        &self,
        expected: &Operation,
    ) -> Result<ArchiveCheckout<D>, ArchiveError> {
        if !self.original.same_instance(expected)
            || self.std_pending_slot.is_some()
            || self.std_launch_started
            || self.std_start_failure.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| ArchiveError::Identity)?;
        if state.checkout_started || state.in_service || state.work.is_none() {
            return Err(ArchiveError::Identity);
        }
        // Construct the guard before removing the canonical box. Arc clone is
        // allocation-free; no callback or suspension occurs during this move.
        let mut checkout = ArchiveCheckout {
            shared: self.shared.clone(),
            work: None,
        };
        state.checkout_started = true;
        state.in_service = true;
        checkout.work = state.work.take();
        Ok(checkout)
    }

    /// Private physical-only route before archive checkout. An original
    /// interruption closes foreground dispatch but must not strand the children
    /// whose actual joins archival still requires. No allowance check/renewal,
    /// checkout-start latch, result/custody change or worker wait occurs here.
    fn borrow_physical_cleanup(
        &mut self,
        expected: &Operation,
    ) -> Result<PhysicalCleanupDriver<'_, D>, ArchiveError> {
        if !self.original.same_instance(expected)
            || self.worker.is_some()
            || self.worker_join.is_some()
            || self.std_worker.is_some()
            || self.std_registration_error.is_some()
            || self.std_start_failure.is_some()
            || self.std_pending_slot.is_some()
            || self.std_launch_started
            || self.std_launch_error.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        let mut state = self
            .shared
            .state
            .try_lock()
            .map_err(|_| ArchiveError::Identity)?;
        if state.checkout_started
            || state.in_service
            || state.work.as_ref().is_none_or(|work| {
                !work.original.same_instance(expected)
                    || work.attempted
                    || work.result.is_some()
                    || work.panic.is_some()
                    || work.worker_refusal.is_some()
                    || work.custody_accepted
            })
        {
            return Err(ArchiveError::Identity);
        }
        let mut borrowed = PhysicalCleanupDriver {
            checkout: ArchiveCheckout {
                shared: self.shared.clone(),
                work: None,
            },
            root_borrow: std::marker::PhantomData,
        };
        state.in_service = true;
        borrowed.checkout.work = state.work.take();
        drop(state); // No metadata lock during actual physical supervision.
        Ok(borrowed)
    }

    /// Retain an already-admitted ORIGINAL slot in this installed root BEFORE
    /// the producer takes the canonical guard or invokes Builder. This does not
    /// claim/admit/launch anything. The concrete producer uses claim_std_start
    /// so no unowned claimed slot crosses a fallible retention boundary.
    /// Even expiry/cancellation must not erase a slot already claimed elsewhere.
    pub(super) fn retain_std_start_slot(
        &mut self,
        expected: &Operation,
        slot: super::archive_thread::ThreadSlot,
    ) -> Result<(), (ArchiveError, super::archive_thread::ThreadSlot)> {
        if !self.original.same_instance(expected)
            || self.worker.is_some()
            || self.worker_join.is_some()
            || self.std_worker.is_some()
            || self.std_registration_error.is_some()
            || self.std_start_failure.is_some()
            || self.std_pending_slot.is_some()
            || self.std_launch_started
        {
            return Err((ArchiveError::Identity, slot));
        }
        if let Err(error) = slot.check_unstarted(expected) {
            return Err((error, slot));
        }
        let state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(_) => return Err((ArchiveError::Identity, slot)),
        };
        if state.checkout_started
            || state.in_service
            || !state
                .work
                .as_ref()
                .is_some_and(|work| work.original.same_instance(expected) && !work.attempted)
        {
            return Err((ArchiveError::Identity, slot));
        }
        self.std_pending_slot = Some(slot); // Canonical root owns BEFORE handoff.
        drop(state);
        Ok(())
    }

    /// One synchronous MOVE from the retained root to its future std producer.
    /// Latch original launch ownership BEFORE taking slot/Box. None thereafter
    /// means in service, not available. No Builder/thread effect happens here.
    /// Returned owners must remain together with this root through actual
    /// handle retention or spawn-Err retention; no await/retry/drop disposition.
    pub(super) fn begin_std_launch(
        &mut self,
        expected: &Operation,
    ) -> Result<(super::archive_thread::ThreadSlot, ArchiveCheckout<D>), ArchiveError> {
        if !self.original.same_instance(expected)
            || self.std_launch_started
            || self.std_worker.is_some()
            || self.std_start_failure.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        self.std_pending_slot
            .as_ref()
            .ok_or(ArchiveError::Identity)?
            .check_unstarted(expected)?;
        self.original.allowance().check()?; // SAME deadline, no overflow recharge.
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| ArchiveError::Identity)?;
        if state.checkout_started
            || state.in_service
            || state.first_interruption.is_some()
            || !state
                .work
                .as_ref()
                .is_some_and(|work| work.original.same_instance(expected) && !work.attempted)
        {
            return Err(ArchiveError::Identity);
        }
        let mut checkout = ArchiveCheckout {
            shared: self.shared.clone(),
            work: None,
        };
        self.std_launch_started = true; // Sticky BEFORE owners leave root storage.
        state.checkout_started = true;
        state.in_service = true;
        checkout.work = state.work.take();
        let slot = self
            .std_pending_slot
            .take()
            .expect("validated original launch slot");
        drop(state); // No lock across producer code/Builder/registration/I/O.
        Ok((slot, checkout))
    }

    /// Claim only after all root checks, and install immediately before any
    /// fallible handoff. Holding metadata here spans only the allocation-free
    /// atomic capacity claim and original Arc-header clone, never a callback.
    fn claim_std_start(
        &mut self,
        expected: &Operation,
        capacity: &'static super::archive_thread::ThreadCapacity,
    ) -> Result<(), ArchiveError> {
        if !self.original.same_instance(expected)
            || self.worker.is_some()
            || self.worker_join.is_some()
            || self.std_worker.is_some()
            || self.std_registration_error.is_some()
            || self.std_start_failure.is_some()
            || self.std_pending_slot.is_some()
            || self.std_launch_started
            || self.std_launch_error.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        let state = self
            .shared
            .state
            .lock()
            .map_err(|_| ArchiveError::Identity)?;
        if state.checkout_started
            || state.in_service
            || state.first_interruption.is_some()
            || !state.work.as_ref().is_some_and(|work| {
                work.original.same_instance(expected)
                    && !work.attempted
                    && work.worker_refusal.is_none()
            })
        {
            return Err(ArchiveError::Identity);
        }
        self.std_pending_slot = Some(capacity.try_claim(&self.original, expected)?);
        drop(state);
        Ok(())
    }

    pub(super) fn std_launch_error(
        &self,
        expected: &Operation,
    ) -> Result<Option<&ArchiveError>, ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        Ok(self.std_launch_error.as_ref())
    }

    /// Register an already-started ORIGINAL worker synchronously BEFORE its
    /// first borrowed await. If the checkout's actual task was already observed,
    /// require its exact ID. Otherwise retain first and require observation before
    /// perform_worker. No supplied ID manufactures association or allocation/
    /// effect admission; the concrete launch producer remains separately needed.
    /// Refusal MOVES the SAME handle back, never aborts/drops/detaches it.
    pub(super) fn retain_worker(
        &mut self,
        expected: &Operation,
        worker: JoinHandle<()>,
    ) -> Result<(), (ArchiveError, JoinHandle<()>)> {
        if !self.original.same_instance(expected)
            || self.worker.is_some()
            || self.worker_join.is_some()
            || self.std_worker.is_some()
            || self.std_start_failure.is_some()
            || self.std_pending_slot.is_some()
            || self.std_launch_started
        {
            return Err((ArchiveError::Identity, worker));
        }
        let mut state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(_) => return Err((ArchiveError::Identity, worker)),
        };
        if !state.checkout_started
            || state.registered_worker.is_some()
            || state.observed_worker.is_some_and(|id| id != worker.id())
        {
            return Err((ArchiveError::Identity, worker));
        }
        state.registered_worker = Some(worker.id());
        self.worker = Some(worker); // Store BEFORE unlock/wake, without await/callback.
        drop(state);
        // Registration is not association until the actual checkout task agrees.
        self.shared.returned.notify_waiters();
        Ok(())
    }

    /// SOURCE std replacement path. Store the actual std owner in this installed
    /// canonical root BEFORE registration can wake the checkout. Refusal before
    /// installation returns the SAME complete owner. False after installation
    /// retains both worker and registration error HERE, closes callback admission
    /// and still requires actual nonblocking completion/Box restoration later.
    /// Never detach/drop an error-path handle, or infer joins from registration.
    pub(super) fn retain_std_worker(
        &mut self,
        expected: &Operation,
        worker: super::archive_thread::StdArchiveWorker,
    ) -> Result<bool, (ArchiveError, super::archive_thread::StdArchiveWorker)> {
        if !self.original.same_instance(expected)
            || self.worker.is_some()
            || self.worker_join.is_some()
            || self.std_worker.is_some()
            || self.std_start_failure.is_some()
            || self.std_pending_slot.is_some()
            || !self.std_launch_started
        {
            return Err((ArchiveError::Identity, worker));
        }
        if let Err(error) = worker.check_original(expected) {
            return Err((error, worker));
        }
        let state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(_) => return Err((ArchiveError::Identity, worker)),
        };
        if !state.checkout_started || state.registered_worker.is_some() {
            return Err((ArchiveError::Identity, worker));
        }
        drop(state); // No synchronous lock spans registration/wake or join.
        self.std_worker = Some(worker); // ORIGINAL actual handle retained FIRST.
        let registered = self.shared.std_registration.register(
            expected,
            self.std_worker.as_ref().expect("installed std worker"),
        );
        if let Err(error) = registered {
            self.std_registration_error = Some(error); // MOVE, no clone/format.
            // Poison also refuses workers; close failure cannot lose the owner.
            let _ = self.shared.std_registration.close(expected);
            return Ok(false);
        }
        Ok(true)
    }

    /// Operator/async path polls only; a running original handle stays retained.
    /// Actual join Result remains owned inside std_worker, including opaque panic.
    pub(super) fn poll_std_join(&mut self, expected: &Operation) -> Result<bool, ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        self.std_worker
            .as_mut()
            .ok_or(ArchiveError::Identity)?
            .poll_join(expected)
    }

    pub(super) fn std_join(
        &self,
        expected: &Operation,
    ) -> Result<Option<&std::thread::Result<()>>, ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        match &self.std_worker {
            Some(worker) => worker.joined(expected),
            None => Ok(None),
        }
    }

    pub(super) fn std_registration_error(
        &self,
        expected: &Operation,
    ) -> Result<Option<&ArchiveError>, ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        Ok(self.std_registration_error.as_ref())
    }

    /// Install the actual Builder Err ONLY after its closure's SAME guard has
    /// restored the original unattempted Box. Refusals return the complete owner;
    /// no await, I/O, formatting, callback, capacity release or retry is added.
    /// The concrete producer preserves returned owners even if this check refuses.
    pub(super) fn retain_std_start_failure(
        &mut self,
        expected: &Operation,
        failure: super::archive_thread::RefusedStdStart,
    ) -> Result<(), (ArchiveError, super::archive_thread::RefusedStdStart)> {
        if !self.original.same_instance(expected)
            || self.worker.is_some()
            || self.worker_join.is_some()
            || self.std_worker.is_some()
            || self.std_registration_error.is_some()
            || self.std_start_failure.is_some()
            || self.std_pending_slot.is_some()
            || !self.std_launch_started
        {
            return Err((ArchiveError::Identity, failure));
        }
        if let Err(error) = failure.check_original(expected) {
            return Err((error, failure));
        }
        let state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(_) => return Err((ArchiveError::Identity, failure)),
        };
        if !state.checkout_started
            || state.in_service
            || state.registered_worker.is_some()
            || state.observed_worker.is_some()
            || !state.work.as_ref().is_some_and(|work| {
                work.original.same_instance(expected)
                    && !work.attempted
                    && work.result.is_none()
                    && work.panic.is_none()
            })
        {
            return Err((ArchiveError::Identity, failure));
        }
        self.std_start_failure = Some(failure); // Root owns opaque Err/slot FIRST.
        drop(state);
        // Nonwaiting closure only. Even poison cannot lose the installed owner.
        let _ = self.shared.std_registration.close(expected);
        Ok(())
    }

    pub(super) fn std_start_error(
        &self,
        expected: &Operation,
    ) -> Result<Option<&std::io::Error>, ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        match &self.std_start_failure {
            Some(failure) => {
                failure.check_original(expected)?;
                Ok(Some(failure.error()))
            }
            None => Ok(None),
        }
    }

    /// Borrow the stored handle, preserving it on wait cancellation. Store the
    /// actual join Result synchronously, retaining the completed handle too.
    /// Tokio task IDs may be reused once task exit AND all handles are gone;
    /// holding this original handle keeps checkout association valid through
    /// custody. No ordinary join/drop releases original ownership or credit.
    /// Opaque panic/cancellation errors remain original values, never text.
    /// Ok here means only a join Result was retained, including Err; neither
    /// file publication, original child/readers joins nor healthy custody follows.
    pub(super) async fn join_worker(&mut self, expected: &Operation) -> Result<(), ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        if self.worker_join.is_some() {
            return Ok(()); // Return the same retained fact, never repeat-poll a handle.
        }
        let result = self.worker.as_mut().ok_or(ArchiveError::Identity)?.await;
        self.worker_join = Some(result); // Synchronous MOVE, no suspension/callback.
        // Keep the SAME completed handle; never repeat-poll after result storage.
        // Root disposition/process exit, not join alone, governs its lifetime.
        Ok(())
    }

    pub(super) fn worker_join(
        &self,
        expected: &Operation,
    ) -> Result<Option<&Result<(), JoinError>>, ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        Ok(self.worker_join.as_ref())
    }

    /// Borrow the SAME returned work for inspection without holding metadata
    /// across the reader's code. Restoration alone refuses: retain the actual
    /// original worker join Result first, INCLUDING failure/cancellation. That
    /// Result and its completed handle remain in the root. No second publication
    /// checkout, mutable driver access, callback, custody or healthy ACK exists
    /// through this lease; allocation/physical/accepted-file proof stays separate.
    pub(super) fn read_returned(
        &mut self,
        expected: &Operation,
    ) -> Result<ReturnedArchive<'_, D>, ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        let worker_join = self.worker_join.as_ref().ok_or(ArchiveError::Identity)?;
        let worker = self.worker.as_ref().ok_or(ArchiveError::Identity)?;
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| ArchiveError::Identity)?;
        if !state.checkout_started
            || state.in_service
            || state.registered_worker != Some(worker.id())
            || !worker.is_finished()
            || !state
                .work
                .as_ref()
                .is_some_and(|work| work.original.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        // Construct the sole restoration guard BEFORE taking the canonical work.
        // Arc clone is allocation-free. The creation-time fixed sizing includes
        // this lease; no post-overflow allocation, charge or admission renewal.
        let mut returned = ReturnedArchive {
            checkout: ArchiveCheckout {
                shared: self.shared.clone(),
                work: None,
            },
            worker_join,
            worker_associated: state.observed_worker == state.registered_worker,
            first_interruption: state.first_interruption,
        };
        state.in_service = true;
        returned.checkout.work = state.work.take();
        drop(state); // No reader code runs with the synchronous lock held.
        Ok(returned)
    }

    /// Restoration alone refuses, as does a Tokio task join. Require this root's
    /// actual original std join Result first, including opaque panic, and SAME
    /// returned original Box outside service. No join, wait or filesystem effect
    /// occurs here. A failed association/registration remains diagnostic only.
    pub(super) fn read_std_returned(
        &mut self,
        expected: &Operation,
    ) -> Result<ReturnedStdArchive<'_, D>, ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        let worker = self.std_worker.as_ref().ok_or(ArchiveError::Identity)?;
        worker.check_original(expected)?;
        let worker_join = worker.joined(expected)?.ok_or(ArchiveError::Identity)?;
        let worker_associated = self.shared.std_registration.associated(expected)?;
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| ArchiveError::Identity)?;
        if !state.checkout_started
            || state.in_service
            || !state
                .work
                .as_ref()
                .is_some_and(|work| work.original.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        // Construct the SAME canonical guard BEFORE taking work. Arc clone has
        // no allocation; lease size was admitted at original root creation.
        // Borrows of actual join/error fields keep the mutable root borrow live.
        let mut returned = ReturnedStdArchive {
            checkout: ArchiveCheckout {
                shared: self.shared.clone(),
                work: None,
            },
            worker_join,
            registration_error: self.std_registration_error.as_ref(),
            launch_error: self.std_launch_error.as_ref(),
            worker_id: worker.id(),
            worker_associated,
            first_interruption: state.first_interruption,
        };
        state.in_service = true;
        returned.checkout.work = state.work.take();
        drop(state); // No synchronous metadata lock crosses inspection code.
        Ok(returned)
    }

    /// Root interruption is independent of the callback Result. While checked
    /// out, no result or body is inspected/inferred through the empty slot.
    /// An unjoined worker may still receive its caller's first interruption after
    /// box restoration. That cause is separate and never rewrites a callback
    /// result. Once an actual join Result is retained, a late cause is ignored.
    pub(super) fn interrupt(
        &self,
        expected: &Operation,
        cause: Interruption,
    ) -> Result<(), ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        let std_joined = self.std_join(expected)?.is_some();
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| ArchiveError::Identity)?;
        if self.worker_join.is_none()
            && !std_joined
            && (!state.checkout_started
                || state.in_service
                || self.worker.is_some()
                || self.std_worker.is_some()
                || (self.std_launch_started && self.std_start_failure.is_none()))
        {
            state.first_interruption.get_or_insert(cause);
        }
        drop(state);
        // Nonwaiting closure of std admission after preserving the first cause.
        // This also covers cancel-before-handle-return/registration. The SAME
        // guard still restores the Box; closure is NOT an actual join/discard.
        self.shared.std_registration.close(expected)?;
        Ok(())
    }

    pub(super) fn status(&self, expected: &Operation) -> Result<RetainedStatus, ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        let std_joined = self.std_join(expected)?.is_some();
        let std_associated = self.shared.std_registration.associated(expected)?;
        let state = self
            .shared
            .state
            .lock()
            .map_err(|_| ArchiveError::Identity)?;
        Ok(RetainedStatus {
            in_service: state.in_service,
            returned: state.checkout_started && !state.in_service && state.work.is_some(),
            result_retained: state
                .work
                .as_ref()
                .is_some_and(|work| work.result.is_some()),
            panic_retained: state.work.as_ref().is_some_and(|work| work.panic.is_some()),
            first_interruption: state.first_interruption,
            worker_owned: self.worker.is_some() || self.std_worker.is_some(),
            worker_join_retained: self.worker_join.is_some() || std_joined,
            worker_associated: std_associated
                || (state.registered_worker.is_some()
                    && state.registered_worker == state.observed_worker),
            std_start_refused: self.std_start_failure.is_some(),
            std_start_pending: self.std_pending_slot.is_some()
                || (self.std_launch_started
                    && self.std_worker.is_none()
                    && self.std_start_failure.is_none()),
        })
    }

    /// The mutable root borrow permits only ONE notification wait at a time.
    /// Await ONLY a borrow of the retained root. Canceled waits touch no owner,
    /// handle, result or cause. Returned means box restoration, NOT worker join,
    /// accepted file custody, original child/readers joins or healthy settlement.
    pub(super) async fn wait_returned(&mut self, expected: &Operation) -> Result<(), ArchiveError> {
        if !self.original.same_instance(expected) {
            return Err(ArchiveError::Identity);
        }
        loop {
            let notified = self.shared.returned.notified();
            tokio::pin!(notified);
            // Register BEFORE checking so restoration between check and await
            // wakes this waiter. A prior restoration is seen directly in state.
            notified.as_mut().enable();
            {
                let state = self
                    .shared
                    .state
                    .lock()
                    .map_err(|_| ArchiveError::Identity)?;
                if !state.checkout_started {
                    return Err(ArchiveError::Identity);
                }
                if !state.in_service && state.work.is_some() {
                    return Ok(());
                }
            }
            notified.await;
        }
    }
}

type ArchiveCallback<D> = fn(&mut D, &Operation) -> Result<ArchiveFileObservation, ArchiveError>;
type ArchiveSpawn<D> = fn(
    std::thread::Builder,
    ArchiveCheckout<D>,
    Operation,
    ArchiveCallback<D>,
) -> std::io::Result<std::thread::JoinHandle<()>>;

impl<D: JoinedArchiveSource + Send + 'static> RetainedArchive<D> {
    /// SOURCE producer only: the caller must already own this installed root.
    /// Creation admitted exact stack, fixed workspace and accepted overhead.
    /// Ok(true) means a handle was retained and registration accepted, NEVER
    /// completed callback, actual join, archive custody or healthy settlement.
    /// Ok(false) keeps a post-claim refusal in this SAME root; it cannot retry.
    pub(super) fn start_std_worker(
        &mut self,
        expected: &Operation,
        callback: ArchiveCallback<D>,
    ) -> Result<bool, ArchiveError> {
        self.start_std_worker_with(
            expected,
            callback,
            &super::archive_thread::ARCHIVE_THREADS,
            spawn_archive_worker::<D>,
        )
    }

    // The private synchronous seam lets controls refuse spawn without starting
    // a thread. Production passes only the named Builder function below.
    fn start_std_worker_with(
        &mut self,
        expected: &Operation,
        callback: ArchiveCallback<D>,
        capacity: &'static super::archive_thread::ThreadCapacity,
        spawn: ArchiveSpawn<D>,
    ) -> Result<bool, ArchiveError> {
        self.claim_std_start(expected, capacity)?;
        let (slot, checkout) = match self.begin_std_launch(expected) {
            Ok(owners) => owners,
            Err(error) => {
                self.std_launch_error = Some(error); // Pending slot stays owned.
                let _ = self.shared.std_registration.close(expected);
                return Ok(false);
            }
        };
        let original = self.original.clone(); // Existing headers, no allocation.
        let builder = std::thread::Builder::new()
            .name("cna-failed-evidence".into())
            .stack_size(crate::failed_evidence::ARCHIVE_WORKER_STACK_BYTES);
        if let Err(error) = original.allowance().check() {
            self.std_pending_slot = Some(slot);
            self.std_launch_error = Some(error);
            drop(checkout); // Restore the SAME box, with no callback or join.
            let _ = self.shared.std_registration.close(expected);
            return Ok(false);
        }
        // No metadata lock crosses Builder/spawn or worker filesystem I/O.
        // The closure owns the restoration guard even on spawn Err or unwind.
        match slot.retain_spawn_outcome(spawn(builder, checkout, original, callback)) {
            Ok(worker) => match self.retain_std_worker(expected, worker) {
                Ok(registered) => Ok(registered),
                Err((error, worker)) => {
                    // Exclusive mutable-root preflight proved this slot empty.
                    // Even poisoned metadata cannot drop/detach the real handle.
                    self.std_worker = Some(worker);
                    self.std_launch_error = Some(error);
                    let _ = self.shared.std_registration.close(expected);
                    Ok(false)
                }
            },
            Err(failure) => {
                // Builder Err dropped its closure and restored the Box already.
                if let Err((error, failure)) = self.retain_std_start_failure(expected, failure) {
                    self.std_start_failure = Some(failure); // Opaque error MOVE FIRST.
                    self.std_launch_error = Some(error);
                }
                let _ = self.shared.std_registration.close(expected);
                Ok(false)
            }
        }
    }
}

fn spawn_archive_worker<D: JoinedArchiveSource + Send + 'static>(
    builder: std::thread::Builder,
    checkout: ArchiveCheckout<D>,
    original: Operation,
    callback: ArchiveCallback<D>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    builder.spawn(move || run_archive_worker(checkout, original, callback))
}

fn run_archive_worker<D: JoinedArchiveSource>(
    mut checkout: ArchiveCheckout<D>,
    original: Operation,
    callback: ArchiveCallback<D>,
) {
    let performed = checkout
        .wait_std_registration(&original)
        .and_then(|()| checkout.perform_std_worker(&original, callback));
    if let Err(error) = performed {
        if let Some(work) = checkout.work.as_mut() {
            work.worker_refusal.get_or_insert(error); // Original typed MOVE.
        }
    }
    // Guard Drop restores canonical Box before actual thread completion.
}

impl<D: JoinedArchiveSource> ReturnedArchive<'_, D> {
    /// Borrow only; neither the canonical box nor the original callback Result
    /// can be moved or mutated by the reader. No filesystem or custom formatting.
    pub(super) fn work(&self) -> Result<&ArchiveWork<D>, ArchiveError> {
        self.checkout.work.as_ref().ok_or(ArchiveError::Identity)
    }

    pub(super) fn worker_join(&self) -> &Result<(), JoinError> {
        self.worker_join
    }

    pub(super) fn worker_associated(&self) -> bool {
        self.worker_associated
    }

    pub(super) fn first_interruption(&self) -> Option<Interruption> {
        self.first_interruption
    }
}

impl<D: JoinedArchiveSource> ReturnedStdArchive<'_, D> {
    pub(super) fn work(&self) -> Result<&ArchiveWork<D>, ArchiveError> {
        self.checkout.work.as_ref().ok_or(ArchiveError::Identity)
    }

    pub(super) fn worker_join(&self) -> &std::thread::Result<()> {
        self.worker_join
    }

    pub(super) fn registration_error(&self) -> Option<&ArchiveError> {
        self.registration_error
    }

    pub(super) fn launch_error(&self) -> Option<&ArchiveError> {
        self.launch_error
    }

    pub(super) fn worker_id(&self) -> std::thread::ThreadId {
        self.worker_id
    }

    pub(super) fn worker_associated(&self) -> bool {
        self.worker_associated
    }

    pub(super) fn first_interruption(&self) -> Option<Interruption> {
        self.first_interruption
    }
}

impl RetainedArchive<super::claude::ClaudeDriver> {
    /// Captured outcome must reach ORIGINAL journal ownership before complete
    /// archive enumeration (which refuses completed_turn still in the driver).
    /// Unlike the post-custody accessor, this uses ONLY the physical-only lease;
    /// actual child/readers joins and file/actor/journal acceptance stay separate.
    pub(super) fn handoff_captured_before_archive(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<super::TurnOutcome>,
    ) -> Result<bool, ArchiveError> {
        if !expected.same_instance(destination_original) || destination.is_some() {
            return Err(ArchiveError::Identity);
        }
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            Self::fixed_bytes()?,
        )?;
        let mut borrowed = self.borrow_physical_cleanup(expected)?;
        let driver = borrowed
            .checkout
            .work
            .as_mut()
            .ok_or(ArchiveError::Identity)?
            .driver
            .as_mut();
        driver.handoff_original_captured(expected, destination_original, destination)
    }

    async fn supervise_before_archive_into(
        &mut self,
        expected: &Operation,
        mut destination: CleanupReturnSlot<'_>,
    ) -> Result<bool, ArchiveError> {
        if !destination.empty() {
            return Err(ArchiveError::Identity);
        }
        let fixed = match Self::fixed_bytes() {
            Ok(fixed) => fixed,
            Err(cause) => return Ok(destination.refuse(cause)),
        };
        let mut borrowed = match self.borrow_physical_cleanup(expected) {
            Ok(lease) => lease,
            Err(cause) => return Ok(destination.refuse(cause)),
        };
        let driver = match borrowed.checkout.work.as_mut() {
            Some(work) => work.driver.as_mut(),
            None => return Ok(destination.refuse(ArchiveError::Identity)),
        };
        // Check the SAME inner concrete family unpolled, BEFORE it can supervise.
        // Prototype borrows end before refusal retention or actual construction.
        let checked = {
            let future = physical_cleanup_into(driver, expected, fixed, destination.reborrow());
            check_cleanup_frame(fixed, &future)
        };
        if let Err(cause) = checked {
            return Ok(destination.refuse(cause));
        }
        physical_cleanup_into(driver, expected, fixed, destination).await
    }

    /// Join original child/readers BEFORE an archive attempt, preserving failed
    /// foreground and interruption ownership. A successful physical pass is
    /// neither archive custody nor prompt/actor/journal/healthy settlement.
    /// Every child uses its existing original deadline; archive threads are
    /// never waited on by this route. The outer supervisor must retain this
    /// root and cancel/exit under its own unchanged original deadline.
    pub(super) async fn supervise_before_archive(
        &mut self,
        expected: &Operation,
    ) -> Result<(), CleanupError> {
        let fixed = Self::fixed_bytes().map_err(CleanupError::Archive)?;
        let mut borrowed = self
            .borrow_physical_cleanup(expected)
            .map_err(CleanupError::Archive)?;
        let driver = borrowed
            .checkout
            .work
            .as_mut()
            .ok_or(CleanupError::Archive(ArchiveError::Identity))?
            .driver
            .as_mut();
        driver
            .check_physical_cleanup_original(expected)
            .map_err(CleanupError::Archive)?;
        let cleanup = driver.supervise_retained_children();
        let required = fixed
            .checked_add(std::mem::size_of_val(&cleanup))
            .ok_or(CleanupError::Archive(ArchiveError::Fit))?;
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(required)
            .map_err(CleanupError::Archive)?;
        cleanup.await.map_err(|source| {
            CleanupError::Driver(super::DriverError::Spawn {
                cli: "claude cleanup",
                source,
            })
        })
    }

    /// Successful ORIGINAL std callback observation only. This synchronous
    /// source-only custody path performs no file I/O, thread launch or wait.
    /// Its full serializer/library fit and supervisor wiring remain required.
    /// Do not invoke on async/operator stop until CPU responsiveness is proven.
    /// Original join/result/registration/first-cause values stay retained, and
    /// raw custody releases no prompt/actor/journal credit or healthy ACK.
    pub(super) fn accept_std_archive(&mut self, expected: &Operation) -> Result<(), ArchiveError> {
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            Self::fixed_bytes()?,
        )?;
        // Actual original std join plus SAME returned Box are required. The
        // lease constructs its restoration guard before taking canonical work.
        let mut returned = self.read_std_returned(expected)?;
        if returned.worker_join.is_err()
            || returned.registration_error.is_some()
            || returned.launch_error.is_some()
            || !returned.worker_associated
        {
            return Err(ArchiveError::Identity);
        }
        let work = returned
            .checkout
            .work
            .as_mut()
            .ok_or(ArchiveError::Identity)?;
        if work.custody_accepted {
            // Same retained acceptance, not a second clearing/measurement of
            // already archived storage. The actual join gate above still holds.
            work.checked_cleanup_source(expected)?;
            return Ok(());
        }
        let (driver, observed) = work.checked_custody_source(expected)?;
        // Rechecks ALL physical children/readers, complete original coverage,
        // unique storage and current borrowed measurement BEFORE any clearing.
        // No metadata lock spans this call; lease drop restores the SAME Box.
        driver.accept_original_archive(expected, observed)?;
        // Synchronous latch BEFORE restoration or suspension. Only the actual
        // complete-custody callback above may authorize this state transition.
        work.custody_accepted = true;
        Ok(())
    }

    /// Original captured outcome disposition AFTER accepted raw custody. The
    /// actual retained worker/join/association and canonical Box stay owned.
    /// This is the existing MOVE into host/journal ownership, never a refund,
    /// healthy ACK or permission to install another Operation.
    pub(super) fn take_captured_after_custody(
        &mut self,
        expected: &Operation,
    ) -> Result<Option<super::TurnOutcome>, CleanupError> {
        if !self.original.same_instance(expected) {
            return Err(CleanupError::Archive(ArchiveError::Identity));
        }
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            Self::fixed_bytes().map_err(CleanupError::Archive)?,
        )
        .map_err(CleanupError::Archive)?;
        let started = {
            let state = self
                .shared
                .state
                .try_lock()
                .map_err(|_| CleanupError::Archive(ArchiveError::Identity))?;
            if state.in_service {
                return Err(CleanupError::Archive(ArchiveError::Identity));
            }
            let work = state
                .work
                .as_ref()
                .filter(|work| work.original.same_instance(expected))
                .ok_or(CleanupError::Archive(ArchiveError::Identity))?;
            work.custody_capture.started
        };
        if !started {
            // Actual original join/association/custody gates remain unchanged.
            let mut cleanup = self
                .borrow_custody_cleanup(expected)
                .map_err(CleanupError::Archive)?;
            let work = cleanup
                .checkout
                .work
                .as_mut()
                .ok_or(CleanupError::Archive(ArchiveError::Identity))?;
            work.checked_cleanup_source(expected)
                .map_err(CleanupError::Archive)?;
            work.custody_capture
                .begin()
                .map_err(CleanupError::Archive)?;
            let returned = work.driver.handoff_original_captured(
                expected,
                expected,
                &mut work.custody_capture.body,
            );
            // Body moved DIRECTLY into canonical work, then actual typed
            // return retained BEFORE cleanup's Drop restores/notifies/wakes.
            work.custody_capture.returned = Some(returned);
        }
        // Re-entry after a restoration unwind only transfers the retained
        // first return. It does not repeat capture or manufacture a join/ACK.
        self.take_retained_custody_capture(expected)
    }

    /// Future supervisor integration must use this direct ORIGINAL destination
    /// before dropping its own outer leases. The by-value helper above alone
    /// does not retain its return across an enclosing supervisor's restoration.
    /// No public trait method or driver effect is added by this private handoff.
    pub(super) fn take_captured_after_custody_into(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<Result<Option<super::TurnOutcome>, CleanupError>>,
    ) -> Result<bool, ArchiveError> {
        if !self.original.same_instance(expected)
            || !expected.same_instance(destination_original)
            || destination.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        let returned = self.take_captured_after_custody(expected);
        // The inner restore completed, or unwound with canonical work intact.
        // From this return to FIRST MOVE there is no guard drop or callback.
        // Caller keeps its outer leases until this exact slot has the body.
        Ok(retain_dispatch_return(destination, None, returned))
    }

    fn take_retained_custody_capture(
        &mut self,
        expected: &Operation,
    ) -> Result<Option<super::TurnOutcome>, CleanupError> {
        if !self.original.same_instance(expected) {
            return Err(CleanupError::Archive(ArchiveError::Identity));
        }
        let mut state = self
            .shared
            .state
            .try_lock()
            .map_err(|_| CleanupError::Archive(ArchiveError::Identity))?;
        if state.in_service {
            return Err(CleanupError::Archive(ArchiveError::Identity));
        }
        let work = state
            .work
            .as_mut()
            .filter(|work| work.original.same_instance(expected))
            .ok_or(CleanupError::Archive(ArchiveError::Identity))?;
        // No ArchiveCheckout/restore guard here: dropping this plain mutex
        // guard cannot call an observer after the final infallible body MOVE.
        work.custody_capture.take_return()
    }

    /// Borrowed cleanup, not an owner transfer or a replacement-ready result.
    /// Canceling this future drops the returned lease and restores the SAME Box,
    /// with the original result, custody latch and pending prompt still retained.
    /// No archive thread is waited on: read_std_returned requires its actual join.
    pub(super) async fn stop_after_custody(
        &mut self,
        expected: &Operation,
    ) -> Result<(), CleanupError> {
        let fixed = Self::fixed_bytes().map_err(CleanupError::Archive)?;
        let mut borrowed = self
            .borrow_custody_cleanup(expected)
            .map_err(CleanupError::Archive)?;
        let driver = borrowed
            .checkout
            .work
            .as_mut()
            .ok_or(CleanupError::Archive(ArchiveError::Identity))?
            .checked_cleanup_source(expected)
            .map_err(CleanupError::Archive)?;
        // Use the concrete, unboxed future. Check its overlapping inline size
        // against the already creation-reserved workspace BEFORE its first
        // poll/effect; this allocates/charges/renews nothing after overflow.
        let cleanup = driver.stop_original_owner();
        let required = fixed
            .checked_add(std::mem::size_of_val(&cleanup))
            .ok_or(CleanupError::Archive(ArchiveError::Fit))?;
        crate::failed_evidence::check_retained_publication_workspace::<super::claude::ClaudeDriver>(
            required,
        )
        .map_err(CleanupError::Archive)?;
        cleanup.await.map_err(CleanupError::Driver)
    }

    fn borrow_custody_cleanup(
        &mut self,
        expected: &Operation,
    ) -> Result<CustodyCleanupDriver<'_, super::claude::ClaudeDriver>, ArchiveError> {
        let mut returned = self.read_std_returned(expected)?;
        returned.cleanup_source(expected)?;
        // MOVE the sole restoration guard. Join/error values remain in the
        // exclusively borrowed root, with no synthetic clone or new witness.
        Ok(CustodyCleanupDriver {
            checkout: returned.checkout,
            root_borrow: std::marker::PhantomData,
        })
    }
}

impl ReturnedStdArchive<'_, super::claude::ClaudeDriver> {
    fn cleanup_source(
        &mut self,
        expected: &Operation,
    ) -> Result<&mut super::claude::ClaudeDriver, ArchiveError> {
        if self.worker_join.is_err()
            || self.registration_error.is_some()
            || self.launch_error.is_some()
            || !self.worker_associated
        {
            return Err(ArchiveError::Identity);
        }
        self.checkout
            .work
            .as_mut()
            .ok_or(ArchiveError::Identity)?
            .checked_cleanup_source(expected)
    }
}

impl<D: JoinedArchiveSource> ArchiveCheckout<D> {
    /// Dedicated std worker ONLY. Validate canonical checkout ownership BEFORE
    /// waiting; the bridge uses actual current ThreadId and original deadline.
    /// The guard remains in service throughout, including cancellation/refusal.
    pub(super) fn wait_std_registration(
        &mut self,
        expected: &Operation,
    ) -> Result<(), ArchiveError> {
        let work = self.work.as_ref().ok_or(ArchiveError::Identity)?;
        if !work.original.same_instance(expected) || work.attempted {
            return Err(ArchiveError::Identity);
        }
        {
            let state = self
                .shared
                .state
                .lock()
                .map_err(|_| ArchiveError::Identity)?;
            if !state.in_service || state.work.is_some() {
                return Err(ArchiveError::Identity);
            }
        }
        self.shared.std_registration.wait_in_worker(expected)
    }

    /// Revalidate the SAME actual std worker immediately before effect. No
    /// Tokio ID checks are relabeled as thread checks. Release all metadata
    /// locks before the existing original callback/opaque-result capture.
    pub(super) fn perform_std_worker(
        &mut self,
        expected: &Operation,
        callback: impl FnOnce(&mut D, &Operation) -> Result<ArchiveFileObservation, ArchiveError>,
    ) -> Result<(), ArchiveError> {
        self.shared.std_registration.validate_current(expected)?;
        {
            let state = self
                .shared
                .state
                .lock()
                .map_err(|_| ArchiveError::Identity)?;
            if !state.in_service || state.work.is_some() {
                return Err(ArchiveError::Identity);
            }
        }
        self.perform(expected, callback)
    }
    /// Borrow the canonical checkout while waiting for synchronous handle
    /// registration. Observe only Tokio's actual current task ID, never a
    /// caller-supplied ID. Cancellation leaves this guard and its box intact.
    /// Two orderings are supported; no callback/I/O/await holds the metadata lock.
    /// This is not launch allocation admission, physical joins or archive ACK.
    pub(super) async fn wait_worker_registration(
        &mut self,
        expected: &Operation,
    ) -> Result<(), ArchiveError> {
        {
            let work = self.work.as_ref().ok_or(ArchiveError::Identity)?;
            if !work.original.same_instance(expected) || work.attempted {
                return Err(ArchiveError::Identity);
            }
        }
        let actual = tokio::task::try_id().ok_or(ArchiveError::Identity)?;
        loop {
            let notified = self.shared.returned.notified();
            tokio::pin!(notified);
            notified.as_mut().enable(); // Register BEFORE inspecting handle state.
            {
                let mut state = self
                    .shared
                    .state
                    .lock()
                    .map_err(|_| ArchiveError::Identity)?;
                if !state.in_service
                    || state.work.is_some()
                    || state.observed_worker.is_some_and(|id| id != actual)
                    || state.registered_worker.is_some_and(|id| id != actual)
                {
                    return Err(ArchiveError::Identity);
                }
                state.observed_worker = Some(actual);
                if state.registered_worker == Some(actual) {
                    return Ok(());
                }
            }
            notified.await;
        }
    }

    /// The eventual native worker uses this entry point. Recheck the actual
    /// task against BOTH retained handle and canonical checkout observations
    /// immediately before effect; neither a visible Operation ID nor a restored
    /// box can substitute. Pure callback/fit controls use the private helper.
    pub(super) fn perform_worker(
        &mut self,
        expected: &Operation,
        callback: impl FnOnce(&mut D, &Operation) -> Result<ArchiveFileObservation, ArchiveError>,
    ) -> Result<(), ArchiveError> {
        let actual = tokio::task::try_id().ok_or(ArchiveError::Identity)?;
        {
            let state = self
                .shared
                .state
                .lock()
                .map_err(|_| ArchiveError::Identity)?;
            if !state.in_service
                || state.work.is_some()
                || state.registered_worker != Some(actual)
                || state.observed_worker != Some(actual)
            {
                return Err(ArchiveError::Identity);
            }
        }
        self.perform(expected, callback)
    }

    /// Test-only canonical inspection through a synthetic failed callback.
    /// Production callers still require the actual worker-association gate.
    #[cfg(test)]
    pub(super) fn inspect_for_source_control(
        &mut self,
        expected: &Operation,
        callback: impl FnOnce(&mut D, &Operation) -> Result<ArchiveFileObservation, ArchiveError>,
    ) -> Result<(), ArchiveError> {
        self.perform(expected, callback)
    }

    fn perform(
        &mut self,
        expected: &Operation,
        callback: impl FnOnce(&mut D, &Operation) -> Result<ArchiveFileObservation, ArchiveError>,
    ) -> Result<(), ArchiveError> {
        let retained = RetainedArchive::<D>::fixed_bytes()?
            .checked_add(std::mem::size_of_val(&callback))
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    Result<Result<ArchiveFileObservation, ArchiveError>, Box<dyn Any + Send>>,
                >())
            })
            .ok_or(ArchiveError::Fit)?;
        crate::failed_evidence::check_retained_publication_workspace::<D>(retained)?;
        self.work
            .as_mut()
            .ok_or(ArchiveError::Identity)?
            .perform(expected, callback)
    }
}

impl<D> Drop for ArchiveCheckout<D> {
    fn drop(&mut self) {
        // Recover a poisoned metadata lock only to preserve canonical ownership;
        // public observation/admission still refuses poison. There is no callback,
        // I/O, await, formatting, body clone, evidence clear or operation settlement.
        let mut state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Only this sole guard can restore work; root methods never insert a box.
        state.work = self.work.take();
        state.in_service = false;
        drop(state);
        self.shared.returned.notify_waiters();
    }
}

impl<D: JoinedArchiveSource> ArchiveWork<D> {
    /// Prepare before native capture can overflow. Refusal returns the ORIGINAL
    /// box unchanged; no callback, allocation, path I/O or owner replacement.
    /// Fixed metadata shares the creation-time original workspace. Full variable
    /// evidence/library/task/future/allocator fit remains separately required.
    pub(super) fn prepare(
        driver: Box<D>,
        original: &Operation,
    ) -> Result<Self, (ArchiveError, Box<D>)> {
        let checked = (|| {
            original.check()?;
            original.archive_plan().ok_or(ArchiveError::Identity)?;
            crate::failed_evidence::check_retained_publication_workspace::<D>(std::mem::size_of::<
                Self,
            >())
        })();
        if let Err(error) = checked {
            return Err((error, driver));
        }
        Ok(Self {
            original: original.clone(),
            driver,
            attempted: false,
            first_interruption: None,
            result: None,
            panic: None,
            worker_refusal: None,
            custody_accepted: false,
            custody_capture: CustodyCapture::empty(),
        })
    }

    /// Latch a caller interruption separately from the archive result. It cannot
    /// rewrite a returned callback error or panic, and success cannot erase it.
    pub(super) fn interrupt(&mut self, cause: Interruption) {
        if self.result.is_none() && self.panic.is_none() {
            self.first_interruption.get_or_insert(cause);
        }
    }

    /// At most one synchronous callback, borrowing the canonical box without a
    /// lock or await. Unwinding retains the ORIGINAL box and opaque panic payload
    /// without formatting it. An abort still follows process-exit Unsettled rules.
    /// This is not a task launch, actual join, custody or healthy settlement ACK.
    pub(super) fn perform(
        &mut self,
        expected: &Operation,
        callback: impl FnOnce(&mut D, &Operation) -> Result<ArchiveFileObservation, ArchiveError>,
    ) -> Result<(), ArchiveError> {
        if !self.original.same_instance(expected) || self.attempted || self.worker_refusal.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        // Recheck the ORIGINAL absolute deadline here, not only at a caller's
        // registration boundary. Capture overflow can use already reserved
        // cleanup storage; Operation::check would incorrectly refuse that lane.
        self.original.allowance().check()?;
        // Count the concrete closure and synchronous unwind result as well as
        // the owner before effect. Hidden variable storage/library/task fit is
        // not inferred from these inline sizes; native callback is a function pointer.
        let retained = std::mem::size_of::<Self>()
            .checked_add(std::mem::size_of_val(&callback))
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    Result<Result<ArchiveFileObservation, ArchiveError>, Box<dyn Any + Send>>,
                >())
            })
            .ok_or(ArchiveError::Fit)?;
        crate::failed_evidence::check_retained_publication_workspace::<D>(retained)?;
        self.original.allowance().check()?; // Immediately before attempt/effect.
        self.attempted = true; // BEFORE callback/panic/effect; never renewed by a wait/drop.
        match catch_unwind(AssertUnwindSafe(|| {
            callback(&mut self.driver, &self.original)
        })) {
            Ok(result) => self.result = Some(result), // Synchronous MOVE before return.
            Err(payload) => self.panic = Some(payload), // Original opaque payload, not text.
        }
        Ok(())
    }

    /// Private projection used only behind the actual std-join/association gate.
    /// A callback Err/panic/absence cannot be exchanged for a recovery observation
    /// here. This projection itself clears nothing and supplies no join proof.
    fn checked_custody_source(
        &mut self,
        expected: &Operation,
    ) -> Result<(&mut D, &ArchiveFileObservation), ArchiveError> {
        if !self.original.same_instance(expected)
            || !self.attempted
            || self.panic.is_some()
            || self.worker_refusal.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        match self.result.as_ref() {
            Some(Ok(observed)) => Ok((self.driver.as_mut(), observed)),
            _ => Err(ArchiveError::Identity),
        }
    }

    /// This predicate alone proves no join or file acceptance. Only the private
    /// returned std lease exposes cleanup, behind its actual original join gate.
    /// No caller can exchange a callback Err/panic/refusal for a custody latch.
    fn checked_cleanup_source(&mut self, expected: &Operation) -> Result<&mut D, ArchiveError> {
        if !self.original.same_instance(expected)
            || !self.custody_accepted
            || !self.attempted
            || self.panic.is_some()
            || self.worker_refusal.is_some()
            || !matches!(self.result.as_ref(), Some(Ok(_)))
        {
            return Err(ArchiveError::Identity);
        }
        Ok(self.driver.as_mut())
    }

    pub(super) fn driver(&self) -> &D {
        &self.driver
    }

    pub(super) fn result(&self) -> Option<&Result<ArchiveFileObservation, ArchiveError>> {
        self.result.as_ref()
    }

    pub(super) fn has_panic(&self) -> bool {
        self.panic.is_some()
    }

    pub(super) fn worker_refusal(&self) -> Option<&ArchiveError> {
        self.worker_refusal.as_ref()
    }

    pub(super) fn first_interruption(&self) -> Option<Interruption> {
        self.first_interruption
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failed_evidence::{ArchiveMeasurement, EvidenceCoverage};
    use std::{
        future::Future,
        io::Write,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    struct OriginalDriver {
        body: [u8; 4],
        calls: usize,
    }
    impl JoinedArchiveSource for OriginalDriver {
        fn check_original_supervision(&mut self, _: &Operation) -> Result<(), ArchiveError> {
            Err(ArchiveError::Identity) // Inert fixture never claims physical joins.
        }
        fn stream<W: Write>(
            &mut self,
            _: &Operation,
            _: EvidenceCoverage,
            _: &mut W,
        ) -> Result<ArchiveMeasurement, ArchiveError> {
            Err(ArchiveError::Identity)
        }
    }
    fn driver() -> Box<OriginalDriver> {
        Box::new(OriginalDriver {
            body: *b"keep",
            calls: 0,
        })
    }
    fn operation() -> Operation {
        crate::failed_evidence::tests::verification_operation()
    }

    #[test]
    fn physical_only_lease_survives_interruption_overflow_cancel_and_refuses_archive_work() {
        let original = operation();
        let foreign = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver()) as usize;
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let shared = root.shared.clone(); // Inert metadata inspection only.
        let charged = original.captured_bytes();
        let deadline = original.allowance().deadline();
        root.interrupt(&original, Interruption::OperatorStop)
            .unwrap();
        root.interrupt(&original, Interruption::Timeout).unwrap();
        assert!(root.borrow_foreground(&original).is_err());
        assert!(root.borrow_physical_cleanup(&foreign).is_err());
        {
            let state = shared.state.try_lock().unwrap();
            assert!(state.work.is_some());
            assert!(root.borrow_physical_cleanup(&original).is_err()); // Never wait on metadata.
        }
        original.overflow();
        {
            let future = async {
                let mut cleanup = root.borrow_physical_cleanup(&original).unwrap();
                {
                    let state = shared.state.try_lock().unwrap();
                    assert!(state.in_service && state.work.is_none() && !state.checkout_started);
                    assert_eq!(state.first_interruption, Some(Interruption::OperatorStop));
                }
                let work = cleanup.checkout.work.as_mut().unwrap();
                assert_eq!(pointer, std::ptr::from_ref(work.driver()) as usize);
                work.driver.calls += 1; // Inert simulation of a pending physical pass.
                std::future::pending::<()>().await;
            };
            fn send(_: &(impl Future<Output = ()> + Send)) {}
            send(&future);
            let mut future = std::pin::pin!(future);
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(future.as_mut().poll(&mut cx).is_pending());
            // Cancel before completion; restore the SAME interrupted Box.
        }
        {
            let state = shared.state.try_lock().unwrap();
            assert!(!state.in_service && !state.checkout_started);
            let work = state.work.as_ref().unwrap();
            assert_eq!(pointer, std::ptr::from_ref(work.driver()) as usize);
            assert_eq!(work.driver().body, *b"keep");
            assert_eq!(work.driver().calls, 1);
            assert!(!work.attempted && !work.custody_accepted && work.result.is_none());
        }
        assert!(root.borrow_foreground(&original).is_err()); // No dispatch reopening.
        let mut checkout = root.checkout(&original).unwrap();
        checkout
            .perform(&original, |_, _| Err(ArchiveError::Filesystem))
            .unwrap();
        drop(checkout); // Synthetic failed callback; no worker/file/join witness.
        assert!(root.borrow_physical_cleanup(&original).is_err()); // Archive already attempted.
        let status = root.status(&original).unwrap();
        assert_eq!(status.first_interruption, Some(Interruption::OperatorStop));
        assert!(status.returned && status.result_retained && !status.in_service);
        assert!(!status.worker_owned && !status.worker_join_retained);
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(original.allowance().deadline(), deadline);

        let original = operation();
        let mut root =
            RetainedArchive::prepare(ArchiveWork::prepare(driver(), &original).ok().unwrap())
                .ok()
                .unwrap();
        root.std_launch_started = true; // Inert pending-launch fault injection, no actual worker.
        assert!(root.borrow_physical_cleanup(&original).is_err());
        assert!(root.shared.state.try_lock().unwrap().work.is_some());
    }

    #[test]
    fn preparation_physical_cancel_restores_all_stages_and_first_refusals() {
        async fn pending_physical(
            cleanup: PreparationCleanup<'_, OriginalDriver>,
            original: &Operation,
            pointer: usize,
        ) {
            fn touch(driver: &mut OriginalDriver, pointer: usize) {
                assert_eq!(std::ptr::from_ref(&*driver) as usize, pointer);
                driver.calls += 1; // Inert pending-cleanup simulation only.
            }
            match cleanup {
                PreparationCleanup::Driver(driver) => {
                    touch(driver, pointer);
                    std::future::pending::<()>().await;
                }
                PreparationCleanup::Root(root) => {
                    let mut physical = root.borrow_physical_cleanup(original).unwrap();
                    touch(
                        physical.checkout.work.as_mut().unwrap().driver.as_mut(),
                        pointer,
                    );
                    std::future::pending::<()>().await;
                }
                PreparationCleanup::Ready(root) => {
                    let mut supervision = root.borrow(original).unwrap();
                    let mut physical = supervision
                        .root_mut()
                        .unwrap()
                        .borrow_physical_cleanup(original)
                        .unwrap();
                    touch(
                        physical.checkout.work.as_mut().unwrap().driver.as_mut(),
                        pointer,
                    );
                    std::future::pending::<()>().await;
                }
            }
        }
        for position in [
            PreparationPosition::Driver,
            PreparationPosition::Work,
            PreparationPosition::Root,
            PreparationPosition::Ready,
        ] {
            let original = operation();
            let foreign = operation();
            let boxed = driver();
            let pointer = std::ptr::from_ref(boxed.as_ref()) as usize;
            let mut owner = match position {
                PreparationPosition::Driver => {
                    PreparationOwner::prepare(Err((ArchiveError::Fit, boxed)), &original)
                }
                PreparationPosition::Work => PreparationOwner {
                    original: original.clone(),
                    slot: Mutex::new(Some(PreparedOwner {
                        stage: PreparationStage::Work(
                            ArchiveWork::prepare(boxed, &original).ok().unwrap(),
                        ),
                        first_refusal: Some(ArchiveError::Fit),
                    })), // Synthetic pre-root refusal, not production stage promotion.
                },
                PreparationPosition::Root => {
                    let mut root = RetainedArchive::prepare(
                        ArchiveWork::prepare(boxed, &original).ok().unwrap(),
                    )
                    .ok()
                    .unwrap();
                    root.interrupt(&original, Interruption::OperatorStop)
                        .unwrap();
                    PreparationOwner {
                        original: original.clone(),
                        slot: Mutex::new(Some(PreparationOwner::prepare_root(root, &original))),
                    }
                }
                PreparationPosition::Ready => {
                    PreparationOwner::prepare(ArchiveWork::prepare(boxed, &original), &original)
                }
            };
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            let expected_position = (position, position != PreparationPosition::Ready);
            assert_eq!(owner.position(&original).unwrap(), expected_position);
            assert!(owner.borrow(&foreign).is_err());
            original.overflow();
            {
                let future = async {
                    let mut preparation = owner.borrow(&original).unwrap();
                    assert!(preparation.slot.is_none());
                    pending_physical(
                        preparation.physical_cleanup(&original).unwrap(),
                        &original,
                        pointer,
                    )
                    .await;
                };
                fn send(_: &(impl Future<Output = ()> + Send)) {}
                send(&future);
                let mut future = std::pin::pin!(future);
                let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
                assert!(future.as_mut().poll(&mut cx).is_pending());
            } // Cancel restores physical, supervisor AND preparation leases.
            assert_eq!(owner.position(&original).unwrap(), expected_position);
            let mut preparation = owner.borrow(&original).unwrap();
            match position {
                PreparationPosition::Driver | PreparationPosition::Work => assert!(matches!(
                    preparation.first_refusal().unwrap(),
                    Some(ArchiveError::Fit)
                )),
                PreparationPosition::Root => assert!(matches!(
                    preparation.first_refusal().unwrap(),
                    Some(ArchiveError::Identity)
                )),
                PreparationPosition::Ready => {
                    assert!(preparation.first_refusal().unwrap().is_none())
                }
            }
            if position != PreparationPosition::Ready {
                assert!(preparation.supervisor().is_err());
            }
            match preparation.physical_cleanup(&original).unwrap() {
                PreparationCleanup::Driver(driver) => {
                    assert_eq!(std::ptr::from_ref(&*driver) as usize, pointer);
                    assert_eq!(driver.calls, 1);
                    assert_eq!(driver.body, *b"keep");
                }
                PreparationCleanup::Root(root) => {
                    let status = root.status(&original).unwrap();
                    assert_eq!(status.first_interruption, Some(Interruption::OperatorStop));
                    assert!(status.returned && !status.in_service && !status.result_retained);
                    assert!(root.borrow_foreground(&original).is_err());
                    let physical = root.borrow_physical_cleanup(&original).unwrap();
                    let driver = physical.checkout.work.as_ref().unwrap().driver();
                    assert_eq!(std::ptr::from_ref(driver) as usize, pointer);
                    assert_eq!(driver.calls, 1);
                }
                PreparationCleanup::Ready(root) => {
                    let mut supervision = root.borrow(&original).unwrap();
                    let physical = supervision
                        .root_mut()
                        .unwrap()
                        .borrow_physical_cleanup(&original)
                        .unwrap();
                    let driver = physical.checkout.work.as_ref().unwrap().driver();
                    assert_eq!(std::ptr::from_ref(driver) as usize, pointer);
                    assert_eq!(driver.calls, 1);
                }
            }
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
            assert!(original.charge(1).is_err()); // No effect admission reopened.
        }
    }

    #[test]
    fn preparation_owner_cancel_restores_every_nested_original_without_retry_or_charge() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<PreparationOwner<OriginalDriver>>();
        let original = operation();
        let foreign = operation();
        let boxed = driver();
        let pointer = std::ptr::from_ref(boxed.as_ref()) as usize;
        let charged = original.captured_bytes();
        let deadline = original.allowance().deadline();
        let mut owner =
            PreparationOwner::prepare(ArchiveWork::prepare(boxed, &original), &original);
        assert_eq!(
            owner.position(&original).unwrap(),
            (PreparationPosition::Ready, false)
        );
        assert!(owner.position(&foreign).is_err() && owner.borrow(&foreign).is_err());
        {
            let busy = owner.slot.try_lock().unwrap();
            assert!(busy.is_some());
            assert!(owner.position(&original).is_err()); // Immediate refusal, no lock wait.
        }
        {
            let future = async {
                let mut preparation = owner.borrow(&original).unwrap();
                assert!(preparation.slot.is_none());
                assert!(preparation.first_refusal().unwrap().is_none());
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let root = supervision.root_mut().unwrap();
                let mut foreground = root.borrow_foreground(&original).unwrap();
                assert_eq!(
                    pointer,
                    std::ptr::from_ref(foreground.driver().unwrap()) as usize
                );
                foreground.driver_mut().unwrap().calls += 1; // Inert fixture only.
                std::future::pending::<()>().await;
            };
            fn send(_: &(impl Future<Output = ()> + Send)) {}
            send(&future);
            let mut future = std::pin::pin!(future);
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(future.as_mut().poll(&mut context).is_pending());
            // Cancel restores the SAME Box, root, supervisor and preparation.
        }
        original.overflow();
        {
            let mut preparation = owner.borrow(&original).unwrap(); // Inspection only.
            let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
            let root = supervision.root_mut().unwrap();
            let status = root.status(&original).unwrap();
            assert!(status.returned && !status.in_service);
            assert!(!status.worker_owned && !status.worker_join_retained);
            assert!(!status.result_retained && !status.panic_retained);
            let foreground = root.borrow_foreground(&original).unwrap();
            assert_eq!(
                pointer,
                std::ptr::from_ref(foreground.driver().unwrap()) as usize
            );
            assert_eq!(foreground.driver().unwrap().body, *b"keep");
            assert_eq!(foreground.driver().unwrap().calls, 1);
            assert!(original.charge(1).is_err()); // Borrow never renews effects.
        }
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(original.allowance().deadline(), deadline);
    }

    #[test]
    fn refused_preparation_keeps_typed_box_and_foreign_work_without_exposing_supervisor() {
        let original = operation();
        let foreign = operation();
        let boxed = driver();
        let pointer = std::ptr::from_ref(boxed.as_ref());
        let charged = original.captured_bytes();
        original.overflow();
        let mut owner =
            PreparationOwner::prepare(ArchiveWork::prepare(boxed, &original), &original);
        assert_eq!(
            owner.position(&original).unwrap(),
            (PreparationPosition::Driver, true)
        );
        {
            let mut lease = owner.borrow(&original).unwrap();
            assert!(matches!(
                lease.first_refusal().unwrap(),
                Some(ArchiveError::Operation(
                    crate::operation::OperationError::CaptureOverflow
                ))
            ));
            assert!(lease.supervisor().is_err());
            match &lease.prepared.as_ref().unwrap().stage {
                PreparationStage::Driver(boxed) => {
                    assert_eq!(pointer, std::ptr::from_ref(boxed.as_ref()));
                    assert_eq!(boxed.body, *b"keep");
                    assert_eq!(boxed.calls, 0);
                }
                _ => panic!("refused box was changed to another stage"),
            }
        }
        assert!(owner.borrow(&foreign).is_err());
        assert_eq!(original.captured_bytes(), charged);
        // A caller supplies SAME textual ID with a different instance. Refuse
        // before root/Arc construction and retain work's own ORIGINAL instance.
        let work = ArchiveWork::prepare(driver(), &foreign).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut owner = PreparationOwner::prepare(Ok(work), &original);
        assert_eq!(
            owner.position(&original).unwrap(),
            (PreparationPosition::Work, true)
        );
        let mut lease = owner.borrow(&original).unwrap();
        assert!(matches!(
            lease.first_refusal().unwrap(),
            Some(ArchiveError::Identity)
        ));
        assert!(lease.supervisor().is_err());
        assert!(lease.physical_cleanup(&original).is_err()); // Never retag foreign work for cleanup.
        match &lease.prepared.as_ref().unwrap().stage {
            PreparationStage::Work(work) => {
                assert!(work.original.same_instance(&foreign));
                assert_eq!(pointer, std::ptr::from_ref(work.driver()));
                assert!(work.result.is_none() && work.panic.is_none() && !work.attempted);
            }
            _ => panic!("foreign work was consumed or retagged"),
        }
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn partial_preparation_refusals_keep_opaque_work_and_root_through_outer_unwind() {
        struct Opaque {
            marker: std::cell::Cell<usize>, // Send, not Sync; never formatted/copied.
            dropped: Arc<AtomicUsize>,
        }
        impl Drop for Opaque {
            fn drop(&mut self) {
                self.dropped.fetch_add(1, Ordering::SeqCst);
            }
        }
        let original = operation();
        let charged = original.captured_bytes();
        let mut work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        work.interrupt(Interruption::OperatorStop);
        let dropped = Arc::new(AtomicUsize::new(0));
        let payload = Box::new(Opaque {
            marker: std::cell::Cell::new(42),
            dropped: dropped.clone(),
        });
        let payload_pointer = std::ptr::from_ref(payload.as_ref());
        work.perform(&original, |driver, _| {
            driver.calls += 1;
            std::panic::resume_unwind(payload); // Synthetic callback, no I/O or worker.
        })
        .unwrap();
        let mut owner = PreparationOwner::prepare(Ok(work), &original);
        assert_eq!(
            owner.position(&original).unwrap(),
            (PreparationPosition::Work, true)
        );
        let unwind = catch_unwind(AssertUnwindSafe(|| {
            let mut lease = owner.borrow(&original).unwrap();
            assert!(lease.supervisor().is_err());
            std::panic::resume_unwind(Box::new(())); // Independent outer cancellation path.
        }));
        assert!(unwind.is_err());
        {
            let mut lease = owner.borrow(&original).unwrap();
            assert!(lease.physical_cleanup(&original).is_err()); // Used work is not pre-archive cleanup.
            assert!(matches!(
                lease.first_refusal().unwrap(),
                Some(ArchiveError::Identity)
            ));
            let PreparationStage::Work(work) = &lease.prepared.as_ref().unwrap().stage else {
                panic!("used work did not survive preparation refusal");
            };
            assert_eq!(pointer, std::ptr::from_ref(work.driver()));
            assert_eq!(work.driver().calls, 1);
            assert_eq!(work.first_interruption, Some(Interruption::OperatorStop));
            let retained = work
                .panic
                .as_ref()
                .unwrap()
                .downcast_ref::<Opaque>()
                .unwrap();
            assert_eq!(payload_pointer, std::ptr::from_ref(retained));
            assert_eq!(retained.marker.get(), 42);
            assert!(work.result.is_none() && !work.custody_accepted);
            assert_eq!(dropped.load(Ordering::SeqCst), 0);
        }
        drop(owner); // Inert fixture teardown only, not production discard authority.
        assert_eq!(dropped.load(Ordering::SeqCst), 1);

        // A root can also refuse the final supervisor install. Preserve its
        // existing first cause/Arc/Box instead of treating that stage as ready.
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let shared = Arc::as_ptr(&root.shared);
        root.interrupt(&original, Interruption::Timeout).unwrap();
        let prepared = PreparationOwner::prepare_root(root, &original);
        let mut owner = PreparationOwner {
            original: original.clone(),
            slot: Mutex::new(Some(prepared)),
        }; // Synthetic final-stage fixture, not a production retry.
        assert_eq!(
            owner.position(&original).unwrap(),
            (PreparationPosition::Root, true)
        );
        let mut lease = owner.borrow(&original).unwrap();
        assert!(lease.supervisor().is_err());
        let PreparationStage::Root(root) = &lease.prepared.as_ref().unwrap().stage else {
            panic!("refused root was discarded or promoted");
        };
        assert_eq!(Arc::as_ptr(&root.shared), shared);
        let status = root.status(&original).unwrap();
        assert_eq!(status.first_interruption, Some(Interruption::Timeout));
        assert!(!status.worker_owned && !status.worker_join_retained);
        let state = root.shared.state.lock().unwrap();
        assert_eq!(
            pointer,
            std::ptr::from_ref(state.work.as_ref().unwrap().driver())
        );
        assert_eq!(original.captured_bytes(), charged);
    }

    #[test]
    fn supervisor_root_cancel_restores_same_root_and_foreground_box_without_new_admission() {
        let original = operation();
        let foreign = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver()) as usize;
        let root = RetainedArchive::prepare(work).ok().unwrap();
        let shared = Arc::as_ptr(&root.shared) as usize;
        let charged = original.captured_bytes();
        let deadline = original.allowance().deadline();
        let mut supervisor = SupervisorRoot::prepare(root, &original).ok().unwrap();
        assert!(supervisor.borrow(&foreign).is_err());
        assert!(supervisor.status(&foreign).is_err());
        {
            let busy = supervisor.slot.try_lock().unwrap();
            assert!(busy.is_some());
            assert!(supervisor.status(&original).is_err()); // No blocking outer lock.
        }
        {
            let future = async {
                let mut lease = supervisor.borrow(&original).unwrap();
                assert!(lease.slot.is_none()); // In service, not replacement-ready.
                let root = lease.root_mut().unwrap();
                assert_eq!(Arc::as_ptr(&root.shared) as usize, shared);
                let mut foreground = root.borrow_foreground(&original).unwrap();
                assert_eq!(
                    pointer,
                    std::ptr::from_ref(foreground.driver().unwrap()) as usize
                );
                foreground.driver_mut().unwrap().calls += 1; // Inert fixture only.
                std::future::pending::<()>().await;
            };
            fn check_send(_: &(impl Future<Output = ()> + Send)) {}
            check_send(&future);
            let mut future = std::pin::pin!(future); // No boxed task/worker.
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(future.as_mut().poll(&mut context).is_pending());
            // Scope cancellation restores inner SAME Box, then outer SAME root.
        }
        let status = supervisor.status(&original).unwrap();
        assert!(!status.in_service && !status.worker_owned && !status.worker_join_retained);
        assert!(!status.result_retained && !status.panic_retained);
        assert!(status.first_interruption.is_none());
        original.overflow();
        {
            let mut lease = supervisor.borrow(&original).unwrap(); // Cleanup/inspection only.
            let root = lease.root_mut().unwrap();
            assert_eq!(Arc::as_ptr(&root.shared) as usize, shared);
            let foreground = root.borrow_foreground(&original).unwrap();
            assert_eq!(
                pointer,
                std::ptr::from_ref(foreground.driver().unwrap()) as usize
            );
            assert_eq!(foreground.driver().unwrap().body, *b"keep");
            assert_eq!(foreground.driver().unwrap().calls, 1);
            assert!(original.check().is_err() && original.charge(1).is_err());
        }
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(original.allowance().deadline(), deadline);
        // No physical joins/file/custody/prompt/actor/healthy witness manufactured.
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn supervisor_unwind_keeps_send_only_payload_and_refused_install_keeps_original_root() {
        struct SendOnlyPanic {
            marker: std::cell::Cell<usize>, // Send, deliberately not Sync.
            dropped: Arc<AtomicUsize>,
        }
        impl Drop for SendOnlyPanic {
            fn drop(&mut self) {
                self.dropped.fetch_add(1, Ordering::SeqCst);
            }
        }
        let original = operation();
        let foreign = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let root = RetainedArchive::prepare(work).ok().unwrap();
        let shared = Arc::as_ptr(&root.shared);
        let charged = original.captured_bytes();
        let (error, root) = match SupervisorRoot::prepare(root, &foreign) {
            Err(refused) => refused,
            Ok(_) => panic!("foreign supervisor root accepted"),
        };
        assert!(matches!(error, ArchiveError::Identity));
        assert_eq!(Arc::as_ptr(&root.shared), shared);
        let mut supervisor = SupervisorRoot::prepare(root, &original).ok().unwrap();
        let dropped = Arc::new(AtomicUsize::new(0));
        let payload = Box::new(SendOnlyPanic {
            marker: std::cell::Cell::new(42),
            dropped: dropped.clone(),
        });
        let payload_pointer = std::ptr::from_ref(payload.as_ref());
        let unwound = catch_unwind(AssertUnwindSafe(|| {
            let mut lease = supervisor.borrow(&original).unwrap();
            let root = lease.root_mut().unwrap();
            {
                let mut checkout = root.checkout(&original).unwrap();
                checkout
                    .work
                    .as_mut()
                    .unwrap()
                    .perform(&original, |driver, _| {
                        driver.calls += 1;
                        std::panic::resume_unwind(payload); // Original opaque MOVE; no hook.
                    })
                    .unwrap();
            }
            std::panic::resume_unwind(Box::new(())); // Independently unwind outer lease.
        }));
        assert!(unwound.is_err());
        let status = supervisor.status(&original).unwrap();
        assert!(status.panic_retained && status.returned);
        assert!(!status.worker_owned && !status.worker_join_retained && !status.result_retained);
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        {
            let mut lease = supervisor.borrow(&original).unwrap();
            let root = lease.root_mut().unwrap();
            assert_eq!(Arc::as_ptr(&root.shared), shared);
            assert!(root.borrow_foreground(&original).is_err());
            assert!(root.read_std_returned(&original).is_err()); // No fake worker/join.
            let state = root.shared.state.lock().unwrap();
            let work = state.work.as_ref().unwrap();
            assert_eq!(pointer, std::ptr::from_ref(work.driver()));
            assert_eq!(work.driver().calls, 1);
            let retained = work
                .panic
                .as_ref()
                .unwrap()
                .downcast_ref::<SendOnlyPanic>()
                .unwrap();
            assert_eq!(payload_pointer, std::ptr::from_ref(retained));
            assert_eq!(retained.marker.get(), 42);
            assert!(work.result.is_none() && work.attempted && !work.custody_accepted);
        }
        // A used root cannot be installed as fresh. Return the SAME original
        // root, including its opaque payload, on the preparation refusal.
        let root = supervisor.slot.get_mut().unwrap().take().unwrap(); // Synthetic teardown only.
        let (error, root) = match SupervisorRoot::prepare(root, &original) {
            Err(refused) => refused,
            Ok(_) => panic!("attempted root installed as fresh"),
        };
        assert!(matches!(error, ArchiveError::Identity));
        assert_eq!(Arc::as_ptr(&root.shared), shared);
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        assert_eq!(original.captured_bytes(), charged);
        drop(root); // Inert test teardown, never production discard permission.
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cleanup_latch_without_successful_custody_cannot_expose_mutable_driver() {
        let original = operation();
        let foreign = operation();
        let mut work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let charged = original.captured_bytes();
        assert!(!work.custody_accepted);
        assert!(work.checked_cleanup_source(&original).is_err());
        // Corrupt only an inert fixture flag: it supplies NO accepted file,
        // worker, join, canonical clearing, healthy ACK or production witness.
        work.custody_accepted = true;
        assert!(work.checked_cleanup_source(&original).is_err());
        work.perform(&original, |_, _| Err(ArchiveError::Filesystem))
            .unwrap();
        assert!(work.checked_cleanup_source(&original).is_err());
        assert!(work.checked_cleanup_source(&foreign).is_err());
        assert!(matches!(work.result(), Some(Err(ArchiveError::Filesystem))));
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().body, *b"keep");
        assert_eq!(work.driver().calls, 0);
        assert_eq!(original.captured_bytes(), charged);
    }

    #[test]
    fn custody_flag_without_actual_std_owner_stays_closed_and_retains_box() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let charged = original.captured_bytes();
        {
            let mut state = root.shared.state.lock().unwrap();
            state.work.as_mut().unwrap().custody_accepted = true;
        }
        assert!(root.read_std_returned(&original).is_err());
        assert!(root.borrow_foreground(&original).is_err());
        let state = root.shared.state.lock().unwrap();
        assert!(!state.in_service && !state.checkout_started);
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().body, *b"keep");
        assert!(work.result().is_none() && !work.attempted);
        assert!(root.std_worker.is_none() && root.std_pending_slot.is_none());
        assert_eq!(original.captured_bytes(), charged);
        // Test teardown of this impossible synthetic state proves no custody.
    }

    #[test]
    fn canceled_foreground_borrow_restores_box_without_starting_archive() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let charged = original.captured_bytes();
        {
            let borrowed = async {
                let mut guard = root.borrow_foreground(&original).unwrap();
                assert_eq!(pointer, std::ptr::from_ref(guard.driver().unwrap()));
                guard.driver_mut().unwrap().calls += 1; // Inert field mutation only.
                std::future::pending::<()>().await;
                assert_eq!(pointer, std::ptr::from_ref(guard.driver().unwrap()));
            };
            crate::failed_evidence::check_retained_publication_workspace::<OriginalDriver>(
                RetainedArchive::<OriginalDriver>::fixed_bytes().unwrap()
                    + std::mem::size_of_val(&borrowed),
            )
            .unwrap();
            let mut borrowed = std::pin::pin!(borrowed); // Stack fixture, no task/heap/worker.
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(borrowed.as_mut().poll(&mut context).is_pending());
            // Scope drop cancels the borrow, synchronously restoring the sole Box.
        }
        {
            let mut guard = root.borrow_foreground(&original).unwrap();
            assert_eq!(pointer, std::ptr::from_ref(guard.driver().unwrap()));
            assert_eq!(guard.driver().unwrap().calls, 1);
            guard.driver_mut().unwrap().calls += 1;
        }
        let state = root.shared.state.lock().unwrap();
        assert!(!state.in_service && !state.checkout_started);
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().body, *b"keep");
        assert_eq!(work.driver().calls, 2);
        assert!(!work.attempted && work.result().is_none() && !work.has_panic());
        assert!(root.std_pending_slot.is_none() && root.std_worker.is_none());
        assert!(!root.std_launch_started && root.std_join(&original).unwrap().is_none());
        assert_eq!(original.captured_bytes(), charged);
        // No original deadline, child/readers/prompt ACK or custody is manufactured.
    }

    #[test]
    fn foreground_cleanup_borrow_after_overflow_is_not_new_admission_or_rebind() {
        let original = operation();
        let foreign = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let charged = original.captured_bytes();
        original.overflow();
        assert!(root.borrow_foreground(&foreign).is_err());
        {
            let guard = root.borrow_foreground(&original).unwrap();
            assert_eq!(pointer, std::ptr::from_ref(guard.driver().unwrap()));
            assert_eq!(guard.driver().unwrap().body, *b"keep");
            assert!(matches!(
                original.check(),
                Err(crate::operation::OperationError::CaptureOverflow)
            ));
            assert!(matches!(
                original.charge(1),
                Err(crate::operation::OperationError::CaptureOverflow)
            ));
        }
        assert_eq!(original.captured_bytes(), charged);
        let checkout = root.checkout(&original).unwrap(); // Inert ownership move, no worker.
        assert!(root.borrow_foreground(&original).is_err());
        drop(checkout);
        assert!(root.borrow_foreground(&original).is_err()); // Archive checkout stays one-shot.
        let state = root.shared.state.lock().unwrap();
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().calls, 0);
        assert!(!work.attempted && work.result().is_none());
        assert!(state.checkout_started && !state.in_service);
        assert_eq!(original.captured_bytes(), charged);
    }

    #[test]
    fn retained_std_claim_refuses_foreground_borrow_before_box_take() {
        use super::super::archive_thread::ThreadCapacity;
        static CAPACITY: ThreadCapacity = ThreadCapacity::isolated_test_counter();
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let slot = CAPACITY.try_claim(&original, &original).unwrap();
        root.retain_std_start_slot(&original, slot).ok().unwrap();
        let charged = original.captured_bytes();
        assert!(root.borrow_foreground(&original).is_err());
        assert!(root.std_pending_slot.is_some());
        let state = root.shared.state.lock().unwrap();
        assert!(!state.checkout_started && !state.in_service);
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().body, *b"keep");
        assert_eq!(work.driver().calls, 0);
        assert_eq!(original.captured_bytes(), charged);
        // Test-only capacity/root teardown supplies no actual join/release ACK.
    }

    struct StackPolicyDriver {
        event: super::super::StreamEvent,
        measured: Option<Result<(ArchiveMeasurement, usize), ArchiveError>>,
    }
    impl JoinedArchiveSource for StackPolicyDriver {
        fn check_original_supervision(&mut self, _: &Operation) -> Result<(), ArchiveError> {
            Err(ArchiveError::Identity) // Synthetic body never claims child/readers joins.
        }
        fn stream<W: Write>(
            &mut self,
            _: &Operation,
            _: EvidenceCoverage,
            _: &mut W,
        ) -> Result<ArchiveMeasurement, ArchiveError> {
            Err(ArchiveError::Identity)
        }
    }
    fn stack_policy_callback(
        driver: &mut StackPolicyDriver,
        original: &Operation,
    ) -> Result<ArchiveFileObservation, ArchiveError> {
        // Admit overlapping test-only output counter/measurement headers inside
        // the ORIGINAL fixed workspace before invoking the borrowed encoder.
        crate::failed_evidence::check_retained_publication_workspace::<StackPolicyDriver>(
            RetainedArchive::<StackPolicyDriver>::fixed_bytes()?
                .checked_add(std::mem::size_of::<crate::failed_evidence::ArchiveMeter>())
                .and_then(|bytes| bytes.checked_add(std::mem::size_of::<usize>() * 2))
                .and_then(|bytes| bytes.checked_add(std::mem::size_of::<ArchiveMeasurement>()))
                .ok_or(ArchiveError::Fit)?,
        )?;
        driver.measured = Some(super::super::archive_stdout::measure_stack_fixture(
            original,
            &driver.event,
        ));
        // Even a successful measurement is NOT a file/custody observation.
        Err(ArchiveError::Identity)
    }
    fn stack_leaf_mut(event: &mut super::super::StreamEvent) -> &mut String {
        let super::super::StreamEvent::Entry(cna_protocol::TranscriptEntry::ToolCall {
            args, ..
        }) = event
        else {
            panic!("stack fixture entry changed")
        };
        let mut value = args;
        for _ in 0..crate::failed_evidence::MAX_ARCHIVE_JSON_CONTAINERS {
            value = &mut value.as_array_mut().unwrap()[0];
        }
        value.as_str().expect("stack fixture leaf changed");
        let serde_json::Value::String(leaf) = value else {
            unreachable!()
        };
        leaf
    }

    fn admit_stack_leaf_growth(
        original: &Operation,
        entry_bytes: usize,
        growth: usize,
    ) -> Result<(), crate::operation::OperationError> {
        original.check()?;
        entry_bytes
            .checked_add(growth)
            .filter(|bytes| *bytes <= crate::operation::FRAME_BYTES)
            .ok_or(crate::operation::OperationError::CaptureOverflow)?;
        original.charge(growth)
    }

    #[test]
    #[ignore = "requires separately admitted normal-debug exact-stack validation after D folder GO"]
    fn archive_policy_exact_stack_largest_admitted_depth() {
        use crate::failed_evidence::{CampaignArchive, MAX_ARCHIVE_JSON_CONTAINERS};
        use crate::operation::{Allowance, FRAME_BYTES, OPERATION_BYTES};
        use std::time::Duration;
        if !cfg!(debug_assertions) {
            panic!("normal debug test profile required")
        }
        let path = std::env::temp_dir().join("synthetic-stack-control-no-io");
        let original = CampaignArchive::new(
            &path.join("campaigns/11111111-1111-4111-8111-111111111111.sqlite"),
            &path.join("sandbox"),
            &path.join("run"),
            &[path.join("isolation")],
        )
        .unwrap()
        .operation(
            "axis.commander".parse().unwrap(),
            7,
            "22222222-2222-4222-8222-222222222222".into(),
            Allowance::required(Some(tokio::time::Instant::now() + Duration::from_secs(60)))
                .unwrap(),
        )
        .unwrap();
        // The plan already reserved the FULL policy stack + residual overhead.
        // Charge sole canonical Box body and every one-element array backing
        // BEFORE allocating; no body clone/output buffer/second operation.
        original.check().unwrap();
        original
            .charge(
                std::mem::size_of::<StackPolicyDriver>()
                    + MAX_ARCHIVE_JSON_CONTAINERS * std::mem::size_of::<serde_json::Value>(),
            )
            .unwrap();
        let mut args = serde_json::Value::String(String::new());
        for _ in 0..MAX_ARCHIVE_JSON_CONTAINERS {
            let mut array = Vec::new();
            array.try_reserve_exact(1).unwrap();
            assert_eq!(array.capacity(), 1);
            array.push(args);
            args = serde_json::Value::Array(array);
        }
        let mut driver = Box::new(StackPolicyDriver {
            event: super::super::StreamEvent::Entry(cna_protocol::TranscriptEntry::ToolCall {
                call_id: String::new(),
                tool: String::new(),
                args,
            }),
            measured: None,
        });
        let empty_entry = super::super::event_bytes(&driver.event).unwrap();
        let (_, empty_output) =
            super::super::archive_stdout::measure_stack_fixture(&original, &driver.event).unwrap();
        let remaining = OPERATION_BYTES - original.captured_bytes();
        // Maximum ASCII leaf for THIS original-metered 128-level fixture.
        // FRAME_BYTES is a conservative serialized-entry ingress guard, not
        // proof about CLI frame wrappers. JSONL has no added product size cap.
        let input_room = FRAME_BYTES.checked_sub(empty_entry).unwrap();
        let leaf_bytes = remaining.min(input_room);
        assert!(leaf_bytes > 0);
        admit_stack_leaf_growth(&original, empty_entry, leaf_bytes).unwrap();
        let leaf = stack_leaf_mut(&mut driver.event);
        leaf.try_reserve_exact(leaf_bytes).unwrap();
        assert_eq!(leaf.capacity(), leaf_bytes);
        for _ in 0..leaf_bytes {
            leaf.push('x');
        }
        let leaf_pointer = leaf.as_ptr() as usize;
        let driver_pointer = std::ptr::from_ref(driver.as_ref()) as usize;
        assert_eq!(
            super::super::event_bytes(&driver.event).unwrap(),
            empty_entry + leaf_bytes
        );
        let charged = original.captured_bytes();
        let work = ArchiveWork::prepare(driver, &original).ok().unwrap();
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let launched = root
            .start_std_worker(&original, stack_policy_callback)
            .unwrap();
        // Keep any actually installed handle until real completion, even if
        // registration refused. The admitted runner needs an outer wall guard;
        // an aborted runner is UNKNOWN, never a completed or passed control.
        if root.std_worker.is_some() {
            while !root.poll_std_join(&original).unwrap() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        assert!(launched);
        let returned = root.read_std_returned(&original).unwrap();
        assert!(returned.worker_join().is_ok());
        assert!(returned.worker_associated());
        assert!(returned.registration_error().is_none() && returned.launch_error().is_none());
        let work = returned.work().unwrap();
        assert!(work.attempted && !work.has_panic() && work.worker_refusal().is_none());
        assert!(matches!(work.result(), Some(Err(ArchiveError::Identity))));
        assert_eq!(driver_pointer, std::ptr::from_ref(work.driver()) as usize);
        let Some(Ok((_, length))) = &work.driver().measured else {
            panic!("exact-stack serialization did not return a measurement")
        };
        assert_eq!(*length, empty_output + leaf_bytes);
        let super::super::StreamEvent::Entry(cna_protocol::TranscriptEntry::ToolCall {
            args, ..
        }) = &work.driver().event
        else {
            unreachable!()
        };
        let mut value = args;
        for _ in 0..MAX_ARCHIVE_JSON_CONTAINERS {
            value = &value.as_array().unwrap()[0];
        }
        let leaf = value.as_str().unwrap();
        assert_eq!(leaf.as_ptr() as usize, leaf_pointer);
        assert_eq!(leaf.len(), leaf_bytes);
        assert!(leaf.bytes().all(|byte| byte == b'x'));
        assert_eq!(original.captured_bytes(), charged);
        // One more input byte is refused BEFORE any allocation/mutation.
        assert!(matches!(
            admit_stack_leaf_growth(&original, empty_entry + leaf_bytes, 1),
            Err(crate::operation::OperationError::CaptureOverflow)
        ));
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(leaf.as_ptr() as usize, leaf_pointer);
        assert_eq!(leaf.len(), leaf_bytes);
        // Test teardown after ACTUAL std join is not production discard/health.
    }

    #[test]
    fn actual_driver_canonical_owners_meet_std_worker_transfer_bounds() {
        fn requires_send_static<T: Send + 'static>() {}
        fn requires_send_sync<T: Send + Sync>() {}
        fn supervisor_cleanup_future_send(
            supervisor: &mut SupervisorRoot<super::super::claude::ClaudeDriver>,
            original: &Operation,
        ) {
            fn requires_send(_: impl Send) {}
            requires_send(async move {
                let mut lease = supervisor.borrow(original).map_err(CleanupError::Archive)?;
                lease
                    .root_mut()
                    .map_err(CleanupError::Archive)?
                    .stop_after_custody(original)
                    .await
            });
        }
        fn cleanup_future_send(
            root: &mut RetainedArchive<super::super::claude::ClaudeDriver>,
            original: &Operation,
        ) {
            fn requires_send(_: impl Send) {}
            requires_send(root.stop_after_custody(original));
        }

        // Merely name this type-checking function; never construct or poll a
        // cleanup future/driver here. The future borrows the exclusive root,
        // not a shared reference to its opaque Send-only panic payload.
        let _send_check = cleanup_future_send;
        let _supervisor_send_check = supervisor_cleanup_future_send;

        // Type-only control for the ACTUAL source, not a fixture replacement.
        // Builder's closure must own its guard; a borrowed driver/root cannot
        // cross a thread lifetime. No driver, task, thread or path is created.
        requires_send_static::<super::super::claude::ClaudeDriver>();
        requires_send_static::<ArchiveWork<super::super::claude::ClaudeDriver>>();
        requires_send_static::<ArchiveCheckout<super::super::claude::ClaudeDriver>>();
        requires_send_static::<CustodyCleanupDriver<'static, super::super::claude::ClaudeDriver>>();
        requires_send_static::<RetainedArchive<super::super::claude::ClaudeDriver>>();
        requires_send_sync::<SupervisorRoot<super::super::claude::ClaudeDriver>>();
        requires_send_static::<SupervisorLease<'static, super::super::claude::ClaudeDriver>>();
        requires_send_static::<super::super::archive_thread::ThreadRegistration>();
        requires_send_static::<super::super::archive_thread::ThreadSlot>();
        requires_send_static::<super::super::archive_thread::StdArchiveWorker>();
        requires_send_static::<super::super::archive_thread::RefusedStdStart>();
        requires_send_static::<Operation>();
        requires_send_static::<ArchiveFileObservation>();
        // This proves neither std launch/library/stack fit nor physical joins,
        // registration, restoration, custody, health or execution permission.
        // ReturnedStdArchive remains a borrow of the ORIGINAL root/result;
        // it is deliberately not asserted Send or converted into a new owner.
    }

    // SOURCE ONLY/UNRUN. These private fixture channels hold the real worker
    // AFTER canonical Box restoration, to distinguish return from actual join.
    // They are not an archive output channel or a production completion route.
    struct StdLifecycleDriver {
        body: [u8; 4],
        calls: usize,
        restored: Option<std::sync::mpsc::SyncSender<()>>,
        release: Option<std::sync::mpsc::Receiver<()>>,
        callback_panic: Option<Box<OpaquePanic>>,
        thread_panic: Option<Box<OpaquePanic>>,
    }

    impl JoinedArchiveSource for StdLifecycleDriver {
        fn check_original_supervision(&mut self, _: &Operation) -> Result<(), ArchiveError> {
            Err(ArchiveError::Identity) // No physical child/reader or file witness.
        }
        fn stream<W: Write>(
            &mut self,
            _: &Operation,
            _: EvidenceCoverage,
            _: &mut W,
        ) -> Result<ArchiveMeasurement, ArchiveError> {
            Err(ArchiveError::Identity)
        }
    }

    fn std_lifecycle_callback(
        driver: &mut StdLifecycleDriver,
        _: &Operation,
    ) -> Result<ArchiveFileObservation, ArchiveError> {
        driver.calls += 1;
        if let Some(payload) = driver.callback_panic.take() {
            std::panic::resume_unwind(payload); // Opaque MOVE, no panic hook/formatting.
        }
        Err(ArchiveError::Filesystem) // Actual fixture callback Err, never a fake file.
    }

    fn held_restoration_spawn(
        builder: std::thread::Builder,
        mut checkout: ArchiveCheckout<StdLifecycleDriver>,
        original: Operation,
        callback: ArchiveCallback<StdLifecycleDriver>,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        let fixture = checkout.work.as_mut().unwrap().driver.as_mut();
        let restored = fixture.restored.take().unwrap();
        let release = fixture.release.take().unwrap();
        let thread_panic = fixture.thread_panic.take();
        let deadline = original.allowance().deadline();
        // Use the actual producer's named Builder and exact policy stack. No
        // thread::spawn, helper thread, queue or std worker join is used here.
        builder.spawn(move || {
            run_archive_worker(checkout, original, callback);
            // run_archive_worker dropped its original restoration guard.
            let _ = restored.try_send(());
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let _ = release.recv_timeout(remaining); // Worker/test fixture ONLY.
            if let Some(payload) = thread_panic {
                std::panic::resume_unwind(payload); // Actual thread join Err, not callback panic.
            }
        })
    }

    fn closed_registration_spawn(
        builder: std::thread::Builder,
        checkout: ArchiveCheckout<StdLifecycleDriver>,
        original: Operation,
        callback: ArchiveCallback<StdLifecycleDriver>,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        // The producer must retain the actual handle even when synchronous
        // registration subsequently refuses. No callback may then be inferred.
        checkout.shared.std_registration.close(&original).unwrap();
        held_restoration_spawn(builder, checkout, original, callback)
    }

    fn std_lifecycle_operation() -> Operation {
        let path = std::env::temp_dir().join("synthetic-std-lifecycle-no-io");
        crate::failed_evidence::tests::context(&path)
            .operation(
                "axis.commander".parse().unwrap(),
                7,
                "22222222-2222-4222-8222-222222222222".into(),
                crate::operation::Allowance::required(Some(
                    tokio::time::Instant::now() + std::time::Duration::from_secs(60),
                ))
                .unwrap(),
            )
            .unwrap()
    }

    #[test]
    #[cfg(panic = "unwind")]
    #[ignore = "requires separate genuine std lifecycle admission after D folder GO"]
    fn real_std_producer_keeps_restoration_callback_panic_join_and_registration_distinct() {
        use super::super::archive_thread::ThreadCapacity;
        use std::{sync::mpsc::sync_channel, time::Duration};
        static ERROR_CAP: ThreadCapacity = ThreadCapacity::isolated_test_counter();
        static CALLBACK_PANIC_CAP: ThreadCapacity = ThreadCapacity::isolated_test_counter();
        static THREAD_PANIC_CAP: ThreadCapacity = ThreadCapacity::isolated_test_counter();
        static REFUSED_REGISTRATION_CAP: ThreadCapacity = ThreadCapacity::isolated_test_counter();
        let capacities = [
            &ERROR_CAP,
            &CALLBACK_PANIC_CAP,
            &THREAD_PANIC_CAP,
            &REFUSED_REGISTRATION_CAP,
        ];
        for (case, capacity) in capacities.into_iter().enumerate() {
            let original = std_lifecycle_operation();
            let foreign = std_lifecycle_operation();
            let (restored, receipt) = sync_channel(1);
            let (release, held) = sync_channel(1);
            let dropped = Arc::new(AtomicUsize::new(0));
            let payload = Box::new(OpaquePanic {
                dropped: dropped.clone(),
            });
            let payload_pointer = std::ptr::from_ref(payload.as_ref());
            let mut fixture = Box::new(StdLifecycleDriver {
                body: *b"keep",
                calls: 0,
                restored: Some(restored),
                release: Some(held),
                callback_panic: None,
                thread_panic: None,
            });
            match case {
                1 => fixture.callback_panic = Some(payload),
                2 => fixture.thread_panic = Some(payload),
                _ => drop(payload), // No panic payload used in Err/refused cases.
            }
            let pointer = std::ptr::from_ref(fixture.as_ref());
            let work = ArchiveWork::prepare(fixture, &original).ok().unwrap();
            let mut root = RetainedArchive::prepare(work).ok().unwrap();
            let charged = original.captured_bytes();
            let spawn: ArchiveSpawn<StdLifecycleDriver> = if case == 3 {
                closed_registration_spawn
            } else {
                held_restoration_spawn
            };
            let started =
                root.start_std_worker_with(&original, std_lifecycle_callback, capacity, spawn);
            let actual_worker_id = root.std_worker.as_ref().map(|worker| worker.id());
            // A failed assertion must not abandon a started fixture worker.
            // Preserve it through actual completion first; outer admitted runner
            // still owns wall/no-survivor closure if this test is interrupted.
            let observations = catch_unwind(AssertUnwindSafe(|| {
                assert_eq!(started.unwrap(), case != 3);
                assert!(root.std_worker.is_some());
                let remaining = original
                    .allowance()
                    .deadline()
                    .saturating_duration_since(tokio::time::Instant::now());
                receipt.recv_timeout(remaining).unwrap();
                let status = root.status(&original).unwrap();
                assert!(status.returned && !status.in_service && status.worker_owned);
                assert!(!status.worker_join_retained);
                assert_eq!(status.worker_associated, case != 3);
                // The operator's current thread cannot impersonate the retained
                // worker merely because registration/restoration was observed.
                assert!(
                    root.shared
                        .std_registration
                        .validate_current(&original)
                        .is_err()
                );
                assert!(!root.poll_std_join(&original).unwrap()); // Held real thread, no blocking join.
                assert!(root.std_join(&original).unwrap().is_none());
                assert!(root.read_std_returned(&original).is_err()); // Box return is insufficient.
                assert!(matches!(
                    capacity.try_claim(&foreign, &foreign),
                    Err(ArchiveError::Fit)
                ));
                assert!(root.poll_std_join(&foreign).is_err());
                {
                    let state = root.shared.state.lock().unwrap();
                    let kept = state.work.as_ref().unwrap();
                    assert_eq!(pointer, std::ptr::from_ref(kept.driver()));
                    assert_eq!(kept.driver().body, *b"keep");
                    assert_eq!(kept.driver().calls, usize::from(case != 3));
                    assert_eq!(kept.attempted, case != 3);
                    assert!(!kept.custody_accepted);
                }
                original.overflow(); // Existing reserve; no post-return credit/refund.
                root.interrupt(&original, Interruption::OperatorStop)
                    .unwrap();
                root.interrupt(&original, Interruption::Timeout).unwrap();
                assert_eq!(
                    root.status(&original).unwrap().first_interruption,
                    Some(Interruption::OperatorStop)
                );
                assert_eq!(original.captured_bytes(), charged);
            }));
            let _ = release.try_send(()); // Always unblock the SAME fixture worker.
            if root.std_worker.is_some() {
                while !root.poll_std_join(&original).unwrap() {
                    // Foreground test harness only; production poll_std_join never waits.
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            observations.unwrap(); // Fail only AFTER any installed actual handle joined.
            assert!(root.poll_std_join(&original).unwrap()); // Same retained join, not a second join.
            assert!(root.read_std_returned(&foreign).is_err());
            {
                let mut returned = root.read_std_returned(&original).unwrap();
                assert_eq!(Some(returned.worker_id()), actual_worker_id);
                assert_eq!(returned.worker_join().is_err(), case == 2);
                assert_eq!(returned.worker_associated(), case != 3);
                assert_eq!(returned.registration_error().is_some(), case == 3);
                assert!(returned.launch_error().is_none());
                assert_eq!(
                    returned.first_interruption(),
                    Some(Interruption::OperatorStop)
                );
                if case == 2 {
                    let payload = returned
                        .worker_join()
                        .as_ref()
                        .err()
                        .unwrap()
                        .downcast_ref::<OpaquePanic>()
                        .unwrap();
                    assert_eq!(payload_pointer, std::ptr::from_ref(payload));
                    assert_eq!(dropped.load(Ordering::SeqCst), 0);
                }
                let kept = returned.checkout.work.as_mut().unwrap();
                assert_eq!(pointer, std::ptr::from_ref(kept.driver()));
                assert_eq!(kept.driver().body, *b"keep");
                assert_eq!(kept.driver().calls, usize::from(case != 3));
                assert_eq!(kept.has_panic(), case == 1);
                if case == 1 {
                    let payload = kept
                        .panic
                        .as_ref()
                        .unwrap()
                        .downcast_ref::<OpaquePanic>()
                        .unwrap();
                    assert_eq!(payload_pointer, std::ptr::from_ref(payload));
                    assert_eq!(dropped.load(Ordering::SeqCst), 0);
                    assert!(kept.result().is_none()); // Thread joined Ok, callback panicked.
                } else if case == 3 {
                    assert!(!kept.attempted && kept.result().is_none());
                    assert!(matches!(
                        kept.worker_refusal(),
                        Some(ArchiveError::Identity)
                    ));
                } else {
                    assert!(matches!(kept.result(), Some(Err(ArchiveError::Filesystem))));
                }
                assert!(kept.checked_custody_source(&original).is_err());
                assert!(kept.checked_cleanup_source(&original).is_err());
                assert!(kept.perform(&original, std_lifecycle_callback).is_err());
            }
            root.interrupt(&original, Interruption::Timeout).unwrap();
            assert_eq!(
                root.status(&original).unwrap().first_interruption,
                Some(Interruption::OperatorStop)
            );
            assert!(root.borrow_foreground(&original).is_err());
            assert!(
                root.start_std_worker_with(
                    &original,
                    std_lifecycle_callback,
                    capacity,
                    held_restoration_spawn
                )
                .is_err()
            );
            assert_eq!(original.captured_bytes(), charged);
            // Actual join alone frees ONLY this isolated thread counter. It
            // grants no Operation credit, custody or healthy/refund witness.
            let next_slot = capacity.try_claim(&foreign, &foreign).unwrap();
            drop(next_slot); // No next thread; Drop cannot free an unjoined slot.
            assert!(matches!(
                capacity.try_claim(&foreign, &foreign),
                Err(ArchiveError::Fit)
            ));
            if matches!(case, 1 | 2) {
                assert_eq!(dropped.load(Ordering::SeqCst), 0);
                drop(root); // Synthetic joined test teardown, not production discard permission.
                assert_eq!(dropped.load(Ordering::SeqCst), 1);
            }
        }
    }

    #[derive(Debug)]
    struct OpaqueLaunchError([u8; 4]);

    impl std::fmt::Display for OpaqueLaunchError {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("opaque launch error must never be formatted by retention");
        }
    }
    impl std::error::Error for OpaqueLaunchError {}

    fn inert_callback(
        driver: &mut OriginalDriver,
        _: &Operation,
    ) -> Result<ArchiveFileObservation, ArchiveError> {
        driver.calls += 1;
        Err(ArchiveError::Identity) // No fixture physical joins or file I/O.
    }

    fn refused_spawn(
        _: std::thread::Builder,
        checkout: ArchiveCheckout<OriginalDriver>,
        original: Operation,
        _: ArchiveCallback<OriginalDriver>,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        assert!(
            checkout
                .work
                .as_ref()
                .unwrap()
                .original
                .same_instance(&original)
        );
        {
            let state = checkout.shared.state.lock().unwrap();
            assert!(state.checkout_started && state.in_service && state.work.is_none());
        }
        drop(checkout); // Simulate Builder Err disposing its closure, no thread.
        Err(std::io::Error::other(OpaqueLaunchError(*b"kept")))
    }

    fn poisoned_refused_spawn(
        builder: std::thread::Builder,
        checkout: ArchiveCheckout<OriginalDriver>,
        original: Operation,
        callback: ArchiveCallback<OriginalDriver>,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        let shared = checkout.shared.clone();
        let _poison = catch_unwind(AssertUnwindSafe(|| {
            let _held = shared.state.lock().unwrap();
            panic!("synthetic metadata poison, not a worker panic");
        }));
        drop(builder);
        drop(original);
        let _ = callback;
        drop(checkout); // Recovery restores the original Box despite poison.
        Err(std::io::Error::other(OpaqueLaunchError(*b"kept")))
    }

    #[test]
    fn concrete_producer_refusal_retains_box_error_cap_and_refuses_retry() {
        use super::super::archive_thread::ThreadCapacity;
        static CAPACITY: ThreadCapacity = ThreadCapacity::isolated_test_counter();
        let original = operation();
        let foreign = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let before = original.captured_bytes();
        assert!(
            root.start_std_worker_with(&foreign, inert_callback, &CAPACITY, refused_spawn)
                .is_err()
        );
        assert!(root.std_pending_slot.is_none() && !root.std_launch_started);
        original.overflow(); // Uses original reserve, never a late admission.
        assert!(
            !root
                .start_std_worker_with(&original, inert_callback, &CAPACITY, refused_spawn)
                .unwrap()
        );
        assert!(
            root.std_start_error(&original)
                .unwrap()
                .unwrap()
                .get_ref()
                .unwrap()
                .is::<OpaqueLaunchError>()
        );
        assert!(root.std_launch_error(&original).unwrap().is_none());
        let state = root.shared.state.lock().unwrap();
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().body, *b"keep");
        assert_eq!(work.driver().calls, 0);
        assert!(!work.attempted && work.result.is_none() && work.worker_refusal().is_none());
        assert!(!state.in_service && state.checkout_started);
        drop(state);
        assert!(
            root.start_std_worker_with(&original, inert_callback, &CAPACITY, refused_spawn)
                .is_err()
        );
        assert!(root.checkout(&original).is_err());
        assert!(root.read_std_returned(&original).is_err());
        assert!(root.std_join(&original).unwrap().is_none());
        assert!(matches!(
            CAPACITY.try_claim(&foreign, &foreign),
            Err(ArchiveError::Fit)
        ));
        let other_work = ArchiveWork::prepare(driver(), &foreign).ok().unwrap();
        let other_pointer = std::ptr::from_ref(other_work.driver());
        let mut other_root = RetainedArchive::prepare(other_work).ok().unwrap();
        let other_before = foreign.captured_bytes();
        assert!(matches!(
            other_root.start_std_worker_with(&foreign, inert_callback, &CAPACITY, refused_spawn),
            Err(ArchiveError::Fit)
        ));
        assert!(!other_root.std_launch_started && other_root.std_pending_slot.is_none());
        let other_state = other_root.shared.state.lock().unwrap();
        assert!(!other_state.checkout_started && !other_state.in_service);
        assert_eq!(
            other_pointer,
            std::ptr::from_ref(other_state.work.as_ref().unwrap().driver())
        );
        assert_eq!(foreign.captured_bytes(), other_before);
        assert_eq!(original.captured_bytes(), before);
        // No actual Builder failure, launch/join/custody/healthy ACK is asserted.
    }

    #[test]
    fn concrete_producer_poison_keeps_original_box_and_opaque_refused_start() {
        use super::super::archive_thread::ThreadCapacity;
        static CAPACITY: ThreadCapacity = ThreadCapacity::isolated_test_counter();
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let before = original.captured_bytes();
        assert!(
            !root
                .start_std_worker_with(&original, inert_callback, &CAPACITY, poisoned_refused_spawn)
                .unwrap()
        );
        assert!(matches!(
            root.std_launch_error(&original).unwrap(),
            Some(ArchiveError::Identity)
        ));
        let kept = root
            .std_start_error(&original)
            .unwrap()
            .unwrap()
            .get_ref()
            .unwrap()
            .downcast_ref::<OpaqueLaunchError>()
            .unwrap();
        assert_eq!(kept.0, *b"kept");
        let state = root.shared.state.lock().err().unwrap().into_inner();
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().calls, 0);
        assert!(!state.in_service && work.result.is_none() && !work.attempted);
        drop(state);
        assert!(
            root.start_std_worker_with(&original, inert_callback, &CAPACITY, refused_spawn)
                .is_err()
        );
        assert!(root.read_std_returned(&original).is_err());
        assert_eq!(original.captured_bytes(), before);
        // Metadata poison is diagnostic only. No thread or file was created.
    }

    #[test]
    fn worker_registration_refusal_returns_box_without_callback_or_custody() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let root = RetainedArchive::prepare(work).ok().unwrap();
        let before = original.captured_bytes();
        let checkout = root.checkout(&original).unwrap();
        root.shared.std_registration.close(&original).unwrap();
        run_archive_worker(checkout, original.clone(), inert_callback);
        let mut state = root.shared.state.lock().unwrap();
        let work = state.work.as_mut().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().calls, 0);
        assert!(matches!(
            work.worker_refusal(),
            Some(ArchiveError::Identity)
        ));
        assert!(!work.attempted && work.result.is_none() && !work.has_panic());
        assert!(work.perform(&original, inert_callback).is_err());
        assert!(work.checked_custody_source(&original).is_err());
        assert!(!state.in_service);
        drop(state);
        assert!(root.std_join(&original).unwrap().is_none());
        assert_eq!(original.captured_bytes(), before);
        // Closed bridge refuses synchronously; this is NOT an actual worker join.
    }

    #[test]
    fn refused_std_start_moves_same_error_and_slot_without_join_retry_or_release() {
        use super::super::archive_thread::{RefusedStdStart, ThreadCapacity};
        static CAPACITY: ThreadCapacity = ThreadCapacity::isolated_test_counter();
        let original = operation();
        let foreign = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let before = original.captured_bytes();
        let slot = CAPACITY.try_claim(&original, &original).unwrap();
        // Synthetic private error ONLY, not an actual Builder failure/fit proof.
        let error = std::io::Error::other(OpaqueLaunchError(*b"kept"));
        let payload = std::ptr::from_ref(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<OpaqueLaunchError>()
                .unwrap(),
        );
        let (_, slot, error) = RefusedStdStart::retain(slot, &foreign, error)
            .err()
            .unwrap();
        let (_, slot) = root.retain_std_start_slot(&foreign, slot).err().unwrap();
        root.retain_std_start_slot(&original, slot).ok().unwrap();
        assert!(root.status(&original).unwrap().std_start_pending);
        assert!(root.checkout(&original).is_err());
        assert!(root.begin_std_launch(&foreign).is_err());
        let (slot, checkout) = root.begin_std_launch(&original).unwrap();
        assert!(root.std_pending_slot.is_none() && root.std_launch_started);
        let failure = RefusedStdStart::retain(slot, &original, error)
            .ok()
            .unwrap();
        let (_, failure) = root
            .retain_std_start_failure(&foreign, failure)
            .err()
            .unwrap();
        assert!(!root.status(&original).unwrap().std_start_refused);
        let (_, failure) = root
            .retain_std_start_failure(&original, failure)
            .err()
            .unwrap();
        root.interrupt(&original, Interruption::Timeout).unwrap();
        drop(checkout); // SAME guard restores Box; not an actual spawn/join witness.
        original.overflow();
        root.retain_std_start_failure(&original, failure)
            .ok()
            .unwrap();
        let error = root.std_start_error(&original).unwrap().unwrap();
        let kept = error
            .get_ref()
            .unwrap()
            .downcast_ref::<OpaqueLaunchError>()
            .unwrap();
        assert_eq!(payload, std::ptr::from_ref(kept));
        assert_eq!(kept.0, *b"kept");
        assert!(root.std_start_error(&foreign).is_err());
        let status = root.status(&original).unwrap();
        assert!(status.std_start_refused && status.returned && !status.in_service);
        assert!(!status.std_start_pending);
        assert!(!status.worker_owned && !status.worker_join_retained && !status.worker_associated);
        assert!(!status.result_retained && !status.panic_retained);
        assert_eq!(status.first_interruption, Some(Interruption::Timeout));
        let state = root.shared.state.lock().unwrap();
        let kept = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(kept.driver()));
        assert_eq!(kept.driver().body, *b"keep");
        assert_eq!(kept.driver().calls, 0);
        assert!(!kept.attempted);
        drop(state);
        assert!(root.checkout(&original).is_err());
        assert!(root.read_std_returned(&original).is_err());
        assert!(root.poll_std_join(&original).is_err());
        assert!(root.std_join(&original).unwrap().is_none());
        assert_eq!(original.captured_bytes(), before);
        drop(root); // Process bookkeeping drop is NEVER a free-capacity receipt.
        assert!(matches!(
            CAPACITY.try_claim(&foreign, &foreign),
            Err(ArchiveError::Fit)
        ));
        assert_eq!(original.captured_bytes(), before);
    }

    #[test]
    fn callback_result_cannot_be_reclassified_as_refused_std_start() {
        use super::super::archive_thread::{RefusedStdStart, ThreadCapacity};
        static CAPACITY: ThreadCapacity = ThreadCapacity::isolated_test_counter();
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let slot = CAPACITY.try_claim(&original, &original).unwrap();
        root.retain_std_start_slot(&original, slot).ok().unwrap();
        let (slot, mut checkout) = root.begin_std_launch(&original).unwrap();
        let failure =
            RefusedStdStart::retain(slot, &original, std::io::Error::from_raw_os_error(5))
                .ok()
                .unwrap();
        checkout
            .perform(&original, |_, _| Err(ArchiveError::Filesystem))
            .unwrap();
        drop(checkout);
        let (_, failure) = root
            .retain_std_start_failure(&original, failure)
            .err()
            .unwrap();
        assert_eq!(failure.error().raw_os_error(), Some(5));
        assert!(!root.status(&original).unwrap().std_start_refused);
        let state = root.shared.state.lock().unwrap();
        let kept = state.work.as_ref().unwrap();
        assert!(kept.attempted && matches!(kept.result(), Some(Err(ArchiveError::Filesystem))));
        assert!(root.std_start_error(&original).unwrap().is_none());
        // Refusal preserves BOTH caller-owned failure and canonical callback error.
        // No actual thread, slot release, replacement or healthy ACK is invented.
    }

    #[test]
    fn std_start_failure_without_prelaunch_handoff_returns_original_owner() {
        use super::super::archive_thread::{RefusedStdStart, ThreadCapacity};
        static CAPACITY: ThreadCapacity = ThreadCapacity::isolated_test_counter();
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let before = original.captured_bytes();
        let slot = CAPACITY.try_claim(&original, &original).unwrap();
        let failure =
            RefusedStdStart::retain(slot, &original, std::io::Error::from_raw_os_error(5))
                .ok()
                .unwrap();
        // Preserve the prior pre-checkout failure-refusal assertion: a candidate
        // error cannot manufacture this root's original prelaunch handoff.
        let (_, failure) = root
            .retain_std_start_failure(&original, failure)
            .err()
            .unwrap();
        assert_eq!(failure.error().raw_os_error(), Some(5));
        assert!(failure.check_original(&original).is_ok());
        let status = root.status(&original).unwrap();
        assert!(
            !status.std_start_refused
                && !status.std_start_pending
                && !status.in_service
                && !status.returned
        );
        assert!(!status.worker_owned && !status.worker_join_retained);
        let state = root.shared.state.lock().unwrap();
        assert_eq!(
            pointer,
            std::ptr::from_ref(state.work.as_ref().unwrap().driver())
        );
        assert!(!state.checkout_started);
        drop(state);
        assert_eq!(original.captured_bytes(), before);
        // Both canonical root and refused candidate remain owned, no retry or
        // actual spawn-Err receipt. Fixture only, no thread or filesystem effect.
    }

    #[test]
    fn cancellation_before_std_launch_keeps_installed_slot_and_original_box() {
        use super::super::archive_thread::ThreadCapacity;
        static CAPACITY: ThreadCapacity = ThreadCapacity::isolated_test_counter();
        let original = operation();
        let foreign = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let slot = CAPACITY.try_claim(&original, &original).unwrap();
        let before = original.captured_bytes();
        root.interrupt(&original, Interruption::Timeout).unwrap();
        // Cancellation cannot erase an already-claimed original slot.
        root.retain_std_start_slot(&original, slot).ok().unwrap();
        original.overflow();
        assert!(root.begin_std_launch(&original).is_err());
        assert!(root.begin_std_launch(&foreign).is_err());
        root.interrupt(&original, Interruption::OperatorStop)
            .unwrap();
        let status = root.status(&original).unwrap();
        assert!(status.std_start_pending && !status.in_service && !status.returned);
        assert_eq!(status.first_interruption, Some(Interruption::Timeout));
        assert!(!status.worker_owned && !status.worker_join_retained && !status.std_start_refused);
        assert!(root.std_pending_slot.is_some() && !root.std_launch_started);
        let state = root.shared.state.lock().unwrap();
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().body, *b"keep");
        assert_eq!(work.driver().calls, 0);
        assert!(!state.checkout_started && !work.attempted);
        drop(state);
        assert_eq!(original.captured_bytes(), before);
        drop(root); // No actual launch/join: ordinary drop cannot free the slot.
        assert!(matches!(
            CAPACITY.try_claim(&foreign, &foreign),
            Err(ArchiveError::Fit)
        ));
    }

    #[test]
    fn std_launch_slot_in_service_and_returned_box_do_not_imply_handle_or_join() {
        use super::super::archive_thread::{RefusedStdStart, ThreadCapacity};
        static CAPACITY: ThreadCapacity = ThreadCapacity::isolated_test_counter();
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let slot = CAPACITY.try_claim(&original, &original).unwrap();
        root.retain_std_start_slot(&original, slot).ok().unwrap();
        let before = original.captured_bytes();
        original.overflow();
        let (slot, checkout) = root.begin_std_launch(&original).unwrap();
        assert!(slot.check_unstarted(&original).is_ok());
        let status = root.status(&original).unwrap();
        assert!(status.std_start_pending && status.in_service && !status.returned);
        assert!(!status.worker_owned && !status.worker_join_retained && !status.worker_associated);
        assert!(root.begin_std_launch(&original).is_err());
        drop(checkout);
        let status = root.status(&original).unwrap();
        assert!(status.std_start_pending && status.returned && !status.in_service);
        assert!(root.std_pending_slot.is_none()); // Slot in caller service, not free.
        assert!(root.read_std_returned(&original).is_err());
        root.interrupt(&original, Interruption::Timeout).unwrap();
        root.interrupt(&original, Interruption::OperatorStop)
            .unwrap();
        assert_eq!(
            root.status(&original).unwrap().first_interruption,
            Some(Interruption::Timeout)
        );
        let state = root.shared.state.lock().unwrap();
        assert_eq!(
            pointer,
            std::ptr::from_ref(state.work.as_ref().unwrap().driver())
        );
        drop(state);
        // Synthetic Err routes the SAME pending slot back; no actual Builder ran.
        let failure =
            RefusedStdStart::retain(slot, &original, std::io::Error::from_raw_os_error(5))
                .ok()
                .unwrap();
        root.retain_std_start_failure(&original, failure)
            .ok()
            .unwrap();
        assert_eq!(original.captured_bytes(), before);
        assert!(root.status(&original).unwrap().std_start_refused);
        assert!(!root.status(&original).unwrap().std_start_pending);
    }

    #[test]
    fn std_read_refuses_restored_callback_without_actual_std_join_and_preserves_error() {
        let original = operation();
        let foreign = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        assert!(root.read_std_returned(&original).is_err());
        let mut checkout = root.checkout(&original).unwrap();
        checkout
            .perform(&original, |driver, _| {
                driver.calls += 1; // Inert private callback, NO physical/file witness.
                Err(ArchiveError::Filesystem)
            })
            .unwrap();
        drop(checkout);
        let charged = original.captured_bytes();
        original.overflow();
        for _ in 0..2 {
            assert!(root.read_std_returned(&foreign).is_err());
            assert!(root.read_std_returned(&original).is_err());
            let status = root.status(&original).unwrap();
            assert!(status.returned && !status.in_service && status.result_retained);
            assert!(
                !status.worker_owned && !status.worker_join_retained && !status.worker_associated
            );
            let state = root.shared.state.lock().unwrap();
            let kept = state.work.as_ref().unwrap();
            assert_eq!(pointer, std::ptr::from_ref(kept.driver()));
            assert_eq!(kept.driver().body, *b"keep");
            assert_eq!(kept.driver().calls, 1);
            assert!(matches!(kept.result(), Some(Err(ArchiveError::Filesystem))));
        }
        assert_eq!(original.captured_bytes(), charged);
        assert!(root.checkout(&original).is_err()); // No second publication attempt.
        assert!(root.std_join(&original).unwrap().is_none());
    }

    #[test]
    fn std_cancel_before_registration_keeps_first_cause_and_original_checkout() {
        let original = operation();
        let foreign = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let root = RetainedArchive::prepare(work).ok().unwrap();
        let mut checkout = root.checkout(&original).unwrap();
        let before = original.captured_bytes();
        original.overflow();
        assert!(
            root.interrupt(&foreign, Interruption::OperatorStop)
                .is_err()
        );
        root.interrupt(&original, Interruption::Timeout).unwrap();
        root.interrupt(&original, Interruption::OperatorStop)
            .unwrap();
        assert!(checkout.wait_std_registration(&original).is_err());
        let mut called = false;
        assert!(
            checkout
                .perform_std_worker(&original, |_, _| {
                    called = true;
                    Err(ArchiveError::Filesystem)
                })
                .is_err()
        );
        assert!(!called);
        let pending = root.status(&original).unwrap();
        assert!(pending.in_service && !pending.returned && !pending.worker_associated);
        assert!(!pending.worker_owned && !pending.worker_join_retained);
        assert_eq!(pending.first_interruption, Some(Interruption::Timeout));
        assert!(root.std_join(&original).unwrap().is_none());
        assert!(root.std_registration_error(&original).unwrap().is_none());
        drop(checkout); // SAME canonical guard restores; no join fabricated.
        let returned = root.status(&original).unwrap();
        assert!(returned.returned && !returned.in_service);
        assert!(!returned.result_retained && !returned.panic_retained);
        let state = root.shared.state.lock().unwrap();
        let kept = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(kept.driver()));
        assert_eq!(kept.driver().body, *b"keep");
        assert_eq!(kept.driver().calls, 0);
        assert_eq!(original.captured_bytes(), before);
    }

    // Bounded inert fixture cleanup only. Timeout does NOT mark a join: abort
    // the same async fixture task and retain its actual JoinError before fail.
    // This helper is not supervision for a blocked spawn_blocking worker.
    async fn join_fixture(root: &mut RetainedArchive<OriginalDriver>, original: &Operation) {
        let waited =
            tokio::time::timeout_at(original.allowance().deadline(), root.join_worker(original))
                .await;
        if waited.is_err() {
            root.worker.as_ref().unwrap().abort();
            root.join_worker(original).await.unwrap();
            panic!("inert fixture join deadline exhausted; actual canceled join retained");
        }
        waited.unwrap().unwrap();
    }

    async fn retain_fixture(
        root: &mut RetainedArchive<OriginalDriver>,
        original: &Operation,
        task: JoinHandle<()>,
    ) {
        if let Err((_, task)) = root.retain_worker(original, task) {
            task.abort();
            let _actual_join = task.await;
            panic!("inert fixture registration refused; same task actually joined");
        }
    }

    #[tokio::test]
    async fn returned_read_requires_actual_join_and_borrows_same_error_without_lock_or_recharge() {
        let original = operation();
        let foreign = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        assert!(root.read_returned(&original).is_err());
        let mut checkout = root.checkout(&original).unwrap();
        let task_original = original.clone();
        let (returned, ready) = tokio::sync::oneshot::channel::<()>();
        let (release, gate) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            checkout
                .wait_worker_registration(&task_original)
                .await
                .unwrap();
            checkout
                .perform_worker(&task_original, |driver, _| {
                    driver.calls += 1;
                    Err(ArchiveError::Filesystem)
                })
                .unwrap();
            drop(checkout);
            let _ = returned.send(());
            let _ = gate.await;
        });
        retain_fixture(&mut root, &original, task).await;
        let ready = tokio::time::timeout_at(original.allowance().deadline(), ready).await;
        if !matches!(ready, Ok(Ok(()))) {
            root.worker.as_ref().unwrap().abort();
            join_fixture(&mut root, &original).await;
            panic!("inert return gate failed; actual original task join retained");
        }
        let restored_before_join = root.status(&original).unwrap().returned;
        let unjoined_refused = root.read_returned(&original).is_err();
        root.interrupt(&original, Interruption::Timeout).unwrap();
        let _ = release.send(());
        join_fixture(&mut root, &original).await;
        assert!(restored_before_join && unjoined_refused);
        // Actual Tokio fixture join/Box return cannot supply a std-thread receipt.
        assert!(root.read_std_returned(&original).is_err());
        assert!(root.read_returned(&foreign).is_err());
        let shared = root.shared.clone();
        let join_pointer = std::ptr::from_ref(root.worker_join(&original).unwrap().unwrap());
        let charged = original.captured_bytes();
        original.overflow(); // ORIGINAL pre-admitted lease remains readable, no renewal.
        for _ in 0..2 {
            let lease = root.read_returned(&original).unwrap();
            assert!(shared.state.try_lock().is_ok());
            {
                let state = shared.state.lock().unwrap();
                assert!(state.in_service && state.work.is_none());
            }
            assert_eq!(join_pointer, std::ptr::from_ref(lease.worker_join()));
            assert!(lease.worker_join().is_ok() && lease.worker_associated());
            assert_eq!(lease.first_interruption(), Some(Interruption::Timeout));
            let work = lease.work().unwrap();
            assert_eq!(pointer, std::ptr::from_ref(work.driver()));
            assert_eq!(work.driver().body, *b"keep");
            assert_eq!(work.driver().calls, 1);
            assert!(matches!(work.result(), Some(Err(ArchiveError::Filesystem))));
            assert!(!work.has_panic());
            drop(lease);
            let status = root.status(&original).unwrap();
            assert!(status.returned && !status.in_service && status.result_retained);
            assert!(status.worker_owned && status.worker_join_retained);
            assert!(root.checkout(&original).is_err()); // No publication retry.
        }
        assert_eq!(original.captured_bytes(), charged);
        assert!(original.borrow_archive_chunk(&original).is_ok());
        // No successful file observation, physical supervision or custody is
        // manufactured by reading an actual inert task's retained error.
    }

    #[tokio::test]
    #[cfg(panic = "unwind")]
    async fn returned_read_unwind_restores_box_and_poison_refuses_without_consuming_evidence() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let mut checkout = root.checkout(&original).unwrap();
        let task_original = original.clone();
        let task = tokio::spawn(async move {
            checkout
                .wait_worker_registration(&task_original)
                .await
                .unwrap();
            checkout
                .perform_worker(&task_original, |_, _| Err(ArchiveError::Mismatch))
                .unwrap();
        });
        retain_fixture(&mut root, &original, task).await;
        join_fixture(&mut root, &original).await;
        let shared = root.shared.clone();
        let unwind = catch_unwind(AssertUnwindSafe(|| {
            let lease = root.read_returned(&original).unwrap();
            assert!(shared.state.try_lock().is_ok());
            assert_eq!(pointer, std::ptr::from_ref(lease.work().unwrap().driver()));
            std::panic::resume_unwind(Box::new(11_u8));
        }));
        assert!(unwind.is_err());
        assert!(root.status(&original).unwrap().returned);
        {
            let lease = root.read_returned(&original).unwrap();
            assert!(matches!(
                lease.work().unwrap().result(),
                Some(Err(ArchiveError::Mismatch))
            ));
        }
        let poisoned = catch_unwind(AssertUnwindSafe(|| {
            let _state = shared.state.lock().unwrap();
            std::panic::resume_unwind(Box::new(12_u8));
        }));
        assert!(poisoned.is_err());
        assert!(root.read_returned(&original).is_err());
        let state = match shared.state.lock() {
            Err(poisoned) => poisoned.into_inner(),
            Ok(_) => panic!("synthetic poison cleared by returned read"),
        };
        assert!(!state.in_service);
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().body, *b"keep");
        assert!(matches!(work.result(), Some(Err(ArchiveError::Mismatch))));
        assert!(root.worker_join(&original).unwrap().unwrap().is_ok());
    }

    #[test]
    fn taskless_checkout_refuses_worker_callback_without_touching_original_box() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let root = RetainedArchive::prepare(work).ok().unwrap();
        let mut checkout = root.checkout(&original).unwrap();
        let charged = original.captured_bytes();
        assert!(matches!(
            checkout.perform_worker(&original, |_, _| panic!("taskless callback ran")),
            Err(ArchiveError::Identity)
        ));
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut wait = Box::pin(checkout.wait_worker_registration(&original));
        assert!(matches!(
            wait.as_mut().poll(&mut context),
            std::task::Poll::Ready(Err(ArchiveError::Identity))
        ));
        drop(wait);
        assert!(!checkout.work.as_ref().unwrap().attempted);
        assert_eq!(
            pointer,
            std::ptr::from_ref(checkout.work.as_ref().unwrap().driver())
        );
        assert_eq!(original.captured_bytes(), charged);
        drop(checkout);
        let status = root.status(&original).unwrap();
        assert!(status.returned);
        assert!(
            !status.worker_associated && !status.worker_join_retained && !status.result_retained
        );
    }

    #[tokio::test]
    async fn retained_handle_before_checkout_observation_requires_exact_task_and_original() {
        let original = operation();
        let foreign = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let mut checkout = root.checkout(&original).unwrap();
        let task_original = original.clone();
        let charged = original.captured_bytes();
        let (release, gate) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            if gate.await.is_err() {
                return;
            }
            // A retained handle alone is not callback admission.
            assert!(matches!(
                checkout.perform_worker(&task_original, |_, _| panic!("unobserved task callback")),
                Err(ArchiveError::Identity)
            ));
            assert!(checkout.wait_worker_registration(&foreign).await.is_err());
            checkout
                .wait_worker_registration(&task_original)
                .await
                .unwrap();
            assert!(matches!(
                checkout.perform_worker(&foreign, |_, _| panic!("foreign operation callback")),
                Err(ArchiveError::Identity)
            ));
            let shared = checkout.shared.clone();
            checkout
                .perform_worker(&task_original, |driver, operation| {
                    assert!(shared.state.try_lock().is_ok());
                    assert!(operation.same_instance(&task_original));
                    driver.calls += 1;
                    Err(ArchiveError::Filesystem)
                })
                .unwrap();
        });
        let task_id = task.id();
        retain_fixture(&mut root, &original, task).await;
        let before = root.status(&original).unwrap();
        original.overflow(); // SAME original reserve, no credit refill while binding.
        let released = release.send(()).is_ok();
        join_fixture(&mut root, &original).await;
        assert!(released && before.worker_owned && !before.worker_associated);
        assert!(matches!(root.worker_join(&original).unwrap(), Some(Ok(()))));
        let status = root.status(&original).unwrap();
        assert!(status.returned && status.worker_associated && status.worker_join_retained);
        let state = root.shared.state.lock().unwrap();
        assert_eq!(state.registered_worker, Some(task_id));
        assert_eq!(state.observed_worker, Some(task_id));
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().body, *b"keep");
        assert_eq!(work.driver().calls, 1);
        assert!(matches!(work.result(), Some(Err(ArchiveError::Filesystem))));
        assert_eq!(original.captured_bytes(), charged);
        // Even exact task binding and joined fixture are not file/physical custody.
    }

    #[tokio::test]
    async fn observed_checkout_canceled_wait_and_foreign_handle_keep_original_association() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let mut checkout = root.checkout(&original).unwrap();
        let task_original = original.clone();
        let charged = original.captured_bytes();
        let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
        let task_wake = wake.clone();
        let (observed, ready) = tokio::sync::oneshot::channel::<()>();
        let (release, gate) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let mut canceled = Box::pin(checkout.wait_worker_registration(&task_original));
            {
                let waker = std::task::Waker::from(task_wake.clone());
                let mut context = std::task::Context::from_waker(&waker);
                assert!(canceled.as_mut().poll(&mut context).is_pending());
            }
            drop(canceled); // SAME checkout/box and observed ID survive canceled borrow.
            let mut pending = Box::pin(checkout.wait_worker_registration(&task_original));
            {
                let waker = std::task::Waker::from(task_wake);
                let mut context = std::task::Context::from_waker(&waker);
                assert!(pending.as_mut().poll(&mut context).is_pending());
            }
            let _ = observed.send(()); // Both waits registered before root inspects state.
            if gate.await.is_err() {
                return;
            }
            pending.await.unwrap();
            checkout
                .perform_worker(&task_original, |driver, _| {
                    driver.calls += 1;
                    Err(ArchiveError::Filesystem)
                })
                .unwrap();
        });
        let task_id = task.id();
        let ready = tokio::time::timeout_at(original.allowance().deadline(), ready).await;
        if !matches!(ready, Ok(Ok(()))) {
            task.abort();
            let _actual_join = task.await; // No fixture worker left running on failure.
            panic!("checkout observation fixture failed; original task actually joined");
        }
        let before = root.status(&original).unwrap();
        let unrelated = tokio::spawn(std::future::pending::<()>());
        let unrelated_id = unrelated.id();
        let (error, unrelated) = match root.retain_worker(&original, unrelated) {
            Err(refused) => refused,
            Ok(()) => {
                root.worker.as_ref().unwrap().abort();
                root.join_worker(&original).await.unwrap();
                task.abort();
                let _original_join = task.await;
                panic!("wrong task registered; both inert fixture tasks actually joined");
            }
        };
        let returned_id = unrelated.id();
        unrelated.abort();
        let unrelated_join = unrelated.await;
        retain_fixture(&mut root, &original, task).await;
        let woken = wake.0.load(Ordering::SeqCst) > 0;
        root.interrupt(&original, Interruption::Timeout).unwrap();
        root.interrupt(&original, Interruption::OperatorStop)
            .unwrap();
        let released = release.send(()).is_ok();
        join_fixture(&mut root, &original).await;
        assert!(before.in_service && !before.worker_owned && !before.worker_associated);
        assert!(matches!(error, ArchiveError::Identity));
        assert_eq!(unrelated_id, returned_id);
        assert!(unrelated_join.as_ref().is_err_and(JoinError::is_cancelled));
        assert!(woken && released);
        assert!(matches!(root.worker_join(&original).unwrap(), Some(Ok(()))));
        let status = root.status(&original).unwrap();
        assert!(status.returned && status.worker_associated && status.worker_join_retained);
        assert_eq!(status.first_interruption, Some(Interruption::Timeout));
        let state = root.shared.state.lock().unwrap();
        assert_eq!(state.registered_worker, Some(task_id));
        assert_eq!(state.observed_worker, Some(task_id));
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().calls, 1);
        assert!(matches!(work.result(), Some(Err(ArchiveError::Filesystem))));
        assert_eq!(original.captured_bytes(), charged);
    }

    #[tokio::test]
    async fn moved_checkout_cannot_retag_to_another_actual_task_after_original_join() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let mut checkout = root.checkout(&original).unwrap();
        let task_original = original.clone();
        let charged = original.captured_bytes();
        let (handed, received) = tokio::sync::oneshot::channel::<ArchiveCheckout<OriginalDriver>>();
        let task = tokio::spawn(async move {
            checkout
                .wait_worker_registration(&task_original)
                .await
                .unwrap();
            // Deliberately invalid fixture handoff. Original-task join must NOT
            // be mistaken for box restoration or a usable replacement boundary.
            let _ = handed.send(checkout);
        });
        let original_task_id = task.id();
        retain_fixture(&mut root, &original, task).await;
        join_fixture(&mut root, &original).await;
        let joined_before_restore = root.status(&original).unwrap();
        assert!(root.read_returned(&original).is_err()); // Joined, but box is still in service.
        assert_eq!(root.worker.as_ref().unwrap().id(), original_task_id);
        assert!(root.worker.as_ref().unwrap().is_finished());
        let mut checkout = received.await.unwrap(); // Producer is already actually joined.
        let other_original = original.clone();
        let mut other = tokio::spawn(async move {
            assert!(
                checkout
                    .wait_worker_registration(&other_original)
                    .await
                    .is_err()
            );
            assert!(matches!(
                checkout.perform_worker(&other_original, |_, _| panic!("retagged worker callback")),
                Err(ArchiveError::Identity)
            ));
            assert!(!checkout.work.as_ref().unwrap().attempted);
            // Drop restores SAME box; this is not a task or archive acceptance ACK.
        });
        let other_id = other.id();
        let waited = tokio::time::timeout_at(original.allowance().deadline(), &mut other).await;
        if waited.is_err() {
            other.abort();
            let _actual_join = other.await;
            panic!("retag fixture deadline exhausted; same task actually joined");
        }
        assert!(matches!(waited, Ok(Ok(()))));
        assert_ne!(original_task_id, other_id);
        assert!(joined_before_restore.in_service && joined_before_restore.worker_associated);
        assert!(joined_before_restore.worker_join_retained && !joined_before_restore.returned);
        assert!(!joined_before_restore.result_retained);
        let after = root.status(&original).unwrap();
        assert!(after.returned && after.worker_join_retained && after.worker_associated);
        assert!(!after.result_retained && !after.panic_retained);
        let state = root.shared.state.lock().unwrap();
        assert_eq!(state.registered_worker, Some(original_task_id));
        assert_eq!(state.observed_worker, Some(original_task_id));
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().calls, 0);
        assert_eq!(work.driver().body, *b"keep");
        assert_eq!(original.captured_bytes(), charged);
    }

    #[tokio::test]
    async fn original_worker_handle_refusal_canceled_borrow_and_duplicate_keep_handles() {
        let original = operation();
        let foreign = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let premature = tokio::spawn(std::future::pending::<()>());
        let premature_id = premature.id();
        let (premature_error, premature) = root.retain_worker(&original, premature).err().unwrap();
        let refused_premature_id = premature.id();
        premature.abort();
        let premature_join = premature.await;
        let checkout = root.checkout(&original).unwrap();
        let charged = original.captured_bytes();
        let (release, gate) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _checkout = checkout;
            let _ = gate.await;
        });
        let task_id = task.id();
        let (error, task) = match root.retain_worker(&foreign, task) {
            Err(refused) => refused,
            Ok(_) => panic!("foreign original associated"),
        };
        let refused_same_handle = task.id() == task_id;
        let refused_identity = matches!(error, ArchiveError::Identity);
        root.retain_worker(&original, task).ok().unwrap();
        let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
        let waker = std::task::Waker::from(wake);
        let mut context = std::task::Context::from_waker(&waker);
        let mut canceled = Box::pin(root.join_worker(&original));
        let pending = canceled.as_mut().poll(&mut context).is_pending();
        drop(canceled); // Root still owns the ORIGINAL handle; no abort/ACK.
        let retained_id = root.worker.as_ref().unwrap().id();
        let retained_before = root.worker_join.is_none();
        root.interrupt(&original, Interruption::Timeout).unwrap();
        original.overflow();
        let extra = tokio::spawn(std::future::pending::<()>());
        let extra_id = extra.id();
        let (extra_error, extra) = root.retain_worker(&original, extra).err().unwrap();
        let refused_extra_id = extra.id();
        extra.abort();
        let extra_join = extra.await; // Rejected fixture handle is also actually joined.
        let released = release.send(()).is_ok();
        join_fixture(&mut root, &original).await;
        assert!(refused_identity && refused_same_handle && pending && retained_before && released);
        assert_eq!(premature_id, refused_premature_id);
        assert!(matches!(premature_error, ArchiveError::Identity));
        assert!(premature_join.as_ref().is_err_and(JoinError::is_cancelled));
        assert_eq!(task_id, retained_id);
        assert_eq!(extra_id, refused_extra_id);
        assert!(matches!(extra_error, ArchiveError::Identity));
        assert!(extra_join.as_ref().is_err_and(JoinError::is_cancelled));
        let status = root.status(&original).unwrap();
        assert!(status.returned && status.worker_join_retained);
        assert!(status.worker_owned && !status.result_retained);
        assert!(root.worker.as_ref().unwrap().is_finished());
        assert_eq!(root.worker.as_ref().unwrap().id(), task_id);
        assert_eq!(status.first_interruption, Some(Interruption::Timeout));
        assert!(matches!(root.worker_join(&original).unwrap(), Some(Ok(()))));
        root.join_worker(&original).await.unwrap(); // SAME fact, no repeat handle poll.
        assert!(root.worker_join(&foreign).is_err());
        let state = root.shared.state.lock().unwrap();
        assert_eq!(
            pointer,
            std::ptr::from_ref(state.work.as_ref().unwrap().driver())
        );
        assert_eq!(original.captured_bytes(), charged);
        // Inert task joins establish no file publication or physical child/readers joins.
    }

    #[tokio::test]
    async fn restored_callback_error_is_separate_from_pending_worker_timeout_and_join() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let mut checkout = root.checkout(&original).unwrap();
        let task_original = original.clone();
        let (returned, ready) = tokio::sync::oneshot::channel::<()>();
        let (release, gate) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            checkout
                .perform(&task_original, |driver, _| {
                    driver.calls += 1;
                    Err(ArchiveError::Filesystem)
                })
                .unwrap();
            drop(checkout);
            let _ = returned.send(());
            let _ = gate.await; // Callback return/restoration precedes actual task completion.
        });
        root.retain_worker(&original, task).ok().unwrap();
        let ready = tokio::time::timeout_at(original.allowance().deadline(), ready).await;
        if ready.is_err() {
            root.worker.as_ref().unwrap().abort();
            join_fixture(&mut root, &original).await;
            panic!("fixture restoration did not return by original deadline");
        }
        let returned_ack = ready.unwrap().is_ok();
        let before = root.status(&original).unwrap();
        root.interrupt(&original, Interruption::Timeout).unwrap();
        root.interrupt(&original, Interruption::OperatorStop)
            .unwrap();
        let released = release.send(()).is_ok();
        join_fixture(&mut root, &original).await;
        root.interrupt(&original, Interruption::OperatorStop)
            .unwrap();
        assert!(returned_ack && released);
        assert!(before.returned && before.result_retained && before.worker_owned);
        assert!(!before.worker_join_retained);
        assert_eq!(
            root.status(&original).unwrap().first_interruption,
            Some(Interruption::Timeout)
        );
        assert!(matches!(root.worker_join(&original).unwrap(), Some(Ok(()))));
        let state = root.shared.state.lock().unwrap();
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().calls, 1);
        assert!(matches!(work.result(), Some(Err(ArchiveError::Filesystem))));
    }

    #[tokio::test]
    #[cfg(panic = "unwind")]
    async fn worker_opaque_join_panic_and_restored_box_are_retained_without_formatting() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let checkout = root.checkout(&original).unwrap();
        let dropped = Arc::new(AtomicUsize::new(0));
        let payload = Box::new(OpaquePanic {
            dropped: dropped.clone(),
        });
        let task = tokio::spawn(async move {
            let _checkout = checkout;
            std::panic::resume_unwind(payload); // No Display, hook or diagnostic conversion.
        });
        root.retain_worker(&original, task).ok().unwrap();
        join_fixture(&mut root, &original).await;
        let error = root
            .worker_join(&original)
            .unwrap()
            .unwrap()
            .as_ref()
            .unwrap_err();
        assert!(error.is_panic());
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        let status = root.status(&original).unwrap();
        assert!(status.returned && status.worker_join_retained);
        assert!(!status.result_retained && !status.panic_retained && status.worker_owned);
        assert!(root.worker.as_ref().unwrap().is_finished());
        assert_eq!(status.first_interruption, None);
        root.interrupt(&original, Interruption::Timeout).unwrap();
        assert_eq!(root.status(&original).unwrap().first_interruption, None);
        let state = root.shared.state.lock().unwrap();
        assert_eq!(
            pointer,
            std::ptr::from_ref(state.work.as_ref().unwrap().driver())
        );
        drop(state);
        let lease = root.read_returned(&original).unwrap();
        assert!(lease.worker_join().as_ref().is_err_and(JoinError::is_panic));
        assert!(!lease.worker_associated()); // Task unwound before observation.
        let work = lease.work().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert!(work.result().is_none() && !work.has_panic());
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        drop(lease);
        assert!(root.status(&original).unwrap().returned);
        // Root retains this ORIGINAL JoinError separately; it is NOT archiveable
        // merely because the box returned or the task joined. Teardown is fixture-only.
    }

    #[tokio::test]
    async fn aborted_fixture_worker_join_is_not_callback_or_healthy_archive_ack() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let checkout = root.checkout(&original).unwrap();
        let charged = original.captured_bytes();
        let task = tokio::spawn(async move {
            let _checkout = checkout;
            std::future::pending::<()>().await;
        });
        root.retain_worker(&original, task).ok().unwrap();
        root.worker.as_ref().unwrap().abort(); // Abort alone is never a join.
        join_fixture(&mut root, &original).await;
        assert!(
            root.worker_join(&original)
                .unwrap()
                .unwrap()
                .as_ref()
                .is_err_and(JoinError::is_cancelled)
        );
        let status = root.status(&original).unwrap();
        assert!(status.returned && status.worker_join_retained);
        assert!(status.worker_owned && !status.result_retained && !status.panic_retained);
        assert!(root.worker.as_ref().unwrap().is_finished());
        let state = root.shared.state.lock().unwrap();
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().body, *b"keep");
        assert_eq!(work.driver().calls, 0);
        assert_eq!(original.captured_bytes(), charged);
        drop(state);
        let lease = root.read_returned(&original).unwrap();
        assert!(
            lease
                .worker_join()
                .as_ref()
                .is_err_and(JoinError::is_cancelled)
        );
        assert!(!lease.worker_associated());
        assert_eq!(pointer, std::ptr::from_ref(lease.work().unwrap().driver()));
        assert!(lease.work().unwrap().result().is_none());
        drop(lease);
        assert!(root.status(&original).unwrap().returned);
    }

    #[test]
    fn retained_slot_preparation_refusal_keeps_same_work_box_and_meter() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let charged = original.captured_bytes();
        original.overflow();
        let (error, work) = match RetainedArchive::prepare(work) {
            Err(refused) => refused,
            Ok(_) => panic!("closed retained slot accepted"),
        };
        assert!(matches!(
            error,
            ArchiveError::Operation(crate::operation::OperationError::CaptureOverflow)
        ));
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().body, *b"keep");
        assert_eq!(original.captured_bytes(), charged);

        let original = operation();
        let mut work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        work.perform(&original, |_, _| Err(ArchiveError::Filesystem))
            .unwrap();
        let (error, work) = match RetainedArchive::prepare(work) {
            Err(refused) => refused,
            Ok(_) => panic!("post-callback root preparation accepted"),
        };
        assert!(matches!(error, ArchiveError::Identity));
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert!(matches!(work.result(), Some(Err(ArchiveError::Filesystem))));
    }

    #[test]
    fn checkout_combined_fixed_refusal_runs_no_callback_or_recharge() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let root = RetainedArchive::prepare(work).ok().unwrap();
        let mut checkout = root.checkout(&original).unwrap();
        let charged = original.captured_bytes();
        let oversized = [0_u8; crate::failed_evidence::FIXED_ARCHIVE_WORKSPACE];
        assert!(matches!(
            checkout.perform(&original, move |_, _| {
                std::hint::black_box(&oversized);
                panic!("unadmitted callback ran")
            }),
            Err(ArchiveError::Fit)
        ));
        let work = checkout.work.as_ref().unwrap();
        assert!(!work.attempted);
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().calls, 0);
        assert_eq!(original.captured_bytes(), charged);
        drop(checkout);
        assert!(!root.status(&original).unwrap().result_retained);
    }

    #[test]
    fn checkout_none_is_owned_and_callback_uses_no_metadata_lock() {
        let original = operation();
        let foreign = operation();
        let mut work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        work.interrupt(Interruption::Timeout);
        let pointer = std::ptr::from_ref(work.driver());
        let root = RetainedArchive::prepare(work).ok().unwrap();
        let charged = original.captured_bytes();
        assert!(root.checkout(&foreign).is_err());
        let mut checkout = root.checkout(&original).unwrap();
        assert!(root.checkout(&original).is_err());
        let status = root.status(&original).unwrap();
        assert!(status.in_service);
        assert!(!status.returned);
        assert!(!status.result_retained);
        assert!(!status.panic_retained);
        assert_eq!(status.first_interruption, Some(Interruption::Timeout));
        assert!(root.shared.state.lock().unwrap().work.is_none());
        assert_eq!(
            pointer,
            std::ptr::from_ref(checkout.work.as_ref().unwrap().driver())
        );
        original.overflow(); // Uses only ORIGINAL pre-admitted slot, no refill.
        root.interrupt(&original, Interruption::OperatorStop)
            .unwrap();
        checkout
            .perform(&original, |driver, operation| {
                // A held callback lock fails immediately instead of hanging the control.
                assert!(root.shared.state.try_lock().is_ok());
                assert!(operation.same_instance(&original));
                driver.calls += 1;
                Err(ArchiveError::Filesystem)
            })
            .unwrap();
        assert!(root.status(&original).unwrap().in_service);
        drop(checkout); // SAME box restoration, not task/child join or custody.
        let status = root.status(&original).unwrap();
        assert!(!status.in_service);
        assert!(status.returned);
        assert!(status.result_retained);
        assert!(!status.panic_retained);
        assert_eq!(status.first_interruption, Some(Interruption::Timeout));
        root.interrupt(&original, Interruption::OperatorStop)
            .unwrap();
        assert_eq!(
            root.status(&original).unwrap().first_interruption,
            Some(Interruption::Timeout)
        );
        assert!(root.checkout(&original).is_err()); // No checkout/retry renewal.
        let state = root.shared.state.lock().unwrap();
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().body, *b"keep");
        assert_eq!(work.driver().calls, 1);
        assert!(matches!(work.result(), Some(Err(ArchiveError::Filesystem))));
        assert_eq!(original.captured_bytes(), charged);
        assert!(original.borrow_archive_chunk(&original).is_ok());
    }

    struct WakeCounter(AtomicUsize);
    impl std::task::Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn canceled_borrowed_wait_keeps_box_and_restoration_wakes_registered_wait() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let mut root = RetainedArchive::prepare(work).ok().unwrap();
        let checkout = root.checkout(&original).unwrap();
        let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
        let waker = std::task::Waker::from(wake.clone());
        let mut context = std::task::Context::from_waker(&waker);
        // Manual bounded polling: no worker, runtime task, timer or filesystem.
        let mut canceled = Box::pin(root.wait_returned(&original));
        assert!(canceled.as_mut().poll(&mut context).is_pending());
        drop(canceled);
        assert!(root.status(&original).unwrap().in_service);
        assert_eq!(
            pointer,
            std::ptr::from_ref(checkout.work.as_ref().unwrap().driver())
        );
        let mut pending = Box::pin(root.wait_returned(&original));
        assert!(pending.as_mut().poll(&mut context).is_pending());
        drop(checkout);
        assert!(wake.0.load(Ordering::SeqCst) > 0);
        assert!(matches!(
            pending.as_mut().poll(&mut context),
            std::task::Poll::Ready(Ok(()))
        ));
        drop(pending);
        let status = root.status(&original).unwrap();
        assert!(status.returned);
        assert!(!status.result_retained); // Restoration is NOT publication success.
        assert!(!status.panic_retained);
        assert_eq!(status.first_interruption, None);
        // Already-restored before registration is observed without needing a wake.
        let mut late = Box::pin(root.wait_returned(&original));
        assert!(matches!(
            late.as_mut().poll(&mut context),
            std::task::Poll::Ready(Ok(()))
        ));
        drop(late);
        let state = root.shared.state.lock().unwrap();
        assert_eq!(
            pointer,
            std::ptr::from_ref(state.work.as_ref().unwrap().driver())
        );
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn checkout_guard_unwind_restores_original_box_without_callback_ack() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let root = RetainedArchive::prepare(work).ok().unwrap();
        let checkout = root.checkout(&original).unwrap();
        root.interrupt(&original, Interruption::OperatorStop)
            .unwrap();
        let unwind = catch_unwind(AssertUnwindSafe(move || {
            let _checkout = checkout;
            std::panic::resume_unwind(Box::new(7_u8)); // No hook/formatter/task launch.
        }));
        assert!(unwind.is_err());
        let status = root.status(&original).unwrap();
        assert!(status.returned);
        assert!(!status.in_service);
        assert!(!status.result_retained);
        assert!(!status.panic_retained); // Outer task error needs its OWN retained join result.
        assert_eq!(status.first_interruption, Some(Interruption::OperatorStop));
        let state = root.shared.state.lock().unwrap();
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().body, *b"keep");
        assert_eq!(work.driver().calls, 0);
        // Fixture teardown grants no production root/task discard permission.
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn poisoned_metadata_refuses_observation_but_guard_restores_same_box() {
        let original = operation();
        let work = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(work.driver());
        let root = RetainedArchive::prepare(work).ok().unwrap();
        let checkout = root.checkout(&original).unwrap();
        let unwind = catch_unwind(AssertUnwindSafe(|| {
            let _state = root.shared.state.lock().unwrap();
            std::panic::resume_unwind(Box::new(9_u8));
        }));
        assert!(unwind.is_err());
        assert!(root.status(&original).is_err());
        assert!(root.checkout(&original).is_err());
        drop(checkout);
        assert!(root.status(&original).is_err()); // Recovery is not healthy admission.
        let state = match root.shared.state.lock() {
            Err(poisoned) => poisoned.into_inner(),
            Ok(_) => panic!("synthetic poison was cleared"),
        };
        assert!(!state.in_service);
        let work = state.work.as_ref().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(work.driver()));
        assert_eq!(work.driver().body, *b"keep");
        assert_eq!(work.driver().calls, 0);
        assert!(work.result().is_none());
    }

    #[test]
    fn pre_capture_preparation_refusal_returns_same_box_without_callback() {
        let original = operation();
        let driver = driver();
        let pointer = std::ptr::from_ref(driver.as_ref());
        let charged = original.captured_bytes();
        original.overflow();
        let (error, driver) = match ArchiveWork::prepare(driver, &original) {
            Err(refused) => refused,
            Ok(_) => panic!("closed preparation accepted"),
        };
        assert!(matches!(
            error,
            ArchiveError::Operation(crate::operation::OperationError::CaptureOverflow)
        ));
        assert_eq!(pointer, std::ptr::from_ref(driver.as_ref()));
        assert_eq!(driver.calls, 0);
        assert_eq!(driver.body, *b"keep");
        assert_eq!(original.captured_bytes(), charged);
    }

    #[test]
    fn callback_error_and_original_box_survive_move_without_wait_or_retry_ack() {
        fn send_owner<T: Send + 'static>() {}
        send_owner::<ArchiveWork<OriginalDriver>>(); // No worker/task launch.
        let original = operation();
        let replacement = operation();
        let driver = driver();
        let pointer = std::ptr::from_ref(driver.as_ref());
        let mut owner = ArchiveWork::prepare(driver, &original).ok().unwrap();
        let charged = original.captured_bytes();
        assert!(owner.checked_custody_source(&original).is_err());
        assert!(owner.checked_custody_source(&replacement).is_err());
        assert!(
            owner
                .perform(&replacement, |_, _| panic!("retagged callback"))
                .is_err()
        );
        assert!(!owner.attempted);
        owner.interrupt(Interruption::Timeout);
        original.overflow(); // Prepared ORIGINAL cleanup storage, no re-admission.
        owner
            .perform(&original, |driver, operation| {
                assert!(operation.same_instance(&original));
                driver.calls += 1;
                Err(ArchiveError::Filesystem)
            })
            .unwrap();
        let mut retained = Some(owner); // Simulates retained outer slot, not task/join evidence.
        let mut owner = retained.take().unwrap();
        owner.interrupt(Interruption::OperatorStop);
        assert!(owner.checked_custody_source(&original).is_err());
        assert!(owner.checked_custody_source(&replacement).is_err());
        assert!(matches!(
            owner.result(),
            Some(Err(ArchiveError::Filesystem))
        ));
        assert_eq!(owner.first_interruption(), Some(Interruption::Timeout));
        assert!(!owner.has_panic());
        assert_eq!(pointer, std::ptr::from_ref(owner.driver()));
        assert_eq!(owner.driver().body, *b"keep");
        assert_eq!(owner.driver().calls, 1);
        assert!(
            owner
                .perform(&original, |_, _| panic!("second callback"))
                .is_err()
        );
        assert_eq!(original.captured_bytes(), charged);
        assert!(original.borrow_archive_chunk(&original).is_ok());
    }

    #[test]
    fn late_interruption_cannot_replace_already_returned_archive_error() {
        let original = operation();
        let mut owner = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        owner
            .perform(&original, |_, _| Err(ArchiveError::Mismatch))
            .unwrap();
        owner.interrupt(Interruption::Timeout);
        assert_eq!(owner.first_interruption(), None);
        assert!(matches!(owner.result(), Some(Err(ArchiveError::Mismatch))));
        assert!(owner.attempted);
    }

    struct OpaquePanic {
        dropped: Arc<AtomicUsize>,
    }
    impl Drop for OpaquePanic {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[test]
    #[cfg(panic = "unwind")]
    fn unwind_keeps_canonical_box_and_original_opaque_payload() {
        let original = operation();
        let mut owner = ArchiveWork::prepare(driver(), &original).ok().unwrap();
        let pointer = std::ptr::from_ref(owner.driver());
        let charged = original.captured_bytes();
        let dropped = Arc::new(AtomicUsize::new(0));
        let payload = Box::new(OpaquePanic {
            dropped: dropped.clone(),
        });
        let payload_pointer = std::ptr::from_ref(payload.as_ref());
        owner.interrupt(Interruption::OperatorStop);
        owner
            .perform(&original, |driver, _| {
                driver.calls += 1;
                std::panic::resume_unwind(payload); // No panic hook/Display/body conversion.
            })
            .unwrap();
        assert!(owner.has_panic());
        assert!(owner.result().is_none());
        assert!(owner.checked_custody_source(&original).is_err());
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        assert_eq!(
            payload_pointer,
            std::ptr::from_ref(
                owner
                    .panic
                    .as_ref()
                    .unwrap()
                    .downcast_ref::<OpaquePanic>()
                    .unwrap()
            )
        );
        assert_eq!(pointer, std::ptr::from_ref(owner.driver()));
        assert_eq!(owner.driver().body, *b"keep");
        assert_eq!(owner.driver().calls, 1);
        owner.interrupt(Interruption::Timeout);
        assert_eq!(owner.first_interruption(), Some(Interruption::OperatorStop));
        assert!(
            owner
                .perform(&original, |_, _| panic!("panic retry"))
                .is_err()
        );
        assert_eq!(original.captured_bytes(), charged);
        // Synthetic fixture teardown is not permission to discard production evidence.
    }
}
