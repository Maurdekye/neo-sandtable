//! Narrow installed-writer row/content checks, NOT complete resume admission.
//! Ordinary API requestRef/wire/batch/timing and tool-result wrapper metadata
//! remain unsupported. Callers owe run/seat/file/park binding and effective graph.
use super::{exact_fields, restore_parts::RawRow};
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RowRefusal {
    #[error("unknown or unsupported row fields")]
    Fields,
    #[error("transcript row differs from launcher binding")]
    Binding,
    #[error("invalid canonical transcript UUID or timestamp")]
    Identity,
    #[error("unsupported user text content")]
    UserContent,
    #[error("unsupported assistant content or tool binding")]
    AssistantContent,
}

/// These optional fields bind absence exactly, never null-as-absence. Values
/// must be source-bound launcher/park records, not adopted from an arbitrary row.
/// This helper does not establish how sessionKind/entrypoint were generated.
pub struct RowBindings<'a> {
    pub session: Uuid,
    pub cwd: &'a str,
    pub session_kind: Option<&'a str>,
    pub entrypoint: Option<&'a str>,
    pub git_branch: Option<&'a str>,
    pub slug: Option<&'a str>,
}

fn bound_optional(row: &Value, key: &str, expected: Option<&str>) -> bool {
    match (row.get(key), expected) {
        (None, None) => true,
        (Some(Value::String(actual)), Some(expected)) => actual == expected,
        _ => false,
    }
}
fn uuid(value: &Value) -> Result<Uuid, RowRefusal> {
    let text = value.as_str().ok_or(RowRefusal::Identity)?;
    let parsed = Uuid::parse_str(text).map_err(|_| RowRefusal::Identity)?;
    if parsed.to_string() != text {
        return Err(RowRefusal::Identity);
    }
    Ok(parsed)
}
fn timestamp(value: &Value) -> Result<(), RowRefusal> {
    let text = value.as_str().ok_or(RowRefusal::Identity)?;
    let time = chrono::DateTime::parse_from_rfc3339(text).map_err(|_| RowRefusal::Identity)?;
    if time.offset().local_minus_utc() != 0
        || time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string() != text
    {
        return Err(RowRefusal::Identity);
    }
    Ok(())
}
fn text_content(value: &Value) -> Result<(), RowRefusal> {
    if value.is_string() {
        return Ok(());
    }
    let blocks = value.as_array().ok_or(RowRefusal::UserContent)?;
    if blocks.is_empty() {
        return Err(RowRefusal::UserContent);
    }
    for block in blocks {
        exact_fields(block, &["type", "text"], &[]).map_err(|_| RowRefusal::UserContent)?;
        if block["type"] != "text" || !block["text"].is_string() {
            return Err(RowRefusal::UserContent);
        }
    }
    Ok(())
}

/// Only the plain user-text Ae221300796 + Wtr221472021/main221507687
/// writer subset. Summary/tool-result/virtual/denial/policy/agent variants refuse.
/// No fallback role coercion, timestamp repair or row-field stripping.
pub fn check_plain_user(row: &Value, binding: &RowBindings<'_>) -> Result<RawRow, RowRefusal> {
    exact_fields(
        row,
        &[
            "type",
            "uuid",
            "timestamp",
            "parentUuid",
            "isSidechain",
            "message",
            "userType",
            "cwd",
            "sessionId",
            "version",
        ],
        &[
            "isMeta",
            "permissionMode",
            "sessionKind",
            "entrypoint",
            "gitBranch",
            "slug",
        ],
    )
    .map_err(|_| RowRefusal::Fields)?;
    if row["type"] != "user"
        || row["isSidechain"] != false
        || row["userType"] != "external"
        || row["cwd"].as_str() != Some(binding.cwd)
        || row["version"] != "2.1.293"
        || uuid(&row["sessionId"])? != binding.session
        || !bound_optional(row, "sessionKind", binding.session_kind)
        || !bound_optional(row, "entrypoint", binding.entrypoint)
        || !bound_optional(row, "gitBranch", binding.git_branch)
        || !bound_optional(row, "slug", binding.slug)
        || row.get("isMeta").is_some_and(|v| v != false)
        || row.get("permissionMode").is_some_and(|v| v != "dontAsk")
    {
        return Err(RowRefusal::Binding);
    }
    let id = uuid(&row["uuid"])?;
    let parent = if row["parentUuid"].is_null() {
        None
    } else {
        Some(uuid(&row["parentUuid"])?)
    };
    if parent == Some(id) {
        return Err(RowRefusal::Identity);
    }
    timestamp(&row["timestamp"])?;
    exact_fields(&row["message"], &["role", "content"], &[])
        .map_err(|_| RowRefusal::UserContent)?;
    if row["message"]["role"] != "user" {
        return Err(RowRefusal::UserContent);
    }
    text_content(&row["message"]["content"])?;
    Ok(RawRow {
        uuid: id,
        parent,
        logical_parent: None,
        assistant_message_id: None,
    })
}

pub struct CheckedToolUse {
    pub id: String,
    pub name: String,
}

/// Assistant CONTENT leaf only: not the normal API row or message envelope.
/// The trusted pure checker must bind each full MCP name AND input to this run's
/// exact tool definitions/argument schemas; missing recorded action schema must
/// refuse. A prefix check alone cannot satisfy this callback's contract.
pub fn check_assistant_content(
    content: &Value,
    tool_check: impl Fn(&str, &Value) -> bool,
) -> Result<Vec<CheckedToolUse>, RowRefusal> {
    let refused = || RowRefusal::AssistantContent;
    let blocks = content.as_array().ok_or_else(refused)?;
    if blocks.is_empty() {
        return Err(refused());
    }
    let mut calls = Vec::new();
    let mut ids = BTreeSet::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => {
                exact_fields(block, &["type", "text"], &[]).map_err(|_| refused())?;
                if !block["text"].is_string() {
                    return Err(refused());
                }
            }
            Some("thinking") => {
                exact_fields(block, &["type", "thinking", "signature"], &[])
                    .map_err(|_| refused())?;
                if !block["thinking"].is_string() || !block["signature"].is_string() {
                    return Err(refused());
                }
            }
            Some("redacted_thinking") => {
                exact_fields(block, &["type", "data"], &[]).map_err(|_| refused())?;
                if !block["data"].is_string() {
                    return Err(refused());
                }
            }
            Some("tool_use") => {
                exact_fields(block, &["type", "id", "name", "input"], &[])
                    .map_err(|_| refused())?;
                let id = block["id"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(refused)?;
                let name = block["name"].as_str().ok_or_else(refused)?;
                if !ids.insert(id)
                    || !name.starts_with("mcp__cna__")
                    || !block["input"].is_object()
                    || !tool_check(name, &block["input"])
                {
                    return Err(refused());
                }
                calls.push(CheckedToolUse {
                    id: id.into(),
                    name: name.into(),
                });
            }
            _ => return Err(refused()),
        }
    }
    Ok(calls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn user() -> Value {
        json!({"type":"user","uuid":Uuid::from_u128(2).to_string(),"timestamp":"2026-10-20T00:00:00.000Z","parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"Synthetic briefing"},"userType":"external","cwd":"C:/synthetic-seat","sessionId":Uuid::from_u128(1).to_string(),"version":"2.1.293"})
    }
    fn binding() -> RowBindings<'static> {
        RowBindings {
            session: Uuid::from_u128(1),
            cwd: "C:/synthetic-seat",
            session_kind: None,
            entrypoint: None,
            git_branch: None,
            slug: None,
        }
    }
    #[test]
    fn known_plain_user_binding_and_exact_absence_without_row_repair() {
        let good = user();
        assert!(check_plain_user(&good, &binding()).is_ok());
        for key in ["entrypoint", "sessionKind", "gitBranch", "slug"] {
            let mut bad = good.clone();
            bad[key] = Value::Null;
            assert!(matches!(
                check_plain_user(&bad, &binding()),
                Err(RowRefusal::Binding)
            ));
        }
        for key in [
            "agentId",
            "origin",
            "mcpMeta",
            "logicalParentUuid",
            "isCompactSummary",
            "wireToolInputs",
        ] {
            let mut bad = good.clone();
            bad[key] = json!("unreviewed");
            assert!(matches!(
                check_plain_user(&bad, &binding()),
                Err(RowRefusal::Fields)
            ));
        }
        let mut bad = good.clone();
        bad["timestamp"] = json!("2026-10-20T00:00:00+00:00");
        assert!(matches!(
            check_plain_user(&bad, &binding()),
            Err(RowRefusal::Identity)
        ));
        let mut bad = good;
        bad["isSidechain"] = json!(true);
        assert!(matches!(
            check_plain_user(&bad, &binding()),
            Err(RowRefusal::Binding)
        ));
    }
    #[test]
    fn user_blocks_require_exact_text_shape_no_loader_coercion() {
        let mut row = user();
        row["message"]["content"] = json!([{"type":"text","text":"briefing"}]);
        assert!(check_plain_user(&row, &binding()).is_ok());
        row["message"]["content"] = json!([{"type":"image","source":{}}]);
        assert!(matches!(
            check_plain_user(&row, &binding()),
            Err(RowRefusal::UserContent)
        ));
        row["message"]["content"] = json!([{"type":"text","text":null}]);
        assert!(matches!(
            check_plain_user(&row, &binding()),
            Err(RowRefusal::UserContent)
        ));
    }
    #[test]
    fn assistant_leaf_requires_exact_tool_and_schema_not_prefix() {
        let allow = |name: &str, args: &Value| name == "mcp__cna__observe" && args == &json!({});
        let good = json!([{"type":"tool_use","id":"call1","name":"mcp__cna__observe","input":{}}]);
        let checked = check_assistant_content(&good, allow).unwrap();
        assert_eq!(checked[0].id, "call1");
        for (name, input) in [
            ("mcp__cna__foreign", json!({})),
            ("mcp__cna__observe", json!({"seat":"other"})),
            ("Bash", json!({})),
        ] {
            let bad = json!([{"type":"tool_use","id":"call1","name":name,"input":input}]);
            assert!(check_assistant_content(&bad, allow).is_err());
        }
        let duplicate = json!([good[0].clone(), good[0].clone()]);
        assert!(check_assistant_content(&duplicate, allow).is_err());
        let unknown = json!([{"type":"text","text":"hi","citations":[]}]);
        assert!(check_assistant_content(&unknown, allow).is_err());
    }
}
