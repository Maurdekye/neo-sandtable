//! Axis counter shipping in its Truck Convoy half and Commonwealth abstract port transfers.
//! Each loading or unloading spends the relevant port budget for the current stage.
//! Answers are atomic owner lists; validation reads owned stocks and public positions.
//! Unknown sea entries and port anchors are never replaced by land-road or straight-line routes.
use super::{
    SupplyError, ports,
    stores::{engine, field, option},
    water::WaterStage,
};
use crate::{
    CnaContent,
    state::{DumpLocation, Location, Pending, State},
    steps::{illegal, open},
};
use cna_content::scenario::Supplies;
use cna_core::{
    decision::{ActionSchema, ActionSpace, Secrecy, Trigger},
    engine::{Cx, EngineError, Rejection},
    event::EngineEvent,
    ids::{HexId, SeatId},
    visibility::Audience,
};
use cna_protocol::{GameEvent, Role, Side};
use cna_tables::airlog::supply::SupplyType;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const AXIS: &str = "cna.logistics.coastal.axis";
pub const CW: &str = "cna.logistics.coastal.commonwealth";
const TYPES: [SupplyType; 4] = [
    SupplyType::Ammo,
    SupplyType::Fuel,
    SupplyType::Stores,
    SupplyType::Water,
];

/// Printed capacity remains in content; cargo, position and loading/movement CP are state.
/// Cases: airlog:56.31, airlog:56.34, airlog:56.35
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoastalShipState {
    pub location: Location,
    pub cargo: Supplies,
    pub cp_quarters: i32,
    pub stage: Option<WaterStage>,
}
/// Cases: scen:59.54, scen:60.35
pub fn initialize(content: &CnaContent, state: &mut State) -> Result<(), SupplyError> {
    let Some(setup) = &content.scenario.fleet_logistics.axis_coastal_shipping else {
        return Ok(());
    };
    let choices =
        crate::setup::placement::choices(content, &setup.location, Side::Axis, "scen:59.54");
    let at = match choices {
        Ok(v) if v.len() == 1 => v[0].clone(),
        _ => return Err(SupplyError::Unsupported { case: "scen:59.54" }),
    };
    for id in content
        .units
        .coastal_rosters
        .get(&setup.roster)
        .ok_or(SupplyError::Invalid)?
    {
        state
            .logistics
            .coastal_ships
            .entry(id.clone())
            .or_insert_with(|| CoastalShipState {
                location: at.clone(),
                cargo: Supplies::default(),
                cp_quarters: 0,
                stage: None,
            });
    }
    Ok(())
}
fn active(state: &State) -> bool {
    state.cursor.anchor() == "opstage.truck_convoy_movement"
        && state.cursor.phasing(state.turn.player_a) == Some(Side::Axis)
}
fn refresh(state: &mut State, id: &str) -> Result<(), SupplyError> {
    let stage = WaterStage::current(state);
    let ship = state
        .logistics
        .coastal_ships
        .get_mut(id)
        .ok_or(SupplyError::Invalid)?;
    if ship.stage != Some(stage) {
        ship.stage = Some(stage);
        ship.cp_quarters = 0;
    }
    if ship.cp_quarters < 0 || ship.cp_quarters > 200 {
        return Err(SupplyError::Invalid);
    }
    Ok(())
}
fn cp(state: &mut State, id: &str, quarters: i32) -> Result<(), SupplyError> {
    refresh(state, id)?;
    let s = state.logistics.coastal_ships.get_mut(id).unwrap();
    let total = s
        .cp_quarters
        .checked_add(quarters)
        .ok_or(SupplyError::Invalid)?;
    if total > 200 {
        return Err(SupplyError::Insufficient);
    }
    s.cp_quarters = total;
    Ok(())
}
fn dump_at(state: &State, side: Side, id: &str) -> Result<Location, SupplyError> {
    let d = state
        .logistics
        .dumps
        .get(id)
        .filter(|d| d.side == side && d.active && !d.dummy)
        .ok_or(SupplyError::Invalid)?;
    match &d.location {
        DumpLocation::Hex { hex } => Ok(Location::Hex { hex: hex.clone() }),
        DumpLocation::OffMap { id } => Ok(Location::OffMap { id: id.clone() }),
        _ => Err(SupplyError::Invalid),
    }
}

fn receiving_dump(
    content: &CnaContent,
    state: &mut State,
    side: Side,
    key: &str,
    at: &Location,
) -> Result<String, SupplyError> {
    if let Some(port_id) = key.strip_prefix("new:") {
        let port = ports::at(content, at)?;
        if port.id != port_id
            || !state
                .logistics
                .ports
                .get(&port.id)
                .is_some_and(|p| p.owner == side && p.efficiency > 0)
        {
            return Err(SupplyError::Invalid);
        }
        let id = format!("{}.coastal.port.{}", crate::state::side_key(side), port.id);
        if let Some(d) = state.logistics.dumps.get(&id) {
            if d.side != side || !d.active || d.dummy || dump_at(state, side, &id)? != *at {
                return Err(SupplyError::Invalid);
            }
        } else {
            let location = match at {
                Location::Hex { hex } => DumpLocation::Hex { hex: hex.clone() },
                Location::OffMap { id } => DumpLocation::OffMap { id: id.clone() },
                _ => return Err(SupplyError::Invalid),
            };
            let marker = super::dump_markers::next_marker(&mut state.logistics)?;
            state.logistics.dumps.insert(
                id.clone(),
                crate::state::Dump {
                    id: id.clone(),
                    marker,
                    side,
                    location,
                    supplies: Supplies::default(),
                    active: true,
                    dummy: false,
                },
            );
        }
        Ok(id)
    } else if dump_at(state, side, key)? == *at {
        Ok(key.into())
    } else {
        Err(SupplyError::Invalid)
    }
}
fn unlimited_location(content: &CnaContent, hex: &HexId) -> Result<Location, SupplyError> {
    let at = content
        .map
        .canonical(hex)
        .ok_or(SupplyError::Invalid)?
        .clone();
    let policy = content
        .scenario
        .supply
        .unlimited_supply
        .as_ref()
        .filter(|s| s.side == Side::Commonwealth)
        .ok_or(SupplyError::Invalid)?;
    for id in &policy.locations {
        let area = content
            .areas
            .areas
            .get(id)
            .ok_or(SupplyError::Unsupported { case: "scen:60.44" })?;
        if area.membership_status != "resolved" {
            return Err(SupplyError::Unsupported { case: "scen:60.44" });
        }
        if area.hex_ids.contains(&at) {
            return Ok(Location::Hex { hex: at });
        }
    }
    Err(SupplyError::Invalid)
}
fn cw_origin(content: &CnaContent, state: &State, key: &str) -> Result<Location, SupplyError> {
    if let Some(hex) = key.strip_prefix("base:") {
        unlimited_location(content, &HexId::new(hex))
    } else {
        dump_at(state, Side::Commonwealth, key)
    }
}
fn cw_target(content: &CnaContent, state: &State, key: &str) -> Result<Location, SupplyError> {
    if let Some(hex) = key.strip_prefix("new:") {
        let at = Location::Hex {
            hex: content
                .map
                .canonical(&HexId::new(hex))
                .ok_or(SupplyError::Invalid)?
                .clone(),
        };
        let port = ports::at(content, &at)?;
        if !state
            .logistics
            .ports
            .get(&port.id)
            .is_some_and(|p| p.owner == Side::Commonwealth && p.efficiency > 0)
        {
            return Err(SupplyError::Invalid);
        }
        Ok(at)
    } else {
        dump_at(state, Side::Commonwealth, key)
    }
}
fn add(a: &Supplies, b: &Supplies, subtract: bool) -> Result<Supplies, SupplyError> {
    let mut result = *a;
    for t in TYPES {
        let n = super::capacity::points(b, t);
        if n < 0 {
            return Err(SupplyError::Invalid);
        }
        let new = if subtract {
            super::capacity::points(a, t).checked_sub(n)
        } else {
            super::capacity::points(a, t).checked_add(n)
        }
        .ok_or(SupplyError::Invalid)?;
        if new < 0 {
            return Err(SupplyError::Insufficient);
        }
        super::capacity::set_points(&mut result, t, new);
    }
    Ok(result)
}
fn one_type(cargo: &Supplies) -> bool {
    TYPES
        .into_iter()
        .filter(|t| super::capacity::points(cargo, *t) > 0)
        .count()
        == 1
}
/// Loading occurs at the beginning of the ship's convoy phase, and takes5CP. No personnel
/// or equipment action exists here. The ship carries just one supply type.
/// Cases: airlog:55.14, airlog:56.31, airlog:56.32, airlog:56.34
/// Interpretations: interp:airlog-0010, interp:airlog-0011
pub fn load(
    content: &CnaContent,
    state: &mut State,
    id: &str,
    dump: &str,
    cargo: Supplies,
) -> Result<(), SupplyError> {
    if !active(state)
        || !one_type(&cargo)
        || TYPES
            .into_iter()
            .any(|t| super::capacity::points(&cargo, t) < 0)
    {
        return Err(SupplyError::Invalid);
    }
    let mut draft = state.clone();
    refresh(&mut draft, id)?;
    let ship = &draft.logistics.coastal_ships[id];
    if ship.cp_quarters != 0
        || draft.logistics.coastal_loading_closed_stage == Some(WaterStage::current(&draft))
    {
        return Err(SupplyError::Invalid);
    }
    let at = dump_at(&draft, Side::Axis, dump)?;
    if at != ship.location {
        return Err(SupplyError::Invalid);
    }
    let total = add(&ship.cargo, &cargo, false)?;
    if !one_type(&total) {
        return Err(SupplyError::Invalid);
    }
    let capacity = content
        .units
        .coastal_ships
        .get(id)
        .and_then(|s| s.capacity_tons)
        .ok_or(SupplyError::Unsupported {
            case: "airlog:56.31",
        })?;
    if ports::weight24(content, &total)? > i64::from(capacity) * 24 {
        return Err(SupplyError::Insufficient);
    }
    let remaining = add(&draft.logistics.dumps[dump].supplies, &cargo, true)?;
    let port = ports::at(content, &at)?;
    ports::charge(
        content,
        &mut draft,
        Side::Axis,
        &port,
        ports::weight24(content, &cargo)?,
        false,
    )?;
    cp(&mut draft, id, 20)?;
    draft.logistics.coastal_ships.get_mut(id).unwrap().cargo = total;
    draft.logistics.dumps.get_mut(dump).unwrap().supplies = remaining;
    state.logistics = draft.logistics;
    Ok(())
}
/// Unloading may be partial, then the ship may continue while its50CP budget permits.
/// Cases: airlog:55.14, airlog:56.34, airlog:56.35
/// Interpretations: interp:airlog-0010, interp:airlog-0011
pub fn unload(
    content: &CnaContent,
    state: &mut State,
    id: &str,
    dump: &str,
    cargo: Supplies,
) -> Result<(), SupplyError> {
    if !active(state) || !one_type(&cargo) {
        return Err(SupplyError::Invalid);
    }
    let mut draft = state.clone();
    refresh(&mut draft, id)?;
    let ship = draft
        .logistics
        .coastal_ships
        .get(id)
        .ok_or(SupplyError::Invalid)?;
    let at = ship.location.clone();
    let dump = receiving_dump(content, &mut draft, Side::Axis, dump, &at)?;
    let ship = &draft.logistics.coastal_ships[id];
    let remaining = add(&ship.cargo, &cargo, true)?;
    super::distribution::validate_dump_capacity(content, &draft, Side::Axis, &at, &cargo)?;
    let total = add(&draft.logistics.dumps[&dump].supplies, &cargo, false)?;
    let port = ports::at(content, &at)?;
    ports::charge(
        content,
        &mut draft,
        Side::Axis,
        &port,
        ports::weight24(content, &cargo)?,
        false,
    )?;
    cp(&mut draft, id, 20)?;
    draft.logistics.coastal_loading_closed_stage = Some(WaterStage::current(&draft));
    draft.logistics.coastal_ships.get_mut(id).unwrap().cargo = remaining;
    draft.logistics.dumps.get_mut(&dump).unwrap().supplies = total;
    state.logistics = draft.logistics;
    Ok(())
}
/// Consecutive verified sea hexes cost1CP each. Off-map joins and transit-box topology need
/// their own verified source data; the land entry hex is not assumed to be a sea entrance.
/// Ports encountered along a path must remain friendly and operative.
/// Cases: airlog:56.31, airlog:56.32, airlog:56.33, airlog:56.35
/// Interpretations: interp:airlog-0011
pub fn sail(
    content: &CnaContent,
    state: &mut State,
    id: &str,
    path: &[HexId],
) -> Result<(), SupplyError> {
    if !active(state) || path.is_empty() {
        return Err(SupplyError::Invalid);
    }
    let mut draft = state.clone();
    refresh(&mut draft, id)?;
    let mut from = draft.logistics.coastal_ships[id]
        .location
        .hex()
        .cloned()
        .ok_or(SupplyError::Unsupported {
            case: "airlog:56.31",
        })?;
    for hex in path {
        let to = content.map.canonical(hex).ok_or(SupplyError::Invalid)?;
        if !content.map.neighbors(&from).iter().any(|r| &r.id == to) {
            return Err(SupplyError::Invalid);
        }
        let loc = Location::Hex { hex: to.clone() };
        if let Ok(port) = ports::at(content, &loc) {
            ports::advance(content, &mut draft, &port)?;
            let occupied = draft
                .land
                .units
                .values()
                .any(|u| u.side != Side::Axis && u.location.hex() == Some(to));
            let ps = draft
                .logistics
                .ports
                .get(&port.id)
                .ok_or(SupplyError::Unsupported {
                    case: "airlog:55.11",
                })?;
            if occupied || ps.owner != Side::Axis || ps.efficiency == 0 {
                return Err(SupplyError::Insufficient);
            }
        } else if !matches!(
            content.map.terrain_survey(to),
            cna_content::map::Survey::Present("sea")
        ) {
            return Err(SupplyError::Unsupported {
                case: "airlog:56.31",
            });
        }
        cp(&mut draft, id, 4)?;
        from = to.clone();
    }
    draft.logistics.coastal_loading_closed_stage = Some(WaterStage::current(&draft));
    draft.logistics.coastal_ships.get_mut(id).unwrap().location = Location::Hex { hex: from };
    state.logistics = draft.logistics;
    Ok(())
}
/// Commonwealth uses abstract coastal transport, limited by both ports. Only existing
/// friendly stocks or the scenario-authorized unlimited base may supply it. No personnel,
/// equipment or replacement production is created by this supply shipment.
/// Cases: airlog:48.0, airlog:55.13, airlog:55.14, land:8.82
/// Interpretations: interp:airlog-0010
pub fn commonwealth_transfer(
    content: &CnaContent,
    state: &mut State,
    from: &str,
    to: &str,
    cargo: Supplies,
) -> Result<(), SupplyError> {
    if state.cursor.anchor() != "opstage.organization.tactical_shipping"
        || from == to
        || ports::weight24(content, &cargo)? == 0
    {
        return Err(SupplyError::Invalid);
    }
    let mut draft = state.clone();
    let origin = cw_origin(content, &draft, from)?;
    let target = cw_target(content, &draft, to)?;
    let target_id = receiving_dump(content, &mut draft, Side::Commonwealth, to, &target)?;
    if origin == target
        || !matches!(origin, Location::Hex { .. })
        || !matches!(target, Location::Hex { .. })
    {
        return Err(SupplyError::Invalid);
    }
    let source = if from.starts_with("base:") {
        if cargo.water != 0
            || TYPES
                .into_iter()
                .any(|t| super::capacity::points(&cargo, t) < 0)
        {
            return Err(SupplyError::Invalid);
        }
        None
    } else {
        Some(add(&draft.logistics.dumps[from].supplies, &cargo, true)?)
    };
    let destination = add(&draft.logistics.dumps[&target_id].supplies, &cargo, false)?;
    super::distribution::validate_dump_capacity(
        content,
        &draft,
        Side::Commonwealth,
        &target,
        &cargo,
    )?;
    let weight = ports::weight24(content, &cargo)?;
    for loc in [&origin, &target] {
        let p = ports::at(content, loc)?;
        ports::charge(content, &mut draft, Side::Commonwealth, &p, weight, false)?;
    }
    if let Some(source) = source {
        draft.logistics.dumps.get_mut(from).unwrap().supplies = source;
    }
    draft.logistics.dumps.get_mut(&target_id).unwrap().supplies = destination;
    state.logistics = draft.logistics;
    Ok(())
}
/// Cases: airlog:48.0, airlog:56.32, land:3.6
pub fn enter_axis(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    if !active(state) {
        return Ok(());
    }
    initialize(content, state).map_err(engine)?;
    let ids: Vec<_> = state.logistics.coastal_ships.keys().cloned().collect();
    for id in &ids {
        refresh(state, id).map_err(engine)?;
    }
    if ids.is_empty() {
        return Ok(());
    }
    let fields = vec![
        field(
            "ship",
            "Axis coastal counter",
            ActionSchema::Choice {
                options: ids.into_iter().map(|id| option(id.clone(), id)).collect(),
            },
        ),
        field(
            "operation",
            "Load at phase start, sail, or unload",
            ActionSchema::Choice {
                options: ["load", "sail", "unload"]
                    .into_iter()
                    .map(|s| option(s.into(), s.into()))
                    .collect(),
            },
        ),
        field(
            "dump",
            "Owned port dump id (empty when sailing)",
            ActionSchema::Choice {
                options: std::iter::once(option("".into(), "No dump when sailing".into()))
                    .chain(
                        state
                            .logistics
                            .dumps
                            .values()
                            .filter(|d| d.side == Side::Axis && d.active && !d.dummy)
                            .map(|d| option(d.id.clone(), d.id.clone())),
                    )
                    .chain(
                        state
                            .logistics
                            .ports
                            .iter()
                            .filter(|(_, p)| p.owner == Side::Axis && p.efficiency > 0)
                            .map(|(id, _)| {
                                option(
                                    format!("new:{id}"),
                                    format!("Create a receiving dump at {id}"),
                                )
                            }),
                    )
                    .collect(),
            },
        ),
        field(
            "cargo",
            "Single supply type; zero when sailing",
            cargo_schema(),
        ),
        field(
            "path",
            "Consecutive verified sea hexes, excluding origin",
            ActionSchema::List {
                item: Box::new(ActionSchema::Hex {
                    among: Some(
                        content
                            .map
                            .iter()
                            .filter(|h| {
                                matches!(
                                    content.map.terrain_survey(&h.id),
                                    cna_content::map::Survey::Present("sea")
                                ) || content.places.at(&h.id).any(|p| p.kind == "port")
                            })
                            .map(|h| h.id.clone())
                            .collect(),
                    ),
                }),
                min: 0,
                max: 50,
            },
        ),
    ];
    open(state,cx,SeatId::new(Side::Axis,Role::Logistics),AXIS,"Submit a list of coastal orders. Load all ships before any sail or unload;50CP includes handling, one cargo type per ship. Off-map sea connections are not yet verified.".into(),&["airlog:56.31","airlog:56.32","airlog:56.34","airlog:56.35","land:3.6"],Trigger::Scheduled,Secrecy::Secret,ActionSpace::new(ActionSchema::List{item:Box::new(ActionSchema::Record{fields}),min:0,max:1024}).with_pass("Finish coastal shipping"));
    Ok(())
}
fn cargo_schema() -> ActionSchema {
    ActionSchema::Record {
        fields: ["ammo", "fuel", "stores", "water"]
            .into_iter()
            .map(|name| {
                field(
                    name,
                    "Whole supply points",
                    ActionSchema::Integer {
                        min: 0,
                        max: i32::MAX.into(),
                    },
                )
            })
            .collect(),
    }
}
/// Cases: airlog:48.0, airlog:55.14, land:3.6
pub fn enter_cw(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    ports::initialize(content, state);

    let mut origins = state
        .logistics
        .dumps
        .values()
        .filter(|d| {
            dump_at(state, Side::Commonwealth, &d.id).is_ok_and(|at| {
                matches!(at, Location::Hex { .. })
                    && ports::at(content, &at).is_ok_and(|p| {
                        state
                            .logistics
                            .ports
                            .get(&p.id)
                            .is_some_and(|s| s.owner == Side::Commonwealth && s.efficiency > 0)
                    })
            })
        })
        .map(|d| option(d.id.clone(), d.id.clone()))
        .collect::<Vec<_>>();
    let mut destinations = origins.clone();
    for (id, ps) in &state.logistics.ports {
        if ps.owner != Side::Commonwealth || ps.efficiency == 0 {
            continue;
        }
        let at = Location::Hex {
            hex: HexId::new(id),
        };
        if ports::at(content, &at).is_err() {
            continue;
        }
        destinations.push(option(
            format!("new:{id}"),
            format!("Receive into a port dump at {id}"),
        ));
        if unlimited_location(content, &HexId::new(id)).is_ok() {
            origins.push(option(
                format!("base:{id}"),
                format!("Scenario supply at {id}"),
            ));
        }
    }
    if origins.is_empty() || destinations.is_empty() {
        return Ok(());
    }
    open(state,cx,SeatId::new(Side::Commonwealth,Role::Logistics),CW,"Coastal supply shipment between controlled African ports, within both remaining port budgets.".into(),&["airlog:48.0","airlog:55.14","land:3.6"],Trigger::Scheduled,Secrecy::Secret,ActionSpace::new(ActionSchema::List{item:Box::new(ActionSchema::Record{fields:vec![field("from","Origin port dump",ActionSchema::Choice{options:origins}),field("to","Destination port dump",ActionSchema::Choice{options:destinations}),field("cargo","Supplies to ship",cargo_schema())]}),min:0,max:1024}).with_pass("Finish coastal shipping"));
    Ok(())
}

/// Stack presence follows the ordinary counter visibility. Individual identities,
/// cargo and CP remain in the owner-only ship view.
/// Cases: land:3.62, airlog:56.31
pub(crate) fn markers(state: &State) -> Vec<cna_protocol::Marker> {
    let hexes = state
        .logistics
        .coastal_ships
        .values()
        .filter_map(|s| s.location.hex().cloned())
        .collect::<std::collections::BTreeSet<_>>();
    hexes
        .into_iter()
        .map(|hex| cna_protocol::Marker {
            id: format!("coastal:axis:{hex}"),
            kind: "coastal_ship_stack".into(),
            hex: hex.to_string(),
            side: Some(Side::Axis),
            label: Some("Axis coastal ships".into()),
        })
        .collect()
}
fn public_markers(state: &State) -> Vec<cna_protocol::Marker> {
    let mut out = markers(state);
    out.extend(state.logistics.dumps.values().filter_map(|d| {
        super::dump_markers::marker(
            d,
            cna_core::visibility::Perspective::Side(d.side.opponent()),
        )
    }));
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}
fn publish_presence(state: &State, before: &[cna_protocol::Marker], cx: &mut Cx<'_>) {
    let after = public_markers(state);
    for old in before
        .iter()
        .filter(|old| !after.iter().any(|m| m.id == old.id))
    {
        cx.emit(EngineEvent::public(GameEvent::MarkerRemoved {
            marker_id: old.id.clone(),
        }));
    }
    for new in after
        .iter()
        .filter(|new| !before.iter().any(|m| m.id == new.id))
    {
        cx.emit(EngineEvent::public(GameEvent::MarkerPlaced {
            marker: new.clone(),
        }));
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AxisOrder {
    ship: String,
    operation: String,
    dump: String,
    cargo: Supplies,
    path: Vec<HexId>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CwOrder {
    from: String,
    to: String,
    cargo: Supplies,
}
/// Cases: airlog:55.14, airlog:56.31, airlog:56.32, airlog:56.34, land:3.6
/// Interpretations: interp:airlog-0010, interp:airlog-0011
pub fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    let axis = pending.kind == AXIS && pending.seat == SeatId::new(Side::Axis, Role::Logistics);
    let cw = pending.kind == CW && pending.seat == SeatId::new(Side::Commonwealth, Role::Logistics);
    if !axis && !cw {
        return Err(illegal("not this seat's coastal window"));
    }
    if action.is_null() || action.as_array().is_some_and(Vec::is_empty) {
        return Ok("Finished coastal shipping".into());
    }
    let list = action
        .as_array()
        .filter(|v| v.len() <= 1024)
        .ok_or_else(|| illegal("expected a coastal order list"))?;
    let mut draft = state.clone();
    let before = public_markers(state);
    for entry in list {
        let result = if axis {
            let o: AxisOrder = serde_json::from_value(entry.clone())
                .map_err(|_| illegal("invalid coastal order"))?;
            match o.operation.as_str() {
                "load" if o.path.is_empty() => load(content, &mut draft, &o.ship, &o.dump, o.cargo),
                "unload" if o.path.is_empty() => {
                    unload(content, &mut draft, &o.ship, &o.dump, o.cargo)
                }
                "sail" if o.dump.is_empty() && o.cargo == Supplies::default() => {
                    sail(content, &mut draft, &o.ship, &o.path)
                }
                _ => Err(SupplyError::Invalid),
            }
        } else {
            let o: CwOrder = serde_json::from_value(entry.clone())
                .map_err(|_| illegal("invalid coastal order"))?;
            commonwealth_transfer(content, &mut draft, &o.from, &o.to, o.cargo)
        };
        result.map_err(|e| match e {
            SupplyError::Unsupported { .. } => Rejection::Engine(engine(e)),
            _ => illegal("coastal list violates cargo, ownership, phase, CPA or port limits"),
        })?;
    }
    state.logistics = draft.logistics;
    publish_presence(state, &before, cx);
    cx.emit(EngineEvent::new(
        Audience::Side(pending.seat.side),
        GameEvent::Note {
            text: format!("Accepted coastal shipping list: {action}"),
        },
    ));
    if axis {
        enter_axis(content, state, cx)
    } else {
        enter_cw(content, state, cx)
    }
    .map_err(Rejection::Engine)?;
    Ok("Coastal shipping list executed".into())
}
#[cfg(test)]
mod tests;
