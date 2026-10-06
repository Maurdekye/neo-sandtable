//! One Haiku seat, nine scripted seats, at most two turns. Real quota; opt-in required.
use cna_play::Demo;
use cna_seats::{
    driver::claude::{ClaudeConfig, ClaudeDriver},
    run::PromptBuilder,
};
use std::{path::PathBuf, time::Duration};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--replay") {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let path = PathBuf::from(args.get(1).expect("--replay requires SQLite path"));
        cna_play::replay_board(&path, &repo.join("data"), &repo.join("web/dist")).await;
        return;
    }
    assert!(args.is_empty(), "usage: cna-play [--replay <SQLite path>]");
    assert_eq!(
        std::env::var("CNA_LIVE_CLI_TESTS").as_deref(),
        Ok("1"),
        "set CNA_LIVE_CLI_TESTS=1 to authorize this bounded paid demo"
    );
    let profile = std::env::var_os("CNA_CLAUDE_CONFIG_DIR")
        .expect("CNA_CLAUDE_CONFIG_DIR must point to claude-5");
    let email = std::env::var("CNA_CLAUDE_EMAIL").expect("expected claude-5 email required");
    let root = tempfile::Builder::new()
        .prefix("cna-server-seat-")
        .tempdir()
        .expect("fresh campaign directory")
        .keep();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let demo = Demo::new(
        &root.join("campaigns"),
        &repo.join("data"),
        &repo.join("web/dist"),
    )
    .await;
    println!("Board: {}", demo.board_url());
    println!(
        "Budget: one haiku seat on claude-5, two turns, 40 tools, 150 seconds; nine scripted seats."
    );
    println!("Attach viewer now; starting in 10 seconds.");
    tokio::time::sleep(Duration::from_secs(10)).await;
    let mut driver = ClaudeDriver::new(
        ClaudeConfig {
            seat: demo.seat,
            exe: None,
            model: "haiku".into(),
            config_dir: Some(profile.into()),
            expected_email: Some(email),
            sandbox: root.join("sandbox"),
            run_dir: root.join("run"),
            mcp_url: demo.mcp.url(demo.seat).unwrap(),
            system_prompt: demo.prompts.system_prompt(demo.seat),
            effort: None,
        },
        demo.sink.clone(),
    );
    let result = demo.play(&mut driver, 2).await;
    let transcript = demo.transcript();
    let fixture = root.join("claude_sandbox_transcript.jsonl");
    let text = transcript
        .iter()
        .map(|m| serde_json::to_string(m).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&fixture, text).unwrap();
    println!("Result: {result:?}; fixture: {}", fixture.display());
    println!("Campaign viewer remains available for 20 seconds.");
    tokio::time::sleep(Duration::from_secs(20)).await;
    let cleanup = demo.shutdown().await;
    cna_play::combine_results(result, cleanup).expect("live seat demo failed");
}
