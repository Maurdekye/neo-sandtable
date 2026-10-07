use super::*;
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
