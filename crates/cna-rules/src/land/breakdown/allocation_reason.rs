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
    BrokenPassengersExceedCapacity,
}

impl std::fmt::Debug for AllocationReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passenger(_) => {
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

// Input/arithmetic are constructed in losses::passenger_obligation; capacity is
// constructed only at baseline's actual broken-passenger capacity predicate.
// No serde, IDs/holdings/counters, unused stage/leaf scaffold or lint suppression.
// The later operator-private seam extends this SAME enum only with real producer hooks.
