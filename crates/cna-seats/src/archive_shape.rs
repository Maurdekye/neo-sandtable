//! Fixed, borrowed preflight for the recursive fields in archive evidence.
//! This bounds serializer nesting, not body length, custody or actual stack fit.
//! An ineligible original value stays canonical; no truncated replacement exists.

use cna_protocol::TranscriptEntry;
use serde_json::Value;

use super::ArchiveError;

// Use the standard JSON reader's 128-container nesting budget for archive
// serialization too. Retained Values can also be constructed directly, so
// their type alone provides NO finite depth bound; validate every actual value.
// This is an archive stack-fit refusal, not a change to game/protocol payloads.
pub(crate) const MAX_ARCHIVE_JSON_CONTAINERS: usize = 128;

enum Children<'a> {
    Array(std::slice::Iter<'a, Value>),
    Object(serde_json::map::Values<'a>),
}

impl<'a> Children<'a> {
    fn next(&mut self) -> Option<&'a Value> {
        match self {
            Self::Array(values) => values.next(),
            Self::Object(values) => values.next(),
        }
    }
}

/// Include the entire iterative walk state in ORIGINAL creation-time scratch.
/// One frame per ancestor, never one per sibling or body-sized collection.
pub(crate) const fn shape_workspace_bytes() -> usize {
    std::mem::size_of::<[Option<Children<'static>>; MAX_ARCHIVE_JSON_CONTAINERS]>()
        + std::mem::size_of::<Option<&'static Value>>()
        + std::mem::size_of::<usize>()
}

pub(crate) fn check_value_shape(value: &Value) -> Result<(), ArchiveError> {
    let mut ancestors: [Option<Children<'_>>; MAX_ARCHIVE_JSON_CONTAINERS] =
        std::array::from_fn(|_| None);
    let mut depth = 0;
    let mut current = Some(value);
    loop {
        if let Some(value) = current.take() {
            let children = match value {
                Value::Array(values) => Some(Children::Array(values.iter())),
                Value::Object(values) => Some(Children::Object(values.values())),
                _ => None,
            };
            if let Some(children) = children {
                if depth == ancestors.len() {
                    return Err(ArchiveError::Fit);
                }
                ancestors[depth] = Some(children);
                depth += 1;
            }
        }
        loop {
            if depth == 0 {
                return Ok(());
            }
            if let Some(next) = ancestors[depth - 1].as_mut().and_then(Children::next) {
                current = Some(next);
                break;
            }
            depth -= 1;
            ancestors[depth] = None;
        }
    }
}

/// These are the ONLY recursively typed fields in the current TranscriptEntry.
/// Keep the match exhaustive so a future protocol variant needs an explicit
/// shape decision. Other variants contain scalars/strings or flat collections.
pub(crate) fn check_entry_shape(entry: &TranscriptEntry) -> Result<(), ArchiveError> {
    match entry {
        TranscriptEntry::ToolCall { args, .. } => check_value_shape(args),
        TranscriptEntry::ToolResult {
            detail: Some(detail),
            ..
        } => check_value_shape(detail),
        TranscriptEntry::AssistantText { .. }
        | TranscriptEntry::Reasoning { .. }
        | TranscriptEntry::ToolResult { detail: None, .. }
        | TranscriptEntry::DecisionSubmitted { .. }
        | TranscriptEntry::System { .. }
        | TranscriptEntry::System1Query { .. }
        | TranscriptEntry::System1Answer { .. }
        | TranscriptEntry::UsageSnapshot(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(containers: usize) -> Value {
        let mut value = Value::String("original unchanged".into());
        for _ in 0..containers {
            value = Value::Array(vec![value]); // Synthetic fixture construction only.
        }
        value
    }

    #[test]
    fn exact_depth_boundary_refuses_without_changing_original_body() {
        let allowed = nested(MAX_ARCHIVE_JSON_CONTAINERS);
        assert!(check_value_shape(&allowed).is_ok());
        let refused = nested(MAX_ARCHIVE_JSON_CONTAINERS + 1);
        let original = std::ptr::from_ref(&refused);
        assert!(matches!(
            check_value_shape(&refused),
            Err(ArchiveError::Fit)
        ));
        assert_eq!(original, std::ptr::from_ref(&refused));
        let mut leaf = &refused;
        for _ in 0..=MAX_ARCHIVE_JSON_CONTAINERS {
            leaf = &leaf.as_array().unwrap()[0];
        }
        assert_eq!(leaf.as_str(), Some("original unchanged"));
    }

    #[test]
    fn width_and_mixed_containers_visit_every_borrowed_suffix() {
        let mut object = serde_json::Map::new();
        object.insert("first".into(), nested(4));
        object.insert("last".into(), nested(MAX_ARCHIVE_JSON_CONTAINERS));
        let original = Value::Array(vec![Value::Object(object)]);
        assert!(matches!(
            check_value_shape(&original),
            Err(ArchiveError::Fit)
        ));
        let wide = Value::Array((0..1024).map(|_| nested(2)).collect());
        let pointer = wide.as_array().unwrap().as_ptr();
        assert!(check_value_shape(&wide).is_ok());
        assert_eq!(pointer, wide.as_array().unwrap().as_ptr());
        assert!(shape_workspace_bytes() < super::super::FIXED_ARCHIVE_WORKSPACE);
    }

    #[test]
    fn both_recursive_entry_fields_are_checked_without_protocol_rewrite() {
        let call = TranscriptEntry::ToolCall {
            call_id: "original".into(),
            tool: "observe".into(),
            args: nested(MAX_ARCHIVE_JSON_CONTAINERS + 1),
        };
        let result = TranscriptEntry::ToolResult {
            call_id: "original".into(),
            ok: false,
            summary: "retained".into(),
            detail: Some(nested(MAX_ARCHIVE_JSON_CONTAINERS + 1)),
        };
        assert!(matches!(check_entry_shape(&call), Err(ArchiveError::Fit)));
        assert!(matches!(check_entry_shape(&result), Err(ArchiveError::Fit)));
        assert!(
            check_entry_shape(&TranscriptEntry::ToolResult {
                call_id: "original".into(),
                ok: false,
                summary: "retained".into(),
                detail: None,
            })
            .is_ok()
        );
    }
}
