//! Durable per-seat supervisor. The CLI owns compaction; the game owns notebooks.
use crate::{Demo, budget::Admission, combine_results};
use cna_core::ids::SeatId;
use cna_seats::{
    driver::{CliKind, DriverError, SeatDriver, TimeoutDiagnostic, measured_millis},
    run::PromptBuilder,
};
use cna_server::CampaignStatus;
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, watch},
    time::Instant,
};

fn elapsed(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .saturating_add(1)
}
fn unavailable(text: &str) -> bool {
    text.to_ascii_lowercase()
        .contains("no conversation found with session id")
}

fn retryable_death(error: &DriverError) -> bool {
    let DriverError::Died(text) = error else {
        return false;
    };
    let text = text.to_ascii_lowercase();
    ![
        "quota",
        "rate limit",
        "usage limit",
        "not logged",
        "authentication",
        "unauthorized",
        "invalid api key",
        "insufficient credit",
    ]
    .iter()
    .any(|word| text.contains(word))
}
/// Observe which timer branch actually returned. An inner diagnostic stays intact.
async fn bounded_turn(
    driver: &mut dyn SeatDriver,
    prompt: &str,
    reserved_ms: u64,
    start: Instant,
) -> Result<cna_seats::driver::TurnOutcome, DriverError> {
    match tokio::time::timeout(
        Duration::from_millis(reserved_ms),
        driver.run_turn(prompt, Duration::from_millis(reserved_ms)),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => Err(DriverError::TimeoutAt(TimeoutDiagnostic::turn_reservation(
            start,
            reserved_ms,
            driver.turn_progress(),
        ))),
    }
}
fn completion_failure(
    original: Option<&DriverError>,
    storage: &crate::journal::CompletionError,
) -> String {
    match original {
        Some(error) => format!("{error}; {storage}"),
        None => storage.to_string(),
    }
}
impl Demo {
    /// Stop is graceful and resumable; exhausted budgets or CLI failures pause the bound seat.
    pub async fn play_durable(
        &self,
        drivers: &mut [(SeatId, Box<dyn SeatDriver>)],
        stop: watch::Receiver<bool>,
    ) -> Result<(), String> {
        if self.journal.is_none() {
            return Err("durable session journal required".into());
        }
        let expected: BTreeSet<_> = self.epochs.keys().copied().collect();
        let provided: BTreeSet<_> = drivers.iter().map(|(s, _)| *s).collect();
        if expected != provided
            || provided.len() != drivers.len()
            || drivers.iter().any(|(_, d)| d.kind() != CliKind::Claude)
        {
            return Err("exactly one confined Claude driver per binding required".into());
        }
        if drivers.is_empty() {
            return self.play_sessions(drivers, self.config.max_turns).await;
        }
        let mut result = Ok(());
        for seat in &expected {
            result = combine_results(result, self.publish_usage(*seat).await);
        }
        if result.is_ok() {
            result = self.handle.pause(false).await.map_err(|e| e.to_string());
        }
        if result.is_ok() {
            let (cancel, peer) = watch::channel(false);
            let mut tasks = FuturesUnordered::new();
            let slots = Arc::new(Semaphore::new(2));
            for (seat, driver) in drivers.iter_mut() {
                let seat = *seat;
                let stop = stop.clone();
                let peer = peer.clone();
                let slots = slots.clone();
                tasks.push(async move {
                    (
                        seat,
                        self.durable_seat(seat, driver.as_mut(), stop, peer, slots)
                            .await,
                    )
                });
            }
            while let Some((seat, outcome)) = tasks.next().await {
                if self.config.run.is_none() || outcome.is_err() {
                    let _ = cancel.send(true);
                }
                result = combine_results(result, outcome.map_err(|e| format!("{seat}: {e}")));
            }
        } else {
            for (_, driver) in drivers.iter_mut() {
                result = combine_results(result, driver.stop().await.map_err(|e| e.to_string()));
            }
        }
        self.finish_run(result).await
    }
    async fn stop_child(&self, seat: SeatId, driver: &mut dyn SeatDriver) -> Result<(), String> {
        let stopped = tokio::time::timeout(Duration::from_secs(5), driver.stop())
            .await
            .map_err(|_| "CLI stop exceeded five seconds".to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));
        if stopped.is_err() {
            // Neither a failed stop nor absence of a turn marker proves no model work.
            let marked = self
                .journal
                .as_ref()
                .ok_or("missing journal")?
                .interrupt(seat, false);
            let paused = self.handle.pause(true).await.map_err(|e| e.to_string());
            return combine_results(combine_results(stopped, marked), paused);
        }
        Ok(())
    }
    async fn publish_usage(&self, seat: SeatId) -> Result<(), String> {
        // Board decoder/revision replacement landed in 87ffa001 before producer enable.
        self.journal
            .as_ref()
            .ok_or("missing journal")?
            .deliver_usage(seat, &self.sink)
            .await
    }
    async fn durable_seat(
        &self,
        seat: SeatId,
        driver: &mut dyn SeatDriver,
        mut stop: watch::Receiver<bool>,
        mut peer: watch::Receiver<bool>,
        slots: Arc<Semaphore>,
    ) -> Result<(), String> {
        let epoch = self.epochs[&seat];
        let mut binding = self.handle.watch_seat(seat);
        let invalidated = async {
            loop {
                {
                    let state = binding.borrow_and_update();
                    if state.binding.controller_epoch != epoch || state.binding.paused {
                        break;
                    }
                }
                if binding.changed().await.is_err() {
                    break;
                }
            }
        };
        let cancelled = async {
            loop {
                if *stop.borrow_and_update() || *peer.borrow_and_update() {
                    break;
                }
                tokio::select! {r = stop.changed()=>{if r.is_err(){break;}}, r = peer.changed()=>{if r.is_err(){break;}}}
            }
        };
        let saved = self.journal.as_ref().ok_or("missing journal")?.snapshot()?;
        let used = saved.seats[&seat].wall_millis;
        let remaining = self
            .config
            .session
            .as_ref()
            .ok_or("missing limits")?
            .wall_seconds
            .saturating_mul(1000)
            .saturating_sub(used);
        let mut permit: Option<OwnedSemaphorePermit> = None;
        let mut result = tokio::select! {
            _=invalidated=>Err("controller binding changed or paused; old session stopped".into()),
            _=cancelled=>{self.sink.system(seat,"campaign session stopped; journal retained for resume");Ok(())},
            r=tokio::time::timeout(Duration::from_millis(remaining),self.durable_work(seat,driver,slots,&mut permit))=>r.unwrap_or_else(|_|Err("durable wall-clock budget exhausted".into())),
        };
        let stopped = self.stop_child(seat, driver).await;
        let cleanup_confirmed = stopped.is_ok();
        result = combine_results(result, stopped);
        if self.config.run.is_some()
            && result.as_ref().is_err_and(|e| {
                matches!(
                    e.as_str(),
                    "durable turn budget exhausted"
                        | "durable tool budget exhausted"
                        | "durable wall-clock budget exhausted"
                )
            })
        {
            let reason = result.as_ref().unwrap_err().clone();
            self.sink.system(
                seat,
                format!("automatic budget stop: {reason}; lifetime journal retained"),
            );
            result = self.handle.pause(true).await.map_err(|e| e.to_string());
        }
        result = combine_results(
            result,
            self.journal
                .as_ref()
                .ok_or("missing journal")?
                .interrupt(seat, cleanup_confirmed),
        );
        // Unconfirmed cleanup has already durably blocked admissions and paused the
        // campaign. Only then may this task relinquish capacity.
        drop(permit);
        result = combine_results(result, self.publish_usage(seat).await);
        if let Err(reason) = &result
            && !matches!(
                self.handle.status(),
                CampaignStatus::Finished { .. } | CampaignStatus::Stopped { .. }
            )
        {
            match self.handle.mark_failure_if_epoch(seat, epoch, reason).await {
                Ok(()) | Err(cna_server::Error::StaleEpoch) => {}
                Err(e) => {
                    result =
                        combine_results(result, Err(format!("recording seat failure failed: {e}")))
                }
            }
        }
        result
    }
    async fn durable_work(
        &self,
        seat: SeatId,
        driver: &mut dyn SeatDriver,
        slots: Arc<Semaphore>,
        permit: &mut Option<OwnedSemaphorePermit>,
    ) -> Result<(), String> {
        let journal = self.journal.as_ref().ok_or("missing journal")?;
        let limits = self.config.session.as_ref().ok_or("missing limits")?;
        let mut windows = self.handle.watch_seat(seat);
        let mut started = false;
        let mut first = true;
        let mut reseed = false;
        loop {
            let pending = loop {
                match self.handle.status() {
                    CampaignStatus::Finished { .. } => return Ok(()),
                    CampaignStatus::Running => {}
                    CampaignStatus::Paused if self.config.run.is_some() => return Ok(()),
                    other => return Err(format!("campaign stopped: {other:?}")),
                }
                let p = windows.borrow_and_update().pending.first().cloned();
                if let Some(p) = p {
                    break p;
                }
                // Idle intervals are reserved too, so an abrupt restart cannot reset elapsed time.
                let ms = journal.reserve(seat, "idle", "idle", 1000)?;
                let start = Instant::now();
                tokio::select! {_=tokio::time::sleep(Duration::from_millis(ms))=>{}, _=windows.changed()=>{}}
                journal.complete(seat, elapsed(start), None, &driver.telemetry())?;
            };
            let stage = format!(
                "GT{}:{}",
                pending.clock.game_turn,
                pending
                    .clock
                    .op_stage
                    .map(|n| format!("OpStage{n}"))
                    .unwrap_or_else(|| pending.clock.anchor.stage().into())
            );
            if cna_seats::automatic::forced_pass(&pending.space) {
                let ms = journal.reserve(seat, "automatic", &stage, 5000)?;
                let start = Instant::now();
                let result = tokio::time::timeout(
                    Duration::from_millis(ms),
                    cna_seats::automatic::answer(
                        &self.handle,
                        seat,
                        self.epochs[&seat],
                        &pending,
                        &self.sink,
                    ),
                )
                .await
                .map_err(|_| "automatic wall-clock budget exhausted".to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
                journal.complete_automatic(seat, elapsed(start), matches!(result, Ok(true)))?;
                result?;
                continue;
            }
            if self.config.run.is_some() {
                *permit = Some(
                    slots
                        .clone()
                        .acquire_owned()
                        .await
                        .map_err(|e| e.to_string())?,
                );
                if !matches!(self.handle.status(), CampaignStatus::Running) {
                    return Ok(());
                }
                let current = self.handle.seat(seat);
                if current.binding.controller_epoch != self.epochs[&seat] {
                    return Err("binding changed while waiting for model slot".into());
                }
                if !current
                    .pending
                    .iter()
                    .any(|p| p.id == pending.id && p.revision == pending.revision)
                {
                    permit.take();
                    continue;
                }
                let bound = driver.estimate_bound();
                let admission = journal.admit(seat, bound.as_ref())?;
                let applied = match admission {
                    Admission::Stop(reason) => {
                        self.sink.system(
                            seat,
                            format!("automatic budget stop: {reason}; no new model turn"),
                        );
                        self.handle.pause(true).await.map_err(|e| e.to_string())?;
                        return Ok(());
                    }
                    Admission::Ready {
                        remaining_usd: Some(usd),
                        ..
                    } => driver.set_reported_cost_limit(usd),
                    Admission::Ready {
                        remaining_tokens: Some(tokens),
                        ..
                    } => driver.set_token_limit(tokens),
                    _ => return Err("missing enforced budget".into()),
                };
                if let Err(error) = applied {
                    self.sink.system(
                        seat,
                        format!("budget enforcement unavailable: {error}; no model turn"),
                    );
                    self.handle.pause(true).await.map_err(|e| e.to_string())?;
                    return Ok(());
                }
            }
            if !started {
                let saved = journal.snapshot()?.seats[&seat].session.clone();
                let resume = saved.as_ref().map(|s| s.session_id.as_str());
                let ms = journal.reserve(seat, "start", "start", 30_000)?;
                let start = Instant::now();
                let info = tokio::time::timeout(Duration::from_millis(ms), driver.start(resume))
                    .await
                    .map_err(|_| "session start timed out".to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()));
                journal.complete(seat, elapsed(start), None, &driver.telemetry())?;
                match info {
                    Ok(info) => {
                        journal.session(seat, info)?;
                        started = true;
                        first = true;
                    }
                    Err(error) if resume.is_some() && unavailable(&error) => {
                        journal.recover_process(seat, true)?;
                        self.stop_child(seat, driver).await?;
                        reseed = true;
                        journal.release_admission(seat, true)?;
                        permit.take();
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            if let Some(context) = driver.telemetry().context_tokens
                && context >= limits.context_tokens.saturating_mul(95) / 100
            {
                return Err(
                    "context budget exhausted after native compaction opportunity; session paused"
                        .into(),
                );
            }
            let prompt = if first {
                let notes = self
                    .handle
                    .notebook(seat)
                    .await
                    .map_err(|e| format!("notebook recovery failed: {e}"))?;
                let preface = if reseed {
                    "The previous CLI session was unavailable. Recover your standing plan from this durable notebook."
                } else {
                    "Continue your campaign seat, preserving your standing plan."
                };
                format!(
                    "{preface}\n{}",
                    self.prompts
                        .first_turn(seat, &notes, std::slice::from_ref(&pending))
                )
            } else {
                self.prompts
                    .window_turn(seat, std::slice::from_ref(&pending))
            };
            let prompt = format!(
                "{prompt}\n\nAnswer exactly decision {} revision {} once, then end this model turn. The same group may reopen with a newer revision: leave that new request for the next turn. Keep durable plans in notebook_write. Use only your scoped MCP tools.",
                pending.id, pending.revision
            );

            let ms = journal.reserve_decision(
                seat,
                &stage,
                limits.turn_seconds * 1000,
                pending.id.as_str(),
                pending.revision,
            )?;
            let start = Instant::now();
            let outcome = bounded_turn(driver, &prompt, ms, start).await;
            let timeout = outcome
                .as_ref()
                .err()
                .and_then(DriverError::timeout_diagnostic)
                .map(|diagnostic| crate::journal::TimeoutCompletion {
                    diagnostic,
                    requested_turn_ms: limits.turn_seconds * 1000,
                    supervisor_turn_elapsed_ms: measured_millis(start.elapsed()),
                });
            journal
                .complete_with_diagnostic(
                    seat,
                    elapsed(start),
                    outcome.as_ref().ok(),
                    &driver.telemetry(),
                    timeout,
                )
                .map_err(|storage| completion_failure(outcome.as_ref().err(), &storage))?;
            self.publish_usage(seat).await?;
            if let Some(id) = driver.session_id()
                && journal.snapshot()?.seats[&seat]
                    .session
                    .as_ref()
                    .map(|s| s.session_id.as_str())
                    != Some(id.as_str())
            {
                return Err("CLI session identity changed unexpectedly".into());
            }
            let error = match outcome {
                Ok(ref o) if o.ok => None,
                Ok(ref o) => Some(o.error.clone().unwrap_or_else(|| "CLI refused turn".into())),
                Err(ref e) => Some(e.to_string()),
            };
            if let Some(error) = error {
                if self.config.run.is_some()
                    && error.to_ascii_lowercase().contains("max_budget_usd")
                {
                    self.sink.system(
                        seat,
                        "provider reported-cost limit reached; campaign paused",
                    );
                    self.handle.pause(true).await.map_err(|e| e.to_string())?;
                    return Ok(());
                }
                let saved = journal.snapshot()?.seats[&seat].session.clone();
                let reseed_now =
                    first && saved.as_ref().is_some_and(|s| s.resumed) && unavailable(&error);
                if reseed_now || outcome.as_ref().is_err_and(retryable_death) {
                    journal.recover_process(seat, reseed_now)?;
                    self.stop_child(seat, driver).await?;
                    started = false;
                    journal.release_admission(seat, true)?;
                    permit.take();
                    reseed = reseed_now;
                    self.sink.system(
                        seat,
                        if reseed_now {
                            "CLI resume unavailable; reseeding from durable notebook"
                        } else {
                            "CLI process died; resuming the same session"
                        },
                    );
                    continue;
                }
                return Err(error);
            }
            if self.config.run.is_some() {
                self.stop_child(seat, driver).await?;
                journal.release_admission(seat, true)?;
                permit.take();
                started = false;
            }
            if self
                .handle
                .seat(seat)
                .pending
                .iter()
                .any(|p| p.id == pending.id && p.revision == pending.revision)
            {
                return Err("CLI ended without answering the requested decision revision".into());
            }
            if self.config.run.is_some()
                && let Admission::Stop(reason) =
                    journal.admission(seat, driver.estimate_bound().as_ref())?
            {
                self.sink.system(
                    seat,
                    format!("automatic budget stop: {reason}; journal retained"),
                );
                self.handle.pause(true).await.map_err(|e| e.to_string())?;
                return Ok(());
            }
            first = false;
            reseed = false;
            let record = journal.snapshot()?.seats[&seat].clone();
            self.sink.system(seat,format!("durable turn {} complete ({stage}); calls {}, reported cumulative cost ${:.6}, compactions {}, incomplete telemetry turns {}",record.turns,record.calls,record.reported_cost_usd,record.compactions,record.incomplete_turns));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_refusals_are_not_retried_as_process_crashes() {
        for text in [
            "usage limit reached",
            "quota exhausted",
            "authentication required",
            "rate limit exceeded",
        ] {
            assert!(!retryable_death(&DriverError::Died(text.into())));
        }
        assert!(retryable_death(&DriverError::Died(
            "process exited unexpectedly".into()
        )));
        assert!(!retryable_death(&DriverError::Timeout));
    }

    struct TimerDriver {
        error: Option<DriverError>,
        hang: bool,
        progress: Option<cna_seats::driver::TurnProgress>,
        entered: bool,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
    }
    struct CancelGuard(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for CancelGuard {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    #[async_trait::async_trait]
    impl SeatDriver for TimerDriver {
        fn kind(&self) -> CliKind {
            CliKind::Claude
        }
        fn turn_progress(&self) -> Option<cna_seats::driver::TurnProgress> {
            self.progress
        }
        async fn start(
            &mut self,
            _: Option<&str>,
        ) -> Result<cna_seats::driver::SessionInfo, DriverError> {
            unreachable!()
        }
        async fn run_turn(
            &mut self,
            _: &str,
            limit: Duration,
        ) -> Result<cna_seats::driver::TurnOutcome, DriverError> {
            assert!(!limit.is_zero());
            self.entered = true;
            let _guard = CancelGuard(self.cancelled.clone());
            if self.hang {
                std::future::pending::<()>().await;
            }
            Err(self.error.take().unwrap())
        }
        fn session_id(&self) -> Option<String> {
            None
        }
        fn is_alive(&mut self) -> bool {
            false
        }
        async fn stop(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
    }
    fn timer_driver(error: Option<DriverError>, hang: bool) -> TimerDriver {
        TimerDriver {
            error,
            hang,
            progress: None,
            entered: false,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
    #[tokio::test]
    async fn actual_outer_timer_cancels_and_classifies_without_retry_or_stale_phase() {
        use cna_seats::driver::{PhaseMeasurement, TimeoutKind, TurnPhase, TurnProgress};
        let mut driver = timer_driver(None, true);
        let start = Instant::now();
        driver.progress = Some(TurnProgress {
            turn_started_at: start - Duration::from_secs(1),
            phase_started_at: start,
            phase: TurnPhase::Submit,
        });
        let result = bounded_turn(&mut driver, "inert", 5, start).await;
        let error = result.unwrap_err();
        assert!(driver.entered);
        assert!(driver.cancelled.load(std::sync::atomic::Ordering::SeqCst));
        let d = error.timeout_diagnostic().unwrap();
        assert_eq!(d.kind, TimeoutKind::TurnReservation);
        assert_eq!(d.observed_limit_ms, Some(5));
        assert!(d.observed_elapsed_ms.unwrap() >= 5);
        assert_eq!(
            (d.phase, d.phase_measurement),
            (TurnPhase::Unknown, PhaseMeasurement::RejectedStale)
        );
        assert!(!retryable_death(&error));
    }
    #[tokio::test]
    async fn inner_timeout_and_other_errors_are_not_reclassified_by_outer_timer() {
        let diagnostic =
            TimeoutDiagnostic::transcript_confirmation(Instant::now(), Duration::from_secs(30));
        let mut driver = timer_driver(Some(DriverError::TimeoutAt(diagnostic.clone())), false);
        let error = bounded_turn(&mut driver, "inert", 1000, Instant::now())
            .await
            .unwrap_err();
        assert_eq!(error.timeout_diagnostic(), Some(diagnostic));
        assert!(!retryable_death(&error));
        let mut driver = timer_driver(Some(DriverError::Timeout), false);
        let error = bounded_turn(&mut driver, "inert", 1000, Instant::now())
            .await
            .unwrap_err();
        assert!(matches!(error, DriverError::Timeout));
        assert_eq!(
            error.timeout_diagnostic(),
            Some(TimeoutDiagnostic::legacy())
        );
        for original in [
            DriverError::Cli("refused".into()),
            DriverError::Protocol("sink failed".into()),
        ] {
            let text = original.to_string();
            let mut driver = timer_driver(Some(original), false);
            let error = bounded_turn(&mut driver, "inert", 1000, Instant::now())
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), text);
            assert!(!error.is_timeout());
        }
    }
    #[test]
    fn completion_error_preserves_both_failures_without_exposing_diagnostic() {
        let error = DriverError::TimeoutAt(TimeoutDiagnostic::transcript_confirmation(
            Instant::now(),
            Duration::from_secs(30),
        ));
        let storage = crate::journal::CompletionError {
            timeout: error.timeout_diagnostic(),
            storage: "injected completion persistence failure".into(),
        };
        let displayed = completion_failure(Some(&error), &storage);
        assert!(displayed.contains(&DriverError::Timeout.to_string()));
        assert!(displayed.contains(&storage.storage));
        assert!(!displayed.contains("transcript_confirmation"));
        assert_eq!(storage.timeout, error.timeout_diagnostic());
    }
}
