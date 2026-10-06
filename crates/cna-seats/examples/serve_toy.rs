//! Serves the toy game's MCP endpoints and prints each seat's URL. For manual CLI experiments.
use std::sync::{Arc, Mutex};

use cna_seats::game::GameBackend;
use cna_seats::mcp::{McpServer, SeatEndpoint, SharedGame, ToolRouter};
use cna_seats::memory::InMemorySeatMemory;
use cna_seats::toy::NumberDuel;

#[tokio::main]
async fn main() {
    let game = NumberDuel::new(1);
    let seats = game.seats();
    let game: SharedGame = Arc::new(Mutex::new(game));
    let memory = Arc::new(InMemorySeatMemory::new(seats.clone()));
    let router = Arc::new(ToolRouter::new(game, memory, &seats));
    let endpoints = seats
        .iter()
        .map(|s| SeatEndpoint {
            seat: s.id.clone(),
            epoch: 1,
            instructions: "Number duel seat.".into(),
        })
        .collect();
    let server = McpServer::start(router, endpoints).await.unwrap();
    for s in &seats {
        println!("{} {}", s.id, server.url(&s.id).unwrap());
    }
    tokio::signal::ctrl_c().await.ok();
}
