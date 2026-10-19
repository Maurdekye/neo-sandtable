//! Private Host-owned file observations, not namespace authority/custody proof.
//! Ancestor/leaf guards, SQLite path association and publication revalidation
//! remain mandatory. Nothing here produces a verified namespace or a lease.
//!
//! Lead m29a: admission supports Windows local NTFS roots only. Filesystem
//! type is unobservable here: unsupported ReFS/FAT/exFAT/virtual/cloud storage
//! is UNDETECTED. Interior volume mounts/junctions are not detected by the
//! domain rule. This is not isolation proof; containment owns that obligation.
//! Canonical OS-returned drive spelling is classification only, never identity.
//! Network/UNC and other unparsed domains must refuse without a downgrade.

use std::alloc::Layout;
use std::fs::File;
use std::io;
#[cfg(windows)]
use std::sync::{Arc, Weak};
#[cfg(windows)]
use std::path::{Component, Path, PathBuf, Prefix};

use super::original::{ChargeRef, Source, prepay};
use crate::operation::OperationError;
#[cfg(test)]
use crate::operation::Operation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role { Directory, RegularFile }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal { Closed, Query, Open, Lock, Path, WrongKind, Link, Alias, Changed, UnsupportedIdentity, UnsupportedTarget, UnsupportedStorageDomain, AmbiguousStorageDomain, Canonicalize, CanonicalExtent }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Observation { Observed, Refused(Refusal) }

/// A spelling observation ONLY: no File, namespace authority or open permission.
#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PathObservation { Prepared, Refused(Refusal) }

/// Observed values only. Equality never confers filesystem/sandbox authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Identity { pub(super) volume: u64, pub(super) file: u64, pub(super) links: u64 }

#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirectoryRelation { Same, Contains, Within, Disjoint }

/// Classification from an actual canonical Result AFTER its identity bracket,
/// not a token or caller assertion. This pure parser alone never admits a root.
#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocalDomain { drive: u8, serial: u64 }

#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DomainRelation { SameDrive, DisjointDrives }

#[cfg(windows)]
impl LocalDomain {
    fn classify(returned_canonical: &Path, retained: &Identity) -> Result<Self, Refusal> {
        let bytes = returned_canonical.as_os_str().as_encoded_bytes();
        // Parse ONLY OS-returned \\?\X:\... . Never derive the domain from
        // the caller's drive, UNC server, volume serial alone or current dir.
        if !bounded_verbatim(returned_canonical) || bytes.len() < 7
            || !bytes.starts_with(br"\\?\") || !bytes[4].is_ascii_alphabetic()
            || bytes[5] != b':' || bytes[6] != b'\\' || retained.volume == 0
        { return Err(Refusal::UnsupportedStorageDomain); }
        let mut components = returned_canonical.components();
        if !matches!(components.next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::VerbatimDisk(_)))
        { return Err(Refusal::UnsupportedStorageDomain); }
        Ok(Self { drive: bytes[4].to_ascii_uppercase(), serial: retained.volume })
    }

    fn relation(self, other: Self) -> Result<DomainRelation, Refusal> {
        match (self.drive == other.drive, self.serial == other.serial) {
            (true, true) => Ok(DomainRelation::SameDrive),
            (false, false) => Ok(DomainRelation::DisjointDrives),
            // Same letter/different serial OR two letters/same serial is
            // ambiguous. Do not fall back to a path-prefix or file-index key.
            _ => Err(Refusal::AmbiguousStorageDomain),
        }
    }
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReservedName { Namespace, Database, Lock }

#[cfg(windows)]
impl ReservedName {
    fn text(self) -> &'static str {
        match self { Self::Namespace => ".seat-host", Self::Database => "host.sqlite", Self::Lock => "host.lock" }
    }
}

#[cfg(windows)]
enum PathFailure { Path, Credit(OperationError) }

#[cfg(windows)]
pub(super) struct JournalAssociation { _private: () }

// Allocation identity ONLY: this marker binds a connection to its retained
// owner. It never proves filesystem identity, domain, isolation or admission.
#[cfg(windows)]
pub(super) struct JournalInputs<'a> {
    pub(super) association: &'a Arc<JournalAssociation>,
    pub(super) database: &'a Path,
    pub(super) lock_path: &'a Path,
    pub(super) new_database: bool,
}

#[cfg(windows)]
fn install_child_path(
    original: &impl Source, destination: &mut Option<PathBuf>, parent: &Path, name: ReservedName,
) -> Result<(), PathFailure> {
    if destination.is_some() || !bounded_verbatim(parent) { return Err(PathFailure::Path); }
    let bytes = parent.as_os_str().as_encoded_bytes();
    let separator = usize::from(!bytes.ends_with(b"\\"));
    let size = bytes.len().checked_add(separator).and_then(|n| n.checked_add(name.text().len()))
        .filter(|n| *n <= 4096).ok_or(PathFailure::Path)?;
    let layout = Layout::array::<u8>(size).map_err(|_| PathFailure::Path)?;
    prepay(original, layout).map_err(PathFailure::Credit)?;
    *destination = Some(PathBuf::from(std::ffi::OsString::with_capacity(size)));
    let path = destination.as_mut().ok_or(PathFailure::Path)?;
    path.as_mut_os_string().push(parent.as_os_str());
    if separator != 0 { path.as_mut_os_string().push("\\"); }
    path.as_mut_os_string().push(name.text());
    Ok(())
}

#[cfg(windows)]
fn native_path_layout(path: &Path) -> Result<Layout, Refusal> {
    use std::os::windows::ffi::OsStrExt;
    let bytes = path.as_os_str().as_encoded_bytes().len();
    if !bounded_verbatim(path) || path.as_os_str().encode_wide().count() > bytes { return Err(Refusal::Path); }
    Layout::array::<u16>(bytes.checked_add(1).ok_or(Refusal::Path)?).map_err(|_| Refusal::Path)
}

/// Every node retains its actual File and raw query result independently of
/// fixed observations. No handle clone/duplication, callback or worker entry.
pub(super) struct GuardRoot {
    original: ChargeRef,
    role: Option<Role>,
    file: Option<File>,
    #[cfg(windows)]
    native_path: Option<PathBuf>,
    #[cfg(windows)]
    opened: Option<Result<File, io::Error>>,
    #[cfg(windows)]
    leaf_probe: Option<Result<std::fs::Metadata, io::Error>>,
    #[cfg(windows)]
    leaf_created: Option<bool>,
    #[cfg(windows)]
    information: Option<Result<winapi_util::file::Information, io::Error>>,
    #[cfg(not(windows))]
    metadata: Option<Result<std::fs::Metadata, io::Error>>,
    #[cfg(windows)]
    recheck_information: Option<Result<winapi_util::file::Information, io::Error>>,
    #[cfg(not(windows))]
    recheck_metadata: Option<Result<std::fs::Metadata, io::Error>>,
    credit_refusal: Option<OperationError>,
    observation: Option<Observation>,
    identity: Option<Identity>,
    pub(super) next: Option<Box<Self>>,
}

impl Drop for GuardRoot {
    fn drop(&mut self) {
        // Deallocation only. Keep the current node alive while detaching each
        // successor, so retained prefix length does not become Drop depth.
        // No query, admission, cleanup verdict, rollback or credit refund.
        let mut next = self.next.take();
        while let Some(mut root) = next { next = root.next.take(); }
    }
}

impl GuardRoot {
    pub(super) fn reserve(original: &impl Source) -> Result<Self, OperationError> {
        // Public actual types include Information/Result/File fields. All other
        // path/open/error children still require their actual Host inventory.
        // This does not extend J or use private layout mirrors/byte factors.
        prepay(original, Layout::new::<Self>())?;
        Ok(Self {
            original: original.charge_ref(), role: None, file: None,
            #[cfg(windows)] native_path: None,
            #[cfg(windows)] opened: None,
            #[cfg(windows)] leaf_probe: None,
            #[cfg(windows)] leaf_created: None,
            #[cfg(windows)] information: None,
            #[cfg(not(windows))] metadata: None,
            #[cfg(windows)] recheck_information: None,
            #[cfg(not(windows))] recheck_metadata: None,
            credit_refusal: None, observation: None, identity: None, next: None,
        })
    }

    /// Called ONLY after installing the whole node in the actual Host list.
    /// A refusal before the MOVE leaves pending untouched; a later refusal keeps
    /// the original File in this root. Never open a path or consume caller credit
    /// after ordinary admission closes. No public/generic query seam is exposed.
    pub(super) fn observe(&mut self, pending: &mut Option<File>, role: Role) -> Observation {
        if self.retained_file().is_some() || self.observation.is_some() {
            return Observation::Refused(Refusal::Closed);
        }
        #[cfg(windows)]
        if self.native_path.is_some() { return Observation::Refused(Refusal::Closed); }
        if let Err(error) = self.original.check() {
            self.credit_refusal = Some(error);
            self.observation = Some(Observation::Refused(Refusal::Closed));
            return Observation::Refused(Refusal::Closed);
        }
        let Some(file) = pending.take() else {
            self.observation = Some(Observation::Refused(Refusal::Closed));
            return Observation::Refused(Refusal::Closed);
        };
        self.role = Some(role);
        self.file = Some(file); // Actual owner BEFORE query/observation.
        let observation = self.query(role);
        self.observation = Some(observation);
        observation
    }

    fn retained_file(&self) -> Option<&File> {
        #[cfg(windows)] {
            self.file.as_ref().or_else(|| self.opened.as_ref()?.as_ref().ok())
        }
        #[cfg(not(windows))] { self.file.as_ref() }
    }

    /// Private staged producer for already-absolute owner input. It follows the
    /// journal's existing UTF-8 bound, but refuses ambiguous normal spellings.
    /// This is NOT a public path contract or proof of normalization/equivalence.
    /// Host must install this prepaid root BEFORE allocating the path child.
    #[cfg(windows)]
    pub(super) fn prepare_owner_path(&mut self, path: &Path) -> PathObservation {
        if self.file.is_some() || self.opened.is_some() || self.native_path.is_some()
            || self.observation.is_some()
        {
            return PathObservation::Refused(Refusal::Closed);
        }
        let Some((prefix, tail, bytes)) = owner_path_parts(path) else {
            self.refuse_once(Refusal::Path);
            return PathObservation::Refused(Refusal::Path);
        };
        let Ok(layout) = Layout::array::<u8>(bytes) else {
            self.refuse_once(Refusal::Path);
            return PathObservation::Refused(Refusal::Path);
        };
        if let Err(error) = prepay(&self.original, layout) {
            self.credit_refusal = Some(error);
            self.refuse_once(Refusal::Closed);
            return PathObservation::Refused(Refusal::Closed);
        }
        // Pinned OsString/Wtf8Buf requests ONE Vec<u8>(bytes). PathBuf moves it.
        // Install empty allocated owner before copying either borrowed slice.
        self.native_path = Some(PathBuf::from(std::ffi::OsString::with_capacity(bytes)));
        let Some(native) = self.native_path.as_mut() else {
            self.refuse_once(Refusal::Closed);
            return PathObservation::Refused(Refusal::Closed);
        };
        native.as_mut_os_string().push(prefix);
        native.as_mut_os_string().push(tail);
        // Neither append can exceed the prepaid exact sum; no capacity repair.
        if native.as_os_str().as_encoded_bytes().len() != bytes || !bounded_verbatim(native) {
            self.refuse_once(Refusal::Path);
            return PathObservation::Refused(Refusal::Path);
        }
        if let Err(error) = self.original.check() {
            self.credit_refusal = Some(error);
            self.refuse_once(Refusal::Closed);
            return PathObservation::Refused(Refusal::Closed);
        }
        PathObservation::Prepared
    }

    /// Continue ONLY the installed prepared root. No path clone, new root,
    /// fallback, creation or retry. Ancestor/SQLite authority is still absent.
    #[cfg(windows)]
    pub(super) fn open_prepared_existing(&mut self, role: Role) -> Observation {
        if self.file.is_some() || self.opened.is_some() || self.observation.is_some() {
            return Observation::Refused(Refusal::Closed);
        }
        let Some(path) = self.native_path.as_ref() else {
            return self.refuse_once(Refusal::Closed);
        };
        if !bounded_verbatim(path) || (role == Role::RegularFile && path.file_name().is_none()) {
            return self.refuse_once(Refusal::Path);
        }
        let bytes = path.as_os_str().as_encoded_bytes().len();
        use std::os::windows::ffi::OsStrExt;
        if path.as_os_str().encode_wide().count() > bytes {
            return self.refuse_once(Refusal::Path);
        }
        let Some(capacity) = bytes.checked_add(1) else {
            return self.refuse_once(Refusal::Path);
        };
        let Ok(layout) = Layout::array::<u16>(capacity) else {
            return self.refuse_once(Refusal::Path);
        };
        // The path child's n bytes were prepaid BEFORE its creation. Charge
        // ONLY this one pinned to_u16s(n+1) request before the actual std open.
        if let Err(error) = prepay(&self.original, layout) {
            self.credit_refusal = Some(error);
            return self.refuse_once(Refusal::Closed);
        }
        let Some(path) = self.native_path.as_ref() else {
            return self.refuse_once(Refusal::Closed);
        };
        use std::os::windows::fs::OpenOptionsExt;
        self.opened = Some(std::fs::OpenOptions::new().read(true)
            .share_mode(guard_sharing(role)).custom_flags(0x0220_0000).open(path));
        if !matches!(self.opened.as_ref(), Some(Ok(_))) {
            return self.refuse_once(Refusal::Open);
        }
        self.role = Some(role);
        let outcome = self.query(role);
        self.observation = Some(outcome);
        outcome
    }

    /// ONLY the actual .seat-host parent chain retained by LayoutRoot calls
    /// this. Probe selects one atomic open mode; it never certifies identity.
    #[cfg(windows)]
    fn open_named_leaf(&mut self, parent: &mut ChainRoot, name: ReservedName) -> Observation {
        if name == ReservedName::Namespace || self.native_path.is_some() || self.file.is_some()
            || self.opened.is_some() || self.leaf_probe.is_some() || self.observation.is_some()
        { return Observation::Refused(Refusal::Closed); }
        if let Observation::Refused(reason) = parent.revalidate() { return self.refuse_once(reason); }
        if parent.directory_endpoint().is_err() { return self.refuse_once(Refusal::WrongKind); }
        let Some(path) = parent.guards.as_ref().and_then(|guard| guard.native_path.as_deref()) else {
            return self.refuse_once(Refusal::Closed);
        };
        if path.file_name() != Some(std::ffi::OsStr::new(".seat-host")) { return self.refuse_once(Refusal::Path); }
        if let Err(error) = install_child_path(&self.original, &mut self.native_path, path, name) {
            return match error {
                PathFailure::Path => self.refuse_once(Refusal::Path),
                PathFailure::Credit(error) => { self.credit_refusal = Some(error); self.refuse_once(Refusal::Closed) }
            };
        }
        let Some(path) = self.native_path.as_ref() else { return self.refuse_once(Refusal::Closed) };
        let layout = match native_path_layout(path) { Ok(layout) => layout, Err(reason) => return self.refuse_once(reason) };
        if let Err(error) = prepay(&self.original, layout) {
            self.credit_refusal = Some(error); return self.refuse_once(Refusal::Closed);
        }
        // Retain the complete preflight result, including expected NotFound.
        // Cached metadata is NOT guarded identity/newness/absence authority.
        self.leaf_probe = Some(std::fs::symlink_metadata(path));
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        let create_new = match self.leaf_probe.as_ref() {
            Some(Ok(metadata)) if metadata.file_attributes() & 0x400 != 0 => return self.refuse_once(Refusal::Link),
            Some(Ok(metadata)) if metadata.is_file() => false,
            Some(Ok(_)) => return self.refuse_once(Refusal::WrongKind),
            Some(Err(error)) if error.kind() == io::ErrorKind::NotFound => true,
            _ => return self.refuse_once(Refusal::Query),
        };
        if let Err(error) = prepay(&self.original, layout) {
            self.credit_refusal = Some(error); return self.refuse_once(Refusal::Closed);
        }
        let Some(path) = self.native_path.as_ref() else { return self.refuse_once(Refusal::Closed) };
        // ONE actual open. A race with expected absence/existence refuses;
        // never retry OPEN_ALWAYS, truncate, overwrite or replace that result.
        self.opened = Some(std::fs::OpenOptions::new().read(true).write(true)
            .create_new(create_new).share_mode(guard_sharing(Role::RegularFile))
            .custom_flags(0x0220_0000).open(path));
        if !matches!(self.opened.as_ref(), Some(Ok(_))) { return self.refuse_once(Refusal::Open); }
        self.leaf_created = Some(create_new); // Derived from acknowledged atomic open only.
        self.role = Some(Role::RegularFile);
        let outcome = self.query(Role::RegularFile);
        self.observation = Some(outcome);
        outcome
    }

    /// Staged existing-path opener, not a namespace verifier or issuer.
    /// The root must already be installed in Host before this call. Refuse
    /// general spellings rather than enter unaccounted GetFullPathNameW growth.
    /// No public path producer or implicit canonicalize/fallback is introduced.
    #[cfg(windows)]
    pub(super) fn open_existing_verbatim(&mut self, path: &Path, role: Role) -> Observation {
        if self.file.is_some() || self.opened.is_some() || self.native_path.is_some()
            || self.observation.is_some()
        {
            return Observation::Refused(Refusal::Closed);
        }
        if !bounded_verbatim(path) || (role == Role::RegularFile && path.file_name().is_none()) {
            return self.refuse_once(Refusal::Path);
        }
        let bytes = path.as_os_str().as_encoded_bytes().len();
        use std::os::windows::ffi::OsStrExt;
        if path.as_os_str().encode_wide().count() > bytes {
            return self.refuse_once(Refusal::Path);
        }
        let Some(wide_capacity) = bytes.checked_add(1) else {
            return self.refuse_once(Refusal::Path);
        };
        let layouts = match (Layout::array::<u8>(bytes), Layout::array::<u16>(wide_capacity)) {
            (Ok(owned_path), Ok(native_call)) => (owned_path, native_call),
            _ => return self.refuse_once(Refusal::Path),
        };
        // Pinned owned clone: exact byte Vec. Pinned std open: to_u16s allocates
        // bytes+1 u16, then get_long_path returns early for this verbatim prefix.
        // No post-allocation capacity correction or allowance charged to J.
        for layout in [layouts.0, layouts.1] {
            if let Err(error) = prepay(&self.original, layout) {
                self.credit_refusal = Some(error);
                return self.refuse_once(Refusal::Closed);
            }
        }
        self.native_path = Some(path.to_path_buf());
        if let Err(error) = self.original.check() {
            self.credit_refusal = Some(error);
            return self.refuse_once(Refusal::Closed);
        }
        let Some(native_path) = self.native_path.as_ref() else {
            return self.refuse_once(Refusal::Closed);
        };
        use std::os::windows::fs::OpenOptionsExt;
        // Directory READ sharing excludes WRITE and DELETE. Regular leaves
        // permit WRITE for SQLite but exclude DELETE. Observe the reparse leaf;
        // BACKUP_SEMANTICS permits directories. Default noninheritance remains.
        self.opened = Some(std::fs::OpenOptions::new().read(true)
            .share_mode(guard_sharing(role)).custom_flags(0x0220_0000).open(native_path));
        if !matches!(self.opened.as_ref(), Some(Ok(_))) {
            return self.refuse_once(Refusal::Open);
        }
        self.role = Some(role);
        let outcome = self.query(role);
        self.observation = Some(outcome);
        outcome
    }

    #[cfg(windows)]
    pub(super) fn raw_open_error(&self) -> Option<&io::Error> {
        self.opened.as_ref()?.as_ref().err()
    }

    #[cfg(windows)]
    fn query(&mut self, role: Role) -> Observation {
        let Some(file) = self.retained_file() else { return Observation::Refused(Refusal::Closed) };
        // EXACT approved specialization: private Host, &retained std File only.
        // Pinned file.rs694B5AB3 / win.rs7AC48F92; no raw FFI/own unsafe.
        self.information = Some(winapi_util::file::information(file));
        let Some(Ok(information)) = self.information.as_ref() else {
            return Observation::Refused(Refusal::Query);
        };
        let attributes = information.file_attributes();
        // Same reparse bit used by the existing ancestor preflight. This
        // describes the opened handle, NOT the path used to obtain it.
        if attributes & 0x400 != 0 { return Observation::Refused(Refusal::Link); }
        let directory = attributes & 0x10 != 0;
        if directory != matches!(role, Role::Directory) {
            return Observation::Refused(Refusal::WrongKind);
        }
        self.record_identity(Identity {
            volume: information.volume_serial_number(),
            file: information.file_index(), links: information.number_of_links(),
        }, role)
    }

    #[cfg(unix)]
    fn query(&mut self, role: Role) -> Observation {
        use std::os::unix::fs::MetadataExt;
        let Some(file) = self.file.as_ref() else { return Observation::Refused(Refusal::Closed) };
        self.metadata = Some(file.metadata());
        let Some(Ok(metadata)) = self.metadata.as_ref() else {
            return Observation::Refused(Refusal::Query);
        };
        if metadata.file_type().is_symlink() { return Observation::Refused(Refusal::Link); }
        let matches_role = match role {
            Role::Directory => metadata.is_dir(), Role::RegularFile => metadata.is_file(),
        };
        if !matches_role { return Observation::Refused(Refusal::WrongKind); }
        self.record_identity(Identity {
            volume: metadata.dev(), file: metadata.ino(), links: metadata.nlink(),
        }, role)
    }

    #[cfg(not(any(windows, unix)))]
    fn query(&mut self, _role: Role) -> Observation {
        // Compiles on unsupported targets; NEVER silently claims equality/pass.
        Observation::Refused(Refusal::UnsupportedTarget)
    }

    fn record_identity(&mut self, identity: Identity, role: Role) -> Observation {
        if identity.file == 0 || identity.links == 0 {
            return Observation::Refused(Refusal::UnsupportedIdentity);
        }
        // Unix directory link counts normally exceed1; do not apply the regular
        // file rule to ancestors. Their alias/parent proof is separately owed.
        if role == Role::RegularFile && identity.links != 1 {
            return Observation::Refused(Refusal::Link);
        }
        self.identity = Some(identity);
        Observation::Observed
    }

    pub(super) fn identity(&self) -> Option<&Identity> { self.identity.as_ref() }

    /// Duplicate regular-file owners cannot be interpreted as distinct leaves.
    /// Directory ancestry/path equivalence is separately owed, not inferred here.
    pub(super) fn aliases_regular_file(&self, other: &Self) -> bool {
        if self.role != Some(Role::RegularFile) || other.role != Some(Role::RegularFile) {
            return false;
        }
        match (self.identity(), other.identity()) {
            (Some(left), Some(right)) => left.volume == right.volume && left.file == right.file,
            _ => false,
        }
    }

    pub(super) fn refuse_alias(&mut self) -> Observation {
        self.refuse_once(Refusal::Alias)
    }

    fn refuse_once(&mut self, reason: Refusal) -> Observation {
        if let Some(observation @ Observation::Refused(_)) = self.observation {
            return observation;
        }
        let observation = Observation::Refused(reason);
        self.observation = Some(observation);
        observation
    }

    /// Re-observe the SAME owned handle, not a path or a replacement File.
    /// Initial result and identity never change. Only a prior successful recheck
    /// may be superseded; first failure stops all later queries and stays owned.
    /// This still says nothing about whether SQLite opened this same identity.
    pub(super) fn revalidate_handle(&mut self) -> Observation {
        if let Some(observation @ Observation::Refused(_)) = self.observation {
            return observation;
        }
        let (Some(expected), Some(role)) = (self.identity, self.role) else {
            return self.refuse_once(Refusal::Closed);
        };
        if let Err(error) = self.original.check() {
            self.credit_refusal = Some(error);
            return self.refuse_once(Refusal::Closed);
        }
        let actual = match self.query_recheck(role) {
            Ok(actual) => actual,
            Err(reason) => return self.refuse_once(reason),
        };
        if actual.file == 0 || actual.links == 0 {
            return self.refuse_once(Refusal::UnsupportedIdentity);
        }
        if role == Role::RegularFile && actual.links != 1 {
            return self.refuse_once(Refusal::Link);
        }
        if actual != expected { return self.refuse_once(Refusal::Changed); }
        Observation::Observed
    }

    #[cfg(windows)]
    fn query_recheck(&mut self, role: Role) -> Result<Identity, Refusal> {
        let file = self.retained_file().ok_or(Refusal::Closed)?;
        self.recheck_information = Some(winapi_util::file::information(file));
        let information = self.recheck_information.as_ref()
            .ok_or(Refusal::Closed)?.as_ref().map_err(|_| Refusal::Query)?;
        let attributes = information.file_attributes();
        if attributes & 0x400 != 0 { return Err(Refusal::Link); }
        if (attributes & 0x10 != 0) != matches!(role, Role::Directory) {
            return Err(Refusal::WrongKind);
        }
        Ok(Identity { volume: information.volume_serial_number(),
            file: information.file_index(), links: information.number_of_links() })
    }

    #[cfg(unix)]
    fn query_recheck(&mut self, role: Role) -> Result<Identity, Refusal> {
        use std::os::unix::fs::MetadataExt;
        let file = self.file.as_ref().ok_or(Refusal::Closed)?;
        self.recheck_metadata = Some(file.metadata());
        let metadata = self.recheck_metadata.as_ref()
            .ok_or(Refusal::Closed)?.as_ref().map_err(|_| Refusal::Query)?;
        if metadata.file_type().is_symlink() { return Err(Refusal::Link); }
        let matches_role = match role {
            Role::Directory => metadata.is_dir(), Role::RegularFile => metadata.is_file(),
        };
        if !matches_role { return Err(Refusal::WrongKind); }
        Ok(Identity { volume: metadata.dev(), file: metadata.ino(), links: metadata.nlink() })
    }

    #[cfg(not(any(windows, unix)))]
    fn query_recheck(&mut self, _role: Role) -> Result<Identity, Refusal> {
        Err(Refusal::UnsupportedTarget)
    }

    pub(super) fn recheck_error(&self) -> Option<&io::Error> {
        #[cfg(windows)] { self.recheck_information.as_ref()?.as_ref().err() }
        #[cfg(not(windows))] { self.recheck_metadata.as_ref()?.as_ref().err() }
    }

    pub(super) fn credit_refusal(&self) -> Option<&OperationError> { self.credit_refusal.as_ref() }

    pub(super) fn raw_error(&self) -> Option<&io::Error> {
        #[cfg(windows)] { self.information.as_ref()?.as_ref().err() }
        #[cfg(not(windows))] { self.metadata.as_ref()?.as_ref().err() }
    }
}

#[cfg(windows)]
fn guard_sharing(role: Role) -> u32 {
    // Pinned std/os/windows/fs.rs171-179: removal prevents corresponding
    // sharing operations until close. No blanket filesystem-race theorem.
    match role { Role::Directory => 0x1, Role::RegularFile => 0x3 }
}

/// One inline owner in the process Host, shared across ALL original meters.
/// No Clone/Drop unlock: an unwound call leaves the Host uncertain/busy.
#[cfg(windows)]
pub(super) struct CanonicalSerial { busy: bool }

#[cfg(windows)]
impl CanonicalSerial {
    pub(super) fn new() -> Self { Self { busy: false } }
}

#[cfg(windows)]
const CANONICAL_UNITS: usize = 32_767;
#[cfg(windows)]
const CANONICAL_RETAINED_BYTES: usize = 4 * CANONICAL_UNITS;

/// Actual Host-owned call/result/error/fresh-File children. C covers only the
/// accepted temporary external call class; this root and surviving path have
/// their own NONREFUNDABLE original charges before any std call.
#[cfg(windows)]
struct CanonicalRoot {
    original: ChargeRef,
    attempted: bool,
    expected: Option<Identity>,
    reservation: Option<Result<super::original::CanonicalRefCredit, OperationError>>,
    returned: Option<Result<PathBuf, io::Error>>,
    release: Option<Result<(), OperationError>>,
    fresh: Option<Box<GuardRoot>>,
    credit_refusal: Option<OperationError>,
    observation: Option<Observation>,
    domain: Option<LocalDomain>,
}

#[cfg(windows)]
impl CanonicalRoot {
    fn reserve(original: &impl Source) -> Result<Self, OperationError> {
        prepay(original, Layout::new::<Self>())?;
        prepay(original, Layout::array::<u8>(CANONICAL_RETAINED_BYTES)
            .map_err(|_| OperationError::CaptureOverflow)?)?;
        Ok(Self { original: original.charge_ref(), attempted: false, expected: None,
            reservation: None, returned: None, release: None, fresh: None,
            credit_refusal: None, observation: None, domain: None })
    }

    fn refuse(&mut self, reason: Refusal) -> Observation {
        if let Some(observation @ Observation::Refused(_)) = self.observation { return observation; }
        let observation = Observation::Refused(reason);
        self.observation = Some(observation);
        observation
    }

    fn observe_once(&mut self, serial: &mut CanonicalSerial, path: &Path, expected: Identity) -> Observation {
        if self.attempted || self.observation.is_some() { return self.refuse(Refusal::Closed); }
        self.attempted = true;
        self.expected = Some(expected);
        if serial.busy { return self.refuse(Refusal::Closed); }
        serial.busy = true;
        let observation = self.observe_serialized(path, expected);
        // No await/callback or ownership MOVE into a temporary future. A panic
        // or process abort does not reach this explicit serial release.
        serial.busy = false;
        observation
    }

    fn observe_serialized(&mut self, path: &Path, expected: Identity) -> Observation {
        if !bounded_verbatim(path) { return self.refuse(Refusal::Path); }
        self.reservation = Some(self.original.reserve_host_canonical());
        if !matches!(self.reservation.as_ref(), Some(Ok(_))) { return self.refuse(Refusal::Closed); }
        // m9aff: ONE operator-root call. Private std allocation beyond C from
        // an intermediate size hint or oversized result retains the accepted
        // ordinary Host process allocation-abort residual; NOT a heap bound.
        self.returned = Some(std::fs::canonicalize(path)); // FIRST whole Result MOVE.
        self.release = Some(match self.reservation.as_mut() {
            Some(Ok(credit)) => credit.release_after_retained_result(),
            _ => Err(OperationError::CreditOwned),
        });
        if !matches!(self.release.as_ref(), Some(Ok(()))) { return self.refuse(Refusal::Closed); }
        let path = match self.returned.as_ref() {
            Some(Ok(path)) => path,
            _ => return self.refuse(Refusal::Canonicalize),
        };
        use std::os::windows::ffi::OsStrExt;
        if path.as_os_str().encode_wide().take(CANONICAL_UNITS + 1).count() > CANONICAL_UNITS {
            // Retain actual Result, refuse BEFORE classification/fresh reopen.
            // Its out-of-envelope surviving allocation has NO positive fit proof.
            return self.refuse(Refusal::CanonicalExtent);
        }
        let domain = match LocalDomain::classify(path, &expected) {
            Ok(domain) => domain, // Tentative classification, no authority yet.
            Err(reason) => return self.refuse(reason),
        };
        let fresh = match GuardRoot::reserve(&self.original) {
            Ok(fresh) => fresh,
            Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
        };
        self.fresh = Some(Box::new(fresh)); // Owner BEFORE normal path/open/query.
        let Some(fresh) = self.fresh.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = fresh.open_existing_verbatim(path, Role::Directory) {
            return self.refuse(reason);
        }
        if fresh.identity() != Some(&expected) { return self.refuse(Refusal::Changed); }
        self.domain = Some(domain); // ONLY after actual returned-path File equality.
        self.observation = Some(Observation::Observed);
        Observation::Observed
    }

    fn revalidate(&mut self) -> Observation {
        if let Some(observation @ Observation::Refused(_)) = self.observation { return observation; }
        if self.observation != Some(Observation::Observed) || self.domain.is_none()
            || !matches!(self.release.as_ref(), Some(Ok(())))
        { return self.refuse(Refusal::Closed); }
        let Some(fresh) = self.fresh.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = fresh.revalidate_handle() { return self.refuse(reason); }
        if fresh.identity() != self.expected.as_ref() { return self.refuse(Refusal::Changed); }
        Observation::Observed
    }
}

/// Private retained observation graph for ONE existing endpoint. This does not
/// certify excluded roots, newly created leaves or SQLite's own connection.
#[cfg(windows)]
pub(super) struct ChainRoot {
    original: ChargeRef,
    source: Option<Box<GuardRoot>>,
    guards: Option<Box<GuardRoot>>,
    observation: Option<Observation>,
    credit_refusal: Option<OperationError>,
    canonical: Option<Box<CanonicalRoot>>,
    pub(super) next: Option<Box<Self>>,
}

#[cfg(windows)]
impl Drop for ChainRoot {
    fn drop(&mut self) {
        let mut next = self.next.take();
        while let Some(mut root) = next { next = root.next.take(); }
    }
}

#[cfg(windows)]
impl ChainRoot {
    pub(super) fn reserve(original: &impl Source) -> Result<Self, OperationError> {
        prepay(original, Layout::new::<Self>())?;
        Ok(Self { original: original.charge_ref(), source: None, guards: None,
            observation: None, credit_refusal: None, canonical: None, next: None })
    }

    fn refuse(&mut self, reason: Refusal) -> Observation {
        if let Some(observation @ Observation::Refused(_)) = self.observation { return observation; }
        let observation = Observation::Refused(reason);
        self.observation = Some(observation);
        observation
    }

    /// Host installs this whole root BEFORE any allocation or File operation.
    /// Each exact node is prepaid/installed before its open. Every prefix is
    /// opened and checked before a descendant; all prior owners stay retained.
    pub(super) fn observe_existing(&mut self, path: &Path, role: Role) -> Observation {
        if self.source.is_some() || self.guards.is_some() || self.observation.is_some() {
            return Observation::Refused(Refusal::Closed);
        }
        let source = match GuardRoot::reserve(&self.original) {
            Ok(source) => source,
            Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
        };
        self.source = Some(Box::new(source));
        let Some(source) = self.source.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let PathObservation::Refused(reason) = source.prepare_owner_path(path) {
            return self.refuse(reason);
        }
        let Some(native) = self.source.as_ref().and_then(|source| source.native_path.as_deref()) else {
            return self.refuse(Refusal::Closed);
        };
        // Ancestors walks parents; indexing in reverse avoids an unbounded Vec
        // and preserves the actual root-first open order. Paths are <=4096B.
        let depth = native.ancestors().count();
        if depth == 0 { return self.refuse(Refusal::Path); }
        for index in (0..depth).rev() {
            let Some(prefix) = native.ancestors().nth(index) else {
                return self.refuse(Refusal::Path);
            };
            let current_role = if index == 0 { role } else { Role::Directory };
            let node = match GuardRoot::reserve(&self.original) {
                Ok(node) => node,
                Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
            };
            let mut node = Box::new(node);
            node.next = self.guards.take();
            self.guards = Some(node);
            let Some(node) = self.guards.as_deref_mut() else { return self.refuse(Refusal::Closed) };
            let observation = node.open_existing_verbatim(prefix, current_role);
            if let Observation::Refused(reason) = observation { return self.refuse(reason); }
            let mut prior = node.next.as_deref();
            while let Some(ancestor) = prior {
                if matches!((node.identity(), ancestor.identity()), (Some(left), Some(right))
                    if left.volume == right.volume && left.file == right.file)
                { return self.refuse(Refusal::Alias); }
                prior = ancestor.next.as_deref();
            }
        }
        self.observation = Some(Observation::Observed);
        Observation::Observed
    }

    pub(super) fn revalidate(&mut self) -> Observation {
        if self.observation != Some(Observation::Observed) { return self.refuse(Refusal::Closed); }
        let mut node = self.guards.as_deref_mut();
        let mut seen = false;
        while let Some(guard) = node {
            if let Observation::Refused(reason) = guard.revalidate_handle() { return self.refuse(reason); }
            seen = true;
            node = guard.next.as_deref_mut();
        }
        if !seen { return self.refuse(Refusal::Closed); }
        if let Some(canonical) = self.canonical.as_deref_mut() {
            if let Observation::Refused(reason) = canonical.revalidate() { return self.refuse(reason); }
        }
        Observation::Observed
    }

    /// Host-only operator root, never a campaign/worker-supplied path. The
    /// original held chain brackets the ONE call and its returned-path File.
    pub(super) fn canonicalize_local_root(&mut self, serial: &mut CanonicalSerial) -> Observation {
        if self.canonical.is_some() { return self.revalidate(); }
        if let Observation::Refused(reason) = self.revalidate() { return self.refuse(reason); }
        let expected = match self.directory_endpoint() {
            Ok(expected) => *expected,
            Err(reason) => return self.refuse(reason),
        };
        let canonical = match CanonicalRoot::reserve(&self.original) {
            Ok(root) => root,
            Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
        };
        self.canonical = Some(Box::new(canonical));
        let Some(path) = self.source.as_deref().and_then(|source| source.native_path.as_deref()) else {
            return self.refuse(Refusal::Closed);
        };
        let Some(root) = self.canonical.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = root.observe_once(serial, path, expected) { return self.refuse(reason); }
        self.revalidate() // SAME original ancestors and actual fresh File still held.
    }

    fn local_domain(&self) -> Result<LocalDomain, Refusal> {
        if self.observation != Some(Observation::Observed) { return Err(Refusal::Closed); }
        let root = self.canonical.as_deref().ok_or(Refusal::Closed)?;
        if root.observation != Some(Observation::Observed) { return Err(Refusal::Closed); }
        root.domain.ok_or(Refusal::UnsupportedStorageDomain)
    }

    fn directory_relation_local(&self, other: &Self) -> Result<DirectoryRelation, Refusal> {
        match self.local_domain()?.relation(other.local_domain()?)? {
            DomainRelation::SameDrive => self.directory_relation(other),
            DomainRelation::DisjointDrives => Ok(DirectoryRelation::Disjoint),
        }
    }

    fn directory_endpoint(&self) -> Result<&Identity, Refusal> {
        if self.observation != Some(Observation::Observed) { return Err(Refusal::Closed); }
        let guard = self.guards.as_deref().ok_or(Refusal::Closed)?;
        if guard.role != Some(Role::Directory) { return Err(Refusal::WrongKind); }
        guard.identity().ok_or(Refusal::UnsupportedIdentity)
    }

    /// Relation of observed directory identities, ONLY for conservative
    /// exclusion. Same never authorizes switching a namespace/campaign binding.
    fn directory_relation(&self, other: &Self) -> Result<DirectoryRelation, Refusal> {
        let left = self.directory_endpoint()?;
        let right = other.directory_endpoint()?;
        if left.volume == right.volume && left.file == right.file { return Ok(DirectoryRelation::Same); }
        let contains = other.has_directory_identity(left);
        let within = self.has_directory_identity(right);
        match (contains, within) {
            (true, true) => Err(Refusal::Alias),
            (true, false) => Ok(DirectoryRelation::Contains),
            (false, true) => Ok(DirectoryRelation::Within),
            (false, false) => Ok(DirectoryRelation::Disjoint),
        }
    }

    fn has_directory_identity(&self, identity: &Identity) -> bool {
        let mut next = self.guards.as_deref();
        while let Some(guard) = next {
            if guard.role == Some(Role::Directory)
                && guard.identity().is_some_and(|value| value.volume == identity.volume && value.file == identity.file)
            { return true; }
            next = guard.next.as_deref();
        }
        false
    }
}

/// Actual retained storage/excluded-root observations. Input backing belongs
/// to the trusted caller; each accepted chain/path child is independently owned
/// here before effects. This is not a verified issuer or complete census proof.
#[cfg(windows)]
pub(super) struct LayoutRoot {
    original: ChargeRef,
    storage: Option<Box<ChainRoot>>,
    excluded: Option<Box<ChainRoot>>,
    observation: Option<Observation>,
    credit_refusal: Option<OperationError>,
    creation_attempted: bool,
    native_directory: Option<PathBuf>,
    directory_creation: Option<Result<(), io::Error>>,
    namespace: Option<Box<ChainRoot>>,
    lock_leaf: Option<Box<GuardRoot>>,
    lock_result: Option<Result<(), std::fs::TryLockError>>,
    database_leaf: Option<Box<GuardRoot>>,
    journal_association: Option<Arc<JournalAssociation>>,
    pub(super) next: Option<Box<Self>>,
}

#[cfg(windows)]
impl Drop for LayoutRoot {
    fn drop(&mut self) {
        let mut next = self.next.take();
        while let Some(mut root) = next { next = root.next.take(); }
    }
}

#[cfg(windows)]
impl LayoutRoot {
    pub(super) fn reserve(original: &impl Source) -> Result<Self, OperationError> {
        prepay(original, Layout::new::<Self>())?;
        Ok(Self { original: original.charge_ref(), storage: None, excluded: None,
            observation: None, credit_refusal: None, creation_attempted: false,
            native_directory: None, directory_creation: None, namespace: None,
            lock_leaf: None, lock_result: None, database_leaf: None,
            journal_association: None, next: None })
    }

    /// Operator-configured roots ONLY. Namespace has been created/retained;
    /// each root gets exactly one canonical call, serialized by the SAME Host.
    pub(super) fn canonicalize_local_roots(&mut self, serial: &mut CanonicalSerial) -> Observation {
        if self.namespace.is_none() { return self.refuse(Refusal::Closed); }
        if let Observation::Refused(reason) = self.revalidate() { return self.refuse(reason); }
        let Some(storage) = self.storage.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = storage.canonicalize_local_root(serial) { return self.refuse(reason); }
        let mut next = self.excluded.as_deref_mut();
        while let Some(root) = next {
            if let Observation::Refused(reason) = root.canonicalize_local_root(serial) { return self.refuse(reason); }
            next = root.next.as_deref_mut();
        }
        let Some(namespace) = self.namespace.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = namespace.canonicalize_local_root(serial) { return self.refuse(reason); }
        self.revalidate_local_roots()
    }

    fn local_roots(&self) -> impl Iterator<Item = &ChainRoot> {
        self.storage.as_deref().into_iter()
            .chain(std::iter::successors(self.excluded.as_deref(), |root| root.next.as_deref()))
            .chain(self.namespace.as_deref())
    }

    fn validate_local_relations(&self) -> Result<(), Refusal> {
        // All pairs, including exclusion-to-exclusion. A storage-only star
        // could miss two excluded roots with ambiguous letters/serials.
        for (index, left) in self.local_roots().enumerate() {
            for right in self.local_roots().skip(index + 1) {
                left.local_domain()?.relation(right.local_domain()?)?;
            }
        }
        let storage = self.storage.as_deref().ok_or(Refusal::Closed)?;
        let namespace = self.namespace.as_deref().ok_or(Refusal::Closed)?;
        if storage.directory_relation_local(namespace)? != DirectoryRelation::Contains { return Err(Refusal::Alias); }
        let mut next = self.excluded.as_deref();
        while let Some(excluded) = next {
            if storage.directory_relation_local(excluded)? != DirectoryRelation::Disjoint
                || namespace.directory_relation_local(excluded)? != DirectoryRelation::Disjoint
            { return Err(Refusal::Alias); }
            next = excluded.next.as_deref();
        }
        Ok(())
    }

    pub(super) fn revalidate_local_roots(&mut self) -> Observation {
        if let Observation::Refused(reason) = self.revalidate() { return self.refuse(reason); }
        let Some(namespace) = self.namespace.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = namespace.revalidate() { return self.refuse(reason); }
        if let Err(reason) = self.validate_local_relations() { return self.refuse(reason); }
        Observation::Observed
    }

    pub(super) fn check_local_domains_with(&self, other: &Self) -> Result<(), Refusal> {
        self.validate_local_relations()?;
        other.validate_local_relations()?;
        for left in self.local_roots() {
            for right in other.local_roots() {
                left.local_domain()?.relation(right.local_domain()?)?;
            }
        }
        Ok(())
    }

    fn refuse(&mut self, reason: Refusal) -> Observation {
        if let Some(observation @ Observation::Refused(_)) = self.observation { return observation; }
        let observation = Observation::Refused(reason);
        self.observation = Some(observation);
        observation
    }

    /// Requires actual existing directories. Missing roots refuse rather than
    /// silently dropping an exclusion or relying on lexical starts_with.
    pub(super) fn observe_existing(&mut self, storage: &Path, excluded: &[&Path]) -> Observation {
        if self.storage.is_some() || self.excluded.is_some() || self.observation.is_some() {
            return Observation::Refused(Refusal::Closed);
        }
        let chain = match ChainRoot::reserve(&self.original) {
            Ok(chain) => chain,
            Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
        };
        self.storage = Some(Box::new(chain));
        let Some(chain) = self.storage.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = chain.observe_existing(storage, Role::Directory) {
            return self.refuse(reason);
        }
        for path in excluded {
            let chain = match ChainRoot::reserve(&self.original) {
                Ok(chain) => chain,
                Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
            };
            let mut chain = Box::new(chain);
            chain.next = self.excluded.take();
            self.excluded = Some(chain);
            let Some(chain) = self.excluded.as_deref_mut() else { return self.refuse(Refusal::Closed) };
            if let Observation::Refused(reason) = chain.observe_existing(path, Role::Directory) {
                return self.refuse(reason);
            }
        }
        self.revalidate()
    }

    /// Rechecks actual held handles, then rejects either direction of overlap.
    /// Shared ancestors alone are permitted. Exclusions may overlap each other;
    /// none may equal/contain/be contained by the candidate storage endpoint.
    pub(super) fn revalidate(&mut self) -> Observation {
        if let Some(observation @ Observation::Refused(_)) = self.observation { return observation; }
        let Some(storage) = self.storage.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = storage.revalidate() { return self.refuse(reason); }
        let mut next = self.excluded.as_deref_mut();
        while let Some(chain) = next {
            if let Observation::Refused(reason) = chain.revalidate() { return self.refuse(reason); }
            match storage.directory_relation(chain) {
                Ok(DirectoryRelation::Disjoint) => (),
                Ok(_) => return self.refuse(Refusal::Alias),
                Err(reason) => return self.refuse(reason),
            }
            next = chain.next.as_deref_mut();
        }
        self.observation = Some(Observation::Observed);
        Observation::Observed
    }

    /// One private namespace preparation, still NOT a lease/connection issuer.
    /// The complete trusted census and final SQLite association are separate.
    pub(super) fn prepare_namespace_once(&mut self) -> Observation {
        if self.creation_attempted || self.observation != Some(Observation::Observed) {
            return Observation::Refused(Refusal::Closed);
        }
        self.creation_attempted = true;
        if let Observation::Refused(reason) = self.revalidate() { return self.refuse(reason); }
        let Some(storage) = self.storage.as_deref() else { return self.refuse(Refusal::Closed) };
        let Some(path) = storage.guards.as_ref().and_then(|guard| guard.native_path.as_deref()) else {
            return self.refuse(Refusal::Closed);
        };
        if let Err(error) = install_child_path(&self.original, &mut self.native_directory, path, ReservedName::Namespace) {
            return match error {
                PathFailure::Path => self.refuse(Refusal::Path),
                PathFailure::Credit(error) => { self.credit_refusal = Some(error); self.refuse(Refusal::Closed) }
            };
        }
        let Some(path) = self.native_directory.as_ref() else { return self.refuse(Refusal::Closed) };
        let layout = match native_path_layout(path) { Ok(layout) => layout, Err(reason) => return self.refuse(reason) };
        if let Err(error) = prepay(&self.original, layout) {
            self.credit_refusal = Some(error); return self.refuse(Refusal::Closed);
        }
        // Nonrecursive mkdir has exactly one pinned native path conversion.
        // Retain expected AlreadyExists too; it never proves directory identity.
        self.directory_creation = Some(std::fs::DirBuilder::new().create(path));
        if !matches!(self.directory_creation.as_ref(), Some(Ok(())))
            && !matches!(self.directory_creation.as_ref(), Some(Err(error)) if error.kind() == io::ErrorKind::AlreadyExists)
        { return self.refuse(Refusal::Open); }
        let chain = match ChainRoot::reserve(&self.original) {
            Ok(chain) => chain,
            Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
        };
        self.namespace = Some(Box::new(chain));
        let Some(namespace) = self.namespace.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = namespace.observe_existing(path, Role::Directory) { return self.refuse(reason); }
        match storage.directory_relation(namespace) {
            Ok(DirectoryRelation::Contains) => (),
            Ok(_) => return self.refuse(Refusal::Alias),
            Err(reason) => return self.refuse(reason),
        }
        let mut excluded = self.excluded.as_deref();
        while let Some(chain) = excluded {
            match namespace.directory_relation(chain) {
                Ok(DirectoryRelation::Disjoint) => (),
                Ok(_) => return self.refuse(Refusal::Alias),
                Err(reason) => return self.refuse(reason),
            }
            excluded = chain.next.as_deref();
        }
        let lock = match GuardRoot::reserve(&self.original) {
            Ok(lock) => lock,
            Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
        };
        self.lock_leaf = Some(Box::new(lock));
        let Some(lock) = self.lock_leaf.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = lock.open_named_leaf(namespace, ReservedName::Lock) { return self.refuse(reason); }
        let Some(file) = lock.retained_file() else { return self.refuse(Refusal::Closed) };
        if let Err(error) = self.original.check() { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
        self.lock_result = Some(file.try_lock());
        if !matches!(self.lock_result.as_ref(), Some(Ok(()))) { return self.refuse(Refusal::Lock); }
        // The same retained File holds the lock through Host lifetime. Only
        // after this actual nonblocking lock succeeds may the DB be prepared.
        let database = match GuardRoot::reserve(&self.original) {
            Ok(database) => database,
            Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
        };
        self.database_leaf = Some(Box::new(database));
        let Some(database) = self.database_leaf.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = database.open_named_leaf(namespace, ReservedName::Database) { return self.refuse(reason); }
        if lock.aliases_regular_file(database) { return self.refuse(Refusal::Alias); }
        if let Observation::Refused(reason) = lock.revalidate_handle() { return self.refuse(reason); }
        if let Observation::Refused(reason) = database.revalidate_handle() { return self.refuse(reason); }
        if let Observation::Refused(reason) = namespace.revalidate() { return self.refuse(reason); }
        self.revalidate()
    }

    /// Borrowed ONLY by the private journal while this actual Host retains all
    /// ancestor/leaf handles and the same locked File. No worker/public token.
    pub(super) fn journal_inputs(&mut self, original: &impl Source) -> Result<JournalInputs<'_>, Refusal> {
        if let Err(reason) = self.check_journal_association(original) {
            self.refuse(reason); return Err(reason);
        }
        if self.journal_association.is_none() {
            let charged = crate::driver::owned_driver_task::own_arc_layout::<JournalAssociation>()
                .and_then(|layout| prepay(&self.original, layout));
            if let Err(error) = charged {
                self.credit_refusal = Some(error);
                self.refuse(Refusal::Closed); return Err(Refusal::Closed);
            }
            // This LayoutRoot is already installed in Host. The exact Arc
            // allocation is normal cumulative credit, never C/J or refunded.
            self.journal_association = Some(Arc::new(JournalAssociation { _private: () }));
        }
        let database = self.database_leaf.as_ref().ok_or(Refusal::Closed)?;
        let lock = self.lock_leaf.as_ref().ok_or(Refusal::Closed)?;
        Ok(JournalInputs {
            association: self.journal_association.as_ref().ok_or(Refusal::Closed)?,
            database: database.native_path.as_deref().ok_or(Refusal::Closed)?,
            lock_path: lock.native_path.as_deref().ok_or(Refusal::Closed)?,
            new_database: database.leaf_created.ok_or(Refusal::Closed)?,
        })
    }

    fn check_journal_association(&mut self, original: &impl Source) -> Result<(), Refusal> {
        if !self.original.same_instance(original) || !self.creation_attempted
            || !matches!(self.lock_result.as_ref(), Some(Ok(())))
        { return Err(Refusal::Closed); }
        if let Observation::Refused(reason) = self.revalidate() { return Err(reason); }
        let namespace = self.namespace.as_deref_mut().ok_or(Refusal::Closed)?;
        if let Observation::Refused(reason) = namespace.revalidate() { return Err(reason); }
        let storage = self.storage.as_deref().ok_or(Refusal::Closed)?;
        if storage.directory_relation(namespace)? != DirectoryRelation::Contains { return Err(Refusal::Alias); }
        let lock = self.lock_leaf.as_deref_mut().ok_or(Refusal::Closed)?;
        let database = self.database_leaf.as_deref_mut().ok_or(Refusal::Closed)?;
        if lock.observation != Some(Observation::Observed) || database.observation != Some(Observation::Observed)
            || database.leaf_created.is_none() || lock.leaf_created.is_none()
        { return Err(Refusal::Closed); }
        if let Observation::Refused(reason) = lock.revalidate_handle() { return Err(reason); }
        if let Observation::Refused(reason) = database.revalidate_handle() { return Err(reason); }
        if lock.aliases_regular_file(database) { return Err(Refusal::Alias); }
        Ok(())
    }
}

/// Retained operator-configured App context observation. It owns no second
/// journal/lock and cannot mint a scope, complete-fit proof or public token.
#[cfg(windows)]
pub(super) struct ContextRoot {
    original: ChargeRef,
    attempted: bool,
    requested_namespace: Option<Box<ChainRoot>>,
    app: Option<Box<LayoutRoot>>,
    process_owner: Option<Weak<JournalAssociation>>,
    binding: Option<super::journal::OpenBinding>,
    observation: Option<Observation>,
    credit_refusal: Option<OperationError>,
    scope_anchor: Option<Arc<ContextAnchor>>,
    pub(super) next: Option<Box<Self>>,
}

/// Allocation identity of one retained context, never a filesystem identity.
#[cfg(windows)]
struct ContextAnchor;

/// Private nonclonable observation tied to the actual retained context and
/// OPEN. It cannot issue a lease, set durable_open or admit a driver.
#[cfg(windows)]
pub(super) struct ScopeContext {
    owner: Weak<ContextAnchor>,
    binding: super::journal::OpenBinding,
    refusal: Option<Refusal>,
}

#[cfg(windows)]
impl ScopeContext {
    pub(super) fn matches(&self, context: &ContextRoot) -> bool {
        match (self.owner.upgrade(), context.scope_anchor.as_ref()) {
            (Some(actual), Some(expected)) => Arc::ptr_eq(&actual, expected),
            _ => false,
        }
    }

    pub(super) fn revalidate(&mut self, context: &mut ContextRoot,
        journal: &mut super::journal::JournalRoot, process: &mut LayoutRoot) -> Observation
    {
        if let Some(reason) = self.refusal { return Observation::Refused(reason); }
        let observation = if !self.matches(context) {
            Observation::Refused(Refusal::Changed)
        } else if !context.binding.as_ref().is_some_and(|binding|
            binding.generation == self.binding.generation && binding.incarnation == self.binding.incarnation)
        {
            Observation::Refused(Refusal::Changed)
        } else { context.revalidate(journal, process) };
        if let Observation::Refused(reason) = observation { self.refusal = Some(reason); }
        observation
    }
}

#[cfg(windows)]
impl Drop for ContextRoot {
    fn drop(&mut self) {
        let mut next = self.next.take();
        while let Some(mut root) = next { next = root.next.take(); }
    }
}

#[cfg(windows)]
impl ContextRoot {
    pub(super) fn reserve(original: &impl Source) -> Result<Self, OperationError> {
        prepay(original, Layout::new::<Self>())?;
        Ok(Self { original: original.charge_ref(), attempted: false,
            requested_namespace: None, app: None, process_owner: None,
            binding: None, observation: None, credit_refusal: None, scope_anchor: None, next: None })
    }

    /// Only retained successful guards and actual OPEN can supply this
    /// observation. The Arc is created and charged once by this context owner.
    pub(super) fn scope_context(&mut self, journal: &mut super::journal::JournalRoot,
        process: &mut LayoutRoot) -> Result<ScopeContext, Refusal>
    {
        if self.credit_refusal.is_some() { return Err(Refusal::Closed); }
        if let Observation::Refused(reason) = self.revalidate(journal, process) { return Err(reason); }
        if self.scope_anchor.is_none() {
            let layout = crate::driver::owned_driver_task::own_arc_layout::<ContextAnchor>()
                .map_err(|error| { self.credit_refusal = Some(error); Refusal::Closed })?;
            prepay(&self.original, layout)
                .map_err(|error| { self.credit_refusal = Some(error); Refusal::Closed })?;
            self.scope_anchor = Some(Arc::new(ContextAnchor));
        }
        let anchor = self.scope_anchor.as_ref().ok_or(Refusal::Closed)?;
        let binding = *self.binding.as_ref().ok_or(Refusal::Closed)?;
        Ok(ScopeContext { owner: Arc::downgrade(anchor), binding, refusal: None })
    }

    pub(super) fn refuse(&mut self, reason: Refusal) -> Observation {
        if let Some(observation @ Observation::Refused(_)) = self.observation { return observation; }
        let observation = Observation::Refused(reason);
        self.observation = Some(observation); observation
    }

    pub(super) fn observe_once(
        &mut self, journal: &mut super::journal::JournalRoot, process: &mut LayoutRoot,
        requested_namespace: &Path, app_storage: &Path, excluded: &[&Path], serial: &mut CanonicalSerial,
    ) -> Observation {
        if self.attempted { return Observation::Refused(Refusal::Closed); }
        self.attempted = true;
        match self.observe_body(journal, process, requested_namespace, app_storage, excluded, serial) {
            Ok(()) => self.revalidate(journal, process),
            Err(reason) => self.refuse(reason),
        }
    }

    fn observe_body(
        &mut self, journal: &mut super::journal::JournalRoot, process: &mut LayoutRoot,
        requested_namespace: &Path, app_storage: &Path, excluded: &[&Path], serial: &mut CanonicalSerial,
    ) -> Result<(), Refusal> {
        if !self.original.same_instance(journal.context_original()) { return Err(Refusal::Closed); }
        self.binding = Some(journal.context_binding(process)?);
        self.process_owner = Some(Arc::downgrade(process.journal_inputs(&self.original)?.association));
        let requested = ChainRoot::reserve(&self.original).map_err(|error| {
            self.credit_refusal = Some(error); Refusal::Closed
        })?;
        self.requested_namespace = Some(Box::new(requested));
        let requested = self.requested_namespace.as_deref_mut().ok_or(Refusal::Closed)?;
        if let Observation::Refused(reason) = requested.observe_existing(requested_namespace, Role::Directory) { return Err(reason); }
        if let Observation::Refused(reason) = requested.canonicalize_local_root(serial) { return Err(reason); }
        // Reject a namespace switch before even observing a new App root.
        let actual = process.namespace.as_deref().ok_or(Refusal::Closed)?;
        if requested.directory_relation_local(actual)? != DirectoryRelation::Same { return Err(Refusal::Changed); }
        let app = LayoutRoot::reserve(&self.original).map_err(|error| {
            self.credit_refusal = Some(error); Refusal::Closed
        })?;
        self.app = Some(Box::new(app));
        let app = self.app.as_deref_mut().ok_or(Refusal::Closed)?;
        if let Observation::Refused(reason) = app.observe_existing(app_storage, excluded) { return Err(reason); }
        let storage = app.storage.as_deref_mut().ok_or(Refusal::Closed)?;
        if let Observation::Refused(reason) = storage.canonicalize_local_root(serial) { return Err(reason); }
        let mut next = app.excluded.as_deref_mut();
        while let Some(root) = next {
            if let Observation::Refused(reason) = root.canonicalize_local_root(serial) { return Err(reason); }
            next = root.next.as_deref_mut();
        }
        // App roots NEVER call prepare_namespace_once: no second mkdir, file
        // lock, Connection, entropy or lifecycle generation is created here.
        Ok(())
    }

    fn roots(&self) -> impl Iterator<Item = &ChainRoot> {
        self.requested_namespace.as_deref().into_iter()
            .chain(self.app.as_deref().into_iter().flat_map(|app| app.local_roots()))
    }

    fn validate_relations(&self, process: &LayoutRoot) -> Result<(), Refusal> {
        for (index, left) in self.roots().enumerate() {
            for right in self.roots().skip(index + 1) { left.local_domain()?.relation(right.local_domain()?)?; }
            for right in process.local_roots() { left.local_domain()?.relation(right.local_domain()?)?; }
        }
        let requested = self.requested_namespace.as_deref().ok_or(Refusal::Closed)?;
        let actual = process.namespace.as_deref().ok_or(Refusal::Closed)?;
        if requested.directory_relation_local(actual)? != DirectoryRelation::Same { return Err(Refusal::Changed); }
        let app = self.app.as_deref().ok_or(Refusal::Closed)?;
        let storage = app.storage.as_deref().ok_or(Refusal::Closed)?;
        let mut next = app.excluded.as_deref();
        while let Some(excluded) = next {
            if storage.directory_relation_local(excluded)? != DirectoryRelation::Disjoint
                || actual.directory_relation_local(excluded)? != DirectoryRelation::Disjoint
            { return Err(Refusal::Alias); }
            next = excluded.next.as_deref();
        }
        let mut next = process.excluded.as_deref();
        while let Some(excluded) = next {
            if storage.directory_relation_local(excluded)? != DirectoryRelation::Disjoint { return Err(Refusal::Alias); }
            next = excluded.next.as_deref();
        }
        Ok(())
    }

    pub(super) fn revalidate(&mut self, journal: &mut super::journal::JournalRoot, process: &mut LayoutRoot) -> Observation {
        if let Some(observation @ Observation::Refused(_)) = self.observation { return observation; }
        match self.revalidate_body(journal, process) {
            Ok(()) => { self.observation = Some(Observation::Observed); Observation::Observed }
            Err(reason) => self.refuse(reason),
        }
    }

    fn revalidate_body(&mut self, journal: &mut super::journal::JournalRoot, process: &mut LayoutRoot) -> Result<(), Refusal> {
        let actual = journal.context_binding(process)?;
        let expected = self.binding.as_ref().ok_or(Refusal::Closed)?;
        if actual.generation != expected.generation || actual.incarnation != expected.incarnation { return Err(Refusal::Changed); }
        let owner = self.process_owner.as_ref().and_then(Weak::upgrade).ok_or(Refusal::Closed)?;
        if !Arc::ptr_eq(&owner, process.journal_inputs(&self.original)?.association) { return Err(Refusal::Changed); }
        let requested = self.requested_namespace.as_deref_mut().ok_or(Refusal::Closed)?;
        if let Observation::Refused(reason) = requested.revalidate() { return Err(reason); }
        let app = self.app.as_deref_mut().ok_or(Refusal::Closed)?;
        if let Observation::Refused(reason) = app.revalidate() { return Err(reason); }
        self.validate_relations(process)
    }

    pub(super) fn check_with(&self, other: &Self) -> Result<(), Refusal> {
        let left_owner = self.process_owner.as_ref().and_then(Weak::upgrade).ok_or(Refusal::Closed)?;
        let right_owner = other.process_owner.as_ref().and_then(Weak::upgrade).ok_or(Refusal::Closed)?;
        if !Arc::ptr_eq(&left_owner, &right_owner) { return Err(Refusal::Changed); }
        for left in self.roots() {
            for right in other.roots() { left.local_domain()?.relation(right.local_domain()?)?; }
        }
        // App roots may differ or coincide. Only the actual exclusion relations
        // prohibit overlap; no invented all-App-roots-disjoint product rule.
        for (candidate, protected) in [(self, other), (other, self)] {
            let candidate = candidate.app.as_deref().and_then(|app| app.storage.as_deref()).ok_or(Refusal::Closed)?;
            let mut next = protected.app.as_deref().ok_or(Refusal::Closed)?.excluded.as_deref();
            while let Some(excluded) = next {
                if candidate.directory_relation_local(excluded)? != DirectoryRelation::Disjoint { return Err(Refusal::Alias); }
                next = excluded.next.as_deref();
            }
        }
        Ok(())
    }
}

/// Private mode for one actual attempt; never an identity/provenance boolean.
#[cfg(windows)]
#[derive(Clone, Copy)]
pub(super) enum PreopenMode { Fresh, Existing }

/// Retained pre-SQLite prefix. Host installs this prepaid node before any
/// child allocation or file operation. No public custody or membership yet.
#[cfg(windows)]
pub(super) struct PreopenRoot {
    original: ChargeRef,
    attempted: bool,
    context: Option<ScopeContext>,
    source: Option<Box<GuardRoot>>,
    parent: Option<Box<ChainRoot>>,
    leaf: Option<Box<GuardRoot>>,
    credit_refusal: Option<OperationError>,
    observation: Option<Observation>,
    pub(super) next: Option<Box<Self>>,
}

#[cfg(windows)]
impl PreopenRoot {
    pub(super) fn reserve(original: &impl Source) -> Result<Self, OperationError> {
        prepay(original, Layout::new::<Self>())?;
        Ok(Self { original: original.charge_ref(), attempted: false, context: None,
            source: None, parent: None, leaf: None, credit_refusal: None,
            observation: None, next: None })
    }

    fn refuse(&mut self, reason: Refusal) -> Observation {
        if let Some(observation @ Observation::Refused(_)) = self.observation { return observation; }
        let observation = Observation::Refused(reason); self.observation = Some(observation); observation
    }

    pub(super) fn observe_once(&mut self, context: &mut ContextRoot,
        journal: &mut super::journal::JournalRoot, process: &mut LayoutRoot,
        path: &Path, mode: PreopenMode) -> Observation
    {
        if self.attempted { return Observation::Refused(Refusal::Closed); }
        self.attempted = true;
        if !self.original.same_instance(&context.original)
            || !self.original.same_instance(journal.context_original())
        { return self.refuse(Refusal::Changed); }
        self.context = Some(match context.scope_context(journal, process) {
            Ok(binding) => binding, Err(reason) => return self.refuse(reason),
        });
        let source = match GuardRoot::reserve(&self.original) {
            Ok(root) => root,
            Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
        };
        self.source = Some(Box::new(source));
        let Some(source) = self.source.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let PathObservation::Refused(reason) = source.prepare_owner_path(path) { return self.refuse(reason); }
        let parent = match ChainRoot::reserve(&self.original) {
            Ok(root) => root,
            Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
        };
        self.parent = Some(Box::new(parent));
        let Some(path) = self.source.as_deref().and_then(|root| root.native_path.as_deref()) else {
            return self.refuse(Refusal::Closed);
        };
        if path.file_name().is_none() { return self.refuse(Refusal::Path); }
        let Some(parent_path) = path.parent() else { return self.refuse(Refusal::Path) };
        let Some(parent) = self.parent.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = parent.observe_existing(parent_path, Role::Directory) { return self.refuse(reason); }
        // Critically BEFORE a fresh leaf effect: actual guarded ancestry must
        // place this parent under the admitted App root, outside exclusions.
        if let Err(reason) = self.validate_parent(context, process) { return self.refuse(reason); }
        let Some(parent) = self.parent.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = parent.revalidate() { return self.refuse(reason); }
        let leaf = match GuardRoot::reserve(&self.original) {
            Ok(root) => root,
            Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
        };
        self.leaf = Some(Box::new(leaf));
        let Some(path) = self.source.as_deref().and_then(|root| root.native_path.as_deref()) else {
            return self.refuse(Refusal::Closed);
        };
        let Some(leaf) = self.leaf.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        let observation = match mode {
            PreopenMode::Existing => leaf.open_existing_verbatim(path, Role::RegularFile),
            PreopenMode::Fresh => leaf.create_fresh_verbatim(path),
        };
        if let Observation::Refused(reason) = observation { return self.refuse(reason); }
        self.revalidate(context, journal, process)
    }

    fn validate_parent(&self, context: &ContextRoot, process: &LayoutRoot) -> Result<(), Refusal> {
        let parent = self.parent.as_deref().ok_or(Refusal::Closed)?;
        let app = context.app.as_deref().ok_or(Refusal::Closed)?;
        let storage = app.storage.as_deref().ok_or(Refusal::Closed)?;
        // Route only descendants of the exact retained operator input. This
        // spelling gate is not identity proof: all held ancestry/domain checks
        // below remain mandatory. It prevents an unrelated UNC/drive alias from
        // being promoted solely because its numeric Information happens to fit.
        let requested = self.source.as_deref().and_then(|source| source.native_path.as_deref()).ok_or(Refusal::Closed)?;
        let configured = storage.source.as_deref().and_then(|source| source.native_path.as_deref()).ok_or(Refusal::Closed)?;
        let relative = requested.strip_prefix(configured).map_err(|_| Refusal::Alias)?;
        if relative.as_os_str().is_empty() { return Err(Refusal::Alias); }
        if parent.directory_endpoint()?.volume != storage.local_domain()?.serial {
            return Err(Refusal::AmbiguousStorageDomain);
        }
        if !parent.has_directory_identity(storage.directory_endpoint()?) { return Err(Refusal::Alias); }
        for layout in [app, process] {
            let mut next = layout.excluded.as_deref();
            while let Some(excluded) = next {
                if parent.has_directory_identity(excluded.directory_endpoint()?) { return Err(Refusal::Alias); }
                next = excluded.next.as_deref();
            }
        }
        let namespace = process.namespace.as_deref().ok_or(Refusal::Closed)?;
        if parent.has_directory_identity(namespace.directory_endpoint()?) { return Err(Refusal::Alias); }
        Ok(())
    }

    pub(super) fn revalidate(&mut self, context: &mut ContextRoot,
        journal: &mut super::journal::JournalRoot, process: &mut LayoutRoot) -> Observation
    {
        if let Some(observation @ Observation::Refused(_)) = self.observation { return observation; }
        let Some(binding) = self.context.as_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = binding.revalidate(context, journal, process) { return self.refuse(reason); }
        let Some(parent) = self.parent.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = parent.revalidate() { return self.refuse(reason); }
        if let Err(reason) = self.validate_parent(context, process) { return self.refuse(reason); }
        let Some(leaf) = self.leaf.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = leaf.revalidate_handle() { return self.refuse(reason); }
        let expected = context.app.as_deref().and_then(|app| app.storage.as_deref())
            .and_then(|storage| storage.local_domain().ok()).map(|domain| domain.serial);
        if !matches!((leaf.identity(), expected), (Some(identity), Some(volume)) if identity.volume == volume) {
            return self.refuse(Refusal::AmbiguousStorageDomain);
        }
        self.observation = Some(Observation::Observed); Observation::Observed
    }

    /// Private comparison stage, not a durable member or publication token.
    /// The future Host registration entry keeps the Server connection and both
    /// joins outside this module; no claim about those owners is invented here.
    pub(super) fn pair_observed_files(&mut self, files: &mut MemberFilesRoot,
        context: &mut ContextRoot, journal: &mut super::journal::JournalRoot,
        process: &mut LayoutRoot) -> Observation
    {
        if !self.original.same_instance(&files.original) { return self.refuse(Refusal::Changed); }
        if let Observation::Refused(reason) = self.revalidate(context, journal, process) { return self.refuse(reason); }
        if let Observation::Refused(reason) = files.revalidate(context, journal, process) { return self.refuse(reason); }
        let Some(protected) = self.leaf.as_deref().and_then(GuardRoot::identity).copied() else {
            return self.refuse(Refusal::UnsupportedIdentity);
        };
        for chain in [files.database.as_deref(), files.archive.as_deref()] {
            let Some(chain) = chain else { return self.refuse(Refusal::Closed) };
            let endpoint = match MemberFilesRoot::regular_endpoint(chain) {
                Ok(identity) => identity, Err(reason) => return self.refuse(reason),
            };
            if &protected != endpoint { return self.refuse(Refusal::Changed); }
        }
        Observation::Observed
    }

    pub(super) fn protected_path(&self) -> Result<&Path, Refusal> {
        if self.observation != Some(Observation::Observed) { return Err(Refusal::Closed); }
        self.leaf.as_deref().and_then(|leaf| leaf.native_path.as_deref()).ok_or(Refusal::Closed)
    }
}

#[cfg(windows)]
impl Drop for PreopenRoot {
    fn drop(&mut self) {
        // Terminal deallocation only, never cleanup/refund/effects in Drop.
        let mut next = self.next.take();
        while let Some(mut root) = next { next = root.next.take(); }
    }
}

#[cfg(windows)]
impl GuardRoot {
    fn create_fresh_verbatim(&mut self, path: &Path) -> Observation {
        if self.file.is_some() || self.opened.is_some() || self.native_path.is_some() || self.observation.is_some() {
            return Observation::Refused(Refusal::Closed);
        }
        if !bounded_verbatim(path) || path.file_name().is_none() { return self.refuse_once(Refusal::Path); }
        let bytes = path.as_os_str().as_encoded_bytes().len();
        let owned = match Layout::array::<u8>(bytes) { Ok(layout) => layout, Err(_) => return self.refuse_once(Refusal::Path) };
        let native = match native_path_layout(path) { Ok(layout) => layout, Err(reason) => return self.refuse_once(reason) };
        for layout in [owned, native] {
            if let Err(error) = prepay(&self.original, layout) {
                self.credit_refusal = Some(error); return self.refuse_once(Refusal::Closed);
            }
        }
        self.native_path = Some(path.to_path_buf());
        if let Err(error) = self.original.check() {
            self.credit_refusal = Some(error); return self.refuse_once(Refusal::Closed);
        }
        let Some(path) = self.native_path.as_deref() else { return self.refuse_once(Refusal::Closed) };
        use std::os::windows::fs::OpenOptionsExt;
        self.opened = Some(std::fs::OpenOptions::new().read(true).write(true).create_new(true)
            .share_mode(guard_sharing(Role::RegularFile)).custom_flags(0x0220_0000).open(path));
        if !matches!(self.opened.as_ref(), Some(Ok(_))) { return self.refuse_once(Refusal::Open); }
        self.leaf_created = Some(true); self.role = Some(Role::RegularFile);
        let observation = self.query(Role::RegularFile);
        self.observation = Some(observation); observation
    }
}

/// Host-owned existing database/archive File association only. No canonicalize
/// call is permitted for these campaign-derived paths. This is NOT a complete
/// member-domain/SQLite-connection binding or an issuer input capability.
#[cfg(windows)]
pub(super) struct MemberFilesRoot {
    original: ChargeRef,
    attempted: bool,
    context: Option<ScopeContext>,
    database: Option<Box<ChainRoot>>,
    archive: Option<Box<ChainRoot>>,
    archive_roots: Option<Box<ChainRoot>>,
    archive_roots_attempted: bool,
    credit_refusal: Option<OperationError>,
    observation: Option<Observation>,
    pub(super) next: Option<Box<Self>>,
}

#[cfg(windows)]
impl Drop for MemberFilesRoot {
    fn drop(&mut self) {
        let mut next = self.next.take();
        while let Some(mut root) = next { next = root.next.take(); }
    }
}

#[cfg(windows)]
impl MemberFilesRoot {
    pub(super) fn reserve(original: &impl Source) -> Result<Self, OperationError> {
        prepay(original, Layout::new::<Self>())?;
        Ok(Self { original: original.charge_ref(), attempted: false, context: None,
            database: None, archive: None, archive_roots: None, archive_roots_attempted: false,
            credit_refusal: None, observation: None, next: None })
    }

    fn refuse(&mut self, reason: Refusal) -> Observation {
        if let Some(observation @ Observation::Refused(_)) = self.observation { return observation; }
        let observation = Observation::Refused(reason); self.observation = Some(observation); observation
    }

    pub(super) fn observe_once(&mut self, context: &mut ContextRoot,
        journal: &mut super::journal::JournalRoot, process: &mut LayoutRoot,
        database: &Path, archive_database: &Path) -> Observation
    {
        if self.attempted { return Observation::Refused(Refusal::Closed); }
        self.attempted = true;
        if !self.original.same_instance(&context.original)
            || !self.original.same_instance(journal.context_original())
        { return self.refuse(Refusal::Changed); }
        self.context = Some(match context.scope_context(journal, process) {
            Ok(context) => context,
            Err(reason) => return self.refuse(reason),
        });
        let root = match ChainRoot::reserve(&self.original) {
            Ok(root) => root,
            Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
        };
        self.database = Some(Box::new(root)); // Original custody before path/open/query.
        let Some(root) = self.database.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = root.observe_existing(database, Role::RegularFile) { return self.refuse(reason); }
        let root = match ChainRoot::reserve(&self.original) {
            Ok(root) => root,
            Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
        };
        self.archive = Some(Box::new(root));
        let Some(root) = self.archive.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = root.observe_existing(archive_database, Role::RegularFile) { return self.refuse(reason); }
        self.revalidate(context, journal, process)
    }

    /// Archive's configured roots must match actual admitted exclusions. Only
    /// already admitted operator roots supply domain classification; these
    /// additional retained chains use no canonicalize and cannot add roots.
    pub(super) fn observe_archive_roots(&mut self, archive: &crate::failed_evidence::CampaignArchive,
        context: &ContextRoot, process: &LayoutRoot) -> Observation
    {
        if self.archive_roots_attempted { return Observation::Refused(Refusal::Closed); }
        self.archive_roots_attempted = true;
        if self.observation != Some(Observation::Observed) { return self.refuse(Refusal::Closed); }
        for path in archive.roots_for_host() {
            let root = match ChainRoot::reserve(&self.original) {
                Ok(root) => root,
                Err(error) => { self.credit_refusal = Some(error); return self.refuse(Refusal::Closed); }
            };
            let mut root = Box::new(root); root.next = self.archive_roots.take(); self.archive_roots = Some(root);
            let Some(root) = self.archive_roots.as_deref_mut() else { return self.refuse(Refusal::Closed) };
            if let Observation::Refused(reason) = root.observe_existing(path, Role::Directory) { return self.refuse(reason); }
            let endpoint = match root.directory_endpoint() { Ok(endpoint) => *endpoint, Err(reason) => return self.refuse(reason) };
            let Some(app) = context.app.as_deref() else { return self.refuse(Refusal::Closed) };
            let mut matched = false;
            for layout in [app, process] {
                let mut admitted = layout.excluded.as_deref();
                while let Some(excluded) = admitted {
                    let same_input = matches!((root.source.as_deref().and_then(|source| source.native_path.as_deref()),
                        excluded.source.as_deref().and_then(|source| source.native_path.as_deref())),
                        (Some(left), Some(right)) if left == right);
                    if same_input && excluded.directory_endpoint().is_ok_and(|identity| *identity == endpoint) { matched = true; break; }
                    admitted = excluded.next.as_deref();
                }
            }
            if !matched { return self.refuse(Refusal::Alias); }
        }
        if self.archive_roots.is_none() { return self.refuse(Refusal::Closed); }
        let Some(app) = context.app.as_deref() else { return self.refuse(Refusal::Closed) };
        for layout in [app, process] {
            let mut admitted = layout.excluded.as_deref();
            while let Some(excluded) = admitted {
                let endpoint = match excluded.directory_endpoint() { Ok(value) => *value, Err(reason) => return self.refuse(reason) };
                let mut observed = self.archive_roots.as_deref(); let mut matched = false;
                while let Some(root) = observed {
                    if root.directory_endpoint().is_ok_and(|identity| *identity == endpoint) { matched = true; break; }
                    observed = root.next.as_deref();
                }
                if !matched { return self.refuse(Refusal::Alias); }
                admitted = excluded.next.as_deref();
            }
        }
        Observation::Observed
    }

    fn regular_endpoint(chain: &ChainRoot) -> Result<&Identity, Refusal> {
        if chain.observation != Some(Observation::Observed) { return Err(Refusal::Closed); }
        let endpoint = chain.guards.as_deref().ok_or(Refusal::Closed)?;
        if endpoint.role != Some(Role::RegularFile) { return Err(Refusal::WrongKind); }
        endpoint.identity().ok_or(Refusal::UnsupportedIdentity)
    }

    fn validate_relations(&self, context: &ContextRoot, process: &LayoutRoot) -> Result<(), Refusal> {
        let database = self.database.as_deref().ok_or(Refusal::Closed)?;
        let archive = self.archive.as_deref().ok_or(Refusal::Closed)?;
        let identity = Self::regular_endpoint(database)?;
        if identity != Self::regular_endpoint(archive)? { return Err(Refusal::Changed); }
        let app = context.app.as_deref().ok_or(Refusal::Closed)?;
        let storage = app.storage.as_deref().ok_or(Refusal::Closed)?;
        // Existing retained App-root identity and actual ancestor membership,
        // never caller string starts_with or a canonicalization of a campaign.
        let app_identity = storage.directory_endpoint()?;
        let domain = storage.local_domain()?;
        if identity.volume != domain.serial { return Err(Refusal::AmbiguousStorageDomain); }
        for chain in [database, archive] {
            if !chain.has_directory_identity(app_identity) { return Err(Refusal::Alias); }
            for layout in [app, process] {
                let mut next = layout.excluded.as_deref();
                while let Some(excluded) = next {
                    if chain.has_directory_identity(excluded.directory_endpoint()?) { return Err(Refusal::Alias); }
                    next = excluded.next.as_deref();
                }
            }
            let namespace = process.namespace.as_deref().ok_or(Refusal::Closed)?;
            if chain.has_directory_identity(namespace.directory_endpoint()?) { return Err(Refusal::Alias); }
        }
        Ok(())
    }

    pub(super) fn revalidate(&mut self, context: &mut ContextRoot,
        journal: &mut super::journal::JournalRoot, process: &mut LayoutRoot) -> Observation
    {
        if let Some(observation @ Observation::Refused(_)) = self.observation { return observation; }
        let Some(binding) = self.context.as_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = binding.revalidate(context, journal, process) { return self.refuse(reason); }
        let Some(database) = self.database.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = database.revalidate() { return self.refuse(reason); }
        let Some(archive) = self.archive.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if let Observation::Refused(reason) = archive.revalidate() { return self.refuse(reason); }
        let mut next = self.archive_roots.as_deref_mut();
        while let Some(root) = next {
            if let Observation::Refused(reason) = root.revalidate() { return self.refuse(reason); }
            next = root.next.as_deref_mut();
        }
        if let Err(reason) = self.validate_relations(context, process) { return self.refuse(reason); }
        self.observation = Some(Observation::Observed); Observation::Observed
    }
}

/// Only a private prevalidated-spelling seam. This establishes neither path
/// identity nor exclusion. General owner paths still need a charged producer.
#[cfg(windows)]
fn bounded_verbatim(path: &Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes.len() > 4096 || bytes.contains(&0) || bytes.contains(&b'/')
        || !bytes.starts_with(br"\\?\") || !path.is_absolute()
    {
        return false;
    }
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Prefix(prefix))
        if matches!(prefix.kind(), Prefix::VerbatimDisk(_) | Prefix::VerbatimUNC(_, _)))
        || !matches!(components.next(), Some(Component::RootDir))
    {
        return false;
    }
    components.all(|component| match component {
        Component::Normal(value) => {
            let value = value.as_encoded_bytes();
            !value.is_empty() && value != b"." && value != b".."
                && !value.contains(&b':') && !matches!(value.last(), Some(b'.' | b' '))
        }
        _ => false,
    })
}

/// Allocation-free staging plan. Keep raw separators/components: Components
/// alone can hide '.'/duplicate separators before a verbatim conversion.
/// No canonicalize/absolute/env/current-dir/OS resolution is performed here.
#[cfg(windows)]
fn owner_path_parts(path: &Path) -> Option<(&'static str, &str, usize)> {
    let text = path.to_str()?;
    if text.is_empty() || text.len() > 4096 || !path.is_absolute() { return None; }
    let (prefix, tail, body, unc) = if let Some(tail) = text.strip_prefix(r"\\?\UNC\") {
        ("", text, tail, true)
    } else if let Some(tail) = text.strip_prefix(r"\\?\") {
        let bytes = tail.as_bytes();
        if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic()
            || &bytes[1..3] != b":\\" { return None; }
        ("", text, &tail[3..], false)
    } else if let Some(tail) = text.strip_prefix(r"\\") {
        (r"\\?\UNC\", tail, tail, true)
    } else {
        let bytes = text.as_bytes();
        if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic()
            || &bytes[1..3] != b":\\" { return None; }
        (r"\\?\", text, &text[3..], false)
    };
    if unc && body.split('\\').count() < 2 { return None; }
    if !body.is_empty() || unc {
        for component in body.split('\\') {
            if !unambiguous_owner_component(component) { return None; }
        }
    }
    let bytes = prefix.len().checked_add(tail.len())?;
    if bytes > 4096 { return None; }
    Some((prefix, tail, bytes))
}

#[cfg(windows)]
fn unambiguous_owner_component(component: &str) -> bool {
    if component.is_empty() || component == "." || component == ".."
        || component.ends_with(['.', ' '])
        || component.chars().any(|value| value < ' ' || matches!(value, '/' | ':' | '<' | '>' | '"' | '|' | '?' | '*'))
    { return false; }
    // Deliberately conservative private staging filter, not a complete Windows
    // device-name oracle or a promise that allowed spellings identify one file.
    let stem = component.split('.').next().unwrap_or("").trim_end_matches(' ');
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "CLOCK$"]
        .iter().any(|device| stem.eq_ignore_ascii_case(device))
    { return false; }
    for prefix in ["COM", "LPT"] {
        if stem.len() >= 3 && stem.as_bytes()[..3].eq_ignore_ascii_case(prefix.as_bytes())
            && matches!(&stem[3..], "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
        { return false; }
    }
    true
}

#[cfg(test)]
mod controls {
    use super::*;
    use crate::operation::Allowance;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::time::Instant;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn long_unopened_guard_prefix_drops_without_query_or_credit_refund() {
        tokio::runtime::Handle::current().spawn(async {
            // Partial legacy-meter fixture, not a complete Host fit. No paths,
            // files, queries, SQLite, canonicalization or native child effects.
            let original = operation();
            let mut prefix = None;
            for _ in 0..8192 {
                let mut node = GuardRoot::reserve(&original).unwrap();
                node.next = prefix;
                prefix = Some(Box::new(node));
            }
            let mut count = 0;
            let mut cursor = prefix.as_deref();
            while let Some(node) = cursor {
                assert!(node.file.is_none() && node.identity.is_none() && node.observation.is_none());
                count += 1; cursor = node.next.as_deref();
            }
            assert_eq!(count, 8192);
            let charged = original.captured_bytes();
            drop(prefix);
            assert_eq!(original.captured_bytes(), charged);
            original.check().unwrap();
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn long_partial_namespace_forests_drop_without_opening_or_refunding() {
        tokio::runtime::Handle::current().spawn(async {
            let original = operation();
            let mut chains = None; let mut layouts = None;
            let mut contexts = None; let mut members = None;
            for _ in 0..4096 {
                let mut chain = ChainRoot::reserve(&original).unwrap();
                let mut guard = GuardRoot::reserve(&original).unwrap();
                guard.next = Some(Box::new(GuardRoot::reserve(&original).unwrap()));
                chain.guards = Some(Box::new(guard));
                chain.next = chains; chains = Some(Box::new(chain));
                let mut layout = LayoutRoot::reserve(&original).unwrap();
                layout.next = layouts; layouts = Some(Box::new(layout));
                let mut context = ContextRoot::reserve(&original).unwrap();
                context.next = contexts; contexts = Some(Box::new(context));
                let mut member = MemberFilesRoot::reserve(&original).unwrap();
                member.next = members; members = Some(Box::new(member));
            }
            let charged = original.captured_bytes();
            drop((contexts, layouts, members, chains));
            assert_eq!(original.captured_bytes(), charged);
            original.check().unwrap();
            // Incomplete, unopened prefixes only. Finishing Drop is not a
            // join/terminal verdict, physical association or production fit.
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn archive_root_association_requires_exact_admitted_exclusion_set_and_retains_missing_root_refusal() {
        tokio::runtime::Handle::current().spawn(async {
            for complete in [false, true] {
                let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
                let app = base.join("app"); let sandbox = base.join("sandbox"); let run = base.join("run"); let extra = base.join("extra");
                for path in [&app, &sandbox, &run, &extra] { std::fs::create_dir(path).unwrap(); }
                let database = app.join("00000000-0000-0000-0000-000000000001.sqlite"); std::fs::write(&database, b"archive-root-fixture").unwrap();
                let original = operation(); let mut host = opened_context_host(&original, &base);
                assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &app, &[&sandbox, &run, &extra]).unwrap(), Observation::Observed);
                let archive = crate::failed_evidence::CampaignArchive::new(&database, &sandbox, &run,
                    if complete { std::slice::from_ref(&extra) } else { std::slice::from_ref(&sandbox) }).unwrap();
                host.member_files = Some(Box::new(MemberFilesRoot::reserve(&original).unwrap()));
                let files = host.member_files.as_deref_mut().unwrap();
                let context = host.storage_contexts.as_deref_mut().unwrap(); let process = host.namespace_layouts.as_deref_mut().unwrap();
                assert_eq!(files.observe_once(context, host.journal.as_deref_mut().unwrap(), process, &database, &database), Observation::Observed);
                let expected = if complete { Observation::Observed } else { Observation::Refused(Refusal::Alias) };
                assert_eq!(files.observe_archive_roots(&archive, context, process), expected);
                assert!(files.archive_roots.is_some() && files.database.is_some() && files.archive.is_some());
                let charged = original.captured_bytes();
                assert_eq!(files.observe_archive_roots(&archive, context, process), Observation::Refused(Refusal::Closed));
                assert_eq!(original.captured_bytes(), charged);
                if !complete {
                    assert_eq!(files.revalidate(context, host.journal.as_deref_mut().unwrap(), process), expected);
                }
                // Legacy-meter physical observations, not public Host pairing,
                // Server joins, committed member rows or a complete fit.
                drop(host); std::fs::remove_file(database).unwrap();
                for path in [&app, &sandbox, &run, &extra] { std::fs::remove_dir(path).unwrap(); }
                remove_context_process(&base);
            }
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn custody_arc_keeps_original_prefix_and_host_charge_across_server_style_pointer_clones() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let database = base.join("arc.sqlite"); std::fs::write(&database, b"retained-prefix").unwrap();
            let original = operation(); let mut host = opened_context_host(&original, &base);
            assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &base, &[]).unwrap(), Observation::Observed);
            host.custody_attempts = Some(Box::new(super::super::custody::Attempt::reserve(&original).unwrap()));
            let attempt = host.custody_attempts.as_deref_mut().unwrap();
            let context = host.storage_contexts.as_deref_mut().unwrap(); let journal = host.journal.as_deref_mut().unwrap();
            let process = host.namespace_layouts.as_deref_mut().unwrap();
            attempt.observe(context, journal, process, &database, PreopenMode::Existing);
            let owner = std::ptr::from_ref(attempt.prefix().unwrap().leaf.as_deref().unwrap().retained_file().unwrap()) as usize;
            let before = original.captured_bytes();
            let custody = attempt.publish().unwrap();
            let charged = original.captured_bytes();
            assert_eq!(charged - before, crate::driver::owned_driver_task::own_arc_layout::<super::super::CampaignFileCustody>().unwrap().size()
                + custody.protected_path().as_os_str().as_encoded_bytes().len());
            assert!(attempt.prefix().is_none() && Arc::ptr_eq(attempt.published().unwrap(), &custody));
            let path = custody.protected_path().as_os_str().as_encoded_bytes().as_ptr() as usize;
            let parent = custody.clone(); let campaign = custody.clone();
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(std::ptr::from_ref(custody.prefix.lock().unwrap().leaf.as_deref().unwrap().retained_file().unwrap()) as usize, owner);
            assert_eq!(parent.protected_path().as_os_str().as_encoded_bytes().as_ptr() as usize, path);
            assert_eq!(campaign.protected_path().as_os_str().as_encoded_bytes().as_ptr() as usize, path);
            assert!(attempt.publish().is_err());
            host.member_files = Some(Box::new(MemberFilesRoot::reserve(&original).unwrap()));
            let files = host.member_files.as_deref_mut().unwrap();
            assert_eq!(files.observe_once(context, journal, process, &database, &database), Observation::Observed);
            assert_eq!(attempt.pair(files, context, journal, process).unwrap(), Observation::Observed);
            assert!(attempt.pair(files, context, journal, process).is_err());
            let readback = attempt.readback();
            assert!(readback.unpublished_prefix.is_none());
            assert!(Arc::ptr_eq(readback.custody.unwrap(), &custody));
            assert_eq!(readback.observed, Some(&Ok(Observation::Observed)));
            assert_eq!(readback.publication, Some(&Ok(())));
            assert_eq!(readback.pairing, Some(&Ok(Observation::Observed)));
            let final_charge = original.captured_bytes();
            drop(custody); drop(campaign); drop(parent); drop(host);
            assert_eq!(original.captured_bytes(), final_charge);
            std::fs::remove_file(database).unwrap(); remove_context_process(&base);
            // Arc/legacy-meter fixture only: no Server connection, original
            // joins, complete Host fit or positive durable member is produced.
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn custody_failed_prefix_cannot_publish_or_retry_and_close_refuses_arc_publication() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let database = base.join("already.sqlite"); std::fs::write(&database, b"keep-original").unwrap();
            let original = operation(); let mut host = opened_context_host(&original, &base);
            assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &base, &[]).unwrap(), Observation::Observed);
            host.custody_attempts = Some(Box::new(super::super::custody::Attempt::reserve(&original).unwrap()));
            let attempt = host.custody_attempts.as_deref_mut().unwrap();
            let context = host.storage_contexts.as_deref_mut().unwrap(); let journal = host.journal.as_deref_mut().unwrap();
            let process = host.namespace_layouts.as_deref_mut().unwrap();
            attempt.observe(context, journal, process, &database, PreopenMode::Fresh);
            let raw = std::ptr::from_ref(attempt.prefix().unwrap().leaf.as_deref().unwrap().raw_open_error().unwrap()) as usize;
            let charged = original.captured_bytes();
            assert!(attempt.publish().is_err() && attempt.published().is_none());
            attempt.observe(context, journal, process, &database, PreopenMode::Existing);
            assert!(attempt.publish().is_err() && attempt.published().is_none());
            assert_eq!(std::ptr::from_ref(attempt.prefix().unwrap().leaf.as_deref().unwrap().raw_open_error().unwrap()) as usize, raw);
            assert_eq!(original.captured_bytes(), charged);
            let mut closed = Box::new(super::super::custody::Attempt::reserve(&original).unwrap());
            closed.next = host.custody_attempts.take(); host.custody_attempts = Some(closed);
            let closed = host.custody_attempts.as_deref_mut().unwrap();
            closed.observe(context, journal, process, &database, PreopenMode::Existing);
            assert!(closed.prefix().unwrap().protected_path().is_ok());
            closed.refuse_publication(OperationError::CreditOwned);
            assert!(closed.publish().is_err() && closed.published().is_none() && closed.prefix().is_some());
            // Explicit close-after-guard fixture; no scheduling-race claim.
            let now = Instant::now(); let deadline = now + Duration::from_secs(10);
            assert_eq!(host.close_global(now, deadline), Some(deadline));
            let view = host.closed_readback().unwrap(); let mut attempts = view.custodies();
            let closed = attempts.next().unwrap();
            assert!(closed.custody.is_none() && closed.unpublished_prefix.is_some());
            assert_eq!(closed.publication, Some(&Err(OperationError::CreditOwned)));
            let failed = attempts.next().unwrap();
            assert!(failed.custody.is_none() && failed.pairing.is_none());
            assert_eq!(std::ptr::from_ref(failed.unpublished_prefix.unwrap().leaf.as_deref().unwrap().raw_open_error().unwrap()) as usize, raw);
            assert!(attempts.next().is_none());
            drop(host); assert_eq!(std::fs::read(&database).unwrap(), b"keep-original");
            std::fs::remove_file(database).unwrap(); remove_context_process(&base);
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preopen_fresh_retains_parent_and_atomic_leaf_before_any_sqlite_and_never_reopens() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let database = base.join("fresh.sqlite");
            let original = operation(); let mut host = opened_context_host(&original, &base);
            assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &base, &[]).unwrap(), Observation::Observed);
            host.preopen_files = Some(Box::new(PreopenRoot::reserve(&original).unwrap()));
            let root = host.preopen_files.as_deref_mut().unwrap();
            let context = host.storage_contexts.as_deref_mut().unwrap();
            assert_eq!(root.observe_once(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap(), &database, PreopenMode::Fresh), Observation::Observed);
            assert_eq!(root.leaf.as_deref().unwrap().leaf_created, Some(true));
            assert!(root.parent.as_deref().unwrap().canonical.is_none());
            let owner = std::ptr::from_ref(root.leaf.as_deref().unwrap().retained_file().unwrap()) as usize;
            let path = root.protected_path().unwrap(); let backing = path.as_os_str().as_encoded_bytes().as_ptr() as usize;
            let charged = original.captured_bytes();
            assert!(std::fs::rename(&database, base.join("renamed.sqlite")).is_err());
            assert_eq!(root.revalidate(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()), Observation::Observed);
            assert_eq!(root.observe_once(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap(), &database, PreopenMode::Fresh), Observation::Refused(Refusal::Closed));
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(std::ptr::from_ref(root.leaf.as_deref().unwrap().retained_file().unwrap()) as usize, owner);
            assert_eq!(root.protected_path().unwrap().as_os_str().as_encoded_bytes().as_ptr() as usize, backing);
            // Legacy-meter observation fixture, not an Arc bridge, Server
            // connection association, complete Host fit or membership issuer.
            drop(host); assert_eq!(std::fs::metadata(&database).unwrap().len(), 0);
            std::fs::remove_file(database).unwrap(); remove_context_process(&base);
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preopen_fresh_collision_keeps_raw_failure_without_truncate_fallback_or_retry() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let database = base.join("collision.sqlite"); std::fs::write(&database, b"original-bytes").unwrap();
            let original = operation(); let mut host = opened_context_host(&original, &base);
            assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &base, &[]).unwrap(), Observation::Observed);
            host.preopen_files = Some(Box::new(PreopenRoot::reserve(&original).unwrap()));
            let root = host.preopen_files.as_deref_mut().unwrap();
            let context = host.storage_contexts.as_deref_mut().unwrap();
            assert_eq!(root.observe_once(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap(), &database, PreopenMode::Fresh), Observation::Refused(Refusal::Open));
            let raw = root.leaf.as_deref().unwrap().raw_open_error().unwrap();
            assert_eq!(raw.kind(), std::io::ErrorKind::AlreadyExists);
            let pointer = std::ptr::from_ref(raw) as usize; let charged = original.captured_bytes();
            assert!(root.parent.is_some() && root.protected_path().is_err());
            assert_eq!(root.observe_once(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap(), &database, PreopenMode::Existing), Observation::Refused(Refusal::Closed));
            assert_eq!(root.revalidate(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()), Observation::Refused(Refusal::Open));
            assert_eq!(std::ptr::from_ref(root.leaf.as_deref().unwrap().raw_open_error().unwrap()) as usize, pointer);
            assert_eq!(original.captured_bytes(), charged);
            drop(host); assert_eq!(std::fs::read(&database).unwrap(), b"original-bytes");
            std::fs::remove_file(database).unwrap(); remove_context_process(&base);
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preopen_parent_outside_app_refuses_before_fresh_leaf_creation() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let app = base.join("app"); std::fs::create_dir(&app).unwrap();
            let database = base.join("outside.sqlite");
            let original = operation(); let mut host = opened_context_host(&original, &base);
            assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &app, &[]).unwrap(), Observation::Observed);
            host.preopen_files = Some(Box::new(PreopenRoot::reserve(&original).unwrap()));
            let root = host.preopen_files.as_deref_mut().unwrap();
            let context = host.storage_contexts.as_deref_mut().unwrap();
            assert_eq!(root.observe_once(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap(), &database, PreopenMode::Fresh), Observation::Refused(Refusal::Alias));
            assert!(root.parent.is_some() && root.leaf.is_none() && root.protected_path().is_err());
            assert!(!database.exists());
            drop(host); std::fs::remove_dir(app).unwrap(); remove_context_process(&base);
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preopen_existing_pair_compares_both_retained_endpoints_and_refuses_another_file() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let database = base.join("existing.sqlite"); let other = base.join("other.sqlite");
            std::fs::write(&database, b"first").unwrap(); std::fs::write(&other, b"second").unwrap();
            let original = operation(); let mut host = opened_context_host(&original, &base);
            assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &base, &[]).unwrap(), Observation::Observed);
            host.preopen_files = Some(Box::new(PreopenRoot::reserve(&original).unwrap()));
            let root = host.preopen_files.as_deref_mut().unwrap();
            let context = host.storage_contexts.as_deref_mut().unwrap();
            let journal = host.journal.as_deref_mut().unwrap(); let process = host.namespace_layouts.as_deref_mut().unwrap();
            assert_eq!(root.observe_once(context, journal, process, &database, PreopenMode::Existing), Observation::Observed);
            assert_eq!(root.leaf.as_deref().unwrap().leaf_created, None);
            host.member_files = Some(Box::new(MemberFilesRoot::reserve(&original).unwrap()));
            let files = host.member_files.as_deref_mut().unwrap();
            assert_eq!(files.observe_once(context, journal, process, &database, &database), Observation::Observed);
            let charged = original.captured_bytes();
            assert_eq!(root.pair_observed_files(files, context, journal, process), Observation::Observed);
            assert_eq!(original.captured_bytes(), charged);
            // This is an actual different-file fixture, not a claimed SQLite
            // race. File observations never supply connection/join provenance.
            let mut different = Box::new(MemberFilesRoot::reserve(&original).unwrap());
            different.next = host.member_files.take(); host.member_files = Some(different);
            let different = host.member_files.as_deref_mut().unwrap();
            assert_eq!(different.observe_once(context, journal, process, &other, &other), Observation::Observed);
            assert_eq!(root.pair_observed_files(different, context, journal, process), Observation::Refused(Refusal::Changed));
            assert!(root.protected_path().is_err() && root.leaf.as_deref().unwrap().retained_file().is_some());
            drop(host); std::fs::remove_file(database).unwrap(); std::fs::remove_file(other).unwrap(); remove_context_process(&base);
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn member_file_association_keeps_both_actual_chains_without_campaign_canonicalize_or_new_journal_open() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let database = base.join("member.sqlite"); std::fs::write(&database, b"retained-file-fixture").unwrap();
            let original = operation(); let mut host = opened_context_host(&original, &base);
            assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &base, &[]).unwrap(), Observation::Observed);
            let before = *host.journal.as_ref().unwrap().open_binding().unwrap();
            host.member_files = Some(Box::new(MemberFilesRoot::reserve(&original).unwrap()));
            let root = host.member_files.as_deref_mut().unwrap();
            let context = host.storage_contexts.as_deref_mut().unwrap();
            assert_eq!(root.observe_once(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap(), &database, &database), Observation::Observed);
            let left = root.database.as_deref().unwrap(); let right = root.archive.as_deref().unwrap();
            assert_eq!(MemberFilesRoot::regular_endpoint(left).unwrap(), MemberFilesRoot::regular_endpoint(right).unwrap());
            assert!(left.canonical.is_none() && right.canonical.is_none());
            let charged = original.captured_bytes();
            let left_owner = std::ptr::from_ref(left.guards.as_deref().unwrap().retained_file().unwrap()) as usize;
            assert_eq!(root.revalidate(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()), Observation::Observed);
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(std::ptr::from_ref(root.database.as_deref().unwrap().guards.as_deref().unwrap().retained_file().unwrap()) as usize, left_owner);
            let after = host.journal.as_ref().unwrap().open_binding().unwrap();
            assert_eq!(after.generation, before.generation); assert_eq!(after.incarnation, before.incarnation);
            assert!(host.scopes.is_none() && host.registration.is_none());
            // Legacy meter + observation fixture, not a SQLite data file,
            // actual archive binding, local-domain issuer or complete Host fit.
            drop(host); std::fs::remove_file(database).unwrap(); remove_context_process(&base);
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn member_file_association_refuses_different_file_or_outside_app_and_retains_both_owners() {
        tokio::runtime::Handle::current().spawn(async {
            for different in [false, true] {
                let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
                let app = base.join("app"); std::fs::create_dir(&app).unwrap();
                let database = if different { app.join("one.sqlite") } else { base.join("outside.sqlite") };
                let archive = if different { app.join("two.sqlite") } else { database.clone() };
                std::fs::write(&database, b"first-file").unwrap();
                if different { std::fs::write(&archive, b"second-file").unwrap(); }
                let original = operation(); let mut host = opened_context_host(&original, &base);
                assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &app, &[]).unwrap(), Observation::Observed);
                host.member_files = Some(Box::new(MemberFilesRoot::reserve(&original).unwrap()));
                let root = host.member_files.as_deref_mut().unwrap();
                let context = host.storage_contexts.as_deref_mut().unwrap();
                let expected = Observation::Refused(if different { Refusal::Changed } else { Refusal::Alias });
                assert_eq!(root.observe_once(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap(), &database, &archive), expected);
                assert!(MemberFilesRoot::regular_endpoint(root.database.as_deref().unwrap()).is_ok());
                assert!(MemberFilesRoot::regular_endpoint(root.archive.as_deref().unwrap()).is_ok());
                let charged = original.captured_bytes();
                assert_eq!(root.revalidate(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()), expected);
                assert_eq!(original.captured_bytes(), charged);
                drop(host); std::fs::remove_file(database).unwrap();
                if different { std::fs::remove_file(archive).unwrap(); }
                std::fs::remove_dir(app).unwrap(); remove_context_process(&base);
            }
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn member_file_missing_archive_retains_actual_open_error_and_never_retries_after_path_appears() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let database = base.join("member.sqlite"); let missing = base.join("missing.sqlite");
            std::fs::write(&database, b"existing-file").unwrap();
            let original = operation(); let mut host = opened_context_host(&original, &base);
            assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &base, &[]).unwrap(), Observation::Observed);
            host.member_files = Some(Box::new(MemberFilesRoot::reserve(&original).unwrap()));
            let root = host.member_files.as_deref_mut().unwrap();
            let context = host.storage_contexts.as_deref_mut().unwrap();
            assert_eq!(root.observe_once(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap(), &database, &missing), Observation::Refused(Refusal::Open));
            let error = root.archive.as_deref().unwrap().guards.as_deref().unwrap().raw_open_error().unwrap();
            let pointer = std::ptr::from_ref(error) as usize;
            let charged = original.captured_bytes();
            std::fs::write(&missing, b"late-file").unwrap();
            assert_eq!(root.revalidate(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()), Observation::Refused(Refusal::Open));
            assert_eq!(std::ptr::from_ref(root.archive.as_deref().unwrap().guards.as_deref().unwrap().raw_open_error().unwrap()) as usize, pointer);
            assert!(MemberFilesRoot::regular_endpoint(root.database.as_deref().unwrap()).is_ok());
            assert_eq!(original.captured_bytes(), charged);
            drop(host); std::fs::remove_file(database).unwrap(); std::fs::remove_file(missing).unwrap(); remove_context_process(&base);
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scope_context_observation_uses_one_prepaid_anchor_and_never_substitutes_another_context() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let original = operation(); let mut host = opened_context_host(&original, &base);
            assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &base, &[]).unwrap(), Observation::Observed);
            let before = original.captured_bytes();
            let context = host.storage_contexts.as_deref_mut().unwrap();
            let mut first = context.scope_context(host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()).unwrap();
            let charged = original.captured_bytes();
            assert_eq!(charged - before, crate::driver::owned_driver_task::own_arc_layout::<ContextAnchor>().unwrap().size());
            let mut retained = context.scope_context(host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()).unwrap();
            assert_eq!(original.captured_bytes(), charged);
            assert!(first.matches(context) && retained.matches(context));
            assert_eq!(retained.revalidate(context, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()), Observation::Observed);
            assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &base, &[]).unwrap(), Observation::Observed);
            let right = host.storage_contexts.as_deref_mut().unwrap();
            // Identical path and OPEN values cannot replace allocation identity.
            let second = right.scope_context(host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()).unwrap();
            assert!(second.matches(right) && !first.matches(right));
            assert_eq!(first.revalidate(right, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()), Observation::Refused(Refusal::Changed));
            let left = right.next.as_deref_mut().unwrap();
            assert_eq!(first.revalidate(left, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()), Observation::Refused(Refusal::Changed));
            assert_eq!(retained.revalidate(left, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()), Observation::Observed);
            let mut right = host.storage_contexts.take().unwrap();
            drop(right.next.take());
            assert!(retained.owner.upgrade().is_none());
            assert_eq!(retained.revalidate(&mut right, host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()), Observation::Refused(Refusal::Changed));
            host.storage_contexts = Some(right);
            assert!(host.scopes.is_none() && host.registration.is_none());
            // Legacy meter observation fixture only; no complete fit or lease.
            assert!(original.original_growth_usage().unwrap().is_none());
            drop(host); remove_context_process(&base);
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scope_context_without_actual_open_refuses_before_allocating_an_anchor() {
        tokio::runtime::Handle::current().spawn(async {
            let original = operation();
            let mut context = ContextRoot::reserve(&original).unwrap();
            let mut process = LayoutRoot::reserve(&original).unwrap();
            let mut journal = super::super::journal::JournalRoot::reserve(&original).unwrap();
            let before = original.captured_bytes();
            assert!(matches!(context.scope_context(&mut journal, &mut process), Err(Refusal::Closed)));
            assert!(context.scope_anchor.is_none() && context.requested_namespace.is_none() && context.app.is_none());
            assert_eq!(context.observation, Some(Observation::Refused(Refusal::Closed)));
            assert!(matches!(context.scope_context(&mut journal, &mut process), Err(Refusal::Closed)));
            assert_eq!(original.captured_bytes(), before);
            assert!(journal.open_binding().is_none());
        }).await.unwrap();
    }

    #[cfg(windows)]
    fn opened_context_host(original: &Operation, process: &Path) -> crate::seat_host::SeatHost {
        let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
        assert_eq!(host.observe_existing_namespace_layout(original, process, &[]).unwrap(), Observation::Observed);
        assert_eq!(host.prepare_observed_namespace().unwrap(), Observation::Observed);
        assert_eq!(host.canonicalize_observed_namespace_layout().unwrap(), Observation::Observed);
        host.reserve_journal(original).unwrap();
        assert_eq!(host.open_observed_journal().unwrap(), super::super::journal::Observation::Configured);
        assert_eq!(host.establish_observed_open().unwrap(), super::super::journal::Observation::Configured);
        assert!(host.journal.as_ref().unwrap().open_binding().is_some());
        host
    }

    #[cfg(windows)]
    fn remove_context_process(process: &Path) {
        std::fs::remove_file(process.join(".seat-host/host.sqlite")).unwrap();
        std::fs::remove_file(process.join(".seat-host/host.lock")).unwrap();
        std::fs::remove_dir(process.join(".seat-host")).unwrap();
        std::fs::remove_dir(process).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn app_context_retains_different_app_roots_on_one_actual_open_without_second_journal() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let app_path = base.join("app"); std::fs::create_dir(&app_path).unwrap();
            let original = operation();
            let mut host = opened_context_host(&original, &base);
            let binding = *host.journal.as_ref().unwrap().open_binding().unwrap();
            let journal = std::ptr::from_ref(host.journal.as_ref().unwrap().as_ref()) as usize;
            assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &app_path, &[]).unwrap(), Observation::Observed);
            assert_eq!(host.observe_storage_context(&base.join(".seat-host"), &base, &[]).unwrap(), Observation::Observed);
            let current = host.storage_contexts.as_deref().unwrap();
            let prior = current.next.as_deref().unwrap();
            assert!(current.original.same_instance(&original) && prior.original.same_instance(&original));
            assert!(current.check_with(prior).is_ok());
            for context in [current, prior] {
                assert_eq!(context.binding.as_ref().unwrap().generation, binding.generation);
                assert_eq!(context.binding.as_ref().unwrap().incarnation, binding.incarnation);
                assert!(context.process_owner.as_ref().unwrap().upgrade().is_some());
                let app = context.app.as_ref().unwrap();
                assert!(!app.creation_attempted && app.namespace.is_none() && app.lock_result.is_none());
                assert!(app.database_leaf.is_none() && app.journal_association.is_none());
            }
            assert_eq!(std::ptr::from_ref(host.journal.as_ref().unwrap().as_ref()) as usize, journal);
            assert_eq!(host.journal.as_ref().unwrap().open_binding().unwrap().incarnation, binding.incarnation);
            assert!(!app_path.join(".seat-host").exists());
            assert!(host.scopes.is_none() && host.registration.is_none());
            // This legacy-meter fixture verifies observations, not a complete
            // original fit, real scope admission or positive driver creation.
            assert!(original.original_growth_usage().unwrap().is_none());
            drop(host); std::fs::remove_dir(app_path).unwrap(); remove_context_process(&base);
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn app_context_namespace_switch_refuses_before_observing_app_or_changing_open() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let wrong = base.join("wrong-namespace"); std::fs::create_dir(&wrong).unwrap();
            let original = operation(); let mut host = opened_context_host(&original, &base);
            let binding = *host.journal.as_ref().unwrap().open_binding().unwrap();
            let absent_app = base.join("must-not-open");
            assert_eq!(host.observe_storage_context(&wrong, &absent_app, &[]).unwrap(), Observation::Refused(Refusal::Changed));
            assert!(host.admission_closed);
            let context = host.storage_contexts.as_deref_mut().unwrap();
            assert!(context.requested_namespace.is_some() && context.app.is_none());
            assert_eq!(context.observation, Some(Observation::Refused(Refusal::Changed)));
            let charged = original.captured_bytes();
            assert_eq!(context.revalidate(host.journal.as_deref_mut().unwrap(), host.namespace_layouts.as_deref_mut().unwrap()), Observation::Refused(Refusal::Changed));
            assert_eq!(original.captured_bytes(), charged);
            let current = host.journal.as_ref().unwrap().open_binding().unwrap();
            assert_eq!(current.generation, binding.generation); assert_eq!(current.incarnation, binding.incarnation);
            assert!(!wrong.join("host.sqlite").exists() && !absent_app.exists());
            drop(host); std::fs::remove_dir(wrong).unwrap(); remove_context_process(&base);
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn app_context_without_durable_open_retains_refusal_before_context_path_io() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_layout(&original, &base, &[]).unwrap(), Observation::Observed);
            assert_eq!(host.prepare_observed_namespace().unwrap(), Observation::Observed);
            host.reserve_journal(&original).unwrap();
            assert_eq!(host.open_observed_journal().unwrap(), super::super::journal::Observation::Configured);
            let before = original.captured_bytes();
            assert_eq!(host.observe_storage_context(Path::new("not-absolute"), Path::new("not-absolute"), &[]).unwrap(), Observation::Refused(Refusal::Closed));
            let context = host.storage_contexts.as_deref().unwrap();
            assert!(context.requested_namespace.is_none() && context.app.is_none());
            assert!(context.binding.is_none() && context.process_owner.is_none());
            assert_eq!(original.captured_bytes() - before, Layout::new::<ContextRoot>().size());
            assert!(host.journal.as_ref().unwrap().open_binding().is_none());
            assert!(host.scopes.is_none() && host.admission_closed);
            drop(host); remove_context_process(&base);
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canonical_bracket_retains_actual_result_fresh_file_and_normal_liability() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_chain(&original, &base, Role::Directory).unwrap(), Observation::Observed);
            let before = original.captured_bytes();
            assert_eq!(host.canonicalize_observed_namespace_chain().unwrap(), Observation::Observed);
            let chain = host.namespace_chains.as_deref().unwrap();
            let root = chain.canonical.as_deref().unwrap();
            let result_pointer = std::ptr::from_ref(root.returned.as_ref().unwrap()) as usize;
            let path = root.returned.as_ref().unwrap().as_ref().unwrap();
            let path_pointer = path.as_os_str().as_encoded_bytes().as_ptr() as usize;
            let n = path.as_os_str().as_encoded_bytes().len();
            assert_eq!(root.fresh.as_deref().unwrap().identity(), chain.guards.as_deref().unwrap().identity());
            assert!(root.domain.is_some()); assert!(matches!(root.release.as_ref(), Some(Ok(()))));
            assert!(root.attempted && !host.canonical_serial.busy);
            let charged = original.captured_bytes();
            assert_eq!(charged - before, Layout::new::<CanonicalRoot>().size() + CANONICAL_RETAINED_BYTES
                + Layout::new::<GuardRoot>().size() + n + 2 * (n + 1));
            assert_eq!(host.canonicalize_observed_namespace_chain().unwrap(), Observation::Observed);
            let root = host.namespace_chains.as_deref().unwrap().canonical.as_deref().unwrap();
            assert_eq!(std::ptr::from_ref(root.returned.as_ref().unwrap()) as usize, result_pointer);
            assert_eq!(root.returned.as_ref().unwrap().as_ref().unwrap().as_os_str().as_encoded_bytes().as_ptr() as usize, path_pointer);
            assert_eq!(original.captured_bytes(), charged); // No second canonical call/normal charge.
            // Success of another same-original reservation proves the actual C
            // was released, not subtracted from the ordinary retained prefix.
            let mut credit = original.reserve_host_canonical().unwrap();
            let fixture_result: Option<Result<(), std::io::Error>> = Some(Ok(()));
            assert!(fixture_result.is_some());
            credit.release_after_retained_result().unwrap();
            assert_eq!(original.captured_bytes(), charged);
            drop(host); assert_eq!(original.captured_bytes(), charged);
            std::fs::remove_dir(&base).unwrap();
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canonical_serial_refusal_is_sticky_before_std_even_with_another_original() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let first = operation(); let other = operation();
            assert!(!first.same_instance(&other));
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_chain(&other, &base, Role::Directory).unwrap(), Observation::Observed);
            // Explicit in-progress serial-state fixture, not a native race or
            // actual blocked canonical call. It applies across original meters.
            let mut held = first.reserve_host_canonical().unwrap();
            host.canonical_serial.busy = true;
            assert_eq!(host.canonicalize_observed_namespace_chain().unwrap(), Observation::Refused(Refusal::Closed));
            let root = host.namespace_chains.as_deref().unwrap().canonical.as_deref().unwrap();
            assert!(root.returned.is_none() && root.reservation.is_none() && root.fresh.is_none());
            assert!(root.attempted && host.canonical_serial.busy && host.namespace_refused);
            let charged = other.captured_bytes();
            host.canonical_serial.busy = false; // Original in-progress fixture has returned.
            let fixture_result: Option<Result<(), std::io::Error>> = Some(Ok(()));
            assert!(fixture_result.is_some()); held.release_after_retained_result().unwrap();
            assert!(host.canonicalize_observed_namespace_chain().is_err());
            assert_eq!(other.captured_bytes(), charged);
            drop(host); std::fs::remove_dir(&base).unwrap();
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canonical_mismatch_and_raw_error_keep_original_results_without_retry() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let other = base.join("other"); std::fs::create_dir(&other).unwrap();
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_chain(&original, &base, Role::Directory).unwrap(), Observation::Observed);
            let expected = *host.namespace_chains.as_deref().unwrap().directory_endpoint().unwrap();
            for exists in [true, false] {
                let path = if exists { other.clone() } else { base.join("missing") };
                let mut input = Box::new(GuardRoot::reserve(&original).unwrap());
                assert_eq!(input.prepare_owner_path(&path), PathObservation::Prepared);
                let input = input.native_path.as_deref().unwrap();
                let mut root = Box::new(CanonicalRoot::reserve(&original).unwrap());
                // Deliberate binding-fault fixture: actual retained base tuple
                // paired with a different or missing operator-root path. This
                // is not an OS resolution-race injection or production caller.
                let refusal = if exists { Refusal::Changed } else { Refusal::Canonicalize };
                assert_eq!(root.observe_once(&mut host.canonical_serial, input, expected), Observation::Refused(refusal));
                assert!(root.domain.is_none());
                let pointer = std::ptr::from_ref(root.returned.as_ref().unwrap()) as usize;
                if !exists { assert!(root.returned.as_ref().unwrap().as_ref().unwrap_err().raw_os_error().is_some()); }
                let charged = original.captured_bytes();
                assert_eq!(root.observe_once(&mut host.canonical_serial, input, expected), Observation::Refused(refusal));
                assert_eq!(std::ptr::from_ref(root.returned.as_ref().unwrap()) as usize, pointer);
                assert_eq!(original.captured_bytes(), charged);
                assert!(matches!(root.release.as_ref(), Some(Ok(()))))
            }
            drop(host); std::fs::remove_dir(&other).unwrap(); std::fs::remove_dir(&base).unwrap();
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canonical_layout_checks_all_roots_and_does_not_hide_exclusion_pair_ambiguity() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let storage = base.join("storage"); let one = base.join("one"); let two = base.join("two");
            for path in [&storage, &one, &two] { std::fs::create_dir(path).unwrap(); }
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_layout(&original, &storage, &[one.as_path(), two.as_path()]).unwrap(), Observation::Observed);
            assert_eq!(host.prepare_observed_namespace().unwrap(), Observation::Observed);
            assert_eq!(host.canonicalize_observed_namespace_layout().unwrap(), Observation::Observed);
            let layout = host.namespace_layouts.as_deref_mut().unwrap();
            assert_eq!(layout.local_roots().count(), 4);
            assert!(layout.local_roots().all(|root| root.canonical.as_deref().is_some_and(|call|
                call.attempted && call.returned.as_ref().is_some_and(Result::is_ok) && matches!(call.release.as_ref(), Some(Ok(()))))));
            // Explicit private-domain inconsistency fixture after the real
            // bracket, not simulated OS volumes or a validated-domain producer.
            let storage_domain = layout.storage.as_deref().unwrap().local_domain().unwrap();
            let alternate = if storage_domain.drive == b'Z' { b'Y' } else { b'Z' };
            let first = layout.excluded.as_deref_mut().unwrap();
            first.canonical.as_deref_mut().unwrap().domain = Some(LocalDomain { drive: alternate, serial: storage_domain.serial ^ 1 });
            first.next.as_deref_mut().unwrap().canonical.as_deref_mut().unwrap().domain = Some(LocalDomain { drive: alternate, serial: storage_domain.serial ^ 2 });
            assert_eq!(layout.validate_local_relations(), Err(Refusal::AmbiguousStorageDomain));
            drop(host);
            std::fs::remove_file(storage.join(".seat-host/host.sqlite")).unwrap();
            std::fs::remove_file(storage.join(".seat-host/host.lock")).unwrap();
            std::fs::remove_dir(storage.join(".seat-host")).unwrap();
            for path in [&storage, &one, &two, &base] { std::fs::remove_dir(path).unwrap(); }
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_domain_parser_never_promotes_unc_devices_or_caller_drive_spelling() {
        tokio::runtime::Handle::current().spawn(async {
            let tuple = Identity { volume: 23, file: 51, links: 1 };
            // Classification-only fixtures; no canonicalize/File/authority.
            assert_eq!(LocalDomain::classify(Path::new(r"\\?\c:\storage"), &tuple),
                Ok(LocalDomain { drive: b'C', serial: 23 }));
            for spelling in [r"C:\storage", r"storage", r"\\server\share\storage",
                r"\\?\UNC\server\share\storage", r"\\?\Volume{fixture}\storage",
                r"\\.\C:\storage", r"\\?\GLOBALROOT\Device\fixture",
                r"\\?\C:storage", r"\\?\C:\storage\..\other", "\\\\?\\C:\\nul\0tail"]
            {
                assert_eq!(LocalDomain::classify(Path::new(spelling), &tuple),
                    Err(Refusal::UnsupportedStorageDomain));
            }
            let missing = Identity { volume: 0, file: 51, links: 1 };
            assert_eq!(LocalDomain::classify(Path::new(r"\\?\C:\storage"), &missing),
                Err(Refusal::UnsupportedStorageDomain));
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_domain_matrix_refuses_alias_letters_and_inconsistent_serials() {
        tokio::runtime::Handle::current().spawn(async {
            let c1 = LocalDomain { drive: b'C', serial: 11 };
            let c2 = LocalDomain { drive: b'C', serial: 12 };
            let d1 = LocalDomain { drive: b'D', serial: 11 };
            let d2 = LocalDomain { drive: b'D', serial: 12 };
            assert_eq!(c1.relation(c1), Ok(DomainRelation::SameDrive));
            assert_eq!(c1.relation(d2), Ok(DomainRelation::DisjointDrives));
            assert_eq!(d2.relation(c1), Ok(DomainRelation::DisjointDrives));
            for (left, right) in [(c1, c2), (c2, c1), (c1, d1), (d1, c1)] {
                assert_eq!(left.relation(right), Err(Refusal::AmbiguousStorageDomain));
            }
            // SameDrive requires the actual retained ancestor relation later;
            // equal letter/serial here proves neither containment nor identity.
            assert_eq!(LocalDomain::classify(Path::new(r"\\?\c:\different-root"),
                &Identity { volume: 11, file: 999, links: 1 }).unwrap(), c1);
        }).await.unwrap();
    }

    fn operation() -> Operation {
        Operation::new(String::from("namespace-observation-source"), 1,
            Allowance::required(Some(Instant::now() + Duration::from_secs(30))).unwrap()).unwrap()
    }

    fn paths() -> (PathBuf, PathBuf) {
        let path = std::env::temp_dir().join(format!("cna-namespace-{}", uuid::Uuid::new_v4()));
        let alias = path.with_extension("alias");
        (path, alias)
    }

    // AUTHORED/UNRUN. Future STRICT only; these never exercise a CLI/provider.
    #[cfg(any(windows, unix))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actual_file_is_owned_before_observation_and_reentry_preserves_pending() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
        let (path, alias) = paths();
        let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).unwrap();
        let extra = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&alias).unwrap();
        let original = operation();
        let before = original.captured_bytes();
        let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
        let mut pending = Some(file);
        assert_eq!(host.observe_namespace_file(&original, &mut pending, Role::RegularFile).unwrap(),
            Observation::Observed);
        assert_eq!(original.captured_bytes() - before, Layout::new::<GuardRoot>().size());
        let charged = original.captured_bytes();
        let root = host.namespace_observations.as_deref_mut().unwrap();
        assert!(pending.is_none());
        assert!(root.file.is_some());
        assert_eq!(root.identity().unwrap().links, 1);
        let identity = std::ptr::from_ref(root.identity().unwrap()) as usize;
        let mut refused = Some(extra);
        assert_eq!(root.observe(&mut refused, Role::RegularFile), Observation::Refused(Refusal::Closed));
        assert!(refused.is_some());
        assert_eq!(std::ptr::from_ref(root.identity().unwrap()) as usize, identity);
        assert_eq!(original.captured_bytes(), charged);
        drop(refused); drop(host);
        std::fs::remove_file(path).unwrap(); std::fs::remove_file(alias).unwrap();
        });
        control.await.unwrap();
    }

    #[cfg(any(windows, unix))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_regular_hardlink_refuses_and_retains_actual_metadata_without_refund() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
        let (path, alias) = paths();
        let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).unwrap();
        std::fs::hard_link(&path, &alias).unwrap(); // Unsupported fixture is failure, not skip/pass.
        let original = operation();
        let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
        let mut pending = Some(file);
        assert_eq!(host.observe_namespace_file(&original, &mut pending, Role::RegularFile).unwrap(),
            Observation::Refused(Refusal::Link));
        let charged = original.captured_bytes();
        let root = host.namespace_observations.as_ref().unwrap();
        assert!(pending.is_none()); assert!(root.file.is_some()); assert!(root.identity().is_none());
        #[cfg(unix)] {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(root.metadata.as_ref().unwrap().as_ref().unwrap().nlink(), 2);
        }
        #[cfg(windows)] {
            assert_eq!(root.information.as_ref().unwrap().as_ref().unwrap().number_of_links(), 2);
        }
        assert_eq!(original.captured_bytes(), charged);
        drop(host);
        std::fs::remove_file(path).unwrap(); std::fs::remove_file(alias).unwrap();
        });
        control.await.unwrap();
    }

    #[test]
    fn empty_pending_refuses_without_query_or_fallback() {
        let original = operation();
        let mut root = Box::new(GuardRoot::reserve(&original).unwrap());
        let charged = original.captured_bytes();
        assert_eq!(root.observe(&mut None, Role::RegularFile), Observation::Refused(Refusal::Closed));
        assert!(root.file.is_none()); assert!(root.identity().is_none()); assert!(root.raw_error().is_none());
        assert_eq!(original.captured_bytes(), charged);
    }

    #[cfg(not(any(windows, unix)))]
    #[test]
    fn unsupported_target_returns_explicit_refusal_without_identity() {
        let original = operation();
        let mut root = Box::new(GuardRoot::reserve(&original).unwrap());
        assert_eq!(root.query(Role::RegularFile), Observation::Refused(Refusal::UnsupportedTarget));
        assert!(root.file.is_none()); assert!(root.identity().is_none());
    }

    #[cfg(any(windows, unix))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn same_regular_file_refuses_alias_and_keeps_both_original_owners() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
            let (path, _) = paths();
            let file = std::fs::OpenOptions::new().read(true).write(true)
                .create_new(true).open(&path).unwrap();
            let second = std::fs::File::open(&path).unwrap();
            let third = std::fs::File::open(&path).unwrap();
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_namespace_file(&original, &mut Some(file), Role::RegularFile)
                .unwrap(), Observation::Observed);
            let mut pending = Some(second);
            assert_eq!(host.observe_namespace_file(&original, &mut pending, Role::RegularFile)
                .unwrap(), Observation::Refused(Refusal::Alias));
            assert!(pending.is_none());
            let newest = host.namespace_observations.as_ref().unwrap();
            let previous = newest.next.as_ref().unwrap();
            assert!(newest.file.is_some()); assert!(previous.file.is_some());
            assert_eq!(newest.identity(), previous.identity());
            assert_eq!(newest.observation, Some(Observation::Refused(Refusal::Alias)));
            assert_eq!(previous.observation, Some(Observation::Observed));
            let charged = original.captured_bytes();
            let mut refused = Some(third);
            assert_eq!(host.observe_namespace_file(&original, &mut refused, Role::RegularFile)
                .unwrap_err(), OperationError::CreditOwned);
            assert!(refused.is_some()); assert_eq!(original.captured_bytes(), charged);
            drop(refused); drop(host); std::fs::remove_file(path).unwrap();
        });
        control.await.unwrap();
    }

    #[cfg(any(windows, unix))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn changed_link_count_latches_first_recheck_and_does_not_retry_or_refund() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
            let (path, alias) = paths();
            let file = std::fs::OpenOptions::new().read(true).write(true)
                .create_new(true).open(&path).unwrap();
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_namespace_file(&original, &mut Some(file), Role::RegularFile)
                .unwrap(), Observation::Observed);
            assert_eq!(host.revalidate_namespace_handles(), Observation::Observed);
            let charged = original.captured_bytes();
            let identity = std::ptr::from_ref(host.namespace_observations.as_ref()
                .unwrap().identity().unwrap()) as usize;
            std::fs::hard_link(&path, &alias).unwrap();
            assert_eq!(host.revalidate_namespace_handles(), Observation::Refused(Refusal::Link));
            std::fs::remove_file(alias).unwrap();
            let root = host.namespace_observations.as_deref_mut().unwrap();
            assert_eq!(root.revalidate_handle(), Observation::Refused(Refusal::Link));
            assert!(root.file.is_some()); assert_eq!(root.identity().unwrap().links, 1);
            assert_eq!(std::ptr::from_ref(root.identity().unwrap()) as usize, identity);
            #[cfg(windows)] {
                assert_eq!(root.information.as_ref().unwrap().as_ref().unwrap().number_of_links(), 1);
                assert_eq!(root.recheck_information.as_ref().unwrap().as_ref().unwrap().number_of_links(), 2);
            }
            #[cfg(unix)] {
                use std::os::unix::fs::MetadataExt;
                assert_eq!(root.metadata.as_ref().unwrap().as_ref().unwrap().nlink(), 1);
                assert_eq!(root.recheck_metadata.as_ref().unwrap().as_ref().unwrap().nlink(), 2);
            }
            assert!(root.recheck_error().is_none());
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(host.revalidate_namespace_handles(), Observation::Refused(Refusal::Closed));
            assert_eq!(original.captured_bytes(), charged);
            drop(host); std::fs::remove_file(path).unwrap();
        });
        control.await.unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn staged_opener_rejects_general_and_ambiguous_spellings_without_io() {
        for rejected in [r"C:\campaigns\host.sqlite", r"host.sqlite",
            r"\\?\C:\campaigns\..\host.sqlite", r"\\?\C:\campaigns\host.sqlite:stream",
            r"\\?\C:\campaigns\host.sqlite.", r"\\.\NUL"]
        {
            let original = operation();
            let mut root = Box::new(GuardRoot::reserve(&original).unwrap());
            let charged = original.captured_bytes();
            assert_eq!(root.open_existing_verbatim(Path::new(rejected), Role::RegularFile),
                Observation::Refused(Refusal::Path));
            assert!(root.native_path.is_none()); assert!(root.opened.is_none());
            assert!(root.retained_file().is_none());
            assert_eq!(original.captured_bytes(), charged);
        }
        assert!(bounded_verbatim(Path::new(r"\\?\C:\campaigns\.seat-host\host.sqlite")));
        assert!(bounded_verbatim(Path::new(r"\\?\UNC\server\share\.seat-host\host.sqlite")));
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn staged_existing_open_prepays_buffers_and_denies_delete_sharing_until_owner_drop() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
            let (path, alias) = paths();
            std::fs::write(&path, b"fixture").unwrap();
            let native = std::fs::canonicalize(&path).unwrap(); // Fixture only.
            assert!(bounded_verbatim(&native));
            let bytes = native.as_os_str().as_encoded_bytes().len();
            let original = operation();
            let before = original.captured_bytes();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.open_namespace_existing_verbatim(&original, &native, Role::RegularFile)
                .unwrap(), Observation::Observed);
            let expected = Layout::new::<GuardRoot>().size() + bytes
                + Layout::array::<u16>(bytes + 1).unwrap().size();
            assert_eq!(original.captured_bytes() - before, expected);
            let root = host.namespace_observations.as_ref().unwrap();
            assert_eq!(root.native_path.as_deref(), Some(native.as_path()));
            assert!(matches!(root.opened.as_ref(), Some(Ok(_))));
            assert!(root.file.is_none()); assert!(root.retained_file().is_some());
            assert!(root.raw_open_error().is_none());
            assert!(std::fs::rename(&path, &alias).is_err());
            assert_eq!(host.revalidate_namespace_handles(), Observation::Observed);
            let charged = original.captured_bytes();
            drop(host);
            assert_eq!(original.captured_bytes(), charged);
            std::fs::rename(&path, &alias).unwrap();
            std::fs::remove_file(alias).unwrap();
        });
        control.await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn staged_open_failure_retains_original_path_and_raw_error_without_retry() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
            let (path, _) = paths();
            let directory = std::fs::canonicalize(path.parent().unwrap()).unwrap();
            let native = directory.join(path.file_name().unwrap());
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.open_namespace_existing_verbatim(&original, &native, Role::RegularFile)
                .unwrap(), Observation::Refused(Refusal::Open));
            let charged = original.captured_bytes();
            let root = host.namespace_observations.as_deref_mut().unwrap();
            let cause = std::ptr::from_ref(root.raw_open_error().unwrap()) as usize;
            assert_eq!(root.native_path.as_deref(), Some(native.as_path()));
            assert!(root.retained_file().is_none()); assert!(root.identity().is_none());
            assert_eq!(root.open_existing_verbatim(&directory, Role::Directory),
                Observation::Refused(Refusal::Closed));
            assert_eq!(std::ptr::from_ref(root.raw_open_error().unwrap()) as usize, cause);
            assert_eq!(host.open_namespace_existing_verbatim(&original, &directory, Role::Directory)
                .unwrap_err(), OperationError::CreditOwned);
            assert_eq!(original.captured_bytes(), charged);
        });
        control.await.unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn staged_owner_path_rejects_raw_ambiguity_before_child_allocation_or_io() {
        for rejected in [r"host.sqlite", r"C:host.sqlite", r"\host.sqlite",
            r"C:\campaigns\.\host.sqlite", r"C:\campaigns\..\host.sqlite",
            r"C:\campaigns\\host.sqlite", r"C:\campaigns\host.sqlite:stream",
            r"C:\campaigns\host.sqlite.", r"C:\campaigns\CON.txt",
            r"C:\campaigns\con .txt", r"C:\campaigns\COM¹.txt",
            r"C:\campaigns\lpt9", r"C:\campaigns\CONOUT$", r"C:\campaigns\file?",
            r"C:/campaigns/host.sqlite", r"\\.\NUL", r"\\?\GLOBALROOT\Device\x",
            r"\\server", r"\\server\\share\host.sqlite", r"\\server\share\..\host.sqlite",
            r"\\?\C:\campaigns\.\host.sqlite", "C:\\campaigns\\bad\0name"]
        {
            let original = operation();
            let mut root = Box::new(GuardRoot::reserve(&original).unwrap());
            let charged = original.captured_bytes();
            assert_eq!(root.prepare_owner_path(Path::new(rejected)), PathObservation::Refused(Refusal::Path));
            assert!(root.native_path.is_none()); assert!(root.opened.is_none());
            assert!(root.retained_file().is_none()); assert!(root.identity().is_none());
            assert_eq!(root.prepare_owner_path(Path::new(r"C:\valid\host.sqlite")),
                PathObservation::Refused(Refusal::Closed));
            assert_eq!(original.captured_bytes(), charged);
        }
        let too_long = format!("C:\\{}", "a".repeat(4090));
        assert_eq!(too_long.len(), 4093);
        assert!(owner_path_parts(Path::new(&too_long)).is_none()); // Verbatim output exceeds4096.
        use std::os::windows::ffi::OsStringExt;
        let unpaired = std::ffi::OsString::from_wide(&[67, 58, 92, 0xD800]);
        assert!(owner_path_parts(Path::new(&unpaired)).is_none());
        let original = operation();
        let mut root = Box::new(GuardRoot::reserve(&original).unwrap());
        let input = Path::new(r"C:\campaigns\host.sqlite");
        let required = owner_path_parts(input).unwrap().2;
        original.charge(crate::operation::OPERATION_BYTES - original.captured_bytes() - required + 1).unwrap();
        let charged = original.captured_bytes();
        assert_eq!(root.prepare_owner_path(input), PathObservation::Refused(Refusal::Closed));
        assert_eq!(root.credit_refusal(), Some(&OperationError::CaptureOverflow));
        let cause = std::ptr::from_ref(root.credit_refusal().unwrap()) as usize;
        assert!(root.native_path.is_none()); assert!(root.opened.is_none());
        assert_eq!(root.prepare_owner_path(input), PathObservation::Refused(Refusal::Closed));
        assert_eq!(std::ptr::from_ref(root.credit_refusal().unwrap()) as usize, cause);
        assert_eq!(original.captured_bytes(), charged);
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn staged_owner_path_is_host_retained_and_exactly_prepaid_without_file_authority() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
            for (input, expected) in [
                (r"C:\campaigns\.seat-host\host.sqlite", r"\\?\C:\campaigns\.seat-host\host.sqlite"),
                (r"\\server\share\.seat-host\host.sqlite", r"\\?\UNC\server\share\.seat-host\host.sqlite"),
                (r"\\?\C:\campaigns\host.sqlite", r"\\?\C:\campaigns\host.sqlite"),
                (r"\\?\UNC\server\share\host.sqlite", r"\\?\UNC\server\share\host.sqlite"),
                (r"C:\", r"\\?\C:\"), (r"\\server\share", r"\\?\UNC\server\share"),
                (r"C:\café\東京\🦀\host.sqlite", r"\\?\C:\café\東京\🦀\host.sqlite"),
            ] {
                let original = operation();
                let before = original.captured_bytes();
                let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
                assert_eq!(host.prepare_namespace_owner_path(&original, Path::new(input)).unwrap(),
                    PathObservation::Prepared);
                assert!(!host.namespace_refused);
                let charged = original.captured_bytes();
                assert_eq!(charged - before, Layout::new::<GuardRoot>().size() + expected.len());
                let root = host.namespace_observations.as_deref_mut().unwrap();
                let native = root.native_path.as_ref().unwrap();
                assert_eq!(native.as_path(), Path::new(expected));
                assert_eq!(native.capacity(), expected.len());
                let storage = native.as_os_str().as_encoded_bytes().as_ptr() as usize;
                assert!(root.opened.is_none()); assert!(root.retained_file().is_none());
                assert!(root.identity().is_none()); assert!(root.observation.is_none());
                assert!(root.information.is_none()); assert!(root.recheck_information.is_none());
                assert_eq!(root.prepare_owner_path(Path::new(r"C:\other")), PathObservation::Refused(Refusal::Closed));
                assert_eq!(root.native_path.as_ref().unwrap().as_os_str().as_encoded_bytes().as_ptr() as usize, storage);
                let mut no_file = None;
                assert_eq!(root.observe(&mut no_file, Role::RegularFile), Observation::Refused(Refusal::Closed));
                assert_eq!(original.captured_bytes(), charged);
                // A prepared spelling cannot be promoted by a handle recheck.
                assert_eq!(host.revalidate_namespace_handles(), Observation::Refused(Refusal::Closed));
                assert!(host.namespace_refused);
                drop(host);
                assert_eq!(original.captured_bytes(), charged);
            }
        });
        control.await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prepared_existing_open_keeps_same_host_node_path_and_one_native_buffer_charge() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
            let (path, alias) = paths();
            std::fs::write(&path, b"fixture").unwrap();
            let bytes = owner_path_parts(&path).unwrap().2;
            let original = operation();
            let before = original.captured_bytes();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.prepare_namespace_owner_path(&original, &path).unwrap(), PathObservation::Prepared);
            let root = host.namespace_observations.as_ref().unwrap();
            let node = std::ptr::from_ref(root.as_ref()) as usize;
            let buffer = root.native_path.as_ref().unwrap().as_os_str().as_encoded_bytes().as_ptr() as usize;
            assert_eq!(original.captured_bytes() - before, Layout::new::<GuardRoot>().size() + bytes);
            assert_eq!(host.open_prepared_namespace_existing(Role::RegularFile).unwrap(), Observation::Observed);
            let charged = original.captured_bytes();
            assert_eq!(charged - before, Layout::new::<GuardRoot>().size() + bytes
                + Layout::array::<u16>(bytes + 1).unwrap().size());
            let root = host.namespace_observations.as_deref_mut().unwrap();
            assert_eq!(std::ptr::from_ref(root) as usize, node);
            assert_eq!(root.native_path.as_ref().unwrap().as_os_str().as_encoded_bytes().as_ptr() as usize, buffer);
            assert!(root.next.is_none()); assert!(root.file.is_none());
            assert!(matches!(root.opened.as_ref(), Some(Ok(_))));
            let identity = std::ptr::from_ref(root.identity().unwrap()) as usize;
            assert_eq!(root.open_prepared_existing(Role::RegularFile), Observation::Refused(Refusal::Closed));
            assert_eq!(std::ptr::from_ref(root.identity().unwrap()) as usize, identity);
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(host.revalidate_namespace_handles(), Observation::Observed);
            assert!(std::fs::rename(&path, &alias).is_err());
            drop(host);
            assert_eq!(original.captured_bytes(), charged);
            std::fs::rename(&path, &alias).unwrap(); std::fs::remove_file(alias).unwrap();
        });
        control.await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prepared_open_failure_and_short_credit_keep_path_first_cause_and_no_retry() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
            let (missing, _) = paths();
            let bytes = owner_path_parts(&missing).unwrap().2;
            let original = operation();
            let before = original.captured_bytes();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.prepare_namespace_owner_path(&original, &missing).unwrap(), PathObservation::Prepared);
            let root = host.namespace_observations.as_ref().unwrap();
            let buffer = root.native_path.as_ref().unwrap().as_os_str().as_encoded_bytes().as_ptr() as usize;
            assert_eq!(host.open_prepared_namespace_existing(Role::RegularFile).unwrap(), Observation::Refused(Refusal::Open));
            let charged = original.captured_bytes();
            assert_eq!(charged - before, Layout::new::<GuardRoot>().size() + bytes
                + Layout::array::<u16>(bytes + 1).unwrap().size());
            let root = host.namespace_observations.as_deref_mut().unwrap();
            let cause = std::ptr::from_ref(root.raw_open_error().unwrap()) as usize;
            assert_eq!(root.native_path.as_ref().unwrap().as_os_str().as_encoded_bytes().as_ptr() as usize, buffer);
            assert!(root.retained_file().is_none()); assert!(root.information.is_none());
            assert!(root.identity().is_none()); assert!(root.next.is_none());
            assert_eq!(root.open_prepared_existing(Role::RegularFile), Observation::Refused(Refusal::Closed));
            assert_eq!(std::ptr::from_ref(root.raw_open_error().unwrap()) as usize, cause);
            assert_eq!(host.open_prepared_namespace_existing(Role::RegularFile).unwrap_err(), OperationError::CreditOwned);
            drop(host); assert_eq!(original.captured_bytes(), charged);

            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.prepare_namespace_owner_path(&original, &missing).unwrap(), PathObservation::Prepared);
            let wide_bytes = Layout::array::<u16>(bytes + 1).unwrap().size();
            original.charge(crate::operation::OPERATION_BYTES - original.captured_bytes() - wide_bytes + 1).unwrap();
            let charged = original.captured_bytes();
            let root = host.namespace_observations.as_ref().unwrap();
            let buffer = root.native_path.as_ref().unwrap().as_os_str().as_encoded_bytes().as_ptr() as usize;
            assert_eq!(host.open_prepared_namespace_existing(Role::RegularFile).unwrap(), Observation::Refused(Refusal::Closed));
            let root = host.namespace_observations.as_deref_mut().unwrap();
            assert_eq!(root.credit_refusal(), Some(&OperationError::CaptureOverflow));
            let cause = std::ptr::from_ref(root.credit_refusal().unwrap()) as usize;
            assert!(root.opened.is_none()); assert!(root.retained_file().is_none());
            assert!(root.information.is_none()); assert!(root.identity().is_none());
            assert_eq!(root.native_path.as_ref().unwrap().as_os_str().as_encoded_bytes().as_ptr() as usize, buffer);
            assert_eq!(root.open_prepared_existing(Role::RegularFile), Observation::Refused(Refusal::Closed));
            assert_eq!(std::ptr::from_ref(root.credit_refusal().unwrap()) as usize, cause);
            assert_eq!(original.captured_bytes(), charged);
            drop(host); assert_eq!(original.captured_bytes(), charged);
        });
        control.await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ancestor_chain_retains_actual_root_first_opens_and_every_exact_path_charge() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
            let (path, _) = paths();
            std::fs::write(&path, b"fixture").unwrap();
            let original = operation();
            let before = original.captured_bytes();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_chain(&original, &path, Role::RegularFile).unwrap(),
                Observation::Observed);
            let chain = host.namespace_chains.as_deref_mut().unwrap();
            let source = chain.source.as_ref().unwrap();
            assert!(source.retained_file().is_none()); assert!(source.opened.is_none());
            let native = source.native_path.as_ref().unwrap();
            let mut expected = Layout::new::<ChainRoot>().size() + Layout::new::<GuardRoot>().size()
                + native.as_os_str().as_encoded_bytes().len();
            let mut actual = chain.guards.as_deref();
            for (index, prefix) in native.ancestors().enumerate() {
                let guard = actual.unwrap();
                assert_eq!(guard.native_path.as_deref(), Some(prefix));
                assert_eq!(guard.role, Some(if index == 0 { Role::RegularFile } else { Role::Directory }));
                assert_eq!(guard.observation, Some(Observation::Observed));
                assert!(guard.retained_file().is_some()); assert!(guard.identity().is_some());
                let n = prefix.as_os_str().as_encoded_bytes().len();
                expected += Layout::new::<GuardRoot>().size() + n
                    + Layout::array::<u16>(n + 1).unwrap().size();
                actual = guard.next.as_deref();
            }
            assert!(actual.is_none());
            assert_eq!(original.captured_bytes() - before, expected);
            let charged = original.captured_bytes();
            assert_eq!(chain.revalidate(), Observation::Observed);
            assert_eq!(chain.observe_existing(&path, Role::RegularFile), Observation::Refused(Refusal::Closed));
            assert_eq!(original.captured_bytes(), charged);
            drop(host); assert_eq!(original.captured_bytes(), charged);
            std::fs::remove_file(path).unwrap();
        });
        control.await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn directory_guard_refuses_preexisting_writer_and_prevents_later_write_access() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
            use std::os::windows::fs::OpenOptionsExt;
            let (directory, _) = paths();
            std::fs::create_dir(&directory).unwrap();
            let native = std::fs::canonicalize(&directory).unwrap(); // Fixture only.
            let writer = std::fs::OpenOptions::new().read(true).write(true)
                .share_mode(0x3).custom_flags(0x0220_0000).open(&native).unwrap();
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.open_namespace_existing_verbatim(&original, &native, Role::Directory).unwrap(),
                Observation::Refused(Refusal::Open));
            let root = host.namespace_observations.as_deref_mut().unwrap();
            let cause = std::ptr::from_ref(root.raw_open_error().unwrap()) as usize;
            drop(writer);
            assert_eq!(root.open_existing_verbatim(&native, Role::Directory), Observation::Refused(Refusal::Closed));
            assert_eq!(std::ptr::from_ref(root.raw_open_error().unwrap()) as usize, cause);
            drop(host);
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.open_namespace_existing_verbatim(&original, &native, Role::Directory).unwrap(),
                Observation::Observed);
            assert!(std::fs::OpenOptions::new().read(true).write(true)
                .share_mode(0x3).custom_flags(0x0220_0000).open(&native).is_err());
            drop(host); std::fs::remove_dir(directory).unwrap();
        });
        control.await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ancestor_chain_refuses_intermediate_reparse_before_descendant_open_and_retains_prefix() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
            let (base, _) = paths();
            std::fs::create_dir(&base).unwrap();
            let target = base.join("target"); let alias = base.join("alias");
            std::fs::create_dir(&target).unwrap();
            std::fs::write(target.join("leaf"), b"fixture").unwrap();
            // A fixture setup failure fails this authored control; never skip
            // and present a missing reparse exercise as a successful refusal.
            std::os::windows::fs::symlink_dir(&target, &alias).unwrap();
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_chain(&original, &alias.join("leaf"), Role::RegularFile).unwrap(),
                Observation::Refused(Refusal::Link));
            assert!(host.namespace_refused);
            let chain = host.namespace_chains.as_deref_mut().unwrap();
            let guard = chain.guards.as_ref().unwrap();
            assert_eq!(guard.native_path.as_ref().unwrap().file_name().unwrap(), "alias");
            assert_eq!(guard.role, Some(Role::Directory));
            assert!(guard.information.as_ref().unwrap().as_ref().unwrap().file_attributes() & 0x400 != 0);
            assert!(guard.identity().is_none()); assert!(guard.next.is_some());
            let charged = original.captured_bytes();
            assert_eq!(chain.revalidate(), Observation::Refused(Refusal::Link));
            assert_eq!(original.captured_bytes(), charged);
            drop(host);
            std::fs::remove_dir(alias).unwrap(); std::fs::remove_file(target.join("leaf")).unwrap();
            std::fs::remove_dir(target).unwrap(); std::fs::remove_dir(base).unwrap();
        });
        control.await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn layout_distinguishes_real_siblings_from_lexical_prefixes_with_same_ancestors() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
            let (base, _) = paths();
            std::fs::create_dir(&base).unwrap();
            let storage = base.join("campaign"); let excluded = base.join("campaigns");
            std::fs::create_dir(&storage).unwrap(); std::fs::create_dir(&excluded).unwrap();
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_layout(&original, &storage, &[excluded.as_path()]).unwrap(),
                Observation::Observed);
            let layout = host.namespace_layouts.as_deref_mut().unwrap();
            let storage_root = layout.storage.as_ref().unwrap();
            let excluded_root = layout.excluded.as_ref().unwrap();
            assert_eq!(storage_root.directory_relation(excluded_root).unwrap(), DirectoryRelation::Disjoint);
            assert!(storage_root.guards.as_ref().unwrap().next.is_some());
            assert!(excluded_root.guards.as_ref().unwrap().next.is_some());
            let storage_node = std::ptr::from_ref(storage_root.as_ref()) as usize;
            let excluded_node = std::ptr::from_ref(excluded_root.as_ref()) as usize;
            let charged = original.captured_bytes();
            assert_eq!(layout.revalidate(), Observation::Observed);
            assert_eq!(std::ptr::from_ref(layout.storage.as_ref().unwrap().as_ref()) as usize, storage_node);
            assert_eq!(std::ptr::from_ref(layout.excluded.as_ref().unwrap().as_ref()) as usize, excluded_node);
            assert_eq!(original.captured_bytes(), charged);
            drop(host); assert_eq!(original.captured_bytes(), charged);
            std::fs::remove_dir(storage).unwrap(); std::fs::remove_dir(excluded).unwrap();
            std::fs::remove_dir(base).unwrap();
        });
        control.await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn layout_refuses_same_and_both_directions_of_directory_overlap_with_roots_retained() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let storage = base.join("store"); std::fs::create_dir(&storage).unwrap();
            let child = storage.join("child"); std::fs::create_dir(&child).unwrap();
            for (excluded, relation) in [(storage.as_path(), DirectoryRelation::Same),
                (child.as_path(), DirectoryRelation::Contains), (base.as_path(), DirectoryRelation::Within)]
            {
                let original = operation();
                let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
                assert_eq!(host.observe_existing_namespace_layout(&original, &storage, &[excluded]).unwrap(),
                    Observation::Refused(Refusal::Alias));
                assert!(host.namespace_refused);
                let layout = host.namespace_layouts.as_deref_mut().unwrap();
                let storage_root = layout.storage.as_ref().unwrap();
                let excluded_root = layout.excluded.as_ref().unwrap();
                assert_eq!(storage_root.directory_relation(excluded_root).unwrap(), relation);
                assert!(storage_root.guards.as_ref().unwrap().retained_file().is_some());
                assert!(excluded_root.guards.as_ref().unwrap().retained_file().is_some());
                let charged = original.captured_bytes();
                assert_eq!(layout.revalidate(), Observation::Refused(Refusal::Alias));
                assert_eq!(layout.observe_existing(&storage, &[]), Observation::Refused(Refusal::Closed));
                assert_eq!(original.captured_bytes(), charged);
                drop(host); assert_eq!(original.captured_bytes(), charged);
            }
            std::fs::remove_dir(child).unwrap(); std::fs::remove_dir(storage).unwrap();
            std::fs::remove_dir(base).unwrap();
        });
        control.await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn layout_missing_exclusion_refuses_and_keeps_original_failing_prefix_without_retry() {
        let runtime = tokio::runtime::Handle::current();
        let control = runtime.spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let storage = base.join("store"); std::fs::create_dir(&storage).unwrap();
            let missing = base.join("missing");
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_layout(&original, &storage, &[missing.as_path()]).unwrap(),
                Observation::Refused(Refusal::Open));
            let layout = host.namespace_layouts.as_deref_mut().unwrap();
            assert!(layout.storage.as_ref().unwrap().guards.as_ref().unwrap().retained_file().is_some());
            let failed = layout.excluded.as_ref().unwrap().guards.as_ref().unwrap();
            let cause = std::ptr::from_ref(failed.raw_open_error().unwrap()) as usize;
            assert!(failed.retained_file().is_none()); assert!(failed.next.is_some());
            assert_eq!(failed.native_path.as_ref().unwrap().file_name().unwrap(), "missing");
            let charged = original.captured_bytes();
            assert_eq!(layout.revalidate(), Observation::Refused(Refusal::Open));
            let failed = layout.excluded.as_ref().unwrap().guards.as_ref().unwrap();
            assert_eq!(std::ptr::from_ref(failed.raw_open_error().unwrap()) as usize, cause);
            assert_eq!(host.observe_existing_namespace_layout(&original, &storage, &[]).unwrap_err(), OperationError::CreditOwned);
            assert_eq!(original.captured_bytes(), charged);
            drop(host); assert_eq!(original.captured_bytes(), charged);
            assert!(!missing.exists());
            std::fs::remove_dir(storage).unwrap(); std::fs::remove_dir(base).unwrap();
        });
        control.await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn namespace_creation_retains_atomic_newness_and_reopens_existing_without_truncation() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_layout(&original, &base, &[]).unwrap(), Observation::Observed);
            assert_eq!(host.prepare_observed_namespace().unwrap(), Observation::Observed);
            let layout = host.namespace_layouts.as_deref_mut().unwrap();
            assert!(matches!(layout.directory_creation.as_ref(), Some(Ok(()))));
            assert!(matches!(layout.lock_result.as_ref(), Some(Ok(()))));
            for leaf in [layout.lock_leaf.as_ref().unwrap(), layout.database_leaf.as_ref().unwrap()] {
                assert_eq!(leaf.leaf_created, Some(true));
                assert!(matches!(leaf.leaf_probe.as_ref(), Some(Err(error)) if error.kind() == io::ErrorKind::NotFound));
                assert!(leaf.retained_file().is_some()); assert_eq!(leaf.identity().unwrap().links, 1);
                assert!(leaf.recheck_information.is_some());
            }
            let charged = original.captured_bytes();
            assert_eq!(layout.prepare_namespace_once(), Observation::Refused(Refusal::Closed));
            assert_eq!(original.captured_bytes(), charged);
            drop(host); assert_eq!(original.captured_bytes(), charged);

            let directory = base.join(".seat-host");
            std::fs::write(directory.join("host.sqlite"), b"existing fixture bytes").unwrap();
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_layout(&original, &base, &[]).unwrap(), Observation::Observed);
            assert_eq!(host.prepare_observed_namespace().unwrap(), Observation::Observed);
            let layout = host.namespace_layouts.as_deref().unwrap();
            assert!(matches!(layout.directory_creation.as_ref(), Some(Err(error)) if error.kind() == io::ErrorKind::AlreadyExists));
            for leaf in [layout.lock_leaf.as_ref().unwrap(), layout.database_leaf.as_ref().unwrap()] {
                assert_eq!(leaf.leaf_created, Some(false));
                assert!(matches!(leaf.leaf_probe.as_ref(), Some(Ok(_))));
                assert!(leaf.retained_file().is_some());
            }
            drop(host);
            assert_eq!(std::fs::read(directory.join("host.sqlite")).unwrap(), b"existing fixture bytes");
            std::fs::remove_file(directory.join("host.sqlite")).unwrap();
            std::fs::remove_file(directory.join("host.lock")).unwrap();
            std::fs::remove_dir(directory).unwrap(); std::fs::remove_dir(base).unwrap();
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn namespace_competing_retained_lock_refuses_before_database_root_or_open() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let first = operation();
            let mut owner = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(owner.observe_existing_namespace_layout(&first, &base, &[]).unwrap(), Observation::Observed);
            assert_eq!(owner.prepare_observed_namespace().unwrap(), Observation::Observed);
            let second = operation();
            let mut refused = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(refused.observe_existing_namespace_layout(&second, &base, &[]).unwrap(), Observation::Observed);
            assert_eq!(refused.prepare_observed_namespace().unwrap(), Observation::Refused(Refusal::Lock));
            let layout = refused.namespace_layouts.as_deref_mut().unwrap();
            assert!(layout.database_leaf.is_none());
            assert!(layout.lock_leaf.as_ref().unwrap().retained_file().is_some());
            assert!(matches!(layout.lock_result.as_ref(), Some(Err(_))));
            let cause = std::ptr::from_ref(layout.lock_result.as_ref().unwrap()) as usize;
            let charged = second.captured_bytes();
            assert_eq!(layout.prepare_namespace_once(), Observation::Refused(Refusal::Closed));
            assert_eq!(std::ptr::from_ref(layout.lock_result.as_ref().unwrap()) as usize, cause);
            assert_eq!(second.captured_bytes(), charged);
            drop(refused); drop(owner);
            let directory = base.join(".seat-host");
            std::fs::remove_file(directory.join("host.sqlite")).unwrap();
            std::fs::remove_file(directory.join("host.lock")).unwrap();
            std::fs::remove_dir(directory).unwrap(); std::fs::remove_dir(base).unwrap();
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn namespace_wrong_directory_and_insufficient_child_credit_keep_original_failures() {
        tokio::runtime::Handle::current().spawn(async {
            let (base, _) = paths(); std::fs::create_dir(&base).unwrap();
            let occupied = base.join(".seat-host"); std::fs::write(&occupied, b"not a directory").unwrap();
            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_layout(&original, &base, &[]).unwrap(), Observation::Observed);
            assert_eq!(host.prepare_observed_namespace().unwrap(), Observation::Refused(Refusal::WrongKind));
            let layout = host.namespace_layouts.as_deref().unwrap();
            assert!(layout.directory_creation.is_some());
            assert!(layout.namespace.as_ref().unwrap().guards.as_ref().unwrap().retained_file().is_some());
            assert!(layout.lock_leaf.is_none()); assert!(layout.database_leaf.is_none());
            drop(host); assert_eq!(std::fs::read(&occupied).unwrap(), b"not a directory");
            std::fs::remove_file(&occupied).unwrap();

            let original = operation();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_layout(&original, &base, &[]).unwrap(), Observation::Observed);
            original.charge(crate::operation::OPERATION_BYTES - original.captured_bytes()).unwrap();
            let charged = original.captured_bytes();
            assert_eq!(host.prepare_observed_namespace().unwrap(), Observation::Refused(Refusal::Closed));
            let layout = host.namespace_layouts.as_deref_mut().unwrap();
            assert_eq!(layout.credit_refusal, Some(OperationError::CaptureOverflow));
            assert!(layout.creation_attempted); assert!(layout.native_directory.is_none());
            assert!(layout.directory_creation.is_none()); assert!(layout.namespace.is_none());
            assert!(layout.lock_leaf.is_none()); assert!(layout.database_leaf.is_none());
            assert_eq!(layout.prepare_namespace_once(), Observation::Refused(Refusal::Closed));
            assert_eq!(original.captured_bytes(), charged);
            drop(host); assert!(!occupied.exists()); std::fs::remove_dir(base).unwrap();
        }).await.unwrap();
    }
}
