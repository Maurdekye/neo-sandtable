//! Quantities with units in their types.
//!
//! Every count the rules keep (capability points, fuel, ammunition, …) gets its own newtype, so
//! adding fuel to ammunition does not compile. All arithmetic is integer; rounding happens only
//! through explicitly named functions that cite the rule defining them.

use std::fmt;
use std::iter::Sum;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

macro_rules! quantity {
    ($(#[$doc:meta])* $name:ident, $unit:literal) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub i32);

        impl $name {
            pub const ZERO: Self = Self(0);
            pub const UNIT: &'static str = $unit;

            pub const fn new(value: i32) -> Self {
                Self(value)
            }

            pub const fn get(self) -> i32 {
                self.0
            }

            pub fn is_zero(self) -> bool {
                self.0 == 0
            }

            /// `self - rhs`, or `None` if the result would be negative.
            pub fn checked_sub(self, rhs: Self) -> Option<Self> {
                let v = self.0.checked_sub(rhs.0)?;
                (v >= 0).then_some(Self(v))
            }

            /// `self - rhs`, floored at zero.
            pub fn saturating_sub(self, rhs: Self) -> Self {
                Self((self.0 - rhs.0).max(0))
            }
        }

        impl Add for $name {
            type Output = Self;
            fn add(self, rhs: Self) -> Self {
                Self(self.0 + rhs.0)
            }
        }

        impl Sub for $name {
            type Output = Self;
            fn sub(self, rhs: Self) -> Self {
                Self(self.0 - rhs.0)
            }
        }

        impl Neg for $name {
            type Output = Self;
            fn neg(self) -> Self {
                Self(-self.0)
            }
        }

        impl AddAssign for $name {
            fn add_assign(&mut self, rhs: Self) {
                self.0 += rhs.0;
            }
        }

        impl SubAssign for $name {
            fn sub_assign(&mut self, rhs: Self) {
                self.0 -= rhs.0;
            }
        }

        impl Sum for $name {
            fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
                Self(iter.map(|q| q.0).sum())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{} {}", self.0, $unit)
            }
        }
    };
}

quantity!(
    /// Capability points (`land:6`).
    CapabilityPoints,
    "CP"
);
quantity!(
    /// Fuel points (`airlog:49`).
    FuelPoints,
    "fuel"
);
quantity!(
    /// Ammunition points (`airlog:50`).
    AmmoPoints,
    "ammo"
);
quantity!(
    /// Stores points (`airlog:51`).
    StoresPoints,
    "stores"
);
quantity!(
    /// Water points (`airlog:52`).
    WaterPoints,
    "water"
);
quantity!(
    /// TOE strength points (`land:3.5`).
    ToeStrengthPoints,
    "TOE"
);
quantity!(
    /// Stacking points (`land:9`).
    StackingPoints,
    "SP"
);
quantity!(
    /// Truck points (`airlog:53`).
    TruckPoints,
    "truck"
);
quantity!(
    /// Motorization points (`land:32.5`, `scen:59.63`).
    MotorizationPoints,
    "MP"
);
quantity!(
    /// Replacement points (`land:20.2`).
    ReplacementPoints,
    "RP"
);
quantity!(
    /// Shipping or cargo tonnage (`airlog:56`).
    Tons,
    "tons"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_stays_within_a_unit() {
        let a = FuelPoints::new(7) + FuelPoints::new(5);
        assert_eq!(a, FuelPoints::new(12));
        assert_eq!(a.checked_sub(FuelPoints::new(13)), None);
        assert_eq!(a.saturating_sub(FuelPoints::new(13)), FuelPoints::ZERO);
        let total: AmmoPoints = [1, 2, 3].into_iter().map(AmmoPoints::new).sum();
        assert_eq!(total, AmmoPoints::new(6));
        assert_eq!(CapabilityPoints::new(3).to_string(), "3 CP");
    }
}
