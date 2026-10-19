//! Bounded JSONL inspection, NOT native resume admission. No file is opened,
//! installed or rewritten here. Namespace, file identity/link/stream checks and
//! complete conversation/compaction restoration review are still required.
//!
//! CLI2.1.293 sources: W7 cost schema at217120554; permission-mode writer
//! at221493933; J1 policy restore at221574798; Ame cost restore at217412701.
//! We retain original bytes and refuse malformed state rather than dropping it.

use crate::{calibration::MODEL, calibration_http::strict_json};
use serde_json::Value;
use std::io::BufRead;
use uuid::Uuid;

pub const MAX_TRANSCRIPT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_RECORDS: usize = 100_000;
pub const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy)]
struct Limits {
    bytes: usize,
    records: usize,
    record_bytes: usize,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TranscriptRefusal {
    #[error("transcript read unavailable")]
    Read,
    #[error("transcript byte cap exceeded")]
    Bytes,
    #[error("transcript record cap exceeded")]
    Records,
    #[error("transcript record byte cap exceeded")]
    RecordBytes,
    #[error("empty or incomplete transcript record")]
    Incomplete,
    #[error("ambiguous or invalid transcript JSON")]
    Json,
    #[error("transcript session binding differs")]
    Session,
    #[error("unsupported or policy-bearing transcript record")]
    Policy,
    #[error("persisted permission differs from invocation")]
    Permission,
    #[error("invalid or unsupported cost state")]
    Cost,
    #[error("required cost state is missing")]
    MissingCost,
}

/// Syntax and the two inspected metadata schemas only. Successful inspection
/// does not certify conversation/compaction fields or authorize any launch.
/// Deliberately no Debug/serialization of the retained conversation or cost map.
pub struct TranscriptInspection {
    original: Vec<u8>,
    records: usize,
    unreviewed_conversation_records: usize,
    last_cost: Value,
}

impl TranscriptInspection {
    pub fn original(&self) -> &[u8] {
        &self.original
    }
    pub fn records(&self) -> usize {
        self.records
    }
    pub fn unreviewed_conversation_records(&self) -> usize {
        self.unreviewed_conversation_records
    }
    pub fn last_cost_state(&self) -> &Value {
        &self.last_cost
    }
}

pub fn inspect(
    reader: impl BufRead,
    session: Uuid,
) -> Result<TranscriptInspection, TranscriptRefusal> {
    inspect_with_limits(
        reader,
        session,
        Limits {
            bytes: MAX_TRANSCRIPT_BYTES,
            records: MAX_RECORDS,
            record_bytes: MAX_RECORD_BYTES,
        },
    )
}

fn inspect_with_limits(
    mut reader: impl BufRead,
    session: Uuid,
    limits: Limits,
) -> Result<TranscriptInspection, TranscriptRefusal> {
    let mut original = Vec::new();
    let mut row = Vec::new();
    let mut records = 0;
    let mut unreviewed_conversation_records = 0;
    let mut last_cost = None;
    let expected = session.to_string();
    loop {
        let buffer = reader.fill_buf().map_err(|_| TranscriptRefusal::Read)?;
        if buffer.is_empty() {
            break;
        }
        let end = buffer
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(buffer.len());
        if original.len().saturating_add(end) > limits.bytes {
            return Err(TranscriptRefusal::Bytes);
        }
        if row.len().saturating_add(end) > limits.record_bytes {
            return Err(TranscriptRefusal::RecordBytes);
        }
        let complete = buffer[end - 1] == b'\n';
        original.extend_from_slice(&buffer[..end]);
        row.extend_from_slice(&buffer[..end]);
        reader.consume(end);
        if !complete {
            continue;
        }
        records += 1;
        if records > limits.records {
            return Err(TranscriptRefusal::Records);
        }
        let raw = &row[..row.len() - 1];
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        if raw.is_empty() {
            return Err(TranscriptRefusal::Incomplete);
        }
        let value = strict_json::parse(raw).map_err(|_| TranscriptRefusal::Json)?;
        if value["sessionId"].as_str() != Some(expected.as_str()) {
            return Err(TranscriptRefusal::Session);
        }
        match value["type"].as_str() {
            Some("cost-state") => {
                check_cost(&value)?;
                last_cost = Some(value);
            }
            Some("permission-mode") => {
                exact_fields(&value, &["type", "sessionId", "permissionMode"], &[])
                    .map_err(|_| TranscriptRefusal::Permission)?;
                if value["permissionMode"] != "dontAsk" {
                    return Err(TranscriptRefusal::Permission);
                }
            }
            // Actual SWr221447500 categorizes these as transcript. Their full
            // nested restoration schema is deliberately NOT certified here.
            Some("user" | "assistant" | "system") => unreviewed_conversation_records += 1,
            _ => return Err(TranscriptRefusal::Policy),
        }
        row.clear();
    }
    if !row.is_empty() || original.is_empty() {
        return Err(TranscriptRefusal::Incomplete);
    }
    Ok(TranscriptInspection {
        original,
        records,
        unreviewed_conversation_records,
        last_cost: last_cost.ok_or(TranscriptRefusal::MissingCost)?,
    })
}

fn exact_fields(value: &Value, required: &[&str], optional: &[&str]) -> Result<(), ()> {
    let object = value.as_object().ok_or(())?;
    if required.iter().any(|key| !object.contains_key(*key))
        || object
            .keys()
            .any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(());
    }
    Ok(())
}

fn number(value: &Value, max: f64) -> Result<f64, TranscriptRefusal> {
    value
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= max)
        .ok_or(TranscriptRefusal::Cost)
}

fn check_cost(value: &Value) -> Result<(), TranscriptRefusal> {
    const TIMES: &[&str] = &[
        "totalAPIDuration",
        "totalAPIDurationWithoutRetries",
        "totalToolDuration",
        "totalLinesAdded",
        "totalLinesRemoved",
        "totalDuration",
        "startTime",
    ];
    let mut required = vec!["type", "sessionId", "totalCostUSD", "modelUsage"];
    required.extend_from_slice(TIMES);
    exact_fields(value, &required, &["hasUnknownModelCost"])
        .map_err(|_| TranscriptRefusal::Cost)?;
    number(&value["totalCostUSD"], 1e9)?;
    for name in TIMES {
        number(&value[*name], 1e15)?;
    }
    if value
        .get("hasUnknownModelCost")
        .is_some_and(|v| !v.is_boolean())
    {
        return Err(TranscriptRefusal::Cost);
    }
    let models = value["modelUsage"]
        .as_object()
        .ok_or(TranscriptRefusal::Cost)?;
    let mut channels: [Vec<f64>; 4] = std::array::from_fn(|_| Vec::new());
    for (model, usage) in models {
        // Conservative watched-run binding, stricter than W7's general model
        // regex. No other model or unreviewed alias is silently adopted.
        if model != MODEL {
            return Err(TranscriptRefusal::Cost);
        }
        let fields = [
            "inputTokens",
            "outputTokens",
            "cacheReadInputTokens",
            "cacheCreationInputTokens",
            "webSearchRequests",
            "costUSD",
        ];
        exact_fields(usage, &fields, &["thinkingTokens"]).map_err(|_| TranscriptRefusal::Cost)?;
        for (index, name) in fields.iter().enumerate() {
            let n = number(&usage[*name], 1e15)?;
            if index < 4 {
                channels[index].push(n);
            }
        }
        if let Some(thinking) = usage.get("thinkingTokens") {
            number(thinking, 1e15)?;
        }
    }
    if channels
        .into_iter()
        .any(|values| values.into_iter().sum::<f64>() > 1e15)
    {
        return Err(TranscriptRefusal::Cost);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufReader, Cursor};

    fn session() -> Uuid {
        Uuid::from_u128(1)
    }
    fn cost() -> Value {
        json!({"type":"cost-state","sessionId":session().to_string(),"totalCostUSD":0.2,
            "totalAPIDuration":1,"totalAPIDurationWithoutRetries":1,"totalToolDuration":0,
            "totalLinesAdded":0,"totalLinesRemoved":0,"totalDuration":1,"startTime":1,
            "modelUsage":{MODEL:{"inputTokens":10,"outputTokens":2,"cacheReadInputTokens":3,"cacheCreationInputTokens":4,"webSearchRequests":0,"costUSD":0.2}}})
    }
    fn raw(rows: &[Value]) -> Vec<u8> {
        rows.iter()
            .map(|r| format!("{}\r\n", serde_json::to_string(r).unwrap()))
            .collect::<String>()
            .into_bytes()
    }
    fn refusal(bytes: &[u8]) -> TranscriptRefusal {
        inspect(Cursor::new(bytes), session()).err().unwrap()
    }

    #[test]
    fn exact_bytes_last_cost_and_unreviewed_conversation_are_preserved() {
        let mut second = cost();
        second["totalCostUSD"] = json!(0.3);
        let bytes = raw(&[
            cost(),
            json!({"type":"user","sessionId":session().to_string(),"message":{"content":"synthetic"}}),
            second.clone(),
        ]);
        let result = inspect(BufReader::with_capacity(1, Cursor::new(&bytes)), session()).unwrap();
        assert_eq!(result.original(), bytes);
        assert_eq!(result.records(), 3);
        assert_eq!(result.unreviewed_conversation_records(), 1);
        assert_eq!(result.last_cost_state(), &second);
    }

    #[test]
    fn duplicates_unknown_policy_wrong_session_and_permission_refuse() {
        assert_eq!(
            refusal(
                format!(
                    "{{\"sessionId\":\"{}\",\"type\":\"user\",\"type\":\"cost-state\"}}\n",
                    session()
                )
                .as_bytes()
            ),
            TranscriptRefusal::Json
        );
        for kind in [
            "agent-setting",
            "relocated",
            "worktree-state",
            "dev-mods",
            "memory-mode",
            "bridge-session",
            "fork-context-ref",
            "queue-operation",
            "future-record",
        ] {
            assert_eq!(
                refusal(&raw(&[
                    json!({"type":kind,"sessionId":session().to_string()})
                ])),
                TranscriptRefusal::Policy
            );
        }
        let mut row = cost();
        row["sessionId"] = json!(Uuid::from_u128(2).to_string());
        assert_eq!(refusal(&raw(&[row])), TranscriptRefusal::Session);
        let permission = json!({"type":"permission-mode","sessionId":session().to_string(),"permissionMode":"dontAsk"});
        assert!(inspect(Cursor::new(raw(&[cost(), permission.clone()])), session()).is_ok());
        let mut wrong = permission;
        wrong["permissionMode"] = json!("bypassPermissions");
        assert_eq!(
            refusal(&raw(&[cost(), wrong])),
            TranscriptRefusal::Permission
        );
    }

    #[test]
    fn malformed_cost_is_not_skipped_in_favor_of_earlier_valid_state() {
        for field in ["totalCostUSD", "startTime", "modelUsage"] {
            let mut broken = cost();
            broken[field] = Value::Null;
            assert_eq!(refusal(&raw(&[cost(), broken])), TranscriptRefusal::Cost);
        }
        let mut broken = cost();
        broken["modelUsage"][MODEL]["inputTokens"] = json!(1e16);
        assert_eq!(refusal(&raw(&[broken])), TranscriptRefusal::Cost);
        assert_eq!(
            refusal(&raw(&[
                json!({"type":"user","sessionId":session().to_string()})
            ])),
            TranscriptRefusal::MissingCost
        );
    }

    #[test]
    fn caps_apply_to_original_bytes_rows_and_each_record_before_json_parse() {
        assert_eq!(
            (MAX_TRANSCRIPT_BYTES, MAX_RECORDS, MAX_RECORD_BYTES),
            (33_554_432, 100_000, 2_097_152)
        );
        let bytes = raw(&[cost()]);
        let policy = Limits {
            bytes: bytes.len(),
            records: 1,
            record_bytes: bytes.len(),
        };
        assert!(inspect_with_limits(Cursor::new(&bytes), session(), policy).is_ok());
        for (limits, expected) in [
            (
                Limits {
                    bytes: bytes.len() - 1,
                    ..policy
                },
                TranscriptRefusal::Bytes,
            ),
            (
                Limits {
                    record_bytes: bytes.len() - 1,
                    ..policy
                },
                TranscriptRefusal::RecordBytes,
            ),
            (
                Limits {
                    records: 0,
                    ..policy
                },
                TranscriptRefusal::Records,
            ),
        ] {
            assert_eq!(
                inspect_with_limits(Cursor::new(&bytes), session(), limits).err(),
                Some(expected)
            );
        }
        assert_eq!(
            refusal(&bytes[..bytes.len() - 1]),
            TranscriptRefusal::Incomplete
        );
        assert_eq!(refusal(b"\n"), TranscriptRefusal::Incomplete);
    }
}
