//! Watched launcher ownership. Operator opt-in is separate from seat authority.
use cna_core::ids::SeatId;
use cna_play::{Demo, config::LaunchConfig};
use cna_seats::driver::SeatDriver;
type Drivers = Vec<(SeatId, Box<dyn SeatDriver>)>;

pub fn check_admission(config: &LaunchConfig, resume: bool) -> Result<(), String> {
    let operator = std::env::var("CNA_WATCHED_OPERATOR_GO").ok();
    check_admission_with(config, resume, operator.as_deref())
}

fn check_admission_with(
    config: &LaunchConfig,
    resume: bool,
    operator: Option<&str>,
) -> Result<(), String> {
    if config.claude_seats().next().is_none() {
        return Ok(());
    }
    if resume {
        return Err("cold campaign resume has no verified same-run in-memory binding".into());
    }
    if config.claude_seats().count() != 10
        || config
            .claude_seats()
            .any(|(_, model)| model != cna_seats::calibration::MODEL)
        || config.kind != cna_play::config::GameKind::Cna
        || config
            .run
            .as_ref()
            .is_none_or(|r| r.boundary.game_turn != 1 || r.boundary.op_stage.is_some())
        || config
            .session
            .as_ref()
            .is_none_or(|s| s.context_tokens > 100_000 || s.recoveries != 0)
    {
        return Err(
            "watched run requires ten pinned Haiku roles, GT1, context <=100000 and no recovery"
                .into(),
        );
    }
    #[cfg(windows)]
    {
        cna_seats::driver::claude::native_admission(operator.map(std::ffi::OsStr::new))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = operator;
        Err("watched launcher requires Windows job containment".into())
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use cna_seats::{
        calibration_http::watched::{WatchedGateway, WatchedReport},
        calibration_isolation::{ISOLATION_ROOT, WatchedNamespace},
        driver::claude::{ClaudeConfig, ClaudeDriver, WatchedLaunch},
        request_budget::{RequestBudget, RequestLimits},
        subscription_login::SubscriptionLogin,
        watched_job::JobRun,
    };
    use std::{
        fs,
        net::SocketAddr,
        path::{Path, PathBuf},
        sync::Arc,
        time::Duration,
    };
    use tokio::{sync::oneshot, task::JoinHandle};

    pub struct WatchedRuntime {
        launch: Arc<WatchedLaunch>,
        gateway: WatchedGateway,
        shutdown: Option<oneshot::Sender<()>>,
        service: Option<JoinHandle<std::io::Result<()>>>,
    }

    fn plain_directory(path: &Path) -> Result<(), String> {
        use std::os::windows::fs::MetadataExt;
        let metadata =
            fs::symlink_metadata(path).map_err(|_| "watched namespace metadata unavailable")?;
        if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
            return Err("watched namespace reparse/non-directory".into());
        }
        Ok(())
    }

    fn create_namespace(namespace: &WatchedNamespace) -> Result<(), String> {
        let root = Path::new(ISOLATION_ROOT);
        // Never create/follow missing machine ancestors or adopt old run state.
        plain_directory(Path::new("C:/"))?;
        plain_directory(root)?;
        if fs::read_dir(root)
            .map_err(|_| "isolation root unreadable")?
            .next()
            .is_some()
        {
            return Err("isolation root must be empty for a fresh watched run".into());
        }
        fs::create_dir(namespace.run_dir()).map_err(|_| "fresh watched run creation failed")?;
        for seat in SeatId::all() {
            let pair = namespace
                .cwd(seat)
                .parent()
                .ok_or("seat pair unavailable")?
                .to_owned();
            fs::create_dir(pair).map_err(|_| "seat pair creation failed")?;
            fs::create_dir(namespace.cwd(seat)).map_err(|_| "seat cwd creation failed")?;
            fs::create_dir(namespace.config(seat)).map_err(|_| "seat config creation failed")?;
        }
        // Metadata guard is repeated at actual spawn, after acquiring capacity.
        for seat in SeatId::all() {
            namespace
                .with_checked_initial_spawn(seat, false, |_, _| Ok::<_, ()>(()))
                .map_err(|_| "watched initial namespace refused")?;
        }
        Ok(())
    }

    fn positive_env(name: &str) -> Result<u64, String> {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|v| *v > 0)
            .ok_or_else(|| format!("explicit positive {name} required"))
    }

    impl WatchedRuntime {
        pub async fn prepare(config: &LaunchConfig, resume: bool) -> Result<Self, String> {
            check_admission(config, resume)?; // No credential read or process before this gate.
            if std::env::var("CNA_LIVE_CLI_TESTS").as_deref() != Ok("1") {
                return Err("CNA_LIVE_CLI_TESTS=1 and current owner go required".into());
            }
            let operator = std::env::var_os("CNA_WATCHED_OPERATOR_GO");
            let go = cna_seats::driver::claude::native_admission(operator.as_deref())
                .map_err(|e| e.to_string())?;
            let executable = PathBuf::from(
                std::env::var_os("CNA_WATCHED_CLAUDE_EXE")
                    .ok_or("explicit reviewed Claude executable required")?,
            );
            if !executable.is_absolute() {
                return Err("absolute Claude executable required".into());
            }
            let token_file = PathBuf::from(
                std::env::var_os("CNA_OWNER_LOGIN_FILE")
                    .ok_or("owner-provisioned login file required")?,
            );
            let session = config
                .session
                .as_ref()
                .ok_or("durable session limits required")?;
            let wall = Duration::from_secs(session.wall_seconds);
            let limits = RequestLimits {
                total: positive_env("CNA_WATCHED_REQUEST_CAP")?,
                generations: positive_env("CNA_WATCHED_GENERATION_CAP")?,
                wall,
            };
            let budget = RequestBudget::new(limits).map_err(|e| e.to_string())?;
            let jobs = JobRun::new(wall).map_err(|e| e.to_string())?;
            let gateway = WatchedGateway::new(budget.clone()).map_err(|e| e.to_string())?;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|e| e.to_string())?;
            let fence: SocketAddr = listener.local_addr().map_err(|e| e.to_string())?;
            let namespace = Arc::new(WatchedNamespace::new(uuid::Uuid::new_v4()));
            create_namespace(&namespace)?;
            // The only launcher reader; deletion succeeds before any seat can
            // receive the intended environment. Never auth-probe ambient login.
            let login =
                Arc::new(SubscriptionLogin::consume_file(&token_file).map_err(|e| e.to_string())?);
            let launch = Arc::new(WatchedLaunch::new(
                namespace,
                executable,
                fence,
                budget.clone(),
                jobs,
                login,
                go,
            ));
            let (shutdown, stopped) = oneshot::channel();
            let server = gateway.clone();
            let service = tokio::spawn(async move {
                server
                    .serve(listener, async move {
                        tokio::select! {
                            _ = stopped => {},
                            _ = budget.bound(std::future::pending::<()>()) => {},
                        }
                    })
                    .await
            });
            println!(
                "Watched routed caps: {} forwards including counts/retries; {} generations; {}s; <=2 contained CLI roots. Direct CLI egress is outside routed accounting.",
                limits.total, limits.generations, session.wall_seconds
            );
            Ok(Self {
                launch,
                gateway,
                shutdown: Some(shutdown),
                service: Some(service),
            })
        }

        pub fn drivers(&self, demo: &Demo) -> Result<Drivers, String> {
            demo.config
                .claude_seats()
                .map(|(seat, model)| {
                    let cfg = ClaudeConfig {
                        seat,
                        exe: Some(self.launch.executable.clone()),
                        model: model.into(),
                        config_dir: Some(self.launch.namespace.config(seat)),
                        expected_email: None,
                        sandbox: self.launch.namespace.cwd(seat),
                        run_dir: demo.outbox.join(seat.to_string()),
                        mcp_url: demo
                            .mcp
                            .url(seat)
                            .ok_or("own seat MCP endpoint unavailable")?,
                        system_prompt: cna_seats::run::PromptBuilder::system_prompt(
                            &demo.prompts,
                            seat,
                        ),
                        effort: None,
                        context_window: demo.config.session.as_ref().map(|s| s.context_tokens),
                    };
                    Ok((
                        seat,
                        Box::new(ClaudeDriver::new_watched(
                            cfg,
                            demo.sink.clone(),
                            self.launch.clone(),
                        )) as Box<dyn SeatDriver>,
                    ))
                })
                .collect()
        }

        pub fn stop_admission(&self) {
            let _ = self.gateway.stop();
        }

        pub async fn shutdown(&mut self) -> Result<WatchedReport, String> {
            shutdown_service(&self.gateway, &mut self.shutdown, &mut self.service).await?;
            self.gateway.report().map_err(|e| e.to_string())
        }
    }

    async fn shutdown_service(
        gateway: &WatchedGateway,
        shutdown: &mut Option<oneshot::Sender<()>>,
        service_task: &mut Option<JoinHandle<std::io::Result<()>>>,
    ) -> Result<(), String> {
        let _ = gateway.stop();
        if let Some(stop) = shutdown.take() {
            let _ = stop.send(());
        }
        let mut failure = None;
        // HTTP graceful drain is independently bounded. Even on timeout we
        // join service destruction AND explicitly drain its tracked pumps.
        if let Some(service) = service_task.as_mut() {
            match tokio::time::timeout(Duration::from_secs(3), &mut *service).await {
                Ok(Ok(Ok(()))) => {}
                Ok(_) => failure = Some("watched gateway service failed".to_string()),
                Err(_) => {
                    service.abort();
                    let _ = service.await;
                    failure = Some("watched gateway HTTP cleanup timed out".to_string());
                }
            }
        }
        service_task.take();
        if gateway.shutdown_pumps().await.is_err() {
            failure = Some("watched gateway pump cleanup failed".into());
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(())
    }

    impl Drop for WatchedRuntime {
        fn drop(&mut self) {
            self.stop_admission();
            if let Some(stop) = self.shutdown.take() {
                let _ = stop.send(());
            }
            if let Some(task) = &self.service {
                task.abort();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct Destroyed(Arc<AtomicBool>);
        impl Drop for Destroyed {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        #[tokio::test]
        async fn bounded_service_timeout_stops_admission_and_joins_abort() {
            let budget = RequestBudget::new(RequestLimits {
                total: 2,
                generations: 1,
                wall: Duration::from_secs(30),
            })
            .unwrap();
            let gateway = WatchedGateway::new(budget.clone()).unwrap();
            let destroyed = Arc::new(AtomicBool::new(false));
            let marker = Destroyed(destroyed.clone());
            let mut task = Some(tokio::spawn(async move {
                let _marker = marker;
                std::future::pending::<std::io::Result<()>>().await
            }));
            let (stop, _rx) = oneshot::channel();
            let mut shutdown = Some(stop);
            let result = shutdown_service(&gateway, &mut shutdown, &mut task).await;
            assert!(result.as_ref().is_err_and(|e| e.contains("timed out")));
            assert!(task.is_none() && shutdown.is_none() && destroyed.load(Ordering::SeqCst));
            assert!(budget.usage().unwrap().stopped);
            assert_eq!(budget.usage().unwrap().total, 0);
        }

        #[tokio::test]
        async fn real_loopback_service_owner_stops_and_joins_without_provider() {
            let budget = RequestBudget::new(RequestLimits {
                total: 2,
                generations: 1,
                wall: Duration::from_secs(30),
            })
            .unwrap();
            let gateway = WatchedGateway::new(budget.clone()).unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let (stop, stopped) = oneshot::channel();
            let server = gateway.clone();
            let mut task = Some(tokio::spawn(async move {
                server
                    .serve(listener, async move {
                        let _ = stopped.await;
                    })
                    .await
            }));
            let mut shutdown = Some(stop);
            let result = shutdown_service(&gateway, &mut shutdown, &mut task).await;
            assert!(result.is_ok() && task.is_none() && shutdown.is_none());
            assert_eq!(budget.usage().unwrap().total, 0);
        }
    }
}
#[cfg(windows)]
pub use platform::WatchedRuntime;

#[cfg(not(windows))]
pub struct WatchedRuntime;
#[cfg(not(windows))]
impl WatchedRuntime {
    pub async fn prepare(_: &LaunchConfig, _: bool) -> Result<Self, String> {
        Err("Windows required".into())
    }
    pub fn drivers(&self, _: &Demo) -> Result<Drivers, String> {
        Err("Windows required".into())
    }
    pub fn stop_admission(&self) {}
    pub async fn shutdown(&mut self) -> Result<(), String> {
        Err("Windows required".into())
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    #[test]
    fn operator_go_is_exact_and_cold_resume_still_has_no_binding() {
        let config = cna_play::config::parse_args(&[
            "--preset".into(),
            "graziani-haiku".into(),
            "--stop-after-turn".into(),
            "1".into(),
            "--budget-usd".into(),
            "1".into(),
            "--recoveries".into(),
            "0".into(),
        ])
        .unwrap();
        let cna_play::config::Command::Play(config) = config else {
            panic!()
        };
        for value in [
            None,
            Some(""),
            Some("1"),
            Some("calibration"),
            Some("setup-gt1 "),
            Some("setup-gt1,same-run-resume"),
        ] {
            assert!(check_admission_with(&config, false, value).is_err());
        }
        for value in ["setup-gt1", "setup-gt1+same-run-resume"] {
            assert_eq!(
                check_admission_with(&config, false, Some(value)).is_ok(),
                cfg!(windows)
            );
            assert!(check_admission_with(&config, true, Some(value)).is_err());
        }
    }
    #[test]
    fn scripted_and_human_launches_do_not_acquire_login_or_native_admission() {
        let config = LaunchConfig::resolve(
            cna_play::config::GameKind::Sandbox,
            &["*=scripted:legal_random".into()],
        )
        .unwrap();
        assert!(check_admission(&config, false).is_ok());
        assert!(check_admission(&config, true).is_ok());
    }
}
