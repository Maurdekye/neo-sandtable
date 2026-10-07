//! Reviewed cleanup regression: synthetic child flags and real server, no provider calls.
use cna_core::ids::SeatId;
use cna_play::{
    Demo,
    budget::{RunControl, SpendBudget},
    config::{GameKind, LaunchConfig, SessionLimits},
};
use cna_seats::driver::{
    CliKind, DriverError, EstimateBound, SeatDriver, SessionInfo, TurnOutcome,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
fn seat() -> SeatId {
    "axis.commander".parse().unwrap()
}
fn other() -> SeatId {
    "commonwealth.commander".parse().unwrap()
}
fn bound() -> EstimateBound {
    EstimateBound {
        pinned_model: "haiku".into(),
        context_tokens: 10,
        output_tokens: 10,
        max_input_or_cache_write_usd_per_token: 0.001,
        output_usd_per_token: 0.001,
        evidence: "reviewer inert assumption, not native proof".into(),
    }
}
fn config() -> LaunchConfig {
    let mut c = LaunchConfig::resolve(
        GameKind::Sandbox,
        &[
            "axis.commander=claude:haiku".into(),
            "commonwealth.commander=claude:haiku".into(),
            "*=scripted:aggressive".into(),
        ],
    )
    .unwrap();
    c.session = Some(SessionLimits::default());
    c.max_turns = 20;
    c.tool_calls = 40;
    c.run = Some(RunControl {
        boundary: cna_server::RunBoundary {
            game_turn: 2,
            op_stage: None,
        },
        budget: SpendBudget::usd(
            4.,
            &[seat(), other()],
            BTreeMap::from([(seat(), 1.), (other(), 1.)]),
        )
        .unwrap(),
    });
    c
}
#[derive(Clone, Copy)]
enum StopMode {
    Slow,
    Failed,
    Confirmed,
}
struct SlowStop {
    alive: Arc<AtomicBool>,
    mode: StopMode,
}
#[async_trait::async_trait]
impl SeatDriver for SlowStop {
    fn kind(&self) -> CliKind {
        CliKind::Claude
    }
    fn estimate_bound(&self) -> Option<EstimateBound> {
        Some(bound())
    }
    fn set_reported_cost_limit(&mut self, _: f64) -> Result<(), DriverError> {
        Ok(())
    }
    async fn start(&mut self, _: Option<&str>) -> Result<SessionInfo, DriverError> {
        self.alive.store(true, Ordering::SeqCst);
        Err(DriverError::Protocol(
            "inert startup failed after spawn".into(),
        ))
    }
    async fn run_turn(&mut self, _: &str, _: Duration) -> Result<TurnOutcome, DriverError> {
        panic!("no model turn should start")
    }
    fn session_id(&self) -> Option<String> {
        None
    }
    fn is_alive(&mut self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
    async fn stop(&mut self) -> Result<(), cna_seats::driver::DriverError> {
        match self.mode {
            StopMode::Slow => tokio::time::sleep(Duration::from_secs(6)).await,
            StopMode::Failed => return Err(DriverError::Died("inert termination denied".into())),
            StopMode::Confirmed => {}
        }
        self.alive.store(false, Ordering::SeqCst);
        Ok(())
    }
}
async fn check_cleanup(mode: StopMode) {
    let root = tempfile::tempdir().unwrap();
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut c = config();
    c.seats.insert(
        other(),
        cna_play::config::Controller::Scripted("aggressive".into()),
    );
    c.run.as_mut().unwrap().budget =
        SpendBudget::usd(4., &[seat()], BTreeMap::from([(seat(), 1.)])).unwrap();
    let demo = Demo::with_config(root.path(), &repo.join("data"), &repo.join("web/dist"), c)
        .await
        .unwrap();
    let alive = Arc::new(AtomicBool::new(false));
    let mut ds = vec![(
        seat(),
        Box::new(SlowStop {
            alive: alive.clone(),
            mode,
        }) as Box<dyn SeatDriver>,
    )];
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let result = tokio::time::timeout(Duration::from_secs(20), demo.play_durable(&mut ds, rx))
        .await
        .unwrap();
    let error = result.unwrap_err();
    let unconfirmed = alive.load(Ordering::SeqCst);
    let s = demo.journal.as_ref().unwrap().snapshot().unwrap();
    let status = demo.handle.status();
    let admission = demo
        .journal
        .as_ref()
        .unwrap()
        .admission(seat(), Some(&bound()))
        .unwrap();
    // Settle the synthetic resource and join every real server helper before assertions.
    if matches!(mode, StopMode::Slow) {
        ds[0].1.stop().await.unwrap();
    } else {
        alive.store(false, Ordering::SeqCst);
    }
    demo.shutdown().await.unwrap();
    match mode {
        StopMode::Confirmed => {
            assert!(error.contains("inert startup failed after spawn"));
            assert!(!unconfirmed);
            assert!(s.seats[&seat()].admitted_estimate_usd.is_none());
            assert!(!s.seats[&seat()].uncertain_budget_spend);
            assert!(matches!(
                admission,
                cna_play::budget::Admission::Ready { .. }
            ));
        }
        StopMode::Slow | StopMode::Failed => {
            assert!(unconfirmed);
            assert_eq!(s.seats[&seat()].admitted_estimate_usd, Some(1.02));
            assert!(s.seats[&seat()].uncertain_budget_spend);
            assert_eq!(s.seats[&seat()].incomplete_turns, 1);
            assert!(matches!(status, cna_server::CampaignStatus::Paused));
            assert!(matches!(admission, cna_play::budget::Admission::Stop(_)));
            assert!(error.contains(if matches!(mode, StopMode::Slow) {
                "CLI stop exceeded five seconds"
            } else {
                "inert termination denied"
            }));
        }
    }
}
#[tokio::test]
async fn correct_cleanup_timeout_must_retain_uncertainty() {
    check_cleanup(StopMode::Slow).await;
}
#[tokio::test]
async fn failed_termination_retains_envelope_and_stops_admission() {
    check_cleanup(StopMode::Failed).await;
}
#[tokio::test]
async fn confirmed_no_model_cleanup_releases_startup_envelope() {
    check_cleanup(StopMode::Confirmed).await;
}
