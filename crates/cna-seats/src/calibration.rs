//! Lane-neutral calibration admission core. It has no network, credentials, CLI
//! integration or managed settings. This is NOT an installed transport fence.
//! The HTTP adapter enforces its routing; native egress remains a separate gate.
//! The documented token count is an estimate; the 95K check is not a hard 100K proof.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const MODEL: &str = "claude-haiku-5-5";
pub const COUNT_LIMIT: u64 = 95_000;
pub const ACTUAL_PROMPT_LIMIT: u64 = 100_000;
pub const OUTPUT_LIMIT: u64 = 128_000;
const BODY_LIMIT: usize = 16 * 1024 * 1024;
const COUNT_ATTEMPTS: i64 = 4;

/// Errors intentionally never include body, headers, paths or transport diagnostics.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FenceError {
    #[error("invalid or unsupported request body")]
    Body,
    #[error("unavailable or invalid private permit ledger")]
    Ledger,
    #[error("calibration generation permit already spent")]
    Spent,
    #[error("bounded token-count attempts exhausted")]
    CountsExhausted,
    #[error("missing, stale, untrusted or excessive token estimate")]
    Estimate,
    #[error("generation transport failed; permit remains spent")]
    Transport,
}

/// Private trusted-adapter ticket. Never accepted from a seat's HTTP payload.
pub struct CountTicket {
    attempt: i64,
    digest: Vec<u8>,
    body: Vec<u8>,
    count_body: Vec<u8>,
    dropped_fields: Vec<String>,
}

impl CountTicket {
    /// Exact normalized inference body: forwarded unchanged after its matching
    /// documented prompt projection has been counted. Not the count bytes.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Beta count schema projection, preserving accepted fields verbatim.
    pub fn count_body(&self) -> &[u8] {
        &self.count_body
    }

    /// Inference-only field names omitted by the documented count schema.
    pub fn dropped_fields(&self) -> &[String] {
        &self.dropped_fields
    }
}

/// Independent connections use SQLite's atomic conditional update, including across
/// process restarts. Opening never creates or rearms a missing/spent ledger.
pub struct CalibrationFence {
    path: PathBuf,
}

impl CalibrationFence {
    /// Explicit trusted creation only, at a new absolute private scratch path.
    /// Existing files (including interrupted initialization) are never replaced.
    pub fn initialize(path: &Path) -> Result<Self, FenceError> {
        if !path.is_absolute() {
            return Err(FenceError::Ledger);
        }
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| FenceError::Ledger)?;
        let fence = Self { path: path.into() };
        let conn = fence.connection()?;
        conn.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE calibration (
                 id INTEGER PRIMARY KEY CHECK (id=1),
                 version INTEGER NOT NULL CHECK (version=1),
                 spent INTEGER NOT NULL CHECK (spent IN (0,1)),
                 attempts INTEGER NOT NULL CHECK (attempts BETWEEN 0 AND 4),
                 digest BLOB, estimate INTEGER);
             INSERT INTO calibration VALUES (1,1,0,0,NULL,NULL);
             COMMIT;",
        )
        .map_err(|_| FenceError::Ledger)?;
        Ok(fence)
    }

    pub fn open(path: &Path) -> Result<Self, FenceError> {
        if !path.is_absolute() {
            return Err(FenceError::Ledger);
        }
        let fence = Self { path: path.into() };
        let conn = fence.connection()?;
        let version: i64 = conn
            .query_row("SELECT version FROM calibration WHERE id=1", [], |r| {
                r.get(0)
            })
            .map_err(|_| FenceError::Ledger)?;
        if version != 1 {
            return Err(FenceError::Ledger);
        }
        Ok(fence)
    }

    fn connection(&self) -> Result<Connection, FenceError> {
        let metadata = self
            .path
            .symlink_metadata()
            .map_err(|_| FenceError::Ledger)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(FenceError::Ledger);
        }
        let conn = Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|_| FenceError::Ledger)?;
        conn.busy_timeout(Duration::from_secs(1))
            .map_err(|_| FenceError::Ledger)?;
        conn.execute_batch("PRAGMA synchronous=FULL; PRAGMA trusted_schema=OFF;")
            .map_err(|_| FenceError::Ledger)?;
        Ok(conn)
    }

    /// Charge a count attempt BEFORE its transport is invoked. A new attempt
    /// invalidates the old quote, even for the same body. No credential is stored.
    pub fn begin_count(&self, raw: &[u8]) -> Result<CountTicket, FenceError> {
        let body = checked_body(raw)?;
        let digest = Sha256::digest(&body).to_vec();
        let (count_body, dropped_fields) = count_projection(&body)?;
        let conn = self.connection()?;
        let attempt = conn.query_row(
            "UPDATE calibration SET attempts=attempts+1,digest=?1,estimate=NULL
             WHERE id=1 AND version=1 AND spent=0 AND attempts<?2 RETURNING attempts",
            params![digest, COUNT_ATTEMPTS],
            |r| r.get(0),
        );
        match attempt {
            Ok(attempt) => Ok(CountTicket {
                attempt,
                digest,
                body,
                count_body,
                dropped_fields,
            }),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                if self.spent()? {
                    Err(FenceError::Spent)
                } else {
                    Err(FenceError::CountsExhausted)
                }
            }
            Err(_) => Err(FenceError::Ledger),
        }
    }

    /// The trusted adapter must authenticate the official pinned-model response
    /// for this ticket's deterministic count projection of the inference body. Unknown basis or a failed response never creates a quote.
    pub fn finish_count(
        &self,
        ticket: CountTicket,
        response: &[u8],
        trusted_same_body_basis: bool,
    ) -> Result<(), FenceError> {
        let tokens = if trusted_same_body_basis {
            serde_json::from_slice::<Value>(response)
                .ok()
                .and_then(|v| v["input_tokens"].as_u64())
                .filter(|n| *n > 0 && *n <= COUNT_LIMIT)
        } else {
            None
        };
        let conn = self.connection()?;
        let changed = conn
            .execute(
                "UPDATE calibration SET estimate=?1 WHERE id=1 AND version=1
                 AND spent=0 AND attempts=?2 AND digest=?3",
                params![tokens, ticket.attempt, ticket.digest],
            )
            .map_err(|_| FenceError::Ledger)?;
        if changed == 1 && tokens.is_some() {
            Ok(())
        } else {
            Err(FenceError::Estimate)
        }
    }

    /// Commit the one permit before calling a trusted one-shot transport. The
    /// callback is invoked at most once by this core. Adapter transport and
    /// native egress must be reviewed separately; a callback is not wire proof.
    /// Results are returned unchanged, including opaque SSE/error/header bytes.
    pub fn generate<T>(
        &self,
        raw: &[u8],
        forward_once: impl FnOnce(&[u8]) -> Result<T, FenceError>,
    ) -> Result<T, FenceError> {
        let body = checked_body(raw)?;
        let digest = Sha256::digest(&body).to_vec();
        let conn = self.connection()?;
        let changed = conn
            .execute(
                "UPDATE calibration SET spent=1 WHERE id=1 AND version=1 AND spent=0
                 AND digest=?1 AND estimate BETWEEN 1 AND ?2",
                params![digest, COUNT_LIMIT],
            )
            .map_err(|_| FenceError::Ledger)?;
        if changed != 1 {
            return Err(if self.spent()? {
                FenceError::Spent
            } else {
                FenceError::Estimate
            });
        }
        forward_once(&body).map_err(|_| FenceError::Transport)
    }

    pub fn spent(&self) -> Result<bool, FenceError> {
        self.connection()?
            .query_row(
                "SELECT spent FROM calibration WHERE id=1 AND version=1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| FenceError::Ledger)
    }
}

pub(crate) fn checked_body(raw: &[u8]) -> Result<Vec<u8>, FenceError> {
    if raw.len() > BODY_LIMIT {
        return Err(FenceError::Body);
    }
    let value: Value = serde_json::from_slice(raw).map_err(|_| FenceError::Body)?;
    let object = value.as_object().ok_or(FenceError::Body)?;
    let allowed = [
        "model",
        "max_tokens",
        "messages",
        "system",
        "tools",
        "tool_choice",
        "thinking",
        "output_config",
        "stream",
        "temperature",
        "top_p",
        "top_k",
        "stop_sequences",
        "metadata",
        "context_management",
    ];
    if object.keys().any(|k| !allowed.contains(&k.as_str()))
        || value["model"] != MODEL
        || !value["max_tokens"]
            .as_u64()
            .is_some_and(|n| n > 0 && n <= OUTPUT_LIMIT)
        || value["messages"].as_array().is_none_or(|m| m.is_empty())
        || value.get("stream").is_some_and(|v| !v.is_boolean())
        || object.contains_key("thinking")
        || contains_budget_control(&value)
    {
        return Err(FenceError::Body);
    }
    if let Some(tools) = value.get("tools") {
        let tools = tools.as_array().ok_or(FenceError::Body)?;
        for tool in tools {
            let name = tool["name"].as_str().ok_or(FenceError::Body)?;
            let name = name.strip_prefix("mcp__cna__").ok_or(FenceError::Body)?;
            if !crate::mcp::TOOL_NAMES.contains(&name) || tool.get("type").is_some() {
                return Err(FenceError::Body);
            }
        }
    }
    // Generation uses these normalized bytes. Its count projection preserves
    // accepted prompt fields exactly. Duplicate-key ambiguity cannot change
    // interpretation between the two transports.
    serde_json::to_vec(&value).map_err(|_| FenceError::Body)
}

// Calibration-only request whitelist, lead ruling 2026-10-08: omit thinking.
// Reject structural manual-budget controls at any depth, not words in prose.
fn contains_budget_control(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key("budget_tokens") || object.values().any(contains_budget_control)
        }
        Value::Array(array) => array.iter().any(contains_budget_control),
        _ => false,
    }
}

/// Official beta count schema, lead-fetched 2026-10-08:
/// https://platform.claude.com/docs/en/api/beta/messages/count_tokens .
/// Inference-only fields are deliberately omitted, not silently represented
/// as identical counted bytes. The ledger's digest always binds FULL inference.
pub(crate) fn count_projection(inference: &[u8]) -> Result<(Vec<u8>, Vec<String>), FenceError> {
    const FIELDS: &[&str] = &[
        "messages",
        "model",
        "cache_control",
        "compaction",
        "context_management",
        "mcp_servers",
        "output_config",
        "speed",
        "system",
        "thinking",
        "tool_choice",
        "tools",
        "output_format",
    ];
    let value: Value = serde_json::from_slice(inference).map_err(|_| FenceError::Body)?;
    let object = value.as_object().ok_or(FenceError::Body)?;
    let mut projected = serde_json::Map::new();
    let mut dropped = Vec::new();
    for (name, value) in object {
        if FIELDS.contains(&name.as_str()) {
            projected.insert(name.clone(), value.clone());
        } else {
            dropped.push(name.clone());
        }
    }
    Ok((
        serde_json::to_vec(&projected).map_err(|_| FenceError::Body)?,
        dropped,
    ))
}

/// Post-response accounting, not a pre-send guarantee. Absent channels remain
/// unknown. The prompt-tier interpretation of this sum needs provider verification.
pub fn assess_prompt_usage(
    input: Option<u64>,
    cache_creation: Option<u64>,
    cache_read: Option<u64>,
) -> Result<(u64, bool), FenceError> {
    let total = input
        .zip(cache_creation)
        .zip(cache_read)
        .and_then(|((i, w), h)| i.checked_add(w)?.checked_add(h))
        .ok_or(FenceError::Estimate)?;
    Ok((total, total > ACTUAL_PROMPT_LIMIT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    };

    struct PrivateLedger(PathBuf);
    impl PrivateLedger {
        fn new() -> Self {
            Self(
                std::env::temp_dir()
                    .join(format!("cna-offline-fence-{}.sqlite", uuid::Uuid::new_v4())),
            )
        }
    }
    impl Drop for PrivateLedger {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn body() -> Vec<u8> {
        serde_json::to_vec(&json!({"model": MODEL,"max_tokens": 128000,
            "messages":[{"role":"user","content":"Synthetic offline fixture"}],"stream":true}))
        .unwrap()
    }
    fn quote(fence: &CalibrationFence, body: &[u8], tokens: u64) {
        let ticket = fence.begin_count(body).unwrap();
        fence
            .finish_count(
                ticket,
                &serde_json::to_vec(&json!({"input_tokens":tokens})).unwrap(),
                true,
            )
            .unwrap();
    }

    #[test]
    fn one_forward_persists_across_reopen_and_preserves_opaque_reply() {
        let path = PrivateLedger::new();
        let fence = CalibrationFence::initialize(&path.0).unwrap();
        let body = body();
        let ticket = fence.begin_count(&body).unwrap();
        let counted = ticket.body().to_vec();
        fence
            .finish_count(ticket, br#"{"input_tokens":95000}"#, true)
            .unwrap();
        let opaque = (
            429,
            vec![("retry-after", "3"), ("anthropic-beta", "synthetic")],
            vec![
                b"event: ping\ndata: {}\n\n".to_vec(),
                b"data: synthetic error\n\n".to_vec(),
            ],
        );
        let result = fence
            .generate(&body, |forwarded| {
                assert_eq!(forwarded, counted);
                assert!(CalibrationFence::open(&path.0).unwrap().spent().unwrap());
                Ok(opaque.clone())
            })
            .unwrap();
        assert_eq!(result, opaque);
        let reopened = CalibrationFence::open(&path.0).unwrap();
        assert_eq!(reopened.generate(&body, |_| Ok(())), Err(FenceError::Spent));
        assert_eq!(reopened.begin_count(&body).err(), Some(FenceError::Spent));
        assert!(CalibrationFence::initialize(&path.0).is_err());
    }

    #[test]
    fn error_and_panic_after_commit_never_restore_permit() {
        for panic in [false, true] {
            let path = PrivateLedger::new();
            let fence = CalibrationFence::initialize(&path.0).unwrap();
            quote(&fence, &body(), 100);
            if panic {
                assert!(
                    std::panic::catch_unwind(
                        || fence.generate::<()>(&body(), |_| panic!("synthetic cancellation"))
                    )
                    .is_err()
                );
            } else {
                assert_eq!(
                    fence.generate::<()>(&body(), |_| Err(FenceError::Body)),
                    Err(FenceError::Transport)
                );
            }
            assert!(CalibrationFence::open(&path.0).unwrap().spent().unwrap());
        }
    }

    #[test]
    fn concurrent_independent_connections_forward_exactly_once() {
        let path = PrivateLedger::new();
        let fence = CalibrationFence::initialize(&path.0).unwrap();
        quote(&fence, &body(), 100);
        let barrier = Arc::new(Barrier::new(8));
        let sends = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let barrier = barrier.clone();
                let sends = sends.clone();
                let path = &path.0;
                scope.spawn(move || {
                    let fence = CalibrationFence::open(path).unwrap();
                    barrier.wait();
                    let _ = fence.generate(&body(), |_| {
                        sends.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    });
                });
            }
        });
        assert_eq!(sends.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn child_exit_after_durable_commit_does_not_rearm() {
        const CHILD: &str = "CNA_OFFLINE_FENCE_CRASH_CHILD";
        if let Some(path) = std::env::var_os(CHILD) {
            let fence = CalibrationFence::open(Path::new(&path)).unwrap();
            let _ = fence.generate::<()>(&body(), |_| std::process::exit(37));
            panic!("inert child must exit only after permit commit");
        }
        let path = PrivateLedger::new();
        let fence = CalibrationFence::initialize(&path.0).unwrap();
        quote(&fence, &body(), 100);
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "calibration::tests::child_exit_after_durable_commit_does_not_rearm",
            ])
            .env(CHILD, &path.0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                result => {
                    let killed = child.kill();
                    let joined = child.wait();
                    panic!(
                        "inert child did not finish: {result:?}; kill={killed:?}; join={joined:?}"
                    );
                }
            }
        };
        assert_eq!(status.code(), Some(37));
        let reopened = CalibrationFence::open(&path.0).unwrap();
        assert!(reopened.spent().unwrap());
        assert_eq!(
            reopened.generate(&body(), |_| Ok(())),
            Err(FenceError::Spent)
        );
    }

    #[test]
    fn zero_count_is_unknown_and_cannot_admit_nonempty_request() {
        let path = PrivateLedger::new();
        let fence = CalibrationFence::initialize(&path.0).unwrap();
        let ticket = fence.begin_count(&body()).unwrap();
        assert_eq!(
            fence.finish_count(ticket, br#"{"input_tokens":0}"#, true),
            Err(FenceError::Estimate)
        );
        assert_eq!(
            fence.generate(&body(), |_| Ok(())),
            Err(FenceError::Estimate)
        );
        assert!(!fence.spent().unwrap());
    }

    #[test]
    fn changed_body_and_stale_quote_cannot_forward() {
        let path = PrivateLedger::new();
        let fence = CalibrationFence::initialize(&path.0).unwrap();
        let old = fence.begin_count(&body()).unwrap();
        let mut changed: Value = serde_json::from_slice(&body()).unwrap();
        changed["system"] = json!("Changed native context: \u{1F30D}");
        let changed = serde_json::to_vec(&changed).unwrap();
        let current = fence.begin_count(&changed).unwrap();
        assert_eq!(
            fence.finish_count(old, br#"{"input_tokens":1}"#, true),
            Err(FenceError::Estimate)
        );
        fence
            .finish_count(current, br#"{"input_tokens":94999}"#, true)
            .unwrap();
        assert_eq!(
            fence.generate(&body(), |_| Ok(())),
            Err(FenceError::Estimate)
        );
        assert!(!fence.spent().unwrap());
        assert!(fence.generate(&changed, |_| Ok(())).is_ok());
    }

    #[test]
    fn invalid_count_basis_and_count_exhaustion_fail_closed() {
        let path = PrivateLedger::new();
        let fence = CalibrationFence::initialize(&path.0).unwrap();
        for (response, trusted) in [
            (br#"{"input_tokens":95001}"#.as_slice(), true),
            (br#"{"input_tokens":-1}"#, true),
            (br#"{"input_tokens":1}"#, false),
            (b"private malformed response", true),
        ] {
            let ticket = fence.begin_count(&body()).unwrap();
            assert_eq!(
                fence.finish_count(ticket, response, trusted),
                Err(FenceError::Estimate)
            );
            assert_eq!(
                fence.generate(&body(), |_| Ok(())),
                Err(FenceError::Estimate)
            );
        }
        assert_eq!(
            fence.begin_count(&body()).err(),
            Some(FenceError::CountsExhausted)
        );
        assert!(!fence.spent().unwrap());
    }

    #[test]
    fn unsupported_model_output_tools_and_fields_rejected_before_count() {
        for (key, value) in [
            ("model", json!("haiku")),
            ("max_tokens", json!(128001)),
            ("max_tokens", json!(0)),
            ("max_tokens", json!(1.5)),
            ("fallback_model", json!("other")),
            ("messages", json!([])),
            ("tools", json!([{"name":"Bash"}])),
            ("tools", json!([{"name":"mcp__other__observe"}])),
            (
                "tools",
                json!([{"name":"mcp__cna__observe","type":"web_search"}]),
            ),
        ] {
            let mut request: Value = serde_json::from_slice(&body()).unwrap();
            request[key] = value;
            assert_eq!(
                checked_body(&serde_json::to_vec(&request).unwrap()).err(),
                Some(FenceError::Body)
            );
        }
        assert_eq!(checked_body(b"not JSON").err(), Some(FenceError::Body));
    }

    #[test]
    fn thinking_and_structural_budget_refusal_preserves_all_durable_admission_state() {
        let path = PrivateLedger::new();
        let fence = CalibrationFence::initialize(&path.0).unwrap();
        let attempts = || {
            fence
                .connection()
                .unwrap()
                .query_row("SELECT attempts FROM calibration WHERE id=1", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap()
        };
        let mut rejected = Vec::new();
        for thinking in [
            json!({"type":"adaptive"}),
            json!({"type":"disabled"}),
            json!({"type":"enabled","budget_tokens":100}),
            json!({"type":"enabled"}),
            Value::Null,
            json!({}),
            json!(true),
            json!(1),
            json!("adaptive"),
            json!([]),
            json!({"type":"unknown"}),
        ] {
            let mut request: Value = serde_json::from_slice(&body()).unwrap();
            request["thinking"] = thinking;
            rejected.push(request);
        }
        for (field, value) in [
            ("budget_tokens", json!(100)),
            ("metadata", json!({"budget_tokens":null})),
            (
                "output_config",
                json!({"task_budget":{"budget_tokens":100}}),
            ),
            (
                "messages",
                json!([{"role":"user","content":[{"budget_tokens":100}]}]),
            ),
        ] {
            let mut request: Value = serde_json::from_slice(&body()).unwrap();
            request[field] = value;
            rejected.push(request);
        }
        let sends = AtomicUsize::new(0);
        for request in &rejected {
            let raw = serde_json::to_vec(request).unwrap();
            assert!(matches!(fence.begin_count(&raw), Err(FenceError::Body)));
            assert_eq!(
                fence.generate(&raw, |_| {
                    sends.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
                Err(FenceError::Body)
            );
            assert_eq!(attempts(), 0);
            assert!(!fence.spent().unwrap());
        }
        // An omission-only quote cannot authorize adding any refused field.
        quote(&fence, &body(), 100);
        for request in rejected {
            let raw = serde_json::to_vec(&request).unwrap();
            assert!(matches!(fence.begin_count(&raw), Err(FenceError::Body)));
            assert_eq!(fence.generate(&raw, |_| Ok(())), Err(FenceError::Body));
            assert_eq!(attempts(), 1);
            assert!(!fence.spent().unwrap());
        }
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        assert!(fence.generate(&body(), |_| Ok(())).is_ok());
        assert!(fence.spent().unwrap());
        assert_eq!(attempts(), 1);
    }

    #[test]
    fn budget_control_words_in_prose_are_not_request_controls() {
        let path = PrivateLedger::new();
        let fence = CalibrationFence::initialize(&path.0).unwrap();
        let mut request: Value = serde_json::from_slice(&body()).unwrap();
        request["system"] = json!("Explain the budget_tokens field in prose.");
        request["messages"][0]["content"] = json!("thinking budget_tokens enabled adaptive");
        let raw = serde_json::to_vec(&request).unwrap();
        let ticket = fence.begin_count(&raw).unwrap();
        let projected: Value = serde_json::from_slice(ticket.count_body()).unwrap();
        assert_eq!(projected["system"], request["system"]);
        assert_eq!(projected["messages"], request["messages"]);
        fence
            .finish_count(ticket, br#"{"input_tokens":100}"#, true)
            .unwrap();
        assert!(fence.generate(&raw, |_| Ok(())).is_ok());
    }

    #[test]
    fn beta_projection_preserves_prompt_fields_and_binds_create_only_mutations() {
        let path = PrivateLedger::new();
        let fence = CalibrationFence::initialize(&path.0).unwrap();
        let mut request: Value = serde_json::from_slice(&body()).unwrap();
        request["system"] = json!([{"type":"text","text":"Synthetic system \u{1F30D}","cache_control":{"type":"ephemeral"}}]);
        request["output_config"] = json!({"effort":"medium"});
        request["tools"] = Value::Array(
            crate::mcp::ToolRouter::tool_list()["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| {
                    json!({
                        "name": format!("mcp__cna__{}", tool["name"].as_str().unwrap()),
                        "description": tool["description"],
                        "input_schema": tool["inputSchema"]
                    })
                })
                .collect(),
        );
        request["tool_choice"] = json!({"type":"auto"});
        request["metadata"] = json!({"user_id":"synthetic-user"});
        let raw = serde_json::to_vec(&request).unwrap();
        let ticket = fence.begin_count(&raw).unwrap();
        let count: Value = serde_json::from_slice(ticket.count_body()).unwrap();
        for name in [
            "model",
            "messages",
            "system",
            "output_config",
            "tools",
            "tool_choice",
        ] {
            assert_eq!(count[name], request[name]);
        }
        assert_eq!(
            ticket.dropped_fields(),
            ["max_tokens", "metadata", "stream"]
        );
        assert_ne!(ticket.count_body(), ticket.body());
        fence
            .finish_count(ticket, br#"{"input_tokens":90000}"#, true)
            .unwrap();
        request["max_tokens"] = json!(100);
        assert_eq!(
            fence.generate(&serde_json::to_vec(&request).unwrap(), |_| Ok(())),
            Err(FenceError::Estimate)
        );
        assert!(!fence.spent().unwrap());
        assert!(fence.generate(&raw, |_| Ok(())).is_ok());
    }

    #[test]
    fn usage_is_nullable_checked_and_crossing_is_reported() {
        assert_eq!(
            assess_prompt_usage(Some(90000), Some(5000), Some(5000)),
            Ok((100000, false))
        );
        assert_eq!(
            assess_prompt_usage(Some(90001), Some(5000), Some(5000)),
            Ok((100001, true))
        );
        assert_eq!(
            assess_prompt_usage(Some(1), None, Some(0)),
            Err(FenceError::Estimate)
        );
        assert_eq!(
            assess_prompt_usage(Some(u64::MAX), Some(1), Some(0)),
            Err(FenceError::Estimate)
        );
    }

    #[test]
    fn missing_or_corrupt_ledger_does_not_rearm() {
        let path = PrivateLedger::new();
        assert!(CalibrationFence::open(&path.0).is_err());
        assert!(!path.0.exists());
        std::fs::write(&path.0, b"invalid ledger; never replace").unwrap();
        assert!(CalibrationFence::open(&path.0).is_err());
        assert!(CalibrationFence::initialize(&path.0).is_err());
        assert_eq!(
            std::fs::read(&path.0).unwrap(),
            b"invalid ledger; never replace"
        );
    }
}
