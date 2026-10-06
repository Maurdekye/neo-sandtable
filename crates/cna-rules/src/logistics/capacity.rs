//! Exact packing by truck type and innate vehicle fuel capacity.
use super::rations;
use super::{SupplyError, toe_strength};
use crate::{CnaContent, state::State};
use cna_content::{
    scenario::Supplies,
    units::{Toe, Trucks},
};
use cna_core::{ids::UnitId, quantity::FuelTenths};
use cna_tables::airlog::{supply::SupplyType, trucks::TruckType};
use serde::{Deserialize, Serialize};

const TYPES: [TruckType; 3] = [TruckType::Light, TruckType::Medium, TruckType::Heavy];
const SUPPLIES: [SupplyType; 4] = [
    SupplyType::Ammo,
    SupplyType::Fuel,
    SupplyType::Stores,
    SupplyType::Water,
];

/// A proposed post-load packing; quantities remain in the unit's aggregate holdings.
/// Cases: airlog:53.11, airlog:54.2
/// Interpretations: interp:airlog-0008
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CargoPacking {
    pub light: Supplies,
    pub medium: Supplies,
    pub heavy: Supplies,
}
impl CargoPacking {
    pub fn cargo(&self, kind: TruckType) -> &Supplies {
        match kind {
            TruckType::Light => &self.light,
            TruckType::Medium => &self.medium,
            TruckType::Heavy => &self.heavy,
        }
    }
    pub fn totals(&self) -> Result<Supplies, SupplyError> {
        let mut total = Supplies::default();
        for kind in TYPES {
            let s = self.cargo(kind);
            for t in SUPPLIES {
                let n = points(s, t);
                if n < 0 {
                    return Err(SupplyError::Invalid);
                }
                let new = points(&total, t)
                    .checked_add(n)
                    .ok_or(SupplyError::Invalid)?;
                set_points(&mut total, t, new);
            }
        }
        Ok(total)
    }
}
pub(super) fn points(s: &Supplies, t: SupplyType) -> i32 {
    match t {
        SupplyType::Ammo => s.ammo,
        SupplyType::Fuel => s.fuel,
        SupplyType::Stores => s.stores,
        SupplyType::Water => s.water,
    }
}
pub(super) fn set_points(s: &mut Supplies, t: SupplyType, n: i32) {
    match t {
        SupplyType::Ammo => s.ammo = n,
        SupplyType::Fuel => s.fuel = n,
        SupplyType::Stores => s.stores = n,
        SupplyType::Water => s.water = n,
    }
}
pub(super) fn trucks(t: &Trucks, kind: TruckType) -> i32 {
    match kind {
        TruckType::Light => t.light,
        TruckType::Medium => t.medium,
        TruckType::Heavy => t.heavy,
    }
}

/// Men and cargo share the same attached trucks. Mixed cargo uses the exact sum of
/// chart-capacity fractions; no floats or independent full maxima for each supply.
/// Cases: airlog:53.11, airlog:54.2
/// Interpretations: interp:airlog-0008
pub fn validate_packing(
    content: &CnaContent,
    attached: &Trucks,
    transport: &Trucks,
    expected: &Supplies,
    packing: &CargoPacking,
) -> Result<(), SupplyError> {
    if &packing.totals()? != expected {
        return Err(SupplyError::Invalid);
    }
    for kind in TYPES {
        let total = trucks(attached, kind);
        let motorizing = trucks(transport, kind);
        if motorizing < 0 || total < motorizing {
            return Err(SupplyError::Invalid);
        }
        let chart = content.tables.airlog.truck_characteristics.truck(kind);
        let den = SUPPLIES.into_iter().try_fold(1i64, |d, t| {
            let cap = chart.supply_capacity(t);
            if cap <= 0 {
                return Err(SupplyError::Unsupported {
                    case: "airlog:54.2",
                });
            }
            d.checked_mul(i64::from(cap)).ok_or(SupplyError::Invalid)
        })?;
        let used = SUPPLIES.into_iter().try_fold(0i64, |sum, t| {
            let cap = i64::from(chart.supply_capacity(t));
            let fraction = i64::from(points(packing.cargo(kind), t))
                .checked_mul(den / cap)
                .ok_or(SupplyError::Invalid)?;
            sum.checked_add(fraction).ok_or(SupplyError::Invalid)
        })?;
        let allowed = i64::from(total - motorizing)
            .checked_mul(den)
            .ok_or(SupplyError::Invalid)?;
        if used > allowed {
            return Err(SupplyError::Insufficient);
        }
    }
    Ok(())
}

/// Innate tank space follows each identified component's own CPA and fuel rate,
/// plus the trucks' printed tank ratings. An unresolved HQ composition stays unknown.
/// Cases: airlog:49.12, airlog:49.14, airlog:54.2
/// Interpretations: interp:units-0005
pub fn fuel_capacity(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<FuelTenths, SupplyError> {
    let unit = state.land.units.get(id).ok_or(SupplyError::Invalid)?;
    let class = rations::class(content, id)?;
    let mut tenths = 0i64;
    let mut add = |n: i32, cpa: i32, rate: i32| -> Result<(), SupplyError> {
        if n < 0 || cpa < 0 || rate < 0 {
            return Err(SupplyError::Invalid);
        }
        let v = i64::from(n)
            .checked_mul(i64::from(cpa))
            .and_then(|v| v.checked_mul(i64::from(rate)))
            .and_then(|v| v.checked_mul(2))
            .ok_or(SupplyError::Invalid)?;
        tenths = tenths.checked_add(v).ok_or(SupplyError::Invalid)?;
        Ok(())
    };
    match &unit.toe {
        Some(Toe::Weapons(ws)) => {
            for w in ws {
                let definition = content
                    .units
                    .weapons
                    .get(&w.weapon)
                    .ok_or(SupplyError::Unsupported { case: "land:4.48" })?;
                add(
                    w.n,
                    definition.cpa,
                    definition.fuel_rate.ok_or(SupplyError::UnknownFuelRate)?,
                )?;
            }
        }
        _ if class.unit_type == "recce" => add(toe_strength(content, unit)?.get(), class.cpa, 1)?,
        _ if matches!(class.unit_type.as_str(), "infantry" | "engineer") => {}
        _ if class.unit_type == "headquarters" && class.max_toe_paren => {}
        _ if class.unit_type == "headquarters" => return Err(SupplyError::UnknownFuelRate),
        _ => {
            return Err(SupplyError::Unsupported {
                case: "airlog:49.12",
            });
        }
    }
    for kind in TYPES {
        let n = trucks(&unit.trucks, kind);
        if n < 0 {
            return Err(SupplyError::Invalid);
        }
        let tank = content
            .tables
            .airlog
            .truck_characteristics
            .truck(kind)
            .fuel_capacity_points;
        tenths = tenths
            .checked_add(i64::from(n) * i64::from(tank) * 10)
            .ok_or(SupplyError::Invalid)?;
    }
    Ok(FuelTenths::new(
        i32::try_from(tenths).map_err(|_| SupplyError::Invalid)?,
    ))
}

/// A broad action bound. A submitted packing must still validate mixed cargo and
/// existing holdings; this bound cannot authorize simultaneous independent maxima.
/// Cases: airlog:53.11, airlog:54.2
pub fn cargo_bound(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    supply: SupplyType,
) -> Result<i32, SupplyError> {
    let unit = state.land.units.get(id).ok_or(SupplyError::Invalid)?;
    let mut capacity = 0i64;
    for kind in TYPES {
        let n = trucks(&unit.trucks, kind);
        let used = trucks(&unit.transport_trucks, kind);
        if used < 0 || n < used {
            return Err(SupplyError::Invalid);
        }
        capacity += i64::from(n - used)
            * i64::from(
                content
                    .tables
                    .airlog
                    .truck_characteristics
                    .truck(kind)
                    .supply_capacity(supply),
            );
    }
    i32::try_from(capacity).map_err(|_| SupplyError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Cases: airlog:53.11, airlog:54.2
    /// Interpretations: interp:airlog-0008
    #[test]
    fn mixed_loads_and_motorization_use_one_shared_capacity() {
        let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let attached = Trucks {
            heavy: 1,
            ..Trucks::default()
        };
        let packing = CargoPacking {
            heavy: Supplies {
                ammo: 4,
                fuel: 125,
                ..Supplies::default()
            },
            ..CargoPacking::default()
        };
        validate_packing(
            &content,
            &attached,
            &Trucks::default(),
            &packing.totals().unwrap(),
            &packing,
        )
        .unwrap();
        let mut too_much = packing.clone();
        too_much.heavy.water = 1;
        assert_eq!(
            validate_packing(
                &content,
                &attached,
                &Trucks::default(),
                &too_much.totals().unwrap(),
                &too_much
            ),
            Err(SupplyError::Insufficient)
        );
        let transport = Trucks {
            heavy: 1,
            ..Trucks::default()
        };
        assert_eq!(
            validate_packing(
                &content,
                &attached,
                &transport,
                &packing.totals().unwrap(),
                &packing
            ),
            Err(SupplyError::Insufficient)
        );
        let mut bad = packing;
        bad.light.stores = -1;
        assert!(bad.totals().is_err());
    }
    /// Cases: airlog:49.12, airlog:49.14, airlog:54.2
    /// Interpretations: interp:units-0005
    #[test]
    fn fuel_tanks_use_identified_component_cpa_and_printed_truck_tanks() {
        let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut state = State::new(&content).unwrap();
        let id: UnitId = "it.1_libyan_div.viii_libyan_bn".into();
        state.land.units.get_mut(&id).unwrap().trucks = Trucks {
            light: 1,
            medium: 2,
            heavy: 3,
        };
        assert_eq!(fuel_capacity(&content, &state, &id).unwrap().get(), 380);
        let hq = state
            .land
            .units
            .values()
            .find(|u| {
                u.toe.as_ref().is_some_and(|t| matches!(t, Toe::Normal(_)))
                    && rations::class(&content, &u.id)
                        .is_ok_and(|c| c.unit_type == "headquarters" && !c.max_toe_paren)
            })
            .unwrap();
        assert_eq!(
            fuel_capacity(&content, &state, &hq.id),
            Err(SupplyError::UnknownFuelRate)
        );
    }
}
