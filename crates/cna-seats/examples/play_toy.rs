//! Plays the toy game with live CLIs. Costs real quota: use the cheapest model.
//!
//! `cargo run -p cna-seats --example play_toy -- claude [model]`
//!
//! Environment: `CNA_CLAUDE_CONFIG_DIR` (the account's profile dir), `CNA_CLAUDE_EMAIL` (expected
//! login). Prints each seat's transcript as it arrives.
use std::path::PathBuf;

use cna_seats::demo::play_toy_game;
use cna_seats::driver::claude::{ClaudeConfig, ClaudeDriver};
use cna_seats::run::RunLimits;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cli = args.get(1).map_or("claude", String::as_str);
    let model = args.get(2).cloned().unwrap_or_else(|| "haiku".into());
    let root = std::env::temp_dir().join(format!("cna-seats-{}", std::process::id()));
    assert_eq!(cli, "claude", "only claude is wired in this example so far");
    let report = play_toy_game(7, RunLimits::default(), |seat, url, sink, system| {
        let cfg = ClaudeConfig {
            seat,
            exe: None,
            model: model.clone(),
            config_dir: std::env::var_os("CNA_CLAUDE_CONFIG_DIR").map(PathBuf::from),
            expected_email: std::env::var("CNA_CLAUDE_EMAIL").ok(),
            sandbox: root.join("sandbox").join(seat.to_string()),
            run_dir: root.join("run"),
            mcp_url: url,
            system_prompt: system,
            effort: None,
        };
        Box::new(ClaudeDriver::new(cfg, sink))
    })
    .await;
    for r in &report.transcript {
        println!("{} #{} {:?}", r.seat, r.tseq, r.entry);
    }
    println!("ends: {:?}", report.ends);
    println!("outcome: {:?}", report.outcome);
    println!("tool calls: {:?}", report.tool_calls);
}
