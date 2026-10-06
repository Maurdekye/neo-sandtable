//! Fuel Consumption Chart.

use serde::Deserialize;

use crate::units::FuelTenths;
use crate::{Bound, RawTable, TableError};

#[derive(Debug, Clone, Deserialize)]
struct FuelRaw {
    consumption_rates: Vec<i32>,
    row: Vec<FuelRowRaw>,
}

#[derive(Debug, Clone, Deserialize)]
struct FuelRowRaw {
    cp_expended: i32,
    fuel_tenths: Vec<i32>,
}

/// The Fuel Consumption Chart (`airlog:49.19`): fuel per vehicle by consumption rate and CP.
#[derive(Debug, Clone)]
pub struct FuelConsumption {
    rates: Vec<i32>,
    /// `(cp, fuel tenths per rate column)`, ascending by cp.
    rows: Vec<(i32, Vec<i32>)>,
}

impl Bound for FuelConsumption {
    const ID: &'static str = "airlog.49.19.fuel_consumption";

    fn from_raw(raw: &RawTable) -> Result<Self, TableError> {
        let r: FuelRaw = raw.deserialize()?;
        if r.consumption_rates.is_empty() {
            return Err(raw.err("consumption_rates", "no rate columns"));
        }
        let mut last = 0;
        for (i, row) in r.row.iter().enumerate() {
            if row.cp_expended <= last {
                return Err(raw.err(
                    format!("row[{i}].cp_expended"),
                    "rows must ascend by CP with no repeats",
                ));
            }
            last = row.cp_expended;
            if row.fuel_tenths.len() != r.consumption_rates.len() {
                return Err(raw.err(
                    format!("row[{i}].fuel_tenths"),
                    "needs one value per rate column",
                ));
            }
        }
        if r.row.last().map(|x| x.cp_expended) != Some(50) {
            return Err(raw.err("row", "the last printed row must be 50 CP"));
        }
        Ok(Self {
            rates: r.consumption_rates,
            rows: r
                .row
                .into_iter()
                .map(|x| (x.cp_expended, x.fuel_tenths))
                .collect(),
        })
    }
}

impl FuelConsumption {
    /// The cost printed on the chart for exactly this many CP, or `None` if that CP count or
    /// rate has no printed cell. `airlog:49.19`.
    pub fn printed(&self, rate: i32, cp: i32) -> Option<FuelTenths> {
        let col = self.rates.iter().position(|&r| r == rate)?;
        let row = self.rows.iter().find(|(c, _)| *c == cp)?;
        Some(FuelTenths::new(row.1[col]))
    }

    /// Fuel one vehicle pays for moving `cp` Capability Points at a consumption `rate`.
    ///
    /// `interp:airlog-0001`: 1-4 CP use the printed fractional rows; above that the CP count is
    /// rounded up to the next printed row (multiples of 5); counts over 50 are priced as
    /// whole 50-CP rows plus the remainder. Returns `None` for a rate with no column or a
    /// negative CP. `airlog:49.19`, `airlog:49.13`.
    pub fn fuel_for(&self, rate: i32, cp: i32) -> Option<FuelTenths> {
        if cp < 0 {
            return None;
        }
        let mut remaining = cp;
        let mut total = FuelTenths::ZERO;
        while remaining > 50 {
            total += self.printed(rate, 50)?;
            remaining -= 50;
        }
        if remaining == 0 {
            // Still validates the rate column.
            self.printed(rate, 50)?;
            return Some(total);
        }
        let priced_cp = if remaining < 5 {
            remaining
        } else {
            (remaining + 4) / 5 * 5
        };
        Some(total + self.printed(rate, priced_cp)?)
    }

    /// The rate columns of the chart.
    pub fn rates(&self) -> &[i32] {
        &self.rates
    }
}
