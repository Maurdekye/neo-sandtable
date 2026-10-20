//! Original ONE-generation calibration. This command is separate from setup/GT1.
const USAGE: &str = "cna-calibrate-one --operator-intent calibration-one --config ABSOLUTE_OPERATOR_JSON\nRequires separately authorized current calibration go, CNA_LIVE_CLI_TESTS=1 and on-call-day explicit prices. No resume or full-turn mode.";

fn arguments(
    args: &[std::ffi::OsString],
) -> Result<
    (
        std::path::PathBuf,
        cna_seats::calibration_run::CalibrationIntent,
    ),
    &'static str,
> {
    if args.len() != 4 || args[0] != "--operator-intent" || args[2] != "--config" {
        return Err(USAGE);
    }
    let intent = cna_seats::calibration_run::CalibrationIntent::parse(Some(&args[1]))?;
    let path = std::path::PathBuf::from(&args[3]);
    if !path.is_absolute() {
        return Err("absolute trusted operator configuration required");
    }
    Ok((path, intent))
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--help" {
        println!("{USAGE}");
        return;
    }
    let result = match arguments(&args) {
        Ok((path, intent)) => run(path, intent).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
async fn run(
    _: std::path::PathBuf,
    _: cna_seats::calibration_run::CalibrationIntent,
) -> Result<(), &'static str> {
    Err("Windows job containment required")
}

#[cfg(windows)]
async fn run(
    path: std::path::PathBuf,
    intent: cna_seats::calibration_run::CalibrationIntent,
) -> Result<(), &'static str> {
    use cna_play::{
        Demo,
        config::{GameKind, LaunchConfig},
    };
    use cna_seats::{
        calibration::MODEL,
        calibration_run::{CalibrationConfig, CalibrationRuntime},
    };
    use std::{io::Read, path::PathBuf};
    if std::env::var("CNA_LIVE_CLI_TESTS").as_deref() != Ok("1") {
        return Err("separate current calibration go and live opt-in required");
    }
    // Configuration contains paths/prompt/prices, never the owner token itself.
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| "operator configuration unavailable")?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "operator configuration unreadable")?;
    if bytes.len() > 1024 * 1024 {
        return Err("operator configuration exceeds bound");
    }
    let config: CalibrationConfig = serde_json::from_slice(&bytes)
        .map_err(|_| "explicit calibration configuration required")?;
    config.validate_live_launch()?;
    let launch = LaunchConfig::resolve(
        GameKind::Cna,
        &["*=human".into(), format!("{}=claude:{MODEL}", config.seat)],
    )
    .map_err(|_| "calibration seat binding refused")?;
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let campaigns = config
        .ledger
        .parent()
        .ok_or("private ledger parent required")?
        .join(format!("calibration-campaign-{}", uuid::Uuid::new_v4()));
    let demo = Demo::with_config(
        &campaigns,
        &repo.join("data"),
        &repo.join("web/dist"),
        launch,
    )
    .await
    .map_err(|_| "calibration game/MCP setup failed")?;
    // The parent derives this endpoint from its own canonical SeatId. No operator
    // URL/capability or other seat endpoint is accepted by the command input.
    let own_mcp = match demo.mcp.url(config.seat) {
        Some(url) => url,
        None => {
            let _ = demo.shutdown().await;
            return Err("own calibration MCP unavailable");
        }
    };
    let mut runtime = match CalibrationRuntime::prepare(&config, intent, own_mcp).await {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = demo.shutdown().await;
            return Err(error);
        }
    };
    let outcome = tokio::select! {
        result = runtime.turn(&config.prompt) => result,
        _ = tokio::signal::ctrl_c() => Err("calibration cancelled; cleanup required"),
    };
    let cleanup = runtime.shutdown().await;
    let report = runtime.report();
    let game_cleanup = demo.shutdown().await;
    let (joined, report) = report?;
    println!(
        "{}",
        serde_json::json!({"cleanup_confirmed":joined,"game_cleanup_confirmed":game_cleanup.is_ok(),"model_window_complete":outcome.is_ok(),"routed":report})
    );
    cleanup?;
    game_cleanup.map_err(|_| "calibration game/MCP cleanup failed")?;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_requires_separate_exact_intent_and_configuration() {
        let args = |s: &[&str]| s.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        for mode in ["setup-gt1", "calibration-one ", ""] {
            assert!(
                arguments(&args(&[
                    "--operator-intent",
                    mode,
                    "--config",
                    "C:/UNREAD/config.json"
                ]))
                .is_err()
            );
        }
        assert!(
            arguments(&args(&[
                "--operator-intent",
                "calibration-one",
                "--config",
                "relative.json"
            ]))
            .is_err()
        );
        assert!(arguments(&[]).is_err());
    }
}
