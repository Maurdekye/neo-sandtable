use super::*;
use axum::http::HeaderValue;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Notify, oneshot};
const TOKEN: &str = "SYNTHETIC_AUTH_SECRET_REVIEW_92abcd";
#[derive(Clone)]
struct Plan {
    chunks: Arc<Vec<Vec<u8>>>,
    calls: Arc<AtomicUsize>,
    count_calls: Arc<AtomicUsize>,
    headers: Arc<Mutex<Option<HeaderMap>>>,
    gate: Arc<Notify>,
    wait: bool,
    count_tokens: u64,
}
async fn count(State(p): State<Plan>, _: Bytes) -> Response {
    p.count_calls.fetch_add(1, Ordering::SeqCst);
    Response::new(Body::from(
        json!({"input_tokens":p.count_tokens}).to_string(),
    ))
}
async fn generation(State(p): State<Plan>, headers: HeaderMap, _: Bytes) -> Response {
    p.calls.fetch_add(1, Ordering::SeqCst);
    *p.headers.lock().unwrap() = Some(headers);
    let stream = stream::unfold((p, 0usize), |(p, n)| async move {
        if n >= p.chunks.len() {
            return None;
        }
        if p.wait && n == 1 {
            p.gate.notified().await
        }
        Some((
            Ok::<Bytes, io::Error>(Bytes::from(p.chunks[n].clone())),
            (p, n + 1),
        ))
    });
    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}
fn frame(value: Value) -> Vec<u8> {
    format!("data: {}\n\n", value).into_bytes()
}
fn start(usage: Value) -> Vec<u8> {
    frame(json!({"type":"message_start","message":{"usage":usage}}))
}
fn stop() -> Vec<u8> {
    frame(json!({"type":"message_stop"}))
}
fn body() -> Value {
    json!({"model":crate::calibration::MODEL,"max_tokens":128000,"stream":true,"messages":[{"role":"user","content":"Independent synthetic request"}]})
}
struct Fixture {
    plan: Plan,
    fence: LaneAFence,
    url: String,
    upstream: String,
    path: std::path::PathBuf,
    client: reqwest::Client,
    stops: Vec<oneshot::Sender<()>>,
    joins: Vec<tokio::task::JoinHandle<io::Result<()>>>,
}
impl Fixture {
    async fn new(chunks: Vec<Vec<u8>>, wait: bool, count_tokens: u64) -> Self {
        let plan = Plan {
            chunks: Arc::new(chunks),
            calls: Arc::new(AtomicUsize::new(0)),
            count_calls: Arc::new(AtomicUsize::new(0)),
            headers: Arc::new(Mutex::new(None)),
            gate: Arc::new(Notify::new()),
            wait,
            count_tokens,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/v1/messages/count_tokens", post(count))
            .route("/v1/messages", post(generation))
            .with_state(plan.clone());
        let (s1, r1) = oneshot::channel();
        let j1 = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = r1.await;
                })
                .await
        });
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("synthetic-{}.sqlite", uuid::Uuid::new_v4()));
        CalibrationFence::initialize(&path).unwrap();
        let fence = LaneAFence::build(&path, upstream.clone()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let clone = fence.clone();
        let (s2, r2) = oneshot::channel();
        let j2 = tokio::spawn(async move {
            clone
                .serve(listener, async {
                    let _ = r2.await;
                })
                .await
        });
        Self {
            plan,
            fence,
            url,
            upstream,
            path,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            stops: vec![s1, s2],
            joins: vec![j1, j2],
        }
    }
    fn request(&self) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}/v1/messages?beta=true", self.url))
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("anthropic-beta", "oauth-safe-beta")
            .header("anthropic-version", "2023-06-01")
            .json(&body())
    }
    async fn close(self) {
        self.plan.gate.notify_waiters();
        drop(self.client);
        for stop in self.stops {
            let _ = stop.send(());
        }
        for join in self.joins {
            tokio::time::timeout(Duration::from_secs(3), join)
                .await
                .expect("inert loopback shutdown bound")
                .unwrap()
                .unwrap();
        }
        std::fs::remove_file(self.path).unwrap();
    }
}
#[tokio::test]
async fn clean_usage_and_verbatim_headers_control() {
    let f=Fixture::new(vec![start(json!({"input_tokens":94999,"cache_creation_input_tokens":1,"cache_read_input_tokens":0,"output_tokens":2})),stop()],false,95000).await;
    let response = f.request().send().await.unwrap();
    let bytes = response.bytes().await.unwrap();
    let report = f.fence.report().unwrap();
    let headers = f.plan.headers.lock().unwrap().clone().unwrap();
    let calls = f.plan.calls.load(Ordering::SeqCst);
    f.close().await;
    assert!(!bytes.is_empty());
    assert!(report.stream_finished && !report.accounting_unknown && !report.prompt_over_limit);
    assert_eq!(calls, 1);
    assert_eq!(headers["authorization"], format!("Bearer {TOKEN}"));
    assert_eq!(headers["anthropic-beta"], "oauth-safe-beta");
    assert_eq!(headers["anthropic-version"], "2023-06-01");
}
#[tokio::test]
async fn partial_and_overflow_usage_abort() {
    for usage in [
        json!({"input_tokens":100001,"output_tokens":1}),
        json!({"input_tokens":100001,"cache_creation_input_tokens":null,"cache_read_input_tokens":0,"output_tokens":1}),
        json!({"input_tokens":u64::MAX,"cache_creation_input_tokens":1,"cache_read_input_tokens":0,"output_tokens":1}),
    ] {
        let f = Fixture::new(vec![start(usage), stop()], false, 95000).await;
        let bytes = match f.request().send().await {
            Ok(response) => response.bytes().await,
            Err(error) => Err(error),
        };
        let report = f.fence.report().unwrap();
        let retry = f.request().send().await.unwrap().status();
        f.close().await;
        assert!(
            bytes.is_err(),
            "known usage lower bound must stop the stream"
        );
        assert!(report.prompt_over_limit);
        assert!(report.accounting_unknown);
        assert_eq!(retry, StatusCode::CONFLICT);
    }
}
#[tokio::test]
async fn known_partial_over_limit_should_abort() {
    let f = Fixture::new(
        vec![
            start(json!({"input_tokens":100001,"output_tokens":1})),
            stop(),
        ],
        false,
        95000,
    )
    .await;
    let bytes = match f.request().send().await {
        Ok(response) => response.bytes().await,
        Err(error) => Err(error),
    };
    let report = f.fence.report().unwrap();
    f.close().await;
    assert!(
        bytes.is_err() && report.prompt_over_limit,
        "known input alone exceeds100K: stream must stop even when cache channels are unknown"
    );
}
#[tokio::test]
async fn json_escaped_reflection_aborts() {
    let encoded = TOKEN
        .chars()
        .map(|c| format!("\\u{:04x}", c as u32))
        .collect::<String>();
    let reflected=format!("data: {{\"type\":\"content_block_delta\",\"delta\":{{\"type\":\"text_delta\",\"text\":\"{encoded}\"}}}}\n\n").into_bytes();
    let f=Fixture::new(vec![reflected,start(json!({"input_tokens":95000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":5})),stop()],false,95000).await;
    let bytes = match f.request().send().await {
        Ok(response) => response.bytes().await,
        Err(error) => Err(error),
    };
    let report = f.fence.report().unwrap();
    f.close().await;
    assert!(bytes.is_err());
    assert!(!report.stream_finished && report.accounting_unknown);
}
#[tokio::test]
async fn decoded_reflection_should_not_reach_caller() {
    let encoded = TOKEN
        .chars()
        .map(|c| format!("\\u{:04x}", c as u32))
        .collect::<String>();
    let reflected=format!("data: {{\"type\":\"content_block_delta\",\"delta\":{{\"type\":\"text_delta\",\"text\":\"{encoded}\"}}}}\n\n").into_bytes();
    let f = Fixture::new(vec![reflected, stop()], false, 95000).await;
    let bytes = match f.request().send().await {
        Ok(response) => response.bytes().await,
        Err(error) => Err(error),
    };
    f.close().await;
    assert!(
        bytes.is_err(),
        "JSON-unescaped Authorization token must not reach SSE caller"
    );
}
#[tokio::test]
async fn ping_then_cancel_reopen_preserves_one_generation() {
    let f=Fixture::new(vec![frame(json!({"type":"ping"})),start(json!({"input_tokens":95000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":5})),stop()],true,95000).await;
    let mut response = f.request().send().await.unwrap();
    let first = tokio::time::timeout(Duration::from_millis(500), response.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(first.windows(4).any(|w| w == b"ping"));
    drop(response);
    let retry = f.request().send().await.unwrap().status();
    let reopened = LaneAFence::build(&f.path, f.upstream.clone())
        .unwrap()
        .report()
        .unwrap();
    let original = f.fence.report().unwrap();
    let calls = f.plan.calls.load(Ordering::SeqCst);
    f.close().await;
    assert_eq!(retry, StatusCode::CONFLICT);
    assert_eq!(calls, 1);
    assert!(reopened.generation_permit_spent && reopened.accounting_unknown);
    assert!(original.accounting_unknown && !original.stream_finished);
}
#[test]
fn chunked_plain_reflection_and_overflow_accounting_control() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {TOKEN}")).unwrap(),
    );
    headers.insert(
        "anthropic-beta",
        HeaderValue::from_static("oauth-safe-beta"),
    );
    for boundary in 1..TOKEN.len() {
        let mut guard = ProtectedValues::new(&headers);
        let first = guard.push(&TOKEN.as_bytes()[..boundary]).unwrap();
        assert!(first.is_empty());
        assert!(guard.push(&TOKEN.as_bytes()[boundary..]).is_err());
    }
    assert!(assess_prompt_usage(Some(u64::MAX), Some(1), Some(0)).is_err());
}

#[tokio::test]
async fn normal_streaming_consumer_never_receives_decoded_header_values() {
    for secret in [TOKEN, "oauth-safe-beta"] {
        let encoded = secret
            .chars()
            .map(|c| format!("\\u{:04x}", c as u32))
            .collect::<String>();
        let attack = format!(
            "data: {{\"type\":\"content_block_delta\",\"delta\":{{\"text\":\"{encoded}\"}}}}\n\n"
        )
        .into_bytes();
        let mut chunks = vec![b": ping\n\n".to_vec()];
        chunks.extend(attack.iter().map(|b| vec![*b]));
        chunks.push(stop());
        let f = Fixture::new(chunks, true, 95000).await;
        let mut response = f.request().send().await.unwrap();
        let first = response.chunk().await.unwrap().unwrap();
        let mut delivered = first.to_vec();
        f.plan.gate.notify_one();
        let mut error = false;
        loop {
            match response.chunk().await {
                Ok(Some(bytes)) => delivered.extend_from_slice(&bytes),
                Ok(None) => break,
                Err(_) => {
                    error = true;
                    break;
                }
            }
        }
        let report = f.fence.report().unwrap();
        drop(response);
        f.close().await;
        assert_eq!(first.as_ref(), b": ping\n\n");
        assert!(error && !report.stream_finished && report.accounting_unknown);
        let mut decoded = String::new();
        for line in delivered.split(|b| *b == b'\n') {
            if let Some(data) = line.strip_prefix(b"data: ")
                && let Ok(value) = serde_json::from_slice::<Value>(data)
                && let Some(text) = value["delta"]["text"].as_str()
            {
                decoded.push_str(text);
            }
        }
        assert!(!decoded.contains(TOKEN) && !decoded.contains("oauth-safe-beta"));
    }
}
#[tokio::test]
async fn incomplete_known_usage_is_not_measured_zero_and_each_channel_stops() {
    for usage in [
        json!({"cache_read_input_tokens":100001}),
        json!({"cache_creation_input_tokens":100001}),
        json!({"input_tokens":60000,"cache_read_input_tokens":40001,"cache_creation_input_tokens":null}),
    ] {
        let f = Fixture::new(vec![start(usage), stop()], false, 95000).await;
        let result = match f.request().send().await {
            Ok(r) => r.bytes().await,
            Err(e) => Err(e),
        };
        let report = f.fence.report().unwrap();
        f.close().await;
        assert!(result.is_err() && report.prompt_over_limit && report.accounting_unknown);
    }
    let f = Fixture::new(
        vec![
            start(json!({"input_tokens":90000,"output_tokens":1})),
            stop(),
        ],
        false,
        95000,
    )
    .await;
    let result = f.request().send().await.unwrap().bytes().await;
    let report = f.fence.report().unwrap();
    f.close().await;
    assert!(result.is_ok() && report.stream_finished && report.accounting_unknown);
    assert_eq!(report.input_tokens, Some(90000));
    assert_eq!(report.cache_creation_input_tokens, None);
    assert_eq!(report.cache_read_input_tokens, None);
    assert!(!report.prompt_over_limit);
}
#[tokio::test]
async fn cr_only_usage_crossing_is_sticky_and_retry_stays_spent() {
    let wire = start(json!({"input_tokens":100001,"output_tokens":1}));
    let wire = wire
        .into_iter()
        .map(|b| if b == b'\n' { b'\r' } else { b })
        .collect();
    let f = Fixture::new(vec![wire, stop()], false, 95000).await;
    let result = match f.request().send().await {
        Ok(r) => r.bytes().await,
        Err(e) => Err(e),
    };
    let report = f.fence.report().unwrap();
    let retry = f.request().send().await.unwrap().status();
    f.close().await;
    assert!(result.is_err() && report.prompt_over_limit && report.accounting_unknown);
    assert_eq!(retry, StatusCode::CONFLICT);
}

#[tokio::test]
async fn late_error_keeps_unknown_accounting_and_spent_permit_after_cleanup() {
    let f = Fixture::new(vec![
        start(json!({"input_tokens":95000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":2})),
        frame(json!({"type":"error","error":{"type":"synthetic_late_error"}})),
        stop(),
    ], false, 95000).await;
    let bytes = f.request().send().await.unwrap().bytes().await;
    let report = f.fence.report().unwrap();
    let retry = f.request().send().await.unwrap().status();
    let calls = f.plan.calls.load(Ordering::SeqCst);
    f.close().await;
    assert!(bytes.is_ok() && report.stream_finished && report.accounting_unknown);
    assert_eq!(report.input_tokens, Some(95000));
    assert_eq!(retry, StatusCode::CONFLICT);
    assert_eq!(calls, 1);
}
#[tokio::test]
async fn initial_bom_and_cr_only_complete_usage_remain_measured() {
    let mut wire = b"\xef\xbb\xbf".to_vec();
    wire.extend(start(json!({"input_tokens":95000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":2})));
    wire.extend(stop());
    let wire = wire
        .into_iter()
        .map(|b| if b == b'\n' { b'\r' } else { b })
        .collect();
    let f = Fixture::new(vec![wire], false, 95000).await;
    let bytes = f.request().send().await.unwrap().bytes().await;
    let report = f.fence.report().unwrap();
    f.close().await;
    assert!(bytes.is_ok() && report.stream_finished && !report.accounting_unknown);
    assert_eq!(report.input_tokens, Some(95000));
}
