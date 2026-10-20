//! Watched run binding. Native admission intentionally remains closed pending
//! integrated review and the owner's current go through the lead.
use super::*;
use crate::watched_state::{ObservedAssistant, ParkedFiles};
use crate::{
    calibration::MODEL,
    calibration_isolation::WatchedNamespace,
    request_budget::RequestBudget,
    subscription_login::SubscriptionLogin,
    watched_job::{JobFailure, JobRun},
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Read, net::SocketAddr, path::Path, sync::Mutex};

pub const WATCHED_VERSION: &str = "2.1.293";
// Exact installed SDK0.3.293 executable whose readers were independently
// inspected. Identity is checked as data; no auth/version subprocess is run.
const EXECUTABLE_SHA256: &str = "8693c4a02dde7441d0066ede68af8ddfc408bb982d77e12b506286268224e6fa";

pub fn native_admission() -> Result<(), DriverError> {
    Err(DriverError::Isolation(
        "watched native admission remains closed pending integration review and owner go".into(),
    ))
}

fn resume_admission() -> Result<(), DriverError> {
    Err(DriverError::Isolation(
        "watched resume remains closed pending file/park review".into(),
    ))
}

pub(super) fn check_init(
    expected_session: Option<&str>,
    actual_session: &str,
    model: Option<&str>,
    version: Option<&str>,
    tools: &[String],
    connected: Option<bool>,
) -> Result<(), DriverError> {
    check_isolation(tools, connected)?;
    if expected_session != Some(actual_session)
        || actual_session.is_empty()
        || model != Some(MODEL)
        || version != Some(WATCHED_VERSION)
        || connected != Some(true)
        || tools.is_empty()
    {
        return Err(DriverError::Isolation(
            "watched init identity mismatch".into(),
        ));
    }
    Ok(())
}

pub struct WatchedLaunch {
    pub namespace: Arc<WatchedNamespace>,
    pub executable: PathBuf,
    pub fence: SocketAddr,
    pub budget: RequestBudget,
    pub jobs: JobRun,
    pub login: Arc<SubscriptionLogin>,
    parked: Mutex<BTreeMap<SeatId, (uuid::Uuid, ParkedFiles)>>,
}

fn verify_executable(path: &Path) -> Result<(), DriverError> {
    let refused = || DriverError::Isolation("watched executable identity mismatch".into());
    if !path.is_absolute() || path.file_name().and_then(|n| n.to_str()) != Some("claude.exe") {
        return Err(refused());
    }
    for (index, ancestor) in path.ancestors().enumerate() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| refused())?;
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0
            || if index == 0 {
                !metadata.is_file()
            } else {
                !metadata.is_dir()
            }
        {
            return Err(refused());
        }
    }
    let file = fs::File::open(path).map_err(|_| refused())?;
    if file.metadata().map_err(|_| refused())?.len() > 512 * 1024 * 1024 {
        return Err(refused());
    }
    let mut hash = Sha256::new();
    let mut reader = file.take(512 * 1024 * 1024 + 1);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buffer).map_err(|_| refused())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    if format!("{:x}", hash.finalize()) != EXECUTABLE_SHA256 {
        return Err(refused());
    }
    Ok(())
}

impl WatchedLaunch {
    pub fn new(
        namespace: Arc<WatchedNamespace>,
        executable: PathBuf,
        fence: SocketAddr,
        budget: RequestBudget,
        jobs: JobRun,
        login: Arc<SubscriptionLogin>,
    ) -> Self {
        Self {
            namespace,
            executable,
            fence,
            budget,
            jobs,
            login,
            parked: Mutex::new(BTreeMap::new()),
        }
    }

    pub(super) fn record_park(
        &self,
        seat: SeatId,
        session: uuid::Uuid,
        observed: ObservedAssistant,
    ) -> Result<(), DriverError> {
        let captured = (|| {
            let files = ParkedFiles::capture(
                &self.namespace.config(seat),
                &self.namespace.cwd(seat),
                session,
                observed,
            )
            .map_err(|_| DriverError::Isolation("same-seat parked file witness refused".into()))?;
            self.parked
                .lock()
                .map_err(|_| DriverError::Isolation("park binding unavailable".into()))?
                .insert(seat, (session, files));
            Ok(())
        })();
        if captured.is_err() {
            let _ = self.budget.stop();
        }
        captured
    }

    fn consume_park(&self, seat: SeatId, session: uuid::Uuid) -> Result<(), DriverError> {
        let mut parked = self
            .parked
            .lock()
            .map_err(|_| DriverError::Isolation("park binding unavailable".into()))?;
        let (bound, files) = parked
            .get(&seat)
            .ok_or_else(|| DriverError::Isolation("verified same-run park missing".into()))?;
        if *bound != session {
            return Err(DriverError::Isolation("park session differs".into()));
        }
        files
            .verify(
                &self.namespace.config(seat),
                &self.namespace.cwd(seat),
                session,
            )
            .map_err(|_| DriverError::Isolation("parked file tree changed or unsafe".into()))?;
        parked.remove(&seat);
        Ok(())
    }

    pub(super) async fn start(
        &self,
        cfg: &ClaudeConfig,
        resume: Option<&str>,
        reported_cost_limit: Option<f64>,
    ) -> Result<(ChildProc, SessionInfo), DriverError> {
        native_admission()?;
        if resume.is_some() {
            // Separate unconditional hold: a future first-start go cannot
            // accidentally activate retained-file adoption before review.
            resume_admission()?;
        }
        let session = match resume {
            Some(id) => {
                let parsed = uuid::Uuid::parse_str(id)
                    .map_err(|_| DriverError::Isolation("invalid saved session".into()))?;
                if parsed.to_string() != id {
                    return Err(DriverError::Isolation("noncanonical saved session".into()));
                }
                parsed
            }
            None => uuid::Uuid::new_v4(),
        };
        let id = session.to_string();
        let job = self
            .jobs
            .spawn_checked(|| {
                if self.budget.usage().map_or(true, |usage| usage.stopped) {
                    return Err(JobFailure::Stopped);
                }
                // No await after these checks. Slot waiting cannot stale the guard.
                verify_executable(&self.executable).map_err(|_| JobFailure::Spawn)?;
                let construct = |_: &Path, _: &Path| {
                    let mut command = if resume.is_some() {
                        self.consume_park(cfg.seat, session)?;
                        watched::parked_command(
                            &self.executable,
                            &self.namespace,
                            cfg,
                            self.fence,
                            SessionArg::Resume(&id),
                        )?
                    } else {
                        watched_command(
                            &self.executable,
                            &self.namespace,
                            cfg,
                            self.fence,
                            SessionArg::New(&id),
                        )?
                    };
                    if let Some(limit) = reported_cost_limit {
                        command.args(["--max-budget-usd".to_string(), limit.to_string()]);
                    }
                    self.login.apply_to(&mut command);
                    Ok::<_, DriverError>(command)
                };
                if resume.is_some() {
                    self.namespace
                        .with_checked_parked_spawn(cfg.seat, construct)
                        .map_err(|_| JobFailure::Spawn)
                } else {
                    self.namespace
                        .with_checked_initial_spawn(cfg.seat, false, construct)
                        .map_err(|_| JobFailure::Spawn)
                }
            })
            .await
            .map_err(|_| {
                let _ = self.budget.stop();
                DriverError::Isolation("watched guarded job launch refused".into())
            })?;
        // This is synchronous: cancellation cannot detach the just-started job
        // between spawn and registration in the driver-owned supervisor.
        let mut process = ChildProc::from_watched(job, self.budget.clone());
        process.bind_watched_session(session);
        Ok((
            process,
            SessionInfo {
                session_id: id,
                model: Some(MODEL.into()),
                cli_version: Some(WATCHED_VERSION.into()),
                resumed: resume.is_some(),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn init_requires_exact_session_model_version_and_connected_own_tools() {
        let tools = vec!["mcp__cna__observe".into()];
        assert!(
            check_init(
                Some("own"),
                "own",
                Some(MODEL),
                Some(WATCHED_VERSION),
                &tools,
                Some(true)
            )
            .is_ok()
        );
        for (session, model, version, connected) in [
            ("foreign", Some(MODEL), Some(WATCHED_VERSION), Some(true)),
            ("own", Some("haiku"), Some(WATCHED_VERSION), Some(true)),
            ("own", Some(MODEL), Some("2.1.292"), Some(true)),
            ("own", None, Some(WATCHED_VERSION), Some(true)),
            ("own", Some(MODEL), None, Some(true)),
            ("own", Some(MODEL), Some(WATCHED_VERSION), None),
            ("own", Some(MODEL), Some(WATCHED_VERSION), Some(false)),
        ] {
            assert!(check_init(Some("own"), session, model, version, &tools, connected).is_err());
        }
        assert!(
            check_init(
                Some("own"),
                "own",
                Some(MODEL),
                Some(WATCHED_VERSION),
                &[],
                Some(true)
            )
            .is_err()
        );
        assert!(
            check_init(
                Some("own"),
                "own",
                Some(MODEL),
                Some(WATCHED_VERSION),
                &["Bash".into()],
                Some(true)
            )
            .is_err()
        );
    }
}
