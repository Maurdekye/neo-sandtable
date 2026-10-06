//! Hex geometry on one continuous axial grid covering all five map sections.
//!
//! The map uses pointy-top hexes; axial `q` increases eastward and `r` increases toward the
//! south-east. Converting printed ids (`C4218`) to axial coordinates is the job of the map
//! content (`data/map`), not of this module.

use serde::{Deserialize, Serialize};

/// Axial hex coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Axial {
    pub q: i32,
    pub r: i32,
}

/// The six neighbour directions of a pointy-top hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    East,
    NorthEast,
    NorthWest,
    West,
    SouthWest,
    SouthEast,
}

impl Direction {
    pub const ALL: [Direction; 6] = [
        Direction::East,
        Direction::NorthEast,
        Direction::NorthWest,
        Direction::West,
        Direction::SouthWest,
        Direction::SouthEast,
    ];

    /// The axial offset one step in this direction.
    pub const fn offset(self) -> Axial {
        match self {
            Direction::East => Axial { q: 1, r: 0 },
            Direction::NorthEast => Axial { q: 1, r: -1 },
            Direction::NorthWest => Axial { q: 0, r: -1 },
            Direction::West => Axial { q: -1, r: 0 },
            Direction::SouthWest => Axial { q: -1, r: 1 },
            Direction::SouthEast => Axial { q: 0, r: 1 },
        }
    }

    pub const fn opposite(self) -> Direction {
        match self {
            Direction::East => Direction::West,
            Direction::NorthEast => Direction::SouthWest,
            Direction::NorthWest => Direction::SouthEast,
            Direction::West => Direction::East,
            Direction::SouthWest => Direction::NorthEast,
            Direction::SouthEast => Direction::NorthWest,
        }
    }
}

impl Axial {
    pub const fn new(q: i32, r: i32) -> Self {
        Self { q, r }
    }

    /// The cube coordinate `s = -q - r`.
    pub const fn s(self) -> i32 {
        -self.q - self.r
    }

    pub fn neighbor(self, dir: Direction) -> Axial {
        let d = dir.offset();
        Axial::new(self.q + d.q, self.r + d.r)
    }

    pub fn neighbors(self) -> [Axial; 6] {
        Direction::ALL.map(|d| self.neighbor(d))
    }

    /// The number of hex steps between two hexes, ignoring terrain.
    pub fn distance(self, other: Axial) -> u32 {
        let dq = (self.q - other.q).unsigned_abs();
        let dr = (self.r - other.r).unsigned_abs();
        let ds = (self.s() - other.s()).unsigned_abs();
        dq.max(dr).max(ds)
    }

    /// The direction of an adjacent hex, if `other` is adjacent.
    pub fn direction_to(self, other: Axial) -> Option<Direction> {
        Direction::ALL
            .into_iter()
            .find(|&d| self.neighbor(d) == other)
    }

    /// Every hex within `radius` steps, including `self`, in a stable order.
    pub fn within(self, radius: u32) -> Vec<Axial> {
        let n = radius as i32;
        let mut out = Vec::new();
        for dq in -n..=n {
            let lo = (-n).max(-dq - n);
            let hi = n.min(-dq + n);
            for dr in lo..=hi {
                out.push(Axial::new(self.q + dq, self.r + dr));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbors_are_at_distance_one_and_opposites_cancel() {
        let h = Axial::new(3, -2);
        for d in Direction::ALL {
            let n = h.neighbor(d);
            assert_eq!(h.distance(n), 1);
            assert_eq!(n.neighbor(d.opposite()), h);
            assert_eq!(h.direction_to(n), Some(d));
        }
    }

    #[test]
    fn distance_matches_cube_metric() {
        assert_eq!(Axial::new(0, 0).distance(Axial::new(3, -1)), 3);
        assert_eq!(Axial::new(0, 0).distance(Axial::new(-2, 4)), 4);
        assert_eq!(Axial::new(1, 1).distance(Axial::new(1, 1)), 0);
    }

    #[test]
    fn within_counts_hexes() {
        // 1 + 3r(r+1) hexes within radius r.
        for r in 0..5u32 {
            assert_eq!(Axial::new(2, 2).within(r).len() as u32, 1 + 3 * r * (r + 1));
        }
        assert!(
            Axial::new(2, 2)
                .within(2)
                .iter()
                .all(|h| h.distance(Axial::new(2, 2)) <= 2)
        );
    }
}
