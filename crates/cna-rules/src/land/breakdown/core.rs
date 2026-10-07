//! Carrier-neutral arithmetic; no state, random generator or unit identity is accessed.
use super::{Category, overflow};
use cna_core::{dice::TwoDiceReading, engine::EngineError};
use cna_protocol::Side;
use cna_tables::land::{breakdown::BreakdownTable, weather::WeatherKind};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Exposure {
    pub accumulated_quarters: i32,
    pub light_extra_quarters: i32,
    pub travel_cp_quarters: i32,
    pub sandstorm_cp_quarters: i32,
}
/// The map pricing procedure supplies the actual terrain/edge BP and travelled CP, including rain.
/// Light-truck extra is separate and never applies to the body's or medium/heavy exposure.
/// Cases: land:21.21, land:21.22, land:21.25, land:21.29, land:21.37, airlog:54.2
/// Interpretations: interp:land-0027, interp:land-0029
pub fn accrue_step(
    old: Exposure,
    bp_quarters: i32,
    light_extra_quarters: i32,
    cp_quarters: i32,
    weather: WeatherKind,
) -> Result<Exposure, EngineError> {
    if bp_quarters < 0
        || light_extra_quarters < 0
        || cp_quarters <= 0
        || [
            old.accumulated_quarters,
            old.light_extra_quarters,
            old.travel_cp_quarters,
            old.sandstorm_cp_quarters,
        ]
        .iter()
        .any(|n| *n < 0)
        || old.sandstorm_cp_quarters > old.travel_cp_quarters
    {
        return Err(overflow());
    }
    Ok(Exposure {
        accumulated_quarters: old
            .accumulated_quarters
            .checked_add(bp_quarters)
            .ok_or_else(overflow)?,
        light_extra_quarters: old
            .light_extra_quarters
            .checked_add(light_extra_quarters)
            .ok_or_else(overflow)?,
        travel_cp_quarters: old
            .travel_cp_quarters
            .checked_add(cp_quarters)
            .ok_or_else(overflow)?,
        sandstorm_cp_quarters: old
            .sandstorm_cp_quarters
            .checked_add(if weather == WeatherKind::Sandstorm {
                cp_quarters
            } else {
                0
            })
            .ok_or_else(overflow)?,
    })
}
/// CP exposure defines the sandstorm threshold; hot and sandstorm shifts accumulate.
/// Cases: land:21.37, land:29.33, land:29.45
/// Interpretations: interp:land-0027
pub fn weather_shift(travel_cp_quarters: i32, sandstorm_cp_quarters: i32, hot: bool) -> i32 {
    i32::from(hot)
        + i32::from(
            travel_cp_quarters > 0
                && i64::from(sandstorm_cp_quarters) * 2 >= i64::from(travel_cp_quarters),
        )
}
#[derive(Debug, Clone)]
pub struct VehicleInput<T> {
    pub identity: T,
    pub side: Side,
    pub category: Category,
    pub points: i32,
    pub bar: i32,
    pub bp_quarters: i32,
    pub checked_column: Option<usize>,
    pub weather_shift: i32,
}
#[derive(Debug, Clone)]
pub struct VehicleGroup<T> {
    pub side: Side,
    pub category: Category,
    pub bar: i32,
    pub column: usize,
    pub shift: i32,
    pub assets: Vec<VehicleInput<T>>,
}
/// Physical cohorts with different category, BAR, base BP band or weather use distinct checks.
/// The unadjusted band controls a repeated check, including a previous shifted zero-loss result.
/// Cases: land:21.26, land:21.27, land:21.28, land:21.29, land:21.31, land:21.32
pub fn check_groups<T>(
    table: &BreakdownTable,
    inputs: impl IntoIterator<Item = VehicleInput<T>>,
) -> Result<Vec<VehicleGroup<T>>, EngineError> {
    let mut grouped: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for a in inputs {
        if a.points < 0 {
            return Err(overflow());
        }
        let column = table.column_quarters(a.bp_quarters).ok_or_else(overflow)?;
        if a.points == 0 || a.bp_quarters <= 12 || a.checked_column.is_some_and(|old| column <= old)
        {
            continue;
        }
        let shift = a.bar.checked_add(a.weather_shift).ok_or_else(overflow)?;
        grouped
            .entry((a.side, a.category, a.bar, column, shift))
            .or_default()
            .push(a);
    }
    Ok(grouped
        .into_iter()
        .map(
            |((side, category, bar, column, shift), assets)| VehicleGroup {
                side,
                category,
                bar,
                column,
                shift,
                assets,
            },
        )
        .collect())
}
/// Apply the already drawn ordered reading and conserve the integer loss count.
/// Cases: land:21.31, land:21.32, land:21.33, land:21.34, land:21.35, land:21.38
pub fn losses(
    table: &BreakdownTable,
    bp_quarters: i32,
    shift: i32,
    points: &[i32],
    reading: TwoDiceReading,
) -> Result<(i32, i32), EngineError> {
    if points.iter().any(|n| *n < 0) {
        return Err(overflow());
    }
    let total = points
        .iter()
        .try_fold(0i32, |sum, n| sum.checked_add(*n))
        .ok_or_else(overflow)?;
    let percent = table
        .percent_quarters(bp_quarters, shift, reading)
        .ok_or_else(overflow)?;
    let broken = table.broken_points(total, percent).ok_or_else(overflow)?;
    Ok((percent, broken))
}
pub use super::allocation::{CarrierAsset, balanced_carrier_allocation, valid_carrier_allocation};
pub use super::{proportional_allocation, valid_allocation};
#[cfg(test)]
mod tests {
    use super::*;
    /// Cases: land:21.21, land:21.29, land:21.37
    #[test]
    fn travelled_cp_and_type_extra_remain_distinct_and_checked() {
        let a = accrue_step(Exposure::default(), 4, 4, 12, WeatherKind::Normal).unwrap();
        let b = accrue_step(a, 8, 8, 12, WeatherKind::Sandstorm).unwrap();
        assert_eq!(
            b,
            Exposure {
                accumulated_quarters: 12,
                light_extra_quarters: 12,
                travel_cp_quarters: 24,
                sandstorm_cp_quarters: 12
            }
        );
        assert_eq!(
            weather_shift(b.travel_cp_quarters, b.sandstorm_cp_quarters, true),
            2
        );
        assert_eq!(weather_shift(25, 12, true), 1);
        assert!(
            accrue_step(
                Exposure {
                    accumulated_quarters: i32::MAX,
                    ..Default::default()
                },
                1,
                0,
                1,
                WeatherKind::Normal
            )
            .is_err()
        );
    }
    /// Paraphrased21.34 example: the same35BP gives trucks3 losses and tanks7 with their distinct shifts/readings.
    /// Cases: land:21.26, land:21.28, land:21.34, land:21.35
    #[test]
    fn pool_strings_and_unit_ids_share_exact_chart_arithmetic_without_state() {
        let c = crate::CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let input = |identity, bar| VehicleInput {
            identity,
            side: Side::Axis,
            category: Category::Truck,
            points: 30,
            bar,
            bp_quarters: 140,
            checked_column: None,
            weather_shift: 1,
        };
        let groups = check_groups(
            &c.tables.land.breakdown,
            [input("physical-pool-cohort".to_string(), -2)],
        )
        .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].column, 4);
        assert_eq!(groups[0].shift, -1);
        assert_eq!(
            losses(
                &c.tables.land.breakdown,
                140,
                -1,
                &[30],
                TwoDiceReading {
                    tens: cna_core::dice::Die::new(3).unwrap(),
                    units: cna_core::dice::Die::new(3).unwrap()
                }
            )
            .unwrap(),
            (10, 3)
        );
        assert_eq!(
            losses(
                &c.tables.land.breakdown,
                140,
                2,
                &[20],
                TwoDiceReading {
                    tens: cna_core::dice::Die::new(6).unwrap(),
                    units: cna_core::dice::Die::new(1).unwrap()
                }
            )
            .unwrap(),
            (33, 7)
        );
        let mut old = input("physical-pool-cohort".to_string(), -2);
        old.checked_column = Some(4);
        assert!(
            check_groups(&c.tables.land.breakdown, [old])
                .unwrap()
                .is_empty()
        );
        assert_eq!(proportional_allocation(&[20, 10], 3), Some(vec![2, 1]));
    }
}
