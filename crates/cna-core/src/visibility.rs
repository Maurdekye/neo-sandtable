//! Who may see what.
//!
//! Every event and every projected fact carries an [`Audience`]. Projections for a
//! [`Perspective`] keep only what that perspective may see, so filtering is decided once, by the
//! ruleset that produced the fact, and never re-derived by the server or the board.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ids::{IdError, SeatId, Side};

/// The set of viewers allowed to see a fact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "to", content = "who")]
pub enum Audience {
    /// Everyone, both sides.
    Public,
    /// Every seat of one side (and the operator).
    Side(Side),
    /// One seat only (and the operator).
    Seat(SeatId),
    /// Only the omniscient operator view (adjudication internals, secret plots before reveal).
    Operator,
}

/// A viewpoint a stream or observation is projected for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Perspective {
    /// The omniscient view. Must always be labelled as such in any UI.
    Operator,
    Side(Side),
    Seat(SeatId),
}

impl Perspective {
    /// Whether this perspective may see a fact addressed to `audience`.
    pub fn can_see(self, audience: &Audience) -> bool {
        match (self, audience) {
            (Perspective::Operator, _) => true,
            (_, Audience::Public) => true,
            (_, Audience::Operator) => false,
            (Perspective::Side(s), Audience::Side(a)) => s == *a,
            (Perspective::Side(s), Audience::Seat(seat)) => s == seat.side,
            (Perspective::Seat(me), Audience::Side(a)) => me.side == *a,
            (Perspective::Seat(me), Audience::Seat(seat)) => me == *seat,
        }
    }

    /// The side this perspective belongs to, if any.
    pub fn side(self) -> Option<Side> {
        match self {
            Perspective::Operator => None,
            Perspective::Side(s) => Some(s),
            Perspective::Seat(seat) => Some(seat.side),
        }
    }

    /// The fixed set of perspectives a campaign maintains streams for: the operator, both
    /// sides, and all ten seats.
    pub fn all() -> impl Iterator<Item = Perspective> {
        std::iter::once(Perspective::Operator)
            .chain(Side::ALL.into_iter().map(Perspective::Side))
            .chain(SeatId::all().map(Perspective::Seat))
    }
}

impl fmt::Display for Perspective {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Perspective::Operator => f.write_str("operator"),
            Perspective::Side(s) => write!(f, "side:{s}"),
            Perspective::Seat(seat) => write!(f, "seat:{seat}"),
        }
    }
}

impl FromStr for Perspective {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "operator" {
            return Ok(Perspective::Operator);
        }
        if let Some(side) = s.strip_prefix("side:") {
            return Ok(Perspective::Side(side.parse()?));
        }
        if let Some(seat) = s.strip_prefix("seat:") {
            return Ok(Perspective::Seat(seat.parse()?));
        }
        Err(IdError {
            kind: "perspective",
            value: s.to_owned(),
        })
    }
}

impl Serialize for Perspective {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Perspective {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::Role;

    #[test]
    fn perspectives_filter_audiences() {
        let axis_cmd = SeatId::new(Side::Axis, Role::Commander);
        let axis_log = SeatId::new(Side::Axis, Role::Logistics);
        let cw_cmd = SeatId::new(Side::Commonwealth, Role::Commander);

        let op = Perspective::Operator;
        let axis = Perspective::Side(Side::Axis);
        let me = Perspective::Seat(axis_cmd);

        assert!(op.can_see(&Audience::Operator));
        assert!(!axis.can_see(&Audience::Operator));
        assert!(axis.can_see(&Audience::Seat(axis_log)));
        assert!(!axis.can_see(&Audience::Seat(cw_cmd)));
        assert!(me.can_see(&Audience::Side(Side::Axis)));
        assert!(!me.can_see(&Audience::Seat(axis_log)));
        assert!(!me.can_see(&Audience::Side(Side::Commonwealth)));
        assert!(me.can_see(&Audience::Public));
    }

    #[test]
    fn perspectives_round_trip_as_strings() {
        assert_eq!(Perspective::all().count(), 13);
        for p in Perspective::all() {
            assert_eq!(p.to_string().parse::<Perspective>().unwrap(), p);
        }
        assert_eq!(
            "seat:commonwealth.air".parse::<Perspective>().unwrap(),
            Perspective::Seat(SeatId::new(Side::Commonwealth, Role::Air))
        );
        assert!("side:italy".parse::<Perspective>().is_err());
    }
}
