//! md15 boundary control only: no worker panic is manufactured here. Seats owns
//! actual poll/Drop panic classification, raw JoinError disposal and hook checks.
use super::*;

fn engine_marker(path: &Path) -> Result<(u64, String, String, u64), Error> {
    let db = rusqlite::Connection::open(path)?;
    Ok(db.query_row(
        "SELECT revision,state_hash,rng,(SELECT COUNT(*) FROM commands) FROM campaign WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?)
}

fn persisted_binding(path: &Path, seat: SeatId) -> Result<Binding, Error> {
    let db = rusqlite::Connection::open(path)?;
    let binding: String = db.query_row(
        "SELECT binding FROM seats WHERE seat=?",
        [seat.to_string()],
        |row| row.get(0),
    )?;
    Ok(serde_json::from_str(&binding)?)
}

#[tokio::test]
async fn fixed_worker_panic_reason_is_seat_local_and_cannot_pause_replacement() {
    const REASON: &str = "transcript delivery worker panicked; see server log";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("worker-panic-seat-boundary.sqlite");
    let (handle, identity, _request, responses) = forced_fixture(&path);
    let seat = identity.seat;
    // Keep assertions until after the original actor joins, including when any
    // fallible boundary call fails. No task, worker or panic payload is created.
    let observed = async {
        let before = engine_marker(&path)?;
        let pending = handle.seat(seat).pending;
        let mut checks = Vec::new();
        handle
            .mark_failure_if_epoch(seat, identity.epoch, REASON)
            .await?;
        let failed = handle.seat(seat);
        checks.push(("original seat paused", failed.binding.paused));
        checks.push((
            "fixed reason stored without diagnostic rendering",
            failed.binding.failure.as_deref() == Some(REASON),
        ));
        checks.push((
            "pause durable before acknowledgement",
            persisted_binding(&path, seat)? == failed.binding,
        ));
        checks.push(("pending decision preserved", failed.pending == pending));
        checks.push((
            "worker failure is not campaign fatal",
            matches!(handle.status(), CampaignStatus::Running),
        ));
        handle
            .mark_failure_if_epoch(seat, identity.epoch, REASON)
            .await?;
        checks.push((
            "repeat callback leaves original fixed reason",
            handle.seat(seat) == failed,
        ));
        handle.pause(true).await?;
        checks.push((
            "operator pause remains available",
            matches!(handle.status(), CampaignStatus::Paused),
        ));
        handle.pause(false).await?;
        checks.push((
            "operator resume does not unpause failed seat",
            matches!(handle.status(), CampaignStatus::Running)
                && handle.seat(seat).binding == failed.binding,
        ));
        let replacement = handle
            .handover(seat, failed.binding.controller.clone(), Value::Null)
            .await?;
        checks.push((
            "handover alone clears failed seat at new epoch",
            replacement.controller_epoch == identity.epoch + 1
                && !replacement.paused
                && replacement.failure.is_none(),
        ));
        let before_stale = handle.seat(seat);
        let stored_before_stale = persisted_binding(&path, seat)?;
        let rejected = handle
            .mark_failure_if_epoch(seat, identity.epoch, REASON)
            .await;
        checks.push((
            "old panic callback rejected at writer boundary",
            matches!(rejected, Err(Error::StaleEpoch)),
        ));
        checks.push((
            "replacement unaffected in memory and storage",
            handle.seat(seat) == before_stale
                && persisted_binding(&path, seat)? == stored_before_stale,
        ));
        checks.push((
            "callbacks and controls invent no order or RNG transition",
            engine_marker(&path)? == before && responses.load(Ordering::SeqCst) == 0,
        ));
        Ok::<_, Error>(checks)
    }
    .await;
    let joined = handle.shutdown().await;
    joined.expect("original campaign actor must join without a campaign fatal");
    for (name, passed) in observed.expect("fixed-reason boundary operations") {
        assert!(passed, "{name}");
    }
}
