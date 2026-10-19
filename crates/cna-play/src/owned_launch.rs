//! Actual early Play partial ownership, using the m741 revised Server surface.
//! No legacy initialization, capacity choice, Host fit/lease producer or live
//! main caller. Transport/guarded factory producers and full fit remain owed.

use cna_play::{config::{Command, LaunchConfig, ResumeUpdates}, journal::SessionJournal};
use cna_seats::{failed_evidence::{ArchiveError, CampaignArchive},
    mcp::{McpServer, ToolRouter}, run::DefaultPrompts, seat_host::{SeatHost, SeatHostLease},
    transcript::TranscriptSink};
use cna_server::{actor::CampaignHandle, http::{App, CreateRequest},
    play_owner::{Attempt, Cleanup, Pending, Ready, Refusal, Service}};
use std::{future::Future, io, net::SocketAddr, path::PathBuf, pin::Pin, sync::Arc};
use tokio::net::TcpListener;

/// Original operator input backing. Moving these paths is not namespace proof.
pub(super) struct Paths {
    pub(super) directory: PathBuf,
    pub(super) data: PathBuf,
    pub(super) dist: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Observation {
    CleanupMissing, CleanupAlreadyAttempted, CommandMissing, NoDurableScope,
    FreshRequestMissing, AlreadyOwned, ReservationMissing, MintMissing,
    ArchiveMissing, ArchiveRefused, UnobservedAttempt, Server(Refusal),
}

enum RejectedInput { Fresh(CreateRequest), Recovery(PathBuf) }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose { Fresh, Resume, Replay, Diagnostic }

/// Canonical early state exists before borrowing any initializer. These are
/// REAL result/resource types, not a generic completed-Demo placeholder. Each
/// later producer must put its partial prefix in this owner BEFORE effects.
/// Empty slots do not establish those producers, full F or positive readiness.
pub(super) struct Initialization {
    paths: Paths,
    purpose: Purpose,
    command: Option<Command>,
    fresh_request: Option<CreateRequest>,
    request_install: Option<Result<(), Observation>>,
    configuration: Option<Result<LaunchConfig, String>>,
    resume_updates: Option<ResumeUpdates>,
    rejected: Option<RejectedInput>,
    attempt: Option<Attempt>,
    reservation_started: bool,
    reservation: Option<Result<(), Observation>>,
    mint_started: bool,
    mint: Option<Result<(), Observation>>,
    pair_started: bool,
    pair: Option<Result<(), Observation>>,
    campaign: Option<CampaignHandle>,
    archive: Option<Result<CampaignArchive, ArchiveError>>,
    archive_install: Option<Result<(), Observation>>,
    journal: Option<Result<SessionJournal, String>>,
    router: Option<Arc<ToolRouter>>,
    prompts: Option<DefaultPrompts>,
    transcript: Option<TranscriptSink>,
    mcp: Option<Result<McpServer, io::Error>>,
    listener: Option<Result<TcpListener, io::Error>>,
    listener_address: Option<Result<SocketAddr, io::Error>>,
    observer_app: Option<App>,
    preparation: Option<Result<(), String>>,
}

/// Nonclonable actual outer root. Construct EMPTY cleanup before constructing
/// Server Pending under the eventual actual prepaid Host admission. No take of
/// cleanup is available to a borrowed initializer, and no default capacity.
pub(super) struct PlayPending {
    partial: Initialization,
    cleanup: Option<Pending>,
    cleanup_install: Option<Result<(), Observation>>,
}

/// Both variants retain whole owners. No large Result error, hidden Box or
/// extraction of partial state just to observe the missing cleanup case.
pub(super) enum EpilogueOwner { MissingCleanup(PlayPending), Retained(Ready<Initialization>) }

impl PlayPending {
    /// Pure whole moves/empty slots; no clone, allocation, query or dispatch.
    pub(super) fn new(command: Command, paths: Paths) -> Self {
        let purpose = match &command { Command::Play(_) => Purpose::Fresh,
            Command::Resume(_) | Command::ResumeControlled { .. } => Purpose::Resume,
            Command::Replay(_) => Purpose::Replay, Command::Help => Purpose::Diagnostic };
        Self { partial: Initialization { paths, purpose, command: Some(command), fresh_request: None, request_install: None,
            configuration: None, resume_updates: None, rejected: None, attempt: None,
            reservation_started: false, reservation: None, mint_started: false, mint: None,
            pair_started: false, pair: None, campaign: None, archive: None, archive_install: None,
            journal: None, router: None, prompts: None, transcript: None, mcp: None,
            listener: None, listener_address: None, observer_app: None, preparation: None },
            cleanup: None, cleanup_install: None }
    }

    /// Caller keeps a rejected original in its incoming slot. No replacement
    /// or retry after the first installation attempt, including missing input.
    pub(super) fn install_cleanup(&mut self, incoming: &mut Option<Pending>) -> Result<(), Observation> {
        if self.cleanup_install.is_some() { return Err(Observation::CleanupAlreadyAttempted); }
        let result = if incoming.is_some() {
            self.cleanup = incoming.take(); Ok(())
        } else { Err(Observation::CleanupMissing) };
        self.cleanup_install = Some(result); result
    }

    pub(super) fn initialization_parts(&mut self) -> Result<(&mut Initialization, Cleanup<'_>), Observation> {
        let cleanup = self.cleanup.as_mut().ok_or(Observation::CleanupMissing)?;
        Ok((&mut self.partial, cleanup.cleanup()))
    }

    /// Retains even FAILED/PARTIAL state through the owner epilogue. Server's
    /// Ready<T> is whole custody, not successful Demo/GO/CLEAN evidence. Missing
    /// cleanup returns the SAME complete original, with no partial extraction.
    pub(super) fn retain_for_epilogue(self) -> EpilogueOwner {
        match self {
            Self { partial, cleanup: Some(cleanup), .. } => EpilogueOwner::Retained(cleanup.ready(partial)),
            original => EpilogueOwner::MissingCleanup(original),
        }
    }
}

impl Initialization {
    /// Original concrete request is prepared once by the eventual PREPAID
    /// initializer. This method does not synthesize a request/path/UUID/config.
    pub(super) fn retain_fresh_request(&mut self, incoming: &mut Option<CreateRequest>) -> Result<(), Observation> {
        if self.request_install.is_some() { return Err(Observation::AlreadyOwned); }
        let result = if self.reservation_started || self.fresh_request.is_some() {
            Err(Observation::AlreadyOwned)
        } else if incoming.is_none() { Err(Observation::FreshRequestMissing) }
        else { self.fresh_request = incoming.take(); Ok(()) };
        self.request_install = Some(result); result
    }

    /// Queue only. Each actual fresh/recovery/Replay request goes through the
    /// SAME Server owner. No filesystem operation or unguarded factory call.
    pub(super) fn reserve_once(&mut self, cleanup: &mut Cleanup<'_>) -> Result<(), Observation> {
        if let Some(result) = self.reservation { return result; }
        if self.reservation_started { return Err(Observation::UnobservedAttempt); }
        self.reservation_started = true; // BEFORE Server can publish/notify.
        let result = self.reserve_input(cleanup);
        self.reservation = Some(result); result
    }

    fn reserve_input(&mut self, cleanup: &mut Cleanup<'_>) -> Result<(), Observation> {
        match self.command.as_ref() {
            None => return Err(Observation::CommandMissing),
            Some(Command::Help) => return Err(Observation::NoDurableScope),
            Some(Command::Play(_)) if self.fresh_request.is_none() => return Err(Observation::FreshRequestMissing),
            _ => (),
        }
        let command = self.command.take().ok_or(Observation::CommandMissing)?;
        let reserved = match command {
            Command::Play(config) => {
                self.configuration = Some(Ok(config));
                let request = self.fresh_request.take().ok_or(Observation::FreshRequestMissing)?;
                cleanup.reserve_fresh(request).map_err(|(tag, original)| {
                    self.rejected = Some(RejectedInput::Fresh(original)); Observation::Server(tag)
                })
            }
            Command::ResumeControlled { path, updates } => {
                self.resume_updates = Some(updates);
                cleanup.reserve_recovery(path).map_err(|(tag, original)| {
                    self.rejected = Some(RejectedInput::Recovery(original)); Observation::Server(tag)
                })
            }
            Command::Resume(path) | Command::Replay(path) =>
                cleanup.reserve_recovery(path).map_err(|(tag, original)| {
                    self.rejected = Some(RejectedInput::Recovery(original)); Observation::Server(tag)
                }),
            Command::Help => return Err(Observation::NoDurableScope),
        };
        // FIRST original opaque attempt MOVE, before success observation.
        self.attempt = Some(reserved?); Ok(())
    }

    pub(super) async fn mint_once(&mut self, cleanup: &mut Cleanup<'_>, host: &mut SeatHost,
        lease: &SeatHostLease) -> Result<(), Observation>
    {
        if let Some(result) = self.mint { return result; }
        if self.mint_started { return Err(Observation::UnobservedAttempt); }
        self.mint_started = true; // BEFORE Host effects; never repair an unwind.
        let result = match (self.reservation, self.attempt.as_ref()) {
            (Some(Ok(())), Some(attempt)) => cleanup.prepare_mint_and_park(attempt, host, lease).await.map_err(Observation::Server),
            _ => Err(Observation::ReservationMissing),
        };
        self.mint = Some(result); result
    }

    /// Whole original result MOVE before observing it. No archive constructor,
    /// path clone or retry; its real producer/full charge is still owed.
    pub(super) fn retain_archive(&mut self, incoming: &mut Option<Result<CampaignArchive, ArchiveError>>)
        -> Result<(), Observation>
    {
        if self.archive_install.is_some() { return Err(Observation::AlreadyOwned); }
        let result = if self.archive.is_some() || self.pair_started { Err(Observation::AlreadyOwned) }
        else if incoming.is_none() { Err(Observation::ArchiveMissing) }
        else {
            self.archive = incoming.take();
            if self.archive.as_ref().is_some_and(Result::is_ok) { Ok(()) } else { Err(Observation::ArchiveRefused) }
        };
        self.archive_install = Some(result); result
    }

    /// Caller keeps real connection/custody/both joins in the SAME Server
    /// owner. Archive is borrowed from this original partial, never rebuilt.
    pub(super) fn pair_once(&mut self, cleanup: &mut Cleanup<'_>, host: &mut SeatHost,
        lease: &SeatHostLease) -> Result<(), Observation>
    {
        if let Some(result) = self.pair { return result; }
        if self.pair_started { return Err(Observation::UnobservedAttempt); }
        self.pair_started = true;
        let result = match (self.mint, self.attempt.as_ref(), self.archive.as_ref()) {
            (Some(Ok(())), Some(attempt), Some(Ok(archive))) =>
                cleanup.pair_before_go(attempt, host, lease, archive).map_err(Observation::Server),
            (_, _, Some(Err(_))) => Err(Observation::ArchiveRefused),
            (_, _, None) => Err(Observation::ArchiveMissing),
            _ => Err(Observation::MintMissing),
        };
        self.pair = Some(result); result
    }
}

/// Actual serving future stays pinned in main's SEPARATE original frame. No
/// self-reference to P, new task, fallback runtime or invented cleanup token.
pub(super) fn bind_original_service<'a, S: Future>(ready: Ready<Initialization>, serving: Pin<&'a mut S>)
    -> Service<'a, Initialization, S>
{
    ready.bind_service(serving)
}

/// PROPOSED concrete borrowed pre-Ready initializer. Its actual original
/// PlayPending stays in main throughout; no completed Demo or cleanup is moved
/// into this future. Host/lease/archive are genuine incoming retained values,
/// not constructed here. An archive absent/refused in P remains a refusal.
pub(super) async fn drive_before_ready(root: &mut PlayPending, host: &mut SeatHost,
    lease: &SeatHostLease) -> Result<(), Observation>
{
    let result = {
        let (partial, mut cleanup) = root.initialization_parts()?;
        match partial.reserve_once(&mut cleanup) {
            Err(original) => Err(original),
            Ok(()) => match partial.mint_once(&mut cleanup, host, lease).await {
                Err(original) => Err(original),
                Ok(()) => partial.pair_once(&mut cleanup, host, lease),
            },
        }
    };
    if result.is_err() {
        // Request CLOSE only. Original close result/clock is held by Server.
        // Actual main still performs Host joins THEN writer finalization with
        // its SAME original cutoff; this function never waits for main.
        if let Ok((_, cleanup)) = root.initialization_parts() { let _ = cleanup.request_shutdown(); }
    }
    result
}

/// Call after a borrowing initializer was canceled. Never resume a started
/// mint/pair with no stored result. P and Server retain every original prefix.
/// This requests CLOSE only; it does not join or consume the outer owner.
pub(super) fn close_cancelled_initialization(root: &mut PlayPending) -> Result<(), Observation> {
    let (_, cleanup) = root.initialization_parts()?;
    cleanup.request_shutdown().map_err(Observation::Server)
}

#[cfg(test)]
mod controls {
    use super::*;
    use std::{future::{poll_fn, Future}, task::Poll};

    fn paths() -> Paths {
        Paths { directory: PathBuf::from("never-open-campaigns"),
            data: PathBuf::from("never-open-data"), dist: PathBuf::from("never-open-dist") }
    }

    fn cleanup_fixture() -> Pending {
        // No public capacity/default/Host lease: actual Server private constants
        // remain unresolved. This partial SOURCE fixture is UNCOMPILED/UNRUN.
        Pending::new_prepaid(tokio::runtime::Handle::current(), uuid::Uuid::from_u128(73),
            PathBuf::from("never-open-campaigns"), PathBuf::from("never-open-data"), 0)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_replay_reservation_keeps_actual_attempt_partial_paths_and_cleanup() {
        tokio::spawn(async {
            let mut root = PlayPending::new(Command::Replay(PathBuf::from("never-open-campaigns/replay.sqlite")), paths());
            let directory = root.partial.paths.directory.as_os_str().as_encoded_bytes().as_ptr() as usize;
            root.install_cleanup(&mut Some(cleanup_fixture())).unwrap();
            let mut initializing = Box::pin(async {
                let (partial, mut cleanup) = root.initialization_parts().unwrap();
                partial.reserve_once(&mut cleanup).unwrap();
                std::future::pending::<()>().await;
            });
            poll_fn(|cx| match initializing.as_mut().poll(cx) {
                Poll::Pending => Poll::Ready(()), Poll::Ready(_) => panic!("fixture initializer stays pending"),
            }).await;
            drop(initializing);
            let attempt = std::ptr::from_ref(root.partial.attempt.as_ref().unwrap()) as usize;
            assert!(root.partial.command.is_none() && root.partial.mint.is_none() && root.partial.campaign.is_none());
            let (partial, mut cleanup) = root.initialization_parts().unwrap();
            partial.reserve_once(&mut cleanup).unwrap();
            assert_eq!(std::ptr::from_ref(partial.attempt.as_ref().unwrap()) as usize, attempt);
            let ready = match root.retain_for_epilogue() { EpilogueOwner::Retained(ready) => ready,
                EpilogueOwner::MissingCleanup(_) => panic!("same cleanup remains installed") };
            assert_eq!(ready.payload().reservation, Some(Ok(())));
            assert_eq!(ready.payload().purpose, Purpose::Replay);
            assert_eq!(ready.payload().paths.directory.as_os_str().as_encoded_bytes().as_ptr() as usize, directory);
            // Queued custody only: no File/SQLite/creator dispatch, Host scope,
            // production admission, completed Replay, writer join or CLEAN.
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_fresh_request_stays_refused_and_rejected_late_request_keeps_its_backing() {
        tokio::spawn(async {
            let mut root = PlayPending::new(Command::Play(LaunchConfig::default()), paths());
            root.install_cleanup(&mut Some(cleanup_fixture())).unwrap();
            let (partial, mut cleanup) = root.initialization_parts().unwrap();
            assert_eq!(partial.reserve_once(&mut cleanup), Err(Observation::FreshRequestMissing));
            let mut late = Some(CreateRequest { kind: cna_server::http::CampaignKind::Sandbox,
                rules_profile: String::from("sandbox-v1"), seed: [0; 32], title: String::from("unsubmitted fixture"),
                paused: true, controller: String::from("human") });
            let title = late.as_ref().unwrap().title.as_ptr() as usize;
            assert_eq!(partial.retain_fresh_request(&mut late), Err(Observation::AlreadyOwned));
            assert_eq!(late.as_ref().unwrap().title.as_ptr() as usize, title);
            assert_eq!(partial.reserve_once(&mut cleanup), Err(Observation::FreshRequestMissing));
            assert!(partial.command.is_some() && partial.attempt.is_none());
        }).await.unwrap();
    }

    #[test]
    fn failed_archive_result_is_moved_once_and_cannot_be_replaced() {
        let mut root = PlayPending::new(Command::Replay(PathBuf::from("never-open.sqlite")), paths());
        let mut first = Some(Err(ArchiveError::Fit));
        assert_eq!(root.partial.retain_archive(&mut first), Err(Observation::ArchiveRefused));
        assert!(first.is_none() && matches!(root.partial.archive, Some(Err(ArchiveError::Fit))));
        let mut replacement = Some(Err(ArchiveError::Filesystem));
        assert_eq!(root.partial.retain_archive(&mut replacement), Err(Observation::AlreadyOwned));
        assert!(matches!(replacement, Some(Err(ArchiveError::Filesystem))));
        assert!(matches!(root.partial.archive, Some(Err(ArchiveError::Fit))));
        // Original cleanup is absent: whole failure returns, no partial take.
        let returned = match root.retain_for_epilogue() { EpilogueOwner::MissingCleanup(original) => original,
            EpilogueOwner::Retained(_) => panic!("no cleanup was installed") };
        assert!(matches!(returned.partial.archive, Some(Err(ArchiveError::Fit))));
    }
}
