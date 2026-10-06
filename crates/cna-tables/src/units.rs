//! Quantities used by the table bindings.

/// Shared exact fuel quantity (airlog:49.19; interp:airlog-0001).
pub use cna_core::quantity::FuelTenths;

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
