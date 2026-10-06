//! The seam between the seat tools and the rules engine.
//!
//! The tools talk to a [`GameBackend`]: everything a seat may do, already scoped to that seat.
//! [`RulesetCore`] implements it over any [`cna_core::engine::Ruleset`] (the toy game now, CNA
//! later) and plays the part the campaign runner will play in `cna-server`: it owns the
//! controller epoch, makes submissions idempotent, runs `Advance` after every accepted answer and
//! collects the audience-tagged events. When the real runner exists it can implement
//! [`GameBackend`] itself; the tools and drivers do not change.
//!
//! Every method takes the calling seat and returns only what that seat may know
//! (`docs/architecture.md` §4). The tools never filter again, so a leak here is a leak.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use async_trait::async_trait;

use cna_core::decision::{DecisionRequest, DecisionResponse};
use cna_core::dice::CampaignRng;
use cna_core::engine::{Command, Game, Progress, Rejection, Ruleset, evaluate};
use cna_core::event::EngineEvent;
use cna_core::ids::{DecisionId, SeatId};
use cna_core::visibility::Perspective;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A request to commit a decision response, as the `submit` tool builds it.
#[derive(Clone, Debug)]
pub struct SubmitRequest {
    pub decision_id: String,
    /// The controller epoch of the endpoint the call arrived on. A handover bumps the epoch, so a
    /// response from a superseded session is rejected.
    pub epoch: u64,
    /// The decision revision the seat saw; `None` means "the current one".
    pub revision: Option<u32>,
    /// Unique per intended action; a replay with the same key is never applied twice.
    pub idempotency_key: String,
    pub action: Value,
    pub public_explanation: Option<String>,
}

/// What a successful `submit` reports back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubmitReceipt {
    pub decision_id: String,
    /// `true` when this exact submission had already been accepted (idempotent replay).
    pub duplicate: bool,
    /// One-liner for the transcript's `decision_submitted` entry.
    pub summary: String,
    /// Model-facing result.
    pub result: Value,
}

/// Why a tool call failed. The message goes back to the model, so it must not leak hidden state.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ToolError {
    #[error("unknown decision `{0}` for this seat (it may already be resolved)")]
    UnknownDecision(String),
    #[error("unknown target `{0}`")]
    UnknownTarget(String),
    #[error("illegal: {0}")]
    Illegal(String),
    #[error("stale: {0}")]
    Stale(String),
    #[error("this seat's controller epoch is out of date; the session was superseded")]
    EpochMismatch,
    #[error("{0}")]
    Other(String),
}

/// The game as the seat tools see it. Async so that the campaign server can implement it over its
/// single-writer request channel; the toy backend answers immediately.
#[async_trait]
pub trait GameBackend: Send + Sync + 'static {
    /// All seats of this game, in a stable order.
    async fn seats(&self) -> Vec<SeatId>;
    /// Latest game event sequence number (used to align transcripts with the replay).
    async fn game_seq(&self) -> u64;
    /// The seat's open decisions.
    async fn pending(&self, seat: SeatId) -> Vec<DecisionRequest>;
    /// The seat's filtered situation report (the ruleset's `observe` for `Perspective::Seat`).
    async fn observe(&self, seat: SeatId) -> Value;
    /// Authorized detail of one thing (hex, unit, dump, …).
    async fn inspect(&self, seat: SeatId, target: &str) -> Result<Value, ToolError>;
    /// The legal action space of one pending decision, as JSON Schema plus the decision's context.
    async fn describe_actions(&self, seat: SeatId, decision_id: &str) -> Result<Value, ToolError>;
    /// Check a draft action without committing: no state change, no randomness consumed.
    async fn validate(
        &self,
        seat: SeatId,
        decision_id: &str,
        action: &Value,
    ) -> Result<Value, ToolError>;
    /// Commit an action. Idempotent on `idempotency_key`.
    async fn submit(
        &self,
        seat: SeatId,
        request: SubmitRequest,
    ) -> Result<SubmitReceipt, ToolError>;
    /// The controller epoch the game currently accepts for a seat.
    async fn epoch(&self, seat: SeatId) -> u64;
    /// `Some` once the game is over.
    async fn outcome(&self) -> Option<Value>;
}

/// A seat's ad-hoc `inspect` implementation for a ruleset. The engine contract has no inspect
/// method yet, so rulesets that want one supply it here.
pub type InspectFn<R> = Box<
    dyn Fn(
            &<R as Ruleset>::Content,
            &<R as Ruleset>::State,
            SeatId,
            &str,
        ) -> Result<Value, ToolError>
        + Send,
>;

/// The synchronous core of [`RulesetGame`]: a [`Ruleset`] plus the runner's duties (epoch,
/// idempotency, advance).
pub struct RulesetCore<R: Ruleset> {
    ruleset: R,
    content: R::Content,
    game: Game<R>,
    seats: Vec<SeatId>,
    epochs: BTreeMap<SeatId, u64>,
    receipts: BTreeMap<String, SubmitReceipt>,
    events: Vec<EngineEvent>,
    finished: Option<String>,
    inspector: Option<InspectFn<R>>,
}

impl<R> RulesetCore<R>
where
    R: Ruleset + Send + 'static,
    R::State: Send,
    R::Content: Send,
{
    /// Start a game and run its automatic opening steps.
    pub fn new(
        ruleset: R,
        content: R::Content,
        state: R::State,
        seed: [u8; 32],
        seats: Vec<SeatId>,
    ) -> Result<Self, Rejection> {
        let game = Game {
            state,
            rng: CampaignRng::from_seed(seed).state(),
        };
        let epochs = seats.iter().map(|s| (*s, 1)).collect();
        let mut me = Self {
            ruleset,
            content,
            game,
            seats,
            epochs,
            receipts: BTreeMap::new(),
            events: Vec::new(),
            finished: None,
            inspector: None,
        };
        me.advance()?;
        Ok(me)
    }

    pub fn with_inspector(mut self, f: InspectFn<R>) -> Self {
        self.inspector = Some(f);
        self
    }

    /// Replace a seat's epoch (a handover); older endpoints are then refused.
    pub fn set_epoch(&mut self, seat: SeatId, epoch: u64) {
        self.epochs.insert(seat, epoch);
    }

    /// Every event so far, with its sequence number (1-based).
    pub fn events(&self) -> impl Iterator<Item = (u64, &EngineEvent)> {
        self.events
            .iter()
            .enumerate()
            .map(|(i, e)| (i as u64 + 1, e))
    }

    /// Run `Advance` until a decision is pending or the game is over.
    fn advance(&mut self) -> Result<(), Rejection> {
        let t = evaluate(&self.ruleset, &self.content, &self.game, &Command::Advance)?;
        self.game = t.game;
        self.events.extend(t.events);
        if let Some(Progress::Finished { summary }) = t.progress {
            self.finished = Some(summary);
        }
        Ok(())
    }

    fn open_request(&self, seat: SeatId, decision_id: &str) -> Result<DecisionRequest, ToolError> {
        self.ruleset
            .pending(&self.content, &self.game.state)
            .into_iter()
            .find(|d| d.seat == seat && d.id.as_str() == decision_id)
            .ok_or_else(|| ToolError::UnknownDecision(decision_id.to_string()))
    }

    fn response(
        seat: SeatId,
        req: &SubmitRequest,
        request: &DecisionRequest,
        epoch: u64,
    ) -> DecisionResponse {
        DecisionResponse {
            decision_id: DecisionId::new(req.decision_id.clone()),
            seat,
            controller_epoch: epoch,
            decision_revision: req.revision.unwrap_or(request.revision),
            idempotency_key: req.idempotency_key.clone(),
            action: req.action.clone(),
            public_explanation: req.public_explanation.clone(),
        }
    }
}

fn reject(r: Rejection) -> ToolError {
    match r {
        // A decision of another seat looks exactly like a missing one: never confirm it exists.
        Rejection::UnknownDecision { decision_id } | Rejection::WrongSeat { decision_id, .. } => {
            ToolError::UnknownDecision(decision_id.to_string())
        }
        Rejection::StaleRevision { expected, got } => ToolError::Stale(format!(
            "decision revision {got} is out of date (current is {expected}); observe again"
        )),
        Rejection::Illegal { message } => ToolError::Illegal(message),
        Rejection::Engine(e) => ToolError::Other(e.to_string()),
    }
}

impl<R> RulesetCore<R>
where
    R: Ruleset + Send + 'static,
    R::State: Send,
    R::Content: Send,
{
    pub fn seats(&self) -> Vec<SeatId> {
        self.seats.clone()
    }

    pub fn game_seq(&self) -> u64 {
        self.events.len() as u64
    }

    pub fn pending(&self, seat: SeatId) -> Vec<DecisionRequest> {
        self.ruleset
            .pending(&self.content, &self.game.state)
            .into_iter()
            .filter(|d| d.seat == seat)
            .collect()
    }

    pub fn observe(&self, seat: SeatId) -> Value {
        self.ruleset
            .observe(&self.content, &self.game.state, Perspective::Seat(seat))
    }

    pub fn inspect(&self, seat: SeatId, target: &str) -> Result<Value, ToolError> {
        match &self.inspector {
            Some(f) => f(&self.content, &self.game.state, seat, target),
            None => Err(ToolError::UnknownTarget(format!(
                "{target} (this game offers no inspect targets; use observe)"
            ))),
        }
    }

    pub fn describe_actions(&self, seat: SeatId, decision_id: &str) -> Result<Value, ToolError> {
        let d = self.open_request(seat, decision_id)?;
        Ok(json!({
            "decision_id": d.id,
            "kind": d.kind,
            "revision": d.revision,
            "summary": d.summary,
            "rules": d.rules,
            "secrecy": d.secrecy,
            "action_schema": d.space.to_json_schema(),
            "pass": d.space.pass,
            "how_to_answer": "Call submit with decision_id and `action` shaped like action_schema (null passes where `pass` is set).",
        }))
    }

    pub fn validate(
        &self,
        seat: SeatId,
        decision_id: &str,
        action: &Value,
    ) -> Result<Value, ToolError> {
        let d = self.open_request(seat, decision_id)?;
        let req = SubmitRequest {
            decision_id: decision_id.to_string(),
            epoch: self.epoch(seat),
            revision: None,
            idempotency_key: "validate".into(),
            action: action.clone(),
            public_explanation: None,
        };
        let response = Self::response(seat, &req, &d, self.epoch(seat));
        // `evaluate` clones the state and the RNG, so nothing here can leak into the game.
        evaluate(
            &self.ruleset,
            &self.content,
            &self.game,
            &Command::Respond(response),
        )
        .map(|_| json!({ "valid": true }))
        .map_err(reject)
    }

    pub fn submit(&mut self, seat: SeatId, req: SubmitRequest) -> Result<SubmitReceipt, ToolError> {
        if req.epoch != self.epoch(seat) {
            return Err(ToolError::EpochMismatch);
        }
        let key = format!("{seat}|{}", req.idempotency_key);
        if let Some(previous) = self.receipts.get(&key) {
            let mut again = previous.clone();
            again.duplicate = true;
            again.result =
                json!({ "accepted": true, "duplicate": true, "decision_id": again.decision_id });
            return Ok(again);
        }
        let d = self.open_request(seat, &req.decision_id)?;
        let response = Self::response(seat, &req, &d, req.epoch);
        let t = evaluate(
            &self.ruleset,
            &self.content,
            &self.game,
            &Command::Respond(response),
        )
        .map_err(reject)?;
        self.game = t.game;
        let first_new = self.events.len();
        self.events.extend(t.events);
        self.advance().map_err(reject)?;
        let me = Perspective::Seat(seat);
        let visible: Vec<&_> = self.events[first_new..]
            .iter()
            .filter(|e| e.visible_to(me))
            .map(|e| &e.event)
            .collect();
        let receipt = SubmitReceipt {
            decision_id: req.decision_id.clone(),
            duplicate: false,
            summary: format!("submitted {}", compact(&req.action)),
            result: json!({
                "accepted": true,
                "duplicate": false,
                "decision_id": req.decision_id,
                "events": visible,
                "your_pending_decisions": self.pending(seat).len(),
                "finished": self.finished,
            }),
        };
        self.receipts.insert(key, receipt.clone());
        Ok(receipt)
    }

    pub fn epoch(&self, seat: SeatId) -> u64 {
        self.epochs.get(&seat).copied().unwrap_or(0)
    }

    pub fn outcome(&self) -> Option<Value> {
        self.finished.as_ref().map(|s| json!({ "summary": s }))
    }
}

fn compact(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 120 {
        format!("{}…", s.chars().take(120).collect::<String>())
    } else {
        s
    }
}

/// Seats that have at least one pending decision, in seat order.
pub async fn seats_with_pending(game: &dyn GameBackend) -> BTreeSet<SeatId> {
    let mut out = BTreeSet::new();
    for s in game.seats().await {
        if !game.pending(s).await.is_empty() {
            out.insert(s);
        }
    }
    out
}

/// [`GameBackend`] over any [`Ruleset`]; see [`RulesetCore`] for the semantics.
pub struct RulesetGame<R: Ruleset>(Mutex<RulesetCore<R>>);

impl<R> RulesetGame<R>
where
    R: Ruleset + Send + 'static,
    R::State: Send,
    R::Content: Send,
{
    pub fn new(
        ruleset: R,
        content: R::Content,
        state: R::State,
        seed: [u8; 32],
        seats: Vec<SeatId>,
    ) -> Result<Self, Rejection> {
        Ok(Self(Mutex::new(RulesetCore::new(
            ruleset, content, state, seed, seats,
        )?)))
    }

    pub fn from_core(core: RulesetCore<R>) -> Self {
        Self(Mutex::new(core))
    }

    pub fn with_inspector(self, f: InspectFn<R>) -> Self {
        let core = self.0.into_inner().expect("game lock");
        Self(Mutex::new(core.with_inspector(f)))
    }

    /// Run `f` with the core locked (tests and the local runner).
    pub fn with_core<T>(&self, f: impl FnOnce(&mut RulesetCore<R>) -> T) -> T {
        f(&mut self.0.lock().expect("game lock"))
    }
}

#[async_trait]
impl<R> GameBackend for RulesetGame<R>
where
    R: Ruleset + Send + 'static,
    R::State: Send,
    R::Content: Send,
{
    async fn seats(&self) -> Vec<SeatId> {
        self.with_core(|c| c.seats())
    }
    async fn game_seq(&self) -> u64 {
        self.with_core(|c| c.game_seq())
    }
    async fn pending(&self, seat: SeatId) -> Vec<DecisionRequest> {
        self.with_core(|c| c.pending(seat))
    }
    async fn observe(&self, seat: SeatId) -> Value {
        self.with_core(|c| c.observe(seat))
    }
    async fn inspect(&self, seat: SeatId, target: &str) -> Result<Value, ToolError> {
        self.with_core(|c| c.inspect(seat, target))
    }
    async fn describe_actions(&self, seat: SeatId, decision_id: &str) -> Result<Value, ToolError> {
        self.with_core(|c| c.describe_actions(seat, decision_id))
    }
    async fn validate(
        &self,
        seat: SeatId,
        decision_id: &str,
        action: &Value,
    ) -> Result<Value, ToolError> {
        self.with_core(|c| c.validate(seat, decision_id, action))
    }
    async fn submit(
        &self,
        seat: SeatId,
        request: SubmitRequest,
    ) -> Result<SubmitReceipt, ToolError> {
        self.with_core(|c| c.submit(seat, request))
    }
    async fn epoch(&self, seat: SeatId) -> u64 {
        self.with_core(|c| c.epoch(seat))
    }
    async fn outcome(&self) -> Option<Value> {
        self.with_core(|c| c.outcome())
    }
}
