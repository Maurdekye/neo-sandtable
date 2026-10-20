//! Eager, tracked calibration response ownership; original ledger stays in the adapter.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) async fn response(
    fence: LaneAFence,
    upstream: reqwest::Response,
    response: axum::http::response::Builder,
    protected: ProtectedValues,
) -> Response {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let failed = Arc::new(AtomicBool::new(true));
    let mut pumps = fence.0.pumps.lock().await;
    if fence.0.budget.usage().map_or(true, |u| u.stopped) {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
    pumps.spawn(forward(
        fence.clone(),
        upstream.bytes_stream(),
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
                None if failed.load(Ordering::SeqCst) => Some((
                    Err(io::Error::other("calibration response refused")),
                    (receiver, failed, true),
                )),
                None => None,
            }
        },
    );
    response.body(Body::from_stream(bytes)).unwrap()
}

async fn forward<S, E>(
    fence: LaneAFence,
    mut source: S,
    mut protected: ProtectedValues,
    sender: tokio::sync::mpsc::Sender<Bytes>,
    failed: Arc<AtomicBool>,
) where
    S: futures_util::Stream<Item = Result<Bytes, E>> + Unpin,
{
    let mut observer = UsageObserver::default();
    let result: Result<(), ()> = async {
        loop {
            let next = fence
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
            let mut inspect = |event: &[u8]| {
                observer.feed(event, &fence);
                let report = fence.report().map_err(|_| ())?;
                if report.prompt_over_limit || report.output_over_limit {
                    Err(())
                } else {
                    Ok(())
                }
            };
            let (bytes, finished) = match next {
                Some(Ok(bytes)) => (protected.push_with(&bytes, &mut inspect)?, false),
                Some(Err(_)) => return Err(()),
                None => {
                    let bytes = protected.finish_with(&mut inspect)?;
                    std::mem::take(&mut observer).finish(&fence);
                    // Incomplete numeric channels stay unknown; the launcher
                    // refuses successful calibration, while opaque safe EOF
                    // retains the original adapter's response behavior.
                    (bytes, true)
                }
            };
            if !bytes.is_empty() {
                fence
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
        fence.stop();
    }
    drop(source);
    failed.store(result.is_err(), Ordering::SeqCst);
    drop(sender);
}
