//! Server-backed seat launcher with opt-in durable campaign sessions.
mod campaign;
pub mod config;
pub mod journal;
use cna_core::{ids::SeatId, visibility::Perspective};
use cna_protocol::{ControllerInfo, ControllerKind, ServerMessage};
use cna_seats::{
    driver::SeatDriver,
    mcp::{McpServer, SeatEndpoint, ToolRouter},
    run::{DefaultPrompts, PromptBuilder},
    transcript::TranscriptSink,
};
use cna_server::{
    CampaignStatus,
    actor::CampaignHandle,
    campaigns,
    http::{App, CreateRequest},
};
use config::{Controller, GameKind, LaunchConfig};
use futures_util::{StreamExt, stream::FuturesUnordered};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::task::JoinHandle;

pub const CALL_CAP: u64 = 40;
pub const WALL_LIMIT: Duration = Duration::from_secs(150);
pub const SCRIPTED_WALL_LIMIT: Duration = Duration::from_secs(600);
pub const TURN_LIMIT: Duration = Duration::from_secs(60);

pub struct Demo {
    pub handle: CampaignHandle,
    pub config: LaunchConfig,
    pub epochs: BTreeMap<SeatId, u64>,
    pub seat: SeatId,
    pub epoch: u64,
    pub router: Arc<ToolRouter>,
    pub sink: TranscriptSink,
    pub prompts: DefaultPrompts,
    pub mcp: McpServer,
    pub base_url: String,
    pub outbox: PathBuf,
    pub journal: Option<Arc<journal::SessionJournal>>,
    app: App,
    http: JoinHandle<()>,
}

impl Demo {
    pub async fn new(directory: &Path, data: &Path, dist: &Path) -> Self {
        Self::with_config(directory, data, dist, LaunchConfig::default())
            .await
            .expect("create demo")
    }

    pub async fn with_config(
        directory: &Path,
        data: &Path,
        dist: &Path,
        config: LaunchConfig,
    ) -> Result<Self, String> {
        config.validate()?;
        let handle = campaigns::create(
            directory,
            data,
            CreateRequest {
                kind: match config.kind {
                    GameKind::Sandbox => cna_server::http::CampaignKind::Sandbox,
                    GameKind::Cna => cna_server::http::CampaignKind::Cna,
                },
                rules_profile: config.profile().into(),
                seed: [7; 32],
                title: format!("AI integration ({})", config.profile()),
                paused: true,
                controller: "human".into(),
            },
        )
        .map_err(|e| e.to_string())?;
        let mut epochs = BTreeMap::new();
        for (seat, controller) in &config.seats {
            let (info, settings) = match controller {
                Controller::Claude(model) => (
                    ControllerInfo {
                        kind: ControllerKind::LlmCli,
                        label: format!("Claude Code / {model}"),
                    },
                    json!({"model":model,"max_turns":config.max_turns,"max_tool_calls":config.tool_calls,
                        "max_wall_s":config.session.as_ref().map_or(WALL_LIMIT.as_secs(),|s|s.wall_seconds),"turn_timeout_s":config.session.as_ref().map_or(TURN_LIMIT.as_secs(),|s|s.turn_seconds),"session_limits":config.session}),
                ),
                Controller::Scripted(mode) => (
                    ControllerInfo {
                        kind: ControllerKind::Scripted,
                        label: format!("scripted:{mode}"),
                    },
                    json!({"mode":mode}),
                ),
                Controller::Human => (
                    ControllerInfo {
                        kind: ControllerKind::Human,
                        label: "Human".into(),
                    },
                    json!({"mode":"human"}),
                ),
            };
            let binding = handle
                .handover(*seat, Some(info), settings)
                .await
                .map_err(|e| e.to_string())?;
            if matches!(controller, Controller::Claude(_)) {
                epochs.insert(*seat, binding.controller_epoch);
            }
        }
        let id = handle.projection(Perspective::Operator).meta.id;
        let journal = if config.session.is_some() {
            Some(Arc::new(journal::SessionJournal::create(
                directory,
                &id,
                config.clone(),
                &epochs,
            )?))
        } else {
            None
        };
        Self::serve(directory, data, dist, config, handle, epochs, journal).await
    }

    pub async fn resume(path: &Path, data: &Path, dist: &Path) -> Result<Self, String> {
        let directory = path.parent().ok_or("campaign path needs a directory")?;
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("invalid campaign database name")?;
        // Acquire lease before starting the recovered campaign writer.
        let journal = Arc::new(journal::SessionJournal::recover(directory, id)?);
        let saved = journal.snapshot()?;
        let handle = campaigns::recover(path, data).map_err(|e| e.to_string())?;
        let checked = async {
            if handle.projection(Perspective::Operator).meta.id != id {return Err("campaign identity differs from journal".to_string());}
            if matches!(handle.status(), CampaignStatus::Running) {handle.pause(true).await.map_err(|e|e.to_string())?;}
            for (seat, record) in &saved.seats {
                let binding=handle.seat(*seat).binding;
                if binding.controller_epoch!=record.epoch || binding.paused || binding.failure.is_some() || binding.controller.as_ref().map(|c|c.kind)!=Some(ControllerKind::LlmCli) || binding.config.get("model").and_then(|s|s.as_str())!=Some(record.model.as_str()) {
                    return Err(format!("{seat}: journal binding was replaced or failed; explicit operator action required"));
                }
            }
            Ok(())
        }.await;
        if let Err(error) = checked {
            let cleanup = handle.shutdown().await.map_err(|e| e.to_string());
            return combine_results(Err(error), cleanup).map(|_| unreachable!());
        }
        let epochs = saved.seats.iter().map(|(s, r)| (*s, r.epoch)).collect();
        Self::serve(
            directory,
            data,
            dist,
            saved.config,
            handle,
            epochs,
            Some(journal),
        )
        .await
    }

    async fn serve(
        directory: &Path,
        data: &Path,
        dist: &Path,
        config: LaunchConfig,
        handle: CampaignHandle,
        epochs: BTreeMap<SeatId, u64>,
        journal: Option<Arc<journal::SessionJournal>>,
    ) -> Result<Self, String> {
        let seat = epochs
            .keys()
            .next()
            .copied()
            .unwrap_or_else(|| "axis.commander".parse().unwrap());
        let epoch = handle.seat(seat).binding.controller_epoch;
        let shared = Arc::new(handle.clone());
        let seats: Vec<_> = epochs.keys().copied().collect();
        let mut router = ToolRouter::new(shared.clone(), shared.clone(), &seats);
        if let Some(journal) = &journal {
            router = router.with_budget(journal.clone());
        } else {
            router = router.with_call_cap(Some(config.tool_calls));
        }
        let router = Arc::new(router);
        let prompts = DefaultPrompts { game_description: match config.kind {
            GameKind::Sandbox => "This is sandbox-v1, a synthetic integration game. Call observe for its complete rules summary.",
            GameKind::Cna => "This is The Campaign for North Africa, Graziani's Offensive, under the development rules profile. Only implemented procedures are offered. Read the current observation, decision context and legal action schema through your tools. Future windows may ask for different orders; never assume initiative is the only kind. Unimplemented procedures are skipped by this profile; this is not a complete rules simulation.",
        }.into() };
        let endpoints = epochs
            .iter()
            .map(|(seat, epoch)| SeatEndpoint {
                seat: *seat,
                epoch: *epoch,
                instructions: prompts.system_prompt(*seat),
            })
            .collect();
        let mcp = McpServer::start(router.clone(), endpoints)
            .await
            .map_err(|e| e.to_string())?;
        let sink = TranscriptSink::new(shared);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let data: PathBuf = data.into();
        let app = App::new(
            directory.into(),
            port,
            Arc::new(move |r, dir| campaigns::create(dir, &data, r)),
        );
        app.register(handle.clone());
        let routes = app.router(dist);
        let http = tokio::spawn(async move {
            axum::serve(listener, routes).await.unwrap();
        });
        let campaign_id = handle.projection(Perspective::Operator).meta.id;
        Ok(Self {
            handle,
            config,
            epochs,
            seat,
            epoch,
            router,
            sink,
            prompts,
            mcp,
            base_url: format!("http://127.0.0.1:{port}"),
            outbox: directory.join(format!("{campaign_id}.unconfirmed.jsonl")),
            journal,
            app,
            http,
        })
    }
    pub fn campaign_id(&self) -> String {
        self.handle.projection(Perspective::Operator).meta.id
    }

    /// Trusted spectator URL. Never pass it to a seat driver or its environment.
    pub fn board_url(&self) -> String {
        format!(
            "{}/?campaign={}#cap={}",
            self.base_url,
            self.campaign_id(),
            self.app.operator_token()
        )
    }

    /// Compatibility helper for the original single-seat probe.
    pub async fn play(&self, driver: &mut dyn SeatDriver, turns: usize) -> Result<(), String> {
        if self.epochs.len() != 1 {
            return Err("use play_sessions for zero or multiple Claude bindings".into());
        }
        let start = self.handle.pause(false).await.map_err(|e| e.to_string());
        let result = if start.is_ok() {
            let (_cancel, stop) = tokio::sync::watch::channel(false);
            self.run_session(self.seat, self.epoch, driver, turns, stop)
                .await
        } else {
            driver.stop().await;
            start
        };
        self.finish_run(result).await
    }

    /// All configured CLI seats run concurrently (at most two); a completed or failed bounded session stops siblings.
    /// Human/scripted-only runs launch no CLI and need no paid opt-in.
    pub async fn play_sessions(
        &self,
        drivers: &mut [(SeatId, Box<dyn SeatDriver>)],
        turns: usize,
    ) -> Result<(), String> {
        let provided: BTreeSet<_> = drivers.iter().map(|(seat, _)| *seat).collect();
        let expected: BTreeSet<_> = self.epochs.keys().copied().collect();
        if provided != expected
            || provided.len() != drivers.len()
            || drivers
                .iter()
                .any(|(_, driver)| driver.kind() != cna_seats::driver::CliKind::Claude)
        {
            return Err(
                "exactly one confined Claude driver is required for each Claude binding".into(),
            );
        }
        let start = self.handle.pause(false).await.map_err(|e| e.to_string());
        if start.is_err() {
            for (_, driver) in drivers {
                driver.stop().await;
            }
            return self.finish_run(start).await;
        }
        let result = if drivers.is_empty() {
            let mut state = self.handle.watch_status();
            tokio::time::timeout(SCRIPTED_WALL_LIMIT, async {
                loop {
                    match state.borrow_and_update().clone() {
                        CampaignStatus::Running => {}
                        CampaignStatus::Finished { .. } => return Ok(()),
                        other => {
                            return Err(format!("campaign stopped before completion: {other:?}"));
                        }
                    }
                    state.changed().await.map_err(|e| e.to_string())?;
                }
            })
            .await
            .unwrap_or_else(|_| Err("scripted campaign wall-clock budget exhausted".into()))
        } else {
            let (cancel, stop) = tokio::sync::watch::channel(false);
            let mut tasks = FuturesUnordered::new();
            for (seat, driver) in drivers {
                let seat = *seat;
                let epoch = self.epochs[&seat];
                let stop = stop.clone();
                tasks.push(async move {
                    (
                        seat,
                        self.run_session(seat, epoch, driver.as_mut(), turns, stop)
                            .await,
                    )
                });
            }
            let mut result = Ok(());
            while let Some((seat, outcome)) = tasks.next().await {
                let _ = cancel.send(true);
                result = combine_results(result, outcome.map_err(|e| format!("{seat}: {e}")));
            }
            result
        };
        self.finish_run(result).await
    }

    async fn run_session(
        &self,
        seat: SeatId,
        epoch: u64,
        driver: &mut dyn SeatDriver,
        turns: usize,
        mut stop: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), String> {
        let mut state = self.handle.watch_seat(seat);
        let invalidated = async {
            loop {
                {
                    let current = state.borrow_and_update();
                    if current.binding.controller_epoch != epoch || current.binding.paused {
                        break;
                    }
                }
                if state.changed().await.is_err() {
                    break;
                }
            }
        };
        let cancelled = async {
            while !*stop.borrow_and_update() {
                if stop.changed().await.is_err() {
                    break;
                }
            }
        };
        let work = async {
            let mut turn = 0;
            let mut started = false;
            loop {
                let mut windows = self.handle.watch_seat(seat);
                let pending = loop {
                    let p = windows.borrow_and_update().pending.clone();
                    if !p.is_empty() {
                        break p;
                    }
                    match self.handle.status() {
                        CampaignStatus::Finished { .. } => return Ok(()),
                        CampaignStatus::Stopped { .. } | CampaignStatus::Paused => {
                            return Err("campaign is no longer running".into());
                        }
                        CampaignStatus::Running => {}
                    }
                    windows.changed().await.map_err(|e| e.to_string())?;
                };
                let mut automatic = false;
                for request in &pending {
                    if cna_seats::automatic::answer(&self.handle, seat, epoch, request, &self.sink)
                        .await
                        .map_err(|e| e.to_string())?
                    {
                        automatic = true;
                    }
                }
                if automatic {
                    continue;
                }
                if turn >= turns.min(self.config.max_turns) {
                    return Ok(());
                }
                if !started {
                    driver.start(None).await.map_err(|e| e.to_string())?;
                    started = true;
                }
                let prompt = if turn == 0 {
                    let notebook = self
                        .handle
                        .notebook(seat)
                        .await
                        .map_err(|e| e.to_string())?;
                    self.prompts.first_turn(seat, &notebook, &pending)
                } else {
                    self.prompts.window_turn(seat, &pending)
                };
                let prompt = format!(
                    "{prompt}\n\nThis is a bounded probe. Answer only the decision IDs listed in this request, then end your turn even if a later observation reveals new windows. Do not chase later windows in this turn."
                );
                let outcome = driver
                    .run_turn(&prompt, TURN_LIMIT)
                    .await
                    .map_err(|e| e.to_string())?;
                if !outcome.ok {
                    return Err(outcome.error.unwrap_or_else(|| "CLI refused turn".into()));
                }
                let now = self.handle.seat(seat);
                if pending.iter().any(|old| {
                    now.pending
                        .iter()
                        .any(|new| new.id == old.id && new.revision == old.revision)
                }) {
                    return Err("CLI ended without answering its pending decisions".into());
                }
                self.sink.system(
                    seat,
                    format!(
                        "bounded demo turn {} complete; usage {:?}",
                        turn + 1,
                        outcome.usage
                    ),
                );
                turn += 1;
                if turn >= turns.min(self.config.max_turns) {
                    return Ok(());
                }
            }
        };
        let mut result = tokio::select! {
            _ = invalidated => Err("controller binding changed; old session stopped".into()),
            _ = cancelled => { self.sink.system(seat, "bounded peer session ended; this session stopped"); Ok(()) },
            result = tokio::time::timeout(WALL_LIMIT, work) => result.unwrap_or_else(|_| Err("demo wall-clock budget exhausted".into())),
        };
        driver.stop().await;
        if let Err(reason) = &result
            && !matches!(
                self.handle.status(),
                CampaignStatus::Finished { .. } | CampaignStatus::Stopped { .. }
            )
        {
            match self.handle.mark_failure_if_epoch(seat, epoch, reason).await {
                Ok(()) | Err(cna_server::Error::StaleEpoch) => {}
                Err(error) => {
                    result = combine_results(
                        result,
                        Err(format!("recording seat failure failed: {error}")),
                    )
                }
            }
        }
        result
    }

    async fn finish_run(&self, mut result: Result<(), String>) -> Result<(), String> {
        if matches!(self.handle.status(), CampaignStatus::Running)
            && let Err(error) = self.handle.pause(true).await
            && !matches!(
                self.handle.status(),
                CampaignStatus::Finished { .. } | CampaignStatus::Stopped { .. }
            )
        {
            result = combine_results(result, Err(format!("campaign pause failed: {error}")));
        }
        combine_results(result, self.drain().await)
    }
    pub fn transcript(&self) -> Vec<ServerMessage> {
        self.transcript_for(self.seat)
            .expect("persisted transcript")
    }

    pub fn transcript_for(&self, seat: SeatId) -> Result<Vec<ServerMessage>, String> {
        self.handle
            .replay()
            .transcripts(Perspective::Operator, seat, 0)
            .map_err(|e| e.to_string())
    }
    /// Bound a probe's persistence drain. A dead writer cannot hold HTTP/MCP open forever.
    async fn drain(&self) -> Result<(), String> {
        let drained = tokio::time::timeout(Duration::from_secs(5), self.sink.flush()).await;
        if drained.is_ok() && self.sink.pending_count() == 0 {
            return Ok(());
        }
        let pending = self.sink.stop_delivery().await;
        let mut text = String::new();
        for entry in &pending {
            text.push_str(&serde_json::to_string(entry).map_err(|e| e.to_string())?);
            text.push('\n');
        }
        tokio::fs::write(&self.outbox, text).await.map_err(|e| {
            format!(
                "transcript persistence failed; recovery outbox {} could not be written: {e}",
                self.outbox.display()
            )
        })?;
        Err(format!(
            "transcript persistence failed; {} unconfirmed captures saved to {}. Inspect before replaying.",
            pending.len(),
            self.outbox.display()
        ))
    }

    pub async fn shutdown(self) -> Result<(), String> {
        let result = self.drain().await;
        self.sink.stop_delivery().await;
        self.mcp.shutdown();
        self.http.abort();
        let _ = self.http.await;
        self.app.shutdown().await;
        result
    }
}

/// Serve a saved campaign for thirty seconds without starting any model session.
pub async fn replay_board(path: &Path, data: &Path, dist: &Path) {
    let handle = campaigns::recover(path, data).expect("recover saved campaign");
    // Keep any recovered nonterminal game paused; this command never launches controllers.
    if matches!(handle.status(), CampaignStatus::Running) {
        handle.pause(true).await.expect("pause replay");
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let data: PathBuf = data.into();
    let app = App::new(
        path.parent().unwrap().into(),
        port,
        Arc::new(move |r, dir| campaigns::create(dir, &data, r)),
    );
    let id = handle.projection(Perspective::Operator).meta.id;
    app.register(handle);
    let routes = app.router(dist);
    let task = tokio::spawn(async move {
        axum::serve(listener, routes).await.unwrap();
    });
    println!(
        "Board: http://127.0.0.1:{port}/?campaign={id}#cap={}",
        app.operator_token()
    );
    println!("Saved campaign viewer; no CLI starts. Closing in 30 seconds.");
    tokio::time::sleep(Duration::from_secs(30)).await;
    task.abort();
    let _ = task.await;
    app.shutdown().await;
}

/// Preserve a primary failure alongside a cleanup/recovery failure.
pub fn combine_results(
    primary: Result<(), String>,
    cleanup: Result<(), String>,
) -> Result<(), String> {
    match (primary, cleanup) {
        (Err(primary), Err(cleanup)) => Err(format!("{primary}; additionally: {cleanup}")),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}
