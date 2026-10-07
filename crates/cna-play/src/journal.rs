//! Trusted launcher journal, separate from the campaign event log and seat working directories.
//! SQLite commits precede model work. In-flight reservations survive a hard crash.
use crate::config::LaunchConfig;
use cna_core::ids::SeatId;
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
    pub attempts: u64,
    pub completed: u64,
    pub reported_cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
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
                if let Some(r) = s.inflight.take()
                    && r.kind == "turn"
                {
                    s.incomplete_turns += 1;
                }
            }
            Ok(())
        })?;
        Ok(journal)
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
                let stage = s.stages.entry(r.stage).or_default();
                if let Some(o) = outcome {
                    stage.completed += u64::from(o.ok);
                    stage.input_tokens += o.usage.input_tokens.unwrap_or(0);
                    stage.output_tokens += o.usage.output_tokens.unwrap_or(0);
                    if let Some(raw) = o.usage.cost_usd {
                        if !raw.is_finite() || raw < 0. {
                            return Err("invalid reported cost".into());
                        }
                        // Normal resumes restore totals. A CLI reset is visible and conservatively counted.
                        let delta = if raw >= s.last_session_cost {
                            raw - s.last_session_cost
                        } else {
                            s.incomplete_turns += 1;
                            raw
                        };
                        s.last_session_cost = raw;
                        s.reported_cost_usd += delta;
                        stage.reported_cost_usd += delta;
                    } else {
                        s.incomplete_turns += 1;
                    }
                } else {
                    s.incomplete_turns += 1;
                }
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
                ..Usage::default()
            },
            quota: vec![],
        }
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
