//! Breakdown exposure and proportional allocation; all rolls belong to adjudication.
mod allocation;
pub mod baseline;
pub mod cohorts;
pub mod core;
pub mod losses;
pub mod markers;
mod packing;
pub mod pool_losses;
mod pool_window;
pub mod pools;
pub mod window;
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
    /// Actual convoy identities have separate additive bookkeeping.
    pub pools: BTreeMap<String, pools::PoolBreakdown>,
    pub accumulated_quarters: BTreeMap<UnitId, i32>,
    pub light_extra_quarters: BTreeMap<UnitId, i32>,
    pub moving: BTreeMap<UnitId, Motion>,
    /// A completed check does not discharge exposure; the unadjusted band is remembered.
    pub checked: BTreeMap<UnitId, BTreeMap<String, usize>>,
    pub stopped: Vec<StoppedMove>,
    pub markers: BTreeMap<String, markers::BrokenMarker>,
    /// Whole infantry points whose split carriage cannot yet be represented.
    pub unresolved_passengers: BTreeMap<UnitId, Vec<UnresolvedPassengers>>,
    pub next_marker: BTreeMap<cna_protocol::Side, u64>,
    pub truck_histories: BTreeMap<String, cohorts::History>,
    pub window: window::Window,
}
/// Owner-private physical accounting; these men are absent from the working body.
/// Cases: land:21.43, land:21.45
/// Interpretations: interp:land-0028
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnresolvedPassengers {
    pub points: i32,
    pub origin: HexId,
    pub destination: HexId,
    pub working_transport: cna_content::units::Trucks,
    pub origin_transport: cna_content::units::Trucks,
    pub destination_transport: cna_content::units::Trucks,
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
    use super::map;
    use cna_core::engine::Rejection;
    origin_condition(c, s, s.land.units[id].side, origin, strict, |from, to| {
        let rain = crate::logistics::weather::at_hex(c, s, to)? == WeatherKind::Rainstorm;
        match map::step_cost(c, s, id, from, to, strict, rain) {
            Ok(_) => Ok(true),
            Err(Rejection::Engine(e)) => Err(e),
            Err(_) => Ok(false),
        }
    })
}
fn origin_condition(
    c: &CnaContent,
    s: &State,
    side: cna_protocol::Side,
    origin: &HexId,
    strict: bool,
    passable: impl Fn(&HexId, &HexId) -> Result<bool, EngineError>,
) -> Result<bool, EngineError> {
    use super::{formation, zoc};
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
    let mut unresolved = None;
    for enemy in enemies {
        let result = if start.axial.distance(c.map.get(&enemy).unwrap().axial) == 1 {
            passable(origin, &enemy)
        } else {
            let mut found = false;
            for mid in c.map.neighbors(origin) {
                if mid.axial.distance(c.map.get(&enemy).unwrap().axial) != 1 {
                    continue;
                }
                let route = (|| {
                    if s.stack_presence(&mid.id, side)
                        || zoc::controlled(c, s, side, &mid.id, strict)?
                    {
                        return Ok(false);
                    }
                    Ok(passable(origin, &mid.id)? && passable(&mid.id, &enemy)?)
                })();
                match route {
                    Ok(true) => {
                        found = true;
                        break;
                    }
                    Ok(false) => {}
                    Err(e) => unresolved = Some(e),
                }
            }
            Ok(found)
        };
        match result {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(e) => unresolved = Some(e),
        }
    }
    if let Some(e) = unresolved {
        return Err(e);
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
    cohorts::add(s, id, bp_quarters, 0)?;
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
    let next = core::accrue_step(
        core::Exposure {
            accumulated_quarters: bp.checked_sub(bp_quarters).ok_or_else(overflow)?,
            light_extra_quarters: 0,
            travel_cp_quarters: motion.travel_cp_quarters,
            sandstorm_cp_quarters: motion.sandstorm_cp_quarters,
        },
        bp_quarters,
        0,
        cp_quarters,
        weather,
    )?;
    motion.travel_cp_quarters = next.travel_cp_quarters;
    motion.sandstorm_cp_quarters = next.sandstorm_cp_quarters;
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
    core::weather_shift(motion.travel_cp_quarters, motion.sandstorm_cp_quarters, hot)
}
/// A new roll requires exposure above three BP and a strictly higher unadjusted band.
/// Negative BAR shifts may remove a roll without erasing the accumulated points.
/// Cases: land:21.26, land:21.27, land:21.31, land:21.33
pub fn needs_check(c: &CnaContent, s: &State, id: &UnitId, category: &str) -> bool {
    needs_check_bp(
        c,
        s,
        id,
        category,
        s.land
            .breakdown
            .accumulated_quarters
            .get(id)
            .copied()
            .unwrap_or(0),
    )
}
fn needs_check_bp(c: &CnaContent, s: &State, id: &UnitId, category: &str, n: i32) -> bool {
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
/// Light trucks add one BP for each entered off-road hex and each crossed feature.
/// Counts remain separate from medium/heavy trucks and the unit's fighting vehicles.
/// Cases: airlog:54.2, land:21.29
pub fn record_light_extra(s: &mut State, id: &UnitId, quarters: i32) -> Result<(), EngineError> {
    if quarters < 0 {
        return Err(overflow());
    }
    cohorts::add(s, id, 0, quarters)?;
    let n = s
        .land
        .breakdown
        .light_extra_quarters
        .get(id)
        .copied()
        .unwrap_or(0)
        .checked_add(quarters)
        .ok_or_else(overflow)?;
    s.land.breakdown.light_extra_quarters.insert(id.clone(), n);
    Ok(())
}
fn asset_bp(s: &State, asset: &Asset) -> Result<i32, EngineError> {
    if let Some(bp) = cohorts::points(s, asset) {
        return Ok(bp);
    }
    let id = &asset.unit;
    let equipment = &asset.equipment;
    let base = s
        .land
        .breakdown
        .accumulated_quarters
        .get(id)
        .copied()
        .unwrap_or(0);
    base.checked_add(if *equipment == Equipment::LightTruck {
        s.land
            .breakdown
            .light_extra_quarters
            .get(id)
            .copied()
            .unwrap_or(0)
    } else {
        0
    })
    .ok_or_else(overflow)
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
    #[serde(default)]
    pub cohort: Option<String>,
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
/// Truck and weapon BARs come from typed characteristics data; HQ TOE is exempt.
/// Generic armored recce has no invented BAR when the summary chart is missing.
/// Cases: land:21.11, land:21.12, land:21.13, land:21.28
/// Interpretations: interp:land-0016
fn assets(
    c: &CnaContent,
    s: &State,
    id: &UnitId,
    body: bool,
) -> Result<Vec<(Category, i32, Asset)>, EngineError> {
    use super::formation;
    use cna_content::units::Toe;
    let u = &s.land.units[id];
    let mut out = vec![];
    use cna_tables::airlog::trucks::TruckType;
    let groups = crate::logistics::segment_fuel_cohorts(s, id).map_err(|_| overflow())?;
    for g in groups {
        let (equipment, kind) = match g.kind {
            crate::logistics::FuelTruckKind::Light => (Equipment::LightTruck, TruckType::Light),
            crate::logistics::FuelTruckKind::Medium => (Equipment::MediumTruck, TruckType::Medium),
            crate::logistics::FuelTruckKind::Heavy => (Equipment::HeavyTruck, TruckType::Heavy),
        };
        out.push((
            Category::Truck,
            -c.tables
                .airlog
                .truck_characteristics
                .truck(kind)
                .bar_shift_left,
            Asset {
                unit: id.clone(),
                equipment,
                points: g.count,
                cohort: Some(g.id),
            },
        ));
    }
    if !body {
        return Ok(out);
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
                        cohort: None,
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
    check_groups_profile(c, s, stopped, true).map(|(groups, _)| groups)
}
type GroupsWithGaps = (Vec<CheckGroup>, Vec<(UnitId, EngineError)>);
fn check_groups_profile(
    c: &CnaContent,
    s: &State,
    stopped: &StoppedMove,
    strict: bool,
) -> Result<GroupsWithGaps, EngineError> {
    let mut gaps = vec![];
    let mut inputs = vec![];
    let hot = s
        .turn
        .weather
        .as_ref()
        .is_some_and(|w| w.kind == WeatherKind::Hot);
    for id in &stopped.members {
        let weather = weather_shift(&stopped.exposures[id], hot);
        let available = match assets(
            c,
            s,
            id,
            s.land
                .breakdown
                .accumulated_quarters
                .get(id)
                .copied()
                .unwrap_or(0)
                > 12,
        ) {
            Ok(a) => a,
            Err(e) if !strict => {
                gaps.push((id.clone(), e));
                assets(c, s, id, false)?
            }
            Err(e) => return Err(e),
        };
        for (category, bar, asset) in available {
            let bp = asset_bp(s, &asset)?;
            let key = format!("{category:?}:{bar}:{:?}", asset.equipment);
            let checked_column = asset
                .cohort
                .as_ref()
                .and_then(|g| s.land.breakdown.truck_histories.get(g))
                .map_or_else(
                    || {
                        s.land
                            .breakdown
                            .checked
                            .get(id)
                            .and_then(|k| k.get(&key))
                            .copied()
                    },
                    |h| h.checked,
                );
            inputs.push(core::VehicleInput {
                side: s.land.units[id].side,
                category,
                bar,
                points: asset.points,
                bp_quarters: bp,
                checked_column,
                weather_shift: weather,
                identity: asset,
            });
        }
    }
    Ok((
        core::check_groups(&c.tables.land.breakdown, inputs)?
            .into_iter()
            .map(|g| CheckGroup {
                category: g.category,
                bar: g.bar,
                column: g.column,
                shift: g.shift,
                assets: g.assets.into_iter().map(|a| a.identity).collect(),
            })
            .collect(),
        gaps,
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolledCheck {
    pub group: CheckGroup,
    pub percent: i32,
    pub broken: i32,
    pub destination: HexId,
    pub origins: BTreeMap<UnitId, HexId>,
    pub require_origin: std::collections::BTreeSet<UnitId>,
}
/// Record every checked group's band, including a BAR-adjusted zero-loss check.
/// The dice event is private: breakdown may reveal only an actual presence transition.
/// Cases: land:21.26, land:21.31, land:21.32, land:21.33, land:21.34, land:21.35, land:3.62
pub fn roll_checks(
    c: &CnaContent,
    s: &mut State,
    stopped: &StoppedMove,
    strict: bool,
    cx: &mut cna_core::engine::Cx<'_>,
) -> Result<Vec<RolledCheck>, EngineError> {
    use cna_core::{event::EngineEvent, visibility::Audience};
    use cna_protocol::GameEvent;
    let (groups, gaps) = check_groups_profile(c, s, stopped, strict)?;
    for (id, gap) in gaps {
        cx.emit(
            EngineEvent::new(
                Audience::Side(s.land.units[&id].side),
                GameEvent::Note {
                    text: format!("Own vehicle breakdown unassessed for {id}: {gap:?}"),
                },
            )
            .at(stopped.destination.clone())
            .about(id),
        );
    }
    let mut outcomes = vec![];
    for group in groups {
        for a in &group.assets {
            if let Some(h) = a
                .cohort
                .as_ref()
                .and_then(|g| s.land.breakdown.truck_histories.get_mut(g))
            {
                h.checked = Some(group.column);
            }
            let key = format!("{:?}:{}:{:?}", group.category, group.bar, a.equipment);
            s.land
                .breakdown
                .checked
                .entry(a.unit.clone())
                .or_default()
                .insert(key.clone(), group.column);
        }
        if (group.column as i64) + i64::from(group.shift) < 1 {
            continue;
        }
        let d = cx.rng.two_dice_reading();
        let side = s.land.units[&group.assets[0].unit].side;
        cx.emit(
            EngineEvent::new(
                Audience::Side(side),
                GameEvent::DiceRolled {
                    purpose: format!(
                        "Own {:?} breakdown, BP band {}, shift {}",
                        group.category, group.column, group.shift
                    ),
                    dice: vec![d.tens.value(), d.units.value()],
                    reading: Some(d.value()),
                    rule: Some("land:21.34".into()),
                },
            )
            .at(stopped.destination.clone())
            .about(group.assets[0].unit.clone()),
        );
        let bp = asset_bp(s, &group.assets[0])?;
        let points: Vec<_> = group.assets.iter().map(|a| a.points).collect();
        let (percent, broken) =
            core::losses(&c.tables.land.breakdown, bp, group.shift, &points, d)?;
        if broken == 0 {
            continue;
        }
        let mut require_origin = std::collections::BTreeSet::new();
        let mut origins = BTreeMap::new();
        for a in &group.assets {
            let m = &stopped.exposures[&a.unit];
            origins.insert(a.unit.clone(), m.origin.clone());
            if m.origin_required {
                require_origin.insert(a.unit.clone());
            }
            if let Some(e) = &m.origin_gap {
                if strict {
                    return Err(e.clone());
                }
                cx.emit(EngineEvent::new(Audience::Side(side),GameEvent::Note{text:"Breakdown origin-placement constraint is unassessed because its map data is incomplete (land:21.41).".into()}).at(m.origin.clone()).about(a.unit.clone()));
            }
        }
        outcomes.push(RolledCheck {
            group,
            percent,
            broken,
            destination: stopped.destination.clone(),
            origins,
            require_origin,
        });
    }
    Ok(outcomes)
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
        s.land.units.get_mut(&hq).unwrap().location = crate::state::Location::Hex {
            hex: "C4020".into(),
        };
        assert_eq!(
            assets(&c, &s, &hq, true)
                .unwrap()
                .iter()
                .map(|a| a.0)
                .collect::<Vec<_>>(),
            vec![Category::Truck]
        );
    }
    /// Cases: airlog:54.2, land:21.29
    #[test]
    fn light_extra_reaches_a_different_band_from_other_trucks() {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let id = UnitId::new("it.libyan_tank_command.xxi_l_tank_bn");
        s.land.units.get_mut(&id).unwrap().toe = None;
        s.land.units.get_mut(&id).unwrap().trucks = cna_content::units::Trucks {
            light: 4,
            medium: 4,
            heavy: 0,
        };
        record_edge(&mut s, &id, &"C4020".into(), 40, 8, WeatherKind::Normal).unwrap();
        record_light_extra(&mut s, &id, 8).unwrap();
        stop(&mut s, std::slice::from_ref(&id), &"C4021".into());
        let g = check_groups(&c, &s, &s.land.breakdown.stopped[0]).unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].column, 1);
        assert_eq!(g[0].assets[0].equipment, Equipment::MediumTruck);
        assert_eq!(g[1].column, 2);
        assert_eq!(g[1].assets[0].equipment, Equipment::LightTruck);
    }
    /// Cases: land:21.24, land:21.26, land:21.34, land:3.62
    #[test]
    fn adjudication_rolls_once_per_group_privately_and_checkpoint_preserves_exposure() {
        use cna_core::{dice::CampaignRng, engine::Cx, visibility::Perspective};
        use cna_protocol::Side;
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let id = UnitId::new("it.libyan_tank_command.xxi_l_tank_bn");
        s.land.units.get_mut(&id).unwrap().trucks.light = 30;
        record_edge(&mut s, &id, &"C4020".into(), 140, 8, WeatherKind::Normal).unwrap();
        stop(&mut s, std::slice::from_ref(&id), &"C4021".into());
        let mut s: State = serde_json::from_value(serde_json::to_value(s).unwrap()).unwrap();
        let stopped = s.land.breakdown.stopped[0].clone();
        let mut rng = CampaignRng::from_seed([7; 32]);
        let mut events = vec![];
        roll_checks(
            &c,
            &mut s,
            &stopped,
            true,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.event, cna_protocol::GameEvent::DiceRolled { .. }))
                .count(),
            2
        );
        assert!(
            events
                .iter()
                .all(|e| !Perspective::Side(Side::Commonwealth).can_see(&e.audience))
        );
        assert_eq!(s.land.breakdown.accumulated_quarters[&id], 140);
        assert!(check_groups(&c, &s, &stopped).unwrap().is_empty());
        super::super::capability::finish_opstage(&mut s);
        assert!(s.land.breakdown.accumulated_quarters.is_empty());
        assert!(s.land.breakdown.checked.is_empty());
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
