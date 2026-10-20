//! Separate original fixed-pair calibration. No full-turn admission or resume.
use crate::{calibration::MODEL, calibration_cost::CalibrationPrices};
use cna_core::ids::SeatId;
use serde::Deserialize;
use std::{ffi::OsStr, path::PathBuf};

/// Trusted launcher input only. No credential value or operator capability is serialized.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationConfig {
    pub executable: PathBuf,
    pub login_file: PathBuf,
    pub ledger: PathBuf,
    pub wall_seconds: u64,
    pub seat: SeatId,
    pub system_prompt: String,
    pub prompt: String,
    pub prices: CalibrationPrices,
}

pub struct CalibrationIntent {
    _private: (),
}
impl CalibrationIntent {
    pub fn parse(value: Option<&OsStr>) -> Result<Self, &'static str> {
        if value.and_then(OsStr::to_str) == Some("calibration-one") {
            Ok(Self { _private: () })
        } else {
            Err("explicit calibration-only operator intent required")
        }
    }
}

impl CalibrationConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.prices.validate()?;
        if !self.executable.is_absolute()
            || !self.login_file.is_absolute()
            || !self.ledger.is_absolute()
            || self.wall_seconds == 0
            || self.system_prompt.is_empty()
            || self.prompt.is_empty()
            || self.prices.model != MODEL
        {
            return Err("explicit calibration paths, prompt and wall limit required");
        }
        let payload =
            serde_json::json!({"type":"user", "message":{"role":"user","content":self.prompt}})
                .to_string();
        if payload.len() > 2 * 1024 * 1024 {
            return Err("calibration stdin row exceeds bound");
        }
        Ok(())
    }

    pub fn validate_live_launch(&self) -> Result<(), &'static str> {
        self.validate()?;
        if std::env::var("CNA_LIVE_CLI_TESTS").as_deref() != Ok("1") {
            return Err("separate current calibration go and live opt-in required");
        }
        if self.prices.verified_on != chrono::Utc::now().format("%Y-%m-%d").to_string() {
            return Err("on-call-day explicit price verification required");
        }
        Ok(())
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::{
        calibration::CalibrationFence,
        calibration_http::{LaneAFence, LaneAReport},
        calibration_isolation::{
            CALIBRATION_CONFIG, CALIBRATION_CWD, ISOLATION_ROOT, with_checked_calibration_spawn,
        },
        driver::{
            ChildProc, StreamEvent, StreamParser,
            claude::{
                ClaudeConfig, ClaudeParser, calibration_command, check_watched_init,
                verify_executable,
            },
        },
        subscription_login::SubscriptionLogin,
        watched_job::{JobFailure, JobRun},
    };
    use std::{fs, os::windows::fs::MetadataExt, path::Path, time::Duration};
    use tokio::{sync::oneshot, task::JoinHandle};

    pub struct CalibrationRuntime {
        fence: LaneAFence,
        process: Option<ChildProc>,
        service: Option<JoinHandle<std::io::Result<()>>>,
        shutdown: Option<oneshot::Sender<()>>,
        session: String,
        turn_attempted: bool,
        cleanup_failed: bool,
        cleanup_confirmed: bool,
    }

    fn plain_directory(path: &Path) -> Result<(), &'static str> {
        let m =
            fs::symlink_metadata(path).map_err(|_| "calibration directory metadata unavailable")?;
        if !m.is_dir() || m.file_attributes() & 0x400 != 0 {
            return Err("calibration directory is linked or unsupported");
        }
        Ok(())
    }

    fn prepare_pair(ledger: &Path) -> Result<(), &'static str> {
        plain_directory(Path::new("C:/"))?;
        let root = Path::new(ISOLATION_ROOT);
        plain_directory(root)?;
        // Only this exact pair may be created, and only in an empty plain root.
        if fs::read_dir(root)
            .map_err(|_| "calibration root unreadable")?
            .next()
            .is_none()
        {
            fs::create_dir(CALIBRATION_CWD).map_err(|_| "calibration cwd creation failed")?;
            fs::create_dir(CALIBRATION_CONFIG).map_err(|_| "calibration config creation failed")?;
        }
        with_checked_calibration_spawn(|_, _| Ok::<_, ()>(()))
            .map_err(|_| "fixed calibration pair refused")?;
        let parent = ledger.parent().ok_or("private ledger parent required")?;
        for ancestor in parent.ancestors() {
            plain_directory(ancestor)?;
        }
        let canonical = parent
            .canonicalize()
            .map_err(|_| "private ledger parent unavailable")?;
        if canonical.starts_with(
            root.canonicalize()
                .map_err(|_| "isolation root unavailable")?,
        ) {
            return Err("calibration ledger must be outside the seat tree");
        }
        Ok(())
    }

    impl CalibrationRuntime {
        /// Caller supplies an MCP endpoint derived from its own bound SeatId,
        /// never an endpoint or operator capability supplied by the child.
        pub async fn prepare(
            config: &CalibrationConfig,
            _intent: CalibrationIntent,
            own_mcp_url: String,
        ) -> Result<Self, &'static str> {
            config.validate_live_launch()?;
            let endpoint = reqwest::Url::parse(&own_mcp_url)
                .map_err(|_| "own loopback MCP endpoint required")?;
            if endpoint.scheme() != "http"
                || endpoint.host_str() != Some("127.0.0.1")
                || endpoint.port().is_none()
            {
                return Err("own loopback MCP endpoint required");
            }
            verify_executable(&config.executable)
                .map_err(|_| "pinned calibration executable refused")?;
            prepare_pair(&config.ledger)?;
            let wall = Duration::from_secs(config.wall_seconds);
            let jobs = JobRun::new(wall).map_err(|_| "calibration wall unsupported")?;
            CalibrationFence::initialize(&config.ledger)
                .map_err(|_| "fresh private calibration ledger required; never rearm")?;
            let fence = LaneAFence::open(&config.ledger, &config.prices, wall)
                .map_err(|_| "calibration fence refused")?;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|_| "calibration listener failed")?;
            let address = listener
                .local_addr()
                .map_err(|_| "calibration listener address unavailable")?;
            // The sole consumer deletes the owner file before its intended handoff.
            let login = SubscriptionLogin::consume_file(&config.login_file)
                .map_err(|_| "calibration login handoff refused")?;
            let session = uuid::Uuid::new_v4().to_string();
            let cfg = ClaudeConfig {
                seat: config.seat,
                exe: Some(config.executable.clone()),
                model: MODEL.into(),
                config_dir: Some(CALIBRATION_CONFIG.into()),
                expected_email: None,
                sandbox: CALIBRATION_CWD.into(),
                run_dir: config
                    .ledger
                    .parent()
                    .ok_or("ledger parent missing")?
                    .into(),
                mcp_url: own_mcp_url,
                system_prompt: config.system_prompt.clone(),
                effort: None,
                context_window: Some(crate::calibration::ACTUAL_PROMPT_LIMIT),
            };
            let (shutdown, stopped) = oneshot::channel();
            let gateway = fence.clone();
            let service = tokio::spawn(async move {
                gateway
                    .serve(listener, async move {
                        let _ = stopped.await;
                    })
                    .await
            });
            let mut runtime = Self {
                fence,
                process: None,
                service: Some(service),
                shutdown: Some(shutdown),
                session,
                turn_attempted: false,
                cleanup_failed: false,
                cleanup_confirmed: false,
            };
            let job = jobs
                .spawn_checked(|| {
                    verify_executable(&config.executable).map_err(|_| JobFailure::Spawn)?;
                    with_checked_calibration_spawn(|_, _| {
                        let mut command = calibration_command(
                            &config.executable,
                            &cfg,
                            address,
                            &runtime.session,
                        )?;
                        login.apply_to(&mut command);
                        Ok::<_, crate::driver::DriverError>(command)
                    })
                    .map_err(|_| JobFailure::Spawn)
                })
                .await;
            match job {
                Ok(job) => {
                    runtime.process = Some(ChildProc::from_watched(job, runtime.fence.budget()))
                }
                Err(_) => {
                    let _ = runtime.shutdown().await;
                    return Err("contained calibration launch refused");
                }
            }
            Ok(runtime)
        }

        /// Exactly one stdin model window. No resume, recovery, reseed or retry.
        pub async fn turn(&mut self, prompt: &str) -> Result<(), &'static str> {
            if self.turn_attempted {
                return Err("calibration model window already attempted");
            }
            self.turn_attempted = true;
            let budget = self.fence.budget();
            let process = self
                .process
                .as_mut()
                .ok_or("calibration process unavailable")?;
            let payload =
                serde_json::json!({"type":"user","message":{"role":"user","content":prompt}})
                    .to_string();
            if payload.len() > 2 * 1024 * 1024 {
                return Err("calibration stdin row exceeds bound");
            }
            budget
                .bound(process.send_line(&payload))
                .await
                .map_err(|_| "calibration wall reached")?
                .map_err(|_| "calibration stdin failed")?;
            let mut parser = ClaudeParser::default();
            let mut initialized = false;
            loop {
                let line = budget
                    .bound(
                        process.next_line(tokio::time::Instant::from_std(
                            budget
                                .deadline()
                                .map_err(|_| "calibration deadline unavailable")?,
                        )),
                    )
                    .await
                    .map_err(|_| "calibration wall reached")?
                    .map_err(|_| "calibration stdout deadline")?
                    .ok_or("calibration stdout ended without result")?;
                for event in parser.feed(&line) {
                    match event {
                        StreamEvent::Init {
                            session_id,
                            model,
                            cli_version,
                            tools,
                            mcp_connected,
                        } => {
                            check_watched_init(
                                Some(&self.session),
                                &session_id,
                                model.as_deref(),
                                cli_version.as_deref(),
                                &tools,
                                mcp_connected,
                            )
                            .map_err(|_| "calibration init identity or own MCP mismatch")?;
                            initialized = true;
                        }
                        StreamEvent::TurnDone(outcome) => {
                            let report = self
                                .fence
                                .report()
                                .map_err(|_| "calibration report unavailable")?;
                            return if initialized
                                && outcome.ok
                                && report.generation_forwards == 1
                                && report.stream_finished
                                && !report.accounting_unknown
                            {
                                Ok(())
                            } else {
                                Err("calibration model window failed or accounting incomplete")
                            };
                        }
                        _ => {}
                    }
                }
            }
        }

        pub async fn shutdown(&mut self) -> Result<(), &'static str> {
            self.fence.stop();
            if let Some(process) = &mut self.process {
                process.close_stdin();
                self.cleanup_failed |= process.kill().await.is_err();
            }
            self.process = None;
            if let Some(stop) = self.shutdown.take() {
                let _ = stop.send(());
            }
            if let Some(mut service) = self.service.take() {
                match tokio::time::timeout(Duration::from_secs(3), &mut service).await {
                    Ok(Ok(Ok(()))) => {}
                    Ok(_) => self.cleanup_failed = true,
                    Err(_) => {
                        self.cleanup_failed = true;
                        service.abort();
                        let _ = service.await;
                    }
                }
            }
            self.cleanup_failed |= self.fence.shutdown_pumps().await.is_err();
            self.cleanup_confirmed = !self.cleanup_failed;
            if self.cleanup_failed {
                Err("calibration cleanup failed or unconfirmed")
            } else {
                Ok(())
            }
        }

        pub fn report(&self) -> Result<(bool, LaneAReport), &'static str> {
            Ok((
                self.cleanup_confirmed,
                self.fence
                    .report()
                    .map_err(|_| "calibration report unavailable")?,
            ))
        }
    }
}

#[cfg(windows)]
pub use windows::CalibrationRuntime;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fullturn_or_missing_or_ambiguous_intent_never_enables_calibration() {
        for value in [
            None,
            Some("setup-gt1"),
            Some("setup-gt1+same-run-resume"),
            Some("calibration-one "),
            Some(""),
        ] {
            assert!(CalibrationIntent::parse(value.map(OsStr::new)).is_err());
        }
        assert!(CalibrationIntent::parse(Some(OsStr::new("calibration-one"))).is_ok());
    }
}
