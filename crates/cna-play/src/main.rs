//! Bounded per-seat launcher. Paid controllers require explicit opt-in.
use cna_play::{
    Demo,
    config::{Command, parse_args},
};
use cna_seats::driver::SeatDriver;
mod watched;
use std::{path::PathBuf, time::Duration};

const USAGE: &str = "cna-play [--kind sandbox|cna] [--seat SEAT=claude:MODEL|scripted:MODE|human] [--turns N] [--tool-calls N] [--long-lived --wall-seconds N --turn-timeout N --context-tokens N --recoveries N]\n  Wildcard: --seat '*=scripted:legal_random'\n  CNA: --kind cna --seat axis.commander=claude:haiku --seat '*=scripted:legal_random'\n  Preset: --preset graziani-haiku --stop-after-turn G [--stop-after-opstage S] --budget-usd USD [--seat-budget-usd SEAT=USD for EVERY seat]\n  Resume: --resume CAMPAIGN.sqlite [--stop-after-turn G --stop-after-opstage S] [--seat-budget-usd SEAT=USD for EVERY seat] (lifetime global cap/spend retained)\n  Replay: --replay CAMPAIGN.sqlite (no CLI starts)";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = parse_args(&args).unwrap_or_else(|error| panic!("{error}\n{USAGE}"));
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut updates = None;
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
        Command::ResumeControlled {
            path,
            updates: change,
        } => {
            let config = cna_play::journal::SessionJournal::config_for_campaign(&path)
                .expect("read durable config");
            updates = Some(change);
            (config, Some(path))
        }
        Command::Resume(path) => (
            cna_play::journal::SessionJournal::config_for_campaign(&path)
                .expect("read durable launch configuration"),
            Some(path),
        ),
    };
    let claude_count = config.claude_seats().count();
    watched::check_admission(&config, resume.is_some())
        .expect("watched admission refused before account access or CLI launch");
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
    let mut demo = if let Some(path) = &resume {
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
    if let Some(updates) = updates {
        let old = demo
            .config
            .run
            .as_ref()
            .expect("run controls required for explicit rebalance");
        let seats = demo
            .config
            .claude_seats()
            .map(|(s, _)| s)
            .collect::<Vec<_>>();
        let run = updates
            .apply(old, &seats)
            .expect("valid explicit operator controls");
        demo.reconfigure_run(run)
            .await
            .expect("persist controls without resume");
    }
    let config = demo.config.clone();
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
    if let Some(run) = &config.run {
        println!(
            "Run boundary: {:?}. At most two active CLI processes; forced answers use no slot.",
            run.boundary
        );
        match &run.budget {
            cna_play::budget::SpendBudget::ReportedUsd { .. } => println!(
                "Provider-reported cost limit: an estimate, can overshoot by at most about one model turn. Allocations: {:?}",
                run.budget
            ),
            cna_play::budget::SpendBudget::Tokens { .. } => println!(
                "Lifetime token allocations (native enforcement required): {:?}",
                run.budget
            ),
        }
    }
    if let Some(journal) = &demo.journal {
        println!(
            "Durable journal: {}. Resume preserves all lifetime budgets. Limits: {:?}",
            journal.file().display(),
            config.session
        );
    }
    println!("Attach viewer now; starting in 10 seconds.");
    tokio::time::sleep(Duration::from_secs(10)).await;
    let mut watched = if claude_count > 0 {
        Some(
            watched::WatchedRuntime::prepare(&config, resume.is_some())
                .await
                .expect("prepare watched run"),
        )
    } else {
        None
    };
    let mut drivers: Vec<(_, Box<dyn SeatDriver>)> = if let Some(runtime) = &watched {
        match runtime.drivers(&demo) {
            Ok(drivers) => drivers,
            Err(error) => {
                if let Some(runtime) = &mut watched {
                    let _ = runtime.shutdown().await;
                }
                let _ = demo.shutdown().await;
                panic!("watched driver preparation failed: {error}");
            }
        }
    } else {
        Vec::new()
    };
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
    if let Some(runtime) = &mut watched {
        runtime.stop_admission();
        // Native supervisors run independently of gateway HTTP graceful drain.
        // Join every driver even if play_durable already reported cleanup error.
        let (children, gateway) = tokio::join!(
            async {
                let mut cleanup = Ok(());
                for (_, driver) in &mut drivers {
                    cleanup = cna_play::combine_results(
                        cleanup,
                        driver.stop().await.map_err(|e| e.to_string()),
                    );
                }
                cleanup
            },
            runtime.shutdown()
        );
        result = cna_play::combine_results(result, children);
        match gateway {
            Ok(report) => println!(
                "Watched routed usage (numeric lower bounds; incomplete generations remain unknown): {report:?}"
            ),
            Err(error) => result = cna_play::combine_results(result, Err(error)),
        }
    }
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
