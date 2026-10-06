//! OpStage weather, hot-weather stock loss, and rain refilling depleted wells.
//! Movement, construction and air callers query [`at_hex`] for local conditions.

use cna_content::scenario::Supplies;
use cna_core::engine::{Cx, EngineError};
use cna_core::event::EngineEvent;
use cna_core::ids::HexId;
use cna_protocol::GameEvent;
use cna_tables::land::weather::{MapSection, WeatherKind};

use crate::content::CnaContent;
use crate::state::{DumpLocation, State, WeatherState};

/// Roll the seasonal chart and, for either storm, the affected sections. Hot
/// weather reduces on-map cargo and dump fuel/water; tanks remain separate.
/// Rain replenishes depleted wells only where that storm occurs.
/// Cases: land:29.1, land:29.34, land:29.53, airlog:49.3, airlog:52.44
/// Interpretations: interp:land-0019
pub fn determine(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let turn = i32::from(state.cursor.game_turn);
    if content.tables.land.weather.season(turn).is_none() {
        return Err(EngineError::Unsupported {
            case: "land:29.6".into(),
            detail: "no weather row for this game-turn".into(),
        });
    }
    let reading = cx.rng.two_dice_reading();
    let kind = content
        .tables
        .land
        .weather
        .result(turn, reading)
        .ok_or_else(|| EngineError::Invariant {
            detail: "validated weather table has no cell for a dice reading".into(),
        })?;
    cx.emit(EngineEvent::public(GameEvent::DiceRolled {
        purpose: "OpStage weather".into(),
        dice: vec![reading.tens.value(), reading.units.value()],
        reading: Some(reading.value()),
        rule: Some("land:29.1".into()),
    }));
    let storm_sections = if matches!(kind, WeatherKind::Sandstorm | WeatherKind::Rainstorm) {
        let die = cx.rng.d6();
        cx.emit(EngineEvent::public(GameEvent::DiceRolled {
            purpose: "Storm location".into(),
            dice: vec![die.value()],
            reading: None,
            rule: Some("land:29.7".into()),
        }));
        content
            .tables
            .land
            .foul_weather_location
            .sections(die)
            .to_vec()
    } else {
        Vec::new()
    };
    let weather = WeatherState {
        kind,
        storm_sections,
    };
    let mut next = state.logistics.clone();
    match weather.kind {
        WeatherKind::Hot => {
            for dump in next.dumps.values_mut() {
                if matches!(dump.location, DumpLocation::Hex { .. }) {
                    hot_stock_loss(&mut dump.supplies)?;
                }
            }
            for (id, holdings) in &mut next.unit_supply {
                if state
                    .land
                    .units
                    .get(id)
                    .is_some_and(|u| u.location.hex().is_some())
                {
                    hot_stock_loss(&mut holdings.carried)?;
                }
            }
        }
        WeatherKind::Rainstorm => {
            for (hex, well) in &mut next.wells {
                if local_weather(content, &weather, hex)? == WeatherKind::Rainstorm {
                    well.depleted = false;
                }
            }
        }
        _ => {}
    }
    let name = match kind {
        WeatherKind::Normal => "normal weather",
        WeatherKind::Hot => "hot weather",
        WeatherKind::Sandstorm => "sandstorm",
        WeatherKind::Rainstorm => "rainstorm",
    };
    let text = if weather.storm_sections.is_empty() {
        format!("OpStage weather: {name}.")
    } else {
        let sections = weather
            .storm_sections
            .iter()
            .map(|s| section_letter(*s).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!("OpStage weather: {name} in map sections {sections}.")
    };
    state.logistics = next;
    state.turn.weather = Some(weather);
    cx.emit(EngineEvent::public(GameEvent::Note { text }));
    Ok(())
}

/// Whole cargo-point losses, rounding the five-percent loss downward.
/// Cases: land:29.34, airlog:49.3, airlog:52.44
fn hot_stock_loss(stock: &mut Supplies) -> Result<(), EngineError> {
    if stock.fuel < 0 || stock.water < 0 {
        return Err(EngineError::Invariant {
            detail: "negative fuel or water holding".into(),
        });
    }
    stock.fuel -= (i64::from(stock.fuel) * 5 / 100) as i32;
    stock.water -= (i64::from(stock.water) * 5 / 100) as i32;
    Ok(())
}

/// Conditions in a canonical or alias hex. Sandstorms exclude delta and sea;
/// rainstorms include the sea. Sections outside a storm retain normal weather.
/// An absent roll or an unclassified affected sandstorm hex is unsupported.
/// Cases: land:29.41, land:29.46, land:29.51, land:29.52
pub fn at_hex(
    content: &CnaContent,
    state: &State,
    hex: &HexId,
) -> Result<WeatherKind, EngineError> {
    let weather = state
        .turn
        .weather
        .as_ref()
        .ok_or_else(|| EngineError::Unsupported {
            case: "land:29.1".into(),
            detail: "weather has not been determined".into(),
        })?;
    local_weather(content, weather, hex)
}

fn local_weather(
    content: &CnaContent,
    weather: &WeatherState,
    hex: &HexId,
) -> Result<WeatherKind, EngineError> {
    let record = content
        .map
        .canonical(hex)
        .and_then(|h| content.map.get(h))
        .ok_or_else(|| EngineError::Invariant {
            detail: "unknown weather hex".into(),
        })?;
    local_weather_for_record(weather, record)
}

fn local_weather_for_record(
    weather: &WeatherState,
    record: &cna_content::map::HexRecord,
) -> Result<WeatherKind, EngineError> {
    if !matches!(
        weather.kind,
        WeatherKind::Sandstorm | WeatherKind::Rainstorm
    ) {
        return Ok(weather.kind);
    }
    if !weather
        .storm_sections
        .iter()
        .any(|s| section_letter(*s) == record.section)
    {
        return Ok(WeatherKind::Normal);
    }
    if weather.kind == WeatherKind::Sandstorm {
        let terrain = record
            .terrain
            .as_deref()
            .ok_or_else(|| EngineError::Unsupported {
                case: "land:29.41".into(),
                detail: "sandstorm hex has no terrain classification".into(),
            })?;
        if matches!(terrain, "delta" | "sea")
            || record.flags.iter().any(|f| f == "sea" || f == "delta")
        {
            return Ok(WeatherKind::Normal);
        }
    }
    Ok(weather.kind)
}

fn section_letter(section: MapSection) -> char {
    match section {
        MapSection::A => 'A',
        MapSection::B => 'B',
        MapSection::C => 'C',
        MapSection::D => 'D',
        MapSection::E => 'E',
    }
}

#[cfg(test)]
mod tests;
