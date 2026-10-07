//! Bounded actor commands and isolated, committed reader projections.
use crate::{
    Binding, Campaign, CampaignStatus, Error, Receipt,
    campaign::RuntimeMetrics,
    replay::ReplayReader,
    scripted::{Candidates, NoCandidates, Step},
};
use cna_core::{
    decision::{DecisionRequest, DecisionResponse},
    engine::{EngineError, Rejection, Ruleset},
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
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
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
/// Eager metadata without materializing a board snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectionHeader {
    pub meta: CampaignMeta,
    pub seq: u64,
}
/// One captured committed root, retained across snapshot or WebSocket awaits.
#[derive(Clone)]
pub struct PerspectiveSnapshot {
    published: Arc<Published>,
    perspective: Perspective,
}
impl PerspectiveSnapshot {
    pub fn header(&self) -> ProjectionHeader {
        self.published.headers[&self.perspective].clone()
    }
    pub fn projection(&self) -> Projection {
        let header = self.header();
        let view = self.published.views[&self.perspective]
            .get_or_init(|| (self.published.projector)(self.perspective));
        Projection {
            meta: header.meta,
            seq: header.seq,
            view: view.clone(),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct SeatState {
    pub pending: Vec<DecisionRequest>,
    /// This seat's contiguous stream sequence, never a global revision.
    pub seq: u64,
    pub binding: Binding,
}

pub type Baseline<R> =
    Box<dyn Fn(&<R as Ruleset>::Content, &<R as Ruleset>::State, &DecisionRequest) -> Value + Send>;
/// Checked aggressive baseline. Existing infallible callers are adapted by `spawn`.
pub type FallibleBaseline<R> = Box<
    dyn Fn(
            &<R as Ruleset>::Content,
            &<R as Ruleset>::State,
            &DecisionRequest,
        ) -> Result<Value, EngineError>
        + Send,
>;

/// None is reserved for unhandled request kinds. A checked owner policy must not
/// replace an engine gap or invariant with None, Null, an empty plan or another candidate.
pub(crate) type ActionPolicy<R> = Box<
    dyn Fn(
            &<R as Ruleset>::Content,
            &<R as Ruleset>::State,
            &DecisionRequest,
            u64,
        ) -> Result<Option<Value>, EngineError>
        + Send,
>;

enum Op {
    Pause(bool),
    RunBoundary,
    SetRunBoundary(Option<crate::RunBoundary>),
    Handover(SeatId, Option<ControllerInfo>, Value),
    PauseSeat(SeatId, Option<u64>, String),
    Submit(DecisionResponse),
    SubmitAction(SeatId, SubmitRequest),
    Validate(SeatId, String, Value, Option<(u64, u32)>),
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
    published: watch::Receiver<Arc<Published>>,
    streams: BTreeMap<Perspective, broadcast::Sender<ServerMessage>>,
    seats: BTreeMap<SeatId, watch::Receiver<SeatState>>,
    status: watch::Receiver<CampaignStatus>,
    reader: ReplayReader,
    metrics: Arc<Mutex<RuntimeMetrics>>,
    supports_aggressive: bool,
    thread: Mutex<Option<std::thread::JoinHandle<Result<(), Error>>>>,
}
#[derive(Clone)]
pub struct CampaignHandle {
    inner: Arc<Inner>,
}
struct Publisher {
    published: watch::Sender<Arc<Published>>,
    streams: BTreeMap<Perspective, broadcast::Sender<ServerMessage>>,
    seats: BTreeMap<SeatId, watch::Sender<SeatState>>,
    status: watch::Sender<CampaignStatus>,
    transcript_cursors: BTreeMap<(Perspective, SeatId), u64>,
}
pub(crate) type Observer = Arc<dyn Fn(SeatId) -> Value + Send + Sync>;
pub(crate) type Projector = Arc<dyn Fn(Perspective) -> ViewState + Send + Sync>;
struct Published {
    headers: BTreeMap<Perspective, ProjectionHeader>,
    views: BTreeMap<Perspective, OnceLock<ViewState>>,
    projector: Projector,
    seats: BTreeMap<SeatId, SeatState>,
    observer: Observer,
}
fn published<R>(campaign: &Campaign<R>) -> Result<Arc<Published>, Error>
where
    R: Ruleset + Send + Sync + 'static,
    R::State: Send + Sync,
    R::Content: Send + Sync,
{
    let pending = campaign.pending();
    let headers: BTreeMap<_, _> = Perspective::all()
        .map(|p| {
            Ok((
                p,
                ProjectionHeader {
                    meta: campaign.metadata(p),
                    seq: campaign.current_seq(p)?,
                },
            ))
        })
        .collect::<Result<_, Error>>()?;
    let projector = campaign.projector(&pending)?;
    let seats = SeatId::all()
        .map(|seat| {
            (
                seat,
                SeatState {
                    pending: pending.iter().filter(|d| d.seat == seat).cloned().collect(),
                    seq: headers[&Perspective::Seat(seat)].seq,
                    binding: campaign.binding(seat).clone(),
                },
            )
        })
        .collect();
    Ok(Arc::new(Published {
        headers,
        views: Perspective::all().map(|p| (p, OnceLock::new())).collect(),
        projector,
        seats,
        observer: campaign.observer(),
    }))
}
impl Publisher {
    fn update<R>(&mut self, campaign: &Campaign<R>) -> Result<(), Error>
    where
        R: Ruleset + Send + Sync + 'static,
        R::State: Send + Sync,
        R::Content: Send + Sync,
    {
        let start = Instant::now();
        let next = published(campaign)?;
        // One atomic root binds headers, pending, bindings and snapshot readers to one commit.
        let previous = self.published.send_replace(Arc::clone(&next));
        for (&p, header) in &next.headers {
            let previous_seq = previous.headers[&p].seq;
            let end = header.seq;
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
            let next = next.seats[&seat].clone();
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
        campaign
            .metrics
            .lock()
            .expect("runtime metrics lock")
            .projections += start.elapsed();
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
        R: Ruleset + Send + Sync + 'static,
        R::State: Send + Sync,
        R::Content: Send + Sync,
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
        R: Ruleset + Send + Sync + 'static,
        R::State: Send + Sync,
        R::Content: Send + Sync,
    {
        let baseline = baseline.map(|baseline| {
            Box::new(
                move |content: &R::Content, state: &R::State, request: &DecisionRequest| {
                    Ok(baseline(content, state, request))
                },
            ) as FallibleBaseline<R>
        });
        Self::spawn_with_controllers(campaign, path, baseline, candidates, None)
    }
    pub fn spawn_with_fallible_baseline<R>(
        campaign: Campaign<R>,
        path: &Path,
        baseline: FallibleBaseline<R>,
    ) -> Result<Self, Error>
    where
        R: Ruleset + Send + Sync + 'static,
        R::State: Send + Sync,
        R::Content: Send + Sync,
    {
        Self::spawn_with_controllers(campaign, path, Some(baseline), Box::new(NoCandidates), None)
    }
    pub(crate) fn spawn_with_policy<R>(
        campaign: Campaign<R>,
        path: &Path,
        policy: ActionPolicy<R>,
    ) -> Result<Self, Error>
    where
        R: Ruleset + Send + Sync + 'static,
        R::State: Send + Sync,
        R::Content: Send + Sync,
    {
        Self::spawn_with_controllers(campaign, path, None, Box::new(NoCandidates), Some(policy))
    }
    fn spawn_with_controllers<R>(
        campaign: Campaign<R>,
        path: &Path,
        baseline: Option<FallibleBaseline<R>>,
        candidates: Box<dyn Candidates + Send>,
        policy: Option<ActionPolicy<R>>,
    ) -> Result<Self, Error>
    where
        R: Ruleset + Send + Sync + 'static,
        R::State: Send + Sync,
        R::Content: Send + Sync,
    {
        let (tx, mut rx) = mpsc::channel::<Envelope>(COMMAND_BUFFER);
        let initial = published(&campaign)?;
        let (published_send, published_recv) = watch::channel(Arc::clone(&initial));
        let mut streams = BTreeMap::new();
        let mut seats = BTreeMap::new();
        let mut seat_senders = BTreeMap::new();
        for p in Perspective::all() {
            streams.insert(p, broadcast::channel(VIEWER_BUFFER).0);
        }
        for seat in SeatId::all() {
            let (send, recv) = watch::channel(initial.seats[&seat].clone());
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
            published: published_send,
            streams: streams.clone(),
            seats: seat_senders,
            status: status_send,
            transcript_cursors,
        };
        let metrics = Arc::clone(&campaign.metrics);
        let supports_aggressive = baseline.is_some();
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
                        let step = auto_step(
                            &mut campaign,
                            baseline.as_ref(),
                            candidates.as_ref(),
                            policy.as_ref(),
                        );
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
                published: published_recv,
                streams,
                seats,
                status: status_recv,
                reader: ReplayReader::new(path),
                metrics,
                supports_aggressive,
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
    /// Operator-process diagnostics only; no HTTP/WS/MCP endpoint exposes these timings.
    pub fn runtime_metrics(&self) -> RuntimeMetrics {
        *self.inner.metrics.lock().expect("runtime metrics lock")
    }
    pub fn header(&self, p: Perspective) -> ProjectionHeader {
        self.inner.published.borrow().headers[&p].clone()
    }
    pub fn snapshot(&self, p: Perspective) -> PerspectiveSnapshot {
        let published = Arc::clone(&self.inner.published.borrow());
        PerspectiveSnapshot {
            published,
            perspective: p,
        }
    }
    pub fn projection(&self, p: Perspective) -> Projection {
        self.snapshot(p).projection()
    }
    pub fn seat(&self, seat: SeatId) -> SeatState {
        self.inner.published.borrow().seats[&seat].clone()
    }
    /// Read outside the writer from one immutable published commit.
    pub fn observation(&self, seat: SeatId) -> Value {
        let snapshot = Arc::clone(&self.inner.published.borrow());
        (snapshot.observer)(seat)
    }
    /// HTTP observation and its pending/epoch metadata must describe the same commit.
    pub fn observation_state(&self, seat: SeatId) -> (Value, SeatState) {
        let snapshot = Arc::clone(&self.inner.published.borrow());
        ((snapshot.observer)(seat), snapshot.seats[&seat].clone())
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
    /// Trusted launcher/operator control; never exposed through the seat MCP bridge.
    pub async fn run_boundary(&self) -> Result<Option<crate::RunBoundary>, Error> {
        self.call(Op::RunBoundary).await
    }
    pub async fn set_run_boundary(
        &self,
        boundary: Option<crate::RunBoundary>,
    ) -> Result<(), Error> {
        self.call(Op::SetRunBoundary(boundary)).await
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
        if !self.inner.supports_aggressive
            && controller
                .as_ref()
                .is_some_and(|c| c.kind == cna_protocol::ControllerKind::Scripted)
            && config["mode"].as_str() == Some("aggressive")
        {
            return Err(Error::Invalid("campaign has no aggressive baseline".into()));
        }
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
        self.call(Op::Validate(seat, decision.into(), action, None))
            .await
    }
    /// Check a caller's binding and decision revision on the writer before pure validation.
    pub async fn validate_action_at(
        &self,
        seat: SeatId,
        decision: &str,
        action: Value,
        controller_epoch: u64,
        decision_revision: u32,
    ) -> Result<(), Error> {
        self.call(Op::Validate(
            seat,
            decision.into(),
            action,
            Some((controller_epoch, decision_revision)),
        ))
        .await
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
pub(crate) fn auto_step<R: Ruleset>(
    campaign: &mut Campaign<R>,
    baseline: Option<&FallibleBaseline<R>>,
    candidates: &dyn Candidates,
    policy: Option<&ActionPolicy<R>>,
) -> Result<Step, Error> {
    let before = *campaign.metrics.lock().expect("runtime metrics lock");
    let start = Instant::now();
    let result = auto_step_inner(campaign, baseline, candidates, policy);
    let elapsed = start.elapsed();
    let mut after = campaign.metrics.lock().expect("runtime metrics lock");
    let commit = (after.engine - before.engine) + (after.durable_writer - before.durable_writer);
    after.controllers += elapsed.saturating_sub(commit);
    result
}
fn auto_step_inner<R: Ruleset>(
    campaign: &mut Campaign<R>,
    baseline: Option<&FallibleBaseline<R>>,
    candidates: &dyn Candidates,
    policy: Option<&ActionPolicy<R>>,
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
    if let Some(request) = first_scripted.as_ref()
        && (campaign.binding(request.seat).config["mode"] == "legal_random"
            || (campaign.binding(request.seat).config["mode"] == "pass_when_possible"
                && request.space.pass.is_none()))
        && let Some(policy) = policy
    {
        // None declines this request kind; Err must stop before generic generation or a
        // declared pass can substitute an answer. The writer serializes the durable stop.
        let result = policy(
            &campaign.content,
            &campaign.game.state,
            request,
            campaign.binding(request.seat).controller_epoch,
        );
        let action = checked_policy_result(campaign, result)?;
        if let Some(action) = action {
            let label = if campaign.binding(request.seat).config["mode"] == "pass_when_possible" {
                "scripted:pass_when_possible"
            } else {
                "scripted:legal_random"
            };
            return apply_baseline(campaign, request, action, label);
        }
    }
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
        let result = baseline(&campaign.content, &campaign.game.state, &request);
        let action = checked_policy_result(campaign, result)?;
        return apply_baseline(campaign, &request, action, "aggressive");
    }
    campaign.step(candidates)
}
fn checked_policy_result<R: Ruleset, T>(
    campaign: &mut Campaign<R>,
    result: Result<T, EngineError>,
) -> Result<T, Error> {
    match result {
        Ok(action) => Ok(action),
        Err(error) => {
            campaign.stop_engine(&error)?;
            Err(Rejection::Engine(error).into())
        }
    }
}
fn apply_baseline<R: Ruleset>(
    campaign: &mut Campaign<R>,
    request: &DecisionRequest,
    action: Value,
    label: &str,
) -> Result<Step, Error> {
    if action.is_null() && request.space.pass.is_none() {
        let message = format!(
            "{label} returned no allocation for mandatory decision {}; explicit owner allocation required",
            request.kind
        );
        campaign.pause_seat_with_reason(request.seat, &message)?;
        return Ok(Step::SeatPaused {
            seat: request.seat,
            error: message,
        });
    }
    let epoch = campaign.binding(request.seat).controller_epoch;
    let response = DecisionResponse {
        decision_id: request.id.clone(),
        seat: request.seat,
        controller_epoch: epoch,
        decision_revision: request.revision,
        idempotency_key: format!("{label}:{}:{epoch}:{}", request.id, request.revision),
        action,
        public_explanation: None,
    };
    match campaign.submit(response) {
        Ok(receipt) if !receipt.duplicate => Ok(Step::Responded { seat: request.seat }),
        Ok(_) => {
            let message = "ruleset retained an already-answered request without a new revision";
            campaign.pause_seat_with_reason(request.seat, message)?;
            Ok(Step::SeatPaused {
                seat: request.seat,
                error: message.into(),
            })
        }
        Err(Error::Rejected(Rejection::Illegal { message })) => {
            campaign.pause_seat_with_reason(request.seat, &message)?;
            Ok(Step::SeatPaused {
                seat: request.seat,
                error: message,
            })
        }
        Err(e) => Err(e),
    }
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
fn dispatch_and_publish<R>(
    campaign: &mut Campaign<R>,
    envelope: Envelope,
    publisher: &mut Publisher,
) -> Result<(), Error>
where
    R: Ruleset + Send + Sync + 'static,
    R::State: Send + Sync,
    R::Content: Send + Sync,
{
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
        Op::RunBoundary => serialize(campaign.run_boundary()),
        Op::SetRunBoundary(boundary) => {
            campaign.set_run_boundary(boundary)?;
            Ok(Value::Null)
        }
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
        Op::Validate(seat, id, action, expected) => {
            let (controller_epoch, decision_revision) = match expected {
                Some(expected) => expected,
                None => (
                    campaign.binding(seat).controller_epoch,
                    campaign.actions(seat, &id)?.revision,
                ),
            };
            campaign.validate(&DecisionResponse {
                decision_id: id.as_str().into(),
                seat,
                controller_epoch,
                decision_revision,
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

#[cfg(test)]
#[path = "actor_policy_tests.rs"]
mod policy_tests;
