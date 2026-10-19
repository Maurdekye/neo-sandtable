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
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let interrupt = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = stop.send(true);
        }
    });
    // The cancelable foreground borrows the original owners. An explicit operator
    // stop ends the process, not their supervision or durable Unsettled liability.
    let campaign_stop = stopped.clone();
    let foreground = async {
        let mut result = if demo.journal.is_some() {
            demo.play_durable(&mut drivers, campaign_stop).await
        } else {
            demo.play_sessions(&mut drivers, config.max_turns).await
        };
        if let Some(journal) = &demo.journal {
            match journal.private_start_report() {
                Ok(text) => println!(
                    "Private start accounting (CLI-frame definition, not provider billing proof):\n{text}"
                ),
                Err(error) => {
                    result = cna_play::combine_results(
                        result,
                        Err(format!("reading private start accounting failed: {error}")),
                    )
                }
            }
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
        if let Err(error) = &cleanup {
            eprintln!(
                "Shutdown remains unresolved: {error}. Original launcher and drivers remain foreground-owned; no new work will start."
            );
            // Cancellation drops only this borrow; the SAME owners stay in main until
            // process exit. No signal is a settlement, healthy join or refund witness.
            let never = retain_terminal(&(&demo, &drivers, &root)).await;
            match never {}
        }
        cna_play::combine_results(result, cleanup)
    };
    let disposition = until_operator_stop(&mut stopped, foreground).await;
    interrupt.abort();
    let _ = interrupt.await;
    match disposition {
        ForegroundDisposition::Returned(result) => result.expect("bounded campaign run failed"),
        ForegroundDisposition::OperatorStop => operator_process_exit(),
    }
}

enum ForegroundDisposition<T> {
    Returned(T),
    OperatorStop,
}

async fn until_operator_stop<T>(
    stopped: &mut tokio::sync::watch::Receiver<bool>,
    foreground: impl std::future::Future<Output = T>,
) -> ForegroundDisposition<T> {
    tokio::select! {
        biased;
        _ = explicit_operator_stop(stopped) => ForegroundDisposition::OperatorStop,
        result = foreground => ForegroundDisposition::Returned(result),
    }
}

async fn explicit_operator_stop(stopped: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *stopped.borrow_and_update() {
            return;
        }
        if stopped.changed().await.is_err() {
            // Loss of the signal relay is not operator discard permission.
            std::future::pending::<()>().await;
        }
    }
}

fn operator_process_exit() -> ! {
    eprintln!(
        "Operator stop: process exiting; retained evidence remains durable Unsettled, with no healthy settlement or refund."
    );
    // Original boxes remain in the outer main stack. Process termination releases
    // memory only; do not run a guessed aggregate cleanup/ACK or journal transition.
    std::process::exit(1)
}

/// Terminal uncertainty has no dispatch permission and no detached owner. This keeps
/// the original owners in the executable's foreground; it does not retry cleanup,
/// renew allowances or assert that OS/storage settlement has a finite bound.
fn retain_terminal<T: ?Sized>(
    owners: &T,
) -> impl std::future::Future<Output = std::convert::Infallible> + '_ {
    std::future::poll_fn(move |_| {
        let _retained = &owners;
        std::task::Poll::Pending
    })
}

#[cfg(test)]
mod terminal_tests {
    use std::{
        future::Future,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Context, Poll, Waker},
    };

    struct OriginalOwner(Arc<AtomicUsize>);
    impl Drop for OriginalOwner {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn terminal_future_retains_all_original_owners_before_and_after_borrowed_polls() {
        let drops = Arc::new(AtomicUsize::new(0));
        let owners = (OriginalOwner(drops.clone()), OriginalOwner(drops.clone()));
        let mut foreground = Box::pin(super::retain_terminal(&owners));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            foreground.as_mut().poll(&mut context),
            Poll::Pending
        ));
        assert!(matches!(
            foreground.as_mut().poll(&mut context),
            Poll::Pending
        ));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        // Borrow cancellation is not terminal discard: canonical owners still live.
        drop(foreground);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(owners);
        assert_eq!(drops.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn explicit_stop_cancels_only_the_foreground_borrow_and_never_returns_success() {
        let drops = Arc::new(AtomicUsize::new(0));
        let owners = OriginalOwner(drops.clone());
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        let mut foreground = Box::pin(super::until_operator_stop(
            &mut stopped,
            super::retain_terminal(&owners),
        ));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            foreground.as_mut().poll(&mut context),
            Poll::Pending
        ));
        stop.send(true).unwrap();
        assert!(matches!(
            foreground.await,
            super::ForegroundDisposition::OperatorStop
        ));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        // No aggregate success/ACK, credit release or replacement was performed.
        drop(owners);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn closed_false_signal_is_not_terminal_discard_and_sticky_stop_is_explicit() {
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        drop(stop);
        let mut waiting = Box::pin(super::explicit_operator_stop(&mut stopped));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(waiting.as_mut().poll(&mut context), Poll::Pending));
        drop(waiting);
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        stop.send(true).unwrap();
        drop(stop);
        assert!(matches!(
            super::until_operator_stop(&mut stopped, std::future::ready(Ok::<(), ()>(()))).await,
            super::ForegroundDisposition::OperatorStop
        ));
    }

    #[test]
    fn operator_exit_child() {
        if std::env::var_os("CNA_OPERATOR_EXIT_TEST_CHILD").is_some() {
            super::operator_process_exit();
        }
    }

    #[tokio::test]
    async fn explicit_operator_process_exit_finishes_with_unsettled_nonzero_disposition() {
        // Inert test binary only: this exercises the real exit path, no seat CLI.
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "terminal_tests::operator_exit_child",
                "--nocapture",
            ])
            .env("CNA_OPERATOR_EXIT_TEST_CHILD", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let status =
            match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
                Ok(status) => status.unwrap(),
                Err(_) => {
                    child.kill().await.unwrap();
                    child.wait().await.unwrap();
                    panic!("explicit operator process exit did not finish");
                }
            };
        assert_eq!(status.code(), Some(1));
        let mut stderr = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut child.stderr.take().unwrap(), &mut stderr)
            .await
            .unwrap();
        assert!(stderr.contains("durable Unsettled"));
        assert!(stderr.contains("no healthy settlement or refund"));
    }
}
