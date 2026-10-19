//! Trusted launcher archive planning. Never supplied to a seat, MCP tool or spectator.
//!
//! A plan is neither joined supervision nor accepted storage. Its paths and workspace are
//! admitted at ORIGINAL Operation creation; filesystem containment and publication still
//! require the original driver's later all-join, same-instance custody path.

use std::{
    fmt,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

use cna_core::ids::SeatId;

use crate::operation::{Allowance, OPERATION_BYTES, Operation, OperationError, PREPARE_BYTES};

#[path = "archive_digest.rs"]
mod digest;

#[path = "archive_encoding.rs"]
mod encoding;

#[path = "archive_publication.rs"]
mod publication;

#[path = "archive_shape.rs"]
mod shape;

#[cfg(test)]
pub(crate) use shape::MAX_ARCHIVE_JSON_CONTAINERS;
pub(crate) use shape::{check_entry_shape, check_value_shape, shape_workspace_bytes};

pub(crate) use encoding::{
    ArchiveEncoder, EvidenceCategory, EvidenceCoverageBuilder, EvidenceRecord,
};
pub(crate) use publication::{
    ArchiveFileObservation, JoinedArchiveSource, check_retained_publication_workspace,
    publish_joined_archive, verify_existing_joined_archive,
};

// Fixed scratch includes the later encoder/hash/verification state and bounded errors.
// The chunk payload is separately reserved by Operation; no second capture pool exists.
pub(crate) const FIXED_ARCHIVE_WORKSPACE: usize = 16 * 1024;
// Lead m3acf93c47dd1: POLICY CONSTANT, not a derived/measured stack bound.
// check_value_shape limits nesting to 128 containers: about 16 KiB per level.
// The lead accepts this margin for serde_json, including normal debug builds.
// Builder must use EXACTLY this stack size; charge the full Windows reservation.
pub(crate) const ARCHIVE_WORKER_STACK_BYTES: usize = 2 * 1024 * 1024;
// Lead-accepted residual Windows-x64 allowance, NOT a proven/measured bound:
// spawn handle/name/result packet/boxed closure beyond counted inline headers;
// OS TEB/kernel stack/TLS-FLS/CRT/DLL thread-attach data; Rust TLS; allocator slack.
// Panic payload/hook output/backtraces and OS behavior under memory pressure are
// explicitly outside the meter by the same ruling. Panic remains failed work.
pub(crate) const ARCHIVE_WORKER_OVERHEAD_BYTES: usize = 256 * 1024;
const ERROR_BYTES_PER_STD_CALL: usize = 256;
const MAX_STD_CALLS: usize = 2048;
const MAX_EXCLUDED_ROOTS: usize = 32;
// canonicalize may expand a short-name spelling into a long Windows path. Count
// its separate returned-path/library workspace before creation, never after overflow.
const RESOLVED_PATH_RESERVE: usize = 4 * (32768 + 16);
// Publication checks every boundary and the SAME-attempt lost-ACK final again.
// Include individual file/parent checks as well as full containment passes. This
// is creation-time reserve within the unchanged 16 MiB cap, never a refill.
const CONTAINMENT_PASSES: usize = 8;
const MAX_RESOLVED_CALLS: usize = (MAX_EXCLUDED_ROOTS + 6) * CONTAINMENT_PASSES + 8;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("failed evidence archive identity is invalid")]
    Identity,
    #[error("failed evidence archive path is invalid or overlaps a protected root")]
    Path,
    #[error("failed evidence archive creation reserve does not fit")]
    Fit,
    #[error("failed evidence archive filesystem verification failed")]
    Filesystem,
    #[error("failed evidence archive bytes do not match the original attempt")]
    Mismatch,
    #[error(transparent)]
    Operation(#[from] OperationError),
}

/// Required trusted launcher inputs. This is in-process configuration, not a seat option.
/// The launcher supplies EVERY configured sandbox/run/isolation root, including roots for
/// other seats. Root names and common ancestors alone never establish safe containment.
pub struct CampaignArchive {
    database: PathBuf,
    sandbox_root: PathBuf,
    run_root: PathBuf,
    isolation_roots: Vec<PathBuf>,
}

/// Private representation for the m31/m1e7 COMPLETE creation proof. There is
/// intentionally NO constructor/producer yet: partial known layouts and the
/// generic owner descriptor cannot mint this value. Positive original creation
/// stays closed until the whole actual owner/factory/outer/library fit exists.
/// No caller-declared byte count or custom numeric Layout is accepted.
#[allow(dead_code)]
pub(crate) struct OriginalCreationFit {
    parts: OriginalCreationParts,
}

/// Owned pending inputs plus the actual aggregate layout. This is an output
/// projection only; Operation accepts the sealed Fit, NEVER these raw parts.
#[allow(dead_code)]
pub(crate) struct OriginalCreationParts {
    pub(crate) fixed: std::alloc::Layout,
    pub(crate) id: String,
    pub(crate) generation: u64,
    pub(crate) allowance: Allowance,
    pub(crate) correlation: crate::driver::claude_correlation::Correlation,
    pub(crate) archive: ArchivePlan,
    pub(crate) archive_chunk: Vec<u8>,
}

#[allow(dead_code)]
impl OriginalCreationFit {
    pub(crate) fn parts(&self) -> &OriginalCreationParts {
        &self.parts
    }

    pub(crate) fn into_parts(self) -> OriginalCreationParts {
        self.parts
    }
}

impl CampaignArchive {
    pub fn new(
        database: &Path,
        sandbox_root: &Path,
        run_root: &Path,
        isolation_roots: &[PathBuf],
    ) -> Result<Self, ArchiveError> {
        if isolation_roots.is_empty() || isolation_roots.len() > MAX_EXCLUDED_ROOTS {
            return Err(ArchiveError::Path);
        }
        for path in std::iter::once(database)
            .chain(std::iter::once(sandbox_root))
            .chain(std::iter::once(run_root))
            .chain(isolation_roots.iter().map(PathBuf::as_path))
        {
            normal_absolute(path)?;
        }
        if database.extension().and_then(|s| s.to_str()) != Some("sqlite") {
            return Err(ArchiveError::Path);
        }
        uuid::Uuid::parse_str(
            database
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or(ArchiveError::Identity)?,
        )
        .map_err(|_| ArchiveError::Identity)?;
        // Host configuration remains canonical; each original operation also counts the
        // bytes it references below. Shared references do not hide retained path storage.
        Ok(Self {
            database: database.to_path_buf(),
            sandbox_root: sandbox_root.to_path_buf(),
            run_root: run_root.to_path_buf(),
            isolation_roots: isolation_roots.to_vec(),
        })
    }

    pub fn operation(
        &self,
        seat: SeatId,
        epoch: u64,
        operation_id: String,
        allowance: Allowance,
    ) -> Result<Operation, ArchiveError> {
        allowance.check()?;
        let plan = self.plan(seat, epoch, &operation_id, uuid::Uuid::new_v4())?;
        // No capture/start can observe an Operation before its fixed plan and all reserves
        // are accepted. The private constructor rechecks the same absolute allowance.
        Ok(Operation::new_with_archive(
            operation_id,
            epoch,
            allowance,
            plan,
        )?)
    }

    /// Operator-only original-creation entry; legacy `operation` is unchanged.
    /// The opaque SAME concrete factory/caller retains its first actual typed
    /// Result before fixed observation. Full aggregate fit remains incomplete,
    /// so this SOURCE entry currently refuses before ANY Operation allocation.
    #[allow(dead_code)]
    pub(crate) fn original_operation(
        &self,
        caller: &mut crate::driver::claude::OriginalClaudeCaller,
        allowance: Allowance,
    ) -> Result<bool, ArchiveError> {
        caller.attempt_original_creation(self, allowance)
    }

    fn plan(
        &self,
        seat: SeatId,
        epoch: u64,
        operation_id: &str,
        attempt: uuid::Uuid,
    ) -> Result<ArchivePlan, ArchiveError> {
        uuid::Uuid::parse_str(operation_id).map_err(|_| ArchiveError::Identity)?;
        if operation_id.len() != 36 {
            return Err(ArchiveError::Identity);
        }
        let storage = self.database.parent().ok_or(ArchiveError::Path)?;
        let campaign = self
            .database
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or(ArchiveError::Identity)?;
        let mut folder_name = FixedName::new();
        fmt::write(&mut folder_name, format_args!("{campaign}.failed-evidence"))
            .map_err(|_| ArchiveError::Fit)?;
        let mut final_name = FixedName::new();
        fmt::write(
            &mut final_name,
            format_args!("{seat}-e{epoch}-{operation_id}.jsonl"),
        )
        .map_err(|_| ArchiveError::Fit)?;
        let mut temp_name = FixedName::new();
        fmt::write(
            &mut temp_name,
            format_args!("{seat}-e{epoch}-{operation_id}-{attempt}.tmp"),
        )
        .map_err(|_| ArchiveError::Fit)?;
        let path_bytes = storage
            .as_os_str()
            .as_encoded_bytes()
            .len()
            .checked_add(folder_name.len + temp_name.len.max(final_name.len()) + 2)
            .ok_or(ArchiveError::Fit)?;
        let referenced = self
            .paths()
            .try_fold(0usize, |n, p| n.checked_add(p.capacity()))
            .ok_or(ArchiveError::Fit)?;
        let metadata = std::mem::size_of::<ArchivePlan>()
            .checked_add(std::mem::size_of::<CampaignArchive>())
            // Count canonical host bodies AND the per-operation exclusion copies.
            // Referencing shared configuration cannot hide its bytes or Vec headers.
            .and_then(|n| n.checked_add(referenced.checked_mul(2)?))
            .and_then(|n| {
                n.checked_add(
                    self.isolation_roots
                        .capacity()
                        .checked_mul(std::mem::size_of::<PathBuf>())?,
                )
            })
            .and_then(|n| n.checked_add(path_bytes.checked_mul(4)?))
            .and_then(|n| {
                n.checked_add(
                    (self.isolation_roots.len() + 2).checked_mul(std::mem::size_of::<PathBuf>())?,
                )
            })
            .ok_or(ArchiveError::Fit)?;
        if metadata > PREPARE_BYTES {
            return Err(ArchiveError::Fit);
        }
        let ancestor_calls = self
            .paths()
            .try_fold(0usize, |n, p| n.checked_add(p.components().count()))
            .and_then(|n| {
                n.checked_add(
                    storage
                        .components()
                        .count()
                        .checked_add(3)?
                        .checked_mul(4)?,
                )
            })
            .and_then(|n| n.checked_mul(2 * CONTAINMENT_PASSES))
            .and_then(|n| n.checked_add(32))
            .ok_or(ArchiveError::Fit)?;
        if ancestor_calls > MAX_STD_CALLS {
            return Err(ArchiveError::Fit);
        }
        let max_path_bytes = self
            .paths()
            .map(|p| p.as_os_str().as_encoded_bytes().len())
            .max()
            .unwrap_or(0)
            .max(path_bytes);
        let per_call = max_path_bytes
            .checked_add(16)
            .and_then(|n| n.checked_mul(4))
            .and_then(|n| n.checked_add(ERROR_BYTES_PER_STD_CALL))
            .ok_or(ArchiveError::Fit)?;
        let resolved_calls = self
            .isolation_roots
            .len()
            .checked_add(6)
            .and_then(|n| n.checked_mul(CONTAINMENT_PASSES))
            .and_then(|n| n.checked_add(8))
            .filter(|n| *n <= MAX_RESOLVED_CALLS)
            .ok_or(ArchiveError::Fit)?;
        let reserved_bytes = per_call
            .checked_mul(MAX_STD_CALLS)
            .and_then(|n| n.checked_add(RESOLVED_PATH_RESERVE.checked_mul(resolved_calls)?))
            .and_then(|n| n.checked_add(metadata))
            .and_then(|n| n.checked_add(FIXED_ARCHIVE_WORKSPACE))
            // ORIGINAL creation admission, before any capture or worker launch.
            // No late charge after overflow and no second operation allowance.
            .and_then(|n| n.checked_add(ARCHIVE_WORKER_STACK_BYTES))
            .and_then(|n| n.checked_add(ARCHIVE_WORKER_OVERHEAD_BYTES))
            .ok_or(ArchiveError::Fit)?;
        if reserved_bytes >= OPERATION_BYTES {
            return Err(ArchiveError::Fit);
        }
        // All requested path copies/conversions are admitted before these allocations.
        let folder = storage.join(folder_name.as_str()?);
        let final_path = folder.join(final_name.as_str()?);
        let temp = folder.join(temp_name.as_str()?);
        let excluded = std::iter::once(&self.sandbox_root)
            .chain(std::iter::once(&self.run_root))
            .chain(self.isolation_roots.iter())
            .cloned()
            .collect::<Vec<_>>();
        for root in &excluded {
            if lexical_overlap(&folder, root)? {
                return Err(ArchiveError::Path);
            }
        }
        let plan = ArchivePlan {
            storage: storage.to_path_buf(),
            folder,
            temp,
            final_path,
            excluded,
            seat,
            epoch,
            attempt,
            reserved_bytes,
            std_calls: MAX_STD_CALLS,
            max_path_bytes,
            io: Mutex::new(ArchiveIoState {
                remaining: MAX_STD_CALLS,
                resolved_remaining: resolved_calls,
            }),
            publication: Mutex::new(Some(publication::PublicationProgress::new())),
        };
        // Actual capacities cannot silently exceed the preadmitted retained-metadata bound.
        let retained = plan.path_storage_bytes().ok_or(ArchiveError::Fit)?;
        if retained > metadata {
            return Err(ArchiveError::Fit);
        }
        Ok(plan)
    }

    fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        std::iter::once(&self.database)
            .chain(std::iter::once(&self.sandbox_root))
            .chain(std::iter::once(&self.run_root))
            .chain(self.isolation_roots.iter())
    }
}

/// Kept only by the ORIGINAL operation. Fixed filenames are never refreshed on retries.
pub(crate) struct ArchivePlan {
    pub(crate) storage: PathBuf,
    pub(crate) folder: PathBuf,
    pub(crate) temp: PathBuf,
    pub(crate) final_path: PathBuf,
    pub(crate) excluded: Vec<PathBuf>,
    pub(crate) seat: SeatId,
    pub(crate) epoch: u64,
    pub(crate) attempt: uuid::Uuid,
    pub(crate) reserved_bytes: usize,
    pub(crate) std_calls: usize,
    max_path_bytes: usize,
    // This ledger belongs to the original plan. A refused/canceled/repeated check
    // consumes its calls permanently; obtaining another borrow does not refill it.
    io: Mutex<ArchiveIoState>,
    // Fixed original-attempt progress is included in size_of::<ArchivePlan>()
    // BEFORE capture starts. None means an in-service borrow, never a new attempt.
    publication: Mutex<Option<publication::PublicationProgress>>,
}

struct ArchiveIoState {
    remaining: usize,
    resolved_remaining: usize,
}

/// Identity of the complete covered set, calculated from canonical original records.
/// This descriptor is not proof of completeness or actual original supervision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EvidenceCoverage {
    categories: u16,
    components: u64,
    identities: [u8; 32],
}

impl EvidenceCoverage {
    pub(crate) fn new(
        categories: u16,
        components: u64,
        identities: [u8; 32],
    ) -> Result<Self, ArchiveError> {
        // Failed startup, helper output, stdout, stderr, reader diagnostics and
        // unpaired calls. Unknown/raw origins remain in those original records.
        if categories == 0 || categories & !0x3f != 0 || components == 0 {
            return Err(ArchiveError::Identity);
        }
        Ok(Self {
            categories,
            components,
            identities,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ArchiveMeasurement {
    length: u64,
    sha256: [u8; 32],
}

/// Measures deterministic borrowed serialization without retaining a body. A measured
/// stream is not a file write, accepted custody, join witness, or evidence drain.
pub(crate) struct ArchiveMeter {
    length: u64,
    hash: digest::Sha256,
    failed: bool,
}

impl ArchiveMeter {
    pub(crate) fn new() -> Self {
        Self {
            length: 0,
            hash: digest::Sha256::new(),
            failed: false,
        }
    }
    pub(crate) fn finish(self) -> Result<ArchiveMeasurement, ArchiveError> {
        if self.failed {
            return Err(ArchiveError::Fit);
        }
        Ok(ArchiveMeasurement {
            length: self.length,
            sha256: self.hash.finish(),
        })
    }
}

impl Write for ArchiveMeter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let length = u64::try_from(bytes.len())
            .ok()
            .and_then(|count| self.length.checked_add(count));
        if self.failed || length.is_none() || self.hash.update(bytes).is_none() {
            self.failed = true;
            return Err(std::io::Error::from(std::io::ErrorKind::FileTooLarge));
        }
        let length = length.expect("checked archive length");
        self.length = length;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl ArchivePlan {
    fn header(
        &self,
        operation: &Operation,
        coverage: EvidenceCoverage,
    ) -> Result<FixedText<512>, ArchiveError> {
        if !operation
            .archive_plan()
            .is_some_and(|plan| std::ptr::eq(plan, self))
        {
            return Err(ArchiveError::Identity);
        }
        let mut header = FixedText::new();
        // SeatId's display has a fixed side/role alphabet; operation/attempt are UUIDs.
        // Arbitrary raw evidence is encoded separately, never interpolated here.
        fmt::write(&mut header, format_args!(
            "{{\"format\":\"cna-failed-evidence-v1\",\"seat\":\"{}\",\"epoch\":{},\"operation_id\":\"{}\",\"attempt\":\"{}\",\"covered\":{{\"categories\":{},\"components\":{},\"identities_sha256\":\"",
            self.seat, self.epoch, operation.id(), self.attempt, coverage.categories, coverage.components,
        )).map_err(|_| ArchiveError::Fit)?;
        for byte in coverage.identities {
            fmt::write(&mut header, format_args!("{byte:02x}")).map_err(|_| ArchiveError::Fit)?;
        }
        fmt::Write::write_str(&mut header, "\"}}\n").map_err(|_| ArchiveError::Fit)?;
        Ok(header)
    }

    /// Only compares an already-open borrowed stream. No publication, child/readers
    /// join, ownership transfer, deletion, refund or settlement follows this return.
    /// The caller must supply the original file/attempt under actual custody checks.
    pub(crate) fn verify_stream(
        &self,
        operation: &Operation,
        coverage: EvidenceCoverage,
        expected: ArchiveMeasurement,
        reader: &mut impl Read,
    ) -> Result<(), ArchiveError> {
        let header = self.header(operation, coverage)?;
        // Checkout is original-instance checked and available on sticky overflow/expiry;
        // Drop returns the SAME chunk after every short read, error or mismatch.
        let mut chunk = operation.borrow_archive_chunk(operation)?;
        let mut meter = ArchiveMeter::new();
        let mut header_offset = 0;
        loop {
            let bytes = chunk.bytes_mut();
            let count = reader.read(bytes).map_err(|_| ArchiveError::Filesystem)?;
            if count == 0 {
                break;
            }
            if count > bytes.len() {
                return Err(ArchiveError::Filesystem);
            }
            let pending = header.len - header_offset;
            let checked = count.min(pending);
            if bytes[..checked] != header.bytes[header_offset..header_offset + checked] {
                return Err(ArchiveError::Mismatch);
            }
            header_offset += checked;
            meter
                .write_all(&bytes[..count])
                .map_err(|_| ArchiveError::Fit)?;
            if meter.length > expected.length {
                return Err(ArchiveError::Mismatch);
            }
        }
        if header_offset != header.len || meter.finish()? != expected {
            return Err(ArchiveError::Mismatch);
        }
        Ok(())
    }

    fn reserve_std_call(&self, path: &Path, resolved: bool) -> Result<(), ArchiveError> {
        if path.as_os_str().as_encoded_bytes().len() > self.max_path_bytes {
            return Err(ArchiveError::Fit);
        }
        let mut io = self.io.lock().map_err(|_| ArchiveError::Filesystem)?;
        if io.remaining == 0 || (resolved && io.resolved_remaining == 0) {
            return Err(ArchiveError::Fit);
        }
        io.remaining -= 1;
        if resolved {
            io.resolved_remaining -= 1;
        }
        Ok(())
    }

    /// Read-only preflight, NOT a storage ACK, join witness, or lasting race fence.
    /// Revalidate at each later publication boundary. Safe std offers no directory
    /// handle-relative exclusion of concurrent operator filesystem replacement.
    pub(crate) fn verify_containment(&self) -> Result<(), ArchiveError> {
        normal_absolute(&self.storage)?;
        if self.folder.parent() != Some(self.storage.as_path())
            || self.temp.parent() != Some(self.folder.as_path())
            || self.final_path.parent() != Some(self.folder.as_path())
        {
            return Err(ArchiveError::Path);
        }
        // Existing campaign storage is required. Missing exclusion/archive leaves
        // are allowed only after their nearest existing ancestor is verified, with
        // no creation or probing by a seat and no alias accepted as equivalent.
        if !self.verify_ancestors(&self.storage, true)? {
            return Err(ArchiveError::Path);
        }
        for root in &self.excluded {
            if lexical_overlap(&self.folder, root)? {
                return Err(ArchiveError::Path);
            }
            self.verify_ancestors(root, true)?;
        }
        self.verify_ancestors(&self.folder, true)?;
        self.verify_ancestors(&self.temp, false)?;
        self.verify_ancestors(&self.final_path, false)?;
        Ok(())
    }

    fn verify_ancestors(&self, path: &Path, directory: bool) -> Result<bool, ArchiveError> {
        normal_absolute(path)?;
        let mut existing = None;
        let mut leaf_exists = false;
        for (index, ancestor) in path.ancestors().enumerate() {
            self.reserve_std_call(ancestor, false)?;
            match std::fs::symlink_metadata(ancestor) {
                Ok(metadata) => {
                    if forbidden_link(&metadata) || ((index > 0 || directory) && !metadata.is_dir())
                    {
                        return Err(ArchiveError::Path);
                    }
                    leaf_exists |= index == 0;
                    if existing.is_none() {
                        existing = Some(ancestor);
                    }
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound && existing.is_none() => {}
                Err(_) => return Err(ArchiveError::Filesystem),
            }
        }
        let existing = existing.ok_or(ArchiveError::Path)?;
        self.reserve_std_call(existing, true)?;
        let resolved = std::fs::canonicalize(existing).map_err(|_| ArchiveError::Filesystem)?;
        if resolved.capacity() > RESOLVED_PATH_RESERVE
            || !same_resolved_spelling(existing, &resolved)
        {
            return Err(ArchiveError::Path);
        }
        // Check the ancestors again after resolving: a changed symlink/reparse or
        // missing ancestor refuses. This is still an observation, not a no-race ACK.
        for ancestor in existing.ancestors() {
            self.reserve_std_call(ancestor, false)?;
            let metadata =
                std::fs::symlink_metadata(ancestor).map_err(|_| ArchiveError::Filesystem)?;
            if forbidden_link(&metadata) || ((ancestor != path || directory) && !metadata.is_dir())
            {
                return Err(ArchiveError::Path);
            }
        }
        Ok(leaf_exists)
    }

    fn path_storage_bytes(&self) -> Option<usize> {
        [&self.storage, &self.folder, &self.temp, &self.final_path]
            .into_iter()
            .chain(self.excluded.iter())
            .try_fold(
                std::mem::size_of::<Self>().checked_add(
                    self.excluded
                        .capacity()
                        .checked_mul(std::mem::size_of::<PathBuf>())?,
                )?,
                |n, p| n.checked_add(p.capacity()),
            )
    }
}

fn forbidden_link(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        reparse_attributes(metadata.file_attributes())
    }
    #[cfg(not(windows))]
    false
}

#[cfg(any(windows, test))]
fn reparse_attributes(attributes: u32) -> bool {
    attributes & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT, including junctions/mount points
}

fn same_resolved_spelling(original: &Path, resolved: &Path) -> bool {
    let (Some(original), Some(resolved)) = (original.to_str(), resolved.to_str()) else {
        return false;
    };
    #[cfg(windows)]
    let resolved = resolved.strip_prefix(r"\\?\").unwrap_or(resolved);
    if original.len() != resolved.len() {
        return false;
    }
    original.bytes().zip(resolved.bytes()).all(|(left, right)| {
        if cfg!(windows) {
            let normalize = |byte: u8| match byte {
                b'\\' => b'/',
                other => other.to_ascii_lowercase(),
            };
            normalize(left) == normalize(right)
        } else {
            left == right
        }
    })
}

type FixedName = FixedText<160>;

struct FixedText<const N: usize> {
    bytes: [u8; N],
    len: usize,
}
impl<const N: usize> FixedText<N> {
    fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
    fn as_str(&self) -> Result<&str, ArchiveError> {
        std::str::from_utf8(&self.bytes[..self.len]).map_err(|_| ArchiveError::Identity)
    }
}
impl<const N: usize> fmt::Write for FixedText<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self
            .len
            .checked_add(text.len())
            .filter(|n| *n <= self.bytes.len())
            .ok_or(fmt::Error)?;
        self.bytes[self.len..end].copy_from_slice(text.as_bytes());
        self.len = end;
        Ok(())
    }
}

fn normal_absolute(path: &Path) -> Result<(), ArchiveError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return Err(ArchiveError::Path);
    }
    let text = path.to_str().ok_or(ArchiveError::Path)?;
    if text.contains('\0') || (!cfg!(windows) && text.contains(':')) {
        return Err(ArchiveError::Path);
    }
    if text
        .split(['/', '\\'])
        .any(|part| part == "." || part == "..")
    {
        return Err(ArchiveError::Path);
    }
    #[cfg(windows)]
    if text.starts_with(r"\\") || text.chars().filter(|c| *c == ':').count() != 1 {
        // Device/UNC/verbatim aliases are not trusted configuration. canonicalize's
        // internally returned verbatim path is handled by later containment verification.
        return Err(ArchiveError::Path);
    }
    Ok(())
}

fn lexical_overlap(left: &Path, right: &Path) -> Result<bool, ArchiveError> {
    normal_absolute(left)?;
    normal_absolute(right)?;
    // This conservative pre-allocation refusal is NOT resolved/reparse containment proof.
    // The later filesystem stage must verify every ancestor/alias and all excluded roots.
    let left = left.to_str().ok_or(ArchiveError::Path)?;
    let right = right.to_str().ok_or(ArchiveError::Path)?;
    // Borrowed normalization avoids allocating repeated path copies during cleanup.
    let ancestor = |a: &str, b: &str| {
        let mut b = normalized_path(b);
        let mut last = None;
        for ch in normalized_path(a) {
            if b.next() != Some(ch) {
                return false;
            }
            last = Some(ch);
        }
        b.next().is_none_or(|ch| last == Some('/') || ch == '/')
    };
    Ok(ancestor(left, right) || ancestor(right, left))
}

fn normalized_path(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars()
        .flat_map(|ch| if ch == '\\' { '/' } else { ch }.to_lowercase())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn verification_operation() -> Operation {
        let root = std::env::temp_dir().join("synthetic-archive-stream-no-io");
        context(&root)
            .operation(
                "axis.commander".parse().unwrap(),
                7,
                "22222222-2222-4222-8222-222222222222".into(),
                Allowance::required(Some(
                    tokio::time::Instant::now() + std::time::Duration::from_secs(1),
                ))
                .unwrap(),
            )
            .unwrap()
    }
    pub(crate) fn coverage() -> EvidenceCoverage {
        EvidenceCoverage::new(0x3f, 6, [0x5a; 32]).unwrap()
    }
    pub(crate) fn fixture_bytes(operation: &Operation) -> Vec<u8> {
        let header = operation
            .archive_plan()
            .unwrap()
            .header(operation, coverage())
            .unwrap();
        let mut bytes = header.bytes[..header.len].to_vec();
        // Whole body exists ONLY in this synthetic fixture. Production measures and
        // verifies borrowed streams without constructing this Vec.
        for _ in 0..1000 {
            bytes.extend_from_slice(b"{\"raw_hex\":\"00ff\"}\n");
        }
        bytes
    }
    pub(crate) fn measurement(bytes: &[u8]) -> ArchiveMeasurement {
        let mut meter = ArchiveMeter::new();
        meter.write_all(bytes).unwrap();
        meter.finish().unwrap()
    }
    struct ShortRead<'a> {
        bytes: &'a [u8],
        max: usize,
    }
    impl Read for ShortRead<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let count = self.bytes.len().min(output.len()).min(self.max);
            output[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            Ok(count)
        }
    }
    #[test]
    fn same_attempt_header_is_valid_bounded_json_and_instance_bound() {
        let operation = verification_operation();
        let plan = operation.archive_plan().unwrap();
        let header = plan.header(&operation, coverage()).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&header.bytes[..header.len]).unwrap();
        assert_eq!(json["seat"], "axis.commander");
        assert_eq!(json["epoch"], 7);
        assert_eq!(json["operation_id"], operation.id());
        assert_eq!(json["attempt"], plan.attempt.to_string());
        assert_eq!(json["covered"]["categories"], 0x3f);
        assert_eq!(json["covered"]["components"], 6);
        let same_text = verification_operation();
        assert!(matches!(
            plan.header(&same_text, coverage()),
            Err(ArchiveError::Identity)
        ));
        assert!(EvidenceCoverage::new(0, 6, [0; 32]).is_err());
        assert!(EvidenceCoverage::new(0x40, 6, [0; 32]).is_err());
        assert!(EvidenceCoverage::new(1, 0, [0; 32]).is_err());
    }
    #[test]
    fn failed_measurement_cannot_be_finished_or_reopened_as_an_accepted_prefix() {
        let mut meter = ArchiveMeter::new();
        meter.write_all(b"accepted prefix").unwrap();
        meter.length = u64::MAX;
        assert!(meter.write_all(b"refused suffix").is_err());
        assert!(meter.write_all(b"").is_err());
        assert!(matches!(meter.finish(), Err(ArchiveError::Fit)));
    }
    #[test]
    fn short_stream_verification_uses_original_chunk_after_overflow_without_refund() {
        let operation = verification_operation();
        let plan = operation.archive_plan().unwrap();
        // Policy stack/overhead must already reduce ORIGINAL capture capacity;
        // borrowing/verifying after overflow never charges or renews them.
        assert!(
            plan.reserved_bytes
                >= ARCHIVE_WORKER_STACK_BYTES
                    + ARCHIVE_WORKER_OVERHEAD_BYTES
                    + FIXED_ARCHIVE_WORKSPACE
        );
        assert!(
            operation.captured_bytes()
                >= plan.reserved_bytes + crate::operation::ARCHIVE_CHUNK_BYTES
        );
        let bytes = fixture_bytes(&operation);
        let expected = measurement(&bytes);
        let pointer = {
            operation
                .borrow_archive_chunk(&operation)
                .unwrap()
                .bytes_mut()
                .as_ptr()
        };
        let charged = operation.captured_bytes();
        operation.overflow();
        let mut reader = ShortRead {
            bytes: &bytes,
            max: 13,
        };
        plan.verify_stream(&operation, coverage(), expected, &mut reader)
            .unwrap();
        assert_eq!(operation.captured_bytes(), charged);
        assert_eq!(operation.check(), Err(OperationError::CaptureOverflow));
        assert_eq!(
            operation
                .borrow_archive_chunk(&operation)
                .unwrap()
                .bytes_mut()
                .as_ptr(),
            pointer
        );
        // Re-verifying the same lost-ACK bytes creates no second body, attempt or ACK.
        let mut reader = ShortRead {
            bytes: &bytes,
            max: 97,
        };
        plan.verify_stream(&operation, coverage(), expected, &mut reader)
            .unwrap();
        assert_eq!(operation.captured_bytes(), charged);
    }
    #[test]
    fn changed_body_length_digest_coverage_and_instance_refuse_without_consumption() {
        let operation = verification_operation();
        let plan = operation.archive_plan().unwrap();
        let bytes = fixture_bytes(&operation);
        let expected = measurement(&bytes);
        let changed_coverage = EvidenceCoverage::new(0x1f, 6, [0x5a; 32]).unwrap();
        let mut reader = std::io::Cursor::new(&bytes);
        assert!(matches!(
            plan.verify_stream(&operation, changed_coverage, expected, &mut reader),
            Err(ArchiveError::Mismatch)
        ));
        let mut changed = bytes.clone();
        *changed.last_mut().unwrap() = b'x';
        let mut reader = std::io::Cursor::new(&changed);
        assert!(matches!(
            plan.verify_stream(&operation, coverage(), expected, &mut reader),
            Err(ArchiveError::Mismatch)
        ));
        let mut reader = std::io::Cursor::new(&bytes[..bytes.len() - 1]);
        assert!(matches!(
            plan.verify_stream(&operation, coverage(), expected, &mut reader),
            Err(ArchiveError::Mismatch)
        ));
        let mut extra = bytes.clone();
        extra.push(b'x');
        let mut reader = std::io::Cursor::new(&extra);
        assert!(matches!(
            plan.verify_stream(&operation, coverage(), expected, &mut reader),
            Err(ArchiveError::Mismatch)
        ));
        let mut reader = std::io::Cursor::new(&bytes);
        let same_text = verification_operation();
        assert!(matches!(
            plan.verify_stream(&same_text, coverage(), expected, &mut reader),
            Err(ArchiveError::Identity)
        ));
        assert_eq!(reader.position(), 0);
        assert_eq!(bytes, fixture_bytes(&operation));
        assert!(operation.borrow_archive_chunk(&operation).is_ok());
    }
    #[test]
    fn reader_failure_returns_same_chunk_and_fixed_verifier_storage_fits_workspace() {
        struct FailingRead;
        impl Read for FailingRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
            }
        }
        let operation = verification_operation();
        let bytes = fixture_bytes(&operation);
        let pointer = {
            operation
                .borrow_archive_chunk(&operation)
                .unwrap()
                .bytes_mut()
                .as_ptr()
        };
        let charged = operation.captured_bytes();
        assert!(matches!(
            operation.archive_plan().unwrap().verify_stream(
                &operation,
                coverage(),
                measurement(&bytes),
                &mut FailingRead
            ),
            Err(ArchiveError::Filesystem)
        ));
        assert_eq!(
            operation
                .borrow_archive_chunk(&operation)
                .unwrap()
                .bytes_mut()
                .as_ptr(),
            pointer
        );
        assert_eq!(operation.captured_bytes(), charged);
        // This accounts fixed own stack state, not std/allocator/future RSS or an
        // arbitrary reader's captures. Their separate complete fit remains required.
        let fixed = 2 * std::mem::size_of::<FixedText<512>>()
            + 2 * std::mem::size_of::<ArchiveMeter>()
            + 2 * std::mem::size_of::<ArchiveMeasurement>()
            + std::mem::size_of::<EvidenceCoverage>()
            + 64 * std::mem::size_of::<u32>();
        assert!(fixed < FIXED_ARCHIVE_WORKSPACE);
    }
    pub(crate) fn context(root: &Path) -> CampaignArchive {
        CampaignArchive::new(
            &root.join("campaigns/11111111-1111-4111-8111-111111111111.sqlite"),
            &root.join("sandbox"),
            &root.join("run"),
            &[root.join("isolation")],
        )
        .unwrap()
    }
    fn fixture_plan(root: &Path) -> ArchivePlan {
        context(root)
            .plan(
                "axis.commander".parse().unwrap(),
                7,
                "22222222-2222-4222-8222-222222222222",
                uuid::Uuid::nil(),
            )
            .unwrap()
    }

    pub(crate) fn filesystem_fixture() -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("cna-archive-containment-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("campaigns")).unwrap();
        std::fs::create_dir(root.join("sandbox")).unwrap();
        std::fs::create_dir(root.join("run")).unwrap();
        root
    }

    #[test]
    fn containment_checks_existing_storage_without_creating_archive_or_excluded_leaf() {
        let root = filesystem_fixture();
        let plan = fixture_plan(&root);
        let reserve = plan.reserved_bytes;
        let before = plan.io.lock().unwrap().remaining;
        plan.verify_containment().unwrap();
        assert!(plan.io.lock().unwrap().remaining < before);
        assert_eq!(plan.reserved_bytes, reserve);
        assert!(!plan.folder.exists());
        assert!(!root.join("isolation").exists());
        assert!(!plan.temp.exists());
        assert!(!plan.final_path.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_campaign_storage_and_nondirectory_archive_refuse() {
        let root = filesystem_fixture();
        std::fs::remove_dir(root.join("campaigns")).unwrap();
        assert!(matches!(
            fixture_plan(&root).verify_containment(),
            Err(ArchiveError::Path)
        ));
        std::fs::create_dir(root.join("campaigns")).unwrap();
        let plan = fixture_plan(&root);
        std::fs::write(&plan.folder, b"not a directory").unwrap();
        assert!(matches!(plan.verify_containment(), Err(ArchiveError::Path)));
        assert!(!plan.temp.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn original_io_reserve_is_consumed_before_call_and_not_renewed_by_refusal() {
        let root = std::env::temp_dir().join("synthetic-archive-call-budget-no-io");
        let operation = context(&root)
            .operation(
                "axis.commander".parse().unwrap(),
                7,
                "22222222-2222-4222-8222-222222222222".into(),
                Allowance::required(Some(
                    tokio::time::Instant::now() + std::time::Duration::from_secs(1),
                ))
                .unwrap(),
            )
            .unwrap();
        let plan = operation.archive_plan().unwrap();
        let charged = operation.captured_bytes();
        operation.overflow();
        plan.io.lock().unwrap().remaining = 1;
        plan.reserve_std_call(&plan.storage, false).unwrap();
        let retry = operation.clone();
        let retry_plan = retry.archive_plan().unwrap();
        assert!(std::ptr::eq(plan, retry_plan));
        for _ in 0..2 {
            assert!(matches!(
                retry_plan.reserve_std_call(&retry_plan.storage, false),
                Err(ArchiveError::Fit)
            ));
            assert_eq!(retry_plan.io.lock().unwrap().remaining, 0);
            assert_eq!(retry.captured_bytes(), charged);
            assert_eq!(retry.check(), Err(OperationError::CaptureOverflow));
        }
        let plan = fixture_plan(&root);
        plan.io.lock().unwrap().resolved_remaining = 0;
        let before = plan.io.lock().unwrap().remaining;
        assert!(matches!(
            plan.reserve_std_call(&plan.storage, true),
            Err(ArchiveError::Fit)
        ));
        assert_eq!(plan.io.lock().unwrap().remaining, before);
    }

    #[test]
    fn full_containment_workspace_refuses_at_creation_when_original_cap_cannot_fit() {
        let root = std::env::temp_dir().join("synthetic-many-archive-roots-no-io");
        let roots = (0..MAX_EXCLUDED_ROOTS)
            .map(|index| root.join(format!("isolation-{index}")))
            .collect::<Vec<_>>();
        let archive = CampaignArchive::new(
            &root.join("campaigns/11111111-1111-4111-8111-111111111111.sqlite"),
            &root.join("sandbox"),
            &root.join("run"),
            &roots,
        )
        .unwrap();
        assert!(matches!(
            archive.operation(
                "axis.commander".parse().unwrap(),
                7,
                "22222222-2222-4222-8222-222222222222".into(),
                Allowance::required(Some(
                    tokio::time::Instant::now() + std::time::Duration::from_secs(1),
                ))
                .unwrap(),
            ),
            Err(ArchiveError::Fit)
        ));
    }

    #[test]
    fn reparse_attributes_and_alias_spelling_fail_closed() {
        assert!(reparse_attributes(0x400));
        assert!(reparse_attributes(0x410));
        assert!(!reparse_attributes(0x10));
        let root = std::env::temp_dir();
        assert!(same_resolved_spelling(&root, &root));
        assert!(!same_resolved_spelling(
            &root.join("short"),
            &root.join("long-name")
        ));
        assert!(!lexical_overlap(&root.join("sandbox-other"), &root.join("sandbox")).unwrap());
        assert!(lexical_overlap(&root.join("sandbox/archive"), &root.join("sandbox")).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_archive_and_exclusion_ancestors_refuse_without_publication() {
        let root = filesystem_fixture();
        let plan = fixture_plan(&root);
        std::os::unix::fs::symlink(root.join("sandbox"), &plan.folder).unwrap();
        assert!(matches!(plan.verify_containment(), Err(ArchiveError::Path)));
        assert!(!plan.temp.exists());
        std::fs::remove_file(&plan.folder).unwrap();
        let plan = fixture_plan(&root);
        std::os::unix::fs::symlink(&plan.storage, root.join("isolation")).unwrap();
        assert!(matches!(plan.verify_containment(), Err(ArchiveError::Path)));
        assert!(!plan.folder.exists());
        std::fs::remove_file(root.join("isolation")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn campaign_paths_are_fixed_before_operation_acceptance_and_retry_keeps_attempt() {
        let root = std::env::temp_dir().join("synthetic-archive-plan-no-io");
        let archive = context(&root);
        let allowance = Allowance::required(Some(
            tokio::time::Instant::now() + std::time::Duration::from_secs(1),
        ))
        .unwrap();
        let op = archive
            .operation(
                "axis.commander".parse().unwrap(),
                7,
                "22222222-2222-4222-8222-222222222222".into(),
                allowance,
            )
            .unwrap();
        let plan = op.archive_plan().unwrap();
        assert_eq!(
            plan.folder,
            root.join("campaigns/11111111-1111-4111-8111-111111111111.failed-evidence")
        );
        assert!(
            plan.final_path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .contains(op.id())
        );
        assert!(
            plan.temp
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .contains(&plan.attempt.to_string())
        );
        assert!(op.captured_bytes() > plan.reserved_bytes);
        let temp = plan.temp.clone();
        let final_path = plan.final_path.clone();
        op.overflow();
        let cloned = op.clone();
        assert_eq!(cloned.archive_plan().unwrap().temp, temp);
        assert_eq!(cloned.archive_plan().unwrap().final_path, final_path);
        assert_eq!(cloned.archive_plan().unwrap().attempt, plan.attempt);
        assert!(op.borrow_archive_chunk(&op).is_ok());
        assert_eq!(op.check(), Err(OperationError::CaptureOverflow));
    }
    #[test]
    fn overlap_and_missing_exclusion_refuse_before_an_archive_operation_exists() {
        let root = std::env::temp_dir().join("synthetic-archive-plan-no-io");
        let database = root.join("11111111-1111-4111-8111-111111111111.sqlite");
        assert!(matches!(
            CampaignArchive::new(&database, &root.join("s"), &root.join("r"), &[]),
            Err(ArchiveError::Path)
        ));
        let archive =
            CampaignArchive::new(&database, &root, &root.join("r"), &[root.join("i")]).unwrap();
        assert!(matches!(
            archive.plan(
                "axis.commander".parse().unwrap(),
                7,
                "22222222-2222-4222-8222-222222222222",
                uuid::Uuid::nil()
            ),
            Err(ArchiveError::Path)
        ));
    }
    #[test]
    fn conversion_budget_overflow_refuses_instead_of_starting_with_unreserved_paths() {
        let root = std::env::temp_dir().join("p".repeat(4096));
        let archive = context(&root);
        assert!(matches!(
            archive.plan(
                "axis.commander".parse().unwrap(),
                7,
                "22222222-2222-4222-8222-222222222222",
                uuid::Uuid::nil()
            ),
            Err(ArchiveError::Fit)
        ));
    }
}
