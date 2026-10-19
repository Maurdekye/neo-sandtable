//! Admission for owned event payloads. Pure recorded-frame parsing uses no operation;
//! the runtime caller must supply its original read operation. No ownership is released.
//! Requested payload storage and the lead-set JSON map entry backing bound are
//! counted cumulatively. Allocator excess remains separate, not a proven RSS bound.

use cna_protocol::TranscriptEntry;
use serde_json::{Map, Value};

use crate::operation::{Operation, OperationError};

// mddec93a18791: lead-set structural/spare-capacity bound, NOT a mirror or
// proof of serde_json's private map layout. Child payloads charge separately.
// Owner-local representation only; no Server source/constant is imported.
pub(super) const JSON_MAP_ENTRY_BACKING_BYTES: usize = 16
    * (std::mem::size_of::<String>() + std::mem::size_of::<Value>() + std::mem::size_of::<usize>());

#[derive(Clone, Copy)]
pub(super) struct EventStorage<'a>(pub(super) Option<&'a Operation>);

impl EventStorage<'_> {
    fn check(self) -> Result<(), OperationError> {
        self.0.map_or(Ok(()), Operation::check)
    }

    fn overflow(self) -> OperationError {
        self.0
            .map_or(OperationError::CaptureOverflow, Operation::overflow)
    }

    pub(super) fn charge(self, bytes: Option<usize>) -> Result<(), OperationError> {
        // A byte charge alone does not check the original absolute deadline. Check
        // before arithmetic failure too, so a closed cause is not retagged overflow.
        self.check()?;
        let bytes = bytes.ok_or_else(|| self.overflow())?;
        self.0.map_or(Ok(()), |operation| operation.charge(bytes))
    }

    fn failure(self) -> OperationError {
        self.check().err().unwrap_or_else(|| self.overflow())
    }

    /// Charge once before fallible reserve. Reported capacity is not allocator RSS proof.
    fn string_buffer(self, length: Option<usize>) -> Result<String, OperationError> {
        self.charge(length.and_then(|n| n.checked_add(std::mem::size_of::<String>())))?;
        let length = length.expect("admitted string length");
        self.check()?;
        let mut text = String::new();
        text.try_reserve_exact(length).map_err(|_| self.failure())?;
        if self.0.is_some() && text.capacity() > length {
            return Err(self.failure());
        }
        self.check()?;
        Ok(text)
    }

    fn append_admitted(
        self,
        output: &mut String,
        fragment: &str,
        limit: usize,
    ) -> Result<(), OperationError> {
        self.check()?;
        let next = output.len().checked_add(fragment.len());
        if next.is_none_or(|next| next > limit || next > output.capacity()) {
            return Err(self.failure());
        }
        output.push_str(fragment); // Bounded by retained capacity; no append growth.
        Ok(())
    }

    pub(super) fn copy(self, text: &str) -> Result<String, OperationError> {
        let mut copy = self.string_buffer(Some(text.len()))?;
        self.append_admitted(&mut copy, text, text.len())?;
        self.check()?;
        Ok(copy)
    }

    /// Count this platform's encoded OS-string payload before its explicit owned copy.
    /// Encoding conversions/internal command storage and allocator excess are not RSS proof.
    pub(super) fn copy_os(
        self,
        text: &std::ffi::OsStr,
    ) -> Result<std::ffi::OsString, OperationError> {
        let length = text.as_encoded_bytes().len();
        self.charge(length.checked_add(std::mem::size_of::<std::ffi::OsString>()))?;
        self.check()?;
        let mut copy = std::ffi::OsString::new();
        // OsStr length and OsString capacity/reservation share encoding units.
        // Refuse an unexpected mismatch before reserve; never convert through Unicode.
        if text.len() != length {
            return Err(self.failure());
        }
        copy.try_reserve_exact(length).map_err(|_| self.failure())?;
        if (self.0.is_some() && copy.capacity() > length) || copy.capacity() < text.len() {
            return Err(self.failure());
        }
        self.check()?;
        copy.push(text); // Empty destination has room for the entire borrowed OS string.
        self.check()?;
        Ok(copy)
    }

    pub(super) fn optional(self, text: Option<&str>) -> Result<Option<String>, OperationError> {
        text.map(|text| self.copy(text)).transpose()
    }

    /// Count the actual rendered UTF-8 bytes before allocating a diagnostic copy.
    /// Formatter-internal library storage and allocator excess are separate fit obligations.
    pub(super) fn formatted(self, args: std::fmt::Arguments<'_>) -> Result<String, OperationError> {
        if let Some(operation) = self.0 {
            operation.check()?; // Do not invoke an opaque formatter after admission closed.
        }
        let failure = || {
            self.0
                .and_then(|operation| operation.check().err())
                .unwrap_or_else(|| self.overflow())
        };
        let limit = self.0.map_or(Ok(usize::MAX), |operation| {
            crate::operation::OPERATION_BYTES
                .checked_sub(operation.captured_bytes())
                .and_then(|remaining| remaining.checked_sub(std::mem::size_of::<String>()))
                .ok_or_else(|| failure())
        })?;
        struct Length {
            bytes: usize,
            limit: usize,
        }
        impl std::fmt::Write for Length {
            fn write_str(&mut self, text: &str) -> std::fmt::Result {
                let next = self.bytes.checked_add(text.len()).ok_or(std::fmt::Error)?;
                if next > self.limit {
                    return Err(std::fmt::Error);
                }
                self.bytes = next;
                Ok(())
            }
        }
        let mut length = Length { bytes: 0, limit };
        std::fmt::write(&mut length, args).map_err(|_| failure())?;
        let mut text = self.string_buffer(Some(length.bytes))?;
        struct AdmittedText<'a> {
            text: &'a mut String,
            limit: usize,
            storage: EventStorage<'a>,
        }
        impl std::fmt::Write for AdmittedText<'_> {
            fn write_str(&mut self, fragment: &str) -> std::fmt::Result {
                if let Some(operation) = self.storage.0 {
                    operation.check().map_err(|_| std::fmt::Error)?;
                }
                let next = self.text.len().checked_add(fragment.len());
                if next.is_none_or(|next| next > self.limit || next > self.text.capacity()) {
                    return Err(std::fmt::Error);
                }
                // Even a stateful second pass cannot grow this accepted buffer.
                self.text.push_str(fragment);
                Ok(())
            }
        }
        if let Some(operation) = self.0 {
            operation.check()?;
        }
        std::fmt::write(
            &mut AdmittedText {
                text: &mut text,
                limit: length.bytes,
                storage: self,
            },
            args,
        )
        .map_err(|_| failure())?;
        if let Some(operation) = self.0 {
            operation.check()?;
        }
        if text.len() != length.bytes {
            return Err(failure());
        }
        Ok(text)
    }

    pub(super) fn concat(self, parts: &[&str]) -> Result<String, OperationError> {
        let length = parts.iter().try_fold(0usize, |n, s| n.checked_add(s.len()));
        let mut result = self.string_buffer(length)?;
        let length = length.expect("admitted string length");
        for part in parts {
            self.append_admitted(&mut result, part, length)?;
        }
        self.check()?;
        Ok(result)
    }

    pub(super) fn reserve_next<T>(self, values: &mut Vec<T>) -> Result<(), OperationError> {
        if self.0.is_none() {
            return Ok(());
        }
        self.check()?; // Spare capacity does not reopen admission.
        if values.len() == values.capacity() {
            let capacity = values.capacity().checked_mul(2).map(|n| n.max(1));
            self.charge(capacity.and_then(|n| n.checked_mul(std::mem::size_of::<T>())))?;
            let capacity = capacity.expect("admitted vector capacity");
            values
                .try_reserve_exact(capacity - values.len())
                .map_err(|_| self.failure())?;
            if values.capacity() > capacity {
                // Retain the accepted prefix and resulting allocation on refusal.
                // This post-reserve check does not bound allocator-internal storage.
                return Err(self.failure());
            }
        }
        self.check()
    }

    pub(super) fn reserve_text_extension(
        self,
        text: &mut String,
        additional: usize,
    ) -> Result<(), OperationError> {
        self.check()?;
        let required = text
            .len()
            .checked_add(additional)
            .ok_or_else(|| self.overflow())?;
        if required > text.capacity() {
            // Charge the entire requested replacement buffer, including its accepted
            // prefix, BEFORE reserve/copy. Releasing old capacity never refunds this meter.
            // A reported excess is refused after reserve; this is not allocator RSS proof.
            self.charge(Some(required))?;
            text.try_reserve_exact(additional)
                .map_err(|_| self.failure())?;
            if text.capacity() > required {
                return Err(self.failure()); // The retained text/prefix is never cleared.
            }
        }
        self.check()
    }

    pub(super) fn push<T>(self, values: &mut Vec<T>, value: T) -> Result<(), OperationError> {
        self.reserve_next(values)?;
        self.check()?;
        values.push(value);
        Ok(())
    }

    pub(super) fn copy_value(self, value: &Value) -> Result<Value, OperationError> {
        if self.0.is_none() {
            return Ok(value.clone());
        }
        self.charge(Some(std::mem::size_of::<Value>()))?;
        Ok(match value {
            Value::Null => Value::Null,
            Value::Bool(value) => Value::Bool(*value),
            Value::Number(value) => Value::Number(value.clone()),
            Value::String(value) => Value::String(self.copy(value)?),
            Value::Array(values) => {
                self.charge(Some(std::mem::size_of::<Vec<Value>>()))?;
                let mut copy = Vec::new();
                for value in values {
                    self.push(&mut copy, self.copy_value(value)?)?;
                }
                Value::Array(copy)
            }
            Value::Object(values) => {
                self.charge(Some(std::mem::size_of::<Map<String, Value>>()))?;
                let mut copy = Map::new();
                for (key, value) in values {
                    const {
                        assert!(
                            std::mem::size_of::<(String, Value)>()
                                <= std::mem::size_of::<String>() + std::mem::size_of::<Value>()
                        );
                    }
                    // Whole entry backing BEFORE its key/value copies. If it
                    // refuses, no child allocation or map insert has occurred.
                    self.charge(Some(JSON_MAP_ENTRY_BACKING_BYTES))?;
                    let key = self.copy(key)?;
                    let value = self.copy_value(value)?;
                    copy.insert(key, value);
                }
                Value::Object(copy)
            }
        })
    }

    pub(super) fn content(self, value: &Value) -> Result<String, OperationError> {
        match value {
            Value::String(value) => self.copy(value),
            Value::Array(values) => {
                let mut count = 0usize;
                let length =
                    values
                        .iter()
                        .filter_map(|v| v["text"].as_str())
                        .try_fold(0usize, |n, text| {
                            let separator = usize::from(count != 0);
                            count = count.checked_add(1)?;
                            n.checked_add(separator)
                                .and_then(|n| n.checked_add(text.len()))
                        });
                let mut joined = self.string_buffer(length)?;
                let length = length.expect("admitted joined content");
                let mut first = true;
                for text in values.iter().filter_map(|v| v["text"].as_str()) {
                    if !first {
                        self.append_admitted(&mut joined, "\n", length)?;
                    }
                    first = false;
                    self.append_admitted(&mut joined, text, length)?;
                }
                self.check()?;
                Ok(joined)
            }
            _ => self.copy(""),
        }
    }

    pub(super) fn tool_result(
        self,
        call_id: String,
        ok: bool,
        text: &str,
    ) -> Result<TranscriptEntry, OperationError> {
        let Some(operation) = self.0 else {
            return Ok(super::tool_result_entry(call_id, ok, text));
        };
        let detail = if text.len() > 64 * 1024 {
            None
        } else {
            Some(match super::charged_json::decode(text, operation)? {
                Some(value) => value,
                None => {
                    self.charge(Some(std::mem::size_of::<Value>()))?;
                    Value::String(self.copy(text)?)
                }
            })
        };
        // Equivalent to flattening words and clip(..., 200), without an intermediate
        // String/Vec or growth while counting. UTF-8 bytes and the ellipsis are exact.
        let mut characters = normalized_chars(text);
        let length = characters
            .by_ref()
            .take(200)
            .try_fold(0usize, |n, c| n.checked_add(c.len_utf8()));
        let truncated = characters.next().is_some();
        let length =
            length.and_then(|n| n.checked_add(if truncated { '…'.len_utf8() } else { 0 }));
        let mut summary = self.string_buffer(length)?;
        let length = length.expect("admitted summary");
        let mut encoded = [0u8; 4];
        for character in normalized_chars(text).take(200) {
            self.append_admitted(&mut summary, character.encode_utf8(&mut encoded), length)?;
        }
        if truncated {
            self.append_admitted(&mut summary, "…", length)?;
        }
        self.check()?;
        Ok(TranscriptEntry::ToolResult {
            call_id,
            ok,
            summary,
            detail,
        })
    }
}

fn normalized_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    text.split_whitespace()
        .enumerate()
        .flat_map(|(index, word)| (index != 0).then_some(' ').into_iter().chain(word.chars()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::{Allowance, OPERATION_BYTES, StartEvidence};
    use tokio::time::Instant;

    fn event_operation() -> Operation {
        Operation::new(
            "event-original".into(),
            1,
            Allowance::required(Some(Instant::now() + std::time::Duration::from_secs(30))).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn owned_tool_result_matches_original_detail_and_unicode_summary_boundaries() {
        let texts = [
            String::new(),
            "  a\n\t b  ".into(),
            "β".repeat(200),
            "β".repeat(201),
            format!("{} β", "x".repeat(199)),
            "{\"text\":\"a\\nβ\",\"array\":[0,null]}".into(),
            "{ malformed".into(),
            "x".repeat(64 * 1024 + 1),
        ];
        for text in texts {
            let operation = event_operation();
            let owned = EventStorage(Some(&operation))
                .tool_result("c".into(), false, &text)
                .unwrap();
            assert_eq!(
                owned,
                super::super::tool_result_entry("c".into(), false, &text)
            );
            assert_eq!(operation.start_evidence(), StartEvidence::NotStarted);
        }
    }

    #[test]
    fn joined_tool_content_counts_exact_newlines_before_allocating() {
        let operation = event_operation();
        let operation_baseline = operation.captured_bytes();
        let value = serde_json::json!([{"text":""}, {}, {"text":7}, {"text":"β"}, {"text":"last"}]);
        let storage = EventStorage(Some(&operation));
        assert_eq!(storage.content(&value).unwrap(), "\nβ\nlast");
        assert_eq!(
            operation.captured_bytes(),
            operation_baseline + std::mem::size_of::<String>() + "\nβ\nlast".len()
        );
        let refused = event_operation();
        refused
            .charge(
                OPERATION_BYTES
                    - refused.captured_bytes()
                    - (std::mem::size_of::<String>() + "\nβ\nlast".len() - 1),
            )
            .unwrap();
        let before = refused.captured_bytes();
        assert_eq!(
            EventStorage(Some(&refused)).content(&value),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(refused.captured_bytes(), before);
        assert_eq!(value[3]["text"], "β");
    }

    #[test]
    fn diagnostic_format_admits_exact_utf8_before_allocation_without_refund() {
        let operation = event_operation();
        let operation_baseline = operation.captured_bytes();
        let text = EventStorage(Some(&operation))
            .formatted(format_args!(
                "account {} expected {:?}",
                "λ@example.invalid", "a\"b"
            ))
            .unwrap();
        assert_eq!(text, "account λ@example.invalid expected \"a\\\"b\"");
        let required = text.len() + std::mem::size_of::<String>();
        assert_eq!(operation.captured_bytes(), operation_baseline + required);
        let refused = event_operation();
        refused.possible_start();
        refused
            .charge(OPERATION_BYTES - refused.captured_bytes() - required + 1)
            .unwrap();
        let before = refused.captured_bytes();
        assert_eq!(
            EventStorage(Some(&refused)).formatted(format_args!(
                "account {} expected {:?}",
                "λ@example.invalid", "a\"b"
            )),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(refused.captured_bytes(), before);
        assert_eq!(refused.start_evidence(), StartEvidence::Possible);
    }

    struct ChangingDisplay {
        calls: std::cell::Cell<usize>,
        first: &'static str,
        second: &'static str,
        close: Option<Operation>,
    }
    impl std::fmt::Display for ChangingDisplay {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let call = self.calls.get();
            self.calls.set(call + 1);
            if call == 0 {
                if let Some(operation) = &self.close {
                    operation.settle(); // Synthetic closure during measurement, no join ACK.
                }
                formatter.write_str(self.first)
            } else {
                formatter.write_str(self.second)
            }
        }
    }

    // SOURCE ONLY/UNRUN: no child, filesystem, provider or opaque error callback.
    #[test]
    fn closed_diagnostic_operation_refuses_before_first_formatter_call() {
        for settled in [false, true] {
            let original = event_operation();
            original.possible_start();
            let charged = original.captured_bytes();
            if settled {
                original.settle();
            } else {
                original.overflow();
            }
            let display = ChangingDisplay {
                calls: std::cell::Cell::new(0),
                first: "canonical",
                second: "canonical",
                close: None,
            };
            assert_eq!(
                EventStorage(Some(&original)).formatted(format_args!("{display}")),
                Err(if settled {
                    OperationError::Settled
                } else {
                    OperationError::CaptureOverflow
                })
            );
            assert_eq!(display.calls.get(), 0);
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.start_evidence(), StartEvidence::Possible);
        }
    }

    #[test]
    fn diagnostic_measurement_stops_at_remaining_original_credit_before_allocation() {
        let original = event_operation();
        original
            .charge(OPERATION_BYTES - original.captured_bytes() - std::mem::size_of::<String>() - 2)
            .unwrap();
        let charged = original.captured_bytes();
        let display = ChangingDisplay {
            calls: std::cell::Cell::new(0),
            first: "base",
            second: "must not render",
            close: None,
        };
        assert_eq!(
            EventStorage(Some(&original)).formatted(format_args!("{display}")),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(display.calls.get(), 1);
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(original.check(), Err(OperationError::CaptureOverflow));
    }

    #[test]
    fn changed_second_pass_cannot_grow_or_hide_original_diagnostic_charge() {
        for (first, second) in [
            ("base", "unadmitted longer body"),
            ("base", "b"),
            ("β", "βγ"),
        ] {
            let original = event_operation();
            original.possible_start();
            let charged = original.captured_bytes();
            let display = ChangingDisplay {
                calls: std::cell::Cell::new(0),
                first,
                second,
                close: None,
            };
            assert_eq!(
                EventStorage(Some(&original)).formatted(format_args!("{display}")),
                Err(OperationError::CaptureOverflow)
            );
            assert_eq!(display.calls.get(), 2);
            assert_eq!(
                original.captured_bytes(),
                charged + first.len() + std::mem::size_of::<String>()
            );
            assert_eq!(original.check(), Err(OperationError::CaptureOverflow));
            assert_eq!(original.start_evidence(), StartEvidence::Possible);
        }
    }

    #[test]
    fn closure_during_measurement_keeps_original_cause_without_second_pass_or_refund() {
        let original = event_operation();
        let charged = original.captured_bytes();
        let display = ChangingDisplay {
            calls: std::cell::Cell::new(0),
            first: "canonical",
            second: "must not render",
            close: Some(original.clone()),
        };
        assert_eq!(
            EventStorage(Some(&original)).formatted(format_args!("{display}")),
            Err(OperationError::Settled)
        );
        assert_eq!(display.calls.get(), 1);
        assert_eq!(original.captured_bytes(), charged);
        assert_eq!(original.check(), Err(OperationError::Settled));
    }

    #[test]
    fn os_copy_counts_encoded_payload_before_allocating_and_keeps_original_on_refusal() {
        let original = std::ffi::OsString::from("original profile λ \\ space");
        let pointer = original.as_encoded_bytes().as_ptr();
        let bytes = original.as_encoded_bytes().len() + std::mem::size_of::<std::ffi::OsString>();
        let operation = event_operation();
        let operation_baseline = operation.captured_bytes();
        let copied = EventStorage(Some(&operation)).copy_os(&original).unwrap();
        assert_eq!(copied, original);
        assert_eq!(operation.captured_bytes(), operation_baseline + bytes);
        let refused = event_operation();
        refused.possible_start();
        refused
            .charge(OPERATION_BYTES - refused.captured_bytes() - bytes + 1)
            .unwrap();
        let before = refused.captured_bytes();
        assert_eq!(
            EventStorage(Some(&refused)).copy_os(&original),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(original.as_encoded_bytes().as_ptr(), pointer);
        assert_eq!(refused.captured_bytes(), before);
        assert_eq!(refused.start_evidence(), StartEvidence::Possible);
    }

    // SOURCE ONLY/UNRUN: preserve Windows OS bytes without a lossy Unicode round trip.
    #[cfg(windows)]
    #[test]
    fn fallible_os_copy_preserves_unpaired_surrogates_and_encoded_charge() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let units = [0xd800, 0x0061, 0xdc00, 0x0000, 0xd83d, 0xde00];
        let raw = std::ffi::OsString::from_wide(&units);
        let pointer = raw.as_encoded_bytes().as_ptr();
        let capacity = raw.capacity();
        let original = event_operation();
        let before = original.captured_bytes();
        let copied = EventStorage(Some(&original)).copy_os(&raw).unwrap();
        assert_eq!(copied.encode_wide().collect::<Vec<_>>(), units);
        assert_eq!(copied.as_encoded_bytes(), raw.as_encoded_bytes());
        assert_eq!(copied.capacity(), raw.as_encoded_bytes().len());
        assert_ne!(copied.as_encoded_bytes().as_ptr(), pointer);
        assert_eq!(
            original.captured_bytes(),
            before + raw.as_encoded_bytes().len() + std::mem::size_of::<std::ffi::OsString>()
        );
        assert_eq!(raw.as_encoded_bytes().as_ptr(), pointer);
        assert_eq!(raw.capacity(), capacity);
        assert_eq!(EventStorage(None).copy_os(&raw).unwrap(), raw);
        assert_eq!(original.start_evidence(), StartEvidence::NotStarted);
    }

    #[test]
    fn text_extension_reuses_admitted_capacity_and_closes_arithmetic_overflow() {
        let operation = event_operation();
        let mut text = EventStorage(Some(&operation)).copy("retained").unwrap();
        let extra = text.capacity() + 1;
        let required = text.len() + extra;
        let before = operation.captured_bytes();
        EventStorage(Some(&operation))
            .reserve_text_extension(&mut text, extra)
            .unwrap();
        assert_eq!(text, "retained");
        assert!(text.capacity() >= required);
        assert_eq!(operation.captured_bytes(), before + required);
        let pointer = text.as_ptr();
        let charged = operation.captured_bytes();
        EventStorage(Some(&operation))
            .reserve_text_extension(&mut text, extra)
            .unwrap();
        assert_eq!(text.as_ptr(), pointer);
        assert_eq!(operation.captured_bytes(), charged); // Same in-service request, no new allocation.
        operation.possible_start();
        assert_eq!(
            EventStorage(Some(&operation)).reserve_text_extension(&mut text, usize::MAX),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(text.as_ptr(), pointer);
        assert_eq!(text, "retained");
        assert_eq!(operation.captured_bytes(), charged);
        assert_eq!(operation.start_evidence(), StartEvidence::Possible);
    }

    #[test]
    fn value_copy_refusal_preserves_borrowed_body_and_original_liability() {
        let operation = event_operation();
        operation.possible_start();
        let value = Value::String("original body".into());
        let pointer = value.as_str().unwrap().as_ptr();
        let bytes =
            std::mem::size_of::<Value>() + std::mem::size_of::<String>() + "original body".len();
        operation
            .charge(OPERATION_BYTES - operation.captured_bytes() - bytes + 1)
            .unwrap();
        let before = operation.captured_bytes();
        assert_eq!(
            EventStorage(Some(&operation)).copy_value(&value),
            Err(OperationError::CaptureOverflow)
        );
        assert_eq!(value.as_str().unwrap().as_ptr(), pointer);
        assert_eq!(
            operation.captured_bytes(),
            before + std::mem::size_of::<Value>()
        );
        assert_eq!(operation.start_evidence(), StartEvidence::Possible);
    }

    // SOURCE ONLY/UNRUN: fixture closure is not a child/reader/settlement ACK.
    #[test]
    fn closed_storage_refuses_spare_capacity_without_mutating_retained_prefix() {
        for settled in [false, true] {
            let original = event_operation();
            let storage = EventStorage(Some(&original));
            let mut values = Vec::new();
            storage.push(&mut values, 7u64).unwrap();
            while values.len() < values.capacity() {
                storage.push(&mut values, 7).unwrap();
            }
            storage.reserve_next(&mut values).unwrap();
            assert!(values.len() < values.capacity());
            let mut text = storage.copy("retained").unwrap();
            storage.reserve_text_extension(&mut text, 32).unwrap();
            assert!(text.len() < text.capacity());
            let values_pointer = values.as_ptr();
            let values_length = values.len();
            let values_capacity = values.capacity();
            let text_pointer = text.as_ptr();
            let text_capacity = text.capacity();
            let charged = original.captured_bytes();
            original.possible_start();
            let expected = if settled {
                original.settle();
                OperationError::Settled
            } else {
                original.overflow()
            };
            for _ in 0..2 {
                assert_eq!(storage.reserve_next(&mut values), Err(expected));
                assert_eq!(storage.push(&mut values, 9), Err(expected));
                assert_eq!(storage.reserve_text_extension(&mut text, 1), Err(expected));
                assert_eq!(values.as_ptr(), values_pointer);
                assert_eq!(values.len(), values_length);
                assert_eq!(values.capacity(), values_capacity);
                assert!(values.iter().all(|value| *value == 7));
                assert_eq!(text.as_ptr(), text_pointer);
                assert_eq!(text.capacity(), text_capacity);
                assert_eq!(text, "retained");
                assert_eq!(original.captured_bytes(), charged);
                assert_eq!(original.check(), Err(expected));
                assert_eq!(original.start_evidence(), StartEvidence::Possible);
            }
        }
    }

    #[test]
    fn closed_storage_keeps_original_cause_before_copy_or_arithmetic_failure() {
        for settled in [false, true] {
            let original = event_operation();
            let storage = EventStorage(Some(&original));
            let raw = Value::String("canonical borrowed body".into());
            let pointer = raw.as_str().unwrap().as_ptr();
            let mut text = storage.copy("retained").unwrap();
            let text_pointer = text.as_ptr();
            let text_capacity = text.capacity();
            let charged = original.captured_bytes();
            let expected = if settled {
                original.settle();
                OperationError::Settled
            } else {
                original.overflow()
            };
            assert_eq!(storage.charge(None), Err(expected));
            assert_eq!(storage.copy("new"), Err(expected));
            assert_eq!(storage.copy_os(std::ffi::OsStr::new("new")), Err(expected));
            assert_eq!(storage.copy_value(&raw), Err(expected));
            assert_eq!(storage.content(&raw), Err(expected));
            assert_eq!(storage.concat(&["new", " body"]), Err(expected));
            assert_eq!(
                storage.reserve_text_extension(&mut text, usize::MAX),
                Err(expected)
            );
            assert_eq!(raw.as_str().unwrap().as_ptr(), pointer);
            assert_eq!(raw.as_str(), Some("canonical borrowed body"));
            assert_eq!(text.as_ptr(), text_pointer);
            assert_eq!(text.capacity(), text_capacity);
            assert_eq!(text, "retained");
            assert_eq!(original.captured_bytes(), charged);
            assert_eq!(original.check(), Err(expected));
        }
        // The unmetered recorded-frame fixture lane still has no closed operation.
        let mut values = Vec::new();
        EventStorage(None).push(&mut values, 7u64).unwrap();
        assert_eq!(values, [7]);
        assert_eq!(EventStorage(None).copy("recorded").unwrap(), "recorded");
    }

    #[test]
    fn event_vector_growth_refusal_keeps_accepted_prefix_without_refund() {
        let operation = event_operation();
        operation.possible_start();
        let storage = EventStorage(Some(&operation));
        let mut accepted = Vec::new();
        storage.push(&mut accepted, 7u64).unwrap();
        while accepted.len() < accepted.capacity() {
            storage.push(&mut accepted, 7).unwrap();
        }
        let pointer = accepted.as_ptr();
        let count = accepted.len();
        let capacity = accepted.capacity();
        operation
            .charge(OPERATION_BYTES - operation.captured_bytes() - 1)
            .unwrap();
        let before = operation.captured_bytes();
        for _ in 0..2 {
            assert_eq!(
                storage.push(&mut accepted, 9),
                Err(OperationError::CaptureOverflow)
            );
            assert_eq!(accepted.as_ptr(), pointer);
            assert_eq!(accepted.len(), count);
            assert_eq!(accepted.capacity(), capacity);
            assert!(accepted.iter().all(|v| *v == 7));
            assert_eq!(operation.captured_bytes(), before);
            assert_eq!(operation.start_evidence(), StartEvidence::Possible);
        }
    }

    // SOURCE ONLY/UNRUN: real allocator refusal is not injected by these controls.
    #[test]
    fn fallible_event_strings_keep_exact_utf8_and_single_original_charge() {
        for text in ["", "canonical", "\u{03b2}\n\u{1f600}"] {
            let original = event_operation();
            let before = original.captured_bytes();
            let storage = EventStorage(Some(&original));
            let copied = storage.copy(text).unwrap();
            assert_eq!(copied, text);
            assert_eq!(copied.capacity(), text.len());
            assert_eq!(
                original.captured_bytes(),
                before + text.len() + std::mem::size_of::<String>()
            );
            let before = original.captured_bytes();
            let joined = storage.concat(&["", text, "\n", "tail"]).unwrap();
            assert_eq!(joined, format!("{text}\ntail"));
            assert_eq!(joined.capacity(), joined.len());
            assert_eq!(
                original.captured_bytes(),
                before + joined.len() + std::mem::size_of::<String>()
            );
            assert_eq!(original.start_evidence(), StartEvidence::NotStarted);
        }
    }

    #[test]
    fn bounded_event_append_keeps_prepared_prefix_on_growth_or_closed_refusal() {
        for cause in 0..3 {
            let original = event_operation();
            original.possible_start();
            let storage = EventStorage(Some(&original));
            let mut prefix = storage.string_buffer(Some(16)).unwrap();
            storage
                .append_admitted(&mut prefix, "retained", 16)
                .unwrap();
            let pointer = prefix.as_ptr();
            let capacity = prefix.capacity();
            let charged = original.captured_bytes();
            let fragment = String::from(if cause == 0 {
                "this would require destination growth"
            } else {
                "x"
            });
            let fragment_pointer = fragment.as_ptr();
            let expected = match cause {
                0 => OperationError::CaptureOverflow,
                1 => original.overflow(),
                _ => {
                    original.settle(); // Fixture closure, no actual supervision ACK.
                    OperationError::Settled
                }
            };
            for _ in 0..2 {
                assert_eq!(
                    storage.append_admitted(&mut prefix, &fragment, 16),
                    Err(expected)
                );
                assert_eq!(prefix, "retained");
                assert_eq!(prefix.as_ptr(), pointer);
                assert_eq!(prefix.capacity(), capacity);
                assert_eq!(fragment.as_ptr(), fragment_pointer);
                assert_eq!(original.captured_bytes(), charged);
                assert_eq!(original.check(), Err(expected));
                assert_eq!(original.start_evidence(), StartEvidence::Possible);
            }
        }
    }
}
