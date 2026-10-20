//! Local HTTP controls for the new calibration composition. No native CLI/login.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::{sync::oneshot, task::JoinHandle};

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct Fixture {
    root: std::path::PathBuf,
    fence: LaneAFence,
    counts: Arc<AtomicU64>,
    generations: Arc<AtomicU64>,
    dropped: Arc<AtomicBool>,
    stop: Option<oneshot::Sender<()>>,
    server: Option<JoinHandle<()>>,
}

impl Fixture {
    async fn start(count: u64, stalled: bool, wall: Duration) -> Self {
        let root =
            std::env::temp_dir().join(format!("cna-calibration-runtime-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("one.sqlite");
        CalibrationFence::initialize(&path).unwrap();
        let counts = Arc::new(AtomicU64::new(0));
        let generations = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let count_copy = counts.clone();
        let gen_copy = generations.clone();
        let drop_copy = dropped.clone();
        let app = Router::new()
            .route("/v1/messages/count_tokens", post(move || {
                count_copy.fetch_add(1, Ordering::SeqCst);
                async move { axum::Json(serde_json::json!({"input_tokens":count})) }
            }))
            .route("/v1/messages", post(move || {
                gen_copy.fetch_add(1, Ordering::SeqCst);
                let marker = Dropped(drop_copy.clone());
                async move {
                    let body = if stalled {
                        Body::from_stream(stream::unfold((marker, false), |(marker, sent)| async move {
                            if !sent { Some((Ok::<_, io::Error>(Bytes::from_static(b"event: ping\ndata: {\"type\":\"ping\"}\n\n")), (marker, true))) }
                            else { let _held = marker; std::future::pending::<Option<(Result<Bytes, io::Error>, (Dropped, bool))>>().await }
                        }))
                    } else {
                        drop(marker);
                        Body::from("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0}}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":2}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
                    };
                    Response::builder().header(header::CONTENT_TYPE,"text/event-stream").body(body).unwrap()
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        let fence = LaneAFence::build_configured(
            &path,
            upstream,
            &crate::calibration_cost::source_fixture_prices(),
            wall,
        )
        .unwrap();
        Self {
            root,
            fence,
            counts,
            generations,
            dropped,
            stop: Some(stop),
            server: Some(server),
        }
    }

    async fn request(&self) -> Response {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            "Bearer SYNTHETIC_CALIBRATION_ONLY".parse().unwrap(),
        );
        headers.insert("anthropic-beta", "oauth-2025-04-20".parse().unwrap());
        headers.insert("anthropic-version", "2023-06-01".parse().unwrap());
        let raw = serde_json::to_vec(&serde_json::json!({"model":crate::calibration::MODEL,"max_tokens":128000,"stream":true,"messages":[{"role":"user","content":"Local fixture only"}]})).unwrap();
        messages(
            State(self.fence.clone()),
            OriginalUri("/v1/messages".parse().unwrap()),
            headers,
            raw.into(),
        )
        .await
    }

    async fn shutdown(&mut self) -> bool {
        let pumps = self.fence.shutdown_pumps().await.is_ok();
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let mut joined = true;
        if let Some(mut server) = self.server.take() {
            match tokio::time::timeout(Duration::from_secs(2), &mut server).await {
                Ok(result) => joined = result.is_ok(),
                Err(_) => {
                    joined = false;
                    server.abort();
                    let _ = server.await;
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
        pumps && joined
    }
}

#[tokio::test]
async fn original_one_generation_and_fifth_count_refusal_have_exact_transport_totals() {
    let mut fixture = Fixture::start(3, false, Duration::from_secs(30)).await;
    let response = fixture.request().await;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024).await;
    let denied = fixture.request().await.status();
    let report = fixture.fence.report();
    let counts = fixture.counts.load(Ordering::SeqCst);
    let generations = fixture.generations.load(Ordering::SeqCst);
    let joined = fixture.shutdown().await;
    assert!(joined && body.is_ok());
    assert_eq!(status, StatusCode::OK);
    assert_eq!(denied, StatusCode::CONFLICT);
    assert_eq!((counts, generations), (1, 1));
    let report = report.unwrap();
    assert_eq!(
        (
            report.count_forwards,
            report.generation_forwards,
            report.transport_total
        ),
        (1, 1, 2)
    );
    assert!(report.generation_permit_spent && report.stream_finished && !report.accounting_unknown);

    let mut fixture = Fixture::start(95_001, false, Duration::from_secs(30)).await;
    let mut statuses = Vec::new();
    for _ in 0..5 {
        statuses.push(fixture.request().await.status());
    }
    let report = fixture.fence.report();
    let counts = fixture.counts.load(Ordering::SeqCst);
    let generations = fixture.generations.load(Ordering::SeqCst);
    let joined = fixture.shutdown().await;
    assert!(joined);
    assert_eq!(
        statuses,
        vec![
            StatusCode::UNPROCESSABLE_ENTITY,
            StatusCode::UNPROCESSABLE_ENTITY,
            StatusCode::UNPROCESSABLE_ENTITY,
            StatusCode::UNPROCESSABLE_ENTITY,
            StatusCode::CONFLICT
        ]
    );
    assert_eq!((counts, generations), (4, 0));
    let report = report.unwrap();
    assert_eq!(
        (
            report.count_forwards,
            report.generation_forwards,
            report.transport_total
        ),
        (4, 0, 4)
    );
}

#[tokio::test]
async fn unpolled_response_stop_wall_and_drop_cancel_upstream_and_join() {
    for mode in ["stop", "wall", "drop"] {
        let wall = if mode == "wall" {
            Duration::from_secs(2)
        } else {
            Duration::from_secs(30)
        };
        let mut fixture = Fixture::start(3, true, wall).await;
        let response = fixture.request().await;
        let status = response.status();
        let mut response = Some(response);
        if mode == "stop" {
            fixture.fence.stop();
        }
        if mode == "drop" {
            drop(response.take());
        }
        let observed = tokio::time::timeout(Duration::from_secs(4), async {
            while !fixture.dropped.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .is_ok();
        drop(response);
        let report = fixture.fence.report();
        let joined = fixture.shutdown().await;
        assert!(observed && joined, "mode {mode}");
        assert_eq!(status, StatusCode::OK);
        let report = report.unwrap();
        assert_eq!(report.transport_total, 2);
        assert!(report.generation_permit_spent && report.accounting_unknown);
    }
}

#[test]
fn absent_or_over_two_dollar_prices_refuse_before_ledger_or_transport() {
    let mut prices = crate::calibration_cost::source_fixture_prices();
    prices.high_cache_write_1h_micro_usd_per_mtok = 2_000_000;
    assert!(
        LaneAFence::build_configured(
            Path::new("C:/UNOPENED/ledger"),
            "http://127.0.0.1:1".into(),
            &prices,
            Duration::from_secs(1)
        )
        .is_err()
    );
    prices = crate::calibration_cost::source_fixture_prices();
    prices.verified_on.clear();
    assert!(
        LaneAFence::build_configured(
            Path::new("C:/UNOPENED/ledger"),
            "http://127.0.0.1:1".into(),
            &prices,
            Duration::from_secs(1)
        )
        .is_err()
    );
}
