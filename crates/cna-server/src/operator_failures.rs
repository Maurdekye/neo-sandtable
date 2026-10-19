//! Host-local, operator-private failure retention. Never exposed by the network API.
use std::{fmt, path::Path};

use cna_core::diagnostic::{MAX_PRIVATE_PAYLOAD_BYTES, PrivatePayload};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

use crate::Error;

pub const RECORD_VERSION: u32 = 1;
pub const RETAINED_RECORDS: usize = 32;

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| matches!(byte,b'0'..=b'9' | b'a'..=b'f'))
}

/// A private record has constant ordinary formatting, including corrupt/unavailable records.
pub struct OperatorFailure {
    pub committed_revision: u64,
    pub state_hash: String,
    pub availability: Availability,
    payload: Option<PrivatePayload>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Available,
    Missing,
    Legacy,
    Corrupt,
    Mismatched,
}
impl OperatorFailure {
    pub fn private_payload(&self) -> Option<&PrivatePayload> {
        self.payload.as_ref()
    }
}
impl fmt::Debug for OperatorFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OperatorFailure([redacted])")
    }
}
impl fmt::Display for OperatorFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OperatorFailure([redacted])")
    }
}

pub(crate) fn store(
    tx: &Transaction<'_>,
    campaign: &str,
    revision: u64,
    state_hash: &str,
    payload: &PrivatePayload,
) -> Result<(), Error> {
    let digest = format!("{:x}", Sha256::digest(payload.as_bytes()));
    if tx.execute(
        "INSERT INTO operator_failures(campaign, committed_revision, state_hash, version, payload, payload_sha256) VALUES (?, ?, ?, ?, ?, ?)",
        params![campaign, revision, state_hash, RECORD_VERSION, payload.as_bytes(), digest],
    )? != 1 {
        // A successful statement with zero affected rows is not evidence that
        // capture was retained. Roll back the combined stop before its ONE
        // stop-only fallback, just as for an explicit insert/storage error.
        return Err(Error::Recovery("operator failure record unavailable".into()));
    }
    tx.execute(
        "DELETE FROM operator_failures WHERE campaign=? AND id NOT IN (SELECT id FROM operator_failures WHERE campaign=? ORDER BY id DESC LIMIT ?)",
        params![campaign, campaign, RETAINED_RECORDS as u32],
    )?;
    let over_retention: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM operator_failures WHERE campaign=? ORDER BY id DESC LIMIT 1 OFFSET ?)",
        params![campaign, RETAINED_RECORDS as u32], |row| row.get(0),
    )?;
    if over_retention {
        // An ignored prune cannot silently grow private retention. This fixed
        // storage failure rolls back the combined stop, including the new row;
        // the caller still attempts exactly ONE generic stop-only transaction.
        return Err(Error::Recovery("operator failure record unavailable".into()));
    }
    Ok(())
}

/// Read an existing operator-owned database without migration, writes or evaluation.
/// Rows and their correlation are checked in one read transaction. Payloads never
/// enter Error/Debug/Display, even when integrity checks fail.
pub fn read_local(path: &Path) -> Result<Vec<OperatorFailure>, Error> {
    read_local_checked(path,None)
}

/// The CLI must bind the selected filename to the database's campaign identity.
/// A renamed/copied database is not evidence for another requested campaign.
pub fn read_campaign(path: &Path, campaign: &str) -> Result<Vec<OperatorFailure>, Error> {
    read_local_checked(path,Some(campaign))
}

fn read_local_checked(path: &Path, expected_campaign: Option<&str>) -> Result<Vec<OperatorFailure>, Error> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    let tx = db.unchecked_transaction()?;
    let (campaign, current_revision, current_hash): (String, u64, String) = tx.query_row(
        "SELECT json_extract(meta, '$.id'), revision, state_hash FROM campaign WHERE id=1 AND length(CAST(meta AS BLOB))<=65536 AND length(CAST(state_hash AS BLOB))=64 AND length(CAST(json_extract(meta, '$.id') AS BLOB))<=128",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if campaign.is_empty() || !valid_hash(&current_hash)
        || expected_campaign.is_some_and(|expected|expected != campaign)
    {
        return Err(Error::Recovery("operator failure records unavailable".into()));
    }
    let present: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='operator_failures')",
        [], |r| r.get(0),
    )?;
    if !present {
        return Ok(vec![OperatorFailure {
            committed_revision: current_revision, state_hash: current_hash,
            availability: Availability::Legacy, payload: None,
        }]);
    }
    let mut stmt = tx.prepare(
        "SELECT CASE WHEN typeof(campaign)='text' AND length(CAST(campaign AS BLOB)) BETWEEN 1 AND 128 THEN campaign ELSE '' END, CASE WHEN typeof(committed_revision)='integer' AND committed_revision>=0 THEN committed_revision ELSE 0 END, CASE WHEN typeof(state_hash)='text' AND length(CAST(state_hash AS BLOB))=64 THEN state_hash ELSE '' END, CASE WHEN typeof(version)='integer' AND version BETWEEN 0 AND 4294967295 THEN version ELSE 0 END, length(CAST(payload AS BLOB)), CASE WHEN typeof(payload_sha256)='text' AND length(CAST(payload_sha256 AS BLOB))=64 THEN payload_sha256 ELSE '' END, CASE WHEN typeof(payload)='blob' AND length(CAST(payload AS BLOB)) BETWEEN 1 AND 32768 THEN payload ELSE NULL END, typeof(campaign)='text' AND typeof(committed_revision)='integer' AND committed_revision>=0 AND typeof(state_hash)='text' AND typeof(version)='integer' AND version BETWEEN 0 AND 4294967295 AND typeof(payload)='blob' AND typeof(payload_sha256)='text' FROM operator_failures ORDER BY id DESC LIMIT 32",
    )?;
    let rows = stmt.query_map([], |r| Ok((
        r.get::<_, String>(0)?, r.get::<_, u64>(1)?, r.get::<_, String>(2)?,
        r.get::<_, u32>(3)?, r.get::<_, u64>(4)?, r.get::<_, String>(5)?,
        r.get::<_, Option<Vec<u8>>>(6)?, r.get::<_,bool>(7)?,
    )))?;
    let mut records = Vec::new();
    for row in rows {
        let (id, revision, state_hash, version, bytes, digest, encoded, valid_types) = row?;
        let expected_hash: Option<String> = if revision == current_revision {
            Some(current_hash.clone())
        } else if revision == 0 {
            tx.query_row("SELECT CASE WHEN typeof(state_hash)='text' AND length(CAST(state_hash AS BLOB))=64 THEN state_hash ELSE '' END FROM checkpoints WHERE revision=0", [], |r| r.get(0)).optional()?
        } else {
            tx.query_row("SELECT CASE WHEN typeof(state_hash)='text' AND length(CAST(state_hash AS BLOB))=64 THEN state_hash ELSE '' END FROM commands WHERE revision=?", [revision], |r| r.get(0)).optional()?
        };
        let mut availability = if !valid_types || !valid_hash(&state_hash) || !valid_hash(&digest) {
            Availability::Corrupt
        } else if version != RECORD_VERSION {
            Availability::Legacy
        } else if id != campaign || revision > current_revision || expected_hash.as_deref() != Some(state_hash.as_str()) {
            Availability::Mismatched
        } else {
            Availability::Available
        };
        // Invalid metadata/version/correlation never needs a payload decode.
        let payload = encoded.filter(|_|availability == Availability::Available).and_then(|encoded| {
            if bytes == 0 || bytes > MAX_PRIVATE_PAYLOAD_BYTES as u64
                || format!("{:x}", Sha256::digest(&encoded)) != digest
                || std::str::from_utf8(&encoded).is_err()
                || !serde_json::from_slice::<serde_json::Value>(&encoded).is_ok_and(|v| v.is_object())
            {
                return None;
            }
            PrivatePayload::from_encoded(encoded).ok()
        });
        if payload.is_none() && availability == Availability::Available {
            availability = Availability::Corrupt;
        }
        records.push(OperatorFailure {
            committed_revision: revision, state_hash, availability,
            payload: if availability == Availability::Available { payload } else { None },
        });
    }
    if records.is_empty() {
        records.push(OperatorFailure {
            committed_revision: current_revision, state_hash: current_hash,
            availability: Availability::Missing, payload: None,
        });
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(path: &Path) -> Connection {
        let db = Connection::open(path).unwrap();
        db.execute_batch(include_str!("schema.sql")).unwrap();
        db.execute("INSERT INTO campaign VALUES(1,?, 'null',0,'null',?,'{\"state\":\"stopped\",\"error\":\"fixed\"}')",
            params!["{\"id\":\"synthetic\"}","a".repeat(64)]).unwrap();
        db
    }
    fn payload(number: u32) -> PrivatePayload {
        PrivatePayload::from_encoded(format!("{{\"version\":1,\"quantity\":{number}}}").into_bytes()).unwrap()
    }

    #[test]
    fn newest_32_private_records_and_integrity_checked_read_only_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.sqlite");
        let mut db = fixture(&path);
        for n in 0..35 {
            let tx = db.transaction().unwrap();
            store(&tx,"synthetic",0,&"a".repeat(64),&payload(n)).unwrap();
            tx.commit().unwrap();
        }
        let before: i64 = db.query_row("PRAGMA data_version",[],|r|r.get(0)).unwrap();
        let rows = read_local(&path).unwrap();
        assert_eq!(rows.len(),32);
        assert!(rows.iter().all(|r|r.availability == Availability::Available));
        assert_eq!(rows[0].private_payload().unwrap().as_bytes(),payload(34).as_bytes());
        assert_eq!(rows[31].private_payload().unwrap().as_bytes(),payload(3).as_bytes());
        assert_eq!(db.query_row("PRAGMA data_version",[],|r|r.get::<_,i64>(0)).unwrap(),before);
        assert_eq!(db.query_row("SELECT count(*) FROM operator_failures",[],|r|r.get::<_,u64>(0)).unwrap(),32);

        db.execute("UPDATE operator_failures SET payload_sha256=? WHERE id=(SELECT max(id) FROM operator_failures)",["b".repeat(64)]).unwrap();
        let corrupt = read_local(&path).unwrap();
        assert_eq!(corrupt[0].availability,Availability::Corrupt);
        assert!(corrupt[0].private_payload().is_none());
        assert_eq!(format!("{:?}",corrupt[0]),"OperatorFailure([redacted])");
        db.execute("UPDATE operator_failures SET version=999 WHERE id=(SELECT max(id) FROM operator_failures)",[]).unwrap();
        assert_eq!(read_local(&path).unwrap()[0].availability,Availability::Legacy);
        db.execute("UPDATE operator_failures SET version=1,state_hash=? WHERE id=(SELECT max(id) FROM operator_failures)",["c".repeat(64)]).unwrap();
        assert_eq!(read_local(&path).unwrap()[0].availability,Availability::Mismatched);
    }

    #[test]
    fn missing_and_legacy_tables_do_not_migrate_or_create_a_database() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("absent.sqlite");
        assert!(read_local(&absent).is_err());
        assert!(!absent.exists());
        let path = dir.path().join("legacy.sqlite");
        let db = fixture(&path);
        assert_eq!(read_local(&path).unwrap()[0].availability,Availability::Missing);
        db.execute_batch("DROP TABLE operator_failures").unwrap();
        assert_eq!(read_local(&path).unwrap()[0].availability,Availability::Legacy);
        assert_eq!(db.query_row("SELECT count(*) FROM sqlite_master WHERE name='operator_failures'",[],|r|r.get::<_,u64>(0)).unwrap(),0);
    }

    #[test]
    fn ignored_pruning_rolls_back_new_private_record_without_deleting_old_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ignored-prune.sqlite");
        let mut db = fixture(&path);
        for quantity in 0..RETAINED_RECORDS as u32 {
            let tx = db.transaction().unwrap();
            store(&tx,"synthetic",0,&"a".repeat(64),&payload(quantity)).unwrap();
            tx.commit().unwrap();
        }
        let old_ids: Vec<u64> = db.prepare("SELECT id FROM operator_failures ORDER BY id")
            .unwrap().query_map([],|row|row.get(0)).unwrap().map(Result::unwrap).collect();
        db.execute_batch("CREATE TRIGGER ignore_prune BEFORE DELETE ON operator_failures BEGIN SELECT RAISE(IGNORE); END;").unwrap();
        {
            let tx = db.transaction().unwrap();
            assert!(matches!(store(&tx,"synthetic",0,&"a".repeat(64),&payload(99)),Err(Error::Recovery(_))));
            // Production's combined-stop caller drops this failed transaction
            // before its stop-only fallback; no diagnostic retry is performed.
        }
        let remaining: Vec<u64> = db.prepare("SELECT id FROM operator_failures ORDER BY id")
            .unwrap().query_map([],|row|row.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(remaining,old_ids);
        let records = read_local(&path).unwrap();
        assert_eq!(records.len(),RETAINED_RECORDS);
        assert_eq!(records[0].private_payload().unwrap().as_bytes(),payload(31).as_bytes());
    }

    #[test]
    fn reader_metadata_limits_count_utf8_bytes_not_characters() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("utf8-metadata.sqlite");
        let db = fixture(&path);
        // 65 characters occupy 130 encoded UTF-8 bytes and cannot evade the
        // 128-byte campaign correlation limit by SQLite TEXT length semantics.
        let overlong = serde_json::json!({"id":"é".repeat(65)}).to_string();
        db.execute("UPDATE campaign SET meta=?",[overlong]).unwrap();
        assert!(read_local(&path).is_err());
        let oversized = serde_json::json!({"id":"synthetic","padding":"é".repeat(32768)}).to_string();
        db.execute("UPDATE campaign SET meta=?",[oversized]).unwrap();
        assert!(read_local(&path).is_err());
    }

    #[test]
    fn selected_campaign_and_hash_syntax_cannot_be_repaired_by_matching_corrupt_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("renamed.sqlite");
        let mut db = fixture(&path);
        let tx = db.transaction().unwrap();
        store(&tx,"synthetic",0,&"a".repeat(64),&payload(13)).unwrap();
        tx.commit().unwrap();
        assert_eq!(read_campaign(&path,"synthetic").unwrap()[0].availability,Availability::Available);
        assert!(read_campaign(&path,"different-campaign").is_err());
        db.execute("UPDATE operator_failures SET state_hash=?",["z".repeat(64)]).unwrap();
        assert_eq!(read_local(&path).unwrap()[0].availability,Availability::Corrupt);
        db.execute("UPDATE campaign SET state_hash=?",["z".repeat(64)]).unwrap();
        assert!(read_local(&path).is_err(),"two matching non-hashes are not integrity evidence");
    }

    #[test]
    fn malformed_record_types_do_not_expose_payload_or_hide_valid_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("malformed-record.sqlite");
        let mut db = fixture(&path);
        for quantity in [13,71] {
            let tx = db.transaction().unwrap();
            store(&tx,"synthetic",0,&"a".repeat(64),&payload(quantity)).unwrap();
            tx.commit().unwrap();
        }
        // SQLite affinity permits corrupt non-integer version values. The
        // bounded reader reports this row unavailable, retaining valid siblings.
        db.execute("UPDATE operator_failures SET version='not-an-integer' WHERE id=(SELECT max(id) FROM operator_failures)",[]).unwrap();
        let rows = read_local(&path).unwrap();
        assert_eq!(rows[0].availability,Availability::Corrupt);
        assert!(rows[0].private_payload().is_none());
        assert_eq!(rows[1].availability,Availability::Available);
        assert_eq!(rows[1].private_payload().unwrap().as_bytes(),payload(13).as_bytes());
        db.execute("UPDATE operator_failures SET version=1,payload=? WHERE id=(SELECT max(id) FROM operator_failures)",["é".repeat(32768)]).unwrap();
        let rows = read_local(&path).unwrap();
        assert_eq!(rows[0].availability,Availability::Corrupt);
        assert!(rows[0].private_payload().is_none());
        assert_eq!(rows[1].availability,Availability::Available);
    }
}
