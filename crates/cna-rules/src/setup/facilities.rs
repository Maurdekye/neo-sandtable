//! Setup reporting delegates to the canonical Air source catalog.
use crate::CnaContent;
use cna_core::{
    engine::{Cx, EngineError},
    event::EngineEvent,
    ids::SeatId,
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role, Side};

pub(super) use crate::air::facilities::{Facility, catalog, compatible};

fn unsupported(detail: impl Into<String>) -> EngineError {
    EngineError::Unsupported {
        case: "scen:59.35".into(),
        detail: detail.into(),
    }
}

/// Unknown ownership/locations remain unavailable; only the owner receives development notes.
/// Cases: scen:59.35, scen:60.5
pub(super) fn report(
    content: &CnaContent,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let data = catalog(content)?;
    if strict && let Some(issue) = data.unresolved.first() {
        return Err(unsupported(issue));
    }
    if !data.unresolved.is_empty() {
        for side in [Side::Axis, Side::Commonwealth] {
            cx.emit(EngineEvent::new(Audience::Seat(SeatId::new(side,Role::Air)),GameEvent::Note {
                text:format!("Some initial air facilities are unavailable until their ownership or location is verified (scen:59.35): {}.",data.unresolved.join("; ")),
            }));
        }
    }
    Ok(())
}
