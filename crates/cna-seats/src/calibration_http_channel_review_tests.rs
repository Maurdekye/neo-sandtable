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
async fn lower_bound_over_limit_witness() {
    for usage in [
        json!({"input_tokens":100001,"output_tokens":1}),
        json!({"input_tokens":100001,"cache_creation_input_tokens":null,"cache_read_input_tokens":0,"output_tokens":1}),
        json!({"input_tokens":u64::MAX,"cache_creation_input_tokens":1,"cache_read_input_tokens":0,"output_tokens":1}),
    ] {
        let f = Fixture::new(vec![start(usage), stop()], false, 95000).await;
        let bytes = match f.request().send().await {
            Ok(r) => r.bytes().await,
            Err(e) => Err(e),
        };
        let report = f.fence.report().unwrap();
        let retry = f.request().send().await.unwrap().status();
        f.close().await;
        assert!(bytes.is_err(), "repaired lower bound must refuse");
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
        Ok(r) => r.bytes().await,
        Err(e) => Err(e),
    };
    let report = f.fence.report().unwrap();
    f.close().await;
    assert!(
        bytes.is_err() && report.prompt_over_limit,
        "known input alone exceeds100K: stream must stop even when cache channels are unknown"
    );
}
#[tokio::test]
async fn json_escaped_reflection_witness() {
    let encoded = TOKEN
        .chars()
        .map(|c| format!("\\u{:04x}", c as u32))
        .collect::<String>();
    let reflected=format!("data: {{\"type\":\"content_block_delta\",\"delta\":{{\"type\":\"text_delta\",\"text\":\"{encoded}\"}}}}\n\n").into_bytes();
    let f=Fixture::new(vec![reflected,start(json!({"input_tokens":95000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":5})),stop()],false,95000).await;
    let bytes = match f.request().send().await {
        Ok(r) => r.bytes().await,
        Err(e) => Err(e),
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
        Ok(r) => r.bytes().await,
        Err(e) => Err(e),
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
fn encode(value: &str) -> String {
    value
        .chars()
        .map(|c| format!("\\u{:04x}", c as u32))
        .collect()
}
fn delta(kind: &str, key: &str, value: &str, index: u64) -> Vec<u8> {
    format!("data: {{\"type\":\"content_block_delta\",\"index\":{index},\"delta\":{{\"type\":\"{kind}\",\"{key}\":\"{}\"}}}}\n\n",encode(value)).into_bytes()
}
fn headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    h.insert("anthropic-beta", "oauth-safe-beta".parse().unwrap());
    h
}
fn strings(bytes: &[u8], key: &str) -> String {
    let mut result = String::new();
    for line in bytes.split(|b| *b == b'\n') {
        if let Some(data) = line.strip_prefix(b"data: ")
            && let Ok(v) = serde_json::from_slice::<Value>(data)
            && let Some(s) = v["delta"][key].as_str()
        {
            result.push_str(s)
        }
    }
    result
}
#[test]
fn separate_text_and_thinking_channels_should_never_reconstruct_secret() {
    let mut g = ProtectedValues::new(&headers());
    let mut delivered = Vec::new();
    let mut refused = false;
    for chunk in [
        block_start(0, "text"),
        delta("text_delta", "text", &TOKEN[..12], 0),
        block_stop(0),
        block_start(1, "thinking"),
        delta(
            "thinking_delta",
            "thinking",
            "safe intermediate reasoning",
            1,
        ),
        block_stop(1),
        block_start(2, "text"),
        delta("text_delta", "text", &TOKEN[12..], 2),
    ] {
        match g.push(&chunk) {
            Ok(b) => delivered.extend_from_slice(&b),
            Err(()) => {
                refused = true;
                break;
            }
        }
    }
    assert!(
        refused && !strings(&delivered, "text").contains(TOKEN),
        "normal text-only assembly must not recover protected value"
    );
}
#[test]
fn independently_bytewise_mixed_escaping_and_both_headers_refuse() {
    for secret in [TOKEN, "oauth-safe-beta"] {
        for key in ["text", "thinking"] {
            for cut in 1..secret.len() {
                let mut g = ProtectedValues::new(&headers());
                g.push(&block_start(0, key)).unwrap();
                let kind = if key == "text" {
                    "text_delta"
                } else {
                    "thinking_delta"
                };
                let mut refused = false;
                let mut delivered = Vec::new();
                let mut wire = delta(kind, key, &secret[..cut], 0);
                wire.extend(delta(kind, key, &secret[cut..], 0));
                for b in wire {
                    match g.push(&[b]) {
                        Ok(x) => delivered.extend_from_slice(&x),
                        Err(()) => {
                            refused = true;
                            break;
                        }
                    }
                }
                assert!(refused);
                assert!(!strings(&delivered, key).contains(secret));
            }
        }
    }
}
#[test]
fn safe_multiline_crlf_partial_event_unicode_and_eof_controls() {
    let mut g = ProtectedValues::new(&headers());
    let chunks: [&[u8]; 4] = [
        b": ping\r\n\r\n",
        b"data: {\r\ndata: \"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"clear ",
        b"\\ud83d",
        b"\\ude00!\"}}\r\n\r\n",
    ];
    g.push(&block_start(0, "text")).unwrap();
    let mut all = Vec::new();
    for b in chunks {
        all.extend_from_slice(&g.push(b).unwrap());
    }
    assert!(g.tail.is_empty());
    assert_eq!(all, chunks.concat());
    let mut g = ProtectedValues::new(&headers());
    g.push(&block_start(0, "text")).unwrap();
    let partial = delta("text_delta", "text", &TOKEN[..12], 0);
    g.push(&partial).unwrap();
    assert!(!g.tail.is_empty());
    let mut g = ProtectedValues::new(&headers());
    assert!(g.push(b"data: {\"text\":\"\\ud83d").is_ok());
    assert!(!g.tail.is_empty());
}
#[tokio::test]
async fn actual_loopback_separate_channels_should_not_leak() {
    let chunks = vec![
        start(
            json!({"input_tokens":95000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":5}),
        ),
        block_start(0, "text"),
        delta("text_delta", "text", &TOKEN[..12], 0),
        block_stop(0),
        block_start(1, "thinking"),
        delta(
            "thinking_delta",
            "thinking",
            "safe intermediate reasoning",
            1,
        ),
        block_stop(1),
        block_start(2, "text"),
        delta("text_delta", "text", &TOKEN[12..], 2),
        stop(),
    ];
    let f = Fixture::new(chunks, false, 95000).await;
    let received = match f.request().send().await {
        Ok(r) => r.bytes().await,
        Err(e) => Err(e),
    };
    let report = f.fence.report().unwrap();
    let retry = f.request().send().await.unwrap().status();
    let count = f.plan.calls.load(Ordering::SeqCst);
    f.close().await;
    let leaked = received
        .as_ref()
        .is_ok_and(|b| strings(b, "text").contains(TOKEN));
    assert_eq!(retry, StatusCode::CONFLICT);
    assert_eq!(count, 1);
    assert!(
        !leaked,
        "normal separate-channel assembly must not receive protected value; stream completed={}",
        report.stream_finished
    );
}
#[tokio::test]
async fn independent_all_partial_channels_and_exact_boundary_accounting() {
    for usage in [
        json!({"input_tokens":100001}),
        json!({"cache_creation_input_tokens":100001}),
        json!({"cache_read_input_tokens":100001}),
        json!({"input_tokens":60000,"cache_read_input_tokens":40001}),
        json!({"input_tokens":u64::MAX,"cache_read_input_tokens":2}),
    ] {
        let f = Fixture::new(vec![start(usage), stop()], false, 95000).await;
        let received = match f.request().send().await {
            Ok(r) => r.bytes().await,
            Err(e) => Err(e),
        };
        let report = f.fence.report().unwrap();
        let retry = f.request().send().await.unwrap().status();
        f.close().await;
        assert!(received.is_err() && report.prompt_over_limit && report.accounting_unknown);
        assert_eq!(retry, StatusCode::CONFLICT);
    }
    let f=Fixture::new(vec![start(json!({"input_tokens":95000,"cache_creation_input_tokens":0,"cache_read_input_tokens":5000,"output_tokens":128000})),stop()],false,95000).await;
    let received = f.request().send().await.unwrap().bytes().await;
    let report = f.fence.report().unwrap();
    f.close().await;
    assert!(
        received.is_ok()
            && !report.prompt_over_limit
            && !report.output_over_limit
            && !report.accounting_unknown
    );
}

fn block_start(index: u64, kind: &str) -> Vec<u8> {
    let block = if kind == "text" {
        json!({"type":"text","text":""})
    } else {
        json!({"type":"thinking","thinking":"","signature":""})
    };
    frame(json!({"type":"content_block_start","index":index,"content_block":block}))
}
fn block_stop(index: u64) -> Vec<u8> {
    frame(json!({"type":"content_block_stop","index":index}))
}
fn crossed_blocks(secret: &str, reverse: bool) -> Vec<Vec<u8>> {
    let (a, b, key) = if reverse {
        ("thinking_delta", "thinking", "thinking")
    } else {
        ("text_delta", "text", "text")
    };
    let other_kind = if reverse { "text" } else { "thinking" };
    let other_delta = if reverse {
        "text_delta"
    } else {
        "thinking_delta"
    };
    vec![
        start(
            json!({"input_tokens":95000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":5}),
        ),
        block_start(0, key),
        delta(a, b, &secret[..12.min(secret.len() - 1)], 0),
        block_stop(0),
        block_start(1, other_kind),
        delta(other_delta, other_kind, "clear intermediate content!", 1),
        block_stop(1),
        block_start(2, key),
        delta(a, b, &secret[12.min(secret.len() - 1)..], 2),
        block_stop(2),
        frame(
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":5}}),
        ),
        stop(),
    ]
}
#[tokio::test]
async fn explicit_content_block_lifecycles_cross_channel_reflection_is_refused() {
    for secret in [TOKEN, "oauth-safe-beta"] {
        for reverse in [false, true] {
            let f = Fixture::new(crossed_blocks(secret, reverse), false, 95000).await;
            let bytes = match f.request().send().await {
                Ok(r) => r.bytes().await,
                Err(e) => Err(e),
            };
            let report = f.fence.report().unwrap();
            let retry = f.request().send().await.unwrap().status();
            let reopened = LaneAFence::build(&f.path, f.upstream.clone())
                .unwrap()
                .report()
                .unwrap();
            let calls = f.plan.calls.load(Ordering::SeqCst);
            f.close().await;
            let key = if reverse { "thinking" } else { "text" };
            assert!(
                !bytes
                    .as_ref()
                    .is_ok_and(|b| strings(b, key).contains(secret))
            );
            assert!(bytes.is_err() && !report.stream_finished && report.accounting_unknown);
            assert_eq!(retry, StatusCode::CONFLICT);
            assert_eq!(calls, 1);
            assert!(reopened.generation_permit_spent);
        }
    }
}
#[tokio::test]
async fn explicit_content_block_lifecycles_should_not_expose_protected_channel() {
    let f = Fixture::new(crossed_blocks(TOKEN, false), false, 95000).await;
    let bytes = match f.request().send().await {
        Ok(r) => r.bytes().await,
        Err(e) => Err(e),
    };
    f.close().await;
    assert!(
        !bytes
            .as_ref()
            .is_ok_and(|b| strings(b, "text").contains(TOKEN)),
        "protected text assembly across explicit content block events must refuse"
    );
}
#[tokio::test]
async fn conservative_eof_refusal_and_late_usage_errors_keep_spent_state() {
    for tail in [
        delta("text_delta", "text", &TOKEN[..12], 0),
        b"data: {\"text\":\"\\u00".to_vec(),
    ] {
        let f = Fixture::new(vec![frame(json!({"type":"ping"})), tail], true, 95000).await;
        let mut r = f.request().send().await.unwrap();
        let first = tokio::time::timeout(Duration::from_millis(500), r.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        f.plan.gate.notify_one();
        let mut refused = false;
        loop {
            match r.chunk().await {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => {
                    refused = true;
                    break;
                }
            }
        }
        let report = f.fence.report().unwrap();
        let retry = f.request().send().await.unwrap().status();
        drop(r);
        f.close().await;
        assert!(
            !first.is_empty() && refused && report.accounting_unknown && !report.stream_finished
        );
        assert_eq!(retry, StatusCode::CONFLICT);
    }
    let f=Fixture::new(vec![start(json!({"input_tokens":95000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":5})),frame(json!({"type":"error","error":{"type":"synthetic"}})),stop()],false,95000).await;
    let received = f.request().send().await.unwrap().bytes().await;
    let report = f.fence.report().unwrap();
    let retry = f.request().send().await.unwrap().status();
    f.close().await;
    assert!(received.is_ok() && report.accounting_unknown);
    assert_eq!(retry, StatusCode::CONFLICT);
}

#[tokio::test]
async fn interleaved_channel_incremental_receiver_keeps_both_header_values_private() {
    for secret in [TOKEN, "oauth-safe-beta"] {
        for reverse in [false, true] {
            let f = Fixture::new(crossed_blocks(secret, reverse), true, 95000).await;
            let mut response = f.request().send().await.unwrap();
            let first = tokio::time::timeout(Duration::from_secs(10), response.chunk())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let mut delivered = first.to_vec();
            f.plan.gate.notify_one();
            let mut refused = false;
            loop {
                match response.chunk().await {
                    Ok(Some(bytes)) => delivered.extend_from_slice(&bytes),
                    Ok(None) => break,
                    Err(_) => {
                        refused = true;
                        break;
                    }
                }
            }
            let report = f.fence.report().unwrap();
            let retry = f.request().send().await.unwrap().status();
            let calls = f.plan.calls.load(Ordering::SeqCst);
            drop(response);
            f.close().await;
            let key = if reverse { "thinking" } else { "text" };
            assert!(refused && !strings(&delivered, key).contains(secret));
            assert!(!report.stream_finished && report.accounting_unknown);
            assert_eq!(retry, StatusCode::CONFLICT);
            assert_eq!(calls, 1);
        }
    }
}

#[test]
fn reordered_complete_events_and_protocol_violations_are_distinguished() {
    let mut g = ProtectedValues::new(&headers());
    let start = br#"data: {"content_block":{"text":"","type":"text"},"index":0,"type":"content_block_start"}

"#;
    let delta = br#"data: {"delta":{"text":"benign","type":"text_delta"},"index":0,"type":"content_block_delta"}

"#;
    assert_eq!(g.push(start).unwrap().as_ref(), start);
    assert_eq!(g.push(delta).unwrap().as_ref(), delta);
    assert_eq!(g.push(&block_stop(0)).unwrap().as_ref(), block_stop(0));
    assert!(g.finish().unwrap().is_empty());
    for bad in [
        frame(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"benign"}}),
        ),
        frame(
            json!({"type":"content_block_start","index":1024,"content_block":{"type":"text","text":""}}),
        ),
        br#"data: {"type":"ping","type":"message_stop"}

"#
        .to_vec(),
    ] {
        assert!(ProtectedValues::new(&headers()).push(&bad).is_err());
    }
    let mut g = ProtectedValues::new(&headers());
    g.push(&block_start(0, "thinking")).unwrap();
    assert!(g.push(delta).is_err());
    let mut g = ProtectedValues::new(&headers());
    g.push(start).unwrap();
    assert!(g.push(start).is_err());
}
fn tool_start(index: u64) -> Vec<u8> {
    frame(json!({"type":"content_block_start","index":index,
        "content_block":{"type":"tool_use","id":format!("call-{index}"),"name":"own_tool","input":{}}}))
}
#[test]
fn tool_decoders_keep_interleaved_indices_and_escapes_independent() {
    for secret in [TOKEN, headers()["anthropic-beta"].to_str().unwrap()] {
        for cut in 1..secret.len() {
            let mut g = ProtectedValues::new(&headers());
            g.push(&tool_start(0)).unwrap();
            g.push(&tool_start(1)).unwrap();
            let encoded = encode(secret);
            let cut = cut * 6;
            let a = format!("{{\"value\":\"{}", &encoded[..cut]);
            let b = format!("{}\"}}", &encoded[cut..]);
            assert!(
                g.push(&delta("input_json_delta", "partial_json", &a, 0))
                    .unwrap()
                    .is_empty()
            );
            // Another tool's independent string close must not clear tool 0.
            assert!(
                g.push(&delta(
                    "input_json_delta",
                    "partial_json",
                    "{\"value\":\"benign\"}",
                    1
                ))
                .is_ok()
            );
            assert!(g.push(&block_stop(1)).is_ok());
            assert!(
                g.push(&delta("input_json_delta", "partial_json", &b, 0))
                    .is_err()
            );
        }
    }
    // Separate complete tool strings are not concatenated across calls.
    let mut g = ProtectedValues::new(&headers());
    g.push(&tool_start(0)).unwrap();
    g.push(&tool_start(1)).unwrap();
    let a = serde_json::to_string(&json!({"value":&TOKEN[..12]})).unwrap();
    let b = serde_json::to_string(&json!({"value":&TOKEN[12..]})).unwrap();
    g.push(&delta("input_json_delta", "partial_json", &a, 0))
        .unwrap();
    g.push(&delta("input_json_delta", "partial_json", &b, 1))
        .unwrap();
    g.push(&block_stop(0)).unwrap();
    g.push(&block_stop(1)).unwrap();
    assert!(g.finish().unwrap().is_empty());
}
#[test]
fn ordered_prefix_hold_inspects_arrivals_and_only_safe_pings_bypass() {
    for secret in [TOKEN, headers()["anthropic-beta"].to_str().unwrap()] {
        for reverse in [false, true] {
            let (kind, key, delta_kind, other) = if reverse {
                ("thinking", "thinking", "thinking_delta", "text")
            } else {
                ("text", "text", "text_delta", "thinking")
            };
            let mut g = ProtectedValues::new(&headers());
            let first = block_start(0, kind);
            assert_eq!(g.push(&first).unwrap().as_ref(), first);
            let mut queued = Vec::new();
            for event in [
                delta(delta_kind, key, &secret[..12], 0),
                block_stop(0),
                block_start(1, other),
                delta(
                    if other == "text" {
                        "text_delta"
                    } else {
                        "thinking_delta"
                    },
                    other,
                    "benign",
                    1,
                ),
                block_stop(1),
                block_start(2, kind),
            ] {
                assert!(g.push(&event).unwrap().is_empty());
                queued.extend(event);
            }
            let ping = frame(json!({"type":"ping"}));
            assert_eq!(g.push(&ping).unwrap().as_ref(), ping);
            let resolve = delta(delta_kind, key, "!", 2);
            queued.extend(&resolve);
            assert_eq!(g.push(&resolve).unwrap().as_ref(), queued);
            let close = block_stop(2);
            assert_eq!(g.push(&close).unwrap().as_ref(), close);
            assert_eq!(g.push(&stop()).unwrap().as_ref(), stop());
            assert!(g.finish().unwrap().is_empty());
        }
    }
}
#[test]
fn bounded_queue_event_channel_and_terminal_guards_refuse() {
    let mut g = ProtectedValues::new(&headers());
    g.push(&block_start(0, "text")).unwrap();
    g.push(&delta("text_delta", "text", &TOKEN[..12], 0))
        .unwrap();
    g.push(&block_stop(0)).unwrap();
    assert!(g.push(&stop()).is_err());
    let mut g = ProtectedValues::new(&headers());
    g.push(&block_start(0, "text")).unwrap();
    g.push(&delta("text_delta", "text", &TOKEN[..12], 0))
        .unwrap();
    assert!(g.finish().is_err());
    assert!(
        ProtectedValues::new(&headers())
            .push(&vec![b' '; EVENT_LIMIT + 1])
            .is_err()
    );
    let mut g = ProtectedValues::new(&headers());
    for index in 0..128 {
        g.push(&block_start(index, "text")).unwrap();
    }
    assert!(g.push(&block_start(128, "text")).is_err());
    let mut g = ProtectedValues::new(&headers());
    g.push(&block_start(0, "text")).unwrap();
    g.push(&delta("text_delta", "text", &TOKEN[..12], 0))
        .unwrap();
    for _ in 0..1023 {
        g.push(b": safe comment\n\n").unwrap();
    }
    assert!(g.push(b": safe comment\n\n").is_err());
}
#[tokio::test]
async fn actual_receiver_preserves_ordered_lifecycles_and_terminal_after_resolution() {
    let mut chunks = vec![
        frame(json!({"type":"ping"})),
        start(json!({
        "input_tokens":95000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":5})),
        block_start(0, "text"),
    ];
    let first = chunks.concat();
    let queued = [
        delta("text_delta", "text", &TOKEN[..12], 0),
        block_stop(0),
        block_start(1, "thinking"),
        delta("thinking_delta", "thinking", "benign", 1),
        block_stop(1),
        block_start(2, "text"),
        delta("text_delta", "text", "!", 2),
    ];
    let ping = frame(json!({"type":"ping"}));
    chunks.extend(queued[..6].iter().cloned());
    chunks.push(ping.clone());
    chunks.push(queued[6].clone());
    chunks.push(block_stop(2));
    chunks.push(stop());
    let mut expected = first;
    expected.extend(ping);
    expected.extend(queued.concat());
    expected.extend(block_stop(2));
    expected.extend(stop());
    let f = Fixture::new(chunks, true, 95000).await;
    let result = f.request().send().await;
    f.plan.gate.notify_one();
    let body = match result {
        Ok(response) => response.bytes().await.map(|b| b.to_vec()),
        Err(e) => Err(e),
    };
    let report = f.fence.report().unwrap();
    let calls = f.plan.calls.load(Ordering::SeqCst);
    f.close().await;
    assert_eq!(body.unwrap(), expected);
    assert!(report.stream_finished && !report.accounting_unknown);
    assert_eq!(calls, 1);
}

#[tokio::test]
async fn queued_usage_crossing_is_observed_on_arrival_before_prefix_release() {
    for usage in [
        json!({"input_tokens":100001}),
        json!({"output_tokens":128001}),
        json!({"input_tokens":u64::MAX,"cache_creation_input_tokens":1}),
    ] {
        let chunks = vec![
            frame(json!({"type":"ping"})),
            start(
                json!({"input_tokens":95000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":5}),
            ),
            block_start(0, "text"),
            delta("text_delta", "text", &TOKEN[..12], 0),
            block_stop(0),
            frame(json!({"type":"message_delta","usage":usage})),
        ];
        let f = Fixture::new(chunks, true, 95000).await;
        let mut response = f.request().send().await.unwrap();
        let first = response.chunk().await.unwrap().unwrap();
        f.plan.gate.notify_one();
        let mut delivered = first.to_vec();
        let mut failed = false;
        loop {
            match response.chunk().await {
                Ok(Some(bytes)) => delivered.extend(bytes),
                Ok(None) => break,
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        drop(response);
        let report = f.fence.report().unwrap();
        let calls = f.plan.calls.load(Ordering::SeqCst);
        let retry = f.request().send().await.unwrap().status();
        f.close().await;
        assert!(failed && (report.prompt_over_limit || report.output_over_limit));
        assert!(report.accounting_unknown && !report.stream_finished);
        assert!(!strings(&delivered, "text").contains(&TOKEN[..12]));
        assert_eq!(calls, 1);
        assert_eq!(retry, StatusCode::CONFLICT);
    }
}
#[test]
fn event_arrival_callback_runs_once_even_when_lifecycle_frames_are_queued() {
    let mut guard = ProtectedValues::new(&headers());
    let chunks = [
        block_start(0, "text"),
        delta("text_delta", "text", &TOKEN[..12], 0),
        block_stop(0),
        frame(json!({"type":"message_delta","usage":{"output_tokens":7}})),
        frame(json!({"type":"ping"})),
        block_start(1, "text"),
        delta("text_delta", "text", "!", 1),
        block_stop(1),
        stop(),
    ];
    let mut observed = Vec::new();
    for event in &chunks {
        guard
            .push_with(event, |frame| {
                observed.push(frame.to_vec());
                Ok(())
            })
            .unwrap();
    }
    guard
        .finish_with(|frame| {
            observed.push(frame.to_vec());
            Ok(())
        })
        .unwrap();
    assert_eq!(observed, chunks);
}
#[test]
fn oversized_protected_headers_and_ambiguous_semantic_fields_fail_closed() {
    let mut h = headers();
    h.insert("authorization", "x".repeat(8193).parse().unwrap());
    assert!(
        ProtectedValues::new(&h)
            .push(&frame(json!({"type":"ping"})))
            .is_err()
    );
    let mut g = ProtectedValues::new(&headers());
    g.push(&block_start(0, "text")).unwrap();
    assert!(
        g.push(&frame(json!({"type":"content_block_delta","index":0,
        "delta":{"type":"text_delta","text":"benign","thinking":"other"}})))
            .is_err()
    );
}
