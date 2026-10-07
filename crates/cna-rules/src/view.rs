//! What each perspective may see: the board view, the seats' `observe` report and `inspect`
//! details, filtered by Limited Intelligence (`land:3.6`).
//!
//! Rules as written (`land:3.61`, `land:3.62`, the owner's ruling of 2026-10-07): a player may
//! examine any stack of counters on the game map, so every counter on the map is visible to the
//! other side by its printed face (designation, type, size, nationality, printed stacking points).
//! What a counter contains is not: units attached to it (`land:4.25`, `land:19.12`), strength,
//! TOE, capability, cohesion, supply and status. Off-map boxes are not on the game map. Own-side
//! seats see everything their side knows; the operator sees everything and is labelled as
//! omniscient by the board.

use std::collections::BTreeMap;

use cna_core::clock::{Anchor, Clock};
use cna_core::engine::{Cx, Rejection};
use cna_core::event::EngineEvent;
use cna_core::ids::{HexId, UnitId};
use cna_core::visibility::{Audience, Perspective};
use cna_protocol::{self as wire, Role, Side};
use serde_json::{Value, json};

use crate::content::CnaContent;
use crate::seq::Block;
use crate::state::{LandUnit, Location, State};
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

/// Whether `unit` is a counter on the game map, which the other side sees by its printed face:
/// it is in a map hex and not attached to a parent formation in that hex, whose counter
/// represents it instead (`land:4.25`, `land:19.12`, `land:19.13`). Rules that depend on what a
/// player can see of enemy units use this, never their own reading of attachments.
/// Cases: land:3.62, land:4.25, land:19.12, land:19.13
pub(crate) fn is_map_counter(content: &CnaContent, state: &State, unit: &LandUnit) -> bool {
    unit.location.hex().is_some()
        && !crate::ownership::parent_for_land_unit(content, unit).is_some_and(|parent| {
            state
                .land
                .units
                .get(parent)
                .is_some_and(|p| p.location == unit.location && p.side == unit.side)
        })
}

/// Whether a counter's printed face shows a combat unit: its type symbol (infantry, armor,
/// recce, artillery, anti-tank, anti-air). A headquarters counter shows a headquarters, whatever
/// is attached to it.
/// Cases: land:3.62
#[allow(dead_code)] // for land-0023 and land-0030, which switch to the visible counter type
pub(crate) fn printed_combat_face(content: &CnaContent, id: &UnitId) -> bool {
    crate::land::formation::combat_unit(content, id)
}

/// A map counter as the other side sees it: its printed face and where it is (`land:3.62`).
/// Cases: land:3.62, land:9.22
pub(crate) fn counter_face(content: &CnaContent, unit: &LandUnit) -> wire::UnitView {
    let oa = content.units.units.get(&unit.id);
    let class = oa
        .and_then(|o| o.class.as_ref())
        .and_then(|c| content.units.classes.get(c));
    let mut detail = BTreeMap::new();
    if let Some(oa) = oa {
        detail.insert("counter".to_owned(), json!(oa.counter));
        // The printed stacking point value, not the current one (an HQ with nothing attached
        // counts zero, 9.25, but its counter still shows the printed value).
        if let Some(sp) = oa.stacking_points {
            detail.insert("stacking_points".to_owned(), json!(sp));
        }
    }
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
        parent: None,
        detail: Some(detail),
    }
}

/// A broken-down vehicle marker as the other side sees it: the marker, not its contents
/// (`land:3.62`).
/// Cases: land:3.62, land:21.42
fn marker_face(marker: &crate::land::breakdown::markers::BrokenMarker) -> wire::UnitView {
    wire::UnitView {
        id: marker.id.clone(),
        side: marker.side,
        name: "Broken-down vehicles".to_owned(),
        kind: "broken_vehicle".to_owned(),
        size: "marker".to_owned(),
        nationality: String::new(),
        hex: Some(marker.hex.to_string()),
        parent: None,
        detail: None,
    }
}

/// TOE strength points a unit holds now (`land:3.5`).
pub(crate) fn toe_points(content: &CnaContent, unit: &LandUnit) -> Option<i32> {
    crate::logistics::toe_strength(content, unit)
        .ok()
        .map(|n| n.get())
}

/// Stamp `moved_this_segment` (own detail only) from the movement state.
fn stamp_moved(state: &State, id: &UnitId, view: &mut wire::UnitView) {
    crate::land::combat::stamp_view(state, id, view);
    if let Some(detail) = view.detail.as_mut() {
        detail.insert(
            "box_handling".to_owned(),
            json!(
                state
                    .land
                    .units
                    .get(id)
                    .and_then(|u| u.box_handling.as_ref())
            ),
        );
        detail.insert(
            "box_movement_block".to_owned(),
            json!(crate::logistics::box_handling::blocks_movement(
                state,
                &crate::logistics::box_handling::Carrier::Unit(id.clone())
            )),
        );
        detail.insert(
            "moved_this_segment".to_owned(),
            json!(state.land.movement.moved.contains(id)),
        );
    }
}

/// Whether `event` only re-states what a perspective's board view now shows. Those are derived
/// centrally by [`Sync`]; procedures may emit them, but the central pass replaces
/// them.
fn is_state_sync(event: &wire::GameEvent) -> bool {
    matches!(
        event,
        wire::GameEvent::UnitUpdated { .. }
            | wire::GameEvent::StackUpdated { .. }
            | wire::GameEvent::StackRemoved { .. }
            | wire::GameEvent::MarkerPlaced { .. }
            | wire::GameEvent::MarkerRemoved { .. }
    )
}

/// The board perspectives: each side's and the operator's. A seat sees its side's board.
const BOARDS: [Perspective; 3] = [
    Perspective::Side(Side::Axis),
    Perspective::Side(Side::Commonwealth),
    Perspective::Operator,
];

/// The index in [`BOARDS`] of the board `perspective` sees.
fn board_index(perspective: Perspective) -> usize {
    let side = match perspective {
        Perspective::Operator => return 2,
        Perspective::Side(side) => side,
        Perspective::Seat(seat) => seat.side,
    };
    match side {
        Side::Axis => 0,
        Side::Commonwealth => 1,
    }
}

/// One state's board, built once and shared by every perspective that looks at it: the full view
/// of each unit of the built sides (on the map, in an off-map box or awaiting set-up placement)
/// and of each broken-down vehicle marker, the printed face of every counter on the map, every
/// stack's members, and the markers each side and the operator see. A perspective's board view is
/// a filter of it, so [`view`], [`views`] and [`Sync`] cannot disagree, and unit
/// views are built once per state rather than once per perspective.
#[cfg_attr(test, derive(Debug, PartialEq))]
pub(crate) struct Board {
    /// Unit or broken-down vehicle marker id -> (owning side, full view), for the built sides.
    units: BTreeMap<String, (Side, wire::UnitView)>,
    /// Every counter and broken-down vehicle marker on the map, of every side -> (side, printed
    /// face): what the other side sees of it (`land:3.62`).
    faces: BTreeMap<String, (Side, wire::UnitView)>,
    /// Every occupied (hex, side) with its sorted member ids (the owner's) and the sorted ids of
    /// its counters and markers (what the other side sees, `land:3.62`).
    stacks: BTreeMap<(HexId, Side), (Vec<String>, Vec<String>)>,
    /// The markers each board perspective sees, in [`BOARDS`] order, each sorted by id.
    markers: [Vec<wire::Marker>; 3],
}

impl Board {
    /// The board of `state`, with full unit views for `sides`. Other sides' units are seen only
    /// by the faces of their counters on the map.
    pub(crate) fn new(content: &CnaContent, state: &State, sides: &[Side]) -> Self {
        let mut units = BTreeMap::new();
        let mut faces = BTreeMap::new();
        let mut stacks = BTreeMap::new();
        for ((hex, side), members) in state.stacks() {
            let markers: Vec<_> = state
                .land
                .breakdown
                .markers
                .values()
                .filter(|m| m.side == side && m.hex == hex)
                .collect();
            let mut ids: Vec<_> = members.iter().map(|u| u.id.to_string()).collect();
            ids.extend(markers.iter().map(|m| m.id.clone()));
            ids.sort();
            let mut counters = Vec::new();
            for u in members.iter().filter(|u| is_map_counter(content, state, u)) {
                counters.push(u.id.to_string());
                faces.insert(u.id.to_string(), (side, counter_face(content, u)));
            }
            for m in &markers {
                counters.push(m.id.clone());
                faces.insert(m.id.clone(), (side, marker_face(m)));
            }
            counters.sort();
            if sides.contains(&side) {
                for m in markers {
                    let view = crate::land::breakdown::markers::unit_view(m);
                    units.insert(m.id.clone(), (side, view));
                }
                for u in members {
                    let mut view = unit_view(content, u);
                    // Own units only: whether the unit has already used its move this segment.
                    stamp_moved(state, &u.id, &mut view);
                    units.insert(u.id.to_string(), (side, view));
                }
            }
            stacks.insert((hex, side), (ids, counters));
        }
        // Own units in play but not on a map hex: in an off-map box, or deployed and awaiting the
        // owner's set-up placement. `hex` is null and `detail.location` says which.
        for u in state.land.units.values() {
            if !sides.contains(&u.side) {
                continue;
            }
            let location = match &u.location {
                Location::OffMap { id } => json!({ "at": "off_map", "id": id }),
                Location::AwaitingSetup { group } => {
                    json!({ "at": "awaiting_setup", "group": group })
                }
                Location::Hex { .. } | Location::NotArrived | Location::Eliminated => continue,
            };
            let mut view = unit_view(content, u);
            stamp_moved(state, &u.id, &mut view);
            if let Some(detail) = view.detail.as_mut() {
                detail.insert("location".to_owned(), location);
            }
            units.insert(u.id.to_string(), (u.side, view));
        }
        Board {
            units,
            faces,
            stacks,
            markers: BOARDS.map(|p| markers(state, p)),
        }
    }

    /// The units `perspective` sees, by id: its own side's in full, then the other side's
    /// counters on the map by their faces.
    fn units_for(
        &self,
        perspective: Perspective,
    ) -> impl Iterator<Item = (&String, &wire::UnitView)> {
        let own = self
            .units
            .iter()
            .filter(move |(_, (side, _))| sees_side(perspective, *side));
        let enemy = self
            .faces
            .iter()
            .filter(move |(_, (side, _))| !sees_side(perspective, *side));
        own.chain(enemy).map(|(id, (_, view))| (id, view))
    }

    /// Unit `id` as `perspective` sees it, if it does.
    fn unit_for(&self, perspective: Perspective, id: &str) -> Option<&wire::UnitView> {
        self.units
            .get(id)
            .filter(|(side, _)| sees_side(perspective, *side))
            .or_else(|| {
                self.faces
                    .get(id)
                    .filter(|(side, _)| !sees_side(perspective, *side))
            })
            .map(|(_, view)| view)
    }

    /// The stack at (`hex`, `side`) as `perspective` sees it: every member for its owner, the
    /// counters and markers for anyone else (`land:3.62`).
    fn stack_for(
        perspective: Perspective,
        (hex, side): &(HexId, Side),
        (members, counters): &(Vec<String>, Vec<String>),
    ) -> wire::Stack {
        let ids = if sees_side(perspective, *side) {
            members
        } else {
            counters
        };
        wire::Stack {
            hex: hex.to_string(),
            side: *side,
            visible_count: Some(ids.len() as u32),
            unit_ids: ids.clone(),
        }
    }

    /// `perspective`'s board view, with the state's `clock` and the `pending` decisions it sees.
    fn view_for(
        &self,
        perspective: Perspective,
        clock: wire::Clock,
        pending: Vec<wire::PendingDecision>,
    ) -> wire::ViewState {
        wire::ViewState {
            clock,
            stacks: self
                .stacks
                .iter()
                .map(|(key, ids)| Self::stack_for(perspective, key, ids))
                .collect(),
            units: self
                .units_for(perspective)
                .map(|(id, view)| (id.clone(), view.clone()))
                .collect(),
            markers: self.markers[board_index(perspective)].clone(),
            pending,
        }
    }

    /// [`Board::view_for`] for a board built for one perspective: moves the unit views out
    /// instead of copying them.
    fn into_view(
        mut self,
        perspective: Perspective,
        clock: wire::Clock,
        pending: Vec<wire::PendingDecision>,
    ) -> wire::ViewState {
        wire::ViewState {
            clock,
            stacks: self
                .stacks
                .iter()
                .map(|(key, ids)| Self::stack_for(perspective, key, ids))
                .collect(),
            units: self
                .units
                .into_iter()
                .filter(|(_, (side, _))| sees_side(perspective, *side))
                .chain(
                    self.faces
                        .into_iter()
                        .filter(|(_, (side, _))| !sees_side(perspective, *side)),
                )
                .map(|(id, (_, view))| (id, view))
                .collect(),
            markers: std::mem::take(&mut self.markers[board_index(perspective)]),
            pending,
        }
    }
}

/// The markers `perspective` sees, sorted by id: coastal shipping, supply dumps (contents for
/// the owner only) and the printed condition markers of wells it knows about.
fn markers(state: &State, perspective: Perspective) -> Vec<wire::Marker> {
    let mut markers = crate::logistics::coastal::markers(state);
    markers.extend(
        state
            .logistics
            .dumps
            .values()
            .filter_map(|dump| crate::logistics::dump_markers::marker(dump, perspective)),
    );
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
    // Marker order is public too: never preserve private dump-map key ordering.
    // Cases: land:3.6, land:3.62
    markers.sort_by(|a, b| a.id.cmp(&b.id));
    markers
}

/// Whether every state input [`Board::new`] reads is equal in `a` and `b`, so both states have
/// the same board and every perspective the same units, stacks and markers: the units
/// themselves (their views and faces depend only on the units and the content), the Game-Turn
/// and Operations Stage (stamps such as the 8.88 movement ban expire with the stage; nothing on
/// the board depends on the step within it), the movement and combat stamps, broken-down vehicle
/// markers, and each board perspective's markers. Most engine calls only record answers, and
/// most step changes touch nothing on the board, and this lets them skip building two boards. It
/// must be exact, not merely usually right: a missed input would announce an own change only when
/// some unrelated hidden enemy input also changed, which is a channel. So anything new that
/// `Board::new` reads (directly or through a stamp) must be compared here, and in this crate's
/// tests every skip is verified against both boards.
fn board_unchanged(a: &State, b: &State) -> bool {
    (a.cursor.game_turn, a.cursor.op_stage) == (b.cursor.game_turn, b.cursor.op_stage)
        && a.land.units == b.land.units
        && a.land.movement.moved == b.land.movement.moved
        && a.land.combat.positions == b.land.combat.positions
        && a.land.combat.pinned == b.land.combat.pinned
        && serde_json::to_value(&a.land.breakdown.markers).ok()
            == serde_json::to_value(&b.land.breakdown.markers).ok()
        && BOARDS.into_iter().all(|p| markers(a, p) == markers(b, p))
}

/// Keeps every viewer's live board equal to its snapshot, by construction. An engine call opens
/// one with the state as it was at its start ([`Sync::new`]) and calls [`Sync::point`] before
/// each phase change and at its end. At each point, for each of the three board perspectives
/// (each side and the operator), it compares the board at the previous point with the board now
/// and emits exactly the difference, addressed to that perspective alone (`SideOnly(side)`,
/// `Operator`): a `UnitUpdated` per unit whose view changed or appeared, a `UnitRemoved` per unit
/// that left it (unless a procedure already said why), `StackUpdated`/`StackRemoved` per changed
/// stack, and `MarkerPlaced`/`MarkerRemoved` per changed marker. State-sync events that
/// procedures emitted since the previous point are dropped first, so an enemy can never receive
/// one its view does not justify (faces of counters only, `land:3.62`). Semantic events (moves
/// along paths, dice, decisions, notes, reasoned removals) are kept in order; the derived events
/// follow them.
///
/// The points before phase changes keep the observer's ordered stream independent of how a
/// step's work splits into engine calls: a change made during a step is announced before the
/// next step's phase change, whether the step ran in one call or waited on private decisions in
/// between (docs/engine.md §3 rules 7 and 9).
pub(crate) struct Sync {
    /// The state at the previous point, or an earlier one with the same board.
    last: State,
    /// Where the events emitted since the previous point begin.
    mark: usize,
}

impl Sync {
    pub(crate) fn new(state: &State, cx: &Cx<'_>) -> Self {
        Sync {
            last: state.clone(),
            mark: cx.events.len(),
        }
    }

    /// Announce, here in the event order, the board changes since the previous point.
    pub(crate) fn point(&mut self, content: &CnaContent, state: &State, cx: &mut Cx<'_>) {
        let since: Vec<EngineEvent> = cx.events.drain(self.mark..).collect();
        cx.events
            .extend(since.into_iter().filter(|e| !is_state_sync(&e.event)));
        if board_unchanged(&self.last, state) {
            #[cfg(test)]
            assert_eq!(
                Board::new(content, &self.last, &Side::ALL),
                Board::new(content, state, &Side::ALL),
                "board_unchanged missed an input that Board::new reads"
            );
            self.mark = cx.events.len();
            return;
        }
        let before = &Board::new(content, &self.last, &Side::ALL);
        let after = Board::new(content, state, &Side::ALL);
        let mut derived = Vec::new();
        for (index, perspective) in BOARDS.into_iter().enumerate() {
            let audience = match perspective {
                Perspective::Side(side) => Audience::SideOnly(side),
                _ => Audience::Operator,
            };
            for event in diff(before, &after, index, perspective, &cx.events[self.mark..]) {
                derived.push(EngineEvent::new(audience.clone(), event));
            }
        }
        for event in derived {
            cx.emit(event);
        }
        self.last = state.clone();
        self.mark = cx.events.len();
    }
}

/// What changed between two boards for one board perspective (`index` in [`BOARDS`]), given the
/// events emitted in between.
fn diff(
    before: &Board,
    after: &Board,
    index: usize,
    perspective: Perspective,
    since: &[EngineEvent],
) -> Vec<wire::GameEvent> {
    let mut derived = Vec::new();
    for (id, unit) in after.units_for(perspective) {
        if before.unit_for(perspective, id) != Some(unit) {
            derived.push(wire::GameEvent::UnitUpdated { unit: unit.clone() });
        }
    }
    for (id, _) in before.units_for(perspective) {
        let explained = since.iter().any(|e| {
            perspective.can_see(&e.audience)
                && matches!(&e.event, wire::GameEvent::UnitRemoved { unit_id, .. } if unit_id == id)
        });
        if after.unit_for(perspective, id).is_none() && !explained {
            derived.push(wire::GameEvent::UnitRemoved {
                unit_id: id.clone(),
                reason: "no longer in view".to_owned(),
            });
        }
    }
    // A stack changes for this perspective when what it sees of it changes: an enemy stack only
    // when its counters do, never when something attaches inside one.
    for (key, ids) in &after.stacks {
        let stack = Board::stack_for(perspective, key, ids);
        let changed = before
            .stacks
            .get(key)
            .is_none_or(|old| Board::stack_for(perspective, key, old) != stack);
        if changed {
            derived.push(wire::GameEvent::StackUpdated { stack });
        }
    }
    for (hex, side) in before.stacks.keys() {
        if !after.stacks.contains_key(&(hex.clone(), *side)) {
            derived.push(wire::GameEvent::StackRemoved {
                hex: hex.to_string(),
                side: *side,
            });
        }
    }
    let markers = |board: &Board| -> BTreeMap<String, wire::Marker> {
        board.markers[index]
            .iter()
            .map(|m| (m.id.clone(), m.clone()))
            .collect()
    };
    let (m0, m1) = (markers(before), markers(after));
    for (id, marker) in &m1 {
        if m0.get(id) != Some(marker) {
            derived.push(wire::GameEvent::MarkerPlaced {
                marker: marker.clone(),
            });
        }
    }
    for id in m0.keys() {
        if !m1.contains_key(id) {
            derived.push(wire::GameEvent::MarkerRemoved {
                marker_id: id.clone(),
            });
        }
    }
    derived
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
    detail.insert("engaged".to_owned(), json!(unit.engaged));
    detail.insert("reserve".to_owned(), json!(unit.reserve));
    detail.insert(
        "assigned_to".to_owned(),
        json!(crate::ownership::assigned_parent(
            content,
            &unit.id,
            Some(unit)
        )),
    );
    detail.insert("trucks".to_owned(), json!(unit.trucks));
    detail.insert("transport_trucks".to_owned(), json!(unit.transport_trucks));
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
        parent: crate::ownership::parent_for_land_unit(content, unit).map(ToString::to_string),
        detail: Some(detail),
    }
}

/// The board view for one perspective.
pub(crate) fn view(
    content: &CnaContent,
    state: &State,
    perspective: Perspective,
) -> wire::ViewState {
    let sides: Vec<Side> = Side::ALL
        .into_iter()
        .filter(|s| sees_side(perspective, *s))
        .collect();
    Board::new(content, state, &sides).into_view(
        perspective,
        wire_clock(content, state),
        pending_for(state, perspective),
    )
}

/// The board views of several perspectives of one state, in order: one shared [`Board`] and
/// each pending decision's schema built once. Equal to calling [`view`] for each.
pub(crate) fn views(
    content: &CnaContent,
    state: &State,
    perspectives: &[Perspective],
) -> Vec<wire::ViewState> {
    let sides: Vec<Side> = Side::ALL
        .into_iter()
        .filter(|s| perspectives.iter().any(|p| sees_side(*p, *s)))
        .collect();
    let board = Board::new(content, state, &sides);
    let clock = wire_clock(content, state);
    let pending: Vec<_> = state
        .decisions
        .pending
        .iter()
        .map(|p| (p.seat, pending_view(p)))
        .collect();
    perspectives
        .iter()
        .map(|&perspective| {
            let seen = pending
                .iter()
                .filter(|(seat, _)| perspective.can_see(&Audience::Seat(*seat)))
                .map(|(_, d)| d.clone())
                .collect();
            board.view_for(perspective, clock.clone(), seen)
        })
        .collect()
}

/// The pending decisions `perspective` may see, as the board lists them.
fn pending_for(state: &State, perspective: Perspective) -> Vec<wire::PendingDecision> {
    state
        .decisions
        .pending
        .iter()
        .filter(|p| perspective.can_see(&Audience::Seat(p.seat)))
        .map(pending_view)
        .collect()
}

fn pending_view(p: &crate::state::Pending) -> wire::PendingDecision {
    wire::PendingDecision {
        id: p.id.to_string(),
        seat: p.seat.to_string(),
        kind: p.kind.clone(),
        summary: p.summary.clone(),
        opened_seq: 0,
        rules: p.rules.clone(),
        space: Some(p.space.to_json_schema()),
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
stores, water) must reach units by truck from dumps and ports. You see every enemy counter on the \
map by its printed face (designation, type, size) and watch it move, but not what it contains: \
attached units, strength, supplies or status. Steps the engine does not implement yet are skipped \
and reported as such.";

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
    // The enemy counters on the map by printed face, counted by type and size per hex; inspecting
    // a hex lists each counter's face with its id (land:3.62).
    let mut enemy_counters: BTreeMap<String, BTreeMap<String, u32>> = BTreeMap::new();
    for ((hex, side), members) in state.stacks() {
        if sees_side(perspective, side) {
            continue;
        }
        let faces = enemy_counters.entry(hex.to_string()).or_default();
        for u in members.iter().filter(|u| is_map_counter(content, state, u)) {
            let face = counter_face(content, u);
            *faces
                .entry(format!("{} {}", face.kind, face.size).trim().to_owned())
                .or_default() += 1;
        }
        for _ in state
            .land
            .breakdown
            .markers
            .values()
            .filter(|m| m.side == side && m.hex == hex)
        {
            *faces.entry("broken_vehicle marker".to_owned()).or_default() += 1;
        }
    }
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
        "air": {
            "forces": state.air.forces.iter().filter(|(force,_)|sees_side(perspective,if force.as_str()=="axis"{Side::Axis}else{Side::Commonwealth})).collect::<BTreeMap<_,_>>(),
            // Squadron detail is for the seats that fly them (and the side and operator views);
            // the ground seats' observations stay small. Any seat may still inspect a squadron.
            "squadrons": if flies_air(perspective) {
                json!(state.air.squadrons.iter().filter(|(_,s)|sees_side(perspective,s.side)).collect::<BTreeMap<_,_>>())
            } else {
                json!(format!("{} own squadrons; the air and commander seats see their detail", state.air.squadrons.values().filter(|s| sees_side(perspective, s.side)).count()))
            },
        },
        "logistics": {
            "cargo_history": crate::logistics::cargo_history::disclosed(state,perspective),
            "truck_pool_destinations": state.setup.pool_locations.iter().filter(|(id,_)|state.logistics.truck_pools.iter().any(|p|&p.id==*id&&sees_side(perspective,p.side))).collect::<BTreeMap<_,_>>(),
            "truck_pools": state.logistics.truck_pools.iter().filter(|p|sees_side(perspective,p.side)).collect::<Vec<_>>(),
            "well_conditions": state.logistics.wells.keys().filter_map(|hex| {
                let condition=crate::logistics::wells::condition(state,hex,perspective);
                (!condition.as_object().expect("condition object").is_empty()).then_some((hex,condition))
            }).collect::<BTreeMap<_,_>>(),
            "drawn_water": state.logistics.drawn_water.iter().filter(|(id,_)|state.land.units.get(*id).is_some_and(|u|sees_side(perspective,u.side))).collect::<BTreeMap<_,_>>(),
            "rations": ration_problems(state, perspective),
            "food_losses": state.logistics.food_losses.iter().filter(|l| sees_side(perspective, l.owner)).collect::<Vec<_>>(),
            "prisoners": state.logistics.prisoners.iter().filter(|(_, p)| sees_side(perspective, p.owner)).collect::<BTreeMap<_, _>>(),
            "unit_box_handling": state.land.units.iter().filter(|(_,u)|sees_side(perspective,u.side)).filter_map(|(id,u)|u.box_handling.as_ref().map(|h|(id,h))).collect::<BTreeMap<_,_>>(),
            "unit_supply": state.logistics.unit_supply.iter().filter(|(id, _)| {
                state.land.units.get(*id).is_some_and(|u| sees_side(perspective, u.side))
            }).collect::<BTreeMap<_, _>>(),
            "coastal_ships": if sees_side(perspective,Side::Axis){serde_json::to_value(&state.logistics.coastal_ships).unwrap()}else{json!({})},
            "convoy_turns": if sees_side(perspective,Side::Axis){serde_json::to_value(&state.logistics.convoy_turns).unwrap()}else{json!({})},
            "ports":state.logistics.ports.iter().filter(|(_,p)|sees_side(perspective,p.owner)).collect::<BTreeMap<_,_>>(),
            "dumps": state.logistics.dumps.iter().filter(|(_, d)| sees_side(perspective, d.side)).collect::<BTreeMap<_, _>>(),
        },
        "combat": {
            "force_assignment":crate::land::combat::assignment::disclosed(content,state,perspective),
            "barrage_targets": crate::land::combat::barrage::disclosed(state,perspective),
            "barrage_plans": state.land.combat.barrage.plans.iter().filter(|(seat,_)|sees_side(perspective,seat.side)).collect::<BTreeMap<_,_>>(),
            "retreat_plans": state.land.combat.retreat.plans.iter().filter(|(seat,_)|sees_side(perspective,seat.side)).collect::<BTreeMap<_,_>>(),
            "retreated": state.land.combat.retreat.retreated.iter().filter(|id|state.land.units.get(*id).is_some_and(|u|sees_side(perspective,u.side))).collect::<Vec<_>>(),
            "pinned": state.land.combat.pinned.iter().filter(|id|state.land.units.get(*id).is_some_and(|u|sees_side(perspective,u.side))).collect::<Vec<_>>(),
            "positions": state.land.combat.positions.iter().filter(|(id,_)| state.land.units.get(*id).is_some_and(|u| sees_side(perspective,u.side))).collect::<BTreeMap<_,_>>(),
            "position_orders": state.land.combat.position_orders.iter().filter(|(seat,_)| sees_side(perspective,seat.side)).collect::<BTreeMap<_,_>>(),
        },
        "enemy_counters": enemy_counters,
        "pending_decisions": pending_for(state, perspective),
        "result": state.result,
    })
}

/// Whether `perspective` gets full squadron detail in `observe`: the side and operator views and
/// the air and commander seats.
fn flies_air(perspective: Perspective) -> bool {
    match perspective {
        Perspective::Seat(seat) => matches!(seat.role, Role::Air | Role::Commander),
        Perspective::Side(_) | Perspective::Operator => true,
    }
}

/// Whether `perspective` gets ration problems as unit lists in `observe`: the logistics seat that
/// feeds the units, and the side and operator views. Other seats get the counts.
fn feeds_units(perspective: Perspective) -> bool {
    match perspective {
        Perspective::Seat(seat) => seat.role == Role::Logistics,
        Perspective::Side(_) | Perspective::Operator => true,
    }
}

/// Own units with a current ration or water problem, grouped by problem as lists of unit ids:
/// on half rations, by consecutive short weeks, by consecutive short water stages. Each unit's
/// full record is in `inspect`; listing every unit's bookkeeping here was three quarters of a
/// seat's observation, paid in model tokens on every decision. Seats other than logistics get
/// each list's length instead (late in a turn the lists alone were 12 KB). Empty when nothing is
/// wrong.
fn ration_problems(state: &State, perspective: Perspective) -> serde_json::Map<String, Value> {
    let mut half = Vec::new();
    let mut weeks: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut water: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, r) in &state.logistics.rations {
        if !state
            .land
            .units
            .get(id)
            .is_some_and(|u| sees_side(perspective, u.side))
        {
            continue;
        }
        if r.half {
            half.push(id.to_string());
        }
        if r.consecutive_short_gt > 0 {
            weeks
                .entry(r.consecutive_short_gt.to_string())
                .or_default()
                .push(id.to_string());
        }
        if r.consecutive_short_water_stages > 0 {
            water
                .entry(r.consecutive_short_water_stages.to_string())
                .or_default()
                .push(id.to_string());
        }
    }
    let mut out = serde_json::Map::new();
    if !feeds_units(perspective) {
        let count = |groups: &BTreeMap<String, Vec<String>>| -> BTreeMap<String, usize> {
            groups.iter().map(|(k, v)| (k.clone(), v.len())).collect()
        };
        if !half.is_empty() {
            out.insert("half_rations_units".into(), json!(half.len()));
        }
        if !weeks.is_empty() {
            out.insert(
                "short_of_stores_units_by_consecutive_weeks".into(),
                json!(count(&weeks)),
            );
        }
        if !water.is_empty() {
            out.insert(
                "short_of_water_units_by_consecutive_stages".into(),
                json!(count(&water)),
            );
        }
        if !out.is_empty() {
            out.insert(
                "detail".into(),
                json!("the logistics seat sees the unit lists; inspect a unit for its record"),
            );
        }
        return out;
    }
    if !half.is_empty() {
        out.insert("half_rations".into(), json!(half));
    }
    if !weeks.is_empty() {
        out.insert("short_of_stores_by_consecutive_weeks".into(), json!(weeks));
    }
    if !water.is_empty() {
        out.insert("short_of_water_by_consecutive_stages".into(), json!(water));
    }
    out
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
    let face_only = "printed face only: what the counter contains is not visible (land:3.62)";
    if let Some(unit) = state.land.units.get(&UnitId::new(target)) {
        if !sees_side(perspective, unit.side) {
            // An enemy counter on the map shows its face; anything else about enemy units,
            // including whether an attached unit exists, is answered like an unknown id.
            if is_map_counter(content, state, unit) {
                return Ok(json!({ "unit": counter_face(content, unit), "visibility": face_only }));
            }
            return Err(hidden());
        }
        let view = unit_view(content, unit);
        return Ok(json!({
            "unit": view,
            "reachable": if state.cursor.anchor()==crate::land::combat::retreat::ANCHOR {
                crate::land::combat::retreat::reachable(content,state,&unit.id,strict)
            } else {crate::land::movement::reachable(content,state,&unit.id,strict)},
            "fuel_truck_cohorts": crate::logistics::segment_fuel_cohorts(state,&unit.id).ok(),
            "reaction_cpa_options": state.land.reaction.window.as_ref().and_then(|w|w.cpa_options.get(&unit.id)),
            "reaction_division_paths": crate::land::reaction::plans(content,state,&unit.id,strict),
            "retreated_before_assault": state.land.combat.retreat.retreated.contains(&unit.id),
            "movement_allowance": crate::land::formation::allowance(content,state,&unit.id).map(|a| json!({"cpa":a.cpa,"motorized":a.motorized})),
            "command_role": crate::ownership::seat_for_unit(content,state,&unit.id),
            "gun_position": state.land.combat.positions.get(&unit.id),
            "moved_this_segment": state.land.movement.moved.contains(&unit.id),
            "repeat_movement_allowed": crate::land::cycles::movement_allowed(state, &unit.id),
            "reserve": unit.reserve,
            "engaged": unit.engaged,
            "unresolved_embarked":state.land.breakdown.unresolved_passengers.get(&unit.id),
            "breakdown_points_quarters":state.land.breakdown.accumulated_quarters.get(&unit.id).copied().unwrap_or(0),
            "light_truck_extra_breakdown_quarters":state.land.breakdown.light_extra_quarters.get(&unit.id).copied().unwrap_or(0),
            "assault_intentions": state.land.assault_intentions.get(&unit.id),
            "movement_restrictions": crate::land::formation::members(content,state,&unit.id).into_iter().map(|id| {
                let assessment=crate::logistics::movement_restrictions(content,state,&id).map(|r|json!({
                    "may_move":r.may_move,"may_exceed_cpa":r.may_exceed_cpa,"may_enter_enemy_zoc":r.may_enter_enemy_zoc,
                })).unwrap_or_else(|e|match e {
                    crate::logistics::SupplyError::Unsupported {case}=>json!({"assessment_error":"Water requirement is unknown.","case":case}),
                    _=>json!({"assessment_error":"Water requirement could not be assessed."}),
                });
                json!({"unit":id,"restrictions":assessment})
            }).collect::<Vec<_>>(),
            "location": unit.location,
            "transit": crate::land::offmap::transit_for_unit(state, &unit.id),
            "setup_destination": state.setup.unit_locations.get(&unit.id),
            "attached_to": unit.attached_to,
            "assigned_to": crate::ownership::assigned_parent_for_unit(content,state,&unit.id),
            "trucks": unit.trucks,
            "box_handling": unit.box_handling,
            "box_movement_block": crate::logistics::box_handling::blocks_movement(state,&crate::logistics::box_handling::Carrier::Unit(unit.id.clone())),
            "toe": format!("{:?}", unit.toe),
            "rations": state.logistics.rations.get(&unit.id).cloned().unwrap_or_default(),
            "supplies": state.logistics.unit_supply.get(&unit.id).cloned().unwrap_or_default(),
        }));
    }
    if let Some(ship) = state.logistics.coastal_ships.get(target) {
        if !sees_side(perspective, Side::Axis) {
            return Err(hidden());
        }
        return Ok(json!({"coastal_ship": ship, "id":target,
            "capacity_tons":content.units.coastal_ships.get(target).and_then(|s|s.capacity_tons)}));
    }
    if let Some(squadron) = state.air.squadrons.get(target) {
        if !sees_side(perspective, squadron.side) {
            return Err(hidden());
        }
        return Ok(json!({"squadron":squadron}));
    }
    if let Some(marker) = state.land.breakdown.markers.get(target) {
        if !sees_side(perspective, marker.side) {
            return Ok(json!({ "unit": marker_face(marker), "visibility": face_only }));
        }
        return Ok(json!({"broken_vehicles":marker}));
    }
    if let Some(pool) = state.logistics.truck_pools.iter().find(|p| p.id == target) {
        if !sees_side(perspective, pool.side) {
            return Err(hidden());
        }
        return Ok(
            json!({"truck_pool":pool,"setup_destination":state.setup.pool_locations.get(&pool.id)}),
        );
    }
    if let Some(dump) = state
        .logistics
        .dumps
        .values()
        .find(|d| !d.marker.is_empty() && d.marker == target)
    {
        if sees_side(perspective, dump.side) {
            return Ok(
                json!({"dump":dump, "setup_destination":state.setup.dump_locations.get(&dump.id)}),
            );
        }
        if let Some(marker) = crate::logistics::dump_markers::marker(dump, perspective) {
            return Ok(json!({"marker":marker}));
        }
        return Err(hidden());
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
                    "broken_vehicles":state.land.breakdown.markers.values().filter(|m|m.side==side&&&m.hex==canonical).collect::<Vec<_>>(),
                    "units": members.iter().map(|u| unit_view(content, u)).collect::<Vec<_>>(),
                }));
            } else {
                let mut counters: Vec<wire::UnitView> = members
                    .iter()
                    .filter(|u| is_map_counter(content, state, u))
                    .map(|u| counter_face(content, u))
                    .collect();
                counters.extend(
                    state
                        .land
                        .breakdown
                        .markers
                        .values()
                        .filter(|m| m.side == side && &m.hex == canonical)
                        .map(marker_face),
                );
                counters.sort_by(|a, b| a.id.cmp(&b.id));
                stacks.push(json!({ "side": side, "counters": counters, "visibility": face_only }));
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
