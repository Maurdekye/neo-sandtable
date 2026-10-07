//! Bounded per-seat launcher. Paid controllers require explicit opt-in.
use cna_play::{
    Demo,
    config::{Command, parse_args},
};
use cna_seats::{
    driver::{
        SeatDriver,
        claude::{ClaudeConfig, ClaudeDriver},
    },
    run::PromptBuilder,
};
use std::{path::PathBuf, time::Duration};

const USAGE: &str = "cna-play [--kind sandbox|cna] [--seat SEAT=claude:MODEL|scripted:MODE|human] [--turns N] [--tool-calls N] [--long-lived --wall-seconds N --turn-timeout N --context-tokens N --recoveries N]\n  Wildcard: --seat '*=scripted:legal_random'\n  CNA: --kind cna --seat axis.commander=claude:haiku --seat '*=scripted:legal_random'\n  Resume: --resume CAMPAIGN.sqlite (original lifetime budgets)\n  Replay: --replay CAMPAIGN.sqlite (no CLI starts)";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = parse_args(&args).unwrap_or_else(|error| panic!("{error}\n{USAGE}"));
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (config, resume) = match command {
        Command::Help => {
            println!("{USAGE}");
            return;
        }
        Command::Replay(path) => {
            cna_play::replay_board(&path, &repo.join("data"), &repo.join("web/dist")).await;
            return;
        }
        Command::Play(config) => (config, None),
        Command::Resume(path) => (
            cna_play::journal::SessionJournal::config_for_campaign(&path)
                .expect("read durable launch configuration"),
            Some(path),
        ),
    };
    let claude_count = config.claude_seats().count();
    let account = if claude_count > 0 {
        assert_eq!(
            std::env::var("CNA_LIVE_CLI_TESTS").as_deref(),
            Ok("1"),
            "set CNA_LIVE_CLI_TESTS=1 to authorize this bounded paid run"
        );
        let profile = std::env::var_os("CNA_CLAUDE_CONFIG_DIR")
            .expect("CNA_CLAUDE_CONFIG_DIR must point to claude-5");
        let email = std::env::var("CNA_CLAUDE_EMAIL").expect("expected claude-5 email required");
        Some((PathBuf::from(profile), email))
    } else {
        None
    };
    let root = if let Some(path) = &resume {
        path.parent()
            .and_then(|p| p.parent())
            .expect("campaign needs run root")
            .to_path_buf()
    } else {
        tempfile::Builder::new()
            .prefix("cna-server-seat-")
            .tempdir()
            .expect("fresh campaign directory")
            .keep()
    };
    let demo = if let Some(path) = &resume {
        Demo::resume(path, &repo.join("data"), &repo.join("web/dist")).await
    } else {
        Demo::with_config(
            &root.join("campaigns"),
            &repo.join("data"),
            &repo.join("web/dist"),
            config.clone(),
        )
        .await
    }
    .expect("create or recover campaign");
    println!("Board: {}", demo.board_url());
    for (seat, url) in demo.human_console_urls() {
        println!("Human console ({seat}): {url}");
    }
    println!("Campaign directory: {}", root.join("campaigns").display());
    println!(
        "Profile: {}; budget: {} Claude seats, {} turns and {} tools each. Account: claude-5.",
        config.profile(),
        claude_count,
        config.max_turns,
        config.tool_calls
    );
    if let Some(journal) = &demo.journal {
        println!(
            "Durable journal: {}. Resume preserves all lifetime budgets. Limits: {:?}",
            journal.file().display(),
            config.session
        );
    }
    println!("Attach viewer now; starting in 10 seconds.");
    tokio::time::sleep(Duration::from_secs(10)).await;
    let mut drivers: Vec<_> = config
        .claude_seats()
        .map(|(seat, model)| {
            let (profile, email) = account.as_ref().unwrap();
            let driver = ClaudeDriver::new(
                ClaudeConfig {
                    seat,
                    exe: None,
                    model: model.into(),
                    config_dir: Some(profile.clone()),
                    expected_email: Some(email.clone()),
                    sandbox: root.join("sandbox").join(seat.to_string()),
                    run_dir: root.join("run").join(seat.to_string()),
                    mcp_url: demo.mcp.url(seat).unwrap(),
                    system_prompt: demo.prompts.system_prompt(seat),
                    effort: None,
                    context_window: config.session.as_ref().map(|s| s.context_tokens),
                },
                demo.sink.clone(),
            );
            (seat, Box::new(driver) as Box<dyn SeatDriver>)
        })
        .collect();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let interrupt = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = stop.send(true);
        }
    });
    let mut result = if demo.journal.is_some() {
        demo.play_durable(&mut drivers, stopped).await
    } else {
        demo.play_sessions(&mut drivers, config.max_turns).await
    };
    interrupt.abort();
    let _ = interrupt.await;
    if let Some(journal) = &demo.journal {
        match journal
            .snapshot()
            .and_then(|s| serde_json::to_string_pretty(&s).map_err(|e| e.to_string()))
        {
            Ok(text) => println!("Session accounting: {text}"),
            Err(error) => {
                result = cna_play::combine_results(
                    result,
                    Err(format!("reading session accounting failed: {error}")),
                )
            }
        }
    }
    // Export every CLI seat, or the commander for a scripted-only demonstration.
    let seats: Vec<_> = if claude_count == 0 {
        vec![demo.seat]
    } else {
        config.claude_seats().map(|(seat, _)| seat).collect()
    };
    for seat in seats {
        let saved = async {
            let transcript = demo.transcript_for(seat)?;
            let text = transcript
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
                .join("\n")
                + "\n";
            let path = root.join(format!("{seat}.transcript.jsonl"));
            tokio::fs::write(&path, text)
                .await
                .map_err(|e| e.to_string())?;
            println!("Transcript: {}", path.display());
            Ok(())
        }
        .await;
        result = cna_play::combine_results(result, saved);
    }
    println!("Result: {result:?}. Campaign viewer remains available for 20 seconds.");
    tokio::time::sleep(Duration::from_secs(20)).await;
    let cleanup = demo.shutdown().await;
    cna_play::combine_results(result, cleanup).expect("bounded campaign run failed");
}
