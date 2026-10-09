//! Repair-owned reasons emitted by actual passenger accounting branches.
//! No wire representation or complete search/stop-reason availability claim.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AllocationReason {
    Passenger(PassengerReason),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PassengerReason {
    InvalidInput,
    Arithmetic,
}

impl std::fmt::Debug for AllocationReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passenger(PassengerReason::InvalidInput | PassengerReason::Arithmetic) => {
                f.write_str("AllocationReason(<redacted>)")
            }
        }
    }
}
impl std::fmt::Display for AllocationReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}
impl std::fmt::Debug for PassengerReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PassengerReason(<redacted>)")
    }
}
impl std::fmt::Display for PassengerReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PassengerReason(<redacted>)")
    }
}

// Both cases are constructed in losses::passenger_obligation's actual branches.
// No serde, IDs/holdings/counters, unused stage/leaf scaffold or lint suppression.
// The later operator-private seam extends this SAME enum only with real producer hooks.
