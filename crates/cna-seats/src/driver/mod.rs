//! Session drivers: one long-lived CLI session per seat.
//!
//! A [`SeatDriver`] owns one seat's CLI process (or its resumable session) and turns the CLI's
//! output into transcript entries. The shared plumbing lives here: the child-process wrapper, the
//! scrubbed environment every seat runs with, and the event vocabulary the per-CLI stream parsers
//! produce. Parsers are pure (`line -> events`) so they are tested against recorded fixtures.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cna_core::ids::SeatId;
use cna_protocol::TranscriptEntry;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;
use tokio::time::Instant;

pub mod antigravity;
pub mod claude;
pub mod codex;

/// Which CLI drives a seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliKind {
    Claude,
    Codex,
    Antigravity,
}

impl CliKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CliKind::Claude => "claude-code",
            CliKind::Codex => "codex",
            CliKind::Antigravity => "antigravity",
        }
    }
}

/// Token and cost accounting for one turn, where the CLI reports it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    /// CLI estimate; Claude streaming reports cumulative session cost.
    pub cost_usd: Option<f64>,
    /// Whether every reported model price basis is list or managed (not unknown/missing).
    #[serde(default)]
    pub cost_basis_known: Option<bool>,
}

/// A quota reading the CLI volunteered (Claude Code emits these on every turn).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuotaReading {
    pub window: String,
    /// 0.0..=1.0
    pub utilization: f64,
    pub resets_at_epoch_s: Option<i64>,
}

/// How a turn ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnOutcome {
    pub ok: bool,
    /// The CLI's own error text when `ok` is false.
    pub error: Option<String>,
    /// The final assistant text of the turn.
    pub text: Option<String>,
    pub usage: Usage,
    pub quota: Vec<QuotaReading>,
}

/// What a stream parser extracts from one line of CLI output.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    /// The session is up. `tools` is the CLI's own list of tools the model can call; the driver
    /// checks it against the isolation policy.
    Init {
        session_id: String,
        model: Option<String>,
        cli_version: Option<String>,
        tools: Vec<String>,
        mcp_connected: Option<bool>,
    },
    Entry(TranscriptEntry),
    Context(u64),
    Compacted {
        trigger: String,
        pre_tokens: Option<u64>,
    },
    Quota(QuotaReading),
    TurnDone(TurnOutcome),
}

/// Pure line-by-line parser for one CLI's stream.
pub trait StreamParser: Send {
    fn feed(&mut self, line: &str) -> Vec<StreamEvent>;
}

/// What can go wrong driving a CLI.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("could not start {cli}: {source}")]
    Spawn {
        cli: &'static str,
        source: std::io::Error,
    },
    #[error("the CLI process ended unexpectedly: {0}")]
    Died(String),
    #[error("the turn exceeded its time limit")]
    Timeout,
    #[error("the CLI refused the turn: {0}")]
    Cli(String),
    #[error("isolation check failed: {0}")]
    Isolation(String),
    #[error("protocol error: {0}")]
    Protocol(String),
}

/// Information about a started session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub model: Option<String>,
    pub cli_version: Option<String>,
    /// `true` when an existing session was resumed rather than started fresh.
    pub resumed: bool,
}

/// Driver-reported context occupancy and compaction events, never inferred from
/// aggregate turn token usage. Counts restart at zero for each process.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionTelemetry {
    pub model: Option<String>,
    pub cli_version: Option<String>,
    pub tools: Vec<String>,
    pub context_tokens: Option<u64>,
    pub compactions: u64,
}
/// Verified upper bound on one request's client-side estimate, never a billing claim.
#[derive(Clone, Debug)]
pub struct EstimateBound {
    pub pinned_model: String,
    pub context_tokens: u64,
    pub output_tokens: u64,
    pub max_input_or_cache_write_usd_per_token: f64,
    pub output_usd_per_token: f64,
    pub evidence: String,
}
impl EstimateBound {
    #[allow(clippy::float_arithmetic)]
    pub fn ceiling_usd(&self) -> Result<f64, DriverError> {
        if self.pinned_model.is_empty()
            || self.evidence.is_empty()
            || self.context_tokens == 0
            || self.output_tokens == 0
            || !self.max_input_or_cache_write_usd_per_token.is_finite()
            || self.max_input_or_cache_write_usd_per_token <= 0.
            || !self.output_usd_per_token.is_finite()
            || self.output_usd_per_token <= 0.
        {
            return Err(DriverError::Isolation("unproved estimate bound".into()));
        }
        let ceiling = self.context_tokens as f64 * self.max_input_or_cache_write_usd_per_token
            + self.output_tokens as f64 * self.output_usd_per_token;
        if !ceiling.is_finite() || ceiling <= 0. {
            return Err(DriverError::Isolation("invalid estimate ceiling".into()));
        }
        Ok(ceiling)
    }
}
/// One seat's CLI session.
#[async_trait]
pub trait SeatDriver: Send + Sync {
    fn kind(&self) -> CliKind;
    fn telemetry(&self) -> SessionTelemetry {
        SessionTelemetry::default()
    }
    /// None means real admission must fail closed; fake drivers may inject explicit bounds.
    fn estimate_bound(&self) -> Option<EstimateBound> {
        None
    }
    /// Apply only this seat's remaining provider-reported cost allocation before spawn/resume.
    /// Drivers without a verified provider enforcement mechanism fail closed.
    fn set_reported_cost_limit(&mut self, _remaining_usd: f64) -> Result<(), DriverError> {
        Err(DriverError::Isolation(
            "driver cannot enforce reported-cost limit".into(),
        ))
    }
    /// Token-budget fallback requires native enforcement, not a guessed USD conversion.
    fn set_token_limit(&mut self, _remaining: u64) -> Result<(), DriverError> {
        Err(DriverError::Isolation(
            "driver cannot enforce token limit".into(),
        ))
    }
    /// Start (or resume, if `resume_session` is given) the CLI session.
    async fn start(&mut self, resume_session: Option<&str>) -> Result<SessionInfo, DriverError>;
    /// Send one user turn and stream the CLI's work into the transcript until the turn ends.
    async fn run_turn(&mut self, prompt: &str, limit: Duration)
    -> Result<TurnOutcome, DriverError>;
    /// The CLI's session id, once known. Persist it to resume after a crash.
    fn session_id(&self) -> Option<String>;
    /// Is the CLI process still alive?
    fn is_alive(&mut self) -> bool;
    /// Stop the CLI process, leaving the session resumable.
    async fn stop(&mut self);
}

// ---------------------------------------------------------------------------------------------
// Child process plumbing

/// A child process with line-oriented stdout, writable stdin and a stderr tail for diagnostics.
pub struct ChildProc {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl ChildProc {
    pub fn spawn(cmd: &mut Command) -> std::io::Result<Self> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let (tx, lines) = mpsc::channel(1024);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut buf = String::new();
            loop {
                buf.clear();
                match reader.read_line(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let line = buf.trim_end_matches(['\r', '\n']).to_string();
                        if !line.is_empty() && tx.send(line).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text).await;
            let mut g = tail.lock().expect("stderr tail");
            for l in text.lines() {
                g.push_back(l.to_string());
                if g.len() > 40 {
                    g.pop_front();
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            lines,
            stderr_tail,
        })
    }

    /// Close input and give an idle CLI time to persist resumable session totals.
    /// Active/canceled turns use kill instead; this grace is bounded.
    pub async fn finish(&mut self, grace: Duration) {
        self.close_stdin();
        if !matches!(
            tokio::time::timeout(grace, self.child.wait()).await,
            Ok(Ok(_))
        ) {
            self.kill().await;
        }
    }
    pub async fn send_line(&mut self, line: &str) -> std::io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::other("stdin closed"))?;
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await
    }

    /// Close stdin (EOF), which ends a CLI that reads its prompts from it.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Next stdout line; `Ok(None)` at EOF; `Err` when `deadline` passes first.
    pub async fn next_line(
        &mut self,
        deadline: Instant,
    ) -> Result<Option<String>, tokio::time::error::Elapsed> {
        match tokio::time::timeout_at(deadline, self.lines.recv()).await {
            Ok(line) => Ok(line),
            Err(e) => Err(e),
        }
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn stderr_tail(&self) -> String {
        let g = self.stderr_tail.lock().expect("stderr tail");
        g.iter().cloned().collect::<Vec<_>>().join("\n")
    }

    pub async fn kill(&mut self) {
        self.stdin = None;
        let _ = self.child.kill().await;
    }
}

/// Only OS/runtime variables may be inherited by a seat process. Credential names are
/// not predictable, so a prefix deny-list cannot prove absence of ambient authority.
/// Provider/profile/compaction settings are supplied explicitly by the trusted driver.
const SEAT_RUNTIME_ENV: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "WINDIR",
    "SYSTEMDRIVE",
    "COMSPEC",
    "TEMP",
    "TMP",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
];

/// The runtime allowlist plus explicit trusted-driver settings, with no ambient credentials.
pub fn seat_env(
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    set: &[(&str, OsString)],
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = parent
        .into_iter()
        .filter(|(k, _)| {
            let k = k.to_string_lossy().to_ascii_uppercase();
            SEAT_RUNTIME_ENV.contains(&k.as_str())
        })
        .collect();
    for (k, v) in set {
        env.retain(|(ek, _)| !ek.to_string_lossy().eq_ignore_ascii_case(k));
        env.push((OsString::from(k), v.clone()));
    }
    env
}

/// Apply [`seat_env`] to a command (clearing everything else).
pub fn apply_seat_env(cmd: &mut Command, set: &[(&str, OsString)]) {
    cmd.env_clear();
    for (k, v) in seat_env(std::env::vars_os(), set) {
        cmd.env(k, v);
    }
}

/// Find an executable on `PATH` (honouring `PATHEXT` on Windows), preferring real executables
/// over `.cmd` shims, which mangle JSON arguments.
pub fn find_exe(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        vec![".exe".into(), ".com".into()]
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Create (and return) an empty per-seat sandbox working directory under `root`. The directory is
/// the CLI's cwd; it holds nothing, and nothing above it is ever read by the seat's tools (it has
/// none beyond the game's).
pub fn make_sandbox(root: &Path, seat_label: &str) -> std::io::Result<PathBuf> {
    let dir = root.join(seat_label.replace(['.', '/', '\\', ':'], "_"));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Drive one turn: feed CLI lines to the parser, send entries to the sink in order, and return
/// when the parser reports the turn is done. `on_init` sees the session's init event.
pub(crate) async fn pump_turn(
    proc: &mut ChildProc,
    parser: &mut dyn StreamParser,
    limit: Duration,
    mut on_event: impl FnMut(&StreamEvent) -> Result<(), DriverError>,
) -> Result<TurnOutcome, DriverError> {
    let deadline = Instant::now() + limit;
    let mut quota = Vec::new();
    loop {
        let line = match proc.next_line(deadline).await {
            Err(_) => return Err(DriverError::Timeout),
            Ok(None) => {
                // Give the stderr reader a moment to finish before reporting why it died.
                tokio::time::sleep(Duration::from_millis(100)).await;
                return Err(DriverError::Died(proc.stderr_tail()));
            }
            Ok(Some(line)) => line,
        };
        for event in parser.feed(&line) {
            on_event(&event)?;
            match event {
                StreamEvent::Quota(q) => quota.push(q),
                StreamEvent::TurnDone(mut outcome) => {
                    outcome.quota = quota;
                    return Ok(outcome);
                }
                _ => {}
            }
        }
    }
}

/// Truncate to at most `max` characters on a char boundary, appending `…` when cut.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max).collect();
        s.push('…');
        s
    }
}

/// A tool result as a transcript entry: a short summary plus the parsed detail (bounded).
pub fn tool_result_entry(call_id: String, ok: bool, text: &str) -> TranscriptEntry {
    const MAX_DETAIL_BYTES: usize = 64 * 1024;
    let detail = if text.len() > MAX_DETAIL_BYTES {
        None
    } else {
        Some(
            serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.to_string())),
        )
    };
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    TranscriptEntry::ToolResult {
        call_id,
        ok,
        summary: clip(&flat, 200),
        detail,
    }
}

/// Sends parsed entries to the transcript and derives `decision_submitted` entries: when a
/// `submit` tool call returns successfully and was not a duplicate replay, the decision counts as
/// submitted. Works for every CLI because it only looks at paired call/result entries.
pub struct EntryEmitter {
    seat: SeatId,
    sink: crate::transcript::TranscriptSink,
    calls: BTreeMap<String, (String, Value)>,
}

impl EntryEmitter {
    pub fn new(seat: SeatId, sink: crate::transcript::TranscriptSink) -> Self {
        Self {
            seat,
            sink,
            calls: BTreeMap::new(),
        }
    }

    pub fn emit(&mut self, entry: TranscriptEntry) {
        let follow_up = match &entry {
            TranscriptEntry::ToolCall {
                call_id,
                tool,
                args,
            } => {
                self.calls
                    .insert(call_id.clone(), (tool.clone(), args.clone()));
                None
            }
            TranscriptEntry::ToolResult {
                call_id,
                ok: true,
                detail,
                ..
            } => match self.calls.remove(call_id) {
                Some((tool, args)) if tool == "submit" => {
                    let duplicate = detail
                        .as_ref()
                        .and_then(|d| d.get("duplicate"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    (!duplicate).then(|| TranscriptEntry::DecisionSubmitted {
                        decision_id: args["decision_id"].as_str().unwrap_or_default().to_string(),
                        summary: format!("submitted {}", clip(&args["action"].to_string(), 120)),
                    })
                }
                _ => None,
            },
            _ => None,
        };
        self.sink.emit(self.seat, entry);
        if let Some(f) = follow_up {
            self.sink.emit(self.seat, f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    #[test]
    fn env_scrubbing_removes_identity_and_credentials() {
        let parent = vec![
            (os("PATH"), os("/bin")),
            (os("SystemRoot"), os("C:/Windows")),
            (os("CNA_OPERATOR_TOKEN"), os("operator-secret")),
            (os("UNRELATED_SECRET_NAME"), os("operator-secret")),
            (os("CAPABILITY"), os("operator-secret")),
            (os("AWS_SESSION_TOKEN"), os("unrelated-provider")),
            (os("HTTPS_PROXY"), os("credential-bearing-proxy")),
            (
                os("CNA_CAPABILITY_FILE"),
                os("/server/operator-capabilities.json"),
            ),
            (os("CNA_SOURCES"), os("/game-private-sources")),
            (os("ORGTREE_AGENT_TOKEN"), os("secret")),
            (os("CLAUDE_CONFIG_DIR"), os("/other")),
            (os("ClaudeCode"), os("1")),
            (os("ANTHROPIC_API_KEY"), os("k")),
            (os("OPENAI_API_KEY"), os("k")),
            (os("CODEX_HOME"), os("/x")),
            (os("GEMINI_API_KEY"), os("k")),
            (os("AI_AGENT"), os("claude-code")),
        ];
        let env = seat_env(parent, &[("CLAUDE_CONFIG_DIR", os("/seat"))]);
        let names: Vec<String> = env
            .iter()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"PATH".to_string()));
        assert!(names.contains(&"SystemRoot".to_string()));
        assert_eq!(names.len(), 3, "{names:?}");
        let dir = env.iter().find(|(k, _)| k == "CLAUDE_CONFIG_DIR").unwrap();
        assert_eq!(dir.1, os("/seat"));
    }

    #[tokio::test]
    async fn runtime_allowlist_is_applied_to_spawn_and_resume() {
        if let Ok(phase) = std::env::var("SEAT_ENV_PROBE") {
            assert!(["spawn", "resume"].contains(&phase.as_str()));
            for name in [
                "UNRELATED_SECRET_NAME",
                "CNA_OPERATOR_TOKEN",
                "AWS_SESSION_TOKEN",
                "HTTPS_PROXY",
            ] {
                assert!(
                    std::env::var_os(name).is_none(),
                    "ambient credential name survived"
                );
            }
            assert_eq!(
                std::env::var("CLAUDE_CONFIG_DIR").unwrap(),
                "fixture-own-profile"
            );
            assert_eq!(
                std::env::var("CLAUDE_CODE_DISABLE_REFUSAL_FALLBACK").unwrap(),
                "1"
            );
            assert!(std::env::vars_os().all(|(name, _)| {
                let upper = name.to_string_lossy().to_ascii_uppercase();
                SEAT_RUNTIME_ENV.contains(&upper.as_str())
                    || [
                        "SEAT_ENV_PROBE",
                        "CLAUDE_CONFIG_DIR",
                        "CLAUDE_CODE_DISABLE_REFUSAL_FALLBACK",
                    ]
                    .contains(&upper.as_str())
            }));
            return;
        }
        for phase in ["spawn", "resume"] {
            let mut cmd = tokio::process::Command::new(std::env::current_exe().unwrap());
            cmd.args([
                "--exact",
                "driver::tests::runtime_allowlist_is_applied_to_spawn_and_resume",
                "--nocapture",
            ]);
            cmd.env("UNRELATED_SECRET_NAME", "synthetic-operator-canary");
            apply_seat_env(
                &mut cmd,
                &[
                    ("SEAT_ENV_PROBE", os(phase)),
                    ("CLAUDE_CONFIG_DIR", os("fixture-own-profile")),
                    ("CLAUDE_CODE_DISABLE_REFUSAL_FALLBACK", os("1")),
                ],
            );
            #[cfg(windows)]
            cmd.creation_flags(0x08000000);
            let output = tokio::time::timeout(Duration::from_secs(10), cmd.output())
                .await
                .unwrap()
                .unwrap();
            assert!(
                output.status.success(),
                "inert child environment assertion failed: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }
    #[test]
    fn tool_results_get_summary_and_detail() {
        let e = tool_result_entry("c1".into(), true, "{\n  \"a\": 1\n}");
        match e {
            TranscriptEntry::ToolResult {
                summary, detail, ..
            } => {
                assert_eq!(summary, "{ \"a\": 1 }");
                assert_eq!(detail.unwrap()["a"], 1);
            }
            other => panic!("{other:?}"),
        }
    }
}
