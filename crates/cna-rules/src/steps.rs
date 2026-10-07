//! Step procedures: what happens when the cursor enters a step of the sequence of play, and how
//! the decisions a step opens are answered.
//!
//! Adding a step procedure: add an arm to [`Cna::enter_step`] for its anchor (and, if it opens
//! decisions, to [`Cna::respond_to`] for each decision kind), put the procedure in the module of
//! its rules area, and cite every case it implements on a `/// Cases:` line. A step that opens
//! decisions stays current until all of them are answered; keep one open (with a "done" option)
//! for as long as the seat may keep acting.

use cna_core::decision::{ActionSchema, ActionSpace, ChoiceOption, Secrecy, Trigger};
use cna_core::engine::{Cx, EngineError, Rejection};
use cna_core::event::EngineEvent;
use cna_core::ids::{DecisionId, SeatId};
use cna_core::visibility::Audience;
use cna_protocol::{self as wire, GameEvent, Role, Side};
use serde_json::Value;

use crate::Cna;
use crate::content::{CnaContent, InitiativeRatings};
use crate::state::{Location, Pending, State};

pub(crate) fn illegal(message: impl Into<String>) -> Rejection {
    Rejection::Illegal {
        message: message.into(),
    }
}

impl Cna {
    /// Run the entry procedure of the cursor's current step.
    /// Let the current step resolve what its answers closed, once nothing is pending (see
    /// docs/engine.md section 3 rule 7: answering is not adjudicating). Dispatches on the anchor like
    /// `enter_step`; a procedure may open further decisions here, and is called again each time
    /// its step has nothing pending, so it must track what it has already resolved.
    pub(crate) fn finish_step(
        &self,
        content: &CnaContent,
        state: &mut State,
        cx: &mut Cx<'_>,
    ) -> Result<(), EngineError> {
        match state.cursor.anchor() {
            "opstage.organization.water_distribution" => {
                crate::logistics::batches::finish_water(content, state, cx, self.strict)
            }
            "opstage.movement_and_combat.movement"
            | "opstage.movement_and_combat.combat.retreat_before_assault" => {
                crate::land::reaction::finish_adjudication(state)
            }
            "opstage.movement_and_combat.combat.barrage" => {
                crate::land::combat::barrage::finish(content, state, cx, self.strict)
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn enter_step(
        &self,
        content: &CnaContent,
        state: &mut State,
        cx: &mut Cx<'_>,
    ) -> Result<(), EngineError> {
        let anchor = state.cursor.anchor();
        match anchor {
            "setup" => {
                crate::logistics::dump_markers::initialize(state, cx)?;
                crate::setup::enter(content, state, cx, self.strict)
            }
            "initiative" => determine_initiative(content, state, cx),
            "opstage.initiative_declaration" => {
                open_initiative_declaration(state, cx);
                Ok(())
            }
            "opstage.movement_and_combat.movement" => {
                crate::land::movement::enter(content, state, self.strict, cx)
            }
            "naval_convoy.schedule" => {
                crate::logistics::convoys::schedule(content, state, self.strict, cx)
            }
            "opstage.convoy_arrival" => crate::logistics::convoys::arrive(content, state, cx),
            "opstage.movement_and_combat.combat.barrage" => {
                crate::land::combat::barrage::enter(content, state, cx, self.strict)
            }
            "opstage.movement_and_combat.combat.position" => {
                crate::land::combat::enter_positions(content, state, cx)
            }
            "opstage.movement_and_combat.reserve_release" => {
                crate::land::reserve::enter_release(content, state, cx)
            }
            "opstage.reserve_designation" => {
                crate::land::reserve::enter_designation(content, state, cx)
            }
            "logistics.stores_expenditure" => {
                crate::logistics::batches::enter_stores(content, state, cx)
            }
            "opstage.organization.water_distribution" => {
                crate::logistics::batches::enter_water(content, state, cx, self.strict)
            }
            "opstage.organization.supply_distribution" => {
                crate::logistics::batches::enter_distribution(content, state, cx)
            }
            "opstage.organization.attrition" => {
                crate::logistics::attrition::enter(content, state, cx)
            }
            "opstage.weather" => crate::logistics::weather::determine(content, state, cx),
            "opstage.movement_and_combat.combat.close_assault" => {
                self.unimplemented(content, anchor)?;
                crate::land::combat::finish_pins(content, state, cx);
                Ok(())
            }
            "end_of_game" => self.end_of_game(content, state, cx),
            _ => self.unimplemented(content, anchor),
        }
    }

    /// Apply an answer to one of the current step's decisions. `pending` has already been
    /// checked against the response (id, seat, revision) and removed from the pending list.
    pub(crate) fn respond_to(
        &self,
        content: &CnaContent,
        state: &mut State,
        pending: &Pending,
        action: &Value,
        cx: &mut Cx<'_>,
    ) -> Result<String, Rejection> {
        match pending.kind.as_str() {
            crate::logistics::batches::STORES
            | crate::logistics::batches::WATER
            | crate::logistics::batches::WELL_ALLOCATION
            | crate::logistics::batches::DISTRIBUTION => {
                crate::logistics::batches::answer(content, state, pending, action, cx, self.strict)
            }

            crate::land::combat::barrage::DECLARE => {
                crate::land::combat::barrage::declare(content, state, pending, action, cx)
            }
            crate::land::combat::barrage::PLOT => {
                crate::land::combat::barrage::answer(content, state, pending, action, cx)
            }
            crate::land::combat::barrage::LOSSES => {
                crate::land::combat::barrage::losses(content, state, pending, action, cx)
            }
            crate::land::combat::POSITION_KIND => {
                crate::land::combat::answer_positions(content, state, pending, action, cx)
            }
            crate::land::reaction::KIND => {
                crate::land::reaction::answer(content, state, pending, action, self.strict, cx)
            }
            crate::land::reaction::CONTINUE => crate::land::reaction::answer_continuation(
                content,
                state,
                pending,
                action,
                self.strict,
                cx,
            ),
            crate::land::movement::KIND => {
                crate::land::movement::answer(content, state, pending, action, self.strict, cx)
            }
            crate::setup::KIND_UNIT
            | crate::setup::KIND_DUMP
            | crate::setup::KIND_TRUCKS
            | crate::setup::KIND_POOL
            | crate::setup::KIND_PRELOAD => {
                crate::setup::answer(content, state, pending, action, cx, self.strict)
            }
            crate::land::reserve::DESIGNATE | crate::land::reserve::RELEASE => {
                crate::land::reserve::answer(content, state, pending, action, cx)
            }
            crate::land::cycles::KIND => crate::land::cycles::answer(state, pending, action),
            KIND_INITIATIVE_DECLARATION => {
                answer_initiative_declaration(state, pending, action, cx)
            }
            kind if kind == crate::logistics::stores::KIND
                || kind.starts_with(crate::logistics::stores::ISSUE_PREFIX) =>
            {
                crate::logistics::stores::answer(content, state, pending, action, cx)
            }
            kind if kind == crate::logistics::water::KIND
                || kind.starts_with(crate::logistics::water::ISSUE_PREFIX) =>
            {
                crate::logistics::water::answer(content, state, pending, action, cx, self.strict)
            }
            kind if kind.starts_with(crate::logistics::wells::PREFIX)
                || kind.starts_with(crate::logistics::wells::REQUEST_PREFIX)
                || kind.starts_with(crate::logistics::wells::ALLOCATE_PREFIX) =>
            {
                crate::logistics::wells::answer(content, state, pending, action, cx, self.strict)
            }
            kind if kind == crate::logistics::distribution::KIND
                || kind.starts_with(crate::logistics::distribution::PREFIX) =>
            {
                crate::logistics::distribution::answer(content, state, pending, action, cx)
            }
            kind if kind.starts_with(crate::logistics::convoys::PREFIX) => {
                crate::logistics::convoys::answer(content, state, pending, action, cx)
            }
            crate::logistics::attrition::KIND => {
                crate::logistics::attrition::answer(content, state, pending, action, cx)
            }
            other => Err(Rejection::Engine(EngineError::Invariant {
                detail: format!("no handler for decision kind {other}"),
            })),
        }
    }

    /// A step with no procedure yet. Under the strict profile a step that has applicable
    /// procedural cases stops the game; under the development profile it is skipped.
    fn unimplemented(&self, content: &CnaContent, anchor: &str) -> Result<(), EngineError> {
        if !self.strict {
            return Ok(());
        }
        match content
            .registry
            .procedural_at(anchor, content.scenario_key())
            .next()
        {
            Some(case) => Err(EngineError::Unsupported {
                case: case.citation(),
                detail: format!("the step {anchor} is not implemented yet"),
            }),
            None => Ok(()),
        }
    }

    /// Victory determination. Not implemented yet: the game ends with that stated.
    fn end_of_game(
        &self,
        content: &CnaContent,
        state: &mut State,
        cx: &mut Cx<'_>,
    ) -> Result<(), EngineError> {
        self.unimplemented(content, "end_of_game")?;
        let end = content.scenario.meta.end;
        let summary = format!(
            "{} ended at the close of Game-Turn {}, OpStage {}. Victory determination is not \
             implemented yet, so no winner is declared.",
            content.scenario.meta.name, end.gt, end.opstage
        );
        cx.emit(EngineEvent::public(GameEvent::Note {
            text: summary.clone(),
        }));
        state.result = Some(summary);
        Ok(())
    }
}

/// Open a decision and announce it to its seat.
#[allow(clippy::too_many_arguments)] // mirrors the fields of a decision request
pub(crate) fn open(
    state: &mut State,
    cx: &mut Cx<'_>,
    seat: SeatId,
    kind: &str,
    summary: String,
    rules: &[&str],
    trigger: Trigger,
    secrecy: Secrecy,
    space: ActionSpace,
) {
    let n = state.decisions.opened.entry(seat).or_default();
    *n += 1;
    let id = DecisionId::new(format!("{seat}-{n}"));
    cx.emit(EngineEvent::new(
        Audience::Seat(seat),
        GameEvent::DecisionOpened {
            decision: wire::PendingDecision {
                id: id.to_string(),
                seat: seat.to_string(),
                kind: kind.to_owned(),
                summary: summary.clone(),
                opened_seq: 0,
                rules: rules.iter().map(|r| (*r).to_owned()).collect(),
                space: Some(space.to_json_schema()),
            },
        },
    ));
    state.decisions.pending.push(Pending {
        id,
        seat,
        kind: kind.to_owned(),
        summary,
        rules: rules.iter().map(|r| (*r).to_owned()).collect(),
        trigger,
        secrecy,
        space,
        revision: 1,
    });
}

// ---------------------------------------------------------------------------------------------
// Initiative (land:7)
// ---------------------------------------------------------------------------------------------

const KIND_INITIATIVE_DECLARATION: &str = "cna.initiative_declaration";

/// Stage I: who holds the Initiative this game-turn. The scenario fixes the first game-turn;
/// afterwards each side rolls one die plus its rating, higher wins, ties are rolled again.
/// Cases: land:7.12, land:7.13, land:7.14, land:7.15
fn determine_initiative(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let gt = state.cursor.game_turn;
    if gt == content.scenario.meta.start.gt
        && let Some(side) = content.scenario.initiative.gt1
    {
        state.turn.initiative = Some(side);
        cx.emit(EngineEvent::public(GameEvent::Note {
            text: format!(
                "Game-Turn {gt}: {} holds the Initiative, as the scenario sets (land:7.15).",
                side_name(side)
            ),
        }));
        return Ok(());
    }
    let situation = axis_initiative_situation(content, state);
    let rating = |side: Side| {
        content
            .initiative_ratings
            .rating(side, gt, situation)
            .ok_or_else(|| EngineError::Invariant {
                detail: format!("no land:7.2 initiative rating for {side:?} on Game-Turn {gt}"),
            })
    };
    let (axis_rating, cw_rating) = (rating(Side::Axis)?, rating(Side::Commonwealth)?);
    let winner = loop {
        let axis = i32::from(cx.rng.d6().value());
        let cw = i32::from(cx.rng.d6().value());
        for (side, die) in [(Side::Axis, axis), (Side::Commonwealth, cw)] {
            cx.emit(EngineEvent::public(GameEvent::DiceRolled {
                purpose: format!("initiative ({})", side_name(side)),
                dice: vec![u8::try_from(die).unwrap_or(0)],
                reading: None,
                rule: Some("land:7.14".into()),
            }));
        }
        let (a, c) = (axis + axis_rating, cw + cw_rating);
        if a != c {
            break if a > c {
                Side::Axis
            } else {
                Side::Commonwealth
            };
        }
    };
    state.turn.initiative = Some(winner);
    cx.emit(EngineEvent::public(GameEvent::Note {
        text: format!(
            "Game-Turn {gt}: {} wins the Initiative (ratings: Axis {axis_rating}, Commonwealth \
             {cw_rating}).",
            side_name(winner)
        ),
    }));
    Ok(())
}

/// Which Axis row of the Initiative Ratings Chart applies: it depends on German forces on the
/// game maps (the Tripoli/Tunisia boxes do not count).
/// Cases: land:7.13
fn axis_initiative_situation(content: &CnaContent, state: &State) -> &'static str {
    let german_on_map = state.units_of(Side::Axis).any(|u| {
        matches!(u.location, Location::Hex { .. })
            && content
                .units
                .units
                .get(&u.id)
                .is_some_and(|oa| oa.nationality == "german")
    });
    if german_on_map {
        "german_land_combat_units_without_rommel_counter_on_game_maps"
    } else {
        InitiativeRatings::AXIS_NO_GERMANS
    }
}

/// Phase A of each OpStage: the Initiative holder chooses to be Player A or Player B.
/// Cases: land:7.11, land:7.16
fn open_initiative_declaration(state: &mut State, cx: &mut Cx<'_>) {
    let Some(holder) = state.turn.initiative else {
        return;
    };
    let op = state.cursor.op_stage.unwrap_or(1);
    let space = ActionSpace::new(ActionSchema::Choice {
        options: vec![
            ChoiceOption {
                id: "player_a".into(),
                label: "Move first (Player A)".into(),
                detail: Some("Your side acts first in phases G-M of this OpStage.".into()),
            },
            ChoiceOption {
                id: "player_b".into(),
                label: "Move second (Player B)".into(),
                detail: Some("The enemy acts first in phases G-M of this OpStage.".into()),
            },
        ],
    });
    open(
        state,
        cx,
        SeatId::new(holder, Role::Commander),
        KIND_INITIATIVE_DECLARATION,
        format!(
            "Game-Turn {}, OpStage {op}: you hold the Initiative. Choose whether your side is \
             Player A (moves first) or Player B (moves second) in this OpStage.",
            state.cursor.game_turn
        ),
        &["land:7.11", "land:7.16"],
        Trigger::Scheduled,
        Secrecy::Open,
        space,
    );
}

/// Cases: land:7.11
fn answer_initiative_declaration(
    state: &mut State,
    pending: &Pending,
    action: &Value,
    cx: &mut Cx<'_>,
) -> Result<String, Rejection> {
    let holder = pending.seat.side;
    let player_a = match action.as_str() {
        Some("player_a") => holder,
        Some("player_b") => holder.opponent(),
        _ => return Err(illegal("expected \"player_a\" or \"player_b\"")),
    };
    state.turn.player_a = Some(player_a);
    let text = format!(
        "Game-Turn {}, OpStage {}: {} is Player A and moves first.",
        state.cursor.game_turn,
        state.cursor.op_stage.unwrap_or(1),
        side_name(player_a)
    );
    cx.emit(EngineEvent::public(GameEvent::Note { text: text.clone() }));
    Ok(text)
}

pub(crate) fn side_name(side: Side) -> &'static str {
    match side {
        Side::Axis => "the Axis",
        Side::Commonwealth => "the Commonwealth",
    }
}
