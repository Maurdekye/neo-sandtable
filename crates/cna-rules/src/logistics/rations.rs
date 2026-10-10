//! Private supply history and the limits movement and combat must enforce.
use super::{SupplyError, toe_strength};
use crate::content::CnaContent;
use crate::state::{Location, State};
use cna_content::units::{Toe, UnitClass};
use cna_core::ids::UnitId;
use cna_protocol::Side;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WaterStage {
    pub game_turn: u16,
    pub op_stage: u8,
}
impl WaterStage {
    pub fn current(state: &State) -> Self {
        Self {
            game_turn: state.cursor.game_turn,
            op_stage: state.cursor.op_stage.unwrap_or(1),
        }
    }
    pub fn ordinal(self) -> u32 {
        (u32::from(self.game_turn) - 1) * 3 + u32::from(self.op_stage)
    }
}

/// Cases: airlog:51.21, airlog:51.22, airlog:51.23, airlog:52.53, airlog:52.6
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rations {
    #[serde(skip_serializing_if = "is_default")]
    pub issued_gt: Option<u16>,
    #[serde(skip_serializing_if = "is_default")]
    pub stores_received: i32,
    #[serde(skip_serializing_if = "is_default")]
    pub stores_required: i32,
    #[serde(skip_serializing_if = "is_default")]
    pub half: bool,
    #[serde(skip_serializing_if = "is_default")]
    pub finalized_gt: Option<u16>,
    #[serde(skip_serializing_if = "is_default")]
    pub last_short_gt: Option<u16>,
    #[serde(skip_serializing_if = "is_default")]
    pub consecutive_short_gt: u16,
    #[serde(skip_serializing_if = "is_default")]
    pub pasta_gt: Option<u16>,
    #[serde(skip_serializing_if = "is_default")]
    pub pasta_saved_cohesion_quarters: Option<i32>,
    #[serde(skip_serializing_if = "is_default")]
    pub water_stage: Option<WaterStage>,
    #[serde(skip_serializing_if = "is_default")]
    pub water_issue_stage: Option<WaterStage>,
    #[serde(skip_serializing_if = "is_default")]
    pub infantry_water_received: i32,
    #[serde(skip_serializing_if = "is_default")]
    pub last_short_water_stage: Option<WaterStage>,
    #[serde(skip_serializing_if = "is_default")]
    pub consecutive_short_water_stages: u32,
    #[serde(skip_serializing_if = "is_default")]
    pub water_finalized_stage: Option<WaterStage>,
    #[serde(skip_serializing_if = "is_default")]
    pub activity_used_stage: Option<WaterStage>,
    #[serde(skip_serializing_if = "is_default")]
    pub activity_water_ledger: Option<super::activity::ActivityWaterLedger>,
    #[serde(skip_serializing_if = "is_default")]
    pub attrition_stage: Option<WaterStage>,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    value == &T::default()
}

/// Separate guard points are already removed from their parent infantry TOE.
/// Cases: land:28.11, land:28.22, airlog:51.12, airlog:51.17
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrisonerGroup {
    pub owner: Side,
    pub location: Location,
    pub prisoner_points: i32,
    pub guard_points: i32,
    pub food_stage: Option<WaterStage>,
    pub stores_short: i32,
    pub guards_fed_gt: Option<u16>,
    pub guards_stores_short: i32,
}

pub(super) fn class<'a>(
    content: &'a CnaContent,
    id: &UnitId,
) -> Result<&'a UnitClass, SupplyError> {
    content
        .units
        .units
        .get(id)
        .and_then(|oa| oa.class.as_ref())
        .and_then(|id| content.units.classes.get(id))
        .ok_or(SupplyError::Unsupported { case: "land:4.46" })
}
pub(super) fn in_play(location: &Location) -> bool {
    matches!(location, Location::Hex { .. } | Location::OffMap { .. })
}
/// Verified weapon rows represent hardware even where the OA omits a class.
/// Cases: land:4.48, airlog:52.41, airlog:52.42
pub(super) fn infantry(content: &CnaContent, id: &UnitId) -> Result<bool, SupplyError> {
    match class(content, id) {
        Ok(class) => Ok(matches!(class.unit_type.as_str(), "infantry" | "engineer")),
        Err(error) => {
            let row = content.units.units.get(id).ok_or(SupplyError::Invalid)?;
            let Some(Toe::Weapons(weapons)) = &row.toe else {
                return Err(error);
            };
            for w in weapons {
                if w.n < 0 {
                    return Err(SupplyError::Invalid);
                }
                if !content.units.weapons.contains_key(&w.weapon) {
                    return Err(SupplyError::Unsupported { case: "land:4.48" });
                }
            }
            Ok(false)
        }
    }
}
pub(super) fn pasta(content: &CnaContent, id: &UnitId) -> bool {
    content.units.units.get(id).is_some_and(|oa| {
        oa.nationality == "italian"
            && oa.echelon.as_deref().or_else(|| {
                oa.class
                    .as_ref()
                    .and_then(|c| content.units.classes.get(c))
                    .and_then(|c| c.echelon.as_deref())
            }) == Some("battalion")
    })
}

/// Weekly food needs use current strength, except the printed HQ/engineer flat rate.
/// Organizational headers with no class and no TOE are not extra mouths.
/// Cases: airlog:51.11, airlog:51.13, land:4.48
pub fn stores_required(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<i32, SupplyError> {
    let unit = state.land.units.get(id).ok_or(SupplyError::Invalid)?;
    if !in_play(&unit.location) {
        return Ok(0);
    }
    if unit.toe.is_none()
        && content
            .units
            .units
            .get(id)
            .is_some_and(|oa| oa.class.is_none())
    {
        return Ok(0);
    }
    match class(content, id) {
        Ok(class) if matches!(class.unit_type.as_str(), "headquarters" | "engineer") => {
            return Ok(1);
        }
        Ok(_) => {}
        Err(error) if !matches!(unit.toe, Some(Toe::Weapons(_))) => return Err(error),
        Err(_) => {}
    }
    toe_strength(content, unit)?
        .get()
        .checked_mul(4)
        .ok_or(SupplyError::Invalid)
}

/// Normal-weather activity demand; the weather multiplier is applied by the caller.
/// Numeric vehicle HQ points use the owner's house rule, not guessed equipment.
/// Cases: airlog:52.41, airlog:52.42, land:4.48
/// Interpretations: interp:units-0005, interp:units-0006, interp:units-0007
pub(super) fn activity_points(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<i32, SupplyError> {
    let unit = state.land.units.get(id).ok_or(SupplyError::Invalid)?;
    let body = if infantry(content, id)? {
        0
    } else if let Some(Toe::Weapons(_)) = unit.toe {
        toe_strength(content, unit)?.get()
    } else if class(content, id)?.unit_type == "headquarters" {
        // Preserve the water source-gap case; fuel's missing-rate error stays fuel-specific.
        let points = super::supply::house_rule_hq_strength(content, unit).map_err(|error| {
            if error == SupplyError::UnknownFuelRate {
                SupplyError::Unsupported {
                    case: "airlog:52.42",
                }
            } else {
                error
            }
        })?;
        if let Some(points) = points {
            points
        } else {
            if !class(content, id)?.max_toe_paren && unit.toe.is_some() {
                return Err(SupplyError::Unsupported {
                    case: "airlog:52.42",
                });
            }
            0
        }
    } else {
        toe_strength(content, unit)?.get()
    };
    let trucks = [unit.trucks.light, unit.trucks.medium, unit.trucks.heavy];
    if trucks.iter().any(|n| *n < 0) {
        return Err(SupplyError::Invalid);
    }
    trucks.into_iter().try_fold(body, |sum, n| {
        sum.checked_add(n).ok_or(SupplyError::Invalid)
    })
}

pub(super) fn hot_multiplier(
    _content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<i32, SupplyError> {
    let unit = state.land.units.get(id).ok_or(SupplyError::Invalid)?;
    if unit.location.hex().is_none() {
        return Ok(1);
    }
    let weather = state
        .turn
        .weather
        .as_ref()
        .ok_or(SupplyError::Unsupported { case: "land:29.1" })?;
    Ok(
        if weather.kind == cna_tables::land::weather::WeatherKind::Hot {
            2
        } else {
            1
        },
    )
}

/// Limits apply to voluntary movement and offensive close assault. The movement caller
/// checks its CPA and ZOC path against these flags before spending fuel or water.
/// Cases: airlog:51.23, airlog:52.51, airlog:52.52, airlog:52.6
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MovementRestrictions {
    pub may_move: bool,
    pub may_exceed_cpa: bool,
    pub may_enter_enemy_zoc: bool,
    pub may_offensive_close_assault: bool,
    pub defense_divisor: i32,
}

/// Water carried for activity remains available until the first CP expenditure.
/// Unknown composition and missing weather remain explicit errors.
/// Cases: airlog:51.23, airlog:52.41, airlog:52.42, airlog:52.51, airlog:52.52, airlog:52.6
pub fn movement_restrictions(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<MovementRestrictions, SupplyError> {
    let unit = state.land.units.get(id).ok_or(SupplyError::Invalid)?;
    let history = state.logistics.rations.get(id).cloned().unwrap_or_default();
    let stage = WaterStage::current(state);
    let multiplier = hot_multiplier(content, state, id)?;
    let foot = infantry(content, id)?;
    let infantry_dry = foot
        && !(history.water_stage == Some(stage) && history.infantry_water_received >= multiplier);
    let need = super::activity::activity_water_due(content, state, id)?;
    let reserve = state
        .logistics
        .unit_supply
        .get(id)
        .map_or(0, |s| s.activity_water.get());
    if reserve < 0 {
        return Err(SupplyError::Invalid);
    }
    let activity_dry = reserve < need;
    let half = history.issued_gt == Some(stage.game_turn) && history.half;
    let pasta_missing = pasta(content, id) && history.pasta_gt != Some(stage.game_turn);
    let dry = infantry_dry || activity_dry;
    Ok(MovementRestrictions {
        may_move: in_play(&unit.location)
            && !activity_dry
            && history.pasta_saved_cohesion_quarters.is_none()
            && !(pasta_missing && unit.cohesion_quarters <= -40),
        may_exceed_cpa: !(half || pasta_missing || infantry_dry),
        may_enter_enemy_zoc: !half,
        may_offensive_close_assault: !dry,
        defense_divisor: if dry { 2 } else { 1 },
    })
}

/// Consume the activity reserve once per OpStage, immediately before the first CPA use.
/// Validate movement eligibility separately; drawing a well may still use CP when dry.
/// Cases: airlog:52.42, airlog:52.43
pub fn spend_activity_water(
    content: &CnaContent,
    state: &mut State,
    id: &UnitId,
) -> Result<(), SupplyError> {
    super::activity::spend_activity_water(content, state, id)
}

/// Record pasta and restore the exact cohesion displaced by the pasta rule.
/// Cases: airlog:52.6
pub(super) fn receive_pasta(state: &mut State, id: &UnitId) {
    let history = state.logistics.rations.entry(id.clone()).or_default();
    history.pasta_gt = Some(state.cursor.game_turn);
    if let Some(saved) = history.pasta_saved_cohesion_quarters.take()
        && let Some(unit) = state.land.units.get_mut(id)
    {
        unit.cohesion_quarters = saved;
    }
}
/// Cases: airlog:52.6
pub(super) fn apply_pasta(content: &CnaContent, state: &mut State, id: &UnitId) {
    if !pasta(content, id) {
        return;
    }
    let history = state.logistics.rations.entry(id.clone()).or_default();
    if history.pasta_gt == Some(state.cursor.game_turn) {
        return;
    }
    if let Some(unit) = state.land.units.get_mut(id)
        && history.pasta_saved_cohesion_quarters.is_some()
    {
        unit.cohesion_quarters = unit.cohesion_quarters.min(-104);
    }
    if let Some(unit) = state.land.units.get_mut(id)
        && unit.cohesion_quarters <= -40
        && history.pasta_saved_cohesion_quarters.is_none()
    {
        history.pasta_saved_cohesion_quarters = Some(unit.cohesion_quarters);
        unit.cohesion_quarters = unit.cohesion_quarters.min(-104);
    }
}
