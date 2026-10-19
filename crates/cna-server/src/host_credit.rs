//! Encoded host envelopes. These trusted records are not spectator messages.
//!
//! Admission counts complete escaped JSON before accepting or cloning a body.
//! Encoded limits are not a claim about allocator RSS or storage settlement.
use cna_seats::{
    game::{SubmitReceipt, SubmitRequest},
    host::{HostIdentity, HostRefusal, operation_refusal},
    operation::{COMMAND_BYTES, DELIVERY_BYTES, Operation, encoded_len},
    transcript::UnconfirmedEntry,
};
use serde::Serialize;
use uuid::Uuid;

#[cfg(test)]
#[path = "host_copy_tests.rs"]
mod copy_tests;

/// Lead-set structural bound, mddec93a18791: 16 * (String + Value + usize)
/// per JSON object entry (1024 bytes on 64-bit). This is not a mirror of private
/// map layout; no library allocation proof is claimed. Map storage is INSIDE G.
pub(crate) const JSON_MAP_ENTRY_BACKING_BYTES: usize = 16
    * (std::mem::size_of::<String>()
        + std::mem::size_of::<serde_json::Value>()
        + std::mem::size_of::<usize>());

/// Allocation-free borrowed walk. Inline root bodies belong to original F;
/// this accumulator covers newly copied children before any clone/allocation.
#[derive(Default)]
struct CopyStorage(usize);
impl CopyStorage {
    fn add(&mut self, bytes: usize) -> Option<()> {
        self.0 = self.0.checked_add(bytes)?;
        Some(())
    }
    fn elements<T>(&mut self, count: usize) -> Option<()> {
        self.add(count.checked_mul(std::mem::size_of::<T>())?)
    }
    fn json(&mut self, value: &serde_json::Value) -> Option<()> {
        use serde_json::Value;
        match value {
            Value::String(text) => self.add(text.len()),
            Value::Array(items) => {
                self.elements::<Value>(items.len())?;
                for item in items {
                    self.json(item)?;
                }
                Some(())
            }
            Value::Object(entries) => {
                for (key, value) in entries {
                    self.add(JSON_MAP_ENTRY_BACKING_BYTES)?;
                    self.add(key.len())?;
                    self.json(value)?;
                }
                Some(())
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => Some(()),
        }
    }
    fn schema(&mut self, schema: &cna_core::decision::ActionSchema) -> Option<()> {
        use cna_core::decision::{ActionSchema, ChoiceOption, FieldSchema};
        match schema {
            ActionSchema::Choice { options } => {
                self.elements::<ChoiceOption>(options.capacity())?;
                for option in options {
                    self.add(option.id.len())?;
                    self.add(option.label.len())?;
                    self.add(option.detail.as_ref().map_or(0, String::len))?;
                }
            }
            ActionSchema::Unit { among } => {
                self.elements::<cna_core::ids::UnitId>(among.capacity())?;
                for id in among {
                    self.add(id.as_str().len())?;
                }
            }
            ActionSchema::Hex { among: Some(among) } => {
                self.elements::<cna_core::ids::HexId>(among.capacity())?;
                for id in among {
                    self.add(id.as_str().len())?;
                }
            }
            ActionSchema::Path { from, .. } => {
                self.add(from.as_str().len())?;
            }
            ActionSchema::Record { fields } => {
                self.elements::<FieldSchema>(fields.capacity())?;
                for field in fields {
                    self.add(field.name.len())?;
                    self.add(field.doc.len())?;
                    self.schema(&field.schema)?;
                }
            }
            ActionSchema::List { item, .. } => {
                self.elements::<ActionSchema>(1)?;
                self.schema(item)?;
            }
            ActionSchema::Integer { .. }
            | ActionSchema::Bool
            | ActionSchema::Hex { among: None }
            | ActionSchema::Text { .. } => {}
        }
        Some(())
    }
    fn pending(&mut self, pending: &[cna_core::decision::DecisionRequest]) -> Option<()> {
        // Returned Vec is created with exactly this element capacity.
        self.elements::<cna_core::decision::DecisionRequest>(pending.len())?;
        for request in pending {
            self.add(request.id.as_str().len())?;
            self.add(request.kind.len())?;
            self.add(request.summary.len())?;
            self.add(request.clock.anchor.as_str().len())?;
            self.elements::<String>(request.rules.capacity())?;
            for rule in &request.rules {
                self.add(rule.len())?;
            }
            self.schema(&request.space.schema)?;
            self.add(request.space.pass.as_ref().map_or(0, String::len))?;
            if let Some(context) = &request.space.context {
                self.json(context)?;
            }
        }
        Some(())
    }
    fn transcript(&mut self, entry: &cna_protocol::TranscriptEntry) -> Option<()> {
        use cna_protocol::TranscriptEntry;
        match entry {
            TranscriptEntry::AssistantText { text }
            | TranscriptEntry::Reasoning { text }
            | TranscriptEntry::System { text } => self.add(text.len())?,
            TranscriptEntry::ToolCall {
                call_id,
                tool,
                args,
            } => {
                self.add(call_id.len())?;
                self.add(tool.len())?;
                self.json(args)?;
            }
            TranscriptEntry::ToolResult {
                call_id,
                summary,
                detail,
                ..
            } => {
                self.add(call_id.len())?;
                self.add(summary.len())?;
                if let Some(detail) = detail {
                    self.json(detail)?;
                }
            }
            TranscriptEntry::DecisionSubmitted {
                decision_id,
                summary,
            } => {
                self.add(decision_id.len())?;
                self.add(summary.len())?;
            }
            TranscriptEntry::System1Query { question, options } => {
                self.add(question.len())?;
                self.elements::<String>(options.capacity())?;
                for option in options {
                    self.add(option.len())?;
                }
            }
            TranscriptEntry::System1Answer { choice, scores } => {
                self.add(choice.len())?;
                if let Some(scores) = scores {
                    // m7ea2c2e1ec6e: lock the ACTUAL container/key/value type
                    // beside its owner-local structural bound. HashMap or a
                    // changed key/value type must fail this binding, not inherit
                    // an unrelated private-layout assumption.
                    let scores: &std::collections::BTreeMap<String, f64> = scores;
                    const SCORE_MAP_ENTRY_BACKING_BYTES: usize = 16
                        * (std::mem::size_of::<String>()
                            + std::mem::size_of::<f64>()
                            + std::mem::size_of::<usize>());
                    // Keep m897's already-approved, larger JSON charge and all
                    // existing child/encoded admissions; no reduced reservation.
                    const _: () =
                        assert!(JSON_MAP_ENTRY_BACKING_BYTES >= SCORE_MAP_ENTRY_BACKING_BYTES);
                    for key in scores.keys() {
                        self.add(JSON_MAP_ENTRY_BACKING_BYTES)?;
                        self.add(key.len())?;
                    }
                }
            }
            TranscriptEntry::UsageSnapshot(snapshot) => {
                self.elements::<cna_protocol::UsageSnapshot>(1)?;
                self.add(snapshot.provider.as_ref().map_or(0, String::len))?;
                self.add(snapshot.model.as_ref().map_or(0, String::len))?;
            }
        }
        Some(())
    }
    fn row(&mut self, row: &UnconfirmedEntry) -> Option<()> {
        self.add(row.at.len())?;
        if let Some(origin) = &row.origin {
            self.add(origin.operation_id.len())?;
        }
        self.transcript(&row.entry)
    }
}

fn admit_copy(operation: &Operation, storage: Option<CopyStorage>) -> Result<(), HostRefusal> {
    operation.check().map_err(operation_refusal)?;
    let storage = storage.ok_or_else(|| operation_refusal(operation.overflow()))?;
    operation.charge(storage.0).map_err(operation_refusal)
}

pub(crate) fn copy_pending(
    operation: &Operation,
    pending: &[cna_core::decision::DecisionRequest],
) -> Result<Vec<cna_core::decision::DecisionRequest>, HostRefusal> {
    let mut storage = CopyStorage::default();
    let measured = storage.pending(pending).map(|()| storage);
    admit_copy(operation, measured)?;
    let mut copy = Vec::with_capacity(pending.len());
    copy.extend(pending.iter().cloned());
    Ok(copy)
}

pub(crate) fn copy_delivery_row(
    operation: &Operation,
    row: &UnconfirmedEntry,
) -> Result<UnconfirmedEntry, HostRefusal> {
    let mut storage = CopyStorage::default();
    let measured = storage.row(row).map(|()| storage);
    admit_copy(operation, measured)?;
    Ok(row.clone())
}

pub(crate) fn copy_transcript_entry(
    operation: &Operation,
    entry: &cna_protocol::TranscriptEntry,
) -> Result<cna_protocol::TranscriptEntry, HostRefusal> {
    let mut storage = CopyStorage::default();
    let measured = storage.transcript(entry).map(|()| storage);
    admit_copy(operation, measured)?;
    Ok(entry.clone())
}

pub(crate) fn admit_receipt_children(
    operation: &Operation,
    receipt: &SubmitReceipt,
) -> Result<(), HostRefusal> {
    let mut storage = CopyStorage::default();
    let measured = (|| {
        storage.add(receipt.decision_id.len())?;
        storage.add(receipt.summary.len())?;
        storage.json(&receipt.result)?;
        Some(storage)
    })();
    admit_copy(operation, measured)
}

/// Canonical accepted receipt is constructed after commit; prepay its known
/// children BEFORE submitting so refusal cannot discard an already accepted R.
pub(crate) fn admit_accepted_receipt(
    operation: &Operation,
    decision_id: &str,
) -> Result<(), HostRefusal> {
    let mut storage = CopyStorage::default();
    let measured = (|| {
        storage.add(decision_id.len())?;
        storage.add("accepted decision ".len())?;
        storage.add(decision_id.len())?;
        storage.add(JSON_MAP_ENTRY_BACKING_BYTES)?;
        storage.add("accepted".len())?;
        Some(storage)
    })();
    admit_copy(operation, measured)
}

/// Private resident identity of the canonical Reserved/Active entry. The
/// original Operation UUID is fixed; no cloned operation-id String survives
/// this handoff. The exact UTF-8 decision key has the existing PREPARE bound.
/// Its full boxed child must be included in the original owner layout.
pub(crate) type RetainedDecisionStorage = [u8; cna_seats::operation::PREPARE_BYTES];
pub(crate) struct StoredHostIdentity {
    seat: cna_core::ids::SeatId,
    epoch: u64,
    revision: u32,
    original: Uuid,
    decision_len: usize,
    decision: Box<RetainedDecisionStorage>,
}
impl StoredHostIdentity {
    pub(crate) fn capture(
        identity: &HostIdentity,
        operation: &Operation,
    ) -> Result<Self, HostRefusal> {
        if identity.operation_id != operation.id() {
            return Err(HostRefusal::IdentityChanged);
        }
        let original = Uuid::parse_str(operation.id()).map_err(|_| HostRefusal::IdentityChanged)?;
        if identity.decision_id.len() > cna_seats::operation::PREPARE_BYTES {
            return Err(HostRefusal::CaptureOverflow);
        }
        let mut decision = Box::new([0; cna_seats::operation::PREPARE_BYTES]);
        decision[..identity.decision_id.len()].copy_from_slice(identity.decision_id.as_bytes());
        Ok(Self {
            seat: identity.seat,
            epoch: identity.epoch,
            revision: identity.revision,
            original,
            decision_len: identity.decision_id.len(),
            decision,
        })
    }
    pub(crate) fn seat(&self) -> cna_core::ids::SeatId {
        self.seat
    }
    pub(crate) fn revision(&self) -> u32 {
        self.revision
    }
    pub(crate) fn reply_owner(&self, generation: u64) -> crate::host_reply::ReplyOwner {
        crate::host_reply::ReplyOwner {
            operation: self.original,
            seat: self.seat,
            epoch: self.epoch,
            revision: self.revision,
            host_generation: generation,
        }
    }
    pub(crate) fn decision_id(&self) -> &str {
        // Copied only from a valid String; this private storage is never edited.
        std::str::from_utf8(&self.decision[..self.decision_len]).expect("stored UTF-8 identity")
    }
    pub(crate) fn matches(&self, identity: &HostIdentity, operation: &Operation) -> bool {
        identity.seat == self.seat
            && identity.epoch == self.epoch
            && identity.revision == self.revision
            && identity.decision_id == self.decision_id()
            && identity.operation_id == operation.id()
            && Uuid::parse_str(operation.id()).ok() == Some(self.original)
    }
    pub(crate) fn body<'a>(&'a self, operation: &'a Operation) -> HostIdentityBody<'a> {
        // Borrow the SAME original Operation's key, preserving the old encoded
        // envelope text without synthesizing a UUID String or allocating a copy.
        HostIdentityBody {
            seat: self.seat,
            epoch: self.epoch,
            revision: self.revision,
            decision_id: self.decision_id(),
            operation_id: operation.id(),
        }
    }
}

/// Conservatively meter concrete retained representations without allocating a
/// padding buffer. Each emitted zero costs at least one encoded byte. The count
/// comes from size_of of the actual owner types, never a caller's size assertion.
/// This does not count allocator slack or claim an RSS bound.
pub(crate) struct FixedStorage(pub usize);
impl Serialize for FixedStorage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut sequence = serializer.serialize_seq(Some(self.0))?;
        for _ in 0..self.0 {
            sequence.serialize_element(&0u8)?;
        }
        sequence.end()
    }
}

/// Metadata still retained beside canonical R must participate in the SAME
/// live COMMAND limit alongside all earlier reserve and BEGIN admissions.
/// All strings are borrowed from their actual owner; this creates no hidden copy.
#[derive(Serialize)]
pub(crate) struct CommandMetadata<'a> {
    pub identity: HostIdentityBody<'a>,
    pub stream: &'a str,
    pub token: &'a str,
    pub receipt_identity: &'a str,
    pub caller_receipt_identity: &'a str,
    pub fixed_storage: FixedStorage,
}

#[derive(Serialize)]
pub(crate) struct ReservationBody<'a> {
    pub identity: HostIdentityBody<'a>,
    pub request: RequestBody<'a>,
    pub caller_receipt_identity: &'a str,
    pub fixed_storage: FixedStorage,
}

#[derive(Serialize)]
pub(crate) struct BeginBody<'a> {
    pub identity: &'a HostIdentityBody<'a>,
    pub stream: &'a str,
    pub boundary: u64,
    pub token: &'a str,
}

#[derive(Serialize)]
pub(crate) struct HostIdentityBody<'a> {
    seat: cna_core::ids::SeatId,
    epoch: u64,
    decision_id: &'a str,
    revision: u32,
    operation_id: &'a str,
}
impl<'a> From<&'a HostIdentity> for HostIdentityBody<'a> {
    fn from(identity: &'a HostIdentity) -> Self {
        Self {
            seat: identity.seat,
            epoch: identity.epoch,
            decision_id: &identity.decision_id,
            revision: identity.revision,
            operation_id: &identity.operation_id,
        }
    }
}

// SubmitRequest deliberately has no new Serde/wire implementation. This is a
// private borrowed accounting envelope over its existing fields.
#[derive(Serialize)]
pub(crate) struct RequestBody<'a> {
    decision_id: &'a str,
    epoch: u64,
    revision: Option<u32>,
    idempotency_key: &'a str,
    action: &'a serde_json::Value,
    public_explanation: &'a Option<String>,
}
impl<'a> From<&'a SubmitRequest> for RequestBody<'a> {
    fn from(request: &'a SubmitRequest) -> Self {
        Self {
            decision_id: &request.decision_id,
            epoch: request.epoch,
            revision: request.revision,
            idempotency_key: &request.idempotency_key,
            action: &request.action,
            public_explanation: &request.public_explanation,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct MarkerBody<'a> {
    pub stream: &'a str,
    pub token: &'a str,
    pub owner: HostIdentityBody<'a>,
    pub ordinal: u64,
    pub receipt_identity: Option<&'a str>,
}

#[derive(Serialize)]
pub(crate) struct DeliveryBody<'a> {
    pub stream: &'a str,
    pub token: &'a str,
    pub ordinal: u64,
    pub row: &'a UnconfirmedEntry,
}

/// Complete body size is checked before the caller makes a copy. A new reference
/// conservatively charges the complete logical body; existing owned moves do not.
pub(crate) fn charge_body(
    operation: &Operation,
    value: &impl Serialize,
    cap: usize,
) -> Result<usize, HostRefusal> {
    operation.check().map_err(operation_refusal)?;
    let bytes = encoded_len(value, cap).map_err(|_| operation_refusal(operation.overflow()))?;
    operation.charge(bytes).map_err(operation_refusal)?;
    Ok(bytes)
}

pub(crate) fn charge_command(
    operation: &Operation,
    value: &impl Serialize,
) -> Result<usize, HostRefusal> {
    charge_body(operation, value, COMMAND_BYTES)
}
pub(crate) fn charge_delivery(
    operation: &Operation,
    value: &impl Serialize,
) -> Result<usize, HostRefusal> {
    charge_body(operation, value, DELIVERY_BYTES)
}

/// Actual destination capacity is fixed by Box<str> before the String handoff.
/// The old encoded local guard stays additional, not a resident size estimate.
pub(crate) fn copy_command_text(operation: &Operation, value: &str) -> Result<String, HostRefusal> {
    charge_command(operation, &value)?;
    operation.charge(value.len()).map_err(operation_refusal)?;
    let text: Box<str> = value.into();
    Ok(text.into_string())
}
pub(crate) fn copy_delivery_text(
    operation: &Operation,
    value: &str,
) -> Result<String, HostRefusal> {
    charge_delivery(operation, &value)?;
    operation.charge(value.len()).map_err(operation_refusal)?;
    let text: Box<str> = value.into();
    Ok(text.into_string())
}

/// Called before a returned receipt is cloned. The canonical accepted R remains
/// in CommandCredit on either subcap or cumulative refusal.
#[cfg(test)]
pub(crate) fn charge_receipt_copy(
    operation: &Operation,
    receipt: &SubmitReceipt,
) -> Result<usize, HostRefusal> {
    charge_command(operation, receipt)
}

/// Reserve a retained reply body in the SAME live COMMAND envelope as canonical
/// R. This conservative footprint is computed from the actual complete encoded
/// receipt, not a declared caller size. It is encoded without allocating or
/// cloning R; its charge is at least that full body's bytes. It stays reserved
/// until final settlement, even after a request ACK or dequeue.
pub(crate) fn reserve_receipt_reply(
    credit: &mut cna_seats::operation::CommandCredit<SubmitReceipt>,
) -> Result<(), HostRefusal> {
    let bytes = encoded_len(credit.receipt().ok_or(HostRefusal::Unknown)?, COMMAND_BYTES)
        .map_err(|_| operation_refusal(credit.operation().overflow()))?;
    credit
        .admit_settlement(&FixedStorage(bytes))
        .map_err(operation_refusal)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cna_seats::operation::{Allowance, CommandCredit, OPERATION_BYTES};
    use serde_json::json;

    fn operation() -> Operation {
        Operation::new(
            "synthetic-host-operation".into(),
            1,
            Allowance::required(Some(
                tokio::time::Instant::now() + std::time::Duration::from_secs(5),
            ))
            .unwrap(),
        )
        .unwrap()
    }
    fn receipt(summary: String) -> SubmitReceipt {
        SubmitReceipt {
            decision_id: "synthetic-decision".into(),
            duplicate: false,
            summary,
            result: json!({"accepted":true}),
        }
    }

    fn production_identity(length: usize) -> HostIdentity {
        HostIdentity {
            seat: cna_core::ids::SeatId::all().next().unwrap(),
            epoch: 3,
            decision_id: "d".repeat(length),
            revision: 4,
            operation_id: "synthetic-host-operation".into(),
        }
    }

    #[test]
    fn reserve_then_receipt_reply_and_finish_keep_all_prior_command_charges() {
        // Exercise the actual envelope order missing from receipt-first controls.
        // Search only a synthetic ID length; this is not a whole-game/RSS bound.
        let measure = |length: usize| {
            let identity = production_identity(length);
            let request = SubmitRequest {
                decision_id: identity.decision_id.clone(),
                epoch: identity.epoch,
                revision: Some(identity.revision),
                idempotency_key: "original-key".into(),
                action: json!(null),
                public_explanation: None,
            };
            let reservation = ReservationBody {
                identity: (&identity).into(),
                request: (&request).into(),
                caller_receipt_identity: &request.idempotency_key,
                fixed_storage: FixedStorage(
                    crate::host_forced::retained_reservation_storage_bytes(),
                ),
            };
            let owner = HostIdentityBody::from(&identity);
            let begin = BeginBody {
                identity: &owner,
                stream: "stream",
                boundary: 0,
                token: "token",
            };
            let receipt = crate::host_forced::accepted_receipt(&identity.decision_id, false);
            let metadata = CommandMetadata {
                identity: (&identity).into(),
                stream: "stream",
                token: "token",
                receipt_identity: &request.idempotency_key,
                caller_receipt_identity: &request.idempotency_key,
                fixed_storage: FixedStorage(crate::host_forced::retained_command_storage_bytes()),
            };
            let marker = MarkerBody {
                stream: "stream",
                token: "token",
                owner: (&identity).into(),
                ordinal: 2,
                receipt_identity: Some(&request.idempotency_key),
            };
            let receipt_bytes = encoded_len(&receipt, usize::MAX).unwrap();
            let sizes = [
                encoded_len(&reservation, usize::MAX).unwrap(),
                encoded_len(&begin, usize::MAX).unwrap(),
                receipt_bytes,
                encoded_len(&metadata, usize::MAX).unwrap(),
                encoded_len(&FixedStorage(receipt_bytes), usize::MAX).unwrap(),
                encoded_len(&marker, usize::MAX).unwrap(),
                2,
            ];
            (identity, request, receipt, sizes)
        };
        let fits = |length| measure(length).3.iter().sum::<usize>() <= COMMAND_BYTES;
        assert!(fits(0));
        let (mut low, mut high) = (0, COMMAND_BYTES);
        while low < high {
            let middle = low + (high - low + 1) / 2;
            if fits(middle) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        assert!(fits(low) && !fits(low + 1));
        for (length, expected_fit) in [(low, true), (low + 1, false)] {
            let (identity, request, receipt, sizes) = measure(length);
            let reservation = ReservationBody {
                identity: (&identity).into(),
                request: (&request).into(),
                caller_receipt_identity: &request.idempotency_key,
                fixed_storage: FixedStorage(
                    crate::host_forced::retained_reservation_storage_bytes(),
                ),
            };
            let owner = HostIdentityBody::from(&identity);
            let begin = BeginBody {
                identity: &owner,
                stream: "stream",
                boundary: 0,
                token: "token",
            };
            let metadata = CommandMetadata {
                identity: (&identity).into(),
                stream: "stream",
                token: "token",
                receipt_identity: &request.idempotency_key,
                caller_receipt_identity: &request.idempotency_key,
                fixed_storage: FixedStorage(crate::host_forced::retained_command_storage_bytes()),
            };
            let marker = MarkerBody {
                stream: "stream",
                token: "token",
                owner: (&identity).into(),
                ordinal: 2,
                receipt_identity: Some(&request.idempotency_key),
            };
            let op = operation();
            let mut credit = CommandCredit::new(op.clone()).unwrap();
            assert_eq!(credit.admit_settlement(&reservation).unwrap(), sizes[0]);
            assert_eq!(credit.admit_settlement(&begin).unwrap(), sizes[1]);
            credit.retain_receipt(receipt.clone()).unwrap();
            let result = credit
                .admit_settlement(&metadata)
                .and_then(|_| {
                    reserve_receipt_reply(&mut credit)
                        .map_err(|_| cna_seats::operation::OperationError::CaptureOverflow)
                })
                .and_then(|_| credit.admit_settlement(&marker).map(|_| ()))
                .and_then(|_| {
                    credit
                        .admit_settlement(&Vec::<serde_json::Value>::new())
                        .map(|_| ())
                });
            assert_eq!(
                result.is_ok(),
                expected_fit,
                "reserve/BEGIN must survive receipt retention"
            );
            assert_eq!(credit.receipt(), Some(&receipt));
            if expected_fit {
                assert_eq!(op.captured_bytes(), sizes.iter().sum::<usize>());
            } else {
                assert!(matches!(
                    op.check(),
                    Err(cna_seats::operation::OperationError::CaptureOverflow)
                ));
            }
            assert!(op.captured_bytes() <= OPERATION_BYTES);
        }
    }

    fn production_settlement_sizes(length: usize) -> (usize, usize, usize, usize) {
        let identity = production_identity(length);
        let receipt = crate::host_forced::accepted_receipt(&identity.decision_id, false);
        let metadata = CommandMetadata {
            identity: (&identity).into(),
            stream: "00000000-0000-0000-0000-000000000000",
            token: "11111111-1111-1111-1111-111111111111",
            receipt_identity: "original-key",
            caller_receipt_identity: "original-key",
            fixed_storage: FixedStorage(crate::host_forced::retained_command_storage_bytes()),
        };
        let marker = MarkerBody {
            stream: metadata.stream,
            token: metadata.token,
            owner: (&identity).into(),
            ordinal: 2,
            receipt_identity: Some(metadata.receipt_identity),
        };
        let r = encoded_len(&receipt, usize::MAX).unwrap();
        (
            r,
            encoded_len(&FixedStorage(r), usize::MAX).unwrap(),
            encoded_len(&metadata, usize::MAX).unwrap(),
            encoded_len(&marker, usize::MAX).unwrap(),
        )
    }

    #[test]
    fn production_receipt_copy_and_settlement_largest_fit_or_keep_r_fail_closed() {
        // Search only the synthetic ID length for the largest fit of the actual
        // production R, reply footprint, owner metadata, second marker and []
        // final pending result. This does not claim maximum game fixture size.
        let fits = |length| {
            let (r, copy, metadata, marker) = production_settlement_sizes(length);
            r + copy + metadata + marker + 2 <= COMMAND_BYTES
        };
        let (mut low, mut high) = (0, COMMAND_BYTES);
        while low < high {
            let middle = low + (high - low + 1) / 2;
            if fits(middle) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        assert!(fits(low) && !fits(low + 1));
        for (length, expected_fit) in [(low, true), (low + 1, false)] {
            let identity = production_identity(length);
            let accepted = crate::host_forced::accepted_receipt(&identity.decision_id, false);
            let metadata = CommandMetadata {
                identity: (&identity).into(),
                stream: "00000000-0000-0000-0000-000000000000",
                token: "11111111-1111-1111-1111-111111111111",
                receipt_identity: "original-key",
                caller_receipt_identity: "original-key",
                fixed_storage: FixedStorage(crate::host_forced::retained_command_storage_bytes()),
            };
            let marker = MarkerBody {
                stream: metadata.stream,
                token: metadata.token,
                owner: (&identity).into(),
                ordinal: 2,
                receipt_identity: Some(metadata.receipt_identity),
            };
            let op = operation();
            let mut credit = CommandCredit::new(op.clone()).unwrap();
            credit.retain_receipt(accepted.clone()).unwrap();
            credit.admit_settlement(&metadata).unwrap();
            reserve_receipt_reply(&mut credit).unwrap();
            let settlement = credit
                .admit_settlement(&marker)
                .and_then(|_| credit.admit_settlement(&Vec::<serde_json::Value>::new()));
            assert_eq!(settlement.is_ok(), expected_fit);
            assert_eq!(credit.receipt(), Some(&accepted));
            assert!(op.captured_bytes() <= OPERATION_BYTES);
            if !expected_fit {
                assert!(matches!(
                    op.check(),
                    Err(cna_seats::operation::OperationError::CaptureOverflow)
                ));
            }
        }
    }

    #[test]
    fn canonical_r_alone_fits_but_reply_copy_refuses_same_live_envelope() {
        let op = operation();
        let mut credit = CommandCredit::new(op.clone()).unwrap();
        let accepted = receipt("r".repeat(COMMAND_BYTES / 2));
        credit.retain_receipt(accepted.clone()).unwrap();
        assert!(matches!(
            reserve_receipt_reply(&mut credit),
            Err(HostRefusal::CaptureOverflow)
        ));
        assert_eq!(credit.receipt(), Some(&accepted));
    }

    #[test]
    fn production_reply_hits_same_cumulative_boundary_without_refund_or_copy() {
        let op = operation();
        let mut credit = CommandCredit::new(op.clone()).unwrap();
        let accepted = crate::host_forced::accepted_receipt("synthetic-decision", false);
        credit.retain_receipt(accepted.clone()).unwrap();
        let reply_bytes = encoded_len(
            &FixedStorage(encoded_len(&accepted, COMMAND_BYTES).unwrap()),
            COMMAND_BYTES,
        )
        .unwrap();
        op.charge(OPERATION_BYTES - op.captured_bytes() - reply_bytes)
            .unwrap();
        reserve_receipt_reply(&mut credit).unwrap();
        assert_eq!(op.captured_bytes(), OPERATION_BYTES);
        assert!(matches!(
            reserve_receipt_reply(&mut credit),
            Err(HostRefusal::CaptureOverflow)
        ));
        assert_eq!(credit.receipt(), Some(&accepted));
        assert_eq!(op.captured_bytes(), OPERATION_BYTES);
    }

    #[test]
    fn actual_receipt_and_second_marker_largest_combined_fit_or_fail_closed_keep_receipt() {
        let identity = HostIdentity {
            seat: cna_core::ids::SeatId::all().next().unwrap(),
            epoch: 3,
            decision_id: "synthetic-decision".into(),
            revision: 4,
            operation_id: "synthetic-host-operation".into(),
        };
        let marker = MarkerBody {
            stream: "synthetic-stream",
            token: "synthetic-token",
            owner: (&identity).into(),
            ordinal: 17,
            receipt_identity: Some("original-idempotency-key"),
        };
        let metadata = CommandMetadata {
            identity: (&identity).into(),
            stream: marker.stream,
            token: marker.token,
            receipt_identity: marker.receipt_identity.unwrap(),
            caller_receipt_identity: marker.receipt_identity.unwrap(),
            fixed_storage: FixedStorage(
                std::mem::size_of::<SubmitReceipt>() + std::mem::size_of::<HostIdentity>(),
            ),
        };
        let metadata_bytes = encoded_len(&metadata, COMMAND_BYTES).unwrap();
        let marker_bytes = encoded_len(&marker, COMMAND_BYTES).unwrap();
        let empty = receipt(String::new());
        let fixed_receipt_bytes = encoded_len(&empty, COMMAND_BYTES).unwrap();
        let exact = receipt(
            "r".repeat(COMMAND_BYTES - marker_bytes - metadata_bytes - fixed_receipt_bytes),
        );
        let op = operation();
        let mut credit = CommandCredit::new(op.clone()).unwrap();
        credit.retain_receipt(exact.clone()).unwrap();
        credit.admit_settlement(&metadata).unwrap();
        assert_eq!(
            encoded_len(&exact, COMMAND_BYTES).unwrap() + marker_bytes + metadata_bytes,
            COMMAND_BYTES
        );
        credit.admit_settlement(&marker).unwrap();
        assert_eq!(credit.receipt(), Some(&exact));
        assert!(
            credit
                .admit_settlement(&json!({"additional":true}))
                .is_err()
        );
        assert_eq!(credit.receipt(), Some(&exact));
        assert!(matches!(
            op.check(),
            Err(cna_seats::operation::OperationError::CaptureOverflow)
        ));
    }

    #[test]
    fn retained_metadata_is_not_hidden_by_receipt_replacement_or_new_reference() {
        let identity = HostIdentity {
            seat: cna_core::ids::SeatId::all().next().unwrap(),
            epoch: 3,
            decision_id: "synthetic-decision".into(),
            revision: 4,
            operation_id: "synthetic-host-operation".into(),
        };
        let metadata = CommandMetadata {
            identity: (&identity).into(),
            stream: "stream",
            token: "token",
            receipt_identity: "original-key",
            caller_receipt_identity: "original-key",
            fixed_storage: FixedStorage(std::mem::size_of::<SubmitReceipt>()),
        };
        let bytes = encoded_len(&metadata, COMMAND_BYTES).unwrap();
        assert!(bytes >= std::mem::size_of::<SubmitReceipt>());
        let fixed = encoded_len(&receipt(String::new()), COMMAND_BYTES).unwrap();
        let accepted = receipt("r".repeat(COMMAND_BYTES - fixed - bytes));
        let operation = operation();
        let mut credit = CommandCredit::new(operation.clone()).unwrap();
        credit.retain_receipt(accepted.clone()).unwrap();
        credit.admit_settlement(&metadata).unwrap();
        assert!(
            credit.admit_settlement(&metadata).is_err(),
            "another retained reference cannot hide its body"
        );
        assert_eq!(credit.receipt(), Some(&accepted));
    }

    #[test]
    fn raw_frame_fit_does_not_imply_delivery_fit_and_escaped_body_is_not_split() {
        let op = operation();
        let row = UnconfirmedEntry {
            seat: cna_core::ids::SeatId::all().next().unwrap(),
            origin: None,
            at: "synthetic".into(),
            entry: cna_protocol::TranscriptEntry::System {
                // Escaped JSON exceeds 2 MiB despite fewer than 4 MiB raw bytes.
                text: "\n".repeat(DELIVERY_BYTES / 2),
            },
        };
        let body = DeliveryBody {
            stream: "stream",
            token: "token",
            ordinal: 1,
            row: &row,
        };
        assert!(matches!(
            charge_delivery(&op, &body),
            Err(HostRefusal::CaptureOverflow)
        ));
        assert_eq!(op.captured_bytes(), 0, "no partial envelope accepted");
    }

    #[test]
    fn a_new_full_receipt_reference_uses_same_cumulative_meter_without_refund() {
        let op = operation();
        let accepted = receipt("accepted".into());
        let mut credit = CommandCredit::new(op.clone()).unwrap();
        credit.retain_receipt(accepted.clone()).unwrap();
        op.charge(OPERATION_BYTES - op.captured_bytes()).unwrap();
        assert!(matches!(
            charge_receipt_copy(&op, credit.receipt().unwrap()),
            Err(HostRefusal::CaptureOverflow)
        ));
        assert_eq!(credit.receipt(), Some(&accepted));
        assert_eq!(op.captured_bytes(), OPERATION_BYTES);
    }

    #[test]
    fn borrowed_reservation_counts_all_original_fields_and_utf8_escaping() {
        let identity = HostIdentity {
            seat: cna_core::ids::SeatId::all().next().unwrap(),
            epoch: 3,
            decision_id: "decision".into(),
            revision: 4,
            operation_id: "operation".into(),
        };
        let request = SubmitRequest {
            decision_id: identity.decision_id.clone(),
            epoch: 3,
            revision: Some(4),
            idempotency_key: "original-key".into(),
            action: json!({"value":"é\n\""}),
            public_explanation: Some("bounded".into()),
        };
        let body = ReservationBody {
            identity: (&identity).into(),
            request: (&request).into(),
            caller_receipt_identity: &request.idempotency_key,
            fixed_storage: FixedStorage(64),
        };
        let encoded = serde_json::to_vec(&body).unwrap();
        assert_eq!(encoded_len(&body, COMMAND_BYTES).unwrap(), encoded.len());
        let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(value["request"]["idempotency_key"], "original-key");
        assert_eq!(value["caller_receipt_identity"], "original-key");
        assert_eq!(value["request"]["public_explanation"], "bounded");
        assert_eq!(value["identity"]["revision"], 4);
    }

    #[test]
    fn stored_identity_moves_original_uuid_and_preserves_borrowed_envelope_bytes() {
        let key = Uuid::from_u128(6).to_string();
        let operation = Operation::new(
            key.clone(),
            1,
            Allowance::required(Some(
                tokio::time::Instant::now() + std::time::Duration::from_secs(5),
            ))
            .unwrap(),
        )
        .unwrap();
        let identity = HostIdentity {
            seat: cna_core::ids::SeatId::all().next().unwrap(),
            epoch: 3,
            decision_id: "decision-\u{00e9}\n\"".into(),
            revision: 4,
            operation_id: key,
        };
        let stored = StoredHostIdentity::capture(&identity, &operation).unwrap();
        assert_eq!(stored.original, Uuid::from_u128(6));
        assert!(stored.matches(&identity, &operation));
        let child = stored.decision.as_ptr();
        // This is the same owned field moved from Reserved into ForcedState;
        // movement cannot clone the original UUID/key or allocate another child.
        let active_identity = stored;
        assert_eq!(active_identity.decision.as_ptr(), child);
        assert_eq!(
            serde_json::to_vec(&active_identity.body(&operation)).unwrap(),
            serde_json::to_vec(&HostIdentityBody::from(&identity)).unwrap()
        );
        assert_eq!(active_identity.decision_id(), identity.decision_id);
        assert_eq!(
            std::mem::size_of::<RetainedDecisionStorage>(),
            cna_seats::operation::PREPARE_BYTES
        );
        let complete = std::alloc::Layout::new::<(StoredHostIdentity, RetainedDecisionStorage)>();
        assert!(
            complete.size()
                >= std::mem::size_of::<StoredHostIdentity>()
                    + std::mem::size_of::<RetainedDecisionStorage>()
        );
        let mut foreign = identity.clone();
        foreign.operation_id = Uuid::from_u128(7).to_string();
        assert!(!active_identity.matches(&foreign, &operation));
        foreign = identity.clone();
        foreign.epoch += 1;
        assert!(!active_identity.matches(&foreign, &operation));
        foreign = identity.clone();
        foreign.revision += 1;
        assert!(!active_identity.matches(&foreign, &operation));
        foreign = identity.clone();
        foreign.decision_id.push('x');
        assert!(!active_identity.matches(&foreign, &operation));
    }

    #[test]
    fn stored_identity_refuses_unknown_uuid_and_oversize_before_fixed_child_copy() {
        let identity = HostIdentity {
            seat: cna_core::ids::SeatId::all().next().unwrap(),
            epoch: 3,
            decision_id: "decision".into(),
            revision: 4,
            operation_id: "not-a-uuid".into(),
        };
        let unknown = Operation::new(
            identity.operation_id.clone(),
            1,
            Allowance::required(Some(
                tokio::time::Instant::now() + std::time::Duration::from_secs(5),
            ))
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            StoredHostIdentity::capture(&identity, &unknown),
            Err(HostRefusal::IdentityChanged)
        ));
        let key = Uuid::from_u128(8).to_string();
        let original = Operation::new(
            key.clone(),
            1,
            Allowance::required(Some(
                tokio::time::Instant::now() + std::time::Duration::from_secs(5),
            ))
            .unwrap(),
        )
        .unwrap();
        let mut oversize = identity;
        oversize.operation_id = key;
        oversize.decision_id = "x".repeat(cna_seats::operation::PREPARE_BYTES + 1);
        assert!(matches!(
            StoredHostIdentity::capture(&oversize, &original),
            Err(HostRefusal::CaptureOverflow)
        ));
        assert_eq!(
            original.captured_bytes(),
            0,
            "component control does not establish a complete original-creation charge"
        );
    }
}
