//! Truthful control queries use authoritative strength; planning uses only disclosed answers.
use super::{formation, map};
use crate::{CnaContent, State};
use cna_content::map::{SideKind, Survey};
use cna_core::{
    engine::{EngineError, Rejection},
    ids::HexId,
};
use cna_protocol::Side;

/// A friendly combat unit negates the movement effect of hostile control, not its existence.
/// Cases: land:10.26, land:10.27
pub fn friendly_combat(content: &CnaContent, state: &State, side: Side, hex: &HexId) -> bool {
    state.units_of(side).any(|u| {
        u.location.hex() == Some(hex)
            && u.cohesion_quarters > -104
            && formation::combat_unit(content, &u.id)
            && formation::strength(content, state, &u.id) > 0
    })
}
/// Query enemy control only when a phasing unit begins its segment or enters a hex (10.6).
/// The six neighbors are tested independently; sea, river and escarpment edges block control.
/// Cases: land:10.11, land:10.12, land:10.13, land:10.14, land:10.15, land:10.21, land:10.6
/// Interpretations: interp:land-0007
pub fn controlled(
    content: &CnaContent,
    state: &State,
    enemy: Side,
    hex: &HexId,
    strict: bool,
) -> Result<bool, EngineError> {
    for neighbor in content.map.neighbors(hex) {
        let roots: Vec<_> = formation::roots(content, state, &neighbor.id, enemy)
            .into_iter()
            .filter(|id| state.land.units[id].cohesion_quarters > -104)
            .collect();
        let sp: i32 = roots
            .iter()
            .map(|id| formation::stacking_halves(content, state, id))
            .sum();
        let raw: i64 = roots
            .iter()
            .map(|id| formation::raw_defense(content, state, id))
            .sum();
        if sp <= 2 || raw < 10 {
            continue;
        }
        let mut blocked = false;
        for kind in [SideKind::AllSea, SideKind::MajorRiver, SideKind::Escarpment] {
            match content.map.hexside(&neighbor.id, hex, kind) {
                Survey::Present(_) => blocked = true,
                Survey::Unknown if strict => {
                    return Err(EngineError::Unsupported {
                        case: "land:10.21".into(),
                        detail: "control hexside coverage is incomplete".into(),
                    });
                }
                _ => {}
            }
        }
        if blocked {
            continue;
        }
        for root in roots {
            // A represented combat component capable of entering can exert its formation's zone.
            for id in formation::members(content, state, &root) {
                if !formation::combat_unit(content, &id)
                    || formation::strength(content, state, &id) == 0
                    || state.land.units[&id].cohesion_quarters <= -104
                {
                    continue;
                }
                let rain = if state.turn.weather.is_some() {
                    crate::logistics::weather::at_hex(content, state, hex)?
                        == cna_tables::land::weather::WeatherKind::Rainstorm
                } else {
                    false
                };
                match map::step_cost(content, state, &id, &neighbor.id, hex, strict, rain) {
                    Ok(_) => return Ok(true),
                    Err(Rejection::Engine(e)) => return Err(e),
                    Err(_) => {}
                }
            }
        }
    }
    Ok(false)
}
/// Nearby enemy stack presence is public, but its ability to control an unqueried hex is not.
/// Cases: land:3.61, land:3.62, land:10.6
pub fn possibly_controlled(content: &CnaContent, state: &State, enemy: Side, hex: &HexId) -> bool {
    content.map.neighbors(hex).iter().any(|h| {
        state
            .units_of(enemy)
            .any(|u| u.location.hex() == Some(&h.id))
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Cases: land:10.11, land:10.14, land:10.15, land:10.6
    #[test]
    fn real_units_below_strength_threshold_or_disorganized_do_not_control() {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let own = "C4020".into();
        let target = "C4021".into();
        for u in s.land.units.values_mut() {
            u.location = crate::state::Location::Eliminated;
        }
        let id: cna_core::ids::UnitId = "it.1_libyan_div.viii_libyan_bn".into();
        let u = s.land.units.get_mut(&id).unwrap();
        u.location = crate::state::Location::Hex { hex: own };
        u.detached = true;
        u.toe = Some(cna_content::units::Toe::Under { under: 1 });
        assert!(!controlled(&c, &s, Side::Axis, &target, false).unwrap());
        s.land.units.get_mut(&id).unwrap().cohesion_quarters = -104;
        assert!(!controlled(&c, &s, Side::Axis, &target, false).unwrap());
    }
}
