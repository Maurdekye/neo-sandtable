use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use cna_core::{
    decision::{DecisionRequest, DecisionResponse},
    engine::{Command, Game, Progress, Rejection, Ruleset, evaluate},
    ids::SeatId,
    visibility::{Audience, Perspective},
};
use cna_protocol::{
    CampaignMeta, ControllerInfo, GameEvent, ServerMessage, TranscriptEntry, ViewState,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

const CHECKPOINT_INTERVAL: u64 = 32;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("storage failure: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("serialization failure: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Rejected(#[from] Rejection),
    #[error("controller was superseded")]
    StaleEpoch,
    #[error("idempotency key already identifies another command")]
    IdempotencyConflict,
    #[error("campaign is not running")]
    NotRunning,
    #[error("campaign recovery mismatch: {0}")]
    Recovery(String),
    #[error("invalid request: {0}")]
    Invalid(String),
}

/// Immutable campaign inputs; callers must hash the actual loaded content, not a display label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pins {
    pub rules_profile: String,
    pub content_hash: String,
    pub engine_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CampaignStatus {
    Running,
    Paused,
    Finished { summary: String },
    Stopped { error: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub controller: Option<ControllerInfo>,
    pub config: serde_json::Value,
    pub controller_epoch: u64,
    /// A failure is a pause, never an automatically substituted order.
    pub paused: bool,
    #[serde(default)]
    pub failure: Option<String>,
}

impl Default for Binding {
    fn default() -> Self {
        Self {
            controller: None,
            config: serde_json::Value::Null,
            controller_epoch: 0,
            paused: false,
            failure: None,
        }
    }
}

/// A receipt intentionally carries no global revision, state hash, or hidden event counter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub decision_id: Option<String>,
    pub duplicate: bool,
}

/// Trusted-process timing diagnostics, never included in HTTP, WebSocket or seat tools.
/// They reset on recovery and cannot affect persisted state, receipts or adjudication RNG.
#[derive(Debug, Clone, Copy, Default)]
pub struct RuntimeMetrics {
    pub committed_commands: u64,
    pub engine: Duration,
    /// Serialization, state hashing, pending/stream rows and SQLite FULL transaction commit.
    pub durable_writer: Duration,
    /// All perspective snapshots, seat observations and committed stream publication.
    pub projections: Duration,
    /// Scripted selection/validation outside the separately measured engine and writer.
    pub controllers: Duration,
}

/// Owned by exactly one writer. Viewers consume committed projections, never the mutable game.
pub struct Campaign<R: Ruleset> {
    db: Connection,
    pub(crate) metrics: Arc<Mutex<RuntimeMetrics>>,
    ruleset: R,
    pub(crate) content: R::Content,
    pub(crate) game: Game<R>,
    meta: CampaignMeta,
    revision: u64,
    status: CampaignStatus,
    bindings: BTreeMap<SeatId, Binding>,
}

fn json<T: Serialize + ?Sized>(value: &T) -> Result<String, Error> {
    Ok(serde_json::to_string(value)?)
}
fn decode<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
    Ok(serde_json::from_str(text)?)
}
fn hash<T: Serialize + ?Sized>(value: &T) -> Result<String, Error> {
    Ok(format!("{:x}", Sha256::digest(json(value)?.as_bytes())))
}
fn connection(path: &Path) -> Result<Connection, Error> {
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "synchronous", "FULL")?;
    db.execute_batch(include_str!("schema.sql"))?;
    Ok(db)
}
fn seq(db: &Connection, perspective: Perspective) -> Result<u64, Error> {
    Ok(db.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM perspective_events WHERE perspective = ?",
        [perspective.to_string()],
        |r| r.get(0),
    )?)
}

fn store_transcript(
    db: &Connection,
    seat: SeatId,
    at: &str,
    entry: &TranscriptEntry,
) -> Result<u64, Error> {
    let tseq: u64 = db.query_row(
        "SELECT COALESCE(MAX(tseq), 0)+1 FROM transcripts WHERE seat=?",
        [seat.to_string()],
        |r| r.get(0),
    )?;
    db.execute(
        "INSERT INTO transcripts VALUES (?, ?, ?, ?)",
        params![seat.to_string(), tseq, at, json(entry)?],
    )?;
    for perspective in Perspective::all().filter(|p| p.can_see(&Audience::Seat(seat))) {
        db.execute(
            "INSERT INTO perspective_transcripts VALUES (?, ?, ?, ?)",
            params![
                perspective.to_string(),
                seat.to_string(),
                tseq,
                seq(db, perspective)?
            ],
        )?;
    }
    Ok(tseq)
}

impl<R: Ruleset> Campaign<R> {
    pub fn create(
        path: &Path,
        ruleset: R,
        content: R::Content,
        game: Game<R>,
        meta: CampaignMeta,
        pins: Pins,
    ) -> Result<Self, Error> {
        if pins.rules_profile != ruleset.profile_id() || meta.rules_profile != pins.rules_profile {
            return Err(Error::Invalid("rules-profile pins disagree".into()));
        }
        let mut db = connection(path)?;
        let tx = db.transaction()?;
        let state_hash = hash(&game)?;
        tx.execute(
            "INSERT INTO campaign VALUES (1, ?, ?, 0, ?, ?, ?)",
            params![
                json(&meta)?,
                json(&pins)?,
                json(&game.rng)?,
                state_hash,
                json(&CampaignStatus::Running)?
            ],
        )?;
        tx.execute(
            "INSERT INTO checkpoints VALUES (0, ?, ?)",
            params![json(&game)?, state_hash],
        )?;
        let bindings: BTreeMap<_, _> = SeatId::all().map(|s| (s, Binding::default())).collect();
        for (seat, binding) in &bindings {
            tx.execute(
                "INSERT INTO seats VALUES (?, ?)",
                params![seat.to_string(), json(binding)?],
            )?;
        }
        // Initial decisions, if a ruleset starts with any, are also durable.
        for request in ruleset.pending(&content, &game.state) {
            tx.execute(
                "INSERT INTO decisions VALUES (?, ?, NULL)",
                params![request.id.as_str(), json(&request)?],
            )?;
        }
        tx.commit()?;
        Ok(Self {
            db,
            metrics: Arc::new(Mutex::new(RuntimeMetrics::default())),
            ruleset,
            content,
            game,
            meta,
            revision: 0,
            status: CampaignStatus::Running,
            bindings,
        })
    }

    /// Re-evaluate every accepted command following the latest verified checkpoint.
    pub fn recover(
        path: &Path,
        ruleset: R,
        content: R::Content,
        expected: &Pins,
    ) -> Result<Self, Error> {
        let db = connection(path)?;
        let (meta, pins, revision, rng, final_hash, status): (
            String,
            String,
            u64,
            String,
            String,
            String,
        ) = db.query_row(
            "SELECT meta, pins, revision, rng, state_hash, status FROM campaign WHERE id=1",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )?;
        if decode::<Pins>(&pins)? != *expected || expected.rules_profile != ruleset.profile_id() {
            return Err(Error::Recovery("immutable input pins changed".into()));
        }
        let (at, checkpoint, checkpoint_hash): (u64, String, String) = db.query_row(
            "SELECT revision, game, state_hash FROM checkpoints ORDER BY revision DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let mut game: Game<R> = decode(&checkpoint)?;
        if hash(&game)? != checkpoint_hash || at > revision {
            return Err(Error::Recovery("checkpoint integrity".into()));
        }
        let mut replay_revision = at;
        {
            let mut stmt = db.prepare("SELECT revision, command, state_hash, transition_hash FROM commands WHERE revision > ? ORDER BY revision")?;
            let rows = stmt.query_map([at], |r| {
                Ok((
                    r.get::<_, u64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (next, command, state_hash, transition_hash) = row?;
                if next != replay_revision + 1 {
                    return Err(Error::Recovery("command sequence gap".into()));
                }
                let transition =
                    evaluate(&ruleset, &content, &game, &decode::<Command>(&command)?)?;
                if hash(&transition.game)? != state_hash
                    || hash(&(&transition.events, &transition.progress))? != transition_hash
                {
                    return Err(Error::Recovery(format!("replayed transition {next}")));
                }
                game = transition.game;
                replay_revision = next;
            }
        }
        if replay_revision != revision || hash(&game)? != final_hash || json(&game.rng)? != rng {
            return Err(Error::Recovery("final state or RNG".into()));
        }
        let mut bindings = BTreeMap::new();
        {
            let mut stmt = db.prepare("SELECT seat, binding FROM seats ORDER BY seat")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (seat, binding) = row?;
                bindings.insert(
                    seat.parse()
                        .map_err(|_| Error::Recovery("seat identity".into()))?,
                    decode(&binding)?,
                );
            }
        }
        if bindings.len() != SeatId::all().count() {
            return Err(Error::Recovery("controller bindings missing".into()));
        }
        let campaign = Self {
            db,
            metrics: Arc::new(Mutex::new(RuntimeMetrics::default())),
            ruleset,
            content,
            game,
            meta: decode(&meta)?,
            revision,
            status: decode(&status)?,
            bindings,
        };
        let mut persisted = campaign
            .db
            .prepare("SELECT request FROM decisions WHERE resolved_revision IS NULL ORDER BY id")?;
        let rows = persisted.query_map([], |r| r.get::<_, String>(0))?;
        let mut stored = Vec::new();
        for row in rows {
            stored.push(decode::<DecisionRequest>(&row?)?);
        }
        let mut pending = campaign.pending();
        pending.sort_by(|a, b| a.id.cmp(&b.id));
        if stored != pending {
            return Err(Error::Recovery("pending decisions disagree".into()));
        }
        drop(persisted);
        Ok(campaign)
    }

    pub fn status(&self) -> &CampaignStatus {
        &self.status
    }
    pub fn state_hash(&self) -> Result<String, Error> {
        hash(&self.game)
    }
    pub fn pending(&self) -> Vec<DecisionRequest> {
        self.ruleset.pending(&self.content, &self.game.state)
    }
    pub fn binding(&self, seat: SeatId) -> &Binding {
        &self.bindings[&seat]
    }
    pub fn observe(&self, seat: SeatId) -> serde_json::Value {
        self.ruleset
            .observe(&self.content, &self.game.state, Perspective::Seat(seat))
    }
    pub fn actions(&self, seat: SeatId, id: &str) -> Result<DecisionRequest, Error> {
        self.pending()
            .into_iter()
            .find(|d| d.seat == seat && d.id.as_str() == id)
            .ok_or_else(|| {
                Rejection::UnknownDecision {
                    decision_id: id.into(),
                }
                .into()
            })
    }

    pub fn set_paused(&mut self, paused: bool) -> Result<(), Error> {
        if !matches!(
            self.status,
            CampaignStatus::Running | CampaignStatus::Paused
        ) {
            return Err(Error::NotRunning);
        }
        let next = if paused {
            CampaignStatus::Paused
        } else {
            CampaignStatus::Running
        };
        self.db
            .execute("UPDATE campaign SET status=? WHERE id=1", [json(&next)?])?;
        self.status = next;
        Ok(())
    }

    /// Runtime failure cannot resume automatically, even if publishing a committed projection
    /// failed. Persist the stop when storage permits; retain it in memory if storage is broken.
    pub(crate) fn stop_runtime(&mut self, error: &Error) -> Result<(), Error> {
        self.status = CampaignStatus::Stopped {
            error: error.to_string(),
        };
        self.db.execute(
            "UPDATE campaign SET status=? WHERE id=1",
            [json(&self.status)?],
        )?;
        Ok(())
    }

    /// Handover preserves the entire game and its accepted secret submissions.
    pub fn handover(
        &mut self,
        seat: SeatId,
        controller: Option<ControllerInfo>,
        config: serde_json::Value,
    ) -> Result<Binding, Error> {
        if controller
            .as_ref()
            .is_some_and(|c| c.kind == cna_protocol::ControllerKind::Scripted)
            && !matches!(
                config["mode"].as_str(),
                Some("legal_random" | "pass_when_possible" | "aggressive")
            )
        {
            return Err(Error::Invalid(
                "scripted controller requires a supported mode".into(),
            ));
        }
        let epoch = self
            .binding(seat)
            .controller_epoch
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("epoch exhausted".into()))?;
        let binding = Binding {
            controller,
            config,
            controller_epoch: epoch,
            paused: false,
            failure: None,
        };
        self.store_binding(seat, &binding)?;
        Ok(binding)
    }
    fn store_binding(&mut self, seat: SeatId, binding: &Binding) -> Result<(), Error> {
        self.db.execute(
            "UPDATE seats SET binding=? WHERE seat=?",
            params![json(binding)?, seat.to_string()],
        )?;
        self.bindings.insert(seat, binding.clone());
        Ok(())
    }
    pub fn pause_seat(&mut self, seat: SeatId) -> Result<(), Error> {
        self.pause_seat_with_reason(seat, "paused; retry or hand over the seat")
    }
    pub fn pause_seat_with_reason(&mut self, seat: SeatId, reason: &str) -> Result<(), Error> {
        if reason.len() > 4096 {
            return Err(Error::Invalid("failure reason too long".into()));
        }
        let mut binding = self.binding(seat).clone();
        binding.paused = true;
        binding.failure = Some(reason.into());
        let report = binding != *self.binding(seat)
            && binding
                .controller
                .as_ref()
                .is_some_and(|c| c.kind == cna_protocol::ControllerKind::Scripted);
        let tx = self.db.transaction()?;
        tx.execute(
            "UPDATE seats SET binding=? WHERE seat=?",
            params![json(&binding)?, seat.to_string()],
        )?;
        if report {
            store_transcript(
                &tx,
                seat,
                &cna_seats::transcript::now_rfc3339(),
                &TranscriptEntry::System {
                    text: format!("Scripted controller paused: {reason}"),
                },
            )?;
        }
        tx.commit()?;
        self.bindings.insert(seat, binding);
        Ok(())
    }
    fn check_response(&self, response: &DecisionResponse) -> Result<(), Error> {
        if self.binding(response.seat).controller_epoch != response.controller_epoch {
            return Err(Error::StaleEpoch);
        }
        let request = self.actions(response.seat, response.decision_id.as_str())?;
        if request.revision != response.decision_revision {
            return Err(Rejection::StaleRevision {
                expected: request.revision,
                got: response.decision_revision,
            }
            .into());
        }
        if response.idempotency_key.is_empty() {
            return Err(Error::Invalid("empty idempotency key".into()));
        }
        if self.binding(response.seat).paused {
            return Err(Error::Invalid("seat is paused; handover to retry".into()));
        }
        Ok(())
    }

    /// Validation evaluates a private clone: no game mutation and no RNG consumption.
    pub fn validate(&self, response: &DecisionResponse) -> Result<(), Error> {
        self.check_response(response)?;
        evaluate(
            &self.ruleset,
            &self.content,
            &self.game,
            &Command::Respond(response.clone()),
        )?;
        Ok(())
    }
    pub fn submit(&mut self, response: DecisionResponse) -> Result<Receipt, Error> {
        let command = Command::Respond(response.clone());
        let prior: Option<String> = self
            .db
            .query_row(
                "SELECT command FROM commands WHERE seat=? AND idempotency_key=?",
                params![response.seat.to_string(), response.idempotency_key],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(prior) = prior {
            if decode::<Command>(&prior)? != command {
                return Err(Error::IdempotencyConflict);
            }
            return Ok(Receipt {
                decision_id: Some(response.decision_id.to_string()),
                duplicate: true,
            });
        }
        self.check_response(&response)?;
        self.commit(command, response.seat.to_string(), response.idempotency_key)
    }
    pub fn advance(&mut self) -> Result<Receipt, Error> {
        // Never resume a suspended automatic operation while any decision remains unanswered.
        if !self.pending().is_empty() {
            return Err(Error::Invalid("decisions are pending".into()));
        }
        self.commit(
            Command::Advance,
            String::new(),
            format!("advance:{}", self.revision + 1),
        )
    }
    fn commit(&mut self, command: Command, seat: String, key: String) -> Result<Receipt, Error> {
        if self.status != CampaignStatus::Running {
            return Err(Error::NotRunning);
        }
        let engine_start = Instant::now();
        let transition = match evaluate(&self.ruleset, &self.content, &self.game, &command) {
            Ok(t) => t,
            Err(Rejection::Engine(error)) => {
                let stopped = CampaignStatus::Stopped {
                    error: error.to_string(),
                };
                self.db
                    .execute("UPDATE campaign SET status=? WHERE id=1", [json(&stopped)?])?;
                self.status = stopped;
                tracing::error!(%error, "campaign stopped at an unsupported case or invariant");
                return Err(Rejection::Engine(error).into());
            }
            Err(e) => return Err(e.into()),
        };
        let engine_elapsed = engine_start.elapsed();
        let writer_start = Instant::now();
        let revision = self.revision + 1;
        let state_hash = hash(&transition.game)?;
        let status = match &transition.progress {
            Some(Progress::Finished { summary }) => CampaignStatus::Finished {
                summary: summary.clone(),
            },
            _ => self.status.clone(),
        };
        let pending = self.ruleset.pending(&self.content, &transition.game.state);
        let mut identities = std::collections::BTreeSet::new();
        let invalid = pending.iter().any(|d| !identities.insert(d.id.clone()))
            || (matches!(transition.progress, Some(Progress::AwaitingDecisions))
                && pending.is_empty())
            || (matches!(transition.progress, Some(Progress::Finished { .. }))
                && !pending.is_empty());
        if invalid {
            let error = cna_core::engine::EngineError::Invariant {
                detail: "ruleset progress and pending decisions disagree".into(),
            };
            let stopped = CampaignStatus::Stopped {
                error: error.to_string(),
            };
            self.db
                .execute("UPDATE campaign SET status=? WHERE id=1", [json(&stopped)?])?;
            self.status = stopped;
            return Err(Rejection::Engine(error).into());
        }
        let clock = self
            .ruleset
            .view(&self.content, &self.game.state, Perspective::Operator)
            .clock;
        let scripted_transcript = match &command {
            Command::Respond(response)
                if self
                    .binding(response.seat)
                    .controller
                    .as_ref()
                    .is_some_and(|c| c.kind == cna_protocol::ControllerKind::Scripted) =>
            {
                Some((
                    response.seat,
                    cna_seats::transcript::now_rfc3339(),
                    TranscriptEntry::DecisionSubmitted {
                        decision_id: response.decision_id.to_string(),
                        summary: format!(
                            "{} accepted action {}",
                            self.binding(response.seat).config["mode"]
                                .as_str()
                                .unwrap_or("scripted"),
                            json(&response.action)?
                        ),
                    },
                ))
            }
            _ => None,
        };
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO commands VALUES (?, ?, ?, ?, ?, ?)",
            params![
                revision,
                seat,
                key,
                json(&command)?,
                state_hash,
                hash(&(&transition.events, &transition.progress))?
            ],
        )?;
        let mut clock = clock;
        for event in &transition.events {
            if let GameEvent::PhaseChanged { clock: next } = &event.event {
                clock = next.clone();
            }
            tx.execute(
                "INSERT INTO events(revision, payload) VALUES (?, ?)",
                params![revision, json(event)?],
            )?;
            let event_id = tx.last_insert_rowid();
            for perspective in Perspective::all().filter(|p| event.visible_to(*p)) {
                let next_seq = seq(&tx, perspective)? + 1;
                let mut payload = event.event.clone();
                if let GameEvent::DecisionOpened { decision } = &mut payload {
                    decision.opened_seq = next_seq;
                }
                let message = ServerMessage::Event {
                    seq: next_seq,
                    clock: clock.clone(),
                    event: payload,
                    hex: event.hex.clone(),
                    unit_id: event.unit_id.clone(),
                };
                tx.execute(
                    "INSERT INTO perspective_events VALUES (?, ?, ?, ?)",
                    params![perspective.to_string(), next_seq, event_id, json(&message)?],
                )?;
            }
        }
        tx.execute(
            "UPDATE decisions SET resolved_revision=? WHERE resolved_revision IS NULL",
            [revision],
        )?;
        for request in &pending {
            tx.execute("INSERT INTO decisions VALUES (?, ?, NULL) ON CONFLICT(id) DO UPDATE SET request=excluded.request, resolved_revision=NULL", params![request.id.as_str(), json(request)?])?;
        }
        tx.execute(
            "UPDATE campaign SET revision=?, rng=?, state_hash=?, status=? WHERE id=1",
            params![
                revision,
                json(&transition.game.rng)?,
                state_hash,
                json(&status)?
            ],
        )?;
        // Baseline transcripts commit with their accepted answer, so neither crash nor
        // duplicate retry can lose or duplicate a decision_submitted entry.
        if let Some((seat, at, entry)) = scripted_transcript {
            store_transcript(&tx, seat, &at, &entry)?;
        }
        if revision.is_multiple_of(CHECKPOINT_INTERVAL) {
            tx.execute(
                "INSERT INTO checkpoints VALUES (?, ?, ?)",
                params![revision, json(&transition.game)?, state_hash],
            )?;
        }
        tx.commit()?;
        // A failed transaction cannot advance the in-memory state or produce an acknowledgement.
        self.game = transition.game;
        self.revision = revision;
        self.status = status;
        {
            let mut metrics = self.metrics.lock().expect("runtime metrics lock");
            metrics.committed_commands += 1;
            metrics.engine += engine_elapsed;
            metrics.durable_writer += writer_start.elapsed();
        }
        let decision_id = match command {
            Command::Respond(r) => Some(r.decision_id.to_string()),
            Command::Advance => None,
        };
        Ok(Receipt {
            decision_id,
            duplicate: false,
        })
    }

    pub fn current_seq(&self, perspective: Perspective) -> Result<u64, Error> {
        seq(&self.db, perspective)
    }
    pub fn view(&self, perspective: Perspective) -> Result<ViewState, Error> {
        let mut view = self
            .ruleset
            .view(&self.content, &self.game.state, perspective);
        for decision in &mut view.pending {
            decision.opened_seq = self.opened_seq(perspective, &decision.id)?;
        }
        Ok(view)
    }
    fn opened_seq(&self, perspective: Perspective, id: &str) -> Result<u64, Error> {
        // Never forward a ruleset's global event counter in a perspective projection.
        Ok(self.db.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM perspective_events WHERE perspective=? AND json_extract(message, '$.event.kind')='decision_opened' AND json_extract(message, '$.event.decision.id')=?",
            params![perspective.to_string(), id], |r| r.get(0),
        )?)
    }
    pub fn metadata(&self, perspective: Perspective) -> CampaignMeta {
        let mut meta = self.meta.clone();
        for seat in &mut meta.seats {
            if let Ok(id) = seat.id.parse::<SeatId>() {
                let visible = perspective.can_see(&Audience::Seat(id));
                // Hidden seats get no runtime activity, failure, or session information.
                seat.controller = visible
                    .then(|| self.binding(id).controller.clone())
                    .flatten();
                seat.status = if !visible {
                    cna_protocol::SeatStatus::Idle
                } else if self.binding(id).paused {
                    cna_protocol::SeatStatus::Paused
                } else if self.pending().iter().any(|d| d.seat == id) {
                    cna_protocol::SeatStatus::Deciding
                } else {
                    cna_protocol::SeatStatus::Idle
                };
            }
        }
        meta
    }
    /// Ordered bounded pages for reconnect/playback. An ahead-of-stream cursor is a gap.
    pub fn events_after(
        &self,
        perspective: Perspective,
        from: u64,
        limit: u32,
    ) -> Result<Vec<ServerMessage>, Error> {
        if from > seq(&self.db, perspective)? {
            return Err(Error::Invalid("event cursor is ahead of stream".into()));
        }
        let mut stmt = self.db.prepare("SELECT message FROM perspective_events WHERE perspective=? AND seq>? ORDER BY seq LIMIT ?")?;
        let rows = stmt.query_map(
            params![perspective.to_string(), from, limit.min(512)],
            |r| r.get::<_, String>(0),
        )?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(decode(&row?)?);
        }
        Ok(messages)
    }
    /// The single writer assigns transcript counters and all 13 alignment projections atomically.
    pub fn transcript(
        &mut self,
        seat: SeatId,
        at: &str,
        entry: TranscriptEntry,
    ) -> Result<u64, Error> {
        let tx = self.db.transaction()?;
        let tseq = store_transcript(&tx, seat, at, &entry)?;
        tx.commit()?;
        Ok(tseq)
    }
    pub(crate) fn transcript_seq(&self, seat: SeatId) -> Result<u64, Error> {
        Ok(self.db.query_row(
            "SELECT COALESCE(MAX(tseq),0) FROM transcripts WHERE seat=?",
            [seat.to_string()],
            |r| r.get(0),
        )?)
    }
    pub fn transcripts_after(
        &self,
        perspective: Perspective,
        seat: SeatId,
        from: u64,
        limit: u32,
    ) -> Result<Vec<ServerMessage>, Error> {
        if !perspective.can_see(&Audience::Seat(seat)) {
            return Ok(Vec::new());
        }
        let mut stmt = self.db.prepare("SELECT t.tseq, t.at, p.game_seq, t.entry FROM transcripts t JOIN perspective_transcripts p USING(seat,tseq) WHERE p.perspective=? AND t.seat=? AND t.tseq>? ORDER BY t.tseq LIMIT ?")?;
        let rows = stmt.query_map(
            params![
                perspective.to_string(),
                seat.to_string(),
                from,
                limit.min(512)
            ],
            |r| {
                Ok((
                    r.get::<_, u64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )?;
        let mut messages = Vec::new();
        for row in rows {
            let (tseq, at, game_seq, entry) = row?;
            messages.push(ServerMessage::Transcript {
                seat: seat.to_string(),
                tseq,
                at,
                game_seq,
                entry: decode(&entry)?,
            });
        }
        Ok(messages)
    }
}

impl<R: Ruleset> Campaign<R> {
    pub fn inspect(&self, seat: SeatId, target: &str) -> Result<serde_json::Value, Error> {
        Ok(self.ruleset.inspect(
            &self.content,
            &self.game.state,
            Perspective::Seat(seat),
            target,
        )?)
    }
    pub fn prior_response(
        &self,
        seat: SeatId,
        key: &str,
    ) -> Result<Option<DecisionResponse>, Error> {
        let command: Option<String> = self
            .db
            .query_row(
                "SELECT command FROM commands WHERE seat=? AND idempotency_key=?",
                params![seat.to_string(), key],
                |r| r.get(0),
            )
            .optional()?;
        match command.map(|c| decode::<Command>(&c)).transpose()? {
            Some(Command::Respond(response)) => Ok(Some(response)),
            _ => Ok(None),
        }
    }
    pub fn notebook(&self, seat: SeatId) -> Result<String, Error> {
        Ok(self
            .db
            .query_row(
                "SELECT text FROM notebooks WHERE seat=? AND key='main'",
                [seat.to_string()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default())
    }
    pub fn write_notebook(
        &mut self,
        seat: SeatId,
        mode: cna_seats::memory::WriteMode,
        text: &str,
    ) -> Result<usize, Error> {
        let next = match mode {
            cna_seats::memory::WriteMode::Replace => text.to_owned(),
            cna_seats::memory::WriteMode::Append => self.notebook(seat)? + text,
        };
        if next.len() > cna_seats::memory::NOTEBOOK_LIMIT_BYTES {
            return Err(Error::Invalid("notebook size limit exceeded".into()));
        }
        self.db.execute("INSERT INTO notebooks VALUES (?, 'main', ?) ON CONFLICT(seat,key) DO UPDATE SET text=excluded.text", params![seat.to_string(), next])?;
        Ok(next.len())
    }
    pub fn team_message(&mut self, from: SeatId, text: &str) -> Result<usize, Error> {
        if text.len() > 32768 {
            return Err(Error::Invalid("message size limit exceeded".into()));
        }
        let tx = self.db.transaction()?;
        let mut sent = 0;
        for recipient in SeatId::all().filter(|s| s.side == from.side && *s != from) {
            // Per-recipient counters never reveal how many enemy or other-seat messages exist.
            let n: u64 = tx.query_row(
                "SELECT COALESCE(MAX(json_extract(message, '$.n')),0)+1 FROM messages WHERE seat=?",
                [recipient.to_string()],
                |r| r.get(0),
            )?;
            let message = cna_seats::memory::TeamMessage {
                n,
                from,
                text: text.into(),
            };
            tx.execute(
                "INSERT INTO messages(side,seat,message) VALUES (?, ?, ?)",
                params![
                    from.side.to_string(),
                    recipient.to_string(),
                    json(&message)?
                ],
            )?;
            sent += 1;
        }
        tx.commit()?;
        Ok(sent)
    }
    pub fn read_messages(
        &self,
        seat: SeatId,
        after: u64,
    ) -> Result<Vec<cna_seats::memory::TeamMessage>, Error> {
        let mut stmt = self.db.prepare("SELECT message FROM messages WHERE seat=? AND json_extract(message, '$.n')>? ORDER BY json_extract(message, '$.n') LIMIT 512")?;
        let rows = stmt.query_map(params![seat.to_string(), after], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(decode(&row?)?);
        }
        Ok(out)
    }
}
