//! Local-only HTTP API and bounded WebSocket protocol implementation.
use crate::{
    CampaignStatus, Error,
    actor::CampaignHandle,
    auth::{Capabilities, Grant},
    seats::seat_error,
};
use axum::{
    Extension, Json, Router,
    extract::{
        Path as RoutePath, Query, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket, close_code},
    },
    http::{Method, StatusCode},
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignKind {
    #[default]
    Sandbox,
    Cna,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateRequest {
    #[serde(default)]
    pub kind: CampaignKind,
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
    "Campaign".into()
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
    capabilities: Arc<Capabilities>,
}
impl App {
    pub fn new(directory: PathBuf, port: u16, factory: Factory) -> Self {
        Self {
            campaigns: Arc::new(RwLock::new(BTreeMap::new())),
            directory,
            factory,
            port,
            capabilities: Arc::new(Capabilities::new()),
        }
    }
    pub fn register(&self, campaign: CampaignHandle) {
        let id = campaign.header(Perspective::Operator).meta.id;
        self.capabilities.register(&id);
        self.campaigns
            .write()
            .expect("campaign registry")
            .insert(id, campaign);
    }
    pub fn operator_token(&self) -> String {
        self.capabilities.operator_token()
    }
    pub fn side_token(&self, id: &str, side: cna_protocol::Side) -> Option<String> {
        self.capabilities
            .campaign_tokens(id)?
            .sides
            .get(&side)
            .cloned()
    }
    pub fn seat_token(&self, id: &str, seat: SeatId) -> Option<String> {
        self.capabilities
            .campaign_tokens(id)?
            .seats
            .get(&seat)
            .cloned()
    }
    /// Trusted launcher export only. Never give this path or operator token to a seat process.
    pub fn write_credentials(&self, path: &Path) -> Result<(), Error> {
        self.capabilities
            .write_credentials(path)
            .map_err(|e| Error::Invalid(e.to_string()))
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
            .allow_headers([
                axum::http::header::CONTENT_TYPE,
                axum::http::header::AUTHORIZATION,
            ]);
        Router::new()
            .route("/api/session", get(session))
            .route("/api/campaigns/{id}/capabilities", get(capabilities))
            .route("/api/campaigns", get(list).post(create))
            .route("/api/campaigns/{id}", get(inspect_campaign))
            .route(
                "/api/campaigns/{id}/run-boundary",
                get(run_boundary).post(set_run_boundary),
            )
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
    let mut request = request;
    if request.method() != Method::OPTIONS
        && (request.uri().path() == "/api" || request.uri().path().starts_with("/api/"))
    {
        let header = match request.headers().get(axum::http::header::AUTHORIZATION) {
            None => None,
            Some(raw) => match raw.to_str().ok().and_then(|h| h.strip_prefix("Bearer ")) {
                Some(token) => Some(token),
                None => return StatusCode::UNAUTHORIZED.into_response(),
            },
        };
        let query = if request.uri().path().ends_with("/stream") {
            match Query::<CapabilityQuery>::try_from_uri(request.uri()) {
                Ok(query) => query.0.cap,
                Err(_) => return StatusCode::BAD_REQUEST.into_response(),
            }
        } else {
            None
        };
        let token = match (header, query.as_deref()) {
            (Some(a), Some(b)) if a != b => return StatusCode::UNAUTHORIZED.into_response(),
            (Some(a), _) => Some(a),
            (None, b) => b,
        };
        let Some(grant) = token.and_then(|token| app.capabilities.authenticate(token)) else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        request.extensions_mut().insert(grant);
    }
    let api = request.uri().path() == "/api" || request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        axum::http::header::REFERRER_POLICY,
        "no-referrer".parse().expect("header"),
    );
    if api {
        response.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            "no-store".parse().expect("header"),
        );
    }
    response
}
#[derive(Deserialize)]
struct CapabilityQuery {
    cap: Option<String>,
}
fn authorized(ok: bool) -> Result<(), ApiError> {
    if ok {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::FORBIDDEN,
            "capability scope denied".into(),
        ))
    }
}
async fn session(Extension(grant): Extension<Grant>) -> Json<Value> {
    Json(
        json!({"perspective":grant.perspective.to_string(),"campaign_id":grant.campaign_id,"operator":grant.operator()}),
    )
}
async fn capabilities(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    RoutePath(id): RoutePath<String>,
) -> Result<Json<Value>, ApiError> {
    authorized(grant.operator())?;
    app.campaign(&id)?;
    Ok(Json(json!(app.capabilities.campaign_tokens(&id))))
}
async fn list(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    Query(query): Query<PerspectiveQuery>,
) -> Result<Json<Value>, ApiError> {
    let p = perspective(&query)?;
    authorized(grant.perspective(p))?;
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
        .filter(|h| grant.campaign(&h.header(p).meta.id))
        .map(|h| h.header(p).meta)
        .collect();
    Ok(Json(json!(campaigns)))
}
async fn create(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    Json(request): Json<CreateRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    authorized(grant.operator())?;
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
    let meta = handle.header(Perspective::Operator).meta;
    app.register(handle);
    Ok((StatusCode::CREATED, Json(json!(meta))))
}
async fn inspect_campaign(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<PerspectiveQuery>,
) -> Result<Json<Value>, ApiError> {
    let p = perspective(&query)?;
    authorized(grant.campaign(&id) && grant.perspective(p))?;
    let handle = app.campaign(&id)?;
    let status = match handle.status() {
        CampaignStatus::Stopped { error: _ } if p != Perspective::Operator => {
            json!({"state":"stopped"})
        }
        other => json!(other),
    };
    Ok(Json({
        let projection = handle.projection(p);
        json!({"campaign":projection.meta,"status":status,"snapshot":projection})
    }))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundaryRequest {
    #[serde(deserialize_with = "Deserialize::deserialize")]
    boundary: Option<crate::RunBoundary>,
}
async fn run_boundary(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    RoutePath(id): RoutePath<String>,
) -> Result<Json<Value>, ApiError> {
    authorized(grant.operator())?;
    Ok(Json(
        json!({"boundary":app.campaign(&id)?.run_boundary().await?}),
    ))
}
async fn set_run_boundary(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<BoundaryRequest>,
) -> Result<Json<Value>, ApiError> {
    authorized(grant.operator())?;
    app.campaign(&id)?
        .set_run_boundary(request.boundary)
        .await?;
    Ok(Json(json!({"boundary":request.boundary})))
}
async fn pause(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    RoutePath(id): RoutePath<String>,
) -> Result<Json<Value>, ApiError> {
    authorized(grant.operator())?;
    app.campaign(&id)?.pause(true).await?;
    Ok(Json(json!({"paused":true})))
}
async fn resume(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    RoutePath(id): RoutePath<String>,
) -> Result<Json<Value>, ApiError> {
    authorized(grant.operator())?;
    app.campaign(&id)?.pause(false).await?;
    Ok(Json(json!({"paused":false})))
}
async fn seats(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<PerspectiveQuery>,
) -> Result<Json<Value>, ApiError> {
    authorized(grant.campaign(&id) && grant.perspective(perspective(&query)?))?;
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
    Extension(grant): Extension<Grant>,
    RoutePath((id, s)): RoutePath<(String, String)>,
    Json(request): Json<Handover>,
) -> Result<Json<Value>, ApiError> {
    authorized(grant.operator())?;
    Ok(Json(json!(
        app.campaign(&id)?
            .handover(seat(&s)?, request.controller, request.config)
            .await?
    )))
}
async fn pause_seat(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    RoutePath((id, s)): RoutePath<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    authorized(grant.operator())?;
    app.campaign(&id)?.pause_seat(seat(&s)?).await?;
    Ok(Json(json!({"paused":true})))
}
async fn observe(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    RoutePath((id, s)): RoutePath<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    authorized(grant.campaign(&id) && grant.seat(seat(&s)?, false))?;
    let (observation, state) = app.campaign(&id)?.observation_state(seat(&s)?);
    Ok(Json(
        json!({"observation":observation,"pending":state.pending,"controller_epoch":state.binding.controller_epoch,"paused":state.binding.paused,"failure":state.binding.failure}),
    ))
}
async fn inspect_target(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    RoutePath((id, s, target)): RoutePath<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    authorized(grant.campaign(&id) && grant.seat(seat(&s)?, false))?;
    Ok(Json(app.campaign(&id)?.inspect(seat(&s)?, &target).await?))
}
async fn actions(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    RoutePath((id, s, decision)): RoutePath<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    authorized(grant.campaign(&id) && grant.seat(seat(&s)?, false))?;
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
    Extension(grant): Extension<Grant>,
    RoutePath((id, s, decision)): RoutePath<(String, String, String)>,
    Json(draft): Json<Draft>,
) -> Result<Json<Value>, ApiError> {
    authorized(grant.campaign(&id) && grant.seat(seat(&s)?, false))?;
    app.campaign(&id)?
        .validate_action(seat(&s)?, &decision, draft.action)
        .await?;
    Ok(Json(json!({"valid":true})))
}
async fn submit(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    RoutePath((id, s, decision)): RoutePath<(String, String, String)>,
    Json(response): Json<DecisionResponse>,
) -> Result<Json<Value>, ApiError> {
    authorized(grant.campaign(&id) && grant.seat(seat(&s)?, true))?;
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
    Extension(grant): Extension<Grant>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<TranscriptQuery>,
) -> Result<Json<Value>, ApiError> {
    let p: Perspective = query
        .perspective
        .parse()
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid perspective".into()))?;
    let s = seat(&query.seat)?;
    authorized(grant.campaign(&id) && grant.perspective(p) && grant.seat(s, false))?;
    let reader = app.campaign(&id)?.replay();
    let rows = tokio::task::spawn_blocking(move || reader.transcripts(p, s, query.after))
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "replay unavailable".into()))??;
    Ok(Json(json!(rows)))
}
async fn upgrade(
    State(app): State<App>,
    Extension(grant): Extension<Grant>,
    RoutePath(id): RoutePath<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    authorized(grant.campaign(&id))?;
    let handle = app.campaign(&id)?;
    Ok(ws
        .max_message_size(16384)
        .max_frame_size(16384)
        .max_write_buffer_size(2 * 1024 * 1024)
        .on_upgrade(move |socket| stream(socket, handle, grant)))
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
async fn stream(mut socket: WebSocket, handle: CampaignHandle, grant: Grant) {
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
        if !grant.perspective(p) {
            let _ = socket
                .send(Message::Close(Some(CloseFrame {
                    code: close_code::POLICY,
                    reason: "capability scope denied".into(),
                })))
                .await;
            return;
        }
        // Register first, then read one immutable seq+view projection: no snapshot/live race.
        let mut live = handle.subscribe(p);
        let snapshot = handle.snapshot(p);
        let header = snapshot.header();
        let target = header.seq;
        if send(
            &mut socket,
            &ServerMessage::Hello {
                protocol: PROTOCOL_VERSION,
                campaign: header.meta,
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
                    view: snapshot.projection().view,
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
        // The viewer now follows rows; do not retain an old game for its entire connection.
        drop(snapshot);
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
                            if !grant.perspective(next_p) {
                                let _ = socket.send(Message::Close(Some(CloseFrame {
                                    code: close_code::POLICY, reason: "capability scope denied".into(),
                                }))).await;
                                return;
                            }
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
