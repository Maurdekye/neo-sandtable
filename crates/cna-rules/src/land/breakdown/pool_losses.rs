//! Own pool loss selections preserve exact physical cohorts and every carried holding.
use super::{
    core,
    pools::{self, PoolAsset, PoolCheckGroup},
};
use crate::{
    CnaContent, State,
    logistics::{self, CargoPacking, FuelCohortSelection},
    steps::illegal,
};
use cna_content::{scenario::Supplies, units::Trucks};
use cna_core::{engine::Rejection, ids::HexId};
use cna_tables::airlog::trucks::TruckType;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolOutcome {
    pub pool: String,
    pub group: PoolCheckGroup,
    pub percent: i32,
    pub broken: i32,
    pub origin: HexId,
    pub destination: HexId,
    pub require_origin: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolLossPlan {
    pub pool: String,
    pub losses: Vec<FuelCohortSelection>,
    pub at_origin: Vec<FuelCohortSelection>,
    pub partition: PoolPartition,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolPartition {
    pub working: CargoPacking,
    pub origin: CargoPacking,
    pub destination: CargoPacking,
    pub origin_tank_fuel_tenths: i32,
    pub destination_tank_fuel_tenths: i32,
    pub origin_activity_water: i32,
    pub destination_activity_water: i32,
}
fn neutral(a: &[PoolAsset]) -> Vec<core::CarrierAsset> {
    a.iter()
        .map(|a| core::CarrierAsset {
            carrier: a.pool.clone(),
            equipment: a.equipment.clone(),
            points: a.points,
        })
        .collect()
}
fn selection(choices: &[FuelCohortSelection]) -> Result<BTreeMap<String, i32>, Rejection> {
    let mut out = BTreeMap::new();
    for choice in choices {
        if choice.count <= 0 || out.insert(choice.id.clone(), choice.count).is_some() {
            return Err(illegal(
                "select each physical truck cohort once with a positive count",
            ));
        }
    }
    Ok(out)
}
fn type_count<'a>(
    t: &'a mut Trucks,
    equipment: &super::Equipment,
) -> Result<&'a mut i32, Rejection> {
    match equipment {
        super::Equipment::LightTruck => Ok(&mut t.light),
        super::Equipment::MediumTruck => Ok(&mut t.medium),
        super::Equipment::HeavyTruck => Ok(&mut t.heavy),
        _ => Err(illegal("pool losses contain non-truck equipment")),
    }
}
fn counts(assets: &[PoolAsset], selected: &BTreeMap<String, i32>) -> Result<Trucks, Rejection> {
    let mut out = Trucks::default();
    let mut seen = BTreeSet::new();
    for asset in assets {
        if !seen.insert(&asset.cohort) {
            return Err(illegal("duplicate physical pool cohort"));
        }
        let n = selected.get(&asset.cohort).copied().unwrap_or(0);
        if n < 0 || n > asset.points {
            return Err(illegal("pool loss exceeds its physical cohort"));
        }
        let count = type_count(&mut out, &asset.equipment)?;
        *count = count
            .checked_add(n)
            .ok_or_else(|| illegal("truck count overflow"))?;
    }
    if selected.keys().any(|id| !seen.contains(id)) {
        return Err(illegal("pool loss selects a different cohort"));
    }
    Ok(out)
}
fn sub(a: Trucks, b: Trucks) -> Result<Trucks, Rejection> {
    let n = |x: i32, y: i32| {
        x.checked_sub(y)
            .filter(|v| *v >= 0 && y >= 0)
            .ok_or_else(|| illegal("truck loss exceeds working holdings"))
    };
    Ok(Trucks {
        light: n(a.light, b.light)?,
        medium: n(a.medium, b.medium)?,
        heavy: n(a.heavy, b.heavy)?,
    })
}
fn plus(a: Supplies, b: Supplies, d: Supplies) -> Result<Supplies, Rejection> {
    let n = |x: i32, y: i32, z: i32| {
        x.checked_add(y)
            .and_then(|v| v.checked_add(z))
            .ok_or_else(|| illegal("cargo overflow"))
    };
    Ok(Supplies {
        ammo: n(a.ammo, b.ammo, d.ammo)?,
        fuel: n(a.fuel, b.fuel, d.fuel)?,
        stores: n(a.stores, b.stores, d.stores)?,
        water: n(a.water, b.water, d.water)?,
    })
}
fn fuel_capacity(c: &CnaContent, t: Trucks) -> i64 {
    [
        (TruckType::Light, t.light),
        (TruckType::Medium, t.medium),
        (TruckType::Heavy, t.heavy),
    ]
    .into_iter()
    .map(|(k, n)| {
        i64::from(n)
            * i64::from(
                c.tables
                    .airlog
                    .truck_characteristics
                    .truck(k)
                    .fuel_capacity_points,
            )
            * 10
    })
    .sum()
}
/// Only the disclosed roll and own pool holdings enter answer validation. No state is changed.
/// Cases: land:21.35, land:21.36, land:21.41, land:21.43, airlog:54.2
pub fn validate(
    c: &CnaContent,
    s: &State,
    o: &PoolOutcome,
    p: &PoolLossPlan,
) -> Result<(), Rejection> {
    if p.pool != o.pool {
        return Err(illegal("loss choice belongs to another convoy"));
    }
    let pool = pools::carrier(s, &p.pool).map_err(Rejection::Engine)?;
    if pool.side != o.group.side
        || pool.location.as_ref().and_then(|l| l.hex()) != Some(&o.destination)
    {
        return Err(illegal("pool location changed since breakdown"));
    }
    let loss = selection(&p.losses)?;
    let origin = selection(&p.at_origin)?;
    let allocation: Vec<_> = o
        .group
        .assets
        .iter()
        .map(|a| loss.get(&a.cohort).copied().unwrap_or(0))
        .collect();
    if !core::valid_carrier_allocation(&neutral(&o.group.assets), o.broken, &allocation) {
        return Err(illegal(
            "pool losses must conserve proportional vehicle totals",
        ));
    }
    let lost = counts(&o.group.assets, &loss)?;
    let at = counts(&o.group.assets, &origin)?;
    if origin
        .iter()
        .any(|(id, n)| *n > loss.get(id).copied().unwrap_or(0))
    {
        return Err(illegal("origin selection exceeds broken cohort"));
    }
    if o.require_origin && i64::from(at.total()) * 2 < i64::from(o.broken) {
        return Err(illegal(
            "at least half the convoy's broken trucks must remain at its origin",
        ));
    }
    let current = logistics::pool_fuel::pool_segment_fuel_cohorts(s, &p.pool)
        .map_err(|_| illegal("pool fuel histories changed"))?;
    if o.group.assets.iter().any(|a| {
        !current.iter().any(|g| {
            g.id == a.cohort
                && g.count == a.points
                && matches!(
                    (&a.equipment, g.kind),
                    (
                        super::Equipment::LightTruck,
                        logistics::FuelTruckKind::Light
                    ) | (
                        super::Equipment::MediumTruck,
                        logistics::FuelTruckKind::Medium
                    ) | (
                        super::Equipment::HeavyTruck,
                        logistics::FuelTruckKind::Heavy
                    )
                )
        })
    }) {
        return Err(illegal("pool physical histories changed since breakdown"));
    }
    let working = sub(pool.trucks, lost)?;
    let destination = sub(lost, at)?;
    let q = &p.partition;
    let a = q
        .working
        .totals()
        .map_err(|_| illegal("invalid working cargo"))?;
    let b = q
        .origin
        .totals()
        .map_err(|_| illegal("invalid origin cargo"))?;
    let d = q
        .destination
        .totals()
        .map_err(|_| illegal("invalid destination cargo"))?;
    if plus(a, b, d)? != pool.cargo {
        return Err(illegal(
            "pool cargo must survive between all truck partitions",
        ));
    }
    for (trucks, goods, packing) in [
        (working, a, &q.working),
        (at, b, &q.origin),
        (destination, d, &q.destination),
    ] {
        logistics::validate_packing(c, &trucks, &Trucks::default(), &goods, packing)
            .map_err(|_| illegal("cargo exceeds its pool truck partition"))?;
    }
    let f = q
        .origin_tank_fuel_tenths
        .checked_add(q.destination_tank_fuel_tenths)
        .ok_or_else(|| illegal("fuel overflow"))?;
    let w = q
        .origin_activity_water
        .checked_add(q.destination_activity_water)
        .ok_or_else(|| illegal("water overflow"))?;
    if [
        q.origin_tank_fuel_tenths,
        q.destination_tank_fuel_tenths,
        q.origin_activity_water,
        q.destination_activity_water,
    ]
    .iter()
    .any(|n| *n < 0)
        || f > pool.tank_fuel.get()
        || w > pool.activity_water.get()
    {
        return Err(illegal("pool vehicle reserves must be conserved"));
    }
    if i64::from(q.origin_tank_fuel_tenths) > fuel_capacity(c, at)
        || i64::from(q.destination_tank_fuel_tenths) > fuel_capacity(c, destination)
        || i64::from(pool.tank_fuel.get() - f) > fuel_capacity(c, working)
    {
        return Err(illegal("pool tank fuel exceeds surviving vehicle capacity"));
    }
    if at.total() == 0 && q.origin_activity_water > 0
        || destination.total() == 0 && q.destination_activity_water > 0
        || working.total() == 0 && pool.activity_water.get() - w > 0
    {
        return Err(illegal(
            "activity water requires its physical vehicle partition",
        ));
    }
    Ok(())
}
fn with_losses(
    c: &CnaContent,
    s: &State,
    o: &PoolOutcome,
    chosen: Vec<i32>,
) -> Option<PoolLossPlan> {
    let pool = pools::carrier(s, &o.pool).ok()?;
    let losses: Vec<_> = o
        .group
        .assets
        .iter()
        .zip(chosen)
        .filter(|(_, n)| *n > 0)
        .map(|(a, n)| FuelCohortSelection {
            id: a.cohort.clone(),
            count: n,
        })
        .collect();
    let lost = counts(&o.group.assets, &selection(&losses).ok()?).ok()?;
    let working = sub(pool.trucks, lost).ok()?;
    let (a, b) = super::baseline::pack_two(
        c,
        working,
        Trucks::default(),
        lost,
        Trucks::default(),
        pool.cargo,
    )?;
    let mut partition = PoolPartition {
        working: a,
        origin: b,
        origin_tank_fuel_tenths: (i64::from(pool.tank_fuel.get()) - fuel_capacity(c, working))
            .max(0)
            .try_into()
            .ok()?,
        origin_activity_water: if working.total() == 0 {
            pool.activity_water.get()
        } else {
            0
        },
        ..Default::default()
    };
    if !o.require_origin {
        partition.destination = std::mem::take(&mut partition.origin);
        partition.destination_tank_fuel_tenths =
            std::mem::take(&mut partition.origin_tank_fuel_tenths);
        partition.destination_activity_water = std::mem::take(&mut partition.origin_activity_water);
    }
    let p = PoolLossPlan {
        pool: o.pool.clone(),
        at_origin: if o.require_origin {
            losses.clone()
        } else {
            vec![]
        },
        losses,
        partition,
    };
    validate(c, s, o, &p).ok()?;
    Some(p)
}
/// Search all floor/ceiling type totals, then exact mixed packing. No legal allocation is skipped.
/// Cases: land:21.35, land:21.36, land:21.41, land:21.43
pub fn plan(c: &CnaContent, s: &State, o: &PoolOutcome) -> Option<PoolLossPlan> {
    let assets = neutral(&o.group.assets);
    let preferred = core::balanced_carrier_allocation(&assets, o.broken)?;
    if let Some(p) = with_losses(c, s, o, preferred) {
        return Some(p);
    }
    let (cells, indices) = super::allocation::grouped_carrier(&assets)?;
    let total: i64 = cells.iter().map(|a| i64::from(a.points)).sum();
    if total <= 0 {
        return None;
    }
    #[allow(clippy::too_many_arguments)]
    fn search(
        c: &CnaContent,
        s: &State,
        o: &PoolOutcome,
        cells: &[core::CarrierAsset],
        indices: &[Vec<usize>],
        total: i64,
        i: usize,
        chosen: &mut Vec<i32>,
    ) -> Option<PoolLossPlan> {
        if i == cells.len() {
            let physical =
                super::allocation::expand_carrier(&neutral(&o.group.assets), indices, chosen)?;
            if !core::valid_carrier_allocation(&neutral(&o.group.assets), o.broken, &physical) {
                return None;
            }
            return with_losses(c, s, o, physical);
        }
        let product = i64::from(cells[i].points) * i64::from(o.broken);
        for n in product / total..=(product + total - 1) / total {
            chosen.push(n.try_into().ok()?);
            let result = search(c, s, o, cells, indices, total, i + 1, chosen);
            chosen.pop();
            if result.is_some() {
                return result;
            }
        }
        None
    }
    search(c, s, o, &cells, &indices, total, 0, &mut vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Location;
    use cna_core::quantity::{FuelTenths, WaterPoints};
    pub(super) fn fixture() -> (CnaContent, State, PoolOutcome) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let mut p = s.logistics.truck_pools[0].clone();
        p.id = "real-pool".into();
        p.side = cna_protocol::Side::Commonwealth;
        p.location = Some(Location::Hex {
            hex: "C4021".into(),
        });
        p.trucks = Trucks {
            light: 2,
            medium: 1,
            heavy: 0,
        };
        p.tank_fuel = FuelTenths::new(210);
        p.activity_water = WaterPoints::new(7);
        p.cargo = Supplies {
            ammo: 1,
            fuel: 99,
            stores: 2,
            water: 66,
        };
        s.logistics.truck_pools = vec![p];
        let gs = logistics::pool_fuel::pool_segment_fuel_cohorts(&s, "real-pool").unwrap();
        let assets = gs
            .into_iter()
            .map(|g| PoolAsset {
                pool: "real-pool".into(),
                equipment: match g.kind {
                    logistics::FuelTruckKind::Light => super::super::Equipment::LightTruck,
                    logistics::FuelTruckKind::Medium => super::super::Equipment::MediumTruck,
                    logistics::FuelTruckKind::Heavy => super::super::Equipment::HeavyTruck,
                },
                points: g.count,
                cohort: g.id,
            })
            .collect();
        let o = PoolOutcome {
            pool: "real-pool".into(),
            group: PoolCheckGroup {
                side: cna_protocol::Side::Commonwealth,
                bar: -2,
                column: 4,
                shift: -2,
                assets,
            },
            percent: 33,
            broken: 1,
            origin: "C4020".into(),
            destination: "C4021".into(),
            require_origin: true,
        };
        (c, s, o)
    }
    /// Cases: land:21.35, land:21.36, land:21.41, land:21.43
    #[test]
    fn mixed_pool_cargo_baseline_is_exact_and_choice_rejection_is_read_only() {
        let (c, s, o) = fixture();
        let before = serde_json::to_value(&s).unwrap();
        let plan = plan(&c, &s, &o).unwrap();
        validate(&c, &s, &o, &plan).unwrap();
        assert_eq!(plan.losses.iter().map(|a| a.count).sum::<i32>(), 1);
        assert_eq!(plan.at_origin.iter().map(|a| a.count).sum::<i32>(), 1);
        let mut bad = plan.clone();
        bad.partition.working.light.fuel += 1;
        assert!(validate(&c, &s, &o, &bad).is_err());
        bad = plan;
        bad.losses[0].id = "unowned-history".into();
        assert!(validate(&c, &s, &o, &bad).is_err());
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        let restored: State = serde_json::from_value(before).unwrap();
        assert_eq!(
            serde_json::to_value(super::plan(&c, &restored, &o)).unwrap(),
            serde_json::to_value(super::plan(&c, &s, &o)).unwrap()
        );
    }
    /// Cases: land:21.35, land:21.43
    #[test]
    fn all_broken_pool_retains_every_reserve_and_cargo() {
        let (c, s, mut o) = fixture();
        o.broken = 3;
        o.percent = 100;
        let p = plan(&c, &s, &o).unwrap();
        validate(&c, &s, &o, &p).unwrap();
        assert_eq!(p.partition.working.totals().unwrap(), Supplies::default());
        assert_eq!(
            p.partition.origin.totals().unwrap(),
            s.logistics.truck_pools[0].cargo
        );
        assert_eq!(p.partition.origin_tank_fuel_tenths, 210);
        assert_eq!(p.partition.origin_activity_water, 7);
    }
}
