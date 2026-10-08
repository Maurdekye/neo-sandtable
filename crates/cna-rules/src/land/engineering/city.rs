//! Positive city witnesses only; this never establishes a current fortification level.
use crate::CnaContent;
use cna_content::places::Place;
use cna_core::{engine::EngineError, ids::HexId};

struct CityWitness {
    id: &'static str,
    hex: &'static str,
    group: Option<&'static str>,
    review: &'static str,
    required: &'static [&'static str],
    level: u8,
}
const fn witness(
    id: &'static str,
    hex: &'static str,
    group: Option<&'static str>,
    review: &'static str,
    required: &'static [&'static str],
    level: u8,
) -> CityWitness {
    CityWitness {
        id,
        hex,
        group,
        review,
        required,
        level,
    }
}
const ALEXANDRIA: &[&str] = &["land:8.37", "scen:60.41", "scen:60.5"];
const CAIRO: &[&str] = &["land:8.37", "land:17.32", "scen:60.43"];
// Stable per-cell identity reviewed by the map owner; no prefix/name/extent resolver.
const WITNESSES: [CityWitness; 9] = [
    witness(
        "city-alexandria-e3613",
        "E3613",
        None,
        "alexandria-positive-0001",
        ALEXANDRIA,
        3,
    ),
    witness(
        "city-alexandria-e3714",
        "E3714",
        None,
        "alexandria-positive-0001",
        ALEXANDRIA,
        3,
    ),
    witness(
        "city-bardia-c4321",
        "C4321",
        Some("bardia"),
        "bardia-north-0001",
        &["land:8.37", "scen:60.31"],
        2,
    ),
    witness(
        "city-benghazi-a4827",
        "A4827",
        Some("benghazi"),
        "benghazi-0001",
        &["land:8.37", "scen:60.34", "interp:scen-0001"],
        2,
    ),
    witness(
        "city-cairo-e1730",
        "E1730",
        Some("cairo"),
        "cairo-0001",
        CAIRO,
        3,
    ),
    witness(
        "city-cairo-e1829",
        "E1829",
        Some("cairo"),
        "cairo-0001",
        CAIRO,
        3,
    ),
    witness(
        "city-cairo-e1830",
        "E1830",
        Some("cairo"),
        "cairo-0001",
        CAIRO,
        3,
    ),
    witness(
        "city-cairo-e1930",
        "E1930",
        Some("cairo"),
        "cairo-0001",
        CAIRO,
        3,
    ),
    witness(
        "city-cairo-e1931",
        "E1931",
        Some("cairo"),
        "cairo-0001",
        CAIRO,
        3,
    ),
];
fn by_id(id: &str) -> Option<&'static CityWitness> {
    WITNESSES.iter().find(|w| w.id == id)
}
fn invariant(hex: &HexId, detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: format!("city source {hex}: {detail}"),
    }
}
fn unsupported(hex: &HexId, detail: &str) -> EngineError {
    EngineError::Unsupported {
        case: "land:25.12".into(),
        detail: format!("city source {hex}: {detail}"),
    }
}
fn validate_selected(
    content: &CnaContent,
    hex: &HexId,
    key: &str,
    place: &Place,
) -> Result<&'static CityWitness, EngineError> {
    let invalid = || invariant(hex, &format!("malformed selected city witness {key}"));
    if key.is_empty()
        || key != place.id
        || place.name.is_empty()
        || place.kind.is_empty()
        || place.review_batch.is_empty()
        || place.src.is_empty()
        || place.src.iter().any(String::is_empty)
    {
        return Err(invalid());
    }
    let anchor = content
        .map
        .canonical(&place.hex_id)
        .filter(|h| content.map.get(h).is_some())
        .ok_or_else(invalid)?;
    let Some(w) = by_id(key) else {
        return Err(unsupported(
            hex,
            &format!("unreviewed selected city identity {key}"),
        ));
    };
    if anchor.as_str() != w.hex
        || anchor != hex
        || place.kind != "major_city"
        || place.place_group.as_deref() != w.group
        || place.review_batch != w.review
        || w.required
            .iter()
            .any(|case| !place.src.iter().any(|s| s.as_str() == *case))
    {
        return Err(invalid());
    }
    Ok(w)
}
/// Undamaged intrinsic city source level, not current state or a construction ceiling.
/// Only reviewed positive witnesses authenticate a cell. Missing evidence remains unknown;
/// callers must separately resolve damage, explicit zero and legacy/current-state history.
/// Cases: land:25.12, land:8.37
/// Interpretations: interp:land-0002
pub fn source_city_fortification_level(
    content: &CnaContent,
    hex: &HexId,
) -> Result<u8, EngineError> {
    let canonical = content
        .map
        .canonical(hex)
        .filter(|h| content.map.get(h).is_some())
        .ok_or_else(|| invariant(hex, "requested hex is not existing map geometry"))?;
    let expected = WITNESSES.iter().find(|w| w.hex == canonical.as_str());
    let mut malformed = None;
    let mut unresolved = None;
    let mut known = None;
    for (key, place) in &content.places.places {
        let expected_id = expected.is_some_and(|w| key == w.id || place.id == w.id);
        let anchored_here = content.map.canonical(&place.hex_id) == Some(canonical);
        let recognized = by_id(key).is_some() || by_id(&place.id).is_some();
        if !expected_id && !(anchored_here && (place.kind == "major_city" || recognized)) {
            continue;
        }
        match validate_selected(content, canonical, key, place) {
            Ok(w) => {
                let identity = (w.group, w.level);
                if known.is_some_and(|prior| prior != identity) {
                    malformed.get_or_insert_with(|| {
                        invariant(canonical, "conflicting authenticated city witnesses")
                    });
                } else {
                    known = Some(identity);
                }
            }
            Err(error @ EngineError::Invariant { .. }) => {
                malformed.get_or_insert(error);
            }
            Err(error) => {
                unresolved.get_or_insert(error);
            }
        }
    }
    if let Some(error) = malformed.or(unresolved) {
        return Err(error);
    }
    known
        .map(|(_, level)| level)
        .ok_or_else(|| unsupported(canonical, "missing positive city identity"))
}

#[cfg(test)]
mod tests;
