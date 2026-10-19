use super::*;
use crate::diagnostic::{PrivatePayload, record};

#[derive(Clone, Copy)]
enum Mode { Ok, Engine, Panic, Nested }
struct Probe { mode: Mode, capture: bool }

fn game() -> Game<Probe> {
    Game { state: 0, rng: CampaignRng::from_seed([7; 32]).state() }
}
fn evidence(bytes: &[u8]) {
    record(|| PrivatePayload::from_encoded(bytes.to_vec()).unwrap());
}
fn nested_rule(cx: &mut Cx<'_>, capture: bool) {
    cx.rng.d6();
    if capture { evidence(b"first"); }
}
impl Ruleset for Probe {
    type State = u8;
    type Content = ();
    fn profile_id(&self) -> &str { "diagnostic-test" }
    fn advance(&self, _: &(), state: &mut u8, cx: &mut Cx<'_>) -> Result<Progress, EngineError> {
        *state += 1;
        let mut private_events = Vec::new();
        let mut nested_cx = Cx { rng: cx.rng, events: &mut private_events };
        // The public Cx shape is unchanged and a nested context must not lose capture.
        nested_rule(&mut nested_cx, self.capture && !matches!(self.mode, Mode::Nested));
        match self.mode {
            Mode::Ok => Ok(Progress::Finished { summary: "complete".into() }),
            Mode::Engine => {
                if self.capture { record(|| panic!("later cause encoder must not run")); }
                Err(EngineError::Invariant { detail: "fixed".into() })
            }
            Mode::Panic => panic!("evaluation panic"),
            Mode::Nested => {
                let (_, inner) = evaluate_diagnosed(&Probe { mode: Mode::Engine, capture: true }, &(), &game(), &Command::Advance);
                assert_eq!(inner.unwrap().as_bytes(), b"first");
                evidence(b"outer");
                Err(EngineError::Invariant { detail: "fixed".into() })
            }
        }
    }
    fn respond(&self, _: &(), _: &mut u8, _: &DecisionResponse, _: &mut Cx<'_>) -> Result<(), Rejection> {
        if self.capture { evidence(b"illegal rejected draft"); }
        Err(Rejection::Illegal { message: "fixed".into() })
    }
    fn pending(&self, _: &(), _: &u8) -> Vec<DecisionRequest> { vec![] }
    fn observe(&self, _: &(), _: &u8, _: Perspective) -> Value { Value::Null }
    fn view(&self, _: &(), _: &u8, _: Perspective) -> cna_protocol::ViewState {
        panic!("evaluation must not construct a view")
    }
}

#[test]
fn nested_cx_records_and_first_cause_wins() {
    let (result, payload) = evaluate_diagnosed(&Probe { mode: Mode::Engine, capture: true }, &(), &game(), &Command::Advance);
    assert!(matches!(result, Err(Rejection::Engine(_))));
    assert_eq!(payload.unwrap().as_bytes(), b"first");
}

#[test]
fn successful_and_illegal_evaluations_discard_private_capture() {
    let probe = Probe { mode: Mode::Ok, capture: true };
    let (result, payload) = evaluate_diagnosed(&probe, &(), &game(), &Command::Advance);
    assert!(result.is_ok());
    assert!(payload.is_none());
    let command = Command::Respond(DecisionResponse {
        decision_id: "test".into(), seat: SeatId::all().next().unwrap(),
        controller_epoch: 0, decision_revision: 0, idempotency_key: "key".into(),
        action: Value::Null, public_explanation: None,
    });
    let (result, payload) = evaluate_diagnosed(&probe, &(), &game(), &command);
    assert!(matches!(result, Err(Rejection::Illegal { .. })));
    assert!(payload.is_none());
}

#[test]
fn panic_restores_scope_and_next_call_has_no_stale_evidence() {
    let panic = std::panic::catch_unwind(|| {
        evaluate_diagnosed(&Probe { mode: Mode::Panic, capture: true }, &(), &game(), &Command::Advance)
    });
    assert!(panic.is_err());
    record(|| panic!("panicked scope remained installed"));
    let (_, payload) = evaluate_diagnosed(&Probe { mode: Mode::Engine, capture: false }, &(), &game(), &Command::Advance);
    assert!(payload.is_none());
}

#[test]
fn nested_diagnosed_evaluation_restores_outer_scope() {
    let (_, payload) = evaluate_diagnosed(&Probe { mode: Mode::Nested, capture: true }, &(), &game(), &Command::Advance);
    assert_eq!(payload.unwrap().as_bytes(), b"outer");
    record(|| panic!("outer scope remained installed"));
}

#[test]
fn ordinary_evaluation_is_identical_when_record_calls_are_hit() {
    let input = game();
    let before = serde_json::to_value(&input).unwrap();
    for mode in [Mode::Ok, Mode::Engine] {
        let plain = evaluate(&Probe { mode, capture: false }, &(), &input, &Command::Advance);
        let with_record = evaluate(&Probe { mode, capture: true }, &(), &input, &Command::Advance);
        match (plain, with_record) {
            (Ok(a), Ok(b)) => assert_eq!(serde_json::to_value((&a.game, a.events, a.progress)).unwrap(), serde_json::to_value((&b.game, b.events, b.progress)).unwrap()),
            (Err(a), Err(b)) => assert_eq!(a, b),
            _ => panic!("capture changed ordinary evaluation outcome"),
        }
    }
    assert_eq!(serde_json::to_value(&input).unwrap(), before);
}

#[test]
fn diagnosed_capture_keeps_exact_game_rng_events_progress_and_rejection() {
    let input = game();
    let before = serde_json::to_value(&input).unwrap();
    for mode in [Mode::Ok, Mode::Engine] {
        let plain = evaluate(&Probe { mode, capture: false }, &(), &input, &Command::Advance);
        let (diagnosed, payload) = evaluate_diagnosed(
            &Probe { mode, capture: true }, &(), &input, &Command::Advance,
        );
        match (plain, diagnosed) {
            (Ok(a), Ok(b)) => {
                assert_eq!(serde_json::to_value((&a.game,a.events,a.progress)).unwrap(),
                    serde_json::to_value((&b.game,b.events,b.progress)).unwrap());
                assert!(payload.is_none(),"successful private capture is discarded");
            }
            (Err(a), Err(b)) => {
                assert_eq!(a,b);
                assert_eq!(payload.unwrap().as_bytes(),b"first");
            }
            _ => panic!("diagnosed scope changed ordinary evaluation"),
        }
        assert_eq!(serde_json::to_value(&input).unwrap(),before,"failed draft never mutates input RNG/state");
    }
}

#[test]
fn diagnosed_scope_evaluates_the_original_rule_once() {
    use std::sync::atomic::{AtomicUsize,Ordering};
    struct Counted(AtomicUsize);
    impl Ruleset for Counted {
        type State = u8;
        type Content = ();
        fn profile_id(&self) -> &str { "counted-diagnostic-test" }
        fn advance(&self, _: &(), state:&mut u8,cx:&mut Cx<'_>) -> Result<Progress,EngineError> {
            self.0.fetch_add(1,Ordering::SeqCst);
            *state += 1;
            cx.rng.d6();
            evidence(b"same-evaluation");
            Err(EngineError::Invariant {detail:"fixed".into()})
        }
        fn respond(&self,_:&(),_:&mut u8,_:&DecisionResponse,_:&mut Cx<'_>) -> Result<(),Rejection> {
            panic!("advance proof must not invoke respond")
        }
        fn pending(&self,_:&(),_:&u8) -> Vec<DecisionRequest> { vec![] }
        fn observe(&self,_:&(),_:&u8,_:Perspective) -> Value { Value::Null }
        fn view(&self,_:&(),_:&u8,_:Perspective) -> cna_protocol::ViewState {
            panic!("diagnosed evaluator must not project a view")
        }
    }
    let rule = Counted(AtomicUsize::new(0));
    let input = Game::<Counted> {state:0,rng:CampaignRng::from_seed([7;32]).state()};
    let before = serde_json::to_value(&input).unwrap();
    let (result,payload) = evaluate_diagnosed(&rule,&(),&input,&Command::Advance);
    assert!(matches!(result,Err(Rejection::Engine(_))));
    assert_eq!(rule.0.load(Ordering::SeqCst),1);
    assert_eq!(payload.unwrap().as_bytes(),b"same-evaluation");
    assert_eq!(serde_json::to_value(&input).unwrap(),before);
}
