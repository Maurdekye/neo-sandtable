//! Private caller assembly behind ONE opaque operator seam; no runtime adoption.
//! The concrete root and its SAME-original destinations exist before erasure.

use super::archive_work::{
    ControllerCreationLayouts, ControllerResults, OriginalClaudeOwner, RetainedArchive,
};
use super::claude::{ClaudeConfig, ClaudeDriver};
use super::{DriverError, RetainedDriverFuture, SessionInfo, TurnOutcome};
use crate::{
    failed_evidence::{ArchiveError, CampaignArchive},
    host::HostDriverContext,
    operation::{Allowance, Operation},
    transcript::TranscriptSink,
};

/// Trusted metadata owned BEFORE an Operation exists. No placeholder meter,
/// second Operation, HostDriverContext clone or native admission is needed.
#[allow(dead_code)] // SOURCE creation aggregation; not public factory adoption.
pub(super) struct PendingContextInputs {
    pub(super) admission: std::sync::Arc<dyn crate::host::HostAdmission>,
    pub(super) identity: crate::host::HostIdentity,
}

#[allow(dead_code)]
pub(super) struct ContextCreationLayout {
    pub(super) inline: std::alloc::Layout,
    // Dynamic concrete object itself, not just the Arc handle. The referenced
    // campaign service graph is outside m67's per-Operation cap. This earlier
    // conservative snapshot does NOT replace its persistent per-Operation entry
    // obligation, which arrives separately through the bound owner descriptor.
    pub(super) admission_body: std::alloc::Layout,
    pub(super) payload_bytes: usize,
}

#[allow(dead_code)]
impl PendingContextInputs {
    pub(super) fn creation_layout(&self) -> Result<ContextCreationLayout, ArchiveError> {
        self.creation_layout_with_cap(crate::operation::OPERATION_BYTES)
    }

    fn creation_layout_with_cap(&self, cap: usize) -> Result<ContextCreationLayout, ArchiveError> {
        let inline = std::alloc::Layout::for_value(self).pad_to_align();
        // Reads the actual trait-object layout metadata without calling ANY
        // HostAdmission method, downcasting or constructing a snapshot/future.
        let admission_body = std::alloc::Layout::for_value(self.admission.as_ref()).pad_to_align();
        let mut payload_bytes = 0usize;
        for body in [&self.identity.decision_id, &self.identity.operation_id] {
            let layout =
                std::alloc::Layout::array::<u8>(body.capacity()).map_err(|_| ArchiveError::Fit)?;
            payload_bytes = payload_bytes
                .checked_add(layout.pad_to_align().size())
                .ok_or(ArchiveError::Fit)?;
        }
        if inline
            .size()
            .checked_add(payload_bytes)
            .and_then(|bytes| bytes.checked_add(admission_body.size()))
            .is_none_or(|bytes| bytes > cap)
        {
            return Err(ArchiveError::Fit);
        }
        // Conservative handle/immediate object snapshot. Shared campaign host
        // graph follows its own policy under m67; this is NOT its per-Operation
        // persistent-entry charge or a complete Operation-owned storage fit.
        Ok(ContextCreationLayout {
            inline,
            admission_body,
            payload_bytes,
        })
    }

    #[cfg(test)]
    pub(super) fn layout_with_cap_for_source_control(
        &self,
        cap: usize,
    ) -> Result<ContextCreationLayout, ArchiveError> {
        self.creation_layout_with_cap(cap)
    }
}

/// Actual child body headers which the root's inline ChildProc does not contain.
/// This is a type-only inventory: no child, Arc, mutex, queue or future is created.
/// Backing buffers, error payloads, Arc/library allocations, reader tasks and the
/// private stdout FIFO remain separate obligations. These facts do not establish
/// a precreated child association, a live-instance count or complete original F.
#[allow(dead_code)]
pub(super) struct ChildCreationLayouts {
    pub(super) child_inline: std::alloc::Layout,
    pub(super) stdout_assembly: std::alloc::Layout,
    pub(super) stdout_pending: std::alloc::Layout,
    pub(super) stdout_chunk: std::alloc::Layout,
    pub(super) stderr_chunk: std::alloc::Layout,
    pub(super) stderr_tail: std::alloc::Layout,
    pub(super) stderr_stats: std::alloc::Layout,
    pub(super) reader_failures: std::alloc::Layout,
    pub(super) capture_cell: std::alloc::Layout,
    pub(super) reader_operation_error_body_capacity: usize,
    pub(super) referenced_headers_bytes: usize,
}

impl ChildCreationLayouts {
    fn derive(
        physical: Option<&super::child_pending::PhysicalStorageLayouts>,
    ) -> Result<Self, ArchiveError> {
        use std::{alloc::Layout, sync::Mutex};
        let mut layouts = Self {
            child_inline: Layout::new::<super::ChildProc>(),
            stdout_assembly: Layout::new::<Mutex<super::FrameAssembly>>(),
            stdout_pending: Layout::new::<Mutex<Option<super::ChildFrame>>>(),
            stdout_chunk: Layout::new::<Mutex<Option<super::StdoutChunk>>>(),
            stderr_chunk: Layout::new::<Mutex<Option<super::StdoutChunk>>>(),
            stderr_tail: Layout::new::<Mutex<super::StderrCapture>>(),
            stderr_stats: Layout::new::<Mutex<super::StderrStats>>(),
            reader_failures: Layout::new::<Mutex<super::reader_failure::ReaderFailures>>(),
            capture_cell: Layout::new::<Mutex<Operation>>(),
            // Capacity fact ONLY: do not multiply by a guessed lifetime count
            // or substitute it for opaque I/O wrappers/other error producers.
            reader_operation_error_body_capacity:
                super::reader_failure::OPERATION_ERROR_BODY_CAPACITY,
            referenced_headers_bytes: 0,
        };
        if let Some(physical) = physical {
            // Replace all eight type-only facts with ACTUAL staged bodies.
            // Do not add both inventories; referenced children stay separate.
            [
                layouts.stdout_assembly,
                layouts.stdout_pending,
                layouts.stdout_chunk,
                layouts.stderr_chunk,
                layouts.stderr_stats,
            ] = physical.bodies;
            layouts.reader_failures = physical.reader_failures_body;
            layouts.capture_cell = physical.capture_body;
            layouts.stderr_tail = physical.stderr_tail_body;
        }
        // ChildProc's inline fields already occur inside the concrete root.
        // Only these eight distinct referenced body headers enter this subtotal;
        // pointer/header sizes are never substituted for their heap children.
        layouts.referenced_headers_bytes = [
            layouts.stdout_assembly,
            layouts.stdout_pending,
            layouts.stdout_chunk,
            layouts.stderr_chunk,
            layouts.stderr_tail,
            layouts.stderr_stats,
            layouts.reader_failures,
            layouts.capture_cell,
        ]
        .into_iter()
        .try_fold(0usize, |bytes, layout| bytes.checked_add(layout.size()))
        .ok_or(ArchiveError::Fit)?;
        Ok(layouts)
    }
}

/// Partial, concrete storage inventory. This is intentionally NOT a complete
/// creation-fit receipt and cannot authorize Operation allocation or binding.
/// Operation-owned queue/buffer/library storage, actual owner-source persistent
/// entry facts, campaign outer frames and later growing payload bounds remain
/// incomplete. Shared campaign services follow their own policy under m67.
#[allow(dead_code)]
pub(super) struct KnownCreationLayout {
    pub(super) caller: std::alloc::Layout,
    pub(super) controllers: ControllerCreationLayouts,
    pub(super) child: ChildCreationLayouts,
    pub(super) fixed_bytes: usize,
    pub(super) configuration_payload_bytes: usize,
    pub(super) context_payload_bytes: usize,
    pub(super) admission_body_bytes: usize,
    pub(super) transcript_referenced_bytes: usize,
    pub(super) pending_transcript_bytes: usize,
    pub(super) owner_persistent_bytes: usize,
    pub(super) stderr_lines_bytes: usize,
    pub(super) stdout_fifo_bytes: usize,
    pub(super) stdout_receive_bytes: usize,
    pub(super) physical_storage_counter_bytes: usize,
    pub(super) physical_pipe_body_bytes: usize,
    pub(super) known_bytes: usize,
}

/// Stage actual cfg and pending transcript in one owner before constructing
/// the SAME concrete driver. Every refused/unwound stage retains its inputs.
/// Assembly success is NOT original creation/admission or native authority.
#[allow(dead_code)]
pub(super) struct PendingClaudeFactory {
    configuration: Option<ClaudeConfig>,
    transcript: Option<crate::transcript::PendingTranscriptInputs>,
    producer: Option<TranscriptSink>,
    owner: Option<OriginalClaudeOwner>,
    assembly_started: bool,
    first_assembly: Option<Result<(), ArchiveError>>,
}

#[allow(dead_code)]
impl PendingClaudeFactory {
    pub(super) fn new(
        configuration: ClaudeConfig,
        transcript: crate::transcript::PendingTranscriptInputs,
    ) -> Self {
        Self {
            configuration: Some(configuration),
            transcript: Some(transcript),
            producer: None,
            owner: None,
            assembly_started: false,
            first_assembly: None,
        }
    }

    pub(super) fn assemble(&mut self) -> Result<(), ArchiveError> {
        self.assemble_boundary(|| ())
    }

    fn assemble_boundary(&mut self, after_owner: impl FnOnce()) -> Result<(), ArchiveError> {
        if self.assembly_started {
            return Err(ArchiveError::Identity);
        }
        self.assembly_started = true;
        let result = self.install_owner(after_owner);
        self.first_assembly = Some(result); // First actual MOVE before fixed observation.
        if matches!(self.first_assembly, Some(Ok(()))) {
            Ok(())
        } else {
            Err(ArchiveError::Identity)
        }
    }

    fn install_owner(&mut self, after_owner: impl FnOnce()) -> Result<(), ArchiveError> {
        if self.configuration.is_none() || self.owner.is_some() || self.producer.is_some() {
            return Err(ArchiveError::Identity);
        }
        let transcript = self.transcript.as_mut().ok_or(ArchiveError::Identity)?;
        if transcript.creation_layout().is_none() {
            transcript.measure_creation()?;
        }
        // Earlier sizing refusal stays canonical in the pending input, no cause
        // cloning/reformatting or second measurement to renew the attempt.
        if !matches!(transcript.creation_layout(), Some(Ok(_))) {
            return Err(ArchiveError::Identity);
        }
        transcript.move_producer_into(&mut self.producer)?;
        let cfg = self
            .configuration
            .take()
            .expect("checked factory configuration");
        let producer = self
            .producer
            .take()
            .expect("checked original producer transfer");
        // Pure concrete construction. Install the driver root BEFORE the next
        // fallible association check or the source unwind seam. No Box/task.
        self.owner = Some(ClaudeDriver::original_owner(cfg, producer));
        after_owner();
        self.owner
            .as_mut()
            .expect("installed concrete factory root")
            .attach_pending_transcript(&mut self.transcript)
    }

    pub(super) fn assembly_result(&self) -> Option<&Result<(), ArchiveError>> {
        self.first_assembly.as_ref()
    }

    #[cfg(test)]
    pub(super) fn assemble_unwind_for_source_control(
        &mut self,
        after_owner: impl FnOnce(),
    ) -> Result<(), ArchiveError> {
        self.assemble_boundary(after_owner)
    }

    #[cfg(test)]
    pub(super) fn owner_for_source_control(&mut self) -> Option<&mut OriginalClaudeOwner> {
        self.owner.as_mut()
    }

    #[cfg(test)]
    pub(super) fn retained_configuration_for_source_control(&self) -> Option<&ClaudeConfig> {
        self.configuration.as_ref()
    }

    #[cfg(test)]
    pub(super) fn transcript_for_source_control(
        &self,
    ) -> Option<&crate::transcript::PendingTranscriptInputs> {
        self.transcript.as_ref().or_else(|| {
            self.owner
                .as_ref()
                .and_then(|owner| owner.pending_transcript_for_source_control())
        })
    }
}

#[allow(dead_code)] // Private SOURCE assembly; public facade/full fit remains held.
pub(crate) struct OriginalClaudeCaller {
    owner: OriginalClaudeOwner,
    factory_assembly: Option<Result<(), ArchiveError>>,
    original_creation_started: bool,
    original_creation_allowance: Option<Allowance>,
    original_creation_result: Option<Result<Operation, ArchiveError>>,
    context_derivation_started: bool,
    context_derivation_result: Option<Result<(), ArchiveError>>,
    stderr_lines: Option<super::PendingStderrLines>,
    stderr_lines_layout: Option<Result<std::alloc::Layout, ArchiveError>>,
    stdout_fifo: Option<super::frame_queue::PendingOwnedChannel>,
    stdout_fifo_layout: Option<Result<super::frame_queue::OwnedCreationLayouts, ArchiveError>>,
    physical_storage: Option<super::child_pending::PendingPhysicalChild>,
    physical_storage_layout:
        Option<Result<super::child_pending::PhysicalStorageLayouts, ArchiveError>>,
    known_creation_started: bool,
    known_creation_layout: Option<Result<KnownCreationLayout, ArchiveError>>,
    original: Option<Operation>,
    pending_context: Option<HostDriverContext>,
    binding_started: bool,
    binding_result: Option<Result<(), ArchiveError>>,
    first_erasure_refusal: Option<ArchiveError>,
    start: Option<Result<SessionInfo, DriverError>>,
    turn: Option<Result<TurnOutcome, DriverError>>,
    cleanup: Option<Result<(), DriverError>>,
    captured: Option<TurnOutcome>,
    first_capture_refusal: Option<ArchiveError>,
}

/// References and original attempt markers only, never cloned body/projection
/// wire data or a positive native/settlement witness from an empty result slot.
#[allow(dead_code)]
pub(super) struct CallerResults<'a> {
    pub(super) factory_assembly: Option<&'a Result<(), ArchiveError>>,
    pub(super) original_creation: Option<&'a Result<Operation, ArchiveError>>,
    pub(super) context_derivation: Option<&'a Result<(), ArchiveError>>,
    pub(super) binding: Option<&'a Result<(), ArchiveError>>,
    pub(super) erasure_refusal: Option<&'a ArchiveError>,
    pub(super) original: Result<ControllerResults<'a>, ArchiveError>,
    pub(super) pending_context: Option<&'a HostDriverContext>,
    pub(super) start: Option<&'a Result<SessionInfo, DriverError>>,
    pub(super) turn: Option<&'a Result<TurnOutcome, DriverError>>,
    pub(super) cleanup: Option<&'a Result<(), DriverError>>,
    pub(super) captured: Option<&'a TurnOutcome>,
    pub(super) capture_refusal: Option<&'a ArchiveError>,
}

#[allow(dead_code)]
impl OriginalClaudeCaller {
    pub(super) fn new(cfg: ClaudeConfig, sink: TranscriptSink) -> Self {
        Self {
            owner: ClaudeDriver::original_owner(cfg, sink),
            factory_assembly: None,
            original_creation_started: false,
            original_creation_allowance: None,
            original_creation_result: None,
            context_derivation_started: false,
            context_derivation_result: None,
            stderr_lines: None,
            stderr_lines_layout: None,
            stdout_fifo: None,
            stdout_fifo_layout: None,
            physical_storage: None,
            physical_storage_layout: None,
            known_creation_started: false,
            known_creation_layout: None,
            original: None,
            pending_context: None,
            binding_started: false,
            binding_result: None,
            first_erasure_refusal: None,
            start: None,
            turn: None,
            cleanup: None,
            captured: None,
            first_capture_refusal: None,
        }
    }

    /// Move the SAME assembled concrete root and its assembly receipt into the
    /// caller, preserving pending worker/store/layout ownership in that root.
    /// Refusal returns the whole factory; this path still cannot bind/admit.
    pub(super) fn from_pending_factory(
        mut factory: PendingClaudeFactory,
    ) -> Result<Self, (ArchiveError, PendingClaudeFactory)> {
        if !matches!(factory.first_assembly, Some(Ok(())))
            || factory.owner.is_none()
            || factory.configuration.is_some()
            || factory.transcript.is_some()
            || factory.producer.is_some()
        {
            return Err((ArchiveError::Identity, factory));
        }
        let owner = factory.owner.take().expect("checked original factory root");
        let factory_assembly = factory.first_assembly.take();
        Ok(Self {
            owner,
            factory_assembly,
            original_creation_started: false,
            original_creation_allowance: None,
            original_creation_result: None,
            context_derivation_started: false,
            context_derivation_result: None,
            stderr_lines: None,
            stderr_lines_layout: None,
            stdout_fifo: None,
            stdout_fifo_layout: None,
            physical_storage: None,
            physical_storage_layout: None,
            known_creation_started: false,
            known_creation_layout: None,
            original: None,
            pending_context: None,
            binding_started: false,
            binding_result: None,
            first_erasure_refusal: None,
            start: None,
            turn: None,
            cleanup: None,
            captured: None,
            first_capture_refusal: None,
        })
    }

    /// ONE original creation attempt. The actual typed Result moves into this
    /// SAME factory/caller root before any fixed observation. An interrupted
    /// attempt remains unfinished; neither a fresh allowance nor retry renews it.
    pub(crate) fn attempt_original_creation(
        &mut self,
        archive: &CampaignArchive,
        allowance: Allowance,
    ) -> Result<bool, ArchiveError> {
        if self.original_creation_started {
            return Err(ArchiveError::Identity);
        }
        self.original_creation_started = true;
        self.original_creation_allowance = Some(allowance);
        let result = self.incomplete_original_creation(archive, allowance);
        self.original_creation_result = Some(result); // Canonical first MOVE.
        if matches!(self.original_creation_result, Some(Ok(_))) {
            Ok(true)
        } else {
            Err(ArchiveError::Identity)
        }
    }

    fn incomplete_original_creation(
        &self,
        _archive: &CampaignArchive,
        allowance: Allowance,
    ) -> Result<Operation, ArchiveError> {
        allowance.check()?;
        if self.original.is_some()
            || self.binding_started
            || self.pending_context.is_some()
            || !matches!(self.factory_assembly, Some(Ok(())))
        {
            return Err(ArchiveError::Identity);
        }
        // Missing or refused descriptor NEVER admits an Operation. Preserve any
        // actual first sizing cause in the original owner slot, no cause copy.
        self.owner
            .owner_layout_bytes()
            .ok_or(ArchiveError::Fit)?
            .as_ref()
            .map_err(|_| ArchiveError::Identity)?;
        // Refusal-only SOURCE boundary: partial snapshots/descriptor staging
        // do NOT certify Operation-owned queue/buffer/library allocations or
        // external growing storage. Actual Server producer is not adopted here.
        // Shared campaign service graphs stay outside this cap under m67.
        // No successful creation path exists yet. Refuse BEFORE plan/UUID
        // creation, Operation allocation,
        // meter admission, boxing/binding or effects; NEVER call the legacy path.
        Err(ArchiveError::Fit)
    }

    /// No external Operation ingress: derive only from THIS actual canonical
    /// creation return. Sticky result is independent of binding/erasure proof.
    fn derive_created_context_once(&mut self) -> Result<(), ArchiveError> {
        if self.context_derivation_started {
            return Err(ArchiveError::Identity);
        }
        self.context_derivation_started = true;
        let result = self.derive_created_context();
        self.context_derivation_result = Some(result); // Actual first MOVE.
        if matches!(self.context_derivation_result, Some(Ok(()))) {
            Ok(())
        } else {
            Err(ArchiveError::Identity)
        }
    }

    fn derive_created_context(&mut self) -> Result<(), ArchiveError> {
        if !self.original_creation_started
            || !self.known_creation_started
            || !matches!(self.known_creation_layout, Some(Ok(_)))
            || !matches!(self.stdout_fifo_layout, Some(Ok(_)))
            || !matches!(self.physical_storage_layout, Some(Ok(_)))
            || !matches!(self.factory_assembly, Some(Ok(())))
            || self.binding_started
            || self.original.is_some()
            || self.pending_context.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        let original = self
            .original_creation_result
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .ok_or(ArchiveError::Identity)?;
        if !self
            .original_creation_allowance
            .is_some_and(|allowance| allowance.deadline() == original.allowance().deadline())
        {
            return Err(ArchiveError::Identity);
        }
        self.owner.derive_created_context(original)?;
        self.stdout_fifo
            .as_mut()
            .ok_or(ArchiveError::Identity)?
            .bind_original(original)?;
        self.physical_storage
            .as_mut()
            .ok_or(ArchiveError::Identity)?
            .bind_original(original)
    }

    fn stage_physical_storage(
        &mut self,
        candidate: &mut Option<super::child_pending::PendingPhysicalChild>,
    ) -> Result<(), ArchiveError> {
        if self.original_creation_started
            || self.known_creation_started
            || self.physical_storage.is_some()
            || self.physical_storage_layout.is_some()
            || candidate.is_none()
        {
            return Err(ArchiveError::Identity);
        }
        self.physical_storage = candidate.take(); // WHOLE owner BEFORE fallible layout.
        let result = (|| {
            let ring_layout = *self
                .stderr_lines_layout
                .as_ref()
                .ok_or(ArchiveError::Identity)?
                .as_ref()
                .map_err(|_| ArchiveError::Identity)?;
            let physical = self
                .physical_storage
                .as_mut()
                .expect("retained physical storage");
            physical.stage_stderr_ring(&mut self.stderr_lines, ring_layout)?;
            physical.creation_layout()
        })();
        self.physical_storage_layout = Some(result); // Canonical first result, no replacement.
        if matches!(self.physical_storage_layout, Some(Ok(_))) {
            Ok(())
        } else {
            Err(ArchiveError::Identity)
        }
    }

    fn stage_stdout_fifo(
        &mut self,
        candidate: &mut Option<super::frame_queue::PendingOwnedChannel>,
    ) -> Result<(), ArchiveError> {
        if self.original_creation_started
            || self.known_creation_started
            || self.stdout_fifo.is_some()
            || self.stdout_fifo_layout.is_some()
            || candidate.is_none()
        {
            return Err(ArchiveError::Identity);
        }
        // WHOLE owner precedes the first fallible sizing result. No endpoint
        // escapes and a refusal cannot install a replacement on a later call.
        self.stdout_fifo = candidate.take();
        let result = self
            .stdout_fifo
            .as_ref()
            .expect("retained stdout FIFO")
            .creation_layout();
        self.stdout_fifo_layout = Some(result);
        if matches!(self.stdout_fifo_layout, Some(Ok(_))) {
            Ok(())
        } else {
            Err(ArchiveError::Identity)
        }
    }

    fn stage_stderr_lines(
        &mut self,
        candidate: &mut Option<super::PendingStderrLines>,
    ) -> Result<(), ArchiveError> {
        if self.original_creation_started
            || self.known_creation_started
            || self.stderr_lines.is_some()
            || self.stderr_lines_layout.is_some()
            || candidate.is_none()
        {
            return Err(ArchiveError::Identity);
        }
        self.stderr_lines = candidate.take();
        let result = self
            .stderr_lines
            .as_ref()
            .expect("retained stderr backing")
            .creation_layout();
        self.stderr_lines_layout = Some(result); // Actual first MOVE before observation.
        if matches!(self.stderr_lines_layout, Some(Ok(_))) {
            Ok(())
        } else {
            Err(ArchiveError::Identity)
        }
    }

    pub(super) fn original_creation_result(&self) -> Option<&Result<Operation, ArchiveError>> {
        self.original_creation_result.as_ref()
    }

    pub(super) fn original_creation_allowance(&self) -> Option<Allowance> {
        self.original_creation_allowance
    }

    pub(super) fn stage_creation_context(
        &mut self,
        candidate: &mut Option<PendingContextInputs>,
    ) -> Result<(), ArchiveError> {
        if self.original_creation_started
            || self.known_creation_started
            || self.binding_started
            || self.original.is_some()
            || self.pending_context.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        self.owner.stage_creation_context(candidate)
    }

    pub(super) fn stage_owner_layout(
        &mut self,
        candidate: &mut Option<crate::host::BoundHostOperationLayout>,
    ) -> Result<(), ArchiveError> {
        if self.original_creation_started
            || self.known_creation_started
            || self.binding_started
            || self.original.is_some()
            || self.pending_context.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        self.owner.stage_owner_layout(candidate)
    }

    /// Record the first partial inventory before fixed observation. No original
    /// Operation/Box/binding/effects are permitted by success of this step.
    pub(super) fn measure_known_creation(&mut self) -> Result<(), ArchiveError> {
        self.measure_known_creation_boundary(crate::operation::OPERATION_BYTES)
    }

    fn measure_known_creation_boundary(&mut self, cap: usize) -> Result<(), ArchiveError> {
        if self.original_creation_started
            || self.known_creation_started
            || self.binding_started
            || self.original.is_some()
            || self.pending_context.is_some()
        {
            return Err(ArchiveError::Identity);
        }
        self.known_creation_started = true; // Unwind is unfinished, never retry authority.
        let result = self.derive_known_creation(cap);
        self.known_creation_layout = Some(result); // Canonical first MOVE.
        if matches!(self.known_creation_layout, Some(Ok(_))) {
            Ok(())
        } else {
            Err(ArchiveError::Identity)
        }
    }

    fn derive_known_creation(&mut self, cap: usize) -> Result<KnownCreationLayout, ArchiveError> {
        // Preserve earlier actual layout errors in their root slots. A failed
        // prerequisite is an Identity observation here, not a copied cause.
        let configuration_payload_bytes = self
            .owner
            .configuration_layout()
            .as_ref()
            .map_err(|_| ArchiveError::Identity)?
            .payload_bytes;
        let context_payload_bytes = self
            .owner
            .creation_context_layout()
            .ok_or(ArchiveError::Identity)?
            .as_ref()
            .map_err(|_| ArchiveError::Identity)?
            .payload_bytes;
        let admission_body_bytes = self
            .owner
            .creation_context_layout()
            .ok_or(ArchiveError::Identity)?
            .as_ref()
            .map_err(|_| ArchiveError::Identity)?
            .admission_body
            .size();
        let transcript_referenced_bytes = self
            .owner
            .transcript_layout()
            .as_ref()
            .map_err(|_| ArchiveError::Identity)?
            .referenced_bytes;
        let pending_transcript_bytes = match self.owner.pending_transcript_layout() {
            None => 0, // Earlier sink-only SOURCE constructor; not complete admission.
            Some(Ok(layout)) => layout.known_bytes,
            Some(Err(_)) => return Err(ArchiveError::Identity), // Actual cause retained in root.
        };
        let owner_persistent_bytes = match self.owner.owner_layout_bytes() {
            None => 0, // Partial SOURCE inventory only; original entry refuses missing.
            Some(Ok(bytes)) => *bytes,
            Some(Err(_)) => return Err(ArchiveError::Identity),
        };
        let controllers = self.owner.creation_frame_layouts()?;
        let physical_storage = match &self.physical_storage_layout {
            Some(Ok(layout)) => Some(layout),
            Some(Err(_)) => return Err(ArchiveError::Identity),
            None if self.factory_assembly.is_none() => None, // Earlier PARTIAL source fixture.
            None => return Err(ArchiveError::Identity),
        };
        let child = ChildCreationLayouts::derive(physical_storage)?;
        let physical_storage_counter_bytes = match physical_storage {
            Some(layout) => layout
                .arc_counter_bytes
                .checked_add(layout.reader_failures_counter_bytes)
                .and_then(|bytes| bytes.checked_add(layout.capture_counter_bytes))
                .and_then(|bytes| bytes.checked_add(layout.stderr_tail_counter_bytes))
                .and_then(|bytes| bytes.checked_add(layout.pipe_counter_bytes))
                .ok_or(ArchiveError::Fit)?,
            None => 0,
        };
        // The old eight-body inventory does NOT include these two actual
        // canonical pipe destinations. Their Arc handles are already inline.
        // Library/OS-owned children and task frames remain separate/open.
        let physical_pipe_body_bytes = match physical_storage {
            Some(layout) => layout
                .pipe_bodies
                .iter()
                .try_fold(0usize, |bytes, body| bytes.checked_add(body.size()))
                .ok_or(ArchiveError::Fit)?,
            None => 0,
        };
        let stderr_lines_bytes = match &self.stderr_lines_layout {
            Some(Ok(layout)) => layout.size(),
            Some(Err(_)) => return Err(ArchiveError::Identity),
            None if self.factory_assembly.is_none() => 0, // Earlier inert source fixture only.
            None => return Err(ArchiveError::Identity),
        };
        let caller = std::alloc::Layout::for_value(self).pad_to_align();
        let stdout_fifo_bytes = match &self.stdout_fifo_layout {
            Some(Ok(layout)) => layout.referenced_bytes,
            Some(Err(_)) => return Err(ArchiveError::Identity),
            None => 0, // Earlier partial SOURCE fixtures only, NEVER a complete-F admission.
        };
        let stdout_receive_bytes = match &self.stdout_fifo_layout {
            Some(Ok(layout)) => layout.receive_payload_bytes,
            Some(Err(_)) => return Err(ArchiveError::Identity),
            None => 0, // Same earlier partial inventory, never an actual original Fit.
        };
        // Root/caller concrete headers already include the owned configuration,
        // context and the new Result slot. Add their actual heap capacities
        // once. Some fixed headers overlap conservative prior inventory; no
        // subtraction or cap increase is used to manufacture a positive fit.
        let fixed_bytes = RetainedArchive::<ClaudeDriver>::fixed_bytes()?
            .checked_add(Self::caller_fixed_bytes()?)
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<(
                    KnownCreationLayout,
                    Result<KnownCreationLayout, ArchiveError>,
                    ChildCreationLayouts,
                    &mut Self,
                    std::alloc::Layout,
                    usize,
                    usize,
                    usize,
                    usize,
                    usize,
                    Result<(), ArchiveError>,
                    usize, // Actual pipe-body subtotal local, not another heap body.
                )>())
            })
            .ok_or(ArchiveError::Fit)?;
        let known_bytes = fixed_bytes
            .checked_add(controllers.overlap_bytes)
            .and_then(|bytes| bytes.checked_add(child.referenced_headers_bytes))
            .and_then(|bytes| bytes.checked_add(configuration_payload_bytes))
            .and_then(|bytes| bytes.checked_add(context_payload_bytes))
            .and_then(|bytes| bytes.checked_add(admission_body_bytes))
            .and_then(|bytes| bytes.checked_add(transcript_referenced_bytes))
            .and_then(|bytes| bytes.checked_add(pending_transcript_bytes))
            .and_then(|bytes| bytes.checked_add(owner_persistent_bytes))
            .and_then(|bytes| bytes.checked_add(stderr_lines_bytes))
            .and_then(|bytes| bytes.checked_add(stdout_fifo_bytes))
            .and_then(|bytes| bytes.checked_add(stdout_receive_bytes))
            .and_then(|bytes| bytes.checked_add(physical_storage_counter_bytes))
            .and_then(|bytes| bytes.checked_add(physical_pipe_body_bytes))
            .ok_or(ArchiveError::Fit)?;
        if known_bytes > cap {
            return Err(ArchiveError::Fit);
        }
        Ok(KnownCreationLayout {
            caller,
            controllers,
            child,
            fixed_bytes,
            configuration_payload_bytes,
            context_payload_bytes,
            admission_body_bytes,
            transcript_referenced_bytes,
            pending_transcript_bytes,
            owner_persistent_bytes,
            stderr_lines_bytes,
            stdout_fifo_bytes,
            stdout_receive_bytes,
            physical_storage_counter_bytes,
            physical_pipe_body_bytes,
            known_bytes,
        })
    }

    pub(super) fn known_creation_layout(
        &self,
    ) -> Option<&Result<KnownCreationLayout, ArchiveError>> {
        self.known_creation_layout.as_ref()
    }

    pub(super) fn factory_assembly_result(&self) -> Option<&Result<(), ArchiveError>> {
        self.factory_assembly.as_ref()
    }

    #[cfg(test)]
    pub(super) fn measure_known_creation_with_cap_for_source_control(
        &mut self,
        cap: usize,
    ) -> Result<(), ArchiveError> {
        self.measure_known_creation_boundary(cap)
    }

    /// Refuse repeats without consuming their candidate. First input moves into
    /// caller storage BEFORE invoking the owned root, retaining early refusal.
    pub(super) fn bind(
        &mut self,
        candidate: &mut Option<HostDriverContext>,
    ) -> Result<(), ArchiveError> {
        // Strict sequencing is creation FIRST, then binding of that exact
        // returned Operation. A started/refused/interrupted creation cannot
        // fall through to the earlier source fixture's binding path. Complete
        // original creation is still refusal-only; partial layout success is
        // never enough to make this branch reachable.
        let creation_matches = if self.original_creation_started {
            matches!(self.factory_assembly, Some(Ok(())))
                && self.known_creation_started
                && matches!(self.known_creation_layout, Some(Ok(_)))
                && self
                    .original_creation_result
                    .as_ref()
                    .is_some_and(|result| match result {
                        Ok(created) => candidate.as_ref().is_some_and(|context| {
                            created.same_instance(&context.operation)
                                && self.original_creation_allowance.is_some_and(|allowance| {
                                    created.allowance().deadline() == allowance.deadline()
                                })
                        }),
                        Err(_) => false,
                    })
        } else {
            // Preserve the existing unassembled source/fixture path. It makes
            // no original-F claim and cannot bypass a factory/creation attempt.
            self.factory_assembly.is_none()
                && !self.known_creation_started
                && self.known_creation_layout.is_none()
                && self.original_creation_result.is_none()
                && self.original_creation_allowance.is_none()
        };
        if !creation_matches
            || self.binding_started
            || self.original.is_some()
            || self.pending_context.is_some()
            || self.binding_result.is_some()
            || candidate.is_none()
        {
            return Err(ArchiveError::Identity);
        }
        self.original = Some(
            candidate
                .as_ref()
                .expect("checked caller context")
                .operation
                .clone(),
        );
        self.pending_context = candidate.take();
        self.binding_started = true;
        // Check the caller's own creation-reserved fixed headers BEFORE the
        // concrete root can box/bind. No new charge or input copy occurs here.
        let result = Self::check_fixed_storage().and_then(|()| {
            self.owner
                .bind_original_context_from_slot(&mut self.pending_context)
        });
        self.binding_result = Some(result); // MOVE before refusal projection.
        if matches!(self.binding_result, Some(Ok(()))) {
            Ok(())
        } else {
            Err(ArchiveError::Identity)
        }
    }

    fn check_bound(&self, expected: &Operation) -> Result<(), ArchiveError> {
        if self.binding_started
            && matches!(self.binding_result, Some(Ok(())))
            && self.pending_context.is_none()
            && self
                .original
                .as_ref()
                .is_some_and(|original| original.same_instance(expected))
        {
            Ok(())
        } else {
            Err(ArchiveError::Identity)
        }
    }

    /// The private trait facade cannot turn a legacy/source-fixture binding or
    /// partial layout record into original admission. There is currently NO
    /// successful original creation producer, so every retained entry refuses
    /// here without boxing, consuming an attempt or touching a destination.
    /// Recovery uses the same immutable creation identity without reopening
    /// the allowance: an expired meter must not discard a canonical return.
    fn check_retained_trait_original(&self, expected: &Operation) -> Result<(), DriverError> {
        let admitted = self.original_creation_started
            && matches!(self.factory_assembly, Some(Ok(())))
            && self.known_creation_started
            && matches!(self.known_creation_layout, Some(Ok(_)))
            && self
                .original_creation_result
                .as_ref()
                .is_some_and(|result| match result {
                    Ok(created) => {
                        created.same_instance(expected)
                            && self.original_creation_allowance.is_some_and(|allowance| {
                                created.allowance().deadline() == allowance.deadline()
                            })
                    }
                    Err(_) => false,
                });
        if admitted && self.check_bound(expected).is_ok() {
            Ok(())
        } else {
            Err(DriverError::Host(crate::host::HostRefusal::Unknown))
        }
    }

    /// Count actual private caller storage and return/argument headers in the
    /// original fixed workspace BEFORE delegating to root's concrete prototype
    /// check and Box::pin. FULL growing/outer/library fit remains unfinished.
    fn check_fixed_storage() -> Result<(), ArchiveError> {
        let required = Self::caller_fixed_bytes().and_then(|caller| {
            RetainedArchive::<ClaudeDriver>::fixed_bytes()?
                .checked_add(caller)
                .ok_or(ArchiveError::Fit)
        });
        required.and_then(|bytes| {
            crate::failed_evidence::check_retained_publication_workspace::<ClaudeDriver>(bytes)
        })
    }

    /// This portion excludes the root's separately counted storage. Pass it
    /// into the actual prototype-erasure check so both are checked together.
    fn caller_fixed_bytes() -> Result<usize, ArchiveError> {
        std::mem::size_of::<Self>()
            // Prospective opaque pending/created/failure MOVE headers. These
            // type facts remain partial; they cannot certify full original F.
            .checked_add(std::mem::size_of::<(
                OpaqueOriginalInputs,
                OriginalClaude<ClaudeConfig>,
                OriginalClaude<dyn super::SeatDriver>,
                OriginalClaude<ArchiveError>,
                std::sync::Arc<dyn crate::transcript::TranscriptStore>,
                std::sync::Arc<str>,
                crate::transcript::PendingTranscriptInputs,
            )>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<CallerResults<'_>>()))
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    Result<Self, (ArchiveError, PendingClaudeFactory)>,
                >())
            })
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<(
                    Result<Operation, ArchiveError>,
                    &CampaignArchive,
                    &mut Self,
                    Allowance,
                    Result<bool, ArchiveError>,
                )>())
            })
            .ok_or(ArchiveError::Fit)
            .and_then(|bytes| {
                bytes
                    .checked_add(std::mem::size_of::<(
                        Result<RetainedDriverFuture<'_>, DriverError>,
                        Box<Self>,
                        Box<dyn super::SeatDriver>,
                        Result<bool, DriverError>,
                        &Operation,
                        Option<&str>,
                        &str,
                        std::time::Duration,
                        &mut Self,
                        usize,
                    )>())
                    .ok_or(ArchiveError::Fit)
            })
            .and_then(|bytes| {
                bytes
                    .checked_add(std::mem::size_of::<(
                        Result<bool, ArchiveError>,
                        &Operation,
                        &Operation,
                        &mut Option<TurnOutcome>,
                        &mut Self,
                    )>())
                    .ok_or(ArchiveError::Fit)
            })
            // The external destination is distinct from our retained slots.
            // Count its actual headers as well as forwarding/recovery borrows.
            // Full external caller frame/growing fit remains an adoption gate.
            .and_then(|bytes| {
                bytes
                    .checked_add(std::mem::size_of::<(
                        Option<Result<SessionInfo, DriverError>>,
                        Option<Result<TurnOutcome, DriverError>>,
                        Option<Result<(), DriverError>>,
                        Option<TurnOutcome>,
                        &Operation,
                        &mut Option<Result<SessionInfo, DriverError>>,
                        &mut Option<Result<TurnOutcome, DriverError>>,
                        &mut Option<Result<(), DriverError>>,
                        &mut Option<TurnOutcome>,
                        Result<bool, DriverError>,
                        Result<bool, ArchiveError>,
                    )>())
                    .ok_or(ArchiveError::Fit)
            })
    }

    fn check_erasure(&mut self, expected: &Operation) -> Result<usize, ArchiveError> {
        self.check_bound(expected)?;
        if self.first_erasure_refusal.is_some() {
            return Err(ArchiveError::Identity);
        }
        if let Err(error) = Self::check_fixed_storage() {
            self.first_erasure_refusal = Some(error); // First actual fixed-fit refusal retained.
            return Err(ArchiveError::Identity);
        }
        Self::caller_fixed_bytes()
    }

    pub(super) fn start_future<'a>(
        &'a mut self,
        expected: &'a Operation,
        resume: Option<&'a str>,
    ) -> Result<RetainedDriverFuture<'a>, DriverError> {
        let caller_bytes = self
            .check_erasure(expected)
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))?;
        let original = self.original.as_ref().expect("checked original caller");
        self.owner.start_retained_observation(
            expected,
            resume,
            original,
            &mut self.start,
            caller_bytes,
        )
    }

    pub(super) fn turn_future<'a>(
        &'a mut self,
        expected: &'a Operation,
        prompt: &'a str,
        limit: std::time::Duration,
    ) -> Result<RetainedDriverFuture<'a>, DriverError> {
        let caller_bytes = self
            .check_erasure(expected)
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))?;
        let original = self.original.as_ref().expect("checked original caller");
        self.owner.turn_retained_observation(
            expected,
            prompt,
            limit,
            original,
            &mut self.turn,
            caller_bytes,
        )
    }

    pub(super) fn cleanup_future<'a>(
        &'a mut self,
        expected: &'a Operation,
    ) -> Result<RetainedDriverFuture<'a>, DriverError> {
        let caller_bytes = self
            .check_erasure(expected)
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))?;
        let original = self.original.as_ref().expect("checked original caller");
        self.owner
            .cleanup_retained_observation(expected, original, &mut self.cleanup, caller_bytes)
    }

    fn check_external_destination(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        empty: bool,
    ) -> Result<usize, DriverError> {
        // Refusal cannot consume an attempt or mutate either result owner.
        if !expected.same_instance(destination_original) || !empty {
            return Err(DriverError::Host(crate::host::HostRefusal::Unknown));
        }
        self.check_erasure(expected)
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))
    }

    /// Private operator forwarding to the trusted caller's ORIGINAL empty,
    /// creation-admitted slot. No intermediate result body or second future.
    /// Public trait/factory adoption still needs FULL external/growing fit.
    pub(super) fn start_into<'a>(
        &'a mut self,
        expected: &'a Operation,
        resume: Option<&'a str>,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<SessionInfo, DriverError>>,
    ) -> Result<RetainedDriverFuture<'a>, DriverError> {
        let bytes =
            self.check_external_destination(expected, destination_original, destination.is_none())?;
        self.owner.start_retained_observation(
            expected,
            resume,
            destination_original,
            destination,
            bytes,
        )
    }

    pub(super) fn turn_into<'a>(
        &'a mut self,
        expected: &'a Operation,
        prompt: &'a str,
        limit: std::time::Duration,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<TurnOutcome, DriverError>>,
    ) -> Result<RetainedDriverFuture<'a>, DriverError> {
        let bytes =
            self.check_external_destination(expected, destination_original, destination.is_none())?;
        self.owner.turn_retained_observation(
            expected,
            prompt,
            limit,
            destination_original,
            destination,
            bytes,
        )
    }

    pub(super) fn cleanup_into<'a>(
        &'a mut self,
        expected: &'a Operation,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<(), DriverError>>,
    ) -> Result<RetainedDriverFuture<'a>, DriverError> {
        let bytes =
            self.check_external_destination(expected, destination_original, destination.is_none())?;
        self.owner
            .cleanup_retained_observation(expected, destination_original, destination, bytes)
    }

    /// Recovery transfers only an already retained return from THIS caller or
    /// its SAME root. No dispatch/retry/box/poll/legacy fallback. All checks
    /// precede either direct MOVE, preserving bodies on refused/lost ACK.
    pub(super) fn handoff_start_into(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<Result<SessionInfo, DriverError>>,
    ) -> Result<bool, DriverError> {
        self.check_external_destination(expected, destination_original, destination.is_none())?;
        if self.start.is_some() {
            *destination = self.start.take();
            return Ok(true);
        }
        self.owner
            .handoff_start_driver_result(expected, destination_original, destination)
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))
    }

    pub(super) fn handoff_turn_into(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<Result<TurnOutcome, DriverError>>,
    ) -> Result<bool, DriverError> {
        self.check_external_destination(expected, destination_original, destination.is_none())?;
        if self.turn.is_some() {
            *destination = self.turn.take();
            return Ok(true);
        }
        self.owner
            .handoff_turn_driver_result(expected, destination_original, destination)
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))
    }

    pub(super) fn handoff_cleanup_into(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<Result<(), DriverError>>,
    ) -> Result<bool, DriverError> {
        self.check_external_destination(expected, destination_original, destination.is_none())?;
        if self.cleanup.is_some() {
            *destination = self.cleanup.take();
            return Ok(true);
        }
        self.owner
            .handoff_cleanup_driver_result(expected, destination_original, destination)
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))
    }

    /// Separately retained terminal capture, never a replacement callback
    /// Result. Existing inner physical-owner checks apply before root transfer;
    /// an earlier body already in this caller can MOVE directly after checks.
    pub(super) fn move_captured_into(
        &mut self,
        expected: &Operation,
        destination_original: &Operation,
        destination: &mut Option<TurnOutcome>,
    ) -> Result<bool, DriverError> {
        if !expected.same_instance(destination_original) || destination.is_some() {
            return Err(DriverError::Host(crate::host::HostRefusal::Unknown));
        }
        self.check_bound(expected)
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Unknown))?;
        if self.first_capture_refusal.is_some() {
            return Err(DriverError::Host(crate::host::HostRefusal::Unknown));
        }
        let checked = Self::check_fixed_storage();
        let result = match checked {
            Err(cause) => Err(cause),
            Ok(()) if self.captured.is_some() => {
                *destination = self.captured.take();
                return Ok(true);
            }
            Ok(()) => self.owner.handoff_captured_before_archive(
                expected,
                destination_original,
                destination,
            ),
        };
        match result {
            Ok(moved) => Ok(moved),
            Err(cause) => {
                self.first_capture_refusal = Some(cause);
                Err(DriverError::Host(crate::host::HostRefusal::Unknown))
            }
        }
    }

    /// Recover only an already canonical first callback return. These methods
    /// never dispatch/box/poll a future, restart an attempt or format an error.
    /// The SAME root keeps ArchiveError causes; only success/DriverError bodies
    /// MOVE into the caller's typed EMPTY slot. Combined headers are checked
    /// first; a first caller fit refusal stays in its original refusal slot.
    pub(super) fn handoff_start_returned(
        &mut self,
        expected: &Operation,
    ) -> Result<bool, ArchiveError> {
        self.check_erasure(expected)?;
        let original = self.original.as_ref().expect("checked original caller");
        self.owner
            .handoff_start_driver_result(expected, original, &mut self.start)
    }

    pub(super) fn handoff_turn_returned(
        &mut self,
        expected: &Operation,
    ) -> Result<bool, ArchiveError> {
        self.check_erasure(expected)?;
        let original = self.original.as_ref().expect("checked original caller");
        self.owner
            .handoff_turn_driver_result(expected, original, &mut self.turn)
    }

    pub(super) fn handoff_cleanup_returned(
        &mut self,
        expected: &Operation,
    ) -> Result<bool, ArchiveError> {
        self.check_erasure(expected)?;
        let original = self.original.as_ref().expect("checked original caller");
        self.owner
            .handoff_cleanup_driver_result(expected, original, &mut self.cleanup)
    }

    /// Separate canonical captured body, never the startup/turn/cleanup Result.
    /// Check SAME binding, empty destination and combined fixed storage before
    /// the root's physical-owner checks and direct MOVE. No Operation::check:
    /// later overflow must not discard an earlier captured body. FULL growing/
    /// library/outer admission and public routing remain unfinished/closed.
    pub(super) fn move_captured(&mut self, expected: &Operation) -> Result<bool, ArchiveError> {
        self.check_bound(expected)?;
        if self.captured.is_some() || self.first_capture_refusal.is_some() {
            return Err(ArchiveError::Identity);
        }
        let original = self.original.as_ref().expect("checked original caller");
        let result = Self::check_fixed_storage().and_then(|()| {
            self.owner
                .handoff_captured_before_archive(expected, original, &mut self.captured)
        });
        match result {
            Ok(moved) => Ok(moved), // false is absence, never cleanup/settlement.
            Err(cause) => {
                self.first_capture_refusal = Some(cause); // MOVE before fixed observation.
                Err(ArchiveError::Identity)
            }
        }
    }

    pub(super) fn results(
        &mut self,
        expected: &Operation,
    ) -> Result<CallerResults<'_>, ArchiveError> {
        if !self
            .original
            .as_ref()
            .is_some_and(|original| original.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        // A pre-box refusal can leave the inner root unbound. Preserve its
        // actual projection error rather than inventing default attempt flags.
        let original = self.owner.retained_results(expected);
        Ok(CallerResults {
            factory_assembly: self.factory_assembly.as_ref(),
            original_creation: self.original_creation_result.as_ref(),
            context_derivation: self.context_derivation_result.as_ref(),
            binding: self.binding_result.as_ref(),
            erasure_refusal: self.first_erasure_refusal.as_ref(),
            original,
            pending_context: self.pending_context.as_ref(),
            start: self.start.as_ref(),
            turn: self.turn.as_ref(),
            cleanup: self.cleanup.as_ref(),
            captured: self.captured.as_ref(),
            capture_refusal: self.first_capture_refusal.as_ref(),
        })
    }

    /// Storage projection only, not proof of physical joins/census/custody.
    /// After staging, borrow the SAME canonical ring and its stats/read prefix
    /// under unique access, without holding storage guards across callbacks.
    #[allow(dead_code)]
    pub(super) fn visit_staged_stderr(
        &mut self,
        expected: &Operation,
        physical_child: &str,
        sink: &mut impl super::archive_stdout::RecordSink,
    ) -> Result<(), ArchiveError> {
        if !self
            .original
            .as_ref()
            .is_some_and(|original| original.same_instance(expected))
        {
            return Err(ArchiveError::Identity);
        }
        if let Some(physical) = self.physical_storage.as_mut() {
            return physical.visit_stderr_storage(expected, physical_child, sink);
        }
        self.stderr_lines
            .as_ref()
            .ok_or(ArchiveError::Identity)?
            .visit_staged_archive(expected, physical_child, sink)
    }

    #[cfg(test)]
    pub(super) fn creation_untouched_for_source_control(&mut self) -> bool {
        self.original.is_none()
            && self.pending_context.is_none()
            && !self.binding_started
            && self.binding_result.is_none()
            && self.first_erasure_refusal.is_none()
            && self.start.is_none()
            && self.turn.is_none()
            && self.cleanup.is_none()
            && self.captured.is_none()
            && self.first_capture_refusal.is_none()
            && self.owner.creation_untouched_for_source_control()
    }

    #[cfg(test)]
    pub(super) fn root_for_source_control(&mut self) -> &mut OriginalClaudeOwner {
        &mut self.owner
    }

    #[cfg(test)]
    pub(super) fn handoff_start_for_source_control(
        &mut self,
        expected: &Operation,
    ) -> Result<bool, ArchiveError> {
        self.handoff_start_returned(expected)
    }
}

/// Private operator facade over the SAME retained concrete root. No public
/// factory returns this type yet. The FOUR synchronous entries delegate only
/// after genuine original creation/binding; no legacy method is a fallback.
/// Full external/growing/frame fit and lifecycle still gate public adoption.
#[async_trait::async_trait]
impl super::SeatDriver for OriginalClaudeCaller {
    fn kind(&self) -> super::CliKind {
        super::CliKind::Claude
    }

    fn start_retained<'a>(
        &'a mut self,
        original: &'a Operation,
        resume_session: Option<&'a str>,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<SessionInfo, DriverError>>,
    ) -> Result<RetainedDriverFuture<'a>, DriverError> {
        self.check_retained_trait_original(original)?;
        self.start_into(original, resume_session, destination_original, destination)
    }

    fn turn_retained<'a>(
        &'a mut self,
        original: &'a Operation,
        prompt: &'a str,
        limit: std::time::Duration,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<TurnOutcome, DriverError>>,
    ) -> Result<RetainedDriverFuture<'a>, DriverError> {
        self.check_retained_trait_original(original)?;
        self.turn_into(original, prompt, limit, destination_original, destination)
    }

    fn cleanup_retained<'a>(
        &'a mut self,
        original: &'a Operation,
        destination_original: &'a Operation,
        destination: &'a mut Option<Result<(), DriverError>>,
    ) -> Result<RetainedDriverFuture<'a>, DriverError> {
        self.check_retained_trait_original(original)?;
        self.cleanup_into(original, destination_original, destination)
    }

    fn move_captured_retained(
        &mut self,
        original: &Operation,
        destination_original: &Operation,
        destination: &mut Option<TurnOutcome>,
    ) -> Result<bool, DriverError> {
        self.check_retained_trait_original(original)?;
        self.move_captured_into(original, destination_original, destination)
    }

    // The existing ClaudeDriver legacy implementation is unchanged. These
    // private-facade legacy entries refuse; callers must use the retained path.
    async fn start(&mut self, _: Option<&str>) -> Result<SessionInfo, DriverError> {
        Err(DriverError::Host(crate::host::HostRefusal::Unknown))
    }

    async fn run_turn(
        &mut self,
        _: &str,
        _: std::time::Duration,
    ) -> Result<TurnOutcome, DriverError> {
        Err(DriverError::Host(crate::host::HostRefusal::Unknown))
    }

    fn session_id(&self) -> Option<String> {
        None
    }

    fn is_alive(&mut self) -> bool {
        false
    }

    async fn stop(&mut self) -> Result<(), DriverError> {
        Err(DriverError::Host(crate::host::HostRefusal::Unknown))
    }
}

/// Operator-only consuming assembly. Its public phase parameters reuse existing
/// types: ClaudeConfig is pending, dyn SeatDriver is created-attempt, and
/// ArchiveError is failed custody. No public phase type or inner access exists.
/// Production Play adoption remains closed pending the ONE whole joint.
///
/// Pending values cannot erase before creation:
/// ```compile_fail
/// use cna_seats::driver::claude::{ClaudeConfig, OriginalClaude};
/// fn out_of_order(pending: OriginalClaude<ClaudeConfig>) {
///     let _ = pending.erase();
/// }
/// ```
/// Inner storage is not accessible:
/// ```compile_fail
/// use cna_seats::driver::claude::{ClaudeConfig, OriginalClaude};
/// fn inner(pending: OriginalClaude<ClaudeConfig>) {
///     let _ = pending.inputs;
/// }
/// ```
/// Borrowed readback cannot outlive its created owner:
/// ```compile_fail
/// use cna_seats::driver::{SeatDriver, claude::OriginalClaude};
/// fn escape(created: OriginalClaude<dyn SeatDriver>)
///     -> Option<Result<(), &'static cna_seats::failed_evidence::ArchiveError>> {
///     created.readback().1
/// }
/// ```
#[must_use = "retain the original pending/created owner or its failed custody"]
pub struct OriginalClaude<Phase: ?Sized = ClaudeConfig> {
    inputs: OpaqueOriginalInputs,
    phase: std::marker::PhantomData<Phase>,
}

struct OpaqueOriginalInputs {
    configuration: Option<ClaudeConfig>,
    store: Option<std::sync::Arc<dyn crate::transcript::TranscriptStore>>,
    stream: Option<std::sync::Arc<str>>,
    context: Option<PendingContextInputs>,
    descriptor: Option<crate::host::BoundHostOperationLayout>,
    stderr_lines: Option<super::PendingStderrLines>,
    stdout_fifo: Option<super::frame_queue::PendingOwnedChannel>,
    physical_storage: Option<super::child_pending::PendingPhysicalChild>,
    factory: Option<PendingClaudeFactory>,
    caller: Option<OriginalClaudeCaller>,
    constructor_valid: bool,
    preparation: Option<Result<(), ArchiveError>>,
    creation_observation: Option<Result<bool, ArchiveError>>,
}

impl<Phase: ?Sized> std::fmt::Debug for OriginalClaude<Phase> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Only fixed type/state labels: no input, identity, path, error or body.
        let state = if !self.inputs.constructor_valid {
            "constructor_refused"
        } else if self.inputs.creation_observation.is_some() {
            "creation_attempted"
        } else if self.inputs.preparation.is_some() {
            "preparation_attempted"
        } else {
            "pending"
        };
        formatter
            .debug_struct("OriginalClaude")
            .field("state", &state)
            .finish()
    }
}

impl OriginalClaude<ClaudeConfig> {
    /// ONE public constructor: pure BY-VALUE staging, with no queues, task,
    /// service call, I/O, Operation/State, Box, meter or binding construction.
    /// Failure returns the existing error and the WHOLE opaque failed owner.
    pub fn new(
        configuration: ClaudeConfig,
        store: std::sync::Arc<dyn crate::transcript::TranscriptStore>,
        stream: std::sync::Arc<str>,
        admission: std::sync::Arc<dyn crate::host::HostAdmission>,
        identity: crate::host::HostIdentity,
        descriptor: crate::host::BoundHostOperationLayout,
    ) -> Result<Self, (ArchiveError, OriginalClaude<ArchiveError>)> {
        let constructor_valid = configuration.seat == identity.seat
            && identity.operation_id.len() == 36
            && uuid::Uuid::parse_str(&identity.operation_id).is_ok()
            && descriptor.matches_owner(&admission);
        let inputs = OpaqueOriginalInputs {
            configuration: Some(configuration),
            store: Some(store),
            stream: Some(stream),
            context: Some(PendingContextInputs {
                admission,
                identity,
            }),
            descriptor: Some(descriptor),
            stderr_lines: None,
            stdout_fifo: None,
            physical_storage: None,
            factory: None,
            caller: None,
            constructor_valid,
            preparation: None,
            creation_observation: None,
        };
        if constructor_valid {
            Ok(Self {
                inputs,
                phase: std::marker::PhantomData,
            })
        } else {
            Err((
                ArchiveError::Identity,
                OriginalClaude {
                    inputs,
                    phase: std::marker::PhantomData,
                },
            ))
        }
    }

    /// Consume pending ownership exactly ONCE. First actual preparation and
    /// creation returns stay in the SAME owner before any fixed observation.
    /// The complete creation producer is absent, so the current path refuses
    /// BEFORE Operation/State/meter/Box/binding/effects. No external Op accepted.
    pub fn create(
        mut self,
        archive: &CampaignArchive,
        allowance: Allowance,
    ) -> OriginalClaude<dyn super::SeatDriver> {
        let preparation = self.inputs.prepare();
        self.inputs.preparation = Some(preparation);
        if matches!(self.inputs.preparation, Some(Ok(()))) {
            let observation = archive.original_operation(
                self.inputs
                    .caller
                    .as_mut()
                    .expect("prepared original caller"),
                allowance,
            );
            self.inputs.creation_observation = Some(observation);
            if matches!(self.inputs.creation_observation, Some(Ok(true))) {
                // Private derivation records its canonical actual Result first.
                // The fixed returned observation is not binding/activation.
                let _ = self
                    .inputs
                    .caller
                    .as_mut()
                    .expect("retained created caller")
                    .derive_created_context_once();
            }
        }
        OriginalClaude {
            inputs: self.inputs,
            phase: std::marker::PhantomData,
        }
    }
}

impl OpaqueOriginalInputs {
    fn prepare(&mut self) -> Result<(), ArchiveError> {
        // Check the WHOLE staged graph before taking even one body. Otherwise
        // a missing later input could drop configuration on typed refusal.
        if !self.constructor_valid
            || self.factory.is_some()
            || self.caller.is_some()
            || self.configuration.is_none()
            || self.store.is_none()
            || self.stream.is_none()
            || self.context.is_none()
            || self.descriptor.is_none()
        {
            return Err(ArchiveError::Identity);
        }
        if self.stderr_lines.is_none() {
            self.stderr_lines = Some(super::PendingStderrLines::new()?);
        }
        if self.stdout_fifo.is_none() {
            // Still preparation, BEFORE any Operation/State creation or effects.
            // The public constructor merely stages None and allocates no queue.
            self.stdout_fifo = Some(super::frame_queue::PendingOwnedChannel::new());
        }
        if self.physical_storage.is_none() {
            // Actual fixed cells before canonical Operation creation, no process,
            // task, pipe I/O, registry/store callback or runtime binding.
            self.physical_storage = Some(super::child_pending::PendingPhysicalChild::new());
        }
        // HANDLE copies only; original config/store/stream remain in self while
        // pending queues are constructed. No referenced body copy or new Op.
        let store = self.store.as_ref().expect("checked staged store").clone();
        let stream = self.stream.as_ref().expect("checked staged stream").clone();
        let transcript = crate::transcript::PendingTranscriptInputs::new(store, stream);
        let configuration = self
            .configuration
            .take()
            .expect("checked staged configuration");
        self.factory = Some(PendingClaudeFactory::new(configuration, transcript));
        // Factory owns the complete graph BEFORE these original handle drops
        // or any fallible assembly step. They cannot drop the referenced owner.
        self.store = None;
        self.stream = None;
        self.factory
            .as_mut()
            .expect("retained pending factory")
            .assemble()?;
        let factory = self.factory.take().expect("assembled pending factory");
        self.caller = Some(match OriginalClaudeCaller::from_pending_factory(factory) {
            Ok(caller) => caller,
            Err((error, factory)) => {
                self.factory = Some(factory); // Restore the whole refused factory.
                return Err(error);
            }
        });
        let caller = self.caller.as_mut().expect("retained original caller");
        caller.stage_creation_context(&mut self.context)?;
        caller.stage_owner_layout(&mut self.descriptor)?;
        caller.stage_stderr_lines(&mut self.stderr_lines)?;
        caller.stage_stdout_fifo(&mut self.stdout_fifo)?;
        caller.stage_physical_storage(&mut self.physical_storage)?;
        caller.measure_known_creation()
    }
}

impl OriginalClaude<dyn super::SeatDriver> {
    /// ONE public readback: outcome status and shared canonical errors only,
    /// never an Operation handle, body copy or formatter. Ends at erasure.
    pub fn readback(
        &self,
    ) -> (
        &Result<(), ArchiveError>,
        Option<Result<(), &ArchiveError>>,
        Option<&Result<(), ArchiveError>>,
    ) {
        (
            self.inputs
                .preparation
                .as_ref()
                .expect("consumed preparation attempt"),
            self.inputs
                .caller
                .as_ref()
                .and_then(OriginalClaudeCaller::original_creation_result)
                .map(|result| match result {
                    Ok(_) => Ok(()),
                    Err(error) => Err(error),
                }),
            self.inputs
                .caller
                .as_ref()
                .and_then(|caller| caller.context_derivation_result.as_ref()),
        )
    }

    /// ONLY erasure exit, consuming the created-attempt owner. Complete actual
    /// F/G/outer binding/library/custody proof is not present: remain CLOSED
    /// before ANY Box, returning the WHOLE failed owner without replacing its
    /// canonical preparation/creation results. Internal context derivation,
    /// std::ptr::eq original identity and binding still need joint assembly.
    pub fn erase(
        self,
    ) -> Result<Box<dyn super::SeatDriver>, (ArchiveError, OriginalClaude<ArchiveError>)> {
        Err((
            ArchiveError::Fit,
            OriginalClaude {
                inputs: self.inputs,
                phase: std::marker::PhantomData,
            },
        ))
    }
}

// SOURCE controls for the ONE strict joint; UNCOMPILED/UNRUN. Synthetic owner
// descriptors test association only, never actual Server TOTAL or original F.
#[cfg(test)]
mod opaque_constructor_controls {
    use super::*;
    use crate::host::{
        BoundHostOperationLayout, HostAdmission, HostIdentity, HostPromptLease, HostRefusal,
        HostSnapshot, HostStep, HostStepLease,
    };
    use crate::transcript::TranscriptStore;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NoActivityHost(Arc<AtomicUsize>);

    impl NoActivityHost {
        fn called(&self) -> ! {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("opaque constructor called a host method");
        }
    }

    impl HostAdmission for NoActivityHost {
        fn subscribe(&self) -> tokio::sync::watch::Receiver<HostSnapshot> {
            self.called()
        }

        fn snapshot(&self) -> Result<HostSnapshot, HostRefusal> {
            self.called()
        }

        fn acquire(
            &self,
            _: &HostIdentity,
            _: HostStep,
        ) -> Result<Box<dyn HostStepLease>, HostRefusal> {
            self.called()
        }

        fn acquire_prompt(
            &self,
            _: &HostIdentity,
            _: Allowance,
        ) -> Result<Box<dyn HostPromptLease>, HostRefusal> {
            self.called()
        }
    }

    struct NoActivityStore(Arc<AtomicUsize>);

    // Spell async_trait's boxed signature so the sentinel fires on METHOD
    // INVOCATION, even if a future would otherwise never be polled. No task or
    // service body is needed to observe an accidental constructor-side call.
    impl TranscriptStore for NoActivityStore {
        fn append<'life0, 'async_trait>(
            &'life0 self,
            _: cna_core::ids::SeatId,
            _: String,
            _: cna_protocol::TranscriptEntry,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<u64, String>> + Send + 'async_trait>,
        >
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("opaque constructor called a store method");
        }
    }

    fn configuration() -> ClaudeConfig {
        ClaudeConfig {
            seat: cna_core::ids::SeatId::all().next().expect("fixed seats"),
            exe: None,
            model: "opaque-model-private".into(),
            config_dir: None,
            expected_email: None,
            sandbox: "opaque-sandbox-never-opened".into(),
            run_dir: "opaque-run-never-opened".into(),
            mcp_url: "opaque-endpoint-never-contacted".into(),
            system_prompt: "opaque-prompt-private".into(),
            effort: None,
            context_window: None,
        }
    }

    fn identity(seat: cna_core::ids::SeatId) -> HostIdentity {
        HostIdentity {
            seat,
            epoch: 7,
            decision_id: "opaque-decision-private".into(),
            revision: 11,
            operation_id: "00000000-0000-0000-0000-000000000003".into(),
        }
    }

    #[test] // Deliberately no Tokio runtime, files, actors, DB or actual host.
    fn constructor_stages_same_inputs_without_host_store_or_factory_activity() {
        let calls = Arc::new(AtomicUsize::new(0));
        let store: Arc<dyn TranscriptStore> = Arc::new(NoActivityStore(calls.clone()));
        let admission: Arc<dyn HostAdmission> = Arc::new(NoActivityHost(calls.clone()));
        let store_weak = Arc::downgrade(&store);
        let host_weak = Arc::downgrade(&admission);
        let configuration = configuration();
        let identity = identity(configuration.seat);
        let model_body = configuration.model.as_ptr();
        let decision_body = identity.decision_id.as_ptr();
        let stream: Arc<str> = Arc::from(identity.operation_id.as_str());
        let stream_weak = Arc::downgrade(&stream);
        let descriptor = BoundHostOperationLayout::for_storage::<[u8; 1]>(admission.clone());
        let pending = OriginalClaude::new(
            configuration,
            store,
            stream,
            admission,
            identity,
            descriptor,
        )
        .expect("synthetic same-owner constructor");
        assert!(pending.inputs.constructor_valid);
        assert!(pending.inputs.stderr_lines.is_none());
        assert!(pending.inputs.stdout_fifo.is_none());
        assert!(pending.inputs.physical_storage.is_none());
        assert!(pending.inputs.factory.is_none());
        assert!(pending.inputs.caller.is_none());
        assert!(pending.inputs.preparation.is_none());
        assert!(pending.inputs.creation_observation.is_none());
        assert!(std::ptr::eq(
            pending
                .inputs
                .configuration
                .as_ref()
                .unwrap()
                .model
                .as_ptr(),
            model_body,
        ));
        let context = pending.inputs.context.as_ref().unwrap();
        assert!(std::ptr::eq(
            context.identity.decision_id.as_ptr(),
            decision_body
        ));
        assert!(
            pending
                .inputs
                .descriptor
                .as_ref()
                .unwrap()
                .matches_owner(&context.admission)
        );
        assert_eq!(
            format!("{pending:?}"),
            "OriginalClaude { state: \"pending\" }"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(pending);
        assert!(store_weak.upgrade().is_none());
        assert!(host_weak.upgrade().is_none());
        assert!(stream_weak.upgrade().is_none());
    }

    #[test]
    fn missing_staged_input_refuses_without_consuming_other_input_bodies() {
        for case in 0..5 {
            let calls = Arc::new(AtomicUsize::new(0));
            let store: Arc<dyn TranscriptStore> = Arc::new(NoActivityStore(calls.clone()));
            let admission: Arc<dyn HostAdmission> = Arc::new(NoActivityHost(calls.clone()));
            let configuration = configuration();
            let identity = identity(configuration.seat);
            let model_body = configuration.model.as_ptr();
            let stream = Arc::from(identity.operation_id.as_str());
            let descriptor = BoundHostOperationLayout::for_storage::<[u8; 1]>(admission.clone());
            let mut pending = OriginalClaude::new(
                configuration,
                store,
                stream,
                admission,
                identity,
                descriptor,
            )
            .expect("synthetic whole staged owner");
            // Private fault injection only: a missing later input used to make
            // prepare take and then DROP the earlier configuration on refusal.
            // Hold the removed value separately, never repair/retry the owner.
            let removed_configuration = (case == 0).then(|| pending.inputs.configuration.take());
            let removed_store = (case == 1).then(|| pending.inputs.store.take());
            let removed_stream = (case == 2).then(|| pending.inputs.stream.take());
            let removed_context = (case == 3).then(|| pending.inputs.context.take());
            let removed_descriptor = (case == 4).then(|| pending.inputs.descriptor.take());
            assert!(matches!(
                pending.inputs.prepare(),
                Err(ArchiveError::Identity)
            ));
            assert_eq!(pending.inputs.configuration.is_some(), case != 0);
            assert_eq!(pending.inputs.store.is_some(), case != 1);
            assert_eq!(pending.inputs.stream.is_some(), case != 2);
            assert_eq!(pending.inputs.context.is_some(), case != 3);
            assert_eq!(pending.inputs.descriptor.is_some(), case != 4);
            assert!(pending.inputs.factory.is_none());
            assert!(pending.inputs.caller.is_none());
            assert!(pending.inputs.physical_storage.is_none());
            if let Some(configuration) = &pending.inputs.configuration {
                assert!(std::ptr::eq(configuration.model.as_ptr(), model_body));
            }
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            drop((
                removed_configuration,
                removed_store,
                removed_stream,
                removed_context,
                removed_descriptor,
            ));
            drop(pending);
        }
    }

    #[test] // No runtime/host/store calls; pending factory construction only.
    fn fifo_staging_keeps_one_whole_owner_and_first_layout_before_any_original() {
        let calls = Arc::new(AtomicUsize::new(0));
        let store: Arc<dyn TranscriptStore> = Arc::new(NoActivityStore(calls.clone()));
        let transcript = crate::transcript::PendingTranscriptInputs::new(
            store,
            Arc::from("00000000-0000-0000-0000-000000000003"),
        );
        let mut factory = PendingClaudeFactory::new(configuration(), transcript);
        factory.assemble().unwrap();
        let mut caller = match OriginalClaudeCaller::from_pending_factory(factory) {
            Ok(caller) => caller,
            Err(_) => panic!("source fixture assembly"),
        };
        let mut candidate = Some(super::super::frame_queue::PendingOwnedChannel::new());
        caller.stage_stdout_fifo(&mut candidate).unwrap();
        assert!(candidate.is_none());
        assert!(caller.stdout_fifo.is_some());
        let first_owner = std::ptr::from_ref(caller.stdout_fifo.as_ref().unwrap());
        let first_result = std::ptr::from_ref(caller.stdout_fifo_layout.as_ref().unwrap());
        assert!(matches!(caller.stdout_fifo_layout, Some(Ok(_))));
        let mut replacement = Some(super::super::frame_queue::PendingOwnedChannel::new());
        assert_eq!(
            caller.stage_stdout_fifo(&mut replacement),
            Err(ArchiveError::Identity)
        );
        assert!(replacement.is_some());
        assert_eq!(
            std::ptr::from_ref(caller.stdout_fifo.as_ref().unwrap()),
            first_owner
        );
        assert_eq!(
            std::ptr::from_ref(caller.stdout_fifo_layout.as_ref().unwrap()),
            first_result
        );
        assert!(!caller.original_creation_started);
        assert!(caller.original_creation_result.is_none() && caller.original.is_none());
        assert!(caller.binding_result.is_none() && caller.pending_context.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // No fabricated creation Ok: even a staged actual FIFO cannot cause
        // binding without the caller's actual canonical original return.
        assert_eq!(
            caller.derive_created_context_once(),
            Err(ArchiveError::Identity)
        );
        let first_derivation =
            std::ptr::from_ref(caller.context_derivation_result.as_ref().unwrap());
        assert_eq!(
            caller.derive_created_context_once(),
            Err(ArchiveError::Identity)
        );
        assert_eq!(
            std::ptr::from_ref(caller.context_derivation_result.as_ref().unwrap()),
            first_derivation
        );
        assert!(
            caller
                .stdout_fifo
                .as_ref()
                .unwrap()
                .creation_layout()
                .is_ok()
        );
        assert!(caller.original_creation_result.is_none() && caller.original.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test] // UNCOMPILED/UNRUN, no runtime/host/store effects or original Fit.
    fn physical_storage_stages_whole_once_and_replaces_header_facts_without_duplicate_backing() {
        let calls = Arc::new(AtomicUsize::new(0));
        let store: Arc<dyn TranscriptStore> = Arc::new(NoActivityStore(calls.clone()));
        let transcript = crate::transcript::PendingTranscriptInputs::new(
            store,
            Arc::from("00000000-0000-0000-0000-000000000003"),
        );
        let mut factory = PendingClaudeFactory::new(configuration(), transcript);
        factory.assemble().unwrap();
        let mut caller = match OriginalClaudeCaller::from_pending_factory(factory) {
            Ok(caller) => caller,
            Err(_) => panic!("source fixture assembly"),
        };
        let mut ring = Some(super::super::PendingStderrLines::new().unwrap());
        caller.stage_stderr_lines(&mut ring).unwrap();
        assert!(ring.is_none());
        let mut candidate = Some(super::super::child_pending::PendingPhysicalChild::new());
        caller.stage_physical_storage(&mut candidate).unwrap();
        assert!(candidate.is_none());
        assert!(caller.stderr_lines.is_none()); // SAME ring is now in physical owner.
        let first_owner = std::ptr::from_ref(caller.physical_storage.as_ref().unwrap());
        let first_result = std::ptr::from_ref(caller.physical_storage_layout.as_ref().unwrap());
        let actual = caller
            .physical_storage_layout
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap();
        let headers = ChildCreationLayouts::derive(Some(actual)).unwrap();
        assert_eq!(headers.capture_cell, actual.capture_body);
        assert_eq!(headers.reader_failures, actual.reader_failures_body);
        assert_eq!(headers.stderr_tail, actual.stderr_tail_body);
        assert_eq!(
            [
                headers.stdout_assembly,
                headers.stdout_pending,
                headers.stdout_chunk,
                headers.stderr_chunk,
                headers.stderr_stats
            ],
            actual.bodies
        );
        assert_eq!(
            headers.referenced_headers_bytes,
            actual.referenced_headers_bytes
                + headers.stderr_tail.size()
                + headers.reader_failures.size()
                + headers.capture_cell.size()
        );
        let mut replacement = Some(super::super::child_pending::PendingPhysicalChild::new());
        assert_eq!(
            caller.stage_physical_storage(&mut replacement),
            Err(ArchiveError::Identity)
        );
        assert!(replacement.is_some());
        assert_eq!(
            std::ptr::from_ref(caller.physical_storage.as_ref().unwrap()),
            first_owner
        );
        assert_eq!(
            std::ptr::from_ref(caller.physical_storage_layout.as_ref().unwrap()),
            first_result
        );
        assert!(!caller.original_creation_started && !caller.known_creation_started);
        assert!(caller.original_creation_result.is_none() && caller.original.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // Staged cells and a layout Ok are NOT a fabricated canonical creation.
        assert_eq!(
            caller.derive_created_context_once(),
            Err(ArchiveError::Identity)
        );
        let first_derivation =
            std::ptr::from_ref(caller.context_derivation_result.as_ref().unwrap());
        assert_eq!(
            caller.derive_created_context_once(),
            Err(ArchiveError::Identity)
        );
        assert_eq!(
            std::ptr::from_ref(caller.context_derivation_result.as_ref().unwrap()),
            first_derivation
        );
        assert!(
            caller
                .physical_storage
                .as_ref()
                .unwrap()
                .creation_layout()
                .is_ok()
        );
        assert!(caller.original_creation_result.is_none() && caller.original.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test] // Every refused constructor returns WHOLE staged custody, no repair.
    fn invalid_uuid_seat_and_foreign_owner_retain_inputs_until_failed_owner_drop() {
        for case in 0..3 {
            let calls = Arc::new(AtomicUsize::new(0));
            let store: Arc<dyn TranscriptStore> = Arc::new(NoActivityStore(calls.clone()));
            let admission: Arc<dyn HostAdmission> = Arc::new(NoActivityHost(calls.clone()));
            let store_weak = Arc::downgrade(&store);
            let host_weak = Arc::downgrade(&admission);
            let configuration = configuration();
            let mut identity = identity(configuration.seat);
            if case == 0 {
                identity.operation_id = "invalid-original-uuid".into();
            }
            if case == 1 {
                identity.seat = cna_core::ids::SeatId::all()
                    .nth(1)
                    .expect("second fixed seat");
            }
            let descriptor_owner = if case == 2 {
                Arc::new(NoActivityHost(calls.clone())) as Arc<dyn HostAdmission>
            } else {
                admission.clone()
            };
            let descriptor_weak = Arc::downgrade(&descriptor_owner);
            let descriptor = BoundHostOperationLayout::for_storage::<[u8; 1]>(descriptor_owner);
            let model_body = configuration.model.as_ptr();
            let decision_body = identity.decision_id.as_ptr();
            let stream: Arc<str> = Arc::from(identity.operation_id.as_str());
            let stream_weak = Arc::downgrade(&stream);
            let (error, failed) = OriginalClaude::new(
                configuration,
                store,
                stream,
                admission,
                identity,
                descriptor,
            )
            .expect_err("invalid or foreign staged inputs");
            assert!(matches!(error, ArchiveError::Identity));
            assert!(!failed.inputs.constructor_valid);
            assert!(failed.inputs.stderr_lines.is_none());
            assert!(failed.inputs.stdout_fifo.is_none());
            assert!(failed.inputs.physical_storage.is_none());
            assert!(failed.inputs.factory.is_none());
            assert!(failed.inputs.caller.is_none());
            assert!(failed.inputs.preparation.is_none());
            assert!(failed.inputs.creation_observation.is_none());
            assert!(std::ptr::eq(
                failed.inputs.configuration.as_ref().unwrap().model.as_ptr(),
                model_body,
            ));
            assert!(std::ptr::eq(
                failed
                    .inputs
                    .context
                    .as_ref()
                    .unwrap()
                    .identity
                    .decision_id
                    .as_ptr(),
                decision_body,
            ));
            assert!(store_weak.upgrade().is_some());
            assert!(host_weak.upgrade().is_some());
            assert!(descriptor_weak.upgrade().is_some());
            assert!(stream_weak.upgrade().is_some());
            assert_eq!(
                format!("{failed:?}"),
                "OriginalClaude { state: \"constructor_refused\" }"
            );
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            drop(failed);
            assert!(store_weak.upgrade().is_none());
            assert!(host_weak.upgrade().is_none());
            assert!(descriptor_weak.upgrade().is_none());
            assert!(stream_weak.upgrade().is_none());
        }
    }
}
