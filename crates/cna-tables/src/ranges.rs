//! Integer ranges, dice readings and the tiling checks every dice-driven table must pass.

use cna_core::dice::TwoDiceReading;
use serde::Deserialize;

/// An inclusive integer range, written `[lo, hi]` in the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Vec<i32>")]
pub struct IntRange {
    pub lo: i32,
    pub hi: i32,
}

impl IntRange {
    pub const fn new(lo: i32, hi: i32) -> Self {
        Self { lo, hi }
    }

    pub fn contains(self, v: i32) -> bool {
        (self.lo..=self.hi).contains(&v)
    }
}

impl TryFrom<Vec<i32>> for IntRange {
    type Error = String;
    fn try_from(v: Vec<i32>) -> Result<Self, String> {
        match v.as_slice() {
            [lo, hi] if lo <= hi => Ok(Self { lo: *lo, hi: *hi }),
            [lo, hi] => Err(format!("range [{lo}, {hi}] is reversed")),
            _ => Err(format!(
                "a range needs exactly two numbers, got {}",
                v.len()
            )),
        }
    }
}

/// True when `v` can be produced by two dice read as tens and units (11-66, digits 1-6 only).
pub fn is_reading(v: i32) -> bool {
    (11..=66).contains(&v) && (1..=6).contains(&(v % 10)) && (1..=6).contains(&(v / 10))
}

/// Every one of the 36 real two-dice readings, in ascending order.
pub fn all_readings() -> impl Iterator<Item = i32> {
    (1..=6).flat_map(|t| (1..=6).map(move |u| t * 10 + u))
}

/// A cell of a 2d6-reading table: the inclusive span of readings giving a result, or `None`
/// when the chart prints a dash (written `[]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Vec<i32>")]
pub struct RollCell(pub Option<IntRange>);

impl TryFrom<Vec<i32>> for RollCell {
    type Error = String;
    fn try_from(v: Vec<i32>) -> Result<Self, String> {
        if v.is_empty() {
            return Ok(Self(None));
        }
        let range = IntRange::try_from(v)?;
        if !is_reading(range.lo) || !is_reading(range.hi) {
            return Err(format!(
                "[{}, {}] must start and end on a real two-dice reading (11-66, digits 1-6)",
                range.lo, range.hi
            ));
        }
        Ok(Self(Some(range)))
    }
}

impl RollCell {
    pub fn contains(self, reading: TwoDiceReading) -> bool {
        self.0
            .is_some_and(|r| r.contains(i32::from(reading.value())))
    }
}

/// Check that `spans` cover each of the 36 real readings exactly once. Numbers between readings
/// (17-20, 27-30, ...) are ignored. Returns a description of the first problem.
pub fn check_reading_tiling(spans: impl IntoIterator<Item = IntRange>) -> Result<(), String> {
    let spans: Vec<IntRange> = spans.into_iter().collect();
    for reading in all_readings() {
        let hits = spans.iter().filter(|s| s.contains(reading)).count();
        match hits {
            1 => {}
            0 => return Err(format!("reading {reading} is covered by no row")),
            n => return Err(format!("reading {reading} is covered by {n} rows")),
        }
    }
    Ok(())
}

/// Check that `spans` tile `lo..=hi` exactly: no gap and no overlap, in any order.
pub fn check_int_tiling(
    spans: impl IntoIterator<Item = IntRange>,
    lo: i32,
    hi: i32,
) -> Result<(), String> {
    let mut spans: Vec<IntRange> = spans.into_iter().collect();
    spans.sort_by_key(|s| s.lo);
    let mut next = lo;
    for s in &spans {
        if s.lo > next {
            return Err(format!("values {next}..{} are covered by no row", s.lo - 1));
        }
        if s.lo < next {
            return Err(format!("value {} is covered by two rows", s.lo));
        }
        next = s.hi + 1;
    }
    if next <= hi {
        return Err(format!("values {next}..{hi} are covered by no row"));
    }
    if next > hi + 1 {
        return Err(format!("rows run past the end of the range at {hi}"));
    }
    Ok(())
}

/// A band of a quantity: `min` up to `max`, or open-ended upward when `max` is absent
/// (`{ min = 46 }` is "46 +").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Band {
    pub min: i32,
    #[serde(default)]
    pub max: Option<i32>,
}

impl Band {
    pub fn contains(self, v: i32) -> bool {
        v >= self.min && self.max.is_none_or(|m| v <= m)
    }
}

/// Check that bands are ordered, start at `first_min`, leave no gap, and end open-ended.
pub fn check_bands(bands: &[Band], first_min: i32) -> Result<(), String> {
    let mut next = first_min;
    for (i, b) in bands.iter().enumerate() {
        if b.min != next {
            return Err(format!(
                "band {} starts at {} but the previous band ends just before {next}",
                i + 1,
                b.min
            ));
        }
        match b.max {
            Some(m) if m >= b.min => next = m + 1,
            Some(m) => return Err(format!("band {} has max {m} below min {}", i + 1, b.min)),
            None if i + 1 == bands.len() => return Ok(()),
            None => return Err(format!("band {} is open-ended but is not the last", i + 1)),
        }
    }
    Err("the last band must be open-ended".to_string())
}

/// The index of the band holding `v`, if any.
pub fn band_index(bands: &[Band], v: i32) -> Option<usize> {
    bands.iter().position(|b| b.contains(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_are_digits_one_to_six() {
        assert!(is_reading(11) && is_reading(66) && is_reading(35));
        assert!(!is_reading(17) && !is_reading(10) && !is_reading(67) && !is_reading(70));
        assert_eq!(all_readings().count(), 36);
    }

    #[test]
    fn tiling_detects_gap_and_overlap() {
        let ok = [IntRange::new(11, 35), IntRange::new(36, 66)];
        assert!(check_reading_tiling(ok).is_ok());
        let gap = [IntRange::new(11, 35), IntRange::new(41, 66)];
        assert!(check_reading_tiling(gap).unwrap_err().contains("36"));
        let overlap = [IntRange::new(11, 40), IntRange::new(36, 66)];
        assert!(
            check_reading_tiling(overlap)
                .unwrap_err()
                .contains("covered by 2")
        );
    }

    #[test]
    fn int_tiling_detects_gap_and_overlap() {
        assert!(check_int_tiling([IntRange::new(2, 4), IntRange::new(5, 12)], 2, 12).is_ok());
        assert!(check_int_tiling([IntRange::new(2, 4), IntRange::new(6, 12)], 2, 12).is_err());
        assert!(check_int_tiling([IntRange::new(2, 5), IntRange::new(5, 12)], 2, 12).is_err());
        assert!(check_int_tiling([IntRange::new(2, 11)], 2, 12).is_err());
    }

    #[test]
    fn bands_must_be_contiguous_and_end_open() {
        let b = |min, max| Band { min, max };
        assert!(check_bands(&[b(1, Some(5)), b(6, Some(10)), b(11, None)], 1).is_ok());
        assert!(check_bands(&[b(1, Some(5)), b(7, None)], 1).is_err());
        assert!(check_bands(&[b(1, Some(5)), b(6, Some(10))], 1).is_err());
        assert!(check_bands(&[b(1, None), b(6, None)], 1).is_err());
    }
}
