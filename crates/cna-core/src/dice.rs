//! Campaign randomness and the game's dice.
//!
//! All randomness in a campaign comes from one [`CampaignRng`], whose complete state is
//! serializable so it can be stored in checkpoints and replayed exactly. The generator is
//! versioned: changing the algorithm or the way dice are drawn from it requires a new
//! [`RngAlgorithm`] variant, never a silent change to an existing one.

use rand_chacha::ChaCha8Rng;
use rand_core::{RngCore, SeedableRng};
use serde::{Deserialize, Serialize};

/// Identifies the random algorithm and draw semantics pinned by a campaign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RngAlgorithm {
    /// ChaCha8, one `u32` per draw, rejection sampling for unbiased die faces.
    #[serde(rename = "rng-v1")]
    ChaCha8V1,
}

/// The serializable state of a [`CampaignRng`]: enough to resume the exact sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RngState {
    pub algorithm: RngAlgorithm,
    pub seed: [u8; 32],
    pub stream: u64,
    /// Position in the keystream, in 32-bit words.
    pub word_pos: u128,
}

/// The single source of randomness for a campaign.
#[derive(Debug, Clone)]
pub struct CampaignRng {
    inner: ChaCha8Rng,
}

impl CampaignRng {
    /// A fresh generator for a new campaign.
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            inner: ChaCha8Rng::from_seed(seed),
        }
    }

    /// Restore a generator from a checkpointed state.
    pub fn from_state(state: &RngState) -> Self {
        match state.algorithm {
            RngAlgorithm::ChaCha8V1 => {
                let mut inner = ChaCha8Rng::from_seed(state.seed);
                inner.set_stream(state.stream);
                inner.set_word_pos(state.word_pos);
                Self { inner }
            }
        }
    }

    /// The state to store in a checkpoint.
    pub fn state(&self) -> RngState {
        RngState {
            algorithm: RngAlgorithm::ChaCha8V1,
            seed: self.inner.get_seed(),
            stream: self.inner.get_stream(),
            word_pos: self.inner.get_word_pos(),
        }
    }

    /// Roll one six-sided die: a value from 1 to 6.
    pub fn d6(&mut self) -> Die {
        // u32::MAX % 6 == 3, so values below `ZONE` are an exact multiple of 6 and map
        // uniformly onto the six faces; the rest are rejected and redrawn.
        const ZONE: u32 = u32::MAX - (u32::MAX % 6);
        loop {
            let x = self.inner.next_u32();
            if x < ZONE {
                return Die((x % 6) as u8 + 1);
            }
        }
    }

    /// Roll two dice and read them as tens and units (11–66), as many of the game's tables do.
    /// The first die rolled is the tens digit.
    pub fn two_dice_reading(&mut self) -> TwoDiceReading {
        let tens = self.d6();
        let units = self.d6();
        TwoDiceReading { tens, units }
    }

    /// Roll `n` dice and add them.
    pub fn sum_dice(&mut self, n: u32) -> u32 {
        (0..n).map(|_| u32::from(self.d6().value())).sum()
    }
}

/// The face of one six-sided die.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Die(u8);

impl Die {
    /// A die face from a known value, e.g. when replaying a recorded roll.
    pub fn new(value: u8) -> Option<Self> {
        (1..=6).contains(&value).then_some(Self(value))
    }

    pub fn value(self) -> u8 {
        self.0
    }
}

/// Two dice read as a two-digit number, tens first (11–66).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TwoDiceReading {
    pub tens: Die,
    pub units: Die,
}

impl TwoDiceReading {
    /// The reading as a number, e.g. 34 for a 3 and a 4.
    pub fn value(self) -> u8 {
        self.tens.value() * 10 + self.units.value()
    }

    /// The plain sum of both dice (2–12), for rules that add them instead.
    pub fn sum(self) -> u8 {
        self.tens.value() + self.units.value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    #[test]
    fn same_seed_same_rolls() {
        let mut a = CampaignRng::from_seed(seed(7));
        let mut b = CampaignRng::from_seed(seed(7));
        for _ in 0..1000 {
            assert_eq!(a.d6(), b.d6());
        }
    }

    #[test]
    fn restored_state_continues_the_same_sequence() {
        let mut rng = CampaignRng::from_seed(seed(42));
        for _ in 0..137 {
            rng.d6();
        }
        let checkpoint = rng.state();
        let expected: Vec<Die> = (0..500).map(|_| rng.d6()).collect();

        let mut restored = CampaignRng::from_state(&checkpoint);
        let replayed: Vec<Die> = (0..500).map(|_| restored.d6()).collect();
        assert_eq!(expected, replayed);
    }

    #[test]
    fn faces_stay_in_range_and_all_appear() {
        let mut rng = CampaignRng::from_seed(seed(1));
        let mut counts = [0u32; 6];
        for _ in 0..60_000 {
            let face = rng.d6().value();
            assert!((1..=6).contains(&face));
            counts[usize::from(face - 1)] += 1;
        }
        // Each face should appear roughly 10,000 times; a loose bound catches gross bias.
        for count in counts {
            assert!((9_000..=11_000).contains(&count), "counts: {counts:?}");
        }
    }

    #[test]
    fn two_dice_reading_is_tens_then_units() {
        let reading = TwoDiceReading {
            tens: Die::new(3).unwrap(),
            units: Die::new(4).unwrap(),
        };
        assert_eq!(reading.value(), 34);
        assert_eq!(reading.sum(), 7);
    }

    #[test]
    fn die_rejects_out_of_range_values() {
        assert!(Die::new(0).is_none());
        assert!(Die::new(7).is_none());
        assert_eq!(Die::new(6).map(Die::value), Some(6));
    }
}
