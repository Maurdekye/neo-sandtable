//! Local-only HTTP API and bounded WebSocket protocol implementation.
use crate::{CampaignStatus, Error, actor::CampaignHandle, seats::seat_error};
use axum::{
    Json, Router,
    extract::{
        Path as RoutePath, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cna_core::{decision::DecisionResponse, ids::SeatId, visibility::Perspective};
use cna_protocol::{ClientMessage, ControllerInfo, PROTOCOL_VERSION, ServerMessage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::broadcast;
use tower_http::{
    cors::CorsLayer,
    services::{ServeDir, ServeFile},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateRequest {
    pub rules_profile: String,
    pub seed: [u8; 32],
    #[serde(default = "default_title")]
    pub title: String,
    #[serde(default)]
    pub paused: bool,
    #[serde(default = "default_mode")]
    pub controller: String,
}
fn default_title() -> String {
    "Synthetic sandbox".into()
}
fn default_mode() -> String {
    "legal_random".into()
}
pub type Factory = Arc<dyn Fn(CreateRequest, &Path) -> Result<CampaignHandle, Error> + Send + Sync>;
#[derive(Clone)]
pub struct App {
    campaigns: Arc<RwLock<BTreeMap<String, CampaignHandle>>>,
    directory: PathBuf,
    factory: Factory,
    port: u16,
}
impl App {
    pub fn new(directory: PathBuf, port: u16, factory: Factory) -> Self {
        Self {
            campaigns: Arc::new(RwLock::new(BTreeMap::new())),
            directory,
            factory,
            port,
        }
    }
    pub fn register(&self, campaign: CampaignHandle) {
        let id = campaign.projection(Perspective::Operator).meta.id;
        self.campaigns
            .write()
            .expect("campaign registry")
            .insert(id, campaign);
    }
    fn campaign(&self, id: &str) -> Result<CampaignHandle, ApiError> {
        self.campaigns
            .read()
            .map_err(|_| {
                ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "campaign registry unavailable".into(),
                )
            })?
            .get(id)
            .cloned()
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "unknown campaign".into()))
    }
    pub async fn shutdown(&self) {
        let handles: Vec<_> = self
            .campaigns
            .read()
            .expect("campaign registry")
            .values()
            .cloned()
            .collect();
        for handle in handles {
            let _ = handle.shutdown().await;
        }
    }
    pub fn router(&self, dist: &Path) -> Router {
        let cors = CorsLayer::new()
            .allow_origin([
                format!("http://127.0.0.1:{}", self.port)
                    .parse()
                    .expect("origin"),
                format!("http://localhost:{}", self.port)
                    .parse()
                    .expect("origin"),
                "http://127.0.0.1:5173".parse().expect("origin"),
                "http://localhost:5173".parse().expect("origin"),
            ])
            .allow_methods([Method::GET, Method::POST])
            .allow_headers([axum::http::header::CONTENT_TYPE]);
        Router::new()
            .route("/api/campaigns", get(list).post(create))
            .route("/api/campaigns/{id}", get(inspect_campaign))
            .route("/api/campaigns/{id}/pause", post(pause))
            .route("/api/campaigns/{id}/resume", post(resume))
            .route("/api/campaigns/{id}/seats", get(seats))
            .route(
                "/api/campaigns/{id}/seats/{seat}/controller",
                post(handover),
            )
            .route("/api/campaigns/{id}/seats/{seat}/pause", post(pause_seat))
            .route("/api/campaigns/{id}/seats/{seat}/observe", get(observe))
            .route(
                "/api/campaigns/{id}/seats/{seat}/inspect/{target}",
                get(inspect_target),
            )
            .route(
                "/api/campaigns/{id}/seats/{seat}/decisions/{decision}/actions",
                get(actions),
            )
            .route(
                "/api/campaigns/{id}/seats/{seat}/decisions/{decision}/validate",
                post(validate),
            )
            .route(
                "/api/campaigns/{id}/seats/{seat}/decisions/{decision}/submit",
                post(submit),
            )
            .route("/api/campaigns/{id}/transcripts", get(transcripts))
            .route("/api/campaigns/{id}/stream", get(upgrade))
            .fallback_service(
                ServeDir::new(dist).not_found_service(ServeFile::new(dist.join("index.html"))),
            )
            .layer(cors)
            .layer(middleware::from_fn_with_state(self.clone(), local_requests))
            .with_state(self.clone())
    }
}
struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        Self(StatusCode::BAD_REQUEST, seat_error(e).to_string())
    }
}
fn seat(text: &str) -> Result<SeatId, ApiError> {
    text.parse()
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid seat".into()))
}
#[derive(Default, Deserialize)]
struct PerspectiveQuery {
    perspective: Option<String>,
}
fn perspective(query: &PerspectiveQuery) -> Result<Perspective, ApiError> {
    query
        .perspective
        .as_deref()
        .unwrap_or("operator")
        .parse()
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid perspective".into()))
}
async fn local_requests(
    State(app): State<App>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let host = request
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if ![
        format!("127.0.0.1:{}", app.port),
        format!("localhost:{}", app.port),
        format!("[::1]:{}", app.port),
    ]
    .contains(&host.to_owned())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(origin) = request.headers().get("origin") {
        let origin = origin.to_str().unwrap_or("");
        if ![
            format!("http://127.0.0.1:{}", app.port),
            format!("http://localhost:{}", app.port),
            "http://127.0.0.1:5173".into(),
            "http://localhost:5173".into(),
        ]
        .contains(&origin.to_owned())
        {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    next.run(request).await
}
async fn list(
    State(app): State<App>,
    Query(query): Query<PerspectiveQuery>,
) -> Result<Json<Value>, ApiError> {
    let p = perspective(&query)?;
    let campaigns: Vec<_> = app
        .campaigns
        .read()
        .map_err(|_| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "registry unavailable".into(),
            )
        })?
        .values()
        .map(|h| h.projection(p).meta)
        .collect();
    Ok(Json(json!(campaigns)))
}
async fn create(
    State(app): State<App>,
    Json(request): Json<CreateRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let factory = app.factory.clone();
    let dir = app.directory.clone();
    let handle = tokio::task::spawn_blocking(move || factory(request, &dir))
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "creation unavailable".into(),
            )
        })??;
    let meta = handle.projection(Perspective::Operator).meta;
    app.register(handle);
    Ok((StatusCode::CREATED, Json(json!(meta))))
}
async fn inspect_campaign(
    State(app): State<App>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<PerspectiveQuery>,
) -> Result<Json<Value>, ApiError> {
    let p = perspective(&query)?;
    let handle = app.campaign(&id)?;
    let status = match handle.status() {
        CampaignStatus::Stopped { error: _ } if p != Perspective::Operator => {
            json!({"state":"stopped"})
        }
        other => json!(other),
    };
    Ok(Json(
        json!({"campaign":handle.projection(p).meta,"status":status,"snapshot":handle.projection(p)}),
    ))
}
async fn pause(
    State(app): State<App>,
    RoutePath(id): RoutePath<String>,
) -> Result<Json<Value>, ApiError> {
    app.campaign(&id)?.pause(true).await?;
    Ok(Json(json!({"paused":true})))
}
async fn resume(
    State(app): State<App>,
    RoutePath(id): RoutePath<String>,
) -> Result<Json<Value>, ApiError> {
    app.campaign(&id)?.pause(false).await?;
    Ok(Json(json!({"paused":false})))
}
async fn seats(
    State(app): State<App>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<PerspectiveQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        app.campaign(&id)?
            .projection(perspective(&query)?)
            .meta
            .seats
    )))
}
#[derive(Deserialize)]
struct Handover {
    controller: Option<ControllerInfo>,
    #[serde(default)]
    config: Value,
}
async fn handover(
    State(app): State<App>,
    RoutePath((id, s)): RoutePath<(String, String)>,
    Json(request): Json<Handover>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        app.campaign(&id)?
            .handover(seat(&s)?, request.controller, request.config)
            .await?
    )))
}
async fn pause_seat(
    State(app): State<App>,
    RoutePath((id, s)): RoutePath<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    app.campaign(&id)?.pause_seat(seat(&s)?).await?;
    Ok(Json(json!({"paused":true})))
}
async fn observe(
    State(app): State<App>,
    RoutePath((id, s)): RoutePath<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let state = app.campaign(&id)?.seat(seat(&s)?);
    Ok(Json(
        json!({"observation":state.observation,"pending":state.pending,"controller_epoch":state.binding.controller_epoch,"paused":state.binding.paused,"failure":state.binding.failure}),
    ))
}
async fn inspect_target(
    State(app): State<App>,
    RoutePath((id, s, target)): RoutePath<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(app.campaign(&id)?.inspect(seat(&s)?, &target).await?))
}
async fn actions(
    State(app): State<App>,
    RoutePath((id, s, decision)): RoutePath<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    let request = app
        .campaign(&id)?
        .seat(seat(&s)?)
        .pending
        .into_iter()
        .find(|d| d.id.as_str() == decision)
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                "unknown decision for this seat".into(),
            )
        })?;
    Ok(Json(
        json!({"request":request,"action_schema":request.space.to_json_schema()}),
    ))
}
#[derive(Deserialize)]
struct Draft {
    action: Value,
}
async fn validate(
    State(app): State<App>,
    RoutePath((id, s, decision)): RoutePath<(String, String, String)>,
    Json(draft): Json<Draft>,
) -> Result<Json<Value>, ApiError> {
    app.campaign(&id)?
        .validate_action(seat(&s)?, &decision, draft.action)
        .await?;
    Ok(Json(json!({"valid":true})))
}
async fn submit(
    State(app): State<App>,
    RoutePath((id, s, decision)): RoutePath<(String, String, String)>,
    Json(response): Json<DecisionResponse>,
) -> Result<Json<Value>, ApiError> {
    if response.seat != seat(&s)? || response.decision_id.as_str() != decision {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "response does not match endpoint".into(),
        ));
    }
    Ok(Json(json!(app.campaign(&id)?.submit(response).await?)))
}
#[derive(Deserialize)]
struct TranscriptQuery {
    perspective: String,
    seat: String,
    #[serde(default)]
    after: u64,
}
async fn transcripts(
    State(app): State<App>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<TranscriptQuery>,
) -> Result<Json<Value>, ApiError> {
    let p: Perspective = query
        .perspective
        .parse()
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid perspective".into()))?;
    let s = seat(&query.seat)?;
    let reader = app.campaign(&id)?.replay();
    let rows = tokio::task::spawn_blocking(move || reader.transcripts(p, s, query.after))
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "replay unavailable".into()))??;
    Ok(Json(json!(rows)))
}
async fn upgrade(
    State(app): State<App>,
    RoutePath(id): RoutePath<String>,
    ws: WebSocketUpgrade,
    _headers: HeaderMap,
) -> Result<Response, ApiError> {
    let handle = app.campaign(&id)?;
    Ok(ws
        .max_message_size(16384)
        .max_frame_size(16384)
        .max_write_buffer_size(2 * 1024 * 1024)
        .on_upgrade(move |socket| stream(socket, handle)))
}
async fn send(socket: &mut WebSocket, message: &ServerMessage) -> Result<(), ()> {
    let text = serde_json::to_string(message).map_err(|_| ())?;
    tokio::time::timeout(
        Duration::from_secs(5),
        socket.send(Message::Text(text.into())),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}
async fn subscribe(socket: &mut WebSocket) -> Option<(Perspective, Option<u64>)> {
    loop {
        let message = socket.recv().await?.ok()?;
        match message {
            Message::Text(text) => {
                let ClientMessage::Subscribe {
                    perspective,
                    from_seq,
                } = serde_json::from_str(&text).ok()?;
                return Some((perspective.parse().ok()?, from_seq));
            }
            Message::Ping(_) | Message::Pong(_) => {}
            _ => return None,
        }
    }
}
async fn stream(mut socket: WebSocket, handle: CampaignHandle) {
    let mut lifecycle = handle.watch_status();
    let mut pending_subscription = None;
    'subscriptions: loop {
        let Some((p, from)) = (match pending_subscription.take() {
            Some(subscription) => Some(subscription),
            None => {
                tokio::select! { result = tokio::time::timeout(Duration::from_secs(30), subscribe(&mut socket)) => result.ok().flatten(), _ = lifecycle.wait_for(|_| false) => None }
            }
        }) else {
            return;
        };
        // Register first, then read one immutable seq+view projection: no snapshot/live race.
        let mut live = handle.subscribe(p);
        let projection = handle.projection(p);
        let target = projection.seq;
        if send(
            &mut socket,
            &ServerMessage::Hello {
                protocol: PROTOCOL_VERSION,
                campaign: projection.meta,
                perspective: p.to_string(),
            },
        )
        .await
        .is_err()
        {
            return;
        }
        let mut last = from.unwrap_or(target);
        if last > target {
            if send(&mut socket, &ServerMessage::Resync).await.is_err() {
                return;
            }
            continue;
        }
        if from.is_none() {
            if send(
                &mut socket,
                &ServerMessage::Snapshot {
                    seq: target,
                    view: projection.view,
                },
            )
            .await
            .is_err()
            {
                return;
            }
        } else {
            while last < target {
                let reader = handle.replay();
                let cursor = last;
                let Ok(Ok(rows)) =
                    tokio::task::spawn_blocking(move || reader.events(p, cursor, target)).await
                else {
                    return;
                };
                if rows.is_empty() {
                    let _ = send(&mut socket, &ServerMessage::Resync).await;
                    continue 'subscriptions;
                }
                for message in rows {
                    let ServerMessage::Event { seq, .. } = &message else {
                        return;
                    };
                    if *seq != last + 1 {
                        let _ = send(&mut socket, &ServerMessage::Resync).await;
                        continue 'subscriptions;
                    }
                    last = *seq;
                    if send(&mut socket, &message).await.is_err() {
                        return;
                    }
                }
            }
        }
        let mut transcript_cursors: BTreeMap<String, u64> = BTreeMap::new();
        // Histories are paged off the campaign writer. Replay duplicates on reconnect are
        // identified by (seat,tseq), not a global capture counter.
        for s in SeatId::all() {
            let reader = handle.replay();
            let Ok(Ok(through)) =
                tokio::task::spawn_blocking(move || reader.transcript_seq(p, s)).await
            else {
                return;
            };
            let mut cursor = 0;
            loop {
                let reader = handle.replay();
                let Ok(Ok(rows)) = tokio::task::spawn_blocking(move || {
                    reader.transcripts_through(p, s, cursor, through)
                })
                .await
                else {
                    return;
                };
                let done = rows.len() < 512;
                for message in rows {
                    let ServerMessage::Transcript { tseq, .. } = &message else {
                        return;
                    };
                    cursor = *tseq;
                    if send(&mut socket, &message).await.is_err() {
                        return;
                    }
                }
                if done {
                    break;
                }
            }
            transcript_cursors.insert(s.to_string(), cursor);
        }
        loop {
            tokio::select! {
                changed=lifecycle.changed() => { if changed.is_err() { return; } },
                incoming=socket.recv() => {
                    match incoming {
                        Some(Ok(Message::Text(text))) => {
                            let Ok(ClientMessage::Subscribe {perspective,from_seq})=serde_json::from_str(&text) else {return;};
                            let Ok(next_p) = perspective.parse::<Perspective>() else { return; };
                            pending_subscription = Some((next_p, from_seq));
                            continue 'subscriptions;
                        }
                        Some(Ok(Message::Ping(_)|Message::Pong(_))) => {},
                        _ => return,
                    }
                }
                message=live.recv() => {
                    match message {
                        Ok(message) => {
                            match &message {
                                ServerMessage::Event {seq,..} => {
                                    if *seq <= last { continue; }
                                    if *seq != last + 1 { let _=send(&mut socket,&ServerMessage::Resync).await; continue 'subscriptions; }
                                    last = *seq;
                                },
                                ServerMessage::Transcript {seat,tseq,..} => {
                                    let previous=transcript_cursors.entry(seat.clone()).or_default();
                                    if *tseq <= *previous { continue; }
                                    if *tseq != *previous + 1 { let _=send(&mut socket,&ServerMessage::Resync).await; continue 'subscriptions; }
                                    *previous = *tseq;
                                },
                                _ => {},
                            }
                            if send(&mut socket,&message).await.is_err() {return;}
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {if send(&mut socket,&ServerMessage::Resync).await.is_err() {return;} continue 'subscriptions;},
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        }
    }
}
