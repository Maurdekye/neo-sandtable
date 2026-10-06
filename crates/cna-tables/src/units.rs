//! Quantities local to the table bindings.
//!
//! `FuelTenths` exists because airlog-0001 tracks fuel in tenths of a Fuel Point; it is proposed
//! for `cna-core::quantity` and lives here until the lead decides.

use std::fmt;
use std::ops::{Add, AddAssign, Mul};

use cna_core::quantity::FuelPoints;

/// Fuel in tenths of a Fuel Point (`airlog:49.19`, `interp:airlog-0001`): 12 = 1.2 Fuel Points.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FuelTenths(pub i32);

impl FuelTenths {
    pub const ZERO: Self = Self(0);

    pub const fn new(tenths: i32) -> Self {
        Self(tenths)
    }

    pub const fn tenths(self) -> i32 {
        self.0
    }

    /// Whole Fuel Points, rounded up (`interp:airlog-0001`: the total drawn from a source is
    /// rounded up when the fuel is taken).
    pub fn ceil_points(self) -> FuelPoints {
        FuelPoints::new((self.0 + 9).div_euclid(10))
    }
}

impl Add for FuelTenths {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for FuelTenths {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl Mul<i32> for FuelTenths {
    type Output = Self;
    fn mul(self, rhs: i32) -> Self {
        Self(self.0 * rhs)
    }
}

impl fmt::Display for FuelTenths {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{} fuel", self.0 / 10, self.0 % 10)
    }
}

/// An exact fraction, for the few tables that print one (`airlog:54.5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
pub struct Ratio {
    pub num: i32,
    pub den: i32,
}

impl Ratio {
    /// `self * n`, as an exact ratio (not reduced).
    pub fn times(self, n: i32) -> Self {
        Self {
            num: self.num * n,
            den: self.den,
        }
    }

    /// The value rounded up to a whole number.
    pub fn ceil(self) -> i32 {
        (self.num + self.den - 1).div_euclid(self.den)
    }

    /// The value rounded down to a whole number.
    pub fn floor(self) -> i32 {
        self.num.div_euclid(self.den)
    }
}
