//! The pure engine: rulesets and command evaluation.
//!
//! ```text
//! evaluate(game, command) -> Ok(Transition { game', events })   // accepted
//!                         -> Err(Rejection)                     // nothing changed
//! ```
//!
//! A [`Ruleset`] owns everything game-specific: its state type, its sequence of play, legality,
//! and projections. This module owns what every ruleset shares: the RNG lives in the game and
//! only advances inside an accepted transition; a rejected command leaves the game untouched;
//! events are collected per transition in emission order.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::decision::{DecisionRequest, DecisionResponse};
use crate::dice::{CampaignRng, RngState};
use crate::event::EngineEvent;
use crate::ids::{DecisionId, SeatId};
use crate::visibility::Perspective;

/// A game system implementation: CNA under a rules profile, or a test ruleset.
pub trait Ruleset {
    /// The ruleset's dynamic world state. Cloned per transition so rejected commands cannot leave
    /// a trace; keep it plain data.
    type State: Clone + std::fmt::Debug + Serialize + DeserializeOwned;
    /// Immutable content the ruleset reads (map, tables, units, scenario).
    type Content;

    /// The rules-profile id this ruleset implements, pinned by campaigns.
    fn profile_id(&self) -> &str;

    /// Run automatic steps until at least one decision is pending or the game ends.
    fn advance(
        &self,
        content: &Self::Content,
        state: &mut Self::State,
        cx: &mut Cx<'_>,
    ) -> Result<Progress, EngineError>;

    /// Apply one seat's answer to one of its pending decisions. Must not change anything if it
    /// returns an error.
    fn respond(
        &self,
        content: &Self::Content,
        state: &mut Self::State,
        response: &DecisionResponse,
        cx: &mut Cx<'_>,
    ) -> Result<(), Rejection>;

    /// Every currently pending decision, each addressed to its owning seat.
    fn pending(&self, content: &Self::Content, state: &Self::State) -> Vec<DecisionRequest>;

    /// A seat's or side's (or the operator's) structured view of the situation: what an AI seat
    /// gets from its `observe` tool, filtered for `perspective`.
    fn observe(
        &self,
        content: &Self::Content,
        state: &Self::State,
        perspective: Perspective,
    ) -> Value;

    /// The board's view for `perspective` (stacks, units, markers, clock).
    fn view(
        &self,
        content: &Self::Content,
        state: &Self::State,
        perspective: Perspective,
    ) -> cna_protocol::ViewState;

    /// The board views of several perspectives of one state, in order. Rulesets override it to
    /// share work between perspectives; the result must equal calling `view` for each.
    fn views(
        &self,
        content: &Self::Content,
        state: &Self::State,
        perspectives: &[Perspective],
    ) -> Vec<cna_protocol::ViewState> {
        perspectives
            .iter()
            .map(|p| self.view(content, state, *p))
            .collect()
    }

    /// Authorized detail about one thing — a hex, a unit, a dump, an airfield — identified by
    /// `target` (a hex id, unit id, or another id the ruleset documents), filtered for
    /// `perspective`. Backs the AI seats' `inspect` tool. An unknown or unauthorized target is a
    /// `Rejection::Illegal` whose message reveals nothing either way.
    fn inspect(
        &self,
        content: &Self::Content,
        state: &Self::State,
        perspective: Perspective,
        target: &str,
    ) -> Result<Value, Rejection> {
        let _ = (content, state, perspective, target);
        Err(Rejection::Illegal {
            message: "this ruleset does not support inspect".into(),
        })
    }
}

/// What a ruleset may touch while running a transition.
pub struct Cx<'a> {
    pub rng: &'a mut CampaignRng,
    pub events: &'a mut Vec<EngineEvent>,
}

impl Cx<'_> {
    pub fn emit(&mut self, event: EngineEvent) {
        self.events.push(event);
    }
}

/// Where a game stands after automatic steps have run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Progress {
    /// At least one decision is pending.
    AwaitingDecisions,
    /// The game is over.
    Finished { summary: String },
}

/// Why a ruleset cannot continue at all. The runner stops the campaign; it never guesses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum EngineError {
    /// The situation needs a rule the profile does not implement (yet).
    Unsupported { case: String, detail: String },
    /// An internal invariant was violated: a bug, never a game situation.
    Invariant { detail: String },
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Unsupported { case, detail } => {
                write!(f, "unsupported rule case {case}: {detail}")
            }
            EngineError::Invariant { detail } => write!(f, "engine invariant violated: {detail}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// Why an answer was refused. Nothing changed and no randomness was consumed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Rejection {
    /// No such pending decision (it may have been resolved already).
    UnknownDecision { decision_id: DecisionId },
    /// The decision belongs to another seat.
    WrongSeat {
        decision_id: DecisionId,
        seat: SeatId,
    },
    /// The answer was for an older revision of the decision.
    StaleRevision { expected: u32, got: u32 },
    /// The action does not fit the action space or breaks a rule. `message` must not reveal
    /// anything the seat may not know.
    Illegal { message: String },
    /// The engine cannot continue.
    Engine(EngineError),
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rejection::UnknownDecision { decision_id } => {
                write!(f, "no pending decision {decision_id}")
            }
            Rejection::WrongSeat { decision_id, seat } => {
                write!(f, "decision {decision_id} does not belong to {seat}")
            }
            Rejection::StaleRevision { expected, got } => {
                write!(f, "stale decision revision {got} (current is {expected})")
            }
            Rejection::Illegal { message } => write!(f, "illegal action: {message}"),
            Rejection::Engine(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Rejection {}

impl From<EngineError> for Rejection {
    fn from(e: EngineError) -> Self {
        Rejection::Engine(e)
    }
}

/// The complete dynamic state of a campaign: the ruleset's state plus the RNG.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Game<R: Ruleset> {
    pub state: R::State,
    pub rng: RngState,
}

/// A command for the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    /// Run automatic steps until a decision is pending or the game ends.
    Advance,
    /// A seat's answer to a pending decision.
    Respond(DecisionResponse),
}

/// The result of an accepted command.
#[derive(Debug, Clone)]
pub struct Transition<R: Ruleset> {
    pub game: Game<R>,
    pub events: Vec<EngineEvent>,
    /// Set when the command was `Advance`.
    pub progress: Option<Progress>,
}

/// Evaluate one command against a game without mutating it.
pub fn evaluate<R: Ruleset>(
    ruleset: &R,
    content: &R::Content,
    game: &Game<R>,
    command: &Command,
) -> Result<Transition<R>, Rejection> {
    let mut state = game.state.clone();
    let mut rng = CampaignRng::from_state(&game.rng);
    let mut events = Vec::new();
    let progress = {
        let mut cx = Cx {
            rng: &mut rng,
            events: &mut events,
        };
        match command {
            Command::Advance => Some(ruleset.advance(content, &mut state, &mut cx)?),
            Command::Respond(response) => {
                ruleset.respond(content, &mut state, response, &mut cx)?;
                None
            }
        }
    };
    Ok(Transition {
        game: Game {
            state,
            rng: rng.state(),
        },
        events,
        progress,
    })
}
