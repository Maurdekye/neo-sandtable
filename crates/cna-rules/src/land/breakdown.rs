//! Breakdown exposure and proportional allocation; all rolls belong to adjudication.
mod allocation;
use crate::{CnaContent, State};
pub use allocation::{balanced_allocation, valid_group_allocation};
use cna_core::{
    engine::EngineError,
    ids::{HexId, UnitId},
};
use cna_tables::land::weather::WeatherKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BreakdownState {
    pub accumulated_quarters: BTreeMap<UnitId, i32>,
    pub moving: BTreeMap<UnitId, Motion>,
    /// A completed check does not discharge exposure; the unadjusted band is remembered.
    pub checked: BTreeMap<UnitId, BTreeMap<String, usize>>,
    pub stopped: Vec<StoppedMove>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Motion {
    pub origin: HexId,
    pub travel_cp_quarters: i32,
    pub sandstorm_cp_quarters: i32,
    #[serde(default)]
    pub origin_required: bool,
    #[serde(default)]
    pub origin_gap: Option<EngineError>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoppedMove {
    pub members: Vec<UnitId>,
    pub exposures: BTreeMap<UnitId, Motion>,
    pub destination: HexId,
}
fn overflow() -> EngineError {
    EngineError::Invariant {
        detail: "breakdown exposure overflow".into(),
    }
}
/// Snapshot the start-of-move placement constraint before an enemy reaction can change it.
/// Missing truth-side map data is retained for adjudication rather than rejecting the order.
/// Cases: land:21.41
pub fn begin_motion(c: &CnaContent, s: &mut State, id: &UnitId, origin: &HexId, strict: bool) {
    if s.land.breakdown.moving.contains_key(id) {
        return;
    }
    let result = origin_requirement(c, s, id, origin, strict);
    s.land.breakdown.moving.insert(
        id.clone(),
        Motion {
            origin: origin.clone(),
            travel_cp_quarters: 0,
            sandstorm_cp_quarters: 0,
            origin_required: result.as_ref().copied().unwrap_or(false),
            origin_gap: result.err(),
        },
    );
}
/// A larger represented enemy combat formation requires an unobstructed one/two-hex route.
/// Friendly counters, friendly control or a prohibited intervening crossing protect the origin.
/// Cases: land:21.41
fn origin_requirement(
    c: &CnaContent,
    s: &State,
    id: &UnitId,
    origin: &HexId,
    strict: bool,
) -> Result<bool, EngineError> {
    use super::{formation, map, zoc};
    use cna_core::engine::Rejection;
    let side = s.land.units[id].side;
    let Some(start) = c.map.get(origin) else {
        return Err(gap("land:21.41", "movement origin is unresolved"));
    };
    let mut enemies = vec![];
    for root in s.units_of(side.opponent()) {
        let Some(h) = root.location.hex().and_then(|h| c.map.get(h)) else {
            continue;
        };
        if start.axial.distance(h.axial) > 2 || start.axial.distance(h.axial) == 0 {
            continue;
        }
        if c.units
            .units
            .get(&root.id)
            .and_then(|u| u.stacking_points)
            .unwrap_or(0)
            <= 1
        {
            continue;
        }
        if formation::members(c, s, &root.id)
            .iter()
            .any(|m| formation::combat_unit(c, m) && formation::strength(c, s, m) > 0)
        {
            enemies.push(h.id.clone());
        }
    }
    let passable = |from: &HexId, to: &HexId| -> Result<bool, EngineError> {
        let rain = crate::logistics::weather::at_hex(c, s, to)? == WeatherKind::Rainstorm;
        match map::step_cost(c, s, id, from, to, strict, rain) {
            Ok(_) => Ok(true),
            Err(Rejection::Engine(e)) => Err(e),
            Err(_) => Ok(false),
        }
    };
    for enemy in enemies {
        if start.axial.distance(c.map.get(&enemy).unwrap().axial) == 1 {
            if passable(origin, &enemy)? {
                return Ok(true);
            }
            continue;
        }
        for mid in c.map.neighbors(origin) {
            if mid.axial.distance(c.map.get(&enemy).unwrap().axial) != 1 {
                continue;
            }
            if s.units_of(side).any(|u| u.location.hex() == Some(&mid.id))
                || zoc::controlled(c, s, side, &mid.id, strict)?
            {
                continue;
            }
            if passable(origin, &mid.id)? && passable(&mid.id, &enemy)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
/// Only travelled edges count: detachment, combat and breaking-off expenditure are excluded.
/// One motion continues through reaction interrupts and its revised remaining path.
/// Cases: land:21.21, land:21.22, land:21.23, land:21.25, land:21.37
/// Interpretations: interp:land-0027
pub fn record_edge(
    s: &mut State,
    id: &UnitId,
    from: &HexId,
    bp_quarters: i32,
    cp_quarters: i32,
    weather: WeatherKind,
) -> Result<(), EngineError> {
    if bp_quarters < 0 || cp_quarters <= 0 {
        return Err(overflow());
    }
    let bp = s
        .land
        .breakdown
        .accumulated_quarters
        .get(id)
        .copied()
        .unwrap_or(0)
        .checked_add(bp_quarters)
        .ok_or_else(overflow)?;
    let mut motion = s.land.breakdown.moving.get(id).cloned().unwrap_or(Motion {
        origin: from.clone(),
        travel_cp_quarters: 0,
        sandstorm_cp_quarters: 0,
        origin_required: false,
        origin_gap: None,
    });
    motion.travel_cp_quarters = motion
        .travel_cp_quarters
        .checked_add(cp_quarters)
        .ok_or_else(overflow)?;
    if weather == WeatherKind::Sandstorm {
        motion.sandstorm_cp_quarters = motion
            .sandstorm_cp_quarters
            .checked_add(cp_quarters)
            .ok_or_else(overflow)?;
    }
    s.land.breakdown.accumulated_quarters.insert(id.clone(), bp);
    s.land.breakdown.moving.insert(id.clone(), motion);
    Ok(())
}
/// Queue one check when a complete ordinary, reaction or retreat move stops.
/// Cases: land:21.24, land:21.25, land:21.28, land:21.29
pub fn stop(s: &mut State, members: &[UnitId], destination: &HexId) {
    let exposures: BTreeMap<_, _> = members
        .iter()
        .filter_map(|id| s.land.breakdown.moving.remove(id).map(|m| (id.clone(), m)))
        .collect();
    if !exposures.is_empty() {
        s.land.breakdown.stopped.push(StoppedMove {
            members: exposures.keys().cloned().collect(),
            exposures,
            destination: destination.clone(),
        });
    }
}
/// Hot weather and the majority of actual movement CP each contribute a right shift.
/// Cases: land:21.32, land:21.37, land:29.33, land:29.45
/// Interpretations: interp:land-0027
pub fn weather_shift(motion: &Motion, hot: bool) -> i32 {
    i32::from(hot)
        + i32::from(
            motion.travel_cp_quarters > 0
                && i64::from(motion.sandstorm_cp_quarters) * 2
                    >= i64::from(motion.travel_cp_quarters),
        )
}
/// A new roll requires exposure above three BP and a strictly higher unadjusted band.
/// Negative BAR shifts may remove a roll without erasing the accumulated points.
/// Cases: land:21.26, land:21.27, land:21.31, land:21.33
pub fn needs_check(c: &CnaContent, s: &State, id: &UnitId, category: &str) -> bool {
    let n = s
        .land
        .breakdown
        .accumulated_quarters
        .get(id)
        .copied()
        .unwrap_or(0);
    n > 12
        && c.tables
            .land
            .breakdown
            .column_quarters(n)
            .is_some_and(|band| {
                s.land
                    .breakdown
                    .checked
                    .get(id)
                    .and_then(|k| k.get(category))
                    .is_none_or(|old| band > *old)
            })
}
/// An allocation uses each member's proportional quota, with all integer remainders conserved.
/// Players may choose ties; the baseline uses largest remainder then stable input order.
/// Cases: land:21.35, land:21.36
pub fn proportional_allocation(points: &[i32], broken: i32) -> Option<Vec<i32>> {
    if points.iter().any(|n| *n < 0) || broken < 0 {
        return None;
    }
    let total: i64 = points.iter().map(|n| i64::from(*n)).sum();
    if i64::from(broken) > total {
        return None;
    }
    if total == 0 {
        return Some(vec![0; points.len()]);
    }
    let mut answer: Vec<i32> = points
        .iter()
        .map(|n| (i64::from(*n) * i64::from(broken) / total) as i32)
        .collect();
    let mut order: Vec<_> = (0..points.len()).collect();
    order.sort_by_key(|i| {
        (
            std::cmp::Reverse(i64::from(points[*i]) * i64::from(broken) % total),
            *i,
        )
    });
    let remaining = broken - answer.iter().sum::<i32>();
    for i in order.into_iter().take(remaining as usize) {
        answer[i] += 1;
    }
    Some(answer)
}
/// Accept any player-selected floor/ceiling allocation that conserves the rolled total.
/// Cases: land:21.36
pub fn valid_allocation(points: &[i32], broken: i32, allocation: &[i32]) -> bool {
    if points.len() != allocation.len() || proportional_allocation(points, broken).is_none() {
        return false;
    }
    let total: i64 = points.iter().map(|n| i64::from(*n)).sum();
    allocation.iter().map(|n| i64::from(*n)).sum::<i64>() == i64::from(broken)
        && points.iter().zip(allocation).all(|(n, l)| {
            if total == 0 {
                return *l == 0;
            }
            let product = i64::from(*n) * i64::from(broken);
            *l >= 0
                && *l <= *n
                && i64::from(*l) >= product / total
                && i64::from(*l) <= (product + total - 1) / total
        })
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Truck,
    Tank,
    ArmoredRecce,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Equipment {
    LightTruck,
    MediumTruck,
    HeavyTruck,
    Weapon(String),
    ArmoredRecce,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub unit: UnitId,
    pub equipment: Equipment,
    pub points: i32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckGroup {
    pub category: Category,
    pub bar: i32,
    pub column: usize,
    pub shift: i32,
    pub assets: Vec<Asset>,
}
fn gap(case: &str, detail: &str) -> EngineError {
    EngineError::Unsupported {
        case: case.into(),
        detail: detail.into(),
    }
}
/// The examples establish truck BAR -2. Weapon BARs remain source data; HQ TOE is exempt.
/// Generic armored recce has no invented BAR when the summary chart is missing.
/// Cases: land:21.11, land:21.12, land:21.13, land:21.28
/// Interpretations: interp:land-0016
fn assets(
    c: &CnaContent,
    s: &State,
    id: &UnitId,
) -> Result<Vec<(Category, i32, Asset)>, EngineError> {
    use super::formation;
    use cna_content::units::Toe;
    let u = &s.land.units[id];
    let mut out = vec![];
    for (equipment, n) in [
        (Equipment::LightTruck, u.trucks.light),
        (Equipment::MediumTruck, u.trucks.medium),
        (Equipment::HeavyTruck, u.trucks.heavy),
    ] {
        if n < 0 {
            return Err(overflow());
        }
        if n > 0 {
            out.push((
                Category::Truck,
                -2,
                Asset {
                    unit: id.clone(),
                    equipment,
                    points: n,
                },
            ));
        }
    }
    let Some(class) = formation::class(c, id) else {
        return Ok(out);
    };
    if class.unit_type == "headquarters" || class.echelon.as_deref() == Some("hq") {
        return Ok(out);
    }
    if let Some(Toe::Weapons(points)) = &u.toe {
        for p in points.iter().filter(|p| p.n > 0) {
            let w = c
                .units
                .weapons
                .get(&p.weapon)
                .ok_or_else(|| gap("land:21.12", "weapon breakdown rating unavailable"))?;
            if let Some(bar) = &w.bar {
                let shift = match bar.dir.as_deref() {
                    Some("L") => -bar.shift,
                    Some("R") | None => bar.shift,
                    _ => return Err(overflow()),
                };
                out.push((
                    Category::Tank,
                    shift,
                    Asset {
                        unit: id.clone(),
                        equipment: Equipment::Weapon(p.weapon.clone()),
                        points: p.n,
                    },
                ));
            } else if w.kind == "tank" {
                return Err(gap("land:21.12", "tank breakdown rating unavailable"));
            }
        }
    } else if class.unit_type == "tank" && formation::strength(c, s, id) > 0 {
        return Err(gap("land:21.12", "tank composition is unresolved"));
    } else if class.unit_type == "recce"
        && formation::strength(c, s, id) > 0
        && formation::individual_allowance(c, s, id).is_some_and(|a| a.motorized)
        && !class
            .equipment_note
            .as_deref()
            .is_some_and(|n| n.contains("motorcycle"))
    {
        return Err(gap(
            "land:21.14",
            "generic armored recce breakdown rating is unavailable (interp:land-0016)",
        ));
    }
    Ok(out)
}
/// Units of different vehicle categories, BARs or exposure bands use separate rolls.
/// All fractional BP is retained until this band lookup; weather shifts do not reset exposure.
/// Cases: land:21.26, land:21.27, land:21.28, land:21.29, land:21.31, land:21.32
pub fn check_groups(
    c: &CnaContent,
    s: &State,
    stopped: &StoppedMove,
) -> Result<Vec<CheckGroup>, EngineError> {
    let mut groups: BTreeMap<(Category, i32, usize, i32), Vec<Asset>> = BTreeMap::new();
    let hot = s
        .turn
        .weather
        .as_ref()
        .is_some_and(|w| w.kind == WeatherKind::Hot);
    for id in &stopped.members {
        let bp = s
            .land
            .breakdown
            .accumulated_quarters
            .get(id)
            .copied()
            .unwrap_or(0);
        if bp <= 12 {
            continue;
        }
        let column = c
            .tables
            .land
            .breakdown
            .column_quarters(bp)
            .ok_or_else(overflow)?;
        let weather = weather_shift(&stopped.exposures[id], hot);
        for (category, bar, asset) in assets(c, s, id)? {
            let key = format!("{category:?}:{bar}");
            if needs_check(c, s, id, &key) {
                groups
                    .entry((category, bar, column, bar + weather))
                    .or_default()
                    .push(asset);
            }
        }
    }
    Ok(groups
        .into_iter()
        .map(|((category, bar, column, shift), assets)| CheckGroup {
            category,
            bar,
            column,
            shift,
            assets,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Cases: land:21.21, land:21.22, land:21.25, land:21.26, land:21.27
    #[test]
    fn stopping_retains_bp_and_only_a_new_band_requires_another_roll() {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let id = UnitId::new("it.libyan_tank_command.xxi_l_tank_bn");
        record_edge(&mut s, &id, &"C4020".into(), 12, 8, WeatherKind::Normal).unwrap();
        assert!(!needs_check(&c, &s, &id, "tank:2"));
        record_edge(&mut s, &id, &"C4021".into(), 48, 8, WeatherKind::Normal).unwrap();
        stop(&mut s, std::slice::from_ref(&id), &"C4022".into());
        assert_eq!(s.land.breakdown.accumulated_quarters[&id], 60);
        assert_eq!(
            s.land.breakdown.stopped[0].exposures[&id].origin,
            HexId::new("C4020")
        );
        s.land
            .breakdown
            .checked
            .entry(id.clone())
            .or_default()
            .insert("tank:2".into(), 2);
        assert!(!needs_check(&c, &s, &id, "tank:2"));
        record_edge(&mut s, &id, &"C4022".into(), 8, 8, WeatherKind::Normal).unwrap();
        assert!(!needs_check(&c, &s, &id, "tank:2"));
        record_edge(&mut s, &id, &"C4023".into(), 40, 8, WeatherKind::Normal).unwrap();
        assert!(needs_check(&c, &s, &id, "tank:2"));
    }
    /// Cases: land:21.11, land:21.12, land:21.28, land:21.29
    #[test]
    fn truck_and_weapon_groups_are_separate_and_hq_weapons_are_exempt() {
        use cna_content::units::{Toe, WeaponPoints};
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let id = UnitId::new("it.libyan_tank_command.xxi_l_tank_bn");
        let weapon = c
            .units
            .weapons
            .values()
            .find(|w| w.nation == "it" && w.kind == "tank" && w.bar.is_some())
            .unwrap()
            .id
            .clone();
        s.land.units.get_mut(&id).unwrap().toe = Some(Toe::Weapons(vec![WeaponPoints {
            weapon: weapon.clone(),
            n: 20,
        }]));
        s.land.units.get_mut(&id).unwrap().trucks.light = 30;
        record_edge(&mut s, &id, &"C4020".into(), 140, 8, WeatherKind::Normal).unwrap();
        stop(&mut s, std::slice::from_ref(&id), &"C4021".into());
        let g = check_groups(&c, &s, &s.land.breakdown.stopped[0]).unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].category, Category::Truck);
        assert_eq!(g[0].bar, -2);
        assert_eq!(g[0].assets[0].points, 30);
        assert_eq!(g[1].category, Category::Tank);
        let hq = c
            .units
            .units
            .keys()
            .find(|id| {
                super::super::formation::class(&c, id)
                    .is_some_and(|k| k.unit_type == "headquarters")
            })
            .unwrap()
            .clone();
        s.land.units.get_mut(&hq).unwrap().toe =
            Some(Toe::Weapons(vec![WeaponPoints { weapon, n: 5 }]));
        s.land.units.get_mut(&hq).unwrap().trucks.medium = 2;
        assert_eq!(
            assets(&c, &s, &hq)
                .unwrap()
                .iter()
                .map(|a| a.0)
                .collect::<Vec<_>>(),
            vec![Category::Truck]
        );
    }
    /// Cases: land:21.37, land:29.45
    /// Interpretations: interp:land-0027
    #[test]
    fn sandstorm_share_uses_travel_cp_including_the_exact_half_boundary() {
        let mut m = Motion {
            origin: "C4020".into(),
            travel_cp_quarters: 16,
            sandstorm_cp_quarters: 8,
            origin_required: false,
            origin_gap: None,
        };
        assert_eq!(weather_shift(&m, false), 1);
        assert_eq!(weather_shift(&m, true), 2);
        m.sandstorm_cp_quarters = 7;
        assert_eq!(weather_shift(&m, false), 0);
        m.travel_cp_quarters = 0;
        assert_eq!(weather_shift(&m, false), 0);
    }
    /// Cases: land:21.35, land:21.36
    #[test]
    fn proportional_losses_conserve_unequal_strengths_and_allow_player_ties() {
        assert_eq!(proportional_allocation(&[10, 20], 3), Some(vec![1, 2]));
        assert_eq!(proportional_allocation(&[1, 1, 1], 2), Some(vec![1, 1, 0]));
        assert!(valid_allocation(&[1, 1, 1], 2, &[0, 1, 1]));
        assert!(!valid_allocation(&[10, 20], 3, &[3, 0]));
        for a in 0..=20 {
            for b in 0..=20 {
                for lost in 0..=a + b {
                    let out = proportional_allocation(&[a, b], lost).unwrap();
                    assert!(valid_allocation(&[a, b], lost, &out));
                }
            }
        }
    }
}
