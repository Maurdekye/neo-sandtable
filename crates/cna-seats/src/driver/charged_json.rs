//! Original-operation admission for JSON tree payloads, before derived allocation.
//!
//! This meters requested vector storage, string payloads, and the lead-set JSON
//! map entry backing bound. It does not establish allocator RSS or later copy fit.

use std::cell::Cell;
use std::fmt;

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::operation::{Operation, OperationError};

struct Budget<'a> {
    operation: &'a Operation,
    failure: Cell<Option<OperationError>>,
}

impl Budget<'_> {
    fn refused<E: serde::de::Error>(&self, failure: OperationError) -> E {
        if self.failure.get().is_none() {
            self.failure.set(Some(failure));
        }
        E::custom("owned JSON admission refused")
    }

    fn check<E: serde::de::Error>(&self) -> Result<(), E> {
        self.operation
            .check()
            .map_err(|failure| self.refused(failure))
    }

    fn charge<E: serde::de::Error>(&self, bytes: Option<usize>) -> Result<(), E> {
        // Byte admission alone does not enforce the original absolute deadline.
        // Existing closed causes precede arithmetic overflow and derived allocation.
        self.check::<E>()?;
        let result = bytes
            .ok_or_else(|| self.operation.overflow())
            .and_then(|n| self.operation.charge(n));
        result.map_err(|failure| self.refused(failure))
    }

    fn string<E: serde::de::Error>(&self, value: &str) -> Result<String, E> {
        // One payload/header charge and fallible copy on the SAME original operation.
        // The Tree's Value header remains a separate existing charge.
        super::event_storage::EventStorage(Some(self.operation))
            .copy(value)
            .map_err(|failure| self.refused(failure))
    }
}

#[derive(Clone, Copy)]
struct Tree<'a, 'b>(&'a Budget<'b>);

impl<'de> DeserializeSeed<'de> for Tree<'_, '_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        self.0
            .charge::<D::Error>(Some(std::mem::size_of::<Value>()))?;
        d.deserialize_any(self)
    }
}

struct Key<'a, 'b>(&'a Budget<'b>);

impl<'de> DeserializeSeed<'de> for Key<'_, '_> {
    type Value = String;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<String, D::Error> {
        self.0.check::<D::Error>()?;
        d.deserialize_str(self)
    }
}

impl<'de> Visitor<'de> for Key<'_, '_> {
    type Value = String;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object key")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<String, E> {
        const {
            assert!(
                std::mem::size_of::<(String, Value)>()
                    <= std::mem::size_of::<String>() + std::mem::size_of::<Value>()
            );
        }
        // Each map-key visitor owns exactly one decoded entry attempt. Charge
        // structural/spare backing BEFORE allocating either key or value.
        self.0
            .charge::<E>(Some(super::event_storage::JSON_MAP_ENTRY_BACKING_BYTES))?;
        self.0.string(value)
    }
}

impl<'de> Visitor<'de> for Tree<'_, '_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Value, E> {
        Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("nonfinite JSON number"))
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Value, E> {
        self.0.string(v).map(Value::String)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        self.0
            .charge::<A::Error>(Some(std::mem::size_of::<Vec<Value>>()))?;
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(self)? {
            self.0.check::<A::Error>()?; // Spare capacity is not renewed admission.
            // Same cumulative growth admission as event storage, with no duplicate
            // backing charge, allowance renewal or refund of the in-service element.
            super::event_storage::EventStorage(Some(self.0.operation))
                .reserve_next(&mut values)
                .map_err(|failure| self.0.refused::<A::Error>(failure))?;
            self.0.check::<A::Error>()?;
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        self.0
            .charge::<A::Error>(Some(std::mem::size_of::<Map<String, Value>>()))?;
        let mut values = Map::new();
        while let Some(key) = map.next_key_seed(Key(self.0))? {
            let value = map.next_value_seed(self)?;
            // Duplicate keys keep serde_json's last-value semantics. Both decoded copies
            // remain charged, including replacements that are no longer live.
            // Key's visitor already prepaid this attempt's backing before its
            // first child allocation. Never refund malformed/duplicate entries.
            self.0.check::<A::Error>()?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

/// Longest encoded string requiring serde_json's reusable escape scratch. No string,
/// token list or tree is allocated by this scan, including on malformed input.
fn escaped_string_span(text: &str) -> usize {
    let mut start = None;
    let mut escaped = false;
    let mut slash = false;
    let mut largest = 0;
    for (offset, byte) in text.bytes().enumerate() {
        if let Some(begin) = start {
            if slash {
                slash = false;
            } else if byte == b'\\' {
                escaped = true;
                slash = true;
            } else if byte == b'"' {
                if escaped {
                    largest = largest.max(offset - begin);
                }
                start = None;
            }
        } else if byte == b'"' {
            start = Some(offset + 1);
            escaped = false;
        }
    }
    if let Some(begin) = start.filter(|_| escaped) {
        largest = largest.max(text.len() - begin);
    }
    largest
}

/// The same library scratch can hold long numeric significands when float_roundtrip
/// is enabled by feature unification. Include those spans without parsing or allocating.
fn number_span(text: &str) -> usize {
    let mut quoted = false;
    let mut slash = false;
    let mut number = 0usize;
    let mut largest = 0usize;
    for byte in text.bytes() {
        if quoted {
            if slash {
                slash = false;
            } else if byte == b'\\' {
                slash = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            largest = largest.max(number);
            number = 0;
            quoted = true;
        } else if matches!(byte, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
            // A numeric span cannot exceed text.len(), already representable as usize.
            number += 1;
        } else {
            largest = largest.max(number);
            number = 0;
        }
    }
    largest.max(number)
}

/// Malformed lines retain the historical empty-parser result. A budget refusal is
/// distinct and propagates so ChildProc keeps its original raw frame and association.
pub(super) fn decode(text: &str, operation: &Operation) -> Result<Option<Value>, OperationError> {
    decode_checked(text, operation).map(Result::ok)
}

/// Owned preflight validation preserves the actual syntax diagnostic while the helper
/// still owns its original raw output. Runtime stream parsing keeps its historical empty
/// result for malformed lines through `decode` above.
pub(super) fn decode_checked(
    text: &str,
    operation: &Operation,
) -> Result<Result<Value, serde_json::Error>, OperationError> {
    operation.check()?; // Do not even scan an already closed original input.
    let scratch = escaped_string_span(text).max(number_span(text));
    // SliceRead borrows unescaped strings. Escapes and long numeric significands
    // use the same reused Vec<u8>;
    // geometric requested capacities cumulatively fit 4*encoded-span+16, including
    // the minimum allocation. Allocator excess and library error storage are not RSS proof.
    let scratch = if scratch == 0 {
        Some(0)
    } else {
        scratch.checked_mul(4).and_then(|n| n.checked_add(16))
    };
    operation.check()?;
    operation.charge(
        scratch
            .and_then(|n| n.checked_add(std::mem::size_of::<Vec<u8>>()))
            .ok_or_else(|| operation.overflow())?,
    )?;
    let budget = Budget {
        operation,
        failure: Cell::new(None),
    };
    operation.check()?;
    let mut decoder = serde_json::Deserializer::from_str(text);
    let value = Tree(&budget)
        .deserialize(&mut decoder)
        .and_then(|v| decoder.end().map(|()| v));
    if let Some(failure) = budget.failure.get() {
        return Err(failure);
    }
    operation.check()?; // Syntax failure cannot hide closure during decoding.
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::{Allowance, OPERATION_BYTES, StartEvidence};
    use tokio::time::Instant;

    fn operation() -> Operation {
        Operation::new(
            "original-parser".into(),
            1,
            Allowance::required(Some(Instant::now() + std::time::Duration::from_secs(30))).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn charged_tree_preserves_json_semantics_and_duplicate_keys() {
        for text in [
            r#"{"x":[null,true,-9,18446744073709551615,1.25,"é\\n"],"x":"last","u":"\uD834\uDD1E"}"#,
            "[]",
            "{}",
            "null",
            "[1,2] trailing",
            "[",
            r#""\uD800""#,
        ] {
            let op = operation();
            let expected = serde_json::from_str::<Value>(text).ok();
            assert_eq!(decode(text, &op).unwrap(), expected);
            assert_eq!(op.start_evidence(), StartEvidence::NotStarted);
        }
    }

    #[test]
    fn checked_decode_keeps_syntax_error_and_stream_wrapper_keeps_empty_result() {
        for text in ["{broken", "{\"loggedIn\":true} tail", "[0,", "\"bad\\q\""] {
            let expected = serde_json::from_str::<Value>(text).unwrap_err().to_string();
            let original = operation();
            let error = decode_checked(text, &original).unwrap().unwrap_err();
            assert_eq!(error.to_string(), expected);
            assert!(decode(text, &operation()).unwrap().is_none());
            assert_eq!(original.start_evidence(), StartEvidence::NotStarted);
        }
    }

    #[test]
    fn escaped_scratch_is_admitted_before_decode_and_not_refunded() {
        let op = operation();
        op.possible_start();
        op.charge(OPERATION_BYTES - op.captured_bytes() - 1)
            .unwrap();
        assert_eq!(
            decode(r#""\u1234""#, &op),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(op.captured_bytes(), OPERATION_BYTES - 1);
        assert_eq!(op.start_evidence(), StartEvidence::Possible);
        assert_eq!(op.check(), Err(OperationError::CaptureOverflow));
    }

    // SOURCE ONLY/UNRUN: test the visitor's copy boundary, not serde scratch RSS.
    #[test]
    fn json_string_copy_uses_one_original_charge_and_retains_borrow_on_refusal() {
        let raw = String::from("borrowed \u{03b2}\n\u{1f600}");
        let pointer = raw.as_ptr();
        let capacity = raw.capacity();
        let original = operation();
        let before = original.captured_bytes();
        let budget = Budget {
            operation: &original,
            failure: Cell::new(None),
        };
        let copied = budget.string::<serde_json::Error>(&raw).unwrap();
        assert_eq!(copied, raw);
        assert_ne!(copied.as_ptr(), pointer);
        assert_eq!(
            original.captured_bytes(),
            before + raw.len() + std::mem::size_of::<String>()
        );
        assert_eq!(budget.failure.get(), None);
        let refused = operation();
        refused.possible_start();
        refused
            .charge(
                OPERATION_BYTES
                    - refused.captured_bytes()
                    - raw.len()
                    - std::mem::size_of::<String>()
                    + 1,
            )
            .unwrap();
        let before = refused.captured_bytes();
        let budget = Budget {
            operation: &refused,
            failure: Cell::new(None),
        };
        for _ in 0..2 {
            assert!(budget.string::<serde_json::Error>(&raw).is_err());
            assert_eq!(budget.failure.get(), Some(OperationError::CaptureOverflow));
            assert_eq!(raw.as_ptr(), pointer);
            assert_eq!(raw.capacity(), capacity);
            assert_eq!(raw, "borrowed \u{03b2}\n\u{1f600}");
            assert_eq!(refused.captured_bytes(), before);
            assert_eq!(refused.start_evidence(), StartEvidence::Possible);
        }
    }

    #[test]
    fn scalar_dense_array_cannot_evade_original_operation_budget() {
        let op = operation();
        let text = format!(
            "[{}]",
            std::iter::repeat_n("0", 16).collect::<Vec<_>>().join(",")
        );
        op.charge(OPERATION_BYTES - op.captured_bytes() - 4 * std::mem::size_of::<Value>())
            .unwrap();
        assert_eq!(decode(&text, &op), Err(OperationError::CaptureOverflow));
        assert_eq!(op.check(), Err(OperationError::CaptureOverflow));
        assert_eq!(op.start_evidence(), StartEvidence::NotStarted);
    }

    #[test]
    fn lexical_scratch_scan_covers_numbers_but_does_not_copy_quoted_text() {
        assert_eq!(
            number_span(
                r#"{"n":1234567890123456789012345,"text":"123456789012345678901234567890"}"#
            ),
            25
        );
        assert_eq!(number_span(r#"["quoted\"12345",-1.25e+123]"#), 10);
        assert_eq!(escaped_string_span(r#"["no scratch","a\u1234"]"#), 7);
        assert_eq!(escaped_string_span(r#""unterminated\u1234"#), 18);
    }

    // SOURCE ONLY/UNRUN: no child/reader, filesystem, provider or real settlement.
    #[test]
    fn closed_json_admission_keeps_raw_input_meter_and_original_cause() {
        for settled in [false, true] {
            let original = operation();
            original.possible_start();
            let text = String::from(r#"{"raw":"\u1234","values":[0,1,2,3]}"#);
            let pointer = text.as_ptr();
            let capacity = text.capacity();
            let charged = original.captured_bytes();
            let expected = if settled {
                original.settle();
                OperationError::Settled
            } else {
                original.overflow()
            };
            for _ in 0..2 {
                assert_eq!(decode(&text, &original), Err(expected));
                assert_eq!(decode_checked("{broken", &original).unwrap_err(), expected);
                let budget = Budget {
                    operation: &original,
                    failure: Cell::new(None),
                };
                assert!(budget.charge::<serde_json::Error>(None).is_err());
                assert_eq!(budget.failure.get(), Some(expected));
                assert!(budget.string::<serde_json::Error>("must not copy").is_err());
                assert_eq!(budget.failure.get(), Some(expected));
                assert_eq!(text.as_ptr(), pointer);
                assert_eq!(text.capacity(), capacity);
                assert_eq!(text, r#"{"raw":"\u1234","values":[0,1,2,3]}"#);
                assert_eq!(original.captured_bytes(), charged);
                assert_eq!(original.check(), Err(expected));
                assert_eq!(original.start_evidence(), StartEvidence::Possible);
            }
        }
    }

    struct ClosingSequence<'a> {
        original: &'a Operation,
        calls: &'a Cell<usize>,
        charged_at_closure: &'a Cell<usize>,
        settled: bool,
    }

    impl<'de> SeqAccess<'de> for ClosingSequence<'_> {
        type Error = serde_json::Error;

        fn next_element_seed<T: DeserializeSeed<'de>>(
            &mut self,
            seed: T,
        ) -> Result<Option<T::Value>, Self::Error> {
            let call = self.calls.get() + 1;
            self.calls.set(call);
            let value = seed.deserialize(serde::de::value::UnitDeserializer::new())?;
            if call == 4 {
                // Close only after the fourth element's own successful admission.
                self.charged_at_closure.set(self.original.captured_bytes());
                if self.settled {
                    self.original.settle();
                } else {
                    self.original.overflow();
                }
            }
            Ok(Some(value))
        }
    }

    #[test]
    fn array_callback_closure_refuses_append_without_next_callback_or_refund() {
        for settled in [false, true] {
            let original = operation();
            let charged = original.captured_bytes();
            let budget = Budget {
                operation: &original,
                failure: Cell::new(None),
            };
            let calls = Cell::new(0);
            let charged_at_closure = Cell::new(0);
            assert!(
                Tree(&budget)
                    .visit_seq(ClosingSequence {
                        original: &original,
                        calls: &calls,
                        charged_at_closure: &charged_at_closure,
                        settled,
                    })
                    .is_err()
            );
            let expected = if settled {
                OperationError::Settled
            } else {
                OperationError::CaptureOverflow
            };
            assert_eq!(calls.get(), 4);
            assert_eq!(budget.failure.get(), Some(expected));
            // Destination growth after the callback and a fifth element are refused;
            // the incomplete derived tree drops without refunding any admitted work.
            assert_eq!(original.captured_bytes(), charged_at_closure.get());
            assert!(
                charged_at_closure.get()
                    >= charged
                        + std::mem::size_of::<Vec<Value>>()
                        + 4 * std::mem::size_of::<Value>()
            );
            assert_eq!(original.check(), Err(expected));
            assert_eq!(original.start_evidence(), StartEvidence::NotStarted);
        }
    }
}
