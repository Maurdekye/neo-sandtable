//! Authored SOURCE controls only; all remain UNCOMPILED and UNRUN.
use super::*;
use crate::{Error, Pins, CampaignStatus, http::{CampaignKind, CreateRequest},
    owned_writer::{StoredWriterRow, WriterRecord, WriterJoin, WriterStartFailure},
    owned_retained::JoinObservation};
use cna_core::ids::SeatId;
use cna_protocol::{CampaignMeta, SeatInfo, SeatStatus};
use rusqlite::{OpenFlags, params};
use std::{path::Path, time::Duration};
use tokio::{runtime::Handle, time::Instant};

fn request(original_title: String) -> CreateRequest {
    CreateRequest { kind: CampaignKind::Sandbox, rules_profile: cna_sandbox::PROFILE_ID.into(),
        seed: [7;32], title: original_title, paused: true, controller: "human".into() }
}
fn metadata(profile: &str, scenario: &str, original_title: &str) -> CampaignMeta {
    CampaignMeta { id: "00000000-0000-0000-0000-000000000001".into(),
        scenario_id: scenario.into(), rules_profile: profile.into(), title: original_title.into(),
        seats: SeatId::all().map(|id| SeatInfo { id: id.to_string(), side: id.side,
            role: id.role, controller: None, status: SeatStatus::Idle }).collect() }
}
fn raw(profile: &str, scenario: &str, title: &str) -> [String;4] {
    let pins = Pins { rules_profile: profile.into(), content_hash: "a".repeat(64), engine_version: "b".repeat(64) };
    [serde_json::to_string(&metadata(profile,scenario,title)).unwrap(),
        serde_json::to_string(&pins).unwrap(), "c".repeat(64),
        serde_json::to_string(&CampaignStatus::Running).unwrap()]
}
fn admitted(original: &[String;4], filename: &str) -> Result<(), RowFailure> {
    fields([original[0].as_bytes(),original[1].as_bytes(),original[2].as_bytes(),original[3].as_bytes(),filename.as_bytes()])
}
fn database(path: &Path, original: &[String;4]) -> Connection {
    let db=Connection::open(path).unwrap();
    db.execute_batch("CREATE TABLE campaign(id INTEGER PRIMARY KEY,meta TEXT,pins TEXT,revision INTEGER,rng TEXT,state_hash TEXT,status TEXT)").unwrap();
    db.execute("INSERT INTO campaign VALUES(1,?,?,0,'{}',?,?)",
        params![original[0],original[1],original[2],original[3]]).unwrap();
    db
}

#[test]
fn title_uses_utf8_bytes_and_forbids_each_c0_without_trimming() {
    assert_eq!(title(&"é".repeat(128)),Ok(()));
    assert_eq!(title(&format!("{}x","é".repeat(128))),Err(TitleRefusal::Bytes { actual:257 }));
    assert_eq!(title("  preserved  "),Ok(()));
    assert_eq!(title("\u{7f}"),Ok(())); // C0 policy ends at U+001F.
    for c in 0..=31 {
        assert_eq!(title(&format!("é{}",char::from_u32(c).unwrap())),
            Err(TitleRefusal::Control { byte_index:2 }));
    }
}

#[test]
fn actual_serializer_reaches_the_three_profile_maxima_and_checked_r() {
    let maxima=[(cna_rules::PROFILE_DEV,"graziani",META_DEV_MAX,PINS_DEV_MAX,11090),
        (cna_rules::PROFILE_FULL,"graziani",META_FULL_MAX,PINS_FULL_MAX,11092),
        (cna_sandbox::PROFILE_ID,"sandbox",META_SANDBOX_MAX,PINS_SANDBOX_MAX,11085)];
    let original_title="\"".repeat(256);
    assert_eq!(serde_json::to_string(&original_title).unwrap().len()-2,TITLE_ESCAPED_MAX);
    for (profile,scenario,m,p,total) in maxima {
        let mut row=raw(profile,scenario,&original_title);
        assert_eq!(row[0].len(),m); assert_eq!(row[1].len(),p);
        row[3]=" ".repeat(STATUS_MAX); // Raw length cap; status semantics decoded later.
        assert!(admitted(&row,&"d".repeat(DATABASE_PATH_MAX)).is_ok());
        let sum=[m,p,STATE_HASH_MAX,STATUS_MAX,DATABASE_PATH_MAX].into_iter()
            .try_fold(0usize,usize::checked_add).unwrap();
        assert_eq!(sum,total);
    }
    assert_eq!(WRITER_ROW_BYTES,11092);
    assert_eq!(WRITER_ROW_BYTES.checked_mul(3),Some(33276)); // payload, not Layout.
}

#[test]
fn raw_profile_caps_refuse_one_byte_over_before_decoding_or_copying() {
    for (profile,scenario,m,p) in [(cna_rules::PROFILE_DEV,"graziani",META_DEV_MAX,PINS_DEV_MAX),
        (cna_rules::PROFILE_FULL,"graziani",META_FULL_MAX,PINS_FULL_MAX),
        (cna_sandbox::PROFILE_ID,"sandbox",META_SANDBOX_MAX,PINS_SANDBOX_MAX)] {
        let mut row=raw(profile,scenario,"");
        let padding=m-row[0].len(); row[0].extend(std::iter::repeat_n(' ',padding));
        assert!(admitted(&row,"path").is_ok());
        row[0].push(' ');
        assert!(matches!(admitted(&row,"path"),Err(RowFailure::Capacity { required,available }) if required==m+1 && available==m));
        row=raw(profile,scenario,"");
        let padding=p-row[1].len(); row[1].extend(std::iter::repeat_n(' ',padding));
        assert!(admitted(&row,"path").is_ok());
        row[1].push(' ');
        assert!(matches!(admitted(&row,"path"),Err(RowFailure::Capacity { required,available }) if required==p+1 && available==p));
    }
}

#[test]
fn alternate_json_escaping_is_admitted_without_raw_rewrite() {
    let mut row=raw(cna_rules::PROFILE_DEV,"graziani","quoted \" and slash \\");
    row[1]=row[1].replace("cna-2021-dev","cna-2021-d\\u0065v");
    // Free room by removing irrelevant hex text: private profile visitor still
    // decodes correctly; real recovery's Pins equality rejects changed pins.
    row[1]=row[1].replace(&"a".repeat(64),"a");
    let original=row[1].clone();
    assert!(admitted(&row,"path").is_ok());
    assert_eq!(row[1],original);
    row[0]=row[0].replace("quoted","\\u0071uoted");
    assert!(admitted(&row,"path").is_ok());
    row[0]=row[0].replace("\\u0071uoted","\\u0001");
    assert!(matches!(admitted(&row,"path"),Err(RowFailure::Title(TitleRefusal::Control { .. }))));
}

#[test]
fn raw_status_boundary_and_real_filename_boundary_are_independent() {
    let mut row=raw(cna_rules::PROFILE_FULL,"graziani","");
    row[3]=" ".repeat(8192);
    assert!(admitted(&row,&"p".repeat(1032)).is_ok());
    row[3].push(' ');
    assert!(matches!(admitted(&row,"p"),Err(RowFailure::Capacity { required:8193,available:8192 })));
    assert!(matches!(path(Some(&"p".repeat(1033))),Err(RowFailure::Capacity { required:1033,available:1032 })));
    assert!(matches!(path(None),Err(RowFailure::MissingConnectionPath)));
    assert!(matches!(path(Some("")),Err(RowFailure::MissingConnectionPath)));
}

#[test]
fn raw_recovery_refusal_leaves_original_file_and_journal_mode_unchanged() {
    let dir=tempfile::tempdir().unwrap(); let filename=dir.path().join("refused.sqlite");
    let mut row=raw(cna_sandbox::PROFILE_ID,"sandbox","");
    let padding=PINS_SANDBOX_MAX+1-row[1].len(); row[1].extend(std::iter::repeat_n(' ',padding));
    drop(database(&filename,&row));
    let original=std::fs::read(&filename).unwrap();
    let db=Connection::open_with_flags(&filename,OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert!(matches!(recovery_fields(&db),Err(RowFailure::Capacity { .. })));
    assert!(matches!(recovery_identity(&db),Err(RowFailure::Capacity { .. })));
    assert!(matches!(check_recovery(&db),Err(RowFailure::Capacity { .. })));
    let mode:String=db.query_row("PRAGMA journal_mode",[],|r|r.get(0)).unwrap();
    assert_eq!(mode,"delete"); drop(db);
    assert_eq!(std::fs::read(&filename).unwrap(),original);
    assert!(!dir.path().join("refused.sqlite-wal").exists());
}

#[test]
fn dispatcher_refuses_oversize_before_loading_content_or_opening_writer() {
    let dir=tempfile::tempdir().unwrap(); let filename=dir.path().join("dispatch.sqlite");
    let mut row=raw(cna_sandbox::PROFILE_ID,"sandbox",""); row[3]="s".repeat(8193);
    drop(database(&filename,&row)); let original=std::fs::read(&filename).unwrap();
    let owner=WriterJoin::<Result<(),Error>,WRITER_ROW_BYTES>::new_prepaid();
    let publication=owner.creator_publication(); let claimed=publication.claim().unwrap();
    let result=crate::campaigns::recover_owned(&filename,&dir.path().join("absent-content"),claimed);
    assert!(matches!(result,Err(WriterStartFailure::Envelope(RowFailure::Capacity { required:8193,available:8192 }))));
    assert!(owner.require_pairing_window().is_err());
    assert_eq!(std::fs::read(&filename).unwrap(),original);
    assert!(!dir.path().join("dispatch.sqlite-wal").exists());
}

#[test]
fn retained_direct_fresh_refuses_title_before_path_publication_or_content() {
    let dir=tempfile::tempdir().unwrap();
    let record=WriterRecord::<WRITER_ROW_BYTES>::new_prepaid();
    let original=request("x".repeat(257));
    let result=crate::campaigns::prepare_guarded_fresh(dir.path(),&dir.path().join("absent"),original,&record);
    assert!(matches!(result,Err(WriterStartFailure::Envelope(RowFailure::Title(TitleRefusal::Bytes { actual:257 })))));
    assert!(record.attempt_path().is_none());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(),0);
}

#[tokio::test]
async fn observer_returns_same_invalid_title_input_before_census_consumption() {
    let dir=tempfile::tempdir().unwrap();
    let owner=crate::owned_ingress::CreationOwner::<1,WRITER_ROW_BYTES>::new_prepaid(
        Handle::current(),uuid::Uuid::from_u128(518),dir.path().into(),dir.path().join("absent"));
    let observer=owner.observer(); let original=request("x\nx".into());
    let address=original.title.as_ptr();
    match observer.submit(crate::owned_ingress::CreationInput::Fresh(original)) {
        Err((crate::owned_ingress::IngressRefusal::Title(TitleRefusal::Control { byte_index:1 }),
            crate::owned_ingress::CreationInput::Fresh(returned))) => {
                assert_eq!(returned.title,"x\nx"); assert_eq!(returned.title.as_ptr(),address);
            }
        _=>panic!("expected typed refusal with original input"),
    }
    assert!(observer.submit(crate::owned_ingress::CreationInput::Fresh(request("accepted".into()))).is_ok());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(),0);
}

#[test]
fn initial_path_failure_is_first_retained_and_cannot_be_replaced() {
    let record=WriterRecord::<WRITER_ROW_BYTES>::new_prepaid();
    let memory=Connection::open_in_memory().unwrap();
    assert!(!record.capture_initial_path(&memory));
    let original=record.initial_path().unwrap() as *const _;
    let dir=tempfile::tempdir().unwrap(); let file=Connection::open(dir.path().join("ok.sqlite")).unwrap();
    assert!(!record.capture_initial_path(&file));
    assert_eq!(record.initial_path().unwrap() as *const _,original);
    assert!(matches!(record.initial_path(),Some(Err(RowFailure::MissingConnectionPath))));
    assert!(record.initial().is_none()); // preflight did not replace row snapshot.
}

#[test]
fn after_gate_initial_still_detects_actual_database_drift() {
    let dir=tempfile::tempdir().unwrap(); let filename=dir.path().join("drift.sqlite");
    let original=raw(cna_sandbox::PROFILE_ID,"sandbox",""); let db=database(&filename,&original);
    let record=WriterRecord::<WRITER_ROW_BYTES>::new_prepaid();
    assert!(record.capture_initial_path(&db)); record.capture_prepared(&db);
    db.execute("UPDATE campaign SET status=? WHERE id=1",["{\"state\":\"paused\"}"]).unwrap();
    assert!(!record.capture_initial(&db));
    assert!(matches!(record.start_binding(),Some(Err(crate::owned_writer::StartBindingFailure::ChangedField { index:3 }))));
    assert!(record.prepared().unwrap().is_ok()); assert!(record.initial().unwrap().is_ok());
}

#[tokio::test]
async fn terminal_oversize_keeps_original_status_raw_error_and_join_and_refuses_evidence() {
    let dir=tempfile::tempdir().unwrap(); let filename=dir.path().join("terminal.sqlite");
    let row=raw(cna_sandbox::PROFILE_ID,"sandbox",""); let db=database(&filename,&row);
    let mut owner=WriterJoin::<Result<(),Error>,WRITER_ROW_BYTES>::new_prepaid();
    let record=std::sync::Arc::clone(owner.record());
    assert!(record.capture_initial_path(&db)); record.capture_prepared(&db);
    let diagnostic="d".repeat(8193);
    let original_status=serde_json::to_string(&CampaignStatus::Stopped { error:diagnostic.clone() }).unwrap();
    let stored=original_status.clone(); let raw_cause=diagnostic.clone();
    let standard=std::thread::spawn(move || {
        assert!(record.await_owner_publication()); assert!(record.capture_initial(&db));
        db.execute("UPDATE campaign SET status=? WHERE id=1",[stored]).unwrap();
        record.capture_terminal(&db);
        Err(Error::Recovery(raw_cause))
    });
    assert!(owner.publish_standard(standard).is_ok()); owner.release().unwrap();
    owner.dispatch_join(&Handle::current()).unwrap();
    owner.join_until(Instant::now()+Duration::from_secs(5)).await.unwrap();
    let address=match owner.observe() {
        JoinObservation::Returned { original:Ok(Ok(Err(Error::Recovery(cause)))),was_unknown:false }=> {
            assert_eq!(cause,&diagnostic); cause.as_ptr()
        }, _=>panic!("whole original writer cause not held"),
    };
    assert!(matches!(owner.record().terminal(),Some(Err(RowFailure::Capacity { required,available:8192 })) if *required==original_status.len()));
    assert!(owner.terminal_rows().is_err());
    match owner.observe() { JoinObservation::Returned { original:Ok(Ok(Err(Error::Recovery(cause)))), .. }=>assert_eq!(cause.as_ptr(),address), _=>panic!("raw result replaced") }
    let db=Connection::open_with_flags(&filename,OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let persisted:String=db.query_row("SELECT status FROM campaign WHERE id=1",[],|r|r.get(0)).unwrap();
    assert_eq!(persisted,original_status);
    // This is not a real Host/Play UNKNOWN-exit70/CLEAN control; that fixture is owed.
}

#[test]
fn actual_stored_row_capture_preserves_raw_whitespace_under_envelope() {
    let dir=tempfile::tempdir().unwrap(); let filename=dir.path().join("raw.sqlite");
    let mut original=raw(cna_sandbox::PROFILE_ID,"sandbox",""); original[0].push(' ');
    // Pins are already at their canonical cap: even a single trailing space
    // there is refused, while an under-cap metadata spelling stays exact.
    let db=database(&filename,&original);
    let captured=StoredWriterRow::<WRITER_ROW_BYTES>::capture(&db).unwrap();
    assert_eq!(captured.field(0).unwrap(),original[0].as_bytes());
    assert_eq!(captured.field(1).unwrap(),original[1].as_bytes());
}
