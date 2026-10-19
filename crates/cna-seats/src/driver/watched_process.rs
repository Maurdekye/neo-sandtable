//! Actively driven watched process supervision; every task has a joined owner.
use super::*;
use crate::{
    request_budget::RequestBudget,
    watched_job::{JobFailure, WatchedJob},
};
use tokio::{sync::watch, task::JoinHandle};

pub(super) struct SupervisedChild {
    stop: watch::Sender<bool>,
    supervisor: Option<JoinHandle<Result<(), JobFailure>>>,
    pumps: Vec<Option<JoinHandle<()>>>,
    result: Option<Result<(), String>>,
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
            job.supervise(async move {
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

impl SupervisedChild {
    pub(super) fn is_running(&self) -> bool {
        self.supervisor
            .as_ref()
            .is_some_and(|task| !task.is_finished())
    }

    pub(super) async fn stop(&mut self) -> std::io::Result<()> {
        self.stop.send_replace(true);
        if let Some(task) = self.supervisor.as_mut() {
            let result = match tokio::time::timeout(Duration::from_secs(5), &mut *task).await {
                Ok(Ok(Ok(()))) => Ok(()),
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
