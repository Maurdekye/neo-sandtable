//! **sandbox-v1**: a small synthetic wargame on the real CNA map grid, used to exercise the
//! whole neo-sandtable pipeline (runner, persistence, live board, AI seats) before the real
//! rules are implemented. **It is not The Campaign for North Africa** and nothing here cites a
//! rule case; its mechanics are invented and deliberately simple.
//!
//! What it exercises:
//! - all five seats per side (commander: initiative; logistics: secret supply allocation; front
//!   line: movement and assaults; air: simultaneous secret air allocation; rear area: repairs);
//! - scheduled decisions, a triggered decision that interrupts movement (reaction), and a
//!   simultaneous secret window;
//! - hidden stack contents (`land:3.6`-style: enemy stacks visible, contents not);
//! - dice, losses, retreats, elimination, objectives and a game end.
//!
//! Every action space is enumerable (choices, unit ids, hex lists, small integers), so legal
//! random, System 1 and LLM controllers can all play it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cna_content::map::MapContent;
use cna_core::clock::{Anchor, Clock};
use cna_core::decision::{
    ActionSchema, ActionSpace, ChoiceOption, DecisionRequest, DecisionResponse, FieldSchema,
    Secrecy, Trigger,
};
use cna_core::engine::{Cx, EngineError, Progress, Rejection, Ruleset};
use cna_core::event::{EngineEvent, GameEvent};
use cna_core::hex::Axial;
use cna_core::ids::{DecisionId, HexId, Role, SeatId, Side, UnitId};
use cna_core::visibility::{Audience, Perspective};
use cna_protocol as wire;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const PROFILE_ID: &str = "sandbox-v1";
pub const MAX_TURNS: u16 = 6;
const STACK_LIMIT: usize = 4;
const AIR_POINTS: i64 = 3;
const REGION_CENTER: Axial = Axial::new(80, 21);
const REGION_RADIUS: u32 = 8;

/// A short explanation of the sandbox's invented rules, given to AI seats in observations.
pub const RULES_SUMMARY: &str = "SANDBOX (synthetic test game, not CNA). 6 turns. Each turn a \
commander chooses whether their side moves first. Each side in turn: LOGISTICS secretly picks \
which units are supplied (unsupplied units move at half speed); FRONT LINE orders moves \
(each unit to one reachable hex; entering a hex next to an enemy stack ends a unit's move, and \
the threatened enemy may react by holding or withdrawing one hex); if any stacks are in \
contact, both AIR seats secretly split 3 air points among contact hexes (attacker's points \
add to attacks on that enemy hex, defender's points add to defense of its own hex); FRONT LINE \
then chooses assaults (each of its stacks may attack one adjacent enemy stack); REAR AREA may \
repair one damaged unit (+1 strength). Assault: roll two dice, add (attack - defense); \
<=4 attacker loses 2, 5-6 attacker loses 1, 7 both lose 1, 8-9 defender loses 1, 10-11 \
defender loses 1 and retreats, >=12 defender loses 2 and retreats. Objective hexes add 1 to \
defense. At the end of each turn each side scores 1 victory point per objective it holds \
(occupied alone, or last held). Most victory points after turn 6 wins.";

// ---------------------------------------------------------------------------------------------
// Content
// ---------------------------------------------------------------------------------------------

/// (id, side, name, kind, size, nationality, strength, cp, target axial (q, r))
type SetupRow<'a> = (
    &'a str,
    Side,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    i32,
    i32,
    (i32, i32),
);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitSetup {
    pub id: UnitId,
    pub side: Side,
    pub name: String,
    pub kind: String,
    pub size: String,
    pub nationality: String,
    pub strength: i32,
    pub cp: i32,
    pub hex: HexId,
}

/// The sandbox's static content: a region of the real map grid, objectives and the setup.
#[derive(Debug, Clone)]
pub struct SandboxContent {
    hexes: BTreeMap<HexId, Axial>,
    by_axial: BTreeMap<Axial, HexId>,
    pub objectives: Vec<HexId>,
    pub setup: Vec<UnitSetup>,
}

impl SandboxContent {
    /// Build the sandbox around the Libya–Egypt frontier from the published map grid.
    pub fn from_map(map: &MapContent) -> Result<Self, String> {
        let mut hexes = BTreeMap::new();
        let mut by_axial = BTreeMap::new();
        for h in map.iter() {
            if h.axial.distance(REGION_CENTER) <= REGION_RADIUS {
                hexes.insert(h.id.clone(), h.axial);
                by_axial.insert(h.axial, h.id.clone());
            }
        }
        if hexes.len() < 100 {
            return Err(format!("sandbox region too small: {} hexes", hexes.len()));
        }
        let mut used = BTreeSet::new();
        let mut snap = |q: i32, r: i32| -> Result<HexId, String> {
            let target = Axial::new(q, r);
            let best = hexes
                .iter()
                .filter(|(id, _)| !used.contains(*id))
                .min_by_key(|(id, a)| (a.distance(target), (*id).clone()))
                .map(|(id, _)| id.clone())
                .ok_or("no free hex")?;
            used.insert(best.clone());
            Ok(best)
        };
        let objectives = vec![snap(80, 22)?, snap(83, 20)?, snap(82, 23)?];

        let units: [SetupRow<'_>; 9] = [
            (
                "it.sb.1",
                Side::Axis,
                "1st Division (sandbox)",
                "infantry",
                "division",
                "italian",
                6,
                4,
                (74, 21),
            ),
            (
                "it.sb.2",
                Side::Axis,
                "2nd Division (sandbox)",
                "infantry",
                "division",
                "italian",
                6,
                4,
                (75, 22),
            ),
            (
                "it.sb.3",
                Side::Axis,
                "3rd Division (sandbox)",
                "infantry",
                "division",
                "italian",
                6,
                4,
                (74, 23),
            ),
            (
                "it.sb.4",
                Side::Axis,
                "Tank Group (sandbox)",
                "armor",
                "regiment",
                "italian",
                4,
                8,
                (76, 20),
            ),
            (
                "it.sb.5",
                Side::Axis,
                "Artillery Group (sandbox)",
                "artillery",
                "regiment",
                "italian",
                3,
                4,
                (73, 22),
            ),
            (
                "cw.sb.1",
                Side::Commonwealth,
                "Armoured Brigade (sandbox)",
                "armor",
                "brigade",
                "british",
                5,
                10,
                (87, 20),
            ),
            (
                "cw.sb.2",
                Side::Commonwealth,
                "Infantry Brigade A (sandbox)",
                "infantry",
                "brigade",
                "british",
                4,
                5,
                (86, 21),
            ),
            (
                "cw.sb.3",
                Side::Commonwealth,
                "Infantry Brigade B (sandbox)",
                "infantry",
                "brigade",
                "indian",
                4,
                5,
                (85, 23),
            ),
            (
                "cw.sb.4",
                Side::Commonwealth,
                "Support Group (sandbox)",
                "artillery",
                "brigade",
                "british",
                3,
                6,
                (86, 22),
            ),
        ];
        let mut setup = Vec::new();
        for (id, side, name, kind, size, nat, strength, cp, (q, r)) in units {
            setup.push(UnitSetup {
                id: id.into(),
                side,
                name: name.into(),
                kind: kind.into(),
                size: size.into(),
                nationality: nat.into(),
                strength,
                cp,
                hex: snap(q, r)?,
            });
        }
        Ok(Self {
            hexes,
            by_axial,
            objectives,
            setup,
        })
    }

    fn axial(&self, hex: &HexId) -> Option<Axial> {
        self.hexes.get(hex).copied()
    }

    fn neighbors(&self, hex: &HexId) -> Vec<HexId> {
        match self.axial(hex) {
            None => Vec::new(),
            Some(a) => a
                .neighbors()
                .into_iter()
                .filter_map(|n| self.by_axial.get(&n).cloned())
                .collect(),
        }
    }

    fn adjacent(&self, a: &HexId, b: &HexId) -> bool {
        matches!((self.axial(a), self.axial(b)), (Some(x), Some(y)) if x.distance(y) == 1)
    }

    fn distance(&self, a: &HexId, b: &HexId) -> u32 {
        match (self.axial(a), self.axial(b)) {
            (Some(x), Some(y)) => x.distance(y),
            _ => u32::MAX,
        }
    }

    /// Every hex in the sandbox region, for the board and tests.
    pub fn region(&self) -> impl Iterator<Item = &HexId> {
        self.hexes.keys()
    }
}

// ---------------------------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unit {
    pub id: UnitId,
    pub side: Side,
    pub name: String,
    pub kind: String,
    pub size: String,
    pub nationality: String,
    pub hex: HexId,
    pub strength: i32,
    pub max_strength: i32,
    pub cp: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    Start,
    Initiative,
    Supply { half: u8 },
    Movement { half: u8 },
    ExecuteMoves { half: u8 },
    Air { half: u8 },
    Assault { half: u8 },
    Repair { half: u8 },
    EndOfTurn,
    Finished,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PendingContext {
    None,
    Reaction { threatened: HexId, from: HexId },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    pub id: DecisionId,
    pub seat: SeatId,
    pub kind: String,
    pub summary: String,
    pub trigger: Trigger,
    pub secrecy: Secrecy,
    pub space: ActionSpace,
    pub context: PendingContext,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub turn: u16,
    pub step: Step,
    pub first: Side,
    pub units: BTreeMap<UnitId, Unit>,
    pub vp: BTreeMap<Side, i32>,
    pub control: BTreeMap<HexId, Side>,
    pub pending: Vec<Pending>,
    pub next_decision: u32,
    pub supplied: BTreeSet<UnitId>,
    pub move_queue: VecDeque<(UnitId, HexId)>,
    pub reacted: BTreeSet<HexId>,
    pub air_plans: BTreeMap<Side, BTreeMap<HexId, i64>>,
    pub result: Option<String>,
}

impl State {
    /// The initial state for a new sandbox campaign.
    pub fn new(content: &SandboxContent) -> Self {
        let units = content
            .setup
            .iter()
            .map(|u| {
                (
                    u.id.clone(),
                    Unit {
                        id: u.id.clone(),
                        side: u.side,
                        name: u.name.clone(),
                        kind: u.kind.clone(),
                        size: u.size.clone(),
                        nationality: u.nationality.clone(),
                        hex: u.hex.clone(),
                        strength: u.strength,
                        max_strength: u.strength,
                        cp: u.cp,
                    },
                )
            })
            .collect();
        State {
            turn: 0,
            step: Step::Start,
            first: Side::Axis,
            units,
            vp: Side::ALL.into_iter().map(|s| (s, 0)).collect(),
            control: BTreeMap::new(),
            pending: Vec::new(),
            next_decision: 1,
            supplied: BTreeSet::new(),
            move_queue: VecDeque::new(),
            reacted: BTreeSet::new(),
            air_plans: BTreeMap::new(),
            result: None,
        }
    }

    fn phasing(&self, half: u8) -> Side {
        if half == 0 {
            self.first
        } else {
            self.first.opponent()
        }
    }

    fn units_of(&self, side: Side) -> impl Iterator<Item = &Unit> {
        self.units.values().filter(move |u| u.side == side)
    }

    fn stack_hexes(&self, side: Side) -> BTreeSet<HexId> {
        self.units_of(side).map(|u| u.hex.clone()).collect()
    }

    fn stack(&self, hex: &HexId, side: Side) -> Vec<&Unit> {
        self.units
            .values()
            .filter(|u| u.side == side && &u.hex == hex)
            .collect()
    }

    fn occupant(&self, hex: &HexId) -> Option<Side> {
        self.units.values().find(|u| &u.hex == hex).map(|u| u.side)
    }

    fn anchor(&self) -> &'static str {
        match self.step {
            Step::Start | Step::Initiative => "initiative",
            Step::Supply { .. } => "opstage.organization.supply_distribution",
            Step::Movement { .. } | Step::ExecuteMoves { .. } => {
                "opstage.movement_and_combat.movement"
            }
            Step::Air { .. } => "opstage.land_support_air.assignment",
            Step::Assault { .. } => "opstage.movement_and_combat.combat.close_assault",
            Step::Repair { .. } => "opstage.repair.maintenance",
            Step::EndOfTurn | Step::Finished => "end_of_turn",
        }
    }

    fn half(&self) -> Option<u8> {
        match self.step {
            Step::Supply { half }
            | Step::Movement { half }
            | Step::ExecuteMoves { half }
            | Step::Air { half }
            | Step::Assault { half }
            | Step::Repair { half } => Some(half),
            _ => None,
        }
    }

    pub fn clock(&self) -> Clock {
        let half = self.half();
        Clock {
            game_turn: self.turn.max(1),
            op_stage: half.map(|_| 1),
            anchor: Anchor::new(self.anchor()),
            phasing: half.map(|h| self.phasing(h)),
            cycle: None,
        }
    }

    pub fn wire_clock(&self) -> wire::Clock {
        let clock = self.clock();
        let parts: Vec<&str> = clock.anchor.parts().collect();
        let stage = parts.first().copied().unwrap_or("").to_owned();
        let (phase, segment, step) = if stage == "opstage" {
            (
                parts.get(1).copied().unwrap_or("").to_owned(),
                parts.get(2).map(|s| (*s).to_owned()),
                parts.get(3).map(|s| (*s).to_owned()),
            )
        } else {
            (stage.clone(), None, None)
        };
        wire::Clock {
            game_turn: clock.game_turn,
            date: turn_date(clock.game_turn),
            stage,
            op_stage: clock.op_stage,
            phase,
            segment,
            step,
            phasing: clock.phasing,
        }
    }
}

/// The game date of a turn's first day: Game-Turn 1 begins 15 September 1940.
fn turn_date(turn: u16) -> String {
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
    let start = days_from_civil(1940, 9, 15);
    let (y, m, d) = civil_from_days(start + 7 * (i64::from(turn.max(1)) - 1));
    format!("{y:04}-{m:02}-{d:02}")
}

// ---------------------------------------------------------------------------------------------
// The ruleset
// ---------------------------------------------------------------------------------------------

/// The sandbox ruleset.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sandbox;

fn illegal(message: impl Into<String>) -> Rejection {
    Rejection::Illegal {
        message: message.into(),
    }
}

impl Sandbox {
    #[allow(clippy::too_many_arguments)] // a private helper mirroring DecisionRequest's fields
    fn open(
        &self,
        state: &mut State,
        cx: &mut Cx<'_>,
        seat: SeatId,
        kind: &str,
        summary: String,
        trigger: Trigger,
        secrecy: Secrecy,
        space: ActionSpace,
        context: PendingContext,
    ) {
        let id = DecisionId::new(format!("d{}", state.next_decision));
        state.next_decision += 1;
        cx.emit(EngineEvent::new(
            Audience::Seat(seat),
            GameEvent::DecisionOpened {
                decision: wire::PendingDecision {
                    id: id.to_string(),
                    seat: seat.to_string(),
                    kind: kind.to_owned(),
                    summary: summary.clone(),
                    opened_seq: 0,
                    rules: Vec::new(),
                },
            },
        ));
        state.pending.push(Pending {
            id,
            seat,
            kind: kind.to_owned(),
            summary,
            trigger,
            secrecy,
            space,
            context,
        });
    }

    fn set_step(&self, state: &mut State, step: Step, cx: &mut Cx<'_>) {
        let before = state.anchor();
        let before_phasing = state.half().map(|h| state.phasing(h));
        state.step = step;
        if state.anchor() != before || state.half().map(|h| state.phasing(h)) != before_phasing {
            cx.emit(EngineEvent::public(GameEvent::PhaseChanged {
                clock: state.wire_clock(),
            }));
        }
    }

    fn after_half(&self, state: &mut State, half: u8, cx: &mut Cx<'_>) {
        state.air_plans.clear();
        if half == 0 {
            self.set_step(state, Step::Supply { half: 1 }, cx);
        } else {
            self.set_step(state, Step::EndOfTurn, cx);
        }
    }

    /// Hexes a unit can reach this phase, with the path to each (BFS, 1 CP per hex).
    fn reachable(
        &self,
        content: &SandboxContent,
        state: &State,
        unit: &Unit,
    ) -> BTreeMap<HexId, Vec<HexId>> {
        let budget = if state.supplied.contains(&unit.id) {
            unit.cp
        } else {
            (unit.cp / 2).max(1)
        };
        let enemy = state.stack_hexes(unit.side.opponent());
        let in_contact = |hex: &HexId| content.neighbors(hex).iter().any(|n| enemy.contains(n));
        let mut paths: BTreeMap<HexId, Vec<HexId>> = BTreeMap::new();
        let mut frontier = VecDeque::from([(unit.hex.clone(), Vec::<HexId>::new())]);
        let mut seen = BTreeSet::from([unit.hex.clone()]);
        while let Some((hex, path)) = frontier.pop_front() {
            if path.len() as i32 >= budget {
                continue;
            }
            // Entering a hex next to an enemy stack ends movement (the start hex excepted).
            if !path.is_empty() && in_contact(&hex) {
                continue;
            }
            for n in content.neighbors(&hex) {
                if seen.contains(&n) || enemy.contains(&n) {
                    continue;
                }
                seen.insert(n.clone());
                let mut p = path.clone();
                p.push(n.clone());
                let own_here = state
                    .units
                    .values()
                    .filter(|u| u.side == unit.side && u.hex == n)
                    .count();
                if own_here < STACK_LIMIT {
                    paths.insert(n.clone(), p.clone());
                }
                frontier.push_back((n, p));
            }
        }
        paths
    }

    /// Emit the full stack view to its own side (and the operator) and a contents-free view to
    /// the opponent only.
    fn emit_stack(&self, state: &State, hex: &HexId, side: Side, cx: &mut Cx<'_>) {
        let units: Vec<String> = state
            .stack(hex, side)
            .iter()
            .map(|u| u.id.to_string())
            .collect();
        if units.is_empty() {
            for audience in [Audience::Side(side), Audience::SideOnly(side.opponent())] {
                cx.emit(EngineEvent::new(
                    audience,
                    GameEvent::StackRemoved {
                        hex: hex.to_string(),
                        side,
                    },
                ));
            }
        } else {
            let count = units.len() as u32;
            cx.emit(EngineEvent::new(
                Audience::Side(side),
                GameEvent::StackUpdated {
                    stack: wire::Stack {
                        hex: hex.to_string(),
                        side,
                        unit_ids: units,
                        visible_count: Some(count),
                    },
                },
            ));
            cx.emit(EngineEvent::new(
                Audience::SideOnly(side.opponent()),
                GameEvent::StackUpdated {
                    stack: wire::Stack {
                        hex: hex.to_string(),
                        side,
                        unit_ids: Vec::new(),
                        visible_count: None,
                    },
                },
            ));
        }
    }

    fn move_stack_member(
        &self,
        state: &mut State,
        unit_id: &UnitId,
        path: Vec<HexId>,
        cx: &mut Cx<'_>,
    ) {
        let Some(unit) = state.units.get_mut(unit_id) else {
            return;
        };
        let from = unit.hex.clone();
        let side = unit.side;
        let Some(to) = path.last().cloned() else {
            return;
        };
        unit.hex = to.clone();
        cx.emit(EngineEvent::new(
            Audience::Side(side),
            GameEvent::UnitMoved {
                unit_id: unit_id.to_string(),
                path: std::iter::once(&from)
                    .chain(path.iter())
                    .map(|h| h.to_string())
                    .collect(),
                cp_spent: Some(path.len() as i32),
            },
        ));
        self.emit_stack(state, &from, side, cx);
        self.emit_stack(state, &to, side, cx);
    }

    fn unit_view(&self, state: &State, u: &Unit) -> wire::UnitView {
        let mut detail = BTreeMap::new();
        detail.insert("strength".into(), json!(u.strength));
        detail.insert("max_strength".into(), json!(u.max_strength));
        detail.insert("cp".into(), json!(u.cp));
        detail.insert("supplied".into(), json!(state.supplied.contains(&u.id)));
        wire::UnitView {
            id: u.id.to_string(),
            side: u.side,
            name: u.name.clone(),
            kind: u.kind.clone(),
            size: u.size.clone(),
            nationality: u.nationality.clone(),
            hex: Some(u.hex.to_string()),
            parent: None,
            detail: Some(detail),
        }
    }

    /// Apply `losses` strength losses to a set of units, strongest first; eliminate at zero.
    fn apply_losses(&self, state: &mut State, units: &[UnitId], losses: i32, cx: &mut Cx<'_>) {
        for _ in 0..losses {
            let target = units
                .iter()
                .filter_map(|id| state.units.get(id))
                .max_by_key(|u| (u.strength, std::cmp::Reverse(u.id.clone())))
                .map(|u| u.id.clone());
            let Some(id) = target else { break };
            let (side, hex, eliminated) = {
                let u = state.units.get_mut(&id).expect("unit exists");
                u.strength -= 1;
                (u.side, u.hex.clone(), u.strength <= 0)
            };
            if eliminated {
                state.units.remove(&id);
                cx.emit(EngineEvent::new(
                    Audience::Side(side),
                    GameEvent::UnitRemoved {
                        unit_id: id.to_string(),
                        reason: "eliminated".into(),
                    },
                ));
                self.emit_stack(state, &hex, side, cx);
            } else {
                let view = self.unit_view(state, &state.units[&id]);
                cx.emit(EngineEvent::new(
                    Audience::Side(side),
                    GameEvent::UnitUpdated { unit: view },
                ));
            }
        }
    }

    /// The best hex for a stack in `hex` to step away from `threats` into; None if trapped.
    fn retreat_hex(
        &self,
        content: &SandboxContent,
        state: &State,
        hex: &HexId,
        side: Side,
        threats: &[HexId],
    ) -> Option<HexId> {
        let moving = state.stack(hex, side).len();
        content
            .neighbors(hex)
            .into_iter()
            .filter(|n| state.occupant(n).is_none_or(|s| s == side))
            .filter(|n| state.stack(n, side).len() + moving <= STACK_LIMIT)
            .max_by_key(|n| {
                let d = threats
                    .iter()
                    .map(|t| content.distance(n, t))
                    .min()
                    .unwrap_or(0);
                (d, std::cmp::Reverse(n.clone()))
            })
            .filter(|n| {
                let d_new = threats.iter().map(|t| content.distance(n, t)).min();
                let d_old = threats.iter().map(|t| content.distance(hex, t)).min();
                d_new > d_old
            })
    }

    fn move_whole_stack(
        &self,
        state: &mut State,
        hex: &HexId,
        side: Side,
        to: &HexId,
        cx: &mut Cx<'_>,
    ) {
        let ids: Vec<UnitId> = state
            .stack(hex, side)
            .iter()
            .map(|u| u.id.clone())
            .collect();
        for id in ids {
            self.move_stack_member(state, &id, vec![to.clone()], cx);
        }
    }

    fn contact_pairs(
        &self,
        content: &SandboxContent,
        state: &State,
        attacker: Side,
    ) -> Vec<(HexId, HexId)> {
        let own = state.stack_hexes(attacker);
        let enemy = state.stack_hexes(attacker.opponent());
        let mut pairs = Vec::new();
        for a in &own {
            for e in &enemy {
                if content.adjacent(a, e) {
                    pairs.push((a.clone(), e.clone()));
                }
            }
        }
        pairs
    }

    fn execute_next_move(
        &self,
        content: &SandboxContent,
        state: &mut State,
        half: u8,
        cx: &mut Cx<'_>,
    ) {
        let side = state.phasing(half);
        while let Some((unit_id, dest)) = state.move_queue.pop_front() {
            let Some(unit) = state.units.get(&unit_id).cloned() else {
                continue;
            };
            let paths = self.reachable(content, state, &unit);
            let Some(path) = paths.get(&dest).cloned() else {
                cx.emit(EngineEvent::new(
                    Audience::Side(side),
                    GameEvent::Note {
                        text: format!("{} could not reach {dest}; it stays put.", unit.name),
                    },
                ));
                continue;
            };
            self.move_stack_member(state, &unit_id, path, cx);
            // Threatened enemy stacks may react, once per stack per movement step.
            let enemy = state.stack_hexes(side.opponent());
            let threatened: Vec<HexId> = content
                .neighbors(&dest)
                .into_iter()
                .filter(|n| enemy.contains(n) && !state.reacted.contains(n))
                .collect();
            for t in threatened {
                state.reacted.insert(t.clone());
                let seat = SeatId::new(side.opponent(), Role::FrontLine);
                let space = ActionSpace::new(ActionSchema::Choice {
                    options: vec![
                        ChoiceOption {
                            id: "hold".into(),
                            label: "Hold position".into(),
                            detail: None,
                        },
                        ChoiceOption {
                            id: "withdraw".into(),
                            label: "Withdraw one hex away from the enemy".into(),
                            detail: None,
                        },
                    ],
                });
                self.open(
                    state,
                    cx,
                    seat,
                    "sandbox.reaction",
                    format!("An enemy stack moved next to your stack at {t}. Hold or withdraw?"),
                    Trigger::Triggered,
                    Secrecy::Open,
                    space,
                    PendingContext::Reaction {
                        threatened: t,
                        from: dest.clone(),
                    },
                );
            }
            if !state.pending.is_empty() {
                return;
            }
        }
        self.set_step(state, Step::Air { half }, cx);
    }

    fn end_of_turn(&self, content: &SandboxContent, state: &mut State, cx: &mut Cx<'_>) {
        for obj in &content.objectives {
            let sides: BTreeSet<Side> = state
                .units
                .values()
                .filter(|u| &u.hex == obj)
                .map(|u| u.side)
                .collect();
            if sides.len() == 1 {
                let side = *sides.iter().next().expect("one side");
                state.control.insert(obj.clone(), side);
            }
        }
        for side in Side::ALL {
            let held = state.control.values().filter(|s| **s == side).count() as i32;
            *state.vp.entry(side).or_insert(0) += held;
        }
        cx.emit(EngineEvent::public(GameEvent::Note {
            text: format!(
                "End of turn {}: victory points Axis {}, Commonwealth {}.",
                state.turn,
                state.vp[&Side::Axis],
                state.vp[&Side::Commonwealth]
            ),
        }));
        let wiped = Side::ALL
            .into_iter()
            .find(|s| state.units_of(*s).next().is_none());
        if state.turn >= MAX_TURNS || wiped.is_some() {
            let (a, c) = (state.vp[&Side::Axis], state.vp[&Side::Commonwealth]);
            let summary = match wiped {
                Some(s) => format!("{} was eliminated; {} wins.", s, s.opponent()),
                None if a > c => format!("Axis wins on victory points, {a} to {c}."),
                None if c > a => format!("Commonwealth wins on victory points, {c} to {a}."),
                None => format!("Draw, {a} victory points each."),
            };
            cx.emit(EngineEvent::public(GameEvent::Note {
                text: summary.clone(),
            }));
            state.result = Some(summary);
            self.set_step(state, Step::Finished, cx);
        } else {
            state.turn += 1;
            self.set_step(state, Step::Initiative, cx);
        }
    }

    fn resolve_assaults(
        &self,
        content: &SandboxContent,
        state: &mut State,
        half: u8,
        orders: BTreeMap<HexId, HexId>,
        cx: &mut Cx<'_>,
    ) {
        let attacker = state.phasing(half);
        let defender = attacker.opponent();
        let mut by_target: BTreeMap<HexId, Vec<HexId>> = BTreeMap::new();
        for (from, target) in orders {
            by_target.entry(target).or_default().push(from);
        }
        for (target, froms) in by_target {
            let attackers: Vec<UnitId> = froms
                .iter()
                .flat_map(|h| state.stack(h, attacker))
                .map(|u| u.id.clone())
                .collect();
            let defenders: Vec<UnitId> = state
                .stack(&target, defender)
                .iter()
                .map(|u| u.id.clone())
                .collect();
            if attackers.is_empty() || defenders.is_empty() {
                continue;
            }
            let att_air = state
                .air_plans
                .get(&attacker)
                .and_then(|p| p.get(&target))
                .copied()
                .unwrap_or(0);
            let def_air = state
                .air_plans
                .get(&defender)
                .and_then(|p| p.get(&target))
                .copied()
                .unwrap_or(0);
            let attack: i64 = attackers
                .iter()
                .map(|id| i64::from(state.units[id].strength))
                .sum::<i64>()
                + att_air;
            let objective = i64::from(content.objectives.contains(&target));
            let defense: i64 = defenders
                .iter()
                .map(|id| i64::from(state.units[id].strength))
                .sum::<i64>()
                + def_air
                + objective;
            let dice = cx.rng.two_dice_reading();
            let total = i64::from(dice.sum()) + (attack - defense);
            cx.emit(EngineEvent::public(GameEvent::DiceRolled {
                purpose: format!("assault on {target}"),
                dice: vec![dice.tens.value(), dice.units.value()],
                reading: None,
                rule: None,
            }));
            let (att_loss, def_loss, retreat) = match total {
                ..=4 => (2, 0, false),
                5..=6 => (1, 0, false),
                7 => (1, 1, false),
                8..=9 => (0, 1, false),
                10..=11 => (0, 1, true),
                _ => (0, 2, true),
            };
            let mut detail = BTreeMap::new();
            detail.insert("attack".into(), json!(attack));
            detail.insert("defense".into(), json!(defense));
            detail.insert("roll".into(), json!(dice.sum()));
            detail.insert("total".into(), json!(total));
            detail.insert("attacker_losses".into(), json!(att_loss));
            detail.insert("defender_losses".into(), json!(def_loss));
            detail.insert("defender_retreats".into(), json!(retreat));
            cx.emit(EngineEvent::public(GameEvent::CombatResolved {
                hex: target.to_string(),
                summary: format!(
                    "{attacker} assaults {target}: attack {attack} vs defense {defense}, roll {} -> total {total}: attacker loses {att_loss}, defender loses {def_loss}{}.",
                    dice.sum(),
                    if retreat { " and retreats" } else { "" }
                ),
                detail: Some(detail),
            }));
            self.apply_losses(state, &attackers, att_loss, cx);
            self.apply_losses(state, &defenders, def_loss, cx);
            if retreat && !state.stack(&target, defender).is_empty() {
                match self.retreat_hex(content, state, &target, defender, &froms) {
                    Some(to) => self.move_whole_stack(state, &target, defender, &to, cx),
                    None => {
                        let still: Vec<UnitId> = state
                            .stack(&target, defender)
                            .iter()
                            .map(|u| u.id.clone())
                            .collect();
                        cx.emit(EngineEvent::public(GameEvent::Note {
                            text: format!("The stack at {target} cannot retreat and loses 1 more."),
                        }));
                        self.apply_losses(state, &still, 1, cx);
                    }
                }
            }
        }
        state.air_plans.clear();
        self.set_step(state, Step::Repair { half }, cx);
    }
}

fn parse_object(action: &Value) -> Result<serde_json::Map<String, Value>, Rejection> {
    match action {
        Value::Null => Ok(serde_json::Map::new()),
        Value::Object(o) => Ok(o.clone()),
        _ => Err(illegal("expected an object")),
    }
}

impl Ruleset for Sandbox {
    type State = State;
    type Content = SandboxContent;

    fn profile_id(&self) -> &str {
        PROFILE_ID
    }

    fn advance(
        &self,
        content: &SandboxContent,
        state: &mut State,
        cx: &mut Cx<'_>,
    ) -> Result<Progress, EngineError> {
        for _ in 0..10_000 {
            if !state.pending.is_empty() {
                return Ok(Progress::AwaitingDecisions);
            }
            match state.step {
                Step::Start => {
                    state.turn = 1;
                    self.set_step(state, Step::Initiative, cx);
                }
                Step::Initiative => {
                    state.supplied.clear();
                    let decider = if state.turn % 2 == 1 {
                        Side::Axis
                    } else {
                        Side::Commonwealth
                    };
                    let space = ActionSpace::new(ActionSchema::Choice {
                        options: vec![
                            ChoiceOption {
                                id: "first".into(),
                                label: "Our side moves first this turn".into(),
                                detail: None,
                            },
                            ChoiceOption {
                                id: "second".into(),
                                label: "Our side moves second this turn".into(),
                                detail: None,
                            },
                        ],
                    });
                    self.open(
                        state,
                        cx,
                        SeatId::new(decider, Role::Commander),
                        "sandbox.initiative",
                        format!(
                            "Turn {}: choose whether your side moves first or second.",
                            state.turn
                        ),
                        Trigger::Scheduled,
                        Secrecy::Open,
                        space,
                        PendingContext::None,
                    );
                }
                Step::Supply { half } => {
                    let side = state.phasing(half);
                    let own: Vec<UnitId> = state.units_of(side).map(|u| u.id.clone()).collect();
                    if own.is_empty() {
                        self.after_half(state, half, cx);
                        continue;
                    }
                    let points = (own.len() as u32).saturating_sub(1).max(1);
                    let space = ActionSpace::new(ActionSchema::List {
                        item: Box::new(ActionSchema::Unit { among: own }),
                        min: 0,
                        max: points,
                    });
                    self.open(
                        state,
                        cx,
                        SeatId::new(side, Role::Logistics),
                        "sandbox.supply",
                        format!("Choose up to {points} units to supply this turn (unsupplied units move at half speed). Each unit at most once."),
                        Trigger::Scheduled,
                        Secrecy::Secret,
                        space,
                        PendingContext::None,
                    );
                }
                Step::Movement { half } => {
                    let side = state.phasing(half);
                    state.reacted.clear();
                    let mut fields = Vec::new();
                    for u in state.units_of(side) {
                        let reach = self.reachable(content, state, u);
                        if !reach.is_empty() {
                            fields.push(FieldSchema {
                                name: u.id.to_string(),
                                doc: format!("destination for {} (now at {})", u.name, u.hex),
                                schema: ActionSchema::Hex {
                                    among: Some(reach.into_keys().collect()),
                                },
                                optional: true,
                            });
                        }
                    }
                    if fields.is_empty() {
                        self.set_step(state, Step::Air { half }, cx);
                        continue;
                    }
                    let space = ActionSpace::new(ActionSchema::Record { fields })
                        .with_pass("no unit moves this turn");
                    self.open(
                        state,
                        cx,
                        SeatId::new(side, Role::FrontLine),
                        "sandbox.movement",
                        "Order moves: for any of your units, pick a reachable destination hex. Units not listed stay put.".into(),
                        Trigger::Scheduled,
                        Secrecy::Open,
                        space,
                        PendingContext::None,
                    );
                }
                Step::ExecuteMoves { half } => self.execute_next_move(content, state, half, cx),
                Step::Air { half } => {
                    let attacker = state.phasing(half);
                    let pairs = self.contact_pairs(content, state, attacker);
                    if pairs.is_empty() {
                        self.set_step(state, Step::Repair { half }, cx);
                        continue;
                    }
                    let targets: BTreeSet<HexId> = pairs.iter().map(|(_, e)| e.clone()).collect();
                    for side in [attacker, attacker.opponent()] {
                        let fields = targets
                            .iter()
                            .map(|h| FieldSchema {
                                name: h.to_string(),
                                doc: if side == attacker {
                                    format!(
                                        "air points supporting attacks on the enemy stack at {h}"
                                    )
                                } else {
                                    format!("air points defending your stack at {h}")
                                },
                                schema: ActionSchema::Integer {
                                    min: 0,
                                    max: AIR_POINTS,
                                },
                                optional: true,
                            })
                            .collect();
                        self.open(
                            state,
                            cx,
                            SeatId::new(side, Role::Air),
                            "sandbox.air",
                            format!("Secretly split up to {AIR_POINTS} air points among the contact hexes (total at most {AIR_POINTS}). Both air commanders answer before either plan is revealed."),
                            Trigger::Scheduled,
                            Secrecy::SecretSimultaneous,
                            ActionSpace::new(ActionSchema::Record { fields })
                                .with_pass("no air support"),
                            PendingContext::None,
                        );
                    }
                }
                Step::Assault { half } => {
                    let attacker = state.phasing(half);
                    let pairs = self.contact_pairs(content, state, attacker);
                    if pairs.is_empty() {
                        self.set_step(state, Step::Repair { half }, cx);
                        continue;
                    }
                    let mut by_from: BTreeMap<HexId, Vec<HexId>> = BTreeMap::new();
                    for (from, to) in pairs {
                        by_from.entry(from).or_default().push(to);
                    }
                    let fields = by_from
                        .into_iter()
                        .map(|(from, targets)| FieldSchema {
                            name: from.to_string(),
                            doc: format!("enemy stack your stack at {from} assaults"),
                            schema: ActionSchema::Hex {
                                among: Some(targets),
                            },
                            optional: true,
                        })
                        .collect();
                    self.open(
                        state,
                        cx,
                        SeatId::new(attacker, Role::FrontLine),
                        "sandbox.assault",
                        "Choose assaults: each of your stacks in contact may attack one adjacent enemy stack.".into(),
                        Trigger::Scheduled,
                        Secrecy::Open,
                        ActionSpace::new(ActionSchema::Record { fields }).with_pass("no assaults"),
                        PendingContext::None,
                    );
                }
                Step::Repair { half } => {
                    let side = state.phasing(half);
                    let damaged: Vec<&Unit> = state
                        .units_of(side)
                        .filter(|u| u.strength < u.max_strength)
                        .collect();
                    if damaged.is_empty() {
                        self.after_half(state, half, cx);
                        continue;
                    }
                    let options = damaged
                        .iter()
                        .map(|u| ChoiceOption {
                            id: u.id.to_string(),
                            label: format!("Repair {} ({}/{})", u.name, u.strength, u.max_strength),
                            detail: Some(format!("at {}", u.hex)),
                        })
                        .collect();
                    self.open(
                        state,
                        cx,
                        SeatId::new(side, Role::RearArea),
                        "sandbox.repair",
                        "Choose one damaged unit to restore 1 strength.".into(),
                        Trigger::Scheduled,
                        Secrecy::Open,
                        ActionSpace::new(ActionSchema::Choice { options }).with_pass("no repair"),
                        PendingContext::None,
                    );
                }
                Step::EndOfTurn => self.end_of_turn(content, state, cx),
                Step::Finished => {
                    return Ok(Progress::Finished {
                        summary: state.result.clone().unwrap_or_default(),
                    });
                }
            }
        }
        Err(EngineError::Invariant {
            detail: "advance made no progress after 10000 steps".into(),
        })
    }

    fn respond(
        &self,
        content: &SandboxContent,
        state: &mut State,
        response: &DecisionResponse,
        cx: &mut Cx<'_>,
    ) -> Result<(), Rejection> {
        let idx = state
            .pending
            .iter()
            .position(|p| p.id == response.decision_id)
            .ok_or_else(|| Rejection::UnknownDecision {
                decision_id: response.decision_id.clone(),
            })?;
        let pending = state.pending[idx].clone();
        if pending.seat != response.seat {
            return Err(Rejection::WrongSeat {
                decision_id: pending.id,
                seat: response.seat,
            });
        }
        if response.decision_revision != 1 {
            return Err(Rejection::StaleRevision {
                expected: 1,
                got: response.decision_revision,
            });
        }
        let action = &response.action;
        if action.is_null() && pending.space.pass.is_none() {
            return Err(illegal("passing is not allowed for this decision"));
        }
        state.pending.remove(idx);
        let side = pending.seat.side;
        let summary: String;

        match pending.kind.as_str() {
            "sandbox.initiative" => {
                let choice = action
                    .as_str()
                    .ok_or_else(|| illegal("expected \"first\" or \"second\""))?;
                state.first = match choice {
                    "first" => side,
                    "second" => side.opponent(),
                    _ => return Err(illegal("expected \"first\" or \"second\"")),
                };
                summary = format!("{} moves first", state.first);
                cx.emit(EngineEvent::public(GameEvent::Note {
                    text: format!("Turn {}: {} moves first.", state.turn, state.first),
                }));
                self.set_step(state, Step::Supply { half: 0 }, cx);
            }
            "sandbox.supply" => {
                let items = match action {
                    Value::Null => Vec::new(),
                    Value::Array(a) => a.clone(),
                    _ => return Err(illegal("expected a list of unit ids")),
                };
                let ActionSchema::List { max, .. } = &pending.space.schema else {
                    return Err(Rejection::Engine(EngineError::Invariant {
                        detail: "supply space".into(),
                    }));
                };
                if items.len() > *max as usize {
                    return Err(illegal(format!("at most {max} units may be supplied")));
                }
                let mut chosen = BTreeSet::new();
                for v in items {
                    let id =
                        UnitId::new(v.as_str().ok_or_else(|| illegal("unit ids are strings"))?);
                    match state.units.get(&id) {
                        Some(u) if u.side == side => {}
                        _ => return Err(illegal(format!("{id} is not one of your units"))),
                    }
                    if !chosen.insert(id.clone()) {
                        return Err(illegal(format!("{id} listed twice")));
                    }
                }
                summary = format!("{} units supplied", chosen.len());
                state.supplied.extend(chosen);
                let Step::Supply { half } = state.step else {
                    return Err(Rejection::Engine(EngineError::Invariant {
                        detail: "supply step".into(),
                    }));
                };
                self.set_step(state, Step::Movement { half }, cx);
            }
            "sandbox.movement" => {
                let orders = parse_object(action)?;
                let Step::Movement { half } = state.step else {
                    return Err(Rejection::Engine(EngineError::Invariant {
                        detail: "movement step".into(),
                    }));
                };
                let mut queue = VecDeque::new();
                for (unit, dest) in &orders {
                    let id = UnitId::new(unit.as_str());
                    let u = match state.units.get(&id) {
                        Some(u) if u.side == side => u,
                        _ => return Err(illegal(format!("{unit} is not one of your units"))),
                    };
                    let dest = HexId::new(
                        dest.as_str()
                            .ok_or_else(|| illegal("destinations are hex ids"))?,
                    );
                    if !self.reachable(content, state, u).contains_key(&dest) {
                        return Err(illegal(format!("{unit} cannot reach {dest}")));
                    }
                    queue.push_back((id, dest));
                }
                summary = format!("{} move orders", queue.len());
                state.move_queue = queue;
                self.set_step(state, Step::ExecuteMoves { half }, cx);
            }
            "sandbox.reaction" => {
                let PendingContext::Reaction { threatened, from } = &pending.context else {
                    return Err(Rejection::Engine(EngineError::Invariant {
                        detail: "reaction context".into(),
                    }));
                };
                match action.as_str() {
                    Some("hold") => summary = format!("held at {threatened}"),
                    Some("withdraw") => {
                        match self.retreat_hex(
                            content,
                            state,
                            threatened,
                            side,
                            std::slice::from_ref(from),
                        ) {
                            Some(to) => {
                                self.move_whole_stack(state, threatened, side, &to, cx);
                                summary = format!("withdrew from {threatened} to {to}");
                            }
                            None => {
                                summary = format!("could not withdraw from {threatened}; held");
                            }
                        }
                    }
                    _ => return Err(illegal("expected \"hold\" or \"withdraw\"")),
                }
            }
            "sandbox.air" => {
                let plan = parse_object(action)?;
                let mut total = 0;
                let mut parsed = BTreeMap::new();
                let ActionSchema::Record { fields } = &pending.space.schema else {
                    return Err(Rejection::Engine(EngineError::Invariant {
                        detail: "air space".into(),
                    }));
                };
                for (hex, points) in &plan {
                    if !fields.iter().any(|f| &f.name == hex) {
                        return Err(illegal(format!("{hex} is not a contact hex")));
                    }
                    let p = points
                        .as_i64()
                        .filter(|p| (0..=AIR_POINTS).contains(p))
                        .ok_or_else(|| illegal("air points are integers 0..=3"))?;
                    total += p;
                    if p > 0 {
                        parsed.insert(HexId::new(hex.as_str()), p);
                    }
                }
                if total > AIR_POINTS {
                    return Err(illegal(format!("at most {AIR_POINTS} air points in total")));
                }
                summary = format!("{total} air points allocated");
                state.air_plans.insert(side, parsed);
                // Reveal only when both plans are in.
                if !state.pending.iter().any(|p| p.kind == "sandbox.air") {
                    for s in Side::ALL {
                        let plan = state.air_plans.get(&s).cloned().unwrap_or_default();
                        let text = if plan.is_empty() {
                            format!("{s} air: no sorties.")
                        } else {
                            let parts: Vec<String> =
                                plan.iter().map(|(h, p)| format!("{p} at {h}")).collect();
                            format!("{s} air: {}.", parts.join(", "))
                        };
                        cx.emit(EngineEvent::public(GameEvent::Note { text }));
                    }
                    let Step::Air { half } = state.step else {
                        return Err(Rejection::Engine(EngineError::Invariant {
                            detail: "air step".into(),
                        }));
                    };
                    self.set_step(state, Step::Assault { half }, cx);
                }
            }
            "sandbox.assault" => {
                let orders = parse_object(action)?;
                let ActionSchema::Record { fields } = &pending.space.schema else {
                    return Err(Rejection::Engine(EngineError::Invariant {
                        detail: "assault space".into(),
                    }));
                };
                let mut parsed = BTreeMap::new();
                for (from, target) in &orders {
                    let field = fields.iter().find(|f| &f.name == from).ok_or_else(|| {
                        illegal(format!("no stack of yours in contact at {from}"))
                    })?;
                    let target = HexId::new(
                        target
                            .as_str()
                            .ok_or_else(|| illegal("targets are hex ids"))?,
                    );
                    match &field.schema {
                        ActionSchema::Hex { among: Some(t) } if t.contains(&target) => {}
                        _ => return Err(illegal(format!("{from} cannot assault {target}"))),
                    }
                    parsed.insert(HexId::new(from.as_str()), target);
                }
                summary = format!("{} assaults", parsed.len());
                let Step::Assault { half } = state.step else {
                    return Err(Rejection::Engine(EngineError::Invariant {
                        detail: "assault step".into(),
                    }));
                };
                self.resolve_assaults(content, state, half, parsed, cx);
            }
            "sandbox.repair" => {
                let Step::Repair { half } = state.step else {
                    return Err(Rejection::Engine(EngineError::Invariant {
                        detail: "repair step".into(),
                    }));
                };
                match action {
                    Value::Null => summary = "no repair".into(),
                    Value::String(id) => {
                        let ActionSchema::Choice { options } = &pending.space.schema else {
                            return Err(Rejection::Engine(EngineError::Invariant {
                                detail: "repair space".into(),
                            }));
                        };
                        if !options.iter().any(|o| &o.id == id) {
                            return Err(illegal(format!("{id} cannot be repaired now")));
                        }
                        let uid = UnitId::new(id.as_str());
                        let view = {
                            let u = state
                                .units
                                .get_mut(&uid)
                                .ok_or_else(|| illegal("unknown unit"))?;
                            u.strength = (u.strength + 1).min(u.max_strength);
                            u.clone()
                        };
                        let view = self.unit_view(state, &view);
                        cx.emit(EngineEvent::new(
                            Audience::Side(side),
                            GameEvent::UnitUpdated { unit: view },
                        ));
                        summary = format!("repaired {id}");
                    }
                    _ => return Err(illegal("expected a unit id or null")),
                }
                self.after_half(state, half, cx);
            }
            other => {
                return Err(Rejection::Engine(EngineError::Invariant {
                    detail: format!("unknown decision kind {other}"),
                }));
            }
        }

        cx.emit(EngineEvent::new(
            Audience::Seat(pending.seat),
            GameEvent::DecisionResolved {
                decision_id: pending.id.to_string(),
                seat: pending.seat.to_string(),
                summary,
            },
        ));
        Ok(())
    }

    fn pending(&self, _content: &SandboxContent, state: &State) -> Vec<DecisionRequest> {
        state
            .pending
            .iter()
            .map(|p| DecisionRequest {
                id: p.id.clone(),
                seat: p.seat,
                kind: p.kind.clone(),
                revision: 1,
                clock: state.clock(),
                summary: p.summary.clone(),
                rules: Vec::new(),
                trigger: p.trigger,
                secrecy: p.secrecy,
                space: p.space.clone(),
            })
            .collect()
    }

    fn observe(&self, content: &SandboxContent, state: &State, perspective: Perspective) -> Value {
        let sees_side = |s: Side| perspective.can_see(&Audience::Side(s));
        let units: Vec<Value> = state
            .units
            .values()
            .filter(|u| sees_side(u.side))
            .map(|u| {
                json!({
                    "id": u.id, "name": u.name, "side": u.side, "kind": u.kind, "hex": u.hex,
                    "strength": u.strength, "max_strength": u.max_strength, "cp": u.cp,
                    "supplied_this_turn": state.supplied.contains(&u.id),
                })
            })
            .collect();
        let hidden_stacks: Vec<Value> = Side::ALL
            .into_iter()
            .filter(|s| !sees_side(*s))
            .flat_map(|s| {
                state
                    .stack_hexes(s)
                    .into_iter()
                    .map(move |h| json!({ "hex": h, "side": s }))
            })
            .collect();
        let objectives: Vec<Value> = content
            .objectives
            .iter()
            .map(|h| json!({ "hex": h, "held_by": state.control.get(h) }))
            .collect();
        let pending: Vec<Value> = state
            .pending
            .iter()
            .filter(|p| perspective.can_see(&Audience::Seat(p.seat)))
            .map(|p| json!({ "id": p.id, "seat": p.seat, "kind": p.kind, "summary": p.summary }))
            .collect();
        json!({
            "ruleset": PROFILE_ID,
            "rules": RULES_SUMMARY,
            "perspective": perspective.to_string(),
            "clock": state.wire_clock(),
            "turn": state.turn, "max_turns": MAX_TURNS,
            "victory_points": state.vp,
            "objectives": objectives,
            "visible_units": units,
            "enemy_stacks_contents_hidden": hidden_stacks,
            "pending_decisions": pending,
            "result": state.result,
        })
    }

    fn view(
        &self,
        content: &SandboxContent,
        state: &State,
        perspective: Perspective,
    ) -> wire::ViewState {
        let sees_side = |s: Side| perspective.can_see(&Audience::Side(s));
        let mut stacks = Vec::new();
        for side in Side::ALL {
            for hex in state.stack_hexes(side) {
                let ids: Vec<String> = state
                    .stack(&hex, side)
                    .iter()
                    .map(|u| u.id.to_string())
                    .collect();
                let visible = sees_side(side);
                stacks.push(wire::Stack {
                    hex: hex.to_string(),
                    side,
                    visible_count: visible.then_some(ids.len() as u32),
                    unit_ids: if visible { ids } else { Vec::new() },
                });
            }
        }
        let units = state
            .units
            .values()
            .filter(|u| sees_side(u.side))
            .map(|u| (u.id.to_string(), self.unit_view(state, u)))
            .collect();
        let markers = content
            .objectives
            .iter()
            .map(|h| wire::Marker {
                id: format!("objective-{h}"),
                kind: "objective".into(),
                hex: h.to_string(),
                side: state.control.get(h).copied(),
                label: Some("Objective".into()),
            })
            .collect();
        let pending = state
            .pending
            .iter()
            .filter(|p| perspective.can_see(&Audience::Seat(p.seat)))
            .map(|p| wire::PendingDecision {
                id: p.id.to_string(),
                seat: p.seat.to_string(),
                kind: p.kind.clone(),
                summary: p.summary.clone(),
                opened_seq: 0,
                rules: Vec::new(),
            })
            .collect();
        wire::ViewState {
            clock: state.wire_clock(),
            stacks,
            units,
            markers,
            pending,
        }
    }

    fn inspect(
        &self,
        content: &SandboxContent,
        state: &State,
        perspective: Perspective,
        target: &str,
    ) -> Result<Value, Rejection> {
        let sees_side = |s: Side| perspective.can_see(&Audience::Side(s));
        let hex = HexId::new(target);
        if content.axial(&hex).is_some() {
            let stacks: Vec<Value> = Side::ALL
                .into_iter()
                .filter_map(|s| {
                    let units = state.stack(&hex, s);
                    if units.is_empty() {
                        None
                    } else if sees_side(s) {
                        Some(json!({ "side": s, "units": units.iter().map(|u| json!({"id": u.id, "name": u.name, "strength": u.strength})).collect::<Vec<_>>() }))
                    } else {
                        Some(json!({ "side": s, "units": "hidden" }))
                    }
                })
                .collect();
            return Ok(json!({
                "hex": hex,
                "objective": content.objectives.contains(&hex),
                "held_by": state.control.get(&hex),
                "terrain": "clear (sandbox: all hexes cost 1)",
                "stacks": stacks,
                "neighbors": content.neighbors(&hex),
            }));
        }
        match state.units.get(&UnitId::new(target)) {
            Some(u) if sees_side(u.side) => Ok(json!({
                "id": u.id, "name": u.name, "side": u.side, "kind": u.kind, "size": u.size,
                "hex": u.hex, "strength": u.strength, "max_strength": u.max_strength, "cp": u.cp,
                "supplied_this_turn": state.supplied.contains(&u.id),
                "reachable_now": self.reachable(content, state, u).into_keys().collect::<Vec<_>>(),
            })),
            _ => Err(illegal(format!("nothing visible called {target}"))),
        }
    }
}

pub mod baseline;

#[cfg(test)]
mod tests;
