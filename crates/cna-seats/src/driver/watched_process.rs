//! Actively driven watched process supervision; every task has a joined owner.
use super::*;
use crate::{
    request_budget::RequestBudget,
    watched_job::{JobCompletion, JobFailure, WatchedJob},
};
use tokio::{sync::watch, task::JoinHandle};

pub(super) struct SupervisedChild {
    stop: watch::Sender<bool>,
    supervisor: Option<JoinHandle<Result<JobCompletion, JobFailure>>>,
    pumps: Vec<Option<JoinHandle<()>>>,
    result: Option<Result<JobCompletion, String>>,
    budget: RequestBudget,
}

async fn stopping(mut stop: watch::Receiver<bool>) {
    let _ = stop.wait_for(|value| *value).await;
}

impl ChildProc {
    /// Called synchronously after the guarded, contained spawn. The job is moved
    /// into an actively scheduled task even when no model turn is being polled.
    pub(crate) fn from_watched(mut job: WatchedJob, budget: RequestBudget) -> Self {
        let stdin = job.take_stdin();
        let stdout = job.take_stdout();
        let stderr = job.take_stderr();
        let (stop, stopped) = watch::channel(false);
        let supervisor_stop = stopped.clone();
        let supervisor_budget = budget.clone();
        let supervisor = tokio::spawn(async move {
            job.supervise_completion(async move {
                tokio::select! {
                    _ = stopping(supervisor_stop) => {},
                    _ = supervisor_budget.bound(std::future::pending::<()>()) => {},
                }
            })
            .await
        });
        let (tx, lines) = mpsc::channel(16);
        let mut pumps = Vec::new();
        let stdout_stop = stopped.clone();
        pumps.push(Some(tokio::spawn(async move {
            let Some(stdout) = stdout else { return };
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                // Cap each stream-json row before allocation can grow without
                // bound. No raw stderr or parse error includes credential text.
                let mut limited = (&mut reader).take(2 * 1024 * 1024 + 1);
                let read = limited.read_until(b'\n', &mut bytes);
                let length = tokio::select! {
                    _ = stopping(stdout_stop.clone()) => break,
                    result = read => match result { Ok(n) => n, Err(_) => break },
                };
                if length == 0 || length > 2 * 1024 * 1024 {
                    break;
                }
                let Ok(line) = String::from_utf8(bytes) else {
                    break;
                };
                let line = line.trim_end_matches(['\r', '\n']).to_string();
                if !line.is_empty() {
                    tokio::select! {
                        _ = stopping(stdout_stop.clone()) => break,
                        result = tx.send(line) => if result.is_err() { break },
                    }
                }
            }
        })));
        pumps.push(Some(tokio::spawn(async move {
            let Some(mut stderr) = stderr else { return };
            // Drain without retaining raw CLI diagnostics, which can contain
            // headers or login values. Canonical transcript comes from stdout.
            let mut sink = tokio::io::sink();
            tokio::select! {
                _ = stopping(stopped) => {},
                _ = tokio::io::copy(&mut stderr, &mut sink) => {},
            }
        })));
        Self {
            child: None,
            watched: Some(SupervisedChild {
                stop,
                supervisor: Some(supervisor),
                pumps,
                result: None,
                budget,
            }),
            stdin,
            lines,
            stderr_tail: Arc::new(Mutex::new(VecDeque::new())),
        }
    }
}

impl ChildProc {
    pub(super) async fn park_watched(&mut self, grace: Duration) -> std::io::Result<()> {
        self.close_stdin();
        let result = {
            let watched = self
                .watched
                .as_mut()
                .ok_or_else(|| std::io::Error::other("missing watched owner"))?;
            let park = watched.park(grace);
            tokio::pin!(park);
            let mut closed = false;
            loop {
                tokio::select! {
                    result = &mut park => break result,
                    line = self.lines.recv(), if !closed => { closed = line.is_none(); }
                }
            }
        };
        if let Err(error) = result {
            if let Some(watched) = &self.watched {
                let _ = watched.budget.stop();
            }
            // Cleanup success cannot turn forced/unverified park into retention.
            let cleanup = self.kill().await;
            return Err(cleanup.err().unwrap_or(error));
        }
        Ok(())
    }
}

impl SupervisedChild {
    pub(super) fn is_running(&self) -> bool {
        self.supervisor
            .as_ref()
            .is_some_and(|task| !task.is_finished())
    }

    async fn park(&mut self, grace: Duration) -> std::io::Result<()> {
        let deadline = Instant::now() + grace;
        if let Some(task) = self.supervisor.as_mut() {
            let completion = tokio::time::timeout_at(deadline, &mut *task)
                .await
                .map_err(|_| std::io::Error::other("natural CLI park timed out"))?
                .map_err(|_| std::io::Error::other("natural CLI supervisor failed"))?
                .map_err(|_| std::io::Error::other("natural CLI job cleanup failed"))?;
            self.result = Some(Ok(completion));
            self.supervisor.take();
        }
        if !matches!(self.result,Some(Ok(JobCompletion::Natural(status))) if status.success()) {
            return Err(std::io::Error::other(
                "forced or unsuccessful CLI exit is not resumable",
            ));
        }
        for slot in &mut self.pumps {
            let Some(pump) = slot.as_mut() else { continue };
            tokio::time::timeout_at(deadline, &mut *pump)
                .await
                .map_err(|_| std::io::Error::other("natural CLI output drain timed out"))?
                .map_err(|_| std::io::Error::other("natural CLI output drain failed"))?;
            slot.take();
        }
        self.pumps.clear();
        Ok(())
    }

    pub(super) async fn stop(&mut self) -> std::io::Result<()> {
        self.stop.send_replace(true);
        if let Some(task) = self.supervisor.as_mut() {
            let result = match tokio::time::timeout(Duration::from_secs(5), &mut *task).await {
                Ok(Ok(Ok(completion))) => Ok(completion),
                Ok(_) => Err("watched job cleanup failed".to_string()),
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    Err("watched job cleanup timed out".to_string())
                }
            };
            self.result = Some(result);
        }
        self.supervisor.take();
        let mut pump_failed = false;
        for slot in &mut self.pumps {
            let Some(pump) = slot.as_mut() else { continue };
            match tokio::time::timeout(Duration::from_secs(2), &mut *pump).await {
                Ok(result) => pump_failed |= result.is_err(),
                Err(_) => {
                    pump.abort();
                    let _ = pump.await;
                    pump_failed = true;
                }
            }
            slot.take();
        }
        self.pumps.clear();
        if pump_failed {
            self.result = Some(Err("watched output cleanup failed".into()));
        }
        if self.result.as_ref().is_some_and(Result::is_err) {
            let _ = self.budget.stop();
        }
        self.result
            .clone()
            .unwrap_or_else(|| Err("missing watched cleanup".into()))
            .map(|_| ())
            .map_err(std::io::Error::other)
    }
}

impl Drop for SupervisedChild {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        if self.supervisor.is_some() || !self.pumps.is_empty() {
            let _ = self.budget.stop();
        }
        // Emergency drop never claims joined. Normal driver stop awaits every
        // task; aborting the supervisor drops the job and latches admission.
        if let Some(task) = &self.supervisor {
            task.abort();
        }
        for pump in self.pumps.iter().flatten() {
            pump.abort();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{request_budget::RequestLimits, watched_job::JobRun};
    use std::{fs, path::PathBuf};

    struct Files(PathBuf);
    impl Files {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("cna-inert-supervisor-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn command(&self, kind: &str) -> Command {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "driver::watched_process::tests::fixture",
                    "--nocapture",
                ])
                .env_clear()
                .env("CNA_INERT_SUPERVISOR", kind)
                .env("CNA_INERT_MARKER", self.0.join("ready"));
            if let Some(root) = std::env::var_os("SYSTEMROOT") {
                command.env("SYSTEMROOT", root);
            }
            command
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0.join("ready"));
            let _ = fs::remove_dir(&self.0);
        }
    }

    #[test]
    fn fixture() {
        let Ok(kind) = std::env::var("CNA_INERT_SUPERVISOR") else {
            return;
        };
        fs::write(
            PathBuf::from(std::env::var_os("CNA_INERT_MARKER").unwrap()),
            b"inert",
        )
        .unwrap();
        if kind == "park" || kind == "nonzero" {
            use std::io::{Read, Write};
            let mut input = Vec::new();
            std::io::stdin().read_to_end(&mut input).unwrap();
            // More than the16-row queue AFTER EOF proves park keeps consuming.
            for _ in 0..40 {
                println!("synthetic-final-frame");
            }
            std::io::stdout().flush().unwrap();
            let marker = PathBuf::from(std::env::var_os("CNA_INERT_MARKER").unwrap());
            fs::write(marker, b"flushed-after-eof").unwrap();
            if kind == "nonzero" {
                std::process::exit(17);
            }
            return;
        }
        if kind == "flood" {
            use std::io::Write;
            let row = "x".repeat(32 * 1024);
            let mut stderr = std::io::stderr().lock();
            stderr.write_all(&vec![b'e'; 1024 * 1024]).unwrap();
            for _ in 0..128 {
                println!("{row}");
            }
        }
        std::thread::sleep(Duration::from_secs(20));
    }

    fn budget(wall: Duration) -> RequestBudget {
        RequestBudget::new(RequestLimits {
            total: 4,
            generations: 2,
            wall,
        })
        .unwrap()
    }

    async fn ready(files: &Files) -> bool {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !files.0.join("ready").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .is_ok()
    }

    #[tokio::test]
    async fn eof_park_keeps_draining_and_requires_natural_success() {
        for kind in ["park", "nonzero"] {
            let files = Files::new();
            let requests = budget(Duration::from_secs(20));
            let jobs = JobRun::new(Duration::from_secs(20)).unwrap();
            let job = jobs.spawn(files.command(kind)).await.unwrap();
            let mut process = ChildProc::from_watched(job, requests.clone());
            let executed = ready(&files).await;
            let parked = process.finish(Duration::from_secs(2)).await;
            let flushed = fs::read(files.0.join("ready")).ok().as_deref()
                == Some(b"flushed-after-eof".as_slice());
            let cleanup = process.kill().await;
            assert!(executed && flushed && cleanup.is_ok());
            assert_eq!(parked.is_ok(), kind == "park");
            assert!(!process.is_running());
        }
    }

    #[tokio::test]
    async fn timed_out_eof_park_forces_join_but_never_reports_resumable() {
        let files = Files::new();
        let requests = budget(Duration::from_secs(20));
        let jobs = JobRun::new(Duration::from_secs(20)).unwrap();
        let job = jobs.spawn(files.command("idle")).await.unwrap();
        let mut process = ChildProc::from_watched(job, requests.clone());
        let executed = ready(&files).await;
        let parked = process.finish(Duration::from_millis(80)).await;
        let cleanup = process.kill().await;
        assert!(executed && parked.is_err() && cleanup.is_ok() && !process.is_running());
        assert!(requests.usage().unwrap().stopped);
    }

    #[tokio::test]
    async fn wall_supervisor_runs_without_turn_polling_and_joins_backpressured_io() {
        let files = Files::new();
        let requests = budget(Duration::from_secs(20));
        let jobs = JobRun::new(Duration::from_secs(3)).unwrap();
        let job = jobs.spawn(files.command("flood")).await.unwrap();
        let mut process = ChildProc::from_watched(job, requests.clone());
        let executed = ready(&files).await;
        // No next_line/run_turn polling: the bounded stdout channel fills.
        let terminated = tokio::time::timeout(Duration::from_secs(5), async {
            while process.is_running() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .is_ok();
        let cleanup = process.kill().await;
        let repeated = process.kill().await;
        assert!(executed && terminated);
        assert!(cleanup.is_ok() && repeated.is_ok());
        assert!(process.watched.as_ref().unwrap().pumps.is_empty());
        assert_eq!(requests.usage().unwrap().total, 0);
    }

    #[tokio::test]
    async fn shared_budget_stop_kills_idle_process_and_joins_without_turns() {
        let files = Files::new();
        let requests = budget(Duration::from_secs(20));
        let jobs = JobRun::new(Duration::from_secs(20)).unwrap();
        let job = jobs.spawn(files.command("idle")).await.unwrap();
        let mut process = ChildProc::from_watched(job, requests.clone());
        let executed = ready(&files).await;
        requests.stop().unwrap();
        let terminated = tokio::time::timeout(Duration::from_secs(3), async {
            while process.is_running() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .is_ok();
        let cleanup = process.kill().await;
        assert!(executed && terminated && cleanup.is_ok());
        assert!(requests.usage().unwrap().stopped);
    }
}
