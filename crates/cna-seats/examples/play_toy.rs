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
    // Keep the ORIGINAL run future in this stack through explicit process exit.
    // Selecting a stop cancels only its borrow, never drops its retained runners,
    // child/readers, listener or task handles while the process continues.
    let mut original_run = std::pin::pin!(play_toy_game(
        7,
        RunLimits::default(),
        |seat, url, sink, system| {
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
                context_window: None,
            };
            Box::new(ClaudeDriver::new(cfg, sink))
        }
    ));
    let mut operator_stop = std::pin::pin!(tokio::signal::ctrl_c());
    let report = match until_operator_exit(operator_stop.as_mut(), original_run.as_mut()).await {
        RunDisposition::Returned(report) => report,
        RunDisposition::Stop(signal) => terminal_process_exit(signal),
    };
    for r in &report.transcript {
        println!("{} #{} {:?}", r.seat, r.tseq, r.entry);
    }
    println!("ends: {:?}", report.ends);
    println!("outcome: {:?}", report.outcome);
    println!("tool calls: {:?}", report.tool_calls);
    if report.has_retained_owners() {
        eprintln!(
            "{} original runners remain Unsettled and foreground-owned; no new work will start. Ctrl-C exits without healthy settlement or refund.",
            report.retained_runners.len()
        );
        match until_operator_exit(operator_stop.as_mut(), retain_terminal(&report)).await {
            RunDisposition::Stop(signal) => terminal_process_exit(signal),
            RunDisposition::Returned(never) => match never {},
        }
    }
}

enum RunDisposition<T> {
    Returned(T),
    Stop(std::io::Result<()>),
}

async fn until_operator_exit<T>(
    signal: impl std::future::Future<Output = std::io::Result<()>>,
    foreground: impl std::future::Future<Output = T>,
) -> RunDisposition<T> {
    tokio::select! {
        biased;
        result = signal => RunDisposition::Stop(result),
        result = foreground => RunDisposition::Returned(result),
    }
}

fn retain_terminal<T: ?Sized>(
    original: &T,
) -> impl std::future::Future<Output = std::convert::Infallible> + '_ {
    std::future::poll_fn(move |_| {
        let _retained = original;
        std::task::Poll::Pending
    })
}

fn terminal_process_exit(signal: std::io::Result<()>) -> ! {
    // Operator stop is 130; a failed stop listener is a terminal infrastructure
    // failure (2), never an operator request or successful foreground result.
    // No I/O, await, destructor cleanup, guessed joins, settlement or refund here.
    // ORIGINAL run/report owners remain on the main stack until process termination.
    std::process::exit(if signal.is_ok() { 130 } else { 2 })
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

    struct OriginalOwner {
        drops: Arc<AtomicUsize>,
        raw: String,
    }
    impl Drop for OriginalOwner {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    // SOURCE ONLY/UNRUN: synthetic stop futures, no OS listener, child or model.
    #[tokio::test]
    async fn explicit_stop_cancels_only_borrow_of_original_pending_run() {
        let drops = Arc::new(AtomicUsize::new(0));
        let owners = OriginalOwner {
            drops: drops.clone(),
            raw: "canonical failure".into(),
        };
        let pointer = owners.raw.as_ptr();
        let (stop, requested) = tokio::sync::oneshot::channel();
        let signal = async { requested.await.map_err(std::io::Error::other) };
        let mut original = Box::pin(super::retain_terminal(&owners));
        let mut borrowed = Box::pin(super::until_operator_exit(signal, original.as_mut()));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            borrowed.as_mut().poll(&mut context),
            Poll::Pending
        ));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        stop.send(()).unwrap();
        assert!(matches!(
            borrowed.await,
            super::RunDisposition::Stop(Ok(()))
        ));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert_eq!(owners.raw.as_ptr(), pointer);
        // Borrow cancellation/stop does not consume evidence or settle the original.
        assert!(matches!(
            original.as_mut().poll(&mut context),
            Poll::Pending
        ));
        drop(original);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(owners);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn terminal_report_stop_and_listener_failure_cannot_return_healthy_foreground() {
        let drops = Arc::new(AtomicUsize::new(0));
        let report = OriginalOwner {
            drops: drops.clone(),
            raw: "retained report".into(),
        };
        let pointer = report.raw.as_ptr();
        assert!(matches!(
            super::until_operator_exit(std::future::ready(Ok(())), super::retain_terminal(&report))
                .await,
            super::RunDisposition::Stop(Ok(()))
        ));
        let error = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let disposition = super::until_operator_exit(
            std::future::ready(Err(error)),
            std::future::ready("otherwise ready"),
        )
        .await;
        assert!(
            matches!(disposition, super::RunDisposition::Stop(Err(ref error))
            if error.kind() == std::io::ErrorKind::PermissionDenied)
        );
        // Stop priority is explicit even if the foreground also became ready.
        assert!(matches!(
            super::until_operator_exit(
                std::future::ready(Ok(())),
                std::future::ready("otherwise ready")
            )
            .await,
            super::RunDisposition::Stop(Ok(()))
        ));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert_eq!(report.raw.as_ptr(), pointer);
        drop(report);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn operator_exit_probe_child() {
        let Some(mode) = std::env::var_os("CNA_TOY_EXIT_PROBE") else {
            return;
        };
        struct DiscardTripwire;
        impl Drop for DiscardTripwire {
            fn drop(&mut self) {
                // Ordinary unwinding/discard must not impersonate process termination.
                std::process::exit(99);
            }
        }
        let _original = DiscardTripwire;
        let signal = if mode == "stop" {
            Ok(())
        } else {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        };
        super::terminal_process_exit(signal)
    }

    // SOURCE ONLY/UNRUN: an actual bounded inert test-binary process control,
    // never the live CLI example, auth, provider, filesystem archive or game run.
    #[tokio::test]
    async fn actual_terminal_exit_finishes_without_owner_destructor_or_healthy_exit() {
        for (mode, expected) in [("stop", 130), ("listener-error", 2)] {
            let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
            command.env_clear();
            if let Some(system_root) = std::env::var_os("SystemRoot") {
                command.env("SystemRoot", system_root);
            }
            command
                .args([
                    "--exact",
                    "terminal_tests::operator_exit_probe_child",
                    "--nocapture",
                ])
                .env("CNA_TOY_EXIT_PROBE", mode)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let mut original = command.spawn().unwrap();
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(5), original.wait()).await;
            let status = match result {
                Ok(status) => status.unwrap(),
                Err(_) => {
                    original
                        .kill()
                        .await
                        .expect("actual original process kill and join");
                    panic!("terminal exit did not finish within the inert control bound");
                }
            };
            assert_eq!(status.code(), Some(expected));
            assert!(!status.success());
        }
    }
}
