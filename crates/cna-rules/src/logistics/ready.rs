//! The unit's own reserve is separate from first-line truck ammunition.
use super::{SupplyError, ammunition_cost, rations, toe_strength};
use crate::{CnaContent, state::State};
use cna_content::units::{InfantryKind, Toe};
use cna_core::{
    ids::UnitId,
    quantity::{AmmoPoints, ToeStrengthPoints},
};
use cna_tables::airlog::supply::{AmmoAction, AmmoMode};

/// Infantry identity comes from the double-read OA/counter classification. A class
/// containing both engineers and MG battalions cannot supply a class-wide default.
/// Classification provenance: orig79:land:4.22 (the original component legend).
/// Cases: airlog:50.13, airlog:50.2, land:4.48
pub fn close_assault_ammo_action(
    content: &CnaContent,
    id: &UnitId,
) -> Result<AmmoAction, SupplyError> {
    let row = content.units.units.get(id).ok_or(SupplyError::Invalid)?;
    let class = row
        .class
        .as_ref()
        .and_then(|id| content.units.classes.get(id));
    if class.is_some_and(|c| matches!(c.unit_type.as_str(), "infantry" | "engineer")) {
        match row.infantry_kind {
            Some(InfantryKind::Ordinary) => Ok(AmmoAction::CloseAssaultInfClass),
            Some(InfantryKind::MachineGun | InfantryKind::HeavyWeapons) => {
                Ok(AmmoAction::CloseAssaultArmorGunMgInfHvywpnInf)
            }
            None => Err(SupplyError::Unsupported {
                case: "airlog:50.17",
            }),
        }
    } else if matches!(
        class.map(|c| c.unit_type.as_str()),
        Some("tank" | "recce" | "artillery" | "anti_tank" | "anti_air")
    ) || matches!(&row.toe, Some(Toe::Weapons(_)))
    {
        Ok(AmmoAction::CloseAssaultArmorGunMgInfHvywpnInf)
    } else {
        Err(SupplyError::Unsupported {
            case: "airlog:50.17",
        })
    }
}

/// Enough for one supported firing, priced by actual participating TOE. Take the
/// largest supported single function, not the sum of consecutive combat actions.
/// Explicit weapon components contribute only to functions they can actually fire.
/// Cases: airlog:50.13, airlog:50.14, airlog:50.17, airlog:50.2, land:4.48
/// Interpretations: interp:airlog-0014
pub fn ready_ammo_capacity(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<AmmoPoints, SupplyError> {
    let unit = state.land.units.get(id).ok_or(SupplyError::Invalid)?;
    let strength = toe_strength(content, unit)?.get();
    if strength == 0 {
        return Ok(AmmoPoints::ZERO);
    }
    let mut maximum = 0;
    let mut assess = |action, n| -> Result<(), SupplyError> {
        let cost =
            ammunition_cost(content, AmmoMode::Played, action, ToeStrengthPoints::new(n))?.get();
        maximum = maximum.max(cost);
        Ok(())
    };
    let supports = |v: Option<i32>| v.is_some_and(|n| n > 0);
    if let Some(Toe::Weapons(weapons)) = &unit.toe {
        for function in 0..5 {
            let action = match function {
                0 => AmmoAction::Barrage,
                1 => AmmoAction::AntiArmor,
                2 | 3 => AmmoAction::CloseAssaultArmorGunMgInfHvywpnInf,
                _ => AmmoAction::AntiAirSingleTargetGroup,
            };
            let mut n = 0i32;
            for w in weapons {
                let definition = content
                    .units
                    .weapons
                    .get(&w.weapon)
                    .ok_or(SupplyError::Unsupported { case: "land:4.48" })?;
                let supported = match function {
                    0 => supports(definition.barrage),
                    1 => supports(definition.anti_armor),
                    2 => supports(definition.ca_off),
                    3 => supports(definition.ca_def),
                    _ => supports(definition.aa),
                };
                if supported {
                    n = n.checked_add(w.n).ok_or(SupplyError::Invalid)?;
                }
            }
            if n > 0 {
                assess(action, n)?;
            }
        }
    } else {
        let class = rations::class(content, id)?;
        if class.unit_type == "headquarters" {
            return Err(SupplyError::Unsupported {
                case: "airlog:50.17",
            });
        }
        let close_action = close_assault_ammo_action(content, id)?;
        if supports(class.ca_off) || supports(class.ca_def) {
            assess(close_action, strength)?;
        }
        if supports(class.barrage) {
            assess(AmmoAction::Barrage, strength)?;
        }
        if supports(class.anti_armor) {
            assess(AmmoAction::AntiArmor, strength)?;
        }
        if supports(class.aa) {
            assess(AmmoAction::AntiAirSingleTargetGroup, strength)?;
        }
    }
    if maximum == 0 {
        return Err(SupplyError::Unsupported {
            case: "airlog:50.17",
        });
    }
    Ok(AmmoPoints::new(maximum))
}

/// Full-profile source availability is checked from public scenario content, not current holdings.
/// Cases: airlog:50.17, land:3.6
pub(super) fn preflight(content: &CnaContent) -> Result<(), SupplyError> {
    // State::new supplies the immutable source TOE definitions, including scheduled counters.
    // No private locations, losses, assignments, stocks or actor choices enter this check.
    let printed = State::new(content).map_err(|_| SupplyError::Invalid)?;
    for unit in printed.land.units.values().filter(|u| u.toe.is_some()) {
        ready_ammo_capacity(content, &printed, &unit.id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
