//! Stable identifiers: sides, roles, seats, units, hexes, decisions.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use cna_protocol::{Role, Side};

impl From<cna_protocol::UnknownName> for IdError {
    fn from(e: cna_protocol::UnknownName) -> Self {
        IdError {
            kind: e.kind,
            value: e.value,
        }
    }
}

/// A command seat, written `<side>.<role>` (e.g. `axis.logistics`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SeatId {
    pub side: Side,
    pub role: Role,
}

impl SeatId {
    pub const fn new(side: Side, role: Role) -> Self {
        Self { side, role }
    }

    /// All ten default seats, in a stable order.
    pub fn all() -> impl Iterator<Item = SeatId> {
        Side::ALL
            .into_iter()
            .flat_map(|side| Role::ALL.into_iter().map(move |role| SeatId { side, role }))
    }
}

impl fmt::Display for SeatId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.side, self.role)
    }
}

impl FromStr for SeatId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (side, role) = s.split_once('.').ok_or_else(|| IdError::new("seat", s))?;
        Ok(SeatId {
            side: side.parse()?,
            role: role.parse()?,
        })
    }
}

impl Serialize for SeatId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SeatId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }
    };
}

string_id!(
    /// A unit's stable id (e.g. `it.1ccnn_div.219_lgn.129`). Never a display name.
    UnitId
);
string_id!(
    /// A hex's printed id (e.g. `C4218`), as defined by `data/map`.
    HexId
);
string_id!(
    /// An open or resolved decision window.
    DecisionId
);

/// A malformed identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdError {
    pub kind: &'static str,
    pub value: String,
}

impl IdError {
    fn new(kind: &'static str, value: &str) -> Self {
        Self {
            kind,
            value: value.to_owned(),
        }
    }
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {} id: {:?}", self.kind, self.value)
    }
}

impl std::error::Error for IdError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seat_ids_round_trip() {
        for seat in SeatId::all() {
            let text = seat.to_string();
            assert_eq!(text.parse::<SeatId>().unwrap(), seat);
        }
        assert_eq!(SeatId::all().count(), 10);
        assert_eq!(
            "axis.front_line".parse::<SeatId>().unwrap(),
            SeatId::new(Side::Axis, Role::FrontLine)
        );
        assert!("axis.navy".parse::<SeatId>().is_err());
        assert!("axis".parse::<SeatId>().is_err());
    }

    #[test]
    fn seat_ids_serialize_as_strings() {
        let seat = SeatId::new(Side::Commonwealth, Role::Logistics);
        let json = serde_json::to_string(&seat).unwrap();
        assert_eq!(json, "\"commonwealth.logistics\"");
        assert_eq!(serde_json::from_str::<SeatId>(&json).unwrap(), seat);
    }
}
