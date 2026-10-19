use super::*;
use crate::request_budget::RequestLimits;
use futures_util::Stream;
use std::{
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    task::{Context, Poll},
};
use tokio::sync::{Notify, oneshot};

struct TrackedStream<S> {
    source: S,
    dropped: Arc<AtomicBool>,
}
impl<S: Stream + Unpin> Stream for TrackedStream<S> {
    type Item = S::Item;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.get_mut().source).poll_next(cx)
    }
}
impl<S> Drop for TrackedStream<S> {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

async fn full_channel_stop_or_wall(wall: bool) {
    let reads = Arc::new(AtomicU64::new(0));
    let ready = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let count = reads.clone();
    let signal = ready.clone();
    let source = stream::iter(
        (0..4).map(|_| Ok::<_, io::Error>(Bytes::from_static(b"data: {\"type\":\"ping\"}\n\n"))),
    )
    .inspect(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        signal.notify_one();
    })
    .chain(stream::pending());
    let source = TrackedStream {
        source,
        dropped: dropped.clone(),
    };
    let budget = RequestBudget::new(RequestLimits {
        total: 4,
        generations: 2,
        wall: if wall {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(10)
        },
    })
    .unwrap();
    let gateway = WatchedGateway::build(budget, "http://127.0.0.1:1".into()).unwrap();
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let failed = Arc::new(AtomicBool::new(true));
    let mut task = tokio::spawn(forward_pump(
        gateway.clone(),
        source,
        account(gateway.0.totals.clone()),
        ProtectedValues::new(&HeaderMap::new()),
        sender,
        failed.clone(),
    ));
    let blocked = tokio::time::timeout(Duration::from_secs(2), async {
        while reads.load(Ordering::SeqCst) < 2 {
            ready.notified().await;
        }
    })
    .await
    .is_ok();
    // The first frame fills the ONE-slot queue; the second read reaches a
    // blocked send. Keep receiver alive and never poll it through cancellation.
    let exactly_two = reads.load(Ordering::SeqCst) == 2;
    if !wall {
        gateway.stop().unwrap();
    }
    let joined = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
    drop(receiver);
    if joined.is_err() {
        let _ = gateway.stop();
        if tokio::time::timeout(Duration::from_secs(2), &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = tokio::time::timeout(Duration::from_secs(2), &mut task).await;
        }
    }
    // Cleanup happens before any regression assertion, including on timeout.
    assert!(joined.is_ok_and(|outcome| outcome.is_ok()));
    assert!(blocked && exactly_two);
    assert!(dropped.load(Ordering::SeqCst));
    assert!(failed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn full_channel_cancels_and_drops_upstream_on_stop() {
    full_channel_stop_or_wall(false).await;
}
#[tokio::test]
async fn full_channel_cancels_and_drops_upstream_on_wall() {
    full_channel_stop_or_wall(true).await;
}

struct PendingBody {
    first: Option<Bytes>,
    dropped: Arc<AtomicBool>,
    notify: Arc<Notify>,
}
impl Stream for PendingBody {
    type Item = Result<Bytes, io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(first) = self.first.take() {
            Poll::Ready(Some(Ok(first)))
        } else {
            Poll::Pending
        }
    }
}
impl Drop for PendingBody {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }
}
struct Fixture {
    gateway: WatchedGateway,
    stop: oneshot::Sender<()>,
    join: tokio::task::JoinHandle<io::Result<()>>,
    dropped: Arc<AtomicBool>,
    notify: Arc<Notify>,
    forwards: Arc<AtomicU64>,
}
impl Fixture {
    async fn new(wall: Duration) -> Self {
        let dropped = Arc::new(AtomicBool::new(false));
        let notify = Arc::new(Notify::new());
        let forwards = Arc::new(AtomicU64::new(0));
        let d = dropped.clone();
        let n = notify.clone();
        let f = forwards.clone();
        let app = Router::new().fallback(move |uri: axum::http::Uri| {
            let d = d.clone();
            let n = n.clone();
            let f = f.clone();
            async move {
                f.fetch_add(1, Ordering::SeqCst);
                if uri.path().ends_with("count_tokens") {
                    Response::builder()
                        .body(Body::from("{\"input_tokens\":10}"))
                        .unwrap()
                } else {
                    Response::builder()
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from_stream(PendingBody {
                            first: Some(Bytes::from_static(
                                b"event: ping\ndata: {\"type\":\"ping\"}\n\n",
                            )),
                            dropped: d,
                            notify: n,
                        }))
                        .unwrap()
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        let (stop, done) = oneshot::channel();
        let join = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = done.await;
                })
                .await
        });
        let budget = RequestBudget::new(RequestLimits {
            total: 6,
            generations: 3,
            wall,
        })
        .unwrap();
        let gateway = WatchedGateway::build(budget, upstream).unwrap();
        Self {
            gateway,
            stop,
            join,
            dropped,
            notify,
            forwards,
        }
    }
    async fn response(&self) -> Response {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            "Bearer INERT_NO_AUTHORITY".parse().unwrap(),
        );
        headers.insert("anthropic-beta", "INERT_BETA".parse().unwrap());
        headers.insert("anthropic-version", "2023-06-01".parse().unwrap());
        let raw=serde_json::to_vec(&serde_json::json!({"model":crate::calibration::MODEL,"max_tokens":128000,"messages":[{"role":"user","content":"synthetic"}],"stream":true})).unwrap();
        messages(
            State(self.gateway.clone()),
            OriginalUri("/v1/messages".parse().unwrap()),
            headers,
            Bytes::from(raw),
        )
        .await
    }
    async fn close(self) {
        let _ = self.gateway.stop();
        self.gateway.join_pumps().await.unwrap();
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(3), self.join)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn response_drop_cancels_and_joins_upstream_without_body_polling() {
    let fixture = Fixture::new(Duration::from_secs(10)).await;
    let response = fixture.response().await;
    drop(response);
    let closed = tokio::time::timeout(Duration::from_secs(2), fixture.notify.notified())
        .await
        .is_ok();
    fixture.close().await;
    assert!(closed);
}

#[tokio::test]
async fn independent_unpolled_response_should_cancel_upstream_on_stop() {
    let f = Fixture::new(Duration::from_secs(10)).await;
    let response = f.response().await;
    assert_eq!(response.status(), StatusCode::OK);
    f.gateway.stop().unwrap();
    let stopped_without_downstream_poll =
        tokio::time::timeout(Duration::from_millis(100), f.notify.notified())
            .await
            .is_ok();
    let count = f.forwards.load(Ordering::SeqCst);
    drop(response);
    if !f.dropped.load(Ordering::SeqCst) {
        tokio::time::timeout(Duration::from_secs(3), f.notify.notified())
            .await
            .unwrap();
    }
    let report = f.gateway.report().unwrap();
    f.close().await;
    assert_eq!(count, 2);
    assert_eq!(report.requests.total, 2);
    assert_eq!(report.unknown_generations, 1);
    assert!(
        stopped_without_downstream_poll,
        "stop did not close retained upstream response until downstream body was dropped"
    );
}

#[tokio::test]
async fn independent_unpolled_response_should_cancel_upstream_on_wall() {
    let f = Fixture::new(Duration::from_millis(150)).await;
    let response = f.response().await;
    assert_eq!(response.status(), StatusCode::OK);
    let stopped_without_downstream_poll =
        tokio::time::timeout(Duration::from_millis(400), f.notify.notified())
            .await
            .is_ok();
    drop(response);
    if !f.dropped.load(Ordering::SeqCst) {
        tokio::time::timeout(Duration::from_secs(3), f.notify.notified())
            .await
            .unwrap();
    }
    f.close().await;
    assert!(
        stopped_without_downstream_poll,
        "wall expiry did not close retained upstream response until downstream body was dropped"
    );
}

#[tokio::test]
async fn independent_polling_response_observes_stop_and_joins() {
    let f = Fixture::new(Duration::from_secs(10)).await;
    let response = f.response().await;
    let mut stream = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!first.is_empty());
    f.gateway.stop().unwrap();
    let refused = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .is_err();
    drop(stream);
    if !f.dropped.load(Ordering::SeqCst) {
        tokio::time::timeout(Duration::from_secs(3), f.notify.notified())
            .await
            .unwrap();
    }
    let report = f.gateway.report().unwrap();
    f.close().await;
    assert!(refused);
    assert_eq!(report.unknown_generations, 1);
}

fn frame(value: serde_json::Value) -> Vec<u8> {
    format!("data: {value}\n\n").into_bytes()
}
fn account(totals: Arc<Mutex<Totals>>) -> Accounting {
    Accounting {
        report: Mutex::new(LaneAReport {
            accounting_unknown: true,
            ..LaneAReport::default()
        }),
        observer: UsageObserver::default(),
        totals,
    }
}
#[test]
fn independent_complete_and_partial_numeric_usage_remain_distinct() {
    let totals = Arc::new(Mutex::new(Totals::default()));
    for complete in [true, false] {
        let mut a = account(totals.clone());
        a.feed(&frame(serde_json::json!({"type":"message_start","message":{"usage":{"input_tokens":10,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":1}}}))).unwrap();
        if complete {
            a.feed(&frame(
                serde_json::json!({"type":"message_delta","usage":{"output_tokens":7}}),
            ))
            .unwrap();
            a.feed(&frame(serde_json::json!({"type":"message_stop"})))
                .unwrap();
            a.finish().unwrap();
        }
        drop(a);
    }
    let total = totals.lock().unwrap();
    assert_eq!(
        total.tokens,
        TokenLowerBounds {
            input: 20,
            cache_creation: 4,
            cache_read: 6,
            output: 8
        }
    );
    assert_eq!(total.complete, 1);
}

#[test]
fn independent_partial_prompt_crossing_and_invalid_usage_cannot_complete() {
    let totals = Arc::new(Mutex::new(Totals::default()));
    let mut a = account(totals.clone());
    assert!(
        a.feed(&frame(
            serde_json::json!({"type":"message_start","message":{"usage":{"input_tokens":100001}}})
        ))
        .is_err()
    );
    drop(a);
    let total = totals.lock().unwrap();
    assert_eq!(total.tokens.input, 100001);
    assert_eq!(total.complete, 0);
    drop(total);
    let mut a = account(totals.clone());
    assert!(
        a.feed(&frame(
            serde_json::json!({"type":"message_delta","usage":{"output_tokens":null}})
        ))
        .is_err()
    );
    drop(a);
    assert_eq!(totals.lock().unwrap().complete, 0);
}
