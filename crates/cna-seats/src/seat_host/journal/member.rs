//! Private bounded member transaction behind Host's guarded pairing path.
//! Complete fit/production lease issuance remain closed; raw fixture controls
//! below are not physical Host association or durability evidence.

use super::*;

struct Text<const N: usize> { bytes: [u8; N], len: usize }

impl<const N: usize> Text<N> {
    fn copy(value: &str) -> Result<Self, Refusal> {
        if value.len() > N { return Err(Refusal::Bounds); }
        let mut retained = Self { bytes: [0; N], len: value.len() };
        retained.bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(retained)
    }

    fn text(&self) -> Result<&str, Refusal> {
        let bytes = self.bytes.get(..self.len).ok_or(Refusal::Bounds)?;
        std::str::from_utf8(bytes).map_err(|_| Refusal::Bounds)
    }
}

// All bytes are inline in the charged MemberAttempt, never uncharged clones or
// caller-authorized path identity. Host pairing supplies validated ownership
// plus actual borrowed row facts before the private bounded constructor.
struct MemberRecord {
    binding: OpenBinding,
    scope: i64,
    campaign: Text<256>,
    storage: Text<4096>,
    rules_profile: Text<1024>,
    content_hash: Text<1024>,
    engine_version: Text<1024>,
}

enum Presence { Absent, Exact, Different }

pub(super) struct MemberAttempt {
    record: Option<MemberRecord>,
    attempted: bool,
    lookup: Option<Result<Presence, Cause>>,
    insert: Option<Result<usize, Cause>>,
    commit: Option<Result<(), Cause>>,
    committed: bool,
    next: Option<Box<Self>>,
}

impl Drop for MemberAttempt {
    fn drop(&mut self) {
        // Terminal deallocation only: no SQL, rollback, join or credit refund.
        // Detach each link so the number of retained rows cannot recurse Drop.
        let mut next = self.next.take();
        while let Some(mut entry) = next { next = entry.next.take(); }
    }
}

impl MemberAttempt {
    pub(super) fn first_failure(&self) -> Option<&Cause> {
        let mut next = Some(self);
        while let Some(entry) = next {
            if let Some(cause) = entry.lookup.as_ref().and_then(|value| value.as_ref().err())
                .or_else(|| entry.insert.as_ref().and_then(|value| value.as_ref().err()))
                .or_else(|| entry.commit.as_ref().and_then(|value| value.as_ref().err()))
            { return Some(cause); }
            next = entry.next.as_deref();
        }
        None
    }
}

impl JournalRoot {
    /// Called only by Host's checked pairing/registration path. Primitive
    /// borrowed row facts are supporting inputs, never authority by themselves.
    pub(in crate::seat_host) fn admit_paired_member(&mut self, scope: i64,
        campaign: [&str; 2], storage: &str, pins: [&str; 3]) -> Observation
    {
        if self.failed { return Observation::Refused; }
        if campaign[1] != pins[0] || scope <= 0 { return self.refuse(Refusal::MemberMismatch); }
        if self.reserve_member_attempt().is_err() { return self.refuse(Refusal::Closed); }
        let Some(binding) = self.open_binding().copied() else { return self.refuse(Refusal::Closed) };
        // All input storage is inline in the already charged original attempt.
        // Any bounds refusal leaves that attempt plus original caller rows held.
        let record = match (Text::copy(campaign[0]), Text::copy(storage), Text::copy(pins[0]),
            Text::copy(pins[1]), Text::copy(pins[2]))
        {
            (Ok(campaign), Ok(storage), Ok(rules_profile), Ok(content_hash), Ok(engine_version)) =>
                MemberRecord { binding, scope, campaign, storage, rules_profile, content_hash, engine_version },
            _ => return self.refuse(Refusal::Bounds),
        };
        let Some(attempt) = self.members.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        attempt.record = Some(record);
        self.commit_member()
    }

    /// Installed before copying any inline input or starting the transaction.
    /// Public callers cannot construct a record or access this reservation.
    fn reserve_member_attempt(&mut self) -> Result<(), OperationError> {
        if self.failed || self.open_binding().is_none()
            || self.members.as_ref().is_some_and(|entry| !entry.committed)
        { return Err(OperationError::CreditOwned); }
        if let Err(error) = prepay(&self.original, Layout::new::<MemberAttempt>()) {
            self.step = Some(Err(Cause::Credit(error))); self.fail_retained(); return Err(error);
        }
        let next = self.members.take();
        self.members = Some(Box::new(MemberAttempt { record: None, attempted: false,
            lookup: None, insert: None, commit: None, committed: false, next }));
        Ok(())
    }

    fn commit_member(&mut self) -> Observation {
        if self.failed { return Observation::Refused; }
        let Some(open) = self.open_binding().copied() else { return self.refuse(Refusal::Closed) };
        let Some(attempt) = self.members.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        if attempt.attempted { return Observation::Refused; }
        attempt.attempted = true;
        let Some(record) = attempt.record.as_ref() else { return self.refuse(Refusal::Closed) };
        if record.binding.generation != open.generation || record.binding.incarnation != open.incarnation
            || record.scope <= 0 || record.storage.text().is_err()
            || record.storage.text().is_ok_and(|text| text.as_bytes().contains(&0))
            || record.campaign.text().is_err() || record.rules_profile.text().is_err()
            || record.content_hash.text().is_err() || record.engine_version.text().is_err()
        { return self.refuse(Refusal::Bounds); }
        if self.batch(Sql::Begin) == Observation::Refused { return Observation::Refused; }
        if self.perform(|_| Ok(())) == Observation::Refused { return Observation::Refused; }
        let lookup = match (self.connection(), self.members.as_deref().and_then(|entry| entry.record.as_ref())) {
            (Some(db), Some(record)) => lookup_member(db, record),
            _ => Err(Cause::Refusal(Refusal::Closed)),
        };
        let Some(attempt) = self.members.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        attempt.lookup = Some(lookup); // Whole library result before observation.
        match attempt.lookup.as_ref() {
            Some(Ok(Presence::Absent)) => (),
            Some(Ok(Presence::Exact)) => return self.refuse(Refusal::DuplicateMember),
            Some(Ok(Presence::Different)) => return self.refuse(Refusal::MemberMismatch),
            _ => { self.fail_retained(); return Observation::Refused; }
        }
        if self.perform(|_| Ok(())) == Observation::Refused { return Observation::Refused; }
        let insert = match (self.connection(), self.members.as_deref().and_then(|entry| entry.record.as_ref())) {
            (Some(db), Some(record)) => insert_member(db, record),
            _ => Err(Cause::Refusal(Refusal::Closed)),
        };
        let Some(attempt) = self.members.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        attempt.insert = Some(insert);
        match attempt.insert.as_ref() {
            Some(Ok(1)) => (),
            Some(Ok(_)) => return self.refuse(Refusal::MemberMismatch),
            _ => { self.fail_retained(); return Observation::Refused; }
        }
        if self.perform(|_| Ok(())) == Observation::Refused { return Observation::Refused; }
        let commit = match self.connection() {
            Some(db) => match sql(Sql::Commit) {
                Some(text) => db.execute_batch(text).map_err(Cause::Sqlite),
                None => Err(Cause::Refusal(Refusal::Catalog)),
            },
            None => Err(Cause::Refusal(Refusal::Closed)),
        };
        let Some(attempt) = self.members.as_deref_mut() else { return self.refuse(Refusal::Closed) };
        attempt.commit = Some(commit);
        if !matches!(attempt.commit.as_ref(), Some(Ok(()))) {
            self.fail_retained(); return Observation::Refused;
        }
        attempt.committed = true; // Only acknowledged FULL commit, never insert count alone.
        self.observation()
    }
}

fn lookup_member(db: &Connection, record: &MemberRecord) -> Result<Presence, Cause> {
    let storage = record.storage.text().map_err(Cause::Refusal)?;
    let mut statement = db.prepare_cached(sql(Sql::ReadMember).ok_or(Cause::Refusal(Refusal::Catalog))?)
        .map_err(Cause::Sqlite)?;
    let mut rows = statement.query(rusqlite::params![record.binding.generation, storage]).map_err(Cause::Sqlite)?;
    let Some(row) = rows.next().map_err(Cause::Sqlite)? else { return Ok(Presence::Absent) };
    let scope: i64 = row.get(0).map_err(Cause::Sqlite)?;
    let mut exact = scope == record.scope;
    for (index, expected, bound) in [
        (1usize, record.campaign.text().map_err(Cause::Refusal)?, 256usize),
        (2, storage, 4096),
        (3, record.rules_profile.text().map_err(Cause::Refusal)?, 1024),
        (4, record.content_hash.text().map_err(Cause::Refusal)?, 1024),
        (5, record.engine_version.text().map_err(Cause::Refusal)?, 1024),
    ] {
        let field = row.get_ref(index).map_err(Cause::Sqlite)?;
        let value = field.as_str().map_err(|_| Cause::Refusal(Refusal::Configuration))?;
        if value.len() > bound { return Err(Cause::Refusal(Refusal::Bounds)); }
        exact &= value == expected;
    }
    if scope <= 0 || rows.next().map_err(Cause::Sqlite)?.is_some() { return Err(Cause::Refusal(Refusal::Configuration)); }
    Ok(if exact { Presence::Exact } else { Presence::Different })
}

fn insert_member(db: &Connection, record: &MemberRecord) -> Result<usize, Cause> {
    db.execute(sql(Sql::InsertMember).ok_or(Cause::Refusal(Refusal::Catalog))?, rusqlite::params![
        record.binding.generation, record.scope, record.campaign.text().map_err(Cause::Refusal)?,
        record.storage.text().map_err(Cause::Refusal)?, record.rules_profile.text().map_err(Cause::Refusal)?,
        record.content_hash.text().map_err(Cause::Refusal)?, record.engine_version.text().map_err(Cause::Refusal)?,
        &record.binding.incarnation[..],
    ]).map_err(Cause::Sqlite)
}

#[cfg(test)]
mod controls {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn paired_input_profile_or_bound_refusal_keeps_original_failure_and_never_starts_member_transaction() {
        tokio::runtime::Handle::current().spawn(async {
            for bounds in [false, true] {
                let (_original, mut root) = fixture_journal();
                let long = "x".repeat(257);
                let campaign = if bounds { [long.as_str(), "profile"] } else { ["campaign", "different-profile"] };
                assert_eq!(root.admit_paired_member(1, campaign, "C:/fixture.sqlite", ["profile", "content", "engine"]), Observation::Refused);
                assert!(root.failed && root.members.as_ref().is_none_or(|attempt| !attempt.attempted && attempt.record.is_none()));
                let pointer = std::ptr::from_ref(root.step.as_ref().unwrap()) as usize;
                assert_eq!(root.admit_paired_member(1, ["campaign", "profile"], "C:/fixture.sqlite", ["profile", "content", "engine"]), Observation::Refused);
                assert_eq!(std::ptr::from_ref(root.step.as_ref().unwrap()) as usize, pointer);
                assert!(root.members.as_ref().is_none_or(|attempt| !attempt.attempted && attempt.lookup.is_none()
                    && attempt.insert.is_none() && attempt.commit.is_none()));
                // In-memory configured journal fixture only. No physical Host,
                // actual Server rows/joins or FULL durability/fit proof.
            }
        }).await.unwrap();
    }

    fn fixture_journal() -> (Operation, JournalRoot) {
        use crate::operation::Allowance;
        let original = Operation::new(String::from("member-transaction-fixture"), 1,
            Allowance::required(Some(tokio::time::Instant::now() + std::time::Duration::from_secs(30))).unwrap()).unwrap();
        let mut root = JournalRoot::reserve(&original).unwrap();
        // Explicit in-memory connection/configuration fixture. This is NOT
        // guarded production open, FULL durability, namespace or fit proof.
        root.connection = Some(Ok(Connection::open_in_memory().unwrap()));
        root.configured = true;
        assert_eq!(root.establish_open([1; 16]), Observation::Configured);
        (original, root)
    }

    fn fixture_record(root: &mut JournalRoot, scope: i64, campaign: &str, storage: &str, content: &str) {
        root.reserve_member_attempt().unwrap();
        let binding = *root.open_binding().unwrap();
        let record = MemberRecord { binding, scope,
            campaign: Text::copy(campaign).unwrap(), storage: Text::copy(storage).unwrap(),
            rules_profile: Text::copy("profile").unwrap(), content_hash: Text::copy(content).unwrap(),
            engine_version: Text::copy("engine").unwrap() };
        root.members.as_deref_mut().unwrap().record = Some(record);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn member_commit_keeps_inline_record_and_refuses_exact_or_changed_duplicate_without_replacement() {
        tokio::runtime::Handle::current().spawn(async {
            for (content, expected) in [("content", Refusal::DuplicateMember), ("changed", Refusal::MemberMismatch)] {
                let (original, mut root) = fixture_journal();
                let before = original.captured_bytes();
                fixture_record(&mut root, 1, "campaign", "fixture-storage", "content");
                assert_eq!(original.captured_bytes() - before, Layout::new::<MemberAttempt>().size());
                assert_eq!(root.commit_member(), Observation::Configured);
                let first = root.members.as_deref().unwrap();
                assert!(first.committed && matches!(first.insert.as_ref(), Some(Ok(1))) && matches!(first.commit.as_ref(), Some(Ok(()))));
                let pointer = std::ptr::from_ref(first) as usize;
                fixture_record(&mut root, 1, "campaign", "fixture-storage", content);
                assert_eq!(root.commit_member(), Observation::Refused);
                let first = root.members.as_deref().unwrap().next.as_deref().unwrap();
                assert_eq!(std::ptr::from_ref(first) as usize, pointer);
                assert!(first.committed); assert_eq!(first.record.as_ref().unwrap().content_hash.text().unwrap(), "content");
                let current = root.members.as_deref().unwrap();
                assert!(!current.committed && current.insert.is_none() && current.commit.is_none());
                assert!(matches!(root.failure().primary, Some(FailureRef::Stage(Cause::Refusal(reason))) if *reason == expected));
                assert!(matches!(root.rollback.as_ref(), Some(Ok(()))));
                let retained = root.failure().primary.and_then(|failure| match failure { FailureRef::Stage(cause) => Some(std::ptr::from_ref(cause) as usize), _ => None }).unwrap();
                let charged = original.captured_bytes();
                assert_eq!(root.commit_member(), Observation::Refused);
                assert!(root.reserve_member_attempt().is_err());
                assert_eq!(original.captured_bytes(), charged);
                assert!(matches!(root.failure().primary, Some(FailureRef::Stage(cause)) if std::ptr::from_ref(cause) as usize == retained));
            }
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn member_campaign_collision_retains_original_insert_error_and_one_rollback_without_retry_sql() {
        tokio::runtime::Handle::current().spawn(async {
            let (original, mut root) = fixture_journal();
            fixture_record(&mut root, 1, "campaign", "first-storage", "content");
            assert_eq!(root.commit_member(), Observation::Configured);
            fixture_record(&mut root, 1, "campaign", "second-storage", "content");
            assert_eq!(root.commit_member(), Observation::Refused);
            let current = root.members.as_deref().unwrap();
            assert!(matches!(current.lookup.as_ref(), Some(Ok(Presence::Absent))));
            assert!(matches!(current.insert.as_ref(), Some(Err(Cause::Sqlite(_)))));
            assert!(current.commit.is_none() && !current.committed);
            assert!(current.next.as_deref().unwrap().committed);
            let pointer = std::ptr::from_ref(current.first_failure().unwrap()) as usize;
            assert!(matches!(root.failure().primary, Some(FailureRef::Stage(cause)) if std::ptr::from_ref(cause) as usize == pointer));
            assert!(root.rollback_attempted && matches!(root.rollback.as_ref(), Some(Ok(()))));
            let rollback = std::ptr::from_ref(root.rollback.as_ref().unwrap()) as usize;
            let charged = original.captured_bytes();
            assert_eq!(root.commit_member(), Observation::Refused);
            assert!(root.reserve_member_attempt().is_err());
            assert_eq!(original.captured_bytes(), charged);
            assert!(matches!(root.failure().primary, Some(FailureRef::Stage(cause)) if std::ptr::from_ref(cause) as usize == pointer));
            assert_eq!(std::ptr::from_ref(root.rollback.as_ref().unwrap()) as usize, rollback);
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn member_invalid_scope_or_binding_refuses_before_begin_and_text_bounds_never_truncate() {
        tokio::runtime::Handle::current().spawn(async {
            assert!(matches!(Text::<1>::copy("é"), Err(Refusal::Bounds)));
            assert_eq!(Text::<2>::copy("é").unwrap().text().unwrap(), "é");
            for wrong_binding in [false, true] {
                let (_, mut root) = fixture_journal();
                fixture_record(&mut root, if wrong_binding { 1 } else { 0 }, "campaign", "fixture-storage", "content");
                if wrong_binding { root.members.as_deref_mut().unwrap().record.as_mut().unwrap().binding.generation += 1; }
                assert_eq!(root.commit_member(), Observation::Refused);
                let current = root.members.as_deref().unwrap();
                assert!(current.lookup.is_none() && current.insert.is_none() && current.commit.is_none() && !current.committed);
                assert!(root.rollback.is_none());
                assert!(matches!(root.failure().primary, Some(FailureRef::Stage(Cause::Refusal(Refusal::Bounds)))));
                assert_eq!(root.commit_member(), Observation::Refused);
            }
        }).await.unwrap();
    }
}
