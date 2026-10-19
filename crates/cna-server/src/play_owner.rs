//! Source-staged approved Play custody wrapper; no production admission/activation.
//! Constructors require the already-retained original Host and its complete
//! admission/OPEN prerequisites; this module does not manufacture those proofs.

use crate::http::{App, owned::{OwnedAppOwner, service::OriginalService}};
use crate::owned_ingress::{CreationOwner, IngressRefusal};
use crate::http::CreateRequest;
use cna_seats::{seat_host::{SeatHost, SeatHostLease}, failed_evidence::CampaignArchive};
use std::{future::Future, path::PathBuf, pin::Pin};
use tokio::{runtime::Handle, time::Instant};

#[path = "play_owner_capacity.rs"]
mod capacity;
// R is derived under m80; CREATION_COUNT still awaits the actual Host N.
// No instantiation/frame safety or numeric count fallback is supplied.
use capacity::{CREATION_COUNT, WRITER_ROW_BYTES};
type FixedOwner = OwnedAppOwner<{ CREATION_COUNT }, { WRITER_ROW_BYTES }>;
type FixedCreation = CreationOwner<{ CREATION_COUNT }, { WRITER_ROW_BYTES }>;


/// Owned by Play's outer Pending before any initialization effects. Not Clone.
pub struct Pending {
    owner: FixedOwner,
}

/// The whole initialized payload and SAME cleanup owner move together.
/// T may itself be Play's retained Result/partial-root state; it is not a receipt.
pub struct Ready<T> {
    payload: T,
    pending: Pending,
}

/// A scoped mutable borrow, never sole original custody and never Clone.
pub struct Cleanup<'a> {
    owner: &'a mut FixedOwner,
}

/// Opaque attempt index, not a member/GO/custody capability. Only reservation
/// mints it; every use checks the original owner's retained attempt identity.
pub struct Attempt {
    key: crate::owned_ingress::CreatorKey,
    create_new: bool,
}

/// Holds the same original owner, a borrow of the ORIGINAL pinned serving
/// future, and its actual full return slot. This does not own a new task.
pub struct Service<'a, T, S: Future> {
    payload: T,
    original: OriginalService<'a, S, { CREATION_COUNT }, { WRITER_ROW_BYTES }>,
}

/// Coarse ingress tags only. Actual close/drive failures remain in the original
/// owner. These tags cannot prove Host join, writer settlement or CLEAN.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Closed,
    Full,
    ForeignAttempt,
    MissingAttempt,
    Poisoned,
    ShutdownNotRequested,
    NoStopRoot,
    AlreadyMinted,
    MissingCustody,
    MissingConnectionProvenance,
    PreparedUnavailable,
    MissingAttemptPath,
    Host(std::io::ErrorKind),
}

impl From<phase::PhaseRefusal> for Refusal {
    fn from(original: phase::PhaseRefusal) -> Self {
        match original {
            phase::PhaseRefusal::Closed => Self::Closed,
            phase::PhaseRefusal::AttemptUnavailable => Self::MissingAttempt,
            phase::PhaseRefusal::AlreadyMinted => Self::AlreadyMinted,
            phase::PhaseRefusal::MissingCustody => Self::MissingCustody,
            phase::PhaseRefusal::MissingConnectionProvenance => Self::MissingConnectionProvenance,
            phase::PhaseRefusal::PreparedUnavailable => Self::PreparedUnavailable,
            phase::PhaseRefusal::MissingAttemptPath => Self::MissingAttemptPath,
            phase::PhaseRefusal::Host(kind) => Self::Host(kind),
        }
    }
}

impl From<IngressRefusal> for Refusal {
    fn from(original: IngressRefusal) -> Self {
        match original {
            IngressRefusal::Closed => Self::Closed,
            IngressRefusal::Full => Self::Full,
            IngressRefusal::ForeignAttempt => Self::ForeignAttempt,
            IngressRefusal::MissingAttempt => Self::MissingAttempt,
            IngressRefusal::Poisoned => Self::Poisoned,
            IngressRefusal::ShutdownNotRequested => Self::ShutdownNotRequested,
            IngressRefusal::NoStopRoot => Self::NoStopRoot,
            // Coarse existing public tag; original typed title refusal/input remain private.
            IngressRefusal::Title(_) => Self::PreparedUnavailable,
        }
    }
}

impl Pending {
    /// No runtime discovery. Caller has already retained the actual process
    /// Host/admitted scope; this supporting constructor is NOT its producer.
    /// Fixed private capacity definitions are still owed; no live caller exists.
    pub fn new_prepaid(
        runtime: Handle, incarnation: uuid::Uuid,
        directory: PathBuf, data: PathBuf, port: u16,
    ) -> Self {
        let creation = FixedCreation::new_prepaid(runtime, incarnation, directory, data);
        Self { owner: OwnedAppOwner::new_prepaid(creation, port) }
    }

    /// Initializers borrow this; cancellation drops only the borrow/future.
    /// They must use the eventual guarded concrete producers, NOT legacy Demo.
    pub fn cleanup(&mut self) -> Cleanup<'_> { Cleanup { owner: &mut self.owner } }

    pub fn observer(&self) -> App { self.owner.observer() }

    /// Infallible synchronous whole move, with no await or partial take.
    /// T is an owned payload; a borrow of this Pending cannot escape into T.
    pub fn ready<T>(self, payload: T) -> Ready<T> {
        Ready { payload, pending: self }
    }
}

impl<T> Ready<T> {
    pub fn payload(&self) -> &T { &self.payload }
    pub fn payload_mut(&mut self) -> &mut T { &mut self.payload }
    pub fn cleanup(&mut self) -> Cleanup<'_> { self.pending.cleanup() }
    pub fn observer(&self) -> App { self.pending.observer() }

    /// Main pins its serving future in a separate retained original frame
    /// BEFORE this call; neither the future nor payload is cloned or spawned.
    pub fn bind_service<'a, S: Future>(self, serving: Pin<&'a mut S>)
        -> Service<'a, T, S>
    {
        let Self { payload, pending } = self;
        Service { payload, original: OriginalService::bind_prepaid(pending.owner, serving) }
    }
}

impl Cleanup<'_> {
    pub fn observer(&self) -> App { self.owner.observer() }

    /// Queue only; actual original input is stored before notification/effects.
    /// On refusal caller retains the returned original request in outer P.
    pub fn reserve_fresh(&mut self, request: CreateRequest)
        -> Result<Attempt, (Refusal, CreateRequest)>
    {
        use crate::owned_ingress::CreationInput;
        match self.owner.original_creation().observer().submit(CreationInput::Fresh(request)) {
            Ok(key) => Ok(Attempt { key, create_new: true }),
            Err((tag, CreationInput::Fresh(original))) => Err((tag.into(), original)),
            Err(_) => unreachable!("observer returns the same rejected input variant"),
        }
    }

    /// Trusted launcher recovery only; no HTTP arbitrary-path producer.
    pub fn reserve_recovery(&mut self, path: PathBuf)
        -> Result<Attempt, (Refusal, PathBuf)>
    {
        self.owner.original_creation().reserve_guarded_recovery(path)
            .map(|key| Attempt { key, create_new: false })
            .map_err(|(tag, original)| (tag.into(), original))
    }

    /// PROPOSED: await ONLY this original guarded creator's completed
    /// preparation, synchronously mint once, then await the SAME creator's
    /// retained return/parked writer. Pairing remains a synchronous next cut.
    /// Ok grants no membership, provenance acceptance or GO; pairing remains required.
    /// The mutable Host borrow is held by this outer future across the wait;
    /// it is never sent to a worker. Canceling releases borrows, not custody.
    /// Path comes ONLY from the original record: recovery's original path or
    /// fresh preparation's ONCE generated path. Missing path refuses without
    /// fallback. The guarded producer must use ONLY custody.protected_path.
    /// No current unguarded dispatch is exported by this supporting API.
    pub async fn prepare_mint_and_park(&mut self, attempt: &Attempt, host: &mut SeatHost,
        lease: &SeatHostLease,
    ) -> Result<(), Refusal> {
        phase::prepare_mint_and_park(self.owner, attempt.key, host, lease, attempt.create_new)
            .await.map_err(Refusal::from)
    }

    pub fn pair_before_go(&mut self, attempt: &Attempt, host: &mut SeatHost,
        lease: &SeatHostLease, archive: &CampaignArchive,
    ) -> Result<(), Refusal> {
        phase::pair_before_go(self.owner, attempt.key, host, lease, archive).map_err(Refusal::from)
    }

    /// Request only. The original OnceLock stores result/clock before waking.
    pub fn request_shutdown(&self) -> Result<(), Refusal> {
        self.owner.request_shutdown().map(|_| ()).map_err(Refusal::from)
    }

    pub fn close_started_at(&mut self) -> Option<&Instant> {
        self.owner.original_creation().close_started_at()
    }

    /// Projection of the held original; does not replace or consume it.
    pub fn close_result(&mut self) -> Option<Result<(), Refusal>> {
        self.owner.original_creation().close_result()
            .map(|original| original.map_err(Refusal::from))
    }

    /// Only AFTER main has retained/completed the actual required Host joins.
    /// Cutoff must be the SAME original process cutoff; not a fresh duration.
    /// This is a borrowed phase, no success/absence/CLEAN bit and no Host token
    /// invented here. Canceling it leaves all actual roots in the owner.
    pub async fn finalize_writers_after_host_until(&mut self, cutoff: Instant) {
        self.owner.original_creation().finalize_writers_after_host_until(cutoff).await;
    }

    /// Neutral diagnostic only. Full raw row/creator/writer correspondence
    /// remains in Server's private owner for the eventual report builder.
    pub fn finalization_refusal(&mut self) -> Option<Refusal> {
        self.owner.original_creation().finalization_refusal().copied().map(Refusal::from)
    }
}

impl<T, S: Future> Service<'_, T, S> {
    pub fn payload(&self) -> &T { &self.payload }
    pub fn payload_mut(&mut self) -> &mut T { &mut self.payload }

    pub fn cleanup(&mut self) -> Cleanup<'_> {
        Cleanup { owner: self.original.owner_mut() }
    }

    /// Poll serving only: the old private dispatch loop is deliberately NOT
    /// exported before guarded factory phases exist. Canceling this borrow
    /// drops neither pinned serving future nor owner/raw output slots.
    pub async fn drive(&mut self) { self.original.drive_serving_only().await; }

    /// After close, continue polling the SAME future within the SAME cutoff.
    /// A timeout stays held even if that future later returns. No service join
    /// or Axum-internal-task proof is inferred from close or this method return.
    pub async fn settle_serving_until(&mut self, cutoff: Instant) {
        self.original.settle_serving_until(cutoff).await;
    }

    pub fn serving_timeout(&self) -> Option<&tokio::time::error::Elapsed> {
        self.original.serving_timeout()
    }

    pub fn returned(&self) -> Option<&S::Output> { self.original.observe().returned }

    pub fn ingress_refusal(&self) -> Option<Refusal> {
        self.original.observe().ingress_failure.copied().map(Refusal::from)
    }

    pub fn close_started_at(&self) -> Option<&Instant> {
        self.original.observe().close_started_at
    }
}

#[path = "play_phase.rs"]
mod phase;
