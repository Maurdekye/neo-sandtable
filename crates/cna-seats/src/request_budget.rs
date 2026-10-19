//! Shared, non-recovering request budget for the watched run.
//!
//! The transport must call `forward` for EVERY upstream request, including token
//! counts and retries. A spent admission is never refunded, including on errors
//! or cancellation. This is an admission primitive, not native egress isolation.

use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    Count,
    Generation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestLimits {
    /// All upstream forwards combined, including count requests.
    pub total: u64,
    pub generations: u64,
    pub wall: Duration,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RequestUsage {
    pub total: u64,
    pub generations: u64,
    pub stopped: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RequestBudgetError {
    #[error("invalid request limits")]
    Limits,
    #[error("run request budget unavailable or stopped")]
    Stopped,
    #[error("run request cap exhausted")]
    Exhausted,
    #[error("run wall limit exhausted")]
    Deadline,
}

struct State {
    limits: RequestLimits,
    deadline: Instant,
    usage: RequestUsage,
    cancelled: tokio::sync::watch::Sender<bool>,
}

/// All ten seats and their transports must share this same allocation.
#[derive(Clone)]
pub struct RequestBudget(Arc<Mutex<State>>);

impl RequestBudget {
    pub fn new(limits: RequestLimits) -> Result<Self, RequestBudgetError> {
        if limits.generations == 0 || limits.total < limits.generations || limits.wall.is_zero() {
            return Err(RequestBudgetError::Limits);
        }
        let deadline = Instant::now()
            .checked_add(limits.wall)
            .ok_or(RequestBudgetError::Limits)?;
        Ok(Self(Arc::new(Mutex::new(State {
            limits,
            deadline,
            usage: RequestUsage::default(),
            cancelled: tokio::sync::watch::channel(false).0,
        }))))
    }

    fn admit(&self, kind: RequestKind) -> Result<Instant, RequestBudgetError> {
        let mut state = self.0.lock().map_err(|_| RequestBudgetError::Stopped)?;
        if state.usage.stopped {
            return Err(RequestBudgetError::Stopped);
        }
        if Instant::now() >= state.deadline {
            state.usage.stopped = true;
            state.cancelled.send_replace(true);
            return Err(RequestBudgetError::Deadline);
        }
        if state.usage.total >= state.limits.total
            || (kind == RequestKind::Generation
                && state.usage.generations >= state.limits.generations)
        {
            state.usage.stopped = true;
            state.cancelled.send_replace(true);
            return Err(RequestBudgetError::Exhausted);
        }
        state.usage.total += 1;
        if kind == RequestKind::Generation {
            state.usage.generations += 1;
        }
        Ok(state.deadline)
    }

    /// Debit before constructing the one transport future. No automatic retry.
    /// The deadline covers this future; streamed bodies and native children need
    /// the supervisor's same deadline plus explicit stop/join handling.
    pub async fn forward<T>(
        &self,
        kind: RequestKind,
        once: impl FnOnce() -> T,
    ) -> Result<T::Output, RequestBudgetError>
    where
        T: Future,
    {
        let deadline = self.admit(kind)?;
        match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), once()).await {
            Ok(result) => Ok(result),
            Err(_) => {
                self.stop()?;
                Err(RequestBudgetError::Deadline)
            }
        }
    }

    /// Bound a body read or other continuation by this run's deadline and stop
    /// signal without spending another request. Dropping this future cancels
    /// its IO. This is also used for every streamed response read.
    pub async fn bound<T: Future>(&self, future: T) -> Result<T::Output, RequestBudgetError> {
        let (deadline, mut cancelled) = {
            let state = self.0.lock().map_err(|_| RequestBudgetError::Stopped)?;
            (state.deadline, state.cancelled.subscribe())
        };
        tokio::select! {
            biased;
            _ = cancelled.wait_for(|stopped| *stopped) => Err(RequestBudgetError::Stopped),
            result = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), future) => {
                match result {
                    Ok(value) => Ok(value),
                    Err(_) => {
                        self.stop()?;
                        Err(RequestBudgetError::Deadline)
                    }
                }
            }
        }
    }

    /// Refuse later admissions and cancel bound IO. The supervisor still owns
    /// native process cleanup and joining server tasks.
    pub fn stop(&self) -> Result<(), RequestBudgetError> {
        let mut state = self.0.lock().map_err(|_| RequestBudgetError::Stopped)?;
        state.usage.stopped = true;
        state.cancelled.send_replace(true);
        Ok(())
    }

    pub fn usage(&self) -> Result<RequestUsage, RequestBudgetError> {
        self.0
            .lock()
            .map(|state| state.usage)
            .map_err(|_| RequestBudgetError::Stopped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn budget(total: u64, generations: u64) -> RequestBudget {
        RequestBudget::new(RequestLimits {
            total,
            generations,
            wall: Duration::from_secs(10),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn ten_seats_share_one_cap_and_refusal_prevents_transport_construction() {
        let budget = budget(3, 2);
        let constructions = Arc::new(AtomicU64::new(0));
        let mut tasks = Vec::new();
        for _ in 0..10 {
            let budget = budget.clone();
            let constructions = constructions.clone();
            tasks.push(tokio::spawn(async move {
                budget
                    .forward(RequestKind::Generation, || {
                        constructions.fetch_add(1, Ordering::SeqCst);
                        async {}
                    })
                    .await
            }));
        }
        let mut admitted = 0;
        for task in tasks {
            admitted += u64::from(task.await.unwrap().is_ok());
        }
        assert_eq!(admitted, 2);
        assert_eq!(constructions.load(Ordering::SeqCst), 2);
        assert_eq!(budget.usage().unwrap().generations, 2);
        assert!(budget.usage().unwrap().stopped);
    }

    #[tokio::test]
    async fn counts_transport_errors_and_cancellation_keep_their_debit() {
        let budget = budget(3, 2);
        assert_eq!(
            budget
                .forward(RequestKind::Count, || async { Err::<(), _>(()) })
                .await,
            Ok(Err(()))
        );
        let (started, ready) = tokio::sync::oneshot::channel();
        let copy = budget.clone();
        let request = tokio::spawn(async move {
            copy.forward(RequestKind::Generation, || async {
                started.send(()).unwrap();
                std::future::pending::<()>().await;
            })
            .await
        });
        ready.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(budget.usage().unwrap().total, 2);
        budget.stop().unwrap();
        assert_eq!(
            budget
                .forward(RequestKind::Generation, || async {
                    panic!("must not forward")
                })
                .await,
            Err(RequestBudgetError::Stopped)
        );
        assert_eq!(budget.usage().unwrap().total, 2);
    }

    #[tokio::test]
    async fn wall_deadline_cancels_transport_and_sticks() {
        let budget = RequestBudget::new(RequestLimits {
            total: 2,
            generations: 2,
            wall: Duration::from_millis(10),
        })
        .unwrap();
        assert_eq!(
            budget
                .forward(RequestKind::Generation, std::future::pending::<()>)
                .await,
            Err(RequestBudgetError::Deadline)
        );
        assert!(budget.usage().unwrap().stopped);
        assert_eq!(budget.usage().unwrap().total, 1);
        assert_eq!(
            budget
                .forward(RequestKind::Count, || async { panic!("must not forward") })
                .await,
            Err(RequestBudgetError::Stopped)
        );
    }

    #[tokio::test]
    async fn fake_upstream_observes_total_forward_limit_including_counts() {
        let requests = Arc::new(AtomicU64::new(0));
        let observed = requests.clone();
        let app = axum::Router::new().fallback(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            async { "fake upstream" }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        let budget = budget(2, 2);
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for kind in [RequestKind::Count, RequestKind::Generation] {
            budget
                .forward(kind, || client.post(&url).send())
                .await
                .unwrap()
                .unwrap();
        }
        let refused = budget
            .forward(RequestKind::Generation, || client.post(&url).send())
            .await;
        let count = requests.load(Ordering::SeqCst);
        stop.send(()).unwrap();
        server.await.unwrap();
        assert!(matches!(refused, Err(RequestBudgetError::Exhausted)));
        assert_eq!(count, 2);
        assert_eq!(budget.usage().unwrap().total, 2);
        assert_eq!(budget.usage().unwrap().generations, 1);
    }
}
