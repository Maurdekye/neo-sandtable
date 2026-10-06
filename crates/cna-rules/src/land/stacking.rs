//! Terrain and road capacity count represented formations, including their shell equivalents.
use super::formation;
use crate::steps::illegal;
use crate::{CnaContent, State};
use cna_content::map::Survey;
use cna_core::{
    engine::{EngineError, Rejection},
    ids::{HexId, UnitId},
};
use cna_protocol::Side;
use cna_tables::land::terrain::{StackingLimit, TerrainFeature as F};

/// Count one represented counter per root. Attached first-line trucks add no points.
/// Cases: land:9.11, land:9.12, land:9.13, land:9.21, land:9.29
pub fn halves(content: &CnaContent, state: &State, hex: &HexId, side: Side) -> i32 {
    formation::roots(content, state, hex, side)
        .iter()
        .map(|id| formation::stacking_halves(content, state, id))
        .sum()
}
/// Network transit is limited to five stacking points for vehicles. Off-road counters do not count.
/// Cases: land:8.34, land:9.29, land:9.33, land:9.34
pub fn road_halves(
    content: &CnaContent,
    state: &State,
    hex: &HexId,
    side: Side,
    excluded: &[UnitId],
) -> i32 {
    formation::roots(content, state, hex, side)
        .iter()
        .filter(|id| !excluded.contains(id) && !state.land.movement.off_road.contains(id))
        .map(|id| formation::stacking_halves(content, state, id))
        .sum()
}
/// A unit may pass an overfull ordinary hex, but may never finish its move there.
/// Zero-point independent companies/batteries have a separate five-counter limit.
/// Cases: land:9.14, land:9.25, land:9.31, land:9.32
/// Cases: land:9.16
/// Unsupported: land:9.16 - garrison assignments and airfield exemptions need placement data.
pub fn validate_end(
    content: &CnaContent,
    state: &State,
    hex: &HexId,
    side: Side,
    strict: bool,
) -> Result<(), Rejection> {
    let terrain: F = match content.map.terrain_survey(hex) {
        Survey::Present(name) => serde_json::from_value(serde_json::Value::String(name.into()))
            .map_err(|_| illegal("terrain class has no stacking limit"))?,
        _ => {
            return Err(Rejection::Engine(EngineError::Unsupported {
                case: "land:8.37".into(),
                detail: "terrain not yet digitized".into(),
            }));
        }
    };
    let limit = match content
        .tables
        .land
        .terrain_effects
        .feature(terrain)
        .stacking_limit
    {
        StackingLimit::Points(n) => n * 2,
        _ => return Err(illegal("terrain class has no stacking limit")),
    };
    let roots = formation::roots(content, state, hex, side);
    let zero = roots
        .iter()
        .filter(|id| {
            content.units.units[*id].stacking_points == Some(0)
                && formation::class(content, id).is_some_and(|c| c.unit_type != "headquarters")
        })
        .count();
    let mut counted = 0;
    for id in &roots {
        let class = formation::class(content, id);
        let aa = class.is_some_and(|c| c.unit_type == "anti_air");
        let immobile_plus = class.is_some_and(|c| c.cpa == 0 && c.cpa_plus)
            && state.land.units[id].transport_trucks.total() == 0;
        if immobile_plus || (aa && terrain == F::MajorCity) {
            continue;
        }
        let garrison = content.units.units[id].sheet.contains("garrison");
        if strict && (garrison || aa) {
            return Err(Rejection::Engine(EngineError::Unsupported {
                case: "land:9.16".into(),
                detail: "garrison/airfield stacking exemption placement is not digitized".into(),
            }));
        }
        counted += formation::stacking_halves(content, state, id);
    }
    if counted > limit || (terrain != F::MajorCity && zero > 5) {
        return Err(illegal("destination exceeds the stacking limit"));
    }
    Ok(())
}
