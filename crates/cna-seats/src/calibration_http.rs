//! Lane-A subscription Messages adapter. No native CLI is started here.
//! Header credentials exist only in request memory; no logs or diagnostic payloads.
//! This adapter proves its own routing, not a child's direct-egress restrictions.

use std::{
    future::Future,
    io,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, OriginalUri, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::post,
};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::calibration::{CalibrationFence, FenceError, OUTPUT_LIMIT, assess_prompt_usage};
use crate::{
    calibration_cost::{CalibrationCostProof, CalibrationPrices},
    request_budget::{RequestBudget, RequestKind, RequestLimits},
};

#[path = "watched_gateway.rs"]
pub mod watched;

const REQUEST_LIMIT: usize = 16 * 1024 * 1024;
const COUNT_REPLY_LIMIT: usize = 32 * 1024;
const EVENT_LIMIT: usize = 256 * 1024;

/// Safe numeric-only evidence. Missing channels are unknown, never zero.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneAReport {
    pub cost: Option<CalibrationCostProof>,
    /// Charged forward attempts, including failed/cancelled sends; not provider receipts.
    pub count_forwards: u64,
    pub generation_forwards: u64,
    pub transport_total: u64,
    pub generation_permit_spent: bool,
    pub inference_sha256: Option<String>,
    pub count_sha256: Option<String>,
    pub dropped_count_fields: Vec<String>,
    pub estimated_input_tokens: Option<u64>,
    pub stream_finished: bool,
    pub input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub prompt_over_limit: bool,
    pub output_over_limit: bool,
    pub accounting_unknown: bool,
}

struct Shared {
    ledger: CalibrationFence,
    client: reqwest::Client,
    upstream: String,
    report: Mutex<LaneAReport>,
    budget: RequestBudget,
    pumps: tokio::sync::Mutex<tokio::task::JoinSet<()>>,
}

/// Opens an already-authorized private ledger. There is no create/rearm endpoint,
/// upstream selection in client payload, credential file, or implicit CLI launch.
#[derive(Clone)]
pub struct LaneAFence(Arc<Shared>);

impl LaneAFence {
    pub fn open(
        path: &Path,
        prices: &CalibrationPrices,
        wall: Duration,
    ) -> Result<Self, FenceError> {
        Self::build_configured(path, "https://api.anthropic.com".into(), prices, wall)
    }

    #[cfg(test)]
    fn build(path: &Path, upstream: String) -> Result<Self, FenceError> {
        Self::build_configured(
            path,
            upstream,
            &crate::calibration_cost::source_fixture_prices(),
            Duration::from_secs(180),
        )
    }

    fn build_configured(
        path: &Path,
        upstream: String,
        prices: &CalibrationPrices,
        wall: Duration,
    ) -> Result<Self, FenceError> {
        let cost = prices.validate().map_err(|_| FenceError::Estimate)?;
        let budget = RequestBudget::new(RequestLimits {
            total: 5,
            generations: 1,
            wall,
        })
        .map_err(|_| FenceError::Estimate)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            // Hyper can retry an unstarted request on a reused connection
            // separately from Reqwest's policy. No pool reuse, HTTP/1 only.
            .http1_only()
            .pool_max_idle_per_host(0)
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
            .timeout(Duration::from_secs(180))
            .tcp_nodelay(true)
            .build()
            .map_err(|_| FenceError::Transport)?;
        let ledger = CalibrationFence::open(path)?;
        let spent = ledger.spent()?;
        Ok(Self(Arc::new(Shared {
            ledger,
            client,
            upstream,
            report: Mutex::new(LaneAReport {
                cost: Some(cost),
                generation_permit_spent: spent,
                accounting_unknown: spent,
                ..LaneAReport::default()
            }),
            budget,
            pumps: tokio::sync::Mutex::new(tokio::task::JoinSet::new()),
        })))
    }

    pub fn report(&self) -> Result<LaneAReport, FenceError> {
        let mut report = self
            .0
            .report
            .lock()
            .map(|r| r.clone())
            .map_err(|_| FenceError::Ledger)?;
        let usage = self.0.budget.usage().map_err(|_| FenceError::Ledger)?;
        report.generation_forwards = usage.generations;
        report.count_forwards = usage.total.saturating_sub(usage.generations);
        report.transport_total = usage.total;
        Ok(report)
    }

    pub fn budget(&self) -> RequestBudget {
        self.0.budget.clone()
    }

    pub fn stop(&self) {
        let _ = self.0.budget.stop();
    }

    pub async fn shutdown_pumps(&self) -> io::Result<()> {
        self.stop();
        let mut pumps = self.0.pumps.lock().await;
        let joined = tokio::time::timeout(Duration::from_secs(2), async {
            let mut failed = false;
            while let Some(result) = pumps.join_next().await {
                failed |= result.is_err();
            }
            failed
        })
        .await;
        match joined {
            Ok(false) => Ok(()),
            Ok(true) => Err(io::Error::other("calibration forwarding task failed")),
            Err(_) => {
                pumps.abort_all();
                let joined = tokio::time::timeout(Duration::from_secs(2), async {
                    while pumps.join_next().await.is_some() {}
                })
                .await;
                if joined.is_err() {
                    return Err(io::Error::other("calibration pump join unconfirmed"));
                }
                Err(io::Error::other("calibration forwarding cleanup timed out"))
            }
        }
    }

    /// Serve only loopback. Caller owns checked shutdown/native cleanup. This
    /// function never starts a provider request or a seat process by itself.
    pub async fn serve(
        self,
        listener: tokio::net::TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> io::Result<()> {
        if !listener.local_addr()?.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "loopback listener required",
            ));
        }
        let owner = self.clone();
        let router = Router::new()
            .route("/v1/messages", post(messages))
            .fallback(|| async { deny(StatusCode::NOT_FOUND) })
            .layer(DefaultBodyLimit::max(REQUEST_LIMIT))
            .with_state(self);
        let control = owner.clone();
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                // Admission/IO stop does not erase the listener before it can
                // return the durable spent-ledger refusal. The launcher owns
                // the separate bounded HTTP shutdown and native cleanup.
                shutdown.await;
                control.stop();
            })
            .await;
        let joined = owner.shutdown_pumps().await;
        result?;
        joined
    }
}

fn deny(status: StatusCode) -> Response {
    // Never format errors, headers, upstream bodies, URIs or request payloads.
    Response::builder()
        .status(status)
        .body(Body::empty())
        .unwrap()
}

fn subscription_headers(headers: &HeaderMap) -> Option<HeaderMap> {
    let mut output = HeaderMap::new();
    for name in ["authorization", "anthropic-beta", "anthropic-version"] {
        let mut values = headers.get_all(name).iter();
        let value = values.next()?;
        if values.next().is_some() || value.is_empty() {
            return None;
        }
        if name == "authorization"
            && value
                .to_str()
                .ok()?
                .strip_prefix("Bearer ")
                .is_none_or(|s| s.is_empty())
        {
            return None;
        }
        let mut value = value.clone();
        if matches!(name, "authorization" | "anthropic-beta") {
            value.set_sensitive(true);
        }
        output.insert(name, value);
    }
    // Subscription only: refuse API credentials rather than choosing one lane.
    if headers.contains_key("x-api-key") {
        return None;
    }
    output.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    Some(output)
}

async fn messages(
    State(fence): State<LaneAFence>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    raw: Bytes,
) -> Response {
    if !matches!(uri.query(), None | Some("beta=true")) {
        return deny(StatusCode::BAD_REQUEST);
    }
    let Some(headers) = subscription_headers(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    if serde_json::from_slice::<Value>(&raw)
        .ok()
        .and_then(|v| v["stream"].as_bool())
        != Some(true)
    {
        return deny(StatusCode::BAD_REQUEST);
    }
    let protected = ProtectedValues::new(&headers);
    let ticket = match fence.0.ledger.begin_count(&raw) {
        Ok(ticket) => ticket,
        Err(_) => return deny(StatusCode::CONFLICT),
    };
    // Count only the official beta schema projection. Keep the complete
    // normalized inference identity as well: these are DIFFERENT wire bodies.
    use sha2::{Digest, Sha256};
    let inference_sha256 = format!("{:x}", Sha256::digest(ticket.body()));
    let count_sha256 = format!("{:x}", Sha256::digest(ticket.count_body()));
    let dropped_count_fields = ticket.dropped_fields().to_vec();
    let counted = fence
        .0
        .budget
        .bound(fence.0.budget.forward(RequestKind::Count, || {
            fence
                .0
                .client
                .post(format!(
                    "{}/v1/messages/count_tokens?beta=true",
                    fence.0.upstream
                ))
                .headers(headers.clone())
                .body(ticket.count_body().to_vec())
                .send()
        }))
        .await;
    let Ok(Ok(Ok(mut counted))) = counted else {
        return deny(StatusCode::BAD_GATEWAY);
    };
    if !counted.status().is_success() {
        return deny(StatusCode::BAD_GATEWAY);
    }
    let mut count_body = Vec::new();
    loop {
        match fence.0.budget.bound(counted.chunk()).await {
            Ok(Ok(Some(chunk)))
                if count_body.len().saturating_add(chunk.len()) <= COUNT_REPLY_LIMIT =>
            {
                count_body.extend_from_slice(&chunk);
            }
            Ok(Ok(None)) => break,
            _ => return deny(StatusCode::BAD_GATEWAY),
        }
    }
    let estimated_input_tokens = serde_json::from_slice::<Value>(&count_body)
        .ok()
        .and_then(|v| v["input_tokens"].as_u64());
    if fence
        .0
        .ledger
        .finish_count(ticket, &count_body, true)
        .is_err()
    {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    // The core commits spent BEFORE even constructing the one transport future.
    // Reqwest cannot retry or redirect. Cancellation at any subsequent point
    // leaves this same ledger spent, even if HTTP dispatch never completed.
    // Recheck the immutable explicit price proof before spending the original
    // durable permit. No CLI cost report or estimated cache discount is used.
    if fence
        .report()
        .ok()
        .and_then(|r| r.cost)
        .is_none_or(|proof| proof.prices.validate().is_err())
    {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let sent = fence.0.ledger.generate(&raw, |body| {
        let body = body.to_vec();
        let headers = headers.clone();
        let client = fence.0.client.clone();
        let url = format!("{}/v1/messages?beta=true", fence.0.upstream);
        let budget = fence.0.budget.clone();
        Ok(async move {
            budget
                .forward(RequestKind::Generation, || {
                    client.post(url).headers(headers).body(body).send()
                })
                .await
        })
    });
    let Ok(sent) = sent else {
        return deny(StatusCode::CONFLICT);
    };
    if let Ok(mut report) = fence.0.report.lock() {
        report.generation_permit_spent = true;
        report.inference_sha256 = Some(inference_sha256);
        report.count_sha256 = Some(count_sha256);
        report.dropped_count_fields = dropped_count_fields;
        report.estimated_input_tokens = estimated_input_tokens;
        report.accounting_unknown = true;
    } else {
        return deny(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let Ok(Ok(Ok(upstream))) = fence.0.budget.bound(sent).await else {
        return deny(StatusCode::BAD_GATEWAY);
    };
    let status = upstream.status();
    // Deny redirects without forwarding Location or any possible capabilities.
    if status.is_redirection() {
        return deny(StatusCode::BAD_GATEWAY);
    }
    // Only guarded SSE belongs to this streamed calibration path. Non-SSE
    // errors can contain decoded JSON reflections; never relay those bodies or
    // headers. Drop them rather than buffering/parsing a second response format.
    let is_sse = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("text/event-stream"));
    if !is_sse {
        return deny(if status.is_success() {
            StatusCode::BAD_GATEWAY
        } else {
            status
        });
    }
    let mut response = Response::builder().status(status);
    // Only documented public metadata. Never forward auth, beta/capability,
    // cookies, Location, or arbitrary reflected request headers.
    for (name, value) in upstream.headers() {
        if name == header::CONTENT_TYPE
            || name == "retry-after"
            || name == "request-id"
            || name.as_str().starts_with("anthropic-ratelimit-")
        {
            if protected.contains(value.as_bytes()) {
                return deny(StatusCode::BAD_GATEWAY);
            }
            response = response.header(name, value);
        }
    }
    pump::response(fence, upstream, response, protected).await
}

#[path = "calibration_pump.rs"]
mod pump;

#[cfg(test)]
#[path = "calibration_runtime_tests.rs"]
mod runtime_tests;

#[path = "calibration_strict_json.rs"]
pub(crate) mod strict_json;

#[path = "calibration_reflection.rs"]
mod reflection;
use reflection::ProtectedValues;

#[derive(Default)]
struct UsageObserver {
    line: Vec<u8>,
    data: Vec<u8>,
    invalid: bool,
    stopped: bool,
    previous_cr: bool,
}

impl UsageObserver {
    fn feed(&mut self, bytes: &[u8], fence: &LaneAFence) {
        self.feed_report(bytes, &fence.0.report);
    }

    fn feed_report(&mut self, bytes: &[u8], report: &Mutex<LaneAReport>) {
        for byte in bytes {
            if self.previous_cr && *byte == b'\n' {
                self.previous_cr = false;
                continue;
            }
            self.previous_cr = *byte == b'\r';
            if *byte == b'\n' || *byte == b'\r' {
                if self.line.is_empty() {
                    self.event(report);
                    self.data.clear();
                } else {
                    let line = self
                        .line
                        .strip_prefix(b"\xef\xbb\xbf")
                        .unwrap_or(&self.line);
                    if let Some(data) = line.strip_prefix(b"data:") {
                        let data = data.strip_prefix(b" ").unwrap_or(data);
                        if self.data.len().saturating_add(data.len()).saturating_add(1)
                            > EVENT_LIMIT
                        {
                            self.invalid = true;
                            self.data.clear();
                        } else {
                            if !self.data.is_empty() {
                                self.data.push(b'\n');
                            }
                            self.data.extend_from_slice(data);
                        }
                    }
                }
                self.line.clear();
            } else if self.line.len() < EVENT_LIMIT {
                self.line.push(*byte);
            } else {
                self.invalid = true;
            }
        }
    }

    fn event(&mut self, report: &Mutex<LaneAReport>) {
        if self.data.is_empty() {
            return;
        }
        let Ok(event) = serde_json::from_slice::<Value>(&self.data) else {
            self.invalid = true;
            return;
        };
        let usage = match event["type"].as_str() {
            Some("message_start") => Some(&event["message"]["usage"]),
            Some("message_delta") => Some(&event["usage"]),
            Some("message_stop") => {
                self.stopped = true;
                None
            }
            Some("error") => {
                self.invalid = true;
                None
            }
            _ => None,
        };
        if let Some(usage) = usage {
            let Ok(mut report) = report.lock() else {
                self.invalid = true;
                return;
            };
            let LaneAReport {
                input_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
                output_tokens,
                ..
            } = &mut *report;
            for (name, slot) in [
                ("input_tokens", input_tokens),
                ("cache_creation_input_tokens", cache_creation_input_tokens),
                ("cache_read_input_tokens", cache_read_input_tokens),
                ("output_tokens", output_tokens),
            ] {
                if let Some(value) = usage.get(name) {
                    if let Some(value) = value.as_u64() {
                        if slot.is_some_and(|old| old > value) {
                            self.invalid = true;
                        }
                        *slot = Some(value);
                    } else {
                        self.invalid = true;
                    }
                }
            }
            // Missing channels remain unmeasured in the report. Known channels
            // provide a nonnegative lower bound; saturation also proves a crossing
            // when their sum exceeds u64. Completeness is assessed separately.
            report.prompt_over_limit |= [
                report.input_tokens,
                report.cache_creation_input_tokens,
                report.cache_read_input_tokens,
            ]
            .into_iter()
            .flatten()
            .fold(0_u64, u64::saturating_add)
                > crate::calibration::ACTUAL_PROMPT_LIMIT;
            report.output_over_limit |= report.output_tokens.is_some_and(|n| n > OUTPUT_LIMIT);
        }
    }

    fn finish(self, fence: &LaneAFence) {
        self.finish_report(&fence.0.report);
    }

    fn finish_report(self, report: &Mutex<LaneAReport>) {
        if let Ok(mut report) = report.lock() {
            report.stream_finished = true;
            report.accounting_unknown = self.invalid
                || !self.stopped
                || !self.line.is_empty()
                || !self.data.is_empty()
                || report.output_tokens.is_none()
                || assess_prompt_usage(
                    report.input_tokens,
                    report.cache_creation_input_tokens,
                    report.cache_read_input_tokens,
                )
                .is_err();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::json;
    use tokio::{
        sync::{Notify, oneshot},
        task::JoinHandle,
    };

    #[derive(Default)]
    struct Calls {
        counts: Vec<(Vec<u8>, HeaderMap)>,
        generations: Vec<(Vec<u8>, HeaderMap)>,
    }
    #[derive(Clone)]
    struct Fake {
        calls: Arc<Mutex<Calls>>,
        count: Value,
        count_status: StatusCode,
        generation_status: StatusCode,
        generation_media: Option<&'static str>,
        tail: Vec<u8>,
        gate: Arc<Notify>,
        wait: bool,
        reflect_header: bool,
    }
    struct Fixture {
        path: std::path::PathBuf,
        fence: LaneAFence,
        url: String,
        fake: Fake,
        stops: Vec<oneshot::Sender<()>>,
        joins: Vec<JoinHandle<io::Result<()>>>,
        client: reqwest::Client,
    }
    fn body() -> Value {
        json!({"model":crate::calibration::MODEL,"max_tokens":128000,
            "messages":[{"role":"user","content":"Synthetic offline request"}],"stream":true})
    }
    fn complete(input: u64) -> Vec<u8> {
        format!(
            "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"input_tokens\":{input},\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":1}}}}}}\n\n\
             event: message_delta\ndata: {{\"type\":\"message_delta\",\"usage\":{{\"output_tokens\":7}}}}\n\n\
             event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
        ).into_bytes()
    }
    async fn count(State(fake): State<Fake>, headers: HeaderMap, raw: Bytes) -> Response {
        fake.calls
            .lock()
            .unwrap()
            .counts
            .push((raw.to_vec(), headers));
        Response::builder()
            .status(fake.count_status)
            .header("location", "/v1/messages/count_tokens")
            .body(Body::from(serde_json::to_vec(&fake.count).unwrap()))
            .unwrap()
    }
    async fn generate(State(fake): State<Fake>, headers: HeaderMap, raw: Bytes) -> Response {
        fake.calls
            .lock()
            .unwrap()
            .generations
            .push((raw.to_vec(), headers));
        let status = fake.generation_status;
        let media = fake.generation_media;
        let reflected = fake.reflect_header.then(|| {
            fake.calls.lock().unwrap().generations.last().unwrap().1["authorization"].clone()
        });
        let body = stream::unfold((0, fake), |(step, fake)| async move {
            match step {
                0 => Some((
                    Ok::<_, io::Error>(Bytes::from_static(
                        if fake.generation_media == Some("application/json") {
                            b""
                        } else {
                            b"event: ping\ndata: {\"type\":\"ping\"}\n\n"
                        },
                    )),
                    (1, fake),
                )),
                1 => {
                    if fake.wait {
                        fake.gate.notified().await;
                    }
                    Some((Ok(Bytes::from(fake.tail.clone())), (2, fake)))
                }
                _ => None,
            }
        });
        let mut response = Response::builder().status(status);
        if let Some(value) = reflected {
            response = response.header("request-id", value);
        }
        if let Some(media) = media {
            response = response.header("content-type", media);
        }
        response
            .header("retry-after", "3")
            .header("anthropic-ratelimit-requests-remaining", "0")
            .header("authorization", "synthetic-response-auth-canary")
            .header("anthropic-beta", "synthetic-response-capability-canary")
            .header("set-cookie", "synthetic-cookie-canary")
            .header("location", "/v1/messages")
            .body(Body::from_stream(body))
            .unwrap()
    }
    impl Fixture {
        async fn new(mutator: impl FnOnce(&mut Fake)) -> Self {
            Self::new_with_path(None, mutator).await
        }
        async fn new_with_path(
            path: Option<std::path::PathBuf>,
            mutator: impl FnOnce(&mut Fake),
        ) -> Self {
            let mut fake = Fake {
                calls: Arc::new(Mutex::new(Calls::default())),
                count: json!({"input_tokens":95000}),
                count_status: StatusCode::OK,
                generation_status: StatusCode::OK,
                generation_media: Some("text/event-stream"),
                tail: complete(95000),
                gate: Arc::new(Notify::new()),
                wait: false,
                reflect_header: false,
            };
            mutator(&mut fake);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let upstream = format!("http://{}", listener.local_addr().unwrap());
            let router = Router::new()
                .route("/v1/messages/count_tokens", post(count))
                .route("/v1/messages", post(generate))
                .with_state(fake.clone());
            let (stop, wait) = oneshot::channel();
            let join = tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async {
                        let _ = wait.await;
                    })
                    .await
            });
            let path = path.unwrap_or_else(|| {
                std::env::temp_dir().join(format!(
                    "cna-lane-a-offline-{}.sqlite",
                    uuid::Uuid::new_v4()
                ))
            });
            if !path.exists() {
                CalibrationFence::initialize(&path).unwrap();
            }
            let fence = LaneAFence::build(&path, upstream).unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let (stop2, wait) = oneshot::channel();
            let serving = fence.clone();
            let join2 = tokio::spawn(async move {
                serving
                    .serve(listener, async {
                        let _ = wait.await;
                    })
                    .await
            });
            Self {
                path,
                fence,
                url,
                fake,
                stops: vec![stop, stop2],
                joins: vec![join, join2],
                client: reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap(),
            }
        }
        fn request(&self, value: &Value) -> reqwest::RequestBuilder {
            self.client
                .post(format!("{}/v1/messages?beta=true", self.url))
                .header("authorization", "Bearer synthetic-request-auth-canary")
                .header(
                    "anthropic-beta",
                    "oauth-synthetic-capability,other-synthetic-beta",
                )
                .header("anthropic-version", "2023-06-01")
                .header("x-operator-canary", "synthetic-operator-canary")
                .json(value)
        }
        async fn close(self) {
            self.fake.gate.notify_waiters();
            drop(self.client);
            for stop in self.stops {
                let _ = stop.send(());
            }
            for join in self.joins {
                tokio::time::timeout(Duration::from_secs(10), join)
                    .await
                    .expect("offline server hang guard")
                    .unwrap()
                    .unwrap();
            }
            std::fs::remove_file(self.path).unwrap();
        }
    }

    // Explicit offline harness only: this test never starts a native process.
    // The separate owner-authorized one-shot emitter writes body bytes to scratch.
    #[tokio::test]
    #[ignore = "opt-in: replay a captured request against fake loopback only"]
    async fn captured_request_fake_only_replay() {
        let Ok(directory) = std::env::var("CNA_CAPTURED_REQUEST_OFFLINE_DIR") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        assert!(directory.is_absolute());
        assert!(directory.is_dir());
        let path = directory.join("disposable-calibration.sqlite");
        let report_path = directory.join("offline-fence-result.json");
        if std::env::var("CNA_CAPTURED_REQUEST_PREPARE").as_deref() == Ok("1") {
            assert!(!path.exists());
            CalibrationFence::initialize(&path).unwrap();
            std::fs::write(
                directory.join("own-nine-tools.json"),
                serde_json::to_vec(&crate::mcp::ToolRouter::tool_list()).unwrap(),
            )
            .unwrap();
            return;
        }
        assert!(path.is_file()); // Reopening never creates/rearms a permit.
        let body_path = directory.join("emitted-body.bin");
        let mut status = None;
        let mut count_calls = 0;
        let mut generation_calls = 0;
        let mut projection_verified = false;
        let mut observed_body = false;
        if body_path.exists() {
            observed_body = true;
            let raw = std::fs::read(&body_path).unwrap();
            let fixture = Fixture::new_with_path(Some(path.clone()), |_| {}).await;
            let response = fixture
                .request(&serde_json::from_slice::<Value>(&raw).unwrap())
                .send()
                .await
                .unwrap();
            status = Some(response.status().as_u16());
            let _ = response.bytes().await.unwrap();
            {
                let calls = fixture.fake.calls.lock().unwrap();
                count_calls = calls.counts.len();
                generation_calls = calls.generations.len();
                if let Some((count, _)) = calls.counts.first() {
                    let count: Value = serde_json::from_slice(count).unwrap();
                    let request: Value = serde_json::from_slice(&raw).unwrap();
                    for field in ["system", "messages", "tools", "tool_choice"] {
                        assert_eq!(count.get(field), request.get(field));
                    }
                    projection_verified = true;
                }
            }
            // Read only numeric durable state before the fixture closes/deletes it.
            let ledger = CalibrationFence::open(&path).unwrap();
            let attempts: i64 = rusqlite::Connection::open(&path)
                .unwrap()
                .query_row("SELECT attempts FROM calibration WHERE id=1", [], |row| {
                    row.get(0)
                })
                .unwrap();
            let report = json!({"observed_body":observed_body,"http_status":status,
                "fake_count_calls":count_calls,"fake_generation_calls":generation_calls,
                "count_attempts":attempts,"permit_spent":ledger.spent().unwrap(),
                "projection_verified":projection_verified,"native_execution_in_this_test":false});
            fixture.close().await;
            std::fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        } else {
            let ledger = CalibrationFence::open(&path).unwrap();
            let attempts: i64 = rusqlite::Connection::open(&path)
                .unwrap()
                .query_row("SELECT attempts FROM calibration WHERE id=1", [], |row| {
                    row.get(0)
                })
                .unwrap();
            let report = json!({"observed_body":observed_body,"http_status":status,
                "fake_count_calls":count_calls,"fake_generation_calls":generation_calls,
                "count_attempts":attempts,"permit_spent":ledger.spent().unwrap(),
                "projection_verified":projection_verified,"native_execution_in_this_test":false});
            std::fs::remove_file(&path).unwrap();
            std::fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        }
        assert!(!path.exists());
        assert!(!path.with_extension("sqlite-wal").exists());
        assert!(!path.with_extension("sqlite-shm").exists());
    }

    #[tokio::test]
    async fn real_http_projected_count_and_verbatim_subscription_headers_no_echo() {
        let fixture = Fixture::new(|_| {}).await;
        let response = fixture.request(&body()).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["retry-after"], "3");
        assert_eq!(
            response.headers()["anthropic-ratelimit-requests-remaining"],
            "0"
        );
        for key in ["authorization", "anthropic-beta", "set-cookie", "location"] {
            assert!(!response.headers().contains_key(key));
        }
        let raw = response.bytes().await.unwrap();
        assert_eq!(
            raw,
            [
                b"event: ping\ndata: {\"type\":\"ping\"}\n\n".as_slice(),
                complete(95000).as_slice()
            ]
            .concat()
        );
        {
            let calls = fixture.fake.calls.lock().unwrap();
            assert_eq!(calls.counts.len(), 1);
            assert_eq!(calls.generations.len(), 1);
            let counted: Value = serde_json::from_slice(&calls.counts[0].0).unwrap();
            let inference: Value = serde_json::from_slice(&calls.generations[0].0).unwrap();
            assert_eq!(counted["messages"], inference["messages"]);
            assert_eq!(counted["model"], inference["model"]);
            assert!(counted.get("max_tokens").is_none());
            assert!(counted.get("stream").is_none());
            assert_eq!(inference["max_tokens"], 128000);
            assert_eq!(inference["stream"], true);
            for (_, headers) in calls.counts.iter().chain(calls.generations.iter()) {
                assert_eq!(
                    headers["authorization"],
                    "Bearer synthetic-request-auth-canary"
                );
                assert_eq!(
                    headers["anthropic-beta"],
                    "oauth-synthetic-capability,other-synthetic-beta"
                );
                assert_eq!(headers["anthropic-version"], "2023-06-01");
                assert!(!headers.contains_key("x-operator-canary"));
            }
        }
        let report = fixture.fence.report().unwrap();
        assert!(report.stream_finished && !report.accounting_unknown);
        assert_eq!(report.output_tokens, Some(7));
        let report = serde_json::to_string(&report).unwrap();
        let ledger = std::fs::read(&fixture.path).unwrap();
        for secret in [
            "synthetic-request-auth-canary",
            "oauth-synthetic-capability",
            "synthetic-operator-canary",
        ] {
            assert!(!report.contains(secret));
            assert!(!ledger.windows(secret.len()).any(|w| w == secret.as_bytes()));
        }
        assert_eq!(
            fixture.request(&body()).send().await.unwrap().status(),
            StatusCode::CONFLICT
        );
        assert!(
            CalibrationFence::open(&fixture.path)
                .unwrap()
                .spent()
                .unwrap()
        );
        fixture.close().await;
    }

    #[tokio::test]
    async fn sse_ping_reaches_client_before_provider_finishes() {
        let fixture = Fixture::new(|f| f.wait = true).await;
        let mut response = fixture.request(&body()).send().await.unwrap();
        let ping = tokio::time::timeout(Duration::from_secs(10), response.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            ping,
            b"event: ping\ndata: {\"type\":\"ping\"}\n\n".as_slice()
        );
        assert!(!fixture.fence.report().unwrap().stream_finished);
        fixture.fake.gate.notify_one();
        response.bytes().await.unwrap();
        assert!(fixture.fence.report().unwrap().stream_finished);
        fixture.close().await;
    }

    #[tokio::test]
    async fn redirects_and_rate_errors_never_trigger_second_generation() {
        for status in [
            StatusCode::TEMPORARY_REDIRECT,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let fixture=Fixture::new(|f| { f.generation_status=status; f.tail=b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"synthetic_error\"}}\n\n".to_vec(); }).await;
            let response = fixture.request(&body()).send().await.unwrap();
            assert_eq!(
                response.status(),
                if status.is_redirection() {
                    StatusCode::BAD_GATEWAY
                } else {
                    status
                }
            );
            response.bytes().await.unwrap();
            assert_eq!(
                fixture.request(&body()).send().await.unwrap().status(),
                StatusCode::CONFLICT
            );
            assert_eq!(fixture.fake.calls.lock().unwrap().generations.len(), 1);
            assert!(fixture.fence.report().unwrap().accounting_unknown);
            fixture.close().await;
        }
    }

    #[tokio::test]
    async fn reflected_header_and_body_canaries_never_reach_http_client() {
        for reflected_header in [true, false] {
            let fixture = Fixture::new(|f| {
                f.reflect_header = reflected_header;
                f.tail = b"data: synthetic-auth-canary and oauth-synthetic-capability\n\n".to_vec();
            })
            .await;
            let response = fixture.request(&body()).send().await;
            if let Ok(mut response) = response {
                assert!(!response.headers().contains_key("request-id"));
                let mut received = Vec::new();
                while let Ok(Some(chunk)) = response.chunk().await {
                    received.extend_from_slice(&chunk);
                }
                for value in ["synthetic-auth-canary", "oauth-synthetic-capability"] {
                    assert!(!received.windows(value.len()).any(|w| w == value.as_bytes()));
                }
            }
            assert!(
                CalibrationFence::open(&fixture.path)
                    .unwrap()
                    .spent()
                    .unwrap()
            );
            assert_eq!(fixture.fake.calls.lock().unwrap().generations.len(), 1);
            fixture.close().await;
        }
    }

    #[tokio::test]
    async fn cancelled_client_cannot_retry_or_continue_after_committed_send() {
        let fixture = Fixture::new(|f| f.wait = true).await;
        let mut response = fixture.request(&body()).send().await.unwrap();
        response.chunk().await.unwrap().unwrap();
        drop(response);
        assert!(
            CalibrationFence::open(&fixture.path)
                .unwrap()
                .spent()
                .unwrap()
        );
        assert_eq!(
            fixture.request(&body()).send().await.unwrap().status(),
            StatusCode::CONFLICT
        );
        assert_eq!(fixture.fake.calls.lock().unwrap().generations.len(), 1);
        assert!(fixture.fence.report().unwrap().accounting_unknown);
        fixture.fake.gate.notify_one();
        fixture.close().await;
    }

    #[tokio::test]
    async fn unknown_excessive_and_redirected_counts_fail_closed() {
        for value in [
            json!({"input_tokens":95001}),
            json!({"input_tokens":0}),
            json!({"input_tokens":"95000"}),
            json!({}),
        ] {
            let fixture = Fixture::new(|f| f.count = value).await;
            for _ in 0..4 {
                assert_eq!(
                    fixture.request(&body()).send().await.unwrap().status(),
                    StatusCode::UNPROCESSABLE_ENTITY
                );
            }
            assert_eq!(
                fixture.request(&body()).send().await.unwrap().status(),
                StatusCode::CONFLICT
            );
            assert_eq!(fixture.fake.calls.lock().unwrap().counts.len(), 4);
            assert!(fixture.fake.calls.lock().unwrap().generations.is_empty());
            fixture.close().await;
        }
        let fixture = Fixture::new(|f| f.count_status = StatusCode::TEMPORARY_REDIRECT).await;
        assert_eq!(
            fixture.request(&body()).send().await.unwrap().status(),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(fixture.fake.calls.lock().unwrap().counts.len(), 1);
        assert!(fixture.fake.calls.lock().unwrap().generations.is_empty());
        fixture.close().await;
    }

    #[tokio::test]
    async fn concurrent_http_retries_and_reopen_keep_one_generation() {
        let fixture = Fixture::new(|_| {}).await;
        let requests = (0..12).map(|_| fixture.request(&body()).send());
        let responses = futures_util::future::join_all(requests).await;
        let mut successes = 0;
        for response in responses {
            let response = response.unwrap();
            if response.status().is_success() {
                successes += 1;
            }
            response.bytes().await.unwrap();
        }
        assert_eq!(successes, 1);
        assert_eq!(fixture.fake.calls.lock().unwrap().generations.len(), 1);
        let reopened = LaneAFence::open(
            &fixture.path,
            &crate::calibration_cost::source_fixture_prices(),
            Duration::from_secs(180),
        )
        .unwrap();
        assert!(reopened.0.ledger.spent().unwrap());
        let unknown = reopened.report().unwrap();
        assert!(unknown.generation_permit_spent && unknown.accounting_unknown);
        assert!(!unknown.stream_finished && unknown.input_tokens.is_none());
        assert_eq!(
            fixture.request(&body()).send().await.unwrap().status(),
            StatusCode::CONFLICT
        );
        fixture.close().await;
    }

    #[tokio::test]
    async fn no_unknown_endpoint_model_auth_tool_or_query_reaches_upstream() {
        let fixture = Fixture::new(|_| {}).await;
        for path in [
            "/api/hello",
            "/v1/models",
            "/v1/messages/count_tokens",
            "/v1/messages?beta=false",
            "/v1/messages?beta=true&target=other",
        ] {
            let response = fixture
                .client
                .post(format!("{}{path}", fixture.url))
                .json(&body())
                .send()
                .await
                .unwrap();
            assert!(!response.status().is_success());
        }
        let mut wrong = body();
        wrong["model"] = json!("other");
        assert_eq!(
            fixture.request(&wrong).send().await.unwrap().status(),
            StatusCode::CONFLICT
        );
        wrong = body();
        wrong["tools"] = json!([{"name":"Bash"}]);
        assert_eq!(
            fixture.request(&wrong).send().await.unwrap().status(),
            StatusCode::CONFLICT
        );
        let response = fixture
            .request(&body())
            .header("x-api-key", "synthetic-console-canary")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let text = response.text().await.unwrap();
        assert!(!text.contains("canary"));
        let mut duplicate = HeaderMap::new();
        duplicate.append(
            "anthropic-beta",
            HeaderValue::from_static("synthetic-extra"),
        );
        duplicate.append(
            "anthropic-beta",
            HeaderValue::from_static("synthetic-second"),
        );
        assert_eq!(
            fixture
                .request(&body())
                .headers(duplicate)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert!(fixture.fake.calls.lock().unwrap().counts.is_empty());
        assert!(fixture.fake.calls.lock().unwrap().generations.is_empty());
        fixture.close().await;
    }

    #[tokio::test]
    async fn usage_crossing_drops_stream_unknown_usage_never_becomes_zero() {
        for input in [100001, 95000] {
            let fixture=Fixture::new(|f| {
                f.tail=if input>100000 {complete(input)}
                    else {b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_vec()};
            }).await;
            let response = fixture.request(&body()).send().await;
            if let Ok(response) = response {
                let _ = response.bytes().await;
            }
            let report = fixture.fence.report().unwrap();
            assert_eq!(report.prompt_over_limit, input > 100000);
            assert!(report.accounting_unknown);
            assert_eq!(
                fixture.request(&body()).send().await.unwrap().status(),
                StatusCode::CONFLICT
            );
            assert_eq!(fixture.fake.calls.lock().unwrap().generations.len(), 1);
            fixture.close().await;
        }
    }

    #[tokio::test]
    async fn output_above_model_ceiling_aborts_and_does_not_rearm() {
        let fixture = Fixture::new(|f| {
            f.tail = String::from_utf8(complete(95000))
                .unwrap()
                .replace("\"output_tokens\":7", "\"output_tokens\":128001")
                .into_bytes();
        })
        .await;
        if let Ok(response) = fixture.request(&body()).send().await {
            let _ = response.bytes().await;
        }
        let report = fixture.fence.report().unwrap();
        assert!(report.output_over_limit && report.accounting_unknown);
        assert!(
            CalibrationFence::open(&fixture.path)
                .unwrap()
                .spent()
                .unwrap()
        );
        assert_eq!(
            fixture.request(&body()).send().await.unwrap().status(),
            StatusCode::CONFLICT
        );
        assert_eq!(fixture.fake.calls.lock().unwrap().generations.len(), 1);
        fixture.close().await;
    }

    #[test]
    fn ephemeral_forwarded_auth_and_capability_values_are_sensitive() {
        let mut input = HeaderMap::new();
        input.insert(
            "authorization",
            HeaderValue::from_static("Bearer synthetic-auth-canary"),
        );
        input.insert(
            "anthropic-beta",
            HeaderValue::from_static("oauth-synthetic-capability"),
        );
        input.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        let forwarded = subscription_headers(&input).unwrap();
        for name in ["authorization", "anthropic-beta"] {
            assert!(forwarded[name].is_sensitive());
            assert_eq!(forwarded[name].as_bytes(), input[name].as_bytes());
        }
    }

    #[test]
    fn reflected_subscription_headers_are_rejected_across_chunk_boundaries() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer synthetic-auth-canary"),
        );
        headers.insert(
            "anthropic-beta",
            HeaderValue::from_static("oauth-synthetic-capability,synthetic-other"),
        );
        for secret in [
            "Bearer synthetic-auth-canary",
            "synthetic-auth-canary",
            "oauth-synthetic-capability",
        ] {
            let mut guard = ProtectedValues::new(&headers);
            assert_eq!(
                guard.push(b"event: ping\n\n").unwrap(),
                b"event: ping\n\n".as_slice()
            );
            for chunk in secret.as_bytes()[..secret.len() - 1].chunks(2) {
                assert!(guard.push(chunk).unwrap().is_empty());
            }
            assert!(guard.push(&secret.as_bytes()[secret.len() - 1..]).is_err());
        }
    }

    #[test]
    fn split_crlf_and_multiple_data_lines_preserve_usage() {
        let path =
            std::env::temp_dir().join(format!("cna-lane-a-parser-{}.sqlite", uuid::Uuid::new_v4()));
        CalibrationFence::initialize(&path).unwrap();
        let fence = LaneAFence::open(
            &path,
            &crate::calibration_cost::source_fixture_prices(),
            Duration::from_secs(180),
        )
        .unwrap();
        let mut observer = UsageObserver::default();
        let bytes = complete(95000);
        let bytes = String::from_utf8(bytes)
            .unwrap()
            .replace(
                "\"type\":\"message_delta\",\"usage\"",
                "\"type\":\"message_delta\",\ndata: \"usage\"",
            )
            .replace('\n', "\r\n")
            .into_bytes();
        for byte in bytes {
            observer.feed(&[byte], &fence);
        }
        observer.finish(&fence);
        assert!(!fence.report().unwrap().accounting_unknown);
        std::fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn unsupported_media_errors_never_relay_encoded_reflections() {
        for media in [
            Some("application/json"),
            None,
            Some("text/plain"),
            Some("text/event-streamish"),
        ] {
            for status in [StatusCode::OK, StatusCode::UNAUTHORIZED] {
                let fixture = Fixture::new(|fake| {
                    fake.generation_status = status;
                    fake.generation_media = media;
                    let encoded = "synthetic-auth-canary"
                        .chars()
                        .map(|c| format!("\\u{:04x}", c as u32))
                        .collect::<String>();
                    fake.tail = format!("{{\"error\":{{\"message\":\"{encoded}\"}}}}").into_bytes();
                })
                .await;
                let response = fixture.request(&body()).send().await.unwrap();
                let observed_status = response.status();
                let received = response.bytes().await.unwrap();
                let report = fixture.fence.report().unwrap();
                let retry = fixture.request(&body()).send().await.unwrap().status();
                let count = fixture.fake.calls.lock().unwrap().generations.len();
                let valid_json_source = serde_json::from_slice::<Value>(&fixture.fake.tail)
                    .ok()
                    .is_some_and(|value| {
                        value["error"]["message"].as_str() == Some("synthetic-auth-canary")
                    });
                fixture.close().await;
                assert!(valid_json_source);
                assert_eq!(
                    observed_status,
                    if status.is_success() {
                        StatusCode::BAD_GATEWAY
                    } else {
                        status
                    }
                );
                assert!(
                    received.is_empty() && report.accounting_unknown && !report.stream_finished
                );
                assert_eq!(retry, StatusCode::CONFLICT);
                assert_eq!(count, 1);
            }
        }
    }
    #[tokio::test]
    async fn sse_media_parameters_and_case_keep_streaming_control() {
        let fixture =
            Fixture::new(|fake| fake.generation_media = Some("Text/Event-Stream; charset=utf-8"))
                .await;
        let response = fixture.request(&body()).send().await.unwrap();
        let status = response.status();
        let bytes = response.bytes().await.unwrap();
        let report = fixture.fence.report().unwrap();
        fixture.close().await;
        assert_eq!(status, StatusCode::OK);
        assert!(!bytes.is_empty() && report.stream_finished && !report.accounting_unknown);
    }
}

#[cfg(test)]
#[path = "calibration_http_review_tests.rs"]
mod independent_review;

#[cfg(test)]
#[path = "calibration_http_channel_review_tests.rs"]
mod channel_review;
