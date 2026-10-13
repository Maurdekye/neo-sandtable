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
/// The cited cw.a not-applicable body is zero for Normal N or absent TOE (units-0007).
/// U/O indicators contain the actual arrival strength, never a deficit or increment.
/// Cases: land:4.45, airlog:49.12, airlog:50.13
pub fn toe_strength(
    content: &CnaContent,
    unit: &LandUnit,
) -> Result<ToeStrengthPoints, SupplyError> {
    toe_strength_from_toe(content, &unit.id, unit.toe.as_ref())
}
fn toe_strength_from_toe(
    content: &CnaContent,
    id: &UnitId,
    toe: Option<&Toe>,
) -> Result<ToeStrengthPoints, SupplyError> {
    if not_applicable_hq_body(content, id, toe) {
        return Ok(ToeStrengthPoints::new(0));
    }
    let max = || {
        content
            .units
            .units
            .get(id)
            .and_then(|oa| oa.class.as_ref())
            .and_then(|id| content.units.classes.get(id))
            .and_then(|class| class.max_toe)
            .ok_or(SupplyError::Unsupported { case: "land:4.46" })
    };
    let strength = match toe {
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

/// The cited cw.a chart maximum is not applicable, distinct from a missing source maximum.
/// Explicit equipment and malformed U/O fields retain their canonical validation paths.
/// Cases: land:4.46, airlog:49.12, airlog:49.13, airlog:52.42
/// Interpretations: interp:units-0007
fn not_applicable_hq_body(content: &CnaContent, id: &UnitId, toe: Option<&Toe>) -> bool {
    content
        .units
        .units
        .get(id)
        .and_then(|row| row.class.as_ref())
        .and_then(|id| content.units.classes.get(id))
        .is_some_and(|class| {
            class.id == "cw.a"
                && class.nation == "cw"
                && class.code == "a"
                && class.unit_type == "headquarters"
                && !class.max_toe_paren
                && class.max_toe.is_none()
        })
        && matches!(
            toe,
            None | Some(Toe::Normal(cna_content::units::NormalToe::N))
        )
}

/// Numeric, unparenthesized HQ points use the adopted house rate; known cw.a body points are zero.
/// Explicit weapons and attached trucks retain their independent printed rates.
/// Cases: airlog:49.12, airlog:49.13
/// Interpretations: interp:units-0005, interp:units-0007
pub(super) fn house_rule_hq_strength(
    content: &CnaContent,
    unit: &LandUnit,
) -> Result<Option<i32>, SupplyError> {
    house_rule_hq_strength_from_toe(content, &unit.id, unit.toe.as_ref())
}
/// Source-only counterpart for public-content preflight, sharing the mutable-unit resolver.
/// Cases: airlog:49.12, airlog:52.42, land:4.46
/// Interpretations: interp:units-0005, interp:units-0006, interp:units-0007
pub(super) fn house_rule_hq_strength_from_toe(
    content: &CnaContent,
    id: &UnitId,
    toe: Option<&Toe>,
) -> Result<Option<i32>, SupplyError> {
    if not_applicable_hq_body(content, id, toe) {
        return Ok(Some(0));
    }
    let class = content
        .units
        .units
        .get(id)
        .and_then(|row| row.class.as_ref())
        .and_then(|id| content.units.classes.get(id));
    if class.is_some_and(|c| c.unit_type == "headquarters" && !c.max_toe_paren)
        && matches!(
            toe,
            Some(
                Toe::Normal(cna_content::units::NormalToe::N)
                    | Toe::Under { .. }
                    | Toe::Over { .. }
            )
        )
    {
        return toe_strength_from_toe(content, id, toe)
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
/// Interpretations: interp:airlog-0001, interp:units-0005, interp:units-0007
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
    #[cfg(test)]
    let _timing = crate::land::movement::step2_query_timing::span(
        crate::land::movement::step2_query_timing::Bucket::WithdrawDrawsTotal,
    );
    let mut next = {
        #[cfg(test)]
        let _timing = crate::land::movement::step2_query_timing::span(
            crate::land::movement::step2_query_timing::Bucket::SupplyLogisticsClone,
        );
        logistics.clone()
    };
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
    withdraw_into_state(next, unit_id, demand, draws, sources, prior)
}

pub(super) fn withdraw_into_state(
    next: &mut impl WithdrawalState,
    unit_id: Option<&UnitId>,
    demand: SupplyDemand,
    draws: &[SupplyDraw],
    sources: &BTreeMap<SupplySource, SupplyDemand>,
    prior: &BTreeMap<SupplySource, FuelTenths>,
) -> Result<(), SupplyError> {
    #[cfg(test)]
    let _detail = crate::land::movement::step2_query_timing::span(
        crate::land::movement::step2_query_timing::Bucket::Step2HistoryWithdrawalOverlay,
    );
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
                *next
                    .tank_mut(unit_id.ok_or(SupplyError::Invalid)?)
                    .ok_or(SupplyError::Invalid)? -= amount.fuel
            }
            SupplySource::PoolTank(id) => {
                if !amount.ammo.is_zero() || !amount.stores.is_zero() || !amount.water.is_zero() {
                    return Err(SupplyError::Invalid);
                }
                let tank = next.pool_tank_mut(&id).ok_or(SupplyError::Invalid)?;
                *tank = FuelTenths::new(
                    tank.get()
                        .checked_sub(amount.fuel.get())
                        .filter(|n| *n >= 0)
                        .ok_or(SupplyError::Insufficient)?,
                );
            }
            SupplySource::ReadyAmmo => {
                *next
                    .ready_ammo_mut(unit_id.ok_or(SupplyError::Invalid)?)
                    .ok_or(SupplyError::Invalid)? -= amount.ammo
            }
            source => {
                if let SupplySource::AirDump(id) = &source
                    && !next.air_dump_matches(id)
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
                // Rounded fuel credit survives source departure exactly as before.
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
                next.retire_debit(
                    &site,
                    Supplies {
                        fuel: withdrawal.get(),
                        ammo: amount.ammo.get(),
                        stores: amount.stores.get(),
                        water: amount.water.get(),
                    },
                )?;
                let stock = next.stock_mut(&site).ok_or(SupplyError::Invalid)?;
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

pub(super) trait WithdrawalState {
    fn tank_mut(&mut self, id: &UnitId) -> Option<&mut FuelTenths>;
    fn ready_ammo_mut(&mut self, id: &UnitId) -> Option<&mut AmmoPoints>;
    fn pool_tank_mut(&mut self, id: &str) -> Option<&mut FuelTenths>;
    fn air_dump_matches(&self, id: &str) -> bool;
    fn stock_mut(&mut self, site: &super::cargo_history::CargoSite) -> Option<&mut Supplies>;
    fn retire_debit(
        &mut self,
        site: &super::cargo_history::CargoSite,
        amount: Supplies,
    ) -> Result<(), SupplyError>;
}

impl WithdrawalState for LogisticsState {
    fn tank_mut(&mut self, id: &UnitId) -> Option<&mut FuelTenths> {
        self.unit_supply.get_mut(id).map(|s| &mut s.tank_fuel)
    }
    fn ready_ammo_mut(&mut self, id: &UnitId) -> Option<&mut AmmoPoints> {
        self.unit_supply.get_mut(id).map(|s| &mut s.ready_ammo)
    }
    fn pool_tank_mut(&mut self, id: &str) -> Option<&mut FuelTenths> {
        self.truck_pools
            .iter_mut()
            .find(|pool| pool.id == id)
            .map(|p| &mut p.tank_fuel)
    }
    fn air_dump_matches(&self, id: &str) -> bool {
        self.air_dumps.get(id).is_some_and(|dump| dump.id == id)
    }
    fn stock_mut(&mut self, site: &super::cargo_history::CargoSite) -> Option<&mut Supplies> {
        use super::cargo_history::CargoSite;
        match site {
            CargoSite::Unit(id) => self.unit_supply.get_mut(id).map(|s| &mut s.carried),
            CargoSite::Pool(id) => self
                .truck_pools
                .iter_mut()
                .find(|p| &p.id == id)
                .map(|p| &mut p.cargo),
            CargoSite::Dump(id) => self.dumps.get_mut(id).map(|d| &mut d.supplies),
            CargoSite::AirDump(id) => self.air_dumps.get_mut(id).map(|d| &mut d.supplies),
            _ => None,
        }
    }
    fn retire_debit(
        &mut self,
        site: &super::cargo_history::CargoSite,
        amount: Supplies,
    ) -> Result<(), SupplyError> {
        super::cargo_history::retire_debit(self, site, amount)
    }
}

// Ephemeral field writes; histories remain canonical full histories.
#[derive(Default)]
struct WithdrawalEdits {
    tanks: BTreeMap<UnitId, FuelTenths>,
    ready_ammo: BTreeMap<UnitId, AmmoPoints>,
    unit_stocks: BTreeMap<UnitId, Supplies>,
    pool_tanks: BTreeMap<usize, FuelTenths>,
    pool_stocks: BTreeMap<usize, Supplies>,
    dumps: BTreeMap<String, Supplies>,
    air_dumps: BTreeMap<String, Supplies>,
    histories: BTreeMap<super::cargo_history::CargoSite, super::cargo_history::CargoHistory>,
}

pub(super) struct WithdrawalOverlay<'a> {
    base: &'a LogisticsState,
    edits: WithdrawalEdits,
}
impl<'a> WithdrawalOverlay<'a> {
    pub(super) fn new(base: &'a LogisticsState) -> Self {
        Self {
            base,
            edits: WithdrawalEdits::default(),
        }
    }
    fn pool_index(&self, id: &str) -> Option<usize> {
        self.base.truck_pools.iter().position(|pool| pool.id == id)
    }
    pub(super) fn finish(self) -> PreparedWithdrawal {
        PreparedWithdrawal { edits: self.edits }
    }
}
pub(super) struct PreparedWithdrawal {
    edits: WithdrawalEdits,
}
impl PreparedWithdrawal {
    pub(super) fn apply(self, logistics: &mut LogisticsState) {
        // Private synchronous contract: every physical field below came from an
        // existing record, and no base record/identity can change before apply.
        for (id, value) in self.edits.tanks {
            logistics
                .unit_supply
                .get_mut(&id)
                .expect("existing withdrawal tank")
                .tank_fuel = value;
        }
        for (id, value) in self.edits.ready_ammo {
            logistics
                .unit_supply
                .get_mut(&id)
                .expect("existing withdrawal ready ammo")
                .ready_ammo = value;
        }
        for (id, value) in self.edits.unit_stocks {
            logistics
                .unit_supply
                .get_mut(&id)
                .expect("existing withdrawal unit stock")
                .carried = value;
        }
        for (index, value) in self.edits.pool_tanks {
            logistics.truck_pools[index].tank_fuel = value;
        }
        for (index, value) in self.edits.pool_stocks {
            logistics.truck_pools[index].cargo = value;
        }
        for (id, value) in self.edits.dumps {
            logistics
                .dumps
                .get_mut(&id)
                .expect("existing withdrawal dump")
                .supplies = value;
        }
        for (id, value) in self.edits.air_dumps {
            logistics
                .air_dumps
                .get_mut(&id)
                .expect("existing withdrawal air dump")
                .supplies = value;
        }
        for (site, history) in self.edits.histories {
            logistics.cargo_history.histories.insert(site, history);
        }
    }
}
impl WithdrawalState for WithdrawalOverlay<'_> {
    fn tank_mut(&mut self, id: &UnitId) -> Option<&mut FuelTenths> {
        let old = self.base.unit_supply.get(id)?.tank_fuel;
        Some(self.edits.tanks.entry(id.clone()).or_insert(old))
    }
    fn ready_ammo_mut(&mut self, id: &UnitId) -> Option<&mut AmmoPoints> {
        let old = self.base.unit_supply.get(id)?.ready_ammo;
        Some(self.edits.ready_ammo.entry(id.clone()).or_insert(old))
    }
    fn pool_tank_mut(&mut self, id: &str) -> Option<&mut FuelTenths> {
        let index = self.pool_index(id)?;
        Some(
            self.edits
                .pool_tanks
                .entry(index)
                .or_insert(self.base.truck_pools[index].tank_fuel),
        )
    }
    fn air_dump_matches(&self, id: &str) -> bool {
        self.base
            .air_dumps
            .get(id)
            .is_some_and(|dump| dump.id == id)
    }
    fn stock_mut(&mut self, site: &super::cargo_history::CargoSite) -> Option<&mut Supplies> {
        use super::cargo_history::CargoSite;
        match site {
            CargoSite::Unit(id) => {
                let old = self.base.unit_supply.get(id)?.carried;
                Some(self.edits.unit_stocks.entry(id.clone()).or_insert(old))
            }
            CargoSite::Pool(id) => {
                let index = self.pool_index(id)?;
                Some(
                    self.edits
                        .pool_stocks
                        .entry(index)
                        .or_insert(self.base.truck_pools[index].cargo),
                )
            }
            CargoSite::Dump(id) => {
                let old = self.base.dumps.get(id)?.supplies;
                Some(self.edits.dumps.entry(id.clone()).or_insert(old))
            }
            CargoSite::AirDump(id) => {
                let old = self.base.air_dumps.get(id)?.supplies;
                Some(self.edits.air_dumps.entry(id.clone()).or_insert(old))
            }
            _ => None,
        }
    }
    fn retire_debit(
        &mut self,
        site: &super::cargo_history::CargoSite,
        amount: Supplies,
    ) -> Result<(), SupplyError> {
        if !self.edits.histories.contains_key(site)
            && let Some(history) = self.base.cargo_history.histories.get(site)
        {
            self.edits.histories.insert(site.clone(), history.clone());
        }
        super::cargo_history::retire_debit_entry(self.edits.histories.get_mut(site), amount)
    }
}

// Only identities/order are indexed. No stock, history, credit or access result is cached.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum QuerySupplyLocation {
    Hex(cna_core::ids::HexId),
    OffMap(String),
}
impl QuerySupplyLocation {
    fn of(location: &Location) -> Option<Self> {
        match location {
            Location::Hex { hex } => Some(Self::Hex(hex.clone())),
            Location::OffMap { id } => Some(Self::OffMap(id.clone())),
            _ => None,
        }
    }
}

pub(crate) struct QueryFuelSources<'a> {
    content: &'a CnaContent,
    stationary: BTreeMap<(cna_protocol::Side, QuerySupplyLocation), Vec<UnitId>>,
    moving: BTreeMap<cna_protocol::Side, Vec<UnitId>>,
    dumps: BTreeMap<(cna_protocol::Side, QuerySupplyLocation), Vec<String>>,
    source_buffer: std::cell::RefCell<Vec<SupplyDraw>>,
    choice_buffer: std::cell::RefCell<Vec<SupplySource>>,
}

impl<'a> QueryFuelSources<'a> {
    // Construct after query detachment from the exact draft, once per search.
    // run(truth=false) cannot add/remove units/dumps, move stationary units,
    // change side/dump location, or insert/remove dumps. UnitSupply presence IS dynamic.
    pub(crate) fn new(content: &'a CnaContent, state: &State, movers: &[UnitId]) -> Self {
        let mut stationary = BTreeMap::<_, Vec<_>>::new();
        let mut moving = BTreeMap::<_, Vec<_>>::new();
        for (id, unit) in &state.land.units {
            if movers.contains(id) {
                moving.entry(unit.side).or_default().push(id.clone());
            } else if let Some(location) = QuerySupplyLocation::of(&unit.location) {
                stationary
                    .entry((unit.side, location))
                    .or_default()
                    .push(id.clone());
            }
        }
        let mut dumps = BTreeMap::<_, Vec<_>>::new();
        for (id, dump) in &state.logistics.dumps {
            let location = match &dump.location {
                DumpLocation::Hex { hex } => QuerySupplyLocation::Hex(hex.clone()),
                DumpLocation::OffMap { id } => QuerySupplyLocation::OffMap(id.clone()),
                DumpLocation::AwaitingSetup { .. } => continue,
            };
            dumps
                .entry((dump.side, location))
                .or_default()
                .push(id.clone());
        }
        Self {
            content,
            stationary,
            moving,
            dumps,
            source_buffer: Default::default(),
            choice_buffer: Default::default(),
        }
    }

    fn fill_sources(
        &self,
        state: &State,
        id: &UnitId,
        location: &Location,
        sources: &mut Vec<SupplyDraw>,
    ) -> Result<(), SupplyError> {
        sources.clear();
        let unit = state.land.units.get(id).ok_or(SupplyError::Invalid)?;
        if !matches!(
            unit.location,
            Location::Hex { .. } | Location::OffMap { .. }
        ) {
            return Err(SupplyError::Invalid);
        }
        let key = QuerySupplyLocation::of(location).ok_or(SupplyError::Invalid)?;
        if let Some(holdings) = state.logistics.unit_supply.get(id) {
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
        let fixed = self
            .stationary
            .get(&(unit.side, key.clone()))
            .map_or(&[][..], Vec::as_slice);
        let movers = self.moving.get(&unit.side).map_or(&[][..], Vec::as_slice);
        let mut fixed = fixed.iter().peekable();
        let mut movers = movers.iter().peekable();
        // Merge disjoint sorted rosters: canonical UnitSupply BTreeMap candidate order.
        while fixed.peek().is_some() || movers.peek().is_some() {
            let next = match (fixed.peek(), movers.peek()) {
                (Some(a), Some(b)) if a < b => fixed.next(),
                (Some(_), Some(_)) => movers.next(),
                (Some(_), None) => fixed.next(),
                (None, Some(_)) => movers.next(),
                (None, None) => None,
            }
            .expect("nonempty query roster merge");
            let Some(holdings) = state.logistics.unit_supply.get(next) else {
                continue;
            };
            let Some(carrier) = state.land.units.get(next) else {
                continue;
            };
            if carrier.side == unit.side
                && &carrier.location == location
                && truck_count(carrier)? > 0
            {
                sources.push(SupplyDraw {
                    source: SupplySource::UnitStock(next.clone()),
                    amount: stock_demand(holdings.carried)?,
                });
            }
        }
        if let Some(ids) = self.dumps.get(&(unit.side, key)) {
            for id in ids {
                let Some(dump) = state.logistics.dumps.get(id) else {
                    continue;
                };
                if dump.side == unit.side
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
        }
        // Original opaque-location filtering happens AFTER all local stock validation.
        if matches!(location, Location::OffMap { id } if !self.content.areas.locations.contains_key(id))
        {
            sources.retain(|s| {
                matches!(
                    s.source,
                    SupplySource::Tank | SupplySource::ReadyAmmo | SupplySource::UnitStock(_)
                )
            });
            return Ok(());
        }
        add_unlimited_source(self.content, unit.side, location, sources)
    }

    pub(super) fn capacities(
        &self,
        state: &State,
        id: &UnitId,
        account: &super::segment::FuelFundingAccount,
        used: &BTreeMap<SupplySource, i32>,
    ) -> Result<BTreeMap<SupplySource, SupplyDemand>, SupplyError> {
        if state.land.units.get(id).ok_or(SupplyError::Invalid)?.side != account.side {
            return Err(SupplyError::Invalid);
        }
        let mut buffer = self.source_buffer.borrow_mut();
        if let Err(error) = self.fill_sources(state, id, &account.origin, &mut buffer) {
            buffer.clear();
            return Err(error);
        }
        let sources = buffer.drain(..).map(|s| (s.source, s.amount)).collect();
        super::segment::source_capacities(sources, account, used)
    }

    pub(super) fn content_matches(&self, content: &CnaContent) -> bool {
        std::ptr::eq(self.content, content)
    }

    pub(super) fn choices(&self) -> std::cell::RefMut<'_, Vec<SupplySource>> {
        self.choice_buffer.borrow_mut()
    }
}

// SCRATCH SOURCE ONLY. Insert into owned supply.rs; export crate-private.
// Compute ONCE from the full query root BEFORE any branch. This is a conservative
// complete history write roster, not a current-edge draw/side/location filter.
pub(crate) fn query_withdrawal_history_sites(
    logistics: &LogisticsState,
) -> Vec<super::cargo_history::CargoSite> {
    use super::cargo_history::CargoSite;
    let mut sites = std::collections::BTreeSet::new();
    sites.extend(logistics.unit_supply.keys().cloned().map(CargoSite::Unit));
    sites.extend(
        logistics
            .truck_pools
            .iter()
            .map(|p| CargoSite::Pool(p.id.clone())),
    );
    sites.extend(logistics.dumps.keys().cloned().map(CargoSite::Dump));
    sites.extend(logistics.air_dumps.keys().cloned().map(CargoSite::AirDump));
    // A malformed/unmatched existing history can be retired BEFORE stock lookup
    // fails. Include it even if its physical record is absent at the root.
    sites.extend(
        logistics
            .cargo_history
            .histories
            .keys()
            .filter(|site| {
                matches!(
                    site,
                    CargoSite::Unit(_)
                        | CargoSite::Pool(_)
                        | CargoSite::Dump(_)
                        | CargoSite::AirDump(_)
                )
            })
            .cloned(),
    );
    sites.into_iter().collect()
}

// Contract: this query's fuel withdrawal does not create history entries or
// physical source identities. Carrier movement changes availability, not this
// roster; every root source is included regardless of its original location.
// If any OTHER query operation creates histories, changes source identity/map
// membership, or retires Ship/BrokenMarker histories, Land must establish its
// wider write set or retain legacy. This fuel helper is not consent to omit it.
// Capture Option<CargoHistory> at EACH node for ALL roster sites; restore before
// sibling/resumed evaluation. No next_id/motion snapshot or normalization.

const PREPARED_FUEL_ROWS: [i32; 13] = [1, 2, 3, 4, 5, 10, 15, 20, 25, 30, 35, 40, 45];

pub(super) struct PreparedFuelRate {
    whole_fifty: Option<FuelTenths>,
    cells: [Option<FuelTenths>; 13],
}

impl PreparedFuelRate {
    fn new(content: &CnaContent, rate: i32) -> Self {
        let table = &content.tables.airlog.fuel_consumption;
        Self {
            whole_fifty: table.printed(rate, 50),
            cells: PREPARED_FUEL_ROWS.map(|cp| table.printed(rate, cp)),
        }
    }

    // Same first matching column/row, mandatory 50 cell, and checked order as
    // FuelConsumption::fuel_for. A missing unused cell remains None.
    fn fuel_for(&self, cp: i32) -> Option<FuelTenths> {
        if cp < 0 {
            return None;
        }
        let remaining = cp % 50;
        let whole = self.whole_fifty?.tenths().checked_mul(cp / 50)?;
        if remaining == 0 {
            return Some(FuelTenths::new(whole));
        }
        let priced_cp = if cp < 5 {
            remaining
        } else {
            (remaining + 4) / 5 * 5
        };
        let remainder = if priced_cp == 50 {
            self.whole_fifty?
        } else {
            let index = PREPARED_FUEL_ROWS
                .iter()
                .position(|row| *row == priced_cp)?;
            self.cells[index]?
        };
        whole.checked_add(remainder.tenths()).map(FuelTenths::new)
    }

    fn cost(&self, cp: i32, count: i32) -> Result<i32, SupplyError> {
        if count < 0 {
            return Err(SupplyError::Invalid);
        }
        self.fuel_for(cp)
            .ok_or(SupplyError::Unsupported {
                case: "airlog:49.19",
            })?
            .get()
            .checked_mul(count)
            .ok_or(SupplyError::Invalid)
    }
}

struct PreparedFuelFactor {
    rate: PreparedFuelRate,
    count: i32,
}

// Holds no State, source-access cache, balances, or persistent/shared field.
// The immutable content borrow prevents dependency changes for its lifetime.
pub(crate) struct PreparedMovementFuel<'a> {
    content: &'a CnaContent,
    id: UnitId,
    represented_id: UnitId,
    toe: Option<Toe>,
    trucks: cna_content::units::Trucks,
    truck_count: i32,
    ordered_body: Vec<PreparedFuelFactor>,
    truck_rate: PreparedFuelRate,
}

impl<'a> PreparedMovementFuel<'a> {
    // Preparation never creates a game error. Unsupported composition/errors
    // are None; the original operation decides errors only if actually visited.
    /// Cases: airlog:49.12, airlog:49.13, airlog:49.19, land:4.48
    /// Interpretations: interp:airlog-0001, interp:units-0005
    pub(crate) fn new(content: &'a CnaContent, state: &State, id: &UnitId) -> Option<Self> {
        let unit = state.land.units.get(id)?;
        let oa = content.units.units.get(id)?;
        let class = oa
            .class
            .as_ref()
            .and_then(|id| content.units.classes.get(id));
        let house = house_rule_hq_strength(content, unit).ok()?;
        let mut ordered_body = Vec::new();
        let mut factor = |rate, count| {
            ordered_body.push(PreparedFuelFactor {
                rate: PreparedFuelRate::new(content, rate),
                count,
            });
        };
        match &unit.toe {
            Some(Toe::Weapons(weapons)) => {
                for point in weapons {
                    let weapon = content.units.weapons.get(&point.weapon)?;
                    factor(weapon.fuel_rate?, point.n);
                }
            }
            _ if class.is_some_and(|c| c.unit_type == "recce") => {
                factor(1, toe_strength(content, unit).ok()?.get());
            }
            _ if class.is_some_and(|c| matches!(c.unit_type.as_str(), "infantry" | "engineer")) => {
            }
            _ if class.is_some_and(|c| c.unit_type == "headquarters" && c.max_toe_paren) => {}
            _ if house.is_some() => factor(1, house?),
            _ => return None,
        }
        let truck_count = truck_count(unit).ok()?;
        Some(Self {
            content,
            id: id.clone(),
            represented_id: unit.id.clone(),
            toe: unit.toe.clone(),
            trucks: unit.trucks,
            truck_count,
            ordered_body,
            truck_rate: PreparedFuelRate::new(content, 1),
        })
    }

    pub(crate) fn id(&self) -> &UnitId {
        &self.id
    }

    pub(super) fn content(&self) -> &'a CnaContent {
        self.content
    }

    pub(super) fn matches(&self, state: &State) -> bool {
        state.land.units.get(&self.id).is_some_and(|unit| {
            unit.id == self.represented_id && unit.toe == self.toe && unit.trucks == self.trucks
        })
    }

    // Direct differential seam: None means use movement_fuel_cost, never a
    // substituted Invalid/Unsupported error. No production logging/serialization.
    #[cfg(test)]
    pub(crate) fn movement_cost(
        &self,
        state: &State,
        cp_quarters: i32,
    ) -> Option<Result<FuelTenths, SupplyError>> {
        self.matches(state)
            .then(|| self.full_cost(state, cp_quarters))
    }

    fn full_cost(&self, state: &State, cp_quarters: i32) -> Result<FuelTenths, SupplyError> {
        if cp_quarters < 0 {
            return Err(SupplyError::Invalid);
        }
        state.land.units.get(&self.id).ok_or(SupplyError::Invalid)?;
        self.content
            .units
            .units
            .get(&self.id)
            .ok_or(SupplyError::Invalid)?;
        let cp = cp_quarters / 4 + i32::from(cp_quarters % 4 != 0);
        if cp == 0 {
            return Ok(FuelTenths::ZERO);
        }
        let mut total = 0i32;
        for factor in &self.ordered_body {
            total = total
                .checked_add(factor.rate.cost(cp, factor.count)?)
                .ok_or(SupplyError::Invalid)?;
        }
        total = total
            .checked_add(self.truck_rate.cost(cp, self.truck_count)?)
            .ok_or(SupplyError::Invalid)?;
        Ok(FuelTenths::new(total))
    }

    pub(super) fn chart(&self, cp_quarters: i32) -> Result<i32, SupplyError> {
        if cp_quarters < 0 {
            return Err(SupplyError::Invalid);
        }
        if cp_quarters == 0 {
            return Ok(0);
        }
        let cp = cp_quarters / 4 + i32::from(cp_quarters % 4 != 0);
        self.truck_rate
            .fuel_for(cp)
            .map(|n| n.get())
            .ok_or(SupplyError::Unsupported {
                case: "airlog:49.19",
            })
    }

    pub(super) fn body_cost(&self, state: &State, cp: i32) -> Result<i32, SupplyError> {
        let unit = state.land.units.get(&self.id).ok_or(SupplyError::Invalid)?;
        let full = self.full_cost(state, cp)?.get();
        // Preserve full accumulation BEFORE subtraction, including checked overflow.
        let truck_charge = self
            .chart(cp)?
            .checked_mul(super::segment::truck_total(&unit.trucks)?)
            .ok_or(SupplyError::Invalid)?;
        full.checked_sub(truck_charge)
            .filter(|n| *n >= 0)
            .ok_or(SupplyError::Invalid)
    }
}

// Append to owned logistics/supply.rs. All controls are ordinary/silent, UNRUN.
#[cfg(test)]
mod step1_controls {
    use super::*;
    use crate::logistics::{
        self,
        cargo_history::{CargoHistory, CargoLot, CargoSite},
    };

    fn fixture() -> (CnaContent, State, UnitId) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani")
            .expect("fuel control content load failed");
        let s = State::new(&c).expect("fuel control State creation failed");
        let id = "it.libyan_tank_command.trivioli_2nd_regt_hq".into();
        (c, s, id)
    }

    fn compare_spend(c: &CnaContent, state: &State, id: &UnitId, cp: i32) -> State {
        let prepared = PreparedMovementFuel::new(c, state, id);
        let mut old = state.clone();
        let mut new = state.clone();
        let a = logistics::spend_segment_fuel_report(c, &mut old, id, cp);
        let (path, b) =
            logistics::spend_query_segment_fuel_report(c, &mut new, id, cp, prepared.as_ref());
        assert!(
            path == if prepared.is_some() {
                logistics::QueryFuelPath::PreparedAllSources
            } else {
                logistics::QueryFuelPath::Legacy
            },
            "prepared spending route differs"
        );
        assert!(
            a == b,
            "prepared spending Result/increment/draw order differs"
        );
        assert!(
            serde_json::to_vec(&old).expect("fuel State encoding failed")
                == serde_json::to_vec(&new).expect("fuel State encoding failed"),
            "prepared spending State bytes/presence differs"
        );
        if a.is_err() {
            assert!(
                serde_json::to_vec(&new).expect("fuel State encoding failed")
                    == serde_json::to_vec(state).expect("fuel State encoding failed"),
                "failed prepared spending was not atomic"
            );
        }
        new
    }

    #[test]
    fn step1_prepared_chart_and_composition_price_match_exact_original() {
        let (c, s, _) = fixture();
        for rate in c
            .tables
            .airlog
            .fuel_consumption
            .rates()
            .iter()
            .copied()
            .chain([i32::MAX])
        {
            let p = PreparedFuelRate::new(&c, rate);
            for cp in [
                -1,
                0,
                1,
                2,
                3,
                4,
                5,
                46,
                47,
                48,
                49,
                50,
                51,
                52,
                53,
                54,
                55,
                96,
                97,
                98,
                99,
                100,
                i32::MAX,
            ] {
                assert!(
                    p.fuel_for(cp) == c.tables.airlog.fuel_consumption.fuel_for(rate, cp),
                    "prepared chart differs in fraction/50/remainder/missing rate/overflow"
                );
            }
        }
        // Every authored composition offered by this source fixture, including
        // unsupported entries, keeps actual original error/fallback behavior.
        for id in s.land.units.keys() {
            if let Some(p) = PreparedMovementFuel::new(&c, &s, id) {
                for cp in [-1, 0, 1, 4, 5, 16, 20, 196, 200, 204, 216, i32::MAX] {
                    assert!(
                        p.movement_cost(&s, cp) == Some(movement_fuel_cost(&c, &s, id, cp)),
                        "prepared ordered body/truck/CP error differs"
                    );
                }
            }
        }
        let mut missing = PreparedFuelRate::new(&c, 1);
        missing.whole_fifty = None;
        assert!(
            missing.fuel_for(1).is_none(),
            "prepared pricing skipped mandatory 50 cell"
        );
        let mut missing_fraction = PreparedFuelRate::new(&c, 1);
        missing_fraction.cells[0] = None;
        assert!(
            missing_fraction.fuel_for(1).is_none(),
            "prepared missing fraction became default"
        );
        assert!(
            missing_fraction.fuel_for(51) == c.tables.airlog.fuel_consumption.fuel_for(1, 51),
            "prepared remainder used fractional row after 50"
        );
    }

    #[test]
    fn step1_query_spend_keeps_all_source_checks_credit_errors_and_zero_ledger() {
        let (c, s, id) = fixture();
        assert!(
            PreparedMovementFuel::new(&c, &s, &id).is_some(),
            "actual HQ has no prepared pricing descriptor"
        );
        for cp in [-1, 0, 1, 4, 5, 16, 20, 196, 200, 204, 216, i32::MAX] {
            let _ = compare_spend(&c, &s, &id, cp);
        }
        let mut branch = s.clone();
        for cp in [1, 2, 4, 8, 12, 16] {
            branch = compare_spend(&c, &branch, &id, cp);
        }
        let origin = s.land.units[&id].location.clone();
        let dump_id = s
            .logistics
            .dumps
            .iter()
            .find(|(_, d)| {
                d.active
                    && !d.dummy
                    && d.side == s.land.units[&id].side
                    && matches!((&d.location, &origin),
                (crate::state::DumpLocation::Hex { hex: a }, Location::Hex { hex: b }) if a == b)
            })
            .map(|(key, _)| key.clone())
            .expect("actual HQ has no original active dump");
        let paid = compare_spend(&c, &s, &id, 1);
        assert!(
            paid.logistics.fuel_segments.get(&id).is_some_and(|l| l
                .draws
                .iter()
                .any(|d| d.source == SupplySource::Dump(dump_id.clone()) && d.fuel.get() > 0)),
            "actual HQ direct control did not use real dump funding"
        );
        let mut departed = paid.clone();
        departed.logistics.dumps.remove(&dump_id);
        let _ = compare_spend(&c, &departed, &id, 2);
        let _ = compare_spend(&c, &departed, &id, 200);

        for fault in 0..7 {
            let mut bad = s.clone();
            let u = bad.land.units.get_mut(&id).unwrap();
            match fault {
                0 => {
                    bad.logistics
                        .unit_supply
                        .entry(id.clone())
                        .or_default()
                        .ready_ammo = AmmoPoints::new(-1);
                }
                1 => {
                    bad.logistics
                        .unit_supply
                        .entry(id.clone())
                        .or_default()
                        .tank_fuel = FuelTenths::new(-1);
                }
                2 => {
                    u.trucks.light = 1;
                    bad.logistics
                        .unit_supply
                        .entry(id.clone())
                        .or_default()
                        .carried
                        .ammo = -1;
                }
                3 => {
                    u.trucks.light = 1;
                    bad.logistics
                        .unit_supply
                        .entry(id.clone())
                        .or_default()
                        .carried
                        .stores = -1;
                }
                4 => {
                    u.trucks.light = 1;
                    bad.logistics
                        .unit_supply
                        .entry(id.clone())
                        .or_default()
                        .carried
                        .water = -1;
                }
                5 => {
                    u.trucks.light = -1;
                }
                _ => {
                    bad.logistics.dumps.get_mut(&dump_id).unwrap().supplies.fuel = -1;
                }
            }
            let _ = compare_spend(&c, &bad, &id, 4);
            let zero = compare_spend(&c, &bad, &id, 0);
            // Both paths decide original zero semantics before positive-source validation.
            assert!(
                zero.land.units.len() == bad.land.units.len(),
                "zero spend changed units"
            );
        }
        if let Some(p) = PreparedMovementFuel::new(&c, &s, &id) {
            let mut changed = s.clone();
            changed.land.units.get_mut(&id).unwrap().trucks.light += 1;
            let (path, _) =
                logistics::spend_query_segment_fuel_report(&c, &mut changed, &id, 4, Some(&p));
            assert!(
                path == logistics::QueryFuelPath::Legacy,
                "changed composition did not fallback"
            );
            let mut absent = s.clone();
            absent.land.units.remove(&id);
            let (path, _) =
                logistics::spend_query_segment_fuel_report(&c, &mut absent, &id, 4, Some(&p));
            assert!(
                path == logistics::QueryFuelPath::Legacy,
                "missing unit did not fallback"
            );
            let (another_content, _, _) = fixture();
            let mut same_state = s.clone();
            let (path, _) = logistics::spend_query_segment_fuel_report(
                &another_content,
                &mut same_state,
                &id,
                4,
                Some(&p),
            );
            assert!(
                path == logistics::QueryFuelPath::Legacy,
                "different content did not fallback"
            );
            let mut changed_toe = s.clone();
            changed_toe.land.units.get_mut(&id).unwrap().toe = None;
            let (path, _) =
                logistics::spend_query_segment_fuel_report(&c, &mut changed_toe, &id, 4, Some(&p));
            assert!(
                path == logistics::QueryFuelPath::Legacy,
                "changed TOE did not fallback"
            );
        }
    }

    #[test]
    fn step1_query_spend_retains_shared_cohort_history_accounts_and_branch_balances() {
        let (c, mut root, id) = fixture();
        root.land.units.get_mut(&id).unwrap().trucks.light = 2;
        let root = compare_spend(&c, &root, &id, 4);
        let account = root
            .logistics
            .fuel_accounts
            .get(&id)
            .expect("shared control lacks original funding account")
            .clone();
        let other = root
            .land
            .units
            .keys()
            .find(|other| {
                *other != &id && root.land.units[*other].side == root.land.units[&id].side
            })
            .expect("shared control lacks a same-side identity")
            .clone();
        let mut shared = root.clone();
        let mut ledger = shared.logistics.fuel_segments[&id].clone();
        let first = ledger
            .cohorts
            .first()
            .expect("shared control has no physical cohort")
            .clone();
        assert!(
            first.count == 2,
            "shared control physical cohort count differs"
        );
        let mut earlier = first.clone();
        earlier.count = 1;
        earlier.cp_quarters = 1;
        let mut later = first;
        later.count = 1;
        later.id.push_str("-step1-split");
        later.account = other.clone();
        later.cp_quarters = 3;
        ledger.cohorts = vec![earlier, later];
        shared.logistics.fuel_segments.insert(id.clone(), ledger);
        shared
            .logistics
            .fuel_accounts
            .insert(other.clone(), account);
        let mut a = compare_spend(&c, &shared, &id, 8);
        a = compare_spend(&c, &a, &id, 12);
        let _ = compare_spend(&c, &a, &id, 16);
        // Repeated sibling restoration uses exact root balances/account bytes.
        let _ = compare_spend(&c, &shared, &id, 16);
        let mut absent = shared.clone();
        absent.logistics.fuel_accounts.remove(&other);
        let _ = compare_spend(&c, &absent, &id, 8);
        let mut foreign = shared.clone();
        foreign
            .logistics
            .fuel_accounts
            .get_mut(&other)
            .unwrap()
            .side = root.land.units[&id].side.opponent();
        let _ = compare_spend(&c, &foreign, &id, 8);
        let mut wrong_origin = shared.clone();
        wrong_origin
            .logistics
            .fuel_accounts
            .get_mut(&other)
            .unwrap()
            .origin = Location::Eliminated;
        let _ = compare_spend(&c, &wrong_origin, &id, 8);
        let mut wrong_totals = shared;
        wrong_totals
            .logistics
            .fuel_accounts
            .get_mut(&other)
            .unwrap()
            .paid_cost = FuelTenths::new(i32::MAX);
        let _ = compare_spend(&c, &wrong_totals, &id, 8);
    }

    fn compare_withdraw(
        logistics: &LogisticsState,
        id: &UnitId,
        demand: SupplyDemand,
        draws: &[SupplyDraw],
        sources: &BTreeMap<SupplySource, SupplyDemand>,
        prior: &BTreeMap<SupplySource, FuelTenths>,
    ) {
        let mut old = logistics.clone();
        let a = withdraw_into(&mut old, Some(id), demand, draws, sources, prior);
        let mut new = logistics.clone();
        let mut overlay = WithdrawalOverlay::new(&new);
        let b = withdraw_into_state(&mut overlay, Some(id), demand, draws, sources, prior);
        if b.is_ok() {
            overlay.finish().apply(&mut new);
        }
        assert!(a == b, "withdrawal accessor Result differs");
        let expected = if a.is_ok() { &old } else { logistics };
        assert!(
            serde_json::to_vec(expected).expect("withdrawal encoding failed")
                == serde_json::to_vec(&new).expect("withdrawal encoding failed"),
            "overlay commit/late failure bytes/presence differs"
        );
    }

    #[test]
    fn step1_withdrawal_overlay_matches_all_source_variants_and_late_errors() {
        let (_, s, id) = fixture();
        let stage = logistics::water::WaterStage::current(&s);
        let side = s.land.units[&id].side;
        let mut l = s.logistics;
        l.unit_supply.insert(
            id.clone(),
            crate::state::UnitSupply {
                tank_fuel: FuelTenths::new(40),
                ready_ammo: AmmoPoints::new(4),
                carried: Supplies {
                    fuel: 4,
                    ammo: 4,
                    stores: 4,
                    water: 4,
                },
                ..Default::default()
            },
        );
        let dump_id = l
            .dumps
            .keys()
            .next()
            .expect("withdrawal fixture has no dump")
            .clone();
        l.dumps.get_mut(&dump_id).unwrap().supplies = Supplies {
            fuel: 4,
            ammo: 4,
            stores: 4,
            water: 4,
        };
        let air = "__step1_air_source__".to_string();
        l.air_dumps.insert(
            air.clone(),
            super::super::air_supply::AirDump {
                id: air.clone(),
                facility: crate::air::facilities::FacilityId("step1-control".into()),
                side,
                supplies: Supplies {
                    fuel: 4,
                    ammo: 4,
                    stores: 4,
                    water: 4,
                },
            },
        );
        let pool = l
            .truck_pools
            .first()
            .expect("withdrawal fixture has no pool")
            .id
            .clone();
        l.truck_pools[0].tank_fuel = FuelTenths::new(40);
        l.truck_pools[0].cargo = Supplies {
            fuel: 4,
            ammo: 4,
            stores: 4,
            water: 4,
        };
        let duplicate = l.truck_pools[0].clone();
        l.truck_pools.push(duplicate);
        let sources = [
            SupplySource::Unlimited,
            SupplySource::Tank,
            SupplySource::ReadyAmmo,
            SupplySource::UnitStock(id.clone()),
            SupplySource::Dump(dump_id.clone()),
            SupplySource::PoolTank(pool.clone()),
            SupplySource::PoolStock(pool.clone()),
            SupplySource::AirDump(air.clone()),
        ];
        for source in sources {
            let demand = if source == SupplySource::ReadyAmmo {
                SupplyDemand {
                    ammo: AmmoPoints::new(1),
                    ..Default::default()
                }
            } else {
                SupplyDemand {
                    fuel: FuelTenths::new(1),
                    ..Default::default()
                }
            };
            let capacities = BTreeMap::from([(source.clone(), demand)]);
            let draw = SupplyDraw {
                source: source.clone(),
                amount: demand,
            };
            compare_withdraw(
                &l,
                &id,
                demand,
                std::slice::from_ref(&draw),
                &capacities,
                &BTreeMap::new(),
            );
            let mut staged = WithdrawalOverlay::new(&l);
            for _ in 0..2 {
                let _ = withdraw_into_state(
                    &mut staged,
                    Some(&id),
                    demand,
                    std::slice::from_ref(&draw),
                    &capacities,
                    &BTreeMap::new(),
                );
            }
            // Multiple operations share one staged record, exactly as the old clone.
            let mut old = l.clone();
            for _ in 0..2 {
                let _ = withdraw_into(
                    &mut old,
                    Some(&id),
                    demand,
                    std::slice::from_ref(&draw),
                    &capacities,
                    &BTreeMap::new(),
                );
            }
            let mut new = l.clone();
            staged.finish().apply(&mut new);
            assert!(
                serde_json::to_vec(&old).unwrap() == serde_json::to_vec(&new).unwrap(),
                "repeated source draws did not share the staged record"
            );
        }
        let site = CargoSite::Dump(dump_id.clone());
        let history = CargoHistory {
            stage,
            lots: vec![CargoLot {
                id: "step1-lot".into(),
                goods: Supplies {
                    fuel: 4,
                    ..Default::default()
                },
                spent_cp_quarters: 0,
                ceiling_cp_quarters: 100,
                continuous_first_line: true,
            }],
        };
        l.cargo_history.histories.insert(site.clone(), history);
        let demand = SupplyDemand {
            fuel: FuelTenths::new(1),
            ..Default::default()
        };
        let source = SupplySource::Dump(dump_id.clone());
        let capacities = BTreeMap::from([(source.clone(), demand)]);
        let draws = [SupplyDraw {
            source: source.clone(),
            amount: demand,
        }];
        compare_withdraw(&l, &id, demand, &draws, &capacities, &BTreeMap::new());
        l.dumps.remove(&dump_id); // history retires BEFORE missing physical stock error
        compare_withdraw(&l, &id, demand, &draws, &capacities, &BTreeMap::new());
        let prior = BTreeMap::from([(source.clone(), FuelTenths::new(1))]);
        compare_withdraw(&l, &id, demand, &draws, &capacities, &prior); // rounded credit, no fresh stock
        l.cargo_history.histories.get_mut(&site).unwrap().lots[0]
            .goods
            .fuel = -1;
        compare_withdraw(&l, &id, demand, &draws, &capacities, &BTreeMap::new());
        l.cargo_history
            .histories
            .get_mut(&site)
            .unwrap()
            .lots
            .clear();
        compare_withdraw(&l, &id, demand, &draws, &capacities, &BTreeMap::new());
        l.cargo_history.histories.remove(&site);
        compare_withdraw(&l, &id, demand, &draws, &capacities, &BTreeMap::new());
        // A real first source debit precedes a late missing second source.
        // Original caller discards the clone; the overlay must discard both edits.
        let total = SupplyDemand {
            fuel: FuelTenths::new(2),
            ..Default::default()
        };
        let first = SupplySource::Tank;
        let missing = SupplySource::Dump("__step1_missing_second__".into());
        compare_withdraw(
            &l,
            &id,
            total,
            &[
                SupplyDraw {
                    source: first.clone(),
                    amount: demand,
                },
                SupplyDraw {
                    source: missing.clone(),
                    amount: demand,
                },
            ],
            &BTreeMap::from([(first, demand), (missing, demand)]),
            &BTreeMap::new(),
        );
        l.air_dumps.get_mut(&air).unwrap().id = "mismatch".into();
        let source = SupplySource::AirDump(air);
        compare_withdraw(
            &l,
            &id,
            demand,
            &[SupplyDraw {
                source: source.clone(),
                amount: demand,
            }],
            &BTreeMap::from([(source, demand)]),
            &BTreeMap::new(),
        );
    }
    fn compare_indexed_spend(c: &CnaContent, state: &State, id: &UnitId, cp: i32) -> State {
        let pricing = PreparedMovementFuel::new(c, state, id);
        let sources = QueryFuelSources::new(c, state, std::slice::from_ref(id));
        let mut old = state.clone();
        let mut new = state.clone();
        let a = logistics::spend_query_segment_fuel_report(c, &mut old, id, cp, pricing.as_ref()).1;
        let b = logistics::spend_indexed_query_segment_fuel_report(
            c,
            &mut new,
            id,
            cp,
            pricing.as_ref(),
            &sources,
        )
        .1;
        assert_eq!(a, b, "indexed fuel Result/draw order differs");
        assert_eq!(
            serde_json::to_vec(&old).unwrap(),
            serde_json::to_vec(&new).unwrap()
        );
        if b.is_err() {
            assert_eq!(
                serde_json::to_vec(state).unwrap(),
                serde_json::to_vec(&new).unwrap()
            );
        }
        new
    }

    #[test]
    fn step2_indexed_sources_keep_dynamic_membership_error_order_and_reused_buffers() {
        let (c, s, id) = fixture();
        // Mark every unit as mobile in this isolated source-index control: this
        // tests movement into/out of origins while retaining full canonical key order.
        let movers: Vec<_> = s.land.units.keys().cloned().collect();
        let roster = QueryFuelSources::new(&c, &s, &movers);
        let origin = s
            .land
            .units
            .get(&id)
            .expect("source-index fixture missing consuming unit")
            .location
            .clone();
        let before = serde_json::to_vec(&s).expect("source-index root State encoding failed");
        assert!(
            !s.logistics.unit_supply.contains_key(&id),
            "source-index fixture requires absent consuming-unit holdings"
        );
        let mut buffer = Vec::new();
        let mut baseline_result_and_buffer = None;
        let mut absent_result_and_buffer = None;
        for variant in 0..10 {
            let mut current = s.clone();
            match variant {
                1 => {
                    current
                        .logistics
                        .unit_supply
                        .entry(id.clone())
                        .or_default()
                        .ready_ammo = AmmoPoints::new(-1)
                }
                2 => {
                    current.logistics.unit_supply.remove(&id);
                }
                3 => {
                    current
                        .land
                        .units
                        .get_mut(&id)
                        .expect("source-index invalid-location variant missing consuming unit")
                        .location = Location::Eliminated;
                }
                4 => {
                    for dump in current.logistics.dumps.values_mut() {
                        dump.active = false;
                    }
                }
                5 => {
                    for unit in current.land.units.values_mut() {
                        unit.location = origin.clone();
                    }
                }
                6 => {
                    for unit in current.land.units.values_mut() {
                        unit.location = Location::Eliminated;
                    }
                }
                8 => {
                    current.logistics.unit_supply.entry(id.clone()).or_default();
                }
                _ => {}
            }
            let expected = available_sources_at_with_content(&c, &current, &id, &origin);
            let actual = roster
                .fill_sources(&current, &id, &origin, &mut buffer)
                .map(|()| buffer.clone());
            assert_eq!(actual, expected);
            if let Ok(sources) = &expected {
                assert_eq!(
                    &buffer, sources,
                    "indexed source buffer content/order differs"
                );
            }
            match variant {
                0 => baseline_result_and_buffer = Some((expected.clone(), buffer.clone())),
                1 => {
                    assert_eq!(actual, Err(SupplyError::Invalid));
                    assert!(
                        buffer.is_empty(),
                        "negative-present error retained source buffer"
                    );
                }
                2 => {
                    assert!(!current.logistics.unit_supply.contains_key(&id));
                    absent_result_and_buffer = Some((expected.clone(), buffer.clone()));
                }
                // Retain the original baseline AFTER invalid-location variant 6,
                // and repeat it after the new present-default variant 8.
                7 | 9 => {
                    let (baseline_result, baseline_buffer) = baseline_result_and_buffer
                        .as_ref()
                        .expect("source-index baseline result/buffer witness missing");
                    assert_eq!(&expected, baseline_result);
                    assert_eq!(&buffer, baseline_buffer);
                }
                8 => {
                    assert_eq!(
                        current.logistics.unit_supply.get(&id),
                        Some(&crate::state::UnitSupply::default()),
                        "source-index present-default fixture branch is not default holdings"
                    );
                    let present = expected
                        .as_ref()
                        .expect("source-index present-default canonical availability unavailable");
                    let (absent_result, absent_buffer) = absent_result_and_buffer
                        .as_ref()
                        .expect("source-index absent result/buffer witness missing");
                    let absent = absent_result
                        .as_ref()
                        .expect("source-index absent canonical availability unavailable");
                    assert_eq!(absent, absent_buffer);
                    assert_eq!(
                        present
                            .get(..2)
                            .expect("source-index present-default missing two leading sources"),
                        &[
                            SupplyDraw {
                                source: SupplySource::Tank,
                                amount: SupplyDemand::default()
                            },
                            SupplyDraw {
                                source: SupplySource::ReadyAmmo,
                                amount: SupplyDemand::default()
                            },
                        ]
                    );
                    assert_eq!(
                        &present[2..],
                        absent.as_slice(),
                        "canonical presence witness differs beyond two leading zero sources"
                    );
                }
                _ => {}
            }
        }
        // Full actual rollback after success/error/zero and sibling ABBA reuse.
        let pricing = PreparedMovementFuel::new(&c, &s, &id);
        for cp in [4, -1, 0, 16, 4, 4, 16, 0] {
            let mut old = s.clone();
            let mut new = s.clone();
            let a =
                logistics::spend_query_segment_fuel_report(&c, &mut old, &id, cp, pricing.as_ref())
                    .1;
            let b = logistics::spend_indexed_query_segment_fuel_report(
                &c,
                &mut new,
                &id,
                cp,
                pricing.as_ref(),
                &roster,
            )
            .1;
            assert_eq!(a, b);
            assert_eq!(
                serde_json::to_vec(&old).expect("source-index old State encoding failed"),
                serde_json::to_vec(&new).expect("source-index indexed State encoding failed")
            );
        }
        assert_eq!(
            serde_json::to_vec(&s).expect("source-index final root State encoding failed"),
            before
        );
    }
    #[test]
    fn step2_indexed_query_spend_keeps_all_source_checks_credit_errors_and_zero_ledger() {
        let (c, s, id) = fixture();
        assert!(
            PreparedMovementFuel::new(&c, &s, &id).is_some(),
            "actual HQ has no prepared pricing descriptor"
        );
        for cp in [-1, 0, 1, 4, 5, 16, 20, 196, 200, 204, 216, i32::MAX] {
            let _ = compare_indexed_spend(&c, &s, &id, cp);
        }
        let mut branch = s.clone();
        for cp in [1, 2, 4, 8, 12, 16] {
            branch = compare_indexed_spend(&c, &branch, &id, cp);
        }
        let origin = s.land.units[&id].location.clone();
        let dump_id = s
            .logistics
            .dumps
            .iter()
            .find(|(_, d)| {
                d.active
                    && !d.dummy
                    && d.side == s.land.units[&id].side
                    && matches!((&d.location, &origin),
                (crate::state::DumpLocation::Hex { hex: a }, Location::Hex { hex: b }) if a == b)
            })
            .map(|(key, _)| key.clone())
            .expect("actual HQ has no original active dump");
        let paid = compare_indexed_spend(&c, &s, &id, 1);
        assert!(
            paid.logistics.fuel_segments.get(&id).is_some_and(|l| l
                .draws
                .iter()
                .any(|d| d.source == SupplySource::Dump(dump_id.clone()) && d.fuel.get() > 0)),
            "actual HQ direct control did not use real dump funding"
        );
        let mut departed = paid.clone();
        departed.logistics.dumps.remove(&dump_id);
        let _ = compare_indexed_spend(&c, &departed, &id, 2);
        let _ = compare_indexed_spend(&c, &departed, &id, 200);

        for fault in 0..7 {
            let mut bad = s.clone();
            let u = bad.land.units.get_mut(&id).unwrap();
            match fault {
                0 => {
                    bad.logistics
                        .unit_supply
                        .entry(id.clone())
                        .or_default()
                        .ready_ammo = AmmoPoints::new(-1);
                }
                1 => {
                    bad.logistics
                        .unit_supply
                        .entry(id.clone())
                        .or_default()
                        .tank_fuel = FuelTenths::new(-1);
                }
                2 => {
                    u.trucks.light = 1;
                    bad.logistics
                        .unit_supply
                        .entry(id.clone())
                        .or_default()
                        .carried
                        .ammo = -1;
                }
                3 => {
                    u.trucks.light = 1;
                    bad.logistics
                        .unit_supply
                        .entry(id.clone())
                        .or_default()
                        .carried
                        .stores = -1;
                }
                4 => {
                    u.trucks.light = 1;
                    bad.logistics
                        .unit_supply
                        .entry(id.clone())
                        .or_default()
                        .carried
                        .water = -1;
                }
                5 => {
                    u.trucks.light = -1;
                }
                _ => {
                    bad.logistics.dumps.get_mut(&dump_id).unwrap().supplies.fuel = -1;
                }
            }
            let _ = compare_indexed_spend(&c, &bad, &id, 4);
            let zero = compare_indexed_spend(&c, &bad, &id, 0);
            // Both paths decide original zero semantics before positive-source validation.
            assert!(
                zero.land.units.len() == bad.land.units.len(),
                "zero spend changed units"
            );
        }
        if let Some(p) = PreparedMovementFuel::new(&c, &s, &id) {
            let mut changed = s.clone();
            changed.land.units.get_mut(&id).unwrap().trucks.light += 1;
            let (path, _) =
                logistics::spend_query_segment_fuel_report(&c, &mut changed, &id, 4, Some(&p));
            assert!(
                path == logistics::QueryFuelPath::Legacy,
                "changed composition did not fallback"
            );
            let mut absent = s.clone();
            absent.land.units.remove(&id);
            let (path, _) =
                logistics::spend_query_segment_fuel_report(&c, &mut absent, &id, 4, Some(&p));
            assert!(
                path == logistics::QueryFuelPath::Legacy,
                "missing unit did not fallback"
            );
            let (another_content, _, _) = fixture();
            let mut same_state = s.clone();
            let (path, _) = logistics::spend_query_segment_fuel_report(
                &another_content,
                &mut same_state,
                &id,
                4,
                Some(&p),
            );
            assert!(
                path == logistics::QueryFuelPath::Legacy,
                "different content did not fallback"
            );
            let mut changed_toe = s.clone();
            changed_toe.land.units.get_mut(&id).unwrap().toe = None;
            let (path, _) =
                logistics::spend_query_segment_fuel_report(&c, &mut changed_toe, &id, 4, Some(&p));
            assert!(
                path == logistics::QueryFuelPath::Legacy,
                "changed TOE did not fallback"
            );
        }
    }

    #[test]
    fn step2_indexed_query_spend_retains_shared_cohort_history_accounts_and_branch_balances() {
        let (c, mut root, id) = fixture();
        root.land.units.get_mut(&id).unwrap().trucks.light = 2;
        let root = compare_indexed_spend(&c, &root, &id, 4);
        let account = root
            .logistics
            .fuel_accounts
            .get(&id)
            .expect("shared control lacks original funding account")
            .clone();
        let other = root
            .land
            .units
            .keys()
            .find(|other| {
                *other != &id && root.land.units[*other].side == root.land.units[&id].side
            })
            .expect("shared control lacks a same-side identity")
            .clone();
        let mut shared = root.clone();
        let mut ledger = shared.logistics.fuel_segments[&id].clone();
        let first = ledger
            .cohorts
            .first()
            .expect("shared control has no physical cohort")
            .clone();
        assert!(
            first.count == 2,
            "shared control physical cohort count differs"
        );
        let mut earlier = first.clone();
        earlier.count = 1;
        earlier.cp_quarters = 1;
        let mut later = first;
        later.count = 1;
        later.id.push_str("-step1-split");
        later.account = other.clone();
        later.cp_quarters = 3;
        ledger.cohorts = vec![earlier, later];
        shared.logistics.fuel_segments.insert(id.clone(), ledger);
        shared
            .logistics
            .fuel_accounts
            .insert(other.clone(), account);
        let mut a = compare_indexed_spend(&c, &shared, &id, 8);
        a = compare_indexed_spend(&c, &a, &id, 12);
        let _ = compare_indexed_spend(&c, &a, &id, 16);
        // Repeated sibling restoration uses exact root balances/account bytes.
        let _ = compare_indexed_spend(&c, &shared, &id, 16);
        let mut absent = shared.clone();
        absent.logistics.fuel_accounts.remove(&other);
        let _ = compare_indexed_spend(&c, &absent, &id, 8);
        let mut foreign = shared.clone();
        foreign
            .logistics
            .fuel_accounts
            .get_mut(&other)
            .unwrap()
            .side = root.land.units[&id].side.opponent();
        let _ = compare_indexed_spend(&c, &foreign, &id, 8);
        let mut wrong_origin = shared.clone();
        wrong_origin
            .logistics
            .fuel_accounts
            .get_mut(&other)
            .unwrap()
            .origin = Location::Eliminated;
        let _ = compare_indexed_spend(&c, &wrong_origin, &id, 8);
        let mut wrong_totals = shared;
        wrong_totals
            .logistics
            .fuel_accounts
            .get_mut(&other)
            .unwrap()
            .paid_cost = FuelTenths::new(i32::MAX);
        let _ = compare_indexed_spend(&c, &wrong_totals, &id, 8);
    }

    #[test]
    fn step2_indexed_sources_keep_opaque_dump_faults_exclusion_and_new_query_side_changes() {
        let (c, root, id) = fixture();
        let origin = root.land.units[&id].location.clone();
        let owner = root.land.units[&id].side;
        let dump = root.logistics.dumps.iter().find(|(_,d)| d.side == owner && d.active && !d.dummy
            && matches!((&d.location,&origin), (DumpLocation::Hex {hex:a}, Location::Hex {hex:b}) if a==b))
            .map(|(id,_)| id.clone()).expect("source control has no actual local dump");
        let moving: Vec<_> = root.land.units.keys().cloned().collect();
        for variant in 0..5 {
            let mut current = root.clone();
            let origin = if variant == 0 {
                let opaque = Location::OffMap {
                    id: "step2-control-opaque".into(),
                };
                current.land.units.get_mut(&id).unwrap().location = opaque.clone();
                current.logistics.dumps.get_mut(&dump).unwrap().location = DumpLocation::OffMap {
                    id: "step2-control-opaque".into(),
                };
                current
                    .logistics
                    .dumps
                    .get_mut(&dump)
                    .unwrap()
                    .supplies
                    .ammo = -1;
                opaque
            } else {
                match variant {
                    1 => current.logistics.dumps.get_mut(&dump).unwrap().dummy = true,
                    2 => {
                        current
                            .logistics
                            .dumps
                            .get_mut(&dump)
                            .unwrap()
                            .supplies
                            .water = -1
                    }
                    3 => current.land.units.get_mut(&id).unwrap().side = owner.opponent(),
                    _ => {
                        current.logistics.dumps.get_mut(&dump).unwrap().side = owner.opponent();
                    }
                }
                origin.clone()
            };
            // Side/location/identity changes outside run(false) start a NEW query index.
            let index = QueryFuelSources::new(&c, &current, &moving);
            let expected = available_sources_at_with_content(&c, &current, &id, &origin);
            let mut buffer = Vec::new();
            let actual = index
                .fill_sources(&current, &id, &origin, &mut buffer)
                .map(|()| buffer.clone());
            assert_eq!(actual, expected);
            if variant == 0 {
                assert_eq!(actual, Err(SupplyError::Invalid));
            }
        }
    }
}

#[cfg(test)]
mod step2_pending_dump_controls {
    use super::*;

    #[test]
    fn step2_indexed_sources_exclude_awaiting_setup_with_located_dump_control() {
        let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani")
            .expect("pending dump control content load failed");
        let root = State::new(&content).expect("pending dump control State creation failed");
        let id: UnitId = "it.libyan_tank_command.trivioli_2nd_regt_hq".into();
        let origin = root.land.units[&id].location.clone();
        let owner = root.land.units[&id].side;
        let dump_id = root
            .logistics
            .dumps
            .iter()
            .find(|(_, dump)| {
                dump.side == owner
                    && dump.active
                    && !dump.dummy
                    && dump.supplies.fuel > 0
                    && matches!((&dump.location, &origin),
                    (DumpLocation::Hex { hex: a }, Location::Hex { hex: b }) if a == b)
            })
            .map(|(key, _)| key.clone())
            .expect("actual HQ has no populated located dump");
        let located_dump = root.logistics.dumps[&dump_id].clone();
        let hex = match &located_dump.location {
            DumpLocation::Hex { hex } => hex.clone(),
            _ => panic!("located dump fixture selection differed"),
        };
        let root_bytes = serde_json::to_vec(&root).unwrap();
        let located_result = available_sources_at_with_content(&content, &root, &id, &origin);
        let located_index = QueryFuelSources::new(&content, &root, std::slice::from_ref(&id));
        let mut located_buffer = Vec::new();
        let indexed_located = located_index
            .fill_sources(&root, &id, &origin, &mut located_buffer)
            .map(|()| located_buffer.clone());
        assert_eq!(
            indexed_located, located_result,
            "complete located Result/ordered draws differ"
        );
        let mut expected_pending = located_result.expect("located canonical source control failed");
        assert!(
            expected_pending
                .iter()
                .any(|draw| draw.source == SupplySource::Dump(dump_id.clone())),
            "located canonical control omitted the actual populated dump"
        );
        assert!(
            located_index
                .dumps
                .values()
                .flatten()
                .any(|key| key == &dump_id),
            "located index omitted the actual dump identity"
        );
        assert_eq!(
            serde_json::to_vec(&root).unwrap(),
            root_bytes,
            "located source reads changed State"
        );

        let mut pending = root.clone();
        pending.logistics.dumps.get_mut(&dump_id).unwrap().location = DumpLocation::AwaitingSetup {
            placement: cna_content::scenario::Placement::Hex { hex },
        };
        let pending_dump = &pending.logistics.dumps[&dump_id];
        assert_eq!(pending_dump.side, located_dump.side);
        assert_eq!(pending_dump.active, located_dump.active);
        assert_eq!(pending_dump.dummy, located_dump.dummy);
        assert_eq!(pending_dump.supplies, located_dump.supplies);
        assert!(
            pending_dump.side == owner
                && pending_dump.active
                && !pending_dump.dummy
                && pending_dump.supplies.fuel > 0,
            "pending control lost its actual populated eligible fields"
        );
        let pending_bytes = serde_json::to_vec(&pending).unwrap();
        let pending_result = available_sources_at_with_content(&content, &pending, &id, &origin);
        let pending_index = QueryFuelSources::new(&content, &pending, std::slice::from_ref(&id));
        assert!(
            !pending_index
                .dumps
                .values()
                .flatten()
                .any(|key| key == &dump_id),
            "pending dump entered the fixed location roster"
        );
        let mut pending_buffer = Vec::new();
        let indexed_pending = pending_index
            .fill_sources(&pending, &id, &origin, &mut pending_buffer)
            .map(|()| pending_buffer.clone());
        assert_eq!(
            indexed_pending, pending_result,
            "complete pending Result/ordered draws differ"
        );
        expected_pending.retain(|draw| draw.source != SupplySource::Dump(dump_id.clone()));
        assert_eq!(
            pending_result,
            Ok(expected_pending),
            "pending canonical result differs beyond removal of the located dump"
        );
        assert_eq!(
            serde_json::to_vec(&pending).unwrap(),
            pending_bytes,
            "pending source reads changed State"
        );
        assert_eq!(
            serde_json::to_vec(&root).unwrap(),
            root_bytes,
            "pending control changed original State"
        );
    }
}
