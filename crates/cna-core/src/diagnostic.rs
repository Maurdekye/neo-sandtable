//! Opaque operator-private failure capture. It is not game state or an event.
//! The producing ruleset owns the vocabulary and encodes a bounded payload.
use std::{cell::RefCell, fmt};

pub const MAX_PRIVATE_PAYLOAD_BYTES: usize = 32768;

/// Already-encoded bytes for a trusted local operator. Intentionally not Clone
/// or Serialize: ordinary errors, state, projections and replay cannot carry it.
pub struct PrivatePayload(Vec<u8>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadBounds;

impl PrivatePayload {
    /// Total, nonallocating unavailable sentinel for a producer whose bounded
    /// encoding or fallible reservation failed. It is never a complete record.
    pub fn empty() -> Self {
        Self(Vec::new())
    }

    pub fn from_encoded(bytes: Vec<u8>) -> Result<Self, PayloadBounds> {
        if bytes.len() > MAX_PRIVATE_PAYLOAD_BYTES {
            return Err(PayloadBounds);
        }
        Ok(Self(bytes))
    }

    /// Trusted persistence/host-local reader only; never format in ordinary errors.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl fmt::Debug for PrivatePayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrivatePayload").field("len", &self.0.len()).finish()
    }
}
impl fmt::Display for PrivatePayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PrivatePayload(len={})", self.0.len())
    }
}

// A slot is installed only for a synchronous diagnosed evaluation on this thread.
// `claimed` precedes invoking the encoder, so a reentrant record cannot replace it.
#[derive(Default)]
struct Capture {
    claimed: bool,
    payload: Option<PrivatePayload>,
}
thread_local! {
    static ACTIVE: RefCell<Option<Capture>> = const { RefCell::new(None) };
}

/// Write-only first-cause capture. Outside a diagnosed evaluation this does
/// nothing and does not invoke `make`. Records from a different thread are not
/// captured. This is not an ambient tracing sink and must never span an await.
pub fn record(make: impl FnOnce() -> PrivatePayload) {
    let admitted = ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        match active.as_mut() {
            Some(slot) if !slot.claimed => { slot.claimed = true; true }
            _ => false,
        }
    });
    if admitted {
        // No RefCell borrow crosses caller code or a nested diagnosed scope.
        let payload = make();
        ACTIVE.with(|active| {
            if let Some(slot) = active.borrow_mut().as_mut() {
                slot.payload = Some(payload);
            }
        });
    }
}

struct Guard(Option<Capture>);
impl Drop for Guard {
    fn drop(&mut self) {
        ACTIVE.with(|active| *active.borrow_mut() = self.0.take());
    }
}

// Only the core evaluator can install/read a scope; rule producers only record.
pub(crate) fn scoped<T>(evaluate: impl FnOnce() -> T) -> (T, Option<PrivatePayload>) {
    let previous = ACTIVE.with(|active| active.replace(Some(Capture::default())));
    let guard = Guard(previous);
    let result = evaluate();
    let payload = ACTIVE.with(|active| active.borrow_mut().as_mut().and_then(|slot| slot.payload.take()));
    drop(guard);
    (result, payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_opaque_payload_debug_discloses_only_length() {
        assert!(PrivatePayload::from_encoded(vec![]).is_ok());
        assert!(PrivatePayload::from_encoded(vec![0; MAX_PRIVATE_PAYLOAD_BYTES + 1]).is_err());
        let small = PrivatePayload::from_encoded(b"hidden holdings".to_vec()).unwrap();
        let large = PrivatePayload::from_encoded(vec![0; MAX_PRIVATE_PAYLOAD_BYTES]).unwrap();
        assert_eq!(format!("{small:?}"), "PrivatePayload { len: 15 }");
        assert_eq!(format!("{large:?}"), "PrivatePayload { len: 32768 }");
        assert_eq!(small.as_bytes(), b"hidden holdings");
    }

    #[test]
    fn failed_bounded_encoding_can_return_total_empty_without_panicking() {
        let fallback = PrivatePayload::from_encoded(vec![0; MAX_PRIVATE_PAYLOAD_BYTES + 1])
            .unwrap_or_else(|_| PrivatePayload::empty());
        assert!(fallback.as_bytes().is_empty());
        let (_, captured) = scoped(|| record(|| fallback));
        assert!(captured.unwrap().as_bytes().is_empty());
        assert_eq!(format!("{:?}",PrivatePayload::empty()),"PrivatePayload { len: 0 }");
    }

    #[test]
    fn inactive_capture_never_runs_encoder() {
        record(|| panic!("inactive encoder ran"));
    }
    #[test]
    fn first_capture_wins_without_running_later_encoder() {
        let (_, payload) = scoped(|| {
            record(|| PrivatePayload::from_encoded(b"first".to_vec()).unwrap());
            record(|| panic!("later encoder ran"));
        });
        assert_eq!(payload.unwrap().as_bytes(), b"first");
    }

    #[test]
    fn another_thread_cannot_record_into_this_scope() {
        let (_, payload) = scoped(|| {
            std::thread::spawn(|| record(|| panic!("another thread inherited capture"))).join().unwrap();
            record(|| PrivatePayload::from_encoded(b"owner".to_vec()).unwrap());
        });
        assert_eq!(payload.unwrap().as_bytes(),b"owner");
    }
}
