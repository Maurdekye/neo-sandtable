//! Map-to-TEC movement inputs. Unknown feature layers remain explicit in the price.
use super::{
    formation,
    terrain::{self, Entry, Route},
};
use crate::steps::illegal;
use crate::{CnaContent, State};
use cna_content::map::{LineKind, SideKind, Survey};
use cna_core::{
    engine::{EngineError, Rejection},
    ids::{HexId, UnitId},
};
use cna_tables::land::terrain::TerrainFeature as F;

#[derive(Debug, Clone, Copy)]
pub struct StepCost {
    pub cp_quarters: i32,
    pub breakdown_quarters: i32,
    pub light_extra_quarters: i32,
    pub on_network: bool,
    pub assumed_edges: bool,
}
fn unsupported(case: &str, detail: &str) -> Rejection {
    Rejection::Engine(EngineError::Unsupported {
        case: case.into(),
        detail: detail.into(),
    })
}
/// Require a digitized base class before weather or terrain-dependent planning.
/// Cases: land:8.37
pub fn terrain(content: &CnaContent, to: &HexId, strict: bool) -> Result<F, Rejection> {
    match content.map.terrain_survey(to) {
        Survey::Present("sea") => Err(illegal("not a legal destination")),
        Survey::Present(name) => {
            serde_json::from_value::<F>(serde_json::Value::String(name.into()))
                .map_err(|_| unsupported("land:8.37", "terrain class is not supported"))
        }
        _ => Err(if strict {
            unsupported("land:8.37", "terrain not yet digitized")
        } else {
            illegal("terrain not yet digitized")
        }),
    }
}

/// Translate base terrain and directional features; roads require a verified shared-edge connection.
/// Unknown edge kinds under dev use plain terrain with an assumption flag; callers emit the Note.
/// Cases: land:8.13, land:8.19, land:8.31, land:8.32, land:8.33, land:8.35, land:8.37
/// Cases: land:8.41, land:8.42, land:8.43, land:8.44, land:8.45, land:8.46, land:8.47
/// Cases: land:29.57, land:29.58
/// Interpretations: interp:land-0002
pub fn step_cost(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    from: &HexId,
    to: &HexId,
    strict: bool,
    rainstorm: bool,
) -> Result<StepCost, Rejection> {
    step_cost_with_network(content, state, id, from, to, strict, rainstorm, true)
}

/// Road congestion requires an explicit price without network benefits.
/// Cases: land:8.34, land:9.33
#[allow(clippy::too_many_arguments)]
pub fn step_cost_with_network(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
    from: &HexId,
    to: &HexId,
    strict: bool,
    rainstorm: bool,
    use_network: bool,
) -> Result<StepCost, Rejection> {
    let unit = state
        .land
        .units
        .get(id)
        .ok_or_else(|| illegal("unknown unit"))?;
    let allowance = formation::individual_allowance(content, state, id).ok_or_else(|| {
        unsupported(
            "land:8.91",
            "unit transport or movement rating is unresolved",
        )
    })?;
    let c = formation::class(content, id).ok_or_else(|| illegal("unit has no movement class"))?;
    price(
        content,
        unit.side,
        &unit.trucks,
        from,
        to,
        strict,
        rainstorm,
        use_network,
        allowance.motorized,
        c.unit_type == "recce",
        c.equipment_note
            .as_deref()
            .is_some_and(|s| s.contains("motorcycle")),
    )
}

/// Convoys use their actual physical trucks, without a fabricated combat-unit class.
/// Every present type must be able to traverse the edge; mixed carriers share its price.
/// Cases: airlog:53.12, airlog:53.21, airlog:54.2, land:8.37, land:8.44, land:8.48
#[allow(clippy::too_many_arguments)]
pub fn truck_step_cost(
    content: &CnaContent,
    side: cna_protocol::Side,
    trucks: &cna_content::units::Trucks,
    from: &HexId,
    to: &HexId,
    strict: bool,
    rainstorm: bool,
) -> Result<StepCost, Rejection> {
    let counts = [trucks.light, trucks.medium, trucks.heavy];
    if counts.iter().any(|n| *n < 0) || counts.iter().all(|n| *n == 0) {
        return Err(illegal("convoy has no valid trucks"));
    }
    price(
        content, side, trucks, from, to, strict, rainstorm, true, true, false, false,
    )
}

#[allow(clippy::too_many_arguments)]
fn price(
    content: &CnaContent,
    side: cna_protocol::Side,
    trucks: &cna_content::units::Trucks,
    from: &HexId,
    to: &HexId,
    strict: bool,
    rainstorm: bool,
    use_network: bool,
    motorized: bool,
    recce: bool,
    motorcycle: bool,
) -> Result<StepCost, Rejection> {
    let (a, b) = content
        .map
        .get(from)
        .zip(content.map.get(to))
        .ok_or_else(|| illegal("not a legal destination"))?;
    if a.axial.distance(b.axial) != 1 {
        return Err(illegal("path must follow adjacent hexes"));
    }
    if side == cna_protocol::Side::Commonwealth
        && content
            .map
            .get(&"A2109".into())
            .is_some_and(|arch| b.axial.q * 2 + b.axial.r < arch.axial.q * 2 + arch.axial.r)
    {
        return Err(illegal(
            "Commonwealth land units may not move west of Marble Arch",
        ));
    }
    let terrain = terrain(content, to, strict)?;
    let light = trucks.light > 0;
    let mut unknown = false;
    let mut route = Route::Plain;
    for (kind, candidate) in [
        (LineKind::UnfinishedRailroad, Route::Plain),
        (LineKind::Railroad, Route::Railroad),
        (LineKind::UnfinishedRoad, Route::UnfinishedRoad),
        (LineKind::Track, Route::Track),
        (LineKind::Road, Route::Road),
    ] {
        match content.map.line(from, to, kind) {
            Survey::Unknown => unknown = true,
            Survey::Present(()) => route = candidate,
            Survey::Absent => {}
        }
    }
    let mut features = Vec::new();
    for kind in SideKind::ALL {
        match content.map.hexside(from, to, kind) {
            Survey::Unknown => unknown = true,
            Survey::Absent => {}
            Survey::Present(feature) => {
                let up = feature.high_side.as_ref() == Some(&b.id);
                match kind {
                    SideKind::AllSea => return Err(illegal("not a legal destination")),
                    SideKind::Border => {}
                    SideKind::Slope => features.push(if up { F::UpSlope } else { F::DownSlope }),
                    SideKind::Escarpment => features.push(if up {
                        F::UpEscarpment
                    } else {
                        F::DownEscarpment
                    }),
                    SideKind::Ridge => features.push(F::Ridge),
                    SideKind::Wadi => features.push(F::Wadi),
                    SideKind::MajorRiver => features.push(F::MajorRiver),
                    SideKind::MinorRiver => features.push(F::MinorRiver),
                }
            }
        }
    }
    if unknown && strict {
        return Err(unsupported(
            "land:8.37",
            "road, track or hexside coverage is incomplete",
        ));
    }
    // The authorized dev assumption is plain terrain when an edge's feature layers are incomplete.
    // Still honor known prohibitions (e.g. all-sea/escarpment), never fabricate a network benefit.
    if unknown || !use_network {
        route = Route::Plain;
    }
    if rainstorm {
        let bridge = matches!(route, Route::Road | Route::Railroad);
        if !bridge
            && features
                .iter()
                .any(|f| matches!(f, F::MinorRiver | F::MajorRiver))
        {
            return Err(illegal("river crossing is prohibited during rainstorm"));
        }
        if motorized && !bridge && (terrain == F::Delta || a.terrain.as_deref() == Some("delta")) {
            return Err(illegal(
                "vehicle cannot move through delta off road or rail during rainstorm",
            ));
        }
    }
    let network = matches!(route, Route::Road | Route::Track | Route::UnfinishedRoad);
    if (trucks.medium > 0 || trucks.heavy > 0)
        && !network
        && (terrain == F::SaltMarsh || a.terrain.as_deref() == Some("salt_marsh"))
    {
        return Err(illegal(
            "attached trucks cannot enter or leave salt marsh off network",
        ));
    }
    let entry = Entry {
        terrain,
        route,
        hexsides: &features,
        rainstorm,
        motorized,
        salt_marsh_exception: light || recce || motorcycle,
        desert_prohibited: light || motorcycle,
    };
    if motorized
        && a.terrain.as_deref() == Some("salt_marsh")
        && !entry.salt_marsh_exception
        && !matches!(route, Route::Road | Route::Track | Route::UnfinishedRoad)
    {
        return Err(illegal(
            "unit cannot leave salt marsh off the road or track",
        ));
    }
    let cp = terrain::entry_cost(&content.tables.land.terrain_effects, entry)
        .map_err(|_| illegal("unit cannot cross this terrain"))?;
    Ok(StepCost {
        cp_quarters: cp,
        breakdown_quarters: terrain::entry_breakdown(&content.tables.land.terrain_effects, entry)
            .map_err(|_| {
            unsupported("land:21.21", "breakdown terrain value is unresolved")
        })?,
        on_network: matches!(route, Route::Road | Route::Track | Route::UnfinishedRoad),
        light_extra_quarters: content
            .tables
            .airlog
            .truck_characteristics
            .light_breakdown_extra_quarters(
                route == Route::Road,
                i32::try_from(features.len())
                    .map_err(|_| illegal("breakdown exposure overflow"))?,
            )
            .ok_or_else(|| illegal("breakdown exposure overflow"))?,
        assumed_edges: unknown,
    })
}

#[cfg(test)]
#[path = "map_truck_tests.rs"]
mod truck_tests;
