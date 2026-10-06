//! Calendar months as the tables name them (`jan` .. `dec`).

use serde::Deserialize;

/// A calendar month, written as its three-letter lowercase abbreviation in the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Month {
    Jan,
    Feb,
    Mar,
    Apr,
    May,
    Jun,
    Jul,
    Aug,
    Sep,
    Oct,
    Nov,
    Dec,
}

impl Month {
    pub const ALL: [Month; 12] = [
        Month::Jan,
        Month::Feb,
        Month::Mar,
        Month::Apr,
        Month::May,
        Month::Jun,
        Month::Jul,
        Month::Aug,
        Month::Sep,
        Month::Oct,
        Month::Nov,
        Month::Dec,
    ];

    /// The month for a number 1-12.
    pub fn from_number(n: u32) -> Option<Self> {
        Self::ALL.get((n as usize).checked_sub(1)?).copied()
    }

    /// The month number 1-12.
    pub fn number(self) -> u32 {
        Self::ALL.iter().position(|m| *m == self).expect("listed") as u32 + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_round_trip() {
        for n in 1..=12 {
            assert_eq!(Month::from_number(n).map(Month::number), Some(n));
        }
        assert_eq!(Month::from_number(0), None);
        assert_eq!(Month::from_number(13), None);
    }
}
