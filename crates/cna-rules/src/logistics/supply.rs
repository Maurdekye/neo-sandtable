//! Costing and transactional withdrawals from existing friendly holdings.

use std::collections::BTreeMap;

use cna_content::scenario::Supplies;
use cna_content::units::Toe;
use cna_core::ids::UnitId;
use cna_core::quantity::{AmmoPoints, FuelTenths, StoresPoints, ToeStrengthPoints, WaterPoints};
use cna_tables::airlog::supply::{AmmoAction, AmmoCost, AmmoMode};
use serde::{Deserialize, Serialize};

use crate::content::CnaContent;
use crate::state::{DumpLocation, LandUnit, State};

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
    /// The consuming unit's tanks; other units' tanks require siphoning.
    Tank,
    /// The consuming unit's ready ammunition.
    ReadyAmmo,
    /// First-line cargo belonging to a friendly unit in the same hex.
    UnitStock(UnitId),
    /// An active, real, friendly dump in the same hex.
    Dump(String),
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
    /// Required content or a separate rule procedure is not available yet.
    Unsupported {
        case: &'static str,
    },
}

impl SupplyDemand {
    fn valid(self) -> bool {
        self.fuel.get() >= 0
            && self.ammo.get() >= 0
            && self.stores.get() >= 0
            && self.water.get() >= 0
    }

    fn checked_add(self, other: Self) -> Result<Self, SupplyError> {
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
/// Cases: airlog:49.12, airlog:50.13
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
        Some(Toe::Under { under }) if *under >= 0 => {
            max()?.checked_sub(*under).ok_or(SupplyError::Invalid)?
        }
        Some(Toe::Over { over }) if *over >= 0 => {
            max()?.checked_add(*over).ok_or(SupplyError::Invalid)?
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

/// Fuel for one movement segment, including the unit's first-line trucks.
/// Quarter CP are rounded up to whole CP before looking up the chart; all vehicle
/// costs are added exactly before a source draw is rounded. Non-movement CP must
/// be excluded by the movement caller. Special patrols need their own procedure.
/// Cases: airlog:49.12, airlog:49.13
/// Interpretations: interp:airlog-0001
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
        .and_then(|id| content.units.classes.get(id))
        .ok_or(SupplyError::Unsupported { case: "land:4.46" })?;
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
        _ if class.unit_type == "recce" => {
            total = cost(1, toe_strength(content, unit)?.get())?;
        }
        _ if matches!(class.unit_type.as_str(), "infantry" | "engineer") => {}
        _ if class.unit_type == "headquarters" && class.max_toe_paren => {}
        _ => {
            return Err(SupplyError::Unsupported {
                case: "airlog:49.12",
            });
        }
    }
    let trucks = unit
        .trucks
        .light
        .checked_add(unit.trucks.medium)
        .and_then(|n| n.checked_add(unit.trucks.heavy))
        .ok_or(SupplyError::Invalid)?;
    if unit.trucks.light < 0 || unit.trucks.medium < 0 || unit.trucks.heavy < 0 {
        return Err(SupplyError::Invalid);
    }
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

fn stock_demand(stock: Supplies) -> Result<SupplyDemand, SupplyError> {
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
    let hex = unit.location.hex().ok_or(SupplyError::Invalid)?;
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
    for (id, holdings) in &state.logistics.unit_supply {
        let Some(carrier) = state.land.units.get(id) else {
            continue;
        };
        if carrier.side == unit.side
            && carrier.location.hex() == Some(hex)
            && carrier.trucks.total() > 0
        {
            sources.push(SupplyDraw {
                source: SupplySource::UnitStock(id.clone()),
                amount: stock_demand(holdings.carried)?,
            });
        }
    }
    for (id, dump) in &state.logistics.dumps {
        if dump.side == unit.side
            && dump.active
            && !dump.dummy
            && matches!(&dump.location, DumpLocation::Hex { hex: h } if h == hex)
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
    if !demand.valid() {
        return Err(SupplyError::Invalid);
    }
    let sources: BTreeMap<_, _> = available_sources(state, unit_id)?
        .into_iter()
        .map(|s| (s.source, s.amount))
        .collect();
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
    let mut next = state.logistics.clone();
    for (source, amount) in allocations {
        match source {
            SupplySource::Tank => {
                next.unit_supply
                    .get_mut(unit_id)
                    .ok_or(SupplyError::Invalid)?
                    .tank_fuel -= amount.fuel
            }
            SupplySource::ReadyAmmo => {
                next.unit_supply
                    .get_mut(unit_id)
                    .ok_or(SupplyError::Invalid)?
                    .ready_ammo -= amount.ammo
            }
            SupplySource::UnitStock(id) => deduct_stock(
                &mut next
                    .unit_supply
                    .get_mut(&id)
                    .ok_or(SupplyError::Invalid)?
                    .carried,
                amount,
            )?,
            SupplySource::Dump(id) => deduct_stock(
                &mut next
                    .dumps
                    .get_mut(&id)
                    .ok_or(SupplyError::Invalid)?
                    .supplies,
                amount,
            )?,
        }
    }
    state.logistics = next;
    Ok(())
}

fn deduct_stock(stock: &mut Supplies, amount: SupplyDemand) -> Result<(), SupplyError> {
    stock.fuel = stock
        .fuel
        .checked_sub(amount.fuel.ceil_points().get())
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
