//! Actual convoy identities retain their own BP, physical cohorts and completed stops.
//! Unit maps, unit loss plans and their checkpoint representation remain independent.
use super::{Category, Equipment, Motion, cohorts::History, core, overflow};
use crate::{
    CnaContent, State,
    logistics::{self, FuelTruckKind, TruckFuelCohort},
};
use cna_core::{engine::EngineError, ids::HexId};
use cna_protocol::Side;
use cna_tables::{airlog::trucks::TruckType, land::weather::WeatherKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PoolBreakdown {
    pub accumulated_quarters: i32,
    pub light_extra_quarters: i32,
    pub truck_histories: BTreeMap<String, History>,
    pub moving: Option<Motion>,
    pub stopped: VecDeque<PoolStop>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStop {
    pub motion: Motion,
    pub destination: HexId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolAsset {
    pub pool: String,
    pub equipment: Equipment,
    pub points: i32,
    pub cohort: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolCheckGroup {
    pub side: Side,
    pub bar: i32,
    pub column: usize,
    pub shift: i32,
    pub assets: Vec<PoolAsset>,
}
fn carrier<'a>(s: &'a State, id: &str) -> Result<&'a crate::state::TruckPool, EngineError> {
    let mut matches = s.logistics.truck_pools.iter().filter(|p| p.id == id);
    let p = matches.next().ok_or_else(overflow)?;
    if id.is_empty() || matches.next().is_some() {
        return Err(overflow());
    }
    Ok(p)
}
fn histories(
    s: &State,
    id: &str,
) -> Result<(PoolBreakdown, Vec<TruckFuelCohort<String>>), EngineError> {
    carrier(s, id)?;
    let groups = logistics::pool_fuel::pool_segment_fuel_cohorts(s, id).map_err(|_| overflow())?;
    let mut b = s.land.breakdown.pools.get(id).cloned().unwrap_or_default();
    for g in &groups {
        if b.truck_histories.contains_key(&g.id) {
            continue;
        }
        let prior = s
            .land
            .breakdown
            .pools
            .values()
            .find_map(|b| b.truck_histories.get(&g.id))
            .cloned()
            .or_else(|| {
                g.parent.as_ref().and_then(|p| {
                    s.land
                        .breakdown
                        .pools
                        .values()
                        .find_map(|b| b.truck_histories.get(p))
                        .cloned()
                })
            });
        b.truck_histories.insert(
            g.id.clone(),
            prior.unwrap_or(History {
                base_quarters: b.accumulated_quarters,
                light_extra_quarters: b.light_extra_quarters,
                checked: None,
            }),
        );
    }
    Ok((b, groups))
}
/// Snapshot origin placement truth at the accepted move; unknown crossing data is deferred.
/// Cases: land:21.41, airlog:53.12
pub fn begin_motion(
    c: &CnaContent,
    s: &mut State,
    id: &str,
    origin: &HexId,
    strict: bool,
) -> Result<(), EngineError> {
    let p = carrier(s, id)?;
    if s.land
        .breakdown
        .pools
        .get(id)
        .is_some_and(|b| b.moving.is_some())
    {
        return Ok(());
    }
    let side = p.side;
    let trucks = p.trucks;
    let result = super::origin_condition(c, s, side, origin, strict, |from, to| {
        let rain = crate::logistics::weather::at_hex(c, s, to)? == WeatherKind::Rainstorm;
        match super::super::map::truck_step_cost(c, side, &trucks, from, to, strict, rain) {
            Ok(_) => Ok(true),
            Err(cna_core::engine::Rejection::Engine(e)) => Err(e),
            Err(_) => Ok(false),
        }
    });
    s.land.breakdown.pools.entry(id.into()).or_default().moving = Some(Motion {
        origin: origin.clone(),
        travel_cp_quarters: 0,
        sandstorm_cp_quarters: 0,
        origin_required: result.as_ref().copied().unwrap_or(false),
        origin_gap: result.err(),
    });
    Ok(())
}
/// Accrue the same source-priced BP and weather exposure for genuine pool trucks.
/// The caller supplies an accepted edge; no dice or hidden eligibility is evaluated here.
/// Cases: land:21.21, land:21.22, land:21.25, land:21.29, land:21.37
pub fn record_edge(
    s: &mut State,
    id: &str,
    from: &HexId,
    bp_quarters: i32,
    cp_quarters: i32,
    weather: WeatherKind,
) -> Result<(), EngineError> {
    let (mut b, groups) = histories(s, id)?;
    let mut motion = b.moving.take().unwrap_or(Motion {
        origin: from.clone(),
        travel_cp_quarters: 0,
        sandstorm_cp_quarters: 0,
        origin_required: false,
        origin_gap: None,
    });
    let next = core::accrue_step(
        core::Exposure {
            accumulated_quarters: b.accumulated_quarters,
            light_extra_quarters: b.light_extra_quarters,
            travel_cp_quarters: motion.travel_cp_quarters,
            sandstorm_cp_quarters: motion.sandstorm_cp_quarters,
        },
        bp_quarters,
        0,
        cp_quarters,
        weather,
    )?;
    for g in groups {
        let h = b.truck_histories.get_mut(&g.id).ok_or_else(overflow)?;
        h.base_quarters = h
            .base_quarters
            .checked_add(bp_quarters)
            .ok_or_else(overflow)?;
    }
    b.accumulated_quarters = next.accumulated_quarters;
    motion.travel_cp_quarters = next.travel_cp_quarters;
    motion.sandstorm_cp_quarters = next.sandstorm_cp_quarters;
    b.moving = Some(motion);
    s.land.breakdown.pools.insert(id.into(), b);
    Ok(())
}
/// Light-only additions preserve other physical cohorts' original BP bands.
/// Cases: airlog:54.2, land:21.29
pub fn record_light_extra(s: &mut State, id: &str, quarters: i32) -> Result<(), EngineError> {
    if quarters < 0 {
        return Err(overflow());
    }
    let (mut b, groups) = histories(s, id)?;
    b.light_extra_quarters = b
        .light_extra_quarters
        .checked_add(quarters)
        .ok_or_else(overflow)?;
    for g in groups
        .into_iter()
        .filter(|g| g.kind == FuelTruckKind::Light)
    {
        let h = b.truck_histories.get_mut(&g.id).ok_or_else(overflow)?;
        h.light_extra_quarters = h
            .light_extra_quarters
            .checked_add(quarters)
            .ok_or_else(overflow)?;
    }
    s.land.breakdown.pools.insert(id.into(), b);
    Ok(())
}
/// Park a completed convoy move without rolling at the answer boundary.
/// Cases: land:21.22, land:21.24, land:21.25, land:21.28
pub fn stop(s: &mut State, id: &str, destination: &HexId) {
    if let Some(b) = s.land.breakdown.pools.get_mut(id)
        && let Some(motion) = b.moving.take()
    {
        b.stopped.push_back(PoolStop {
            motion,
            destination: destination.clone(),
        });
    }
}
/// Build checks from real pool truck characteristics and the exact physical history.
/// Cases: land:21.26, land:21.27, land:21.28, land:21.29, land:21.31, land:21.32, airlog:54.2
pub fn check_groups(
    c: &CnaContent,
    s: &State,
    id: &str,
    stopped: &PoolStop,
) -> Result<Vec<PoolCheckGroup>, EngineError> {
    let side = carrier(s, id)?.side;
    let (b, groups) = histories(s, id)?;
    let hot = s
        .turn
        .weather
        .as_ref()
        .is_some_and(|w| w.kind == WeatherKind::Hot);
    let mut inputs = vec![];
    for g in groups {
        let h = &b.truck_histories[&g.id];
        let (equipment, ty, extra) = match g.kind {
            FuelTruckKind::Light => (
                Equipment::LightTruck,
                TruckType::Light,
                h.light_extra_quarters,
            ),
            FuelTruckKind::Medium => (Equipment::MediumTruck, TruckType::Medium, 0),
            FuelTruckKind::Heavy => (Equipment::HeavyTruck, TruckType::Heavy, 0),
        };
        inputs.push(core::VehicleInput {
            identity: PoolAsset {
                pool: id.into(),
                equipment,
                points: g.count,
                cohort: g.id,
            },
            side,
            category: Category::Truck,
            points: g.count,
            bar: -c
                .tables
                .airlog
                .truck_characteristics
                .truck(ty)
                .bar_shift_left,
            bp_quarters: h.base_quarters.checked_add(extra).ok_or_else(overflow)?,
            checked_column: h.checked,
            weather_shift: super::weather_shift(&stopped.motion, hot),
        });
    }
    Ok(core::check_groups(&c.tables.land.breakdown, inputs)?
        .into_iter()
        .map(|g| PoolCheckGroup {
            side: g.side,
            bar: g.bar,
            column: g.column,
            shift: g.shift,
            assets: g.assets.into_iter().map(|a| a.identity).collect(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cna, state::Location};
    use cna_content::units::Trucks;
    fn setup() -> (CnaContent, State, String) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let mut p = s.logistics.truck_pools[0].clone();
        p.id = "physical-pool-a".into();
        p.side = Side::Commonwealth;
        p.location = Some(Location::Hex {
            hex: "C4020".into(),
        });
        p.trucks = Trucks {
            light: 1,
            medium: 1,
            heavy: 0,
        };
        p.cargo = Default::default();
        p.tank_fuel = Default::default();
        p.activity_water = Default::default();
        s.logistics.truck_pools = vec![p];
        (c, s, "physical-pool-a".into())
    }
    /// Cases: land:21.22, land:21.25, land:21.29, land:21.37, airlog:54.2
    #[test]
    fn genuine_pool_types_keep_distinct_bands_without_touching_unit_maps() {
        let (c, mut s, id) = setup();
        let unit_maps = serde_json::to_value((
            &s.land.units,
            &s.logistics.fuel_segments,
            &s.land.breakdown.truck_histories,
        ))
        .unwrap();
        record_edge(&mut s, &id, &"C4020".into(), 40, 8, WeatherKind::Sandstorm).unwrap();
        record_light_extra(&mut s, &id, 8).unwrap();
        stop(&mut s, &id, &"C4021".into());
        assert_eq!(
            serde_json::to_value((
                &s.land.units,
                &s.logistics.fuel_segments,
                &s.land.breakdown.truck_histories
            ))
            .unwrap(),
            unit_maps
        );
        let recovered: State = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        let b = &recovered.land.breakdown.pools[&id];
        assert!(b.moving.is_none());
        assert_eq!(b.stopped.len(), 1);
        let groups = check_groups(&c, &recovered, &id, &b.stopped[0]).unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].column, 1);
        assert_eq!(groups[0].assets[0].equipment, Equipment::MediumTruck);
        assert_eq!(groups[1].column, 2);
        assert_eq!(groups[1].assets[0].equipment, Equipment::LightTruck);
        assert_eq!(groups[0].shift, -1); // printed truck BAR -2 plus sandstorm +1
        let before = serde_json::to_value(&s).unwrap();
        assert!(
            record_edge(
                &mut s,
                &id,
                &"C4021".into(),
                i32::MAX,
                4,
                WeatherKind::Normal
            )
            .is_err()
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        let mut private = recovered.clone();
        private
            .land
            .breakdown
            .pools
            .get_mut(&id)
            .unwrap()
            .accumulated_quarters += 4;
        crate::testkit::assert_indistinguishable(&Cna::dev(), &c, &recovered, &private, Side::Axis);
        super::super::super::capability::finish_opstage(&mut s);
        assert!(s.land.breakdown.pools.is_empty());
    }
    /// Incoming physical cohorts inherit their prior checked band, not the receiver's empty history.
    /// Cases: land:21.25, land:21.26, land:21.29
    #[test]
    fn incoming_pool_cohort_keeps_checked_band_after_checkpoint() {
        let (c, mut s, id) = setup();
        record_edge(&mut s, &id, &"C4020".into(), 80, 8, WeatherKind::Normal).unwrap();
        let groups = logistics::pool_fuel::pool_segment_fuel_cohorts(&s, &id).unwrap();
        let medium = groups
            .iter()
            .find(|g| g.kind == FuelTruckKind::Medium)
            .unwrap();
        s.land
            .breakdown
            .pools
            .get_mut(&id)
            .unwrap()
            .truck_histories
            .get_mut(&medium.id)
            .unwrap()
            .checked = Some(2);
        let mut p = s.logistics.truck_pools[0].clone();
        p.id = "physical-pool-b".into();
        p.trucks = Trucks::default();
        s.logistics.truck_pools.push(p);
        let selected = logistics::pool_fuel::remove_selected_pool_fuel_cohorts(
            &mut s,
            &id,
            &[logistics::FuelCohortSelection {
                id: medium.id.clone(),
                count: 1,
            }],
        )
        .unwrap();
        logistics::pool_fuel::restore_pool_fuel_cohorts(&mut s, "physical-pool-b", &selected)
            .unwrap();
        s.logistics.truck_pools[0].trucks.medium -= 1;
        s.logistics.truck_pools[1].trucks.medium += 1;
        let mut s: State = serde_json::from_value(serde_json::to_value(s).unwrap()).unwrap();
        record_edge(
            &mut s,
            "physical-pool-b",
            &"C4020".into(),
            4,
            4,
            WeatherKind::Normal,
        )
        .unwrap();
        stop(&mut s, "physical-pool-b", &"C4021".into());
        let b = &s.land.breakdown.pools["physical-pool-b"];
        assert_eq!(b.accumulated_quarters, 4);
        let g = check_groups(&c, &s, "physical-pool-b", &b.stopped[0]).unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].column, 3);
        assert_eq!(g[0].assets[0].cohort, medium.id);
    }
}
