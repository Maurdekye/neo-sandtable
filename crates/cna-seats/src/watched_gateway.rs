//! Multi-request adapter for one watched, non-recovering run. Every upstream
//! forward spends the same run budget, including count requests and CLI retries.
//! This enforces routed traffic only; native process containment is independent.

use super::*;
use crate::{
    calibration::{COUNT_LIMIT, checked_body, count_projection},
    request_budget::{RequestBudget, RequestBudgetError, RequestKind, RequestUsage},
};

/// Sums of measured channels only. Unknown or interrupted generations are
/// counted separately: these values must never be called complete run usage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenLowerBounds {
    pub input: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub output: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WatchedReport {
    pub requests: RequestUsage,
    pub tokens: TokenLowerBounds,
    pub complete_generations: u64,
    pub unknown_generations: u64,
}

#[derive(Default)]
struct Totals {
    tokens: TokenLowerBounds,
    complete: u64,
}

struct WatchedShared {
    budget: RequestBudget,
    client: reqwest::Client,
    upstream: String,
    totals: Arc<Mutex<Totals>>,
    pumps: tokio::sync::Mutex<tokio::task::JoinSet<()>>,
}

/// The launcher creates one instance and shares it with all ten seats. There is
/// no rearm/config/report HTTP endpoint. Opening starts no network or CLI work.
#[derive(Clone)]
pub struct WatchedGateway(Arc<WatchedShared>);

impl WatchedGateway {
    pub fn new(budget: RequestBudget) -> Result<Self, FenceError> {
        Self::build(budget, "https://api.anthropic.com".into())
    }

    fn build(budget: RequestBudget, upstream: String) -> Result<Self, FenceError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .http1_only()
            .pool_max_idle_per_host(0)
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| FenceError::Transport)?;
        Ok(Self(Arc::new(WatchedShared {
            budget,
            client,
            upstream,
            totals: Arc::new(Mutex::new(Totals::default())),
            pumps: tokio::sync::Mutex::new(tokio::task::JoinSet::new()),
        })))
    }

    pub fn report(&self) -> Result<WatchedReport, FenceError> {
        let requests = self.0.budget.usage().map_err(|_| FenceError::Ledger)?;
        let totals = self.0.totals.lock().map_err(|_| FenceError::Ledger)?;
        Ok(WatchedReport {
            requests,
            tokens: totals.tokens,
            complete_generations: totals.complete,
            unknown_generations: requests.generations.saturating_sub(totals.complete),
        })
    }

    pub fn stop(&self) -> Result<(), RequestBudgetError> {
        self.0.budget.stop()
    }

    /// Launcher fallback after joining/aborting a bounded HTTP service. Stop
    /// admissions first; service ownership must be settled before this drain.
    pub async fn shutdown_pumps(&self) -> io::Result<()> {
        let _ = self.stop();
        self.join_pumps().await
    }

    /// Shutdown first cancels active upstream IO, then drains HTTP connections.
    /// Caller must join this task and independently kill/join native children.
    pub async fn serve(
        self,
        listener: tokio::net::TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> io::Result<()> {
        if !listener.local_addr()?.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "loopback required",
            ));
        }
        let budget = self.0.budget.clone();
        let control = self.clone();
        let router = Router::new()
            .route("/v1/messages", post(messages))
            .fallback(|| async { deny(StatusCode::NOT_FOUND) })
            .layer(DefaultBodyLimit::max(REQUEST_LIMIT))
            .layer(axum::middleware::from_fn_with_state(
                self.clone(),
                request_boundary,
            ))
            .with_state(self);
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                shutdown.await;
                let _ = budget.stop();
            })
            .await;
        let _ = control.stop();
        let joined = control.join_pumps().await;
        result?;
        joined
    }

    async fn join_pumps(&self) -> io::Result<()> {
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
            Ok(true) => Err(io::Error::other("watched forwarding task failed")),
            Err(_) => {
                pumps.abort_all();
                // Bounded task destruction; never leave an unbounded waiter.
                let _ = tokio::time::timeout(Duration::from_secs(2), async {
                    while pumps.join_next().await.is_some() {}
                })
                .await;
                Err(io::Error::other("watched forwarding cleanup timed out"))
            }
        }
    }

    async fn counted(&self, headers: HeaderMap, body: Vec<u8>) -> Result<(), ()> {
        let result = self
            .0
            .budget
            .bound(async {
                let sent = self
                    .0
                    .budget
                    .forward(RequestKind::Count, || {
                        self.0
                            .client
                            .post(format!(
                                "{}/v1/messages/count_tokens?beta=true",
                                self.0.upstream
                            ))
                            .headers(headers)
                            .body(body)
                            .send()
                    })
                    .await
                    .map_err(|_| ())?;
                let mut response = sent.map_err(|_| ())?;
                if !response.status().is_success() {
                    return Err(());
                }
                let mut raw = Vec::new();
                while let Some(bytes) = response.chunk().await.map_err(|_| ())? {
                    if raw.len().saturating_add(bytes.len()) > COUNT_REPLY_LIMIT {
                        return Err(());
                    }
                    raw.extend_from_slice(&bytes);
                }
                strict_json::parse(&raw)?["input_tokens"]
                    .as_u64()
                    .filter(|n| *n > 0 && *n <= COUNT_LIMIT)
                    .ok_or(())?;
                Ok(())
            })
            .await;
        result.map_err(|_| ())?
    }
}

async fn request_boundary(
    State(gateway): State<WatchedGateway>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    // Include incoming-body extraction in cancellation/deadline handling. SSE
    // body reads remain separately bound after this response has been built.
    gateway
        .0
        .budget
        .bound(next.run(request))
        .await
        .unwrap_or_else(|_| deny(StatusCode::SERVICE_UNAVAILABLE))
}

/// Owned by the response body. Drop records each generation once, including
/// partial usage on disconnect/cancellation. No request text or headers survive.
struct Accounting {
    report: Mutex<LaneAReport>,
    observer: UsageObserver,
    totals: Arc<Mutex<Totals>>,
}

impl Accounting {
    fn feed(&mut self, event: &[u8]) -> Result<(), ()> {
        self.observer.feed_report(event, &self.report);
        let report = self.report.lock().map_err(|_| ())?;
        if self.observer.invalid || report.prompt_over_limit || report.output_over_limit {
            Err(())
        } else {
            Ok(())
        }
    }

    fn finish(&mut self) -> Result<(), ()> {
        std::mem::take(&mut self.observer).finish_report(&self.report);
        let report = self.report.lock().map_err(|_| ())?;
        if report.accounting_unknown {
            Err(())
        } else {
            Ok(())
        }
    }
}

impl Drop for Accounting {
    fn drop(&mut self) {
        let (Ok(report), Ok(mut totals)) = (self.report.lock(), self.totals.lock()) else {
            return;
        };
        totals.tokens.input = totals
            .tokens
            .input
            .saturating_add(report.input_tokens.unwrap_or(0));
        totals.tokens.cache_creation = totals
            .tokens
            .cache_creation
            .saturating_add(report.cache_creation_input_tokens.unwrap_or(0));
        totals.tokens.cache_read = totals
            .tokens
            .cache_read
            .saturating_add(report.cache_read_input_tokens.unwrap_or(0));
        totals.tokens.output = totals
            .tokens
            .output
            .saturating_add(report.output_tokens.unwrap_or(0));
        if report.stream_finished && !report.accounting_unknown {
            totals.complete = totals.complete.saturating_add(1);
        }
    }
}

async fn messages(
    State(gateway): State<WatchedGateway>,
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
    if strict_json::parse(&raw)
        .ok()
        .and_then(|v| v["stream"].as_bool())
        != Some(true)
    {
        return deny(StatusCode::BAD_REQUEST);
    }
    let Ok(body) = checked_body(&raw) else {
        return deny(StatusCode::BAD_REQUEST);
    };
    let Ok((count, _)) = count_projection(&body) else {
        return deny(StatusCode::BAD_REQUEST);
    };
    if gateway.counted(headers.clone(), count).await.is_err() {
        let _ = gateway.stop();
        return deny(StatusCode::BAD_GATEWAY);
    }
    let protected = ProtectedValues::new(&headers);
    let sent = gateway
        .0
        .budget
        .bound(gateway.0.budget.forward(RequestKind::Generation, || {
            gateway
                .0
                .client
                .post(format!("{}/v1/messages?beta=true", gateway.0.upstream))
                .headers(headers)
                .body(body)
                .send()
        }))
        .await;
    let Ok(Ok(Ok(upstream))) = sent else {
        let _ = gateway.stop();
        return deny(StatusCode::BAD_GATEWAY);
    };
    let accounting = Accounting {
        report: Mutex::new(LaneAReport {
            accounting_unknown: true,
            ..LaneAReport::default()
        }),
        observer: UsageObserver::default(),
        totals: gateway.0.totals.clone(),
    };
    let is_sse = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("text/event-stream"));
    if !upstream.status().is_success() || !is_sse {
        let _ = gateway.stop();
        return deny(StatusCode::BAD_GATEWAY);
    }
    // No upstream headers except the fixed public media type are relayed.
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let failed = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let mut pumps = gateway.0.pumps.lock().await;
    // A bounded launcher timeout may cancel the HTTP owner. Under the same
    // registry lock as drain, never register another pump after run stop.
    if gateway.0.budget.usage().map_or(true, |usage| usage.stopped) {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
    pumps.spawn(forward_pump(
        gateway.clone(),
        upstream.bytes_stream(),
        accounting,
        protected,
        sender,
        failed.clone(),
    ));
    drop(pumps);
    let bytes = stream::unfold(
        (receiver, failed, false),
        |(mut receiver, failed, ended)| async move {
            if ended {
                return None;
            }
            match receiver.recv().await {
                Some(bytes) => Some((Ok::<_, io::Error>(bytes), (receiver, failed, false))),
                None if failed.load(std::sync::atomic::Ordering::SeqCst) => Some((
                    Err(io::Error::other("watched response refused")),
                    (receiver, failed, true),
                )),
                None => None,
            }
        },
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(bytes))
        .unwrap()
}

async fn forward_pump<S, E>(
    gateway: WatchedGateway,
    mut source: S,
    mut accounting: Accounting,
    mut protected: ProtectedValues,
    sender: tokio::sync::mpsc::Sender<Bytes>,
    failed: Arc<std::sync::atomic::AtomicBool>,
) where
    S: futures_util::Stream<Item = Result<Bytes, E>> + Unpin,
{
    let result: Result<(), ()> = async {
        loop {
            let next = gateway
                .0
                .budget
                .bound(async {
                    tokio::select! {
                        biased;
                        _ = sender.closed() => Err(()),
                        next = source.next() => Ok(next),
                    }
                })
                .await
                .map_err(|_| ())??;
            let (bytes, finished) = match next {
                Some(Ok(bytes)) => (
                    protected.push_with(&bytes, |event| accounting.feed(event))?,
                    false,
                ),
                Some(Err(_)) => return Err(()),
                None => {
                    let bytes = protected.finish_with(|event| accounting.feed(event))?;
                    accounting.finish()?;
                    (bytes, true)
                }
            };
            if !bytes.is_empty() {
                // A full downstream queue cannot prevent stop or wall expiry
                // from dropping source. Response drop also wakes this send.
                gateway
                    .0
                    .budget
                    .bound(sender.send(bytes))
                    .await
                    .map_err(|_| ())?
                    .map_err(|_| ())?;
            }
            if finished {
                return Ok(());
            }
        }
    }
    .await;
    if result.is_err() {
        let _ = gateway.stop();
    }
    // Record once before closing the channel so receiver EOF observes finalized
    // accounting. The failure marker cannot be lost behind a full data queue.
    drop(source);
    drop(accounting);
    failed.store(result.is_err(), std::sync::atomic::Ordering::SeqCst);
    drop(sender);
}

#[cfg(test)]
#[path = "watched_gateway_cancel_tests.rs"]
mod cancellation_review;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_budget::RequestLimits;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::{
        sync::{Notify, oneshot},
        task::JoinHandle,
    };

    #[derive(Clone, Copy)]
    enum Mode {
        Complete,
        CountStall,
        StreamStall,
        Overestimate,
        Reflection,
    }

    struct Fake {
        counts: AtomicU64,
        generations: AtomicU64,
        started: Notify,
        mode: Mode,
    }

    async fn count(State(fake): State<Arc<Fake>>) -> Response {
        fake.counts.fetch_add(1, Ordering::SeqCst);
        if matches!(fake.mode, Mode::CountStall) {
            fake.started.notify_one();
            return Response::builder()
                .body(Body::from_stream(
                    stream::pending::<Result<Bytes, io::Error>>(),
                ))
                .unwrap();
        }
        let tokens = if matches!(fake.mode, Mode::Overestimate) {
            COUNT_LIMIT + 1
        } else {
            10
        };
        Response::builder()
            .body(Body::from(format!("{{\"input_tokens\":{tokens}}}")))
            .unwrap()
    }

    async fn generate(State(fake): State<Arc<Fake>>, headers: HeaderMap) -> Response {
        fake.generations.fetch_add(1, Ordering::SeqCst);
        fake.started.notify_one();
        let body = match fake.mode {
            Mode::StreamStall => Body::from_stream(
                stream::once(async {
                    Ok::<_, io::Error>(Bytes::from_static(
                        b"event: ping\ndata: {\"type\":\"ping\"}\n\n",
                    ))
                })
                .chain(stream::pending::<Result<Bytes, io::Error>>()),
            ),
            Mode::Reflection => Body::from(format!(
                "data: {{\"type\":\"content_block_delta\",\"delta\":{{\"text\":{}}}}}\n\n",
                serde_json::to_string(headers["authorization"].to_str().unwrap()).unwrap()
            )),
            _ => Body::from(concat!(
                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_creation_input_tokens\":2,\"cache_read_input_tokens\":3,\"output_tokens\":1}}}\n\n",
                "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":7}}\n\n",
                "data: {\"type\":\"message_stop\"}\n\n"
            )),
        };
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(body)
            .unwrap()
    }

    struct Fixture {
        gateway: WatchedGateway,
        fake: Arc<Fake>,
        url: String,
        client: reqwest::Client,
        stops: Vec<oneshot::Sender<()>>,
        joins: Vec<JoinHandle<io::Result<()>>>,
    }

    impl Fixture {
        async fn new(mode: Mode, wall: Duration) -> Self {
            let fake = Arc::new(Fake {
                counts: AtomicU64::new(0),
                generations: AtomicU64::new(0),
                started: Notify::new(),
                mode,
            });
            let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", upstream.local_addr().unwrap());
            let router = Router::new()
                .route("/v1/messages/count_tokens", post(count))
                .route("/v1/messages", post(generate))
                .with_state(fake.clone());
            let (stop_fake, done_fake) = oneshot::channel();
            let fake_task = tokio::spawn(async move {
                axum::serve(upstream, router)
                    .with_graceful_shutdown(async {
                        let _ = done_fake.await;
                    })
                    .await
            });
            let budget = RequestBudget::new(RequestLimits {
                total: 4,
                generations: 2,
                wall,
            })
            .unwrap();
            let gateway = WatchedGateway::build(budget, base).unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
            let (stop_gateway, done_gateway) = oneshot::channel();
            let copy = gateway.clone();
            let gateway_task = tokio::spawn(copy.serve(listener, async {
                let _ = done_gateway.await;
            }));
            Self {
                gateway,
                fake,
                url,
                client: reqwest::Client::builder().no_proxy().build().unwrap(),
                stops: vec![stop_gateway, stop_fake],
                joins: vec![gateway_task, fake_task],
            }
        }

        fn request(&self) -> reqwest::RequestBuilder {
            self.client.post(&self.url)
                .header("authorization", "Bearer synthetic-watched-credential")
                .header("anthropic-beta", "synthetic-beta")
                .header("anthropic-version", "2023-06-01")
                .json(&serde_json::json!({"model":crate::calibration::MODEL,"max_tokens":128000,"messages":[{"role":"user","content":"offline fixture"}],"stream":true}))
        }

        async fn close(self) {
            let _ = self.gateway.stop();
            for stop in self.stops {
                let _ = stop.send(());
            }
            for task in self.joins {
                tokio::time::timeout(Duration::from_secs(3), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
            }
        }
    }

    #[tokio::test]
    async fn counted_generation_forwards_share_cap_and_sum_numeric_channels() {
        let fixture = Fixture::new(Mode::Complete, Duration::from_secs(10)).await;
        for _ in 0..2 {
            let response = fixture.request().send().await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(!response.bytes().await.unwrap().is_empty());
        }
        assert_eq!(fixture.fake.counts.load(Ordering::SeqCst), 2);
        assert_eq!(fixture.fake.generations.load(Ordering::SeqCst), 2);
        let report = fixture.gateway.report().unwrap();
        assert_eq!(report.requests.total, 4);
        assert_eq!(report.requests.generations, 2);
        assert_eq!(report.complete_generations, 2);
        assert_eq!(report.unknown_generations, 0);
        assert_eq!(
            report.tokens,
            TokenLowerBounds {
                input: 20,
                cache_creation: 4,
                cache_read: 6,
                output: 14
            }
        );
        assert!(
            !fixture
                .request()
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
        assert_eq!(fixture.fake.counts.load(Ordering::SeqCst), 2);
        assert_eq!(fixture.fake.generations.load(Ordering::SeqCst), 2);
        fixture.close().await;
    }

    #[tokio::test]
    async fn ten_concurrent_clients_cannot_exceed_shared_upstream_cap() {
        let fixture = Fixture::new(Mode::Complete, Duration::from_secs(10)).await;
        let mut tasks = Vec::new();
        for _ in 0..10 {
            let request = fixture.request();
            tasks.push(tokio::spawn(async move {
                if let Ok(response) = request.send().await {
                    let _ = response.bytes().await;
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        let report = fixture.gateway.report().unwrap();
        let counts = fixture.fake.counts.load(Ordering::SeqCst);
        let generations = fixture.fake.generations.load(Ordering::SeqCst);
        assert!(counts + generations <= report.requests.total);
        assert_eq!(report.requests.total, 4);
        assert!(generations <= 2);
        assert!(report.requests.stopped);
        fixture.close().await;
    }

    #[tokio::test]
    async fn duplicate_body_and_unsupported_endpoint_never_reach_upstream() {
        let fixture = Fixture::new(Mode::Complete, Duration::from_secs(10)).await;
        let response = fixture
            .request()
            .body(r#"{"stream":true,"stream":false}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = fixture
            .client
            .post(format!("{}/count_tokens", fixture.url))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(fixture.fake.counts.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.fake.generations.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.gateway.report().unwrap().requests.total, 0);
        fixture.close().await;
    }

    #[tokio::test]
    async fn overestimate_spends_count_but_never_constructs_generation() {
        let fixture = Fixture::new(Mode::Overestimate, Duration::from_secs(10)).await;
        assert_eq!(
            fixture.request().send().await.unwrap().status(),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(fixture.fake.counts.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.fake.generations.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.gateway.report().unwrap().requests.total, 1);
        fixture.close().await;
    }

    #[tokio::test]
    async fn deadline_covers_stalled_count_body_after_headers() {
        let fixture = Fixture::new(Mode::CountStall, Duration::from_secs(1)).await;
        let response = tokio::time::timeout(Duration::from_secs(3), fixture.request().send())
            .await
            .unwrap()
            .unwrap();
        assert!(!response.status().is_success());
        assert_eq!(fixture.fake.generations.load(Ordering::SeqCst), 0);
        assert!(fixture.gateway.report().unwrap().requests.stopped);
        fixture.close().await;
    }

    #[tokio::test]
    async fn deadline_covers_stream_and_preserves_unknown_usage_and_debit() {
        let fixture = Fixture::new(Mode::StreamStall, Duration::from_secs(1)).await;
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            fixture.request().send().await?.bytes().await
        })
        .await
        .unwrap();
        assert!(result.is_err());
        let report = fixture.gateway.report().unwrap();
        assert_eq!(report.requests.total, 2);
        assert_eq!(report.unknown_generations, 1);
        assert_eq!(report.complete_generations, 0);
        assert!(report.requests.stopped);
        fixture.close().await;
    }

    #[tokio::test]
    async fn explicit_stop_cancels_active_stream_and_join_finishes() {
        let fixture = Fixture::new(Mode::StreamStall, Duration::from_secs(10)).await;
        let response = fixture.request().send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut source = response.bytes_stream();
        // Confirm the streamed body is active before stopping; a signal from
        // the upstream handler can race with response-header cancellation.
        let first = tokio::time::timeout(Duration::from_secs(3), source.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!first.is_empty());
        fixture.gateway.stop().unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(3), source.next())
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        drop(source);
        assert_eq!(fixture.gateway.report().unwrap().requests.total, 2);
        fixture.close().await;
    }

    #[tokio::test]
    async fn reflected_credential_event_is_withheld_and_run_stops() {
        let fixture = Fixture::new(Mode::Reflection, Duration::from_secs(10)).await;
        let result = async { fixture.request().send().await?.bytes().await }.await;
        assert!(result.is_err());
        assert!(fixture.gateway.report().unwrap().requests.stopped);
        assert_eq!(fixture.gateway.report().unwrap().unknown_generations, 1);
        fixture.close().await;
    }
}
