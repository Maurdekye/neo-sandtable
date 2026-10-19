//! Source-derived restore building blocks, NOT a row schema or resume admission.
//! Callers still owe exact outer writer envelopes, run/seat/park bindings, full
//! sibling/compaction reconstruction and exact file-tree adoption. Original
//! transcript bytes are never changed by these helpers.
use super::exact_fields;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RestorePartRefusal {
    #[error("unsupported game MCP result envelope")]
    McpEnvelope,
    #[error("external or transformed tool result is unsupported")]
    ExternalResult,
    #[error("date is not bound to a launcher-recorded historical date")]
    Date,
    #[error("cached date rendering differs from the installed renderer")]
    Rendering,
    #[error("duplicate transcript row UUID")]
    DuplicateUuid,
    #[error("self-parent or cyclic raw conversation graph")]
    Cycle,
    #[error("raw graph record cap exceeded")]
    GraphCap,
}

/// Validate ONLY the server-result value nested inside a future typed CLI row.
/// mcp.rs tools/call emits this exact envelope. A successful check is not a
/// check of toolUseResult/mcpMeta's CLI wrapper, its tool binding or its graph.
/// Return JS UTF-16 text length for the future source-bound group accounting.
/// Installed default offload thresholds are not universal enforcement bounds.
pub fn check_game_result(value: &Value) -> Result<usize, RestorePartRefusal> {
    let refused = || RestorePartRefusal::McpEnvelope;
    exact_fields(value, &["content", "isError"], &[]).map_err(|_| refused())?;
    if !value["isError"].is_boolean() {
        return Err(refused());
    }
    let content = value["content"].as_array().ok_or_else(refused)?;
    if content.len() != 1 {
        return Err(refused());
    }
    let block = &content[0];
    exact_fields(block, &["type", "text"], &[]).map_err(|_| refused())?;
    if block["type"] != "text" {
        return Err(refused());
    }
    let text = block["text"].as_str().ok_or_else(refused)?;
    // CLI2.1.293 Kke215306827 and constants215304572. Single-tool
    // offload can appear without a content-replacement row. Do not strip it.
    for marker in [
        "<persisted-output>",
        "</persisted-output>",
        "<truncated-output>",
        "</truncated-output>",
        "[Old tool result content cleared]",
    ] {
        if text.contains(marker) {
            return Err(RestorePartRefusal::ExternalResult);
        }
    }
    Ok(text.encode_utf16().count())
}

/// Validate the date payload/cache ONLY; no other attachment subtype or outer
/// row fields are certified. The allowlist must come from a source-bound clock,
/// timezone and launcher-recorded run span. No such allowlist is created here.
/// Missing cached data is valid only when ALL cache fields are absent: the
/// inspected CLI can then deterministically recompute the same date reminder.
/// Sources: fq217142974, cst217143686, h8221391587, Ka221360102/Ml221357502.
pub fn check_date_payload(
    row: &Value,
    permitted_dates: &BTreeSet<String>,
) -> Result<(), RestorePartRefusal> {
    let date_error = || RestorePartRefusal::Date;
    if row["type"] != "attachment" {
        return Err(date_error());
    }
    let attachment = &row["attachment"];
    exact_fields(attachment, &["type", "date"], &["changed"]).map_err(|_| date_error())?;
    if attachment["type"] != "date" {
        return Err(date_error());
    }
    let date = attachment["date"].as_str().ok_or_else(date_error)?;
    let parsed = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| date_error())?;
    if parsed.format("%Y-%m-%d").to_string() != date || !permitted_dates.contains(date) {
        return Err(date_error());
    }
    let changed = match attachment.get("changed") {
        None => false,
        Some(Value::Bool(changed)) => *changed,
        _ => return Err(date_error()),
    };
    let text = if changed {
        format!(
            "The date has changed. Today's date is now {date}. No need to announce the new date \u{2014} the user's own clock shows it."
        )
    } else {
        format!("Today's date is {date}.")
    };
    let expected = json!([{"content": format!("<system-reminder>\n{text}\n</system-reminder>")}]);
    let cached = [
        "rendered",
        "renderedRole",
        "renderedInHumanTurn",
        "renderedBesideToolResult",
    ];
    if cached.iter().all(|key| row.get(*key).is_none()) {
        return Ok(());
    }
    if row.get("rendered") != Some(&expected)
        || row.get("renderedRole").and_then(Value::as_str) != Some("system")
        || row.get("renderedInHumanTurn").is_some()
        || row.get("renderedBesideToolResult").is_some()
    {
        return Err(RestorePartRefusal::Rendering);
    }
    Ok(())
}

/// One source-validated conversation row's graph fields. This does not parse
/// unknown writer metadata or classify user/assistant/compaction variants.
/// Multiple assistant fragments may share message.id; row UUIDs cannot repeat.
#[derive(Clone)]
pub struct RawRow {
    pub uuid: Uuid,
    pub parent: Option<Uuid>,
    pub logical_parent: Option<Uuid>,
    pub assistant_message_id: Option<String>,
}

pub struct RawGraph {
    rows: Vec<RawRow>,
    unresolved_parents: BTreeSet<Uuid>,
}
impl RawGraph {
    pub fn rows(&self) -> &[RawRow] {
        &self.rows
    }
    /// Not silently adopted as active references. Later source-complete
    /// compaction/fragment selection must settle these before parked admission.
    pub fn unresolved_parents(&self) -> &BTreeSet<Uuid> {
        &self.unresolved_parents
    }
}

/// Check unique UUIDs and acyclic PRESENT raw parent edges without assuming
/// file order or one linear parent chain. logicalParentUuid is provenance and
/// may be absent from retained history; it is never traversed as a live edge.
/// No graph relinking, timestamp repair, fragment collapse or byte rewrite.
pub fn check_raw_graph(rows: &[RawRow]) -> Result<RawGraph, RestorePartRefusal> {
    if rows.len() > super::MAX_RECORDS {
        return Err(RestorePartRefusal::GraphCap);
    }
    let mut by_uuid = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        if by_uuid.insert(row.uuid, index).is_some() {
            return Err(RestorePartRefusal::DuplicateUuid);
        }
        if row.parent == Some(row.uuid) {
            return Err(RestorePartRefusal::Cycle);
        }
    }
    let unresolved_parents = rows
        .iter()
        .filter_map(|row| row.parent)
        .filter(|parent| !by_uuid.contains_key(parent))
        .collect();
    let mut colors = vec![0u8; rows.len()];
    for start in 0..rows.len() {
        if colors[start] != 0 {
            continue;
        }
        let mut path = Vec::new();
        let mut next = Some(start);
        while let Some(index) = next {
            match colors[index] {
                1 => return Err(RestorePartRefusal::Cycle),
                2 => break,
                _ => {
                    colors[index] = 1;
                    path.push(index);
                }
            }
            next = rows[index]
                .parent
                .and_then(|parent| by_uuid.get(&parent).copied());
        }
        for index in path {
            colors[index] = 2;
        }
    }
    Ok(RawGraph {
        rows: rows.to_vec(),
        unresolved_parents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_game_result_shape_counts_utf16_and_refuses_flow_metadata_and_references() {
        let value = json!({"content":[{"type":"text","text":"\u{1f600}x"}],"isError":false});
        assert_eq!(check_game_result(&value), Ok(3));
        for key in ["_meta", "structuredContent", "resources", "resourceLinks"] {
            let mut bad = value.clone();
            bad[key] = json!({});
            assert_eq!(
                check_game_result(&bad),
                Err(RestorePartRefusal::McpEnvelope)
            );
        }
        let reference = json!({"content":[{"type":"text","text":"<persisted-output>elsewhere</persisted-output>"}],"isError":false});
        assert_eq!(
            check_game_result(&reference),
            Err(RestorePartRefusal::ExternalResult)
        );
        let mut wrong = value;
        wrong["isError"] = json!(null);
        assert_eq!(
            check_game_result(&wrong),
            Err(RestorePartRefusal::McpEnvelope)
        );
    }

    #[test]
    fn cached_date_requires_exact_rendering_and_source_bound_historical_allowlist() {
        let allowed = BTreeSet::from(["2026-10-20".to_string()]);
        let mut row = json!({"type":"attachment","attachment":{"type":"date","date":"2026-10-20"}});
        assert_eq!(check_date_payload(&row, &allowed), Ok(()));
        assert_eq!(
            check_date_payload(&row, &BTreeSet::new()),
            Err(RestorePartRefusal::Date)
        );
        row["rendered"] = json!([{"content":"<system-reminder>\nToday's date is 2026-10-20.\n</system-reminder>"}]);
        row["renderedRole"] = json!("system");
        assert_eq!(check_date_payload(&row, &allowed), Ok(()));
        row["renderedBesideToolResult"] = json!(false);
        assert_eq!(
            check_date_payload(&row, &allowed),
            Err(RestorePartRefusal::Rendering)
        );
        row.as_object_mut()
            .unwrap()
            .remove("renderedBesideToolResult");
        row["attachment"]["changed"] = json!(true);
        assert_eq!(
            check_date_payload(&row, &allowed),
            Err(RestorePartRefusal::Rendering)
        );
        row["rendered"] = json!([{"content":"<system-reminder>\nThe date has changed. Today's date is now 2026-10-20. No need to announce the new date \u{2014} the user's own clock shows it.\n</system-reminder>"}]);
        assert_eq!(check_date_payload(&row, &allowed), Ok(()));
        row["renderedRole"] = json!("user");
        assert_eq!(
            check_date_payload(&row, &allowed),
            Err(RestorePartRefusal::Rendering)
        );
    }

    fn row(id: u128, parent: Option<u128>) -> RawRow {
        RawRow {
            uuid: Uuid::from_u128(id),
            parent: parent.map(Uuid::from_u128),
            logical_parent: None,
            assistant_message_id: None,
        }
    }
    #[test]
    fn graph_preserves_sibling_fragments_and_ignores_historical_logical_parent() {
        let mut first = row(2, Some(1));
        first.assistant_message_id = Some("same-api-message".into());
        let mut sibling = row(3, Some(1));
        sibling.assistant_message_id = first.assistant_message_id.clone();
        sibling.logical_parent = Some(Uuid::from_u128(99));
        let graph = check_raw_graph(&[row(1, None), first, sibling, row(4, Some(2))]).unwrap();
        assert_eq!(graph.rows().len(), 4);
        assert_eq!(
            graph.rows()[1].assistant_message_id,
            graph.rows()[2].assistant_message_id
        );
        assert!(graph.unresolved_parents().is_empty());
        let unresolved = check_raw_graph(&[row(4, Some(99))]).unwrap();
        assert_eq!(
            unresolved.unresolved_parents(),
            &BTreeSet::from([Uuid::from_u128(99)])
        );
    }
    #[test]
    fn duplicate_rows_self_parent_and_cycles_refuse_without_repair() {
        assert!(matches!(
            check_raw_graph(&[row(1, None), row(1, None)]),
            Err(RestorePartRefusal::DuplicateUuid)
        ));
        assert!(matches!(
            check_raw_graph(&[row(1, Some(1))]),
            Err(RestorePartRefusal::Cycle)
        ));
        assert!(matches!(
            check_raw_graph(&[row(1, Some(2)), row(2, Some(3)), row(3, Some(1))]),
            Err(RestorePartRefusal::Cycle)
        ));
    }
}
