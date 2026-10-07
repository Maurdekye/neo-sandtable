//! Trusted launcher journal, separate from the campaign event log and seat working directories.
//! SQLite commits precede model work. In-flight reservations survive a hard crash.
use crate::config::LaunchConfig;
use cna_core::ids::SeatId;
use cna_protocol::UsageSnapshot;
use cna_seats::{
    driver::{SessionInfo, SessionTelemetry, TurnOutcome},
    mcp::ToolBudget,
};
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct StageUsage {
    #[serde(default)]
    pub automatic_attempts: u64,
    #[serde(default)]
    pub automatic_completed: u64,
    pub attempts: u64,
    pub completed: u64,
    pub reported_cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
}
/// Known provider totals, with a transactionally committed transcript outbox.
/// Missing optional channels stay null until measured; partial totals are marked
/// by incomplete_turns when input, output or cost reports are unavailable.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct UsageJournal {
    pub revision: u64,
    pub acknowledged_revision: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub reported_cost_usd: Option<f64>,
    pub outbox: Vec<UsageSnapshot>,
}
fn add_channel(total: &mut Option<u64>, measured: Option<u64>) -> Result<(), String> {
    if let Some(value) = measured {
        let value = total
            .unwrap_or(0)
            .checked_add(value)
            .ok_or("usage token total overflow")?;
        if value > 9_007_199_254_740_991 {
            return Err("usage token total exceeds wire integer precision".into());
        }
        *total = Some(value);
    }
    Ok(())
}
fn migrate_usage(s: &mut SeatJournal) -> bool {
    if s.usage.is_some() {
        return false;
    }
    // Old journals discarded auxiliary channels and used zero for absent input
    // reports. Do not reconstruct measured zeros or complete totals from them.
    let interrupted = u64::from(s.inflight.as_ref().is_some_and(|r| r.kind == "turn"));
    s.incomplete_turns = s.incomplete_turns.max(s.turns.saturating_sub(interrupted));
    s.usage = Some(UsageJournal {
        reported_cost_usd: (s.reported_cost_usd > 0.).then_some(s.reported_cost_usd),
        ..UsageJournal::default()
    });
    true
}
fn queue_usage(s: &mut SeatJournal) -> Result<(), String> {
    let completed = s
        .stages
        .values()
        .try_fold(0u64, |sum, stage| sum.checked_add(stage.completed))
        .ok_or("usage completion total overflow")?;
    let model = s
        .session
        .as_ref()
        .and_then(|i| i.model.clone())
        .unwrap_or_else(|| s.model.clone());
    let u = s.usage.as_mut().ok_or("missing usage journal")?;
    u.revision = u.revision.checked_add(1).ok_or("usage revision overflow")?;
    u.outbox.push(UsageSnapshot {
        controller_epoch: s.epoch,
        revision: u.revision,
        provider: Some("claude-code".into()),
        model: Some(model),
        attempts: s.turns,
        completed,
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        cache_read_tokens: u.cache_read_tokens,
        cache_creation_tokens: u.cache_creation_tokens,
        reasoning_tokens: u.reasoning_tokens,
        reported_cost_usd: u.reported_cost_usd,
        incomplete_turns: s.incomplete_turns,
    });
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reservation {
    pub kind: String,
    pub stage: String,
    pub millis: u64,
    pub decision: Option<(String, u32)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeatJournal {
    pub epoch: u64,
    pub model: String,
    pub session: Option<SessionInfo>,
    pub calls: u64,
    pub turns: u64,
    pub wall_millis: u64,
    pub recoveries: u32,
    pub incomplete_turns: u64,
    pub reported_cost_usd: f64,
    pub last_session_cost: f64,
    pub compactions: u64,
    pub telemetry: SessionTelemetry,
    pub stages: BTreeMap<String, StageUsage>,
    pub inflight: Option<Reservation>,
    #[serde(default)]
    pub usage: Option<UsageJournal>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalState {
    pub version: u32,
    pub campaign_id: String,
    pub config: LaunchConfig,
    pub seats: BTreeMap<SeatId, SeatJournal>,
}
pub struct SessionJournal {
    path: PathBuf,
    db: Mutex<Connection>,
    // Held transaction in a separate DB is an OS-released exclusive launcher lease.
    _lease: Mutex<Connection>,
}
impl SessionJournal {
    pub fn path(directory: &Path, id: &str) -> Result<PathBuf, String> {
        uuid::Uuid::parse_str(id).map_err(|_| "invalid campaign id")?;
        Ok(directory
            .join("session-journals")
            .join(format!("{id}.sqlite")))
    }
    fn open(path: PathBuf, create: bool) -> Result<Self, String> {
        if !create && !path.is_file() {
            return Err("no durable session journal for campaign".into());
        }
        std::fs::create_dir_all(path.parent().ok_or("journal needs parent")?)
            .map_err(|e| e.to_string())?;
        let lease =
            Connection::open(path.with_extension("lease.sqlite")).map_err(|e| e.to_string())?;
        lease
            .busy_timeout(std::time::Duration::ZERO)
            .map_err(|e| e.to_string())?;
        lease
            .execute_batch("CREATE TABLE IF NOT EXISTS lease (id INTEGER); BEGIN EXCLUSIVE;")
            .map_err(|e| format!("campaign launcher already leased or lease unavailable: {e}"))?;
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | if create {
                OpenFlags::SQLITE_OPEN_CREATE
            } else {
                OpenFlags::empty()
            };
        let db = Connection::open_with_flags(&path, flags).map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS journal (id INTEGER PRIMARY KEY CHECK(id=1), state TEXT NOT NULL);").map_err(|e|e.to_string())?;
        Ok(Self {
            path,
            db: Mutex::new(db),
            _lease: Mutex::new(lease),
        })
    }
    pub fn config_for_campaign(path: &Path) -> Result<LaunchConfig, String> {
        let directory = path.parent().ok_or("campaign needs parent")?;
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("invalid campaign file")?;
        let db = Connection::open_with_flags(
            Self::path(directory, id)?,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| e.to_string())?;
        let state = Self::read(&db)?;
        if state.version != 1 || state.campaign_id != id || state.config.session.is_none() {
            return Err("invalid durable campaign journal".into());
        }
        state.config.validate()?;
        Ok(state.config)
    }
    pub fn create(
        directory: &Path,
        id: &str,
        config: LaunchConfig,
        epochs: &BTreeMap<SeatId, u64>,
    ) -> Result<Self, String> {
        config.validate()?;
        if config.session.is_none() {
            return Err("durable limits required".into());
        }
        let journal = Self::open(Self::path(directory, id)?, true)?;
        let seats = config
            .claude_seats()
            .map(|(seat, model)| {
                let epoch = *epochs.get(&seat).expect("configured seat epoch");
                (
                    seat,
                    SeatJournal {
                        epoch,
                        model: model.into(),
                        session: None,
                        calls: 0,
                        turns: 0,
                        wall_millis: 0,
                        recoveries: 0,
                        incomplete_turns: 0,
                        reported_cost_usd: 0.,
                        last_session_cost: 0.,
                        compactions: 0,
                        telemetry: SessionTelemetry::default(),
                        stages: BTreeMap::new(),
                        inflight: None,
                        usage: Some(UsageJournal::default()),
                    },
                )
            })
            .collect();
        let state = JournalState {
            version: 1,
            campaign_id: id.into(),
            config,
            seats,
        };
        let encoded = serde_json::to_string(&state).map_err(|e| e.to_string())?;
        journal
            .db
            .lock()
            .map_err(|e| e.to_string())?
            .execute("INSERT INTO journal VALUES(1,?1)", params![encoded])
            .map_err(|e| format!("journal already exists or cannot be created: {e}"))?;
        Ok(journal)
    }
    pub fn recover(directory: &Path, id: &str) -> Result<Self, String> {
        let journal = Self::open(Self::path(directory, id)?, false)?;
        let state = journal.snapshot()?;
        if state.version != 1 || state.campaign_id != id {
            return Err("journal identity/version mismatch".into());
        }
        state.config.validate()?;
        if state.config.session.is_none() {
            return Err("missing durable limits".into());
        }
        if state.seats.len() != state.config.claude_seats().count() {
            return Err("journal seat roster mismatch".into());
        }
        for (seat, model) in state.config.claude_seats() {
            let s = state.seats.get(&seat).ok_or("journal seat missing")?;
            if s.model != model
                || !s.last_session_cost.is_finite()
                || s.last_session_cost < 0.
                || !s.reported_cost_usd.is_finite()
                || s.reported_cost_usd < 0.
            {
                return Err("invalid journal session metadata".into());
            }
            if let Some(info) = &s.session {
                uuid::Uuid::parse_str(&info.session_id)
                    .map_err(|_| "invalid stored CLI session id")?;
            }
        }
        // Keep the full interrupted reservation charged. Result-less turns have unknown spend.
        journal.update(|state| {
            for s in state.seats.values_mut() {
                let migrated = migrate_usage(s);
                let interrupted = s.inflight.take().is_some_and(|r| r.kind == "turn");
                if interrupted {
                    s.incomplete_turns += 1;
                }
                if interrupted || (migrated && s.turns > 0) {
                    queue_usage(s)?;
                }
            }
            Ok(())
        })?;
        Ok(journal)
    }
    /// Settle a cancelled operation without claiming result-less model spend is known.
    /// Its full pre-action reservation remains charged, as on hard-crash recovery.
    pub fn interrupt(&self, seat: SeatId) -> Result<(), String> {
        self.update(|state| {
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            migrate_usage(s);
            if s.inflight.take().is_some_and(|r| r.kind == "turn") {
                s.incomplete_turns += 1;
                queue_usage(s)?;
            }
            Ok(())
        })
    }
    pub fn usage_outbox(&self, seat: SeatId) -> Result<Vec<UsageSnapshot>, String> {
        let state = self.snapshot()?;
        Ok(state
            .seats
            .get(&seat)
            .ok_or("unknown seat")?
            .usage
            .as_ref()
            .map(|u| u.outbox.clone())
            .unwrap_or_default())
    }
    /// Only acknowledge the exact committed prefix whose delivery marker was confirmed.
    pub fn acknowledge_usage(&self, seat: SeatId, epoch: u64, revision: u64) -> Result<(), String> {
        self.update(|state| {
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            if s.epoch != epoch {
                return Err("usage acknowledgement epoch mismatch".into());
            }
            let u = s.usage.as_mut().ok_or("missing usage journal")?;
            if revision <= u.acknowledged_revision {
                return Ok(());
            }
            if !u.outbox.iter().any(|entry| entry.revision == revision) {
                return Err("usage acknowledgement was not committed".into());
            }
            u.outbox.retain(|entry| entry.revision > revision);
            u.acknowledged_revision = revision;
            Ok(())
        })
    }
    /// Deliver a committed prefix. A crash after server commit but before this
    /// acknowledgement replays the same epoch/revision, never a new cost increment.
    pub async fn deliver_usage(
        &self,
        seat: SeatId,
        sink: &cna_seats::transcript::TranscriptSink,
    ) -> Result<(), String> {
        let committed = self.usage_outbox(seat)?;
        let Some(last) = committed.last() else {
            return Ok(());
        };
        for snapshot in &committed {
            sink.emit(
                seat,
                cna_protocol::TranscriptEntry::UsageSnapshot(Box::new(snapshot.clone())),
            );
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), sink.flush_confirmed())
            .await
            .map_err(|_| {
                "usage transcript confirmation timed out; committed outbox retained".to_string()
            })??;
        self.acknowledge_usage(seat, last.controller_epoch, last.revision)
    }
    pub fn file(&self) -> &Path {
        &self.path
    }
    pub fn snapshot(&self) -> Result<JournalState, String> {
        let db = self.db.lock().map_err(|e| e.to_string())?;
        Self::read(&db)
    }
    fn read(db: &Connection) -> Result<JournalState, String> {
        let text: String = db
            .query_row("SELECT state FROM journal WHERE id=1", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| format!("invalid session journal: {e}"))
    }
    fn update<T>(
        &self,
        action: impl FnOnce(&mut JournalState) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut db = self.db.lock().map_err(|e| e.to_string())?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let mut state = Self::read(&tx)?;
        let result = action(&mut state)?;
        let text = serde_json::to_string(&state).map_err(|e| e.to_string())?;
        tx.execute("UPDATE journal SET state=?1 WHERE id=1", params![text])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(result)
    }
    pub fn session(&self, seat: SeatId, info: SessionInfo) -> Result<(), String> {
        uuid::Uuid::parse_str(&info.session_id).map_err(|_| "invalid CLI session id")?;
        self.update(|state| {
            let s = state.seats.get_mut(&seat).ok_or("unknown journal seat")?;
            if s.session.as_ref().map(|i| &i.session_id) != Some(&info.session_id) {
                s.last_session_cost = 0.;
            }
            s.session = Some(info);
            s.telemetry = SessionTelemetry::default();
            Ok(())
        })
    }
    pub fn recover_process(&self, seat: SeatId, reseed: bool) -> Result<(), String> {
        self.update(|state| {
            let max = state
                .config
                .session
                .as_ref()
                .ok_or("missing limits")?
                .recoveries;
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            if s.recoveries >= max {
                return Err("session recovery budget exhausted".into());
            }
            s.recoveries += 1;
            if reseed {
                s.session = None;
                s.last_session_cost = 0.;
            }
            Ok(())
        })
    }
    /// Reserve a bounded operation before it starts; the reservation remains charged on hard kill.
    pub fn reserve(
        &self,
        seat: SeatId,
        kind: &str,
        stage: &str,
        requested_ms: u64,
    ) -> Result<u64, String> {
        self.update(|state| {
            let limits = state.config.session.as_ref().ok_or("missing limits")?;
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            if s.inflight.is_some() {
                return Err("overlapping model operations".into());
            }
            if kind == "automatic" {
                s.stages.entry(stage.into()).or_default().automatic_attempts += 1;
            }
            if kind == "turn" {
                if s.turns >= state.config.max_turns as u64 {
                    return Err("durable turn budget exhausted".into());
                }
                if s.calls >= state.config.tool_calls {
                    return Err("durable tool budget exhausted".into());
                }
                s.turns += 1;
                s.stages.entry(stage.into()).or_default().attempts += 1;
            }
            let remaining = limits
                .wall_seconds
                .saturating_mul(1000)
                .saturating_sub(s.wall_millis);
            let millis = requested_ms.min(remaining);
            if millis == 0 {
                return Err("durable wall-clock budget exhausted".into());
            }
            s.wall_millis += millis;
            s.inflight = Some(Reservation {
                kind: kind.into(),
                stage: stage.into(),
                millis,
                decision: None,
            });
            Ok(millis)
        })
    }
    pub fn reserve_decision(
        &self,
        seat: SeatId,
        stage: &str,
        millis: u64,
        id: &str,
        revision: u32,
    ) -> Result<u64, String> {
        let reserved = self.reserve(seat, "turn", stage, millis)?;
        self.update(|state| {
            state
                .seats
                .get_mut(&seat)
                .ok_or("unknown seat")?
                .inflight
                .as_mut()
                .ok_or("missing reservation")?
                .decision = Some((id.into(), revision));
            Ok(())
        })?;
        Ok(reserved)
    }
    /// Finish a local automatic operation without attributing it to a model turn.
    pub fn complete_automatic(
        &self,
        seat: SeatId,
        elapsed_ms: u64,
        accepted: bool,
    ) -> Result<(), String> {
        self.update(|state| {
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            let r = s.inflight.take().ok_or("no reserved operation")?;
            if r.kind != "automatic" {
                return Err("not an automatic reservation".into());
            }
            s.wall_millis = s
                .wall_millis
                .saturating_sub(r.millis)
                .saturating_add(elapsed_ms.max(1));
            s.stages.entry(r.stage).or_default().automatic_completed += u64::from(accepted);
            Ok(())
        })
    }
    // CLI estimates use decimal USD; these values never enter game adjudication.
    #[allow(clippy::float_arithmetic)]
    pub fn complete(
        &self,
        seat: SeatId,
        elapsed_ms: u64,
        outcome: Option<&TurnOutcome>,
        telemetry: &SessionTelemetry,
    ) -> Result<(), String> {
        self.update(|state| {
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            let r = s.inflight.take().ok_or("no reserved operation")?;
            // Round up elapsed time; never charge less than one millisecond for an operation.
            s.wall_millis = s
                .wall_millis
                .saturating_sub(r.millis)
                .saturating_add(elapsed_ms.max(1));
            s.compactions += telemetry
                .compactions
                .saturating_sub(s.telemetry.compactions);
            s.telemetry = telemetry.clone();
            if let Some(info) = &mut s.session {
                if telemetry.model.is_some() {
                    info.model = telemetry.model.clone();
                }
                if telemetry.cli_version.is_some() {
                    info.cli_version = telemetry.cli_version.clone();
                }
            }
            if r.kind == "turn" {
                migrate_usage(s);
                let mut incomplete = false;
                let stage = s.stages.entry(r.stage).or_default();
                if let Some(o) = outcome {
                    stage.completed += u64::from(o.ok);
                    stage.input_tokens = stage
                        .input_tokens
                        .checked_add(o.usage.input_tokens.unwrap_or(0))
                        .ok_or("stage input overflow")?;
                    stage.output_tokens = stage
                        .output_tokens
                        .checked_add(o.usage.output_tokens.unwrap_or(0))
                        .ok_or("stage output overflow")?;
                    add_channel(&mut stage.cache_read_tokens, o.usage.cached_input_tokens)?;
                    add_channel(
                        &mut stage.cache_creation_tokens,
                        o.usage.cache_creation_tokens,
                    )?;
                    add_channel(&mut stage.reasoning_tokens, o.usage.reasoning_tokens)?;
                    let usage = s.usage.as_mut().ok_or("missing usage journal")?;
                    add_channel(&mut usage.input_tokens, o.usage.input_tokens)?;
                    add_channel(&mut usage.output_tokens, o.usage.output_tokens)?;
                    add_channel(&mut usage.cache_read_tokens, o.usage.cached_input_tokens)?;
                    add_channel(
                        &mut usage.cache_creation_tokens,
                        o.usage.cache_creation_tokens,
                    )?;
                    add_channel(&mut usage.reasoning_tokens, o.usage.reasoning_tokens)?;
                    incomplete |= o.usage.input_tokens.is_none() || o.usage.output_tokens.is_none();
                    if let Some(raw) = o.usage.cost_usd {
                        if !raw.is_finite() || raw < 0. {
                            return Err("invalid reported cost".into());
                        }
                        // Preserve the existing per-session delta rules across resume/reseed.
                        let delta = if raw >= s.last_session_cost {
                            raw - s.last_session_cost
                        } else {
                            incomplete = true;
                            raw
                        };
                        s.last_session_cost = raw;
                        s.reported_cost_usd += delta;
                        stage.reported_cost_usd += delta;
                        if !s.reported_cost_usd.is_finite() {
                            return Err("reported cost overflow".into());
                        }
                        usage.reported_cost_usd = Some(s.reported_cost_usd);
                    } else {
                        incomplete = true;
                    }
                } else {
                    incomplete = true;
                }
                s.incomplete_turns += u64::from(incomplete);
                queue_usage(s)?;
            }
            Ok(())
        })
    }
}
impl ToolBudget for SessionJournal {
    fn authorize(
        &self,
        seat: SeatId,
        epoch: u64,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<(), String> {
        let state = self.snapshot()?;
        let s = state.seats.get(&seat).ok_or("unknown seat")?;
        if s.epoch != epoch {
            return Err("journal epoch differs from endpoint".into());
        }
        if tool == "submit" {
            let decision = s
                .inflight
                .as_ref()
                .and_then(|r| r.decision.as_ref())
                .ok_or("no authorized decision")?;
            if args["decision_id"].as_str() != Some(decision.0.as_str())
                || args["revision"].as_u64() != Some(u64::from(decision.1))
            {
                return Err(
                    "only the requested decision revision is authorized in this turn".into(),
                );
            }
        }
        Ok(())
    }

    fn charge(&self, seat: SeatId) -> Result<(), String> {
        self.update(|state| {
            let s = state.seats.get_mut(&seat).ok_or("unknown seat")?;
            if s.inflight.as_ref().map(|r| r.kind.as_str()) != Some("turn") {
                return Err("no authorized model turn".into());
            }
            if s.calls >= state.config.tool_calls {
                return Err("durable tool budget exhausted".into());
            }
            s.calls += 1;
            Ok(())
        })
    }
}
#[cfg(test)]
#[allow(clippy::float_arithmetic)]
mod tests {
    use super::*;
    use crate::config::{GameKind, SessionLimits};
    use cna_seats::driver::{SessionTelemetry, Usage};
    fn config() -> LaunchConfig {
        let mut c = LaunchConfig::resolve(
            GameKind::Cna,
            &[
                "axis.front_line=claude:haiku".into(),
                "*=scripted:legal_random".into(),
            ],
        )
        .unwrap();
        c.session = Some(SessionLimits::default());
        c.max_turns = 20;
        c.tool_calls = 6;
        c
    }
    fn seat() -> SeatId {
        "axis.front_line".parse().unwrap()
    }
    fn outcome(cost: f64) -> TurnOutcome {
        TurnOutcome {
            ok: true,
            error: None,
            text: None,
            usage: Usage {
                cost_usd: Some(cost),
                input_tokens: Some(10),
                output_tokens: Some(5),
                ..Usage::default()
            },
            quota: vec![],
        }
    }
    #[test]
    fn usage_outbox_survives_restart_and_acknowledges_only_confirmed_revisions() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        let sid = uuid::Uuid::new_v4().to_string();
        let info = SessionInfo {
            session_id: sid,
            model: Some("haiku".into()),
            cli_version: None,
            resumed: false,
        };
        journal.session(seat(), info.clone()).unwrap();
        let mut report = outcome(0.01);
        report.usage.cached_input_tokens = Some(100);
        report.usage.cache_creation_tokens = Some(30);
        report.usage.reasoning_tokens = Some(2);
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        journal
            .complete(seat(), 10, Some(&report), &SessionTelemetry::default())
            .unwrap();
        let first = journal.usage_outbox(seat()).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(
            (
                first[0].controller_epoch,
                first[0].revision,
                first[0].attempts,
                first[0].completed
            ),
            (7, 1, 1, 1)
        );
        assert_eq!(
            (
                first[0].input_tokens,
                first[0].cache_read_tokens,
                first[0].cache_creation_tokens,
                first[0].reasoning_tokens
            ),
            (Some(10), Some(100), Some(30), Some(2))
        );
        drop(journal); // crash after journal commit or transcript delivery, before acknowledgement
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        assert_eq!(journal.usage_outbox(seat()).unwrap(), first);
        journal
            .session(
                seat(),
                SessionInfo {
                    resumed: true,
                    ..info
                },
            )
            .unwrap();
        report.usage.cost_usd = Some(0.025);
        journal
            .reserve_decision(seat(), "GT1:OpStage2", 1000, "d2", 1)
            .unwrap();
        journal
            .complete(seat(), 10, Some(&report), &SessionTelemetry::default())
            .unwrap();
        let second = journal.usage_outbox(seat()).unwrap();
        assert_eq!(second.len(), 2);
        assert_eq!(
            (
                second[1].revision,
                second[1].input_tokens,
                second[1].cache_read_tokens,
                second[1].cache_creation_tokens,
                second[1].reasoning_tokens
            ),
            (2, Some(20), Some(200), Some(60), Some(4))
        );
        assert_eq!(second[1].reported_cost_usd, Some(0.025));
        assert!(journal.acknowledge_usage(seat(), 8, 2).is_err());
        assert!(journal.acknowledge_usage(seat(), 7, 3).is_err());
        assert_eq!(journal.usage_outbox(seat()).unwrap(), second);
        journal.acknowledge_usage(seat(), 7, 1).unwrap();
        assert_eq!(journal.usage_outbox(seat()).unwrap(), second[1..]);
        journal.acknowledge_usage(seat(), 7, 2).unwrap();
        journal.acknowledge_usage(seat(), 7, 2).unwrap();
        assert!(journal.usage_outbox(seat()).unwrap().is_empty());
        journal.recover_process(seat(), true).unwrap();
        report.usage.cost_usd = Some(0.005);
        journal
            .reserve_decision(seat(), "GT2:OpStage1", 1000, "d3", 1)
            .unwrap();
        journal
            .complete(seat(), 10, Some(&report), &SessionTelemetry::default())
            .unwrap();
        let third = journal.usage_outbox(seat()).unwrap();
        assert_eq!(third[0].revision, 3);
        assert!((third[0].reported_cost_usd.unwrap() - 0.03).abs() < 1e-10);
        assert_eq!(third[0].input_tokens, Some(30));
    }
    #[test]
    fn unusable_provider_counts_roll_back_and_become_an_unknown_turn() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        let mut report = outcome(0.01);
        report.usage.input_tokens = Some(u64::MAX);
        assert!(
            journal
                .complete(seat(), 10, Some(&report), &SessionTelemetry::default())
                .is_err()
        );
        assert!(journal.usage_outbox(seat()).unwrap().is_empty());
        assert!(
            journal.snapshot().unwrap().seats[&seat()]
                .inflight
                .is_some()
        );
        journal.interrupt(seat()).unwrap();
        let snapshot = journal.usage_outbox(seat()).unwrap()[0].clone();
        assert_eq!(
            (
                snapshot.attempts,
                snapshot.completed,
                snapshot.incomplete_turns
            ),
            (1, 0, 1)
        );
        assert_eq!(
            (snapshot.input_tokens, snapshot.reported_cost_usd),
            (None, None)
        );
    }
    #[test]
    fn interrupted_usage_is_durable_null_and_never_counted_twice() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        drop(journal);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        let snapshot = journal.usage_outbox(seat()).unwrap()[0].clone();
        assert_eq!(
            (
                snapshot.attempts,
                snapshot.completed,
                snapshot.incomplete_turns
            ),
            (1, 0, 1)
        );
        assert_eq!(
            (
                snapshot.input_tokens,
                snapshot.output_tokens,
                snapshot.reported_cost_usd
            ),
            (None, None, None)
        );
        journal.interrupt(seat()).unwrap();
        assert_eq!(
            journal.usage_outbox(seat()).unwrap(),
            vec![snapshot.clone()]
        );
        journal
            .reserve_decision(seat(), "GT1:OpStage2", 1000, "d2", 1)
            .unwrap();
        journal.interrupt(seat()).unwrap();
        drop(journal);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        let entries = journal.usage_outbox(seat()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            (
                entries[1].revision,
                entries[1].attempts,
                entries[1].incomplete_turns
            ),
            (2, 2, 2)
        );
        assert_eq!(journal.snapshot().unwrap().seats[&seat()].wall_millis, 2000);
    }
    #[tokio::test]
    async fn usage_delivery_confirms_before_ack_and_replays_after_a_dead_sink() {
        use cna_seats::transcript::{LocalTranscript, TranscriptSink};
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        journal
            .complete(
                seat(),
                10,
                Some(&outcome(0.01)),
                &SessionTelemetry::default(),
            )
            .unwrap();
        let store = LocalTranscript::detached();
        let sink = TranscriptSink::new(store.clone());
        journal.deliver_usage(seat(), &sink).await.unwrap();
        assert!(journal.usage_outbox(seat()).unwrap().is_empty());
        assert_eq!(store.seat_log(seat()).len(), 1);
        assert!(sink.stop_delivery().await.is_empty());
        journal
            .reserve_decision(seat(), "GT1:OpStage2", 1000, "d2", 1)
            .unwrap();
        journal
            .complete(
                seat(),
                10,
                Some(&outcome(0.025)),
                &SessionTelemetry::default(),
            )
            .unwrap();
        let committed = journal.usage_outbox(seat()).unwrap();
        assert!(journal.deliver_usage(seat(), &sink).await.is_err());
        assert_eq!(journal.usage_outbox(seat()).unwrap(), committed);
        drop(journal);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        let recovered = TranscriptSink::new(store.clone());
        journal.deliver_usage(seat(), &recovered).await.unwrap();
        assert!(journal.usage_outbox(seat()).unwrap().is_empty());
        let rows = store.seat_log(seat());
        assert_eq!(rows.len(), 2);
        let cna_protocol::TranscriptEntry::UsageSnapshot(snapshot) = &rows[1].entry else {
            panic!("missing typed usage")
        };
        assert_eq!(snapshot.as_ref(), &committed[0]);
        assert!(recovered.stop_delivery().await.is_empty());
    }
    #[test]
    fn old_journals_do_not_invent_auxiliary_or_missing_token_totals() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        journal
            .complete(
                seat(),
                10,
                Some(&outcome(0.01)),
                &SessionTelemetry::default(),
            )
            .unwrap();
        let path = journal.file().to_owned();
        drop(journal);
        let db = Connection::open(&path).unwrap();
        let mut old: serde_json::Value = serde_json::from_str(
            &db.query_row::<String, _, _>("SELECT state FROM journal WHERE id=1", [], |r| r.get(0))
                .unwrap(),
        )
        .unwrap();
        old["seats"][seat().to_string()]
            .as_object_mut()
            .unwrap()
            .remove("usage");
        db.execute("UPDATE journal SET state=?1 WHERE id=1", [old.to_string()])
            .unwrap();
        drop(db);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        let snapshot = journal.usage_outbox(seat()).unwrap()[0].clone();
        assert_eq!(
            (
                snapshot.attempts,
                snapshot.completed,
                snapshot.incomplete_turns
            ),
            (1, 1, 1)
        );
        assert_eq!(
            (
                snapshot.input_tokens,
                snapshot.cache_read_tokens,
                snapshot.cache_creation_tokens,
                snapshot.reasoning_tokens
            ),
            (None, None, None, None)
        );
        assert_eq!(snapshot.reported_cost_usd, Some(0.01));
    }
    #[test]
    fn automatic_answers_preserve_model_caps_cost_and_crash_reservations() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let mut limits = config();
        limits.tool_calls = 1;
        limits.max_turns = 1;
        let journal =
            SessionJournal::create(root.path(), &id, limits, &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 1000, "d1", 1)
            .unwrap();
        ToolBudget::charge(&journal, seat()).unwrap();
        journal
            .complete(
                seat(),
                10,
                Some(&outcome(0.01)),
                &SessionTelemetry::default(),
            )
            .unwrap();
        journal
            .reserve(seat(), "automatic", "GT1:OpStage1", 5000)
            .unwrap();
        assert!(ToolBudget::charge(&journal, seat()).is_err());
        journal.complete_automatic(seat(), 12, true).unwrap();
        let state = journal.snapshot().unwrap();
        let record = &state.seats[&seat()];
        assert_eq!((record.turns, record.calls, record.wall_millis), (1, 1, 22));
        assert!((record.reported_cost_usd - 0.01).abs() < 1e-9);
        assert_eq!(record.stages["GT1:OpStage1"].automatic_completed, 1);
        journal
            .reserve(seat(), "automatic", "GT1:OpStage1", 5000)
            .unwrap();
        drop(journal);
        let recovered = SessionJournal::recover(root.path(), &id).unwrap();
        let state = recovered.snapshot().unwrap();
        let record = &state.seats[&seat()];
        assert_eq!(
            (
                record.turns,
                record.calls,
                record.wall_millis,
                record.incomplete_turns
            ),
            (1, 1, 5022, 0)
        );
        assert_eq!(record.stages["GT1:OpStage1"].automatic_attempts, 2);
        assert_eq!(record.stages["GT1:OpStage1"].automatic_completed, 1);
        let old:StageUsage=serde_json::from_str(r#"{"attempts":1,"completed":1,"reported_cost_usd":0,"input_tokens":0,"output_tokens":0}"#).unwrap();
        assert_eq!(old.automatic_completed, 0);
    }
    #[test]
    fn hard_crash_keeps_charge_lease_and_lifetime_call_cap() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        assert!(SessionJournal::recover(root.path(), &id).is_err());
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 60_000, "d1", 2)
            .unwrap();
        for _ in 0..3 {
            journal.charge(seat()).unwrap();
        }
        assert!(
            journal
                .authorize(
                    seat(),
                    7,
                    "submit",
                    &serde_json::json!({"decision_id":"d1","revision":3})
                )
                .is_err()
        );
        assert!(
            journal
                .authorize(seat(), 8, "observe", &serde_json::json!({}))
                .is_err()
        );
        assert!(
            journal
                .authorize(
                    seat(),
                    7,
                    "submit",
                    &serde_json::json!({"decision_id":"d1","revision":2})
                )
                .is_ok()
        );
        drop(journal);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        let s = &journal.snapshot().unwrap().seats[&seat()];
        assert_eq!(
            (s.turns, s.calls, s.wall_millis, s.incomplete_turns),
            (1, 3, 60_000, 1)
        );
        assert!(journal.charge(seat()).is_err());
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 60_000, "d1", 3)
            .unwrap();
        for _ in 0..3 {
            journal.charge(seat()).unwrap();
        }
        assert!(journal.charge(seat()).is_err());
    }
    #[test]
    fn cumulative_cost_deltas_and_compactions_survive_same_session_resume() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        let sid = uuid::Uuid::new_v4().to_string();
        let info = SessionInfo {
            session_id: sid.clone(),
            model: Some("haiku".into()),
            cli_version: None,
            resumed: false,
        };
        journal.session(seat(), info.clone()).unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage1", 60_000, "d1", 1)
            .unwrap();
        journal
            .complete(
                seat(),
                20,
                Some(&outcome(0.01)),
                &SessionTelemetry {
                    compactions: 1,
                    context_tokens: Some(1000),
                    ..SessionTelemetry::default()
                },
            )
            .unwrap();
        drop(journal);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        journal
            .session(
                seat(),
                SessionInfo {
                    resumed: true,
                    ..info
                },
            )
            .unwrap();
        journal
            .reserve_decision(seat(), "GT1:OpStage2", 60_000, "d2", 1)
            .unwrap();
        journal
            .complete(
                seat(),
                30,
                Some(&outcome(0.025)),
                &SessionTelemetry {
                    compactions: 1,
                    ..SessionTelemetry::default()
                },
            )
            .unwrap();
        let s = &journal.snapshot().unwrap().seats[&seat()];
        assert!((s.reported_cost_usd - 0.025).abs() < 1e-10);
        assert!((s.stages["GT1:OpStage2"].reported_cost_usd - 0.015).abs() < 1e-10);
        assert_eq!((s.compactions, s.wall_millis, s.turns), (2, 50, 2));
        assert_eq!(s.session.as_ref().unwrap().session_id, sid);
    }
    #[test]
    fn unavailable_session_reseed_keeps_budget_and_caps_recovery() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal
            .reserve_decision(seat(), "setup", 60_000, "d1", 1)
            .unwrap();
        journal.charge(seat()).unwrap();
        journal
            .complete(seat(), 10, None, &SessionTelemetry::default())
            .unwrap();
        journal.recover_process(seat(), true).unwrap();
        journal.recover_process(seat(), false).unwrap();
        assert!(journal.recover_process(seat(), true).is_err());
        let s = &journal.snapshot().unwrap().seats[&seat()];
        assert_eq!((s.turns, s.calls, s.recoveries), (1, 1, 2));
        drop(journal);
        std::fs::write(SessionJournal::path(root.path(), &id).unwrap(), b"corrupt").unwrap();
        assert!(SessionJournal::recover(root.path(), &id).is_err());
    }
    #[test]
    fn measured_overrun_is_charged_and_survives_recovery() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let journal =
            SessionJournal::create(root.path(), &id, config(), &BTreeMap::from([(seat(), 7)]))
                .unwrap();
        journal.reserve(seat(), "idle", "idle", 1000).unwrap();
        journal
            .complete(seat(), 1200, None, &SessionTelemetry::default())
            .unwrap();
        drop(journal);
        let journal = SessionJournal::recover(root.path(), &id).unwrap();
        assert_eq!(journal.snapshot().unwrap().seats[&seat()].wall_millis, 1200);
        journal.reserve(seat(), "idle", "idle", 1000).unwrap();
        journal
            .complete(seat(), u64::MAX, None, &SessionTelemetry::default())
            .unwrap();
        assert!(journal.reserve(seat(), "idle", "idle", 1000).is_err());
    }
}
