use super::*;
use cna_core::{
    clock::{Anchor, Clock},
    decision::{ActionSchema, ActionSpace, DecisionRequest, Secrecy, Trigger},
    ids::SeatId,
};
use cna_seats::operation::{Allowance, OPERATION_BYTES};
use serde_json::{Value, json};

fn operation() -> Operation {
    Operation::new(
        Uuid::from_u128(19).to_string(),
        1,
        Allowance::required(Some(
            tokio::time::Instant::now() + std::time::Duration::from_secs(5),
        ))
        .unwrap(),
    )
    .unwrap()
}

fn bytes(value: &Value) -> usize {
    let mut storage = CopyStorage::default();
    storage.json(value).unwrap();
    storage.0
}

#[test]
fn lead_json_bound_counts_empty_one_nested_and_array_objects() {
    assert_eq!(
        JSON_MAP_ENTRY_BACKING_BYTES,
        16 * (std::mem::size_of::<String>()
            + std::mem::size_of::<Value>()
            + std::mem::size_of::<usize>())
    );
    assert_eq!(bytes(&json!({})), 0);
    assert_eq!(bytes(&json!({"x": true})), JSON_MAP_ENTRY_BACKING_BYTES + 1);
    assert_eq!(
        bytes(&json!({"x": {"y": "abc"}})),
        2 * JSON_MAP_ENTRY_BACKING_BYTES + 2 + 3
    );
    assert_eq!(
        bytes(&json!([{}, {"x": null}, "ab", 7])),
        4 * std::mem::size_of::<Value>() + JSON_MAP_ENTRY_BACKING_BYTES + 1 + 2
    );
}

fn pending() -> Vec<DecisionRequest> {
    vec![DecisionRequest {
        id: "id".into(),
        seat: SeatId::all().next().unwrap(),
        kind: "kind".into(),
        revision: 2,
        clock: Clock::start(1, Anchor::new("anchor")),
        summary: "summary".into(),
        rules: vec!["case".into()],
        trigger: Trigger::Scheduled,
        secrecy: Secrecy::Secret,
        space: ActionSpace::new(ActionSchema::List {
            item: Box::new(ActionSchema::Bool),
            min: 0,
            max: 1,
        })
        .with_pass("pass")
        .with_context(json!({"key": ["value"]})),
    }]
}

#[test]
fn actual_pending_copy_precharges_typed_children_and_preserves_bytes() {
    let pending = pending();
    let operation = operation();
    let expected = std::mem::size_of::<DecisionRequest>()
        + 2
        + 4
        + 6
        + 7
        + pending[0].rules.capacity() * std::mem::size_of::<String>()
        + 4
        + std::mem::size_of::<ActionSchema>()
        + 4
        + JSON_MAP_ENTRY_BACKING_BYTES
        + 3
        + std::mem::size_of::<Value>()
        + 5;
    let copied = copy_pending(&operation, &pending).unwrap();
    assert_eq!(copied, pending);
    assert_eq!(
        serde_json::to_vec(&copied).unwrap(),
        serde_json::to_vec(&pending).unwrap()
    );
    assert_eq!(operation.captured_bytes(), expected);
    drop(copied);
    assert_eq!(operation.captured_bytes(), expected, "no refund");
    assert!(copy_pending(&operation, &[]).unwrap().is_empty());
    assert_eq!(operation.captured_bytes(), expected);
}

#[test]
fn pending_refuses_before_clone_and_keeps_source_and_sticky_meter() {
    let pending = pending();
    let source_bytes = serde_json::to_vec(&pending).unwrap();
    let mut storage = CopyStorage::default();
    storage.pending(&pending).unwrap();
    let operation = operation();
    operation.charge(OPERATION_BYTES - storage.0 + 1).unwrap();
    let before = operation.captured_bytes();
    assert!(matches!(
        copy_pending(&operation, &pending),
        Err(HostRefusal::CaptureOverflow)
    ));
    assert_eq!(operation.captured_bytes(), before);
    assert_eq!(serde_json::to_vec(&pending).unwrap(), source_bytes);
    assert!(matches!(
        copy_pending(&operation, &[]),
        Err(HostRefusal::CaptureOverflow)
    ));
    assert_eq!(operation.captured_bytes(), before);
}

#[test]
fn canonical_receipt_children_are_prepaid_then_reply_charged_without_refund() {
    let operation = operation();
    let id = "decision";
    let expected =
        2 * id.len() + "accepted decision ".len() + JSON_MAP_ENTRY_BACKING_BYTES + "accepted".len();
    admit_accepted_receipt(&operation, id).unwrap();
    assert_eq!(operation.captured_bytes(), expected);
    let receipt = crate::host_forced::accepted_receipt(id, false);
    assert_eq!(receipt.summary, format!("accepted decision {id}"));
    assert_eq!(receipt.result, json!({"accepted": true}));
    admit_receipt_children(&operation, &receipt).unwrap();
    let copied = receipt.clone();
    assert_eq!(copied, receipt);
    assert_eq!(operation.captured_bytes(), 2 * expected);
    drop(copied);
    assert_eq!(operation.captured_bytes(), 2 * expected);
}

#[test]
fn score_tree_none_empty_and_all_keys_use_approved_bound() {
    use cna_protocol::TranscriptEntry;
    for scores in [None, Some(std::collections::BTreeMap::new())] {
        let entry = TranscriptEntry::System1Answer {
            choice: "chosen".into(),
            scores,
        };
        let operation = operation();
        let copy = copy_transcript_entry(&operation, &entry).unwrap();
        assert_eq!(copy, entry, "None and empty remain distinct");
        assert_eq!(operation.captured_bytes(), "chosen".len());
    }
    let entry = TranscriptEntry::System1Answer {
        choice: "chosen".into(),
        scores: Some(std::collections::BTreeMap::from([
            ("alpha".into(), 0.25),
            ("β".into(), -1.5),
        ])),
    };
    let operation = operation();
    let copied = copy_transcript_entry(&operation, &entry).unwrap();
    assert_eq!(copied, entry);
    assert_eq!(
        operation.captured_bytes(),
        "chosen".len() + 2 * JSON_MAP_ENTRY_BACKING_BYTES + "alpha".len() + "β".len()
    );
    assert_eq!(
        serde_json::to_vec(&copied).unwrap(),
        serde_json::to_vec(&entry).unwrap()
    );
}

#[test]
fn all_transcript_variants_keep_payloads_and_precharge_owned_children() {
    use cna_protocol::{TranscriptEntry, UsageSnapshot};
    let options = vec!["a".into(), "bc".into()];
    let query_bytes = 1 + options.capacity() * std::mem::size_of::<String>() + 1 + 2;
    let snapshot = UsageSnapshot {
        controller_epoch: 2,
        revision: 3,
        provider: Some("p".into()),
        model: Some("m".into()),
        attempts: 1,
        completed: 1,
        input_tokens: None,
        output_tokens: Some(2),
        cache_read_tokens: None,
        cache_creation_tokens: None,
        reasoning_tokens: None,
        reported_cost_usd: None,
        incomplete_turns: 0,
    };
    let examples = [
        (TranscriptEntry::AssistantText { text: "ab".into() }, 2),
        (TranscriptEntry::Reasoning { text: "cd".into() }, 2),
        (TranscriptEntry::System { text: "ef".into() }, 2),
        (
            TranscriptEntry::ToolCall {
                call_id: "c".into(),
                tool: "t".into(),
                args: json!({"k": "v"}),
            },
            2 + JSON_MAP_ENTRY_BACKING_BYTES + 2,
        ),
        (
            TranscriptEntry::ToolResult {
                call_id: "c".into(),
                ok: true,
                summary: "s".into(),
                detail: Some(json!(["d"])),
            },
            2 + std::mem::size_of::<Value>() + 1,
        ),
        (
            TranscriptEntry::DecisionSubmitted {
                decision_id: "d".into(),
                summary: "s".into(),
            },
            2,
        ),
        (
            TranscriptEntry::System1Query {
                question: "q".into(),
                options,
            },
            query_bytes,
        ),
        (
            TranscriptEntry::System1Answer {
                choice: "a".into(),
                scores: None,
            },
            1,
        ),
        (
            TranscriptEntry::UsageSnapshot(Box::new(snapshot)),
            std::mem::size_of::<UsageSnapshot>() + 2,
        ),
    ];
    for (entry, expected) in examples {
        let operation = operation();
        let copied = copy_transcript_entry(&operation, &entry).unwrap();
        assert_eq!(copied, entry);
        assert_eq!(operation.captured_bytes(), expected);
        drop(copied);
        assert_eq!(operation.captured_bytes(), expected, "no refund after drop");
    }
}

fn transcript_row() -> UnconfirmedEntry {
    UnconfirmedEntry {
        seat: SeatId::all().next().unwrap(),
        at: "at".into(),
        entry: cna_protocol::TranscriptEntry::ToolCall {
            call_id: "c".into(),
            tool: "t".into(),
            args: json!({"k": "v"}),
        },
        origin: Some(cna_seats::operation::CaptureOrigin {
            operation_id: "original".into(),
            child_generation: 7,
            ordinal: 11,
        }),
    }
}

#[test]
fn queued_row_and_entry_copy_charge_independently_on_same_meter() {
    let operation = operation();
    let row = transcript_row();
    let entry_bytes = 2 + JSON_MAP_ENTRY_BACKING_BYTES + 2;
    let row_bytes = entry_bytes + "at".len() + "original".len();
    let queued = copy_delivery_row(&operation, &row).unwrap();
    assert_eq!(operation.captured_bytes(), row_bytes);
    let entry = copy_transcript_entry(&operation, &queued.entry).unwrap();
    assert_eq!(operation.captured_bytes(), row_bytes + entry_bytes);
    assert_eq!(queued, row);
    assert_eq!(entry, row.entry);
    drop(queued);
    drop(entry);
    assert_eq!(
        operation.captured_bytes(),
        row_bytes + entry_bytes,
        "both copies stay prepaid"
    );
}

#[test]
fn transcript_copy_overflow_preserves_queued_prefix_and_sticky_refusal() {
    let row = transcript_row();
    let original_bytes = serde_json::to_vec(&row).unwrap();
    let mut row_storage = CopyStorage::default();
    row_storage.row(&row).unwrap();
    let mut entry_storage = CopyStorage::default();
    entry_storage.transcript(&row.entry).unwrap();
    let operation = operation();
    operation
        .charge(OPERATION_BYTES - row_storage.0 - entry_storage.0 + 1)
        .unwrap();
    let queued = copy_delivery_row(&operation, &row).unwrap();
    let before = operation.captured_bytes();
    assert!(matches!(
        copy_transcript_entry(&operation, &queued.entry),
        Err(HostRefusal::CaptureOverflow)
    ));
    assert_eq!(operation.captured_bytes(), before);
    assert_eq!(serde_json::to_vec(&queued).unwrap(), original_bytes);
    assert_eq!(serde_json::to_vec(&row).unwrap(), original_bytes);
    assert!(matches!(
        copy_delivery_row(&operation, &row),
        Err(HostRefusal::CaptureOverflow)
    ));
    assert_eq!(operation.captured_bytes(), before);
}

#[test]
fn queued_row_refuses_before_its_first_clone_without_partial_growth() {
    let row = transcript_row();
    let original_bytes = serde_json::to_vec(&row).unwrap();
    let mut storage = CopyStorage::default();
    storage.row(&row).unwrap();
    let operation = operation();
    operation.charge(OPERATION_BYTES - storage.0 + 1).unwrap();
    let before = operation.captured_bytes();
    assert!(matches!(
        copy_delivery_row(&operation, &row),
        Err(HostRefusal::CaptureOverflow)
    ));
    assert_eq!(operation.captured_bytes(), before);
    assert_eq!(serde_json::to_vec(&row).unwrap(), original_bytes);
    assert!(matches!(
        copy_transcript_entry(&operation, &row.entry),
        Err(HostRefusal::CaptureOverflow)
    ));
    assert_eq!(operation.captured_bytes(), before);
}
