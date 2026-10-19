//! Construct the watched command without starting it or reading a login.
//! The caller still owes the immediate guard, job containment, wall supervisor
//! and owner-login handoff. Configured routing is not an OS egress boundary.

use super::{ClaudeConfig, DriverError, SessionArg, claude_args_value, mcp_config_json};
use crate::{calibration::MODEL, calibration_isolation::WatchedNamespace, driver::seat_env};
use std::{ffi::OsString, net::SocketAddr, path::Path};
use tokio::process::Command;

const PROJECT: &str = "watched-seat";

fn environment(
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    config: &Path,
    fence: SocketAddr,
) -> Result<Vec<(OsString, OsString)>, DriverError> {
    if !fence.ip().is_loopback() || fence.port() == 0 || !config.is_absolute() {
        return Err(DriverError::Isolation(
            "watched loopback/configuration required".into(),
        ));
    }
    let mut settings: Vec<(&str, OsString)> = [
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
        ("DISABLE_TELEMETRY", "1"),
        ("DISABLE_ERROR_REPORTING", "1"),
        ("DISABLE_AUTOUPDATER", "1"),
        ("DISABLE_UPDATES", "1"),
        ("CLAUDE_CODE_DISABLE_REFUSAL_FALLBACK", "1"),
        ("CLAUDE_CODE_DISABLE_REFUSAL_RETRY", "1"),
        ("CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK", "1"),
        ("CLAUDE_CODE_DISABLE_THINKING", "1"),
        ("CLAUDE_CODE_MAX_RETRIES", "0"),
        ("CLAUDE_CODE_NONSTREAMING_TIMEOUT_RETRIES", "0"),
        // Treat ONLY the typed loopback endpoint as the subscription route.
        ("CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL", "1"),
        ("CLAUDE_CODE_PROJECT_DIR_NAME", PROJECT),
        ("ANTHROPIC_MODEL", MODEL),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", MODEL),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", MODEL),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", MODEL),
        ("ANTHROPIC_SMALL_FAST_MODEL", MODEL),
    ]
    .into_iter()
    .map(|(key, value)| (key, value.into()))
    .collect();
    settings.push(("ANTHROPIC_BASE_URL", format!("http://{fence}").into()));
    for key in [
        "CLAUDE_CONFIG_DIR",
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        settings.push((key, config.as_os_str().into()));
    }
    Ok(seat_env(parent, &settings))
}

/// An explicit executable and exact model/configuration are required. The MCP
/// document is inline, so first-start configuration can remain genuinely empty.
/// Returns an UNSTARTED command; this does not authorize or contain a process.
pub fn watched_command(
    executable: &Path,
    namespace: &WatchedNamespace,
    cfg: &ClaudeConfig,
    fence: SocketAddr,
    session: SessionArg<'_>,
) -> Result<Command, DriverError> {
    let config = namespace.config(cfg.seat);
    if !executable.is_absolute()
        || cfg.exe.as_deref() != Some(executable)
        || cfg.model != MODEL
        || cfg.effort.is_some()
        || cfg.sandbox != namespace.cwd(cfg.seat)
        || cfg.config_dir.as_deref() != Some(config.as_path())
        || cfg
            .context_window
            .is_some_and(|window| window == 0 || window > 100_000)
    {
        return Err(DriverError::Isolation(
            "watched executable/model/namespace mismatch".into(),
        ));
    }
    if matches!(session, SessionArg::Resume(_)) {
        return Err(DriverError::Isolation(
            "watched resume persistence is not yet admitted".into(),
        ));
    }
    let env = environment(std::env::vars_os(), &config, fence)?;
    let mcp = mcp_config_json(&cfg.mcp_url).to_string();
    let args = claude_args_value(cfg, &mcp, session);
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(namespace.cwd(cfg.seat))
        .env_clear();
    for (key, value) in env {
        command.env(key, value);
    }
    if let Some(window) = cfg.context_window {
        command.env("CLAUDE_CODE_AUTO_COMPACT_WINDOW", window.to_string());
        command.env("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE", "80");
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn ambient_provider_operator_and_login_authority_is_removed() {
        let config = std::env::current_dir().unwrap().join("notional-config");
        let parent = [
            ("PATH", "runtime-path"),
            ("HOME", "ambient-home"),
            ("ANTHROPIC_API_KEY", "SYNTHETIC_OTHER_ACCOUNT"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "SYNTHETIC_AMBIENT_LOGIN"),
            ("ANTHROPIC_BASE_URL", "https://external.invalid"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("CNA_OPERATOR_TOKEN", "SYNTHETIC_OPERATOR"),
            ("CLAUDE_CODE_FALLBACK_MODEL", "other-model"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()));
        let env: BTreeMap<_, _> = environment(parent, &config, "127.0.0.1:1234".parse().unwrap())
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new("ANTHROPIC_BASE_URL")).unwrap(),
            "http://127.0.0.1:1234"
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("CLAUDE_CODE_MAX_RETRIES"))
                .unwrap(),
            "0"
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("HOME")).unwrap(),
            config.as_os_str()
        );
        for name in [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_USE_BEDROCK",
            "CNA_OPERATOR_TOKEN",
            "CLAUDE_CODE_FALLBACK_MODEL",
        ] {
            assert!(!env.contains_key(std::ffi::OsStr::new(name)));
        }
        for name in [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_SMALL_FAST_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
        ] {
            assert_eq!(env.get(std::ffi::OsStr::new(name)).unwrap(), MODEL);
        }
    }

    #[test]
    fn nonloopback_or_unbound_endpoint_is_refused() {
        let config = std::env::current_dir().unwrap();
        for endpoint in ["192.0.2.1:1234", "127.0.0.1:0", "0.0.0.0:1234"] {
            assert!(environment([], &config, endpoint.parse().unwrap()).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn constructed_command_has_inline_own_mcp_and_refuses_other_model_or_resume() {
        let namespace = WatchedNamespace::new(uuid::Uuid::from_u128(1));
        let seat = cna_core::ids::SeatId::all().next().unwrap();
        let executable = Path::new("C:/UNEXECUTED/claude.exe");
        let mut cfg = ClaudeConfig {
            seat,
            exe: Some(executable.into()),
            model: MODEL.into(),
            config_dir: Some(namespace.config(seat)),
            expected_email: None,
            sandbox: namespace.cwd(seat),
            run_dir: "C:/UNEXECUTED/run".into(),
            mcp_url: "http://127.0.0.1:12345/mcp/SYNTHETIC_OWN_SCOPE".into(),
            system_prompt: "Synthetic seat briefing".into(),
            effort: None,
            context_window: Some(100_000),
        };
        let address = "127.0.0.1:1234".parse().unwrap();
        let command = watched_command(
            executable,
            &namespace,
            &cfg,
            address,
            SessionArg::New("SYNTHETIC_SESSION"),
        )
        .unwrap();
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        let value = |flag| args[args.iter().position(|arg| *arg == flag).unwrap() + 1];
        assert_eq!(value("--model"), MODEL);
        assert_eq!(value("--tools"), "");
        assert_eq!(value("--setting-sources"), "");
        assert_eq!(value("--allowedTools"), "mcp__cna__*");
        let mcp: serde_json::Value = serde_json::from_str(value("--mcp-config")).unwrap();
        assert_eq!(mcp["mcpServers"].as_object().unwrap().len(), 1);
        assert_eq!(mcp["mcpServers"]["cna"]["url"], cfg.mcp_url);
        assert!(!args.contains(&"--fallback-model"));
        assert!(
            watched_command(
                executable,
                &namespace,
                &cfg,
                address,
                SessionArg::Resume("SYNTHETIC_SESSION")
            )
            .is_err()
        );
        cfg.model = "other-model".into();
        assert!(
            watched_command(
                executable,
                &namespace,
                &cfg,
                address,
                SessionArg::New("SYNTHETIC_SESSION")
            )
            .is_err()
        );
    }
}
