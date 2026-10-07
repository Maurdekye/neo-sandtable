//! Owner-selected breakdown allocation preserves the working body and physical marker holdings.
use super::{
    Asset, Equipment, RolledCheck,
    markers::{self, BrokenMarker},
    valid_group_allocation,
};
use crate::{
    CnaContent, State,
    logistics::{self, capacity::CargoPacking},
    steps::illegal,
};
use cna_content::{
    scenario::Supplies,
    units::{Toe, Trucks},
};
use cna_core::{
    engine::Rejection,
    event::EngineEvent,
    ids::UnitId,
    quantity::{FuelTenths, WaterPoints},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LossPlan {
    pub losses: Vec<i32>,
    pub at_origin: Vec<i32>,
    pub partitions: Vec<UnitPartition>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitPartition {
    pub unit: UnitId,
    pub working: CargoPacking,
    pub origin: CargoPacking,
    pub destination: CargoPacking,
    pub origin_transport: Trucks,
    pub destination_transport: Trucks,
    pub origin_passengers: i32,
    pub destination_passengers: i32,
    pub origin_tank_fuel_tenths: i32,
    pub destination_tank_fuel_tenths: i32,
    pub origin_activity_water: i32,
    pub destination_activity_water: i32,
}
fn subtract(a: Trucks, b: Trucks) -> Result<Trucks, Rejection> {
    let sub = |x: i32, y: i32| {
        x.checked_sub(y)
            .filter(|n| *n >= 0 && y >= 0)
            .ok_or_else(|| illegal("truck allocation exceeds the available type"))
    };
    Ok(Trucks {
        light: sub(a.light, b.light)?,
        medium: sub(a.medium, b.medium)?,
        heavy: sub(a.heavy, b.heavy)?,
    })
}
fn plus(a: Trucks, b: Trucks) -> Result<Trucks, Rejection> {
    Ok(Trucks {
        light: a
            .light
            .checked_add(b.light)
            .ok_or_else(|| illegal("truck count overflow"))?,
        medium: a
            .medium
            .checked_add(b.medium)
            .ok_or_else(|| illegal("truck count overflow"))?,
        heavy: a
            .heavy
            .checked_add(b.heavy)
            .ok_or_else(|| illegal("truck count overflow"))?,
    })
}
fn trucks(assets: &[Asset]) -> Result<Trucks, Rejection> {
    let mut t = Trucks::default();
    for a in assets {
        let part = match a.equipment {
            Equipment::LightTruck => Trucks {
                light: a.points,
                ..Trucks::default()
            },
            Equipment::MediumTruck => Trucks {
                medium: a.points,
                ..Trucks::default()
            },
            Equipment::HeavyTruck => Trucks {
                heavy: a.points,
                ..Trucks::default()
            },
            _ => Trucks::default(),
        };
        t = plus(t, part)?;
    }
    Ok(t)
}
fn sum_cargo(a: &Supplies, b: &Supplies, d: &Supplies) -> Result<Supplies, Rejection> {
    let add = |x: i32, y: i32, z: i32| {
        x.checked_add(y)
            .and_then(|n| n.checked_add(z))
            .filter(|n| *n >= 0)
            .ok_or_else(|| illegal("cargo quantity overflow"))
    };
    Ok(Supplies {
        fuel: add(a.fuel, b.fuel, d.fuel)?,
        ammo: add(a.ammo, b.ammo, d.ammo)?,
        stores: add(a.stores, b.stores, d.stores)?,
        water: add(a.water, b.water, d.water)?,
    })
}
fn passenger_capacity(c: &CnaContent, t: Trucks) -> i64 {
    use cna_tables::airlog::trucks::TruckType::*;
    [(Light, t.light), (Medium, t.medium), (Heavy, t.heavy)]
        .into_iter()
        .map(|(k, n)| {
            i64::from(n)
                * i64::from(
                    c.tables
                        .airlog
                        .truck_characteristics
                        .truck(k)
                        .capacity_inf_toe_halves,
                )
        })
        .sum()
}
pub(super) fn marker_fuel_capacity(c: &CnaContent, assets: &[Asset]) -> Result<i32, Rejection> {
    use cna_tables::airlog::trucks::TruckType;
    let mut capacity = 0i64;
    for a in assets {
        let per_point = match &a.equipment {
            Equipment::LightTruck => {
                i64::from(
                    c.tables
                        .airlog
                        .truck_characteristics
                        .truck(TruckType::Light)
                        .fuel_capacity_points,
                ) * 10
            }
            Equipment::MediumTruck => {
                i64::from(
                    c.tables
                        .airlog
                        .truck_characteristics
                        .truck(TruckType::Medium)
                        .fuel_capacity_points,
                ) * 10
            }
            Equipment::HeavyTruck => {
                i64::from(
                    c.tables
                        .airlog
                        .truck_characteristics
                        .truck(TruckType::Heavy)
                        .fuel_capacity_points,
                ) * 10
            }
            Equipment::Weapon(w) => {
                let w = c
                    .units
                    .weapons
                    .get(w)
                    .ok_or_else(|| illegal("unknown broken weapon"))?;
                i64::from(w.cpa)
                    * i64::from(
                        w.fuel_rate
                            .ok_or_else(|| illegal("broken weapon fuel rate is unknown"))?,
                    )
                    * 2
            }
            Equipment::ArmoredRecce => return Err(illegal("broken recce composition is unknown")),
        };
        capacity = capacity
            .checked_add(
                per_point
                    .checked_mul(i64::from(a.points))
                    .ok_or_else(|| illegal("vehicle capacity overflow"))?,
            )
            .ok_or_else(|| illegal("vehicle capacity overflow"))?;
    }
    i32::try_from(capacity).map_err(|_| illegal("vehicle capacity overflow"))
}
/// Remove the exact rolled physical groups before changing attached counts.
/// Cases: land:21.25, land:21.29, airlog:49.16
fn remove_cohorts(
    s: &mut State,
    id: &UnitId,
    assets: &[Asset],
) -> Result<Vec<logistics::TruckFuelCohort>, Rejection> {
    use logistics::{FuelCohortSelection, FuelTruckKind};
    super::cohorts::ensure(s, id).map_err(Rejection::Engine)?;
    let available = logistics::segment_fuel_cohorts(s, id)
        .map_err(|_| illegal("truck histories are inconsistent"))?;
    let mut selected: BTreeMap<String, i32> = BTreeMap::new();
    for a in assets {
        let kind = match a.equipment {
            Equipment::LightTruck => FuelTruckKind::Light,
            Equipment::MediumTruck => FuelTruckKind::Medium,
            Equipment::HeavyTruck => FuelTruckKind::Heavy,
            _ => continue,
        };
        let mut need = a.points;
        for g in available
            .iter()
            .filter(|g| g.kind == kind && a.cohort.as_ref().is_none_or(|id| id == &g.id))
        {
            let used = selected.get(&g.id).copied().unwrap_or(0);
            let take = need.min(g.count - used);
            if take > 0 {
                *selected.entry(g.id.clone()).or_default() += take;
                need -= take;
            }
        }
        if need != 0 {
            return Err(illegal("loss exceeds its rolled physical truck cohort"));
        }
    }
    if selected.is_empty() {
        return Ok(vec![]);
    }
    let choices: Vec<_> = selected
        .into_iter()
        .map(|(id, count)| FuelCohortSelection { id, count })
        .collect();
    let removed = logistics::remove_selected_segment_fuel_cohorts(s, id, &choices)
        .map_err(|_| illegal("truck histories are inconsistent"))?;
    super::cohorts::inherit(s, &removed).map_err(Rejection::Engine)?;
    let old = s.land.units[id].trucks;
    s.land.units.get_mut(id).unwrap().trucks = subtract(old, trucks(assets)?)?;
    Ok(removed)
}
/// Validate proportional equipment losses, source-defined origin placement and every cargo split.
/// A rejected allocation leaves both units and numbered markers unchanged.
/// Cases: land:21.35, land:21.36, land:21.41, land:21.42, land:21.43
/// Interpretations: interp:land-0028
pub fn apply(
    c: &CnaContent,
    s: &mut State,
    outcome: &RolledCheck,
    plan: &LossPlan,
) -> Result<Vec<EngineEvent>, Rejection> {
    if !valid_group_allocation(&outcome.group.assets, outcome.broken, &plan.losses)
        || plan.at_origin.len() != plan.losses.len()
    {
        return Err(illegal(
            "breakdown losses must conserve proportional unit and vehicle totals",
        ));
    }
    let mut by_unit: BTreeMap<UnitId, (Vec<Asset>, Vec<Asset>)> = BTreeMap::new();
    for ((a, n), origin) in outcome
        .group
        .assets
        .iter()
        .zip(&plan.losses)
        .zip(&plan.at_origin)
    {
        if *origin < 0 || *origin > *n {
            return Err(illegal("origin placement exceeds the broken vehicles"));
        }
        let entry = by_unit.entry(a.unit.clone()).or_default();
        if *origin > 0 {
            entry.0.push(Asset {
                points: *origin,
                ..a.clone()
            });
        }
        if *n > *origin {
            entry.1.push(Asset {
                points: *n - *origin,
                ..a.clone()
            });
        }
    }
    for (unit, (origin, destination)) in &by_unit {
        let old: i64 = origin.iter().map(|a| i64::from(a.points)).sum();
        let total = old + destination.iter().map(|a| i64::from(a.points)).sum::<i64>();
        if outcome.require_origin.contains(unit) && old * 2 < total {
            return Err(illegal(
                "at least half of this unit's broken vehicles must remain at the origin",
            ));
        }
    }
    let wanted: BTreeSet<_> = by_unit.keys().cloned().collect();
    let mut seen = BTreeSet::new();
    if plan.partitions.len() != wanted.len()
        || plan
            .partitions
            .iter()
            .any(|p| !wanted.contains(&p.unit) || !seen.insert(p.unit.clone()))
    {
        return Err(illegal(
            "provide one conserved holdings partition for every affected unit",
        ));
    }
    let mut draft = s.clone();
    let mut events = vec![];
    for p in &plan.partitions {
        let (origin, destination) = &by_unit[&p.unit];
        let lost_trucks = plus(trucks(origin)?, trucks(destination)?)?;
        let old = draft.land.units[&p.unit].clone();
        let working_trucks = subtract(old.trucks, lost_trucks)?;
        for t in [p.origin_transport, p.destination_transport] {
            if [t.light, t.medium, t.heavy].into_iter().any(|n| n < 0) {
                return Err(illegal("transport counts cannot be negative"));
            }
        }
        let lost_transport = plus(p.origin_transport, p.destination_transport)?;
        let working_transport = subtract(old.transport_trucks, lost_transport)?;
        let origin_trucks = trucks(origin)?;
        let destination_trucks = trucks(destination)?;
        let stock = draft
            .logistics
            .unit_supply
            .get(&p.unit)
            .cloned()
            .unwrap_or_default();
        let cargo = p
            .working
            .totals()
            .map_err(|_| illegal("invalid working cargo"))?;
        let origin_cargo = p
            .origin
            .totals()
            .map_err(|_| illegal("invalid origin cargo"))?;
        let destination_cargo = p
            .destination
            .totals()
            .map_err(|_| illegal("invalid destination cargo"))?;
        if sum_cargo(&cargo, &origin_cargo, &destination_cargo)? != stock.carried {
            return Err(illegal(
                "cargo must be conserved between working and broken trucks",
            ));
        }
        for (attached, transport, expected, packing) in [
            (&working_trucks, &working_transport, &cargo, &p.working),
            (
                &origin_trucks,
                &p.origin_transport,
                &origin_cargo,
                &p.origin,
            ),
            (
                &destination_trucks,
                &p.destination_transport,
                &destination_cargo,
                &p.destination,
            ),
        ] {
            logistics::capacity::validate_packing(c, attached, transport, expected, packing)
                .map_err(|_| illegal("cargo or passengers exceed their truck partition"))?;
        }
        let passengers = p
            .origin_passengers
            .checked_add(p.destination_passengers)
            .ok_or_else(|| illegal("passenger count overflow"))?;
        let strength = super::super::formation::strength(c, &draft, &p.unit);
        if p.origin_passengers < 0
            || p.destination_passengers < 0
            || passengers > strength
            || i64::from(p.origin_passengers) * 2 > passenger_capacity(c, p.origin_transport)
            || i64::from(p.destination_passengers) * 2
                > passenger_capacity(c, p.destination_transport)
        {
            return Err(illegal("passenger allocation exceeds transported infantry"));
        }
        if passengers > 0
            && !super::super::formation::class(c, &p.unit)
                .is_some_and(|k| k.unit_type == "infantry")
        {
            return Err(illegal(
                "only infantry may be recorded as embarked passengers",
            ));
        }
        if old.transport_trucks.total() > 0
            && super::super::formation::class(c, &p.unit).is_some_and(|k| k.unit_type == "infantry")
            && i64::from(strength - passengers) * 2 > passenger_capacity(c, working_transport)
        {
            return Err(illegal(
                "men must remain accounted for in their working or broken transport",
            ));
        }
        let fuel = p
            .origin_tank_fuel_tenths
            .checked_add(p.destination_tank_fuel_tenths)
            .ok_or_else(|| illegal("fuel quantity overflow"))?;
        let water = p
            .origin_activity_water
            .checked_add(p.destination_activity_water)
            .ok_or_else(|| illegal("water quantity overflow"))?;
        if [
            p.origin_tank_fuel_tenths,
            p.destination_tank_fuel_tenths,
            p.origin_activity_water,
            p.destination_activity_water,
        ]
        .into_iter()
        .any(|n| n < 0)
            || fuel > stock.tank_fuel.get()
            || water > stock.activity_water.get()
        {
            return Err(illegal("vehicle reserves must be conserved"));
        }
        let origin_cohorts = remove_cohorts(&mut draft, &p.unit, origin)?;
        let destination_cohorts = remove_cohorts(&mut draft, &p.unit, destination)?;
        let u = draft.land.units.get_mut(&p.unit).unwrap();
        u.trucks = working_trucks;
        u.transport_trucks = working_transport;
        for a in origin.iter().chain(destination) {
            if let Equipment::Weapon(w) = &a.equipment {
                let Some(Toe::Weapons(ws)) = &mut u.toe else {
                    return Err(illegal("weapon holdings changed since breakdown"));
                };
                let point = ws
                    .iter_mut()
                    .find(|p| &p.weapon == w)
                    .ok_or_else(|| illegal("weapon holdings changed since breakdown"))?;
                point.n = point
                    .n
                    .checked_sub(a.points)
                    .filter(|n| *n >= 0)
                    .ok_or_else(|| illegal("weapon losses exceed holdings"))?;
            }
        }
        if passengers > 0 {
            u.toe = Some(Toe::Under {
                under: strength - passengers,
            });
        }
        if p.origin_tank_fuel_tenths > marker_fuel_capacity(c, origin)?
            || p.destination_tank_fuel_tenths > marker_fuel_capacity(c, destination)?
        {
            return Err(illegal(
                "fuel exceeds a broken vehicle partition's tank capacity",
            ));
        }
        let working_capacity = logistics::capacity::fuel_capacity(c, &draft, &p.unit)
            .map_err(|_| illegal("working vehicle tank capacity is unknown"))?;
        if stock.tank_fuel.get() - fuel > working_capacity.get() {
            return Err(illegal("fuel exceeds the working vehicles' tank capacity"));
        }
        let holdings = draft
            .logistics
            .unit_supply
            .entry(p.unit.clone())
            .or_default();
        holdings.carried = cargo;
        holdings.tank_fuel = FuelTenths::new(stock.tank_fuel.get() - fuel);
        holdings.activity_water = WaterPoints::new(stock.activity_water.get() - water);
        for (hex, assets, transport, packing, n, tank, water, fuel_cohorts) in [
            (
                &outcome.origins[&p.unit],
                origin,
                p.origin_transport,
                &p.origin,
                p.origin_passengers,
                p.origin_tank_fuel_tenths,
                p.origin_activity_water,
                origin_cohorts,
            ),
            (
                &outcome.destination,
                destination,
                p.destination_transport,
                &p.destination,
                p.destination_passengers,
                p.destination_tank_fuel_tenths,
                p.destination_activity_water,
                destination_cohorts,
            ),
        ] {
            if assets.is_empty() {
                if packing.totals().map_err(|_| illegal("invalid cargo"))? != Supplies::default()
                    || n != 0
                    || tank != 0
                    || water != 0
                {
                    return Err(illegal("empty marker cannot hold cargo or passengers"));
                }
                continue;
            }
            events.extend(markers::add(
                &mut draft,
                BrokenMarker {
                    id: String::new(),
                    side: old.side,
                    hex: hex.clone(),
                    assets: assets.clone(),
                    passengers: if n > 0 {
                        BTreeMap::from([(p.unit.clone(), n)])
                    } else {
                        BTreeMap::new()
                    },
                    transport,
                    cargo: packing.clone(),
                    tank_fuel: FuelTenths::new(tank),
                    activity_water: WaterPoints::new(water),
                    fuel_cohorts,
                },
            ));
        }
        events.push(EngineEvent::new(
            cna_core::visibility::Audience::Side(old.side),
            cna_protocol::GameEvent::UnitUpdated {
                unit: crate::view::unit_view(c, &draft.land.units[&p.unit]),
            },
        ));
    }
    *s = draft;
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::land::breakdown::{Category, CheckGroup};
    use cna_core::ids::HexId;
    fn fixture() -> (CnaContent, State, RolledCheck, LossPlan) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let id: UnitId = "cw.unassigned_inf.1st_rnf_mg_bn".into();
        let u = s.land.units.get_mut(&id).unwrap();
        u.location = crate::state::Location::Hex {
            hex: "C4021".into(),
        };
        u.toe = Some(Toe::Under { under: 2 });
        u.trucks = Trucks {
            medium: 4,
            ..Trucks::default()
        };
        u.transport_trucks = Trucks {
            medium: 2,
            ..Trucks::default()
        };
        let stock = s.logistics.unit_supply.entry(id.clone()).or_default();
        stock.carried.ammo = 8;
        stock.tank_fuel = FuelTenths::new(20);
        stock.activity_water = WaterPoints::new(6);
        let outcome = RolledCheck {
            group: CheckGroup {
                category: Category::Truck,
                bar: -2,
                column: 6,
                shift: -2,
                assets: vec![Asset {
                    unit: id.clone(),
                    equipment: Equipment::MediumTruck,
                    points: 4,
                    cohort: None,
                }],
            },
            percent: 75,
            broken: 3,
            destination: "C4021".into(),
            origins: BTreeMap::from([(id.clone(), "C4020".into())]),
            require_origin: BTreeSet::from([id.clone()]),
        };
        let plan = LossPlan {
            losses: vec![3],
            at_origin: vec![2],
            partitions: vec![UnitPartition {
                unit: id,
                working: CargoPacking::default(),
                origin: CargoPacking {
                    medium: Supplies {
                        ammo: 4,
                        ..Supplies::default()
                    },
                    ..CargoPacking::default()
                },
                destination: CargoPacking {
                    medium: Supplies {
                        ammo: 4,
                        ..Supplies::default()
                    },
                    ..CargoPacking::default()
                },
                origin_transport: Trucks {
                    medium: 1,
                    ..Trucks::default()
                },
                destination_transport: Trucks::default(),
                origin_passengers: 1,
                destination_passengers: 0,
                origin_tank_fuel_tenths: 10,
                destination_tank_fuel_tenths: 10,
                origin_activity_water: 2,
                destination_activity_water: 1,
            }],
        };
        (c, s, outcome, plan)
    }
    /// Cases: land:21.35, land:21.36, land:21.41, land:21.43, land:21.45
    /// Interpretations: interp:land-0028
    #[test]
    fn broken_transport_retains_men_and_every_supply_at_the_selected_locations() {
        let (c, mut s, outcome, plan) = fixture();
        let id = plan.partitions[0].unit.clone();
        apply(&c, &mut s, &outcome, &plan).unwrap();
        assert_eq!(s.land.units[&id].trucks.medium, 1);
        assert_eq!(crate::land::formation::strength(&c, &s, &id), 1);
        let origin = s
            .land
            .breakdown
            .markers
            .values()
            .find(|m| m.hex == HexId::new("C4020"))
            .unwrap();
        assert_eq!(origin.passengers[&id], 1);
        assert_eq!(origin.trucks().medium, 2);
        assert_eq!(origin.cargo.totals().unwrap().ammo, 4);
        let destination = s
            .land
            .breakdown
            .markers
            .values()
            .find(|m| m.hex == HexId::new("C4021"))
            .unwrap();
        assert_eq!(destination.cargo.totals().unwrap().ammo, 4);
        assert_eq!(
            s.logistics.unit_supply[&id].activity_water.get()
                + origin.activity_water.get()
                + destination.activity_water.get(),
            6
        );
        assert_eq!(
            s.logistics.unit_supply[&id].tank_fuel.get()
                + origin.tank_fuel.get()
                + destination.tank_fuel.get(),
            20
        );
        let restored: State = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(&restored).unwrap(),
            serde_json::to_value(&s).unwrap()
        );
    }
    /// Cases: land:21.35, land:21.36, land:21.41, land:21.43
    #[test]
    fn scripted_partitions_validate_for_every_loss_count_and_origin_requirement() {
        let (c, s, outcome, _) = fixture();
        for broken in 1..=4 {
            for require in [false, true] {
                let mut o = outcome.clone();
                o.broken = broken;
                if !require {
                    o.require_origin.clear();
                }
                let plan = super::super::baseline::plan(&c, &s, &o)
                    .expect("valid exact baseline partition");
                let mut draft = s.clone();
                apply(&c, &mut draft, &o, &plan).unwrap();
                let working =
                    crate::land::formation::strength(&c, &draft, &plan.partitions[0].unit);
                let embarked: i32 = draft
                    .land
                    .breakdown
                    .markers
                    .values()
                    .flat_map(|m| m.passengers.values())
                    .sum();
                assert_eq!(working + embarked, 2);
                assert_eq!(
                    draft.logistics.unit_supply[&plan.partitions[0].unit]
                        .carried
                        .ammo
                        + draft
                            .land
                            .breakdown
                            .markers
                            .values()
                            .map(|m| m.cargo.totals().unwrap().ammo)
                            .sum::<i32>(),
                    8
                );
            }
        }
    }
    /// Cases: land:21.36, land:21.41, land:21.43, airlog:49.14
    #[test]
    fn invalid_partitions_are_atomic_and_cannot_destroy_cargo_or_free_passengers() {
        let (c, s, outcome, plan) = fixture();
        assert!(super::super::baseline::plan(&c, &s, &outcome).is_some());
        let unchanged = serde_json::to_value(&s).unwrap();
        for variant in 0..5 {
            let mut p = plan.clone();
            match variant {
                0 => p.partitions[0].origin.medium.ammo -= 1,
                1 => p.at_origin[0] = 1,
                2 => p.partitions[0].origin_passengers = 0,
                3 => {
                    p.partitions[0].destination_tank_fuel_tenths = 61;
                }
                _ => p.partitions[0].origin_transport.medium = i32::MAX,
            }
            let mut draft = s.clone();
            assert!(
                apply(&c, &mut draft, &outcome, &p).is_err(),
                "variant {variant}"
            );
            assert_eq!(serde_json::to_value(&draft).unwrap(), unchanged);
        }
    }
}
