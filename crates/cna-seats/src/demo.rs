//! A complete toy game run: MCP server, two seat runners, local transcript store. Used by the
//! `play_toy` example and the opt-in live CLI tests.

use std::sync::Arc;

use cna_core::ids::SeatId;
use serde_json::Value;
use tokio::sync::{Semaphore, watch};
use tokio::time::Instant;

use crate::driver::SeatDriver;
use crate::game::GameBackend;
use crate::mcp::{McpServer, SeatEndpoint, ToolRouter};
use crate::memory::{InMemorySeatMemory, SeatMemory};
use crate::run::{
    DefaultPrompts, InMemorySessions, PromptBuilder, RunLimits, SeatEnd, SeatRunner, SeatState,
};
use crate::toy::{AXIS, COMMONWEALTH, NumberDuel};
use crate::transcript::{LocalTranscript, StoredEntry, TranscriptSink};

/// What a toy run produced.
pub struct DemoReport {
    pub ends: Vec<(SeatId, SeatEnd)>,
    pub transcript: Vec<StoredEntry>,
    pub outcome: Option<Value>,
    pub tool_calls: Vec<(SeatId, u64)>,
    pub game: Arc<dyn GameBackend>,
    pub router: Arc<ToolRouter>,
}

pub const TOY_DESCRIPTION: &str = "The game is a two-player card duel (Number Duel): each seat holds a \
private hand of five distinct cards from 1 to 9 and secretly plays one per round for five rounds; the \
higher card wins the round. You cannot see the opponent's hand or their pending choice. Win more \
rounds than they do.";

/// Play the toy game to the end. `make_driver(seat, mcp_url, sink)` builds each seat's driver.
pub async fn play_toy_game(
    seed: u64,
    limits: RunLimits,
    make_driver: impl Fn(SeatId, String, TranscriptSink, String) -> Box<dyn SeatDriver>,
) -> DemoReport {
    let seats = vec![AXIS, COMMONWEALTH];
    let game: Arc<dyn GameBackend> = Arc::new(NumberDuel::game(seed));
    let memory: Arc<dyn SeatMemory> = Arc::new(InMemorySeatMemory::new(seats.clone()));
    let router = Arc::new(ToolRouter::new(game.clone(), memory.clone(), &seats));
    let prompts = Arc::new(DefaultPrompts {
        game_description: TOY_DESCRIPTION.into(),
    });
    let endpoints = seats
        .iter()
        .map(|s| SeatEndpoint {
            seat: *s,
            epoch: 1,
            instructions: prompts.system_prompt(*s),
        })
        .collect();
    let server = McpServer::start(router.clone(), endpoints)
        .await
        .expect("mcp server");

    let g = game.clone();
    let seq: Arc<dyn Fn() -> u64 + Send + Sync> = Arc::new(move || {
        // The store is called from async context but only needs a best-effort sequence.
        futures_lite_block(g.game_seq())
    });
    let store = LocalTranscript::new(seq);
    let sink = TranscriptSink::new(store.clone());
    let sessions = Arc::new(InMemorySessions::default());
    let permits = Arc::new(Semaphore::new(limits.max_concurrent_sessions));
    let started = Instant::now();
    let (_stop_tx, stop_rx) = watch::channel(false);

    let mut tasks = Vec::new();
    for seat in &seats {
        let (state, _) = watch::channel(SeatState {
            status: cna_protocol::SeatStatus::Idle,
            reason: None,
        });
        let url = server.url(*seat).expect("endpoint");
        let mut runner = SeatRunner {
            seat: *seat,
            driver: make_driver(*seat, url, sink.clone(), prompts.system_prompt(*seat)),
            game: game.clone(),
            memory: memory.clone(),
            router: router.clone(),
            sink: sink.clone(),
            sessions: sessions.clone(),
            prompts: prompts.clone(),
            limits: limits.clone(),
            permits: permits.clone(),
            started,
            state,
            stop: stop_rx.clone(),
        };
        let seat = *seat;
        tasks.push(tokio::spawn(async move { (seat, runner.run().await) }));
    }
    let mut ends = Vec::new();
    for t in tasks {
        ends.push(t.await.expect("runner task"));
    }
    sink.flush().await;
    DemoReport {
        ends,
        transcript: store.log(),
        outcome: game.outcome().await,
        tool_calls: seats
            .iter()
            .map(|s| {
                (
                    *s,
                    router
                        .counters(*s)
                        .map_or(0, |c| c.calls.load(std::sync::atomic::Ordering::SeqCst)),
                )
            })
            .collect(),
        game,
        router,
    }
}

/// Resolve a future that is known to complete without waiting (the toy game's `game_seq`).
fn futures_lite_block<T>(fut: impl std::future::Future<Output = T>) -> T {
    use std::task::{Context, Poll, Waker};
    let mut fut = std::pin::pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("future was expected to complete immediately"),
    }
}
