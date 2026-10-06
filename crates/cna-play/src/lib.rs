//! Bounded integration probe, not the production campaign scheduler.
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
    http::{App, CreateRequest},
    sandbox,
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::task::JoinHandle;

pub const CALL_CAP: u64 = 40;
pub const WALL_LIMIT: Duration = Duration::from_secs(150);
pub const TURN_LIMIT: Duration = Duration::from_secs(60);

pub struct Demo {
    pub handle: CampaignHandle,
    pub seat: SeatId,
    pub epoch: u64,
    pub router: Arc<ToolRouter>,
    pub sink: TranscriptSink,
    pub prompts: DefaultPrompts,
    pub mcp: McpServer,
    pub base_url: String,
    pub outbox: PathBuf,
    app: App,
    http: JoinHandle<()>,
}

impl Demo {
    pub async fn new(directory: &Path, data: &Path, dist: &Path) -> Self {
        let handle = sandbox::create(
            directory,
            data,
            CreateRequest {
                kind: cna_server::http::CampaignKind::Sandbox,
                rules_profile: "sandbox-v1".into(),
                seed: [7; 32],
                title: "Haiku seat integration (synthetic sandbox)".into(),
                paused: true,
                controller: "aggressive".into(),
            },
        )
        .expect("create campaign");
        let seat: SeatId = "axis.commander".parse().unwrap();
        let binding = handle
            .handover(
                seat,
                Some(ControllerInfo {
                    kind: ControllerKind::LlmCli,
                    label: "Claude Code / haiku".into(),
                }),
                json!({"model":"haiku","max_turns":2,"max_tool_calls":CALL_CAP,
            "max_wall_s":WALL_LIMIT.as_secs(),"turn_timeout_s":TURN_LIMIT.as_secs()}),
            )
            .await
            .expect("handover");
        let shared = Arc::new(handle.clone());
        let router = Arc::new(
            ToolRouter::new(shared.clone(), shared.clone(), &[seat]).with_call_cap(Some(CALL_CAP)),
        );
        let prompts = DefaultPrompts {
            game_description: "This is sandbox-v1, a synthetic integration game. Call observe for its complete rules summary.".into(),
        };
        let mcp = McpServer::start(
            router.clone(),
            vec![SeatEndpoint {
                seat,
                epoch: binding.controller_epoch,
                instructions: prompts.system_prompt(seat),
            }],
        )
        .await
        .expect("MCP server");
        let sink = TranscriptSink::new(shared);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let data: PathBuf = data.into();
        let app = App::new(
            directory.into(),
            port,
            Arc::new(move |r, dir| sandbox::create(dir, &data, r)),
        );
        app.register(handle.clone());
        let routes = app.router(dist);
        let http = tokio::spawn(async move {
            axum::serve(listener, routes).await.unwrap();
        });
        let campaign_id = handle.projection(Perspective::Operator).meta.id;
        Self {
            handle,
            seat,
            epoch: binding.controller_epoch,
            router,
            sink,
            prompts,
            mcp,
            base_url: format!("http://127.0.0.1:{port}"),
            outbox: directory.join(format!("{}.{}.unconfirmed.jsonl", campaign_id, seat)),
            app,
            http,
        }
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

    /// At most two turns in one CLI process. Errors pause durably; no fallback orders.
    /// Epoch changes cancel the old process and its MCP endpoint remains bound to the old epoch.
    pub async fn play(&self, driver: &mut dyn SeatDriver, turns: usize) -> Result<(), String> {
        let mut state = self.handle.watch_seat(self.seat);
        let epoch = self.epoch;
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
        let work = async {
            driver.start(None).await.map_err(|e| e.to_string())?;
            self.handle.pause(false).await.map_err(|e| e.to_string())?;
            for turn in 0..turns.min(2) {
                let mut windows = self.handle.watch_seat(self.seat);
                let pending = loop {
                    let p = windows.borrow_and_update().pending.clone();
                    if !p.is_empty() {
                        break p;
                    }
                    if matches!(self.handle.status(), CampaignStatus::Finished { .. }) {
                        return Ok(());
                    }
                    windows.changed().await.map_err(|e| e.to_string())?;
                };
                let prompt = if turn == 0 {
                    self.prompts.first_turn(self.seat, "", &pending)
                } else {
                    self.prompts.window_turn(self.seat, &pending)
                };
                let outcome = driver
                    .run_turn(&prompt, TURN_LIMIT)
                    .await
                    .map_err(|e| e.to_string())?;
                if !outcome.ok {
                    return Err(outcome.error.unwrap_or_else(|| "CLI refused turn".into()));
                }
                let now = self.handle.seat(self.seat);
                if pending
                    .iter()
                    .any(|old| now.pending.iter().any(|new| new.id == old.id))
                {
                    return Err("CLI ended without answering its pending decisions".into());
                }
                self.sink.system(
                    self.seat,
                    format!(
                        "bounded demo turn {} complete; usage {:?}",
                        turn + 1,
                        outcome.usage
                    ),
                );
            }
            Ok(())
        };
        let mut result = tokio::select! {
            _ = invalidated => Err("controller binding changed; old session stopped".into()),
            result = tokio::time::timeout(WALL_LIMIT, work) =>
                result.unwrap_or_else(|_| Err("demo wall-clock budget exhausted".into())),
        };
        driver.stop().await;
        // Compare the session-bound epoch inside the writer, never with a check-then-write.
        if let Err(reason) = &result
            && !matches!(
                self.handle.status(),
                CampaignStatus::Finished { .. } | CampaignStatus::Stopped { .. }
            )
        {
            match self
                .handle
                .mark_failure_if_epoch(self.seat, epoch, reason)
                .await
            {
                Ok(()) | Err(cna_server::Error::StaleEpoch) => {}
                Err(error) => {
                    result = combine_results(
                        result,
                        Err(format!("recording seat failure failed: {error}")),
                    );
                }
            }
        }
        if matches!(self.handle.status(), CampaignStatus::Running) {
            // A final accepted order can finish while this control operation is queued.
            if let Err(error) = self.handle.pause(true).await
                && !matches!(
                    self.handle.status(),
                    CampaignStatus::Finished { .. } | CampaignStatus::Stopped { .. }
                )
            {
                result = combine_results(result, Err(format!("campaign pause failed: {error}")));
            }
        }
        let persistence = self.drain().await;
        combine_results(result, persistence)
    }

    pub fn transcript(&self) -> Vec<ServerMessage> {
        self.handle
            .replay()
            .transcripts(Perspective::Operator, self.seat, 0)
            .expect("persisted transcript")
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
    let handle = sandbox::recover(path, data).expect("recover saved campaign");
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
        Arc::new(move |r, dir| sandbox::create(dir, &data, r)),
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
