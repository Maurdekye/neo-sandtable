//! Durable per-seat supervisor. The CLI owns compaction; the game owns notebooks.
use crate::{Demo, combine_results};
use cna_core::ids::SeatId;
use cna_seats::{
    driver::{CliKind, DriverError, SeatDriver},
    run::PromptBuilder,
};
use cna_server::CampaignStatus;
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::{collections::BTreeSet, time::Duration};
use tokio::{sync::watch, time::Instant};

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
            for (seat, driver) in drivers.iter_mut() {
                let seat = *seat;
                let stop = stop.clone();
                let peer = peer.clone();
                tasks.push(async move {
                    (
                        seat,
                        self.durable_seat(seat, driver.as_mut(), stop, peer).await,
                    )
                });
            }
            while let Some((seat, outcome)) = tasks.next().await {
                let _ = cancel.send(true);
                result = combine_results(result, outcome.map_err(|e| format!("{seat}: {e}")));
            }
        } else {
            for (_, driver) in drivers.iter_mut() {
                driver.stop().await;
            }
        }
        self.finish_run(result).await
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
        let mut result = tokio::select! {
            _=invalidated=>Err("controller binding changed or paused; old session stopped".into()),
            _=cancelled=>{self.sink.system(seat,"campaign session stopped; journal retained for resume");Ok(())},
            r=tokio::time::timeout(Duration::from_millis(remaining),self.durable_work(seat,driver))=>r.unwrap_or_else(|_|Err("durable wall-clock budget exhausted".into())),
        };
        let stopped = tokio::time::timeout(Duration::from_secs(5), driver.stop())
            .await
            .map_err(|_| "CLI stop exceeded five seconds".to_string());
        result = combine_results(result, stopped);
        result = combine_results(
            result,
            self.journal
                .as_ref()
                .ok_or("missing journal")?
                .interrupt(seat),
        );
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
    async fn durable_work(&self, seat: SeatId, driver: &mut dyn SeatDriver) -> Result<(), String> {
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
                        driver.stop().await;
                        reseed = true;
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
            let outcome = tokio::time::timeout(
                Duration::from_millis(ms),
                driver.run_turn(&prompt, Duration::from_millis(ms)),
            )
            .await
            .unwrap_or(Err(DriverError::Timeout));
            journal.complete(
                seat,
                elapsed(start),
                outcome.as_ref().ok(),
                &driver.telemetry(),
            )?;
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
                let saved = journal.snapshot()?.seats[&seat].session.clone();
                let reseed_now =
                    first && saved.as_ref().is_some_and(|s| s.resumed) && unavailable(&error);
                if reseed_now || outcome.as_ref().is_err_and(retryable_death) {
                    journal.recover_process(seat, reseed_now)?;
                    driver.stop().await;
                    started = false;
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
            first = false;
            reseed = false;
            if self
                .handle
                .seat(seat)
                .pending
                .iter()
                .any(|p| p.id == pending.id && p.revision == pending.revision)
            {
                return Err("CLI ended without answering the requested decision revision".into());
            }
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
}
