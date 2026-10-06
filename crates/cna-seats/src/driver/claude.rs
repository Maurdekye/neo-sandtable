//! Claude Code driver.
//!
//! One `claude -p --input-format stream-json --output-format stream-json --verbose` process per
//! seat, kept alive across turns: each turn is one JSON line on stdin and ends with a `result`
//! event on stdout. The process is confined to the seat's MCP tools:
//!
//! * `--tools ""` removes every built-in tool (no files, no shell, no web, no sub-agents);
//! * `--strict-mcp-config --mcp-config <file>` allows only the seat's own MCP endpoint;
//! * `--allowedTools mcp__cna__*` pre-approves exactly those tools and `--permission-mode dontAsk`
//!   denies anything else instead of prompting (a headless session cannot answer a prompt);
//! * `--setting-sources ""` ignores user, project and local settings, `CLAUDE.md` files, hooks;
//! * `--disable-slash-commands` drops skills;
//! * `--system-prompt` replaces the coding-assistant prompt with the seat briefing.
//!
//! Do not use `--safe-mode`: measured, it brings the built-in tools back.
//!
//! The account is chosen by `CLAUDE_CONFIG_DIR` (a profile directory holding the account's
//! login); credentials are never copied. The seat environment is scrubbed of everything else.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cna_core::ids::SeatId;
use cna_protocol::TranscriptEntry;
use serde_json::{Value, json};
use tokio::process::Command;

use super::{
    ChildProc, CliKind, DriverError, EntryEmitter, QuotaReading, SeatDriver, SessionInfo,
    StreamEvent, StreamParser, TurnOutcome, Usage, apply_seat_env, find_exe, pump_turn,
    tool_result_entry,
};
use crate::transcript::TranscriptSink;

/// Locate the real `claude` executable: on `PATH` as an `.exe`, or behind an npm `.cmd` shim
/// (`<npm>/node_modules/@anthropic-ai/claude-code/bin/claude.exe`), or via `CLAUDE_CODE_EXECPATH`.
pub fn find_claude_exe() -> Option<PathBuf> {
    if let Some(p) = find_exe("claude") {
        return Some(p);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let shim = dir.join("claude.cmd");
        if shim.is_file() {
            let real = dir.join("node_modules/@anthropic-ai/claude-code/bin/claude.exe");
            if real.is_file() {
                return Some(real);
            }
        }
    }
    std::env::var_os("CLAUDE_CODE_EXECPATH")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}

/// The MCP server name seats see; tools appear to the model as `mcp__cna__<tool>`.
pub const MCP_NAME: &str = "cna";

/// Configuration of one Claude Code seat.
#[derive(Clone, Debug)]
pub struct ClaudeConfig {
    pub seat: SeatId,
    /// Path to `claude` (an `.exe`, not a `.cmd` shim); found on `PATH` when `None`.
    pub exe: Option<PathBuf>,
    /// Model alias or id (`haiku` for tests).
    pub model: String,
    /// The account's profile directory (`CLAUDE_CONFIG_DIR`). `None` = the machine's default login.
    pub config_dir: Option<PathBuf>,
    /// If set, `claude auth status` must report this email before the seat starts.
    pub expected_email: Option<String>,
    /// Empty working directory for the session.
    pub sandbox: PathBuf,
    /// Where the per-seat MCP config file is written (outside `sandbox`).
    pub run_dir: PathBuf,
    /// The seat's MCP endpoint URL.
    pub mcp_url: String,
    /// Replaces Claude Code's default system prompt.
    pub system_prompt: String,
    pub effort: Option<String>,
}

/// Whether to start a new session with a chosen id or resume an existing one.
#[derive(Clone, Copy, Debug)]
pub enum SessionArg<'a> {
    New(&'a str),
    Resume(&'a str),
}

/// The exact argument list. Pure so tests can pin the isolation flags.
pub fn claude_args(
    cfg: &ClaudeConfig,
    mcp_config: &std::path::Path,
    session: SessionArg<'_>,
) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--tools",
        "",
        "--strict-mcp-config",
        "--allowedTools",
        "mcp__cna__*",
        "--permission-mode",
        "dontAsk",
        "--setting-sources",
        "",
        "--disable-slash-commands",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    a.push("--model".into());
    a.push(cfg.model.clone());
    a.push("--mcp-config".into());
    a.push(mcp_config.to_string_lossy().into_owned());
    a.push("--system-prompt".into());
    a.push(cfg.system_prompt.clone());
    if let Some(e) = &cfg.effort {
        a.push("--effort".into());
        a.push(e.clone());
    }
    match session {
        SessionArg::New(id) => {
            a.push("--session-id".into());
            a.push(id.into());
        }
        SessionArg::Resume(id) => {
            a.push("--resume".into());
            a.push(id.into());
        }
    }
    a
}

/// The MCP config document for a seat endpoint.
pub fn mcp_config_json(url: &str) -> Value {
    json!({ "mcpServers": { MCP_NAME: { "type": "http", "url": url } } })
}

// ---------------------------------------------------------------------------------------------
// Stream parser

/// Parses Claude Code's `stream-json` output.
#[derive(Default)]
pub struct ClaudeParser {
    init_seen: bool,
    last_text: Option<String>,
}

fn strip_prefix(name: &str) -> String {
    name.strip_prefix(&format!("mcp__{MCP_NAME}__"))
        .unwrap_or(name)
        .to_string()
}

fn u64_at(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

impl StreamParser for ClaudeParser {
    fn feed(&mut self, line: &str) -> Vec<StreamEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        match v.get("type").and_then(Value::as_str) {
            Some("system") => match v.get("subtype").and_then(Value::as_str) {
                Some("init") => {
                    // Claude repeats `init` at the start of every turn; report it once.
                    if !self.init_seen {
                        self.init_seen = true;
                        let tools = v["tools"]
                            .as_array()
                            .map(|t| {
                                t.iter()
                                    .filter_map(|x| x.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let mcp_connected = v["mcp_servers"].as_array().map(|s| {
                            s.iter()
                                .any(|m| m["name"] == MCP_NAME && m["status"] == "connected")
                        });
                        out.push(StreamEvent::Init {
                            session_id: v["session_id"].as_str().unwrap_or_default().to_string(),
                            model: v["model"].as_str().map(String::from),
                            cli_version: v["claude_code_version"].as_str().map(String::from),
                            tools,
                            mcp_connected,
                        });
                    }
                }
                Some("permission_denied") => {
                    out.push(StreamEvent::Entry(TranscriptEntry::System {
                        text: format!(
                            "tool call denied: {}",
                            strip_prefix(v["tool_name"].as_str().unwrap_or("?"))
                        ),
                    }));
                }
                _ => {}
            },
            Some("assistant") => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            let text = block["text"].as_str().unwrap_or_default();
                            if !text.trim().is_empty() {
                                self.last_text = Some(text.to_string());
                                out.push(StreamEvent::Entry(TranscriptEntry::AssistantText {
                                    text: text.to_string(),
                                }));
                            }
                        }
                        Some("thinking") => {
                            // Some models stream only a signature; show reasoning when it exists.
                            let text = block["thinking"].as_str().unwrap_or_default();
                            if !text.trim().is_empty() {
                                out.push(StreamEvent::Entry(TranscriptEntry::Reasoning {
                                    text: text.to_string(),
                                }));
                            }
                        }
                        Some("tool_use") => {
                            out.push(StreamEvent::Entry(TranscriptEntry::ToolCall {
                                call_id: block["id"].as_str().unwrap_or_default().to_string(),
                                tool: strip_prefix(block["name"].as_str().unwrap_or("?")),
                                args: block["input"].clone(),
                            }));
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] != "tool_result" {
                        continue;
                    }
                    let text = match &block["content"] {
                        Value::String(s) => s.clone(),
                        Value::Array(parts) => parts
                            .iter()
                            .filter_map(|p| p["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n"),
                        _ => String::new(),
                    };
                    out.push(StreamEvent::Entry(tool_result_entry(
                        block["tool_use_id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        block["is_error"] != true,
                        &text,
                    )));
                }
            }
            Some("rate_limit_event") => {
                if let Some(windows) = v["rate_limit_info"]["unifiedWindows"].as_object() {
                    for (name, w) in windows {
                        if let Some(u) = w["utilization"].as_f64() {
                            out.push(StreamEvent::Quota(QuotaReading {
                                window: name.clone(),
                                utilization: u,
                                resets_at_epoch_s: w["resetsAt"].as_i64(),
                            }));
                        }
                    }
                }
            }
            Some("result") => {
                let is_error = v["is_error"] == true || v["subtype"] != "success";
                let usage = &v["usage"];
                out.push(StreamEvent::TurnDone(TurnOutcome {
                    ok: !is_error,
                    error: is_error.then(|| {
                        v["result"]
                            .as_str()
                            .or(v["subtype"].as_str())
                            .unwrap_or("error")
                            .to_string()
                    }),
                    text: v["result"]
                        .as_str()
                        .map(String::from)
                        .or(self.last_text.take()),
                    usage: Usage {
                        input_tokens: u64_at(usage, "input_tokens"),
                        output_tokens: u64_at(usage, "output_tokens"),
                        cached_input_tokens: u64_at(usage, "cache_read_input_tokens"),
                        reasoning_tokens: usage["output_tokens_details"]["thinking_tokens"]
                            .as_u64(),
                        cost_usd: v["total_cost_usd"].as_f64(),
                    },
                    quota: Vec::new(),
                }));
                self.last_text = None;
            }
            _ => {}
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Driver

pub struct ClaudeDriver {
    cfg: ClaudeConfig,
    sink: TranscriptSink,
    proc: Option<ChildProc>,
    parser: ClaudeParser,
    session_id: Option<String>,
}

impl ClaudeDriver {
    pub fn new(cfg: ClaudeConfig, sink: TranscriptSink) -> Self {
        Self {
            cfg,
            sink,
            proc: None,
            parser: ClaudeParser::default(),
            session_id: None,
        }
    }

    fn exe(&self) -> Result<PathBuf, DriverError> {
        self.cfg
            .exe
            .clone()
            .or_else(find_claude_exe)
            .ok_or_else(|| DriverError::Spawn {
                cli: "claude",
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "claude not found on PATH",
                ),
            })
    }

    fn env(&self) -> Vec<(&'static str, std::ffi::OsString)> {
        match &self.cfg.config_dir {
            Some(d) => vec![("CLAUDE_CONFIG_DIR", d.clone().into_os_string())],
            None => Vec::new(),
        }
    }

    /// Check that the configured login is the expected account (`claude auth status`).
    pub async fn verify_account(&self) -> Result<String, DriverError> {
        let mut cmd = Command::new(self.exe()?);
        cmd.args(["auth", "status"])
            .current_dir(&self.cfg.sandbox)
            .kill_on_drop(true);
        cmd.stdin(std::process::Stdio::null());
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        apply_seat_env(&mut cmd, &self.env());
        let out = cmd.output().await.map_err(|e| DriverError::Spawn {
            cli: "claude",
            source: e,
        })?;
        let v: Value = serde_json::from_slice(&out.stdout)
            .map_err(|e| DriverError::Protocol(format!("claude auth status: {e}")))?;
        if v["loggedIn"] != true {
            return Err(DriverError::Isolation("claude is not logged in".into()));
        }
        let email = v["email"].as_str().unwrap_or_default().to_string();
        if let Some(want) = &self.cfg.expected_email
            && !email.eq_ignore_ascii_case(want)
        {
            return Err(DriverError::Isolation(format!(
                "claude is logged in as {email}, expected {want}"
            )));
        }
        Ok(email)
    }
}

/// Isolation policy applied to the CLI's own report of what the model can call.
pub fn check_isolation(tools: &[String], mcp_connected: Option<bool>) -> Result<(), DriverError> {
    let prefix = format!("mcp__{MCP_NAME}__");
    if let Some(t) = tools.iter().find(|t| !t.starts_with(&prefix)) {
        return Err(DriverError::Isolation(format!(
            "the session exposes a tool outside the seat's MCP tools: {t}"
        )));
    }
    if mcp_connected == Some(false) {
        return Err(DriverError::Isolation(
            "the seat's MCP server did not connect".into(),
        ));
    }
    Ok(())
}

#[async_trait]
impl SeatDriver for ClaudeDriver {
    fn kind(&self) -> CliKind {
        CliKind::Claude
    }

    async fn start(&mut self, resume_session: Option<&str>) -> Result<SessionInfo, DriverError> {
        if let Some(p) = &mut self.proc {
            p.kill().await;
        }
        self.proc = None;
        self.parser = ClaudeParser::default();
        let io = |e| DriverError::Spawn {
            cli: "claude",
            source: e,
        };
        std::fs::create_dir_all(&self.cfg.sandbox).map_err(io)?;
        std::fs::create_dir_all(&self.cfg.run_dir).map_err(io)?;
        let mcp_path = self.cfg.run_dir.join(format!("{}.mcp.json", self.cfg.seat));
        std::fs::write(&mcp_path, mcp_config_json(&self.cfg.mcp_url).to_string()).map_err(io)?;
        let email = self.verify_account().await?;

        let new_id = uuid::Uuid::new_v4().to_string();
        let (arg, id) = match resume_session {
            Some(id) => (SessionArg::Resume(id), id.to_string()),
            None => (SessionArg::New(&new_id), new_id.clone()),
        };
        let mut cmd = Command::new(self.exe()?);
        cmd.args(claude_args(&self.cfg, &mcp_path, arg))
            .current_dir(&self.cfg.sandbox);
        apply_seat_env(&mut cmd, &self.env());
        self.proc = Some(ChildProc::spawn(&mut cmd).map_err(io)?);
        self.session_id = Some(id.clone());
        self.sink.system(
            self.cfg.seat,
            format!(
                "{} session {} ({}, model {}, account {email})",
                if resume_session.is_some() {
                    "resumed"
                } else {
                    "started"
                },
                id,
                CliKind::Claude.as_str(),
                self.cfg.model
            ),
        );
        // Claude reports `init` with the first turn, so the isolation check happens in run_turn.
        Ok(SessionInfo {
            session_id: id,
            model: Some(self.cfg.model.clone()),
            cli_version: None,
            resumed: resume_session.is_some(),
        })
    }

    async fn run_turn(
        &mut self,
        prompt: &str,
        limit: Duration,
    ) -> Result<TurnOutcome, DriverError> {
        let mut emitter = EntryEmitter::new(self.cfg.seat, self.sink.clone());
        let proc = self
            .proc
            .as_mut()
            .ok_or_else(|| DriverError::Protocol("session not started".into()))?;
        let line = json!({ "type": "user", "message": { "role": "user", "content": prompt } });
        proc.send_line(&line.to_string())
            .await
            .map_err(|e| DriverError::Died(e.to_string()))?;
        let mut real_session = None;
        let outcome = pump_turn(proc, &mut self.parser, limit, |event| {
            match event {
                StreamEvent::Init {
                    tools,
                    mcp_connected,
                    session_id,
                    model,
                    cli_version,
                } => {
                    check_isolation(tools, *mcp_connected)?;
                    self.sink.system(
                        self.cfg.seat,
                        format!(
                            "CLI metadata: model {}, version {}",
                            model.as_deref().unwrap_or("unknown"),
                            cli_version.as_deref().unwrap_or("unknown"),
                        ),
                    );
                    real_session = Some(session_id.clone());
                }
                StreamEvent::Entry(entry) => emitter.emit(entry.clone()),
                _ => {}
            }
            Ok(())
        })
        .await?;
        if let Some(id) = real_session.filter(|s| !s.is_empty()) {
            self.session_id = Some(id);
        }
        Ok(outcome)
    }

    fn session_id(&self) -> Option<String> {
        self.session_id.clone()
    }

    fn is_alive(&mut self) -> bool {
        self.proc.as_mut().is_some_and(ChildProc::is_running)
    }

    async fn stop(&mut self) {
        if let Some(p) = &mut self.proc {
            p.kill().await;
        }
        self.proc = None;
    }
}

/// Convenience for supervisors that hold drivers behind a box.
pub fn boxed(cfg: ClaudeConfig, sink: TranscriptSink) -> Box<dyn SeatDriver> {
    Box::new(ClaudeDriver::new(cfg, sink))
}

#[allow(dead_code)]
fn _assert_arc(_: Arc<()>) {}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/claude_two_turns.jsonl");

    fn parse_all() -> Vec<StreamEvent> {
        let mut p = ClaudeParser::default();
        FIXTURE.lines().flat_map(|l| p.feed(l)).collect()
    }

    #[test]
    fn fixture_yields_one_init_with_only_the_seat_tools() {
        let events = parse_all();
        let inits: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Init {
                    tools,
                    mcp_connected,
                    model,
                    cli_version,
                    ..
                } => Some((
                    tools.clone(),
                    *mcp_connected,
                    model.clone(),
                    cli_version.clone(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(inits.len(), 1, "init is reported once per session");
        let (tools, connected, model, version) = &inits[0];
        assert_eq!(tools.len(), 9);
        assert!(tools.iter().all(|t| t.starts_with("mcp__cna__")));
        assert_eq!(*connected, Some(true));
        assert!(model.as_deref().unwrap().contains("haiku"));
        assert_eq!(version.as_deref(), Some("2.1.289"));
        assert!(check_isolation(tools, *connected).is_ok());
    }

    #[test]
    fn fixture_pairs_calls_with_results_in_order() {
        let entries: Vec<TranscriptEntry> = parse_all()
            .into_iter()
            .filter_map(|e| match e {
                StreamEvent::Entry(x) => Some(x),
                _ => None,
            })
            .collect();
        let calls: Vec<(&str, &str)> = entries
            .iter()
            .filter_map(|e| match e {
                TranscriptEntry::ToolCall { call_id, tool, .. } => {
                    Some((call_id.as_str(), tool.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            calls.iter().map(|c| c.1).collect::<Vec<_>>(),
            ["observe", "describe_actions", "submit"]
        );
        for (id, _) in &calls {
            let call_pos = entries
                .iter()
                .position(
                    |e| matches!(e, TranscriptEntry::ToolCall { call_id, .. } if call_id == id),
                )
                .unwrap();
            let result_pos = entries
                .iter()
                .position(
                    |e| matches!(e, TranscriptEntry::ToolResult { call_id, .. } if call_id == id),
                )
                .unwrap_or_else(|| panic!("no result for {id}"));
            assert!(result_pos > call_pos);
        }
        // Empty thinking blocks are not shown; the assistant's text is.
        assert!(
            !entries
                .iter()
                .any(|e| matches!(e, TranscriptEntry::Reasoning { .. }))
        );
        assert!(
            entries
                .iter()
                .any(|e| matches!(e, TranscriptEntry::AssistantText { text } if text == "Done."))
        );
        let TranscriptEntry::ToolResult { ok, detail, .. } = entries
            .iter()
            .find(|e| matches!(e, TranscriptEntry::ToolResult { .. }))
            .unwrap()
        else {
            unreachable!()
        };
        assert!(*ok);
        assert!(detail.as_ref().unwrap().get("your_hand").is_some());
    }

    #[test]
    fn fixture_has_two_turn_ends_with_usage_and_quota() {
        let events = parse_all();
        let turns: Vec<&TurnOutcome> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TurnDone(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(turns.len(), 2);
        assert!(turns.iter().all(|t| t.ok));
        assert!(turns[1].usage.cost_usd.unwrap() > 0.0);
        assert!(turns[1].usage.output_tokens.unwrap() > 0);
        assert_eq!(turns[1].text.as_deref(), Some("Done."));
        let quotas = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::Quota(_)))
            .count();
        assert!(quotas >= 2);
    }

    #[test]
    fn denied_tools_and_error_results_are_surfaced() {
        let mut p = ClaudeParser::default();
        let denied = p.feed(r#"{"type":"system","subtype":"permission_denied","tool_name":"mcp__cna__observe","tool_use_id":"t1"}"#);
        assert!(
            matches!(&denied[0], StreamEvent::Entry(TranscriptEntry::System { text }) if text.contains("observe"))
        );
        let failed = p.feed(r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"boom","usage":{}}"#);
        match &failed[0] {
            StreamEvent::TurnDone(t) => {
                assert!(!t.ok);
                assert_eq!(t.error.as_deref(), Some("boom"));
            }
            other => panic!("{other:?}"),
        }
        assert!(p.feed("not json").is_empty());
    }

    #[test]
    fn isolation_policy_rejects_builtin_tools() {
        let tools = vec!["mcp__cna__observe".to_string(), "Bash".to_string()];
        assert!(check_isolation(&tools, Some(true)).is_err());
        assert!(check_isolation(&["mcp__cna__observe".to_string()], Some(false)).is_err());
    }

    #[test]
    fn the_command_line_pins_every_isolation_flag() {
        let cfg = ClaudeConfig {
            seat: "axis.commander".parse().unwrap(),
            exe: None,
            model: "haiku".into(),
            config_dir: None,
            expected_email: None,
            sandbox: "/s".into(),
            run_dir: "/r".into(),
            mcp_url: "http://127.0.0.1:1/mcp/t".into(),
            system_prompt: "play".into(),
            effort: None,
        };
        let args = claude_args(
            &cfg,
            std::path::Path::new("/r/x.mcp.json"),
            SessionArg::New("id1"),
        );
        let pos = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .unwrap_or_else(|| panic!("{flag}"))
        };
        assert_eq!(args[pos("--tools") + 1], "", "no built-in tools");
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert_eq!(args[pos("--setting-sources") + 1], "");
        assert_eq!(args[pos("--permission-mode") + 1], "dontAsk");
        assert_eq!(args[pos("--allowedTools") + 1], "mcp__cna__*");
        assert!(args.contains(&"--disable-slash-commands".to_string()));
        assert!(!args.contains(&"--safe-mode".to_string()));
        assert!(!args.iter().any(|a| a.contains("dangerously")));
        assert_eq!(args[pos("--session-id") + 1], "id1");
        let resumed = claude_args(
            &cfg,
            std::path::Path::new("/r/x.mcp.json"),
            SessionArg::Resume("id1"),
        );
        assert!(resumed.contains(&"--resume".to_string()));
        assert!(!resumed.contains(&"--session-id".to_string()));
    }
}
