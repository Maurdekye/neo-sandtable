//! Source-backed initial air facility identities and readying capacities.
use crate::{CnaContent, state::Location};
use cna_content::scenario::Placement;
use cna_core::engine::EngineError;
use cna_protocol::Side;

#[derive(Clone, Debug)]
pub(super) struct Facility {
    pub id: String,
    pub force: String,
    pub side: Side,
    pub location: Location,
}
#[derive(Default)]
pub(super) struct Catalog {
    pub facilities: Vec<Facility>,
    pub unresolved: Vec<String>,
}
fn unsupported(detail: impl Into<String>) -> EngineError {
    EngineError::Unsupported {
        case: "scen:59.35".into(),
        detail: detail.into(),
    }
}
/// Cases: scen:60.5, scen:60.46, airlog:36.12, airlog:36.2, airlog:36.3, airlog:36.4, airlog:36.5
pub(super) fn catalog(content: &CnaContent) -> Result<Catalog, EngineError> {
    let mut out = Catalog::default();
    for f in &content.scenario.facilities.facilities {
        let side = match f.owner.as_deref() {
            Some("axis") => Side::Axis,
            Some("commonwealth") => Side::Commonwealth,
            _ => {
                out.unresolved.push(format!(
                    "{}: initial national ownership is not verified",
                    f.id
                ));
                continue;
            }
        };
        let force = f.theatre.clone().unwrap_or_else(|| {
            match side {
                Side::Axis => "axis",
                Side::Commonwealth => "commonwealth",
            }
            .into()
        });
        let mut locations = Vec::new();
        for hex in f.hex.iter().chain(&f.hexes) {
            let h = content
                .map
                .canonical(hex)
                .ok_or_else(|| unsupported(format!("{}: unknown facility hex", f.id)))?;
            locations.push(Location::Hex { hex: h.clone() });
        }
        if let Some(id) = &f.location {
            if content.areas.locations.get(id).is_some_and(|l| l.off_map) {
                locations.push(Location::OffMap { id: id.clone() });
            } else {
                out.unresolved
                    .push(format!("{}: off-map location is not verified", f.id));
                continue;
            }
        }
        if let Some(area) = &f.location_area {
            match super::placement::choices(
                content,
                &Placement::Area {
                    area: area.clone(),
                    exclusion: None,
                },
                side,
                "scen:59.35",
            ) {
                Ok(list) => locations.extend(list),
                Err(EngineError::Unsupported { detail, .. }) => {
                    out.unresolved.push(format!("{}: {detail}", f.id));
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        if locations.is_empty() {
            out.unresolved
                .push(format!("{}: facility has no verified location", f.id));
            continue;
        }
        let many = locations.len() > 1;
        for location in locations {
            let id = if many {
                format!(
                    "{}@{}",
                    f.id,
                    super::placement::destination_id(&location).expect("facility location")
                )
            } else {
                f.id.clone()
            };
            out.facilities.push(Facility {
                id,
                force: force.clone(),
                side,
                location,
            });
        }
    }
    out.facilities.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}
