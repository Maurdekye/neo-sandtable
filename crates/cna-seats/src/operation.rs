//! Trusted operation ownership. These records never enter the spectator protocol.
//!
//! Encoded caps bound accepted bytes, not the allocator's RSS. A timeout closes admission;
//! only actual supervision settlement releases ownership. ACKs and drops release nothing.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::time::Instant;

pub const FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const OPERATION_BYTES: usize = 16 * 1024 * 1024;
pub const STDOUT_CHUNK_BYTES: usize = 4096;
pub const STDERR_BYTES: usize = 64 * 1024;
pub const STDERR_LINES: usize = 40;
pub const COMMAND_BYTES: usize = 64 * 1024;
pub const DELIVERY_BYTES: usize = 2 * 1024 * 1024;
pub const PREPARE_BYTES: usize = 64 * 1024;
pub const MIRROR_BYTES: usize = 4 * 1024 * 1024;
pub const MIRROR_ENTRIES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OperationError {
    #[error("no_allowance")]
    NoAllowance,
    #[error("operation allowance exhausted")]
    Expired,
    #[error("capture_overflow")]
    CaptureOverflow,
    #[error("operation already settled")]
    Settled,
    #[error("operation credit still owned")]
    CreditOwned,
}

/// A host-derived absolute deadline. Cloning never renews its allowance.
#[derive(Clone, Copy, Debug)]
pub struct Allowance(Instant);

impl Allowance {
    pub fn required(deadline: Option<Instant>) -> Result<Self, OperationError> {
        let value = Self(deadline.ok_or(OperationError::NoAllowance)?);
        value.check()?;
        Ok(value)
    }

    pub fn deadline(self) -> Instant {
        self.0
    }

    pub fn check(self) -> Result<(), OperationError> {
        if Instant::now() >= self.0 { Err(OperationError::Expired) } else { Ok(()) }
    }

    /// Existing local ceilings may shorten, never extend, the parent allowance.
    pub fn capped(self, ceiling: std::time::Duration) -> Self {
        Self(self.0.min(Instant::now() + ceiling))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureOrigin {
    pub operation_id: String,
    pub child_generation: u64,
    pub ordinal: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationFrame {
    MessageStart,
    Assistant,
    ResultUsage,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "classification", rename_all = "snake_case")]
pub enum StartEvidence {
    #[default]
    NotStarted,
    Possible,
    Confirmed { frame: ConfirmationFrame, ordinal: u64 },
}

struct State {
    captured_bytes: usize,
    ordinal: u64,
    overflow: bool,
    settled: bool,
    start: StartEvidence,
    command_owned: bool,
}

/// The same handle owns reader/capture liability and the canonical receipt through final
/// settlement. There is no automatic release on clone/drop, dispatch ACK or deadline expiry.
#[derive(Clone)]
pub struct Operation {
    id: Arc<str>,
    child_generation: u64,
    allowance: Allowance,
    state: Arc<Mutex<State>>,
    overflowed: Arc<Notify>,
}

impl Operation {
    pub fn new(id: String, child_generation: u64, allowance: Allowance) -> Result<Self, OperationError> {
        allowance.check()?;
        Ok(Self {
            id: id.into(), child_generation, allowance,
            state: Arc::new(Mutex::new(State {
                captured_bytes: 0, ordinal: 0, overflow: false, settled: false,
                start: StartEvidence::NotStarted,
                command_owned: false,
            })),
            overflowed: Arc::new(Notify::new()),
        })
    }

    pub fn allowance(&self) -> Allowance { self.allowance }

    pub fn id(&self) -> &str { &self.id }

    pub fn check(&self) -> Result<(), OperationError> {
        let state = self.state.lock().expect("operation state");
        if state.overflow { return Err(OperationError::CaptureOverflow); }
        if state.settled { return Err(OperationError::Settled); }
        drop(state);
        self.allowance.check()
    }

    /// Admission precedes acceptance or copying. A failed charge is sticky; the successful
    /// prefix remains owned and no ACK/dequeue/eviction refunds this cumulative meter.
    pub fn charge(&self, bytes: usize) -> Result<(), OperationError> {
        let mut state = self.state.lock().expect("operation state");
        if state.overflow { return Err(OperationError::CaptureOverflow); }
        if state.settled { return Err(OperationError::Settled); }
        match state.captured_bytes.checked_add(bytes).filter(|n| *n <= OPERATION_BYTES) {
            Some(total) => { state.captured_bytes = total; Ok(()) }
            None => {
                state.overflow = true;
                drop(state);
                self.overflowed.notify_one();
                Err(OperationError::CaptureOverflow)
            }
        }
    }

    pub fn overflow(&self) -> OperationError {
        self.state.lock().expect("operation state").overflow = true;
        self.overflowed.notify_one();
        OperationError::CaptureOverflow
    }

    /// Register before checking the latch so a concurrent overflow cannot become a lost wake.
    pub async fn wait_overflow(&self) {
        loop {
            let notified = self.overflowed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.state.lock().expect("operation state").overflow { return; }
            notified.await;
        }
    }

    pub fn next_origin(&self) -> Result<CaptureOrigin, OperationError> {
        let mut state = self.state.lock().expect("operation state");
        if state.overflow { return Err(OperationError::CaptureOverflow); }
        if state.settled { return Err(OperationError::Settled); }
        let Some(ordinal) = state.ordinal.checked_add(1) else {
            drop(state);
            return Err(self.overflow());
        };
        state.ordinal = ordinal;
        Ok(CaptureOrigin {
            operation_id: self.id.to_string(), child_generation: self.child_generation, ordinal,
        })
    }

    /// Call after a completed write, or conservatively when an in-progress transfer is canceled.
    pub fn possible_start(&self) {
        let mut state = self.state.lock().expect("operation state");
        if !state.settled && state.start == StartEvidence::NotStarted { state.start = StartEvidence::Possible; }
    }

    /// The reader must establish the operation/session boundary before issuing an origin.
    /// Stale/unattributed rows are never repaired by the current active pointer.
    pub fn confirm(&self, origin: &CaptureOrigin, frame: ConfirmationFrame) -> bool {
        let mut state = self.state.lock().expect("operation state");
        if state.settled || origin.operation_id != self.id.as_ref()
            || origin.child_generation != self.child_generation || origin.ordinal == 0
            || origin.ordinal > state.ordinal || state.start != StartEvidence::Possible
        { return false; }
        state.start = StartEvidence::Confirmed { frame, ordinal: origin.ordinal };
        true
    }

    pub fn start_evidence(&self) -> StartEvidence {
        self.state.lock().expect("operation state").start.clone()
    }

    pub fn captured_bytes(&self) -> usize {
        self.state.lock().expect("operation state").captured_bytes
    }

    fn claim_command_credit(&self) -> Result<(), OperationError> {
        self.check()?;
        let mut state = self.state.lock().expect("operation state");
        if state.command_owned { return Err(OperationError::CreditOwned); }
        if state.settled { return Err(OperationError::Settled); }
        if state.overflow { return Err(OperationError::CaptureOverflow); }
        state.command_owned = true;
        Ok(())
    }

    /// Only the owning supervisor calls this after actual child/reader/storage settlement.
    /// Timeout itself is not settlement, and settlement never changes start/spend evidence.
    pub fn settle(&self) {
        self.state.lock().expect("operation state").settled = true;
    }
}

/// Counts encoded JSON without first allocating an oversized string or cloning its body.
pub fn encoded_len(value: &impl Serialize, cap: usize) -> Result<usize, OperationError> {
    struct Counter { bytes: usize, cap: usize }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes = self.bytes.checked_add(bytes.len()).filter(|n| *n <= self.cap)
                .ok_or_else(|| io::Error::other("capture_overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }
    let mut counter = Counter { bytes: 0, cap };
    serde_json::to_writer(&mut counter, value).map_err(|_| OperationError::CaptureOverflow)?;
    Ok(counter.bytes)
}

/// One COMMAND *operation*, with its receipt and settlement envelope under one live subcap.
/// Replacing/dropping an envelope does not refund the cumulative operation charge.
pub struct CommandCredit<R> {
    operation: Operation,
    receipt: Option<R>,
    live_bytes: usize,
}

impl<R: Serialize> CommandCredit<R> {
    pub fn new(operation: Operation) -> Result<Self, OperationError> {
        operation.claim_command_credit()?;
        Ok(Self { operation, receipt: None, live_bytes: 0 })
    }

    pub fn retain_receipt(&mut self, receipt: R) -> Result<(), OperationError> {
        if self.receipt.is_some() { return Err(OperationError::CreditOwned); }
        // An actual accepted receipt is never erased even when its retention charge refuses.
        // The caller already owns R; move it here without cloning, keep it on every error.
        self.receipt = Some(receipt);
        let bytes = encoded_len(self.receipt.as_ref().expect("retained receipt"), COMMAND_BYTES)
            .map_err(|_| self.operation.overflow())?;
        self.operation.charge(bytes)?;
        self.live_bytes = bytes;
        Ok(())
    }

    /// Check R+C+metadata together BEFORE the caller copies/accepts C. R survives refusal.
    pub fn admit_settlement(&mut self, command: &impl Serialize) -> Result<usize, OperationError> {
        self.operation.check()?;
        let remaining = COMMAND_BYTES.checked_sub(self.live_bytes)
            .ok_or_else(|| self.operation.overflow())?;
        let bytes = encoded_len(command, remaining).map_err(|_| self.operation.overflow())?;
        self.operation.charge(bytes)?;
        self.live_bytes += bytes;
        Ok(bytes)
    }

    pub fn receipt(&self) -> Option<&R> { self.receipt.as_ref() }

    pub fn operation(&self) -> &Operation { &self.operation }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn operation() -> Operation {
        Operation::new("op".into(), 1,
            Allowance::required(Some(Instant::now() + std::time::Duration::from_secs(1))).unwrap()).unwrap()
    }

    #[test]
    fn missing_or_exhausted_allowance_refuses() {
        assert!(matches!(Allowance::required(None), Err(OperationError::NoAllowance)));
        assert!(matches!(Allowance::required(Some(Instant::now())), Err(OperationError::Expired)));
    }

    #[test]
    fn command_receipt_and_settlement_exact_fit_or_receipt_kept() {
        let command = json!({"marker":2,"metadata":"known"});
        let command_bytes = encoded_len(&command, COMMAND_BYTES).unwrap();
        let receipt = "r".repeat(COMMAND_BYTES - command_bytes - 2);
        let mut credit = CommandCredit::new(operation()).unwrap();
        credit.retain_receipt(receipt.clone()).unwrap();
        assert_eq!(credit.admit_settlement(&command).unwrap(), command_bytes);
        assert_eq!(credit.live_bytes, COMMAND_BYTES);
        assert_eq!(credit.admit_settlement(&json!(null)), Err(OperationError::CaptureOverflow));
        assert_eq!(credit.receipt(), Some(&receipt));
        assert_eq!(credit.operation.check(), Err(OperationError::CaptureOverflow));
    }

    #[test]
    fn copies_exhaust_total_without_refunding_receipt() {
        let op = operation();
        let mut credit = CommandCredit::new(op.clone()).unwrap();
        credit.retain_receipt(json!({"accepted":true})).unwrap();
        let used = op.captured_bytes();
        op.charge(OPERATION_BYTES - used).unwrap();
        assert_eq!(credit.admit_settlement(&json!({"finish":true})), Err(OperationError::CaptureOverflow));
        assert_eq!(credit.receipt(), Some(&json!({"accepted":true})));
        assert_eq!(op.captured_bytes(), OPERATION_BYTES);
    }

    #[test]
    fn actual_oversize_receipt_is_retained_on_subcap_refusal() {
        let receipt = "r".repeat(COMMAND_BYTES);
        let mut credit = CommandCredit::new(operation()).unwrap();
        assert_eq!(credit.retain_receipt(receipt.clone()), Err(OperationError::CaptureOverflow));
        assert_eq!(credit.receipt(), Some(&receipt));
    }

    #[test]
    fn one_command_credit_counts_operation_not_ack_or_handle_drop() {
        let operation = operation();
        let credit = CommandCredit::<serde_json::Value>::new(operation.clone()).unwrap();
        assert!(matches!(CommandCredit::<serde_json::Value>::new(operation.clone()), Err(OperationError::CreditOwned)));
        drop(credit);
        assert!(matches!(CommandCredit::<serde_json::Value>::new(operation.clone()), Err(OperationError::CreditOwned)));
        operation.settle();
        assert!(matches!(CommandCredit::<serde_json::Value>::new(operation), Err(OperationError::Settled)));
    }

    #[test]
    fn escaped_json_size_is_encoded_and_confirmations_are_monotonic() {
        assert_eq!(encoded_len(&"\n", 3), Err(OperationError::CaptureOverflow));
        assert_eq!(encoded_len(&"\n", 4).unwrap(), 4);
        let op = operation();
        let origin = op.next_origin().unwrap();
        assert!(!op.confirm(&origin, ConfirmationFrame::Assistant));
        op.possible_start();
        let mut stale = origin.clone(); stale.child_generation = 0;
        assert!(!op.confirm(&stale, ConfirmationFrame::Assistant));
        assert!(op.confirm(&origin, ConfirmationFrame::Assistant));
        assert!(!op.confirm(&origin, ConfirmationFrame::ResultUsage));
        op.possible_start(); op.settle();
        assert_eq!(op.start_evidence(), StartEvidence::Confirmed { frame: ConfirmationFrame::Assistant, ordinal: 1 });
    }
}
