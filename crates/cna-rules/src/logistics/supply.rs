//! Costing and transactional withdrawals from existing friendly holdings.

use std::collections::BTreeMap;

use cna_content::scenario::Supplies;
use cna_content::units::Toe;
use cna_core::ids::UnitId;
use cna_core::quantity::{
    AmmoPoints, FuelPoints, FuelTenths, StoresPoints, ToeStrengthPoints, WaterPoints,
};
use cna_tables::airlog::supply::{AmmoAction, AmmoCost, AmmoMode};
use serde::{Deserialize, Serialize};

use crate::content::CnaContent;
use crate::state::{DumpLocation, LandUnit, Location, LogisticsState, State};

/// Exact demand before any source-specific fuel rounding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplyDemand {
    pub fuel: FuelTenths,
    pub ammo: AmmoPoints,
    pub stores: StoresPoints,
    pub water: WaterPoints,
}

/// Supply identities presented only to their owning side.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "source", content = "id", rename_all = "snake_case")]
pub enum SupplySource {
    /// Scenario-authorized unlimited stocks at a resolved friendly location.
    Unlimited,
    /// The consuming unit's tanks; other units' tanks require siphoning.
    Tank,
    /// The consuming unit's ready ammunition.
    ReadyAmmo,
    /// First-line cargo belonging to a friendly unit in the same hex.
    UnitStock(UnitId),
    /// An active, real, friendly dump in the same hex.
    Dump(String),
    /// Only the canonical Air facade grants exact facility access.
    AirDump(String),
    /// One specific pool's vehicle tanks.
    PoolTank(String),
    /// Own second-/third-line fuel cargo; other pools must unload before use.
    PoolStock(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplyDraw {
    pub source: SupplySource,
    pub amount: SupplyDemand,
}

/// Deliberately generic errors: no enemy holdings or locations are disclosed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupplyError {
    Invalid,
    Insufficient,
    /// No printed rate or identified equipment for an HQ (units gap U-025).
    UnknownFuelRate,
    /// Required content or a separate rule procedure is not available yet.
    Unsupported {
        case: &'static str,
    },
}

impl SupplyDemand {
    pub(super) fn valid(self) -> bool {
        self.fuel.get() >= 0
            && self.ammo.get() >= 0
            && self.stores.get() >= 0
            && self.water.get() >= 0
    }

    pub(super) fn checked_add(self, other: Self) -> Result<Self, SupplyError> {
        Ok(Self {
            fuel: FuelTenths::new(
                self.fuel
                    .get()
                    .checked_add(other.fuel.get())
                    .ok_or(SupplyError::Invalid)?,
            ),
            ammo: AmmoPoints::new(
                self.ammo
                    .get()
                    .checked_add(other.ammo.get())
                    .ok_or(SupplyError::Invalid)?,
            ),
            stores: StoresPoints::new(
                self.stores
                    .get()
                    .checked_add(other.stores.get())
                    .ok_or(SupplyError::Invalid)?,
            ),
            water: WaterPoints::new(
                self.water
                    .get()
                    .checked_add(other.water.get())
                    .ok_or(SupplyError::Invalid)?,
            ),
        })
    }
}

/// Current strength from the mutable TOE and the printed class maximum.
/// Missing composition is rejected rather than replaced by full strength.
/// U/O indicators contain the actual arrival strength, never a deficit or increment.
/// Cases: land:4.45, airlog:49.12, airlog:50.13
pub fn toe_strength(
    content: &CnaContent,
    unit: &LandUnit,
) -> Result<ToeStrengthPoints, SupplyError> {
    let max = || {
        content
            .units
            .units
            .get(&unit.id)
            .and_then(|oa| oa.class.as_ref())
            .and_then(|id| content.units.classes.get(id))
            .and_then(|class| class.max_toe)
            .ok_or(SupplyError::Unsupported { case: "land:4.46" })
    };
    let strength = match &unit.toe {
        Some(Toe::Normal(_)) => max()?,
        Some(Toe::Under { under }) => {
            let maximum = max().map_err(|_| SupplyError::Invalid)?;
            if *under < 0 || *under >= maximum {
                return Err(SupplyError::Invalid);
            }
            *under
        }
        Some(Toe::Over { over }) => {
            let maximum = max().map_err(|_| SupplyError::Invalid)?;
            if *over <= maximum || *over < 0 {
                return Err(SupplyError::Invalid);
            }
            *over
        }
        Some(Toe::Weapons(weapons)) => weapons.iter().try_fold(0i32, |sum, w| {
            if w.n < 0 {
                return Err(SupplyError::Invalid);
            }
            sum.checked_add(w.n).ok_or(SupplyError::Invalid)
        })?,
        _ => return Err(SupplyError::Unsupported { case: "land:4.46" }),
    };
    if strength < 0 {
        return Err(SupplyError::Invalid);
    }
    Ok(ToeStrengthPoints::new(strength))
}

/// The adopted house rate covers only numeric, unparenthesized HQ points without equipment.
/// Explicit weapons and attached trucks retain their independent printed rates.
/// Cases: airlog:49.12, airlog:49.13
/// Interpretations: interp:units-0005
pub(super) fn house_rule_hq_strength(
    content: &CnaContent,
    unit: &LandUnit,
) -> Result<Option<i32>, SupplyError> {
    let class = content
        .units
        .units
        .get(&unit.id)
        .and_then(|row| row.class.as_ref())
        .and_then(|id| content.units.classes.get(id));
    if class.is_some_and(|c| c.unit_type == "headquarters" && !c.max_toe_paren)
        && matches!(
            unit.toe,
            Some(
                Toe::Normal(cna_content::units::NormalToe::N)
                    | Toe::Under { .. }
                    | Toe::Over { .. }
            )
        )
    {
        return toe_strength(content, unit)
            .map(|n| Some(n.get()))
            .map_err(|error| {
                if error == (SupplyError::Unsupported { case: "land:4.46" }) {
                    SupplyError::UnknownFuelRate
                } else {
                    error
                }
            });
    }
    Ok(None)
}

/// Fuel for one movement segment, including the unit's first-line trucks.
/// Quarter CP are rounded up to whole CP before looking up the chart; all vehicle
/// costs are added exactly before a source draw is rounded. Non-movement CP must
/// be excluded by the movement caller. Special patrols need their own procedure.
/// Cases: airlog:49.12, airlog:49.13, land:4.48
/// Interpretations: interp:airlog-0001, interp:units-0005
/// Numeric HQ points use the adopted factor-one house rate; other unidentified equipment stays unresolved.
pub fn movement_fuel_cost(
    content: &CnaContent,
    state: &State,
    unit_id: &UnitId,
    cp_quarters: i32,
) -> Result<FuelTenths, SupplyError> {
    if cp_quarters < 0 {
        return Err(SupplyError::Invalid);
    }
    let unit = state.land.units.get(unit_id).ok_or(SupplyError::Invalid)?;
    let oa = content
        .units
        .units
        .get(unit_id)
        .ok_or(SupplyError::Invalid)?;
    let class = oa
        .class
        .as_ref()
        .and_then(|id| content.units.classes.get(id));
    let cp = cp_quarters / 4 + i32::from(cp_quarters % 4 != 0);
    if cp == 0 {
        return Ok(FuelTenths::ZERO);
    }
    let cost = |rate: i32, n: i32| -> Result<i32, SupplyError> {
        if n < 0 {
            return Err(SupplyError::Invalid);
        }
        content
            .tables
            .airlog
            .fuel_consumption
            .fuel_for(rate, cp)
            .ok_or(SupplyError::Unsupported {
                case: "airlog:49.19",
            })?
            .get()
            .checked_mul(n)
            .ok_or(SupplyError::Invalid)
    };
    let mut total = 0i32;
    let house = house_rule_hq_strength(content, unit)?;
    match &unit.toe {
        Some(Toe::Weapons(weapons)) => {
            for point in weapons {
                let weapon = content
                    .units
                    .weapons
                    .get(&point.weapon)
                    .ok_or(SupplyError::Unsupported { case: "land:4.48" })?;
                let rate = weapon.fuel_rate.ok_or(SupplyError::Unsupported {
                    case: "airlog:49.12",
                })?;
                total = total
                    .checked_add(cost(rate, point.n)?)
                    .ok_or(SupplyError::Invalid)?;
            }
        }
        _ if class.is_some_and(|c| c.unit_type == "recce") => {
            total = cost(1, toe_strength(content, unit)?.get())?;
        }
        _ if class.is_some_and(|c| matches!(c.unit_type.as_str(), "infantry" | "engineer")) => {}
        _ if class.is_some_and(|c| c.unit_type == "headquarters" && c.max_toe_paren) => {}
        _ if house.is_some() => {
            total = cost(1, house.ok_or(SupplyError::Invalid)?)?;
        }
        _ if class.is_some_and(|c| c.unit_type == "headquarters") => {
            return Err(SupplyError::UnknownFuelRate);
        }
        _ if class.is_none() => return Err(SupplyError::Unsupported { case: "land:4.46" }),
        _ => {
            return Err(SupplyError::Unsupported {
                case: "airlog:49.12",
            });
        }
    }
    let trucks = truck_count(unit)?;
    total = total
        .checked_add(cost(1, trucks)?)
        .ok_or(SupplyError::Invalid)?;
    Ok(FuelTenths::new(total))
}

/// Ammunition for the TOE actually participating in a land combat action.
/// Combat selects the chart action appropriate to the firing unit and mode.
/// Cases: airlog:50.13, airlog:50.2
pub fn ammunition_cost(
    content: &CnaContent,
    mode: AmmoMode,
    action: AmmoAction,
    participating_toe: ToeStrengthPoints,
) -> Result<AmmoPoints, SupplyError> {
    if participating_toe.get() < 0 {
        return Err(SupplyError::Invalid);
    }
    if mode != AmmoMode::Played
        || !matches!(
            action,
            AmmoAction::Barrage
                | AmmoAction::AntiArmor
                | AmmoAction::CloseAssaultArmorGunMgInfHvywpnInf
                | AmmoAction::CloseAssaultInfClass
                | AmmoAction::AntiAirSingleTargetGroup
        )
    {
        return Err(SupplyError::Unsupported {
            case: "airlog:50.2",
        });
    }
    match content
        .tables
        .airlog
        .ammunition_consumption
        .cost(mode, action)
    {
        Some(AmmoCost::Points(p)) => p
            .get()
            .checked_mul(participating_toe.get())
            .map(AmmoPoints::new)
            .ok_or(SupplyError::Invalid),
        _ => Err(SupplyError::Unsupported {
            case: "airlog:50.2",
        }),
    }
}

fn truck_count(unit: &LandUnit) -> Result<i32, SupplyError> {
    if unit.trucks.light < 0 || unit.trucks.medium < 0 || unit.trucks.heavy < 0 {
        return Err(SupplyError::Invalid);
    }
    unit.trucks
        .light
        .checked_add(unit.trucks.medium)
        .and_then(|n| n.checked_add(unit.trucks.heavy))
        .ok_or(SupplyError::Invalid)
}

pub(super) fn stock_demand(stock: Supplies) -> Result<SupplyDemand, SupplyError> {
    let d = SupplyDemand {
        fuel: FuelTenths::new(stock.fuel.checked_mul(10).ok_or(SupplyError::Invalid)?),
        ammo: AmmoPoints::new(stock.ammo),
        stores: StoresPoints::new(stock.stores),
        water: WaterPoints::new(stock.water),
    };
    if !d.valid() {
        return Err(SupplyError::Invalid);
    }
    Ok(d)
}

/// Friendly sources accessible to this on-map unit, in stable identity order.
/// No enemy stock or another unit's tank/ready-ammunition is returned.
/// Cases: airlog:49.15, airlog:49.16, airlog:50.15
pub fn available_sources(state: &State, unit_id: &UnitId) -> Result<Vec<SupplyDraw>, SupplyError> {
    let unit = state.land.units.get(unit_id).ok_or(SupplyError::Invalid)?;
    available_sources_at_location(state, unit_id, &unit.location)
}

/// Sources at a recorded segment origin, with the consuming unit's tanks always
/// available. The caller obtains the origin from trusted movement state.
/// Cases: airlog:49.15, airlog:49.16, airlog:50.15
pub fn available_sources_at(
    state: &State,
    unit_id: &UnitId,
    hex: &cna_core::ids::HexId,
) -> Result<Vec<SupplyDraw>, SupplyError> {
    let unit = state.land.units.get(unit_id).ok_or(SupplyError::Invalid)?;
    unit.location.hex().ok_or(SupplyError::Invalid)?;
    available_sources_at_location(state, unit_id, &Location::Hex { hex: hex.clone() })
}

/// Friendly stocks in the same map hex or off-map location.
/// Cases: airlog:51.15, airlog:49.15, land:8.84
pub fn available_sources_at_location(
    state: &State,
    unit_id: &UnitId,
    location: &Location,
) -> Result<Vec<SupplyDraw>, SupplyError> {
    let unit = state.land.units.get(unit_id).ok_or(SupplyError::Invalid)?;
    if !matches!(
        unit.location,
        Location::Hex { .. } | Location::OffMap { .. }
    ) || !matches!(location, Location::Hex { .. } | Location::OffMap { .. })
    {
        return Err(SupplyError::Invalid);
    }
    let mut sources = Vec::new();
    if let Some(holdings) = state.logistics.unit_supply.get(unit_id) {
        if holdings.tank_fuel.get() < 0 || holdings.ready_ammo.get() < 0 {
            return Err(SupplyError::Invalid);
        }
        sources.push(SupplyDraw {
            source: SupplySource::Tank,
            amount: SupplyDemand {
                fuel: holdings.tank_fuel,
                ..SupplyDemand::default()
            },
        });
        sources.push(SupplyDraw {
            source: SupplySource::ReadyAmmo,
            amount: SupplyDemand {
                ammo: holdings.ready_ammo,
                ..SupplyDemand::default()
            },
        });
    }
    sources.extend(local_stocks(state, unit.side, location)?);
    Ok(sources)
}

/// Shared location stocks. Second-/third-line cargo is deliberately excluded.
pub(super) fn local_stocks(
    state: &State,
    side: cna_protocol::Side,
    location: &Location,
) -> Result<Vec<SupplyDraw>, SupplyError> {
    let mut sources = Vec::new();
    for (id, holdings) in &state.logistics.unit_supply {
        let Some(carrier) = state.land.units.get(id) else {
            continue;
        };
        if carrier.side == side && &carrier.location == location && truck_count(carrier)? > 0 {
            sources.push(SupplyDraw {
                source: SupplySource::UnitStock(id.clone()),
                amount: stock_demand(holdings.carried)?,
            });
        }
    }
    for (id, dump) in &state.logistics.dumps {
        if dump.side == side
            && dump.active
            && !dump.dummy
            && match (&dump.location, location) {
                (DumpLocation::Hex { hex: a }, Location::Hex { hex: b }) => a == b,
                (DumpLocation::OffMap { id: a }, Location::OffMap { id: b }) => a == b,
                _ => false,
            }
        {
            sources.push(SupplyDraw {
                source: SupplySource::Dump(id.clone()),
                amount: stock_demand(dump.supplies)?,
            });
        }
    }
    Ok(sources)
}

/// Withdraw an explicit allocation atomically. Repeated source entries are combined
/// before whole-point fuel rounding. The allocation must exactly meet the demand.
/// Cases: airlog:49.15, airlog:49.16, airlog:50.15
/// Interpretations: interp:airlog-0001
pub fn spend_for_unit(
    state: &mut State,
    unit_id: &UnitId,
    demand: SupplyDemand,
    draws: &[SupplyDraw],
) -> Result<(), SupplyError> {
    let sources = available_sources(state, unit_id)?
        .into_iter()
        .map(|s| (s.source, s.amount))
        .collect();
    state.logistics = apply_draws(state, unit_id, demand, draws, &sources, &BTreeMap::new())?;
    Ok(())
}

/// Validate and stage all withdrawals, including a source's earlier rounded draw.
pub(super) fn apply_draws(
    state: &State,
    unit_id: &UnitId,
    demand: SupplyDemand,
    draws: &[SupplyDraw],
    sources: &BTreeMap<SupplySource, SupplyDemand>,
    prior: &BTreeMap<SupplySource, FuelTenths>,
) -> Result<LogisticsState, SupplyError> {
    apply_draws_from_logistics(&state.logistics, unit_id, demand, draws, sources, prior)
}
pub(super) fn apply_draws_from_logistics(
    logistics: &LogisticsState,
    unit_id: &UnitId,
    demand: SupplyDemand,
    draws: &[SupplyDraw],
    sources: &BTreeMap<SupplySource, SupplyDemand>,
    prior: &BTreeMap<SupplySource, FuelTenths>,
) -> Result<LogisticsState, SupplyError> {
    withdraw_draws(logistics, Some(unit_id), demand, draws, sources, prior)
}

/// Carrier-neutral exact withdrawal. Implicit unit tanks require a real unit id.
pub(super) fn withdraw_draws(
    logistics: &LogisticsState,
    unit_id: Option<&UnitId>,
    demand: SupplyDemand,
    draws: &[SupplyDraw],
    sources: &BTreeMap<SupplySource, SupplyDemand>,
    prior: &BTreeMap<SupplySource, FuelTenths>,
) -> Result<LogisticsState, SupplyError> {
    let mut next = logistics.clone();
    withdraw_into(&mut next, unit_id, demand, draws, sources, prior)?;
    Ok(next)
}

/// Mutate a disposable draft only. Callers must discard it if any withdrawal fails.
pub(super) fn withdraw_into(
    next: &mut LogisticsState,
    unit_id: Option<&UnitId>,
    demand: SupplyDemand,
    draws: &[SupplyDraw],
    sources: &BTreeMap<SupplySource, SupplyDemand>,
    prior: &BTreeMap<SupplySource, FuelTenths>,
) -> Result<(), SupplyError> {
    if !demand.valid() {
        return Err(SupplyError::Invalid);
    }
    let mut allocations = BTreeMap::<SupplySource, SupplyDemand>::new();
    let mut total = SupplyDemand::default();
    for draw in draws {
        if !draw.amount.valid() || !sources.contains_key(&draw.source) {
            return Err(SupplyError::Invalid);
        }
        total = total.checked_add(draw.amount)?;
        let old = allocations.get(&draw.source).copied().unwrap_or_default();
        allocations.insert(draw.source.clone(), old.checked_add(draw.amount)?);
    }
    if total != demand {
        return Err(SupplyError::Invalid);
    }
    for (source, amount) in &allocations {
        let capacity = sources[source];
        if amount.fuel > capacity.fuel
            || amount.ammo > capacity.ammo
            || amount.stores > capacity.stores
            || amount.water > capacity.water
        {
            return Err(SupplyError::Insufficient);
        }
    }
    for (source, amount) in allocations {
        match source {
            SupplySource::Unlimited => {}
            SupplySource::Tank => {
                next.unit_supply
                    .get_mut(unit_id.ok_or(SupplyError::Invalid)?)
                    .ok_or(SupplyError::Invalid)?
                    .tank_fuel -= amount.fuel
            }
            SupplySource::PoolTank(id) => {
                if !amount.ammo.is_zero() || !amount.stores.is_zero() || !amount.water.is_zero() {
                    return Err(SupplyError::Invalid);
                }
                let pool = next
                    .truck_pools
                    .iter_mut()
                    .find(|p| p.id == id)
                    .ok_or(SupplyError::Invalid)?;
                pool.tank_fuel = FuelTenths::new(
                    pool.tank_fuel
                        .get()
                        .checked_sub(amount.fuel.get())
                        .filter(|n| *n >= 0)
                        .ok_or(SupplyError::Insufficient)?,
                );
            }
            SupplySource::ReadyAmmo => {
                next.unit_supply
                    .get_mut(unit_id.ok_or(SupplyError::Invalid)?)
                    .ok_or(SupplyError::Invalid)?
                    .ready_ammo -= amount.ammo
            }
            source => {
                if let SupplySource::AirDump(id) = &source
                    && !next.air_dumps.get(id).is_some_and(|d| &d.id == id)
                {
                    return Err(SupplyError::Invalid);
                }
                let before = prior.get(&source).copied().unwrap_or_default();
                let after = FuelTenths::new(
                    before
                        .get()
                        .checked_add(amount.fuel.get())
                        .ok_or(SupplyError::Invalid)?,
                );
                let withdrawal = after.ceil_points() - before.ceil_points();
                // Rounded fuel already paid for remains usable after its old source
                // leaves the origin or changes hands. No fresh stock is taken then.
                if withdrawal.is_zero()
                    && amount.ammo.is_zero()
                    && amount.stores.is_zero()
                    && amount.water.is_zero()
                {
                    continue;
                }
                let site = match &source {
                    SupplySource::UnitStock(id) => {
                        super::cargo_history::CargoSite::Unit(id.clone())
                    }
                    SupplySource::PoolStock(id) => {
                        super::cargo_history::CargoSite::Pool(id.clone())
                    }
                    SupplySource::Dump(id) => super::cargo_history::CargoSite::Dump(id.clone()),
                    SupplySource::AirDump(id) => {
                        super::cargo_history::CargoSite::AirDump(id.clone())
                    }
                    _ => return Err(SupplyError::Invalid),
                };
                super::cargo_history::retire_debit(
                    next,
                    &site,
                    Supplies {
                        fuel: withdrawal.get(),
                        ammo: amount.ammo.get(),
                        stores: amount.stores.get(),
                        water: amount.water.get(),
                    },
                )?;
                let stock = match source {
                    SupplySource::UnitStock(id) => {
                        &mut next
                            .unit_supply
                            .get_mut(&id)
                            .ok_or(SupplyError::Invalid)?
                            .carried
                    }
                    SupplySource::PoolStock(id) => {
                        &mut next
                            .truck_pools
                            .iter_mut()
                            .find(|p| p.id == id)
                            .ok_or(SupplyError::Invalid)?
                            .cargo
                    }
                    SupplySource::Dump(id) => {
                        &mut next
                            .dumps
                            .get_mut(&id)
                            .ok_or(SupplyError::Invalid)?
                            .supplies
                    }
                    SupplySource::AirDump(id) => {
                        &mut next
                            .air_dumps
                            .get_mut(&id)
                            .ok_or(SupplyError::Invalid)?
                            .supplies
                    }
                    _ => return Err(SupplyError::Invalid),
                };
                deduct_stock(stock, amount, withdrawal)?;
            }
        }
    }
    Ok(())
}

fn deduct_stock(
    stock: &mut Supplies,
    amount: SupplyDemand,
    fuel_withdrawal: FuelPoints,
) -> Result<(), SupplyError> {
    stock.fuel = stock
        .fuel
        .checked_sub(fuel_withdrawal.get())
        .filter(|n| *n >= 0)
        .ok_or(SupplyError::Insufficient)?;
    stock.ammo = stock
        .ammo
        .checked_sub(amount.ammo.get())
        .filter(|n| *n >= 0)
        .ok_or(SupplyError::Insufficient)?;
    stock.stores = stock
        .stores
        .checked_sub(amount.stores.get())
        .filter(|n| *n >= 0)
        .ok_or(SupplyError::Insufficient)?;
    stock.water = stock
        .water
        .checked_sub(amount.water.get())
        .filter(|n| *n >= 0)
        .ok_or(SupplyError::Insufficient)?;
    Ok(())
}

#[cfg(test)]
mod tests;

/// Scenario supply access is checked against resolved membership, never a city rectangle.
/// Water remains a well/pipeline draw under52.11, not a fabricated cargo stock.
/// Cases: scen:60.44, airlog:51.15, airlog:52.11
pub fn available_sources_with_content(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<Vec<SupplyDraw>, SupplyError> {
    let location = &state
        .land
        .units
        .get(id)
        .ok_or(SupplyError::Invalid)?
        .location;
    available_sources_at_with_content(content, state, id, location)
}
/// Content-aware sources at the trusted movement origin, including scenario supply.
/// Cases: scen:60.44, airlog:49.16, land:8.84
pub fn available_sources_at_with_content(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    location: &Location,
) -> Result<Vec<SupplyDraw>, SupplyError> {
    let mut sources = available_sources_at_location(state, id, location)?;
    // Only catalogue locations are named boxes. Opaque traveling-group locations
    // identify co-located carried stocks, never a dump or a scenario supply base.
    if matches!(location, Location::OffMap { id } if !content.areas.locations.contains_key(id)) {
        sources.retain(|s| {
            matches!(
                s.source,
                SupplySource::Tank | SupplySource::ReadyAmmo | SupplySource::UnitStock(_)
            )
        });
        return Ok(sources);
    }
    let unit = state.land.units.get(id).ok_or(SupplyError::Invalid)?;
    add_unlimited_source(content, unit.side, location, &mut sources)?;
    Ok(sources)
}

/// Resolved scenario supply rights, shared by genuine units and truck pools.
/// Cases: scen:60.44, airlog:57.0
pub(super) fn add_unlimited_source(
    content: &CnaContent,
    side: cna_protocol::Side,
    location: &Location,
    sources: &mut Vec<SupplyDraw>,
) -> Result<(), SupplyError> {
    if matches!(location, Location::OffMap { id } if !content.areas.locations.contains_key(id)) {
        return Ok(());
    }
    if let Some(unlimited) = &content.scenario.supply.unlimited_supply
        && unlimited.side == side
    {
        for place in &unlimited.locations {
            let area = content
                .areas
                .areas
                .get(place)
                .ok_or(SupplyError::Unsupported { case: "scen:60.44" })?;
            if area.membership_status != "resolved" {
                return Err(SupplyError::Unsupported { case: "scen:60.44" });
            }
            let matches = match location {
                Location::Hex { hex } => area.hex_ids.contains(hex),
                Location::OffMap { id } => area.location_ids.contains(id),
                _ => false,
            };
            if matches {
                sources.push(SupplyDraw {
                    source: SupplySource::Unlimited,
                    amount: SupplyDemand {
                        fuel: FuelTenths::new(i32::MAX),
                        ammo: AmmoPoints::new(i32::MAX),
                        stores: StoresPoints::new(i32::MAX),
                        water: WaterPoints::ZERO,
                    },
                });
                break;
            }
        }
    }
    Ok(())
}
/// Spend explicit content-authorized sources; the state-only helper remains limited
/// to actual holdings. No enemy side gains access to the scenario's supply source.
/// Cases: scen:60.44, airlog:51.15, airlog:49.15, airlog:50.15
pub fn spend_for_unit_with_content(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
    demand: SupplyDemand,
    draws: &[SupplyDraw],
) -> Result<(), SupplyError> {
    let sources = available_sources_with_content(content, state, id)?
        .into_iter()
        .map(|s| (s.source, s.amount))
        .collect();
    state.logistics = apply_draws(state, id, demand, draws, &sources, &BTreeMap::new())?;
    Ok(())
}
