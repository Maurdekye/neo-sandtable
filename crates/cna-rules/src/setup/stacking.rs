//! End-of-setup capacity checks on owner-private provisional stacks.
use crate::{CnaContent, State, land::stacking, state::Location};
use cna_core::{
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::UnitId,
    visibility::Audience,
};
use cna_protocol::GameEvent;

fn provisional(state: &State, unit: &UnitId) -> State {
    let side = state.land.units[unit].side;
    let mut copy = state.clone();
    for (id, location) in &state.setup.unit_locations {
        if state.land.units[id].side == side {
            copy.land
                .units
                .get_mut(id)
                .expect("buffered setup unit")
                .location = location.clone();
        }
    }
    copy
}
/// Use strict stacking to surface the unresolved exemptions even during development.
/// Development permits only the explicitly agreed unassessed constraints, without a limit.
/// Cases: land:8.37, land:9.16, land:9.25, land:9.31, land:9.32
fn assess(
    content: &CnaContent,
    copy: &State,
    unit: &UnitId,
    strict: bool,
) -> Result<Option<String>, Rejection> {
    let u = &copy.land.units[unit];
    let Some(hex) = u.location.hex() else {
        return Ok(None);
    };
    match stacking::validate_end(content, copy, hex, u.side, true) {
        Ok(()) => Ok(None),
        Err(Rejection::Engine(EngineError::Unsupported { case, .. }))
            if !strict && matches!(case.as_str(), "land:8.37" | "land:9.16") =>
        {
            Ok(Some(case))
        }
        Err(error) => Err(error),
    }
}
/// Enumerate only capacity-valid destinations, retaining unknown constraints in dev.
/// A single temporary state is reused across the domain; enemy free choices are never read.
/// Cases: land:9.12, land:9.21, land:9.25, land:9.31, land:9.32, scen:59.2
pub(super) fn choices(
    content: &CnaContent,
    state: &State,
    unit: &UnitId,
    domain: Vec<Location>,
    strict: bool,
) -> Result<Vec<Location>, EngineError> {
    let mut copy = provisional(state, unit);
    let mut legal = Vec::new();
    for destination in domain {
        copy.land.units.get_mut(unit).expect("setup unit").location = destination.clone();
        match assess(content, &copy, unit, strict) {
            Ok(_) => legal.push(destination),
            Err(Rejection::Illegal { .. }) => {}
            Err(Rejection::Engine(error)) => return Err(error),
            Err(_) => {
                return Err(EngineError::Invariant {
                    detail: "unexpected stacking rejection".into(),
                });
            }
        }
    }
    Ok(legal)
}
/// Recheck the chosen destination against current provisional holdings before buffering it.
/// Cases: land:8.37, land:9.16, land:9.25, land:9.31, land:9.32, scen:59.2
pub(super) fn check(
    content: &CnaContent,
    state: &State,
    unit: &UnitId,
    destination: &Location,
    strict: bool,
    cx: &mut Cx<'_>,
) -> Result<(), Rejection> {
    let mut copy = provisional(state, unit);
    copy.land.units.get_mut(unit).expect("setup unit").location = destination.clone();
    if let Some(case) = assess(content, &copy, unit, strict)? {
        cx.emit(EngineEvent::new(Audience::Side(state.land.units[unit].side), GameEvent::Note {
            text: format!("Setup stacking at {} is unassessed ({case}): terrain or exemption placement is not digitized.", super::placement::destination_id(destination).expect("setup destination")),
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cna_core::{dice::CampaignRng, visibility::Perspective};
    use cna_protocol::Side;
    fn fixture() -> (CnaContent, State, Vec<UnitId>) {
        let c = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
        let mut s = State::new(&c).unwrap();
        let ids: Vec<_> = s
            .units_of(Side::Commonwealth)
            .filter(|u| {
                c.units.units[&u.id].stacking_points == Some(1)
                    && !c.units.units[&u.id].sheet.contains("garrison")
                    && c.units.units[&u.id]
                        .class
                        .as_ref()
                        .is_some_and(|id| c.units.classes[id].unit_type == "infantry")
            })
            .take(9)
            .map(|u| u.id.clone())
            .collect();
        assert_eq!(ids.len(), 9);
        for u in s.land.units.values_mut() {
            u.location = Location::NotArrived;
            u.detached = true;
        }
        (c, s, ids)
    }
    /// Cases: land:9.12, land:9.21, land:9.31, scen:59.2
    #[test]
    fn private_owner_buffers_filter_known_limits_and_never_change_world_state() {
        let (c, mut s, ids) = fixture();
        let full = Location::Hex {
            hex: "E1730".into(),
        };
        let empty = Location::Hex {
            hex: "E1830".into(),
        };
        for id in &ids[..8] {
            s.setup.unit_locations.insert(id.clone(), full.clone());
        }
        let before = serde_json::to_value(&s).unwrap();
        assert_eq!(
            choices(&c, &s, &ids[8], vec![full.clone(), empty.clone()], false).unwrap(),
            vec![empty]
        );
        let mut rng = CampaignRng::from_seed([7; 32]);
        let mut events = Vec::new();
        assert!(matches!(
            check(
                &c,
                &s,
                &ids[8],
                &full,
                false,
                &mut Cx {
                    rng: &mut rng,
                    events: &mut events
                }
            ),
            Err(Rejection::Illegal { .. })
        ));
        assert!(events.is_empty());
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        // The other side's private destinations do not contribute to this owner's capacity.
        let axis = s.units_of(Side::Axis).next().unwrap().id.clone();
        s.setup.unit_locations.insert(
            axis,
            Location::Hex {
                hex: "E1830".into(),
            },
        );
        assert_eq!(
            choices(
                &c,
                &s,
                &ids[8],
                vec![Location::Hex {
                    hex: "E1830".into()
                }],
                false
            )
            .unwrap()
            .len(),
            1
        );
    }
    /// Cases: land:8.37, land:9.16, scen:59.2
    #[test]
    fn unknown_limits_and_exemptions_are_private_dev_notes_and_full_errors() {
        let (c, s, ids) = fixture();
        let mut rng = CampaignRng::from_seed([7; 32]);
        let mut events = Vec::new();
        let unknown = Location::Hex {
            hex: "E0101".into(),
        };
        check(
            &c,
            &s,
            &ids[0],
            &unknown,
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        assert!(
            matches!(choices(&c, &s, &ids[0], vec![unknown], true), Err(EngineError::Unsupported {case,..}) if case=="land:8.37")
        );
        let garrison = UnitId::new("it.benghazi_garrison.viii_ii_engineer_bn");
        let city = Location::Hex {
            hex: "E1730".into(),
        };
        check(
            &c,
            &s,
            &garrison,
            &city,
            false,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        assert!(
            matches!(choices(&c, &s, &garrison, vec![city], true), Err(EngineError::Unsupported {case,..}) if case=="land:9.16")
        );
        assert_eq!(events.len(), 2);
        assert!(!matches!(events[0].audience, Audience::Public));
        assert!(!Perspective::Side(Side::Axis).can_see(&events[0].audience));
        assert!(!Perspective::Side(Side::Commonwealth).can_see(&events[1].audience));
    }
}
