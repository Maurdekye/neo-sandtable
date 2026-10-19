use super::*;
#[path = "owned_writer_actor_tests.rs"]
mod owned_writer_actor_tests;
#[path = "host_pending_tests.rs"]
mod pending_copy_tests;
#[path = "writer_fatal_report_tests.rs"]
mod writer_fatal_report_tests;
#[path = "worker_panic_seat_tests.rs"]
mod worker_panic_seat_tests;
use crate::Pins;
use cna_core::{
    clock::{Anchor, Clock},
    decision::{ActionSchema, ActionSpace, Secrecy, Trigger},
    dice::CampaignRng,
    engine::{Cx, EngineError, Game, Progress},
};
use cna_protocol::ControllerKind;
use std::sync::atomic::{AtomicUsize, Ordering};

struct NullPolicyRules {
    pass: bool,
    responses: Arc<AtomicUsize>,
}

// These controls exercise the actual CampaignHandle writer and imported
// reservation/worker capability. No model/provider/CLI is part of the fixture.
fn forced_fixture(path:&Path) -> (CampaignHandle,cna_seats::host::HostIdentity,
    cna_seats::game::SubmitRequest,Arc<AtomicUsize>) {
    let responses = Arc::new(AtomicUsize::new(0));
    let pins = Pins {rules_profile:"null-policy-test-v1".into(),
        content_hash:"empty".into(),engine_version:"policy-test-v1".into()};
    let mut campaign = Campaign::create(path,NullPolicyRules {pass:false,responses:responses.clone()},(),
        Game {state:0,rng:CampaignRng::from_seed([7;32]).state()},
        CampaignMeta {id:"forced-fixture".into(),scenario_id:"test".into(),
            rules_profile:pins.rules_profile.clone(),title:"Forced fixture".into(),seats:vec![]},pins).unwrap();
    let seat = SeatId::all().next().unwrap();
    campaign.handover(seat,Some(ControllerInfo {kind:ControllerKind::LlmCli,label:"synthetic".into()}),Value::Null).unwrap();
    let pending = campaign.pending().into_iter().find(|request|request.seat==seat).unwrap();
    let identity = cna_seats::host::HostIdentity {seat,epoch:campaign.binding(seat).controller_epoch,
        decision_id:pending.id.to_string(),revision:pending.revision,operation_id:uuid::Uuid::from_u128(4).to_string()};
    let request = cna_seats::game::SubmitRequest {decision_id:identity.decision_id.clone(),epoch:identity.epoch,
        revision:Some(identity.revision),idempotency_key:"fixture-original-key".into(),
        action:json!(1),public_explanation:None};
    (CampaignHandle::spawn(campaign,path,None).unwrap(),identity,request,responses)
}

fn forced_operation(id:&str) -> cna_seats::operation::Operation {
    cna_seats::operation::Operation::new(id.into(),1,
        cna_seats::operation::Allowance::required(Some(tokio::time::Instant::now()
            +std::time::Duration::from_secs(5))).unwrap()).unwrap()
}

// Separate from NullPolicyRules: those original policy controls deliberately
// permit an invented null answer. This fixture actually exposes ONLY a pass.
struct ForcedPassRules(NullPolicyRules);
impl Ruleset for ForcedPassRules {
    type State = u8;
    type Content = ();
    fn profile_id(&self) -> &str { "forced-pass-test-v1" }
    fn advance(&self,content:&(),state:&mut u8,cx:&mut Cx<'_>) -> Result<Progress,EngineError> {
        self.0.advance(content,state,cx)
    }
    fn respond(&self,content:&(),state:&mut u8,response:&DecisionResponse,cx:&mut Cx<'_>) -> Result<(),Rejection> {
        if !response.action.is_null() {
            return Err(Rejection::Illegal {message:"synthetic forced pass requires null".into()});
        }
        self.0.respond(content,state,response,cx)
    }
    fn pending(&self,content:&(),state:&u8) -> Vec<DecisionRequest> {
        let mut pending = self.0.pending(content,state);
        for request in &mut pending {
            request.space = ActionSpace::new(ActionSchema::List {
                item:Box::new(ActionSchema::Integer {min:1,max:1}),min:0,max:0,
            });
            request.space.pass = Some("declared done".into());
        }
        pending
    }
    fn observe(&self,content:&(),state:&u8,perspective:Perspective) -> Value {
        self.0.observe(content,state,perspective)
    }
    fn view(&self,content:&(),state:&u8,perspective:Perspective) -> ViewState {
        self.0.view(content,state,perspective)
    }
}

fn forced_pass_fixture(path:&Path) -> (CampaignHandle,cna_seats::host::HostIdentity,
    cna_seats::game::SubmitRequest,Arc<AtomicUsize>,Pins) {
    let responses = Arc::new(AtomicUsize::new(0));
    let pins = Pins {rules_profile:"forced-pass-test-v1".into(),
        content_hash:"empty".into(),engine_version:"policy-test-v1".into()};
    let mut campaign = Campaign::create(path,ForcedPassRules(NullPolicyRules {
        pass:false,responses:responses.clone()}),(),
        Game {state:0,rng:CampaignRng::from_seed([7;32]).state()},
        CampaignMeta {id:"forced-pass-fixture".into(),scenario_id:"test".into(),
            rules_profile:pins.rules_profile.clone(),title:"Forced pass fixture".into(),seats:vec![]},pins.clone()).unwrap();
    let seat = SeatId::all().next().unwrap();
    campaign.handover(seat,Some(ControllerInfo {kind:ControllerKind::LlmCli,label:"synthetic".into()}),Value::Null).unwrap();
    let pending = campaign.pending().into_iter().find(|request|request.seat==seat).unwrap();
    assert!(cna_seats::automatic::forced_pass(&pending.space));
    let identity = cna_seats::host::HostIdentity {seat,epoch:campaign.binding(seat).controller_epoch,
        decision_id:pending.id.to_string(),revision:pending.revision,operation_id:uuid::Uuid::from_u128(5).to_string()};
    let request = cna_seats::game::SubmitRequest {decision_id:identity.decision_id.clone(),epoch:identity.epoch,
        revision:Some(identity.revision),idempotency_key:"forced-pass-original-key".into(),
        action:Value::Null,public_explanation:None};
    (CampaignHandle::spawn(campaign,path,None).unwrap(),identity,request,responses,pins)
}

async fn retained_forced_execute_control(reject_postcommit:bool) {
    use cna_seats::{automatic::ForcedAnswerSupervisor,operation::StartEvidence,transcript::TranscriptSink};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retained-execute.sqlite");
    let (handle,identity,request,responses,pins) = forced_pass_fixture(&path);
    let key = request.idempotency_key.clone();
    let operation = forced_operation(&identity.operation_id);
    let sink = TranscriptSink::new(Arc::new(handle.clone()));
    let mut supervisor = ForcedAnswerSupervisor::new(handle.host_forced_submission(),identity.clone(),request,operation.clone()).unwrap();
    let cancel = supervisor.cancellation();
    if reject_postcommit {
        // This NEW synthetic DB trigger rejects only the caller's postcommit row.
        // LlmCli submissions do not insert a scripted transcript in the command
        // transaction, so the accepted command must already exist at this fault.
        rusqlite::Connection::open(&path).unwrap().execute_batch(
            "CREATE TRIGGER reject_postcommit BEFORE INSERT ON transcripts WHEN json_extract(NEW.entry,'$.kind')='decision_submitted' BEGIN SELECT RAISE(FAIL,'synthetic postcommit capture fault'); END;"
        ).unwrap();
    }
    let executed = supervisor.execute(&sink).await;
    let no_actor_ack = cancel.acknowledged_settlement().is_none();
    let admission_closed = handle.host_admission().snapshot().map(|snapshot|!snapshot.activated).unwrap_or(true);
    let retained_pending = sink.pending_count();
    let bounded_join = sink.join_delivery_until(supervisor.allowance().deadline()).await;
    if bounded_join.is_err() {
        // Test cleanup only: retain/await the SAME original worker on regression,
        // never pretend the expired bound joined it. No production cap/allowance
        // extension is supplied. Assertions below still fail the bounded path.
        let _synthetic_outbox = sink.stop_delivery().await;
    }
    let no_ack_after_worker_join = cancel.acknowledged_settlement().is_none();
    supervisor.request_settlement_after_supervision();
    let bounded_ack = supervisor.settlement_ack().await;
    let bounded_ack_succeeded = bounded_ack.is_ok();
    let ack = match bounded_ack {
        Ok(ack)=>ack,
        Err(_)=>{
            // Actual retained actor witness is needed even for failed test cleanup.
            // The first timed wait is still reported as a failure, not as a join.
            let ack = cancel.settlement_ack().await;
            let _ = supervisor.settlement_ack().await;
            ack
        }
    };
    let shutdown = handle.shutdown().await;
    // All fixture-owned work is actually joined before assertions or DB recovery.
    if reject_postcommit {
        assert!(executed.is_err());
        assert!(shutdown.is_err(),"original routed storage error survives writer join");
        assert!(ack.uncertain(),"failed second marker never claims complete delivery");
        assert_eq!(retained_pending,1,"failed accepted row remains in the canonical mirror");
    } else {
        assert!(executed.unwrap());
        shutdown.unwrap();
        assert!(!ack.uncertain(),"both actual marker ACKs preserve complete delivery");
        assert_eq!(retained_pending,0);
    }
    bounded_join.unwrap();
    assert!(bounded_ack_succeeded);
    assert!(no_actor_ack && no_ack_after_worker_join && admission_closed);
    let receipt = supervisor.accepted_receipt().expect("actual accepted R stays retained after settlement");
    assert_eq!(receipt.decision_id,identity.decision_id);
    assert_eq!(receipt.result,json!({"accepted":true}));
    assert!(!receipt.duplicate);
    assert_eq!(responses.load(Ordering::SeqCst),1);
    assert_eq!(operation.start_evidence(),StartEvidence::NotStarted,"forced path never dispatches a model");
    assert!(handle.inner.thread.lock().unwrap().is_none());
    let recovered = Campaign::recover(&path,ForcedPassRules(NullPolicyRules {pass:false,responses}),(),&pins).unwrap();
    assert_eq!(recovered.game.state,1);
    assert_eq!(recovered.game.rng,CampaignRng::from_seed([7;32]).state());
    let accepted = recovered.prior_response(identity.seat,&key).unwrap().unwrap();
    assert_eq!(accepted.action,Value::Null);
    assert_eq!(accepted.controller_epoch,identity.epoch);
    assert_eq!(accepted.decision_revision,identity.revision);
    let rows = recovered.transcripts_after(Perspective::Seat(identity.seat),identity.seat,0,10).unwrap();
    assert_eq!(rows.len(),if reject_postcommit {1} else {2});
    assert!(matches!(&rows[0],ServerMessage::Transcript {entry:TranscriptEntry::System {text},..}
        if text.contains("Automatic forced answer attempt: pass")));
    if !reject_postcommit {
        assert!(matches!(&rows[1],ServerMessage::Transcript {entry:TranscriptEntry::DecisionSubmitted {decision_id,..},..}
            if decision_id==&identity.decision_id));
    }
    let enemy = SeatId::all().find(|seat|seat.side!=identity.seat.side).unwrap();
    assert!(recovered.transcripts_after(Perspective::Side(enemy.side),identity.seat,0,10).unwrap().is_empty());
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM commands WHERE seat=? AND idempotency_key=?",
        rusqlite::params![identity.seat.to_string(),key],|row|row.get::<_,u64>(0)).unwrap(),1);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn retained_execute_confirms_both_markers_before_distinct_worker_and_actor_settlement() {
    retained_forced_execute_control(false).await;
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn retained_execute_keeps_actual_receipt_and_outbox_after_postcommit_storage_failure() {
    retained_forced_execute_control(true).await;
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn oversized_reservation_refuses_before_sink_prepare_or_campaign_claim() {
    use cna_seats::{host::HostRefusal,operation::{COMMAND_BYTES,OperationError},transcript::TranscriptSink};
    let dir = tempfile::tempdir().unwrap();
    let (handle,identity,mut request,responses) = forced_fixture(&dir.path().join("oversized-reserve.sqlite"));
    let sink = TranscriptSink::new(Arc::new(handle.clone()));
    let operation = forced_operation(&identity.operation_id);
    request.public_explanation = Some("x".repeat(COMMAND_BYTES));
    let refusal = handle.host_forced_submission().reserve(identity,request,operation.clone());
    let snapshot = handle.host_admission().snapshot();
    let reserved = handle.inner.forced.reserved();
    let pending = sink.pending_count();
    let outbox = sink.stop_delivery().await;
    let shutdown = handle.shutdown().await;
    assert!(matches!(refusal,Err(HostRefusal::CaptureOverflow)));
    assert!(matches!(operation.check(),Err(OperationError::CaptureOverflow)));
    assert!(!reserved,"oversized original body never claims the campaign");
    assert!(snapshot.unwrap().activated,"refused reserve cannot close healthy admission");
    assert_eq!(pending,0);assert!(outbox.is_empty());
    shutdown.unwrap();
    assert_eq!(responses.load(Ordering::SeqCst),0);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn post_reserve_overflow_keeps_reserved_no_route_actual_ack() {
    use cna_seats::{operation::OPERATION_BYTES,transcript::TranscriptSink};
    let dir = tempfile::tempdir().unwrap();
    let (handle,identity,request,responses) = forced_fixture(&dir.path().join("reserved-cap.sqlite"));
    let sink = TranscriptSink::new(Arc::new(handle.clone()));
    let operation = forced_operation(&identity.operation_id);
    let reservation = handle.host_forced_submission().reserve(identity,request,operation.clone()).unwrap();
    let cancel = reservation.cancellation();
    operation.charge(OPERATION_BYTES-operation.captured_bytes()).unwrap();
    assert!(operation.charge(1).is_err());
    let preparation = sink.prepare_route(reservation.route()).await;
    let still_owned = handle.inner.forced.reserved();
    let before_supervision = cancel.acknowledged_settlement();
    let outbox = sink.stop_delivery().await;
    let settlement = reservation.settle().await;
    let ack = cancel.acknowledged_settlement();
    let shutdown = handle.shutdown().await;
    assert!(preparation.is_err());assert!(still_owned && before_supervision.is_none());
    assert!(outbox.is_empty());settlement.unwrap();assert!(ack.unwrap().uncertain());
    shutdown.unwrap();assert_eq!(responses.load(Ordering::SeqCst),0);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn two_real_sinks_share_campaign_ownership_and_loser_cannot_prepare() {
    use cna_seats::{automatic::ForcedAnswerSupervisor,host::HostRefusal,transcript::TranscriptSink};
    let dir = tempfile::tempdir().unwrap();
    let (handle,identity,request,responses) = forced_fixture(&dir.path().join("two-sinks.sqlite"));
    let first = TranscriptSink::new(Arc::new(handle.clone()));
    let second = TranscriptSink::new(Arc::new(handle.clone()));
    let winner_op = forced_operation(&identity.operation_id);
    let mut winner = ForcedAnswerSupervisor::new(handle.host_forced_submission(),identity.clone(),request.clone(),winner_op).unwrap();
    let mut losing_identity = identity.clone();
    losing_identity.operation_id = "different-sink-operation".into();
    let loser = ForcedAnswerSupervisor::new(handle.host_forced_submission(),losing_identity.clone(),request,
        forced_operation(&losing_identity.operation_id));
    let still_reserved = handle.inner.forced.reserved();
    let loser_pending = second.pending_count();
    let before_activation = handle.host_admission().snapshot().unwrap().activated;
    let preparation = winner.prepare(&first).await;
    // No native work exists. Join both actual delivery tasks before requesting
    // terminal settlement; abort alone must not synthesize the actor witness.
    let first_outbox = first.stop_delivery().await;
    let second_outbox = second.stop_delivery().await;
    winner.request_settlement_after_supervision();
    let ack = winner.settlement_ack().await;
    let shutdown = handle.shutdown().await;
    assert!(matches!(loser,Err(HostRefusal::StoppedBeforeWork)));
    assert!(still_reserved,"sole writer reservation precedes BOTH sink handshakes");
    assert_eq!(loser_pending,0);
    assert!(!before_activation);
    preparation.unwrap();
    assert!(first_outbox.is_empty() && second_outbox.is_empty());
    assert!(ack.unwrap().uncertain(),"prepared-without-BEGIN has no proven complete routed prefix");
    shutdown.unwrap();
    assert_eq!(responses.load(Ordering::SeqCst),0);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn fatal_writer_between_reserve_and_prepare_keeps_original_terminal_ack() {
    use cna_seats::{host::{HostRefusal,HostStatus},transcript::TranscriptSink};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reserved-writer-failure.sqlite");
    let (handle,identity,request,responses) = forced_fixture(&path);
    let sink = TranscriptSink::new(Arc::new(handle.clone()));
    let reservation = handle.host_forced_submission().reserve(identity.clone(),request,
        forced_operation(&identity.operation_id)).unwrap();
    let cancel = reservation.cancellation();
    let mut changes = handle.host_admission().subscribe();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_notebook BEFORE INSERT ON notebooks BEGIN SELECT RAISE(FAIL,'synthetic reserved writer fault'); END;").unwrap();
    let failure = handle.write_notebook(identity.seat,WriteMode::Replace,"synthetic").await;
    while changes.borrow_and_update().status != HostStatus::Unknown {
        changes.changed().await.unwrap();
    }
    let preparation = sink.prepare_route(reservation.route()).await;
    let no_ack_before_supervision = cancel.acknowledged_settlement().is_none();
    let retained = handle.inner.thread.lock().unwrap().as_ref().is_some_and(|thread|!thread.is_finished());
    let outbox = sink.stop_delivery().await;
    let settlement = reservation.settle().await;
    let ack = cancel.acknowledged_settlement();
    let shutdown = handle.shutdown().await;
    assert!(matches!(failure,Err(Error::Storage(_))));
    assert!(preparation.is_err(),"closed writer must cancel reserved sink admission");
    assert!(no_ack_before_supervision && retained);
    assert!(outbox.is_empty());
    settlement.unwrap();
    assert!(ack.unwrap().uncertain());
    assert!(shutdown.is_err(),"original storage failure survives actual writer join");
    assert!(handle.inner.thread.lock().unwrap().is_none());
    assert_eq!(responses.load(Ordering::SeqCst),0);
    assert!(matches!(handle.host_admission().snapshot(),Err(HostRefusal::Unknown)));
}

struct ReleaseFence(Option<std::sync::mpsc::Sender<()>>);
impl Drop for ReleaseFence {
    fn drop(&mut self) { if let Some(release) = self.0.take() { let _ = release.send(()); } }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn shutdown_reserved_before_prepare_waits_for_actual_no_route_settlement() {
    use cna_seats::transcript::TranscriptSink;
    let dir = tempfile::tempdir().unwrap();
    let (handle,identity,request,responses) = forced_fixture(&dir.path().join("shutdown-reserved.sqlite"));
    let operation = forced_operation(&identity.operation_id);
    let sink = TranscriptSink::new(Arc::new(handle.clone()));
    let reservation = handle.host_forced_submission().reserve(identity.clone(),request,operation.clone()).unwrap();
    let cancel = reservation.cancellation();
    let (entered,waiting) = std::sync::mpsc::channel();
    let (release,released) = std::sync::mpsc::channel();
    let fence_guard = ReleaseFence(Some(release));
    let fence_handle = handle.clone();
    let fence = tokio::spawn(async move {fence_handle.call::<()>(Op::WriterFence {entered,release:released}).await});
    tokio::task::spawn_blocking(move || waiting.recv()).await.unwrap().unwrap();
    let stopping = handle.clone();
    let shutdown = tokio::spawn(async move {stopping.shutdown().await});
    // The real writer is held at the test fence. Observe the original Shutdown
    // queued BEFORE releasing it; a following ordinary notebook read then
    // witnesses that the prior conflicting control was deferred, not lost.
    let queued_shutdown = tokio::time::timeout_at(operation.allowance().deadline(),async {
        while handle.inner.tx.capacity()==COMMAND_BUFFER {tokio::task::yield_now().await;}
    }).await;
    drop(fence_guard);
    let fence_result = fence.await;
    let read_while_owned = tokio::time::timeout_at(operation.allowance().deadline(),handle.notebook(identity.seat)).await;
    let held_shutdown = !shutdown.is_finished();
    let no_ack = cancel.acknowledged_settlement().is_none();
    let still_reserved = handle.inner.forced.reserved();
    let worker_join = sink.join_delivery_until(operation.allowance().deadline()).await;
    if worker_join.is_err() {let _synthetic_outbox = sink.stop_delivery().await;}
    // No prepare/begin occurred and no child exists. Even here only the trusted
    // actual-supervision request, followed by the original actor ACK, releases
    // the sole reserved capability and its deferred Shutdown.
    let settlement = reservation.settle().await;
    let ack = cancel.acknowledged_settlement();
    let shutdown_result = shutdown.await;
    queued_shutdown.unwrap();fence_result.unwrap().unwrap();read_while_owned.unwrap().unwrap();worker_join.unwrap();
    assert!(held_shutdown && no_ack && still_reserved);
    settlement.unwrap();assert!(ack.unwrap().uncertain());
    shutdown_result.unwrap().unwrap();
    assert!(handle.inner.thread.lock().unwrap().is_none());
    assert_eq!(responses.load(Ordering::SeqCst),0);
    assert_eq!(sink.pending_count(),0);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn lost_queued_begin_keeps_preallocated_actor_ack_and_blocks_shutdown() {
    use cna_seats::transcript::TranscriptSink;
    let dir = tempfile::tempdir().unwrap();
    let (handle,identity,request,responses) = forced_fixture(&dir.path().join("lost-begin.sqlite"));
    let sink = TranscriptSink::new(Arc::new(handle.clone()));
    let reservation:Arc<dyn cna_seats::host::HostForcedReservation> = handle.host_forced_submission()
        .reserve(identity.clone(),request,forced_operation(&identity.operation_id)).unwrap().into();
    let cancel = reservation.cancellation();
    let route = sink.prepare_route(reservation.route()).await.unwrap();
    let (entered,waiting) = std::sync::mpsc::channel();
    let (release,released) = std::sync::mpsc::channel();
    let fence_guard = ReleaseFence(Some(release));
    let fence_handle = handle.clone();
    let fence = tokio::spawn(async move { fence_handle.call::<()>(Op::WriterFence {entered,release:released}).await });
    tokio::task::spawn_blocking(move || waiting.recv()).await.unwrap().unwrap();
    let begin_owner = reservation.clone();
    let begin = tokio::spawn(async move { begin_owner.begin(route).await });
    while !handle.inner.forced.command_pending() { tokio::task::yield_now().await; }
    begin.abort();
    let original_begin_join = begin.await;
    // This is delivery of an ordinary conflicting control, not a force release.
    let stopping_handle = handle.clone();
    let shutdown = tokio::spawn(async move { stopping_handle.shutdown().await });
    let not_acknowledged = cancel.acknowledged_settlement().is_none();
    drop(fence_guard); // unblock actual original writer, even if an assertion later fails
    let fence_result = fence.await;
    let abort = reservation.abort().await;
    let outbox = sink.stop_delivery().await;
    let held_before_settlement = !shutdown.is_finished();
    let settlement = reservation.settle().await;
    let ack = cancel.acknowledged_settlement();
    let shutdown_result = shutdown.await;
    assert!(matches!(original_begin_join,Err(error) if error.is_cancelled()));
    fence_result.unwrap().unwrap();
    abort.unwrap();
    assert!(not_acknowledged && held_before_settlement);
    assert!(outbox.is_empty());
    settlement.unwrap();
    assert!(ack.unwrap().uncertain());
    shutdown_result.unwrap().unwrap();
    assert_eq!(responses.load(Ordering::SeqCst),0);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn both_markers_keep_original_authors_receipt_and_later_sibling_progress() {
    use cna_seats::transcript::TranscriptSink;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("both-markers.sqlite");
    let (handle,identity,request,responses) = forced_fixture(&path);
    let key = request.idempotency_key.clone();
    let operation = forced_operation(&identity.operation_id);
    let sink = TranscriptSink::new(Arc::new(handle.clone()));
    let forced = sink.for_operation(operation.clone());
    let other = SeatId::all().find(|seat|seat.side!=identity.seat.side).unwrap();
    let reservation = handle.host_forced_submission().reserve(identity.clone(),request,operation).unwrap();
    let route = forced.prepare_route(reservation.route()).await.unwrap();
    let lease = reservation.begin(route).await.unwrap();
    forced.system(identity.seat,"intent");
    let first = forced.confirm_intent().await.unwrap();
    // A sibling after the FIRST marker is allowed to progress; commit cannot
    // require draining that later capture or pending_count == 0.
    sink.system(other,"between-marker sibling");
    let receipt = lease.commit(first).await;
    forced.system(identity.seat,"accepted receipt");
    let second = forced.confirm_postcommit().await;
    sink.system(other,"after-second-marker sibling");
    let final_pending = match second {
        Ok(marker)=>lease.finish_delivery(&key,marker).await,
        Err(_)=>Err(cna_seats::host::HostRefusal::Unknown),
    };
    let later_still_pending = sink.pending_count();
    // There are no child/readers in this fixture; both marker ACKs are actual
    // SQLite append results. The actor releases the second-marker freeze only
    // after the explicit supervisor disposition, never the receipt request ACK.
    let settlement = lease.settle().await;
    let flush = sink.flush_confirmed().await;
    let outbox = sink.stop_delivery().await;
    let shutdown = handle.shutdown().await;
    receipt.unwrap();
    assert!(final_pending.unwrap().is_empty());
    assert_eq!(later_still_pending,1,"second marker freezes later issue only");
    settlement.unwrap();flush.unwrap();
    assert!(outbox.is_empty());shutdown.unwrap();
    assert_eq!(responses.load(Ordering::SeqCst),1,"accepted original command applies once");
    let db = rusqlite::Connection::open(&path).unwrap();
    let authored:Vec<String> = db.prepare("SELECT seat FROM transcripts ORDER BY rowid")
        .unwrap().query_map([],|row|row.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(authored,vec![identity.seat.to_string(),other.to_string(),identity.seat.to_string(),other.to_string()]);
    assert_eq!(db.query_row("SELECT count(*) FROM commands WHERE seat=? AND idempotency_key=?",
        rusqlite::params![identity.seat.to_string(),key],|row|row.get::<_,u64>(0)).unwrap(),1);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn active_overflow_after_accepted_r_keeps_command_and_requires_actual_settlement() {
    use cna_seats::{operation::OPERATION_BYTES,transcript::TranscriptSink};
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("accepted-r-overflow.sqlite");
    let (handle,identity,request,responses)=forced_fixture(&path);
    let key=request.idempotency_key.clone();let operation=forced_operation(&identity.operation_id);
    let sink=TranscriptSink::new(Arc::new(handle.clone()));let forced=sink.for_operation(operation.clone());
    let reservation=handle.host_forced_submission().reserve(identity.clone(),request,operation.clone()).unwrap();
    let cancel=reservation.cancellation();let route=forced.prepare_route(reservation.route()).await.unwrap();
    let lease=reservation.begin(route).await.unwrap();
    forced.system(identity.seat,"intent");let first=forced.confirm_intent().await.unwrap();
    let accepted=lease.commit(first).await;
    // The command has already committed. Overflow closes further capture/effects
    // but cannot refund starts, erase canonical R or replay the accepted answer.
    operation.charge(OPERATION_BYTES-operation.captured_bytes()).unwrap();
    let overflow=operation.charge(1);
    let before_supervision=cancel.acknowledged_settlement();
    let stopping=handle.clone();let shutdown=tokio::spawn(async move {stopping.shutdown().await});
    let _outbox=sink.stop_delivery().await;
    let held=!shutdown.is_finished();
    let settlement=reservation.settle().await;let ack=cancel.acknowledged_settlement();
    let shutdown_result=shutdown.await;
    let accepted=accepted.unwrap();assert_eq!(accepted.result,json!({"accepted":true}));
    assert!(overflow.is_err());assert!(before_supervision.is_none() && held);
    settlement.unwrap();assert!(ack.unwrap().uncertain(),"missing second marker is not complete delivery");
    shutdown_result.unwrap().unwrap();assert_eq!(responses.load(Ordering::SeqCst),1);
    let db=rusqlite::Connection::open(path).unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM commands WHERE seat=? AND idempotency_key=?",
        rusqlite::params![identity.seat.to_string(),key],|row|row.get::<_,u64>(0)).unwrap(),1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_writer_retains_prompt_until_original_settlement_ack() {
    use cna_seats::{host::{HostIdentity,HostRefusal},operation::Allowance};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retained-prompt.sqlite");
    let pins = Pins {rules_profile:"null-policy-test-v1".into(),
        content_hash:"empty".into(),engine_version:"policy-test-v1".into()};
    let mut campaign = Campaign::create(&path,NullPolicyRules {pass:false,
        responses:Arc::new(AtomicUsize::new(0))},(),
        Game {state:0,rng:CampaignRng::from_seed([7;32]).state()},
        CampaignMeta {id:"retained-prompt".into(),scenario_id:"test".into(),
            rules_profile:pins.rules_profile.clone(),title:"Prompt".into(),seats:vec![]},pins).unwrap();
    let seat = SeatId::all().next().unwrap();
    campaign.handover(seat,Some(ControllerInfo {kind:ControllerKind::LlmCli,label:"synthetic".into()}),Value::Null).unwrap();
    let request = campaign.pending().into_iter().find(|r|r.seat==seat).unwrap();
    let identity = HostIdentity {seat,epoch:campaign.binding(seat).controller_epoch,
        decision_id:request.id.to_string(),revision:request.revision,
        operation_id:uuid::Uuid::from_u128(3).to_string()};
    let handle = CampaignHandle::spawn(campaign,&path,None).unwrap();
    let admission = handle.host_admission();
    let allowance = Allowance::required(Some(tokio::time::Instant::now()+std::time::Duration::from_secs(5))).unwrap();
    let prompt = admission.acquire_prompt(&identity,allowance).unwrap();
    let mut admission_changes = admission.subscribe();
    // Only this new synthetic database is touched; no retained private evidence.
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_notebook BEFORE INSERT ON notebooks BEGIN SELECT RAISE(FAIL,'synthetic notebook fault'); END;").unwrap();
    let failure = handle.write_notebook(seat,WriteMode::Replace,"synthetic").await;
    let closed_notification = tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            if admission_changes.borrow_and_update().status == cna_seats::host::HostStatus::Unknown { break; }
            admission_changes.changed().await.map_err(|_|HostRefusal::Unknown)?;
        }
        Ok::<_,HostRefusal>(())
    }).await;
    let closed = admission.snapshot();
    let retained = handle.inner.thread.lock().unwrap().as_ref().is_some_and(|thread|!thread.is_finished());
    // Settlement is awaited and the original writer joined BEFORE assertions,
    // including when the injected storage behavior regresses.
    let settlement = prompt.settle().await;
    let shutdown = handle.shutdown().await;
    assert!(matches!(failure,Err(Error::Storage(_))));
    closed_notification.unwrap().unwrap();
    assert!(matches!(closed,Err(HostRefusal::Unknown)));
    assert!(retained,"fatal writer must keep the reserved original ACK path");
    settlement.unwrap();
    assert!(shutdown.is_err(),"actual writer storage error remains an error after settlement");
    assert!(handle.inner.thread.lock().unwrap().is_none());
}
impl Ruleset for NullPolicyRules {
    type State = u8;
    type Content = ();
    fn profile_id(&self) -> &str {
        "null-policy-test-v1"
    }
    fn advance(&self, _: &(), state: &mut u8, _: &mut Cx<'_>) -> Result<Progress, EngineError> {
        Ok(if *state == 0 {
            Progress::AwaitingDecisions
        } else {
            Progress::Finished {
                summary: "accepted".into(),
            }
        })
    }
    fn respond(
        &self,
        _: &(),
        state: &mut u8,
        _: &DecisionResponse,
        _: &mut Cx<'_>,
    ) -> Result<(), Rejection> {
        // Deliberately permissive: the actor must enforce its policy contract before evaluation,
        // even if a ruleset would accept the sentinel as an invented action.
        self.responses.fetch_add(1, Ordering::SeqCst);
        *state = 1;
        Ok(())
    }
    fn pending(&self, _: &(), state: &u8) -> Vec<DecisionRequest> {
        if *state != 0 {
            return vec![];
        }
        let mut space = ActionSpace::new(ActionSchema::Integer { min: 1, max: 1 });
        if self.pass {
            space.pass = Some("declared done".into());
        }
        vec![DecisionRequest {
            id: "mandatory-1".into(),
            seat: SeatId::all().next().unwrap(),
            kind: "test.mandatory.allocation".into(),
            revision: 1,
            clock: Clock::start(1, Anchor::new("test.allocation")),
            summary: "Allocate exactly".into(),
            rules: vec![],
            trigger: Trigger::Scheduled,
            secrecy: Secrecy::Secret,
            space,
        }]
    }
    fn observe(&self, _: &(), state: &u8, _: Perspective) -> Value {
        json!(state)
    }
    fn view(&self, _: &(), _: &u8, _: Perspective) -> ViewState {
        ViewState {
            clock: cna_protocol::Clock {
                game_turn: 1,
                date: "1940-09-15".into(),
                stage: "test".into(),
                op_stage: None,
                phase: "allocation".into(),
                segment: None,
                step: None,
                phasing: None,
            },
            stacks: vec![],
            units: BTreeMap::new(),
            markers: vec![],
            pending: vec![],
        }
    }
}

#[test]
fn null_policy_without_declared_pass_pauses_before_any_command_and_survives_restart() {
    for pass in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.sqlite");
        let responses = Arc::new(AtomicUsize::new(0));
        let pins = Pins {
            rules_profile: "null-policy-test-v1".into(),
            content_hash: "empty".into(),
            engine_version: "policy-test-v1".into(),
        };
        let mut campaign = Campaign::create(
            &path,
            NullPolicyRules {
                pass,
                responses: responses.clone(),
            },
            (),
            Game {
                state: 0,
                rng: CampaignRng::from_seed([7; 32]).state(),
            },
            CampaignMeta {
                id: "policy".into(),
                scenario_id: "test".into(),
                rules_profile: pins.rules_profile.clone(),
                title: "Policy".into(),
                seats: vec![],
            },
            pins.clone(),
        )
        .unwrap();
        let seat = SeatId::all().next().unwrap();
        campaign
            .handover(
                seat,
                Some(ControllerInfo {
                    kind: ControllerKind::Scripted,
                    label: "legal-random".into(),
                }),
                json!({"mode":"legal_random"}),
            )
            .unwrap();
        let before = campaign.state_hash().unwrap();
        let rng = campaign.game.rng.clone();
        let pending = campaign.pending();
        let calls = Arc::new(AtomicUsize::new(0));
        let policy_calls = calls.clone();
        let policy: ActionPolicy<NullPolicyRules> = Box::new(move |_, _, _, _| {
            policy_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Some(Value::Null))
        });
        let step = auto_step(&mut campaign, None, &NoCandidates, Some(&policy)).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        let commands: u64 = db
            .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0))
            .unwrap();
        assert_eq!(commands, u64::from(pass));
        assert_eq!(responses.load(Ordering::SeqCst), usize::from(pass));
        assert_eq!(campaign.game.rng, rng);
        if pass {
            assert!(matches!(step, Step::Responded { seat: s } if s == seat));
            assert!(!campaign.binding(seat).paused);
            continue;
        }
        assert!(matches!(step, Step::SeatPaused { seat: s, ref error }
            if s == seat && error.contains("explicit owner allocation required")));
        assert_eq!(campaign.state_hash().unwrap(), before);
        assert_eq!(campaign.pending(), pending);
        assert!(campaign.binding(seat).paused);
        let rows = campaign
            .transcripts_after(Perspective::Seat(seat), seat, 0, 10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(matches!(&rows[0], ServerMessage::Transcript {
            entry: TranscriptEntry::System { text }, ..
        } if text.contains("explicit owner allocation required")));
        let enemy = SeatId::all().find(|s| s.side != seat.side).unwrap();
        assert!(
            campaign
                .transcripts_after(Perspective::Side(enemy.side), seat, 0, 10)
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            auto_step(&mut campaign, None, &NoCandidates, Some(&policy)).unwrap(),
            Step::Idle
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(responses.load(Ordering::SeqCst), 0);
        drop(campaign);
        let recovered =
            Campaign::recover(&path, NullPolicyRules { pass, responses }, (), &pins).unwrap();
        assert_eq!(recovered.state_hash().unwrap(), before);
        assert_eq!(recovered.game.rng, rng);
        assert_eq!(recovered.pending(), pending);
        assert!(recovered.binding(seat).paused);
        assert_eq!(
            recovered
                .transcripts_after(Perspective::Seat(seat), seat, 0, 10)
                .unwrap(),
            rows
        );
    }
}
