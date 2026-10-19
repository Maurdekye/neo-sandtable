//! Process-main custody for the staged owned driver path (lead mdfe08befa7e8).
//!
//! This source is not connected to positive creation or erasure. The actual
//! journal/whole-creation fit must precede issuing a production lease.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use tokio::runtime::Handle;
use tokio::sync::Notify;
use tokio::time::Instant;

pub(crate) mod task;
mod registration;
mod journal;
mod namespace;
mod original;
mod report;
mod transport;
#[cfg(windows)]
mod custody;

/// Host-created opaque custody, shared only through Arc with Server's retained
/// writer and Campaign owners. No public constructor or observation API.
/// Its borrowed path is for the two guarded SQLite opens, never an identity key.
pub struct CampaignFileCustody {
    protected_path: std::path::PathBuf,
    #[cfg(windows)]
    original: original::ChargeRef,
    #[cfg(windows)]
    prefix: Mutex<Box<namespace::PreopenRoot>>,
    #[cfg(windows)]
    scope: Option<Weak<ScopeControl>>,
}

impl CampaignFileCustody {
    pub fn protected_path(&self) -> &std::path::Path { &self.protected_path }
}

/// The single original process owner. Never move it into an App/facade task.
/// Construction only moves an explicit Handle and empty inline state.
pub struct SeatHost {
    runtime: Handle,
    scopes: Option<Box<ScopeEntry>>,
    admission_closed: bool,
    drivers: Option<Box<DriverEntry>>,
    registration: Option<Arc<registration::Registrar>>,
    registration_uncertain: bool,
    registration_active: usize,
    global_first_request: Option<Instant>,
    global_deadline: Option<Instant>,
    global_unknown: bool,
    journal: Option<Box<journal::JournalRoot>>,
    namespace_observations: Option<Box<namespace::GuardRoot>>,
    namespace_refused: bool,
    #[cfg(windows)]
    namespace_chains: Option<Box<namespace::ChainRoot>>,
    #[cfg(windows)]
    namespace_layouts: Option<Box<namespace::LayoutRoot>>,
    #[cfg(windows)]
    canonical_serial: namespace::CanonicalSerial,
    #[cfg(windows)]
    storage_contexts: Option<Box<namespace::ContextRoot>>,
    #[cfg(windows)]
    scope_contexts: Option<Box<ScopeContextEntry>>,
    #[cfg(windows)]
    member_files: Option<Box<namespace::MemberFilesRoot>>,
    #[cfg(windows)]
    preopen_files: Option<Box<namespace::PreopenRoot>>,
    #[cfg(windows)]
    custody_attempts: Option<Box<custody::Attempt>>,
    #[cfg(windows)]
    last_scope_number: i64,
    bridge_refusal: Option<std::io::ErrorKind>,
    host_original: original::OwnerSlot,
    host_source_refusal: Option<crate::operation::OperationError>,
    report_boundary: Option<report::ReportBoundary>,
    transport: Option<Box<transport::Root>>,
}

impl SeatHost {
    /// Server-only pre-open custody. `create_new` selects the atomic operation
    /// (fresh=true, recovery=false); it is NOT identity/provenance evidence.
    /// The retained Server parent must store the returned Arc before either
    /// SQLite open. Complete Host fit and durable lease production are required.
    pub fn mint_campaign_custody(&mut self, lease: &SeatHostLease,
        path: &std::path::Path, create_new: bool)
        -> Result<Arc<CampaignFileCustody>, std::io::ErrorKind>
    {
        if let Some(refusal) = self.bridge_refusal { return Err(refusal); }
        #[cfg(not(windows))] {
            let _ = (lease, path, create_new);
            self.bridge_refusal = Some(std::io::ErrorKind::Unsupported); self.admission_closed = true;
            Err(std::io::ErrorKind::Unsupported)
        }
        #[cfg(windows)] {
            let mode = if create_new { namespace::PreopenMode::Fresh } else { namespace::PreopenMode::Existing };
            match self.prepare_campaign_custody(lease, path, mode) {
                Ok(custody) => Ok(custody), Err(error) => Err(self.refuse_bridge(error)),
            }
        }
    }

    /// Server keeps its actual connection, custody Arc and both original joins
    /// alive for this call. Project ACTUAL decoded records as campaign=[id,
    /// meta.rules_profile], pins=[rules_profile,content_hash,engine_version].
    /// No raw path, generation, scope number, file ID or boolean proof is input.
    /// Ok(()) acknowledges the private durable member row, never driver GO or
    /// all-writer/CLEAN settlement. The complete Host fit/lease gates still apply.
    pub fn pair_campaign_custody(&mut self, lease: &SeatHostLease, custody: &CampaignFileCustody,
        campaign: [&str; 2], pins: [&str; 3], archive: &crate::failed_evidence::CampaignArchive)
        -> Result<(), std::io::ErrorKind>
    {
        if let Some(refusal) = self.bridge_refusal { return Err(refusal); }
        #[cfg(not(windows))] {
            let _ = (lease, custody, campaign, pins, archive);
            self.bridge_refusal = Some(std::io::ErrorKind::Unsupported); self.admission_closed = true;
            Err(std::io::ErrorKind::Unsupported)
        }
        #[cfg(windows)] {
            match self.register_paired_member(lease, custody, campaign, pins, archive) {
                Ok(()) => Ok(()), Err(error) => Err(self.refuse_bridge(error)),
            }
        }
    }

    #[cfg(windows)]
    fn refuse_bridge(&mut self, error: crate::operation::OperationError) -> std::io::ErrorKind {
        if self.host_source_refusal.is_none() { self.host_source_refusal = Some(error); }
        if self.bridge_refusal.is_none() { self.bridge_refusal = Some(std::io::ErrorKind::Other); }
        self.admission_closed = true; self.global_unknown = true;
        self.bridge_refusal.unwrap_or(std::io::ErrorKind::Other)
    }

    #[cfg(windows)]
    fn require_scope_lease(&self, lease: &SeatHostLease) -> Result<i64, crate::operation::OperationError> {
        use crate::operation::OperationError;
        if self.admission_closed || self.namespace_refused || lease.control.is_closed()
            || lease.control.uncertain.load(Ordering::Acquire)
            || !lease.control.durable_open.load(Ordering::Acquire)
        { return Err(OperationError::CreditOwned); }
        let actual = self.host_original.reference()?;
        let registrar = self.registration.as_ref().ok_or(OperationError::CreditOwned)?;
        let supplied = lease.registration.upgrade().ok_or(OperationError::CreditOwned)?;
        if !Arc::ptr_eq(registrar, &supplied) || !registrar.owned_by(&actual) { return Err(OperationError::CreditOwned); }
        let mut scopes = self.scopes.as_deref(); let mut retained = false;
        while let Some(scope) = scopes {
            if Arc::ptr_eq(&scope.control, &lease.control) { retained = true; break; }
            scopes = scope.next.as_deref();
        }
        if !retained { return Err(OperationError::CreditOwned); }
        let mut contexts = self.scope_contexts.as_deref();
        while let Some(context) = contexts {
            if context.control.upgrade().is_some_and(|control| Arc::ptr_eq(&control, &lease.control)) && context.number > 0 {
                return Ok(context.number);
            }
            contexts = context.next.as_deref();
        }
        Err(OperationError::CreditOwned)
    }

    /// The sanctioned public pairing entry has exactly this private member
    /// registration consumer. Original raw failures stay in their Host roots.
    #[cfg(windows)]
    fn register_paired_member(&mut self, lease: &SeatHostLease, custody: &CampaignFileCustody,
        campaign: [&str; 2], pins: [&str; 3], archive: &crate::failed_evidence::CampaignArchive)
        -> Result<(), crate::operation::OperationError>
    {
        use crate::operation::OperationError;
        let original = self.host_original.reference()?; self.check_host_source(&original)?;
        let number = self.require_scope_lease(lease)?;
        if !custody.original.same_instance(&original)
            || !custody.scope.as_ref().and_then(Weak::upgrade).is_some_and(|control| Arc::ptr_eq(&control, &lease.control))
            || campaign[1] != pins[0]
        { return Err(OperationError::CreditOwned); }
        self.revalidate_scope_contexts()?;
        let mut scope = self.scope_contexts.as_deref();
        while scope.is_some_and(|entry| entry.number != number) { scope = scope.and_then(|entry| entry.next.as_deref()); }
        let scope = scope.ok_or(OperationError::CreditOwned)?;
        let mut context = self.storage_contexts.as_deref_mut();
        while context.as_deref().is_some_and(|entry| !scope.context.matches(entry)) {
            context = context.and_then(|entry| entry.next.as_deref_mut());
        }
        let context = context.ok_or(OperationError::CreditOwned)?;
        let journal = self.journal.as_deref_mut().ok_or(OperationError::CreditOwned)?;
        let process = self.namespace_layouts.as_deref_mut().ok_or(OperationError::CreditOwned)?;
        let mut attempt = self.custody_attempts.as_deref_mut();
        while attempt.as_deref().is_some_and(|entry| !entry.published().is_some_and(|arc| std::ptr::eq(arc.as_ref(), custody))) {
            attempt = attempt.and_then(|entry| entry.next.as_deref_mut());
        }
        let attempt = attempt.ok_or(OperationError::CreditOwned)?;
        attempt.register_member(context, journal, process, custody::MemberInputs {
            number, campaign, pins, archive, control: &lease.control,
        })
    }

    /// Pure construction inside the binary's existing runtime. No discovery,
    /// allocation, journal, task, child or fallback runtime is created here.
    pub fn new(runtime: Handle) -> Self {
        Self {
            runtime, scopes: None, admission_closed: false, drivers: None,
            registration: None, registration_uncertain: false,
            registration_active: 0,
            global_first_request: None, global_deadline: None, global_unknown: false,
            journal: None,
            namespace_observations: None,
            namespace_refused: false,
            #[cfg(windows)] namespace_chains: None,
            #[cfg(windows)] namespace_layouts: None,
            #[cfg(windows)] canonical_serial: namespace::CanonicalSerial::new(),
            #[cfg(windows)] storage_contexts: None,
            #[cfg(windows)] scope_contexts: None,
            #[cfg(windows)] member_files: None,
            #[cfg(windows)] preopen_files: None,
            #[cfg(windows)] custody_attempts: None,
            #[cfg(windows)] last_scope_number: 0,
            bridge_refusal: None,
            host_original: original::OwnerSlot::new(),
            host_source_refusal: None,
            report_boundary: None,
            transport: None,
        }
    }

    /// Sole process-owner consumer. The sealed COMPLETE fit producer is still
    /// unfinished, so this private source seam cannot admit production work.
    fn install_host_original(&mut self, pending: &mut Option<original::HostFit>)
        -> Result<(), crate::operation::OperationError>
    {
        if self.admission_closed || self.namespace_refused || self.journal.is_some()
            || self.scopes.is_some() || self.drivers.is_some() || self.registration.is_some()
            || self.namespace_observations.is_some() || self.transport.is_some()
        { return Err(crate::operation::OperationError::CreditOwned); }
        #[cfg(windows)]
        if self.namespace_chains.is_some() || self.namespace_layouts.is_some() || self.storage_contexts.is_some()
            || self.scope_contexts.is_some() || self.member_files.is_some() || self.preopen_files.is_some()
            || self.custody_attempts.is_some()
        {
            return Err(crate::operation::OperationError::CreditOwned);
        }
        self.host_original.consume(pending)
    }

    /// Host-only fixed counter custody. The real lease and installed process
    /// original are checked before any allocation; no public transport handle.
    #[cfg(windows)]
    fn reserve_owned_transport_counter(&mut self, lease: &SeatHostLease, seat: cna_core::ids::SeatId)
        -> Result<(), crate::operation::OperationError>
    {
        let result = (|| {
            let original = self.host_original.reference()?;
            self.check_host_source(&original)?;
            let number = self.require_scope_lease(lease)?;
            if self.transport.is_none() {
                self.transport = Some(transport::Root::reserve(&original)?);
            }
            self.transport.as_deref_mut()
                .ok_or(crate::operation::OperationError::CreditOwned)?
                .reserve_counter(number, &lease.control, seat)
        })();
        if let Err(error) = result {
            if self.host_source_refusal.is_none() { self.host_source_refusal = Some(error); }
            self.admission_closed = true;
        }
        result
    }

    fn check_host_source(&mut self, supplied: &impl original::Source) -> Result<(), crate::operation::OperationError> {
        let supplied = supplied.charge_ref();
        #[cfg(test)]
        if supplied.is_driver_fixture() { return Ok(()); }
        if let Some(error) = &self.host_source_refusal { return Err(*error); }
        let checked = (|| {
            let actual = self.host_original.reference()?;
            if !supplied.same_instance(&actual) { return Err(crate::operation::OperationError::CreditOwned); }
            actual.check()
        })();
        if let Err(error) = &checked {
            self.host_source_refusal = Some(*error); self.admission_closed = true;
        }
        checked
    }

    /// Proposed private observation only, not verified namespace/lease issuance.
    /// Prepay actual node then install it in Host BEFORE moving the File/query.
    fn observe_namespace_file(
        &mut self,
        original: &impl original::Source,
        pending: &mut Option<std::fs::File>,
        role: namespace::Role,
    ) -> Result<namespace::Observation, crate::operation::OperationError> {
        self.check_host_source(original)?;
        if self.admission_closed || self.namespace_refused || pending.is_none() {
            return Err(crate::operation::OperationError::CreditOwned);
        }
        let mut root = Box::new(namespace::GuardRoot::reserve(original)?);
        root.next = self.namespace_observations.take();
        self.namespace_observations = Some(root);
        let root = self.namespace_observations.as_deref_mut()
            .ok_or(crate::operation::OperationError::CreditOwned)?;
        let observation = root.observe(pending, role);
        Ok(self.finish_namespace_observation(observation))
    }

    /// Installed owner before any staged existing-path open. This does not
    /// create a namespace or assert any SQLite/ancestor path association.
    #[cfg(windows)]
    fn open_namespace_existing_verbatim(
        &mut self, original: &impl original::Source,
        path: &std::path::Path, role: namespace::Role,
    ) -> Result<namespace::Observation, crate::operation::OperationError> {
        self.check_host_source(original)?;
        if self.admission_closed || self.namespace_refused {
            return Err(crate::operation::OperationError::CreditOwned);
        }
        let mut root = Box::new(namespace::GuardRoot::reserve(original)?);
        root.next = self.namespace_observations.take();
        self.namespace_observations = Some(root);
        let root = self.namespace_observations.as_deref_mut()
            .ok_or(crate::operation::OperationError::CreditOwned)?;
        let observation = root.open_existing_verbatim(path, role);
        Ok(self.finish_namespace_observation(observation))
    }

    fn finish_namespace_observation(&mut self, observation: namespace::Observation)
        -> namespace::Observation
    {
        if observation != namespace::Observation::Observed {
            self.namespace_refused = true;
            return observation;
        }
        let Some(root) = self.namespace_observations.as_deref_mut() else {
            self.namespace_refused = true;
            return namespace::Observation::Refused(namespace::Refusal::Closed);
        };
        let mut previous = root.next.as_deref();
        let mut alias = false;
        while let Some(entry) = previous {
            alias |= root.aliases_regular_file(entry);
            previous = entry.next.as_deref();
        }
        if alias {
            self.namespace_refused = true;
            return root.refuse_alias();
        }
        observation
    }

    /// Staged spelling preparation only; no issuer consumes it as authority.
    #[cfg(windows)]
    fn prepare_namespace_owner_path(
        &mut self, original: &impl original::Source, path: &std::path::Path,
    ) -> Result<namespace::PathObservation, crate::operation::OperationError> {
        self.check_host_source(original)?;
        if self.admission_closed || self.namespace_refused {
            return Err(crate::operation::OperationError::CreditOwned);
        }
        let mut root = Box::new(namespace::GuardRoot::reserve(original)?);
        root.next = self.namespace_observations.take();
        self.namespace_observations = Some(root);
        let root = self.namespace_observations.as_deref_mut()
            .ok_or(crate::operation::OperationError::CreditOwned)?;
        let observation = root.prepare_owner_path(path);
        if matches!(observation, namespace::PathObservation::Refused(_)) {
            self.namespace_refused = true;
        }
        Ok(observation)
    }

    /// Continue the SAME newest prepared node. Refusal retains every prior
    /// path/File/error owner; no new node/Operation or native path copy occurs.
    /// An observed handle is not a validated ancestor/SQLite namespace.
    #[cfg(windows)]
    fn open_prepared_namespace_existing(&mut self, role: namespace::Role)
        -> Result<namespace::Observation, crate::operation::OperationError>
    {
        if self.admission_closed || self.namespace_refused {
            return Err(crate::operation::OperationError::CreditOwned);
        }
        let Some(root) = self.namespace_observations.as_deref_mut() else {
            self.namespace_refused = true;
            return Err(crate::operation::OperationError::CreditOwned);
        };
        let observation = root.open_prepared_existing(role);
        Ok(self.finish_namespace_observation(observation))
    }

    /// Private handle check only; no issuer/publication calls it as proof yet.
    /// Keep every actual root/File/raw result in the main owner on any refusal.
    fn revalidate_namespace_handles(&mut self) -> namespace::Observation {
        if self.admission_closed || self.namespace_refused {
            return namespace::Observation::Refused(namespace::Refusal::Closed);
        }
        let mut next = self.namespace_observations.as_deref_mut();
        let mut observed = false;
        while let Some(entry) = next {
            let outcome = entry.revalidate_handle();
            if outcome != namespace::Observation::Observed {
                self.namespace_refused = true;
                return outcome;
            }
            observed = true;
            next = entry.next.as_deref_mut();
        }
        if observed { namespace::Observation::Observed }
        else { namespace::Observation::Refused(namespace::Refusal::Closed) }
    }

    /// Retain an entire existing root-to-leaf graph before interpreting it.
    /// This remains private observation; actual admission/lease validation must
    /// additionally establish excluded-root and SQLite associations.
    #[cfg(windows)]
    fn observe_existing_namespace_chain(
        &mut self, original: &impl original::Source,
        path: &std::path::Path, role: namespace::Role,
    ) -> Result<namespace::Observation, crate::operation::OperationError> {
        self.check_host_source(original)?;
        if self.admission_closed || self.namespace_refused {
            return Err(crate::operation::OperationError::CreditOwned);
        }
        let mut chain = Box::new(namespace::ChainRoot::reserve(original)?);
        chain.next = self.namespace_chains.take();
        self.namespace_chains = Some(chain);
        let chain = self.namespace_chains.as_deref_mut()
            .ok_or(crate::operation::OperationError::CreditOwned)?;
        let observation = chain.observe_existing(path, role);
        if observation != namespace::Observation::Observed { self.namespace_refused = true; }
        Ok(observation)
    }

    /// Private retained existing-root exclusion graph; this is not a lease or
    /// an assertion that the caller supplied the complete isolation census.
    #[cfg(windows)]
    fn observe_existing_namespace_layout(
        &mut self, original: &impl original::Source,
        storage: &std::path::Path, excluded: &[&std::path::Path],
    ) -> Result<namespace::Observation, crate::operation::OperationError> {
        self.check_host_source(original)?;
        if self.admission_closed || self.namespace_refused {
            return Err(crate::operation::OperationError::CreditOwned);
        }
        let mut layout = Box::new(namespace::LayoutRoot::reserve(original)?);
        layout.next = self.namespace_layouts.take();
        self.namespace_layouts = Some(layout);
        let layout = self.namespace_layouts.as_deref_mut()
            .ok_or(crate::operation::OperationError::CreditOwned)?;
        let observation = layout.observe_existing(storage, excluded);
        if observation != namespace::Observation::Observed { self.namespace_refused = true; }
        Ok(observation)
    }

    /// Complete only the newest retained layout's one directory/leaf attempt.
    /// Retains the namespace lock; no SQLite connection, scope or lease yet.
    #[cfg(windows)]
    fn prepare_observed_namespace(&mut self) -> Result<namespace::Observation, crate::operation::OperationError> {
        if self.admission_closed || self.namespace_refused {
            return Err(crate::operation::OperationError::CreditOwned);
        }
        let Some(layout) = self.namespace_layouts.as_deref_mut() else {
            self.namespace_refused = true;
            return Err(crate::operation::OperationError::CreditOwned);
        };
        let observation = layout.prepare_namespace_once();
        if observation != namespace::Observation::Observed { self.namespace_refused = true; }
        Ok(observation)
    }

    /// Private operator-root observation only; never a worker/campaign path.
    /// One serial belongs to Host, not to each root or original Operation.
    #[cfg(windows)]
    fn canonicalize_observed_namespace_chain(&mut self)
        -> Result<namespace::Observation, crate::operation::OperationError>
    {
        if self.admission_closed || self.namespace_refused { return Err(crate::operation::OperationError::CreditOwned); }
        let Some(chain) = self.namespace_chains.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let observation = chain.canonicalize_local_root(&mut self.canonical_serial);
        if observation != namespace::Observation::Observed { self.namespace_refused = true; }
        Ok(observation)
    }

    #[cfg(windows)]
    fn canonicalize_observed_namespace_layout(&mut self)
        -> Result<namespace::Observation, crate::operation::OperationError>
    {
        if self.admission_closed || self.namespace_refused { return Err(crate::operation::OperationError::CreditOwned); }
        let Some(layout) = self.namespace_layouts.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let observation = layout.canonicalize_local_roots(&mut self.canonical_serial);
        if observation != namespace::Observation::Observed { self.namespace_refused = true; return Ok(observation); }
        let mut next = self.namespace_layouts.as_deref_mut();
        while let Some(layout) = next {
            let observation = layout.revalidate_local_roots();
            if observation != namespace::Observation::Observed { self.namespace_refused = true; return Ok(observation); }
            next = layout.next.as_deref_mut();
        }
        let mut refusal = None;
        let mut left = self.namespace_layouts.as_deref();
        'pairs: while let Some(current) = left {
            let mut right = current.next.as_deref();
            while let Some(other) = right {
                if let Err(reason) = current.check_local_domains_with(other) { refusal = Some(reason); break 'pairs; }
                right = other.next.as_deref();
            }
            left = current.next.as_deref();
        }
        if let Some(reason) = refusal {
            self.namespace_refused = true;
            return Ok(namespace::Observation::Refused(reason));
        }
        Ok(namespace::Observation::Observed)
    }

    /// Staged actual parent-meter seam. The caller must establish the complete
    /// process inventory before this; no invented Operation or scalar credit.
    /// No file/library operation occurs until the whole root is installed here.
    fn reserve_journal(&mut self, original: &impl original::Source)
        -> Result<(), crate::operation::OperationError>
    {
        self.check_host_source(original)?;
        if self.admission_closed || self.journal.is_some() {
            return Err(crate::operation::OperationError::CreditOwned);
        }
        let root = journal::JournalRoot::reserve(original)?;
        self.journal = Some(Box::new(root));
        Ok(())
    }

    /// Actual original roots are already installed; fixed Configured is not
    /// durable OPEN, complete census/fit validation or production lease issuance.
    #[cfg(windows)]
    fn open_observed_journal(&mut self) -> Result<journal::Observation, crate::operation::OperationError> {
        if self.admission_closed || self.namespace_refused { return Err(crate::operation::OperationError::CreditOwned); }
        let Some(journal) = self.journal.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let Some(namespace) = self.namespace_layouts.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let observation = journal.open_guarded_once(namespace);
        if observation == journal::Observation::Refused { self.admission_closed = true; }
        Ok(observation)
    }

    /// Private durable OPEN route: same retained connection owner plus the
    /// canonical local-root bracket must survive every pre/post effect check.
    /// No scope/lease is published here; complete inventory/fit remains owed.
    #[cfg(windows)]
    fn establish_observed_open(&mut self) -> Result<journal::Observation, crate::operation::OperationError> {
        if self.admission_closed || self.namespace_refused { return Err(crate::operation::OperationError::CreditOwned); }
        let Some(journal) = self.journal.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let Some(namespace) = self.namespace_layouts.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let observation = journal.establish_fresh_open(namespace);
        if observation == journal::Observation::Refused { self.admission_closed = true; }
        Ok(observation)
    }

    /// Retain operator-configured App roots independently of the ONE process
    /// journal. This is still a private observation, never a scope/fit issuer.
    #[cfg(windows)]
    fn observe_storage_context(
        &mut self, requested_namespace: &std::path::Path,
        app_storage: &std::path::Path, excluded: &[&std::path::Path],
    ) -> Result<namespace::Observation, crate::operation::OperationError> {
        if self.admission_closed || self.namespace_refused { return Err(crate::operation::OperationError::CreditOwned); }
        let Some(journal) = self.journal.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let Some(process) = self.namespace_layouts.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        // Same retained process meter, not a caller-selected/new allowance.
        let mut root = Box::new(namespace::ContextRoot::reserve(journal.context_original())?);
        root.next = self.storage_contexts.take();
        self.storage_contexts = Some(root);
        let Some(root) = self.storage_contexts.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let observation = root.observe_once(journal, process, requested_namespace,
            app_storage, excluded, &mut self.canonical_serial);
        if observation != namespace::Observation::Observed {
            self.admission_closed = true; return Ok(observation);
        }
        let mut next = self.storage_contexts.as_deref_mut();
        while let Some(root) = next {
            let observation = root.revalidate(journal, process);
            if observation != namespace::Observation::Observed {
                self.admission_closed = true; return Ok(observation);
            }
            next = root.next.as_deref_mut();
        }
        let mut refusal = None;
        let mut left = self.storage_contexts.as_deref();
        'pairs: while let Some(root) = left {
            let mut right = root.next.as_deref();
            while let Some(other) = right {
                if let Err(reason) = root.check_with(other) {
                    refusal = Some(reason); break 'pairs;
                }
                right = other.next.as_deref();
            }
            left = root.next.as_deref();
        }
        if let Some(reason) = refusal {
            self.admission_closed = true;
            let Some(root) = self.storage_contexts.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
            return Ok(root.refuse(reason));
        }
        Ok(namespace::Observation::Observed)
    }

    /// Host-created registry allocation, after the same retained durable OPEN
    /// revalidation. No scope is issued and no driver is prepared/spawned here.
    #[cfg(windows)]
    fn install_owned_registration(&mut self) -> Result<(), crate::operation::OperationError> {
        if self.admission_closed || self.namespace_refused || self.registration.is_some() {
            return Err(crate::operation::OperationError::CreditOwned);
        }
        let original = self.host_original.reference()?;
        self.check_host_source(&original)?;
        let Some(journal) = self.journal.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let Some(namespace) = self.namespace_layouts.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        if journal.context_binding(namespace).is_err() {
            self.admission_closed = true; return Err(crate::operation::OperationError::CreditOwned);
        }
        original::prepay(&original, registration::Registrar::creation_layout()?)?;
        self.registration = Some(Arc::new(registration::Registrar::new_owned(self.runtime.clone(), original)));
        Ok(())
    }

    /// Reserve custody only. The dormant control cannot admit a driver and no
    /// lease escapes this helper. Durable scope/context binding is still owed.
    #[cfg(windows)]
    fn reserve_unpublished_scope(&mut self) -> Result<(), crate::operation::OperationError> {
        if self.admission_closed || self.namespace_refused || self.scopes.is_some() {
            return Err(crate::operation::OperationError::CreditOwned);
        }
        let original = self.host_original.reference()?;
        self.check_host_source(&original)?;
        let Some(registrar) = self.registration.as_ref() else { return Err(crate::operation::OperationError::CreditOwned) };
        if !registrar.owned_by(&original) { return Err(crate::operation::OperationError::CreditOwned); }
        if let Some(error) = registrar.admission_failure() {
            self.admission_closed = true; return Err(error);
        }
        let Some(journal) = self.journal.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let Some(process) = self.namespace_layouts.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        if self.storage_contexts.is_none() { return Err(crate::operation::OperationError::CreditOwned); }
        let mut next = self.storage_contexts.as_deref_mut();
        while let Some(context) = next {
            if context.revalidate(journal, process) != namespace::Observation::Observed {
                self.admission_closed = true; return Err(crate::operation::OperationError::CreditOwned);
            }
            next = context.next.as_deref_mut();
        }
        // Until the scope/context binding producer exists there is at most one
        // dormant node, retained directly by Host and visible to global close.
        // Both allocations are charged before either is made; no caller bytes.
        let entry = match ScopeEntry::reserve_unpublished(&original) {
            Ok(entry) => entry,
            Err(error) => { self.admission_closed = true; return Err(error); }
        };
        self.scopes = Some(entry);
        Ok(())
    }

    /// Bind the one dormant node to the latest retained App context. This
    /// still cannot issue a lease or mark a scope ready for driver admission.
    #[cfg(windows)]
    fn bind_unpublished_scope_context(&mut self) -> Result<(), crate::operation::OperationError> {
        if self.admission_closed || self.namespace_refused || self.scope_contexts.is_some() {
            return Err(crate::operation::OperationError::CreditOwned);
        }
        let original = self.host_original.reference()?;
        self.check_host_source(&original)?;
        let Some(scope) = self.scopes.as_ref() else { return Err(crate::operation::OperationError::CreditOwned) };
        if scope.control.is_closed() || scope.control.durable_open.load(Ordering::Acquire) {
            return Err(crate::operation::OperationError::CreditOwned);
        }
        let Some(journal) = self.journal.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let Some(process) = self.namespace_layouts.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let Some(context) = self.storage_contexts.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let Some(number) = self.last_scope_number.checked_add(1) else {
            self.admission_closed = true; return Err(crate::operation::OperationError::CaptureOverflow);
        };
        if let Err(error) = original::prepay(&original, std::alloc::Layout::new::<ScopeContextEntry>()) {
            self.admission_closed = true; return Err(error);
        }
        let context = match context.scope_context(journal, process) {
            Ok(context) => context,
            Err(_) => { self.admission_closed = true; return Err(crate::operation::OperationError::CreditOwned); }
        };
        self.last_scope_number = number;
        self.scope_contexts = Some(Box::new(ScopeContextEntry {
            control: Arc::downgrade(&scope.control), context, number, next: None,
        }));
        Ok(())
    }

    #[cfg(windows)]
    fn revalidate_scope_contexts(&mut self) -> Result<(), crate::operation::OperationError> {
        if self.admission_closed || self.namespace_refused { return Err(crate::operation::OperationError::CreditOwned); }
        let Some(journal) = self.journal.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let Some(process) = self.namespace_layouts.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let mut next = self.scope_contexts.as_deref_mut();
        if next.is_none() { return Err(crate::operation::OperationError::CreditOwned); }
        while let Some(entry) = next {
            let Some(control) = entry.control.upgrade() else {
                self.admission_closed = true; return Err(crate::operation::OperationError::CreditOwned);
            };
            let mut scope = self.scopes.as_deref();
            while let Some(current) = scope {
                if Arc::ptr_eq(&current.control, &control) { break; }
                scope = current.next.as_deref();
            }
            if scope.is_none() { self.admission_closed = true; return Err(crate::operation::OperationError::CreditOwned); }
            let mut context = self.storage_contexts.as_deref_mut();
            let mut selected = None;
            while let Some(current) = context {
                if entry.context.matches(current) { selected = Some(current); break; }
                context = current.next.as_deref_mut();
            }
            let Some(context) = selected else {
                self.admission_closed = true; return Err(crate::operation::OperationError::CreditOwned);
            };
            if entry.context.revalidate(context, journal, process) != namespace::Observation::Observed {
                self.admission_closed = true; return Err(crate::operation::OperationError::CreditOwned);
            }
            next = entry.next.as_deref_mut();
        }
        Ok(())
    }

    /// Private File association observation, NOT a verified MemberRecord or
    /// connection/path authority. The actual archive path producer is owed.
    #[cfg(windows)]
    fn observe_member_files(&mut self, database: &std::path::Path, archive_database: &std::path::Path)
        -> Result<namespace::Observation, crate::operation::OperationError>
    {
        if self.admission_closed || self.namespace_refused { return Err(crate::operation::OperationError::CreditOwned); }
        let original = self.host_original.reference()?;
        self.check_host_source(&original)?;
        self.revalidate_scope_contexts()?;
        let Some(scope) = self.scope_contexts.as_deref() else { return Err(crate::operation::OperationError::CreditOwned) };
        let mut next = self.storage_contexts.as_deref_mut();
        let mut selected = None;
        while let Some(context) = next {
            if scope.context.matches(context) { selected = Some(context); break; }
            next = context.next.as_deref_mut();
        }
        let Some(context) = selected else { self.admission_closed = true; return Err(crate::operation::OperationError::CreditOwned) };
        let Some(journal) = self.journal.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let Some(process) = self.namespace_layouts.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let mut root = Box::new(namespace::MemberFilesRoot::reserve(&original)?);
        root.next = self.member_files.take(); self.member_files = Some(root);
        let Some(root) = self.member_files.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let observation = root.observe_once(context, journal, process, database, archive_database);
        if observation != namespace::Observation::Observed { self.admission_closed = true; }
        Ok(observation)
    }

    /// Private precursor to the sanctioned Arc bridge. No scope/fit bypass or
    /// public custody publication: every partial prefix stays on this Host.
    #[cfg(windows)]
    fn observe_preopen_file(&mut self, path: &std::path::Path, mode: namespace::PreopenMode)
        -> Result<namespace::Observation, crate::operation::OperationError>
    {
        if self.admission_closed || self.namespace_refused { return Err(crate::operation::OperationError::CreditOwned); }
        let original = self.host_original.reference()?;
        self.check_host_source(&original)?;
        self.revalidate_scope_contexts()?;
        let Some(scope) = self.scope_contexts.as_deref() else { return Err(crate::operation::OperationError::CreditOwned) };
        let Some(control) = scope.control.upgrade() else { return Err(crate::operation::OperationError::CreditOwned) };
        if control.is_closed() { return Err(crate::operation::OperationError::CreditOwned); }
        let mut next = self.storage_contexts.as_deref_mut();
        let mut selected = None;
        while let Some(context) = next {
            if scope.context.matches(context) { selected = Some(context); break; }
            next = context.next.as_deref_mut();
        }
        let Some(context) = selected else { self.admission_closed = true; return Err(crate::operation::OperationError::CreditOwned) };
        let Some(journal) = self.journal.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let Some(process) = self.namespace_layouts.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let root = match namespace::PreopenRoot::reserve(&original) {
            Ok(root) => root,
            Err(error) => { self.admission_closed = true; return Err(error); }
        };
        let mut root = Box::new(root); root.next = self.preopen_files.take(); self.preopen_files = Some(root);
        let Some(root) = self.preopen_files.as_deref_mut() else { return Err(crate::operation::OperationError::CreditOwned) };
        let observation = root.observe_once(context, journal, process, path, mode);
        if observation != namespace::Observation::Observed { self.admission_closed = true; }
        Ok(observation)
    }

    /// Private implementation of the sanctioned mint method. The actual
    /// attempt is retained BEFORE guard work and remains after every refusal.
    #[cfg(windows)]
    fn prepare_campaign_custody(&mut self, lease: &SeatHostLease, path: &std::path::Path, mode: namespace::PreopenMode)
        -> Result<Arc<CampaignFileCustody>, crate::operation::OperationError>
    {
        use crate::operation::OperationError;
        if self.admission_closed || self.namespace_refused { return Err(OperationError::CreditOwned); }
        let original = self.host_original.reference()?; self.check_host_source(&original)?;
        let number = self.require_scope_lease(lease)?;
        self.revalidate_scope_contexts()?;
        let mut bound = self.scope_contexts.as_deref();
        while bound.is_some_and(|scope| scope.number != number) { bound = bound.and_then(|scope| scope.next.as_deref()); }
        let Some(scope) = bound else { return Err(OperationError::CreditOwned) };
        let control = &lease.control;
        if control.is_closed() { return Err(OperationError::CreditOwned); }
        let mut next = self.storage_contexts.as_deref_mut(); let mut selected = None;
        while let Some(context) = next {
            if scope.context.matches(context) { selected = Some(context); break; }
            next = context.next.as_deref_mut();
        }
        let Some(context) = selected else { self.admission_closed = true; return Err(OperationError::CreditOwned) };
        let Some(journal) = self.journal.as_deref_mut() else { return Err(OperationError::CreditOwned) };
        let Some(process) = self.namespace_layouts.as_deref_mut() else { return Err(OperationError::CreditOwned) };
        let attempt = match custody::Attempt::reserve_scope(&original, control) {
            Ok(attempt) => attempt, Err(error) => { self.admission_closed = true; return Err(error); }
        };
        let mut attempt = Box::new(attempt); attempt.next = self.custody_attempts.take();
        self.custody_attempts = Some(attempt);
        let Some(attempt) = self.custody_attempts.as_deref_mut() else { return Err(OperationError::CreditOwned) };
        attempt.observe(context, journal, process, path, mode);
        if control.is_closed() {
            attempt.refuse_publication(OperationError::CreditOwned);
            self.admission_closed = true; return Err(OperationError::CreditOwned);
        }
        let returned = attempt.publish();
        if returned.is_err() { self.admission_closed = true; }
        returned
    }

    /// Move queued entry owners, never handles into await temporaries. The
    /// original Host list survives cancellation of every later join observer.
    fn drain_registration(&mut self, close: bool, scope: Option<&ScopeControl>) {
        let Some(registrar) = self.registration.as_ref() else { return };
        let (mut pending, uncertain, active) = registrar.drain(close, scope);
        self.registration_uncertain |= uncertain;
        self.registration_active = active;
        while let Some(mut entry) = pending {
            pending = entry.next.take();
            entry.next = self.drivers.take();
            self.drivers = Some(entry);
        }
    }

    /// Read-only report assembly only after the actual registry is closed and
    /// every already-started factory has settled. No join/SQL is driven here.
    fn closed_readback(&mut self) -> Result<report::ClosedHostView<'_>, report::ReadbackRefusal> {
        if !self.admission_closed { return Err(report::ReadbackRefusal::AdmissionOpen); }
        let (Some(first_requested), Some(deadline)) = (self.global_first_request, self.global_deadline) else {
            return Err(report::ReadbackRefusal::NoGlobalBoundary);
        };
        self.drain_registration(true, None);
        if self.registration_active != 0 { return Err(report::ReadbackRefusal::ActiveRegistrations); }
        // Inline canonical slot exists in Host's actual Layout. The eventual
        // full F must also include caller/view/iterator frames; none is waived.
        self.report_boundary = Some(report::ReportBoundary { first_requested, deadline,
            registration_uncertain: self.registration_uncertain, global_unknown: self.global_unknown });
        Ok(report::ClosedHostView::from_closed(self))
    }

    /// Pin the process boundary and close ALL ingress before the first await.
    /// Original outer ScopeClose time is trusted owner input, never sampled
    /// later when a callback finally lets main drive joins. Cancellation of
    /// observe_global cannot reset these inline fields or reopen registration.
    fn close_global(&mut self, first_requested: Instant, requested_deadline: Instant) -> Option<Instant> {
        self.admission_closed = true;
        let first = self.global_first_request.map_or(first_requested, |old| old.min(first_requested));
        self.global_first_request = Some(first);
        let ceiling = first.checked_add(std::time::Duration::from_secs(120));
        let candidate = ceiling.map(|ceiling| requested_deadline.min(ceiling));
        match candidate {
            Some(candidate) => {
                self.global_deadline = Some(self.global_deadline.map_or(candidate, |old| old.min(candidate)));
            }
            None => self.global_unknown = true,
        }
        let mut entry = self.scopes.as_deref();
        while let Some(scope) = entry {
            if scope.control.request_shutdown(first) == ScopeObservation::Unknown { self.global_unknown = true; }
            if let Some(deadline) = self.global_deadline {
                if scope.control.pin_join_deadline(deadline).is_none() { self.global_unknown = true; }
            }
            entry = scope.next.as_deref();
        }
        // Close the registry synchronously even if no scope was installed.
        // Pending entries move whole into Host before ANY quiescence await.
        self.drain_registration(true, None);
        let mut entry = self.drivers.as_deref();
        while let Some(driver) = entry {
            if driver.scope.request_shutdown(first) == ScopeObservation::Unknown { self.global_unknown = true; }
            if let Some(deadline) = self.global_deadline {
                if driver.scope.pin_join_deadline(deadline).is_none() { self.global_unknown = true; }
            }
            driver.root.request_stop();
            entry = driver.next.as_deref();
        }
        self.global_deadline
    }

    /// Private borrowed join body only. Returned/Empty describe retained task
    /// slots, NEVER child settlement, writer evidence or permission for CLEAN.
    async fn observe_global(&mut self, first_requested: Instant, requested_deadline: Instant) -> task::TaskObservation {
        let Some(deadline) = self.close_global(first_requested, requested_deadline) else {
            return task::TaskObservation::Unknown;
        };
        if let Some(registrar) = self.registration.as_ref() {
            if !registrar.quiesce_all(deadline).await { self.global_unknown = true; }
        }
        // Capture entries from factories which began before ingress closed.
        self.drain_registration(true, None);
        self.global_unknown |= self.registration_uncertain || self.registration_active != 0;
        let mut entry = self.drivers.as_deref();
        while let Some(driver) = entry {
            driver.scope.request_shutdown(first_requested);
            driver.root.request_stop();
            entry = driver.next.as_deref();
        }
        let mut observation = task::TaskObservation::Empty;
        let mut entry = self.drivers.as_deref_mut();
        while let Some(driver) = entry {
            self.global_unknown |= driver.scope.uncertain.load(Ordering::Acquire);
            let returned = if driver.registration_refusal.is_some() {
                task::TaskObservation::Unknown
            } else {
                let scope_deadline = match driver.scope.join_ceiling() {
                    Some(scope) => deadline.min(scope),
                    None => { self.global_unknown = true; deadline }
                };
                driver.task.observe(scope_deadline, driver.original.allowance().deadline()).await
            };
            if returned != task::TaskObservation::Returned { self.global_unknown = true; }
            else { observation = task::TaskObservation::Returned; }
            entry = driver.next.as_deref_mut();
        }
        if self.global_unknown { task::TaskObservation::Unknown } else { observation }
    }

    /// Private source body for selected-scope observation. Actual public report,
    /// journal and complete creation fit remain separate unfinished obligations.
    /// No registry/caller mutex guard or ownership MOVE crosses this await.
    async fn observe_scope(
        &mut self, lease: &SeatHostLease, first_requested: Instant, requested_deadline: Instant,
    ) -> task::TaskObservation {
        let Some(actual) = self.registration.as_ref() else { return task::TaskObservation::Unknown };
        let Some(leased) = lease.registration.upgrade() else { return task::TaskObservation::Unknown };
        if !Arc::ptr_eq(actual, &leased) { return task::TaskObservation::Unknown; }
        // The trusted outer cleanup owner must pass its retained FIRST close
        // timestamp (Server's ScopeClose), not sample a later owner-join time.
        // This private input does not expose registration or a new public API.
        lease.control.request_shutdown(first_requested);
        let candidate = self.global_deadline.map_or(requested_deadline,
            |global| global.min(requested_deadline));
        // Retain the shortest owner boundary BEFORE the first await. A
        // cancelled observer cannot restart a shorter prior join window.
        let Some(requested_deadline) = lease.control.pin_join_deadline(candidate) else {
            return task::TaskObservation::Unknown;
        };
        if !leased.quiesce(&lease.control, requested_deadline).await {
            lease.control.uncertain.store(true, Ordering::Release);
        }
        self.drain_registration(false, Some(&lease.control));
        let mut observation = if self.registration_uncertain || self.registration_active != 0 {
            task::TaskObservation::Unknown
        } else { task::TaskObservation::Empty };
        if self.global_unknown || lease.control.uncertain.load(Ordering::Acquire) { observation = task::TaskObservation::Unknown; }
        let mut entry = self.drivers.as_deref_mut();
        while let Some(driver) = entry {
            if Arc::ptr_eq(&driver.scope, &lease.control) {
                driver.root.request_stop();
                let returned = if driver.registration_refusal.is_some() {
                    task::TaskObservation::Unknown
                } else {
                    driver.task.observe(requested_deadline, driver.original.allowance().deadline()).await
                };
                if returned != task::TaskObservation::Returned {
                    observation = task::TaskObservation::Unknown;
                } else if observation == task::TaskObservation::Empty {
                    observation = task::TaskObservation::Returned;
                }
            }
            entry = driver.next.as_deref_mut();
        }
        // Returned is only a task observation, never a clean child/writer record.
        observation
    }
}

struct DriverEntry {
    root: crate::driver::owned_driver_task::DriverRoot,
    task: task::RetainedTask<crate::driver::owned_driver_task::DriverExit>,
    registration_refusal: Option<crate::operation::OperationError>,
    scope: Arc<ScopeControl>,
    original: crate::operation::Operation,
    next: Option<Box<DriverEntry>>,
}

impl Drop for DriverEntry {
    fn drop(&mut self) {
        // Terminal deallocation only; this does not drive a join or establish
        // task/child/writer settlement. Original explicit joins remain owed.
        let mut next = self.next.take();
        while let Some(mut entry) = next { next = entry.next.take(); }
    }
}

/// An observation of scope closure, not a join, journal or healthy-stop result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeObservation {
    HostJoinPending,
    Unknown,
}

/// Opaque scope reference only. It contains no Handle or join/result owner.
#[derive(Clone)]
pub struct SeatHostLease {
    control: Arc<ScopeControl>,
    registration: Weak<registration::Registrar>,
}

impl SeatHostLease {
    /// Internal factory seam only. Weak cannot prolong the process owner; a
    /// stale lease refuses without runtime discovery, allocation or fallback.
    pub(crate) fn register_driver(
        &self,
        original: &crate::operation::Operation,
        pending: &mut Option<crate::driver::owned_driver_task::OwnedCaller>,
    ) -> Result<crate::driver::owned_driver_task::CommandEndpoint, crate::operation::OperationError> {
        self.registration.upgrade().ok_or(crate::operation::OperationError::CreditOwned)?
            .register(&self.control, original, pending)
    }

    /// Idempotent nonjoining scope cancellation. The original process owner
    /// must still drive actual joins and retain the writer settlement lane.
    pub fn request_shutdown(&self) -> ScopeObservation {
        self.control.request_shutdown(Instant::now())
    }
}

struct ScopeEntry {
    control: Arc<ScopeControl>,
    next: Option<Box<ScopeEntry>>,
}

impl Drop for ScopeEntry {
    fn drop(&mut self) {
        let mut next = self.next.take();
        while let Some(mut entry) = next { next = entry.next.take(); }
    }
}

/// Host owns the slot and context anchor; this record only borrows them. Weak
/// references do not prolong the retained filesystem/process owner graph.
#[cfg(windows)]
struct ScopeContextEntry {
    control: Weak<ScopeControl>,
    context: namespace::ScopeContext,
    number: i64,
    next: Option<Box<Self>>,
}

#[cfg(windows)]
impl Drop for ScopeContextEntry {
    fn drop(&mut self) {
        let mut next = self.next.take();
        while let Some(mut entry) = next { next = entry.next.take(); }
    }
}

impl ScopeEntry {
    /// Complete cost of these two custody allocations only, not the complete
    /// scope/lease/member/report cost or an admitted creation count.
    fn custody_bytes() -> Result<usize, crate::operation::OperationError> {
        std::alloc::Layout::new::<Self>().size()
            .checked_add(crate::driver::owned_driver_task::own_arc_layout::<ScopeControl>()?.size())
            .ok_or(crate::operation::OperationError::CaptureOverflow)
    }

    fn reserve_unpublished(original: &original::HostRef)
        -> Result<Box<Self>, crate::operation::OperationError>
    {
        original::prepay(original, std::alloc::Layout::new::<Self>())?;
        original::prepay(original, crate::driver::owned_driver_task::own_arc_layout::<ScopeControl>()?)?;
        // No fallible observation, await or user callback occurs between the
        // first allocation and the whole node returned to its original owner.
        let control = Arc::new(ScopeControl::unpublished());
        Ok(Box::new(Self { control, next: None }))
    }
}

/// Private retained-boundary readback for later report assembly. This is NOT
/// terminal task/child/writer evidence, a frozen census or permission for CLEAN.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScopeJoinBoundary {
    first_requested: Instant,
    deadline: Instant,
    owner_deadline_pinned: bool,
    uncertain: bool,
}

/// Workers can only wake the private driver task. No public wait future is
/// exposed by the lease. Scope allocation/installation is not yet connected.
pub(crate) struct ScopeControl {
    closed: AtomicBool,
    first_request: Mutex<Option<Instant>>,
    owner_deadline: Mutex<Option<Instant>>,
    uncertain: AtomicBool,
    driver_wake: Notify,
    durable_open: AtomicBool,
    registration_active: std::sync::atomic::AtomicUsize,
    registration_timeout: std::sync::OnceLock<tokio::time::error::Elapsed>,
}

impl ScopeControl {
    /// Pure inline state. Only a later actual durable scope binding can open
    /// registration; allocating this control proves no namespace authority.
    fn unpublished() -> Self {
        Self {
            closed: AtomicBool::new(false), first_request: Mutex::new(None),
            owner_deadline: Mutex::new(None), uncertain: AtomicBool::new(false),
            driver_wake: Notify::new(), durable_open: AtomicBool::new(false),
            registration_active: std::sync::atomic::AtomicUsize::new(0),
            registration_timeout: std::sync::OnceLock::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn source_control() -> Arc<Self> {
        Arc::new(Self {
            closed: AtomicBool::new(false), first_request: Mutex::new(None),
            owner_deadline: Mutex::new(None),
            uncertain: AtomicBool::new(false), driver_wake: Notify::new(),
            durable_open: AtomicBool::new(false),
            registration_active: std::sync::atomic::AtomicUsize::new(0),
            registration_timeout: std::sync::OnceLock::new(),
        })
    }

    pub(crate) fn is_closed(&self) -> bool { self.closed.load(Ordering::Acquire) }

    fn join_ceiling(&self) -> Option<Instant> {
        self.join_boundary().map(|boundary| boundary.deadline)
    }

    fn join_boundary(&self) -> Option<ScopeJoinBoundary> {
        // Both readers take locks in first-request -> owner-deadline order.
        // No guard survives this synchronous read or reaches a wake/await.
        let first = self.first_request.lock().map_err(|_| {
            self.uncertain.store(true, Ordering::Release);
        }).ok()?;
        let first_requested = (*first)?;
        let owner = self.owner_deadline.lock().map_err(|_| {
            self.uncertain.store(true, Ordering::Release);
        }).ok()?;
        let ceiling = first_requested.checked_add(std::time::Duration::from_secs(120))
            .or_else(|| { self.uncertain.store(true, Ordering::Release); None })?;
        Some(ScopeJoinBoundary { first_requested,
            deadline: owner.map_or(ceiling, |owner| owner.min(ceiling)),
            owner_deadline_pinned: owner.is_some(),
            uncertain: self.uncertain.load(Ordering::Acquire) })
    }

    fn pin_join_deadline(&self, requested_deadline: Instant) -> Option<Instant> {
        let first = self.first_request.lock().map_err(|_| {
            self.uncertain.store(true, Ordering::Release);
        }).ok()?;
        let ceiling = (*first)?.checked_add(std::time::Duration::from_secs(120))
            .or_else(|| { self.uncertain.store(true, Ordering::Release); None })?;
        let mut owner = self.owner_deadline.lock().map_err(|_| {
            self.uncertain.store(true, Ordering::Release);
        }).ok()?;
        let candidate = requested_deadline.min(ceiling);
        let retained = owner.map_or(candidate, |old| old.min(candidate));
        *owner = Some(retained);
        Some(retained)
    }

    /// ONLY our private driver task can await this; leases expose no future.
    pub(crate) async fn closed(&self) {
        loop {
            let wake = self.driver_wake.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            if self.is_closed() { return; }
            wake.await;
        }
    }
    fn request_shutdown(&self, requested: Instant) -> ScopeObservation {
        self.closed.store(true, Ordering::Release);
        let observation = {
            match self.first_request.lock() {
                Ok(mut first) => {
                    // Repeated cleanup may shorten, never extend, this boundary.
                    *first = Some(first.map_or(requested, |old| old.min(requested)));
                    ScopeObservation::HostJoinPending
                }
                Err(_) => {
                    self.uncertain.store(true, Ordering::Release);
                    ScopeObservation::Unknown
                }
            }
        };
        // The guard is gone before a potentially synchronous wake.
        self.driver_wake.notify_waiters();
        if self.uncertain.load(Ordering::Acquire) {
            ScopeObservation::Unknown
        } else {
            observation
        }
    }
}

#[cfg(test)]
mod controls {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn public_custody_mint_refuses_without_complete_original_and_retains_first_refusal_before_io() {
        Handle::current().spawn(async {
            let mut host = SeatHost::new(Handle::current());
            let lease = SeatHostLease { control: ScopeControl::source_control(), registration: Weak::new() };
            let path = std::path::Path::new("C:\\unopened-custody-fixture\\campaign.sqlite");
            #[cfg(windows)] let expected = std::io::ErrorKind::Other;
            #[cfg(not(windows))] let expected = std::io::ErrorKind::Unsupported;
            assert_eq!(host.mint_campaign_custody(&lease, path, true).err(), Some(expected));
            assert_eq!(host.bridge_refusal, Some(expected));
            assert!(host.admission_closed && host.journal.is_none() && host.registration.is_none() && host.scopes.is_none());
            #[cfg(windows)] {
                assert_eq!(host.host_source_refusal, Some(crate::operation::OperationError::CreditOwned));
                assert!(host.custody_attempts.is_none() && host.member_files.is_none() && host.storage_contexts.is_none());
            }
            assert_eq!(host.mint_campaign_custody(&lease, path, false).err(), Some(expected));
            assert!(host.journal.is_none());
            // No filesystem fixture is created; this checks the actual public
            // refusal route, not a production fit, lease or successful mint.
        }).await.unwrap();
    }
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_scope_join_keeps_shorter_owner_deadline_before_quiescence() {
        Handle::current().spawn(async {
            use std::future::{Future, poll_fn};
            use std::task::Poll;
            let mut host = SeatHost::new(Handle::current());
            host.registration = Some(Arc::new(registration::Registrar::new(host.runtime.clone())));
            let control = ScopeControl::source_control();
            let lease = SeatHostLease { control: control.clone(),
                registration: Arc::downgrade(host.registration.as_ref().unwrap()) };
            host.scopes = Some(Box::new(ScopeEntry { control: control.clone(), next: None }));
            let first = Instant::now();
            let shortest = first + Duration::from_secs(20);
            // Explicit active-count fixture, not a claimed real factory race.
            // The actual borrowed Host observer is polled into quiescence.
            control.registration_active.store(1, Ordering::Release);
            {
                let future = host.observe_scope(&lease, first, shortest);
                tokio::pin!(future);
                poll_fn(|context| {
                    assert!(future.as_mut().poll(context).is_pending());
                    Poll::Ready(())
                }).await;
                assert_eq!(control.join_boundary(), Some(ScopeJoinBoundary {
                    first_requested: first, deadline: shortest,
                    owner_deadline_pinned: true, uncertain: false,
                }));
                // Drop only the actual borrowed observer, retaining Host.
            }
            control.registration_active.store(0, Ordering::Release);
            assert_eq!(host.observe_scope(&lease, first + Duration::from_secs(2),
                first + Duration::from_secs(100)).await, task::TaskObservation::Empty);
            assert_eq!(control.join_ceiling(), Some(shortest));
            assert_eq!(control.join_boundary().unwrap().first_requested, first);
            assert!(Arc::ptr_eq(&host.scopes.as_ref().unwrap().control, &control));
            assert!(host.journal.is_none() && host.drivers.is_none());
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scope_boundary_readback_combines_original_close_and_shortest_global_owner_request() {
        Handle::current().spawn(async {
            let mut host = SeatHost::new(Handle::current());
            host.registration = Some(Arc::new(registration::Registrar::new(host.runtime.clone())));
            let scope = ScopeControl::source_control();
            let other = ScopeControl::source_control();
            let now = Instant::now();
            assert!(scope.join_boundary().is_none());
            scope.request_shutdown(now);
            assert_eq!(scope.join_boundary(), Some(ScopeJoinBoundary { first_requested: now,
                deadline: now + Duration::from_secs(120), owner_deadline_pinned: false, uncertain: false }));
            assert_eq!(scope.pin_join_deadline(now + Duration::from_secs(90)), Some(now + Duration::from_secs(90)));
            // Earlier trusted outer close arrives after a borrowed request.
            let earlier = now.checked_sub(Duration::from_secs(60)).unwrap();
            scope.request_shutdown(earlier);
            assert_eq!(scope.join_ceiling(), Some(now + Duration::from_secs(60)));
            host.scopes = Some(Box::new(ScopeEntry { control: scope.clone(),
                next: Some(Box::new(ScopeEntry { control: other.clone(), next: None })) }));
            assert_eq!(host.close_global(now, now + Duration::from_secs(10)), Some(now + Duration::from_secs(10)));
            let boundary = scope.join_boundary().unwrap();
            assert_eq!(boundary.first_requested, earlier);
            assert_eq!(boundary.deadline, now + Duration::from_secs(10));
            assert!(boundary.owner_deadline_pinned && !boundary.uncertain);
            assert_eq!(other.join_ceiling(), Some(boundary.deadline));
            assert_eq!(host.observe_global(now, now + Duration::from_secs(100)).await, task::TaskObservation::Empty);
            assert_eq!(scope.join_boundary(), Some(boundary));
            scope.uncertain.store(true, Ordering::Release); // Sticky prior-failure fixture.
            assert!(scope.join_boundary().unwrap().uncertain);
            assert_eq!(scope.pin_join_deadline(now + Duration::from_secs(100)), Some(boundary.deadline));
            assert!(scope.join_boundary().unwrap().uncertain);
            // Boundary facts are not writer evidence, settlement or CLEAN.
            assert!(host.journal.is_none());
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn global_close_closes_every_scope_and_never_restarts_the_process_deadline() {
        Handle::current().spawn(async {
            let mut host = SeatHost::new(Handle::current());
            host.registration = Some(Arc::new(registration::Registrar::new(host.runtime.clone())));
            let early = ScopeControl::source_control();
            let late = ScopeControl::source_control();
            host.scopes = Some(Box::new(ScopeEntry { control: early.clone(),
                next: Some(Box::new(ScopeEntry { control: late.clone(), next: None })) }));
            let now = Instant::now();
            let first = now.checked_sub(Duration::from_secs(20)).unwrap();
            early.request_shutdown(first.checked_sub(Duration::from_secs(10)).unwrap());
            assert_eq!(host.close_global(first, now + Duration::from_secs(600)),
                Some(first + Duration::from_secs(120)));
            assert!(host.admission_closed && early.is_closed() && late.is_closed());
            assert_eq!(early.join_ceiling(), Some(first + Duration::from_secs(110)));
            assert_eq!(late.join_ceiling(), Some(first + Duration::from_secs(120)));
            assert_eq!(host.observe_global(now, now + Duration::from_secs(600)).await,
                task::TaskObservation::Empty);
            assert_eq!(host.global_first_request, Some(first));
            assert_eq!(host.global_deadline, Some(first + Duration::from_secs(120)));
            assert_eq!(host.close_global(now, now + Duration::from_secs(2)),
                Some(now + Duration::from_secs(2)));
            assert_eq!(host.close_global(now, now + Duration::from_secs(600)),
                Some(now + Duration::from_secs(2)));
            let (_, uncertain, active) = host.registration.as_ref().unwrap().drain(false, None);
            assert!(!uncertain); assert_eq!(active, 0);
            // Task Empty is NOT NoDurableScope, writer ACK or CLEAN proof.
            assert!(host.journal.is_none());
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn global_unknown_is_sticky_across_repeated_and_selected_observation() {
        Handle::current().spawn(async {
            let mut host = SeatHost::new(Handle::current());
            host.registration = Some(Arc::new(registration::Registrar::new(host.runtime.clone())));
            let control = ScopeControl::source_control();
            let lease = SeatHostLease { control: control.clone(),
                registration: Arc::downgrade(host.registration.as_ref().unwrap()) };
            host.scopes = Some(Box::new(ScopeEntry { control, next: None }));
            let now = Instant::now();
            host.global_unknown = true; // Fixed retained prior failure fixture.
            assert_eq!(host.observe_global(now, now + Duration::from_secs(1)).await,
                task::TaskObservation::Unknown);
            assert_eq!(host.observe_global(now, now + Duration::from_secs(600)).await,
                task::TaskObservation::Unknown);
            assert_eq!(host.observe_scope(&lease, now, now + Duration::from_secs(600)).await,
                task::TaskObservation::Unknown);
            assert_eq!(host.global_deadline, Some(now + Duration::from_secs(1)));
            assert!(host.global_unknown && host.admission_closed);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn owner_join_uses_original_outer_close_time_instead_of_later_lease_request() {
        Handle::current().spawn(async {
            let mut host = SeatHost::new(Handle::current());
            host.registration = Some(Arc::new(registration::Registrar::new(host.runtime.clone())));
            let lease = SeatHostLease {
                control: ScopeControl::source_control(),
                registration: Arc::downgrade(host.registration.as_ref().unwrap()),
            };
            let now = Instant::now();
            let first = now.checked_sub(Duration::from_secs(60)).unwrap();
            assert_eq!(lease.request_shutdown(), ScopeObservation::HostJoinPending);
            assert_eq!(host.observe_scope(&lease, first, now + Duration::from_secs(120)).await,
                task::TaskObservation::Empty);
            assert_eq!(*lease.control.first_request.lock().unwrap(), Some(first));
            assert_eq!(lease.control.join_ceiling(), first.checked_add(Duration::from_secs(120)));
            assert_eq!(host.observe_scope(&lease, now, now + Duration::from_secs(600)).await,
                task::TaskObservation::Empty);
            assert_eq!(*lease.control.first_request.lock().unwrap(), Some(first));
            assert_eq!(lease.control.join_ceiling(), first.checked_add(Duration::from_secs(120)));
        }).await.unwrap();
    }

    // These controls are SOURCE ONLY until the joint STRICT slot. No native
    // resource or production lease is created by this fixture.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scope_observers_only_close_and_never_extend_the_first_boundary() {
        let runtime = Handle::current();
        let control = runtime.spawn(async {
            let scope = Arc::new(ScopeControl {
                closed: AtomicBool::new(false),
                first_request: Mutex::new(None),
                owner_deadline: Mutex::new(None),
                uncertain: AtomicBool::new(false),
                driver_wake: Notify::new(),
                durable_open: AtomicBool::new(false),
                registration_active: std::sync::atomic::AtomicUsize::new(0),
                registration_timeout: std::sync::OnceLock::new(),
            });
            let observer = SeatHostLease { control: scope.clone(), registration: Weak::new() };
            let alias = observer.clone();
            let first = Instant::now();
            assert_eq!(scope.request_shutdown(first), ScopeObservation::HostJoinPending);
            assert_eq!(
                scope.request_shutdown(first + Duration::from_secs(60)),
                ScopeObservation::HostJoinPending,
            );
            assert_eq!(*scope.first_request.lock().unwrap(), Some(first));
            drop(observer);
            assert!(Arc::ptr_eq(&alias.control, &scope));
            assert!(scope.closed.load(Ordering::Acquire));
            assert_eq!(alias.request_shutdown(), ScopeObservation::HostJoinPending);
            assert_eq!(*scope.first_request.lock().unwrap(), Some(first));
            assert!(!scope.uncertain.load(Ordering::Acquire));
        });
        control.await.unwrap();
    }

}
