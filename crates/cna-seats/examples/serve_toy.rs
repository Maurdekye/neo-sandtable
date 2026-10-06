//! Serves the toy game's MCP endpoints and prints each seat's URL. For manual CLI experiments.
use std::sync::Arc;

use cna_seats::mcp::{McpServer, SeatEndpoint, SharedGame, ToolRouter};
use cna_seats::memory::InMemorySeatMemory;
use cna_seats::toy::{AXIS, COMMONWEALTH, NumberDuel};

#[tokio::main]
async fn main() {
    let seats = vec![AXIS, COMMONWEALTH];
    let game: SharedGame = Arc::new(NumberDuel::game(1));
    let memory = Arc::new(InMemorySeatMemory::new(seats.clone()));
    let router = Arc::new(ToolRouter::new(game, memory, &seats));
    let endpoints = seats
        .iter()
        .map(|s| SeatEndpoint {
            seat: *s,
            epoch: 1,
            instructions: "Number duel seat.".into(),
        })
        .collect();
    let server = McpServer::start(router, endpoints).await.unwrap();
    for s in &seats {
        println!("{s} {}", server.url(*s).unwrap());
    }
    tokio::signal::ctrl_c().await.ok();
}
