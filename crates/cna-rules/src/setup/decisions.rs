//! Placement and first-line distribution in the shared blind setup window.
use super::{SetupTask, placement};
use crate::state::{DumpLocation, Location, Pending};
use crate::steps::{illegal, open};
use crate::{CnaContent, State};
use cna_content::scenario::Placement;
use cna_content::units::Trucks;
use cna_core::decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger};
use cna_core::engine::{Cx, EngineError, Rejection};
use cna_core::event::EngineEvent;
use cna_core::ids::{SeatId, UnitId};
use cna_core::visibility::Audience;
use cna_protocol::{GameEvent, Role, Side};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const KIND_UNIT: &str = "cna.setup.unit";
pub(crate) const KIND_DUMP: &str = "cna.setup.dump";
pub(crate) const KIND_TRUCKS: &str = "cna.setup.first_line_trucks";

fn invariant(detail: impl Into<String>) -> EngineError {
    EngineError::Invariant {
        detail: detail.into(),
    }
}
fn role(content: &CnaContent, case: &str, fallback: Role) -> Role {
    match content.registry.cases.get(case).map(|c| c.seat.as_str()) {
        Some("commander" | "naval") => Role::Commander,
        Some("front_line") => Role::FrontLine,
        Some("rear_area") => Role::RearArea,
        Some("logistics") => Role::Logistics,
        Some("air") => Role::Air,
        _ => fallback,
    }
}
fn source_case(src: &[String], fallback: &str) -> String {
    src.iter()
        .find(|s| s.starts_with("scen:"))
        .cloned()
        .unwrap_or_else(|| fallback.into())
}
fn group_placement(
    content: &CnaContent,
    state: &State,
    id: &UnitId,
) -> Result<(Placement, String), EngineError> {
    let unit = state
        .land
        .units
        .get(id)
        .ok_or_else(|| invariant("unknown setup unit"))?;
    let group = unit
        .setup_group
        .as_ref()
        .ok_or_else(|| invariant("setup unit has no group"))?;
    let g = content
        .scenario
        .land
        .iter()
        .flat_map(|f| &f.groups)
        .find(|g| &g.id == group)
        .ok_or_else(|| invariant("setup group absent from content"))?;
    Ok((g.placement.clone(), source_case(&g.src, "scen:59.2")))
}
fn option_space(domain: &[Location]) -> ActionSpace {
    ActionSpace::new(ActionSchema::Choice {
        options: domain
            .iter()
            .map(|l| {
                let id = placement::destination_id(l).expect("geographic domain");
                ChoiceOption {
                    label: id.clone(),
                    id,
                    detail: None,
                }
            })
            .collect(),
    })
}
#[allow(clippy::too_many_arguments)]
fn open_task(
    state: &mut State,
    cx: &mut Cx<'_>,
    seat: SeatId,
    kind: &str,
    summary: String,
    case: &str,
    task: SetupTask,
    space: ActionSpace,
) {
    open(
        state,
        cx,
        seat,
        kind,
        summary,
        &[case],
        Trigger::Scheduled,
        Secrecy::SecretSimultaneous,
        space,
    );
    let id = state
        .decisions
        .pending
        .last()
        .expect("opened setup decision")
        .id
        .clone();
    state.setup.tasks.insert(id, task);
}
fn same_hex(a: &Location, b: &Location) -> bool {
    matches!((a.hex(), b.hex()), (Some(x), Some(y)) if x == y)
}
fn enemy_fixed_at(state: &State, side: Side, destination: &Location) -> bool {
    state
        .units_of(side.opponent())
        .any(|u| same_hex(&u.location, destination))
}
fn enemy_buffered_at(state: &State, side: Side, destination: &Location) -> bool {
    state.setup.unit_locations.iter().any(|(id, l)| {
        state
            .land
            .units
            .get(id)
            .is_some_and(|u| u.side == side.opponent())
            && same_hex(l, destination)
    })
}
fn facility_hexes(content: &CnaContent) -> BTreeSet<cna_core::ids::HexId> {
    content
        .scenario
        .facilities
        .facilities
        .iter()
        .flat_map(|f| f.hex.iter().chain(&f.hexes))
        .filter_map(|h| content.map.canonical(h).cloned())
        .collect()
}
fn resolved_destination(domain: &[Location], action: &Value) -> Result<Location, Rejection> {
    let chosen = action
        .as_str()
        .ok_or_else(|| illegal("choose a destination id"))?;
    domain
        .iter()
        .find(|l| placement::destination_id(l).as_deref() == Some(chosen))
        .cloned()
        .ok_or_else(|| illegal("not a legal setup destination"))
}

/// Open the owning seats' choices without publishing free destinations.
/// TODO land:9 - stacking awaits its procedure; no numerical limit is fabricated here.
/// Unsupported: scen:59.33 - squadron setup is still being implemented.
/// Unsupported: scen:59.43 - air convoy setup is still being implemented.
/// Unsupported: scen:59.44 - supply convoy setup is still being implemented.
/// Cases: scen:59.2, scen:59.42, scen:59.53, scen:60.31, scen:60.34, scen:60.41, scen:60.44, land:8.13
/// Interpretations: interp:scen-0005
pub(crate) fn enter(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<(), EngineError> {
    if state.setup.started {
        return Err(invariant("setup window already started"));
    }
    state.setup.started = true;
    let units: Vec<_> = state
        .land
        .units
        .values()
        .filter(|u| matches!(u.location, Location::AwaitingSetup { .. }))
        .map(|u| (u.id.clone(), u.side))
        .collect();
    let mut opened_groups = BTreeSet::new();
    for (id, side) in units {
        let group = state.land.units[&id]
            .setup_group
            .clone()
            .ok_or_else(|| invariant("setup unit lacks group"))?;
        if !opened_groups.insert(group) {
            continue;
        }
        let (p, case) = group_placement(content, state, &id)?;
        let Some(mut domain) = placement::for_profile(content, &p, side, &case, strict, cx)? else {
            continue;
        };
        domain.retain(|l| !enemy_fixed_at(state, side, l));
        if domain.is_empty() {
            return Err(EngineError::Unsupported {
                case,
                detail: "no unoccupied setup destination".into(),
            });
        }
        open_task(
            state,
            cx,
            SeatId::new(side, role(content, &case, Role::Commander)),
            KIND_UNIT,
            format!("Place {id} within its scenario setup area."),
            &case,
            SetupTask::Unit {
                unit: id,
                case: case.clone(),
            },
            option_space(&domain),
        );
    }
    let dumps: Vec<_> = state
        .logistics
        .dumps
        .values()
        .filter_map(|d| match &d.location {
            DumpLocation::AwaitingSetup { placement } => {
                Some((d.id.clone(), d.side, d.dummy, placement.clone()))
            }
            _ => None,
        })
        .collect();
    let facility_hexes = facility_hexes(content);
    for (id, side, dummy, p) in dumps {
        let case = if dummy {
            "scen:59.53".into()
        } else {
            source_case(
                &content
                    .scenario
                    .supply
                    .dumps
                    .iter()
                    .find(|d| d.id == id)
                    .ok_or_else(|| invariant("setup dump absent from content"))?
                    .src,
                "scen:59.51",
            )
        };
        let Some(mut domain) = placement::for_profile(content, &p, side, &case, strict, cx)? else {
            continue;
        };
        if dummy {
            domain.retain(|l| l.hex().is_none_or(|h| !facility_hexes.contains(h)));
        }
        if domain.is_empty() {
            return Err(EngineError::Unsupported {
                case,
                detail: "no permitted dump destination".into(),
            });
        }
        open_task(
            state,
            cx,
            SeatId::new(side, role(content, &case, Role::Logistics)),
            KIND_DUMP,
            format!(
                "Place {} {id}.",
                if dummy { "dummy dump" } else { "supply dump" }
            ),
            &case,
            SetupTask::Dump {
                dump: id,
                case: case.clone(),
            },
            option_space(&domain),
        );
    }
    let groups: Vec<_> = state.land.undistributed_trucks.keys().cloned().collect();
    for group in groups {
        open_trucks(content, state, cx, &group)?;
    }
    // Development can resolve available setup choices while the remaining procedures are built.
    // Strict play must stop rather than treating the unassigned assets as placed.
    let mut unfinished = BTreeMap::<(Side, &'static str), Role>::new();
    for (force, assets) in &state.air.forces {
        if assets.sgsu_available > 0 || assets.planes.values().any(|p| p.total > 0) {
            unfinished.insert(
                (
                    if force == "axis" {
                        Side::Axis
                    } else {
                        Side::Commonwealth
                    },
                    "scen:59.33",
                ),
                Role::Air,
            );
        }
    }
    for (index, pool) in state.logistics.truck_pools.iter().enumerate() {
        if pool.trucks.total() > 0 {
            let case = if content.scenario.supply.second_third_line_trucks[index]
                .purpose
                .as_deref()
                == Some("air_facilities")
            {
                "scen:59.43"
            } else {
                "scen:59.44"
            };
            unfinished.insert((pool.side, case), Role::Logistics);
        }
    }
    for ((side, case), role) in unfinished {
        let detail = "air or convoy setup assets remain awaiting their placement procedure";
        if strict {
            return Err(EngineError::Unsupported {
                case: case.into(),
                detail: detail.into(),
            });
        }
        cx.emit(EngineEvent::new(
            Audience::Seat(SeatId::new(side, role)),
            GameEvent::Note {
                text: format!("Awaiting setup ({case}): {detail}."),
            },
        ));
    }
    close_if_ready(state, cx)?;
    if state.setup.closed {
        crate::logistics::convoys::initialize(content, state, strict, cx)?;
    }
    Ok(())
}

/// Cases: scen:59.42
fn open_trucks(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
    group: &str,
) -> Result<(), EngineError> {
    let remaining = *state
        .land
        .undistributed_trucks
        .get(group)
        .ok_or_else(|| invariant("unknown first-line pool"))?;
    if remaining.total() <= 0 {
        return Err(invariant("empty first-line pool should be removed"));
    }
    let members: Vec<_> = state
        .land
        .units
        .values()
        .filter(|u| u.setup_group.as_deref() == Some(group))
        .collect();
    let side = members
        .first()
        .ok_or_else(|| invariant("first-line pool has no starting units"))?
        .side;
    if members.iter().any(|u| u.side != side) {
        return Err(invariant("first-line pool mixes sides"));
    }
    let among = members.iter().map(|u| u.id.clone()).collect();
    let mut fields = vec![FieldSchema {
        name: "unit".into(),
        doc: "A unit in this starting group.".into(),
        schema: ActionSchema::Unit { among },
        optional: false,
    }];
    for (name, max) in [
        ("light", remaining.light),
        ("medium", remaining.medium),
        ("heavy", remaining.heavy),
    ] {
        fields.push(FieldSchema {
            name: name.into(),
            doc: format!("{name} truck points to allocate; allocate at least one point in total."),
            schema: ActionSchema::Integer {
                min: 0,
                max: i64::from(max),
            },
            optional: false,
        });
    }
    open_task(
        state,
        cx,
        SeatId::new(side, role(content, "scen:59.42", Role::Logistics)),
        KIND_TRUCKS,
        format!(
            "Distribute first-line trucks from {group}; remaining {} light, {} medium, {} heavy. Repeat until the pool is allocated.",
            remaining.light, remaining.medium, remaining.heavy
        ),
        "scen:59.42",
        SetupTask::Trucks {
            group: group.into(),
        },
        ActionSpace::new(ActionSchema::Record { fields }),
    );
    Ok(())
}

/// Apply one accepted answer privately and leave conflicting answers available for retry.
/// Cases: scen:59.2, scen:59.42, scen:59.53, land:8.13
/// Interpretations: interp:scen-0005
pub(crate) fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
    strict: bool,
) -> Result<String, Rejection> {
    let task = state
        .setup
        .tasks
        .get(&pending.id)
        .cloned()
        .ok_or_else(|| Rejection::Engine(invariant("unknown setup task")))?;
    match task {
        SetupTask::Unit { unit, case } => {
            let owner = state
                .land
                .units
                .get(&unit)
                .ok_or_else(|| Rejection::Engine(invariant("missing setup unit")))?
                .side;
            if owner != pending.seat.side {
                return Err(illegal("setup unit belongs to another side"));
            }
            let (p, _) = group_placement(content, state, &unit).map_err(Rejection::Engine)?;
            let domain =
                placement::choices(content, &p, owner, &case).map_err(Rejection::Engine)?;
            let destination = resolved_destination(&domain, action)?;
            if enemy_fixed_at(state, owner, &destination)
                || enemy_buffered_at(state, owner, &destination)
            {
                return Err(illegal("not a legal setup destination"));
            }
            let group = state.land.units[&unit].setup_group.clone();
            state.setup.unit_locations.insert(unit, destination);
            // One domain per group at a time; every unit retains its own unrestricted choice.
            let next = state
                .land
                .units
                .values()
                .find(|u| {
                    u.setup_group == group
                        && matches!(u.location, Location::AwaitingSetup { .. })
                        && !state.setup.unit_locations.contains_key(&u.id)
                })
                .map(|u| u.id.clone());
            if let Some(id) = next {
                let (p, case) = group_placement(content, state, &id).map_err(Rejection::Engine)?;
                let mut domain =
                    placement::choices(content, &p, owner, &case).map_err(Rejection::Engine)?;
                domain.retain(|l| !enemy_fixed_at(state, owner, l));
                open_task(
                    state,
                    cx,
                    SeatId::new(owner, role(content, &case, Role::Commander)),
                    KIND_UNIT,
                    format!("Place {id} within its scenario setup area."),
                    &case,
                    SetupTask::Unit {
                        unit: id,
                        case: case.clone(),
                    },
                    option_space(&domain),
                );
            }
        }
        SetupTask::Dump { dump, case } => {
            let d = state
                .logistics
                .dumps
                .get(&dump)
                .ok_or_else(|| Rejection::Engine(invariant("missing setup dump")))?;
            if d.side != pending.seat.side {
                return Err(illegal("setup dump belongs to another side"));
            }
            let DumpLocation::AwaitingSetup { placement: p } = &d.location else {
                return Err(illegal("dump already placed"));
            };
            let domain =
                placement::choices(content, p, d.side, &case).map_err(Rejection::Engine)?;
            let destination = resolved_destination(&domain, action)?;
            if d.dummy
                && destination
                    .hex()
                    .is_some_and(|h| facility_hexes(content).contains(h))
            {
                return Err(illegal("not a legal setup destination"));
            }
            state.setup.dump_locations.insert(dump, destination);
        }
        SetupTask::Trucks { group } => {
            let fields = action
                .as_object()
                .ok_or_else(|| illegal("provide a unit and truck allocation"))?;
            if fields.len() != 4 {
                return Err(illegal("provide unit, light, medium and heavy"));
            }
            let id = fields
                .get("unit")
                .and_then(Value::as_str)
                .ok_or_else(|| illegal("choose a starting unit"))?;
            let id = UnitId::new(id);
            let target = state
                .land
                .units
                .get(&id)
                .ok_or_else(|| illegal("unknown starting unit"))?;
            if target.side != pending.seat.side || target.setup_group.as_deref() != Some(&group) {
                return Err(illegal("unit is outside this starting group"));
            }
            let count = |name: &str| -> Result<i32, Rejection> {
                fields
                    .get(name)
                    .and_then(Value::as_i64)
                    .and_then(|v| i32::try_from(v).ok())
                    .filter(|n| *n >= 0)
                    .ok_or_else(|| illegal("truck allocation needs nonnegative integer points"))
            };
            let chosen = Trucks {
                light: count("light")?,
                medium: count("medium")?,
                heavy: count("heavy")?,
            };
            let remaining = state
                .land
                .undistributed_trucks
                .get(&group)
                .ok_or_else(|| Rejection::Engine(invariant("missing first-line pool")))?;
            if chosen.light > remaining.light
                || chosen.medium > remaining.medium
                || chosen.heavy > remaining.heavy
                || i64::from(chosen.light) + i64::from(chosen.medium) + i64::from(chosen.heavy) == 0
            {
                return Err(illegal(
                    "allocate positive points within the remaining pool",
                ));
            }
            let left = Trucks {
                light: remaining.light - chosen.light,
                medium: remaining.medium - chosen.medium,
                heavy: remaining.heavy - chosen.heavy,
            };
            let target = state
                .land
                .units
                .get_mut(&id)
                .expect("validated starting unit");
            target.trucks.light += chosen.light;
            target.trucks.medium += chosen.medium;
            target.trucks.heavy += chosen.heavy;
            if left.total() == 0 {
                state.land.undistributed_trucks.remove(&group);
            } else {
                state.land.undistributed_trucks.insert(group.clone(), left);
                open_trucks(content, state, cx, &group).map_err(Rejection::Engine)?;
            }
        }
    }
    state.setup.tasks.remove(&pending.id);
    close_if_ready(state, cx).map_err(Rejection::Engine)?;
    if state.setup.closed {
        crate::logistics::convoys::initialize(content, state, strict, cx)
            .map_err(Rejection::Engine)?;
    }
    Ok("Setup choice accepted privately.".into())
}

/// Publish only stack presence once all available setup decisions in the shared window close.
/// Cases: land:3.62, scen:59.2
/// Interpretations: interp:scen-0005
fn close_if_ready(state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    if state.setup.closed
        || !state.setup.tasks.is_empty()
        || state
            .decisions
            .pending
            .iter()
            .any(|p| p.kind.starts_with("cna.setup."))
    {
        return Ok(());
    }
    for (id, location) in std::mem::take(&mut state.setup.unit_locations) {
        state
            .land
            .units
            .get_mut(&id)
            .ok_or_else(|| invariant("buffered unit disappeared"))?
            .location = location;
    }
    for (id, location) in std::mem::take(&mut state.setup.dump_locations) {
        state
            .logistics
            .dumps
            .get_mut(&id)
            .ok_or_else(|| invariant("buffered dump disappeared"))?
            .location = match location {
            Location::Hex { hex } => DumpLocation::Hex { hex },
            Location::OffMap { id } => DumpLocation::OffMap { id },
            _ => return Err(invariant("invalid buffered dump destination")),
        };
    }
    state.setup.closed = true;
    cx.emit(EngineEvent::public(GameEvent::Note {
        text: "The simultaneous setup placement window is closed.".into(),
    }));
    Ok(())
}

#[cfg(test)]
#[path = "decisions_tests.rs"]
mod tests;
