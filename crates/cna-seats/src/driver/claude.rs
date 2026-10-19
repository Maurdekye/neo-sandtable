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

pub use super::claude_caller::OriginalClaude;
pub(crate) use super::claude_caller::OriginalClaudeCaller;
use super::event_storage::EventStorage;
use super::{
    ChildProc, CliKind, DriverError, EntryEmitter, QuotaReading, SeatDriver, SessionInfo,
    StreamEvent, StreamParser, TurnOutcome, Usage, apply_seat_env_owned, find_exe, pump_turn,
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
    /// Effective window for native automatic compaction; None keeps CLI defaults.
    pub context_window: Option<u64>,
}

/// Concrete configuration storage measurement for ORIGINAL creation. This is
/// not a full-fit receipt: context/outer frames/library/allocator/transient
/// storage and archive reserves must still join the SAME creation admission.
#[allow(dead_code)] // SOURCE assembly; creation entry/public adoption unfinished.
pub(super) struct ConfigCreationLayout {
    pub(super) inline: std::alloc::Layout,
    pub(super) payload_bytes: usize,
}

#[allow(dead_code)]
impl ClaudeConfig {
    pub(super) fn original_creation_layout(
        &self,
    ) -> Result<ConfigCreationLayout, crate::failed_evidence::ArchiveError> {
        self.original_creation_layout_with_cap(crate::operation::OPERATION_BYTES)
    }

    // Production always uses the unchanged ORIGINAL cap above. The private
    // smaller-cap test seam exercises refusal without a huge fixture allocation;
    // no public/trusted caller can declare measured byte counts or renew a cap.
    fn original_creation_layout_with_cap(
        &self,
        cap: usize,
    ) -> Result<ConfigCreationLayout, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        let inline = std::alloc::Layout::for_value(self).pad_to_align();
        let mut payload_bytes = 0usize;
        let mut add_owned_buffer = |capacity| {
            let layout =
                std::alloc::Layout::array::<u8>(capacity).map_err(|_| ArchiveError::Fit)?;
            payload_bytes = payload_bytes
                .checked_add(layout.pad_to_align().size())
                .ok_or(ArchiveError::Fit)?;
            if inline
                .size()
                .checked_add(payload_bytes)
                .is_none_or(|bytes| bytes > cap)
            {
                return Err(ArchiveError::Fit);
            }
            Ok(())
        };
        // String/PathBuf headers already live inside the concrete cfg Layout.
        // Count capacity (including spare storage), not string/path length.
        // PathBuf capacity covers its owned encoded buffer, NOT later native
        // path conversion/library scratch or allocator rounding overhead.
        for body in [&self.model, &self.mcp_url, &self.system_prompt] {
            add_owned_buffer(body.capacity())?;
        }
        for body in [&self.expected_email, &self.effort].into_iter().flatten() {
            add_owned_buffer(body.capacity())?;
        }
        for path in [&self.sandbox, &self.run_dir] {
            add_owned_buffer(path.capacity())?;
        }
        for path in [&self.exe, &self.config_dir].into_iter().flatten() {
            add_owned_buffer(path.capacity())?;
        }
        // Also refuses a cap smaller than inline storage when every body is
        // empty. No allocation, clone, operation construction or effect here.
        if inline
            .size()
            .checked_add(payload_bytes)
            .is_none_or(|bytes| bytes > cap)
        {
            return Err(ArchiveError::Fit);
        }
        Ok(ConfigCreationLayout {
            inline,
            payload_bytes,
        })
    }
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
    build_claude_args(cfg, mcp_config, session, None, EventStorage(None))
        .expect("recorded argument construction has no operation cap")
}

/// Runtime argument copies use the original operation before allocating. The public
/// recorded-configuration helper keeps its exact argv and signature; it admits no effect.
/// Command/OS internals and allocator excess remain separate fit obligations.
fn build_claude_args(
    cfg: &ClaudeConfig,
    mcp_config: &std::path::Path,
    session: SessionArg<'_>,
    reported_cost_limit: Option<f64>,
    storage: EventStorage<'_>,
) -> Result<Vec<String>, crate::operation::OperationError> {
    if let Some(operation) = storage.0 {
        operation.check()?;
    }
    fn append(
        args: &mut Vec<String>,
        text: &str,
        storage: EventStorage<'_>,
    ) -> Result<(), crate::operation::OperationError> {
        storage.reserve_next(args)?;
        args.push(storage.copy(text)?);
        Ok(())
    }
    storage.charge(Some(std::mem::size_of::<Vec<String>>()))?;
    let mut a = Vec::new();
    for text in [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--replay-user-messages",
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
    ] {
        append(&mut a, text, storage)?;
    }
    append(&mut a, "--model", storage)?;
    append(&mut a, &cfg.model, storage)?;
    append(&mut a, "--mcp-config", storage)?;
    storage.reserve_next(&mut a)?;
    a.push(storage.formatted(format_args!("{}", mcp_config.display()))?);
    append(&mut a, "--system-prompt", storage)?;
    append(&mut a, &cfg.system_prompt, storage)?;
    if let Some(e) = &cfg.effort {
        append(&mut a, "--effort", storage)?;
        append(&mut a, e, storage)?;
    }
    match session {
        SessionArg::New(id) => {
            append(&mut a, "--session-id", storage)?;
            append(&mut a, id, storage)?;
        }
        SessionArg::Resume(id) => {
            append(&mut a, "--resume", storage)?;
            append(&mut a, id, storage)?;
        }
    }
    if let Some(limit) = reported_cost_limit {
        append(&mut a, "--max-budget-usd", storage)?;
        storage.reserve_next(&mut a)?;
        a.push(storage.formatted(format_args!("{limit}"))?);
    }
    if let Some(operation) = storage.0 {
        operation.check()?;
    }
    Ok(a)
}

fn claude_env(
    cfg: &ClaudeConfig,
    storage: EventStorage<'_>,
) -> Result<Vec<(&'static str, std::ffi::OsString)>, crate::operation::OperationError> {
    fn append(
        env: &mut Vec<(&'static str, std::ffi::OsString)>,
        key: &'static str,
        value: &std::ffi::OsStr,
        storage: EventStorage<'_>,
    ) -> Result<(), crate::operation::OperationError> {
        storage.reserve_next(env)?;
        env.push((key, storage.copy_os(value)?));
        Ok(())
    }
    if let Some(operation) = storage.0 {
        operation.check()?;
    }
    storage.charge(Some(std::mem::size_of::<
        Vec<(&'static str, std::ffi::OsString)>,
    >()))?;
    let mut env = Vec::new();
    if let Some(path) = &cfg.config_dir {
        append(&mut env, "CLAUDE_CONFIG_DIR", path.as_os_str(), storage)?;
    }
    if let Some(window) = cfg.context_window {
        let text = storage.formatted(format_args!("{window}"))?;
        append(
            &mut env,
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
            std::ffi::OsStr::new(&text),
            storage,
        )?;
        append(
            &mut env,
            "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
            std::ffi::OsStr::new("80"),
            storage,
        )?;
    }
    append(
        &mut env,
        "CLAUDE_CODE_DISABLE_REFUSAL_FALLBACK",
        std::ffi::OsStr::new("1"),
        storage,
    )?;
    if let Some(operation) = storage.0 {
        operation.check()?;
    }
    Ok(env)
}

/// Installed-version gate for durable resume totals (official CLI >=2.1.277).
pub fn supported_version(text: &str) -> Result<String, DriverError> {
    checked_supported_version(text)
        .map(str::to_owned)
        .map_err(|error| DriverError::Isolation(error.into()))
}

fn checked_supported_version(text: &str) -> Result<&str, &'static str> {
    let version = text.split_whitespace().next().unwrap_or("");
    let mut numbers = [0; 3];
    let mut count = 0usize;
    // Validate every component, including a malformed fourth component, preserving
    // the old gate's diagnostic precedence without allocating an unbounded Vec.
    for part in version.split('.') {
        let number = part
            .parse::<u32>()
            .map_err(|_| "unrecognized Claude version")?;
        if count < numbers.len() {
            numbers[count] = number;
        }
        count = count.saturating_add(1);
    }
    if count != 3 || (numbers[0], numbers[1], numbers[2]) < (2, 1, 277) {
        return Err("Claude >=2.1.277 is required for durable resumed totals");
    }
    Ok(version)
}

fn verified_version(
    text: &str,
    operation: &crate::operation::Operation,
) -> Result<String, DriverError> {
    let storage = EventStorage(Some(operation));
    match checked_supported_version(text) {
        Ok(version) => storage
            .copy(version)
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e))),
        Err(error) => {
            Err(DriverError::Isolation(storage.copy(error).map_err(
                |e| DriverError::Host(crate::host::operation_refusal(e)),
            )?))
        }
    }
}

fn verified_account(
    text: &str,
    operation: &crate::operation::Operation,
    expected: Option<&str>,
) -> Result<String, DriverError> {
    let storage = EventStorage(Some(operation));
    let value = match super::charged_json::decode_checked(text, operation)
        .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?
    {
        Ok(value) => value,
        Err(error) => {
            return Err(DriverError::Protocol(
                storage
                    .formatted(format_args!("claude auth status: {error}"))
                    .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?,
            ));
        }
    };
    if value["loggedIn"] != true {
        return Err(DriverError::Isolation(
            storage
                .copy("claude is not logged in")
                .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?,
        ));
    }
    let email = value["email"].as_str().unwrap_or_default();
    if let Some(want) = expected
        && !email.eq_ignore_ascii_case(want)
    {
        return Err(DriverError::Isolation(
            storage
                .formatted(format_args!(
                    "claude is logged in as {email}, expected {want}"
                ))
                .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?,
        ));
    }
    storage
        .copy(email)
        .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))
}

/// Verification borrows canonical bytes under the original helper owner. A failure keeps
/// the actual helper, raw bytes and diagnostics; there is no temporary output handoff that
/// disappears before version/account validation returns.
fn finish_verified_helper<T>(
    helpers: &mut Vec<ChildProc>,
    original: &crate::operation::Operation,
    allowance: crate::operation::Allowance,
    verify: impl FnOnce(&str, &crate::operation::Operation) -> Result<T, DriverError>,
) -> Result<T, DriverError> {
    let child = helpers.last_mut().ok_or_else(|| {
        DriverError::Host(crate::host::HostRefusal::Ownership(
            "original preflight helper is missing".into(),
        ))
    })?;
    if !child.capture_operation().same_instance(original)
        || child.is_running()
        || !child.readers.is_empty()
        || child.queued_frames() != 0
    {
        return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
            "original preflight child/readers/output remain unsettled".into(),
        )));
    }
    if !child.exit_success()? {
        return Err(DriverError::Isolation("CLI preflight refused".into()));
    }
    allowance
        .check()
        .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
    original
        .check()
        .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
    let verified = verify(&child.helper_output, original)?;
    // Synchronous parsing/copying consumes the SAME earlier helper bound too.
    allowance
        .check()
        .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
    original
        .check()
        .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
    helpers.pop(); // Only actual joins + accepted validation dispose of this helper output.
    Ok(verified)
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
    // Runtime-only association. Unarmed pure/owned fixture APIs keep their
    // existing assertion intent; actual run_turn installs this BEFORE write.
    // Never cleared/replaced by ACK, result, cancellation or parser refusal.
    runtime_operation: Option<crate::operation::Operation>,
    init_seen: bool,
    last_text: Option<String>,
    // Association belongs to the frame that created this canonical fallback.
    // Pure fixture parsing remains unowned; runtime cannot adopt that text.
    last_text_operation: Option<crate::operation::Operation>,
}

fn strip_prefix(name: &str) -> &str {
    name.strip_prefix("mcp__")
        .and_then(|name| name.strip_prefix(MCP_NAME))
        .and_then(|name| name.strip_prefix("__"))
        .unwrap_or(name)
}

fn u64_at(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

impl StreamParser for ClaudeParser {
    fn feed(&mut self, line: &str) -> Vec<StreamEvent> {
        // An unowned entry point cannot consume or replace runtime-owned text.
        if self.last_text_operation.is_some() || self.runtime_operation.is_some() {
            return Vec::new();
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        self.feed_value(v, EventStorage(None))
            .expect("pure parser storage")
    }

    fn feed_owned(
        &mut self,
        line: &str,
        operation: &crate::operation::Operation,
    ) -> Result<Vec<StreamEvent>, DriverError> {
        self.check_fallback_owner(Some(operation))
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        if self
            .runtime_operation
            .as_ref()
            .is_some_and(|original| !original.same_instance(operation))
        {
            return Err(DriverError::Host(crate::host::HostRefusal::HostUnknown));
        }
        let value = super::charged_json::decode(line, operation)
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        match value {
            Some(value) => {
                if let Some(original) = &self.runtime_operation {
                    match original.validate_claude_frame(&value) {
                        Ok(super::claude_correlation::Frame::ReplayAck) => return Ok(Vec::new()),
                        Ok(super::claude_correlation::Frame::Parse) => {}
                        Err(_) => {
                            return Err(DriverError::Host(crate::host::HostRefusal::HostUnknown));
                        }
                    }
                }
                self.feed_value(value, EventStorage(Some(operation)))
                    .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))
            }
            None if self.runtime_operation.is_some() => {
                self.runtime_operation
                    .as_ref()
                    .expect("runtime original")
                    .refuse_claude_frame();
                Err(DriverError::Host(crate::host::HostRefusal::HostUnknown))
            }
            None => Ok(Vec::new()),
        }
    }
}

impl ClaudeParser {
    fn check_fallback_owner(
        &self,
        operation: Option<&crate::operation::Operation>,
    ) -> Result<(), crate::operation::OperationError> {
        match (&self.last_text, &self.last_text_operation, operation) {
            (None, None, _) | (Some(_), None, None) => Ok(()),
            (Some(_), Some(owner), Some(original)) if owner.same_instance(original) => Ok(()),
            _ => Err(crate::operation::OperationError::CreditOwned),
        }
    }

    fn visit_archive(
        &self,
        original: &crate::operation::Operation,
        physical_child: Option<&str>,
        sink: &mut impl super::archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::{ArchiveError, EvidenceCategory, EvidenceRecord};
        self.check_fallback_owner(Some(original))
            .map_err(|_| ArchiveError::Identity)?;
        if let Some(owner) = self.runtime_operation.as_ref()
            && (!owner.same_instance(original) || physical_child.is_none())
        {
            return Err(ArchiveError::Identity);
        }
        if let Some(text) = &self.last_text {
            // Presence is a supervision gate, not a read-origin witness. This
            // parser interface receives only the original Operation; it cannot
            // manufacture a physical association from the current child.
            physical_child.ok_or(ArchiveError::Identity)?;
            let owner = self
                .last_text_operation
                .as_ref()
                .ok_or(ArchiveError::Identity)?;
            sink.bytes(
                EvidenceRecord {
                    operation: owner,
                    category: EvidenceCategory::Stdout,
                    part: "parser_fallback",
                    index: 0,
                    physical_child: None,
                    origin: None,
                },
                text.as_bytes(),
            )?;
        }
        Ok(())
    }

    fn feed_value(
        &mut self,
        v: Value,
        storage: EventStorage<'_>,
    ) -> Result<Vec<StreamEvent>, crate::operation::OperationError> {
        self.check_fallback_owner(storage.0)?;
        storage.charge(Some(std::mem::size_of::<Vec<StreamEvent>>()))?;
        let mut out = Vec::new();
        let mut init_seen = self.init_seen;
        let mut last_text = None;
        let mut result_done = false;
        // ONLY current-frame fields qualify. Restored modelUsage, cost, num_turns, init and
        // arbitrary result text are not evidence of this operation's model start.
        let confirmation = match v["type"].as_str() {
            Some("stream_event")
                if v["event"]["type"] == "message_start"
                    && v["event"]["message"]["role"] == "assistant" =>
            {
                Some(crate::operation::ConfirmationFrame::MessageStart)
            }
            Some("assistant")
                if v["message"]["content"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| match block["type"].as_str() {
                        Some("text") => block["text"].as_str().is_some_and(|s| !s.is_empty()),
                        Some("thinking") => {
                            block["thinking"].as_str().is_some_and(|s| !s.is_empty())
                        }
                        Some("tool_use") => {
                            block["id"].as_str().is_some_and(|s| !s.is_empty())
                                && block["name"].as_str().is_some_and(|s| !s.is_empty())
                                && block["input"].is_object()
                        }
                        _ => false,
                    })
                }) =>
            {
                Some(crate::operation::ConfirmationFrame::Assistant)
            }
            Some("result")
                if [
                    "input_tokens",
                    "output_tokens",
                    "cache_read_input_tokens",
                    "cache_creation_input_tokens",
                ]
                .iter()
                .any(|key| v["usage"][*key].as_u64().is_some_and(|n| n > 0))
                    || v["usage"]["output_tokens_details"]["thinking_tokens"]
                        .as_u64()
                        .is_some_and(|n| n > 0) =>
            {
                Some(crate::operation::ConfirmationFrame::ResultUsage)
            }
            _ => None,
        };
        if let (Some(frame), Some(session_id)) = (confirmation, v["session_id"].as_str()) {
            storage.push(
                &mut out,
                StreamEvent::ModelStart {
                    session_id: storage.copy(session_id)?,
                    frame,
                },
            )?;
        }
        match v.get("type").and_then(Value::as_str) {
            Some("system") => match v.get("subtype").and_then(Value::as_str) {
                Some("init") => {
                    // Claude repeats `init` at the start of every turn; report it once.
                    if !self.init_seen {
                        init_seen = true;
                        storage.charge(Some(std::mem::size_of::<Vec<String>>()))?;
                        let mut tools = Vec::new();
                        for tool in v["tools"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                        {
                            storage.push(&mut tools, storage.copy(tool)?)?;
                        }
                        let mcp_connected = v["mcp_servers"].as_array().map(|s| {
                            s.iter()
                                .any(|m| m["name"] == MCP_NAME && m["status"] == "connected")
                        });
                        storage.push(
                            &mut out,
                            StreamEvent::Init {
                                session_id: storage
                                    .copy(v["session_id"].as_str().unwrap_or_default())?,
                                model: storage.optional(v["model"].as_str())?,
                                cli_version: storage.optional(v["claude_code_version"].as_str())?,
                                tools,
                                mcp_connected,
                            },
                        )?;
                    }
                }
                Some("compact_boundary") => {
                    storage.push(
                        &mut out,
                        StreamEvent::Compacted {
                            trigger: storage.copy(
                                v["compact_metadata"]["trigger"]
                                    .as_str()
                                    .unwrap_or("unknown"),
                            )?,
                            pre_tokens: v["compact_metadata"]["pre_tokens"].as_u64(),
                        },
                    )?;
                }
                Some("permission_denied") => {
                    storage.push(
                        &mut out,
                        StreamEvent::Entry(TranscriptEntry::System {
                            text: storage.concat(&[
                                "tool call denied: ",
                                strip_prefix(v["tool_name"].as_str().unwrap_or("?")),
                            ])?,
                        }),
                    )?;
                }
                _ => {}
            },
            Some("assistant") => {
                let usage = &v["message"]["usage"];
                if usage.is_object() {
                    let context = [
                        "input_tokens",
                        "cache_read_input_tokens",
                        "cache_creation_input_tokens",
                        "output_tokens",
                    ]
                    .iter()
                    .map(|key| usage[*key].as_u64().unwrap_or(0))
                    .fold(0u64, u64::saturating_add);
                    storage.push(&mut out, StreamEvent::Context(context))?;
                }
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            let text = block["text"].as_str().unwrap_or_default();
                            if !text.trim().is_empty() {
                                // Fixed owner metadata precedes its Arc association/copy.
                                storage.charge(Some(std::mem::size_of::<
                                    Option<crate::operation::Operation>,
                                >()))?;
                                last_text = Some(storage.copy(text)?);
                                storage.push(
                                    &mut out,
                                    StreamEvent::Entry(TranscriptEntry::AssistantText {
                                        text: storage.copy(text)?,
                                    }),
                                )?;
                            }
                        }
                        Some("thinking") => {
                            // Some models stream only a signature; show reasoning when it exists.
                            let text = block["thinking"].as_str().unwrap_or_default();
                            if !text.trim().is_empty() {
                                storage.push(
                                    &mut out,
                                    StreamEvent::Entry(TranscriptEntry::Reasoning {
                                        text: storage.copy(text)?,
                                    }),
                                )?;
                            }
                        }
                        Some("tool_use") => {
                            storage.push(
                                &mut out,
                                StreamEvent::Entry(TranscriptEntry::ToolCall {
                                    call_id: storage
                                        .copy(block["id"].as_str().unwrap_or_default())?,
                                    tool: storage.copy(strip_prefix(
                                        block["name"].as_str().unwrap_or("?"),
                                    ))?,
                                    args: storage.copy_value(&block["input"])?,
                                }),
                            )?;
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
                    let text = storage.content(&block["content"])?;
                    storage.push(
                        &mut out,
                        StreamEvent::Entry(storage.tool_result(
                            storage.copy(block["tool_use_id"].as_str().unwrap_or_default())?,
                            block["is_error"] != true,
                            &text,
                        )?),
                    )?;
                }
            }
            Some("rate_limit_event") => {
                if let Some(windows) = v["rate_limit_info"]["unifiedWindows"].as_object() {
                    for (name, w) in windows {
                        if let Some(u) = w["utilization"].as_f64() {
                            storage.push(
                                &mut out,
                                StreamEvent::Quota(QuotaReading {
                                    window: storage.copy(name)?,
                                    utilization: u,
                                    resets_at_epoch_s: w["resetsAt"].as_i64(),
                                }),
                            )?;
                        }
                    }
                }
            }
            Some("result") => {
                let is_error = v["is_error"] == true || v["subtype"] != "success";
                let usage = &v["usage"];
                let error = if !is_error {
                    None
                } else {
                    Some(storage.copy(if v["subtype"] == "error_max_budget_usd" {
                        "error_max_budget_usd"
                    } else {
                        v["result"]
                            .as_str()
                            .or(v["subtype"].as_str())
                            .unwrap_or("error")
                    })?)
                };
                storage.charge(Some(std::mem::size_of::<Vec<QuotaReading>>()))?;
                storage.push(
                    &mut out,
                    StreamEvent::TurnDone(TurnOutcome {
                        ok: !is_error,
                        error,
                        text: storage.optional(v["result"].as_str())?,
                        usage: Usage {
                            input_tokens: u64_at(usage, "input_tokens"),
                            output_tokens: u64_at(usage, "output_tokens"),
                            cached_input_tokens: u64_at(usage, "cache_read_input_tokens"),
                            cache_creation_tokens: u64_at(usage, "cache_creation_input_tokens"),
                            reasoning_tokens: usage["output_tokens_details"]["thinking_tokens"]
                                .as_u64(),
                            cost_usd: v["total_cost_usd"].as_f64(),
                            cost_basis_known: v["modelUsage"].as_object().map(|models| {
                                !models.is_empty()
                                    && models.values().all(|m| {
                                        matches!(m["costBasis"].as_str(), Some("list" | "managed"))
                                    })
                            }),
                        },
                        quota: Vec::new(),
                    }),
                )?;
                result_done = true;
            }
            _ => {}
        }
        // Publish state only after all fallible derived admissions succeeded. A refused
        // frame remains raw-owned and cannot consume init or the previous text fallback.
        self.init_seen = init_seen;
        if result_done {
            if let Some(StreamEvent::TurnDone(outcome)) = out.last_mut()
                && outcome.text.is_none()
            {
                outcome.text = self.last_text.take();
            }
            self.last_text = None;
            self.last_text_operation = None;
        } else if last_text.is_some() {
            self.last_text = last_text;
            self.last_text_operation = storage.0.cloned();
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------------------------
// Driver

/// Canonical startup evidence, retained before the native effect. Transcript admission
/// failure cannot discard the SAME child or publish a healthy session to the caller.
struct StartupCapture {
    operation: crate::operation::Operation,
    text: String,
    info: SessionInfo,
    published_session_id: String,
    effect_accepted: bool,
    failure: Option<crate::operation::OperationError>,
}

impl StartupCapture {
    fn visit_archive(
        &self,
        original: &crate::operation::Operation,
        physical_child: Option<&str>,
        sink: &mut impl super::archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::{ArchiveError, EvidenceCategory, EvidenceRecord};
        use crate::operation::OperationError;
        if !self.operation.same_instance(original)
            || (self.effect_accepted && physical_child.is_none())
        {
            return Err(ArchiveError::Identity);
        }
        let record = |part| EvidenceRecord {
            operation: &self.operation,
            category: EvidenceCategory::Startup,
            part,
            index: 0,
            physical_child,
            origin: None,
        };
        let failure = self.failure.map(|error| match error {
            OperationError::NoAllowance => "no_allowance",
            OperationError::Expired => "expired",
            OperationError::CaptureOverflow => "capture_overflow",
            OperationError::Settled => "settled",
            OperationError::CreditOwned => "credit_owned",
        });
        // Canonical private startup descriptors remain separate from transcript
        // copies and controller/session identity never becomes physical origin.
        sink.bytes(record("startup_text"), self.text.as_bytes())?;
        sink.json(record("startup_info"), &self.info)?;
        sink.json(
            record("startup_state"),
            &(
                self.published_session_id.as_str(),
                self.effect_accepted,
                failure,
            ),
        )
    }

    fn prepare(
        operation: &crate::operation::Operation,
        id: &str,
        model: &str,
        version: String,
        email: &str,
        resumed: bool,
    ) -> Result<Self, DriverError> {
        let storage = EventStorage(Some(operation));
        let prepare = || {
            storage.charge(Some(std::mem::size_of::<Self>()))?;
            let text = storage.concat(&[
                if resumed {
                    "resumed session "
                } else {
                    "started session "
                },
                id,
                " (",
                CliKind::Claude.as_str(),
                ", model ",
                model,
                ", account ",
                email,
                ")",
            ])?;
            let session_id = storage.copy(id)?;
            let published_session_id = storage.copy(id)?;
            let model = storage.copy(model)?;
            Ok::<_, crate::operation::OperationError>((
                text,
                session_id,
                published_session_id,
                model,
            ))
        };
        let (text, session_id, published_session_id, model) =
            prepare().map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        Ok(Self {
            operation: operation.clone(),
            text,
            info: SessionInfo {
                session_id,
                model: Some(model),
                // This is the already-owned preflight output, moved rather than cloned.
                cli_version: Some(version),
                resumed,
            },
            published_session_id,
            effect_accepted: false,
            failure: None,
        })
    }
}

pub struct ClaudeDriver {
    cfg: ClaudeConfig,
    sink: TranscriptSink,
    proc: Option<ChildProc>,
    parser: ClaudeParser,
    session_id: Option<String>,
    telemetry: super::SessionTelemetry,
    idle: bool,
    reported_cost_limit: Option<f64>,
    host: Option<crate::host::HostDriverContext>,
    helpers: tokio::sync::Mutex<Vec<ChildProc>>,
    prompt_lease: Option<Box<dyn crate::host::HostPromptLease>>,
    emitter: Option<EntryEmitter>,
    completed_turn: Option<TurnOutcome>,
    startup: Option<StartupCapture>,
}

impl ClaudeDriver {
    /// Borrowed version of the existing host preflight. No context is removed
    /// from its caller while checks can refuse or unwind.
    fn check_host_context(
        &self,
        context: &crate::host::HostDriverContext,
    ) -> Result<(), DriverError> {
        if let Some(proc) = &self.proc {
            proc.check_reader_rebind(&context.operation)?;
        }
        match self.helpers.try_lock() {
            Ok(helpers) if helpers.is_empty() => {}
            _ => {
                return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                    "original preflight helper/allowance remains owned".into(),
                )));
            }
        }
        if self.prompt_lease.is_some()
            || self.emitter.is_some()
            || self.completed_turn.is_some()
            || self.startup.is_some()
            || self.parser.last_text.is_some()
            || self.parser.runtime_operation.is_some()
            || self.proc.as_ref().is_some_and(|p| p.queued_frames() != 0)
        {
            return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                "previous prompt remains owned".into(),
            )));
        }
        context
            .operation
            .check()
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))
    }

    /// The new private owner binds ONLY a previously unbound concrete driver.
    /// Its original context stays installed in the owner through preflight;
    /// final assignment MOVES it to this SAME Box with no later callback/check.
    pub(super) fn bind_host_from_slot(
        &mut self,
        slot: &mut Option<crate::host::HostDriverContext>,
        expected: &crate::operation::Operation,
    ) -> Result<(), DriverError> {
        let context = slot
            .as_ref()
            .ok_or(DriverError::Host(crate::host::HostRefusal::Unknown))?;
        if self.host.is_some()
            || !context.operation.same_instance(expected)
            || context.identity.seat != self.cfg.seat
            || context.identity.operation_id != expected.id()
        {
            return Err(DriverError::Host(crate::host::HostRefusal::IdentityChanged));
        }
        self.check_host_context(context)?;
        self.host = slot.take();
        Ok(())
    }

    /// Private one-shot construction for the future boxed factory adapter.
    /// The host must already name the ORIGINAL operation. Even a refused box
    /// preparation returns a retained stageful owner, never a discarded Box.
    /// No native method/worker is entered and the public factory stays closed
    /// to this path until its boxed/outer future allocation fit is assembled.
    fn prepare_original_owner(
        self: Box<Self>,
        original: &crate::operation::Operation,
    ) -> super::archive_work::PreparationOwner<Self> {
        super::archive_work::PreparationOwner::prepare(
            self.prepare_archive_work(original),
            original,
        )
    }

    /// Borrow original terminal/result/prompt ownership through actor ACK. A
    /// pending/refused/canceled ACK keeps every owner and the non-idle cleanup
    /// policy. BOTH-reader pause is a prerequisite, not a replacement witness.
    async fn settle_captured_terminal(
        &mut self,
        original: &crate::operation::Operation,
    ) -> Result<TurnOutcome, DriverError> {
        if self.idle
            || self.completed_turn.is_none()
            || self
                .host
                .as_ref()
                .is_none_or(|host| !host.operation.same_instance(original))
            || self
                .parser
                .runtime_operation
                .as_ref()
                .is_none_or(|saved| !saved.same_instance(original))
            || self
                .emitter
                .as_ref()
                .is_some_and(EntryEmitter::has_pending_calls)
        {
            return Err(DriverError::Host(crate::host::HostRefusal::Unknown));
        }
        self.proc
            .as_ref()
            .ok_or(DriverError::Host(crate::host::HostRefusal::Unknown))?
            .check_terminal_pause(original)?;
        self.prompt_lease
            .as_ref()
            .ok_or(DriverError::Host(crate::host::HostRefusal::Unknown))?
            .settle()
            .await
            .map_err(DriverError::Host)?;
        // A read-winning boundary failure during actor settlement is still a
        // refusal, even when the actor accepted its own settlement. Recheck
        // before removing owners or changing the cleanup policy.
        self.proc
            .as_ref()
            .ok_or(DriverError::Host(crate::host::HostRefusal::Unknown))?
            .check_terminal_pause(original)?;
        // No further await or fallible settlement before the outcome MOVE.
        // The child/gate/request and correlation association remain owned.
        self.prompt_lease = None;
        self.emitter = None;
        self.idle = true;
        Ok(self
            .completed_turn
            .take()
            .expect("preflighted original terminal"))
    }

    /// Private original-box preparation only; no task launch or public seat tool.
    /// Caller must retain this owner and original task handle before worker await.
    fn prepare_archive_work(
        mut self: Box<Self>,
        original: &crate::operation::Operation,
    ) -> Result<
        super::archive_work::ArchiveWork<Self>,
        (crate::failed_evidence::ArchiveError, Box<Self>),
    > {
        if let Err(error) = self.check_archive_preparation(original) {
            return Err((error, self));
        }
        super::archive_work::ArchiveWork::prepare(self, original)
    }

    /// Borrowed preparation preflight: the caller keeps its SAME Box installed
    /// throughout every fallible/panicking check and unpolled frame inspection.
    /// Root allocation/installation is separate; this grants no native effect.
    pub(super) fn check_archive_preparation(
        &mut self,
        original: &crate::operation::Operation,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        if self.host.as_ref().is_none_or(|context| {
            !context.operation.same_instance(original)
                || context.identity.seat != self.cfg.seat
                || context.identity.operation_id != original.id()
        }) {
            return Err(crate::failed_evidence::ArchiveError::Identity);
        }
        // Creation-time fit, BEFORE capture or Arc/root allocation. Constructing
        // the unpolled concrete owner futures borrow only; they perform/allocate
        // no native work. The previously admitted workspace covers these frames
        // headers; no late reserve/charge after capture overflow is introduced.
        let required = self.archive_owner_inline_bytes();
        required.and_then(|bytes| {
            crate::failed_evidence::check_retained_publication_workspace::<Self>(bytes)
        })
    }

    /// Creation-time concrete foreground frames, not a whole-runtime bound.
    /// These futures are constructed and dropped WITHOUT polling. Their layouts
    /// do not depend on prompt length, resume text or relative limit values.
    /// Sum all three conservatively rather than relying on exclusive scheduling;
    /// each frame already includes its inline awaited concrete child futures.
    /// The outer launcher and trait/host boxed futures, payload allocations and
    /// library/transient storage still require their separate original fit.
    fn archive_owner_inline_bytes(
        &mut self,
    ) -> Result<usize, crate::failed_evidence::ArchiveError> {
        let start = std::mem::size_of_val(&self.start_original_owner(None));
        let turn = std::mem::size_of_val(&self.run_turn_original_owner("", Duration::ZERO));
        let stop = std::mem::size_of_val(&self.stop_original_owner());
        super::archive_work::RetainedArchive::<Self>::fixed_bytes()?
            .checked_add(start)
            .and_then(|bytes| bytes.checked_add(turn))
            .and_then(|bytes| bytes.checked_add(stop))
            .ok_or(crate::failed_evidence::ArchiveError::Fit)
    }

    /// The SAME driver owns every record throughout both passes. There is no
    /// snapshot/mirror, dequeue/requeue, child-ID inference or lock across a
    /// callback. Helpers stay in their original vector order, then the session,
    /// startup, parser fallback and pairing cache. Each component validates its
    /// original instance and actual joins; the coverage pass preflights all of
    /// them, including late opaque panic refusal, before any output is invoked.
    fn visit_original_archive(
        &mut self,
        original: &crate::operation::Operation,
        sink: &mut impl super::archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        let plan = original.archive_plan().ok_or(ArchiveError::Identity)?;
        if plan.seat != self.cfg.seat
            || self.host.as_ref().is_some_and(|host| {
                !host.operation.same_instance(original)
                    || host.identity.seat != plan.seat
                    || host.identity.epoch != plan.epoch
                    || host.identity.operation_id != original.id()
            })
            || self.completed_turn.is_some()
        {
            // Captured outcome must MOVE through the separate expected-original
            // journal handoff. Archive enumeration never substitutes usage/CAS
            // completion, overwrites a supervisor failure or drops that owner.
            return Err(ArchiveError::Identity);
        }
        // ALL retained physical owners precede even the first borrowed coverage
        // callback. A joined earlier helper cannot stand in for a later owner.
        for helper in self.helpers.get_mut() {
            if !helper.physical_supervision_complete()
                || !super::archive_stdout::unique_storage(&mut helper.capture)?
                    .same_instance(original)
            {
                return Err(ArchiveError::Identity);
            }
        }
        if let Some(child) = self.proc.as_mut()
            && (!child.physical_supervision_complete()
                || !super::archive_stdout::unique_storage(&mut child.capture)?
                    .same_instance(original))
        {
            return Err(ArchiveError::Identity);
        }
        let gate = original.reader_handoff_ref();
        let gate_child = match self.proc.as_ref() {
            Some(child)
                if child
                    .handoff
                    .as_ref()
                    .is_some_and(|saved| std::ptr::eq(saved.as_ref(), gate)) =>
            {
                Some((&child.physical_child_id, child.handoff_request.as_ref()))
            }
            Some(child) if child.handoff.is_some() || child.handoff_request.is_some() => {
                return Err(ArchiveError::Identity);
            }
            _ => None,
        };
        // Original gate exists even on a no-effect failure. ALL actual original
        // joins were checked above; complete metadata never substitutes them.
        gate.visit_archive(original, gate_child, sink)?;
        for helper in self.helpers.get_mut() {
            helper.visit_retained_archive(original, sink)?;
        }
        if let Some(child) = self.proc.as_mut() {
            child.visit_retained_archive(original, sink)?;
        }
        self.visit_startup_archive(original, sink)?;
        self.visit_parser_archive(original, sink)?;
        self.visit_pairing_archive(original, sink)
    }

    /// Partial transcript diagnostic visit only, deliberately NOT included in
    /// visit_original_archive until its OTHER queue/reply/capture owners and
    /// actual worker join custody are enumerated. Empty is not whole coverage.
    /// Both passes share this original borrow/order; no mutex guard, body clone,
    /// Display/format conversion, fabricated child/origin or source release.
    #[allow(dead_code)]
    fn visit_worker_refusal_archive(
        &mut self,
        original: &crate::operation::Operation,
        sink: &mut impl super::archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::{ArchiveError, EvidenceCategory, EvidenceRecord};
        use crate::host::HostRefusal;
        use crate::transcript::BorrowedWorkerRefusal;

        let plan = original.archive_plan().ok_or(ArchiveError::Identity)?;
        if plan.seat != self.cfg.seat
            || self.host.as_ref().is_some_and(|host| {
                !host.operation.same_instance(original)
                    || host.identity.seat != plan.seat
                    || host.identity.epoch != plan.epoch
                    || host.identity.operation_id != original.id()
            })
        {
            return Err(ArchiveError::Identity);
        }
        let diagnostics = self.sink.borrowed_worker_refusal(original)?;
        let class_part = match diagnostics.join {
            crate::transcript::OwnedDeliveryJoinObservation::Completed => "worker_completed",
            crate::transcript::OwnedDeliveryJoinObservation::Cancelled => "worker_cancelled",
            crate::transcript::OwnedDeliveryJoinObservation::Panicked => "worker_panicked",
        };
        sink.bytes(
            EvidenceRecord {
                operation: original,
                category: EvidenceCategory::Diagnostic,
                part: class_part,
                index: 0,
                physical_child: None,
                origin: None,
            },
            if diagnostics.join == crate::transcript::OwnedDeliveryJoinObservation::Panicked {
                b"see server log; seat cleanup Unsettled"
            } else {
                b""
            },
        )?;
        let Some(refusal) = diagnostics.refusal else {
            return Ok(()); // No first worker cause ONLY, not no other liability.
        };
        // Static part names retain the exact variant. Ownership preserves its
        // raw child bytes rather than flattening it into a displayed message.
        let (part, bytes): (&'static str, &[u8]) = match refusal {
            BorrowedWorkerRefusal::Installation(body) => {
                ("worker_installation_refusal", body.as_bytes())
            }
            BorrowedWorkerRefusal::Append(body) => ("worker_append_refusal", body.as_bytes()),
            BorrowedWorkerRefusal::SettlementUncertain(body) => {
                ("worker_settlement_uncertain", body.as_bytes())
            }
            BorrowedWorkerRefusal::SettlementTimedOut(body) => {
                ("worker_settlement_timed_out", body.as_bytes())
            }
            BorrowedWorkerRefusal::MarkerCopy(body) => match body {
                HostRefusal::WaitingForActivation => ("worker_marker_waiting_for_activation", b""),
                HostRefusal::StoppedBeforeWork => ("worker_marker_stopped_before_work", b""),
                HostRefusal::IdentityChanged => ("worker_marker_identity_changed", b""),
                HostRefusal::Unknown => ("worker_marker_unknown", b""),
                HostRefusal::NoAllowance => ("worker_marker_no_allowance", b""),
                HostRefusal::Expired => ("worker_marker_expired", b""),
                HostRefusal::CaptureOverflow => ("worker_marker_capture_overflow", b""),
                HostRefusal::Ownership(body) => ("worker_marker_ownership", body.as_bytes()),
            },
        };
        sink.bytes(
            EvidenceRecord {
                operation: original,
                category: EvidenceCategory::Diagnostic,
                part,
                index: 0,             // ONE actual fixed first-cause cell, not global N.
                physical_child: None, // Worker cause saved no physical witness.
                origin: None,         // Unknown provenance stays unknown.
            },
            bytes,
        )
    }

    #[allow(dead_code)]
    fn cover_worker_refusal_archive(
        &mut self,
        original: &crate::operation::Operation,
        coverage: &mut crate::failed_evidence::EvidenceCoverageBuilder<'_>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.visit_worker_refusal_archive(original, coverage)
    }

    #[allow(dead_code)]
    fn encode_worker_refusal_archive<W: std::io::Write>(
        &mut self,
        original: &crate::operation::Operation,
        encoder: &mut crate::failed_evidence::ArchiveEncoder<'_, W>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.visit_worker_refusal_archive(original, encoder)
    }

    fn original_archive_coverage(
        &mut self,
        original: &crate::operation::Operation,
    ) -> Result<crate::failed_evidence::EvidenceCoverage, crate::failed_evidence::ArchiveError>
    {
        let mut coverage = crate::failed_evidence::EvidenceCoverageBuilder::new(original)?;
        self.visit_original_archive(original, &mut coverage)?;
        coverage.finish()
    }

    /// Trusted internal publication assembly only. Every canonical owner stays
    /// in this driver after success or failure, and captured-outcome handoff is
    /// still separate. Neither stop nor the async supervisor invokes blocking
    /// filesystem work here; responsiveness/custody integration remains required.
    fn publish_original_archive(
        &mut self,
        original: &crate::operation::Operation,
    ) -> Result<crate::failed_evidence::ArchiveFileObservation, crate::failed_evidence::ArchiveError>
    {
        let coverage = self.original_archive_coverage(original)?;
        crate::failed_evidence::publish_joined_archive(original, coverage, self)
    }

    /// Inline overlap only, shared by original driver and retained-root checks.
    /// Hidden serializer/library/stack allocations are not covered by headers.
    pub(super) fn archive_custody_fixed_bytes() -> usize {
        use crate::failed_evidence::{ArchiveEncoder, ArchiveMeter, EvidenceCoverageBuilder};
        std::mem::size_of::<ArchiveMeter>()
            + std::mem::size_of::<crate::failed_evidence::ArchiveFileObservation>()
            + std::mem::size_of::<crate::operation::ArchiveChunkLease>()
            + std::mem::size_of::<EvidenceCoverageBuilder<'_>>()
            + std::mem::size_of::<ArchiveEncoder<'_, ArchiveMeter>>()
    }

    /// Private accepted-custody step, distinct from publishing/observing a file.
    /// Re-measure the SAME canonical source without a body buffer or filesystem
    /// operation. Stale identity, changed bytes, unfinished/shared storage or
    /// incomplete error coverage leave EVERY original owner intact. The caller
    /// retains the observation, including any UNACCEPTED-temp/flush warnings.
    /// Neither this method nor publication is wired into the async stop path.
    pub(super) fn accept_original_archive(
        &mut self,
        original: &crate::operation::Operation,
        observed: &crate::failed_evidence::ArchiveFileObservation,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::{ArchiveError, ArchiveMeter, JoinedArchiveSource};
        let fixed = Self::archive_custody_fixed_bytes();
        if fixed > crate::failed_evidence::FIXED_ARCHIVE_WORKSPACE {
            return Err(ArchiveError::Fit);
        }
        // Borrow the creation-time cleanup reserve; checked-out or already
        // settled ownership refuses before source release. Drop returns this
        // SAME buffer even on refusal, without new allocation/credit/deadline.
        let _chunk = original.borrow_archive_chunk(original)?;
        let coverage = self.original_archive_coverage(original)?;
        if self.parser.runtime_operation.is_some() {
            // Enumeration is now complete for correlation, but its original
            // lifetime/rebind disposition and active handoff custody are unfinished.
            // Never clear runtime ownership or accept a partial lifecycle here.
            return Err(ArchiveError::Identity);
        }
        // Validate the ENTIRE release set before its first mutation. No callbacks
        // on custom error payloads are permitted by this release path.
        for helper in self.helpers.get_mut() {
            helper.check_archive_release()?;
        }
        if let Some(child) = self.proc.as_mut() {
            child.check_archive_release()?;
        }
        let mut meter = ArchiveMeter::new();
        let measurement = self.stream(original, coverage, &mut meter)?;
        if meter.finish()? != measurement || !observed.matches(original, coverage, measurement) {
            return Err(ArchiveError::Identity);
        }
        // Everything above may refuse without mutation. No callback/await/I/O
        // occurs below; the complete source and unique storage remain fixed.
        for helper in self.helpers.get_mut() {
            helper.clear_archived_storage();
        }
        if let Some(child) = self.proc.as_mut() {
            child.clear_archived_storage();
        }
        self.startup = None;
        self.parser.last_text = None;
        self.parser.last_text_operation = None;
        self.emitter = None;
        // Host context, captured outcome, prompt lease, telemetry/session identity,
        // original Operation and child owners remain. No settle/check/permit/ACK,
        // refund, usage/CAS completion or replacement occurs in this interval.
        Ok(())
    }

    fn visit_parser_archive(
        &mut self,
        original: &crate::operation::Operation,
        sink: &mut impl super::archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use super::archive_stdout::unique_storage;
        use crate::failed_evidence::ArchiveError;
        for helper in self.helpers.get_mut() {
            if !helper.physical_supervision_complete()
                || !unique_storage(&mut helper.capture)?.same_instance(original)
            {
                return Err(ArchiveError::Identity);
            }
        }
        let physical_child = match self.proc.as_mut() {
            Some(child) => {
                if !child.physical_supervision_complete()
                    || !unique_storage(&mut child.capture)?.same_instance(original)
                {
                    return Err(ArchiveError::Identity);
                }
                Some(child.physical_child_id.as_ref())
            }
            None => None,
        };
        // ALL actual joins precede this original metadata borrow. Include the
        // Unwritten state even when no runtime parser association was installed.
        // Validate parser identities BEFORE emitting this metadata prefix.
        self.parser
            .check_fallback_owner(Some(original))
            .map_err(|_| ArchiveError::Identity)?;
        if let Some(owner) = self.parser.runtime_operation.as_ref()
            && (!owner.same_instance(original) || physical_child.is_none())
        {
            return Err(ArchiveError::Identity);
        }
        let metadata = original.borrow_claude_correlation(original)?;
        metadata
            .value()
            .visit_archive(original, self.parser.runtime_operation.is_some(), sink)?;
        self.parser.visit_archive(original, physical_child, sink)
    }

    /// Attempt every original physical owner even when an earlier owner retains
    /// evidence or returns an error. This pass neither removes owners nor asks
    /// for prompt/actor/archive custody; cancellation leaves each child in place.
    /// Each child pins its existing earliest cleanup deadline independently.
    pub(super) async fn supervise_retained_children(&mut self) -> std::io::Result<()> {
        let mut first = None;
        // Exclusive driver access needs no mutex guard across these awaits.
        for helper in self.helpers.get_mut().iter_mut().rev() {
            if let Err(error) = helper.kill().await {
                // Original reader errors stay in their child. Keep only a fixed
                // summary here, with no formatted error/body copy at overflow.
                first.get_or_insert(error.kind());
            }
        }
        if let Some(child) = self.proc.as_mut() {
            let result = if self.idle {
                child.finish(Duration::from_secs(2)).await
            } else {
                child.kill().await
            };
            if let Err(error) = result {
                first.get_or_insert(error.kind());
            }
        }
        match first {
            Some(kind) => Err(kind.into()),
            None => Ok(()),
        }
    }

    /// Physical cleanup has no dispatch/custody permission. Validate the SAME
    /// host and EVERY saved child's original read owner before attempting any
    /// joins. In-flight bytes stay with those children and exact pipe objects.
    pub(super) fn check_physical_cleanup_original(
        &mut self,
        original: &crate::operation::Operation,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        if self.host.as_ref().is_none_or(|host| {
            !host.operation.same_instance(original)
                || host.identity.operation_id != original.id()
                || host.identity.seat != self.cfg.seat
        }) || self
            .helpers
            .get_mut()
            .iter()
            .any(|helper| !helper.capture_operation().same_instance(original))
            || self
                .proc
                .as_ref()
                .is_some_and(|child| !child.capture_operation().same_instance(original))
        {
            return Err(ArchiveError::Identity);
        }
        Ok(())
    }

    /// Synchronous MOVE to an already creation-admitted ORIGINAL journal slot.
    /// Independent from physical joins, archive custody and healthy settlement.
    /// No returned outcome can be dropped between this move and slot retention.
    pub(super) fn handoff_original_captured(
        &mut self,
        original: &crate::operation::Operation,
        destination_original: &crate::operation::Operation,
        destination: &mut Option<TurnOutcome>,
    ) -> Result<bool, crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::ArchiveError;
        if !original.same_instance(destination_original) || destination.is_some() {
            return Err(ArchiveError::Identity);
        }
        self.check_physical_cleanup_original(original)?;
        if self.completed_turn.is_none() {
            return Ok(false); // No captured value, never supervision/usage proof.
        }
        if self.proc.is_none()
            || self
                .parser
                .runtime_operation
                .as_ref()
                .is_none_or(|saved| !saved.same_instance(original))
        {
            return Err(ArchiveError::Identity);
        }
        // The SAME parser/child owns this already captured evidence. A later
        // correlation/boundary refusal must remain sticky, not strand that
        // earlier value or turn this evidence transfer into healthy admission.
        // Check the existing pairing state BEFORE the transfer. No fallible
        // work, await, callback or formatted body allocation follows the MOVE.
        let empty_emitter = self
            .emitter
            .as_ref()
            .is_some_and(|emitter| !emitter.has_pending_calls());
        *destination = self.completed_turn.take();
        if empty_emitter {
            self.emitter = None;
        }
        Ok(true)
    }

    fn visit_pairing_archive(
        &mut self,
        original: &crate::operation::Operation,
        sink: &mut impl super::archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use super::archive_stdout::unique_storage;
        use crate::failed_evidence::ArchiveError;
        for helper in self.helpers.get_mut() {
            if !helper.physical_supervision_complete()
                || !unique_storage(&mut helper.capture)?.same_instance(original)
            {
                return Err(ArchiveError::Identity);
            }
        }
        let physical_child = match self.proc.as_mut() {
            Some(child) => {
                if !child.physical_supervision_complete()
                    || !unique_storage(&mut child.capture)?.same_instance(original)
                {
                    return Err(ArchiveError::Identity);
                }
                Some(child.physical_child_id.as_ref())
            }
            None => None,
        };
        if let Some(emitter) = self.emitter.as_ref() {
            emitter.visit_pairing_archive(original, physical_child, sink)?;
        }
        Ok(())
    }

    fn cover_pairing_archive(
        &mut self,
        original: &crate::operation::Operation,
        coverage: &mut crate::failed_evidence::EvidenceCoverageBuilder<'_>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.visit_pairing_archive(original, coverage)
    }

    fn encode_pairing_archive<W: std::io::Write>(
        &mut self,
        original: &crate::operation::Operation,
        encoder: &mut crate::failed_evidence::ArchiveEncoder<'_, W>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.visit_pairing_archive(original, encoder)
    }

    fn visit_startup_archive(
        &mut self,
        original: &crate::operation::Operation,
        sink: &mut impl super::archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use super::archive_stdout::unique_storage;
        use crate::failed_evidence::ArchiveError;
        // Exclusive &mut access uses get_mut, never an async mutex guard or
        // callback under a lock. Every retained helper must actually be joined.
        for helper in self.helpers.get_mut() {
            if !helper.physical_supervision_complete()
                || !unique_storage(&mut helper.capture)?.same_instance(original)
            {
                return Err(ArchiveError::Identity);
            }
        }
        let physical_child = match self.proc.as_mut() {
            Some(child) => {
                if !child.physical_supervision_complete()
                    || !unique_storage(&mut child.capture)?.same_instance(original)
                {
                    return Err(ArchiveError::Identity);
                }
                Some(child.physical_child_id.as_ref())
            }
            None => None,
        };
        if let Some(startup) = self.startup.as_ref() {
            startup.visit_archive(original, physical_child, sink)?;
        }
        Ok(())
    }

    fn cover_startup_archive(
        &mut self,
        original: &crate::operation::Operation,
        coverage: &mut crate::failed_evidence::EvidenceCoverageBuilder<'_>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.visit_startup_archive(original, coverage)
    }

    fn encode_startup_archive<W: std::io::Write>(
        &mut self,
        original: &crate::operation::Operation,
        encoder: &mut crate::failed_evidence::ArchiveEncoder<'_, W>,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.visit_startup_archive(original, encoder)
    }

    /// Private concrete factory. Install the exact unboxed value in retained
    /// root storage BEFORE Box allocation, host binding or any native effect.
    /// Existing public constructor/main facade is not redirected by this bridge.
    pub(super) fn original_owner(
        cfg: ClaudeConfig,
        sink: TranscriptSink,
    ) -> super::archive_work::OriginalClaudeOwner {
        super::archive_work::OriginalClaudeOwner::from_concrete(Self::new(cfg, sink))
    }

    pub(super) fn original_configuration_layout(
        &self,
    ) -> Result<ConfigCreationLayout, crate::failed_evidence::ArchiveError> {
        self.cfg.original_creation_layout()
    }

    pub(super) fn original_transcript_layout(
        &self,
    ) -> Result<crate::transcript::TranscriptCreationLayout, crate::failed_evidence::ArchiveError>
    {
        self.sink.original_referenced_layout()
    }

    pub(super) fn pending_transcript_matches(
        &self,
        input: &crate::transcript::PendingTranscriptInputs,
    ) -> bool {
        input.matches_transferred_producer(&self.sink)
    }

    pub(super) fn original_factory_seat(&self) -> cna_core::ids::SeatId {
        self.cfg.seat
    }

    pub(super) fn original_factory_unbound(&self) -> bool {
        self.host.is_none()
            && self.proc.is_none()
            && self.session_id.is_none()
            && self.prompt_lease.is_none()
            && self.emitter.is_none()
            && self.completed_turn.is_none()
            && self.startup.is_none()
            && self.parser.last_text.is_none()
            && self.parser.runtime_operation.is_none()
            && self.idle
            && self
                .helpers
                .try_lock()
                .is_ok_and(|helpers| helpers.is_empty())
    }

    pub fn new(cfg: ClaudeConfig, sink: TranscriptSink) -> Self {
        Self {
            cfg,
            sink,
            proc: None,
            parser: ClaudeParser::default(),
            session_id: None,
            telemetry: super::SessionTelemetry::default(),
            idle: true,
            reported_cost_limit: None,
            host: None,
            helpers: tokio::sync::Mutex::new(Vec::new()),
            prompt_lease: None,
            emitter: None,
            completed_turn: None,
            startup: None,
        }
    }

    fn spawn_session(
        &mut self,
        command: &mut Command,
        context: &crate::host::HostDriverContext,
        permit: &mut dyn crate::host::HostStepLease,
    ) -> Result<(), DriverError> {
        if self.proc.is_some()
            || self
                .startup
                .as_ref()
                .is_none_or(|startup| !startup.operation.same_instance(&context.operation))
        {
            return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                "startup effect lacks its original retained owner".into(),
            )));
        }
        Self::check_native_archive(context, self.cfg.seat).map_err(DriverError::Host)?;
        let seat = self.cfg.seat;
        let result = permit.perform_sync(&mut || {
            // An intervening await/acquire cannot substitute for this fresh effect check.
            Self::check_native_archive(context, seat)?;
            if self.proc.is_some() {
                return Err(crate::host::HostRefusal::Ownership(
                    "startup effect already owns its child".into(),
                ));
            }
            self.proc = Some(
                ChildProc::spawn_session(command, context.operation.clone())
                    .map_err(|e| crate::host::HostRefusal::Ownership(e.to_string()))?,
            );
            // Registered synchronously before returning to the lease, never in a local
            // spawned handle that a post-effect refusal could drop.
            Ok(())
        });
        if let Err(error) = result {
            if self.proc.is_none() {
                // No effect/row was accepted. Charges and operation liability are not
                // refunded; only the unaccepted local descriptor can be discarded.
                self.startup = None;
            }
            return Err(DriverError::Host(error));
        }
        if self.proc.is_none() {
            self.startup = None;
            return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                "startup lease returned without a registered child".into(),
            )));
        }
        self.startup
            .as_mut()
            .expect("retained startup")
            .effect_accepted = true;
        Ok(())
    }

    fn publish_startup(&mut self) -> Result<SessionInfo, DriverError> {
        let startup = self.startup.as_mut().ok_or_else(|| {
            DriverError::Host(crate::host::HostRefusal::Ownership(
                "startup evidence owner is missing".into(),
            ))
        })?;
        if !startup.effect_accepted
            || self
                .proc
                .as_ref()
                .is_none_or(|child| !child.capture_operation().same_instance(&startup.operation))
        {
            return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                "startup evidence is not bound to the accepted original child".into(),
            )));
        }
        if let Some(error) = startup.failure {
            return Err(DriverError::Host(crate::host::operation_refusal(error)));
        }
        let result = (|| {
            startup.operation.check()?;
            let storage = EventStorage(Some(&startup.operation));
            // Keep the canonical descriptor until admission succeeds. This attempted
            // row is an independently charged copy; failure neither refunds nor retries it.
            storage.charge(
                std::mem::size_of::<Vec<TranscriptEntry>>()
                    .checked_add(std::mem::size_of::<TranscriptEntry>()),
            )?;
            let text = storage.copy(&startup.text)?;
            self.sink
                .for_operation(startup.operation.clone())
                .try_emit_batch(self.cfg.seat, vec![TranscriptEntry::System { text }])
        })();
        if let Err(error) = result {
            startup.failure = Some(error);
            return Err(DriverError::Host(crate::host::operation_refusal(error)));
        }
        let startup = self.startup.take().expect("accepted startup owner");
        self.session_id = Some(startup.published_session_id);
        Ok(startup.info) // MOVE only after row admission; no child/reader/storage ACK implied.
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

    pub async fn verify_version(&self) -> Result<String, DriverError> {
        self.preflight_verified(
            &["--version"],
            Some(Duration::from_secs(10)),
            verified_version,
        )
        .await
    }
    /// Check that the configured login is the expected account (`claude auth status`).
    pub async fn verify_account(&self) -> Result<String, DriverError> {
        self.preflight_verified(&["auth", "status"], None, |text, operation| {
            verified_account(text, operation, self.cfg.expected_email.as_deref())
        })
        .await
    }

    fn context(&self) -> Result<&crate::host::HostDriverContext, DriverError> {
        self.host
            .as_ref()
            .ok_or(DriverError::Host(crate::host::HostRefusal::NoAllowance))
    }

    // Creation-time admission is required even when the outer runner has not yet
    // wired a campaign archive. Cleanup/recovery may still borrow legacy contexts;
    // only native entrypoints use this gate. No path I/O or replacement reserve.
    fn check_native_archive(
        context: &crate::host::HostDriverContext,
        seat: cna_core::ids::SeatId,
    ) -> Result<(), crate::host::HostRefusal> {
        let original = &context.operation;
        original.check().map_err(crate::host::operation_refusal)?;
        let plan = original
            .archive_plan()
            .ok_or(crate::host::HostRefusal::NoAllowance)?;
        if context.identity.seat != seat
            || plan.seat != seat
            || plan.epoch != context.identity.epoch
            || context.identity.operation_id != original.id()
        {
            return Err(crate::host::HostRefusal::IdentityChanged);
        }
        // Prove the SAME original creation-time workspace is available without
        // allocating or consuming its cumulative credit. Drop returns that buffer.
        let _chunk = original
            .borrow_archive_chunk(original)
            .map_err(crate::host::operation_refusal)?;
        original.check().map_err(crate::host::operation_refusal)
    }

    async fn helper_owner(
        &self,
        allowance: crate::operation::Allowance,
    ) -> Result<tokio::sync::MutexGuard<'_, Vec<ChildProc>>, DriverError> {
        allowance
            .check()
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        let owner = tokio::time::timeout_at(allowance.deadline(), self.helpers.lock())
            .await
            .map_err(|_| DriverError::Host(crate::host::HostRefusal::Expired))?;
        allowance
            .check()
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        Ok(owner)
    }

    async fn preflight_verified<T: Send>(
        &self,
        args: &[&str],
        local_ceiling: Option<Duration>,
        verify: impl FnOnce(&str, &crate::operation::Operation) -> Result<T, DriverError> + Send,
    ) -> Result<T, DriverError> {
        let context = self.context()?;
        Self::check_native_archive(context, self.cfg.seat).map_err(DriverError::Host)?;
        let context = context.clone();
        let allowance = match local_ceiling {
            Some(ceiling) => context.operation.allowance().capped(ceiling),
            None => context.operation.allowance(),
        };
        allowance
            .check()
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        let mut helpers = self.helper_owner(allowance).await?;
        if !helpers.is_empty() {
            return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                "original preflight helper/evidence remains owned; no redispatch".into(),
            )));
        }
        let mut cmd = Command::new(self.exe()?);
        cmd.args(args).current_dir(&self.cfg.sandbox);
        let env = claude_env(&self.cfg, EventStorage(Some(&context.operation)))
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        apply_seat_env_owned(&mut cmd, &env, &context.operation)
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        let mut permit = context
            .acquire(crate::host::HostStep::PreflightSpawn)
            .map_err(DriverError::Host)?;
        permit
            .perform_sync(&mut || {
                allowance.check().map_err(crate::host::operation_refusal)?;
                Self::check_native_archive(&context, self.cfg.seat)?;
                let mut child = ChildProc::spawn(&mut cmd, context.operation.clone())
                    .map_err(|e| crate::host::HostRefusal::Ownership(e.to_string()))?;
                child.restrict_cleanup_deadline(allowance.deadline());
                child.close_stdin();
                helpers.push(child); // Ownership registered before any await/cancellation boundary.
                Ok(())
            })
            .map_err(DriverError::Host)?;
        let child = helpers.last_mut().expect("owned preflight child");
        let result = async {
            loop {
                context.operation.check().map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
                let line = tokio::select! {
                    result = child.next_line(allowance.deadline()) => result?,
                    () = context.operation.wait_overflow() => return Err(DriverError::Host(crate::host::HostRefusal::CaptureOverflow)),
                };
                let Some(line) = line else { break; };
                child.append_helper_output(&line)?;
            }
            child.finish_until(allowance.deadline()).await
                .map_err(|e| DriverError::Died(format!("preflight cleanup failed: {e}")))?;
            if !child.exit_success()? {
                return Err(DriverError::Isolation("CLI preflight refused".into()));
            }
            Ok(())
        }.await;
        if let Err(error) = result {
            // A dropped output future never loses the helper. If cleanup is unconfirmed its
            // SAME ChildProc and retained readers stay registered for the owning supervisor.
            // Keep the SAME helper and its accumulated output on error, including accepted
            // partial output. Child/reader joins alone cannot erase that evidence.
            let _cleanup = child.kill_until(allowance.deadline()).await;
            return Err(error);
        }
        finish_verified_helper(&mut helpers, &context.operation, allowance, verify)
    }
}

impl super::archive_work::ArchiveWork<ClaudeDriver> {
    fn perform_original_archive(
        &mut self,
        original: &crate::operation::Operation,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.perform(original, ClaudeDriver::publish_original_archive)
    }
}

impl super::archive_work::RetainedArchive<ClaudeDriver> {
    /// Bind only the actual private borrowed publication path. The operator
    /// retains this SAME root; launch/registration is never custody or health.
    pub(super) fn start_original_archive(
        &mut self,
        original: &crate::operation::Operation,
    ) -> Result<bool, crate::failed_evidence::ArchiveError> {
        self.start_std_worker(original, ClaudeDriver::publish_original_archive)
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

/// Count the actual formatted UTF-8 bytes before allocating the derived metadata row.
/// This charges logical bytes and the String header, not allocator capacity or RSS.
fn metadata_text(
    sink: &TranscriptSink,
    args: std::fmt::Arguments<'_>,
) -> Result<String, DriverError> {
    struct Length(usize);
    impl std::fmt::Write for Length {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0 = self.0.checked_add(text.len()).ok_or(std::fmt::Error)?;
            Ok(())
        }
    }
    let mut length = Length(0);
    std::fmt::write(&mut length, args)
        .map_err(|_| DriverError::Protocol("metadata length overflow".into()))?;
    sink.charge_allocation(length.0.saturating_add(std::mem::size_of::<String>()))
        .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
    Ok(args.to_string())
}

/// A refused metadata row leaves both the retained parser event and published session state
/// untouched. The caller has already installed the reader's exact known/unknown origin.
fn capture_session_event(
    event: &StreamEvent,
    emitter: &mut EntryEmitter,
    sink: &TranscriptSink,
    telemetry: &mut super::SessionTelemetry,
    session: &mut Option<String>,
) -> Result<(), DriverError> {
    match event {
        StreamEvent::Init {
            tools,
            mcp_connected,
            session_id,
            model,
            cli_version,
        } => {
            check_isolation(tools, *mcp_connected)?;
            sink.charge_copy(&(session_id, model, cli_version, tools))
                .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
            // The serialized copy estimate also needs the retained containers' headers.
            let headers = std::mem::size_of::<String>()
                .saturating_add(2 * std::mem::size_of::<Option<String>>())
                .saturating_add(std::mem::size_of::<Vec<String>>())
                .saturating_add(tools.len().saturating_mul(std::mem::size_of::<String>()));
            sink.charge_allocation(headers)
                .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
            let text = metadata_text(
                sink,
                format_args!(
                    "CLI metadata: model {}, version {}",
                    model.as_deref().unwrap_or("unknown"),
                    cli_version.as_deref().unwrap_or("unknown"),
                ),
            )?;
            emitter.try_emit(TranscriptEntry::System { text })?;
            telemetry.model = model.clone();
            telemetry.cli_version = cli_version.clone();
            telemetry.tools = tools.clone();
            if !session_id.is_empty() {
                *session = Some(session_id.clone());
            }
        }
        StreamEvent::Context(tokens) => telemetry.context_tokens = Some(*tokens),
        StreamEvent::Compacted {
            trigger,
            pre_tokens,
        } => {
            let compactions = telemetry
                .compactions
                .checked_add(1)
                .ok_or_else(|| DriverError::Protocol("compaction count overflow".into()))?;
            let text = metadata_text(
                sink,
                format_args!("CLI compacted context ({trigger}, pre_tokens {pre_tokens:?})"),
            )?;
            emitter.try_emit(TranscriptEntry::System { text })?;
            telemetry.compactions = compactions;
            telemetry.context_tokens = None;
        }
        StreamEvent::Entry(entry) => {
            sink.charge_copy(entry)
                .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
            emitter.try_emit(entry.clone())?;
        }
        _ => {}
    }
    Ok(())
}

#[async_trait]
impl SeatDriver for ClaudeDriver {
    fn take_captured_outcome(
        &mut self,
        original: &crate::operation::Operation,
    ) -> Result<Option<TurnOutcome>, DriverError> {
        if self.completed_turn.is_none() {
            return Ok(None);
        }
        // SAME checked handoff as the retained controller: host identity, ALL
        // saved helpers, physical child and parser original; pairing is checked
        // BEFORE the MOVE. No callback/await/format/clone follows a taken body.
        // The journal retains this return before observation. Its caller slot
        // and these overlapping headers are original-creation obligations.
        let mut captured = None;
        match self.handoff_original_captured(original, original, &mut captured) {
            Ok(true) => Ok(captured),
            // The shared check emits only fixed Identity here. Expose a fixed
            // fail-closed projection without allocating diagnostic text; keep
            // the earlier supervisor error and raw/correlation/pipe owners.
            Ok(false) | Err(_) => Err(DriverError::Host(crate::host::HostRefusal::Unknown)),
        }
    }

    fn kind(&self) -> CliKind {
        CliKind::Claude
    }

    fn set_host_context(
        &mut self,
        context: crate::host::HostDriverContext,
    ) -> Result<(), DriverError> {
        self.check_host_context(&context)?;
        self.host = Some(context);
        Ok(())
    }

    fn start_evidence(&self) -> Option<crate::operation::StartEvidence> {
        self.host.as_ref().map(|h| h.operation.start_evidence())
    }

    fn set_reported_cost_limit(&mut self, remaining_usd: f64) -> Result<(), DriverError> {
        if self.proc.is_some() || !remaining_usd.is_finite() || remaining_usd <= 0. {
            return Err(DriverError::Isolation(
                "positive finite cost limit requires a parked process".into(),
            ));
        }
        self.reported_cost_limit = Some(remaining_usd);
        Ok(())
    }
    async fn start(&mut self, resume_session: Option<&str>) -> Result<SessionInfo, DriverError> {
        self.start_original_owner(resume_session).await
    }

    fn telemetry(&self) -> super::SessionTelemetry {
        self.telemetry.clone()
    }

    async fn run_turn(
        &mut self,
        prompt: &str,
        limit: Duration,
    ) -> Result<TurnOutcome, DriverError> {
        self.run_turn_original_owner(prompt, limit).await
    }

    fn session_id(&self) -> Option<String> {
        self.session_id.clone()
    }

    fn is_alive(&mut self) -> bool {
        self.proc.as_mut().is_some_and(ChildProc::is_running)
    }

    async fn stop(&mut self) -> Result<(), DriverError> {
        self.stop_original_owner().await
    }
}

impl ClaudeDriver {
    // SAME method bodies, shared by existing trait adapters. Creating these
    // concrete unpolled futures allocates nothing and performs no native effect.
    pub(super) async fn start_original_owner(
        &mut self,
        resume_session: Option<&str>,
    ) -> Result<SessionInfo, DriverError> {
        let context = self.context()?;
        if self.emitter.is_some()
            || self.completed_turn.is_some()
            || self.startup.is_some()
            || self.prompt_lease.is_some()
            || self.parser.last_text.is_some()
            || self.parser.runtime_operation.is_some()
        {
            return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                "previous captured turn remains owned; no redispatch".into(),
            )));
        }
        Self::check_native_archive(context, self.cfg.seat).map_err(DriverError::Host)?;
        let context = context.clone();
        context
            .operation
            .check()
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        if let Some(p) = &mut self.proc {
            p.kill()
                .await
                .map_err(|e| DriverError::Died(format!("CLI termination failed: {e}")))?;
            if p.queued_frames() != 0 {
                return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                    "joined previous child retains accepted stdout frames".into(),
                )));
            }
        }
        self.proc = None;
        self.parser = ClaudeParser::default();
        self.telemetry = super::SessionTelemetry::default();
        self.idle = true;
        let io = |e| DriverError::Spawn {
            cli: "claude",
            source: e,
        };
        std::fs::create_dir_all(&self.cfg.sandbox).map_err(io)?;
        if std::fs::read_dir(&self.cfg.sandbox)
            .map_err(io)?
            .next()
            .is_some()
        {
            return Err(DriverError::Isolation(
                "seat working directory must be empty".into(),
            ));
        }
        std::fs::create_dir_all(&self.cfg.run_dir).map_err(io)?;
        let mcp_path = self.cfg.run_dir.join(format!("{}.mcp.json", self.cfg.seat));
        std::fs::write(&mcp_path, mcp_config_json(&self.cfg.mcp_url).to_string()).map_err(io)?;
        let version = self.verify_version().await?;
        let email = self.verify_account().await?;

        let new_id = if resume_session.is_none() {
            context
                .operation
                .charge(36 + std::mem::size_of::<String>())
                .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
            Some(uuid::Uuid::new_v4().to_string())
        } else {
            None
        };
        let (arg, id) = match resume_session {
            Some(id) => (SessionArg::Resume(id), id),
            None => {
                let id = new_id.as_deref().expect("new session id");
                (SessionArg::New(id), id)
            }
        };
        let args = build_claude_args(
            &self.cfg,
            &mcp_path,
            arg,
            self.reported_cost_limit,
            EventStorage(Some(&context.operation)),
        )
        .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        let mut cmd = Command::new(self.exe()?);
        cmd.args(args).current_dir(&self.cfg.sandbox);
        let env = claude_env(&self.cfg, EventStorage(Some(&context.operation)))
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        apply_seat_env_owned(&mut cmd, &env, &context.operation)
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        let mut permit = context
            .acquire(if resume_session.is_some() {
                crate::host::HostStep::Resume
            } else {
                crate::host::HostStep::SessionSpawn
            })
            .map_err(DriverError::Host)?;
        self.startup = Some(StartupCapture::prepare(
            &context.operation,
            id,
            &self.cfg.model,
            version,
            &email,
            resume_session.is_some(),
        )?);
        self.spawn_session(&mut cmd, &context, permit.as_mut())?;
        // Claude reports `init` with the first turn, so the isolation check happens in run_turn.
        self.publish_startup()
    }

    pub(super) async fn run_turn_original_owner(
        &mut self,
        prompt: &str,
        limit: Duration,
    ) -> Result<TurnOutcome, DriverError> {
        let context = self.context()?;
        if self.emitter.is_some()
            || self.completed_turn.is_some()
            || self.prompt_lease.is_some()
            || self.startup.is_some()
            || self.parser.last_text.is_some()
            || self.parser.runtime_operation.is_some()
        {
            return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                "previous captured turn remains owned; no redispatch".into(),
            )));
        }
        Self::check_native_archive(context, self.cfg.seat).map_err(DriverError::Host)?;
        let context = context.clone();
        // Existing relative argument may only shorten this operation's already-stamped bound.
        let deadline = context.operation.allowance().capped(limit).deadline();
        let lease = context.acquire_prompt().map_err(DriverError::Host)?;
        lease.check().map_err(DriverError::Host)?;
        self.prompt_lease = Some(lease);
        let lease = self.prompt_lease.as_ref().expect("owned prompt lease");
        self.idle = false;
        let sink = self.sink.for_operation(context.operation.clone());
        self.emitter = Some(EntryEmitter::new(self.cfg.seat, sink.clone()));
        let emitter = self.emitter.as_mut().expect("retained entry emitter");
        let proc = self
            .proc
            .as_mut()
            .ok_or_else(|| DriverError::Protocol("session not started".into()))?;
        proc.bind_operation(context.operation.clone())?;
        context
            .operation
            .charge(proc.physical_child_id().len() + std::mem::size_of::<String>())
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        let physical_child_id = proc.physical_child_id().to_owned();
        if self.parser.runtime_operation.is_some() {
            return Err(DriverError::Host(crate::host::HostRefusal::HostUnknown));
        }
        let input_uuid = context
            .operation
            .prepare_claude_input()
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        // ASCII produced at original creation; borrowed view allocates nothing.
        let input_uuid = std::str::from_utf8(&input_uuid).expect("creation-time lowercase UUID");
        self.parser.runtime_operation = Some(context.operation.clone());
        #[derive(serde::Serialize)]
        struct UserMessage<'a> {
            role: &'static str,
            content: &'a str,
        }
        #[derive(serde::Serialize)]
        struct UserFrame<'a> {
            r#type: &'static str,
            uuid: &'a str,
            message: UserMessage<'a>,
        }
        let frame = UserFrame {
            r#type: "user",
            uuid: input_uuid,
            message: UserMessage {
                role: "user",
                content: prompt,
            },
        };
        let bytes =
            crate::operation::encoded_len(&frame, crate::operation::FRAME_BYTES).map_err(|_| {
                DriverError::Host(crate::host::operation_refusal(context.operation.overflow()))
            })?;
        context
            .operation
            .charge(bytes)
            .map_err(|e| DriverError::Host(crate::host::operation_refusal(e)))?;
        let line =
            serde_json::to_string(&frame).map_err(|e| DriverError::Protocol(e.to_string()))?;
        proc.send_line(&line, deadline, lease.as_ref()).await?;
        let real_session = &mut self.session_id;
        let outcome = pump_turn(proc, &mut self.parser, deadline, |event, origin| {
            lease.check().map_err(DriverError::Host)?;
            emitter.set_origin(origin)?;
            match event {
                StreamEvent::ModelStart { session_id, frame } => {
                    if real_session.as_ref() == Some(session_id)
                        && let Some(origin) = origin
                    {
                        context
                            .operation
                            .confirm_child(origin, &physical_child_id, *frame);
                    }
                }
                _ => {
                    capture_session_event(event, emitter, &sink, &mut self.telemetry, real_session)?
                }
            }
            Ok(())
        })
        .await?;
        // Move the actual outcome into the driver before the fallible/cancelable settlement
        // await. A lost settlement ACK cannot erase usage or invent a healthy completion.
        self.completed_turn = Some(outcome);
        if emitter.has_pending_calls() {
            return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                "terminal frame leaves unpaired accepted tool calls".into(),
            )));
        }
        // Actual terminal is already consumed and its outcome owned above. The
        // original private gate installs its request before waiting for BOTH
        // readers. Cancellation/refusal retains outcome/request/root; ACKs are
        // not child/reader joins or authority to rebind/write another prompt.
        let pause = proc.pause_after_terminal(&context.operation);
        if std::mem::size_of_val(&pause) > super::reader_handoff::WAIT_BYTES {
            return Err(DriverError::Host(crate::host::HostRefusal::Unknown));
        }
        pause.await?;
        let settle = self.settle_captured_terminal(&context.operation);
        if std::mem::size_of_val(&settle) > super::reader_handoff::WAIT_BYTES {
            return Err(DriverError::Host(crate::host::HostRefusal::Unknown));
        }
        settle.await
    }

    /// Concrete unboxed cleanup future, shared with the existing trait adapter.
    /// Retained archive callers measure it before polling and keep the canonical
    /// restore guard across every await. This supplies no healthy rebind witness.
    pub(super) async fn stop_original_owner(&mut self) -> Result<(), DriverError> {
        // Retained output from the last helper must not prevent attempting the
        // other original helpers/session. Actual joins still release nothing.
        self.supervise_retained_children().await.map_err(|error| {
            DriverError::Died(format!(
                "original child supervision retained unsettled: {error}"
            ))
        })?;
        if self.proc.as_ref().is_some_and(|child| {
            child.handoff_request.is_some()
                || child
                    .handoff
                    .as_ref()
                    .is_some_and(|gate| gate.boundary_active())
        }) {
            // Original joins are attempted first. Refuse before any helper/root
            // owner removal or prompt settlement, even with no queued bodies.
            return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                "Unsettled: original reader handoff/request disposition remains owned".into(),
            )));
        }
        {
            let mut helpers = self.helpers.try_lock().map_err(|_| {
                DriverError::Host(crate::host::HostRefusal::Ownership(
                    "original preflight helper owner remains borrowed".into(),
                ))
            })?;
            while let Some(helper) = helpers.last_mut() {
                if helper.queued_frames() != 0 || helper.has_retained_helper_output() {
                    return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                        if helper.physical_supervision_complete() {
                            "Unsettled: physically joined helper retains original output evidence"
                        } else {
                            "Unsettled: original helper supervision/evidence remains owned"
                        }
                        .into(),
                    )));
                }
                helpers.pop();
            }
        }
        if let Some(p) = &mut self.proc {
            if p.queued_frames() != 0 {
                return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                    if p.physical_supervision_complete() {
                        "Unsettled: physically joined child retains original stdout evidence"
                    } else {
                        "Unsettled: original child supervision/stdout remains owned"
                    }
                    .into(),
                )));
            }
        }
        if self.emitter.is_some()
            || self.completed_turn.is_some()
            || self.startup.is_some()
            || self.parser.last_text.is_some()
            || self.parser.runtime_operation.is_some()
        {
            return Err(DriverError::Host(crate::host::HostRefusal::Ownership(
                "Unsettled: original captured turn/startup/pairing evidence remains foreground-owned".into(),
            )));
        }
        if let Some(lease) = &self.prompt_lease {
            lease.settle().await.map_err(DriverError::Host)?;
        }
        self.prompt_lease = None;
        // Physical joins alone do not relinquish the canonical child evidence. Keep the
        // SAME child/diagnostics on foreground-evidence refusal or canceled/failed actor ACK.
        // Removal follows all of those dispositions, never the earlier process wait.
        self.proc = None;
        Ok(())
    }
}

/// Convenience for supervisors that hold drivers behind a box.
pub fn boxed(cfg: ClaudeConfig, sink: TranscriptSink) -> Box<dyn SeatDriver> {
    Box::new(ClaudeDriver::new(cfg, sink))
}

#[allow(dead_code)]
fn _assert_arc(_: Arc<()>) {}

/// Borrowed source only: successful streaming leaves every canonical owner,
/// prompt lease and original operation credit in place. Publication and accepted
/// custody remain separate; physical joins do not authorize healthy replacement.
impl crate::failed_evidence::JoinedArchiveSource for ClaudeDriver {
    fn check_original_supervision(
        &mut self,
        original: &crate::operation::Operation,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        self.original_archive_coverage(original).map(|_| ())
    }

    fn stream<W: std::io::Write>(
        &mut self,
        original: &crate::operation::Operation,
        coverage: crate::failed_evidence::EvidenceCoverage,
        output: &mut W,
    ) -> Result<crate::failed_evidence::ArchiveMeasurement, crate::failed_evidence::ArchiveError>
    {
        use crate::failed_evidence::{ArchiveEncoder, ArchiveError, EvidenceCoverageBuilder};
        // Both fixed pass states were reserved at original operation creation.
        // This check is not proof of library/custom writer/future allocation fit.
        let fixed = std::mem::size_of::<EvidenceCoverageBuilder<'_>>()
            + std::mem::size_of::<ArchiveEncoder<'_, W>>();
        if fixed > crate::failed_evidence::FIXED_ARCHIVE_WORKSPACE {
            return Err(ArchiveError::Fit);
        }
        // Never trust a caller's descriptor or write even a header until ALL
        // original owners, opaque errors and separately captured outcome gates
        // have been checked. No await can change ownership between these passes.
        if self.original_archive_coverage(original)? != coverage {
            return Err(ArchiveError::Identity);
        }
        let mut encoder = ArchiveEncoder::new(original, coverage, output)?;
        self.visit_original_archive(original, &mut encoder)?;
        encoder.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::super::apply_seat_env;
    use super::*;
    use tokio::time::Instant;

    #[test]
    fn correlation_refusal_keeps_actual_raw_original_and_bypasses_no_normal_parser() {
        let original = archive_startup_operation();
        let baseline = original.captured_bytes();
        original.prepare_claude_input().unwrap();
        assert_eq!(original.captured_bytes(), baseline); // Creation admission, no late charge.
        let mut parser = ClaudeParser {
            runtime_operation: Some(original.clone()),
            ..ClaudeParser::default()
        };
        let text = r#"{"type":"result","subtype":"success","user_message_uuid":"stale","result":"must stay operator-only"}"#.to_owned();
        let pointer = text.as_ptr();
        let mut pending = Some(super::super::ChildFrame {
            text,
            origin: None,
            operation: original.clone(),
        });
        assert!(matches!(
            super::super::decode_pending_frame(&mut pending, &mut parser),
            Err(DriverError::Host(crate::host::HostRefusal::HostUnknown))
        ));
        let raw = pending.as_ref().unwrap();
        assert_eq!(raw.text.as_ptr(), pointer);
        assert!(raw.operation.same_instance(&original));
        assert!(raw.origin.is_none());
        assert!(!parser.init_seen);
        assert!(parser.last_text.is_none());
        assert!(parser.feed(&raw.text).is_empty()); // No unowned fallback bypass.
        assert!(
            parser
                .runtime_operation
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        assert!(original.prepare_claude_input().is_err()); // No renewed input/retry.
        let foreign = archive_startup_operation();
        let foreign_credit = foreign.captured_bytes();
        assert!(parser.feed_owned(&raw.text, &foreign).is_err());
        assert_eq!(foreign.captured_bytes(), foreign_credit);
        assert_eq!(pending.as_ref().unwrap().text.as_ptr(), pointer);
    }

    #[test]
    fn correlation_creation_uuid_is_unique_and_overflow_cannot_prepare() {
        let original = archive_startup_operation();
        let foreign = archive_startup_operation();
        assert_ne!(
            original.prepare_claude_input().unwrap(),
            foreign.prepare_claude_input().unwrap()
        );
        let overflowed = archive_startup_operation();
        overflowed
            .charge(crate::operation::OPERATION_BYTES - overflowed.captured_bytes())
            .unwrap();
        assert!(overflowed.charge(1).is_err());
        assert_eq!(
            overflowed.prepare_claude_input(),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert_eq!(
            overflowed.captured_bytes(),
            crate::operation::OPERATION_BYTES
        );
    }

    #[test]
    fn runtime_ack_has_no_turn_event_and_terminal_does_not_clear_original_association() {
        let original = archive_startup_operation();
        let uuid = original.prepare_claude_input().unwrap();
        let uuid = std::str::from_utf8(&uuid).unwrap();
        let mut parser = ClaudeParser {
            runtime_operation: Some(original.clone()),
            ..ClaudeParser::default()
        };
        let ack = json!({"type":"user","uuid":uuid,"session_id":"fixture",
            "parent_tool_use_id":null,"isReplay":true,
            "message":{"role":"user","content":"fixture"}})
        .to_string();
        assert!(parser.feed_owned(&ack, &original).unwrap().is_empty());
        assert!(parser.last_text.is_none());
        let result = json!({"type":"result","subtype":"success",
            "user_message_uuid":uuid,"user_message_uuids":[uuid],"result":"fixture"})
        .to_string();
        assert!(
            parser
                .feed_owned(&result, &original)
                .unwrap()
                .iter()
                .any(|event| matches!(event, StreamEvent::TurnDone(_)))
        );
        assert!(
            parser
                .runtime_operation
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        assert!(original.prepare_claude_input().is_err());
        assert!(parser.feed_owned(&result, &original).is_err());
        // No actor/lease/journal/CAS/child/readers/archive or healthy witness.
    }

    fn archive_startup_operation() -> crate::operation::Operation {
        let root = std::env::temp_dir().join("synthetic-startup-archive-no-io");
        archive_operation_at(&root)
    }

    #[test]
    fn correlation_archive_checkout_restores_same_metadata_on_writer_refusal_and_overflow() {
        use super::super::archive_stdout::RecordSink;
        use crate::failed_evidence::{ArchiveError, EvidenceRecord};
        struct Refusing<'a> {
            original: &'a crate::operation::Operation,
            payload: Option<Value>,
        }
        impl RecordSink for Refusing<'_> {
            fn bytes(&mut self, _: EvidenceRecord<'_>, _: &[u8]) -> Result<(), ArchiveError> {
                panic!("metadata must use borrowed JSON");
            }
            fn json<T: serde::Serialize + ?Sized>(
                &mut self,
                record: EvidenceRecord<'_>,
                value: &T,
            ) -> Result<(), ArchiveError> {
                assert!(record.operation.same_instance(self.original));
                assert_eq!(record.part, "claude_input_correlation");
                assert!(record.origin.is_none());
                assert!(record.physical_child.is_none());
                // Reentrant access refuses immediately: no lock across callback,
                // no metadata clone or second checkout, even after overflow.
                assert!(
                    self.original
                        .borrow_claude_correlation(self.original)
                        .is_err()
                );
                assert_eq!(
                    self.original.validate_claude_frame(&Value::Null),
                    Err(super::super::claude_correlation::Refusal::Unsupported)
                );
                self.payload = Some(serde_json::to_value(value).unwrap()); // Test output only.
                Err(ArchiveError::Filesystem)
            }
        }
        let original = archive_startup_operation();
        let uuid = original.prepare_claude_input().unwrap();
        original.refuse_claude_frame();
        let replacement = archive_startup_operation();
        assert_eq!(original.id(), replacement.id());
        assert!(original.borrow_claude_correlation(&replacement).is_err());
        original
            .charge(crate::operation::OPERATION_BYTES - original.captured_bytes())
            .unwrap();
        assert!(original.charge(1).is_err());
        let before = original.captured_bytes();
        let mut sink = Refusing {
            original: &original,
            payload: None,
        };
        {
            let lease = original.borrow_claude_correlation(&original).unwrap();
            assert!(matches!(
                lease.value().visit_archive(&original, true, &mut sink),
                Err(ArchiveError::Filesystem)
            ));
        }
        let first = sink.payload.take().unwrap();
        {
            let lease = original.borrow_claude_correlation(&original).unwrap();
            assert!(
                lease
                    .value()
                    .visit_archive(&original, true, &mut sink)
                    .is_err()
            );
        }
        assert_eq!(sink.payload.as_ref().unwrap(), &first);
        assert_eq!(first["expected"], json!(uuid.as_slice()));
        assert_eq!(first["phase"], json!({"failed":"malformed"}));
        for name in ["init_seen", "assistant_seen", "partial_seen"] {
            assert_eq!(first[name], false);
        }
        assert_eq!(
            original.validate_claude_frame(&Value::Null),
            Err(super::super::claude_correlation::Refusal::Malformed)
        );
        assert_eq!(original.captured_bytes(), before); // No refund, late reserve or settlement.
    }

    #[test]
    fn correlation_archive_unwind_restores_original_and_parser_does_not_release_it() {
        use crate::failed_evidence::{ArchiveError, EvidenceCoverageBuilder};
        let original = archive_startup_operation();
        let uuid = original.prepare_claude_input().unwrap();
        let before = original.captured_bytes();
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _lease = original.borrow_claude_correlation(&original).unwrap();
            panic!("synthetic synchronous inspection unwind");
        }));
        assert!(interrupted.is_err());
        let ack = json!({"type":"user","uuid":std::str::from_utf8(&uuid).unwrap(),
            "session_id":"fixture","parent_tool_use_id":null,"isReplay":true,
            "message":{"role":"user","content":"fixture"}});
        assert!(matches!(
            original.validate_claude_frame(&ack),
            Ok(super::super::claude_correlation::Frame::ReplayAck)
        ));
        let parser = ClaudeParser {
            runtime_operation: Some(original.clone()),
            ..ClaudeParser::default()
        };
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        assert!(matches!(
            parser.visit_archive(&original, None, &mut coverage),
            Err(ArchiveError::Identity)
        )); // No fabricated physical join witness.
        let physical = Some("33333333-3333-4333-8333-333333333333");
        parser
            .visit_archive(&original, physical, &mut coverage)
            .unwrap();
        original
            .borrow_claude_correlation(&original)
            .unwrap()
            .value()
            .visit_archive(&original, true, &mut coverage)
            .unwrap();
        let first = coverage.finish().unwrap();
        let mut again = EvidenceCoverageBuilder::new(&original).unwrap();
        parser
            .visit_archive(&original, physical, &mut again)
            .unwrap();
        original
            .borrow_claude_correlation(&original)
            .unwrap()
            .value()
            .visit_archive(&original, true, &mut again)
            .unwrap();
        assert_eq!(again.finish().unwrap(), first);
        let replacement = archive_startup_operation();
        let mut foreign = EvidenceCoverageBuilder::new(&replacement).unwrap();
        assert!(
            parser
                .visit_archive(&replacement, physical, &mut foreign)
                .is_err()
        );
        assert!(
            parser
                .runtime_operation
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        assert_eq!(original.captured_bytes(), before);
        // Exercise the actual driver traversal for never-armed metadata too.
        // No physical owner exists in this synthetic no-effect fixture.
        let unused = archive_startup_operation();
        let mut driver = ClaudeDriver::new(
            argument_config(),
            TranscriptSink::new(crate::transcript::LocalTranscript::detached()),
        );
        let mut coverage = EvidenceCoverageBuilder::new(&unused).unwrap();
        driver.visit_parser_archive(&unused, &mut coverage).unwrap();
        let mut output = Vec::new(); // Test reference only, no archive file.
        let mut encoder = crate::failed_evidence::ArchiveEncoder::new(
            &unused,
            coverage.finish().unwrap(),
            &mut output,
        )
        .unwrap();
        driver.visit_parser_archive(&unused, &mut encoder).unwrap();
        encoder.finish().unwrap();
        let row: Value =
            serde_json::from_slice(output.split(|b| *b == b'\n').nth(1).unwrap()).unwrap();
        assert_eq!(row["evidence"]["part"], "claude_input_correlation");
        assert!(row["evidence"]["physical_child"].is_null());
        assert!(row["evidence"]["origin"].is_null());
        assert_eq!(row["payload"]["phase"], "unwritten");
        assert_eq!(row["payload"]["runtime_associated"], false);
        assert_eq!(row["payload"]["expected"].as_array().unwrap().len(), 36);
        // Pure synthetic enumeration, not actual child/readers/custody/healthy acceptance.
    }

    fn archive_operation_at(root: &std::path::Path) -> crate::operation::Operation {
        crate::failed_evidence::CampaignArchive::new(
            &root.join("campaigns/11111111-1111-4111-8111-111111111111.sqlite"),
            &root.join("sandbox"),
            &root.join("run"),
            &[root.join("isolation")],
        )
        .unwrap()
        .operation(
            "axis.commander".parse().unwrap(),
            7,
            "22222222-2222-4222-8222-222222222222".into(),
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(60)))
                .unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn driver_pairing_archive_cannot_infer_original_child_from_cached_origin() {
        use crate::failed_evidence::{ArchiveEncoder, EvidenceCoverageBuilder};
        let original = archive_startup_operation();
        let child = "33333333-3333-4333-8333-333333333333";
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached())
            .for_operation(original.clone());
        let mut emitter = EntryEmitter::new("axis.commander".parse().unwrap(), sink.clone());
        let origin = original.next_child_origin(child).unwrap();
        emitter.set_origin(Some(&origin)).unwrap();
        emitter
            .try_emit(TranscriptEntry::ToolCall {
                call_id: "original-call".into(),
                tool: "submit".into(),
                args: json!({"decision_id": "retained-decision"}),
            })
            .unwrap();
        sink.flush_confirmed().await.unwrap();
        let pointer = emitter.calls["original-call"].args["decision_id"]
            .as_str()
            .unwrap()
            .as_ptr();
        // Storage-only expected descriptors, never a child/reader join witness.
        let mut expected = EvidenceCoverageBuilder::new(&original).unwrap();
        emitter
            .visit_pairing_archive(&original, Some(child), &mut expected)
            .unwrap();
        let expected = expected.finish().unwrap();
        let mut driver = ClaudeDriver::new(argument_config(), sink.clone());
        driver.emitter = Some(emitter);
        assert!(driver.proc.is_none());
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        assert!(
            driver
                .cover_pairing_archive(&original, &mut coverage)
                .is_err()
        );
        assert!(coverage.finish().is_err());
        let mut output = Vec::new();
        let mut encoder = ArchiveEncoder::new(&original, expected, &mut output).unwrap();
        assert!(
            driver
                .encode_pairing_archive(&original, &mut encoder)
                .is_err()
        );
        assert!(encoder.finish().is_err());
        assert_eq!(output.iter().filter(|b| **b == b'\n').count(), 1); // Header only.
        let emitter = driver.emitter.as_ref().unwrap();
        assert!(emitter.has_pending_calls());
        assert_eq!(
            emitter.calls["original-call"].args["decision_id"]
                .as_str()
                .unwrap()
                .as_ptr(),
            pointer
        );
        assert!(
            emitter.calls["original-call"]
                .operation
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[test]
    fn startup_archive_borrows_all_descriptors_and_keeps_failed_original_instance() {
        use crate::failed_evidence::{ArchiveEncoder, EvidenceCoverageBuilder};
        let original = archive_startup_operation();
        let mut startup = StartupCapture::prepare(
            &original,
            "controller-only-session",
            "model λ",
            "2.1.277".into(),
            "synthetic@example.invalid",
            false,
        )
        .unwrap();
        startup.failure = Some(crate::operation::OperationError::CaptureOverflow);
        let text_pointer = startup.text.as_ptr();
        let info_pointer = startup.info.session_id.as_ptr();
        let published_pointer = startup.published_session_id.as_ptr();
        let charged = original.captured_bytes();
        original.overflow();
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        startup
            .visit_archive(&original, None, &mut coverage)
            .unwrap();
        let coverage = coverage.finish().unwrap();
        let mut output = Vec::new(); // synthetic serialized reference only
        let mut encoder = ArchiveEncoder::new(&original, coverage, &mut output).unwrap();
        startup
            .visit_archive(&original, None, &mut encoder)
            .unwrap();
        let measurement = encoder.finish().unwrap();
        let records: Vec<Value> = output
            .split(|byte| *byte == b'\n')
            .filter(|row| !row.is_empty())
            .map(|row| serde_json::from_slice(row).unwrap())
            .collect();
        assert_eq!(records.len(), 4);
        assert_eq!(
            records[2]["payload"]["session_id"],
            "controller-only-session"
        );
        assert_eq!(
            records[3]["payload"],
            json!(["controller-only-session", false, "capture_overflow"])
        );
        assert!(
            records[1..]
                .iter()
                .all(|row| row["evidence"]["origin"].is_null()
                    && row["evidence"]["physical_child"].is_null())
        );
        original
            .archive_plan()
            .unwrap()
            .verify_stream(
                &original,
                coverage,
                measurement,
                &mut std::io::Cursor::new(&output),
            )
            .unwrap();
        assert_eq!(startup.text.as_ptr(), text_pointer);
        assert_eq!(startup.info.session_id.as_ptr(), info_pointer);
        assert_eq!(startup.published_session_id.as_ptr(), published_pointer);
        assert_eq!(original.captured_bytes(), charged);
        assert!(startup.operation.same_instance(&original));
    }

    #[test]
    fn startup_archive_refuses_same_text_replacement_or_missing_effect_child_before_suffix() {
        use crate::failed_evidence::{ArchiveEncoder, EvidenceCoverageBuilder};
        let original = archive_startup_operation();
        let replacement = archive_startup_operation();
        let mut startup = StartupCapture::prepare(
            &original,
            "controller-not-child",
            "model",
            "2.1.277".into(),
            "",
            true,
        )
        .unwrap();
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        assert!(
            startup
                .visit_archive(&replacement, None, &mut coverage)
                .is_err()
        );
        startup
            .visit_archive(&original, None, &mut coverage)
            .unwrap();
        let coverage = coverage.finish().unwrap();
        let mut output = Vec::new();
        let mut encoder = ArchiveEncoder::new(&original, coverage, &mut output).unwrap();
        startup.effect_accepted = true;
        assert!(
            startup
                .visit_archive(&original, None, &mut encoder)
                .is_err()
        );
        drop(encoder);
        assert_eq!(output.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert_eq!(startup.info.session_id, "controller-not-child");
        assert!(startup.effect_accepted);
    }

    #[test]
    fn driver_startup_archive_requires_actual_effect_child_without_rewriting_descriptor() {
        let original = archive_startup_operation();
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let mut driver = ClaudeDriver::new(argument_config(), sink);
        let mut startup = StartupCapture::prepare(
            &original,
            "unpaired-controller",
            "model",
            "2.1.277".into(),
            "",
            false,
        )
        .unwrap();
        startup.effect_accepted = true;
        let pointer = startup.text.as_ptr();
        driver.startup = Some(startup);
        let mut coverage = crate::failed_evidence::EvidenceCoverageBuilder::new(&original).unwrap();
        assert!(
            driver
                .cover_startup_archive(&original, &mut coverage)
                .is_err()
        );
        assert!(driver.proc.is_none());
        assert!(driver.helpers.get_mut().is_empty());
        assert_eq!(driver.startup.as_ref().unwrap().text.as_ptr(), pointer);
        assert!(driver.startup.as_ref().unwrap().effect_accepted);
        // Missing child is uncertainty, never inferred zero-work or healthy stop.
    }

    fn refuse_derived_frame(line: &str, init_seen: bool) {
        let probe_operation = metadata_operation();
        let mut probe = ClaudeParser {
            runtime_operation: None,
            init_seen,
            last_text: Some("previous accepted text".into()),
            last_text_operation: Some(probe_operation.clone()),
        };
        let probe_operation_baseline = probe_operation.captured_bytes();
        probe.feed_owned(line, &probe_operation).unwrap();
        let required = probe_operation.captured_bytes() - probe_operation_baseline;
        let tree_probe = metadata_operation();
        let tree_probe_baseline = tree_probe.captured_bytes();
        super::super::charged_json::decode(line, &tree_probe)
            .unwrap()
            .unwrap();
        assert!(tree_probe.captured_bytes() - tree_probe_baseline < required);
        let original = metadata_operation();
        original.possible_start();
        original
            .charge(crate::operation::OPERATION_BYTES - original.captured_bytes() - required + 1)
            .unwrap();
        let text = line.to_owned();
        let raw_pointer = text.as_ptr();
        let mut pending = Some(super::super::ChildFrame {
            text,
            origin: None,
            operation: original.clone(),
        });
        let mut parser = ClaudeParser {
            runtime_operation: None,
            init_seen,
            last_text: Some("previous accepted text".into()),
            last_text_operation: Some(original.clone()),
        };
        let prior_pointer = parser.last_text.as_ref().unwrap().as_ptr();
        assert!(matches!(
            super::super::decode_pending_frame(&mut pending, &mut parser),
            Err(DriverError::Host(_))
        ));
        assert_eq!(parser.init_seen, init_seen);
        assert_eq!(parser.last_text.as_ref().unwrap().as_ptr(), prior_pointer);
        assert_eq!(parser.last_text.as_deref(), Some("previous accepted text"));
        assert!(
            parser
                .last_text_operation
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        let retained = pending.as_ref().unwrap();
        assert_eq!(retained.text.as_ptr(), raw_pointer);
        assert!(retained.operation.same_instance(&original));
        assert!(retained.origin.is_none());
        assert_eq!(
            original.start_evidence(),
            crate::operation::StartEvidence::Possible
        );
        assert_eq!(
            original.check(),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
    }

    #[test]
    fn parser_fallback_refuses_replacement_and_moves_only_with_original_result() {
        let original = metadata_operation();
        let replacement = metadata_operation(); // Same textual ID, distinct INSTANCE.
        let mut parser = ClaudeParser::default();
        parser.feed_owned(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"canonical fallback"}]}}"#,
            &original,
        ).unwrap();
        let pointer = parser.last_text.as_ref().unwrap().as_ptr();
        let original_bytes = original.captured_bytes();
        let replacement_bytes = replacement.captured_bytes();
        let result = r#"{"type":"result","subtype":"success","usage":{}}"#;
        assert!(parser.feed_owned(result, &replacement).is_err());
        assert_eq!(replacement.captured_bytes(), replacement_bytes); // Refuse before decode/copies.
        assert_eq!(original.captured_bytes(), original_bytes);
        assert_eq!(parser.last_text.as_ref().unwrap().as_ptr(), pointer);
        assert!(
            parser
                .last_text_operation
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        assert!(parser.feed(result).is_empty()); // Unowned entry cannot consume runtime evidence.
        assert_eq!(parser.last_text.as_ref().unwrap().as_ptr(), pointer);
        let events = parser.feed_owned(result, &original).unwrap();
        let StreamEvent::TurnDone(outcome) = events.last().unwrap() else {
            panic!("original result");
        };
        assert_eq!(outcome.text.as_ref().unwrap().as_ptr(), pointer); // MOVE, no fallback clone.
        assert_eq!(outcome.text.as_deref(), Some("canonical fallback"));
        assert!(parser.last_text.is_none());
        assert!(parser.last_text_operation.is_none());
        assert_eq!(
            original.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
        // Pure recorded parsing retains its established output and has no runtime witness.
        let mut pure = ClaudeParser::default();
        pure.feed(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"pure fallback"}]}}"#);
        assert!(pure.feed_owned(result, &replacement).is_err());
        assert_eq!(pure.last_text.as_deref(), Some("pure fallback"));
        assert!(pure.last_text_operation.is_none());
        assert!(
            matches!(pure.feed(result).last(), Some(StreamEvent::TurnDone(outcome))
            if outcome.text.as_deref() == Some("pure fallback"))
        );
    }

    #[test]
    fn parser_archive_borrows_original_fallback_even_after_overflow_without_adoption() {
        use crate::failed_evidence::{ArchiveEncoder, EvidenceCoverageBuilder};
        const CHILD: &str = "33333333-3333-4333-8333-333333333333";
        let original = archive_startup_operation();
        let replacement = archive_startup_operation();
        let mut parser = ClaudeParser::default();
        parser.feed_owned(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"fallback λ\n\"body\""}]}}"#,
            &original,
        ).unwrap();
        let pointer = parser.last_text.as_ref().unwrap().as_ptr();
        let bytes = original.captured_bytes();
        original.overflow();
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        assert!(
            parser
                .visit_archive(&replacement, Some(CHILD), &mut coverage)
                .is_err()
        );
        assert!(
            parser
                .visit_archive(&original, None, &mut coverage)
                .is_err()
        );
        parser
            .visit_archive(&original, Some(CHILD), &mut coverage)
            .unwrap();
        let coverage = coverage.finish().unwrap();
        let mut output = Vec::new(); // Synthetic serialization only, no archive filesystem I/O.
        let mut encoder = ArchiveEncoder::new(&original, coverage, &mut output).unwrap();
        parser
            .visit_archive(&original, Some(CHILD), &mut encoder)
            .unwrap();
        let measurement = encoder.finish().unwrap();
        original
            .archive_plan()
            .unwrap()
            .verify_stream(
                &original,
                coverage,
                measurement,
                &mut std::io::Cursor::new(&output),
            )
            .unwrap();
        let rows: Vec<serde_json::Value> = output
            .split(|byte| *byte == b'\n')
            .filter(|row| !row.is_empty())
            .map(|row| serde_json::from_slice(row).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1]["evidence"]["part"], "parser_fallback");
        assert!(rows[1]["evidence"]["physical_child"].is_null());
        assert!(rows[1]["evidence"]["origin"].is_null());
        assert_eq!(parser.last_text.as_ref().unwrap().as_ptr(), pointer);
        assert!(
            parser
                .last_text_operation
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        assert_eq!(original.captured_bytes(), bytes);
        assert_eq!(
            original.check(),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
    }

    #[tokio::test]
    async fn retained_parser_fallback_blocks_context_replacement_and_requires_actual_reader_join() {
        use crate::failed_evidence::EvidenceCoverageBuilder;
        let original = archive_startup_operation();
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let mut driver = cleanup_driver(sink.clone());
        driver.host = Some(recovery_context(driver.cfg.seat, original.clone()));
        driver.parser.feed_owned(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"retained parser evidence"}]}}"#,
            &original,
        ).unwrap();
        let pointer = driver.parser.last_text.as_ref().unwrap().as_ptr();
        assert!(
            driver
                .set_host_context(recovery_context(driver.cfg.seat, metadata_operation()))
                .is_err()
        );
        assert!(driver.start(None).await.is_err()); // Before startup path/native effects.
        assert!(
            driver
                .run_turn("never dispatched", Duration::from_secs(1))
                .await
                .is_err()
        );
        assert!(driver.stop().await.is_err());
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        assert!(
            driver
                .visit_parser_archive(&original, &mut coverage)
                .is_err()
        ); // Missing physical owner.
        let (mut child, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let (release, waiting) = tokio::sync::oneshot::channel::<()>();
        child.readers.push(tokio::spawn(async move {
            let _ = waiting.await;
            Ok(())
        }));
        driver.proc = Some(child);
        assert!(
            driver
                .visit_parser_archive(&original, &mut coverage)
                .is_err()
        );
        release.send(()).unwrap();
        driver
            .proc
            .as_mut()
            .unwrap()
            .join_readers(original.allowance().deadline())
            .await
            .unwrap();
        driver
            .visit_parser_archive(&original, &mut coverage)
            .unwrap();
        assert!(coverage.finish().is_ok()); // Component only, no complete custody/healthy ACK.
        assert_eq!(driver.parser.last_text.as_ref().unwrap().as_ptr(), pointer);
        assert!(
            driver
                .parser
                .last_text_operation
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        assert!(driver.stop().await.is_err());
        assert!(sink.stop_delivery().await.is_empty()); // No raw/fallback -> normal transcript conversion.
    }

    #[test]
    fn refused_init_event_does_not_consume_once_only_parser_state() {
        refuse_derived_frame(
            r#"{"type":"system","subtype":"init","session_id":"s","tools":["mcp__cna__list"],"model":"fixture"}"#,
            false,
        );
    }

    #[test]
    fn refused_assistant_suffix_does_not_replace_previously_accepted_text() {
        refuse_derived_frame(
            r#"{"type":"assistant","session_id":"s","message":{"content":[{"type":"text","text":"staged replacement"},{"type":"tool_use","id":"c","name":"mcp__cna__list","input":{"nested":["β",{"x":4}]}}]}}"#,
            true,
        );
    }

    #[test]
    fn refused_result_does_not_take_previous_text_before_event_admission() {
        refuse_derived_frame(
            r#"{"type":"result","subtype":"success","session_id":"s","usage":{"output_tokens":1}}"#,
            true,
        );
    }

    #[test]
    fn owned_decode_keeps_recorded_frame_sequence_and_parser_state() {
        let operation = metadata_operation();
        let mut pure = ClaudeParser::default();
        let mut owned = ClaudeParser::default();
        for line in [
            r#"{"type":"system","subtype":"init","session_id":"s","tools":[],"mcp_servers":[],"model":"fixture","claude_code_version":"2.1.292"}"#,
            r#"{"type":"assistant","session_id":"s","message":{"usage":{"input_tokens":8},"content":[{"type":"text","text":"a\nβ"},{"type":"tool_use","id":"c","name":"mcp__cna__list","input":{"x":[1,null]}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"c","content":"{\"a\":2}"}]}}"#,
            r#"{"type":"result","subtype":"success","session_id":"s","usage":{"output_tokens":1}}"#,
            r#"{"type":"system","subtype":"init","session_id":"s"}"#,
            "[ malformed",
        ] {
            assert_eq!(owned.feed_owned(line, &operation).unwrap(), pure.feed(line));
            assert_eq!(owned.init_seen, pure.init_seen);
            assert_eq!(owned.last_text, pure.last_text);
        }
        // Decoding a frame alone never invents trusted operation/session start evidence.
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    struct HeldMetadataStore;
    #[async_trait]
    impl crate::transcript::TranscriptStore for HeldMetadataStore {
        async fn append(&self, _: SeatId, _: String, _: TranscriptEntry) -> Result<u64, String> {
            // No storage ACK is invented. Each fixture explicitly joins this worker.
            std::future::pending().await
        }
    }

    fn metadata_operation() -> crate::operation::Operation {
        crate::operation::Operation::new(
            "metadata-original".into(),
            7,
            crate::operation::Allowance::required(Some(Instant::now() + Duration::from_secs(10)))
                .unwrap(),
        )
        .unwrap()
    }

    fn retained_metadata(
        event: StreamEvent,
        operation: &crate::operation::Operation,
        origin: Option<crate::operation::CaptureOrigin>,
    ) -> super::super::RetainedEvents {
        super::super::RetainedEvents {
            raw: super::super::ChildFrame {
                text: "original accepted raw metadata".into(),
                origin,
                operation: operation.clone(),
            },
            events: [event].into(),
        }
    }

    fn accept_metadata(
        retained: &mut super::super::RetainedEvents,
        emitter: &mut EntryEmitter,
        sink: &TranscriptSink,
        telemetry: &mut super::super::SessionTelemetry,
        session: &mut Option<String>,
    ) -> Result<Option<StreamEvent>, DriverError> {
        retained.accept_front(&mut |event, origin| {
            emitter.set_origin(origin)?;
            capture_session_event(event, emitter, sink, telemetry, session)
        })
    }

    #[tokio::test]
    async fn refused_init_row_retains_original_event_and_published_session() {
        let operation = metadata_operation();
        operation.possible_start();
        let sink =
            TranscriptSink::new(Arc::new(HeldMetadataStore)).for_operation(operation.clone());
        let mut emitter = EntryEmitter::new("axis.commander".parse().unwrap(), sink.clone());
        let mut telemetry = super::super::SessionTelemetry {
            model: Some("original model".into()),
            cli_version: Some("original version".into()),
            tools: vec!["original tool".into()],
            context_tokens: Some(42),
            compactions: 3,
        };
        let mut session = Some("original session".into());
        let mut retained = retained_metadata(
            StreamEvent::Init {
                session_id: "replacement session".into(),
                model: Some("x".repeat(crate::operation::DELIVERY_BYTES)),
                cli_version: Some("new version".into()),
                tools: vec!["mcp__cna__observe".into()],
                mcp_connected: Some(true),
            },
            &operation,
            Some(operation.next_origin().unwrap()),
        );
        let raw_pointer = retained.raw.text.as_ptr();
        let StreamEvent::Init { model, .. } = retained.events.front().unwrap() else {
            panic!()
        };
        let model_pointer = model.as_ref().unwrap().as_ptr();
        let bytes = operation.captured_bytes();
        assert!(
            accept_metadata(
                &mut retained,
                &mut emitter,
                &sink,
                &mut telemetry,
                &mut session
            )
            .is_err()
        );
        assert_eq!(retained.events.len(), 1);
        let StreamEvent::Init { model, .. } = retained.events.front().unwrap() else {
            panic!()
        };
        assert_eq!(model.as_ref().unwrap().as_ptr(), model_pointer);
        assert_eq!(retained.raw.text.as_ptr(), raw_pointer);
        assert!(retained.raw.operation.same_instance(&operation));
        assert_eq!(session.as_deref(), Some("original session"));
        assert_eq!(telemetry.model.as_deref(), Some("original model"));
        assert_eq!(telemetry.cli_version.as_deref(), Some("original version"));
        assert_eq!(telemetry.tools, ["original tool"]);
        assert_eq!(telemetry.context_tokens, Some(42));
        assert_eq!(telemetry.compactions, 3);
        assert_eq!(sink.pending_count(), 0);
        assert!(operation.captured_bytes() > bytes); // Admitted derivation is not refunded.
        assert_eq!(
            operation.check(),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert!(matches!(
            operation.start_evidence(),
            crate::operation::StartEvidence::Possible
        ));
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn refused_compaction_keeps_event_count_and_context_without_unknown_retag() {
        let operation = metadata_operation();
        let sink =
            TranscriptSink::new(Arc::new(HeldMetadataStore)).for_operation(operation.clone());
        let mut emitter = EntryEmitter::new("axis.commander".parse().unwrap(), sink.clone());
        let mut telemetry = super::super::SessionTelemetry {
            context_tokens: Some(91),
            compactions: 4,
            ..Default::default()
        };
        let mut session = Some("original session".into());
        let mut retained = retained_metadata(
            StreamEvent::Compacted {
                trigger: "x".repeat(crate::operation::DELIVERY_BYTES),
                pre_tokens: Some(u64::MAX),
            },
            &operation,
            None,
        );
        let raw_pointer = retained.raw.text.as_ptr();
        assert!(
            accept_metadata(
                &mut retained,
                &mut emitter,
                &sink,
                &mut telemetry,
                &mut session
            )
            .is_err()
        );
        assert_eq!(retained.events.len(), 1);
        assert_eq!(retained.raw.text.as_ptr(), raw_pointer);
        assert!(retained.raw.origin.is_none());
        assert!(retained.raw.operation.same_instance(&operation));
        assert_eq!(telemetry.compactions, 4);
        assert_eq!(telemetry.context_tokens, Some(91));
        assert_eq!(session.as_deref(), Some("original session"));
        assert_eq!(sink.pending_count(), 0);
        assert!(matches!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        ));
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn accepted_metadata_preserves_fifo_known_and_unknown_origins_without_model_start() {
        let operation = metadata_operation();
        let sink =
            TranscriptSink::new(Arc::new(HeldMetadataStore)).for_operation(operation.clone());
        let mut emitter = EntryEmitter::new("axis.commander".parse().unwrap(), sink.clone());
        let mut telemetry = super::super::SessionTelemetry {
            context_tokens: Some(91),
            ..Default::default()
        };
        let mut session = None;
        let origin = operation.next_origin().unwrap();
        let mut init = retained_metadata(
            StreamEvent::Init {
                session_id: "new session".into(),
                model: Some("escaped\" model λ".into()),
                cli_version: None,
                tools: vec!["mcp__cna__observe".into()],
                mcp_connected: Some(true),
            },
            &operation,
            Some(origin.clone()),
        );
        accept_metadata(&mut init, &mut emitter, &sink, &mut telemetry, &mut session).unwrap();
        assert!(init.events.is_empty());
        assert_eq!(session.as_deref(), Some("new session"));
        assert_eq!(telemetry.model.as_deref(), Some("escaped\" model λ"));
        assert_eq!(telemetry.context_tokens, Some(91));
        let mut compacted = retained_metadata(
            StreamEvent::Compacted {
                trigger: "automatic λ".into(),
                pre_tokens: Some(u64::MAX),
            },
            &operation,
            None,
        );
        accept_metadata(
            &mut compacted,
            &mut emitter,
            &sink,
            &mut telemetry,
            &mut session,
        )
        .unwrap();
        assert!(compacted.events.is_empty());
        assert_eq!(telemetry.compactions, 1);
        assert_eq!(telemetry.context_tokens, None);
        assert_eq!(sink.pending_count(), 2);
        let rows = sink.stop_delivery().await; // Actual fixture worker join, no storage ACK.
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].origin.as_ref(), Some(&origin));
        assert_eq!(rows[1].origin, None);
        assert_eq!(
            rows[0].entry,
            TranscriptEntry::System {
                text: "CLI metadata: model escaped\" model λ, version unknown".into()
            }
        );
        assert_eq!(
            rows[1].entry,
            TranscriptEntry::System {
                text: "CLI compacted context (automatic λ, pre_tokens Some(18446744073709551615))"
                    .into()
            }
        );
        assert!(matches!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        ));
    }

    #[tokio::test]
    async fn compaction_counter_refusal_keeps_original_event_and_no_capture() {
        let operation = metadata_operation();
        let sink =
            TranscriptSink::new(Arc::new(HeldMetadataStore)).for_operation(operation.clone());
        let mut emitter = EntryEmitter::new("axis.commander".parse().unwrap(), sink.clone());
        let mut telemetry = super::super::SessionTelemetry {
            context_tokens: Some(7),
            compactions: u64::MAX,
            ..Default::default()
        };
        let mut session = None;
        let mut retained = retained_metadata(
            StreamEvent::Compacted {
                trigger: "automatic".into(),
                pre_tokens: None,
            },
            &operation,
            None,
        );
        assert!(
            accept_metadata(
                &mut retained,
                &mut emitter,
                &sink,
                &mut telemetry,
                &mut session
            )
            .is_err()
        );
        assert_eq!(retained.events.len(), 1);
        assert_eq!(telemetry.compactions, u64::MAX);
        assert_eq!(telemetry.context_tokens, Some(7));
        assert_eq!(sink.pending_count(), 0);
        assert!(sink.stop_delivery().await.is_empty());
    }

    fn cleanup_driver(sink: TranscriptSink) -> ClaudeDriver {
        ClaudeDriver::new(
            ClaudeConfig {
                seat: "axis.commander".parse().unwrap(),
                exe: None,
                model: "haiku".into(),
                config_dir: None,
                expected_email: None,
                sandbox: "unused-inert-fixture".into(),
                run_dir: "unused-inert-fixture".into(),
                mcp_url: "unused-inert-fixture".into(),
                system_prompt: String::new(),
                effort: None,
                context_window: None,
            },
            sink,
        )
    }

    fn startup_capture(operation: &crate::operation::Operation, model: &str) -> StartupCapture {
        StartupCapture::prepare(
            operation,
            "original session",
            model,
            "2.1.292".into(),
            "fixture@invalid.example",
            false,
        )
        .unwrap()
    }

    #[test]
    fn owned_version_gate_preserves_errors_and_admits_only_the_derived_copy() {
        for (text, expected) in [
            ("2.1.277 (Claude Code)", Ok("2.1.277")),
            ("  3.0.0 extra", Ok("3.0.0")),
            (
                "2.1.276",
                Err("Claude >=2.1.277 is required for durable resumed totals"),
            ),
            (
                "2.1.277.4",
                Err("Claude >=2.1.277 is required for durable resumed totals"),
            ),
            ("2.1.277.bad", Err("unrecognized Claude version")),
            ("2.bad", Err("unrecognized Claude version")),
            ("", Err("unrecognized Claude version")),
        ] {
            let original = metadata_operation();
            let original_baseline = original.captured_bytes();
            match (verified_version(text, &original), expected) {
                (Ok(version), Ok(want)) => {
                    assert_eq!(version, want);
                    assert_eq!(supported_version(text).unwrap(), want);
                    assert_eq!(
                        original.captured_bytes(),
                        original_baseline + want.len() + std::mem::size_of::<String>()
                    );
                }
                (Err(DriverError::Isolation(error)), Err(want)) => {
                    assert_eq!(error, want);
                    assert_eq!(
                        original.captured_bytes(),
                        original_baseline + want.len() + std::mem::size_of::<String>()
                    );
                    assert!(
                        matches!(supported_version(text), Err(DriverError::Isolation(error)) if error == want)
                    );
                }
                _ => panic!("version result changed"),
            }
        }
        let original = metadata_operation();
        let bytes = "2.1.277".len() + std::mem::size_of::<String>();
        original
            .charge(crate::operation::OPERATION_BYTES - original.captured_bytes() - bytes + 1)
            .unwrap();
        let before = original.captured_bytes();
        assert!(matches!(
            verified_version("2.1.277 (Claude Code)", &original),
            Err(DriverError::Host(crate::host::HostRefusal::CaptureOverflow))
        ));
        assert_eq!(original.captured_bytes(), before);
        assert_eq!(
            original.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[test]
    fn owned_account_gate_preserves_expected_login_and_syntax_diagnostics() {
        let original = metadata_operation();
        assert_eq!(
            verified_account(
                r#"{"loggedIn":true,"email":"First@Example.invalid"}"#,
                &original,
                Some("FIRST@EXAMPLE.INVALID"),
            )
            .unwrap(),
            "First@Example.invalid"
        );
        assert_eq!(
            verified_account(r#"{"loggedIn":true}"#, &metadata_operation(), None).unwrap(),
            ""
        );
        assert!(
            matches!(verified_account(r#"{"loggedIn":false}"#, &metadata_operation(), None),
            Err(DriverError::Isolation(error)) if error == "claude is not logged in")
        );
        assert!(matches!(verified_account(
            r#"{"loggedIn":true,"email":"λ@example.invalid"}"#, &metadata_operation(), Some("other"),
        ), Err(DriverError::Isolation(error))
            if error == "claude is logged in as λ@example.invalid, expected other"));
        let text = "{bad";
        let syntax = serde_json::from_str::<Value>(text).unwrap_err();
        assert!(
            matches!(verified_account(text, &metadata_operation(), None),
            Err(DriverError::Protocol(error)) if error == format!("claude auth status: {syntax}"))
        );
        assert_eq!(
            original.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    fn install_helper_text(
        child: &mut ChildProc,
        original: &crate::operation::Operation,
        text: &str,
    ) {
        // Synthetic recorded stdout belongs to the actually joined inert child. It is not
        // Claude auth, a provider witness or an executable verification invocation.
        child.pending_frame = Some(super::super::ChildFrame {
            text: text.into(),
            origin: None,
            operation: original.clone(),
        });
        child.append_helper_output(text).unwrap();
        assert!(child.pending_frame.is_none());
    }

    #[tokio::test]
    async fn helper_verification_refusal_keeps_original_raw_and_blocks_retry() {
        for text in ["2.1.276 (Claude Code)", "malformed"] {
            let sink = TranscriptSink::new(Arc::new(HeldMetadataStore));
            let (mut child, original) = joined_cleanup_child().await;
            let child_id = child.physical_child_id().to_owned();
            install_helper_text(&mut child, &original, text);
            let pointer = child.helper_output.as_ptr();
            let mut driver = cleanup_driver(sink.clone());
            driver.host = Some(recovery_context(driver.cfg.seat, original.clone()));
            driver.helpers.get_mut().push(child);
            assert!(
                finish_verified_helper(
                    driver.helpers.get_mut(),
                    &original,
                    original.allowance(),
                    verified_version,
                )
                .is_err()
            );
            let helper = driver.helpers.get_mut().last_mut().unwrap();
            assert_eq!(helper.helper_output.as_ptr(), pointer);
            assert_eq!(helper.helper_output, format!("{text}\n"));
            assert_eq!(helper.physical_child_id(), child_id);
            assert!(helper.capture_operation().same_instance(&original));
            assert!(!helper.is_running());
            assert!(helper.readers.is_empty());
            assert!(
                helper
                    .stderr_tail()
                    .contains("original joined-child diagnostic")
            );
            assert!(matches!(
                driver.verify_version().await,
                Err(DriverError::Host(crate::host::HostRefusal::Ownership(_)))
            ));
            // The retry refuses before executable discovery, filesystem or native effects.
            assert_eq!(driver.helpers.get_mut().len(), 1);
            assert!(driver.stop().await.is_err());
            assert_eq!(driver.helpers.get_mut()[0].helper_output.as_ptr(), pointer);
            assert_eq!(
                original.start_evidence(),
                crate::operation::StartEvidence::NotStarted
            );
            assert!(sink.stop_delivery().await.is_empty());
        }
    }

    #[tokio::test]
    async fn helper_account_copy_overflow_keeps_raw_and_original_liability() {
        let text = r#"{"loggedIn":true,"email":"λ@example.invalid"}"#;
        let probe = metadata_operation();
        let probe_baseline = probe.captured_bytes();
        verified_account(text, &probe, None).unwrap();
        let required = probe.captured_bytes() - probe_baseline;
        let sink = TranscriptSink::new(Arc::new(HeldMetadataStore));
        let (mut child, original) = joined_cleanup_child().await;
        install_helper_text(&mut child, &original, text);
        let pointer = child.helper_output.as_ptr();
        original.possible_start(); // Preserve existing unknown dispatch liability, never refund.
        original
            .charge(crate::operation::OPERATION_BYTES - original.captured_bytes() - required + 1)
            .unwrap();
        let mut driver = cleanup_driver(sink.clone());
        driver.host = Some(recovery_context(driver.cfg.seat, original.clone()));
        driver.helpers.get_mut().push(child);
        assert!(matches!(
            finish_verified_helper(
                driver.helpers.get_mut(),
                &original,
                original.allowance(),
                |text, operation| verified_account(text, operation, None),
            ),
            Err(DriverError::Host(crate::host::HostRefusal::CaptureOverflow))
        ));
        let helper = &driver.helpers.get_mut()[0];
        assert_eq!(helper.helper_output.as_ptr(), pointer);
        assert_eq!(helper.helper_output, format!("{text}\n"));
        assert!(helper.capture_operation().same_instance(&original));
        assert!(helper.readers.is_empty());
        assert!(driver.stop().await.is_err());
        assert_eq!(driver.helpers.get_mut()[0].helper_output.as_ptr(), pointer);
        assert_eq!(
            original.start_evidence(),
            crate::operation::StartEvidence::Possible
        );
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn helper_verified_handoff_requires_original_instance_and_actual_disposition() {
        let (mut child, original) = joined_cleanup_child().await;
        install_helper_text(&mut child, &original, "2.1.277 (Claude Code)");
        let pointer = child.helper_output.as_ptr();
        let mut helpers = vec![child];
        let replacement =
            crate::operation::Operation::new(original.id().into(), 7, original.allowance())
                .unwrap();
        assert!(helpers[0].bind_operation(replacement.clone()).is_err());
        assert!(helpers[0].capture_operation().same_instance(&original));
        assert_eq!(helpers[0].helper_output.as_ptr(), pointer);
        let called = std::cell::Cell::new(0usize);
        assert!(
            finish_verified_helper(&mut helpers, &replacement, original.allowance(), |_, _| {
                called.set(called.get() + 1);
                Ok(())
            })
            .is_err()
        );
        assert_eq!(called.get(), 0);
        assert_eq!(helpers[0].helper_output.as_ptr(), pointer);
        // A genuinely pending retained reader handle blocks validation, even after the
        // fixture child has exited. Join this exact synthetic reader before removing it.
        helpers[0]
            .readers
            .push(tokio::spawn(std::future::pending::<std::io::Result<()>>()));
        assert!(
            finish_verified_helper(&mut helpers, &original, original.allowance(), |_, _| {
                called.set(called.get() + 1);
                Ok(())
            })
            .is_err()
        );
        assert_eq!(called.get(), 0);
        assert_eq!(helpers[0].helper_output.as_ptr(), pointer);
        helpers[0].readers.last().unwrap().abort();
        assert!(
            helpers[0]
                .readers
                .last_mut()
                .unwrap()
                .await
                .unwrap_err()
                .is_cancelled()
        );
        helpers[0].readers.pop();
        let expired = original.allowance().capped(Duration::ZERO);
        assert!(matches!(
            finish_verified_helper(&mut helpers, &original, expired, |_, _| {
                called.set(called.get() + 1);
                Ok(())
            }),
            Err(DriverError::Host(crate::host::HostRefusal::Expired))
        ));
        assert_eq!(called.get(), 0);
        assert_eq!(helpers[0].helper_output.as_ptr(), pointer);
        assert_eq!(
            finish_verified_helper(
                &mut helpers,
                &original,
                original.allowance(),
                verified_version,
            )
            .unwrap(),
            "2.1.277"
        );
        assert!(helpers.is_empty());
        assert_eq!(
            original.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
        // Verification neither settles the operation nor releases an actor/prompt credit.
        original.check().unwrap();
    }

    #[test]
    fn startup_descriptor_admits_actual_copies_before_acceptance() {
        let operation = metadata_operation();
        let operation_baseline = operation.captured_bytes();
        let version = "already owned version".to_owned();
        let version_pointer = version.as_ptr();
        let capture = StartupCapture::prepare(
            &operation,
            "session λ",
            "escaped\" model",
            version,
            "fixture@invalid.example",
            true,
        )
        .unwrap();
        assert_eq!(
            capture.info.cli_version.as_ref().unwrap().as_ptr(),
            version_pointer
        );
        assert_eq!(
            capture.text,
            "resumed session session λ (claude-code, model escaped\" model, account fixture@invalid.example)"
        );
        let required = std::mem::size_of::<StartupCapture>()
            + capture.text.len()
            + std::mem::size_of::<String>()
            + 2 * ("session λ".len() + std::mem::size_of::<String>())
            + "escaped\" model".len()
            + std::mem::size_of::<String>();
        assert_eq!(operation.captured_bytes(), operation_baseline + required);
        assert!(!capture.effect_accepted);
        assert!(capture.operation.same_instance(&operation));
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
        let refused = metadata_operation();
        refused
            .charge(crate::operation::OPERATION_BYTES - refused.captured_bytes() - required + 1)
            .unwrap();
        assert!(
            StartupCapture::prepare(
                &refused,
                "session λ",
                "escaped\" model",
                "already owned version".into(),
                "fixture@invalid.example",
                true,
            )
            .is_err()
        );
        assert_eq!(
            refused.check(),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert_eq!(
            refused.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[tokio::test]
    async fn accepted_startup_moves_session_only_after_fifo_row_admission() {
        let sink = TranscriptSink::new(Arc::new(HeldMetadataStore));
        let (child, operation) = joined_cleanup_child().await;
        let child_id = child.physical_child_id().to_owned();
        let mut driver = cleanup_driver(sink.clone());
        driver.proc = Some(child);
        driver.host = Some(recovery_context(driver.cfg.seat, operation.clone()));
        sink.try_emit_batch(
            driver.cfg.seat,
            vec![TranscriptEntry::System {
                text: "already accepted prefix".into(),
            }],
        )
        .unwrap();
        let mut capture = startup_capture(&operation, "escaped\" model λ");
        capture.effect_accepted = true; // Synthetic binding to the actually joined inert child.
        let info_id = capture.info.session_id.as_ptr();
        let published_id = capture.published_session_id.as_ptr();
        driver.startup = Some(capture);
        let info = driver.publish_startup().unwrap();
        assert_eq!(info.session_id.as_ptr(), info_id);
        assert_eq!(driver.session_id.as_ref().unwrap().as_ptr(), published_id);
        assert!(driver.startup.is_none());
        assert_eq!(driver.proc.as_ref().unwrap().physical_child_id(), child_id);
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
        let rows = sink.stop_delivery().await; // Actual worker join, no fabricated storage ACK.
        assert_eq!(rows.len(), 2);
        assert!(matches!(&rows[0].entry, TranscriptEntry::System { text }
            if text == "already accepted prefix"));
        assert!(matches!(&rows[1].entry, TranscriptEntry::System { text }
            if text == "started session original session (claude-code, model escaped\" model λ, account fixture@invalid.example)"));
        assert!(rows[1].origin.is_none()); // Host metadata is not a CLI confirmation frame.
        driver.stop().await.unwrap();
    }

    #[tokio::test]
    async fn startup_delivery_overflow_keeps_original_descriptor_child_and_session() {
        let sink = TranscriptSink::new(Arc::new(HeldMetadataStore));
        let (child, operation) = joined_cleanup_child().await;
        let child_id = child.physical_child_id().to_owned();
        let mut driver = cleanup_driver(sink.clone());
        driver.proc = Some(child);
        driver.host = Some(recovery_context(driver.cfg.seat, operation.clone()));
        driver.session_id = Some("previous accepted session".into());
        sink.try_emit_batch(
            driver.cfg.seat,
            vec![TranscriptEntry::System {
                text: "already accepted prefix".into(),
            }],
        )
        .unwrap();
        let model = "λ".repeat(crate::operation::DELIVERY_BYTES / 2);
        let mut capture = startup_capture(&operation, &model);
        assert!(capture.text.len() < crate::operation::FRAME_BYTES);
        assert!(capture.text.len() > crate::operation::DELIVERY_BYTES);
        let canonical = capture.text.as_ptr();
        capture.effect_accepted = true;
        driver.startup = Some(capture);
        operation.possible_start(); // Existing liability is not refunded by metadata refusal.
        assert!(matches!(
            driver.publish_startup(),
            Err(DriverError::Host(crate::host::HostRefusal::CaptureOverflow))
        ));
        let charged = operation.captured_bytes();
        assert!(driver.publish_startup().is_err());
        assert_eq!(operation.captured_bytes(), charged); // Sticky cause, no replay/extra copy.
        assert_eq!(driver.startup.as_ref().unwrap().text.as_ptr(), canonical);
        assert!(
            driver
                .startup
                .as_ref()
                .unwrap()
                .operation
                .same_instance(&operation)
        );
        assert_eq!(
            driver.session_id.as_deref(),
            Some("previous accepted session")
        );
        assert!(driver.start(None).await.is_err()); // Refuses before filesystem/preflight effects.
        assert!(
            driver
                .run_turn("no dispatch", Duration::from_secs(1))
                .await
                .is_err()
        );
        assert!(driver.prompt_lease.is_none());
        assert!(
            driver
                .set_host_context(recovery_context(driver.cfg.seat, metadata_operation()))
                .is_err()
        );
        assert!(driver.stop().await.is_err());
        let retained_child = driver.proc.as_mut().unwrap();
        assert!(!retained_child.is_running());
        assert!(retained_child.readers.is_empty());
        assert_eq!(retained_child.physical_child_id(), child_id);
        assert!(
            retained_child
                .stderr_tail()
                .contains("original joined-child diagnostic")
        );
        assert_eq!(driver.startup.as_ref().unwrap().text.as_ptr(), canonical);
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::Possible
        );
        let rows = sink.stop_delivery().await;
        assert_eq!(rows.len(), 1);
        assert!(matches!(&rows[0].entry, TranscriptEntry::System { text }
            if text == "already accepted prefix"));
        // Original failed descriptor/child deliberately remain owned; no healthy finish
        // or guessed public drain/handoff API is introduced by this source control.
    }

    #[tokio::test]
    async fn startup_publication_rejects_same_text_id_other_operation_instance() {
        let sink = TranscriptSink::new(Arc::new(HeldMetadataStore));
        let (child, original) = joined_cleanup_child().await;
        let other = crate::operation::Operation::new(original.id().into(), 7, original.allowance())
            .unwrap();
        let mut driver = cleanup_driver(sink.clone());
        driver.proc = Some(child);
        let mut capture = startup_capture(&other, "fixture");
        capture.effect_accepted = true;
        let canonical = capture.text.as_ptr();
        driver.startup = Some(capture);
        let charged = other.captured_bytes();
        assert!(matches!(
            driver.publish_startup(),
            Err(DriverError::Host(crate::host::HostRefusal::Ownership(_)))
        ));
        assert_eq!(other.captured_bytes(), charged);
        assert_eq!(driver.startup.as_ref().unwrap().text.as_ptr(), canonical);
        assert!(
            driver
                .proc
                .as_ref()
                .unwrap()
                .capture_operation()
                .same_instance(&original)
        );
        assert!(sink.stop_delivery().await.is_empty());
    }

    struct ArchiveAdmissionProbe(std::sync::atomic::AtomicUsize);

    impl crate::host::HostAdmission for ArchiveAdmissionProbe {
        fn subscribe(&self) -> tokio::sync::watch::Receiver<crate::host::HostSnapshot> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            crate::host::HostAdmission::subscribe(&RecoveryClosedAdmission)
        }
        fn snapshot(&self) -> Result<crate::host::HostSnapshot, crate::host::HostRefusal> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(crate::host::HostRefusal::Unknown)
        }
        fn acquire(
            &self,
            _: &crate::host::HostIdentity,
            _: crate::host::HostStep,
        ) -> Result<Box<dyn crate::host::HostStepLease>, crate::host::HostRefusal> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(crate::host::HostRefusal::Unknown)
        }
        fn acquire_prompt(
            &self,
            _: &crate::host::HostIdentity,
            _: crate::operation::Allowance,
        ) -> Result<Box<dyn crate::host::HostPromptLease>, crate::host::HostRefusal> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(crate::host::HostRefusal::Unknown)
        }
    }

    // SOURCE ONLY/UNRUN: these entrypoint refusals perform no filesystem, child,
    // provider or archive effects. The counter measures only this inert host adapter.
    #[tokio::test]
    async fn native_entrypoints_refuse_missing_misbound_or_in_service_archive_before_admission() {
        for case in 0..5 {
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let mut driver = cleanup_driver(sink.clone());
            let original = if case == 0 {
                metadata_operation()
            } else {
                archive_startup_operation()
            };
            let probe = Arc::new(ArchiveAdmissionProbe(std::sync::atomic::AtomicUsize::new(
                0,
            )));
            let mut context = recovery_context(driver.cfg.seat, original.clone());
            context.admission = probe.clone();
            match case {
                1 => context.identity.seat = "axis.logistics".parse().unwrap(),
                2 => context.identity.epoch += 1,
                3 => context.identity.operation_id = "different-original-id".into(),
                _ => {}
            }
            let held_chunk = (case == 4).then(|| original.borrow_archive_chunk(&original).unwrap());
            let expected =
                ClaudeDriver::check_native_archive(&context, driver.cfg.seat).unwrap_err();
            match case {
                0 => assert_eq!(expected, crate::host::HostRefusal::NoAllowance),
                1..=3 => assert_eq!(expected, crate::host::HostRefusal::IdentityChanged),
                4 => assert!(matches!(expected, crate::host::HostRefusal::Ownership(_))),
                _ => unreachable!(),
            }
            driver.host = Some(context);
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            assert!(matches!(driver.verify_version().await,
                Err(DriverError::Host(ref error)) if error == &expected));
            assert!(matches!(driver.verify_account().await,
                Err(DriverError::Host(ref error)) if error == &expected));
            assert!(matches!(driver.start(None).await,
                Err(DriverError::Host(ref error)) if error == &expected));
            assert!(matches!(driver.start(Some("existing-session")).await,
                Err(DriverError::Host(ref error)) if error == &expected));
            assert!(
                matches!(driver.run_turn("never dispatched", Duration::from_secs(1)).await,
                Err(DriverError::Host(ref error)) if error == &expected)
            );
            assert_eq!(probe.0.load(std::sync::atomic::Ordering::SeqCst), 0);
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
            assert!(
                driver
                    .host
                    .as_ref()
                    .unwrap()
                    .operation
                    .same_instance(&original)
            );
            assert!(driver.proc.is_none());
            assert!(driver.helpers.get_mut().is_empty());
            assert!(driver.startup.is_none());
            assert!(driver.prompt_lease.is_none());
            assert!(driver.emitter.is_none());
            assert!(driver.completed_turn.is_none());
            assert!(driver.idle);
            assert_eq!(sink.pending_count(), 0);
            assert_eq!(
                original.start_evidence(),
                crate::operation::StartEvidence::NotStarted
            );
            drop(held_chunk); // SAME borrowed workspace returns, without charge/refund.
            if case == 4 {
                ClaudeDriver::check_native_archive(driver.host.as_ref().unwrap(), driver.cfg.seat)
                    .unwrap();
                assert_eq!(original.captured_bytes(), charged);
            }
        }
    }

    #[test]
    fn native_archive_gate_returns_same_workspace_and_preserves_closed_first_cause() {
        let original = archive_startup_operation();
        let seat = "axis.commander".parse().unwrap();
        let context = recovery_context(seat, original.clone());
        let charged = original.captured_bytes();
        let deadline = original.allowance().deadline();
        ClaudeDriver::check_native_archive(&context, seat).unwrap();
        ClaudeDriver::check_native_archive(&context.clone(), seat).unwrap();
        let chunk = original.borrow_archive_chunk(&original).unwrap();
        assert!(matches!(
            ClaudeDriver::check_native_archive(&context, seat),
            Err(crate::host::HostRefusal::Ownership(_))
        ));
        drop(chunk);
        original.overflow();
        assert_eq!(
            ClaudeDriver::check_native_archive(&context, seat),
            Err(crate::host::HostRefusal::CaptureOverflow)
        );
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(original.allowance().deadline(), deadline);
        // Refusal never settles or discards the original overflowed cleanup reserve.
        assert!(original.borrow_archive_chunk(&original).is_ok());
    }

    #[test]
    fn missing_archive_cannot_enter_startup_effect_or_discard_original_descriptor() {
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let mut driver = cleanup_driver(sink);
        let original = metadata_operation();
        let context = recovery_context(driver.cfg.seat, original.clone());
        driver.startup = Some(startup_capture(&original, "fixture"));
        let pointer = driver.startup.as_ref().unwrap().text.as_ptr();
        let charged = original.captured_bytes();
        let mut command = Command::new("never-started-inert-placeholder");
        let mut lease = StartupEffectLease {
            invalidate: None,
            fail_after_effect: false,
            calls: 0,
        };
        assert!(matches!(
            driver.spawn_session(&mut command, &context, &mut lease),
            Err(DriverError::Host(crate::host::HostRefusal::NoAllowance))
        ));
        assert_eq!(lease.calls, 0);
        assert!(driver.proc.is_none());
        let retained = driver.startup.as_ref().unwrap();
        assert_eq!(retained.text.as_ptr(), pointer);
        assert!(retained.operation.same_instance(&original));
        assert!(!retained.effect_accepted);
        assert_eq!(original.captured_bytes(), charged);
        original.check().unwrap();
    }

    struct StartupEffectLease {
        invalidate: Option<crate::operation::Operation>,
        fail_after_effect: bool,
        calls: usize,
    }
    impl crate::host::HostStepLease for StartupEffectLease {
        fn perform_sync(
            &mut self,
            effect: &mut dyn FnMut() -> Result<(), crate::host::HostRefusal>,
        ) -> Result<(), crate::host::HostRefusal> {
            self.calls += 1;
            if let Some(operation) = &self.invalidate {
                operation.overflow();
            }
            effect()?;
            if self.fail_after_effect {
                Err(crate::host::HostRefusal::Unknown)
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn startup_effect_rechecks_closed_operation_and_retains_posteffect_child() {
        let sink = TranscriptSink::new(Arc::new(HeldMetadataStore));
        let mut driver = cleanup_driver(sink.clone());
        let operation = archive_startup_operation();
        let context = recovery_context(driver.cfg.seat, operation.clone());
        driver.startup = Some(startup_capture(&operation, "fixture"));
        let mut command = Command::new("never-started-inert-placeholder");
        let mut before = StartupEffectLease {
            invalidate: Some(operation.clone()),
            fail_after_effect: false,
            calls: 0,
        };
        assert!(matches!(
            driver.spawn_session(&mut command, &context, &mut before),
            Err(DriverError::Host(crate::host::HostRefusal::CaptureOverflow))
        ));
        assert_eq!(before.calls, 1);
        assert!(driver.proc.is_none());
        assert!(driver.startup.is_none());
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );

        let operation = archive_startup_operation();
        let context = recovery_context(driver.cfg.seat, operation.clone());
        driver.host = Some(context.clone());
        driver.startup = Some(startup_capture(&operation, "fixture"));
        let canonical = driver.startup.as_ref().unwrap().text.as_ptr();
        // This is only the already-existing inert test executable, never Claude/auth/model.
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args([
            "--exact",
            "driver::claude::tests::cleanup_evidence_probe_child",
            "--nocapture",
        ]);
        apply_seat_env(&mut command, &[("SEAT_CLEANUP_EVIDENCE_PROBE", "1".into())]);
        let mut after = StartupEffectLease {
            invalidate: None,
            fail_after_effect: true,
            calls: 0,
        };
        assert!(matches!(
            driver.spawn_session(&mut command, &context, &mut after),
            Err(DriverError::Host(crate::host::HostRefusal::Unknown))
        ));
        assert_eq!(after.calls, 1);
        let child = driver
            .proc
            .as_mut()
            .expect("registered inside actual effect");
        let child_id = child.physical_child_id().to_owned();
        child
            .finish_until(operation.allowance().deadline())
            .await
            .unwrap();
        while child
            .next_line(operation.allowance().deadline())
            .await
            .unwrap()
            .is_some()
        {
            child.acknowledge_line();
        }
        assert!(child.readers.is_empty());
        assert!(driver.publish_startup().is_err()); // Refused lease cannot publish a session.
        assert!(driver.stop().await.is_err());
        assert_eq!(driver.startup.as_ref().unwrap().text.as_ptr(), canonical);
        assert_eq!(driver.proc.as_ref().unwrap().physical_child_id(), child_id);
        assert!(
            driver
                .proc
                .as_ref()
                .unwrap()
                .capture_operation()
                .same_instance(&operation)
        );
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[test]
    fn cleanup_evidence_probe_child() {
        if std::env::var_os("SEAT_CLEANUP_EVIDENCE_PROBE").is_some() {
            eprintln!("original joined-child diagnostic");
        }
    }

    async fn joined_cleanup_child() -> (ChildProc, crate::operation::Operation) {
        // Runs only this inert test binary, never Claude, auth, a model or a provider.
        let deadline = Instant::now() + Duration::from_secs(10);
        let operation = crate::operation::Operation::new(
            "cleanup-original".into(),
            7,
            crate::operation::Allowance::required(Some(deadline)).unwrap(),
        )
        .unwrap();
        joined_cleanup_child_with_operation(operation).await
    }

    async fn joined_cleanup_child_with_operation(
        operation: crate::operation::Operation,
    ) -> (ChildProc, crate::operation::Operation) {
        let deadline = operation
            .allowance()
            .capped(Duration::from_secs(10))
            .deadline();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args([
            "--exact",
            "driver::claude::tests::cleanup_evidence_probe_child",
            "--nocapture",
        ]);
        apply_seat_env(&mut command, &[("SEAT_CLEANUP_EVIDENCE_PROBE", "1".into())]);
        let mut child = ChildProc::spawn(&mut command, operation.clone()).unwrap();
        child.finish_until(deadline).await.unwrap();
        assert!(!child.is_running());
        assert!(child.readers.is_empty()); // Actual SAME child + reader joins, not a timer.
        assert!(child.physical_supervision_complete()); // Private fact, never aggregate release.
        // Acknowledge only this inert harness's stdout, leaving its canonical stderr intact.
        while child.next_line(deadline).await.unwrap().is_some() {
            child.acknowledge_line();
        }
        assert_eq!(child.lines.len(), 0);
        assert!(child.pending_frame.is_none());
        // Stdout was explicitly acknowledged; original diagnostic liability remains.
        assert!(child.queued_frames() > 0);
        assert!(
            child
                .stderr_tail()
                .contains("original joined-child diagnostic")
        );
        (child, operation)
    }

    struct RecoveryClosedAdmission;
    // SOURCE ONLY/UNRUN. This creates only a future isolated test fixture;
    // production archive roots still come from the trusted campaign launcher.
    fn custody_fixture_root() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("cna-custody-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        for child in ["campaigns", "sandbox", "run"] {
            std::fs::create_dir(root.join(child)).unwrap();
        }
        root
    }

    // SOURCE ONLY/UNRUN. Future admitted inert-child fixture; gate transitions
    // below are synthetic, not execution of the native read/pause race control.
    #[tokio::test]
    async fn joined_failed_handoff_streams_old_evidence_without_custody_or_root_release() {
        use super::super::reader_handoff::{Pipe, Refusal};
        use crate::failed_evidence::JoinedArchiveSource;
        let original = archive_startup_operation();
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (mut child, _) = joined_cleanup_child_with_operation(original.clone()).await;
        assert!(child.physical_supervision_complete()); // Actual future fixture joins.
        let gate = original.reader_handoff();
        gate.attach(&original, &child.physical_child_id).unwrap();
        gate.terminal_processed(&original).unwrap();
        let request = gate.request(&original, &child.physical_child_id).unwrap();
        assert_eq!(
            gate.delivered(&original, Pipe::Stdout, 1),
            Err(Refusal::Bytes)
        );
        // Separate already-observed original physical closure, not an ACK.
        gate.child_exit_observed();
        child.handoff = Some(gate.clone());
        child.handoff_request = Some(request);
        original.charge(128).unwrap(); // Synthetic retained old-turn body only.
        child.pending_frame = Some(super::super::ChildFrame {
            text: "old between-turn bytes retained for operator".into(),
            origin: None,
            operation: original.clone(),
        });
        let pointer = child.pending_frame.as_ref().unwrap().text.as_ptr();
        let physical = child.physical_child_id.clone();
        let mut driver = ClaudeDriver::new(argument_config(), sink.clone());
        driver.proc = Some(child);
        let coverage = driver.original_archive_coverage(&original).unwrap();

        // Same text in another allocation cannot satisfy saved physical identity.
        driver.proc.as_mut().unwrap().physical_child_id = physical.as_ref().into();
        let mut refused = Vec::new();
        assert!(driver.stream(&original, coverage, &mut refused).is_err());
        assert!(refused.is_empty()); // All association checks precede output header.
        driver.proc.as_mut().unwrap().physical_child_id = physical.clone();
        let request = driver
            .proc
            .as_mut()
            .unwrap()
            .handoff_request
            .take()
            .unwrap();
        assert!(driver.stream(&original, coverage, &mut refused).is_err());
        assert!(refused.is_empty());
        driver.proc.as_mut().unwrap().handoff_request = Some(request);

        let _ = original.charge(crate::operation::OPERATION_BYTES + 1);
        let charged = original.captured_bytes();
        let mut output = Vec::new(); // Test reference output, no production mirror.
        let measurement = driver.stream(&original, coverage, &mut output).unwrap();
        original
            .archive_plan()
            .unwrap()
            .verify_stream(&original, coverage, measurement, &mut output.as_slice())
            .unwrap();
        let encoded = String::from_utf8(output).unwrap();
        assert_eq!(encoded.matches("reader_handoff_gate").count(), 1);
        assert_eq!(encoded.matches("child_pause_request").count(), 1);
        assert!(encoded.contains("old between-turn bytes retained for operator"));
        assert!(encoded.contains("original joined-child diagnostic"));
        assert_eq!(
            gate.resume_before_prompt(
                driver
                    .proc
                    .as_ref()
                    .unwrap()
                    .handoff_request
                    .as_ref()
                    .unwrap()
            ),
            Err(Refusal::Bytes)
        );
        assert!(
            driver
                .proc
                .as_mut()
                .unwrap()
                .check_archive_release()
                .is_err()
        );
        assert!(driver.stop().await.is_err()); // Covered bytes grant no actor/root release.
        let child = driver.proc.as_ref().unwrap();
        assert!(child.physical_supervision_complete());
        assert!(Arc::ptr_eq(&child.physical_child_id, &physical));
        assert!(child.handoff_request.is_some());
        assert_eq!(child.pending_frame.as_ref().unwrap().text.as_ptr(), pointer);
        assert!(
            child
                .pending_frame
                .as_ref()
                .unwrap()
                .operation
                .same_instance(&original)
        );
        assert_eq!(original.captured_bytes(), charged);
        assert!(original.borrow_archive_chunk(&original).is_ok());
        assert!(driver.completed_turn.is_none());
        assert!(sink.stop_delivery().await.is_empty()); // No normal/spectator output.
    }

    // SOURCE ONLY/UNRUN. Future inert fixture joins; pause ACKs are synthetic.
    // This tests the actual private terminal seam, not native pipe cancel safety.
    #[tokio::test]
    async fn terminal_pause_cancellation_retains_actual_request_and_captured_result_owner() {
        use super::super::claude_correlation::Refusal;
        use super::super::reader_handoff::Pipe;
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let original = archive_startup_operation();
        let foreign = archive_startup_operation();
        let input = original.prepare_claude_input().unwrap();
        let input = std::str::from_utf8(&input).unwrap();
        original
            .validate_claude_frame(&json!({
                "type":"user", "isReplay":true, "parent_tool_use_id":null,
                "session_id":"fixture", "uuid":input,
                "message":{"role":"user", "content":"fixture"}
            }))
            .unwrap();
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (mut child, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let gate = original.reader_handoff();
        gate.attach(&original, &child.physical_child_id).unwrap();
        child.handoff = Some(gate.clone());
        // ACK alone never installs a terminal pause request.
        assert!(child.pause_after_terminal(&original).await.is_err());
        assert!(child.handoff_request.is_none());
        assert!(gate.requested().is_none());
        original
            .validate_claude_frame(&json!({
                "type":"result", "subtype":"success", "user_message_uuid":input
            }))
            .unwrap();
        assert_eq!(original.check_claude_terminal(), Ok(()));
        {
            let checkout = original.borrow_claude_correlation(&original).unwrap();
            assert_eq!(original.check_claude_terminal(), Err(Refusal::Unsupported));
            drop(checkout);
        }
        let physical = child.physical_child_id.clone();
        let mut driver = ClaudeDriver::new(argument_config(), sink.clone());
        driver.host = Some(recovery_context(driver.cfg.seat, original.clone()));
        driver.proc = Some(child);
        driver.parser.runtime_operation = Some(original.clone());
        driver.idle = false;
        original.charge(256).unwrap(); // Synthetic retained outcome strings only.
        driver.completed_turn = Some(TurnOutcome {
            ok: false,
            error: Some("original terminal failure".into()),
            text: Some("original captured answer".into()),
            usage: Usage::default(),
            quota: Vec::new(),
        });
        let pointer = driver
            .completed_turn
            .as_ref()
            .unwrap()
            .text
            .as_ref()
            .unwrap()
            .as_ptr();
        let charged = original.captured_bytes();
        {
            let pause = driver
                .proc
                .as_mut()
                .unwrap()
                .pause_after_terminal(&original);
            assert!(std::mem::size_of_val(&pause) <= super::super::reader_handoff::WAIT_BYTES);
            let mut pause = std::pin::pin!(pause);
            let mut cx = Context::from_waker(Waker::noop());
            assert!(matches!(pause.as_mut().poll(&mut cx), Poll::Pending));
            let request = gate.requested().unwrap();
            gate.acknowledge(&request, Pipe::Stdout).unwrap();
            assert!(matches!(pause.as_mut().poll(&mut cx), Poll::Pending));
            // Cancel borrowed wait after only ONE ACK. Request/result stay owned.
        }
        assert!(driver.proc.as_ref().unwrap().handoff_request.is_some());
        assert!(!driver.idle);
        assert!(driver.take_captured_outcome(&foreign).is_err());
        assert_eq!(
            driver
                .completed_turn
                .as_ref()
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .as_ptr(),
            pointer
        );
        let request = gate.requested().unwrap();
        gate.acknowledge(&request, Pipe::Stderr).unwrap();
        gate.wait_both(&request).unwrap().await.unwrap(); // Synthetic BOTH only.
        let outcome = driver.take_captured_outcome(&original).unwrap().unwrap();
        assert_eq!(outcome.text.as_ref().unwrap().as_ptr(), pointer); // MOVE, no copy.
        assert_eq!(outcome.error.as_deref(), Some("original terminal failure"));
        assert!(driver.stop().await.is_err()); // BOTH ACKs do not release root.
        let child = driver.proc.as_ref().unwrap();
        assert!(child.physical_supervision_complete());
        assert!(Arc::ptr_eq(&child.physical_child_id, &physical));
        assert!(child.handoff_request.is_some());
        assert!(
            driver
                .parser
                .runtime_operation
                .as_ref()
                .unwrap()
                .same_instance(&original)
        );
        assert_eq!(original.captured_bytes(), charged);
        assert!(sink.stop_delivery().await.is_empty());
    }

    // SOURCE ONLY/UNRUN. Actual inert fixture child joins when this test is
    // separately admitted. Terminal/reader/actor ACKs below are synthetic;
    // this proves neither native pause safety nor healthy archive custody.
    #[tokio::test]
    async fn terminal_settlement_keeps_owners_through_cancel_refusal_and_boundary_race() {
        use super::super::reader_handoff::{Pipe, Refusal};
        use std::{
            future::Future,
            sync::atomic::{AtomicU8, AtomicUsize, Ordering},
            task::{Context, Poll, Waker},
        };
        for case in 0..3 {
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            let input = original.prepare_claude_input().unwrap();
            let input = std::str::from_utf8(&input).unwrap();
            original
                .validate_claude_frame(&json!({
                    "type":"user", "isReplay":true, "parent_tool_use_id":null,
                    "session_id":"fixture", "uuid":input,
                    "message":{"role":"user", "content":"fixture"}
                }))
                .unwrap();
            original
                .validate_claude_frame(&json!({
                    "type":"result", "subtype":"success", "user_message_uuid":input
                }))
                .unwrap();
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let (mut child, _) = joined_cleanup_child_with_operation(original.clone()).await;
            let physical = child.physical_child_id.clone();
            let gate = original.reader_handoff();
            gate.attach(&original, &physical).unwrap();
            gate.terminal_processed(&original).unwrap();
            let request = gate.request(&original, &physical).unwrap();
            child.handoff = Some(gate.clone());
            child.handoff_request = Some(request.clone());
            gate.acknowledge(&request, Pipe::Stdout).unwrap();
            let phase = Arc::new(AtomicU8::new(0));
            let requests = Arc::new(AtomicUsize::new(0));
            let mut driver = ClaudeDriver::new(argument_config(), sink.clone());
            driver.host = Some(recovery_context(driver.cfg.seat, original.clone()));
            driver.proc = Some(child);
            driver.parser.runtime_operation = Some(original.clone());
            driver.prompt_lease = Some(Box::new(CleanupPromptLease {
                phase: phase.clone(),
                requests: requests.clone(),
            }));
            driver.emitter = Some(EntryEmitter::new(
                driver.cfg.seat,
                sink.for_operation(original.clone()),
            ));
            driver.idle = false;
            original.charge(256).unwrap(); // Synthetic fixture bodies only.
            driver.completed_turn = Some(TurnOutcome {
                ok: false,
                error: Some("original terminal failure".into()),
                text: Some("original captured answer".into()),
                usage: Usage::default(),
                quota: Vec::new(),
            });
            let pointer = driver
                .completed_turn
                .as_ref()
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .as_ptr();
            let charged = original.captured_bytes();
            // ONE reader, foreign original, mismatched physical allocation and
            // stale request all refuse BEFORE invoking the prompt actor.
            assert!(driver.settle_captured_terminal(&original).await.is_err());
            gate.acknowledge(&request, Pipe::Stderr).unwrap();
            assert!(driver.settle_captured_terminal(&foreign).await.is_err());
            driver.proc.as_mut().unwrap().physical_child_id = Arc::from(physical.as_ref());
            assert!(driver.settle_captured_terminal(&original).await.is_err());
            driver.proc.as_mut().unwrap().physical_child_id = physical.clone();
            driver.proc.as_mut().unwrap().handoff_request = None;
            assert!(driver.settle_captured_terminal(&original).await.is_err());
            driver.proc.as_mut().unwrap().handoff_request = Some(request.clone());
            assert_eq!(requests.load(Ordering::SeqCst), 0);
            if case == 2 {
                let foreign_gate = foreign.reader_handoff();
                foreign_gate.attach(&foreign, &physical).unwrap();
                foreign_gate.terminal_processed(&foreign).unwrap();
                let stale = foreign_gate.request(&foreign, &physical).unwrap();
                driver.proc.as_mut().unwrap().handoff_request = Some(stale);
                assert!(driver.settle_captured_terminal(&original).await.is_err());
                assert_eq!(requests.load(Ordering::SeqCst), 0);
                assert!(!driver.idle);
                assert!(driver.prompt_lease.is_some() && driver.emitter.is_some());
                assert_eq!(
                    driver
                        .completed_turn
                        .as_ref()
                        .unwrap()
                        .text
                        .as_ref()
                        .unwrap()
                        .as_ptr(),
                    pointer
                );
                // A stale request poisons the SAME gate; restoring a header
                // must not reset the first refusal or make a later ACK healthy.
                assert_eq!(
                    gate.check_paused(&request, &physical),
                    Err(Refusal::Identity)
                );
                assert!(driver.stop().await.is_err());
                assert!(
                    driver
                        .proc
                        .as_ref()
                        .unwrap()
                        .physical_supervision_complete()
                );
                assert!(driver.proc.as_ref().unwrap().handoff_request.is_some());
                assert_eq!(original.captured_bytes(), charged);
                assert!(sink.stop_delivery().await.is_empty());
                continue;
            }
            {
                let settling = driver.settle_captured_terminal(&original);
                assert!(
                    std::mem::size_of_val(&settling) <= super::super::reader_handoff::WAIT_BYTES
                );
                let mut settling = std::pin::pin!(settling);
                let mut cx = Context::from_waker(Waker::noop());
                assert!(matches!(settling.as_mut().poll(&mut cx), Poll::Pending));
                // Cancellation removes only this waiter, not original owners.
            }
            assert_eq!(requests.load(Ordering::SeqCst), 1);
            assert!(!driver.idle);
            assert!(driver.prompt_lease.is_some() && driver.emitter.is_some());
            assert_eq!(
                driver
                    .completed_turn
                    .as_ref()
                    .unwrap()
                    .text
                    .as_ref()
                    .unwrap()
                    .as_ptr(),
                pointer
            );
            phase.store(1, Ordering::SeqCst);
            assert!(driver.settle_captured_terminal(&original).await.is_err());
            assert_eq!(requests.load(Ordering::SeqCst), 2);
            assert!(!driver.idle);
            assert!(driver.prompt_lease.is_some() && driver.emitter.is_some());
            assert_eq!(
                driver
                    .completed_turn
                    .as_ref()
                    .unwrap()
                    .text
                    .as_ref()
                    .unwrap()
                    .as_ptr(),
                pointer
            );
            if case == 1 {
                let released = Arc::new(tokio::sync::Notify::new());
                driver.prompt_lease = Some(Box::new(BoundaryPromptLease {
                    released: released.clone(),
                    requests: requests.clone(),
                })); // Synthetic fixture only, not production lease replacement.
                {
                    let settling = driver.settle_captured_terminal(&original);
                    let mut settling = std::pin::pin!(settling);
                    let mut cx = Context::from_waker(Waker::noop());
                    assert!(matches!(settling.as_mut().poll(&mut cx), Poll::Pending));
                    // Gate records delivery only; its caller must retain the
                    // actual OLD raw bytes. This synthetic gate is not a pipe.
                    assert_eq!(
                        gate.delivered(&original, Pipe::Stderr, 1),
                        Err(Refusal::Bytes)
                    );
                    released.notify_one();
                    assert!(matches!(
                        settling.as_mut().poll(&mut cx),
                        Poll::Ready(Err(_))
                    ));
                }
                assert_eq!(requests.load(Ordering::SeqCst), 3);
                // Actor ACK completed, but the post-await gate check refused.
                assert!(driver.settle_captured_terminal(&original).await.is_err());
                assert_eq!(requests.load(Ordering::SeqCst), 3);
                assert!(!driver.idle);
                assert!(driver.prompt_lease.is_some() && driver.emitter.is_some());
                assert_eq!(
                    driver
                        .completed_turn
                        .as_ref()
                        .unwrap()
                        .text
                        .as_ref()
                        .unwrap()
                        .as_ptr(),
                    pointer
                );
                assert_eq!(gate.check_paused(&request, &physical), Err(Refusal::Bytes));
            } else {
                phase.store(2, Ordering::SeqCst); // Explicit synthetic actor ACK.
                let outcome = driver.settle_captured_terminal(&original).await.unwrap();
                assert_eq!(requests.load(Ordering::SeqCst), 3);
                assert_eq!(outcome.text.as_ref().unwrap().as_ptr(), pointer); // SAME MOVE.
                assert!(!outcome.ok);
                assert_eq!(outcome.error.as_deref(), Some("original terminal failure"));
                assert!(driver.idle);
                assert!(driver.completed_turn.is_none());
                assert!(driver.prompt_lease.is_none() && driver.emitter.is_none());
            }
            let child = driver.proc.as_ref().unwrap();
            assert!(Arc::ptr_eq(&child.physical_child_id, &physical));
            assert!(child.handoff_request.is_some());
            assert!(std::ptr::eq(
                child.handoff.as_ref().unwrap().as_ref(),
                original.reader_handoff_ref()
            ));
            assert!(
                driver
                    .parser
                    .runtime_operation
                    .as_ref()
                    .unwrap()
                    .same_instance(&original)
            );
            assert!(
                driver
                    .host
                    .as_ref()
                    .unwrap()
                    .operation
                    .same_instance(&original)
            );
            assert_eq!(original.captured_bytes(), charged);
            assert!(driver.stop().await.is_err()); // No custody/root/resume release.
            assert!(
                driver
                    .proc
                    .as_ref()
                    .unwrap()
                    .physical_supervision_complete()
            );
            assert!(driver.proc.as_ref().unwrap().handoff_request.is_some());
            assert_eq!(original.captured_bytes(), charged);
            assert!(sink.stop_delivery().await.is_empty());
        }
    }

    #[test]
    fn original_foreground_frame_sizing_does_not_poll_or_change_canonical_owners() {
        // Future admission control only; these concrete futures are NEVER
        // polled, so no helper, path, child, prompt, model or actor is entered.
        fn unpolled_bytes(future: impl std::future::Future + Send) -> usize {
            std::mem::size_of_val(&future)
        }
        let original = archive_startup_operation();
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let mut driver = Box::new(ClaudeDriver::new(argument_config(), sink));
        let pointer = std::ptr::from_ref(driver.as_ref());
        let charged = original.captured_bytes();
        let deadline = original.allowance().deadline();
        let idle = driver.idle;
        // Missing host would refuse on first poll. Layout inspection must not
        // even reach that preflight, and introduces no replacement context.
        let start = unpolled_bytes(driver.start_original_owner(None));
        let resumed = unpolled_bytes(driver.start_original_owner(Some("unpolled")));
        let turn = unpolled_bytes(driver.run_turn_original_owner("", Duration::ZERO));
        let other_turn = unpolled_bytes(
            driver.run_turn_original_owner("unpolled prompt", Duration::from_secs(1)),
        );
        let stop = unpolled_bytes(driver.stop_original_owner());
        assert_eq!(start, resumed);
        assert_eq!(turn, other_turn);
        assert!(start > 0 && turn > 0 && stop > 0);
        let fixed =
            super::super::archive_work::RetainedArchive::<ClaudeDriver>::fixed_bytes().unwrap();
        assert_eq!(
            driver.archive_owner_inline_bytes().unwrap(),
            fixed
                .checked_add(start)
                .unwrap()
                .checked_add(turn)
                .unwrap()
                .checked_add(stop)
                .unwrap()
        );
        assert_eq!(pointer, std::ptr::from_ref(driver.as_ref()));
        assert!(driver.host.is_none());
        assert!(driver.helpers.get_mut().is_empty());
        assert!(driver.proc.is_none());
        assert!(driver.prompt_lease.is_none() && driver.emitter.is_none());
        assert!(driver.completed_turn.is_none() && driver.startup.is_none());
        assert!(driver.parser.runtime_operation.is_none());
        assert!(driver.parser.last_text.is_none());
        assert!(driver.session_id.is_none());
        assert_eq!(driver.idle, idle);
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(original.allowance().deadline(), deadline);
        // Typed original preparation refusal returns the SAME Box, no charge,
        // root allocation or false witness supplied by successful size inspection.
        let (error, driver) = match driver.prepare_archive_work(&original) {
            Err(refused) => refused,
            Ok(_) => panic!("sizing invented a host/root witness"),
        };
        assert!(matches!(
            error,
            crate::failed_evidence::ArchiveError::Identity
        ));
        assert_eq!(pointer, std::ptr::from_ref(driver.as_ref()));
        assert_eq!(original.captured_bytes(), charged);
    }

    #[test]
    fn stageful_claude_preparer_keeps_refused_box_and_installs_original_without_polling() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<super::super::archive_work::PreparationOwner<ClaudeDriver>>();
        let original = archive_startup_operation();
        let foreign = archive_startup_operation();
        let charged = original.captured_bytes();
        let deadline = original.allowance().deadline();
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let driver = Box::new(ClaudeDriver::new(argument_config(), sink.clone()));
        let mut refused = driver.prepare_original_owner(&original); // Missing host.
        assert_eq!(
            refused.position(&original).unwrap(),
            (
                super::super::archive_work::PreparationPosition::Driver,
                true
            )
        );
        let mut lease = refused.borrow(&original).unwrap();
        assert!(matches!(
            lease.first_refusal().unwrap(),
            Some(crate::failed_evidence::ArchiveError::Identity)
        ));
        assert!(lease.supervisor().is_err());
        drop(lease);
        assert!(refused.borrow(&foreign).is_err());

        let mut driver = Box::new(ClaudeDriver::new(argument_config(), sink));
        driver.host = Some(recovery_context(driver.cfg.seat, original.clone()));
        let pointer = std::ptr::from_ref(driver.as_ref());
        let idle = driver.idle;
        let mut owner = driver.prepare_original_owner(&original);
        assert_eq!(
            owner.position(&original).unwrap(),
            (
                super::super::archive_work::PreparationPosition::Ready,
                false
            )
        );
        let mut preparation = owner.borrow(&original).unwrap();
        let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
        let root = supervision.root_mut().unwrap();
        let mut foreground = root.borrow_foreground(&original).unwrap();
        let driver = foreground.driver_mut().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(driver));
        assert!(
            driver
                .host
                .as_ref()
                .unwrap()
                .operation
                .same_instance(&original)
        );
        assert!(driver.proc.is_none() && driver.helpers.get_mut().is_empty());
        assert!(driver.prompt_lease.is_none() && driver.emitter.is_none());
        assert!(driver.startup.is_none() && driver.completed_turn.is_none());
        assert!(driver.parser.runtime_operation.is_none() && driver.parser.last_text.is_none());
        assert_eq!(driver.idle, idle);
        drop(driver.start_original_owner(None)); // NEVER poll any native future.
        drop(driver.run_turn_original_owner("unpolled", Duration::ZERO));
        drop(driver.stop_original_owner());
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(original.allowance().deadline(), deadline);
        // No factory/main adoption, child/reader join, archive/custody or healthy witness.
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Actual private caller/factory association.
    // Native futures NEVER polled; optional returned SessionInfo injection is
    // explicitly SYNTHETIC, not model/native/whole-fit/public-facade evidence.
    #[test]
    fn original_caller_preserves_ingress_refusal_and_same_operation_slots() {
        use super::super::archive_work::DispatchError;
        use super::super::claude_caller::OriginalClaudeCaller;
        use crate::failed_evidence::ArchiveError;
        fn context_pointer(
            caller: &mut OriginalClaudeCaller,
            original: &crate::operation::Operation,
        ) -> usize {
            {
                let view = caller.results(original).unwrap();
                if let Some(context) = view.pending_context {
                    return context.identity.operation_id.as_ptr() as usize;
                }
            }
            let root = caller.root_for_source_control();
            if let Ok((driver, pending)) = root.unprepared_for_source_control(original) {
                return pending.map_or_else(
                    || driver.host.as_ref().unwrap().identity.operation_id.as_ptr() as usize,
                    |context| context.identity.operation_id.as_ptr() as usize,
                );
            }
            let prepared = root.prepared_original(original).unwrap();
            let mut preparation = prepared.borrow(original).unwrap();
            let mut supervisor = preparation.supervisor().unwrap().borrow(original).unwrap();
            let foreground = supervisor
                .root_mut()
                .unwrap()
                .borrow_foreground(original)
                .unwrap();
            foreground
                .driver()
                .unwrap()
                .host
                .as_ref()
                .unwrap()
                .identity
                .operation_id
                .as_ptr() as usize
        }
        for case in 0..3 {
            let cfg = argument_config();
            let seat = cfg.seat;
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let mut caller = OriginalClaudeCaller::new(cfg, sink);
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            let mut missing = None;
            assert!(caller.bind(&mut missing).is_err());
            assert!(caller.results(&original).is_err()); // No fabricated original flags.
            assert!(caller.start_future(&original, None).is_err());
            assert!(
                caller
                    .turn_future(&original, "not dispatched", Duration::ZERO)
                    .is_err()
            );
            assert!(caller.cleanup_future(&original).is_err());
            if case == 1 {
                original.overflow();
            }
            if case == 2 {
                let (driver, _, _) = caller
                    .root_for_source_control()
                    .concrete_factory_for_source_control()
                    .unwrap();
                driver.parser.last_text = Some("synthetic original pending body".into());
            }
            let mut input = Some(recovery_context(seat, original.clone()));
            let pointer = input.as_ref().unwrap().identity.operation_id.as_ptr() as usize;
            let bound = caller.bind(&mut input);
            assert!(input.is_none()); // First accepted caller owns input even if binding refuses.
            assert_eq!(context_pointer(&mut caller, &original), pointer);
            assert!(caller.results(&foreign).is_err());
            let mut repeat = Some(recovery_context(seat, foreign));
            let repeat_pointer = repeat.as_ref().unwrap().identity.operation_id.as_ptr();
            assert!(caller.bind(&mut repeat).is_err());
            assert_eq!(
                repeat.as_ref().unwrap().identity.operation_id.as_ptr(),
                repeat_pointer
            );
            assert_eq!(context_pointer(&mut caller, &original), pointer);
            if bound.is_err() {
                let view = caller.results(&original).unwrap();
                assert!(matches!(view.binding, Some(Err(_))));
                assert!(view.start.is_none() && view.turn.is_none() && view.cleanup.is_none());
                // Inner unbound/error projection is preserved as Err, never a
                // zeroed/default healthy ControllerResults structure.
                if view.pending_context.is_some() {
                    assert!(view.original.is_err());
                }
                assert!(caller.start_future(&original, None).is_err());
                assert!(
                    caller
                        .turn_future(&original, "not dispatched", Duration::ZERO)
                        .is_err()
                );
                assert!(caller.cleanup_future(&original).is_err());
                continue;
            }
            assert_eq!(case, 0);
            let erased = caller.start_future(&original, None);
            let unpolled = erased.is_ok();
            drop(erased); // NEVER poll preflight/native/model/archive work.
            let view = caller.results(&original).unwrap();
            assert!(view.start.is_none());
            if unpolled {
                assert!(view.original.as_ref().unwrap().start_started);
                assert!(view.original.as_ref().unwrap().start.is_none());
                let info = SessionInfo {
                    session_id: uuid::Uuid::new_v4().to_string(),
                    model: None,
                    cli_version: None,
                    resumed: false,
                };
                let body_pointer = info.session_id.as_ptr();
                // Explicitly SYNTHETIC returned body after unpolled creation.
                // Validates ownership MOVE only, not a real successful start.
                caller
                    .root_for_source_control()
                    .retain_start_for_source_control(&original, Ok(info))
                    .unwrap();
                assert!(caller.handoff_start_for_source_control(&original).unwrap());
                let view = caller.results(&original).unwrap();
                assert_eq!(
                    view.start
                        .unwrap()
                        .as_ref()
                        .ok()
                        .unwrap()
                        .session_id
                        .as_ptr(),
                    body_pointer
                );
                assert!(view.original.as_ref().unwrap().start.is_none());
                assert!(caller.handoff_start_for_source_control(&original).is_err());
                assert!(caller.start_future(&original, None).is_err());
                let view = caller.results(&original).unwrap();
                assert_eq!(
                    view.start
                        .unwrap()
                        .as_ref()
                        .ok()
                        .unwrap()
                        .session_id
                        .as_ptr(),
                    body_pointer
                );
            } else {
                assert!(
                    view.erasure_refusal.is_some()
                        || matches!(
                            view.original.as_ref().ok().and_then(|root| root.start),
                            Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                        )
                );
            }
            assert_eq!(context_pointer(&mut caller, &original), pointer);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. No child, native polling, archive I/O or
    // positive captured-body MOVE witness. Missing physical owner must retain
    // the synthetic earlier body and the distinct original turn Timeout.
    #[test]
    fn original_caller_capture_absence_and_refusal_preserve_distinct_results() {
        use super::super::archive_work::DispatchError;
        use super::super::claude_caller::OriginalClaudeCaller;
        for overflow in [false, true] {
            let cfg = argument_config();
            let seat = cfg.seat;
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let mut caller = OriginalClaudeCaller::new(cfg, sink);
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            assert!(caller.move_captured(&original).is_err());
            let mut input = Some(recovery_context(seat, original.clone()));
            if caller.bind(&mut input).is_err() {
                // An actual fixed-fit refusal is retained, not waived to force
                // a positive source-control branch on an unknown target.
                assert!(caller.results(&original).unwrap().binding.unwrap().is_err());
                assert!(caller.move_captured(&original).is_err());
                continue;
            }
            assert!(caller.move_captured(&foreign).is_err());
            assert!(!caller.move_captured(&original).unwrap());
            assert!(!caller.move_captured(&original).unwrap());
            let view = caller.results(&original).unwrap();
            assert!(view.captured.is_none() && view.capture_refusal.is_none());
            assert!(view.start.is_none() && view.turn.is_none() && view.cleanup.is_none());
            let body = String::from("synthetic captured body without child");
            let pointer = body.as_ptr();
            {
                let owner = caller.root_for_source_control();
                owner.begin_start_for_source_control(&original).unwrap();
                owner
                    .retain_start_for_source_control(
                        &original,
                        Ok(SessionInfo {
                            session_id: String::from("synthetic-start-prerequisite"),
                            model: None,
                            cli_version: None,
                            resumed: false,
                        }),
                    )
                    .unwrap();
                owner.begin_turn_for_source_control(&original).unwrap();
                owner
                    .retain_turn_for_source_control(
                        &original,
                        Err(DispatchError::Driver(DriverError::Timeout)),
                    )
                    .unwrap();
                let prepared = owner.prepared_original(&original).unwrap();
                let mut preparation = prepared.borrow(&original).unwrap();
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let mut foreground = supervision
                    .root_mut()
                    .unwrap()
                    .borrow_foreground(&original)
                    .unwrap();
                let driver = foreground.driver_mut().unwrap();
                assert!(driver.proc.is_none() && driver.helpers.get_mut().is_empty());
                driver.parser.runtime_operation = Some(original.clone());
                driver.completed_turn = Some(TurnOutcome {
                    ok: false,
                    error: None,
                    text: Some(body),
                    usage: Usage::default(),
                    quota: Vec::new(),
                });
            }
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            if overflow {
                original.overflow();
            }
            assert!(caller.move_captured(&foreign).is_err());
            assert!(caller.results(&original).unwrap().capture_refusal.is_none());
            assert!(caller.move_captured(&original).is_err());
            let cause =
                std::ptr::from_ref(caller.results(&original).unwrap().capture_refusal.unwrap());
            assert!(caller.move_captured(&original).is_err());
            let view = caller.results(&original).unwrap();
            assert!(std::ptr::eq(view.capture_refusal.unwrap(), cause));
            assert!(view.captured.is_none());
            assert!(matches!(
                view.original.as_ref().unwrap().turn,
                Some(Err(DispatchError::Driver(DriverError::Timeout)))
            ));
            assert!(view.start.is_none() && view.turn.is_none() && view.cleanup.is_none());
            {
                let prepared = caller
                    .root_for_source_control()
                    .prepared_original(&original)
                    .unwrap();
                let mut preparation = prepared.borrow(&original).unwrap();
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let mut foreground = supervision
                    .root_mut()
                    .unwrap()
                    .borrow_foreground(&original)
                    .unwrap();
                let driver = foreground.driver_mut().unwrap();
                assert!(std::ptr::eq(
                    driver
                        .completed_turn
                        .as_ref()
                        .unwrap()
                        .text
                        .as_ref()
                        .unwrap()
                        .as_ptr(),
                    pointer
                ));
                assert!(driver.proc.is_none() && driver.helpers.get_mut().is_empty());
            }
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Actual typed caller entry constructs then
    // DROPS the native future UNPOLLED. Injected first returns are synthetic;
    // no positive full-fit/public-trait/native/custody/settlement witness.
    #[test]
    fn typed_caller_slots_keep_original_first_return_and_archive_refusal() {
        use super::super::RetainedDriverFuture;
        use super::super::archive_work::{CleanupError, DispatchError};
        use super::super::claude_caller::{CallerResults, OriginalClaudeCaller};
        use crate::{failed_evidence::ArchiveError, operation::Operation};
        fn entry<'a>(
            caller: &'a mut OriginalClaudeCaller,
            original: &'a Operation,
            mode: usize,
        ) -> Result<RetainedDriverFuture<'a>, DriverError> {
            match mode {
                0 => caller.start_future(original, None),
                1 => caller.turn_future(original, "never polled", Duration::ZERO),
                2 => caller.cleanup_future(original),
                _ => unreachable!(),
            }
        }
        fn handoff(
            caller: &mut OriginalClaudeCaller,
            original: &Operation,
            mode: usize,
        ) -> Result<bool, ArchiveError> {
            match mode {
                0 => caller.handoff_start_returned(original),
                1 => caller.handoff_turn_returned(original),
                2 => caller.handoff_cleanup_returned(original),
                _ => unreachable!(),
            }
        }
        fn pointer(view: &CallerResults<'_>, mode: usize) -> Option<*const u8> {
            match mode {
                0 => match view.start {
                    Some(Ok(info)) => Some(info.session_id.as_ptr()),
                    Some(Err(DriverError::Protocol(body))) => Some(body.as_ptr()),
                    None => None,
                    _ => panic!("unexpected startup return"),
                },
                1 => match view.turn {
                    Some(Ok(outcome)) => outcome.text.as_ref().map(|text| text.as_ptr()),
                    Some(Err(DriverError::Protocol(body))) => Some(body.as_ptr()),
                    None => None,
                    _ => panic!("unexpected turn return"),
                },
                2 => match view.cleanup {
                    Some(Err(DriverError::Protocol(body))) => Some(body.as_ptr()),
                    Some(Ok(())) | None => None,
                    _ => panic!("unexpected cleanup return"),
                },
                _ => unreachable!(),
            }
        }
        fn root_fit(view: &CallerResults<'_>, mode: usize) -> bool {
            let root = view.original.as_ref().unwrap();
            match mode {
                0 => matches!(
                    root.start,
                    Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                ),
                1 => matches!(
                    root.turn,
                    Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                ),
                2 => matches!(
                    root.cleanup,
                    Some(Err(CleanupError::Archive(ArchiveError::Fit)))
                ),
                _ => unreachable!(),
            }
        }
        for mode in 0..3 {
            for returned in 0..3 {
                let cfg = argument_config();
                let seat = cfg.seat;
                let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
                let mut caller = OriginalClaudeCaller::new(cfg, sink);
                let original = archive_startup_operation();
                let foreign = archive_startup_operation();
                let mut context = Some(recovery_context(seat, original.clone()));
                if caller.bind(&mut context).is_err() {
                    assert!(caller.results(&original).unwrap().binding.unwrap().is_err());
                    assert!(entry(&mut caller, &original, mode).is_err());
                    continue; // Keep real fixed-fit refusal, never force positive admission.
                }
                if mode != 0 {
                    let owner = caller.root_for_source_control();
                    owner.begin_start_for_source_control(&original).unwrap();
                    owner
                        .retain_start_for_source_control(
                            &original,
                            Ok(SessionInfo {
                                session_id: String::from("synthetic typed caller prerequisite"),
                                model: None,
                                cli_version: None,
                                resumed: false,
                            }),
                        )
                        .unwrap();
                    if mode == 2 {
                        owner.begin_turn_for_source_control(&original).unwrap();
                        owner
                            .retain_turn_for_source_control(
                                &original,
                                Err(DispatchError::Driver(DriverError::Timeout)),
                            )
                            .unwrap();
                    }
                }
                let charged = original.captured_bytes();
                let deadline = original.allowance().deadline();
                assert!(entry(&mut caller, &foreign, mode).is_err());
                {
                    let view = caller.results(&original).unwrap();
                    let root = view.original.as_ref().unwrap();
                    assert!(!match mode {
                        0 => root.start_started,
                        1 => root.turn_started,
                        2 => root.cleanup_started,
                        _ => unreachable!(),
                    });
                }
                let future = entry(&mut caller, &original, mode);
                let admitted = future.is_ok();
                drop(future); // NEVER poll any native/helper/reader/worker work.
                if !admitted {
                    let view = caller.results(&original).unwrap();
                    assert!(view.erasure_refusal.is_some() || root_fit(&view, mode));
                    assert!(view.start.is_none() && view.turn.is_none() && view.cleanup.is_none());
                    assert!(entry(&mut caller, &original, mode).is_err());
                    assert_eq!(original.captured_bytes(), charged);
                    assert_eq!(original.allowance().deadline(), deadline);
                    continue;
                }
                {
                    let view = caller.results(&original).unwrap();
                    let root = view.original.as_ref().unwrap();
                    assert!(match mode {
                        0 => root.start_started && root.start.is_none(),
                        1 => root.turn_started && root.turn.is_none(),
                        2 => root.cleanup_started && root.cleanup.is_none(),
                        _ => unreachable!(),
                    });
                    assert!(view.start.is_none() && view.turn.is_none() && view.cleanup.is_none());
                }
                let body = String::from("synthetic actual typed caller first body");
                let body_pointer = body.as_ptr();
                let owner = caller.root_for_source_control();
                match mode {
                    0 => {
                        owner
                            .retain_start_for_source_control(
                                &original,
                                match returned {
                                    0 => Ok(SessionInfo {
                                        session_id: body,
                                        model: None,
                                        cli_version: None,
                                        resumed: false,
                                    }),
                                    1 => Err(DispatchError::Driver(DriverError::Protocol(body))),
                                    2 => Err(DispatchError::Archive(ArchiveError::Fit)),
                                    _ => unreachable!(),
                                },
                            )
                            .unwrap();
                    }
                    1 => {
                        owner
                            .retain_turn_for_source_control(
                                &original,
                                match returned {
                                    0 => Ok(TurnOutcome {
                                        ok: false,
                                        error: None,
                                        text: Some(body),
                                        usage: Usage::default(),
                                        quota: Vec::new(),
                                    }),
                                    1 => Err(DispatchError::Driver(DriverError::Protocol(body))),
                                    2 => Err(DispatchError::Archive(ArchiveError::Fit)),
                                    _ => unreachable!(),
                                },
                            )
                            .unwrap();
                    }
                    2 => {
                        owner
                            .retain_cleanup_for_source_control(
                                &original,
                                match returned {
                                    0 => Ok(()),
                                    1 => Err(CleanupError::Driver(DriverError::Protocol(body))),
                                    2 => Err(CleanupError::Archive(ArchiveError::Fit)),
                                    _ => unreachable!(),
                                },
                            )
                            .unwrap();
                    }
                    _ => unreachable!(),
                }
                assert!(handoff(&mut caller, &foreign, mode).is_err());
                let moved = handoff(&mut caller, &original, mode);
                if returned == 2 {
                    assert!(moved.is_err());
                    let view = caller.results(&original).unwrap();
                    assert!(root_fit(&view, mode));
                    assert!(view.start.is_none() && view.turn.is_none() && view.cleanup.is_none());
                    assert!(handoff(&mut caller, &original, mode).is_err());
                    assert!(root_fit(&caller.results(&original).unwrap(), mode));
                } else {
                    assert!(moved.unwrap());
                    let view = caller.results(&original).unwrap();
                    if mode == 2 && returned == 0 {
                        assert!(matches!(view.cleanup, Some(Ok(()))));
                    } else {
                        assert!(std::ptr::eq(pointer(&view, mode).unwrap(), body_pointer));
                    }
                    let root = view.original.as_ref().unwrap();
                    assert!(match mode {
                        0 => root.start.is_none(),
                        1 => root.turn.is_none(),
                        2 => root.cleanup.is_none(),
                        _ => unreachable!(),
                    });
                    assert!(handoff(&mut caller, &original, mode).is_err()); // Occupied typed slot.
                    if !(mode == 2 && returned == 0) {
                        assert!(std::ptr::eq(
                            pointer(&caller.results(&original).unwrap(), mode).unwrap(),
                            body_pointer
                        ));
                    }
                }
                assert!(entry(&mut caller, &original, mode).is_err()); // Original attempt is sticky.
                let view = caller.results(&original).unwrap();
                assert!(view.captured.is_none() && view.capture_refusal.is_none());
                if mode == 2 {
                    assert!(matches!(
                        view.original.as_ref().unwrap().turn,
                        Some(Err(DispatchError::Driver(DriverError::Timeout)))
                    ));
                }
                assert_eq!(original.captured_bytes(), charged);
                assert_eq!(original.allowance().deadline(), deadline);
            }
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Actual approved-output boundary must refuse
    // finite-capacity/checked-add failure BEFORE boxing; no future is accepted
    // or polled, no child/helper/provider/native/archive work is performed.
    #[test]
    fn retained_observation_boundary_keeps_original_pre_erasure_fit_cause() {
        use super::super::archive_work::{CleanupError, DispatchError};
        use crate::failed_evidence::{ArchiveError, FIXED_ARCHIVE_WORKSPACE};
        for mode in 0..3 {
            for extra in [FIXED_ARCHIVE_WORKSPACE, usize::MAX] {
                let cfg = argument_config();
                let seat = cfg.seat;
                let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
                let mut owner = ClaudeDriver::original_owner(cfg, sink);
                let original = archive_startup_operation();
                let foreign = archive_startup_operation();
                owner
                    .bind_original_context(recovery_context(seat, original.clone()))
                    .unwrap();
                if mode != 0 {
                    owner.begin_start_for_source_control(&original).unwrap();
                    owner
                        .retain_start_for_source_control(
                            &original,
                            Ok(SessionInfo {
                                session_id: String::from("synthetic observation fit prerequisite"),
                                model: None,
                                cli_version: None,
                                resumed: false,
                            }),
                        )
                        .unwrap();
                }
                if mode == 2 {
                    owner.begin_turn_for_source_control(&original).unwrap();
                    owner
                        .retain_turn_for_source_control(
                            &original,
                            Err(DispatchError::Driver(DriverError::Timeout)),
                        )
                        .unwrap();
                }
                let charged = original.captured_bytes();
                let deadline = original.allowance().deadline();
                let mut start = None;
                let mut turn = None;
                let mut cleanup = None;
                for destination_original in [&foreign, &original, &original] {
                    let result = match mode {
                        0 => owner.start_retained_observation(
                            &original,
                            None,
                            destination_original,
                            &mut start,
                            extra,
                        ),
                        1 => owner.turn_retained_observation(
                            &original,
                            "never polled",
                            Duration::ZERO,
                            destination_original,
                            &mut turn,
                            extra,
                        ),
                        2 => owner.cleanup_retained_observation(
                            &original,
                            destination_original,
                            &mut cleanup,
                            extra,
                        ),
                        _ => unreachable!(),
                    };
                    assert!(matches!(
                        result,
                        Err(DriverError::Host(crate::host::HostRefusal::Unknown))
                    ));
                    drop(result);
                    assert!(start.is_none() && turn.is_none() && cleanup.is_none());
                    let root = owner.retained_results(&original).unwrap();
                    let same = original.same_instance(destination_original);
                    match mode {
                        0 => {
                            assert_eq!(root.start_started, same);
                            if same {
                                assert!(matches!(
                                    root.start,
                                    Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                                ));
                            }
                        }
                        1 => {
                            assert_eq!(root.turn_started, same);
                            if same {
                                assert!(matches!(
                                    root.turn,
                                    Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                                ));
                            }
                        }
                        2 => {
                            assert_eq!(root.cleanup_started, same);
                            if same {
                                assert!(matches!(
                                    root.cleanup,
                                    Some(Err(CleanupError::Archive(ArchiveError::Fit)))
                                ));
                            }
                            assert!(matches!(
                                root.turn,
                                Some(Err(DispatchError::Driver(DriverError::Timeout)))
                            ));
                        }
                        _ => unreachable!(),
                    }
                    // The actual cause moved to the action result. No erased
                    // observation, fabricated callback or duplicate cause.
                    assert!(root.start_observation_refusal.is_none());
                    assert!(root.turn_observation_refusal.is_none());
                    assert!(root.cleanup_observation_refusal.is_none());
                }
                assert_eq!(original.captured_bytes(), charged);
                assert_eq!(original.allowance().deadline(), deadline);
            }
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Injected observation Result exercises the
    // SAME production projection, NOT an executed callback/positive body MOVE.
    #[test]
    fn retained_observation_projection_retains_first_cause_and_distinct_body() {
        use super::super::archive_work::{CleanupError, DispatchError};
        use crate::failed_evidence::ArchiveError;
        for mode in 0..3 {
            let cfg = argument_config();
            let seat = cfg.seat;
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let mut owner = ClaudeDriver::original_owner(cfg, sink);
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            owner
                .bind_original_context(recovery_context(seat, original.clone()))
                .unwrap();
            owner.begin_start_for_source_control(&original).unwrap();
            if mode != 0 {
                owner
                    .retain_start_for_source_control(
                        &original,
                        Ok(SessionInfo {
                            session_id: String::from("synthetic observation return prerequisite"),
                            model: None,
                            cli_version: None,
                            resumed: false,
                        }),
                    )
                    .unwrap();
                owner.begin_turn_for_source_control(&original).unwrap();
                if mode == 2 {
                    owner
                        .retain_turn_for_source_control(
                            &original,
                            Err(DispatchError::Driver(DriverError::Timeout)),
                        )
                        .unwrap();
                    owner.begin_cleanup_for_source_control(&original).unwrap();
                }
            }
            let body = String::from("synthetic original callback error, distinct from observation");
            let pointer = body.as_ptr();
            match mode {
                0 => {
                    owner
                        .retain_start_for_source_control(
                            &original,
                            Err(DispatchError::Driver(DriverError::Protocol(body))),
                        )
                        .unwrap();
                }
                1 => {
                    owner
                        .retain_turn_for_source_control(
                            &original,
                            Err(DispatchError::Driver(DriverError::Protocol(body))),
                        )
                        .unwrap();
                }
                2 => {
                    owner
                        .retain_cleanup_for_source_control(
                            &original,
                            Err(CleanupError::Driver(DriverError::Protocol(body))),
                        )
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            assert!(
                owner
                    .project_driver_observation_for_source_control(
                        &foreign,
                        mode,
                        Err(ArchiveError::Fit)
                    )
                    .is_err()
            );
            for (index, observed) in [
                Err(ArchiveError::Fit),
                Err(ArchiveError::Identity),
                Ok(true),
            ]
            .into_iter()
            .enumerate()
            {
                let result =
                    owner.project_driver_observation_for_source_control(&original, mode, observed);
                assert!(matches!(
                    result,
                    Err(DriverError::Host(crate::host::HostRefusal::Unknown))
                ));
                let root = owner.retained_results(&original).unwrap();
                let refusal = match mode {
                    0 => root.start_observation_refusal,
                    1 => root.turn_observation_refusal,
                    2 => root.cleanup_observation_refusal,
                    _ => unreachable!(),
                };
                assert!(
                    matches!(refusal, Some(ArchiveError::Fit)),
                    "first cause replaced at {index}"
                );
                let saved = match mode {
                    0 => match root.start {
                        Some(Err(DispatchError::Driver(DriverError::Protocol(body)))) => body,
                        _ => panic!("original startup body lost"),
                    },
                    1 => match root.turn {
                        Some(Err(DispatchError::Driver(DriverError::Protocol(body)))) => body,
                        _ => panic!("original turn body lost"),
                    },
                    2 => match root.cleanup {
                        Some(Err(CleanupError::Driver(DriverError::Protocol(body)))) => body,
                        _ => panic!("original cleanup body lost"),
                    },
                    _ => unreachable!(),
                };
                assert!(std::ptr::eq(saved.as_ptr(), pointer));
                if mode == 2 {
                    assert!(matches!(
                        root.turn,
                        Some(Err(DispatchError::Driver(DriverError::Timeout)))
                    ));
                }
            }
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. External typed entry/recovery uses actual
    // private caller, no native polling. First returns are SYNTHETIC; this is
    // neither public facade/full-fit nor an executed callback/custody witness.
    #[test]
    fn external_caller_slots_preserve_original_root_and_caller_returns() {
        use super::super::RetainedDriverFuture;
        use super::super::archive_work::{CleanupError, DispatchError};
        use super::super::claude_caller::OriginalClaudeCaller;
        use crate::{failed_evidence::ArchiveError, operation::Operation};
        type Slots = (
            Option<Result<SessionInfo, DriverError>>,
            Option<Result<TurnOutcome, DriverError>>,
            Option<Result<(), DriverError>>,
        );
        fn entry<'a>(
            caller: &'a mut OriginalClaudeCaller,
            original: &'a Operation,
            destination_original: &'a Operation,
            slots: &'a mut Slots,
            mode: usize,
        ) -> Result<RetainedDriverFuture<'a>, DriverError> {
            match mode {
                0 => caller.start_into(original, None, destination_original, &mut slots.0),
                1 => caller.turn_into(
                    original,
                    "never polled",
                    Duration::ZERO,
                    destination_original,
                    &mut slots.1,
                ),
                2 => caller.cleanup_into(original, destination_original, &mut slots.2),
                _ => unreachable!(),
            }
        }
        fn recover(
            caller: &mut OriginalClaudeCaller,
            original: &Operation,
            destination_original: &Operation,
            slots: &mut Slots,
            mode: usize,
        ) -> Result<bool, DriverError> {
            match mode {
                0 => caller.handoff_start_into(original, destination_original, &mut slots.0),
                1 => caller.handoff_turn_into(original, destination_original, &mut slots.1),
                2 => caller.handoff_cleanup_into(original, destination_original, &mut slots.2),
                _ => unreachable!(),
            }
        }
        fn pointer(slots: &Slots, mode: usize) -> *const u8 {
            let saved = match mode {
                0 => slots.0.as_ref().unwrap().as_ref().unwrap_err(),
                1 => slots.1.as_ref().unwrap().as_ref().unwrap_err(),
                2 => slots.2.as_ref().unwrap().as_ref().unwrap_err(),
                _ => unreachable!(),
            };
            match saved {
                DriverError::Protocol(body) => body.as_ptr(),
                _ => panic!("original body replaced"),
            }
        }
        for mode in 0..3 {
            for via_caller in [false, true] {
                let cfg = argument_config();
                let seat = cfg.seat;
                let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
                let mut caller = OriginalClaudeCaller::new(cfg, sink);
                let original = archive_startup_operation();
                let foreign = archive_startup_operation();
                let mut context = Some(recovery_context(seat, original.clone()));
                if caller.bind(&mut context).is_err() {
                    assert!(caller.results(&original).unwrap().binding.unwrap().is_err());
                    continue; // Preserve real fixed-fit refusal; no forced positive admission.
                }
                if mode != 0 {
                    let owner = caller.root_for_source_control();
                    owner.begin_start_for_source_control(&original).unwrap();
                    owner
                        .retain_start_for_source_control(
                            &original,
                            Ok(SessionInfo {
                                session_id: String::from("synthetic external-slot prerequisite"),
                                model: None,
                                cli_version: None,
                                resumed: false,
                            }),
                        )
                        .unwrap();
                    if mode == 2 {
                        owner.begin_turn_for_source_control(&original).unwrap();
                        owner
                            .retain_turn_for_source_control(
                                &original,
                                Err(DispatchError::Driver(DriverError::Timeout)),
                            )
                            .unwrap();
                    }
                }
                let charged = original.captured_bytes();
                let deadline = original.allowance().deadline();
                let mut empty = (None, None, None);
                let mut occupied = (
                    Some(Err(DriverError::Protocol(String::from("occupied startup")))),
                    Some(Err(DriverError::Protocol(String::from("occupied turn")))),
                    Some(Err(DriverError::Protocol(String::from("occupied cleanup")))),
                );
                let occupied_pointer = pointer(&occupied, mode);
                assert!(entry(&mut caller, &original, &foreign, &mut empty, mode).is_err());
                assert!(entry(&mut caller, &original, &original, &mut occupied, mode).is_err());
                assert!(std::ptr::eq(pointer(&occupied, mode), occupied_pointer));
                {
                    let view = caller.results(&original).unwrap();
                    let root = view.original.as_ref().unwrap();
                    assert!(!match mode {
                        0 => root.start_started,
                        1 => root.turn_started,
                        2 => root.cleanup_started,
                        _ => unreachable!(),
                    });
                }
                let future = entry(&mut caller, &original, &original, &mut empty, mode);
                let admitted = future.is_ok();
                drop(future); // NEVER poll any native/helper/reader/worker future.
                assert!(empty.0.is_none() && empty.1.is_none() && empty.2.is_none());
                if !admitted {
                    let view = caller.results(&original).unwrap();
                    let root = view.original.as_ref().unwrap();
                    let fit = match mode {
                        0 => matches!(
                            root.start,
                            Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                        ),
                        1 => matches!(
                            root.turn,
                            Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                        ),
                        2 => matches!(
                            root.cleanup,
                            Some(Err(CleanupError::Archive(ArchiveError::Fit)))
                        ),
                        _ => unreachable!(),
                    };
                    assert!(view.erasure_refusal.is_some() || fit);
                    assert!(entry(&mut caller, &original, &original, &mut empty, mode).is_err());
                    assert_eq!(original.captured_bytes(), charged);
                    assert_eq!(original.allowance().deadline(), deadline);
                    continue;
                }
                let body = String::from("synthetic first external callback error");
                let body_pointer = body.as_ptr();
                let owner = caller.root_for_source_control();
                match mode {
                    0 => {
                        owner
                            .retain_start_for_source_control(
                                &original,
                                Err(DispatchError::Driver(DriverError::Protocol(body))),
                            )
                            .unwrap();
                    }
                    1 => {
                        owner
                            .retain_turn_for_source_control(
                                &original,
                                Err(DispatchError::Driver(DriverError::Protocol(body))),
                            )
                            .unwrap();
                    }
                    2 => {
                        owner
                            .retain_cleanup_for_source_control(
                                &original,
                                Err(CleanupError::Driver(DriverError::Protocol(body))),
                            )
                            .unwrap();
                    }
                    _ => unreachable!(),
                }
                if via_caller {
                    assert!(
                        match mode {
                            0 => caller.handoff_start_returned(&original),
                            1 => caller.handoff_turn_returned(&original),
                            2 => caller.handoff_cleanup_returned(&original),
                            _ => unreachable!(),
                        }
                        .unwrap()
                    );
                }
                assert!(recover(&mut caller, &original, &foreign, &mut empty, mode).is_err());
                assert!(recover(&mut caller, &original, &original, &mut occupied, mode).is_err());
                assert!(std::ptr::eq(pointer(&occupied, mode), occupied_pointer));
                assert!(recover(&mut caller, &original, &original, &mut empty, mode).unwrap());
                assert!(std::ptr::eq(pointer(&empty, mode), body_pointer));
                assert!(recover(&mut caller, &original, &original, &mut empty, mode).is_err());
                assert!(std::ptr::eq(pointer(&empty, mode), body_pointer));
                let mut second = (None, None, None);
                assert!(!recover(&mut caller, &original, &original, &mut second, mode).unwrap());
                assert!(second.0.is_none() && second.1.is_none() && second.2.is_none());
                assert!(entry(&mut caller, &original, &original, &mut second, mode).is_err());
                let view = caller.results(&original).unwrap();
                assert!(view.start.is_none() && view.turn.is_none() && view.cleanup.is_none());
                assert!(view.captured.is_none() && view.capture_refusal.is_none());
                let root = view.original.as_ref().unwrap();
                assert!(match mode {
                    0 => root.start.is_none(),
                    1 => root.turn.is_none(),
                    2 => root.cleanup.is_none(),
                    _ => unreachable!(),
                });
                if mode == 2 {
                    assert!(matches!(
                        root.turn,
                        Some(Err(DispatchError::Driver(DriverError::Timeout)))
                    ));
                }
                assert_eq!(original.captured_bytes(), charged);
                assert_eq!(original.allowance().deadline(), deadline);
            }
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. No physical child is constructed. Separate
    // capture refuses without discarding an earlier synthetic canonical body.
    #[test]
    fn external_capture_keeps_body_and_first_refusal_without_physical_owner() {
        use super::super::claude_caller::OriginalClaudeCaller;
        for overflow in [false, true] {
            let cfg = argument_config();
            let seat = cfg.seat;
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let mut caller = OriginalClaudeCaller::new(cfg, sink);
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            let mut context = Some(recovery_context(seat, original.clone()));
            if caller.bind(&mut context).is_err() {
                assert!(caller.results(&original).unwrap().binding.unwrap().is_err());
                continue;
            }
            let mut destination = None;
            let mut occupied = Some(TurnOutcome {
                ok: false,
                error: None,
                text: Some(String::from("occupied capture")),
                usage: Usage::default(),
                quota: Vec::new(),
            });
            let occupied_pointer = occupied.as_ref().unwrap().text.as_ref().unwrap().as_ptr();
            assert!(
                caller
                    .move_captured_into(&original, &foreign, &mut destination)
                    .is_err()
            );
            assert!(
                caller
                    .move_captured_into(&original, &original, &mut occupied)
                    .is_err()
            );
            assert!(std::ptr::eq(
                occupied.as_ref().unwrap().text.as_ref().unwrap().as_ptr(),
                occupied_pointer
            ));
            assert!(
                !caller
                    .move_captured_into(&original, &original, &mut destination)
                    .unwrap()
            );
            assert!(caller.results(&original).unwrap().capture_refusal.is_none());
            let body = String::from("synthetic earlier capture lacks child");
            let pointer = body.as_ptr();
            {
                let prepared = caller
                    .root_for_source_control()
                    .prepared_original(&original)
                    .unwrap();
                let mut preparation = prepared.borrow(&original).unwrap();
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let mut foreground = supervision
                    .root_mut()
                    .unwrap()
                    .borrow_foreground(&original)
                    .unwrap();
                let driver = foreground.driver_mut().unwrap();
                assert!(driver.proc.is_none() && driver.helpers.get_mut().is_empty());
                driver.parser.runtime_operation = Some(original.clone());
                driver.completed_turn = Some(TurnOutcome {
                    ok: false,
                    error: None,
                    text: Some(body),
                    usage: Usage::default(),
                    quota: Vec::new(),
                });
            }
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            if overflow {
                original.overflow();
            }
            assert!(
                caller
                    .move_captured_into(&original, &original, &mut destination)
                    .is_err()
            );
            let cause =
                std::ptr::from_ref(caller.results(&original).unwrap().capture_refusal.unwrap());
            assert!(
                caller
                    .move_captured_into(&original, &original, &mut destination)
                    .is_err()
            );
            assert!(std::ptr::eq(
                caller.results(&original).unwrap().capture_refusal.unwrap(),
                cause
            ));
            assert!(destination.is_none());
            {
                let prepared = caller
                    .root_for_source_control()
                    .prepared_original(&original)
                    .unwrap();
                let mut preparation = prepared.borrow(&original).unwrap();
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let mut foreground = supervision
                    .root_mut()
                    .unwrap()
                    .borrow_foreground(&original)
                    .unwrap();
                let driver = foreground.driver_mut().unwrap();
                assert!(std::ptr::eq(
                    driver
                        .completed_turn
                        .as_ref()
                        .unwrap()
                        .text
                        .as_ref()
                        .unwrap()
                        .as_ptr(),
                    pointer
                ));
                assert!(driver.proc.is_none() && driver.helpers.get_mut().is_empty());
            }
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Configuration measurement only; no original
    // meter/Box/context/native allocation or full-fit receipt is constructed.
    #[test]
    fn original_factory_retains_actual_config_layout_before_operation_creation() {
        use crate::failed_evidence::ArchiveError;
        for mode in 0..3 {
            let mut cfg = argument_config();
            if mode == 0 {
                cfg.exe = None;
                cfg.config_dir = None;
                cfg.expected_email = None;
                cfg.effort = None;
            } else {
                cfg.exe = Some(PathBuf::from("synthetic-cli.exe"));
                cfg.config_dir = Some(PathBuf::from("synthetic-profile"));
                cfg.expected_email = Some(String::from("fixture@example.invalid"));
                cfg.effort = Some(String::from("low"));
            }
            if mode == 2 {
                cfg.system_prompt = String::from("synthetic unicode: λ שלום");
                cfg.system_prompt.reserve_exact(91);
                cfg.model.reserve_exact(39);
                cfg.sandbox.push("synthetic-unicode-λ");
                cfg.sandbox.reserve(67);
                cfg.exe.as_mut().unwrap().reserve(73);
            }
            let pointer = cfg.model.as_ptr();
            let prompt_pointer = cfg.system_prompt.as_ptr();
            let string_bytes = [&cfg.model, &cfg.mcp_url, &cfg.system_prompt]
                .into_iter()
                .map(|body| body.capacity())
                .sum::<usize>()
                + [&cfg.expected_email, &cfg.effort]
                    .into_iter()
                    .flatten()
                    .map(|body| body.capacity())
                    .sum::<usize>();
            let path_bytes = [&cfg.sandbox, &cfg.run_dir]
                .into_iter()
                .map(|path| path.capacity())
                .sum::<usize>()
                + [&cfg.exe, &cfg.config_dir]
                    .into_iter()
                    .flatten()
                    .map(|path| path.capacity())
                    .sum::<usize>();
            let expected = std::mem::size_of_val(&cfg) + string_bytes + path_bytes;
            assert!(matches!(
                cfg.original_creation_layout_with_cap(expected - 1),
                Err(ArchiveError::Fit)
            ));
            let exact = cfg.original_creation_layout_with_cap(expected).unwrap();
            assert_eq!(exact.inline.size(), std::mem::size_of_val(&cfg));
            assert_eq!(exact.inline.align(), std::mem::align_of_val(&cfg));
            assert_eq!(exact.payload_bytes, string_bytes + path_bytes);
            assert!(std::ptr::eq(cfg.model.as_ptr(), pointer));
            assert!(std::ptr::eq(cfg.system_prompt.as_ptr(), prompt_pointer));
            if mode == 2 {
                assert!(cfg.model.capacity() > cfg.model.len());
                assert!(cfg.system_prompt.capacity() > cfg.system_prompt.len());
                assert!(cfg.sandbox.capacity() > cfg.sandbox.as_os_str().as_encoded_bytes().len());
            }
            let sink = TranscriptSink::closed_for_source_control();
            let mut owner = ClaudeDriver::original_owner(cfg, sink);
            let saved = owner.configuration_layout().as_ref().unwrap();
            assert_eq!(saved.inline.size() + saved.payload_bytes, expected);
            let saved_pointer = std::ptr::from_ref(saved);
            assert!(std::ptr::eq(
                owner.configuration_layout().as_ref().unwrap(),
                saved_pointer
            ));
            let (driver, boxing_started, allocation) =
                owner.concrete_factory_for_source_control().unwrap();
            assert!(!boxing_started && allocation.is_none());
            assert!(std::ptr::eq(driver.cfg.model.as_ptr(), pointer));
            assert!(std::ptr::eq(
                driver.cfg.system_prompt.as_ptr(),
                prompt_pointer
            ));
            assert!(driver.host.is_none() && driver.proc.is_none());
            assert!(driver.helpers.get_mut().is_empty() && driver.startup.is_none());
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Empty payload still has actual inline cost;
    // a private tiny-cap refusal is not real allocator-failure/full-fit proof.
    #[test]
    fn original_config_layout_refuses_missing_inline_budget_without_buffers() {
        use crate::failed_evidence::ArchiveError;
        let cfg = ClaudeConfig {
            seat: "axis.commander".parse().unwrap(),
            exe: None,
            model: String::new(),
            config_dir: None,
            expected_email: None,
            sandbox: PathBuf::new(),
            run_dir: PathBuf::new(),
            mcp_url: String::new(),
            system_prompt: String::new(),
            effort: None,
            context_window: None,
        };
        assert!(matches!(
            cfg.original_creation_layout_with_cap(0),
            Err(ArchiveError::Fit)
        ));
        let inline = std::mem::size_of_val(&cfg);
        let measured = cfg.original_creation_layout_with_cap(inline).unwrap();
        assert_eq!(measured.inline.size(), inline);
        assert_eq!(measured.payload_bytes, 0);
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Precreation metadata staging only. No
    // Operation/driver Box/host callback/native work or complete-fit receipt.
    #[test]
    #[cfg(panic = "unwind")]
    fn original_factory_context_keeps_first_layout_and_unwound_input() {
        use super::super::claude_caller::PendingContextInputs;
        use crate::{
            failed_evidence::ArchiveError,
            host::{HostAdmission, HostIdentity},
        };
        use std::cell::Cell;
        fn input(
            seat: cna_core::ids::SeatId,
            admission: Arc<dyn HostAdmission>,
        ) -> PendingContextInputs {
            let mut decision_id = String::from("synthetic-creation-λ");
            decision_id.reserve_exact(47);
            let mut operation_id = String::from("00000000-0000-0000-0000-000000000001");
            operation_id.reserve_exact(53);
            PendingContextInputs {
                admission,
                identity: HostIdentity {
                    seat,
                    epoch: 7,
                    decision_id,
                    revision: 3,
                    operation_id,
                },
            }
        }
        for mode in 0..3 {
            let cfg = argument_config();
            let seat = cfg.seat;
            let model_pointer = cfg.model.as_ptr();
            let sink = TranscriptSink::closed_for_source_control();
            let mut owner = ClaudeDriver::original_owner(cfg, sink);
            let config_pointer = std::ptr::from_ref(owner.configuration_layout());
            let probe = Arc::new(ArchiveAdmissionProbe(std::sync::atomic::AtomicUsize::new(
                0,
            )));
            let admission: Arc<dyn HostAdmission> = probe.clone();
            let mut missing = None;
            assert!(owner.stage_creation_context(&mut missing).is_err());
            assert!(
                owner.creation_context().is_none() && owner.creation_context_layout().is_none()
            );
            let mut foreign = Some(input(
                "allied.commander".parse().unwrap(),
                admission.clone(),
            ));
            let foreign_pointer = foreign.as_ref().unwrap().identity.operation_id.as_ptr();
            assert!(owner.stage_creation_context(&mut foreign).is_err());
            assert!(std::ptr::eq(
                foreign.as_ref().unwrap().identity.operation_id.as_ptr(),
                foreign_pointer
            ));
            assert!(
                owner.creation_context().is_none() && owner.creation_context_layout().is_none()
            );
            let mut candidate = Some(input(seat, admission.clone()));
            let pointer = candidate.as_ref().unwrap().identity.operation_id.as_ptr();
            let decision_pointer = candidate.as_ref().unwrap().identity.decision_id.as_ptr();
            let expected_inline = std::mem::size_of_val(candidate.as_ref().unwrap());
            let expected_payload = candidate.as_ref().unwrap().identity.operation_id.capacity()
                + candidate.as_ref().unwrap().identity.decision_id.capacity();
            if mode == 2 {
                struct Opaque(Cell<usize>); // Send-only source unwind payload.
                let opaque = Box::new(Opaque(Cell::new(41)));
                let opaque_pointer = std::ptr::from_ref(opaque.as_ref());
                let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _ = owner.stage_creation_context_for_source_control(&mut candidate, |_| {
                        std::panic::resume_unwind(opaque)
                    });
                }))
                .unwrap_err();
                let opaque = caught.downcast::<Opaque>().ok().unwrap();
                assert!(std::ptr::eq(opaque.as_ref(), opaque_pointer));
                assert_eq!(opaque.0.get(), 41);
                assert!(owner.creation_context_layout().is_none()); // Unknown, never fabricated success.
            } else if mode == 1 {
                assert!(
                    owner
                        .stage_creation_context_for_source_control(&mut candidate, |input| input
                            .layout_with_cap_for_source_control(0))
                        .is_err()
                );
                assert!(matches!(
                    owner.creation_context_layout(),
                    Some(Err(ArchiveError::Fit))
                ));
            } else {
                owner.stage_creation_context(&mut candidate).unwrap();
                let saved = owner.creation_context_layout().unwrap().as_ref().unwrap();
                assert_eq!(saved.inline.size(), expected_inline);
                assert_eq!(
                    saved.admission_body,
                    std::alloc::Layout::for_value(probe.as_ref()).pad_to_align()
                );
                assert_eq!(saved.payload_bytes, expected_payload);
            }
            assert!(candidate.is_none()); // Accepted metadata moved before measurement/unwind.
            let saved = owner.creation_context().unwrap();
            assert!(std::ptr::eq(saved.identity.operation_id.as_ptr(), pointer));
            assert!(std::ptr::eq(
                saved.identity.decision_id.as_ptr(),
                decision_pointer
            ));
            assert!(Arc::ptr_eq(&saved.admission, &admission));
            assert!(saved.identity.operation_id.capacity() > saved.identity.operation_id.len());
            let layout_pointer = owner.creation_context_layout().map(std::ptr::from_ref);
            let mut repeated = Some(input(seat, admission.clone()));
            let repeated_pointer = repeated.as_ref().unwrap().identity.operation_id.as_ptr();
            assert!(owner.stage_creation_context(&mut repeated).is_err());
            assert!(std::ptr::eq(
                repeated.as_ref().unwrap().identity.operation_id.as_ptr(),
                repeated_pointer
            ));
            assert_eq!(
                owner.creation_context_layout().map(std::ptr::from_ref),
                layout_pointer
            );
            assert!(std::ptr::eq(
                owner
                    .creation_context()
                    .unwrap()
                    .identity
                    .operation_id
                    .as_ptr(),
                pointer
            ));
            assert!(std::ptr::eq(owner.configuration_layout(), config_pointer));
            let (driver, boxing_started, allocation) =
                owner.concrete_factory_for_source_control().unwrap();
            assert!(!boxing_started && allocation.is_none());
            assert!(std::ptr::eq(driver.cfg.model.as_ptr(), model_pointer));
            assert!(driver.host.is_none() && driver.proc.is_none());
            assert!(driver.helpers.get_mut().is_empty() && driver.startup.is_none());
            assert_eq!(probe.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Partial aggregate frame/config/metadata
    // sizing before any Operation. No future is polled/boxed, no native/host
    // callback is invoked, and positive sizing is not complete fit admission.
    #[test]
    fn original_caller_known_creation_keeps_first_result_and_has_no_meter() {
        use super::super::claude_caller::{OriginalClaudeCaller, PendingContextInputs};
        use crate::{failed_evidence::ArchiveError, host::HostIdentity};

        for mode in 0..4 {
            let mut earlier_parser_pointer = None;
            let cfg = argument_config();
            let seat = cfg.seat;
            let model_pointer = cfg.model.as_ptr();
            let config_payload = cfg.original_creation_layout().unwrap().payload_bytes;
            let sink = TranscriptSink::closed_for_source_control();
            let transcript_bytes = sink.original_referenced_layout().unwrap().referenced_bytes;
            let mut caller = OriginalClaudeCaller::new(cfg, sink);
            let probe = Arc::new(ArchiveAdmissionProbe(std::sync::atomic::AtomicUsize::new(
                0,
            )));
            let mut decision_id = String::from("synthetic-known-creation");
            decision_id.reserve_exact(71);
            let mut operation_id = String::from("00000000-0000-0000-0000-000000000001");
            operation_id.reserve_exact(67);
            let context_payload = decision_id.capacity() + operation_id.capacity();
            let decision_pointer = decision_id.as_ptr();
            let operation_pointer = operation_id.as_ptr();
            let mut candidate = Some(PendingContextInputs {
                admission: probe.clone(),
                identity: HostIdentity {
                    seat,
                    epoch: 7,
                    decision_id,
                    revision: 3,
                    operation_id,
                },
            });
            if mode == 3 {
                // Omitted trusted metadata produces a sticky first refusal.
                assert!(caller.measure_known_creation().is_err());
                assert!(matches!(
                    caller.known_creation_layout(),
                    Some(Err(ArchiveError::Identity))
                ));
                assert!(caller.stage_creation_context(&mut candidate).is_err());
                assert!(std::ptr::eq(
                    candidate.as_ref().unwrap().identity.operation_id.as_ptr(),
                    operation_pointer
                ));
                assert!(
                    caller
                        .root_for_source_control()
                        .creation_context()
                        .is_none()
                );
            } else {
                candidate.as_mut().unwrap().identity.seat = "allied.commander".parse().unwrap();
                assert!(caller.stage_creation_context(&mut candidate).is_err());
                assert!(caller.known_creation_layout().is_none());
                assert!(std::ptr::eq(
                    candidate.as_ref().unwrap().identity.operation_id.as_ptr(),
                    operation_pointer
                ));
                candidate.as_mut().unwrap().identity.seat = seat;
                caller.stage_creation_context(&mut candidate).unwrap();
                assert!(candidate.is_none());
                if mode == 2 {
                    // Prior concrete evidence cannot be laundered by measuring
                    // just its layout. The earlier body is retained unchanged.
                    let earlier = String::from("synthetic earlier parser body");
                    earlier_parser_pointer = Some(earlier.as_ptr());
                    caller
                        .root_for_source_control()
                        .concrete_factory_for_source_control()
                        .unwrap()
                        .0
                        .parser
                        .last_text = Some(earlier);
                }
                if mode == 1 {
                    assert!(
                        caller
                            .measure_known_creation_with_cap_for_source_control(0)
                            .is_err()
                    );
                    assert!(matches!(
                        caller.known_creation_layout(),
                        Some(Err(ArchiveError::Fit))
                    ));
                } else if mode == 2 {
                    assert!(caller.measure_known_creation().is_err());
                    assert!(matches!(
                        caller.known_creation_layout(),
                        Some(Err(ArchiveError::Identity))
                    ));
                } else {
                    let caller_inline = std::alloc::Layout::for_value(&caller).pad_to_align();
                    caller.measure_known_creation().unwrap();
                    let saved = caller.known_creation_layout().unwrap().as_ref().unwrap();
                    assert_eq!(saved.caller, caller_inline);
                    assert_eq!(saved.configuration_payload_bytes, config_payload);
                    assert_eq!(saved.context_payload_bytes, context_payload);
                    assert_eq!(
                        saved.admission_body_bytes,
                        std::mem::size_of_val(probe.as_ref())
                    );
                    assert_eq!(saved.transcript_referenced_bytes, transcript_bytes);
                    assert_eq!(
                        saved.known_bytes,
                        saved.fixed_bytes
                            + saved.controllers.overlap_bytes
                            + config_payload
                            + context_payload
                            + saved.admission_body_bytes
                            + saved.transcript_referenced_bytes
                    );
                    assert!(saved.controllers.start.size() > 0);
                    assert_eq!(saved.controllers.start, saved.controllers.turn);
                    assert_eq!(saved.controllers.start, saved.controllers.cleanup);
                    let layouts = (
                        saved.controllers.start,
                        saved.controllers.turn,
                        saved.controllers.cleanup,
                    );
                    let measured_again = caller
                        .root_for_source_control()
                        .creation_frame_layouts()
                        .unwrap();
                    assert_eq!(
                        layouts,
                        (
                            measured_again.start,
                            measured_again.turn,
                            measured_again.cleanup
                        )
                    );
                }
                let context = caller.root_for_source_control().creation_context().unwrap();
                assert!(std::ptr::eq(
                    context.identity.decision_id.as_ptr(),
                    decision_pointer
                ));
                assert!(std::ptr::eq(
                    context.identity.operation_id.as_ptr(),
                    operation_pointer
                ));
                let admission: Arc<dyn crate::host::HostAdmission> = probe.clone();
                assert!(Arc::ptr_eq(&context.admission, &admission));
            }
            let first_result = std::ptr::from_ref(caller.known_creation_layout().unwrap());
            assert!(caller.measure_known_creation().is_err());
            assert!(std::ptr::eq(
                caller.known_creation_layout().unwrap(),
                first_result
            ));
            // Even the positive partial inventory cannot bind an empty input.
            let mut no_context = None;
            assert!(caller.bind(&mut no_context).is_err());
            assert!(no_context.is_none());
            assert!(caller.creation_untouched_for_source_control());
            let (driver, boxing_started, allocation) = caller
                .root_for_source_control()
                .concrete_factory_for_source_control()
                .unwrap();
            assert!(!boxing_started && allocation.is_none());
            assert!(std::ptr::eq(driver.cfg.model.as_ptr(), model_pointer));
            assert!(driver.host.is_none() && driver.proc.is_none());
            assert!(driver.helpers.get_mut().is_empty() && driver.startup.is_none());
            if let Some(pointer) = earlier_parser_pointer {
                assert!(std::ptr::eq(
                    driver.parser.last_text.as_ref().unwrap().as_ptr(),
                    pointer
                ));
            } else {
                assert!(driver.parser.last_text.is_none());
            }
            assert_eq!(probe.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Actual pending factory/root/caller MOVE,
    // with no Operation, driver Box, delivery task or full admission token.
    #[test]
    #[cfg(panic = "unwind")]
    fn pending_factory_keeps_concrete_root_and_inputs_through_refusal_or_unwind() {
        use super::super::claude_caller::{
            OriginalClaudeCaller, PendingClaudeFactory, PendingContextInputs,
        };
        use crate::{
            failed_evidence::ArchiveError,
            transcript::{LocalTranscript, PendingTranscriptInputs},
        };
        struct Opaque(std::cell::Cell<usize>);
        for mode in 0..3 {
            let cfg = argument_config();
            let seat = cfg.seat;
            let model_pointer = cfg.model.as_ptr();
            // Store data/channels only: detached() starts no delivery task.
            let mut pending = PendingTranscriptInputs::new(
                LocalTranscript::detached(),
                Arc::from("00000000-0000-0000-0000-000000000001"),
            );
            if mode == 1 {
                assert!(pending.measure_with_cap_for_source_control(0).is_err());
            }
            let mut factory = PendingClaudeFactory::new(cfg, pending);
            if mode == 1 {
                // Private tiny sizing limit supplies an actual first Fit cause.
                // Preserve it when assembly observes the already refused input.
                let mut refused = factory;
                assert!(refused.assemble().is_err());
                let first = std::ptr::from_ref(refused.assembly_result().unwrap());
                assert!(refused.assemble().is_err());
                assert!(std::ptr::eq(refused.assembly_result().unwrap(), first));
                assert!(std::ptr::eq(
                    refused
                        .retained_configuration_for_source_control()
                        .unwrap()
                        .model
                        .as_ptr(),
                    model_pointer
                ));
                assert!(matches!(
                    refused
                        .transcript_for_source_control()
                        .unwrap()
                        .creation_layout(),
                    Some(Err(ArchiveError::Fit))
                ));
                assert!(refused.owner_for_source_control().is_none());
                let mut refused = match OriginalClaudeCaller::from_pending_factory(refused) {
                    Err((ArchiveError::Identity, refused)) => refused,
                    _ => panic!("partial refused factory must be returned owned"),
                };
                assert!(
                    refused
                        .transcript_for_source_control()
                        .unwrap()
                        .unstarted_for_source_control()
                );
                assert!(matches!(
                    refused
                        .transcript_for_source_control()
                        .unwrap()
                        .creation_layout(),
                    Some(Err(ArchiveError::Fit))
                ));
                assert!(refused.owner_for_source_control().is_none());
                continue;
            }
            if mode == 2 {
                let payload = Box::new(Opaque(std::cell::Cell::new(67)));
                let payload_pointer = std::ptr::from_ref(payload.as_ref());
                let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _ = factory
                        .assemble_unwind_for_source_control(|| std::panic::resume_unwind(payload));
                }))
                .unwrap_err();
                let payload = caught.downcast::<Opaque>().ok().unwrap();
                assert!(std::ptr::eq(payload.as_ref(), payload_pointer));
                assert_eq!(payload.0.get(), 67);
                assert!(factory.assembly_result().is_none()); // Unfinished, never fake success.
            } else {
                factory.assemble().unwrap();
                assert!(matches!(factory.assembly_result(), Some(Ok(()))));
            }
            assert!(factory.assemble().is_err());
            let pending = factory.transcript_for_source_control().unwrap();
            assert!(pending.unstarted_for_source_control());
            let pending_bytes = pending
                .creation_layout()
                .unwrap()
                .as_ref()
                .unwrap()
                .known_bytes;
            let root = factory.owner_for_source_control().unwrap();
            assert!(root.creation_untouched_for_source_control());
            let (driver, boxing_started, allocation) =
                root.concrete_factory_for_source_control().unwrap();
            assert!(!boxing_started && allocation.is_none());
            assert!(std::ptr::eq(driver.cfg.model.as_ptr(), model_pointer));
            assert!(driver.host.is_none() && driver.proc.is_none());
            if mode == 2 {
                let mut returned = match OriginalClaudeCaller::from_pending_factory(factory) {
                    Err((ArchiveError::Identity, factory)) => factory,
                    _ => panic!("unfinished assembly must stay owned"),
                };
                assert!(returned.assembly_result().is_none());
                assert!(
                    returned
                        .transcript_for_source_control()
                        .unwrap()
                        .unstarted_for_source_control()
                );
                assert!(
                    returned
                        .owner_for_source_control()
                        .unwrap()
                        .creation_untouched_for_source_control()
                );
            } else {
                assert!(root.pending_matches_driver_for_source_control());
                let mut caller = match OriginalClaudeCaller::from_pending_factory(factory) {
                    Ok(caller) => caller,
                    Err(_) => panic!("input assembly must MOVE to caller"),
                };
                assert!(matches!(caller.factory_assembly_result(), Some(Ok(()))));
                assert!(
                    caller
                        .root_for_source_control()
                        .pending_matches_driver_for_source_control()
                );
                let mut context = Some(PendingContextInputs {
                    admission: Arc::new(RecoveryClosedAdmission),
                    identity: crate::host::HostIdentity {
                        seat,
                        epoch: 7,
                        decision_id: String::from("synthetic pending factory context"),
                        revision: 3,
                        operation_id: String::from("00000000-0000-0000-0000-000000000001"),
                    },
                });
                caller.stage_creation_context(&mut context).unwrap();
                assert!(context.is_none());
                caller.measure_known_creation().unwrap();
                assert_eq!(
                    caller
                        .known_creation_layout()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .pending_transcript_bytes,
                    pending_bytes
                );
                assert!(caller.creation_untouched_for_source_control());
                assert!(
                    caller
                        .root_for_source_control()
                        .pending_transcript_for_source_control()
                        .unwrap()
                        .unstarted_for_source_control()
                );
            }
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Synthetic descriptor producer types, never
    // actual Server-layout equality or successful original Operation creation.
    #[test]
    fn precreation_owner_layout_checks_same_instance_and_keeps_first_layout_result() {
        use super::super::claude_caller::{PendingClaudeFactory, PendingContextInputs};
        use crate::{
            failed_evidence::ArchiveError,
            host::{BoundHostOperationLayout, HostAdmission, HostIdentity},
            transcript::{LocalTranscript, PendingTranscriptInputs},
        };
        #[repr(align(64))]
        struct Storage {
            _entry: [u8; 53],
            _permit: [u8; 31],
        }
        for oversized in [false, true] {
            let cfg = argument_config();
            let seat = cfg.seat;
            let model_pointer = cfg.model.as_ptr();
            let pending = PendingTranscriptInputs::new(
                LocalTranscript::detached(),
                Arc::from("00000000-0000-0000-0000-000000000001"),
            );
            let mut factory = PendingClaudeFactory::new(cfg, pending);
            factory.assemble().unwrap();
            let mut caller = match OriginalClaudeCaller::from_pending_factory(factory) {
                Ok(caller) => caller,
                Err(_) => panic!("source factory must transfer"),
            };
            let admission: Arc<dyn HostAdmission> = Arc::new(RecoveryClosedAdmission);
            let foreign: Arc<dyn HostAdmission> = Arc::new(RecoveryClosedAdmission);
            let mut context = Some(PendingContextInputs {
                admission: admission.clone(),
                identity: HostIdentity {
                    seat,
                    epoch: 7,
                    revision: 3,
                    decision_id: String::from("synthetic descriptor context"),
                    operation_id: String::from("00000000-0000-0000-0000-000000000001"),
                },
            });
            caller.stage_creation_context(&mut context).unwrap();
            let mut absent = None;
            assert!(caller.stage_owner_layout(&mut absent).is_err());
            assert!(
                caller
                    .root_for_source_control()
                    .owner_layout_bytes()
                    .is_none()
            );
            let mut wrong = Some(BoundHostOperationLayout::for_storage::<Storage>(
                foreign.clone(),
            ));
            assert!(caller.stage_owner_layout(&mut wrong).is_err());
            assert!(wrong.as_ref().unwrap().matches_owner(&foreign));
            assert!(
                caller
                    .root_for_source_control()
                    .owner_layout_bytes()
                    .is_none()
            );
            let mut original = Some(if oversized {
                BoundHostOperationLayout::for_storage::<[u8; crate::operation::OPERATION_BYTES + 1]>(
                    admission.clone(),
                )
            } else {
                BoundHostOperationLayout::for_storage::<Storage>(admission.clone())
            });
            let staged = caller.stage_owner_layout(&mut original);
            assert!(original.is_none()); // Canonical descriptor retained even on Fit.
            let first = caller
                .root_for_source_control()
                .owner_layout_bytes()
                .unwrap();
            let first_pointer = std::ptr::from_ref(first);
            if oversized {
                assert!(staged.is_err() && matches!(first, Err(ArchiveError::Fit)));
                assert!(caller.measure_known_creation().is_err());
            } else {
                staged.unwrap();
                assert_eq!(*first.as_ref().unwrap(), std::mem::size_of::<Storage>());
                caller.measure_known_creation().unwrap();
                let known = caller.known_creation_layout().unwrap().as_ref().unwrap();
                assert_eq!(known.owner_persistent_bytes, std::mem::size_of::<Storage>());
                assert_eq!(
                    known.known_bytes,
                    known.fixed_bytes
                        + known.controllers.overlap_bytes
                        + known.configuration_payload_bytes
                        + known.context_payload_bytes
                        + known.admission_body_bytes
                        + known.transcript_referenced_bytes
                        + known.pending_transcript_bytes
                        + std::mem::size_of::<Storage>()
                );
            }
            let mut repeat = Some(BoundHostOperationLayout::for_storage::<Storage>(admission));
            assert!(caller.stage_owner_layout(&mut repeat).is_err());
            assert!(repeat.is_some() && wrong.is_some());
            assert!(std::ptr::eq(
                caller
                    .root_for_source_control()
                    .owner_layout_bytes()
                    .unwrap(),
                first_pointer
            ));
            assert!(caller.original_creation_result().is_none());
            assert!(caller.creation_untouched_for_source_control());
            let owner = caller.root_for_source_control();
            assert!(
                owner
                    .pending_transcript_for_source_control()
                    .unwrap()
                    .unstarted_for_source_control()
            );
            let (driver, boxing_started, allocation) =
                owner.concrete_factory_for_source_control().unwrap();
            assert!(std::ptr::eq(driver.cfg.model.as_ptr(), model_pointer));
            assert!(
                !boxing_started
                    && allocation.is_none()
                    && driver.host.is_none()
                    && driver.proc.is_none()
            );
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. These negative controls construct NO
    // Operation/task/driver Box. A partial inventory must never admit one.
    #[test]
    fn original_creation_keeps_first_typed_refusal_before_any_operation() {
        use super::super::claude_caller::{PendingClaudeFactory, PendingContextInputs};
        use crate::{
            failed_evidence::{ArchiveError, CampaignArchive},
            operation::{Allowance, OperationError},
            transcript::{LocalTranscript, PendingTranscriptInputs},
        };

        fn archive(label: &str) -> CampaignArchive {
            // Pure path validation/configuration only, no filesystem operation.
            let root = std::env::temp_dir().join(label);
            CampaignArchive::new(
                &root.join("00000000-0000-0000-0000-000000000001.sqlite"),
                &root.join("sandbox"),
                &root.join("run"),
                &[root.join("isolation")],
            )
            .unwrap()
        }

        let original_archive = archive("source-original-creation-no-io");
        let foreign_archive = archive("source-foreign-creation-no-io");
        for mode in 0..4 {
            let cfg = argument_config();
            let seat = cfg.seat;
            let model_pointer = cfg.model.as_ptr();
            let mut caller = if mode == 2 {
                OriginalClaudeCaller::new(cfg, TranscriptSink::closed_for_source_control())
            } else {
                let pending = PendingTranscriptInputs::new(
                    LocalTranscript::detached(),
                    Arc::from("00000000-0000-0000-0000-000000000001"),
                );
                let mut factory = PendingClaudeFactory::new(cfg, pending);
                factory.assemble().unwrap();
                match OriginalClaudeCaller::from_pending_factory(factory) {
                    Ok(caller) => caller,
                    Err(_) => panic!("actual source input assembly must transfer"),
                }
            };
            if mode == 0 {
                let mut context = Some(PendingContextInputs {
                    admission: Arc::new(RecoveryClosedAdmission),
                    identity: crate::host::HostIdentity {
                        seat,
                        epoch: 7,
                        decision_id: String::from("synthetic negative creation"),
                        revision: 3,
                        operation_id: String::from("00000000-0000-0000-0000-000000000001"),
                    },
                });
                caller.stage_creation_context(&mut context).unwrap();
                assert!(context.is_none());
                caller.measure_known_creation().unwrap();
                assert!(matches!(caller.known_creation_layout(), Some(Ok(_))));
            } else {
                assert!(caller.known_creation_layout().is_none());
            }
            let fresh =
                Allowance::required(Some(tokio::time::Instant::now() + Duration::from_secs(60)))
                    .unwrap();
            let allowance = if mode == 3 {
                fresh.capped(Duration::ZERO)
            } else {
                fresh
            };
            assert!(caller.original_creation_result().is_none());
            assert!(caller.original_creation_allowance().is_none());
            assert!(matches!(
                original_archive.original_operation(&mut caller, allowance),
                Err(ArchiveError::Identity)
            ));
            let first = caller.original_creation_result().unwrap();
            match mode {
                2 => assert!(matches!(first, Err(ArchiveError::Identity))),
                3 => assert!(matches!(
                    first,
                    Err(ArchiveError::Operation(OperationError::Expired))
                )),
                _ => assert!(matches!(first, Err(ArchiveError::Fit))),
            }
            let first_pointer = std::ptr::from_ref(first);
            let first_deadline = allowance.deadline();
            assert_eq!(
                caller.original_creation_allowance().unwrap().deadline(),
                first_deadline
            );

            // A different archive/new allowance cannot replace the first cause
            // or renew the original absolute deadline. Neither archive is used.
            let retry =
                Allowance::required(Some(tokio::time::Instant::now() + Duration::from_secs(120)))
                    .unwrap();
            assert!(matches!(
                foreign_archive.original_operation(&mut caller, retry),
                Err(ArchiveError::Identity)
            ));
            assert!(std::ptr::eq(
                caller.original_creation_result().unwrap(),
                first_pointer
            ));
            assert_eq!(
                caller.original_creation_allowance().unwrap().deadline(),
                first_deadline
            );
            assert!(caller.measure_known_creation().is_err());
            let mut later_context = Some(PendingContextInputs {
                admission: Arc::new(RecoveryClosedAdmission),
                identity: crate::host::HostIdentity {
                    seat,
                    epoch: 7,
                    decision_id: String::from("later must stay owned"),
                    revision: 3,
                    operation_id: String::from("00000000-0000-0000-0000-000000000002"),
                },
            });
            let context_pointer = later_context
                .as_ref()
                .unwrap()
                .identity
                .decision_id
                .as_ptr();
            assert!(caller.stage_creation_context(&mut later_context).is_err());
            assert!(std::ptr::eq(
                later_context
                    .as_ref()
                    .unwrap()
                    .identity
                    .decision_id
                    .as_ptr(),
                context_pointer
            ));
            assert!(std::ptr::eq(
                caller.original_creation_result().unwrap(),
                first_pointer
            ));
            assert!(caller.creation_untouched_for_source_control());
            let owner = caller.root_for_source_control();
            if mode != 2 {
                assert!(owner.pending_matches_driver_for_source_control());
                assert!(
                    owner
                        .pending_transcript_for_source_control()
                        .unwrap()
                        .unstarted_for_source_control()
                );
            }
            let (driver, boxing_started, allocation) =
                owner.concrete_factory_for_source_control().unwrap();
            assert!(!boxing_started && allocation.is_none());
            assert!(std::ptr::eq(driver.cfg.model.as_ptr(), model_pointer));
            assert!(driver.host.is_none() && driver.proc.is_none());
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Same-text foreign producer/receiver root
    // cannot attach. Both candidates and all unstarted worker inputs survive.
    #[test]
    fn pending_factory_rejects_foreign_producer_before_consuming_worker_inputs() {
        use crate::{
            failed_evidence::ArchiveError,
            transcript::{LocalTranscript, PendingTranscriptInputs},
        };
        let mut original = PendingTranscriptInputs::new(
            LocalTranscript::detached(),
            Arc::from("00000000-0000-0000-0000-000000000001"),
        );
        let mut foreign = PendingTranscriptInputs::new(
            LocalTranscript::detached(),
            Arc::from("00000000-0000-0000-0000-000000000001"),
        );
        original.measure_creation().unwrap();
        foreign.measure_creation().unwrap();
        let mut original_producer = None;
        let mut foreign_producer = None;
        original.move_producer_into(&mut original_producer).unwrap();
        foreign.move_producer_into(&mut foreign_producer).unwrap();
        let cfg = argument_config();
        let model_pointer = cfg.model.as_ptr();
        let mut root = ClaudeDriver::original_owner(cfg, original_producer.take().unwrap());
        let mut foreign = Some(foreign);
        assert!(matches!(
            root.attach_pending_transcript(&mut foreign),
            Err(ArchiveError::Identity)
        ));
        assert!(foreign.as_ref().unwrap().unstarted_for_source_control());
        assert!(
            foreign
                .as_ref()
                .unwrap()
                .matches_transferred_producer(foreign_producer.as_ref().unwrap())
        );
        assert!(root.pending_transcript_for_source_control().is_none());
        assert!(root.creation_untouched_for_source_control());
        let mut original = Some(original);
        root.attach_pending_transcript(&mut original).unwrap();
        assert!(original.is_none());
        assert!(root.pending_matches_driver_for_source_control());
        assert!(root.attach_pending_transcript(&mut foreign).is_err());
        assert!(foreign.as_ref().unwrap().unstarted_for_source_control());
        let (driver, boxing_started, allocation) =
            root.concrete_factory_for_source_control().unwrap();
        assert!(!boxing_started && allocation.is_none());
        assert!(std::ptr::eq(driver.cfg.model.as_ptr(), model_pointer));
        assert!(driver.host.is_none() && driver.proc.is_none());
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Actual dynamic concrete Layout, no host
    // methods. Nested Vec allocation deliberately remains OUTSIDE this partial
    // measurement, demonstrating why it cannot certify the referenced graph.
    #[test]
    fn pending_context_layout_counts_dynamic_admission_object_without_host_calls() {
        use super::super::claude_caller::PendingContextInputs;
        use crate::{failed_evidence::ArchiveError, host::*};
        struct NoCalls<T>(T);
        impl<T: Send + Sync> HostAdmission for NoCalls<T> {
            fn subscribe(&self) -> tokio::sync::watch::Receiver<HostSnapshot> {
                panic!("layout must not subscribe")
            }
            fn snapshot(&self) -> Result<HostSnapshot, HostRefusal> {
                panic!("layout must not take a host snapshot")
            }
            fn acquire(
                &self,
                _: &HostIdentity,
                _: HostStep,
            ) -> Result<Box<dyn HostStepLease>, HostRefusal> {
                panic!("layout must not acquire")
            }
            fn acquire_prompt(
                &self,
                _: &HostIdentity,
                _: crate::operation::Allowance,
            ) -> Result<Box<dyn HostPromptLease>, HostRefusal> {
                panic!("layout must not acquire prompt")
            }
        }
        #[repr(align(64))]
        struct Inline([u64; 37]);
        let inline = Arc::new(NoCalls(Inline([17; 37])));
        assert_eq!(inline.0.0[36], 17);
        let mut heap = Vec::with_capacity(4096);
        heap.push(23u8);
        let nested = Arc::new(NoCalls(heap));
        let nested_pointer = nested.0.as_ptr();
        let nested_capacity = nested.0.capacity();
        let objects: [(Arc<dyn HostAdmission>, std::alloc::Layout); 2] = [
            (
                inline.clone(),
                std::alloc::Layout::for_value(inline.as_ref()).pad_to_align(),
            ),
            (
                nested.clone(),
                std::alloc::Layout::for_value(nested.as_ref()).pad_to_align(),
            ),
        ];
        for (admission, expected) in objects {
            let mut decision_id = String::from("synthetic-dynamic-layout");
            decision_id.reserve_exact(37);
            let operation_id = String::from("00000000-0000-0000-0000-000000000001");
            let candidate = PendingContextInputs {
                admission,
                identity: HostIdentity {
                    seat: "axis.commander".parse().unwrap(),
                    epoch: 7,
                    decision_id,
                    revision: 3,
                    operation_id,
                },
            };
            let pointer = candidate.identity.decision_id.as_ptr();
            let exact = std::mem::size_of_val(&candidate)
                + candidate.identity.decision_id.capacity()
                + candidate.identity.operation_id.capacity()
                + expected.size();
            assert!(matches!(
                candidate.layout_with_cap_for_source_control(exact - 1),
                Err(ArchiveError::Fit)
            ));
            let measured = candidate.layout_with_cap_for_source_control(exact).unwrap();
            assert_eq!(measured.admission_body, expected);
            assert!(std::ptr::eq(
                candidate.identity.decision_id.as_ptr(),
                pointer
            ));
        }
        assert_eq!(nested.0.len(), 1);
        assert_eq!(nested.0.capacity(), nested_capacity);
        assert!(std::ptr::eq(nested.0.as_ptr(), nested_pointer));
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. No meter is created: prior driver evidence
    // is a refusal, not permission to reclassify that concrete value as fresh.
    #[test]
    fn original_factory_context_refuses_prior_parser_body_without_consuming_input() {
        use super::super::claude_caller::PendingContextInputs;
        let cfg = argument_config();
        let seat = cfg.seat;
        let sink = TranscriptSink::closed_for_source_control();
        let mut owner = ClaudeDriver::original_owner(cfg, sink);
        let body = String::from("synthetic previously owned parser body");
        let pointer = body.as_ptr();
        owner
            .concrete_factory_for_source_control()
            .unwrap()
            .0
            .parser
            .last_text = Some(body);
        let mut input = Some(PendingContextInputs {
            admission: Arc::new(RecoveryClosedAdmission),
            identity: crate::host::HostIdentity {
                seat,
                epoch: 7,
                decision_id: String::from("synthetic"),
                revision: 3,
                operation_id: String::from("00000000-0000-0000-0000-000000000001"),
            },
        });
        let input_pointer = input.as_ref().unwrap().identity.operation_id.as_ptr();
        assert!(owner.stage_creation_context(&mut input).is_err());
        assert!(std::ptr::eq(
            input.as_ref().unwrap().identity.operation_id.as_ptr(),
            input_pointer
        ));
        assert!(owner.creation_context().is_none() && owner.creation_context_layout().is_none());
        let (driver, started, allocation) = owner.concrete_factory_for_source_control().unwrap();
        assert!(!started && allocation.is_none());
        assert!(std::ptr::eq(
            driver.parser.last_text.as_ref().unwrap().as_ptr(),
            pointer
        ));
        assert!(driver.host.is_none() && driver.proc.is_none());
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Actual private concrete factory, no public
    // facade/native future or allocator-failure witness; no archive I/O.
    #[test]
    #[cfg(panic = "unwind")]
    fn concrete_factory_retains_inline_and_same_first_box_without_reboxing() {
        use super::super::archive_work::{DispatchError, OriginalClaudeOwner};
        use crate::failed_evidence::ArchiveError;
        fn ready_snapshot(
            owner: &mut OriginalClaudeOwner,
            original: &crate::operation::Operation,
        ) -> (usize, usize, usize) {
            let prepared = owner.prepared_original(original).unwrap();
            let mut preparation = prepared.borrow(original).unwrap();
            let mut supervisor = preparation.supervisor().unwrap().borrow(original).unwrap();
            let foreground = supervisor
                .root_mut()
                .unwrap()
                .borrow_foreground(original)
                .unwrap();
            let driver = foreground.driver().unwrap();
            assert!(driver.proc.is_none() && driver.helpers.try_lock().unwrap().is_empty());
            assert!(driver.prompt_lease.is_none() && driver.startup.is_none());
            (
                std::ptr::from_ref(driver) as usize,
                driver.cfg.model.as_ptr() as usize,
                driver.host.as_ref().unwrap().identity.operation_id.as_ptr() as usize,
            )
        }
        for case in 0..4 {
            let cfg = argument_config();
            let model_pointer = cfg.model.as_ptr() as usize;
            let seat = cfg.seat;
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let mut owner = ClaudeDriver::original_owner(cfg, sink);
            let saved = String::from("original factory evidence");
            let saved_pointer = saved.as_ptr();
            {
                let (driver, started, allocation) =
                    owner.concrete_factory_for_source_control().unwrap();
                assert!(!started && allocation.is_none());
                assert_eq!(driver.cfg.model.as_ptr() as usize, model_pointer);
                assert!(driver.proc.is_none() && driver.host.is_none());
                if case == 1 {
                    driver.parser.last_text = Some(saved);
                } else {
                    drop(saved);
                }
            }
            // ORIGINAL is created only after the concrete root exists. Existing
            // archive fixture admits fixed workspace; full growing/factory fit
            // remains a production obligation, not a claim from this fixture.
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            let context = recovery_context(seat, original.clone());
            let context_pointer = context.identity.operation_id.as_ptr() as usize;
            let captured = original.captured_bytes();
            let deadline = original.allowance().deadline();
            if case == 2 {
                original.overflow();
            }
            if case == 3 {
                struct Opaque(std::cell::Cell<u32>);
                let payload = Box::new(Opaque(std::cell::Cell::new(59)));
                let payload_pointer = std::ptr::from_ref(payload.as_ref());
                let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _ = owner.bind_original_box_for_source_control(context, || {
                        std::panic::resume_unwind(payload); // Empty Box retained, inline body not moved.
                    });
                }))
                .unwrap_err();
                let payload = panic.downcast::<Opaque>().ok().unwrap();
                assert_eq!(std::ptr::from_ref(payload.as_ref()), payload_pointer);
                assert_eq!(payload.0.get(), 59);
                assert!(owner.first_refusal(&original).unwrap().is_none());
            } else {
                let result = owner.bind_original_context(context);
                if case == 0 {
                    assert!(result.is_ok());
                } else {
                    assert!(matches!(result, Err(ArchiveError::Identity)));
                }
            }
            let retained_snapshot;
            if case == 0 {
                retained_snapshot = ready_snapshot(&mut owner, &original);
                assert_eq!(retained_snapshot.1, model_pointer);
                assert_eq!(retained_snapshot.2, context_pointer);
            } else {
                let (driver, started, allocation) =
                    owner.concrete_factory_for_source_control().unwrap();
                assert_eq!(started, case != 2);
                assert_eq!(allocation.is_some(), case == 3);
                assert_eq!(driver.cfg.model.as_ptr() as usize, model_pointer);
                assert!(driver.host.is_none() && driver.proc.is_none());
                retained_snapshot = (
                    std::ptr::from_ref(driver) as usize,
                    model_pointer,
                    allocation.unwrap_or(0),
                );
                if case == 1 {
                    assert_eq!(
                        driver.parser.last_text.as_ref().unwrap().as_ptr(),
                        saved_pointer
                    );
                }
                let (driver, pending) = owner.unprepared_for_source_control(&original).unwrap();
                assert_eq!(std::ptr::from_ref(driver) as usize, retained_snapshot.0);
                assert_eq!(
                    pending.unwrap().identity.operation_id.as_ptr() as usize,
                    context_pointer
                );
                assert!(owner.prepared_original(&original).is_err());
                if case == 1 {
                    assert!(matches!(
                        owner.first_refusal(&original).unwrap(),
                        Some(DispatchError::Driver(DriverError::Host(
                            crate::host::HostRefusal::Ownership(_)
                        )))
                    ));
                } else if case == 2 {
                    assert!(matches!(
                        owner.first_refusal(&original).unwrap(),
                        Some(DispatchError::Driver(DriverError::Host(
                            crate::host::HostRefusal::CaptureOverflow
                        )))
                    ));
                }
            }
            assert!(
                owner
                    .bind_original_context(recovery_context(seat, foreign))
                    .is_err()
            );
            assert!(
                owner
                    .bind_original_context(recovery_context(seat, original.clone()))
                    .is_err()
            );
            if case == 0 {
                assert_eq!(ready_snapshot(&mut owner, &original), retained_snapshot);
            } else {
                let (driver, _, allocation) = owner.concrete_factory_for_source_control().unwrap();
                assert_eq!(
                    (
                        std::ptr::from_ref(driver) as usize,
                        driver.cfg.model.as_ptr() as usize,
                        allocation.unwrap_or(0)
                    ),
                    retained_snapshot
                );
                let (_, pending) = owner.unprepared_for_source_control(&original).unwrap();
                assert_eq!(
                    pending.unwrap().identity.operation_id.as_ptr() as usize,
                    context_pointer
                );
            }
            assert_eq!(original.captured_bytes(), captured);
            assert_eq!(original.allowance().deadline(), deadline);
        }
    }

    // SOURCE ONLY/UNRUN. Private owner installation, not factory/native adoption.
    #[test]
    #[cfg(panic = "unwind")]
    fn original_controller_retains_context_box_and_failed_binding_without_retry() {
        use super::super::archive_work::{DispatchError, OriginalClaudeOwner, PreparationPosition};
        use crate::failed_evidence::ArchiveError;
        fn send_sync<T: Send + Sync>() {}
        send_sync::<OriginalClaudeOwner>(); // Opaque prepared payload may be Send-only.
        for case in 0..4 {
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let mut driver = Box::new(ClaudeDriver::new(argument_config(), sink));
            let pointer = std::ptr::from_ref(driver.as_ref());
            let seat = driver.cfg.seat;
            let saved = String::from("original saved refusal evidence");
            let saved_pointer = saved.as_ptr();
            if case == 1 {
                driver.parser.last_text = Some(saved); // Synthetic existing evidence blocks binding.
            }
            let context = recovery_context(seat, original.clone());
            let context_pointer = context.identity.operation_id.as_ptr();
            let mut owner = OriginalClaudeOwner::new(driver);
            if case == 2 {
                original.overflow();
            }
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            if case == 3 {
                struct Opaque(std::cell::Cell<usize>);
                let payload = Box::new(Opaque(std::cell::Cell::new(73)));
                let payload_pointer = std::ptr::from_ref(payload.as_ref());
                let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _ = owner.bind_original_context_for_source_control(context, || {
                        std::panic::resume_unwind(payload); // After empty-root allocation, before Box MOVE.
                    });
                }));
                let opaque = match unwind {
                    Err(payload) => payload.downcast::<Opaque>().ok().unwrap(),
                    Ok(()) => panic!("original binding never unwound"),
                };
                assert_eq!(std::ptr::from_ref(opaque.as_ref()), payload_pointer);
                assert_eq!(opaque.0.get(), 73);
                let (driver, pending) = owner.unprepared_for_source_control(&original).unwrap();
                assert_eq!(std::ptr::from_ref(driver), pointer);
                assert!(pending.is_none()); // SAME context has moved to SAME Box, not dropped.
                assert_eq!(
                    driver.host.as_ref().unwrap().identity.operation_id.as_ptr(),
                    context_pointer
                );
                assert!(
                    driver
                        .host
                        .as_ref()
                        .unwrap()
                        .operation
                        .same_instance(&original)
                );
                assert!(owner.first_refusal(&original).unwrap().is_none()); // Unwind is not a typed error.
                assert!(owner.prepared_original(&original).is_err()); // Still NOT ready.
            } else {
                let bound = owner.bind_original_context(context);
                if case == 0 {
                    assert!(bound.is_ok());
                    let prepared = owner.prepared_original(&original).unwrap();
                    assert_eq!(
                        prepared.position(&original).unwrap(),
                        (PreparationPosition::Ready, false)
                    );
                    let mut preparation = prepared.borrow(&original).unwrap();
                    let mut supervision =
                        preparation.supervisor().unwrap().borrow(&original).unwrap();
                    let foreground = supervision
                        .root_mut()
                        .unwrap()
                        .borrow_foreground(&original)
                        .unwrap();
                    let driver = foreground.driver().unwrap();
                    assert_eq!(std::ptr::from_ref(driver), pointer);
                    assert_eq!(
                        driver.host.as_ref().unwrap().identity.operation_id.as_ptr(),
                        context_pointer
                    );
                    assert!(
                        driver
                            .host
                            .as_ref()
                            .unwrap()
                            .operation
                            .same_instance(&original)
                    );
                    assert!(driver.proc.is_none() && driver.helpers.try_lock().unwrap().is_empty());
                    assert!(driver.prompt_lease.is_none() && driver.startup.is_none());
                } else {
                    assert!(matches!(bound, Err(ArchiveError::Identity)));
                    let (driver, pending) = owner.unprepared_for_source_control(&original).unwrap();
                    assert_eq!(std::ptr::from_ref(driver), pointer);
                    assert!(driver.host.is_none());
                    let context = pending.unwrap();
                    assert_eq!(context.identity.operation_id.as_ptr(), context_pointer);
                    assert!(context.operation.same_instance(&original));
                    if case == 1 {
                        assert_eq!(
                            driver.parser.last_text.as_ref().unwrap().as_ptr(),
                            saved_pointer
                        );
                        assert!(matches!(
                            owner.first_refusal(&original).unwrap(),
                            Some(DispatchError::Driver(DriverError::Host(
                                crate::host::HostRefusal::Ownership(_)
                            )))
                        ));
                    } else {
                        assert!(matches!(
                            owner.first_refusal(&original).unwrap(),
                            Some(DispatchError::Driver(DriverError::Host(
                                crate::host::HostRefusal::CaptureOverflow
                            )))
                        ));
                    }
                    assert!(owner.prepared_original(&original).is_err());
                }
            }
            assert!(owner.prepared_original(&foreign).is_err());
            assert!(owner.first_refusal(&foreign).is_err());
            // ANY attempted bind, including unwind, forbids a successor or retry.
            assert!(matches!(
                owner.bind_original_context(recovery_context(seat, foreign)),
                Err(ArchiveError::Identity)
            ));
            assert!(matches!(
                owner.bind_original_context(recovery_context(seat, original.clone())),
                Err(ArchiveError::Identity)
            ));
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
            // No start/turn/worker/archive/physical join/custody/healthy witness.
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Synthetic returns, actual restoration wake
    // unwind; no actual driver/native future or physical supervision is polled.
    #[test]
    #[cfg(panic = "unwind")]
    fn canonical_start_return_survives_restoration_waker_unwind() {
        use super::super::archive_work::{DispatchError, OriginalClaudeOwner};
        struct Opaque(std::cell::Cell<usize>);
        struct WakePanic(std::sync::Mutex<Option<Box<Opaque>>>);
        impl std::task::Wake for WakePanic {
            fn wake(self: Arc<Self>) {
                Self::wake_by_ref(&self);
            }
            fn wake_by_ref(self: &Arc<Self>) {
                let payload = self.0.lock().unwrap().take();
                if let Some(payload) = payload {
                    std::panic::resume_unwind(payload);
                }
            }
        }
        for failed in [false, true] {
            let original = archive_startup_operation();
            let driver = Box::new(ClaudeDriver::new(
                argument_config(),
                TranscriptSink::closed_for_source_control(),
            ));
            let driver_pointer = std::ptr::from_ref(driver.as_ref());
            let context = recovery_context(driver.cfg.seat, original.clone());
            let mut owner = OriginalClaudeOwner::new(driver);
            owner.bind_original_context(context).unwrap();
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            let body = String::from("synthetic original retained before restoration");
            let body_pointer = body.as_ptr();
            let outcome = if failed {
                Err(DispatchError::Driver(DriverError::Host(
                    crate::host::HostRefusal::Ownership(body),
                )))
            } else {
                Ok(SessionInfo {
                    session_id: body,
                    model: None,
                    cli_version: None,
                    resumed: false,
                })
            };
            let payload = Box::new(Opaque(std::cell::Cell::new(83)));
            let payload_pointer = std::ptr::from_ref(payload.as_ref());
            let wake = Arc::new(WakePanic(std::sync::Mutex::new(Some(payload))));
            let waker = std::task::Waker::from(wake.clone());
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                owner
                    .retained_start_wake_for_source_control(&original, outcome, &waker)
                    .unwrap();
            }))
            .unwrap_err();
            let opaque = caught.downcast::<Opaque>().ok().unwrap();
            assert!(std::ptr::eq(opaque.as_ref(), payload_pointer));
            assert_eq!(opaque.0.get(), 83);
            assert!(wake.0.lock().unwrap().is_none());
            let results = owner.retained_results(&original).unwrap();
            assert!(results.start_started && results.turn.is_none() && results.cleanup.is_none());
            assert_eq!(results.start_returned_ok, Some(!failed));
            let retained = results.start.unwrap();
            let retained_pointer = std::ptr::from_ref(retained);
            match retained {
                Ok(info) => {
                    assert!(!failed);
                    assert!(std::ptr::eq(info.session_id.as_ptr(), body_pointer));
                }
                Err(DispatchError::Driver(DriverError::Host(
                    crate::host::HostRefusal::Ownership(body),
                ))) => {
                    assert!(failed);
                    assert!(std::ptr::eq(body.as_ptr(), body_pointer));
                }
                _ => panic!("original synthetic return was lost/replaced"),
            }
            assert!(owner.begin_start_for_source_control(&original).is_err());
            assert!(std::ptr::eq(
                owner.retained_results(&original).unwrap().start.unwrap(),
                retained_pointer
            ));
            // Every actual nested lease restored the SAME Box despite wake unwind.
            {
                let mut preparation = owner
                    .prepared_original(&original)
                    .unwrap()
                    .borrow(&original)
                    .unwrap();
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let mut foreground = supervision
                    .root_mut()
                    .unwrap()
                    .borrow_foreground(&original)
                    .unwrap();
                let driver = foreground.driver_mut().unwrap();
                assert!(std::ptr::eq(driver, driver_pointer));
                assert!(driver.proc.is_none() && driver.helpers.try_lock().unwrap().is_empty());
            }
            let mut destination = None;
            assert!(
                owner
                    .handoff_start_result(&original, &original, &mut destination)
                    .unwrap()
            );
            match destination.as_ref().unwrap() {
                Ok(info) => assert!(std::ptr::eq(info.session_id.as_ptr(), body_pointer)),
                Err(DispatchError::Driver(DriverError::Host(
                    crate::host::HostRefusal::Ownership(body),
                ))) => assert!(std::ptr::eq(body.as_ptr(), body_pointer)),
                _ => panic!("original synthetic body transfer changed"),
            }
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Actual retained cleanup of a driver with NO
    // physical owners, then actual restoration wake. No native work/joins exist.
    #[test]
    #[cfg(panic = "unwind")]
    fn canonical_empty_cleanup_return_precedes_restoration_wake_and_unwind() {
        use super::super::archive_work::{DispatchError, OriginalClaudeOwner};
        use std::{
            future::Future,
            task::{Context, Poll, Wake, Waker},
        };
        struct Opaque(std::cell::Cell<usize>);
        struct WakeState {
            count: std::sync::atomic::AtomicUsize,
            payload: std::sync::Mutex<Option<Box<Opaque>>>,
        }
        impl Wake for WakeState {
            fn wake(self: Arc<Self>) {
                Self::wake_by_ref(&self);
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let payload = self.payload.lock().unwrap().take();
                if let Some(payload) = payload {
                    std::panic::resume_unwind(payload);
                }
            }
        }
        for unwind in [false, true] {
            let original = archive_startup_operation();
            let driver = Box::new(ClaudeDriver::new(
                argument_config(),
                TranscriptSink::closed_for_source_control(),
            ));
            let pointer = std::ptr::from_ref(driver.as_ref());
            let context = recovery_context(driver.cfg.seat, original.clone());
            let mut owner = OriginalClaudeOwner::new(driver);
            owner.bind_original_context(context).unwrap();
            owner.begin_start_for_source_control(&original).unwrap();
            let earlier = String::from("synthetic dispatch refusal retained through cleanup");
            let earlier_pointer = earlier.as_ptr();
            owner
                .retain_start_for_source_control(
                    &original,
                    Err(DispatchError::Driver(DriverError::Host(
                        crate::host::HostRefusal::Ownership(earlier),
                    ))),
                )
                .unwrap();
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            let payload = Box::new(Opaque(std::cell::Cell::new(89)));
            let payload_pointer = std::ptr::from_ref(payload.as_ref());
            let wake = Arc::new(WakeState {
                count: std::sync::atomic::AtomicUsize::new(0),
                payload: std::sync::Mutex::new(if unwind { Some(payload) } else { None }),
            });
            let waker = Waker::from(wake.clone());
            let mut wake_context = Context::from_waker(&waker);
            let mut waiting = std::pin::pin!(
                owner
                    .restoration_wait_for_source_control(&original)
                    .unwrap()
            );
            assert!(waiting.as_mut().poll(&mut wake_context).is_pending());
            let mut context = Context::from_waker(Waker::noop());
            let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // Actual cleanup family is polled ONCE. Empty helpers/proc
                // means no task/native operation, deadline wait or archive IO.
                let mut cleanup = std::pin::pin!(owner.observe_cleanup(&original));
                cleanup.as_mut().poll(&mut context)
            }));
            if unwind {
                let opaque = observed.err().unwrap().downcast::<Opaque>().ok().unwrap();
                assert!(std::ptr::eq(opaque.as_ref(), payload_pointer));
                assert_eq!(opaque.0.get(), 89);
            } else {
                assert!(matches!(observed, Ok(Poll::Ready(Ok(true)))));
            }
            assert!(wake.count.load(std::sync::atomic::Ordering::SeqCst) > 0);
            let results = owner.retained_results(&original).unwrap();
            assert!(results.cleanup_started && matches!(results.cleanup, Some(Ok(()))));
            assert_eq!(results.cleanup_returned_ok, Some(true));
            assert_eq!(results.start_returned_ok, Some(false));
            let cleanup_pointer = std::ptr::from_ref(results.cleanup.unwrap());
            match results.start.unwrap() {
                Err(DispatchError::Driver(DriverError::Host(
                    crate::host::HostRefusal::Ownership(body),
                ))) => assert!(std::ptr::eq(body.as_ptr(), earlier_pointer)),
                _ => panic!("cleanup changed the earlier typed dispatch cause"),
            }
            {
                let mut repeat = std::pin::pin!(owner.observe_cleanup(&original));
                assert!(matches!(
                    repeat.as_mut().poll(&mut context),
                    Poll::Ready(Err(_))
                ));
            }
            assert!(std::ptr::eq(
                owner.retained_results(&original).unwrap().cleanup.unwrap(),
                cleanup_pointer
            ));
            {
                let mut preparation = owner
                    .prepared_original(&original)
                    .unwrap()
                    .borrow(&original)
                    .unwrap();
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let mut foreground = supervision
                    .root_mut()
                    .unwrap()
                    .borrow_foreground(&original)
                    .unwrap();
                let driver = foreground.driver_mut().unwrap();
                assert!(std::ptr::eq(driver, pointer));
                assert!(driver.proc.is_none() && driver.helpers.try_lock().unwrap().is_empty());
            }
            let mut cleanup_destination = None;
            assert!(
                owner
                    .handoff_cleanup_result(&original, &original, &mut cleanup_destination)
                    .unwrap()
            );
            assert!(matches!(cleanup_destination, Some(Ok(()))));
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Empty concrete owner; valid native dispatch
    // is NEVER polled. Injected returns/capture are synthetic ownership controls.
    #[test]
    #[cfg(panic = "unwind")]
    fn original_controller_keeps_dispatch_marker_results_and_separate_cleanup() {
        use super::super::archive_work::{DispatchError, OriginalClaudeOwner};
        use crate::failed_evidence::ArchiveError;
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        fn send<F: Future + Send>(_: &F) {}
        for case in 0..4 {
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let mut driver = Box::new(ClaudeDriver::new(argument_config(), sink));
            if case == 3 {
                driver.parser.last_text = Some("synthetic refused binding evidence".into());
            }
            let pointer = std::ptr::from_ref(driver.as_ref());
            let context = recovery_context(driver.cfg.seat, original.clone());
            let mut owner = OriginalClaudeOwner::new(driver);
            let bound = owner.bind_original_context(context);
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            if case == 3 {
                assert!(matches!(bound, Err(ArchiveError::Identity)));
                let mut cx = Context::from_waker(Waker::noop());
                {
                    let mut cleanup = std::pin::pin!(owner.observe_cleanup(&original));
                    assert!(matches!(
                        cleanup.as_mut().poll(&mut cx),
                        Poll::Ready(Ok(false))
                    ));
                }
                assert!(matches!(
                    owner.first_refusal(&original).unwrap(),
                    Some(DispatchError::Driver(DriverError::Host(
                        crate::host::HostRefusal::Ownership(_)
                    )))
                ));
                let results = owner.retained_results(&original).unwrap();
                assert!(results.cleanup_started && matches!(results.cleanup, Some(Err(_))));
                assert_eq!(results.cleanup_returned_ok, Some(false));
                assert!(
                    !results.start_started && results.start.is_none() && results.turn.is_none()
                );
                assert_eq!(results.start_returned_ok, None);
                assert!(owner.begin_start_for_source_control(&original).is_err());
                let mut start_destination = None;
                assert!(matches!(
                    owner.handoff_start_result(&original, &original, &mut start_destination),
                    Err(ArchiveError::Identity)
                ));
                assert!(start_destination.is_none());
                let mut capture = None;
                assert!(matches!(
                    owner.handoff_captured_before_archive(&original, &original, &mut capture),
                    Err(ArchiveError::Identity)
                )); // Pending host context cannot masquerade as a bound physical source.
                assert!(capture.is_none());
                let (driver, pending) = owner.unprepared_for_source_control(&original).unwrap();
                assert_eq!(std::ptr::from_ref(driver), pointer);
                assert!(pending.unwrap().operation.same_instance(&original));
                assert!(driver.proc.is_none() && driver.helpers.try_lock().unwrap().is_empty());
                assert_eq!(original.captured_bytes(), charged);
                assert_eq!(original.allowance().deadline(), deadline);
                continue; // Typed bind cause and SAME direct Box never promoted.
            }
            assert!(bound.is_ok());
            // Actual outer future families are constructed/dropped unpolled.
            {
                let future = owner.observe_start(&original, None);
                send(&future);
            }
            {
                let future = owner.observe_turn(&original, "unpolled", Duration::ZERO);
                send(&future);
            }
            {
                let future = owner.observe_cleanup(&original);
                send(&future);
            }
            assert!(matches!(
                owner.check_controller_frames(&original),
                Ok(()) | Err(ArchiveError::Fit)
            ));
            let mut cx = Context::from_waker(Waker::noop());
            {
                let mut foreign_start = std::pin::pin!(owner.observe_start(&foreign, None));
                assert!(matches!(
                    foreign_start.as_mut().poll(&mut cx),
                    Poll::Ready(Err(ArchiveError::Identity))
                ));
            }
            assert!(!owner.retained_results(&original).unwrap().start_started);
            if case == 0 {
                original.overflow(); // Refuse BEFORE any actual native body.
                {
                    let mut start = std::pin::pin!(owner.observe_start(&original, None));
                    assert!(matches!(
                        start.as_mut().poll(&mut cx),
                        Poll::Ready(Ok(false))
                    ));
                }
                let results = owner.retained_results(&original).unwrap();
                assert!(results.start_started);
                assert!(matches!(
                    results.start,
                    Some(Err(DispatchError::Archive(_)))
                ));
                assert!(results.turn.is_none() && !results.turn_started);
                assert_eq!(results.start_returned_ok, Some(false));
                let mut start_destination = None;
                assert!(
                    owner
                        .handoff_start_result(&original, &original, &mut start_destination)
                        .unwrap()
                );
                assert!(matches!(
                    start_destination,
                    Some(Err(DispatchError::Archive(_)))
                ));
                let results = owner.retained_results(&original).unwrap();
                assert!(results.start.is_none() && results.start_started);
                assert_eq!(results.start_returned_ok, Some(false));
                assert!(owner.begin_turn_for_source_control(&original).is_err());
                assert!(
                    !owner
                        .handoff_start_result(&original, &original, &mut None)
                        .unwrap()
                );
            } else if case == 1 {
                // Same begin helper as actual dispatch, followed by synthetic
                // suspension. Dropping it must never reopen the original start.
                {
                    let pending = async {
                        owner.begin_start_for_source_control(&original).unwrap();
                        std::future::pending::<()>().await;
                    };
                    let mut pending = std::pin::pin!(pending);
                    assert!(pending.as_mut().poll(&mut cx).is_pending());
                }
                let results = owner.retained_results(&original).unwrap();
                assert!(results.start_started && results.start.is_none());
                assert!(!results.turn_started && results.turn.is_none());
                assert_eq!(results.start_returned_ok, None);
                let mut start_destination = None;
                assert!(
                    !owner
                        .handoff_start_result(&original, &original, &mut start_destination)
                        .unwrap()
                );
                assert!(start_destination.is_none());
                assert!(owner.begin_turn_for_source_control(&original).is_err());
            } else {
                owner.begin_start_for_source_control(&original).unwrap();
                let info = SessionInfo {
                    session_id: "synthetic controller result".into(),
                    model: Some("synthetic model".into()),
                    cli_version: Some("synthetic version".into()),
                    resumed: false,
                };
                let info_pointer = info.session_id.as_ptr();
                let model_pointer = info.model.as_ref().unwrap().as_ptr();
                let version_pointer = info.cli_version.as_ref().unwrap().as_ptr();
                assert!(
                    owner
                        .retain_start_for_source_control(&original, Ok(info))
                        .unwrap()
                );
                // Install separate synthetic captured evidence in SAME driver.
                let captured = TurnOutcome {
                    ok: true,
                    error: None,
                    text: Some("separate captured body".into()),
                    usage: Default::default(),
                    quota: vec![],
                };
                let captured_pointer = captured.text.as_ref().unwrap().as_ptr();
                {
                    let prepared = owner.prepared_original(&original).unwrap();
                    let mut preparation = prepared.borrow(&original).unwrap();
                    let mut supervision =
                        preparation.supervisor().unwrap().borrow(&original).unwrap();
                    let mut foreground = supervision
                        .root_mut()
                        .unwrap()
                        .borrow_foreground(&original)
                        .unwrap();
                    foreground.driver_mut().unwrap().completed_turn = Some(captured);
                }
                let mut start_destination = None;
                assert!(
                    owner
                        .handoff_start_result(&foreign, &original, &mut start_destination)
                        .is_err()
                );
                assert!(
                    owner
                        .handoff_start_result(&original, &foreign, &mut start_destination)
                        .is_err()
                );
                assert!(start_destination.is_none());
                let mut occupied_start = Some(Err(DispatchError::Driver(DriverError::Timeout)));
                assert!(
                    owner
                        .handoff_start_result(&original, &original, &mut occupied_start)
                        .is_err()
                );
                assert!(matches!(
                    occupied_start,
                    Some(Err(DispatchError::Driver(DriverError::Timeout)))
                ));
                assert_eq!(
                    owner
                        .retained_results(&original)
                        .unwrap()
                        .start
                        .unwrap()
                        .as_ref()
                        .ok()
                        .unwrap()
                        .session_id
                        .as_ptr(),
                    info_pointer
                ); // All refusals kept the original body in its original slot.
                assert!(
                    owner
                        .handoff_start_result(&original, &original, &mut start_destination)
                        .unwrap()
                );
                assert!(
                    !owner
                        .handoff_start_result(&original, &original, &mut None)
                        .unwrap()
                );
                let results = owner.retained_results(&original).unwrap();
                assert!(results.start.is_none() && results.start_started);
                assert_eq!(results.start_returned_ok, Some(true));
                assert!(owner.begin_start_for_source_control(&original).is_err());
                assert!(
                    owner
                        .retain_start_for_source_control(
                            &original,
                            Err(DispatchError::Driver(DriverError::Timeout))
                        )
                        .is_err()
                ); // A transferred result cannot be replaced or reopened.
                // The original successful disposition survives body MOVE;
                // this synthetic begin helper polls no actual native dispatch.
                owner.begin_turn_for_source_control(&original).unwrap();
                let body = String::from("first controller error body");
                let body_pointer = body.as_ptr();
                assert!(
                    !owner
                        .retain_turn_for_source_control(
                            &original,
                            Err(DispatchError::Driver(DriverError::Cli(body)))
                        )
                        .unwrap()
                );
                // Unwind AFTER result installation preserves it. This catcher
                // is a control, not production outer panic-payload custody.
                struct Opaque(std::cell::Cell<usize>);
                let payload = Box::new(Opaque(std::cell::Cell::new(91)));
                let payload_pointer = std::ptr::from_ref(payload.as_ref());
                let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    std::panic::resume_unwind(payload)
                }));
                let payload = unwind.unwrap_err().downcast::<Opaque>().ok().unwrap();
                assert_eq!(std::ptr::from_ref(payload.as_ref()), payload_pointer);
                assert_eq!(payload.0.get(), 91);
                let results = owner.retained_results(&original).unwrap();
                assert_eq!(
                    start_destination
                        .as_ref()
                        .unwrap()
                        .as_ref()
                        .ok()
                        .unwrap()
                        .session_id
                        .as_ptr(),
                    info_pointer
                );
                let transferred = start_destination.as_ref().unwrap().as_ref().ok().unwrap();
                assert_eq!(transferred.model.as_ref().unwrap().as_ptr(), model_pointer);
                assert_eq!(
                    transferred.cli_version.as_ref().unwrap().as_ptr(),
                    version_pointer
                );
                assert_eq!(results.start_returned_ok, Some(true));
                assert!(results.start.is_none());
                match results.turn.unwrap() {
                    Err(DispatchError::Driver(DriverError::Cli(body))) => {
                        assert_eq!(body.as_ptr(), body_pointer)
                    }
                    _ => panic!("first controller body lost"),
                }
                assert!(results.cleanup.is_none() && !results.cleanup_started);
                // Future EMPTY physical pass or honest frame refusal; no child
                // or archive thread, and never a join/custody/healthy witness.
                {
                    let mut cleanup = std::pin::pin!(owner.observe_cleanup(&original));
                    assert!(matches!(cleanup.as_mut().poll(&mut cx), Poll::Ready(Ok(_))));
                }
                let results = owner.retained_results(&original).unwrap();
                assert!(results.cleanup_started && results.cleanup.is_some());
                assert_eq!(
                    results.cleanup_returned_ok,
                    results.cleanup.map(|result| result.is_ok()),
                );
                assert!(
                    matches!(results.turn, Some(Err(DispatchError::Driver(DriverError::Cli(body)))) if body.as_ptr() == body_pointer)
                );
                let mut destination = None;
                assert!(
                    owner
                        .handoff_turn_result(&original, &foreign, &mut destination)
                        .is_err()
                );
                assert!(destination.is_none());
                let mut occupied = Some(Err(DispatchError::Driver(DriverError::Timeout)));
                assert!(
                    owner
                        .handoff_turn_result(&original, &original, &mut occupied)
                        .is_err()
                );
                assert!(
                    owner
                        .handoff_turn_result(&original, &original, &mut destination)
                        .unwrap()
                );
                assert!(
                    matches!(destination, Some(Err(DispatchError::Driver(DriverError::Cli(ref body)))) if body.as_ptr() == body_pointer)
                );
                assert!(
                    !owner
                        .handoff_turn_result(&original, &original, &mut None)
                        .unwrap()
                );
                assert!(owner.begin_turn_for_source_control(&original).is_err());
                let prepared = owner.prepared_original(&original).unwrap();
                let mut preparation = prepared.borrow(&original).unwrap();
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let foreground = supervision
                    .root_mut()
                    .unwrap()
                    .borrow_foreground(&original)
                    .unwrap();
                assert_eq!(
                    foreground
                        .driver()
                        .unwrap()
                        .completed_turn
                        .as_ref()
                        .unwrap()
                        .text
                        .as_ref()
                        .unwrap()
                        .as_ptr(),
                    captured_pointer
                );
            }
            assert!(owner.begin_start_for_source_control(&original).is_err());
            assert!(owner.retained_results(&foreign).is_err());
            // SAME Box retained through any refused/suspended result route.
            let prepared = owner.prepared_original(&original).unwrap();
            let mut preparation = prepared.borrow(&original).unwrap();
            let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
            let foreground = supervision
                .root_mut()
                .unwrap()
                .borrow_foreground(&original)
                .unwrap();
            let driver = foreground.driver().unwrap();
            assert_eq!(std::ptr::from_ref(driver), pointer);
            assert!(driver.proc.is_none() && driver.helpers.try_lock().unwrap().is_empty());
            assert!(driver.prompt_lease.is_none() && driver.startup.is_none());
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. The actual private erasure entry runs its
    // synchronous checks, but accepted futures are NEVER polled. No native body.
    #[test]
    fn original_erasure_entry_retains_refusal_or_unpolled_attempt() {
        use super::super::archive_work::{CleanupError, DispatchError, OriginalClaudeOwner};
        use crate::failed_evidence::ArchiveError;
        for mode in 0..3 {
            for forced_fit in [false, true] {
                let original = archive_startup_operation();
                let foreign = archive_startup_operation();
                let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
                let driver = Box::new(ClaudeDriver::new(argument_config(), sink));
                let pointer = std::ptr::from_ref(driver.as_ref());
                let context = recovery_context(driver.cfg.seat, original.clone());
                let mut owner = OriginalClaudeOwner::new(driver);
                assert!(owner.bind_original_context(context).is_ok());
                if mode != 0 {
                    owner.begin_start_for_source_control(&original).unwrap();
                    owner
                        .retain_start_for_source_control(
                            &original,
                            Ok(SessionInfo {
                                session_id: "synthetic erasure control".into(),
                                model: None,
                                cli_version: None,
                                resumed: false,
                            }),
                        )
                        .unwrap();
                }
                if mode == 2 {
                    owner.begin_turn_for_source_control(&original).unwrap();
                    owner
                        .retain_turn_for_source_control(
                            &original,
                            Err(DispatchError::Driver(DriverError::Timeout)),
                        )
                        .unwrap();
                }
                let charged = original.captured_bytes();
                let deadline = original.allowance().deadline();
                let foreign_entry = match mode {
                    0 => owner.start_future(&foreign, None),
                    1 => owner.turn_future(&foreign, "unpolled", Duration::ZERO),
                    2 => owner.cleanup_future(&foreign),
                    _ => unreachable!(),
                };
                assert!(matches!(foreign_entry, Err(ArchiveError::Identity)));
                drop(foreign_entry);
                let extra = if forced_fit { usize::MAX } else { 0 };
                let entry = match mode {
                    0 => owner.start_future_for_source_control(&original, extra),
                    1 => owner.turn_future_for_source_control(&original, extra),
                    2 => owner.cleanup_future_for_source_control(&original, extra),
                    _ => unreachable!(),
                };
                let refused = match entry {
                    Ok(future) => {
                        assert!(!forced_fit);
                        drop(future); // BEFORE its first poll; marker stays incomplete.
                        false
                    }
                    Err(ArchiveError::Identity) => true,
                    Err(_) => panic!("private entry lost its canonical fit refusal"),
                };
                if forced_fit {
                    assert!(refused);
                }
                let results = owner.retained_results(&original).unwrap();
                match mode {
                    0 => {
                        assert!(results.start_started);
                        assert_eq!(results.start_returned_ok, refused.then_some(false));
                        match (refused, results.start) {
                            (true, Some(Err(DispatchError::Archive(ArchiveError::Fit)))) => {}
                            (false, None) => {}
                            _ => panic!("start attempt or original fit refusal lost"),
                        }
                        assert!(!results.turn_started && !results.cleanup_started);
                    }
                    1 => {
                        assert!(results.turn_started && !results.cleanup_started);
                        match (refused, results.turn) {
                            (true, Some(Err(DispatchError::Archive(ArchiveError::Fit)))) => {}
                            (false, None) => {}
                            _ => panic!("turn attempt or original fit refusal lost"),
                        }
                        assert_eq!(results.start_returned_ok, Some(true));
                    }
                    2 => {
                        assert!(results.cleanup_started);
                        assert_eq!(results.cleanup_returned_ok, refused.then_some(false));
                        match (refused, results.cleanup) {
                            (true, Some(Err(CleanupError::Archive(ArchiveError::Fit)))) => {}
                            (false, None) => {}
                            _ => panic!("cleanup attempt or original fit refusal lost"),
                        }
                        assert!(matches!(
                            results.turn,
                            Some(Err(DispatchError::Driver(DriverError::Timeout)))
                        )); // Cleanup disposition never replaces the first turn cause.
                    }
                    _ => unreachable!(),
                }
                let repeated = match mode {
                    0 => owner.start_future(&original, None),
                    1 => owner.turn_future(&original, "no retry", Duration::ZERO),
                    2 => owner.cleanup_future(&original),
                    _ => unreachable!(),
                };
                assert!(matches!(repeated, Err(ArchiveError::Identity)));
                drop(repeated);
                let prepared = owner.prepared_original(&original).unwrap();
                let mut preparation = prepared.borrow(&original).unwrap();
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let foreground = supervision
                    .root_mut()
                    .unwrap()
                    .borrow_foreground(&original)
                    .unwrap();
                let driver = foreground.driver().unwrap();
                assert_eq!(std::ptr::from_ref(driver), pointer);
                assert!(driver.proc.is_none() && driver.helpers.try_lock().unwrap().is_empty());
                assert!(driver.prompt_lease.is_none() && driver.startup.is_none());
                assert_eq!(original.captured_bytes(), charged);
                assert_eq!(original.allowance().deadline(), deadline);
                // Either honest fit refusal or an unpolled private frame; this
                // test never proves allocator/whole fit, native start or joins.
            }
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. The bridge is never polled: synthetic
    // completion checks ownership only, not its native await/physical joins.
    #[test]
    fn combined_caller_storage_refuses_before_erasure_and_retains_original_cause() {
        use super::super::archive_work::{CleanupError, DispatchError, OriginalClaudeOwner};
        use crate::failed_evidence::{ArchiveError, FIXED_ARCHIVE_WORKSPACE};
        // Finite over-cap combined storage and checked-add overflow. These
        // are refusal controls, not positive fit or actual allocation proofs.
        for caller_bytes in [FIXED_ARCHIVE_WORKSPACE, usize::MAX] {
            for mode in 0..3 {
                let original = archive_startup_operation();
                let foreign = archive_startup_operation();
                let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
                let driver = Box::new(ClaudeDriver::new(argument_config(), sink));
                let pointer = std::ptr::from_ref(driver.as_ref());
                let context = recovery_context(driver.cfg.seat, original.clone());
                let mut owner = OriginalClaudeOwner::new(driver);
                owner.bind_original_context(context).unwrap();
                if mode != 0 {
                    owner.begin_start_for_source_control(&original).unwrap();
                    owner
                        .retain_start_for_source_control(
                            &original,
                            Ok(SessionInfo {
                                session_id: "synthetic combined fit prerequisite".into(),
                                model: None,
                                cli_version: None,
                                resumed: false,
                            }),
                        )
                        .unwrap();
                }
                if mode == 2 {
                    owner.begin_turn_for_source_control(&original).unwrap();
                    owner
                        .retain_turn_for_source_control(
                            &original,
                            Err(DispatchError::Driver(DriverError::Timeout)),
                        )
                        .unwrap();
                }
                let charged = original.captured_bytes();
                let deadline = original.allowance().deadline();
                let mut start = None;
                let mut turn = None;
                let mut cleanup = None;
                // Foreign destination refuses BEFORE consuming the attempt,
                // even when caller storage itself cannot fit.
                for destination in [&foreign, &original] {
                    let refused = match mode {
                        0 => owner.start_to_slot_with_caller_storage(
                            &original,
                            None,
                            destination,
                            &mut start,
                            caller_bytes,
                        ),
                        1 => owner.turn_to_slot_with_caller_storage(
                            &original,
                            "never polled",
                            Duration::ZERO,
                            destination,
                            &mut turn,
                            caller_bytes,
                        ),
                        2 => owner.cleanup_to_slot_with_caller_storage(
                            &original,
                            destination,
                            &mut cleanup,
                            caller_bytes,
                        ),
                        _ => unreachable!(),
                    };
                    assert!(matches!(refused, Err(ArchiveError::Identity)));
                    drop(refused); // No erased accepted future or first poll.
                    assert!(start.is_none() && turn.is_none() && cleanup.is_none());
                    let results = owner.retained_results(&original).unwrap();
                    let same = original.same_instance(destination);
                    match mode {
                        0 => {
                            assert_eq!(results.start_started, same);
                            if same {
                                assert!(matches!(
                                    results.start,
                                    Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                                ));
                                assert_eq!(results.start_returned_ok, Some(false));
                            }
                        }
                        1 => {
                            assert_eq!(results.turn_started, same);
                            if same {
                                assert!(matches!(
                                    results.turn,
                                    Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                                ));
                            }
                        }
                        2 => {
                            assert_eq!(results.cleanup_started, same);
                            if same {
                                assert!(matches!(
                                    results.cleanup,
                                    Some(Err(CleanupError::Archive(ArchiveError::Fit)))
                                ));
                                assert_eq!(results.cleanup_returned_ok, Some(false));
                            }
                            assert!(matches!(
                                results.turn,
                                Some(Err(DispatchError::Driver(DriverError::Timeout)))
                            ));
                        }
                        _ => unreachable!(),
                    }
                }
                // A later smaller caller does not renew the refused attempt.
                let retry = match mode {
                    0 => owner.start_to_slot_future(&original, None, &original, &mut start),
                    1 => owner.turn_to_slot_future(
                        &original,
                        "no retry",
                        Duration::ZERO,
                        &original,
                        &mut turn,
                    ),
                    2 => owner.cleanup_to_slot_future(&original, &original, &mut cleanup),
                    _ => unreachable!(),
                };
                assert!(matches!(retry, Err(ArchiveError::Identity)));
                drop(retry);
                assert!(start.is_none() && turn.is_none() && cleanup.is_none());
                let retained = owner.retained_results(&original).unwrap();
                match mode {
                    0 => assert!(matches!(
                        retained.start,
                        Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                    )),
                    1 => assert!(matches!(
                        retained.turn,
                        Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                    )),
                    2 => {
                        assert!(matches!(
                            retained.cleanup,
                            Some(Err(CleanupError::Archive(ArchiveError::Fit)))
                        ));
                        assert!(matches!(
                            retained.turn,
                            Some(Err(DispatchError::Driver(DriverError::Timeout)))
                        ));
                    }
                    _ => unreachable!(),
                }
                let prepared = owner.prepared_original(&original).unwrap();
                let mut preparation = prepared.borrow(&original).unwrap();
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let foreground = supervision
                    .root_mut()
                    .unwrap()
                    .borrow_foreground(&original)
                    .unwrap();
                let driver = foreground.driver().unwrap();
                assert_eq!(std::ptr::from_ref(driver), pointer);
                assert!(driver.proc.is_none() && driver.helpers.try_lock().unwrap().is_empty());
                assert!(driver.prompt_lease.is_none() && driver.startup.is_none());
                assert_eq!(original.captured_bytes(), charged);
                assert_eq!(original.allowance().deadline(), deadline);
            }
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Synthetic first returns exercise typed
    // canonical MOVE/refusal only; no native/trait-facade/positive-fit witness.
    #[test]
    fn typed_driver_destinations_keep_first_bodies_and_archive_causes() {
        use super::super::archive_work::{CleanupError, DispatchError, OriginalClaudeOwner};
        use crate::failed_evidence::ArchiveError;
        fn error_pointer<T>(slot: &Option<Result<T, DriverError>>) -> Option<*const u8> {
            match slot {
                Some(Err(DriverError::Protocol(body))) => Some(body.as_ptr()),
                _ => None,
            }
        }
        for mode in 0..3 {
            for outcome_kind in 0..3 {
                let original = archive_startup_operation();
                let foreign = archive_startup_operation();
                let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
                let driver = Box::new(ClaudeDriver::new(argument_config(), sink));
                let driver_pointer = std::ptr::from_ref(driver.as_ref());
                let context = recovery_context(driver.cfg.seat, original.clone());
                let mut owner = OriginalClaudeOwner::new(driver);
                owner.bind_original_context(context).unwrap();
                owner.begin_start_for_source_control(&original).unwrap();
                if mode != 0 {
                    owner
                        .retain_start_for_source_control(
                            &original,
                            Ok(SessionInfo {
                                session_id: "synthetic typed prerequisite".into(),
                                model: None,
                                cli_version: None,
                                resumed: false,
                            }),
                        )
                        .unwrap();
                    if mode == 1 {
                        owner.begin_turn_for_source_control(&original).unwrap();
                    } else {
                        owner.begin_turn_for_source_control(&original).unwrap();
                        owner
                            .retain_turn_for_source_control(
                                &original,
                                Err(DispatchError::Driver(DriverError::Timeout)),
                            )
                            .unwrap();
                        owner.begin_cleanup_for_source_control(&original).unwrap();
                    }
                }
                let body = String::from("synthetic first typed return");
                let body_pointer = body.as_ptr();
                match mode {
                    0 => {
                        owner
                            .retain_start_for_source_control(
                                &original,
                                match outcome_kind {
                                    0 => Ok(SessionInfo {
                                        session_id: body,
                                        model: None,
                                        cli_version: None,
                                        resumed: false,
                                    }),
                                    1 => Err(DispatchError::Driver(DriverError::Protocol(body))),
                                    2 => Err(DispatchError::Archive(ArchiveError::Fit)),
                                    _ => unreachable!(),
                                },
                            )
                            .unwrap();
                    }
                    1 => {
                        owner
                            .retain_turn_for_source_control(
                                &original,
                                match outcome_kind {
                                    0 => Ok(TurnOutcome {
                                        ok: false,
                                        error: None,
                                        text: Some(body),
                                        usage: Usage::default(),
                                        quota: Vec::new(),
                                    }),
                                    1 => Err(DispatchError::Driver(DriverError::Protocol(body))),
                                    2 => Err(DispatchError::Archive(ArchiveError::Fit)),
                                    _ => unreachable!(),
                                },
                            )
                            .unwrap();
                    }
                    2 => {
                        owner
                            .retain_cleanup_for_source_control(
                                &original,
                                match outcome_kind {
                                    0 => Ok(()),
                                    1 => Err(CleanupError::Driver(DriverError::Protocol(body))),
                                    2 => Err(CleanupError::Archive(ArchiveError::Fit)),
                                    _ => unreachable!(),
                                },
                            )
                            .unwrap();
                    }
                    _ => unreachable!(),
                }
                let charged = original.captured_bytes();
                let deadline = original.allowance().deadline();
                let mut start = None;
                let mut turn = None;
                let mut cleanup = None;
                // Foreign and occupied destinations preserve the first root.
                let mut occupied_start =
                    Some(Err(DriverError::Protocol("occupied startup".into())));
                let mut occupied_turn = Some(Err(DriverError::Protocol("occupied turn".into())));
                let mut occupied_cleanup =
                    Some(Err(DriverError::Protocol("occupied cleanup".into())));
                let occupied_pointers = (
                    error_pointer(&occupied_start),
                    error_pointer(&occupied_turn),
                    error_pointer(&occupied_cleanup),
                );
                for full in [false, true] {
                    let destination = if full { &original } else { &foreign };
                    let refused = match mode {
                        0 => owner.handoff_start_driver_result(
                            &original,
                            destination,
                            if full {
                                &mut occupied_start
                            } else {
                                &mut start
                            },
                        ),
                        1 => owner.handoff_turn_driver_result(
                            &original,
                            destination,
                            if full { &mut occupied_turn } else { &mut turn },
                        ),
                        2 => owner.handoff_cleanup_driver_result(
                            &original,
                            destination,
                            if full {
                                &mut occupied_cleanup
                            } else {
                                &mut cleanup
                            },
                        ),
                        _ => unreachable!(),
                    };
                    assert!(matches!(refused, Err(ArchiveError::Identity)));
                }
                for (before, after) in [
                    (occupied_pointers.0, error_pointer(&occupied_start)),
                    (occupied_pointers.1, error_pointer(&occupied_turn)),
                    (occupied_pointers.2, error_pointer(&occupied_cleanup)),
                ] {
                    assert!(std::ptr::eq(before.unwrap(), after.unwrap()));
                }
                let transfer = match mode {
                    0 => owner.handoff_start_driver_result(&original, &original, &mut start),
                    1 => owner.handoff_turn_driver_result(&original, &original, &mut turn),
                    2 => owner.handoff_cleanup_driver_result(&original, &original, &mut cleanup),
                    _ => unreachable!(),
                };
                if outcome_kind == 2 {
                    assert!(matches!(transfer, Err(ArchiveError::Identity)));
                    assert!(start.is_none() && turn.is_none() && cleanup.is_none());
                } else {
                    assert!(transfer.unwrap());
                    let pointer = if outcome_kind == 1 {
                        match mode {
                            0 => error_pointer(&start),
                            1 => error_pointer(&turn),
                            2 => error_pointer(&cleanup),
                            _ => unreachable!(),
                        }
                    } else {
                        match mode {
                            0 => start
                                .as_ref()
                                .unwrap()
                                .as_ref()
                                .ok()
                                .map(|info| info.session_id.as_ptr()),
                            1 => turn
                                .as_ref()
                                .unwrap()
                                .as_ref()
                                .ok()
                                .map(|outcome| outcome.text.as_ref().unwrap().as_ptr()),
                            2 => {
                                assert!(matches!(cleanup, Some(Ok(()))));
                                None
                            }
                            _ => unreachable!(),
                        }
                    };
                    if let Some(pointer) = pointer {
                        assert!(std::ptr::eq(pointer, body_pointer));
                    }
                }
                let repeat = match mode {
                    0 => owner.handoff_start_driver_result(&original, &original, &mut start),
                    1 => owner.handoff_turn_driver_result(&original, &original, &mut turn),
                    2 => owner.handoff_cleanup_driver_result(&original, &original, &mut cleanup),
                    _ => unreachable!(),
                };
                assert!(matches!(repeat, Err(ArchiveError::Identity)));
                if outcome_kind == 1 {
                    let after = match mode {
                        0 => error_pointer(&start),
                        1 => error_pointer(&turn),
                        2 => error_pointer(&cleanup),
                        _ => unreachable!(),
                    };
                    assert!(std::ptr::eq(after.unwrap(), body_pointer));
                } else if outcome_kind == 0 {
                    match mode {
                        0 => assert!(std::ptr::eq(
                            start
                                .as_ref()
                                .unwrap()
                                .as_ref()
                                .ok()
                                .unwrap()
                                .session_id
                                .as_ptr(),
                            body_pointer
                        )),
                        1 => assert!(std::ptr::eq(
                            turn.as_ref()
                                .unwrap()
                                .as_ref()
                                .ok()
                                .unwrap()
                                .text
                                .as_ref()
                                .unwrap()
                                .as_ptr(),
                            body_pointer
                        )),
                        2 => assert!(matches!(cleanup, Some(Ok(())))),
                        _ => unreachable!(),
                    }
                }
                let retained = owner.retained_results(&original).unwrap();
                match mode {
                    0 => {
                        assert!(retained.start_started);
                        assert_eq!(retained.start_returned_ok, Some(outcome_kind == 0));
                        if outcome_kind == 2 {
                            assert!(matches!(
                                retained.start,
                                Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                            ));
                        } else {
                            assert!(retained.start.is_none());
                        }
                    }
                    1 => {
                        assert!(retained.turn_started);
                        if outcome_kind == 2 {
                            assert!(matches!(
                                retained.turn,
                                Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                            ));
                        } else {
                            assert!(retained.turn.is_none());
                        }
                    }
                    2 => {
                        assert!(retained.cleanup_started);
                        assert_eq!(retained.cleanup_returned_ok, Some(outcome_kind == 0));
                        if outcome_kind == 2 {
                            assert!(matches!(
                                retained.cleanup,
                                Some(Err(CleanupError::Archive(ArchiveError::Fit)))
                            ));
                        } else {
                            assert!(retained.cleanup.is_none());
                        }
                        assert!(matches!(
                            retained.turn,
                            Some(Err(DispatchError::Driver(DriverError::Timeout)))
                        ));
                    }
                    _ => unreachable!(),
                }
                let prepared = owner.prepared_original(&original).unwrap();
                let mut preparation = prepared.borrow(&original).unwrap();
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let foreground = supervision
                    .root_mut()
                    .unwrap()
                    .borrow_foreground(&original)
                    .unwrap();
                assert!(std::ptr::eq(foreground.driver().unwrap(), driver_pointer));
                assert_eq!(original.captured_bytes(), charged);
                assert_eq!(original.allowance().deadline(), deadline);
            }
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Force checked caller-header overflow at
    // the actual typed bridge entry; NEVER accept/poll a native future.
    #[test]
    fn typed_driver_slot_bridge_retains_pre_erasure_fit_refusal() {
        use super::super::archive_work::{CleanupError, DispatchError, OriginalClaudeOwner};
        use crate::failed_evidence::ArchiveError;
        for mode in 0..3 {
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let driver = Box::new(ClaudeDriver::new(argument_config(), sink));
            let context = recovery_context(driver.cfg.seat, original.clone());
            let mut owner = OriginalClaudeOwner::new(driver);
            owner.bind_original_context(context).unwrap();
            if mode != 0 {
                owner.begin_start_for_source_control(&original).unwrap();
                owner
                    .retain_start_for_source_control(
                        &original,
                        Ok(SessionInfo {
                            session_id: "synthetic typed fit prerequisite".into(),
                            model: None,
                            cli_version: None,
                            resumed: false,
                        }),
                    )
                    .unwrap();
            }
            if mode == 2 {
                owner.begin_turn_for_source_control(&original).unwrap();
                owner
                    .retain_turn_for_source_control(
                        &original,
                        Err(DispatchError::Driver(DriverError::Timeout)),
                    )
                    .unwrap();
            }
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            let mut start = None;
            let mut turn = None;
            let mut cleanup = None;
            for destination in [&foreign, &original, &original] {
                let refused = match mode {
                    0 => owner.start_to_driver_slot_future(
                        &original,
                        None,
                        destination,
                        &mut start,
                        usize::MAX,
                    ),
                    1 => owner.turn_to_driver_slot_future(
                        &original,
                        "never polled",
                        Duration::ZERO,
                        destination,
                        &mut turn,
                        usize::MAX,
                    ),
                    2 => owner.cleanup_to_driver_slot_future(
                        &original,
                        destination,
                        &mut cleanup,
                        usize::MAX,
                    ),
                    _ => unreachable!(),
                };
                assert!(matches!(refused, Err(ArchiveError::Identity)));
                drop(refused);
                assert!(start.is_none() && turn.is_none() && cleanup.is_none());
                let retained = owner.retained_results(&original).unwrap();
                let same = original.same_instance(destination);
                match mode {
                    0 => {
                        assert_eq!(retained.start_started, same);
                        if same {
                            assert!(matches!(
                                retained.start,
                                Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                            ));
                        }
                    }
                    1 => {
                        assert_eq!(retained.turn_started, same);
                        if same {
                            assert!(matches!(
                                retained.turn,
                                Some(Err(DispatchError::Archive(ArchiveError::Fit)))
                            ));
                        }
                    }
                    2 => {
                        assert_eq!(retained.cleanup_started, same);
                        if same {
                            assert!(matches!(
                                retained.cleanup,
                                Some(Err(CleanupError::Archive(ArchiveError::Fit)))
                            ));
                        }
                        assert!(matches!(
                            retained.turn,
                            Some(Err(DispatchError::Driver(DriverError::Timeout)))
                        ));
                    }
                    _ => unreachable!(),
                }
            }
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. The bridge is never polled: synthetic
    // completion checks ownership only, not its native await/physical joins.
    #[test]
    fn original_slot_bridge_refuses_foreign_full_and_retains_unpolled_attempt() {
        use super::super::archive_work::{CleanupError, DispatchError, OriginalClaudeOwner};
        use crate::failed_evidence::ArchiveError;
        for mode in 0..3 {
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let driver = Box::new(ClaudeDriver::new(argument_config(), sink));
            let pointer = std::ptr::from_ref(driver.as_ref());
            let context = recovery_context(driver.cfg.seat, original.clone());
            let mut owner = OriginalClaudeOwner::new(driver);
            owner.bind_original_context(context).unwrap();
            if mode != 0 {
                owner.begin_start_for_source_control(&original).unwrap();
                owner
                    .retain_start_for_source_control(
                        &original,
                        Ok(SessionInfo {
                            session_id: "synthetic slot bridge".into(),
                            model: None,
                            cli_version: None,
                            resumed: false,
                        }),
                    )
                    .unwrap();
            }
            if mode == 2 {
                owner.begin_turn_for_source_control(&original).unwrap();
                owner
                    .retain_turn_for_source_control(
                        &original,
                        Err(DispatchError::Driver(DriverError::Timeout)),
                    )
                    .unwrap();
            }
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            let mut start = None;
            let mut turn = None;
            let mut cleanup = None;
            let mut occupied_start = Some(Err(DispatchError::Driver(DriverError::Timeout)));
            let mut occupied_turn = Some(Err(DispatchError::Driver(DriverError::Timeout)));
            let mut occupied_cleanup = Some(Err(CleanupError::Driver(DriverError::Timeout)));
            for full in [false, true] {
                let destination_original = if full { &original } else { &foreign };
                let refused = match mode {
                    0 => owner.start_to_slot_future(
                        &original,
                        None,
                        destination_original,
                        if full {
                            &mut occupied_start
                        } else {
                            &mut start
                        },
                    ),
                    1 => owner.turn_to_slot_future(
                        &original,
                        "unpolled",
                        Duration::ZERO,
                        destination_original,
                        if full { &mut occupied_turn } else { &mut turn },
                    ),
                    2 => owner.cleanup_to_slot_future(
                        &original,
                        destination_original,
                        if full {
                            &mut occupied_cleanup
                        } else {
                            &mut cleanup
                        },
                    ),
                    _ => unreachable!(),
                };
                assert!(matches!(refused, Err(ArchiveError::Identity)));
                drop(refused);
                let results = owner.retained_results(&original).unwrap();
                assert_eq!(results.start_started, mode != 0);
                assert_eq!(results.turn_started, mode == 2);
                assert!(!results.cleanup_started);
            }
            assert!(matches!(
                occupied_start,
                Some(Err(DispatchError::Driver(DriverError::Timeout)))
            ));
            assert!(matches!(
                occupied_turn,
                Some(Err(DispatchError::Driver(DriverError::Timeout)))
            ));
            assert!(matches!(
                occupied_cleanup,
                Some(Err(CleanupError::Driver(DriverError::Timeout)))
            ));
            let entry = match mode {
                0 => owner.start_to_slot_future(&original, None, &original, &mut start),
                1 => owner.turn_to_slot_future(
                    &original,
                    "unpolled",
                    Duration::ZERO,
                    &original,
                    &mut turn,
                ),
                2 => owner.cleanup_to_slot_future(&original, &original, &mut cleanup),
                _ => unreachable!(),
            };
            let refused = match entry {
                Ok(future) => {
                    drop(future);
                    false
                } // No first poll/native effect.
                Err(ArchiveError::Identity) => true,
                Err(_) => panic!("caller-slot bridge lost canonical refusal"),
            };
            assert!(start.is_none() && turn.is_none() && cleanup.is_none());
            let results = owner.retained_results(&original).unwrap();
            match mode {
                0 => {
                    assert!(results.start_started);
                    assert_eq!(results.start_returned_ok, refused.then_some(false));
                    assert_eq!(results.start.is_some(), refused);
                }
                1 => {
                    assert!(results.turn_started);
                    assert_eq!(results.turn.is_some(), refused);
                }
                2 => {
                    assert!(results.cleanup_started);
                    assert_eq!(results.cleanup_returned_ok, refused.then_some(false));
                    assert_eq!(results.cleanup.is_some(), refused);
                }
                _ => unreachable!(),
            }
            // Even BEFORE a synthetic completion is injected, dropping the
            // unpolled accepted bridge cannot permit a new dispatch/cleanup.
            let repeated = match mode {
                0 => owner.start_to_slot_future(&original, None, &original, &mut start),
                1 => owner.turn_to_slot_future(
                    &original,
                    "no retry",
                    Duration::ZERO,
                    &original,
                    &mut turn,
                ),
                2 => owner.cleanup_to_slot_future(&original, &original, &mut cleanup),
                _ => unreachable!(),
            };
            assert!(matches!(repeated, Err(ArchiveError::Identity)));
            drop(repeated);
            assert!(start.is_none() && turn.is_none() && cleanup.is_none());
            let body = String::from("synthetic original slot return");
            let body_pointer = body.as_ptr();
            if !refused {
                match mode {
                    0 => {
                        owner
                            .retain_start_for_source_control(
                                &original,
                                Err(DispatchError::Driver(DriverError::Protocol(body))),
                            )
                            .unwrap();
                    }
                    1 => {
                        owner
                            .retain_turn_for_source_control(
                                &original,
                                Err(DispatchError::Driver(DriverError::Protocol(body))),
                            )
                            .unwrap();
                    }
                    2 => {
                        owner
                            .retain_cleanup_for_source_control(
                                &original,
                                Err(CleanupError::Driver(DriverError::Protocol(body))),
                            )
                            .unwrap();
                    }
                    _ => unreachable!(),
                }
            }
            let handed = match mode {
                0 => owner.handoff_start_result(&original, &original, &mut start),
                1 => owner.handoff_turn_result(&original, &original, &mut turn),
                2 => owner.handoff_cleanup_result(&original, &original, &mut cleanup),
                _ => unreachable!(),
            };
            assert!(handed.unwrap());
            if mode == 2 {
                match cleanup.as_ref().unwrap().as_ref().err().unwrap() {
                    CleanupError::Archive(error) => {
                        assert!(refused && matches!(error, ArchiveError::Fit));
                    }
                    CleanupError::Driver(DriverError::Protocol(text)) => {
                        assert!(!refused);
                        assert_eq!(text.as_ptr(), body_pointer);
                    }
                    _ => panic!("original cleanup body changed"),
                }
            } else {
                let returned = if mode == 0 {
                    start.as_ref().unwrap().as_ref().err().unwrap()
                } else {
                    turn.as_ref().unwrap().as_ref().err().unwrap()
                };
                match returned {
                    DispatchError::Archive(ArchiveError::Fit) => assert!(refused),
                    DispatchError::Driver(DriverError::Protocol(text)) => {
                        assert!(!refused);
                        assert_eq!(text.as_ptr(), body_pointer);
                    }
                    _ => panic!("original dispatch body changed"),
                }
            }
            let mut retry_start = None;
            let mut retry_turn = None;
            let mut retry_cleanup = None;
            let retry = match mode {
                0 => owner.start_to_slot_future(&original, None, &original, &mut retry_start),
                1 => owner.turn_to_slot_future(
                    &original,
                    "no retry",
                    Duration::ZERO,
                    &original,
                    &mut retry_turn,
                ),
                2 => owner.cleanup_to_slot_future(&original, &original, &mut retry_cleanup),
                _ => unreachable!(),
            };
            assert!(matches!(retry, Err(ArchiveError::Identity)));
            drop(retry);
            assert!(retry_start.is_none() && retry_turn.is_none() && retry_cleanup.is_none());
            let results = owner.retained_results(&original).unwrap();
            assert!(results.start_started);
            assert_eq!(results.turn_started, mode != 0);
            assert_eq!(results.cleanup_started, mode == 2);
            if mode == 2 {
                assert!(matches!(
                    results.turn,
                    Some(Err(DispatchError::Driver(DriverError::Timeout)))
                ));
            }
            let prepared = owner.prepared_original(&original).unwrap();
            let mut preparation = prepared.borrow(&original).unwrap();
            let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
            let foreground = supervision
                .root_mut()
                .unwrap()
                .borrow_foreground(&original)
                .unwrap();
            let driver = foreground.driver().unwrap();
            assert_eq!(std::ptr::from_ref(driver), pointer);
            assert!(driver.proc.is_none() && driver.helpers.try_lock().unwrap().is_empty());
            assert!(driver.prompt_lease.is_none() && driver.startup.is_none());
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
        }
    }

    // SOURCE ONLY/UNCOMPILED/UNRUN. Synthetic returns and suspension; no native
    // dispatch, cleanup, reader join, archive verification or custody witness.
    #[test]
    #[cfg(panic = "unwind")]
    fn original_cleanup_handoff_preserves_return_disposition_and_first_cause() {
        use super::super::archive_work::{CleanupError, DispatchError, OriginalClaudeOwner};
        use crate::failed_evidence::ArchiveError;
        use std::{
            future::Future,
            panic::{AssertUnwindSafe, catch_unwind},
            task::{Context, Waker},
        };
        struct Opaque(std::cell::Cell<usize>);
        for case in 0..4 {
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let driver = Box::new(ClaudeDriver::new(argument_config(), sink));
            let pointer = std::ptr::from_ref(driver.as_ref());
            let context = recovery_context(driver.cfg.seat, original.clone());
            let mut owner = OriginalClaudeOwner::new(driver);
            assert!(owner.bind_original_context(context).is_ok());
            owner.begin_start_for_source_control(&original).unwrap();
            owner
                .retain_start_for_source_control(
                    &original,
                    Ok(SessionInfo {
                        session_id: "synthetic cleanup control".into(),
                        model: None,
                        cli_version: None,
                        resumed: false,
                    }),
                )
                .unwrap();
            owner.begin_turn_for_source_control(&original).unwrap();
            owner
                .retain_turn_for_source_control(
                    &original,
                    Err(DispatchError::Driver(DriverError::Timeout)),
                )
                .unwrap();
            let captured = String::from("separate original captured evidence");
            let captured_pointer = captured.as_ptr();
            {
                let prepared = owner.prepared_original(&original).unwrap();
                let mut preparation = prepared.borrow(&original).unwrap();
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let mut foreground = supervision
                    .root_mut()
                    .unwrap()
                    .borrow_foreground(&original)
                    .unwrap();
                foreground.driver_mut().unwrap().completed_turn = Some(TurnOutcome {
                    ok: true,
                    error: None,
                    text: Some(captured),
                    usage: Default::default(),
                    quota: Vec::new(),
                });
            }
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            let mut destination = None;
            assert!(matches!(
                owner.handoff_cleanup_result(&original, &original, &mut destination),
                Err(ArchiveError::Identity)
            )); // A not-started callback is never an empty successful cleanup.
            assert!(destination.is_none());
            if case == 0 {
                let mut cx = Context::from_waker(Waker::noop());
                {
                    let pending = async {
                        owner.begin_cleanup_for_source_control(&original).unwrap();
                        std::future::pending::<()>().await;
                    };
                    let mut pending = std::pin::pin!(pending);
                    assert!(pending.as_mut().poll(&mut cx).is_pending());
                }
                let results = owner.retained_results(&original).unwrap();
                assert!(results.cleanup_started && results.cleanup.is_none());
                assert_eq!(results.cleanup_returned_ok, None);
                assert!(
                    !owner
                        .handoff_cleanup_result(&original, &original, &mut destination)
                        .unwrap()
                );
                assert!(destination.is_none());
            } else {
                owner.begin_cleanup_for_source_control(&original).unwrap();
                let body = String::from("original typed cleanup refusal");
                let body_pointer = body.as_ptr();
                let result = match case {
                    1 => Err(CleanupError::Driver(DriverError::Cli(body))),
                    2 => Ok(()),
                    3 => Err(CleanupError::Archive(ArchiveError::Identity)),
                    _ => unreachable!(),
                };
                assert_eq!(
                    owner
                        .retain_cleanup_for_source_control(&original, result)
                        .unwrap(),
                    case == 2,
                );
                let payload = Box::new(Opaque(std::cell::Cell::new(57)));
                let payload_pointer = std::ptr::from_ref(payload.as_ref());
                let unwind = catch_unwind(AssertUnwindSafe(|| std::panic::resume_unwind(payload)));
                let payload = unwind.unwrap_err().downcast::<Opaque>().ok().unwrap();
                assert_eq!(std::ptr::from_ref(payload.as_ref()), payload_pointer);
                assert_eq!(payload.0.get(), 57);
                assert!(
                    owner
                        .handoff_cleanup_result(&foreign, &original, &mut destination)
                        .is_err()
                );
                assert!(
                    owner
                        .handoff_cleanup_result(&original, &foreign, &mut destination)
                        .is_err()
                );
                assert!(destination.is_none());
                let mut occupied = Some(Err(CleanupError::Driver(DriverError::Timeout)));
                assert!(
                    owner
                        .handoff_cleanup_result(&original, &original, &mut occupied)
                        .is_err()
                );
                assert!(matches!(
                    occupied,
                    Some(Err(CleanupError::Driver(DriverError::Timeout)))
                ));
                let results = owner.retained_results(&original).unwrap();
                assert_eq!(results.cleanup_returned_ok, Some(case == 2));
                if case == 1 {
                    assert!(matches!(
                        results.cleanup,
                        Some(Err(CleanupError::Driver(DriverError::Cli(body))))
                            if body.as_ptr() == body_pointer
                    ));
                }
                assert!(
                    owner
                        .handoff_cleanup_result(&original, &original, &mut destination)
                        .unwrap()
                );
                match (case, destination.as_ref()) {
                    (1, Some(Err(CleanupError::Driver(DriverError::Cli(body))))) => {
                        assert_eq!(body.as_ptr(), body_pointer)
                    }
                    (2, Some(Ok(()))) => {}
                    (3, Some(Err(CleanupError::Archive(ArchiveError::Identity)))) => {}
                    _ => panic!("original cleanup return lost or replaced"),
                }
                assert!(
                    !owner
                        .handoff_cleanup_result(&original, &original, &mut None)
                        .unwrap()
                );
                let results = owner.retained_results(&original).unwrap();
                assert!(results.cleanup_started && results.cleanup.is_none());
                assert_eq!(results.cleanup_returned_ok, Some(case == 2));
                assert!(
                    owner
                        .retain_cleanup_for_source_control(&original, Ok(()))
                        .is_err()
                );
            }
            assert!(owner.begin_cleanup_for_source_control(&original).is_err());
            assert!(owner.begin_start_for_source_control(&original).is_err());
            assert!(owner.begin_turn_for_source_control(&original).is_err());
            let results = owner.retained_results(&original).unwrap();
            assert!(matches!(
                results.turn,
                Some(Err(DispatchError::Driver(DriverError::Timeout)))
            ));
            assert_eq!(results.start_returned_ok, Some(true));
            assert!(results.start.is_some());
            let prepared = owner.prepared_original(&original).unwrap();
            let mut preparation = prepared.borrow(&original).unwrap();
            let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
            let foreground = supervision
                .root_mut()
                .unwrap()
                .borrow_foreground(&original)
                .unwrap();
            let driver = foreground.driver().unwrap();
            assert_eq!(std::ptr::from_ref(driver), pointer);
            assert_eq!(
                driver
                    .completed_turn
                    .as_ref()
                    .unwrap()
                    .text
                    .as_ref()
                    .unwrap()
                    .as_ptr(),
                captured_pointer,
            );
            assert!(driver.proc.is_none() && driver.helpers.try_lock().unwrap().is_empty());
            assert!(driver.prompt_lease.is_none() && driver.startup.is_none());
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
        }
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn borrowed_slot_preparation_retains_box_until_final_infallible_move() {
        use super::super::archive_work::{PreparationOwner, PreparationPosition};
        use crate::failed_evidence::ArchiveError;
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let original = archive_startup_operation();
        let foreign = archive_startup_operation();
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let mut slot = Some(Box::new(ClaudeDriver::new(argument_config(), sink)));
        let pointer = std::ptr::from_ref(slot.as_ref().unwrap().as_ref());
        let mut invoked = false;
        assert!(matches!(
            PreparationOwner::prepare_from_slot_for_source_control(&mut slot, &original, || {
                invoked = true
            }),
            Err(ArchiveError::Identity)
        ));
        assert!(!invoked && slot.is_some()); // Missing host, before boundary/Arc/Box MOVE.
        assert_eq!(std::ptr::from_ref(slot.as_ref().unwrap().as_ref()), pointer);
        let driver = slot.as_mut().unwrap();
        driver.host = Some(recovery_context(driver.cfg.seat, original.clone()));
        driver.parser.last_text = Some("original saved evidence".into()); // Inert fixture only.
        let text_pointer = driver.parser.last_text.as_ref().unwrap().as_ptr();
        assert!(matches!(
            PreparationOwner::prepare_from_slot_for_source_control(&mut slot, &foreign, || {
                invoked = true
            }),
            Err(ArchiveError::Identity)
        ));
        assert!(!invoked && slot.is_some());
        let overflowed = archive_startup_operation();
        let seat = slot.as_ref().unwrap().cfg.seat;
        slot.as_mut().unwrap().host = Some(recovery_context(seat, overflowed.clone()));
        overflowed.overflow();
        assert!(matches!(
            PreparationOwner::prepare_from_slot(&mut slot, &overflowed),
            Err(ArchiveError::Operation(
                crate::operation::OperationError::CaptureOverflow
            ))
        ));
        assert_eq!(std::ptr::from_ref(slot.as_ref().unwrap().as_ref()), pointer);
        slot.as_mut().unwrap().host = Some(recovery_context(seat, original.clone()));
        let charged = original.captured_bytes();
        let deadline = original.allowance().deadline();
        struct Opaque {
            value: std::cell::Cell<usize>,
            dropped: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl Drop for Opaque {
            fn drop(&mut self) {
                self.dropped
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let payload = Box::new(Opaque {
            value: std::cell::Cell::new(73),
            dropped: dropped.clone(),
        });
        let payload_pointer = std::ptr::from_ref(payload.as_ref());
        let unwind = catch_unwind(AssertUnwindSafe(|| {
            let _ = PreparationOwner::prepare_from_slot_for_source_control(
                &mut slot,
                &original,
                || {
                    invoked = true;
                    std::panic::resume_unwind(payload); // AFTER prototype/prechecks, BEFORE Box MOVE.
                },
            );
        }));
        let opaque = match unwind {
            Err(payload) => payload.downcast::<Opaque>().ok().unwrap(),
            Ok(()) => panic!("boundary never unwound"),
        };
        assert!(invoked && slot.is_some());
        assert_eq!(std::ptr::from_ref(opaque.as_ref()), payload_pointer);
        assert_eq!(opaque.value.get(), 73);
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(std::ptr::from_ref(slot.as_ref().unwrap().as_ref()), pointer);
        assert_eq!(
            slot.as_ref()
                .unwrap()
                .parser
                .last_text
                .as_ref()
                .unwrap()
                .as_ptr(),
            text_pointer
        );
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(original.allowance().deadline(), deadline);
        drop(opaque); // Test payload teardown only; no production discard authority.
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
        let mut owner = PreparationOwner::prepare_from_slot(&mut slot, &original)
            .ok()
            .unwrap();
        assert!(slot.is_none()); // Sole MOVE only after all fallible constructors finish.
        assert_eq!(
            owner.position(&original).unwrap(),
            (PreparationPosition::Ready, false)
        );
        assert!(matches!(
            PreparationOwner::prepare_from_slot(&mut slot, &original),
            Err(ArchiveError::Identity)
        ));
        {
            let mut preparation = owner.borrow(&original).unwrap();
            let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
            let foreground = supervision
                .root_mut()
                .unwrap()
                .borrow_foreground(&original)
                .unwrap();
            let driver = foreground.driver().unwrap();
            assert_eq!(std::ptr::from_ref(driver), pointer);
            assert_eq!(
                driver.parser.last_text.as_ref().unwrap().as_ptr(),
                text_pointer
            );
            assert!(driver.proc.is_none() && driver.helpers.try_lock().unwrap().is_empty());
            assert!(driver.prompt_lease.is_none() && driver.startup.is_none());
        }
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(original.allowance().deadline(), deadline);
        // No native method/worker/physical join/custody or complete-fit witness.
    }

    // SOURCE ONLY/UNRUN. Future separately admitted inert fixture child joins;
    // synthetic correlation/handshake transitions are not native CLI evidence.
    #[tokio::test]
    async fn retained_capture_handoff_precedes_archive_without_settling_first_failure() {
        use super::super::{
            archive_work::{Interruption, OriginalClaudeOwner},
            reader_handoff::Pipe,
        };
        use crate::failed_evidence::ArchiveError;
        let original = archive_startup_operation();
        let foreign = archive_startup_operation();
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let driver = Box::new(ClaudeDriver::new(argument_config(), sink.clone()));
        let context = recovery_context(driver.cfg.seat, original.clone());
        let mut owner = OriginalClaudeOwner::new(driver);
        assert!(owner.bind_original_context(context).is_ok());
        // Controller/root installed BEFORE the future inert child fixture. No
        // source run or native adoption; same independently joined fixture below.
        owner.begin_start_for_source_control(&original).unwrap();
        owner
            .retain_start_for_source_control(
                &original,
                Ok(SessionInfo {
                    session_id: "fixture".into(),
                    model: None,
                    cli_version: None,
                    resumed: false,
                }),
            )
            .unwrap(); // Injected controller return, never actual start evidence.
        owner.begin_turn_for_source_control(&original).unwrap();
        owner
            .retain_turn_for_source_control(
                &original,
                Err(super::super::archive_work::DispatchError::Driver(
                    DriverError::Timeout,
                )),
            )
            .unwrap(); // First synthetic dispatch cause is separate from captured evidence.
        let input = original.prepare_claude_input().unwrap();
        let input = std::str::from_utf8(&input).unwrap();
        original
            .validate_claude_frame(&json!({
                "type":"user", "isReplay":true, "parent_tool_use_id":null,
                "session_id":"fixture", "uuid":input,
                "message":{"role":"user", "content":"fixture"}
            }))
            .unwrap();
        original
            .validate_claude_frame(&json!({
                "type":"result", "subtype":"success", "user_message_uuid":input
            }))
            .unwrap();
        let (mut child, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let physical = child.physical_child_id.clone();
        let gate = original.reader_handoff();
        gate.attach(&original, &physical).unwrap();
        gate.terminal_processed(&original).unwrap();
        let request = gate.request(&original, &physical).unwrap();
        gate.acknowledge(&request, Pipe::Stdout).unwrap(); // ONE ACK, not ready.
        child.handoff = Some(gate.clone());
        child.handoff_request = Some(request.clone());
        let late = String::from("synthetic old boundary bytes");
        let late_pointer = late.as_ptr();
        let late_count = late.len();
        original
            .charge(late_count + std::mem::size_of::<super::super::ChildFrame>())
            .unwrap();
        child.pending_frame = Some(super::super::ChildFrame {
            text: late,
            origin: None,
            operation: original.clone(),
        }); // Synthetic SAME old body; never publish as a normal transcript.
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (pointer, driver_pointer) = {
            let prepared = owner.prepared_original(&original).unwrap();
            let mut preparation = prepared.borrow(&original).unwrap();
            let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
            let mut foreground = supervision
                .root_mut()
                .unwrap()
                .borrow_foreground(&original)
                .unwrap();
            let driver = foreground.driver_mut().unwrap();
            driver.proc = Some(child);
            driver.parser.runtime_operation = Some(original.clone());
            driver.idle = false;
            driver.emitter = Some(EntryEmitter::new(
                driver.cfg.seat,
                sink.for_operation(original.clone()),
            ));
            driver.prompt_lease = Some(Box::new(BoundaryPromptLease {
                released: Arc::new(tokio::sync::Notify::new()),
                requests: requests.clone(),
            }));
            original.charge(256).unwrap(); // Synthetic fixture strings only.
            driver.completed_turn = Some(TurnOutcome {
                ok: false,
                error: Some("original failed terminal".into()),
                text: Some("canonical original answer".into()),
                usage: Usage::default(),
                quota: Vec::new(),
            });
            let pointer = driver
                .completed_turn
                .as_ref()
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .as_ptr();
            let driver_pointer = std::ptr::from_ref(&*driver);
            // A captured value blocks archive until its separate journal MOVE.
            assert!(driver.original_archive_coverage(&original).is_err());
            (pointer, driver_pointer)
        };
        {
            let prepared = owner.prepared_original(&original).unwrap();
            let mut preparation = prepared.borrow(&original).unwrap();
            let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
            supervision
                .root_mut()
                .unwrap()
                .interrupt(&original, Interruption::OperatorStop)
                .unwrap();
        }
        // Journal first supervisor failure is a DIFFERENT canonical slot.
        let supervisor = Err::<TurnOutcome, _>(DriverError::Timeout);
        let mut journal_capture = None;
        let mut occupied = Some(TurnOutcome {
            ok: true,
            error: None,
            text: Some("prior journal owner".into()),
            usage: Usage::default(),
            quota: Vec::new(),
        });
        let occupied_pointer = occupied.as_ref().unwrap().text.as_ref().unwrap().as_ptr();
        assert!(matches!(
            owner.handoff_captured_before_archive(&foreign, &original, &mut journal_capture),
            Err(ArchiveError::Identity)
        ));
        assert!(matches!(
            owner.handoff_captured_before_archive(&original, &foreign, &mut journal_capture),
            Err(ArchiveError::Identity)
        ));
        assert!(matches!(
            owner.handoff_captured_before_archive(&original, &original, &mut occupied),
            Err(ArchiveError::Identity)
        ));
        assert_eq!(
            occupied.as_ref().unwrap().text.as_ref().unwrap().as_ptr(),
            occupied_pointer
        );
        assert!(journal_capture.is_none());
        let charged = original.captured_bytes();
        let deadline = original.allowance().deadline();
        original.overflow();
        original.refuse_claude_frame(); // Later failure does not erase earlier captured evidence.
        assert!(original.check_claude_terminal().is_err());
        gate.delivered(&original, Pipe::Stdout, late_count)
            .unwrap_err(); // Synthetic boundary-byte failure.
        // Gate was attached AFTER the fixture actually joined; record that
        // independently observed exit, never infer it from an ACK or timeout.
        gate.child_exit_observed();
        assert!(
            owner
                .handoff_captured_before_archive(&original, &original, &mut journal_capture)
                .unwrap()
        );
        assert_eq!(
            journal_capture
                .as_ref()
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .as_ptr(),
            pointer
        );
        assert!(matches!(supervisor, Err(DriverError::Timeout))); // Not replaced by captured result.
        assert!(matches!(
            owner.retained_results(&original).unwrap().turn,
            Some(Err(super::super::archive_work::DispatchError::Driver(
                DriverError::Timeout
            )))
        ));
        assert!(owner.begin_turn_for_source_control(&original).is_err());
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 0); // No actor settlement.
        assert!(
            owner
                .handoff_captured_before_archive(&original, &original, &mut journal_capture)
                .is_err()
        );
        let mut second = None;
        assert!(
            !owner
                .handoff_captured_before_archive(&original, &original, &mut second)
                .unwrap()
        );
        assert!(second.is_none()); // No double return/usage/refund witness.
        {
            let prepared = owner.prepared_original(&original).unwrap();
            let mut preparation = prepared.borrow(&original).unwrap();
            let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
            let root = supervision.root_mut().unwrap();
            let status = root.status(&original).unwrap();
            assert_eq!(status.first_interruption, Some(Interruption::OperatorStop));
            assert!(!status.result_retained && !status.worker_owned);
            assert!(root.borrow_foreground(&original).is_err());
            assert!(root.accept_std_archive(&original).is_err());
            assert!(root.take_captured_after_custody(&original).is_err());
            let mut checkout = root.checkout(&original).unwrap();
            checkout
                .inspect_for_source_control(&original, |driver, saved| {
                    assert_eq!(std::ptr::from_ref(&*driver), driver_pointer);
                    assert!(saved.same_instance(&original));
                    assert!(driver.completed_turn.is_none() && driver.emitter.is_none());
                    assert!(driver.prompt_lease.is_some() && !driver.idle);
                    assert!(
                        driver
                            .parser
                            .runtime_operation
                            .as_ref()
                            .unwrap()
                            .same_instance(&original)
                    );
                    let child = driver.proc.as_ref().unwrap();
                    assert!(child.physical_supervision_complete()); // Fixture must actually join when admitted.
                    assert!(Arc::ptr_eq(&child.physical_child_id, &physical));
                    assert!(child.handoff_request.is_some());
                    assert_eq!(
                        child.pending_frame.as_ref().unwrap().text.as_ptr(),
                        late_pointer
                    );
                    assert!(
                        child
                            .pending_frame
                            .as_ref()
                            .unwrap()
                            .operation
                            .same_instance(saved)
                    );
                    assert!(driver.original_archive_coverage(saved).is_ok()); // Coverage only, not file custody.
                    Err(ArchiveError::Filesystem) // Synthetic callback, NO archive I/O/thread.
                })
                .unwrap();
        }
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(original.allowance().deadline(), deadline);
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[test]
    fn canonical_dispatch_frames_are_unpolled_and_closed_originals_keep_owners() {
        use super::super::archive_work::{
            DispatchError, Interruption, PreparationOwner, PreparationPosition,
        };
        use crate::failed_evidence::ArchiveError;
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        // SOURCE ONLY/UNRUN. Never poll a valid live dispatch into the native
        // body. Only foreign, refused, interrupted and overflow cases poll.
        for mode in 0..3 {
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let mut driver = Box::new(ClaudeDriver::new(argument_config(), sink));
            driver.host = Some(recovery_context(driver.cfg.seat, original.clone()));
            let pointer = std::ptr::from_ref(driver.as_ref());
            let mut owner = if mode == 0 {
                PreparationOwner::prepare(Err((ArchiveError::Fit, driver)), &original)
            } else {
                driver.prepare_original_owner(&original)
            };
            let expected = if mode == 0 {
                (PreparationPosition::Driver, true)
            } else {
                (PreparationPosition::Ready, false)
            };
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            fn send<F: Future + Send>(_: &F) {}
            let start_bytes = {
                let future = owner.start_original(&original, None);
                send(&future);
                std::mem::size_of_val(&future)
            }; // Dropped unpolled; no native check/effect/boxed allocation.
            {
                let future = owner.start_original(&original, Some("unpolled resume"));
                send(&future);
                assert_eq!(std::mem::size_of_val(&future), start_bytes);
            }
            let turn_bytes = {
                let future = owner.turn_original(&original, "", Duration::ZERO);
                send(&future);
                std::mem::size_of_val(&future)
            };
            {
                let future =
                    owner.turn_original(&original, "unpolled prompt", Duration::from_secs(1));
                send(&future);
                assert_eq!(std::mem::size_of_val(&future), turn_bytes);
            }
            // This synchronous check constructs/drops the actual families;
            // either fixed fit or refusal is honest. No fit/PASS is invented.
            assert!(matches!(
                owner.check_dispatch_frames(&original),
                Ok(()) | Err(ArchiveError::Fit)
            ));
            assert!(matches!(
                owner.check_dispatch_frames(&foreign),
                Err(ArchiveError::Identity)
            ));
            assert_eq!(owner.position(&original).unwrap(), expected);
            if mode != 0 {
                let mut preparation = owner.borrow(&original).unwrap();
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let root = supervision.root_mut().unwrap();
                let foreground = root.borrow_foreground(&original).unwrap();
                let driver = foreground.driver().unwrap();
                assert_eq!(std::ptr::from_ref(driver), pointer);
                assert!(
                    driver.proc.is_none()
                        && driver.prompt_lease.is_none()
                        && driver.startup.is_none()
                );
                drop(foreground);
                root.interrupt(&original, Interruption::OperatorStop)
                    .unwrap();
            }
            if mode == 2 {
                original.overflow();
            }
            let mut cx = Context::from_waker(Waker::noop());
            {
                let future = owner.start_original(&foreign, None);
                let mut future = std::pin::pin!(future);
                assert!(matches!(
                    future.as_mut().poll(&mut cx),
                    Poll::Ready(Err(DispatchError::Archive(ArchiveError::Identity)))
                ));
            }
            {
                let future = owner.start_original(&original, None);
                let mut future = std::pin::pin!(future);
                assert!(matches!(
                    future.as_mut().poll(&mut cx),
                    Poll::Ready(Err(DispatchError::Archive(_)))
                ));
            }
            {
                let future =
                    owner.turn_original(&original, "must not reach native", Duration::ZERO);
                let mut future = std::pin::pin!(future);
                assert!(matches!(
                    future.as_mut().poll(&mut cx),
                    Poll::Ready(Err(DispatchError::Archive(_)))
                ));
            }
            assert_eq!(owner.position(&original).unwrap(), expected);
            let mut preparation = owner.borrow(&original).unwrap();
            if mode == 0 {
                assert!(matches!(
                    preparation.first_refusal().unwrap(),
                    Some(ArchiveError::Fit)
                ));
                assert!(preparation.supervisor().is_err());
            } else {
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let status = supervision.root_mut().unwrap().status(&original).unwrap();
                assert!(status.returned && !status.in_service && !status.result_retained);
                assert_eq!(status.first_interruption, Some(Interruption::OperatorStop));
                assert!(!status.worker_owned && !status.worker_join_retained);
            }
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
        }
    }

    #[test]
    fn preparation_physical_pass_preserves_refusal_and_never_promotes_driver() {
        use super::super::archive_work::{
            CleanupError, Interruption, PreparationOwner, PreparationPosition,
        };
        use crate::failed_evidence::ArchiveError;
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        // SOURCE ONLY/UNRUN. Empty passes construct no real child/reader/task.
        for (has_host, ready) in [(false, false), (true, false), (true, true)] {
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let mut driver = Box::new(ClaudeDriver::new(argument_config(), sink));
            if has_host {
                driver.host = Some(recovery_context(driver.cfg.seat, original.clone()));
            }
            let mut owner = if ready {
                driver.prepare_original_owner(&original)
            } else {
                // Retain an independently refused construction owner, no retry.
                PreparationOwner::prepare(Err((ArchiveError::Fit, driver)), &original)
            };
            let expected = if ready {
                (PreparationPosition::Ready, false)
            } else {
                (PreparationPosition::Driver, true)
            };
            assert_eq!(owner.position(&original).unwrap(), expected);
            if ready {
                let mut preparation = owner.borrow(&original).unwrap();
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                supervision
                    .root_mut()
                    .unwrap()
                    .interrupt(&original, Interruption::OperatorStop)
                    .unwrap();
            }
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            original.overflow();
            let mut cx = Context::from_waker(Waker::noop());
            {
                let future = owner.supervise_before_archive(&foreign);
                let mut future = std::pin::pin!(future);
                assert!(matches!(
                    future.as_mut().poll(&mut cx),
                    Poll::Ready(Err(CleanupError::Archive(ArchiveError::Identity)))
                ));
            }
            {
                let future = owner.supervise_before_archive(&original);
                fn send(_: &(impl Future<Output = Result<(), CleanupError>> + Send)) {}
                send(&future);
                let mut future = std::pin::pin!(future);
                let result = future.as_mut().poll(&mut cx);
                if has_host {
                    assert!(matches!(result, Poll::Ready(Ok(()))));
                } else {
                    assert!(matches!(
                        result,
                        Poll::Ready(Err(CleanupError::Archive(ArchiveError::Identity)))
                    ));
                }
            }
            assert_eq!(owner.position(&original).unwrap(), expected);
            let mut preparation = owner.borrow(&original).unwrap();
            if ready {
                assert!(preparation.first_refusal().unwrap().is_none());
                let mut supervision = preparation.supervisor().unwrap().borrow(&original).unwrap();
                let root = supervision.root_mut().unwrap();
                let status = root.status(&original).unwrap();
                assert!(status.returned && !status.in_service && !status.result_retained);
                assert_eq!(status.first_interruption, Some(Interruption::OperatorStop));
                assert!(root.borrow_foreground(&original).is_err());
                assert!(root.accept_std_archive(&original).is_err());
            } else {
                assert!(matches!(
                    preparation.first_refusal().unwrap(),
                    Some(ArchiveError::Fit)
                ));
                assert!(preparation.supervisor().is_err()); // Physical Ok never promotes refusal.
            }
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
            assert!(original.charge(1).is_err());
        }
    }

    #[test]
    fn retained_root_physical_pass_keeps_original_result_and_refuses_foreign_host() {
        use super::super::archive_work::{CleanupError, Interruption, RetainedArchive};
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        // SOURCE ONLY/UNRUN. No fixture child/thread or task is constructed.
        // An empty physical pass proves no real child/reader/archive joins.
        for wrong_host in [false, true] {
            let original = archive_startup_operation();
            let foreign = archive_startup_operation();
            let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
            let mut driver = Box::new(ClaudeDriver::new(argument_config(), sink));
            driver.host = Some(recovery_context(driver.cfg.seat, original.clone()));
            let pointer = std::ptr::from_ref(driver.as_ref());
            let work = driver.prepare_archive_work(&original).ok().unwrap();
            let mut root = RetainedArchive::prepare(work).ok().unwrap();
            let text = String::from("original failed terminal retained");
            let text_pointer = text.as_ptr();
            {
                let mut foreground = root.borrow_foreground(&original).unwrap();
                let driver = foreground.driver_mut().unwrap();
                driver.completed_turn = Some(TurnOutcome {
                    ok: false,
                    error: None,
                    text: Some(text),
                    usage: Usage::default(),
                    quota: Vec::new(),
                });
                if wrong_host {
                    driver.host = Some(recovery_context(driver.cfg.seat, foreign.clone()));
                }
            } // Test injection BEFORE interruption; production never rebinds this root.
            let charged = original.captured_bytes();
            let deadline = original.allowance().deadline();
            root.interrupt(&original, Interruption::OperatorStop)
                .unwrap();
            original.overflow();
            assert!(root.borrow_foreground(&original).is_err());
            let mut cx = Context::from_waker(Waker::noop());
            {
                let pass = root.supervise_before_archive(&foreign);
                let mut pass = std::pin::pin!(pass);
                assert!(matches!(
                    pass.as_mut().poll(&mut cx),
                    Poll::Ready(Err(CleanupError::Archive(
                        crate::failed_evidence::ArchiveError::Identity
                    )))
                ));
            }
            {
                let pass = root.supervise_before_archive(&original);
                fn send(_: &(impl Future<Output = Result<(), CleanupError>> + Send)) {}
                send(&pass);
                let mut pass = std::pin::pin!(pass);
                let result = pass.as_mut().poll(&mut cx);
                if wrong_host {
                    assert!(matches!(
                        result,
                        Poll::Ready(Err(CleanupError::Archive(
                            crate::failed_evidence::ArchiveError::Identity
                        )))
                    ));
                } else {
                    assert!(matches!(result, Poll::Ready(Ok(()))));
                }
            }
            let status = root.status(&original).unwrap();
            assert!(status.returned && !status.in_service && !status.result_retained);
            assert_eq!(status.first_interruption, Some(Interruption::OperatorStop));
            assert!(!status.worker_owned && !status.worker_join_retained);
            assert!(root.take_captured_after_custody(&original).is_err());
            assert!(root.accept_std_archive(&original).is_err());
            assert!(root.borrow_foreground(&original).is_err());
            // Inert callback inspection after the physical pass. This consumes
            // a synthetic failed archive attempt and performs NO filesystem I/O.
            let mut checkout = root.checkout(&original).unwrap();
            checkout
                .inspect_for_source_control(&original, |driver, saved| {
                    assert_eq!(pointer, std::ptr::from_ref(&*driver));
                    assert!(saved.same_instance(&original));
                    assert!(
                        driver
                            .host
                            .as_ref()
                            .unwrap()
                            .operation
                            .same_instance(if wrong_host { &foreign } else { &original })
                    );
                    assert_eq!(
                        driver
                            .completed_turn
                            .as_ref()
                            .unwrap()
                            .text
                            .as_ref()
                            .unwrap()
                            .as_ptr(),
                        text_pointer
                    );
                    assert!(driver.proc.is_none() && driver.helpers.get_mut().is_empty());
                    Err(crate::failed_evidence::ArchiveError::Filesystem)
                })
                .unwrap();
            drop(checkout);
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.allowance().deadline(), deadline);
        }
    }

    #[test]
    fn original_archive_work_preparation_returns_box_on_missing_or_wrong_host_instance() {
        let original = archive_startup_operation();
        let replacement = archive_startup_operation();
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let driver = Box::new(ClaudeDriver::new(argument_config(), sink));
        let pointer = std::ptr::from_ref(driver.as_ref());
        let charged = original.captured_bytes();
        let (error, mut driver) = match driver.prepare_archive_work(&original) {
            Err(refused) => refused,
            Ok(_) => panic!("missing host accepted"),
        };
        assert!(matches!(
            error,
            crate::failed_evidence::ArchiveError::Identity
        ));
        assert_eq!(pointer, std::ptr::from_ref(driver.as_ref()));
        driver.host = Some(recovery_context(driver.cfg.seat, original.clone()));
        let (error, driver) = match driver.prepare_archive_work(&replacement) {
            Err(refused) => refused,
            Ok(_) => panic!("replacement instance accepted"),
        };
        assert!(matches!(
            error,
            crate::failed_evidence::ArchiveError::Identity
        ));
        assert_eq!(pointer, std::ptr::from_ref(driver.as_ref()));
        assert!(
            driver
                .host
                .as_ref()
                .unwrap()
                .operation
                .same_instance(&original)
        );
        let owner = driver.prepare_archive_work(&original).ok().unwrap();
        assert_eq!(pointer, std::ptr::from_ref(owner.driver()));
        let mut retained = super::super::archive_work::RetainedArchive::prepare(owner)
            .ok()
            .unwrap();
        assert!(retained.accept_std_archive(&replacement).is_err());
        assert!(retained.accept_std_archive(&original).is_err());
        let status = retained.status(&original).unwrap();
        assert!(status.returned && !status.in_service);
        assert!(!status.worker_owned && !status.worker_join_retained);
        assert!(!status.result_retained && !status.panic_retained);
        let owner = retained.checkout(&original).unwrap();
        // Refused custody did not consume the sole original checkout allowance.
        drop(owner);
        assert_eq!(original.captured_bytes(), charged);
        // No worker, callback, path effect or canonical body is created by preparation.
    }

    #[tokio::test]
    async fn accepted_archive_disposes_raw_only_and_preserves_failed_prompt_ack() {
        use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
        let root = custody_fixture_root();
        let original = archive_operation_at(&root);
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (child, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let (mut helper, _) = joined_cleanup_child_with_operation(original.clone()).await;
        original.charge(64).unwrap(); // Synthetic fixture body admission only.
        helper.helper_output = "retained original helper".into();
        let mut driver = ClaudeDriver::new(argument_config(), sink.clone());
        driver.host = Some(recovery_context(driver.cfg.seat, original.clone()));
        driver.helpers.get_mut().push(helper);
        driver.proc = Some(child);
        driver.parser.feed_owned(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"canonical failed evidence"}]}}"#,
            &original,
        ).unwrap();
        original.charge(2048).unwrap(); // Synthetic startup/pairing fixture bodies.
        driver.startup = Some(StartupCapture {
            operation: original.clone(),
            text: "original startup evidence".into(),
            info: SessionInfo {
                session_id: "original-session".into(),
                model: Some("fixture".into()),
                cli_version: Some("fixture".into()),
                resumed: false,
            },
            published_session_id: "original-session".into(),
            effect_accepted: true,
            failure: Some(crate::operation::OperationError::CaptureOverflow),
        });
        let mut emitter = EntryEmitter::new(driver.cfg.seat, sink.clone());
        emitter.calls.insert(
            "unpaired-original".into(),
            super::super::CachedCall {
                tool: "submit".into(),
                args: json!({"decision_id": "original"}),
                operation: Some(original.clone()),
                origin: None,
            },
        );
        driver.emitter = Some(emitter);
        let requests = Arc::new(AtomicUsize::new(0));
        driver.prompt_lease = Some(Box::new(CleanupPromptLease {
            phase: Arc::new(AtomicU8::new(1)), // Actual fixture ACK refuses.
            requests: requests.clone(),
        }));
        let child_id = driver.proc.as_ref().unwrap().physical_child_id().to_owned();
        let _ = original.charge(crate::operation::OPERATION_BYTES + 1);
        let charged = original.captured_bytes();
        let observed = driver.publish_original_archive(&original).unwrap();
        assert!(driver.parser.last_text.is_some()); // Publication did not consume.
        driver
            .accept_original_archive(&original, &observed)
            .unwrap();
        assert!(driver.parser.last_text.is_none());
        assert!(driver.parser.last_text_operation.is_none());
        assert!(driver.startup.is_none());
        assert!(driver.emitter.is_none()); // Archived pairing, no transcript conversion.
        assert_eq!(driver.helpers.get_mut().len(), 1); // Physical owners retained.
        assert_eq!(driver.helpers.get_mut()[0].queued_frames(), 0);
        assert!(!driver.helpers.get_mut()[0].has_retained_helper_output());
        assert_eq!(driver.proc.as_ref().unwrap().queued_frames(), 0);
        assert_eq!(driver.proc.as_ref().unwrap().physical_child_id(), child_id);
        assert!(
            driver
                .host
                .as_ref()
                .unwrap()
                .operation
                .same_instance(&original)
        );
        assert!(driver.prompt_lease.is_some());
        assert_eq!(requests.load(Ordering::SeqCst), 0); // No implicit prompt ACK.
        assert_eq!(original.captured_bytes(), charged); // No refund/reset.
        assert!(
            driver
                .accept_original_archive(&original, &observed)
                .is_err()
        );
        assert!(driver.stop().await.is_err()); // Separate prompt obligation refuses.
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert!(driver.prompt_lease.is_some());
        assert_eq!(driver.proc.as_ref().unwrap().physical_child_id(), child_id);
        assert_eq!(original.captured_bytes(), charged);
        assert!(original.borrow_archive_chunk(&original).is_ok()); // Not settled.
        assert!(sink.stop_delivery().await.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn archive_custody_refuses_stale_instance_changed_body_and_checked_out_storage() {
        let root = custody_fixture_root();
        let original = archive_operation_at(&root);
        let replacement = archive_operation_at(&root);
        assert_eq!(original.id(), replacement.id());
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (child, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let mut driver = ClaudeDriver::new(argument_config(), sink.clone());
        driver.proc = Some(child);
        driver.parser.feed_owned(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"canonical evidence"}]}}"#,
            &original,
        ).unwrap();
        let pointer = driver.parser.last_text.as_ref().unwrap().as_ptr();
        let observed = driver.publish_original_archive(&original).unwrap();
        let charged = original.captured_bytes();
        assert!(
            driver
                .accept_original_archive(&replacement, &observed)
                .is_err()
        );
        let borrowed = original.borrow_archive_chunk(&original).unwrap();
        assert!(
            driver
                .accept_original_archive(&original, &observed)
                .is_err()
        );
        drop(borrowed); // Cancellation/borrow return alone is not custody.
        // Equal-size in-place synthetic mutation: same identities/length are
        // insufficient. No production body clone or changed-operation inference.
        driver
            .parser
            .last_text
            .as_mut()
            .unwrap()
            .replace_range(0..1, "C");
        assert!(
            driver
                .accept_original_archive(&original, &observed)
                .is_err()
        );
        assert_eq!(driver.parser.last_text.as_ref().unwrap().as_ptr(), pointer);
        assert!(driver.proc.as_ref().unwrap().queued_frames() > 0);
        driver
            .parser
            .last_text
            .as_mut()
            .unwrap()
            .replace_range(0..1, "c");
        let retained = driver.proc.as_ref().unwrap().stdout_chunk.clone();
        assert!(
            driver
                .accept_original_archive(&original, &observed)
                .is_err()
        );
        drop(retained); // Shared storage no longer borrowed; still no release.
        assert!(driver.parser.last_text.is_some());
        let (release, waiting) = tokio::sync::oneshot::channel::<()>();
        driver
            .proc
            .as_mut()
            .unwrap()
            .readers
            .push(tokio::spawn(async move {
                waiting.await.unwrap();
                Ok(())
            }));
        assert!(
            driver
                .accept_original_archive(&original, &observed)
                .is_err()
        );
        assert_eq!(driver.parser.last_text.as_ref().unwrap().as_ptr(), pointer);
        release.send(()).unwrap();
        driver
            .proc
            .as_mut()
            .unwrap()
            .join_readers(original.allowance().deadline())
            .await
            .unwrap();
        assert!(
            driver
                .proc
                .as_ref()
                .unwrap()
                .physical_supervision_complete()
        );
        assert!(driver.parser.last_text.is_some()); // Actual join alone releases nothing.
        assert_eq!(original.captured_bytes(), charged);
        original.settle(); // Synthetic already-settled owner also refuses.
        assert!(
            driver
                .accept_original_archive(&original, &observed)
                .is_err()
        );
        assert_eq!(driver.parser.last_text.as_ref().unwrap().as_ptr(), pointer);
        assert!(sink.stop_delivery().await.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn later_joined_reader_error_keeps_entire_archive_release_set_and_first_cause() {
        let root = custody_fixture_root();
        let original = archive_operation_at(&root);
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (mut first, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let (mut last, _) = joined_cleanup_child_with_operation(original.clone()).await;
        original.charge(128).unwrap(); // Existing-domain synthetic test storage.
        first.helper_output = "original earlier helper evidence".into();
        let pointer = first.helper_output.as_ptr();
        super::super::archive_stdout::unique_storage(&mut last.reader_error)
            .unwrap()
            .retain_io(
                super::super::reader_failure::ReaderStream::Stderr,
                std::io::ErrorKind::BrokenPipe.into(),
            )
            .unwrap();
        let mut driver = ClaudeDriver::new(argument_config(), sink.clone());
        driver.helpers.get_mut().extend([first, last]);
        let observed = driver.publish_original_archive(&original).unwrap();
        let charged = original.captured_bytes();
        assert!(
            driver
                .accept_original_archive(&original, &observed)
                .is_err()
        );
        let helpers = driver.helpers.get_mut();
        assert_eq!(helpers.len(), 2);
        assert_eq!(helpers[0].helper_output.as_ptr(), pointer);
        assert!(helpers[0].queued_frames() > 0);
        assert!(helpers[1].queued_frames() > 0);
        assert_eq!(
            super::super::archive_stdout::unique_storage(&mut helpers[1].reader_error)
                .unwrap()
                .first_kind(),
            Some(std::io::ErrorKind::BrokenPipe)
        );
        assert!(helpers.iter().all(ChildProc::physical_supervision_complete));
        assert_eq!(original.captured_bytes(), charged);
        assert!(driver.stop().await.is_err());
        assert_eq!(driver.helpers.get_mut().len(), 2);
        assert!(original.borrow_archive_chunk(&original).is_ok());
        assert!(sink.stop_delivery().await.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn opaque_late_reader_failure_refuses_publication_and_preserves_joined_helpers() {
        // Existing inert join fixture, UNRUN until separately admitted validation.
        let root = custody_fixture_root();
        let original = archive_operation_at(&root);
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (mut first, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let (mut last, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let storage = super::super::event_storage::EventStorage(Some(&original));
        first.helper_output = storage.copy("original earlier helper evidence").unwrap();
        let pointer = first.helper_output.as_ptr();
        let opaque = storage.copy("original boxed reader failure").unwrap();
        super::super::archive_stdout::unique_storage(&mut last.reader_error)
            .unwrap()
            .retain_io(
                super::super::reader_failure::ReaderStream::Stderr,
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, opaque),
            )
            .unwrap();
        let mut driver = ClaudeDriver::new(argument_config(), sink.clone());
        driver.helpers.get_mut().extend([first, last]);
        let charged = original.captured_bytes();
        original.overflow();
        assert!(driver.publish_original_archive(&original).is_err());
        // Coverage refusal precedes publication, not an accepted partial file.
        assert_eq!(
            std::fs::read_dir(root.join("campaigns")).unwrap().count(),
            0
        );
        let helpers = driver.helpers.get_mut();
        assert_eq!(helpers.len(), 2);
        assert_eq!(helpers[0].helper_output.as_ptr(), pointer);
        assert!(helpers.iter().all(ChildProc::physical_supervision_complete));
        assert!(
            helpers
                .iter()
                .all(|helper| helper.capture_operation().same_instance(&original))
        );
        assert!(helpers.iter().all(|helper| helper.queued_frames() > 0));
        assert_eq!(
            super::super::archive_stdout::unique_storage(&mut helpers[1].reader_error)
                .unwrap()
                .first_kind(),
            Some(std::io::ErrorKind::BrokenPipe)
        );
        assert_eq!(original.captured_bytes(), charged);
        assert!(driver.stop().await.is_err());
        assert_eq!(driver.helpers.get_mut().len(), 2);
        assert!(original.borrow_archive_chunk(&original).is_ok());
        assert!(sink.stop_delivery().await.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn file_observation_cannot_discard_unhanded_accounting_in_parser_suffix() {
        let root = custody_fixture_root();
        let original = archive_operation_at(&root);
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (mut child, _) = joined_cleanup_child_with_operation(original.clone()).await;
        original.charge(512).unwrap(); // Synthetic retained-value admission only.
        child.pending_events = Some(super::super::RetainedEvents {
            raw: super::super::ChildFrame {
                text: "original result raw".into(),
                origin: None,
                operation: original.clone(),
            },
            events: std::collections::VecDeque::from([
                StreamEvent::TurnDone(TurnOutcome {
                    ok: true,
                    error: None,
                    text: Some("captured CLI ok".into()),
                    usage: super::super::Usage::default(),
                    quota: Vec::new(),
                }),
                StreamEvent::Quota(super::super::QuotaReading {
                    window: "original measured window".into(),
                    utilization: 0.5,
                    resets_at_epoch_s: None,
                }),
            ]),
        });
        let pointer = child.pending_events.as_ref().unwrap().raw.text.as_ptr();
        let mut driver = ClaudeDriver::new(argument_config(), sink.clone());
        driver.proc = Some(child);
        let observed = driver.publish_original_archive(&original).unwrap();
        let charged = original.captured_bytes();
        assert!(
            driver
                .accept_original_archive(&original, &observed)
                .is_err()
        );
        let pending = driver
            .proc
            .as_ref()
            .unwrap()
            .pending_events
            .as_ref()
            .unwrap();
        assert_eq!(pending.events.len(), 2);
        assert_eq!(pending.raw.text.as_ptr(), pointer);
        assert!(pending.raw.operation.same_instance(&original));
        assert!(
            matches!(pending.events.front(), Some(StreamEvent::TurnDone(outcome)) if outcome.ok)
        );
        assert!(driver.completed_turn.is_none()); // No invented captured-outcome handoff.
        assert_eq!(original.captured_bytes(), charged);
        assert!(original.borrow_archive_chunk(&original).is_ok());
        assert!(sink.stop_delivery().await.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    // SOURCE ONLY/UNRUN: future separately admitted inert-child/filesystem fixture.
    #[tokio::test]
    async fn actual_driver_publication_keeps_original_evidence_after_file_observation() {
        use crate::failed_evidence::{ArchiveMeter, JoinedArchiveSource};
        let root =
            std::env::temp_dir().join(format!("cna-driver-archive-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        for child in ["campaigns", "sandbox", "run"] {
            std::fs::create_dir(root.join(child)).unwrap();
        }
        let original = archive_operation_at(&root);
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (child, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let mut driver = ClaudeDriver::new(argument_config(), sink.clone());
        driver.proc = Some(child);
        driver.parser.feed_owned(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"canonical failed evidence"}]}}"#,
            &original,
        ).unwrap();
        let pointer = driver.parser.last_text.as_ref().unwrap().as_ptr();
        let coverage = driver.original_archive_coverage(&original).unwrap();
        let mut meter = ArchiveMeter::new();
        let expected = driver.stream(&original, coverage, &mut meter).unwrap();
        assert_eq!(meter.finish().unwrap(), expected);
        let _ = original.charge(crate::operation::OPERATION_BYTES + 1);
        let charged = original.captured_bytes();
        let observation = driver.publish_original_archive(&original).unwrap();
        assert!(observation.matches(&original, coverage, expected));
        assert!(!observation.unaccepted_temp_possible());
        let _flush_warning = observation.best_effort_sync_failed();
        assert_eq!(driver.parser.last_text.as_ref().unwrap().as_ptr(), pointer);
        let plan = original.archive_plan().unwrap();
        let mut file = std::fs::File::open(&plan.final_path).unwrap();
        plan.verify_stream(&original, coverage, expected, &mut file)
            .unwrap();
        drop(file);
        drop(observation); // Lost ACK retains the original source and attempt.
        assert!(
            driver
                .publish_original_archive(&original)
                .unwrap()
                .matches(&original, coverage, expected)
        );
        assert_eq!(driver.parser.last_text.as_ref().unwrap().as_ptr(), pointer);
        assert!(
            driver
                .proc
                .as_ref()
                .unwrap()
                .physical_supervision_complete()
        );
        assert!(driver.proc.as_ref().unwrap().queued_frames() > 0);
        assert!(driver.stop().await.is_err()); // File observation is not custody or prompt ACK.
        assert_eq!(original.captured_bytes(), charged);
        assert!(original.borrow_archive_chunk(&original).is_ok());
        assert!(sink.stop_delivery().await.is_empty());
        // Only this isolated synthetic fixture is removed; production never deletes leftovers.
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn original_archive_traverses_helpers_session_and_parser_without_consuming() {
        use crate::failed_evidence::JoinedArchiveSource;
        let original = archive_startup_operation();
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (mut helper, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let (mut session, _) = joined_cleanup_child_with_operation(original.clone()).await;
        original.charge(64).unwrap(); // Synthetic existing-domain body admission.
        helper.helper_output = "accepted original helper".into();
        session.pending_frame = Some(super::super::ChildFrame {
            operation: original.clone(),
            origin: None,
            text: "retained session raw".into(),
        });
        let pointer = session.pending_frame.as_ref().unwrap().text.as_ptr();
        let mut driver = ClaudeDriver::new(argument_config(), sink.clone());
        driver.helpers.get_mut().push(helper);
        driver.proc = Some(session);
        driver.parser.feed_owned(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"fallback original"}]}}"#,
            &original,
        ).unwrap();
        let fallback = driver.parser.last_text.as_ref().unwrap().as_ptr();
        let coverage = driver.original_archive_coverage(&original).unwrap();
        let _ = original.charge(crate::operation::OPERATION_BYTES + 1);
        let meter = original.captured_bytes();
        let mut bytes = Vec::new(); // Test output only, never a production archive buffer.
        let measurement = driver.stream(&original, coverage, &mut bytes).unwrap();
        original
            .archive_plan()
            .unwrap()
            .verify_stream(&original, coverage, measurement, &mut bytes.as_slice())
            .unwrap();
        let encoded = String::from_utf8(bytes).unwrap();
        assert!(encoded.contains("accepted_helper_output"));
        assert!(encoded.contains("pending_frame"));
        assert!(encoded.contains("stderr_line"));
        assert!(encoded.contains("parser_fallback"));
        assert_eq!(encoded.matches("child_retained_metadata").count(), 2);
        assert_eq!(
            driver
                .proc
                .as_ref()
                .unwrap()
                .pending_frame
                .as_ref()
                .unwrap()
                .text
                .as_ptr(),
            pointer
        );
        assert_eq!(driver.parser.last_text.as_ref().unwrap().as_ptr(), fallback);
        assert_eq!(driver.helpers.get_mut().len(), 1);
        assert_eq!(original.captured_bytes(), meter);
        assert!(original.borrow_archive_chunk(&original).is_ok()); // Still owned, no settlement.
        assert!(driver.stop().await.is_err()); // Streaming is no custody/healthy ACK.
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn original_archive_refuses_unjoined_stale_and_captured_outcome_before_output() {
        use crate::failed_evidence::JoinedArchiveSource;
        let original = archive_startup_operation();
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (helper, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let (child, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let mut driver = ClaudeDriver::new(argument_config(), sink.clone());
        driver.host = Some(recovery_context(driver.cfg.seat, original.clone()));
        driver.helpers.get_mut().push(helper);
        driver.proc = Some(child);
        let coverage = driver.original_archive_coverage(&original).unwrap();
        let mut output = Vec::new();
        assert!(
            driver
                .stream(&archive_startup_operation(), coverage, &mut output)
                .is_err()
        );
        assert!(output.is_empty()); // Same textual identity cannot replace the instance.
        let (release, waiting) = tokio::sync::oneshot::channel::<()>();
        driver
            .proc
            .as_mut()
            .unwrap()
            .readers
            .push(tokio::spawn(async move {
                let _ = waiting.await;
                Ok(())
            }));
        assert!(driver.check_original_supervision(&original).is_err());
        let mut before_join =
            crate::failed_evidence::EvidenceCoverageBuilder::new(&original).unwrap();
        assert!(
            driver
                .visit_original_archive(&original, &mut before_join)
                .is_err()
        );
        assert!(before_join.finish().is_err()); // Not even earlier helper coverage was enumerated.
        assert!(driver.stream(&original, coverage, &mut output).is_err());
        assert!(output.is_empty());
        release.send(()).unwrap();
        driver
            .proc
            .as_mut()
            .unwrap()
            .join_readers(original.allowance().deadline())
            .await
            .unwrap();
        // Synthetic parser ownership, not a live correlated turn witness.
        driver.parser.runtime_operation = Some(original.clone());
        driver.completed_turn = Some(TurnOutcome {
            ok: true,
            error: None,
            text: Some("captured usage evidence".into()),
            usage: Usage::default(),
            quota: Vec::new(),
        });
        let pointer = driver
            .completed_turn
            .as_ref()
            .unwrap()
            .text
            .as_ref()
            .unwrap()
            .as_ptr();
        assert!(driver.stream(&original, coverage, &mut output).is_err());
        assert!(output.is_empty());
        assert_eq!(
            driver
                .completed_turn
                .as_ref()
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .as_ptr(),
            pointer
        );
        let outcome = driver.take_captured_outcome(&original).unwrap().unwrap();
        assert_eq!(outcome.text.as_ref().unwrap().as_ptr(), pointer); // Separate synchronous MOVE.
        driver.stream(&original, coverage, &mut output).unwrap();
        assert!(!output.is_empty());
        assert!(original.borrow_archive_chunk(&original).is_ok());
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn original_archive_late_opaque_panic_refuses_before_header_or_body() {
        use crate::failed_evidence::JoinedArchiveSource;
        let original = archive_startup_operation();
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (helper, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let (session, _) = joined_cleanup_child_with_operation(original.clone()).await;
        let mut driver = ClaudeDriver::new(argument_config(), sink.clone());
        driver.helpers.get_mut().push(helper);
        driver.proc = Some(session);
        let coverage = driver.original_archive_coverage(&original).unwrap();
        driver
            .proc
            .as_mut()
            .unwrap()
            .readers
            .push(tokio::spawn(async {
                std::panic::panic_any(vec![9_u8; 16]);
                #[allow(unreachable_code)]
                Ok(())
            }));
        assert!(
            driver
                .proc
                .as_mut()
                .unwrap()
                .join_readers(original.allowance().deadline())
                .await
                .is_err()
        );
        assert!(
            driver
                .proc
                .as_ref()
                .unwrap()
                .physical_supervision_complete()
        );
        let mut output = Vec::new();
        assert!(driver.stream(&original, coverage, &mut output).is_err());
        assert!(output.is_empty()); // Valid earlier helper cannot produce a prefix.
        assert!(
            driver
                .proc
                .as_ref()
                .unwrap()
                .reader_error
                .lock()
                .unwrap()
                .has_failure()
        );
        assert_eq!(driver.helpers.get_mut().len(), 1);
        assert!(original.borrow_archive_chunk(&original).is_ok());
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn stop_attempts_all_original_children_before_retained_failure_refusal() {
        use super::super::reader_failure::ReaderStream;
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (mut earlier, earlier_operation) = joined_cleanup_child().await;
        let (mut failing, failing_operation) = joined_cleanup_child().await;
        let (mut session, session_operation) = joined_cleanup_child().await;
        let physical = [
            earlier.physical_child_id().to_owned(),
            failing.physical_child_id().to_owned(),
            session.physical_child_id().to_owned(),
        ];
        let meters = [
            earlier_operation.captured_bytes(),
            failing_operation.captured_bytes(),
            session_operation.captured_bytes(),
        ];
        // Finished is not joined: the actual original handles must be awaited.
        earlier.readers.push(tokio::spawn(async { Ok(()) }));
        session.readers.push(tokio::spawn(async { Ok(()) }));
        failing
            .reader_error
            .lock()
            .unwrap()
            .retain_io(ReaderStream::Stdout, std::io::ErrorKind::InvalidData.into())
            .unwrap();
        let mut driver = cleanup_driver(sink.clone());
        driver.helpers.get_mut().extend([earlier, failing]);
        driver.proc = Some(session);
        driver.idle = false;
        assert!(driver.stop().await.is_err());
        let helpers = driver.helpers.get_mut();
        assert_eq!(helpers.len(), 2); // No healthy-owner pop before the first failure.
        for (index, child) in helpers.iter().enumerate() {
            assert!(child.physical_supervision_complete());
            assert_eq!(child.physical_child_id(), physical[index]);
            assert!(child.queued_frames() > 0);
        }
        assert_eq!(
            helpers[1].reader_error.lock().unwrap().first_kind(),
            Some(std::io::ErrorKind::InvalidData)
        );
        let session = driver.proc.as_ref().unwrap();
        assert!(session.physical_supervision_complete());
        assert_eq!(session.physical_child_id(), physical[2]);
        assert!(
            session
                .capture_operation()
                .same_instance(&session_operation)
        );
        assert_eq!(earlier_operation.captured_bytes(), meters[0]);
        assert_eq!(failing_operation.captured_bytes(), meters[1]);
        assert_eq!(session_operation.captured_bytes(), meters[2]);
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn canceled_all_child_supervision_keeps_original_wait_and_other_owners() {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (earlier, earlier_operation) = joined_cleanup_child().await;
        let (mut waiting, waiting_operation) = joined_cleanup_child().await;
        let (session, session_operation) = joined_cleanup_child().await;
        let original_deadline = waiting_operation.allowance().deadline();
        let (release, receiver) = tokio::sync::oneshot::channel::<()>();
        waiting.readers.push(tokio::spawn(async move {
            let _ = receiver.await;
            Ok(())
        }));
        let mut driver = cleanup_driver(sink.clone());
        driver.helpers.get_mut().extend([earlier, waiting]);
        driver.proc = Some(session);
        let mut joining = Box::pin(driver.supervise_retained_children());
        assert!(matches!(
            joining
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        drop(joining); // No task/child abort or archive/prompt custody from borrow cancellation.
        let helpers = driver.helpers.get_mut();
        assert_eq!(helpers.len(), 2);
        assert_eq!(helpers[1].readers.len(), 1);
        assert!(!helpers[1].physical_supervision_complete());
        assert!(
            helpers[0]
                .capture_operation()
                .same_instance(&earlier_operation)
        );
        assert!(
            helpers[1]
                .capture_operation()
                .same_instance(&waiting_operation)
        );
        assert_eq!(helpers[1].cleanup_deadline, Some(original_deadline));
        assert!(
            driver
                .proc
                .as_ref()
                .unwrap()
                .capture_operation()
                .same_instance(&session_operation)
        );
        release.send(()).unwrap();
        driver.supervise_retained_children().await.unwrap();
        assert!(
            driver
                .helpers
                .get_mut()
                .iter()
                .all(ChildProc::physical_supervision_complete)
        );
        assert!(
            driver
                .proc
                .as_ref()
                .unwrap()
                .physical_supervision_complete()
        );
        assert_eq!(
            driver.helpers.get_mut()[1].cleanup_deadline,
            Some(original_deadline)
        );
        assert!(driver.stop().await.is_err()); // Physical joins do not settle retained diagnostics.
        assert_eq!(driver.helpers.get_mut().len(), 2);
        assert!(driver.proc.is_some());
        assert!(sink.stop_delivery().await.is_empty());
    }

    impl crate::host::HostAdmission for RecoveryClosedAdmission {
        fn subscribe(&self) -> tokio::sync::watch::Receiver<crate::host::HostSnapshot> {
            tokio::sync::watch::channel(crate::host::HostSnapshot {
                generation: 0,
                activated: false,
                status: crate::host::HostStatus::Unknown,
                epochs: std::collections::BTreeMap::new(),
                pending: std::collections::BTreeMap::new(),
            })
            .1
        }
        fn snapshot(&self) -> Result<crate::host::HostSnapshot, crate::host::HostRefusal> {
            Err(crate::host::HostRefusal::Unknown)
        }
        fn acquire(
            &self,
            _: &crate::host::HostIdentity,
            _: crate::host::HostStep,
        ) -> Result<Box<dyn crate::host::HostStepLease>, crate::host::HostRefusal> {
            Err(crate::host::HostRefusal::Unknown)
        }
        fn acquire_prompt(
            &self,
            _: &crate::host::HostIdentity,
            _: crate::operation::Allowance,
        ) -> Result<Box<dyn crate::host::HostPromptLease>, crate::host::HostRefusal> {
            Err(crate::host::HostRefusal::Unknown)
        }
    }

    fn recovery_context(
        seat: cna_core::ids::SeatId,
        operation: crate::operation::Operation,
    ) -> crate::host::HostDriverContext {
        crate::host::HostDriverContext {
            // This fixture can never authorize a native effect. Only original instance
            // association is exercised by the synchronous recovery move.
            admission: Arc::new(RecoveryClosedAdmission),
            identity: crate::host::HostIdentity {
                seat,
                epoch: 7,
                decision_id: "original".into(),
                revision: 1,
                operation_id: operation.id().into(),
            },
            operation,
        }
    }

    #[tokio::test]
    async fn captured_outcome_move_requires_original_instance_and_releases_no_supervision() {
        use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (child, operation) = joined_cleanup_child().await;
        let child_id = child.physical_child_id().to_owned();
        let mut driver = cleanup_driver(sink.clone());
        driver.host = Some(recovery_context(driver.cfg.seat, operation.clone()));
        driver.proc = Some(child);
        driver.idle = false;
        driver.completed_turn = Some(TurnOutcome {
            ok: true,
            error: None,
            text: Some("canonical captured output".into()),
            usage: super::super::Usage {
                output_tokens: Some(5),
                ..Default::default()
            },
            quota: Vec::new(),
        });
        let text_pointer = driver
            .completed_turn
            .as_ref()
            .unwrap()
            .text
            .as_ref()
            .unwrap()
            .as_ptr();
        driver.emitter = Some(EntryEmitter::new(
            driver.cfg.seat,
            sink.for_operation(operation.clone()),
        ));
        let requests = Arc::new(AtomicUsize::new(0));
        let phase = Arc::new(AtomicU8::new(1));
        driver.prompt_lease = Some(Box::new(CleanupPromptLease {
            phase: phase.clone(),
            requests: requests.clone(),
        }));
        let same_text_id =
            crate::operation::Operation::new(operation.id().into(), 7, operation.allowance())
                .unwrap();
        // SOURCE controls added for the actual public journal capture adapter;
        // UNCOMPILED/UNRUN. Host/child already match, so absent/stale parser
        // and a foreign saved helper independently block the original MOVE.
        assert!(matches!(
            driver.take_captured_outcome(&operation),
            Err(DriverError::Host(crate::host::HostRefusal::Unknown))
        ));
        assert!(driver.completed_turn.is_some() && driver.emitter.is_some());
        driver.parser.runtime_operation = Some(same_text_id.clone());
        assert!(matches!(
            driver.take_captured_outcome(&operation),
            Err(DriverError::Host(crate::host::HostRefusal::Unknown))
        ));
        assert_eq!(
            driver
                .completed_turn
                .as_ref()
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .as_ptr(),
            text_pointer
        );
        driver.parser.runtime_operation = Some(operation.clone());
        let (foreign_helper, helper_original) = joined_cleanup_child().await;
        assert!(!helper_original.same_instance(&operation));
        assert!(foreign_helper.physical_supervision_complete()); // Actual fixture join when admitted.
        driver.helpers.get_mut().push(foreign_helper);
        assert!(matches!(
            driver.take_captured_outcome(&operation),
            Err(DriverError::Host(crate::host::HostRefusal::Unknown))
        ));
        assert_eq!(driver.helpers.get_mut().len(), 1);
        assert_eq!(
            driver
                .completed_turn
                .as_ref()
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .as_ptr(),
            text_pointer
        );
        // Remove ONLY this independently joined synthetic foreign fixture.
        // This is not production archive custody or discard permission.
        let returned_fixture = driver.helpers.get_mut().pop().unwrap();
        assert!(
            returned_fixture
                .capture_operation()
                .same_instance(&helper_original)
        );
        drop(returned_fixture);
        assert!(driver.take_captured_outcome(&same_text_id).is_err());
        assert!(driver.completed_turn.is_some());
        assert!(driver.emitter.is_some());
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        // Cleanup moves remain reachable even when dispatch has closed through overflow.
        assert!(
            operation
                .charge(crate::operation::OPERATION_BYTES + 1)
                .is_err()
        );
        let evidence = operation.start_evidence();
        let bytes = operation.captured_bytes();
        let captured = driver.take_captured_outcome(&operation).unwrap().unwrap();
        assert!(captured.ok);
        assert_eq!(captured.text.as_ref().unwrap().as_ptr(), text_pointer);
        assert!(driver.completed_turn.is_none());
        assert!(driver.emitter.is_none()); // Empty accepted pairing handoff only.
        assert!(driver.prompt_lease.is_some());
        let retained = driver.proc.as_ref().unwrap();
        assert_eq!(retained.physical_child_id(), child_id);
        assert!(retained.capture_operation().same_instance(&operation));
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        assert_eq!(operation.captured_bytes(), bytes);
        assert_eq!(operation.start_evidence(), evidence);
        assert!(driver.take_captured_outcome(&operation).unwrap().is_none());
        assert!(driver.stop().await.is_err()); // Captured MOVE retains parser ownership too.
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        assert!(
            driver
                .parser
                .runtime_operation
                .as_ref()
                .unwrap()
                .same_instance(&operation)
        );
        // Explicit synthetic parser-owner consumption so the pre-existing actor
        // ACK fixture can continue. Not a production archive/custody settlement.
        let parser_original = driver.parser.runtime_operation.take().unwrap();
        assert!(parser_original.same_instance(&operation));
        assert!(driver.stop().await.is_err()); // Explicit failed actor ACK still retains child.
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(driver.proc.as_ref().unwrap().physical_child_id(), child_id);
        assert!(driver.prompt_lease.is_some());
        phase.store(2, Ordering::SeqCst); // Explicit fixture ACK, independent of the move.
        driver.stop().await.unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert!(driver.proc.is_none());
        assert!(driver.prompt_lease.is_none());
        assert!(sink.stop_delivery().await.is_empty()); // Actual fixture worker join.
    }

    #[tokio::test]
    async fn captured_move_keeps_unpaired_emitter_and_missing_child_refuses_move() {
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (child, operation) = joined_cleanup_child().await;
        let mut driver = cleanup_driver(sink.clone());
        driver.host = Some(recovery_context(driver.cfg.seat, operation.clone()));
        driver.completed_turn = Some(TurnOutcome {
            ok: true,
            error: None,
            text: None,
            usage: Default::default(),
            quota: Vec::new(),
        });
        assert!(driver.take_captured_outcome(&operation).is_err());
        assert!(driver.completed_turn.is_some());
        driver.proc = Some(child);
        driver.parser.runtime_operation = Some(operation.clone()); // Synthetic association only.
        let mut emitter = EntryEmitter::new(driver.cfg.seat, sink.for_operation(operation.clone()));
        emitter
            .try_emit(TranscriptEntry::ToolCall {
                call_id: "original-call".into(),
                tool: "submit".into(),
                args: serde_json::json!({"decision_id":"original", "action":null}),
            })
            .unwrap();
        assert!(emitter.has_pending_calls());
        driver.emitter = Some(emitter);
        assert!(
            driver
                .take_captured_outcome(&operation)
                .unwrap()
                .unwrap()
                .ok
        );
        assert!(driver.emitter.as_ref().unwrap().has_pending_calls());
        assert!(driver.stop().await.is_err());
        assert!(driver.proc.is_some());
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn stop_retains_joined_child_until_captured_outcome_and_emitter_are_handed_off() {
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (child, operation) = joined_cleanup_child().await;
        let child_id = child.physical_child_id().to_owned();
        let mut driver = cleanup_driver(sink.clone());
        driver.proc = Some(child);
        driver.idle = false;
        driver.completed_turn = Some(TurnOutcome {
            ok: false,
            error: Some("original terminal evidence".into()),
            text: Some("accepted assistant output".into()),
            usage: super::super::Usage::default(),
            quota: Vec::new(),
        });
        driver.emitter = Some(EntryEmitter::new(
            driver.cfg.seat,
            sink.for_operation(operation.clone()),
        ));
        assert!(driver.stop().await.is_err());
        let retained = driver.proc.as_mut().unwrap();
        assert_eq!(retained.physical_child_id(), child_id);
        assert!(!retained.is_running());
        assert!(retained.capture_operation().same_instance(&operation));
        assert!(
            retained
                .stderr_tail()
                .contains("original joined-child diagnostic")
        );
        assert_eq!(
            driver.completed_turn.as_ref().unwrap().error.as_deref(),
            Some("original terminal evidence")
        );
        // Explicit fixture consumption is not a production journal handoff or a timer ACK.
        let outcome = driver.completed_turn.take().unwrap();
        assert_eq!(outcome.text.as_deref(), Some("accepted assistant output"));
        assert!(driver.stop().await.is_err()); // Empty emitter still has an owned disposition.
        assert_eq!(driver.proc.as_ref().unwrap().physical_child_id(), child_id);
        assert!(!driver.emitter.as_ref().unwrap().has_pending_calls());
        driver.emitter.take();
        driver.stop().await.unwrap();
        assert!(driver.proc.is_none());
        assert!(sink.stop_delivery().await.is_empty());
    }

    struct CleanupPromptLease {
        phase: Arc<std::sync::atomic::AtomicU8>,
        requests: Arc<std::sync::atomic::AtomicUsize>,
    }

    struct BoundaryPromptLease {
        released: Arc<tokio::sync::Notify>,
        requests: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl crate::host::HostPromptLease for BoundaryPromptLease {
        fn check(&self) -> Result<(), crate::host::HostRefusal> {
            Ok(())
        }
        async fn settle(&self) -> Result<(), crate::host::HostRefusal> {
            self.requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.released.notified().await;
            Ok(()) // Explicit synthetic actor ACK, never a pipe/custody witness.
        }
    }

    #[tokio::test]
    async fn joined_raw_evidence_keeps_same_driver_prompt_liability_and_blocks_replacement() {
        use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (mut child, operation) = joined_cleanup_child().await;
        let physical = child.physical_child_id().to_owned();
        assert!(child.physical_supervision_complete());
        operation.possible_start();
        let text = "original undecoded frame";
        operation
            .charge(text.len() + std::mem::size_of::<super::super::ChildFrame>())
            .unwrap();
        child.pending_frame = Some(super::super::ChildFrame {
            text: text.into(),
            origin: None,
            operation: operation.clone(),
        });
        let canonical = child.pending_frame.as_ref().unwrap().text.as_ptr();
        let bytes = operation.captured_bytes();
        let deadline = operation.allowance().deadline();
        let requests = Arc::new(AtomicUsize::new(0));
        let mut driver = cleanup_driver(sink.clone());
        driver.host = Some(recovery_context(driver.cfg.seat, operation.clone()));
        driver.proc = Some(child);
        driver.prompt_lease = Some(Box::new(CleanupPromptLease {
            // Even an ACK-capable actor cannot be asked to free raw evidence.
            phase: Arc::new(AtomicU8::new(2)),
            requests: requests.clone(),
        }));
        for _ in 0..2 {
            assert!(matches!(driver.stop().await,
                Err(DriverError::Host(crate::host::HostRefusal::Ownership(ref reason)))
                    if reason.contains("Unsettled")));
            let retained = driver.proc.as_ref().unwrap();
            assert!(retained.physical_supervision_complete());
            assert_eq!(retained.physical_child_id(), physical);
            let raw = retained.pending_frame.as_ref().unwrap();
            assert_eq!(raw.text.as_ptr(), canonical);
            assert!(raw.operation.same_instance(&operation));
            assert!(raw.origin.is_none());
            assert!(driver.prompt_lease.is_some());
            assert_eq!(requests.load(Ordering::SeqCst), 0);
            assert_eq!(operation.captured_bytes(), bytes);
            assert_eq!(operation.allowance().deadline(), deadline);
            assert_eq!(
                operation.start_evidence(),
                crate::operation::StartEvidence::Possible
            );
        }
        assert!(driver.start(None).await.is_err());
        assert!(
            driver
                .set_host_context(recovery_context(driver.cfg.seat, metadata_operation()))
                .is_err()
        );
        assert_eq!(
            driver
                .proc
                .as_ref()
                .unwrap()
                .pending_frame
                .as_ref()
                .unwrap()
                .text
                .as_ptr(),
            canonical
        );
        assert!(sink.stop_delivery().await.is_empty()); // No raw->normal transcript conversion.
        // This fixture ends here; no production evidence archive/discard/healthy ACK is added.
    }

    #[tokio::test]
    async fn physical_completion_requires_actual_reader_join_not_finished_handle_or_canceled_borrow()
     {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let (mut child, operation) = joined_cleanup_child().await;
        let physical = child.physical_child_id().to_owned();
        let (release, waiting) = tokio::sync::oneshot::channel::<()>();
        child.readers.push(tokio::spawn(async move {
            let _ = waiting.await;
            Ok(())
        }));
        assert!(!child.physical_supervision_complete());
        let mut joining = Box::pin(child.join_readers(operation.allowance().deadline()));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(joining.as_mut().poll(&mut context), Poll::Pending));
        drop(joining);
        assert!(!child.physical_supervision_complete());
        assert_eq!(child.readers.len(), 1);
        release.send(()).unwrap();
        child
            .join_readers(operation.allowance().deadline())
            .await
            .unwrap();
        assert!(child.physical_supervision_complete());
        assert_eq!(child.physical_child_id(), physical);
        assert!(child.capture_operation().same_instance(&operation));
        assert!(
            child
                .stderr_tail()
                .contains("original joined-child diagnostic")
        );
    }

    #[tokio::test]
    async fn pending_stderr_does_not_hide_finished_stdout_or_repoll_its_disposition() {
        use super::super::reader_failure::ReaderStream;
        let (mut child, original) = joined_cleanup_child().await;
        let physical = child.physical_child_id().to_owned();
        let meter = original.captured_bytes();
        let deadline = original.allowance().deadline();
        let (stdout_release, stdout_wait) = tokio::sync::oneshot::channel::<()>();
        let (stderr_release, stderr_wait) = tokio::sync::oneshot::channel::<()>();
        child.readers.extend([
            tokio::spawn(async move {
                let _ = stdout_wait.await;
                Err(std::io::ErrorKind::InvalidData.into())
            }),
            tokio::spawn(async move {
                let _ = stderr_wait.await;
                Ok(())
            }),
        ]);
        let handles = [child.readers[0].id(), child.readers[1].id()];
        stdout_release.send(()).unwrap();
        while !child.readers[0].is_finished() {
            assert!(Instant::now() < deadline);
            tokio::task::yield_now().await;
        }
        // Expiry must still observe the actual finished stdout result, but must
        // neither manufacture stderr completion nor abort its original task.
        assert_eq!(
            child.join_readers(Instant::now()).await.unwrap_err().kind(),
            std::io::ErrorKind::TimedOut
        );
        assert_eq!(child.reader_joined, [true, false]);
        assert_eq!(child.readers.len(), 2);
        assert_eq!([child.readers[0].id(), child.readers[1].id()], handles);
        assert!(!child.physical_supervision_complete());
        assert!(
            child
                .reader_error
                .lock()
                .unwrap()
                .has_io(ReaderStream::Stdout)
        );
        assert_eq!(
            child.reader_error.lock().unwrap().first_kind(),
            Some(std::io::ErrorKind::InvalidData)
        );
        // Repeated expired waits skip the already-consumed stdout future.
        assert_eq!(
            child.join_readers(Instant::now()).await.unwrap_err().kind(),
            std::io::ErrorKind::TimedOut
        );
        stderr_release.send(()).unwrap();
        assert_eq!(
            child.join_readers(deadline).await.unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert!(child.physical_supervision_complete());
        assert!(child.readers.is_empty());
        assert_eq!(child.reader_joined, [false, false]);
        assert_eq!(child.physical_child_id(), physical);
        assert!(child.capture_operation().same_instance(&original));
        assert_eq!(original.captured_bytes(), meter);
        assert_eq!(original.allowance().deadline(), deadline);
        assert!(child.queued_frames() > 0); // Canonical failure/diagnostics, no healthy release.
        assert_eq!(
            child.join_readers(deadline).await.unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[tokio::test]
    async fn canceled_two_reader_wait_retains_both_handles_and_exact_stream_indices() {
        use super::super::reader_failure::ReaderStream;
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let (mut child, original) = joined_cleanup_child().await;
        let (stdout_release, stdout_wait) = tokio::sync::oneshot::channel::<()>();
        let (stderr_release, stderr_wait) = tokio::sync::oneshot::channel::<()>();
        child.readers.extend([
            tokio::spawn(async move {
                let _ = stdout_wait.await;
                Ok(())
            }),
            tokio::spawn(async move {
                let _ = stderr_wait.await;
                Err(std::io::ErrorKind::PermissionDenied.into())
            }),
        ]);
        let handles = [child.readers[0].id(), child.readers[1].id()];
        let mut joining = Box::pin(child.join_readers(original.allowance().deadline()));
        assert!(matches!(
            joining
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        drop(joining);
        assert_eq!(child.reader_joined, [false, false]);
        assert_eq!([child.readers[0].id(), child.readers[1].id()], handles);
        assert!(!child.physical_supervision_complete());
        stderr_release.send(()).unwrap();
        while !child.readers[1].is_finished() {
            assert!(Instant::now() < original.allowance().deadline());
            tokio::task::yield_now().await;
        }
        assert!(child.join_readers(Instant::now()).await.is_err());
        assert_eq!(child.readers.len(), 1);
        assert_eq!(child.readers[0].id(), handles[0]); // Stdout never slides into stderr's slot.
        assert!(
            child
                .reader_error
                .lock()
                .unwrap()
                .has_io(ReaderStream::Stderr)
        );
        assert!(
            !child
                .reader_error
                .lock()
                .unwrap()
                .has_io(ReaderStream::Stdout)
        );
        stdout_release.send(()).unwrap();
        assert_eq!(
            child
                .join_readers(original.allowance().deadline())
                .await
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(child.physical_supervision_complete());
        assert!(child.capture_operation().same_instance(&original));
        assert!(child.queued_frames() > 0);
    }

    #[tokio::test]
    async fn retained_reader_failure_archive_waits_for_actual_join_and_keeps_original_cause() {
        use crate::failed_evidence::{ArchiveEncoder, EvidenceCoverageBuilder};
        // Existing inert child fixture; this control is source-only until a genuine slot.
        let (mut child, original) =
            joined_cleanup_child_with_operation(archive_startup_operation()).await;
        let error = std::io::Error::from(std::io::ErrorKind::BrokenPipe);
        child
            .reader_error
            .lock()
            .unwrap()
            .retain_io(super::super::reader_failure::ReaderStream::Stdout, error)
            .unwrap();
        child
            .readers
            .push(tokio::spawn(std::future::pending::<std::io::Result<()>>()));
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        assert!(
            child
                .cover_reader_failures_archive(&original, &mut coverage)
                .is_err()
        );
        assert!(!child.physical_supervision_complete());
        child.readers.last().unwrap().abort();
        // Original task join reports the original first pipe cause, while also
        // retaining the canceled-reader outcome independently in the SAME owner.
        assert_eq!(
            child
                .join_readers(original.allowance().deadline())
                .await
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert!(child.physical_supervision_complete());
        let replacement = archive_startup_operation();
        assert!(child.bind_operation(replacement).is_err());
        assert!(child.capture_operation().same_instance(&original));
        let mut coverage = EvidenceCoverageBuilder::new(&original).unwrap();
        child
            .cover_reader_failures_archive(&original, &mut coverage)
            .unwrap();
        let coverage = coverage.finish().unwrap();
        let mut output = Vec::new();
        let mut encoder = ArchiveEncoder::new(&original, coverage, &mut output).unwrap();
        child
            .encode_reader_failures_archive(&original, &mut encoder)
            .unwrap();
        encoder.finish().unwrap();
        assert!(child.reader_error.lock().unwrap().has_failure());
        assert!(child.capture_operation().same_instance(&original));
    }

    #[async_trait]
    impl crate::host::HostPromptLease for CleanupPromptLease {
        fn check(&self) -> Result<(), crate::host::HostRefusal> {
            Ok(())
        }
        async fn settle(&self) -> Result<(), crate::host::HostRefusal> {
            use std::sync::atomic::Ordering;
            self.requests.fetch_add(1, Ordering::SeqCst);
            match self.phase.load(Ordering::SeqCst) {
                0 => std::future::pending().await,
                1 => Err(crate::host::HostRefusal::Ownership(
                    "original ACK unavailable".into(),
                )),
                2 => Ok(()), // Explicit fixture ACK AFTER actual joined_cleanup_child boundary.
                _ => unreachable!(),
            }
        }
    }

    #[tokio::test]
    async fn stop_canceled_or_failed_prompt_ack_keeps_same_joined_child_and_lease() {
        use std::{
            future::Future,
            sync::atomic::{AtomicU8, AtomicUsize, Ordering},
            task::{Context, Poll, Waker},
        };
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let (child, operation) = joined_cleanup_child().await;
        let child_id = child.physical_child_id().to_owned();
        let mut driver = cleanup_driver(sink.clone());
        driver.proc = Some(child);
        driver.idle = false;
        let phase = Arc::new(AtomicU8::new(0));
        let requests = Arc::new(AtomicUsize::new(0));
        driver.prompt_lease = Some(Box::new(CleanupPromptLease {
            phase: phase.clone(),
            requests: requests.clone(),
        }));
        let mut stopping = Box::pin(driver.stop());
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            stopping.as_mut().poll(&mut context),
            Poll::Pending
        ));
        drop(stopping); // Cancels a borrow, not the original child/lease or actor request.
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(driver.proc.as_ref().unwrap().physical_child_id(), child_id);
        assert!(driver.prompt_lease.is_some());
        phase.store(1, Ordering::SeqCst);
        assert!(driver.stop().await.is_err());
        let retained = driver.proc.as_ref().unwrap();
        assert_eq!(retained.physical_child_id(), child_id);
        assert!(retained.capture_operation().same_instance(&operation));
        assert!(
            retained
                .stderr_tail()
                .contains("original joined-child diagnostic")
        );
        assert!(driver.prompt_lease.is_some());
        phase.store(2, Ordering::SeqCst);
        driver.stop().await.unwrap();
        assert!(driver.proc.is_none());
        assert!(driver.prompt_lease.is_none());
        assert_eq!(requests.load(Ordering::SeqCst), 3);
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
        assert!(sink.stop_delivery().await.is_empty());
    }

    #[tokio::test]
    async fn preflight_owner_pending_cancellation_and_expired_admission_keep_original_borrow() {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let worker_deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        let sink = TranscriptSink::new(crate::transcript::LocalTranscript::detached());
        let driver = ClaudeDriver::new(
            ClaudeConfig {
                seat: "axis.commander".parse().unwrap(),
                exe: None,
                model: "haiku".into(),
                config_dir: None,
                expected_email: None,
                sandbox: "unused-inert-fixture".into(),
                run_dir: "unused-inert-fixture".into(),
                mcp_url: "unused-inert-fixture".into(),
                system_prompt: String::new(),
                effort: None,
                context_window: None,
            },
            sink,
        );
        // This exercises only the production helper-owner acquisition; no exe lookup,
        // filesystem, auth/helper/CLI spawn or provider path is entered.
        let original = driver.helpers.lock().await;
        let allowance = crate::operation::Allowance::required(Some(
            tokio::time::Instant::now() + Duration::from_secs(1),
        ))
        .unwrap();
        let mut waiting = Box::pin(driver.helper_owner(allowance));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(waiting.as_mut().poll(&mut context), Poll::Pending));
        drop(waiting); // cancels a borrow, never takes/drops the original helper owner
        assert!(driver.helpers.try_lock().is_err());
        let expired = allowance.capped(Duration::ZERO);
        assert!(matches!(
            driver.helper_owner(expired).await,
            Err(DriverError::Host(crate::host::HostRefusal::Expired))
        ));
        assert!(driver.helpers.try_lock().is_err());
        assert!(original.is_empty());
        assert!(driver.proc.is_none());
        drop(original);
        // The expired allowance cannot be renewed merely because the lock became free.
        assert!(matches!(
            driver.helper_owner(expired).await,
            Err(DriverError::Host(crate::host::HostRefusal::Expired))
        ));
        assert!(driver.helpers.try_lock().is_ok());
        driver
            .sink
            .join_delivery_until(worker_deadline)
            .await
            .unwrap();
    }

    #[test]
    fn only_model_output_or_positive_turn_usage_produces_private_start_evidence() {
        let mut parser = ClaudeParser::default();
        for frame in [
            json!({"type":"system","subtype":"init","session_id":"s"}),
            json!({"type":"result","subtype":"success","session_id":"s","num_turns":1,
                "modelUsage":{"restored":{"inputTokens":123}},"total_cost_usd":1,"usage":{"output_tokens":0}}),
            json!({"type":"assistant","session_id":"s","message":{"content":[{"type":"tool_use","id":"","name":"submit"}]}}),
            json!({"type":"result","session_id":"s","usage":{"input_tokens":-1,"output_tokens":1.5}}),
        ] {
            assert!(
                !parser
                    .feed(&frame.to_string())
                    .iter()
                    .any(|event| matches!(event, StreamEvent::ModelStart { .. }))
            );
        }
        for (frame, expected) in [
            (
                json!({"type":"stream_event","session_id":"s","event":{"type":"message_start","message":{"role":"assistant"}}}),
                crate::operation::ConfirmationFrame::MessageStart,
            ),
            (
                json!({"type":"assistant","session_id":"s","message":{"content":[{"type":"text","text":"answer"}]}}),
                crate::operation::ConfirmationFrame::Assistant,
            ),
            (
                json!({"type":"result","session_id":"s","usage":{"cache_creation_input_tokens":1}}),
                crate::operation::ConfirmationFrame::ResultUsage,
            ),
        ] {
            assert!(parser.feed(&frame.to_string()).iter().any(|event|
                matches!(event, StreamEvent::ModelStart { session_id, frame } if session_id == "s" && *frame == expected)));
        }
    }

    const FIXTURE: &str = include_str!("../../tests/fixtures/claude_two_turns.jsonl");

    fn parse_all() -> Vec<StreamEvent> {
        let mut p = ClaudeParser::default();
        FIXTURE.lines().flat_map(|l| p.feed(l)).collect()
    }

    #[test]
    #[allow(clippy::float_arithmetic)]
    fn dated_haiku_reference_bounds_one_response_without_claiming_native_enforcement() {
        let table: Value =
            serde_json::from_str(include_str!("../../data/estimate_prices_2026-10-07.json"))
                .unwrap();
        assert_eq!(table["verified_date"], "2026-10-07");
        assert_eq!(table["pinned_model"], "claude-haiku-4-5-20251001");
        let bound = super::super::EstimateBound {
            pinned_model: table["pinned_model"].as_str().unwrap().into(),
            context_tokens: table["context_tokens"].as_u64().unwrap(),
            output_tokens: table["model_max_output_tokens"].as_u64().unwrap(),
            max_input_or_cache_write_usd_per_token: table["cache_write_1h"].as_f64().unwrap()
                / 1_000_000.,
            output_usd_per_token: table["output"].as_f64().unwrap() / 1_000_000.,
            evidence: "lead-verified dated source table; native cap enforcement remains unproved"
                .into(),
        };
        assert!((bound.ceiling_usd().unwrap() - 0.72).abs() < 1e-12);
    }
    #[test]
    fn version_and_cost_basis_gates_do_not_assume_installed_configuration() {
        assert!(supported_version("2.1.276 (Claude Code)").is_err());
        assert!(supported_version("unknown").is_err());
        assert!(supported_version("2.1.277 (Claude Code)").is_ok());
        let mut p = ClaudeParser::default();
        let events=p.feed(r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true,"result":"budget reached","total_cost_usd":0.3,"usage":{"input_tokens":10,"output_tokens":5},"modelUsage":{"haiku":{"costBasis":"list"}}}"#);
        let StreamEvent::TurnDone(outcome) = &events[0] else {
            panic!()
        };
        assert_eq!(outcome.error.as_deref(), Some("error_max_budget_usd"));
        assert_eq!(outcome.usage.cost_basis_known, Some(true));
        let events=p.feed(r#"{"type":"result","subtype":"success","modelUsage":{"haiku":{"costBasis":"unknown"}}}"#);
        let StreamEvent::TurnDone(outcome) = &events[0] else {
            panic!()
        };
        assert_eq!(outcome.usage.cost_basis_known, Some(false));
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
    fn raw_input_and_each_optional_cache_channel_are_preserved() {
        let mut parser = ClaudeParser::default();
        let events=parser.feed(r#"{"type":"result","subtype":"success","usage":{"input_tokens":11,"output_tokens":12,"cache_read_input_tokens":101,"cache_creation_input_tokens":102,"output_tokens_details":{"thinking_tokens":3}},"total_cost_usd":0.01}"#);
        let StreamEvent::TurnDone(turn) = &events[0] else {
            panic!("no turn outcome")
        };
        assert_eq!(
            (
                turn.usage.input_tokens,
                turn.usage.output_tokens,
                turn.usage.cached_input_tokens,
                turn.usage.cache_creation_tokens,
                turn.usage.reasoning_tokens
            ),
            (Some(11), Some(12), Some(101), Some(102), Some(3))
        );
        let missing = parser.feed(r#"{"type":"result","subtype":"success","usage":{}}"#);
        let StreamEvent::TurnDone(turn) = &missing[0] else {
            panic!("no turn outcome")
        };
        assert_eq!(turn.usage, Usage::default());
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

    fn argument_config() -> ClaudeConfig {
        ClaudeConfig {
            seat: "axis.commander".parse().unwrap(),
            exe: None,
            model: "model \"λ\"".into(),
            config_dir: None,
            expected_email: None,
            sandbox: "/s".into(),
            run_dir: "/r".into(),
            mcp_url: "unused-recorded-fixture".into(),
            system_prompt: "first\nβ \"quoted\" \\ suffix".into(),
            effort: Some("medium".into()),
            context_window: None,
        }
    }

    #[test]
    fn runtime_environment_keeps_profile_and_compaction_values_exact() {
        let mut cfg = argument_config();
        cfg.config_dir = Some("recorded profile λ".into());
        cfg.context_window = Some(u64::MAX);
        let path_pointer = cfg
            .config_dir
            .as_ref()
            .unwrap()
            .as_os_str()
            .as_encoded_bytes()
            .as_ptr();
        let operation = metadata_operation();
        let env = claude_env(&cfg, EventStorage(Some(&operation))).unwrap();
        assert_eq!(
            env,
            [
                (
                    "CLAUDE_CONFIG_DIR",
                    std::ffi::OsString::from("recorded profile λ")
                ),
                (
                    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
                    std::ffi::OsString::from(u64::MAX.to_string())
                ),
                (
                    "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
                    std::ffi::OsString::from("80")
                ),
                (
                    "CLAUDE_CODE_DISABLE_REFUSAL_FALLBACK",
                    std::ffi::OsString::from("1")
                ),
            ]
        );
        assert_eq!(
            cfg.config_dir
                .as_ref()
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .as_ptr(),
            path_pointer
        );
        assert!(
            operation.captured_bytes()
                > env
                    .iter()
                    .map(|(_, value)| value.as_encoded_bytes().len())
                    .sum::<usize>()
        );
        cfg.config_dir = None;
        cfg.context_window = None;
        let defaults = claude_env(&cfg, EventStorage(Some(&metadata_operation()))).unwrap();
        assert_eq!(
            defaults,
            [(
                "CLAUDE_CODE_DISABLE_REFUSAL_FALLBACK",
                std::ffi::OsString::from("1")
            )]
        );
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[test]
    fn runtime_environment_profile_refusal_does_not_copy_or_dispatch() {
        let mut cfg = argument_config();
        cfg.config_dir = Some("λ".repeat(crate::operation::OPERATION_BYTES / 2).into());
        let profile = cfg.config_dir.as_ref().unwrap().as_os_str();
        let pointer = profile.as_encoded_bytes().as_ptr();
        let operation = metadata_operation();
        assert_eq!(
            claude_env(&cfg, EventStorage(Some(&operation))),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert_eq!(profile.as_encoded_bytes().as_ptr(), pointer);
        assert_eq!(
            profile.as_encoded_bytes().len(),
            crate::operation::OPERATION_BYTES
        );
        assert!(operation.captured_bytes() < profile.as_encoded_bytes().len());
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
    }

    #[test]
    fn runtime_arguments_preserve_unicode_flags_sessions_and_cost_value() {
        let cfg = argument_config();
        let path = std::path::Path::new("/recorded λ/config space.mcp.json");
        for resumed in [false, true] {
            let operation = metadata_operation();
            let operation_baseline = operation.captured_bytes();
            let session = if resumed {
                SessionArg::Resume("original λ session")
            } else {
                SessionArg::New("original λ session")
            };
            let mut expected = claude_args(&cfg, path, session);
            expected.extend(["--max-budget-usd".into(), "0.0125".into()]);
            let args = build_claude_args(
                &cfg,
                path,
                session,
                Some(0.0125),
                EventStorage(Some(&operation)),
            )
            .unwrap();
            assert_eq!(args, expected);
            let at = |flag: &str| args.iter().position(|value| value == flag).unwrap();
            assert_eq!(args[at("--system-prompt") + 1], cfg.system_prompt);
            assert_eq!(args[at("--model") + 1], cfg.model);
            assert_eq!(args[at("--mcp-config") + 1], path.to_string_lossy());
            assert_eq!(args[at("--effort") + 1], "medium");
            assert_eq!(args[at("--tools") + 1], "");
            assert_eq!(args[at("--setting-sources") + 1], "");
            assert_eq!(args[at("--permission-mode") + 1], "dontAsk");
            assert!(args.contains(&"--strict-mcp-config".into()));
            assert_eq!(args.contains(&"--resume".into()), resumed);
            assert_eq!(args.contains(&"--session-id".into()), !resumed);
            let payload = args.iter().map(String::len).sum::<usize>()
                + args.len() * std::mem::size_of::<String>()
                + std::mem::size_of::<Vec<String>>();
            assert!(operation.captured_bytes() - operation_baseline >= payload);
            assert_eq!(
                operation.start_evidence(),
                crate::operation::StartEvidence::NotStarted
            );
        }
    }

    #[test]
    fn argument_copy_boundary_refusal_preserves_borrowed_configuration_and_liability() {
        let cfg = argument_config();
        let prompt_pointer = cfg.system_prompt.as_ptr();
        let model_pointer = cfg.model.as_ptr();
        let path = std::path::Path::new("/recorded/config.mcp.json");
        let probe = metadata_operation();
        let probe_baseline = probe.captured_bytes();
        let expected = build_claude_args(
            &cfg,
            path,
            SessionArg::Resume("original session"),
            Some(0.0125),
            EventStorage(Some(&probe)),
        )
        .unwrap();
        let required = probe.captured_bytes() - probe_baseline;
        let fits = metadata_operation();
        fits.charge(crate::operation::OPERATION_BYTES - fits.captured_bytes() - required)
            .unwrap();
        assert_eq!(
            build_claude_args(
                &cfg,
                path,
                SessionArg::Resume("original session"),
                Some(0.0125),
                EventStorage(Some(&fits)),
            )
            .unwrap(),
            expected
        );
        assert_eq!(fits.captured_bytes(), crate::operation::OPERATION_BYTES);
        let refused = metadata_operation();
        refused.possible_start(); // Prior dispatched work, never refunded by argv refusal.
        refused
            .charge(crate::operation::OPERATION_BYTES - refused.captured_bytes() - required + 1)
            .unwrap();
        let before = refused.captured_bytes();
        assert_eq!(
            build_claude_args(
                &cfg,
                path,
                SessionArg::Resume("original session"),
                Some(0.0125),
                EventStorage(Some(&refused)),
            ),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert!(refused.captured_bytes() >= before);
        assert!(refused.captured_bytes() <= crate::operation::OPERATION_BYTES);
        let charged = refused.captured_bytes();
        assert_eq!(
            build_claude_args(
                &cfg,
                path,
                SessionArg::Resume("original session"),
                None,
                EventStorage(Some(&refused)),
            ),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert_eq!(refused.captured_bytes(), charged); // Sticky, no retry allocation/refund.
        assert_eq!(cfg.system_prompt.as_ptr(), prompt_pointer);
        assert_eq!(cfg.model.as_ptr(), model_pointer);
        assert_eq!(
            refused.start_evidence(),
            crate::operation::StartEvidence::Possible
        );
    }

    #[test]
    fn oversized_argument_body_refuses_without_dispatch_or_reclassifying_start() {
        let mut cfg = argument_config();
        cfg.system_prompt = "λ".repeat(crate::operation::OPERATION_BYTES / 2);
        let pointer = cfg.system_prompt.as_ptr();
        let operation = metadata_operation();
        assert_eq!(
            build_claude_args(
                &cfg,
                std::path::Path::new("/recorded/config.mcp.json"),
                SessionArg::New("original session"),
                None,
                EventStorage(Some(&operation)),
            ),
            Err(crate::operation::OperationError::CaptureOverflow)
        );
        assert_eq!(cfg.system_prompt.as_ptr(), pointer);
        assert_eq!(cfg.system_prompt.len(), crate::operation::OPERATION_BYTES);
        assert!(operation.captured_bytes() < cfg.system_prompt.len());
        assert_eq!(
            operation.start_evidence(),
            crate::operation::StartEvidence::NotStarted
        );
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
            context_window: None,
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
    #[test]
    fn compaction_and_current_request_context_are_observed_separately_from_turn_usage() {
        let mut p = ClaudeParser::default();
        let events=p.feed(r#"{"type":"assistant","message":{"usage":{"input_tokens":20,"cache_read_input_tokens":100,"cache_creation_input_tokens":30,"output_tokens":5},"content":[]}}"#);
        assert_eq!(events, [StreamEvent::Context(155)]);
        let events=p.feed(r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"auto","pre_tokens":10000}}"#);
        assert_eq!(
            events,
            [StreamEvent::Compacted {
                trigger: "auto".into(),
                pre_tokens: Some(10000)
            }]
        );
        let events=p.feed(r#"{"type":"result","subtype":"success","usage":{"input_tokens":99999},"total_cost_usd":0.02}"#);
        assert!(!events.iter().any(|e| matches!(e, StreamEvent::Context(_))));
    }
}
