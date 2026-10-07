//! The Campaign for North Africa ruleset: state, sequence of play and rule procedures.
//!
//! - [`content`]: everything a campaign reads (map, units, scenario, tables, registry).
//! - [`state`]: the authoritative world state and its construction from a scenario.
//! - [`seq`]: the sequence of play as an explicit state machine of timing anchors.
//! - `steps`: step procedures and decision handlers, citing their cases (`/// Cases:`).
//! - `view`: Limited Intelligence filtering for the board, `observe` and `inspect`.
//!
//! Two rules profiles: [`PROFILE_DEV`] skips steps that are not implemented yet, so a campaign
//! can be watched end to end while the rules are filled in; [`PROFILE_FULL`] stops the game with
//! `EngineError::Unsupported` at the first applicable case it cannot play. How to add rules:
//! `docs/engine.md`.

pub mod baseline;
pub mod content;
pub mod land;
pub mod logistics;
pub mod ownership;
pub mod seq;
pub mod setup;
pub mod state;
mod steps;
mod view;

#[cfg(test)]
mod tests;

use cna_core::decision::{DecisionRequest, DecisionResponse};
use cna_core::engine::{Cx, EngineError, Progress, Rejection, Ruleset};
use cna_core::event::EngineEvent;
use cna_core::visibility::{Audience, Perspective};
use cna_protocol::GameEvent;
use serde_json::Value;

pub use content::CnaContent;
pub use state::State;

/// Unimplemented steps are skipped.
pub const PROFILE_DEV: &str = "cna-2021-dev";
/// Unimplemented applicable cases stop the game.
pub const PROFILE_FULL: &str = "cna-2021-full";

/// The CNA ruleset under one of its profiles.
#[derive(Debug, Clone, Copy, Default)]
pub struct Cna {
    /// `true` for [`PROFILE_FULL`].
    pub strict: bool,
}

impl Cna {
    pub fn dev() -> Self {
        Cna { strict: false }
    }

    pub fn full() -> Self {
        Cna { strict: true }
    }
}

impl Ruleset for Cna {
    type State = State;
    type Content = CnaContent;

    fn profile_id(&self) -> &str {
        if self.strict {
            PROFILE_FULL
        } else {
            PROFILE_DEV
        }
    }

    fn advance(
        &self,
        content: &CnaContent,
        state: &mut State,
        cx: &mut Cx<'_>,
    ) -> Result<Progress, EngineError> {
        let moved = state.land.movement.moved.clone();
        let progress = self.run_steps(content, state, cx);
        view::sync_moved_flags(content, state, &moved, cx);
        progress
    }

    fn respond(
        &self,
        content: &CnaContent,
        state: &mut State,
        response: &DecisionResponse,
        cx: &mut Cx<'_>,
    ) -> Result<(), Rejection> {
        let moved = state.land.movement.moved.clone();
        let outcome = self.resolve(content, state, response, cx);
        view::sync_moved_flags(content, state, &moved, cx);
        outcome
    }

    fn pending(&self, _content: &CnaContent, state: &State) -> Vec<DecisionRequest> {
        let clock = view::core_clock(state);
        state
            .decisions
            .pending
            .iter()
            .map(|p| DecisionRequest {
                id: p.id.clone(),
                seat: p.seat,
                kind: p.kind.clone(),
                revision: p.revision,
                clock: clock.clone(),
                summary: p.summary.clone(),
                rules: p.rules.clone(),
                trigger: p.trigger,
                secrecy: p.secrecy,
                space: p.space.clone(),
            })
            .collect()
    }

    fn observe(&self, content: &CnaContent, state: &State, perspective: Perspective) -> Value {
        view::observe(content, state, perspective)
    }

    fn view(
        &self,
        content: &CnaContent,
        state: &State,
        perspective: Perspective,
    ) -> cna_protocol::ViewState {
        view::view(content, state, perspective)
    }

    fn inspect(
        &self,
        content: &CnaContent,
        state: &State,
        perspective: Perspective,
        target: &str,
    ) -> Result<Value, Rejection> {
        view::inspect(content, state, perspective, target, self.strict)
    }
}

impl Cna {
    /// Run automatic steps until a decision is pending or the campaign ends.
    fn run_steps(
        &self,
        content: &CnaContent,
        state: &mut State,
        cx: &mut Cx<'_>,
    ) -> Result<Progress, EngineError> {
        for _ in 0..100_000 {
            if state.cursor.is_finished() {
                return Ok(Progress::Finished {
                    summary: state.result.clone().unwrap_or_default(),
                });
            }
            if !state.decisions.pending.is_empty() {
                return Ok(Progress::AwaitingDecisions);
            }
            if !state.cursor.entered {
                state.cursor.entered = true;
                cx.emit(EngineEvent::public(GameEvent::PhaseChanged {
                    clock: view::wire_clock(content, state),
                }));
                self.enter_step(content, state, cx)?;
                continue;
            }
            // Entered and nothing left to decide: the step is complete.
            let new_op_stage = state.cursor.block == seq::Block::PlayerHalf
                && state.cursor.index + 1 == seq::PLAYER_HALF.len()
                && state.cursor.half == Some(seq::Half::B);
            if state.cursor.advance(&content.bounds) {
                state.turn.initiative = None;
            }
            if new_op_stage {
                state.turn.player_a = None;
                land::capability::finish_opstage(state);
            }
        }
        Err(EngineError::Invariant {
            detail: "advance made no progress after 100000 steps".into(),
        })
    }

    /// Apply one decision response.
    fn resolve(
        &self,
        content: &CnaContent,
        state: &mut State,
        response: &DecisionResponse,
        cx: &mut Cx<'_>,
    ) -> Result<(), Rejection> {
        let idx = state
            .decisions
            .pending
            .iter()
            .position(|p| p.id == response.decision_id)
            .ok_or_else(|| Rejection::UnknownDecision {
                decision_id: response.decision_id.clone(),
            })?;
        let pending = state.decisions.pending[idx].clone();
        if pending.seat != response.seat {
            return Err(Rejection::WrongSeat {
                decision_id: pending.id,
                seat: response.seat,
            });
        }
        if response.decision_revision != pending.revision {
            return Err(Rejection::StaleRevision {
                expected: pending.revision,
                got: response.decision_revision,
            });
        }
        if response.action.is_null() && pending.space.pass.is_none() {
            return Err(steps::illegal("passing is not allowed for this decision"));
        }
        state.decisions.pending.remove(idx);
        let summary = self.respond_to(content, state, &pending, &response.action, cx)?;
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
}
