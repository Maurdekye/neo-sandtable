//! Host-created attempt/pointee retention behind the sanctioned Host bridge.
//! No Server adapter. Complete Host fit and production lease issuance stay closed.

use super::{CampaignFileCustody, namespace, original};
use crate::operation::OperationError;
use std::alloc::Layout;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub(super) struct Attempt {
    original: original::ChargeRef,
    scope: Option<std::sync::Weak<super::ScopeControl>>,
    observed: Option<Result<namespace::Observation, OperationError>>,
    prefix: Option<Box<namespace::PreopenRoot>>,
    custody: Option<Arc<CampaignFileCustody>>,
    publication_attempted: bool,
    publication: Option<Result<(), OperationError>>,
    pairing: Option<Result<namespace::Observation, OperationError>>,
    post_commit: Option<Result<namespace::Observation, OperationError>>,
    member_files: Option<Box<namespace::MemberFilesRoot>>,
    membership_attempted: bool,
    membership: Option<Result<(), OperationError>>,
    pub(super) next: Option<Box<Self>>,
}

/// Borrows only original Host-side slots. Published raw guard results remain
/// in the Arc's private mutex; no copied path/tuple or terminal verdict here.
pub(super) struct Readback<'a> {
    pub(super) observed: Option<&'a Result<namespace::Observation, OperationError>>,
    pub(super) unpublished_prefix: Option<&'a namespace::PreopenRoot>,
    pub(super) custody: Option<&'a Arc<CampaignFileCustody>>,
    pub(super) publication: Option<&'a Result<(), OperationError>>,
    pub(super) pairing: Option<&'a Result<namespace::Observation, OperationError>>,
    pub(super) post_commit: Option<&'a Result<namespace::Observation, OperationError>>,
    pub(super) member_files: Option<&'a namespace::MemberFilesRoot>,
    pub(super) membership: Option<&'a Result<(), OperationError>>,
}

// Private frame, not a new cross-crate DTO or caller byte budget. Its actual
// layout/temporary frame belongs in the complete Host F inventory.
pub(super) struct MemberInputs<'a> {
    pub(super) number: i64,
    pub(super) campaign: [&'a str; 2],
    pub(super) pins: [&'a str; 3],
    pub(super) archive: &'a crate::failed_evidence::CampaignArchive,
    pub(super) control: &'a super::ScopeControl,
}

impl Attempt {
    pub(super) fn reserve(source: &impl original::Source) -> Result<Self, OperationError> {
        original::prepay(source, Layout::new::<Self>())?;
        Ok(Self { original: source.charge_ref(), scope: None, observed: None, prefix: None,
            custody: None, publication_attempted: false, publication: None, pairing: None, post_commit: None,
            member_files: None, membership_attempted: false, membership: None, next: None })
    }

    pub(super) fn reserve_scope(source: &original::HostRef, control: &Arc<super::ScopeControl>) -> Result<Self, OperationError> {
        let mut attempt = Self::reserve(source)?; attempt.scope = Some(Arc::downgrade(control)); Ok(attempt)
    }

    /// Original attempt slot must already be installed in Host before this.
    pub(super) fn observe(&mut self, context: &mut namespace::ContextRoot,
        journal: &mut super::journal::JournalRoot, process: &mut namespace::LayoutRoot,
        path: &Path, mode: namespace::PreopenMode)
    {
        if self.observed.is_some() { return; }
        let root = match namespace::PreopenRoot::reserve(&self.original) {
            Ok(root) => root,
            Err(error) => { self.observed = Some(Err(error)); return; }
        };
        self.prefix = Some(Box::new(root));
        let Some(root) = self.prefix.as_deref_mut() else {
            self.observed = Some(Err(OperationError::CreditOwned)); return;
        };
        self.observed = Some(Ok(root.observe_once(context, journal, process, path, mode)));
    }

    /// Charge both future allocations BEFORE either is made. Failed credit or
    /// path checks leave the exact original prefix in this attempt, unmoved.
    pub(super) fn publish(&mut self) -> Result<Arc<CampaignFileCustody>, OperationError> {
        if self.publication_attempted { return Err(OperationError::CreditOwned); }
        self.publication_attempted = true;
        let returned = self.publish_once();
        self.publication = Some(returned.as_ref().map(|_| ()).map_err(|error| *error));
        returned
    }

    pub(super) fn refuse_publication(&mut self, error: OperationError) {
        if self.publication_attempted { return; }
        self.publication_attempted = true; self.publication = Some(Err(error));
    }

    pub(super) fn prefix(&self) -> Option<&namespace::PreopenRoot> { self.prefix.as_deref() }
    pub(super) fn published(&self) -> Option<&Arc<CampaignFileCustody>> { self.custody.as_ref() }

    pub(super) fn readback(&self) -> Readback<'_> {
        Readback { observed: self.observed.as_ref(), unpublished_prefix: self.prefix.as_deref(),
            custody: self.custody.as_ref(), publication: self.publication.as_ref(), pairing: self.pairing.as_ref(),
            post_commit: self.post_commit.as_ref(), member_files: self.member_files.as_deref(), membership: self.membership.as_ref() }
    }

    pub(super) fn register_member(&mut self, context: &mut namespace::ContextRoot,
        journal: &mut super::journal::JournalRoot, process: &mut namespace::LayoutRoot,
        input: MemberInputs<'_>) -> Result<(), OperationError>
    {
        if self.membership_attempted || self.pairing.is_some() { return Err(OperationError::CreditOwned); }
        self.membership_attempted = true;
        let returned = self.register_member_once(context, journal, process, input);
        self.membership = Some(returned); returned
    }

    fn register_member_once(&mut self, context: &mut namespace::ContextRoot,
        journal: &mut super::journal::JournalRoot, process: &mut namespace::LayoutRoot,
        input: MemberInputs<'_>) -> Result<(), OperationError>
    {
        let custody = self.custody.as_ref().ok_or(OperationError::CreditOwned)?;
        let root = namespace::MemberFilesRoot::reserve(&self.original)?;
        self.member_files = Some(Box::new(root)); // SAME attempt owns all member failure prefixes.
        let files = self.member_files.as_deref_mut().ok_or(OperationError::CreditOwned)?;
        if files.observe_once(context, journal, process, custody.protected_path(), input.archive.database_for_host()) != namespace::Observation::Observed
            || files.observe_archive_roots(input.archive, context, process) != namespace::Observation::Observed
        { return Err(OperationError::CreditOwned); }
        let paired = Self::check_pair(&self.original, custody, files, context, journal, process);
        self.pairing = Some(paired);
        if paired? != namespace::Observation::Observed || input.control.is_closed() { return Err(OperationError::CreditOwned); }
        let storage = custody.protected_path().to_str().ok_or(OperationError::CreditOwned)?;
        if journal.admit_paired_member(input.number, input.campaign, storage, input.pins) != super::journal::Observation::Configured {
            return Err(OperationError::CreditOwned);
        }
        let checked = Self::check_pair(&self.original, custody, files, context, journal, process);
        self.post_commit = Some(checked);
        if checked? != namespace::Observation::Observed || input.control.is_closed() { return Err(OperationError::CreditOwned); }
        Ok(())
    }

    pub(super) fn pair(&mut self, files: &mut namespace::MemberFilesRoot,
        context: &mut namespace::ContextRoot, journal: &mut super::journal::JournalRoot,
        process: &mut namespace::LayoutRoot) -> Result<namespace::Observation, OperationError>
    {
        if self.pairing.is_some() { return Err(OperationError::CreditOwned); }
        let returned = self.pair_once(files, context, journal, process);
        self.pairing = Some(returned); // Fixed result, not a membership token.
        returned
    }

    pub(super) fn revalidate_after_commit(&mut self, files: &mut namespace::MemberFilesRoot,
        context: &mut namespace::ContextRoot, journal: &mut super::journal::JournalRoot,
        process: &mut namespace::LayoutRoot) -> Result<namespace::Observation, OperationError>
    {
        if self.post_commit.is_some() || !matches!(self.pairing, Some(Ok(namespace::Observation::Observed))) {
            return Err(OperationError::CreditOwned);
        }
        let returned = self.pair_once(files, context, journal, process); self.post_commit = Some(returned); returned
    }

    fn pair_once(&self, files: &mut namespace::MemberFilesRoot,
        context: &mut namespace::ContextRoot, journal: &mut super::journal::JournalRoot,
        process: &mut namespace::LayoutRoot) -> Result<namespace::Observation, OperationError>
    {
        let custody = self.custody.as_ref().ok_or(OperationError::CreditOwned)?;
        Self::check_pair(&self.original, custody, files, context, journal, process)
    }

    fn check_pair(original: &original::ChargeRef, custody: &CampaignFileCustody,
        files: &mut namespace::MemberFilesRoot, context: &mut namespace::ContextRoot,
        journal: &mut super::journal::JournalRoot, process: &mut namespace::LayoutRoot)
        -> Result<namespace::Observation, OperationError>
    {
        original.check()?;
        if !custody.original.same_instance(original) { return Err(OperationError::CreditOwned); }
        let mut root = custody.prefix.lock().map_err(|_| OperationError::CreditOwned)?;
        // No await, poll, Notify, user callback or charge/waker under this lock.
        Ok(root.pair_observed_files(files, context, journal, process))
    }

    fn publish_once(&mut self) -> Result<Arc<CampaignFileCustody>, OperationError> {
        if !matches!(self.observed, Some(Ok(namespace::Observation::Observed))) || self.custody.is_some() {
            return Err(OperationError::CreditOwned);
        }
        self.original.check()?;
        let root = self.prefix.as_deref().ok_or(OperationError::CreditOwned)?;
        let path = root.protected_path().map_err(|_| OperationError::CreditOwned)?;
        let owned = Layout::array::<u8>(path.as_os_str().as_encoded_bytes().len())
            .map_err(|_| OperationError::CaptureOverflow)?;
        original::prepay(&self.original, crate::driver::owned_driver_task::own_arc_layout::<CampaignFileCustody>()?)?;
        original::prepay(&self.original, owned)?;
        // This separate immutable path copy belongs to Host, just like the Arc
        // pointee. Server borrowing it never clones or rebills its backing.
        let protected_path = path.to_path_buf();
        let prefix = self.prefix.take().ok_or(OperationError::CreditOwned)?;
        self.custody = Some(Arc::new(CampaignFileCustody { protected_path,
            original: self.original.clone(), prefix: Mutex::new(prefix), scope: self.scope.clone() }));
        // Canonical parent owns the Arc BEFORE returning its inline clone.
        self.custody.as_ref().cloned().ok_or(OperationError::CreditOwned)
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        // Terminal deallocation only: no query, join, cleanup or meter refund.
        let mut next = self.next.take();
        while let Some(mut attempt) = next { next = attempt.next.take(); }
    }
}
