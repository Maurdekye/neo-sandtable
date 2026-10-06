//! Bounded actor commands and isolated, committed reader projections.
use crate::{
    Binding, Campaign, CampaignStatus, Error, Receipt,
    replay::ReplayReader,
    scripted::{Candidates, NoCandidates, Step},
};
use cna_core::{
    decision::{DecisionRequest, DecisionResponse},
    engine::{Rejection, Ruleset},
    ids::SeatId,
    visibility::{Audience, Perspective},
};
use cna_protocol::{CampaignMeta, ControllerInfo, ServerMessage, TranscriptEntry, ViewState};
use cna_seats::{game::SubmitRequest, memory::WriteMode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

const COMMAND_BUFFER: usize = 128;
pub const VIEWER_BUFFER: usize = 128;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    pub meta: CampaignMeta,
    pub seq: u64,
    pub view: ViewState,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SeatState {
    pub pending: Vec<DecisionRequest>,
    pub observation: Value,
    pub binding: Binding,
}

pub type Baseline<R> =
    Box<dyn Fn(&<R as Ruleset>::Content, &<R as Ruleset>::State, &DecisionRequest) -> Value + Send>;

enum Op {
    Pause(bool),
    Handover(SeatId, Option<ControllerInfo>, Value),
    PauseSeat(SeatId, Option<u64>, String),
    Submit(DecisionResponse),
    SubmitAction(SeatId, SubmitRequest),
    Validate(SeatId, String, Value),
    Inspect(SeatId, String),
    Transcript(SeatId, String, TranscriptEntry),
    Notebook(SeatId),
    WriteNotebook(SeatId, WriteMode, String),
    Message(SeatId, String),
    Messages(SeatId, u64),
    Shutdown,
}
struct Envelope {
    op: Op,
    reply: oneshot::Sender<Result<Value, Error>>,
}
struct Inner {
    tx: mpsc::Sender<Envelope>,
    projections: BTreeMap<Perspective, watch::Receiver<Projection>>,
    streams: BTreeMap<Perspective, broadcast::Sender<ServerMessage>>,
    seats: BTreeMap<SeatId, watch::Receiver<SeatState>>,
    status: watch::Receiver<CampaignStatus>,
    reader: ReplayReader,
    thread: Mutex<Option<std::thread::JoinHandle<Result<(), Error>>>>,
}
#[derive(Clone)]
pub struct CampaignHandle {
    inner: Arc<Inner>,
}
struct Publisher {
    projections: BTreeMap<Perspective, watch::Sender<Projection>>,
    streams: BTreeMap<Perspective, broadcast::Sender<ServerMessage>>,
    seats: BTreeMap<SeatId, watch::Sender<SeatState>>,
    status: watch::Sender<CampaignStatus>,
    transcript_cursors: BTreeMap<(Perspective, SeatId), u64>,
}
fn seat_state<R: Ruleset>(campaign: &Campaign<R>, seat: SeatId) -> SeatState {
    SeatState {
        pending: campaign
            .pending()
            .into_iter()
            .filter(|d| d.seat == seat)
            .collect(),
        observation: campaign.observe(seat),
        binding: campaign.binding(seat).clone(),
    }
}
fn projection<R: Ruleset>(campaign: &Campaign<R>, p: Perspective) -> Result<Projection, Error> {
    Ok(Projection {
        meta: campaign.metadata(p),
        seq: campaign.current_seq(p)?,
        view: campaign.view(p)?,
    })
}
impl Publisher {
    fn update<R: Ruleset>(&mut self, campaign: &Campaign<R>) -> Result<(), Error> {
        for p in Perspective::all() {
            let previous_seq = self.projections[&p].borrow().seq;
            let next = projection(campaign, p)?;
            let end = next.seq;
            self.projections[&p].send_if_modified(|current| {
                if *current == next {
                    false
                } else {
                    *current = next;
                    true
                }
            });
            let mut from = previous_seq;
            while from < end {
                let events = campaign.events_after(p, from, 512)?;
                if events.is_empty() {
                    return Err(Error::Recovery("missing committed stream row".into()));
                }
                for message in events {
                    if let ServerMessage::Event { seq, .. } = &message {
                        from = *seq;
                    }
                    // Sending is immediate even when nobody reads or every reader is lagged.
                    let _ = self.streams[&p].send(message);
                }
            }
        }
        // Both CLI ingestion and atomic scripted entries use the same persistent fanout.
        // The writer never waits for viewers; each perspective retains its own seat cursors.
        for ((perspective, seat), cursor) in &mut self.transcript_cursors {
            loop {
                let rows = campaign.transcripts_after(*perspective, *seat, *cursor, 512)?;
                let done = rows.len() < 512;
                for message in rows {
                    if let ServerMessage::Transcript { tseq, .. } = &message {
                        *cursor = *tseq;
                    }
                    let _ = self.streams[perspective].send(message);
                }
                if done {
                    break;
                }
            }
        }
        for seat in SeatId::all() {
            let next = seat_state(campaign, seat);
            self.seats[&seat].send_if_modified(|current| {
                if *current == next {
                    false
                } else {
                    *current = next;
                    true
                }
            });
        }
        self.status.send_if_modified(|current| {
            if current == campaign.status() {
                false
            } else {
                *current = campaign.status().clone();
                true
            }
        });
        Ok(())
    }
}

impl CampaignHandle {
    pub fn spawn<R>(
        campaign: Campaign<R>,
        path: &Path,
        baseline: Option<Baseline<R>>,
    ) -> Result<Self, Error>
    where
        R: Ruleset + Send + 'static,
        R::State: Send,
        R::Content: Send,
    {
        Self::spawn_with_candidates(campaign, path, baseline, Box::new(NoCandidates))
    }
    pub fn spawn_with_candidates<R>(
        campaign: Campaign<R>,
        path: &Path,
        baseline: Option<Baseline<R>>,
        candidates: Box<dyn Candidates + Send>,
    ) -> Result<Self, Error>
    where
        R: Ruleset + Send + 'static,
        R::State: Send,
        R::Content: Send,
    {
        let (tx, mut rx) = mpsc::channel::<Envelope>(COMMAND_BUFFER);
        let mut projections = BTreeMap::new();
        let mut projection_senders = BTreeMap::new();
        let mut streams = BTreeMap::new();
        let mut seats = BTreeMap::new();
        let mut seat_senders = BTreeMap::new();
        for p in Perspective::all() {
            let (send, recv) = watch::channel(projection(&campaign, p)?);
            projections.insert(p, recv);
            projection_senders.insert(p, send);
            streams.insert(p, broadcast::channel(VIEWER_BUFFER).0);
        }
        for seat in SeatId::all() {
            let (send, recv) = watch::channel(seat_state(&campaign, seat));
            seats.insert(seat, recv);
            seat_senders.insert(seat, send);
        }
        let (status_send, status_recv) = watch::channel(campaign.status().clone());
        let mut transcript_cursors = BTreeMap::new();
        for seat in SeatId::all() {
            let current = campaign.transcript_seq(seat)?;
            for p in Perspective::all().filter(|p| p.can_see(&Audience::Seat(seat))) {
                transcript_cursors.insert((p, seat), current);
            }
        }
        let mut publisher = Publisher {
            projections: projection_senders,
            streams: streams.clone(),
            seats: seat_senders,
            status: status_send,
            transcript_cursors,
        };
        let thread = std::thread::Builder::new()
            .name("campaign-writer".into())
            .spawn(move || {
                let mut campaign = campaign;
                let result = (|| {
                    loop {
                        match rx.try_recv() {
                            Ok(envelope) => {
                                if matches!(envelope.op, Op::Shutdown) {
                                    let _ = envelope.reply.send(Ok(Value::Null));
                                    return Ok(());
                                }
                                dispatch_and_publish(&mut campaign, envelope, &mut publisher)?;
                            }
                            Err(mpsc::error::TryRecvError::Disconnected) => return Ok(()),
                            Err(mpsc::error::TryRecvError::Empty) => {}
                        }
                        let step = auto_step(&mut campaign, baseline.as_ref(), candidates.as_ref());
                        match step {
                            Ok(Step::Idle) => {
                                // An idle writer waits for control or a seat submission without polling.
                                publisher.update(&campaign)?;
                                let Some(envelope) = rx.blocking_recv() else {
                                    return Ok(());
                                };
                                if matches!(envelope.op, Op::Shutdown) {
                                    let _ = envelope.reply.send(Ok(Value::Null));
                                    return Ok(());
                                }
                                dispatch_and_publish(&mut campaign, envelope, &mut publisher)?;
                            }
                            Err(_error)
                                if matches!(campaign.status(), CampaignStatus::Stopped { .. }) =>
                            {
                                publisher.update(&campaign)?;
                            }
                            Err(error) => return Err(error),
                            Ok(_) => publisher.update(&campaign)?,
                        }
                    }
                })();
                if let Err(error) = &result {
                    tracing::error!(%error, "campaign writer stopped");
                    if let Err(storage_error) = campaign.stop_runtime(error) {
                        tracing::error!(%storage_error, "could not persist campaign writer stop");
                    }
                    // Publishing may itself be the failing operation. Status remains visible
                    // through its separate watch even when a projection cannot be read.
                    let _ = publisher.update(&campaign);
                    publisher.status.send_replace(campaign.status().clone());
                }
                result
            })
            .map_err(|e| Error::Invalid(format!("cannot start campaign writer: {e}")))?;
        Ok(Self {
            inner: Arc::new(Inner {
                tx,
                projections,
                streams,
                seats,
                status: status_recv,
                reader: ReplayReader::new(path),
                thread: Mutex::new(Some(thread)),
            }),
        })
    }
    async fn call<T: DeserializeOwned>(&self, op: Op) -> Result<T, Error> {
        let (reply, wait) = oneshot::channel();
        self.inner
            .tx
            .send(Envelope { op, reply })
            .await
            .map_err(|_| Error::NotRunning)?;
        let result = wait.await.map_err(|_| Error::NotRunning)??;
        Ok(serde_json::from_value(result)?)
    }
    pub fn projection(&self, p: Perspective) -> Projection {
        self.inner.projections[&p].borrow().clone()
    }
    pub fn seat(&self, seat: SeatId) -> SeatState {
        self.inner.seats[&seat].borrow().clone()
    }
    /// Changes are scoped to this seat, including handover reissues with a new epoch.
    pub fn watch_seat(&self, seat: SeatId) -> watch::Receiver<SeatState> {
        self.inner.seats[&seat].clone()
    }
    pub fn status(&self) -> CampaignStatus {
        self.inner.status.borrow().clone()
    }
    pub fn watch_status(&self) -> watch::Receiver<CampaignStatus> {
        self.inner.status.clone()
    }
    pub fn subscribe(&self, p: Perspective) -> broadcast::Receiver<ServerMessage> {
        self.inner.streams[&p].subscribe()
    }
    pub fn replay(&self) -> ReplayReader {
        self.inner.reader.clone()
    }
    pub async fn pause(&self, paused: bool) -> Result<(), Error> {
        self.call(Op::Pause(paused)).await
    }
    pub async fn handover(
        &self,
        seat: SeatId,
        controller: Option<ControllerInfo>,
        config: Value,
    ) -> Result<Binding, Error> {
        self.call(Op::Handover(seat, controller, config)).await
    }
    pub async fn pause_seat(&self, seat: SeatId) -> Result<(), Error> {
        self.call(Op::PauseSeat(seat, None, "paused by operator".into()))
            .await
    }
    /// Unconditional administrative pause. Controller callbacks must use
    /// `mark_failure_if_epoch` so a superseded controller cannot pause its replacement.
    pub async fn mark_failure(&self, seat: SeatId, reason: &str) -> Result<(), Error> {
        self.call(Op::PauseSeat(seat, None, reason.into())).await
    }
    /// Compare the controller epoch and apply its failure pause on the single writer.
    /// Stale callbacks leave the replacement binding and transcript history unchanged.
    pub async fn mark_failure_if_epoch(
        &self,
        seat: SeatId,
        expected_epoch: u64,
        reason: &str,
    ) -> Result<(), Error> {
        self.call(Op::PauseSeat(seat, Some(expected_epoch), reason.into()))
            .await
    }
    pub async fn submit(&self, response: DecisionResponse) -> Result<Receipt, Error> {
        self.call(Op::Submit(response)).await
    }
    pub async fn submit_action(
        &self,
        seat: SeatId,
        request: SubmitRequest,
    ) -> Result<Receipt, Error> {
        self.call(Op::SubmitAction(seat, request)).await
    }
    pub async fn validate_action(
        &self,
        seat: SeatId,
        decision: &str,
        action: Value,
    ) -> Result<(), Error> {
        self.call(Op::Validate(seat, decision.into(), action)).await
    }
    pub async fn inspect(&self, seat: SeatId, target: &str) -> Result<Value, Error> {
        self.call(Op::Inspect(seat, target.into())).await
    }
    pub async fn transcript(
        &self,
        seat: SeatId,
        at: String,
        entry: TranscriptEntry,
    ) -> Result<u64, Error> {
        self.call(Op::Transcript(seat, at, entry)).await
    }
    pub async fn notebook(&self, seat: SeatId) -> Result<String, Error> {
        self.call(Op::Notebook(seat)).await
    }
    pub async fn write_notebook(
        &self,
        seat: SeatId,
        mode: WriteMode,
        text: &str,
    ) -> Result<usize, Error> {
        self.call(Op::WriteNotebook(seat, mode, text.into())).await
    }
    pub async fn message_team(&self, seat: SeatId, text: &str) -> Result<usize, Error> {
        self.call(Op::Message(seat, text.into())).await
    }
    pub async fn messages(
        &self,
        seat: SeatId,
        after: u64,
    ) -> Result<Vec<cna_seats::memory::TeamMessage>, Error> {
        self.call(Op::Messages(seat, after)).await
    }
    pub async fn shutdown(&self) -> Result<(), Error> {
        let result: Result<(), Error> = self.call(Op::Shutdown).await;
        let thread = self
            .inner
            .thread
            .lock()
            .map_err(|_| Error::NotRunning)?
            .take();
        if let Some(thread) = thread {
            tokio::task::spawn_blocking(move || thread.join())
                .await
                .map_err(|_| Error::NotRunning)?
                .map_err(|_| Error::Invalid("campaign writer panicked".into()))??;
        }
        result
    }
}
fn auto_step<R: Ruleset>(
    campaign: &mut Campaign<R>,
    baseline: Option<&Baseline<R>>,
    candidates: &dyn Candidates,
) -> Result<Step, Error> {
    if campaign.status() != &CampaignStatus::Running {
        return Ok(Step::Idle);
    }
    let first_scripted = campaign.pending().into_iter().find(|d| {
        !campaign.binding(d.seat).paused
            && campaign
                .binding(d.seat)
                .controller
                .as_ref()
                .is_some_and(|c| c.kind == cna_protocol::ControllerKind::Scripted)
    });
    if let Some(request) =
        first_scripted.filter(|d| campaign.binding(d.seat).config["mode"] == "aggressive")
    {
        let Some(baseline) = baseline else {
            campaign.pause_seat_with_reason(request.seat, "aggressive baseline unavailable")?;
            return Ok(Step::SeatPaused {
                seat: request.seat,
                error: "aggressive baseline unavailable".into(),
            });
        };
        let epoch = campaign.binding(request.seat).controller_epoch;
        let action = baseline(&campaign.content, &campaign.game.state, &request);
        let response = DecisionResponse {
            decision_id: request.id.clone(),
            seat: request.seat,
            controller_epoch: epoch,
            decision_revision: request.revision,
            idempotency_key: format!("aggressive:{}:{epoch}:{}", request.id, request.revision),
            action,
            public_explanation: None,
        };
        match campaign.submit(response) {
            Ok(receipt) if !receipt.duplicate => return Ok(Step::Responded { seat: request.seat }),
            Ok(_) => {
                let message = "ruleset retained an already-answered request without a new revision";
                campaign.pause_seat_with_reason(request.seat, message)?;
                return Ok(Step::SeatPaused {
                    seat: request.seat,
                    error: message.into(),
                });
            }
            Err(Error::Rejected(Rejection::Illegal { message })) => {
                campaign.pause_seat_with_reason(request.seat, &message)?;
                return Ok(Step::SeatPaused {
                    seat: request.seat,
                    error: message,
                });
            }
            Err(e) => return Err(e),
        }
    }
    campaign.step(candidates)
}
fn serialize<T: Serialize>(value: T) -> Result<Value, Error> {
    Ok(serde_json::to_value(value)?)
}
fn submit<R: Ruleset>(
    campaign: &mut Campaign<R>,
    response: DecisionResponse,
) -> Result<Value, Error> {
    let seat = response.seat;
    match campaign.submit(response) {
        Ok(receipt) => serialize(receipt),
        Err(e @ Error::Rejected(Rejection::Illegal { .. })) => {
            campaign.pause_seat(seat)?;
            Err(e)
        }
        Err(e) => Err(e),
    }
}
fn dispatch_and_publish<R: Ruleset>(
    campaign: &mut Campaign<R>,
    envelope: Envelope,
    publisher: &mut Publisher,
) -> Result<(), Error> {
    let result = dispatch(campaign, envelope.op);
    let publication = publisher.update(campaign);
    let fatal = match &result {
        Err(error @ (Error::Storage(_) | Error::Json(_) | Error::Recovery(_))) => {
            Some(Error::Recovery(error.to_string()))
        }
        _ => None,
    };
    // A committed operation retains its original receipt even if stream publication fails.
    // A retry after recovery therefore resolves to the same durable command, not a new order.
    let _ = envelope.reply.send(result);
    publication?;
    match fatal {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
fn dispatch<R: Ruleset>(campaign: &mut Campaign<R>, op: Op) -> Result<Value, Error> {
    match op {
        Op::Pause(paused) => {
            campaign.set_paused(paused)?;
            Ok(Value::Null)
        }
        Op::Handover(seat, controller, config) => {
            serialize(campaign.handover(seat, controller, config)?)
        }
        Op::PauseSeat(seat, expected_epoch, reason) => {
            if expected_epoch.is_some_and(|epoch| epoch != campaign.binding(seat).controller_epoch)
            {
                return Err(Error::StaleEpoch);
            }
            campaign.pause_seat_with_reason(seat, &reason)?;
            Ok(Value::Null)
        }
        Op::Submit(response) => submit(campaign, response),
        Op::SubmitAction(seat, req) => {
            let revision = match req.revision {
                Some(r) => r,
                None => match campaign.prior_response(seat, &req.idempotency_key)? {
                    Some(r) => r.decision_revision,
                    None => campaign.actions(seat, &req.decision_id)?.revision,
                },
            };
            submit(
                campaign,
                DecisionResponse {
                    decision_id: req.decision_id.as_str().into(),
                    seat,
                    controller_epoch: req.epoch,
                    decision_revision: revision,
                    idempotency_key: req.idempotency_key,
                    action: req.action,
                    public_explanation: req.public_explanation,
                },
            )
        }
        Op::Validate(seat, id, action) => {
            let request = campaign.actions(seat, &id)?;
            campaign.validate(&DecisionResponse {
                decision_id: id.as_str().into(),
                seat,
                controller_epoch: campaign.binding(seat).controller_epoch,
                decision_revision: request.revision,
                idempotency_key: "validate-only".into(),
                action,
                public_explanation: None,
            })?;
            Ok(Value::Null)
        }
        Op::Inspect(seat, target) => campaign.inspect(seat, &target),
        Op::Transcript(seat, at, entry) => Ok(json!(campaign.transcript(seat, &at, entry)?)),
        Op::Notebook(seat) => serialize(campaign.notebook(seat)?),
        Op::WriteNotebook(seat, mode, text) => {
            serialize(campaign.write_notebook(seat, mode, &text)?)
        }
        Op::Message(seat, text) => serialize(campaign.team_message(seat, &text)?),
        Op::Messages(seat, after) => serialize(campaign.read_messages(seat, after)?),
        Op::Shutdown => Ok(Value::Null),
    }
}
