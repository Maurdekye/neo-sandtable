//! m5b7759e8ebc9: ONE prepaid process connection, fixed catalog, no worker I/O.
//! J is a pinned accounting allowance, NOT an enforced private heap ceiling.
//! Public lease issuance and complete namespace/wholefit validation are staged.

use std::alloc::Layout;
use std::fs::{File, TryLockError};
#[cfg(test)]
use std::fs::OpenOptions;
use std::io;
use std::path::{Component, Path};
#[cfg(windows)]
use std::sync::{Arc, Weak};

use rusqlite::{Connection, OpenFlags};
use rusqlite::limits::Limit;

mod member;

use super::original::{ChargeRef, Source, prepay};
use crate::operation::OperationError;
#[cfg(test)]
use crate::operation::Operation;

pub(super) const J_BYTES: usize = 8_388_608;
const STATEMENT_COUNT: usize = 35;
const CATALOG: &str = include_str!("journal/statements.sql");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Observation { Pending, Configured, Refused }

#[derive(Clone, Copy)]
pub(super) struct OpenBinding { pub(super) generation: i64, pub(super) incarnation: [u8; 16] }

#[derive(Clone, Copy)]
struct PriorHost { generation: i64, incarnation: [u8; 16] }

#[derive(Clone, Copy, PartialEq, Eq)]
enum Schema { Empty, Exact }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal { Closed, Bounds, Configuration, Catalog, Entropy, DuplicateMember, MemberMismatch }

pub(super) enum Cause {
    Sqlite(rusqlite::Error),
    Credit(OperationError),
    Refusal(Refusal),
    #[cfg(windows)]
    Namespace(super::namespace::Refusal),
}

pub(super) enum FailureRef<'a> {
    #[cfg(windows)]
    Entropy(&'a getrandom::Error),
    File(&'a io::Error),
    Lock(&'a TryLockError),
    Open(&'a rusqlite::Error),
    Stage(&'a Cause),
    Cas(&'a Result<usize, rusqlite::Error>),
}

pub(super) struct FailureReadback<'a> {
    pub(super) primary: Option<FailureRef<'a>>,
    pub(super) rollback: Option<&'a Result<(), rusqlite::Error>>,
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum Sql {
    CacheSize, MmapOff, SpillOn, TempFile, JournalDelete, FullSync,
    ForeignKeysOn, UntrustedSchema, NewPageSize,
    PageSizeRead, CacheSizeRead, MmapRead, SyncRead, ForeignKeysRead,
    JournalRead, CompileOptions, Schema, CreateHost, CreateMember,
    CreateWriter, ReadHost, OpenCas, ReadMember, InsertMember,
    StreamMembers, Freeze, InsertWriter, StreamWriters, WriterCount,
    UnmatchedMember, CleanCas, Begin, Commit, Rollback, TableInfo,
}

fn sql(statement: Sql) -> Option<&'static str> {
    CATALOG.lines().filter(|line| !line.is_empty() && !line.starts_with("--"))
        .nth(statement as usize)
}

/// The actual process owner retains this whole root, including failed opens,
/// library results and the namespace lock. Fixed observations never own errors.
/// No await, user callback, runtime lookup or thread creation occurs here.
pub(super) struct JournalRoot {
    original: ChargeRef,
    attempted: bool,
    configured: bool,
    failed: bool,
    rollback_attempted: bool,
    namespace_lock: Option<Result<File, io::Error>>,
    lock_result: Option<Result<(), TryLockError>>,
    connection: Option<Result<Connection, rusqlite::Error>>,
    step: Option<Result<(), Cause>>,
    rollback: Option<Result<(), rusqlite::Error>>,
    schema: Option<Result<Schema, Cause>>,
    prior: Option<Result<Option<PriorHost>, Cause>>,
    open_cas: Option<Result<usize, rusqlite::Error>>,
    durable_open: Option<OpenBinding>,
    members: Option<Box<member::MemberAttempt>>,
    #[cfg(windows)]
    nonce: [u8; 16],
    #[cfg(windows)]
    entropy_result: Option<Result<(), getrandom::Error>>,
    #[cfg(windows)]
    namespace_association: Option<Weak<super::namespace::JournalAssociation>>,
}

impl JournalRoot {
    pub(super) fn reserve(original: &impl Source) -> Result<Self, OperationError> {
        // Exact Box payload plus the approved journal-only class. Neither charge
        // is refunded, repeated on failure, or assigned to a fresh operation.
        prepay(original, Layout::new::<Self>())?;
        prepay(original, Layout::array::<u8>(J_BYTES)
            .map_err(|_| OperationError::CaptureOverflow)?)?;
        Ok(Self {
            original: original.charge_ref(), attempted: false, configured: false,
            failed: false, rollback_attempted: false,
            namespace_lock: None, lock_result: None, connection: None,
            step: None, rollback: None,
            schema: None, prior: None, open_cas: None, durable_open: None, members: None,
            #[cfg(windows)] nonce: [0; 16],
            #[cfg(windows)] entropy_result: None,
            #[cfg(windows)] namespace_association: None,
        })
    }

    fn connection(&self) -> Option<&Connection> {
        self.connection.as_ref()?.as_ref().ok()
    }

    fn observation(&self) -> Observation {
        if self.failed { Observation::Refused }
        else if self.configured { Observation::Configured }
        else { Observation::Pending }
    }

    fn refuse(&mut self, reason: Refusal) -> Observation {
        if !self.failed {
            self.step = Some(Err(Cause::Refusal(reason)));
            self.failed = true;
            self.rollback_once();
        }
        Observation::Refused
    }

    /// Historical primitive bounds/failure controls ONLY. Production source
    /// cannot open caller paths or invent a newness boolean through this seam.
    #[cfg(test)]
    pub(super) fn open_once(
        &mut self, database: &Path, lock_path: &Path, new_database: bool,
    ) -> Observation {
        if self.failed || self.attempted { return Observation::Refused; }
        self.attempted = true; // Even failure cannot open another connection.
        if !bounded_namespace(database, lock_path) {
            return self.refuse(Refusal::Bounds);
        }
        if let Err(error) = self.original.check() {
            self.step = Some(Err(Cause::Credit(error)));
            self.failed = true;
            return Observation::Refused;
        }
        self.namespace_lock = Some(OpenOptions::new()
            .read(true).write(true).create(true).truncate(false).open(lock_path));
        let Some(Ok(lock)) = self.namespace_lock.as_ref() else {
            self.failed = true;
            return Observation::Refused;
        };
        self.lock_result = Some(lock.try_lock());
        if !matches!(self.lock_result.as_ref(), Some(Ok(()))) {
            self.failed = true;
            return Observation::Refused;
        }
        if let Err(error) = self.original.check() {
            self.step = Some(Err(Cause::Credit(error)));
            self.failed = true;
            return Observation::Refused;
        }
        self.open_database(database, new_database)
    }

    /// One connection against the same retained locked namespace graph. The
    /// caller cannot supply a path/newness tuple or export/replace the File.
    #[cfg(windows)]
    pub(super) fn open_guarded_once(&mut self, namespace: &mut super::namespace::LayoutRoot) -> Observation {
        if self.failed || self.attempted { return Observation::Refused; }
        self.attempted = true;
        let opened = {
            let inputs = match namespace.journal_inputs(&self.original) {
                Ok(inputs) => inputs,
                Err(reason) => return self.namespace_refusal(reason),
            };
            if !bounded_namespace(inputs.database, inputs.lock_path) { return self.refuse(Refusal::Bounds); }
            // Weak retains the allocation identity even if its owner is lost;
            // upgrading requires the actual retained LayoutRoot still alive.
            // No path string, caller nonce or same-meter shortcut is authority.
            self.namespace_association = Some(Arc::downgrade(inputs.association));
            self.open_database(inputs.database, inputs.new_database)
        };
        if opened == Observation::Refused { return opened; }
        // The whole connection result is already retained, including failure.
        // Recheck the actual original handles after synchronous open/configure.
        if let Err(reason) = self.check_namespace_owner(namespace) { return self.namespace_refusal(reason); }
        opened
    }

    #[cfg(windows)]
    fn check_namespace_owner(&self, namespace: &mut super::namespace::LayoutRoot)
        -> Result<(), super::namespace::Refusal>
    {
        use super::namespace::Refusal;
        let retained = self.namespace_association.as_ref()
            .and_then(Weak::upgrade).ok_or(Refusal::Closed)?;
        let inputs = namespace.journal_inputs(&self.original)?;
        if !Arc::ptr_eq(&retained, inputs.association) { return Err(Refusal::Changed); }
        Ok(())
    }

    #[cfg(windows)]
    fn check_open_namespace(&self, namespace: &mut super::namespace::LayoutRoot)
        -> Result<(), super::namespace::Refusal>
    {
        self.check_namespace_owner(namespace)?;
        match namespace.revalidate_local_roots() {
            super::namespace::Observation::Observed => Ok(()),
            super::namespace::Observation::Refused(reason) => Err(reason),
        }
    }

    #[cfg(windows)]
    pub(super) fn context_original(&self) -> &ChargeRef { &self.original }

    /// Only an actual retained OPEN on its live namespace owner can supply
    /// this private context observation. No caller-provided generation/nonce.
    #[cfg(windows)]
    pub(super) fn context_binding(&mut self, namespace: &mut super::namespace::LayoutRoot)
        -> Result<OpenBinding, super::namespace::Refusal>
    {
        let binding = self.open_binding().copied().ok_or(super::namespace::Refusal::Closed)?;
        if let Err(reason) = self.check_open_namespace(namespace) {
            self.namespace_refusal(reason); return Err(reason);
        }
        Ok(binding)
    }

    #[cfg(windows)]
    fn namespace_refusal(&mut self, reason: super::namespace::Refusal) -> Observation {
        if !self.failed {
            self.step = Some(Err(Cause::Namespace(reason)));
            self.failed = true;
            self.rollback_once();
        }
        Observation::Refused
    }

    fn open_database(&mut self, database: &Path, new_database: bool) -> Observation {
        if let Err(error) = self.original.check() {
            self.step = Some(Err(Cause::Credit(error)));
            self.failed = true; return Observation::Refused;
        }
        // The file is already opened/retained. Never let SQLite create a
        // replacement name. No URI/shared-cache/external VFS/allocator hook.
        self.connection = Some(Connection::open_with_flags(database,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE));
        if !matches!(self.connection.as_ref(), Some(Ok(_))) {
            self.failed = true;
            return Observation::Refused;
        }
        self.configure(new_database)
    }

    fn configure(&mut self, new_database: bool) -> Observation {
        if self.failed { return Observation::Refused; }
        let Some(connection) = self.connection() else {
            return self.refuse(Refusal::Closed);
        };
        connection.set_prepared_statement_cache_capacity(STATEMENT_COUNT);
        // Existing safe limits feature, pinned manifest A9F4C5A7: limits=[];
        // no dependency/version resolution is introduced by that feature.
        const LIMITS: [(Limit, i32); 12] = [
            (Limit::SQLITE_LIMIT_LENGTH, 16_384),
            (Limit::SQLITE_LIMIT_SQL_LENGTH, 8_192),
            (Limit::SQLITE_LIMIT_COLUMN, 32),
            (Limit::SQLITE_LIMIT_EXPR_DEPTH, 64),
            (Limit::SQLITE_LIMIT_COMPOUND_SELECT, 1),
            (Limit::SQLITE_LIMIT_VDBE_OP, 512),
            (Limit::SQLITE_LIMIT_FUNCTION_ARG, 8),
            (Limit::SQLITE_LIMIT_ATTACHED, 0),
            (Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 16),
            (Limit::SQLITE_LIMIT_LIKE_PATTERN_LENGTH, 128),
            (Limit::SQLITE_LIMIT_TRIGGER_DEPTH, 0),
            (Limit::SQLITE_LIMIT_WORKER_THREADS, 0),
        ];
        for (limit, value) in LIMITS {
            if self.perform(|db| {
                db.set_limit(limit, value).map_err(Cause::Sqlite)?;
                if db.limit(limit).map_err(Cause::Sqlite)? != value {
                    return Err(Cause::Refusal(Refusal::Configuration));
                }
                Ok(())
            }) == Observation::Refused { return Observation::Refused; }
        }
        // J01 FIRST SQL, explicitly configure immediately after successful open.
        for statement in [Sql::CacheSize, Sql::MmapOff, Sql::SpillOn,
            Sql::TempFile, Sql::JournalDelete, Sql::FullSync,
            Sql::ForeignKeysOn, Sql::UntrustedSchema]
        {
            if self.batch(statement) == Observation::Refused { return Observation::Refused; }
        }
        if new_database && self.batch(Sql::NewPageSize) == Observation::Refused {
            return Observation::Refused;
        }
        for (statement, expected) in [(Sql::PageSizeRead, 4096),
            (Sql::CacheSizeRead, -128), (Sql::MmapRead, 0),
            (Sql::SyncRead, 2), (Sql::ForeignKeysRead, 1)]
        {
            if self.perform(|db| {
                let text = sql(statement).ok_or(Cause::Refusal(Refusal::Catalog))?;
                let actual: i64 = db.query_row(text, [], |row| row.get(0))
                    .map_err(Cause::Sqlite)?;
                if actual != expected { return Err(Cause::Refusal(Refusal::Configuration)); }
                Ok(())
            }) == Observation::Refused { return Observation::Refused; }
        }
        if self.perform(|db| {
            let text = sql(Sql::JournalRead).ok_or(Cause::Refusal(Refusal::Catalog))?;
            let matches = db.query_row(text, [], |row| {
                Ok(row.get_ref(0)?.as_str().is_ok_and(|mode| mode == "delete"))
            }).map_err(Cause::Sqlite)?;
            if !matches { return Err(Cause::Refusal(Refusal::Configuration)); }
            Ok(())
        }) == Observation::Refused { return Observation::Refused; }
        // Build/global lookaside inventory, exact schema, durable OPEN and
        // actual scope/member binding must follow before any lease can exist.
        // Configured is deliberately NOT Open, Healthy, Joined or Clean.
        self.configured = true;
        self.observation()
    }

    /// Staged source transaction, called only after configure + actual namespace
    /// and whole-process inventory verification. The nonce is fresh host-only
    /// data, never a Game RNG value or a caller-created campaign identity.
    /// No scope can be published from a mere Configured observation.
    #[cfg(windows)]
    pub(super) fn establish_fresh_open(&mut self, namespace: &mut super::namespace::LayoutRoot) -> Observation {
        if self.failed || !self.configured || self.durable_open.is_some() || self.entropy_result.is_some() {
            return Observation::Refused;
        }
        if let Err(reason) = self.check_open_namespace(namespace) { return self.namespace_refusal(reason); }
        if let Err(error) = self.original.check() {
            self.step = Some(Err(Cause::Credit(error)));
            self.failed = true; return Observation::Refused;
        }
        // Inline destination and WHOLE result belong to the already installed,
        // prepaid JournalRoot. Never Game RNG, uuid panic, formatting or retry.
        self.entropy_result = Some(getrandom::fill(&mut self.nonce));
        if !matches!(self.entropy_result.as_ref(), Some(Ok(()))) { return self.refuse(Refusal::Entropy); }
        // Refuse zero or repeated prior nonce; never request fresh bytes again.
        if self.nonce == [0; 16] { return self.refuse(Refusal::Entropy); }
        if let Err(reason) = self.check_open_namespace(namespace) { return self.namespace_refusal(reason); }
        let observed = self.establish_open(self.nonce);
        if observed == Observation::Refused { return observed; }
        if let Err(reason) = self.check_open_namespace(namespace) { return self.namespace_refusal(reason); }
        observed
    }

    // Caller-provided nonce values exist only in this module's old controls;
    // production siblings can use only the retained fresh producer above.
    fn establish_open(&mut self, incarnation: [u8; 16]) -> Observation {
        if self.failed || !self.configured || self.durable_open.is_some() {
            return Observation::Refused;
        }
        if incarnation == [0; 16] { return self.refuse(Refusal::Bounds); }
        if self.verify_compile_options() == Observation::Refused { return Observation::Refused; }
        if self.batch(Sql::Begin) == Observation::Refused { return Observation::Refused; }
        if self.perform(|_| Ok(())) == Observation::Refused { return Observation::Refused; }
        self.schema = Some(match self.connection() {
            Some(db) => inspect_schema(db),
            None => Err(Cause::Refusal(Refusal::Closed)),
        });
        let empty = match self.schema.as_ref() {
            Some(Ok(Schema::Empty)) => true,
            Some(Ok(Schema::Exact)) => false,
            _ => { self.fail_retained(); return Observation::Refused; }
        };
        if empty {
            for statement in [Sql::CreateHost, Sql::CreateMember, Sql::CreateWriter] {
                if self.batch(statement) == Observation::Refused { return Observation::Refused; }
            }
            self.schema = Some(match self.connection() {
                Some(db) => inspect_schema(db),
                None => Err(Cause::Refusal(Refusal::Closed)),
            });
            if !matches!(self.schema.as_ref(), Some(Ok(Schema::Exact))) {
                if matches!(self.schema.as_ref(), Some(Ok(_))) {
                    return self.refuse(Refusal::Configuration);
                }
                self.fail_retained();
                return Observation::Refused;
            }
        }
        if self.perform(|_| Ok(())) == Observation::Refused { return Observation::Refused; }
        self.prior = Some(match self.connection() {
            Some(db) => read_prior_host(db),
            None => Err(Cause::Refusal(Refusal::Closed)),
        });
        let old = match self.prior.as_ref() {
            Some(Ok(Some(prior))) if prior.incarnation != incarnation => prior.generation,
            Some(Ok(None)) if empty => 0,
            // Missing singleton in an existing schema is not a fresh database.
            Some(Ok(_)) => return self.refuse(Refusal::Configuration),
            _ => { self.fail_retained(); return Observation::Refused; }
        };
        let Some(generation) = old.checked_add(1) else { return self.refuse(Refusal::Bounds) };
        if self.perform(|_| Ok(())) == Observation::Refused { return Observation::Refused; }
        self.open_cas = Some(match self.connection() {
            Some(db) => match sql(Sql::OpenCas) {
                Some(text) => db.execute(text, rusqlite::params![1i64, generation, &incarnation[..], old]),
                None => return self.refuse(Refusal::Catalog),
            },
            None => return self.refuse(Refusal::Closed),
        });
        if !matches!(self.open_cas.as_ref(), Some(Ok(1))) {
            self.fail_retained();
            return Observation::Refused;
        }
        if self.batch(Sql::Commit) == Observation::Refused { return Observation::Refused; }
        // Acknowledged FULL OPEN invalidates the old CLEAN before any lease.
        // Prior positive CLEAN classification still requires full member/writer
        // validation; this code does not manufacture it from the host phase.
        self.durable_open = Some(OpenBinding { generation, incarnation });
        self.observation()
    }

    /// Borrow fixed binding only after acknowledged FULL OPEN. This is not a
    /// lease, positive creation, prior CLEAN classification or fit certificate.
    pub(super) fn open_binding(&self) -> Option<&OpenBinding> {
        if self.failed { None } else { self.durable_open.as_ref() }
    }

    /// Borrow original causes/row count only. Never expose Connection, a SQL
    /// executor, lock handle, mutable slot, clone or waiter-registering future.
    pub(super) fn failure(&self) -> FailureReadback<'_> {
        #[cfg(windows)]
        if let Some(Err(error)) = self.entropy_result.as_ref() {
            return FailureReadback { primary: Some(FailureRef::Entropy(error)), rollback: self.rollback.as_ref() };
        }
        let primary = if let Some(Err(error)) = self.namespace_lock.as_ref() {
            Some(FailureRef::File(error))
        } else if let Some(Err(error)) = self.lock_result.as_ref() {
            Some(FailureRef::Lock(error))
        } else if let Some(Err(error)) = self.connection.as_ref() {
            Some(FailureRef::Open(error))
        } else if let Some(Err(error)) = self.step.as_ref() {
            Some(FailureRef::Stage(error))
        } else if let Some(Err(error)) = self.schema.as_ref() {
            Some(FailureRef::Stage(error))
        } else if let Some(Err(error)) = self.prior.as_ref() {
            Some(FailureRef::Stage(error))
        } else {
            self.members.as_deref().and_then(member::MemberAttempt::first_failure)
                .map(FailureRef::Stage)
                .or_else(|| self.open_cas.as_ref().filter(|result| !matches!(result, Ok(1)))
                    .map(FailureRef::Cas))
        };
        FailureReadback { primary, rollback: self.rollback.as_ref() }
    }

    fn fail_retained(&mut self) {
        self.failed = true;
        self.rollback_once();
    }

    fn verify_compile_options(&mut self) -> Observation {
        self.perform(|db| {
            let mut statement = db.prepare(sql(Sql::CompileOptions)
                .ok_or(Cause::Refusal(Refusal::Catalog))?).map_err(Cause::Sqlite)?;
            let mut rows = statement.query([]).map_err(Cause::Sqlite)?;
            while let Some(row) = rows.next().map_err(Cause::Sqlite)? {
                let value = row.get_ref(0).map_err(Cause::Sqlite)?;
                let text = value.as_str().map_err(|_| Cause::Refusal(Refusal::Configuration))?;
                let mismatched = text == "OMIT_TWOSIZE_LOOKASIDE"
                    || (text.starts_with("DEFAULT_LOOKASIDE=") && text != "DEFAULT_LOOKASIDE=1200,40")
                    || (text.starts_with("DEFAULT_CACHE_SIZE=") && text != "DEFAULT_CACHE_SIZE=-2000")
                    || (text.starts_with("DEFAULT_PAGE_SIZE=") && text != "DEFAULT_PAGE_SIZE=4096");
                if mismatched { return Err(Cause::Refusal(Refusal::Configuration)); }
            }
            // Source/caller/runner provenance must additionally rule out global
            // runtime changes; this row stream alone is not that proof.
            Ok(())
        })
    }

    /// Module-private closures only. All SQL bodies come from the sealed enum,
    /// not a caller string, callback, extension trait or public registration.
    fn perform(&mut self, body: impl FnOnce(&Connection) -> Result<(), Cause>) -> Observation {
        if self.failed { return Observation::Refused; }
        let result = match self.original.check() {
            Err(error) => Err(Cause::Credit(error)),
            Ok(()) => match self.connection() {
                Some(db) => body(db),
                None => Err(Cause::Refusal(Refusal::Closed)),
            },
        };
        self.step = Some(result); // Whole result BEFORE fixed observation.
        if matches!(self.step.as_ref(), Some(Err(_))) {
            self.failed = true;
            self.rollback_once();
        }
        self.observation()
    }

    fn batch(&mut self, statement: Sql) -> Observation {
        self.perform(|db| {
            db.execute_batch(sql(statement).ok_or(Cause::Refusal(Refusal::Catalog))?)
                .map_err(Cause::Sqlite)
        })
    }

    /// Prepaid failure cleanup: never checks a now-closed ordinary allowance.
    /// Do not use Transaction::Drop (070AF67D lines242-246 discards its result).
    fn rollback_once(&mut self) {
        if self.rollback_attempted { return; }
        let Some(db) = self.connection() else { return };
        if db.is_autocommit() { return; }
        let Some(text) = sql(Sql::Rollback) else { return };
        let result = db.execute_batch(text);
        self.rollback = Some(result); // Raw rollback error remains distinct.
        self.rollback_attempted = true;
    }
}

fn inspect_schema(db: &Connection) -> Result<Schema, Cause> {
    const EXPECTED: [(&str, &str, &str, Option<Sql>); 7] = [
        ("table", "host", "host", Some(Sql::CreateHost)),
        ("table", "member", "member", Some(Sql::CreateMember)),
        ("table", "writer", "writer", Some(Sql::CreateWriter)),
        ("index", "sqlite_autoindex_member_1", "member", None),
        ("index", "sqlite_autoindex_member_2", "member", None),
        ("index", "sqlite_autoindex_writer_1", "writer", None),
        ("index", "sqlite_autoindex_writer_2", "writer", None),
    ];
    let mut seen = [false; 7];
    let mut any = false;
    let mut statement = db.prepare(sql(Sql::Schema).ok_or(Cause::Refusal(Refusal::Catalog))?)
        .map_err(Cause::Sqlite)?;
    let mut rows = statement.query([]).map_err(Cause::Sqlite)?;
    while let Some(row) = rows.next().map_err(Cause::Sqlite)? {
        any = true;
        let kind = row.get_ref(0).map_err(Cause::Sqlite)?;
        let name = row.get_ref(1).map_err(Cause::Sqlite)?;
        let table = row.get_ref(2).map_err(Cause::Sqlite)?;
        let definition = row.get_ref(3).map_err(Cause::Sqlite)?;
        let mut matched = false;
        for (present, (expected_kind, expected_name, expected_table, expected_sql))
            in seen.iter_mut().zip(EXPECTED)
        {
            if kind.as_str().ok() == Some(expected_kind) && name.as_str().ok() == Some(expected_name)
                && table.as_str().ok() == Some(expected_table)
            {
                if *present { return Err(Cause::Refusal(Refusal::Configuration)); }
                let exact = match expected_sql {
                    Some(expected) => sql(expected).is_some_and(|text|
                        definition.as_str().ok() == Some(text.trim_end_matches(';'))),
                    None => matches!(definition, rusqlite::types::ValueRef::Null),
                };
                if !exact { return Err(Cause::Refusal(Refusal::Configuration)); }
                *present = true;
                matched = true;
                break;
            }
        }
        if !matched { return Err(Cause::Refusal(Refusal::Configuration)); }
    }
    if !any { Ok(Schema::Empty) }
    else if seen.into_iter().all(|present| present) { Ok(Schema::Exact) }
    else { Err(Cause::Refusal(Refusal::Configuration)) }
}

fn read_prior_host(db: &Connection) -> Result<Option<PriorHost>, Cause> {
    let mut statement = db.prepare(sql(Sql::ReadHost).ok_or(Cause::Refusal(Refusal::Catalog))?)
        .map_err(Cause::Sqlite)?;
    let mut rows = statement.query([]).map_err(Cause::Sqlite)?;
    let Some(row) = rows.next().map_err(Cause::Sqlite)? else { return Ok(None) };
    let format: i64 = row.get(0).map_err(Cause::Sqlite)?;
    let generation: i64 = row.get(1).map_err(Cause::Sqlite)?;
    let nonce = row.get_ref(2).map_err(Cause::Sqlite)?;
    let phase: i64 = row.get(3).map_err(Cause::Sqlite)?;
    let freeze = row.get_ref(4).map_err(Cause::Sqlite)?;
    let expected: Option<i64> = row.get(5).map_err(Cause::Sqlite)?;
    let valid_phase = match phase {
        0 => matches!(freeze, rusqlite::types::ValueRef::Null) && expected.is_none(),
        1 | 2 => freeze.as_blob().is_ok_and(|bytes| bytes.len() == 16)
            && expected.is_some_and(|count| count >= 0),
        _ => false,
    };
    if format != 1 || generation <= 0 || !nonce.as_blob().is_ok_and(|bytes| bytes.len() == 16)
        || !valid_phase
    { return Err(Cause::Refusal(Refusal::Configuration)); }
    // Do not claim prior CLEAN from phase2 without the complete frozen census.
    let incarnation = nonce.as_blob().map_err(|_| Cause::Refusal(Refusal::Configuration))?
        .try_into().map_err(|_| Cause::Refusal(Refusal::Configuration))?;
    Ok(Some(PriorHost { generation, incarnation }))
}

fn bounded_namespace(database: &Path, lock: &Path) -> bool {
    fn bounded(path: &Path) -> bool {
        path.is_absolute() && path.to_str().is_some_and(|text|
            text.len() <= 4096 && !text.as_bytes().contains(&0))
            && !path.components().any(|part| matches!(part, Component::ParentDir))
    }
    bounded(database) && bounded(lock) && database.parent() == lock.parent()
        && database.file_name().is_some_and(|name| name == "host.sqlite")
        && lock.file_name().is_some_and(|name| name == "host.lock")
        && database.parent().and_then(Path::file_name)
            .is_some_and(|name| name == ".seat-host")
}

#[cfg(test)]
mod controls {
    use super::*;

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn same_original_cannot_substitute_another_prepared_connection_owner() {
        tokio::runtime::Handle::current().spawn(async {
            use crate::operation::Allowance;
            use std::time::Duration;
            use tokio::time::Instant;
            let base = std::env::temp_dir().join(format!("cna-journal-owner-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&base).unwrap();
            let first = base.join("first"); let second = base.join("second");
            std::fs::create_dir(&first).unwrap(); std::fs::create_dir(&second).unwrap();
            let original = Operation::new(String::from("one-meter-two-owners"), 1,
                Allowance::required(Some(Instant::now() + Duration::from_secs(30))).unwrap()).unwrap();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_layout(&original, &first, &[]).unwrap(), super::super::namespace::Observation::Observed);
            assert_eq!(host.prepare_observed_namespace().unwrap(), super::super::namespace::Observation::Observed);
            host.reserve_journal(&original).unwrap();
            assert_eq!(host.open_observed_journal().unwrap(), Observation::Configured);
            let owner = host.journal.as_ref().unwrap().namespace_association.as_ref().unwrap().clone();
            let connection = std::ptr::from_ref(host.journal.as_ref().unwrap().connection.as_ref().unwrap()) as usize;
            assert_eq!(host.observe_existing_namespace_layout(&original, &second, &[]).unwrap(), super::super::namespace::Observation::Observed);
            assert_eq!(host.prepare_observed_namespace().unwrap(), super::super::namespace::Observation::Observed);
            assert_eq!(host.establish_observed_open().unwrap(), Observation::Refused);
            assert!(host.admission_closed && owner.upgrade().is_some());
            let root = host.journal.as_deref_mut().unwrap();
            assert!(root.entropy_result.is_none() && root.schema.is_none() && root.open_cas.is_none());
            assert!(root.rollback.is_none() && root.open_binding().is_none());
            assert_eq!(std::ptr::from_ref(root.connection.as_ref().unwrap()) as usize, connection);
            let refusal = match root.failure().primary {
                Some(FailureRef::Stage(cause @ Cause::Namespace(super::super::namespace::Refusal::Changed))) => std::ptr::from_ref(cause) as usize,
                _ => panic!("missing retained owner mismatch"),
            };
            let charged = original.captured_bytes();
            assert_eq!(root.establish_fresh_open(host.namespace_layouts.as_deref_mut().unwrap()), Observation::Refused);
            assert!(matches!(root.failure().primary, Some(FailureRef::Stage(cause)) if std::ptr::from_ref(cause) as usize == refusal));
            assert_eq!(original.captured_bytes(), charged);
            drop(host); assert!(owner.upgrade().is_none());
            for path in [&first, &second] {
                std::fs::remove_file(path.join(".seat-host/host.sqlite")).unwrap();
                std::fs::remove_file(path.join(".seat-host/host.lock")).unwrap();
                std::fs::remove_dir(path.join(".seat-host")).unwrap(); std::fs::remove_dir(path).unwrap();
            }
            std::fs::remove_dir(base).unwrap();
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn configured_connection_owner_without_canonical_roots_cannot_start_entropy_or_open() {
        tokio::runtime::Handle::current().spawn(async {
            use crate::operation::Allowance;
            use std::time::Duration;
            use tokio::time::Instant;
            let base = std::env::temp_dir().join(format!("cna-journal-domain-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&base).unwrap();
            let original = Operation::new(String::from("configured-is-not-admitted"), 1,
                Allowance::required(Some(Instant::now() + Duration::from_secs(30))).unwrap()).unwrap();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_layout(&original, &base, &[]).unwrap(), super::super::namespace::Observation::Observed);
            assert_eq!(host.prepare_observed_namespace().unwrap(), super::super::namespace::Observation::Observed);
            host.reserve_journal(&original).unwrap();
            assert_eq!(host.open_observed_journal().unwrap(), Observation::Configured);
            let charged = original.captured_bytes();
            assert_eq!(host.establish_observed_open().unwrap(), Observation::Refused);
            let root = host.journal.as_deref().unwrap();
            assert!(root.connection.as_ref().unwrap().is_ok());
            assert!(root.entropy_result.is_none() && root.schema.is_none() && root.rollback.is_none());
            assert!(root.open_cas.is_none() && root.open_binding().is_none());
            assert!(matches!(root.failure().primary, Some(FailureRef::Stage(Cause::Namespace(super::super::namespace::Refusal::Closed)))));
            assert_eq!(original.captured_bytes(), charged);
            drop(host);
            std::fs::remove_file(base.join(".seat-host/host.sqlite")).unwrap();
            std::fs::remove_file(base.join(".seat-host/host.lock")).unwrap();
            std::fs::remove_dir(base.join(".seat-host")).unwrap(); std::fs::remove_dir(base).unwrap();
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn namespace_owner_witness_is_prepaid_once_and_weak_does_not_keep_guards_alive() {
        tokio::runtime::Handle::current().spawn(async {
            use crate::operation::Allowance;
            use std::time::Duration;
            use tokio::time::Instant;
            let base = std::env::temp_dir().join(format!("cna-journal-witness-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&base).unwrap();
            let original = Operation::new(String::from("normal-owner-arc"), 1,
                Allowance::required(Some(Instant::now() + Duration::from_secs(30))).unwrap()).unwrap();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_layout(&original, &base, &[]).unwrap(), super::super::namespace::Observation::Observed);
            assert_eq!(host.prepare_observed_namespace().unwrap(), super::super::namespace::Observation::Observed);
            let before = original.captured_bytes();
            let inputs = host.namespace_layouts.as_deref_mut().unwrap().journal_inputs(&original).unwrap();
            let owner = Arc::downgrade(inputs.association);
            let charged = original.captured_bytes();
            assert_eq!(charged - before, crate::driver::owned_driver_task::own_arc_layout::<super::super::namespace::JournalAssociation>().unwrap().size());
            let again = host.namespace_layouts.as_deref_mut().unwrap().journal_inputs(&original).unwrap();
            assert!(Arc::ptr_eq(&owner.upgrade().unwrap(), again.association));
            assert_eq!(original.captured_bytes(), charged);
            drop(host.namespace_layouts.take());
            assert!(owner.upgrade().is_none());
            assert_eq!(original.captured_bytes(), charged);
            std::fs::remove_file(base.join(".seat-host/host.sqlite")).unwrap();
            std::fs::remove_file(base.join(".seat-host/host.lock")).unwrap();
            std::fs::remove_dir(base.join(".seat-host")).unwrap(); std::fs::remove_dir(base).unwrap();
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fresh_nonce_refuses_unprepared_namespace_before_rng_and_preserves_first_failure() {
        tokio::runtime::Handle::current().spawn(async {
            use crate::operation::Allowance;
            use std::time::Duration;
            use tokio::time::Instant;
            let original = Operation::new(String::from("nonce-unprepared"), 1,
                Allowance::required(Some(Instant::now() + Duration::from_secs(30))).unwrap()).unwrap();
            let mut root = Box::new(JournalRoot::reserve(&original).unwrap());
            let mut layout = Box::new(super::super::namespace::LayoutRoot::reserve(&original).unwrap());
            root.configured = true; // State fixture only; NO connection/SQL/RNG.
            let charged = original.captured_bytes();
            assert_eq!(root.establish_fresh_open(&mut layout), Observation::Refused);
            assert!(root.entropy_result.is_none()); assert_eq!(root.nonce, [0; 16]);
            let pointer = match root.failure().primary {
                Some(FailureRef::Stage(cause @ Cause::Namespace(_))) => std::ptr::from_ref(cause) as usize,
                _ => panic!("missing retained namespace refusal"),
            };
            assert_eq!(root.establish_fresh_open(&mut layout), Observation::Refused);
            assert!(matches!(root.failure().primary, Some(FailureRef::Stage(cause))
                if std::ptr::from_ref(cause) as usize == pointer));
            assert!(root.connection.is_none() && root.open_binding().is_none());
            assert!(root.entropy_result.is_none()); assert_eq!(original.captured_bytes(), charged);
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retained_entropy_error_and_partial_bytes_cannot_be_replaced_by_retry() {
        tokio::runtime::Handle::current().spawn(async {
            use crate::operation::Allowance;
            use std::time::Duration;
            use tokio::time::Instant;
            let original = Operation::new(String::from("nonce-failure"), 1,
                Allowance::required(Some(Instant::now() + Duration::from_secs(30))).unwrap()).unwrap();
            let mut root = Box::new(JournalRoot::reserve(&original).unwrap());
            let mut layout = Box::new(super::super::namespace::LayoutRoot::reserve(&original).unwrap());
            // Explicit deterministic failure fixture, never an RNG hook or
            // claimed OS failure injection. These are canonical root fields.
            root.nonce = [17; 16]; root.entropy_result = Some(Err(getrandom::Error::UNEXPECTED));
            assert_eq!(root.refuse(Refusal::Entropy), Observation::Refused);
            let pointer = match root.failure().primary {
                Some(FailureRef::Entropy(error)) => std::ptr::from_ref(error) as usize,
                _ => panic!("missing retained entropy error"),
            };
            let charged = original.captured_bytes();
            assert_eq!(root.establish_fresh_open(&mut layout), Observation::Refused);
            assert!(matches!(root.failure().primary, Some(FailureRef::Entropy(error))
                if std::ptr::from_ref(error) as usize == pointer && *error == getrandom::Error::UNEXPECTED));
            assert_eq!(root.nonce, [17; 16]);
            assert!(root.connection.is_none() && root.rollback.is_none() && root.open_binding().is_none());
            assert_eq!(original.captured_bytes(), charged);
        }).await.unwrap();
    }

    #[test]
    fn fixed_sql_catalog_retains_approved_count_and_transaction_literals() {
        let statements: Vec<_> = CATALOG.lines()
            .filter(|line| !line.is_empty() && !line.starts_with("--")).collect();
        assert_eq!(statements.len(), STATEMENT_COUNT);
        assert_eq!(statements.iter().map(|line| line.len()).max(), Some(505));
        assert_eq!(sql(Sql::Begin), Some("BEGIN IMMEDIATE;"));
        assert_eq!(sql(Sql::Commit), Some("COMMIT;"));
        assert_eq!(sql(Sql::Rollback), Some("ROLLBACK;"));
        assert_eq!(J_BYTES, 8_388_608);
        assert!(statements.iter().all(|line| line.ends_with(';')));
    }

    #[test]
    fn lexical_namespace_checks_do_not_turn_into_physical_identity_proof() {
        let parent = std::env::temp_dir().join(".seat-host");
        let database = parent.join("host.sqlite");
        let lock = parent.join("host.lock");
        assert!(bounded_namespace(&database, &lock));
        assert!(!bounded_namespace(Path::new("host.sqlite"), &lock));
        assert!(!bounded_namespace(&parent.join("other.sqlite"), &lock));
        assert!(!bounded_namespace(&database, &parent.join("other.lock")));
        assert!(!bounded_namespace(&parent.join("../host.sqlite"), &lock));
    }

    #[test]
    fn bounds_refusal_keeps_real_prepaid_root_and_first_cause_without_io_or_refund() {
        use crate::operation::Allowance;
        use std::time::Duration;
        use tokio::time::Instant;

        let original = Operation::new(String::from("journal-source-refusal"), 1,
            Allowance::required(Some(Instant::now() + Duration::from_secs(30))).unwrap()).unwrap();
        let before = original.captured_bytes();
        let mut root = Box::new(JournalRoot::reserve(&original).unwrap());
        let charged = original.captured_bytes();
        assert_eq!(charged - before, Layout::new::<JournalRoot>().size() + J_BYTES);
        assert_eq!(root.open_once(Path::new("host.sqlite"), Path::new("host.lock"), true),
            Observation::Refused);
        let pointer = match root.failure().primary {
            Some(FailureRef::Stage(cause @ Cause::Refusal(Refusal::Bounds))) =>
                std::ptr::from_ref(cause) as usize,
            _ => panic!("missing original bounds refusal"),
        };
        assert!(root.namespace_lock.is_none());
        assert!(root.connection.is_none());
        assert!(root.rollback.is_none());
        assert_eq!(root.establish_open([1; 16]), Observation::Refused);
        assert_eq!(root.open_once(Path::new("host.sqlite"), Path::new("host.lock"), true),
            Observation::Refused);
        assert!(matches!(root.failure().primary, Some(FailureRef::Stage(cause))
            if std::ptr::from_ref(cause) as usize == pointer));
        assert!(root.open_binding().is_none());
        assert_eq!(original.captured_bytes(), charged);
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn guarded_journal_refuses_unprepared_or_different_original_before_connection_or_sql() {
        tokio::runtime::Handle::current().spawn(async {
            use crate::operation::Allowance;
            use std::time::Duration;
            use tokio::time::Instant;
            for prepared in [false, true] {
                let base = std::env::temp_dir().join(format!("cna-guarded-journal-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir(&base).unwrap();
                let original = Operation::new(String::from("same-textual-id"), 1,
                    Allowance::required(Some(Instant::now() + Duration::from_secs(30))).unwrap()).unwrap();
                let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
                assert_eq!(host.observe_existing_namespace_layout(&original, &base, &[]).unwrap(), super::super::namespace::Observation::Observed);
                if prepared { assert_eq!(host.prepare_observed_namespace().unwrap(), super::super::namespace::Observation::Observed); }
                let journal_original = if prepared {
                    Operation::new(String::from("same-textual-id"), 1,
                        Allowance::required(Some(Instant::now() + Duration::from_secs(30))).unwrap()).unwrap()
                } else { original.clone() };
                host.reserve_journal(&journal_original).unwrap();
                let charged = journal_original.captured_bytes();
                assert_eq!(host.open_observed_journal().unwrap(), Observation::Refused);
                assert!(host.admission_closed);
                let root = host.journal.as_deref_mut().unwrap();
                assert!(root.connection.is_none()); assert!(root.namespace_lock.is_none());
                assert!(root.lock_result.is_none()); assert!(root.rollback.is_none());
                let cause = match root.failure().primary {
                    Some(FailureRef::Stage(cause @ Cause::Namespace(super::super::namespace::Refusal::Closed))) => std::ptr::from_ref(cause) as usize,
                    _ => panic!("missing original namespace refusal"),
                };
                assert_eq!(root.open_guarded_once(host.namespace_layouts.as_deref_mut().unwrap()), Observation::Refused);
                assert!(matches!(root.failure().primary, Some(FailureRef::Stage(value)) if std::ptr::from_ref(value) as usize == cause));
                assert_eq!(journal_original.captured_bytes(), charged);
                drop(host); assert_eq!(journal_original.captured_bytes(), charged);
                if prepared {
                    std::fs::remove_file(base.join(".seat-host/host.sqlite")).unwrap();
                    std::fs::remove_file(base.join(".seat-host/host.lock")).unwrap();
                    std::fs::remove_dir(base.join(".seat-host")).unwrap();
                } else { assert!(!base.join(".seat-host").exists()); }
                std::fs::remove_dir(base).unwrap();
            }
        }).await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn guarded_journal_rechecks_actual_leaf_link_count_before_any_sqlite_open() {
        tokio::runtime::Handle::current().spawn(async {
            use crate::operation::Allowance;
            use std::time::Duration;
            use tokio::time::Instant;
            let base = std::env::temp_dir().join(format!("cna-journal-link-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&base).unwrap();
            let original = Operation::new(String::from("journal-link-source"), 1,
                Allowance::required(Some(Instant::now() + Duration::from_secs(30))).unwrap()).unwrap();
            let mut host = crate::seat_host::SeatHost::new(tokio::runtime::Handle::current());
            assert_eq!(host.observe_existing_namespace_layout(&original, &base, &[]).unwrap(), super::super::namespace::Observation::Observed);
            assert_eq!(host.prepare_observed_namespace().unwrap(), super::super::namespace::Observation::Observed);
            let database = base.join(".seat-host/host.sqlite"); let alias = base.join("alias");
            std::fs::hard_link(&database, &alias).unwrap(); // Failure is not skipped.
            host.reserve_journal(&original).unwrap();
            let charged = original.captured_bytes();
            assert_eq!(host.open_observed_journal().unwrap(), Observation::Refused);
            let root = host.journal.as_deref_mut().unwrap();
            assert!(root.connection.is_none()); assert!(root.rollback.is_none());
            assert!(matches!(root.failure().primary, Some(FailureRef::Stage(Cause::Namespace(super::super::namespace::Refusal::Link)))));
            assert!(root.open_binding().is_none()); assert_eq!(original.captured_bytes(), charged);
            drop(host); std::fs::remove_file(alias).unwrap();
            std::fs::remove_file(database).unwrap(); std::fs::remove_file(base.join(".seat-host/host.lock")).unwrap();
            std::fs::remove_dir(base.join(".seat-host")).unwrap(); std::fs::remove_dir(base).unwrap();
        }).await.unwrap();
    }
}
