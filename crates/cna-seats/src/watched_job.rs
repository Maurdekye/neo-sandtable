//! Safe Windows job composition for the watched path. No CLI-specific launch is
//! wired here. User code starts only after suspended outer/inner assignment.
//! Completion-port packets and root exit never stand in for an empty job.

use process_wrap::tokio::{
    ChildWrapper, CommandWrap, CommandWrapper, CreationFlags, JobObject, KillOnDrop,
};
use std::{
    future::Future,
    io,
    os::windows::io::{AsRawHandle, BorrowedHandle},
    panic::{AssertUnwindSafe, catch_unwind},
    process::{ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    sync::{OwnedSemaphorePermit, Semaphore},
};
use win32job::{ExtendedLimitInfo, Job};

const CLEANUP: Duration = Duration::from_secs(2);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum JobFailure {
    #[error("job admission stopped")]
    Stopped,
    #[error("job wall deadline reached")]
    Deadline,
    #[error("job spawn or assignment failed")]
    Spawn,
    #[error("job spawn panicked")]
    Panic,
    #[error("job termination failed")]
    Kill,
    #[error("root cleanup unavailable or timed out")]
    Root,
    #[error("job membership unavailable or saturated")]
    Membership,
    #[error("job did not drain before cleanup deadline")]
    Drain,
}

/// One allocation per run. Unknown cleanup latches admission closed before any
/// capacity is returned; a caller cannot create a third simultaneous root.
#[derive(Clone)]
pub struct JobRun {
    slots: Arc<Semaphore>,
    failed: Arc<AtomicBool>,
    deadline: Instant,
}

impl JobRun {
    pub fn new(wall: Duration) -> Result<Self, JobFailure> {
        let deadline = Instant::now()
            .checked_add(wall)
            .filter(|_| !wall.is_zero())
            .ok_or(JobFailure::Deadline)?;
        Ok(Self {
            slots: Arc::new(Semaphore::new(2)),
            failed: Arc::new(AtomicBool::new(false)),
            deadline,
        })
    }

    pub async fn spawn(&self, command: Command) -> Result<WatchedJob, JobFailure> {
        self.spawn_checked(|| Ok(command)).await
    }

    /// Acquire capacity FIRST, then repeat the caller's identity/layout checks
    /// and construct the command synchronously immediately before assignment.
    /// Never retain a prelaunch check across an asynchronous slot wait.
    pub async fn spawn_checked(
        &self,
        command: impl FnOnce() -> Result<Command, JobFailure>,
    ) -> Result<WatchedJob, JobFailure> {
        if self.failed.load(Ordering::SeqCst) {
            return Err(JobFailure::Stopped);
        }
        let permit = tokio::time::timeout_at(
            tokio::time::Instant::from_std(self.deadline),
            self.slots.clone().acquire_owned(),
        )
        .await
        .map_err(|_| JobFailure::Deadline)?
        .map_err(|_| JobFailure::Stopped)?;
        if self.failed.load(Ordering::SeqCst) {
            return Err(JobFailure::Stopped);
        }
        if Instant::now() >= self.deadline {
            return Err(JobFailure::Deadline);
        }
        let command = command()?;
        if Instant::now() >= self.deadline {
            return Err(JobFailure::Deadline);
        }
        self.spawn_admitted(command, permit, Injection::None)
    }

    fn spawn_admitted(
        &self,
        mut command: Command,
        permit: OwnedSemaphorePermit,
        injection: Injection,
    ) -> Result<WatchedJob, JobFailure> {
        let mut limits = ExtendedLimitInfo::new();
        limits.limit_kill_on_job_close(); // No breakaway flags, no caller policy.
        let outer = Arc::new(Job::create_with_limit_info(&limits).map_err(|_| JobFailure::Spawn)?);
        let reaped = Arc::new(AtomicBool::new(false));
        let spawned = Arc::new(AtomicBool::new(false));
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut flags = CreationFlags(Default::default());
        flags.0.0 = CREATE_NO_WINDOW;
        #[cfg(test)]
        if matches!(injection, Injection::Resume) {
            // Test-only: have the real inner wrapper retain suspension so the
            // subsequent injected resume failure cannot run a child marker.
            flags.0.0 |= 4;
        }
        let mut wrapped = CommandWrap::from(command);
        wrapped.wrap(flags).wrap(KillOnDrop).wrap(AssignOuter {
            job: outer.clone(),
            #[cfg(test)]
            injection,
        });
        #[cfg(test)]
        wrapped.wrap(BeforeInner(injection));
        wrapped.wrap(JobObject);
        #[cfg(test)]
        wrapped.wrap(AfterInner(injection));
        #[cfg(not(test))]
        let _ = injection;
        let proof = reaped.clone();
        // The guard exists before the FIRST fallible wrapper hook. Any error or
        // unwind, even from inner assignment/resume, kills AND reaps the root.
        // This synchronous spawn sequence has no async cancellation point.
        let result = catch_unwind(AssertUnwindSafe(|| {
            wrapped.spawn_with_child(|command| {
                let child = command.spawn()?;
                spawned.store(true, Ordering::SeqCst);
                Ok(Box::new(RootGuard {
                    child,
                    reaped: proof,
                }) as Box<dyn ChildWrapper>)
            })
        }));
        let failure = match result {
            Ok(Ok(child)) => {
                return Ok(WatchedJob {
                    child,
                    outer,
                    permit: Some(permit),
                    failed: self.failed.clone(),
                    deadline: self.deadline,
                    joined: false,
                    #[cfg(test)]
                    query_failure: false,
                    #[cfg(test)]
                    query_stuck: false,
                });
            }
            Ok(Err(_)) => JobFailure::Spawn,
            Err(_) => JobFailure::Panic,
        };
        // Missing spawn means no root; any root that did exist passed through
        // RootGuard. Failure is terminal for this run, including unknown reap.
        self.failed.store(true, Ordering::SeqCst);
        drop(wrapped);
        drop(outer);
        drop(permit);
        if spawned.load(Ordering::SeqCst) && !reaped.load(Ordering::SeqCst) {
            return Err(JobFailure::Root);
        }
        Err(failure)
    }
}

#[derive(Clone, Copy, Debug)]
enum Injection {
    None,
    #[cfg(test)]
    Outer,
    #[cfg(test)]
    MissingHandle,
    #[cfg(test)]
    Nested,
    #[cfg(test)]
    Resume,
    #[cfg(test)]
    Panic,
}

#[derive(Debug)]
struct AssignOuter {
    job: Arc<Job>,
    #[cfg(test)]
    injection: Injection,
}

impl CommandWrapper for AssignOuter {
    fn wrap_child(
        &mut self,
        child: Box<dyn ChildWrapper>,
        _core: &CommandWrap,
    ) -> io::Result<Box<dyn ChildWrapper>> {
        #[cfg(test)]
        {
            if matches!(self.injection, Injection::Panic) {
                panic!("synthetic pre-execution failure");
            }
            if matches!(self.injection, Injection::Outer) {
                return Err(io::Error::other("synthetic outer refusal"));
            }
        }
        let handle = child.process_handle();
        #[cfg(test)]
        let handle = if matches!(self.injection, Injection::MissingHandle) {
            None
        } else {
            handle
        };
        let handle = handle.ok_or_else(|| io::Error::other("missing suspended root handle"))?;
        self.job
            .assign_process(handle.as_raw_handle() as isize)
            .map_err(|_| io::Error::other("outer assignment refused"))?;
        Ok(child)
    }
}

#[derive(Debug)]
#[cfg(test)]
struct BeforeInner(Injection);
#[cfg(test)]
impl CommandWrapper for BeforeInner {
    fn wrap_child(
        &mut self,
        child: Box<dyn ChildWrapper>,
        _core: &CommandWrap,
    ) -> io::Result<Box<dyn ChildWrapper>> {
        #[cfg(test)]
        if matches!(self.0, Injection::Nested) {
            return Err(io::Error::other("synthetic nested assignment refusal"));
        }
        Ok(child)
    }
}

#[derive(Debug)]
#[cfg(test)]
struct AfterInner(Injection);
#[cfg(test)]
impl CommandWrapper for AfterInner {
    fn wrap_child(
        &mut self,
        child: Box<dyn ChildWrapper>,
        _core: &CommandWrap,
    ) -> io::Result<Box<dyn ChildWrapper>> {
        #[cfg(test)]
        if matches!(self.0, Injection::Resume) {
            return Err(io::Error::other("synthetic resume refusal"));
        }
        Ok(child)
    }
}

#[derive(Debug)]
struct RootGuard {
    child: Child,
    reaped: Arc<AtomicBool>,
}

impl ChildWrapper for RootGuard {
    fn inner(&self) -> &dyn ChildWrapper {
        &self.child
    }
    fn inner_mut(&mut self) -> &mut dyn ChildWrapper {
        &mut self.child
    }
    fn into_inner(self: Box<Self>) -> Box<dyn ChildWrapper> {
        // Private guard cannot be removed from its contained process. No code
        // in this module calls into_inner on this or the inner JobObject.
        panic!("contained root cannot be unwrapped")
    }
    fn process_handle(&self) -> Option<BorrowedHandle<'_>> {
        self.child.process_handle()
    }
}

impl Drop for RootGuard {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let deadline = Instant::now() + CLEANUP;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    self.reaped.store(true, Ordering::SeqCst);
                    break;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                _ => break,
            }
        }
    }
}

/// Keep this owned by the driver's wall supervisor and await `supervise` or
/// `stop` before reporting joined. Dropping/cancelling is a terminal run failure.
/// No caller gets the wrapper list, job handles, or into_inner escape hatch.
pub struct WatchedJob {
    child: Box<dyn ChildWrapper>,
    outer: Arc<Job>,
    permit: Option<OwnedSemaphorePermit>,
    failed: Arc<AtomicBool>,
    deadline: Instant,
    joined: bool,
    #[cfg(test)]
    query_failure: bool,
    #[cfg(test)]
    query_stuck: bool,
}

impl WatchedJob {
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin().take()
    }
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout().take()
    }
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr().take()
    }

    fn membership(&self) -> Result<Vec<usize>, JobFailure> {
        #[cfg(test)]
        {
            if self.query_failure {
                return Err(JobFailure::Membership);
            }
            if self.query_stuck {
                return Ok(vec![1]);
            }
        }
        let members = self
            .outer
            .query_process_id_list()
            .map_err(|_| JobFailure::Membership)?;
        if members.len() >= 1024 {
            return Err(JobFailure::Membership);
        }
        Ok(members)
    }

    async fn root_wait(&mut self) -> Result<ExitStatus, JobFailure> {
        // The fixed outermost wrapper is JobObjectChild. Its direct inner is
        // OUR RootGuard, whose default wait delegates to the native root only.
        // Retain the JobObjectChild itself; never use unsafe mutable unwrapping.
        self.child
            .inner_mut()
            .wait()
            .await
            .map_err(|_| JobFailure::Root)
    }

    /// A root-first exit with a live descendant keeps its permit and waits for
    /// explicit stop/wall expiry. Never call JobObjectChild::wait for drain.
    pub async fn supervise(mut self, stop: impl Future<Output = ()>) -> Result<(), JobFailure> {
        tokio::pin!(stop);
        let deadline = tokio::time::Instant::from_std(self.deadline);
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {},
            _ = &mut stop => {},
            result = self.root_wait() => {
                result?;
                if !self.membership()?.is_empty() {
                    tokio::select! { _ = tokio::time::sleep_until(deadline) => {}, _ = &mut stop => {} }
                }
            }
        }
        self.cleanup().await
    }

    pub async fn stop(mut self) -> Result<(), JobFailure> {
        self.cleanup().await
    }

    async fn cleanup(&mut self) -> Result<(), JobFailure> {
        // Must run EVEN when the root has exited: its descendants may be alive.
        self.child.start_kill().map_err(|_| JobFailure::Kill)?;
        let deadline = tokio::time::Instant::now() + CLEANUP;
        tokio::time::timeout_at(deadline, self.root_wait())
            .await
            .map_err(|_| JobFailure::Root)??;
        loop {
            if self.membership()?.is_empty() {
                self.joined = true;
                self.permit.take();
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(JobFailure::Drain);
            }
            tokio::time::sleep_until(
                (tokio::time::Instant::now() + Duration::from_millis(5)).min(deadline),
            )
            .await;
        }
    }
}

impl Drop for WatchedJob {
    fn drop(&mut self) {
        if !self.joined {
            self.failed.store(true, Ordering::SeqCst);
            let _ = self.child.start_kill();
        }
        // RootGuard's bounded synchronous reap also covers cancelled cleanup.
        // Fields close both kill-on-close jobs; no detached waiter is spawned.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::windows::process::CommandExt,
        path::{Path, PathBuf},
    };

    struct Files(PathBuf);
    impl Files {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("cna-inert-job-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            for name in ["marker", "descendant.pid", "leaf-ready"] {
                let _ = fs::remove_file(self.0.join(name));
            }
            let _ = fs::remove_dir(&self.0);
        }
    }

    fn command(kind: &str, directory: &Path) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "watched_job::tests::job_fixture", "--nocapture"])
            .env_clear()
            .env("CNA_INERT_JOB_CASE", kind)
            .env("CNA_INERT_JOB_DIR", directory);
        if let Some(root) = std::env::var_os("SYSTEMROOT") {
            cmd.env("SYSTEMROOT", root);
        }
        cmd
    }

    #[tokio::test]
    async fn queued_spawn_repeats_guard_after_capacity_and_can_refuse_without_execution() {
        use std::sync::atomic::AtomicUsize;
        let files = Files::new();
        let run = JobRun::new(Duration::from_secs(10)).unwrap();
        let first = run.spawn(command("leaf", &files.0)).await.unwrap();
        let second = run.spawn(command("leaf", &files.0)).await.unwrap();
        let checks = Arc::new(AtomicUsize::new(0));
        let seen = checks.clone();
        let waiting = run.spawn_checked(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            Err(JobFailure::Spawn) // changed layout/identity; never construct child
        });
        tokio::pin!(waiting);
        let queued = tokio::time::timeout(Duration::from_millis(40), &mut waiting)
            .await
            .is_err();
        let before = checks.load(Ordering::SeqCst);
        let first_joined = first.stop().await;
        let refusal = waiting.await;
        let second_joined = second.stop().await;
        assert!(queued && before == 0);
        assert!(first_joined.is_ok() && second_joined.is_ok());
        assert!(matches!(refusal, Err(JobFailure::Spawn)));
        assert_eq!(checks.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn job_fixture() {
        let Ok(kind) = std::env::var("CNA_INERT_JOB_CASE") else {
            return;
        };
        let directory = PathBuf::from(std::env::var_os("CNA_INERT_JOB_DIR").unwrap());
        match kind.as_str() {
            "tree" => {
                let mut child = std::process::Command::new(std::env::current_exe().unwrap());
                child
                    .args(["--exact", "watched_job::tests::job_fixture", "--nocapture"])
                    .env_clear()
                    .env("CNA_INERT_JOB_CASE", "leaf")
                    .env("CNA_INERT_JOB_DIR", &directory)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(CREATE_NO_WINDOW);
                if let Some(root) = std::env::var_os("SYSTEMROOT") {
                    child.env("SYSTEMROOT", root);
                }
                let descendant = child.spawn().unwrap();
                fs::write(
                    directory.join("descendant.pid"),
                    descendant.id().to_string(),
                )
                .unwrap();
                let deadline = Instant::now() + Duration::from_secs(2);
                while !directory.join("leaf-ready").exists() {
                    assert!(Instant::now() < deadline, "inert descendant did not start");
                    std::thread::sleep(Duration::from_millis(2));
                }
                // Intentionally leave a living descendant after root exit.
                std::process::exit(0);
            }
            "leaf" => {
                fs::write(directory.join("leaf-ready"), b"inert descendant executing").unwrap();
                std::thread::sleep(Duration::from_secs(10)); // finite orphan guard
            }
            "marker" => {
                fs::write(directory.join("marker"), b"inert root executing").unwrap();
                std::thread::sleep(Duration::from_secs(10));
            }
            _ => panic!("unknown inert helper role"),
        }
    }

    async fn root_exits_with_live_descendant(job: &mut WatchedJob, files: &Files) -> usize {
        let status = tokio::time::timeout(Duration::from_secs(3), job.root_wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success());
        let pid: usize = fs::read_to_string(files.0.join("descendant.pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(job.membership().unwrap().contains(&pid));
        pid
    }

    #[tokio::test]
    async fn root_exit_does_not_drain_or_release_capacity_then_explicit_stop_verifies_empty() {
        let files = Files::new();
        let run = JobRun::new(Duration::from_secs(10)).unwrap();
        let mut job = run.spawn(command("tree", &files.0)).await.unwrap();
        root_exits_with_live_descendant(&mut job, &files).await;
        assert_eq!(run.slots.available_permits(), 1);
        let outer = job.outer.clone();
        job.stop().await.unwrap();
        assert!(outer.query_process_id_list().unwrap().is_empty());
        assert_eq!(run.slots.available_permits(), 2);
        assert!(!run.failed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn wall_kills_live_descendant_after_root_has_exited() {
        let files = Files::new();
        let run = JobRun::new(Duration::from_secs(1)).unwrap();
        let mut job = run.spawn(command("tree", &files.0)).await.unwrap();
        root_exits_with_live_descendant(&mut job, &files).await;
        let outer = job.outer.clone();
        tokio::time::timeout(
            Duration::from_secs(4),
            job.supervise(std::future::pending()),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(outer.query_process_id_list().unwrap().is_empty());
        assert_eq!(run.slots.available_permits(), 2);
    }

    #[tokio::test]
    async fn assignment_missing_handle_nested_resume_and_panic_failures_reap_unexecuted_root() {
        for injection in [
            Injection::Outer,
            Injection::MissingHandle,
            Injection::Nested,
            Injection::Resume,
            Injection::Panic,
        ] {
            let files = Files::new();
            let run = JobRun::new(Duration::from_secs(10)).unwrap();
            let permit = run.slots.clone().acquire_owned().await.unwrap();
            let result = run.spawn_admitted(command("marker", &files.0), permit, injection);
            let expected = if matches!(injection, Injection::Panic) {
                JobFailure::Panic
            } else {
                JobFailure::Spawn
            };
            // These classifications require actual RootGuard reap after spawn;
            // failure to reap would instead produce typed JobFailure::Root.
            assert_eq!(result.err(), Some(expected));
            assert!(!files.0.join("marker").exists());
            assert!(run.failed.load(Ordering::SeqCst));
            assert_eq!(
                run.spawn(command("marker", &files.0)).await.err(),
                Some(JobFailure::Stopped)
            );
        }
    }

    #[tokio::test]
    async fn query_failure_and_cleanup_timeout_never_claim_joined_or_allow_new_admission() {
        for stuck in [false, true] {
            let files = Files::new();
            let run = JobRun::new(Duration::from_secs(10)).unwrap();
            let mut job = run.spawn(command("tree", &files.0)).await.unwrap();
            root_exits_with_live_descendant(&mut job, &files).await;
            job.query_failure = !stuck;
            job.query_stuck = stuck;
            let expected = if stuck {
                JobFailure::Drain
            } else {
                JobFailure::Membership
            };
            assert_eq!(job.stop().await, Err(expected));
            assert!(run.failed.load(Ordering::SeqCst));
            assert_eq!(
                run.spawn(command("marker", &files.0)).await.err(),
                Some(JobFailure::Stopped)
            );
        }
    }

    #[tokio::test]
    async fn supervisor_cancellation_kills_and_reaps_without_detached_waiter() {
        let files = Files::new();
        let run = JobRun::new(Duration::from_secs(10)).unwrap();
        let mut job = run.spawn(command("tree", &files.0)).await.unwrap();
        root_exits_with_live_descendant(&mut job, &files).await;
        let outer = job.outer.clone();
        let task = tokio::spawn(job.supervise(std::future::pending()));
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(run.failed.load(Ordering::SeqCst));
        // Terminal-error drop closes the inner kill-on-close job. This test's
        // retained outer handle observes the actual OS membership reaching zero.
        tokio::time::timeout(Duration::from_secs(2), async {
            while !outer.query_process_id_list().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn two_slots_are_held_until_verified_cleanup() {
        let files = Files::new();
        let run = JobRun::new(Duration::from_secs(10)).unwrap();
        let first = run.spawn(command("marker", &files.0)).await.unwrap();
        let second = run.spawn(command("marker", &files.0)).await.unwrap();
        assert_eq!(run.slots.available_permits(), 0);
        let third = run.spawn(command("marker", &files.0));
        tokio::pin!(third);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut third)
                .await
                .is_err()
        );
        first.stop().await.unwrap();
        let third = third.await.unwrap();
        second.stop().await.unwrap();
        third.stop().await.unwrap();
        assert_eq!(run.slots.available_permits(), 2);
    }
}
