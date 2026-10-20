//! Explicit trusted calibration prices. No defaults, provider lookup or CLI cost ledger.
use crate::calibration::{MODEL, OUTPUT_LIMIT};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationPrices {
    pub model: String,
    pub verified_on: String,
    pub context_tokens: u64,
    pub output_tokens: u64,
    pub max_cost_micro_usd: u64,
    /// Every rate is integer micro-USD per million tokens, not per token.
    pub input_micro_usd_per_mtok: u64,
    pub output_micro_usd_per_mtok: u64,
    pub cache_hit_micro_usd_per_mtok: u64,
    pub cache_write_1h_micro_usd_per_mtok: u64,
    pub high_input_micro_usd_per_mtok: u64,
    pub high_output_micro_usd_per_mtok: u64,
    pub high_cache_write_1h_micro_usd_per_mtok: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationCostProof {
    pub prices: CalibrationPrices,
    pub worst_prompt_micro_usd_per_mtok: u64,
    pub worst_output_micro_usd_per_mtok: u64,
    pub worst_cost_micro_usd: u64,
}

impl CalibrationPrices {
    pub fn validate(&self) -> Result<CalibrationCostProof, &'static str> {
        let date = chrono::NaiveDate::parse_from_str(&self.verified_on, "%Y-%m-%d")
            .map_err(|_| "explicit canonical price verification date required")?;
        let rates = [
            self.input_micro_usd_per_mtok,
            self.cache_hit_micro_usd_per_mtok,
            self.cache_write_1h_micro_usd_per_mtok,
            self.high_input_micro_usd_per_mtok,
            self.high_cache_write_1h_micro_usd_per_mtok,
        ];
        if self.model != MODEL
            || date.format("%Y-%m-%d").to_string() != self.verified_on
            || self.context_tokens != 1_000_000
            || self.output_tokens != OUTPUT_LIMIT
            || self.max_cost_micro_usd == 0
            || self.max_cost_micro_usd > 2_000_000
            || rates.contains(&0)
            || self.output_micro_usd_per_mtok == 0
            || self.high_output_micro_usd_per_mtok == 0
        {
            return Err("missing or unsupported calibration budget inputs");
        }
        // All prompt channels together fit the authoritative model context.
        // Price that entire context at the most expensive input/cache bucket;
        // unknown cache partitioning never gets a cheaper assumed rate.
        let prompt_rate = *rates.iter().max().ok_or("missing input rates")?;
        let output_rate = self
            .output_micro_usd_per_mtok
            .max(self.high_output_micro_usd_per_mtok);
        let numerator = u128::from(self.context_tokens) * u128::from(prompt_rate)
            + u128::from(self.output_tokens) * u128::from(output_rate);
        let cost = u64::try_from(numerator.div_ceil(1_000_000))
            .map_err(|_| "calibration cost overflow")?;
        if cost > self.max_cost_micro_usd {
            return Err("worst-case calibration exceeds explicit cost cap");
        }
        Ok(CalibrationCostProof {
            prices: self.clone(),
            worst_prompt_micro_usd_per_mtok: prompt_rate,
            worst_output_micro_usd_per_mtok: output_rate,
            worst_cost_micro_usd: cost,
        })
    }
}

#[cfg(test)]
pub(crate) fn source_fixture_prices() -> CalibrationPrices {
    // Lead's 2026-10-08 verification, SOURCE/FAKE ONLY. Never a live default/go.
    CalibrationPrices {
        model: MODEL.into(),
        verified_on: "2026-10-08".into(),
        context_tokens: 1_000_000,
        output_tokens: 128_000,
        max_cost_micro_usd: 2_000_000,
        input_micro_usd_per_mtok: 100_000,
        output_micro_usd_per_mtok: 500_000,
        cache_hit_micro_usd_per_mtok: 10_000,
        cache_write_1h_micro_usd_per_mtok: 200_000,
        high_input_micro_usd_per_mtok: 500_000,
        high_output_micro_usd_per_mtok: 2_500_000,
        high_cache_write_1h_micro_usd_per_mtok: 1_000_000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_lead_prices_bound_every_cache_partition_at_132() {
        let prices = source_fixture_prices();
        let proof = prices.validate().unwrap();
        assert_eq!(proof.worst_cost_micro_usd, 1_320_000);
        assert_eq!(proof.prices.verified_on, "2026-10-08");
        assert_eq!(proof.worst_prompt_micro_usd_per_mtok, 1_000_000);
    }
    #[test]
    fn missing_stale_shape_wrong_model_understated_context_and_over_budget_refuse() {
        let mut raw = serde_json::to_value(source_fixture_prices()).unwrap();
        raw.as_object_mut().unwrap().remove("verified_on");
        assert!(serde_json::from_value::<CalibrationPrices>(raw).is_err());
        for field in [
            "model",
            "verified_on",
            "max_cost_micro_usd",
            "context_tokens",
            "high_output_micro_usd_per_mtok",
        ] {
            let mut p = source_fixture_prices();
            match field {
                "model" => p.model = "haiku".into(),
                "verified_on" => p.verified_on.clear(),
                "max_cost_micro_usd" => p.max_cost_micro_usd = 1_319_999,
                "context_tokens" => p.context_tokens = 95_000,
                _ => p.high_output_micro_usd_per_mtok = u64::MAX,
            }
            assert!(p.validate().is_err());
        }
    }
}
