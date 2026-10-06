//! What each perspective may see: the board view, the seats' `observe` report and `inspect`
//! details, filtered by Limited Intelligence (`land:3.6`).
//!
//! Baseline (`land:3.61`, `land:3.62`): the presence of every stack on the map is public; the
//! composition, status and attributes of enemy units are not. Own-side seats see everything their
//! side knows; the operator sees everything and is labelled as omniscient by the board.

use std::collections::{BTreeMap, BTreeSet};

use cna_content::units::Toe;
use cna_core::clock::{Anchor, Clock};
use cna_core::engine::{Cx, Rejection};
use cna_core::event::EngineEvent;
use cna_core::ids::{HexId, UnitId};
use cna_core::visibility::{Audience, Perspective};
use cna_protocol::{self as wire, Side};
use serde_json::{Value, json};

use crate::content::CnaContent;
use crate::seq::Block;
use crate::state::{DumpLocation, LandUnit, Location, State};
use crate::steps::illegal;

/// Whether `perspective` may see the full detail of something belonging to `side`.
pub(crate) fn sees_side(perspective: Perspective, side: Side) -> bool {
    match perspective {
        Perspective::Operator => true,
        Perspective::Side(s) => s == side,
        Perspective::Seat(seat) => seat.side == side,
    }
}

/// The engine clock for decision requests.
pub(crate) fn core_clock(state: &State) -> Clock {
    let c = &state.cursor;
    Clock {
        game_turn: c.game_turn,
        op_stage: c.op_stage,
        anchor: Anchor::new(c.anchor()),
        phasing: c.phasing(state.turn.player_a),
        cycle: (c.block == Block::PlayerHalf).then_some(c.cycle),
    }
}

/// The wire clock for the board.
pub(crate) fn wire_clock(content: &CnaContent, state: &State) -> wire::Clock {
    let c = &state.cursor;
    let anchor = c.anchor();
    let parts: Vec<&str> = anchor.split('.').collect();
    let (stage, phase, segment, step) = if parts.first() == Some(&"opstage") {
        (
            "opstage".to_owned(),
            parts.get(1).copied().unwrap_or("").to_owned(),
            parts.get(2).map(|s| (*s).to_owned()),
            parts.get(3).map(|s| (*s).to_owned()),
        )
    } else {
        (
            parts.first().copied().unwrap_or("").to_owned(),
            parts
                .get(1)
                .copied()
                .unwrap_or(parts.first().copied().unwrap_or(""))
                .to_owned(),
            None,
            None,
        )
    };
    wire::Clock {
        game_turn: c.game_turn,
        date: turn_date(
            content.scenario.meta.campaign_start_date.as_deref(),
            c.game_turn,
        ),
        stage,
        op_stage: c.op_stage,
        phase,
        segment,
        step,
        phasing: c.phasing(state.turn.player_a),
    }
}

/// The ISO date of a game-turn's first day: Game-Turn 1 is the campaign start, each turn a week.
fn turn_date(start: Option<&str>, game_turn: u16) -> String {
    let Some((y, m, d)) = start.and_then(parse_date) else {
        return String::new();
    };
    let (y, m, d) =
        civil_from_days(days_from_civil(y, m, d) + 7 * (i64::from(game_turn.max(1)) - 1));
    format!("{y:04}-{m:02}-{d:02}")
}

fn parse_date(s: &str) -> Option<(i64, i64, i64)> {
    let mut it = s.split('-').map(|p| p.parse::<i64>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

// Days-from-civil / civil-from-days (H. Hinnant), integer only.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The board's symbol kind for a unit class (`web/src/map/counters.ts`).
fn board_kind(unit_type: &str) -> &'static str {
    match unit_type {
        "infantry" => "infantry",
        "recce" => "recce",
        "headquarters" => "hq",
        "artillery" => "artillery",
        "anti_tank" => "anti_tank",
        "anti_air" => "aa",
        "tank" => "armor",
        "engineer" => "engineers",
        _ => "unknown",
    }
}

/// The board's echelon word.
fn board_size(echelon: Option<&str>) -> String {
    match echelon {
        Some("super_brigade") => "brigade".to_owned(),
        Some(e) => e.to_owned(),
        None => String::new(),
    }
}

/// TOE strength points a unit holds now (`land:3.5`).
pub(crate) fn toe_points(content: &CnaContent, unit: &LandUnit) -> Option<i32> {
    let class = content
        .units
        .units
        .get(&unit.id)
        .and_then(|oa| oa.class.as_ref())
        .and_then(|c| content.units.classes.get(c));
    match unit.toe.as_ref()? {
        Toe::Normal(_) => class.and_then(|c| c.max_toe),
        Toe::Under { under } => Some(*under),
        Toe::Over { over } => Some(*over),
        Toe::Weapons(list) => Some(list.iter().map(|w| w.n).sum()),
    }
}

/// Stamp `moved_this_segment` (own detail only) from the movement state.
fn stamp_moved(state: &State, id: &UnitId, view: &mut wire::UnitView) {
    if let Some(detail) = view.detail.as_mut() {
        detail.insert(
            "moved_this_segment".to_owned(),
            json!(state.land.movement.moved.contains(id)),
        );
    }
}

/// Keep `moved_this_segment` in live updates consistent with snapshots. Runs at the end of every
/// engine call (`advance`, `respond`): it stamps the flag on each `UnitUpdated` of a land unit and
/// sends an update for every unit whose flag the call cleared (a new Movement Segment), so viewers
/// never keep a stale `true`. `before` is the moved set when the call began.
pub(crate) fn sync_moved_flags(
    content: &CnaContent,
    state: &State,
    before: &BTreeSet<UnitId>,
    cx: &mut Cx<'_>,
) {
    let moved = &state.land.movement.moved;
    let updated: BTreeSet<String> = cx
        .events
        .iter()
        .filter_map(|e| match &e.event {
            wire::GameEvent::UnitUpdated { unit } => Some(unit.id.clone()),
            _ => None,
        })
        .collect();
    for id in before.difference(moved) {
        if let Some(unit) = state.land.units.get(id)
            && !updated.contains(&id.to_string())
        {
            cx.emit(EngineEvent::new(
                Audience::Side(unit.side),
                wire::GameEvent::UnitUpdated {
                    unit: unit_view(content, unit),
                },
            ));
        }
    }
    for event in cx.events.iter_mut() {
        if let wire::GameEvent::UnitUpdated { unit } = &mut event.event {
            let id = UnitId::new(&unit.id);
            if state.land.units.contains_key(&id) {
                stamp_moved(state, &id, unit);
            }
        }
    }
}

/// A unit's full board view (own side or operator only).
pub(crate) fn unit_view(content: &CnaContent, unit: &LandUnit) -> wire::UnitView {
    let oa = content.units.units.get(&unit.id);
    let class = oa
        .and_then(|o| o.class.as_ref())
        .and_then(|c| content.units.classes.get(c));
    let mut detail = BTreeMap::new();
    if let Some(oa) = oa {
        detail.insert("counter".to_owned(), json!(oa.counter));
        if let Some(m) = oa.basic_morale {
            detail.insert("basic_morale".to_owned(), json!(m));
        }
        if let Some(sp) = oa.stacking_points {
            detail.insert("stacking_points".to_owned(), json!(sp));
        }
    }
    if let Some(c) = class {
        detail.insert("class".to_owned(), json!(c.code));
        detail.insert("cpa".to_owned(), json!(c.cpa));
    }
    if let Some(points) = toe_points(content, unit) {
        detail.insert("strength".to_owned(), json!(points));
    }
    detail.insert(
        "cp_spent_quarters".to_owned(),
        json!(unit.cp_spent_quarters),
    );
    detail.insert(
        "cohesion_quarters".to_owned(),
        json!(unit.cohesion_quarters),
    );
    detail.insert(
        "quarter_units".to_owned(),
        json!("4 quarters = 1 CP or cohesion point"),
    );
    wire::UnitView {
        id: unit.id.to_string(),
        side: unit.side,
        name: oa.map_or_else(|| unit.id.to_string(), |o| o.name.clone()),
        kind: class
            .map_or("unknown", |c| board_kind(&c.unit_type))
            .to_owned(),
        size: board_size(oa.and_then(|o| o.echelon.as_deref())),
        nationality: oa.map_or_else(String::new, |o| o.nationality.clone()),
        hex: unit.location.hex().map(|h| h.to_string()),
        parent: if unit.detached {
            None
        } else {
            unit.attached_to
                .clone()
                .or_else(|| oa.and_then(|o| o.parent.clone()))
                .map(|p| p.to_string())
        },
        detail: Some(detail),
    }
}

/// The board view for one perspective.
pub(crate) fn view(
    content: &CnaContent,
    state: &State,
    perspective: Perspective,
) -> wire::ViewState {
    let mut stacks = Vec::new();
    let mut units = BTreeMap::new();
    for ((hex, side), members) in state.stacks() {
        if sees_side(perspective, side) {
            stacks.push(wire::Stack {
                hex: hex.to_string(),
                side,
                unit_ids: members.iter().map(|u| u.id.to_string()).collect(),
                visible_count: Some(u32::try_from(members.len()).unwrap_or(u32::MAX)),
            });
            for u in members {
                let mut view = unit_view(content, u);
                // Own units only: whether the unit has already used its move this segment.
                stamp_moved(state, &u.id, &mut view);
                units.insert(u.id.to_string(), view);
            }
        } else {
            // land:3.62: the stack's presence is public, its contents are not.
            stacks.push(wire::Stack {
                hex: hex.to_string(),
                side,
                unit_ids: Vec::new(),
                visible_count: None,
            });
        }
    }
    // Own units in play but not on a map hex: in an off-map box, or deployed and awaiting the
    // owner's set-up placement. `hex` is null and `detail.location` says which.
    for u in state.land.units.values() {
        if !sees_side(perspective, u.side) {
            continue;
        }
        let location = match &u.location {
            Location::OffMap { id } => json!({ "at": "off_map", "id": id }),
            Location::AwaitingSetup { group } => json!({ "at": "awaiting_setup", "group": group }),
            Location::Hex { .. } | Location::NotArrived | Location::Eliminated => continue,
        };
        let mut view = unit_view(content, u);
        stamp_moved(state, &u.id, &mut view);
        if let Some(detail) = view.detail.as_mut() {
            detail.insert("location".to_owned(), location);
        }
        units.insert(u.id.to_string(), view);
    }
    let mut markers = Vec::new();
    for dump in state.logistics.dumps.values() {
        let DumpLocation::Hex { hex } = &dump.location else {
            continue;
        };
        let own = sees_side(perspective, dump.side);
        markers.push(wire::Marker {
            id: format!("dump:{}", dump.id),
            kind: "supply_dump".into(),
            hex: hex.to_string(),
            side: Some(dump.side),
            label: own.then(|| {
                let s = dump.supplies;
                format!(
                    "{}{}: ammo {}, fuel {}, stores {}, water {}",
                    dump.id,
                    if dump.dummy { " (dummy)" } else { "" },
                    s.ammo,
                    s.fuel,
                    s.stores,
                    s.water
                )
            }),
        });
    }
    // A well attempt reveals only its printed condition marker, never the secret roll or quantity.
    // Cases: airlog:52.14, airlog:52.16
    for hex in state.logistics.wells.keys() {
        let known = crate::logistics::wells::condition(state, hex, perspective);
        for condition in ["depleted", "poisoned"] {
            if known.get(condition) == Some(&json!(true)) {
                markers.push(wire::Marker {
                    id: format!("well:{hex}:{condition}"),
                    kind: format!("well_{condition}"),
                    hex: hex.to_string(),
                    side: None,
                    label: Some(format!("{condition} well")),
                });
            }
        }
    }
    let pending = state
        .decisions
        .pending
        .iter()
        .filter(|p| perspective.can_see(&cna_core::visibility::Audience::Seat(p.seat)))
        .map(|p| wire::PendingDecision {
            id: p.id.to_string(),
            seat: p.seat.to_string(),
            kind: p.kind.clone(),
            summary: p.summary.clone(),
            opened_seq: 0,
            rules: p.rules.clone(),
            space: Some(p.space.to_json_schema()),
        })
        .collect();
    wire::ViewState {
        clock: wire_clock(content, state),
        stacks,
        units,
        markers,
        pending,
    }
}

/// A plain-English orientation for AI seats, in our own words.
pub const RULES_SUMMARY: &str = "THE CAMPAIGN FOR NORTH AFRICA (SPI 1979), digital edition. \
Each Game-Turn is one week: initiative, strategic air, naval convoys and stores expenditure, then \
three Operations Stages. In each OpStage both sides do weather and organization (water, \
reorganization, attrition, construction, training, supply distribution, coastal shipping), \
convoy arrivals, the Commonwealth fleet and land-support air missions; then Player A runs \
reserve designation, movement and combat (repeatable), truck convoys, rail, repair and patrols, \
and Player B does the same. Every action costs capability points; supplies (fuel, ammunition, \
stores, water) must reach units by truck from dumps and ports. You see the presence of enemy \
stacks but not their contents. Steps the engine does not implement yet are skipped and reported \
as such.";

/// The `observe` report for a perspective.
pub(crate) fn observe(content: &CnaContent, state: &State, perspective: Perspective) -> Value {
    let anchor = state.cursor.anchor();
    let cases: Vec<String> = content
        .registry
        .procedural_at(anchor, content.scenario_key())
        .map(|c| c.citation())
        .collect();
    let mut forces = serde_json::Map::new();
    for side in Side::ALL {
        if !sees_side(perspective, side) {
            continue;
        }
        let mut on_map = 0;
        let mut off_map = 0;
        let mut awaiting = 0;
        let mut not_arrived = 0;
        for u in state.units_of(side) {
            match u.location {
                Location::Hex { .. } => on_map += 1,
                Location::OffMap { .. } => off_map += 1,
                Location::AwaitingSetup { .. } => awaiting += 1,
                Location::NotArrived => not_arrived += 1,
                Location::Eliminated => {}
            }
        }
        forces.insert(
            crate::state::side_key(side).to_owned(),
            json!({
                "units_on_map": on_map,
                "units_off_map": off_map,
                "units_awaiting_setup_placement": awaiting,
                "units_not_yet_arrived": not_arrived,
                "setup": {
                    "window_closed": state.setup.closed,
                    "unit_destinations": state.setup.unit_locations.iter().filter(|(id, _)| state.land.units.get(*id).is_some_and(|u| u.side == side)).collect::<BTreeMap<_, _>>(),
                    "dump_destinations": state.setup.dump_locations.iter().filter(|(id, _)| state.logistics.dumps.get(*id).is_some_and(|d| d.side == side)).collect::<BTreeMap<_, _>>(),
                    "first_line_pools": state.land.undistributed_trucks.iter().filter(|(group, _)| state.units_of(side).any(|u| u.setup_group.as_ref() == Some(*group))).collect::<BTreeMap<_, _>>(),
                },
            }),
        );
    }
    let enemy_stacks: Vec<String> = state
        .stacks()
        .keys()
        .filter(|(_, side)| !sees_side(perspective, *side))
        .map(|(hex, _)| hex.to_string())
        .collect();
    json!({
        "game": "The Campaign for North Africa",
        "scenario": content.scenario.meta.name,
        "rules_summary": RULES_SUMMARY,
        "clock": wire_clock(content, state),
        "initiative": state.turn.initiative,
        "player_a": state.turn.player_a,
        "current_step": {
            "anchor": anchor,
            "applicable_rule_cases": cases,
        },
        "your_forces": forces,
        "logistics": {
            "truck_pools": state.logistics.truck_pools.iter().filter(|p|sees_side(perspective,p.side)).collect::<Vec<_>>(),
            "well_conditions": state.logistics.wells.keys().filter_map(|hex| {
                let condition=crate::logistics::wells::condition(state,hex,perspective);
                (!condition.as_object().expect("condition object").is_empty()).then_some((hex,condition))
            }).collect::<BTreeMap<_,_>>(),
            "drawn_water": state.logistics.drawn_water.iter().filter(|(id,_)|state.land.units.get(*id).is_some_and(|u|sees_side(perspective,u.side))).collect::<BTreeMap<_,_>>(),
            "rations": state.logistics.rations.iter().filter(|(id, _)| {
                state.land.units.get(*id).is_some_and(|u| sees_side(perspective, u.side))
            }).collect::<BTreeMap<_, _>>(),
            "food_losses": state.logistics.food_losses.iter().filter(|l| sees_side(perspective, l.owner)).collect::<Vec<_>>(),
            "prisoners": state.logistics.prisoners.iter().filter(|(_, p)| sees_side(perspective, p.owner)).collect::<BTreeMap<_, _>>(),
            "unit_supply": state.logistics.unit_supply.iter().filter(|(id, _)| {
                state.land.units.get(*id).is_some_and(|u| sees_side(perspective, u.side))
            }).collect::<BTreeMap<_, _>>(),
            "dumps": state.logistics.dumps.iter().filter(|(_, d)| sees_side(perspective, d.side)).collect::<BTreeMap<_, _>>(),
        },
        "enemy_stack_hexes": enemy_stacks,
        "pending_decisions": view(content, state, perspective).pending,
        "result": state.result,
    })
}

/// Authorized detail about a unit (by id) or a hex (by printed id).
pub(crate) fn inspect(
    content: &CnaContent,
    state: &State,
    perspective: Perspective,
    target: &str,
    strict: bool,
) -> Result<Value, Rejection> {
    let hidden = || illegal(format!("{target}: unknown or not visible"));
    if let Some(unit) = state.land.units.get(&UnitId::new(target)) {
        if !sees_side(perspective, unit.side) {
            return Err(hidden());
        }
        let view = unit_view(content, unit);
        return Ok(json!({
            "unit": view,
            "reachable": crate::land::movement::reachable(content,state,&unit.id,strict),
            "movement_allowance": crate::land::formation::allowance(content,state,&unit.id).map(|a| json!({"cpa":a.cpa,"motorized":a.motorized})),
            "command_role": crate::ownership::seat_for_unit(content,state,&unit.id),
            "moved_this_segment": state.land.movement.moved.contains(&unit.id),
            "location": unit.location,
            "setup_destination": state.setup.unit_locations.get(&unit.id),
            "attached_to": unit.attached_to,
            "trucks": unit.trucks,
            "toe": format!("{:?}", unit.toe),
            "rations": state.logistics.rations.get(&unit.id).cloned().unwrap_or_default(),
            "supplies": state.logistics.unit_supply.get(&unit.id).cloned().unwrap_or_default(),
        }));
    }
    if let Some(pool) = state.logistics.truck_pools.iter().find(|p| p.id == target) {
        if !sees_side(perspective, pool.side) {
            return Err(hidden());
        }
        return Ok(json!({"truck_pool":pool}));
    }
    if let Some(dump) = state.logistics.dumps.get(target) {
        if !sees_side(perspective, dump.side) {
            return Err(hidden());
        }
        return Ok(
            json!({ "dump": dump, "setup_destination": state.setup.dump_locations.get(&dump.id) }),
        );
    }
    let hex = HexId::new(target);
    if let Some(canonical) = content.map.canonical(&hex) {
        let record = content.map.get(canonical);
        let mut stacks = Vec::new();
        for ((h, side), members) in state.stacks() {
            if &h != canonical {
                continue;
            }
            if sees_side(perspective, side) {
                stacks.push(json!({
                    "side": side,
                    "units": members.iter().map(|u| unit_view(content, u)).collect::<Vec<_>>(),
                }));
            } else {
                stacks.push(json!({ "side": side, "units": "hidden (land:3.6)" }));
            }
        }
        return Ok(json!({
            "hex": canonical,
            // Cases: airlog:52.14, airlog:52.16
            "well": crate::logistics::wells::condition(state,canonical,perspective),
            "terrain": record.and_then(|r| r.terrain.clone()),
            "flags": record.map(|r| r.flags.clone()).unwrap_or_default(),
            "stacks": stacks,
        }));
    }
    Err(hidden())
}
